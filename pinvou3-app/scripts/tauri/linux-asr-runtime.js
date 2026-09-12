const path = require("node:path");
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

function prepareLinuxAsrRuntime({
  platform = process.platform,
  architecture = process.arch,
  environment = process.env,
  spawn = spawnSync,
} = {}) {
  if (platform !== "linux") return null;
  const runtime = linuxAsrRuntimeOutput(architecture);
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
};
