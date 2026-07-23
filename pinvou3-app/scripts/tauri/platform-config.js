const path = require("node:path");

const APP_ROOT = path.resolve(__dirname, "..", "..");
const PLATFORM_CONFIG_NAMES = {
  darwin: path.join("platforms", "macos", "tauri.conf.json"),
  linux: path.join("platforms", "linux", "tauri.conf.json"),
  win32: path.join("platforms", "windows", "tauri.conf.json"),
};
const ARCHITECTURE_CONFIG_NAMES = {
  linux: {
    arm64: path.join("platforms", "linux", "aarch64.tauri.conf.json"),
  },
};

function platformConfigPath(platform = process.platform) {
  const configName = PLATFORM_CONFIG_NAMES[platform];
  if (!configName) {
    throw new Error(`当前构建平台没有对应的 Tauri 配置：${platform}`);
  }
  return path.resolve(APP_ROOT, "src-tauri", "config", configName);
}

function platformArchitectureConfigPath(
  platform = process.platform,
  architecture = process.arch,
) {
  const configName = ARCHITECTURE_CONFIG_NAMES[platform]?.[architecture];
  return configName
    ? path.resolve(APP_ROOT, "src-tauri", "config", configName)
    : null;
}

module.exports = {
  APP_ROOT,
  ARCHITECTURE_CONFIG_NAMES,
  PLATFORM_CONFIG_NAMES,
  platformArchitectureConfigPath,
  platformConfigPath,
};
