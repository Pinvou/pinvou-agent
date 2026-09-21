// Shared shell for the Codex-family confirm dialogs: portal to <body> (same as the shared
// YoloConfirmCard — keeps the composer container's backdrop-blur from becoming the containing
// block for fixed descendants), focus capture/restore (useDialogFocusRestore), Escape to close
// (disabled while busy), and backdrop buttons disabled together with busy (an in-flight confirmation cannot be dismissed by clicking the backdrop).
// Title/error row/footer are all optional nodes composed by callers: the Rewind confirm/undo dialog
// (RewindChip.jsx) uses the full set, while CodexAcpView's branch-switch dialog uses only the shell + its own panel.

import { useEffect, useRef } from 'react';
import { createPortal } from 'react-dom';
import { useDialogFocusRestore } from '../../hooks/useDialogFocusRestore.js';

// Escape to close (disabled while busy). Shared by the confirm dialogs built
// on ModalDialogShell and CodexAcpView's branch-switch dialog.
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

export function ModalDialogShell({
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
        className={panelClass}
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
