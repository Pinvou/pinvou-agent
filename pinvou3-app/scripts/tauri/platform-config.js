const path = require("node:path");

const APP_ROOT = path.resolve(__dirname, "..", "..");
const PLATFORM_CONFIG_NAMES = {
  darwin: path.join("platforms", "macos", "tauri.conf.json"),
  linux: path.join("platforms", "linux", "tauri.conf.json"),
  win32: path.join("platforms", "windows", "tauri.conf.json"),
};

function platformConfigPath(platform = process.platform) {
  const configName = PLATFORM_CONFIG_NAMES[platform];
  if (!configName) {
    throw new Error(`当前构建平台没有对应的 Tauri 配置：${platform}`);
  }
  return path.resolve(APP_ROOT, "src-tauri", "config", configName);
}

module.exports = {
  APP_ROOT,
  PLATFORM_CONFIG_NAMES,
  platformConfigPath,
};
