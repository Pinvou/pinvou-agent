const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const appRoot = path.resolve(__dirname, "..");
const tauriRoot = path.join(appRoot, "src-tauri");
const repoRoot = path.resolve(appRoot, "..");

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
const windowsSigning = readJson("config/platforms/windows/signing.wosign.conf.json");

assert.ok(resourceSources(common).length > 0, "common Tauri config must declare shared resources");
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

for (const legacyPath of ["resources/bundle", "resources/skill-marketplace", "resources/web-template", "resources/asr"]) {
  assert.equal(fs.existsSync(path.join(tauriRoot, legacyPath)), false, `legacy resource root must be removed: ${legacyPath}`);
}

const packagingPaths = [
  linux.bundle.linux.deb.desktopTemplate,
  linux.bundle.linux.deb.postInstallScript,
  linux.bundle.linux.deb.preRemoveScript,
  windows.bundle.windows.wix.template,
  ...windows.bundle.windows.wix.fragmentPaths,
  windows.bundle.windows.nsis.template,
  windows.bundle.windows.nsis.installerHooks,
  windowsSigning.bundle.windows.signCommand.args[
    windowsSigning.bundle.windows.signCommand.args.indexOf("-File") + 1
  ],
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

const gitmodules = fs.readFileSync(path.join(repoRoot, ".gitmodules"), "utf8");
assert.match(
  gitmodules,
  /\[submodule "private-runtimes\/windows"\][\s\S]*?\n\s*update\s*=\s*none(?:\r?\n|$)/,
  "private Windows runtime must remain opt-in during recursive submodule updates",
);
const workflow = fs.readFileSync(path.join(repoRoot, ".github/workflows/pr-check.yml"), "utf8");
const submoduleUpdates = workflow.match(/git submodule update[^\r\n]*/g) || [];
assert.ok(submoduleUpdates.length > 0, "CI must initialize the public DeepSeek-TUI submodule");
assert.ok(
  submoduleUpdates.every((command) => command.endsWith("-- DeepSeek-TUI")),
  "Linux CI may only initialize DeepSeek-TUI, never the private Windows runtime",
);
assert.match(workflow, /npm run test:bridge-smoke/);
assert.doesNotMatch(workflow, /frontend-test:[\s\S]{0,300}\n\s*if:\s*\$\{\{\s*false\s*\}\}/);

console.log("tauri platform layout contract: ok");
