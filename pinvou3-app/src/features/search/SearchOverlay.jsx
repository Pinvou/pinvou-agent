import React, { useEffect, useRef, useState } from 'react';
import { Search, X } from '../../components/icons.jsx';

export const SearchOverlay = ({ theme, history, t, onSelect, onClose }) => {
  const isDark = theme === 'dark';
  const [query, setQuery] = useState('');
  const inputRef = useRef(null);
  const filtered = query
    ? history.filter(h => String(h.title || '').toLowerCase().includes(query.toLowerCase()))
    : history;

  useEffect(() => {
    const timer = window.setTimeout(() => inputRef.current && inputRef.current.focus(), 80);
    const onKey = (e) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener('keydown', onKey);
    };
  }, [onClose]);

  return (
    <div
      role="presentation"
      className="fixed inset-0 z-[180] flex items-start justify-center px-5 pt-[76px]"
      style={{
        background: isDark ? 'rgba(0,0,0,.34)' : 'rgba(255,255,255,.28)',
        backdropFilter: 'blur(18px) saturate(150%)',
        WebkitBackdropFilter: 'blur(18px) saturate(150%)',
        fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif',
      }}
      onClick={onClose}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={t.searchChats}
        className="w-full max-w-[680px] overflow-hidden rounded-[28px] border shadow-2xl"
        style={{
          background: isDark ? 'rgba(32,33,36,.86)' : 'rgba(255,255,255,.88)',
          borderColor: isDark ? 'rgba(255,255,255,.10)' : 'rgba(0,0,0,.08)',
          boxShadow: isDark ? '0 30px 90px rgba(0,0,0,.58)' : '0 30px 90px rgba(25,33,45,.20)',
          color: isDark ? '#F2F2F7' : '#1F1F1F',
        }}
        onClick={e => e.stopPropagation()}
      >
        <div className="p-3">
          <div
            className="flex h-12 items-center gap-3 rounded-full px-4"
            style={{
              background: isDark ? 'rgba(255,255,255,.08)' : 'rgba(118,118,128,.12)',
            }}
          >
            <Search size={20} className="text-[#6E6E73] dark:text-[#C7C7CC]" />
            <input
              ref={inputRef}
              type="text"
              value={query}
              onChange={e => setQuery(e.target.value)}
              placeholder={t.searchPlaceholder}
              className="min-w-0 flex-1 bg-transparent border-none outline-none text-[17px] leading-6 text-[#1D1D1F] placeholder:text-[#8A8A8E] dark:text-[#F2F2F7] dark:placeholder:text-[#8E8E93]"
            />
            {query && (
              <button
                type="button"
                onClick={() => { setQuery(''); inputRef.current && inputRef.current.focus(); }}
                title={t.clearSearch}
                aria-label={t.clearSearch}
                className="w-7 h-7 shrink-0 rounded-full flex items-center justify-center transition-colors"
                style={{
                  background: isDark ? 'rgba(255,255,255,.10)' : 'rgba(60,60,67,.18)',
                  color: isDark ? '#C7C7CC' : '#6E6E73',
                }}
              >
                <X size={15} />
              </button>
            )}
            <button
              type="button"
              onClick={onClose}
              title={t.winClose}
              aria-label={t.winClose}
              className="w-7 h-7 shrink-0 rounded-full flex items-center justify-center transition-colors"
              style={{
                background: isDark ? 'rgba(255,255,255,.10)' : 'rgba(60,60,67,.18)',
                color: isDark ? '#C7C7CC' : '#6E6E73',
              }}
            >
              <X size={15} />
            </button>
          </div>
        </div>

        <div className="max-h-[min(620px,calc(100vh-180px))] overflow-y-auto custom-scrollbar px-2 pb-2">
          <div className="px-4 pb-2 pt-1 text-[13px] font-semibold text-[#8A8A8E] dark:text-[#8E8E93]">
            {t.recent}
          </div>
          {filtered.length > 0 ? filtered.map(chat => (
            <button
              key={chat.id}
              type="button"
              onClick={() => onSelect && onSelect(chat.id)}
              className="w-full min-w-0 rounded-[18px] px-4 py-3 text-left transition-colors hover:bg-black/[.05] dark:hover:bg-white/[.08]"
              style={{ color: isDark ? '#F2F2F7' : '#1D1D1F' }}
            >
              <div className="flex min-w-0 items-center justify-between gap-4">
                <span className="min-w-0 truncate text-[16px] leading-6">{chat.title}</span>
                <span className="shrink-0 text-[13px] text-[#8A8A8E] dark:text-[#8E8E93]">{chat.date}</span>
              </div>
            </button>
          )) : (
            <div className="px-4 py-8 text-center text-[14px] text-[#8A8A8E] dark:text-[#8E8E93]">
              {t.sidebarTaskEmpty}
            </div>
          )}
        </div>
      </div>
    </div>
  );
};
