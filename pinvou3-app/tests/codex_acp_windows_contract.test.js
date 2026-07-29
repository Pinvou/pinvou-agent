const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
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
const codexAcpPlatform = read(
  "src-tauri",
  "src",
  "features",
  "codex_acp",
  "platform",
  "mod.rs",
);
const codexAcpWindows = read(
  "src-tauri",
  "src",
  "features",
  "codex_acp",
  "platform",
  "windows.rs",
);
const windowsPath = read(
  "src-tauri",
  "src",
  "platform",
  "os",
  "windows",
  "windows_path.rs",
);
const codexRuntime = read(
  "src-tauri",
  "src",
  "features",
  "codex_acp",
  "runtime.rs",
);
const buildScript = read("scripts", "tauri", "build.js");
const bridgeBuildScript = read("scripts", "tauri", "codex-bridge.js");
const {
  BRIDGE_ENTRYPOINT,
  expectedMarker,
  hideWindowsChildProcesses,
  isPrepared,
  validateNodeRuntime,
  windowsBridgeOverlay,
} = require("../scripts/tauri/codex-bridge.js");

assert.match(
  capabilities,
  /matches!\(os,\s*"linux"\s*\|\s*"windows"\s*\|\s*"macos"\)/,
  "Windows、Linux 和 macOS must advertise Codex ACP capability",
);
assert.match(
  codexAcpWindows,
  /fn adapter_needs_node\(_adapter: &Path\) -> bool \{\s*true\s*\}/,
  "Windows adapters must validate Node even when the adapter is a command shim",
);
assert.match(
  codexAcpWindows,
  /adapter\.extension\(\)[\s\S]*?Some\("cmd"\)/,
  "Windows command shims must be detected explicitly",
);
assert.match(
  codexAcpWindows,
  /HiddenTokioCommand::new\("cmd"\)[\s\S]*?\["\/D", "\/S", "\/C"\]/,
  "codex-acp.cmd must run through cmd /D /S /C",
);
assert.match(
  codexAcpWindows,
  /HiddenTokioCommand::new\(crate::platform::os::external_application_path\(node\)\)/,
  "the installed Node Bridge must start without a visible Windows console",
);
assert.match(
  codexAcpWindows,
  /"x86_64"[\s\S]*?x86_64-pc-windows-msvc/,
  "Windows x64 must have a managed Codex artifact",
);
assert.match(
  windowsPath,
  /fn platform_compat_path[\s\S]*?strip_prefix\(r"\\\\\?\\UNC\\"\)[\s\S]*?strip_prefix\(r"\\\\\?\\"\)/,
  "Windows OS paths must remove verbatim prefixes before external-process launch",
);
assert.match(
  codexAcpWindows,
  /HiddenTokioCommand::new\(crate::platform::os::external_application_path\(node\)\)[\s\S]*?command\.arg\(adapter\)/,
  "bundled Node and the JavaScript Bridge must receive normalized installed paths",
);
assert.match(
  codexAcpPlatform,
  /#\[cfg\(target_os = "windows"\)\][\s\S]*?use windows as current/,
  "Codex platform behavior must be selected at compile time",
);
assert.match(
  codexAcp,
  /"session:bridge_stderr"[\s\S]*?"session:initialize_failed"[\s\S]*?exit_status=/,
  "Bridge stderr and exit status must remain available in persistent ACP diagnostics",
);
assert.match(
  bridgeBuildScript,
  /windowsHide: true[\s\S]*?hideWindowsChildProcesses/,
  "the packaged ACP Bridge must hide the Codex CLI process it starts on Windows",
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
  undefined,
  "Codex Bridge must not own or duplicate the shared Windows Node runtime",
);
assert.doesNotMatch(
  bridgeBuildScript,
  /WINDOWS_NODE_VERSION|nodejs\.org\/dist|curl\.exe|tar\.exe/,
  "Codex Bridge must reuse an existing Node instead of downloading one",
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
  const expected = expectedMarker({ architecture: "x64" });
  const sourcePackage = JSON.parse(
    read("scripts", "codex-bridge-runtime", "package.json"),
  );
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
  fs.writeFileSync(path.join(bridgeRoot, "manifest.json"), JSON.stringify(expected));
  fs.writeFileSync(
    packageJsonPath,
    JSON.stringify({ version: sourcePackage.dependencies["@agentclientprotocol/codex-acp"] }),
  );
  const bridgeEntrypoint = path.join(bridgeRoot, BRIDGE_ENTRYPOINT);
  fs.writeFileSync(
    bridgeEntrypoint,
    [
      'spawn(`"${codexPath}" app-server`, { shell: true, env: spawnEnv })',
      'spawn(process.execPath, [bundledCodexPath, "app-server"], { env: spawnEnv })',
    ].join("\n"),
  );
  hideWindowsChildProcesses(bridgeEntrypoint);

  assert.deepEqual(Object.keys(expected), [
    "schema_version",
    "platform",
    "arch",
    "package_json_sha256",
    "lockfile_sha256",
  ]);
  assert.equal(isPrepared(expected, bridgeRoot), true);
  fs.rmSync(bridgeEntrypoint);
  assert.equal(
    isPrepared(expected, bridgeRoot),
    false,
    "prepared Bridge must be rejected when its entrypoint is absent",
  );
} finally {
  fs.rmSync(preparedRoot, { recursive: true, force: true });
}

assert.match(
  validateNodeRuntime(process.execPath, {
    environment: process.env,
    spawn: spawnSync,
  }),
  /^v\d+\./,
  "Bridge preparation must validate and reuse the supplied Node runtime",
);

console.log("✓ Windows Codex ACP packaging and command-shim contract passed");
