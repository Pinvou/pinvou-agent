import { useEffect } from 'react';

/**
 * Auto-grow a textarea with its content: after each value change set height to auto, then clamp to [min, max] via scrollHeight.
 * ChatView's main input and PetWindow's reply box previously inlined the same effect; both now use this hook.
 * @param {Object} ref - ref to the textarea ({ current: HTMLTextAreaElement|null })
 * @param {string} value - controlled value that triggers recomputation
 * @param {Object} [opts] - { min, max } height clamp (px)
 */
export function useAutoResizeTextarea(ref, value, { min = 48, max = 160 } = {}) {
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(Math.max(el.scrollHeight, min), max)}px`;
  }, [ref, value, min, max]);
}
