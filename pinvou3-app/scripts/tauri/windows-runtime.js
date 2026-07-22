const path = require("node:path");
const { spawnSync } = require("node:child_process");
const { APP_ROOT } = require("./platform-config.js");

function stageWindowsRuntime({ environment = process.env } = {}) {
  if (process.platform !== "win32") return null;

  const scriptPath = path.resolve(
    APP_ROOT,
    "src-tauri",
    "packaging",
    "windows",
    "runtime",
    "scripts",
    "stage-runtime.ps1",
  );
  const child = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", scriptPath],
    { cwd: APP_ROOT, env: environment, stdio: "inherit" },
  );
  if (child.error) throw child.error;
  if (child.status !== 0) {
    throw new Error(`Windows 私有运行时 staging 失败（退出码 ${child.status ?? "unknown"}）。`);
  }
  return path.resolve(
    APP_ROOT,
    "src-tauri",
    "target",
    "windows-runtime",
    "tauri.generated.conf.json",
  );
}

module.exports = { stageWindowsRuntime };
