// One-shot confirmation card for the first switch to YOLO (globally remembered,
// confirmed at the UI layer; the backend does not enforce the gate). Shared by
// the code mode (CodexAcpView) and plain-chat bound-workspace sessions
// (ChatView): semantics = "in this mode the model reads/writes the project or
// working directory fully automatically, can run shell commands, with no
// per-step approval". Once confirmed it is remembered globally and never shown
// again. The copy differs per side (project directory vs working directory) and
// is injected via `copy`.
import { useEffect, useRef } from 'react';
import { createPortal } from 'react-dom';

// Button styling is a verbatim mirror of cardBtnCls in
// features/tools/tool-renderers.jsx: the shared layer must not depend back on
// features, so keep both sides in sync when changing either.
function cardBtnCls(variant) {
  const base = 'px-3 py-1.5 rounded-full text-[13px] font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed';
  if (variant === 'danger') return `${base} bg-[#C5221F] text-white hover:bg-[#A50E0E]`;
  return `${base} bg-white text-[#1F1F1F] hover:bg-[#E1E5EA] border border-black/10 dark:border-transparent dark:bg-[#333537] dark:text-[#E3E3E3] dark:hover:bg-[#444746]`;
}

// copy = { title, body, hint, ok, cancel } (the i18n keys differ per side; the caller maps them).
export function YoloConfirmCard({ theme, copy, error, busy, onConfirm, onCancel }) {
  const isDark = theme === 'dark';
  const dialogRef = useRef(null);
  // Grab focus once on mount (keyboard accessibility) and restore the previous
  // focus element on unmount (the trigger element may have been rebuilt with
  // the timeline, hence the isConnected guard). The focus effect has no
  // dependency array entry: the parent's inline onCancel gets a new identity
  // every render, so any parent re-render while open would yank focus from the
  // button back to the container (mirrors useDialogFocusRestore in
  // features/codex/RewindChip.jsx; the shared layer must not depend back on
  // features, so keep both sides in sync when changing either).
  useEffect(() => {
    const previous = document.activeElement;
    dialogRef.current?.focus();
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  }, []);
  // Esc counts as cancel (disabled while busy) — unlike the NativePlanCard
  // inline card this is a full-screen modal that must block the underlying
  // controls. Re-registering the listener is harmless and has no focus side
  // effects.
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
  // Portal to <body>: this card renders inside the composer container, whose
  // backdrop-blur becomes the containing block for `position: fixed`. Without
  // the portal the full-screen modal would only cover the composer area and
  // click-the-backdrop-to-cancel would stop working too.
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
