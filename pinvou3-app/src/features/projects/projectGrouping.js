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

// True when `path` equals `root` or lives directly under it.
// Windows-shaped paths (drive letter or UNC) fold case, unify separators and
// strip a trailing one, mirroring the store's filesystem_path_identity_key /
// key_is_same_or_nested (windows_path.rs, store.rs) — mixed-shape pairs like
// root `D:\work` vs path `D:/work/x` must still hit tier 2. The pure module
// has no host-OS signal, so it keys off path shape — drive-letter/UNC paths
// only ever come from Windows sessions. POSIX paths stay case-sensitive and
// keep the loose both-separator match for windows-stored paths.
function looksWindowsPath(value) {
  return /^[A-Za-z]:[\\/]/.test(value) || value.startsWith('\\\\');
}

function isUnderRoot(path, root) {
  if (!path || !root) return false;
  let a = String(path);
  let b = String(root);
  if (looksWindowsPath(a) && looksWindowsPath(b)) {
    const fold = (value) => {
      let v = value.toLowerCase().replaceAll('\\', '/');
      while (v.endsWith('/')) v = v.slice(0, -1);
      return v;
    };
    a = fold(a);
    b = fold(b);
    return a === b || a.startsWith(`${b}/`);
  }
  if (a === b) return true;
  return a.startsWith(`${b}/`) || a.startsWith(`${b}\\`);
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

// Auto-grouping ownership (design §9.9, 2026-09-11 ruling): root overlap
// across projects is legal, and a session whose workspace is covered by
// several projects is claimed by the one with the smallest position —
// sidebar order is the user-controllable knob, id breaks residual ties so
// the result never depends on input order. Matches the backend's
// resolve_session_project exactly.
function matchProjectByPath(projects, workspacePath) {
  const ordered = (Array.isArray(projects) ? [...projects] : [])
    .filter(Boolean)
    .sort((a, b) => (a.position || 0) - (b.position || 0) || String(a.id).localeCompare(String(b.id)));
  return ordered.find(project =>
    (project.roots ? project.roots : []).some((root) => {
      const rootPath = root && typeof root === 'object' ? root.path : root;
      return isUnderRoot(workspacePath, rootPath);
    })
  ) || null;
}

// Resolve the project a session currently belongs to for UI affordances
// (current-project marker in the move picker). Explicit assignment first,
// then auto-grouping; null for ungrouped sessions.
function resolveSessionProjectId(item, projects, assignments) {
  const projectList = Array.isArray(projects) ? projects.filter(Boolean) : [];
  const assignmentMap = assignments && typeof assignments === 'object' ? assignments : {};
  if (!item) return null;
  // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 is the floor and Object.hasOwn is unavailable; this call is already in safe form
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
// sessions that no materialized project ANCHORS yet AND that carry no
// assignment entry. Sessions with an entry are excluded both ways — null =
// explicit move-out (deleting a project writes null for its members, so a
// folder whose project was deleted only re-materializes when a NEW
// entry-less session appears), and an explicit Some(projectId) already
// filed the session elsewhere.
// Coverage is anchored (design §9.9, mirrors the backend ensure): only an
// origin=folder project whose roots contain this exact path counts as
// covering — a folder merely referenced by another project (even as its
// primary root) still materializes, overlap being legal. The backend
// re-checks under its own canonical keys, so a disagreement can only cause
// a harmless extra Created, never a duplicate anchor. Each entry reports
// the driving session ids so the caller can re-trigger per new session
// instead of per refresh.
function projectAnchorsFolder(project, folderPath) {
  if (!project || project.origin !== 'folder') return false;
  return (project.roots ? project.roots : []).some((root) => {
    const rootPath = root && typeof root === 'object' ? root.path : root;
    // 精确锚定:双向 isUnderRoot 即同路径(折叠大小写/分隔符差异)。
    return isUnderRoot(folderPath, rootPath) && isUnderRoot(rootPath, folderPath);
  });
}

function uncoveredWorkspaceRoots(items, projects, assignments) {
  const projectList = Array.isArray(projects) ? projects.filter(Boolean) : [];
  const assignmentMap = assignments && typeof assignments === 'object' ? assignments : {};
  const byRoot = new Map();
  (Array.isArray(items) ? items : []).forEach((item) => {
    if (!item || !hasProjectWorkspace(item)) return;
    const root = String(item.workspacePath || '');
    // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 is the floor and Object.hasOwn is unavailable; this call is already in safe form
    if (!root || Object.prototype.hasOwnProperty.call(assignmentMap, item.id)) return;
    if (!byRoot.has(root)) byRoot.set(root, []);
    byRoot.get(root).push(String(item.id));
  });
  return [...byRoot.entries()]
    .filter(([root]) => projectList.every(project => !projectAnchorsFolder(project, root)))
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
function groupSessionsByProject(items, projects, assignments) {
  const projectList = Array.isArray(projects) ? projects.filter(Boolean) : [];
  const assignmentMap = assignments && typeof assignments === 'object' ? assignments : {};
  const byId = new Map(projectList.map(project => [project.id, project]));
  const projectRows = new Map(projectList.map(project => [project.id, []]));
  const ungrouped = [];

  (Array.isArray(items) ? items : []).forEach((item) => {
    if (!item) return;
    // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 is the floor and Object.hasOwn is unavailable; this call is already in safe form
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

  const finalize = (key, kind, meta, rows) => {
    rows.sort((a, b) => itemTime(b).localeCompare(itemTime(a)));
    return { key, kind, ...meta, rows, latestAt: itemTime(rows[0]) };
  };

  const groups = [...projectList]
    .sort((a, b) => (a.position || 0) - (b.position || 0) || String(a.id).localeCompare(String(b.id)))
    .map(project => finalize(`project:${project.id}`, 'project', {
      projectId: project.id,
      name: project.name,
      roots: project.roots || [],
      path: '',
    }, projectRows.get(project.id) || []));
  if (ungrouped.length > 0) {
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
  projectAnchorsFolder,
  groupSessionsByFolder,
  groupSessionsByProject,
  projectCoversPath,
  resolveSessionProjectId,
  needsAddFolderConfirm,
  hasProjectWorkspace,
  uncoveredWorkspaceRoots,
};
