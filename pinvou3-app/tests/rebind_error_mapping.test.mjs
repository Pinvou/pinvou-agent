// Frontend half of the rebind marker contract (review #463 round-8 minor 10):
// nothing exercised the FE↔BE mapping before, so a marker the backend emits
// could silently fall through to the raw-error branch and surface unlocalized
// backend prose. These tests pin both directions — every marker classifies,
// and every marker resolves to real copy in all three languages.
import assert from "node:assert/strict";
import test from "node:test";

import { dictEn } from "../src/shared/i18n/en.js";
import { dictJa } from "../src/shared/i18n/ja.js";
import { dictZh } from "../src/shared/i18n/zh.js";
import {
  REBIND_MARKER_MESSAGE_KEYS,
  REBIND_SESSIONS_BUSY,
  classifyRebindError,
} from "../src/features/projects/rebindErrors.js";

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

test("the three destination markers are mapped and distinct", () => {
  // A silent collision would make one marker unreachable: the classifier picks
  // the first matching prefix.
  for (const marker of ["REBIND_TO_ROOT:", "REBIND_TO_NESTED:", "REBIND_TO_UNUSABLE:"]) {
    const classified = classifyRebindError(marker, dictEn);
    assert.equal(classified.kind, "copy", `${marker} must be mapped`);
  }
  assert.deepEqual(
    [
      classifyRebindError("REBIND_TO_ROOT:", dictEn).message,
      classifyRebindError("REBIND_TO_NESTED:", dictEn).message,
      classifyRebindError("REBIND_TO_UNUSABLE:", dictEn).message,
    ],
    [
      dictEn.uiProjects.rebindToRoot,
      dictEn.uiProjects.rebindToNested,
      dictEn.uiProjects.rebindToUnusable,
    ],
  );
});

test("an unmapped backend error stays verbatim for diagnostics", () => {
  const classified = classifyRebindError(new Error("rebind_workspace_root: boom"), dictEn);
  assert.deepEqual(classified, {
    kind: "raw",
    message: "Error: rebind_workspace_root: boom",
  });
});
