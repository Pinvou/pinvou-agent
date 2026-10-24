// Shared centered-dialog shell: portal to <body> (same as the shared YoloConfirmCard — keeps
// a composer container's backdrop-blur from becoming the containing block for fixed descendants),
// focus capture/restore (useDialogFocusRestore), Escape to close (disabled while busy), and
// backdrop buttons disabled together with busy (an in-flight confirmation cannot be dismissed by
// clicking the backdrop). The panel is pinned `relative` so it always stacks above the
// positioned backdrop regardless of caller classes. Title/error row/footer are all optional
// nodes composed by callers: the
// rewind confirm/undo dialogs (features/conversation/RewindChip.jsx, shared by both native
// lanes) use the full set, CodexAcpView's
// branch-switch dialog uses only the shell + its own panel, and the settings provider
// delete/uninstall/transfer dialogs keep their local wrappers on top of it. Promoted out of
// features/codex/ModalDialogShell.jsx per the 2026-10 reuse audit (its header already said it
// was meant to be shared); keep the prop surface minimal until a second shape actually needs it.

import { useEffect, useRef } from 'react';
import { createPortal } from 'react-dom';
import { useDialogFocusRestore } from '../hooks/useDialogFocusRestore.js';
import { isImeComposing } from '../shared/ime-guard.mjs';

// Escape to close (disabled while busy, and ignored during IME composition —
// that Escape cancels the candidate window and must not close the dialog;
// shared/ime-guard.mjs has the macOS WKWebView details). Shared by the confirm
// dialogs built on ModalShell and CodexAcpView's branch-switch dialog.
function useDialogEscapeKey(busy, onCancel) {
  useEffect(() => {
    const onKey = (event) => {
      if (event.key === 'Escape' && !busy && !isImeComposing(event)) {
        event.preventDefault();
        onCancel();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [busy, onCancel]);
}

export function ModalShell({
  testid,
  zIndexClass = 'z-50',
  backdropClass,
  backdropLabel,
  panelClass,
  busy = false,
  initialFocusRef,
  labelledBy,
  title = null,
  error = null,
  footer = null,
  onCancel,
  children,
}) {
  const dialogRef = useRef(null);
  useDialogFocusRestore(dialogRef, initialFocusRef);
  useDialogEscapeKey(busy, onCancel);
  return createPortal(
    <div data-testid={testid} className={`fixed inset-0 ${zIndexClass} flex items-center justify-center p-4`}>
      <button
        type="button"
        aria-label={backdropLabel}
        className={backdropClass}
        disabled={busy}
        onClick={onCancel}
      />
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={labelledBy}
        tabIndex={-1}
        className={`relative ${panelClass}`}
      >
        {title}
        {children}
        {error && <div className="mt-3 text-[12px] leading-5 text-red-500">{error}</div>}
        {footer}
      </div>
    </div>,
    document.body,
  );
}
