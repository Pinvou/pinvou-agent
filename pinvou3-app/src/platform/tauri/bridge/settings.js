/**
 * settings feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting statements would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["settings"] = function (context) {let pinvouSharedtauriSettingsCache = null;
function pinvouSharedtauriSettings() {
  if (!pinvouSharedtauriSettingsCache) pinvouSharedtauriSettingsCache = window.PinvouBridgeShared.create("tauriSettings", { state, invoke, notify, settingsWriteQueue: { get value() { return settingsWriteQueue; }, set value(v) { settingsWriteQueue = v; } }, modelsLoadSeq: { get value() { return modelsLoadSeq; }, set value(v) { modelsLoadSeq = v; } }, loadSessionModel, setActiveModel });
  return pinvouSharedtauriSettingsCache;
}


    const state = context.state;
    const notify = context.notify;
    const invoke = context.invoke;
    const listen = context.listen;
  // ── Settings ─────────────────────────────────────────────────────
  // 桌宠开关由 Rust set_pet_enabled 直接写盘(设置页/宠物右键/快捷图标共用),
  // 这里同步进内存副本，保证设置界面立即反映专用命令返回的桌宠状态。
  listen("pet:enabled_changed", function (e) {
    if (state.settings) {
      state.settings.pet = Object.assign({}, state.settings.pet || {}, {
        enabled: !!(e.payload && e.payload.enabled),
      });
      notify();
    }
  });

  listen("pet:selected_changed", function (e) {
    const selectedPet = e.payload && e.payload.selected_pet;
    if (typeof selectedPet === "string") {
      state.selectedPet = selectedPet;
      notify();
    }
  });

async function loadSettings() { return pinvouSharedtauriSettings().loadSettings(); }
async function loadSelectedPet() { return pinvouSharedtauriSettings().loadSelectedPet(); }
async function setSelectedPet(id) { return pinvouSharedtauriSettings().setSelectedPet(id); }
async function loadEffectiveModelConfig(...args) { return pinvouSharedtauriSettings().loadEffectiveModelConfig.apply(null, args); }
  let settingsWriteQueue = Promise.resolve();
function enqueueSettingsWrite(write) { return pinvouSharedtauriSettings().enqueueSettingsWrite(write); }
  async function saveSettings(patch) {
    return enqueueSettingsWrite(async function () {
      try {
        state.settings = await invoke("update_settings", { patch });
        await loadEffectiveModelConfig();
        notify();
        return true;
      } catch (e) {
        console.warn("save settings failed", e);
        return false;
      }
    });
  }
  async function saveSearchSettings(search) {
    return enqueueSettingsWrite(async function () {
      try {
        state.settings = await invoke("update_search_settings", { search });
        await loadEffectiveModelConfig();
        notify();
        return true;
      } catch (e) {
        console.warn("save search settings failed", e);
        return false;
      }
    });
  }
  async function saveSearchSettingsAndRestart(search) {
    return enqueueSettingsWrite(async function () {
      try {
        await invoke("save_search_settings_and_restart", { search });
        return true;
      } catch (e) {
        console.warn("save search settings and restart failed", e);
        return false;
      }
    });
  }

async function submitFeedback(request) { return pinvouSharedtauriSettings().submitFeedback(request); }
async function discoverLocalVllm(request) { return pinvouSharedtauriSettings().discoverLocalVllm(request); }

async function getEffectiveModelConfig(...args) { return pinvouSharedtauriSettings().getEffectiveModelConfig(...args); }
  // 当前有效模型的图片输入能力(普通会话选图即时警告用);后端按会话模型绑定解析。
async function getImageInputCapability(...args) { return pinvouSharedtauriSettings().getImageInputCapability(...args); }

  // ── 模型列表(「添加模型」方案)─────────────────────────────────
  // Whole-list reload: when save/delete/switch chain loadModels calls concurrently, an older list must not
  // overwrite a newer one (audit b). The last request sequence wins.
  let modelsLoadSeq = 0;
async function loadModels() { return pinvouSharedtauriSettings().loadModels(); }
  // model 对象字段须是 snake_case(SavedModel serde):
  // {id,name,preset,context_window_tokens,max_output_tokens,model,base_url,api_key,credential_action,image_capability_override,vision_model_id}
 async function saveModel(model) {
   await invoke("save_model", { model });
   await loadModels();
   await loadSettings();
   await loadEffectiveModelConfig();
 }
async function revealModelApiKey(id) { return pinvouSharedtauriSettings().revealModelApiKey(id); }
 async function deleteModel(id) {
   await invoke("delete_model", { id });
   await loadModels();
   await loadSettings();
   await loadEffectiveModelConfig();
  }
  async function setActiveModel(id) {
    await invoke("set_active_model", { id });
    await loadModels();
    await loadSettings();
    await loadEffectiveModelConfig();
  }
  // 读某会话当前绑定的模型 id(切会话时刷新 chip)。
  async function loadSessionModel(sessionId) {
    const requestedSessionId = sessionId || null;
    const results = await Promise.all([
      requestedSessionId
        ? invoke("get_session_model_id", { sessionId: requestedSessionId }).catch(function () { return null; })
        : Promise.resolve(null),
      invoke("get_effective_model_config", { sessionId: requestedSessionId }).catch(function () { return null; }),
    ]);
    if (requestedSessionId !== (state.activeSessionId || null)) return;
    state.currentSessionModelId = results[0];
    state.effectiveModelConfig = results[1];
    notify();
  }
  // 切当前会话模型(chip 热切)。无 session(草稿态)时改全局默认。
async function switchModel(sessionId, modelId) { return pinvouSharedtauriSettings().switchModel(sessionId, modelId); }
async function testModelConnection(baseUrl, apiKey, modelId) { return pinvouSharedtauriSettings().testModelConnection(baseUrl, apiKey, modelId); }
  // 测试图片输入能力(设计 §7.3):用当前表单的 model/base_url/key 发一张内置纯色图,
  // 仅由模型编辑弹窗主动点击触发,无任何启动/定时自动测试。
async function testImageInputCapability(model, baseUrl, apiKey, modelId) { return pinvouSharedtauriSettings().testImageInputCapability(model, baseUrl, apiKey, modelId); }
  async function probeLocalServerKind(baseUrl, apiKey, modelId) {
    // 本地/内网 OpenAI 兼容端点的服务类型探测（vllm/ollama/lmstudio/generic）。
    // Rust 侧按 base_url TTL 缓存；命令失败（老版本桌面/命令被拒）在这里 reject，
    // 由消费方 catch 降级为「未知」——吞错伪造成 generic 会让 UI 误报
    // 「该端点不支持思考档位调节」（localProbeTiersForKind('generic') 为 null）。
    // apiKey/modelId follow testModelConnection: a freshly typed form key
    // wins, otherwise the saved credential is read — authenticated vLLM
    // (--api-key) 401s on /v1/models, so probing without credentials
    // misclassifies the authenticated endpoint as generic.
    return invoke("probe_local_server_kind", {
      baseUrl,
      apiKey: apiKey || null,
      modelId: modelId || null,
    });
  }

    return {
      loadSettings,
      loadSelectedPet,
      setSelectedPet,
      loadEffectiveModelConfig,
      saveSettings,
      saveSearchSettings,
      saveSearchSettingsAndRestart,
      submitFeedback,
      discoverLocalVllm,
      getEffectiveModelConfig,
      getImageInputCapability,
      loadModels,
      saveModel,
      revealModelApiKey,
      deleteModel,
      setActiveModel,
      loadSessionModel,
      switchModel,
      testModelConnection,
      testImageInputCapability,
      probeLocalServerKind
    };
  };
})(window);
