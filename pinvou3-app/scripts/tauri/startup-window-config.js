const fs = require("node:fs");
const path = require("node:path");

const { APP_ROOT } = require("./platform-config.js");

const BASE_CONFIG_PATH = path.join(APP_ROOT, "src-tauri", "tauri.conf.json");
const STARTUP_WINDOW_PARAMETER = "startupWindow=hidden";

function addStartupWindowMarker(url) {
  if (String(url).split("#", 1)[0].includes(STARTUP_WINDOW_PARAMETER)) return String(url);
  const value = String(url);
  const hashIndex = value.indexOf("#");
  const beforeHash = hashIndex >= 0 ? value.slice(0, hashIndex) : value;
  const hash = hashIndex >= 0 ? value.slice(hashIndex) : "";
  const separator = beforeHash.includes("?") ? "&" : "?";
  return `${beforeHash}${separator}${STARTUP_WINDOW_PARAMETER}${hash}`;
}

/**
 * Tauri replaces app.windows arrays instead of merging their entries. Generate
 * the Linux startup overlay from the base config so size and window behavior
 * remain defined in exactly one tracked source.
 */
function linuxStartupWindowConfig({ readFile = fs.readFileSync } = {}) {
  const base = JSON.parse(readFile(BASE_CONFIG_PATH, "utf8"));
  const windows = base.app?.windows;
  if (!Array.isArray(windows) || windows.length === 0) {
    throw new Error("基础 Tauri 配置缺少 app.windows");
  }

  let mainWindowCount = 0;
  const startupWindows = windows.map((windowConfig) => {
    if (windowConfig.label !== "main") return windowConfig;
    mainWindowCount += 1;
    return {
      ...windowConfig,
      url: addStartupWindowMarker(windowConfig.url),
      visible: false,
    };
  });
  if (mainWindowCount !== 1) {
    throw new Error(`基础 Tauri 配置必须且只能定义一个 main 窗口，实际为 ${mainWindowCount}`);
  }
  return { app: { windows: startupWindows } };
}

function linuxStartupWindowConfigSpec(options) {
  return JSON.stringify(linuxStartupWindowConfig(options));
}

module.exports = {
  BASE_CONFIG_PATH,
  STARTUP_WINDOW_PARAMETER,
  addStartupWindowMarker,
  linuxStartupWindowConfig,
  linuxStartupWindowConfigSpec,
};
