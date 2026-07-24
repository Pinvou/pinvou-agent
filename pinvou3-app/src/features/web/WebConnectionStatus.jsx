import React, { useEffect, useState } from 'react';
import { isWeb, platform } from '../../shared/platform.js';

const COPY = {
  idle: ['正在准备远程控制', '正在初始化浏览器连接…'],
  connecting: ['正在连接桌面端', '连接中断时会自动重试，尚未确认的操作不会重复执行。'],
  desktop_offline: ['桌面端当前离线', '保持此页面打开；桌面端恢复运行后会自动续接。'],
  credentials_missing: ['链接不完整', '请在桌面端启用远程控制，然后粘贴生成的完整链接。'],
  denied: ['无法访问', '链接无效或已被刷新，请从桌面端复制新链接。'],
  revoked: ['访问已停止', '桌面端已停止此远程控制链接。'],
  replaced: ['已在另一浏览器接管', '同一远程控制链接只保留一个活动浏览器；刷新本页可重新接管。'],
  incompatible_desktop: ['桌面端版本不兼容', '当前远程控制功能需要更新的桌面端，请先升级桌面端后再重新打开链接。'],
  error: ['连接异常', '远程控制会继续尝试恢复连接。'],
};

const BLOCKING = new Set(['credentials_missing', 'denied', 'revoked', 'replaced', 'incompatible_desktop']);

export function WebConnectionStatus({ theme }) {
  const [connection, setConnection] = useState(() => (
    isWeb && typeof platform.getConnectionState === 'function'
      ? platform.getConnectionState()
      : { status: 'connected', message: '' }
  ));

  useEffect(() => {
    if (!isWeb || typeof platform.onConnectionChange !== 'function') return undefined;
    return platform.onConnectionChange(setConnection);
  }, []);

  if (!isWeb || !connection || connection.status === 'connected') return null;

  const [title, fallback] = COPY[connection.status] || COPY.error;
  const message = connection.message || fallback;
  const dark = theme === 'dark';
  const card = (
    <div
      role="status"
      aria-live="polite"
      className={`max-w-[520px] rounded-2xl border px-5 py-4 shadow-2xl ${dark
        ? 'border-[#3C4043] bg-[#202124] text-[#E8EAED]'
        : 'border-[#DADCE0] bg-white text-[#202124]'}`}
    >
      <div className="flex items-center gap-3">
        {!BLOCKING.has(connection.status) && (
          <span className="h-4 w-4 shrink-0 animate-spin rounded-full border-2 border-[#8AB4F8] border-t-transparent" />
        )}
        <div className="min-w-0">
          <div className="text-[15px] font-semibold">{title}</div>
          <div className={`mt-1 text-[13px] leading-relaxed ${dark ? 'text-[#BDC1C6]' : 'text-[#5F6368]'}`}>
            {message}
          </div>
        </div>
      </div>
    </div>
  );

  if (BLOCKING.has(connection.status)) {
    return (
      <div className="fixed inset-0 z-[200] flex items-center justify-center bg-black/55 p-5 backdrop-blur-sm">
        {card}
      </div>
    );
  }

  return <div className="pointer-events-none fixed inset-x-0 top-3 z-[190] flex justify-center px-3">{card}</div>;
}
