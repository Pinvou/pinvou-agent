// Frontend half of the rebind marker contract (review #463 round-8 minor 10):
// nothing exercised the FE↔BE mapping before, so a marker the backend emits
// could silently fall through to the raw-error branch and surface unlocalized
// backend prose. These tests pin both directions — every marker classifies,
// every marker resolves to real copy in all three languages, and no marker the
// backend emits is missing from the classifier.
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";
import test from "node:test";

import { dictEn } from "../src/shared/i18n/en.js";
import { dictJa } from "../src/shared/i18n/ja.js";
import { dictZh } from "../src/shared/i18n/zh.js";
import {
  REBIND_MARKER_MESSAGE_KEYS,
  REBIND_SESSIONS_BUSY,
  classifyRebindError,
} from "../src/features/projects/rebindErrors.js";

const here = path.dirname(fileURLToPath(import.meta.url));
// The whole backend source tree, not a hand-picked pair of files: markers are
// emitted wherever a rebind step runs (the busy/eviction path lives in the
// assistant engine pool, codex/ACP carries its own rebind store), and a
// hardcoded list goes stale the moment a new call site appears (round-7
// should-fix).
const backendSources = (() => {
  const root = path.join(here, "..", "src-tauri", "src");
  const out = [];
  (function walk(dir) {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const child = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(child);
      else if (entry.name.endsWith(".rs")) out.push(child);
    }
  })(root);
  return out;
})();

test("old-root-exists escalates instead of rendering an error", () => {
  const classified = classifyRebindError(
    "REBIND_OLD_ROOT_EXISTS: the original folder still exists",
    dictEn,
  );
  assert.deepEqual(classified, { kind: "old-root-exists" });
});

test("the busy marker yields its session ids as data", () => {
  const classified = classifyRebindError(`${REBIND_SESSIONS_BUSY}: a1, b2`, dictEn);
  assert.equal(classified.kind, "sessions-busy");
  assert.deepEqual(classified.busySessionIds, ["a1", "b2"]);
});

test("the busy marker without ids does not fabricate one", () => {
  const classified = classifyRebindError(`${REBIND_SESSIONS_BUSY}:`, dictEn);
  assert.deepEqual(classified, { kind: "sessions-busy", busySessionIds: [] });
});

test("every copy marker resolves to trilingual uiProjects copy", () => {
  for (const [marker, key] of Object.entries(REBIND_MARKER_MESSAGE_KEYS)) {
    for (const [lang, dict] of [["zh", dictZh], ["en", dictEn], ["ja", dictJa]]) {
      const copy = dict.uiProjects[key];
      assert.equal(
        typeof copy,
        "string",
        `${key} must be a plain string in ${lang} (markers carry no arguments)`,
      );
      assert.ok(copy.length > 0, `${key} must be non-empty in ${lang}`);
    }
    const classified = classifyRebindError(`${marker}: backend prose`, dictEn);
    assert.equal(classified.kind, "copy", `${marker} must map to copy`);
    assert.equal(classified.message, dictEn.uiProjects[key]);
  }
});

test("the destination markers are mapped and distinct", () => {
  // A silent collision would make one marker unreachable: the classifier picks
  // the first matching prefix.
  assert.deepEqual(
    [
      classifyRebindError("REBIND_TO_ROOT:", dictEn).message,
      classifyRebindError("REBIND_TO_NESTED:", dictEn).message,
      classifyRebindError("REBIND_TO_UNUSABLE:", dictEn).message,
      classifyRebindError("REBIND_ROOTS_CONFLICT:", dictEn).message,
    ],
    [
      dictEn.uiProjects.rebindToRoot,
      dictEn.uiProjects.rebindToNested,
      dictEn.uiProjects.rebindToUnusable,
      dictEn.uiProjects.rebindRootsConflict,
    ],
  );
});

test("the backend emits no marker the frontend cannot classify", () => {
  // The Rust sources are the source of truth for the marker set: a new marker
  // must not ship without a mapping, or it reaches the dialog as raw backend
  // prose in every language. This is the test that would have caught
  // REBIND_ROOTS_CONFLICT before it was mapped.
  const markers = new Set();
  for (const file of backendSources) {
    for (const match of readFileSync(file, "utf8").matchAll(/"(REBIND_[A-Z_]+)/g)) {
      markers.add(match[1]);
    }
  }
  assert.ok(markers.size >= 5, `expected the backend marker set, saw ${[...markers]}`);
  for (const marker of markers) {
    const classified = classifyRebindError(`${marker}: payload`, dictEn);
    assert.notEqual(
      classified.kind,
      "raw",
      `${marker} is emitted by the backend but has no frontend mapping`,
    );
  }
});

test("an unmapped backend error stays verbatim for diagnostics", () => {
  const classified = classifyRebindError(new Error("rebind_workspace_root: boom"), dictEn);
  assert.deepEqual(classified, {
    kind: "raw",
    message: "Error: rebind_workspace_root: boom",
  });
});
