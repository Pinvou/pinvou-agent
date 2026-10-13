import { useMediaQuery } from './useMediaQuery.js';

/**
 * 跟随系统「减少动态效果」偏好，且监听运行中的变化（不是只读挂载时快照）。
 * 从 PetWindow 提取为共享 hook：桌宠窗口与设置页宠物选择器共用同一语义。
 */
export function useReducedMotion() {
  return useMediaQuery('(prefers-reduced-motion: reduce)');
}
