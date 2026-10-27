const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const { stageWindowsInstaller } = require("../scripts/tauri/windows-installer.js");
const { describeWindowsRuntime } = require("../scripts/tauri/windows-runtime.js");

// The body needs a staged private runtime, so bare `node --test` discovery
// skips everywhere (the pr-check Windows lane never stages it). The
// `--require-staged-runtime` escape hatch must be passed via DIRECT
// invocation (`node tests/windows_runtime_installer_integration.test.js
// --require-staged-runtime`): `node --test` drops unknown argv flags before
// they reach process.argv. The release build-windows-x64 job invokes it
// directly right after `npm run runtime:windows:init` has staged the
// runtime and written the descriptor.
const descriptorPath = path.join(
  __dirname,
  "..",
  "src-tauri",
  "target",
  "windows-runtime",
  "runtime-descriptor.json",
);
const requireStagedRuntime = process.argv.includes("--require-staged-runtime");
const skipReason = !requireStagedRuntime && (
  process.platform !== "win32" || !fs.existsSync(descriptorPath)
)
  ? "requires a staged Windows private runtime"
  : false;

test("Windows runtime installer adapter integration", { skip: skipReason }, () => {
  assert.equal(process.platform, "win32", "this integration test requires Windows staging");

  const outputRoot = fs.mkdtempSync(path.join(os.tmpdir(), "pinvou-installer-adapter-"));
  try {
    const runtime = describeWindowsRuntime();
    // The descriptor records the EXPANDED staging destination
    // (resolve-runtime.ps1 Get-RuntimeDescriptorContent writes
    // "<relativeRoot>/expanded/vc_redist/..."; the payload/ tree is only the
    // transient copy source inside the resolver's temporary root and is
    // deleted after staging).
    assert.match(
      runtime.vcRedist.sourcePath.replaceAll("\\", "/"),
      /target\/windows-runtime\/[^/]+\/expanded\/vc_redist\/VC_redist\.x64\.exe$/u,
      "the runtime descriptor must expose the verified staged VC++ payload",
    );

    const staged = stageWindowsInstaller({
      platform: "win32",
      bundleTargets: ["nsis"],
      runtime,
      destinationRoot: outputRoot,
    });
    assert.equal(staged.vcRedistPath, path.join(outputRoot, "vc_redist", "VC_redist.x64.exe"));
    assert.equal(fs.statSync(staged.vcRedistPath).size, runtime.vcRedist.bytes);
  } finally {
    fs.rmSync(outputRoot, { recursive: true, force: true });
  }
});
