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

function linuxAsrRuntimeOutput(architecture = process.arch) {
  const descriptor = LINUX_ASR_ARCHITECTURES[architecture];
  if (!descriptor) {
    throw new Error(`Linux SenseVoice runtime 暂不支持 ${architecture} 架构`);
  }
  return {
    ...descriptor,
    binaryPath: path.join(
      APP_ROOT,
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
} = {}) {
  if (platform !== "linux") return null;
  const runtime = linuxAsrRuntimeOutput(architecture);
  if (prebuilt) {
    // Cross builds cannot run the build script for the target architecture:
    // its requested_arch == host_arch assertion fires before its own cache
    // reuse, so the release pipeline pre-builds the engine for the target and
    // this step only verifies and accepts it (same contract as the script's
    // own cached-build fast path). The skip is opt-in on purpose: skipping
    // whenever a file happens to exist would bypass the script's
    // commit-pinned cache invalidation and could package a stale engine.
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
