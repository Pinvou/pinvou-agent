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
const runtimeEnvScript = read(
  "src-tauri",
  "packaging",
  "windows",
  "nsis",
  "runtime-env.ps1",
);
const mainWxs = read("src-tauri", "packaging", "windows", "wix", "main.wxs");
const windowsPlatformConfig = JSON.parse(
  read("src-tauri", "config", "platforms", "windows", "tauri.conf.json"),
);
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
// Pin the producer side of the staging linchpin: bundle:nsis consumes the
// conf this script writes, so a producer rename must fail here, not at
// makensis.
assert.match(stageScript, /tauri\.nsis-stage\.conf\.json/u);
// The size accounting feeds !define /math ESTIMATEDSIZE; a unit or sum
// mutation must fail here rather than silently corrupt Apps & Features.
assert.match(
  stageScript,
  /\[Math\]::Ceiling\(\$StagedResourceBytes \/ 1KB\)/u,
  "staged size must stay in KiB to match the bundler's estimated_size units",
);
assert.match(stageScript, /Measure-Object -Property Length -Sum/u);
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

// The template must satisfy the bundler version pinned by package.json: its
// generated utils.nsh pulls RestartManager macros (so Win\RestartManager.nsh
// must be included) and CheckIfAppIsRunning registers an absolute path with
// Restart Manager. The provenance header records the re-base source.
assert.match(
  installerTemplate,
  /; Pinvou fork of the Tauri bundler NSIS template\. Base: tauri-cli-v2\.12\.4/u,
  "re-base the template on the bundler version pinned by package.json before adding deltas",
);
assert.match(
  installerTemplate,
  /!include "Win\\RestartManager\.nsh"/u,
  "the bundler's generated utils.nsh requires the RestartManager include",
);
assert.match(
  installerTemplate,
  /!insertmacro CheckIfAppIsRunning "\$INSTDIR\\\$\{MAINBINARYNAME\}\.exe" "\$\{PRODUCTNAME\}"/u,
  "Restart Manager registration requires the absolute main binary path",
);
assert.equal(
  windowsPlatformConfig.bundle.windows.nsis.template,
  "packaging/windows/nsis/installer-template.nsi",
  "tauri.conf.json must wire the custom NSIS template, or tauri silently falls back to its default",
);
assert.ok(
  ["SimpChinese", "English", "Japanese"].every(language =>
    windowsPlatformConfig.bundle.windows.nsis.languages.includes(language),
  ),
  "installer languages must cover Simplified Chinese, English and Japanese",
);
assert.deepEqual(
  windowsPlatformConfig.bundle.windows.wix.languages,
  ["en-US", "zh-CN", "ja-JP"],
  "MSI languages must cover English, Simplified Chinese and Japanese (repo i18n policy)",
);

// Installer hooks pack helper scripts next to the installer via the
// file-level ${PINVOU_HOOKS_DIR} anchor (${__FILEDIR__} inside !macro keeps
// the insertion-site value): both the committed base hooks and the staged
// generated copy sit exactly three directories below src-tauri.
assert.match(
  baseInstallerHooks,
  /!define PINVOU_HOOKS_DIR "\$\{__FILEDIR__\}"/u,
);
assert.match(
  baseInstallerHooks,
  /\$\{PINVOU_HOOKS_DIR\}\\\.\.\\\.\.\\\.\.\\target\\windows-runtime\\nsis\\vc_redist\\VC_redist\.x64\.exe/u,
  "vc_redist is staged to src-tauri\\target\\windows-runtime\\nsis by windows-installer.js",
);
assert.match(
  baseInstallerHooks,
  /\$\{PINVOU_HOOKS_DIR\}\\\.\.\\\.\.\\\.\.\\packaging\\windows\\nsis\\vcredist-temp-preflight\.ps1/u,
  "hook-packed scripts must resolve against src-tauri, not the repository root",
);
assert.match(baseInstallerHooks, /!macro NSIS_HOOK_POSTINSTALL/u);
assert.match(baseInstallerHooks, /!macro NSIS_HOOK_POSTUNINSTALL/u);
assert.match(baseInstallerHooks, /PinvouRunRuntimeEnv Install/u);
assert.match(baseInstallerHooks, /PinvouRunRuntimeEnv Uninstall/u);
assert.match(
  baseInstallerHooks,
  /\$\{PINVOU_HOOKS_DIR\}\\\.\.\\\.\.\\\.\.\\packaging\\windows\\nsis\\runtime-env\.ps1/u,
  "runtime-env.ps1 must be packed into the installer and executed at install/uninstall time",
);
assert.match(
  baseInstallerHooks,
  /!insertmacro PinvouRunRuntimeEnv Install true/u,
  "install must abort when the machine runtime environment cannot be configured (WiX Environment parity)",
);
assert.match(
  baseInstallerHooks,
  /組み込みランタイム環境/u,
  "installer failure copy must cover Japanese alongside Chinese and English",
);
const runtimeEnvMacroBody = baseInstallerHooks.slice(
  baseInstallerHooks.indexOf("!macro PinvouRunRuntimeEnv"),
  baseInstallerHooks.indexOf(
    "!macroend",
    baseInstallerHooks.indexOf("!macro PinvouRunRuntimeEnv"),
  ),
);
assert.doesNotMatch(
  runtimeEnvMacroBody,
  /^\s+SetOutPath "\$INSTDIR"\s*$/mu,
  "the runtime-env macro must not recreate $INSTDIR after the uninstall removed it (the vc_redist macro may: it only runs on the install leg)",
);
assert.match(
  runtimeEnvScript,
  /DoNotExpandEnvironmentNames/u,
  "machine PATH rewrite must preserve REG_EXPAND_SZ entries",
);

// MSI user-data deletion must stay behind an explicit opt-in property
// (mirroring the NSIS uninstaller's unchecked delete-app-data checkbox,
// which additionally removes the WebView2 cache directories).
assert.match(
  mainWxs,
  /\(REMOVE = "ALL"\) AND NOT UPGRADINGPRODUCTCODE AND PINVOU_REMOVE_USER_DATA/u,
  "MSI must not delete ~\\.pinvou3 unless the uninstall explicitly opts in",
);
assert.match(
  mainWxs,
  /<Property Id="PINVOU_REMOVE_USER_DATA" Secure="yes" \/>/u,
  "the opt-in property must be declared secure, or MSI drops it for non-elevated uninstall sessions",
);
assert.match(
  mainWxs,
  /msiexec \/x <msi> PINVOU_REMOVE_USER_DATA=1/u,
  "the in-template documentation must use the working msiexec uninstall form",
);
assert.match(
  mainWxs,
  /Base: tauri-cli-v2\.12\.4/u,
  "main.wxs must carry the same provenance contract as the NSIS template",
);
assert.match(
  installerTemplate,
  /RmDir \/r "\$PROFILE\\\.pinvou3"/u,
  "the NSIS delete-app-data checkbox must also remove ~/.pinvou3 (MSI RemovePinvouUserData parity)",
);

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
// Positive needles are required: an indexOf ordering assertion alone is
// vacuously true once its antecedent string is deleted (both sides -1).
assert.match(
  localNsisBuildScript,
  /Refusing to clean an unexpected local NSIS output path/u,
);
assert.match(
  localNsisBuildScript,
  /Remove-Item -LiteralPath \$outputPath -Recurse -Force/u,
);
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
// The clean step must never be named prebuild:nsis: npm would auto-run it as
// a lifecycle pre-hook of build:nsis, doubling the clean pass.
assert.match(packageJson.scripts["nsis:clean-staging"], /clean-staging\.ps1/u);
assert.match(packageJson.scripts["build:nsis:stage"], /npm run nsis:clean-staging/u);
assert.equal(
  packageJson.scripts["prebuild:nsis"],
  undefined,
  "the npm pre-hook name would double-run clean-staging on every build:nsis",
);
assert.doesNotMatch(
  packageJson.scripts["nsis:clean-staging"],
  /ensure-rust-toolchain\.ps1/u,
  "build.js must establish the isolated RUSTUP_HOME before invoking the guard",
);
// The rust-toolchain guard and rustup-repair smoke internals are owned by
// tests/windows_rust_toolchain_contract.test.js; only the build-chain
// relevance stays pinned here.
assert.match(packageJson.scripts["build:nsis:stage"], /--verbose/u);
assert.match(packageJson.scripts["build:nsis:stage"], /--ci/u);
assert.doesNotMatch(
  packageJson.scripts["nsis:clean-staging"],
  /stage-runtime\.ps1|runtime:windows:stage/u,
  "clean-staging must leave runtime validation and staging to build.js",
);
// Positive needles are required here too: an indexOf ordering assertion is
// vacuously true once its antecedent string is deleted (both sides -1).
assert.match(
  packageJson.scripts["bundle:nsis"],
  /stage-resources\.ps1 -ValidateOnly/u,
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
