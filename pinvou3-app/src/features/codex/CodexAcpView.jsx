import React, { useEffect, useMemo, useRef, useState } from 'react';
import { FileTypeIcon } from '../../components/files/FileTypeIcon.jsx';
import {
  AlertTriangle, Check, CheckCircle2, ChevronDown, FileText, FolderOpen, Paperclip, Send,
  RefreshCw, Sparkles, StopCircle, Terminal, User, Wrench,
} from '../../components/icons.jsx';
import { AcpAgentLogo } from './AcpAgentLogo.jsx';
import { CodexWorkspacePanel } from './CodexWorkspacePanel.jsx';
import { ComposerPopover } from '../../components/ComposerPopover.jsx';
import {
  appendAcpEvent,
  commandExecutionDetails,
  projectAcpTimeline,
  resolveAcpSessionControls,
} from './acp-state.js';
import {
  ConversationActivityIndicator,
  ConversationMarkdown,
  ConversationTurn,
} from '../conversation/ConversationTimeline.jsx';
import { isNearConversationBottom } from '../conversation/conversation-model.js';
import { QuestionChoiceCard } from '../conversation/QuestionChoiceCard.jsx';
import { AttachmentChips } from '../attachments/AttachmentChips.jsx';
import { HomeModeSwitcher } from '../conversation/HomeModeSwitcher.jsx';
import {
  invokeTauri,
  listenTauri,
  openTauriDialog,
} from '../../platform/tauri/client.js';

const invoke = invokeTauri;
const RECENT_WORKSPACES_KEY = 'pinvou_codex_recent_workspaces';
const UNIFIED_CONVERSATION_UI_KEY = 'pinvou_conversation_ui_v2';
const DRAFT_ATTACHMENT_KEY = '__codex_draft__';
const DRAFT_CONTROLS_CACHE_KEY = 'pinvou_codex_draft_controls';

function unifiedConversationUiEnabled() {
  try {
    return localStorage.getItem(UNIFIED_CONVERSATION_UI_KEY) !== 'false';
  } catch {
    return true;
  }
}

function workspaceName(path, unknownDirectory = '未知目录') {
  const normalized = String(path || '').replace(/[\\/]+$/, '');
  if (!normalized) return unknownDirectory;
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

function forgetWorkspace(path) {
  const next = loadRecentWorkspaces().filter(item => item !== path);
  try {
    localStorage.setItem(RECENT_WORKSPACES_KEY, JSON.stringify(next));
  } catch {
    // localStorage 不可用时仍允许当前窗口继续创建新会话。
  }
  return next;
}

// 草稿态（尚未创建会话）也需要展示模型/权限模式/推理强度等选项：ACP 的配置项是会话级的，
// 这里缓存每个 agent 最近一次会话上报的配置快照，供新会话草稿预展示和预选。
function loadDraftControlsCache() {
  try {
    const value = JSON.parse(localStorage.getItem(DRAFT_CONTROLS_CACHE_KEY) || '{}');
    return value && typeof value === 'object' && !Array.isArray(value) ? value : {};
  } catch {
    return {};
  }
}

function snapshotSessionControls(info) {
  if (!info) return null;
  const snapshot = {
    models: Array.isArray(info.models) ? info.models : [],
    current_model_id: info.current_model_id || '',
    modes: info.modes || null,
    config_options: Array.isArray(info.config_options) ? info.config_options : [],
  };
  if (!snapshot.models.length && !snapshot.modes && !snapshot.config_options.length) return null;
  return snapshot;
}

function rememberDraftControls(agentId, info) {
  const snapshot = snapshotSessionControls(info);
  if (!agentId || !snapshot) return null;
  const cache = { ...loadDraftControlsCache(), [agentId]: snapshot };
  try {
    localStorage.setItem(DRAFT_CONTROLS_CACHE_KEY, JSON.stringify(cache));
  } catch {
    // 缓存写不进去时仅影响下次草稿预展示，本次会话不受影响。
  }
  return snapshot;
}

function isAcpAuthenticationFailure(envelope) {
  if (envelope?.event?.type !== 'turn_completed') return false;
  const error = String(envelope.event?.data?.error || '');
  return /authentication[_ ]failed|authentication required|failed to authenticate|oauth.{0,80}expired|not logged in/i.test(error);
}

function classifyAcpServiceFailure(envelope) {
  if (envelope?.event?.type !== 'turn_completed') return null;
  const detail = String(envelope.event?.data?.error || '').trim();
  if (!detail) return null;
  let kind = 'service';
  if (/HTTP\s*402|会员.{0,12}(权益|额度|到期|失效)|订阅.{0,12}(到期|失效)|payment required/i.test(detail)) {
    kind = 'entitlement';
  } else if (/HTTP\s*429|rate.?limit|quota|额度.{0,12}(不足|用尽)|用量.{0,12}(超出|耗尽)/i.test(detail)) {
    kind = 'quota';
  } else if (/HTTP\s*401|authentication[_ ]failed|authentication required|failed to authenticate|oauth.{0,80}expired|not logged in/i.test(detail)) {
    kind = 'authentication';
  } else if (/network|connection|timeout|timed out|网络|连接.{0,8}(失败|超时)/i.test(detail)) {
    kind = 'network';
  }
  return {
    kind,
    detail,
    key: `${envelope.seq || ''}:${envelope.timestamp || ''}:${detail}`,
  };
}

function configChoices(option) {
  const raw = option && option.options;
  if (!Array.isArray(raw)) return [];
  if (raw.every(item => item && Array.isArray(item.options))) {
    return raw.flatMap(group => group.options || []);
  }
  return raw;
}

function configLabel(option, copy) {
  const labels = copy?.configLabels || {};
  switch (option && option.id) {
    case 'mode': return labels.mode || '权限模式';
    case 'collaboration_mode': return labels.collaboration_mode || '协作方式';
    case 'model': return labels.model || '模型';
    case 'reasoning_effort': return labels.reasoning_effort || '推理强度';
    case 'fast-mode': return labels['fast-mode'] || '快速模式';
    default: return option && option.name || '';
  }
}

function CodexComposerConfigSelect({
  id,
  label,
  value,
  choices,
  onChange,
  disabled = false,
  title,
  unsetLabel = '未设置',
}) {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef(null);
  const selected = choices.find(choice => String(choice.value) === String(value));
  const selectedLabel = selected && (selected.name || selected.value) || value || unsetLabel;
  const pick = (choiceValue) => {
    setOpen(false);
    if (String(choiceValue) !== String(value)) onChange(choiceValue);
  };
  return (
    <div className="relative min-w-0" data-testid={`codex-config-${id}`}>
      <button
        ref={triggerRef}
        type="button"
        title={title || `${label}：${selectedLabel}`}
        aria-label={label}
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen(current => !current)}
        className={`inline-flex h-8 min-w-0 max-w-[220px] items-center gap-1.5 overflow-hidden rounded-xl border px-2.5 transition-all ${
          disabled
            ? 'cursor-default opacity-50'
            : 'cursor-pointer hover:-translate-y-px hover:shadow-sm focus-within:border-[#007AFF]/45 focus-within:ring-2 focus-within:ring-[#007AFF]/10'
        } border-black/[0.07] bg-black/[0.025] text-[#1F1F1F] dark:border-white/[0.09] dark:bg-white/[0.055] dark:text-[#E8EAED]`}
      >
        <span className="pointer-events-none shrink-0 text-[10px] font-medium text-gray-400 dark:text-gray-500">
          {label}
        </span>
        <span className="pointer-events-none min-w-0 truncate text-[11px] font-semibold">
          {selectedLabel}
        </span>
        <ChevronDown
          size={12}
          aria-hidden="true"
          className={`pointer-events-none ml-auto shrink-0 text-gray-400 transition-transform ${open ? 'rotate-180' : ''}`}
        />
      </button>
      <ComposerPopover
        open={open}
        onClose={() => setOpen(false)}
        triggerRef={triggerRef}
        compact={false}
        desktopClassName="absolute bottom-full left-0 mb-2 z-50 max-h-72 w-56 overflow-y-auto custom-scrollbar rounded-2xl border border-black/5 bg-white/95 p-1.5 shadow-xl backdrop-blur-xl dark:border-white/10 dark:bg-[#1E1E20]/95"
      >
        {choices.map(choice => {
          const isSelected = String(choice.value) === String(value);
          return (
            <button
              key={choice.value}
              type="button"
              onClick={() => pick(choice.value)}
              className="group w-full flex items-center justify-between gap-2.5 rounded-xl px-3 py-2.5 text-[13px] text-gray-700 transition-colors hover:bg-[#007AFF] hover:text-white dark:text-gray-200"
            >
              <span className="min-w-0 truncate">{choice.name || choice.value}</span>
              {isSelected && <Check size={15} className="shrink-0 text-[#007AFF] group-hover:text-white" />}
            </button>
          );
        })}
      </ComposerPopover>
    </div>
  );
}

function StatusBadge({ status, copy }) {
  const done = ['Completed', 'completed', 'end_turn'].includes(status);
  const failed = ['Failed', 'failed', 'Refused'].includes(status);
  const label = done
    ? copy.completed
    : failed
      ? copy.failed
      : status === 'Interrupted'
        ? copy.interrupted
        : status === 'LimitReached'
          ? copy.limitReached
          : copy.processing;
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

function CommandExecutionItem({ item, now, copy }) {
  const details = commandExecutionDetails(item.tool);
  const state = terminalStatus(item.status, details.exitCode);
  const [open, setOpen] = useState(false);
  const countHint = details.commandCount > 1 ? ` · ${copy.segments(details.commandCount)}` : '';
  const duration = copy.elapsed(elapsedMs(item.startedAt, item.completedAt, now));
  const outcome = state === 'running'
    ? `${copy.running} · ${duration}`
    : state === 'failed'
      ? `${copy.executionFailed}${details.exitCode == null ? '' : ` · exit ${details.exitCode}`}`
      : `${copy.executionFinished}${details.exitCode == null ? '' : ` · exit ${details.exitCode}`} · ${duration}`;
  return (
    <div className={`rounded-xl border ${state === 'failed' ? 'border-red-500/20' : 'border-black/[0.05] dark:border-white/[0.07]'} bg-white/45 dark:bg-white/[0.015]`}>
      <CompactItemRow icon={<Terminal size={13} />} title={details.summary}
        meta={`${outcome}${countHint}`} status={state} open={open} onToggle={() => setOpen(value => !value)} />
      {open && (
        <div className="px-3 pb-3 border-t border-black/[0.05] dark:border-white/[0.06]">
          <TerminalBlock label={copy.command} text={details.command} />
          {details.cwd && (
            <div className="mt-2 text-[10px] text-gray-400">
              {copy.workingDirectory} <span className="ml-1 font-mono text-gray-600 dark:text-gray-300">{details.cwd}</span>
            </div>
          )}
          <TerminalBlock label={copy.output} text={details.output} />
        </div>
      )}
    </div>
  );
}

function GenericToolItem({ item, now, copy, cv }) {
  const tool = item.tool || {};
  const state = terminalStatus(item.status);
  const [open, setOpen] = useState(false);
  const duration = copy.elapsed(elapsedMs(item.startedAt, item.completedAt, now));
  const label = item.type === 'file_change' ? copy.fileChange : (tool.kind || cv.codexTool);
  return (
    <div className="rounded-xl border border-black/[0.05] dark:border-white/[0.07] bg-white/45 dark:bg-white/[0.015]">
      <CompactItemRow icon={<Wrench size={13} />} title={tool.title || label}
        meta={`${label} · ${state === 'running' ? `${copy.inProgress} · ${duration}` : state === 'failed' ? copy.failed : `${cv.ended} · ${duration}`}`}
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
          <StructuredValue label={copy.arguments} value={tool.rawInput} />
          <StructuredValue label={copy.result} value={tool.rawOutput != null ? tool.rawOutput : tool.content} />
        </div>
      )}
    </div>
  );
}

function ToolGroup({ group, now, copy, cv }) {
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
        <span>{running ? copy.executing : failed ? cv.stepsFailed : copy.executionSteps} · {items.length}</span>
        <ChevronDown size={13} className={`ml-auto transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>
      {open && (
        <div className="ml-3 pl-3 border-l border-black/[0.06] dark:border-white/[0.08] space-y-1.5 pb-1">
          {items.map(item => item.type === 'command_execution'
            ? <CommandExecutionItem key={item.id} item={item} now={now} copy={copy} />
            : <GenericToolItem key={item.id} item={item} now={now} copy={copy} cv={cv} />)}
        </div>
      )}
    </div>
  );
}

function ReasoningItem({ item, now, copy }) {
  const running = item.status === 'in_progress';
  const [open, setOpen] = useState(false);
  const duration = copy.elapsed(elapsedMs(item.startedAt, item.completedAt, now));
  return (
    <div>
      <button type="button" onClick={() => setOpen(value => !value)}
        className="w-full h-9 px-1 flex items-center gap-2 text-left text-[12px] text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200">
        <span className={`w-1.5 h-1.5 rounded-full bg-violet-500 ${running ? 'animate-pulse' : ''}`} />
        <span>{running ? copy.thinking : copy.thoughtCompleted} · {duration}</span>
        <ChevronDown size={13} className={`ml-auto transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>
      {open && <div className="ml-3 pl-3 py-1 border-l border-violet-500/15 text-[12px] leading-6 text-gray-500 dark:text-gray-300 whitespace-pre-wrap">{item.text}</div>}
    </div>
  );
}

function PlanBlock({ plan, copy }) {
  const entries = plan && plan.entries || [];
  if (!entries.length) return null;
  return (
    <div className="rounded-2xl border border-violet-500/15 bg-violet-500/[0.04] p-3.5">
      <div className="text-[12px] font-semibold text-violet-600 dark:text-violet-300 mb-2">{copy.plan}</div>
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

function PermissionCard({ permission, pending, onRespond, responding, agentName, copy }) {
  const request = permission.request || {};
  const tool = request.toolCall || {};
  const options = request.options || [];
  const actionable = !!pending && !permission.resolved;
  return (
    <div className="rounded-2xl border border-amber-500/25 bg-amber-500/[0.06] p-4">
      <div className="flex items-start gap-3">
        <AlertTriangle size={18} className="text-amber-500 mt-0.5 shrink-0" />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold">{copy.permissionRequest(agentName)}</div>
          <div className="mt-1 text-[12px] text-gray-500 dark:text-gray-400">{tool.title || copy.protectedOperation}</div>
          {tool.rawInput && tool.rawInput.command
            ? <TerminalBlock label={copy.command} text={String(tool.rawInput.command)} />
            : <StructuredValue label={copy.operationArguments} value={tool.rawInput} />}
          <div className="mt-3 flex flex-wrap gap-2">
            {options.map(option => (
              <button key={option.optionId} disabled={!actionable || responding}
                onClick={() => onRespond(permission.toolCallId, option.optionId)}
                className={`max-w-full min-w-0 whitespace-normal break-all px-3 py-1.5 rounded-xl text-[12px] leading-5 font-medium transition-colors ${
                  String(option.kind || '').startsWith('allow')
                    ? 'bg-blue-600 text-white hover:bg-blue-700'
                    : 'bg-black/[0.06] dark:bg-white/10 hover:bg-black/10 dark:hover:bg-white/15'
                } disabled:opacity-45 disabled:cursor-not-allowed`}>
                {option.optionId === 'allow_once'
                  ? copy.allowOnce
                  : option.optionId === 'allow_always'
                    ? copy.allowSession
                    : option.optionId === 'reject_once'
                      ? copy.reject
                      : option.name}
              </button>
            ))}
          </div>
          {!actionable && <div className="mt-2 text-[11px] text-gray-400">{permission.resolved ? copy.handled : copy.expired}</div>}
        </div>
      </div>
    </div>
  );
}

function ElicitationCard({ elicitation, pending, onRespond, responding, copy, conversationCopy }) {
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
      otherPlaceholder: other && (other.field.title || (conversationCopy && conversationCopy.otherPlaceholder) || 'Other'),
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
      title={copy.choiceTitle}
      description={request.message && request.message !== 'Input requested' ? request.message : ''}
      questions={normalizedQuestions}
      resolved={!actionable}
      submitting={responding}
      submitLabel={copy.submit}
      cancelLabel={copy.cancel}
      otherAnswerLabel={conversationCopy && conversationCopy.otherAnswer}
      inputPlaceholder={conversationCopy && conversationCopy.inputPlaceholder}
      statusText={!actionable
        ? elicitation.resolved
          ? (elicitation.action === 'accept' ? copy.submitted : copy.canceled)
          : copy.inputExpired
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
  agentName,
  copy,
  cv,
  pendingByTool,
  pendingByElicitation,
  onRespond,
  onRespondElicitation,
  responding,
  onOpenExternal,
}) {
  if (item.type === 'reasoning') return <ReasoningItem item={item} now={now} copy={copy} />;
  if (item.type === 'tool_group') return <ToolGroup group={item} now={now} copy={copy} cv={cv} />;
  if (item.type === 'plan') return <PlanBlock plan={item.plan} copy={copy} />;
  if (item.type === 'permission') {
    return (
      <PermissionCard permission={item.permission}
        pending={pendingByTool[item.permission.toolCallId]}
        onRespond={onRespond} responding={responding} agentName={agentName} copy={copy} />
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
      ? <ConversationMarkdown text={item.text} onOpenExternal={onOpenExternal}
          className="text-[13px] leading-6 text-gray-500 dark:text-gray-400" />
      : <ConversationMarkdown text={item.text} onOpenExternal={onOpenExternal} />;
  }
  return null;
}

function Turn({
  turn,
  now,
  agentId,
  agentName,
  copy,
  cv,
  pendingByTool,
  pendingByElicitation,
  onRespond,
  onRespondElicitation,
  responding,
  onOpenExternal,
}) {
  const waitingPermission = turn.permissions.some(permission => !permission.resolved);
  const waitingInput = turn.elicitations.some(elicitation => !elicitation.resolved);
  const running = turn.status === 'running';
  const duration = copy.elapsed(elapsedMs(turn.startedAt, turn.completedAt, now));
  return (
    <section className="space-y-4">
      {(turn.userText || turn.userAttachments.length > 0) && (
        <div className="flex justify-end">
          <div className="max-w-[78%] rounded-[20px] rounded-br-md bg-[#E9EEF6] dark:bg-[#2A2B2E] px-4 py-3 text-[14px] leading-6 whitespace-pre-wrap break-words">
            {turn.userText && <div>{turn.userText}</div>}
            {turn.userAttachments.length > 0 && (
              <div className={`flex flex-wrap gap-1.5 ${turn.userText ? 'mt-2' : ''}`}>
                {turn.userAttachments.map((attachment, index) => (
                  <span key={`${attachment.name || 'attachment'}-${index}`}
                    className="inline-flex max-w-full items-center gap-1 rounded-lg bg-white/65 dark:bg-white/[0.07] px-2 py-1 text-[11px] leading-4">
                    <FileTypeIcon name={attachment.name} className="h-4 w-4 shrink-0" />
                    <span className="truncate">{attachment.name || copy.attachment}</span>
                  </span>
                ))}
              </div>
            )}
          </div>
        </div>
      )}
      <div className="flex items-start gap-3">
        <div className="mt-1 flex h-7 w-7 shrink-0 items-center justify-center text-[#1F1F1F] dark:text-[#E3E3E3]">
          <AcpAgentLogo agentId={agentId} className="h-5 w-5" title={agentName} />
        </div>
        <div className="min-w-0 flex-1 space-y-1">
          {running && (
            <div className={`h-9 flex items-center gap-2 text-[12px] ${waitingPermission || waitingInput ? 'text-amber-600 dark:text-amber-300' : 'text-gray-500 dark:text-gray-400'}`}>
              <span className={`w-1.5 h-1.5 rounded-full ${waitingPermission || waitingInput ? 'bg-amber-500' : 'bg-emerald-500 animate-pulse'}`} />
              {waitingPermission ? copy.waitingPermission : waitingInput ? copy.waitingInputShort : cv.processing} · {duration}
            </div>
          )}
          {turn.presentation.map((item, index) => (
            <TurnItem key={item.id || `${item.type}-${index}`} item={item} now={now}
              agentName={agentName} copy={copy} cv={cv}
              pendingByTool={pendingByTool} pendingByElicitation={pendingByElicitation}
              onRespond={onRespond} onRespondElicitation={onRespondElicitation}
              responding={responding} onOpenExternal={onOpenExternal} />
          ))}
          {(turn.completedAt || turn.error) && (
            <div className="flex items-center gap-2 pt-2">
              <StatusBadge status={turn.status} />
              <span className="text-[11px] text-gray-400">{duration}</span>
              {turn.usage && <span className="text-[11px] text-gray-400">{copy.contextUsage(Number(turn.usage.used || 0).toLocaleString(), Number(turn.usage.size || 0).toLocaleString())}</span>}
              {turn.error && <span className="text-[11px] text-red-500">{turn.error}</span>}
            </div>
          )}
        </div>
      </div>
    </section>
  );
}

function setupHintText(copy, hint) {
  return copy.setupHints?.[hint] || '';
}

function RuntimeNotice({
  status,
  working,
  error,
  onPrepare,
  onBrewInstall,
  onLogin,
  onOpenLogin,
  onSubmitLoginCode,
  onRefresh,
  copy,
}) {
  const [authorizationCode, setAuthorizationCode] = useState('');
  useEffect(() => {
    setAuthorizationCode('');
  }, [status?.agent_id, status?.login_in_progress]);
  if (!status) return <div className="text-[13px] text-gray-400">{copy.checking}</div>;
  const rawError = error || status.error;
  const visibleError = rawError
    ? (copy.showRawErrors ? rawError : copy.operationFailed)
    : '';
  if (!status.bridge_ready) {
    const isCodex = status.agent_id === 'codex';
    return (
      <div className="rounded-2xl border border-red-500/20 bg-red-500/[0.05] p-4 flex items-start gap-3">
        <AlertTriangle size={19} className="text-red-500 shrink-0 mt-0.5" />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold">{isCodex ? copy.bridgeUnavailable : copy.setupRequired}</div>
          <div className="mt-1 text-[12px] text-gray-500">{setupHintText(copy, status.setup_hint) || copy.bridgeRepair}</div>
          {visibleError && <div className="mt-2 text-[11px] text-red-500">{visibleError}</div>}
        </div>
        {!isCodex && (
          <button onClick={onRefresh} className="px-3 py-1.5 rounded-xl border border-red-500/20 text-[12px] font-medium">
            {copy.recheck}
          </button>
        )}
      </div>
    );
  }
  if (!status.codex_available) {
    const progress = status.download_progress;
    const isCodex = status.agent_id === 'codex';
    const installMethod = status.install_method;
    if (isCodex && (installMethod === 'homebrew' || installMethod === 'manual')) {
      const incompatible = Boolean(status.system_codex_incompatible);
      const brewInstallable = installMethod === 'homebrew' && status.brew_available;
      return (
        <div className="rounded-2xl border border-blue-500/20 bg-blue-500/[0.05] p-4 flex items-center gap-3">
          <Terminal size={19} className="text-blue-500 shrink-0" />
          <div className="min-w-0 flex-1">
            <div className="text-[13px] font-semibold">{incompatible ? copy.codexIncompatible : copy.codexMissing}</div>
            <div className="text-[12px] text-gray-500">
              {incompatible
                ? copy.codexIncompatibleHint(status.min_codex_version)
                : installMethod === 'manual'
                  ? copy.manualInstallHint
                  : status.brew_available
                    ? copy.brewInstallHint
                    : copy.brewMissingHint}
            </div>
            {visibleError && <div className="mt-1 text-[11px] text-red-500">{visibleError}</div>}
          </div>
          {brewInstallable && (
            <button onClick={onBrewInstall} disabled={working} className="px-3 py-1.5 rounded-xl bg-blue-600 text-white text-[12px] font-medium disabled:opacity-50">
              {working ? copy.brewInstalling : copy.brewInstall}
            </button>
          )}
        </div>
      );
    }
    return (
      <div className="rounded-2xl border border-blue-500/20 bg-blue-500/[0.05] p-4 flex items-center gap-3">
        <Terminal size={19} className="text-blue-500 shrink-0" />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold">{isCodex ? copy.codexMissing : copy.setupRequired}</div>
          <div className="text-[12px] text-gray-500">
            {isCodex ? copy.managedAvailable(status.managed_codex_version) : setupHintText(copy, status.setup_hint)}
          </div>
          {visibleError && <div className="mt-1 text-[11px] text-red-500">{visibleError}</div>}
        </div>
        <button onClick={isCodex ? onPrepare : onRefresh} disabled={working} className="px-3 py-1.5 rounded-xl bg-blue-600 text-white text-[12px] font-medium disabled:opacity-50">
          {isCodex
            ? (working ? (progress == null ? copy.downloading : copy.downloadProgress(progress)) : copy.downloadManaged)
            : copy.recheck}
        </button>
      </div>
    );
  }
  if (!status.authenticated) {
    const waitingForLogin = Boolean(status.login_in_progress);
    const loginUrlReady = waitingForLogin && Boolean(status.login_url);
    const agentName = status.agent_name || 'Agent';
    const waitingTitle = copy.waitingAgentLogin
      ? copy.waitingAgentLogin(agentName)
      : copy.waitingLogin;
    const signedOutTitle = copy.agentNotLoggedIn
      ? copy.agentNotLoggedIn(agentName)
      : copy.notLoggedIn;
    const loginHint = copy.agentLoginHint
      ? copy.agentLoginHint(agentName)
      : (setupHintText(copy, status.setup_hint) || copy.loginHint);
    return (
      <div className="rounded-2xl border border-amber-500/20 bg-amber-500/[0.06] p-4 flex items-start gap-3">
        <Sparkles size={19} className="text-amber-500 shrink-0" />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold">{waitingForLogin ? waitingTitle : signedOutTitle}</div>
          <div className="text-[12px] text-gray-500">
            {loginUrlReady
              ? (copy.finishAgentAuth ? copy.finishAgentAuth(agentName) : copy.finishBrowserAuth)
              : waitingForLogin
                ? copy.openingAuth
                : loginHint}
          </div>
          {status.login_code && (
            <div className="mt-2 inline-flex rounded-lg border border-amber-500/25 bg-white/70 px-2.5 py-1 font-mono text-[13px] font-semibold tracking-wider text-amber-800 dark:bg-black/20 dark:text-amber-200">
              {copy.deviceCode ? copy.deviceCode(status.login_code) : status.login_code}
            </div>
          )}
          {waitingForLogin && status.login_input_required && status.agent_id === 'claude' && (
            <form
              className="mt-2 flex max-w-md items-center gap-2"
              onSubmit={(event) => {
                event.preventDefault();
                const code = authorizationCode.trim();
                if (code) onSubmitLoginCode(code);
              }}
            >
              <input
                value={authorizationCode}
                onChange={event => setAuthorizationCode(event.target.value)}
                placeholder={copy.authorizationCodePlaceholder}
                aria-label={copy.authorizationCodePlaceholder}
                autoComplete="off"
                className="min-w-0 flex-1 rounded-lg border border-amber-500/25 bg-white/80 px-2.5 py-1.5 text-[12px] outline-none focus:border-amber-500 dark:bg-black/20"
              />
              <button
                type="submit"
                disabled={!authorizationCode.trim()}
                className="rounded-lg border border-amber-500/30 px-2.5 py-1.5 text-[12px] font-medium text-amber-700 disabled:opacity-40 dark:text-amber-300"
              >
                {copy.submitAuthorizationCode}
              </button>
            </form>
          )}
          {visibleError && <div className="mt-1 text-[11px] text-red-500">{visibleError}</div>}
        </div>
        {loginUrlReady && (
          <button onClick={onOpenLogin} className="px-3 py-1.5 rounded-xl border border-amber-500/30 text-amber-700 dark:text-amber-300 text-[12px] font-medium">
            {copy.reopenAuth}
          </button>
        )}
        <button onClick={onLogin} disabled={working || waitingForLogin} className="px-3 py-1.5 rounded-xl bg-amber-500 text-white text-[12px] font-medium disabled:opacity-50">
          {working || waitingForLogin ? copy.waitAuth : copy.authorize}
        </button>
      </div>
    );
  }
  if (visibleError) return <div className="rounded-xl bg-red-500/8 text-red-600 dark:text-red-300 px-3 py-2 text-[12px]">{visibleError}</div>;
  return null;
}

function runtimeSourceLabel(status, copy) {
  if (!status) return '';
  return copy?.runtimeSources?.[status.runtime_source] || '';
}

function AgentServiceFailureNotice({
  failure,
  agentName,
  working,
  onSwitchAccount,
  onDismiss,
  copy,
}) {
  if (!failure) return null;
  const recoverWithAccount = ['entitlement', 'quota', 'authentication'].includes(failure.kind);
  const title = failure.kind === 'entitlement'
    ? copy.entitlementUnavailable(agentName)
    : failure.kind === 'quota'
      ? copy.quotaUnavailable(agentName)
      : failure.kind === 'authentication'
        ? copy.authorizationExpired(agentName)
        : copy.serviceUnavailable(agentName);
  const description = recoverWithAccount
    ? copy.accountRecoveryHint
    : copy.serviceRecoveryHint;
  return (
    <div data-testid="acp-service-failure" className="rounded-2xl border border-red-500/20 bg-red-500/[0.055] p-4">
      <div className="flex items-start gap-3">
        <AlertTriangle size={19} className="mt-0.5 shrink-0 text-red-500" />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold text-red-700 dark:text-red-300">{title}</div>
          <div className="mt-1 text-[12px] leading-5 text-gray-500 dark:text-gray-400">{description}</div>
          <details className="mt-2">
            <summary className="cursor-pointer text-[11px] text-gray-400">{copy.errorDetails}</summary>
            <div className="mt-1 break-words text-[11px] text-red-500">{failure.detail}</div>
          </details>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {recoverWithAccount && (
            <button
              type="button"
              onClick={onSwitchAccount}
              disabled={working}
              className="rounded-xl bg-red-500 px-3 py-1.5 text-[12px] font-medium text-white disabled:opacity-50"
            >
              {copy.switchAccount}
            </button>
          )}
          <button
            type="button"
            onClick={onDismiss}
            disabled={working}
            className="rounded-xl border border-red-500/20 px-3 py-1.5 text-[12px] font-medium text-red-600 disabled:opacity-50 dark:text-red-300"
          >
            {copy.dismissNotice}
          </button>
        </div>
      </div>
    </div>
  );
}

export function CodexAcpView({
  theme,
  t,
  sessions = [],
  activeId = null,
  draftEpoch = 0,
  onActiveSessionChange,
  onSessionsChange,
  onSwitchHomeMode,
}) {
  const codexCopy = t.uiCodex;
  const [agents, setAgents] = useState([]);
  const [draftAgentId, setDraftAgentId] = useState('codex');
  const [status, setStatus] = useState(null);
  const [events, setEvents] = useState([]);
  const [pending, setPending] = useState([]);
  const [pendingElicitations, setPendingElicitations] = useState([]);
  const [sessionInfo, setSessionInfo] = useState(null);
  const [sessionInfoSessionId, setSessionInfoSessionId] = useState(null);
  const [sessionLoading, setSessionLoading] = useState(false);
  const [draft, setDraft] = useState('');
  const [attachmentDrafts, setAttachmentDrafts] = useState({});
  const [workspaceReferenceDrafts, setWorkspaceReferenceDrafts] = useState({});
  const [workspaceOpen, setWorkspaceOpen] = useState(false);
  const [workspaceChangeCount, setWorkspaceChangeCount] = useState(0);
  const [now, setNow] = useState(Date.now());
  const useUnifiedConversationUi = unifiedConversationUiEnabled();
  const [configApplying, setConfigApplying] = useState('');
  const [working, setWorking] = useState(false);
  const [error, setError] = useState('');
  const showError = (nextError) => {
    console.error('Codex operation failed:', nextError);
    setError(codexCopy.showRawErrors ? String(nextError) : codexCopy.operationFailed);
  };
  const [responding, setResponding] = useState(false);
  const [commandOpen, setCommandOpen] = useState(false);
  const [workspaceMenuOpen, setWorkspaceMenuOpen] = useState(false);
  const [accountMenuOpen, setAccountMenuOpen] = useState(false);
  const [dismissedFailureKey, setDismissedFailureKey] = useState('');
  const [draftWorkspacePath, setDraftWorkspacePath] = useState(null);
  const [recentWorkspaces, setRecentWorkspaces] = useState(loadRecentWorkspaces);
  const [draftControlsCache, setDraftControlsCache] = useState(loadDraftControlsCache);
  // 草稿态（会话未创建）下用户预选的配置：{ [agentId]: { model?, mode?, configs: { [id]: value } } }
  const [draftConfigSelections, setDraftConfigSelections] = useState({});
  const [showScrollBottom, setShowScrollBottom] = useState(false);
  const scroller = useRef(null);
  const autoScrollRef = useRef(true);
  const lastScrollTopRef = useRef(0);
  const attachmentIdRef = useRef(0);
  const skipNextActiveLoadRef = useRef(null);
  const sessionLoadRequestRef = useRef(0);
  const preserveDraftWorkspaceRef = useRef(false);
  const draftEpochRef = useRef(draftEpoch);
  const activeIdRef = useRef(activeId);
  activeIdRef.current = activeId;
  const projection = useMemo(() => projectAcpTimeline(events), [events]);
  // 草稿态（!activeId）没有会话，退回使用该 agent 缓存的配置快照来预展示选项。
  const draftControlsInfo = !activeId ? draftControlsCache[draftAgentId] || null : null;
  const sessionControlsInfo = sessionInfoSessionId === activeId ? sessionInfo : null;
  const controls = useMemo(
    () => resolveAcpSessionControls(sessionControlsInfo || draftControlsInfo),
    [sessionControlsInfo, draftControlsInfo],
  );
  const draftConfigSelection = draftConfigSelections[draftAgentId] || null;
  const composerControlsVisible = Boolean(sessionControlsInfo || draftControlsInfo);
  // 有会话时以会话上报为准；草稿态优先显示用户预选，其次显示缓存快照里的当前值。
  const composerModelValue = sessionControlsInfo
    ? sessionControlsInfo.current_model_id || ''
    : (draftConfigSelection && draftConfigSelection.model)
      || (draftControlsInfo && draftControlsInfo.current_model_id)
      || '';
  const composerModeValue = sessionControlsInfo
    ? controls.effectiveMode || ''
    : (draftConfigSelection && draftConfigSelection.mode) || controls.effectiveMode || '';
  function composerConfigOptionValue(option) {
    if (sessionControlsInfo) return option.currentValue || '';
    const staged = draftConfigSelection && draftConfigSelection.configs
      ? draftConfigSelection.configs[option.id]
      : undefined;
    return staged !== undefined ? String(staged) : (option.currentValue || '');
  }
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
  const activeConversationTurn = [...projection.turns]
    .reverse()
    .find(turn => turn.status === 'running') || null;
  const activeSession = useMemo(
    () => sessions.find(session => session.id === activeId) || null,
    [sessions, activeId],
  );
  const activeAgentId = activeSession?.agent_id || draftAgentId;
  const activeAgentName = activeSession?.agent_name
    || agents.find(agent => agent.agent_id === activeAgentId)?.agent_name
    || (activeAgentId === 'claude' ? 'Claude Code' : activeAgentId === 'kimi' ? 'Kimi' : 'Codex');
  const activeAgentIdRef = useRef(activeAgentId);
  activeAgentIdRef.current = activeAgentId;
  const activeStatus = status?.agent_id === activeAgentId ? status : null;
  const serviceFailure = useMemo(() => {
    const latestCompleted = [...events]
      .reverse()
      .find(envelope => envelope?.event?.type === 'turn_completed');
    return classifyAcpServiceFailure(latestCompleted);
  }, [events]);
  const visibleServiceFailure = serviceFailure?.key === dismissedFailureKey
    ? null
    : serviceFailure;
  const workspaceUnavailable = Boolean(
    activeSession
      && activeSession.workspace_kind === 'project'
      && activeSession.workspace_available === false,
  );
  const attachmentKey = activeId || DRAFT_ATTACHMENT_KEY;
  const attachments = attachmentDrafts[attachmentKey] || [];
  const workspaceReferences = workspaceReferenceDrafts[attachmentKey] || [];
  const sessionReady = !activeId || (
    sessionInfoSessionId === activeId && Boolean(sessionInfo)
  );
  const sessionSyncing = Boolean(activeId && !sessionReady && sessionLoading);

  function applySessionInfo(info, sessionId = activeIdRef.current) {
    if (sessionId !== activeIdRef.current) return info;
    setSessionInfo(info);
    setSessionInfoSessionId(sessionId || null);
    const agentId = activeAgentIdRef.current;
    const snapshot = rememberDraftControls(agentId, info);
    if (snapshot) {
      setDraftControlsCache(current => ({ ...current, [agentId]: snapshot }));
    }
    return info;
  }

  function stageDraftConfigSelection(patch) {
    setDraftConfigSelections(current => {
      const prev = current[draftAgentId] || {};
      const next = {
        model: patch.model !== undefined ? patch.model : prev.model,
        mode: patch.mode !== undefined ? patch.mode : prev.mode,
        configs: { ...(prev.configs || {}), ...(patch.configs || {}) },
      };
      return { ...current, [draftAgentId]: next };
    });
  }

  // 首次发送创建会话后，把草稿态预选的模型/权限模式/配置应用到新会话。
  // 以新会话实际上报的 config_options 为准自适应：走 config 的项用 set_config_option，
  // 否则退回 set_model/set_mode；与当前值相同或会话未暴露的项跳过。
  async function applyDraftConfigSelections(targetId, info) {
    const staged = draftConfigSelections[draftAgentId];
    if (!staged) return info;
    let current = info || null;
    const currentOptionValue = (configId) => {
      const options = current && Array.isArray(current.config_options) ? current.config_options : [];
      const option = options.find(item => item && item.id === configId);
      return option ? String(option.currentValue ?? '') : null;
    };
    try {
      if (staged.model) {
        const viaConfig = currentOptionValue('model') !== null;
        const currentValue = viaConfig
          ? currentOptionValue('model')
          : String(current && current.current_model_id || '');
        if (String(staged.model) !== currentValue) {
          current = viaConfig
            ? await invoke('set_codex_acp_config_option', { sessionId: targetId, configId: 'model', valueId: staged.model })
            : await invoke('set_codex_acp_model', { sessionId: targetId, modelId: staged.model });
        }
      }
      if (staged.mode) {
        const viaConfig = currentOptionValue('mode') !== null;
        const currentValue = viaConfig
          ? currentOptionValue('mode')
          : String(current && current.modes && current.modes.currentModeId || '');
        if (String(staged.mode) !== currentValue) {
          current = viaConfig
            ? await invoke('set_codex_acp_config_option', { sessionId: targetId, configId: 'mode', valueId: staged.mode })
            : await invoke('set_codex_acp_mode', { sessionId: targetId, modeId: staged.mode });
        }
      }
      for (const [configId, valueId] of Object.entries(staged.configs || {})) {
        const optionValue = currentOptionValue(configId);
        if (optionValue === null || optionValue === String(valueId)) continue;
        current = await invoke('set_codex_acp_config_option', { sessionId: targetId, configId, valueId });
      }
    } catch (err) {
      showError(err);
    }
    return current;
  }

  async function refreshSessions() {
    const next = await invoke('list_codex_acp_sessions');
    const list = next || [];
    if (onSessionsChange) onSessionsChange(list);
    return list;
  }

  async function refreshAgents() {
    const next = await invoke('list_acp_agents');
    const list = next || [];
    setAgents(list);
    return list;
  }

  async function refreshStatus(agentId = activeAgentId) {
    const next = await invoke('get_acp_agent_status', { agentId });
    if (next?.agent_id === activeAgentIdRef.current) setStatus(next);
    return next;
  }

  function selectDraftAgent(agentId) {
    if (activeId || !agentId) return;
    setDraftAgentId(agentId);
    setStatus(null);
    setError('');
  }

  async function loadSession(id) {
    const requestId = sessionLoadRequestRef.current + 1;
    sessionLoadRequestRef.current = requestId;
    activeIdRef.current = id;
    setError('');
    setSessionInfo(null);
    setSessionInfoSessionId(null);
    setSessionLoading(true);
    try {
      const [timeline, permissions, elicitations] = await Promise.all([
        invoke('get_codex_acp_timeline', { sessionId: id }),
        invoke('get_codex_acp_pending_permissions', { sessionId: id }),
        invoke('get_codex_acp_pending_elicitations', { sessionId: id }),
      ]);
      if (sessionLoadRequestRef.current !== requestId) return null;
      setEvents(timeline || []);
      setPending(permissions || []);
      setPendingElicitations(elicitations || []);
      const session = sessions.find(item => item.id === id);
      const runtime = await invoke('get_acp_agent_status', {
        agentId: session?.agent_id || draftAgentId,
      });
      if (sessionLoadRequestRef.current !== requestId) return null;
      if (runtime?.agent_id === activeAgentIdRef.current) setStatus(runtime);
      if (runtime.installed && runtime.node_supported) {
        try {
          const info = await invoke('get_codex_acp_session_info', { sessionId: id });
          if (sessionLoadRequestRef.current !== requestId) return null;
          return applySessionInfo(info, id);
        } catch (err) {
          if (sessionLoadRequestRef.current === requestId) showError(err);
        }
      }
      return null;
    } finally {
      if (sessionLoadRequestRef.current === requestId) setSessionLoading(false);
    }
  }

  async function createSession(workspacePath = draftWorkspacePath) {
    setError('');
    setWorkspaceMenuOpen(false);
    const metadata = await invoke('create_codex_acp_session', {
      workspacePath,
      agentId: draftAgentId,
    });
    if (workspacePath) setRecentWorkspaces(rememberWorkspace(workspacePath));
    await refreshSessions();
    skipNextActiveLoadRef.current = metadata.id;
    if (onActiveSessionChange) onActiveSessionChange(metadata.id);
    const info = await loadSession(metadata.id);
    return { id: metadata.id, info };
  }

  function beginDraft(workspacePath = null, { clearComposer = false } = {}) {
    preserveDraftWorkspaceRef.current = true;
    setWorkspaceMenuOpen(false);
    setDraftWorkspacePath(workspacePath);
    if (clearComposer) {
      setDraft('');
      setAttachmentDrafts(current => {
        const next = { ...current };
        delete next[DRAFT_ATTACHMENT_KEY];
        return next;
      });
      setWorkspaceReferenceDrafts(current => {
        const next = { ...current };
        delete next[DRAFT_ATTACHMENT_KEY];
        return next;
      });
    } else if (activeId) {
      setAttachmentDrafts(current => ({
        ...current,
        [DRAFT_ATTACHMENT_KEY]: current[activeId] || [],
      }));
      setWorkspaceReferenceDrafts(current => ({
        ...current,
        [DRAFT_ATTACHMENT_KEY]: current[activeId] || [],
      }));
    }
    setEvents([]);
    setPending([]);
    setPendingElicitations([]);
    sessionLoadRequestRef.current += 1;
    setSessionInfo(null);
    setSessionInfoSessionId(null);
    setSessionLoading(false);
    setError('');
    if (onActiveSessionChange) onActiveSessionChange(null);
  }

  function recreateUnavailableWorkspaceSession() {
    if (activeSession && activeSession.workspace_path) {
      setRecentWorkspaces(forgetWorkspace(activeSession.workspace_path));
    }
    beginDraft(null);
    setWorkspaceMenuOpen(true);
  }

  async function chooseProjectDraft() {
    const selected = await openTauriDialog({
      directory: true,
      multiple: false,
      title: codexCopy.chooseProjectDialog,
    });
    const path = Array.isArray(selected) ? selected[0] : selected;
    if (path) {
      setRecentWorkspaces(rememberWorkspace(path));
      beginDraft(path);
    }
  }

  function updateAttachments(sessionId, update) {
    if (!sessionId) return;
    setAttachmentDrafts(current => {
      const previous = current[sessionId] || [];
      const next = typeof update === 'function' ? update(previous) : update;
      return { ...current, [sessionId]: next };
    });
  }

  async function addAttachmentByPath(path, sessionId = attachmentKey) {
    if (!path || !sessionId) return;
    const id = `codex-attachment-${++attachmentIdRef.current}`;
    const basename = String(path).split(/[\\/]/).filter(Boolean).pop() || String(path);
    updateAttachments(sessionId, current => [
      ...current,
      { id, basename, status: 'parsing', result: null, error: null },
    ]);
    try {
      const result = await invoke('ingest_file', { path });
      updateAttachments(sessionId, current => current.map(attachment => (
        attachment.id === id
          ? { ...attachment, basename: result.basename || basename, status: 'ready', result }
          : attachment
      )));
    } catch (err) {
      updateAttachments(sessionId, current => current.map(attachment => (
        attachment.id === id
          ? { ...attachment, status: 'error', error: String(err) }
          : attachment
      )));
    }
  }

  async function pickAttachments() {
    const selected = await openTauriDialog({
      multiple: true,
      directory: false,
      title: codexCopy.addAttachmentDialog,
    });
    const paths = Array.isArray(selected) ? selected : selected ? [selected] : [];
    await Promise.all(paths.map(path => addAttachmentByPath(path, attachmentKey)));
  }

  function removeAttachment(id) {
    updateAttachments(attachmentKey, current => current.filter(attachment => attachment.id !== id));
  }

  function addWorkspaceReference(relativePath) {
    if (!relativePath || !attachmentKey) return;
    setWorkspaceReferenceDrafts(current => {
      const previous = current[attachmentKey] || [];
      if (previous.includes(relativePath)) return current;
      return { ...current, [attachmentKey]: [...previous, relativePath] };
    });
  }

  function removeWorkspaceReference(relativePath) {
    setWorkspaceReferenceDrafts(current => ({
      ...current,
      [attachmentKey]: (current[attachmentKey] || []).filter(path => path !== relativePath),
    }));
  }

  function handlePaste(event) {
    const items = Array.from(event.clipboardData && event.clipboardData.items || []);
    const images = items.filter(item => item.type && item.type.startsWith('image/'));
    if (!images.length) return;
    event.preventDefault();
    images.forEach(item => {
      const file = item.getAsFile();
      if (!file) return;
      const reader = new FileReader();
      reader.onload = async () => {
        const bytes = Array.from(new Uint8Array(reader.result));
        const ext = (file.type.split('/')[1] || 'png').replace('jpeg', 'jpg');
        try {
          const path = await invoke('save_paste_image', {
            filename: `paste-${Date.now()}.${ext}`,
            bytes,
          });
          await addAttachmentByPath(path, attachmentKey);
        } catch (err) {
          showError(err);
        }
      };
      reader.readAsArrayBuffer(file);
    });
  }

  useEffect(() => {
    let unlisten = null;
    Promise.all([refreshAgents(), refreshSessions()]).catch(showError);
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
          invoke('get_codex_acp_session_info', { sessionId: incoming.sessionId })
            .then(info => applySessionInfo(info, incoming.sessionId))
            .catch(() => {});
        }
      }
    }).then(fn => { unlisten = fn; });
    return () => { if (unlisten) unlisten(); };
  }, []);

  useEffect(() => {
    refreshStatus(activeAgentId).catch(showError);
  }, [activeAgentId]);

  useEffect(() => {
    const latest = events[events.length - 1];
    if (!isAcpAuthenticationFailure(latest)) return;
    refreshStatus(activeAgentId).catch(() => {});
  }, [events.length, activeAgentId]);

  useEffect(() => {
    if (!activeId) {
      activeIdRef.current = null;
      sessionLoadRequestRef.current += 1;
      if (preserveDraftWorkspaceRef.current) preserveDraftWorkspaceRef.current = false;
      else setDraftWorkspacePath(null);
      setEvents([]);
      setPending([]);
      setPendingElicitations([]);
      setSessionInfo(null);
      setSessionInfoSessionId(null);
      setSessionLoading(false);
      return;
    }
    if (skipNextActiveLoadRef.current === activeId) {
      skipNextActiveLoadRef.current = null;
      return;
    }
    loadSession(activeId).catch(showError);
  }, [activeId]);

  useEffect(() => {
    if (draftEpochRef.current === draftEpoch) return;
    draftEpochRef.current = draftEpoch;
    beginDraft(null, { clearComposer: true });
  }, [draftEpoch]);

  useEffect(() => {
    if (!activeStatus?.login_in_progress) return undefined;
    let cancelled = false;
    let timer = null;
    const poll = async () => {
      await refreshStatus(activeAgentId).catch(() => {});
      if (!cancelled) timer = window.setTimeout(poll, 750);
    };
    timer = window.setTimeout(poll, 750);
    return () => {
      cancelled = true;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [activeAgentId, activeStatus?.login_in_progress]);

  useEffect(() => {
    setNow(Date.now());
    if (!busy) return undefined;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [busy]);

  useEffect(() => {
    const element = scroller.current;
    if (!element) return undefined;
    const onScroll = () => {
      const near = isNearConversationBottom(element);
      const movingUp = element.scrollTop < lastScrollTopRef.current - 1;
      lastScrollTopRef.current = element.scrollTop;
      if (movingUp) autoScrollRef.current = false;
      else if (near) autoScrollRef.current = true;
      const shouldShow = !autoScrollRef.current
        && element.scrollHeight > element.clientHeight + 4;
      setShowScrollBottom(current => current === shouldShow ? current : shouldShow);
    };
    onScroll();
    element.addEventListener('scroll', onScroll, { passive: true });
    return () => element.removeEventListener('scroll', onScroll);
  }, []);

  useEffect(() => {
    const element = scroller.current;
    if (!element) return;
    if (autoScrollRef.current) {
      element.scrollTop = element.scrollHeight;
      setShowScrollBottom(false);
      return;
    }
    const shouldShow = element.scrollHeight > element.clientHeight + 4;
    setShowScrollBottom(current => current === shouldShow ? current : shouldShow);
  }, [events.length, projection.turns.length]);

  useEffect(() => {
    autoScrollRef.current = true;
    lastScrollTopRef.current = 0;
    setShowScrollBottom(false);
    const frame = window.requestAnimationFrame(() => {
      const element = scroller.current;
      if (element) {
        element.scrollTop = element.scrollHeight;
        lastScrollTopRef.current = element.scrollTop;
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [activeId]);

  function scrollConversationToBottom() {
    const element = scroller.current;
    if (!element) return;
    autoScrollRef.current = true;
    setShowScrollBottom(false);
    element.scrollTo({ top: element.scrollHeight, behavior: 'smooth' });
  }

  async function prepare() {
    setWorking(true); setError('');
    const poll = window.setInterval(() => refreshStatus(activeAgentId).catch(() => {}), 500);
    try {
      const next = await invoke('prepare_codex_acp');
      if (next?.agent_id === activeAgentIdRef.current) setStatus(next);
    }
    catch (err) { showError(err); }
    finally { window.clearInterval(poll); await refreshStatus(activeAgentId).catch(() => {}); setWorking(false); }
  }

  async function brewInstall() {
    setWorking(true); setError('');
    const poll = window.setInterval(() => refreshStatus().catch(() => {}), 500);
    try { setStatus(await invoke('install_codex_homebrew')); }
    catch (err) { showError(err); }
    finally { window.clearInterval(poll); await refreshStatus().catch(() => {}); setWorking(false); }
  }

  async function login() {
    setWorking(true); setError('');
    try {
      const next = await invoke('login_acp_agent', { agentId: activeAgentId });
      if (next?.agent_id === activeAgentIdRef.current) setStatus(next);
    }
    catch (err) { showError(err); }
    finally { setWorking(false); }
  }

  async function switchAccount() {
    setAccountMenuOpen(false);
    if (serviceFailure?.key) setDismissedFailureKey(serviceFailure.key);
    setWorking(true);
    setError('');
    try {
      const next = await invoke('switch_acp_agent_account', { agentId: activeAgentId });
      if (next?.agent_id === activeAgentIdRef.current) setStatus(next);
    } catch (err) {
      showError(err);
    } finally {
      setWorking(false);
    }
  }

  async function openLogin() {
    setError('');
    try { await invoke('open_acp_agent_login_url', { agentId: activeAgentId }); }
    catch (err) { showError(err); }
  }

  async function submitLoginCode(code) {
    setError('');
    try {
      await invoke('submit_acp_agent_login_code', { agentId: activeAgentId, code });
      await refreshStatus(activeAgentId);
    } catch (err) {
      showError(err);
    }
  }

  async function send() {
    const message = draft.trim();
    const readyAttachments = attachments.filter(attachment => (
      attachment.status === 'ready' && attachment.result
    ));
    if ((!message && !readyAttachments.length && !workspaceReferences.length) || busy || working) return;
    if (!activeStatus?.authenticated) {
      setError(codexCopy.loginRequiredBeforeSend);
      return;
    }
    if (attachments.some(attachment => attachment.status === 'parsing')) {
      setError(codexCopy.attachmentsParsing);
      return;
    }
    if (workspaceUnavailable) return;
    if (activeId && !sessionReady) return;
    setWorking(true); setError('');
    try {
      let targetId = activeId;
      if (!targetId) {
        const created = await createSession(draftWorkspacePath);
        targetId = created.id;
        const appliedInfo = await applyDraftConfigSelections(targetId, created.info);
        if (appliedInfo && appliedInfo !== created.info) applySessionInfo(appliedInfo, targetId);
        setDraftConfigSelections(current => {
          const next = { ...current };
          delete next[draftAgentId];
          return next;
        });
        setAttachmentDrafts(current => {
          const draftAttachments = current[DRAFT_ATTACHMENT_KEY] || [];
          const next = { ...current, [targetId]: draftAttachments };
          delete next[DRAFT_ATTACHMENT_KEY];
          return next;
        });
        setWorkspaceReferenceDrafts(current => {
          const draftReferences = current[DRAFT_ATTACHMENT_KEY] || [];
          const next = { ...current, [targetId]: draftReferences };
          delete next[DRAFT_ATTACHMENT_KEY];
          return next;
        });
      }
      autoScrollRef.current = true;
      setShowScrollBottom(false);
      setDraft('');
      await invoke('codex_acp_prompt', {
        sessionId: targetId,
        message,
        attachments: readyAttachments.map(attachment => attachment.result),
        workspaceReferences,
      });
      updateAttachments(targetId, current => current.filter(
        attachment => !readyAttachments.some(ready => ready.id === attachment.id),
      ));
      setWorkspaceReferenceDrafts(current => ({ ...current, [targetId]: [] }));
    } catch (err) {
      showError(err);
      setDraft(message);
    } finally {
      setWorking(false);
    }
  }

  async function cancel() {
    if (!activeId) return;
    await invoke('cancel_codex_acp', { sessionId: activeId }).catch(showError);
  }

  async function respond(toolCallId, optionId) {
    if (!activeId) return;
    setResponding(true); setError('');
    try {
      await invoke('respond_codex_acp_permission', { sessionId: activeId, toolCallId, optionId });
      setPending(current => current.filter(item => item.toolCallId !== toolCallId));
    } catch (err) { showError(err); }
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
    } catch (err) { showError(err); }
    finally { setResponding(false); }
  }

  async function changeModel(modelId) {
    if (!modelId) return;
    if (!activeId) {
      stageDraftConfigSelection({ model: modelId });
      return;
    }
    setWorking(true); setConfigApplying('model');
    try { applySessionInfo(await invoke('set_codex_acp_model', { sessionId: activeId, modelId })); }
    catch (err) { showError(err); }
    finally { setWorking(false); setConfigApplying(''); }
  }

  async function changeConfig(configId, valueId) {
    if (!activeId) {
      stageDraftConfigSelection({ configs: { [configId]: valueId } });
      return;
    }
    setWorking(true); setConfigApplying(configId); setError('');
    try {
      applySessionInfo(await invoke('set_codex_acp_config_option', {
        sessionId: activeId, configId, valueId,
      }));
    } catch (err) { showError(err); }
    finally { setWorking(false); setConfigApplying(''); }
  }

  async function changeMode(modeId) {
    if (!modeId) return;
    if (!activeId) {
      stageDraftConfigSelection({ mode: modeId });
      return;
    }
    setWorking(true); setConfigApplying('mode'); setError('');
    try {
      applySessionInfo(await invoke('set_codex_acp_mode', { sessionId: activeId, modeId }));
    } catch (err) { showError(err); }
    finally { setWorking(false); setConfigApplying(''); }
  }

  return (
    <div className={`relative h-full min-h-0 flex flex-col ${theme === 'dark' ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>
        {activeSession && (
        <header className="h-14 shrink-0 px-5 flex items-center gap-3 border-b border-black/[0.05] dark:border-white/[0.06]">
          <div className="w-8 h-8 rounded-xl bg-black/[0.04] dark:bg-white/[0.08] flex items-center justify-center"><AcpAgentLogo agentId={activeAgentId} className="h-5 w-5" title={activeAgentName} /></div>
          <div className="min-w-0 flex-1">
            <div className="text-[14px] font-semibold">{activeSession.title || 'Codex'}</div>
            <div className={`text-[10px] truncate ${activeSession && !activeSession.workspace_available ? 'text-red-500' : 'text-gray-400'}`}
              title={activeSession && activeSession.workspace_path}>
              {`${activeAgentName} · ${activeSession.workspace_kind === 'project' ? activeSession.workspace_path : codexCopy.temporaryWorkspace}${activeSession.workspace_available ? '' : ` · ${codexCopy.projectMissing}`}`}
            </div>
          </div>
          {configApplying && <span className="text-[10px] text-blue-500 animate-pulse">{codexCopy.applyingConfig}</span>}
          {busy && <StatusBadge status="running" copy={t.uiConversation} />}
          <button
            type="button"
            onClick={() => setWorkspaceOpen(value => !value)}
            className={`h-8 px-2.5 rounded-lg inline-flex items-center gap-1.5 text-[11px] transition-colors ${
              workspaceOpen
                ? 'bg-blue-500/10 text-blue-600 dark:text-blue-300'
                : 'text-gray-500 dark:text-gray-400 hover:bg-black/[0.04] dark:hover:bg-white/[0.06]'
            }`}
            title={codexCopy.workspaceTitle}
          >
            <FolderOpen size={14} />
            <span>{codexCopy.workspace}</span>
            {workspaceChangeCount > 0 && (
              <span className="min-w-4 h-4 px-1 rounded-full bg-amber-500/15 text-amber-600 dark:text-amber-300 inline-flex items-center justify-center text-[9px] font-medium">
                {workspaceChangeCount > 99 ? '99+' : workspaceChangeCount}
              </span>
            )}
          </button>
        </header>
        )}

        <div className="flex-1 min-h-0 flex">
        <div className="relative min-w-0 flex-1 min-h-0 flex flex-col">
        <div ref={scroller} className="flex-1 min-h-0 overflow-y-auto custom-scrollbar">
          <div className="w-full max-w-[920px] min-h-full mx-auto px-6 py-6 flex flex-col gap-7">
            {workspaceUnavailable ? (
              <div
                data-testid="codex-workspace-unavailable"
                className="rounded-xl bg-red-500/8 px-3 py-2 text-[12px] text-red-600 dark:text-red-300"
              >
                {codexCopy.recreatePrefix}
                <button
                  type="button"
                  data-testid="codex-recreate-session"
                  onClick={recreateUnavailableWorkspaceSession}
                  className="font-medium underline underline-offset-2 hover:text-red-700 dark:hover:text-red-200"
                >
                  {codexCopy.recreate}
                </button>
              </div>
            ) : (
              <>
                <RuntimeNotice
                  status={activeStatus}
                  working={working}
                  error={error}
                  onPrepare={prepare}
                  onBrewInstall={brewInstall}
                  onLogin={login}
                  onOpenLogin={openLogin}
                  onSubmitLoginCode={submitLoginCode}
                  onRefresh={() => refreshStatus(activeAgentId)}
                  copy={codexCopy}
                />
                {activeStatus?.authenticated && (
                  <AgentServiceFailureNotice
                    failure={visibleServiceFailure}
                    agentName={activeAgentName}
                    working={working}
                    onSwitchAccount={switchAccount}
                    onDismiss={() => setDismissedFailureKey(serviceFailure?.key || '')}
                    copy={codexCopy}
                  />
                )}
              </>
            )}
            {!projection.turns.length && (
              <div className="flex min-h-[320px] flex-1 flex-col items-center justify-center text-center">
                <div className="w-14 h-14 rounded-2xl bg-black/[0.04] dark:bg-white/[0.08] flex items-center justify-center shadow-lg"><AcpAgentLogo agentId={activeAgentId} className="h-8 w-8" title={activeAgentName} /></div>
                <div className="mt-5 text-[20px] font-semibold">
                  {codexCopy.welcomeTitle}
                </div>
                <div className="mt-2 max-w-md text-[13px] leading-6 text-gray-500 dark:text-gray-400">
                  {activeSession
                    ? codexCopy.activeHint
                    : codexCopy.draftHint}
                </div>
              </div>
            )}
            {projection.turns.map(turn => useUnifiedConversationUi
              ? (
                  <ConversationTurn
                    key={turn.id}
                    turn={turn}
                    now={now}
                    copy={t.uiConversation}
                    pendingByTool={pendingByTool}
                    onRespond={respond}
                    responding={responding}
                    assistantAvatar={(
                      <div className="mt-1 flex h-7 w-7 shrink-0 items-center justify-center text-[#1F1F1F] dark:text-[#E3E3E3]">
                        <AcpAgentLogo agentId={activeAgentId} className="h-5 w-5" title={activeAgentName} />
                      </div>
                    )}
                    renderItem={(item) => item.type === 'elicitation'
                      ? (
                          <ElicitationCard
                            elicitation={item.elicitation}
                            pending={pendingByElicitation[item.elicitation.elicitationId]}
                            onRespond={respondElicitation}
                            responding={responding}
                            copy={codexCopy}
                            conversationCopy={t.uiConversation}
                          />
                        )
                      : undefined}
                    agentLabel={activeAgentName}
                    onOpenExternal={(url) => invoke('open_external_url', { url }).catch(showError)}
                  />
                )
              : (
                  <Turn key={turn.id} turn={turn} now={now}
                    agentId={activeAgentId} agentName={activeAgentName}
                    copy={t.uiConversation}
                    cv={t.uiCodexView}
                    pendingByTool={pendingByTool}
                    pendingByElicitation={pendingByElicitation}
                    onRespond={respond}
                    onRespondElicitation={respondElicitation}
                    responding={responding}
                    onOpenExternal={(url) => invoke('open_external_url', { url }).catch(showError)} />
                ))}
          </div>
        </div>

        <div className={`relative shrink-0 px-6 pt-2 ${activeId ? 'pb-5' : 'pb-[60px]'}`}>
          {showScrollBottom && (
            <div className="pointer-events-none absolute inset-x-0 bottom-full z-20 flex justify-center pb-2">
              <button
                type="button"
                onClick={scrollConversationToBottom}
                aria-label={pending.length || pendingElicitations.length ? codexCopy.attentionLatest : codexCopy.latest}
                title={pending.length || pendingElicitations.length ? codexCopy.attentionLatest : codexCopy.latest}
                className={`pointer-events-auto w-9 h-9 rounded-full flex items-center justify-center shadow-lg backdrop-blur transition-all hover:-translate-y-0.5 active:translate-y-0 border ${
                  pending.length || pendingElicitations.length
                    ? 'bg-amber-500/95 text-white border-amber-400'
                    : 'bg-white/95 dark:bg-[#2B2C2F]/95 text-[#1F1F1F] dark:text-[#E3E3E3] border-black/10 dark:border-white/10'
                }`}
              >
                <ChevronDown size={15} />
              </button>
            </div>
          )}
          <div className={`w-full mx-auto ${activeId ? 'max-w-[920px]' : 'max-w-[800px]'}`}>
            {!activeId && (
              <HomeModeSwitcher
                mode="code"
                codeSupported
                codeAgent={activeAgentId}
                onCodeAgentChange={selectDraftAgent}
                isDark={theme === 'dark'}
                onChange={onSwitchHomeMode}
                copy={t.uiHomeMode}
              />
            )}
            {sessionSyncing && (
              <div data-testid="acp-session-loading" className="mb-2 flex items-center gap-2 px-3 text-[11px] text-blue-600 dark:text-blue-300">
                <span className="h-3 w-3 shrink-0 animate-spin rounded-full border-2 border-blue-500/20 border-t-blue-500" />
                <span>{codexCopy.sessionSyncing}</span>
              </div>
            )}
            {error && <div className="mb-2 px-3 text-[11px] text-red-500 break-words">{error}</div>}
            <div className="relative rounded-[24px] border border-black/[0.08] dark:border-white/10 bg-white/85 dark:bg-[#1B1C1E]/90 backdrop-blur-xl shadow-lg px-4 pt-3 pb-2.5 focus-within:border-blue-400/50">
              <ConversationActivityIndicator
                turn={activeConversationTurn}
                now={now}
                onRequestAttention={scrollConversationToBottom}
                className="mb-0.5"
                copy={t.uiConversation}
              />
              <AttachmentChips
                attachments={attachments}
                onRemove={removeAttachment}
                dark={theme === 'dark'}
                parsingLabel={t.uiAttachments.parsing}
                uploadingLabel={t.uiAttachments.uploading}
                failedLabel={t.uiAttachments.failed}
                removeLabel={t.uiAttachments.remove}
                className="mb-2"
                formatError={value => String(value || '')}
              />
              {workspaceReferences.length > 0 && (
                <div className="mb-2 flex flex-wrap items-center gap-1.5">
                  {workspaceReferences.map(path => (
                    <span
                      key={path}
                      title={path}
                      className="max-w-[260px] h-7 pl-2.5 pr-1 rounded-lg inline-flex items-center gap-1.5 bg-blue-500/8 text-blue-700 dark:text-blue-300 text-[10px]"
                    >
                      <FileText size={12} className="shrink-0" />
                      <span className="truncate">@{path}</span>
                      <button
                        type="button"
                        onClick={() => removeWorkspaceReference(path)}
                        className="w-5 h-5 rounded-md flex items-center justify-center hover:bg-blue-500/10"
                        aria-label={codexCopy.removeReference(path)}
                      >
                        ×
                      </button>
                    </span>
                  ))}
                </div>
              )}
              {commandOpen && availableCommands.length > 0 && (
                <>
                  <button aria-label={codexCopy.commandMenuClose} className="fixed inset-0 z-30 cursor-default" onClick={() => setCommandOpen(false)} />
                  <div className="absolute z-40 left-0 right-0 bottom-full mb-2 max-h-72 overflow-y-auto rounded-2xl border border-black/[0.08] dark:border-white/10 bg-white/95 dark:bg-[#202124]/95 backdrop-blur-xl shadow-xl p-2">
                    <div className="px-2 py-1 text-[10px] uppercase tracking-wider text-gray-400">{codexCopy.agentCommands}</div>
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
                onPaste={handlePaste}
                onKeyDown={event => {
                  if (event.key === 'Enter' && !event.shiftKey) {
                    event.preventDefault();
                    if (!sessionSyncing) send();
                  }
                }}
                placeholder={codexCopy.placeholder}
                rows={1} className="w-full min-h-[48px] max-h-48 resize-none bg-transparent outline-none text-[15px] leading-6 placeholder:text-gray-400" />
              <div data-testid="codex-composer-footer" className="flex items-center justify-between mt-1">
                <div className="flex min-w-0 flex-wrap items-center gap-2 text-[10px] text-gray-400">
                  {!activeId && (
                    <div className="relative min-w-0">
                      <button
                        type="button"
                        data-testid="codex-workspace-selector"
                        onClick={() => setWorkspaceMenuOpen(value => !value)}
                        className="h-7 max-w-[180px] rounded-lg px-2 inline-flex items-center gap-1.5 text-[11px] text-gray-500 dark:text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]"
                        title={draftWorkspacePath || codexCopy.temporarySession}
                      >
                        {draftWorkspacePath
                          ? <FolderOpen size={13} className="shrink-0" />
                          : <Sparkles size={13} className="shrink-0 text-emerald-500" />}
                        <span className="truncate">
                          {draftWorkspacePath ? workspaceName(draftWorkspacePath, codexCopy.unknownDirectory) : codexCopy.temporarySession}
                        </span>
                        <ChevronDown size={12} className="shrink-0" />
                      </button>
                      {workspaceMenuOpen && (
                        <>
                          <button aria-label={codexCopy.workspaceMenuClose} className="fixed inset-0 z-30 cursor-default" onClick={() => setWorkspaceMenuOpen(false)} />
                          <div className="absolute z-40 bottom-9 left-0 w-[280px] max-w-[calc(100vw-32px)] rounded-2xl border border-black/[0.08] dark:border-white/10 bg-white/95 dark:bg-[#202124]/95 backdrop-blur-xl shadow-xl p-2">
                            <button type="button" onClick={() => chooseProjectDraft().catch(showError)}
                              className="w-full rounded-xl px-3 py-2.5 flex items-center gap-3 text-left hover:bg-black/[0.04] dark:hover:bg-white/[0.06]">
                              <FolderOpen size={16} className="text-blue-500 shrink-0" />
                              <span><span className="block text-[12px] font-semibold">{codexCopy.chooseProject}</span><span className="block text-[10px] text-gray-400 mt-0.5">{codexCopy.chooseProjectDesc}</span></span>
                            </button>
                            <button type="button" onClick={() => beginDraft(null)}
                              className="w-full rounded-xl px-3 py-2.5 flex items-center gap-3 text-left hover:bg-black/[0.04] dark:hover:bg-white/[0.06]">
                              <Sparkles size={16} className="text-emerald-500 shrink-0" />
                              <span><span className="block text-[12px] font-semibold">{codexCopy.temporarySession}</span><span className="block text-[10px] text-gray-400 mt-0.5">{codexCopy.temporarySessionDesc}</span></span>
                            </button>
                            {recentWorkspaces.length > 0 && (
                              <div className="mt-1 pt-2 border-t border-black/[0.05] dark:border-white/[0.06]">
                                <div className="px-3 pb-1 text-[10px] uppercase tracking-wider text-gray-400">{codexCopy.recentProjects}</div>
                                {recentWorkspaces.map(path => (
                                  <button key={path} type="button" title={path}
                                    onClick={() => beginDraft(path)}
                                    className="w-full rounded-lg px-3 py-1.5 flex items-center gap-2 text-left hover:bg-black/[0.04] dark:hover:bg-white/[0.06]">
                                    <FolderOpen size={13} className="shrink-0 text-gray-400" />
                                    <span className="truncate text-[11px]">{workspaceName(path, codexCopy.unknownDirectory)}</span>
                                  </button>
                                ))}
                              </div>
                            )}
                          </div>
                        </>
                      )}
                    </div>
                  )}
                  <button
                    type="button"
                    onClick={() => pickAttachments().catch(showError)}
                    className="w-7 h-7 rounded-lg flex items-center justify-center hover:bg-black/[0.05] dark:hover:bg-white/[0.07]"
                    title={codexCopy.addAttachment}
                    aria-label={codexCopy.addAttachment}
                  >
                    <Paperclip size={16} />
                  </button>
                  <button type="button" onClick={() => setCommandOpen(value => !value)}
                    disabled={!availableCommands.length}
                    className="h-7 px-2 rounded-lg text-[11px] font-mono hover:bg-black/[0.05] dark:hover:bg-white/[0.07] disabled:opacity-40"
                    title={availableCommands.length ? codexCopy.commandsAvailable : codexCopy.commandsAfterSession}>/</button>
                  <div className="relative min-w-0">
                    <button
                      type="button"
                      data-testid="acp-account-menu-trigger"
                      onClick={() => setAccountMenuOpen(value => !value)}
                      className="inline-flex h-7 min-w-0 max-w-[260px] items-center gap-1.5 rounded-lg px-2 text-[10px] text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]"
                      title={codexCopy.accountAndService}
                    >
                      <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${
                        visibleServiceFailure
                          ? 'bg-red-500'
                          : activeStatus?.installed && activeStatus?.authenticated
                            ? 'bg-emerald-500'
                            : 'bg-gray-400'
                      }`} />
                      <span className="hidden min-w-0 truncate sm:inline">
                        {activeStatus?.installed && activeStatus?.authenticated
                          ? `${activeAgentName} ${visibleServiceFailure ? codexCopy.serviceAbnormal : codexCopy.connectedSuffix}`
                          : `${activeAgentName} ${codexCopy.notReadySuffix}`}
                      </span>
                      <ChevronDown size={11} className="shrink-0" />
                    </button>
                    {accountMenuOpen && (
                      <>
                        <button
                          type="button"
                          aria-label={codexCopy.closeAccountMenu}
                          className="fixed inset-0 z-30 cursor-default"
                          onClick={() => setAccountMenuOpen(false)}
                        />
                        <div
                          data-testid="acp-account-menu"
                          className="absolute bottom-9 left-0 z-40 w-[300px] max-w-[calc(100vw-32px)] rounded-2xl border border-black/[0.08] bg-white/95 p-2 shadow-xl backdrop-blur-xl dark:border-white/10 dark:bg-[#202124]/95"
                        >
                          <div className="flex items-center gap-3 px-3 py-2.5">
                            <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl bg-black/[0.04] dark:bg-white/[0.07]">
                              <AcpAgentLogo agentId={activeAgentId} className="h-5 w-5" title={activeAgentName} />
                            </div>
                            <div className="min-w-0 flex-1">
                              <div className="truncate text-[12px] font-semibold">{activeAgentName}</div>
                              <div className={`mt-0.5 text-[10px] ${visibleServiceFailure ? 'text-red-500' : 'text-gray-400'}`}>
                                {visibleServiceFailure
                                  ? codexCopy.serviceAbnormal
                                  : activeStatus?.authenticated
                                    ? codexCopy.accountAuthorized
                                    : codexCopy.accountNotAuthorized}
                                {runtimeSourceLabel(activeStatus, codexCopy) ? ` · ${runtimeSourceLabel(activeStatus, codexCopy)}` : ''}
                              </div>
                            </div>
                          </div>
                          <div className="mt-1 border-t border-black/[0.05] pt-1 dark:border-white/[0.06]">
                            <button
                              type="button"
                              onClick={switchAccount}
                              disabled={working || activeStatus?.login_in_progress}
                              className="flex w-full items-center gap-2.5 rounded-xl px-3 py-2 text-left text-[12px] font-medium hover:bg-black/[0.04] disabled:opacity-40 dark:hover:bg-white/[0.06]"
                            >
                              <User size={15} className="text-blue-500" />
                              <span className="min-w-0">
                                <span className="block">{codexCopy.switchAccount}</span>
                                <span className="mt-0.5 block text-[10px] font-normal text-gray-400">{codexCopy.switchAccountAffectsSessions}</span>
                              </span>
                            </button>
                            <button
                              type="button"
                              onClick={() => {
                                setAccountMenuOpen(false);
                                refreshStatus(activeAgentId).catch(showError);
                              }}
                              className="flex w-full items-center gap-2.5 rounded-xl px-3 py-2 text-left text-[12px] hover:bg-black/[0.04] dark:hover:bg-white/[0.06]"
                            >
                              <RefreshCw size={15} className="text-gray-400" />
                              {codexCopy.recheck}
                            </button>
                          </div>
                        </div>
                      </>
                    )}
                  </div>
                  {composerControlsVisible && (
                    <div data-testid="codex-composer-configs" className="flex flex-wrap items-center gap-2">
                      {controls.fallbackModels.length > 0 && (
                        <CodexComposerConfigSelect
                          id="model"
                          label={codexCopy.model}
                          value={composerModelValue}
                          choices={controls.fallbackModels.map(model => ({
                            value: model.id,
                            name: model.name || model.id,
                          }))}
                          onChange={changeModel}
                          disabled={busy || working}
                          unsetLabel={codexCopy.notSet}
                        />
                      )}
                      {controls.fallbackModes && controls.fallbackModes.availableModes && (
                        <CodexComposerConfigSelect
                          id="mode"
                          label={codexCopy.permissionMode}
                          value={composerModeValue}
                          choices={controls.fallbackModes.availableModes.map(item => ({
                            value: item.id,
                            name: item.name || item.id,
                          }))}
                          onChange={changeMode}
                          disabled={busy || working}
                          title={codexCopy.sessionModeTitle}
                          unsetLabel={codexCopy.notSet}
                        />
                      )}
                      {controls.configOptions.map(option => (
                        <CodexComposerConfigSelect
                          key={option.id}
                          id={option.id}
                          label={configLabel(option, codexCopy)}
                          value={composerConfigOptionValue(option)}
                          choices={configChoices(option)}
                          onChange={value => changeConfig(option.id, value)}
                          disabled={busy || working}
                          title={option.description || option.name}
                          unsetLabel={codexCopy.notSet}
                        />
                      ))}
                    </div>
                  )}
                </div>
                {busy ? (
                  <button onClick={cancel} className="w-9 h-9 rounded-full flex items-center justify-center bg-red-500/10 text-red-500 hover:bg-red-500/15"><StopCircle size={18} /></button>
                ) : (
                  <button onClick={send} disabled={!sessionReady || (!draft.trim() && !attachments.some(attachment => attachment.status === 'ready') && !workspaceReferences.length) || working || !activeStatus || !activeStatus.installed || !activeStatus.authenticated}
                    className="w-9 h-9 rounded-full flex items-center justify-center bg-[#007AFF] text-white shadow-sm hover:bg-[#006EE6] disabled:bg-black/[0.06] dark:disabled:bg-white/10 disabled:text-gray-400 disabled:shadow-none">
                    <Send size={16} />
                  </button>
                )}
              </div>
            </div>
          </div>
        </div>
        </div>
        {activeSession && (
          <CodexWorkspacePanel
            session={activeSession}
            visible={workspaceOpen}
            onClose={() => setWorkspaceOpen(false)}
            references={workspaceReferences}
            onAddReference={addWorkspaceReference}
            refreshToken={events.length}
            onChangeCount={setWorkspaceChangeCount}
            copy={t.uiCodexWorkspace}
          />
        )}
        </div>
    </div>
  );
}
