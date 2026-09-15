import assert from "node:assert/strict";
import test from "node:test";

import {
  TEMPORARY_GROUP_KEY,
  capUnavailableRootsForDisplay,
  groupSessionsWithProjects,
  needsAddFolderConfirm,
  projectCoversPath,
  resolveSessionProjectId,
} from "../src/features/projects/projectGrouping.js";

const projectItem = (id, path, updatedAt) => ({
  id,
  workspaceKind: "project",
  workspacePath: path,
  updatedAt,
});

const temporaryItem = (id, updatedAt) => ({
  id,
  workspaceKind: "temporary",
  workspacePath: `C:/Users/x/.pinvou3/tmp/${id}`,
  updatedAt,
});

const project = (id, name, roots, position) => ({
  id,
  name,
  roots: roots.map((path) => ({ path, available: true })),
  position,
});

test("without projects the behavior matches legacy folder grouping", () => {
  const groups = groupSessionsWithProjects(
    [
      projectItem("a1", "D:/work/alpha", "2026-08-01T08:00:00Z"),
      projectItem("b1", "D:/work/beta", "2026-08-02T08:00:00Z"),
      temporaryItem("t1", "2026-08-03T08:00:00Z"),
    ],
    [],
    {},
  );
  assert.deepEqual(groups.map((g) => [g.kind, g.key]), [
    ["folder", "D:/work/beta"],
    ["folder", "D:/work/alpha"],
    ["temporary", TEMPORARY_GROUP_KEY],
  ]);
});

test("tier 2 auto-groups sessions under a project root", () => {
  const projects = [project("p1", "Alpha", ["D:/work/alpha"], 0)];
  const groups = groupSessionsWithProjects(
    [
      projectItem("a1", "D:/work/alpha", "2026-08-01T08:00:00Z"),
      projectItem("a2", "D:/work/alpha/sub", "2026-08-02T08:00:00Z"),
      projectItem("b1", "D:/work/beta", "2026-08-03T08:00:00Z"),
    ],
    projects,
    {},
  );
  const alpha = groups.find((g) => g.kind === "project");
  assert.equal(alpha.projectId, "p1");
  assert.deepEqual(alpha.rows.map((r) => r.id).sort((a, b) => a.localeCompare(b)), ["a1", "a2"]);
  assert.equal(groups.find((g) => g.kind === "folder").key, "D:/work/beta");
});

test("tier 1 explicit assignment beats tier 2 auto-grouping", () => {
  const projects = [
    project("p1", "Alpha", ["D:/work/alpha"], 0),
    project("p2", "Beta", ["D:/work/beta"], 1),
  ];
  const groups = groupSessionsWithProjects(
    [projectItem("a1", "D:/work/alpha", "2026-08-01T08:00:00Z")],
    projects,
    { a1: "p2" },
  );
  const beta = groups.find((g) => g.projectId === "p2");
  assert.deepEqual(beta.rows.map((r) => r.id), ["a1"]);
  const alpha = groups.find((g) => g.projectId === "p1");
  assert.equal(alpha.rows.length, 0, "empty projects still render");
});

test("tier 1 explicit move-out blocks tier 2 auto revival", () => {
  const projects = [project("p1", "Alpha", ["D:/work/alpha"], 0)];
  const groups = groupSessionsWithProjects(
    [projectItem("a1", "D:/work/alpha", "2026-08-01T08:00:00Z")],
    projects,
    { a1: null },
  );
  assert.equal(groups.find((g) => g.projectId === "p1").rows.length, 0);
  assert.equal(groups.find((g) => g.kind === "folder").key, "D:/work/alpha");
});

test("stale assignment ids fall through to auto grouping", () => {
  const projects = [project("p1", "Alpha", ["D:/work/alpha"], 0)];
  const groups = groupSessionsWithProjects(
    [projectItem("a1", "D:/work/alpha", "2026-08-01T08:00:00Z")],
    projects,
    { a1: "prj-deleted" },
  );
  assert.equal(groups.find((g) => g.projectId === "p1").rows.length, 1);
});

test("longest matching root wins for nested roots", () => {
  const projects = [
    project("p1", "Work", ["D:/work"], 0),
    project("p2", "Deep", ["D:/work/deep"], 1),
  ];
  const groups = groupSessionsWithProjects(
    [
      projectItem("shallow", "D:/work/other", "2026-08-01T08:00:00Z"),
      projectItem("deep", "D:/work/deep/x", "2026-08-02T08:00:00Z"),
    ],
    projects,
    {},
  );
  assert.deepEqual(groups.find((g) => g.projectId === "p1").rows.map((r) => r.id), ["shallow"]);
  assert.deepEqual(groups.find((g) => g.projectId === "p2").rows.map((r) => r.id), ["deep"]);
});

test("project groups sort by manual position, folders by activity, temporary last", () => {
  const projects = [
    project("p2", "Second", ["D:/work/beta"], 1),
    project("p1", "First", ["D:/work/alpha"], 0),
  ];
  const groups = groupSessionsWithProjects(
    [
      projectItem("old", "D:/work/legacy", "2026-07-01T08:00:00Z"),
      projectItem("fresh", "D:/work/new", "2026-08-10T08:00:00Z"),
      temporaryItem("t1", "2026-08-19T08:00:00Z"),
      projectItem("a1", "D:/work/alpha", "2026-08-02T08:00:00Z"),
    ],
    projects,
    {},
  );
  assert.deepEqual(groups.map((g) => g.key), [
    "project:p1",
    "project:p2",
    "D:/work/new",
    "D:/work/legacy",
    TEMPORARY_GROUP_KEY,
  ]);
});

test("root string form is accepted alongside { path } objects", () => {
  const projects = [{ id: "p1", name: "Alpha", roots: ["D:/work/alpha"], position: 0 }];
  const groups = groupSessionsWithProjects(
    [projectItem("a1", "D:/work/alpha", "2026-08-01T08:00:00Z")],
    projects,
    {},
  );
  assert.equal(groups.find((g) => g.kind === "project").rows.length, 1);
});

test("temporary sessions never auto-group even under a project root", () => {
  const projects = [project("p1", "Alpha", ["C:/Users/x"], 0)];
  const groups = groupSessionsWithProjects(
    [temporaryItem("t1", "2026-08-01T08:00:00Z")],
    projects,
    {},
  );
  const temporary = groups.find((g) => g.kind === "temporary");
  assert.deepEqual(temporary.rows.map((r) => r.id), ["t1"]);
});

test("windows roots match case-insensitively, posix roots stay case-sensitive", () => {
  // The store folds identity keys on Windows (root_keys_fold_case_only_on_windows);
  // the display-side longest-root guard must not let a case-differing workspace
  // path slip past its project (review finding 19).
  const projects = [project("p1", "Alpha", ["D:/Work/Alpha"], 0)];
  const groups = groupSessionsWithProjects(
    [projectItem("a1", "d:/work/alpha/sub", "2026-08-01T08:00:00Z")],
    projects,
    {},
  );
  assert.deepEqual(groups.find((g) => g.projectId === "p1").rows.map((r) => r.id), ["a1"]);

  const posix = [project("p2", "Beta", ["/home/x/Beta"], 0)];
  const posixGroups = groupSessionsWithProjects(
    [projectItem("b1", "/home/x/beta/sub", "2026-08-01T08:00:00Z")],
    posix,
    {},
  );
  assert.equal(posixGroups.find((g) => g.projectId === "p2").rows.length, 0);
  assert.equal(posixGroups.find((g) => g.kind === "folder").key, "/home/x/beta/sub");
});

test("empty and invalid inputs degrade safely", () => {
  assert.deepEqual(groupSessionsWithProjects([], [], {}), []);
  assert.deepEqual(groupSessionsWithProjects(null, null, null), []);
  const groups = groupSessionsWithProjects([null, projectItem("a1", "D:/w", "x")], null, null);
  assert.equal(groups.length, 1);
});

// ── Cases ported from the legacy sidebar-grouping suite so the old module
// can be retired without losing coverage. ──────────────────────────────────

test("missing updatedAt does not crash and sorts as oldest", () => {
  const groups = groupSessionsWithProjects(
    [
      projectItem("no-time", "D:/work/alpha", ""),
      projectItem("timed", "D:/work/alpha", "2026-08-01T08:00:00Z"),
      { id: "null-item", workspaceKind: "project", workspacePath: "D:/work/alpha" },
      null,
    ],
    [],
    {},
  );
  assert.equal(groups.length, 1);
  assert.deepEqual(groups[0].rows.map((r) => r.id), ["timed", "no-time", "null-item"]);
});

test("project sessions without a workspace path fall into the temporary group", () => {
  const groups = groupSessionsWithProjects(
    [{ id: "no-path", workspaceKind: "project", workspacePath: "", updatedAt: "2026-08-01T08:00:00Z" }],
    [],
    {},
  );
  assert.equal(groups.length, 1);
  assert.equal(groups[0].key, TEMPORARY_GROUP_KEY);
});

// ── Move-picker helpers ────────────────────────────────────────────────────

test("resolveSessionProjectId mirrors the grouping tiers", () => {
  const projects = [project("p1", "Alpha", ["D:/work/alpha"], 0)];
  const item = projectItem("a1", "D:/work/alpha", "2026-08-01T08:00:00Z");
  assert.equal(resolveSessionProjectId(item, projects, {}), "p1");
  assert.equal(resolveSessionProjectId(item, projects, { a1: "p1" }), "p1");
  assert.equal(resolveSessionProjectId(item, projects, { a1: null }), null, "explicit move-out wins");
  assert.equal(resolveSessionProjectId(temporaryItem("t1", "x"), projects, { t1: "p1" }), "p1");
  assert.equal(resolveSessionProjectId(temporaryItem("t1", "x"), projects, {}), null, "temp never auto-groups");
  // The real fork the picker must survive: a stale id does not stick as
  // "ungrouped" — with a non-empty project list whose root still covers the
  // path, resolution falls through to tier 2 and returns that project.
  // (projects=[] would trivially yield null and pin nothing.)
  assert.equal(resolveSessionProjectId(item, projects, { a1: "prj-gone" }), "p1", "stale id falls through to root matching");
});

test("projectCoversPath gates the move-only confirm prompt", () => {
  const projects = [project("p1", "Alpha", ["D:/work/alpha"], 0)];
  assert.equal(projectCoversPath(projects[0], "D:/work/alpha"), true);
  assert.equal(projectCoversPath(projects[0], "D:/work/alpha/sub"), true);
  assert.equal(projectCoversPath(projects[0], "D:/work/beta"), false);
  assert.equal(projectCoversPath(null, "D:/work/alpha"), false);
});

test("rows sort by updatedAt descending within a group", () => {
  // Ported from the retired sidebar_grouping_logic suite: in-group ordering
  // with multiple timestamps must survive the project-layer rewrite.
  const projects = [project("p1", "Alpha", ["D:/work/alpha"], 0)];
  const groups = groupSessionsWithProjects(
    [
      projectItem("old", "D:/work/alpha", "2026-07-01T08:00:00Z"),
      projectItem("new", "D:/work/alpha", "2026-08-01T08:00:00Z"),
      projectItem("mid", "D:/work/alpha", "2026-07-15T08:00:00Z"),
    ],
    projects,
    {},
  );
  assert.deepEqual(groups[0].rows.map((r) => r.id), ["new", "mid", "old"]);
});

test("temporary group rows sort by recency while the group stays last", () => {
  const projects = [project("p1", "Alpha", ["D:/work/alpha"], 0)];
  const groups = groupSessionsWithProjects(
    [
      temporaryItem("t1", "2026-08-19T08:00:00Z"),
      projectItem("a1", "D:/work/alpha", "2026-08-02T08:00:00Z"),
      temporaryItem("t2", "2026-08-18T08:00:00Z"),
    ],
    projects,
    {},
  );
  assert.equal(groups[groups.length - 1].key, TEMPORARY_GROUP_KEY);
  assert.deepEqual(groups[groups.length - 1].rows.map((r) => r.id), ["t1", "t2"]);
});

test("projectCoversPath accepts raw string roots and empty paths", () => {
  // The project() helper normalizes roots to { path } objects like the bridge
  // sends them; hand-edited state can still carry raw strings, so the raw
  // branch must satisfy the same containment rule.
  const rawRoots = { id: "p1", name: "Alpha", roots: ["D:/work/alpha"] };
  assert.equal(projectCoversPath(rawRoots, "D:/work/alpha"), true);
  assert.equal(projectCoversPath(rawRoots, "D:/work/alpha/sub"), true);
  assert.equal(projectCoversPath(rawRoots, "D:/work/alpha-beta"), false);
  assert.equal(projectCoversPath(rawRoots, ""), false);
});

test("resolveSessionProjectId degrades safely on invalid inputs", () => {
  const projects = [project("p1", "Alpha", ["D:/work/alpha"], 0)];
  const item = projectItem("a1", "D:/work/alpha", "2026-08-01T08:00:00Z");
  assert.equal(resolveSessionProjectId(null, projects, {}), null);
  assert.equal(resolveSessionProjectId(undefined, projects, {}), null);
  // Non-object assignments must not throw and fall through to tier 2.
  assert.equal(resolveSessionProjectId(item, projects, "garbage"), "p1");
  assert.equal(resolveSessionProjectId(item, projects, null), "p1");
});

test("containment honors separator boundaries and mirrors the store rule", () => {
  // key_is_same_or_nested's reason to exist: a bare startsWith would file
  // D:/work/alpha-beta under D:/work/alpha. Pin that boundary on all three
  // public helpers (mutation guard for isUnderRoot).
  const projects = [project("p1", "Alpha", ["D:/work/alpha"], 0)];
  const sibling = projectItem("s1", "D:/work/alpha-beta", "2026-08-01T08:00:00Z");
  assert.equal(projectCoversPath(projects[0], "D:/work/alpha-beta"), false);
  assert.equal(resolveSessionProjectId(sibling, projects, {}), null);
  const groups = groupSessionsWithProjects([sibling], projects, {});
  // Empty project groups still render; the point is that the sibling lands
  // in its own folder bucket instead of the project's rows.
  assert.deepEqual(groups.find((g) => g.projectId === "p1")?.rows, []);

  // Mixed separators fold for Windows-shaped paths (store identity keys fold
  // separators and case); a trailing separator on the root still lets children
  // match while the exact path keeps comparing unequal before the strip, and
  // a bare separator root covers every absolute path on that side — all
  // mirroring key_is_same_or_nested.
  const backslash = { id: "p2", name: "Win", roots: ["D:\\work\\alpha"] };
  assert.equal(projectCoversPath(backslash, "D:/Work/Alpha"), true);
  assert.equal(projectCoversPath(backslash, "D:/Work/Alpha/deep"), true);
  const trailing = { id: "p3", name: "Trail", roots: ["D:/work/alpha/"] };
  assert.equal(projectCoversPath(trailing, "D:/work/alpha"), false);
  assert.equal(projectCoversPath(trailing, "D:/work/alpha/deep"), true);
  const posixRoot = { id: "p4", name: "Posix", roots: ["/"] };
  assert.equal(projectCoversPath(posixRoot, "/home/x/anything"), true);

  // The UNC branch of the shape heuristic: backslash-UNC root against a
  // case-differing and a separator-differing UNC path beneath it.
  const unc = { id: "p5", name: "Unc", roots: ["\\\\Server\\Share\\Alpha"] };
  assert.equal(projectCoversPath(unc, "\\\\server\\share\\alpha\\sub"), true);
  assert.equal(projectCoversPath(unc, "\\\\server/share/alpha/sub"), true);
});

test("needsAddFolderConfirm is the shared drop/pick decision", () => {
  const target = project("p1", "Alpha", ["D:/work/alpha"], 0);
  assert.equal(
    needsAddFolderConfirm(projectItem("a1", "D:/work/alpha", "x"), target),
    false,
    "covered workspace moves instantly",
  );
  assert.equal(
    needsAddFolderConfirm(projectItem("a1", "D:/work/other", "x"), target),
    true,
    "uncovered workspace confirms the add-folder step first",
  );
  assert.equal(
    needsAddFolderConfirm(temporaryItem("t1", "x"), target),
    false,
    "temporary sessions have no workspace and move instantly",
  );
  assert.equal(needsAddFolderConfirm(null, target), false, "missing session is a no-op");
  assert.equal(needsAddFolderConfirm(projectItem("a1", "x", "x"), null), false, "missing target is a no-op");
});

test("capUnavailableRootsForDisplay keeps one badge and folds the rest into +N", () => {
  const roots = ["/a/gone", "/b/gone", "/c/gone"];
  const collapsed = capUnavailableRootsForDisplay(roots, false);
  assert.deepEqual(collapsed, { visibleRoots: ["/a/gone"], hiddenCount: 2 });
  const expanded = capUnavailableRootsForDisplay(roots, true);
  assert.deepEqual(expanded, { visibleRoots: roots, hiddenCount: 0 });
  // 单根与空列表:不出现 +N,行为与裁剪前一致。
  assert.deepEqual(capUnavailableRootsForDisplay(["/a/gone"], false), { visibleRoots: ["/a/gone"], hiddenCount: 0 });
  assert.deepEqual(capUnavailableRootsForDisplay([], false), { visibleRoots: [], hiddenCount: 0 });
  // 非数组入参(后端缺席时的 undefined)按空列表处理。
  assert.deepEqual(capUnavailableRootsForDisplay(undefined, false), { visibleRoots: [], hiddenCount: 0 });
});
