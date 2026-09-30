// Fork-session dialog logic (docs/fork-session-plan.md §2.3/§3.1), split out
// as pure functions so the default toggle values, per-root overrides, and the
// copy-path previews stay unit-testable without rendering React (the
// session-management logic test pattern).
//
// Keychain semantics: `roots` is the session's §6 keychain snapshot in stored
// order — roots[0] is the primary folder (the creation-time cwd). The default
// plan isolates the primary root (the agent's write surface) and shares the
// attached roots (usually reference material); the user overrides per root.

// Normalize the sidecar's root list into the dialog's ordered root view:
// strings only, empties dropped. An empty result means the session is
// unbound — the dialog offers no isolation options for it.
export function forkDialogRoots(roots) {
  return (Array.isArray(roots) ? roots : []).filter(Boolean).map(String);
}

// Default per-root isolation toggles: primary isolated, attached shared.
export function initialIsolationByRoot(roots) {
  const byRoot = {};
  forkDialogRoots(roots).forEach((root, index) => {
    byRoot[root] = index === 0;
  });
  return byRoot;
}

// The roots the user chose to isolate, in keychain order (backend re-checks
// membership against the keychain; order only keeps the request readable).
export function selectedIsolateRoots(roots, byRoot) {
  return forkDialogRoots(roots).filter((root) => byRoot && byRoot[root]);
}

// Preview of a copy's location for the one-time creation notice. The exact
// directory name carries the NEW session id's first 4 chars, which only
// exists after the backend mints it — the preview pins the parent + prefix
// (`<name>-fork-`) and marks the unknown suffix, matching the backend's
// `<name>-fork-<id4>` convention without promising a path it cannot know.
export function forkCopyPathPreview(root) {
  if (!root) return '';
  const normalized = String(root).replaceAll('\\', '/');
  const cut = normalized.lastIndexOf('/');
  const name = cut === -1 ? normalized : normalized.slice(cut + 1);
  const parent = cut === -1 ? '' : normalized.slice(0, cut);
  const prefix = parent ? `${parent}/` : '';
  return `${prefix}${name || 'workspace'}-fork-xxxx`;
}

// Whether the fork button may submit: isolation mode requires at least one
// selected root (an isolation plan with zero roots is just "share all").
export function forkConfirmEnabled(mode, roots, byRoot) {
  if (mode !== 'isolate') return true;
  return selectedIsolateRoots(roots, byRoot).length > 0;
}
