// Pure logic for the manage-folders panel (§4): roots row shape, removal
// decisions, primary-root resolution. No UI/i18n dependencies; node-side unit
// testable.

import { isUnderRoot } from './projectGrouping.js';

function rootPathOf(root) {
  return String((root && typeof root === 'object' ? root.path : root) || '');
}

// Panel row: path + availability badge + primary marker. Primary =
// last_primary_root (adopted only while still a roots member), otherwise the
// first roots entry (same criterion as pickerPrimaryRoot).
export function manageFolderRows(project) {
  const roots = (project && Array.isArray(project.roots) ? project.roots : [])
    .map(root => ({
      path: rootPathOf(root),
      available: !!(root && typeof root === 'object' ? root.available : true),
    }))
    .filter(row => row.path);
  if (!roots.length) return [];
  const remembered = project && project.last_primary_root ? String(project.last_primary_root) : '';
  const primaryIndex = remembered && roots.some(row => row.path === remembered)
    ? roots.findIndex(row => row.path === remembered)
    : 0;
  return roots.map((row, index) => ({ ...row, isPrimary: index === primaryIndex }));
}

// Removal decision (§4/§9.5): removing the primary root while other roots
// remain requires picking a new primary first (a downgrade hint; never pick
// for the user); removing the only root → the project degrades to tag-only
// (empty roots), allowed; non-primary roots are removable directly. A
// duplicate/nonexistent path returns removed=false.
export function removeRootPlan(project, path) {
  const rows = manageFolderRows(project);
  const target = rows.find(row => row.path === path);
  if (!target) return { removed: false, roots: rows.map(row => row.path), needsNewPrimary: false, becomesTagOnly: false };
  const remaining = rows.filter(row => row.path !== path);
  return {
    removed: true,
    roots: remaining.map(row => row.path),
    needsNewPrimary: target.isPrimary && remaining.length > 0,
    becomesTagOnly: remaining.length === 0,
  };
}

// Add duplicate check: already covered (exact same path) needs no add; only
// exact duplicates are blocked (adding the same path twice would trip
// update_project's in-group dedup validation, so block it early).
export function rootAlreadyPresent(project, path) {
  const target = String(path || '');
  if (!target) return false;
  return manageFolderRows(project).some(row => row.path === target);
}

// Intra-set nesting guard: §9.9 legalized cross-project overlap only — within
// one project's root set the store still rejects nesting/containment
// (validate_roots: "project roots must not nest"). The panel checks before
// invoking so picking a child of an existing root in the system folder picker
// surfaces a specific message instead of the backend's generic failure.
export function rootConflictsWithExisting(project, path) {
  const target = String(path || '');
  if (!target) return false;
  return manageFolderRows(project).some(
    row => isUnderRoot(target, row.path) || isUnderRoot(row.path, target),
  );
}
