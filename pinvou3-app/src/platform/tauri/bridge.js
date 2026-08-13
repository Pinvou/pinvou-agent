/**
 * tauri-bridge.js — Tauri 后端通信桥
 *
 * 封装所有 invoke/listen，维护前端状态，通过 pub/sub 推给 React。
 * 浏览器预览时（无 window.__TAURI__）自动降级。
 */
(function () {
  "use strict";

  // Browser transport owns its own replay and persistence semantics.
  if (window.PinvouPlatform && (window.PinvouPlatform.kind === "web" || window.PinvouPlatform.isWeb === true)) return;

  const TAURI = window.__TAURI__;
  if (!TAURI) {
    console.warn("[TauriBridge] Tauri not available — browser preview mode");
    window.TauriBridge = {
      available: false,
      lifecycle: { init: function () { return Promise.resolve(); } },
      state: {
        get: function () { return {}; },
        getMany: function () { return {}; },
        subscribe: function () { return function () {}; },
        subscribeMany: function () { return function () {}; },
      },
      rendering: { renderMarkdown: function (text) { return String(text || ""); } },
    };
    return;
  }

  const { invoke } = TAURI.core;
  const { listen } = TAURI.event;
  const dialogOpen = TAURI.dialog?.open;
  function startupMark(stage, detail) {
    if (window.__PINVOU_STARTUP__) window.__PINVOU_STARTUP__.mark(stage, detail);
  }
  function startupNow() {
    return window.performance && typeof window.performance.now === "function"
      ? window.performance.now()
      : Date.now();
  }
  async function startupAwait(stage, action) {
    var started = startupNow();
    startupMark(stage + ":start");
    try {
      var result = await action();
      startupMark(stage + ":done", "duration_ms=" + (startupNow() - started).toFixed(1));
      return result;
    } catch (error) {
      startupMark(stage + ":error", "duration_ms=" + (startupNow() - started).toFixed(1) + " error=" + String(error));
      throw error;
    }
  }
  async function refreshConnectorAuthGates() {
    startupMark("bridge:connector_auth_refresh:start");
    try {
      var result = await invoke("refresh_connector_auth_gates");
      startupMark("bridge:connector_auth_refresh:done", "elapsed_ms=" + result.elapsed_ms);
      return result;
    } catch (error) {
      startupMark("bridge:connector_auth_refresh:error", String(error));
      throw error;
    }
  }

  async function loadPlatformCapabilities() {
    try {
      state.platformCapabilities = Object.assign(
        {},
        state.platformCapabilities,
        await invoke("get_platform_capabilities"),
        { loaded: true }
      );
    } catch (error) {
      console.warn("[platform] capability detection failed", error);
    }
    notify();
    return state.platformCapabilities;
  }

  async function loadKnowledgeEmbedderAfterFirstFrame() {
    startupMark("bridge:knowledge_embedder_async:start");
    state.kbModelSetup = Object.assign({}, state.kbModelSetup, {
      startupLoading: true,
      startupReady: null,
      error: null,
    });
    notify();
    try {
      var ready = await invoke("kb_model_load_after_first_frame");
      var modelStatus = await invoke("kb_model_status").catch(function () { return null; });
      state.kbModelSetup = Object.assign({}, state.kbModelSetup, {
        startupLoading: false,
        startupReady: !!ready,
        status: modelStatus,
      });
      notify();
      startupMark("bridge:knowledge_embedder_async:done", "ready=" + !!ready);
      if (window.__PINVOU_STARTUP__) window.__PINVOU_STARTUP__.flush();
      return !!ready;
    } catch (error) {
      var failedStatus = await invoke("kb_model_status").catch(function () { return null; });
      state.kbModelSetup = Object.assign({}, state.kbModelSetup, {
        startupLoading: false,
        startupReady: false,
        status: failedStatus,
        error: String(error),
      });
      notify();
      startupMark("bridge:knowledge_embedder_async:error", String(error));
      if (window.__PINVOU_STARTUP__) window.__PINVOU_STARTUP__.flush();
      console.warn("[knowledge] embedding 后台加载失败", error);
      return false;
    }
  }

  // ── Markdown rendering (vendor scripts loaded in index.html) ─────
  // 抹平裸 <script>/<style>/<iframe> 等危险标签:它们一旦被 marked 透传成真 HTML,
  // 浏览器按 HTML 解析时 script 元素会"吞掉"后续兄弟节点直到 </script>(或文档末尾),
  // 然后 DOMPurify 把整段 script 连同被卷进去的内容一起剥掉。后果:LLM 正文里裸写
  // "在同一个 <script> 标签内……"会把后续表格/文字整段吞掉(历史上品悟报告表格踩过)。
  //
  // 关键:在 marked.parse 【之后】做替换,而不是之前。原因:marked 给代码块/inline code 的
  // 输出本身就已经把 < 转义成 &lt;(不会有真 <script>),只有用户在正文里裸写 HTML 时才会
  // 透传出 <script>。post-process 只命中后者,不会双重转义代码块里的 `<script>` 字面量。
  var DANGEROUS_TAGS_RE = /<(\/?(?:script|style|iframe|object|embed|link|meta)\b[^>]*)>/gi;
  function neutralizeRawDangerousTags(html) {
    return html.replace(DANGEROUS_TAGS_RE, function (_, inner) { return "&lt;" + inner + "&gt;"; });
  }
  function renderMarkdown(text) {
    if (window.PinvouMarkdownRenderer && typeof window.PinvouMarkdownRenderer.renderMarkdown === "function") {
      return window.PinvouMarkdownRenderer.renderMarkdown(text);
    }
    if (!window.marked || !window.DOMPurify) return escapeHtml(text);
    var html = neutralizeRawDangerousTags(marked.parse(text || ""));
    return DOMPurify.sanitize(html, {
      // 兜底:即使 neutralize 有漏网(罕见 HTML 注释/CDATA 等),DOMPurify 仍剥掉这些
      FORBID_TAGS: ["style", "iframe", "object", "embed", "link", "meta"],
      FORBID_ATTR: ["onerror", "onload", "onclick", "onmouseover", "onfocus", "onblur"],
    });
  }
  function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c];
    });
  }
  if (window.marked) {
    marked.setOptions({ gfm: true, breaks: true, headerIds: false, mangle: false });
  }

  // The pet is a separate WebView and must not own a second copy of the main
  // application state. Keep only the renderer used by its activity cards and
  // return before chat listeners, session loading, polling, or update checks.
  const locationSearch = String((window.location && window.location.search) || "");
  const isPetWindow = /(?:^|[?&])window=pet(?:&|$)/.test(locationSearch);
  const isDetachedWindow = /(?:^|[?&])detached=1(?:&|$)/.test(locationSearch);
  function detachedQueryValue(name) {
    var query = locationSearch.replace(/^\?/, "").split("&");
    for (var i = 0; i < query.length; i++) {
      var pair = query[i].split("=");
      if (pair[0] !== name) continue;
      try { return decodeURIComponent((pair.slice(1).join("=") || "").replace(/\+/g, " ")); }
      catch (_) { return pair.slice(1).join("=") || ""; }
    }
    return "";
  }
  const detachedWindowKind = isDetachedWindow ? (detachedQueryValue("kind") || "monitor") : "";
  const detachedWindowSessionId = isDetachedWindow ? detachedQueryValue("id") : "";
  if (isPetWindow) {
    window.TauriBridge = {
      available: false,
      rendering: { renderMarkdown: renderMarkdown },
    };
    return;
  }

  function installBridgeFeature(name, context) {
    var registry = window.__PINVOU_TAURI_BRIDGE_FEATURES__;
    var factory = registry && registry[name];
    if (typeof factory !== "function") throw new Error("Tauri bridge feature not loaded: " + name);
    return factory(context);
  }

  // ── State ────────────────────────────────────────────────────────
  var state = {
    sessions: [],
    archivedSessions: [],
    activeSessionId: null,
    // 模型 load_skill 触发的当前技能 id（如 'visual-design'）→ 点亮 composer 技能标；null=无。
    // 内置自动技能（视觉设计）的"正在使用"指示：新一轮用户消息时清、相关时再点亮。
    activeSkill: null,
    // 「新建对话」点击计数:每次 enterDraft() 自增(含已在草稿态的提前返回)。前端 welcomeToolId
    // 复位 effect 挂它 → 即便 activeSessionId 没变(draft→draft)也能重新求值,否则残留的工具欢迎卡
    // 会一直顶掉「你好」欢迎语(该 tool 无 welcomeQueries 时整块空白)。
    draftEpoch: 0,
    // 跨页面预填输入框请求。比如侧边栏「产出物」一级入口点击「续写/新项目」：
    // 只把草稿放进 composer，不自动发送给模型。
    composerPrefill: { id: 0, text: "" },
    // 当前会话未发送的输入草稿。只存内存，随 session working set 切换；
    // 不落盘，避免把敏感的未发送内容带到下次启动。
    composerDraft: "",
    messages: [],      // Anthropic Messages schema
    chatItems: [],     // display items for React
    // DeepSeek Turn 生命周期(user_start / assistant_done)，来自 timing_events.jsonl；
    // 纯展示诊断数据，不进入 messages 或 LLM 上下文。
    turnTimeline: [],
    activeTurnTimelineId: null,
    // 卡牌加持/卸下事件时间线(sidecar, 不进 messages/LLM)。每项 {kind,pos,...}。
    // pos = 事件发生时的 messages 数, rerender 时按 pos 插回原位, 让重载历史不割裂。
    personaEvents: [],
    // Pinvou 召唤检阅时间线(sidecar, 同 personaEvents, 不进 messages/LLM)。每项 {pos, review}。
    pinvouReviews: [],
    // 专业子模式用户消息标签(sidecar, 不进 messages/LLM)。每项 {pos, scene}。
    pinvouSceneEvents: [],
    // Pinvou 检阅结果弹窗(不进对话流);null=关闭。一次只一个,裁决/跳过直接操作它的 review、不靠 pos。
    pinvouModal: null,
    // 本 turn 被 write/append/edit 改过的产物 path(去重)。chat:done 时给每个补一张成品卡
    // (present 过的复用 title/desc;没 present 的兜底首卡),turn 内改几次都只一张。
    turnDirtyArtifacts: [],
    // 本 turn 已 present_artifact 出过成品卡的产物 path —— chat:done 兜底补卡时跳过,不重复。
    turnPresentedArtifacts: [],
    busy: false,
    monitor: null,
    backendOnline: null, // null=checking, true, false
    platformCapabilities: {
      loaded: false,
      os: "unknown",
      showMegacubeSite: false,
      showSuperPermissionSettings: false,
      usesBundledDependencyInstaller: false,
      taskCompletionNotificationsDefault: true,
      localVllmSupported: false,
      codexAcpSupported: false,
    },
    settings: null,
    selectedPet: "lingling",
    memory: {
      loading: false,
      error: null,
      profile: null,
      preferences: [],
      work_context: [],
      current_focus: [],
      recent_activity: [],
      recent_work: [],
      pending: [],
      never: [],
      runtime: null,
      snapshot_path: "",
    },
    // 「添加模型」方案:已保存模型列表 + 全局默认 id + 当前会话绑定的模型 id
    savedModels: [],
    activeModelId: null,
    currentSessionModelId: null, // 当前 active session 显式绑定的模型;null=跟随全局默认
    superPermEnabled: false,
    modeState: { mode: "yolo" },
    // 草稿态寄存的多智能体开关意图：不物化会话，首条消息创建会话时落后端。
    pendingDraftMultiAgent: false,
    // 最新 plan/todos 快照（用于 mode header 进度 chip，与 plan_ready 卡解耦）
    planSnapshot: { plan: null, todos: null },
    // 当前 session 产物列表 [{ path, basename }]
    artifacts: [],
    // 最近一次磁盘产物变更。用于刷新已打开的预览；列表是否变化不能作为唯一信号。
    artifactChange: { seq: 0, path: "", event: "", sessionId: "", at: 0 },
    // 多 session 并发:每个 session 是否正在生成 { session_id: bool }，会话列表显示「工作中」转圈
    sessionBusy: {},
    // 排队式输入:当前 session 生成中时积压的待发消息 [{ id, text, displayText, attachments }]
    queued: [],
    // 输入框待发附件 [{ id, basename, status:'parsing'|'ready'|'error', result, error }]
    attachments: [],
    attachmentDragActive: false,
    // token 预算（input_tokens / maxModelLen）
    tokens: { input: 0, max: 32768 },
    // 思考指示器：active 时 React 渲染计时气泡（Braille + 思考中/调用工具 + 秒数）
    thinking: { active: false, phase: "thinking", toolName: "", startedAt: 0 },
    // 工作流状态
    workflow: {
      skills: [],
      loadState: "idle", // idle | loading | ready | error
      activeSkillName: null,
      phases: [],
      currentPhaseId: null,
      reachedPhaseIds: [],
      bindings: {},      // session_id → skill_name
      demo: null,        // { open, name, loading, kind, content, error, description, duration }
      // 卡片流工作流运行态（无聊天，事件驱动看板）。详见 09-ui-plane 决策。
      run: {
        active: false,       // 是否有进行中的工作流
        sessionId: null,
        projectDir: null,
        scenario: null,
        status: "idle",      // idle | running | complete | blocked | stopped
        agents: {},          // role_id → { id, name, status, last_gate_verdict, outputs_present, last_run_ts, depends_on }
        cards: [],           // 底部交互卡片队列 [{ cardId, kind:'user_input'|'gate'|'system', resolved, ... }]
        selectedRole: null,  // 右抽屉选中的角色
      },
    },
    // 卡片池: 专家面具。activePersona = 当前 session 加持的专家卡(完整对象)或 null,
    // 驱动聊天室右上角挂件。
    activePersona: null,
    // 知识库挂载: 当前 session 挂载的知识集 id(number)或 null。仿 activePersona 走 buffer,
    // 仅驻内存(后端也只驻内存),重启回到未挂载。名字由前端用知识集列表解析。
    mountedCollection: null,
    mountedCollections: [],
    mountedCollectionsRevision: 0,
    // personaPool 只放轻量元信息(loadState),1078 张卡放模块级 personaPoolCache,
    // 不进 notify() 的 JSON 深拷贝(否则每个流式 token 都克隆 ~950KB,卡顿)。
    personaPool: { loadState: "idle" }, // idle | loading | ready | error
    // 应用内升级: updateInfo = check_for_update 返回值(available=true 才有意义)
    appVersion: null,
    updateInfo: null,
    webAccess: {
      active: false,
      endpoint_id: null,
      url: null,
      qr_data_url: null,
      status: "idle",
      relay_url: "",
      web_client_connected: false,
      last_error: null,
      starting: false,
    },
    updateChecking: false,
    updateCheckError: null,   // 手动检查的错误/「已是最新」提示文案
    updateDownloading: false,
    updateProgress: 0,        // 0-100
    updateReady: false,       // 安装完成,等用户点重启
    updateError: null,        // 下载/安装阶段错误(sha256/apt stderr 透传)
    updateCancelling: false,  // 用户点了取消,据此把后端「已取消下载」当正常而非错误
    // 依赖体检(设置页): deps = [{key, installed, apt}], null = 尚未检测
    deps: null,
    depsChecking: false,
    depsInstalling: false,    // 一键安装进行中(brew/apt/winget)
    depsInstallError: null,   // 安装失败原因(stderr 透传/取消/包管理器不可用)
    depsInstallProgress: null, // 安装进度 {package,current,total,detail}(后端 deps:install_progress 事件)
    // MegaCube(GB10) 本地大模型一键引导:首屏检测结果 + 引导执行态
    vllmSetup: null,          // {eligible, may_offer_setup, has_packages, engine_state:ready|starting|stopped|failed, ...}
    vllmBootstrapping: false, // 引导进行中(pkexec + 拉起 + 轮询就绪)
    vllmSetupPhase: null,     // 阶段:'authorizing'|'waiting'|'ready'(后端 vllm-setup:phase 事件驱动步骤指示)
    vllmSetupAttempt: 0,      // waiting 阶段第几次探测(后端报)
    vllmBootstrapDone: null,  // 成功结果 {base_url, model}, 据此显示「立即重启」
    vllmBootstrapError: null, // 失败原因(pkexec stderr / 超时透传)
    vllmSetupDismissed: false,// 本次会话内点了「跳过」,不再弹(不写持久标记)
    voiceInput: {
      status: "idle",         // idle | requesting_permission | recording | transcribing | completed | cancelled | failed
      message: "",
      error: null,
      category: null,
      stage: null,
      sessionId: null,
      startedAt: 0,
    },
    // 本地语音识别依赖安装引导（首次点麦克风缺组件时弹框）
    voiceAsrSetup: {
      open: false,        // 弹框是否展示
      status: null,       // voice_asr_status 返回 { engine, ffmpeg, model, ready, missing }
      installing: false,  // 安装中
      cancelling: false,  // 已请求取消，等待后端停止下载
      progress: null,     // { stage:'ffmpeg'|'model'|'cancelling'|'cancelled'|'done', downloaded, total }
      error: null,
    },
    // 知识库 embedding 模型按需下载引导（知识库页未装模型时显 gate）
    kbModelSetup: {
      downloading: false, // 下载/部署中
      startupLoading: false, // 已安装模型在首帧后的后台加载状态
      startupReady: null, // null=未知；true=当前进程可用；false=未安装或加载失败
      status: null,       // kb_model_status 返回 { installed, ready, loading, downloading, ... }
      progress: null,     // kb_model:progress 事件 { stage:'download'|'verify'|'extract'|'done', downloaded, total, ready }
      error: null,
    },
    scheduledTasks: [],
    selectedScheduledTaskId: null,
    scheduledTaskSelectionGeneration: 0,
    scheduledTaskDetail: null,
    scheduledTaskRuns: [],
    scheduledTaskRecentRuns: [],
    scheduledTaskLoading: false,
    scheduledTaskBusyAction: null,
    scheduledTaskError: null,
    scheduledTaskErrorKind: null,
    scheduledTaskDraft: null,
    scheduledTaskCreationSessionId: null,
    scheduledTaskAutoOpenId: null,
    scheduledRunContext: null,
    // 「通过聊天创建」的引导词:只随该会话首条消息发给模型,永不显示在气泡里。
    scheduledTaskPendingGuide: null,
  };
  var initPromise = null;
  // 卡片池 1078 张卡的前端缓存。只读,通过 getPersonas() 取引用,不走 notify 快照。

  // internal streaming state
  var currentStreamText = "";
  var currentStreamId = 0;
  var pendingAssistantText = "";
  var pendingAssistantBlocks = [];
  var itemIdSeq = 0;
  var toolMeta = {};       // id → { name, args }
  // 上下文行口径保护：TurnComplete 的 usage.input_tokens 是本轮所有请求的累加
  // （计费口径）。只有单请求的"干净轮"该值才等于当前上下文占用；本轮一旦出现
  // 工具调用/重试/压缩（= 多请求），就跳过这次 tokens 更新，保留上一个准确值。
  var turnUsageDirty = {};  // session_id → bool

  function safeConsoleInfo() {
    if (typeof console !== "undefined" && typeof console.info === "function") {
      console.info.apply(console, arguments);
    }
  }

  // ── bridge 层 UI 文案（系统消息/状态标签）──────────────────────
  // bridge 在事件回调里生成文案,拿不到 React 的 t;按 state.settings.language 取词,中文兜底。
  // 注意:发给 LLM 的指令不在此表,保持中文。
  var BT_TABLE = {
    en: {
      newChatFailed: "⚠️ Failed to create chat: ", loadChatFailed: "⚠️ Failed to load chat: ", deleteFailed: "⚠️ Delete failed: ",
      personaUnequipped: "🎴 Expert card removed: ",
      planHistorical: "📜 Past plan", planSuperseded: "📜 Superseded by a newer plan",
      attachStillParsing: "⚠️ Attachment still parsing, try again shortly",
      turnAlreadyInProgress: "⚠️ This chat is already processing a turn. The duplicate send was not executed.",
      compactStart: "⏳ Compacting context", compactDone: "✓ Context compacted", compactFail: "⚠️ Compaction failed", compactAuto: " (auto)",
      compactPruneMerged: "Auto-compaction: tool-result cleanup, messages unchanged",
      gpuUnavailable: "GPU info unavailable",
      cpuUnavailable: "CPU info unavailable",
      superOn: "⚠️ Super permission enabled", superOff: "Super permission disabled",
      approved: "✅ Approved", echoGo: "✅ Do it",
      acceptPlanFailed: "⚠️ accept_plan failed: ",
      planDiscarded: "🚪 Plan discarded", discardPlanFailed: "⚠️ discard_plan failed: ", exitPlanFailed: "⚠️ Failed to exit Plan: ", switchModeFailed: "⚠️ Failed to switch mode: ",
      replanRequested: "📋 Asking the AI to re-plan…",
      openFailed: "⚠️ Open failed: ", pasteImageFailed: "⚠️ Paste image failed: ",
      filePickUnavailable: "⚠️ File picker unavailable", filePickFailed: "⚠️ File selection failed: ",
      equipNoSession: "⚠️ Open or create a chat before equipping an expert", equipFailed: "⚠️ Equip failed: ",
      shellOutputOmitted: kind => `[Earlier ${kind} output omitted]`, shellUnknownExit: "unknown",
      shellTaskFinished: code => `[Task finished, exit code: ${code}]`,
      skillContentHidden: "(Skill loaded, content hidden)",
      desktopDoneSyncPending: "⚠️ The conversation finished on desktop, but the authoritative record has not synced yet; you can retry once reconnected.",
      sessionSyncingTurn: "This chat is still syncing a turn completed elsewhere. Please try again shortly.",
      targetSessionMissing: "Target chat does not exist",
      replyContentEmpty: "Reply content is empty",
      targetSessionSyncing: "The target chat is still syncing a turn completed elsewhere",
      summonNeedsSession: "Start a conversation first, then summon Pinvou to review.",
      runHasNoSession: "This run has no chat to open",
      sessionDataInvalid: "Chat data is invalid",
      voicePermissionDenied: "Microphone permission was denied. Allow this app to access the microphone in system settings, then try again.",
      voiceNoDevice: "No available microphone detected. Check that the recording device is enabled and not in use by another app.",
      voiceConstraintUnsupported: "Could not start recording: the current microphone or WebView does not support the required recording configuration. Try again; if it still fails, check the microphone settings or update system components.",
      voiceEmptyResult: "No speech was recognized. Move closer to the microphone and try again.",
      voiceContextMismatch: "Recognition finished, but the chat had already switched, so the result was not inserted.",
      voiceTimeout: "Voice input timed out. Please try again.",
      voiceRecognitionFailed: "Speech recognition failed. Please try again later.",
      voiceInputFailed: "Voice input failed. Check the microphone and try again.",
      voiceCancelled: "Voice input cancelled",
      voiceDeviceTimeout: "Microphone detection timed out; no recording device found. Check the device connection and Windows microphone settings, then try again.",
      voiceTranscribing: "Transcribing…",
      voiceRecordingTooShort: "Recording is too short. Please try again.",
      voiceWrittenBack: "Transcribed text inserted into the input box",
      voiceCheckingDevice: "Checking microphone…",
      voiceRequestingPermission: "Requesting microphone permission…",
      voiceWebviewNoMic: "This WebView does not support microphone capture.",
      voiceWebviewNoRecording: "This WebView does not support audio recording.",
      voiceNoDeviceConnect: "No available microphone detected. Connect or enable a recording device, then try again.",
      voiceRecording: "Recording… tap again to finish",
      voicePermissionDeniedRetry: "Microphone permission was denied. Tap voice input again and choose Allow in the prompt; if it still fails, check Windows microphone settings.",
      scheduledDraftInvalid: "The scheduled task draft is missing a name, task description, or schedule rule",
      scheduledCreateFailed: "Failed to create scheduled task: ",
      scheduledTaskFallbackName: "Scheduled task",
      scheduledActionBusy: "Another scheduled task operation is still in progress",
      scheduledCreateNoId: "Failed to create scheduled task: backend returned no task ID",
      scheduledChatPrefill: "I want to create a scheduled task: ",
      workflowBlockedPrefix: "⚙️ Workflow blocked: ",
      workflowBlockedUnknown: "unknown reason",
      workflowCompleteArtifact: "🎉 Workflow complete — deliverable generated",
      workflowActivateFailed: "⚠️ Failed to activate workflow: ",
      workflowCreateFailed: "⚠️ Failed to create workflow: ",
      workflowStartFailed: "⚠️ Failed to start workflow: ",
      workflowNoStoppableRun: "There is no workflow run to stop",
      workflowSubmitFailed: "⚠️ Submit failed: ",
      workflowMaterialsAdded: (count, names) => "✅ Added " + count + " material(s) to materials folder: " + names.join(", "),
      workflowFolderPickerUnavailable: "Cannot open the folder picker in this environment",
      workflowPickWorkDirTitle: "Choose a working directory",
      kbPickFolderTitle: "Choose folders to import into the knowledge base",
      workflowMediaFilterName: "Images and videos",
      workflowApproveFailed: "⚠️ Approve failed: ",
      workflowRejectFailed: "⚠️ Reject failed: ",
      workflowRejectDefaultReason: "Sent back by the user. Please improve and try again.",
      workflowRerunPrefix: "🔄 Rerun ",
      workflowRerunFailed: "⚠️ Rerun failed: ",
      memoryWriteFailed: "Memory write failed: ", memoryIgnoreFailed: "Failed to ignore memory: ", memoryNeverFailed: "Failed to set \"never ask\": ",
      attachNeedSession: "⚠️ Start a new chat before adding attachments", attachTooLarge: "Attachment exceeds the 20 MiB limit", attachEmptyFile: "Empty files cannot be added", attachAddCancelled: "Attachment add canceled", attachInvalidResult: "Attachment add returned no valid result",
      planTicketInvalid: "⚠️ The plan credential is no longer valid. Regenerate the plan before executing.",
      remoteTurnSyncing: "⚠️ This chat is still syncing a turn finished on another device. Try again shortly.",
      mountCollectionFailed: "Failed to mount collection: ",
      metricNotApplicable: "N/A", metricUnavailable: "Not provided",
      targetKindRemote: "Remote model", targetKindLocal: "Local model", targetKindInvalid: "Config error",
      betaVersionSuffix: " (Beta)",
      depsInstallManual: "The missing items cannot be installed in one click. Install them as described in the notes above each missing item, then re-check.",
      remoteCmdNotAllowed: cmd => "Remote control does not allow this command: " + cmd,
      remoteDialogDesktop: "Remote control uses the desktop file picker",
      echoOtherPrefix: "(Other) ",
      newChatFallbackTitle: "New chat",
    },
    ja: {
      newChatFailed: "⚠️ 新規チャットの作成に失敗: ", loadChatFailed: "⚠️ チャットの読み込みに失敗: ", deleteFailed: "⚠️ 削除に失敗: ",
      personaUnequipped: "🎴 エキスパートカードを外しました: ",
      planHistorical: "📜 過去のプラン", planSuperseded: "📜 新しいプランで上書きされました",
      attachStillParsing: "⚠️ 添付ファイルを解析中です。少し待ってから送信してください",
      turnAlreadyInProgress: "⚠️ このチャットでは別のターンを処理中です。重複した送信は実行されませんでした。",
      compactStart: "⏳ コンテキストを圧縮中", compactDone: "✓ コンテキスト圧縮完了", compactFail: "⚠️ 圧縮に失敗", compactAuto: "（自動）",
      compactPruneMerged: "自動圧縮: ツール結果を整理、メッセージ数は不変",
      gpuUnavailable: "GPU 情報を取得できません",
      cpuUnavailable: "CPU 情報を取得できません",
      superOn: "⚠️ スーパー権限が有効になりました", superOff: "スーパー権限が無効になりました",
      approved: "✅ 承認済み", echoGo: "✅ これでいく",
      acceptPlanFailed: "⚠️ accept_plan に失敗: ",
      planDiscarded: "🚪 プランを破棄", discardPlanFailed: "⚠️ discard_plan に失敗: ", exitPlanFailed: "⚠️ Plan の終了に失敗: ", switchModeFailed: "⚠️ モード切替に失敗: ",
      replanRequested: "📋 AI にプランを出し直させています…",
      openFailed: "⚠️ 開けませんでした: ", pasteImageFailed: "⚠️ 画像の貼り付けに失敗: ",
      filePickUnavailable: "⚠️ ファイル選択を利用できません", filePickFailed: "⚠️ ファイル選択に失敗: ",
      equipNoSession: "⚠️ エキスパートを装備する前にチャットを開くか新規作成してください", equipFailed: "⚠️ 装備に失敗: ",
      shellOutputOmitted: kind => `[途中の${kind === "stderr" ? "標準エラー" : "標準出力"}を省略]`, shellUnknownExit: "不明",
      shellTaskFinished: code => `[タスク終了、終了コード: ${code}]`,
      skillContentHidden: "（スキルを読み込みました。内容は非表示です）",
      desktopDoneSyncPending: "⚠️ 会話はデスクトップ側で完了しましたが、権威レコードはまだ同期されていません。接続回復後に再試行できます。",
      sessionSyncingTurn: "このチャットは別端末で完了したターンを同期中です。しばらくしてから再試行してください",
      targetSessionMissing: "対象のチャットが存在しません",
      replyContentEmpty: "返信内容が空です",
      targetSessionSyncing: "対象のチャットは別端末で完了したターンをまだ同期中です",
      summonNeedsSession: "先に会話を始めてから Pinvou レビューを召喚してください。",
      runHasNoSession: "この実行記録には開けるセッションがありません",
      sessionDataInvalid: "セッションデータが無効です",
      voicePermissionDenied: "マイクへのアクセスが拒否されました。システム設定でこのアプリのマイクアクセスを許可してから再試行してください。",
      voiceNoDevice: "利用可能なマイクが見つかりません。録音デバイスが有効か、他で使用されていないか確認してください。",
      voiceConstraintUnsupported: "録音を開始できません：現在のマイクまたは WebView が必要な録音設定に対応していません。再試行し、それでも失敗する場合はマイク設定やシステムコンポーネントを確認・更新してください。",
      voiceEmptyResult: "音声を認識できませんでした。マイクに近づいて再試行してください。",
      voiceContextMismatch: "認識は完了しましたが、セッションが切り替わったため結果は自動入力されませんでした。",
      voiceTimeout: "音声入力がタイムアウトしました。再試行してください。",
      voiceRecognitionFailed: "音声認識に失敗しました。しばらくしてから再試行してください。",
      voiceInputFailed: "音声入力に失敗しました。マイクを確認して再試行してください。",
      voiceCancelled: "音声入力をキャンセルしました",
      voiceDeviceTimeout: "マイク検出がタイムアウトし、録音デバイスが見つかりませんでした。デバイスの接続と Windows のマイク設定を確認して再試行してください。",
      voiceTranscribing: "音声を認識中…",
      voiceRecordingTooShort: "録音時間が短すぎます。再試行してください。",
      voiceWrittenBack: "音声を入力ボックスに書き込みました",
      voiceCheckingDevice: "マイクデバイスを確認中…",
      voiceRequestingPermission: "マイクの権限をリクエスト中…",
      voiceWebviewNoMic: "この WebView はマイク入力に対応していません。",
      voiceWebviewNoRecording: "この WebView は音声録音に対応していません。",
      voiceNoDeviceConnect: "利用可能なマイクが見つかりません。録音デバイスを接続または有効にして再試行してください。",
      voiceRecording: "録音中です。もう一度タップすると終了します",
      voicePermissionDeniedRetry: "マイクの権限が拒否されています。もう一度音声入力をタップし、許可を選択してください。それでも失敗する場合は Windows のマイク設定を確認してください。",
      scheduledDraftInvalid: "スケジュールタスクの下書きに名前・タスク説明・時間ルールのいずれかが不足しています",
      scheduledCreateFailed: "スケジュールタスクの作成に失敗：",
      scheduledTaskFallbackName: "スケジュールタスク",
      scheduledActionBusy: "別のスケジュールタスク操作がまだ実行中です",
      scheduledCreateNoId: "スケジュールタスクの作成に失敗：バックエンドがタスク ID を返しませんでした",
      scheduledChatPrefill: "スケジュールタスクを作成したい：",
      workflowBlockedPrefix: "⚙️ ワークフローが停止：",
      workflowBlockedUnknown: "原因不明",
      workflowCompleteArtifact: "🎉 ワークフロー完了、成果物が生成されました",
      workflowActivateFailed: "⚠️ ワークフローの有効化に失敗: ",
      workflowCreateFailed: "⚠️ ワークフローの作成に失敗: ",
      workflowStartFailed: "⚠️ ワークフローの開始に失敗: ",
      workflowNoStoppableRun: "停止できるワークフローがありません",
      workflowSubmitFailed: "⚠️ 送信に失敗: ",
      workflowMaterialsAdded: (count, names) => "✅ 素材を " + count + " 件、付属資料に追加しました：" + names.join("、"),
      workflowFolderPickerUnavailable: "現在の環境ではフォルダー選択を開けません",
      workflowPickWorkDirTitle: "作業ディレクトリを選択",
      kbPickFolderTitle: "知識ベースにインポートするフォルダーを選択",
      workflowMediaFilterName: "画像と動画",
      workflowApproveFailed: "⚠️ 承認に失敗: ",
      workflowRejectFailed: "⚠️ 差し戻しに失敗: ",
      workflowRejectDefaultReason: "ユーザーによる差し戻し。改善して再試行してください。",
      workflowRerunPrefix: "🔄 再実行 ",
      workflowRerunFailed: "⚠️ 再実行に失敗: ",
      memoryWriteFailed: "メモリの書き込みに失敗: ", memoryIgnoreFailed: "メモリの無視に失敗: ", memoryNeverFailed: "「今後表示しない」の設定に失敗: ",
      attachNeedSession: "⚠️ 添付ファイルを追加する前に新しいチャットを開始してください", attachTooLarge: "添付ファイルが 20 MiB の上限を超えています", attachEmptyFile: "空のファイルは追加できません", attachAddCancelled: "添付ファイルの追加はキャンセルされました", attachInvalidResult: "添付ファイルの追加で有効な結果が返されませんでした",
      planTicketInvalid: "⚠️ プランの資格情報が無効になりました。プランを再生成してから実行してください。",
      remoteTurnSyncing: "⚠️ このセッションは別の端末で完了したターンを同期中です。しばらくしてから再試行してください。",
      mountCollectionFailed: "ナレッジセットのマウントに失敗: ",
      metricNotApplicable: "対象外", metricUnavailable: "未提供",
      targetKindRemote: "リモートモデル", targetKindLocal: "ローカルモデル", targetKindInvalid: "設定エラー",
      betaVersionSuffix: " (ベータ版)",
      depsInstallManual: "不足している項目はワンクリックでインストールできません。各不足項目の上にある説明に従ってインストールしてから、再検出してください。",
      remoteCmdNotAllowed: cmd => "リモートコントロールではこのコマンドを呼び出せません: " + cmd,
      remoteDialogDesktop: "リモートコントロールではデスクトップ側のファイル選択ダイアログを使用します",
      echoOtherPrefix: "(その他) ",
      newChatFallbackTitle: "新しいチャット",
    },
    zh: {
      newChatFailed: "⚠️ 新建对话失败: ", loadChatFailed: "⚠️ 加载对话失败: ", deleteFailed: "⚠️ 删除失败: ",
      personaUnequipped: "🎴 已卸下专家卡牌: ",
      planHistorical: "📜 历史方案", planSuperseded: "📜 已被新方案覆盖",
      attachStillParsing: "⚠️ 附件还在解析,请稍后再发",
      turnAlreadyInProgress: "⚠️ 当前会话已有一轮正在处理，本次重复发送未执行。",
      compactStart: "⏳ 正在压缩上下文", compactDone: "✓ 上下文压缩完成", compactFail: "⚠️ 压缩失败", compactAuto: "（自动）",
      compactPruneMerged: "自动压缩：已整理工具结果，消息数不变",
      gpuUnavailable: "GPU 信息不可用",
      cpuUnavailable: "CPU 信息不可用",
      superOn: "⚠️ 超级权限已开启", superOff: "超级权限已关闭",
      approved: "✅ 已批准", echoGo: "✅ 就这么干",
      acceptPlanFailed: "⚠️ accept_plan 失败: ",
      planDiscarded: "🚪 已放弃此方案", discardPlanFailed: "⚠️ discard_plan 失败: ", exitPlanFailed: "⚠️ 退出 Plan 失败: ", switchModeFailed: "⚠️ 切换模式失败: ",
      replanRequested: "📋 让 AI 重出方案…",
      openFailed: "⚠️ 打开失败: ", pasteImageFailed: "⚠️ 粘贴图片失败: ",
      filePickUnavailable: "⚠️ 文件选择不可用", filePickFailed: "⚠️ 选择文件失败: ",
      equipNoSession: "⚠️ 请先打开或新建一个对话再加持专家", equipFailed: "⚠️ 加持失败: ",
      shellOutputOmitted: kind => `[中间${kind === "stderr" ? "错误" : "标准"}输出已省略]`, shellUnknownExit: "未知",
      shellTaskFinished: code => `[任务已结束，退出码: ${code}]`,
      skillContentHidden: "（技能已加载，内容不展示）",
      desktopDoneSyncPending: "⚠️ 对话已在桌面端完成，但权威记录暂未同步；恢复连接后可重试。",
      sessionSyncingTurn: "该会话正在同步另一端完成的回合，请稍后重试",
      targetSessionMissing: "目标会话不存在",
      replyContentEmpty: "回复内容为空",
      targetSessionSyncing: "目标会话仍在同步另一端完成的回合",
      summonNeedsSession: "先开始一个对话,再召唤 Pinvou 检阅。",
      runHasNoSession: "该运行记录没有可打开的会话",
      sessionDataInvalid: "会话数据无效",
      voicePermissionDenied: "麦克风权限被拒绝，请在系统设置中允许本应用访问麦克风后重试。",
      voiceNoDevice: "未检测到可用麦克风，请检查录音设备是否启用或被占用。",
      voiceConstraintUnsupported: "无法启动录音：当前麦克风或 WebView 不支持所需的录音配置。请重试；若仍失败，请检查麦克风设置或更新系统组件。",
      voiceEmptyResult: "未识别到语音内容，请靠近麦克风后重试。",
      voiceContextMismatch: "识别已完成，但当前会话已切换，结果未自动写入。",
      voiceTimeout: "本次语音输入超时，请重试。",
      voiceRecognitionFailed: "语音识别失败，请稍后重试。",
      voiceInputFailed: "语音输入失败，请检查麦克风后重试。",
      voiceCancelled: "已取消语音输入",
      voiceDeviceTimeout: "麦克风检测超时，未发现可用录音设备。请检查设备连接和 Windows 麦克风设置后重试。",
      voiceTranscribing: "正在识别语音…",
      voiceRecordingTooShort: "录音时间过短，请重试。",
      voiceWrittenBack: "语音已写入输入框",
      voiceCheckingDevice: "正在检测麦克风设备…",
      voiceRequestingPermission: "正在请求麦克风权限…",
      voiceWebviewNoMic: "当前 WebView 不支持麦克风采集。",
      voiceWebviewNoRecording: "当前 WebView 不支持音频录制。",
      voiceNoDeviceConnect: "未检测到可用麦克风，请连接或启用录音设备后重试。",
      voiceRecording: "正在录音，再点一次结束",
      voicePermissionDeniedRetry: "麦克风权限已被拒绝，请再次点击语音输入并在授权提示中选择允许；若仍失败，请检查 Windows 麦克风设置。",
      scheduledDraftInvalid: "定时任务草稿缺少名称、任务说明或时间规则",
      scheduledCreateFailed: "定时任务创建失败：",
      scheduledTaskFallbackName: "定时任务",
      scheduledActionBusy: "另一个定时任务操作仍在进行中",
      scheduledCreateNoId: "创建定时任务失败：后端未返回任务 ID",
      scheduledChatPrefill: "我想创建一个定时任务：",
      workflowBlockedPrefix: "⚙️ 工作流卡住：",
      workflowBlockedUnknown: "未知原因",
      workflowCompleteArtifact: "🎉 工作流完成，成品已生成",
      workflowActivateFailed: "⚠️ 启用工作流失败: ",
      workflowCreateFailed: "⚠️ 创建工作流失败: ",
      workflowStartFailed: "⚠️ 启动工作流失败: ",
      workflowNoStoppableRun: "当前没有可停止的工作流",
      workflowSubmitFailed: "⚠️ 提交失败: ",
      workflowMaterialsAdded: (count, names) => "✅ 已添加 " + count + " 个素材到配套材料：" + names.join("、"),
      workflowFolderPickerUnavailable: "当前环境无法打开文件夹选择器",
      workflowPickWorkDirTitle: "选择工作目录",
      kbPickFolderTitle: "选择要导入知识库的文件夹",
      workflowMediaFilterName: "图片和视频",
      workflowApproveFailed: "⚠️ 通过失败: ",
      workflowRejectFailed: "⚠️ 打回失败: ",
      workflowRejectDefaultReason: "用户打回，请改进后重试",
      workflowRerunPrefix: "🔄 重跑 ",
      workflowRerunFailed: "⚠️ 重跑失败: ",
      memoryWriteFailed: "记忆写入失败：", memoryIgnoreFailed: "忽略记忆失败：", memoryNeverFailed: "设置不再提示失败：",
      attachNeedSession: "⚠️ 请先新建会话再添加附件", attachTooLarge: "附件超过 20 MiB 上限", attachEmptyFile: "空文件无法添加", attachAddCancelled: "附件添加已取消", attachInvalidResult: "附件添加未返回有效结果",
      planTicketInvalid: "⚠️ 方案凭证已失效，请重新生成方案后再执行",
      remoteTurnSyncing: "⚠️ 该会话仍在同步另一端完成的回合，请稍后重试",
      mountCollectionFailed: "挂载知识集失败: ",
      metricNotApplicable: "不适用", metricUnavailable: "未提供",
      targetKindRemote: "远端模型", targetKindLocal: "本地模型", targetKindInvalid: "配置异常",
      betaVersionSuffix: " (内测版)",
      depsInstallManual: "当前缺失项无法一键安装，请按上方各缺失项的说明手动安装后重新检测。",
      remoteCmdNotAllowed: cmd => "远程控制不允许调用该命令：" + cmd,
      remoteDialogDesktop: "远程控制使用桌面端文件选择器",
      echoOtherPrefix: "(其他) ",
      newChatFallbackTitle: "新对话",
    },
  };
  function bt(key) {
    var lang = state.settings && state.settings.language;
    var m = lang === "en" ? BT_TABLE.en : lang === "ja" ? BT_TABLE.ja : BT_TABLE.zh;
    return m[key] !== undefined ? m[key] : BT_TABLE.zh[key];
  }
  // 默认会话标题哨兵:三语兜底标题都视为占位(自动改名/显示映射的依据),
  // 与 web 桥和 main.jsx 的同款判断保持一致。
  function isDefaultChatTitle(title) {
    return title === BT_TABLE.zh.newChatFallbackTitle
      || title === BT_TABLE.en.newChatFallbackTitle
      || title === BT_TABLE.ja.newChatFallbackTitle;
  }

  // ── Per-session 工作集缓冲（多 session 并发）────────────────────
  // active session 的工作集 = state.* + 上面那批模块级 stream 变量(保持原逻辑零改动)。
  // 后台 session 的工作集存在 sessionStates[id];后台事件进来时临时把工作集切到对应
  // buffer 跑同步逻辑再切回(saveWorkingSetTo/loadWorkingSetFrom),期间 suppressNotify
  // 避免把后台渲染成 active。异步收尾(落盘)按显式 session_id 路由,不依赖工作集。
  var sessionStates = {};
  var authoritativeTranscriptSyncs = Object.create(null);
  var scheduledRunSessionOwners = Object.create(null);
  var MAX_SCHEDULED_SESSION_BUFFERS = 64;
  var MAX_SCHEDULED_RUN_SESSION_OWNERS = 64;
  var suppressNotify = false;
  function discardManagedAttachment(result) {
    var sessionId = result && result.__pinvouManagedAttachmentSessionId;
    if (!sessionId || !result.path) return Promise.resolve();
    return invoke("discard_dropped_attachment", {
      sessionId: sessionId,
      path: result.path,
    }).catch(function (error) {
      console.warn("[attachment] failed to discard managed attachment", error);
    });
  }
  // sessionId → true:标题当前是「卡牌占位名」(加卡时自动取的),可被首条用户消息覆盖。
  // 卡牌名只在「加了卡但还没开口」时当临时标题;一旦开始对话,对话内容更能区分同卡会话。
  // 内存态(不持久化):重启后丢标记仅影响「加卡→重启→才发首条消息」这一冷门路径。
  var personaPlaceholderTitles = {};
  var PINVOU_SCENE_EVENTS_STORAGE_PREFIX = "pinvou_scene_events_v1:";
  function normalizePinvouScene(scene) {
    scene = String(scene || "").trim();
    return /^(work:document-writing|work:personal-workbench|design:poster|design:data-visualization)$/.test(scene) ? scene : "";
  }
  function pinvouSceneStorageKey(sid) {
    return PINVOU_SCENE_EVENTS_STORAGE_PREFIX + String(sid || "").trim();
  }
  function normalizePinvouSceneEvents(events) {
    return (Array.isArray(events) ? events : []).map(function (event) {
      var pos = Number(event && event.pos);
      var scene = normalizePinvouScene(event && event.scene);
      if (!Number.isFinite(pos) || pos < 0 || !scene) return null;
      return { pos: Math.floor(pos), scene: scene };
    }).filter(Boolean).sort(function (left, right) { return left.pos - right.pos; });
  }
  function loadPinvouSceneEventsForSession(sid) {
    if (!sid || !window.localStorage) return [];
    try {
      return normalizePinvouSceneEvents(JSON.parse(window.localStorage.getItem(pinvouSceneStorageKey(sid)) || "[]"));
    } catch (_) {
      return [];
    }
  }
  function savePinvouSceneEventsForSession(sid, events) {
    if (!sid) return;
    var normalized = normalizePinvouSceneEvents(events);
    try {
      if (window.localStorage) {
        window.localStorage.setItem(pinvouSceneStorageKey(sid), JSON.stringify(normalized));
      }
    } catch (_) {
      // localStorage 只作旧版本迁移和离线缓存，写失败不影响后端 sidecar。
    }
    Promise.resolve().then(function () {
      return invoke("save_session_pinvou_scene_events", {
        sessionId: sid,
        events: normalized,
      });
    }).catch(function () {});
  }
  async function syncPinvouSceneEventsForSession(sid) {
    var cached = loadPinvouSceneEventsForSession(sid);
    if (!sid) return cached;
    try {
      var remote = normalizePinvouSceneEvents(
        await invoke("get_session_pinvou_scene_events", { sessionId: sid })
      );
      if (remote.length) {
        try {
          window.localStorage.setItem(pinvouSceneStorageKey(sid), JSON.stringify(remote));
        } catch (_) {}
        return remote;
      }
      if (cached.length) {
        await invoke("save_session_pinvou_scene_events", { sessionId: sid, events: cached });
      }
      return cached;
    } catch (_) {
      return cached;
    }
  }
  function recordPinvouSceneForMessage(sid, pos, scene) {
    scene = normalizePinvouScene(scene);
    pos = Number(pos);
    if (!sid || !scene || !Number.isFinite(pos) || pos < 0) return;
    pos = Math.floor(pos);
    var events = normalizePinvouSceneEvents(state.pinvouSceneEvents)
      .filter(function (event) { return event.pos !== pos; });
    events.push({ pos: pos, scene: scene });
    events = normalizePinvouSceneEvents(events);
    state.pinvouSceneEvents = events;
    savePinvouSceneEventsForSession(sid, events);
  }
  function pinvouSceneForMessagePos(pos) {
    var events = normalizePinvouSceneEvents(state.pinvouSceneEvents);
    for (var i = 0; i < events.length; i++) {
      if (events[i].pos === pos) return events[i].scene;
    }
    return "";
  }
  var artifactTrackerFeature = installBridgeFeature("artifact-tracker", {
    state: state, invoke: invoke, sessionStates: sessionStates,
    notify: function () { return notify.apply(null, arguments); },
    isScheduledRunSession: function () { return isScheduledRunSession.apply(null, arguments); },
  });
  var basename = artifactTrackerFeature.basename;
  var isAbsPath = artifactTrackerFeature.isAbsPath;
  var normalizedPath = artifactTrackerFeature.normalizedPath;
  var noteArtifactChange = artifactTrackerFeature.noteArtifactChange;
  var isSharedMcpArtifactPath = artifactTrackerFeature.isSharedMcpArtifactPath;
  var artifactBelongsToSession = artifactTrackerFeature.artifactBelongsToSession;
  var filterSessionArtifacts = artifactTrackerFeature.filterSessionArtifacts;
  var isDeliverable = artifactTrackerFeature.isDeliverable;
  var trackArtifact = artifactTrackerFeature.trackArtifact;
  var markTurnDirtyArtifact = artifactTrackerFeature.markTurnDirtyArtifact;
  var untrackArtifact = artifactTrackerFeature.untrackArtifact;
  var findPresentedArtifact = artifactTrackerFeature.findPresentedArtifact;
  var reconcileArtifacts = artifactTrackerFeature.reconcileArtifacts;
  var extractArtifactPaths = artifactTrackerFeature.extractArtifactPaths;
  var extractArtifactPath = artifactTrackerFeature.extractArtifactPath;
  var fileMutationAction = artifactTrackerFeature.fileMutationAction;
  var isPresentArtifactTool = artifactTrackerFeature.isPresentArtifactTool;
  var parseToolResultPayload = artifactTrackerFeature.parseToolResultPayload;
  var artifactPathFromToolOutput = artifactTrackerFeature.artifactPathFromToolOutput;
  var shouldUseToolOutputAsArtifact = artifactTrackerFeature.shouldUseToolOutputAsArtifact;
  var presentArtifactAbsPath = artifactTrackerFeature.presentArtifactAbsPath;

  var chatFeature = installBridgeFeature("chat", {
    state: state, invoke: invoke, TAURI: TAURI,
    sessionStates: sessionStates, turnUsageDirty: turnUsageDirty,
    personaPlaceholderTitles: personaPlaceholderTitles,
    renderMarkdown: renderMarkdown, safeConsoleInfo: safeConsoleInfo, bt: bt,
    isDefaultChatTitle: isDefaultChatTitle,
    notify: function () { return notify.apply(null, arguments); },
    runSyncOnSession: function () { return runSyncOnSession.apply(null, arguments); },
    startThinking: function () { return startThinking.apply(null, arguments); },
    stopThinking: function () { return stopThinking.apply(null, arguments); },
    ensureSessionBufferLoaded: function () { return ensureSessionBufferLoaded.apply(null, arguments); },
    ensureSession: function () { return ensureSession.apply(null, arguments); },
    getBuffer: function () { return getBuffer.apply(null, arguments); },
    recordPinvouSceneForMessage: recordPinvouSceneForMessage,
    reconcileRemoteTurn: function () { return reconcileRemoteTurn.apply(null, arguments); },
    markRemoteTurn: function () { return markRemoteTurn.apply(null, arguments); },
    clearAttachments: function () { return clearAttachments.apply(null, arguments); },
    discardManagedAttachment: discardManagedAttachment,
    isScheduledRunSession: function () { return isScheduledRunSession.apply(null, arguments); },
    basename: basename,
    extractArtifactPath: extractArtifactPath,
    fileMutationAction: fileMutationAction,
    parseScheduledTaskDraftFromText: function () { return parseScheduledTaskDraftFromText.apply(null, arguments); },
    autoCreateScheduledTaskDraft: function () { return autoCreateScheduledTaskDraft.apply(null, arguments); },
    get currentStreamText() { return currentStreamText; },
    set currentStreamText(value) { currentStreamText = value; },
    get currentStreamId() { return currentStreamId; },
    set currentStreamId(value) { currentStreamId = value; },
    get pendingAssistantText() { return pendingAssistantText; },
    set pendingAssistantText(value) { pendingAssistantText = value; },
    get pendingAssistantBlocks() { return pendingAssistantBlocks; },
    set pendingAssistantBlocks(value) { pendingAssistantBlocks = value; },
    get itemIdSeq() { return itemIdSeq; },
    set itemIdSeq(value) { itemIdSeq = value; },
  });
  var addChatItem = chatFeature.addChatItem;
  var toolCallAlreadyStarted = chatFeature.toolCallAlreadyStarted;
  var toolCallAlreadyFinished = chatFeature.toolCallAlreadyFinished;
  var hasChatItemForTool = chatFeature.hasChatItemForTool;
  var isDuplicateArtifactCard = chatFeature.isDuplicateArtifactCard;
  var addSystemItem = chatFeature.addSystemItem;
  var addAuthoritySyncNotice = chatFeature.addAuthoritySyncNotice;
  var compactPruneRollupText = chatFeature.compactPruneRollupText;
  var removeCompactionStartItem = chatFeature.removeCompactionStartItem;
  var addOrMergePruneCompaction = chatFeature.addOrMergePruneCompaction;
  var timeStr = chatFeature.timeStr;
  var flushPendingTextBlock = chatFeature.flushPendingTextBlock;
  var flushAssistantMessageToHistory = chatFeature.flushAssistantMessageToHistory;
  var resetPendingAssistant = chatFeature.resetPendingAssistant;
  var isBusyFor = chatFeature.isBusyFor;
  var emitPetEvent = chatFeature.emitPetEvent;
  var doSendFor = chatFeature.doSendFor;
  var publishRemoteUserMessage = chatFeature.publishRemoteUserMessage;
  var flushQueued = chatFeature.flushQueued;
  var sendMessageToSession = chatFeature.sendMessageToSession;
  var sendMessage = chatFeature.sendMessage;
  var getComposerDraft = chatFeature.getComposerDraft;
  var setComposerDraft = chatFeature.setComposerDraft;
  var retryFirstTurn = chatFeature.retryFirstTurn;
  var prefillComposer = chatFeature.prefillComposer;
  var removeQueued = chatFeature.removeQueued;
  var summonPinvou = chatFeature.summonPinvou;
  var inspectPinvou = chatFeature.inspectPinvou;
  var recordPinvouReview = chatFeature.recordPinvouReview;
  var resolvePinvouReview = chatFeature.resolvePinvouReview;
  var dismissPinvouReview = chatFeature.dismissPinvouReview;
  var persistPinvouReviews = chatFeature.persistPinvouReviews;
  var cancelGeneration = chatFeature.cancelGeneration;
  var persistMessages = chatFeature.persistMessages;

  var sessionsFeature = installBridgeFeature("sessions", {
    state: state, invoke: invoke, listen: listen, notify: notify,
    sessionStates: sessionStates, scheduledRunSessionOwners: scheduledRunSessionOwners,
    personaPlaceholderTitles: personaPlaceholderTitles, turnUsageDirty: turnUsageDirty,
    runSyncOnSession: runSyncOnSession, persistMessagesFor: persistMessagesFor,
    resetPendingAssistant: function () { return resetPendingAssistant.apply(null, arguments); },
    stopThinking: function () { return stopThinking.apply(null, arguments); },
    rerenderFromMessages: rerenderFromMessages,
    syncModeState: function () { return syncModeState.apply(null, arguments); },
    syncActivePersona: function () { return syncActivePersona(); },
    syncMountedCollection: function () { return syncMountedCollection(); },
    reconcileArtifacts: reconcileArtifacts,
    loadSessionModel: function () { return loadSessionModel.apply(null, arguments); },
    clearScheduledTaskSelection: function () { return clearScheduledTaskSelection(); },
    invalidateScheduledRecentRunsForSession: function () { return invalidateScheduledRecentRunsForSession.apply(null, arguments); },
    setScheduledTaskError: function () { return setScheduledTaskError.apply(null, arguments); },
    invalidateScheduledTaskReads: function () { return invalidateScheduledTaskReads.apply(null, arguments); },
    applyScheduledRunViewed: function () { return applyScheduledRunViewed.apply(null, arguments); },
    loadScheduledTaskRecentRuns: function () { return loadScheduledTaskRecentRuns.apply(null, arguments); },
    addSystemItem: addSystemItem, basename: basename,
    isAbsPath: isAbsPath,
    filterSessionArtifacts: filterSessionArtifacts,
    scheduleShellPoll: function () { return scheduleShellPoll.apply(null, arguments); },
    bt: bt, userMessageDisplayText: userMessageDisplayText,
    loadPinvouSceneEventsForSession: loadPinvouSceneEventsForSession,
    syncPinvouSceneEventsForSession: syncPinvouSceneEventsForSession,
    loadMemoryOverview: function () { return loadMemoryOverview.apply(null, arguments); },
    isScheduledRunSession: isScheduledRunSession,
    get currentStreamText() { return currentStreamText; },
    set currentStreamText(value) { currentStreamText = value; },
    get currentStreamId() { return currentStreamId; },
    set currentStreamId(value) { currentStreamId = value; },
    get pendingAssistantText() { return pendingAssistantText; },
    set pendingAssistantText(value) { pendingAssistantText = value; },
    get pendingAssistantBlocks() { return pendingAssistantBlocks; },
    set pendingAssistantBlocks(value) { pendingAssistantBlocks = value; },
    get itemIdSeq() { return itemIdSeq; },
    set itemIdSeq(value) { itemIdSeq = value; },
    get toolMeta() { return toolMeta; },
    set toolMeta(value) { toolMeta = value; },
  });
  var freshBuffer = sessionsFeature.freshBuffer;
  var getBuffer = sessionsFeature.getBuffer;
  var isProtectedScheduledBuffer = sessionsFeature.isProtectedScheduledBuffer;
  var pruneScheduledSessionBuffers = sessionsFeature.pruneScheduledSessionBuffers;
  var touchSessionBuffer = sessionsFeature.touchSessionBuffer;
  var purgeSessionBuffer = sessionsFeature.purgeSessionBuffer;
  var registerScheduledRunOwner = sessionsFeature.registerScheduledRunOwner;
  var scheduledRunOwnerVisibleRank = sessionsFeature.scheduledRunOwnerVisibleRank;
  var scheduledRunOwnerPriority = sessionsFeature.scheduledRunOwnerPriority;
  var isProtectedScheduledRunOwner = sessionsFeature.isProtectedScheduledRunOwner;
  var pruneScheduledRunSessionOwner = sessionsFeature.pruneScheduledRunSessionOwner;
  var pruneScheduledRunSessionOwners = sessionsFeature.pruneScheduledRunSessionOwners;
  var isScheduledRunTerminal = sessionsFeature.isScheduledRunTerminal;
  var rememberScheduledRunOwner = sessionsFeature.rememberScheduledRunOwner;
  var scheduledRunBuffer = sessionsFeature.scheduledRunBuffer;
  var markScheduledInitialTurnActive = sessionsFeature.markScheduledInitialTurnActive;
  var markScheduledInitialTurnTerminal = sessionsFeature.markScheduledInitialTurnTerminal;
  var beginScheduledOpenActivation = sessionsFeature.beginScheduledOpenActivation;
  var rollbackScheduledOpenActivation = sessionsFeature.rollbackScheduledOpenActivation;
  var saveWorkingSetTo = sessionsFeature.saveWorkingSetTo;
  var loadWorkingSetFrom = sessionsFeature.loadWorkingSetFrom;
  var hydrateWorkingSetFromSaved = sessionsFeature.hydrateWorkingSetFromSaved;
  var ensureSessionBufferLoaded = sessionsFeature.ensureSessionBufferLoaded;
  var switchActiveTo = sessionsFeature.switchActiveTo;
  var refreshHistoryList = sessionsFeature.refreshHistoryList;
  var enterDraft = sessionsFeature.enterDraft;
  var createNewSession = sessionsFeature.createNewSession;
  var ensureSession = sessionsFeature.ensureSession;
  var reportSessionSwitchFailure = sessionsFeature.reportSessionSwitchFailure;
  var hydratedMessageKey = sessionsFeature.hydratedMessageKey;
  var mergeHydratedMessages = sessionsFeature.mergeHydratedMessages;
  var mergeHydratedArtifacts = sessionsFeature.mergeHydratedArtifacts;
  var hydratedChatItemKey = sessionsFeature.hydratedChatItemKey;
  var mergeHydratedChatItems = sessionsFeature.mergeHydratedChatItems;
  var switchToSessionInternal = sessionsFeature.switchToSessionInternal;
  var switchToSession = sessionsFeature.switchToSession;
  var openScheduledRunChatOnce = sessionsFeature.openScheduledRunChatOnce;
  var openScheduledRunChat = sessionsFeature.openScheduledRunChat;
  var exitScheduledRunChat = sessionsFeature.exitScheduledRunChat;
  var recentScheduledRunForSession = sessionsFeature.recentScheduledRunForSession;
  var leaveSessionView = sessionsFeature.leaveSessionView;
  var deleteSession = sessionsFeature.deleteSession;
  var renameSession = sessionsFeature.renameSession;
  var toggleSessionPinned = sessionsFeature.toggleSessionPinned;
  var archiveSession = sessionsFeature.archiveSession;
  var restoreArchivedSession = sessionsFeature.restoreArchivedSession;
  function runSyncOnSession(sid, fn) {
    if (!sid || sid === state.activeSessionId) { fn(); return; }
    var bg = sessionStates[sid]; if (!bg) return;
    touchSessionBuffer(sid, bg, isScheduledRunSession(sid));
    var realId = state.activeSessionId;
    var draftComposer = realId ? "" : (state.composerDraft || "");
    saveWorkingSetTo(getBuffer(realId));
    loadWorkingSetFrom(bg);
    state.activeSessionId = sid;
    var prev = suppressNotify; suppressNotify = true;
    try { fn(); }
    finally {
      suppressNotify = prev;
      saveWorkingSetTo(bg);
      state.activeSessionId = realId;
      // realId 为 null(草稿态)时 getBuffer(null)=null、loadWorkingSetFrom(null) 是 no-op,
      // 会把刚处理的后台 session 工作集泄漏进草稿视图(activeSessionId=null 却带着它的 chatItems),
      // 召唤检阅等依赖 activeSessionId 的操作随之错乱。草稿态须切回干净工作集，
      // 但要保留用户正在输入的未发送文本。
      var restoreBuffer = realId ? getBuffer(realId) : freshBuffer();
      if (!realId) restoreBuffer.composerDraft = draftComposer;
      loadWorkingSetFrom(restoreBuffer);
    }
  }
  // 事件监听器统一入口:按 payload.session_id 路由同步逻辑;后台变更后补一次 notify 刷新列表。
  function markRemoteTurn(sid, buf, preserveCommittedRevision) {
    if (!sid || !buf || buf.localTurnOwned) return;
    if (!buf.remoteTurnActive) {
      var meta = state.sessions.find(function (session) { return session.id === sid; });
      buf.remoteBaselineTrusted = !!buf.loadedFromDisk;
      buf.remoteBaselineMessageCount = buf.loadedFromDisk
        ? (buf.messages || []).length
        : Number(meta && meta.message_count);
      if (!Number.isFinite(buf.remoteBaselineMessageCount)) buf.remoteBaselineMessageCount = null;
      buf.remoteExpectedAssistantKey = "";
      if (!preserveCommittedRevision) buf.remoteCommittedRevision = "";
      buf.remoteTerminalSeen = false;
    }
    buf.remoteTurnActive = true;
    buf.busy = true;
    if (sid === state.activeSessionId) {
      state.busy = true;
      if (!state.thinking.active) startThinking();
    }
  }
  function onSessionEvent(e, fn) {
    var sid = (e && e.payload && e.payload.session_id) || state.activeSessionId;
    if (sid) {
      var eventBuffer = getBuffer(sid);
      var eventName = String((e && e.event) || "");
      var isTurnEvent = /chat:(user_message|turn_started|delta|reasoning_start|reasoning_delta|reasoning_done|tool_start|tool_end|user_input_required|transient_error)$/.test(eventName);
      if (eventBuffer && !eventBuffer.localTurnOwned && (eventBuffer.busy || isTurnEvent)) {
        markRemoteTurn(sid, eventBuffer);
      }
    }
    var isBg = sid && sid !== state.activeSessionId;
    runSyncOnSession(sid, fn);
    if (isBg) notify();
  }
  function isScheduledRunSession(sid) {
    return !!sid && (
      sid.indexOf("sched-") === 0 ||
      !!scheduledRunSessionOwners[sid] ||
      !!(sessionStates[sid] && sessionStates[sid].scheduledRunSession) ||
      !!(state.scheduledRunContext && state.scheduledRunContext.sessionId === sid)
    );
  }

  // Transcript persistence is authoritative in Rust. The UI only persists the
  // presentation-side artifact index and derives the optional auto-title.
  async function persistMessagesFor(sid) {
    if (!sid) return;
    if (isScheduledRunSession(sid)) return;
    // 代码会话（品悟原生/ACP）不在 list_sessions 里：它不是桥接聊天会话——
    // 消息由后端 persist_chat_engine_state 持久化、标题由后端自动命名管理。
    // 跳过产物索引与自动重命名：meta 缺失时 msgs 会错读 active 聊天 state 的
    // 首条用户消息，把别的会话文本命名到代码会话上。正常聊天会话经
    // ensureSession 创建后即 refreshHistoryList 入列，!meta 只会命中非桥接会话。
    var meta = state.sessions.find(function (s) { return s.id === sid; });
    if (!meta) return;
    var buf = sid === state.activeSessionId ? null : sessionStates[sid];
    var msgs = buf ? buf.messages : state.messages;
    var arts = filterSessionArtifacts(buf ? buf.artifacts : state.artifacts, sid);
    if (buf) buf.artifacts = arts;
    else state.artifacts = arts;
    try {
      try { await invoke("save_session_artifacts", { id: sid, paths: arts.map(function (a) { return a.path; }) }); } catch (_) {}
      if (isDefaultChatTitle(meta.title) || personaPlaceholderTitles[sid]) {
        var firstUser = msgs.find(function (m) { return m.role === "user"; });
        var text = firstUser && firstUser.content && firstUser.content.find(function (c) { return c.type === "text"; });
        if (text && text.text) {
          var newTitle = text.text.slice(0, 20);
          await invoke("rename_session", { id: sid, title: newTitle });
          meta.title = newTitle;
          delete personaPlaceholderTitles[sid]; // 已被对话内容命名,卸下占位标记
        }
      }
    } catch (e) { console.warn("persist failed", e); }
  }

  function planCardHydrationKey(item) {
    if (!item || item.type !== "plan_card") return "";
    if (item.planMarkdown) return "markdown:" + String(item.planMarkdown);
    try {
      return "snapshot:" + JSON.stringify({ plan: item.plan || null, todos: item.todos || null });
    } catch (_) {
      return "";
    }
  }

  async function reconcileRemoteTurn(sid) {
    if (!sid) return true;
    var buf = sessionStates[sid];
    if (!buf || (!buf.remoteTurnActive && !buf.remoteTerminalSeen)) return true;
    if (!buf.remoteTerminalSeen && isBusyFor(sid)) return false;
    if (authoritativeTranscriptSyncs[sid]) return authoritativeTranscriptSyncs[sid];
    var expectedAssistantKey = buf.remoteTerminalSeen
      ? String(buf.remoteExpectedAssistantKey || "")
      : "";
    // A committed transcript revision is the backend's canonical identity for
    // this turn. Prefer it over presentation-derived message equality: native
    // tools may normalize blocks between streamed events and durable storage
    // without changing the committed conversation.
    var expectedCommittedRevision = buf.remoteTerminalSeen
      ? String(buf.remoteCommittedRevision || "")
      : "";
    var minimumTerminalMessageCount = expectedAssistantKey && Array.isArray(buf.messages)
      ? buf.messages.length
      : 0;
    var sync = (async function () {
      for (var attempt = 0; attempt < 6; attempt++) {
        if (attempt) await new Promise(function (resolve) { setTimeout(resolve, 250); });
        // A reconnect/replay can deliver the commit marker just after done,
        // and a newer turn's commit can land while this retry window is open.
        // Re-read the live revision every attempt so a bumped expected value
        // converges instead of comparing a stale one (which would report a
        // false unsynced warning and block queued sends until the next event).
        if (buf.remoteTerminalSeen) {
          expectedCommittedRevision = String(buf.remoteCommittedRevision || "");
        }
        try {
          var saved = await invoke("load_session", { id: sid, setActive: false });
          if (!saved || !Array.isArray(saved.messages)) continue;
          var savedRevision = String(saved.transcript_revision || saved.transcriptRevision || "");
          // 仅当快照确实携带 revision 时才用严格相等作为权威屏障;旧后端/旧契约
          // 不含该字段时降级到消息数与 assistant 身份校验,避免「期望非空但快照
          // 无字段」导致对账必然失败(每轮误报)。
          if (expectedCommittedRevision && savedRevision) {
            if (savedRevision !== expectedCommittedRevision) continue;
          } else {
            if (minimumTerminalMessageCount && saved.messages.length < minimumTerminalMessageCount) continue;
          }
          if ((!expectedCommittedRevision || !savedRevision) && expectedAssistantKey) {
            var hasExpectedAssistant = saved.messages.some(function (message) {
              return message && message.role === "assistant" &&
                hydratedMessageKey(message, isScheduledRunSession(sid)) === expectedAssistantKey;
            });
            if (!hasExpectedAssistant) continue;
          }
          runSyncOnSession(sid, function () {
            var rawLiveChatItems = Array.isArray(state.chatItems) ? state.chatItems : [];
            var resolvedPlanTickets = Object.create(null);
            var activePlanCards = Object.create(null);
            rawLiveChatItems.forEach(function (item) {
              if (!item || item.type !== "plan_card") return;
              var key = planCardHydrationKey(item);
              if (!key) return;
              if (!item.resolved && item.cardState === "active" && item.planId) {
                if (!activePlanCards[key]) activePlanCards[key] = [];
                activePlanCards[key].push(item);
                return;
              }
              if (!item.planId) return;
              if (!resolvedPlanTickets[key]) resolvedPlanTickets[key] = [];
              resolvedPlanTickets[key].push(String(item.planId));
            });
            var liveChatItems = rawLiveChatItems.filter(function (item) {
              if (!item || item.type === "user") return false;
              if (item.type === "assistant") return item.interruptedDisplayOnly === true;
              if (item.turnErrorNotice && !item.legacyConversationOnly) return false;
              // 后台 Shell 在 chat:done 后仍继续运行；持久化 transcript 只有工具结果文本，
              // 没有 taskId/background/liveOutput，权威重载时必须保留实时卡片并按 toolId 合并。
              if (item.type === "tool") return item.background === true && item.state === "running";
              if (item.type === "plan_card") return false;
              return true;
            });
            state.messages = saved.messages;
            state.artifacts = filterSessionArtifacts(
              mergeHydratedArtifacts(saved.artifacts, state.artifacts),
              sid,
            );
            resetPendingAssistant();
            state.chatItems = [];
            rerenderFromMessages();
            for (var planIndex = state.chatItems.length - 1; planIndex >= 0; planIndex--) {
              var hydratedPlan = state.chatItems[planIndex];
              if (!hydratedPlan || hydratedPlan.type !== "plan_card") continue;
              var hydratedKey = planCardHydrationKey(hydratedPlan);
              var activeQueue = hydratedKey && activePlanCards[hydratedKey];
              if (activeQueue && activeQueue.length) {
                var liveActivePlan = activeQueue.pop();
                hydratedPlan.planId = String(liveActivePlan.planId);
                hydratedPlan.cardState = "active";
                hydratedPlan.resolved = false;
                hydratedPlan.statusLabel = liveActivePlan.statusLabel || "";
                hydratedPlan.planResolutionConfirmed = !!liveActivePlan.planResolutionConfirmed;
                continue;
              }
              var ticketQueue = hydratedKey && resolvedPlanTickets[hydratedKey];
              if (!hydratedPlan.planId && ticketQueue && ticketQueue.length) hydratedPlan.planId = ticketQueue.pop();
            }
            var unmatchedActivePlans = [];
            Object.keys(activePlanCards).forEach(function (key) {
              (activePlanCards[key] || []).forEach(function (item) { unmatchedActivePlans.push(item); });
            });
            mergeHydratedChatItems(unmatchedActivePlans, 0);
            mergeHydratedChatItems(liveChatItems, 0);
            currentStreamId = 0;
            currentStreamText = "";
            pendingAssistantText = "";
            pendingAssistantBlocks = [];
            state.busy = false;
            stopThinking();
          });
          buf.loadedFromDisk = true;
          buf.sessionRevision = String(saved.transcript_revision || saved.transcriptRevision || buf.sessionRevision || "");
          buf.localTurnOwned = false;
          buf.remoteTurnActive = false;
          buf.remoteTerminalSeen = false;
          buf.remoteBaselineMessageCount = null;
          buf.remoteBaselineTrusted = false;
          buf.remoteExpectedAssistantKey = "";
          buf.remoteCommittedRevision = "";
          buf.deferredRemoteUserEvent = null;
          buf.busy = false;
          if (sid === state.activeSessionId) saveWorkingSetTo(buf);
          notify();
          return true;
        } catch (_) {}
      }
      return false;
    })();
    authoritativeTranscriptSyncs[sid] = sync;
    try { return await sync; }
    finally { if (authoritativeTranscriptSyncs[sid] === sync) delete authoritativeTranscriptSyncs[sid]; }
  }

  // ── Pub/Sub ──────────────────────────────────────────────────────
  var subscribers = [];
  function snapshotState() {
    if (typeof structuredClone === "function") {
      try { return structuredClone(state); } catch (_) {}
    }
    return JSON.parse(JSON.stringify(state));
  }
  var STATE_SLICE_FIELDS = {
    platform: ["appVersion", "backendOnline", "platformCapabilities"],
    sessions: ["sessions", "archivedSessions", "activeSessionId", "sessionBusy", "draftEpoch"],
    chat: ["activeSkill", "artifacts", "artifactChange", "attachmentDragActive", "attachments", "busy", "chatItems", "composerDraft", "composerPrefill", "messages", "modeState", "planSnapshot", "queued", "thinking", "tokens", "turnDirtyArtifacts", "turnPresentedArtifacts", "turnTimeline"],
    voice: ["voiceInput", "voiceAsrSetup"],
    knowledge: ["kbModelSetup", "mountedCollection", "mountedCollections", "mountedCollectionsRevision"],
    scheduled: ["scheduledRunContext", "scheduledTaskAutoOpenId", "scheduledTaskBusyAction", "scheduledTaskCreationSessionId", "scheduledTaskDetail", "scheduledTaskDraft", "scheduledTaskError", "scheduledTaskErrorKind", "scheduledTaskLoading", "scheduledTaskPendingGuide", "scheduledTaskRecentRuns", "scheduledTaskRuns", "scheduledTasks", "scheduledTaskSelectionGeneration", "selectedScheduledTaskId"],
    monitor: ["monitor", "monitorError"],
    settings: ["settings", "selectedPet"],
    models: ["activeModelId", "currentSessionModelId", "effectiveModelConfig", "savedModels"],
    vllm: ["vllmBootstrapDone", "vllmBootstrapError", "vllmBootstrapping", "vllmSetup", "vllmSetupAttempt", "vllmSetupDismissed", "vllmSetupPhase"],
    interaction: ["pinvouModal", "pinvouReviews", "pinvouSummoning", "superPermEnabled"],
    personas: ["activePersona", "personaEvents", "personaPool"],
    workflow: ["workflow"],
    memory: ["memory"],
    remoteControl: ["webAccess"],
    updater: ["updateCancelling", "updateCheckError", "updateChecking", "updateDownloading", "updateError", "updateInfo", "updateProgress", "updateReady"],
    dependencies: ["deps", "depsChecking", "depsInstallError", "depsInstallProgress", "depsInstalling"],
  };
  function snapshotStateSlice(domain) {
    var fields = STATE_SLICE_FIELDS[domain];
    if (!fields) throw new Error("Unknown Tauri bridge state slice: " + domain);
    var slice = {};
    for (var i = 0; i < fields.length; i++) slice[fields[i]] = state[fields[i]];
    if (typeof structuredClone === "function") {
      try { return structuredClone(slice); } catch (_) {}
    }
    return JSON.parse(JSON.stringify(slice));
  }
  function snapshotStateSlices(domains) {
    if (!Array.isArray(domains) || domains.length === 0) {
      throw new Error("Tauri bridge state.getMany requires at least one domain");
    }
    var result = {};
    for (var i = 0; i < domains.length; i++) Object.assign(result, snapshotStateSlice(domains[i]));
    return result;
  }
  function cloneJson(value, fallback) {
    try { return JSON.parse(JSON.stringify(value == null ? fallback : value)); }
    catch (_) { return fallback; }
  }
  function remoteWorkingSetFor(sid) {
    if (!sid) return null;
    if (sid === state.activeSessionId) {
      saveWorkingSetTo(getBuffer(sid));
      return {
        messages: state.messages, chatItems: state.chatItems, artifacts: state.artifacts,
        turnTimeline: state.turnTimeline,
        busy: state.busy, thinking: state.thinking, planSnapshot: state.planSnapshot,
      };
    }
    return sessionStates[sid] || null;
  }
  function remoteArtifactSnapshot(artifacts) {
    return (Array.isArray(artifacts) ? artifacts : []).map(function (a) {
      var p = a && (a.path || a.path_tail || a.storage_path || "");
      return {
        id: (a && a.id) || "",
        basename: (a && a.basename) || basename(p),
        path: p,
        path_tail: p,
        kind: (a && a.kind) || "",
        byte_size: (a && a.byte_size) || 0,
        created_at: (a && a.created_at) || "",
      };
    }).filter(function (a) { return !!(a.path || a.basename); });
  }
  function buildRemoteLiveSnapshot(sid) {
    var ws = remoteWorkingSetFor(sid);
    if (!ws) return null;
    var meta = state.sessions.find(function (s) { return s.id === sid; }) || {};
    var msgs = cloneJson(ws.messages, []);
    var chatItems = cloneJson(ws.chatItems, []);
    return {
      snapshot_source: "live",
      session: {
        id: sid,
        title: meta.title || bt("newChatFallbackTitle"),
        status: ws.busy ? "running" : "idle",
        updated_at: meta.updated_at || "",
        message_count: meta.message_count || msgs.length || chatItems.length || 0,
      },
      messages: msgs.map(function (m, idx) {
        var blocks = Array.isArray(m.content) ? m.content : [];
        return {
          index: idx,
          role: m.role,
          content: blocks.filter(function (b) { return b && b.type === "text"; }).map(function (b) { return b.text || ""; }).join(""),
          blocks: blocks,
        };
      }),
      chat_items: chatItems,
      artifacts: remoteArtifactSnapshot(filterSessionArtifacts(ws.artifacts, sid)),
      busy: !!ws.busy,
      thinking: cloneJson(ws.thinking, null),
      plan_snapshot: cloneJson(ws.planSnapshot, null),
    };
  }
  async function publishRemoteLiveSnapshot(sid) {
    try { await ensureSessionBufferLoaded(sid); }
    catch (err) { console.warn("remote snapshot hydrate failed", err); }
    var snapshot = buildRemoteLiveSnapshot(sid);
    if (!snapshot) return false;
    await invoke("remote_control_publish_event", {
      sessionId: sid,
      kind: "session_snapshot",
      payload: snapshot,
    });
    return true;
  }
  function notify() {
    if (suppressNotify) return;
    // 会话列表「工作中」指示:active 取活动工作集 state.busy,其余取各自 buffer.busy
    state.sessionBusy = {};
    for (var id in sessionStates) state.sessionBusy[id] = !!sessionStates[id].busy;
    if (state.activeSessionId) state.sessionBusy[state.activeSessionId] = !!state.busy;
    var snapshot = snapshotState();
    for (var i = 0; i < subscribers.length; i++) subscribers[i](snapshot);
  }
  function subscribe(fn) {
    subscribers.push(fn);
    return function () {
      subscribers = subscribers.filter(function (f) { return f !== fn; });
    };
  }
  function subscribeStateSlice(domain, fn) {
    return subscribe(function () { fn(snapshotStateSlice(domain)); });
  }
  function subscribeStateSlices(domains, fn) {
    snapshotStateSlices(domains);
    return subscribe(function () { fn(snapshotStateSlices(domains)); });
  }

  var scheduledFeature = installBridgeFeature("scheduled", { state: state, notify: notify, invoke: invoke, bt: bt, runSyncOnSession: runSyncOnSession, addSystemItem: addSystemItem, rememberScheduledRunOwner: rememberScheduledRunOwner, isScheduledRunTerminal: isScheduledRunTerminal, purgeSessionBuffer: purgeSessionBuffer, createNewSession: createNewSession, prefillComposer: prefillComposer, sessionStates: sessionStates });
  var loadScheduledTaskTemplateSources = scheduledFeature.loadScheduledTaskTemplateSources;
  var persistScheduledTaskTemplateSources = scheduledFeature.persistScheduledTaskTemplateSources;
  var rememberScheduledTaskTemplateSource = scheduledFeature.rememberScheduledTaskTemplateSource;
  var forgetScheduledTaskTemplateSource = scheduledFeature.forgetScheduledTaskTemplateSource;
  var attachScheduledTaskTemplateSource = scheduledFeature.attachScheduledTaskTemplateSource;
  var attachAndPruneScheduledTaskTemplateSources = scheduledFeature.attachAndPruneScheduledTaskTemplateSources;
  var upsertScheduledTask = scheduledFeature.upsertScheduledTask;
  var applyScheduledRunViewed = scheduledFeature.applyScheduledRunViewed;
  var invalidateScheduledTaskReads = scheduledFeature.invalidateScheduledTaskReads;
  var invalidateScheduledRecentRuns = scheduledFeature.invalidateScheduledRecentRuns;
  var invalidateScheduledRecentRunsForSession = scheduledFeature.invalidateScheduledRecentRunsForSession;
  var scheduleScheduledRunRefresh = scheduledFeature.scheduleScheduledRunRefresh;
  var scheduledTaskErrorText = scheduledFeature.scheduledTaskErrorText;
  var setScheduledTaskError = scheduledFeature.setScheduledTaskError;
  var dismissScheduledTaskError = scheduledFeature.dismissScheduledTaskError;
  var clearScheduledTaskLoadError = scheduledFeature.clearScheduledTaskLoadError;
  var beginScheduledTaskLoad = scheduledFeature.beginScheduledTaskLoad;
  var endScheduledTaskLoad = scheduledFeature.endScheduledTaskLoad;
  var scheduledTaskRequestStamp = scheduledFeature.scheduledTaskRequestStamp;
  var isCurrentScheduledTaskRequest = scheduledFeature.isCurrentScheduledTaskRequest;
  var selectScheduledTask = scheduledFeature.selectScheduledTask;
  var clearScheduledTaskSelection = scheduledFeature.clearScheduledTaskSelection;
  var extractBalancedJsonObject = scheduledFeature.extractBalancedJsonObject;
  var parseLooseJsonObject = scheduledFeature.parseLooseJsonObject;
  var normalizeScheduledTaskDraft = scheduledFeature.normalizeScheduledTaskDraft;
  var activeScheduledTaskModelConfig = scheduledFeature.activeScheduledTaskModelConfig;
  var activeScheduledTaskModel = scheduledFeature.activeScheduledTaskModel;
  var lockScheduledTaskDraftModel = scheduledFeature.lockScheduledTaskDraftModel;
  var parseScheduledTaskDraftFromText = scheduledFeature.parseScheduledTaskDraftFromText;
  var clearScheduledTaskDraft = scheduledFeature.clearScheduledTaskDraft;
  var confirmScheduledTaskDraft = scheduledFeature.confirmScheduledTaskDraft;
  var scheduledTaskInputFromDraft = scheduledFeature.scheduledTaskInputFromDraft;
  var autoCreateScheduledTaskDraft = scheduledFeature.autoCreateScheduledTaskDraft;
  var loadScheduledTasks = scheduledFeature.loadScheduledTasks;
  var readScheduledTask = scheduledFeature.readScheduledTask;
  var mergeScheduledTaskRecentRuns = scheduledFeature.mergeScheduledTaskRecentRuns;
  var loadScheduledTaskRuns = scheduledFeature.loadScheduledTaskRuns;
  var loadScheduledTaskRecentRuns = scheduledFeature.loadScheduledTaskRecentRuns;
  var refreshScheduledTaskData = scheduledFeature.refreshScheduledTaskData;
  var refreshScheduledRunShortcutUntilLinked = scheduledFeature.refreshScheduledRunShortcutUntilLinked;
  var upsertScheduledTaskRun = scheduledFeature.upsertScheduledTaskRun;
  var runScheduledTaskAction = scheduledFeature.runScheduledTaskAction;
  var scheduledTaskBackendInput = scheduledFeature.scheduledTaskBackendInput;
  var createScheduledTask = scheduledFeature.createScheduledTask;
  var updateScheduledTask = scheduledFeature.updateScheduledTask;
  var pauseScheduledTask = scheduledFeature.pauseScheduledTask;
  var resumeScheduledTask = scheduledFeature.resumeScheduledTask;
  var toggleScheduledTaskPinned = scheduledFeature.toggleScheduledTaskPinned;
  var deleteScheduledTask = scheduledFeature.deleteScheduledTask;
  var runScheduledTaskNow = scheduledFeature.runScheduledTaskNow;
  var startScheduledTaskChat = scheduledFeature.startScheduledTaskChat;
  // ── Session management ───────────────────────────────────────────
  var PLAN_TOOLS = ["update_plan", "checklist_write", "todo_write"];

  // tool_result.content 可能是 string 或 Anthropic content blocks 数组，归一成纯文本。
  function toolResultText(content) {
    if (typeof content === "string") return content;
    if (Array.isArray(content)) {
      return content.map(function (b) { return b && typeof b.text === "string" ? b.text : ""; }).join("");
    }
    return "";
  }

  // plan 类工具结果格式："...updated:\n{json}"——切第一个换行后 parse（与 engine.rs 一致）。
  function parsePlanSnapshot(content) {
    var txt = toolResultText(content);
    var i = txt.indexOf("\n");
    if (i < 0) return null;
    try { return JSON.parse(txt.slice(i + 1)); } catch (_) { return null; }
  }

  // request_user_input 结果是纯 JSON {answers:[{id,label,value}]}（turn_loop.rs ToolResult::json）。
  // 按 question.id 匹配，还原成 UserInputCard 的 answers 数组（顺序对齐 questions）。
  function parseUserAnswers(content, questions) {
    var ans;
    try { ans = JSON.parse(toolResultText(content)).answers; } catch (_) { return null; }
    if (!Array.isArray(ans)) return null;
    var byId = {};
    ans.forEach(function (a) { if (a && a.id != null) byId[a.id] = a; });
    return questions.map(function (q) {
      var a = byId[q.id];
      return a ? { id: q.id, label: a.label, value: a.value } : null;
    });
  }

  // careful hook 拦截结果(shell.rs BLOCKED 固定格式)→ 反解出 careful_blocked 卡所需 metadata。
  // metadata 不进持久化 messages,session 重载只能从 tool_result 文本识别,否则 🛑 红卡重启即丢。
  function parseCarefulBlocked(text) {
    if (typeof text !== "string" || text.indexOf("BLOCKED: This command was blocked for safety reasons") !== 0) return null;
    var rm = text.match(/Reasons: ([^\n]*)/);
    var sm = text.match(/Suggestions: ([^\n]*)/);
    return {
      safety_level: "dangerous", blocked: true,
      reasons: rm && rm[1] ? rm[1].split("; ") : [],
      suggestions: sm && sm[1] ? sm[1].split("; ") : [],
    };
  }

  function userMessageInputProvenance(blocks) {
    var textBlocks = Array.isArray(blocks) ? blocks : [];
    for (var i = 0; i < textBlocks.length; i++) {
      var block = textBlocks[i];
      if (!block || block.type !== "text") continue;
      var text = String(block.text || "").trim();
      if (text.indexOf("<turn_meta>") !== 0) continue;
      var match = text.match(/(?:^|\n)Input provenance:\s*([^\r\n<]+)/);
      if (match && match[1]) return match[1].trim();
    }
    return "";
  }

  function isInternalUserMessageProvenance(provenance) {
    return provenance === "runtime" || provenance === "subagent_handoff";
  }

  // Engine 的运行时恢复提示为了兼容模型协议会以 role=user 持久化，但它不是用户输入。
  // 子智能体完成交接同理：结果必须留在父模型上下文，但不能冒充用户消息上屏。
  // 原始 blocks 必须保留给模型续聊；展示层只隐藏该内部消息，避免伪装成用户气泡/新 Turn。
  // 定时会话还会过滤送模 envelope，只投影真实任务正文。
  function userMessageDisplayText(blocks, hideInternalEnvelope) {
    var textParts = (Array.isArray(blocks) ? blocks : [])
      .filter(function (block) { return block && block.type === "text"; })
      .map(function (block) { return String(block.text || ""); });
    if (isInternalUserMessageProvenance(userMessageInputProvenance(blocks))) return "";
    if (!hideInternalEnvelope) return textParts.join("");

    return textParts.filter(function (text) {
      var trimmed = text.trim();
      return !(
        (trimmed.indexOf("<turn_meta>") === 0 && trimmed.endsWith("</turn_meta>")) ||
        trimmed === "<turn_meta_unchanged />"
      );
    }).map(function (text) {
      return text.replace(/^\s*<system-reminder>[\s\S]*?<\/system-reminder>\s*/, "");
    }).join("");
  }

  // ── Rerender from messages (session restore) ─────────────────────
  function rerenderFromMessages() {
    state.chatItems = [];
    itemIdSeq = 0;
    // 卡牌事件按 pos 插回原位(pos=事件发生时的 messages 数)。让重载历史不割裂。
    var pe = Array.isArray(state.personaEvents) ? state.personaEvents : [];
    function emitPersonaAt(atOrAfter, isTail) {
      for (var k = 0; k < pe.length; k++) {
        var ev = pe[k];
        if (isTail ? (ev.pos < atOrAfter) : (ev.pos !== atOrAfter)) continue;
        if (ev.kind === "equip" && ev.card) addChatItem({ type: "persona_equip", card: ev.card, time: "" });
        else if (ev.kind === "unequip") addChatItem({ type: "system", text: bt("personaUnequipped") + (ev.name || ""), time: "" });
        else if (ev.kind === "card_creator_intro") addChatItem({ type: "card_creator_intro", time: "" });
      }
    }
    // 预扫 tool_result：tool_use 在 assistant 消息、result 在后续 user 消息，需提前建映射
    // 才能在还原选择卡/方案卡时拿到结果（选项/快照）。
    var resultById = {};
    for (var ri = 0; ri < state.messages.length; ri++) {
      var rc = state.messages[ri].content;
      if (!Array.isArray(rc)) continue;
      for (var rj = 0; rj < rc.length; rj++) {
        if (rc[rj].type === "tool_result") {
          resultById[rc[rj].tool_use_id] = { content: rc[rj].content, is_error: !!rc[rj].is_error };
        }
      }
    }
    // 预扫:每个产物最后一次被 write/append/edit 改的 tool_use id → rerender 只在最后一次
    // 续一张成品卡(与实时 chat:done 的一张对齐,不刷一堆)。
    var lastDirtyArtifactId = {};
    var writtenArtifacts = {}; // write/append 写过的 path=产物;没 present 时兜底补首卡
    var presentedArtifacts = {}; // 整篇 present_artifact 过的 path → 别再兜底补首卡(present 会出卡,否则重复)
    var presentedArtifactNames = {}; // path 可能一边相对一边绝对,basename 去重防重复卡
    for (var di = 0; di < state.messages.length; di++) {
      var dc = state.messages[di].content;
      if (!Array.isArray(dc)) continue;
      for (var dj = 0; dj < dc.length; dj++) {
        var db = dc[dj];
        var dbMutation = db.type === "tool_use" && fileMutationAction(db.name, db.input);
        if (dbMutation) {
          var dap = extractArtifactPath(db.input);
          if (dap) {
            lastDirtyArtifactId[dap] = db.id;
            // 与实时 tool_end 同一门控:tmp/ 中间文件、非成品扩展名不记账,
            // 否则实时不进面板的文件切 session 重放后反而兜底冒出成品卡。
            if (dbMutation !== "edit" && isDeliverable(dap)) writtenArtifacts[dap] = true;
          }
        } else if (db.type === "tool_use" && isPresentArtifactTool(db.name)) {
          var pap = extractArtifactPath(db.input);
          var pres = resultById[db.id];
          var pp = presentArtifactAbsPath(pres && pres.content, pap);
          if (pp) {
            presentedArtifacts[pp] = true;
            presentedArtifactNames[basename(pp)] = true;
          }
        } else if (db.type === "tool_use" && shouldUseToolOutputAsArtifact(db.name)) {
          var gres = resultById[db.id];
          if (!(gres && gres.is_error)) {
            var gp = artifactPathFromToolOutput(gres && gres.content);
            if (gp && isDeliverable(gp)) {
              lastDirtyArtifactId[gp] = db.id;
              writtenArtifacts[gp] = true;
            }
          }
        }
      }
    }
    for (var mi = 0; mi < state.messages.length; mi++) {
      emitPersonaAt(mi, false); // 该消息之前发生的卡牌事件先插
      var m = state.messages[mi];
      var blocks = Array.isArray(m.content) ? m.content : [];
      if (m.role === "user") {
        var utext = userMessageDisplayText(blocks, isScheduledRunSession(state.activeSessionId));
        if (utext) {
          // pinvouTransfer 是展示层标记、不在 messages → rerender 从转交固定措辞还原品/悟样式
          var uitem2 = { type: "user", text: utext, time: "", messageIndex: mi };
          var scene = pinvouSceneForMessagePos(mi);
          if (scene) uitem2.pinvouScene = scene;
          if (utext.indexOf("以下维度产物还缺") >= 0) uitem2.pinvouTransfer = "悟";
          else if (utext.indexOf("请按下面的检阅意见") >= 0 || utext.indexOf("以下事项我已拍板") >= 0 || utext.indexOf("request_user_input 正式问我") >= 0) uitem2.pinvouTransfer = "品";
          addChatItem(uitem2);
        }
        // tool_result（只回填普通工具卡；选择卡/方案卡的结果已在 tool_use 处还原）
        for (var ci = 0; ci < blocks.length; ci++) {
          var c = blocks[ci];
          if (c.type !== "tool_result") continue;
          var tm = toolMeta[c.tool_use_id];
          if (tm) {
            // careful hook 拦截 → 还原 🛑 红卡(实时由 tool_end metadata 插,重载从文本反解)
            var blockedMd = parseCarefulBlocked(toolResultText(c.content));
            if (blockedMd) {
              updateToolItem(c.tool_use_id, c.content, false); // 被拦=失败态,与实时一致
              addChatItem({ type: "careful_blocked", args: tm.args, metadata: blockedMd, time: "" });
            } else {
              // load_skill 同样脱敏：重载历史时也不还原 SKILL.md 全文，展开只见占位。
              var contentForCard = (tm.name === "load_skill") ? bt("skillContentHidden") : c.content;
              updateToolItem(c.tool_use_id, contentForCard, !c.is_error);
            }
          }
        }
        continue;
      }
      if (m.role !== "assistant") continue;
      var textBuf = "";
      var planSnap = null, todosSnap = null, sawPlanTool = false;
      for (var bi = 0; bi < blocks.length; bi++) {
        var b = blocks[bi];
        if (b.type === "text") {
          textBuf += b.text;
        } else if (b.type === "thinking") {
          if (textBuf) {
            addChatItem({ type: "assistant", text: textBuf, html: renderMarkdown(textBuf), time: "", streaming: false });
            textBuf = "";
          }
          var reasoningText = String(b.thinking || b.text || "");
          if (reasoningText) {
            addChatItem({
              type: "reasoning", text: reasoningText, time: "", streaming: false,
              startedAt: null, completedAt: null,
            });
          }
        } else if (b.type === "tool_use") {
          if (textBuf) {
            addChatItem({ type: "assistant", text: textBuf, html: renderMarkdown(textBuf), time: "", streaming: false });
            textBuf = "";
          }
          toolMeta[b.id] = { name: b.name, args: b.input };
          // request_user_input → 还原只读选择卡（问题来自 input，选项高亮来自 result）
          if (b.name === "request_user_input") {
            var qs = (b.input && b.input.questions) || [];
            if (Array.isArray(qs) && qs.length) {
              var res = resultById[b.id];
              addChatItem({
                type: "user_input", toolCallId: b.id, questions: qs,
                resolved: true, cardState: (res && res.is_error) ? "cancelled" : "submitted",
                restoredAnswers: res ? parseUserAnswers(res.content, qs) : null, time: "",
              });
            }
            continue;
          }
          // present_artifact → 还原成品卡(切会话不丢)。仅当工具成功时还原:
          // 失败的调用回退成普通工具卡(下方 default addChatItem)。
          if (isPresentArtifactTool(b.name)) {
            var pares = resultById[b.id];
            if (!(pares && pares.is_error)) {
              var rpp = presentArtifactAbsPath(pares && pares.content, b.input && b.input.path);
              if (!isDuplicateArtifactCard(rpp)) {
                addChatItem({
                  type: "artifact_card",
                  path: rpp,
                  title: (b.input && b.input.title) || "",
                  description: (b.input && b.input.description) || "",
                  time: "",
                  sessionId: state.activeSessionId,
                });
              }
              continue;
            }
          }
          // update_plan / checklist_write / todo_write → 收集快照，本条消息末尾还原方案卡
          if (PLAN_TOOLS.indexOf(b.name) >= 0) {
            var snap = parsePlanSnapshot(resultById[b.id] && resultById[b.id].content);
            if (snap) {
              if (b.name === "update_plan") planSnap = snap; else todosSnap = snap;
            }
            sawPlanTool = true;
            continue;
          }
          addChatItem({ type: "tool", toolId: b.id, name: b.name, args: b.input, output: null, success: null, state: "pending" });
          if (shouldUseToolOutputAsArtifact(b.name)) {
            var gres2 = resultById[b.id];
            var gap = artifactPathFromToolOutput(gres2 && gres2.content);
            if (!(gres2 && gres2.is_error) && gap && isDeliverable(gap) && lastDirtyArtifactId[gap] === b.id && !presentedArtifacts[gap] && !presentedArtifactNames[basename(gap)]) {
              var gprev = findPresentedArtifact(gap);
              if (gprev) {
                addChatItem({
                  type: "artifact_card", path: gprev.path, title: gprev.title,
                  description: gprev.description, time: "", sessionId: state.activeSessionId,
                });
              } else if (writtenArtifacts[gap]) {
                addChatItem({ type: "artifact_card", path: gap, title: basename(gap), description: "", time: "", sessionId: state.activeSessionId });
              }
            }
          }
          // 还原"自动续卡":File.write/File.edit 改的文件之前 present 过 → 续一张
          // 成品卡(与实时 tool_end 的自动续逻辑对齐,切会话不丢)。present 的卡按
          // 顺序在前(必须先 present 才进集合),此处 findPresentedArtifact 能命中。
          if (fileMutationAction(b.name, b.input)) {
            var wres = resultById[b.id];
            var wap = extractArtifactPath(b.input);
            // 去重:同产物只在最后一次修改处补一张卡(与实时对齐)。
            if (!(wres && wres.is_error) && wap && lastDirtyArtifactId[wap] === b.id) {
              var wprev = findPresentedArtifact(wap);
              if (wprev) {
                addChatItem({
                  type: "artifact_card", path: wprev.path, title: wprev.title,
                  description: wprev.description, time: "", sessionId: state.activeSessionId,
                });
              } else if (writtenArtifacts[wap] && !presentedArtifacts[wap] && !presentedArtifactNames[basename(wap)]) {
                // AI 写了产物但全程没 present_artifact → 兜底补首卡(与实时 chat:done 对齐)
                addChatItem({ type: "artifact_card", path: wap, title: basename(wap), description: "", time: "", sessionId: state.activeSessionId });
              }
            }
          }
        }
      }
      if (textBuf) {
        addChatItem({ type: "assistant", text: textBuf, html: renderMarkdown(textBuf), time: "", streaming: false });
      }
      // 本条 assistant 消息用过 plan 工具 → 还原一张只读历史方案卡
      if (sawPlanTool && (planSnap || todosSnap)) {
        var snaps = { plan: planSnap, todos: todosSnap };
        addChatItem({
          type: "plan_card", plan: planSnap, todos: todosSnap,
          planMarkdown: composePlanMarkdown(snaps),
          cardState: "frozen", resolved: true, statusLabel: bt("planHistorical"), time: "",
        });
      }
    }
    emitPersonaAt(state.messages.length, true); // 最后一条消息之后发生的卡牌事件(末尾加持/卸下)
  }

  var terminalFeature = installBridgeFeature("terminal", { state: state, notify: notify, invoke: invoke, bt: bt, runSyncOnSession: runSyncOnSession, addChatItem: addChatItem });
  var updateToolItem = terminalFeature.updateToolItem;
  var isShellExecutionTool = terminalFeature.isShellExecutionTool;
  var utf8Length = terminalFeature.utf8Length;
  var normalizeTerminalTail = terminalFeature.normalizeTerminalTail;
  var formatShellSnapshot = terminalFeature.formatShellSnapshot;
  var shellCommandForItem = terminalFeature.shellCommandForItem;
  var shellSnapshotKey = terminalFeature.shellSnapshotKey;
  var terminalShellHistoryMatch = terminalFeature.terminalShellHistoryMatch;
  var applyShellSnapshots = terminalFeature.applyShellSnapshots;
  var scheduleShellPoll = terminalFeature.scheduleShellPoll;
  var runShellPoll = terminalFeature.runShellPoll;
  var scheduleShellNotify = terminalFeature.scheduleShellNotify;
  var markBackgroundToolItem = terminalFeature.markBackgroundToolItem;
  var finishBackgroundToolItem = terminalFeature.finishBackgroundToolItem;
  var rememberPendingTerminalSequence = terminalFeature.rememberPendingTerminalSequence;
  var stripTerminalSequences = terminalFeature.stripTerminalSequences;
  var terminalParserState = terminalFeature.terminalParserState;
  var mergeTerminalChunk = terminalFeature.mergeTerminalChunk;
  var mergeTerminalTail = terminalFeature.mergeTerminalTail;
  var normalizeTerminalTail = terminalFeature.normalizeTerminalTail;
  var reconcileBackgroundTerminalOutput = terminalFeature.reconcileBackgroundTerminalOutput;
  var appendToolItemOutput = terminalFeature.appendToolItemOutput;
  // 找最后一条匹配的 chat item（用于卡片状态机更新）
  function patchLastItem(pred, patch) {
    for (var i = state.chatItems.length - 1; i >= 0; i--) {
      if (pred(state.chatItems[i])) {
        Object.assign(state.chatItems[i], patch);
        return state.chatItems[i];
      }
    }
    return null;
  }
  // 是否已存在未处理（未 resolved）的某类型卡片 —— 防重复插入
  function hasUnresolvedItem(type) {
    return state.chatItems.some(function (it) { return it.type === type && !it.resolved; });
  }

  // ── Plan markdown 拼接（accept 时发给后端，与 main.js 对齐）────────
  function composePlanMarkdown(snapshots) {
    var lines = [];
    var plan = snapshots && snapshots.plan;
    var todos = snapshots && snapshots.todos;
    function sym(s) { return s === "completed" ? "●" : s === "in_progress" ? "◎" : "○"; }
    if (plan && Array.isArray(plan.items)) {
      if (plan.explanation) { lines.push("**方案：**", plan.explanation, ""); }
      lines.push("**步骤：**");
      plan.items.forEach(function (item, i) { lines.push((i + 1) + ". " + sym(item.status) + " " + item.step); });
      lines.push("");
    }
    if (todos && Array.isArray(todos.items)) {
      lines.push("**细分待办：**");
      todos.items.forEach(function (item, i) { lines.push((i + 1) + ". " + sym(item.status) + " " + item.content); });
    }
    return lines.length > 0 ? lines.join("\n") : "（plan 为空）";
  }

  async function cancelShellTask(sessionId, taskId) {
    if (!sessionId || !taskId) throw new Error("Missing shell task identity");
    return invoke("cancel_shell_task", { sessionId: sessionId, taskId: taskId });
  }

  installBridgeFeature("chat-events", {
    state: state, listen: listen, invoke: invoke, turnUsageDirty: turnUsageDirty,
    sessionStates: sessionStates, renderMarkdown: renderMarkdown, bt: bt,
    notify: notify, onSessionEvent: onSessionEvent, runSyncOnSession: runSyncOnSession,
    addChatItem: addChatItem, addSystemItem: addSystemItem, addAuthoritySyncNotice: addAuthoritySyncNotice, timeStr: timeStr,
    toolCallAlreadyStarted: toolCallAlreadyStarted,
    toolCallAlreadyFinished: toolCallAlreadyFinished,
    hasChatItemForTool: hasChatItemForTool,
    flushPendingTextBlock: flushPendingTextBlock,
    flushAssistantMessageToHistory: flushAssistantMessageToHistory,
    resetPendingAssistant: resetPendingAssistant, flushQueued: flushQueued,
    isBusyFor: isBusyFor, doSendFor: doSendFor,
    ensureSessionBufferLoaded: ensureSessionBufferLoaded,
    getBuffer: getBuffer, markRemoteTurn: markRemoteTurn,
    reconcileRemoteTurn: reconcileRemoteTurn, saveWorkingSetTo: saveWorkingSetTo,
    hydratedMessageKey: hydratedMessageKey,
    thinkingTool: function () { return thinkingTool.apply(null, arguments); },
    thinkingIdle: function () { return thinkingIdle.apply(null, arguments); },
    startThinking: function () { return startThinking.apply(null, arguments); },
    stopThinking: function () { return stopThinking.apply(null, arguments); },
    userMessageDisplayText: userMessageDisplayText,
    scheduleScheduledRunRefresh: scheduleScheduledRunRefresh,
    handleMemoryWrite: function () { return handleMemoryWrite.apply(null, arguments); },
    isPresentArtifactTool: isPresentArtifactTool,
    artifactPathFromToolOutput: artifactPathFromToolOutput,
    shouldUseToolOutputAsArtifact: shouldUseToolOutputAsArtifact,
    presentArtifactAbsPath: presentArtifactAbsPath,
    extractArtifactPaths: extractArtifactPaths, extractArtifactPath: extractArtifactPath, fileMutationAction: fileMutationAction,
    markTurnDirtyArtifact: markTurnDirtyArtifact,
    trackArtifact: trackArtifact, untrackArtifact: untrackArtifact,
    findPresentedArtifact: findPresentedArtifact, isDeliverable: isDeliverable,
    noteArtifactChange: noteArtifactChange,
    publishRemoteLiveSnapshot: publishRemoteLiveSnapshot,
    persistMessagesFor: persistMessagesFor,
    composePlanMarkdown: composePlanMarkdown,
    refreshHistoryList: refreshHistoryList,
    isShellExecutionTool: isShellExecutionTool,
    scheduleShellPoll: scheduleShellPoll,
    appendToolItemOutput: appendToolItemOutput,
    scheduleShellNotify: scheduleShellNotify,
    markBackgroundToolItem: markBackgroundToolItem,
    patchLastItem: patchLastItem,
    isDuplicateArtifactCard: isDuplicateArtifactCard,
    updateToolItem: updateToolItem,
    basename: basename,
    hasUnresolvedItem: hasUnresolvedItem,
    finishBackgroundToolItem: finishBackgroundToolItem,
    safeConsoleInfo: safeConsoleInfo,
    isScheduledRunSession: isScheduledRunSession,
    markScheduledInitialTurnTerminal: markScheduledInitialTurnTerminal,
    isAbsPath: isAbsPath,
    addOrMergePruneCompaction: addOrMergePruneCompaction,
    get currentStreamText() { return currentStreamText; },
    set currentStreamText(value) { currentStreamText = value; },
    get currentStreamId() { return currentStreamId; },
    set currentStreamId(value) { currentStreamId = value; },
    get pendingAssistantText() { return pendingAssistantText; },
    set pendingAssistantText(value) { pendingAssistantText = value; },
    get pendingAssistantBlocks() { return pendingAssistantBlocks; },
    set pendingAssistantBlocks(value) { pendingAssistantBlocks = value; },
    get itemIdSeq() { return itemIdSeq; },
    set itemIdSeq(value) { itemIdSeq = value; },
    get toolMeta() { return toolMeta; },
    set toolMeta(value) { toolMeta = value; },
  });

  var workflowRuntimeFeature = installBridgeFeature("workflow-runtime", {
    state: state, invoke: invoke, listen: listen, notify: notify, bt: bt,
    refreshHistoryList: refreshHistoryList,
    get itemIdSeq() { return itemIdSeq; },
    set itemIdSeq(value) { itemIdSeq = value; },
  });
  var isRunSession = workflowRuntimeFeature.isRunSession;
  var applyAgentPatch = workflowRuntimeFeature.applyAgentPatch;
  var markWorkflowRunStopped = workflowRuntimeFeature.markWorkflowRunStopped;
  var markWorkflowRunBlocked = workflowRuntimeFeature.markWorkflowRunBlocked;
  var mergeFullState = workflowRuntimeFeature.mergeFullState;
  var attachRun = workflowRuntimeFeature.attachRun;
  var resumeWorkflowOnBoot = workflowRuntimeFeature.resumeWorkflowOnBoot;
  var pushRunCard = workflowRuntimeFeature.pushRunCard;
  var resolveRunCard = workflowRuntimeFeature.resolveRunCard;
  var resolveRunCardsForRole = workflowRuntimeFeature.resolveRunCardsForRole;
  var refreshRunState = workflowRuntimeFeature.refreshRunState;

  var monitorFeature = installBridgeFeature("monitor", { state: state, notify: notify, invoke: invoke, bt: bt, safeConsoleInfo: safeConsoleInfo, sessionStates: sessionStates });
  var startMonitorPolling = monitorFeature.startMonitorPolling;
  var stopMonitorPolling = monitorFeature.stopMonitorPolling;
  var clearMonitorStats = monitorFeature.clearMonitorStats;
  var pollBackendStatus = monitorFeature.pollBackendStatus;
  var settingsFeature = installBridgeFeature("settings", { state: state, notify: notify, invoke: invoke, listen: listen });
  var loadSettings = settingsFeature.loadSettings;
  var loadSelectedPet = settingsFeature.loadSelectedPet;
  var setSelectedPet = settingsFeature.setSelectedPet;
  var loadEffectiveModelConfig = settingsFeature.loadEffectiveModelConfig;
  var saveSettings = settingsFeature.saveSettings;
  var saveSettingsAndRestart = settingsFeature.saveSettingsAndRestart;
  var saveSearchSettings = settingsFeature.saveSearchSettings;
  var saveSearchSettingsAndRestart = settingsFeature.saveSearchSettingsAndRestart;
  var submitFeedback = settingsFeature.submitFeedback;
  var discoverLocalVllm = settingsFeature.discoverLocalVllm;
  var detectLocalVllmSetup = settingsFeature.detectLocalVllmSetup;
  var bootstrapLocalVllm = settingsFeature.bootstrapLocalVllm;
  var dismissVllmSetup = settingsFeature.dismissVllmSetup;
  var declineVllmSetup = settingsFeature.declineVllmSetup;
  var getEffectiveModelConfig = settingsFeature.getEffectiveModelConfig;
  var getImageInputCapability = settingsFeature.getImageInputCapability;
  var loadModels = settingsFeature.loadModels;
  var saveModel = settingsFeature.saveModel;
  var revealModelApiKey = settingsFeature.revealModelApiKey;
  var deleteModel = settingsFeature.deleteModel;
  var setActiveModel = settingsFeature.setActiveModel;
  var loadSessionModel = settingsFeature.loadSessionModel;
  var switchModel = settingsFeature.switchModel;
  var testModelConnection = settingsFeature.testModelConnection;
  var testImageInputCapability = settingsFeature.testImageInputCapability;
  var testSearchProvider = settingsFeature.testSearchProvider;

  var interactionFeature = installBridgeFeature("interaction", {
    state: state, invoke: invoke, notify: notify, bt: bt,
    addSystemItem: addSystemItem, addAuthoritySyncNotice: addAuthoritySyncNotice, addChatItem: addChatItem, timeStr: timeStr,
    runSyncOnSession: runSyncOnSession,
    flushAssistantMessageToHistory: flushAssistantMessageToHistory,
    resetPendingAssistant: resetPendingAssistant,
    rerenderFromMessages: rerenderFromMessages,
    turnUsageDirty: turnUsageDirty,
    ensureSession: ensureSession,
    sendMessage: sendMessage,
    getBuffer: getBuffer,
    reconcileRemoteTurn: reconcileRemoteTurn,
    isBusyFor: isBusyFor,
    markRemoteTurn: markRemoteTurn,
    get currentStreamText() { return currentStreamText; },
    set currentStreamText(value) { currentStreamText = value; },
    get currentStreamId() { return currentStreamId; },
    set currentStreamId(value) { currentStreamId = value; },
    get itemIdSeq() { return itemIdSeq; },
    set itemIdSeq(value) { itemIdSeq = value; },
  });
  var refreshSuperPerm = interactionFeature.refreshSuperPerm;
  var toggleSuperPerm = interactionFeature.toggleSuperPerm;
  var syncModeState = interactionFeature.syncModeState;
  var patchItemById = interactionFeature.patchItemById;
  var pushUserEcho = interactionFeature.pushUserEcho;
  var markResolved = interactionFeature.markResolved;
  var runOnSession = interactionFeature.runOnSession;
  var addSystemItemFor = interactionFeature.addSystemItemFor;
  var patchItemByIdFor = interactionFeature.patchItemByIdFor;
  var startThinking = interactionFeature.startThinking;
  var thinkingTool = interactionFeature.thinkingTool;
  var thinkingIdle = interactionFeature.thinkingIdle;
  var stopThinking = interactionFeature.stopThinking;
  var applyModeFromState = interactionFeature.applyModeFromState;
  var acceptPlan = interactionFeature.acceptPlan;
  var discardPlan = interactionFeature.discardPlan;
  var exitPlanToYolo = interactionFeature.exitPlanToYolo;
  var setPlanModeNext = interactionFeature.setPlanModeNext;
  var setMultiAgentMode = interactionFeature.setMultiAgentMode;
  var planStuckReplan = interactionFeature.planStuckReplan;
  var planStuckGo = interactionFeature.planStuckGo;
  var submitUserInput = interactionFeature.submitUserInput;
  var cancelUserInput = interactionFeature.cancelUserInput;
  var editLastTurn = interactionFeature.editLastTurn;
  var compactNow = interactionFeature.compactNow;

  var memoryFeature = installBridgeFeature("memory", { state: state, notify: notify, invoke: invoke, bt: bt, addSystemItem: addSystemItem, runSyncOnSession: runSyncOnSession, patchItemById: patchItemById, runOnSession: runOnSession, addChatItem: addChatItem, timeStr: timeStr });
  var handleMemoryWrite = memoryFeature.handleMemoryWrite;
  var loadMemoryOverview = memoryFeature.loadMemoryOverview;
  var saveMemoryProfilePatch = memoryFeature.saveMemoryProfilePatch;
  var deleteMemoryPreference = memoryFeature.deleteMemoryPreference;
  var updateMemoryItem = memoryFeature.updateMemoryItem;
  var deleteMemoryItem = memoryFeature.deleteMemoryItem;
  var archiveRecentWorkMemory = memoryFeature.archiveRecentWorkMemory;
  var confirmMemoryCandidate = memoryFeature.confirmMemoryCandidate;
  var ignoreMemoryCandidate = memoryFeature.ignoreMemoryCandidate;
  var neverMemoryCandidate = memoryFeature.neverMemoryCandidate;
  var artifactsFeature = installBridgeFeature("artifacts", { state: state, notify: notify, invoke: invoke, bt: bt, addSystemItem: addSystemItem, dialogOpen: dialogOpen, basename: basename, isDeliverable: isDeliverable, isAbsPath: isAbsPath, sessionStates: sessionStates, ensureSession: function () { return ensureSession.apply(null, arguments); }, discardManagedAttachment: discardManagedAttachment });
  var artifactInfo = artifactsFeature.artifactInfo;
  var readArtifactText = artifactsFeature.readArtifactText;
  var writeArtifactText = artifactsFeature.writeArtifactText;
  var readArtifactImageB64 = artifactsFeature.readArtifactImageB64;
  var readArtifactThumbnail = artifactsFeature.readArtifactThumbnail;
  var renderArtifactVisual = artifactsFeature.renderArtifactVisual;
  var openContainingFolder = artifactsFeature.openContainingFolder;
  var revealSessionFolder = artifactsFeature.revealSessionFolder;
  var openScheduledTaskFolder = artifactsFeature.openScheduledTaskFolder;
  var openInSystem = artifactsFeature.openInSystem;
  var openArtifactExternal = artifactsFeature.openArtifactExternal;
  var downloadArtifact = artifactsFeature.downloadArtifact;
  var listDeliverables = artifactsFeature.listDeliverables;
  var listDeliverableIndex = artifactsFeature.listDeliverableIndex;
  var openExternalUrl = artifactsFeature.openExternalUrl;
  var openUserExternalUrl = artifactsFeature.openUserExternalUrl;
  var addAttachmentByPath = artifactsFeature.addAttachmentByPath;
  var addPasteImage = artifactsFeature.addPasteImage;
  var removeAttachment = artifactsFeature.removeAttachment;
  var clearAttachments = artifactsFeature.clearAttachments;
  var pickAndAttach = artifactsFeature.pickAndAttach;
  var uploadDeviceFiles = artifactsFeature.uploadDeviceFiles;
  var resolveConversationAttachment = artifactsFeature.resolveConversationAttachment;
  var openConversationAttachment = artifactsFeature.openConversationAttachment;
  var revealConversationAttachment = artifactsFeature.revealConversationAttachment;
  var personasFeature = installBridgeFeature("personas", { state: state, notify: notify, invoke: invoke, listen: listen, bt: bt, isDefaultChatTitle: isDefaultChatTitle, addSystemItem: addSystemItem, addChatItem: addChatItem, timeStr: timeStr, ensureSession: ensureSession, personaPlaceholderTitles: personaPlaceholderTitles });
  var loadPersonas = personasFeature.loadPersonas;
  var getPersonas = personasFeature.getPersonas;
  var createPersona = personasFeature.createPersona;
  var updatePersona = personasFeature.updatePersona;
  var deletePersona = personasFeature.deletePersona;
  var recordPersonaEvent = personasFeature.recordPersonaEvent;
  var equipPersona = personasFeature.equipPersona;
  var unequipPersona = personasFeature.unequipPersona;
  var syncActivePersona = personasFeature.syncActivePersona;
  var mountCollection = personasFeature.mountCollection;
  var setCollectionEnabled = personasFeature.setCollectionEnabled;
  var removeCollection = personasFeature.removeCollection;
  var unmountCollection = personasFeature.unmountCollection;
  var syncMountedCollection = personasFeature.syncMountedCollection;
  var updaterFeature = installBridgeFeature("updater", { state: state, notify: notify, invoke: invoke, refreshHistoryList: refreshHistoryList, listen: listen, publishRemoteLiveSnapshot: publishRemoteLiveSnapshot, getBuffer: getBuffer, bt: bt });
  var loadAppVersion = updaterFeature.loadAppVersion;
  var checkForUpdateSilently = updaterFeature.checkForUpdateSilently;
  var checkForUpdate = updaterFeature.checkForUpdate;
  var downloadAndInstallUpdate = updaterFeature.downloadAndInstallUpdate;
  var cancelUpdate = updaterFeature.cancelUpdate;
  var restartApp = updaterFeature.restartApp;
  var reportPendingUpdateResult = updaterFeature.reportPendingUpdateResult;
  var remoteControlFeature = installBridgeFeature("remote-control", {
    state: state,
    notify: notify,
    invoke: invoke,
    listen: listen,
    bt: bt,
  });
  // 撕离窗口不是权威主 WebView，不注册桌面 RPC 代理。
  if (!isDetachedWindow) {
    Promise.resolve(remoteControlFeature.startDesktopProxy()).catch(function (error) {
      console.error("[WebAccess] desktop proxy startup failed", error);
    });
  }
  var refreshRemoteControlStatus = remoteControlFeature.refreshRemoteControlStatus;
  var startRemoteControl = remoteControlFeature.startRemoteControl;
  var stopRemoteControl = remoteControlFeature.stopRemoteControl;
  var refreshRemoteControlQr = remoteControlFeature.refreshRemoteControlQr;
  var dependenciesFeature = installBridgeFeature("dependencies", { state: state, notify: notify, invoke: invoke, listen: listen, bt: bt });
  var checkDependencies = dependenciesFeature.checkDependencies;
  var installDependencies = dependenciesFeature.installDependencies;
  var voiceFeature = installBridgeFeature("voice", { state: state, notify: notify, invoke: invoke, bt: bt });
  var startVoiceInput = voiceFeature.startVoiceInput;
  var installVoiceAsr = voiceFeature.installVoiceAsr;
  var cancelVoiceAsrSetup = voiceFeature.cancelVoiceAsrSetup;
  var closeVoiceAsrSetup = voiceFeature.closeVoiceAsrSetup;
  var cancelVoiceInput = voiceFeature.cancelVoiceInput;
  var clearVoiceInput = voiceFeature.clearVoiceInput;
  var appendVoiceText = voiceFeature.appendVoiceText;
  var runVoiceInputDebugAssertions = voiceFeature.runVoiceInputDebugAssertions;
  var knowledgeModelFeature = installBridgeFeature("knowledge-model", { state: state, notify: notify, invoke: invoke });
  var downloadKbModel = knowledgeModelFeature.downloadKbModel;
  var cancelKbModel = knowledgeModelFeature.cancelKbModel;

  var multiAgentFeature = installBridgeFeature("multiagent", { state: state, notify: notify, invoke: invoke, listen: listen });
  var listMultiAgentSubagents = multiAgentFeature.listSubagentTranscripts;
  var readMultiAgentSubagent = multiAgentFeature.readSubagentTranscript;
  var workflowFeature = installBridgeFeature("workflow", { state: state, notify: notify, invoke: invoke, bt: bt, addSystemItem: addSystemItem, dialogOpen: dialogOpen, resetPendingAssistant: resetPendingAssistant, syncModeState: syncModeState, refreshHistoryList: refreshHistoryList, markWorkflowRunStopped: markWorkflowRunStopped, refreshRunState: refreshRunState, resolveRunCard: resolveRunCard, resolveRunCardsForRole: resolveRunCardsForRole });
  var setCurrentPhase = workflowFeature.setCurrentPhase;
  var loadSkills = workflowFeature.loadSkills;
  var activateSkill = workflowFeature.activateSkill;
  var deactivateSkill = workflowFeature.deactivateSkill;
  var openDemo = workflowFeature.openDemo;
  var closeDemo = workflowFeature.closeDemo;
  var startWorkflowTask = workflowFeature.startWorkflowTask;
  var stopWorkflowTask = workflowFeature.stopWorkflowTask;
  var listWorkflows = workflowFeature.listWorkflows;
  var selectWorkflowRole = workflowFeature.selectWorkflowRole;
  var closeWorkflowDrawer = workflowFeature.closeWorkflowDrawer;
  var resetWorkflowRun = workflowFeature.resetWorkflowRun;
  var getRolePrompt = workflowFeature.getRolePrompt;
  var getRoleOutputs = workflowFeature.getRoleOutputs;
  var getGateReport = workflowFeature.getGateReport;
  var getRoleLogs = workflowFeature.getRoleLogs;
  var submitWorkflowUserInput = workflowFeature.submitWorkflowUserInput;
  var pickAndAddMaterials = workflowFeature.pickAndAddMaterials;
  var pickFiles = workflowFeature.pickFiles;
  var pickFolder = workflowFeature.pickFolder;
  var pickFolders = workflowFeature.pickFolders;
  var pickFeedbackFiles = workflowFeature.pickFeedbackFiles;
  var addMaterialsToSession = workflowFeature.addMaterialsToSession;
  var approveWorkflowGate = workflowFeature.approveWorkflowGate;
  var rejectWorkflowGate = workflowFeature.rejectWorkflowGate;
  var retryWorkflowRole = workflowFeature.retryWorkflowRole;
  // ── Init ─────────────────────────────────────────────────────────
  async function init() {
    if (initPromise) return initPromise;
    initPromise = (async function () {
    startupMark("bridge:init_start");
    await startupAwait("bridge:load_platform_capabilities", loadPlatformCapabilities);
    // Populate the global Scheduled unread summary without requiring the user
    // to visit the Scheduled page first. This stays off the startup critical path.
    if (!isDetachedWindow) {
      loadScheduledTasks().catch(function () {}).then(function () {
        loadScheduledTaskRecentRuns().catch(function () {});
      });
    }
    startupMark("bridge:monitor_polling_deferred", "starts when monitor view becomes active");
    await startupAwait("bridge:load_settings", loadSettings);
    await startupAwait("bridge:load_selected_pet", loadSelectedPet);
    await startupAwait("bridge:load_effective_model", loadEffectiveModelConfig);
    await startupAwait("bridge:load_app_version", loadAppVersion);
    await startupAwait("bridge:load_models", loadModels);
    await startupAwait("bridge:refresh_history", refreshHistoryList);
    // 分离会话必须在同一初始化链内绑定目标 id。此前 DetachedShell 的独立 effect
    // 会与这里的 enterDraft() 并发，慢初始化时已加载的原会话会被重置成空白草稿。
    if (isDetachedWindow && detachedWindowKind === "session" && detachedWindowSessionId) {
      await startupAwait("bridge:detached_session", function () {
        return switchToSession(detachedWindowSessionId);
      });
    } else {
      enterDraft(); // 主窗口及非会话分离视图使用本窗口自己的空白工作集
      startupMark("bridge:draft_entered");
    }
    var needsSessionRuntime = !isDetachedWindow || detachedWindowKind === "session";
    if (needsSessionRuntime) {
      await startupAwait("bridge:refresh_super_permission", refreshSuperPerm);
    }
    if (!isDetachedWindow || detachedWindowKind === "session" || detachedWindowKind === "cardpool") {
      loadPersonas(); // 会话和卡池需要本窗口自己的卡牌投影，fire-and-forget
      startupMark("bridge:personas_load_started");
    }
    if (needsSessionRuntime) {
      pollBackendStatus();
      setInterval(pollBackendStatus, 10000);
    }
    if (!isDetachedWindow) {
      reportPendingUpdateResult(); // Windows OTA 升级后反馈,失败保留记录下次再试
      checkForUpdateSilently(); // fire-and-forget,不阻塞启动
    }
    startupMark("bridge:background_checks_started");
    if (!isDetachedWindow) refreshRemoteControlStatus(); // 权威主窗口独占桌面 Web 代理状态
    if (!isDetachedWindow || detachedWindowKind === "workflow") {
      await startupAwait("bridge:resume_workflow", resumeWorkflowOnBoot); // 工作流窗口需要恢复后端共享 run
    }
    notify();
    startupMark("bridge:init_done");
    if (window.__PINVOU_STARTUP__) window.__PINVOU_STARTUP__.flush();
    })();
    return initPromise;
  }

  // ── Expose API ───────────────────────────────────────────────────
  window.TauriBridge = {
    available: true,
    lifecycle: { init: init },
    state: {
      get: snapshotStateSlice,
      getMany: snapshotStateSlices,
      subscribe: subscribeStateSlice,
      subscribeMany: subscribeStateSlices,
    },
    platform: {
      refreshConnectorAuthGates: refreshConnectorAuthGates,
      loadPlatformCapabilities: loadPlatformCapabilities,
    },
    chat: {
      sendMessage: sendMessage,
      sendMessageToSession: sendMessageToSession,
      getComposerDraft: getComposerDraft,
      setComposerDraft: setComposerDraft,
      retryFirstTurn: retryFirstTurn,
      prefillComposer: prefillComposer,
      removeQueued: removeQueued,
      cancelGeneration: cancelGeneration,
      cancelShellTask: cancelShellTask,
    },
    voice: {
      startVoiceInput: startVoiceInput,
      installVoiceAsr: installVoiceAsr,
      cancelVoiceAsrSetup: cancelVoiceAsrSetup,
      closeVoiceAsrSetup: closeVoiceAsrSetup,
      cancelVoiceInput: cancelVoiceInput,
      clearVoiceInput: clearVoiceInput,
      appendVoiceText: appendVoiceText,
      runVoiceInputDebugAssertions: runVoiceInputDebugAssertions,
    },
    knowledge: {
      loadKnowledgeEmbedderAfterFirstFrame: loadKnowledgeEmbedderAfterFirstFrame,
      downloadKbModel: downloadKbModel,
      cancelKbModel: cancelKbModel,
      mountCollection: mountCollection,
      setCollectionEnabled: setCollectionEnabled,
      removeCollection: removeCollection,
      unmountCollection: unmountCollection,
      listCollections: function () { return invoke("kb_collection_list"); },
      kbModelStatus: function () { return invoke("kb_model_status"); },
    },
    scheduled: {
      loadScheduledTasks: loadScheduledTasks,
      readScheduledTask: readScheduledTask,
      loadScheduledTaskRuns: loadScheduledTaskRuns,
      loadScheduledTaskRecentRuns: loadScheduledTaskRecentRuns,
      selectScheduledTask: selectScheduledTask,
      refreshScheduledTaskData: refreshScheduledTaskData,
      clearScheduledTaskSelection: clearScheduledTaskSelection,
      dismissScheduledTaskError: dismissScheduledTaskError,
      createScheduledTask: createScheduledTask,
      updateScheduledTask: updateScheduledTask,
      pauseScheduledTask: pauseScheduledTask,
      resumeScheduledTask: resumeScheduledTask,
      toggleScheduledTaskPinned: toggleScheduledTaskPinned,
      deleteScheduledTask: deleteScheduledTask,
      runScheduledTaskNow: runScheduledTaskNow,
      pickFolder: pickFolder,
      startScheduledTaskChat: startScheduledTaskChat,
      confirmScheduledTaskDraft: confirmScheduledTaskDraft,
      clearScheduledTaskDraft: clearScheduledTaskDraft,
      openScheduledRunChat: openScheduledRunChat,
      exitScheduledRunChat: exitScheduledRunChat,
    },
    sessions: {
      createNewSession: createNewSession,
      switchToSession: switchToSession,
      deleteSession: deleteSession,
      renameSession: renameSession,
      toggleSessionPinned: toggleSessionPinned,
      archiveSession: archiveSession,
      restoreArchivedSession: restoreArchivedSession,
    },
    monitor: {
      startMonitorPolling: startMonitorPolling,
      stopMonitorPolling: stopMonitorPolling,
      clearMonitorStats: clearMonitorStats,
    },
    settings: {
      setSelectedPet: setSelectedPet,
      saveSettings: saveSettings,
      saveSettingsAndRestart: saveSettingsAndRestart,
      saveSearchSettings: saveSearchSettings,
      saveSearchSettingsAndRestart: saveSearchSettingsAndRestart,
      testSearchProvider: testSearchProvider,
    },
    feedback: { submitFeedback: submitFeedback },
    vllm: {
      discoverLocalVllm: discoverLocalVllm,
      detectLocalVllmSetup: detectLocalVllmSetup,
      bootstrapLocalVllm: bootstrapLocalVllm,
      dismissVllmSetup: dismissVllmSetup,
      declineVllmSetup: declineVllmSetup,
    },
    models: {
      getEffectiveModelConfig: getEffectiveModelConfig,
      getImageInputCapability: getImageInputCapability,
      loadModels: loadModels,
      saveModel: saveModel,
      revealModelApiKey: revealModelApiKey,
      deleteModel: deleteModel,
      setActiveModel: setActiveModel,
      loadSessionModel: loadSessionModel,
      switchModel: switchModel,
      testModelConnection: testModelConnection,
      testImageInputCapability: testImageInputCapability,
    },
    interaction: { toggleSuperPerm: toggleSuperPerm,
      // Plan/YOLO
    acceptPlan: acceptPlan,
    discardPlan: discardPlan,
    exitPlanToYolo: exitPlanToYolo,
    setPlanModeNext: setPlanModeNext,
    setMultiAgentMode: setMultiAgentMode,
    planStuckReplan: planStuckReplan,
    planStuckGo: planStuckGo,
    // 用户交互
    submitUserInput: submitUserInput,
    cancelUserInput: cancelUserInput,
    summonPinvou: summonPinvou,
    inspectPinvou: inspectPinvou,
    resolvePinvouReview: resolvePinvouReview,
    dismissPinvouReview: dismissPinvouReview,
    // 编辑/压缩
    editLastTurn: editLastTurn,
      compactNow: compactNow,
    },
    rendering: { renderMarkdown: renderMarkdown },
    remoteControl: {
      startRemoteControl: startRemoteControl,
      stopRemoteControl: stopRemoteControl,
      refreshRemoteControlQr: refreshRemoteControlQr,
      refreshRemoteControlStatus: refreshRemoteControlStatus,
      getWebRelaySettings: remoteControlFeature.getWebRelaySettings,
      setWebRelayAddress: remoteControlFeature.setWebRelayAddress,
      resetWebRelayAddress: remoteControlFeature.resetWebRelayAddress,
    },
    artifacts: {
      artifactInfo: artifactInfo,
      readArtifactText: readArtifactText,
      writeArtifactText: writeArtifactText,
      readArtifactImageB64: readArtifactImageB64,
      readArtifactThumbnail: readArtifactThumbnail,
      renderArtifactVisual: renderArtifactVisual,
      openContainingFolder: openContainingFolder,
      revealSessionFolder: revealSessionFolder,
      openScheduledTaskFolder: openScheduledTaskFolder,
      openInSystem: openInSystem,
      openArtifactExternal: openArtifactExternal,
      downloadArtifact: downloadArtifact,
      listDeliverables: listDeliverables,
      listDeliverableIndex: listDeliverableIndex,
      openExternalUrl: openExternalUrl,
      openUserExternalUrl: openUserExternalUrl,
    },
    attachments: {
      addAttachmentByPath: addAttachmentByPath,
      addPasteImage: addPasteImage,
      removeAttachment: removeAttachment,
      clearAttachments: clearAttachments,
      pickAndAttach: pickAndAttach,
      uploadDeviceFiles: uploadDeviceFiles,
      resolveConversationAttachment: resolveConversationAttachment,
      openConversationAttachment: openConversationAttachment,
      revealConversationAttachment: revealConversationAttachment,
    },
    resolutions: { markResolved: markResolved },
    multiAgent: {
      listSubagentTranscripts: listMultiAgentSubagents,
      readSubagentTranscript: readMultiAgentSubagent,
    },
    workflow: {
      loadSkills: loadSkills,
      activateSkill: activateSkill,
      deactivateSkill: deactivateSkill,
      openDemo: openDemo,
      closeDemo: closeDemo,
      setCurrentPhase: setCurrentPhase,
      startWorkflowTask: startWorkflowTask,
      stopWorkflowTask: stopWorkflowTask,
      listWorkflows: listWorkflows,
      resetWorkflowRun: resetWorkflowRun,
      selectWorkflowRole: selectWorkflowRole,
      closeWorkflowDrawer: closeWorkflowDrawer,
      getRolePrompt: getRolePrompt,
      getRoleOutputs: getRoleOutputs,
      getGateReport: getGateReport,
      getRoleLogs: getRoleLogs,
      submitWorkflowUserInput: submitWorkflowUserInput,
      pickAndAddMaterials: pickAndAddMaterials,
      addMaterialsToSession: addMaterialsToSession,
      attachRun: attachRun,
      resumeWorkflowOnBoot: resumeWorkflowOnBoot,
      approveWorkflowGate: approveWorkflowGate,
      rejectWorkflowGate: rejectWorkflowGate,
      retryWorkflowRole: retryWorkflowRole,
    },
    files: {
      pickFiles: pickFiles,
      pickFolders: pickFolders,
      pickFeedbackFiles: pickFeedbackFiles,
    },
    personas: {
      loadPersonas: loadPersonas,
      getPersonas: getPersonas,
      readPersonaBody: function (id) { return invoke("read_persona_body", { personaId: id }); },
      equipPersona: equipPersona,
      unequipPersona: unequipPersona,
      postCardCreatorIntro: function () { addChatItem({ type: "card_creator_intro", time: "" }); recordPersonaEvent({ kind: "card_creator_intro" }); notify(); },
      createPersona: createPersona,
      updatePersona: updatePersona,
      deletePersona: deletePersona,
    },
    memory: {
      loadMemoryOverview: loadMemoryOverview,
      saveMemoryProfilePatch: saveMemoryProfilePatch,
      deleteMemoryPreference: deleteMemoryPreference,
      updateMemoryItem: updateMemoryItem,
      deleteMemoryItem: deleteMemoryItem,
      archiveRecentWorkMemory: archiveRecentWorkMemory,
      confirmMemoryCandidate: confirmMemoryCandidate,
      ignoreMemoryCandidate: ignoreMemoryCandidate,
      neverMemoryCandidate: neverMemoryCandidate,
    },
    updater: {
      checkForUpdate: checkForUpdate,
      downloadAndInstallUpdate: downloadAndInstallUpdate,
      cancelUpdate: cancelUpdate,
      restartApp: restartApp,
    },
    dependencies: {
      checkDependencies: checkDependencies,
      installDependencies: installDependencies,
    },
  };

  // Auto-init after DOM ready
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    setTimeout(init, 0);
  }
})();
