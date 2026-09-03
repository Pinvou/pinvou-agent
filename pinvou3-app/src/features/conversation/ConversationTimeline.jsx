import React, { useEffect, useId, useMemo, useState, useSyncExternalStore } from 'react';
import { FileTypeIcon } from '../../components/files/FileTypeIcon.jsx';
import { renderMarkdown } from '../../shared/markdown-renderer.js';
import { getSyntaxHighlightVersion, subscribeSyntaxHighlight } from '../../shared/syntax-highlighter.js';
import { useThrottledValue } from './useThrottledValue.js';
import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  Sparkles,
  Terminal,
  Wrench,
} from '../../components/icons.jsx';
import {
  commandExecutionDetails,
  collectToolWorkspaceResources,
  countsAsFailedOperation,
  elapsedMs,
  externalMarkdownUrl,
  fetchToolDetails,
  isFetchTool,
  isSearchTool,
  searchToolDetails,
  terminalStatus,
  toolWorkspaceResources,
  workspaceMarkdownResource,
} from './conversation-model.js';
import { AssistantMessageActions, AssistantMessageFooter } from './AssistantMessageActions.jsx';
import { assistantResponseAvailable, assistantResponseText } from './message-clipboard.js';

const DEFAULT_COPY = {
  completed: '已完成',
  failed: '失败',
  interrupted: '已中断',
  limitReached: '达到限制',
  processing: '处理中',
  processingActive: '正在处理',
  callingTool: name => `正在调用 ${name}`,
  waitingPermission: '等待授权',
  waitingInput: '等待你的输入',
  waitingInputShort: '等待输入',
  goLatest: label => `${label}，前往最新消息`,
  elapsed: milliseconds => {
    const seconds = Math.max(0, Math.floor(milliseconds / 1000));
    if (seconds < 60) return `${seconds}秒`;
    const minutes = Math.floor(seconds / 60);
    const remaining = seconds % 60;
    return remaining ? `${minutes}分${remaining}秒` : `${minutes}分`;
  },
  segments: count => `${count} 段`,
  running: '执行中',
  executionFailed: '执行失败',
  executionFinished: '执行结束',
  command: '命令',
  workingDirectory: '工作目录',
  output: '输出',
  webContent: '网页内容',
  iwencaiNews: '同花顺新闻',
  webSearch: '网页搜索',
  search: '搜索',
  textContent: '文本',
  webPage: '网页',
  shellCommand: '执行 Shell 命令',
  results: count => `${count} 条结果`,
  recognizedResults: count => `识别到 ${count} 条结果`,
  returnedResults: '已返回结果',
  inProgress: '进行中',
  searchCompacted: '搜索结果已交给 Agent 处理；当前压缩结果中没有可稳定提取的条目。',
  resultSummaryOnly: '为控制上下文长度，这里只展示可识别的结果摘要。',
  collapseRaw: '收起原始数据',
  viewRaw: '查看原始数据',
  rawData: '原始数据',
  requestFailed: '请求失败',
  returned: '已返回',
  contentTruncated: '内容已截断',
  contentPreview: '内容预览',
  responseTruncated: '响应内容超过本次抓取上限，Agent 使用的是截断后的内容。',
  fileChange: '文件变更',
  tool: '工具',
  arguments: '参数',
  result: '结果',
  executing: '正在执行',
  executionSteps: '执行步骤',
  items: count => `${count} 项`,
  failedItems: count => `${count} 项失败`,
  thinking: '思考中',
  thoughtCompleted: '思考完成',
  plan: '执行计划',
  permissionRequest: agent => `${agent} 请求权限`,
  protectedOperation: '执行受保护操作',
  operationArguments: '操作参数',
  allowOnce: '允许一次',
  allowSession: '本会话允许',
  reject: '拒绝',
  handled: '已处理',
  expired: '该请求已过期',
  usage: (input, output) => `输入 ${input} · 输出 ${output}`,
  contextUsage: (used, size) => `上下文 ${used} / ${size}`,
  attachment: '附件',
  operations: (count, failedCount) => {
    const suffix = failedCount ? ` · ${failedCount} 项失败` : '';
    return `执行 ${count} 项${suffix}`;
  },
  copyReply: '复制回复',
  copyReplySuccess: '已复制',
  copyReplyFailed: '复制失败',
};

// conversationCopy(copy) merges the ~70-key default copy table with the caller's
// overrides; it used to run once per component per render, so long transcripts
// rebuilt that object for every turn on every keystroke/stream chunk. The merged
// table is immutable and `copy` identities are stable (i18n dict tables), so cache
// the merge per source table (same WeakMap trick as ChatView's legacyMarkdownCache).
const conversationCopyCache = new WeakMap();
function conversationCopy(copy) {
  if (!copy) return DEFAULT_COPY;
  const cached = conversationCopyCache.get(copy);
  if (cached) return cached;
  const merged = { ...DEFAULT_COPY, ...copy };
  conversationCopyCache.set(copy, merged);
  return merged;
}

// Streaming markdown throttle window: rerun marked+DOMPurify+hljs on a 200ms budget instead of
// a full rerun for every streaming delta (O(n²)). When streaming ends, useThrottledValue
// guarantees a verbatim replay of the final full text.
const STREAMING_MARKDOWN_THROTTLE_MS = 200;

// The 1s clock is scoped to the smallest display subtree: the per-second tick used to live on
// ChatView top-level state, re-rendering the whole transcript every second while busy; now only
// the component showing elapsed time owns the tick. On mount/activation it first syncs a baseline,
// then starts the interval (same semantics as the old ChatView/CodexAcpView top-level ticker); no
// timer is created while inactive, and it is cleaned up on unmount.
// Exported for reuse by the ChatView composer activity indicator wrapper.
/**
 * @param {boolean} active - whether the displayed duration is currently advancing
 * @returns {number} a timestamp that advances once per second while active
 */
export function useConversationSecondClock(active) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- sync the clock baseline once on activation so elapsed time is correct immediately, before the first interval tick
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active]);
  return now;
}

function localizedSemanticLabel(value, copy) {
  return {
    同花顺新闻: copy.iwencaiNews,
    网页搜索: copy.webSearch,
    搜索: copy.search,
    文本: copy.textContent,
    网页内容: copy.webContent,
    网页: copy.webPage,
    '执行 Shell 命令': copy.shellCommand,
    工具: copy.tool,
  }[value] || value;
}

export function ConversationMarkdown({ text, className = '', onOpenExternal, onOpenResource, streaming = false }) {
  // lazy language registration bumps the version when it completes; the version must stay in the useMemo deps
  // (syntax-highlighter.js contract), otherwise already-rendered code stays plain text after registration completes.
  const syntaxVersion = useSyncExternalStore(subscribeSyntaxHighlight, getSyntaxHighlightVersion);
  // While the message is still streaming, parse the throttled snapshot instead of
  // every chunk (marked+DOMPurify+hljs over the full growing text is O(n²) per
  // chunk). The hook flushes the exact final text when `streaming` drops, so the
  // completed message always renders verbatim.
  const renderText = useThrottledValue(text, STREAMING_MARKDOWN_THROTTLE_MS, streaming);
  // eslint-disable-next-line react-hooks/exhaustive-deps -- syntaxVersion is a version counter; any change requires recomputing to restore highlighting
  const html = useMemo(() => renderMarkdown(renderText), [renderText, syntaxVersion]);
  const openLink = (event) => {
    const anchor = event.target && event.target.closest && event.target.closest('a[href]');
    if (!anchor) return;
    const href = String(anchor.getAttribute('href') || '').trim();
    if (href.startsWith('#')) return;
    event.preventDefault();
    const external = externalMarkdownUrl(href);
    if (external && onOpenExternal) onOpenExternal(external);
    else {
      const resource = workspaceMarkdownResource(href);
      if (resource && onOpenResource) onOpenResource(resource);
    }
  };
  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents: link-interception layer; the keyboard path is covered by the rendered <a>'s own focus
    // biome-ignore lint/a11y/noStaticElementInteractions: static rich-text container; onClick only intercepts links to open them externally
    <div
      className={`codex-markdown conversation-markdown text-[15px] leading-7 ${className}`}
      onClick={openLink}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

export function ConversationStatusBadge({ status, copy }) {
  const c = conversationCopy(copy);
  const done = ['Completed', 'completed', 'done', 'end_turn'].includes(status);
  const failed = ['Failed', 'failed', 'Refused'].includes(status);
  const interrupted = ['Interrupted', 'interrupted', 'incomplete'].includes(status);
  const stopped = interrupted || status === 'LimitReached';
  const label = done
    ? c.completed
    : failed
      ? c.failed
      : interrupted
        ? c.interrupted
        : status === 'LimitReached'
          ? c.limitReached
          : c.processing;
  return (
    <span className={`inline-flex items-center gap-1 text-[11px] px-2 py-0.5 rounded-full ${
      done ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-300'
        : failed ? 'bg-red-500/10 text-red-600 dark:text-red-300'
          : stopped ? 'bg-amber-500/10 text-amber-600 dark:text-amber-300'
            : 'bg-blue-500/10 text-blue-600 dark:text-blue-300'
    }`}>
      {done
        ? <CheckCircle2 size={12} />
        : failed || stopped
          ? <AlertTriangle size={12} />
          : <span className="w-1.5 h-1.5 rounded-full bg-current animate-pulse" />}
      {label}
    </span>
  );
}

export function ConversationActivityIndicator({
  turn,
  now = 0,
  onRequestAttention,
  className = '',
  copy,
}) {
  const c = conversationCopy(copy);
  // elapsedMs falls back to Date.now() when now=0; reading the current time during render only displays elapsed time and does not feed state derivation.
  // eslint-disable-next-line react-hooks/purity -- elapsed time is a display value that naturally drifts over time; re-rendering is driven by the parent's polling of now
  const nowMs = now || Date.now();
  if (!turn || turn.status !== 'running') return null;
  const waitingPermission = turn.waitingPermission
    || (turn.permissions || []).some(permission => !permission.resolved);
  const waitingInput = turn.waitingInput
    || (turn.elicitations || []).some(elicitation => !elicitation.resolved);
  const waitingAttention = waitingPermission || waitingInput;
  const label = waitingPermission
    ? c.waitingPermission
    : waitingInput
      ? c.waitingInput
      : c.processing;
  const content = (
    <>
      {waitingAttention
        ? <span className="w-2 h-2 rounded-full bg-amber-500 animate-pulse" />
        : <span className="w-3 h-3 rounded-full border-2 border-current/20 border-t-current animate-spin" />}
      <span>{label} · {c.elapsed(elapsedMs(turn.startedAt, null, nowMs))}</span>
    </>
  );
  const sharedClass = `h-6 flex items-center gap-2 px-1 text-[12px] ${
    waitingAttention
      ? 'text-amber-600 dark:text-amber-300'
      : 'text-gray-500 dark:text-gray-400'
  } ${className}`;
  if (waitingAttention && onRequestAttention) {
    return (
      <button type="button" onClick={onRequestAttention}
        aria-label={c.goLatest(label)}
        className={`${sharedClass} hover:text-amber-700 dark:hover:text-amber-200`}>
        {content}
      </button>
    );
  }
  return <div role="status" aria-live="polite" className={sharedClass}>{content}</div>;
}

export function TerminalBlock({ label, text }) {
  if (!text) return null;
  return (
    <div className="mt-3 min-w-0 max-w-full">
      <div className="mb-1.5 text-[10px] font-medium uppercase tracking-wider text-gray-400">{label}</div>
      <pre className="max-h-80 max-w-full overflow-auto whitespace-pre rounded-xl bg-[#F4F5F7] dark:bg-black/30 px-3 py-2.5 text-[12px] leading-5 font-mono text-gray-700 dark:text-gray-200">{text}</pre>
    </div>
  );
}

export function StructuredValue({ label, value }) {
  if (value == null || value === '' || (Array.isArray(value) && !value.length)) return null;
  if (typeof value !== 'object') return <TerminalBlock label={label} text={String(value)} />;
  const entries = Object.entries(value);
  if (!entries.length) return null;
  return (
    <div className="mt-3">
      <div className="mb-1.5 text-[10px] font-medium uppercase tracking-wider text-gray-400">{label}</div>
      <div className="rounded-xl border border-black/[0.05] dark:border-white/[0.07] overflow-hidden">
        {entries.map(([key, entry]) => (
          <div key={key} className="grid grid-cols-[120px_minmax(0,1fr)] border-b last:border-b-0 border-black/[0.05] dark:border-white/[0.06] text-[11px]">
            <div className="px-3 py-2 bg-black/[0.025] dark:bg-white/[0.025] text-gray-400 font-mono">{key}</div>
            <pre className="px-3 py-2 overflow-x-auto whitespace-pre-wrap font-mono text-gray-700 dark:text-gray-200">
              {typeof entry === 'string' ? entry : JSON.stringify(entry, null, 2)}
            </pre>
          </div>
        ))}
      </div>
    </div>
  );
}

export function CompactItemRow({ icon, title, meta, status, open, onToggle, controlsId }) {
  const tone = status === 'failed'
    ? 'text-red-500 bg-red-500/10'
    : status === 'warning'
      ? 'text-amber-500 bg-amber-500/10'
    : status === 'running'
      ? 'text-blue-500 bg-blue-500/10'
      : 'text-gray-500 bg-black/[0.04] dark:bg-white/[0.06]';
  return (
    <button type="button" onClick={onToggle}
      data-testid="conversation-compact-item-toggle"
      aria-expanded={controlsId ? Boolean(open) : undefined}
      aria-controls={controlsId && open ? controlsId : undefined}
      className="w-full min-w-0 min-h-10 overflow-hidden px-2.5 py-2 flex items-center gap-2.5 text-left rounded-xl hover:bg-black/[0.025] dark:hover:bg-white/[0.035]">
      <span className={`w-6 h-6 shrink-0 rounded-lg flex items-center justify-center ${tone}`}>{icon}</span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[12px] font-medium">{title}</span>
        {meta && <span className="block mt-0.5 text-[10px] text-gray-400">{meta}</span>}
      </span>
      {status === 'running' && <span className="w-1.5 h-1.5 rounded-full bg-blue-500 animate-pulse" />}
      <ChevronDown size={13} className={`shrink-0 text-gray-400 transition-transform ${open ? 'rotate-180' : ''}`} />
    </button>
  );
}

function CommandExecutionItem({ item, now, copy }) {
  const c = conversationCopy(copy);
  const details = commandExecutionDetails(item.tool);
  const state = terminalStatus(item.status, details.exitCode);
  const [open, setOpen] = useState(false);
  const detailsId = useId();
  const countHint = details.commandCount > 1 ? ` · ${c.segments(details.commandCount)}` : '';
  const duration = c.elapsed(elapsedMs(item.startedAt, item.completedAt, now));
  const exitHint = details.exitCode == null ? '' : ` · exit ${details.exitCode}`;
  const outcome = state === 'running'
    ? `${c.running} · ${duration}`
    : state === 'failed'
      ? `${c.executionFailed}${exitHint}`
      : `${c.executionFinished}${exitHint} · ${duration}`;
  return (
    <div className={`rounded-xl border ${state === 'failed' ? 'border-red-500/20' : 'border-black/[0.05] dark:border-white/[0.07]'} bg-white/45 dark:bg-white/[0.015]`}>
      <CompactItemRow icon={<Terminal size={13} />} title={localizedSemanticLabel(details.summary, c)}
        meta={`${outcome}${countHint}`} status={state} open={open} controlsId={detailsId}
        onToggle={() => setOpen(value => !value)} />
      {open && (
        <div id={detailsId} data-testid="conversation-compact-item-content" className="px-3 pb-3 border-t border-black/[0.05] dark:border-white/[0.06]">
          <TerminalBlock label={c.command} text={details.command} />
          {details.cwd && (
            <div className="mt-2 text-[10px] text-gray-400">
              {c.workingDirectory} <span className="ml-1 font-mono text-gray-600 dark:text-gray-300">{details.cwd}</span>
            </div>
          )}
          <TerminalBlock label={c.output} text={details.output} />
        </div>
      )}
    </div>
  );
}

function SearchToolItem({ item, now, onOpenExternal, copy }) {
  const c = conversationCopy(copy);
  const tool = item.tool || {};
  const details = searchToolDetails(tool);
  const state = terminalStatus(item.status);
  const [open, setOpen] = useState(false);
  const [rawOpen, setRawOpen] = useState(false);
  const detailsId = useId();
  const duration = c.elapsed(elapsedMs(item.startedAt, item.completedAt, now));
  const query = details.query || tool.title || c.webContent;
  const toolName = String(tool.name || '').trim() || 'web_search';
  const queryLabel = query.length > 48 ? `${query.slice(0, 48)}…` : query;
  const resultLabel = details.count == null
    ? details.results.length
      ? c.recognizedResults(details.results.length)
      : c.returnedResults
    : c.results(details.count);
  const sourceLabel = localizedSemanticLabel(details.source, c);
  const meta = state === 'running'
    ? `${queryLabel} · ${sourceLabel} · ${c.inProgress} · ${duration}`
    : state === 'failed'
      ? `${queryLabel} · ${sourceLabel} · ${c.failed}`
      : `${queryLabel} · ${sourceLabel} · ${resultLabel}`;
  return (
    <div className={`rounded-xl border ${state === 'failed' ? 'border-red-500/20' : 'border-black/[0.05] dark:border-white/[0.07]'} bg-white/45 dark:bg-white/[0.015]`}>
      <CompactItemRow icon={<Wrench size={13} />} title={toolName}
        meta={meta} status={state} open={open} controlsId={detailsId}
        onToggle={() => setOpen(value => !value)} />
      {open && (
        <div id={detailsId} data-testid="conversation-compact-item-content" className="px-3 pb-3 border-t border-black/[0.05] dark:border-white/[0.06]">
          {details.results.length > 0 ? (
            <div className="mt-2 divide-y divide-black/[0.05] dark:divide-white/[0.06]">
              {details.results.slice(0, 5).map((result, index) => {
                let domain = '';
                try { domain = new URL(result.url).hostname.replace(/^www\./, ''); } catch { /* invalid URL: don't show the domain */ }
                return (
                  <button
                    key={result.url}
                    type="button"
                    onClick={() => onOpenExternal && onOpenExternal(result.url)}
                    disabled={!onOpenExternal}
                    className="w-full py-2 flex items-start gap-2.5 text-left enabled:hover:text-blue-600 dark:enabled:hover:text-blue-300 disabled:cursor-default"
                  >
                    <span className="mt-0.5 w-5 h-5 shrink-0 rounded-md bg-black/[0.035] dark:bg-white/[0.06] text-[10px] text-gray-400 flex items-center justify-center">
                      {index + 1}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block text-[12px] leading-5 text-gray-700 dark:text-gray-200">{result.title}</span>
                      {domain && <span className="block mt-0.5 truncate text-[10px] text-gray-400">{domain}</span>}
                    </span>
                  </button>
                );
              })}
            </div>
          ) : state === 'completed' ? (
            <div className="mt-2 text-[11px] leading-5 text-gray-400">
              {c.searchCompacted}
            </div>
          ) : null}
          {details.compacted && (
            <div className="mt-2 text-[10px] text-gray-400">{c.resultSummaryOnly}</div>
          )}
          {details.rawOutput && (
            <div className="mt-2">
              <button
                type="button"
                onClick={() => setRawOpen(value => !value)}
                className="text-[10px] text-gray-400 hover:text-gray-700 dark:hover:text-gray-200"
              >
                {rawOpen ? c.collapseRaw : c.viewRaw}
              </button>
              {rawOpen && <TerminalBlock label={c.rawData} text={details.rawOutput} />}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function FetchToolItem({ item, now, onOpenExternal, copy }) {
  const c = conversationCopy(copy);
  const tool = item.tool || {};
  const details = fetchToolDetails(tool);
  const state = terminalStatus(item.status);
  const responseWarning = details.status != null && details.status >= 400;
  const visualState = responseWarning && state !== 'failed' ? 'warning' : state;
  const [open, setOpen] = useState(false);
  const [rawOpen, setRawOpen] = useState(false);
  const detailsId = useId();
  const duration = c.elapsed(elapsedMs(item.startedAt, item.completedAt, now));
  const toolName = String(tool.name || '').trim() || 'fetch_url';
  const statusLabel = details.status == null
    ? state === 'failed'
      ? c.requestFailed
      : c.returned
    : `HTTP ${details.status}`;
  const targetLabel = localizedSemanticLabel(details.target, c);
  const contentTypeLabel = localizedSemanticLabel(details.contentTypeLabel, c);
  const truncatedHint = details.truncated ? ` · ${c.contentTruncated}` : '';
  const meta = state === 'running'
    ? `${targetLabel} · ${c.inProgress} · ${duration}`
    : `${targetLabel} · ${statusLabel} · ${contentTypeLabel}${truncatedHint}`;
  return (
    <div className={`rounded-xl border ${
      state === 'failed'
        ? 'border-red-500/20'
        : responseWarning
          ? 'border-amber-500/25'
          : 'border-black/[0.05] dark:border-white/[0.07]'
    } bg-white/45 dark:bg-white/[0.015]`}>
      <CompactItemRow icon={<Wrench size={13} />} title={toolName}
        meta={meta} status={visualState} open={open} controlsId={detailsId}
        onToggle={() => setOpen(value => !value)} />
      {open && (
        <div id={detailsId} data-testid="conversation-compact-item-content" className="px-3 pb-3 border-t border-black/[0.05] dark:border-white/[0.06]">
          {details.url && (
            <button
              type="button"
              onClick={() => onOpenExternal && onOpenExternal(details.url)}
              disabled={!onOpenExternal}
              title={details.url}
              className="mt-2 block max-w-full truncate text-left text-[11px] text-blue-600 dark:text-blue-300 enabled:hover:underline disabled:text-gray-400 disabled:cursor-default"
            >
              {details.url}
            </button>
          )}
          {details.preview && (
            <div className="mt-2">
              <div className="mb-1 text-[10px] font-medium text-gray-400">{c.contentPreview}</div>
              <div className="max-h-24 overflow-hidden rounded-lg bg-black/[0.025] dark:bg-white/[0.035] px-3 py-2 text-[11px] leading-5 text-gray-600 dark:text-gray-300">
                {details.preview}{details.contentLength > details.preview.length ? '…' : ''}
              </div>
            </div>
          )}
          {details.truncated && (
            <div className="mt-2 text-[10px] text-gray-400">{c.responseTruncated}</div>
          )}
          {details.rawOutput && (
            <div className="mt-2">
              <button
                type="button"
                onClick={() => setRawOpen(value => !value)}
                className="text-[10px] text-gray-400 hover:text-gray-700 dark:hover:text-gray-200"
              >
                {rawOpen ? c.collapseRaw : c.viewRaw}
              </button>
              {rawOpen && <TerminalBlock label={c.rawData} text={details.rawOutput} />}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export function WorkspaceResourceButtons({ resources, onOpenResource }) {
  if (!resources.length) return null;
  return (
    <div className="flex flex-wrap gap-1.5 px-3 pb-2">
      {resources.map(resource => (
        <button type="button" key={resource.path} onClick={() => onOpenResource && onOpenResource(resource.path)}
          disabled={!onOpenResource} title={resource.path}
          className="max-w-full truncate px-2 py-1 rounded-lg bg-blue-500/8 text-[10px] text-blue-600 dark:text-blue-300 font-mono enabled:hover:bg-blue-500/15 disabled:cursor-default">
          {resource.name}
        </button>
      ))}
    </div>
  );
}

function GenericToolItem({ item, now, onOpenResource, copy }) {
  const c = conversationCopy(copy);
  const tool = item.tool || {};
  const state = terminalStatus(item.status);
  const [open, setOpen] = useState(false);
  const detailsId = useId();
  const duration = c.elapsed(elapsedMs(item.startedAt, item.completedAt, now));
  const label = item.type === 'file_change' ? c.fileChange : (tool.kind || c.tool);
  const stateLabel = state === 'running'
    ? `${c.inProgress} · ${duration}`
    : state === 'failed'
      ? c.failed
      : `${c.executionFinished} · ${duration}`;
  const resources = toolWorkspaceResources(tool);
  return (
    <div className="rounded-xl border border-black/[0.05] dark:border-white/[0.07] bg-white/45 dark:bg-white/[0.015]">
      <CompactItemRow icon={<Wrench size={13} />} title={localizedSemanticLabel(tool.title || label, c)}
        meta={`${label} · ${stateLabel}`}
        status={state} open={open} controlsId={detailsId}
        onToggle={() => setOpen(value => !value)} />
      <WorkspaceResourceButtons resources={resources} onOpenResource={onOpenResource} />
      {open && (
        <div id={detailsId} data-testid="conversation-compact-item-content" className="px-3 pb-3 border-t border-black/[0.05] dark:border-white/[0.06]">
          <StructuredValue label={c.arguments} value={tool.rawInput} />
          <StructuredValue label={c.result} value={tool.rawOutput == null ? tool.content : tool.rawOutput} />
        </div>
      )}
    </div>
  );
}

function ToolItem({ item, now, renderToolItem, onOpenExternal, onOpenResource, copy }) {
  const custom = renderToolItem && renderToolItem(item);
  if (custom !== undefined) return custom;
  if (item.type === 'command_execution') {
    return <CommandExecutionItem item={item} now={now} copy={copy} />;
  }
  if (isSearchTool(item.tool)) {
    return <SearchToolItem item={item} now={now} onOpenExternal={onOpenExternal} copy={copy} />;
  }
  if (isFetchTool(item.tool)) {
    return <FetchToolItem item={item} now={now} onOpenExternal={onOpenExternal} copy={copy} />;
  }
  return <GenericToolItem item={item} now={now} onOpenResource={onOpenResource} copy={copy} />;
}

function runningToolLabel(item, copy) {
  if (!item) return '';
  if (item.type === 'command_execution' || String(item.tool && item.tool.kind || '').toLowerCase() === 'execute') {
    return copy.shellCommand;
  }
  if (item.type === 'file_change') return copy.fileChange;
  const name = String(item.tool && item.tool.name || '').trim();
  return name || copy.tool;
}

function ToolGroup({ group, now, renderToolItem, onOpenExternal, onOpenResource, copy }) {
  const c = conversationCopy(copy);
  const items = group.items || [];
  // Single pass instead of `.some` + `.filter` + reverse().`find` (three scans
  // per group per render): `runningItem` ends at the LAST running item, matching
  // the previous reverse().find.
  let running = false;
  let failedCount = 0;
  let runningItem = null;
  for (const item of items) {
    if (terminalStatus(item.status) === 'running') {
      running = true;
      runningItem = item;
    }
    if (countsAsFailedOperation(item)) failedCount += 1;
  }
  const failed = failedCount > 0;
  const runningLabel = runningToolLabel(runningItem, c);
  const runningSuffix = runningLabel ? ` · ${runningLabel}` : '';
  const leadLabel = running ? `${c.executing}${runningSuffix}` : c.executionSteps;
  const failedSuffix = failedCount ? ` · ${c.failedItems(failedCount)}` : '';
  const summary = `${leadLabel} · ${c.items(items.length)}${failedSuffix}`;
  const [open, setOpen] = useState(running);
  const expanded = running || open;
  const detailsId = useId();
  const hasDetails = items.length > 0;
  const resources = collectToolWorkspaceResources(items);
  return (
    <div className="min-w-0 max-w-full">
      <button type="button" onClick={() => setOpen(value => !value)}
        data-testid="conversation-tool-group-summary"
        aria-expanded={hasDetails ? Boolean(expanded) : undefined}
        aria-controls={hasDetails && expanded ? detailsId : undefined}
        className="w-full min-w-0 h-9 overflow-hidden px-1 flex items-center gap-2 text-left text-[12px] text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200">
        <span className={`w-1.5 h-1.5 shrink-0 rounded-full ${failed ? 'bg-red-500' : running ? 'bg-blue-500 animate-pulse' : 'bg-gray-300 dark:bg-gray-600'}`} />
        <span className="min-w-0 flex-1 truncate">{summary}</span>
        <ChevronDown size={13} className={`shrink-0 transition-transform ${expanded ? 'rotate-180' : ''}`} />
      </button>
      <WorkspaceResourceButtons resources={resources} onOpenResource={onOpenResource} />
      {expanded && hasDetails && (
        <div id={detailsId} data-testid="conversation-tool-group-content" className="min-w-0 max-w-full ml-3 pl-3 border-l border-black/[0.06] dark:border-white/[0.08] space-y-1.5 pb-1">
          {items.map(item => (
            <ToolItem
              key={item.id}
              item={item}
              now={now}
              renderToolItem={renderToolItem}
              onOpenExternal={onOpenExternal}
              onOpenResource={onOpenResource}
              copy={c}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function ReasoningItem({ item, now, copy }) {
  const c = conversationCopy(copy);
  const running = item.status === 'in_progress';
  const [open, setOpen] = useState(false);
  const detailsId = useId();
  const hasDetails = Boolean(item.text);
  const duration = item.startedAt == null
    ? ''
    : c.elapsed(elapsedMs(item.startedAt, item.completedAt, now));
  const statusText = running ? c.thinking : c.thoughtCompleted;
  return (
    <div className="min-w-0 max-w-full">
      <button type="button" onClick={() => setOpen(value => !value)}
        data-testid="conversation-reasoning-toggle"
        aria-expanded={hasDetails ? Boolean(open) : undefined}
        aria-controls={hasDetails && open ? detailsId : undefined}
        className="w-full h-9 px-1 flex items-center gap-2 text-left text-[12px] text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200">
        <span className={`w-1.5 h-1.5 rounded-full bg-violet-500 ${running ? 'animate-pulse' : ''}`} />
        <span>{statusText}{duration ? ` · ${duration}` : ''}</span>
        <ChevronDown size={13} className={`ml-auto transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>
      {open && hasDetails && (
        <div id={detailsId} data-testid="conversation-reasoning-content" className="min-w-0 max-w-full ml-3 pl-3 py-1 border-l border-violet-500/15 text-[12px] leading-6 text-gray-500 dark:text-gray-300 whitespace-pre-wrap break-words [overflow-wrap:anywhere]">
          {item.text}
        </div>
      )}
    </div>
  );
}

export function PlanBlock({ plan, copy }) {
  const c = conversationCopy(copy);
  const entries = plan && plan.entries || [];
  if (!entries.length) return null;
  return (
    <div data-testid="conversation-plan" className="min-w-0 max-w-full rounded-2xl border border-violet-500/15 bg-violet-500/[0.04] p-3.5">
      <div className="text-[12px] font-semibold text-violet-600 dark:text-violet-300 mb-2">{c.plan}</div>
      <div className="space-y-2">
        {entries.map((entry, index) => (
          <div key={index} className="min-w-0 flex items-start gap-2 text-[13px]">
            <span className={`mt-1.5 w-2 h-2 shrink-0 rounded-full ${
              entry.status === 'completed' ? 'bg-emerald-500' : entry.status === 'in_progress' ? 'bg-blue-500 animate-pulse' : 'bg-gray-300 dark:bg-gray-600'
            }`} />
            <span className="min-w-0 flex-1 whitespace-pre-wrap break-words [overflow-wrap:anywhere]">{entry.content}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function PermissionCard({ permission, pending, onRespond, responding, agentLabel, copy }) {
  const c = conversationCopy(copy);
  const request = permission.request || {};
  const tool = request.toolCall || {};
  const options = request.options || [];
  const actionable = !!pending && !permission.resolved;
  return (
    <div className="rounded-2xl border border-amber-500/25 bg-amber-500/[0.06] p-4">
      <div className="flex items-start gap-3">
        <AlertTriangle size={18} className="text-amber-500 mt-0.5 shrink-0" />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold">{c.permissionRequest(agentLabel)}</div>
          <div className="mt-1 min-w-0 max-w-full text-[12px] text-gray-500 dark:text-gray-400 break-words [overflow-wrap:anywhere]">{tool.title || c.protectedOperation}</div>
          {tool.rawInput && tool.rawInput.command
            ? <TerminalBlock label={c.command} text={String(tool.rawInput.command)} />
            : <StructuredValue label={c.operationArguments} value={tool.rawInput} />}
          <div className="mt-3 flex flex-wrap gap-2">
            {options.map(option => (
              <button type="button" key={option.optionId} disabled={!actionable || responding}
                onClick={() => onRespond(permission.toolCallId, option.optionId)}
                className={`max-w-full min-w-0 whitespace-normal break-all px-3 py-1.5 rounded-xl text-[12px] leading-5 font-medium transition-colors ${
                  String(option.kind || '').startsWith('allow')
                    ? 'bg-blue-600 text-white hover:bg-blue-700'
                    : 'bg-black/[0.06] dark:bg-white/10 hover:bg-black/10 dark:hover:bg-white/15'
                } disabled:opacity-45 disabled:cursor-not-allowed`}>
                {option.optionId === 'allow_once'
                  ? c.allowOnce
                  : option.optionId === 'allow_always'
                    ? c.allowSession
                    : option.optionId === 'reject_once'
                      ? c.reject
                      : option.name}
              </button>
            ))}
          </div>
          {!actionable && <div className="mt-2 text-[11px] text-gray-400">{permission.resolved ? c.handled : c.expired}</div>}
        </div>
      </div>
    </div>
  );
}

function DefaultItem({
  item,
  now,
  pendingByTool,
  onRespond,
  responding,
  renderToolItem,
  onOpenExternal,
  onOpenResource,
  agentLabel,
  copy,
}) {
  if (item.type === 'reasoning') return <ReasoningItem item={item} now={now} copy={copy} />;
  if (item.type === 'tool_group') {
    return (
      <ToolGroup
        group={item}
        now={now}
        renderToolItem={renderToolItem}
        onOpenExternal={onOpenExternal}
        onOpenResource={onOpenResource}
        copy={copy}
      />
    );
  }
  if (['tool', 'command_execution', 'file_change'].includes(item.type)) {
    return (
      <ToolItem
        item={item}
        now={now}
        renderToolItem={renderToolItem}
        onOpenExternal={onOpenExternal}
        onOpenResource={onOpenResource}
        copy={copy}
      />
    );
  }
  if (item.type === 'plan') return <PlanBlock plan={item.plan} copy={copy} />;
  if (item.type === 'permission') {
    return (
      <PermissionCard
        permission={item.permission}
        pending={pendingByTool[item.permission.toolCallId]}
        onRespond={onRespond}
        responding={responding}
        agentLabel={agentLabel}
        copy={copy}
      />
    );
  }
  if (item.type === 'agent_message') {
    const commentary = item.phase === 'commentary';
    // streaming = the projection's in_progress convention (deepseek/ACP projections agree):
    // while text can still grow, render through the throttle; when it ends, the full text is replayed verbatim.
    return commentary
      ? <ConversationMarkdown text={item.text} onOpenExternal={onOpenExternal} onOpenResource={onOpenResource}
          streaming={item.status === 'in_progress'}
          className="text-[13px] leading-6 text-gray-500 dark:text-gray-400" />
      : <ConversationMarkdown text={item.text} onOpenExternal={onOpenExternal} onOpenResource={onOpenResource}
          streaming={item.status === 'in_progress'} />;
  }
  return null;
}

// Default props must keep stable identities or React.memo below would see fresh
// objects/functions on every parent render and never skip work.
const EMPTY_PENDING_BY_TOOL = Object.freeze({});
const noopOnRespond = () => {};
/** @type {{ id: string, status: string }[]} */
const EMPTY_TURNS = [];

// The timeline projections (deepseek-conversation.js / acp-state.js) rebuild every turn object on
// each run: the container reference is always fresh, but unchanged turns have identical leaf content
// (bridge snapshots share unchanged values by reference). So `turn` gets a structural comparison
// (bridge subscription state only allows JSON values, so no functions/cycles), letting unchanged
// turns skip re-rendering; the other props (parent-stabilized callbacks, memoized copy/avatar)
// compare by reference.

/**
 * Structural equality for JSON-like conversation data (bridge subscription state
 * admits no functions or cycles, so recursion is safe).
 * @param {unknown} left - previous value
 * @param {unknown} right - next value
 * @returns {boolean} true when both values are structurally equal
 */
function isEqualConversationValue(left, right) {
  if (Object.is(left, right)) return true;
  if (Array.isArray(left)) {
    if (!Array.isArray(right) || left.length !== right.length) return false;
    for (let index = 0; index < left.length; index += 1) {
      if (!isEqualConversationValue(left[index], right[index])) return false;
    }
    return true;
  }
  if (!left || !right || typeof left !== 'object' || typeof right !== 'object') return false;
  const leftObject = /** @type {Record<string, unknown>} */ (left);
  const rightObject = /** @type {Record<string, unknown>} */ (right);
  const leftKeys = Object.keys(leftObject);
  const rightKeys = Object.keys(rightObject);
  if (leftKeys.length !== rightKeys.length) return false;
  for (let index = 0; index < leftKeys.length; index += 1) {
    const key = leftKeys[index];
    // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 floor; Object.hasOwn is unavailable and this call is already the safe form
    if (!Object.prototype.hasOwnProperty.call(rightObject, key)) return false;
    if (!isEqualConversationValue(leftObject[key], rightObject[key])) return false;
  }
  return true;
}

/**
 * @param {Record<string, unknown>} prev - previous props
 * @param {Record<string, unknown>} next - next props
 * @returns {boolean} true when the turn subtree should skip re-rendering
 */
function areConversationTurnPropsEqual(prev, next) {
  const keys = Object.keys(next);
  if (keys.length !== Object.keys(prev).length) return false;
  for (let index = 0; index < keys.length; index += 1) {
    const key = keys[index];
    if (key === 'turn') {
      if (!isEqualConversationValue(prev.turn, next.turn)) return false;
      continue;
    }
    if (!Object.is(prev[key], next[key])) return false;
  }
  return true;
}

function ConversationTurnView({
  turn,
  now,
  pendingByTool = EMPTY_PENDING_BY_TOOL,
  onRespond = noopOnRespond,
  responding = false,
  renderUser,
  renderItem,
  renderToolItem,
  onOpenExternal,
  onOpenResource,
  agentLabel = 'Agent',
  assistantAvatar,
  copy,
}) {
  const c = conversationCopy(copy);
  const running = turn.status === 'running';
  // The per-second tick is scoped to the running turn itself: the parent may still pass a ticking `now`
  // (CodexAcpView keeps its original behavior via `now || tickNow` prop precedence); when omitted, an
  // internal clock drives elapsed time, re-rendering only this one turn subtree per second.
  const tickNow = useConversationSecondClock(running);
  const effectiveNow = now || tickNow;
  const waitingPermission = turn.waitingPermission
    || (turn.permissions || []).some(permission => !permission.resolved);
  const waitingInput = turn.waitingInput
    || (turn.elicitations || []).some(elicitation => !elicitation.resolved);
  const waitingAttention = waitingPermission || waitingInput;
  const duration = c.elapsed(elapsedMs(turn.startedAt, turn.completedAt, effectiveNow));
  const showTerminalDuration = Boolean(turn.startedAt && turn.completedAt);
  const presentation = turn.presentation || turn.items || [];
  const operationCount = Number(turn.operationCount || 0);
  const failedOperationCount = Number(turn.failedOperationCount || 0);
  const turnUsage = turn.usage || null;
  const usageLabel = turnUsage && ('inputTokens' in turnUsage || 'outputTokens' in turnUsage)
    ? c.usage(Number(turnUsage.inputTokens || 0).toLocaleString(), Number(turnUsage.outputTokens || 0).toLocaleString())
    : turnUsage && turnUsage.size
      ? c.contextUsage(Number(turnUsage.used || 0).toLocaleString(), Number(turnUsage.size || 0).toLocaleString())
      : '';
  const userAttachments = Array.isArray(turn.userAttachments) ? turn.userAttachments : [];
  const assistantAvailable = assistantResponseAvailable(turn);
  // Footer/visibility: a turn with no assistant content must not render an
  // avatar-only row. Live repro: a steered message sandwiched between two
  // consecutive injections has no items of its own — the unconditional avatar
  // column showed a lone pinvou avatar between the two user bubbles. The row
  // renders only for running turns (activity indicator), turns with items, or
  // turns carrying a terminal footer (badge/duration/usage/error).
  const assistantFooterVisible = !running
    && (assistantAvailable || turn.lifecycleKnown || turn.completedAt || turn.error);
  const assistantRowVisible = running || presentation.length > 0 || assistantFooterVisible;
  const userContent = renderUser && turn.userItem
    ? renderUser(turn.userItem, turn)
    : (turn.userText || userAttachments.length)
      ? (
          <div className="flex justify-end">
            <div className="max-w-[78%] rounded-[20px] rounded-br-md bg-[#E9EEF6] dark:bg-[#2A2B2E] px-4 py-3 text-[14px] leading-6 whitespace-pre-wrap break-words">
              {turn.userText && <div>{turn.userText}</div>}
              {userAttachments.length > 0 && (
                <div className={`flex flex-wrap gap-1.5 ${turn.userText ? 'mt-2' : ''}`}>
                  {userAttachments.map((attachment, index) => (
                    <span
                      key={`${attachment.name || 'attachment'}-${index}`}
                      className="inline-flex max-w-full items-center gap-1 rounded-lg bg-white/65 dark:bg-white/[0.07] px-2 py-1 text-[11px] leading-4"
                    >
                      <FileTypeIcon name={attachment.name} className="h-4 w-4 shrink-0" />
                      <span className="truncate">{attachment.name || c.attachment}</span>
                    </span>
                  ))}
                </div>
              )}
            </div>
          </div>
        )
      : null;

  return (
    <section className="space-y-4" data-conversation-turn={turn.id} style={{ contentVisibility: 'auto', containIntrinsicSize: 'auto 600px' }}>
      {userContent}
      {assistantRowVisible && (
      <div className="flex items-start gap-3">
        {assistantAvatar || (
          <div className="mt-1 w-7 h-7 rounded-xl bg-gradient-to-br from-[#34A853] to-[#168C46] text-white flex items-center justify-center shrink-0 shadow-sm">
            <Sparkles size={15} />
          </div>
        )}
        <div className="min-w-0 flex-1 space-y-1">
          {running && (
            <div className={`h-9 flex items-center gap-2 text-[12px] ${waitingAttention ? 'text-amber-600 dark:text-amber-300' : 'text-gray-500 dark:text-gray-400'}`}>
              <span className={`w-1.5 h-1.5 rounded-full ${waitingAttention ? 'bg-amber-500' : 'bg-emerald-500 animate-pulse'}`} />
              {waitingPermission ? c.waitingPermission : waitingInput ? c.waitingInputShort : (turn.activityToolName ? c.callingTool(turn.activityToolName) : c.processingActive)} · {duration}
            </div>
          )}
          {presentation.map((item, index) => {
            const context = { turn, now: effectiveNow, pendingByTool, onRespond, responding };
            const custom = renderItem && renderItem(item, context);
            if (custom !== undefined) {
              return <React.Fragment key={item.id || `${item.type}-${index}`}>{custom}</React.Fragment>;
            }
            return (
              <DefaultItem
                key={item.id || `${item.type}-${index}`}
                item={item}
                now={effectiveNow}
                pendingByTool={pendingByTool}
                onRespond={onRespond}
                responding={responding}
                renderToolItem={renderToolItem}
                onOpenExternal={onOpenExternal}
                onOpenResource={onOpenResource}
                agentLabel={agentLabel}
                copy={c}
              />
            );
          })}
          {assistantFooterVisible && <AssistantMessageFooter>
            {assistantAvailable && (
              <AssistantMessageActions resolveText={() => assistantResponseText(turn)} copy={c} />
            )}
            {(turn.lifecycleKnown || turn.completedAt || turn.error) && <>
              <ConversationStatusBadge status={turn.status} copy={c} />
              {showTerminalDuration && <span className="text-[11px] text-gray-400">{duration}</span>}
              {operationCount > 0 && (
                <span className="text-[11px] text-gray-400">
                  {c.operations(operationCount, failedOperationCount)}
                </span>
              )}
              {usageLabel && <span className="text-[11px] text-gray-400">{usageLabel}</span>}
              {turn.error && <span className="text-[11px] text-red-500">{turn.error}</span>}
            </>}
          </AssistantMessageFooter>}
        </div>
      </div>
      )}
    </section>
  );
}

// Declare the component separately, then wrap it in memo: prop types are inferred from the
// component's own parameters, so the comparator's Record<string, unknown> parameter does not
// pollute the inference in reverse.
export const ConversationTurn = React.memo(ConversationTurnView, areConversationTurnPropsEqual);

export function ConversationTimeline({ turns = EMPTY_TURNS, ...props }) {
  return (
    <>
      {turns.map(turn => <ConversationTurn key={turn.id} turn={turn} {...props} />)}
    </>
  );
}
