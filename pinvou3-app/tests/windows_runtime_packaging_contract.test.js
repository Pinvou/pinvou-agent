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
const pythonDependencyRuntimeTest = readApp(
  "tests",
  "windows_python_dependency_contract.ps1",
);
const packageJson = JSON.parse(readApp("package.json"));
const runtimeManifestContract = readApp(
  "src-tauri",
  "packaging",
  "windows",
  "runtime",
  "scripts",
  "runtime-manifest-contract.ps1",
);
const onnxRuntimeScript = readApp(
  "src-tauri",
  "packaging",
  "windows",
  "runtime",
  "scripts",
  "stage-onnx-runtime.ps1",
);
const onnxRuntimeSmoke = readApp(
  "src-tauri",
  "packaging",
  "windows",
  "runtime",
  "scripts",
  "test-onnx-runtime.ps1",
);
const runtimeManifestHarness = readApp(
  "src-tauri",
  "packaging",
  "windows",
  "runtime",
  "scripts",
  "test-runtime-manifest-contract.ps1",
);
const installerHook = readApp(
  "src-tauri",
  "packaging",
  "windows",
  "nsis",
  "installer-hooks.nsh",
);
const vcRedistTempPreflightPath = path.join(
  appRoot,
  "src-tauri",
  "packaging",
  "windows",
  "nsis",
  "vcredist-temp-preflight.ps1",
);
const vcRedistTempPreflight = fs.readFileSync(vcRedistTempPreflightPath, "utf8");
const runtimeWrapper = readApp("scripts", "tauri", "windows-runtime.js");
const installerAdapter = readApp("scripts", "tauri", "windows-installer.js");
const buildScript = readApp("scripts", "tauri", "build.js");
const bridgeScript = readApp("scripts", "tauri", "codex-bridge.js");
const releaseWorkflow = readRepo(".github", "workflows", "release-packages.yml");
const prWorkflow = readRepo(".github", "workflows", "pr-check.yml");

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
assert.match(lock.vcRedist.minimumVersion, /^\d+\.\d+\.\d+\.\d+$/u);

const [vcMajor, vcMinor, vcBuild, vcRevision] = lock.vcRedist.minimumVersion
  .split(".")
  .map(Number);
assert.ok(
  [vcMajor, vcMinor, vcBuild, vcRevision].every(Number.isSafeInteger),
  "VC++ minimum version components must be safe integers",
);

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
  "Assert-WindowsRuntimeStagedFilesExact",
  "System.IO.Compression.ZipFile",
  "Write-Utf8WithoutBom",
  "Write-Utf8Atomically",
  "Test-StageInventory",
  ".verified-stage.json",
  "Get-CachedVerifiedManifest",
  "Enter-ResolverLock",
  ".resolver.lock",
  "Get-RuntimeDescriptorContent",
  "onnxRuntimeDylib",
  'delivery = "download-on-first-use"',
  "Test-PythonDependencyTarget",
  "Test-PythonDependencyTargets",
  "Test-PythonWheelTarget",
  "python_dependencies",
  "windows-x64",
  "files.pythonhosted.org",
]) {
  assert.ok(runtimeScript.includes(contract), `runtime staging must retain ${contract}`);
}
const reusableStageStart = runtimeScript.indexOf("function Test-VerifiedStageReusable");
const freshStageStart = runtimeScript.indexOf("function Stage-Submodule", reusableStageStart);
const onnxDescriptorStart = runtimeScript.indexOf(
  "function Get-OnnxDevDescriptorContent",
  freshStageStart,
);
assert.ok(reusableStageStart >= 0 && freshStageStart > reusableStageStart);
assert.ok(onnxDescriptorStart > freshStageStart);
assert.match(
  runtimeScript.slice(reusableStageStart, freshStageStart),
  /Test-PythonDependencyTargets/,
  "verified stage reuse must revalidate bundled Python ABI and wheel locks",
);
assert.match(
  runtimeScript.slice(freshStageStart, onnxDescriptorStart),
  /Assert-FreshStagePythonDependencies/,
  "fresh runtime staging must validate bundled Python ABI and wheel locks",
);
assert.match(
  pythonDependencyRuntimeTest,
  /ABI mismatch must fail closed/,
  "PowerShell contract must exercise fail-closed ABI validation",
);
assert.match(
  runtimeScript,
  /\$null = & \$pythonExe -I -S -B -c \$probe/u,
  "ABI probe stdout must be discarded so stray output cannot flip the exit-code check",
);
assert.match(
  packageJson.scripts["test:windows-runtime"],
  /windows_python_dependency_contract\.ps1/u,
  "the windows runtime npm chain must execute the python dependency contract, or the ps1 loses its only CI runner",
);
assert.match(runtimeScript, /Get-Sha256 -Path \$sourcePath/u);
assert.match(runtimeScript, /schemaVersion -notin @\(1, 2\)/u);
assert.match(runtimeManifestContract, /Manifest\.stagedFiles/u);
assert.match(runtimeScript, /runtime-manifest-contract\.ps1/u);
assert.match(runtimeScript, /Assert-WindowsRuntimeStagedFilesDeclared -Manifest \$manifest/u);
assert.match(runtimeManifestContract, /Get-CanonicalWindowsRuntimeManifestPath/u);
assert.match(runtimeManifestContract, /\$null -eq \$Manifest\.stagedFiles/u);
assert.match(runtimeManifestContract, /Dictionary\[string, object\]/u);
assert.match(runtimeManifestContract, /Windows runtime lifecycle contains an extra file/u);
assert.match(runtimeManifestContract, /Windows runtime staged file failed verification/u);
const stagedFilesCheck = runtimeScript.indexOf("Assert-WindowsRuntimeStagedFilesExact");
const derivedVcStage = runtimeScript.indexOf("Preparing descriptor-owned VC++ runtime component");
const payloadCleanup = runtimeScript.indexOf("Remove-Item -LiteralPath $stageContext.PayloadRoot");
const verifiedStageWrite = runtimeScript.indexOf(
  'Write-Utf8WithoutBom -Path (Join-Path $stageContext.TemporaryRoot ".verified-stage.json")',
);
assert.ok(stagedFilesCheck >= 0 && stagedFilesCheck < derivedVcStage);
assert.ok(stagedFilesCheck < payloadCleanup);
assert.ok(
  verifiedStageWrite >= 0 && stagedFilesCheck < verifiedStageWrite,
  "runtime verification must finish before the verified-stage marker is written",
);
assert.match(runtimeScript, /vcRedist\.minimumVersion/u);
assert.match(runtimeScript, /System\.Diagnostics\.FileVersionInfo/u);
assert.match(runtimeScript, /\$vcActualVersion -lt \$vcMinimumVersion/u);
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
assert.match(initScript, /\[switch\]\$OnnxOnly/);
assert.match(initScript, /GIT_LFS_SKIP_SMUDGE/);
assert.match(initScript, /onnxruntime-win-x64-\*-runtime\.zip/);
assert.match(initScript, /pinvou3-windows-runtime-\$cacheSchema-\$expectedCommit/);
assert.match(
  initScript,
  /\$lockSha256\.Substring\(0, 16\)/u,
  "the Jenkins cache key must embed the lock content hash so lock changes invalidate it",
);
assert.match(initScript, /Get-RuntimeLockSha256/);
assert.match(initScript, /Enter-RuntimeResolverLock/);
assert.match(
  initScript,
  /\$cacheSchema = "v5"/u,
  "the cache schema literal must stay pinned so a bump is a conscious, reviewed edit",
);
assert.ok(
  initScript.includes('$resolverLockPath = Join-Path $resolverCacheRoot ".resolver.lock"') &&
    runtimeScript.includes('$resolverLockPath = Join-Path $stagingParent ".resolver.lock"'),
  "submodule initialization and runtime staging must share the same workspace lock",
);
assert.ok(
  initScript.includes(
    '$resolverCacheRoot = Join-Path $repoRoot "pinvou3-app\\src-tauri\\target\\windows-runtime"',
  ) && runtimeScript.includes('$stagingParent = Join-Path $tauriRoot "target\\windows-runtime"'),
  "the two scripts' lock-directory derivations must stay identical, not just the lock file name",
);
assert.match(
  runtimeScript,
  /\$sourceVerificationMarkerPath = Join-Path \$stagingParent "\.verified-lock"/u,
);
assert.match(
  runtimeScript,
  /Get-CachedVerifiedManifest -VerifyContent:\(\$Mode -eq "Validate"\)/u,
  "Stage may trust cached source metadata; Validate must re-hash the locked content",
);
assert.ok(
  runtimeScript.includes(
    "if ($VerifyContent -and (Get-Sha256 -Path $sourcePath) -ne [string]$entry.sha256)",
  ),
  "the Validate-mode content re-hash guard must stay inside Get-CachedVerifiedManifest, not just at its call site",
);
// The size-only cache is safe only while the cached path re-runs the full
// runtime identity check itself (marker match alone must never authorize
// per-file sizes), and a vanished checkout must decline to the full path so
// the actionable "not initialized" message survives the cache. Pin the call
// ordering inside Get-CachedVerifiedManifest's own body; removing either the
// checkout guard or the identity re-check there must fail here loudly.
const cachedFnStart = runtimeScript.indexOf("function Get-CachedVerifiedManifest");
const cachedMarkerIdx = runtimeScript.indexOf(
  "Test-VerificationMarker -Path $sourceVerificationMarkerPath",
  cachedFnStart,
);
const cachedCheckoutIdx = runtimeScript.indexOf(
  "Test-Path -LiteralPath $RuntimeRoot -PathType Container",
  cachedFnStart,
);
const cachedIdentityIdx = runtimeScript.indexOf("Assert-RuntimeIdentity", cachedFnStart);
const cachedSizeIdx = runtimeScript.indexOf(
  "$sourcePath).Length -ne [long]$entry.bytes",
  cachedFnStart,
);
assert.ok(
  cachedFnStart !== -1 &&
    cachedMarkerIdx > cachedFnStart &&
    cachedCheckoutIdx > cachedMarkerIdx &&
    cachedIdentityIdx > cachedCheckoutIdx &&
    cachedSizeIdx > cachedIdentityIdx,
  "the cached path must decline to the full path when the checkout is gone and re-run the runtime identity check between the marker check and the per-file size loop, not trust sizes on the marker alone",
);
assert.ok(
  runtimeScript.includes("param([int]$TimeoutSeconds = 120)") &&
    initScript.includes("param([int]$TimeoutSeconds = 120)"),
  "both lock holders must keep the same default 120s waiter timeout (the documented fail-and-retry contract)",
);
assert.match(
  runtimeScript,
  /^\$resolverLock = Enter-ResolverLock$/mu,
  "the lock call site must invoke the shared 120s default (no per-call -TimeoutSeconds override)",
);
assert.match(
  initScript,
  /^\$resolverLock = Enter-RuntimeResolverLock$/mu,
  "the lock call site must invoke the shared 120s default (no per-call -TimeoutSeconds override)",
);
assert.equal(
  (runtimeScript.match(/Write-Utf8Atomically -Path \$sourceVerificationMarkerPath/gu) || []).length,
  2,
  "the workspace source-metadata marker must be rewritten on both the full-verify and the cached-rebuild paths",
);
assert.match(runtimeScript, /Write-Utf8Atomically -Path \$runtimeDescriptorPath/u);
assert.match(runtimeScript, /Write-Utf8Atomically -Path \$generatedConfigPath/u);
assert.match(
  runtimeScript,
  /Write-Utf8Atomically -Path \$onnxDevDescriptorPath/u,
  "the ONNX dev descriptor is shared-visibility state and must be written atomically",
);
const atomicFnStart = runtimeScript.indexOf("function Write-Utf8Atomically");
const atomicFnEnd = runtimeScript.indexOf("\nfunction ", atomicFnStart);
assert.ok(atomicFnStart !== -1 && atomicFnEnd > atomicFnStart);
const atomicFnBody = runtimeScript.slice(atomicFnStart, atomicFnEnd);
assert.match(
  atomicFnBody,
  /^\s*if \(Test-Path -LiteralPath \$Path -PathType Leaf\) \{$/mu,
  "the atomic writer must branch on an existing destination",
);
assert.match(
  atomicFnBody,
  /^\s*\[System\.IO\.File\]::Replace\(\$temporaryPath, \$Path, \$null\)$/mu,
  "an existing destination must be swapped in via File.Replace (atomic ReplaceFile), because Move-Item -Force deletes the destination before the rename and the unlocked descriptor readers in scripts/tauri/windows-runtime.js would observe the file missing",
);
assert.match(
  atomicFnBody,
  /^\s*\[System\.IO\.File\]::Move\(\$temporaryPath, \$Path\)$/mu,
  "a fresh destination must still take the plain move path (File.Replace requires an existing destination)",
);
const atomicFnCode = atomicFnBody
  .split("\n")
  .filter((line) => !line.trimStart().startsWith("#"))
  .join("\n");
assert.doesNotMatch(
  atomicFnCode,
  /Move-Item/,
  "no delete-before-move primitive may re-enter the atomic writer",
);
assert.match(
  runtimeManifestHarness,
  /\. \(Join-Path \$PSScriptRoot "resolve-runtime\.ps1"\) -ImportFunctionsOnly/u,
  "the manifest harness must import the resolver functions to exercise Write-Utf8Atomically behaviorally",
);
assert.match(
  runtimeManifestHarness,
  /foreach \(\$generation in @\("second", "third"\)\)/u,
  "the manifest harness must exercise repeated atomic replacement (the re-stage path), not only first creation",
);
assert.match(
  runtimeManifestHarness,
  /Assert-Throws -Name "atomic replace under a sharing reader"/u,
  "the manifest harness must prove a failed atomic replacement preserves the previous descriptor",
);
assert.match(initScript, /\$previousErrorActionPreference = \$ErrorActionPreference/);
assert.match(initScript, /\$ErrorActionPreference = "Continue"/);
assert.match(initScript, /\$exitCode = \$LASTEXITCODE/);
assert.match(initScript, /\$ErrorActionPreference = \$previousErrorActionPreference/);
assert.match(initScript, /if \(\$exitCode -ne 0\)/);

assert.match(runtimeWrapper, /runtime-descriptor\.json/);
assert.match(runtimeWrapper, /cleanupLegacyWindowsNodeStaging/);
assert.match(runtimeWrapper, /download-on-first-use/);
assert.match(runtimeWrapper, /onnxRuntimeDylib/);
assert.match(runtimeWrapper, /stageWindowsOnnxRuntime/);
assert.match(onnxRuntimeScript, /-Mode StageOnnx/);
assert.match(runtimeScript, /Get-VerifiedOnnxManifest/);
assert.match(runtimeScript, /Stage-OnnxRuntime/);
assert.match(onnxRuntimeSmoke, /LoadLibraryEx/);
assert.match(onnxRuntimeSmoke, /OrtGetApiBase/);
assert.match(releaseWorkflow, /npm run runtime:windows:onnx-smoke/);
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
assert.match(installerHook, /pinvou-vcredist-temp-preflight\.ps1/);
assert.match(
  installerHook,
  /\$\{__FILEDIR__\}\\.\.\\.\.\\.\.\\.\.\\packaging\\windows\\nsis\\vcredist-temp-preflight\.ps1/,
  "preflight script File source must resolve from the installer.nsi output directory",
);
assert.doesNotMatch(
  installerHook,
  /\$\{__FILEDIR__\}\\.\.\\.\.\\.\.\\packaging\\windows\\nsis\\vcredist-temp-preflight\.ps1/,
  "preflight script File source must not resolve past src-tauri",
);
assert.ok(
  installerHook.indexOf("pinvou-vcredist-temp-preflight.ps1") <
    installerHook.indexOf('ExecWait \'"$PLUGINSDIR\\VC_redist.x64.exe"'),
  "Windows Installer temp preflight must run before VC++ starts",
);
assert.match(installerHook, /SetEnvironmentVariableW\(w "TEMP", w "\$WINDIR\\Temp"\)/);
assert.ok(
  installerHook.indexOf('SetEnvironmentVariableW(w "TEMP", w "$WINDIR\\Temp")') <
    installerHook.indexOf('ExecWait \'"$PLUGINSDIR\\VC_redist.x64.exe"'),
  "the TEMP/TMP override must be applied before the VC++ bundle starts",
);
assert.match(
  installerHook,
  /nsExec::ExecToStack 'powershell -NoProfile -ExecutionPolicy Bypass -File/,
  "the preflight must run without a profile and with an explicit execution-policy bypass",
);
assert.match(installerHook, /IntCmp \$5 1632 pinvou_vc_redist_temp_failed/);
assert.match(installerHook, /IntCmp \$5 -2147023264 pinvou_vc_redist_temp_failed/);
assert.match(installerHook, /Pinvou3-vcredist\.log/);
for (const recoveryHint of [
  "释放系统盘空间",
  "检查系统临时目录权限",
  "重启 Windows 后重试",
  "日志：$WINDIR\\Temp\\Pinvou3-vcredist.log",
]) {
  assert.ok(
    installerHook.includes(recoveryHint),
    `temp-dir failure guidance must include: ${recoveryHint}`,
  );
}
assert.match(
  vcRedistTempPreflight,
  /if \(-not \$SkipMachineEnvironment -and -not \(Test-Administrator\)\)[\s\S]*?Ensure-SystemDirectory -Path \$windowsTempPath/,
  "preflight must verify it runs elevated before touching machine ACLs",
);
assert.match(
  vcRedistTempPreflight,
  /Join-Path \$windowsRootPath "Temp"/,
  "preflight must target the Windows temporary directory",
);
assert.match(
  vcRedistTempPreflight,
  /Join-Path \$windowsRootPath "Installer"/,
  "preflight must target the Windows Installer directory",
);
assert.match(
  vcRedistTempPreflight,
  /S-1-5-18/,
  "SYSTEM must keep access to the repaired directories",
);
assert.match(
  vcRedistTempPreflight,
  /S-1-5-32-544/,
  "Administrators must keep access to the repaired directories",
);
// SID constants alone are not a behavior pin: require the full grant chain
// (both SIDs in the applied set, rule granted and written back, every ensured
// directory running the grant) so deleting the ACL repair still fails here.
assert.match(
  vcRedistTempPreflight,
  /\$requiredSids = @\(\$systemSid, \$administratorsSid\)/,
  "SYSTEM and Administrators SIDs must form the required-access set",
);
assert.match(
  vcRedistTempPreflight,
  /\[void\]\$acl\.AddAccessRule\(\$rule\)/,
  "the required-access set must be granted through ACL access rules",
);
assert.match(
  vcRedistTempPreflight,
  /Set-Acl -LiteralPath \$Path -AclObject \$acl/,
  "granted ACL rules must be written back to the repaired directory",
);
assert.match(
  vcRedistTempPreflight,
  /Add-RequiredAccess -Path \$Path/,
  "every ensured system directory must receive the required ACL entries",
);
assert.match(
  vcRedistTempPreflight,
  /Ensure-SystemDirectory -Path \$windowsTempPath/,
  "the Windows temp directory must run the ACL-preserving ensure",
);
assert.match(
  vcRedistTempPreflight,
  /Ensure-SystemDirectory -Path \$windowsInstallerPath -HiddenSystem/,
  "the Windows Installer directory must run the ACL-preserving ensure",
);
assert.match(
  vcRedistTempPreflight,
  /SetEnvironmentVariable\(\$Name, \$FallbackPath, "Machine"\)/,
  "machine TEMP/TMP repair must persist the resolved absolute fallback path",
);
assert.doesNotMatch(
  vcRedistTempPreflight,
  /FallbackValue|%SystemRoot%\\Temp/u,
  "machine TEMP/TMP repair must not persist an unexpanded REG_SZ value",
);
assert.doesNotMatch(
  vcRedistTempPreflight,
  /Remove-Item|RemoveAccessRule|PurgeAccessRules/u,
  "VC++ temp repair must not delete installer cache or existing ACL entries",
);
// PowerShell returns every uncaptured pipeline value from a function, so
// diagnostics written with Write-Output would be concatenated into the repaired
// path. The repaired paths travel through [ref] parameters instead.
assert.match(
  vcRedistTempPreflight,
  /-ResolvedPath \(\[ref\]\$machineTemp\)/u,
  "TEMP repair must return its path separately from diagnostic output",
);
assert.match(
  vcRedistTempPreflight,
  /-ResolvedPath \(\[ref\]\$machineTmp\)/u,
  "TMP repair must return its path separately from diagnostic output",
);
assert.doesNotMatch(
  vcRedistTempPreflight,
  /Write-Output "(?:Repaired|required|Created|Machine|\$Target)/iu,
  "preflight diagnostics must not contaminate PowerShell function return values",
);
if (process.platform === "win32") {
  // Run the real script twice against a fake Windows root: the first run
  // creates the directories, the second must be idempotent. Machine-level
  // TEMP/TMP changes are skipped so the smoke never needs elevation.
  const smokeRoot = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-vcredist-preflight-"));
  const fakeWindowsRoot = path.join(smokeRoot, "Windows");
  try {
    for (let attempt = 1; attempt <= 2; attempt += 1) {
      const result = spawnSync(
        "powershell.exe",
        [
          "-NoProfile",
          "-ExecutionPolicy",
          "Bypass",
          "-File",
          vcRedistTempPreflightPath,
          "-WindowsRoot",
          fakeWindowsRoot,
          "-SkipMachineEnvironment",
        ],
        { encoding: "utf8" },
      );
      assert.equal(
        result.status,
        0,
        `VC++ temp preflight attempt ${attempt} failed: ${result.stderr || result.stdout}`,
      );
    }
    assert.ok(fs.statSync(path.join(fakeWindowsRoot, "Temp")).isDirectory());
    assert.ok(fs.statSync(path.join(fakeWindowsRoot, "Installer")).isDirectory());
  } finally {
    fs.rmSync(smokeRoot, { recursive: true, force: true });
  }
}
for (const [name, version] of [
  ["MAJOR", vcMajor],
  ["MINOR", vcMinor],
  ["BUILD", vcBuild],
  ["REVISION", vcRevision],
]) {
  assert.ok(
    installerHook.includes(`!define PINVOU_VC_REDIST_MIN_${name} ${version}`),
    `NSIS VC++ ${name.toLowerCase()} minimum must match the runtime lock`,
  );
}
for (const registryValue of ["Major", "Minor", "Bld", "Rbld"]) {
  assert.match(
    installerHook,
    new RegExp(`ReadRegDWORD \\$\\d[^\\r\\n]+"${registryValue}"`),
    `NSIS hook must inspect the installed VC++ ${registryValue} value`,
  );
}
assert.match(
  installerHook,
  /IntCmpU \$1 \$\{PINVOU_VC_REDIST_MIN_MAJOR\} pinvou_vc_redist_check_minor pinvou_vc_redist_install pinvou_vc_redist_ready/,
);
assert.match(
  installerHook,
  /IntCmpU \$2 \$\{PINVOU_VC_REDIST_MIN_MINOR\} pinvou_vc_redist_check_build pinvou_vc_redist_install pinvou_vc_redist_ready/,
);
assert.match(
  installerHook,
  /IntCmpU \$3 \$\{PINVOU_VC_REDIST_MIN_BUILD\} pinvou_vc_redist_check_revision pinvou_vc_redist_install pinvou_vc_redist_ready/,
);
assert.match(
  installerHook,
  /IntCmpU \$4 \$\{PINVOU_VC_REDIST_MIN_REVISION\} pinvou_vc_redist_ready pinvou_vc_redist_install pinvou_vc_redist_ready/,
);
assert.match(
  installerHook,
  /ClearErrors\r?\n\s*ExecWait[^\r\n]+\$5\r?\n\s*IfErrors pinvou_vc_redist_exec_failed/,
  "ExecWait errors must be cleared and handled immediately",
);
assert.match(installerHook, /IntCmp \$5 3010/);
assert.match(installerHook, /IntCmp \$5 1641/);
for (const [label, nextLabel] of [
  ["pinvou_vc_redist_exec_failed", "pinvou_vc_redist_temp_failed"],
  ["pinvou_vc_redist_temp_failed", "pinvou_vc_redist_exit_failed"],
  ["pinvou_vc_redist_exit_failed", "pinvou_vc_redist_reboot"],
]) {
  const start = installerHook.indexOf(`${label}:`);
  const end = installerHook.indexOf(`${nextLabel}:`, start);
  assert.notEqual(start, -1, `NSIS hook must define ${label}`);
  assert.ok(end > start, `${label} must end before ${nextLabel}`);
  const failureBranch = installerHook.slice(start, end);
  assert.match(
    failureBranch,
    /MessageBox[^\r\n]+\/SD IDOK/,
    `${label} must not prompt indefinitely during an unattended install`,
  );
  assert.match(failureBranch, /\r?\n\s*Abort\s*(?:\r?\n|$)/, `${label} must abort the install`);
}

assert.deepEqual(windowsBundleTargets(["build"]), ["msi", "nsis"]);
assert.deepEqual(windowsBundleTargets(["build", "--bundles", "msi"]), ["msi"]);
assert.deepEqual(windowsBundleTargets(["build", "--bundles=nsis"]), ["nsis"]);
assert.ok(
  buildScript.indexOf("stageWindowsRuntime()") <
    buildScript.indexOf("stageWindowsInstaller({"),
  "runtime resolver must run before the installer adapter",
);
assert.match(
  buildScript,
  /isDev && process\.platform === "win32" \? stageWindowsOnnxRuntime\(\) : null/,
  "Windows dev must stage only the pinned ONNX Runtime before starting Tauri",
);
// Positive anchor: only a build/bundle command may widen into the full
// packaging runtime. A negative "(hasTauriBuildCommand || isDev)" scan would
// also hit the unrelated Windows toolchain-check gate above.
assert.match(
  buildScript,
  /hasTauriBuildCommand && process\.platform === "win32"\s*\?\s*stageWindowsRuntime\(\)/u,
  "only a build/bundle command must stage the complete packaging runtime",
);
assert.match(
  buildScript,
  /ORT_DYLIB_PATH:\s*runtime\.onnxRuntimeDylib/,
  "Windows dev must expose the staged ONNX Runtime to fastembed",
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

const windowsBuildJobStart = releaseWorkflow.indexOf("  build-windows-x64:");
const macosBuildJobStart = releaseWorkflow.indexOf(
  "  build-macos-universal:",
);
assert.ok(windowsBuildJobStart >= 0);
assert.ok(macosBuildJobStart > windowsBuildJobStart);

const windowsBuildJob = releaseWorkflow.slice(
  windowsBuildJobStart,
  macosBuildJobStart,
);
const windowsPrValidationStart = prWorkflow.indexOf(
  "  windows-codex-runtime-test:",
);
const macosPrValidationStart = prWorkflow.indexOf(
  "  macos-codex-runtime-test:",
);
assert.ok(windowsPrValidationStart >= 0);
assert.ok(macosPrValidationStart > windowsPrValidationStart);
const prValidationJob = prWorkflow.slice(
  windowsPrValidationStart,
  macosPrValidationStart,
);
assert.match(prValidationJob, /npm --prefix pinvou3-app run test:windows-runtime/);
assert.match(
  prValidationJob,
  /npm --prefix pinvou3-app run test:codex-acp-windows-runtime/,
);
assert.doesNotMatch(
  prValidationJob,
  /PINVOU3_WINDOWS_RUNTIME_TOKEN|secrets\./,
  "PR validation must never reference private credentials",
);

assert.match(windowsBuildJob, /github\.ref == 'refs\/heads\/main'/);
assert.match(windowsBuildJob, /environment:\s*\n\s*name: windows-release/);
assert.match(windowsBuildJob, /persist-credentials:\s*false/);
assert.doesNotMatch(
  windowsBuildJob,
  /PINVOU3_WINDOWS_RUNTIME_TOKEN|secrets\./,
  "public Windows runtime builds must not depend on cross-repository credentials",
);
assert.match(windowsBuildJob, /npm run runtime:windows:init/);
assert.doesNotMatch(
  windowsBuildJob,
  /runtime-access|available == 'true'|available != 'true'/,
  "protected Windows builds must not fall back to a reduced PR path",
);

assert.doesNotMatch(
  releaseWorkflow,
  /^\s{2}pull_request:/mu,
  "full release packaging must never run automatically for pull requests",
);
assert.match(
  releaseWorkflow,
  /push:\s*\n\s+branches: \[main\]\s*\n\s+paths:\s*\n\s+- 'VERSION'/,
);

for (const triggerPath of [
  ".gitmodules",
  "private-runtimes/windows",
  "pinvou3-app/package-lock.json",
  "pinvou3-app/scripts/tauri/**",
  "pinvou3-app/src-tauri/config/**",
  "pinvou3-app/src-tauri/packaging/**",
  "pinvou3-app/tests/windows_runtime_packaging_contract.test.js",
]) {
  const escapedPath = triggerPath.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
  const occurrences = prWorkflow.match(new RegExp(escapedPath, "gu")) ?? [];
  assert.ok(
    occurrences.length >= 1,
    `${triggerPath} must trigger the lightweight PR contract gate`,
  );
}
assert.match(
  releaseWorkflow,
  /build_required: \$\{\{ steps\.release\.outputs\.build_required \}\}/,
);

console.log("Windows runtime packaging contract: ok");
