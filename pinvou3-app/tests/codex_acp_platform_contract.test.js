const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const appRoot = path.resolve(__dirname, "..");
const read = (...parts) => fs.readFileSync(path.join(appRoot, ...parts), "utf8");
const featureRoot = ["src-tauri", "src", "features", "codex_acp"];
const feature = read(...featureRoot, "mod.rs");
const runtime = read(...featureRoot, "runtime.rs");
const platform = read(...featureRoot, "platform", "mod.rs");
const windows = read(...featureRoot, "platform", "windows.rs");
const linux = read(...featureRoot, "platform", "linux.rs");
const macos = read(...featureRoot, "platform", "macos.rs");

for (const os of ["windows", "linux", "macos"]) {
  assert.match(
    platform,
    new RegExp(`#\\[cfg\\(target_os = "${os}"\\)\\][\\s\\S]*?${os} as current`),
    `${os} Codex behavior must be selected at compile time`,
  );
}

assert.ok(
  !runtime.includes("capabilities::is_windows()")
    && !runtime.includes("managed_artifact_for("),
  "shared runtime management must delegate OS behavior to the Codex platform adapter",
);
assert.match(runtime, /platform::managed_artifact\(std::env::consts::ARCH\)/);
assert.match(runtime, /platform::should_retry_file_lock\(&error\)/);

assert.match(windows, /SYSTEM_CODEX_NAME: &str = "codex\.cmd"/);
assert.match(windows, /MANAGED_CODEX_EXECUTABLE_NAME: &str = "codex\.exe"/);
assert.match(windows, /external_application_path\(adapter\)/);
assert.match(windows, /HiddenTokioCommand::new\("cmd"\)/);
assert.match(windows, /x86_64-pc-windows-msvc/);

assert.match(linux, /SYSTEM_CODEX_NAME: &str = "codex"/);
assert.match(linux, /x86_64-unknown-linux-musl/);
assert.match(linux, /aarch64-unknown-linux-musl/);
assert.ok(!linux.includes('Command::new("cmd")'));

assert.match(macos, /当前托管 Codex 下载不支持平台: macos-/);
assert.match(macos, /should_retry_file_lock\(_error: &io::Error\) -> bool \{\s*false/);
assert.match(feature, /run_brew\(&\["install", "--cask", "codex"\]\)/);
assert.match(feature, /run_brew\(&\["upgrade", "--cask", "codex"\]\)/);
assert.doesNotMatch(feature, /brew (?:install|upgrade) codex/);

console.log("✓ Codex ACP compile-time platform contract passed");
