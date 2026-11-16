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
const userProfileScript = read(
  "src-tauri",
  "packaging",
  "windows",
  "nsis",
  "pinvou-user-profile.ps1",
);
const runtimeEnvScript = read(
  "src-tauri",
  "packaging",
  "windows",
  "nsis",
  "runtime-env.ps1",
);
const mainWxs = read("src-tauri", "packaging", "windows", "wix", "main.wxs");
const pythonNodePathWxs = read(
  "src-tauri",
  "packaging",
  "windows",
  "wix",
  "python-node-path.wxs",
);
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
// Round-2 space safety: trailing separators are stripped from the staging
// source and destination before the native robocopy call (PowerShell 5.1
// parses a trailing backslash before a closing quote as a literal quote,
// corrupting the argument for checkout paths containing spaces). Both call
// sites are load-bearing — dropping either one corrupts one side of the
// robocopy invocation — so the pin counts occurrences, not mere presence.
assert.ok(
  (stageScript.match(/TrimEnd\('\\',\s*'\/'\)/gu) || []).length >= 2,
  "staging source and destination paths must both strip trailing separators before robocopy (both call sites)",
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
// The MSI Environment fragment only takes effect when the bundler renders
// and links it: the template must stay wired, the fragment path must stay
// listed, and the fragment's component must stay referenced — WiX silently
// installs an unreferenced component, and no CI lane compiles the MSI.
assert.equal(
  windowsPlatformConfig.bundle.windows.wix.template,
  "packaging/windows/wix/main.wxs",
  "tauri.conf.json must wire the custom WiX template",
);
assert.deepEqual(
  windowsPlatformConfig.bundle.windows.wix.fragmentPaths,
  ["packaging/windows/wix/python-node-path.wxs"],
  "the PINVOU3_PYTHON Environment fragment must stay wired into the WiX build",
);
assert.match(
  pythonNodePathWxs,
  /<Component Id="PythonNodeRuntimeEnvironment"/u,
  "componentRefs entries must reference a component the fragment actually defines",
);
assert.deepEqual(
  windowsPlatformConfig.bundle.windows.wix.componentRefs,
  ["PythonNodeRuntimeEnvironment"],
  "the fragment component must stay referenced in the External feature, or it is silently skipped",
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
// $INSTDIR reaches runtime-env.ps1 through the environment (the installer
// sets PINVOU_INSTALL_DIR): a quoted command-line argument whose value ends
// in a backslash (drive-root installs) swallows the closing quote during
// Windows argument parsing.
assert.match(
  runtimeEnvMacroBody,
  /SetEnvironmentVariableW\(w "PINVOU_INSTALL_DIR", w "\$INSTDIR"\)/u,
  "the macro must hand $INSTDIR to PowerShell via the environment",
);
assert.doesNotMatch(
  runtimeEnvMacroBody,
  /-InstallDir/u,
  "the macro must not pass $INSTDIR as a quoted command-line argument (trailing-backslash quoting)",
);
assert.match(
  runtimeEnvScript,
  /\[string\]\$InstallDir = \$env:PINVOU_INSTALL_DIR/u,
  "runtime-env.ps1 must default InstallDir from the installer-provided environment variable",
);
assert.match(
  runtimeEnvScript,
  /InstallDir was not provided/u,
  "runtime-env.ps1 must fail closed when neither the environment nor -InstallDir supplies the install directory",
);
assert.match(
  runtimeEnvScript,
  /DoNotExpandEnvironmentNames/u,
  "machine PATH rewrite must preserve REG_EXPAND_SZ entries",
);

// MSI user-data deletion must stay behind an explicit opt-in property
// (mirroring the NSIS uninstaller's unchecked delete-app-data checkbox,
// which additionally removes the WebView2 app-data directories).
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
// The provenance version must track the bundler actually pinned by
// package.json: a CLI bump without re-basing the template passes every
// other pin (no CI lane runs makensis), so bind the header to the
// dependency.
const cliPin = packageJson.devDependencies["@tauri-apps/cli"];
const cliVersion = String(cliPin).replace(/^[~^><= ]*/, "");
assert.ok(
  cliVersion && installerTemplate.includes("Base: tauri-cli-v" + cliVersion),
  "the template provenance header must track the pinned @tauri-apps/cli (" +
    cliPin +
    "); re-base the template when the CLI moves",
);
// The NSIS delete-app-data checkbox must remove the prompting user's
// ~/.pinvou3 and app-data directories (MSI RemovePinvouUserData parity),
// through the interactive session owner's profile: the uninstaller runs
// elevated, and under over-the-shoulder elevation the shell constants
// ($PROFILE/$APPDATA/$LOCALAPPDATA, like SetShellVarContext current)
// resolve against the elevation token's account, which would wipe the
// admin's data instead of the prompting user's. The whole deletion block
// therefore lives in the Pinvou-owned hooks macro (not the rebase-costly
// template fork) and derives every target from the resolver's profile.
assert.match(
  baseInstallerHooks,
  /!macro PinvouDeleteInteractiveUserData/u,
  "the delete-app-data resolution must stay in the Pinvou-owned hooks macro, not the upstream-fork template",
);
assert.doesNotMatch(
  baseInstallerHooks,
  /!insertmacro PinvouDeleteInteractiveUserData/u,
  "the hooks must define PinvouDeleteInteractiveUserData, not insert it",
);
assert.match(
  installerTemplate,
  /!insertmacro PinvouDeleteInteractiveUserData/u,
  "the template must invoke the hook macro from the delete-app-data checkbox",
);
assert.doesNotMatch(
  installerTemplate,
  /RmDir \/r "\$(PROFILE|APPDATA|LOCALAPPDATA)/u,
  "the uninstaller must not delete through token-following shell constants: $PROFILE/$APPDATA/$LOCALAPPDATA follow the elevation token under over-the-shoulder elevation",
);
assert.match(
  baseInstallerHooks,
  /File "\/oname=\$PLUGINSDIR\\pinvou-user-profile\.ps1" "\$\{PINVOU_HOOKS_DIR\}\\\.\.\\\.\.\\\.\.\\packaging\\windows\\nsis\\pinvou-user-profile\.ps1"/u,
  "the resolver must be packed from the committed packaging/windows/nsis source (same ${PINVOU_HOOKS_DIR} arithmetic as the sibling hook scripts)",
);
assert.match(
  baseInstallerHooks,
  /nsExec::ExecToStack \/OEM 'powershell -NoProfile -ExecutionPolicy Bypass -File "\$PLUGINSDIR\\pinvou-user-profile\.ps1"'/u,
  "the resolver invocation must pass /OEM as a separate nsExec parameter: the nsExec switch parser recognizes /OEM only when a popped argument equals it, so a quoted '/OEM powershell ...' single argument is treated as the whole command line and the resolver never starts",
);
assert.doesNotMatch(
  baseInstallerHooks,
  /nsExec::ExecToStack '\/OEM /u,
  "the quoted '/OEM powershell ...' single-argument form must never return: nsExec would treat it as the command line (spawn failure), silently keeping every uninstall's app data",
);
assert.match(
  baseInstallerHooks,
  /\$\{If\} \$8 == 0\s*\$\{AndIf\} \$9 != ""/u,
  "the delete guard must string-compare the exit code and require a non-empty path: nsExec pushes the literal \"error\" on spawn failure and LogicLib '=' is numeric (\"error\" parses as 0)",
);
assert.match(
  baseInstallerHooks,
  /RmDir \/r "\$9\\AppData\\Roaming\\\$\{BUNDLEID\}"\s*RmDir \/r "\$9\\AppData\\Local\\\$\{BUNDLEID\}"\s*RmDir \/r "\$9\\\.pinvou3"/u,
  "all three deletion targets must derive from the resolved interactive-user profile",
);
assert.match(
  baseInstallerHooks,
  /Could not identify the interactive user's profile/,
  "profile-resolution failure must keep the data and tell the user (fail safe, never guess-and-wipe)",
);
assert.match(
  userProfileScript,
  /Win32_Process/u,
  "the resolver must derive the owner from the interactive session's shell process",
);
assert.match(
  userProfileScript,
  /SessionId = \$sessionId/u,
  "the resolver must scope the shell-process enumeration to the uninstaller's own session: over-the-shoulder elevation keeps the requesting user's session, while the first explorer.exe across all sessions can belong to any other logged-in user",
);
assert.match(
  userProfileScript,
  /GetOwnerSid/u,
  "the resolver must take the session owner's SID, never its bare account name",
);
assert.match(
  userProfileScript,
  /ReturnValue -eq 0/u,
  "the resolver must check GetOwnerSid's ReturnValue before trusting the SID",
);
assert.match(
  userProfileScript,
  /Win32_UserProfile[\s\S]*?SID -ieq \$ownerSids\[0\]/u,
  "profile mapping must compare Win32_UserProfile SID against the session owner's SID: name-based leaf matching breaks on renames and picks the wrong profile on cross-domain name collisions",
);
assert.match(
  userProfileScript,
  /exit 2/u,
  "the resolver must fail closed (non-zero exit, no path printed) when the owner is ambiguous",
);
assert.match(
  userProfileScript,
  /OEMCP/u,
  "the resolver must learn the exact code page the uninstaller's nsExec /OEM decode uses, not assume its own console encoding",
);
assert.match(
  userProfileScript,
  /TryParse\(\$oemCpRaw,[\s\S]*?exit 2/u,
  "the REG_SZ OEMCP value must be parsed as a number with the unparseable case routed to the keep-data exit: the three-argument GetEncoding overload treats its argument as an encoding NAME, so a raw REG_SZ string like '936' throws before the representability check and would strand even an ASCII profile",
);
assert.match(
  userProfileScript,
  /GetEncoding\(\n?\s*\$oemCp,/u,
  "the encoding must be constructed from the parsed numeric code page (integer overload semantics), inside the fail-safe try boundary",
);
assert.match(
  userProfileScript,
  /EncoderExceptionFallback[\s\S]*?GetBytes\(\$profileMatches\[0\]\)/u,
  "the resolver must round-trip the resolved path through the /OEM code page with an exception-throwing fallback: a path with characters outside that code page would be rewritten with replacement characters, every deletion target would miss, and the zero-status/nonempty guard would hide the loss",
);
assert.match(
  userProfileScript,
  /GetBytes[\s\S]*?exit 2[\s\S]*?\[Console\]::Out\.Write/u,
  "the encoding loss check must sit between path resolution and stdout emission, exiting non-zero without printing so the uninstaller keeps the data under the existing manual-removal guidance",
);
assert.doesNotMatch(
  userProfileScript,
  /RmDir|Remove-Item/u,
  "the resolver must never delete anything itself",
);
assert.doesNotMatch(
  baseInstallerHooks,
  /pinjou_/u,
  "the hooks must never reference a j-spelled identifier: it would be a makensis-dangling label no test lane compiles away",
);
// Template delta pin: the dump_system_prompt dev-tool exclusion has no
// runtime test today (nothing enumerates that binary yet), so pin the
// exclusion in both binary loops (install and uninstall). The count targets
// the loop guard itself — the template's third dump_system_prompt mention
// is the provenance comment, which must not be able to keep the pin green
// after either loop loses its exclusion.
assert.ok(
  (installerTemplate.match(/!if "\{\{this\}\}" != "dump_system_prompt\.exe"/gu) || [])
    .length >= 2,
  "dump_system_prompt.exe must be excluded in both binary loops (install and uninstall)",
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
