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

// 1. bridge.rs 接线：vision_config 必须条件覆盖到本地端点（引擎运行中优先），
//    且不引入 --alias（单模型模式忽略请求体 model 字段）。
const bridge = read("features/assistant/platform/bridge.rs");
assert(
  /llama_engine::vision_endpoint\(\s*\)/.test(bridge) &&
    /unwrap_or_else\(.+self\.base_url\(\)\)/s.test(bridge),
  "bridge.rs 必须把 vision_config.base_url 条件覆盖为 llama_engine::vision_endpoint()（引擎运行中优先，否则回退主模型端点）"
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

// 8. 本地识图引擎选项 + 自动启动/关闭契约：
//    SavedModel.vision_prefer_local_engine（is_false 序列化省略）、
//    AdvancedPrefs 自动启动三字段、capability local_engine_state、
//    RunEvent::Exit 停引擎、前端哨兵/文案/发送门。
const prefsMod = read("platform/prefs/mod.rs");
assert(
  /vision_prefer_local_engine/.test(prefsMod) && /skip_serializing_if = "is_false"/.test(prefsMod),
  "SavedModel 必须含 vision_prefer_local_engine（is_false 序列化省略）"
);
for (const field of ["llama_engine_auto_start", "llama_engine_default_model", "llama_engine_default_device"]) {
  assert(prefsMod.includes(field), `AdvancedPrefs 必须含 ${field}`);
}
const settingsCmd = read("app/commands/settings.rs");
assert(settingsCmd.includes("local_engine_state"), "get_image_input_capability 必须返回 local_engine_state");
assert(
  /RunEvent::Exit[\s\S]*?llama_engine::server::stop\(\)/.test(lib),
  "lib.rs 退出时必须调用 llama_engine::server::stop()（退出 pinvou 自动关引擎）"
);
assert(settingsView.includes("'__local_engine__'"), "SettingsView 必须提供本地识图引擎哨兵选项");
assert(settingsView.includes("autoStartLabel"), "SettingsView 必须渲染自动启动引擎设置项");
for (const label of [
  "自动启动引擎", "Auto-start engine", "エンジンの自動起動",
  "本地识图引擎", "Local image engine", "ローカル画像認識エンジン",
  "退出 pinvou 时引擎将自动关闭", "The engine shuts down automatically when you quit pinvou",
  "pinvou を終了するとエンジンも自動停止します",
]) {
  assert(settingsI18n.includes(label), `settings-i18n.js 必须包含: ${label}`);
}
const chatView = readRoot("src/features/chat/ChatView.jsx");
assert(chatView.includes("ensureLocalEngineForSend"), "ChatView 必须实现本地识图引擎发送门");
assert(chatView.includes("local_engine_state"), "ChatView 发送门必须消费 capability.local_engine_state");

// 9. PR3 引擎调优：三档模型表 + Q8_0 mmproj + 默认 q4km。
for (const id of ["qwen3vl-2b-iq2m", "qwen3vl-2b-q4km", "qwen3vl-4b-q4km", "qwen3vl-2b-q3k-s"]) {
  assert(download.includes(id), `download.rs 必须包含模型档 ${id}`);
}
assert(
  /fn default_model\(\)[\s\S]*?MODEL_Q4_K_M/.test(download),
  "default_model() 必须指向 Q4_K_M 默认档"
);
for (const mmproj of ["mmproj-Qwen3VL-2B-Instruct-Q8_0.gguf", "mmproj-Qwen3VL-4B-Instruct-Q8_0.gguf"]) {
  assert(download.includes(mmproj), `download.rs 必须使用 Q8_0 mmproj: ${mmproj}`);
}
// legacy 档保留但标注不推荐（老安装继续可用）。
assert(/旧版不推荐/.test(download), "legacy q3k-s 档必须标注不推荐");

// 10. PR3 启动参数与运行时:物理核线程/batch 1024/flash-attn/KV q8_0/mlock、
//     warmup、会话失效钩子、停止标志消费、自愈复用旧端口。
const server = read("features/llama_engine/server.rs");
for (const flag of ["--flash-attn", "--ubatch-size", "--batch-size", "--cache-type-k", "--cache-type-v", "--mlock"]) {
  assert(server.includes(`"${flag}"`), `build_args 必须含 ${flag}`);
}
assert(server.includes("physical_core_count()"), "build_args 必须用物理核数设置 -t");
assert(server.includes("spawn_warmup"), "引擎就绪后必须发 warmup 请求");
assert(server.includes("set_session_invalidation_hook"), "server.rs 必须提供会话失效钩子");
assert(server.includes("pub(crate) const HEALTH_TIMEOUT"), "HEALTH_TIMEOUT 必须对发送门可见");
const chatCmd = read("app/commands/chat.rs");
assert(
  /wait_until_running\(server::HEALTH_TIMEOUT\)/.test(chatCmd),
  "chat.rs 发送门等待窗口必须跟随引擎 HEALTH_TIMEOUT"
);
assert(!/from_secs\(60\)/.test(chatCmd), "chat.rs 不得保留 60s 发送门超时");

// 11. PR3 设备自动选择：OS 原语 + auto 解析 + 三语 UI 文案。
const osInterface = read("platform/os/interface/system.rs");
assert(osInterface.includes("pub enum GpuClass"), "platform/os 必须提供 GpuClass 分级");
for (const f of ["gpu_class", "physical_core_count"]) {
  assert(osInterface.includes(f), `platform/os interface 必须导出 ${f}`);
}
const windowsSystem = read("platform/os/windows/windows_system.rs");
assert(windowsSystem.includes("EnumAdapters1"), "Windows GPU 检测必须走 DXGI 枚举");
assert(windowsSystem.includes("vulkan-1.dll"), "Windows GPU 判定必须校验 vulkan-1.dll");
const llamaMod = read("features/llama_engine/mod.rs");
assert(llamaMod.includes("auto_detect_device"), "llama_engine 必须实现设备自动检测");
assert(llamaMod.includes("recommended_model"), "引擎状态必须带推荐模型档");
for (const lang of ["zh", "en", "ja"]) {
  const block = new RegExp(`dict\\.${lang}\\.uiSettingsDetail\\.llamaEngine[\\s\\S]*?deviceAuto`);
  assert(block.test(settingsI18n), `settings-i18n.js ${lang} llamaEngine 必须含 deviceAuto 文案`);
  assert(block.test(settingsI18n) && new RegExp(`dict\\.${lang}[\\s\\S]*?recommended`).test(settingsI18n),
    `settings-i18n.js ${lang} 必须含推荐标文案`);
}

// 12. PR3 发送前预缩放：classic script 注册 + 粘贴/拖放两链路消费 + 三语提示。
const indexHtml = readRoot("src/index.html");
assert(indexHtml.includes("features/attachments/image-prescale.js"), "index.html 必须注册 image-prescale.js");
assert(chatView.includes("PinvouImagePrescale"), "ChatView 粘贴链路必须接入预缩放");
const artifactsBridge = readRoot("src/platform/tauri/bridge/artifacts.js");
assert(artifactsBridge.includes("PinvouImagePrescale"), "拖放链路必须接入预缩放");
const bridgeJsFull = readRoot("src/platform/tauri/bridge.js");
for (const [file, text] of [
  [bridgeJsFull, "图片较大已压缩，识别可能较慢"],
  [bridgeJsFull, "Large image compressed before sending"],
  [bridgeJsFull, "大きな画像を圧縮してから送信します"],
  [settingsI18n, "图片较大已压缩，识别可能较慢"],
  [settingsI18n, "Large image compressed before sending"],
  [settingsI18n, "大きな画像を圧縮しました"],
]) {
  assert(file.includes(text), `预缩放提示三语缺失: ${text}`);
}

console.log("llama_engine_contract ok");
