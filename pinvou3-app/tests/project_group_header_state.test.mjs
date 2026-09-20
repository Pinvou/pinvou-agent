import assert from "node:assert/strict";
import test from "node:test";

import {
  groupHeaderHasMenu,
  resolveGroupHeaderEdit,
} from "../src/features/projects/projectGroupHeaderState.js";

const fn = () => {};

test("convert commits with the prefilled folder name on plain Enter", () => {
  // Finding 16: equality-as-cancel must not swallow the convert default path.
  assert.deepEqual(
    resolveGroupHeaderEdit({ mode: "convert", value: "work", label: "work", busy: false }),
    { action: "convert", value: "work" },
  );
});

test("rename with an unchanged value cancels, empty or busy cancels both modes", () => {
  assert.equal(resolveGroupHeaderEdit({ mode: "rename", value: "work", label: "work", busy: false }), null);
  assert.equal(resolveGroupHeaderEdit({ mode: "rename", value: "  ", label: "work", busy: false }), null);
  assert.equal(resolveGroupHeaderEdit({ mode: "convert", value: "", label: "work", busy: false }), null);
  assert.equal(resolveGroupHeaderEdit({ mode: "convert", value: "work", label: "work", busy: true }), null);
  assert.equal(resolveGroupHeaderEdit({ mode: "rename", value: "new", label: "work", busy: true }), null);
});

test("rename commits a changed value, whitespace is trimmed", () => {
  assert.deepEqual(
    resolveGroupHeaderEdit({ mode: "rename", value: "  new name ", label: "work", busy: false }),
    { action: "rename", value: "new name" },
  );
});

test("menu renders only when at least one action is available", () => {
  // Web has no projects backend: callbacks are undefined and no zero-item
  // menu button may render.
  assert.equal(groupHeaderHasMenu("folder", {}), false);
  assert.equal(groupHeaderHasMenu("folder", { onConvert: undefined }), false);
  assert.equal(groupHeaderHasMenu("folder", { onConvert: fn }), true);
  assert.equal(groupHeaderHasMenu("project", {}), false);
  assert.equal(groupHeaderHasMenu("project", { onRename: fn }), true);
  assert.equal(groupHeaderHasMenu("project", { onDelete: fn }), true);
  assert.equal(groupHeaderHasMenu("temporary", { onConvert: fn, onRename: fn, onDelete: fn }), false);
});
