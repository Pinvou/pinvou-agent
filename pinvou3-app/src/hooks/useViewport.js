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
    if (typeof window.matchMedia !== 'function') return undefined;
    const media = window.matchMedia(query);
    const onChange = () => setCompact(media.matches);
    media.addEventListener?.('change', onChange);
    return () => media.removeEventListener?.('change', onChange);
  }, []);

  return compact;
}
