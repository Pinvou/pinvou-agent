const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");
const { writeEffectiveArtifacts } = require("./effective-config.js");
const {
  prepareCodexBridge,
  prepareWindowsCodexBridge,
  WINDOWS_BRIDGE_CONFIG_PATH,
} = require("./codex-bridge.js");
const {
  APP_ROOT,
  platformArchitectureConfigPath,
  platformConfigPath,
} = require("./platform-config.js");
const { linuxStartupWindowConfigSpec } = require("./startup-window-config.js");
const { WRAPPER_ENV } = require("./require-wrapper.js");
const { stageWindowsInstaller } = require("./windows-installer.js");
const {
  stageWindowsOnnxRuntime,
  stageWindowsRuntime,
} = require("./windows-runtime.js");

const LINUX_SUPERVISOR_MANIFEST = path.join(
  APP_ROOT,
  "src-tauri",
  "packaging",
  "linux",
  "supervisor",
  "Cargo.toml",
);
const LINUX_SUPERVISOR_TARGET_DIR = path.join(
  APP_ROOT,
  "src-tauri",
  "target",
  "pinvou-supervisor",
);
const LINUX_SUPERVISOR_BINARY = path.join(
  LINUX_SUPERVISOR_TARGET_DIR,
  "release",
  "pinvou-supervisor",
);

function nativeLinuxArchitecture(architecture = process.arch) {
  if (architecture === "x64") {
    return { rustTarget: "x86_64-unknown-linux-gnu", elfMachine: 62, debArchitecture: "amd64" };
  }
  if (architecture === "arm64") {
    return { rustTarget: "aarch64-unknown-linux-gnu", elfMachine: 183, debArchitecture: "arm64" };
  }
  throw new Error(`pinvou-supervisor does not support Linux architecture ${architecture}`);
}

function explicitTauriTarget(args = []) {
  const targets = [];
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === "--target") {
      if (!args[index + 1]) throw new Error("--target 缺少 target triple");
      targets.push(args[index + 1]);
      index += 1;
    } else if (args[index].startsWith("--target=")) {
      targets.push(args[index].slice("--target=".length));
    }
  }
  const unique = [...new Set(targets)];
  if (unique.length > 1) throw new Error(`conflicting Tauri targets: ${unique.join(", ")}`);
  return unique[0] || null;
}

function verifyElfArchitecture(file, expectedMachine, read = fs.readFileSync) {
  const header = read(file).subarray(0, 20);
  if (
    header.length < 20
    || header[0] !== 0x7f
    || header[1] !== 0x45
    || header[2] !== 0x4c
    || header[3] !== 0x46
    || header[4] !== 2
    || header[5] !== 1
  ) {
    throw new Error("pinvou-supervisor is not a 64-bit little-endian ELF binary");
  }
  const actualMachine = header.readUInt16LE(18);
  if (actualMachine !== expectedMachine) {
    throw new Error(
      `pinvou-supervisor ELF machine mismatch: expected ${expectedMachine}, got ${actualMachine}`,
    );
  }
}

function prepareLinuxSupervisor({
  platform = process.platform,
  architecture = process.arch,
  tauriArgs = [],
  spawn = spawnSync,
  exists = fs.existsSync,
  chmod = fs.chmodSync,
  executable = (file) => {
    fs.accessSync(file, fs.constants.X_OK);
    return true;
  },
  verifyElf = verifyElfArchitecture,
} = {}) {
  if (platform !== "linux") return null;
  const native = nativeLinuxArchitecture(architecture);
  const requestedTarget = explicitTauriTarget(tauriArgs);
  if (requestedTarget && requestedTarget !== native.rustTarget) {
    throw new Error(
      `cross-target Linux packaging is refused: Tauri target ${requestedTarget} cannot use native ${native.rustTarget} supervisor`,
    );
  }
  const args = [
    "build",
    "--release",
    "--locked",
    "--manifest-path",
    LINUX_SUPERVISOR_MANIFEST,
    "--target-dir",
    LINUX_SUPERVISOR_TARGET_DIR,
  ];
  const result = spawn("cargo", args, {
    cwd: APP_ROOT,
    env: process.env,
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`pinvou-supervisor release build failed (${result.status})`);
  }
  if (!exists(LINUX_SUPERVISOR_BINARY)) {
    throw new Error(`pinvou-supervisor release binary missing: ${LINUX_SUPERVISOR_BINARY}`);
  }
  chmod(LINUX_SUPERVISOR_BINARY, 0o755);
  if (!executable(LINUX_SUPERVISOR_BINARY)) {
    throw new Error(`pinvou-supervisor release binary is not executable: ${LINUX_SUPERVISOR_BINARY}`);
  }
  verifyElf(LINUX_SUPERVISOR_BINARY, native.elfMachine);
  return LINUX_SUPERVISOR_BINARY;
}

function linuxDebRequested(args) {
  if (args.includes("--no-bundle")) return false;
  const explicit = [];
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--bundles" || argument === "-b") {
      if (!args[index + 1]) throw new Error(`${argument} 缺少 bundle 类型`);
      explicit.push(args[index + 1]);
      index += 1;
    } else if (argument.startsWith("--bundles=")) {
      explicit.push(argument.slice("--bundles=".length));
    }
  }
  if (explicit.length === 0) return true;
  return explicit.flatMap((value) => value.split(",")).some((value) => value === "deb" || value === "all");
}

function verifyLinuxDebArchitecture({
  platform = process.platform,
  architecture = process.arch,
  targetDirectory = path.join(APP_ROOT, "src-tauri", "target"),
  spawn = spawnSync,
  exists = fs.existsSync,
  readdir = fs.readdirSync,
  stat = fs.statSync,
} = {}) {
  if (platform !== "linux") return null;
  const native = nativeLinuxArchitecture(architecture);
  const debDirectory = path.join(targetDirectory, "release", "bundle", "deb");
  if (!exists(debDirectory)) {
    throw new Error(`Linux deb output directory is missing: ${debDirectory}`);
  }
  const candidates = readdir(debDirectory)
    .filter((name) => name.endsWith(".deb"))
    .map((name) => path.join(debDirectory, name))
    .sort((left, right) => stat(right).mtimeMs - stat(left).mtimeMs);
  if (candidates.length === 0) throw new Error("Linux build produced no deb artifact");
  const dpkgDeb = ["/usr/bin/dpkg-deb", "/bin/dpkg-deb"].find(exists);
  if (!dpkgDeb) throw new Error("dpkg-deb is required to verify Linux package architecture");
  const artifact = candidates[0];
  const result = spawn(dpkgDeb, ["--field", artifact, "Architecture"], {
    cwd: APP_ROOT,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`cannot inspect deb architecture (${result.status}): ${result.stderr || ""}`);
  }
  const actual = String(result.stdout || "").trim();
  if (actual !== native.debArchitecture) {
    throw new Error(
      `Linux deb architecture mismatch: expected ${native.debArchitecture}, got ${actual || "empty"}`,
    );
  }
  return artifact;
}

function tauriCommandIndex(args) {
  return args.findIndex((argument) => argument === "build" || argument === "bundle");
}

function configSpecs(args) {
  const specs = [];
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === "--config" || args[index] === "-c") {
      if (!args[index + 1]) throw new Error("--config 缺少配置值");
      specs.push(args[index + 1]);
      index += 1;
    } else if (args[index].startsWith("--config=")) {
      specs.push(args[index].slice("--config=".length));
    }
  }
  return specs;
}

function windowsBundleTargets(args) {
  const explicit = [];
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--bundles" || argument === "-b") {
      if (!args[index + 1]) throw new Error(`${argument} 缺少 bundle 类型`);
      explicit.push(args[index + 1]);
      index += 1;
    } else if (argument.startsWith("--bundles=")) {
      explicit.push(argument.slice("--bundles=".length));
    }
  }
  if (explicit.length === 0 || explicit.includes("all")) return ["msi", "nsis"];
  return [...new Set(explicit.flatMap((value) => value.split(",")).filter(Boolean))];
}

function prepareTauriArgs(
  args,
  {
    platform = process.platform,
    architecture = process.arch,
    stageRuntime = stageWindowsRuntime,
    additionalConfigs = [],
  } = {},
) {
  const prepared = [...args];
  const commandIndex = tauriCommandIndex(prepared);
  if (commandIndex < 0) {
    // dev 不注入 packaging overlay。macOS 复用平台 overlay 保持原生顶栏一致；
    // Linux 只注入 dev overlay，让冷启动窗口等 React 首次提交后再显示，避开
    // Mutter/XWayland 首次映射期间视觉表面与输入表面短暂错位。
    const devIndex = prepared.indexOf("dev");
    const devConfig = platform === "darwin"
      ? platformConfigPath(platform)
      : platform === "linux"
        ? linuxStartupWindowConfigSpec()
        : null;
    if (devIndex >= 0 && devConfig) {
      // 与 build/bundle 保持相同优先级:自动平台配置在前,调用方显式
      // --config 在后,从而仍可有意覆盖平台默认值。
      prepared.splice(devIndex + 1, 0, "--config", devConfig);
    }
    return prepared;
  }

  const automaticConfigs = [platformConfigPath(platform)];
  if (platform === "linux") automaticConfigs.push(linuxStartupWindowConfigSpec());
  const architectureConfig = platformArchitectureConfigPath(platform, architecture);
  if (architectureConfig) automaticConfigs.push(architectureConfig);
  const stagedRuntime = stageRuntime({ platform });
  const runtimeConfig =
    typeof stagedRuntime === "string" ? stagedRuntime : stagedRuntime?.configPath;
  if (runtimeConfig) automaticConfigs.push(runtimeConfig);
  automaticConfigs.push(...additionalConfigs);
  const injected = automaticConfigs.flatMap((configPath) => ["--config", configPath]);
  // Automatic overlays must precede explicit signing/staging overlays so the
  // caller can intentionally override or remove inherited resource mappings.
  prepared.splice(commandIndex + 1, 0, ...injected);
  return prepared;
}

function runTauri(preparedArgs, spawn = spawnSync, environment = process.env) {
  const tauriCli = require.resolve("@tauri-apps/cli/tauri.js");
  const child = spawn(process.execPath, [tauriCli, ...preparedArgs], {
    cwd: APP_ROOT,
    env: { ...environment, [WRAPPER_ENV]: "1" },
    stdio: "inherit",
  });
  if (child.error) throw child.error;
  return child.status === null ? 1 : child.status;
}

function tauriRuntimeEnvironment(runtime, environment = process.env) {
  return runtime
    ? { ...environment, ORT_DYLIB_PATH: runtime.onnxRuntimeDylib }
    : environment;
}

function main() {
  const args = process.argv.slice(2);
  const validateOnly = args[0] === "--validate-only";
  if (validateOnly) args.shift();

  if (validateOnly) return;

  const isDev = args.includes("dev");
  const hasTauriBuildCommand = tauriCommandIndex(args) >= 0;
  const additionalConfigs = [];
  // Windows 的 fastembed 使用动态 ONNX Runtime。正式包 staging 完整运行时并通过
  // resource overlay 携带 DLL；dev 只校验并展开 ONNX 组件，避免为 UI 开发准备无关工具。
  const windowsRuntime =
    hasTauriBuildCommand && process.platform === "win32"
      ? stageWindowsRuntime()
      : null;
  const windowsDevRuntime =
    isDev && process.platform === "win32" ? stageWindowsOnnxRuntime() : null;
  if (windowsRuntime && hasTauriBuildCommand) {
    stageWindowsInstaller({
      bundleTargets: windowsBundleTargets(args),
      runtime: windowsRuntime,
    });
  }
  const windowsBridgeOptions = windowsRuntime
    ? {
        nodeExecutable: windowsRuntime.nodeExecutable,
        npmExecPath: windowsRuntime.npmExecPath,
      }
    : undefined;
  if (isDev) {
    prepareCodexBridge();
    prepareWindowsCodexBridge();
  }
  if (hasTauriBuildCommand) {
    prepareCodexBridge();
    prepareWindowsCodexBridge(windowsBridgeOptions);
    prepareLinuxSupervisor({ tauriArgs: args });
    if (process.platform === "win32") {
      additionalConfigs.push(WINDOWS_BRIDGE_CONFIG_PATH);
    }
  }

  const preparedArgs = prepareTauriArgs(args, {
    additionalConfigs,
    stageRuntime: () => windowsRuntime,
  });
  if (hasTauriBuildCommand) {
    const artifacts = writeEffectiveArtifacts(configSpecs(preparedArgs));
    console.log(`[build] 有效 Tauri 配置: ${artifacts.effectiveConfigPath}`);
    console.log(
      `[build] 安装包资源清单: ${artifacts.resourceManifestPath} (${artifacts.resourceManifest.resourceFileCount} files)`,
    );
  }

  const tauriEnvironment = tauriRuntimeEnvironment(windowsRuntime || windowsDevRuntime);
  const exitCode = runTauri(preparedArgs, undefined, tauriEnvironment);
  if (
    exitCode === 0
    && hasTauriBuildCommand
    && process.platform === "linux"
    && linuxDebRequested(args)
  ) {
    const requestedTarget = explicitTauriTarget(args);
    verifyLinuxDebArchitecture({
      targetDirectory: path.join(
        APP_ROOT,
        "src-tauri",
        "target",
        ...(requestedTarget ? [requestedTarget] : []),
      ),
    });
  }
  process.exitCode = exitCode;
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`[build] ${error.message}`);
    process.exitCode = 1;
  }
}

module.exports = {
  configSpecs,
  main,
  explicitTauriTarget,
  linuxDebRequested,
  nativeLinuxArchitecture,
  prepareCodexBridge,
  prepareLinuxSupervisor,
  prepareWindowsCodexBridge,
  stageWindowsInstaller,
  stageWindowsOnnxRuntime,
  stageWindowsRuntime,
  prepareTauriArgs,
  runTauri,
  tauriRuntimeEnvironment,
  tauriCommandIndex,
  verifyElfArchitecture,
  verifyLinuxDebArchitecture,
  windowsBundleTargets,
};
