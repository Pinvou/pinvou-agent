import React, { useEffect, useState } from 'react';
import { PinvouLogo } from '../components/PinvouLogo.jsx';
import { tryGetCurrentTauriWindow } from '../platform/tauri/client.js';

const appWindow = tryGetCurrentTauriWindow();
export const TitleBar = ({ theme, t, sidebarOpen = true }) => {
  const isDark = theme === 'dark';
  const hoverBg = isDark ? 'hover:bg-white/10' : 'hover:bg-black/10';
  const titleBarBg = isDark ? (sidebarOpen ? 'bg-[#1E1F20]' : 'bg-[#131314]') : 'bg-[#F0F4F9]';
  // macOS 顶栏走系统原生实现:窗口带 decorations + titleBarStyle=Overlay
  // (见 src-tauri/config/platforms/macos/tauri.conf.json),系统红绿灯悬浮在内容区左上角,
  // 此时不再渲染 Windows 风格三键,并为红绿灯留出左侧空间。
  // 红绿灯纵向位置由 overlay 配置 trafficLightPosition y=20 决定:tao 按
  // "容器高 = 按钮高(14) + y" 布局,按钮顶边距窗口顶 y-9,按钮圆心 7,
  // 故 y=20 时圆心 18,正好与本 h-9(36px)顶栏内容中线对齐;调整顶栏高度需同步改 y。
  // Windows/Linux 窗口无边框(decorations=false),继续用自绘三键。
  // 以窗口实际 decorations 状态为准,不解析 UA / 平台字符串。
  // 初始 null 表示探测未决:此时不渲染自绘三键,避免 macOS 原生顶栏下
  // 三键先闪现一帧再被隐藏;探测失败回退为自绘三键(fail-safe)。
  const [nativeControls, setNativeControls] = useState(null);
  useEffect(() => {
    let cancelled = false;
    if (appWindow && typeof appWindow.isDecorated === 'function') {
      appWindow.isDecorated()
        .then((decorated) => { if (!cancelled) setNativeControls(decorated === true); })
        .catch(() => { if (!cancelled) setNativeControls(false); });
    } else {
      setNativeControls(false);
    }
    return () => { cancelled = true; };
  }, []);
  return (
    <div data-tauri-drag-region
      className={`h-9 shrink-0 flex items-center justify-between select-none ${titleBarBg} ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>
      <div data-tauri-drag-region className={`flex items-center gap-2 ${nativeControls === true ? 'pl-[76px] pr-3' : 'px-3'} text-[13px] font-medium pointer-events-none`}>
        <PinvouLogo className="h-[18px] w-[18px] select-none" />
        {t.appTitle}
      </div>
      {nativeControls === false && (
      <div className="flex items-center h-full">
        <button onClick={() => appWindow && appWindow.minimize()} title={t.winMin}
          className={`h-full w-12 flex items-center justify-center transition-colors ${hoverBg}`}>
          <svg width="11" height="11" viewBox="0 0 11 11"><rect x="1" y="5" width="9" height="1" fill="currentColor"/></svg>
        </button>
        <button onClick={() => appWindow && appWindow.toggleMaximize()} title={t.winMax}
          className={`h-full w-12 flex items-center justify-center transition-colors ${hoverBg}`}>
          <svg width="11" height="11" viewBox="0 0 11 11"><rect x="1.5" y="1.5" width="8" height="8" fill="none" stroke="currentColor" strokeWidth="1"/></svg>
        </button>
        <button onClick={() => appWindow && appWindow.close()} title={t.winClose}
          className="h-full w-12 flex items-center justify-center transition-colors hover:bg-[#E81123] hover:text-white">
          <svg width="11" height="11" viewBox="0 0 11 11"><path d="M1 1 L10 10 M10 1 L1 10" stroke="currentColor" strokeWidth="1.1"/></svg>
        </button>
      </div>
      )}
    </div>
  );
};
