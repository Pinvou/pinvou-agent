/**
 * settings feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  "use strict";
  var registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["settings"] = function (context) {
    var state = context.state;
    var notify = context.notify;
    var invoke = context.invoke;
    var listen = context.listen;
  // ── Settings ─────────────────────────────────────────────────────
  // 桌宠开关由 Rust set_pet_enabled 直接写盘(设置页/宠物右键/快捷图标共用),
  // 这里同步进内存副本——否则下次整份 saveSettings 会用旧值把开关翻回去。
  listen("pet:enabled_changed", function (e) {
    if (state.settings) {
      state.settings.pet = Object.assign({}, state.settings.pet || {}, {
        enabled: !!(e.payload && e.payload.enabled),
      });
      notify();
    }
  });

  listen("pet:selected_changed", function (e) {
    var selectedPet = e.payload && e.payload.selected_pet;
    if (typeof selectedPet === "string") {
      state.selectedPet = selectedPet;
      notify();
    }
  });

  async function loadSettings() {
    try {
      state.settings = await invoke("get_settings");
    } catch (e) {
      state.settings = { theme: "genesis", language: "zh-Hans" };
    }
    notify();
  }
  async function loadSelectedPet() {
    try {
      state.selectedPet = await invoke("get_selected_pet");
    } catch (e) {
      state.selectedPet = "lingling";
    }
    notify();
  }
  async function setSelectedPet(id) {
    return await invoke("set_selected_pet", { id: id });
  }
  async function loadEffectiveModelConfig() {
    try {
      state.effectiveModelConfig = await invoke("get_effective_model_config");
    } catch (e) {
      state.effectiveModelConfig = null;
    }
    notify();
  }
  async function saveSettings(prefs) {
    const previous = state.settings;
    try {
      await invoke("update_settings", { prefs: prefs });
      state.settings = prefs;
      notify();
      return true;
    } catch (e) {
      console.warn("save settings failed", e);
      state.settings = previous;
      notify();
      return false;
    }
  }
  async function saveSettingsAndRestart(prefs) {
    state.settings = prefs;
    try {
      await invoke("save_settings_and_restart", { prefs: prefs });
    } catch (e) {
      console.warn("save settings and restart failed", e);
    }
  }

  function llmApiBackendUserState(status) {
    if (!status) return "unknown";
    if (status.backend_user_state === "exists" || status.backend_user_state === "not_exists" || status.backend_user_state === "unknown") {
      return status.backend_user_state;
    }
    return status.backend_user_exists ? "exists" : "not_exists";
  }

  function llmApiAccountKnownExists(status) {
    return llmApiBackendUserState(status) === "exists";
  }

  function llmApiAccountStateIsKnown(status) {
    var backendUserState = llmApiBackendUserState(status);
    return backendUserState === "exists" || backendUserState === "not_exists";
  }

  var LLMAPI_STARTUP_RETRY_DELAYS_MS = [2000, 5000, 10000, 30000];

  function llmApiStartupRetryDelay(attempt) {
    return LLMAPI_STARTUP_RETRY_DELAYS_MS[
      Math.min(attempt, LLMAPI_STARTUP_RETRY_DELAYS_MS.length - 1)
    ];
  }

  function waitForLlmApiStartupRetry(delayMs) {
    return new Promise(function (resolve) { setTimeout(resolve, delayMs); });
  }

  async function refreshLlmApiState(options) {
    options = options || {};
    var refreshModels = options.refreshModels !== false;
    var refreshSavedModels = !!options.refreshSavedModels;
    var status = null;
    var models = null;
    try {
      status = await invoke("get_llmapi_status");
      // An inconclusive refresh must not change model visibility after an
      // authoritative account result has already been observed.
      if (llmApiAccountStateIsKnown(status) || !llmApiAccountStateIsKnown(state.llmApiStatus)) {
        state.llmApiStatus = status;
      }
    } catch (e) {
      console.warn("get llmapi status failed", e);
      // Keep the last known account state. A transport failure is not proof
      // that the backend account disappeared.
      notify();
      throw e;
    }
    if (refreshModels && llmApiAccountKnownExists(status)) {
      try {
        models = await invoke("get_llmapi_models");
        state.llmApiModels = models;
      } catch (e) {
        console.warn("get llmapi models failed", e);
        // Preserve the last successfully synchronized model list.
        notify();
        throw e;
      }
    } else if (llmApiBackendUserState(status) === "not_exists") {
      state.llmApiModels = null;
    }
    if (refreshSavedModels) await loadModels();
    notify();
    return { status: status, models: models };
  }

  async function getLlmApiStatus() {
    var result = await refreshLlmApiState({ refreshModels: false });
    return result.status;
  }

  async function getLlmApiModels() {
    var models = await invoke("get_llmapi_models");
    state.llmApiModels = models;
    notify();
    return models;
  }

  async function refreshLlmApiOnStartup() {
    var retryAttempt = 0;
    while (true) {
      var status = null;
      try {
        status = await getLlmApiStatus();
      } catch (e) {
        console.warn("load llmapi account status failed", e);
      }

      if (llmApiAccountStateIsKnown(status)) {
        if (llmApiAccountKnownExists(status)) {
          try {
            await getLlmApiModels();
          } catch (e) {
            console.warn("load llmapi models failed", e);
          }
        }
        try {
          await loadModels();
        } catch (e) {
          console.warn("reload saved models after llmapi account refresh failed", e);
        }
        return status;
      }

      var retryDelayMs = llmApiStartupRetryDelay(retryAttempt);
      retryAttempt += 1;
      console.warn("llmapi account status is unknown; retrying in " + retryDelayMs + "ms");
      await waitForLlmApiStartupRetry(retryDelayMs);
    }
  }

  async function setLlmApiDefaultModel(model) {
    var models = await invoke("set_llmapi_default_model", { model: model });
    state.llmApiModels = models;
    await loadSettings();
    await loadModels();
    notify();
    return models;
  }

  async function ensureLlmApiBinding() {
    var result = await invoke("ensure_llmapi_binding");
    await refreshLlmApiState({ refreshSavedModels: true });
    return result;
  }

  async function loginLlmApiUser(username, password) {
    var result = await invoke("login_llmapi_user", { username: username, password: password });
    await refreshLlmApiState({ refreshSavedModels: true });
    return result;
  }

  async function saveLlmApiUserSession(userId, accessToken) {
    var result = await invoke("save_llmapi_user_session", { userId: userId, accessToken: accessToken });
    await refreshLlmApiState({ refreshSavedModels: true });
    return result;
  }

  async function retryLlmApiProvisioning(pinvouUserId, deviceBindingId) {
    var result = await invoke("retry_llmapi_provisioning", { pinvouUserId: pinvouUserId, deviceBindingId: deviceBindingId });
    await refreshLlmApiState({ refreshSavedModels: true });
    return result;
  }

  async function setLlmApiUserEnabled(pinvouUserId, enabled) {
    var result = await invoke("set_llmapi_user_enabled", { pinvouUserId: pinvouUserId, enabled: enabled });
    await refreshLlmApiState({ refreshSavedModels: true });
    return result;
  }

  async function getLlmApiAdminOverview(query, status, limit, offset) {
    return await invoke("get_llmapi_admin_overview", {
      query: query || null,
      status: status || null,
      limit: limit == null ? null : limit,
      offset: offset == null ? null : offset,
    });
  }

  async function submitFeedback(request) {
    return await invoke("submit_feedback", { request: request });
  }
  async function discoverLocalVllm(request) {
    return await invoke("discover_local_vllm", { request: request || null });
  }

  // ── MegaCube(GB10) 本地大模型一键引导 ────────────────────────────
  var vllmSetupPollTimer = null;
  var vllmSetupPollStartedAt = 0;
  var VLLM_SETUP_POLL_INTERVAL_MS = 3000;
  var VLLM_SETUP_POLL_TIMEOUT_MS = 12 * 60 * 1000;
  // 首屏检测「预装但未启用」状态;eligible 时前端弹引导框。
  // 开机加载中不弹框，每 3 秒静默复查；12 分钟后仍 starting 则恢复可重试入口。
  // autoPoll 只供内部定时器续接；用户手动检测会重置本轮截止时间。
  async function detectLocalVllmSetup(options) {
    var autoPoll = !!(options && options.autoPoll);
    if (vllmSetupPollTimer) {
      clearTimeout(vllmSetupPollTimer);
      vllmSetupPollTimer = null;
    }
    if (!autoPoll) vllmSetupPollStartedAt = Date.now();
    try {
      state.vllmSetup = await invoke("detect_local_vllm_setup");
    } catch (e) {
      state.vllmSetup = null; // 检测失败静默,不打扰(等同不弹)
      vllmSetupPollStartedAt = 0;
    }
    if (state.vllmSetup && state.vllmSetup.engine_state === 'starting' && state.vllmSetup.may_offer_setup !== false) {
      var elapsed = Date.now() - vllmSetupPollStartedAt;
      if (vllmSetupPollStartedAt > 0 && elapsed >= VLLM_SETUP_POLL_TIMEOUT_MS) {
        state.vllmSetup = Object.assign({}, state.vllmSetup, {
          engine_state: 'failed',
          eligible: !!state.vllmSetup.may_offer_setup,
          detection_timed_out: true,
        });
        vllmSetupPollStartedAt = 0;
      } else {
        vllmSetupPollTimer = setTimeout(function () {
          vllmSetupPollTimer = null;
          detectLocalVllmSetup({ autoPoll: true });
        }, VLLM_SETUP_POLL_INTERVAL_MS);
      }
    } else {
      vllmSetupPollStartedAt = 0;
    }
    notify();
    return state.vllmSetup; // 返回供设置页「检测本机 vLLM」判断 has_packages
  }
  // 用户点「启用」:后端一次 pkexec 拉起引擎+装 systemd 服务,轮询就绪后写模型配置。
  // 引擎首次载模型可能几分钟,全程 vllmBootstrapping 显示 spinner。
  async function bootstrapLocalVllm() {
    if (state.vllmBootstrapping) return;
    state.vllmBootstrapping = true;
    state.vllmBootstrapError = null;
    state.vllmBootstrapDone = null;
    state.vllmSetupPhase = 'authorizing'; // 后端事件到达前先本地置首阶段(pkexec 阻塞期也有步骤显示)
    state.vllmSetupAttempt = 0;
    notify();
    try {
      state.vllmBootstrapDone = await invoke("bootstrap_local_vllm");
    } catch (e) {
      state.vllmBootstrapError = String(e && e.message ? e.message : e);
    }
    state.vllmBootstrapping = false;
    notify();
  }
  // 点「跳过」:仅本次会话内不再弹(不写持久标记,下次启动若仍未配好会再次友好提示)。
  function dismissVllmSetup() {
    state.vllmSetupDismissed = true;
    notify();
  }
  // 点「不再提醒 → 确认」:持久婉拒,开机引导框不再自动弹(仍可在设置→模型管理手动启用)。
  async function declineVllmSetup() {
    try { await invoke("decline_local_vllm_setup"); } catch (e) { /* 持久失败也先隐藏本会话,不阻断 */ }
    state.vllmSetupDismissed = true;
    notify();
  }
  async function getEffectiveModelConfig() {
    return await invoke("get_effective_model_config");
  }

  // ── 模型列表(「添加模型」方案)─────────────────────────────────
  async function loadModels() {
    try {
      var v = await invoke("list_models");
      state.savedModels = (v && v.models) || [];
      state.activeModelId = (v && v.active_model_id) || null;
    } catch (e) {
      state.savedModels = []; state.activeModelId = null;
    }
    notify();
  }
  // model 对象字段须是 snake_case(SavedModel serde):
  // {id,name,preset,context_window_tokens,max_output_tokens,model,base_url,api_key,credential_action}
 async function saveModel(model) {
   await invoke("save_model", { model: model });
   await loadModels();
 }
 async function revealModelApiKey(id) {
   return await invoke("reveal_model_api_key", { id: id });
 }
 async function deleteModel(id) {
   await invoke("delete_model", { id: id });
   await loadModels();
  }
  async function setActiveModel(id) {
    await invoke("set_active_model", { id: id });
    await loadModels();
  }
  // 读某会话当前绑定的模型 id(切会话时刷新 chip)。
  async function loadSessionModel(sessionId) {
    if (!sessionId) { state.currentSessionModelId = null; notify(); return; }
    try {
      state.currentSessionModelId = await invoke("get_session_model_id", { sessionId: sessionId });
    } catch (e) { state.currentSessionModelId = null; }
    notify();
  }
  // 切当前会话模型(chip 热切)。无 session(草稿态)时改全局默认。
  async function switchModel(sessionId, modelId) {
    if (sessionId) {
      await invoke("set_session_model", { sessionId: sessionId, modelId: modelId });
      state.currentSessionModelId = modelId;
      notify();
    } else {
      await setActiveModel(modelId);
    }
  }
  async function testModelConnection(baseUrl, apiKey, modelId) {
    return await invoke("test_model_connection", { baseUrl: baseUrl, apiKey: apiKey, modelId: modelId || null });
  }
  async function testSearchProvider(provider, apiKey) {
    return await invoke("test_search_provider", { provider: provider, apiKey: apiKey || null });
  }

    return {
      loadSettings: loadSettings,
      loadSelectedPet: loadSelectedPet,
      setSelectedPet: setSelectedPet,
      loadEffectiveModelConfig: loadEffectiveModelConfig,
      saveSettings: saveSettings,
      saveSettingsAndRestart: saveSettingsAndRestart,
      refreshLlmApiOnStartup: refreshLlmApiOnStartup,
      getLlmApiStatus: getLlmApiStatus,
      getLlmApiModels: getLlmApiModels,
      setLlmApiDefaultModel: setLlmApiDefaultModel,
      ensureLlmApiBinding: ensureLlmApiBinding,
      loginLlmApiUser: loginLlmApiUser,
      saveLlmApiUserSession: saveLlmApiUserSession,
      retryLlmApiProvisioning: retryLlmApiProvisioning,
      setLlmApiUserEnabled: setLlmApiUserEnabled,
      getLlmApiAdminOverview: getLlmApiAdminOverview,
      submitFeedback: submitFeedback,
      discoverLocalVllm: discoverLocalVllm,
      detectLocalVllmSetup: detectLocalVllmSetup,
      bootstrapLocalVllm: bootstrapLocalVllm,
      dismissVllmSetup: dismissVllmSetup,
      declineVllmSetup: declineVllmSetup,
      getEffectiveModelConfig: getEffectiveModelConfig,
      loadModels: loadModels,
      saveModel: saveModel,
      revealModelApiKey: revealModelApiKey,
      deleteModel: deleteModel,
      setActiveModel: setActiveModel,
      loadSessionModel: loadSessionModel,
      switchModel: switchModel,
      testModelConnection: testModelConnection,
      testSearchProvider: testSearchProvider
    };
  };
})(window);
