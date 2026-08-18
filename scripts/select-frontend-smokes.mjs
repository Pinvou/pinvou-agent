#!/usr/bin/env node

import { pathToFileURL } from "node:url";

const command = (kind, target) => ({ kind, target });

export const FULL_FRONTEND_SMOKES = Object.freeze([
  command("npm", "test:bridge-smoke"),
  command("npm", "test:code-viewer-ui"),
  command("npm", "test:code-viewer-diff-ui"),
  command("npm", "test:workspace-panel-ui"),
  command("npm", "test:workspace-panel-session-ui"),
  command("npm", "test:reader-ui"),
  command("npm", "test:reader-diff-ui"),
  command("npm", "test:markdown-artifact-ui"),
  command("node", "tests/detached_boot_smoke.js"),
  command("node", "tests/drag_gesture_smoke.js"),
  command("node", "tests/update_notice_ui_smoke.js"),
  command("node", "tests/scheduled_tasks_smoke.js"),
  command("npm", "test:diff-ui"),
  command("npm", "test:choice-card-ui"),
  command("node", "tests/pet_selector_ui_smoke.js"),
  command("npm", "test:tool-store-import"),
  command("npm", "test:tool-store-grouping"),
  command("npm", "test:webui"),
]);

const CORE_SMOKE = command("npm", "test:ui-smoke");
const FEATURE_COMMANDS = new Map([
  ["knowledge", [command("npm", "test:kb-smoke")]],
  ["pet", [command("node", "tests/pet_selector_ui_smoke.js")]],
  ["scheduled", [command("node", "tests/scheduled_tasks_smoke.js")]],
  ["settings", [command("npm", "test:settings-ui")]],
  ["tools", [command("npm", "test:tool-store"), command("npm", "test:tool-store-import"), command("npm", "test:tool-store-grouping")]],
  ["updater", [command("node", "tests/update_notice_ui_smoke.js")]],
  ["web", [command("npm", "test:webui")]],
]);

const FULL_PREFIXES = [
  ".github/workflows/pr-check.yml",
  ".gitmodules",
  "pinvou3-app/eslint.config.mjs",
  "pinvou3-app/package-lock.json",
  "pinvou3-app/package.json",
  "pinvou3-app/scripts/tauri/",
  "pinvou3-app/src/app/",
  "pinvou3-app/src/components/",
  "pinvou3-app/src/hooks/",
  "pinvou3-app/src/index.html",
  "pinvou3-app/src/platform/",
  "pinvou3-app/src/shared/",
  "pinvou3-app/src-tauri/config/",
  "pinvou3-app/src-tauri/packaging/",
  "pinvou3-app/src-tauri/resources/platforms/linux/knowledge-host/",
  "pinvou3-app/src-tauri/tauri.conf.json",
  "pinvou3-app/tests/",
  "pinvou3-app/vite.config.js",
  "scripts/mcp-server-contract-smoke.py",
  "scripts/run-frontend-smokes.mjs",
  "scripts/run-user-journey-tests.sh",
  "scripts/select-frontend-smokes.mjs",
];

function isFrontendRelevant(path) {
  return (
    path === ".gitmodules" ||
    path === ".github/workflows/pr-check.yml" ||
    path.startsWith("pinvou3-app/src/") ||
    path.startsWith("pinvou3-app/tests/") ||
    path.startsWith("pinvou3-app/scripts/tauri/") ||
    path.startsWith("pinvou3-app/src-tauri/config/") ||
    path.startsWith("pinvou3-app/src-tauri/packaging/") ||
    path.startsWith("pinvou3-app/src-tauri/resources/platforms/linux/knowledge-host/") ||
    path.startsWith("remote-control-relay/") ||
    path === "pinvou3-app/package.json" ||
    path === "pinvou3-app/package-lock.json" ||
    path === "pinvou3-app/vite.config.js" ||
    path === "pinvou3-app/eslint.config.mjs" ||
    path === "pinvou3-app/src-tauri/tauri.conf.json" ||
    path === "scripts/run-user-journey-tests.sh" ||
    path === "scripts/run-frontend-smokes.mjs" ||
    path === "scripts/mcp-server-contract-smoke.py" ||
    path === "scripts/select-frontend-smokes.mjs"
  );
}

function requiresFullSuite(path) {
  return FULL_PREFIXES.some((prefix) =>
    prefix.endsWith("/") ? path.startsWith(prefix) : path === prefix,
  );
}

function commandKey(item) {
  return `${item.kind}\0${item.target}`;
}

/**
 * Return the smallest safe browser-smoke set for a PR diff.
 * Unknown frontend paths fail closed to the full suite.
 */
export function selectFrontendSmokes(paths) {
  const selected = new Map([[commandKey(CORE_SMOKE), CORE_SMOKE]]);
  let sawFrontendPath = false;

  for (const path of paths) {
    if (!isFrontendRelevant(path)) continue;
    sawFrontendPath = true;

    if (requiresFullSuite(path)) return [...FULL_FRONTEND_SMOKES];

    if (path.startsWith("remote-control-relay/")) {
      const item = command("npm", "test:webui");
      selected.set(commandKey(item), item);
      continue;
    }

    if (path.startsWith("pinvou3-app/src/assets/pet/")) {
      const item = command("node", "tests/pet_selector_ui_smoke.js");
      selected.set(commandKey(item), item);
      continue;
    }

    const match = path.match(/^pinvou3-app\/src\/features\/([^/]+)\//);
    const commands = match ? FEATURE_COMMANDS.get(match[1]) : undefined;
    if (!commands) return [...FULL_FRONTEND_SMOKES];
    for (const item of commands) selected.set(commandKey(item), item);
  }

  return sawFrontendPath ? [...selected.values()] : [...FULL_FRONTEND_SMOKES];
}

async function main() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  const paths = chunks
    .join("")
    .split(/\r?\n/u)
    .map((path) => path.trim())
    .filter(Boolean);

  for (const item of selectFrontendSmokes(paths)) {
    process.stdout.write(`${item.kind}\t${item.target}\n`);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
