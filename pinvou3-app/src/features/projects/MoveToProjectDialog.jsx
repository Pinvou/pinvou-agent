// "Move to project" picker: searchable list with the current project marked,
// plus an ungrouped entry. Purely presentational — the container passes in
// projects/assignments and receives the chosen move as
// onMove(projectId | null, addWorkspaceRoot). When the target project's roots
// do not cover the session's workspace, the picker first shows the
// add-folder confirmation (add + move vs move-only) instead of moving at once.
import { useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Check, Layers, Search, X } from '../../components/icons.jsx';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import { needsAddFolderConfirm } from './projectGrouping.js';

const MoveToProjectDialog = ({
  open,
  session,
  projects,
  currentProjectId,
  presetProjectId,
  t,
  busy,
  onClose,
  onMove,
}) => {
  const [query, setQuery] = useState('');
  // 拖拽落点直达:拖到 root 未覆盖会话目录的项目上时,直接以该目标预置
  // "添加文件夹"确认;初始化器即可(对话框每次打开都重新挂载)。
  const [pendingAddFolder, setPendingAddFolder] = useState(() => {
    if (!presetProjectId || !session) return null;
    const target = (Array.isArray(projects) ? projects.filter(Boolean) : [])
      .find(project => project.id === presetProjectId);
    if (!target) return null;
    return needsAddFolderConfirm(session, target) ? target : null;
  });
  // onClose is an inline arrow at the call site; keeping it in a ref keeps the
  // key listeners subscribed once instead of per render.
  const onCloseRef = useRef(onClose);
  const dialogRef = useRef(null);
  useEffect(() => {
    onCloseRef.current = onClose;
  });

  useEffect(() => {
    if (!open) return () => {};
    const onKey = (e) => {
      if (e.key === 'Escape' && !isImeComposing(e)) {
        e.preventDefault();
        onCloseRef.current();
      } else if (e.key === 'Tab' && dialogRef.current) {
        // Minimal focus trap: cycle Tab within the dialog instead of letting
        // it escape into the page behind the modal.
        const focusables = dialogRef.current.querySelectorAll(
          'button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex="-1"])',
        );
        if (!focusables.length) return;
        const first = focusables[0];
        const last = focusables[focusables.length - 1];
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open]);

  const projectList = useMemo(
    () => (Array.isArray(projects) ? projects.filter(Boolean) : []),
    [projects],
  );
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return projectList;
    return projectList.filter((project) => {
      const name = String(project.name || '').toLowerCase();
      const roots = (project.roots || [])
        .map((root) => String((root && typeof root === 'object' ? root.path : root) || ''))
        .join(' ');
      return name.includes(q) || roots.toLowerCase().includes(q);
    });
  }, [projectList, query]);

  if (!open || !session || typeof document === 'undefined') return null;

  // 显示用:确认框里向用户展示的目录(侧栏投影),实际添加以命令返回为准。
  const workspacePath = session.workspaceKind === 'project' ? String(session.workspacePath || '') : '';
  const choose = (project) => {
    if (busy || project.id === currentProjectId) return;
    if (needsAddFolderConfirm(session, project)) {
      setPendingAddFolder(project);
      return;
    }
    onMove(project.id, false);
  };
  const commitPending = (addFolder) => {
    const target = pendingAddFolder;
    setPendingAddFolder(null);
    if (target && !busy) onMove(target.id, addFolder);
  };

  const rowCls = 'w-full px-3.5 py-2.5 flex items-center gap-2.5 text-left text-[14px] rounded-2xl transition-colors text-[#1F1F1F] hover:bg-[#F1F3F4] dark:text-[#E3E3E3] dark:hover:bg-[#303134]';
  const projectLabel = (project) => {
    const firstRoot = (project.roots || [])[0];
    const rootPath = firstRoot ? String(typeof firstRoot === 'object' ? firstRoot.path : firstRoot) : '';
    return (
      <span className="min-w-0 flex-1">
        <span className="block truncate">{project.name}</span>
        {rootPath && <span className="block truncate text-[12px] text-[#8A8F94] dark:text-[#9AA0A6]">{rootPath}</span>}
      </span>
    );
  };

  return createPortal(
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close; keyboard path is the Escape listener and the cancel button
    <div
      role="presentation"
      className="fixed inset-0 z-[200] flex items-center justify-center p-4"
      style={{ background: 'rgba(0,0,0,.34)', backdropFilter: 'blur(14px) saturate(140%)', WebkitBackdropFilter: 'blur(14px) saturate(140%)' }}
      onClick={onClose}
    >
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: dialog body stops bubbling so backdrop close is not triggered accidentally; not interactive itself */}
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-label={t.uiProjects.moveToProject}
        onClick={e => e.stopPropagation()}
        className="w-[360px] max-w-[calc(100vw-48px)] overflow-hidden rounded-[16px] shadow-2xl bg-[rgba(250,250,250,.96)] dark:bg-[rgba(44,44,46,.96)] text-[#000] dark:text-[#F2F2F7]"
        style={{ fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}
      >
        <div className="px-4 pt-4 pb-2 flex items-center justify-between gap-2">
          <div className="min-w-0">
            <div className="text-[15px] font-semibold truncate">{t.uiProjects.moveToProject}</div>
            <div className="text-[12px] text-[#8A8F94] dark:text-[#9AA0A6] truncate" title={session.title}>{session.title}</div>
          </div>
          <button
            type="button"
            title={t.cpCancel}
            onClick={onClose}
            className="w-8 h-8 shrink-0 rounded-full flex items-center justify-center text-[#5F6368] hover:bg-[#D3D7DB] dark:text-[#C4C7C5] dark:hover:bg-[#444746]"
          >
            <X size={16} />
          </button>
        </div>
        <div className="px-4 pb-2">
          <div className="flex h-9 items-center gap-2 rounded-full px-3 bg-[#EAECEF] dark:bg-[#303134]">
            <Search size={14} className="shrink-0 text-[#5F6368] dark:text-[#9AA0A6]" />
            {/* biome-ignore lint/a11y/noAutofocus: modal opens for a single purpose; focus belongs in the filter field immediately */}
            <input autoFocus
              value={query}
              onChange={e => setQuery(e.target.value)}
              placeholder={t.uiProjects.searchPlaceholder}
              className="w-full bg-transparent border-0 outline-none text-[14px] placeholder:text-[#8A8F94] dark:placeholder:text-[#9AA0A6]"
            />
          </div>
        </div>
        {pendingAddFolder ? (
          <div className="px-4 pb-4 pt-1">
            <div className="rounded-2xl bg-[#EAECEF] dark:bg-[#303134] px-3.5 py-3">
              <div className="text-[13px] font-semibold mb-1">{t.uiProjects.addFolderTitle}</div>
              <div className="text-[12px] text-[#5F6368] dark:text-[#C4C7C5] mb-3 break-all">{t.uiProjects.addFolderBody(workspacePath)}</div>
              <div className="flex gap-2">
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => commitPending(true)}
                  className="flex-1 h-9 rounded-full bg-[#0B57D0] text-white text-[13px] font-medium hover:bg-[#0A4CB8] disabled:opacity-50"
                >
                  {t.uiProjects.addFolderConfirm}
                </button>
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => commitPending(false)}
                  className="flex-1 h-9 rounded-full bg-[#D3D7DB] dark:bg-[#444746] text-[#1F1F1F] dark:text-[#E3E3E3] text-[13px] font-medium hover:opacity-90 disabled:opacity-50"
                >
                  {t.uiProjects.moveOnly}
                </button>
              </div>
            </div>
          </div>
        ) : (
          <div className="px-2 pb-3 max-h-[320px] overflow-y-auto">
            {filtered.length === 0 && (
              <div className="px-3.5 py-4 text-[13px] text-[#8A8F94] dark:text-[#9AA0A6]">
                {projectList.length === 0 ? t.uiProjects.noProjects : t.uiProjects.noMatchProject}
              </div>
            )}
            {filtered.map(project => (
              <button
                key={project.id}
                type="button"
                disabled={busy || project.id === currentProjectId}
                onClick={() => choose(project)}
                className={`${rowCls} disabled:opacity-60`}
              >
                <Layers size={15} className="shrink-0 text-[#5F6368] dark:text-[#9AA0A6]" />
                {projectLabel(project)}
                {project.id === currentProjectId && (
                  <span className="shrink-0 flex items-center gap-1 text-[11px] text-[#0B57D0] dark:text-[#A8C7FA]">
                    <Check size={12} />
                    {t.uiProjects.currentProject}
                  </span>
                )}
              </button>
            ))}
            <div className="my-1 h-px bg-black/10 dark:bg-white/10" />
            <button
              type="button"
              disabled={busy || !currentProjectId}
              onClick={() => !busy && onMove(null, false)}
              className={`${rowCls} disabled:opacity-40`}
            >
              <X size={15} className="shrink-0 text-[#5F6368] dark:text-[#9AA0A6]" />
              <span className="min-w-0 flex-1 truncate">{t.uiProjects.moveToUngrouped}</span>
            </button>
          </div>
        )}
      </div>
    </div>,
    document.body,
  );
};

export { MoveToProjectDialog };
