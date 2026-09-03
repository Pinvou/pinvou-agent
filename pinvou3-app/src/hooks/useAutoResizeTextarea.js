import { useEffect } from 'react';

/**
 * textarea 随内容自动伸缩：值变化后先置 auto 再按 scrollHeight 收敛到 [min, max]。
 * 此前 ChatView 主输入框与 PetWindow 回复框各自内联同型 effect，收敛到本 hook。
 *
 * @param {{ current: HTMLTextAreaElement|null }} ref
 * @param {string} value - 触发重算的受控值
 * @param {{ min?: number, max?: number }} [opts] - 高度钳制（px）
 */
export function useAutoResizeTextarea(ref, value, { min = 48, max = 160 } = {}) {
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(Math.max(el.scrollHeight, min), max)}px`;
  }, [ref, value, min, max]);
}
