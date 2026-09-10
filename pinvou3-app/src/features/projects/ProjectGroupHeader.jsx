// Sidebar project/folder group header. Presentational only: all actions come
// in as callbacks so the component stays free of bridge/i18n-global access.
// Three visual states mirror RecentItem's patterns (inline rename edit,
// inline delete confirm, portal "more" menu via the shared usePortalMenu
// hook) to keep sidebar interaction idioms uniform.
import { useState } from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown, Edit2, FolderPlus, MoreHorizontal, Trash2, X } from '../../components/icons.jsx';
import { usePortalMenu } from '../../hooks/usePortalMenu.js';
import { capUnavailableRootsForDisplay, PROJECT_SESSION_DRAG_TYPE } from './projectGrouping.js';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import { groupHeaderHasMenu, resolveGroupHeaderEdit } from './projectGroupHeaderState.js';

const ProjectGroupHeader = ({
  label,
  kind,
  count,
  isOpen,
  onToggle,
  theme,
  t,
  title,
  busy,
  onConvert,
  onRename,
  onDelete,
  onDropSession,
  onRebind,
  // One badge entry per unavailable root (per-root rebind): projects with
  // only some roots unavailable also get a repair path, and rebinding one
  // root does not remove the entries for the remaining unavailable roots.
  unavailableRoots,
  // Highlight ownership lives in the sidebar container (one drop target lit at
  // a time) so the source row's dragend can clear it unconditionally even when
  // a webview skips dragleave/drop.
  dropActive,
  onDropActive,
  testId,
  headerExtra,
}) => {
  const [editing, setEditing] = useState(null);
  const [confirming, setConfirming] = useState(false);
  // With several unavailable roots, collapse to "first badge + N": the 28px
  // header row cannot fit multiple shrink-0 badges (review #463 m3); the
  // expanded state lays them out flat and allows wrapping.
  const [showAllUnavailableRoots, setShowAllUnavailableRoots] = useState(false);
  // 菜单门控与编辑提交判定在 ./projectGroupHeaderState.js(纯函数,有单测)。
  const hasMenu = groupHeaderHasMenu(kind, { onConvert, onRename, onDelete });
  const { menuOpen, menuStyle, closeMenu, toggleMenu } = usePortalMenu({
    height: kind === 'project' ? 96 : 48,
  });

  // HTML5 drop-target handlers for the sidebar session drag; kept out of the
  // JSX so the row render stays flat. dragover highlights, drop delegates the
  // session id up, dragend/dragleave clear the highlight (dragend fires on the
  // source row and can be skipped by the webview — the ring here also clears
  // unconditionally on drop).
  const dropHandlers = onDropSession ? {
    onDragOver: (e) => {
      if (!e.dataTransfer.types.includes(PROJECT_SESSION_DRAG_TYPE)) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = 'move';
      onDropActive(true);
    },
    onDragLeave: () => onDropActive(false),
    onDrop: (e) => {
      e.preventDefault();
      onDropActive(false);
      const sessionId = e.dataTransfer.getData(PROJECT_SESSION_DRAG_TYPE);
      if (sessionId) onDropSession(sessionId);
    },
  } : {};

  const startConvert = () => setEditing({ mode: 'convert', value: label });
  const startRename = () => setEditing({ mode: 'rename', value: label });
  const commitEdit = () => {
    const draft = editing;
    setEditing(null);
    if (!draft) return;
    const edit = resolveGroupHeaderEdit({ mode: draft.mode, value: draft.value, label, busy });
    if (!edit) return;
    if (edit.action === 'convert' && onConvert) onConvert(edit.value);
    if (edit.action === 'rename' && onRename) onRename(edit.value);
  };

  const menuItemCls = 'w-full h-9 px-3 flex items-center gap-2 text-left text-[14px] whitespace-nowrap transition-colors text-[#1F1F1F] hover:bg-[#F1F3F4] dark:text-[#E3E3E3] dark:hover:bg-[#303134]';
  const menu = menuOpen && menuStyle && typeof document !== 'undefined' ? createPortal(
    <div
      onPointerDown={e => e.stopPropagation()}
      data-testid="sidebar-project-group-menu"
      className="fixed z-[1000] overflow-hidden rounded-xl py-1 shadow-xl ring-1 bg-white ring-black/10 dark:bg-[#202124] dark:ring-white/10"
      style={menuStyle}
    >
      {kind === 'folder' && onConvert && (
        <button type="button" className={menuItemCls} onClick={() => { closeMenu(); startConvert(); }}>
          <FolderPlus size={15} />
          <span>{t.uiProjects.convertToProject}</span>
        </button>
      )}
      {kind === 'project' && onRename && (
        <button type="button" className={menuItemCls} onClick={() => { closeMenu(); startRename(); }}>
          <Edit2 size={15} />
          <span>{t.uiProjects.renameProject}</span>
        </button>
      )}
      {kind === 'project' && onDelete && (
        <button
          type="button"
          className={`${menuItemCls} text-[#C5221F] hover:bg-[#FAD2CF] dark:text-[#F28B82] dark:hover:bg-[#5c2b29]`}
          onClick={() => { closeMenu(); setConfirming(true); }}
        >
          <Trash2 size={15} />
          <span>{t.uiProjects.deleteProject}</span>
        </button>
      )}
    </div>,
    document.body,
  ) : null;

  if (editing) {
    return (
      <div className="flex h-7 items-center px-4">
        {/* biome-ignore lint/a11y/noAutofocus: converting/renaming lands focus in the input immediately (same idiom as RecentItem rename) */}
        <input autoFocus
          value={editing.value}
          disabled={busy}
          onChange={e => setEditing({ ...editing, value: e.target.value })}
          onClick={e => e.stopPropagation()}
          onKeyDown={e => {
            if (e.key === 'Enter' && !isImeComposing(e)) {
              e.preventDefault();
              commitEdit();
            } else if (e.key === 'Escape') {
              e.preventDefault();
              setEditing(null);
            }
          }}
          onBlur={() => {
            // convert 的预填值即「默认项目名」,blur 提交会把「点击别处取消」
            // 变成无确认静默建项目(评审 #471 finding 46);rename 保持
            // RecentItem 的 blur 提交惯例。
            if (editing.mode === 'convert') {
              setEditing(null);
            } else {
              commitEdit();
            }
          }}
          placeholder={t.uiProjects.projectNamePlaceholder}
          className="w-full h-6 px-3 rounded-full text-[12px] outline-none bg-white text-[#1F1F1F] ring-1 ring-[#0B57D0] dark:bg-[#131314] dark:text-[#E3E3E3] dark:ring-[#A8C7FA]"
        />
      </div>
    );
  }

  if (confirming) {
    return (
      <div className="w-full h-7 px-4 flex items-center justify-between rounded-full text-[12px] text-[#C5221F] dark:text-[#F28B82]">
        <span className="truncate" title={t.uiProjects.deleteProjectHint}>
          {t.uiProjects.deleteConfirmLabel} ({count})
        </span>
        <span className="flex items-center gap-0.5 shrink-0">
          <button
            type="button"
            title={t.uiProjects.deleteProject}
            disabled={busy}
            onClick={(e) => { e.stopPropagation(); if (onDelete) { onDelete(); } setConfirming(false); }}
            className="w-5 h-5 rounded-full flex items-center justify-center hover:bg-[#FAD2CF] dark:hover:bg-[#5c2b29]"
          >
            <Check size={13} />
          </button>
          <button
            type="button"
            title={t.cpCancel}
            onClick={(e) => { e.stopPropagation(); setConfirming(false); }}
            className="w-5 h-5 rounded-full flex items-center justify-center text-[#5F6368] hover:bg-[#D3E7DB] dark:text-[#C4C7C5] dark:hover:bg-[#444746]"
          >
            <X size={12} />
          </button>
        </span>
      </div>
    );
  }

  // Row container is NOT interactive (same idiom as RecentItem): the toggle
  // button and the "more" button are siblings, so no control nests inside
  // another ARIA button. Project headers double as HTML5 drop targets for the
  // sidebar session drag; role="presentation" declares the div
  // non-interactive to the a11y tree while it carries the drag handlers.
  const unavailableRootList = kind === 'project' ? unavailableRoots || [] : [];
  const { visibleRoots, hiddenCount } = capUnavailableRootsForDisplay(unavailableRootList, showAllUnavailableRoots);
  const wrapUnavailable = visibleRoots.length > 1;
  return (
    <div
      role="presentation"
      {...dropHandlers}
      className={`group/header ${wrapUnavailable ? 'w-full min-h-7 h-auto flex-wrap' : 'w-full h-7'} flex items-center rounded-full text-[12px] transition-colors ${dropActive
        ? 'ring-1 ring-[#0B57D0] bg-[#E8F0FE] dark:ring-[#A8C7FA] dark:bg-[#1F2A3D]'
        : theme === 'dark' ? 'text-[#9AA0A6] hover:bg-[#282A2C]' : 'text-[#8A8F94] hover:bg-[#E1E5EA]'}`}
      data-drop-target={onDropSession ? 'project' : undefined}
    >
      <button
        type="button"
        data-testid={testId}
        title={title}
        onClick={onToggle}
        className="flex min-w-0 flex-1 self-stretch items-center border-0 bg-transparent px-4 text-left"
      >
        <span className="min-w-0 flex-1 truncate pr-2">{label} ({count})</span>
        {headerExtra}
        <ChevronDown size={14} className={`shrink-0 transition-transform ${isOpen ? '' : '-rotate-90'}`} />
      </button>
      {/* Per-root badges for unavailable roots with one-click rebind. The
          project is never auto-deleted — ownership and history remain, and
          rebinding the folder is the only repair path. Each badge is a real
          sibling <button> of the toggle button (no longer nested inside a
          <button>), one per root; rebinding one keeps the other entries.
          Multiple roots collapse into a +N expander (m3); the expanded
          container wraps instead of overflowing. */}
      {visibleRoots.map((rootPath) => (
        <button
          key={rootPath}
          type="button"
          data-testid="project-folder-unavailable"
          title={rootPath}
          aria-label={`${t.uiProjects.folderUnavailable} · ${t.uiProjects.rebindFolder} · ${rootPath}`}
          disabled={busy}
          onClick={(e) => { e.stopPropagation(); onRebind && onRebind(rootPath); }}
          className="mr-2 shrink-0 max-w-[9rem] truncate rounded-full bg-[#FCE8E6] dark:bg-[#3C2A29] px-2 py-0.5 text-[11px] text-[#C5221F] dark:text-[#F28B82] hover:opacity-80 disabled:opacity-50"
        >
          {t.uiProjects.folderUnavailable} · {t.uiProjects.rebindFolder}
        </button>
      ))}
      {/* Multi-root collapse toggle (m3/Nit 13): collapsed shows +N and can
          expand; expanded wraps the container, and the same button collapses
          again. Both states carry an aria-label, and each badge's accessible
          name is distinguished by its path. */}
      {unavailableRootList.length > 1 && (
        <button
          type="button"
          data-testid="project-folder-unavailable-more"
          aria-label={showAllUnavailableRoots ? t.uiProjects.rebindRootsCollapse : t.uiProjects.rebindRootsExpand}
          title={showAllUnavailableRoots ? t.uiProjects.rebindRootsCollapse : unavailableRootList.slice(1).join('\n')}
          disabled={busy}
          onClick={(e) => { e.stopPropagation(); setShowAllUnavailableRoots(v => !v); }}
          className="mr-2 shrink-0 flex items-center gap-0.5 rounded-full bg-[#FCE8E6] dark:bg-[#3C2A29] px-2 py-0.5 text-[11px] font-medium text-[#C5221F] dark:text-[#F28B82] hover:opacity-80 disabled:opacity-50"
        >
          {!showAllUnavailableRoots && <span>+{hiddenCount}</span>}
          <ChevronDown size={11} className={`shrink-0 transition-transform ${showAllUnavailableRoots ? '' : '-rotate-90'}`} />
        </button>
      )}
      {hasMenu && (
        // max-sm keeps the actions reachable without hover (touch, narrow
        // windows), group-focus-within reveals them for keyboard users —
        // same contract as RecentItem's action cluster.
        <div className="mr-3 hidden group-hover/header:flex group-focus-within/header:flex max-sm:flex items-center shrink-0">
          <button
            type="button"
            title={t.riMore}
            onClick={toggleMenu}
            className="w-5 h-5 rounded-full flex items-center justify-center text-[#5F6368] hover:bg-[#D3D7DB] dark:text-[#C4C7C5] dark:hover:bg-[#444746]"
          >
            <MoreHorizontal size={12} />
          </button>
        </div>
      )}
      {menu}
    </div>
  );
};

export { ProjectGroupHeader };
