// Sidebar project/folder group header. Presentational only: all actions come
// in as callbacks so the component stays free of bridge/i18n-global access.
// Three visual states mirror RecentItem's patterns (inline rename edit,
// inline delete confirm, portal "more" menu on hover) to keep sidebar
// interaction idioms uniform.
import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown, Edit2, FolderPlus, MoreHorizontal, Trash2, X } from '../../components/icons.jsx';
import { isImeComposing } from '../../shared/ime-guard.mjs';

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
  testId,
  headerExtra,
}) => {
  const [editing, setEditing] = useState(null);
  const [confirming, setConfirming] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [menuStyle, setMenuStyle] = useState(null);
  const hasMenu = kind === 'folder' || kind === 'project';

  const closeMenu = () => setMenuOpen(false);
  const placeMenu = (target) => {
    const rect = target.getBoundingClientRect();
    const width = 176;
    const height = kind === 'project' ? 96 : 48;
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

  useEffect(() => {
    if (!menuOpen) {
      return () => {};
    }
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
  const startConvert = () => setEditing({ mode: 'convert', value: label });
  const startRename = () => setEditing({ mode: 'rename', value: label });
  const commitEdit = () => {
    const draft = editing;
    setEditing(null);
    if (!draft) return;
    const value = String(draft.value || '').trim();
    if (!value || value === label || busy) return;
    if (draft.mode === 'convert' && onConvert) onConvert(value);
    if (draft.mode === 'rename' && onRename) onRename(value);
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
        <input
          autoFocus
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
          onBlur={commitEdit}
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
          {t.uiProjects.deleteConfirmLabel}（{count}）
        </span>
        <span className="flex items-center gap-0.5 shrink-0">
          <button
            type="button"
            title={t.uiProjects.deleteProject}
            disabled={busy}
            onClick={(e) => { e.stopPropagation(); onDelete && onDelete(); setConfirming(false); }}
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
  // another ARIA button.
  return (
    <div
      className={`group/header w-full h-7 flex items-center rounded-full text-[12px] transition-colors ${theme === 'dark' ? 'text-[#9AA0A6] hover:bg-[#282A2C]' : 'text-[#8A8F94] hover:bg-[#E1E5EA]'}`}
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
      {hasMenu && (
        <div className="mr-3 hidden group-hover/header:flex items-center shrink-0">
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
