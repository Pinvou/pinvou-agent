const { spawnSync } = require("node:child_process");
const { loadBuiltinSecrets } = require("./builtin-secrets.js");
const { APP_ROOT, platformConfigPath } = require("./platform-config.js");
const { stageWindowsRuntime } = require("./windows-runtime.js");

function main() {
  const args = process.argv.slice(2);
  const validateOnly = args[0] === "--validate-only";
  if (validateOnly) args.shift();

  if (process.platform === "win32") {
    const result = loadBuiltinSecrets();
    if (result.missing.length > 0) {
      console.warn(`[build] 已显式跳过 ${result.missing.length} 项内置 MCP 密钥。`);
    } else {
      console.log(`[build] 已加载并校验 ${result.loaded.length} 项内置 MCP 密钥。`);
    }
  }
  if (validateOnly) return;

  if (args[0] === "build" || args[0] === "bundle") {
    args.push("--config", platformConfigPath());
    const runtimeConfig = stageWindowsRuntime();
    if (runtimeConfig) args.push("--config", runtimeConfig);
  }

  const tauriCli = require.resolve("@tauri-apps/cli/tauri.js");
  const child = spawnSync(process.execPath, [tauriCli, ...args], {
    cwd: APP_ROOT,
    env: process.env,
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

module.exports = { main };
