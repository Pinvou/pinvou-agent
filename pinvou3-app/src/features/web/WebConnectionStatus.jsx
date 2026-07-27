import React, { useEffect, useState } from 'react';
import { isWeb, platform } from '../../shared/platform.js';

const BLOCKING = new Set(['credentials_missing', 'denied', 'revoked', 'replaced', 'incompatible_desktop']);

export function WebConnectionStatus({ theme, t }) {
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

  const copy = t.uiWebConnection;
  const [title, fallback] = copy[connection.status] || copy.error;
  // 桌面端的已知状态可能携带旧版固定中文文案；界面统一使用当前语言字典。
  const message = copy[connection.status] ? fallback : (connection.message || fallback);
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
