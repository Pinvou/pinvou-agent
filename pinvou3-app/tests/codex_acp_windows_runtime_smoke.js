const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const path = require("node:path");

const {
  BRIDGE_ENTRYPOINT,
  expectedMarker,
  prepareWindowsCodexBridge,
  WINDOWS_BRIDGE_CONFIG_PATH,
  WINDOWS_BRIDGE_ROOT,
  WINDOWS_NODE_EXECUTABLE,
  WINDOWS_NODE_VERSION,
} = require("../scripts/tauri/codex-bridge.js");
const {
  buildResourceManifest,
  composeEffectiveConfig,
} = require("../scripts/tauri/effective-config.js");
const { platformConfigPath } = require("../scripts/tauri/platform-config.js");

assert.equal(process.platform, "win32", "此冒烟测试必须在 Windows 原生 runner 执行");
assert.equal(process.arch, "x64", "Windows Codex Runtime 当前只支持 x64");

prepareWindowsCodexBridge();

const nodeVersion = spawnSync(WINDOWS_NODE_EXECUTABLE, ["--version"], {
  encoding: "utf8",
});
assert.equal(nodeVersion.error, undefined);
assert.equal(nodeVersion.status, 0);
assert.equal(nodeVersion.stdout.trim(), `v${WINDOWS_NODE_VERSION}`);

const marker = expectedMarker({ architecture: "x64" });
const bridgeVersion = spawnSync(
  WINDOWS_NODE_EXECUTABLE,
  [path.join(WINDOWS_BRIDGE_ROOT, BRIDGE_ENTRYPOINT), "--version"],
  {
    encoding: "utf8",
    env: { ...process.env, CODEX_PATH: "" },
  },
);
assert.equal(bridgeVersion.error, undefined);
assert.equal(bridgeVersion.status, 0, bridgeVersion.stderr);
assert.equal(
  bridgeVersion.stdout.trim(),
  `@agentclientprotocol/codex-acp ${marker.codex_acp_version}`,
);

const { effectiveConfig } = composeEffectiveConfig([
  platformConfigPath("win32"),
  WINDOWS_BRIDGE_CONFIG_PATH,
]);
const manifest = buildResourceManifest(effectiveConfig, { platform: "win32" });
const destinations = new Set(manifest.files.map((file) => file.destination));
assert.ok(destinations.has("runtime/node/node.exe"));
assert.ok(destinations.has("runtime/node/LICENSE"));
assert.ok(
  destinations.has(
    `runtime/codex-bridge/${BRIDGE_ENTRYPOINT.replaceAll(path.sep, "/")}`,
  ),
);
assert.ok(destinations.has("runtime/codex-bridge/manifest.json"));

console.log("Windows Codex ACP Bridge + Node installer runtime: ok");
