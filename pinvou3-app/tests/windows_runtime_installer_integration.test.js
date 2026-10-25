const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const { stageWindowsInstaller } = require("../scripts/tauri/windows-installer.js");
const { describeWindowsRuntime } = require("../scripts/tauri/windows-runtime.js");

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
    assert.match(
      runtime.vcRedist.sourcePath.replaceAll("\\", "/"),
      /target\/windows-runtime\/[^/]+\/payload\/vc_redist\/VC_redist\.x64\.exe$/u,
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
