const path = require("node:path");
const fs = require("node:fs");
const { spawnSync } = require("node:child_process");

const { APP_ROOT } = require("./platform-config.js");

const REPOSITORY_ROOT = path.resolve(APP_ROOT, "..");
const BUILD_SCRIPT = path.join(
  REPOSITORY_ROOT,
  "scripts",
  "asr",
  "build-sensevoice-runtime.sh",
);
const LINUX_ASR_ARCHITECTURES = {
  arm64: { directory: "aarch64", scriptArch: "aarch64" },
  // deepin/UOS V25 ships loong64; Node's process.arch uses the same word as
  // the dpkg architecture, while the Rust triple / output directory spell it
  // loongarch64.
  loong64: { directory: "loongarch64", scriptArch: "loongarch64" },
  x64: { directory: "x86_64", scriptArch: "x86_64" },
};

// `outputRoot` is the injection point for tests (same discipline as the
// knowledge-host fixture roots): everything under it is `<outputRoot>/
// src-tauri/target/linux-asr-runtime/<dir>/sense-voice-main`, so a test can
// exercise the prebuilt/cross paths without writing a fake engine into the
// real target tree, where a killed run would leave an artifact that
// `PINVOU3_LINUX_ASR_PREBUILT=1` accepts.
function linuxAsrRuntimeOutput(architecture = process.arch, { outputRoot = APP_ROOT } = {}) {
  const descriptor = LINUX_ASR_ARCHITECTURES[architecture];
  if (!descriptor) {
    throw new Error(`Linux SenseVoice runtime 暂不支持 ${architecture} 架构`);
  }
  return {
    ...descriptor,
    binaryPath: path.join(
      outputRoot,
      "src-tauri",
      "target",
      "linux-asr-runtime",
      descriptor.directory,
      "sense-voice-main",
    ),
  };
}

function prebuiltEngineFile(binaryPath) {
  try {
    return fs.statSync(binaryPath).isFile() ? binaryPath : null;
  } catch {
    return null;
  }
}

function prepareLinuxAsrRuntime({
  platform = process.platform,
  architecture = process.arch,
  environment = process.env,
  spawn = spawnSync,
  prebuilt = false,
  outputRoot = APP_ROOT,
} = {}) {
  if (platform !== "linux") return null;
  const runtime = linuxAsrRuntimeOutput(architecture, { outputRoot });
  if (prebuilt) {
    // The prebuilt engine is the packaging-time input produced by the release
    // pipeline: the pipeline runs build-sensevoice-runtime.sh for the target
    // architecture ahead of packaging (in its cross mode for loongarch64,
    // natively for x86_64/aarch64) and leaves the result at the same path the
    // script itself would write, so this step only verifies and accepts it
    // (same contract as the script's own cached-build fast path). The skip is
    // opt-in on purpose: skipping whenever a file happens to exist would
    // bypass the script's commit-pinned cache invalidation and could package
    // a stale engine.
    // A missing artifact fails loudly - the architecture overlay still
    // declares the file, so a silent skip would fail the bundle later with a
    // much less specific error.
    if (!prebuiltEngineFile(runtime.binaryPath)) {
      throw new Error(
        `预构建的 SenseVoice 引擎不存在: ${runtime.binaryPath}（跨架构打包需先提供`
          + ` ${runtime.scriptArch} 引擎，或改设 PINVOU3_SKIP_LINUX_ASR=1）`,
      );
    }
    console.log(`[linux-asr-runtime] 使用预构建引擎: ${runtime.binaryPath}`);
    return runtime;
  }
  const result = spawn(
    "bash",
    [BUILD_SCRIPT, "--output", runtime.binaryPath, "--arch", runtime.scriptArch],
    { cwd: REPOSITORY_ROOT, env: environment, stdio: "inherit" },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`Linux SenseVoice runtime 构建失败，退出码：${result.status ?? "unknown"}`);
  }
  return runtime;
}

module.exports = {
  BUILD_SCRIPT,
  LINUX_ASR_ARCHITECTURES,
  linuxAsrRuntimeOutput,
  prepareLinuxAsrRuntime,
  prebuiltEngineFile,
};
