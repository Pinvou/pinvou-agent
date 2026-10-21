import assert from "node:assert/strict";
import test from "node:test";

import {
  FULL_FRONTEND_SMOKES,
  selectFrontendSmokes,
} from "../select-frontend-smokes.mjs";

const labels = (items) => items.map(({ kind, target }) => `${kind}:${target}`);
const fullLabels = labels(FULL_FRONTEND_SMOKES);

const fullSuiteCases = [
  [
    "shared frontend paths fail closed to the full browser suite",
    ["pinvou3-app/src/shared/i18n.js"],
  ],
  [
    "test infrastructure changes fail closed to the full browser suite",
    ["pinvou3-app/tests/ui_test_server.js"],
  ],
  [
    "frontend smoke runner changes fail closed to the full browser suite",
    ["scripts/run-frontend-smokes.mjs"],
  ],
  [
    "shared smoke manifest changes fail closed to the full browser suite",
    ["scripts/select-frontend-smokes.mjs"],
  ],
  [
    "Linux knowledge host package resources run the full suite",
    [
      "pinvou3-app/src-tauri/resources/platforms/linux/knowledge-host/pinvou-knowledge-host-helper",
    ],
  ],
  [
    "unknown frontend features fail closed to the full browser suite",
    ["pinvou3-app/src/features/search/SearchView.jsx"],
  ],
  [
    "an empty or unrelated diff fails closed",
    ["docs/ci.md"],
  ],
];

for (const [name, paths] of fullSuiteCases) {
  test(name, () => {
    assert.deepEqual(labels(selectFrontendSmokes(paths)), fullLabels);
  });
}

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
      "npm:test:ui-smoke",
      "npm:test:settings-ui",
      "npm:test:composer-tools-smoke",
      "npm:test:composer-pending-enable-smoke",
      "node:tests/pet_selector_ui_smoke.js",
    ],
  );
});

test("knowledge changes select the desktop knowledge smoke", () => {
  assert.deepEqual(
    labels(
      selectFrontendSmokes([
        "pinvou3-app/src/features/knowledge/KnowledgeBaseView.jsx",
      ]),
    ),
    [
      "npm:test:ui-smoke",
      "npm:test:kb-smoke",
    ],
  );
});

test("relay changes select the web UI smoke", () => {
  assert.deepEqual(
    labels(selectFrontendSmokes(["remote-control-relay/server.js"])),
    ["npm:test:ui-smoke", "npm:test:webui"],
  );
});

test("wrapper-only changes select the web UI smoke plus the core smoke", () => {
  assert.deepEqual(
    labels(selectFrontendSmokes(["pinvou3-app/scripts/run-webui-smoke.mjs"])),
    ["npm:test:ui-smoke", "npm:test:webui"],
  );
});
