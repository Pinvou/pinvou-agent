import { Fragment, useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { FileTypeIcon } from '../../components/files/FileTypeIcon.jsx';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import {
  AlertTriangle, Brain, Check, CheckCircle2, ChevronDown, FileText, FolderOpen, Mic, Monitor, Paperclip,
  Plus, RefreshCw, Send, Sparkles, StopCircle, Terminal, Upload, User, Wrench,
} from '../../components/icons.jsx';
import { AcpAgentLogo } from './AcpAgentLogo.jsx';
import { CodexWorkspacePanel } from './CodexWorkspacePanel.jsx';
import { SubagentTranscriptPanel } from '../multiagent/SubagentTranscriptPanel.jsx';
import {
  refreshAcpAgentCatalog,
  startSerialStatusPolling,
  useAcpAgentStatus,
} from './runtimeStatus.js';
import {
  classifyAcpServiceFailure,
  isAcpAuthenticationFailure,
  runtimeOperationFor,
} from './runtimeNoticeState.js';
import {
  AgentServiceFailureNotice,
  RuntimeNotice,
  runtimeSourceLabel,
} from './AcpRuntimeNotices.jsx';
import { acpErrorMessage } from './acpErrors.js';
import {
  cancelPendingAcpAttachments,
  isPendingAcpAttachment,
  runAcpAttachmentTask,
} from './acp-attachment-lifecycle.js';
import {
  createAcpSessionOperationTracker,
  removeAcpDraftItems,
  transferAcpDraftItems,
} from './acp-session-operation.js';
import { ComposerPopover, useOutsidePointerClose } from '../../components/ComposerPopover.jsx';
import {
  appendAcpEvent,
  buildElicitationContent,
  createAcpEventSeqTracker,
  createAcpGapResyncScheduler,
  mergeAcpTimelineSnapshot,
  updateAcpAttachmentDraft,
  commandExecutionDetails,
  projectAcpTimeline,
  resolveAcpSessionControls, unifiedConversationUiEnabled,
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
  canApplyNativeControlsRefresh,
  claimNativeControlsRefreshId,
  finalizePreparedSessionCreation,
  resolveNativeModelId,
} from './native-session-handoff.js';
import {
  checkpointRefreshKey,
  reloadSessionAfterRewind,
  rewindEntriesByTurnId,
  rewindNoticeText,
  rewindUndoAvailable,
  useSessionCheckpoints,
} from './checkpoints.js';
import { RewindChip, RewindConfirmDialog, RewindUndoChip, RewindUndoConfirmDialog } from './RewindChip.jsx';
import {
  ConversationActivityIndicator,
  ConversationMarkdown,
  ConversationTurn,
  WorkspaceResourceButtons,
} from '../conversation/ConversationTimeline.jsx';
import {
  measureConversationScrollGeometry,
  startConversationBottomFollower,
  transitionConversationScrollState,
} from '../conversation/conversation-scroll.js';
import { AssistantMessageActions, AssistantMessageFooter } from '../conversation/AssistantMessageActions.jsx';
import { assistantResponseAvailable, assistantResponseText } from '../conversation/message-clipboard.js';
import { ComposerModelSelector, ComposerToolMenu } from '../settings/composer-shared.jsx';
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
  isSearchTool,
  restoreConversationScrollPosition,
  toolWorkspaceResources,
} from '../conversation/conversation-model.js';
import { QuestionChoiceCard } from '../conversation/QuestionChoiceCard.jsx';
import { PlanLayer, ToolCard, cardBoxCls, cardBtnCls } from '../tools/tool-renderers.jsx';
import { notifyChatRoundCommitted } from '../tools/tool-events.js';
import { AttachmentChips } from '../attachments/AttachmentChips.jsx';
import { formatAttachmentLimitError } from '../attachments/attachment-limit-errors.js';
import { ComposerAttachmentDropOverlay } from '../attachments/ComposerAttachmentDropOverlay.jsx';
import { HomeModeSwitcher } from '../conversation/HomeModeSwitcher.jsx';
import { bridge } from '../../hooks/useBridge.js';
import {
  invokeTauri,
  listenTauri,
  openTauriDialog,
} from '../../platform/tauri/client.js';
import {
  cancelAcpSession,
  createAcpSession,
  discardAcpAttachment,
  getAcpSessionInfo,
  ingestAcpAttachmentPath,
  loadAcpPendingElicitations,
  loadAcpPendingPermissions,
  loadAcpTimeline,
  listAcpSessions,
  openAcpExternalUrl,
  pickAcpWorkspace,
  respondAcpElicitation,
  respondAcpPermission,
  setAcpConfigOption,
  setAcpMode,
  setAcpModel,
  submitAcpPrompt,
  uploadAcpDeviceAttachment,
} from './acpClient.js';
import { can, canInvoke, isWeb, onPlatformConnectionChange } from '../../shared/platform.js';
import {
  forgetWorkspace,
  loadRecentWorkspaces,
  rememberWorkspace,
  workspaceName,
} from '../../shared/workspace-recents.js';
const invoke = invokeTauri;
const DRAFT_ATTACHMENT_KEY = '__codex_draft__';

// 草稿配置快照缓存已抽到 ./acp-draft-controls.js（供设置页共用，避免与
// SettingsView 的循环引用）。
import {
  consumeAcpModelsProbePending,
  loadDraftControlsCache,
  rememberDraftControls,
} from './acp-draft-controls.js';
const AGENT_SELECTION_KEY = 'pinvou_codex_agent_selection';
// Constant collection used only for membership checks: Set.has replaces array .includes (biome prefer-set-has).
const CODE_AGENT_IDS = new Set(['pinvou', 'codex', 'claude', 'kimi']);
// Stable empty turns for the draft empty state: an inline [] would invalidate any useMemo depending on its reference on every render.
/** @type {{ id: string, status: string }[]} */
const EMPTY_CONVERSATION_TURNS = [];
// Same idea: the sessions default must be a stable reference; an inline [] is a fresh array on every render.
const EMPTY_SESSIONS = [];

// token 缩写与主聊天 ChatView 的 fmtCtxTok 同款（1.2k / 3.4M）。
function fmtNativeCtxTok(n) {
  return n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${(n / 1e3).toFixed(1)}k` : String(n);
}

// 记住用户上次在 code 界面选择的 agent：重开界面/重启应用后沿用，直到用户再次切换。
function loadAgentSelection() {
  try {
    const value = localStorage.getItem(AGENT_SELECTION_KEY);
    return value && CODE_AGENT_IDS.has(value) ? value : null;
  } catch {
    return null;
  }
}

function initialDraftAgentSelection() {
  const saved = loadAgentSelection();
  if (isWeb) return saved && saved !== 'pinvou' ? saved : 'codex';
  return saved || 'pinvou';
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
  const exitHint = details.exitCode == null ? '' : ` · exit ${details.exitCode}`;
  const outcome = state === 'running'
    ? `${copy.running} · ${duration}`
    : state === 'failed'
      ? `${copy.executionFailed}${exitHint}`
      : `${copy.executionFinished}${exitHint} · ${duration}`;
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
  const stateLabel = state === 'running'
    ? `${copy.inProgress} · ${duration}`
    : state === 'failed'
      ? copy.failed
      : `${cv.ended} · ${duration}`;
  const resources = toolWorkspaceResources(tool);
  return (
    <div className="rounded-xl border border-black/[0.05] dark:border-white/[0.07] bg-white/45 dark:bg-white/[0.015]">
      <CompactItemRow icon={<Wrench size={13} />} title={tool.title || label}
        meta={`${label} · ${stateLabel}`}
        status={state} open={open} controlsId={detailsId}
        onToggle={() => setOpen(value => !value)} />
      <WorkspaceResourceButtons resources={resources} onOpenResource={onOpenResource} />
      {open && (
        <div id={detailsId} data-testid="conversation-compact-item-content" className="px-3 pb-3 border-t border-black/[0.05] dark:border-white/[0.06]">
          <StructuredValue label={copy.arguments} value={tool.rawInput} />
          <StructuredValue label={copy.result} value={tool.rawOutput == null ? tool.content : tool.rawOutput} />
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
              <button type="button" key={option.optionId} disabled={!actionable || responding}
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
      statusText={actionable
        ? ''
        : elicitation.resolved
          ? (elicitation.action === 'accept' ? copy.submitted : copy.canceled)
          : copy.inputExpired}
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
  // the parens hold free text excluding delimiters; the quantifier is not nested in its own char class, so backtracking is linear
  // eslint-disable-next-line no-useless-escape -- keep \( \) escaped inside the char class to mark them as literal parens, echoing the negated paren class
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
      statusText={actionable
        ? ''
        : (item.cardState === 'cancelled' ? copy.canceled : copy.submitted)}
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
  // portal 到 <body>：该卡片渲染在 composer 容器内，而容器的 backdrop-blur 会成为
  // `position: fixed` 的包含块，不 portal 的话全屏模态只会盖住输入框区域，
  // 点击遮罩取消也随之失效。
  return createPortal(
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
        <div className="mt-2 text-[12px] text-[#C5221F] dark:text-red-400">{t.modeYoloConfirmHint}</div>
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
            className={cardBtnCls('danger')}
            disabled={busy}
            onClick={onConfirm}
          >{t.modeYoloConfirmOk}</button>
        </div>
      </div>
    </div>,
    document.body,
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
    // streaming = the projection's in_progress convention (ACP/deepseek projections agree): while text
    // can still grow, render through the throttle; when it ends, useThrottledValue replays the full text verbatim.
    return commentary
      ? <ConversationMarkdown text={item.text} onOpenExternal={onOpenExternal} onOpenResource={onOpenResource}
          streaming={item.status === 'in_progress'}
          className="text-[13px] leading-6 text-gray-500 dark:text-gray-400" />
      : <ConversationMarkdown text={item.text} onOpenExternal={onOpenExternal} onOpenResource={onOpenResource}
          streaming={item.status === 'in_progress'} />;
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

// eslint-disable-next-line sonarjs/cognitive-complexity -- ACP code main view: session/event/draft/attachment/scroll lifecycles share one set of ref+state; refactoring is high-risk
export function CodexAcpView({
  theme,
  t,
  sessions = EMPTY_SESSIONS,
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
  const [agents, setAgents] = useState(null); // null=加载中，[] 才允许回退当前 Agent。
  const [draftAgentId, setDraftAgentId] = useState(initialDraftAgentSelection);
  const [status, setStatus] = useState(null);
  const [events, setEvents] = useState([]);
  const [pending, setPending] = useState([]);
  const [pendingElicitations, setPendingElicitations] = useState([]);
  const [sessionInfo, setSessionInfo] = useState(null);
  const [sessionInfoSessionId, setSessionInfoSessionId] = useState(null);
  const [sessionLoading, setSessionLoading] = useState(false);
  const [draft, setDraft] = useState('');
  const [attachmentDrafts, setAttachmentDrafts] = useState({});
  const attachmentDraftsRef = useRef(attachmentDrafts);
  attachmentDraftsRef.current = attachmentDrafts;
  const [workspaceReferenceDrafts, setWorkspaceReferenceDrafts] = useState({});
  const [workspaceOpen, setWorkspaceOpen] = useState(false);
  const [workspaceDockActive, setWorkspaceDockActive] = useState(false);
  const [workspaceDockActivation, setWorkspaceDockActivation] = useState(0);
  const [subagentPanel, setSubagentPanel] = useState(null);
  const [workspaceChangeCount, setWorkspaceChangeCount] = useState(0);
  const [now, setNow] = useState(Date.now());
  const useUnifiedConversationUi = unifiedConversationUiEnabled();
  const [localConfigApplying, setConfigApplying] = useState('');
  const acpConfigOperationTrackerRef = useRef(null);
  if (!acpConfigOperationTrackerRef.current) {
    acpConfigOperationTrackerRef.current = createAcpSessionOperationTracker(activeId);
  }
  const acpConfigOperationTracker = acpConfigOperationTrackerRef.current;
  const [acpConfigOperation, setAcpConfigOperation] = useState(null);
  const activeAcpConfigOperation = acpConfigOperationTracker.isCurrent(acpConfigOperation)
    && acpConfigOperation?.sessionId === activeId
    ? acpConfigOperation
    : null;
  const configApplying = localConfigApplying || activeAcpConfigOperation?.key || '';
  const acpSendOperationTrackerRef = useRef(null);
  if (!acpSendOperationTrackerRef.current) {
    acpSendOperationTrackerRef.current = createAcpSessionOperationTracker(
      activeId || DRAFT_ATTACHMENT_KEY,
    );
  }
  const acpSendOperationTracker = acpSendOperationTrackerRef.current;
  const [acpSendOperation, setAcpSendOperation] = useState(null);
  const activeAcpSendOperation = acpSendOperationTracker.isCurrent(acpSendOperation)
    && acpSendOperation?.sessionId === (activeId || DRAFT_ATTACHMENT_KEY)
    ? acpSendOperation
    : null;
  const [localWorking, setWorking] = useState(false);
  const working = localWorking || Boolean(activeAcpSendOperation);
  const [runtimeOperations, setRuntimeOperations] = useState({});
  const [runtimeErrors, setRuntimeErrors] = useState({});
  const [error, setError] = useState('');
  const showError = (nextError) => {
    console.error('Codex operation failed:', nextError);
    setError(acpErrorMessage(nextError, codexCopy, { allowRaw: !isWeb }));
  };
  const [respondingSessionId, setRespondingSessionId] = useState(null);
  const responding = Boolean(activeId && respondingSessionId === activeId);
  const [commandOpen, setCommandOpen] = useState(false);
  const [attachmentMenuOpen, setAttachmentMenuOpen] = useState(false);
  const [workspaceMenuOpen, setWorkspaceMenuOpen] = useState(false);
  const [accountMenuOpen, setAccountMenuOpen] = useState(false);
  const [memoryOpen, setMemoryOpen] = useState(false);
  // 这四个 composer 弹层不能用 `fixed inset-0` 关闭层：composer 容器的 backdrop-blur
  // 会成为 fixed 后代的包含块，使关闭层只覆盖输入框区域、外点失效。统一用
  // document 级 pointerdown 外点检测（见 ComposerPopover.jsx）。
  const commandMenuPanelRef = useRef(null);
  const commandMenuTriggerRef = useRef(null);
  const workspaceMenuPanelRef = useRef(null);
  const workspaceMenuTriggerRef = useRef(null);
  const memoryPanelRef = useRef(null);
  const memoryTriggerRef = useRef(null);
  const accountMenuPanelRef = useRef(null);
  const accountMenuTriggerRef = useRef(null);
  useOutsidePointerClose(commandOpen, () => setCommandOpen(false), [commandMenuPanelRef, commandMenuTriggerRef]);
  useOutsidePointerClose(workspaceMenuOpen, () => setWorkspaceMenuOpen(false), [workspaceMenuPanelRef, workspaceMenuTriggerRef]);
  useOutsidePointerClose(memoryOpen, () => setMemoryOpen(false), [memoryPanelRef, memoryTriggerRef]);
  useOutsidePointerClose(accountMenuOpen, () => setAccountMenuOpen(false), [accountMenuPanelRef, accountMenuTriggerRef]);
  const [dismissedFailureKey, setDismissedFailureKey] = useState('');
  const [draftWorkspacePath, setDraftWorkspacePath] = useState(null);
  const [draftWorkspaceHandle, setDraftWorkspaceHandle] = useState(null);
  const [recentWorkspaces, setRecentWorkspaces] = useState(loadRecentWorkspaces);
  const [draftControlsCache, setDraftControlsCache] = useState(loadDraftControlsCache);
  // 草稿态（会话未创建）下用户预选的配置：{ [agentId]: { model?, mode?, configs: { [id]: value } } }
  const [draftConfigSelections, setDraftConfigSelections] = useState({});
  const [showScrollBottom, setShowScrollBottom] = useState(false);
  const scroller = useRef(null);
  const conversationContentRef = useRef(null);
  const rightPanelScrollRef = useRef(null);
  const autoScrollRef = useRef(true);
  const lastScrollTopRef = useRef(0);
  const lastScrollHeightRef = useRef(0);
  const attachmentIdRef = useRef(0);
  const attachmentMenuTriggerRef = useRef(null);
  const deviceFileInputRef = useRef(null);
  const cancelledAttachmentIdsRef = useRef(new Set());
  const skipNextActiveLoadRef = useRef(null);
  const sessionLoadRequestRef = useRef(0);
  const acpEventSeqTrackerRef = useRef(null);
  if (!acpEventSeqTrackerRef.current) acpEventSeqTrackerRef.current = createAcpEventSeqTracker();
  const acpGapResyncRef = useRef(null);
  if (!acpGapResyncRef.current) {
    acpGapResyncRef.current = createAcpGapResyncScheduler(sessionId => {
      if (activeIdRef.current !== sessionId) return;
      return resyncAcpSessionAfterGap(sessionId);
    }, {
      onAttempt: (sessionId, attempt) => console.warn(
        `[acp] live event sequence gap detected for ${sessionId}; resyncing from the authoritative timeline (attempt ${attempt})`,
      ),
      onRetry: (_sessionId, attempt, error) => console.warn(
        `[acp] timeline resync after event sequence gap failed (attempt ${attempt}); retrying with backoff`,
        error,
      ),
      onGiveUp: (_sessionId, attempt, error) => console.warn(
        `[acp] timeline resync after event sequence gap gave up after ${attempt} attempts; the gap stays unhealed until reconnect or session reopen`,
        error,
      ),
    });
  }
  const preserveDraftWorkspaceRef = useRef(false);
  const draftEpochRef = useRef(draftEpoch);
  const activeIdRef = useRef(activeId);
  const lastActiveSessionIdRef = useRef(activeId);
  if (activeId) lastActiveSessionIdRef.current = activeId;
  useLayoutEffect(() => {
    // loadSession may optimistically point this ref at a just-created session before
    // the parent commits activeId. Do not overwrite that handoff from an intermediate
    // render carrying the old prop; layout effects still update ordinary session switches
    // before async responses can commit.
    activeIdRef.current = activeId;
    acpConfigOperationTracker.switchSession(activeId);
    acpSendOperationTracker.switchSession(activeId || DRAFT_ATTACHMENT_KEY);
  }, [acpConfigOperationTracker, acpSendOperationTracker, activeId]);
  const projection = useMemo(() => projectAcpTimeline(events), [events]);
  // 草稿态（!activeId）没有会话，退回使用该 agent 缓存的配置快照来预展示选项。
  const draftControlsInfo = activeId ? null : draftControlsCache[draftAgentId] || null;
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
    return staged === undefined ? (option.currentValue || '') : String(staged);
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
  // Live usage and hydration both restore context_window when available. Legacy timeline
  // records have no maximum, so the chip falls back to an input-token count only.
  const nativeTokensInput = isNativeAgent && activeNativeLane ? Number(activeNativeLane.tokens.input || 0) : 0;
  const nativeTokensMax = isNativeAgent && activeNativeLane ? Number(activeNativeLane.tokens.max || 0) : 0;
  const nativeCtxPct = nativeTokensMax > 0 ? Math.min(100, Math.round((nativeTokensInput / nativeTokensMax) * 100)) : null;
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
  // First-send session creation persists draft controls before activation. Keep the
  // staged values associated with that exact session until its authoritative load
  // completes so the selector never falls back to a different global model in between.
  const nativeDraftControlsHandoffRef = useRef(null);
  // nativeControls 的会话归属：切会话后、refresh 返回前不展示上一会话的控件值。
  const nativeControlsSessionRef = useRef(null);
  // refreshNativeControls 请求序号：快速切会话时多个 get_* invoke 并发在途，
  // 后发起的请求应胜出。没有它，先发起的慢响应会晚返回并把控件值/归属 ref 覆盖
  // 成旧会话——mode chip 随即显示全局 fallback 而非新会话实测值（串台/陈旧覆盖，
  // 与聊天页 modeState epoch 修复同款竞态）。序号只对发起时仍归属当前会话的请求
  // 发放（claimNativeControlsRefreshId）：已切走的陈旧请求反正会被归属检查丢弃，
  // 占用序号反而会把当前会话在途的权威刷新顶成过期且无人补发。
  const nativeControlsRequestRef = useRef(0);
  // code 会话权限模式全局偏好（{ last_mode, yolo_confirmed }，null=未拉到）：
  // 驱动草稿态/刷新途中的默认 mode 展示，以及首次切 yolo 的一次性确认门。
  const [codePermPrefs, setCodePermPrefs] = useState(null);
  // 待确认的 yolo 切换请求（{ draft }）；非 null 时渲染确认卡。
  const [pendingYoloSwitch, setPendingYoloSwitch] = useState(null);
  const [yoloConfirmBusy, setYoloConfirmBusy] = useState(false);
  // 知识库集合列表与 embedding 安装态由 ComposerKbSelector 内部经 bridge.knowledge
  // （kb_collection_list / kb_model_status，全局只读、不带会话）自行加载，代码页
  // 不再重复拉取（PR #214 统一底栏控件时移除 nativeKb* 本地变量）。
  const nativeProjection = useMemo(
    () => (isNativeAgent ? projectNativeLane(activeNativeLane, activeId) : null),
    // nativeLaneTick 是 lane 内容变化的版本号（lane 本体是可变对象，靠 tick 触发重投影）。
    // eslint-disable-next-line react-hooks/exhaustive-deps -- tick is the version counter of the mutable lane object; it must stay in deps to trigger re-projection
    [isNativeAgent, activeNativeLane, activeId, nativeLaneTick],
  );
  const visibleTurns = useMemo(
    () => (isNativeAgent
      ? (nativeProjection ? nativeProjection.turns : EMPTY_CONVERSATION_TURNS)
      : projection.turns),
    [isNativeAgent, nativeProjection, projection.turns],
  );
  const busy = isNativeAgent
    ? Boolean(activeNativeLane && activeNativeLane.busy)
    : projection.turns.some(turn => turn.status === 'running');
  // 「回退到第 N 轮」入口（仅原生代码车道）：checkpoint 列表 + turn 边界对齐。
  // 回退编排（rewind_to_turn）由 confirmRewind 发起；成功后走既有 loadSession
  // 重载（磁盘对话已截断、engine 已被后端回收重注水）。refreshKey 含 busy 边沿：
  // 新 turn 的快照在 turn 开始时由 Rust 写入，发送瞬间的拉取可能拿到旧列表，
  // busy→idle（turn 完成）重拉后入口变体才收敛（联调 Bug A）。ACP 车道与聊天页
  // 不渲染入口（设计 §7）。
  const rewindCheckpoints = useSessionCheckpoints({
    sessionId: activeId,
    enabled: isNativeAgent && Boolean(activeId),
    refreshKey: checkpointRefreshKey({ turnCount: visibleTurns.length, busy }),
  });
  const rewindEntries = useMemo(
    () => (isNativeAgent ? rewindEntriesByTurnId(visibleTurns, rewindCheckpoints.checkpoints) : new Map()),
    [isNativeAgent, visibleTurns, rewindCheckpoints.checkpoints],
  );
  // 待确认的回退目标（{ keepTurns, checkpoint, conversationOnly }）；非 null 时渲染确认弹窗。
  const [rewindTarget, setRewindTarget] = useState(null);
  const [rewindError, setRewindError] = useState('');
  const [rewinding, setRewinding] = useState(false);
  // 回退/撤销是全局单 flight：in-flight 按 sessionId 记账到 ref（state 只是
  // UI 镜像）。切会话的复位 effect 只清 UI 标志；旧 promise 的 finally 仅在
  // ref 仍指向本次调用时才清——否则 A 在途时切 B 发起回退，A settle 的
  // finally 会把 B 的守卫标志抹掉（评审 M2）。
  const rewindInFlightRef = useRef(null);
  const rewindUndoInFlightRef = useRef(null);
  // 「撤销回退」：入口可见性由 rewindCheckpoints.undoState（rewind_undo_state）
  // 驱动，null 不渲染。确认弹窗由本地条目副本 rewindUndoEntry 驱动（打开时快照
  // undoState）——后端可反悔状态会因 refreshKey 边沿/记录消费随时变 null，若
  // 弹窗直接挂在其上，「撤销已生效但重载失败」的重试窗口会被边沿击穿（弹窗关、
  // 错误吞、重试通道丢）。reloadFailed 重试期内弹窗与 undoState 生命周期解耦。
  const rewindUndoState = rewindCheckpoints.undoState;
  const [rewindUndoEntry, setRewindUndoEntry] = useState(null);
  const [rewindUndoError, setRewindUndoError] = useState('');
  const [rewindUndoing, setRewindUndoing] = useState(false);
  useEffect(() => {
    // 可反悔状态消失（回退后发了新轮/记录被消费）时收回弹窗；reloadFailed
    // 重试窗口豁免（弹窗由本地副本驱动，重试只补重载、不再发 undo）。
    if (!rewindUndoAvailable(rewindUndoState)) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- one-shot mirror of the undo-state lifecycle; same pattern as the session-switch reset below
      setRewindUndoEntry(current => (current && current.reloadFailed ? current : null));
    }
  }, [rewindUndoState]);
  // Equivalent to [...visibleTurns].reverse().find(status === 'running'): scan backwards for the last
  // running turn, memoized on the turns reference (both turns branches come from memoized projections,
  // and the draft empty state uses the module-level constant array), so no reversed copy is rebuilt per render.
  const activeConversationTurn = useMemo(() => {
    for (let index = visibleTurns.length - 1; index >= 0; index -= 1) {
      if (visibleTurns[index].status === 'running') return visibleTurns[index];
    }
    return null;
  }, [visibleTurns]);
  // 原生车道底栏控件的展示值（归属保护：refresh 返回前按默认/暂存显示；
  // 默认 = 全局 code_last_mode，从未用过 code 模式 → Plan 只读）。
  const nativeDraftControlsHandoff = nativeDraftControlsHandoffRef.current?.sessionId === activeId
    ? nativeDraftControlsHandoffRef.current.controls
    : null;
  const nativeModeValue = resolveNativeModeValue({
    activeId,
    controlsSessionId: nativeControlsSessionRef.current,
    controlsMode: nativeControls.mode,
    draftMode: nativeDraftControls.mode,
    handoffMode: nativeDraftControlsHandoff?.mode,
    prefs: codePermPrefs,
  });
  const nativeModelChoices = visibleUserModels((bs && bs.savedModels) || [])
    .map(model => ({ value: model.id, name: selectorMainLabel(model, t) || model.id }));
  const nativeSessionModelId = resolveNativeModelId({
    activeId,
    controlsSessionId: nativeControlsSessionRef.current,
    controlsModelId: nativeControls.modelId,
    draftModelId: nativeDraftControls.modelId,
    handoffModelId: nativeDraftControlsHandoff?.modelId,
  });
  const nativeMountedId = activeId
    ? (nativeControlsSessionRef.current === activeId
      ? nativeControls.mountedId
      : (nativeDraftControlsHandoff?.mountedId ?? null))
    : (nativeDraftControls.mountedId ?? null);
  const nativeMultiAgentSelected = activeId
    ? (nativeControlsSessionRef.current === activeId
      ? Boolean(nativeControls.multiAgent)
      : Boolean(nativeDraftControlsHandoff?.multiAgent))
    : Boolean(nativeDraftControls.multiAgent);
  // Existing sessions use the backend SessionPolicy result. A Pinvou draft is
  // known to become a native Code session, so it may stage the same control
  // before a session id exists.
  const nativeMultiAgentAvailable = activeId
    ? (nativeControlsSessionRef.current === activeId && Boolean(nativeControls.multiAgentAvailable))
    : isNativeAgent;
  const nativeMultiAgentEnabled = nativeMultiAgentAvailable && nativeMultiAgentSelected;
  const activeAgentName = activeSession?.agent_name
    || agents?.find(agent => agent.agent_id === activeAgentId)?.agent_name
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
    if (workspaceOpen && workspaceDockActive) {
      setWorkspaceOpen(false);
      return;
    }
    setWorkspaceOpen(true);
    setWorkspaceDockActivation(value => value + 1);
  }, [rememberScrollBeforeRightPanelChange, workspaceDockActive, workspaceOpen]);
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
    lastScrollHeightRef.current = element.scrollHeight;
    if (snapshot.stickToBottom) {
      autoScrollRef.current = true;
      setShowScrollBottom(false);
    }
  }, [subagentPanel, workspaceDockActivation, workspaceOpen]);
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously collapse the subagent panel on session switch; one-shot mirror
    setSubagentPanel(null);
    setRewindTarget(null);
    setRewindError('');
    setRewindUndoEntry(null);
    setRewindUndoError('');
    // in-flight 标志一并复位：回退/撤销进行中切会话时，旧 promise 的 UI 写入
    // 已被 activeIdRef 守卫拦住，标志留着只会把新会话的入口一直禁用。
    setRewinding(false);
    setRewindUndoing(false);
  }, [activeId]);
  useEffect(() => {
    if (typeof window === 'undefined' || !isNativeAgent) return;
    const onOpen = (event) => {
      const detail = event && event.detail;
      const sessionId = detail && detail.sessionId;
      if (!detail?.agentId || !activeIdRef.current) return;
      if (sessionId && sessionId !== activeIdRef.current) return;
      rememberScrollBeforeRightPanelChange();
      setSubagentPanel(current => ({
        agentId: detail.agentId,
        selectionRequestId: (current?.selectionRequestId || 0) + 1,
      }));
    };
    window.addEventListener('pinvou:open-subagent', onOpen);
    return () => window.removeEventListener('pinvou:open-subagent', onOpen);
  }, [isNativeAgent, rememberScrollBeforeRightPanelChange]);
  const { acceptStatus, refreshStatus } = useAcpAgentStatus(activeAgentIdRef, setStatus);
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
  const deviceFileUploadAvailable = can('deviceFileUpload');
  const runtimeManagementAvailable = [
    'install_acp_agent',
    'login_acp_agent',
    'switch_acp_agent_account',
  ].every(canInvoke);
  const providerManagementAvailable = [
    'list_acp_providers',
    'set_codex_acp_session_provider',
  ].every(canInvoke);

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

  function beginAcpConfigOperation(sessionId, key) {
    const operation = acpConfigOperationTracker.begin(sessionId, key);
    setAcpConfigOperation(operation);
    return operation;
  }

  function canApplyAcpConfigOperation(operation) {
    return operation?.sessionId === activeIdRef.current
      && acpConfigOperationTracker.isCurrent(operation);
  }

  function finishAcpConfigOperation(operation) {
    if (!acpConfigOperationTracker.finish(operation)) return;
    setAcpConfigOperation(current => current?.token === operation.token ? null : current);
  }

  function beginAcpSendOperation(sessionId) {
    const operation = acpSendOperationTracker.begin(sessionId || DRAFT_ATTACHMENT_KEY, 'send');
    setAcpSendOperation(operation);
    return operation;
  }

  function canApplyAcpSendOperation(operation) {
    return operation?.sessionId === (activeIdRef.current || DRAFT_ATTACHMENT_KEY)
      && acpSendOperationTracker.isCurrent(operation);
  }

  function finishAcpSendOperation(operation) {
    if (!acpSendOperationTracker.finish(operation)) return;
    setAcpSendOperation(current => current?.token === operation.token ? null : current);
  }

  function stageDraftConfigSelection(patch) {
    setDraftConfigSelections(current => {
      const prev = current[draftAgentId] || {};
      const next = {
        model: patch.model === undefined ? prev.model : patch.model,
        mode: patch.mode === undefined ? prev.mode : patch.mode,
        configs: { ...prev.configs, ...patch.configs },
      };
      return { ...current, [draftAgentId]: next };
    });
  }

  // 首次发送创建会话后，把草稿态预选的模型/权限模式/配置应用到新会话。
  // 以新会话实际上报的 config_options 为准自适应：走 config 的项用 set_config_option，
  // 否则退回 set_model/set_mode；与当前值相同或会话未暴露的项跳过。
  async function applyDraftConfigSelections(targetId, info, staged, canReportError = () => true) {
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
            ? await setAcpConfigOption(targetId, 'model', staged.model)
            : await setAcpModel(targetId, staged.model);
        }
      }
      if (staged.mode) {
        const viaConfig = currentOptionValue('mode') !== null;
        const currentValue = viaConfig
          ? currentOptionValue('mode')
          : String(current && current.modes && current.modes.currentModeId || '');
        if (String(staged.mode) !== currentValue) {
          current = viaConfig
            ? await setAcpConfigOption(targetId, 'mode', staged.mode)
            : await setAcpMode(targetId, staged.mode);
        }
      }
      for (const [configId, valueId] of Object.entries(staged.configs || {})) {
        const optionValue = currentOptionValue(configId);
        if (optionValue === null || optionValue === String(valueId)) continue;
        current = await setAcpConfigOption(targetId, configId, valueId);
      }
    } catch (err) {
      if (canReportError()) showError(err);
    }
    return current;
  }

  /// 拉取原生会话的模型/知识库/模式状态（全部 per-session 命令，显式 sessionId）。
  async function refreshNativeControls(sessionId) {
    // 发起时已不归属当前会话的刷新注定被提交前的归属检查丢弃，不得占用序号——
    // 否则它会把当前会话在途的权威刷新顶成过期且无人补发（跨会话抢占）。
    const claimed = claimNativeControlsRefreshId({
      sessionId,
      activeId: activeIdRef.current,
      latestRequestId: nativeControlsRequestRef.current,
    });
    nativeControlsRequestRef.current = claimed.latestRequestId;
    const requestId = claimed.requestId;
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
    if (!canApplyNativeControlsRefresh({
      requestId,
      latestRequestId: nativeControlsRequestRef.current,
      sessionId,
      activeId: activeIdRef.current,
    })) {
      return controls;
    }
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
    // eslint-disable-next-line react-hooks/set-state-in-effect -- fetch global permission prefs once on mount; afterwards refreshed in place by switch/confirm paths
    refreshCodePermPrefs();
    // 仅挂载拉取一次；后续由切换/确认路径就地刷新。
  }, []);

  /// 草稿态暂存的控件选择在新会话上应用；失败报错不静默（逐个应用，多智能体最后）。
  /// 任一步失败即整体失败：清空暂存并上抛，由 sendNative 外层 catch 兜住（会话已创建，
  /// 保留半份暂存会在下次创建会话时把过期的部分选择悄悄应用，形成孤儿暂存）。
  function clearNativeDraftControls(staged) {
    setNativeDraftControls(current => current === staged ? {} : current);
    if (nativeDraftControlsHandoffRef.current?.controls === staged) {
      nativeDraftControlsHandoffRef.current = null;
    }
  }

  async function persistNativeDraftControls(sessionId, staged) {
    // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 floor: Object.hasOwn is unavailable; this call is already the safe form
    const hasMultiAgentSelection = Object.prototype.hasOwnProperty.call(staged, 'multiAgent');
    const hasStaged = staged.modelId || staged.mountedId != null || staged.mode || hasMultiAgentSelection;
    if (!hasStaged) return false;
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
    } catch (err) {
      // 会话已经创建且部分配置可能已生效：清空暂存避免未来复用过期选择，
      // 错误继续上抛，由 sendNative 的 catch 提示用户并恢复输入框文本。
      clearNativeDraftControls(staged);
      throw err;
    }
    return true;
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

  /// Plan↔Yolo 只改变后续 turn 使用的模式。正在运行的 turn 保持提交时捕获
  /// 的模式并继续流式输出，切换权限配置不应隐式取消用户的回答。
  ///
  /// 首次切 yolo 的一次性确认门（全局记忆，产品已拍板）：未确认先弹卡，
  /// 【确认】调 confirm_code_yolo 写入全局标志后按原路径切换；【取消】留在
  /// 当前 mode。确认是 UI 层语义，后端 exit_plan_to_yolo 不强制门控。
  async function switchNativeMode(target, { isPlan } = {}) {
    if (target === 'yolo' && isPlan) {
      const prefs = await refreshCodePermPrefs();
      if (needsYoloConfirmation(prefs)) {
        setPendingYoloSwitch({ draft: !activeId });
        return;
      }
    }
    await performNativeModeSwitch(target, { isPlan });
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
  async function performNativeModeSwitch(target, { isPlan } = {}) {
    if (!activeId) {
      stageDraftMode(target);
      return;
    }
    setError('');
    try {
      if (target === 'plan' && !isPlan) {
        await invoke('set_plan_mode_next', { sessionId: activeId });
      } else if (target === 'yolo' && isPlan) {
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
        await performNativeModeSwitch('yolo', { isPlan: true });
      }
    } catch (err) {
      showError(err);
    } finally {
      setYoloConfirmBusy(false);
    }
  }

  async function refreshSessions() {
    const next = await listAcpSessions();
    const list = next || [];
    if (onSessionsChange) onSessionsChange(list);
    return list;
  }

  async function refreshAgents() {
    return refreshAcpAgentCatalog(setAgents, list => (
      isWeb || list.some(agent => agent?.agent_id === 'pinvou')
        ? list
        : [{ agent_id: 'pinvou', agent_name: '品悟' }, ...list]
    ));
  }

  // 每个 Agent 的 Provider 视图（会话级覆盖下拉与故障引导共用）。
  const [providersViews, setProvidersViews] = useState({});
  async function refreshProviders(agentId = activeAgentId) {
    if (!agentId || !providerManagementAvailable) return null;
    try {
      const next = await invoke('list_acp_providers', { agent: agentId });
      setProvidersViews(current => ({ ...current, [agentId]: next }));
      return next;
    } catch {
      return null;
    }
  }
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously write this agent's Provider view cache on the agent switch edge
    if (activeAgentId) refreshProviders(activeAgentId);
    // activeAgentId 变化时刷新一次即可；切换/回退后由调用方显式刷新。
    // eslint-disable-next-line react-hooks/exhaustive-deps -- refresh only on the agent switch edge; refreshProviders reference changes must not retrigger fetching
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
    const targetAgentId = activeAgentId;
    if (!targetId || configApplying) return;
    const operation = beginAcpConfigOperation(targetId, 'provider');
    if (!operation) return;
    try {
      const next = await invoke('set_codex_acp_session_provider', {
        sessionId: targetId,
        providerId: value === '__official__' ? null : value,
      });
      if (canApplyAcpConfigOperation(operation)) applySessionInfo(next, targetId);
      refreshProviders(targetAgentId);
    } catch (err) {
      if (canApplyAcpConfigOperation(operation)) showError(err);
    } finally {
      finishAcpConfigOperation(operation);
    }
  }

  function selectDraftAgent(agentId) {
    if (activeId || !agentId) return;
    activeAgentIdRef.current = agentId;
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
        loadAcpTimeline(id),
        loadAcpPendingPermissions(id),
        loadAcpPendingElicitations(id),
      ]);
      if (sessionLoadRequestRef.current !== requestId) return null;
      setEvents(current => mergeAcpTimelineSnapshot(timeline, current, id));
      rebaseAcpEventSeqTracker(id, timeline);
      setPending(permissions || []);
      setPendingElicitations(elicitations || []);
      const runtime = await refreshStatus(activeAgentIdRef.current);
      if (sessionLoadRequestRef.current !== requestId) return null;
      if (runtime.installed && runtime.node_supported) {
        try {
          const info = await getAcpSessionInfo(id);
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

  // After merging a snapshot, advance the live-seq baseline to the snapshot's
  // max seq (never regress) so refetches do not misjudge in-order live events
  // as gaps.
  function rebaseAcpEventSeqTracker(sessionId, timeline) {
    const maxSeq = (timeline || []).reduce((max, event) => Math.max(max, Number(event?.seq) || 0), 0);
    acpEventSeqTrackerRef.current.rebase(sessionId, maxSeq);
  }

  // 打开回退确认弹窗；有快照的目标懒加载 diff 预览（「将撤销的变更」摘要）。
  function openRewindDialog(entry) {
    setRewindError('');
    setRewindTarget(entry);
    if (entry.checkpoint) rewindCheckpoints.preview(entry.checkpoint.id);
  }

  // 确认回退：rewind_to_turn 编排（恢复代码 + 截断对话 + engine 回收重注水，
  // 含本会话/跨会话忙碌门）。成功后先重载再收口（联调 Bug B）：loadSession 走
  // 既有路径按磁盘截断后内容重注水，成败都强制 bump tick 兜底重投影；重载
  // 失败留在弹窗如实上屏（弹窗不在重载前关闭，错误不再被吞进不可见状态）。
  // 重载失败时回退已在后端生效：刷新 checkpoint/undo 状态让「撤销回退」入口
  // 出现（用户自救通道），并把目标标记为 reloadFailed——重试只补重载，不会
  // 对已截断的对话再发一次 rewind_to_turn（那会必败且文案令人困惑）。
  async function confirmRewind() {
    const target = rewindTarget;
    if (!target || !activeId || rewinding) return;
    // 单 flight（跨操作类型）：本会话/另一会话的回退或撤销任一在途时不静默
    // 吞点击，如实上屏（评审 M9）；两个 ref 分别记账回退与撤销，后端执行根
    // flag 对跨类型并发兜底拒绝（评审 finding：本地门也按跨类型检查，注释
    // 与行为口径一致）。
    if (rewindInFlightRef.current || rewindUndoInFlightRef.current) {
      setRewindError(codexCopy.rewindInFlightBusy);
      return;
    }
    const sessionId = activeId;
    rewindInFlightRef.current = sessionId;
    setRewinding(true);
    setRewindError('');
    try {
      const result = target.reloadFailed
        ? null
        : await invoke('rewind_to_turn', {
          sessionId,
          keepTurns: target.keepTurns,
          conversationOnly: target.conversationOnly,
        });
      const { error: reloadError } = await reloadSessionAfterRewind({
        // reload 前置归属检查（评审 M4）：回退/撤销在途时用户切到其它会话，
        // loadSession(原会话) 会把 activeIdRef 改回原会话、作废新会话的在途
        // 加载并冻结其流式输出。已切走则跳过重载——磁盘已是目标状态，切回时
        // loadSession 自然重注水；notice 补发走 pendingNotice 暂存。
        reload: () => (activeIdRef.current === sessionId
          ? loadSession(sessionId)
          : Promise.resolve(null)),
        bumpTick: () => setNativeLaneTick(tick => tick + 1),
      });
      // 跨会话竞态：await 期间会话被程序化切换（remote control）时，UI 收口
      // 动作只认原会话；重载/状态刷新已由上面的调用按 sessionId 定向完成。
      // 已知取舍：若重载失败且暂存了 pendingNotice，此早退（或用户切会话触发
      // 的 [activeId] 复位）会丢弃补发——回到该会话时时间线按磁盘重注水，仅
      // 少一条内联提示；undo 侧有 refresh 重查 rewind_undo_state 的自愈兜底，
      // rewind 的 notice 无等价物（仅写内存 lane，不落盘）。而「重载失败+用户
      // 取消重试」路径不补发是有意的：彼时屏上仍是截断前的陈旧时间线，补发
      // 「已回退」反而误导。
      if (activeIdRef.current !== sessionId) return;
      if (reloadError) {
        // 回退已在后端生效：把成功结果随目标暂存（pendingNotice），重试只补
        // 重载、成功后补发提示——避免整条流走完时间线却没有「已回退」确认项。
        // 重试再失败时保留既有暂存（result 为 null 的重试路径不得覆盖）。
        setRewindTarget({
          ...target,
          reloadFailed: true,
          pendingNotice: result
            ? rewindNoticeText(codexCopy, result, target.keepTurns)
            : (target.pendingNotice ?? null),
        });
        setRewindError(reloadError);
        rewindCheckpoints.refresh();
        return;
      }
      setRewindTarget(null);
      const notice = result
        ? rewindNoticeText(codexCopy, result, target.keepTurns)
        : target.pendingNotice;
      if (notice) {
        appendNativeSystemItem(getNativeLane(sessionId), notice);
        setNativeLaneTick(tick => tick + 1);
      }
      // 入口可用性随新时间线重算（refreshKey 的 turns/busy 变化通常已触发，此处兜底）。
      rewindCheckpoints.refresh();
    } catch (err) {
      setRewindError(String(err && err.message ? err.message : err));
    } finally {
      // 仅当 in-flight 记账仍指向本次调用才清除：切会话后旧 promise 的
      // finally 不得抹掉新会话（或新调用）的守卫标志（评审 M2）。
      if (rewindInFlightRef.current === sessionId) {
        rewindInFlightRef.current = null;
        setRewinding(false);
      }
    }
  }

  // 撤销回退：undo_last_rewind（恢复代码到绑定回滚点（仅对话降级则跳过）+
  // 对话从备份还原 + engine 重建）；成功后复用 reloadSessionAfterRewind 重载编排
  // （与回退后同语义：先重载、成败都 bumpTick、成功才关弹窗），失败错误留在弹窗
  // 上屏。弹窗状态走本地副本 rewindUndoEntry：重载失败时撤销已在后端生效（记录
  // 已消费，undoState 随后收敛为 null），把 entry 标记 reloadFailed 留在屏上——
  // 重试只补重载，不会再发一次必败的 undo_last_rewind。
  async function confirmRewindUndo() {
    const entry = rewindUndoEntry;
    if (!entry || !activeId || rewindUndoing) return;
    // 与 confirmRewind 同款跨类型单 flight：回退/撤销任一在途时如实上屏（评审 M9）。
    if (rewindUndoInFlightRef.current || rewindInFlightRef.current) {
      setRewindUndoError(codexCopy.rewindInFlightBusy);
      return;
    }
    const sessionId = activeId;
    const reloadOnly = Boolean(entry.reloadFailed);
    rewindUndoInFlightRef.current = sessionId;
    setRewindUndoing(true);
    setRewindUndoError('');
    try {
      if (!reloadOnly) {
        await invoke('undo_last_rewind', { sessionId });
      }
      const { error: reloadError } = await reloadSessionAfterRewind({
        // reload 前置归属检查（评审 M4）：回退/撤销在途时用户切到其它会话，
        // loadSession(原会话) 会把 activeIdRef 改回原会话、作废新会话的在途
        // 加载并冻结其流式输出。已切走则跳过重载——磁盘已是目标状态，切回时
        // loadSession 自然重注水；notice 补发走 pendingNotice 暂存。
        reload: () => (activeIdRef.current === sessionId
          ? loadSession(sessionId)
          : Promise.resolve(null)),
        bumpTick: () => setNativeLaneTick(tick => tick + 1),
      });
      // 跨会话竞态：await 期间会话被程序化切换时不再写原会话的 UI 状态。
      if (activeIdRef.current !== sessionId) return;
      if (reloadError) {
        setRewindUndoEntry({ ...entry, reloadFailed: true });
        setRewindUndoError(reloadError);
        // 与回退侧对齐：刷新让 undoState 收敛（记录已消费 → null），用户取消
        // 弹窗后「撤销回退」入口不会以陈旧状态残留。弹窗由本地 entry 驱动，
        // 不受 undoState 收敛影响（复位 effect 豁免 reloadFailed 条目）。
        rewindCheckpoints.refresh();
        return;
      }
      setRewindUndoEntry(null);
      appendNativeSystemItem(getNativeLane(sessionId), codexCopy.rewindUndoDone);
      setNativeLaneTick(tick => tick + 1);
      // refresh 连带重查 rewind_undo_state：撤销后不可再反悔，入口随之消失。
      rewindCheckpoints.refresh();
    } catch (err) {
      setRewindUndoError(String(err && err.message ? err.message : err));
    } finally {
      // 与 confirmRewind 同款：in-flight 记账仍指向本次调用才清除。
      if (rewindUndoInFlightRef.current === sessionId) {
        rewindUndoInFlightRef.current = null;
        setRewindUndoing(false);
      }
    }
  }

  // Self-healing for envelope-seq gaps in the web live stream: after the
  // watchdog skips a stalled predecessor, the missing permission/terminal
  // envelopes are not re-delivered within the connection. On a detected gap,
  // debounce-refetch the authoritative timeline and pending state and merge
  // (merge is idempotent for out-of-order/duplicate arrivals), without
  // touching the loading spinner or cancelling an in-flight session-switch
  // load. A failed refetch retries with bounded exponential backoff so a
  // single transient failure cannot permanently disable healing for that gap.
  async function resyncAcpSessionAfterGap(sessionId) {
    const [timeline, permissions, elicitations] = await Promise.all([
      loadAcpTimeline(sessionId),
      loadAcpPendingPermissions(sessionId),
      loadAcpPendingElicitations(sessionId),
    ]);
    if (activeIdRef.current !== sessionId) return;
    setEvents(current => mergeAcpTimelineSnapshot(timeline, current, sessionId));
    rebaseAcpEventSeqTracker(sessionId, timeline);
    setPending(permissions || []);
    setPendingElicitations(elicitations || []);
  }

  function scheduleAcpGapResync(sessionId) {
    acpGapResyncRef.current.schedule(sessionId);
  }

  async function createSession({ shouldActivate = () => true, prepareSession = null } = {}) {
    const requestedWorkspacePath = draftWorkspacePath;
    const requestedWorkspaceHandle = draftWorkspaceHandle;
    const requestedAgentId = draftAgentId;
    setError('');
    setWorkspaceMenuOpen(false);
    const metadata = await createAcpSession({
      workspacePath: requestedWorkspacePath,
      workspaceHandle: requestedWorkspaceHandle,
      agentId: requestedAgentId,
    });
    // loadSession 用 nativeSessionIdsRef 判定分流；新会话先登记，避免它读到旧 prop。
    if (requestedAgentId === 'pinvou') nativeSessionIdsRef.current.add(metadata.id);
    if (requestedWorkspacePath) setRecentWorkspaces(rememberWorkspace(requestedWorkspacePath));
    setDraftWorkspaceHandle(current => (
      current === requestedWorkspaceHandle ? null : current
    ));
    await refreshSessions();
    // Persist native controls before the first load. If persistence fails after the
    // backend session exists, still activate and load it before surfacing the error;
    // the restored composer can then retry in that session instead of creating another.
    return finalizePreparedSessionCreation({
      sessionId: metadata.id,
      prepareSession,
      shouldActivate,
      activateSession: sessionId => {
        skipNextActiveLoadRef.current = sessionId;
        if (onActiveSessionChange) onActiveSessionChange(sessionId);
      },
      loadSession,
      loadInactiveSessionInfo: requestedAgentId === 'pinvou'
        ? null
        : getAcpSessionInfo,
    });
  }

  function beginDraft(
    workspacePath = null,
    { clearComposer = false, workspaceHandle = null } = {},
  ) {
    preserveDraftWorkspaceRef.current = true;
    setWorkspaceMenuOpen(false);
    setDraftWorkspacePath(workspacePath);
    setDraftWorkspaceHandle(workspaceHandle);
    // 选定项目工作区即默认展开工作区面板（无会话也可浏览文件）；临时会话无路径可浏览。
    setWorkspaceOpen(Boolean(workspacePath) && !isWeb);
    if (clearComposer) {
      setDraft('');
      const keysToClear = [
        DRAFT_ATTACHMENT_KEY,
        activeId || lastActiveSessionIdRef.current,
      ].filter(Boolean);
      keysToClear.forEach(key => {
        const attachmentsToClear = attachmentDraftsRef.current[key] || [];
        cancelPendingAcpAttachments(attachmentsToClear, cancelledAttachmentIdsRef.current);
        attachmentsToClear.forEach(attachment => {
          if (attachment.result) discardAcpAttachment(attachment.result).catch(() => {});
        });
      });
      setAttachmentDrafts(current => {
        const next = { ...current };
        keysToClear.forEach(key => { delete next[key]; });
        return next;
      });
      setWorkspaceReferenceDrafts(current => {
        const next = { ...current };
        keysToClear.forEach(key => { delete next[key]; });
        return next;
      });
    } else if (activeId) {
      // The composer moves to the new draft. Browser attachment handles are
      // one-shot resources, so retaining a second owner on the old session
      // would let a consumed handle reappear when the user switches back.
      setAttachmentDrafts(current => {
        const next = { ...current, [DRAFT_ATTACHMENT_KEY]: current[activeId] || [] };
        delete next[activeId];
        return next;
      });
      setWorkspaceReferenceDrafts(current => {
        const next = { ...current, [DRAFT_ATTACHMENT_KEY]: current[activeId] || [] };
        delete next[activeId];
        return next;
      });
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

  async function chooseProjectDraft(defaultPath = null) {
    const selected = await pickAcpWorkspace({
      title: codexCopy.chooseProjectDialog,
      defaultPath,
    });
    if (selected?.path) {
      setRecentWorkspaces(rememberWorkspace(selected.path));
      beginDraft(selected.path, { workspaceHandle: selected.workspaceHandle });
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
    cancelledAttachmentIdsRef.current.delete(id);
    const basename = String(path).split(/[\\/]/).filter(Boolean).pop() || String(path);
    updateAttachments(sessionId, current => [
      ...current,
      { id, basename, status: 'parsing', result: null, error: null },
    ]);
    await runAcpAttachmentTask({
      id,
      cancelledIds: cancelledAttachmentIdsRef.current,
      load: () => ingestAcpAttachmentPath(path),
      discard: discardAcpAttachment,
      onReady: result => setAttachmentDrafts(current => updateAcpAttachmentDraft(
        current,
        id,
        attachment => ({
          ...attachment, basename: result.basename || basename, status: 'ready', result,
        }),
      )),
      onError: err => setAttachmentDrafts(current => updateAcpAttachmentDraft(
        current,
        id,
        attachment => ({ ...attachment, status: 'error', error: String(err) }),
      )),
    });
  }

  async function pickAttachments() {
    setAttachmentMenuOpen(false);
    const selected = await openTauriDialog({
      multiple: true,
      directory: false,
      title: codexCopy.addAttachmentDialog,
    });
    const paths = Array.isArray(selected) ? selected : selected ? [selected] : [];
    await Promise.all(paths.map(path => addAttachmentByPath(path, attachmentKey)));
  }

  function removeAttachment(id) {
    const removed = attachments.find(attachment => attachment.id === id);
    if (isPendingAcpAttachment(removed)) cancelledAttachmentIdsRef.current.add(id);
    else cancelledAttachmentIdsRef.current.delete(id);
    updateAttachments(attachmentKey, current => current.filter(attachment => attachment.id !== id));
    if (removed?.result) discardAcpAttachment(removed.result).catch(() => {});
  }

  async function uploadDeviceFiles(files, sessionId = attachmentKey) {
    // callers pass a FileList (input.files); WebKit's FileList has no Symbol.iterator, so spreading throws TypeError.
    // eslint-disable-next-line unicorn/prefer-spread -- FileList is not iterable on any Safari/WKWebView version
    const selected = Array.from(files || []).filter(Boolean);
    for (const file of selected) {
      const id = `codex-attachment-${++attachmentIdRef.current}`;
      cancelledAttachmentIdsRef.current.delete(id);
      updateAttachments(sessionId, current => [
        ...current,
        {
          id,
          basename: file.name,
          status: 'uploading',
          progress: 0,
          result: null,
          error: null,
        },
      ]);
      await runAcpAttachmentTask({
        id,
        cancelledIds: cancelledAttachmentIdsRef.current,
        load: () => uploadAcpDeviceAttachment(file, {
          isCancelled: () => cancelledAttachmentIdsRef.current.has(id),
          onProgress: progress => setAttachmentDrafts(current => updateAcpAttachmentDraft(
            current, id, attachment => ({ ...attachment, progress }),
          )),
        }),
        discard: discardAcpAttachment,
        onReady: result => setAttachmentDrafts(current => updateAcpAttachmentDraft(
          current,
          id,
          attachment => ({
            ...attachment,
            basename: result.basename || file.name,
            status: 'ready',
            progress: 100,
            result,
          }),
        )),
        onError: err => {
          if (err?.code === 'device_upload_cancelled') return;
          // Desktop integrity failures arrive as a stable wire code in
          // message (transfer.rs); map to the current-language copy instead
          // of passing the raw error through.
          const uploadErrorText = String(err?.message || '');
          const attachmentLimitError = formatAttachmentLimitError(err, t.uiAttachments);
          const displayError = attachmentLimitError || (err?.code === 'device_upload_empty'
              ? t.uiAttachments.deviceUploadEmpty(file.name)
              : err?.code === 'device_upload_unavailable'
                ? t.uiAttachments.deviceUploadUnavailable
                : err?.code === 'device_upload_invalid'
                  ? t.uiAttachments.deviceUploadInvalid(file.name)
                  : uploadErrorText === 'web_attachment_digest_invalid'
                    ? t.uiAttachments.deviceUploadDigestInvalid
                    : uploadErrorText === 'web_attachment_integrity_mismatch'
                      ? t.uiAttachments.deviceUploadIntegrityMismatch
                      : t.uiAttachments.deviceUploadFailed(file.name));
          setAttachmentDrafts(current => updateAcpAttachmentDraft(current, id, attachment => ({
            ...attachment, status: 'error', error: displayError,
          })));
        },
      });
    }
  }

  function chooseDeviceAttachments() {
    setAttachmentMenuOpen(false);
    deviceFileInputRef.current?.click();
  }

  function handleDeviceFilesChosen(event) {
    const files = event.target.files;
    if (files?.length) uploadDeviceFiles(files).catch(showError);
    event.target.value = '';
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
  const nativeVoiceActive = ['requesting_permission', 'recording', 'transcribing'].includes(nativeVoiceInput.status);
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
      if (voice && ['requesting_permission', 'recording', 'transcribing'].includes(voice.status)
        && bridge.available) {
        bridge.voice.cancelVoiceInput();
      }
    };
  }, []);

  function handlePaste(event) {
    // WebKit's DataTransferItemList has no Symbol.iterator; spreading throws TypeError, so Array.from is required.
    // eslint-disable-next-line unicorn/prefer-spread -- DataTransferItemList is not iterable on any Safari/WKWebView version
    const items = Array.from(event.clipboardData && event.clipboardData.items || []);
    const images = items.filter(item => item.type && item.type.startsWith('image/'));
    if (!images.length) return;
    if (!deviceFileUploadAvailable && !canInvoke('save_paste_image')) return;
    event.preventDefault();
    images.forEach(item => {
      const file = item.getAsFile();
      if (!file) return;
      if (deviceFileUploadAvailable) {
        uploadDeviceFiles([file]).catch(showError);
        return;
      }
      // Safari 14 has no Blob#arrayBuffer; the paste-image bridge path keeps FileReader-based reading
      const reader = new FileReader();
      reader.onload = async () => {
        const bytes = [...new Uint8Array(reader.result)];
        const ext = (file.type.split('/')[1] || 'png').replace('jpeg', 'jpg');
        try {
          const path = await invoke('save_paste_image', {
            filename: `paste-${Date.now()}.${ext}`,
            bytes,
          });
          await addAttachmentByPath(path, attachmentKey);
        } catch (err) {
          const limitError = formatAttachmentLimitError(err, t.uiAttachments);
          if (limitError) {
            console.error('Codex paste attachment failed:', err);
            setError(limitError);
          } else {
            showError(err);
          }
        }
      };
      reader.readAsArrayBuffer(file);
    });
  }

  useEffect(() => {
    let disposed = false;
    let unlisten = null;
    Promise.all([refreshAgents(), refreshSessions()]).catch(nextError => {
      if (!disposed) showError(nextError);
    });
    listenTauri('acp:event', message => {
      if (disposed) return;
      const incoming = message.payload;
      // Live seq continuity check runs before merging: after the watchdog
      // skips a stalled predecessor, later events still arrive in order, and
      // appendAcpEvent alone would silently keep the hole; on a detected gap,
      // debounce-refetch the authoritative timeline to self-heal.
      if (incoming && acpEventSeqTrackerRef.current.note(incoming.sessionId, incoming.seq) === 'gap'
          && incoming.sessionId === activeIdRef.current) {
        scheduleAcpGapResync(incoming.sessionId);
      }
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
          getAcpSessionInfo(incoming.sessionId)
            .then(info => applySessionInfo(info, incoming.sessionId))
            .catch(() => {});
        }
      }
    }).then(fn => {
      if (disposed) fn();
      else unlisten = fn;
    }).catch(nextError => {
      if (!disposed) showError(nextError);
    });
    return () => {
      disposed = true;
      acpGapResyncRef.current.cancel();
      if (unlisten) unlisten();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- ACP event subscription mounts once; depending on refresh functions would repeatedly unbind/resubscribe
  }, []);

  useEffect(() => () => {
    const pendingAttachments = Object.values(attachmentDraftsRef.current).flat();
    cancelPendingAcpAttachments(pendingAttachments, cancelledAttachmentIdsRef.current);
    pendingAttachments.forEach(attachment => {
      if (attachment.result) discardAcpAttachment(attachment.result).catch(() => {});
    });
  }, []);

  useEffect(() => onPlatformConnectionChange((connection) => {
    if (connection?.status !== 'connected') return;
    Promise.all([refreshAgents(), refreshSessions()]).catch(error => {
      console.warn('[acp] refresh after remote reconnect failed', error);
    });
    const sessionId = activeIdRef.current;
    if (sessionId) {
      loadSession(sessionId).catch(error => {
        console.warn('[acp] restore authoritative session after reconnect failed', error);
      });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- remote reconnect callback: subscription mounts only once; depending on refresh functions would repeatedly unbind/resubscribe
  }), []);

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
      if (disposed) fns.forEach(fn => { fn(); });
      else unlisteners = fns;
    }).catch(error => console.warn('[codex] native chat events unavailable', error));
    return () => {
      disposed = true;
      unlisteners.forEach(fn => { fn(); });
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- native event subscription mounts once; depending on refresh functions would repeatedly unbind/resubscribe
  }, []);

  useEffect(() => {
    // In draft, read the selected agent's status on demand: switching agents
    // forces a fresh probe (the CLI may have been installed/upgraded outside
    // the app, and the cache would keep a stale not-installed verdict).
    // Sessions with an id are covered by loadSession; skip here to avoid
    // running the CLI/auth probes twice concurrently. Native (pinvou)
    // sessions have no ACP state machine; skip get_acp_agent_status (the
    // backend rejects non-ACP agents).
    if (activeAgentId === 'pinvou') {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- native sessions have no ACP state machine; synchronously clear the status display
      setStatus(null);
      return;
    }
    if (activeId) return;
    refreshStatus(activeAgentId, true).catch(showError);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- probe only on agent/session switch edges; refreshStatus/showError reference changes must not retrigger probing
  }, [activeAgentId, activeId]);

  useEffect(() => {
    const latest = events[events.length - 1];
    if (!isAcpAuthenticationFailure(latest)) return;
    refreshStatus(activeAgentId).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps -- re-check auth only on the event-sequence edge; refreshStatus reference changes must not retrigger probing
  }, [events.length, activeAgentId]);

  // 一次性模型探针：切换/删除 Provider（或恢复官方）后设置页会写探针标记。
  // 草稿态（!activeId）本来不连接 ACP，这里破例主动连接一次，用新 Provider
  // 的真实 session/new 上报覆盖 reseed 的占位快照，之后恢复懒加载。标记先清
  // 再探（一次性、防重入）；失败静默，保留占位快照不影响使用。
  useEffect(() => {
    if (activeId || isNativeAgent) return;
    if (!activeStatus?.installed || !activeStatus?.authenticated) return;
    if (!consumeAcpModelsProbePending(draftAgentId)) return;
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
      // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously reset events/pending/session info when returning to draft; one-shot mirror
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
    // eslint-disable-next-line react-hooks/exhaustive-deps -- load only on the session switch edge; loadSession/showError reference changes must not retrigger loading
  }, [activeId]);

  useEffect(() => {
    if (draftEpochRef.current === draftEpoch) return;
    draftEpochRef.current = draftEpoch;
    beginDraft(null, { clearComposer: true });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- reset only on the draft-epoch edge; beginDraft reference changes must not re-clear the draft
  }, [draftEpoch]);

  useEffect(() => {
    if (!activeStatus?.login_in_progress) return;
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
    // eslint-disable-next-line react-hooks/exhaustive-deps -- poll only on the login-in-progress edge; refreshStatus reference changes must not restart the poll chain
  }, [activeAgentId, activeStatus?.login_in_progress]);

  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- set the elapsed-time baseline immediately, then advance it every second via timer
    setNow(Date.now());
    if (!busy) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [busy]);

  // 切会话/回草稿时关掉记忆弹层（徽标内容按新会话 lane 自动切换）。
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously close the memory popover on session switch; one-shot mirror
    setMemoryOpen(false);
  }, [activeId]);

  useEffect(() => {
    const element = scroller.current;
    if (!element) return;
    const onScroll = () => {
      const transition = transitionConversationScrollState({
        scrollElement: element,
        following: autoScrollRef.current,
        previousScrollTop: lastScrollTopRef.current,
        previousScrollHeight: lastScrollHeightRef.current,
      });
      lastScrollTopRef.current = transition.scrollTop;
      lastScrollHeightRef.current = transition.scrollHeight;
      autoScrollRef.current = transition.following;
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
    // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously hide the scroll-to-bottom button when a session switch resets the scroll baseline
    setShowScrollBottom(false);
    const frame = window.requestAnimationFrame(() => {
      const element = scroller.current;
      if (element) {
        element.scrollTop = element.scrollHeight;
        lastScrollTopRef.current = element.scrollTop;
        lastScrollHeightRef.current = element.scrollHeight;
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [activeId]);

  useEffect(() => {
    const scrollElement = scroller.current;
    const contentElement = conversationContentRef.current;
    if (!scrollElement || !contentElement) return;
    return startConversationBottomFollower({
      scrollElement,
      contentElement,
      isFollowing: () => autoScrollRef.current,
      onMeasured: () => {
        const measurement = measureConversationScrollGeometry({
          scrollElement,
          following: autoScrollRef.current,
          previousScrollTop: lastScrollTopRef.current,
          previousScrollHeight: lastScrollHeightRef.current,
        });
        lastScrollTopRef.current = measurement.scrollTop;
        lastScrollHeightRef.current = measurement.scrollHeight;
      },
      onRestored: (scrollTop) => {
        lastScrollTopRef.current = scrollTop;
        lastScrollHeightRef.current = scrollElement.scrollHeight;
        setShowScrollBottom(false);
      },
    });
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
    const message = !isWeb && codexCopy.showRawErrors
      ? String(nextError)
      : codexCopy.operationFailed;
    setRuntimeErrors(current => ({ ...current, [agentId]: message }));
  }

  async function install(actionOverride = null) {
    const agentId = activeAgentId;
    beginRuntimeOperation(agentId, 'install');
    setError('');
    const stopPolling = startSerialStatusPolling(() => refreshStatus(agentId));
    try {
      const payload = { agent: agentId };
      if (typeof actionOverride === 'string' && actionOverride) payload.action = actionOverride;
      const next = await invoke('install_acp_agent', payload);
      acceptStatus(agentId, next);
    }
    catch (err) { showRuntimeError(agentId, err); }
    finally {
      await stopPolling();
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
      acceptStatus(agentId, next);
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
      acceptStatus(agentId, next);
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
    const attachmentsAtSend = attachments;
    const readyAttachments = attachments.filter(attachment => (
      attachment.status === 'ready' && attachment.result
    ));
    const workspaceReferencesAtSend = workspaceReferences;
    const draftAgentAtSend = draftAgentId;
    const draftConfigAtSend = draftConfigSelections[draftAgentAtSend];
    if ((!message && !readyAttachments.length && !workspaceReferences.length)
      || busy || working || activeRuntimeBusy || configApplying) return;
    if (!isNativeAgent && !activeStatus?.authenticated) {
      setError(codexCopy.loginRequiredBeforeSend);
      return;
    }
    if (attachments.some(attachment => ['parsing', 'uploading'].includes(attachment.status))) {
      setError(codexCopy.attachmentsParsing);
      return;
    }
    if (workspaceUnavailable) return;
    if (activeId && !sessionReady) return;
    if (isNativeAgent) {
      await sendNative(message, readyAttachments);
      return;
    }
    let targetId = activeId;
    let operation = beginAcpSendOperation(targetId);
    if (!operation) return;
    setError('');
    try {
      if (!targetId) {
        const created = await createSession({
          shouldActivate: () => canApplyAcpSendOperation(operation),
        });
        targetId = created.id;
        if (created.activated && activeIdRef.current === targetId) {
          acpSendOperationTracker.switchSession(targetId);
          operation = beginAcpSendOperation(targetId);
        }
        const appliedInfo = await applyDraftConfigSelections(
          targetId,
          created.info,
          draftConfigAtSend,
          () => canApplyAcpSendOperation(operation),
        );
        if (appliedInfo && appliedInfo !== created.info) applySessionInfo(appliedInfo, targetId);
        setDraftConfigSelections(current => {
          if (current[draftAgentAtSend] !== draftConfigAtSend) return current;
          const next = { ...current };
          delete next[draftAgentAtSend];
          return next;
        });
        setAttachmentDrafts(current => transferAcpDraftItems(
          current,
          DRAFT_ATTACHMENT_KEY,
          targetId,
          attachmentsAtSend,
          attachment => attachment.id,
        ));
        setWorkspaceReferenceDrafts(current => transferAcpDraftItems(
          current,
          DRAFT_ATTACHMENT_KEY,
          targetId,
          workspaceReferencesAtSend,
          reference => reference,
        ));
      }
      if (canApplyAcpSendOperation(operation)) {
        autoScrollRef.current = true;
        setShowScrollBottom(false);
        setDraft('');
      }
      await submitAcpPrompt({
        sessionId: targetId,
        message,
        attachments: readyAttachments.map(attachment => attachment.result),
        workspaceReferences: workspaceReferencesAtSend,
      });
      updateAttachments(targetId, current => current.filter(
        attachment => readyAttachments.every(ready => ready.id !== attachment.id),
      ));
      setWorkspaceReferenceDrafts(current => removeAcpDraftItems(
        current,
        targetId,
        workspaceReferencesAtSend,
        reference => reference,
      ));
    } catch (err) {
      if (canApplyAcpSendOperation(operation)) {
        showError(err);
        setDraft(message);
      } else {
        // The user switched sessions before this send failed. The draft
        // belongs to the original session, so keep the new session's UI
        // untouched, but never swallow the failure silently.
        console.error('[codex] background ACP send failed', err);
      }
    } finally {
      finishAcpSendOperation(operation);
    }
  }

  /// 原生（品悟 Engine）发送：草稿态先建会话（强制临时工作区），随后走 chat 命令；
  /// 用户气泡乐观插入 lane，chat 命令同步失败（空消息 / turn 占用等）时回滚。
  async function sendNative(message, readyAttachments) {
    const attachmentsAtSend = attachments;
    const workspaceReferencesAtSend = workspaceReferences;
    const nativeDraftControlsAtSend = nativeDraftControls;
    let targetId = activeId;
    const materializingDraft = !targetId;
    let operation = beginAcpSendOperation(targetId);
    if (!operation) return;
    setError('');
    try {
      if (!targetId) {
        const created = await createSession({
          shouldActivate: () => canApplyAcpSendOperation(operation),
          prepareSession: async sessionId => {
            const prepared = await persistNativeDraftControls(
              sessionId,
              nativeDraftControlsAtSend,
            );
            if (prepared) {
              nativeDraftControlsHandoffRef.current = {
                sessionId,
                controls: nativeDraftControlsAtSend,
              };
            }
          },
        });
        targetId = created.id;
        if (created.activated && activeIdRef.current === targetId) {
          acpSendOperationTracker.switchSession(targetId);
          operation = beginAcpSendOperation(targetId);
        }
        // Activation invalidates the draft-scoped operation token. Report preparation
        // failures only after rebinding to the created session so the visible error path
        // remains current and the user's message is restored for retry in this session.
        if (created.preparationError) throw created.preparationError;
        // prepareSession has already persisted the draft controls before loadSession,
        // and the authoritative load now owns the displayed values. Clear only after
        // that handoff completes so the selected model label remains stable.
        clearNativeDraftControls(nativeDraftControlsAtSend);
        setAttachmentDrafts(current => transferAcpDraftItems(
          current,
          DRAFT_ATTACHMENT_KEY,
          targetId,
          attachmentsAtSend,
          attachment => attachment.id,
        ));
        setWorkspaceReferenceDrafts(current => transferAcpDraftItems(
          current,
          DRAFT_ATTACHMENT_KEY,
          targetId,
          workspaceReferencesAtSend,
          reference => reference,
        ));
      }
      const referenceMentions = workspaceReferencesAtSend.map(path => `@${path}`).join(' ');
      const referencePrefix = workspaceReferencesAtSend.length
        ? `${referenceMentions}\n\n`
        : '';
      const attachmentLead = message ? '\n' : '';
      const displayText = message + (readyAttachments.length
        ? `${attachmentLead}📎 ${readyAttachments.map(attachment => attachment.basename).join(', ')}`
        : '');
      const lane = getNativeLane(targetId);
      const optimisticId = appendLocalUserMessage(lane, displayText);
      setNativeLaneTick(tick => tick + 1);
      if (canApplyAcpSendOperation(operation)) {
        autoScrollRef.current = true;
        setShowScrollBottom(false);
        setDraft('');
      }
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
        // 发送成功 = 新一轮已受理：code scope 未提交的「打开」转正锁死。
        notifyChatRoundCommitted('code');
      } catch (sendError) {
        removeLocalUserMessage(lane, optimisticId);
        setNativeLaneTick(tick => tick + 1);
        throw sendError;
      }
      updateAttachments(targetId, current => current.filter(
        attachment => readyAttachments.every(ready => ready.id !== attachment.id),
      ));
      setWorkspaceReferenceDrafts(current => removeAcpDraftItems(
        current,
        targetId,
        workspaceReferencesAtSend,
        reference => reference,
      ));
    } catch (err) {
      if (materializingDraft) clearNativeDraftControls(nativeDraftControlsAtSend);
      if (canApplyAcpSendOperation(operation)) {
        showError(err);
        setDraft(message);
      } else {
        // The user switched sessions before this send failed. The draft
        // belongs to the original session, so keep the new session's UI
        // untouched, but never swallow the failure silently.
        console.error('[codex] background native send failed', err);
      }
    } finally {
      finishAcpSendOperation(operation);
    }
  }

  async function cancel() {
    if (!activeId) return;
    if (isNativeAgent) {
      await invoke('cancel_generation', { sessionId: activeId }).catch(showError);
      return;
    }
    await cancelAcpSession(activeId).catch(showError);
  }

  /// 原生会话的选择确认卡提交/取消：chat:user_input_required → submit_user_input /
  /// cancel_user_input（显式 sessionId，不经过 bridge 全局 activeSession）。
  async function respondNativeInput(toolCallId, answers) {
    if (!activeId) return;
    // entry 捕获 sid：invoke 挂起期间用户切到别的原生会话时，await 后重新读
    // activeId 会把 restoredAnswers 写进（或找不到卡而漏写）错误 lane——与 bridge
    // submitUserInput 的 sid 捕获同一约定。
    const sid = activeId;
    setRespondingSessionId(sid); setError('');
    try {
      await invoke('submit_user_input', { toolCallId, answers, sessionId: sid });
      markNativeInputResolved(sid, toolCallId, 'submitted', answers);
    } catch (err) {
      if (sid === activeIdRef.current) showError(err);
    } finally {
      setRespondingSessionId(current => current === sid ? null : current);
    }
  }

  async function cancelNativeInput(toolCallId) {
    if (!activeId) return;
    const sid = activeId;
    setRespondingSessionId(sid); setError('');
    try {
      await invoke('cancel_user_input', { toolCallId, sessionId: sid });
      markNativeInputResolved(sid, toolCallId, 'cancelled');
    } catch (err) {
      if (sid === activeIdRef.current) showError(err);
    } finally {
      setRespondingSessionId(current => current === sid ? null : current);
    }
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
      const rawError = String(err && err.message ? err.message : err || '');
      const detail = rawError.includes('session_engine_not_running')
        ? codexCopy.nativeCompactInactive
        : rawError;
      appendNativeSystemItem(lane, `${codexCopy.compactFail}: ${detail}`);
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
      // 接受方案 = 新一轮已受理：code scope 未提交的「打开」转正锁死。
      notifyChatRoundCommitted('code');
    } catch (err) {
      const errorText = String(err && err.message ? err.message : err || '');
      const planNotActive = errorText.includes('plan_not_active');
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
      const planNotActive = errorText.includes('plan_not_active');
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
          streaming={item.status === 'in_progress'}
          onOpenExternal={(url) => invoke('open_user_external_url', { url }).catch(showError)}
          onOpenResource={isWeb ? undefined : openWorkspaceResource}
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
    return null;
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
    const request = pending.find(item => item.toolCallId === toolCallId);
    const targetId = request?.sessionId || activeId;
    if (!targetId || targetId !== activeIdRef.current) return;
    setRespondingSessionId(targetId); setError('');
    try {
      await respondAcpPermission({ sessionId: targetId, toolCallId, optionId });
      setPending(current => current.filter(item => (
        item.sessionId !== targetId || item.toolCallId !== toolCallId
      )));
    } catch (err) {
      if (targetId === activeIdRef.current) showError(err);
    }
    finally {
      setRespondingSessionId(current => current === targetId ? null : current);
    }
  }

  async function respondElicitation(elicitationId, action, content) {
    const request = pendingElicitations.find(item => item.elicitationId === elicitationId);
    const targetId = request?.sessionId || activeId;
    if (!targetId || targetId !== activeIdRef.current) return;
    setRespondingSessionId(targetId); setError('');
    try {
      await respondAcpElicitation({ sessionId: targetId, elicitationId, action, content });
      setPendingElicitations(current => current.filter(
        item => item.sessionId !== targetId || item.elicitationId !== elicitationId,
      ));
    } catch (err) {
      if (targetId === activeIdRef.current) showError(err);
    }
    finally {
      setRespondingSessionId(current => current === targetId ? null : current);
    }
  }

  async function changeModel(modelId) {
    if (!modelId || activeRuntimeBusy || configApplying) return;
    if (!activeId) {
      stageDraftConfigSelection({ model: modelId });
      return;
    }
    const targetId = activeId;
    const operation = beginAcpConfigOperation(targetId, 'model');
    if (!operation) return;
    try {
      const next = await setAcpModel(targetId, modelId);
      if (canApplyAcpConfigOperation(operation)) applySessionInfo(next, targetId);
    } catch (err) {
      if (canApplyAcpConfigOperation(operation)) showError(err);
    } finally {
      finishAcpConfigOperation(operation);
    }
  }

  async function changeConfig(configId, valueId) {
    if (activeRuntimeBusy || configApplying) return;
    if (!activeId) {
      stageDraftConfigSelection({ configs: { [configId]: valueId } });
      return;
    }
    const targetId = activeId;
    const operation = beginAcpConfigOperation(targetId, configId);
    if (!operation) return;
    setError('');
    try {
      const next = await setAcpConfigOption(targetId, configId, valueId);
      if (canApplyAcpConfigOperation(operation)) applySessionInfo(next, targetId);
    } catch (err) {
      if (canApplyAcpConfigOperation(operation)) showError(err);
    } finally {
      finishAcpConfigOperation(operation);
    }
  }

  async function changeMode(modeId) {
    if (!modeId || activeRuntimeBusy || configApplying) return;
    if (!activeId) {
      stageDraftConfigSelection({ mode: modeId });
      return;
    }
    const targetId = activeId;
    const operation = beginAcpConfigOperation(targetId, 'mode');
    if (!operation) return;
    setError('');
    try {
      const next = await setAcpMode(targetId, modeId);
      if (canApplyAcpConfigOperation(operation)) applySessionInfo(next, targetId);
    } catch (err) {
      if (canApplyAcpConfigOperation(operation)) showError(err);
    } finally {
      finishAcpConfigOperation(operation);
    }
  }

  return (
    <div className={`relative h-full min-h-0 flex flex-col ${theme === 'dark' ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>
        <ComposerAttachmentDropOverlay enabled={deviceFileUploadAvailable || (!isWeb && canInvoke('ingest_draft_file_chunk'))} onFiles={files => uploadDeviceFiles(files, attachmentKey)} dark={theme === 'dark'} variant={isWeb ? 'web' : 'desktop'} copy={t.uiAttachments} />
        {activeSession && (
        <header className="h-14 shrink-0 px-5 flex items-center gap-3 border-b border-black/[0.05] dark:border-white/[0.06]">
          <div className="w-8 h-8 rounded-xl bg-black/[0.04] dark:bg-white/[0.08] flex items-center justify-center"><AcpAgentLogo agentId={activeAgentId} className="h-5 w-5" title={activeAgentName} /></div>
          <div className="min-w-0 flex-1">
            <div className="text-[14px] font-semibold">{activeSession.title || 'Codex'}</div>
            <div className={`text-[10px] truncate ${activeSession && !activeSession.workspace_available ? 'text-red-500' : 'text-gray-400'}`}
              title={activeSession && activeSession.workspace_path}>
              {activeAgentName + ' · ' + (activeSession.workspace_kind === 'project' ? activeSession.workspace_path : codexCopy.temporaryWorkspace) + (activeSession.workspace_available ? '' : ' · ' + codexCopy.projectMissing)}
            </div>
          </div>
          {configApplying && <span className="text-[10px] text-blue-500 animate-pulse">{codexCopy.applyingConfig}</span>}
          {busy && <StatusBadge status="running" copy={t.uiConversation} />}
          <button
            type="button"
            onClick={toggleWorkspacePanel}
            className={`h-8 px-2.5 rounded-lg inline-flex items-center gap-1.5 text-[11px] transition-colors ${
              workspaceOpen && workspaceDockActive
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
        {!isWeb && !activeSession && draftWorkspacePath && (
        <header className="h-14 shrink-0 px-5 flex items-center justify-end border-b border-black/[0.05] dark:border-white/[0.06]">
          <button
            type="button"
            data-testid="codex-workspace-toggle"
            onClick={toggleWorkspacePanel}
            className={`h-8 px-2.5 rounded-lg inline-flex items-center gap-1.5 text-[11px] transition-colors ${
              workspaceOpen && workspaceDockActive
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
          <div ref={conversationContentRef} className="w-full max-w-[920px] min-h-full mx-auto px-6 py-6 flex flex-col gap-7">
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
                  managementAvailable={runtimeManagementAvailable}
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
                    managementAvailable={runtimeManagementAvailable}
                    onSwitchAccount={switchAccount}
                    onManageProviders={
                      providerManagementAvailable && onOpenSettingsSection
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
                  <Fragment key={turn.id}>
                    {isNativeAgent && rewindEntries.has(turn.id) && (
                      // 原生车道 turn 边界回退入口：turn N+1 前的 chip =「回退到第 N 轮」；
                      // 无快照的边界为「仅回退对话」变体（rewindEntriesByTurnId 判定）。
                      <RewindChip
                        entry={rewindEntries.get(turn.id)}
                        disabled={busy || rewinding || rewindUndoing}
                        copy={codexCopy}
                        onOpen={openRewindDialog}
                      />
                    )}
                    <ConversationTurn
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
                      onOpenExternal={(url) => openAcpExternalUrl(url).catch(showError)}
                      onOpenResource={isWeb ? undefined : openWorkspaceResource}
                    />
                  </Fragment>
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
                    onOpenExternal={(url) => openAcpExternalUrl(url).catch(showError)}
                    onOpenResource={isWeb ? undefined : openWorkspaceResource} />
                ))}
            {isNativeAgent && rewindUndoAvailable(rewindUndoState) && (
              // 「撤销回退」入口：渲染在时间线末尾（回退成功的内联提示其后），
              // 与 RewindChip 同门控（仅原生代码车道）；undoState 为 null 即消失。
              <RewindUndoChip
                state={rewindUndoState}
                disabled={busy || rewinding || rewindUndoing}
                copy={codexCopy}
                onOpen={(state) => { setRewindUndoError(''); setRewindUndoEntry({ ...state, reloadFailed: false }); }}
              />
            )}
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
                codeAgents={agents}
                codeAgentsLoading={agents === null}
                onCodeAgentChange={selectDraftAgent}
                onManageProviders={
                  providerManagementAvailable && onOpenSettingsSection
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
                formatError={value => (
                  formatAttachmentLimitError(value, t.uiAttachments) || String(value || '')
                )}
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
                      <button type="button" onClick={onGotoSettings} className={`px-2 py-1 rounded-full font-medium ${theme === 'dark' ? 'bg-white/10 hover:bg-white/20' : 'bg-black/5 hover:bg-black/10'}`}>{t.voiceGotoDeps}</button>
                    )}
                    {nativeVoiceInput.status === 'failed' && (
                      <button type="button" onClick={handleNativeVoiceClick} className={`px-2 py-1 rounded-full ${theme === 'dark' ? 'hover:bg-white/10' : 'hover:bg-black/5'}`}>{t.voiceRetry}</button>
                    )}
                    {nativeVoiceActive && (
                      <button type="button" onClick={handleNativeVoiceCancel} className={`px-2 py-1 rounded-full ${theme === 'dark' ? 'hover:bg-white/10' : 'hover:bg-black/5'}`}>{t.voiceCancel}</button>
                    )}
                    {!nativeVoiceActive && (
                      <button type="button" onClick={handleNativeVoiceClose} title={t.voiceClose} className={`w-6 h-6 rounded-full flex items-center justify-center ${theme === 'dark' ? 'hover:bg-white/10' : 'hover:bg-black/5'}`}>×</button>
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
                <div ref={commandMenuPanelRef} className="absolute z-40 left-0 right-0 bottom-full mb-2 max-h-72 overflow-y-auto rounded-2xl border border-black/[0.08] dark:border-white/10 bg-white/95 dark:bg-[#202124]/95 backdrop-blur-xl shadow-xl p-2">
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
                        ref={workspaceMenuTriggerRef}
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
                        <div ref={workspaceMenuPanelRef} className="absolute z-40 bottom-9 left-0 w-[280px] max-w-[calc(100vw-32px)] rounded-2xl border border-black/[0.08] dark:border-white/10 bg-white/95 dark:bg-[#202124]/95 backdrop-blur-xl shadow-xl p-2">
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
                                    onClick={() => {
                                      if (isWeb) chooseProjectDraft(path).catch(showError);
                                      else beginDraft(path);
                                    }}
                                    className="w-full rounded-lg px-3 py-1.5 flex items-center gap-2 text-left hover:bg-black/[0.04] dark:hover:bg-white/[0.06]">
                                    <FolderOpen size={13} className="shrink-0 text-gray-400" />
                                    <span className="truncate text-[11px]">{workspaceName(path, codexCopy.unknownDirectory)}</span>
                                  </button>
                                ))}
                              </div>
                            )}
                        </div>
                      )}
                    </div>
                  )}
                  <div className="relative">
                    <button
                      ref={attachmentMenuTriggerRef}
                      type="button"
                      onClick={() => {
                        if (deviceFileUploadAvailable) setAttachmentMenuOpen(value => !value);
                        else pickAttachments().catch(showError);
                      }}
                      className={COMPOSER_ICON_BUTTON_CLASS}
                      title={codexCopy.addAttachment}
                      aria-label={codexCopy.addAttachment}
                    >
                      <Paperclip size={18} />
                    </button>
                    <input
                      ref={deviceFileInputRef}
                      type="file"
                      multiple
                      className="hidden"
                      data-testid="codex-device-file-input"
                      onChange={handleDeviceFilesChosen}
                    />
                    <ComposerPopover
                      open={deviceFileUploadAvailable && attachmentMenuOpen}
                      onClose={() => setAttachmentMenuOpen(false)}
                      triggerRef={attachmentMenuTriggerRef}
                      compact={false}
                      desktopClassName="absolute bottom-full left-0 mb-2 z-50 w-56 rounded-2xl border border-black/5 bg-white/95 p-1.5 shadow-xl backdrop-blur-xl dark:border-white/10 dark:bg-[#1E1E20]/95"
                    >
                      <button
                        type="button"
                        onClick={chooseDeviceAttachments}
                        className="group flex w-full items-center gap-2.5 rounded-xl px-3 py-2.5 text-[13px] text-gray-700 transition-colors hover:bg-[#007AFF] hover:text-white dark:text-gray-200"
                      >
                        <Upload size={15} className="shrink-0 text-gray-400 group-hover:text-white/90" />
                        {t.attachFromDevice}
                      </button>
                      <button
                        type="button"
                        onClick={() => pickAttachments().catch(showError)}
                        className="group flex w-full items-center gap-2.5 rounded-xl px-3 py-2.5 text-[13px] text-gray-700 transition-colors hover:bg-[#007AFF] hover:text-white dark:text-gray-200"
                      >
                        <Monitor size={15} className="shrink-0 text-gray-400 group-hover:text-white/90" />
                        {t.attachFromHost}
                      </button>
                    </ComposerPopover>
                  </div>
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
                  {/* 斜杠命令菜单依赖 ACP 的 available_commands_update，原生车道不经 ACP、
                      命令永不到达；在原生车道隐藏按钮，避免一个永远禁用且提示会误导的控件。 */}
                  {!isNativeAgent && (
                    <button type="button" ref={commandMenuTriggerRef} onClick={() => setCommandOpen(value => !value)}
                      disabled={!availableCommands.length}
                      className={`h-7 px-2 rounded-lg text-[11px] font-mono hover:bg-black/[0.05] dark:hover:bg-white/[0.07] ${
                        // 父容器是 text-gray-400，可用态必须显式加深，否则与禁用态肉眼无差别。
                        availableCommands.length
                          ? 'font-semibold text-gray-900 dark:text-gray-100'
                          : 'opacity-40'
                      }`}
                      title={availableCommands.length ? codexCopy.commandsAvailable : codexCopy.commandsAfterSession}>/</button>
                  )}
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
                        activeSessionId={activeId}
                      />
                      <ComposerKbSelector
                        t={t}
                        bs={bs}
                        mountedId={nativeMountedId}
                        onMount={mountNativeKb}
                        onUnmount={unmountNativeKb}
                      />
                      {activeId && nativeTokensInput > 0 && (
                        // The usage chip is also the manual-compaction action. Its background
                        // shows the percentage while the tooltip carries the full description.
                        <button
                          type="button"
                          data-testid="native-usage-chip"
                          onClick={() => compactNativeSession().catch(showError)}
                          disabled={busy || working || nativeCompacting}
                          title={codexCopy.nativeUsageTitle(fmtNativeCtxTok(nativeTokensInput), nativeCtxPct)}
                          aria-label={codexCopy.nativeUsageTitle(fmtNativeCtxTok(nativeTokensInput), nativeCtxPct)}
                          className="relative inline-flex h-8 items-center gap-1.5 overflow-hidden rounded-xl border border-black/[0.07] bg-black/[0.025] px-2.5 text-[11px] font-semibold text-[#1F1F1F] transition-all hover:-translate-y-px hover:shadow-sm disabled:cursor-default disabled:opacity-50 dark:border-white/[0.09] dark:bg-white/[0.055] dark:text-[#E8EAED]"
                        >
                          {nativeCtxPct != null && (
                            <span
                              aria-hidden="true"
                              className="absolute inset-y-0 left-0 bg-blue-500/15 dark:bg-blue-400/20"
                              style={{ width: `${nativeCtxPct}%` }}
                            />
                          )}
                          <span className="relative">{nativeCompacting ? codexCopy.compactStart : fmtNativeCtxTok(nativeTokensInput)}</span>
                        </button>
                      )}
                      {nativeMemoryItems.length > 0 && (
                        // 记忆轻量展示：条数徽标 + 点击弹层列出本会话注入的记忆条目
                        // （不照搬 work 的完整记忆面板；无条目时不占位）。
                        <div className="relative min-w-0">
                          <button
                            type="button"
                            ref={memoryTriggerRef}
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
                            <div ref={memoryPanelRef} data-testid="native-memory-panel" className="absolute bottom-full left-0 z-40 mb-2 max-h-72 w-[320px] max-w-[calc(100vw-32px)] overflow-y-auto rounded-2xl border border-black/[0.08] bg-white/95 p-2 shadow-xl backdrop-blur-xl dark:border-white/10 dark:bg-[#202124]/95">
                              <div className="px-2 py-1 text-[10px] uppercase tracking-wider text-gray-400">{codexCopy.nativeMemoryTitle}</div>
                              {nativeMemoryItems.map((item, index) => (
                                <div key={item.id || `memory-${index}`} className="rounded-xl px-3 py-2">
                                  <span className="block text-[10px] font-medium text-gray-400">{nativeMemoryKindLabel(item.kind)}</span>
                                  <span className="mt-0.5 block text-[12px] text-gray-700 dark:text-gray-200">{item.text}</span>
                                </div>
                              ))}
                            </div>
                          )}
                        </div>
                      )}
                    </div>
                  )}
                  {!isNativeAgent && (
                  <div className="relative min-w-0">
                    <button
                      type="button"
                      ref={accountMenuTriggerRef}
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
                      <div
                        ref={accountMenuPanelRef}
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
                            {runtimeManagementAvailable ? (
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
                            ) : (
                              <div className="px-3 py-2 text-[10px] leading-4 text-gray-400">
                                {codexCopy.manageAgentOnDesktop(activeAgentName)}
                              </div>
                            )}
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
                          disabled={busy || working || activeRuntimeBusy || Boolean(configApplying)}
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
                          disabled={busy || working || activeRuntimeBusy || Boolean(configApplying)}
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
                          disabled={busy || working || activeRuntimeBusy || Boolean(configApplying)}
                          title={option.description || option.name}
                          unsetLabel={codexCopy.notSet}
                        />
                      ))}
                      {/* 会话级 Provider 覆盖仅 Codex 生效（spawn 时按会话注入
                          OPENAI_API_KEY）；Claude/Kimi 的 CLI 配置是进程级的，
                          无法按会话隔离，不展示该选项避免误导。 */}
                      {providerManagementAvailable && activeAgentId === 'codex' && Boolean(activeId) && sessionProviderChoices.length > 1 && (
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
                  <button type="button" onClick={cancel} className="w-9 h-9 rounded-full flex items-center justify-center bg-red-500/10 text-red-500 hover:bg-red-500/15"><StopCircle size={18} /></button>
                ) : (
                  <button type="button" onClick={send} disabled={!sessionReady || (!draft.trim() && attachments.every(attachment => attachment.status !== 'ready') && !workspaceReferences.length) || working || activeRuntimeBusy || Boolean(configApplying) || (!isNativeAgent && (!activeStatus || !activeStatus.installed || !activeStatus.authenticated))}
                    className="w-9 h-9 rounded-full flex items-center justify-center bg-[#007AFF] text-white shadow-sm hover:bg-[#006EE6] disabled:bg-black/[0.06] dark:disabled:bg-white/10 disabled:text-gray-400 disabled:shadow-none">
                    <Send size={16} />
                  </button>
                )}
              </div>
            </div>
          </div>
        </div>
        </div>
        {pendingYoloSwitch && (
          // 首次切 yolo 的一次性确认卡（全局记忆）；确认后继续切换，取消留在 Plan。
          // 必须挂在输入框容器外：该容器带 backdrop-blur-xl，会按 Filter Effects L2
          // 成为 fixed 后代的包含块，把全屏模态锁进输入框条内（fixed inset-0 相对它解析）。
          <NativeYoloConfirmCard
            theme={theme}
            t={t}
            busy={yoloConfirmBusy}
            onConfirm={confirmPendingYoloSwitch}
            onCancel={() => setPendingYoloSwitch(null)}
          />
        )}
        {rewindTarget && (
          // 「回退到第 N 轮」确认弹窗：变更摘要（懒加载 diff）+ 对话截断位置 +
          // 错误如实上屏；确认后 confirmRewind 走 rewind_to_turn 编排。
          <RewindConfirmDialog
            entry={rewindTarget}
            previewState={rewindTarget.checkpoint
              ? rewindCheckpoints.previews[rewindTarget.checkpoint.id]
              : null}
            error={rewindError}
            busy={rewinding}
            theme={theme}
            copy={codexCopy}
            onCancel={() => { if (!rewinding) setRewindTarget(null); }}
            onConfirm={confirmRewind}
          />
        )}
        {rewindUndoEntry && (
          // 「撤销回退」轻量确认：说明将恢复代码（有绑定回滚点时）与被截掉的
          // N 轮对话；reloadFailed 时降级为「重试加载」语义。本地副本驱动，
          // 与 undoState 生命周期解耦（见状态声明处注释）。
          <RewindUndoConfirmDialog
            state={rewindUndoEntry}
            error={rewindUndoError}
            busy={rewindUndoing}
            theme={theme}
            copy={codexCopy}
            onCancel={() => { if (!rewindUndoing) setRewindUndoEntry(null); }}
            onConfirm={confirmRewindUndo}
          />
        )}
        {(activeSession || (!isWeb && draftWorkspacePath)) && (
          <CodexWorkspacePanel
            session={activeSession}
            workspacePath={activeSession ? '' : (draftWorkspacePath || '')}
            visible={workspaceOpen}
            activationKey={workspaceDockActivation}
            onActiveChange={setWorkspaceDockActive}
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
