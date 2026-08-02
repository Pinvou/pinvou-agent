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
const prepareBridge = read("scripts", "prepare-codex-bridge-runtime.sh");
const runDev = read("run-dev.sh");

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
assert.match(feature, /fn command_version[\s\S]*?HiddenCommand::new/);
assert.match(feature, /fn cli_status_success[\s\S]*?HiddenCommand::new\("cmd"\)/);
assert.match(feature, /"windows" => "win32"/);
assert.match(feature, /format!\("claude-agent-sdk-\{platform\}-\{arch\}\{libc\}"\)/);
assert.match(feature, /binary = if os == "windows"[\s\S]*?"claude\.exe"/);

assert.match(linux, /SYSTEM_CODEX_NAME: &str = "codex"/);
assert.match(linux, /x86_64-unknown-linux-musl/);
assert.match(linux, /aarch64-unknown-linux-musl/);
assert.ok(!linux.includes('Command::new("cmd")'));

assert.match(macos, /当前托管 Codex 下载不支持平台: macos-/);
assert.match(macos, /should_retry_file_lock\(_error: &io::Error\) -> bool \{\s*false/);
assert.match(macos, /"aarch64" => "darwin-arm64"/);
assert.match(macos, /join\("darwin-x64"\)|_ => "darwin-x64"/);
assert.doesNotMatch(prepareBridge, /--os=linux/);
assert.match(prepareBridge, /NODE_TARGETS=\("darwin-arm64" "darwin-x64"\)/);
assert.match(prepareBridge, /npm_ci_for_target "\$ACP_ROOT" "\$NODE_OS" "\$NODE_CPU"/);
assert.doesNotMatch(prepareBridge, /ACP_X64_ROOT/);
// Claude Code 与 Codex/Kimi 一致走系统安装，Bridge 不得携带 claude 平台原生二进制。
assert.match(prepareBridge, /claude-agent-sdk-\{darwin,linux,win32\}-\*/);
assert.match(prepareBridge, /Bridge 中仍残留 Claude 平台二进制，拒绝打包/);
// 旧 staging 残留 claude 平台包时必须判为无效并重打包，防止本地复用旧产物
// 把原生二进制静默打回安装包。
assert.match(prepareBridge, /旧 staging 可能仍残留 Claude 平台原生二进制/);
assert.match(
  runDev,
  /if \[ "\$OS_NAME" = "Linux" \] \|\| \[ "\$OS_NAME" = "Darwin" \]; then\s+\.\/scripts\/prepare-codex-bridge-runtime\.sh\s+fi/,
  "dev startup must delegate the complete ACP Bridge readiness check to the preparation script",
);
assert.doesNotMatch(
  runDev,
  /BRIDGE_(?:NODE|ENTRY)=/,
  "dev startup must not duplicate a partial Bridge readiness check",
);
assert.match(feature, /vec!\["install", "--cask", "codex"\]/);
assert.match(feature, /vec!\["upgrade", "--cask", "codex"\]/);
// brew 升级已泛化到三个 Agent：claude-code 是 cask，kimi-code 是 formula（无 --cask）。
assert.match(feature, /vec!\["upgrade", "--cask", "claude-code"\]/);
assert.match(feature, /vec!\["upgrade", "kimi-code"\]/);
assert.match(feature, /&\["list", "--cask", "codex"\]/);
assert.match(feature, /&\["list", "--cask", "claude-code"\]/);
assert.match(feature, /&\["list", "kimi-code"\]/);
assert.doesNotMatch(feature, /brew (?:install|upgrade) codex/);
// npm 全局来源探测与升级：Windows 用 npm.cmd，包名固定，升级参数 install -g <pkg>@latest。
assert.match(feature, /find_in_path\("npm\.cmd"\)/);
assert.match(feature, /@openai\/codex/);
assert.match(feature, /@anthropic-ai\/claude-code/);
assert.match(feature, /@moonshot-ai\/kimi-code/);
assert.match(feature, /"ls", "-g", package, "--depth=0"/);
assert.match(feature, /format!\("\{\}@latest", npm_package\(backend\)\?\)/);
assert.match(feature, /"npm_upgrade" => self\.upgrade_via_npm\(backend\)/);

console.log("✓ Codex ACP compile-time platform contract passed");
