/**
 * settings feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting statements would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["settings"] = function (context) {
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

  async function loadSettings() {
    try {
      state.settings = await invoke("get_settings");
    } catch {
      state.settings = { theme: "genesis", language: "zh-Hans" };
    }
    notify();
  }
  async function loadSelectedPet() {
    try {
      state.selectedPet = await invoke("get_selected_pet");
    } catch {
      state.selectedPet = "lingling";
    }
    notify();
  }
  async function setSelectedPet(id) {
    return invoke("set_selected_pet", { id });
  }
  async function loadEffectiveModelConfig(sessionId) {
    const requestedSessionId = arguments.length ? (sessionId || null) : (state.activeSessionId || null);
    try {
      const config = await invoke("get_effective_model_config", { sessionId: requestedSessionId });
      if (requestedSessionId !== (state.activeSessionId || null)) return;
      state.effectiveModelConfig = config;
    } catch {
      state.effectiveModelConfig = null;
    }
    notify();
  }
  let settingsWriteQueue = Promise.resolve();
  function enqueueSettingsWrite(write) {
    const pending = settingsWriteQueue.then(write, write);
    settingsWriteQueue = pending.then(function () {}, function () {});
    return pending;
  }
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
  async function saveSettingsAndRestart(patch) {
    return enqueueSettingsWrite(async function () {
      try {
        await invoke("save_settings_and_restart", { patch });
        return true;
      } catch (e) {
        console.warn("save settings and restart failed", e);
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

  async function submitFeedback(request) {
    return invoke("submit_feedback", { request });
  }
  async function discoverLocalVllm(request) {
    return invoke("discover_local_vllm", { request: request || null });
  }

  // ── MegaCube(GB10) 本地大模型一键引导 ────────────────────────────
  let vllmSetupPollTimer = null;
  let vllmSetupPollStartedAt = 0;
  const VLLM_SETUP_POLL_INTERVAL_MS = 3000;
  const VLLM_SETUP_POLL_TIMEOUT_MS = 12 * 60 * 1000;
  // 首屏检测「预装但未启用」状态;eligible 时前端弹引导框。
  // 开机加载中不弹框，每 3 秒静默复查；12 分钟后仍 starting 则恢复可重试入口。
  // autoPoll 只供内部定时器续接；用户手动检测会重置本轮截止时间。
  // 陈旧检测快照覆盖（审计）：检测与长任务引导（bootstrap_local_vllm）并发时，
  // 旧快照会把已就绪引擎覆盖回 starting。任何新检测与引导完成都递增序号，
  // 在途读取一律作废。社区版后端 detect 恒 stopped / bootstrap 恒 Err（厂商版
  // 语义的桩），此守卫为防御性：后端恢复真实探测时竞态即真实存在。
  let vllmDetectSeq = 0;
  async function detectLocalVllmSetup(options) {
    const autoPoll = !!(options && options.autoPoll);
    const seq = ++vllmDetectSeq;
    if (vllmSetupPollTimer) {
      clearTimeout(vllmSetupPollTimer);
      vllmSetupPollTimer = null;
    }
    if (!autoPoll) vllmSetupPollStartedAt = Date.now();
    try {
      const snapshot = await invoke("detect_local_vllm_setup");
      if (seq !== vllmDetectSeq) return state.vllmSetup; // 已作废的陈旧读取
      state.vllmSetup = snapshot;
    } catch {
      if (seq !== vllmDetectSeq) return state.vllmSetup;
      state.vllmSetup = null; // 检测失败静默,不打扰(等同不弹)
      vllmSetupPollStartedAt = 0;
    }
    if (state.vllmSetup && state.vllmSetup.engine_state === 'starting' && state.vllmSetup.may_offer_setup !== false) {
      const elapsed = Date.now() - vllmSetupPollStartedAt;
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
    vllmDetectSeq++; // 引导完成：作废在途的陈旧检测读取（审计）
    state.vllmBootstrapping = false;
    notify();
    // 作废在途读取会中断 autoPoll 续排链（被作废的检测不再续排定时器），
    // 引导完成后主动重检一次，让引擎就绪状态及时收敛（审计补充）。
    detectLocalVllmSetup({ autoPoll: true });
  }
  // 点「跳过」:仅本次会话内不再弹(不写持久标记,下次启动若仍未配好会再次友好提示)。
  function dismissVllmSetup() {
    state.vllmSetupDismissed = true;
    notify();
  }
  // 点「不再提醒 → 确认」:持久婉拒,开机引导框不再自动弹(仍可在设置→模型管理手动启用)。
  async function declineVllmSetup() {
    try { await invoke("decline_local_vllm_setup"); } catch { /* 持久失败也先隐藏本会话,不阻断 */ }
    state.vllmSetupDismissed = true;
    notify();
  }
  async function getEffectiveModelConfig(sessionId) {
    return invoke("get_effective_model_config", {
      sessionId: arguments.length ? (sessionId || null) : (state.activeSessionId || null),
    });
  }
  // 当前有效模型的图片输入能力(普通会话选图即时警告用);后端按会话模型绑定解析。
  async function getImageInputCapability(sessionId) {
    return invoke("get_image_input_capability", {
      sessionId: arguments.length ? (sessionId || null) : (state.activeSessionId || null),
    });
  }

  // ── 模型列表(「添加模型」方案)─────────────────────────────────
  // 整表覆盖加载：保存/删除/切换链式 loadModels 并发时旧列表不得覆盖新列表
  // （审计 b）。请求序号后发者胜（同 vllmDetectSeq 模式）。
  let modelsLoadSeq = 0;
  async function loadModels() {
    const seq = ++modelsLoadSeq;
    try {
      const v = await invoke("list_models");
      if (seq !== modelsLoadSeq) return;
      state.savedModels = (v && v.models) || [];
      state.activeModelId = (v && v.active_model_id) || null;
    } catch {
      if (seq !== modelsLoadSeq) return;
      state.savedModels = []; state.activeModelId = null;
    }
    notify();
  }
  // model 对象字段须是 snake_case(SavedModel serde):
  // {id,name,preset,context_window_tokens,max_output_tokens,model,base_url,api_key,credential_action,image_capability_override,vision_model_id}
 async function saveModel(model) {
   await invoke("save_model", { model });
   await loadModels();
   await loadSettings();
   await loadEffectiveModelConfig();
 }
 async function revealModelApiKey(id) {
   return invoke("reveal_model_api_key", { id });
 }
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
  async function switchModel(sessionId, modelId) {
    if (sessionId) {
      await invoke("set_session_model", { sessionId, modelId });
      await loadSessionModel(sessionId);
    } else {
      await setActiveModel(modelId);
    }
  }
  async function testModelConnection(baseUrl, apiKey, modelId) {
    return invoke("test_model_connection", { baseUrl, apiKey, modelId: modelId || null });
  }
  // 测试图片输入能力(设计 §7.3):用当前表单的 model/base_url/key 发一张内置纯色图,
  // 仅由模型编辑弹窗主动点击触发,无任何启动/定时自动测试。
  async function testImageInputCapability(model, baseUrl, apiKey, modelId) {
    return invoke("test_image_input_capability", { model, baseUrl, apiKey, modelId: modelId || null });
  }
  async function probeLocalServerKind(baseUrl, apiKey, modelId) {
    // 本地/内网 OpenAI 兼容端点的服务类型探测（vllm/ollama/lmstudio/generic）。
    // Rust 侧按 base_url TTL 缓存；命令失败（老版本桌面/命令被拒）在这里 reject，
    // 由消费方 catch 降级为「未知」——吞错伪造成 generic 会让 UI 误报
    // 「该端点不支持思考档位调节」（localProbeTiersForKind('generic') 为 null）。
    // apiKey/modelId 与 testModelConnection 同口径：表单新填 key 优先，否则读
    // 已保存凭据——鉴权 vLLM（--api-key）的 /v1/models 会 401，不带凭据探测
    // 会把鉴权端点误判成 generic。
    return invoke("probe_local_server_kind", {
      baseUrl,
      apiKey: apiKey || null,
      modelId: modelId || null,
    });
  }
  async function testSearchProvider(provider, apiKey) {
    return invoke("test_search_provider", { provider, apiKey: apiKey || null });
  }

    return {
      loadSettings,
      loadSelectedPet,
      setSelectedPet,
      loadEffectiveModelConfig,
      saveSettings,
      saveSettingsAndRestart,
      saveSearchSettings,
      saveSearchSettingsAndRestart,
      submitFeedback,
      discoverLocalVllm,
      detectLocalVllmSetup,
      bootstrapLocalVllm,
      dismissVllmSetup,
      declineVllmSetup,
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
      probeLocalServerKind,
      testSearchProvider
    };
  };
})(window);
