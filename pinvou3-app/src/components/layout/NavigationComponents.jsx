import { memo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Archive, Check, Download, Edit2, FolderOpen, Layers, MoreHorizontal, PinIcon, PinOffIcon, Sparkles, Trash2, X } from '../icons.jsx';
import { useLongPressDrag } from '../../hooks/useLongPressDrag.js';
import { usePortalMenu } from '../../hooks/usePortalMenu.js';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import { PROJECT_SESSION_DRAG_TYPE } from '../../features/projects/projectGrouping.js';

    // NavItem memoization: the main nav re-renders on every App bridge
    // notify (including background streaming tokens); when the props are
    // reference stable (see NAV_ICON_*/NAV_PREFETCH/navNavigateHandlers in
    // main.jsx) the whole nav item can skip the re-render.
    const NavItem = memo(function NavItem({ icon, label, active, unread = false, isSidebarOpen = true, onClick, dragKind, dragging, onPickUp, t, onPointerEnter, onFocus }) {
      const drag = useLongPressDrag(dragKind, onPickUp);
      const dragProps = dragKind ? drag.handlers : {};
      const clickH = dragKind ? drag.guardClick(onClick) : onClick;
      return (
        // biome-ignore lint/a11y/noStaticElementInteractions: main sidebar nav item; activation is pointer-only by design and the collapsed rail shares this markup (behavior unchanged from main)
        // biome-ignore lint/a11y/useKeyWithClickEvents: same nav item; no keyboard activation is wired on any host
        <div
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
        </div>
      );
    });

    // 近期会话项：支持重命名(内联编辑) + 删除(内联二次确认)
    // Inline styles are extracted into pure functions: dragging lowers opacity; the persona target row builds its highlight color at runtime (isDark-dependent, cannot use static dark: variants).
    const recentItemRowStyle = (dragging, personaTarget, isDark) => {
      if (dragging) return { opacity: 0.4 };
      if (!personaTarget) return null;
      return {
        background: isDark ? 'rgba(10,132,255,.20)' : 'rgba(0,122,255,.12)',
        boxShadow: 'inset 0 0 0 1px ' + (isDark ? 'rgba(10,132,255,.6)' : 'rgba(0,122,255,.45)'),
        color: isDark ? '#fff' : '#1F1F1F',
      };
    };
    // RecentItem memoization: the O(sessions) sidebar list is the dominant
    // token-rate re-render cost. Props must be reference stable — chat is
    // derived by the parent's useMemo and callbacks come from the parent's
    // useCallback / per-item closure cache (see renderSidebarTaskItem in
    // main.jsx); the default shallow compare then skips correctly.
    const RecentItem = memo(function RecentItem({ chat, active, personaTarget, theme, t, onSelect, onRename, onDelete, onTogglePinned, onOpenFolder, onExportArchive, onArchive, onMoveToProject, dragKind = 'session', dragging, onPickUp, dndPayload, dndDisabled, onDragEnd }) {
      const isDark = theme === 'dark';
      const [editing, setEditing] = useState(false);
      const [confirming, setConfirming] = useState(false);
      const [val, setVal] = useState(chat.title);
      const sessionDragKind = onPickUp ? dragKind : null;
      const drag = useLongPressDrag(sessionDragKind, onPickUp);
      const dragProps = sessionDragKind ? drag.handlers : {};
      const selectChat = () => onSelect(chat.id);
      function save() { const tx = val.trim(); setEditing(false); if (tx && tx !== chat.title) onRename(chat.id, tx); }
      // Portal "more" menu placement/close lives in the shared hook (same
      // plumbing as the project-group header menu). Height covers the tallest
      // variant actually rendered — 6 menu items at h-9 (36px) + 9px divider +
      // 8px vertical padding ≈ 233: codex rows render move-to-project, other
      // rows render export-archive, and the two are taskKind-exclusive. It
      // only drives the bottom-edge flip decision and the portal clips
      // (per-menu height convention, see ProjectGroupHeader).
      const { menuOpen, menuStyle, closeMenu, toggleMenu, openMenuAt } = usePortalMenu({ height: 233 });
      // 移动菜单项把流程移交给 App 级弹窗:菜单门户与弹窗在同一次提交里
      // 卸载/挂载,被聚焦的菜单项随门户消失,弹窗的焦点还原来不及捕获它;
      // 且此刻行的 :hover/focus-within 都已失效,hover 显隐的按钮容器是
      // display:none,往里面聚焦是空操作。只能交接给常驻的行标签按钮,
      // 弹窗关闭后焦点回到出发点所在的行。
      const rowLabelRef = useRef(null);
      const openContextMenu = openMenuAt;
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
          {onMoveToProject && (
            <button type="button" className={menuItemCls} onClick={() => { rowLabelRef.current?.focus(); closeMenu(); onMoveToProject(chat); }}>
              <Layers size={15} />
              <span>{t.uiProjects.moveToProject}</span>
            </button>
          )}
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
          {onExportArchive && (
            <button type="button" className={menuItemCls} data-testid="session-export-archive" onClick={() => { closeMenu(); onExportArchive(chat.id); }}>
              <Download size={15} />
              <span>{t.exportSessionArchive}</span>
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
            {/* biome-ignore lint/a11y/noAutofocus: clicking "Rename" enters inline editing, so focus must land on the input immediately (payload behavior) */}
            <input autoFocus value={val}
              onChange={e => setVal(e.target.value)}
              onClick={e => e.stopPropagation()}
              onKeyDown={e => { if (e.key === 'Enter' && !isImeComposing(e)) { e.preventDefault(); save(); } else if (e.key === 'Escape') { setEditing(false); setVal(chat.title); } }}
              onBlur={save}
              className="w-full px-3 py-1 rounded-full text-[15px] outline-none bg-white text-[#1F1F1F] ring-1 ring-[#0B57D0] dark:bg-[#131314] dark:text-[#E3E3E3] dark:ring-[#A8C7FA]" />
          </div>
        );
      }
      // Keyboard path: the label button below is a native <button type="button">,
      // so Enter/Space activation and Tab focus come from the platform. Only the
      // label button selects the session; pin/more/delete are sibling controls,
      // so no interactive control is nested inside another one.
      return (
        // Row container: NOT interactive. It only hosts the context menu, drag
        // styling (persona target highlight / dragging opacity) and hover
        // grouping; session selection is the label button, so the action
        // buttons are siblings instead of descendants of an ARIA button.
        // biome-ignore lint/a11y/noStaticElementInteractions: right-click is a pointer-only shortcut for the same menu the "more" button opens; keyboard users reach the "more" button too (Tab reveals the hover-hidden action row via group-focus-within, menu items are real buttons, Escape closes)
        <div
          role="presentation"
          onContextMenu={openContextMenu}
          data-session-key={chat.id}
          data-drag-kind={sessionDragKind || undefined}
          title={personaTarget ? t.cpTargetMarkTitle : undefined}
          style={recentItemRowStyle(dragging, personaTarget, isDark)}
          className={`group flex h-11 items-center rounded-full text-[15px] transition-all
            ${personaTarget ? ''
              : active ? 'bg-[#E1E5EA] text-[#1F1F1F] dark:bg-[#333537] dark:text-white'
                     : 'text-[#1F1F1F] hover:bg-[#E1E5EA] dark:text-[#E3E3E3] dark:hover:bg-[#282A2C]'}`}>{/* isDark dynamic-value: 保留 (personaTarget boxShadow 运行时拼色,与 background/color 同对象) */}
          <button
            ref={rowLabelRef}
            type="button"
            data-testid={chat.testId}
            data-drag-surface
            // HTML5 拖拽(移动到项目)与 tear-off(长按 350ms)共享同一手势
            // 面:都只从标签按钮(data-drag-surface)启动,置顶/更多/删除等
            // 动作按钮起手不会拖走整行(useLongPressDrag 的按钮排除同源)。
            // 两者互斥由 hook 的工程手段提供:pointerdown 时装 capture 阶段
            // dragstart 监听({once}) + pointercancel 兜底,clearPress 幂等,
            // 自然竞态被消除;不要因"看起来多余"而删联锁。tear-off 进行中
            // (dndDisabled)不再启动 HTML5 拖拽。
            draggable={dndPayload && !dndDisabled ? true : undefined}
            onDragEnd={onDragEnd}
            onDragStart={dndPayload && !dndDisabled ? (e) => {
              e.dataTransfer.setData(PROJECT_SESSION_DRAG_TYPE, dndPayload.sessionId);
              e.dataTransfer.effectAllowed = 'move';
            } : undefined}
            onClick={sessionDragKind ? drag.guardClick(selectChat) : selectChat}
            {...dragProps}
            className="flex min-w-0 flex-1 cursor-pointer items-center self-stretch border-0 bg-transparent px-4 text-left">
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
          </button>
          {confirming ? (
            <div className="mr-4 flex items-center gap-0.5 shrink-0">
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
                <span className="text-[11px] mr-4 shrink-0 opacity-60 whitespace-nowrap group-hover:hidden group-focus-within:hidden max-sm:hidden text-[#5F6368] dark:text-[#9AA0A6]">
                  {chat.date}
                </span>
              )}
              <div className="mr-4 hidden group-hover:flex group-focus-within:flex max-sm:flex items-center gap-0.5 shrink-0">
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
    });

export { NavItem, RecentItem };
