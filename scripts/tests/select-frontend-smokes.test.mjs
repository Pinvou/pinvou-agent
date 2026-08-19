import assert from "node:assert/strict";
import test from "node:test";

import {
  FULL_FRONTEND_SMOKES,
  selectFrontendSmokes,
} from "../select-frontend-smokes.mjs";

const labels = (items) => items.map(({ kind, target }) => `${kind}:${target}`);
const fullLabels = labels(FULL_FRONTEND_SMOKES);

test("shared frontend paths fail closed to the full browser suite", () => {
  assert.deepEqual(
    labels(selectFrontendSmokes(["pinvou3-app/src/shared/i18n.js"])),
    fullLabels,
  );
});

test("test infrastructure changes fail closed to the full browser suite", () => {
  assert.deepEqual(
    labels(selectFrontendSmokes(["pinvou3-app/tests/ui_test_server.js"])),
    fullLabels,
  );
});

test("frontend smoke runner changes fail closed to the full browser suite", () => {
  assert.deepEqual(
    labels(selectFrontendSmokes(["scripts/run-frontend-smokes.mjs"])),
    fullLabels,
  );
});

test("shared smoke manifest changes fail closed to the full browser suite", () => {
  assert.deepEqual(
    labels(selectFrontendSmokes(["scripts/select-frontend-smokes.mjs"])),
    fullLabels,
  );
});

test("feature-local changes select core and feature smokes", () => {
  assert.deepEqual(
    labels(
      selectFrontendSmokes([
        "README.md",
        "pinvou3-app/src/features/settings/SettingsView.jsx",
        "pinvou3-app/src/features/pet/PetWindow.jsx",
      ]),
    ),
    [
      "npm:test:settings-ui",
      "node:tests/pet_selector_ui_smoke.js",
      "npm:test:ui-smoke",
    ],
  );
});

test("artifact card and Knowledge changes run the unified browser smoke before legacy shells", () => {
  assert.deepEqual(
    labels(selectFrontendSmokes(["pinvou3-app/src/features/tools/tool-common.jsx"])),
    [
      "npm:test:artifact-browser-ui",
      "npm:test:tool-store",
      "npm:test:tool-store-import",
      "npm:test:tool-store-grouping",
      "npm:test:ui-smoke",
    ],
  );
  assert.deepEqual(
    labels(selectFrontendSmokes(["pinvou3-app/src/features/knowledge/KnowledgeView.jsx"])),
    ["npm:test:artifact-browser-ui", "npm:test:kb-smoke", "npm:test:ui-smoke"],
  );
});

test("relay changes select the web UI smoke", () => {
  assert.deepEqual(
    labels(selectFrontendSmokes(["remote-control-relay/server.js"])),
    ["npm:test:webui", "npm:test:ui-smoke"],
  );
});

test("unknown frontend features fail closed to the full browser suite", () => {
  assert.deepEqual(
    labels(selectFrontendSmokes(["pinvou3-app/src/features/search/SearchView.jsx"])),
    fullLabels,
  );
});

test("an empty or unrelated diff fails closed", () => {
  assert.deepEqual(labels(selectFrontendSmokes(["docs/ci.md"])), fullLabels);
});
