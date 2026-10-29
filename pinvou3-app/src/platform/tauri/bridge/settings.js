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

  // 当前有效模型的图片输入能力(普通会话选图即时警告用);后端按会话模型绑定解析。
async function getImageInputCapability(...args) { return pinvouSharedtauriSettings().getImageInputCapability(...args); }

  // ── 模型列表(「添加模型」方案)─────────────────────────────────
  // Whole-list reload: when save/delete/switch chain loadModels calls concurrently, an older list must not
  // overwrite a newer one (audit b). The last request sequence wins.
  let modelsLoadSeq = 0;
async function loadModels() { return pinvouSharedtauriSettings().loadModels(); }
  // model 对象字段须是 snake_case(SavedModel serde):
  // {id,name,alias,preset,context_window_tokens,max_output_tokens,reasoning_effort,model,base_url,provider_kind,vendor,endpoint_mode,image_capability_override,vision_model_id,api_key,credential_ref,credential_state,has_secret,credential_action}
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

  // ── Global bring-to-front shortcut ────────────────────────────────
  // The dedicated Rust command set_global_shortcut_config exclusively owns
  // registration and persistence (update_settings is never used): success =
  // status "registered"/"disabled", and a committed save always reports the
  // accelerator disk now holds — including a disabled save, which may itself
  // have changed the stored combo (null remains possible on
  // "invalid"/"conflict", i.e. nothing was persisted). "invalid"/"conflict"
  // mean the registration was refused (conflict's message is technical
  // detail, console only; the UI shows localized copy). Hard failures (IPC
  // exceptions) resolve null so the caller can roll its toggle back and show
  // the generic failure copy.
  function setGlobalShortcutConfig(enabled, accelerator) {
    const sync = function () {
      return invoke("set_global_shortcut_config", { enabled: !!enabled, accelerator: String(accelerator || "") });
    };
    // Retry once on any invocation failure — a transient IPC error, or a
    // Rust-side persist failure (which rolls the registration back before
    // returning Err). The full re-apply is safe: apply_config is idempotent
    // per (enabled, accelerator) and disk stays authoritative (same shape as
    // setVoiceShortcutEnabled); on persistent failure return null so the
    // caller can roll its optimistic state back. On success the authoritative
    // response is mirrored into state.settings so a settings-page remount
    // reads the persisted combo instead of the startup snapshot.
    return sync().then(function (result) { return applyGlobalShortcutResult(result, enabled); }, function (error) {
      console.warn("[settings] global shortcut config sync failed, retrying once", error);
      return sync().then(function (result) { return applyGlobalShortcutResult(result, enabled); }, function (retryError) {
        console.warn("[settings] global shortcut config sync failed after retry", retryError);
        return null;
      });
    });
  }
  function applyGlobalShortcutResult(result, enabled) {
    if (!result || typeof result.status !== "string") return null;
    if (result.status !== "registered" && result.status !== "disabled") return result;
    if (state.settings) {
      // Same mechanics as the pet:enabled_changed listener above: the bridge
      // owns its settings slice; the React layer only reads it. Mirror the
      // accelerator whenever Rust reports one — a disabled save reports the
      // combo it just persisted, so the next toggle submits it instead of a
      // stale mirror. (A missing accelerator must never regress the snapshot
      // to the raw input.)
      const mirror = { global_shortcut_enabled: !!enabled };
      if (typeof result.accelerator === "string" && result.accelerator) {
        mirror.global_shortcut_accelerator = result.accelerator;
      }
      state.settings = Object.assign({}, state.settings, mirror);
      notify();
    }
    return result;
  }

  // ── Builtin feature toggles (docs/builtin-toolset-contract.md hook) ──────
  // list_builtin_features is consumed by ChatView (the session-mention feature
  // gate, PR #586); set_builtin_feature_enabled only wraps the command channel
  // for future per-feature settings pages — no consumer yet, no UI.
  async function listBuiltinFeatures() {
    return invoke("list_builtin_features");
  }
  // The Rust command takes `feature_id` (app/commands/builtin.rs); Tauri v2
  // converts camelCase JS keys to snake_case, so the payload must use
  // `featureId` — same convention as get_effective_model_config({ sessionId }).
  async function setBuiltinFeatureEnabled(id, enabled) {
    return invoke("set_builtin_feature_enabled", { featureId: id, enabled });
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
      probeLocalServerKind,
      setGlobalShortcutConfig,
      listBuiltinFeatures,
      setBuiltinFeatureEnabled
    };
  };
})(window);
