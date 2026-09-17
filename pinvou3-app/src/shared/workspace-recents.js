// Recent workspaces list: shared by code mode (CodexAcpView) and normal chat
// mode (ChatView's draft-state workspace selector). Extracted verbatim from
// CodexAcpView.jsx; the storage key is unchanged, so both modes share the
// same recents list.
//
// Note: platform/tauri/bridge/sessions.js is a classic script loaded via
// <script src> and cannot import this module; the remember logic inside
// pickDraftWorkspace is a verbatim mirror of this file (same key, same
// semantics), so changing either side must sync the other —
// tests/chat_draft_workspace_logic.test.mjs locks the bridge-side behavior.
export const RECENT_WORKSPACES_KEY = 'pinvou_codex_recent_workspaces';

export function workspaceName(path, unknownDirectory) {
  // eslint-disable-next-line sonarjs/super-linear-regex -- trailing [\\/]+ strips path separators; single char class, so backtracking is linear
  const normalized = String(path || '').replace(/[\\/]+$/, '');
  if (!normalized) return unknownDirectory;
  return normalized.split(/[\\/]/).filter(Boolean).pop() || normalized;
}

export function loadRecentWorkspaces() {
  try {
    const value = JSON.parse(localStorage.getItem(RECENT_WORKSPACES_KEY) || '[]');
    return Array.isArray(value) ? value.filter(path => typeof path === 'string').slice(0, 6) : [];
  } catch {
    return [];
  }
}

export function rememberWorkspace(path) {
  const next = [path, ...loadRecentWorkspaces().filter(item => item !== path)].slice(0, 6);
  localStorage.setItem(RECENT_WORKSPACES_KEY, JSON.stringify(next));
  return next;
}

export function forgetWorkspace(path) {
  const next = loadRecentWorkspaces().filter(item => item !== path);
  try {
    localStorage.setItem(RECENT_WORKSPACES_KEY, JSON.stringify(next));
  } catch {
    // When localStorage is unavailable, the current window may still keep
    // creating new sessions.
  }
  return next;
}
