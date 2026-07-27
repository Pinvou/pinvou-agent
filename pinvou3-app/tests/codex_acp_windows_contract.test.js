const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const appRoot = path.resolve(__dirname, "..");
const runtime = fs.readFileSync(
  path.join(appRoot, "src-tauri", "src", "features", "codex_acp", "mod.rs"),
  "utf8",
);
const capabilities = fs.readFileSync(
  path.join(appRoot, "src-tauri", "src", "platform", "capabilities.rs"),
  "utf8",
);
const build = fs.readFileSync(path.join(appRoot, "scripts", "tauri", "build.js"), "utf8");
const windowsConfig = JSON.parse(
  fs.readFileSync(
    path.join(appRoot, "src-tauri", "config", "platforms", "windows", "tauri.conf.json"),
    "utf8",
  ),
);

assert.match(
  capabilities,
  /codex_acp_supported:\s*cfg!\(any\(target_os = "linux", target_os = "windows"\)\)/,
  "Codex ACP must be advertised on Linux and Windows only",
);

assert.match(
  runtime,
  /crate::platform::capabilities::is_windows\(\)\s*\|\|\s*adapter\.extension\(\)/,
  "Windows ACP status must require node.exe even when the adapter is a .cmd shim",
);

assert.ok(
  runtime.includes("fn is_windows_cmd(path: &Path) -> bool")
    && runtime.includes("} else if is_windows_cmd(adapter) {")
    && runtime.includes('command.args(["/D", "/S", "/C"]).arg(adapter);'),
  "Windows codex-acp.cmd adapters must be launched through cmd /D /S /C",
);

assert.ok(
  runtime.includes("fn codex_login_command(codex: &Path) -> Command")
    && runtime.includes("if is_windows_cmd(codex)")
    && runtime.includes('command.args(["/D", "/S", "/C"]).arg(codex).arg("login");'),
  "Windows codex.cmd login must keep using cmd /D /S /C",
);

assert.equal(
  build.includes('if (platform !== "linux") return;'),
  true,
  "packaged Codex Bridge preparation is still Linux-only until Windows runtime assets exist",
);

assert.deepEqual(
  Object.keys(windowsConfig.bundle.resources || {}),
  [],
  "Windows must not package a missing codex-bridge resource mapping",
);

console.log("codex_acp_windows_contract: ok");
