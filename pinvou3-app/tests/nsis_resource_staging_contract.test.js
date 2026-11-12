const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const { composeEffectiveConfig } = require("../scripts/tauri/effective-config.js");
const {
  configSpecs,
  prepareTauriArgs,
} = require("../scripts/tauri/build.js");
const {
  WINDOWS_BRIDGE_CONFIG_PATH,
  windowsBridgeOverlay,
} = require("../scripts/tauri/codex-bridge.js");

const appRoot = path.resolve(__dirname, "..");
const read = (...segments) => fs.readFileSync(path.join(appRoot, ...segments), "utf8");

const stageScript = read(
  "src-tauri",
  "packaging",
  "windows",
  "nsis",
  "scripts",
  "stage-resources.ps1",
);
const installerTemplate = read(
  "src-tauri",
  "packaging",
  "windows",
  "nsis",
  "installer-template.nsi",
);
const baseInstallerHooks = read(
  "src-tauri",
  "packaging",
  "windows",
  "nsis",
  "installer-hooks.nsh",
);
const rustToolchainGuard = read(
  "scripts",
  "ci",
  "ensure-rust-toolchain.ps1",
);
const rustupRepairSmoke = read("tests", "windows_rustup_repair_smoke.ps1");
const buildScript = read("scripts", "tauri", "build.js");
const signScriptPath = path.join(
  appRoot,
  "src-tauri",
  "packaging",
  "windows",
  "signing",
  "wosign",
  "sign.ps1",
);
// WoSign signing is a closed-only machine-local overlay: assert its contract
// only where the file exists (the open checkout does not carry sign.ps1).
const signScript = fs.existsSync(signScriptPath)
  ? fs.readFileSync(signScriptPath, "utf8")
  : null;
const localNsisBuildScriptPath = path.join(
  appRoot,
  "scripts",
  "tauri",
  "build-local-nsis.ps1",
);
const localNsisBuildScript = fs.readFileSync(localNsisBuildScriptPath, "utf8");
const packageJson = JSON.parse(read("package.json"));

assert.doesNotMatch(
  stageScript,
  /\$resourceOverrides\["target\/release\/nsis-stage\/resources\/"\]/u,
  "pre-signed staged resources must not be registered as Tauri bundle.resources",
);
assert.match(
  stageScript,
  /installerHooks = "target\/release\/nsis-stage\/installer-hooks\.nsh"/u,
);
assert.match(stageScript, /!macro PINVOU_INSTALL_STAGED_RESOURCES/u);
assert.match(stageScript, /PINVOU_STAGED_RESOURCES_SIZE_KIB/u);
assert.match(stageScript, /File \/a \/r/u);
assert.match(stageScript, /!macro PINVOU_UNINSTALL_STAGED_RESOURCES/u);
assert.match(stageScript, /function Get-StagedRelativeDirectories/u);
assert.match(stageScript, /Delete \/REBOOTOK/u);
assert.match(stageScript, /RMDir \/REBOOTOK/u);
assert.doesNotMatch(stageScript, /RMDir \/r/u);
assert.match(stageScript, /NSIS staging installer hooks changed after collection/u);
assert.match(stageScript, /schemaVersion = 2/u);
assert.match(
  stageScript,
  /NSIS staging config must only remove the original Tauri resource mappings/u,
);

assert.match(installerTemplate, /!insertmacro PINVOU_INSTALL_STAGED_RESOURCES/u);
assert.match(installerTemplate, /!insertmacro PINVOU_UNINSTALL_STAGED_RESOURCES/u);
assert.match(
  installerTemplate,
  /!define \/math ESTIMATEDSIZE \{\{estimated_size\}\} \+ \$\{PINVOU_STAGED_RESOURCES_SIZE_KIB\}/u,
);
assert.doesNotMatch(baseInstallerHooks, /PINVOU_(?:UN)?INSTALL_STAGED_RESOURCES/u);

const installSectionOffset = installerTemplate.indexOf("Section Install");
const installProcessCheckOffset = installerTemplate.indexOf(
  "CheckIfAppIsRunning",
  installSectionOffset,
);
const stagedInstallOffset = installerTemplate.indexOf(
  "!insertmacro PINVOU_INSTALL_STAGED_RESOURCES",
  installSectionOffset,
);
assert.ok(
  installProcessCheckOffset >= 0 && stagedInstallOffset > installProcessCheckOffset,
  "staged resources must be copied after the running-app check",
);

const uninstallSectionOffset = installerTemplate.indexOf("Section Uninstall");
const uninstallProcessCheckOffset = installerTemplate.indexOf(
  "CheckIfAppIsRunning",
  uninstallSectionOffset,
);
const stagedUninstallOffset = installerTemplate.indexOf(
  "!insertmacro PINVOU_UNINSTALL_STAGED_RESOURCES",
  uninstallSectionOffset,
);
assert.ok(
  uninstallProcessCheckOffset >= 0 && stagedUninstallOffset > uninstallProcessCheckOffset,
  "staged resources must be removed after the running-app check",
);

assert.match(packageJson.scripts["build:nsis"], /npm run build:nsis:stage/u);
assert.match(packageJson.scripts["build:nsis"], /npm run bundle:nsis/u);
assert.match(packageJson.scripts["build:nsis:local"], /build-local-nsis\.ps1/u);
assert.match(packageJson.scripts["build:nsis:stage:local"], /--no-bundle/u);
assert.doesNotMatch(packageJson.scripts["build:nsis:stage:local"], /--features/u);
assert.doesNotMatch(packageJson.scripts["build:nsis:stage:local"], /check:wosign/u);
assert.match(packageJson.scripts["bundle:nsis:local"], /--bundles nsis/u);
assert.match(packageJson.scripts["bundle:nsis:local"], /tauri\.nsis-stage\.conf\.json/u);
assert.doesNotMatch(packageJson.scripts["bundle:nsis:local"], /--features/u);
assert.doesNotMatch(packageJson.scripts["bundle:nsis:local"], /check:wosign/u);
assert.doesNotMatch(packageJson.scripts["bundle:nsis:local"], /signing\.wosign\.conf\.json/u);
assert.match(localNsisBuildScript, /Remove-Item -LiteralPath \$outputPath -Recurse -Force/u);
assert.match(
  localNsisBuildScript,
  /src-tauri\\target\\release\\bundle\\nsis/u,
  "local NSIS cleanup must be pinned to the exact Tauri NSIS output directory",
);
assert.match(localNsisBuildScript, /Fresh local NSIS artifact/u);
assert.match(localNsisBuildScript, /npm\.cmd run build:nsis:stage:local/u);
assert.match(localNsisBuildScript, /npm\.cmd run bundle:nsis:local/u);
assert.ok(
  localNsisBuildScript.indexOf("Refusing to clean an unexpected local NSIS output path") <
    localNsisBuildScript.indexOf("Remove-Item -LiteralPath $outputPath -Recurse -Force"),
  "local NSIS output path must be validated before recursive cleanup",
);
assert.doesNotMatch(packageJson.scripts["build:nsis"], /build-local-nsis/u);
assert.doesNotMatch(packageJson.scripts["bundle:nsis"], /build-local-nsis/u);

function tauriArgsFromScript(script) {
  const marker = "node scripts/tauri/build.js ";
  const offset = script.indexOf(marker);
  assert.notEqual(offset, -1, `missing build.js invocation: ${script}`);
  return script
    .slice(offset + marker.length)
    .split("&&", 1)[0]
    .trim()
    .split(/\s+/u);
}

const localStageArgs = prepareTauriArgs(
  tauriArgsFromScript(packageJson.scripts["build:nsis:stage:local"]),
  {
    platform: "win32",
    stageRuntime: () => null,
    additionalConfigs: [WINDOWS_BRIDGE_CONFIG_PATH],
  },
);
const localBundleArgs = prepareTauriArgs(
  tauriArgsFromScript(packageJson.scripts["bundle:nsis:local"]),
  {
    platform: "win32",
    stageRuntime: () => null,
    additionalConfigs: [WINDOWS_BRIDGE_CONFIG_PATH],
  },
);
for (const [phase, args] of [["compile", localStageArgs], ["bundle", localBundleArgs]]) {
  assert.ok(!args.includes("--features"), `local ${phase} args must not enable features`);
  assert.ok(!args.includes("dev-tools"), `local ${phase} args must not enable dev-tools`);
  assert.ok(
    configSpecs(args).every(config => !config.includes("signing.wosign.conf.json")),
    `local ${phase} args must not include the WoSign overlay`,
  );
}
assert.ok(
  configSpecs(localBundleArgs).includes(WINDOWS_BRIDGE_CONFIG_PATH),
  "local bundle args must retain the Windows bridge overlay synthesized by build.js",
);
const localEffectiveSpecs = configSpecs(localBundleArgs).map(config => {
  // These files are generated by Windows staging and intentionally do not
  // exist in a clean Linux checkout. Compose the bridge from its production
  // source and use a neutral NSIS staging patch so this semantic test remains
  // portable without weakening the complete build.js argument assertions.
  if (config === WINDOWS_BRIDGE_CONFIG_PATH) {
    return JSON.stringify(windowsBridgeOverlay());
  }
  if (config.includes("tauri.nsis-stage.conf.json")) {
    return JSON.stringify({ bundle: { resources: {} } });
  }
  return config;
});
const localEffectiveConfig = composeEffectiveConfig(localEffectiveSpecs).effectiveConfig;
assert.equal(
  localEffectiveConfig.bundle.windows.signCommand,
  undefined,
  "build.js-composed local NSIS config must not contain a WoSign signCommand",
);

if (process.platform === "win32") {
  const smokeRoot = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-local-nsis-"));
  const defaultRoot = path.join(smokeRoot, "default-root");
  const defaultScriptDir = path.join(defaultRoot, "scripts", "tauri");
  const defaultScriptPath = path.join(defaultScriptDir, "build-local-nsis.ps1");
  const smokeOutput = path.join(
    smokeRoot,
    "src-tauri",
    "target",
    "release",
    "bundle",
    "nsis",
  );
  const siblingPath = path.join(smokeRoot, "src-tauri", "target", "release", "bundle", "keep.txt");
  const runLocalNsisScript = (...args) => spawnSync(
    "powershell.exe",
    [
      "-NoProfile",
      "-ExecutionPolicy",
      "Bypass",
      "-File",
      localNsisBuildScriptPath,
      "-AppRoot",
      smokeRoot,
      ...args,
    ],
    { encoding: "utf8" },
  );
  try {
    fs.mkdirSync(defaultScriptDir, { recursive: true });
    fs.copyFileSync(localNsisBuildScriptPath, defaultScriptPath);
    const defaultRootResult = spawnSync(
      "powershell.exe",
      [
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        defaultScriptPath,
        "-PrepareOnly",
      ],
      { encoding: "utf8" },
    );
    assert.equal(
      defaultRootResult.status,
      0,
      defaultRootResult.stderr || defaultRootResult.stdout,
    );
    assert.equal(
      fs.existsSync(path.join(defaultRoot, "src-tauri", "target", "release", "bundle")),
      true,
      "PowerShell 5.1 must resolve the default AppRoot from the script body",
    );

    fs.mkdirSync(smokeOutput, { recursive: true });
    fs.writeFileSync(path.join(smokeOutput, "stale-installer.exe"), "stale");
    fs.writeFileSync(siblingPath, "keep");
    const prepareResult = runLocalNsisScript("-PrepareOnly");
    assert.equal(prepareResult.status, 0, prepareResult.stderr || prepareResult.stdout);
    assert.equal(fs.existsSync(smokeOutput), false, "prepare must remove stale NSIS output");
    assert.equal(fs.readFileSync(siblingPath, "utf8"), "keep", "prepare must preserve bundle siblings");

    const missingResult = runLocalNsisScript("-VerifyOnly");
    assert.notEqual(missingResult.status, 0, "verification must reject a missing fresh artifact");

    fs.mkdirSync(smokeOutput, { recursive: true });
    const artifactPath = path.join(smokeOutput, "fresh-installer.exe");
    fs.writeFileSync(artifactPath, "fresh");
    const stale = new Date("2000-01-01T00:00:00.000Z");
    fs.utimesSync(artifactPath, stale, stale);
    const staleResult = runLocalNsisScript("-VerifyOnly");
    assert.notEqual(staleResult.status, 0, "verification must reject a predating artifact");

    const future = new Date(Date.now() + 3000);
    fs.utimesSync(artifactPath, future, future);
    const verifyResult = runLocalNsisScript("-VerifyOnly");
    assert.equal(verifyResult.status, 0, verifyResult.stderr || verifyResult.stdout);
    assert.match(verifyResult.stdout, /Fresh local NSIS artifact:.*fresh-installer\.exe/u);
  } finally {
    fs.rmSync(smokeRoot, { recursive: true, force: true });
  }
}
assert.match(packageJson.scripts["prebuild:nsis"], /clean-staging\.ps1/u);
assert.doesNotMatch(
  packageJson.scripts["prebuild:nsis"],
  /ensure-rust-toolchain\.ps1/u,
  "build.js must establish the isolated RUSTUP_HOME before invoking the guard",
);
assert.match(buildScript, /await ensureWindowsRustToolchain\(\)/u);
assert.match(buildScript, /args: \["-CheckOnly"\]/u);
assert.match(buildScript, /accountExitCode === 0/u);
assert.match(buildScript, /accountExitCode !== 2/u);
assert.match(buildScript, /PINVOU3_MANAGED_RUSTUP = "1"/u);
assert.match(buildScript, /\.cache", "rustup"/u);
assert.match(rustToolchainGuard, /workspace-scoped RUSTUP_HOME/u);
assert.match(rustToolchainGuard, /if \(\$CheckOnly\)/u);
assert.match(rustToolchainGuard, /exit 2/u);
assert.doesNotMatch(rustToolchainGuard, /TEMPORARY|JENKINS_/u);
assert.doesNotMatch(rustToolchainGuard, /Get-Process|Stop-Process/u);
assert.doesNotMatch(rustToolchainGuard, /SkipProcessCleanup/u);
assert.match(rustToolchainGuard, /\.pinvou3-managed-rustup/u);
assert.match(rustToolchainGuard, /\.pinvou3-toolchain\.lock/u);
assert.match(rustToolchainGuard, /shared RUSTUP_HOME/u);
assert.match(rustToolchainGuard, /rust-toolchain\.toml/u);
assert.match(rustToolchainGuard, /"toolchain", "install"/u);
assert.match(rustToolchainGuard, /"--profile", "minimal"/u);
assert.match(rustToolchainGuard, /"toolchain", "uninstall"/u);
assert.match(rustToolchainGuard, /RUSTUP_DOWNLOAD_TIMEOUT/u);
assert.match(rustToolchainGuard, /function Invoke-Rustup/u);
assert.match(rustToolchainGuard, /ErrorActionPreference = "Continue"/u);
assert.match(rustToolchainGuard, /RUSTUP_DIST_SERVER/u);
assert.match(rustToolchainGuard, /https:\/\/static\.rust-lang\.org/u);
// The Tsinghua mirror entry is a closed-build-machine pin; the open checkout
// only guarantees the official source plus the rsproxy fallback.
assert.match(rustToolchainGuard, /https:\/\/rsproxy\.cn/u);
assert.match(rustToolchainGuard, /Name = "configured source"/u);
assert.match(rustToolchainGuard, /InstallAttemptTimeoutSeconds = 600/u);
assert.match(rustToolchainGuard, /RepairAttemptsPerSource = 2/u);
assert.match(
  rustToolchainGuard,
  /Repair attempt \$attempt\/\$RepairAttemptsPerSource/u,
);
assert.match(rustToolchainGuard, /WaitForExit\(\$TimeoutSeconds \* 1000\)/u);
assert.match(rustToolchainGuard, /return 124/u);
assert.match(rustToolchainGuard, /postInstallInvalid/u);
assert.match(rustupRepairSmoke, /pinvou-rustup-repair-/u);
assert.match(rustupRepairSmoke, /-CheckOnly/u);
assert.match(rustupRepairSmoke, /127\.0\.0\.1:1/u);
assert.match(rustupRepairSmoke, /-RepairAttemptsPerSource 1/u);
assert.match(rustupRepairSmoke, /rust-toolchain\.toml/u);
assert.match(rustupRepairSmoke, /\.pinvou3-managed-rustup/u);
assert.match(rustupRepairSmoke, /Isolated inconsistent-toolchain repair: PASS/u);
assert.match(
  packageJson.scripts["test:windows-rustup-repair"],
  /windows_rustup_repair_smoke\.ps1/u,
);
assert.match(packageJson.scripts["build:nsis:stage"], /--verbose/u);
assert.match(packageJson.scripts["build:nsis:stage"], /--ci/u);
assert.doesNotMatch(
  packageJson.scripts["prebuild:nsis"],
  /stage-runtime\.ps1|runtime:windows:stage/u,
  "prebuild:nsis must leave runtime validation and staging to build.js",
);
assert.ok(
  packageJson.scripts["bundle:nsis"].indexOf("stage-resources.ps1 -ValidateOnly") <
    packageJson.scripts["bundle:nsis"].indexOf("node scripts/tauri/build.js bundle"),
  "bundle:nsis must validate staging before build.js skips runtime staging",
);
assert.match(packageJson.scripts["bundle:nsis"], /tauri\.nsis-stage\.conf\.json/u);

if (signScript !== null) {
  assert.doesNotMatch(
    signScript,
    /nsis-stage|runtime[\\/]node|resources[\\/]/u,
    "the signing command must not use fragile resource-path special cases",
  );
}

console.log("NSIS pre-signed resource staging contract: ok");
