// Sidebar grouping for workspace-bound sessions, split by dimension:
// - folder view (physical layer): group by workspace directory — the original
//   behavior, byte-identical to the pre-project sidebar; projects never
//   affect it;
// - project view (logical layer): only named projects as groups (explicit
//   assignment + root auto-match), plus a trailing ungrouped bucket that
//   collects everything not claimed by a project (drag source for moving in,
//   and the explicit move-out landing place).
// Pure-function module with no UI/i18n dependencies (node-side testable).

const TEMPORARY_GROUP_KEY = '__temporary__';
const UNGROUPED_GROUP_KEY = '__ungrouped__';

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

// 携带真实项目工作目录的会话形态:'project'(代码/ACP)与 'bound'(#445
// 绑定的普通工作会话)。'bound' 独立成 kind,不伪装成 'project'——将来
// project-kind 获得自有行为(如 baseline 面板)时不会误伤普通绑定会话
// (评审 #452 finding 5)。
const WORKSPACE_KINDS_WITH_PROJECT_DIR = ['project', 'bound'];

function hasProjectWorkspace(item) {
  return (
    !!item
    && WORKSPACE_KINDS_WITH_PROJECT_DIR.includes(item.workspaceKind)
    && !!item.workspacePath
  );
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

// Resolve the project a session currently belongs to for UI affordances
// (current-project marker in the move picker). Explicit assignment first,
// then auto-grouping; null for ungrouped sessions.
function resolveSessionProjectId(item, projects, assignments) {
  const projectList = Array.isArray(projects) ? projects.filter(Boolean) : [];
  const assignmentMap = assignments && typeof assignments === 'object' ? assignments : {};
  if (!item) return null;
  if (Object.prototype.hasOwnProperty.call(assignmentMap, item.id)) {
    const assigned = assignmentMap[item.id];
    if (assigned && projectList.some(project => project.id === assigned)) return assigned;
    if (assigned === null) return null;
  }
  if (!hasProjectWorkspace(item)) return null;
  const matched = matchProjectByPath(projectList, item.workspacePath);
  return matched ? matched.id : null;
}

// True when any of the project roots covers `path` (the move picker uses it
// to decide whether to offer adding the session's folder to the target).
function projectCoversPath(project, path) {
  if (!project || !path) return false;
  return (project.roots || []).some((root) => {
    const rootPath = root && typeof root === 'object' ? root.path : root;
    return isUnderRoot(String(path), rootPath ? String(rootPath) : rootPath);
  });
}

// Does moving `session` into `target` need the add-folder confirmation first
// (target roots don't cover the session's workspace), or can it move instantly?
// Temporary sessions have no workspace and always move instantly. One
// predicate instead of hand-mirrored copies (drag drop handler, dialog
// initializer, dialog choose) — 评审 #452 finding 6 同源化。
function needsAddFolderConfirm(session, target) {
  if (!session || !target) return false;
  const workspacePath = hasProjectWorkspace(session) ? String(session.workspacePath || '') : '';
  return !!workspacePath && !projectCoversPath(target, workspacePath);
}

// Distinct workspace folders driving auto-materialization: folders backing
// sessions that no project root covers yet AND that carry no assignment entry.
// Sessions with an entry are excluded both ways — null = explicit move-out
// (deleting a project writes null for its members, so a folder whose project
// was deleted only re-materializes when a NEW entry-less session appears),
// and an explicit Some(projectId) already filed the session elsewhere.
// Coverage mirrors the display-side grouping guard (matchProjectByPath:
// equal-or-ancestor project root wins); the backend re-checks under its own
// canonical keys, so a disagreement can only cause a harmless Covered
// outcome, never a duplicate project. Each entry reports the driving session
// ids so the caller can re-trigger per new session instead of per refresh.
function uncoveredWorkspaceRoots(items, projects, assignments) {
  const projectList = Array.isArray(projects) ? projects.filter(Boolean) : [];
  const assignmentMap = assignments && typeof assignments === 'object' ? assignments : {};
  const byRoot = new Map();
  (Array.isArray(items) ? items : []).forEach((item) => {
    if (!item || !hasProjectWorkspace(item)) return;
    const root = String(item.workspacePath || '');
    if (!root || Object.prototype.hasOwnProperty.call(assignmentMap, item.id)) return;
    if (!byRoot.has(root)) byRoot.set(root, []);
    byRoot.get(root).push(String(item.id));
  });
  return [...byRoot.entries()]
    .filter(([root]) => !matchProjectByPath(projectList, root))
    .map(([root, sessionIds]) => ({ root, sessionIds }));
}

// ── Folder view (physical layer) ───────────────────────────────────────────
// Original folder grouping restored: workspace-bound sessions bucket by
// directory, temporary sessions merge into one bottom group; rows sort by
// latest activity descending, groups sort by their latest activity descending.
function groupSessionsByFolder(items) {
  const byFolder = new Map();
  (Array.isArray(items) ? items : []).forEach((item) => {
    if (!item) return;
    const key = hasProjectWorkspace(item)
      ? String(item.workspacePath)
      : TEMPORARY_GROUP_KEY;
    if (!byFolder.has(key)) byFolder.set(key, []);
    byFolder.get(key).push(item);
  });
  const groups = [];
  byFolder.forEach((rows, key) => {
    rows.sort((a, b) => itemTime(b).localeCompare(itemTime(a)));
    groups.push({
      key,
      kind: key === TEMPORARY_GROUP_KEY ? 'temporary' : 'folder',
      projectId: null,
      name: '',
      roots: [],
      path: key === TEMPORARY_GROUP_KEY ? '' : key,
      rows,
      latestAt: itemTime(rows[0]),
    });
  });
  groups.sort((a, b) => {
    if (a.key === TEMPORARY_GROUP_KEY) return 1;
    if (b.key === TEMPORARY_GROUP_KEY) return -1;
    return b.latestAt.localeCompare(a.latestAt);
  });
  return groups;
}

// ── Project view (logical layer) ───────────────────────────────────────────
// Membership: explicit assignment (tier 1; null = explicit move-out lands in
// ungrouped and must not auto-revive) then root auto-match (tier 2, longest
// root wins). Temporary sessions never auto-join — they enter a project only
// via explicit assignment (the adopt flow). Projects render in manual
// position order even when empty; the ungrouped bucket always sinks last and
// is the drag source / move-out landing place.
// Within a group: sessions listed in the project's manual order render first
// in that order, the rest follow by latest activity descending.
function groupSessionsByProject(items, projects, assignments, orders) {
  const projectList = Array.isArray(projects) ? projects.filter(Boolean) : [];
  const assignmentMap = assignments && typeof assignments === 'object' ? assignments : {};
  const orderMap = orders && typeof orders === 'object' ? orders : {};
  const byId = new Map(projectList.map(project => [project.id, project]));
  const projectRows = new Map(projectList.map(project => [project.id, []]));
  const ungrouped = [];

  (Array.isArray(items) ? items : []).forEach((item) => {
    if (!item) return;
    if (Object.prototype.hasOwnProperty.call(assignmentMap, item.id)) {
      const assigned = assignmentMap[item.id];
      if (assigned && byId.has(assigned)) {
        projectRows.get(assigned).push(item);
        return;
      }
      if (assigned === null) {
        // 显式移出:直接进未分组,不得经 tier 2 复活;陈旧 id(项目已删/
        // 手改状态)继续走自动归组。
        ungrouped.push(item);
        return;
      }
    }
    if (hasProjectWorkspace(item)) {
      const target = matchProjectByPath(projectList, item.workspacePath);
      if (target) {
        projectRows.get(target.id).push(item);
        return;
      }
    }
    ungrouped.push(item);
  });

  // rows 由调用方排好(项目组走手动序+活动序,未分组走活动序),这里不再排序。
  const finalize = (key, kind, meta, rows) => ({ key, kind, ...meta, rows, latestAt: itemTime(rows[0]) });

  const orderedRows = (projectId, rows) => {
    const manual = Array.isArray(orderMap[projectId]) ? orderMap[projectId] : [];
    const bySession = new Map(rows.map(row => [row.id, row]));
    const listed = [];
    manual.forEach((sessionId) => {
      const row = bySession.get(sessionId);
      if (row) { listed.push(row); bySession.delete(sessionId); }
    });
    const rest = [...bySession.values()].sort((a, b) => itemTime(b).localeCompare(itemTime(a)));
    return [...listed, ...rest];
  };

  const groups = [...projectList]
    .sort((a, b) => (a.position || 0) - (b.position || 0) || String(a.id).localeCompare(String(b.id)))
    .map(project => finalize(`project:${project.id}`, 'project', {
      projectId: project.id,
      name: project.name,
      roots: project.roots || [],
      path: '',
    }, orderedRows(project.id, projectRows.get(project.id) || [])));
  if (ungrouped.length > 0) {
    ungrouped.sort((a, b) => itemTime(b).localeCompare(itemTime(a)));
    groups.push(finalize(UNGROUPED_GROUP_KEY, 'ungrouped', {
      projectId: null,
      name: '',
      roots: [],
      path: '',
    }, ungrouped));
  }
  return groups;
}

export {
  TEMPORARY_GROUP_KEY,
  UNGROUPED_GROUP_KEY,
  groupSessionsByFolder,
  groupSessionsByProject,
  projectCoversPath,
  resolveSessionProjectId,
  needsAddFolderConfirm,
  hasProjectWorkspace,
  uncoveredWorkspaceRoots,
};
