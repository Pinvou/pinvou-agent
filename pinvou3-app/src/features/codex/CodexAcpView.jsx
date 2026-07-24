import React, { useEffect, useMemo, useRef, useState } from 'react';
import DOMPurify from 'dompurify';
import { marked } from 'marked';
import {
  AlertTriangle, CheckCircle2, ChevronDown, FolderOpen, Plus, Send, Sparkles,
  StopCircle, Terminal, Trash2, Wrench,
} from '../../components/icons.jsx';
import {
  appendAcpEvent,
  commandExecutionDetails,
  projectAcpTimeline,
  resolveAcpSessionControls,
} from './acp-state.js';
import { ConversationTurn } from '../conversation/ConversationTimeline.jsx';
import { QuestionChoiceCard } from '../conversation/QuestionChoiceCard.jsx';
import {
  invokeTauri,
  listenTauri,
  openTauriDialog,
} from '../../platform/tauri/client.js';

const invoke = invokeTauri;
const RECENT_WORKSPACES_KEY = 'pinvou_codex_recent_workspaces';
const UNIFIED_CONVERSATION_UI_KEY = 'pinvou_conversation_ui_v2';

function unifiedConversationUiEnabled() {
  try {
    return localStorage.getItem(UNIFIED_CONVERSATION_UI_KEY) !== 'false';
  } catch {
    return true;
  }
}

function workspaceName(path) {
  const normalized = String(path || '').replace(/[\\/]+$/, '');
  if (!normalized) return '未知目录';
  return normalized.split(/[\\/]/).filter(Boolean).pop() || normalized;
}

function loadRecentWorkspaces() {
  try {
    const value = JSON.parse(localStorage.getItem(RECENT_WORKSPACES_KEY) || '[]');
    return Array.isArray(value) ? value.filter(path => typeof path === 'string').slice(0, 6) : [];
  } catch {
    return [];
  }
}

function rememberWorkspace(path) {
  const next = [path, ...loadRecentWorkspaces().filter(item => item !== path)].slice(0, 6);
  localStorage.setItem(RECENT_WORKSPACES_KEY, JSON.stringify(next));
  return next;
}

function configChoices(option) {
  const raw = option && option.options;
  if (!Array.isArray(raw)) return [];
  if (raw.every(item => item && Array.isArray(item.options))) {
    return raw.flatMap(group => group.options || []);
  }
  return raw;
}

function configLabel(option) {
  switch (option && option.id) {
    case 'mode': return '权限模式';
    case 'collaboration_mode': return '协作方式';
    case 'model': return '模型';
    case 'reasoning_effort': return '推理强度';
    case 'fast-mode': return '快速模式';
    default: return option && option.name || '';
  }
}

function Markdown({ text }) {
  const html = useMemo(() => DOMPurify.sanitize(marked.parse(String(text || '')), {
    USE_PROFILES: { html: true },
    FORBID_TAGS: ['script', 'style', 'iframe', 'object', 'embed'],
  }), [text]);
  return <div className="codex-markdown text-[15px] leading-7" dangerouslySetInnerHTML={{ __html: html }} />;
}

function StatusBadge({ status }) {
  const done = ['Completed', 'completed', 'end_turn'].includes(status);
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
      {done ? <CheckCircle2 size={12} /> : failed ? <AlertTriangle size={12} /> : <span className="w-1.5 h-1.5 rounded-full bg-current animate-pulse" />}
      {label}
    </span>
  );
}

function elapsedMs(start, end, now) {
  const from = Date.parse(start || '');
  const to = Date.parse(end || '') || now;
  if (!Number.isFinite(from) || !Number.isFinite(to)) return 0;
  return Math.max(0, to - from);
}

function formatElapsed(milliseconds) {
  const seconds = Math.max(0, Math.floor(milliseconds / 1000));
  if (seconds < 60) return `${seconds}秒`;
  const minutes = Math.floor(seconds / 60);
  const remaining = seconds % 60;
  return remaining ? `${minutes}分${remaining}秒` : `${minutes}分`;
}

function terminalStatus(status, exitCode = null) {
  const normalized = String(status || '').toLowerCase();
  if (normalized === 'failed' || (exitCode != null && exitCode !== 0)) return 'failed';
  if (['completed', 'cancelled', 'canceled'].includes(normalized)) return 'completed';
  return 'running';
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
  const label = item.type === 'file_change' ? '文件变更' : (tool.kind || 'Codex 工具');
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

function ToolGroup({ group, now }) {
  const items = group.items || [];
  const running = items.some(item => terminalStatus(item.status) === 'running');
  const failed = items.some(item => terminalStatus(
    item.status,
    item.type === 'command_execution' ? commandExecutionDetails(item.tool).exitCode : null,
  ) === 'failed');
  const [open, setOpen] = useState(false);
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
          {items.map(item => item.type === 'command_execution'
            ? <CommandExecutionItem key={item.id} item={item} now={now} />
            : <GenericToolItem key={item.id} item={item} now={now} />)}
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
      {open && <div className="ml-3 pl-3 py-1 border-l border-violet-500/15 text-[12px] leading-6 text-gray-500 dark:text-gray-300 whitespace-pre-wrap">{item.text}</div>}
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

function PermissionCard({ permission, pending, onRespond, responding }) {
  const request = permission.request || {};
  const tool = request.toolCall || {};
  const options = request.options || [];
  const actionable = !!pending && !permission.resolved;
  return (
    <div className="rounded-2xl border border-amber-500/25 bg-amber-500/[0.06] p-4">
      <div className="flex items-start gap-3">
        <AlertTriangle size={18} className="text-amber-500 mt-0.5 shrink-0" />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold">Codex 请求权限</div>
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

function ElicitationCard({ elicitation, pending, onRespond, responding }) {
  const request = elicitation.request || {};
  const schema = request.requestedSchema || {};
  const required = new Set(Array.isArray(schema.required) ? schema.required : []);
  const fields = Object.entries(schema.properties || {});
  const otherFields = new Map(fields
    .filter(([, field]) => field && field._meta && field._meta.codex && field._meta.codex.isOtherAnswer)
    .map(([id, field]) => [String(field._meta.codex.questionId || ''), { id, field }]));
  const questions = fields.filter(([, field]) => (
    !(field && field._meta && field._meta.codex && field._meta.codex.isOtherAnswer)
  ));
  const actionable = !!pending && !elicitation.resolved;

  function choices(field) {
    if (Array.isArray(field && field.oneOf)) {
      return field.oneOf.map(option => ({
        value: option && option.const,
        label: option && (option.title || option.const),
        description: option && option.description,
      })).filter(option => option.value != null);
    }
    if (Array.isArray(field && field.enum)) {
      return field.enum.map(value => ({ value, label: String(value), description: '' }));
    }
    return [];
  }

  const normalizedQuestions = questions.map(([id, field]) => {
    const other = otherFields.get(id);
    return {
      id,
      answerKey: id,
      otherAnswerKey: other && other.id,
      header: field.title || id,
      question: field.description || '',
      options: choices(field),
      allowOther: Boolean(other),
      otherPlaceholder: other && (other.field.title || 'Other'),
      required: required.has(id)
        || Boolean(field && field._meta && field._meta.codex && field._meta.codex.isOther),
      inputType: field.type || 'string',
      secret: Boolean(field && field._meta && field._meta.codex && field._meta.codex.isSecret),
    };
  });

  function submit(groups) {
    const content = {};
    for (const group of groups) {
      const custom = group.answers.find(answer => answer.other);
      if (custom && group.otherAnswerKey) {
        content[group.otherAnswerKey] = custom.value;
      } else if (group.multiSelect) {
        content[group.answerKey] = group.answers.map(answer => answer.value);
      } else if (group.answers[0]) {
        content[group.answerKey] = group.answers[0].value;
      }
    }
    onRespond(elicitation.elicitationId, 'accept', content);
  }

  return (
    <QuestionChoiceCard
      title="Codex 需要你的选择"
      description={request.message && request.message !== 'Input requested' ? request.message : ''}
      questions={normalizedQuestions}
      resolved={!actionable}
      submitting={responding}
      statusText={!actionable
        ? elicitation.resolved
          ? (elicitation.action === 'accept' ? '已提交' : '已取消')
          : '该输入请求已过期'
        : ''}
      onSubmit={submit}
      onCancel={actionable
        ? () => onRespond(elicitation.elicitationId, 'cancel', {})
        : undefined}
    />
  );
}

function TurnItem({
  item,
  now,
  pendingByTool,
  pendingByElicitation,
  onRespond,
  onRespondElicitation,
  responding,
}) {
  if (item.type === 'reasoning') return <ReasoningItem item={item} now={now} />;
  if (item.type === 'tool_group') return <ToolGroup group={item} now={now} />;
  if (item.type === 'plan') return <PlanBlock plan={item.plan} />;
  if (item.type === 'permission') {
    return (
      <PermissionCard permission={item.permission}
        pending={pendingByTool[item.permission.toolCallId]}
        onRespond={onRespond} responding={responding} />
    );
  }
  if (item.type === 'elicitation') {
    return (
      <ElicitationCard elicitation={item.elicitation}
        pending={pendingByElicitation[item.elicitation.elicitationId]}
        onRespond={onRespondElicitation}
        responding={responding} />
    );
  }
  if (item.type === 'agent_message') {
    const commentary = item.phase === 'commentary';
    return commentary
      ? <div className="text-[13px] leading-6 text-gray-500 dark:text-gray-400"><Markdown text={item.text} /></div>
      : <Markdown text={item.text} />;
  }
  return null;
}

function Turn({
  turn,
  now,
  pendingByTool,
  pendingByElicitation,
  onRespond,
  onRespondElicitation,
  responding,
}) {
  const waitingPermission = turn.permissions.some(permission => !permission.resolved);
  const waitingInput = turn.elicitations.some(elicitation => !elicitation.resolved);
  const running = turn.status === 'running';
  const duration = formatElapsed(elapsedMs(turn.startedAt, turn.completedAt, now));
  return (
    <section className="space-y-4">
      {turn.userText && (
        <div className="flex justify-end">
          <div className="max-w-[78%] rounded-[20px] rounded-br-md bg-[#E9EEF6] dark:bg-[#2A2B2E] px-4 py-3 text-[14px] leading-6 whitespace-pre-wrap break-words">
            {turn.userText}
          </div>
        </div>
      )}
      <div className="flex items-start gap-3">
        <div className="mt-1 w-7 h-7 rounded-xl bg-gradient-to-br from-[#34A853] to-[#168C46] text-white flex items-center justify-center shrink-0 shadow-sm">
          <Sparkles size={15} />
        </div>
        <div className="min-w-0 flex-1 space-y-1">
          {running && (
            <div className={`h-9 flex items-center gap-2 text-[12px] ${waitingPermission || waitingInput ? 'text-amber-600 dark:text-amber-300' : 'text-gray-500 dark:text-gray-400'}`}>
              <span className={`w-1.5 h-1.5 rounded-full ${waitingPermission || waitingInput ? 'bg-amber-500' : 'bg-emerald-500 animate-pulse'}`} />
              {waitingPermission ? '等待授权' : waitingInput ? '等待输入' : '正在处理'} · {duration}
            </div>
          )}
          {turn.presentation.map((item, index) => (
            <TurnItem key={item.id || `${item.type}-${index}`} item={item} now={now}
              pendingByTool={pendingByTool} pendingByElicitation={pendingByElicitation}
              onRespond={onRespond} onRespondElicitation={onRespondElicitation}
              responding={responding} />
          ))}
          {(turn.completedAt || turn.error) && (
            <div className="flex items-center gap-2 pt-2">
              <StatusBadge status={turn.status} />
              <span className="text-[11px] text-gray-400">{duration}</span>
              {turn.usage && <span className="text-[11px] text-gray-400">上下文 {Number(turn.usage.used || 0).toLocaleString()} / {Number(turn.usage.size || 0).toLocaleString()}</span>}
              {turn.error && <span className="text-[11px] text-red-500">{turn.error}</span>}
            </div>
          )}
        </div>
      </div>
    </section>
  );
}

function RuntimeNotice({ status, working, error, onPrepare, onLogin, onOpenLogin }) {
  if (!status) return <div className="text-[13px] text-gray-400">正在检查 Codex ACP…</div>;
  if (!status.bridge_ready) {
    return (
      <div className="rounded-2xl border border-red-500/20 bg-red-500/[0.05] p-4 flex items-start gap-3">
        <AlertTriangle size={19} className="text-red-500 shrink-0 mt-0.5" />
        <div>
          <div className="text-[13px] font-semibold">Codex ACP Bridge 不可用</div>
          <div className="mt-1 text-[12px] text-gray-500">请修复或重新安装 Pinvou。开发环境可运行 prepare-codex-bridge-runtime.sh。</div>
          {(error || status.error) && <div className="mt-2 text-[11px] text-red-500">{error || status.error}</div>}
        </div>
      </div>
    );
  }
  if (!status.codex_available) {
    const progress = status.download_progress;
    return (
      <div className="rounded-2xl border border-blue-500/20 bg-blue-500/[0.05] p-4 flex items-center gap-3">
        <Terminal size={19} className="text-blue-500 shrink-0" />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold">未检测到系统 Codex</div>
          <div className="text-[12px] text-gray-500">可下载 Pinvou 托管 Codex {status.managed_codex_version}，不修改系统环境</div>
          {(error || status.error) && <div className="mt-1 text-[11px] text-red-500">{error || status.error}</div>}
        </div>
        <button onClick={onPrepare} disabled={working} className="px-3 py-1.5 rounded-xl bg-blue-600 text-white text-[12px] font-medium disabled:opacity-50">
          {working ? (progress == null ? '正在下载…' : `下载 ${progress}%`) : '下载托管 Codex'}
        </button>
      </div>
    );
  }
  if (!status.authenticated) {
    const waitingForLogin = Boolean(status.login_in_progress);
    const loginUrlReady = waitingForLogin && Boolean(status.login_url);
    return (
      <div className="rounded-2xl border border-amber-500/20 bg-amber-500/[0.06] p-4 flex items-center gap-3">
        <Sparkles size={19} className="text-amber-500 shrink-0" />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold">{waitingForLogin ? '等待 Codex 授权' : 'Codex 尚未登录'}</div>
          <div className="text-[12px] text-gray-500">
            {loginUrlReady
              ? '请在浏览器中完成 ChatGPT 授权；完成后 Pinvou 会自动连接'
              : waitingForLogin
                ? '正在启动 Codex 授权页面，请稍候…'
                : '使用 Codex CLI / ChatGPT 账号完成授权'}
          </div>
          {(error || status.error) && <div className="mt-1 text-[11px] text-red-500">{error || status.error}</div>}
        </div>
        {loginUrlReady && (
          <button onClick={onOpenLogin} className="px-3 py-1.5 rounded-xl border border-amber-500/30 text-amber-700 dark:text-amber-300 text-[12px] font-medium">
            重新打开授权页
          </button>
        )}
        <button onClick={onLogin} disabled={working || waitingForLogin} className="px-3 py-1.5 rounded-xl bg-amber-500 text-white text-[12px] font-medium disabled:opacity-50">
          {working || waitingForLogin ? '等待授权…' : '授权登录'}
        </button>
      </div>
    );
  }
  if (error || status.error) return <div className="rounded-xl bg-red-500/8 text-red-600 dark:text-red-300 px-3 py-2 text-[12px]">{error || status.error}</div>;
  return null;
}

function runtimeSourceLabel(status) {
  if (!status) return '';
  if (status.runtime_source === 'system') return '系统 Codex';
  if (status.runtime_source === 'managed') return '托管 Codex';
  if (status.runtime_source === 'override') return '自定义 Codex';
  if (status.runtime_source === 'legacy_bundled') return '内置 Codex';
  return '';
}

export function CodexAcpView({ theme }) {
  const [status, setStatus] = useState(null);
  const [sessions, setSessions] = useState([]);
  const [activeId, setActiveId] = useState(() => localStorage.getItem('pinvou_codex_active_session') || null);
  const [events, setEvents] = useState([]);
  const [pending, setPending] = useState([]);
  const [pendingElicitations, setPendingElicitations] = useState([]);
  const [sessionInfo, setSessionInfo] = useState(null);
  const [draft, setDraft] = useState('');
  const [now, setNow] = useState(Date.now());
  const useUnifiedConversationUi = unifiedConversationUiEnabled();
  const [configApplying, setConfigApplying] = useState('');
  const [working, setWorking] = useState(false);
  const [error, setError] = useState('');
  const [responding, setResponding] = useState(false);
  const [commandOpen, setCommandOpen] = useState(false);
  const [createMenuOpen, setCreateMenuOpen] = useState(false);
  const [recentWorkspaces, setRecentWorkspaces] = useState(loadRecentWorkspaces);
  const scroller = useRef(null);
  const projection = useMemo(() => projectAcpTimeline(events), [events]);
  const controls = useMemo(() => resolveAcpSessionControls(sessionInfo), [sessionInfo]);
  const availableCommands = useMemo(() => {
    const event = [...projection.global].reverse().find(item => item.event && item.event.type === 'available_commands');
    const data = event && event.event && event.event.data || {};
    const update = data.update || data;
    return Array.isArray(update.availableCommands) ? update.availableCommands : [];
  }, [projection.global]);
  const pendingByTool = useMemo(() => Object.fromEntries(pending.map(item => [item.toolCallId, item])), [pending]);
  const pendingByElicitation = useMemo(
    () => Object.fromEntries(pendingElicitations.map(item => [item.elicitationId, item])),
    [pendingElicitations],
  );
  const busy = projection.turns.some(turn => turn.status === 'running');
  const activeSession = useMemo(
    () => sessions.find(session => session.id === activeId) || null,
    [sessions, activeId],
  );

  function applySessionInfo(info) {
    setSessionInfo(info);
    return info;
  }

  async function refreshSessions() {
    const next = await invoke('list_codex_acp_sessions');
    setSessions(next || []);
    return next || [];
  }

  async function refreshStatus() {
    const next = await invoke('get_codex_acp_status');
    setStatus(next);
    return next;
  }

  async function openSession(id) {
    setActiveId(id);
    activeIdRef.current = id;
    localStorage.setItem('pinvou_codex_active_session', id);
    setError('');
    setSessionInfo(null);
    const [timeline, permissions, elicitations] = await Promise.all([
      invoke('get_codex_acp_timeline', { sessionId: id }),
      invoke('get_codex_acp_pending_permissions', { sessionId: id }),
      invoke('get_codex_acp_pending_elicitations', { sessionId: id }),
    ]);
    setEvents(timeline || []);
    setPending(permissions || []);
    setPendingElicitations(elicitations || []);
    const runtime = status || await refreshStatus();
    if (runtime.installed && runtime.node_supported) {
      invoke('get_codex_acp_session_info', { sessionId: id })
        .then(applySessionInfo)
        .catch(err => setError(String(err)));
    }
  }

  async function createSession(workspacePath = null) {
    setError('');
    setCreateMenuOpen(false);
    const metadata = await invoke('create_codex_acp_session', { workspacePath });
    if (workspacePath) setRecentWorkspaces(rememberWorkspace(workspacePath));
    await refreshSessions();
    await openSession(metadata.id);
    return metadata.id;
  }

  async function chooseProjectSession() {
    const selected = await openTauriDialog({
      directory: true,
      multiple: false,
      title: '选择 Codex 项目目录',
    });
    const path = Array.isArray(selected) ? selected[0] : selected;
    if (path) await createSession(path);
  }

  useEffect(() => {
    let unlisten = null;
    Promise.all([refreshStatus(), refreshSessions()]).then(([, list]) => {
      const initial = activeId && list.some(item => item.id === activeId) ? activeId : (list[0] && list[0].id);
      if (initial) openSession(initial).catch(err => setError(String(err)));
    }).catch(err => setError(String(err)));
    listenTauri('acp:event', message => {
      const incoming = message.payload;
      setEvents(current => incoming && incoming.sessionId === activeIdRef.current ? appendAcpEvent(current, incoming) : current);
      if (incoming && incoming.sessionId === activeIdRef.current) {
        const type = incoming.event && incoming.event.type;
        const data = incoming.event && incoming.event.data || {};
        if (type === 'permission_requested') {
          setPending(current => [...current.filter(item => item.toolCallId !== data.toolCallId), {
            sessionId: incoming.sessionId, toolCallId: data.toolCallId, request: data.request,
          }]);
        } else if (type === 'elicitation_requested') {
          setPendingElicitations(current => [
            ...current.filter(item => item.elicitationId !== data.elicitationId),
            {
              sessionId: incoming.sessionId,
              elicitationId: data.elicitationId,
              request: data.request,
            },
          ]);
        } else if (type === 'elicitation_resolved') {
          setPendingElicitations(current => current.filter(
            item => item.elicitationId !== data.elicitationId,
          ));
        } else if (type === 'permission_resolved' || type === 'turn_completed') {
          if (type === 'permission_resolved') setPending(current => current.filter(item => item.toolCallId !== data.toolCallId));
          refreshSessions().catch(() => {});
        } else if (type === 'runtime_ready') {
          invoke('get_codex_acp_session_info', { sessionId: incoming.sessionId }).then(applySessionInfo).catch(() => {});
        }
      }
    }).then(fn => { unlisten = fn; });
    return () => { if (unlisten) unlisten(); };
  }, []);

  useEffect(() => {
    if (!status || !status.login_in_progress) return undefined;
    const timer = window.setInterval(() => refreshStatus().catch(() => {}), 750);
    return () => window.clearInterval(timer);
  }, [status && status.login_in_progress]);

  const activeIdRef = useRef(activeId);
  activeIdRef.current = activeId;

  useEffect(() => {
    setNow(Date.now());
    if (!busy) return undefined;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [busy]);

  useEffect(() => {
    if (!scroller.current) return;
    scroller.current.scrollTop = scroller.current.scrollHeight;
  }, [events.length, projection.turns.length]);

  async function prepare() {
    setWorking(true); setError('');
    const poll = window.setInterval(() => refreshStatus().catch(() => {}), 500);
    try { setStatus(await invoke('prepare_codex_acp')); }
    catch (err) { setError(String(err)); }
    finally { window.clearInterval(poll); await refreshStatus().catch(() => {}); setWorking(false); }
  }

  async function login() {
    setWorking(true); setError('');
    try { setStatus(await invoke('login_codex_acp')); }
    catch (err) { setError(String(err)); }
    finally { setWorking(false); }
  }

  async function openLogin() {
    setError('');
    try { await invoke('open_codex_login_url'); }
    catch (err) { setError(String(err)); }
  }

  async function send() {
    const message = draft.trim();
    if (!message || busy || working) return;
    if (!activeId) {
      setError('请先选择项目目录或创建临时会话');
      setCreateMenuOpen(true);
      return;
    }
    if (!sessionInfo) {
      setError('Codex 会话配置仍在同步，请稍候');
      return;
    }
    setWorking(true); setError('');
    try {
      setDraft('');
      await invoke('codex_acp_prompt', { sessionId: activeId, message });
    } catch (err) {
      setError(String(err));
      setDraft(message);
    } finally {
      setWorking(false);
    }
  }

  async function cancel() {
    if (!activeId) return;
    await invoke('cancel_codex_acp', { sessionId: activeId }).catch(err => setError(String(err)));
  }

  async function respond(toolCallId, optionId) {
    if (!activeId) return;
    setResponding(true); setError('');
    try {
      await invoke('respond_codex_acp_permission', { sessionId: activeId, toolCallId, optionId });
      setPending(current => current.filter(item => item.toolCallId !== toolCallId));
    } catch (err) { setError(String(err)); }
    finally { setResponding(false); }
  }

  async function respondElicitation(elicitationId, action, content) {
    if (!activeId) return;
    setResponding(true); setError('');
    try {
      await invoke('respond_codex_acp_elicitation', {
        sessionId: activeId,
        elicitationId,
        action,
        content,
      });
      setPendingElicitations(current => current.filter(
        item => item.elicitationId !== elicitationId,
      ));
    } catch (err) { setError(String(err)); }
    finally { setResponding(false); }
  }

  async function changeModel(modelId) {
    if (!activeId || !modelId) return;
    setWorking(true); setConfigApplying('model');
    try { applySessionInfo(await invoke('set_codex_acp_model', { sessionId: activeId, modelId })); }
    catch (err) { setError(String(err)); }
    finally { setWorking(false); setConfigApplying(''); }
  }

  async function changeConfig(configId, valueId) {
    if (!activeId) return;
    setWorking(true); setConfigApplying(configId); setError('');
    try {
      applySessionInfo(await invoke('set_codex_acp_config_option', {
        sessionId: activeId, configId, valueId,
      }));
    } catch (err) { setError(String(err)); }
    finally { setWorking(false); setConfigApplying(''); }
  }

  async function changeMode(modeId) {
    if (!activeId || !modeId) return;
    setWorking(true); setConfigApplying('mode'); setError('');
    try {
      applySessionInfo(await invoke('set_codex_acp_mode', { sessionId: activeId, modeId }));
    } catch (err) { setError(String(err)); }
    finally { setWorking(false); setConfigApplying(''); }
  }

  async function removeSession(event, id) {
    event.stopPropagation();
    if (!window.confirm('删除这个 Codex 会话？')) return;
    await invoke('delete_session', { id });
    const next = await refreshSessions();
    const replacement = next.find(item => item.id !== id);
    if (activeId === id) {
      setActiveId(null); setEvents([]); setPending([]); setPendingElicitations([]); setSessionInfo(null);
      localStorage.removeItem('pinvou_codex_active_session');
      if (replacement) await openSession(replacement.id);
    }
  }

  return (
    <div className={`h-full min-h-0 flex ${theme === 'dark' ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>
      <aside className={`w-[238px] shrink-0 border-r flex flex-col ${theme === 'dark' ? 'border-white/[0.07] bg-[#18191B]' : 'border-black/[0.06] bg-[#F7F9FC]'}`}>
        <div className="p-3 relative">
          <button onClick={() => setCreateMenuOpen(value => !value)}
            className="w-full h-10 rounded-xl bg-[#007AFF] hover:bg-[#006EE6] text-white flex items-center justify-center gap-2 text-[13px] font-semibold shadow-sm">
            <Plus size={16} /> 新建 Codex 会话 <ChevronDown size={13} />
          </button>
          {createMenuOpen && (
            <>
              <button aria-label="关闭新建会话菜单" className="fixed inset-0 z-30 cursor-default" onClick={() => setCreateMenuOpen(false)} />
              <div className="absolute z-40 left-3 right-3 top-[56px] rounded-2xl border border-black/[0.08] dark:border-white/10 bg-white/95 dark:bg-[#202124]/95 backdrop-blur-xl shadow-xl p-2">
                <button type="button" onClick={() => chooseProjectSession().catch(err => setError(String(err)))}
                  className="w-full rounded-xl px-3 py-2.5 flex items-center gap-3 text-left hover:bg-black/[0.04] dark:hover:bg-white/[0.06]">
                  <FolderOpen size={16} className="text-blue-500 shrink-0" />
                  <span><span className="block text-[12px] font-semibold">选择项目目录</span><span className="block text-[10px] text-gray-400 mt-0.5">Codex 直接在真实项目中工作</span></span>
                </button>
                <button type="button" onClick={() => createSession().catch(err => setError(String(err)))}
                  className="w-full rounded-xl px-3 py-2.5 flex items-center gap-3 text-left hover:bg-black/[0.04] dark:hover:bg-white/[0.06]">
                  <Sparkles size={16} className="text-emerald-500 shrink-0" />
                  <span><span className="block text-[12px] font-semibold">临时会话</span><span className="block text-[10px] text-gray-400 mt-0.5">使用 Pinvou 管理的隔离目录</span></span>
                </button>
                {recentWorkspaces.length > 0 && (
                  <div className="mt-1 pt-2 border-t border-black/[0.05] dark:border-white/[0.06]">
                    <div className="px-3 pb-1 text-[10px] uppercase tracking-wider text-gray-400">最近项目</div>
                    {recentWorkspaces.map(path => (
                      <button key={path} type="button" title={path}
                        onClick={() => createSession(path).catch(err => setError(String(err)))}
                        className="w-full rounded-lg px-3 py-1.5 flex items-center gap-2 text-left hover:bg-black/[0.04] dark:hover:bg-white/[0.06]">
                        <FolderOpen size={13} className="shrink-0 text-gray-400" />
                        <span className="truncate text-[11px]">{workspaceName(path)}</span>
                      </button>
                    ))}
                  </div>
                )}
              </div>
            </>
          )}
        </div>
        <div className="px-4 pt-2 pb-1 text-[11px] uppercase tracking-wider text-gray-400">Codex 会话</div>
        <div className="flex-1 min-h-0 overflow-y-auto px-2 pb-3">
          {sessions.map(session => (
            <button key={session.id} onClick={() => openSession(session.id).catch(err => setError(String(err)))}
              className={`group w-full min-w-0 px-3 py-2.5 rounded-xl flex items-center gap-2 text-left mb-0.5 ${
                activeId === session.id ? 'bg-blue-500/10 text-blue-600 dark:text-blue-300' : 'hover:bg-black/[0.04] dark:hover:bg-white/[0.05]'
              }`}>
              {session.workspace_kind === 'project'
                ? <FolderOpen size={14} className="shrink-0" />
                : <Sparkles size={14} className="shrink-0" />}
              <span className="min-w-0 flex-1">
                <span className="block truncate text-[12px]">{session.title || '新对话'}</span>
                <span className={`block truncate text-[10px] mt-0.5 ${session.workspace_available ? 'opacity-55' : 'text-red-500'}`}
                  title={session.workspace_path}>
                  {session.workspace_kind === 'project' ? workspaceName(session.workspace_path) : '临时工作区'}
                  {!session.workspace_available && ' · 目录不可用'}
                </span>
              </span>
              <span role="button" tabIndex={0} onClick={event => removeSession(event, session.id)}
                className="opacity-0 group-hover:opacity-60 hover:!opacity-100 p-1 rounded-md"><Trash2 size={13} /></span>
            </button>
          ))}
          {!sessions.length && <div className="px-3 py-8 text-center text-[12px] text-gray-400">还没有 Codex 会话</div>}
        </div>
        <div className="px-3 py-3 border-t border-black/[0.05] dark:border-white/[0.06] text-[10px] leading-4 text-gray-400">
          Codex 原生 system prompt、tools、skills 与 MCP<br />pinvou 不注入技能、卡牌或知识库
        </div>
      </aside>

      <main className="flex-1 min-w-0 min-h-0 flex flex-col">
        <header className="h-14 shrink-0 px-5 flex items-center gap-3 border-b border-black/[0.05] dark:border-white/[0.06]">
          <div className="w-8 h-8 rounded-xl bg-gradient-to-br from-[#34A853] to-[#168C46] text-white flex items-center justify-center"><Sparkles size={16} /></div>
          <div className="min-w-0 flex-1">
            <div className="text-[14px] font-semibold">Codex</div>
            <div className={`text-[10px] truncate ${activeSession && !activeSession.workspace_available ? 'text-red-500' : 'text-gray-400'}`}
              title={activeSession && activeSession.workspace_path}>
              {activeSession
                ? `ACP · ${activeSession.workspace_kind === 'project' ? activeSession.workspace_path : '临时工作区'}${activeSession.workspace_available ? '' : ' · 目录不可用'}`
                : 'ACP · 请选择项目目录或临时会话'}
            </div>
          </div>
          {controls.fallbackModels.length > 0 && (
            <select value={sessionInfo.current_model_id || ''} onChange={event => changeModel(event.target.value)}
              disabled={busy || working}
              className="max-w-[210px] h-8 rounded-lg px-2 bg-black/[0.04] dark:bg-white/[0.06] border border-black/[0.05] dark:border-white/[0.07] text-[11px] outline-none disabled:opacity-50">
              {controls.fallbackModels.map(model => <option key={model.id} value={model.id}>{model.name || model.id}</option>)}
            </select>
          )}
          {controls.fallbackModes && controls.fallbackModes.availableModes && (
            <select value={controls.effectiveMode || ''} onChange={event => changeMode(event.target.value)}
              disabled={busy || working}
              title="Codex Agent 上报的会话模式"
              className="h-8 rounded-lg px-2 bg-black/[0.04] dark:bg-white/[0.06] border border-black/[0.05] dark:border-white/[0.07] text-[11px] outline-none disabled:opacity-50">
              {controls.fallbackModes.availableModes.map(item => <option key={item.id} value={item.id}>{item.name || item.id}</option>)}
            </select>
          )}
          {controls.configOptions.map(option => (
            <select key={option.id} value={option.currentValue || ''} onChange={event => changeConfig(option.id, event.target.value)}
              disabled={busy || working}
              title={option.description || option.name}
              className="max-w-[170px] h-8 rounded-lg px-2 bg-black/[0.04] dark:bg-white/[0.06] border border-black/[0.05] dark:border-white/[0.07] text-[11px] outline-none disabled:opacity-50">
              {configChoices(option).map(choice => <option key={choice.value} value={choice.value}>{configLabel(option)} · {choice.name || choice.value}</option>)}
            </select>
          ))}
          {configApplying && <span className="text-[10px] text-blue-500 animate-pulse">配置应用中…</span>}
          {busy && <StatusBadge status="running" />}
        </header>

        <div ref={scroller} className="flex-1 min-h-0 overflow-y-auto custom-scrollbar">
          <div className="w-full max-w-[920px] mx-auto px-6 py-6 space-y-7">
            <RuntimeNotice status={status} working={working} error={error} onPrepare={prepare} onLogin={login} onOpenLogin={openLogin} />
            {!projection.turns.length && (
              <div className="min-h-[48vh] flex flex-col items-center justify-center text-center">
                <div className="w-14 h-14 rounded-2xl bg-gradient-to-br from-[#34A853] to-[#168C46] text-white flex items-center justify-center shadow-lg shadow-green-500/15"><Sparkles size={26} /></div>
                <div className="mt-5 text-[20px] font-semibold">{activeSession ? '用 Codex 处理代码任务' : '选择 Codex 的工作目录'}</div>
                <div className="mt-2 max-w-md text-[13px] leading-6 text-gray-500 dark:text-gray-400">
                  {activeSession
                    ? '工具调用、思考、计划和权限请求会按 ACP 原始语义展示，不进入品悟原有 DeepSeek 消息框架。'
                    : '项目会话直接在真实仓库中工作；临时会话使用 Pinvou 管理的隔离目录。一个项目可以创建多个独立会话。'}
                </div>
                {!activeSession && (
                  <button type="button" onClick={() => setCreateMenuOpen(true)}
                    className="mt-5 h-9 px-4 rounded-xl bg-blue-600 text-white text-[12px] font-semibold">
                    新建会话
                  </button>
                )}
              </div>
            )}
            {projection.turns.map(turn => useUnifiedConversationUi
              ? (
                  <ConversationTurn
                    key={turn.id}
                    turn={turn}
                    now={now}
                    pendingByTool={pendingByTool}
                    onRespond={respond}
                    responding={responding}
                    renderItem={(item) => item.type === 'elicitation'
                      ? (
                          <ElicitationCard
                            elicitation={item.elicitation}
                            pending={pendingByElicitation[item.elicitation.elicitationId]}
                            onRespond={respondElicitation}
                            responding={responding}
                          />
                        )
                      : undefined}
                    agentLabel="Codex"
                    onOpenExternal={(url) => invoke('open_external_url', { url }).catch(err => setError(String(err)))}
                  />
                )
              : (
                  <Turn key={turn.id} turn={turn} now={now}
                    pendingByTool={pendingByTool}
                    pendingByElicitation={pendingByElicitation}
                    onRespond={respond}
                    onRespondElicitation={respondElicitation}
                    responding={responding} />
                ))}
          </div>
        </div>

        <div className="shrink-0 px-6 pb-5 pt-2">
          <div className="w-full max-w-[920px] mx-auto">
            {error && <div className="mb-2 px-3 text-[11px] text-red-500 break-words">{error}</div>}
            <div className="relative rounded-[24px] border border-black/[0.08] dark:border-white/10 bg-white/85 dark:bg-[#1B1C1E]/90 backdrop-blur-xl shadow-lg px-4 pt-3 pb-2.5 focus-within:border-blue-400/50">
              {commandOpen && availableCommands.length > 0 && (
                <>
                  <button aria-label="关闭 Codex 命令菜单" className="fixed inset-0 z-30 cursor-default" onClick={() => setCommandOpen(false)} />
                  <div className="absolute z-40 left-0 right-0 bottom-full mb-2 max-h-72 overflow-y-auto rounded-2xl border border-black/[0.08] dark:border-white/10 bg-white/95 dark:bg-[#202124]/95 backdrop-blur-xl shadow-xl p-2">
                    <div className="px-2 py-1 text-[10px] uppercase tracking-wider text-gray-400">Codex Agent 命令</div>
                    {availableCommands.map(command => (
                      <button key={command.name} type="button"
                        onClick={() => { setDraft(`/${command.name}${command.input ? ' ' : ''}`); setCommandOpen(false); }}
                        className="w-full rounded-xl px-3 py-2 text-left hover:bg-black/[0.04] dark:hover:bg-white/[0.06]">
                        <span className="block text-[12px] font-semibold">/{command.name}</span>
                        <span className="block mt-0.5 text-[11px] text-gray-400">{command.description}</span>
                      </button>
                    ))}
                  </div>
                </>
              )}
              <textarea value={draft} onChange={event => setDraft(event.target.value)}
                onKeyDown={event => { if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); send(); } }}
                placeholder="让 Codex 处理代码、运行命令或解释仓库…"
                rows={1} className="w-full min-h-[48px] max-h-48 resize-none bg-transparent outline-none text-[15px] leading-6 placeholder:text-gray-400" />
              <div className="flex items-center justify-between mt-1">
                <div className="flex items-center gap-2 text-[10px] text-gray-400">
                  {availableCommands.length > 0 && (
                    <button type="button" onClick={() => setCommandOpen(value => !value)}
                      className="h-7 px-2 rounded-lg text-[11px] font-mono hover:bg-black/[0.05] dark:hover:bg-white/[0.07]"
                      title="Codex Agent 上报的命令">/</button>
                  )}
                  <span className={`w-1.5 h-1.5 rounded-full ${status && status.installed && status.authenticated ? 'bg-emerald-500' : 'bg-gray-400'}`} />
                  {status && status.installed && status.authenticated
                    ? `Codex 已连接${runtimeSourceLabel(status) ? ` · ${runtimeSourceLabel(status)}` : ''}${status.codex_version ? ` ${status.codex_version}` : ''}`
                    : 'Codex 未就绪'}
                </div>
                {busy ? (
                  <button onClick={cancel} className="w-9 h-9 rounded-full flex items-center justify-center bg-red-500/10 text-red-500 hover:bg-red-500/15"><StopCircle size={18} /></button>
                ) : (
                  <button onClick={send} disabled={!activeId || !sessionInfo || !draft.trim() || working || !status || !status.installed || !status.authenticated}
                    className="w-9 h-9 rounded-full flex items-center justify-center bg-[#007AFF] text-white shadow-sm hover:bg-[#006EE6] disabled:bg-black/[0.06] dark:disabled:bg-white/10 disabled:text-gray-400 disabled:shadow-none">
                    <Send size={16} />
                  </button>
                )}
              </div>
            </div>
          </div>
        </div>
      </main>
    </div>
  );
}
