// Interpretation of ensure_folder_projects outcomes (§9.9 folder channel):
// shared by the picker's browse channel and both lanes' "recent directories"
// channels so the interpretation lives in one place instead of drifting apart.
// - created/covered: the picked folder owns an anchored project (created or
//   reused); extract its project id (created carries the project object,
//   covered a project_id). The session must be assigned to THAT project, or
//   tier-2 nested grouping adopts it into a broader project whose root
//   covers the folder (e.g. a Desktop-rooted project).
// - no outcome (a genuinely empty list): the folder hit the
//   never-materialize exclusion table (§3) — honestly treated as a plain
//   folder.
// - failed: the backend explicitly refused the root (nesting conflict etc.)
//   — NOT the exclusion table; the caller should tell the user instead of
//   silently continuing. IPC-level exceptions never reach this function
//   (callers catch).
// round-8 m14: exclusion-table interpretation belongs to a genuinely empty
// list only. created/covered without a project id leaves tier-1 assignment
// nothing to carry — passing materialized=true would silently skip the
// assignment and let tier-2 adopt; unknown status is the same. Both surface
// as failed and never masquerade as the exclusion table.
export function interpretFolderEnsureOutcomes(outcomes) {
  const list = Array.isArray(outcomes) ? outcomes : [];
  const hit = list.find(o => o && (o.status === 'created' || o.status === 'covered'));
  const projectId = hit
    ? (hit.status === 'created' ? (hit.project && hit.project.id) : hit.project_id) || null
    : null;
  const usable = !!hit && projectId !== null;
  const failed = list.length > 0
    && (!usable || list.some(o => o && o.status === 'failed'));
  return {
    materialized: usable,
    projectId: usable ? projectId : null,
    failed,
  };
}
