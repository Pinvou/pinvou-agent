const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const appRoot = path.resolve(__dirname, "..");
const tauriRoot = path.join(appRoot, "src-tauri");
const repoRoot = path.resolve(appRoot, "..");
const { linuxStartupWindowConfig } = require("../scripts/tauri/startup-window-config.js");

function readJson(relativePath) {
  return JSON.parse(fs.readFileSync(path.join(tauriRoot, relativePath), "utf8"));
}

function resourceSources(config) {
  return Object.keys(config.bundle?.resources || {});
}

function assertResourceSourcesExist(config, label) {
  for (const source of resourceSources(config)) {
    assert.ok(
      fs.existsSync(path.join(tauriRoot, source)),
      `${label} resource source must exist: ${source}`,
    );
  }
}

const common = readJson("tauri.conf.json");
const linux = readJson("config/platforms/linux/tauri.conf.json");
const macos = readJson("config/platforms/macos/tauri.conf.json");
const windows = readJson("config/platforms/windows/tauri.conf.json");

assert.equal(resourceSources(common).length, 0, "common bundle assets are embedded by the Rust runtime");
assert.match(common.build.beforeBuildCommand, /scripts\/tauri\/require-wrapper\.js build/);
assert.match(common.build.beforeBundleCommand, /scripts\/tauri\/require-wrapper\.js bundle/);
assert.ok(
  resourceSources(common).every((source) => source.startsWith("resources/common/")),
  "common Tauri config may only package resources/common",
);
assert.ok(
  resourceSources(linux).every((source) => source.startsWith("resources/platforms/linux/")),
  "Linux overlay may only package resources/platforms/linux",
);
assert.ok(
  resourceSources(macos).every((source) => source.startsWith("resources/platforms/macos/")),
  "macOS overlay may only package resources/platforms/macos",
);
assert.ok(
  resourceSources(windows).every((source) => source.startsWith("resources/platforms/windows/")),
  "Windows overlay may only package resources/platforms/windows",
);

assertResourceSourcesExist(common, "common");
assertResourceSourcesExist(linux, "Linux");
assertResourceSourcesExist(macos, "macOS");
assertResourceSourcesExist(windows, "Windows");

assert.equal(linux.app, undefined, "Linux packaging config must not duplicate base window fields");
const linuxStartupMain = { ...linuxStartupWindowConfig().app.windows[0] };
delete linuxStartupMain.visible;
linuxStartupMain.url = linuxStartupMain.url.replace("&startupWindow=hidden", "");
assert.deepEqual(
  linuxStartupMain,
  common.app.windows[0],
  "generated Linux startup config must inherit the complete base main window",
);

for (const legacyPath of ["resources/bundle", "resources/skill-marketplace", "resources/asr"]) {
  assert.equal(fs.existsSync(path.join(tauriRoot, legacyPath)), false, `legacy resource root must be removed: ${legacyPath}`);
}
const packagingPaths = [
  linux.bundle.linux.deb.desktopTemplate,
  linux.bundle.linux.deb.postInstallScript,
  linux.bundle.linux.deb.preRemoveScript,
  windows.bundle.windows.nsis.installerIcon,
];
for (const packagingPath of packagingPaths) {
  assert.ok(
    fs.existsSync(path.join(tauriRoot, packagingPath)),
    `Tauri packaging reference must exist: ${packagingPath}`,
  );
}

for (const legacyPath of [
  "packaging/linux/desktop/pinvou3.desktop",
  "packaging/linux/scripts/postinst.sh",
  "packaging/linux/scripts/prerm.sh",
  "packaging/windows/scripts/windows-runtime-submodule.ps1",
  "packaging/windows/scripts/prepare-windows-runtimes.ps1",
  "packaging/windows/scripts/stage-windows-nsis-resources.ps1",
  "packaging/windows/scripts/clean-nsis-staging.ps1",
  "packaging/windows/wosign/sign.ps1",
  "packaging/windows/main.wxs",
  "packaging/windows/python-node-path.wxs",
]) {
  assert.equal(fs.existsSync(path.join(tauriRoot, legacyPath)), false, `legacy packaging path must be removed: ${legacyPath}`);
}

const packageJson = JSON.parse(fs.readFileSync(path.join(appRoot, "package.json"), "utf8"));
assert.match(packageJson.scripts.tauri, /scripts\/tauri\/build\.js/);
for (const [name, command] of Object.entries(packageJson.scripts)) {
  assert.doesNotMatch(
    command,
    /(?:^|&&\s*)tauri\s+(?:build|bundle)\b/,
    `${name} must route Tauri build/bundle through scripts/tauri/build.js`,
  );
}
const runDev = fs.readFileSync(path.join(appRoot, "run-dev.sh"), "utf8");
assert.match(
  runDev,
  /exec npm run dev -- "\$@"/,
  "the direct dev entry must route through the wrapper that generates the startup overlay",
);

const gitmodules = fs.readFileSync(path.join(repoRoot, ".gitmodules"), "utf8");
assert.match(
  gitmodules,
  /\[submodule "private-runtimes\/windows"\][\s\S]*?update = none/,
  "private Windows runtime must be explicit and excluded from automatic submodule updates",
);
assert.doesNotMatch(
  JSON.stringify({ common, windows }),
  /private-runtimes\/windows|target\/windows-runtime/,
  "public Tauri configs must not hard-code private runtime sources",
);
const workflow = fs.readFileSync(path.join(repoRoot, ".github/workflows/pr-check.yml"), "utf8");
const connectorWorkflow = fs.readFileSync(
  path.join(repoRoot, ".github/workflows/connector-verify.yml"),
  "utf8",
);
const architectureGate = workflow.slice(
  workflow.indexOf("- name: 架构边界门禁"),
  workflow.indexOf("- name: 初始化公共底座 submodule"),
);
// merge_group 事件上 github.base_ref 不可靠,必须用 merge_group.base_ref 兜底。
// 契约锁住"两种事件都解析正确 base + 剥 refs/heads/ 前缀 + 传给守卫",防止改回 PR-only。
assert.match(
  architectureGate,
  /github\.event_name == 'merge_group' && github\.event\.merge_group\.base_ref \|\| github\.base_ref/,
);
assert.match(architectureGate, /base_ref="\$\{base_ref#refs\/heads\/\}"/);
assert.match(architectureGate, /architecture-guard\.py --base-ref "origin\/\$base_ref"/);

// fork 合规联动步骤用同款 base_ref 解析:锁住不被改回 PR-only,避免队列里取错基线。
const forkLinkGate = workflow.slice(
  workflow.indexOf("- name: fork 合规联动"),
  workflow.indexOf("- name: Setup Python"),
);
assert.match(
  forkLinkGate,
  /github\.event_name == 'merge_group' && github\.event\.merge_group\.base_ref \|\| github\.base_ref/,
);
assert.match(forkLinkGate, /base_ref="\$\{base_ref#refs\/heads\/\}"/);
assert.match(forkLinkGate, /ci-fork-link-check\.sh "origin\/\$base_ref"/);
const submoduleUpdates = workflow.match(/git submodule update[^\r\n]*/g) || [];
assert.ok(submoduleUpdates.length > 0, "CI must initialize the public CodeWhale submodule");
assert.ok(
  submoduleUpdates.every((command) => command.endsWith("-- CodeWhale")),
  "CI may only initialize the public engine submodule",
);
assert.match(workflow, /Merge Queue diff-selected browser smoke/);
assert.match(workflow, /github\.event\.merge_group\.base_sha/);
assert.match(workflow, /github\.event\.merge_group\.head_sha/);
assert.doesNotMatch(workflow, /npm run test:browser-smoke/);
assert.doesNotMatch(workflow, /frontend-test:[\s\S]{0,300}\n\s*if:\s*\$\{\{\s*false\s*\}\}/);
for (const stalePath of [
  "pinvou3-app/src-tauri/src/app/bridge",
  "pinvou3-app/src-tauri/src/features/assistant/harness.rs",
  "resources/common/bundle/connectors/linux-arm64",
]) {
  assert.equal(workflow.includes(stalePath), false, `PR workflow still references migrated path: ${stalePath}`);
  assert.equal(connectorWorkflow.includes(stalePath), false, `connector workflow still references migrated path: ${stalePath}`);
}
// l1 filter 已随 strict_mode 测试并入 lib 单测删除;这些 Rust 路径的 CI 触发
// 由 rust_code filter 的 **/*.rs 通配覆盖,不再逐路径断言。
assert.match(workflow, /src\/platform\/prefs\.rs/);
assert.match(connectorWorkflow, /resources\/platforms\/\*\*\/bundle\/connectors\/\*\*/);
for (const resources of ["linux/aarch64", "linux/x86_64", "macos/aarch64", "macos/x86_64", "windows/x86_64"]) {
  assert.ok(
    connectorWorkflow.includes(`resources: ${resources}`),
    `connector verify matrix must cover ${resources}`,
  );
}
assert.match(connectorWorkflow, /src\/platform\/paths\.rs/);

console.log("tauri platform layout contract: ok");
