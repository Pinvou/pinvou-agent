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

// HTML5 drag-and-drop MIME for the sidebar session drag (move onto a project
// group). The producer (RecentItem) and the consumer (ProjectGroupHeader) must
// agree on exactly this type or the gesture silently no-ops — no highlight, no
// preventDefault, no drop, and no console output — so the literal lives here
// once and both sides import it.
const PROJECT_SESSION_DRAG_TYPE = 'application/x-pinvou-session';

function itemTime(item) {
  return String((item && (item.updatedAt || item.pinnedAt)) || '');
}

// True when `path` equals `root` or lives directly under it. Both separators
// are accepted so canonicalized unix roots still match windows-stored paths.
// Windows paths fold case for comparison, mirroring the store's
// filesystem_path_identity_key (Windows identity keys fold case, POSIX does
// not): the pure module has no host-OS signal, so it keys off path shape —
// drive-letter/UNC paths only ever come from Windows sessions.
// Leading-// is deliberately NOT claimed as Windows UNC shape: POSIX leaves
// that prefix implementation-defined, so //tmp/x can be a genuine POSIX path
// on a case-sensitive volume. Store-produced roots are always backslash-form,
// so a forward-slash UNC session path (externally written only) compares
// POSIX-exact here while the store would fold it on Windows hosts — a known,
// pinned divergence (see the grouping test), not an oversight.
function looksWindowsPath(value) {
  return /^[A-Za-z]:[\\/]/.test(value) || value.startsWith('\\\\');
}

// Component-aware "same or nested", mirroring the store's key_is_same_or_nested
// (features/projects/store.rs) line for line — including comparing full
// equality before stripping the root's trailing separator — so the display
// side and the store can never disagree about which sessions belong where. A
// bare startsWith would file /a/bc under /a/b: the character right past the
// base must be a separator, and a bare-separator root ("/") covers every
// absolute path, matching the store's empty-base rule. Windows-shaped paths
// fold separators and case on both sides, mirroring filesystem_path_identity_key
// (Windows identity keys fold case and separators, POSIX does not): the pure
// module has no host-OS signal, so it keys off path shape — drive-letter/UNC
// paths only ever come from Windows sessions.
function isUnderRoot(path, root) {
  if (!path || !root) return false;
  let a = String(path);
  let b = String(root);
  const windowsShape = looksWindowsPath(a) && looksWindowsPath(b);
  if (windowsShape) {
    a = a.toLowerCase().replaceAll('\\', '/');
    b = b.toLowerCase().replaceAll('\\', '/');
  }
  if (a === b) return true;
  b = windowsShape ? b.replace(/[\\/]$/, '') : b.replace(/\/$/, '');
  if (!b) return a.startsWith('/');
  return a.startsWith(b) && a[b.length] === '/';
}

// Roots arrive either as raw strings (hand-edited state, tests) or as the
// bridge's { path, available } objects — one accessor for both shapes.
function rootPath(root) {
  return root && typeof root === 'object' ? root.path : root;
}

// Longest root wins so nested project roots cannot steal sessions from a
// deeper project (backend also rejects cross-project nesting, this is the
// display-side guard for hand-edited state).
function matchProjectByPath(projects, workspacePath) {
  let best = null;
  let bestRoot = '';
  projects.forEach((project) => {
    (project && project.roots ? project.roots : []).forEach((root) => {
      if (isUnderRoot(workspacePath, rootPath(root)) && String(rootPath(root)).length > bestRoot.length) {
        best = project;
        bestRoot = String(rootPath(root));
      }
    });
  });
  return best;
}

// Resolve the project a session currently belongs to for UI affordances
// (current-project marker in the move picker, "remove from project" entry).
// Mirrors the grouping tiers: explicit assignment first, then auto-grouping;
// returns null for ungrouped sessions.
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

// True when any of the project roots covers `path` (same containment rule as
// tier 2; the move picker uses it to gate the move-only confirm panel for a
// target whose roots do not already cover the session's workspace).
function projectCoversPath(project, path) {
  if (!project || !path) return false;
  return (project.roots || []).some((root) => isUnderRoot(String(path), rootPath(root)));
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
    if (!target && !autoGroupBlocked && hasProjectWorkspace(item)) {
      target = matchProjectByPath(projectList, item.workspacePath);
    }
    if (target) {
      projectRows.get(target.id).push(item);
      return;
    }
    // Tier 3: legacy folder bucketing.
    const key = hasProjectWorkspace(item) && item.workspacePath
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

// Shared drop/pick decision: does moving `session` onto project `target` need
// the add-folder confirmation first (target's roots do not cover the session's
// workspace), or can it move instantly? Temporary sessions have no workspace
// and always move instantly. One predicate instead of three hand-mirrored
// copies (drag drop handler, dialog initializer, dialog choose).
function needsAddFolderConfirm(session, target) {
  if (!session || !target) return false;
  const workspacePath = hasProjectWorkspace(session) ? String(session.workspacePath || '') : '';
  return !!workspacePath && !projectCoversPath(target, workspacePath);
}

// 失效 root 徽标的展示裁剪(评审 #463 m3):头部行固定 28px,一个完整徽标
// (Folder unavailable · Rebind)已接近上限,多个 shrink-0 徽标会把折叠按钮
// 挤到零宽并横向溢出。折叠时只保留第一个徽标(首入口),其余计数进 +N;
// expanded 为 true 时全部平铺(容器换行)。返回 { visibleRoots, hiddenCount }。
function capUnavailableRootsForDisplay(roots, expanded) {
  const list = Array.isArray(roots) ? roots : [];
  if (expanded) return { visibleRoots: list, hiddenCount: 0 };
  return { visibleRoots: list.slice(0, 1), hiddenCount: Math.max(0, list.length - 1) };
}

export { TEMPORARY_GROUP_KEY, PROJECT_SESSION_DRAG_TYPE, groupSessionsWithProjects, projectCoversPath, resolveSessionProjectId, rootPath, needsAddFolderConfirm, hasProjectWorkspace, capUnavailableRootsForDisplay };
