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

// 1. bridge.rs 接线：resolve_vision_model_config 的规则 0/3 必须把视觉端点
//    条件覆盖为 llama_engine::vision_endpoint()（引擎运行中优先），
//    且不引入 --alias（单模型模式忽略请求体 model 字段）。
const bridge = read("features/assistant/platform/bridge.rs");
const visionResolveMatch = bridge.match(/fn resolve_vision_model_config\(&self\)[\s\S]*?\n {4}\}/);
assert(visionResolveMatch, "bridge.rs 必须保留 resolve_vision_model_config（视觉工具配置解析）");
const visionResolve = visionResolveMatch[0];
// 规则 0：模型显式选择「本地识图引擎」且引擎运行中 → 本地端点（最高优先级）。
// 有界窗口锚定 if 块内部，避免跨数百行的 [/s\S]* 偶然命中其它规则。
assert(
  /vision_prefer_local_engine\)\s*\{\s*if let Some\(endpoint\) = crate::features::llama_engine::vision_endpoint\(\)/.test(visionResolve),
  "规则 0：vision_prefer_local_engine 命中时必须用 llama_engine::vision_endpoint() 覆盖为本地端点"
);
// 规则 3：全局兜底开关开（默认开）且引擎运行中 → 本地端点。
assert(
  /llama_engine_vision_fallback[\s\S]{0,80}?unwrap_or\(true\)[\s\S]{0,60}?crate::features::llama_engine::vision_endpoint\(\)/.test(visionResolve),
  "规则 3：llama_engine_vision_fallback 兜底（默认开）必须回落到 llama_engine::vision_endpoint()"
);
const visionEndpointCalls = visionResolve.match(/llama_engine::vision_endpoint\(\)/g) || [];
assert.strictEqual(
  visionEndpointCalls.length,
  2,
  "规则 0 与规则 3 应各调用一次 llama_engine::vision_endpoint()"
);
// 本地端点只在引擎运行中返回（Some），否则 None 走后续规则。
const llamaModVision = read("features/llama_engine/mod.rs");
assert(
  /fn vision_endpoint\(\) -> Option<String>\s*\{[\s\S]{0,120}?server::running_endpoint\(\)/.test(llamaModVision),
  "llama_engine::vision_endpoint() 必须仅在引擎运行中返回端点（server::running_endpoint）"
);
assert(
  bridge.includes("llama_engine_vision_fallback"),
  "resolve_vision_model_config 必须支持 llama_engine_vision_fallback 开关（用户可关闭本地引擎视觉兜底）"
);
assert(!/--alias/.test(bridge), "bridge.rs 不得引入 --alias");

// 2. 命令注册：8 条 llama_engine_* 命令齐全，且 #[tauri::command] 只在 app/commands/ 宿主。
const commands = read("app/commands/llama_engine.rs");
for (const name of [
  "llama_engine_status",
  "llama_engine_install_engine",
  "llama_engine_install_model",
  "llama_engine_cancel_download",
  "llama_engine_start",
  "llama_engine_stop",
  "llama_engine_delete_model",
  "llama_engine_delete_engine",
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
  "llama_engine_delete_model",
  "llama_engine_delete_engine",
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

// 4. 下载安全：模型/引擎 URL 全部 https；开发用 env 覆盖仅 debug 构建生效
//    （release 忽略并 warn，不固化暴露面——契约钉“release 下覆盖不生效”的
//    语义，而不是强制这些覆盖存在）。
const download = read("features/llama_engine/download.rs");
const urlConsts = download.match(/primary_url:\s*"[^"]+"/g) || [];
for (const line of urlConsts) {
  assert(/https:\/\//.test(line), `模型下载 URL 必须为 https: ${line}`);
}
assert(
  /fn dev_env_override\(name: &str\)[\s\S]{0,400}?cfg!\(debug_assertions\)/.test(download),
  "download.rs 必须经 dev_env_override 把 env 开发覆盖限制在 debug_assertions 下"
);
for (const envName of ["PINVOU3_LLAMA_MODEL_URL", "PINVOU3_LLAMA_ENGINE_TAG", "PINVOU3_LLAMA_MODEL_SHA256"]) {
  assert(download.includes(envName), `download.rs 必须保留 ${envName} 开发覆盖入口（debug 限定）`);
}
// URL 覆盖在 debug 下也强制 https scheme。
assert(
  /PINVOU3_LLAMA_MODEL_URL[\s\S]{0,300}?starts_with\("https:\/\/"\)/.test(download),
  "PINVOU3_LLAMA_MODEL_URL 覆盖必须通过 https scheme 校验"
);

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
// #341 后设置详情文案并入 shared/i18n/{zh,en,ja}.js（zh 内嵌 + en/ja 惰性 chunk），
// 原 settings-i18n.js 已删除；聚合串供下方 includes 类断言跨三语检查。
const DICT_NAME = { zh: "dictZh", en: "dictEn", ja: "dictJa" };
const langDicts = {
  zh: readRoot("src/shared/i18n/zh.js"),
  en: readRoot("src/shared/i18n/en.js"),
  ja: readRoot("src/shared/i18n/ja.js"),
};
const settingsI18n = Object.values(langDicts).join("\n");
for (const lang of ["zh", "en", "ja"]) {
  assert(
    new RegExp(`${DICT_NAME[lang]}\\.uiSettingsDetail\\.llamaEngine`).test(langDicts[lang]),
    `shared/i18n/${lang}.js 必须为 ${lang} 提供 llamaEngine 文案`
  );
}
// 「本地识图」已并入模型页胶囊（对齐 ACP 管理分页），导航文案为三语 localVision。
for (const [lang, label] of [["zh", "本地识图"], ["en", "Local Vision"], ["ja", "ローカル画像認識"]]) {
  assert(langDicts[lang].includes(label), `shared/i18n/${lang}.js 必须包含导航文案: ${label}`);
}
const settingsView = readRoot("src/features/settings/SettingsView.jsx");
assert(settingsView.includes("modelTab === 'llama'"), "SettingsView 必须以模型页胶囊分发本地识图子页");
assert(settingsView.includes('data-testid={`settings-model-tab-${tab.key}`}'), "SettingsView 必须渲染模型页胶囊按钮");

// 8. 本地识图引擎选项 + 自动启动/关闭契约：
//    SavedModel.vision_prefer_local_engine（is_false 序列化省略）、
//    AdvancedPrefs 自动启动三字段、capability localEngineState（camelCase wire）、
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
// wire 形状与 LlamaEngineStatus 同为 camelCase：serde rename + 字段名都要钉住。
assert(
  /#\[serde\(rename_all = "camelCase"\)\]\s*pub struct ImageInputCapabilityInfo/.test(settingsCmd),
  "ImageInputCapabilityInfo 必须按 camelCase 序列化（与 LlamaEngineStatus 一致）"
);
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
assert(chatView.includes("localEngineState"), "ChatView 发送门必须消费 capability.localEngineState（camelCase wire）");

// 9. 模型表：默认 2B q4km + 独显 4B q4km 两档 + Q8_0 mmproj。
// （IQ2_M / Q3_K_S 量化过低已下线，不得再回到可选列表。）
for (const id of ["qwen3vl-2b-q4km", "qwen3vl-4b-q4km"]) {
  assert(download.includes(id), `download.rs 必须包含模型档 ${id}`);
}
for (const removed of ["qwen3vl-2b-iq2m", "qwen3vl-2b-q3k-s"]) {
  assert(!download.includes(`id: "${removed}"`), `已下线模型档不得残留: ${removed}`);
}
assert(
  /fn default_model\(\)[\s\S]*?MODEL_Q4_K_M/.test(download),
  "default_model() 必须指向 Q4_K_M 默认档"
);
for (const mmproj of ["mmproj-Qwen3VL-2B-Instruct-Q8_0.gguf", "mmproj-Qwen3VL-4B-Instruct-Q8_0.gguf"]) {
  assert(download.includes(mmproj), `download.rs 必须使用 Q8_0 mmproj: ${mmproj}`);
}

// 10. PR3 启动参数与运行时:物理核线程/batch 1024/flash-attn/KV q8_0、
//     warmup、会话失效钩子、停止标志消费、自愈复用旧端口。
//     注意:--mlock 与 -ngl 0 组合在 b10362 上 mmap 崩溃,已刻意移除。
const server = read("features/llama_engine/server.rs");
for (const flag of ["--flash-attn", "--ubatch-size", "--batch-size", "--cache-type-k", "--cache-type-v"]) {
  assert(server.includes(`"${flag}"`), `build_args 必须含 ${flag}`);
}
assert(!server.includes('"--mlock".into()'), "--mlock 与 -ngl 0 组合触发引擎 mmap 断言,不得再传");
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
  const block = new RegExp(`${DICT_NAME[lang]}\\.uiSettingsDetail\\.llamaEngine[\\s\\S]*?deviceAuto`);
  assert(block.test(langDicts[lang]), `shared/i18n/${lang}.js llamaEngine 必须含 deviceAuto 文案`);
  assert(new RegExp(`${DICT_NAME[lang]}[\\s\\S]*?recommended`).test(langDicts[lang]),
    `shared/i18n/${lang}.js 必须含推荐标文案`);
}

// 12. PR3 发送前预缩放：classic script 注册 + 粘贴/拖放两链路消费 + 三语提示。
const indexHtml = readRoot("src/index.html");
assert(indexHtml.includes("features/attachments/image-prescale.js"), "index.html 必须注册 image-prescale.js");
assert(chatView.includes("PinvouImagePrescale"), "ChatView 粘贴链路必须接入预缩放");
const artifactsBridge = readRoot("src/platform/tauri/bridge/artifacts.js");
assert(artifactsBridge.includes("PinvouImagePrescale"), "拖放链路必须接入预缩放");
const bridgeJsFull = readRoot("src/platform/tauri/bridge.js");
for (const [file, text] of [
  [bridgeJsFull, "图片较大已压缩，识别精度可能略降"],
  [bridgeJsFull, "Large image compressed before sending"],
  [bridgeJsFull, "大きな画像を圧縮してから送信します"],
  [settingsI18n, "图片较大已压缩，识别精度可能略降"],
  [settingsI18n, "Large image compressed before sending"],
  [settingsI18n, "大きな画像を圧縮しました"],
]) {
  assert(file.includes(text), `预缩放提示三语缺失: ${text}`);
}

console.log("llama_engine_contract ok");
