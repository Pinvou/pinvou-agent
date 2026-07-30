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
  CLAUDE_BRIDGE_ENTRYPOINT,
  expectedMarker,
  hideWindowsChildProcesses,
  isPrepared,
  WINDOWS_CLAUDE_EXECUTABLE,
  WINDOWS_NODE_VERSION,
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
  codexAcp,
  /fn command_version[\s\S]*?HiddenCommand::new/,
  "Windows Claude version probes must not open a console window",
);
assert.match(
  codexAcp,
  /fn cli_status_success[\s\S]*?HiddenCommand::new\("cmd"\)/,
  "Windows authentication probes must not open a console window",
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
  const claudePackageJsonPath = path.join(
    bridgeRoot,
    "acp",
    "node_modules",
    "@agentclientprotocol",
    "claude-agent-acp",
    "package.json",
  );
  fs.mkdirSync(path.dirname(packageJsonPath), { recursive: true });
  fs.mkdirSync(path.dirname(claudePackageJsonPath), { recursive: true });
  fs.mkdirSync(path.dirname(path.join(bridgeRoot, BRIDGE_ENTRYPOINT)), {
    recursive: true,
  });
  fs.mkdirSync(path.dirname(path.join(bridgeRoot, CLAUDE_BRIDGE_ENTRYPOINT)), {
    recursive: true,
  });
  fs.mkdirSync(path.dirname(path.join(bridgeRoot, WINDOWS_CLAUDE_EXECUTABLE)), {
    recursive: true,
  });
  fs.mkdirSync(nodeRoot, { recursive: true });
  fs.writeFileSync(path.join(bridgeRoot, "manifest.json"), JSON.stringify(expected));
  fs.writeFileSync(
    packageJsonPath,
    JSON.stringify({ version: expected.codex_acp_version }),
  );
  fs.writeFileSync(
    claudePackageJsonPath,
    JSON.stringify({ version: expected.claude_acp_version }),
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
  fs.writeFileSync(
    path.join(bridgeRoot, CLAUDE_BRIDGE_ENTRYPOINT),
    "console.log('claude-agent-acp');",
  );
  fs.writeFileSync(
    path.join(bridgeRoot, WINDOWS_CLAUDE_EXECUTABLE),
    "native-claude-runtime",
  );
  fs.writeFileSync(nodeExecutable, fakeNode);

  assert.equal(expected.node_version, WINDOWS_NODE_VERSION);
  assert.equal(isPrepared(expected, bridgeRoot, nodeRoot), true);
  const redundantCodexPackage = path.join(
    bridgeRoot,
    "acp",
    "node_modules",
    "@openai",
    "codex-win32-x64",
  );
  fs.mkdirSync(redundantCodexPackage, { recursive: true });
  assert.equal(
    isPrepared(expected, bridgeRoot, nodeRoot),
    false,
    "prepared runtime must reject a redundant bundled Codex platform package",
  );
  fs.rmSync(redundantCodexPackage, { recursive: true });
  fs.rmSync(path.join(bridgeRoot, WINDOWS_CLAUDE_EXECUTABLE));
  assert.equal(
    isPrepared(expected, bridgeRoot, nodeRoot),
    false,
    "prepared runtime must be rejected when claude.exe is absent",
  );
  fs.writeFileSync(
    path.join(bridgeRoot, WINDOWS_CLAUDE_EXECUTABLE),
    "native-claude-runtime",
  );
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
