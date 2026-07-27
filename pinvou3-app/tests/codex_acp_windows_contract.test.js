const assert = require("node:assert/strict");
const fs = require("node:fs");
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
const { windowsBridgeOverlay } = require("../scripts/tauri/codex-bridge.js");

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
assert.equal(
  windowsBridgeOverlay().bundle.resources["target/windows-runtime/codex-bridge/"],
  "runtime/codex-bridge",
  "Windows packages must retain the prepared Codex ACP Bridge",
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

console.log("✓ Windows Codex ACP packaging and command-shim contract passed");
