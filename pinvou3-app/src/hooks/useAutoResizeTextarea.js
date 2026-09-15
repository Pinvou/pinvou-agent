import { useEffect } from 'react';

/**
 * Auto-grow a textarea with its content: after each value change set height to auto, then clamp to [min, max] via scrollHeight.
 * ChatView's main input and PetWindow's reply box previously inlined the same effect; both now use this hook.
 * @param {{ current: HTMLTextAreaElement|null }} ref - ref to the textarea
 * @param {string} value - controlled value that triggers recomputation
 * @param {object} [opts] - height clamp options
 * @param {number} [opts.min] - minimum height in px
 * @param {number} [opts.max] - maximum height in px
 */
export function useAutoResizeTextarea(ref, value, { min = 48, max = 160 } = {}) {
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(Math.max(el.scrollHeight, min), max)}px`;
  }, [ref, value, min, max]);
}
