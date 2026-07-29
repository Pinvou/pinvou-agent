const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const { windowsBundleTargets } = require("../scripts/tauri/build.js");
const { stageWindowsInstaller } = require("../scripts/tauri/windows-installer.js");

const appRoot = path.resolve(__dirname, "..");
const repoRoot = path.resolve(appRoot, "..");
const readRepo = (...parts) =>
  fs.readFileSync(path.join(repoRoot, ...parts), "utf8");
const readApp = (...parts) => fs.readFileSync(path.join(appRoot, ...parts), "utf8");

const gitmodules = readRepo(".gitmodules");
const lock = JSON.parse(
  readApp(
    "src-tauri",
    "config",
    "platforms",
    "windows",
    "runtime",
    "x86_64.lock.json",
  ),
);
const windowsConfig = JSON.parse(
  readApp("src-tauri", "config", "platforms", "windows", "tauri.conf.json"),
);
const initScript = readApp(
  "src-tauri",
  "packaging",
  "windows",
  "runtime",
  "scripts",
  "init-submodule.ps1",
);
const runtimeScript = readApp(
  "src-tauri",
  "packaging",
  "windows",
  "runtime",
  "scripts",
  "resolve-runtime.ps1",
);
const installerHook = readApp(
  "src-tauri",
  "packaging",
  "windows",
  "nsis",
  "installer-hooks.nsh",
);
const runtimeWrapper = readApp("scripts", "tauri", "windows-runtime.js");
const installerAdapter = readApp("scripts", "tauri", "windows-installer.js");
const buildScript = readApp("scripts", "tauri", "build.js");
const bridgeScript = readApp("scripts", "tauri", "codex-bridge.js");
const releaseWorkflow = readRepo(".github", "workflows", "release-packages.yml");

assert.match(
  gitmodules,
  /\[submodule "private-runtimes\/windows"\][\s\S]*?url = https:\/\/github\.com\/Pinvou\/pinvou3-windows-runtime\.git[\s\S]*?update = none/,
  "private Windows runtime must be pinned as a non-automatic submodule",
);
assert.equal(lock.schemaVersion, 2);
assert.equal(lock.target, "windows-x86_64");
assert.equal(lock.source.type, "git-submodule");
assert.equal(lock.source.path, "private-runtimes/windows");
assert.equal(lock.source.url, "https://github.com/Pinvou/pinvou3-windows-runtime.git");
assert.match(lock.source.commit, /^[0-9a-f]{40}$/u);
assert.match(lock.manifest.sha256, /^[0-9a-f]{64}$/u);

const gitlink = spawnSync(
  "git",
  ["ls-files", "--stage", "--", lock.source.path],
  { cwd: repoRoot, encoding: "utf8" },
);
assert.equal(gitlink.error, undefined);
assert.equal(gitlink.status, 0, gitlink.stderr);
assert.match(
  gitlink.stdout,
  new RegExp(`^160000 ${lock.source.commit} 0\\t${lock.source.path.replace("/", "\\/")}`),
  "superproject gitlink must match the runtime lock",
);

for (const contract of [
  "Get-SuperprojectGitlinkCommit",
  "Windows runtime submodule commit mismatch",
  "manifest SHA-256",
  "Test-LfsPointer",
  "Test-ManagedArchiveExpansion",
  "System.IO.Compression.ZipFile",
  "Write-Utf8WithoutBom",
  "Test-StageInventory",
  ".verified-stage.json",
  "Get-RuntimeDescriptorContent",
  'delivery = "download-on-first-use"',
]) {
  assert.ok(runtimeScript.includes(contract), `runtime staging must retain ${contract}`);
}
assert.match(runtimeScript, /Get-Sha256 -Path \$sourcePath/u);
assert.match(runtimeScript, /HashSet\[string\]/u);
assert.match(runtimeScript, /Remove-Item -LiteralPath \$bundledAsrModelPath/u);
assert.match(runtimeScript, /Remove-Item -LiteralPath \$stageContext\.PayloadRoot/u);
for (const destination of [
  "runtime/poppler",
  "runtime/pandoc",
  "runtime/tesseract",
  "runtime/python",
  "runtime/node",
  "runtime/onnxruntime",
  "runtime/asr",
  "runtime/7zip",
]) {
  assert.ok(runtimeScript.includes(destination), `runtime overlay must include ${destination}`);
}
assert.doesNotMatch(
  runtimeScript,
  /sensevoice-small-q8\.gguf"\s*=\s*"runtime\/asr/u,
  "download-on-first-use ASR model must not be mapped into the installer",
);
assert.doesNotMatch(runtimeScript, /Stage-NsisBootstrapper/u);
assert.match(initScript, /git lfs pull/);
assert.match(initScript, /--include=/);
assert.match(initScript, /pinvou3-windows-runtime-\$expectedCommit/);

assert.match(runtimeWrapper, /runtime-descriptor\.json/);
assert.match(runtimeWrapper, /cleanupLegacyWindowsNodeStaging/);
assert.match(runtimeWrapper, /download-on-first-use/);
assert.match(installerAdapter, /bundleTargets\.includes\("nsis"\)/);
assert.equal(
  windowsConfig.bundle.windows.nsis.installerHooks,
  "packaging/windows/nsis/installer-hooks.nsh",
);
assert.match(installerHook, /!macro NSIS_HOOK_PREINSTALL/);
assert.match(installerHook, /VC_redist\.x64\.exe/);
assert.match(installerHook, /\.\.\\\.\.\\\.\.\\windows-runtime\\nsis\\vc_redist/);
assert.doesNotMatch(installerHook, /\.\.\\\.\.\\\.\.\\target\\windows-runtime/);
assert.match(installerHook, /\/install \/quiet \/norestart/);
assert.match(installerHook, /IntCmp \$1 3010/);
assert.match(installerHook, /IntCmp \$1 1641/);

assert.deepEqual(windowsBundleTargets(["build"]), ["msi", "nsis"]);
assert.deepEqual(windowsBundleTargets(["build", "--bundles", "msi"]), ["msi"]);
assert.deepEqual(windowsBundleTargets(["build", "--bundles=nsis"]), ["nsis"]);
assert.ok(
  buildScript.indexOf("stageWindowsRuntime()") <
    buildScript.indexOf("stageWindowsInstaller({"),
  "runtime resolver must run before the installer adapter",
);
assert.ok(
  buildScript.indexOf("stageWindowsInstaller({") <
    buildScript.indexOf("prepareWindowsCodexBridge(windowsBridgeOptions)"),
  "installer resources must be staged before the Bridge and Tauri build",
);
assert.doesNotMatch(
  bridgeScript,
  /WINDOWS_NODE_VERSION|nodejs\.org\/dist|curl\.exe|tar\.exe|runtime\/codex-node/,
  "Codex Bridge must not download or package a private Node copy",
);

const temporaryRoot = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-nsis-adapter-"));
try {
  const sourcePath = path.join(temporaryRoot, "source", "VC_redist.x64.exe");
  const destinationRoot = path.join(temporaryRoot, "output", "nsis");
  fs.mkdirSync(path.dirname(sourcePath), { recursive: true });
  fs.writeFileSync(sourcePath, "locked-vc-runtime");
  const runtime = {
    vcRedist: {
      sourcePath,
      bytes: fs.statSync(sourcePath).size,
      sha256: crypto.createHash("sha256").update(fs.readFileSync(sourcePath)).digest("hex"),
    },
  };
  assert.equal(
    stageWindowsInstaller({
      platform: "win32",
      bundleTargets: ["msi"],
      runtime,
      destinationRoot,
    }),
    null,
  );
  assert.equal(fs.existsSync(destinationRoot), false);
  const staged = stageWindowsInstaller({
    platform: "win32",
    bundleTargets: ["nsis"],
    runtime,
    destinationRoot,
  });
  assert.equal(fs.readFileSync(staged.vcRedistPath, "utf8"), "locked-vc-runtime");
  fs.writeFileSync(sourcePath, "tampered-vc-runtime");
  assert.throws(
    () =>
      stageWindowsInstaller({
        platform: "win32",
        bundleTargets: ["nsis"],
        runtime,
        destinationRoot,
      }),
    /指纹不匹配/u,
  );
} finally {
  fs.rmSync(temporaryRoot, { recursive: true, force: true });
}

assert.match(releaseWorkflow, /PINVOU3_WINDOWS_RUNTIME_TOKEN/);
assert.match(releaseWorkflow, /正式 Windows 发布缺少 PINVOU3_WINDOWS_RUNTIME_TOKEN/);
assert.match(releaseWorkflow, /npm run runtime:windows:init/);
assert.match(releaseWorkflow, /npm run test:windows-runtime/);
assert.match(releaseWorkflow, /submodules: false/);

console.log("Windows private runtime packaging contract: ok");
