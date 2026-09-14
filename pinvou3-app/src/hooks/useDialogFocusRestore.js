// Modal focus handoff, promoted from features/codex (RewindChip) so features
// beyond codex can reuse the same dismiss recipe instead of re-deriving it.
// 弹窗焦点：挂载时夺取一次（父组件内联 onCancel 每渲染换新身份，若 focus 放进
// 带依赖的 effect，弹窗打开期间任意父级重渲染都会把焦点从按钮拽回容器），卸载
// 时归还先前焦点元素（触发元素可能已随时间线重载重建，isConnected 守卫；
// focusing a detached element is a spec-permitted no-op). When initialFocusRef is provided it is
// focused first (e.g. a text box needing immediate input) — this runs after React commits autoFocus
// and overrides it, so initial focus must go through this path instead of the autoFocus attribute.
// Shared by the codex confirm dialogs, NativeYoloConfirmCard, CodexAcpView's
// branch-switch dialog, and the projects move picker.
import { useEffect } from 'react';

export function useDialogFocusRestore(dialogRef, initialFocusRef) {
  useEffect(() => {
    const previous = document.activeElement;
    (initialFocusRef?.current || dialogRef.current)?.focus();
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  }, [dialogRef, initialFocusRef]);
}
