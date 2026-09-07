// 原生代码会话时间线上的「回退到第 N 轮」入口与确认弹窗。
//
// 每个用户 turn 边界渲染一个 RewindChip（由 CodexAcpView 用 rewindEntriesByTurnId
// 对齐）：点击打开 RewindConfirmDialog，懒加载 checkpoint_diff 展示「将撤销的
// 变更」摘要（计数 + 文件清单，patch 不上屏），并明示对话将截断到的位置；确认后
// 由视图层调 rewind_to_turn 编排（恢复代码 + 截断对话 + engine 回收重注水）。
// 无 Turn 快照的边界是「仅回退对话」变体（conversationOnly），文案明示代码不回退。

import { useEffect, useRef } from 'react';
import { createPortal } from 'react-dom';
import { RotateCcw } from '../../components/icons.jsx';
import { summarizeCheckpointChanges } from './checkpoints.js';

const FILE_LIST_LIMIT = 8;

// 弹窗焦点：挂载时夺取一次（父组件内联 onCancel 每渲染换新身份，若 focus 放进
// 带依赖的 effect，弹窗打开期间任意父级重渲染都会把焦点从按钮拽回容器），卸载
// 时归还先前焦点元素（触发元素可能已随时间线重载重建，isConnected 守卫；
// focus 分离元素是规范允许的 no-op）。两个确认弹窗共用。
function useDialogFocusRestore(dialogRef) {
  useEffect(() => {
    const previous = document.activeElement;
    dialogRef.current?.focus();
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  }, [dialogRef]);
}

// Escape 关闭（busy 时禁用）。两个确认弹窗共用。
function useDialogEscapeKey(busy, onCancel) {
  useEffect(() => {
    const onKey = (event) => {
      if (event.key === 'Escape' && !busy) {
        event.preventDefault();
        onCancel();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [busy, onCancel]);
}

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
// 撤销文案按 state.checkpointId 分流：有绑定回滚点 = 代码+对话一起恢复；
// null = 被撤销的那次回退是仅对话降级（代码未动过），撤销也只还原对话，
// 文案必须如实、不得承诺恢复代码。
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

// 确认弹窗三要素（设计 §7）：将撤销的变更摘要、对话将截断到的位置、错误如实展示
// （跨会话忙碌/恢复失败等后端文案原样上屏）。portal 到 <body> 与 YoloConfirmCard
// 同款：避免 composer 容器的 backdrop-blur 成为 fixed 包含块。
export function RewindConfirmDialog({ entry, previewState, error, busy, theme, copy, onCancel, onConfirm }) {
  const isDark = theme === 'dark';
  const dialogRef = useRef(null);
  useDialogFocusRestore(dialogRef);
  useDialogEscapeKey(busy, onCancel);

  const summary = previewState?.diff ? summarizeCheckpointChanges(previewState.diff.changes) : null;
  const changes = (previewState?.diff && Array.isArray(previewState.diff.changes))
    ? previewState.diff.changes
    : [];
  // reloadFailed = 回退已生效、仅重载失败：预览与截断说明已执行完毕，不再展示，
  // 只保留「重试仅重新加载」的说明，确认键变为「重试加载」。
  const reloadFailed = Boolean(entry.reloadFailed);

  return createPortal(
    <div data-testid="rewind-confirm" className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <button
        type="button"
        aria-label={copy.rewindCancel}
        className="absolute inset-0 cursor-default bg-black/30 backdrop-blur-[2px]"
        disabled={busy}
        onClick={onCancel}
      />
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="rewind-confirm-title"
        tabIndex={-1}
        className={`relative w-full max-w-[440px] rounded-2xl border p-4 shadow-xl backdrop-blur-xl outline-none ${
          isDark ? 'border-white/10 bg-[#202124]/95' : 'border-black/[0.08] bg-white/95'
        }`}
      >
        <div id="rewind-confirm-title" className={`text-[14px] font-semibold ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>
          {copy.rewindDialogTitle}
        </div>

        {reloadFailed ? (
          <div className={`mt-3 text-[12px] leading-5 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>
            {copy.rewindReloadRetryNote}
          </div>
        ) : (
          <>
            <div className="mt-3">
              <div className="text-[10px] font-medium uppercase tracking-wider text-gray-400">
                {copy.rewindChangesToUndo}
              </div>
              {entry.conversationOnly ? (
                <div className={`mt-1.5 text-[12px] leading-5 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>
                  {copy.rewindConversationOnlyNote}
                </div>
              ) : (
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
              )}
            </div>

            <div className="mt-3">
              <div className="text-[10px] font-medium uppercase tracking-wider text-gray-400">
                {copy.rewindConversationLabel}
              </div>
              <div className={`mt-1.5 text-[12px] leading-5 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>
                {copy.rewindConversationTarget(entry.keepTurns)}
              </div>
            </div>

            {!entry.conversationOnly && (
              <div className="mt-3 text-[11px] leading-5 text-gray-400">{copy.rewindPreRestoreNote}</div>
            )}
          </>
        )}

        {error && <div className="mt-3 text-[12px] leading-5 text-red-500">{error}</div>}

        <div className="mt-4 flex items-center justify-end gap-2">
          <button
            type="button"
            data-testid="rewind-confirm-cancel"
            className="rounded-xl px-3 py-1.5 text-[12px] font-medium transition-colors bg-black/[0.06] hover:bg-black/10 disabled:cursor-not-allowed disabled:opacity-45 dark:bg-white/10 dark:hover:bg-white/15"
            disabled={busy}
            onClick={onCancel}
          >{copy.rewindCancel}</button>
          <button
            type="button"
            data-testid="rewind-confirm-ok"
            className="rounded-xl px-3 py-1.5 text-[12px] font-medium text-white transition-colors bg-blue-600 hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-45"
            disabled={busy}
            onClick={onConfirm}
          >{busy ? copy.rewindBusy : reloadFailed ? copy.rewindRetryReload : copy.rewindConfirm}</button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

// 「撤销回退」轻量确认：说明将恢复代码与被截掉的 N 轮对话；错误（忙碌/已不可
// 反悔等后端文案）原样上屏。结构镜像 RewindConfirmDialog。
export function RewindUndoConfirmDialog({ state, error, busy, theme, copy, onCancel, onConfirm }) {
  const isDark = theme === 'dark';
  const dialogRef = useRef(null);
  useDialogFocusRestore(dialogRef);
  useDialogEscapeKey(busy, onCancel);

  return createPortal(
    <div data-testid="rewind-undo-confirm" className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <button
        type="button"
        aria-label={copy.rewindCancel}
        className="absolute inset-0 cursor-default bg-black/30 backdrop-blur-[2px]"
        disabled={busy}
        onClick={onCancel}
      />
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="rewind-undo-confirm-title"
        tabIndex={-1}
        className={`relative w-full max-w-[440px] rounded-2xl border p-4 shadow-xl backdrop-blur-xl outline-none ${
          isDark ? 'border-white/10 bg-[#202124]/95' : 'border-black/[0.08] bg-white/95'
        }`}
      >
        <div id="rewind-undo-confirm-title" className={`text-[14px] font-semibold ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>
          {copy.rewindUndoTitle}
        </div>
        <div className={`mt-3 text-[12px] leading-5 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>
          {state.reloadFailed ? copy.rewindReloadRetryNote : rewindUndoBodyText(copy, state)}
        </div>

        {error && <div className="mt-3 text-[12px] leading-5 text-red-500">{error}</div>}

        <div className="mt-4 flex items-center justify-end gap-2">
          <button
            type="button"
            data-testid="rewind-undo-confirm-cancel"
            className="rounded-xl px-3 py-1.5 text-[12px] font-medium transition-colors bg-black/[0.06] hover:bg-black/10 disabled:cursor-not-allowed disabled:opacity-45 dark:bg-white/10 dark:hover:bg-white/15"
            disabled={busy}
            onClick={onCancel}
          >{copy.rewindCancel}</button>
          <button
            type="button"
            data-testid="rewind-undo-confirm-ok"
            className="rounded-xl px-3 py-1.5 text-[12px] font-medium text-white transition-colors bg-blue-600 hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-45"
            disabled={busy}
            onClick={onConfirm}
          >{busy ? copy.rewindUndoBusy : state.reloadFailed ? copy.rewindRetryReload : copy.rewindUndoConfirm}</button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
