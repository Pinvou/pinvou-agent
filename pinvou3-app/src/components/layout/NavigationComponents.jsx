import React, { useEffect, useState } from 'react';
import { Archive, Check, Edit2, FolderOpen, MoreHorizontal, PinIcon, PinOffIcon, Sparkles, Trash2, X } from '../icons.jsx';
import { useLongPressDrag } from '../../hooks/useLongPressDrag.js';

const NavItem = ({ icon, label, active, theme, isSidebarOpen = true, onClick, dragKind, dragging, onPickUp }) => {
      const isDark = theme === 'dark';
      const drag = useLongPressDrag(dragKind, onPickUp);
      const dragProps = dragKind ? drag.handlers : {};
      const clickH = dragKind ? drag.guardClick(onClick) : onClick;
      return (
        <div
          onClick={clickH}
          {...dragProps}
          data-nav={dragKind || undefined}
          title={!isSidebarOpen ? label : ""}
          style={dragging ? { opacity: 0.4 } : undefined}
          className={`group flex items-center cursor-pointer text-[15px] font-medium transition-all overflow-hidden select-none
          ${isSidebarOpen ? 'px-4 py-3 rounded-full w-full' : 'w-10 h-10 justify-center rounded-full mx-auto shrink-0'}
          ${active
            ? (isDark ? 'bg-[#A8C7FA] text-[#041E49]' : 'bg-[#D3E3FD] text-[#041E49]')
            : (isDark ? 'text-[#E3E3E3] hover:bg-[#282A2C]' : 'text-[#1F1F1F] hover:bg-[#E1E5EA]')}`}
        >
          <div className={`${isSidebarOpen ? 'mr-4' : ''} shrink-0 ${active ? (isDark ? 'text-[#041E49]' : 'text-[#0B57D0]') : ''}`}>
            {icon}
          </div>
          {isSidebarOpen && <span className="whitespace-nowrap">{label}</span>}
        </div>
      );
    };

    const ArchiveConfirmDialog = ({ theme, t, onCancel, onConfirm }) => {
      const isDark = theme === 'dark';
      useEffect(() => {
        const onKey = (e) => {
          if (e.key === 'Escape') onCancel();
        };
        window.addEventListener('keydown', onKey);
        return () => window.removeEventListener('keydown', onKey);
      }, [onCancel]);
      return (
        <div
          role="presentation"
          className="fixed inset-0 z-[200] flex items-center justify-center p-4"
          style={{
            background: 'rgba(0,0,0,.34)',
            backdropFilter: 'blur(14px) saturate(140%)',
            WebkitBackdropFilter: 'blur(14px) saturate(140%)',
            fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif'
          }}
          onClick={onCancel}
        >
          <div
            role="dialog"
            aria-modal="true"
            aria-labelledby="archive-confirm-title"
            className="w-[320px] max-w-[calc(100vw-48px)] overflow-hidden rounded-[16px] shadow-2xl"
            style={{
              background: isDark ? 'rgba(44,44,46,.96)' : 'rgba(250,250,250,.96)',
              color: isDark ? '#F2F2F7' : '#000',
              boxShadow: isDark ? '0 24px 60px rgba(0,0,0,.55)' : '0 24px 60px rgba(0,0,0,.22)'
            }}
            onClick={e => e.stopPropagation()}
          >
            <div className="px-6 pt-6 pb-5 text-center">
              <div id="archive-confirm-title" className="text-[20px] font-semibold leading-[26px]">{t.archiveConfirmTitle}</div>
              <div className="mt-2.5 text-[15px] leading-[22px]" style={{ color: isDark ? 'rgba(235,235,245,.72)' : 'rgba(60,60,67,.72)' }}>
                <div>{t.archiveConfirmMessage}</div>
                {t.archiveConfirmDetail && <div className="mt-1">{t.archiveConfirmDetail}</div>}
              </div>
            </div>
            <div className="h-px" style={{ background: isDark ? 'rgba(84,84,88,.65)' : 'rgba(60,60,67,.24)' }} />
            <div className="grid grid-cols-2">
              <button
                type="button"
                onClick={onCancel}
                className="h-[50px] text-[17px] active:opacity-70"
                style={{ color: isDark ? '#0A84FF' : '#007AFF' }}
              >
                {t.cpCancel}
              </button>
              <button
                type="button"
                onClick={onConfirm}
                className="h-[50px] text-[17px] font-semibold active:opacity-70"
                style={{
                  color: isDark ? '#0A84FF' : '#007AFF',
                  borderLeft: `1px solid ${isDark ? 'rgba(84,84,88,.65)' : 'rgba(60,60,67,.24)'}`
                }}
              >
                {t.archiveConfirmAction}
              </button>
            </div>
          </div>
        </div>
      );
    };

    const ArchivedDeleteConfirmDialog = ({ theme, t, onCancel, onConfirm }) => {
      const isDark = theme === 'dark';
      useEffect(() => {
        const onKey = (e) => {
          if (e.key === 'Escape') onCancel();
        };
        window.addEventListener('keydown', onKey);
        return () => window.removeEventListener('keydown', onKey);
      }, [onCancel]);
      return (
        <div
          role="presentation"
          className="fixed inset-0 z-[200] flex items-center justify-center p-4"
          style={{
            background: 'rgba(0,0,0,.34)',
            backdropFilter: 'blur(14px) saturate(140%)',
            WebkitBackdropFilter: 'blur(14px) saturate(140%)',
            fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif'
          }}
          onClick={onCancel}
        >
          <div
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="archived-delete-confirm-title"
            className="w-[320px] max-w-[calc(100vw-48px)] overflow-hidden rounded-[16px] shadow-2xl"
            style={{
              background: isDark ? 'rgba(44,44,46,.96)' : 'rgba(250,250,250,.96)',
              color: isDark ? '#F2F2F7' : '#000',
              boxShadow: isDark ? '0 24px 60px rgba(0,0,0,.55)' : '0 24px 60px rgba(0,0,0,.22)'
            }}
            onClick={e => e.stopPropagation()}
          >
            <div className="px-6 pt-6 pb-5 text-center">
              <div id="archived-delete-confirm-title" className="text-[20px] font-semibold leading-[26px]">{t.archivedDeleteTitle}</div>
              <div className="mt-2.5 text-[15px] leading-[22px]" style={{ color: isDark ? 'rgba(235,235,245,.72)' : 'rgba(60,60,67,.72)' }}>
                {t.archivedDeleteMessage}
              </div>
            </div>
            <div className="h-px" style={{ background: isDark ? 'rgba(84,84,88,.65)' : 'rgba(60,60,67,.24)' }} />
            <div className="grid grid-cols-2">
              <button
                type="button"
                onClick={onCancel}
                className="h-[50px] text-[17px] active:opacity-70"
                style={{ color: isDark ? '#0A84FF' : '#007AFF' }}
              >
                {t.cpCancel}
              </button>
              <button
                type="button"
                onClick={onConfirm}
                className="h-[50px] text-[17px] font-semibold active:opacity-70"
                style={{
                  color: '#FF3B30',
                  borderLeft: `1px solid ${isDark ? 'rgba(84,84,88,.65)' : 'rgba(60,60,67,.24)'}`
                }}
              >
                {t.archivedDeleteAction}
              </button>
            </div>
          </div>
        </div>
      );
    };

    const ArchiveToast = ({ theme, t, onClose, onView }) => {
      const isDark = theme === 'dark';
      return (
        <div
          className="fixed left-1/2 top-6 z-[210] -translate-x-1/2 px-2 py-2 rounded-[18px] flex items-center gap-1 shadow-2xl"
          style={{
            background: isDark ? 'rgba(44,44,46,.94)' : 'rgba(250,250,250,.94)',
            color: isDark ? '#F2F2F7' : '#1C1C1E',
            border: `1px solid ${isDark ? 'rgba(255,255,255,.10)' : 'rgba(0,0,0,.08)'}`,
            backdropFilter: 'blur(18px) saturate(150%)',
            WebkitBackdropFilter: 'blur(18px) saturate(150%)',
            fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif'
          }}
        >
          <div className="pl-3 pr-2 text-[14px] leading-5 whitespace-nowrap">{t.archiveSuccess}</div>
          <button
            type="button"
            onClick={onView}
            className="h-8 px-3 rounded-[12px] text-[14px] font-semibold active:opacity-70"
            style={{ color: isDark ? '#0A84FF' : '#007AFF', background: isDark ? 'rgba(10,132,255,.12)' : 'rgba(0,122,255,.10)' }}
          >
            {t.archiveSuccessView}
          </button>
          <button
            type="button"
            onClick={onClose}
            className="w-8 h-8 rounded-full text-[18px] leading-none active:opacity-70"
            style={{ color: isDark ? 'rgba(235,235,245,.62)' : 'rgba(60,60,67,.62)' }}
            aria-label={t.cpCancel}
          >
            ×
          </button>
        </div>
      );
    };

    // 近期会话项：支持重命名(内联编辑) + 删除(内联二次确认)
    const RecentItem = ({ chat, active, personaTarget, theme, t, onSelect, onRename, onDelete, onTogglePinned, onOpenFolder, onArchive, dragging, onPickUp }) => {
      const isDark = theme === 'dark';
      const [editing, setEditing] = useState(false);
      const [confirming, setConfirming] = useState(false);
      const [menuOpen, setMenuOpen] = useState(false);
      const [menuUp, setMenuUp] = useState(false);
      const [val, setVal] = useState(chat.title);
      const drag = useLongPressDrag('session', onPickUp);
      function save() { const tx = val.trim(); setEditing(false); if (tx && tx !== chat.title) onRename(chat.id, tx); }
      const toggleMenu = (e) => {
        e.stopPropagation();
        const rect = e.currentTarget.getBoundingClientRect();
        setMenuUp(window.innerHeight - rect.bottom < 80);
        setMenuOpen(v => !v);
      };
      const menuItemCls = `w-full h-9 px-3 flex items-center gap-2 text-left text-[14px] whitespace-nowrap transition-colors ${isDark ? 'text-[#E3E3E3] hover:bg-[#303134]' : 'text-[#1F1F1F] hover:bg-[#F1F3F4]'}`;
      if (editing) {
        return (
          <div className="px-1.5 py-0.5">
            <input autoFocus value={val}
              onChange={e => setVal(e.target.value)}
              onClick={e => e.stopPropagation()}
              onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); save(); } else if (e.key === 'Escape') { setEditing(false); setVal(chat.title); } }}
              onBlur={save}
              className={`w-full px-3 py-1 rounded-full text-[15px] outline-none ${isDark ? 'bg-[#131314] text-[#E3E3E3] ring-1 ring-[#A8C7FA]' : 'bg-white text-[#1F1F1F] ring-1 ring-[#0B57D0]'}`} />
          </div>
        );
      }
      return (
        <div onClick={drag.guardClick(() => onSelect(chat.id))}
          {...drag.handlers}
          title={personaTarget ? t.cpTargetMarkTitle : undefined}
          style={ dragging ? { opacity: 0.4 } : (personaTarget ? { background: isDark?'rgba(10,132,255,.20)':'rgba(0,122,255,.12)', boxShadow:'inset 0 0 0 1px '+(isDark?'rgba(10,132,255,.6)':'rgba(0,122,255,.45)'), color: isDark?'#fff':'#1F1F1F' } : undefined) }
          className={`group flex items-center px-4 py-1.5 rounded-full cursor-pointer text-[15px] transition-all
            ${personaTarget ? ''
              : active ? (isDark ? 'bg-[#333537] text-white' : 'bg-[#E1E5EA] text-[#1F1F1F]')
                     : (isDark ? 'text-[#E3E3E3] hover:bg-[#282A2C]' : 'text-[#1F1F1F] hover:bg-[#E1E5EA]')}`}>
          {personaTarget && <Sparkles size={13} className="shrink-0 mr-1.5" style={{ color: isDark?'#0A84FF':'#007AFF' }} />}
          <span className="truncate pr-2 leading-relaxed whitespace-nowrap flex-1">{chat.title}</span>
          {chat.working && <span className="shrink-0 mr-1 inline-block w-2 h-2 rounded-full bg-current opacity-70 animate-pulse" title={t.riGenerating}></span>}
          {chat.skill && <span className="text-[11px] shrink-0 opacity-70 mr-1" title={chat.skill}>🧭</span>}
          {confirming ? (
            <div className="flex items-center gap-0.5 shrink-0" onClick={e => e.stopPropagation()}>
              <span className={`text-[11px] mr-0.5 ${isDark ? 'text-[#F28B82]' : 'text-[#C5221F]'}`}>{t.riDelQ}</span>
              <button title={t.riDelConfirm} onClick={(e) => { e.stopPropagation(); onDelete(chat.id); }}
                className={`w-6 h-6 rounded-full flex items-center justify-center ${isDark ? 'text-[#F28B82] hover:bg-[#5c2b29]' : 'text-[#C5221F] hover:bg-[#FAD2CF]'}`}><Check size={14} /></button>
              <button title={t.cpCancel} onClick={(e) => { e.stopPropagation(); setConfirming(false); }}
                className={`w-6 h-6 rounded-full flex items-center justify-center ${isDark ? 'text-[#C4C7C5] hover:bg-[#444746]' : 'text-[#5F6368] hover:bg-[#D3D7DB]'}`}><X size={13} /></button>
            </div>
          ) : (
            <>
              {/* 默认: 显示日期(辨识每条会话什么时候发生);hover/active 时换成编辑/删除按钮 */}
              {chat.date && (
                <span className={`text-[11px] shrink-0 opacity-60 whitespace-nowrap group-hover:hidden ${isDark ? 'text-[#9AA0A6]' : 'text-[#5F6368]'}`}>
                  {chat.date}
                </span>
              )}
              <div className="hidden group-hover:flex items-center gap-0.5 shrink-0">
                <button title={chat.pinned ? t.riUnpin : t.riPin} onClick={(e) => { e.stopPropagation(); onTogglePinned && onTogglePinned(chat.id, !chat.pinned); }}
                  className={`w-6 h-6 rounded-full flex items-center justify-center transition-colors ${isDark ? 'text-[#C4C7C5] hover:bg-[#444746]' : 'text-[#5F6368] hover:bg-[#D3D7DB]'}`}>
                  {chat.pinned ? <PinOffIcon size={13} /> : <PinIcon size={13} />}
                </button>
                <button title={t.riRename} onClick={(e) => { e.stopPropagation(); setVal(chat.title); setEditing(true); }}
                  className={`w-6 h-6 rounded-full flex items-center justify-center ${isDark ? 'text-[#C4C7C5] hover:bg-[#444746]' : 'text-[#5F6368] hover:bg-[#D3D7DB]'}`}><Edit2 size={13} /></button>
                <button title={t.archiveSession} onClick={(e) => { e.stopPropagation(); onArchive && onArchive(chat.id); }}
                  className={`w-6 h-6 rounded-full flex items-center justify-center ${isDark ? 'text-[#C4C7C5] hover:bg-[#444746]' : 'text-[#5F6368] hover:bg-[#D3D7DB]'}`}><Archive size={13} /></button>
                <button title={t.cpDelete} onClick={(e) => { e.stopPropagation(); setConfirming(true); }}
                  className={`w-6 h-6 rounded-full flex items-center justify-center ${isDark ? 'text-[#C4C7C5] hover:text-[#F28B82] hover:bg-[#5c2b29]' : 'text-[#5F6368] hover:text-[#C5221F] hover:bg-[#FAD2CF]'}`}><Trash2 size={13} /></button>
                <div className="relative">
                  <button title={t.riMore} onClick={toggleMenu}
                    className={`w-6 h-6 rounded-full flex items-center justify-center ${isDark ? 'text-[#C4C7C5] hover:bg-[#444746]' : 'text-[#5F6368] hover:bg-[#D3D7DB]'}`}><MoreHorizontal size={14} /></button>
                  {menuOpen && (
                    <div onClick={e => e.stopPropagation()}
                      className={`absolute right-0 ${menuUp ? 'bottom-7' : 'top-7'} z-50 w-40 overflow-hidden rounded-xl py-1 shadow-xl ring-1 ${isDark ? 'bg-[#202124] ring-white/10' : 'bg-white ring-black/10'}`}>
                      <button className={menuItemCls} onClick={() => { setMenuOpen(false); onOpenFolder && onOpenFolder(chat.id); }}>
                        <FolderOpen size={15} />
                        <span>{t.riOpenFolder}</span>
                      </button>
                    </div>
                  )}
                </div>
              </div>
            </>
          )}
        </div>
      );
    };

export { NavItem, ArchiveConfirmDialog, ArchivedDeleteConfirmDialog, ArchiveToast, RecentItem };
