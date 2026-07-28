const { spawnSync } = require("node:child_process");
const path = require("node:path");
const { writeEffectiveArtifacts } = require("./effective-config.js");
const {
  prepareLinuxCodexBridge,
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

function prepareTauriArgs(
  args,
  {
    platform = process.platform,
    architecture = process.arch,
    additionalConfigs = [],
  } = {},
) {
  const prepared = [...args];
  const commandIndex = tauriCommandIndex(prepared);
  if (commandIndex < 0) return prepared;

  const automaticConfigs = [platformConfigPath(platform)];
  const architectureConfig = platformArchitectureConfigPath(platform, architecture);
  if (architectureConfig) automaticConfigs.push(architectureConfig);
  automaticConfigs.push(...additionalConfigs);
  const injected = automaticConfigs.flatMap((configPath) => ["--config", configPath]);
  // Automatic overlays must precede explicit signing/staging overlays so the
  // caller can intentionally override or remove inherited resource mappings.
  prepared.splice(commandIndex + 1, 0, ...injected);
  return prepared;
}

function prepareLinuxArm64Connectors({
  platform = process.platform,
  architecture = process.arch,
} = {}) {
  if (platform !== "linux" || architecture !== "arm64") return;
  const script = path.resolve(APP_ROOT, "..", "scripts", "fetch-linux-arm64-connectors.sh");
  const result = spawnSync("bash", [script], {
    cwd: path.resolve(APP_ROOT, ".."),
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`Linux ARM64 连接器准备失败（退出码 ${result.status ?? "unknown"}）`);
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
  if (isDev) {
    prepareLinuxCodexBridge();
    prepareWindowsCodexBridge();
  }
  if (hasTauriBuildCommand) {
    prepareLinuxArm64Connectors();
    prepareWebTemplate();
    prepareLinuxCodexBridge();
    prepareWindowsCodexBridge();
    if (process.platform === "win32") {
      additionalConfigs.push(WINDOWS_BRIDGE_CONFIG_PATH);
    }
  }

  const preparedArgs = prepareTauriArgs(args, { additionalConfigs });
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
  prepareLinuxArm64Connectors,
  prepareLinuxCodexBridge,
  prepareWindowsCodexBridge,
  prepareTauriArgs,
  runTauri,
  tauriCommandIndex,
};
