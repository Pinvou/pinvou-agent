import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { Archive, Check, Edit2, FolderOpen, MoreHorizontal, PinIcon, PinOffIcon, Sparkles, Trash2, X } from '../icons.jsx';
import { useLongPressDrag } from '../../hooks/useLongPressDrag.js';
import { isImeComposing } from '../../shared/ime-guard.mjs';

const NavItem = ({ icon, label, active, unread = false, isSidebarOpen = true, onClick, dragKind, dragging, onPickUp, nativeButton = false, t, onPointerEnter, onFocus }) => {
      const drag = useLongPressDrag(dragKind, onPickUp);
      const dragProps = dragKind ? drag.handlers : {};
      const clickH = dragKind ? drag.guardClick(onClick) : onClick;
      const Root = nativeButton ? 'button' : 'div';
      return (
        <Root
          {...(nativeButton ? { type: 'button', 'aria-label': label } : {})}
          onClick={clickH}
          onPointerEnter={onPointerEnter}
          onFocus={onFocus}
          {...dragProps}
          data-nav={dragKind || undefined}
          title={isSidebarOpen ? "" : label}
          style={dragging ? { opacity: 0.4 } : undefined}
          className={`group border-0 text-left flex items-center cursor-pointer text-[15px] font-medium transition-all overflow-hidden select-none
          ${isSidebarOpen ? 'px-4 py-2 max-sm:px-3 max-sm:py-2 rounded-full w-full' : 'w-10 h-10 justify-center rounded-full mx-auto shrink-0'}
          ${active
            ? 'bg-[#D3E3FD] text-[#041E49] dark:bg-[#A8C7FA]'
            : 'text-[#1F1F1F] hover:bg-[#E1E5EA] dark:text-[#E3E3E3] dark:hover:bg-[#282A2C]'}`}
        >
          <div className={`relative ${isSidebarOpen ? 'mr-3' : ''} shrink-0 ${active ? 'text-[#0B57D0] dark:text-[#041E49]' : ''}`}>
            {icon}
            {unread && (
              <span role="img" data-testid="scheduled-nav-unread" aria-label={t.uiScheduled.navUnreadAria}
                className={"absolute -right-1.5 -top-1 w-2.5 h-2.5 rounded-full border-2 bg-[#0B57D0] " + (active ? 'border-[#D3E3FD] dark:border-[#A8C7FA]' : 'border-[#F0F4F9] dark:border-[#1E1F20]')} />
            )}
          </div>
          {isSidebarOpen && <span className="whitespace-nowrap">{label}</span>}
        </Root>
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
        // 遮罩点击关闭;键盘路径:Escape(下方 effect 监听)与弹窗内真实「取消」按钮。
        // biome-ignore lint/a11y/noStaticElementInteractions: 遮罩点击关闭层,键盘路径由 Escape 监听与取消按钮承担
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
          {/* biome-ignore lint/a11y/useKeyWithClickEvents: 弹窗体仅阻止冒泡以免误触遮罩关闭,自身非交互控件 */}
          <div
            role="dialog"
            aria-modal="true"
            aria-labelledby="archive-confirm-title"
            className="w-[320px] max-w-[calc(100vw-48px)] overflow-hidden rounded-[16px] shadow-2xl bg-[rgba(250,250,250,.96)] dark:bg-[rgba(44,44,46,.96)] text-[#000] dark:text-[#F2F2F7]"
            style={{
              // isDark dynamic-value: 保留 (boxShadow)
              boxShadow: isDark ? '0 24px 60px rgba(0,0,0,.55)' : '0 24px 60px rgba(0,0,0,.22)'
            }}
            onClick={e => e.stopPropagation()}
          >
            <div className="px-6 pt-6 pb-5 text-center">
              <div id="archive-confirm-title" className="text-[20px] font-semibold leading-[26px]">{t.archiveConfirmTitle}</div>
              <div className="mt-2.5 text-[15px] leading-[22px] text-[rgba(60,60,67,.72)] dark:text-[rgba(235,235,245,.72)]">
                <div>{t.archiveConfirmMessage}</div>
                {t.archiveConfirmDetail && <div className="mt-1">{t.archiveConfirmDetail}</div>}
              </div>
            </div>
            <div className="h-px bg-[rgba(60,60,67,.24)] dark:bg-[rgba(84,84,88,.65)]" />
            <div className="grid grid-cols-2">
              <button
                type="button"
                onClick={onCancel}
                className="h-[50px] text-[17px] active:opacity-70 text-[#007AFF] dark:text-[#0A84FF]"
              >
                {t.cpCancel}
              </button>
              <button
                type="button"
                onClick={onConfirm}
                className="h-[50px] text-[17px] font-semibold active:opacity-70 text-[#007AFF] dark:text-[#0A84FF] border-l border-l-[rgba(60,60,67,.24)] dark:border-l-[rgba(84,84,88,.65)]"
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
        // 遮罩点击关闭;键盘路径:Escape(下方 effect 监听)与弹窗内真实「取消」按钮。
        // biome-ignore lint/a11y/noStaticElementInteractions: 遮罩点击关闭层,键盘路径由 Escape 监听与取消按钮承担
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
          {/* biome-ignore lint/a11y/useKeyWithClickEvents: 弹窗体仅阻止冒泡以免误触遮罩关闭,自身非交互控件 */}
          <div
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="archived-delete-confirm-title"
            className="w-[320px] max-w-[calc(100vw-48px)] overflow-hidden rounded-[16px] shadow-2xl bg-[rgba(250,250,250,.96)] dark:bg-[rgba(44,44,46,.96)] text-[#000] dark:text-[#F2F2F7]"
            style={{
              // isDark dynamic-value: 保留 (boxShadow)
              boxShadow: isDark ? '0 24px 60px rgba(0,0,0,.55)' : '0 24px 60px rgba(0,0,0,.22)'
            }}
            onClick={e => e.stopPropagation()}
          >
            <div className="px-6 pt-6 pb-5 text-center">
              <div id="archived-delete-confirm-title" className="text-[20px] font-semibold leading-[26px]">{t.archivedDeleteTitle}</div>
              <div className="mt-2.5 text-[15px] leading-[22px] text-[rgba(60,60,67,.72)] dark:text-[rgba(235,235,245,.72)]">
                {t.archivedDeleteMessage}
              </div>
            </div>
            <div className="h-px bg-[rgba(60,60,67,.24)] dark:bg-[rgba(84,84,88,.65)]" />
            <div className="grid grid-cols-2">
              <button
                type="button"
                onClick={onCancel}
                className="h-[50px] text-[17px] active:opacity-70 text-[#007AFF] dark:text-[#0A84FF]"
              >
                {t.cpCancel}
              </button>
              <button
                type="button"
                onClick={onConfirm}
                className="h-[50px] text-[17px] font-semibold active:opacity-70 text-[#FF3B30] border-l border-l-[rgba(60,60,67,.24)] dark:border-l-[rgba(84,84,88,.65)]"
              >
                {t.archivedDeleteAction}
              </button>
            </div>
          </div>
        </div>
      );
    };

    const ArchiveToast = ({ t, onClose, onView }) => {
      return (
        <div
          className="fixed left-1/2 top-6 z-[210] -translate-x-1/2 px-2 py-2 rounded-[18px] flex items-center gap-1 shadow-2xl bg-[rgba(250,250,250,.94)] dark:bg-[rgba(44,44,46,.94)] text-[#1C1C1E] dark:text-[#F2F2F7] border border-[rgba(0,0,0,.08)] dark:border-[rgba(255,255,255,.10)]"
          style={{
            backdropFilter: 'blur(18px) saturate(150%)',
            WebkitBackdropFilter: 'blur(18px) saturate(150%)',
            fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif'
          }}
        >
          <div className="pl-3 pr-2 text-[14px] leading-5 whitespace-nowrap">{t.archiveSuccess}</div>
          <button
            type="button"
            onClick={onView}
            className="h-8 min-w-[76px] px-3 rounded-[12px] text-[14px] font-semibold whitespace-nowrap active:opacity-70 text-[#007AFF] dark:text-[#0A84FF] bg-[rgba(0,122,255,.10)] dark:bg-[rgba(10,132,255,.12)]"
          >
            {t.archiveSuccessView}
          </button>
          <button
            type="button"
            onClick={onClose}
            className="w-8 h-8 rounded-full text-[18px] leading-none active:opacity-70 text-[rgba(60,60,67,.62)] dark:text-[rgba(235,235,245,.62)]"
            aria-label={t.cpCancel}
          >
            ×
          </button>
        </div>
      );
    };

    // 近期会话项：支持重命名(内联编辑) + 删除(内联二次确认)
    // 行内样式拆成纯函数:拖拽降透明度;persona 目标行运行时拼高亮色(与 isDark 相关,无法走静态 dark: 变体)。
    const recentItemRowStyle = (dragging, personaTarget, isDark) => {
      if (dragging) return { opacity: 0.4 };
      if (!personaTarget) return null;
      return {
        background: isDark ? 'rgba(10,132,255,.20)' : 'rgba(0,122,255,.12)',
        boxShadow: 'inset 0 0 0 1px ' + (isDark ? 'rgba(10,132,255,.6)' : 'rgba(0,122,255,.45)'),
        color: isDark ? '#fff' : '#1F1F1F',
      };
    };
    const RecentItem = ({ chat, active, personaTarget, theme, t, onSelect, onRename, onDelete, onTogglePinned, onOpenFolder, onArchive, dragKind = 'session', dragging, onPickUp }) => {
      const isDark = theme === 'dark';
      const [editing, setEditing] = useState(false);
      const [confirming, setConfirming] = useState(false);
      const [menuOpen, setMenuOpen] = useState(false);
      const [menuStyle, setMenuStyle] = useState(null);
      const [val, setVal] = useState(chat.title);
      const sessionDragKind = onPickUp ? dragKind : null;
      const drag = useLongPressDrag(sessionDragKind, onPickUp);
      const dragProps = sessionDragKind ? drag.handlers : {};
      const selectChat = () => onSelect(chat.id);
      function save() { const tx = val.trim(); setEditing(false); if (tx && tx !== chat.title) onRename(chat.id, tx); }
      const closeMenu = () => setMenuOpen(false);
      const placeMenu = (target) => {
        const rect = target.getBoundingClientRect();
        const width = 176;
        const height = 184;
        const left = Math.max(8, Math.min(rect.right - width, window.innerWidth - width - 8));
        const top = rect.bottom + 6 + height > window.innerHeight
          ? Math.max(8, rect.top - height - 6)
          : Math.max(8, rect.bottom + 6);
        setMenuStyle({ left, top, width });
      };
      const toggleMenu = (e) => {
        e.stopPropagation();
        placeMenu(e.currentTarget);
        setMenuOpen(v => !v);
      };
      const openContextMenu = (e) => {
        e.preventDefault();
        e.stopPropagation();
        placeMenu(e.currentTarget);
        setMenuOpen(true);
      };
      useEffect(() => {
        if (!menuOpen) return;
        const close = () => setMenuOpen(false);
        const closeOnEscape = (event) => {
          if (event.key === 'Escape') {
            event.preventDefault();
            close();
          }
        };
        document.addEventListener('pointerdown', close);
        window.addEventListener('keydown', closeOnEscape);
        window.addEventListener('resize', close);
        window.addEventListener('scroll', close, true);
        return () => {
          document.removeEventListener('pointerdown', close);
          window.removeEventListener('keydown', closeOnEscape);
          window.removeEventListener('resize', close);
          window.removeEventListener('scroll', close, true);
        };
      }, [menuOpen]);
      const menuItemCls = `w-full h-9 px-3 flex items-center gap-2 text-left text-[14px] whitespace-nowrap transition-colors text-[#1F1F1F] hover:bg-[#F1F3F4] dark:text-[#E3E3E3] dark:hover:bg-[#303134]`;
      const menu = menuOpen && menuStyle && typeof document !== 'undefined' ? createPortal(
        <div onPointerDown={e => e.stopPropagation()}
          data-testid={chat.menuTestId}
          className={`fixed z-[1000] overflow-hidden rounded-xl py-1 shadow-xl ring-1 bg-white ring-black/10 dark:bg-[#202124] dark:ring-white/10`}
          style={menuStyle}>
          <button type="button" className={menuItemCls} onClick={() => { closeMenu(); onTogglePinned && onTogglePinned(chat.id, !chat.pinned); }}>
            {chat.pinned ? <PinOffIcon size={15} /> : <PinIcon size={15} />}
            <span>{chat.pinned ? t.riUnpin : t.riPin}</span>
          </button>
          <button type="button" className={menuItemCls} onClick={() => { closeMenu(); setVal(chat.title); setEditing(true); }}>
            <Edit2 size={15} />
            <span>{t.riRename}</span>
          </button>
          <button type="button" className={`${menuItemCls} text-[#C5221F] hover:bg-[#FAD2CF] dark:text-[#F28B82] dark:hover:bg-[#5c2b29]`} onClick={() => { closeMenu(); setConfirming(true); }}>
            <Trash2 size={15} />
            <span>{t.cpDelete}</span>
          </button>
          {(onOpenFolder || onArchive) && (
            <div className="my-1 h-px bg-black/10 dark:bg-white/10" />
          )}
          {onOpenFolder && (
            <button type="button" className={menuItemCls} onClick={() => { closeMenu(); onOpenFolder(chat.id); }}>
              <FolderOpen size={15} />
              <span>{t.riOpenFolder}</span>
            </button>
          )}
          {onArchive && (
            <button type="button" className={menuItemCls} onClick={() => { closeMenu(); onArchive(chat.id); }}>
              <Archive size={15} />
              <span>{t.archiveSession}</span>
            </button>
          )}
        </div>,
        document.body
      ) : null;
      if (editing) {
        return (
          <div className="flex h-11 items-center px-1.5">
            {/* biome-ignore lint/a11y/noAutofocus: 点击「重命名」即进入内联编辑,焦点必须立即落在输入框(载荷行为) */}
            <input autoFocus value={val}
              onChange={e => setVal(e.target.value)}
              onClick={e => e.stopPropagation()}
              onKeyDown={e => { if (e.key === 'Enter' && !isImeComposing(e)) { e.preventDefault(); save(); } else if (e.key === 'Escape') { setEditing(false); setVal(chat.title); } }}
              onBlur={save}
              className="w-full px-3 py-1 rounded-full text-[15px] outline-none bg-white text-[#1F1F1F] ring-1 ring-[#0B57D0] dark:bg-[#131314] dark:text-[#E3E3E3] dark:ring-[#A8C7FA]" />
          </div>
        );
      }
      // 键盘路径:会话行本身可用 Enter/Space 选中(此前仅可点击,补齐真实键盘可达性)。
      // 仅在焦点落在行自身时生效,避免内部按钮(置顶/更多)的 Enter 冒泡误触选中;
      // 拖拽为纯指针交互(useLongPressDrag),键盘路径无需 guardClick。
      const selectChatOnKey = (e) => {
        if (e.target !== e.currentTarget) return;
        if ((e.key === 'Enter' || e.key === ' ') && !isImeComposing(e)) {
          e.preventDefault();
          selectChat();
        }
      };
      return (
        // 行内嵌套置顶/更多等真实按钮,转为 <button type="button"> 会构成非法嵌套交互元素,故用 role="button" 容器模式。
        // biome-ignore lint/a11y/useSemanticElements: 行内嵌套真实按钮,无法转为 <button type="button">(非法嵌套),用容器 role 承载
        <div role="button" tabIndex={0} onKeyDown={selectChatOnKey}
          onClick={sessionDragKind ? drag.guardClick(selectChat) : selectChat}
          {...dragProps}
          onContextMenu={openContextMenu}
          data-testid={chat.testId}
          data-drag-kind={sessionDragKind || undefined}
          title={personaTarget ? t.cpTargetMarkTitle : undefined}
          style={recentItemRowStyle(dragging, personaTarget, isDark)}
          className={`group flex h-11 items-center px-4 rounded-full cursor-pointer text-[15px] transition-all
            ${personaTarget ? ''
              : active ? 'bg-[#E1E5EA] text-[#1F1F1F] dark:bg-[#333537] dark:text-white'
                     : 'text-[#1F1F1F] hover:bg-[#E1E5EA] dark:text-[#E3E3E3] dark:hover:bg-[#282A2C]'}`}>{/* isDark dynamic-value: 保留 (personaTarget boxShadow 运行时拼色,与 background/color 同对象) */}
          {personaTarget && <Sparkles size={13} className="shrink-0 mr-1.5 text-[#007AFF] dark:text-[#0A84FF]" />}
          {chat.leadingIcon && (
            <span className="mr-3 flex h-5 w-5 shrink-0 items-center justify-center opacity-95">
              {chat.leadingIcon}
            </span>
          )}
          {/* 置顶标:常驻显示在标题前,倾斜小灰标,与「置顶优先」排序呼应 */}
          {chat.pinned && <PinIcon size={12} className="shrink-0 mr-1.5 rotate-45 text-[#8A8F94] dark:text-[#9AA0A6]" />}
          <span className="min-w-0 flex-1 pr-2">
            <span className="block truncate whitespace-nowrap leading-5">{chat.titleContent || chat.title}</span>
            {chat.subtitle && (
              <span className="block truncate text-[12px] leading-4 text-[#8A8F94] dark:text-[#9AA0A6]">{chat.subtitle}</span>
            )}
          </span>
          {/* 等待选择时模型不在生成：橙点替代灰点，避免两个徽标叠加 */}
          {chat.working && !chat.waitingInput && <span className="shrink-0 mr-1 inline-block w-2 h-2 rounded-full bg-current opacity-70 animate-pulse" title={t.riGenerating}></span>}
          {chat.waitingInput && <span className="shrink-0 mr-1 inline-block w-2 h-2 rounded-full bg-[#F9AB00] opacity-90 animate-pulse" title={t.riAwaitingInput}></span>}
          {chat.skill && <span className="text-[11px] shrink-0 opacity-70 mr-1" title={chat.skill}>🧭</span>}
          {chat.unread && (
            <span role="img" data-testid="scheduled-run-sidebar-unread" aria-label={t.uiScheduled.unread}
              className="mr-1 h-2 w-2 shrink-0 rounded-full group-hover:hidden"
              style={{ background: '#0B57D0' }} />
          )}
          {confirming ? (
            // biome-ignore lint/a11y/useKeyWithClickEvents: 容器仅阻止冒泡以免误触会话行选中,自身非交互控件
            // biome-ignore lint/a11y/noStaticElementInteractions: 容器仅阻止冒泡,非交互容器
            <div className="flex items-center gap-0.5 shrink-0" onClick={e => e.stopPropagation()}>
              <span className="text-[11px] mr-0.5 text-[#C5221F] dark:text-[#F28B82]">{t.riDelQ}</span>
              <button type="button" title={t.riDelConfirm} onClick={(e) => { e.stopPropagation(); onDelete(chat.id); }}
                className="w-6 h-6 rounded-full flex items-center justify-center text-[#C5221F] hover:bg-[#FAD2CF] dark:text-[#F28B82] dark:hover:bg-[#5c2b29]"><Check size={14} /></button>
              <button type="button" title={t.cpCancel} onClick={(e) => { e.stopPropagation(); setConfirming(false); }}
                className="w-6 h-6 rounded-full flex items-center justify-center text-[#5F6368] hover:bg-[#D3D7DB] dark:text-[#C4C7C5] dark:hover:bg-[#444746]"><X size={13} /></button>
            </div>
          ) : (
            <>
              {/* 默认: 显示日期(辨识每条会话什么时候发生);hover/active 时换成置顶/更多按钮,重命名/收纳/删除在更多菜单里。
                  窄屏无 hover：按钮组常显、日期让位，保证触屏可达。 */}
              {chat.date && (
                <span className="text-[11px] shrink-0 opacity-60 whitespace-nowrap group-hover:hidden max-sm:hidden text-[#5F6368] dark:text-[#9AA0A6]">
                  {chat.date}
                </span>
              )}
              <div className="hidden group-hover:flex max-sm:flex items-center gap-0.5 shrink-0">
                <button type="button" title={chat.pinned ? t.riUnpin : t.riPin} onClick={(e) => { e.stopPropagation(); onTogglePinned && onTogglePinned(chat.id, !chat.pinned); }}
                  className="w-6 h-6 rounded-full flex items-center justify-center transition-colors text-[#5F6368] hover:bg-[#D3D7DB] dark:text-[#C4C7C5] dark:hover:bg-[#444746]">
                  {chat.pinned ? <PinOffIcon size={13} /> : <PinIcon size={13} />}
                </button>

                <div className="relative">
                  <button type="button" title={t.riMore} onClick={toggleMenu}
                    className="w-6 h-6 rounded-full flex items-center justify-center text-[#5F6368] hover:bg-[#D3D7DB] dark:text-[#C4C7C5] dark:hover:bg-[#444746]"><MoreHorizontal size={14} /></button>
                </div>
              </div>
            </>
          )}
          {menu}
        </div>
      );
    };

export { NavItem, ArchiveConfirmDialog, ArchivedDeleteConfirmDialog, ArchiveToast, RecentItem };
