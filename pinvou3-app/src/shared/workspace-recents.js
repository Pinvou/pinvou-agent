// Recent workspaces list, shared by the code mode (CodexAcpView) and plain
// chat mode (ChatView's draft-state workspace picker); the storage key is
// unchanged so both modes share the same recents list.
//
// Note: platform/tauri/bridge/sessions.js is a classic script loaded via
// <script src> and cannot import this module; the remember logic inside its
// pickDraftWorkspace is a verbatim mirror of this file (same key, same
// semantics). Any change to one side must be mirrored on the other —
// tests/chat_draft_workspace_logic.test.mjs locks the bridge-side behavior.
import { pathBasename } from './path-utils.js';

export const RECENT_WORKSPACES_KEY = 'pinvou_codex_recent_workspaces';

export function workspaceName(path, unknownDirectory) {
  // Trailing-separator stripping and Windows drive-letter semantics are
  // centralized in shared/path-utils (the regex version here and CodexAcpView's
  // pathBasename version had diverged).
  return pathBasename(path, { collapseTrailing: true, fallback: unknownDirectory });
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
  try {
    localStorage.setItem(RECENT_WORKSPACES_KEY, JSON.stringify(next));
  } catch {
    // When localStorage is unavailable (quota full / private mode), losing this
    // one recents entry is acceptable; an in-progress session creation must not
    // be interrupted (same protection as forgetWorkspace below).
  }
  return next;
}

export function forgetWorkspace(path) {
  const next = loadRecentWorkspaces().filter(item => item !== path);
  try {
    localStorage.setItem(RECENT_WORKSPACES_KEY, JSON.stringify(next));
  } catch {
    // When localStorage is unavailable, the current session can still be created.
  }
  return next;
}
