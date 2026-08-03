const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const appRoot = path.resolve(__dirname, "..");
const read = (...parts) => fs.readFileSync(path.join(appRoot, ...parts), "utf8");
const featureRoot = ["src-tauri", "src", "features", "codex_acp"];
const feature = read(...featureRoot, "mod.rs");
const runtime = read(...featureRoot, "runtime.rs");
const latest = read(...featureRoot, "latest.rs");
const platform = read(...featureRoot, "platform", "mod.rs");
const windows = read(...featureRoot, "platform", "windows.rs");
const linux = read(...featureRoot, "platform", "linux.rs");
const macos = read(...featureRoot, "platform", "macos.rs");
const processRuntime = read("src-tauri", "src", "platform", "process.rs");
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
  "shared Codex probing must remain platform-neutral",
);
assert.doesNotMatch(runtime, /Managed|managed_codex|registry\.npmjs|registry\.npmmirror/);
// latest 只查询三家官方安装器实际使用的固定 HTTPS 来源；不查询 npm registry，
// 也不把厂商响应当下载地址执行。
assert.match(latest, /https:\/\/releases\.openai\.com\/codex\/channels\/latest/);
assert.match(latest, /https:\/\/github\.com\/openai\/codex\/releases\/latest/);
assert.match(latest, /https:\/\/downloads\.claude\.ai\/claude-code-releases\/latest/);
assert.match(latest, /https:\/\/code\.kimi\.com\/kimi-code\/latest/);
assert.match(latest, /const CACHE_TTL: Duration = Duration::from_secs\(5 \* 60\)/);
assert.match(latest, /const MAX_RESPONSE_BYTES: usize = 128 \* 1024/);
assert.doesNotMatch(latest, /registry\.npmjs|registry\.npmmirror|api\.github\.com/);
assert.doesNotMatch(
  latest,
  /status\.installed = false/,
  'official latest detection must not make a minimum-compatible CLI unavailable',
);
assert.match(latest, /status\.update_available = true/);
assert.match(
  latest,
  /if status\.codex_available \{[\s\S]*?latest_version_probe[\s\S]*?refresh\(backend, false\)\.await;[\s\S]*?\}/,
  "missing or below-minimum CLIs must not wait for an unnecessary latest request",
);

assert.match(windows, /SYSTEM_CODEX_NAME: &str = "codex\.cmd"/);
assert.match(windows, /external_application_path\(adapter\)/);
assert.match(windows, /HiddenTokioCommand::new\("cmd"\)/);
assert.match(feature, /fn command_version[\s\S]*?external_command/);
assert.match(feature, /fn cli_status_success[\s\S]*?external_command/);
assert.match(processRuntime, /fn external_command_for[\s\S]*?HiddenCommand::new\("cmd"\)/);
assert.match(feature, /"windows" => "win32"/);
assert.match(feature, /format!\("claude-agent-sdk-\{platform\}-\{arch\}\{libc\}"\)/);
assert.match(feature, /binary = if os == "windows"[\s\S]*?"claude\.exe"/);

assert.match(linux, /SYSTEM_CODEX_NAME: &str = "codex"/);
assert.ok(!linux.includes('Command::new("cmd")'));

assert.match(macos, /"aarch64" => "darwin-arm64"/);
assert.match(macos, /join\("darwin-x64"\)|_ => "darwin-x64"/);
assert.doesNotMatch(prepareBridge, /--os=linux/);
assert.match(prepareBridge, /NODE_TARGETS=\("darwin-arm64" "darwin-x64"\)/);
assert.match(prepareBridge, /npm_ci_for_target "\$ACP_ROOT" "\$NODE_OS" "\$NODE_CPU"/);
assert.match(prepareBridge, /node\/lib\/node_modules\/npm\/bin\/npm-cli\.js/);
assert.match(prepareBridge, /cp -R \$DD "\$NODE_DIST_ROOT\/lib\/node_modules\/npm"/);
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
// 三 Agent 未安装均走官方脚本；Codex 官方脚本默认 latest，且安装后从 ~/.local/bin
// 直接解析绝对路径，不依赖桌面进程启动时继承的 PATH。
assert.match(feature, /CODEX_INSTALL_SCRIPT_UNIX: &str = "https:\/\/chatgpt\.com\/codex\/install\.sh"/);
assert.match(feature, /CODEX_INSTALL_SCRIPT_WINDOWS: &str = "https:\/\/chatgpt\.com\/codex\/install\.ps1"/);
assert.match(feature, /AgentBackend::CodexAcp => \(CODEX_INSTALL_SCRIPT_UNIX, CODEX_INSTALL_SCRIPT_WINDOWS\)/);
assert.match(feature, /command\.env\("CODEX_NON_INTERACTIVE", "1"\)/);
assert.match(feature, /fn resolve_codex_cli\([\s\S]*?platform::codex_official_install_path\(\)/);
assert.doesNotMatch(feature, /managed_download|MANAGED_CODEX_VERSION|install_managed_codex/);
assert.doesNotMatch(
  `${windows}\n${linux}\n${macos}`,
  /managed_artifact|should_retry_file_lock|registry\.npmjs|registry\.npmmirror/,
);

// Kimi 不经过独立 Bridge；CLI 缺失时必须继续进入 installed=false 的安装分支，
// 不能被前端 !bridge_ready 的错误提示提前截断。
const kimiStatus = feature.match(
  /if backend == AgentBackend::KimiAcp \{([\s\S]*?)\n        \}\n\n        let \(agent_id/,
);
assert.ok(kimiStatus, "Kimi status branch must remain explicit");
assert.match(kimiStatus[1], /bridge_ready: true/);
assert.match(kimiStatus[1], /install_action: if installed/);

console.log("✓ Codex ACP compile-time platform contract passed");
