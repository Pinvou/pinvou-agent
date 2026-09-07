// Project layer grouping: pure logic that resolves each code session into a
// sidebar group. Like sidebar-grouping.js, this is a pure-function module with
// no UI/i18n dependencies, so it can be unit-tested on the node side.
//
// Deterministic three-tier resolution (see features/projects/mod.rs for the
// invariant list):
//   1. explicit assignment wins — assignments[sessionId] is a project id
//      (or null = explicit move-out, which skips tier 2 on purpose);
//   2. workspace path under a project root -> auto-group into that project
//      (longest matching root wins);
//   3. implicit folder group by workspace path; temporary sessions merge into
//      one bottom group (unchanged legacy behavior).
// Projects sort by manual position, implicit folders follow by latest
// activity, the temporary group always sinks last. Projects with zero members
// still render: they are explicit entities, not derived views.

const TEMPORARY_GROUP_KEY = '__temporary__';

function itemTime(item) {
  return String((item && (item.updatedAt || item.pinnedAt)) || '');
}

// True when `path` equals `root` or lives directly under it. Both separators
// are accepted so canonicalized unix roots still match windows-stored paths.
function isUnderRoot(path, root) {
  if (!path || !root) return false;
  if (path === root) return true;
  return path.startsWith(`${root}/`) || path.startsWith(`${root}\\`);
}

// Longest root wins so nested project roots cannot steal sessions from a
// deeper project (backend also rejects cross-project nesting, this is the
// display-side guard for hand-edited state).
function matchProjectByPath(projects, workspacePath) {
  let best = null;
  let bestRoot = '';
  projects.forEach((project) => {
    (project && project.roots ? project.roots : []).forEach((root) => {
      const rootPath = root && typeof root === 'object' ? root.path : root;
      if (isUnderRoot(workspacePath, rootPath) && String(rootPath).length > bestRoot.length) {
        best = project;
        bestRoot = String(rootPath);
      }
    });
  });
  return best;
}

// Input: items = code sessions [{ id, workspacePath, workspaceKind, updatedAt, ... }],
// projects = [{ id, name, roots: [path | { path }], position }],
// assignments = { [sessionId]: projectId | null }.
// Returns [{ key, kind: 'project' | 'folder' | 'temporary', projectId, name,
//            path, rows, latestAt }].
function groupSessionsWithProjects(items, projects, assignments) {
  const projectList = Array.isArray(projects) ? projects.filter(Boolean) : [];
  const assignmentMap = assignments && typeof assignments === 'object' ? assignments : {};
  const byId = new Map(projectList.map(project => [project.id, project]));

  const projectRows = new Map(projectList.map(project => [project.id, []]));
  const byFolder = new Map();

  (Array.isArray(items) ? items : []).forEach((item) => {
    if (!item) return;
    let target = null;
    let autoGroupBlocked = false;
    if (Object.prototype.hasOwnProperty.call(assignmentMap, item.id)) {
      const assigned = assignmentMap[item.id];
      if (assigned && byId.has(assigned)) {
        // Tier 1: explicit id resolves.
        target = byId.get(assigned);
      } else if (assigned === null) {
        // Explicit move-out: "not in any project" must NOT auto-revive via
        // tier 2. Stale ids (deleted project, hand-edited state) still fall
        // through to auto grouping.
        autoGroupBlocked = true;
      }
    }
    // Tier 2: auto-group by workspace root containment. Only project-kind
    // sessions participate — temporary sessions enter a project exclusively
    // through explicit assignment (the "adopt" flow), never implicitly.
    if (!target && !autoGroupBlocked && item.workspaceKind === 'project') {
      target = matchProjectByPath(projectList, item.workspacePath);
    }
    if (target) {
      projectRows.get(target.id).push(item);
      return;
    }
    // Tier 3: legacy folder bucketing.
    const key = item.workspaceKind === 'project' && item.workspacePath
      ? String(item.workspacePath)
      : TEMPORARY_GROUP_KEY;
    if (!byFolder.has(key)) byFolder.set(key, []);
    byFolder.get(key).push(item);
  });

  const finalize = (key, meta, rows) => {
    rows.sort((a, b) => itemTime(b).localeCompare(itemTime(a)));
    return { key, ...meta, rows, latestAt: itemTime(rows[0]) };
  };

  const groups = [];
  // Projects keep manual position order and render even when empty.
  [...projectList]
    .sort((a, b) => (a.position || 0) - (b.position || 0) || String(a.id).localeCompare(String(b.id)))
    .forEach((project) => {
      groups.push(finalize(`project:${project.id}`, {
        kind: 'project',
        projectId: project.id,
        name: project.name,
        roots: project.roots || [],
        path: '',
      }, projectRows.get(project.id) || []));
    });
  const folderGroups = [];
  byFolder.forEach((rows, key) => {
    if (key === TEMPORARY_GROUP_KEY) return;
    folderGroups.push(finalize(key, { kind: 'folder', projectId: null, name: '', path: key }, rows));
  });
  folderGroups.sort((a, b) => b.latestAt.localeCompare(a.latestAt));
  groups.push(...folderGroups);
  if (byFolder.has(TEMPORARY_GROUP_KEY)) {
    groups.push(finalize(TEMPORARY_GROUP_KEY, {
      kind: 'temporary',
      projectId: null,
      name: '',
      path: '',
    }, byFolder.get(TEMPORARY_GROUP_KEY)));
  }
  return groups;
}

export { TEMPORARY_GROUP_KEY, groupSessionsWithProjects };
