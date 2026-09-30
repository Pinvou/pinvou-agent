// Session workspace viewer (sidebar "more" menu → 查看工作区): a read-only
// view of the session's access scope — the §6 keychain snapshot, where
// roots[0] is the primary folder (the creation-time cwd; the cwd-first
// invariant never re-labels it later). Sessions without a binding (default
// workspace chat / codex temporary) show the unbound note instead — the
// dialog never fabricates a scope the session does not have. Purely
// presentational; the modal recipe (portal / Escape / focus trap / focus
// restore) follows ManageProjectFoldersDialog.
import { useEffect, useRef } from 'react';
import { createPortal } from 'react-dom';
import { FolderOpen, X } from '../../components/icons.jsx';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import { useDialogFocusRestore } from '../../hooks/useDialogFocusRestore.js';
import { useDialogFocusTrap } from '../../hooks/useDialogFocusTrap.js';
import { workspaceName } from '../../shared/workspace-recents.js';

const SessionWorkspaceDialog = ({ open, sessionTitle, roots, t, onClose }) => {
  const copy = t.uiSessionWorkspace;
  const dialogRef = useRef(null);
  const onCloseRef = useRef(onClose);
  const backdropPressRef = useRef(false);
  useDialogFocusTrap(dialogRef);
  useDialogFocusRestore(dialogRef, null, null);
  useEffect(() => { onCloseRef.current = onClose; });

  useEffect(() => {
    if (!open) return () => {};
    const onKey = (e) => {
      if (e.key === 'Escape' && !isImeComposing(e)) {
        e.preventDefault();
        onCloseRef.current();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open]);

  if (!open || typeof document === 'undefined') return null;

  const list = (Array.isArray(roots) ? roots : []).filter(Boolean).map(String);

  return createPortal(
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close; keyboard path is the Escape listener and the close button
    <div
      role="presentation"
      className="fixed inset-0 z-[200] flex items-center justify-center p-4"
      style={{ background: 'rgba(0,0,0,.34)', backdropFilter: 'blur(14px) saturate(140%)', WebkitBackdropFilter: 'blur(14px) saturate(140%)' }}
      onMouseDown={(e) => { backdropPressRef.current = e.target === e.currentTarget; }}
      onMouseUp={(e) => {
        // Two-phase close (ManageProjectFoldersDialog idiom): only a press
        // that started AND ended on the backdrop closes.
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
        data-testid="session-workspace-dialog"
        className="w-[440px] max-w-[calc(100vw-32px)] rounded-2xl bg-white dark:bg-[#202124] shadow-2xl ring-1 ring-black/10 dark:ring-white/10 p-4"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between mb-1">
          <div className="min-w-0">
            <div className="text-[15px] font-semibold text-[#1F1F1F] dark:text-[#E3E3E3]">{copy.title}</div>
            {sessionTitle && (
              <div className="mt-0.5 truncate text-[11px] text-gray-400">{sessionTitle}</div>
            )}
          </div>
          <button type="button" title={t.cpCancel} onClick={() => onCloseRef.current()}
            className="w-7 h-7 shrink-0 rounded-full flex items-center justify-center text-[#5F6368] hover:bg-black/[0.05] dark:text-[#C4C7C5] dark:hover:bg-white/[0.08]">
            <X size={15} />
          </button>
        </div>
        {list.length > 0 ? (
          <>
            <div className="mt-2 mb-1 px-1 text-[10px] uppercase tracking-wider text-gray-400">{copy.scope}</div>
            <div className="max-h-[40vh] overflow-y-auto">
              {list.map((path, index) => (
                <div key={path} title={path}
                  className="w-full px-3 py-2 flex items-center gap-2.5 text-left text-[13px] rounded-xl hover:bg-black/[0.03] dark:hover:bg-white/[0.05]">
                  <FolderOpen size={14} className={`shrink-0 ${index === 0 ? 'text-blue-500' : 'text-gray-400'}`} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[12px] font-semibold text-[#1F1F1F] dark:text-[#E3E3E3]">
                      {workspaceName(path, copy.unknownFolder)}
                    </span>
                    <span className="block truncate text-[10px] text-gray-400">{path}</span>
                  </span>
                  {index === 0 && (
                    <span className="shrink-0 rounded-full bg-blue-50 dark:bg-blue-900/30 px-2 py-0.5 text-[10px] font-medium text-blue-600 dark:text-blue-300">
                      {copy.primary}
                    </span>
                  )}
                </div>
              ))}
            </div>
          </>
        ) : (
          <div className="mt-2 rounded-xl bg-black/[0.03] dark:bg-white/[0.05] px-3 py-2.5 text-[12px] text-gray-500 dark:text-gray-400">
            {copy.unbound}
          </div>
        )}
      </div>
    </div>,
    document.body
  );
};

export { SessionWorkspaceDialog };
