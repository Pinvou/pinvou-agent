// Manage-folders panel (§4): view/add/remove project roots, primary-folder
// memory (set as primary), rename, and view/revoke of the anti-
// materialization exclusion list. Purely presentational: every action goes to
// the container (main.jsx) via props callbacks; the component never touches
// Tauri globals. The dialog conventions (portal/Escape/focus trap/busy)
// follow MoveToProjectDialog.
import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { AlertTriangle, FolderOpen, FolderPlus, PinIcon, Trash2, X } from '../../components/icons.jsx';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import { useDialogFocusRestore } from '../../hooks/useDialogFocusRestore.js';
import { useDialogFocusTrap } from '../../hooks/useDialogFocusTrap.js';
import { manageFolderRows } from './manageFoldersState.js';
import { workspaceNoticeTone } from './workspacePickerState.js';

const ManageProjectFoldersDialog = ({
  open,
  project,
  neverRoots,
  mode,
  busy,
  t,
  onClose,
  onAddFolder,
  onRemoveRoot,
  onSetPrimary,
  onRename,
  onExcludeRoot,
  onRevokeExclusion,
}) => {
  const copy = t.uiManageFolders;
  // Confirm states: pendingRemove = the root row awaiting removal;
  // removingPrimary = the primary-removal interception hint.
  const [pendingRemove, setPendingRemove] = useState(null);
  const [renaming, setRenaming] = useState(null);
  const onCloseRef = useRef(onClose);
  const dialogRef = useRef(null);
  const pendingRemoveRef = useRef(null);
  // Escape grading needs the latest rename/busy state inside the window
  // keydown listener: a focused rename input must collapse its own draft
  // first, not close the whole panel, and a busy panel must not mis-close on
  // a backdrop drag (review #484 M5). Mirrors use effects — refs are not
  // written during render.
  const renamingRef = useRef(null);
  const backdropPressRef = useRef(false);
  const busyRef = useRef(busy);
  // Shared modal recipe (review #484 M5): Tab trap + focus capture/restore
  // come from the shared hooks — the hand-rolled trap dropped focus when the
  // busy state disabled every control, and focus was never restored on
  // close. Only the Escape tiering stays in-component.
  useDialogFocusTrap(dialogRef);
  useDialogFocusRestore(dialogRef, null, null);
  // After a project switch / data refresh the confirm state may be stale (the
  // row is gone): derive it at render time instead of writing state.
  const rows = manageFolderRows(project);
  const activePendingRemove = pendingRemove && rows.some(row => row.path === pendingRemove.path)
    ? pendingRemove
    : null;
  useEffect(() => {
    onCloseRef.current = onClose;
    pendingRemoveRef.current = activePendingRemove;
    renamingRef.current = renaming;
    busyRef.current = busy;
  });

  useEffect(() => {
    if (!open) return () => {};
    const onKey = (e) => {
      if (e.key === 'Escape' && !isImeComposing(e)) {
        e.preventDefault();
        if (renamingRef.current !== null) { setRenaming(null); return; }
        if (pendingRemoveRef.current) { setPendingRemove(null); return; }
        // Busy gate parity with the backdrop path (busyRef interception):
        // closing mid-operation would destroy the in-place retry context the
        // panel deliberately preserves (review #484 m3).
        if (busyRef.current) return;
        onCloseRef.current();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open]);

  if (!open || !project || typeof document === 'undefined') return null;

  const exclusionList = Array.isArray(neverRoots) ? neverRoots : [];
  const canExclude = (row) => project.origin === 'folder' && row.isPrimary;

  const commitRename = () => {
    const draft = renaming;
    setRenaming(null);
    if (!draft || busy) return;
    const value = String(draft || '').trim();
    if (value && value !== project.name) onRename(value);
  };

  const rowCls = 'w-full px-3 py-2 flex items-center gap-2.5 text-left text-[13px] rounded-xl';

  return createPortal(
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close; keyboard path is the Escape listener and the close button
    <div
      role="presentation"
      className="fixed inset-0 z-[200] flex items-center justify-center p-4"
      style={{ background: 'rgba(0,0,0,.34)', backdropFilter: 'blur(14px) saturate(140%)', WebkitBackdropFilter: 'blur(14px) saturate(140%)' }}
      onMouseDown={(e) => { backdropPressRef.current = e.target === e.currentTarget && !busyRef.current; }}
      onMouseUp={(e) => {
        // Two-phase close (MoveToProjectDialog idiom): a press that started
        // on the backdrop AND ended there closes — a drag-select that begins
        // inside the panel must not. Closing while busy would destroy the
        // in-place retry context the panel deliberately preserves.
        if (backdropPressRef.current && e.target === e.currentTarget) onCloseRef.current();
        backdropPressRef.current = false;
      }}
    >
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: dialog body stops bubbling so backdrop close is not triggered accidentally; not interactive itself */}
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-label={copy.title}
        onClick={e => e.stopPropagation()}
        className="w-[420px] max-w-[calc(100vw-48px)] overflow-hidden rounded-[16px] shadow-2xl bg-[rgba(250,250,250,.96)] dark:bg-[rgba(44,44,46,.96)] text-[#000] dark:text-[#F2F2F7]"
        style={{ fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}
      >
        <div className="px-4 pt-4 pb-2 flex items-center justify-between gap-2">
          <div className="min-w-0 flex-1">
            <div className="text-[15px] font-semibold truncate">{copy.title}</div>
            {renaming === null ? (
              <button
                type="button"
                disabled={busy}
                onClick={() => setRenaming(project.name)}
                title={copy.renameHint}
                className="block max-w-full truncate text-[12px] text-[#8A8F94] dark:text-[#9AA0A6] hover:text-[#0B57D0] dark:hover:text-[#A8C7FA]"
              >
                {project.name}
              </button>
            ) : (
              /* biome-ignore lint/a11y/noAutofocus: rename lands focus in the input immediately (same idiom as group header rename) */
              <input autoFocus
                value={renaming}
                disabled={busy}
                onChange={e => setRenaming(e.target.value)}
                onKeyDown={e => {
                  if (e.key === 'Enter' && !isImeComposing(e)) { e.preventDefault(); commitRename(); }
                  if (e.key === 'Escape') { e.preventDefault(); setRenaming(null); }
                }}
                onBlur={commitRename}
                className="mt-0.5 w-full h-6 px-2 rounded-full text-[12px] outline-none bg-white text-[#1F1F1F] ring-1 ring-[#0B57D0] dark:bg-[#131314] dark:text-[#E3E3E3] dark:ring-[#A8C7FA]"
              />
            )}
          </div>
          <button
            type="button"
            title={t.cpCancel}
            disabled={busy}
            onClick={onClose}
            className="w-8 h-8 shrink-0 rounded-full flex items-center justify-center text-[#5F6368] hover:bg-[#D3D7DB] dark:text-[#C4C7C5] dark:hover:bg-[#444746] disabled:opacity-50"
          >
            <X size={16} />
          </button>
        </div>

        <div className="px-2 pb-3 max-h-[360px] overflow-y-auto">
          {rows.length === 0 && (
            <div className="px-3.5 py-3 text-[13px] text-[#8A8F94] dark:text-[#9AA0A6]">{copy.emptyRoots}</div>
          )}
          {rows.map(row => (
            <div key={row.path}>
              <div className={rowCls} title={row.path}>
                <FolderOpen size={14} className={`shrink-0 ${row.available ? 'text-[#5F6368] dark:text-[#9AA0A6]' : 'text-[#C5221F] dark:text-[#F28B82]'}`} />
                <span className="min-w-0 flex-1 truncate">{row.path}</span>
                {!row.available && (
                  <span className="shrink-0 text-[10px] text-[#C5221F] dark:text-[#F28B82]">{t.uiProjects.folderUnavailable}</span>
                )}
                {row.isPrimary && (
                  <span className="shrink-0 text-[10px] text-[#0B57D0] dark:text-[#A8C7FA]">{copy.primaryBadge}</span>
                )}
                {!row.isPrimary && (
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => onSetPrimary(row.path)}
                    title={copy.setPrimaryHint}
                    className="shrink-0 w-6 h-6 rounded-full flex items-center justify-center text-[#8A8F94] hover:bg-[#D3D7DB] dark:hover:bg-[#444746] disabled:opacity-50"
                  >
                    <PinIcon size={12} />
                  </button>
                )}
                {canExclude(row) && (
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => onExcludeRoot(row.path)}
                    title={copy.neverMaterializeHint}
                    className="shrink-0 rounded-full px-2 py-0.5 text-[10px] text-[#8A8F94] hover:bg-[#D3D7DB] dark:hover:bg-[#444746] disabled:opacity-50"
                  >
                    {copy.neverMaterializeAction}
                  </button>
                )}
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => setPendingRemove(row)}
                  title={copy.removeAction}
                  className="shrink-0 w-6 h-6 rounded-full flex items-center justify-center text-[#C5221F] hover:bg-[#FAD2CF] dark:text-[#F28B82] dark:hover:bg-[#5c2b29] disabled:opacity-50"
                >
                  <Trash2 size={12} />
                </button>
              </div>
              {activePendingRemove && activePendingRemove.path === row.path && (
                <div className="mx-2 mb-1 rounded-xl bg-[#FCE8E6] dark:bg-[#3C2A29] px-3 py-2.5">
                  <div className="flex items-start gap-1.5 text-[12px] text-[#C5221F] dark:text-[#F28B82]">
                    <AlertTriangle size={13} className="shrink-0 mt-0.5" />
                    <span>{copy.removeConfirmBody}</span>
                  </div>
                  {activePendingRemove.isPrimary && rows.length > 1 ? (
                    <div className="mt-2 text-[12px] text-[#5F6368] dark:text-[#C4C7C5]">{copy.removePrimaryBlocked}</div>
                  ) : (
                    <div className="mt-2 flex gap-2">
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => { const target = activePendingRemove; setPendingRemove(null); onRemoveRoot(target.path); }}
                        className="flex-1 h-8 rounded-full bg-[#C5221F] text-white text-[12px] font-medium hover:opacity-90 disabled:opacity-50"
                      >
                        {copy.removeConfirm}
                      </button>
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => setPendingRemove(null)}
                        className="flex-1 h-8 rounded-full bg-[#D3D7DB] dark:bg-[#444746] text-[#1F1F1F] dark:text-[#E3E3E3] text-[12px] font-medium hover:opacity-90 disabled:opacity-50"
                      >
                        {t.cpCancel}
                      </button>
                    </div>
                  )}
                </div>
              )}
            </div>
          ))}

          <div className="my-1 mx-2 h-px bg-black/10 dark:bg-white/10" />
          <button
            type="button"
            disabled={busy}
            onClick={onAddFolder}
            className={`${rowCls} text-[#0B57D0] dark:text-[#A8C7FA] hover:bg-[#F1F3F4] dark:hover:bg-[#303134] disabled:opacity-60`}
          >
            <FolderPlus size={14} className="shrink-0" />
            <span className="min-w-0 flex-1">
              <span className="block truncate">{copy.addAction}</span>
              <span className="block truncate text-[11px] text-[#8A8F94] dark:text-[#9AA0A6]">
                {workspaceNoticeTone(mode) === 'restricted' ? copy.addNoticeRestricted : copy.addNoticeVisibility}
              </span>
            </span>
          </button>

          {exclusionList.length > 0 && (
            <div className="mt-2 px-3">
              <div className="pb-1 text-[10px] uppercase tracking-wider text-[#8A8F94] dark:text-[#9AA0A6]">
                {copy.exclusionSection}
              </div>
              {exclusionList.map(root => (
                <div key={root} className="py-1 flex items-center gap-2 text-[12px]" title={root}>
                  <FolderOpen size={13} className="shrink-0 text-gray-400" />
                  <span className="min-w-0 flex-1 truncate">{root}</span>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => onRevokeExclusion(root)}
                    className="shrink-0 rounded-full px-2 py-0.5 text-[11px] text-[#0B57D0] hover:bg-[#E8F0FE] dark:text-[#A8C7FA] dark:hover:bg-[#1F2A3D] disabled:opacity-50"
                  >
                    {copy.revokeExclusion}
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
};

export { ManageProjectFoldersDialog };
