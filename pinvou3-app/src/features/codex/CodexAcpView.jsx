import React, { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { FileTypeIcon } from '../../components/files/FileTypeIcon.jsx';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import { can } from '../../shared/platform.js';
import {
  AlertTriangle, Brain, Check, CheckCircle2, ChevronDown, FileText, FolderOpen, Mic, Paperclip, Plus, Send,
  RefreshCw, Sparkles, StopCircle, Terminal, User, Wrench,
} from '../../components/icons.jsx';
import { AcpAgentLogo } from './AcpAgentLogo.jsx';
import { CodexWorkspacePanel } from './CodexWorkspacePanel.jsx';
import { SubagentTranscriptPanel } from '../multiagent/SubagentTranscriptPanel.jsx';
import {
  classifyAcpServiceFailure,
  isAcpAuthenticationFailure,
  runtimeInstallInProgress,
  runtimeLoginInProgress,
  runtimeNoticeMode,
  runtimeOperationFor,
} from './runtimeNoticeState.js';
import { ComposerPopover } from '../../components/ComposerPopover.jsx';
import {
  appendAcpEvent,
  buildElicitationContent,
  commandExecutionDetails,
  projectAcpTimeline,
  resolveAcpSessionControls,
} from './acp-state.js';
import {
  applyNativeChatEvent,
  appendLocalUserMessage,
  appendNativeSystemItem,
  createNativeLane,
  hydrateNativeLane,
  projectNativeLane,
  removeLocalUserMessage,
} from './code-native-lane.js';
import {
  CODE_MODE_FALLBACK,
  nativeModeFallback,
  needsYoloConfirmation,
  resolveNativeModeValue,
} from './code-permission-state.js';
import {
  ConversationActivityIndicator,
  ConversationMarkdown,
  ConversationTurn,
  WorkspaceResourceButtons,
} from '../conversation/ConversationTimeline.jsx';
import { AssistantMessageActions, AssistantMessageFooter } from '../conversation/AssistantMessageActions.jsx';
import { assistantResponseAvailable, assistantResponseText } from '../conversation/message-clipboard.js';
import {
  ComposerModelSelector,
  ComposerToolMenu,
} from '../settings/SettingsView.jsx';
import {
  COMPOSER_ICON_BUTTON_CLASS,
  ComposerKbSelector,
  ComposerModeChip,
} from '../chat/composer-controls.jsx';
import { visibleUserModels } from '../../shared/model-options.js';
import { selectorMainLabel } from '../settings/model-catalog.js';
import {
  captureConversationScrollPosition,
  collectToolWorkspaceResources,
  isFetchTool,
  isNearConversationBottom,
  isSearchTool,
  restoreConversationScrollPosition,
  toolWorkspaceResources,
} from '../conversation/conversation-model.js';
import { QuestionChoiceCard } from '../conversation/QuestionChoiceCard.jsx';
import { PlanLayer, ToolCard, cardBoxCls, cardBtnCls } from '../tools/tool-renderers.jsx';
import { AttachmentChips } from '../attachments/AttachmentChips.jsx';
import { HomeModeSwitcher } from '../conversation/HomeModeSwitcher.jsx';
import { bridge } from '../../hooks/useBridge.js';
import {
  invokeTauri,
  listenTauri,
  openTauriDialog,
} from '../../platform/tauri/client.js';

const invoke = invokeTauri;
const RECENT_WORKSPACES_KEY = 'pinvou_codex_recent_workspaces';
const UNIFIED_CONVERSATION_UI_KEY = 'pinvou_conversation_ui_v2';
const DRAFT_ATTACHMENT_KEY = '__codex_draft__';

// 草稿配置快照缓存已抽到 ./acp-draft-controls.js（供设置页共用，避免与
// SettingsView 的循环引用）。
import {
  consumeAcpModelsProbePending,
  loadDraftControlsCache,
  rememberDraftControls,
} from './acp-draft-controls.js';
const AGENT_SELECTION_KEY = 'pinvou_codex_agent_selection';
const CODE_AGENT_IDS = ['pinvou', 'codex', 'claude', 'kimi'];

function unifiedConversationUiEnabled() {
  try {
    return localStorage.getItem(UNIFIED_CONVERSATION_UI_KEY) !== 'false';
  } catch {
    return true;
  }
}

function workspaceName(path, unknownDirectory) {
  const normalized = String(path || '').replace(/[\\/]+$/, '');
  if (!normalized) return unknownDirectory;
  return normalized.split(/[\\/]/).filter(Boolean).pop() || normalized;
}

// token 缩写与主聊天 ChatView 的 fmtCtxTok 同款（1.2k / 3.4M）。
function fmtNativeCtxTok(n) {
  return n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${(n / 1e3).toFixed(1)}k` : String(n);
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

// 记住用户上次在 code 界面选择的 agent：重开界面/重启应用后沿用，直到用户再次切换。
function loadAgentSelection() {
  try {
    const value = localStorage.getItem(AGENT_SELECTION_KEY);
    return value && CODE_AGENT_IDS.includes(value) ? value : null;
  } catch {
    return null;
  }
}

function saveAgentSelection(agentId) {
  if (!agentId) return;
  try {
    localStorage.setItem(AGENT_SELECTION_KEY, agentId);
  } catch {
    // 写不进去仅影响下次打开界面的默认值，本次会话不受影响。
  }
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
    case 'mode': return labels.mode || '';
    case 'collaboration_mode': return labels.collaboration_mode || '';
    case 'model': return labels.model || '';
    case 'reasoning_effort': return labels.reasoning_effort || '';
    case 'fast-mode': return labels['fast-mode'] || '';
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
  unsetLabel,
  testId,
  footerAction,
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
    <div className="relative min-w-0" data-testid={testId || `codex-config-${id}`}>
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
              <span className="flex shrink-0 items-center gap-1.5">
                {/* 槽位/别名标签：Claude 的 5 个选项显示名相同（同为槽位映射的
                    模型名），用别名标签区分，避免「五个一样的模型」 */}
                {choice.tag && choice.tag !== choice.name && (
                  <span className="rounded-md bg-black/[0.05] px-1.5 py-0.5 font-mono text-[10px] text-gray-500 group-hover:bg-white/20 group-hover:text-white dark:bg-white/[0.08] dark:text-gray-400">
                    {choice.tag}
                  </span>
                )}
                {isSelected && <Check size={15} className="shrink-0 text-[#007AFF] group-hover:text-white" />}
              </span>
            </button>
          );
        })}
        {footerAction && (
          <>
            <div className="my-1 mx-2 h-px bg-black/5 dark:bg-white/10" />
            <button
              type="button"
              onClick={() => { setOpen(false); footerAction.onClick(); }}
              className="group flex w-full items-center gap-2.5 rounded-xl px-3 py-2.5 text-left text-[13px] text-gray-700 transition-colors hover:bg-[#007AFF] hover:text-white dark:text-gray-200"
            >
              <Plus size={15} className="shrink-0 text-gray-400 group-hover:text-white/90" />
              <span className="min-w-0 truncate">{footerAction.label}</span>
            </button>
          </>
        )}
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
    <div className="mt-3 min-w-0 max-w-full">
      <div className="mb-1.5 text-[10px] font-medium uppercase tracking-wider text-gray-400">{label}</div>
      <pre className="max-h-80 max-w-full overflow-auto whitespace-pre rounded-xl bg-[#F4F5F7] dark:bg-black/30 px-3 py-2.5 text-[12px] leading-5 font-mono text-gray-700 dark:text-gray-200">{text}</pre>
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

function CompactItemRow({ icon, title, meta, status, open, onToggle, controlsId }) {
  const tone = status === 'failed'
    ? 'text-red-500 bg-red-500/10'
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
  const details = commandExecutionDetails(item.tool);
  const state = terminalStatus(item.status, details.exitCode);
  const [open, setOpen] = useState(false);
  const detailsId = useId();
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
        meta={`${outcome}${countHint}`} status={state} open={open} controlsId={detailsId}
        onToggle={() => setOpen(value => !value)} />
      {open && (
        <div id={detailsId} data-testid="conversation-compact-item-content" className="px-3 pb-3 border-t border-black/[0.05] dark:border-white/[0.06]">
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

function GenericToolItem({ item, now, copy, cv, onOpenResource }) {
  const tool = item.tool || {};
  const state = terminalStatus(item.status);
  const [open, setOpen] = useState(false);
  const detailsId = useId();
  const duration = copy.elapsed(elapsedMs(item.startedAt, item.completedAt, now));
  const label = item.type === 'file_change' ? copy.fileChange : (tool.kind || cv.codexTool);
  const resources = toolWorkspaceResources(tool);
  return (
    <div className="rounded-xl border border-black/[0.05] dark:border-white/[0.07] bg-white/45 dark:bg-white/[0.015]">
      <CompactItemRow icon={<Wrench size={13} />} title={tool.title || label}
        meta={`${label} · ${state === 'running' ? `${copy.inProgress} · ${duration}` : state === 'failed' ? copy.failed : `${cv.ended} · ${duration}`}`}
        status={state} open={open} controlsId={detailsId}
        onToggle={() => setOpen(value => !value)} />
      <WorkspaceResourceButtons resources={resources} onOpenResource={onOpenResource} />
      {open && (
        <div id={detailsId} data-testid="conversation-compact-item-content" className="px-3 pb-3 border-t border-black/[0.05] dark:border-white/[0.06]">
          <StructuredValue label={copy.arguments} value={tool.rawInput} />
          <StructuredValue label={copy.result} value={tool.rawOutput != null ? tool.rawOutput : tool.content} />
        </div>
      )}
    </div>
  );
}

function ToolGroup({ group, now, copy, cv, onOpenResource }) {
  const items = group.items || [];
  const running = items.some(item => terminalStatus(item.status) === 'running');
  const failed = items.some(item => terminalStatus(
    item.status,
    item.type === 'command_execution' ? commandExecutionDetails(item.tool).exitCode : null,
  ) === 'failed');
  const [open, setOpen] = useState(false);
  const detailsId = useId();
  const hasDetails = items.length > 0;
  const resources = collectToolWorkspaceResources(items);
  return (
    <div className="min-w-0 max-w-full">
      <button type="button" onClick={() => setOpen(value => !value)}
        data-testid="conversation-tool-group-summary"
        aria-expanded={hasDetails ? Boolean(open) : undefined}
        aria-controls={hasDetails && open ? detailsId : undefined}
        className="w-full h-9 px-1 flex items-center gap-2 text-left text-[12px] text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200">
        <span className={`w-1.5 h-1.5 rounded-full ${failed ? 'bg-red-500' : running ? 'bg-blue-500 animate-pulse' : 'bg-gray-300 dark:bg-gray-600'}`} />
        <span>{running ? copy.executing : failed ? cv.stepsFailed : copy.executionSteps} · {items.length}</span>
        <ChevronDown size={13} className={`ml-auto transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>
      <WorkspaceResourceButtons resources={resources} onOpenResource={onOpenResource} />
      {open && hasDetails && (
        <div id={detailsId} data-testid="conversation-tool-group-content" className="min-w-0 max-w-full ml-3 pl-3 border-l border-black/[0.06] dark:border-white/[0.08] space-y-1.5 pb-1">
          {items.map(item => item.type === 'command_execution'
            ? <CommandExecutionItem key={item.id} item={item} now={now} copy={copy} />
            : <GenericToolItem key={item.id} item={item} now={now} copy={copy} cv={cv} onOpenResource={onOpenResource} />)}
        </div>
      )}
    </div>
  );
}

function ReasoningItem({ item, now, copy }) {
  const running = item.status === 'in_progress';
  const [open, setOpen] = useState(false);
  const detailsId = useId();
  const hasDetails = Boolean(item.text);
  const duration = copy.elapsed(elapsedMs(item.startedAt, item.completedAt, now));
  return (
    <div className="min-w-0 max-w-full">
      <button type="button" onClick={() => setOpen(value => !value)}
        data-testid="conversation-reasoning-toggle"
        aria-expanded={hasDetails ? Boolean(open) : undefined}
        aria-controls={hasDetails && open ? detailsId : undefined}
        className="w-full h-9 px-1 flex items-center gap-2 text-left text-[12px] text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200">
        <span className={`w-1.5 h-1.5 rounded-full bg-violet-500 ${running ? 'animate-pulse' : ''}`} />
        <span>{running ? copy.thinking : copy.thoughtCompleted} · {duration}</span>
        <ChevronDown size={13} className={`ml-auto transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>
      {open && hasDetails && <div id={detailsId} data-testid="conversation-reasoning-content" className="min-w-0 max-w-full ml-3 pl-3 py-1 border-l border-violet-500/15 text-[12px] leading-6 text-gray-500 dark:text-gray-300 whitespace-pre-wrap break-words [overflow-wrap:anywhere]">{item.text}</div>}
    </div>
  );
}

function PlanBlock({ plan, copy }) {
  const entries = plan && plan.entries || [];
  if (!entries.length) return null;
  return (
    <div data-testid="conversation-plan" className="min-w-0 max-w-full rounded-2xl border border-violet-500/15 bg-violet-500/[0.04] p-3.5">
      <div className="text-[12px] font-semibold text-violet-600 dark:text-violet-300 mb-2">{copy.plan}</div>
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
          <div className="mt-1 min-w-0 max-w-full text-[12px] text-gray-500 dark:text-gray-400 break-words [overflow-wrap:anywhere]">{tool.title || copy.protectedOperation}</div>
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
      otherPlaceholder: other && (other.field.title || (conversationCopy && conversationCopy.otherPlaceholder)),
      required: required.has(id)
        || Boolean(field && field._meta && field._meta.codex && field._meta.codex.isOther),
      inputType: field.type || 'string',
      secret: Boolean(field && field._meta && field._meta.codex && field._meta.codex.isSecret),
    };
  });

  function submit(groups) {
    // content 用无原型对象构造（见 buildElicitationContent）：answerKey 为
    // constructor/toString/__proto__ 时普通 {} 会命中 Object.prototype，字段在
    // JSON 序列化时静默丢失。
    const content = buildElicitationContent(groups);
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

// 原生（品悟 Engine）会话的选择确认卡：chat:user_input_required → submit_user_input。
// 选项归一化逻辑与主聊天 UserInputCard 对齐（allow_free_text / multi_select），
// 但提交走显式 sessionId，不依赖 bridge 全局 activeSession。
const NATIVE_CHAT_EVENTS = [
  'chat:user_message',
  'chat:turn_started',
  'chat:reasoning_start',
  'chat:reasoning_delta',
  'chat:reasoning_done',
  'chat:delta',
  'chat:tool_start',
  'chat:tool_delta',
  'chat:tool_end',
  'chat:shell_task_status',
  'chat:compaction',
  'chat:usage',
  'chat:memory',
  'chat:user_input_required',
  'chat:plan_snapshot',
  'chat:plan_ready',
  'chat:transient_error',
  'chat:done',
];

function isFreeTextPlaceholderOption(option) {
  const label = String(option?.label || '').trim();
  return /^(?:其他|其它|other)(?:\s*[\(（][^()（）]*[\)）])?$/i.test(label);
}

function NativeUserInputCard({ item, responding, onSubmitAnswers, onCancelInput, copy, conversationCopy }) {
  const questions = (item.questions || []).map((question, index) => {
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
  const actionable = !item.resolved;

  function submit(groups) {
    const answers = groups.flatMap(group => group.answers.map(answer => ({
      id: group.questionId,
      label: answer.other ? (conversationCopy && conversationCopy.otherAnswer) || answer.label : answer.label,
      value: String(answer.value),
      // 保留 other 标记：QuestionChoiceCard 还原历史答案时据此把“其他”与预设选项区分开，
      // 避免“其他值 == 预设 value”被误判为预设（评审 P2）。
      other: answer.other,
    })));
    onSubmitAnswers(item.toolCallId, answers);
  }

  return (
    <QuestionChoiceCard
      title={copy.choiceTitle}
      questions={questions}
      initialAnswers={item.restoredAnswers || []}
      resolved={!actionable}
      submitting={responding}
      submitLabel={copy.submit}
      cancelLabel={copy.cancel}
      otherAnswerLabel={conversationCopy && conversationCopy.otherAnswer}
      inputPlaceholder={conversationCopy && conversationCopy.inputPlaceholder}
      statusText={!actionable
        ? (item.cardState === 'cancelled' ? copy.canceled : copy.submitted)
        : ''}
      onSubmit={submit}
      onCancel={actionable ? () => onCancelInput(item.toolCallId) : undefined}
    />
  );
}

// 原生车道的 Plan 方案审批卡：结构镜像主聊天 PlanCard（tool-renderers.jsx），
// 批准/放弃走显式 sessionId 的 accept_plan / discard_plan，不经 bridge 全局 activeSession。
// lane 是纯数据不持文案：终态存 statusKey，这里映射三语（copy = uiCodex）。
const NATIVE_PLAN_STATUS_COPY = {
  approved: 'nativePlanApproved',
  discarded: 'nativePlanDiscarded',
  superseded: 'nativePlanSuperseded',
  historical: 'nativePlanHistorical',
};

function NativePlanCard({ item, theme, t, copy, modePlan, busy, onAccept, onDiscard }) {
  const isDark = theme === 'dark';
  const active = item.cardState === 'active' && !item.resolved && !!item.planId;
  const statusText = copy[NATIVE_PLAN_STATUS_COPY[item.statusKey]] || '';
  return (
    <div className={cardBoxCls('border-[#0B57D0]/20 dark:border-[#A8C7FA]/30')}>
      <div className={`text-[14px] font-semibold mb-3 ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{t.planReady}</div>
      {(!item.plan && !item.todos)
        ? <div className={`text-[13px] ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{t.planEmpty}</div>
        : <>
            <PlanLayer label={t.planLabel} explanation={item.plan && item.plan.explanation} items={item.plan && item.plan.items} field="step" />
            <PlanLayer label={t.planTodos} items={item.todos && item.todos.items} field="content" />
          </>}
      <div className={`h-px my-3 ${isDark ? 'bg-white/10' : 'bg-black/10'}`}></div>
      {active ? (
        <div className="flex items-center gap-2 flex-wrap">
          <span className={`text-[13px] mr-1 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{t.planNext}</span>
          <button
            type="button"
            data-testid="native-plan-accept"
            className={cardBtnCls('primary')}
            disabled={busy || !modePlan}
            onClick={() => onAccept(item)}
          >{t.planGo}</button>
          <button
            type="button"
            data-testid="native-plan-discard"
            className={cardBtnCls()}
            onClick={() => onDiscard(item)}
          >{t.planDrop}</button>
        </div>
      ) : (
        <div className={`text-[13px] font-medium ${isDark ? 'text-[#93D5A6]' : 'text-[#137333]'}`}>{statusText}</div>
      )}
    </div>
  );
}

// 首次切 yolo 的一次性确认卡（全局记忆）：语义 = "该模式下模型将对你的项目目录
// 全自动读写、可执行 shell，无逐步审批"；确认后全局记住、不再弹（与 VS Code 同款
// UI 层确认，后端不强制门控）。按钮样式复用方案审批卡的 cardBtnCls。
function NativeYoloConfirmCard({ theme, t, busy, onConfirm, onCancel }) {
  const isDark = theme === 'dark';
  const dialogRef = useRef(null);
  // 打开即聚焦卡片（键盘可达），Esc 视为取消——与 NativePlanCard 内联卡不同，
  // 这是一张全屏模态，必须挡住底层控件，故补 role=dialog/aria-modal/键盘交互。
  useEffect(() => {
    dialogRef.current?.focus();
    const onKey = (e) => {
      if (e.key === 'Escape' && !busy) {
        e.preventDefault();
        onCancel();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [busy, onCancel]);
  return (
    <div data-testid="native-yolo-confirm" className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <button
        type="button"
        aria-label={t.modeYoloConfirmCancel}
        className="absolute inset-0 cursor-default bg-black/30 backdrop-blur-[2px]"
        disabled={busy}
        onClick={onCancel}
      />
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="native-yolo-confirm-title"
        tabIndex={-1}
        className={`relative w-full max-w-[420px] rounded-2xl border p-4 shadow-xl backdrop-blur-xl outline-none ${
          isDark ? 'border-white/10 bg-[#202124]/95' : 'border-black/[0.08] bg-white/95'
        }`}>
        <div id="native-yolo-confirm-title" className={`text-[14px] font-semibold ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>
          {t.modeYoloConfirmTitle}
        </div>
        <div className={`mt-2 text-[13px] leading-relaxed ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>
          {t.modeYoloConfirmBody}
        </div>
        <div className="mt-2 text-[11px] text-gray-400">{t.modeYoloConfirmHint}</div>
        <div className="mt-4 flex items-center justify-end gap-2">
          <button
            type="button"
            data-testid="native-yolo-confirm-cancel"
            className={cardBtnCls()}
            disabled={busy}
            onClick={onCancel}
          >{t.modeYoloConfirmCancel}</button>
          <button
            type="button"
            data-testid="native-yolo-confirm-ok"
            className={cardBtnCls('primary')}
            disabled={busy}
            onClick={onConfirm}
          >{t.modeYoloConfirmOk}</button>
        </div>
      </div>
    </div>
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
  onOpenResource,
}) {
  if (item.type === 'reasoning') return <ReasoningItem item={item} now={now} copy={copy} />;
  if (item.type === 'tool_group') return <ToolGroup group={item} now={now} copy={copy} cv={cv} onOpenResource={onOpenResource} />;
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
      ? <ConversationMarkdown text={item.text} onOpenExternal={onOpenExternal} onOpenResource={onOpenResource}
          className="text-[13px] leading-6 text-gray-500 dark:text-gray-400" />
      : <ConversationMarkdown text={item.text} onOpenExternal={onOpenExternal} onOpenResource={onOpenResource} />;
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
  onOpenResource,
}) {
  const waitingPermission = turn.permissions.some(permission => !permission.resolved);
  const waitingInput = turn.elicitations.some(elicitation => !elicitation.resolved);
  const running = turn.status === 'running';
  const duration = copy.elapsed(elapsedMs(turn.startedAt, turn.completedAt, now));
  const assistantAvailable = assistantResponseAvailable(turn);
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
              responding={responding} onOpenExternal={onOpenExternal} onOpenResource={onOpenResource} />
          ))}
          {!running && (assistantAvailable || turn.completedAt || turn.error) && <AssistantMessageFooter>
            {assistantAvailable && (
              <AssistantMessageActions resolveText={() => assistantResponseText(turn)} copy={copy} />
            )}
            {(turn.completedAt || turn.error) && <>
              <StatusBadge status={turn.status} copy={copy} />
              <span className="text-[11px] text-gray-400">{duration}</span>
              {turn.usage && <span className="text-[11px] text-gray-400">{copy.contextUsage(Number(turn.usage.used || 0).toLocaleString(), Number(turn.usage.size || 0).toLocaleString())}</span>}
              {turn.error && <span className="text-[11px] text-red-500">{turn.error}</span>}
            </>}
          </AssistantMessageFooter>}
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
  operation,
  error,
  onInstall,
  onLogin,
  onOpenLogin,
  onSubmitLoginCode,
  onRefresh,
  resetKey,
  suppressAdvisoryUpgrade = false,
  copy,
}) {
  const [authorizationCode, setAuthorizationCode] = useState('');
  const [declinedUpgrade, setDeclinedUpgrade] = useState(false);
  useEffect(() => {
    setAuthorizationCode('');
  }, [status?.agent_id, status?.login_in_progress]);
  useEffect(() => {
    setDeclinedUpgrade(false);
  }, [resetKey, status?.agent_id, status?.installed, status?.latest_version]);
  const noticeMode = runtimeNoticeMode(status, declinedUpgrade || suppressAdvisoryUpgrade);
  if (noticeMode === 'checking') return <div className="text-[13px] text-gray-400">{copy.checking}</div>;
  const rawError = error || status.error;
  const visibleError = rawError
    ? (copy.showRawErrors ? rawError : copy.operationFailed)
    : '';
  if (noticeMode === 'bridge_unavailable') {
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
  if (noticeMode === 'install') {
    const agentName = status.agent_name || 'Agent';
    const action = status.install_action || 'manual';
    const isPackageManagerUpgrade = action === 'brew_upgrade' || action === 'npm_upgrade';
    const canAutoUpgrade = (status.update_available || status.update_required || isPackageManagerUpgrade) && action !== 'manual' && action !== 'none';
    const canDeferUpgrade = status.update_available && status.installed && !status.update_required;
    const installing = runtimeInstallInProgress(status, operation);
    const installHints = {
      official_script: copy.officialScriptHint(agentName),
    };
    const installButtons = {
      official_script: copy.confirmInstall,
    };
    const hint = isPackageManagerUpgrade
      ? copy.packageManagerUpgradeHint(status.install_source)
      : installHints[action] || setupHintText(copy, status.setup_hint) || copy.manualInstallHint(agentName);
    const busyLabel = copy.installing;
    return (
      <div className="rounded-2xl border border-blue-500/20 bg-blue-500/[0.05] p-4 flex items-center gap-3">
        <Terminal size={19} className="text-blue-500 shrink-0" />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold">
            {status.update_required
              ? copy.cliUpdateRequired(agentName, status.version, status.latest_version)
              : status.update_available
                ? copy.cliUpdateAvailable(agentName, status.version, status.latest_version)
              : status.version
                ? copy.cliOutdated(status.version, status.min_version)
                : copy.cliMissing(agentName)}
          </div>
          <div className="mt-0.5 text-[12px] text-gray-500">{hint}</div>
          {visibleError && <div className="mt-1 text-[11px] text-red-500">{visibleError}</div>}
        </div>
        {canAutoUpgrade ? (
          <div className="flex shrink-0 items-center gap-2">
            {canDeferUpgrade && (
              <button onClick={() => setDeclinedUpgrade(true)} disabled={working || installing} className="px-3 py-1.5 rounded-xl border border-blue-500/20 text-[12px] font-medium disabled:opacity-50">
                {copy.declineUpgrade}
              </button>
            )}
            <button onClick={() => onInstall()} disabled={working || installing} className="px-3 py-1.5 rounded-xl bg-blue-600 text-white text-[12px] font-medium disabled:opacity-50 inline-flex items-center gap-1.5">
              {installing && <RefreshCw size={12} className="animate-spin" />}
              {installing ? busyLabel : copy.upgrade}
            </button>
          </div>
        ) : installButtons[action] ? (
          <button onClick={() => onInstall()} disabled={working || installing} className="px-3 py-1.5 rounded-xl bg-blue-600 text-white text-[12px] font-medium disabled:opacity-50 inline-flex items-center gap-1.5">
            {installing && <RefreshCw size={12} className="animate-spin" />}
            {installing ? busyLabel : installButtons[action]}
          </button>
        ) : (
          <button onClick={onRefresh} className="px-3 py-1.5 rounded-xl border border-blue-500/20 text-[12px] font-medium">
            {copy.recheck}
          </button>
        )}
      </div>
    );
  }
  if (noticeMode === 'login') {
    const waitingForLogin = runtimeLoginInProgress(status, operation);
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
          {waitingForLogin ? copy.waitAuth : copy.authorize}
        </button>
      </div>
    );
  }
  if (noticeMode === 'error') return <div className="rounded-xl bg-red-500/8 text-red-600 dark:text-red-300 px-3 py-2 text-[12px]">{visibleError}</div>;
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
  onManageProviders,
  onDismiss,
  copy,
  providerCopy,
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
          {onManageProviders && providerCopy && (
            <button
              type="button"
              data-testid="acp-failure-manage-providers"
              onClick={onManageProviders}
              disabled={working}
              className="rounded-xl border border-red-500/20 px-3 py-1.5 text-[12px] font-medium text-red-600 disabled:opacity-50 dark:text-red-300"
            >
              {providerCopy.faultManage}
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
  onOpenSettingsSection,
  bs = null,
  onGotoTools,
  onGotoModelSettings,
  onGotoSettings,
  fixedSession = false,
}) {
  const codexCopy = t.uiCodex;
  const [agents, setAgents] = useState([]);
  const [draftAgentId, setDraftAgentId] = useState(loadAgentSelection() || 'pinvou');
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
  const [subagentPanel, setSubagentPanel] = useState(null);
  const [workspaceChangeCount, setWorkspaceChangeCount] = useState(0);
  const [now, setNow] = useState(Date.now());
  const useUnifiedConversationUi = unifiedConversationUiEnabled();
  const [configApplying, setConfigApplying] = useState('');
  const [working, setWorking] = useState(false);
  const [runtimeOperations, setRuntimeOperations] = useState({});
  const [runtimeErrors, setRuntimeErrors] = useState({});
  const [error, setError] = useState('');
  const showError = (nextError) => {
    console.error('Codex operation failed:', nextError);
    setError(codexCopy.showRawErrors ? String(nextError) : codexCopy.operationFailed);
  };
  const [responding, setResponding] = useState(false);
  const [commandOpen, setCommandOpen] = useState(false);
  const [workspaceMenuOpen, setWorkspaceMenuOpen] = useState(false);
  const [accountMenuOpen, setAccountMenuOpen] = useState(false);
  const [memoryOpen, setMemoryOpen] = useState(false);
  const [dismissedFailureKey, setDismissedFailureKey] = useState('');
  const [draftWorkspacePath, setDraftWorkspacePath] = useState(null);
  const [recentWorkspaces, setRecentWorkspaces] = useState(loadRecentWorkspaces);
  const [draftControlsCache, setDraftControlsCache] = useState(loadDraftControlsCache);
  // 草稿态（会话未创建）下用户预选的配置：{ [agentId]: { model?, mode?, configs: { [id]: value } } }
  const [draftConfigSelections, setDraftConfigSelections] = useState({});
  const [showScrollBottom, setShowScrollBottom] = useState(false);
  const scroller = useRef(null);
  const rightPanelScrollRef = useRef(null);
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
  const activeSession = useMemo(
    () => sessions.find(session => session.id === activeId) || null,
    [sessions, activeId],
  );
  const activeAgentId = activeSession?.agent_id || draftAgentId;
  // 原生（品悟 Engine）代码会话：发消息走 chat 命令 + chat:* 事件，会话状态按
  // session 缓存在 lane Map 里（后台会话的 turn 也能继续推进，切回不丢流式内容）。
  const isNativeAgent = activeAgentId === 'pinvou';
  const nativeLanesRef = useRef(new Map());
  const [nativeLaneTick, setNativeLaneTick] = useState(0);
  const nativeSessionIdsRef = useRef(new Set());
  useEffect(() => {
    const ids = new Set(
      sessions
        .filter(session => session && session.agent_id === 'pinvou')
        .map(session => session.id),
    );
    nativeSessionIdsRef.current = ids;
    // 清理已删除会话的 lane，避免 nativeLanesRef 无界增长（只 set 不 delete）。
    for (const id of nativeLanesRef.current.keys()) {
      if (!ids.has(id)) nativeLanesRef.current.delete(id);
    }
  }, [sessions]);

  // 原生车道才加载知识库集合与 embedding 安装态；embedding 明确未装时选择器禁用。
  // 集合列表与安装态由 ComposerKbSelector 内部经 bridge.knowledge（kb_collection_list /
  // kb_model_status，全局只读、不带会话）自行加载，代码页不再重复拉取。
  function getNativeLane(sessionId) {
    let lane = nativeLanesRef.current.get(sessionId);
    if (!lane) {
      lane = createNativeLane();
      nativeLanesRef.current.set(sessionId, lane);
    }
    return lane;
  }
  const activeNativeLane = isNativeAgent && activeId
    ? nativeLanesRef.current.get(activeId) || null
    : null;
  // 原生车道的用量/压缩/记忆展示数据：直接读 lane（可变对象，靠 nativeLaneTick 重渲染）。
  // chat:usage 不带 context 上限（tokens.max 恒 0，docs/code-native-agent.md §9 登记的
  // 已知限制），用量 chip 按降级处理：只显示已用 token，不显示上限与百分比。
  const nativeTokensInput = isNativeAgent && activeNativeLane ? Number(activeNativeLane.tokens.input || 0) : 0;
  const nativeCompacting = Boolean(isNativeAgent && activeNativeLane && activeNativeLane.compacting);
  const nativeMemoryItems = isNativeAgent && activeNativeLane && activeNativeLane.memory
    ? activeNativeLane.memory.items
    : [];
  // 原生车道底栏控件（模型/工具/知识库/模式/多智能体）的会话态：按 activeId 经 invoke 自查，
  // 不读 bridge 聊天 active 绑定（bs.currentSessionModelId/modeState/mountedCollection
  // 都绑聊天 active）。草稿态暂存 nativeDraftControls，建会话成功后再应用。
  // mode 由后端 get_mode_state 驱动（code 会话首次默认 Plan），不写死初值。
  const [nativeControls, setNativeControls] = useState({
    modelId: null,
    mountedId: null,
    mode: CODE_MODE_FALLBACK,
    multiAgent: false,
    multiAgentAvailable: false,
  });
  const [nativeDraftControls, setNativeDraftControls] = useState({});
  // nativeControls 的会话归属：切会话后、refresh 返回前不展示上一会话的控件值。
  const nativeControlsSessionRef = useRef(null);
  // refreshNativeControls 请求序号：快速切会话时多个 get_* invoke 并发在途，
  // 后发起的请求应胜出。没有它，先发起的慢响应会晚返回并把控件值/归属 ref 覆盖
  // 成旧会话——mode chip 随即显示全局 fallback 而非新会话实测值（串台/陈旧覆盖，
  // 与聊天页 modeState epoch 修复同款竞态）。
  // code 会话权限模式全局偏好（{ last_mode, yolo_confirmed }，null=未拉到）：
  // 驱动草稿态/刷新途中的默认 mode 展示，以及首次切 yolo 的一次性确认门。
  const [codePermPrefs, setCodePermPrefs] = useState(null);
  // 待确认的 yolo 切换请求（{ draft, chipBusy }）；非 null 时渲染确认卡。
  const [pendingYoloSwitch, setPendingYoloSwitch] = useState(null);
  const [yoloConfirmBusy, setYoloConfirmBusy] = useState(false);
  // 知识库集合列表与 embedding 安装态由 ComposerKbSelector 内部经 bridge.knowledge
  // （kb_collection_list / kb_model_status，全局只读、不带会话）自行加载，代码页
  // 不再重复拉取（PR #214 统一底栏控件时移除 nativeKb* 本地变量）。
  const nativeProjection = useMemo(
    () => (isNativeAgent ? projectNativeLane(activeNativeLane, activeId) : null),
    // nativeLaneTick 是 lane 内容变化的版本号（lane 本体是可变对象，靠 tick 触发重投影）。
    [isNativeAgent, activeNativeLane, activeId, nativeLaneTick],
  );
  const visibleTurns = isNativeAgent
    ? (nativeProjection ? nativeProjection.turns : [])
    : projection.turns;
  const busy = isNativeAgent
    ? Boolean(activeNativeLane && activeNativeLane.busy)
    : projection.turns.some(turn => turn.status === 'running');
  const activeConversationTurn = [...visibleTurns]
    .reverse()
    .find(turn => turn.status === 'running') || null;
  // 原生车道底栏控件的展示值（归属保护：refresh 返回前按默认/暂存显示；
  // 默认 = 全局 code_last_mode，从未用过 code 模式 → Plan 只读）。
  const nativeModeValue = resolveNativeModeValue({
    activeId,
    controlsSessionId: nativeControlsSessionRef.current,
    controlsMode: nativeControls.mode,
    draftMode: nativeDraftControls.mode,
    prefs: codePermPrefs,
  });
  const nativeModelChoices = visibleUserModels((bs && bs.savedModels) || [])
    .map(model => ({ value: model.id, name: selectorMainLabel(model, t) || model.id }));
  const nativeSessionModelId = activeId
    ? (nativeControlsSessionRef.current === activeId ? nativeControls.modelId : null)
    : (nativeDraftControls.modelId || null);
  const nativeMountedId = activeId
    ? (nativeControlsSessionRef.current === activeId ? nativeControls.mountedId : null)
    : (nativeDraftControls.mountedId ?? null);
  const nativeMultiAgentSelected = activeId
    ? (nativeControlsSessionRef.current === activeId && Boolean(nativeControls.multiAgent))
    : Boolean(nativeDraftControls.multiAgent);
  // Existing sessions use the backend SessionPolicy result. A Pinvou draft is
  // known to become a native Code session, so it may stage the same control
  // before a session id exists.
  const nativeMultiAgentAvailable = activeId
    ? (nativeControlsSessionRef.current === activeId && Boolean(nativeControls.multiAgentAvailable))
    : isNativeAgent;
  const nativeMultiAgentEnabled = nativeMultiAgentAvailable && nativeMultiAgentSelected;
  const activeAgentName = activeSession?.agent_name
    || agents.find(agent => agent.agent_id === activeAgentId)?.agent_name
    || (activeAgentId === 'pinvou' ? '品悟' : activeAgentId === 'claude' ? 'Claude Code' : activeAgentId === 'kimi' ? 'Kimi' : 'Codex');
  const activeAgentIdRef = useRef(activeAgentId);
  activeAgentIdRef.current = activeAgentId;
  const rememberScrollBeforeRightPanelChange = useCallback(() => {
    rightPanelScrollRef.current = captureConversationScrollPosition(
      scroller.current,
      autoScrollRef.current,
    );
  }, []);
  const closeSubagentPanel = useCallback(() => {
    rememberScrollBeforeRightPanelChange();
    setSubagentPanel(null);
  }, [rememberScrollBeforeRightPanelChange]);
  const toggleWorkspacePanel = useCallback(() => {
    rememberScrollBeforeRightPanelChange();
    setSubagentPanel(null);
    setWorkspaceOpen(value => !value);
  }, [rememberScrollBeforeRightPanelChange]);
  const closeWorkspacePanel = useCallback(() => {
    rememberScrollBeforeRightPanelChange();
    setWorkspaceOpen(false);
  }, [rememberScrollBeforeRightPanelChange]);
  useLayoutEffect(() => {
    const snapshot = rightPanelScrollRef.current;
    if (!snapshot) return;
    rightPanelScrollRef.current = null;
    const element = scroller.current;
    if (!element) return;
    restoreConversationScrollPosition(element, snapshot);
    lastScrollTopRef.current = element.scrollTop;
    if (snapshot.stickToBottom) {
      autoScrollRef.current = true;
      setShowScrollBottom(false);
    }
  }, [subagentPanel, workspaceOpen]);
  useEffect(() => {
    setSubagentPanel(null);
  }, [activeId]);
  useEffect(() => {
    if (typeof window === 'undefined' || !isNativeAgent) return undefined;
    const onOpen = (event) => {
      const detail = event && event.detail;
      const sessionId = detail && detail.sessionId;
      if (!detail?.agentId || !activeIdRef.current) return;
      if (sessionId && sessionId !== activeIdRef.current) return;
      rememberScrollBeforeRightPanelChange();
      setWorkspaceOpen(false);
      setSubagentPanel(current => ({
        agentId: detail.agentId,
        selectionRequestId: (current?.selectionRequestId || 0) + 1,
      }));
    };
    window.addEventListener('pinvou:open-subagent', onOpen);
    return () => window.removeEventListener('pinvou:open-subagent', onOpen);
  }, [isNativeAgent, rememberScrollBeforeRightPanelChange]);
  const activeStatus = status?.agent_id === activeAgentId ? status : null;
  const activeRuntimeOperation = runtimeOperationFor(runtimeOperations, activeAgentId);
  const activeRuntimeBusy = Boolean(activeRuntimeOperation);
  const activeRuntimeError = runtimeErrors[activeAgentId] || '';
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
  const sessionReady = isNativeAgent
    ? (!activeId || Boolean(activeNativeLane && activeNativeLane.hydrated))
    : (!activeId || (sessionInfoSessionId === activeId && Boolean(sessionInfo)));
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

  /// 拉取原生会话的模型/知识库/模式状态（全部 per-session 命令，显式 sessionId）。
  async function refreshNativeControls(sessionId) {
    const [modelId, mountedId, modeState] = await Promise.all([
      invoke('get_session_model_id', { sessionId }).catch(() => null),
      invoke('session_mounted_collection', { sessionId }).catch(() => null),
      invoke('get_mode_state', { sessionId }).catch(() => null),
    ]);
    const controls = {
      modelId: modelId || null,
      mountedId: mountedId ?? null,
      // 读取失败兜底走全局默认（首次使用 → Plan 只读），不回退写死 yolo。
      mode: (modeState && modeState.mode) || nativeModeFallback(codePermPrefs),
      multiAgent: Boolean(modeState && modeState.multi_agent),
      multiAgentAvailable: Boolean(modeState && modeState.multi_agent_available),
    };
    // 请求期间可能切换会话；旧响应不得覆盖新会话的模型/模式/多智能体展示。
    if (sessionId !== activeIdRef.current) return controls;
    setNativeControls(controls);
    nativeControlsSessionRef.current = sessionId;
    return controls;
  }

  /// 拉取全局 code 权限偏好（last_mode / yolo_confirmed）：草稿态默认 mode
  /// 与 yolo 一次性确认门的事实源。启动、进/出会话与每次切换后刷新。
  async function refreshCodePermPrefs() {
    const prefs = await invoke('get_code_permission_prefs').catch(() => null);
    if (prefs) setCodePermPrefs(prefs);
    return prefs;
  }

  // 启动时拉一次全局 code 权限偏好（草稿态默认 mode + yolo 确认门）。
  useEffect(() => {
    refreshCodePermPrefs();
    // 仅挂载拉取一次；后续由切换/确认路径就地刷新。
  }, []);

  /// 草稿态暂存的控件选择在新会话上应用；失败报错不静默（逐个应用，多智能体最后）。
  /// 任一步失败即整体失败：清空暂存并上抛，由 sendNative 外层 catch 兜住（会话已创建，
  /// 保留半份暂存会在下次创建会话时把过期的部分选择悄悄应用，形成孤儿暂存）。
  async function applyNativeDraftControls(sessionId) {
    const staged = nativeDraftControls;
    const hasMultiAgentSelection = Object.prototype.hasOwnProperty.call(staged, 'multiAgent');
    const hasStaged = staged.modelId || staged.mountedId != null || staged.mode || hasMultiAgentSelection;
    if (!hasStaged) return;
    try {
      if (staged.modelId) {
        await invoke('set_session_model', { sessionId, modelId: staged.modelId });
      }
      if (staged.mountedId != null) {
        await invoke('session_mount_collection', { sessionId, collectionId: staged.mountedId });
      }
      // 暂存 mode 两个方向都要应用：默认可能是 plan（全局首次），也可能是 yolo
      // （last_mode 记忆）；只设单方向会让反方向暂存静默失效。
      if (staged.mode === 'plan') {
        await invoke('set_plan_mode_next', { sessionId });
      } else if (staged.mode === 'yolo') {
        await invoke('exit_plan_to_yolo', { sessionId });
      }
      if (hasMultiAgentSelection) {
        await invoke('set_multi_agent_mode', {
          sessionId,
          enabled: Boolean(staged.multiAgent),
        });
      }
      setNativeDraftControls({});
    } catch (err) {
      // 会话已经创建且部分配置可能已生效：清空暂存避免未来复用过期选择，
      // 错误继续上抛，由 sendNative 的 catch 提示用户并恢复输入框文本。
      setNativeDraftControls({});
      throw err;
    }
    // 暂存落地后必须再刷新一次实测值：会话物化时 effect 触发的
    // refreshNativeControls 通常在暂存应用链（mode 排最后）落地前就带着
    // 后端默认值返回了，chip 停在旧 mode（如 plan），要重进对话才刷新。
    // 此处以应用后的实测值收口；refreshNativeControls 的请求序号保证在途
    // 旧读返回时被丢弃，不会反向覆盖。
    await refreshNativeControls(sessionId);
  }

  /// 切模型：set_session_model 会 evict 该会话 engine，lane busy 时由控件禁用兜底。
  async function switchNativeModel(sessionId, modelId) {
    if (!sessionId) {
      setNativeDraftControls(current => ({ ...current, modelId }));
      return;
    }
    setError('');
    try {
      await invoke('set_session_model', { sessionId, modelId });
      await refreshNativeControls(sessionId);
    } catch (err) { showError(err); }
  }

  async function switchNativeMultiAgent(enabled) {
    if (!nativeMultiAgentAvailable) return;
    if (!activeId) {
      setNativeDraftControls(current => ({ ...current, multiAgent: Boolean(enabled) }));
      return;
    }
    if (busy || working) return;
    const targetSessionId = activeId;
    const previous = nativeMultiAgentEnabled;
    setError('');
    setWorking(true);
    setConfigApplying('multiagent');
    setNativeControls(current => ({ ...current, multiAgent: Boolean(enabled) }));
    try {
      await invoke('set_multi_agent_mode', { sessionId: targetSessionId, enabled: Boolean(enabled) });
      await refreshNativeControls(targetSessionId);
    } catch (err) {
      // 请求期间可能已切换会话：只有仍是目标会话时才回滚旧值，否则交给
      // 新会话自身的 refresh 覆盖，避免把上一会话的值串写进当前会话。
      if (targetSessionId === activeIdRef.current) {
        setNativeControls(current => ({ ...current, multiAgent: previous }));
      }
      showError(err);
    } finally {
      setWorking(false);
      setConfigApplying('');
    }
  }

  async function mountNativeKb(collectionId) {
    if (!activeId) {
      setNativeDraftControls(current => ({ ...current, mountedId: collectionId }));
      return;
    }
    setError('');
    try {
      await invoke('session_mount_collection', { sessionId: activeId, collectionId });
      await refreshNativeControls(activeId);
    } catch (err) { showError(err); }
  }

  async function unmountNativeKb() {
    if (!activeId) {
      setNativeDraftControls(current => ({ ...current, mountedId: null }));
      return;
    }
    setError('');
    try {
      await invoke('session_unmount_collection', { sessionId: activeId });
      await refreshNativeControls(activeId);
    } catch (err) { showError(err); }
  }

  /// Plan↔Yolo：对齐聊天页语义——切回 Yolo 时若 turn 在跑先取消
  /// （用代码车道已有的 cancel_generation 显式 sessionId 调用，不经 bridge）。
  ///
  /// 首次切 yolo 的一次性确认门（全局记忆，产品已拍板）：未确认先弹卡，
  /// 【确认】调 confirm_code_yolo 写入全局标志后按原路径切换；【取消】留在
  /// 当前 mode。确认是 UI 层语义，后端 exit_plan_to_yolo 不强制门控。
  async function switchNativeMode(target, { isPlan, busy: chipBusy } = {}) {
    if (target === 'yolo' && isPlan) {
      const prefs = await refreshCodePermPrefs();
      if (needsYoloConfirmation(prefs)) {
        setPendingYoloSwitch({ draft: !activeId, chipBusy: Boolean(chipBusy) });
        return;
      }
    }
    await performNativeModeSwitch(target, { isPlan, chipBusy });
  }

  /// 草稿态暂存 mode 选择：本地暂存（新建会话时应用）+ 刷新 code lane 全局
  /// 默认（三分 lane 语义：草稿切换写全局；已生成会话的切换不碰全局）。
  function stageDraftMode(target) {
    setNativeDraftControls(current => ({ ...current, mode: target }));
    invoke('set_mode_default', { lane: 'code', mode: target })
      .then(() => refreshCodePermPrefs())
      .catch(err => showError(err));
  }

  /// mode chip 切换的实际执行路径（不含 yolo 确认门）。
  async function performNativeModeSwitch(target, { isPlan, chipBusy } = {}) {
    if (!activeId) {
      stageDraftMode(target);
      return;
    }
    setError('');
    try {
      if (target === 'plan' && !isPlan) {
        await invoke('set_plan_mode_next', { sessionId: activeId });
      } else if (target === 'yolo' && isPlan) {
        if (chipBusy) await invoke('cancel_generation', { sessionId: activeId });
        await invoke('exit_plan_to_yolo', { sessionId: activeId });
      }
      await refreshNativeControls(activeId);
    } catch (err) { showError(err); }
  }

  /// 确认卡【确认】：写全局 yolo 确认标志，成功后继续被中断的切换。
  async function confirmPendingYoloSwitch() {
    const pending = pendingYoloSwitch;
    if (!pending || yoloConfirmBusy) return;
    setYoloConfirmBusy(true);
    try {
      const prefs = await invoke('confirm_code_yolo');
      if (prefs) setCodePermPrefs(prefs);
      setPendingYoloSwitch(null);
      if (pending.draft) {
        stageDraftMode('yolo');
      } else {
        await performNativeModeSwitch('yolo', { isPlan: true, chipBusy: pending.chipBusy });
      }
    } catch (err) {
      showError(err);
    } finally {
      setYoloConfirmBusy(false);
    }
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

  // 每个 Agent 的 Provider 视图（会话级覆盖下拉与故障引导共用）。
  const [providersViews, setProvidersViews] = useState({});
  async function refreshProviders(agentId = activeAgentId) {
    if (!agentId) return null;
    try {
      const next = await invoke('list_acp_providers', { agent: agentId });
      setProvidersViews(current => ({ ...current, [agentId]: next }));
      return next;
    } catch {
      return null;
    }
  }
  useEffect(() => {
    if (activeAgentId) refreshProviders(activeAgentId);
    // activeAgentId 变化时刷新一次即可；切换/回退后由调用方显式刷新。
  }, [activeAgentId]);
  const activeProvidersView = providersViews[activeAgentId] || null;
  // Kimi 中转激活时（会话覆盖 > 全局当前 Provider），模型列表只保留受管
  // pv-* 条目：writer 按设计保留官方登录的模型表，CLI 会一并上报，全列出
  // 会让用户误以为还在走官方。
  const kimiRelayActive = activeAgentId === 'kimi' && Boolean(
    (sessionControlsInfo && sessionControlsInfo.provider)
    || (activeProvidersView && activeProvidersView.currentProviderId)
  );
  // Codex 中转激活时同理：CLI 的 model/list 会暴露官方内置模型（gpt 系列），
  // 中转商并不提供它们，用户选中会 404——只保留当前 Provider 的模型
  // （Codex 的模型选项 id 是模型名，无 pv- 前缀，按名字匹配）。
  const relayProviderId = (sessionControlsInfo && sessionControlsInfo.provider)
    || (activeProvidersView && activeProvidersView.currentProviderId)
    || null;
  const relayProviderRecord = relayProviderId
    ? (((activeProvidersView && activeProvidersView.providers) || [])
        .find(provider => provider.id === relayProviderId)) || null
    : null;
  const codexRelayModel = activeAgentId === 'codex' && relayProviderRecord && relayProviderRecord.model
    ? relayProviderRecord.model
    : null;
  // Codex 中转激活但 Provider 未配置模型：官方模型全量展示会让用户选中后走
  // 中转 404（复审低危 1）——列表置空并提示先回设置填写模型。
  const codexRelayNoModel = activeAgentId === 'codex' && Boolean(relayProviderRecord) && !codexRelayModel;
  const visibleFallbackModels = kimiRelayActive
    ? controls.fallbackModels.filter(model => String(model.id).startsWith('pv-'))
    : codexRelayModel
      ? controls.fallbackModels.filter(model => String(model.id) === codexRelayModel)
      : codexRelayNoModel
        ? []
        : controls.fallbackModels;
  const modelConfigChoices = option => {
    const choices = configChoices(option);
    if (kimiRelayActive) return choices.filter(choice => String(choice.value).startsWith('pv-'));
    if (codexRelayModel) return choices.filter(choice => String(choice.value) === codexRelayModel);
    if (codexRelayNoModel) return [];
    return choices;
  };
  const sessionProviderChoices = [
    { value: '__official__', name: (t.uiAcpProviders || {}).sessionOfficial || 'Official' },
    ...((activeProvidersView && activeProvidersView.providers) || [])
      .filter(provider => provider.hasCredential)
      .map(provider => ({ value: provider.id, name: provider.name })),
  ];
  const sessionProviderValue = (sessionControlsInfo && sessionControlsInfo.provider) || '__official__';
  async function changeSessionProvider(value) {
    const targetId = activeId;
    if (!targetId) return;
    setConfigApplying('provider');
    try {
      const next = await invoke('set_codex_acp_session_provider', {
        sessionId: targetId,
        providerId: value === '__official__' ? null : value,
      });
      applySessionInfo(next);
      refreshProviders(activeAgentId);
    } catch (err) {
      showError(err);
    } finally {
      setConfigApplying('');
    }
  }

  async function refreshStatus(agentId = activeAgentId, recheck = false) {
    // recheck=true 强制后端忽略缓存重新探测（「重新检测」按钮）；轮询不传，保持读缓存。
    const next = await invoke('get_acp_agent_status', recheck ? { agentId, recheck: true } : { agentId });
    if (next?.agent_id === activeAgentIdRef.current) setStatus(next);
    return next;
  }

  function selectDraftAgent(agentId) {
    if (activeId || !agentId) return;
    setDraftAgentId(agentId);
    saveAgentSelection(agentId);
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
      // 原生（品悟）会话：历史与 turn timeline 来自 SavedSession / timing_events，
      // 不走 ACP 的 timeline / pending / session_info 命令。
      if (nativeSessionIdsRef.current.has(id)) {
        const [saved, sessionTimeline] = await Promise.all([
          invoke('load_session', { id, setActive: false }),
          invoke('get_session_timeline', { sessionId: id }).catch(() => []),
        ]);
        if (sessionLoadRequestRef.current !== requestId) return null;
        const lane = getNativeLane(id);
        hydrateNativeLane(lane, saved, sessionTimeline || []);
        // lane 随组件卸载销毁，chat:user_input_required 不重发：经后端 pending
        // 登记还原挂起的确认卡（applyNativeChatEvent 按 toolCallId 幂等去重），
        // 并顺带恢复 turn 进行中的 busy 展示。
        const pendingState = await invoke('get_pending_user_inputs', { sessionId: id })
          .catch(() => null);
        if (sessionLoadRequestRef.current !== requestId) return null;
        if (pendingState) {
          (pendingState.pending || []).forEach(request => {
            applyNativeChatEvent(lane, 'chat:user_input_required', {
              session_id: id,
              id: request.id,
              questions: request.questions,
            });
          });
          if (pendingState.busy && !lane.busy) {
            lane.busy = true;
            lane.thinking = { active: true, startedAt: Date.now(), phase: 'thinking', toolName: null };
          }
        }
        await refreshNativeControls(id);
        if (sessionLoadRequestRef.current !== requestId) return null;
        setNativeLaneTick(tick => tick + 1);
        return null;
      }
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
    // loadSession 用 nativeSessionIdsRef 判定分流；新会话先登记，避免它读到旧 prop。
    if (draftAgentId === 'pinvou') nativeSessionIdsRef.current.add(metadata.id);
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
    // 选定项目工作区即默认展开工作区面板（无会话也可浏览文件）；临时会话无路径可浏览。
    setWorkspaceOpen(Boolean(workspacePath));
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

  // ── 语音输入（与 ChatView 同款：bridge.voice 一次录音 → 本地 ASR → 写回 draft）。
  // 代码车道不物化聊天会话，语音状态仍由 bridge 全局管理（bs.voiceInput），写回走代码页 draft。
  const nativeVoiceInput = (bs && bs.voiceInput) || { status: 'idle' };
  const nativeVoiceActive = nativeVoiceInput.status === 'requesting_permission'
    || nativeVoiceInput.status === 'recording'
    || nativeVoiceInput.status === 'transcribing';
  const nativeVoiceRecording = nativeVoiceInput.status === 'recording';
  const nativeVoiceBusy = nativeVoiceInput.status === 'transcribing';
  const nativeVoiceDisabled = !bridge.available || nativeVoiceBusy;
  const nativeVoiceCanInstallAsr = can('localModelSetup') && can('dependencyInstall');
  const nativeVoiceLabel = nativeVoiceInput.status === 'recording'
    ? t.voiceStop
    : nativeVoiceInput.status === 'failed'
      ? t.voiceRetry
      : nativeVoiceInput.status === 'requesting_permission'
        ? t.voiceCancel
        : nativeVoiceInput.status === 'transcribing'
          ? t.voiceTranscribing
          : t.voiceStart;
  function handleNativeVoiceClick() {
    if (!bridge.available) return;
    if (nativeVoiceInput.status === 'requesting_permission') {
      bridge.voice.cancelVoiceInput();
      return;
    }
    if (nativeVoiceBusy) return;
    bridge.voice.startVoiceInput(draft, (text) => setDraft(prev => bridge.voice.appendVoiceText(prev, text)));
  }
  function handleNativeVoiceCancel() {
    if (bridge.available) bridge.voice.cancelVoiceInput();
  }
  function handleNativeVoiceClose() {
    if (bridge.available) bridge.voice.clearVoiceInput();
  }

  // 离开代码页（切模式/视图，组件卸载）时可靠取消进行中的语音输入：
  // bridge.voice 的写回守卫只绑定聊天侧 activeSessionId，代码页不物化聊天会话，
  // 若不取消，转写结果可能写回已卸载组件（草稿态 null→null 时守卫还会放行并
  // 显示「已完成」，但文本已丢失）。卸载前取消让「录音中切走」变成显式取消。
  const nativeVoiceInputRef = useRef(nativeVoiceInput);
  nativeVoiceInputRef.current = nativeVoiceInput;
  useEffect(() => {
    return () => {
      const voice = nativeVoiceInputRef.current;
      if (voice && (voice.status === 'requesting_permission'
        || voice.status === 'recording'
        || voice.status === 'transcribing')
        && bridge.available) {
        bridge.voice.cancelVoiceInput();
      }
    };
  }, []);

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

  // 原生（品悟）会话的 engine 事件：按 session 推进对应 lane，仅当前会话 bump 渲染；
  // turn 边界顺手刷新会话列表（标题/时间戳），与 acp:event 的 turn_completed 处理对齐。
  useEffect(() => {
    let disposed = false;
    let unlisteners = [];
    Promise.all(NATIVE_CHAT_EVENTS.map(name => listenTauri(name, message => {
      const payload = (message && message.payload) || {};
      const sessionId = payload.session_id;
      if (!sessionId || !nativeSessionIdsRef.current.has(sessionId)) return;
      const lane = getNativeLane(sessionId);
      const changed = applyNativeChatEvent(lane, name, payload);
      if (name === 'chat:turn_started' || name === 'chat:done') {
        refreshSessions().catch(() => {});
      }
      if (changed && sessionId === activeIdRef.current) {
        setNativeLaneTick(tick => tick + 1);
      }
    }))).then(fns => {
      if (disposed) fns.forEach(fn => fn());
      else unlisteners = fns;
    }).catch(error => console.warn('[codex] native chat events unavailable', error));
    return () => {
      disposed = true;
      unlisteners.forEach(fn => fn());
    };
  }, []);

  useEffect(() => {
    // 原生（品悟）会话没有 ACP 状态机，跳过 get_acp_agent_status（后端会拒绝非 ACP agent）。
    if (activeAgentId === 'pinvou') {
      setStatus(null);
      return;
    }
    // 用户主动切换 Agent 后必须绕过进程内探测缓存，立即反映 App 外的安装/卸载。
    refreshStatus(activeAgentId, true).catch(showError);
  }, [activeAgentId]);

  useEffect(() => {
    const latest = events[events.length - 1];
    if (!isAcpAuthenticationFailure(latest)) return;
    refreshStatus(activeAgentId).catch(() => {});
  }, [events.length, activeAgentId]);

  // 一次性模型探针：切换/删除 Provider（或恢复官方）后设置页会写探针标记。
  // 草稿态（!activeId）本来不连接 ACP，这里破例主动连接一次，用新 Provider
  // 的真实 session/new 上报覆盖 reseed 的占位快照，之后恢复懒加载。标记先清
  // 再探（一次性、防重入）；失败静默，保留占位快照不影响使用。
  useEffect(() => {
    if (activeId || isNativeAgent) return undefined;
    if (!activeStatus?.installed || !activeStatus?.authenticated) return undefined;
    if (!consumeAcpModelsProbePending(draftAgentId)) return undefined;
    let alive = true;
    invoke('probe_acp_agent_models', { agent: draftAgentId })
      .then(info => {
        if (!alive || !info) return;
        const snapshot = rememberDraftControls(draftAgentId, info);
        if (snapshot) {
          setDraftControlsCache(current => ({ ...current, [draftAgentId]: snapshot }));
        }
      })
      .catch(() => {});
    return () => { alive = false; };
  }, [activeId, isNativeAgent, draftAgentId, activeStatus?.installed, activeStatus?.authenticated]);

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

  // 切会话/回草稿时关掉记忆弹层（徽标内容按新会话 lane 自动切换）。
  useEffect(() => {
    setMemoryOpen(false);
  }, [activeId]);

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
  }, [events.length, visibleTurns.length, nativeLaneTick]);

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

  function beginRuntimeOperation(agentId, operation) {
    setRuntimeOperations(current => ({ ...current, [agentId]: operation }));
    setRuntimeErrors(current => ({ ...current, [agentId]: '' }));
  }

  function finishRuntimeOperation(agentId, operation) {
    setRuntimeOperations(current => {
      if (current[agentId] !== operation) return current;
      const next = { ...current };
      delete next[agentId];
      return next;
    });
  }

  function showRuntimeError(agentId, nextError) {
    console.error(`${agentId} runtime operation failed:`, nextError);
    const message = codexCopy.showRawErrors ? String(nextError) : codexCopy.operationFailed;
    setRuntimeErrors(current => ({ ...current, [agentId]: message }));
  }

  async function install(actionOverride = null) {
    const agentId = activeAgentId;
    beginRuntimeOperation(agentId, 'install');
    setError('');
    const poll = window.setInterval(() => refreshStatus(agentId).catch(() => {}), 500);
    try {
      const payload = { agent: agentId };
      if (typeof actionOverride === 'string' && actionOverride) payload.action = actionOverride;
      const next = await invoke('install_acp_agent', payload);
      if (next?.agent_id === activeAgentIdRef.current) setStatus(next);
    }
    catch (err) { showRuntimeError(agentId, err); }
    finally {
      window.clearInterval(poll);
      await refreshStatus(agentId).catch(() => {});
      finishRuntimeOperation(agentId, 'install');
    }
  }

  async function login() {
    const agentId = activeAgentId;
    beginRuntimeOperation(agentId, 'login');
    setError('');
    try {
      const next = await invoke('login_acp_agent', { agentId });
      if (next?.agent_id === activeAgentIdRef.current) setStatus(next);
    }
    catch (err) { showRuntimeError(agentId, err); }
    finally { finishRuntimeOperation(agentId, 'login'); }
  }

  async function switchAccount() {
    const agentId = activeAgentId;
    setAccountMenuOpen(false);
    if (serviceFailure?.key) setDismissedFailureKey(serviceFailure.key);
    beginRuntimeOperation(agentId, 'switch-account');
    setError('');
    try {
      const next = await invoke('switch_acp_agent_account', { agentId });
      if (next?.agent_id === activeAgentIdRef.current) setStatus(next);
    } catch (err) {
      showRuntimeError(agentId, err);
    } finally {
      finishRuntimeOperation(agentId, 'switch-account');
    }
  }

  async function openLogin() {
    const agentId = activeAgentId;
    setRuntimeErrors(current => ({ ...current, [agentId]: '' }));
    try { await invoke('open_acp_agent_login_url', { agentId }); }
    catch (err) { showRuntimeError(agentId, err); }
  }

  async function submitLoginCode(code) {
    const agentId = activeAgentId;
    setRuntimeErrors(current => ({ ...current, [agentId]: '' }));
    try {
      await invoke('submit_acp_agent_login_code', { agentId, code });
      await refreshStatus(agentId);
    } catch (err) {
      showRuntimeError(agentId, err);
    }
  }

  async function send() {
    const message = draft.trim();
    const readyAttachments = attachments.filter(attachment => (
      attachment.status === 'ready' && attachment.result
    ));
    if ((!message && !readyAttachments.length && !workspaceReferences.length)
      || busy || working || activeRuntimeBusy) return;
    if (!isNativeAgent && !activeStatus?.authenticated) {
      setError(codexCopy.loginRequiredBeforeSend);
      return;
    }
    if (attachments.some(attachment => attachment.status === 'parsing')) {
      setError(codexCopy.attachmentsParsing);
      return;
    }
    if (workspaceUnavailable) return;
    if (activeId && !sessionReady) return;
    if (isNativeAgent) {
      await sendNative(message, readyAttachments);
      return;
    }
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

  /// 原生（品悟 Engine）发送：草稿态先建会话（强制临时工作区），随后走 chat 命令；
  /// 用户气泡乐观插入 lane，chat 命令同步失败（空消息 / turn 占用等）时回滚。
  async function sendNative(message, readyAttachments) {
    setWorking(true); setError('');
    try {
      let targetId = activeId;
      if (!targetId) {
        const created = await createSession(draftWorkspacePath);
        targetId = created.id;
        // 草稿态暂存的模型/知识库/模式/多智能体选择先落到新会话（失败会显式报错）。
        await applyNativeDraftControls(targetId);
        // createSession 内的首次 load 发生在草稿控件落盘之前；若用户在草稿态
        // 开启了多智能体，那次 load 读到的是旧的 false。首条消息发送前必须
        // 再读一次后端权威状态，保证输入框开关及时反映刚落盘的会话配置。
        await refreshNativeControls(targetId);
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
      const referencePrefix = workspaceReferences.length
        ? `${workspaceReferences.map(path => `@${path}`).join(' ')}\n\n`
        : '';
      const displayText = message + (readyAttachments.length
        ? `${message ? '\n' : ''}📎 ${readyAttachments.map(attachment => attachment.basename).join(', ')}`
        : '');
      const lane = getNativeLane(targetId);
      const optimisticId = appendLocalUserMessage(lane, displayText);
      setNativeLaneTick(tick => tick + 1);
      autoScrollRef.current = true;
      setShowScrollBottom(false);
      setDraft('');
      try {
        await invoke('chat', {
          message: referencePrefix + message,
          attachments: readyAttachments.map(attachment => attachment.result),
          sessionId: targetId,
          // 逐轮工具白名单入口（R-2）：参数链路对 code 会话已贯通（后端 op
          // allowed_tools 按此生效），本期恒 false 不限制；S-1 安全分化落地时
          // 按 SessionPolicy 逐轮驱动（docs/code-mode-解耦与权限持久化-改动说明.md）。
          restrictTools: false,
        });
      } catch (sendError) {
        removeLocalUserMessage(lane, optimisticId);
        setNativeLaneTick(tick => tick + 1);
        throw sendError;
      }
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
    if (isNativeAgent) {
      await invoke('cancel_generation', { sessionId: activeId }).catch(showError);
      return;
    }
    await invoke('cancel_codex_acp', { sessionId: activeId }).catch(showError);
  }

  /// 原生会话的选择确认卡提交/取消：chat:user_input_required → submit_user_input /
  /// cancel_user_input（显式 sessionId，不经过 bridge 全局 activeSession）。
  async function respondNativeInput(toolCallId, answers) {
    if (!activeId) return;
    // entry 捕获 sid：invoke 挂起期间用户切到别的原生会话时，await 后重新读
    // activeId 会把 restoredAnswers 写进（或找不到卡而漏写）错误 lane——与 bridge
    // submitUserInput 的 sid 捕获同一约定。
    const sid = activeId;
    setResponding(true); setError('');
    try {
      await invoke('submit_user_input', { toolCallId, answers, sessionId: sid });
      markNativeInputResolved(sid, toolCallId, 'submitted', answers);
    } catch (err) { showError(err); }
    finally { setResponding(false); }
  }

  async function cancelNativeInput(toolCallId) {
    if (!activeId) return;
    const sid = activeId;
    setResponding(true); setError('');
    try {
      await invoke('cancel_user_input', { toolCallId, sessionId: sid });
      markNativeInputResolved(sid, toolCallId, 'cancelled');
    } catch (err) { showError(err); }
    finally { setResponding(false); }
  }

  function markNativeInputResolved(sessionId, toolCallId, cardState, answers) {
    const lane = getNativeLane(sessionId);
    // 无条件按 type + toolCallId 定位：chat:tool_end（applyNativeChatEvent 同样按
    // !item.resolved 查找）可能先于 invoke 返回把卡置为 resolved，若这里仍要求
    // !item.resolved 会因竞态漏写 restoredAnswers，重挂载时历史卡丢失选中态。
    const card = [...lane.items].reverse().find(item => (
      item && item.type === 'user_input' && item.toolCallId === toolCallId
    ));
    if (card) {
      card.resolved = true;
      card.cardState = cardState;
      // 提交后立即记住答案：即使不切会话、仅组件重挂载，历史卡也能恢复选中态。
      if (cardState === 'submitted' && Array.isArray(answers) && answers.length) {
        card.restoredAnswers = answers;
      }
    }
    setNativeLaneTick(tick => tick + 1);
  }

  // 原生车道手动压缩：语义镜像 bridge interaction.compactNow——调 compact_now 后，
  // 进行中/结果由 chat:compaction 系统项呈现（compactStart/compactDone/compactFail）；
  // invoke 本身失败按 work 侧同款补一条 compactFail 系统提示项。
  async function compactNativeSession() {
    const sid = activeId;
    if (!sid || !isNativeAgent) return;
    const lane = getNativeLane(sid);
    if (lane.busy || lane.compacting) return;
    setError('');
    try {
      await invoke('compact_now', { sessionId: sid });
    } catch (err) {
      appendNativeSystemItem(lane, `${codexCopy.compactFail}: ${String(err && err.message ? err.message : err || '')}`);
      setNativeLaneTick(tick => tick + 1);
    }
  }

  // 记忆条目的类型标签：复用设置页 memoryTypes 三语；profile 类对应设置页"个人资料"。
  function nativeMemoryKindLabel(kind) {
    const detail = t.uiSettingsDetail || {};
    if (kind === 'profile') return detail.profile || kind;
    return (detail.memoryTypes && detail.memoryTypes[kind]) || kind || codexCopy.nativeMemory;
  }

  // 原生车道方案卡【批准】：语义镜像 bridge interaction.acceptPlan——乐观置卡 +
  // 用户回声（display_message 与按钮同文），accept_plan 失败按 plan_not_active 分流回滚。
  async function acceptNativePlan(card) {
    const sid = activeId;
    if (!sid || !isNativeAgent || busy) return;
    const lane = getNativeLane(sid);
    const planId = String(card.planId || '').trim();
    const stillActionable = Boolean(planId) && lane.items.some(item => (
      item === card && item.cardState === 'active' && !item.resolved
    ));
    if (!stillActionable) return;
    setError('');
    card.cardState = 'approved';
    card.resolved = true;
    card.statusKey = 'approved';
    const echoText = t.planGo;
    const echoId = appendLocalUserMessage(lane, echoText);
    setNativeLaneTick(tick => tick + 1);
    try {
      await invoke('accept_plan', {
        sessionId: sid,
        planId,
        planMarkdown: card.planMarkdown || '',
        displayMessage: echoText,
      });
    } catch (err) {
      const errorText = String(err && err.message ? err.message : err || '');
      const planNotActive = errorText.indexOf('plan_not_active') >= 0;
      if (planNotActive) {
        card.cardState = 'frozen';
        card.resolved = true;
        card.statusKey = 'historical';
      } else {
        card.cardState = 'active';
        card.resolved = false;
        card.statusKey = '';
      }
      removeLocalUserMessage(lane, echoId);
      appendNativeSystemItem(lane, `${codexCopy.nativePlanAcceptFailed}${errorText}`);
      setNativeLaneTick(tick => tick + 1);
      refreshNativeControls(sid).catch(() => {});
      return;
    }
    // accept_plan 已把会话切到 Yolo：同步底栏 mode chip。
    refreshNativeControls(sid).catch(() => {});
  }

  // 原生车道方案卡【放弃】：语义镜像 bridge interaction.discardPlan——只关卡片不动
  // mode（放弃方案 ≠ 退出 Plan）；失败按 plan_not_active 分流恢复/冻结。
  async function discardNativePlan(card) {
    const sid = activeId;
    if (!sid || !isNativeAgent) return;
    const lane = getNativeLane(sid);
    const planId = String(card.planId || '').trim();
    if (!planId || card.resolved || card.cardState !== 'active') return;
    setError('');
    card.cardState = 'frozen';
    card.resolved = true;
    card.statusKey = 'discarded';
    setNativeLaneTick(tick => tick + 1);
    try {
      await invoke('discard_plan', { sessionId: sid, planId });
    } catch (err) {
      const errorText = String(err && err.message ? err.message : err || '');
      const planNotActive = errorText.indexOf('plan_not_active') >= 0;
      if (planNotActive) {
        card.statusKey = 'historical';
        refreshNativeControls(sid).catch(() => {});
      } else {
        card.cardState = 'active';
        card.resolved = false;
        card.statusKey = '';
      }
      appendNativeSystemItem(lane, `${codexCopy.nativePlanDiscardFailed}${errorText}`);
      setNativeLaneTick(tick => tick + 1);
    }
  }

  // 原生（品悟）车道 deepseek 投影项渲染：agent_message 用 lane 保存的原始 markdown；
  // user_input 走选择确认卡；plan_card 走方案审批卡；careful_blocked 是拦截提示
  // （无需交互）；system 是引擎透传提示。reasoning / tool_group 由 ConversationTimeline 默认渲染。
  function renderNativeItem(item) {
    if (item.type === 'agent_message' && item.legacyItem) {
      return (
        <ConversationMarkdown
          text={item.legacyItem.text}
          onOpenExternal={(url) => invoke('open_user_external_url', { url }).catch(showError)}
          onOpenResource={openWorkspaceResource}
        />
      );
    }
    if (item.type === 'plan' && item.extensionType === 'plan_card' && item.legacyItem) {
      return (
        <NativePlanCard
          item={item.legacyItem}
          theme={theme}
          t={t}
          copy={codexCopy}
          modePlan={nativeModeValue === 'plan'}
          busy={busy}
          onAccept={card => acceptNativePlan(card).catch(showError)}
          onDiscard={card => discardNativePlan(card).catch(showError)}
        />
      );
    }
    if (item.type === 'user_input' && item.legacyItem) {
      return (
        <NativeUserInputCard
          item={item.legacyItem}
          responding={responding}
          onSubmitAnswers={respondNativeInput}
          onCancelInput={cancelNativeInput}
          copy={codexCopy}
          conversationCopy={t.uiConversation}
        />
      );
    }
    if (item.type === 'permission' && item.extensionType === 'careful_blocked') {
      return (
        <div className="rounded-xl border border-red-500/20 bg-red-500/[0.06] px-3 py-2 text-[12px] text-red-600 dark:text-red-300">
          {codexCopy.nativeBlockedNotice}
        </div>
      );
    }
    if (item.type === 'system_notice' && item.legacyItem) {
      const legacy = item.legacyItem;
      if (legacy.compactPhase) {
        const label = legacy.compactPhase === 'start'
          ? codexCopy.compactStart
          : legacy.compactPhase === 'fail'
            ? codexCopy.compactFail
            : codexCopy.compactDone;
        return (
          <div className="px-1 text-[11px] text-gray-400">
            {label}{legacy.text ? ` · ${legacy.text}` : ''}
          </div>
        );
      }
      return <div className="px-1 text-[11px] text-gray-400">{legacy.text}</div>;
    }
    return undefined;
  }

  async function openWorkspaceResource(resourcePath) {
    if (!activeId || !resourcePath) return;
    try {
      await invoke('open_codex_workspace_resource', {
        sessionId: activeId,
        resourcePath: String(resourcePath),
      });
    } catch (err) {
      showError(err);
    }
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
    if (!modelId || activeRuntimeBusy) return;
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
    if (activeRuntimeBusy) return;
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
    if (!modeId || activeRuntimeBusy) return;
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
            onClick={toggleWorkspacePanel}
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
        {!activeSession && draftWorkspacePath && (
        <header className="h-14 shrink-0 px-5 flex items-center justify-end border-b border-black/[0.05] dark:border-white/[0.06]">
          <button
            type="button"
            data-testid="codex-workspace-toggle"
            onClick={toggleWorkspacePanel}
            className={`h-8 px-2.5 rounded-lg inline-flex items-center gap-1.5 text-[11px] transition-colors ${
              workspaceOpen
                ? 'bg-blue-500/10 text-blue-600 dark:text-blue-300'
                : 'text-gray-500 dark:text-gray-400 hover:bg-black/[0.04] dark:hover:bg-white/[0.06]'
            }`}
            title={codexCopy.workspaceTitle}
          >
            <FolderOpen size={14} />
            <span>{codexCopy.workspace}</span>
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
                {fixedSession ? codexCopy.projectMissing : (
                  <>
                    {codexCopy.recreatePrefix}
                    <button
                      type="button"
                      data-testid="codex-recreate-session"
                      onClick={recreateUnavailableWorkspaceSession}
                      className="font-medium underline underline-offset-2 hover:text-red-700 dark:hover:text-red-200"
                    >
                      {codexCopy.recreate}
                    </button>
                  </>
                )}
              </div>
            ) : isNativeAgent ? (
              // 原生（品悟）会话没有 ACP 登录/安装状态机；错误由 chat:done 事件内联展示。
              null
            ) : (
              <>
                <RuntimeNotice
                  status={activeStatus}
                  working={working || activeRuntimeBusy}
                  operation={activeRuntimeOperation}
                  error={activeRuntimeError || error}
                  onInstall={install}
                  onLogin={login}
                  onOpenLogin={openLogin}
                  onSubmitLoginCode={submitLoginCode}
                  onRefresh={() => refreshStatus(activeAgentId, true)}
                  resetKey={draftEpoch}
                  suppressAdvisoryUpgrade={Boolean(activeId)}
                  copy={codexCopy}
                />
                {activeStatus?.authenticated && (
                  <AgentServiceFailureNotice
                    failure={visibleServiceFailure}
                    agentName={activeAgentName}
                    working={working || activeRuntimeBusy}
                    onSwitchAccount={switchAccount}
                    onManageProviders={
                      onOpenSettingsSection
                        ? () => onOpenSettingsSection('providers')
                        : null
                    }
                    onDismiss={() => setDismissedFailureKey(serviceFailure?.key || '')}
                    copy={codexCopy}
                    providerCopy={t.uiAcpProviders}
                  />
                )}
              </>
            )}
            {!visibleTurns.length && (
              <div className="flex min-h-[320px] flex-1 flex-col items-center justify-center text-center">
                <div className="w-14 h-14 rounded-2xl bg-black/[0.04] dark:bg-white/[0.08] flex items-center justify-center shadow-lg"><AcpAgentLogo agentId={activeAgentId} className="h-8 w-8" title={activeAgentName} /></div>
                <div className="mt-5 text-[20px] font-semibold">
                  {codexCopy.welcomeTitle}
                </div>
                <div className="mt-2 max-w-md text-[13px] leading-6 text-gray-500 dark:text-gray-400">
                  {activeSession
                    ? (isNativeAgent ? codexCopy.nativeActiveHint : codexCopy.activeHint)
                    : isNativeAgent
                      ? codexCopy.nativeDraftHint
                      : codexCopy.draftHint}
                </div>
              </div>
            )}
            {visibleTurns.map(turn => (useUnifiedConversationUi || isNativeAgent)
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
                    renderItem={isNativeAgent
                      ? (item) => renderNativeItem(item)
                      : (item) => item.type === 'elicitation'
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
                    renderToolItem={isNativeAgent
                      ? (item) => item.legacyItem
                        && !isSearchTool(item.tool)
                        && !isFetchTool(item.tool)
                        ? (
                            <ToolCard
                              item={{ ...item.legacyItem, sessionId: activeId }}
                              sessionId={activeId}
                              theme={theme}
                              t={t}
                              variant="timeline"
                            />
                          )
                        : undefined
                      : undefined}
                    agentLabel={activeAgentName}
                    onOpenExternal={(url) => invoke('open_user_external_url', { url }).catch(showError)}
                    onOpenResource={openWorkspaceResource}
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
                    onOpenExternal={(url) => invoke('open_user_external_url', { url }).catch(showError)}
                    onOpenResource={openWorkspaceResource} />
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
                onManageProviders={
                  onOpenSettingsSection
                    ? () => onOpenSettingsSection('providers')
                    : null
                }
                isDark={theme === 'dark'}
                onChange={onSwitchHomeMode}
                copy={t.uiHomeMode}
              />
            )}
            {sessionSyncing && !isNativeAgent && (
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
              {nativeVoiceInput.status !== 'idle' && nativeVoiceInput.message && (
                <div className={`flex items-center justify-between gap-2 mb-2 px-3 py-2 rounded-2xl text-[12px] ${
                  nativeVoiceInput.status === 'failed'
                    ? (theme === 'dark' ? 'bg-[#3A1F1F] text-[#F28B82]' : 'bg-[#FCE8E6] text-[#C5221F]')
                    : (theme === 'dark' ? 'bg-[#1E2B3A] text-[#A8C7FA]' : 'bg-[#E8F0FE] text-[#174EA6]')
                }`}>
                  <span className="min-w-0 truncate">
                    {nativeVoiceInput.status === 'requesting_permission' ? t.voiceRequesting
                      : nativeVoiceInput.status === 'recording' ? t.voiceRecording
                      : nativeVoiceInput.status === 'transcribing' ? t.voiceTranscribing
                      : nativeVoiceInput.status === 'completed' ? t.voiceCompleted
                      : nativeVoiceInput.message}
                  </span>
                  <div className="flex items-center gap-1 shrink-0">
                    {nativeVoiceInput.status === 'failed' && nativeVoiceInput.category === 'recognition_failed'
                      && nativeVoiceCanInstallAsr && onGotoSettings && (
                      <button onClick={onGotoSettings} className={`px-2 py-1 rounded-full font-medium ${theme === 'dark' ? 'bg-white/10 hover:bg-white/20' : 'bg-black/5 hover:bg-black/10'}`}>{t.voiceGotoDeps}</button>
                    )}
                    {nativeVoiceInput.status === 'failed' && (
                      <button onClick={handleNativeVoiceClick} className={`px-2 py-1 rounded-full ${theme === 'dark' ? 'hover:bg-white/10' : 'hover:bg-black/5'}`}>{t.voiceRetry}</button>
                    )}
                    {nativeVoiceActive && (
                      <button onClick={handleNativeVoiceCancel} className={`px-2 py-1 rounded-full ${theme === 'dark' ? 'hover:bg-white/10' : 'hover:bg-black/5'}`}>{t.voiceCancel}</button>
                    )}
                    {!nativeVoiceActive && (
                      <button onClick={handleNativeVoiceClose} title={t.voiceClose} className={`w-6 h-6 rounded-full flex items-center justify-center ${theme === 'dark' ? 'hover:bg-white/10' : 'hover:bg-black/5'}`}>×</button>
                    )}
                  </div>
                </div>
              )}
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
                  // 输入法合成期间(例如中文输入法敲回车确认候选词)不要触发发送,
                  // 否则一次回车会既上屏又发送。与 ChatView / PetWindow 保持一致。
                  if (event.key === 'Enter' && !event.shiftKey && !isImeComposing(event)) {
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
                    className={COMPOSER_ICON_BUTTON_CLASS}
                    title={codexCopy.addAttachment}
                    aria-label={codexCopy.addAttachment}
                  >
                    <Paperclip size={18} />
                  </button>
                  <button
                    type="button"
                    data-testid="codex-voice-input"
                    onClick={handleNativeVoiceClick}
                    disabled={nativeVoiceDisabled}
                    aria-label={nativeVoiceLabel}
                    title={nativeVoiceLabel}
                    className={`${
                      nativeVoiceRecording
                        ? 'w-9 h-9 shrink-0 rounded-full flex items-center justify-center transition-colors bg-[#C5221F] text-white hover:bg-[#A50E0E] border border-transparent'
                        : nativeVoiceActive
                          ? `${COMPOSER_ICON_BUTTON_CLASS} text-[#174EA6] dark:text-[#A8C7FA]`
                          : COMPOSER_ICON_BUTTON_CLASS
                    } ${nativeVoiceDisabled ? 'opacity-70 cursor-wait' : ''}`}>
                    <Mic size={18} />
                  </button>
                  <button type="button" onClick={() => setCommandOpen(value => !value)}
                    disabled={!availableCommands.length}
                    className="h-7 px-2 rounded-lg text-[11px] font-mono hover:bg-black/[0.05] dark:hover:bg-white/[0.07] disabled:opacity-40"
                    title={availableCommands.length ? codexCopy.commandsAvailable : codexCopy.commandsAfterSession}>/</button>
                  {isNativeAgent && (
                    // 原生（品悟）车道的底栏控件：与工作/设计页共用同一套共享 composer
                    // 控件（ComposerModeChip / ComposerModelSelector / ComposerKbSelector，
                    // 显式会话态驱动 props 绕开 bridge 聊天 active 绑定）；行为（直调
                    // per-session 命令、草稿暂存、busy 禁用、归属保护）不变。Plan 说明：
                    // 原生车道已接 plan_snapshot/plan_ready，切 Plan 后方案以审批卡呈现。
                    <div data-testid="native-composer-controls" className="flex min-w-0 flex-wrap items-center gap-2">
                      <ComposerModeChip
                        t={t}
                        bs={bs}
                        mode={nativeModeValue}
                        busy={busy || working}
                        onSwitch={switchNativeMode}
                      />
                      {nativeModelChoices.length > 0 && (
                        <ComposerModelSelector
                          t={t}
                          bs={bs}
                          onGotoSettings={onGotoModelSettings}
                          sessionId={activeId}
                          sessionModelId={nativeSessionModelId}
                          busy={busy || working}
                          onSwitchModel={(sessionId, modelId) => switchNativeModel(sessionId, String(modelId))}
                          multiAgentEnabled={nativeMultiAgentEnabled}
                          multiAgentAvailable={nativeMultiAgentAvailable}
                          onToggleMultiAgent={switchNativeMultiAgent}
                        />
                      )}
                      <ComposerToolMenu
                        t={t}
                        onGotoTools={onGotoTools}
                        compact={false}
                        activeSkill={null}
                        triggerVariant="pill"
                        triggerTestId="native-tools"
                        scope="code"
                      />
                      <ComposerKbSelector
                        t={t}
                        bs={bs}
                        mountedId={nativeMountedId}
                        onMount={mountNativeKb}
                        onUnmount={unmountNativeKb}
                      />
                      {activeId && nativeTokensInput > 0 && (
                        // 用量 chip 兼手动压缩入口（compact_now 的后端注释语义即"用户点 token
                        // 进度条 → 立即压缩"）；tokens.max 恒 0 的已知限制下只显示已用 token。
                        <button
                          type="button"
                          data-testid="native-usage-chip"
                          onClick={() => compactNativeSession().catch(showError)}
                          disabled={busy || working || nativeCompacting}
                          title={codexCopy.nativeCompactTitle}
                          aria-label={codexCopy.nativeCompactTitle}
                          className="inline-flex h-8 items-center gap-1.5 rounded-xl border border-black/[0.07] bg-black/[0.025] px-2.5 text-[11px] font-semibold text-[#1F1F1F] transition-all hover:-translate-y-px hover:shadow-sm disabled:cursor-default disabled:opacity-50 dark:border-white/[0.09] dark:bg-white/[0.055] dark:text-[#E8EAED]"
                        >
                          {nativeCompacting ? codexCopy.compactStart : `${t.ctxUsage} ${fmtNativeCtxTok(nativeTokensInput)}`}
                        </button>
                      )}
                      {nativeMemoryItems.length > 0 && (
                        // 记忆轻量展示：条数徽标 + 点击弹层列出本会话注入的记忆条目
                        // （不照搬 work 的完整记忆面板；无条目时不占位）。
                        <div className="relative min-w-0">
                          <button
                            type="button"
                            data-testid="native-memory-badge"
                            onClick={() => setMemoryOpen(value => !value)}
                            title={codexCopy.nativeMemoryTitle}
                            aria-label={codexCopy.nativeMemoryTitle}
                            aria-expanded={memoryOpen}
                            className="inline-flex h-8 items-center gap-1.5 rounded-xl border border-black/[0.07] bg-black/[0.025] px-2.5 text-[11px] font-semibold text-[#1F1F1F] transition-all hover:-translate-y-px hover:shadow-sm dark:border-white/[0.09] dark:bg-white/[0.055] dark:text-[#E8EAED]"
                          >
                            <Brain size={13} className="shrink-0 text-gray-400" />
                            {`${codexCopy.nativeMemory} ${nativeMemoryItems.length}`}
                          </button>
                          {memoryOpen && (
                            <>
                              <button type="button" aria-label={codexCopy.nativeMemoryClose} className="fixed inset-0 z-30 cursor-default" onClick={() => setMemoryOpen(false)} />
                              <div data-testid="native-memory-panel" className="absolute bottom-full left-0 z-40 mb-2 max-h-72 w-[320px] max-w-[calc(100vw-32px)] overflow-y-auto rounded-2xl border border-black/[0.08] bg-white/95 p-2 shadow-xl backdrop-blur-xl dark:border-white/10 dark:bg-[#202124]/95">
                                <div className="px-2 py-1 text-[10px] uppercase tracking-wider text-gray-400">{codexCopy.nativeMemoryTitle}</div>
                                {nativeMemoryItems.map((item, index) => (
                                  <div key={item.id || `memory-${index}`} className="rounded-xl px-3 py-2">
                                    <span className="block text-[10px] font-medium text-gray-400">{nativeMemoryKindLabel(item.kind)}</span>
                                    <span className="mt-0.5 block text-[12px] text-gray-700 dark:text-gray-200">{item.text}</span>
                                  </div>
                                ))}
                              </div>
                            </>
                          )}
                        </div>
                      )}
                    </div>
                  )}
                  {!isNativeAgent && (
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
                              disabled={working || activeRuntimeBusy || activeStatus?.login_in_progress}
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
                                refreshStatus(activeAgentId, true).catch(showError);
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
                  )}
                  {composerControlsVisible && !isNativeAgent && (
                    <div data-testid="codex-composer-configs" className="flex flex-wrap items-center gap-2">
                      {codexRelayNoModel && (
                        <span className="text-[11px] opacity-60">{codexCopy.relayNoModelHint}</span>
                      )}
                      {visibleFallbackModels.length > 0 && (
                        <CodexComposerConfigSelect
                          id="model"
                          label={codexCopy.model}
                          value={composerModelValue}
                          choices={visibleFallbackModels.map(model => ({
                            value: model.id,
                            name: model.name || model.id,
                            // 别名标签仅 Claude 需要（其五个选项显示名同为槽位映射值）；
                            // kimi/codex 的 id 与 name 语义不同，加标签反而是噪音
                            tag: activeAgentId === 'claude' ? model.id : undefined,
                          }))}
                          onChange={changeModel}
                          disabled={busy || working || activeRuntimeBusy}
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
                          disabled={busy || working || activeRuntimeBusy}
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
                          choices={option.id === 'model'
                            // 模型走 config 通道时仅 Claude 用别名值作标签（其显示名可能全相同）；
                            // kimi 中转激活时经 modelConfigChoices 过滤掉官方模型
                            ? modelConfigChoices(option).map(choice => ({
                                ...choice,
                                tag: activeAgentId === 'claude' ? choice.value : undefined,
                              }))
                            : configChoices(option)}
                          onChange={value => changeConfig(option.id, value)}
                          disabled={busy || working || activeRuntimeBusy}
                          title={option.description || option.name}
                          unsetLabel={codexCopy.notSet}
                        />
                      ))}
                      {/* 会话级 Provider 覆盖仅 Codex 生效（spawn 时按会话注入
                          OPENAI_API_KEY）；Claude/Kimi 的 CLI 配置是进程级的，
                          无法按会话隔离，不展示该选项避免误导。 */}
                      {activeAgentId === 'codex' && Boolean(activeId) && sessionProviderChoices.length > 1 && (
                        <CodexComposerConfigSelect
                          id="provider"
                          label={(t.uiAcpProviders || {}).sessionProvider || 'Provider'}
                          value={sessionProviderValue}
                          choices={sessionProviderChoices}
                          onChange={changeSessionProvider}
                          disabled={busy || working || activeRuntimeBusy || Boolean(configApplying)}
                          title={(t.uiAcpProviders || {}).sessionProviderDesc || ''}
                          unsetLabel={(t.uiAcpProviders || {}).sessionOfficial || 'Official'}
                        />
                      )}
                    </div>
                  )}
                </div>
                {busy ? (
                  <button onClick={cancel} className="w-9 h-9 rounded-full flex items-center justify-center bg-red-500/10 text-red-500 hover:bg-red-500/15"><StopCircle size={18} /></button>
                ) : (
                  <button onClick={send} disabled={!sessionReady || (!draft.trim() && !attachments.some(attachment => attachment.status === 'ready') && !workspaceReferences.length) || working || activeRuntimeBusy || (!isNativeAgent && (!activeStatus || !activeStatus.installed || !activeStatus.authenticated))}
                    className="w-9 h-9 rounded-full flex items-center justify-center bg-[#007AFF] text-white shadow-sm hover:bg-[#006EE6] disabled:bg-black/[0.06] dark:disabled:bg-white/10 disabled:text-gray-400 disabled:shadow-none">
                    <Send size={16} />
                  </button>
                )}
              </div>
              {pendingYoloSwitch && (
                // 首次切 yolo 的一次性确认卡（全局记忆）；确认后继续切换，取消留在 Plan。
                <NativeYoloConfirmCard
                  theme={theme}
                  t={t}
                  busy={yoloConfirmBusy}
                  onConfirm={confirmPendingYoloSwitch}
                  onCancel={() => setPendingYoloSwitch(null)}
                />
              )}
            </div>
          </div>
        </div>
        </div>
        {!subagentPanel && (activeSession || draftWorkspacePath) && (
          <CodexWorkspacePanel
            session={activeSession}
            workspacePath={activeSession ? '' : (draftWorkspacePath || '')}
            visible={workspaceOpen}
            onClose={closeWorkspacePanel}
            references={workspaceReferences}
            onAddReference={addWorkspaceReference}
            refreshToken={isNativeAgent ? nativeLaneTick : events.length}
            onChangeCount={setWorkspaceChangeCount}
            copy={t.uiCodexWorkspace}
          />
        )}
        {subagentPanel && activeSession && isNativeAgent && (
          <SubagentTranscriptPanel
            sessionId={activeSession.id}
            initialAgentId={subagentPanel.agentId}
            selectionRequestId={subagentPanel.selectionRequestId}
            t={t}
            theme={theme}
            onClose={closeSubagentPanel}
          />
        )}
        </div>
    </div>
  );
}
