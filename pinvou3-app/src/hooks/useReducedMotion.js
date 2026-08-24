import { useEffect, useState } from 'react';

/**
 * 跟随系统「减少动态效果」偏好，且监听运行中的变化（不是只读挂载时快照）。
 * 从 PetWindow 提取为共享 hook：桌宠窗口与设置页宠物选择器共用同一语义。
 */
export function useReducedMotion() {
  const query = '(prefers-reduced-motion: reduce)';
  const [reduced, setReduced] = useState(() => (
    typeof window.matchMedia === 'function' && window.matchMedia(query).matches
  ));

  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return;
    const media = window.matchMedia(query);
    const onChange = () => setReduced(media.matches);
    media.addEventListener?.('change', onChange);
    return () => media.removeEventListener?.('change', onChange);
  }, []);

  return reduced;
}
