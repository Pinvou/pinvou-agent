import assert from "node:assert/strict";
import test from "node:test";

import {
  TEMPORARY_GROUP_KEY,
  UNGROUPED_GROUP_KEY,
  groupSessionsByFolder,
  groupSessionsByProject,
  needsAddFolderConfirm,
  projectCoversPath,
  resolveSessionProjectId,
  uncoveredWorkspaceRoots,
} from "../src/features/projects/projectGrouping.js";

const projectItem = (id, path, updatedAt) => ({
  id,
  workspaceKind: "project",
  workspacePath: path,
  updatedAt,
});

const boundItem = (id, path, updatedAt) => ({
  id,
  // #445 绑定的普通工作会话:独立 'bound' 形态,与 'project' 同为携带
  // 真实目录的会话(评审 #452 finding 5)。
  workspaceKind: "bound",
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

// ── 目录视图(纯物理层) ─────────────────────────────────────────────────────

test("folder view buckets by workspace path, activity-sorted, temporary last", () => {
  const groups = groupSessionsByFolder([
    projectItem("a1", "D:/work/alpha", "2026-08-01T08:00:00Z"),
    projectItem("b1", "D:/work/beta", "2026-08-02T08:00:00Z"),
    projectItem("a2", "D:/work/alpha", "2026-08-03T08:00:00Z"),
    temporaryItem("t1", "2026-08-19T08:00:00Z"),
    boundItem("w1", "D:/work/alpha", "2026-08-05T08:00:00Z"),
  ]);
  assert.deepEqual(groups.map((g) => [g.kind, g.key]), [
    ["folder", "D:/work/alpha"],
    ["folder", "D:/work/beta"],
    ["temporary", TEMPORARY_GROUP_KEY],
  ]);
  const alpha = groups[0];
  assert.deepEqual(alpha.rows.map((r) => r.id), ["w1", "a2", "a1"]);
  assert.equal(alpha.projectId, null, "目录组与项目无关");
});

test("folder view ignores projects and assignments entirely", () => {
  const projects = [project("p1", "Alpha", ["D:/work/alpha"], 0)];
  const groups = groupSessionsByFolder(
    [projectItem("a1", "D:/work/alpha", "2026-08-01T08:00:00Z")],
    projects,
    { a1: "p1" },
  );
  assert.equal(groups.length, 1);
  assert.equal(groups[0].kind, "folder");
});

test("folder view degrades safely on empty or invalid input", () => {
  assert.deepEqual(groupSessionsByFolder([]), []);
  assert.deepEqual(groupSessionsByFolder(null), []);
  const groups = groupSessionsByFolder([
    null,
    { id: "no-path", workspaceKind: "project", workspacePath: "" },
  ]);
  assert.deepEqual(groups.map((g) => g.key), [TEMPORARY_GROUP_KEY]);
});

// ── 项目视图(纯逻辑层 + 未分组桶) ──────────────────────────────────────────

test("project view auto-groups by root and renders empty projects in position order", () => {
  const projects = [
    project("p2", "Second", ["D:/work/beta"], 1),
    project("p1", "First", ["D:/work/alpha"], 0),
  ];
  const groups = groupSessionsByProject(
    [
      projectItem("a1", "D:/work/alpha", "2026-08-01T08:00:00Z"),
      projectItem("b1", "D:/work/beta/sub", "2026-08-02T08:00:00Z"),
      projectItem("u1", "D:/work/other", "2026-08-03T08:00:00Z"),
    ],
    projects,
    {},
  );
  assert.deepEqual(groups.map((g) => [g.kind, g.key]), [
    ["project", "project:p1"],
    ["project", "project:p2"],
    ["ungrouped", UNGROUPED_GROUP_KEY],
  ]);
  assert.deepEqual(groups[0].rows.map((r) => r.id), ["a1"]);
  assert.deepEqual(groups[1].rows.map((r) => r.id), ["b1"]);
  assert.deepEqual(groups[2].rows.map((r) => r.id), ["u1"]);
});

test("project view: explicit assignment beats root auto-grouping; move-out lands in ungrouped", () => {
  const projects = [
    project("p1", "Alpha", ["D:/work/alpha"], 0),
    project("p2", "Beta", ["D:/work/beta"], 1),
  ];
  const groups = groupSessionsByProject(
    [
      projectItem("moved", "D:/work/alpha", "2026-08-01T08:00:00Z"),
      projectItem("out", "D:/work/alpha", "2026-08-02T08:00:00Z"),
    ],
    projects,
    { moved: "p2", out: null },
  );
  assert.deepEqual(groups.find((g) => g.key === "project:p2").rows.map((r) => r.id), ["moved"]);
  assert.equal(groups.find((g) => g.key === "project:p1").rows.length, 0);
  const ungrouped = groups.find((g) => g.kind === "ungrouped");
  assert.deepEqual(ungrouped.rows.map((r) => r.id), ["out"], "显式移出不得经 tier 2 复活");
});

test("project view: temporary sessions join only via explicit assignment", () => {
  const projects = [project("p1", "Alpha", ["C:/Users/x"], 0)];
  const groups = groupSessionsByProject(
    [temporaryItem("t1", "2026-08-01T08:00:00Z")],
    projects,
    {},
  );
  assert.equal(groups[0].rows.length, 0, "临时会话绝不自动归组");
  const adopted = groupSessionsByProject(
    [temporaryItem("t1", "2026-08-01T08:00:00Z")],
    projects,
    { t1: "p1" },
  );
  assert.deepEqual(adopted[0].rows.map((r) => r.id), ["t1"], "显式归属(转正)有效");
  assert.equal(adopted.length, 1, "全被认领时无未分组桶");
});

test("project view: stale assignment ids fall through to auto grouping", () => {
  const projects = [project("p1", "Alpha", ["D:/work/alpha"], 0)];
  const groups = groupSessionsByProject(
    [projectItem("a1", "D:/work/alpha", "2026-08-01T08:00:00Z")],
    projects,
    { a1: "prj-deleted" },
  );
  assert.deepEqual(groups[0].rows.map((r) => r.id), ["a1"]);
});

test("project view: longest matching root wins for nested roots", () => {
  const projects = [
    project("p1", "Work", ["D:/work"], 0),
    project("p2", "Deep", ["D:/work/deep"], 1),
  ];
  const groups = groupSessionsByProject(
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

test("project view: 'bound' work sessions auto-group like code sessions", () => {
  const projects = [project("p1", "Alpha", ["D:/work/alpha"], 0)];
  const groups = groupSessionsByProject(
    [boundItem("w1", "D:/work/alpha/sub", "2026-08-01T08:00:00Z")],
    projects,
    {},
  );
  assert.deepEqual(groups[0].rows.map((r) => r.id), ["w1"]);
});

test("project view: rows sort by updatedAt descending inside groups", () => {
  const projects = [project("p1", "Alpha", ["D:/work/alpha"], 0)];
  const groups = groupSessionsByProject(
    [
      projectItem("old", "D:/work/alpha", "2026-07-01T08:00:00Z"),
      projectItem("new", "D:/work/alpha", "2026-08-01T08:00:00Z"),
      { id: "null-item", workspaceKind: "project", workspacePath: "D:/work/alpha" },
      null,
    ],
    projects,
    {},
  );
  assert.deepEqual(groups[0].rows.map((r) => r.id), ["new", "old", "null-item"]);
});

test("project view without projects puts everything in ungrouped", () => {
  const groups = groupSessionsByProject(
    [projectItem("a1", "D:/w", "x"), temporaryItem("t1", "y")],
    [],
    {},
  );
  assert.deepEqual(groups.map((g) => g.kind), ["ungrouped"]);
  assert.equal(groups[0].rows.length, 2);
});

// ── Move-picker helpers ────────────────────────────────────────────────────

test("resolveSessionProjectId mirrors the project-view tiers", () => {
  const projects = [project("p1", "Alpha", ["D:/work/alpha"], 0)];
  const item = projectItem("a1", "D:/work/alpha", "2026-08-01T08:00:00Z");
  assert.equal(resolveSessionProjectId(item, projects, {}), "p1");
  assert.equal(resolveSessionProjectId(item, projects, { a1: "p1" }), "p1");
  assert.equal(resolveSessionProjectId(item, projects, { a1: null }), null, "显式移出优先");
  assert.equal(resolveSessionProjectId(temporaryItem("t1", "x"), projects, { t1: "p1" }), "p1");
  assert.equal(resolveSessionProjectId(temporaryItem("t1", "x"), projects, {}), null);
  assert.equal(resolveSessionProjectId(item, [], { a1: "prj-gone" }), null);
});

test("projectCoversPath and needsAddFolderConfirm share the containment rule", () => {
  const projects = [project("p1", "Alpha", ["D:/work/alpha"], 0)];
  assert.equal(projectCoversPath(projects[0], "D:/work/alpha"), true);
  assert.equal(projectCoversPath(projects[0], "D:/work/alpha/sub"), true);
  assert.equal(projectCoversPath(projects[0], "D:/work/beta"), false);
  assert.equal(projectCoversPath(null, "D:/work/alpha"), false);
  assert.equal(needsAddFolderConfirm(projectItem("s", "D:/work/beta"), projects[0]), true);
  assert.equal(needsAddFolderConfirm(projectItem("s", "D:/work/alpha/x"), projects[0]), false);
  assert.equal(needsAddFolderConfirm(temporaryItem("t"), projects[0]), false, "临时会话无目录,直移");
});

// ── Folder-project auto-materialization input ──────────────────────────────

test("uncoveredWorkspaceRoots dedupes and drops covered/temporary workspaces", () => {
  const projects = [project("p1", "Work", ["D:/work/alpha"], 0)];
  const roots = uncoveredWorkspaceRoots(
    [
      projectItem("a1", "D:/work/alpha", "x"),
      projectItem("a2", "D:/work/alpha/sub", "x"),
      boundItem("w1", "D:/work/beta", "x"),
      boundItem("w2", "D:/work/beta", "x"),
      temporaryItem("t1", "x"),
      null,
      { id: "no-path", workspaceKind: "project", workspacePath: "" },
    ],
    projects,
    {},
  );
  assert.deepEqual(roots, [{ root: "D:/work/beta", sessionIds: ["w1", "w2"] }]);
});

test("uncoveredWorkspaceRoots skips sessions with any assignment entry", () => {
  // 删除项目把成员写成显式移出(null):被删文件夹不得因旧会话立刻重建,
  // 只有无条目的新会话驱动 ensure;显式归属它处(Some)同样不再驱动。
  const projects = [project("p1", "Elsewhere", ["D:/other"], 0)];
  const roots = uncoveredWorkspaceRoots(
    [
      projectItem("moved-out", "D:/work/beta", "x"),
      projectItem("filed", "D:/work/beta", "x"),
      projectItem("fresh", "D:/work/beta", "x"),
    ],
    projects,
    { "moved-out": null, filed: "p1" },
  );
  assert.deepEqual(roots, [{ root: "D:/work/beta", sessionIds: ["fresh"] }]);
});

test("uncoveredWorkspaceRoots: an ancestor project root covers descendant folders", () => {
  const projects = [project("p1", "Work", ["D:/work"], 0)];
  assert.deepEqual(
    uncoveredWorkspaceRoots([projectItem("a1", "D:/work/alpha", "x")], projects, {}),
    [],
    "D:/work 已覆盖其子目录,无需为子目录建项目",
  );
  assert.deepEqual(
    uncoveredWorkspaceRoots([projectItem("a1", "D:/elsewhere", "x")], projects, {}),
    [{ root: "D:/elsewhere", sessionIds: ["a1"] }],
  );
});

test("uncoveredWorkspaceRoots with no projects lists every distinct bound folder", () => {
  const roots = uncoveredWorkspaceRoots(
    [
      projectItem("a1", "D:/one", "x"),
      boundItem("w1", "D:/two", "x"),
      temporaryItem("t1", "x"),
    ],
    [],
    {},
  );
  assert.deepEqual(roots, [
    { root: "D:/one", sessionIds: ["a1"] },
    { root: "D:/two", sessionIds: ["w1"] },
  ]);
});

// ── Cases ported from the legacy sidebar-grouping suite ────────────────────

test("folder view: missing updatedAt sorts as oldest without crashing", () => {
  const groups = groupSessionsByFolder([
    projectItem("no-time", "D:/work/alpha", ""),
    projectItem("timed", "D:/work/alpha", "2026-08-01T08:00:00Z"),
    { id: "null-item", workspaceKind: "project", workspacePath: "D:/work/alpha" },
    null,
  ]);
  assert.equal(groups.length, 1);
  assert.deepEqual(groups[0].rows.map((r) => r.id), ["timed", "no-time", "null-item"]);
});

test("folder view: project sessions without a workspace path fall into temporary", () => {
  const groups = groupSessionsByFolder([
    { id: "no-path", workspaceKind: "project", workspacePath: "", updatedAt: "2026-08-01T08:00:00Z" },
  ]);
  assert.equal(groups.length, 1);
  assert.equal(groups[0].key, TEMPORARY_GROUP_KEY);
});

// ── Review-finding ports from the PR stack lineage ─────────────────────────

test("windows roots match case-insensitively, posix roots stay case-sensitive", () => {
  // The store folds identity keys on Windows (root_keys_fold_case_only_on_windows);
  // the display-side longest-root guard must not let a case-differing workspace
  // path slip past its project (review finding 19).
  const projects = [project("p1", "Alpha", ["D:/Work/Alpha"], 0)];
  const groups = groupSessionsByProject(
    [projectItem("a1", "d:/work/alpha/sub", "2026-08-01T08:00:00Z")],
    projects,
    {},
  );
  assert.deepEqual(groups.find((g) => g.projectId === "p1").rows.map((r) => r.id), ["a1"]);
  const posix = [project("p2", "Posix", ["/home/u/Work"], 0)];
  const posixGroups = groupSessionsByProject(
    [projectItem("a2", "/home/u/work/sub", "2026-08-01T08:00:00Z")],
    posix,
    {},
  );
  assert.equal(posixGroups.find((g) => g.projectId === "p2").rows.length, 0, "posix 路径保持大小写敏感");
  assert.equal(posixGroups.find((g) => g.kind === "ungrouped").rows.length, 1);
});

test("needsAddFolderConfirm is null-safe on both ends", () => {
  const target = project("p1", "Alpha", ["D:/work/alpha"], 0);
  assert.equal(needsAddFolderConfirm(null, target), false, "missing session is a no-op");
  assert.equal(needsAddFolderConfirm(projectItem("a1", "x", "x"), null), false, "missing target is a no-op");
});
