// Draft-state working directory picker for plain chat, mirroring the code
// mode's interaction (CodexAcpView's draft-state picker): a composer-footer
// button plus a popover menu (Choose directory… / Default workspace / Recent).
// Rendered only in draft state (ChatView guards on !activeSessionId plus
// capability/method existence); all backend actions are injected via props
// (bridge.sessions's setDraftWorkspace / pickDraftWorkspace) and the component
// never touches Tauri globals.
import { useRef, useState } from 'react';
import { ChevronDown, FolderOpen, Sparkles } from '../../components/icons.jsx';
import { useOutsidePointerClose } from '../../components/ComposerPopover.jsx';
import { loadRecentWorkspaces, workspaceName } from '../../shared/workspace-recents.js';

export function ComposerWorkspaceSelector({ copy, draftWorkspacePath, onPickWorkspace, onSelectWorkspace }) {
  const [open, setOpen] = useState(false);
  const [recentWorkspaces, setRecentWorkspaces] = useState(loadRecentWorkspaces);
  const [pickError, setPickError] = useState('');
  const triggerRef = useRef(null);
  const panelRef = useRef(null);
  useOutsidePointerClose(open, () => setOpen(false), [panelRef, triggerRef]);

  function toggle() {
    const next = !open;
    setOpen(next);
    if (next) setRecentWorkspaces(loadRecentWorkspaces()); // re-read on open: the code mode may have recorded a new directory
  }
  function chooseDirectory() {
    setOpen(false);
    setPickError('');
    onPickWorkspace()
      .then(path => { if (path) setRecentWorkspaces(loadRecentWorkspaces()); })
      // Directory dialog failures (including an old backend without the
      // command) must be visible — silently closing the menu would make the
      // user think the selection took effect (the code lane surfaces this
      // through the page error surface).
      .catch(error => setPickError(String((error && error.message) || error || 'error')));
  }
  function select(path) {
    setOpen(false);
    setPickError('');
    onSelectWorkspace(path);
  }

  return (
    <div className="relative min-w-0">
      <button
        type="button"
        ref={triggerRef}
        data-testid="chat-workspace-selector"
        onClick={toggle}
        className="h-7 max-w-[180px] rounded-lg px-2 inline-flex items-center gap-1.5 text-[11px] text-gray-500 dark:text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]"
        title={draftWorkspacePath || copy.defaultWorkspace}
      >
        {draftWorkspacePath
          ? <FolderOpen size={13} className="shrink-0" />
          : <Sparkles size={13} className="shrink-0 text-emerald-500" />}
        <span className="truncate">
          {draftWorkspacePath ? workspaceName(draftWorkspacePath, copy.unknownDirectory) : copy.defaultWorkspace}
        </span>
        <ChevronDown size={12} className="shrink-0" />
      </button>
      {pickError && (
        <div className="absolute z-40 bottom-9 left-0 w-[280px] max-w-[calc(100vw-32px)] rounded-xl border border-red-200 dark:border-red-900/50 bg-white/95 dark:bg-[#202124]/95 shadow-xl px-3 py-2 text-[11px] text-[#C5221F] dark:text-red-400">
          {pickError}
        </div>
      )}
      {open && (
        <div ref={panelRef} className="absolute z-40 bottom-9 left-0 w-[280px] max-w-[calc(100vw-32px)] rounded-2xl border border-black/[0.08] dark:border-white/10 bg-white/95 dark:bg-[#202124]/95 backdrop-blur-xl shadow-xl p-2">
          <button type="button" onClick={chooseDirectory}
            className="w-full rounded-xl px-3 py-2.5 flex items-center gap-3 text-left hover:bg-black/[0.04] dark:hover:bg-white/[0.06]">
            <FolderOpen size={16} className="text-blue-500 shrink-0" />
            <span><span className="block text-[12px] font-semibold">{copy.chooseDirectory}</span><span className="block text-[10px] text-gray-400 mt-0.5">{copy.chooseDirectoryDesc}</span></span>
          </button>
          <button type="button" onClick={() => select(null)}
            className="w-full rounded-xl px-3 py-2.5 flex items-center gap-3 text-left hover:bg-black/[0.04] dark:hover:bg-white/[0.06]">
            <Sparkles size={16} className="text-emerald-500 shrink-0" />
            <span><span className="block text-[12px] font-semibold">{copy.defaultWorkspace}</span><span className="block text-[10px] text-gray-400 mt-0.5">{copy.defaultWorkspaceDesc}</span></span>
          </button>
          {recentWorkspaces.length > 0 && (
            <div className="mt-1 pt-2 border-t border-black/[0.05] dark:border-white/[0.06]">
              <div className="px-3 pb-1 text-[10px] uppercase tracking-wider text-gray-400">{copy.recentDirectories}</div>
              {recentWorkspaces.map(path => (
                <button key={path} type="button" title={path}
                  onClick={() => select(path)}
                  className="w-full rounded-lg px-3 py-1.5 flex items-center gap-2 text-left hover:bg-black/[0.04] dark:hover:bg-white/[0.06]">
                  <FolderOpen size={13} className="shrink-0 text-gray-400" />
                  <span className="truncate text-[11px]">{workspaceName(path, copy.unknownDirectory)}</span>
                </button>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
