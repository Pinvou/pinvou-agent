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
  // #445 bound plain work sessions: a standalone 'bound' kind that, like
  // 'project', carries a real directory (review #452 finding 5).
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

// Materialized project with origin=folder (the only coverage source for
// anchoring decisions, §9.9).
const folderProject = (id, name, roots, position) => ({ ...project(id, name, roots, position), origin: 'folder' });

// ── Folder view (pure physical layer) ──────────────────────────────────────

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
  assert.equal(alpha.projectId, null, "folder groups are project-agnostic");
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

// ── Project view (pure logic layer + ungrouped bucket) ─────────────────────

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
  assert.deepEqual(ungrouped.rows.map((r) => r.id), ["out"], "explicit move-out must not be revived via tier 2");
});

test("project view: temporary sessions join only via explicit assignment", () => {
  const projects = [project("p1", "Alpha", ["C:/Users/x"], 0)];
  const groups = groupSessionsByProject(
    [temporaryItem("t1", "2026-08-01T08:00:00Z")],
    projects,
    {},
  );
  assert.equal(groups[0].rows.length, 0, "temporary sessions never auto-group");
  const adopted = groupSessionsByProject(
    [temporaryItem("t1", "2026-08-01T08:00:00Z")],
    projects,
    { t1: "p1" },
  );
  assert.deepEqual(adopted[0].rows.map((r) => r.id), ["t1"], "explicit assignment (promotion) works");
  assert.equal(adopted.length, 1, "no ungrouped bucket when everything is claimed");
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

test("project view: smallest position claims sessions covered by several projects", () => {
  // §9.9 (2026-09-11 ruling): cross-project overlap/nesting is legal; on
  // multiple hits the project with the smallest position claims the session
  // (sidebar ordering is the user-controllable tiebreaker) instead of the
  // longest root — same rule as the backend resolve_session_project.
  const items = [
    projectItem("shallow", "D:/work/other", "2026-08-01T08:00:00Z"),
    projectItem("deep", "D:/work/deep/x", "2026-08-02T08:00:00Z"),
  ];
  const groups = groupSessionsByProject(
    items,
    [project("p1", "Work", ["D:/work"], 0), project("p2", "Deep", ["D:/work/deep"], 1)],
    {},
  );
  assert.deepEqual(groups.find((g) => g.projectId === "p1").rows.map((r) => r.id), ["deep", "shallow"]);
  assert.equal(groups.find((g) => g.projectId === "p2").rows.length, 0);
  // Flipping position flips ownership: drag-sorting alone re-decides.
  const flipped = groupSessionsByProject(
    items,
    [project("p1", "Work", ["D:/work"], 1), project("p2", "Deep", ["D:/work/deep"], 0)],
    {},
  );
  assert.deepEqual(flipped.find((g) => g.projectId === "p2").rows.map((r) => r.id), ["deep"]);
  assert.deepEqual(flipped.find((g) => g.projectId === "p1").rows.map((r) => r.id), ["shallow"]);
  // Input order does not affect the outcome (id is the final tiebreaker;
  // sorting happens before the decision).
  const shuffled = groupSessionsByProject(
    items,
    [project("p2", "Deep", ["D:/work/deep"], 1), project("p1", "Work", ["D:/work"], 0)],
    {},
  );
  assert.deepEqual(shuffled.find((g) => g.projectId === "p1").rows.map((r) => r.id), ["deep", "shallow"]);
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
  assert.equal(resolveSessionProjectId(item, projects, { a1: null }), null, "explicit move-out wins");
  assert.equal(resolveSessionProjectId(temporaryItem("t1", "x"), projects, { t1: "p1" }), "p1");
  assert.equal(resolveSessionProjectId(temporaryItem("t1", "x"), projects, {}), null);
  // The real fork the picker must survive: a stale id does not stick as
  // "ungrouped" — with a non-empty project list whose root still covers the
  // path, resolution falls through to tier 2 and returns that project.
  // (projects=[] would trivially yield null and pin nothing.)
  assert.equal(resolveSessionProjectId(item, projects, { a1: "prj-gone" }), "p1", "stale id falls through to root matching");
});

test("projectCoversPath and needsAddFolderConfirm share the containment rule", () => {
  const projects = [project("p1", "Alpha", ["D:/work/alpha"], 0)];
  assert.equal(projectCoversPath(projects[0], "D:/work/alpha"), true);
  assert.equal(projectCoversPath(projects[0], "D:/work/alpha/sub"), true);
  assert.equal(projectCoversPath(projects[0], "D:/work/beta"), false);
  assert.equal(projectCoversPath(null, "D:/work/alpha"), false);
  assert.equal(needsAddFolderConfirm(projectItem("s", "D:/work/beta"), projects[0]), true);
  assert.equal(needsAddFolderConfirm(projectItem("s", "D:/work/alpha/x"), projects[0]), false);
  assert.equal(needsAddFolderConfirm(temporaryItem("t"), projects[0]), false, "temporary sessions have no folder; move directly");
});

// ── Folder-project auto-materialization input ──────────────────────────────

test("uncoveredWorkspaceRoots dedupes and drops anchored/temporary workspaces", () => {
  // Anchor coverage (§9.9): only an origin=folder project whose roots contain
  // the exact path counts as coverage; "D:/work/alpha" is anchored → a1 does
  // not drive; "D:/work/alpha/sub" is not exactly anchored → a2 drives
  // (materializing a new project for the subdirectory; overlap is legal).
  const projects = [folderProject("p1", "Work", ["D:/work/alpha"], 0)];
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
  assert.deepEqual(roots, [
    { root: "D:/work/alpha/sub", sessionIds: ["a2"] },
    { root: "D:/work/beta", sessionIds: ["w1", "w2"] },
  ]);
});

test("uncoveredWorkspaceRoots skips sessions with any assignment entry", () => {
  // Deleting a project writes its members as explicit move-out (null): a
  // deleted folder must not be rebuilt immediately because of old sessions —
  // only entry-less new sessions drive ensure; explicit assignment elsewhere
  // (Some) no longer drives either.
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

test("uncoveredWorkspaceRoots: only an exact folder-project anchor covers", () => {
  // The old ancestor-based semantics retired with anchor reuse (§9.9): a
  // materialized project anchored at D:/work no longer covers its
  // subdirectories — those still materialize as usual (matching the backend
  // ensure's exact anchoring).
  const projects = [folderProject("p1", "Work", ["D:/work"], 0)];
  assert.deepEqual(
    uncoveredWorkspaceRoots([projectItem("a1", "D:/work/alpha", "x")], projects, {}),
    [{ root: "D:/work/alpha", sessionIds: ["a1"] }],
    "an ancestor anchor no longer covers subdirectories",
  );
  assert.deepEqual(
    uncoveredWorkspaceRoots([projectItem("a2", "D:/work", "x")], projects, {}),
    [],
    "only an exact anchor covers",
  );
  // A manual project (no origin=folder) referencing the same path does not
  // count as coverage; the browse/materialize channel still creates anew.
  const manual = [project("p2", "Manual", ["D:/work"], 0)];
  assert.deepEqual(
    uncoveredWorkspaceRoots([projectItem("a3", "D:/work", "x")], manual, {}),
    [{ root: "D:/work", sessionIds: ["a3"] }],
    "being referenced by a manual project is not anchoring",
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
  assert.equal(posixGroups.find((g) => g.projectId === "p2").rows.length, 0, "posix paths stay case-sensitive");
  assert.equal(posixGroups.find((g) => g.kind === "ungrouped").rows.length, 1);
});

test("windows roots match across separator shapes and trailing separators", () => {
  // Finding 40: isUnderRoot folds `\` -> `/` and strips trailing separators
  // for windows-shaped paths, mirroring the store's filesystem_path_identity_key
  // (store.rs); a mixed-shape or trailing-separator root must not miss tier 2.
  const projects = [project("p1", "Alpha", ["D:\\work\\alpha\\"], 0)];
  const groups = groupSessionsByProject(
    [
      projectItem("a1", "D:/work/alpha/sub", "2026-08-01T08:00:00Z"),
      projectItem("a2", "D:\\work\\alpha", "2026-08-02T08:00:00Z"),
    ],
    projects,
    {},
  );
  assert.deepEqual(
    groups.find((g) => g.projectId === "p1").rows.map((r) => r.id).sort((x, y) => x.localeCompare(y)),
    ["a1", "a2"],
  );
  assert.equal(groups.some((g) => g.kind === "ungrouped"), false, "both separator shapes must hit tier 2");
  // The move-picker fork shares the fold (same isUnderRoot).
  assert.equal(resolveSessionProjectId(projectItem("a3", "D:\\Work\\Alpha", "x"), projects, {}), "p1");
});

test("needsAddFolderConfirm is null-safe on both ends", () => {
  const target = project("p1", "Alpha", ["D:/work/alpha"], 0);
  assert.equal(needsAddFolderConfirm(null, target), false, "missing session is a no-op");
  assert.equal(needsAddFolderConfirm(projectItem("a1", "x", "x"), null), false, "missing target is a no-op");
});
