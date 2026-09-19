import assert from "node:assert/strict";
import test from "node:test";

import {
  compareSessionsByRecentUpdate,
  compareSessionsPinnedFirst,
  filterSessionsByTab,
  groupSessionsByLocalDate,
  sessionListComparator,
} from "../src/shared/session-list-pipeline.js";

// Sessions in the shape produced by main.jsx allSidebarTasks / SearchView
// sourceHistory: pinned/pinnedAt/updatedAt plus taskKind for the tab filter.
const chat = (id, overrides = {}) => ({ id, updatedAt: `2026-01-0${id}`, ...overrides });

const FIXTURE = [
  chat(1, { pinned: true, pinnedAt: "2026-01-05", updatedAt: "2026-01-01" }),
  chat(2, { taskKind: "codex", updatedAt: "2026-01-03" }),
  chat(3, { taskKind: "scheduled", updatedAt: "2026-01-02" }),
  chat(4, { updatedAt: "2026-01-04" }),
  chat(5, { pinned: "yes", updatedAt: "2026-01-06" }), // truthy non-boolean pinned
];

test("filterSessionsByTab keeps whole tabs and copies the list on 'all'", () => {
  assert.deepEqual(filterSessionsByTab(FIXTURE, "pinned").map(c => c.id), [1, 5]);
  assert.deepEqual(filterSessionsByTab(FIXTURE, "code").map(c => c.id), [2]);
  assert.deepEqual(filterSessionsByTab(FIXTURE, "scheduled").map(c => c.id), [3]);
  assert.deepEqual(filterSessionsByTab(FIXTURE, "all").map(c => c.id), [1, 2, 3, 4, 5]);
  assert.deepEqual(filterSessionsByTab(FIXTURE, "whatever").map(c => c.id), [1, 2, 3, 4, 5]);
  const input = [chat(1)];
  const all = filterSessionsByTab(input, "all");
  assert.notEqual(all, input, "'all' must still return a fresh array so in-place sort is safe");
});

test("compareSessionsPinnedFirst floats pinned sessions and sorts each tier by its own timestamp", () => {
  const rows = [
    chat(1, { updatedAt: "2026-01-01" }),
    chat(2, { pinned: true, pinnedAt: "2026-01-02", updatedAt: "2026-01-08" }),
    chat(3, { updatedAt: "2026-01-03" }),
    chat(4, { pinned: true, updatedAt: "2026-01-07" }), // no pinnedAt: falls back to updatedAt
  ];
  const sorted = [...rows].sort(compareSessionsPinnedFirst);
  assert.deepEqual(sorted.map(c => c.id), [4, 2, 3, 1]);
});

test("compareSessionsByRecentUpdate is plain updatedAt order with pinnedAt fallback", () => {
  const rows = [
    chat(1, { updatedAt: "2026-01-01" }),
    chat(2, { updatedAt: "", pinnedAt: "2026-01-09" }), // empty updatedAt: pinnedAt fallback
    chat(3, { updatedAt: "2026-01-05" }),
  ];
  assert.deepEqual([...rows].sort(compareSessionsByRecentUpdate).map(c => c.id), [2, 3, 1]);
  assert.deepEqual([...rows].sort(sessionListComparator("recent")).map(c => c.id), [2, 3, 1]);
});

test("sessionListComparator picks pinned-first only for 'pinned_first'", () => {
  assert.equal(sessionListComparator("pinned_first"), compareSessionsPinnedFirst);
  assert.equal(sessionListComparator("recent"), compareSessionsByRecentUpdate);
  assert.equal(sessionListComparator(), compareSessionsByRecentUpdate);
  const rows = [
    chat(1, { pinned: true, updatedAt: "2026-01-01" }),
    chat(2, { updatedAt: "2026-01-02" }),
  ];
  assert.deepEqual(
    [...rows].sort(sessionListComparator("pinned_first")).map(c => c.id),
    [1, 2],
    "pinned_first must hoist pinned sessions above newer unpinned ones",
  );
});

test("groupSessionsByLocalDate groups in input order, newest day first, unknown sinks last", () => {
  const rows = [
    { id: "a", updatedAt: "2026-03-02" },
    { id: "b", updatedAt: "2026-03-02" },
    { id: "c", updatedAt: "" }, // no usable timestamp
    { id: "d", updatedAt: "2026-03-05" },
    { id: "e" }, // missing field entirely
  ];
  const groups = groupSessionsByLocalDate(rows, (c) => (c.updatedAt ? c.updatedAt.slice(0, 10) : "unknown"));
  assert.deepEqual(groups.map(g => g.key), ["2026-03-05", "2026-03-02", "unknown"]);
  assert.deepEqual(groups[1].rows.map(c => c.id), ["a", "b"], "row order inside a group follows the pre-sorted input");
  assert.deepEqual(groups[2].rows.map(c => c.id), ["c", "e"]);
  assert.deepEqual(groupSessionsByLocalDate([], () => "unknown"), []);
});
