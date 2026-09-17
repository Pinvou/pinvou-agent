// Pure logic for the unified "choose workspace" picker (design
// §2/§3/§9.3/§9.4): hot-view computation, primary-root resolution, mode-aware
// notices. No UI/i18n dependencies; node-side unit testable.

import { resolveSessionProjectId } from './projectGrouping.js';

// Cold-project threshold (§3 accumulation mitigation): a materialized
// origin=folder project with 30 days of no activity and no explicit members
// is hidden from the picker (hot view); the sidebar's full view is unaffected
// and nothing is deleted.
export const COLD_PROJECT_IDLE_MS = 30 * 24 * 60 * 60 * 1000;

function itemTime(item) {
  return String((item && (item.updatedAt || item.pinnedAt)) || '');
}

// A project's last activity time: the latest updatedAt of member sessions
// (explicit assignment + tier-2 auto-grouping), falling back to the project's
// own updated_at (materialization/edit time) when it has no members.
// Membership must be resolved against the FULL project list: with only this
// project passed, a session explicitly filed into project B but physically
// under A's root would still resolve to A (tier-2 path match) and keep A
// warm — the membership criterion must match the sidebar grouping exactly.
export function projectLastActivity(project, items, assignments, projects) {
  const projectList = Array.isArray(projects) ? projects.filter(Boolean) : [project];
  let latest = String((project && project.updated_at) || '');
  (Array.isArray(items) ? items : []).forEach((item) => {
    if (!item) return;
    if (resolveSessionProjectId(item, projectList, assignments) !== project.id) return;
    const time = itemTime(item);
    if (time > latest) latest = time;
  });
  return latest;
}

// Picker rows (hot view): sorted by most recent use descending; cold
// projects (materialized projects idle for a long time or whose members all
// moved out) are hidden. `now` is injected for testability.
// Rootless (tag-only) projects never appear: there is no root to bind, so
// listing them would inflate the search threshold and render blank rows
// (the row renderer cannot offer a disabled state for them).
export function computePickerRows({ projects, items, assignments, now }) {
  const nowMs = typeof now === 'number' ? now : Date.now();
  const assignmentMap = assignments && typeof assignments === 'object' ? assignments : {};
  const projectList = (Array.isArray(projects) ? projects : []).filter(Boolean);
  return projectList
    .filter(project => pickerProjectRoots(project).length > 0)
    .map((project) => {
      const hasExplicitMembers = Object.values(assignmentMap).includes(project.id);
      return {
        project,
        hasExplicitMembers,
        lastActivity: projectLastActivity(project, items, assignmentMap, projectList),
      };
    })
    .filter((row) => {
      if (row.project.origin !== 'folder') return true;
      if (row.hasExplicitMembers) return true;
      const idleMs = nowMs - Date.parse(row.lastActivity || '');
      // Unparseable timestamps are treated as active (rather show than wrongly hide).
      return Number.isNaN(idleMs) || idleMs <= COLD_PROJECT_IDLE_MS;
    })
    .sort((a, b) => b.lastActivity.localeCompare(a.lastActivity)
      || String(a.project.id).localeCompare(String(b.project.id)));
}

// A project's default primary root (§9.3 project channel): the remembered
// last_primary_root wins (only while it is still a roots member, so a removed
// root cannot linger); otherwise the first roots entry. Returns null for
// tag-only projects (no roots) — callers must not create bound sessions with
// it.
export function pickerPrimaryRoot(project) {
  const roots = (project && Array.isArray(project.roots) ? project.roots : [])
    .map(root => (root && typeof root === 'object' ? root.path : root))
    .filter(Boolean)
    .map(String);
  if (!roots.length) return null;
  const remembered = project && project.last_primary_root ? String(project.last_primary_root) : '';
  if (remembered && roots.includes(remembered)) return remembered;
  return roots[0];
}

// All of a project's roots (display-shape array, order = storage order).
export function pickerProjectRoots(project) {
  return (project && Array.isArray(project.roots) ? project.roots : [])
    .map(root => (root && typeof root === 'object' ? root.path : root))
    .filter(Boolean)
    .map(String);
}

// Mode-aware permission notice (§9.4): restricted modes (Plan/read-only) =
// grant semantics ("will be able to access N folders"); YOLO/full-access =
// visibility semantics ("the model will know these N folders belong to this
// conversation" — YOLO is not workspace-restricted anyway). An unknown mode
// falls back to the restricted wording (the heavier notice is safer).
export function workspaceNoticeTone(mode) {
  return mode === 'yolo' ? 'visibility' : 'restricted';
}

// ── Session keychain chip (§6) ────────────────────────────────────────────
// roots[0] = the primary root (the creation-time cwd; a snapshot never
// re-labels it), the rest are additional roots. Empty/single root = status
// quo (the chip shows only the primary directory name, no "+N").
export function describeKeychain(roots) {
  const list = (Array.isArray(roots) ? roots : []).map(String).filter(Boolean);
  return {
    primary: list[0] || null,
    additional: Math.max(0, list.length - 1),
    roots: list,
  };
}
