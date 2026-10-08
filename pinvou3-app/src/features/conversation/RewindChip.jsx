// 原生车道会话时间线上的「回退到第 N 轮」入口与确认弹窗（工作模式 + 原生
// code 共用）。
//
// 每个用户 turn 边界渲染一个 RewindChip（由视图用 rewindEntriesByTurnId 对齐）：
// 点击打开 RewindConfirmDialog，懒加载 checkpoint_diff 展示「将撤销的变更」
// 摘要（计数 + 文件清单），并明示对话将截断到的位置；确认后由视图层调
// rewind_to_turn 编排（恢复文件 + 截断对话 + engine 回收重注水）。无 Turn 快照
// 的边界是「仅回退对话」变体（conversationOnly），文案明示文件不回退。
//
// 工作模式变体（scopeSelection）：确认弹窗额外呈现「是否同时回退工作区文件」
// 选择项，默认勾选（勾选 = 双层回退，展示变更预览让代价可见；取消勾选 =
// conversation_only=true 仅截断对话，文件不动）。code 车道不传 scopeSelection，
// 弹窗维持「改动随对话回退」的双层默认，无此选择项。

import { useState } from 'react';
import { RotateCcw } from '../../components/icons.jsx';
import { ModalShell } from '../../components/ModalShell.jsx';
import { summarizeCheckpointChanges } from './checkpoints.js';

const FILE_LIST_LIMIT = 8;

function ChangeSummary({ summary, copy }) {
  if (!summary.total) {
    return <span className="text-gray-400">{copy.rewindNoChanges}</span>;
  }
  const parts = [];
  if (summary.added) parts.push(copy.rewindAdded(summary.added));
  if (summary.modified) parts.push(copy.rewindModified(summary.modified));
  if (summary.deleted) parts.push(copy.rewindDeleted(summary.deleted));
  const rest = summary.renamed + summary.copied + summary.other;
  if (rest) parts.push(copy.rewindOther(rest));
  return <span>{parts.join(' · ')}</span>;
}

function ChangeFileList({ changes, copy }) {
  if (!changes.length) return null;
  const visible = changes.slice(0, FILE_LIST_LIMIT);
  const rest = changes.length - visible.length;
  return (
    <div className="mt-2 max-h-44 overflow-y-auto custom-scrollbar rounded-xl border border-black/[0.05] dark:border-white/[0.07]">
      {visible.map((change, index) => (
        <div key={`${change.path || 'file'}-${index}`}
          className="flex items-center gap-2 border-b border-black/[0.04] px-2.5 py-1.5 text-[11px] last:border-b-0 dark:border-white/[0.05]">
          <span className="shrink-0 rounded-md bg-black/[0.05] px-1.5 py-0.5 text-[10px] text-gray-500 dark:bg-white/[0.08] dark:text-gray-400">
            {copy.rewindStatus[change.status] || copy.rewindStatus.other}
          </span>
          <span className="min-w-0 flex-1 truncate font-mono text-gray-600 dark:text-gray-300" title={change.path}>
            {change.path}
          </span>
        </div>
      ))}
      {rest > 0 && (
        <div className="px-2.5 py-1.5 text-[11px] text-gray-400">{copy.rewindMoreFiles(rest)}</div>
      )}
    </div>
  );
}

export function RewindChip({ entry, disabled, copy, onOpen }) {
  const label = entry.conversationOnly
    ? copy.rewindChipConversationOnly(entry.keepTurns)
    : copy.rewindChip(entry.keepTurns);
  return (
    // Idle state is a faint thin divider line; the whole row is the hover zone,
    // and hovering anywhere near the line fades the line out and the rewind
    // button in. focus-visible keeps the button reachable from the keyboard.
    // pointer-events follow visibility: an opacity-0 button would otherwise
    // still intercept clicks aimed at the timeline content beneath it.
    <div className="group relative my-1 flex h-7 items-center justify-center">
      <div
        aria-hidden="true"
        className="h-px w-24 bg-black/[0.08] transition-opacity group-hover:opacity-0 dark:bg-white/[0.12]"
      />
      <button
        type="button"
        data-testid="rewind-chip"
        disabled={disabled}
        onClick={() => onOpen(entry)}
        title={entry.conversationOnly ? copy.rewindConversationOnlyNote : copy.rewindPreRestoreNote}
        className="pointer-events-none absolute inline-flex max-w-full items-center gap-1.5 rounded-xl border border-black/[0.06] bg-white px-2.5 py-1 text-[11px] text-gray-500 opacity-0 shadow-sm transition-opacity focus:pointer-events-auto focus:opacity-100 group-hover:pointer-events-auto group-hover:opacity-100 focus-visible:pointer-events-auto focus-visible:opacity-100 hover:text-gray-700 disabled:cursor-not-allowed dark:border-white/10 dark:bg-[#2A2B2E] dark:text-gray-400 dark:hover:text-gray-200"
      >
        <RotateCcw size={11} className="shrink-0" />
        <span className="truncate">{label}</span>
      </button>
    </div>
  );
}

// 「撤销回退」入口：渲染在时间线末尾（回退成功的内联提示其后），可见性由
// rewind_undo_state 驱动（null 不渲染，见 checkpoints.js rewindUndoAvailable）。
//
// 撤销文案按 state.checkpointId 分流：有绑定回滚点 = 文件+对话一起恢复；
// null = 被撤销的那次回退是仅对话降级（文件未动过），撤销也只还原对话，
// 文案必须如实、不得承诺恢复文件。
function rewindUndoBodyText(copy, state) {
  return state?.checkpointId
    ? copy.rewindUndoBody(state.rewoundTurns)
    : copy.rewindUndoBodyConversationOnly(state.rewoundTurns);
}

export function RewindUndoChip({ state, disabled, copy, onOpen }) {
  return (
    <div className="my-1 flex justify-center">
      <button
        type="button"
        data-testid="rewind-undo-chip"
        disabled={disabled}
        onClick={() => onOpen(state)}
        title={rewindUndoBodyText(copy, state)}
        className="inline-flex max-w-full items-center gap-1.5 rounded-xl border border-blue-500/25 bg-blue-500/[0.06] px-2.5 py-1 text-[11px] text-blue-600 transition-colors hover:bg-blue-500/10 disabled:cursor-not-allowed disabled:opacity-40 dark:text-blue-300"
      >
        <RotateCcw size={11} className="shrink-0" />
        <span className="truncate">{copy.rewindUndo}</span>
      </button>
    </div>
  );
}

// Shared shell for the two rewind dialogs (confirm / undo-confirm), built on
// the shared ModalShell (portal to <body>, focus capture/restore,
// Escape to close disabled while busy, backdrop button disabled along with
// busy so an in-flight rewind cannot be dismissed by clicking away). The
// identical title line, error line and cancel/confirm footer live here too;
// testids derive from `testid` (…-title / …-cancel / …-ok) so both dialogs
// keep their existing hooks.
function RewindDialogShell({
  testid, isDark, busy, title, error, okLabel, copy, onCancel, onConfirm, children,
}) {
  return (
    <ModalShell
      testid={testid}
      zIndexClass="z-50"
      backdropLabel={copy.rewindCancel}
      backdropClass="absolute inset-0 cursor-default bg-black/30 backdrop-blur-[2px]"
      panelClass={`w-full max-w-[440px] rounded-2xl border p-4 shadow-xl backdrop-blur-xl outline-none ${
        isDark ? 'border-white/10 bg-[#202124]/95' : 'border-black/[0.08] bg-white/95'
      }`}
      busy={busy}
      labelledBy={`${testid}-title`}
      title={(
        <div id={`${testid}-title`} className={`text-[14px] font-semibold ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>
          {title}
        </div>
      )}
      error={error}
      footer={(
        <div className="mt-4 flex items-center justify-end gap-2">
          <button
            type="button"
            data-testid={`${testid}-cancel`}
            className="rounded-xl px-3 py-1.5 text-[12px] font-medium transition-colors bg-black/[0.06] hover:bg-black/10 disabled:cursor-not-allowed disabled:opacity-45 dark:bg-white/10 dark:hover:bg-white/15"
            disabled={busy}
            onClick={onCancel}
          >{copy.rewindCancel}</button>
          <button
            type="button"
            data-testid={`${testid}-ok`}
            className="rounded-xl px-3 py-1.5 text-[12px] font-medium text-white transition-colors bg-blue-600 hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-45"
            disabled={busy}
            onClick={onConfirm}
          >{okLabel}</button>
        </div>
      )}
      onCancel={onCancel}
    >
      {children}
    </ModalShell>
  );
}

// 工作模式「回退范围」选择项：checkbox 默认勾选（推荐值 = 双层回退）；取消
// 勾选时展示「文件保持不动」的中性说明。仅在有快照的边界渲染（无快照边界恒为
// 仅对话回退，选择项无法兑现文件恢复的承诺）。
function RewindScopeToggle({ includeWorkspace, onToggle, copy }) {
  return (
    <div className="mt-3">
      <label
        data-testid="rewind-scope-toggle"
        className="flex cursor-pointer select-none items-start gap-2"
      >
        <input
          type="checkbox"
          checked={includeWorkspace}
          onChange={event => onToggle(event.target.checked)}
          className="mt-0.5 h-3.5 w-3.5 shrink-0 accent-blue-600"
        />
        <span className="text-[12px] leading-5 text-[#444746] dark:text-[#C4C7C5]">
          {copy.rewindScopeWorkspace}
        </span>
      </label>
      {includeWorkspace ? null : (
        <div className="mt-1.5 text-[11px] leading-5 text-gray-400">
          {copy.rewindScopeWorkspaceOffNote}
        </div>
      )}
    </div>
  );
}

// 「将撤销的文件变更」区块的三种形态：无快照边界（固定仅对话说明）、显式排除
// 工作区（中性说明，不出预览）、双层回退（懒加载 diff 预览：计数摘要 + 文件
// 清单，错误如实上屏）。
function RewindChangesBody({ entry, previewState, filesWillRewind, isDark, copy }) {
  if (entry.conversationOnly) {
    return (
      <div className={`mt-1.5 text-[12px] leading-5 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>
        {copy.rewindConversationOnlyNote}
      </div>
    );
  }
  if (!filesWillRewind) {
    return (
      <div className={`mt-1.5 text-[12px] leading-5 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>
        {copy.rewindScopeWorkspaceOffNote}
      </div>
    );
  }
  const summary = previewState?.diff ? summarizeCheckpointChanges(previewState.diff.changes) : null;
  const changes = (previewState?.diff && Array.isArray(previewState.diff.changes))
    ? previewState.diff.changes
    : [];
  return (
    <div className="mt-1.5 text-[12px] leading-5">
      {previewState?.loading && <span className="text-gray-400">{copy.rewindLoading}</span>}
      {previewState?.error && (
        <span className="text-red-500">{copy.rewindPreviewFailed}: {previewState.error}</span>
      )}
      {summary && (
        <>
          <div className={isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}>
            <ChangeSummary summary={summary} copy={copy} />
          </div>
          <ChangeFileList changes={changes} copy={copy} />
        </>
      )}
    </div>
  );
}

// The rewind confirm dialog shows three things (design §7): a summary of the
// changes to be reverted, where the conversation will be truncated to, and
// errors rendered truthfully (backend copy such as cross-session busy or
// restore-failure text appears as-is). Portal to <body>, same as the shared
// YoloConfirmCard: avoids the composer container's backdrop-blur becoming the
// containing block for fixed descendants.
//
// Work-mode variant (`scopeSelection`, design: rewind scope explicitly selectable):
// the changes section is prefixed with a「同时回退工作区文件」checkbox, default
// checked. Checked = double-layer rewind (files + conversation) with the change
// preview visible — the cost must be visible at the decision point because user
// manual edits interleave with agent edits in work mode. Unchecked = only the
// conversation is truncated (conversation_only=true), files stay untouched.
// Boundaries without a snapshot keep the fixed conversation-only variant: the
// checkbox would promise a file restore the backend cannot deliver, so the note
// stays as-is and no checkbox renders. onConfirm receives { includeWorkspace }
// in the scope variant (plain call otherwise) so the controller can translate
// the choice into the conversationOnly flag.
export function RewindConfirmDialog({ entry, previewState, error, busy, theme, copy, onCancel, onConfirm, scopeSelection = false }) {
  const isDark = theme === 'dark';
  // 默认勾选（推荐值）：勾选 = 双层回退。弹窗按 rewindTarget 条件挂载，每次
  // 打开重新初始化；reloadFailed 重试路径复用同一挂载，用户选择不被重置。
  const [includeWorkspace, setIncludeWorkspace] = useState(true);
  // 选择项只在有快照的工作模式边界有意义；无快照边界恒为仅对话回退。
  const scopeChoiceable = scopeSelection && !entry.conversationOnly;
  const filesWillRewind = !entry.conversationOnly && (!scopeChoiceable || includeWorkspace);
  // reloadFailed = 回退已生效、仅重载失败：预览与截断说明已执行完毕，不再展示，
  // 只保留「重试仅重新加载」的说明，确认键变为「重试加载」。
  const reloadFailed = Boolean(entry.reloadFailed);

  const confirmHandler = () => {
    if (scopeChoiceable) onConfirm({ includeWorkspace });
    else onConfirm();
  };

  return (
    <RewindDialogShell
      testid="rewind-confirm"
      isDark={isDark}
      busy={busy}
      title={copy.rewindDialogTitle}
      error={error}
      okLabel={busy ? copy.rewindBusy : reloadFailed ? copy.rewindRetryReload : copy.rewindConfirm}
      copy={copy}
      onCancel={onCancel}
      onConfirm={confirmHandler}
    >
      {reloadFailed ? (
        <div className={`mt-3 text-[12px] leading-5 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>
          {copy.rewindReloadRetryNote}
        </div>
      ) : (
        <>
          {scopeChoiceable && (
            <RewindScopeToggle
              includeWorkspace={includeWorkspace}
              onToggle={setIncludeWorkspace}
              copy={copy}
            />
          )}

          <div className="mt-3">
            <div className="text-[10px] font-medium uppercase tracking-wider text-gray-400">
              {copy.rewindChangesToUndo}
            </div>
            <RewindChangesBody
              entry={entry}
              previewState={previewState}
              filesWillRewind={filesWillRewind}
              isDark={isDark}
              copy={copy}
            />
          </div>

          <div className="mt-3">
            <div className="text-[10px] font-medium uppercase tracking-wider text-gray-400">
              {copy.rewindConversationLabel}
            </div>
            <div className={`mt-1.5 text-[12px] leading-5 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>
              {copy.rewindConversationTarget(entry.keepTurns)}
            </div>
          </div>

          {filesWillRewind && (
            <div className="mt-3 text-[11px] leading-5 text-gray-400">{copy.rewindPreRestoreNote}</div>
          )}
        </>
      )}
    </RewindDialogShell>
  );
}

// 「撤销回退」轻量确认：说明将恢复文件与被截掉的 N 轮对话；错误（忙碌/已不可
// 反悔等后端文案）原样上屏。结构镜像 RewindConfirmDialog。
export function RewindUndoConfirmDialog({ state, error, busy, theme, copy, onCancel, onConfirm }) {
  const isDark = theme === 'dark';
  return (
    <RewindDialogShell
      testid="rewind-undo-confirm"
      isDark={isDark}
      busy={busy}
      title={copy.rewindUndoTitle}
      error={error}
      okLabel={busy ? copy.rewindUndoBusy : state.reloadFailed ? copy.rewindRetryReload : copy.rewindUndoConfirm}
      copy={copy}
      onCancel={onCancel}
      onConfirm={onConfirm}
    >
      <div className={`mt-3 text-[12px] leading-5 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>
        {state.reloadFailed ? copy.rewindReloadRetryNote : rewindUndoBodyText(copy, state)}
      </div>
    </RewindDialogShell>
  );
}
