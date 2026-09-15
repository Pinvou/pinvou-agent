import { useEffect, useMemo, useRef, useState } from 'react';
import { ArrowLeft, ChevronDown, ChevronRight, X } from '../../components/icons.jsx';
import { RightDockPanel } from '../../components/layout/RightDock.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { ConversationTimeline } from '../conversation/ConversationTimeline.jsx';
import { commandExecutionDetails, terminalStatus } from '../conversation/conversation-model.js';
import { AppIcon } from '../personas/persona-shared.jsx';
import {
  fileChangeStat,
  projectSubagentTranscript,
  resolveSubagentPresentation,
  subagentAncestorIds,
  subagentRoleOrdinals,
  visibleSubagentTreeRows,
  windowSubagentTranscript,
} from './subagent-conversation.mjs';
import { timelineDisplayError } from '../conversation/deepseek-conversation.js';
import {
  isTranscriptChunk,
  mergeTranscriptMessages,
  startSubagentTranscriptPolling,
  startTranscriptPolling,
} from './runState.mjs';

/**
 * 子智能体面板（Codex 式右侧列，ADR-0006）：**只读执行记录**，不是第二个
 * 聊天入口——与子智能体的一切互动都经父对话表达。
 *
 * 两级结构（CodexWorkspacePanel 同款）：列表（直属代理为根，历史后代折叠成树，
 * 含状态点/受阻标注）→ 详情（该实例的完整思考、工具与结果，共享对话时间线渲染）。
 * 数据全部来自底座落盘投影（transcripts::list / read_chunk），面板打开期间串行
 * 轮询刷新；App 不维护任何运行状态机。
 */

const TRANSCRIPT_WINDOW_STEP = 120;

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

// eslint-disable-next-line sonarjs/cognitive-complexity -- list/detail dual-state panel: poll reset, ancestor expansion, and window stepping share the same session lifecycle state; splitting would sever their coordination
export function SubagentTranscriptPanel({
  sessionId,
  initialAgentId,
  selectionRequestId,
  t,
  theme,
  language,
  modelServiceState,
  onClose,
}) {
  const copy = t.uiMultiAgent;
  const conversationCopy = t.uiConversation;
  const isDark = theme === 'dark';
  const personas = bridge.available && bridge.personas ? bridge.personas.getPersonas() : [];

  const [selectedAgentId, setSelectedAgentId] = useState(initialAgentId || null);
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- mirror once when the externally selected agent changes, to avoid showing stale details
    setSelectedAgentId(initialAgentId || null);
  }, [sessionId, initialAgentId, selectionRequestId]);

  // 列表：面板打开期间串行轮询底座落盘投影（含状态与受阻标注）。
  const [agents, setAgents] = useState(null);
  const [listReadFailed, setListReadFailed] = useState(false);
  const [listWake, setListWake] = useState(0);
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously reset list state on session switch; one-shot mirror
    setAgents(null);
    setListReadFailed(false);
  }, [sessionId]);
  useEffect(() => {
    if (!bridge.available) return;
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
    if (!listPollingDormant || typeof window === 'undefined') return;
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
    // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously clear the expansion set on session switch; one-shot mirror
    setExpandedAgentIds(new Set());
  }, [sessionId]);
  useEffect(() => {
    if (!selectedAgentId || !Array.isArray(agents)) return;
    const ancestors = subagentAncestorIds(agents, selectedAgentId);
    if (!ancestors.length) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously expand the ancestor chain after selecting an agent so the target row stays visible
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
  const [visibleTranscriptItems, setVisibleTranscriptItems] = useState(TRANSCRIPT_WINDOW_STEP);
  const transcriptCursorRef = useRef(null);
  const agentResolved = !!agent;
  const agentDone = !!(agent && agent.done);
  const transcriptUnavailable = !!(agent && agent.has_transcript === false);
  const terminalWithoutTranscript = agentDone && transcriptUnavailable;
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously reset the visible window step when switching agents
    setVisibleTranscriptItems(TRANSCRIPT_WINDOW_STEP);
  }, [sessionId, selectedAgentId]);
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously clear old records and reset read state when switching agents
    setMessages(null);
    setTranscriptReadFailed(false);
    transcriptCursorRef.current = null;
    // 排队/刚启动（ledger 有、transcript 未落盘）不轮询详情：读必失败，
    // 徒增无效 IPC 与告警日志；若它在建文件前已经失败，直接展示 ledger
    // 错误，不再把明确的启动失败伪装成“记录读取失败”。
    return startSubagentTranscriptPolling({
      bridgeAvailable: bridge.available,
      selectedAgentId,
      agentResolved,
      transcriptUnavailable,
      agentDone,
      read: () => bridge.multiAgent.readSubagentTranscript(
        sessionId,
        selectedAgentId,
        transcriptCursorRef.current,
      ),
      accept: isTranscriptChunk,
      onMessages: (chunk) => {
        if (chunk) {
          transcriptCursorRef.current = {
            offset: chunk.next_offset,
            revision: chunk.revision,
          };
          setMessages((current) => mergeTranscriptMessages(current, chunk));
          setTranscriptReadFailed(false);
        } else {
          setTranscriptReadFailed(true);
        }
      },
    });
  }, [sessionId, selectedAgentId, agentResolved, agentDone, transcriptUnavailable]);

  const projectedAgentId = agent ? agent.agent_id : null;
  const projectedAgentRole = agent ? agent.role : null;
  const projectedAgentStatus = agent ? agent.status : null;
  const projectedAgentDone = !!(agent && agent.done);
  const projectedAgentFailed = !!(agent && agent.failed);
  const projectedAgentBlocked = !!(agent && agent.blocked);
  const projectedAgentError = agent ? agent.error : null;
  const projectedAgent = useMemo(
    () => (projectedAgentId
      ? {
          agentId: projectedAgentId,
          role: projectedAgentRole,
          status: projectedAgentStatus,
          done: projectedAgentDone,
          failed: projectedAgentFailed,
          blocked: projectedAgentBlocked,
          error: projectedAgentError,
        }
      : null),
    [
      projectedAgentId,
      projectedAgentRole,
      projectedAgentStatus,
      projectedAgentDone,
      projectedAgentFailed,
      projectedAgentBlocked,
      projectedAgentError,
    ],
  );
  const projected = useMemo(
    () => projectSubagentTranscript({
      messages: messages || [],
      agent: projectedAgent,
      options: { language, modelServiceState },
    }),
    [messages, projectedAgent, language, modelServiceState],
  );
  const transcriptWindow = useMemo(
    () => windowSubagentTranscript(projected, visibleTranscriptItems),
    [projected, visibleTranscriptItems],
  );

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
  const detailSubtitle = detailPresentation ? detailPresentation.subtitle : null;

  return (
    <RightDockPanel
      panelId="subagent-transcript"
      activationKey={selectionRequestId}
      className="border-l border-black/[0.06] bg-white/92 backdrop-blur-xl dark:border-white/[0.07] dark:bg-[#17181A]/96"
      dataTestId="subagent-transcript-panel"
    >
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
                card={{ id: detailIdentity.avatarKey, name: detailSubtitle || detailName, dept: detailIdentity.personaDept }}
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
              <div
                className="truncate text-[10px] text-gray-400"
                title={detailSubtitle ? `${detailSubtitle} · ${selectedAgentId}` : selectedAgentId}
              >
                {detailSubtitle || selectedAgentId}
              </div>
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
          {terminalWithoutTranscript ? (
            <div className={`whitespace-pre-wrap break-words text-[12px] ${
              agent && agent.failed
                ? 'text-red-600 dark:text-red-400'
                : 'text-amber-600 dark:text-amber-400'
            }`}>
              {copy.agentNoTranscript(
                agent && agent.error
                  ? timelineDisplayError(agent.error, { language })
                  : null,
              )}
            </div>
          ) : messages === null && agent && agent.has_transcript === false && !agent.done ? (
            <div className="text-[12px] text-gray-400">{copy.agentPending}</div>
          ) : messages === null && !transcriptReadFailed ? (
            <div className="text-[12px] text-gray-400">{copy.loadingTranscript}</div>
          ) : null}
          {!terminalWithoutTranscript && transcriptReadFailed && (
            <div className="mb-2 text-[12px] text-amber-600 dark:text-amber-400">{copy.transcriptReadFailed}</div>
          )}
          {!terminalWithoutTranscript && Array.isArray(messages) && messages.length === 0 && (
            <div className="text-[12px] text-gray-400">{copy.emptyTranscript}</div>
          )}
          {!terminalWithoutTranscript && Array.isArray(messages) && messages.length > 0 && (
            <>
              {transcriptWindow.hiddenCount > 0 && (
                <button
                  type="button"
                  onClick={() => setVisibleTranscriptItems((count) => count + TRANSCRIPT_WINDOW_STEP)}
                  className="mb-3 w-full rounded-lg border border-black/[0.06] px-3 py-2 text-[11px] text-gray-500 hover:bg-black/[0.03] dark:border-white/[0.08] dark:text-gray-400 dark:hover:bg-white/[0.04]"
                >
                  {copy.showEarlierTranscript(Math.min(
                    TRANSCRIPT_WINDOW_STEP,
                    transcriptWindow.hiddenCount,
                  ))}
                </button>
              )}
              <ConversationTimeline
                turns={transcriptWindow.view.turns}
                now={0}
                copy={conversationCopy}
                agentLabel={detailSubtitle || detailName}
                assistantAvatar={detailIdentity ? (
                  <AppIcon
                    card={{ id: detailIdentity.avatarKey, name: detailSubtitle || detailName, dept: detailIdentity.personaDept }}
                    isDark={isDark}
                    cls="mt-1 h-7 w-7 shrink-0 overflow-hidden rounded-xl"
                    fb={13}
                  />
                ) : undefined}
                renderToolItem={(item) => (
                  <CompactToolRow item={item} conversationCopy={conversationCopy} />
                )}
              />
            </>
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
            const { identity, name, subtitle } = presentation;
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
                    card={{ id: identity.avatarKey, name: subtitle || name, dept: identity.personaDept }}
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
                    <span
                      className="block truncate text-[10px] text-gray-400"
                      title={[subtitle, presentation.task, entry.agent_id].filter(Boolean).join(' · ')}
                    >
                      {subtitle || presentation.task || entry.agent_id}
                    </span>
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
    </RightDockPanel>
  );
}
