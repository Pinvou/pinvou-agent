// Sidebar project/folder group header. Presentational only: all actions come
// in as callbacks so the component stays free of bridge/i18n-global access.
// Three visual states mirror RecentItem's patterns (inline rename edit,
// inline delete confirm, portal "more" menu via the shared usePortalMenu
// hook) to keep sidebar interaction idioms uniform.
import { useState } from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown, Edit2, FolderPlus, FolderOpen, MoreHorizontal, Plus, Trash2, X } from '../../components/icons.jsx';
import { usePortalMenu } from '../../hooks/usePortalMenu.js';
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
  // 项目通道(§9.9):项目组头的"新建会话"专属入口——cwd = 项目记忆主根,
  // 钥匙串 = 项目当时全部根;不经选择器。
  onNewSession,
  // 管理文件夹面板(§4)入口。
  onManage,
  // 指针拖拽的落点在分组容器(main.jsx 的 wrapper 带 data-drop-key,组头与
  // 会话行都算命中);这里只渲染高亮环(父级 dropActive 驱动)。
  onRebind,
  // 每个失效 root 一个徽标入口(逐根重绑定):部分失效的项目也有修复路径,
  // 且一根重绑后其余失效根的入口不会消失。
  unavailableRoots,
  dropActive,
  testId,
  headerExtra,
}) => {
  const [editing, setEditing] = useState(null);
  const [confirming, setConfirming] = useState(false);
  // 菜单门控与编辑提交判定在 ./projectGroupHeaderState.js(纯函数,有单测)。
  const hasMenu = groupHeaderHasMenu(kind, { onConvert, onRename, onDelete });
  const { menuOpen, menuStyle, closeMenu, toggleMenu } = usePortalMenu({
    height: kind === 'project' ? 96 : 48,
  });

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
      {kind === 'project' && onManage && (
        <button type="button" className={menuItemCls} onClick={() => { closeMenu(); onManage(); }}>
          <FolderOpen size={15} />
          <span>{t.uiProjects.manageFolders}</span>
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
  // another ARIA button. The drop ring is driven by the parent's hover state
  // during a pointer drag; role="presentation" declares the div non-interactive
  // to the a11y tree.
  return (
    <div
      role="presentation"
      className={`group/header w-full h-7 flex items-center rounded-full text-[12px] transition-colors ${dropActive
        ? 'ring-1 ring-[#0B57D0] bg-[#E8F0FE] dark:ring-[#A8C7FA] dark:bg-[#1F2A3D]'
        : theme === 'dark' ? 'text-[#9AA0A6] hover:bg-[#282A2C]' : 'text-[#8A8F94] hover:bg-[#E1E5EA]'}`}
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
      {/* 失效 root 逐根徽标 + 一键重绑定。不自动删项目——归属与历史仍在,
          目录接骨是唯一修复路径。徽标是切换按钮的真实兄弟 <button>(不再
          嵌在 <button> 内部),每根一个,重绑一根其余入口保留。 */}
      {(kind === 'project' ? unavailableRoots || [] : []).map((rootPath) => (
        <button
          key={rootPath}
          type="button"
          data-testid="project-folder-unavailable"
          title={rootPath}
          disabled={busy}
          onClick={(e) => { e.stopPropagation(); onRebind && onRebind(rootPath); }}
          className="mr-2 shrink-0 max-w-[9rem] truncate rounded-full bg-[#FCE8E6] dark:bg-[#3C2A29] px-2 py-0.5 text-[11px] text-[#C5221F] dark:text-[#F28B82] hover:opacity-80 disabled:opacity-50"
        >
          {t.uiProjects.folderUnavailable} · {t.uiProjects.rebindFolder}
        </button>
      ))}
      {kind === 'project' && onNewSession && (
        <div className="hidden group-hover/header:flex max-sm:flex items-center shrink-0">
          <button
            type="button"
            data-testid="project-new-session"
            title={t.uiProjects.newSessionHere}
            disabled={busy}
            onClick={(e) => { e.stopPropagation(); onNewSession(); }}
            className="w-5 h-5 rounded-full flex items-center justify-center text-[#5F6368] hover:bg-[#D3E7DB] dark:text-[#A8C7FA] dark:hover:bg-[#1F2A3D] disabled:opacity-50"
          >
            <Plus size={12} />
          </button>
        </div>
      )}
      {hasMenu && (
        // max-sm keeps the actions reachable without hover (touch, narrow
        // windows) — same contract as RecentItem's action cluster.
        <div className="mr-3 hidden group-hover/header:flex max-sm:flex items-center shrink-0">
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
