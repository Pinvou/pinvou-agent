// Shared centered-dialog shell: portal to <body> (same as the shared YoloConfirmCard — keeps
// a composer container's backdrop-blur from becoming the containing block for fixed descendants),
// focus capture/restore (useDialogFocusRestore), Escape to close (disabled while busy), and
// backdrop buttons disabled together with busy (an in-flight confirmation cannot be dismissed by
// clicking the backdrop). The panel is pinned `relative` so it always stacks above the
// positioned backdrop regardless of caller classes. Title/error row/footer are all optional
// nodes composed by callers: the
// rewind confirm/undo dialogs (codex/RewindChip.jsx) use the full set, CodexAcpView's
// branch-switch dialog uses only the shell + its own panel, and the settings provider
// delete/uninstall/transfer dialogs keep their local wrappers on top of it. Promoted out of
// features/codex/ModalDialogShell.jsx per the 2026-10 reuse audit (its header already said it
// was meant to be shared); the prop surface grew only when a second shape actually needed it:
// `role` (ConfirmDialog renders role=alertdialog — destructive-confirm semantics per the
// ArchiveConfirmDialog reference), `describedBy` (descriptions announced alongside the
// title, scheduled-delete confirm) and `ariaLabel` (last-resort accessible name for
// role-bearing shells whose callers pass no title/description).

import { useRef } from 'react';
import { createPortal } from 'react-dom';
import { useDialogFocusRestore } from '../hooks/useDialogFocusRestore.js';
import { useDialogEscapeKey } from '../hooks/useDialogEscapeKey.js';

export function ModalShell({
  testid,
  zIndexClass = 'z-50',
  backdropClass,
  backdropLabel,
  panelClass,
  busy = false,
  initialFocusRef,
  labelledBy,
  describedBy,
  ariaLabel,
  role = 'dialog',
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
        // Pointer-only affordance (click = cancel): kept out of Tab order so
        // keyboard users walk the real controls instead of landing on a
        // full-viewport invisible button or escaping the modal through it.
        tabIndex={-1}
        disabled={busy}
        onClick={onCancel}
      />
      <div
        ref={dialogRef}
        role={role}
        tabIndex={-1}
        className={`relative ${panelClass}`}
        // The aria attributes ride a conditional spread rather than static JSX
        // attributes: `role` is a prop, so Biome's useAriaPropsSupportedByRole
        // cannot tie static aria-* to a statically-known role and flags them
        // against the div's implicit `generic` role. `dialog` and `alertdialog`
        // support all three; any other role gets none. Undefined labelledBy/
        // describedBy values are omitted by React, so the rendered DOM is
        // identical to static attributes for every consumer.
        {...(role === 'dialog' || role === 'alertdialog'
          // The aria-label arm is the last-resort name (aria-labelledby always
          // wins when present): role-bearing shells must never render unnamed.
          ? { 'aria-modal': 'true', 'aria-labelledby': labelledBy, 'aria-describedby': describedBy, ...(ariaLabel ? { 'aria-label': ariaLabel } : {}) }
          : {})}
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
