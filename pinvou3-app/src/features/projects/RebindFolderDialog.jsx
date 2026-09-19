// Folder rebind confirmation: shown after the user picked a successor folder
// for a project root that vanished from disk. Purely presentational; the
// container owns picking and the two-phase confirmExisting handshake (first
// attempt without confirmation, backend rejects when the original folder still
// exists, then this dialog escalates to the strong warning).
import { useEffect, useRef } from 'react';
import { createPortal } from 'react-dom';
import { AlertTriangle, RefreshCw, X } from '../../components/icons.jsx';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import { useDialogFocusRestore } from '../../hooks/useDialogFocusRestore.js';
import { useDialogFocusTrap } from '../../hooks/useDialogFocusTrap.js';

const RebindFolderDialog = ({ from, to, warnExisting, errorMessage, partial, busySessionIds, t, busy, restoreTargetRef, onCancel, onConfirm }) => {
  const dialogRef = useRef(null);
  const confirmButtonRef = useRef(null);
  const backdropPressRef = useRef(false);
  // onCancel is an inline arrow at the call site; mirroring it and busy into
  // refs keeps the key listener subscribed once instead of re-subscribing on
  // every render (review #463 minor: keydown resubscribe — same idiom as
  // MoveToProjectDialog's onCloseRef/busyRef).
  const onCancelRef = useRef(onCancel);
  const busyRef = useRef(busy);
  useEffect(() => {
    onCancelRef.current = onCancel;
    busyRef.current = busy;
  });
  // On close, focus returns to the project header row that opened this dialog
  // (review #463 round-10 T7). The badge itself cannot be the target: a
  // successful rebind makes its root available, so the badge is unmounted by
  // the same commit that closes the dialog, and the hook's `isConnected`
  // guard would drop the restore and leave focus on <body>. The container
  // therefore supplies a resolver that re-finds the surviving header (see
  // main.jsx's rebindRestoreRef and the hook's resolver contract).
  useDialogFocusRestore(dialogRef, confirmButtonRef, restoreTargetRef);
  // Tab cycling goes through the shared trap — it holds focus when busy has
  // disabled every control (the hand-rolled trap returned on the empty set
  // and leaked Tab to the background page, review #463 Major 4), handles
  // focus outside the dialog, and guards IME. Only the Escape tiering stays
  // here.
  useDialogFocusTrap(dialogRef);
  useEffect(() => {
    const onKey = (e) => {
      if (e.key === 'Escape' && !isImeComposing(e)) {
        e.preventDefault();
        if (!busyRef.current) onCancelRef.current();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  if (typeof document === 'undefined') return null;

  // Backdrop close requires BOTH the press and the release to land on the
  // backdrop (both halves mirror MoveToProjectDialog's #449 guard): a text
  // drag-select starting on the break-all path text and releasing on the
  // backdrop — or pressing on the backdrop, dragging into the text, and
  // releasing inside — synthesizes a click whose target is the common
  // ancestor (= the backdrop), so a click-only guard would close a dialog
  // the user never meant to close. In the partial state this dialog is the
  // only retry entry, so a stray close loses it (review #463 Major 3).
  const handleBackdropClick = (e) => {
    if (!backdropPressRef.current || e.target !== e.currentTarget) return;
    backdropPressRef.current = false;
    if (!busy) onCancel();
  };

  return createPortal(
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close; keyboard path is the Escape listener and the cancel button
    <div
      role="presentation"
      className="fixed inset-0 z-[200] flex items-center justify-center p-4"
      style={{ background: 'rgba(0,0,0,.34)', backdropFilter: 'blur(14px) saturate(140%)', WebkitBackdropFilter: 'blur(14px) saturate(140%)' }}
      onMouseDown={(e) => { backdropPressRef.current = e.target === e.currentTarget; }}
      onMouseUp={(e) => { if (backdropPressRef.current && e.target !== e.currentTarget) backdropPressRef.current = false; }}
      onClick={handleBackdropClick}
    >
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: dialog body stops bubbling so backdrop close is not triggered accidentally; not interactive itself */}
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-label={t.uiProjects.rebindTitle}
        tabIndex={-1}
        onClick={e => e.stopPropagation()}
        className="w-[380px] max-w-[calc(100vw-48px)] overflow-hidden rounded-[16px] shadow-2xl bg-[rgba(250,250,250,.96)] dark:bg-[rgba(44,44,46,.96)] text-[#000] dark:text-[#F2F2F7]"
        style={{ fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}
        data-testid="rebind-folder-confirm"
      >
        <div className="px-4 pt-4 pb-2 flex items-start justify-between gap-2">
          <div className="flex items-center gap-2 min-w-0">
            <RefreshCw size={17} className="shrink-0 text-[#5F6368] dark:text-[#9AA0A6]" />
            <span className="text-[15px] font-semibold truncate">{t.uiProjects.rebindTitle}</span>
          </div>
          <button
            type="button"
            title={t.cpCancel}
            disabled={busy}
            onClick={onCancel}
            className="w-8 h-8 shrink-0 rounded-full flex items-center justify-center text-[#5F6368] hover:bg-[#D3D7DB] dark:text-[#C4C7C5] dark:hover:bg-[#444746]"
          >
            <X size={16} />
          </button>
        </div>
        <div className="px-4 pb-2 space-y-2 text-[13px]">
          <div className="break-all">
            <span className="text-[#8A8F94] dark:text-[#9AA0A6]">{t.uiProjects.rebindFolder}: </span>
            <span className="line-through decoration-[#C5221F]/70">{from}</span>
          </div>
          <div className="break-all">
            <span className="text-[#8A8F94] dark:text-[#9AA0A6]">→ </span>
            <span className="font-medium break-all">{to}</span>
          </div>
          <div className="text-[12px] text-[#5F6368] dark:text-[#C4C7C5]">
            {t.uiProjects.rebindSessionsHint()}
          </div>
          {warnExisting && (
            <div className="flex items-start gap-2 rounded-2xl bg-[#FCE8E6] dark:bg-[#3C2A29] px-3 py-2 text-[12px] text-[#C5221F] dark:text-[#F28B82]" data-testid="rebind-warn-existing">
              <AlertTriangle size={14} className="shrink-0 mt-0.5" />
              <span>{t.uiProjects.rebindOldExistsWarn}</span>
            </div>
          )}
        </div>
        {/* Partial-failure report (review #463 M1): the dialog does not
            close — the root has already moved, and the unavailable badge
            (the only rebind entry) disappears with the refresh, so the
            retry promise must be honored inside the dialog. Failed session
            ids are data, not UI copy, and are listed verbatim for manual
            follow-up. It also stays open on a post-busy-only report, because
            that is the entry point for the "retry once when idle" remedy
            (round-8 MAJOR-2).
            Known limitation (review #463 round-8 minor 2, dispositioned):
            dismissing this dialog explicitly (Escape / X / Cancel) or
            reloading the page strands the unfinished remainder, because the
            retry lives only in this component's state. Closing that needs a
            persisted pending-rebind record plus a project-level repair entry,
            a feature of its own rather than a fix to this PR. What converges
            afterwards is precisely: rerunning the same from/to (every step is
            idempotent, and already-moved sessions are no-ops). Re-picking a
            DIFFERENT destination does not — a session whose binding lane
            already moved matches neither the old nor the new target — so a
            plain chat whose metadata write failed is retried only by
            returning to the same dialog. */}
        {partial && (
          <div className="px-4 pb-2 space-y-2" data-testid="rebind-partial-report">
            <div className="flex items-start gap-2 rounded-2xl bg-[#FCE8E6] dark:bg-[#3C2A29] px-3 py-2 text-[12px] text-[#C5221F] dark:text-[#F28B82]">
              <AlertTriangle size={14} className="shrink-0 mt-0.5" />
              <div className="space-y-1">
                {/* Post-busy-only runs (nothing failed) reach this state too:
                    the dialog is the entry point for the "retry once when
                    idle" remedy, so the summary must read as a report and not
                    as a failure (round-8 MAJOR-2). In the carryover-refused /
                    budget-exhausted shape (failed=0, rebound=0, postBusy>0)
                    NO summary line renders at all (round-10 minor 6): a
                    "nothing needed rebinding" line directly above "sessions
                    are busy" would deny what the busy line asserts. */}
                {(partial.failed > 0 || partial.rebound > 0 || partial.postBusy === 0) && (
                  <div>
                    {partial.failed > 0
                      ? t.uiProjects.rebindPartial(partial.rebound, partial.failed)
                      : (partial.rebound > 0
                        ? t.uiProjects.rebindSuccess(partial.rebound)
                        : t.uiProjects.rebindUpToDate)}
                  </div>
                )}
                {partial.postBusy > 0 && (
                  <div>{t.uiProjects.rebindBusyAfter(partial.postBusy)}</div>
                )}
              </div>
            </div>
            {partial.failedIds.length > 0 && (
              <div className="space-y-1">
                <div className="text-[12px] text-[#5F6368] dark:text-[#C4C7C5]">
                  {t.uiProjects.rebindFailedSessions}
                </div>
                <div className="max-h-24 overflow-y-auto rounded-xl bg-black/5 dark:bg-white/10 px-2 py-1 font-mono text-[11px] whitespace-pre-wrap break-all">
                  {partial.failedIds.join('\n')}
                </div>
              </div>
            )}
          </div>
        )}
        {/* Busy rejection (Minor 7): the backend rejects with the typed
            REBIND_SESSIONS_BUSY marker and the frontend maps it to i18n
            copy; session ids are data and are listed verbatim for
            troubleshooting. */}
        {busySessionIds && busySessionIds.length > 0 && (
          <div className="px-4 pb-2 space-y-1" data-testid="rebind-busy-hint">
            <div className="flex items-start gap-2 rounded-2xl bg-[#FEF7E0] dark:bg-[#3C3226] px-3 py-2 text-[12px] text-[#B06000] dark:text-[#FDD663]">
              <AlertTriangle size={14} className="shrink-0 mt-0.5" />
              <span>{t.uiProjects.rebindBusyHint}</span>
            </div>
            <div className="max-h-24 overflow-y-auto rounded-xl bg-black/5 dark:bg-white/10 px-2 py-1 font-mono text-[11px] whitespace-pre-wrap break-all">
              {busySessionIds.join('\n')}
            </div>
          </div>
        )}
        <div className="px-4 pb-4 pt-1 flex gap-2">
          {/* Confirm carries only the strong-confirmation state (review #463
              round-8 minor 3). The partial retry used to force
              confirmExisting=true as well, which silently skipped the fence:
              if the original folder reappeared (backup restore, cloud sync)
              the user would never see rebindOldExistsWarn. Every other case is
              behavior-identical without it — a vanished `from` makes the
              backend's confirm check a no-op. */}
          <button
            type="button"
            ref={confirmButtonRef}
            disabled={busy}
            onClick={() => onConfirm(!!warnExisting)}
            className="flex-1 h-10 rounded-full bg-[#0B57D0] text-white text-[14px] font-medium hover:bg-[#0A4CB8] disabled:opacity-50"
          >
            {partial ? t.uiProjects.rebindRetryRemaining : t.uiProjects.rebindConfirm}
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={onCancel}
            className="h-10 px-4 rounded-full bg-[#D3D7DB] dark:bg-[#444746] text-[#1F1F1F] dark:text-[#E3E3E3] text-[14px] font-medium hover:opacity-90 disabled:opacity-50"
          >
            {t.cpCancel}
          </button>
        </div>
        {/* Inline failure rendering (review #463 M7; a Minor round corrected
            the original comment): shown in place, persistent, right next to
            the retry action — a toast would time out and detach from the
            operation. Stacking order is not the motive: settingsToast
            actually renders at z-[210], above this overlay's z-[200]; the
            earlier comment's stacking claim was the reverse of the facts —
            do not base any future layering decisions on it. */}
        {errorMessage && (
          <div className="px-4 pb-4 -mt-1">
            <div className="flex items-start gap-2 rounded-2xl bg-[#FCE8E6] dark:bg-[#3C2A29] px-3 py-2 text-[12px] text-[#C5221F] dark:text-[#F28B82]" data-testid="rebind-error">
              <AlertTriangle size={14} className="shrink-0 mt-0.5" />
              <span className="break-all">{errorMessage}</span>
            </div>
          </div>
        )}
      </div>
    </div>,
    document.body,
  );
};

export { RebindFolderDialog };
