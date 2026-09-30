// Fork-session dialog (sidebar "more" menu → fork 会话, docs/fork-session-plan
// .md §3.1): pick the workspace plan — share everything, or isolate selected
// keychain roots (primary default-on, attached default-off). While isolating,
// the one-time ownership notice lists the copy paths and states plainly that
// copies are NOT removed when the session is deleted (D6: deletion behavior
// stays globally uniform; ownership transfers at creation time). v1 scope is
// the whole session (message-level keep_turns arrives with v2's turn hover
// action; the backend already accepts the parameter).
//
// The modal recipe (portal / Escape / focus trap / focus restore / two-phase
// backdrop close / busy interception) follows SessionWorkspaceDialog and
// WorkspacePickerDialog; pure plan logic lives in forkDialogState.js.
import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { FolderOpen, X } from '../../components/icons.jsx';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import { useDialogFocusRestore } from '../../hooks/useDialogFocusRestore.js';
import { useDialogFocusTrap } from '../../hooks/useDialogFocusTrap.js';
import { workspaceName } from '../../shared/workspace-recents.js';
import {
  forkConfirmEnabled,
  forkCopyPathPreview,
  forkDialogRoots,
  initialIsolationByRoot,
  selectedIsolateRoots,
} from './forkDialogState.js';

const ForkSessionDialog = ({ open, sessionTitle, workspaceRoots, busy, t, onClose, onConfirm }) => {
  const copy = t.uiForkSession;
  const scopeCopy = t.uiSessionWorkspace;
  const dialogRef = useRef(null);
  const onCloseRef = useRef(onClose);
  const backdropPressRef = useRef(false);
  const busyRef = useRef(false);
  useDialogFocusTrap(dialogRef);
  useDialogFocusRestore(dialogRef, null, null);
  useEffect(() => { onCloseRef.current = onClose; });
  useEffect(() => { busyRef.current = !!busy; }, [busy]);

  const roots = forkDialogRoots(workspaceRoots);
  const [mode, setMode] = useState('share');
  const [byRoot, setByRoot] = useState(() => initialIsolationByRoot(workspaceRoots));

  // Re-seed the toggles whenever the dialog (re)opens for another session:
  // the per-root overrides are per-invocation state, not durable state.
  const openRef = useRef(false);
  useEffect(() => {
    if (open && !openRef.current) {
      setMode('share');
      setByRoot(initialIsolationByRoot(workspaceRoots));
    }
    openRef.current = !!open;
  }, [open, workspaceRoots]);

  const requestClose = () => {
    // Busy interception (WorkspacePickerDialog idiom): a fork in flight must
    // not be dismissed mid-copy — the backend cannot be cancelled halfway.
    if (busyRef.current) return;
    onCloseRef.current();
  };

  useEffect(() => {
    if (!open) return () => {};
    const onKey = (e) => {
      if (e.key === 'Escape' && !isImeComposing(e)) {
        e.preventDefault();
        requestClose();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open]);

  if (!open || typeof document === 'undefined') return null;

  const isolateRoots = selectedIsolateRoots(roots, byRoot);
  const canConfirm = !busy && forkConfirmEnabled(mode, roots, byRoot);
  const radioRow = 'flex items-start gap-2.5 px-3 py-2 rounded-xl cursor-pointer hover:bg-black/[0.03] dark:hover:bg-white/[0.05]';
  const radioDot = (active) => `mt-0.5 w-4 h-4 shrink-0 rounded-full border flex items-center justify-center ${active ? 'border-[#0B57D0]' : 'border-gray-400 dark:border-gray-500'}`;

  return createPortal(
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close; keyboard path is the Escape listener and the close button
    <div
      role="presentation"
      className="fixed inset-0 z-[200] flex items-center justify-center p-4"
      style={{ background: 'rgba(0,0,0,.34)', backdropFilter: 'blur(14px) saturate(140%)', WebkitBackdropFilter: 'blur(14px) saturate(140%)' }}
      onMouseDown={(e) => { backdropPressRef.current = e.target === e.currentTarget; }}
      onMouseUp={(e) => {
        // Two-phase close: only a press that started AND ended on the backdrop closes.
        if (backdropPressRef.current && e.target === e.currentTarget) requestClose();
        backdropPressRef.current = false;
      }}
    >
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: dialog body stops bubbling so backdrop close is not triggered accidentally; not interactive itself */}
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-label={copy.title}
        data-testid="fork-session-dialog"
        className="w-[460px] max-w-[calc(100vw-32px)] rounded-2xl bg-white dark:bg-[#202124] shadow-2xl ring-1 ring-black/10 dark:ring-white/10 p-4"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between mb-1">
          <div className="min-w-0">
            <div className="text-[15px] font-semibold text-[#1F1F1F] dark:text-[#E3E3E3]">{copy.title}</div>
            {sessionTitle && (
              <div className="mt-0.5 truncate text-[11px] text-gray-400">{sessionTitle}</div>
            )}
          </div>
          <button type="button" title={t.cpCancel} onClick={requestClose} disabled={busy}
            className="w-7 h-7 shrink-0 rounded-full flex items-center justify-center text-[#5F6368] hover:bg-black/[0.05] disabled:opacity-50 dark:text-[#C4C7C5] dark:hover:bg-white/[0.08]">
            <X size={15} />
          </button>
        </div>

        <div className="mt-2 mb-1 px-1 text-[10px] uppercase tracking-wider text-gray-400">{copy.scopeLabel}</div>
        <div className="mx-1 rounded-xl bg-black/[0.03] dark:bg-white/[0.05] px-3 py-2 text-[12px] text-gray-500 dark:text-gray-400">
          {copy.scopeFull}
        </div>

        <div className="mt-3 mb-1 px-1 text-[10px] uppercase tracking-wider text-gray-400">{copy.workspaceLabel}</div>
        <label className={radioRow}>
          <span className={radioDot(mode === 'share')}>
            {mode === 'share' && <span className="w-2 h-2 rounded-full bg-[#0B57D0]" />}
          </span>
          <span className="min-w-0">
            <span className="block text-[13px] text-[#1F1F1F] dark:text-[#E3E3E3]">{copy.shareAll}</span>
          </span>
          <input type="radio" name="fork-workspace-mode" className="sr-only" checked={mode === 'share'} disabled={busy}
            onChange={() => setMode('share')} />
        </label>
        <label className={radioRow}>
          <span className={radioDot(mode === 'isolate')}>
            {mode === 'isolate' && <span className="w-2 h-2 rounded-full bg-[#0B57D0]" />}
          </span>
          <span className="min-w-0">
            <span className="block text-[13px] text-[#1F1F1F] dark:text-[#E3E3E3]">{copy.isolate}</span>
            <span className="mt-0.5 block text-[11px] leading-[16px] text-gray-400">{copy.isolateHint}</span>
          </span>
          <input type="radio" name="fork-workspace-mode" className="sr-only" checked={mode === 'isolate'} disabled={busy || roots.length === 0}
            onChange={() => setMode('isolate')} />
        </label>

        {mode === 'isolate' && roots.length > 0 && (
          <div className="mt-1 mx-1 max-h-[30vh] overflow-y-auto" data-testid="fork-session-roots">
            {roots.map((root, index) => (
              <label key={root} title={root} className="w-full px-3 py-2 flex items-center gap-2.5 text-left text-[13px] rounded-xl cursor-pointer hover:bg-black/[0.03] dark:hover:bg-white/[0.05]">
                <input type="checkbox" checked={!!byRoot[root]} disabled={busy}
                  onChange={(e) => setByRoot((prev) => ({ ...prev, [root]: e.target.checked }))}
                  className="w-4 h-4 shrink-0 accent-[#0B57D0]" />
                <FolderOpen size={14} className={`shrink-0 ${index === 0 ? 'text-blue-500' : 'text-gray-400'}`} />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[12px] font-semibold text-[#1F1F1F] dark:text-[#E3E3E3]">
                    {workspaceName(root, scopeCopy.unknownFolder)}
                  </span>
                  <span className="block truncate text-[10px] text-gray-400">{root}</span>
                </span>
                {index === 0 && (
                  <span className="shrink-0 rounded-full bg-blue-50 dark:bg-blue-900/30 px-2 py-0.5 text-[10px] font-medium text-blue-600 dark:text-blue-300">
                    {scopeCopy.primary}
                  </span>
                )}
              </label>
            ))}
          </div>
        )}

        {mode === 'isolate' && isolateRoots.length > 0 && (
          <div data-testid="fork-session-notice" className="mt-2 mx-1 rounded-xl bg-amber-50 dark:bg-amber-900/20 border border-amber-200 dark:border-amber-800/60 px-3 py-2.5">
            <div className="text-[12px] leading-[18px] text-amber-800 dark:text-amber-300">
              {copy.copiesNotice(isolateRoots.length)}
            </div>
            <ul className="mt-1 space-y-0.5">
              {isolateRoots.map((root) => (
                <li key={root} className="truncate font-mono text-[11px] text-amber-900 dark:text-amber-200" title={forkCopyPathPreview(root)}>
                  {forkCopyPathPreview(root)}
                </li>
              ))}
            </ul>
            <div className="mt-1.5 text-[12px] font-medium leading-[18px] text-amber-900 dark:text-amber-200">
              {copy.copiesNoCleanup}
            </div>
          </div>
        )}

        <div className="mt-4 flex gap-2">
          <button type="button" onClick={requestClose} disabled={busy}
            className="flex-1 h-9 rounded-full bg-[#D3D7DB] dark:bg-[#444746] text-[13px] font-medium text-[#1F1F1F] dark:text-[#E3E3E3] hover:opacity-90 disabled:opacity-50">
            {t.cpCancel}
          </button>
          <button type="button" disabled={!canConfirm} data-testid="fork-session-confirm"
            onClick={() => onConfirm(mode === 'isolate' ? isolateRoots : [])}
            className="flex-1 h-9 rounded-full bg-[#0B57D0] text-white text-[13px] font-medium hover:bg-[#0A4CB8] disabled:opacity-50">
            {busy ? copy.busy : copy.create}
          </button>
        </div>
      </div>
    </div>,
    document.body
  );
};

export { ForkSessionDialog };
