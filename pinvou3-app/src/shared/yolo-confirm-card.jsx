// One-time confirm card for the first YOLO switch (remembered globally; a
// UI-layer confirmation the backend does not enforce). Shared by code mode
// (CodexAcpView) and normal chat's working-directory-bound sessions
// (ChatView): semantics = "in this mode the model reads and writes your
// project/working directory fully automatically and can run shell commands,
// without step-by-step approvals", remembered globally once confirmed. The two
// sides word it differently (project directory vs working directory), injected
// via copy.
import { useEffect, useRef } from 'react';
import { createPortal } from 'react-dom';

// The button styling is a verbatim mirror of cardBtnCls in
// features/tools/tool-renderers.jsx: the shared layer must not depend back on
// features, so changing either side must sync the other.
function cardBtnCls(variant) {
  const base = 'px-3 py-1.5 rounded-full text-[13px] font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed';
  if (variant === 'danger') return `${base} bg-[#C5221F] text-white hover:bg-[#A50E0E]`;
  return `${base} bg-white text-[#1F1F1F] hover:bg-[#E1E5EA] border border-black/10 dark:border-transparent dark:bg-[#333537] dark:text-[#E3E3E3] dark:hover:bg-[#444746]`;
}

// copy = { title, body, hint, ok, cancel } (the two sides use different i18n
// keys; the caller maps them).
export function YoloConfirmCard({ theme, copy, error, busy, onConfirm, onCancel }) {
  const isDark = theme === 'dark';
  const dialogRef = useRef(null);
  // Capture focus once on mount (keyboard reachable) and return it to the
  // previously focused element on unmount (the trigger may have been rebuilt
  // with the timeline; guarded by isConnected). The focus effect has no
  // dependency list: the parent's inline onCancel gets a new identity every
  // render, and any parent re-render while open would yank focus from the
  // button back to the container (mirrors useDialogFocusRestore in
  // features/codex/RewindChip.jsx; the shared layer must not depend back on
  // features, so changing either side must sync the other).
  useEffect(() => {
    const previous = document.activeElement;
    dialogRef.current?.focus();
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  }, []);
  // Esc counts as cancel (disabled while busy) — unlike NativePlanCard's
  // inline card, this is a full-screen modal and must shield the underlying
  // controls. Re-registering the listener is harmless and carries no focus
  // side effects.
  useEffect(() => {
    const onKey = (e) => {
      if (e.key === 'Escape' && !busy) {
        e.preventDefault();
        onCancel();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [busy, onCancel]);
  // Portaled to <body>: the card renders inside the composer container, whose
  // backdrop-blur would become the containing block of `position: fixed`;
  // without the portal the full-screen modal would only cover the input area,
  // and backdrop-click cancellation would break with it.
  return createPortal(
    <div data-testid="native-yolo-confirm" className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <button
        type="button"
        aria-label={copy.cancel}
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
          {copy.title}
        </div>
        <div className={`mt-2 text-[13px] leading-relaxed ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>
          {copy.body}
        </div>
        <div className="mt-2 text-[12px] text-[#C5221F] dark:text-red-400">{copy.hint}</div>
        {error && <div className="mt-1 text-[12px] text-[#C5221F] dark:text-red-400">{error}</div>}
        <div className="mt-4 flex items-center justify-end gap-2">
          <button
            type="button"
            data-testid="native-yolo-confirm-cancel"
            className={cardBtnCls()}
            disabled={busy}
            onClick={onCancel}
          >{copy.cancel}</button>
          <button
            type="button"
            data-testid="native-yolo-confirm-ok"
            className={cardBtnCls('danger')}
            disabled={busy}
            onClick={onConfirm}
          >{copy.ok}</button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
