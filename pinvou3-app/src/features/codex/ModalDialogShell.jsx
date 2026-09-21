// Codex 系确认弹窗的共享外壳:portal 到 <body>(与共享 YoloConfirmCard 相同——
// 避免 composer 容器的 backdrop-blur 成为 fixed 后代的包含块)、焦点捕获/还原
// (useDialogFocusRestore)、Escape 关闭(busy 时禁用)、背景按钮随 busy 一起
// 禁用(进行中的确认不能靠点击空白处关掉)。
// 标题/错误行/底部按钮均为可选节点,由调用方组合:Rewind 确认/撤销弹窗
// (RewindChip.jsx)用全套,CodexAcpView 的分支切换弹窗只用外壳 + 自有面板。

import { useEffect, useRef } from 'react';
import { createPortal } from 'react-dom';
import { useDialogFocusRestore } from '../../hooks/useDialogFocusRestore.js';

// Escape to close (disabled while busy). Shared by the confirm dialogs built
// on ModalDialogShell and CodexAcpView's branch-switch dialog.
function useDialogEscapeKey(busy, onCancel) {
  useEffect(() => {
    const onKey = (event) => {
      if (event.key === 'Escape' && !busy) {
        event.preventDefault();
        onCancel();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [busy, onCancel]);
}

export function ModalDialogShell({
  testid,
  zIndexClass = 'z-50',
  backdropClass,
  backdropLabel,
  panelClass,
  busy = false,
  initialFocusRef,
  labelledBy,
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
        disabled={busy}
        onClick={onCancel}
      />
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={labelledBy}
        tabIndex={-1}
        className={panelClass}
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
