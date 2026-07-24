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
  formatElapsed,
  terminalStatus,
} from './conversation-model.js';

function Markdown({ text, className = '' }) {
  const html = useMemo(() => DOMPurify.sanitize(marked.parse(String(text || '')), {
    USE_PROFILES: { html: true },
    FORBID_TAGS: ['script', 'style', 'iframe', 'object', 'embed'],
  }), [text]);
  return (
    <div
      className={`codex-markdown conversation-markdown text-[15px] leading-7 ${className}`}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

export function ConversationStatusBadge({ status }) {
  const done = ['Completed', 'completed', 'done', 'end_turn'].includes(status);
  const failed = ['Failed', 'failed', 'Refused'].includes(status);
  const label = done
    ? '已完成'
    : failed
      ? '失败'
      : status === 'Interrupted'
        ? '已中断'
        : status === 'LimitReached'
          ? '达到限制'
          : '处理中';
  return (
    <span className={`inline-flex items-center gap-1 text-[11px] px-2 py-0.5 rounded-full ${
      done ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-300'
        : failed ? 'bg-red-500/10 text-red-600 dark:text-red-300'
          : 'bg-blue-500/10 text-blue-600 dark:text-blue-300'
    }`}>
      {done
        ? <CheckCircle2 size={12} />
        : failed
          ? <AlertTriangle size={12} />
          : <span className="w-1.5 h-1.5 rounded-full bg-current animate-pulse" />}
      {label}
    </span>
  );
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

function ToolGroup({ group, now, renderToolItem, shouldAutoOpenToolGroup }) {
  const items = group.items || [];
  const running = items.some(item => terminalStatus(item.status) === 'running');
  const failed = items.some(item => terminalStatus(
    item.status,
    item.type === 'command_execution' ? commandExecutionDetails(item.tool).exitCode : null,
  ) === 'failed');
  const autoOpen = Boolean(shouldAutoOpenToolGroup && shouldAutoOpenToolGroup(group));
  const [open, setOpen] = useState(autoOpen);
  useEffect(() => {
    if (autoOpen) setOpen(true);
  }, [autoOpen]);
  return (
    <div>
      <button type="button" onClick={() => setOpen(value => !value)}
        className="w-full h-9 px-1 flex items-center gap-2 text-left text-[12px] text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200">
        <span className={`w-1.5 h-1.5 rounded-full ${failed ? 'bg-red-500' : running ? 'bg-blue-500 animate-pulse' : 'bg-gray-300 dark:bg-gray-600'}`} />
        <span>{running ? '正在执行' : failed ? '执行步骤包含失败' : '执行步骤'} · {items.length}</span>
        <ChevronDown size={13} className={`ml-auto transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>
      {open && (
        <div className="ml-3 pl-3 border-l border-black/[0.06] dark:border-white/[0.08] space-y-1.5 pb-1">
          {items.map(item => {
            const custom = renderToolItem && renderToolItem(item);
            if (custom !== undefined) return <React.Fragment key={item.id}>{custom}</React.Fragment>;
            return item.type === 'command_execution'
              ? <CommandExecutionItem key={item.id} item={item} now={now} />
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
  shouldAutoOpenToolGroup,
  agentLabel,
}) {
  if (item.type === 'reasoning') return <ReasoningItem item={item} now={now} />;
  if (item.type === 'tool_group') {
    return (
      <ToolGroup
        group={item}
        now={now}
        renderToolItem={renderToolItem}
        shouldAutoOpenToolGroup={shouldAutoOpenToolGroup}
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
      ? <Markdown text={item.text} className="text-[13px] leading-6 text-gray-500 dark:text-gray-400" />
      : <Markdown text={item.text} />;
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
  shouldAutoOpenToolGroup,
  agentLabel = 'Agent',
  assistantAvatar,
}) {
  const waitingPermission = turn.waitingPermission
    || (turn.permissions || []).some(permission => !permission.resolved);
  const running = turn.status === 'running';
  const duration = formatElapsed(elapsedMs(turn.startedAt, turn.completedAt, now));
  const presentation = turn.presentation || turn.items || [];
  const userContent = renderUser && turn.userItem
    ? renderUser(turn.userItem, turn)
    : turn.userText
      ? (
          <div className="flex justify-end">
            <div className="max-w-[78%] rounded-[20px] rounded-br-md bg-[#E9EEF6] dark:bg-[#2A2B2E] px-4 py-3 text-[14px] leading-6 whitespace-pre-wrap break-words">
              {turn.userText}
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
            <div className={`h-9 flex items-center gap-2 text-[12px] ${waitingPermission ? 'text-amber-600 dark:text-amber-300' : 'text-gray-500 dark:text-gray-400'}`}>
              <span className={`w-1.5 h-1.5 rounded-full ${waitingPermission ? 'bg-amber-500' : 'bg-emerald-500 animate-pulse'}`} />
              {waitingPermission ? '等待授权' : (turn.activityLabel || '正在处理')} · {duration}
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
                shouldAutoOpenToolGroup={shouldAutoOpenToolGroup}
                agentLabel={agentLabel}
              />
            );
          })}
          {(turn.completedAt || turn.error) && (
            <div className="flex items-center gap-2 pt-2">
              <ConversationStatusBadge status={turn.status} />
              {turn.startedAt && <span className="text-[11px] text-gray-400">{duration}</span>}
              {turn.usage && <span className="text-[11px] text-gray-400">上下文 {Number(turn.usage.used || 0).toLocaleString()} / {Number(turn.usage.size || 0).toLocaleString()}</span>}
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
