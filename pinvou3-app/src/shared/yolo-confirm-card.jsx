// 首次切 YOLO 的一次性确认卡（全局记忆，UI 层确认，后端不强制门控）。
// code 模式（CodexAcpView）与普通聊天绑定工作目录会话（ChatView）共用：
// 语义 = "该模式下模型将对你的项目/工作目录全自动读写、可执行 shell，无逐步审批"，
// 确认后全局记住、不再弹。两侧文案不同（项目目录 vs 工作目录），经 copy 注入。
import { useEffect, useRef } from 'react';
import { createPortal } from 'react-dom';

// 按钮样式与 features/tools/tool-renderers.jsx 的 cardBtnCls 同款逐字镜像：
// shared 层不反向依赖 features，改任一侧须同步另一侧。
function cardBtnCls(variant) {
  const base = 'px-3 py-1.5 rounded-full text-[13px] font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed';
  if (variant === 'danger') return `${base} bg-[#C5221F] text-white hover:bg-[#A50E0E]`;
  return `${base} bg-white text-[#1F1F1F] hover:bg-[#E1E5EA] border border-black/10 dark:border-transparent dark:bg-[#333537] dark:text-[#E3E3E3] dark:hover:bg-[#444746]`;
}

// copy = { title, body, hint, ok, cancel }（两侧 i18n 键不同，由调用方映射）。
export function YoloConfirmCard({ theme, copy, busy, onConfirm, onCancel }) {
  const isDark = theme === 'dark';
  const dialogRef = useRef(null);
  // 打开即聚焦卡片（键盘可达），Esc 视为取消——与 NativePlanCard 内联卡不同，
  // 这是一张全屏模态，必须挡住底层控件，故补 role=dialog/aria-modal/键盘交互。
  useEffect(() => {
    dialogRef.current?.focus();
    const onKey = (e) => {
      if (e.key === 'Escape' && !busy) {
        e.preventDefault();
        onCancel();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [busy, onCancel]);
  // portal 到 <body>：该卡片渲染在 composer 容器内，而容器的 backdrop-blur 会成为
  // `position: fixed` 的包含块，不 portal 的话全屏模态只会盖住输入框区域，
  // 点击遮罩取消也随之失效。
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
