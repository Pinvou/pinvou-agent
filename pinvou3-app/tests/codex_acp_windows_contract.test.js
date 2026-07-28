const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const appRoot = path.resolve(__dirname, "..");
const read = (...parts) =>
  fs.readFileSync(path.join(appRoot, ...parts), "utf8");

const capabilities = read(
  "src-tauri",
  "src",
  "platform",
  "capabilities.rs",
);
const codexAcp = read(
  "src-tauri",
  "src",
  "features",
  "codex_acp",
  "mod.rs",
);
const codexRuntime = read(
  "src-tauri",
  "src",
  "features",
  "codex_acp",
  "runtime.rs",
);
const buildScript = read("scripts", "tauri", "build.js");
const {
  BRIDGE_ENTRYPOINT,
  expectedMarker,
  isPrepared,
  WINDOWS_NODE_VERSION,
  windowsBridgeOverlay,
} = require("../scripts/tauri/codex-bridge.js");

assert.match(
  capabilities,
  /matches!\(os,\s*"linux"\s*\|\s*"windows"\)/,
  "Windows and Linux must advertise Codex ACP capability",
);
assert.match(
  codexAcp,
  /fn adapter_needs_path_node_lookup[\s\S]*?is_windows\s*\|\|[\s\S]*?Some\("js"\)/,
  "Windows adapters must validate Node even when the adapter is a command shim",
);
assert.match(
  codexAcp,
  /fn is_windows_cmd_for_platform[\s\S]*?Some\("cmd"\)/,
  "Windows command shims must be detected explicitly",
);
assert.match(
  codexAcp,
  /is_windows_cmd_for_platform\(adapter, is_windows\)[\s\S]*?Command::new\("cmd"\)[\s\S]*?\["\/D", "\/S", "\/C"\]/,
  "codex-acp.cmd must run through cmd /D /S /C",
);
assert.match(
  codexRuntime,
  /\("windows", "x86_64"\)[\s\S]*?x86_64-pc-windows-msvc/,
  "Windows x64 must have a managed Codex artifact",
);
assert.match(
  codexRuntime,
  /remove_existing_runtime_with_retry\(&target,\s*operation_id\)\.await/,
  "Windows managed runtime replacement must retry removal of a locked old runtime",
);
assert.equal(
  windowsBridgeOverlay().bundle.resources["target/windows-runtime/codex-bridge/"],
  "runtime/codex-bridge",
  "Windows packages must retain the prepared Codex ACP Bridge",
);
assert.equal(
  windowsBridgeOverlay().bundle.resources["target/windows-runtime/node/"],
  "runtime/node",
  "Windows packages must retain the pinned Node.js runtime",
);
assert.match(
  buildScript,
  /if \(isDev\)[\s\S]*?prepareWindowsCodexBridge\(\)/,
  "Windows development must prepare the same managed ACP Bridge",
);
assert.match(
  buildScript,
  /additionalConfigs\.push\(WINDOWS_BRIDGE_CONFIG_PATH\)/,
  "Windows packages must inject the generated Codex Bridge overlay",
);

const preparedRoot = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-codex-bridge-"));
try {
  const bridgeRoot = path.join(preparedRoot, "codex-bridge");
  const nodeRoot = path.join(preparedRoot, "node");
  const nodeExecutable = path.join(nodeRoot, "node.exe");
  const fakeNode = Buffer.from("pinned-node-runtime");
  const expected = {
    ...expectedMarker({ architecture: "x64" }),
    node_executable_sha256: crypto.createHash("sha256").update(fakeNode).digest("hex"),
  };
  const packageJsonPath = path.join(
    bridgeRoot,
    "acp",
    "node_modules",
    "@agentclientprotocol",
    "codex-acp",
    "package.json",
  );
  fs.mkdirSync(path.dirname(packageJsonPath), { recursive: true });
  fs.mkdirSync(path.dirname(path.join(bridgeRoot, BRIDGE_ENTRYPOINT)), {
    recursive: true,
  });
  fs.mkdirSync(nodeRoot, { recursive: true });
  fs.writeFileSync(path.join(bridgeRoot, "manifest.json"), JSON.stringify(expected));
  fs.writeFileSync(
    packageJsonPath,
    JSON.stringify({ version: expected.codex_acp_version }),
  );
  fs.writeFileSync(path.join(bridgeRoot, BRIDGE_ENTRYPOINT), "bridge");
  fs.writeFileSync(nodeExecutable, fakeNode);

  assert.equal(expected.node_version, WINDOWS_NODE_VERSION);
  assert.equal(isPrepared(expected, bridgeRoot, nodeRoot), true);
  fs.rmSync(nodeExecutable);
  assert.equal(
    isPrepared(expected, bridgeRoot, nodeRoot),
    false,
    "prepared runtime must be rejected when node.exe is absent",
  );
} finally {
  fs.rmSync(preparedRoot, { recursive: true, force: true });
}

console.log("✓ Windows Codex ACP packaging and command-shim contract passed");
