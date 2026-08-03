const { spawnSync } = require("node:child_process");
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
const { WRAPPER_ENV } = require("./require-wrapper.js");
const { prepareWebTemplate } = require("./web-template.js");
const { stageWindowsInstaller } = require("./windows-installer.js");
const { stageWindowsRuntime } = require("./windows-runtime.js");

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
    // dev 默认不注入 packaging overlay;但 macOS 的原生红绿灯顶栏定义在平台
    // overlay 里,dev 也必须带上,否则 npm run dev 与打包产物顶栏不一致
    // (run-dev.sh 直连 tauri dev 时已显式带同一份 overlay,两条入口行为对齐)。
    const devIndex = prepared.indexOf("dev");
    if (devIndex >= 0 && platform === "darwin") {
      // 与 build/bundle 保持相同优先级:自动平台配置在前,调用方显式
      // --config 在后,从而仍可有意覆盖平台默认值。
      prepared.splice(devIndex + 1, 0, "--config", platformConfigPath(platform));
    }
    return prepared;
  }

  const automaticConfigs = [platformConfigPath(platform)];
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

/// 构建前按 connectors.lock.json 抓取内置连接器 CLI 二进制(厂家 release,
/// gitignored)。macOS 固定抓双架构:universal-apple-darwin 构建会编译 aarch64 +
/// x86_64 两份,include_dir 需要两个平台的资源目录都已物化(单架构构建走缓存
/// 基本零成本)。无内置二进制的平台组合跳过,运行时走 npm 全局安装兜底。
function prepareConnectorClis({
  platform = process.platform,
  architecture = process.arch,
  spawn = spawnSync,
} = {}) {
  const script = path.resolve(APP_ROOT, "..", "scripts", "fetch-connectors.sh");
  let platforms = [];
  if (platform === "darwin") {
    platforms = ["darwin-arm64", "darwin-x64"];
  } else if (platform === "linux" && architecture === "arm64") {
    platforms = ["linux-arm64"];
  } else if (platform === "linux" && architecture === "x64") {
    platforms = ["linux-x64"];
  } else if (platform === "win32" && architecture === "x64") {
    platforms = ["windows-x64"];
  }
  for (const connectorPlatform of platforms) {
    const result = spawn("bash", [script, "--platform", connectorPlatform], {
      cwd: path.resolve(APP_ROOT, ".."),
      stdio: "inherit",
    });
    if (result.error) throw result.error;
    if (result.status !== 0) {
      throw new Error(`连接器准备失败(${connectorPlatform},退出码 ${result.status ?? "unknown"})`);
    }
  }
}

function runTauri(preparedArgs, spawn = spawnSync) {
  const tauriCli = require.resolve("@tauri-apps/cli/tauri.js");
  const child = spawn(process.execPath, [tauriCli, ...preparedArgs], {
    cwd: APP_ROOT,
    env: { ...process.env, [WRAPPER_ENV]: "1" },
    stdio: "inherit",
  });
  if (child.error) throw child.error;
  return child.status === null ? 1 : child.status;
}

function main() {
  const args = process.argv.slice(2);
  const validateOnly = args[0] === "--validate-only";
  if (validateOnly) args.shift();

  if (validateOnly) return;

  const isDev = args.includes("dev");
  const hasTauriBuildCommand = tauriCommandIndex(args) >= 0;
  const additionalConfigs = [];
  const windowsRuntime =
    hasTauriBuildCommand && process.platform === "win32" ? stageWindowsRuntime() : null;
  if (windowsRuntime) {
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
    prepareConnectorClis();
    prepareWebTemplate();
    prepareCodexBridge();
    prepareWindowsCodexBridge(windowsBridgeOptions);
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

  process.exitCode = runTauri(preparedArgs);
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
  prepareConnectorClis,
  prepareCodexBridge,
  prepareWindowsCodexBridge,
  stageWindowsInstaller,
  stageWindowsRuntime,
  prepareTauriArgs,
  runTauri,
  tauriCommandIndex,
  windowsBundleTargets,
};
