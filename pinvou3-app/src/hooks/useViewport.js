import { useEffect, useState } from 'react';

/**
 * 紧凑视口（<640px，对齐 Tailwind sm 断点与壳层 max-sm 抽屉逻辑）。
 * 监听运行中的变化：旋转屏幕/调窗口即时切换，不是只读挂载时快照。
 */
export function useCompactViewport() {
  const query = '(max-width: 639px)';
  const [compact, setCompact] = useState(() => (
    typeof window.matchMedia === 'function' && window.matchMedia(query).matches
  ));

  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return;
    const media = window.matchMedia(query);
    const onChange = () => setCompact(media.matches);
    media.addEventListener?.('change', onChange);
    return () => media.removeEventListener?.('change', onChange);
  }, []);

  return compact;
}

/**
 * 真实可见视口高度（px）。iOS Safari 上 `100dvh` 会把动态工具栏/安全区算进去，
 * 导致整页比可见区更高、底部（Tab 栏、聊天区尾部）被挤出屏幕、内部滚动失效。
 * visualViewport.height 可靠反映当前可见高度，随工具栏收合、旋转、软键盘弹出实时更新。
 * 不支持 visualViewport 时返回 0，调用方回退到 CSS 的 100dvh。
 */
export function useVisualViewportHeight() {
  const read = () => (
    typeof window !== 'undefined' && window.visualViewport
      ? Math.round(window.visualViewport.height)
      : 0
  );
  const [height, setHeight] = useState(read);

  useEffect(() => {
    const vv = typeof window === 'undefined' ? null : window.visualViewport;
    if (!vv) return;
    const onChange = () => setHeight(Math.round(vv.height));
    vv.addEventListener('resize', onChange);
    vv.addEventListener('scroll', onChange);
    window.addEventListener('resize', onChange);
    window.addEventListener('orientationchange', onChange);
    onChange();
    return () => {
      vv.removeEventListener('resize', onChange);
      vv.removeEventListener('scroll', onChange);
      window.removeEventListener('resize', onChange);
      window.removeEventListener('orientationchange', onChange);
    };
  }, []);

  return height;
}
