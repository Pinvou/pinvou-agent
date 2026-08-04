import React, { useEffect, useMemo, useRef, useState } from 'react';
import { ArrowLeft, ChevronDown, ChevronRight, X } from '../../components/icons.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { ConversationTimeline } from '../conversation/ConversationTimeline.jsx';
import { commandExecutionDetails, terminalStatus } from '../conversation/conversation-model.js';
import { AppIcon } from '../personas/Personas.jsx';
import {
  fileChangeStat,
  projectSubagentTranscript,
  resolveSubagentPresentation,
  subagentAncestorIds,
  subagentRoleOrdinals,
  visibleSubagentTreeRows,
} from './subagent-conversation.mjs';
import { startTranscriptPolling } from './runState.mjs';

/**
 * 子智能体面板（Codex 式右侧列，ADR-0006）：**只读执行记录**，不是第二个
 * 聊天入口——与子智能体的一切互动都经父对话表达。
 *
 * 两级结构（CodexWorkspacePanel 同款）：列表（直属代理为根，历史后代折叠成树，
 * 含状态点/受阻标注）→ 详情（该实例的完整思考、工具与结果，共享对话时间线渲染）。
 * 数据全部来自底座落盘投影（transcripts::list / read），面板打开期间串行
 * 轮询刷新；App 不维护任何运行状态机。
 */

const PANEL_WIDTH_KEY = 'pinvou_subagent_panel_width';
const PANEL_MIN_WIDTH = 360;
const CHAT_MIN_WIDTH = 360;
const PANEL_MAX_RATIO = 0.65;
const PANEL_DEFAULT_WIDTH = 420;

function clampPanelWidth(width, rootWidth) {
  const maximum = Math.max(
    PANEL_MIN_WIDTH,
    Math.min(Math.round(rootWidth * PANEL_MAX_RATIO), rootWidth - CHAT_MIN_WIDTH),
  );
  return Math.max(PANEL_MIN_WIDTH, Math.min(Math.round(width), maximum));
}

function savedPanelWidth() {
  try {
    const value = Number.parseInt(localStorage.getItem(PANEL_WIDTH_KEY) || '', 10);
    return Number.isFinite(value) && value >= PANEL_MIN_WIDTH ? value : PANEL_DEFAULT_WIDTH;
  } catch {
    return PANEL_DEFAULT_WIDTH;
  }
}

function rememberPanelWidth(width) {
  try {
    localStorage.setItem(PANEL_WIDTH_KEY, String(Math.round(width)));
  } catch {
    // localStorage 不可用时只保留当前窗口内的宽度。
  }
}

/** Codex 式紧凑工具行：图标底 + 标题 + 一行 meta，点开看原始入出参。 */
function CompactToolRow({ item, conversationCopy }) {
  const [open, setOpen] = useState(false);
  const c = conversationCopy;
  const tool = item.tool || {};
  const state = terminalStatus(item.status);
  let title = tool.title || tool.name || c.tool;
  let extra = '';
  if (item.type === 'command_execution') {
    title = commandExecutionDetails(tool).summary;
  } else if (item.type === 'file_change') {
    const location = tool.locations && tool.locations[0];
    if (location && location.path) title = location.path;
    const stat = fileChangeStat(tool.rawOutput);
    if (stat) extra = ` · +${stat.added} -${stat.removed}`;
  }
  const label = item.type === 'file_change'
    ? c.fileChange
    : item.type === 'command_execution' ? c.command : c.tool;
  const stateText = state === 'running'
    ? c.inProgress
    : state === 'failed' ? c.failed : c.executionFinished;
  return (
    <div className="rounded-xl border border-black/[0.05] dark:border-white/[0.07] bg-white/45 dark:bg-white/[0.015]">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        className="w-full px-2.5 py-2 flex items-center gap-2 text-left"
      >
        <span className={`w-1.5 h-1.5 shrink-0 rounded-full ${
          state === 'failed' ? 'bg-red-500' : state === 'running' ? 'bg-blue-500 animate-pulse' : 'bg-gray-300 dark:bg-gray-600'
        }`} />
        <span className="min-w-0 flex-1 truncate text-[12px] text-gray-700 dark:text-gray-300 font-mono">
          {title}
        </span>
        <span className="shrink-0 text-[10px] text-gray-400">{label} · {stateText}{extra}</span>
      </button>
      {open && (
        <div className="px-2.5 pb-2.5 border-t border-black/[0.05] dark:border-white/[0.06] space-y-1.5">
          {tool.rawInput != null && (
            <pre className="custom-scrollbar mt-2 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-lg bg-black/[0.03] dark:bg-white/[0.04] p-2 text-[10.5px] leading-4 text-gray-600 dark:text-gray-300">
              {typeof tool.rawInput === 'string' ? tool.rawInput : JSON.stringify(tool.rawInput, null, 2)}
            </pre>
          )}
          {tool.rawOutput != null && (
            <pre className="custom-scrollbar max-h-48 overflow-auto whitespace-pre-wrap break-words rounded-lg bg-black/[0.03] dark:bg-white/[0.04] p-2 text-[10.5px] leading-4 text-gray-600 dark:text-gray-300">
              {typeof tool.rawOutput === 'string' ? tool.rawOutput : JSON.stringify(tool.rawOutput, null, 2)}
            </pre>
          )}
        </div>
      )}
    </div>
  );
}

export function SubagentTranscriptPanel({
  sessionId,
  initialAgentId,
  selectionRequestId,
  t,
  theme,
  onClose,
}) {
  const copy = t.uiMultiAgent;
  const conversationCopy = t.uiConversation;
  const isDark = theme === 'dark';
  const personas = bridge.available && bridge.personas ? bridge.personas.getPersonas() : [];

  const [selectedAgentId, setSelectedAgentId] = useState(initialAgentId || null);
  useEffect(() => {
    setSelectedAgentId(initialAgentId || null);
  }, [sessionId, initialAgentId, selectionRequestId]);

  // 列表：面板打开期间串行轮询底座落盘投影（含状态与受阻标注）。
  const [agents, setAgents] = useState(null);
  const [listReadFailed, setListReadFailed] = useState(false);
  const [listWake, setListWake] = useState(0);
  useEffect(() => {
    setAgents(null);
    setListReadFailed(false);
  }, [sessionId]);
  useEffect(() => {
    if (!bridge.available) return undefined;
    return startTranscriptPolling({
      read: () => bridge.multiAgent.listSubagentTranscripts(sessionId),
      onMessages: (list) => {
        // null = 读取失败：保留上次有效清单并亮出重试提示，不能把故障
        // 伪装成"没有子智能体"（复核 P2）。
        if (Array.isArray(list)) {
          setAgents(list);
          setListReadFailed(false);
        } else {
          setListReadFailed(true);
        }
      },
      // 只在仍有运行中实例（或本次读取失败）时继续定时刷新。全部终态后
      // 停表；下方实时事件监听会在新子智能体出现时唤醒一次读取。
      active: (list) => !Array.isArray(list) || list.some((entry) => !entry.done),
      intervalMs: 2000,
    });
  }, [sessionId, listWake]);

  const listPollingDormant = Array.isArray(agents)
    && !listReadFailed
    && agents.every((entry) => entry.done);
  useEffect(() => {
    if (!listPollingDormant || typeof window === 'undefined') return undefined;
    const wakeForNewAgent = (event) => {
      const detail = event && event.detail;
      if (!detail || detail.sessionId !== sessionId || !detail.agentId) return;
      setListWake((value) => value + 1);
    };
    window.addEventListener('pinvou:subagent-update', wakeForNewAgent);
    return () => window.removeEventListener('pinvou:subagent-update', wakeForNewAgent);
  }, [sessionId, listPollingDormant]);

  const agent = useMemo(() => {
    if (!selectedAgentId) return null;
    return (agents || []).find((entry) => entry.agent_id === selectedAgentId) || null;
  }, [agents, selectedAgentId]);

  // 同角色多实例的序号（按 ledger 登记序），与行内卡的轮询广播同源一致。
  const roleOrdinals = useMemo(() => subagentRoleOrdinals(agents || []), [agents]);
  const [expandedAgentIds, setExpandedAgentIds] = useState(() => new Set());
  useEffect(() => {
    setExpandedAgentIds(new Set());
  }, [sessionId]);
  useEffect(() => {
    if (!selectedAgentId || !Array.isArray(agents)) return;
    const ancestors = subagentAncestorIds(agents, selectedAgentId);
    if (!ancestors.length) return;
    setExpandedAgentIds((current) => {
      if (ancestors.every(id => current.has(id))) return current;
      return new Set([...current, ...ancestors]);
    });
  }, [agents, selectedAgentId]);
  const treeRows = useMemo(
    () => visibleSubagentTreeRows(agents || [], expandedAgentIds),
    [agents, expandedAgentIds],
  );
  const rootAgentCount = useMemo(
    () => visibleSubagentTreeRows(agents || [], new Set()).length,
    [agents],
  );
  const toggleAgentChildren = (agentId) => {
    setExpandedAgentIds((current) => {
      const next = new Set(current);
      if (next.has(agentId)) next.delete(agentId);
      else next.add(agentId);
      return next;
    });
  };

  const [messages, setMessages] = useState(null);
  const [transcriptReadFailed, setTranscriptReadFailed] = useState(false);
  useEffect(() => {
    setMessages(null);
    setTranscriptReadFailed(false);
    // 排队/刚启动（ledger 有、transcript 未落盘）不轮询详情：读必失败，
    // 徒增无效 IPC 与告警日志；清单轮询发现 transcript 出现后自然重启。
    const transcriptPending = !!(agent && agent.has_transcript === false && !agent.done);
    if (!bridge.available || !selectedAgentId || transcriptPending) return undefined;
    return startTranscriptPolling({
      read: () => bridge.multiAgent.readSubagentTranscript(sessionId, selectedAgentId),
      onMessages: (list) => {
        if (Array.isArray(list)) {
          setMessages(list);
          setTranscriptReadFailed(false);
        } else {
          setTranscriptReadFailed(true);
        }
      },
      active: !(agent && agent.done),
    });
  }, [sessionId, selectedAgentId, !!(agent && agent.done), !!(agent && agent.has_transcript === false)]);

  const projected = useMemo(
    () => projectSubagentTranscript({
      messages: messages || [],
      agent: agent
        ? {
            agentId: agent.agent_id,
            role: agent.role,
            status: agent.status,
            done: !!agent.done,
            failed: !!agent.failed,
            blocked: !!agent.blocked,
            error: agent.error,
          }
        : null,
    }),
    [messages, agent],
  );

  const [panelWidth, setPanelWidth] = useState(savedPanelWidth);
  const panelRef = useRef(null);
  const resizeCleanupRef = useRef(null);

  useEffect(() => {
    const clampToViewport = () => {
      const panel = panelRef.current;
      const rootWidth = panel?.parentElement?.getBoundingClientRect().width || window.innerWidth;
      setPanelWidth((current) => clampPanelWidth(current, rootWidth));
    };
    clampToViewport();
    window.addEventListener('resize', clampToViewport);
    return () => window.removeEventListener('resize', clampToViewport);
  }, []);

  useEffect(() => () => {
    if (resizeCleanupRef.current) resizeCleanupRef.current();
  }, []);

  function startPanelResize(event) {
    event.preventDefault();
    const panel = panelRef.current;
    const rootRect = panel?.parentElement?.getBoundingClientRect();
    if (!panel || !rootRect) return;
    if (resizeCleanupRef.current) resizeCleanupRef.current();
    const maximum = Math.max(
      PANEL_MIN_WIDTH,
      Math.min(Math.round(rootRect.width * PANEL_MAX_RATIO), rootRect.width - CHAT_MIN_WIDTH),
    );
    let nextWidth = panelWidth;
    let frame = 0;
    const onMove = (moveEvent) => {
      nextWidth = Math.max(
        PANEL_MIN_WIDTH,
        Math.min(rootRect.right - moveEvent.clientX, maximum),
      );
      if (frame) return;
      frame = window.requestAnimationFrame(() => {
        frame = 0;
        panel.style.width = `${nextWidth}px`;
      });
    };
    const cleanup = () => {
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup', onUp);
      window.removeEventListener('blur', onUp);
      if (frame) window.cancelAnimationFrame(frame);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      resizeCleanupRef.current = null;
    };
    const onUp = () => {
      cleanup();
      setPanelWidth(nextWidth);
      rememberPanelWidth(nextWidth);
    };
    resizeCleanupRef.current = cleanup;
    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup', onUp);
    window.addEventListener('blur', onUp);
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  }

  function resetPanelWidth() {
    const panel = panelRef.current;
    const rootWidth = panel?.parentElement?.getBoundingClientRect().width || window.innerWidth;
    const nextWidth = clampPanelWidth(PANEL_DEFAULT_WIDTH, rootWidth);
    setPanelWidth(nextWidth);
    rememberPanelWidth(nextWidth);
  }

  const detailPresentation = agent
    ? resolveSubagentPresentation({
      role: agent.role,
      agentType: agent.agent_type,
      sessionName: agent.session_name,
      objective: agent.objective,
      personas,
      agentId: agent.agent_id,
      roleCards: copy.roleCards,
      ordinal: roleOrdinals.get(agent.agent_id),
    })
    : null;
  const detailIdentity = detailPresentation && detailPresentation.identity;
  const detailName = detailPresentation ? detailPresentation.name : selectedAgentId;

  return (
    <aside
      ref={panelRef}
      style={{ width: `${panelWidth}px` }}
      className="flex relative max-w-[88vw] min-w-0 shrink-0 border-l border-black/[0.06] dark:border-white/[0.07] bg-white/92 dark:bg-[#17181A]/96 backdrop-blur-xl flex-col"
      data-testid="subagent-transcript-panel"
    >
      <div
        role="separator"
        aria-label={copy.panelResize}
        aria-orientation="vertical"
        onMouseDown={startPanelResize}
        onDoubleClick={resetPanelWidth}
        className="absolute inset-y-0 left-0 z-20 w-1.5 -translate-x-1/2 cursor-col-resize bg-black/10 hover:bg-[#0B57D0]/50 dark:bg-white/10 dark:hover:bg-[#A8C7FA]/60 transition-colors"
        title={copy.panelResizeHint}
      />
      <div className="h-14 shrink-0 px-3 flex items-center gap-2 border-b border-black/[0.05] dark:border-white/[0.06]">
        {selectedAgentId ? (
          <>
            <button
              type="button"
              onClick={() => setSelectedAgentId(null)}
              className="w-7 h-7 rounded-lg flex items-center justify-center text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]"
              aria-label={copy.backToAgents}
            >
              <ArrowLeft size={14} />
            </button>
            {detailIdentity && (
              <AppIcon
                card={{ id: detailIdentity.avatarKey, name: detailName, dept: detailIdentity.personaDept }}
                isDark={isDark}
                cls="h-8 w-8 shrink-0 overflow-hidden rounded-[10px]"
                fb={14}
              />
            )}
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-1.5">
                <span className="truncate text-[13px] font-semibold">{copy.drawerTitle(detailName)}</span>
                {agent && agent.done && !agent.failed && agent.blocked && (
                  <span className="shrink-0 rounded-full bg-orange-500/10 px-1.5 py-px text-[9.5px] text-orange-600 dark:text-orange-300">
                    {copy.blockedTag}
                  </span>
                )}
              </div>
              <div className="truncate text-[10px] text-gray-400" title={selectedAgentId}>{selectedAgentId}</div>
            </div>
          </>
        ) : (
          <div className="min-w-0 flex flex-1 items-center gap-2">
            <span className="text-[13px] font-semibold">{copy.agentsListTitle}</span>
            {Array.isArray(agents) && agents.length > 0 && (
              <span className="truncate text-[10px] text-gray-400">
                {copy.agentsListSummary(rootAgentCount, agents.length)}
              </span>
            )}
          </div>
        )}
        <button
          type="button"
          onClick={onClose}
          className="w-7 h-7 rounded-lg flex items-center justify-center text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]"
          aria-label={copy.close}
        >
          <X size={14} />
        </button>
      </div>

      {selectedAgentId ? (
        <div className="custom-scrollbar flex-1 min-h-0 overflow-y-auto px-4 py-4">
          {messages === null && agent && agent.has_transcript === false && !agent.done ? (
            <div className="text-[12px] text-gray-400">{copy.agentPending}</div>
          ) : messages === null && transcriptReadFailed ? (
            <div className="text-[12px] text-amber-600 dark:text-amber-400">{copy.transcriptReadFailed}</div>
          ) : messages === null ? (
            <div className="text-[12px] text-gray-400">{copy.loadingTranscript}</div>
          ) : null}
          {Array.isArray(messages) && messages.length === 0 && (
            <div className="text-[12px] text-gray-400">{copy.emptyTranscript}</div>
          )}
          {Array.isArray(messages) && messages.length > 0 && (
            <ConversationTimeline
              turns={projected.turns}
              now={0}
              copy={conversationCopy}
              agentLabel={detailName}
              assistantAvatar={detailIdentity ? (
                <AppIcon
                  card={{ id: detailIdentity.avatarKey, name: detailName, dept: detailIdentity.personaDept }}
                  isDark={isDark}
                  cls="mt-1 h-7 w-7 shrink-0 overflow-hidden rounded-xl"
                  fb={13}
                />
              ) : undefined}
              renderToolItem={(item) => (
                <CompactToolRow item={item} conversationCopy={conversationCopy} />
              )}
            />
          )}
        </div>
      ) : (
        <div className="custom-scrollbar flex-1 min-h-0 overflow-y-auto px-3 py-3 space-y-1.5">
          {listReadFailed && (
            <div className="px-2 pb-1 text-[11px] text-amber-600 dark:text-amber-400">{copy.listReadFailed}</div>
          )}
          {agents === null && !listReadFailed && (
            <div className="text-[12px] text-gray-400">{copy.loadingTranscript}</div>
          )}
          {Array.isArray(agents) && agents.length === 0 && (
            <div className="text-[12px] text-gray-400">{copy.agentsEmpty}</div>
          )}
          {Array.isArray(agents) && treeRows.map(({ entry, depth, childCount }) => {
            const presentation = resolveSubagentPresentation({
              role: entry.role,
              agentType: entry.agent_type,
              sessionName: entry.session_name,
              objective: entry.objective,
              personas,
              agentId: entry.agent_id,
              roleCards: copy.roleCards,
              ordinal: roleOrdinals.get(entry.agent_id),
            });
            const { identity, name } = presentation;
            const expanded = expandedAgentIds.has(entry.agent_id);
            return (
              <div
                key={entry.agent_id}
                className="flex min-w-0 items-center"
                style={{ marginLeft: `${Math.min(depth, 4) * 14}px` }}
              >
                {childCount > 0 ? (
                  <button
                    type="button"
                    onClick={() => toggleAgentChildren(entry.agent_id)}
                    className="flex h-7 w-6 shrink-0 items-center justify-center rounded-md text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]"
                    aria-label={expanded ? copy.collapseChildren(name) : copy.expandChildren(name)}
                    title={expanded ? copy.collapseChildren(name) : copy.expandChildren(name)}
                  >
                    {expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                  </button>
                ) : (
                  <span className="w-6 shrink-0" />
                )}
                <button
                  type="button"
                  onClick={() => setSelectedAgentId(entry.agent_id)}
                  className="flex min-w-0 flex-1 items-center gap-2.5 rounded-[12px] px-2 py-2 text-left hover:bg-black/[0.04] dark:hover:bg-white/[0.06]"
                >
                  <AppIcon
                    card={{ id: identity.avatarKey, name, dept: identity.personaDept }}
                    isDark={isDark}
                    cls="h-8 w-8 shrink-0 overflow-hidden rounded-[10px]"
                    fb={14}
                  />
                  <span className="min-w-0 flex-1">
                    <span className="flex min-w-0 items-center gap-1.5">
                      <span className="truncate text-[12.5px] font-semibold">{name}</span>
                      {childCount > 0 && (
                        <span className="shrink-0 rounded-full bg-black/[0.035] px-1.5 py-px text-[9px] text-gray-400 dark:bg-white/[0.06]">
                          {copy.childAgentCount(childCount)}
                        </span>
                      )}
                    </span>
                    <span className="block truncate text-[10px] text-gray-400">{presentation.task || entry.agent_id}</span>
                  </span>
                  {!entry.done && entry.has_transcript === false && (
                    <span className="shrink-0 rounded-full bg-amber-500/10 px-1.5 py-px text-[9.5px] text-amber-600 dark:text-amber-300">
                      {copy.pendingTag}
                    </span>
                  )}
                  {entry.done && !entry.failed && entry.blocked && (
                    <span className="shrink-0 rounded-full bg-orange-500/10 px-1.5 py-px text-[9.5px] text-orange-600 dark:text-orange-300">
                      {copy.blockedTag}
                    </span>
                  )}
                  <span
                    className="inline-block h-1.5 w-1.5 shrink-0 rounded-full"
                    style={{
                      background: entry.done
                        ? (entry.failed ? '#C5221F' : entry.blocked ? '#E8710A' : '#137333')
                        : '#F9AB00',
                    }}
                  />
                </button>
              </div>
            );
          })}
        </div>
      )}
    </aside>
  );
}
