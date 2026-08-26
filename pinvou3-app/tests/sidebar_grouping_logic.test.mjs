import assert from "node:assert/strict";
import test from "node:test";

import { TEMPORARY_GROUP_KEY, groupSessionsByFolder } from "../src/shared/sidebar-grouping.js";

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

test("project sessions bucket by workspace path", () => {
  const groups = groupSessionsByFolder([
    projectItem("a1", "D:/work/alpha", "2026-08-01T08:00:00Z"),
    projectItem("b1", "D:/work/beta", "2026-08-02T08:00:00Z"),
    projectItem("a2", "D:/work/alpha", "2026-08-03T08:00:00Z"),
  ]);
  assert.equal(groups.length, 2);
  const alpha = groups.find((g) => g.key === "D:/work/alpha");
  assert.deepEqual(alpha.rows.map((r) => r.id), ["a2", "a1"]);
});

test("rows sort by updatedAt descending within a group", () => {
  const groups = groupSessionsByFolder([
    projectItem("old", "D:/work/alpha", "2026-07-01T08:00:00Z"),
    projectItem("new", "D:/work/alpha", "2026-08-01T08:00:00Z"),
    projectItem("mid", "D:/work/alpha", "2026-07-15T08:00:00Z"),
  ]);
  assert.deepEqual(groups[0].rows.map((r) => r.id), ["new", "mid", "old"]);
});

test("groups sort by their latest activity, temporary group stays last", () => {
  const groups = groupSessionsByFolder([
    temporaryItem("t1", "2026-08-19T08:00:00Z"),
    projectItem("stale", "D:/work/old-project", "2026-07-01T08:00:00Z"),
    temporaryItem("t2", "2026-08-18T08:00:00Z"),
    projectItem("fresh", "D:/work/new-project", "2026-08-10T08:00:00Z"),
  ]);
  assert.deepEqual(groups.map((g) => g.key), [
    "D:/work/new-project",
    "D:/work/old-project",
    TEMPORARY_GROUP_KEY,
  ]);
  const temporary = groups[groups.length - 1];
  assert.deepEqual(temporary.rows.map((r) => r.id), ["t1", "t2"]);
});

test("empty or invalid input returns an empty array", () => {
  assert.deepEqual(groupSessionsByFolder([]), []);
  assert.deepEqual(groupSessionsByFolder(), []);
  assert.deepEqual(groupSessionsByFolder(null), []);
});

test("missing updatedAt does not crash and sorts as oldest", () => {
  const groups = groupSessionsByFolder([
    projectItem("no-time", "D:/work/alpha", ""),
    projectItem("timed", "D:/work/alpha", "2026-08-01T08:00:00Z"),
    { id: "null-item", workspaceKind: "project", workspacePath: "D:/work/alpha" },
    null,
  ]);
  assert.equal(groups.length, 1);
  assert.deepEqual(groups[0].rows.map((r) => r.id), ["timed", "no-time", "null-item"]);
});

test("project sessions without a workspace path fall into the temporary group", () => {
  const groups = groupSessionsByFolder([
    { id: "no-path", workspaceKind: "project", workspacePath: "", updatedAt: "2026-08-01T08:00:00Z" },
  ]);
  assert.equal(groups.length, 1);
  assert.equal(groups[0].key, TEMPORARY_GROUP_KEY);
});
