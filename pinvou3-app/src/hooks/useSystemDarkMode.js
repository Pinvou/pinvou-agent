import { useEffect, useState } from 'react';
import { systemPrefersDark } from '../shared/color-scheme.js';

/**
 * 跟随系统「深浅色」偏好,且监听运行中的变化(不是只读挂载时快照)。
 * colorScheme 为 system 时,系统切换需实时映射到界面主题,故主窗口/撕离窗
 * 常驻订阅;检测不可用时恒为 false(浅色兜底),与 shared/color-scheme.js 同口径。
 */
export function useSystemDarkMode() {
  const [dark, setDark] = useState(systemPrefersDark);

  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return;
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    const onChange = () => setDark(media.matches);
    media.addEventListener?.('change', onChange);
    return () => media.removeEventListener?.('change', onChange);
  }, []);

  return dark;
}
