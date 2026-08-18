import React, { useEffect, useMemo, useRef, useState } from 'react';
import { ChevronDown, ChevronRight, FileText, Wrench } from '../../components/icons.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { can } from '../../shared/platform.js';
import { expertDelegationText, isAgentWaitCall, isExpertDelegationCall } from '../conversation/conversation-model.js';
import {
  resolveSubagentPresentation,
  resolveSubagentSpawnResult,
  subagentRoleOrdinals,
  subagentTreeIsDone,
  visibleSubagentDescendantRows,
} from '../multiagent/subagent-conversation.mjs';
import { AppIcon } from '../personas/Personas.jsx';
import { QuestionChoiceCard } from '../conversation/QuestionChoiceCard.jsx';
import { AcShieldCheck, AcSparkles, ArtifactCard, DiffView, GrepView, ListDirView, OutputError, OutputPre, ReceiptBlock, ShellTextView, ShellView, StockQuoteCard, TODO_TOOLS, TodoView, WeatherCard, isQuietTool, isReceipt, isStockQuoteTool, isWeatherTool, looksDiff, outBox, parseReceipt, toolBasename, toolSummary, tryParseJson, tryTailJson } from './tool-common.jsx';

const isShellExecutionTool = name => [
  'exec_shell',
  'exec_shell_wait',
  'exec_wait',
  'task_shell_start',
  'task_shell_wait',
  'shell',
  'Bash',
].includes(name);

// P1-C：专家卡是桌面能力。Web 构建没有 multiAgent bridge（capability 关闭），
// 强行渲染专家卡会吞掉原生 agent 工具的输出、点开只得空面板——capability
// 关闭时走回通用工具卡。模块级常量：对一次构建恒定，不破坏 Hook 数量稳定。
const EXPERT_CARD_ENABLED = can('multiAgent');

/** worker ledger 的英文状态 token（agent_worker_status_name）。这些必须映射
 * i18n 文案；不在此列的 status 是实时进展短语（模型侧输出），原样展示。 */
const LEDGER_STATUS_TOKENS = new Set([
  'running', 'queued', 'pending', 'starting',
  'completed', 'failed', 'cancelled', 'canceled', 'stopped', 'interrupted',
]);

// Web 端的多智能体会话是只读的（桌面专属，ADR-0006）：计划裁决/受阻兜底
// 这类会触发新一轮模型执行的卡片操作，与输入框一同置灰。权威拦截在后端
// remote_control 漏斗（复核 P1），前端只是如实反馈。桌面端恒 false。
const multiAgentWebReadOnly = () => {
  if (can('multiAgent')) return false;
  const chat = (bridge.state && bridge.state.get && bridge.state.get('chat')) || {};
  return !!(chat.modeState && chat.modeState.multiAgent);
};

// ── 行内专家卡的权威状态兜底轮询（模块级共享，P1-3） ──
// 实时 DOM 事件可能丢（事件通道拥塞/进程重启后加载历史/主对话停止级联取消），
// 只靠它卡片会永久停在"工作中"。有未终态卡挂载时按 2s 轮询底座落盘投影
// （worker ledger 权威，含 blocked/interrupted），把快照经同一
// `pinvou:subagent-update` DOM 事件广播给全部卡；整份 ledger 另经
// `pinvou:subagent-ledger-update` 广播一次，供直属卡投影自己的后代树。所有
// 被展示的直属树终态即停表，新卡再启；无论卡片多少，每轮仍只做一次 IPC。
const expertCardWatch = new Map();
let expertPollTimer = null;
const expertLedgerSnapshots = new Map();

function expertWatchKey(sessionId, agentId) {
  return `${sessionId || ''}\u0000${agentId || ''}`;
}

function activeExpertSessionId() {
  if (!bridge.available || !bridge.state || typeof bridge.state.get !== 'function') return null;
  return (bridge.state.get('sessions') || {}).activeSessionId || null;
}

function stopExpertPoll() {
  if (expertPollTimer != null) {
    clearTimeout(expertPollTimer);
    expertPollTimer = null;
  }
}

function kickExpertPoll(delay = 2000) {
  if (expertPollTimer != null || typeof window === 'undefined') return;
  expertPollTimer = setTimeout(async () => {
    expertPollTimer = null;
    if (!bridge.available || !bridge.multiAgent) return;
    const sessionIds = [...new Set(
      [...expertCardWatch.values()]
        .filter(entry => entry && !entry.done && entry.sessionId)
        .map(entry => entry.sessionId),
    )];
    for (const sid of sessionIds) {
      try {
        const response = await bridge.multiAgent.listSubagentTranscripts(sid);
        if (!Array.isArray(response)) continue;
        const list = response;
        const snapshot = { sessionId: sid, agents: list };
        expertLedgerSnapshots.set(sid, snapshot);
        window.dispatchEvent(new CustomEvent('pinvou:subagent-ledger-update', {
          detail: snapshot,
        }));
        const ordinals = subagentRoleOrdinals(list);
        for (const summary of list) {
          const key = expertWatchKey(sid, summary && summary.agent_id);
          if (!summary || !summary.agent_id || !expertCardWatch.has(key)) continue;
          const ordinal = ordinals.get(summary.agent_id) || null;
          window.dispatchEvent(new CustomEvent('pinvou:subagent-update', {
            detail: {
              sessionId: sid,
              agentId: summary.agent_id,
              role: summary.role || null,
              status: summary.status || null,
              done: !!summary.done,
              failed: !!summary.failed,
              blocked: !!summary.blocked,
              has_transcript: !!summary.has_transcript,
              seq: ordinal ? ordinal.seq : null,
              roleCount: ordinal ? ordinal.count : null,
              source: 'ledger',
            },
          }));
        }
        for (const [key, entry] of expertCardWatch.entries()) {
          if (entry.sessionId !== sid) continue;
          expertCardWatch.set(key, {
            ...entry,
            done: subagentTreeIsDone(list, entry.agentId),
          });
        }
      } catch (_) {
        // 单次轮询失败不致命，下一轮重试。
      }
    }
    if ([...expertCardWatch.values()].some(entry => entry && !entry.done)) kickExpertPoll(2000);
  }, delay);
}

function watchExpertCard(sessionId, agentId) {
  const key = expertWatchKey(sessionId, agentId);
  const previous = expertCardWatch.get(key);
  expertCardWatch.set(key, {
    sessionId,
    agentId,
    done: Boolean(previous && previous.done),
    count: (previous?.count || 0) + 1,
  });
  kickExpertPoll(0);
  return () => {
    const current = expertCardWatch.get(key);
    if (current && current.count > 1) {
      expertCardWatch.set(key, { ...current, count: current.count - 1 });
    } else {
      expertCardWatch.delete(key);
      if (![...expertCardWatch.values()].some(entry => entry.sessionId === sessionId)) {
        expertLedgerSnapshots.delete(sessionId);
      }
    }
    if (!expertCardWatch.size) stopExpertPoll();
  };
}

function openSubagentTranscript(agentId, sessionId) {
  if (typeof window === 'undefined' || !agentId) return;
  window.dispatchEvent(new CustomEvent('pinvou:open-subagent', {
    detail: { agentId, sessionId: sessionId || null },
  }));
}

function expertStatusPresentation({ summary, failedSpawn = false, itemState, copy }) {
  const blocked = !!(summary && summary.done && !summary.failed && summary.blocked);
  const statusToken = String(summary?.status || '').toLowerCase();
  const pending = !!(
    summary
    && !summary.done
    && (summary.has_transcript === false || ['queued', 'pending', 'starting'].includes(statusToken))
  );
  const interrupted = !!(
    summary
    && summary.done
    && summary.failed
    && summary.status === 'interrupted'
  );
  const text = failedSpawn
    ? copy.agentCard.spawnFailed
    : blocked
      ? copy.blockedTag
      : interrupted
        ? copy.agentCard.interrupted
        : summary && summary.done
          ? (summary.failed ? copy.agentCard.failed : copy.agentCard.completed)
          : pending
            ? copy.pendingTag
            : summary
              ? (summary.status && !LEDGER_STATUS_TOKENS.has(String(summary.status).toLowerCase())
                ? summary.status
                : copy.agentCard.working)
              : itemState === 'running'
                ? copy.agentCard.spawning
                : copy.agentCard.working;
  const dotColor = failedSpawn || (summary && summary.done && summary.failed)
    ? '#C5221F'
    : blocked
      ? '#F9AB00'
      : summary && summary.done
        ? '#137333'
        : '#F9AB00';
  return { text, dotColor };
}

/**
 * 行内专家卡（ADR-0006）：spawn 型 `agent` 工具调用在消息流里渲染成
 * 「头像 · 任务名/专家身份 · 任务摘要 · 状态」，不展示工具 JSON；若它继续派生了
 * 子代，则在本卡下按 ledger 父链折叠展示，主对话也能看清委派层级。
 * 状态自订阅 `pinvou:subagent-update`（bridge 转发的实时事件 + 模块级
 * 轮询广播的落盘权威快照，终态 ratchet 保证落盘赢），点击整卡派发
 * `pinvou:open-subagent`，由 ChatView 打开只读执行记录面板。
 * status/wait/cancel 等协调操作渲染成安静的单行，不冒充新委派。
 */
const ExpertAgentCard = ({ item, t, sessionId: sessionIdProp }) => {
  const copy = t.uiMultiAgent;
  const args = item.args || {};
  const spawnResult = resolveSubagentSpawnResult(item);
  const { agentId } = spawnResult;
  const spawnText = expertDelegationText(args);
  const isDelegation = isExpertDelegationCall(item.name, args);
  const failedSpawn = spawnResult.failed;
  const canOpenTranscript = !!agentId && !failedSpawn;
  const sessionId = sessionIdProp || activeExpertSessionId();
  const sessionSnapshot = expertLedgerSnapshots.get(sessionId);

  // Hook 全部无条件运行（协调行分支在 Hook 之后），实例 Hook 数量恒定。
  const [live, setLive] = useState(null);
  const [ledger, setLedger] = useState(() => (
    sessionSnapshot ? sessionSnapshot.agents : []
  ));
  const [childrenExpanded, setChildrenExpanded] = useState(false);
  const [expandedChildIds, setExpandedChildIds] = useState(() => new Set());
  useEffect(() => {
    setChildrenExpanded(false);
    setExpandedChildIds(new Set());
  }, [agentId]);
  useEffect(() => {
    if (!isDelegation || failedSpawn || !agentId || typeof window === 'undefined') return undefined;
    const onUpdate = event => {
      const detail = event && event.detail;
      if (!detail) return;
      if (detail.sessionId && sessionId && detail.sessionId !== sessionId) return;
      // 已停表后若父模型 followup 唤醒旧代理，或运行中代理新派后代，任一
      // 非终态事件都要唤醒共享 ledger 轮询，重新取得完整父链和权威终态。
      if (!detail.done && detail.source !== 'ledger') {
        const key = expertWatchKey(sessionId, agentId);
        const watched = expertCardWatch.get(key);
        if (watched) expertCardWatch.set(key, { ...watched, done: false });
        kickExpertPoll(0);
      }
      if (detail.agentId !== agentId) return;
      setLive(prev => {
        // 终态 ratchet：落盘终态是权威，迟到的非终态实时事件不得翻回"工作中"。
        if (prev && prev.done && !detail.done) return prev;
        // 字段合并：实时事件不带 seq/roleCount/blocked，不能把轮询补的字段冲掉。
        return { ...(prev || {}), ...detail };
      });
    };
    const onLedgerUpdate = event => {
      const detail = event && event.detail;
      if (!detail || detail.sessionId !== sessionId || !Array.isArray(detail.agents)) return;
      setLedger(detail.agents);
    };
    setLedger(expertLedgerSnapshots.get(sessionId)?.agents || []);
    window.addEventListener('pinvou:subagent-update', onUpdate);
    window.addEventListener('pinvou:subagent-ledger-update', onLedgerUpdate);
    const unwatch = watchExpertCard(sessionId, agentId);
    return () => {
      window.removeEventListener('pinvou:subagent-update', onUpdate);
      window.removeEventListener('pinvou:subagent-ledger-update', onLedgerUpdate);
      unwatch();
    };
  }, [agentId, failedSpawn, isDelegation, sessionId]);

  const ledgerOrdinals = useMemo(() => subagentRoleOrdinals(ledger), [ledger]);
  const descendantRows = useMemo(
    () => visibleSubagentDescendantRows(ledger, agentId, expandedChildIds),
    [agentId, expandedChildIds, ledger],
  );
  const directChildCount = useMemo(
    () => descendantRows.filter(row => row.depth === 0).length,
    [descendantRows],
  );
  const toggleChildBranch = childAgentId => {
    setExpandedChildIds((current) => {
      const next = new Set(current);
      if (next.has(childAgentId)) next.delete(childAgentId);
      else next.add(childAgentId);
      return next;
    });
  };

  if (!isDelegation) {
    const action = isAgentWaitCall(item.name, args) ? 'wait' : String(args.action || 'start');
    return (
      <div
        data-testid="agent-coordination-row"
        className="my-1 flex items-center gap-2 px-1 text-[11.5px] text-[#8E8E93]"
      >
        <span className="inline-block h-1.5 w-1.5 rounded-full bg-current opacity-50" />
        <span className="truncate">{copy.coordinationRow(action)}{args.agent_id ? ` · ${args.agent_id}` : ''}</span>
      </div>
    );
  }

  // 承担者以正式 profile 为准；普通对话常只传 name/type，统一决策函数保证
  // 行内卡与右侧面板不会一个有名、一个又退回“通用执行者”。
  const personas = bridge.available && bridge.personas ? bridge.personas.getPersonas() : [];
  const parentOrdinal = ledgerOrdinals.get(agentId)
    || (live && live.roleCount ? { seq: live.seq, count: live.roleCount } : null);
  const presentation = resolveSubagentPresentation({
    role: args.profile || args.role,
    agentType: args.type || args.agent_type || args.agent_name,
    sessionName: args.name || args.session_name,
    objective: spawnText,
    personas,
    agentId,
    roleCards: copy.roleCards,
    ordinal: parentOrdinal,
  });
  const { identity, name, subtitle } = presentation;
  const task = presentation.task.split(/\r?\n/)[0];
  const status = expertStatusPresentation({
    summary: live,
    failedSpawn,
    itemState: item.state,
    copy,
  });

  return (
    <div data-testid="expert-agent-tree" className="my-1 w-full max-w-[520px]">
      <div
        data-testid="expert-agent-card"
        className={`flex w-full items-center rounded-[12px] border border-[#E5E5EA] bg-white px-2 py-1.5 transition-colors dark:border-[#38383A] dark:bg-[#1C1C1E] ${
          canOpenTranscript ? 'hover:bg-[#F2F2F7] dark:hover:bg-[#2C2C2E]' : ''
        }`}
      >
        <button
          type="button"
          disabled={!canOpenTranscript}
          onClick={canOpenTranscript ? () => openSubagentTranscript(agentId, sessionId) : undefined}
          className={`flex min-w-0 flex-1 items-center gap-2.5 px-1 py-0.5 text-left ${
            canOpenTranscript ? 'cursor-pointer' : 'cursor-default'
          }`}
        >
          <AppIcon
            card={{ id: identity.avatarKey, name: subtitle || name, dept: identity.personaDept }}
            cls="h-8 w-8 shrink-0 overflow-hidden rounded-[10px]"
            fb={14}
          />
          <span
            className="min-w-0 max-w-[148px] shrink-0"
            title={subtitle ? `${name} · ${subtitle}` : name}
          >
            <span className={`block truncate text-[12.5px] font-semibold leading-[15px] text-[#1C1C1E] dark:text-[#E5E5EA]`}>
              {name}
            </span>
            {subtitle && (
              <span className="block truncate text-[10px] leading-[13px] text-[#8E8E93]">{subtitle}</span>
            )}
          </span>
          <span className="min-w-0 flex-1 truncate text-[12px] text-[#8E8E93]">{task}</span>
          <span className="flex max-w-[112px] shrink-0 items-center gap-1.5 truncate text-[11px] text-[#8E8E93]">
            <span className="inline-block h-1.5 w-1.5 shrink-0 rounded-full" style={{ background: status.dotColor }} />
            <span className="truncate">{status.text}</span>
          </span>
        </button>
        {directChildCount > 0 && (
          <button
            type="button"
            data-testid="expert-agent-children-toggle"
            onClick={() => setChildrenExpanded(value => !value)}
            className="ml-1 flex h-7 shrink-0 items-center gap-1 rounded-lg px-1.5 text-[9.5px] text-[#8E8E93] hover:bg-black/[0.05] dark:hover:bg-white/[0.07]"
            aria-label={childrenExpanded ? copy.collapseChildren(name) : copy.expandChildren(name)}
            title={childrenExpanded ? copy.collapseChildren(name) : copy.expandChildren(name)}
            aria-expanded={childrenExpanded}
          >
            <span>{copy.childAgentCount(directChildCount)}</span>
            {childrenExpanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
          </button>
        )}
      </div>
      {childrenExpanded && directChildCount > 0 && (
        <div
          data-testid="expert-agent-children"
          className="ml-5 mt-1 space-y-1 border-l border-black/[0.08] pl-2 dark:border-white/[0.09]"
        >
          {descendantRows.map(({ entry, depth, childCount }) => {
            const childPresentation = resolveSubagentPresentation({
              role: entry.role,
              agentType: entry.agent_type,
              sessionName: entry.session_name,
              objective: entry.objective,
              personas,
              agentId: entry.agent_id,
              roleCards: copy.roleCards,
              ordinal: ledgerOrdinals.get(entry.agent_id),
            });
            const childStatus = expertStatusPresentation({ summary: entry, copy });
            const branchExpanded = expandedChildIds.has(entry.agent_id);
            return (
              <div
                key={entry.agent_id}
                className="flex min-w-0 items-center"
                style={{ marginLeft: `${Math.min(depth, 3) * 12}px` }}
              >
                {childCount > 0 ? (
                  <button
                    type="button"
                    onClick={() => toggleChildBranch(entry.agent_id)}
                    className="flex h-7 w-6 shrink-0 items-center justify-center rounded-md text-[#8E8E93] hover:bg-black/[0.05] dark:hover:bg-white/[0.07]"
                    aria-label={branchExpanded
                      ? copy.collapseChildren(childPresentation.name)
                      : copy.expandChildren(childPresentation.name)}
                    title={branchExpanded
                      ? copy.collapseChildren(childPresentation.name)
                      : copy.expandChildren(childPresentation.name)}
                    aria-expanded={branchExpanded}
                  >
                    {branchExpanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
                  </button>
                ) : (
                  <span className="w-6 shrink-0" />
                )}
                <button
                  type="button"
                  data-testid="expert-agent-child-card"
                  onClick={() => openSubagentTranscript(entry.agent_id, sessionId)}
                  className={`flex min-w-0 flex-1 items-center gap-2 rounded-[10px] px-2 py-1.5 text-left ${
                    'hover:bg-black/[0.035] dark:hover:bg-white/[0.06]'
                  }`}
                >
                  <AppIcon
                    card={{
                      id: childPresentation.identity.avatarKey,
                      name: childPresentation.subtitle || childPresentation.name,
                      dept: childPresentation.identity.personaDept,
                    }}
                    cls="h-7 w-7 shrink-0 overflow-hidden rounded-[9px]"
                    fb={13}
                  />
                  <span
                    className="min-w-0 max-w-[132px] shrink-0"
                    title={childPresentation.subtitle
                      ? `${childPresentation.name} · ${childPresentation.subtitle}`
                      : childPresentation.name}
                  >
                    <span className={`block truncate text-[11.5px] font-semibold leading-[14px] text-[#1C1C1E] dark:text-[#E5E5EA]`}>
                      {childPresentation.name}
                    </span>
                    {childPresentation.subtitle && (
                      <span className="block truncate text-[9.5px] leading-[12px] text-[#8E8E93]">
                        {childPresentation.subtitle}
                      </span>
                    )}
                  </span>
                  <span className="min-w-0 flex-1 truncate text-[11px] text-[#8E8E93]">
                    {childPresentation.task || entry.agent_id}
                  </span>
                  {childCount > 0 && (
                    <span className="shrink-0 rounded-full bg-black/[0.035] px-1.5 py-px text-[9px] text-[#8E8E93] dark:bg-white/[0.06]">
                      {copy.childAgentCount(childCount)}
                    </span>
                  )}
                  <span className="flex max-w-[96px] shrink-0 items-center gap-1.5 truncate text-[10px] text-[#8E8E93]">
                    <span className="inline-block h-1.5 w-1.5 shrink-0 rounded-full" style={{ background: childStatus.dotColor }} />
                    <span className="truncate">{childStatus.text}</span>
                  </span>
                </button>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
};

const ToolOutput = ({ item, t }) => {
      const out = item.output;
      if (item.success === false) return <OutputError text={out} />;
      if (isWeatherTool(item.name)) {
        let raw = out;
        const envelope = tryParseJson(out);
        if (envelope && Array.isArray(envelope.content)) {
          const txt = envelope.content.find(c => c.type === 'text');
          if (txt && txt.text) raw = txt.text;
        }
        const w = tryParseJson(raw);
        if (w && w.type === 'weather' && !w.error) return <WeatherCard data={w} t={t} />;
      }
      // 股票报价卡片：iwencai 返回表格数据 → 映射为卡片
      if (isStockQuoteTool(item.name)) {
        let raw = out;
        const envelope = tryParseJson(out);
        if (envelope && Array.isArray(envelope.content)) {
          const txt = envelope.content.find(c => c.type === 'text');
          if (txt && txt.text) raw = txt.text;
        }
        const w = tryParseJson(raw);
        if (w && Array.isArray(w.datas) && w.datas.length > 0) {
          const d = w.datas[0];
          const findVal = (obj, keyword) => {
            for (const k of Object.keys(obj)) {
              if (k.includes(keyword)) return parseFloat(obj[k]);
            }
            return undefined;
          };
          const mapped = {
            name: d['股票简称'] || '--',
            code: (d['股票代码'] || '').replace(/\.\w+$/, ''),
            price: parseFloat(d['最新价']),
            changePercent: findVal(d, '涨跌幅'),
            open: findVal(d, '开盘价'),
            high: findVal(d, '最高价'),
            low: findVal(d, '最低价'),
          };
          return <StockQuoteCard data={mapped} t={t} />;
        }
        if (w && w.type === 'stock_quote' && !w.error) return <StockQuoteCard data={w} t={t} />;
      }
      if (isReceipt(out)) return <ReceiptBlock text={out} t={t} />;
      if (item.name === 'list_dir' || (item.name === 'File' && item.args?.action === 'list')) { const v = tryParseJson(out); if (Array.isArray(v)) return <ListDirView items={v} t={t} />; }
      else if (item.name === 'grep_files' || (item.name === 'File' && item.args?.action === 'search_content')) { const v = tryParseJson(out); if (v && Array.isArray(v.matches)) return <GrepView data={v} t={t} />; }
      else if (isShellExecutionTool(item.name)) {
        const v = tryParseJson(out);
        if (v && (v.stdout != null || v.exit_code != null || v.status)) return <ShellView data={v} t={t} />;
        return <ShellTextView cmd={item.args && item.args.command} text={out} />;
      }
      // File.write / File.edit 走 unified diff；File.patch 返回结构化 PatchResult。
      else if (item.name === 'File' && item.args?.action === 'patch') {
        const result = tryParseJson(out);
        if (result && typeof result === 'object') {
          const files = Array.isArray(result.touched_files) ? result.touched_files : [];
          return <div className="space-y-1 text-xs">
            {result.message ? <div>{String(result.message)}</div> : null}
            {files.map(path => <div key={path} className="font-mono break-all">{path}</div>)}
            {(result.files_applied != null || result.hunks_applied != null) ? <div className="text-[#757575] dark:text-[#8E8E8E]">
              files {result.files_applied ?? files.length} · hunks {result.hunks_applied ?? 0}
            </div> : null}
          </div>;
        }
      }
      else if ((item.name === 'File' && ['write', 'edit'].includes(item.args?.action)) || item.name === 'edit_file' || item.name === 'write_file') { if (looksDiff(out)) return <DiffView text={out} t={t} />; }
      else if (TODO_TOOLS.indexOf(item.name) >= 0) { const v = tryTailJson(out); if (v && Array.isArray(v.items)) return <TodoView snap={v} t={t} />; }
      return <OutputPre text={out} />;
    };

    const ToolCard = ({ item, t, variant = 'legacy', sessionId }) => {
      // 委派实例不走通用工具卡：专家卡是多智能体的第一公民展示（ADR-0006）。
      // 提前返回发生在本组件任何 Hook 之前，且 item.name 对一个实例终生不变，
      // 因此每个实例的 Hook 数量恒定，不触犯 Hook 规则。
      if (EXPERT_CARD_ENABLED && (item.name === 'agent' || isAgentWaitCall(item.name, item.args))) {
        return <ExpertAgentCard item={item} t={t} sessionId={sessionId} />;
      }
      const isTimeline = variant === 'timeline';
      const isRunning = item.state === 'running';
      const [cancelling, setCancelling] = useState(false);
      const [shellCancelError, setShellCancelError] = useState('');
      // 有可视化卡片的工具(天气/股票)完成后直接展开,不折叠
      const hasCard = (isWeatherTool(item.name) || isStockQuoteTool(item.name)) && item.state === 'done';
      const hasLiveShellOutput = isShellExecutionTool(item.name)
        && isRunning
        && (item.liveOutput || item.output != null);
      const [expanded, setExpanded] = useState(!isTimeline && hasCard);
      useEffect(() => {
        if (!isTimeline && hasCard) {
          setExpanded(true);
        }
      }, [hasCard, isTimeline]);
      const displayExpanded = hasLiveShellOutput || expanded;
      const isDone = item.state === 'done';
      const isFailed = item.state === 'failed';
      const quiet = isQuietTool(item);
      const summary = toolSummary(item.name, item.args, t);

      // 状态色:按 isRunning/isDone/isFailed 三态,各自给出 light base + dark: token。
      const statusColor = isRunning
        ? 'text-[#0B57D0] dark:text-[#A8C7FA]'
        : isDone
          ? 'text-[#137333] dark:text-[#93D5A6]'
          : 'text-[#C5221F] dark:text-[#F28B82]';

      const statusText = isRunning ? t.toolRunning
        : (item.exitCode != null ? `${isDone ? t.toolDone : t.toolFailed} · exit ${item.exitCode}` : (isDone ? t.toolDone : t.toolFailed));
      const timelineStatusText = isRunning
        ? t.uiToolRender.running
        : item.exitCode != null
          ? `${isDone ? t.uiToolRender.done : t.uiToolRender.failed} · exit ${item.exitCode}`
          : isDone
            ? t.uiToolRender.done
            : t.uiToolRender.failed;
      const mutedColor = 'text-[#757575] dark:text-[#8E8E8E]';
      const cancelBackground = async (event) => {
        event.stopPropagation();
        if (!item.taskId || cancelling) return;
        setCancelling(true);
        setShellCancelError('');
        try {
          await bridge.chat.cancelShellTask(item.sessionId, item.taskId);
        } catch (error) {
          console.warn('cancel shell task failed', error);
          setShellCancelError(`${t.shellCancelFailed || t.toolFailed}: ${String(error)}`);
        } finally {
          setCancelling(false);
        }
      };
      const cancelButton = item.taskId && isRunning ? (
        <button
          type="button"
          data-testid="cancel-shell-task"
          data-shell-task-id={item.taskId}
          disabled={cancelling}
          onClick={cancelBackground}
          className={`text-[11px] px-2 py-1 rounded-full disabled:opacity-50 bg-black/5 text-[#C5221F] hover:bg-black/10 dark:bg-white/10 dark:text-[#F28B82] dark:hover:bg-white/15`}
        >
          {cancelling ? t.cancelling : t.cancel}
        </button>
      ) : null;

      const detail = displayExpanded ? (
        <div className={`${isTimeline ? 'px-3 pb-3' : 'px-4 pb-3'} border-t border-black/5 dark:border-white/5`}>
          {item.output != null
            ? <div className="mt-2"><ToolOutput item={item} t={t} /></div>
            : null}
        </div>
      ) : null;

      if (isTimeline) {
        const tone = isFailed
          ? 'text-red-500 bg-red-500/10'
          : isRunning
            ? 'text-blue-500 bg-blue-500/10'
            : 'text-gray-500 bg-black/[0.04] dark:bg-white/[0.06]';
        const meta = `${summary ? `${summary} · ` : ''}${timelineStatusText}`;
        const toggleExpanded = () => setExpanded(value => !value);
        return (
          <div
            data-tool-card-variant="timeline"
            data-tool-name={item.name}
            className={`rounded-xl border ${
              isFailed ? 'border-red-500/20' : 'border-black/[0.05] dark:border-white/[0.07]'
            } bg-white/45 dark:bg-white/[0.015]`}
          >
            <div
              role="button"
              tabIndex={0}
              onClick={toggleExpanded}
              onKeyDown={(event) => {
                if (event.key === 'Enter' || event.key === ' ') {
                  event.preventDefault();
                  toggleExpanded();
                }
              }}
              className="w-full min-h-10 px-2.5 py-2 flex items-center gap-2.5 text-left rounded-xl cursor-pointer hover:bg-black/[0.025] dark:hover:bg-white/[0.035]"
            >
              <span className={`w-6 h-6 shrink-0 rounded-lg flex items-center justify-center ${tone}`}>
                <Wrench size={13} />
              </span>
              <span className="min-w-0 flex-1">
                <span className="block truncate text-[12px] font-medium">{item.name}</span>
                <span className="block mt-0.5 truncate text-[10px] text-gray-400">{meta}</span>
              </span>
              {isRunning && <span className="w-1.5 h-1.5 rounded-full bg-blue-500 animate-pulse" />}
              {cancelButton}
              <ChevronDown size={13} className={`shrink-0 text-gray-400 transition-transform ${displayExpanded ? 'rotate-180' : ''}`} />
            </div>
            {shellCancelError && (
              <div className="px-3 pb-2 text-[11px] text-red-500">{shellCancelError}</div>
            )}
            {detail}
          </div>
        );
      }

      // 弱化类：单行灰条。完成态低调（图标灰），运行/失败态保留状态色以便察觉。
      if (quiet) {
        const iconColor = isDone ? mutedColor : statusColor;
        return (
          <div className={expanded ? `rounded-[12px] overflow-hidden border border-black/5 dark:border-white/5` : ''}>
            <div
              className={`flex items-center gap-2 px-2 py-1 rounded-[8px] cursor-pointer hover:bg-[#E8EDF2] dark:hover:bg-[#282A2C]`}
              onClick={() => setExpanded(!expanded)}
            >
              <Wrench size={12} className={iconColor} />
              <span className={`text-[12px] ${mutedColor}`}>{item.name}</span>
              {summary
                ? <span className={`text-[12px] flex-1 truncate ${mutedColor}`}>{summary}</span>
                : <span className="flex-1" />}
              {isRunning && <span className={`text-[11px] ${statusColor}`}>{t.toolRunning}</span>}
              {isFailed && <span className={`text-[11px] ${statusColor}`}>{t.toolFailed}</span>}
              {cancelButton}
              <ChevronDown size={12} className={`transition-transform ${expanded ? 'rotate-180' : ''} ${mutedColor}`} />
            </div>
            {detail}
          </div>
        );
      }

      // 有产出类：保留醒目卡片，标题行带摘要。
      return (
        <div className={`rounded-[16px] overflow-hidden border bg-[#F0F4F9] border-black/5 dark:bg-[#1E1F20] dark:border-white/5`}>
          <div
            className={`flex items-center gap-3 px-4 py-3 cursor-pointer hover:bg-[#E8EDF2] dark:hover:bg-[#282A2C]`}
            onClick={() => setExpanded(!expanded)}
          >
            <Wrench size={14} className={statusColor} />
            <span className={`text-[13px] font-medium text-[#1F1F1F] dark:text-[#E3E3E3]`}>
              {item.name}
            </span>
            {summary
              ? <span className={`text-[12px] flex-1 truncate ${mutedColor}`}>{summary}</span>
              : <span className="flex-1" />}
            <span className={`text-[12px] ${statusColor}`}>{statusText}</span>
            {cancelButton}
            <ChevronDown size={14} className={`transition-transform ${expanded ? 'rotate-180' : ''} text-[#444746] dark:text-[#C4C7C5]`} />
          </div>
          {shellCancelError && (
            <div className={`px-4 pb-2 text-[11px] text-[#C5221F] dark:text-[#F28B82]`}>
              {shellCancelError}
            </div>
          )}
          {detail}
        </div>
      );
    };

    // ==========================================
    // Plan / 待办 步骤渲染
    // ==========================================
    const STEP_SYM = { completed: '●', in_progress: '◎', pending: '○' };
    const PlanLayer = ({ label, explanation, items, field }) => {
      if (!items || items.length === 0) return null;
      return (
        <section className="mb-2">
          <div className={`text-[12px] font-semibold mb-1 text-[#0B57D0] dark:text-[#A8C7FA]`}>{label}</div>
          {explanation && <p className={`text-[13px] mb-1.5 leading-relaxed text-[#444746] dark:text-[#C4C7C5]`}>{explanation}</p>}
          <ol className="space-y-1">
            {items.map((it, i) => (
              <li key={i} className={`text-[13px] flex gap-2 leading-relaxed ${it.status === 'completed' ? 'opacity-60' : ''} text-[#1F1F1F] dark:text-[#E3E3E3]`}>
                <span className={it.status === 'in_progress' ? 'text-[#E37400] dark:text-[#FDD663]' : ''}>{STEP_SYM[it.status] || '○'}</span>
                <span>{it[field] || ''}</span>
              </li>
            ))}
          </ol>
        </section>
      );
    };

    const cardBoxCls = (accent) =>
      `rounded-[16px] border p-4 my-1 bg-[#F0F4F9] border-black/5 dark:bg-[#1E1F20] dark:border-white/10 ${accent || ''}`;
    const cardBtnCls = (variant) => {
      const base = 'px-3 py-1.5 rounded-full text-[13px] font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed';
      if (variant === 'primary') return `${base} bg-[#0B57D0] text-white hover:bg-[#0A4BB8] dark:bg-[#A8C7FA] dark:text-[#062E6F] dark:hover:bg-[#C2DBFF]`;
      return `${base} bg-white text-[#1F1F1F] hover:bg-[#E1E5EA] border border-black/10 dark:border-transparent dark:bg-[#333537] dark:text-[#E3E3E3] dark:hover:bg-[#444746]`;
    };

    // 品悟角色配色（与产物卡一致）：品=盾·橙 #FF9500/#FF9F0A，悟=闪光·紫 #5E5CE6。
    // 返回 { name, accentHex(inline-style 原色,品需 isDark), text(类), softBg(类), Icon }。
    const pvRole = (isWu, isDark) => isWu
      ? { name: '悟', accentHex: '#5E5CE6', text: 'text-[#5E5CE6]',
          softBg: 'bg-[#5E5CE6]/[0.10] dark:bg-[#5E5CE6]/15', Icon: AcSparkles }
      : { name: '品', accentHex: isDark ? '#FF9F0A' : '#FF9500', text: 'text-[#FF9500] dark:text-[#FF9F0A]',
          softBg: 'bg-[#FF9500]/[0.10] dark:bg-[#FF9F0A]/15', Icon: AcShieldCheck };

    // ==========================================
    // PinvouSummonCard — 🧭 召唤式检阅（Boss 主动呼叫 Pinvou）
    // 自报家门人格(单主+alternates,§3.3) + trace + issues(severity 分色)。
    // ==========================================
    // 逐条裁决行(§2 + kind 分流):每条按本质给对应动作,不再一刀切判断题——
    //   recommendation(决策点/缺信息,Boss 才能定)→ 采纳建议 / 让 AI 问我;
    //   issue.needs_verify(外部事实,AI 无知识)→ 让 AI 核实 / 我确认没问题;
    //   issue 其他(产物缺陷,AI 改得动)→ 让 AI 改 / 接受现状(high 默认勾)。
    // 「交给 AI 处理」按各条动作组装定向指令走 B1。单独子组件:useState 放这避 hooks 错位。
    const PinvouRows = ({ review, t, role }) => {
      role = role || pvRole(false, false);
      const body = 'text-[#000] dark:text-[#fff]';
      const muted = 'text-[#3C3C43]/60 dark:text-[#EBEBF5]/60';
      // iOS 语义色：high 红 / medium 橙 / low 灰
      const sevDot = (s) => s === 'high' ? '#FF3B30' : s === 'medium' ? '#FF9500' : '#C7C7CC';
      const rows = [
        ...(review.recommendations || []).map((x, i) => ({
          k: 'r' + i, raw: x, kind: 'rec', dot: '#FF9500',
          head: (x.topic ? x.topic + '：' : '') + t.pvSuggest + x.pick, sub: x.why,
        })),
        ...(review.issues || []).map((x, i) => ({
          k: 'i' + i, raw: x, kind: x.kind === 'needs_verify' ? 'verify' : 'fix',
          dot: sevDot(x.severity), sev: x.severity, nv: x.kind === 'needs_verify',
          head: x.text, sub: x.suggestion,
        })),
        ...(review.coverage || []).map((x, i) => ({
          k: 'c' + i, raw: x, kind: 'gap', dot: '#5E5CE6',
          sev: x.severity, head: x.dimension + (x.text ? '：' + x.text : ''), sub: x.suggestion,
        })),
      ];
      // 每类二选一 [值,文案]:第一个=「要 AI 做」(高亮),第二个=Boss 自己消化(灰)。
      const ACT = {
        rec: [['adopt', t.pvActAdopt], ['ask', t.pvActAsk]],
        verify: [['verify', t.pvActVerify], ['confirmed', t.pvActConfirmed]],
        fix: [['modify', t.pvActModify], ['accept', t.pvActAccept]],
        gap: [['fill', t.pvActFill], ['skip', t.pvActSkip]],
      };
      const ACTIVE = { adopt: 1, ask: 1, verify: 1, modify: 1, fill: 1 }; // 需转交给 AI 的动作
      const [res, setRes] = useState(() => {
        const m = {};
        rows.forEach(it => {
          let def = null;
          if (it.sev === 'high') def = it.kind === 'fix' ? 'modify' : it.kind === 'gap' ? 'fill' : null;
          m[it.k] = it.raw.resolution || def;
        });
        return m;
      });
      const setOne = (k, v) => setRes(p => ({ ...p, [k]: p[k] === v ? null : v }));
      const activeCount = rows.filter(it => ACTIVE[res[it.k]]).length;
      // iOS 分段按钮风：选中且需转交 AI=填充角色色(背景走 style)；选中但自行消化=灰填充；未选=描边。
      const chip = (on, active) => `text-[12px] px-2.5 py-1 rounded-full font-medium transition-all active:scale-[0.96] ${on
        ? (active ? 'text-white border border-transparent'
                  : 'bg-black/[0.08] text-[#000] border border-transparent dark:bg-white/15 dark:text-[#fff]')
        : 'border border-black/[0.12] text-[#3C3C43]/80 hover:bg-black/5 dark:border-white/15 dark:text-[#EBEBF5]/70 dark:hover:bg-white/5'}`;
      const onResolve = () => {
        if (!bridge.available) return;
        // 弹窗里 review 是 notify 深拷贝,写它的 resolution 落不到原 state;把裁决按下标传给 bridge,
        // 由 bridge 在 state.pinvouModal.review(原 state)上写、再落盘(根治 resolution 不持久化)。
        const resolutions = {
          recs: (review.recommendations || []).map((_, i) => res['r' + i] || 'pending'),
          issues: (review.issues || []).map((_, i) => res['i' + i] || 'pending'),
          coverage: (review.coverage || []).map((_, i) => res['c' + i] || 'pending'),
        };
        const actions = [];
        rows.forEach(it => {
          const a = res[it.k];
          if (a === 'modify') actions.push({ t: 'fix', text: it.head + (it.sub ? '（' + it.sub + '）' : '') });
          else if (a === 'verify') actions.push({ t: 'verify', text: it.head + (it.sub ? '（' + it.sub + '）' : '') });
          else if (a === 'adopt') actions.push({ t: 'adopt', topic: it.raw.topic || '', pick: it.raw.pick || '' });
          else if (a === 'ask') actions.push({ t: 'ask', topic: it.raw.topic || it.head });
          else if (a === 'fill') actions.push({ t: 'fill', dimension: it.raw.dimension || '', suggestion: it.raw.suggestion || '' });
        });
        bridge.interaction.resolvePinvouReview(resolutions, actions);
      };
      return (
        <div>
          <div className="space-y-2">
            {rows.map(it => {
              const decided = res[it.k];
              const passive = decided === 'accept' || decided === 'confirmed' || decided === 'skip';
              return (
                <div key={it.k} className={`rounded-[12px] px-3 py-2.5 transition-opacity ${passive ? 'opacity-40' : ''} bg-[#F2F2F7] dark:bg-white/[0.06]`}>
                  <div className="flex gap-2.5">
                    <span className="mt-[7px] w-[7px] h-[7px] rounded-full shrink-0" style={{ background: it.dot }} />
                    <div className="flex-1 min-w-0">
                      <div className={`text-[14px] leading-relaxed ${body}`}>
                        {it.nv && <span className={`text-[10.5px] font-medium mr-1.5 px-1.5 py-px rounded-full align-[1px] bg-[#FFF8E1] text-[#B25000] dark:bg-[#FFD60A]/20 dark:text-[#FFD60A]`}>{t.pvNeedsVerify}</span>}
                        {it.head}
                      </div>
                      {it.sub && <div className={`text-[13px] mt-0.5 ${muted}`}>{it.sub}</div>}
                      <div className="flex gap-2 mt-2">
                        {ACT[it.kind].map(([v, label]) => (
                          <button key={v} onClick={() => setOne(it.k, v)} className={chip(decided === v, !!ACTIVE[v])}
                            style={decided === v && ACTIVE[v] ? { background: role.accentHex } : undefined}>{label}</button>
                        ))}
                      </div>
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
          <div className="flex items-center gap-2 mt-4 pt-1">
            {activeCount > 0 && (
              <button onClick={onResolve}
                className="px-4 py-2 rounded-full text-[14px] font-semibold text-white active:scale-[0.97] transition-transform"
                style={{ background: role.accentHex }}>
                {t.pvHandToAi(activeCount)}
              </button>
            )}
            <button onClick={() => bridge.available && bridge.interaction.dismissPinvouReview()} title={t.pvSkipTitle}
              className={`px-4 py-2 rounded-full text-[14px] font-medium transition-colors text-[#3C3C43]/70 hover:bg-black/5 dark:text-[#EBEBF5]/70 dark:hover:bg-white/5`}>
              {t.pvSkip}
            </button>
          </div>
        </div>
      );
    };

    // 检阅 loading:本地模型 5-30s / 在线模型通常更快,iOS 旋转菊花 spinner + 计时 + 安抚文字,别让 Boss 干等焦虑。
    const PinvouLoading = ({ isWu, isDark, t, isLocal }) => {
      const [secs, setSecs] = useState(0);
      useEffect(() => {
        const b = setInterval(() => setSecs(s => s + 1), 1000);
        return () => clearInterval(b);
      }, []);
      const role = pvRole(isWu, isDark);
      // isDark 保留——SVG circle stroke 与 pvRole 品·accentHex(inline-style 原色)仍需它。
      const muted = 'text-[#3C3C43]/60 dark:text-[#EBEBF5]/60';
      return (
        <div className="py-8 flex flex-col items-center text-center">
          {/* iOS activity spinner：底环 + 角色色弧，匀速旋转 */}
          <svg className="w-9 h-9" viewBox="0 0 24 24" fill="none" style={{ animation: 'tsSpinner 0.8s linear infinite' }}>
            <circle cx="12" cy="12" r="9" stroke={isDark ? 'rgba(255,255,255,.12)' : 'rgba(0,0,0,.08)'} strokeWidth="3" />
            <path d="M12 3a9 9 0 0 1 9 9" stroke={role.accentHex} strokeWidth="3" strokeLinecap="round" />
          </svg>
          <div className={`flex items-center gap-1.5 mt-4 text-[15px] font-semibold ${role.text}`}>
            <role.Icon className="w-[18px] h-[18px]" />
            <span>{isWu ? t.pvLoadingWu : t.pvLoadingPin}</span>
          </div>
          <div className={`text-[13px] mt-1.5 ${muted}`}>
            {isWu ? t.pvLoadingWuSub : t.pvLoadingPinSub}
            {secs > 0 && <span className="ml-1.5 tabular-nums opacity-70">{secs}s</span>}
          </div>
          <div className={`text-[12px] mt-1 ${muted}`} style={{ opacity: 0.6 }}>{t.pvLoadingHint(isLocal)}</div>
        </div>
      );
    };

    // 检阅结果卡（在底部 sheet 内渲染，无外层卡框；品=橙 / 悟=紫，与产物卡一致）。
    const PinvouSummonCard = ({ item, theme, t, isLocal }) => {
      const isDark = theme === 'dark';
      const isWu = !!item.coverage; // 悟=发散(coverage)；品=查错
      const role = pvRole(isWu, isDark);
      const muted = 'text-[#3C3C43]/60 dark:text-[#EBEBF5]/60';
      const body = 'text-[#000] dark:text-[#fff]';
      // isDark 保留——pvRole 品·accentHex 与 PinvouLoading SVG stroke 仍需它。
      if (item.loading) return <PinvouLoading isWu={isWu} isDark={isDark} t={t} isLocal={isLocal} />;
      if (item.error) return (
        <div className="py-2">
          <div className={`flex items-center gap-1.5 text-[15px] font-semibold ${role.text}`}><role.Icon className="w-[18px] h-[18px]" /><span>Pinvou {role.name}</span></div>
          <div className={`text-[14px] mt-2 text-[#FF3B30] dark:text-[#FF453A]`}>{t.pvFail}{item.error}</div>
        </div>
      );
      const r = item.review || {};
      if (r.dismissed) return (
        <div className={`py-2 flex items-center gap-1.5 text-[14px] ${muted}`}><role.Icon className="w-4 h-4" /><span>{'Pinvou · ' + role.name + ' · ' + t.pvSkipped}</span></div>
      );
      const personas = r.personas || [];
      const primary = personas.find(p => p && p.primary) || personas[0] || {};
      const alts = r.alternates || [];
      const hasRows = (r.recommendations || []).length > 0 || (r.issues || []).length > 0 || (r.coverage || []).length > 0;
      return (
        <div>
          <div className="flex items-center flex-wrap gap-x-2 gap-y-1 mb-2.5">
            <span className={`inline-flex items-center justify-center w-7 h-7 rounded-full ${role.softBg}`}>
              <role.Icon className={`w-[17px] h-[17px] ${role.text}`} />
            </span>
            <span className={`text-[16px] font-semibold ${body}`}>
              {'Pinvou · ' + role.name}
              {primary.label && <span className={`text-[14px] font-normal ${muted}`}> · {primary.label + t.pvPerspective}</span>}
            </span>
            {r.verdict === 'pass' && <span className={`text-[11px] font-semibold px-2 py-0.5 rounded-full bg-[#34C759]/15 text-[#248A3D] dark:bg-[#30D158]/20 dark:text-[#30D158]`}>{t.pvVerdictPass}</span>}
          </div>
          {alts.length > 0 && <div className={`text-[12px] -mt-1 mb-2 ${muted}`}>{t.pvAlsoInvolves} {alts.join(' / ')}</div>}
          {r.trace && <div className={`text-[14px] leading-relaxed mb-3 ${body}`}>{r.trace}</div>}
          {(r.framework || []).length > 0 && (
            <div className={`text-[12px] mb-3 px-3 py-2 rounded-[12px] leading-relaxed ${role.softBg} ${role.text}`}>
              <span className="opacity-70">{t.pvFramework} · {(r.framework || []).length}{t.pvDims}: </span>{(r.framework || []).join(' · ')}
            </div>
          )}
          {hasRows && <PinvouRows review={r} t={t} role={role} />}
        </div>
      );
    };

    // ==========================================
    // PlanCard — ✨ 方案准备好
    // ==========================================
    const PlanCard = ({ item, t, onPrefill }) => {
      const webReadOnly = multiAgentWebReadOnly();
      const active = item.cardState === 'active' && !item.resolved && !!item.planId;
      return (
        <div className={cardBoxCls('border-[#0B57D0]/20 dark:border-[#A8C7FA]/30')}>
          <div className={`text-[14px] font-semibold mb-3 text-[#1F1F1F] dark:text-[#E3E3E3]`}>{t.planReady}</div>
          {(!item.plan && !item.todos)
            ? <div className={`text-[13px] text-[#444746] dark:text-[#C4C7C5]`}>{t.planEmpty}</div>
            : <>
                <PlanLayer label={t.planLabel} explanation={item.plan && item.plan.explanation} items={item.plan && item.plan.items} field="step" />
                <PlanLayer label={t.planTodos} items={item.todos && item.todos.items} field="content" />
              </>}
          <div className={`h-px my-3 bg-black/10 dark:bg-white/10`}></div>
          {active ? (
            <div className="flex items-center gap-2 flex-wrap">
              <span className={`text-[13px] mr-1 text-[#444746] dark:text-[#C4C7C5]`}>{t.planNext}</span>
              <button className={cardBtnCls('primary') + ' disabled:opacity-40 disabled:cursor-not-allowed'} disabled={webReadOnly} onClick={() => bridge.interaction.acceptPlan(item.id, item.planMarkdown, undefined, item.planId)}>{t.planGo}</button>
              <button className={cardBtnCls() + ' disabled:opacity-40 disabled:cursor-not-allowed'} disabled={webReadOnly} onClick={() => onPrefill && onPrefill(t.planRevisePrefill)}>{t.planEdit}</button>
              <button className={cardBtnCls() + ' disabled:opacity-40 disabled:cursor-not-allowed'} disabled={webReadOnly} onClick={() => bridge.interaction.discardPlan(item.id, item.planId)}>{t.planDrop}</button>
            </div>
          ) : (
            <div className={`text-[13px] font-medium text-[#137333] dark:text-[#93D5A6]`}>{item.statusLabel}</div>
          )}
        </div>
      );
    };

    // ==========================================
    // PlanStuckCard — Plan 模式 AI 撞只读保护(白名单/sandbox)的兜底卡
    // ==========================================
    const PlanStuckCard = ({ item, t }) => {
      const webReadOnly = multiAgentWebReadOnly();
      const done = item.resolved;
      return (
        <div className={cardBoxCls('border-[#E37400]/20 dark:border-[#FDD663]/30')}>
          <div className={`text-[13px] leading-relaxed mb-3 text-[#1F1F1F] dark:text-[#E3E3E3]`}>
            {t.stuckPlanPre} <code className="px-1 rounded bg-black/20">{item.toolName || t.uiToolRender.toolUnknown}</code> {t.stuckPlanPost}
          </div>
          {done ? (
            <div className={`text-[13px] text-[#444746] dark:text-[#C4C7C5]`}>{item.statusLabel || t.handled}</div>
          ) : (
            <div className="flex items-center gap-2 flex-wrap">
              <button className={cardBtnCls() + ' disabled:opacity-40 disabled:cursor-not-allowed'} disabled={webReadOnly} onClick={() => bridge.interaction.planStuckReplan(item.id)}>{t.stuckReplan}</button>
              <button className={cardBtnCls('primary') + ' disabled:opacity-40 disabled:cursor-not-allowed'} disabled={webReadOnly} onClick={() => bridge.interaction.planStuckGo(item.id)}>⚡ {t.stuckGo}</button>
            </div>
          )}
        </div>
      );
    };

    // ==========================================
    // CarefulBlockedCard — 🛑 危险操作被拦（人话化：底座英文技术原因→中文人话，技术详情折叠）
    // ==========================================
    const REASON_MAP = [
      [/root filesystem|delete all root|delete root/i, 'rsRoot'],
      [/home director/i, 'rsHome'],
      [/recursiv|rm\s+-rf/i, 'rsRecursive'],
      [/forced? deletion|\bforce\b/i, 'rsForce'],
      [/fork bomb/i, 'rsForkbomb'],
      [/overwrite|\bof=|\bdd\b/i, 'rsOverwrite'],
      [/format|mkfs/i, 'rsFormat'],
    ];
    const humanizeReason = (en, t) => {
      const s = String(en);
      for (let i = 0; i < REASON_MAP.length; i++) if (REASON_MAP[i][0].test(s)) return t[REASON_MAP[i][1]];
      return t.rsDefault;
    };
    const CarefulBlockedCard = ({ item, t }) => {
      const [showTech, setShowTech] = useState(false);
      const md = item.metadata || {};
      const cmd = (item.args && (item.args.command || item.args.cmd)) || t.cbCmdUnknown;
      const rawReasons = (md.reasons && md.reasons.length) ? md.reasons : [];
      const rawSuggestions = md.suggestions || [];
      const humanReasons = [...new Set(rawReasons.map(r => humanizeReason(r, t)))];
      if (humanReasons.length === 0) humanReasons.push(t.rsDefault);
      const hasTech = rawReasons.length > 0 || rawSuggestions.length > 0;
      return (
        <div className={cardBoxCls('border-[#C5221F]/30 dark:border-[#F28B82]/40')}>
          <div className={`text-[14px] font-semibold mb-2 text-[#C5221F] dark:text-[#F28B82]`}>{t.cbTitle}</div>
          <div className={`text-[12px] mb-1 text-[#757575] dark:text-[#8E8E8E]`}>{t.cbWant}</div>
          <pre className={`text-[12px] font-mono rounded-lg p-2 mb-2 overflow-x-auto bg-white text-[#C5221F] dark:bg-[#131314] dark:text-[#F28B82]`}>{cmd}</pre>
          <div className="mb-2">
            <div className={`text-[12px] font-medium mb-1 text-[#444746] dark:text-[#C4C7C5]`}>{t.cbWhy}</div>
            <ul className={`list-disc pl-5 text-[13px] space-y-0.5 text-[#1F1F1F] dark:text-[#E3E3E3]`}>{humanReasons.map((r, i) => <li key={i}>{r}</li>)}</ul>
          </div>
          <div className={`text-[12px] leading-relaxed mb-1.5 text-[#757575] dark:text-[#8E8E8E]`}>{t.cbNote}</div>
          {hasTech && (
            <div>
              <button onClick={() => setShowTech(!showTech)} className={`text-[11px] text-[#0B57D0] dark:text-[#8AB4F8]`}>{showTech ? t.cbTechHide : t.cbTechShow}</button>
              {showTech && (
                <div className={`mt-1 text-[11px] font-mono space-y-0.5 text-[#757575] dark:text-[#8E8E8E]`}>
                  {rawReasons.map((r, i) => <div key={'r' + i}>· {r}</div>)}
                  {rawSuggestions.map((s, i) => <div key={'s' + i}>→ {s}</div>)}
                </div>
              )}
            </div>
          )}
        </div>
      );
    };

    // ==========================================
    // UserInputCard — 🤔 AI 想问你几个问题
    // ==========================================
    const isFreeTextPlaceholderOption = (option) => {
      const label = String(option?.label || '').trim();
      return /^(?:其他|其它|other)(?:\s*[\(（][^()（）]*[\)）])?$/i.test(label);
    };

    const UserInputCard = ({ item, t }) => {
      // Web 只读会话：呈现为锁定卡并说明去桌面端操作（后端漏斗是权威拦截，
      // 这里避免"能点但必败"的按钮，复核 P2）。
      const webReadOnly = multiAgentWebReadOnly();
      const questions = item.questions || [];
      const normalizedQuestions = questions.map((question, index) => {
        const allowOther = question.allow_free_text !== false;
        return {
          id: question.id || `question-${index + 1}`,
          header: question.header || `Q${index + 1}`,
          question: question.question || '',
          options: (question.options || [])
            .filter(option => !allowOther || !isFreeTextPlaceholderOption(option))
            .map(option => ({
              value: option.label,
              label: option.label,
              description: option.description || '',
            })),
          allowOther,
          multiSelect: Boolean(question.multi_select),
          required: !question.multi_select,
        };
      });

      function submit(groups) {
        if (webReadOnly) return;
        const answers = groups.flatMap(group => group.answers.map(answer => ({
          id: group.questionId,
          label: answer.other ? t.uiToolRender.other : answer.label,
          value: String(answer.value),
          // 保留 other 标记：QuestionChoiceCard 还原历史答案时据此把“其他”与预设选项区分开，
          // 避免“其他值 == 预设 value”被误判为预设（评审 P2）。
          other: answer.other,
        })));
        bridge.interaction.submitUserInput(item.id, item.toolCallId, answers, questions);
      }

      const statusText = webReadOnly && !item.resolved
        ? t.uiMultiAgent.webActionHint
        : item.cardState === 'submitted' ? t.uiSubmitted
        : item.cardState === 'cancelled' ? t.uiCancelled
        : item.submitting ? t.uiSubmitting : item.error ? t.uiSubmitFailed(item.error) : '';

      return (
        <QuestionChoiceCard
          title={t.uiqTitle}
          questions={normalizedQuestions}
          initialAnswers={item.restoredAnswers || []}
          resolved={Boolean(item.resolved) || webReadOnly}
          submitting={Boolean(item.submitting)}
          statusText={statusText}
          error={Boolean(item.error)}
          submitLabel={t.uiSubmit}
          cancelLabel={t.cpCancel}
          otherPlaceholder={t.uiToolRender.other}
          otherAnswerLabel={t.uiToolRender.other}
          inputPlaceholder={t.uiConversation.inputPlaceholder}
          onSubmit={submit}
          onCancel={!item.resolved && !webReadOnly
            ? () => bridge.interaction.cancelUserInput(item.id, item.toolCallId)
            : undefined}
        />
      );
    };

    // ==========================================
    // ArtifactsPanel — 产物面板（右侧抽屉 + 预览）
    // ==========================================
    // 产物列表/预览的 iOS 风类型图标:配色圆角 tile + 白色字形。
    // 复用成品卡那套 _ARTIFACT_FMT / _artifactKind / AcFmtIcon(line 3048+),列表与卡片视觉统一。

export { ToolOutput, ToolCard, STEP_SYM, PlanLayer, cardBoxCls, cardBtnCls, pvRole, PinvouRows, PinvouLoading, PinvouSummonCard, PlanCard, PlanStuckCard, REASON_MAP, humanizeReason, CarefulBlockedCard, UserInputCard };
