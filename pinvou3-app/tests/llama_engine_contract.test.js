// 本地多模态引擎（features/llama_engine）契约测试。
// 读 Rust/前端源码做正则断言，锁定必须保留的接线与安全约定
// （模式照 codex_acp_windows_contract.test.js / connector_online_install_contract.test.js）。
"use strict";

const fs = require("fs");
const path = require("path");
const assert = require("assert");

const ROOT = path.join(__dirname, "..");
const SRC_T = path.join(ROOT, "src-tauri", "src");

function read(rel) {
  return fs.readFileSync(path.join(SRC_T, rel), "utf8");
}
function readRoot(rel) {
  return fs.readFileSync(path.join(ROOT, rel), "utf8");
}

// 1. bridge.rs 接线：resolve_vision_model_config 必须以本地引擎为最高优先级规则
//    （引擎运行中 → 本地端点；引擎停止 → 回落 vision_model_id / 主模型复用规则），
//    且不引入 --alias（单模型模式忽略请求体 model 字段）。
const bridge = read("features/assistant/platform/bridge.rs");
assert(
  /fn resolve_vision_model_config[\s\S]*?llama_engine::vision_endpoint\(\)/.test(bridge),
  "resolve_vision_model_config 必须接入 llama_engine::vision_endpoint()（本地引擎最高优先级）"
);
assert(
  bridge.includes("vision_model_id"),
  "resolve_vision_model_config 必须保留 vision_model_id 配置规则（native-image-input 集成）"
);
assert(
  bridge.includes("llama_engine_vision_fallback"),
  "resolve_vision_model_config 必须支持 llama_engine_vision_fallback 开关（用户可关闭本地引擎视觉兜底）"
);
assert(!/--alias/.test(bridge), "bridge.rs 不得引入 --alias");

// 2. 命令注册：6 条 llama_engine_* 命令齐全，且 #[tauri::command] 只在 app/commands/ 宿主。
const commands = read("app/commands/llama_engine.rs");
for (const name of [
  "llama_engine_status",
  "llama_engine_install_engine",
  "llama_engine_install_model",
  "llama_engine_cancel_download",
  "llama_engine_start",
  "llama_engine_stop",
]) {
  assert(
    new RegExp(name).test(commands),
    `commands/llama_engine.rs 必须包含 ${name}`
  );
}
const lib = readRoot("src-tauri/src/lib.rs");
for (const name of [
  "llama_engine_status",
  "llama_engine_install_engine",
  "llama_engine_install_model",
  "llama_engine_cancel_download",
  "llama_engine_start",
  "llama_engine_stop",
]) {
  assert(
    new RegExp(`commands::llama_engine::${name}`).test(lib),
    `lib.rs generate_handler 必须注册 ${name}`
  );
}

// 3. 平台适配边界：cfg(target_os) 只能出现在 features/llama_engine/platform/ 下
//    （架构守卫 rust_target_cfg_outside_adapter）。
const fsMod = require("fs");
const fsPath = require("path");
const llamaDir = fsPath.join(SRC_T, "features", "llama_engine");
function collectRustFiles(dir) {
  let out = [];
  for (const entry of fsMod.readdirSync(dir, { withFileTypes: true })) {
    const full = fsPath.join(dir, entry.name);
    if (entry.isDirectory()) out = out.concat(collectRustFiles(full));
    else if (entry.name.endsWith(".rs")) out.push(full);
  }
  return out;
}
const cfgOutsidePlatform = [];
for (const file of collectRustFiles(llamaDir)) {
  const rel = fsPath.relative(llamaDir, file).replace(/\\/g, "/");
  if (rel.startsWith("platform/")) continue;
  const text = fsMod.readFileSync(file, "utf8");
  if (/cfg\s*\(\s*target_os/.test(text)) cfgOutsidePlatform.push(rel);
}
assert.deepStrictEqual(
  cfgOutsidePlatform,
  [],
  `cfg(target_os) 只能位于 platform/ 下，发现: ${cfgOutsidePlatform.join(", ")}`
);
assert(
  /llama-\{tag\}-bin-win-vulkan-x64\.zip/.test(read("features/llama_engine/platform/windows.rs")),
  "windows 平台资产名必须使用 win-vulkan 包（CPU+Vulkan 一体）"
);

// 4. 下载安全：模型/引擎 URL 全部 https；环境变量覆盖存在（测试/镜像用）。
const download = read("features/llama_engine/download.rs");
const urlConsts = download.match(/primary_url:\s*"[^"]+"/g) || [];
for (const line of urlConsts) {
  assert(/https:\/\//.test(line), `模型下载 URL 必须为 https: ${line}`);
}
for (const envName of ["PINVOU3_LLAMA_MODEL_URL", "PINVOU3_LLAMA_ENGINE_TAG", "PINVOU3_LLAMA_MODEL_SHA256"]) {
  assert(download.includes(envName), `download.rs 必须支持 ${envName} 覆盖`);
}

// 5. 引擎只下载到用户目录（~/.pinvou3/llama-engine/），不引用 resources/common。
const modFile = read("features/llama_engine/mod.rs");
assert(/pinvou3_home\(\)\s*\.\s*join\("llama-engine"\)/.test(modFile), "引擎目录必须在 ~/.pinvou3/llama-engine/");
for (const file of collectRustFiles(llamaDir)) {
  const text = fsMod.readFileSync(file, "utf8");
  assert(!/resources\s*[\\/]\s*common/.test(text), `llama_engine 不得引用 resources/common（${file}）`);
}

// 6. 前端接线：事件 listen、状态 slice、useBridgeState 域齐全。
const chatEvents = readRoot("src/platform/tauri/bridge/chat-events.js");
assert(chatEvents.includes('listen("llama-engine:progress"'), "chat-events.js 必须监听 llama-engine:progress");
assert(chatEvents.includes('listen("llama-engine:state"'), "chat-events.js 必须监听 llama-engine:state");
const bridgeJs = readRoot("src/platform/tauri/bridge.js");
assert(/llamaEngine:\s*\["llamaEngineSetup"\]/.test(bridgeJs), "bridge.js STATE_SLICE_FIELDS 必须含 llamaEngineSetup");
assert(/"llama-engine"/.test(bridgeJs), "bridge.js 必须安装 llama-engine feature");
const mainJsx = readRoot("src/app/main.jsx");
assert(/'llamaEngine'/.test(mainJsx), "main.jsx useBridgeState 域列表必须含 llamaEngine");

// 7. 三语 i18n 齐平（zh/en/ja 都要有 llamaEngine 文案组）。
const settingsI18n = readRoot("src/features/settings/settings-i18n.js");
for (const lang of ["zh", "en", "ja"]) {
  assert(
    new RegExp(`dict\\.${lang}\\.uiSettingsDetail\\.llamaEngine`).test(settingsI18n),
    `settings-i18n.js 必须为 ${lang} 提供 llamaEngine 文案`
  );
}
const i18n = readRoot("src/shared/i18n.js");
for (const label of ["本地多模态引擎", "Local Multimodal Engine", "ローカルマルチモーダルエンジン"]) {
  assert(i18n.includes(label), `i18n.js 必须包含导航文案: ${label}`);
}
const settingsView = readRoot("src/features/settings/SettingsView.jsx");
assert(settingsView.includes("activeSection === 'llama'"), "SettingsView 必须分发 llama 区块");
assert(/id="llama"/.test(settingsView), "SettingsView 必须注册 llama SectionButton");

console.log("llama_engine_contract ok");
