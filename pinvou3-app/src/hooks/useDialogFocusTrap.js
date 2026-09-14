// Minimal Tab focus trap shared by modal dialogs (projects move picker,
// voice-shortcut intro). Forward Tab wraps from the last focusable to the
// first, Shift+Tab from the first to the last, and any focus outside the
// dialog (body after an unmount, or a stray click) wraps like the edges. When
// nothing is focusable — e.g. a busy state disabled every control — focus is
// held in place instead of escaping past the aria-modal backdrop. Owns the
// keydown listener; Escape tiering and initial focus/restore stay with the
// consumer (see useDialogFocusRestore for the dismiss recipe).
import { useEffect } from 'react';
import { isImeComposing } from '../shared/ime-guard.mjs';

export function useDialogFocusTrap(dialogRef) {
  useEffect(() => {
    const onKeyDown = (e) => {
      if (e.key !== 'Tab' || isImeComposing(e) || !dialogRef.current) return;
      // 提交中的 busy 态会把所有可聚焦元素禁用:此时必须按住焦点,不能
      // 让 Tab 走到 aria-modal 背板后的页面里。
      const items = [...dialogRef.current.querySelectorAll(
        'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
      )].filter(el => !el.disabled);
      if (!items.length) { e.preventDefault(); return; }
      const first = items[0];
      const last = items[items.length - 1];
      const contained = dialogRef.current.contains(document.activeElement);
      if (e.shiftKey && (!contained || document.activeElement === first)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && (!contained || document.activeElement === last)) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [dialogRef]);
}
