const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const { stageWindowsInstaller } = require("../scripts/tauri/windows-installer.js");
const { describeWindowsRuntime } = require("../scripts/tauri/windows-runtime.js");

// The body needs a staged private runtime, so bare `node --test` discovery
// skips everywhere (no PR lane stages it). The `--require-staged-runtime`
// escape hatch must be passed via DIRECT invocation (`node tests/…
// --require-staged-runtime`): `node --test` drops unknown argv flags before
// they reach process.argv. The release build-windows-x64 job stages the
// runtime with `npm run runtime:windows:stage` — `runtime:windows:init`
// only materializes the private submodule and neither expands the runtime
// nor writes the descriptor — and then invokes this file directly.
const descriptorPath = path.join(
  __dirname,
  "..",
  "src-tauri",
  "target",
  "windows-runtime",
  "runtime-descriptor.json",
);
const requireStagedRuntime = process.argv.includes("--require-staged-runtime");
if (requireStagedRuntime && !fs.existsSync(descriptorPath)) {
  throw new Error(
    `--require-staged-runtime was passed but no runtime descriptor exists at ${descriptorPath}; ` +
      "run `npm run runtime:windows:stage` before this test",
  );
}
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
    // "<relativeRoot>/expanded/vc_redist/..." with forward slashes; the
    // payload/ tree is only the transient copy source inside the resolver's
    // temporary root and is deleted after staging). describeWindowsRuntime
    // resolves that entry to an absolute host path, so the separators are
    // backslashes on Windows and are normalized before matching.
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
