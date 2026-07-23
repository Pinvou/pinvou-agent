const { spawnSync } = require("node:child_process");
const path = require("node:path");
const { writeEffectiveArtifacts } = require("./effective-config.js");
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
  } = {},
) {
  const prepared = [...args];
  const commandIndex = tauriCommandIndex(prepared);
  if (commandIndex < 0) return prepared;

  const automaticConfigs = [platformConfigPath(platform)];
  const architectureConfig = platformArchitectureConfigPath(platform, architecture);
  if (architectureConfig) automaticConfigs.push(architectureConfig);
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

function prepareLinuxCodexBridge({ platform = process.platform, spawn = spawnSync } = {}) {
  if (platform !== "linux") return;
  const script = path.join(APP_ROOT, "scripts", "prepare-codex-bridge-runtime.sh");
  const child = spawn(script, [], {
    cwd: APP_ROOT,
    stdio: "inherit",
  });
  if (child.error) throw child.error;
  if (child.status !== 0) {
    throw new Error(`准备 Codex ACP Bridge 失败，退出码: ${child.status ?? "unknown"}`);
  }
}

function main() {
  const args = process.argv.slice(2);
  const validateOnly = args[0] === "--validate-only";
  if (validateOnly) args.shift();

  if (validateOnly) return;

  const preparedArgs = prepareTauriArgs(args);
  if (tauriCommandIndex(preparedArgs) >= 0) {
    prepareLinuxArm64Connectors();
    prepareWebTemplate();
    prepareLinuxCodexBridge();
    const artifacts = writeEffectiveArtifacts(configSpecs(preparedArgs));
    console.log(`[build] 有效 Tauri 配置: ${artifacts.effectiveConfigPath}`);
    console.log(
      `[build] 安装包资源清单: ${artifacts.resourceManifestPath} (${artifacts.resourceManifest.resourceFileCount} files)`,
    );
  }

  const tauriCli = require.resolve("@tauri-apps/cli/tauri.js");
  const child = spawnSync(process.execPath, [tauriCli, ...preparedArgs], {
    cwd: APP_ROOT,
    env: { ...process.env, [WRAPPER_ENV]: "1" },
    stdio: "inherit",
  });
  if (child.error) throw child.error;
  process.exitCode = child.status === null ? 1 : child.status;
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
  prepareTauriArgs,
  tauriCommandIndex,
};
