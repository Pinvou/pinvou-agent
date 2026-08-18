import React, { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { isWeb } from '../shared/platform.js';

// 四个输入框下拉菜单（工具 / 知识库 / 模型 / 模式）共用的面板视觉。定位随平台不同，
// 外观一致，抽出来避免四处复制。
const POPOVER_SURFACE =
  'overflow-y-auto custom-scrollbar bg-white/95 dark:bg-[#1E1E20]/95 backdrop-blur-xl ' +
  'border border-black/5 dark:border-white/10 rounded-2xl shadow-xl p-1.5';

// 移动 WebUI 定位：把 fixed 菜单 portal 到 <body>，脱离 composer 祖先的 backdrop-filter
// ——否则该祖先会成为 `position: fixed` 的包含块，让按 getBoundingClientRect 算出的视口
// 坐标被当成 composer 内部坐标，菜单跳位。菜单底边贴在触发按钮上方约 8px，左右各留 12px
// 安全边并向视口内收缩；随窗口缩放 / 旋转 / 软键盘引起的可视区变化实时重算。
function useAnchoredPosition(open, triggerRef, active) {
  const [style, setStyle] = useState(null);
  useLayoutEffect(() => {
    if (!open || !active || !triggerRef.current) {
      setStyle(null);
      return undefined;
    }
    const position = () => {
      const trigger = triggerRef.current;
      if (!trigger) return;
      const rect = trigger.getBoundingClientRect();
      const vw = document.documentElement.clientWidth || window.innerWidth;
      const vh = document.documentElement.clientHeight || window.innerHeight;
      const inset = 12;
      const width = Math.min(288, Math.max(0, vw - inset * 2));
      const desiredLeft = rect.left + rect.width / 2 - width / 2;
      const left = Math.max(inset, Math.min(desiredLeft, vw - width - inset));
      setStyle({
        position: 'fixed',
        left,
        width,
        bottom: Math.max(inset, vh - rect.top + 8),
        maxHeight: Math.max(160, Math.min(420, rect.top - 24)),
        zIndex: 50,
      });
    };
    position();
    window.addEventListener('resize', position);
    window.visualViewport && window.visualViewport.addEventListener('resize', position);
    window.visualViewport && window.visualViewport.addEventListener('scroll', position);
    return () => {
      window.removeEventListener('resize', position);
      window.visualViewport && window.visualViewport.removeEventListener('resize', position);
      window.visualViewport && window.visualViewport.removeEventListener('scroll', position);
    };
  }, [open, active, triggerRef]);
  return style;
}

// 桌面端外点关闭：不能再渲染 `fixed inset-0` 透明关闭层——composer 祖先的
// backdrop-filter 会成为 `position: fixed` 的包含块，使关闭层只铺满输入框盒子，
// 点击弹层外部永远落不到关闭层上。改为 document 级捕获阶段 pointerdown 检测：
// 命中面板或触发按钮（insideRefs）时忽略，否则关闭。触发按钮必须传入，否则
// 弹层打开时再点触发按钮会先被外点关闭、随后 toggle 又把它重新打开。
function useOutsidePointerClose(open, onClose, insideRefs) {
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  const insideRefsRef = useRef(insideRefs);
  insideRefsRef.current = insideRefs;
  useEffect(() => {
    if (!open) return undefined;
    const handlePointerDown = (event) => {
      const refs = insideRefsRef.current || [];
      const inside = refs.some((ref) => ref && ref.current && ref.current.contains(event.target));
      if (!inside) onCloseRef.current();
    };
    document.addEventListener('pointerdown', handlePointerDown, true);
    return () => document.removeEventListener('pointerdown', handlePointerDown, true);
  }, [open]);
}

// composer 下拉外壳。桌面端保持原来的就地 absolute 下拉（外观行为不变）；移动 WebUI
// portal 到 <body> 并按触发按钮真实屏幕位置锚定。`desktopClassName` 是各菜单原有的桌面
// 定位样式，移动端统一用 POPOVER_SURFACE + 计算出的 inline 定位。
const ComposerPopover = ({ open, onClose, triggerRef, compact, desktopClassName, menuProps, children }) => {
  const anchored = isWeb && compact;
  const style = useAnchoredPosition(open, triggerRef, anchored);
  const panelRef = useRef(null);
  useOutsidePointerClose(open && !anchored, onClose, [panelRef, triggerRef]);
  if (!open) return null;
  if (!anchored) {
    return (
      <div {...menuProps} ref={panelRef} className={desktopClassName}>{children}</div>
    );
  }
  return createPortal(
    <>
      <div className="fixed inset-0 z-40" onClick={onClose}></div>
      <div {...menuProps} style={style || { position: 'fixed', visibility: 'hidden' }} className={`fixed ${POPOVER_SURFACE}`}>
        {children}
      </div>
    </>,
    document.body,
  );
};

export { ComposerPopover, POPOVER_SURFACE, useOutsidePointerClose };
