const assert = require("assert");
const fs = require("fs");
const path = require("path");

const appRoot = path.resolve(__dirname, "..");
const runtimeScript = fs.readFileSync(
  path.join(
    appRoot,
    "src-tauri",
    "packaging",
    "windows",
    "runtime",
    "scripts",
    "resolve-runtime.ps1",
  ),
  "utf8",
);
const installerHooks = fs.readFileSync(
  path.join(
    appRoot,
    "src-tauri",
    "packaging",
    "windows",
    "nsis",
    "installer-hooks.nsh",
  ),
  "utf8",
);

const stagedBootstrapper =
  "${__FILEDIR__}\\..\\..\\..\\windows-runtime\\nsis\\vc_redist\\VC_redist.x64.exe";
const removedBootstrapper =
  "${__FILEDIR__}\\..\\..\\..\\..\\resources\\windows\\vc_redist\\VC_redist.x64.exe";
const removedArchitectureBootstrapper =
  "${__FILEDIR__}\\..\\..\\..\\..\\packaging\\windows\\vc_redist\\VC_redist.x64.exe";

assert.ok(
  installerHooks.includes(stagedBootstrapper),
  "NSIS hook must load VC_redist from the verified target/windows-runtime staging area",
);
assert.ok(
  !installerHooks.includes(removedBootstrapper),
  "NSIS hook must not reference the deleted source-tree vc_redist directory",
);
assert.ok(
  !installerHooks.includes(removedArchitectureBootstrapper),
  "NSIS hook must not reference an unstaged packaging/windows vc_redist directory",
);

assert.ok(
  runtimeScript.includes('function Stage-NsisBootstrapper'),
  "runtime staging must expose a dedicated NSIS bootstrapper step",
);
assert.ok(
  runtimeScript.includes('$manifestPath = "payload/vc_redist/VC_redist.x64.exe"'),
  "VC_redist must be selected from the locked private-runtime manifest",
);
assert.ok(
  runtimeScript.includes('$destinationRoot = Join-Path $StagingParent "nsis"'),
  "VC_redist must be copied to the stable target/windows-runtime/nsis directory",
);
assert.ok(
  runtimeScript.includes('(Get-Sha256 -Path $temporaryPath) -ne [string]$entry.sha256'),
  "the NSIS bootstrapper copy must be verified against the locked SHA-256",
);

const simulatedFileDir =
  "D:\\workspace\\pinvou3-app\\src-tauri\\target\\release\\nsis\\x64";
const resolvedHookPath = path.win32.resolve(
  simulatedFileDir,
  "..\\..\\..\\windows-runtime\\nsis\\vc_redist\\VC_redist.x64.exe",
);
const expectedStagingPath =
  "D:\\workspace\\pinvou3-app\\src-tauri\\target\\windows-runtime\\nsis\\vc_redist\\VC_redist.x64.exe";
assert.strictEqual(
  resolvedHookPath.toLowerCase(),
  expectedStagingPath.toLowerCase(),
  "NSIS relative path must resolve to the stable staging destination",
);

console.log("✅ Windows runtime NSIS bootstrapper packaging contract passed");
