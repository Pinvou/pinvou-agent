import React, { useEffect, useMemo, useState } from 'react';
import DOMPurify from 'dompurify';
import { marked } from 'marked';
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
  elapsedMs,
  externalMarkdownUrl,
  fetchToolDetails,
  formatElapsed,
  isFetchTool,
  isSearchTool,
  searchToolDetails,
  terminalStatus,
} from './conversation-model.js';

export function ConversationMarkdown({ text, className = '', onOpenExternal }) {
  const html = useMemo(() => DOMPurify.sanitize(marked.parse(String(text || '')), {
    USE_PROFILES: { html: true },
    FORBID_TAGS: ['script', 'style', 'iframe', 'object', 'embed'],
  }), [text]);
  const openLink = (event) => {
    const anchor = event.target && event.target.closest && event.target.closest('a[href]');
    if (!anchor) return;
    const href = String(anchor.getAttribute('href') || '').trim();
    if (href.startsWith('#')) return;
    event.preventDefault();
    const external = externalMarkdownUrl(href);
    if (external && onOpenExternal) onOpenExternal(external);
  };
  return (
    <div
      className={`codex-markdown conversation-markdown text-[15px] leading-7 ${className}`}
      onClick={openLink}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

export function ConversationStatusBadge({ status }) {
  const done = ['Completed', 'completed', 'done', 'end_turn'].includes(status);
  const failed = ['Failed', 'failed', 'Refused'].includes(status);
  const interrupted = ['Interrupted', 'interrupted', 'incomplete'].includes(status);
  const stopped = interrupted || status === 'LimitReached';
  const label = done
    ? '已完成'
    : failed
      ? '失败'
      : interrupted
        ? '已中断'
        : status === 'LimitReached'
          ? '达到限制'
          : '处理中';
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
  now = Date.now(),
  onRequestAttention,
  className = '',
}) {
  if (!turn || turn.status !== 'running') return null;
  const waitingPermission = turn.waitingPermission
    || (turn.permissions || []).some(permission => !permission.resolved);
  const waitingInput = turn.waitingInput
    || (turn.elicitations || []).some(elicitation => !elicitation.resolved);
  const waitingAttention = waitingPermission || waitingInput;
  const label = waitingPermission
    ? '等待授权'
    : waitingInput
      ? '等待你的输入'
      : '正在处理';
  const content = (
    <>
      {waitingAttention
        ? <span className="w-2 h-2 rounded-full bg-amber-500 animate-pulse" />
        : <span className="w-3 h-3 rounded-full border-2 border-current/20 border-t-current animate-spin" />}
      <span>{label} · {formatElapsed(elapsedMs(turn.startedAt, null, now))}</span>
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
        aria-label={`${label}，前往最新消息`}
        className={`${sharedClass} hover:text-amber-700 dark:hover:text-amber-200`}>
        {content}
      </button>
    );
  }
  return <div role="status" aria-live="polite" className={sharedClass}>{content}</div>;
}

function TerminalBlock({ label, text }) {
  if (!text) return null;
  return (
    <div className="mt-3">
      <div className="mb-1.5 text-[10px] font-medium uppercase tracking-wider text-gray-400">{label}</div>
      <pre className="max-h-80 overflow-auto whitespace-pre rounded-xl bg-[#F4F5F7] dark:bg-black/30 px-3 py-2.5 text-[12px] leading-5 font-mono text-gray-700 dark:text-gray-200">{text}</pre>
    </div>
  );
}

function StructuredValue({ label, value }) {
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

function CompactItemRow({ icon, title, meta, status, open, onToggle }) {
  const tone = status === 'failed'
    ? 'text-red-500 bg-red-500/10'
    : status === 'warning'
      ? 'text-amber-500 bg-amber-500/10'
    : status === 'running'
      ? 'text-blue-500 bg-blue-500/10'
      : 'text-gray-500 bg-black/[0.04] dark:bg-white/[0.06]';
  return (
    <button type="button" onClick={onToggle}
      className="w-full min-h-10 px-2.5 py-2 flex items-center gap-2.5 text-left rounded-xl hover:bg-black/[0.025] dark:hover:bg-white/[0.035]">
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

function CommandExecutionItem({ item, now }) {
  const details = commandExecutionDetails(item.tool);
  const state = terminalStatus(item.status, details.exitCode);
  const [open, setOpen] = useState(false);
  const countHint = details.commandCount > 1 ? ` · ${details.commandCount} 段` : '';
  const duration = formatElapsed(elapsedMs(item.startedAt, item.completedAt, now));
  const outcome = state === 'running'
    ? `执行中 · ${duration}`
    : state === 'failed'
      ? `执行失败${details.exitCode == null ? '' : ` · exit ${details.exitCode}`}`
      : `执行结束${details.exitCode == null ? '' : ` · exit ${details.exitCode}`} · ${duration}`;
  return (
    <div className={`rounded-xl border ${state === 'failed' ? 'border-red-500/20' : 'border-black/[0.05] dark:border-white/[0.07]'} bg-white/45 dark:bg-white/[0.015]`}>
      <CompactItemRow icon={<Terminal size={13} />} title={details.summary}
        meta={`${outcome}${countHint}`} status={state} open={open} onToggle={() => setOpen(value => !value)} />
      {open && (
        <div className="px-3 pb-3 border-t border-black/[0.05] dark:border-white/[0.06]">
          <TerminalBlock label="命令" text={details.command} />
          {details.cwd && (
            <div className="mt-2 text-[10px] text-gray-400">
              工作目录 <span className="ml-1 font-mono text-gray-600 dark:text-gray-300">{details.cwd}</span>
            </div>
          )}
          <TerminalBlock label="输出" text={details.output} />
        </div>
      )}
    </div>
  );
}

function SearchToolItem({ item, now, onOpenExternal }) {
  const tool = item.tool || {};
  const details = searchToolDetails(tool);
  const state = terminalStatus(item.status);
  const [open, setOpen] = useState(false);
  const [rawOpen, setRawOpen] = useState(false);
  const duration = formatElapsed(elapsedMs(item.startedAt, item.completedAt, now));
  const query = details.query || tool.title || '网页内容';
  const toolName = String(tool.name || '').trim() || 'web_search';
  const queryLabel = query.length > 48 ? `${query.slice(0, 48)}…` : query;
  const resultLabel = details.count != null
    ? `${details.count} 条结果`
    : details.results.length
      ? `识别到 ${details.results.length} 条结果`
      : '已返回结果';
  const meta = state === 'running'
    ? `${queryLabel} · ${details.source} · 进行中 · ${duration}`
    : state === 'failed'
      ? `${queryLabel} · ${details.source} · 失败`
      : `${queryLabel} · ${details.source} · ${resultLabel}`;
  return (
    <div className={`rounded-xl border ${state === 'failed' ? 'border-red-500/20' : 'border-black/[0.05] dark:border-white/[0.07]'} bg-white/45 dark:bg-white/[0.015]`}>
      <CompactItemRow icon={<Wrench size={13} />} title={toolName}
        meta={meta} status={state} open={open} onToggle={() => setOpen(value => !value)} />
      {open && (
        <div className="px-3 pb-3 border-t border-black/[0.05] dark:border-white/[0.06]">
          {details.results.length > 0 ? (
            <div className="mt-2 divide-y divide-black/[0.05] dark:divide-white/[0.06]">
              {details.results.slice(0, 5).map((result, index) => {
                let domain = '';
                try { domain = new URL(result.url).hostname.replace(/^www\./, ''); } catch (_) {}
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
              搜索结果已交给 Agent 处理；当前压缩结果中没有可稳定提取的条目。
            </div>
          ) : null}
          {details.compacted && (
            <div className="mt-2 text-[10px] text-gray-400">为控制上下文长度，这里只展示可识别的结果摘要。</div>
          )}
          {details.rawOutput && (
            <div className="mt-2">
              <button
                type="button"
                onClick={() => setRawOpen(value => !value)}
                className="text-[10px] text-gray-400 hover:text-gray-700 dark:hover:text-gray-200"
              >
                {rawOpen ? '收起原始数据' : '查看原始数据'}
              </button>
              {rawOpen && <TerminalBlock label="原始数据" text={details.rawOutput} />}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function FetchToolItem({ item, now, onOpenExternal }) {
  const tool = item.tool || {};
  const details = fetchToolDetails(tool);
  const state = terminalStatus(item.status);
  const responseWarning = details.status != null && details.status >= 400;
  const visualState = responseWarning && state !== 'failed' ? 'warning' : state;
  const [open, setOpen] = useState(false);
  const [rawOpen, setRawOpen] = useState(false);
  const duration = formatElapsed(elapsedMs(item.startedAt, item.completedAt, now));
  const toolName = String(tool.name || '').trim() || 'fetch_url';
  const statusLabel = details.status != null
    ? `HTTP ${details.status}`
    : state === 'failed'
      ? '请求失败'
      : '已返回';
  const meta = state === 'running'
    ? `${details.target} · 进行中 · ${duration}`
    : `${details.target} · ${statusLabel} · ${details.contentTypeLabel}${details.truncated ? ' · 内容已截断' : ''}`;
  return (
    <div className={`rounded-xl border ${
      state === 'failed'
        ? 'border-red-500/20'
        : responseWarning
          ? 'border-amber-500/25'
          : 'border-black/[0.05] dark:border-white/[0.07]'
    } bg-white/45 dark:bg-white/[0.015]`}>
      <CompactItemRow icon={<Wrench size={13} />} title={toolName}
        meta={meta} status={visualState} open={open} onToggle={() => setOpen(value => !value)} />
      {open && (
        <div className="px-3 pb-3 border-t border-black/[0.05] dark:border-white/[0.06]">
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
              <div className="mb-1 text-[10px] font-medium text-gray-400">内容预览</div>
              <div className="max-h-24 overflow-hidden rounded-lg bg-black/[0.025] dark:bg-white/[0.035] px-3 py-2 text-[11px] leading-5 text-gray-600 dark:text-gray-300">
                {details.preview}{details.contentLength > details.preview.length ? '…' : ''}
              </div>
            </div>
          )}
          {details.truncated && (
            <div className="mt-2 text-[10px] text-gray-400">响应内容超过本次抓取上限，Agent 使用的是截断后的内容。</div>
          )}
          {details.rawOutput && (
            <div className="mt-2">
              <button
                type="button"
                onClick={() => setRawOpen(value => !value)}
                className="text-[10px] text-gray-400 hover:text-gray-700 dark:hover:text-gray-200"
              >
                {rawOpen ? '收起原始数据' : '查看原始数据'}
              </button>
              {rawOpen && <TerminalBlock label="原始数据" text={details.rawOutput} />}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function GenericToolItem({ item, now }) {
  const tool = item.tool || {};
  const state = terminalStatus(item.status);
  const [open, setOpen] = useState(false);
  const duration = formatElapsed(elapsedMs(item.startedAt, item.completedAt, now));
  const label = item.type === 'file_change' ? '文件变更' : (tool.kind || '工具');
  return (
    <div className="rounded-xl border border-black/[0.05] dark:border-white/[0.07] bg-white/45 dark:bg-white/[0.015]">
      <CompactItemRow icon={<Wrench size={13} />} title={tool.title || label}
        meta={`${label} · ${state === 'running' ? `进行中 · ${duration}` : state === 'failed' ? '失败' : `已结束 · ${duration}`}`}
        status={state} open={open} onToggle={() => setOpen(value => !value)} />
      {open && (
        <div className="px-3 pb-3 border-t border-black/[0.05] dark:border-white/[0.06]">
          {tool.locations && tool.locations.length > 0 && (
            <div className="mt-3 flex flex-wrap gap-1.5">
              {tool.locations.map((location, index) => (
                <span key={index} className="px-2 py-1 rounded-lg bg-blue-500/8 text-[10px] text-blue-600 dark:text-blue-300 font-mono">
                  {location.path || String(location)}
                </span>
              ))}
            </div>
          )}
          <StructuredValue label="参数" value={tool.rawInput} />
          <StructuredValue label="结果" value={tool.rawOutput != null ? tool.rawOutput : tool.content} />
        </div>
      )}
    </div>
  );
}

function ToolGroup({ group, now, renderToolItem, onOpenExternal }) {
  const items = group.items || [];
  const running = items.some(item => terminalStatus(item.status) === 'running');
  const failedCount = items.filter(item => terminalStatus(
    item.status,
    item.type === 'command_execution' ? commandExecutionDetails(item.tool).exitCode : null,
  ) === 'failed').length;
  const failed = failedCount > 0;
  const runningItem = [...items].reverse().find(item => terminalStatus(item.status) === 'running');
  const runningLabel = runningItem
    ? (runningItem.tool && (runningItem.tool.name || runningItem.tool.title))
      || (runningItem.type === 'file_change' ? '文件变更' : '')
    : '';
  const summary = `${running ? `正在执行${runningLabel ? ` · ${runningLabel}` : ''}` : '执行步骤'} · ${items.length} 项${
    failedCount ? ` · ${failedCount} 项失败` : ''
  }`;
  const [open, setOpen] = useState(false);
  return (
    <div>
      <button type="button" onClick={() => setOpen(value => !value)}
        className="w-full h-9 px-1 flex items-center gap-2 text-left text-[12px] text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200">
        <span className={`w-1.5 h-1.5 rounded-full ${failed ? 'bg-red-500' : running ? 'bg-blue-500 animate-pulse' : 'bg-gray-300 dark:bg-gray-600'}`} />
        <span>{summary}</span>
        <ChevronDown size={13} className={`ml-auto transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>
      {open && (
        <div className="ml-3 pl-3 border-l border-black/[0.06] dark:border-white/[0.08] space-y-1.5 pb-1">
          {items.map(item => {
            const custom = renderToolItem && renderToolItem(item);
            if (custom !== undefined) return <React.Fragment key={item.id}>{custom}</React.Fragment>;
            return item.type === 'command_execution'
              ? <CommandExecutionItem key={item.id} item={item} now={now} />
              : isSearchTool(item.tool)
                ? <SearchToolItem key={item.id} item={item} now={now} onOpenExternal={onOpenExternal} />
                : isFetchTool(item.tool)
                  ? <FetchToolItem key={item.id} item={item} now={now} onOpenExternal={onOpenExternal} />
                : <GenericToolItem key={item.id} item={item} now={now} />;
          })}
        </div>
      )}
    </div>
  );
}

function ReasoningItem({ item, now }) {
  const running = item.status === 'in_progress';
  const [open, setOpen] = useState(false);
  const duration = formatElapsed(elapsedMs(item.startedAt, item.completedAt, now));
  return (
    <div>
      <button type="button" onClick={() => setOpen(value => !value)}
        className="w-full h-9 px-1 flex items-center gap-2 text-left text-[12px] text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200">
        <span className={`w-1.5 h-1.5 rounded-full bg-violet-500 ${running ? 'animate-pulse' : ''}`} />
        <span>{running ? '思考中' : '思考完成'} · {duration}</span>
        <ChevronDown size={13} className={`ml-auto transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>
      {open && item.text && (
        <div className="ml-3 pl-3 py-1 border-l border-violet-500/15 text-[12px] leading-6 text-gray-500 dark:text-gray-300 whitespace-pre-wrap">
          {item.text}
        </div>
      )}
    </div>
  );
}

function PlanBlock({ plan }) {
  const entries = plan && plan.entries || [];
  if (!entries.length) return null;
  return (
    <div className="rounded-2xl border border-violet-500/15 bg-violet-500/[0.04] p-3.5">
      <div className="text-[12px] font-semibold text-violet-600 dark:text-violet-300 mb-2">执行计划</div>
      <div className="space-y-2">
        {entries.map((entry, index) => (
          <div key={index} className="flex items-start gap-2 text-[13px]">
            <span className={`mt-1.5 w-2 h-2 shrink-0 rounded-full ${
              entry.status === 'completed' ? 'bg-emerald-500' : entry.status === 'in_progress' ? 'bg-blue-500 animate-pulse' : 'bg-gray-300 dark:bg-gray-600'
            }`} />
            <span className="flex-1">{entry.content}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function PermissionCard({ permission, pending, onRespond, responding, agentLabel }) {
  const request = permission.request || {};
  const tool = request.toolCall || {};
  const options = request.options || [];
  const actionable = !!pending && !permission.resolved;
  return (
    <div className="rounded-2xl border border-amber-500/25 bg-amber-500/[0.06] p-4">
      <div className="flex items-start gap-3">
        <AlertTriangle size={18} className="text-amber-500 mt-0.5 shrink-0" />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold">{agentLabel} 请求权限</div>
          <div className="mt-1 text-[12px] text-gray-500 dark:text-gray-400">{tool.title || '执行受保护操作'}</div>
          {tool.rawInput && tool.rawInput.command
            ? <TerminalBlock label="命令" text={String(tool.rawInput.command)} />
            : <StructuredValue label="操作参数" value={tool.rawInput} />}
          <div className="mt-3 flex flex-wrap gap-2">
            {options.map(option => (
              <button key={option.optionId} disabled={!actionable || responding}
                onClick={() => onRespond(permission.toolCallId, option.optionId)}
                className={`px-3 py-1.5 rounded-xl text-[12px] font-medium transition-colors ${
                  String(option.kind || '').startsWith('allow')
                    ? 'bg-blue-600 text-white hover:bg-blue-700'
                    : 'bg-black/[0.06] dark:bg-white/10 hover:bg-black/10 dark:hover:bg-white/15'
                } disabled:opacity-45 disabled:cursor-not-allowed`}>
                {option.optionId === 'allow_once'
                  ? '允许一次'
                  : option.optionId === 'allow_always'
                    ? '本会话允许'
                    : option.optionId === 'reject_once'
                      ? '拒绝'
                      : option.name}
              </button>
            ))}
          </div>
          {!actionable && <div className="mt-2 text-[11px] text-gray-400">{permission.resolved ? '已处理' : '该请求已过期'}</div>}
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
  agentLabel,
}) {
  if (item.type === 'reasoning') return <ReasoningItem item={item} now={now} />;
  if (item.type === 'tool_group') {
    return (
      <ToolGroup
        group={item}
        now={now}
        renderToolItem={renderToolItem}
        onOpenExternal={onOpenExternal}
      />
    );
  }
  if (item.type === 'plan') return <PlanBlock plan={item.plan} />;
  if (item.type === 'permission') {
    return (
      <PermissionCard
        permission={item.permission}
        pending={pendingByTool[item.permission.toolCallId]}
        onRespond={onRespond}
        responding={responding}
        agentLabel={agentLabel}
      />
    );
  }
  if (item.type === 'agent_message') {
    const commentary = item.phase === 'commentary';
    return commentary
      ? <ConversationMarkdown text={item.text} onOpenExternal={onOpenExternal}
          className="text-[13px] leading-6 text-gray-500 dark:text-gray-400" />
      : <ConversationMarkdown text={item.text} onOpenExternal={onOpenExternal} />;
  }
  return null;
}

export function ConversationTurn({
  turn,
  now,
  pendingByTool = {},
  onRespond = () => {},
  responding = false,
  renderUser,
  renderItem,
  renderToolItem,
  onOpenExternal,
  agentLabel = 'Agent',
  assistantAvatar,
}) {
  const waitingPermission = turn.waitingPermission
    || (turn.permissions || []).some(permission => !permission.resolved);
  const waitingInput = turn.waitingInput
    || (turn.elicitations || []).some(elicitation => !elicitation.resolved);
  const waitingAttention = waitingPermission || waitingInput;
  const running = turn.status === 'running';
  const duration = formatElapsed(elapsedMs(turn.startedAt, turn.completedAt, now));
  const showTerminalDuration = Boolean(turn.startedAt && turn.completedAt);
  const presentation = turn.presentation || turn.items || [];
  const operationCount = Number(turn.operationCount || 0);
  const failedOperationCount = Number(turn.failedOperationCount || 0);
  const turnUsage = turn.usage || null;
  const usageLabel = turnUsage && ('inputTokens' in turnUsage || 'outputTokens' in turnUsage)
    ? `输入 ${Number(turnUsage.inputTokens || 0).toLocaleString()} · 输出 ${Number(turnUsage.outputTokens || 0).toLocaleString()}`
    : turnUsage && turnUsage.size
      ? `上下文 ${Number(turnUsage.used || 0).toLocaleString()} / ${Number(turnUsage.size || 0).toLocaleString()}`
      : '';
  const userAttachments = Array.isArray(turn.userAttachments) ? turn.userAttachments : [];
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
                      title={attachment.name}
                    >
                      <span>📎</span>
                      <span className="truncate">{attachment.name || '附件'}</span>
                    </span>
                  ))}
                </div>
              )}
            </div>
          </div>
        )
      : null;

  return (
    <section className="space-y-4" data-conversation-turn={turn.id}>
      {userContent}
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
              {waitingPermission ? '等待授权' : waitingInput ? '等待输入' : (turn.activityLabel || '正在处理')} · {duration}
            </div>
          )}
          {presentation.map((item, index) => {
            const context = { turn, now, pendingByTool, onRespond, responding };
            const custom = renderItem && renderItem(item, context);
            if (custom !== undefined) {
              return <React.Fragment key={item.id || `${item.type}-${index}`}>{custom}</React.Fragment>;
            }
            return (
              <DefaultItem
                key={item.id || `${item.type}-${index}`}
                item={item}
                now={now}
                pendingByTool={pendingByTool}
                onRespond={onRespond}
                responding={responding}
                renderToolItem={renderToolItem}
                onOpenExternal={onOpenExternal}
                agentLabel={agentLabel}
              />
            );
          })}
          {(turn.lifecycleKnown || turn.completedAt || turn.error) && !running && (
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1 pt-2">
              <ConversationStatusBadge status={turn.status} />
              {showTerminalDuration && <span className="text-[11px] text-gray-400">{duration}</span>}
              {operationCount > 0 && (
                <span className="text-[11px] text-gray-400">
                  执行 {operationCount} 项{failedOperationCount ? ` · ${failedOperationCount} 项失败` : ''}
                </span>
              )}
              {usageLabel && <span className="text-[11px] text-gray-400">{usageLabel}</span>}
              {turn.error && <span className="text-[11px] text-red-500">{turn.error}</span>}
            </div>
          )}
        </div>
      </div>
    </section>
  );
}

export function ConversationTimeline({ turns = [], ...props }) {
  return (
    <>
      {turns.map(turn => <ConversationTurn key={turn.id} turn={turn} {...props} />)}
    </>
  );
}
