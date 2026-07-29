const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const {
  BRIDGE_ENTRYPOINT,
  expectedMarker,
  prepareWindowsCodexBridge,
  WINDOWS_BRIDGE_CONFIG_PATH,
  WINDOWS_BRIDGE_ROOT,
} = require("../scripts/tauri/codex-bridge.js");
const {
  buildResourceManifest,
  composeEffectiveConfig,
} = require("../scripts/tauri/effective-config.js");
const { platformConfigPath } = require("../scripts/tauri/platform-config.js");
const {
  WINDOWS_RUNTIME_CONFIG_PATH,
  describeWindowsRuntime,
} = require("../scripts/tauri/windows-runtime.js");

assert.equal(process.platform, "win32", "此冒烟测试必须在 Windows 原生 runner 执行");
assert.equal(process.arch, "x64", "Windows Codex Runtime 当前只支持 x64");

const stagedRuntime = fs.existsSync(WINDOWS_RUNTIME_CONFIG_PATH)
  ? describeWindowsRuntime()
  : null;
const nodeExecutable = stagedRuntime?.nodeExecutable ?? process.execPath;
prepareWindowsCodexBridge({
  nodeExecutable,
  npmExecPath: stagedRuntime?.npmExecPath,
});

const marker = expectedMarker({ architecture: "x64" });
const sourcePackage = JSON.parse(
  fs.readFileSync(
    path.join(__dirname, "..", "scripts", "codex-bridge-runtime", "package.json"),
    "utf8",
  ),
);
const bridgeVersion = spawnSync(
  nodeExecutable,
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
  `@agentclientprotocol/codex-acp ${sourcePackage.dependencies["@agentclientprotocol/codex-acp"]}`,
);

const { effectiveConfig } = composeEffectiveConfig([
  platformConfigPath("win32"),
  ...(stagedRuntime ? [stagedRuntime.configPath] : []),
  WINDOWS_BRIDGE_CONFIG_PATH,
]);
const manifest = buildResourceManifest(effectiveConfig, { platform: "win32" });
const destinations = new Set(manifest.files.map((file) => file.destination));
assert.ok(
  destinations.has(
    `runtime/codex-bridge/${BRIDGE_ENTRYPOINT.replaceAll(path.sep, "/")}`,
  ),
);
assert.ok(destinations.has("runtime/codex-bridge/manifest.json"));
if (stagedRuntime) {
  assert.ok(destinations.has("runtime/node/node.exe"));
  assert.ok(destinations.has("runtime/node/LICENSE"));
} else {
  assert.ok(
    [...destinations].every((destination) => !destination.startsWith("runtime/node/")),
    "Codex Bridge overlay must not package a second Node runtime",
  );
}

assert.equal(marker.platform, "win32");
console.log("Windows Codex ACP Bridge existing-Node runtime: ok");
