/**
 * tauri-bridge.js — Tauri 后端通信桥
 *
 * 封装所有 invoke/listen，维护前端状态，通过 pub/sub 推给 React。
 * 浏览器预览时（无 window.__TAURI__）自动降级。
 */
(function () {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic-script artifact; strict mode is part of the payload
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
    const started = startupNow();
    startupMark(stage + ":start");
    try {
      const result = await action();
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
      const result = await invoke("refresh_connector_auth_gates");
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
      const ready = await invoke("kb_model_load_after_first_frame");
      const modelStatus = await invoke("kb_model_status").catch(function () { return null; });
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
      const failedStatus = await invoke("kb_model_status").catch(function () { return null; });
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
  // 优先委托共享渲染器 window.PinvouMarkdownRenderer（npm 版，含语法高亮）；在其尚未安装的
  // 短暂窗口退回 vendor 全局兜底。兜底实现已收敛到 shared/markdown-bridge-fallback.js
  // （随 index.html 以普通脚本加载，暴露 window.PinvouMarkdownBridgeFallback），消除两份逐字复制。
  // 最末级 fallback 必须自带 escapeHtml：共享脚本未加载时 renderMarkdown 仍会被
  // dangerouslySetInnerHTML 消费，原文返回即 fail-open。escapeHtml 作为安全原语保留在本文件
  // （不依赖任何外部脚本），仅 marked.parse+sanitize 这段较重的兜底被抽到共享文件。
  function escapeHtml(s) {
    return String(s).replaceAll(/[&<>"']/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c];
    });
  }
  function renderMarkdown(text) {
    if (window.PinvouMarkdownRenderer && typeof window.PinvouMarkdownRenderer.renderMarkdown === "function") {
      return window.PinvouMarkdownRenderer.renderMarkdown(text);
    }
    if (window.PinvouMarkdownBridgeFallback && typeof window.PinvouMarkdownBridgeFallback.renderMarkdown === "function") {
      return window.PinvouMarkdownBridgeFallback.renderMarkdown(text);
    }
    return escapeHtml(text || "");
  }


  // The pet is a separate WebView and must not own a second copy of the main
  // application state. Keep only the renderer used by its activity cards and
  // return before chat listeners, session loading, polling, or update checks.
  const locationSearch = String((window.location && window.location.search) || "");
  const isPetWindow = /(?:^|[?&])window=pet(?:&|$)/.test(locationSearch);
  const isDetachedWindow = /(?:^|[?&])detached=1(?:&|$)/.test(locationSearch);
  function detachedQueryValue(name) {
    const query = locationSearch.replace(/^\?/, "").split("&");
    for (let i = 0; i < query.length; i++) {
      const pair = query[i].split("=");
      if (pair[0] !== name) continue;
      try { return decodeURIComponent((pair.slice(1).join("=") || "").replaceAll('+', " ")); }
      catch { return pair.slice(1).join("=") || ""; }
    }
    return "";
  }
  const detachedWindowKind = isDetachedWindow ? (detachedQueryValue("kind") || "monitor") : "";
  const detachedWindowSessionId = isDetachedWindow ? detachedQueryValue("id") : "";
  if (isPetWindow) {
    window.TauriBridge = {
      available: false,
      rendering: { renderMarkdown },
    };
    return;
  }

  function installBridgeFeature(name, context) {
    const registry = window.__PINVOU_TAURI_BRIDGE_FEATURES__;
    const factory = registry && registry[name];
    if (typeof factory !== "function") throw new Error("Tauri bridge feature not loaded: " + name);
    return factory(context);
  }

  // ── State ────────────────────────────────────────────────────────
  const state = {
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
    // mid-turn steer 消息位置标记(sidecar, 不进 messages/LLM)。每项 {pos, text}：
    // steer 落盘与 admission 对齐、无 <turn_meta> 尾块, 重载靠它恢复 steeredMidTurn。
    steeredMessages: [],
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
      browserNativeDisplay: false,
      browserAgentAutomation: false,
      browserCdp: false,
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
    // 三个工作区 lane（work/design/code）的全局默认 mode（null=该 lane 未显式
    // 选过；缺省 code→plan、work/design→yolo）。草稿态 chip 显示与切换的事实源，
    // 启动时经 get_mode_defaults 拉取；草稿切换经 set_mode_default 写回。
    modeDefaults: { work: null, design: null, code: null },
    // 当前聊天页所处 lane（work/design；code 页车道有自己的草稿控件逻辑）。
    // lane 是纯前端概念，由 ChatView 随 pinvouMode 显式传入，bridge 不读
    // localStorage。
    modeLane: "work",
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
    // Token budget (input_tokens / maxModelLen). max=0 means the window is
    // unknown: the context meter line stays hidden (its render guard needs
    // max > 0) until a real window lands — a chat:usage context_window or a
    // local backend max_model_len. No fabricated denominator.
    tokens: { input: 0, max: 0 },
    // 思考指示器：active 时 React 渲染计时气泡（Braille + 思考中/调用工具 + 秒数）
    thinking: { active: false, phase: "thinking", toolName: "", startedAt: 0 },
    // 卡片池: 专家面具。activePersona = 当前 session 加持的专家卡(完整对象)或 null,
    // 驱动聊天室右上角挂件。
    activePersona: null,
    // 知识库挂载: 当前 session 挂载的知识集 id(number)或 null。仿 activePersona 走 buffer,
    // 仅驻内存(后端也只驻内存),重启回到未挂载。名字由前端用知识集列表解析。
    mountedCollection: null,
    mountedCollections: [],
    mountedRemoteCollections: [],
    mountedCollectionsRevision: 0,
    // personaPool 只放轻量元信息(loadState),1078 张卡放模块级 personaPoolCache,
    // 不进 state/订阅快照，避免每个流式 token 都复制完整卡池。
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
      host_workspace_authorized: false,
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
      progress: null,     // kb_model:progress 事件 { stage:'download'|'verify'|'prepare'|'done', downloaded, total, ready }
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
  let initPromise = null;
  // 卡片池 1078 张卡的前端缓存。只读,通过 getPersonas() 取引用,不走 notify 快照。

  // internal streaming state
  let currentStreamText = "";
  let currentStreamId = 0;
  let pendingAssistantText = "";
  let pendingAssistantBlocks = [];
  let itemIdSeq = 0;
  let toolMeta = {};       // id → { name, args }
  // 上下文行口径保护：TurnComplete 的 usage.input_tokens 是本轮所有请求的累加
  // （计费口径）。只有单请求的"干净轮"该值才等于当前上下文占用；本轮一旦出现
  // 工具调用/重试/压缩（= 多请求），就跳过这次 tokens 更新，保留上一个准确值。
  const turnUsageDirty = {};  // session_id → bool

  function safeConsoleInfo(...args) {
    if (typeof console !== "undefined" && typeof console.info === "function") {
      console.info(...args);
    }
  }
  function recordAuthoritySyncDiagnostic(event, details) {
    try {
      const diagnostics = window.PinvouAuthoritySyncDiagnostics;
      if (diagnostics && typeof diagnostics.record === "function") {
        diagnostics.record(event, details || {});
      }
    } catch { /* diagnostics reporting failure must degrade silently */ }
  }
  function authoritySyncBufferSnapshot(sid, buf) {
    return {
      session_id: sid || "",
      active_session_id: state.activeSessionId || "",
      buffer_present: !!buf,
      local_turn_owned: !!(buf && buf.localTurnOwned),
      remote_turn_active: !!(buf && buf.remoteTurnActive),
      remote_terminal_seen: !!(buf && buf.remoteTerminalSeen),
      loaded_from_disk: !!(buf && buf.loadedFromDisk),
      buffer_busy: !!(buf && buf.busy),
      ui_busy: !!state.busy,
      message_count: buf && Array.isArray(buf.messages) ? buf.messages.length : null,
      chat_item_count: buf && Array.isArray(buf.chatItems) ? buf.chatItems.length : null,
      queued_count: buf && Array.isArray(buf.queued) ? buf.queued.length : null,
      session_revision: String(buf && buf.sessionRevision || ""),
      committed_revision: String(buf && buf.remoteCommittedRevision || ""),
      expected_assistant_key_length: String(buf && buf.remoteExpectedAssistantKey || "").length,
      baseline_message_count: buf && buf.remoteBaselineMessageCount != null
        ? Number(buf.remoteBaselineMessageCount)
        : null,
      baseline_trusted: !!(buf && buf.remoteBaselineTrusted),
    };
  }

  // ── bridge 层 UI 文案（系统消息/状态标签）──────────────────────
  // bridge 在事件回调里生成文案,拿不到 React 的 t;按 state.settings.language 取词,中文兜底。
  // 注意:发给 LLM 的指令不在此表,保持中文。
  const BT_TABLE = {
    en: {
      newChatFailed: "⚠️ Failed to create chat: ", loadChatFailed: "⚠️ Failed to load chat: ", deleteFailed: "⚠️ Delete failed: ",
      personaUnequipped: "🎴 Expert card removed: ",
      reviewFixHeader: "Apply the review notes below — **revise only the targeted sections, do not rewrite the whole document**:",
      reviewVerifyHeader: "The items below involve external facts — **verify first, cite your sources, and don't edit from memory**:",
      reviewAdoptHeader: "The following decisions are final — update the artifacts accordingly:",
      reviewAskHeader: "For the pending items below, ask me formally via request_user_input instead of guessing:",
      reviewFillHeader: "The artifact is still missing the dimensions below — add them (keep everything else; add only, don't rewrite):",
      reviewFillFooter: "(For anything involving external facts, verify before writing and cite sources — don't fabricate from memory.)",
      planStuckReplanPrompt: "Use the todo_write tool to lay out the full plan steps — don't call write tools directly.",
      planStuckGoPrompt: "Continue the task per the plan discussed above — write files / run commands directly; no more plan discussion.",
      planHistorical: "📜 Past plan", planSuperseded: "📜 Superseded by a newer plan",
      attachStillParsing: "⚠️ Attachment still parsing, try again shortly",
      imageUnsupported: "The current model does not support images. Switch to an image-capable model, or configure a vision model in model settings.",
      imageUnknown: "Image input capability of the current model is unknown. If it supports images, set image input to “Supports images” in model settings; you can also configure a vision model.",
      turnAlreadyInProgress: "⚠️ This chat is already processing a turn. The duplicate send was not executed.",
      steerDropped: "Queued message was not delivered (turn interrupted), cancelled",
      steerFailed: "Interrupt failed (session unavailable or engine not running); your text was restored to the input",
      interruptQueuedFailed: "Interrupt & send failed; the message was restored to the queue",
      interruptBusy: "Another interrupt is already in progress; the message stays queued — retry in a moment",
      compactStart: "⏳ Compacting context", compactDone: "✓ Context compacted", compactFail: "⚠️ Compaction failed", compactAuto: " (auto)",
      compactPruneMerged: "Auto-compaction: tool-result cleanup, messages unchanged",
      compactInactive: "The session engine is not running yet. Send a message before compacting the context",
      gpuUnavailable: "GPU info unavailable",
      cpuUnavailable: "CPU info unavailable",
      superOn: "⚠️ Super permission enabled", superOff: "Super permission disabled",
      approved: "✅ Approved", echoGo: "✅ Do it",
      acceptPlanFailed: "⚠️ accept_plan failed: ",
      planDiscarded: "🚪 Plan discarded", discardPlanFailed: "⚠️ discard_plan failed: ", exitPlanFailed: "⚠️ Failed to exit Plan: ", switchModeFailed: "⚠️ Failed to switch mode: ", planContinueFailed: "⚠️ Failed to send continue instruction: ",
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
      voiceDeviceTimeout: "Microphone detection timed out; no recording device found. Check the device connection and the system microphone settings, then try again.",
      voiceTranscribing: "Transcribing…",
      voiceRecordingTooShort: "Recording is too short. Please try again.",
      voiceWrittenBack: "Transcribed text inserted into the input box",
      voiceCheckingDevice: "Checking microphone…",
      voiceRequestingPermission: "Requesting microphone permission…",
      voiceWebviewNoMic: "This WebView does not support microphone capture.",
      voiceWebviewNoRecording: "This WebView does not support audio recording.",
      voiceNoDeviceConnect: "No available microphone detected. Connect or enable a recording device, then try again.",
      voiceRecording: "Recording… tap again to finish",
      voicePermissionDeniedRetry: "Microphone permission was denied. Tap voice input again and choose Allow in the prompt; if it still fails, check the system microphone settings.",
      scheduledDraftInvalid: "The scheduled task draft is missing a name, task description, or schedule rule",
      scheduledCreateFailed: "Failed to create scheduled task: ",
      scheduledTaskFallbackName: "Scheduled task",
      scheduledActionBusy: "Another scheduled task operation is still in progress",
      scheduledCreateNoId: "Failed to create scheduled task: backend returned no task ID",
      scheduledChatPrefill: "I want to create a scheduled task: ",
      pickFolderTitle: "Choose a working directory",
      fileMediaFilterName: "Images and videos",
      kbPickFolderTitle: "Choose folders to import into the knowledge base",
      memoryWriteFailed: "Memory write failed: ", memoryIgnoreFailed: "Failed to ignore memory: ", memoryNeverFailed: "Failed to set \"never ask\": ",
      attachNeedSession: "⚠️ Start a new chat before adding attachments", attachEmptyFile: "Empty files cannot be added", attachAddCancelled: "Attachment add canceled", attachInvalidResult: "Attachment add returned no valid result", deviceUploadFailed: "⚠️ Upload failed: ",
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
      reviewFixHeader: "下のレビュー意見に従い、**該当するセクションのみを修正してください。全文の書き直しはしないでください**：",
      reviewVerifyHeader: "以下の項目は外部事実に関わります。**必ず検証してから修正し、根拠を示してください（記憶に頼った編集はしないでください）**：",
      reviewAdoptHeader: "以下の事項は確定しました。この通り成果物を更新してください：",
      reviewAskHeader: "以下の未確定項目については、推測せず request_user_input で正式に私に質問してください：",
      reviewFillHeader: "成果物には以下の観点が不足しています。補足してください（既存部分は保持し、追記のみで書き換えないでください）：",
      reviewFillFooter: "（外部事実に関わる部分は、検証してから記述し、根拠を示してください。記憶からの創作はしないでください。）",
      planStuckReplanPrompt: "todo_write ツールで計画の全ステップを出力してください。書き込み系ツールを直接実行しないでください。",
      planStuckGoPrompt: "上で議論した計画に従ってタスクを続行してください。ファイルの書き込みやコマンドの実行を直接行い、計画の再議論はしないでください。",
      planHistorical: "📜 過去のプラン", planSuperseded: "📜 新しいプランで上書きされました",
      attachStillParsing: "⚠️ 添付ファイルを解析中です。少し待ってから送信してください",
      imageUnsupported: "現在のモデルは画像に対応していません。画像対応モデルに切り替えるか、モデル設定でビジョンモデルを構成してください。",
      imageUnknown: "現在のモデルの画像入力能力は不明です。画像に対応している場合は、モデル設定で画像入力能力を「画像対応」に設定してください。ビジョンモデルを構成することもできます。",
      turnAlreadyInProgress: "⚠️ このチャットでは別のターンを処理中です。重複した送信は実行されませんでした。",
      steerDropped: "キューしたメッセージが未達（ターン中断）のため取り消しました",
      steerFailed: "割り込みに失敗しました（セッション無効またはエンジン未起動）。内容は入力欄に復元しました",
      interruptQueuedFailed: "割り込み送信に失敗しました。メッセージはキューに復元しました",
      interruptBusy: "別の割り込みが進行中のため実行できません。メッセージはキューに残ります。しばらくしてから再試行してください",
      compactStart: "⏳ コンテキストを圧縮中", compactDone: "✓ コンテキスト圧縮完了", compactFail: "⚠️ 圧縮に失敗", compactAuto: "（自動）",
      compactPruneMerged: "自動圧縮: ツール結果を整理、メッセージ数は不変",
      compactInactive: "セッション Engine はまだ起動していません。メッセージを送信してからコンテキストを圧縮してください",
      gpuUnavailable: "GPU 情報を取得できません",
      cpuUnavailable: "CPU 情報を取得できません",
      superOn: "⚠️ スーパー権限が有効になりました", superOff: "スーパー権限が無効になりました",
      approved: "✅ 承認済み", echoGo: "✅ これでいく",
      acceptPlanFailed: "⚠️ accept_plan に失敗: ",
      planDiscarded: "🚪 プランを破棄", discardPlanFailed: "⚠️ discard_plan に失敗: ", exitPlanFailed: "⚠️ Plan の終了に失敗: ", switchModeFailed: "⚠️ モード切替に失敗: ", planContinueFailed: "⚠️ 継続指示の送信に失敗: ",
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
      voiceDeviceTimeout: "マイク検出がタイムアウトし、録音デバイスが見つかりませんでした。デバイスの接続とシステムのマイク設定を確認して再試行してください。",
      voiceTranscribing: "音声を認識中…",
      voiceRecordingTooShort: "録音時間が短すぎます。再試行してください。",
      voiceWrittenBack: "音声を入力ボックスに書き込みました",
      voiceCheckingDevice: "マイクデバイスを確認中…",
      voiceRequestingPermission: "マイクの権限をリクエスト中…",
      voiceWebviewNoMic: "この WebView はマイク入力に対応していません。",
      voiceWebviewNoRecording: "この WebView は音声録音に対応していません。",
      voiceNoDeviceConnect: "利用可能なマイクが見つかりません。録音デバイスを接続または有効にして再試行してください。",
      voiceRecording: "録音中です。もう一度タップすると終了します",
      voicePermissionDeniedRetry: "マイクの権限が拒否されています。もう一度音声入力をタップし、許可を選択してください。それでも失敗する場合はシステムのマイク設定を確認してください。",
      scheduledDraftInvalid: "スケジュールタスクの下書きに名前・タスク説明・時間ルールのいずれかが不足しています",
      scheduledCreateFailed: "スケジュールタスクの作成に失敗：",
      scheduledTaskFallbackName: "スケジュールタスク",
      scheduledActionBusy: "別のスケジュールタスク操作がまだ実行中です",
      scheduledCreateNoId: "スケジュールタスクの作成に失敗：バックエンドがタスク ID を返しませんでした",
      scheduledChatPrefill: "スケジュールタスクを作成したい：",
      pickFolderTitle: "作業ディレクトリを選択",
      fileMediaFilterName: "画像と動画",
      kbPickFolderTitle: "知識ベースにインポートするフォルダーを選択",
      memoryWriteFailed: "メモリの書き込みに失敗: ", memoryIgnoreFailed: "メモリの無視に失敗: ", memoryNeverFailed: "「今後表示しない」の設定に失敗: ",
      attachNeedSession: "⚠️ 添付ファイルを追加する前に新しいチャットを開始してください", attachEmptyFile: "空のファイルは追加できません", attachAddCancelled: "添付ファイルの追加はキャンセルされました", attachInvalidResult: "添付ファイルの追加で有効な結果が返されませんでした", deviceUploadFailed: "⚠️ アップロードに失敗: ",
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
      reviewFixHeader: "请按下面的检阅意见，**只定向修改对应段落，不要全文重写**：",
      reviewVerifyHeader: "以下几条涉及外部事实，**先查证再改、标明依据，别凭记忆直接改**：",
      reviewAdoptHeader: "以下事项我已拍板，按此更新产物：",
      reviewAskHeader: "以下待定项请用 request_user_input 正式问我，别自己猜：",
      reviewFillHeader: "以下维度产物还缺，请补充进去（保留其余、只增不改）：",
      reviewFillFooter: "（涉及外部事实的，先查证再写、标依据，别凭记忆编。）",
      planStuckReplanPrompt: "请用 todo_write 工具输出完整方案步骤,不要直接调写工具。",
      planStuckGoPrompt: "按上面讨论的方案继续执行任务,直接写文件/跑命令,不要再讨论方案。",
      planHistorical: "📜 历史方案", planSuperseded: "📜 已被新方案覆盖",
      attachStillParsing: "⚠️ 附件还在解析,请稍后再发",
      imageUnsupported: "当前模型不支持图片。请切换到支持图片的模型，或在模型设置中配置视觉模型。",
      imageUnknown: "当前模型的图片输入能力未知。如果它支持图片，请在模型设置中将图片输入能力设为“支持图片”后重试；也可以配置视觉模型。",
      turnAlreadyInProgress: "⚠️ 当前会话已有一轮正在处理，本次重复发送未执行。",
      steerDropped: "排队消息未送达（回合中断），已取消",
      steerFailed: "插队失败（会话不可用或引擎未运行），内容已恢复到输入框",
      interruptQueuedFailed: "插队发送失败，消息已恢复到排队区",
      interruptBusy: "已有打断正在进行，消息保留在排队区，请稍后重试",
      compactStart: "⏳ 正在压缩上下文", compactDone: "✓ 上下文压缩完成", compactFail: "⚠️ 压缩失败", compactAuto: "（自动）",
      compactPruneMerged: "自动压缩：已整理工具结果，消息数不变",
      compactInactive: "会话引擎尚未运行。请先发送一条消息，再压缩上下文",
      gpuUnavailable: "GPU 信息不可用",
      cpuUnavailable: "CPU 信息不可用",
      superOn: "⚠️ 超级权限已开启", superOff: "超级权限已关闭",
      approved: "✅ 已批准", echoGo: "✅ 就这么干",
      acceptPlanFailed: "⚠️ accept_plan 失败: ",
      planDiscarded: "🚪 已放弃此方案", discardPlanFailed: "⚠️ discard_plan 失败: ", exitPlanFailed: "⚠️ 退出 Plan 失败: ", switchModeFailed: "⚠️ 切换模式失败: ", planContinueFailed: "⚠️ 发送继续执行指令失败: ",
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
      voiceDeviceTimeout: "麦克风检测超时，未发现可用录音设备。请检查设备连接和系统麦克风设置后重试。",
      voiceTranscribing: "正在识别语音…",
      voiceRecordingTooShort: "录音时间过短，请重试。",
      voiceWrittenBack: "语音已写入输入框",
      voiceCheckingDevice: "正在检测麦克风设备…",
      voiceRequestingPermission: "正在请求麦克风权限…",
      voiceWebviewNoMic: "当前 WebView 不支持麦克风采集。",
      voiceWebviewNoRecording: "当前 WebView 不支持音频录制。",
      voiceNoDeviceConnect: "未检测到可用麦克风，请连接或启用录音设备后重试。",
      voiceRecording: "正在录音，再点一次结束",
      voicePermissionDeniedRetry: "麦克风权限已被拒绝，请再次点击语音输入并在授权提示中选择允许；若仍失败，请检查系统麦克风设置。",
      scheduledDraftInvalid: "定时任务草稿缺少名称、任务说明或时间规则",
      scheduledCreateFailed: "定时任务创建失败：",
      scheduledTaskFallbackName: "定时任务",
      scheduledActionBusy: "另一个定时任务操作仍在进行中",
      scheduledCreateNoId: "创建定时任务失败：后端未返回任务 ID",
      scheduledChatPrefill: "我想创建一个定时任务：",
      pickFolderTitle: "选择工作目录",
      fileMediaFilterName: "图片和视频",
      kbPickFolderTitle: "选择要导入知识库的文件夹",
      memoryWriteFailed: "记忆写入失败：", memoryIgnoreFailed: "忽略记忆失败：", memoryNeverFailed: "设置不再提示失败：",
      attachNeedSession: "⚠️ 请先新建会话再添加附件", attachEmptyFile: "空文件无法添加", attachAddCancelled: "附件添加已取消", attachInvalidResult: "附件添加未返回有效结果", deviceUploadFailed: "⚠️ 上传失败: ",
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
    const lang = state.settings && state.settings.language;
    const m = lang === "en" ? BT_TABLE.en : lang === "ja" ? BT_TABLE.ja : BT_TABLE.zh;
    return m[key] === undefined ? BT_TABLE.zh[key] : m[key];
  }
  // Transfer badges are restored from message text, but messages persist in the
  // UI language used at send time; replay must match all three variants instead
  // of only the current language. Used for the review/plan wording keys.
  function textMatchesBtKey(text, key) {
    return text.includes(BT_TABLE.zh[key]) || text.includes(BT_TABLE.en[key]) || text.includes(BT_TABLE.ja[key]);
  }
  // 默认会话标题哨兵:三语兜底标题都视为占位(自动改名/显示映射的依据),
  // 与 web 桥和 main.jsx 的同款判断保持一致。
  function isDefaultChatTitle(title) {
    return [BT_TABLE.zh.newChatFallbackTitle, BT_TABLE.en.newChatFallbackTitle, BT_TABLE.ja.newChatFallbackTitle]
      .includes(title);
  }

  // ── Per-session 工作集缓冲（多 session 并发）────────────────────
  // active session 的工作集 = state.* + 上面那批模块级 stream 变量(保持原逻辑零改动)。
  // 后台 session 的工作集存在 sessionStates[id];后台事件进来时临时把工作集切到对应
  // buffer 跑同步逻辑再切回(saveWorkingSetTo/loadWorkingSetFrom),期间 suppressNotify
  // 避免把后台渲染成 active。异步收尾(落盘)按显式 session_id 路由,不依赖工作集。
  const sessionStates = {};
  const authoritativeTranscriptSyncs = Object.create(null);
  let authoritySyncTraceSequence = 0;
  const scheduledRunSessionOwners = Object.create(null);
  let suppressNotify = false;
  function discardManagedAttachment(result) {
    const draftUploadId = result && result.__pinvouManagedDraftAttachmentId;
    if (draftUploadId) return invoke("cancel_draft_file_upload", { uploadId: draftUploadId })
      .catch(function (error) { console.warn("[attachment] failed to discard draft attachment", error); });
    const sessionId = result && result.__pinvouManagedAttachmentSessionId;
    if (!sessionId || !result.path) return Promise.resolve();
    return invoke("discard_dropped_attachment", { sessionId, path: result.path })
      .catch(function (error) { console.warn("[attachment] failed to discard managed attachment", error); });
  }
  // sessionId → true:标题当前是「卡牌占位名」(加卡时自动取的),可被首条用户消息覆盖。
  // 卡牌名只在「加了卡但还没开口」时当临时标题;一旦开始对话,对话内容更能区分同卡会话。
  // 内存态(不持久化):重启后丢标记仅影响「加卡→重启→才发首条消息」这一冷门路径。
  const personaPlaceholderTitles = {};
  const PINVOU_SCENE_EVENTS_STORAGE_PREFIX = "pinvou_scene_events_v1:";
  function normalizePinvouScene(scene) {
    scene = String(scene || "").trim();
    return /^(work:document-writing|work:personal-workbench|design:poster|design:data-visualization|design:ppt)$/.test(scene) ? scene : "";
  }
  function pinvouSceneStorageKey(sid) {
    return PINVOU_SCENE_EVENTS_STORAGE_PREFIX + String(sid || "").trim();
  }
  function normalizePinvouSceneEvents(events) {
    return (Array.isArray(events) ? events : []).map(function (event) {
      const pos = Number(event && event.pos);
      const scene = normalizePinvouScene(event && event.scene);
      if (!Number.isFinite(pos) || pos < 0 || !scene) return null;
      return { pos: Math.floor(pos), scene };
    }).filter(Boolean).sort(function (left, right) { return left.pos - right.pos; });
  }
  function loadPinvouSceneEventsForSession(sid) {
    if (!sid || !window.localStorage) return [];
    try {
      return normalizePinvouSceneEvents(JSON.parse(window.localStorage.getItem(pinvouSceneStorageKey(sid)) || "[]"));
    } catch {
      return [];
    }
  }
  function savePinvouSceneEventsForSession(sid, events) {
    if (!sid) return;
    const normalized = normalizePinvouSceneEvents(events);
    try {
      if (window.localStorage) {
        window.localStorage.setItem(pinvouSceneStorageKey(sid), JSON.stringify(normalized));
      }
    } catch {
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
    const cached = loadPinvouSceneEventsForSession(sid);
    if (!sid) return cached;
    try {
      const remote = normalizePinvouSceneEvents(
        await invoke("get_session_pinvou_scene_events", { sessionId: sid })
      );
      if (remote.length) {
        try {
          window.localStorage.setItem(pinvouSceneStorageKey(sid), JSON.stringify(remote));
        } catch { /* fall back to the remote data when the localStorage write fails */ }
        return remote;
      }
      if (cached.length) {
        await invoke("save_session_pinvou_scene_events", { sessionId: sid, events: cached });
      }
      return cached;
    } catch {
      return cached;
    }
  }
  function recordPinvouSceneForMessage(sid, pos, scene) {
    scene = normalizePinvouScene(scene);
    pos = Number(pos);
    if (!sid || !scene || !Number.isFinite(pos) || pos < 0) return;
    pos = Math.floor(pos);
    let events = normalizePinvouSceneEvents(state.pinvouSceneEvents)
      .filter(function (event) { return event.pos !== pos; });
    events.push({ pos, scene });
    events = normalizePinvouSceneEvents(events);
    state.pinvouSceneEvents = events;
    savePinvouSceneEventsForSession(sid, events);
  }
  function pinvouSceneForMessagePos(pos) {
    const events = normalizePinvouSceneEvents(state.pinvouSceneEvents);
    for (let i = 0; i < events.length; i++) {
      if (events[i].pos === pos) return events[i].scene;
    }
    return "";
  }
  // ── Steered-message sidecar（mid-turn steer 位置标记）──────────────
  // steer 落盘与普通 admission 对齐、不含 <turn_meta> 块（Rust sanitize 统一
  // 剥离），重载投影无法再借信封反推"非 turn admission"，因此结算时把
  // {pos, text} 显式持久化。text 用于压实/编辑漂移后的保守校验：重载时
  // 仅在 pos 处展示文本一致才打 steeredMidTurn，失配退化为普通 admission，
  // 与信封时代的最坏情况相同。
  const STEERED_MESSAGES_STORAGE_PREFIX = "pinvou_steered_messages_v1:";
  function steeredMessagesStorageKey(sid) {
    return STEERED_MESSAGES_STORAGE_PREFIX + String(sid || "").trim();
  }
  function normalizeSteeredMessages(events) {
    const byPos = {};
    (Array.isArray(events) ? events : []).forEach(function (event) {
      const pos = Number(event && event.pos);
      const text = String(event && event.text != null ? event.text : "");
      if (!Number.isFinite(pos) || pos < 0 || !text) return;
      byPos[Math.floor(pos)] = { pos: Math.floor(pos), text };
    });
    return Object.keys(byPos).map(function (key) { return byPos[key]; })
      .sort(function (left, right) { return left.pos - right.pos; });
  }
  function loadSteeredMessagesForSession(sid) {
    if (!sid || !window.localStorage) return [];
    try {
      return normalizeSteeredMessages(JSON.parse(window.localStorage.getItem(steeredMessagesStorageKey(sid)) || "[]"));
    } catch {
      return [];
    }
  }
  function saveSteeredMessagesForSession(sid, events) {
    if (!sid) return;
    const normalized = normalizeSteeredMessages(events);
    try {
      if (window.localStorage) {
        window.localStorage.setItem(steeredMessagesStorageKey(sid), JSON.stringify(normalized));
      }
    } catch {
      // localStorage 只作离线缓存，写失败不影响后端 sidecar。
    }
    Promise.resolve().then(function () {
      return invoke("save_session_steered_messages", {
        sessionId: sid,
        events: normalized,
      });
    }).catch(function () {});
  }
  async function syncSteeredMessagesForSession(sid) {
    const cached = loadSteeredMessagesForSession(sid);
    if (!sid) return cached;
    try {
      const remote = normalizeSteeredMessages(
        await invoke("get_session_steered_messages", { sessionId: sid })
      );
      if (remote.length) {
        try {
          window.localStorage.setItem(steeredMessagesStorageKey(sid), JSON.stringify(remote));
        } catch { /* fall back to the remote data when the localStorage write fails */ }
        return remote;
      }
      if (cached.length) {
        await invoke("save_session_steered_messages", { sessionId: sid, events: cached });
      }
      return cached;
    } catch {
      return cached;
    }
  }
  // 合并写入一批 {pos, text}（同 pos 后写覆盖），并同步当前内存态（活动
  // session 写 state，后台 session 写其 buffer）。返回归一化后的完整列表。
  function recordSteeredMessages(sid, entries) {
    if (!sid || !Array.isArray(entries) || !entries.length) return;
    const base = sid === state.activeSessionId
      ? state.steeredMessages
      : (sessionStates[sid] && sessionStates[sid].steeredMessages) || loadSteeredMessagesForSession(sid);
    const merged = normalizeSteeredMessages([...(Array.isArray(base) ? base : []), ...entries]);
    if (sid === state.activeSessionId) state.steeredMessages = merged;
    else if (sessionStates[sid]) sessionStates[sid].steeredMessages = merged;
    saveSteeredMessagesForSession(sid, merged);
  }
  // 重载投影查询：pos 命中且展示文本一致才认（压实/编辑会让 pos 漂移，
  // 文本校验防止把别的 admission 误标为 steer）。
  function isSteeredMessagePos(pos, text) {
    const events = normalizeSteeredMessages(state.steeredMessages);
    for (let i = 0; i < events.length; i++) {
      if (events[i].pos === pos) return events[i].text === String(text || "");
    }
    return false;
  }
  const artifactTrackerFeature = installBridgeFeature("artifact-tracker", {
    state, invoke, sessionStates,
    notify,
    isScheduledRunSession,
  });
  const basename = artifactTrackerFeature.basename;
  const isAbsPath = artifactTrackerFeature.isAbsPath;
  const normalizedPath = artifactTrackerFeature.normalizedPath;
  const noteArtifactChange = artifactTrackerFeature.noteArtifactChange;
  const filterSessionArtifacts = artifactTrackerFeature.filterSessionArtifacts;
  const isDeliverable = artifactTrackerFeature.isDeliverable;
  const trackArtifact = artifactTrackerFeature.trackArtifact;
  const markTurnDirtyArtifact = artifactTrackerFeature.markTurnDirtyArtifact;
  const untrackArtifact = artifactTrackerFeature.untrackArtifact;
  const findPresentedArtifact = artifactTrackerFeature.findPresentedArtifact;
  const updatePresentedArtifact = artifactTrackerFeature.updatePresentedArtifact;
  const reconcileArtifacts = artifactTrackerFeature.reconcileArtifacts;
  const extractArtifactPaths = artifactTrackerFeature.extractArtifactPaths;
  const extractArtifactPath = artifactTrackerFeature.extractArtifactPath;
  const fileMutationAction = artifactTrackerFeature.fileMutationAction;
  const isPresentArtifactTool = artifactTrackerFeature.isPresentArtifactTool;
  const artifactPathFromToolOutput = artifactTrackerFeature.artifactPathFromToolOutput;
  const shouldUseToolOutputAsArtifact = artifactTrackerFeature.shouldUseToolOutputAsArtifact;
  const presentArtifactAbsPath = artifactTrackerFeature.presentArtifactAbsPath;

  const chatFeature = installBridgeFeature("chat", {
    state, invoke, TAURI,
    sessionStates, turnUsageDirty,
    personaPlaceholderTitles,
    renderMarkdown, safeConsoleInfo,
    recordAuthoritySyncDiagnostic,
    authoritySyncBufferSnapshot, bt,
    isDefaultChatTitle,
    notify,
    runSyncOnSession: function (...args) { return runSyncOnSession(...args); },
    startThinking: function (...args) { return startThinking(...args); },
    stopThinking: function (...args) { return stopThinking(...args); },
    ensureSessionBufferLoaded: function (...args) { return ensureSessionBufferLoaded(...args); },
    ensureSession: function (...args) { return ensureSession(...args); },
    getBuffer: function (...args) { return getBuffer(...args); },
    recordPinvouSceneForMessage,
    recordSteeredMessages,
    reconcileRemoteTurn: function (...args) { return reconcileRemoteTurn(...args); },
    markRemoteTurn: function (...args) { return markRemoteTurn(...args); },
    adoptManagedAttachments: function (...args) { return adoptManagedAttachments(...args); },
    discardManagedAttachment,
    isScheduledRunSession: function (...args) { return isScheduledRunSession(...args); },
    basename,
    userMessageDisplayText,
    extractArtifactPaths,
    fileMutationAction,
    parseScheduledTaskDraftFromText: function (...args) { return parseScheduledTaskDraftFromText(...args); },
    autoCreateScheduledTaskDraft: function (...args) { return autoCreateScheduledTaskDraft(...args); },
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
  const addChatItem = chatFeature.addChatItem;
  const toolCallAlreadyStarted = chatFeature.toolCallAlreadyStarted;
  const toolCallAlreadyFinished = chatFeature.toolCallAlreadyFinished;
  const hasChatItemForTool = chatFeature.hasChatItemForTool;
  const addSystemItem = chatFeature.addSystemItem;
  const addAuthoritySyncNotice = chatFeature.addAuthoritySyncNotice;
  const addOrMergePruneCompaction = chatFeature.addOrMergePruneCompaction;
  const timeStr = chatFeature.timeStr;
  const flushPendingTextBlock = chatFeature.flushPendingTextBlock;
  const flushAssistantMessageToHistory = chatFeature.flushAssistantMessageToHistory;
  const resetPendingAssistant = chatFeature.resetPendingAssistant;
  const isBusyFor = chatFeature.isBusyFor;
  const doSendFor = chatFeature.doSendFor;
  const flushQueued = chatFeature.flushQueued;
  const sendMessageToSession = chatFeature.sendMessageToSession;
  const sendMessage = chatFeature.sendMessage;
  const getComposerDraft = chatFeature.getComposerDraft;
  const setComposerDraft = chatFeature.setComposerDraft;
  const retryFirstTurn = chatFeature.retryFirstTurn;
  const prefillComposer = chatFeature.prefillComposer;
  const removeQueued = chatFeature.removeQueued;
  const prioritizeQueued = chatFeature.prioritizeQueued;
  const editQueued = chatFeature.editQueued;
  const steer = chatFeature.steer;
  const interruptAndSend = chatFeature.interruptAndSend;
  const interruptAndSendQueued = chatFeature.interruptAndSendQueued;
  const settleSteerCommitted = chatFeature.settleSteerCommitted;
  const settleSteerDropped = chatFeature.settleSteerDropped;
  const captureSteerPositions = chatFeature.captureSteerPositions;
  const summonPinvou = chatFeature.summonPinvou;
  const inspectPinvou = chatFeature.inspectPinvou;
  const resolvePinvouReview = chatFeature.resolvePinvouReview;
  const dismissPinvouReview = chatFeature.dismissPinvouReview;
  const cancelGeneration = chatFeature.cancelGeneration;

  const sessionsFeature = installBridgeFeature("sessions", {
    state, invoke, listen, notify,
    sessionStates, scheduledRunSessionOwners,
    personaPlaceholderTitles, turnUsageDirty,
    // Clean host-side per-session side tables when a session buffer is
    // deleted/evicted (modeStateEpochs is defined later in this file, so
    // the reference cannot be passed in directly — a lazy-lookup hook).
    onSessionBufferPurged: function (id, reason) {
      delete modeStateEpochs[id];
      // The localStorage cache of scene events is the only recovery copy
      // when the sidecar save fails or we are offline
      // (savePinvouSceneEventsForSession intentionally swallows backend
      // failures; syncPinvouSceneEventsForSession replays from this
      // cache). LRU capacity eviction (reason === "evict") ≠ session
      // deletion: removing the key here would let one failed save +
      // eviction silently lose all scene mappings, and the key itself is
      // only a few hundred bytes — keeping it costs far less than losing
      // data. Clean only on real session deletion ("delete"), preventing
      // unbounded accumulation across historical sessions (~5MB shared
      // quota).
      if (id && reason === "delete" && window.localStorage) {
        try { window.localStorage.removeItem(PINVOU_SCENE_EVENTS_STORAGE_PREFIX + id); } catch { /* localStorage may be unavailable or full; the key is a cache and its loss is non-fatal */ }
      }
      // Steer intermediates (stashed events / withdrawn texts / the
      // in-flight interrupt flag) are cleared together with the session: on
      // delete, the engine side is covered by SyncSession's SteerDropped; LRU
      // eviction (reason === "evict") also clears — the chip left the working
      // set with the buffer and no consumer remains for its events.
      if (id && typeof chatFeature.purgeSteerState === "function") {
        chatFeature.purgeSteerState(id);
      }
    },
    runSyncOnSession, persistMessagesFor,
    resetPendingAssistant: function (...args) { return resetPendingAssistant(...args); },
    stopThinking: function (...args) { return stopThinking(...args); },
    rerenderFromMessages,
    syncModeState: function (...args) { return syncModeState(...args); },
    applyAuthoritativeModeState,
    currentDraftModeState,
    syncActivePersona: function () { return syncActivePersona(); },
    syncMountedCollection: function () { return syncMountedCollection(); },
    reconcileArtifacts,
    loadSessionModel: function (...args) { return loadSessionModel(...args); },
    clearScheduledTaskSelection: function () { return clearScheduledTaskSelection(); },
    invalidateScheduledRecentRunsForSession: function (...args) { return invalidateScheduledRecentRunsForSession(...args); },
    setScheduledTaskError: function (...args) { return setScheduledTaskError(...args); },
    invalidateScheduledTaskReads: function (...args) { return invalidateScheduledTaskReads(...args); },
    applyScheduledRunViewed: function (...args) { return applyScheduledRunViewed(...args); },
    loadScheduledTaskRecentRuns: function (...args) { return loadScheduledTaskRecentRuns(...args); },
    addSystemItem, basename,
    isAbsPath,
    filterSessionArtifacts,
    scheduleShellPoll: function (...args) { return scheduleShellPoll(...args); },
    bt, userMessageDisplayText,
    loadPinvouSceneEventsForSession,
    syncPinvouSceneEventsForSession,
    loadSteeredMessagesForSession,
    syncSteeredMessagesForSession,
    loadMemoryOverview: function (...args) { return loadMemoryOverview(...args); },
    isScheduledRunSession,
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
  const freshBuffer = sessionsFeature.freshBuffer;
  const getBuffer = sessionsFeature.getBuffer;
  const touchSessionBuffer = sessionsFeature.touchSessionBuffer;
  const purgeSessionBuffer = sessionsFeature.purgeSessionBuffer;
  const isScheduledRunTerminal = sessionsFeature.isScheduledRunTerminal;
  const rememberScheduledRunOwner = sessionsFeature.rememberScheduledRunOwner;
  const markScheduledInitialTurnTerminal = sessionsFeature.markScheduledInitialTurnTerminal;
  const saveWorkingSetTo = sessionsFeature.saveWorkingSetTo;
  const loadWorkingSetFrom = sessionsFeature.loadWorkingSetFrom;
  const ensureSessionBufferLoaded = sessionsFeature.ensureSessionBufferLoaded;
  const refreshHistoryList = sessionsFeature.refreshHistoryList;
  const enterDraft = sessionsFeature.enterDraft;
  const createNewSession = sessionsFeature.createNewSession;
  const ensureSession = sessionsFeature.ensureSession;
  const hydratedMessageKey = sessionsFeature.hydratedMessageKey;
  const mergeHydratedArtifacts = sessionsFeature.mergeHydratedArtifacts;
  const mergeHydratedChatItems = sessionsFeature.mergeHydratedChatItems;
  const switchToSession = sessionsFeature.switchToSession;
  const openScheduledRunChat = sessionsFeature.openScheduledRunChat;
  const exitScheduledRunChat = sessionsFeature.exitScheduledRunChat;
  const deleteSession = sessionsFeature.deleteSession;
  const renameSession = sessionsFeature.renameSession;
  const toggleSessionPinned = sessionsFeature.toggleSessionPinned;
  const archiveSession = sessionsFeature.archiveSession;
  const restoreArchivedSession = sessionsFeature.restoreArchivedSession;
  function runSyncOnSession(sid, fn) {
    if (!sid || sid === state.activeSessionId) { fn(); return; }
    const bg = sessionStates[sid]; if (!bg) return;
    touchSessionBuffer(sid, bg, isScheduledRunSession(sid));
    const realId = state.activeSessionId;
    // 进入时就把【当前完整工作集】落进 restoreBuffer：realId 为 null（草稿态）
    // 时也要保存——草稿态可能已含乐观的 modeState（如刚打开的多智能体开关）、
    // 未发送文本，finally 里不能拿全新 freshBuffer 覆盖（否则开关状态被后台
    // 会话事件冲掉，表现为"打开开关后被正在运行的对话覆盖成关"）。
    const restoreBuffer = realId ? getBuffer(realId) : freshBuffer();
    saveWorkingSetTo(restoreBuffer);
    loadWorkingSetFrom(bg);
    state.activeSessionId = sid;
    const prev = suppressNotify; suppressNotify = true;
    try { fn(); }
    finally {
      suppressNotify = prev;
      saveWorkingSetTo(bg);
      state.activeSessionId = realId;
      // 恢复的是进入时的同一工作集对象（草稿态下保留乐观 modeState /
      // 未发送文本——saveWorkingSetTo 已含 composerDraft），不是全新 buffer。
      loadWorkingSetFrom(restoreBuffer);
    }
  }
  // ── modeState 权威写回收敛点（评审 P1）────────────────────────────
  // 任何「invoke 返回 / 事件负载」带来的权威 modeState 更新都必须走
  // applyAuthoritativeModeState：内部统一 bump per-session epoch（作废
  // 在途 syncModeState 的旧读取）+ 定向写回触发会话（await 期间用户可能
  // 已切走）。interaction / chat-events 两个 feature 共享同一份 epoch 表，
  // 散点手工 bump+写漏一处就会重现「旧读取覆盖权威值」竞态。
  const modeStateEpochs = {};
  function bumpModeStateEpoch(sid) {
    if (!sid) return;
    modeStateEpochs[sid] = (modeStateEpochs[sid] || 0) + 1;
  }
  function applyAuthoritativeModeState(sid, st) {
    bumpModeStateEpoch(sid);
    runSyncOnSession(sid || state.activeSessionId, function () {
      state.modeState = { mode: st.mode || "yolo", multiAgent: !!st.multi_agent };
    });
  }

  // 草稿态（无 active 会话）的 modeState：取当前 lane 的全局默认，缺省 yolo
  // （与后端 plain 缺省方向一致）。三分 lane 语义：草稿显示 = 本 lane 全局默认。
  function currentDraftModeState() {
    const lane = state.modeLane === "design" ? "design" : "work";
    const d = state.modeDefaults && state.modeDefaults[lane];
    return { mode: d || "yolo", multiAgent: false };
  }

  // 事件监听器统一入口:按 payload.session_id 路由同步逻辑;后台变更后补一次 notify 刷新列表。
  function markRemoteTurn(sid, buf, preserveCommittedRevision, cause) {
    if (!sid || !buf || buf.localTurnOwned) return;
    const wasActive = !!buf.remoteTurnActive;
    if (!buf.remoteTurnActive) {
      const meta = state.sessions.find(function (session) { return session.id === sid; });
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
    if (!wasActive) {
      recordAuthoritySyncDiagnostic("remote_turn_marked", Object.assign({
        cause: String(cause || "unspecified"),
        preserve_committed_revision: !!preserveCommittedRevision,
      }, authoritySyncBufferSnapshot(sid, buf)));
    }
  }
  function onSessionEvent(e, fn) {
    const sid = (e && e.payload && e.payload.session_id) || state.activeSessionId;
    if (sid) {
      const eventBuffer = getBuffer(sid);
      const eventName = String((e && e.event) || "");
      const isTurnEvent = /chat:(user_message|turn_started|delta|reasoning_start|reasoning_delta|reasoning_done|tool_start|tool_end|user_input_required|transient_error)$/.test(eventName);
      if (eventBuffer && !eventBuffer.localTurnOwned && (eventBuffer.busy || isTurnEvent)) {
        markRemoteTurn(sid, eventBuffer, false, "event:" + eventName);
      }
    }
    const isBg = sid && sid !== state.activeSessionId;
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
    const meta = state.sessions.find(function (s) { return s.id === sid; });
    if (!meta) return;
    const buf = sid === state.activeSessionId ? null : sessionStates[sid];
    const msgs = buf ? buf.messages : state.messages;
    const arts = filterSessionArtifacts(buf ? buf.artifacts : state.artifacts, sid);
    if (buf) buf.artifacts = arts;
    else state.artifacts = arts;
    try {
      try { await invoke("save_session_artifacts", { id: sid, paths: arts.map(function (a) { return a.path; }) }); } catch { /* disk-write failure must not block session switching */ }
      if (isDefaultChatTitle(meta.title) || personaPlaceholderTitles[sid]) {
        const firstUser = msgs.find(function (m) { return m.role === "user"; });
        // 自动标题复用展示层过滤（与 web 侧一致）：内部信封/子智能体交接不参与
        // 命名；hideInternalEnvelope=true 同时剥离 turn_meta/system-reminder 元数据
        // 块，避免 XML 痕迹进 sidebar 标题。
        const titleText = firstUser ? userMessageDisplayText(firstUser.content || [], true) : "";
        if (titleText) {
          const newTitle = titleText.slice(0, 20);
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
    } catch {
      return "";
    }
  }

  async function reconcileRemoteTurn(sid) {
    if (!sid) return true;
    const buf = sessionStates[sid];
    if (!buf || (!buf.remoteTurnActive && !buf.remoteTerminalSeen)) return true;
    if (!buf.remoteTerminalSeen && isBusyFor(sid)) {
      recordAuthoritySyncDiagnostic("reconcile_deferred_busy", authoritySyncBufferSnapshot(sid, buf));
      return false;
    }
    if (authoritativeTranscriptSyncs[sid]) {
      recordAuthoritySyncDiagnostic("reconcile_joined_inflight", authoritySyncBufferSnapshot(sid, buf));
      return authoritativeTranscriptSyncs[sid];
    }
    const traceId = "authority_reconcile_" + Date.now().toString(36) + "_" + (++authoritySyncTraceSequence);
    const expectedAssistantKey = buf.remoteTerminalSeen
      ? String(buf.remoteExpectedAssistantKey || "")
      : "";
    // A committed transcript revision is the backend's canonical identity for
    // this turn. Prefer it over presentation-derived message equality: native
    // tools may normalize blocks between streamed events and durable storage
    // without changing the committed conversation.
    let expectedCommittedRevision = buf.remoteTerminalSeen
      ? String(buf.remoteCommittedRevision || "")
      : "";
    const minimumTerminalMessageCount = expectedAssistantKey && Array.isArray(buf.messages)
      ? buf.messages.length
      : 0;
    recordAuthoritySyncDiagnostic("reconcile_started", Object.assign({
      trace_id: traceId,
      expected_committed_revision: expectedCommittedRevision,
      minimum_terminal_message_count: minimumTerminalMessageCount,
    }, authoritySyncBufferSnapshot(sid, buf)));
    const sync = (async function () {
      for (let attempt = 0; attempt < 6; attempt++) {
        if (attempt) await new Promise(function (resolve) { setTimeout(resolve, 250); });
        // A reconnect/replay can deliver the commit marker just after done,
        // and a newer turn's commit can land while this retry window is open.
        // Re-read the live revision every attempt so a bumped expected value
        // converges instead of comparing a stale one (which would report a
        // false unsynced warning and block queued sends until the next event).
        if (buf.remoteTerminalSeen) {
          expectedCommittedRevision = String(buf.remoteCommittedRevision || "");
        }
        const attemptStartedAt = Date.now();
        try {
          const saved = await invoke("load_session", { id: sid, setActive: false });
          if (!saved || !Array.isArray(saved.messages)) {
            recordAuthoritySyncDiagnostic("reconcile_attempt_rejected", {
              trace_id: traceId, session_id: sid, attempt: attempt + 1,
              reason: "invalid_snapshot", elapsed_ms: Date.now() - attemptStartedAt,
              snapshot_present: !!saved,
            });
            continue;
          }
          const savedRevision = String(saved.transcript_revision || saved.transcriptRevision || "");
          // 仅当快照确实携带 revision 时才用严格相等作为权威屏障;旧后端/旧契约
          // 不含该字段时降级到消息数与 assistant 身份校验,避免「期望非空但快照
          // 无字段」导致对账必然失败(每轮误报)。
          if (expectedCommittedRevision && savedRevision) {
            if (savedRevision !== expectedCommittedRevision) {
              recordAuthoritySyncDiagnostic("reconcile_attempt_rejected", {
                trace_id: traceId, session_id: sid, attempt: attempt + 1,
                reason: "revision_mismatch", elapsed_ms: Date.now() - attemptStartedAt,
                expected_committed_revision: expectedCommittedRevision,
                saved_revision: savedRevision,
                saved_message_count: saved.messages.length,
              });
              continue;
            }
          } else {
            if (minimumTerminalMessageCount && saved.messages.length < minimumTerminalMessageCount) {
              recordAuthoritySyncDiagnostic("reconcile_attempt_rejected", {
                trace_id: traceId, session_id: sid, attempt: attempt + 1,
                reason: "message_count_short", elapsed_ms: Date.now() - attemptStartedAt,
                expected_committed_revision: expectedCommittedRevision,
                saved_revision: savedRevision,
                minimum_terminal_message_count: minimumTerminalMessageCount,
                saved_message_count: saved.messages.length,
              });
              continue;
            }
          }
          if ((!expectedCommittedRevision || !savedRevision) && expectedAssistantKey) {
            const hasExpectedAssistant = saved.messages.some(function (message) {
              return message && message.role === "assistant" &&
                hydratedMessageKey(message, isScheduledRunSession(sid)) === expectedAssistantKey;
            });
            if (!hasExpectedAssistant) {
              recordAuthoritySyncDiagnostic("reconcile_attempt_rejected", {
                trace_id: traceId, session_id: sid, attempt: attempt + 1,
                reason: "assistant_identity_missing", elapsed_ms: Date.now() - attemptStartedAt,
                expected_committed_revision: expectedCommittedRevision,
                saved_revision: savedRevision,
                expected_assistant_key_length: expectedAssistantKey.length,
                saved_message_count: saved.messages.length,
                saved_roles: saved.messages.map(function (message) { return message && message.role || "invalid"; }).slice(-12),
              });
              continue;
            }
          }
          // 写入前回合归属校验（与 web 版对齐，审计 #257）：重试窗口内新回合
          // 可能已开始（markRemoteTurn 置 busy/remoteTurnActive、重置 revision），
          // 此时用旧终稿重建工作集会截断新回合直播流——放弃本轮对账，由新回合
          // 自己的 done 事件重新对账。放弃条件只用 busy：不能用 remoteTurnActive
          // （正常远端回合 done 后它恒为 true），也不能用 !remoteTerminalSeen
          // （scheduled run 因 requiresAuthorityReconcile=false 不置 terminalSeen，
          // 但 flushQueued 仍会走 reconcile，会误伤）。
          if (buf.busy) return false;
          runSyncOnSession(sid, function () {
            const rawLiveChatItems = Array.isArray(state.chatItems) ? state.chatItems : [];
            const resolvedPlanTickets = Object.create(null);
            const activePlanCards = Object.create(null);
            rawLiveChatItems.forEach(function (item) {
              if (!item || item.type !== "plan_card") return;
              const key = planCardHydrationKey(item);
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
            const liveChatItems = rawLiveChatItems.filter(function (item) {
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
            for (let planIndex = state.chatItems.length - 1; planIndex >= 0; planIndex--) {
              const hydratedPlan = state.chatItems[planIndex];
              if (!hydratedPlan || hydratedPlan.type !== "plan_card") continue;
              const hydratedKey = planCardHydrationKey(hydratedPlan);
              const activeQueue = hydratedKey && activePlanCards[hydratedKey];
              if (activeQueue && activeQueue.length) {
                const liveActivePlan = activeQueue.pop();
                hydratedPlan.planId = String(liveActivePlan.planId);
                hydratedPlan.cardState = "active";
                hydratedPlan.resolved = false;
                hydratedPlan.statusLabel = liveActivePlan.statusLabel || "";
                hydratedPlan.planResolutionConfirmed = !!liveActivePlan.planResolutionConfirmed;
                continue;
              }
              const ticketQueue = hydratedKey && resolvedPlanTickets[hydratedKey];
              if (!hydratedPlan.planId && ticketQueue && ticketQueue.length) hydratedPlan.planId = ticketQueue.pop();
            }
            const unmatchedActivePlans = [];
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
          recordAuthoritySyncDiagnostic("reconcile_succeeded", Object.assign({
            trace_id: traceId,
            attempt: attempt + 1,
            elapsed_ms: Date.now() - attemptStartedAt,
            saved_revision: savedRevision,
            saved_message_count: saved.messages.length,
          }, authoritySyncBufferSnapshot(sid, buf)));
          return true;
        } catch {
          recordAuthoritySyncDiagnostic("reconcile_attempt_failed", {
            trace_id: traceId,
            session_id: sid,
            attempt: attempt + 1,
            reason: "load_session_error",
            error_category: "snapshot_load_failed",
            error_present: true,
            elapsed_ms: Date.now() - attemptStartedAt,
            expected_committed_revision: expectedCommittedRevision,
          });
        }
      }
      recordAuthoritySyncDiagnostic("reconcile_exhausted", Object.assign({
        trace_id: traceId,
        attempts: 6,
        expected_committed_revision: expectedCommittedRevision,
        minimum_terminal_message_count: minimumTerminalMessageCount,
      }, authoritySyncBufferSnapshot(sid, buf)));
      return false;
    })();
    authoritativeTranscriptSyncs[sid] = sync;
    try { return await sync; }
    finally { if (authoritativeTranscriptSyncs[sid] === sync) delete authoritativeTranscriptSyncs[sid]; }
  }

  // ── Pub/Sub ──────────────────────────────────────────────────────
  // let (not const): unsubscribe removes a subscriber by replacing the whole array.
  let subscribers = [];
  const STATE_SLICE_FIELDS = {
    platform: ["appVersion", "backendOnline", "platformCapabilities"],
    sessions: ["sessions", "archivedSessions", "activeSessionId", "sessionBusy", "draftEpoch"],
    chat: ["activeSkill", "artifacts", "artifactChange", "attachments", "busy", "chatItems", "composerDraft", "composerPrefill", "messages", "modeState", "planSnapshot", "queued", "thinking", "tokens", "turnDirtyArtifacts", "turnPresentedArtifacts", "turnTimeline"],
    voice: ["voiceInput", "voiceAsrSetup"],
    knowledge: ["kbModelSetup", "mountedCollection", "mountedCollections", "mountedRemoteCollections", "mountedCollectionsRevision"],
    scheduled: ["scheduledRunContext", "scheduledTaskAutoOpenId", "scheduledTaskBusyAction", "scheduledTaskCreationSessionId", "scheduledTaskDetail", "scheduledTaskDraft", "scheduledTaskError", "scheduledTaskErrorKind", "scheduledTaskLoading", "scheduledTaskPendingGuide", "scheduledTaskRecentRuns", "scheduledTaskRuns", "scheduledTasks", "scheduledTaskSelectionGeneration", "selectedScheduledTaskId"],
    monitor: ["monitor", "monitorError"],
    settings: ["settings", "selectedPet"],
    models: ["activeModelId", "currentSessionModelId", "effectiveModelConfig", "savedModels"],
    vllm: ["vllmBootstrapDone", "vllmBootstrapError", "vllmBootstrapping", "vllmSetup", "vllmSetupAttempt", "vllmSetupDismissed", "vllmSetupPhase"],
    interaction: ["pinvouModal", "pinvouReviews", "pinvouSummoning", "superPermEnabled"],
    personas: ["activePersona", "personaEvents", "personaPool"],
    memory: ["memory"],
    remoteControl: ["webAccess"],
    updater: ["updateCancelling", "updateCheckError", "updateChecking", "updateDownloading", "updateError", "updateInfo", "updateProgress", "updateReady"],
    dependencies: ["deps", "depsChecking", "depsInstallError", "depsInstallProgress", "depsInstalling"],
  };
  function snapshotStateSlice(domain) {
    const fields = STATE_SLICE_FIELDS[domain];
    if (!fields) throw new Error("Unknown Tauri bridge state slice: " + domain);
    const slice = {};
    for (let i = 0; i < fields.length; i++) slice[fields[i]] = state[fields[i]];
    if (typeof structuredClone === "function") {
      try { return structuredClone(slice); } catch { /* silent JSON fallback */ } // safari14-ok: typeof-guarded with JSON fallback
    }
    return JSON.parse(JSON.stringify(slice));
  }
  function snapshotStateSlices(domains) {
    if (!Array.isArray(domains) || domains.length === 0) {
      throw new Error("Tauri bridge state.getMany requires at least one domain");
    }
    const result = {};
    for (let i = 0; i < domains.length; i++) Object.assign(result, snapshotStateSlice(domains[i]));
    return result;
  }
  // Subscription snapshots are immutable persistent projections. The first
  // projection detaches nested state; later notifications reconcile against it
  // and allocate only changed paths. Long transcripts therefore stay shared
  // between snapshots while an in-place streaming item mutation still produces
  // a stable new item for subscribers. get/getMany retain their deep-copy API.
  function defineSubscriptionStateProperty(target, key, value) {
    Object.defineProperty(target, key, {
      configurable: true,
      enumerable: true,
      value,
      writable: true,
    });
  }
  function copySubscriptionStateObject(source) {
    const result = {};
    Object.keys(source).forEach(function (key) {
      defineSubscriptionStateProperty(result, key, source[key]);
    });
    return result;
  }
  // eslint-disable-next-line sonarjs/cognitive-complexity -- legacy bridge; refactor tracked separately
  function subscriptionStateValue(value, previous, ancestors) {
    const valueType = typeof value;
    if (!value || valueType !== "object") {
      if (["function", "symbol", "bigint"].includes(valueType)) {
        throw new TypeError("Subscription state only supports JSON-like scalar values");
      }
      return value;
    }
    const isArray = Array.isArray(value);
    if (!isArray) {
      const prototype = Object.getPrototypeOf(value);
      const isPlainObject = prototype === null || (
        Object.getPrototypeOf(prototype) === null &&
        // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 floor; Object.hasOwn is unavailable and this call is already the safe form
        Object.prototype.hasOwnProperty.call(prototype, "constructor") &&
        prototype.constructor && prototype.constructor.name === "Object"
      );
      if (!isPlainObject) {
        throw new TypeError("Subscription state only supports arrays and plain objects");
      }
    }
    ancestors = ancestors || new WeakSet();
    if (ancestors.has(value)) throw new TypeError("Subscription state must not contain cycles");
    ancestors.add(value);
    try {
      if (isArray) {
        const previousArray = Array.isArray(previous) ? previous : null;
        if (!previousArray) {
          return Object.freeze(value.map(function (item) {
            return subscriptionStateValue(item, undefined, ancestors);
          }));
        }
        let nextArray = value.length === previousArray.length ? null : previousArray.slice(0, value.length);
        for (let arrayIndex = 0; arrayIndex < value.length; arrayIndex++) {
          const nextItem = subscriptionStateValue(value[arrayIndex], previousArray[arrayIndex], ancestors);
          if (!Object.is(nextItem, previousArray[arrayIndex])) {
            if (!nextArray) nextArray = [...previousArray];
            nextArray[arrayIndex] = nextItem;
          }
        }
        return nextArray ? Object.freeze(nextArray) : previousArray;
      }

      const keys = Object.keys(value);
      const previousObject = previous && typeof previous === "object" && !Array.isArray(previous)
        ? previous
        : null;
      const previousKeys = previousObject ? Object.keys(previousObject) : [];
      const sameShape = !!previousObject && keys.length === previousKeys.length && keys.every(function (key) {
        // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 floor; Object.hasOwn is unavailable and this call is already the safe form
        return Object.prototype.hasOwnProperty.call(previousObject, key);
      });
      let nextObject = sameShape ? null : {};
      for (let objectIndex = 0; objectIndex < keys.length; objectIndex++) {
        const key = keys[objectIndex];
        const nextValue = subscriptionStateValue(value[key], previousObject && previousObject[key], ancestors);
        if (!sameShape || !Object.is(nextValue, previousObject[key])) {
          if (!nextObject) nextObject = copySubscriptionStateObject(previousObject);
          defineSubscriptionStateProperty(nextObject, key, nextValue);
        }
      }
      return nextObject ? Object.freeze(nextObject) : previousObject;
    } finally {
      ancestors.delete(value);
    }
  }
  let subscriptionSliceRevision = 0;
  const subscriptionSliceCache = Object.create(null);
  function subscriptionStateSlice(domain) {
    const fields = STATE_SLICE_FIELDS[domain];
    if (!fields) throw new Error("Unknown Tauri bridge state slice: " + domain);
    const cached = subscriptionSliceCache[domain];
    if (cached && cached.revision === subscriptionSliceRevision) return cached.snapshot;
    const current = {};
    for (let i = 0; i < fields.length; i++) {
      current[fields[i]] = state[fields[i]];
    }
    const snapshot = subscriptionStateValue(current, cached && cached.snapshot);
    subscriptionSliceCache[domain] = { revision: subscriptionSliceRevision, snapshot };
    return snapshot;
  }
  function subscriptionStateSlices(domains) {
    if (!Array.isArray(domains) || domains.length === 0) {
      throw new Error("Tauri bridge state.subscribeMany requires at least one domain");
    }
    const result = {};
    for (let i = 0; i < domains.length; i++) {
      Object.assign(result, subscriptionStateSlice(domains[i]));
    }
    return Object.freeze(result);
  }
  // 远端实时快照已由 transcript 事件流(chat:transcript_committed 等)承载:
  // 旧 publishRemoteLiveSnapshot 调用的 remote_control_publish_event 命令名
  // 在 Rust 侧从未注册("session_snapshot" 也不在任何事件白名单),属 v1
  // 遗留死调用,已删除。

  let notificationQueue = [];
  let notificationDispatching = false;
  function notify() {
    if (suppressNotify) return;
    // 会话列表「工作中」指示:active 取活动工作集 state.busy,其余取各自 buffer.busy
    state.sessionBusy = {};
    for (const id in sessionStates) state.sessionBusy[id] = !!sessionStates[id].busy;
    if (state.activeSessionId) state.sessionBusy[state.activeSessionId] = !!state.busy;
    subscriptionSliceRevision += 1;
    // Membership and state are fixed when a round is queued. Subscribe or
    // unsubscribe during a callback affects only rounds queued afterwards.
    const members = [...subscribers];
    notificationQueue.push(members.map(function (subscriber) {
      return { callback: subscriber.callback, snapshot: subscriber.snapshot() };
    }));
    if (notificationDispatching) return;
    notificationDispatching = true;
    try {
      while (notificationQueue.length) {
        const round = notificationQueue.shift();
        for (let i = 0; i < round.length; i++) round[i].callback(round[i].snapshot);
      }
    } catch (error) {
      // Preserve synchronous callback error propagation. Later queued rounds may
      // depend on the interrupted callback, so discard them instead of replaying
      // stale work on the next notification.
      notificationQueue = [];
      throw error;
    } finally {
      notificationDispatching = false;
    }
  }
  function subscribe(snapshot, callback) {
    const subscriber = { snapshot, callback };
    subscribers.push(subscriber);
    return function () {
      subscribers = subscribers.filter(function (candidate) { return candidate !== subscriber; });
    };
  }
  function subscribeStateSlice(domain, fn) {
    subscriptionStateSlice(domain);
    return subscribe(function () { return subscriptionStateSlice(domain); }, fn);
  }
  function subscribeStateSlices(domains, fn) {
    subscriptionStateSlices(domains);
    return subscribe(function () { return subscriptionStateSlices(domains); }, fn);
  }

  const scheduledFeature = installBridgeFeature("scheduled", { state, notify, invoke, bt, runSyncOnSession, addSystemItem, rememberScheduledRunOwner, isScheduledRunTerminal, purgeSessionBuffer, createNewSession, prefillComposer, sessionStates });
  const applyScheduledRunViewed = scheduledFeature.applyScheduledRunViewed;
  const invalidateScheduledTaskReads = scheduledFeature.invalidateScheduledTaskReads;
  const invalidateScheduledRecentRunsForSession = scheduledFeature.invalidateScheduledRecentRunsForSession;
  const scheduleScheduledRunRefresh = scheduledFeature.scheduleScheduledRunRefresh;
  const setScheduledTaskError = scheduledFeature.setScheduledTaskError;
  const dismissScheduledTaskError = scheduledFeature.dismissScheduledTaskError;
  const selectScheduledTask = scheduledFeature.selectScheduledTask;
  const clearScheduledTaskSelection = scheduledFeature.clearScheduledTaskSelection;
  const parseScheduledTaskDraftFromText = scheduledFeature.parseScheduledTaskDraftFromText;
  const clearScheduledTaskDraft = scheduledFeature.clearScheduledTaskDraft;
  const confirmScheduledTaskDraft = scheduledFeature.confirmScheduledTaskDraft;
  const autoCreateScheduledTaskDraft = scheduledFeature.autoCreateScheduledTaskDraft;
  const loadScheduledTasks = scheduledFeature.loadScheduledTasks;
  const readScheduledTask = scheduledFeature.readScheduledTask;
  const loadScheduledTaskRuns = scheduledFeature.loadScheduledTaskRuns;
  const loadScheduledTaskRecentRuns = scheduledFeature.loadScheduledTaskRecentRuns;
  const refreshScheduledTaskData = scheduledFeature.refreshScheduledTaskData;
  const createScheduledTask = scheduledFeature.createScheduledTask;
  const updateScheduledTask = scheduledFeature.updateScheduledTask;
  const pauseScheduledTask = scheduledFeature.pauseScheduledTask;
  const resumeScheduledTask = scheduledFeature.resumeScheduledTask;
  const toggleScheduledTaskPinned = scheduledFeature.toggleScheduledTaskPinned;
  const deleteScheduledTask = scheduledFeature.deleteScheduledTask;
  const runScheduledTaskNow = scheduledFeature.runScheduledTaskNow;
  const startScheduledTaskChat = scheduledFeature.startScheduledTaskChat;
  // ── Session management ───────────────────────────────────────────
  const PLAN_TOOLS = new Set(["update_plan", "checklist_write", "todo_write"]);

  // tool_result.content 可能是 string 或 Anthropic content blocks 数组，归一成纯文本。
  function toolResultText(content) {
    if (typeof content === "string") return content;
    if (Array.isArray(content)) {
      return content.map(function (b) { return b && typeof b.text === "string" ? b.text : ""; }).join("");
    }
    return "";
  }

  // CodeWhale may append model-only recovery guidance to a persisted tool result
  // to preserve strict provider role ordering. Keep that guidance in durable/model
  // context, but remove only the two known internal suffix kinds from tool cards.
  function stripInternalToolRuntimeSuffix(value) {
    let text = String(value == null ? "" : value);
    const marker = "\n\n<codewhale:runtime_event";
    while (true) {
      const start = text.lastIndexOf(marker);
      if (start < 0) return text;
      const suffix = text.slice(start + 2);
      const opening = suffix.match(/^<codewhale:runtime_event\b[^>]*>/i);
      if (!opening || !/<\/codewhale:runtime_event>\s*$/i.test(suffix)) return text;
      const tag = opening[0];
      const knownKind = /\bkind=(["'])(?:stuck_guard|tool_error_degradation)\1/i.test(tag);
      const internal = /\bvisibility=(["'])internal\1/i.test(tag);
      if (!knownKind || !internal) return text;
      text = text.slice(0, start);
    }
  }

  function toolResultDisplayContent(content) {
    if (typeof content === "string") return stripInternalToolRuntimeSuffix(content);
    if (!Array.isArray(content)) return content;
    return content.map(function (block) {
      if (!block || typeof block.text !== "string") return block;
      return Object.assign({}, block, { text: stripInternalToolRuntimeSuffix(block.text) });
    });
  }

  // plan 类工具结果格式："...updated:\n{json}"——切第一个换行后 parse（与 engine.rs 一致）。
  function parsePlanSnapshot(content) {
    const txt = toolResultText(content);
    const i = txt.indexOf("\n");
    if (i < 0) return null;
    try { return JSON.parse(txt.slice(i + 1)); } catch { return null; }
  }

  // request_user_input 结果是纯 JSON {answers:[{id,label,value}]}（turn_loop.rs ToolResult::json）。
  // 按 question.id 匹配，还原成 UserInputCard 的 answers 数组（顺序对齐 questions）。
  // multi_select 多选保留全部同 id 答案、不塌缩（与 code-native-lane parseNativeUserAnswers 对齐）。
  function parseUserAnswers(content, questions) {
    let ans;
    try { ans = JSON.parse(toolResultText(content)).answers; } catch { return null; }
    if (!Array.isArray(ans)) return null;
    // 用无原型对象：question id 仅后端校验非空，constructor/toString/__proto__ 是合法输入，
    // 普通 {} 会让这些键命中 Object.prototype 继承属性，.push 抛 TypeError（复核 P1）。
    const byId = Object.create(null);
    ans.forEach(function (a) {
      if (a && a.id != null) {
        byId[a.id] = byId[a.id] || [];
        byId[a.id].push(a);
      }
    });
    const out = [];
    for (let qi = 0; qi < questions.length; qi++) {
      const q = questions[qi];
      const matches = byId[q.id];
      if (!matches || !matches.length) { out.push(null); continue; }
      matches.forEach(function (a) { out.push({ id: q.id, label: a.label, value: a.value }); });
    }
    return out;
  }

  // careful hook 拦截结果(shell.rs BLOCKED 固定格式)→ 反解出 careful_blocked 卡所需 metadata。
  // metadata 不进持久化 messages,session 重载只能从 tool_result 文本识别,否则 🛑 红卡重启即丢。
  function parseCarefulBlocked(text) {
    if (typeof text !== "string" || text.indexOf("BLOCKED: This command was blocked for safety reasons") !== 0) return null;
    const rm = text.match(/Reasons: ([^\n]*)/);
    const sm = text.match(/Suggestions: ([^\n]*)/);
    return {
      safety_level: "dangerous", blocked: true,
      reasons: rm && rm[1] ? rm[1].split("; ") : [],
      suggestions: sm && sm[1] ? sm[1].split("; ") : [],
    };
  }

  function userMessageInputProvenance(blocks) {
    return window.PinvouBridgeMessages.userMessageInputProvenance(blocks);
  }

  function isInternalUserMessageProvenance(provenance) {
    return window.PinvouBridgeMessages.isInternalUserMessageProvenance(provenance);
  }

  function isInternalRuntimeEnvelopeText(value) {
    return window.PinvouBridgeMessages.isInternalRuntimeUserMessage(value);
  }

  // Engine 的运行时恢复提示为了兼容模型协议会以 role=user 持久化，但它不是用户输入。
  // 子智能体完成交接同理：结果必须留在父模型上下文，但不能冒充用户消息上屏。
  // 原始 blocks 必须保留给模型续聊；展示层只隐藏该内部消息，避免伪装成用户气泡/新 Turn。
  // 定时会话还会过滤送模 envelope，只投影真实任务正文。
  function userMessageDisplayText(blocks, hideInternalEnvelope) {
    const textParts = (Array.isArray(blocks) ? blocks : [])
      .filter(function (block) { return block && block.type === "text"; })
      .map(function (block) { return String(block.text || ""); });
    if (textParts.some(isInternalRuntimeEnvelopeText)) return "";
    if (isInternalUserMessageProvenance(userMessageInputProvenance(blocks))) return "";
    if (!hideInternalEnvelope) return textParts.join("");

    return textParts.filter(function (text) {
      const trimmed = text.trim();
      return !(
        (trimmed.indexOf("<turn_meta>") === 0 && trimmed.endsWith("</turn_meta>")) ||
        trimmed === "<turn_meta_unchanged />"
      );
    }).map(function (text) {
      return text.replace(/^\s*<system-reminder>[\s\S]*?<\/system-reminder>\s*/, "");
    }).join("");
  }

  // ── Rerender from messages (session restore) ─────────────────────
  // opts.keepLiveToolMeta: passed when hydrating a live session
  // (hydrateLiveSession) — the buffer's toolMeta may hold entries for
  // in-flight tools (tool_use not yet in messages), and clearing it would
  // leave the later chat:tool_end without its meta (stuck selection
  // card / degraded artifact card).
  // eslint-disable-next-line sonarjs/cognitive-complexity -- legacy bridge; refactor tracked separately
  function rerenderFromMessages(opts) {
    state.chatItems = [];
    itemIdSeq = 0;
    // Replay re-adds every historical tool_use's metadata (including
    // write/patch's large args) to toolMeta for tool_result backfill and
    // never deletes after backfill — the residue resides in the buffer
    // with the working set, and memory is bounded by the 32-entry
    // all-session LRU cap. Durable replay clears first (when not live
    // hydrating), reclaiming only orphan entries left by interrupted
    // turns (the live event path itself stays insert/delete balanced);
    // the replay then rebuilds the needed entries for the historical
    // tool_uses inside messages.
    if (!(opts && opts.keepLiveToolMeta)) toolMeta = {};
    // 卡牌事件按 pos 插回原位(pos=事件发生时的 messages 数)。让重载历史不割裂。
    const pe = Array.isArray(state.personaEvents) ? state.personaEvents : [];
    function emitPersonaAt(atOrAfter, isTail) {
      for (let k = 0; k < pe.length; k++) {
        const ev = pe[k];
        if (isTail ? (ev.pos < atOrAfter) : (ev.pos !== atOrAfter)) continue;
        if (ev.kind === "equip" && ev.card) addChatItem({ type: "persona_equip", card: ev.card, time: "" });
        else if (ev.kind === "unequip") addChatItem({ type: "system", text: bt("personaUnequipped") + (ev.name || ""), time: "" });
        else if (ev.kind === "card_creator_intro") addChatItem({ type: "card_creator_intro", time: "" });
      }
    }
    // 预扫 tool_result：tool_use 在 assistant 消息、result 在后续 user 消息，需提前建映射
    // 才能在还原选择卡/方案卡时拿到结果（选项/快照）。
    const resultById = {};
    for (let ri = 0; ri < state.messages.length; ri++) {
      const rc = state.messages[ri].content;
      if (!Array.isArray(rc)) continue;
      for (let rj = 0; rj < rc.length; rj++) {
        if (rc[rj].type === "tool_result") {
          resultById[rc[rj].tool_use_id] = { content: rc[rj].content, is_error: !!rc[rj].is_error };
        }
      }
    }
    // 预扫:每个产物最后一次被 write/append/edit 改的 tool_use id → rerender 只在最后一次
    // 续一张成品卡(与实时 chat:done 的一张对齐,不刷一堆)。
    const lastDirtyArtifactId = {};
    const writtenArtifacts = {}; // write/append 写过的 path=产物;没 present 时兜底补首卡
    const presentedArtifacts = {}; // 整篇 present_artifact 过的 path → 别再兜底补首卡(present 会出卡,否则重复)
    const presentedArtifactNames = {}; // path 可能一边相对一边绝对,basename 去重防重复卡
    for (let di = 0; di < state.messages.length; di++) {
      const dc = state.messages[di].content;
      if (!Array.isArray(dc)) continue;
      for (let dj = 0; dj < dc.length; dj++) {
        const db = dc[dj];
        const dbMutation = db.type === "tool_use" && fileMutationAction(db.name, db.input);
        if (dbMutation) {
          extractArtifactPaths(db.input).forEach(function (dap) {
            lastDirtyArtifactId[dap] = db.id;
            // 与实时 tool_end 同一门控:tmp/ 中间文件、非成品扩展名不记账,
            // 否则实时不进面板的文件切 session 重放后反而兜底冒出成品卡。
            if (dbMutation !== "edit" && isDeliverable(dap)) writtenArtifacts[dap] = true;
          });
        } else if (db.type === "tool_use" && isPresentArtifactTool(db.name)) {
          const pap = extractArtifactPath(db.input);
          const pres = resultById[db.id];
          const pp = presentArtifactAbsPath(pres && pres.content, pap);
          if (pp) {
            presentedArtifacts[pp] = true;
            presentedArtifactNames[basename(pp)] = true;
          }
        } else if (db.type === "tool_use" && shouldUseToolOutputAsArtifact(db.name)) {
          const gres = resultById[db.id];
          if (!(gres && gres.is_error)) {
            const gp = artifactPathFromToolOutput(gres && gres.content);
            if (gp && isDeliverable(gp)) {
              lastDirtyArtifactId[gp] = db.id;
              writtenArtifacts[gp] = true;
            }
          }
        }
      }
    }
    for (let mi = 0; mi < state.messages.length; mi++) {
      emitPersonaAt(mi, false); // 该消息之前发生的卡牌事件先插
      const m = state.messages[mi];
      const blocks = Array.isArray(m.content) ? m.content : [];
      if (m.role === "user") {
        // Always strip exact internal envelopes: sessions saved before the
        // persistence alignment still carry the engine-baked <turn_meta>
        // block on mid-turn steered messages, and without the filter the
        // hydrated bubble rendered the raw envelope after a restart.
        const utext = userMessageDisplayText(blocks, true);
        if (utext) {
          // pinvouTransfer 是展示层标记、不在 messages → rerender 从转交固定措辞还原品/悟样式
          const uitem2 = { type: "user", text: utext, time: "", messageIndex: mi };
          // A mid-turn steer injection is no admission (no timing record):
          // the conversation projection must not let it consume a lifecycle
          // record (phantom "interrupted"). New sessions mark it via the
          // steered-messages sidecar (pos + text verified); the envelope
          // check stays as the fallback for pre-alignment persisted sessions.
          if (isSteeredMessagePos(mi, utext) || (Array.isArray(blocks) ? blocks : []).some(function (b) {
            const t = String(b && b.type === "text" ? b.text : "").trim();
            return (t.indexOf("<turn_meta>") === 0 && t.endsWith("</turn_meta>")) || t === "<turn_meta_unchanged />";
          })) uitem2.steeredMidTurn = true;
          const scene = pinvouSceneForMessagePos(mi);
          if (scene) uitem2.pinvouScene = scene;
          if (textMatchesBtKey(utext, "reviewFillHeader")) uitem2.pinvouTransfer = "悟";
          else if (textMatchesBtKey(utext, "reviewFixHeader") || textMatchesBtKey(utext, "reviewAdoptHeader") || textMatchesBtKey(utext, "reviewAskHeader")) uitem2.pinvouTransfer = "品";
          addChatItem(uitem2);
        }
        // tool_result（只回填普通工具卡；选择卡/方案卡的结果已在 tool_use 处还原）
        for (let ci = 0; ci < blocks.length; ci++) {
          const c = blocks[ci];
          if (c.type !== "tool_result") continue;
          const tm = toolMeta[c.tool_use_id];
          if (tm) {
            // careful hook 拦截 → 还原 🛑 红卡(实时由 tool_end metadata 插,重载从文本反解)
            const blockedMd = parseCarefulBlocked(toolResultText(c.content));
            if (blockedMd) {
              updateToolItem(c.tool_use_id, toolResultDisplayContent(c.content), false); // 被拦=失败态,与实时一致
              addChatItem({ type: "careful_blocked", args: tm.args, metadata: blockedMd, time: "" });
            } else {
              // load_skill 同样脱敏：重载历史时也不还原 SKILL.md 全文，展开只见占位。
              const contentForCard = (tm.name === "load_skill")
                ? bt("skillContentHidden")
                : toolResultDisplayContent(c.content);
              updateToolItem(c.tool_use_id, contentForCard, !c.is_error);
            }
          }
        }
        continue;
      }
      if (m.role !== "assistant") continue;
      let textBuf = "";
      let planSnap = null, todosSnap = null, sawPlanTool = false;
      for (let bi = 0; bi < blocks.length; bi++) {
        const b = blocks[bi];
        if (b.type === "text") {
          textBuf += b.text;
        } else if (b.type === "thinking") {
          if (textBuf) {
            addChatItem({ type: "assistant", text: textBuf, html: renderMarkdown(textBuf), time: "", streaming: false });
            textBuf = "";
          }
          const reasoningText = String(b.thinking || b.text || "");
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
            const qs = (b.input && b.input.questions) || [];
            if (Array.isArray(qs) && qs.length) {
              const res = resultById[b.id];
              // 快照可能落在 turn 进行中（底座每次落盘）：tool_use 尚无对应
              // tool_result，不能按历史恢复为 submitted。跳过，等
              // chat:user_input_required 事件渲染可交互的 active 卡。
              if (!res) continue;
              addChatItem({
                type: "user_input", toolCallId: b.id, questions: qs,
                resolved: true, cardState: res.is_error ? "cancelled" : "submitted",
                restoredAnswers: parseUserAnswers(res.content, qs), time: "",
              });
            }
            continue;
          }
          // present_artifact → 还原成品卡(切会话不丢)。仅当工具成功时还原:
          // 失败的调用回退成普通工具卡(下方 default addChatItem)。
          if (isPresentArtifactTool(b.name)) {
            const pares = resultById[b.id];
            if (!(pares && pares.is_error)) {
              const rpp = presentArtifactAbsPath(pares && pares.content, b.input && b.input.path);
              const restoredCard = {
                type: "artifact_card",
                path: rpp,
                title: (b.input && b.input.title) || "",
                description: (b.input && b.input.description) || "",
                time: "",
                sessionId: state.activeSessionId,
              };
              if (!updatePresentedArtifact(restoredCard)) addChatItem(restoredCard);
              continue;
            }
          }
          // update_plan / checklist_write / todo_write → 收集快照，本条消息末尾还原方案卡
          if (PLAN_TOOLS.has(b.name)) {
            const snap = parsePlanSnapshot(resultById[b.id] && resultById[b.id].content);
            if (snap) {
              if (b.name === "update_plan") planSnap = snap; else todosSnap = snap;
            }
            sawPlanTool = true;
            continue;
          }
          addChatItem({ type: "tool", toolId: b.id, name: b.name, args: b.input, output: null, success: null, state: "pending" });
          if (shouldUseToolOutputAsArtifact(b.name)) {
            const gres2 = resultById[b.id];
            const gap = artifactPathFromToolOutput(gres2 && gres2.content);
            if (!(gres2 && gres2.is_error) && gap && isDeliverable(gap) && lastDirtyArtifactId[gap] === b.id && !presentedArtifacts[gap] && !presentedArtifactNames[basename(gap)]) {
              const gprev = findPresentedArtifact(gap);
              if (gprev) {
                const gcard = {
                  type: "artifact_card", path: gprev.path, title: gprev.title,
                  description: gprev.description, time: "", sessionId: state.activeSessionId,
                };
                if (!updatePresentedArtifact(gcard)) addChatItem(gcard);
              } else if (writtenArtifacts[gap]) {
                addChatItem({ type: "artifact_card", path: gap, title: basename(gap), description: "", time: "", sessionId: state.activeSessionId });
              }
            }
          }
          // 还原"自动续卡":File.write/File.edit 改的文件之前 present 过 → 续一张
          // 成品卡(与实时 tool_end 的自动续逻辑对齐,切会话不丢)。present 的卡按
          // 顺序在前(必须先 present 才进集合),此处 findPresentedArtifact 能命中。
          if (fileMutationAction(b.name, b.input)) {
            const wres = resultById[b.id];
            extractArtifactPaths(b.input).forEach(function (wap) {
              // 去重:同产物只在最后一次修改处补一张卡(与实时对齐)。
              if ((wres && wres.is_error) || lastDirtyArtifactId[wap] !== b.id) return;
              const wprev = findPresentedArtifact(wap);
              if (wprev) {
                const wcard = {
                  type: "artifact_card", path: wprev.path, title: wprev.title,
                  description: wprev.description, time: "", sessionId: state.activeSessionId,
                };
                if (!updatePresentedArtifact(wcard)) addChatItem(wcard);
              } else if (writtenArtifacts[wap] && !presentedArtifacts[wap] && !presentedArtifactNames[basename(wap)]) {
                // AI 写了产物但全程没 present_artifact → 兜底补首卡(与实时 chat:done 对齐)
                addChatItem({ type: "artifact_card", path: wap, title: basename(wap), description: "", time: "", sessionId: state.activeSessionId });
              }
            });
          }
        }
      }
      if (textBuf) {
        addChatItem({ type: "assistant", text: textBuf, html: renderMarkdown(textBuf), time: "", streaming: false });
      }
      // 本条 assistant 消息用过 plan 工具 → 还原一张只读历史方案卡
      if (sawPlanTool && (planSnap || todosSnap)) {
        const snaps = { plan: planSnap, todos: todosSnap };
        addChatItem({
          type: "plan_card", plan: planSnap, todos: todosSnap,
          planMarkdown: composePlanMarkdown(snaps),
          cardState: "frozen", resolved: true, statusLabel: bt("planHistorical"), time: "",
        });
      }
    }
    emitPersonaAt(state.messages.length, true); // 最后一条消息之后发生的卡牌事件(末尾加持/卸下)
  }

  const terminalFeature = installBridgeFeature("terminal", { state, notify, invoke, bt, runSyncOnSession, addChatItem });
  const updateToolItem = terminalFeature.updateToolItem;
  const isShellExecutionTool = terminalFeature.isShellExecutionTool;
  const mentionsShellTool = terminalFeature.mentionsShellTool;
  const scheduleShellPoll = terminalFeature.scheduleShellPoll;
  const scheduleShellNotify = terminalFeature.scheduleShellNotify;
  const markBackgroundToolItem = terminalFeature.markBackgroundToolItem;
  const finishBackgroundToolItem = terminalFeature.finishBackgroundToolItem;
  const appendToolItemOutput = terminalFeature.appendToolItemOutput;
  // 找最后一条匹配的 chat item（用于卡片状态机更新）
  function patchLastItem(pred, patch) {
    for (let i = state.chatItems.length - 1; i >= 0; i--) {
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
    const lines = [];
    const plan = snapshots && snapshots.plan;
    const todos = snapshots && snapshots.todos;
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
    return invoke("cancel_shell_task", { sessionId, taskId });
  }

  installBridgeFeature("chat-events", {
    state, listen, invoke, turnUsageDirty,
    sessionStates, renderMarkdown, bt,
    notify, onSessionEvent, runSyncOnSession,
    recordAuthoritySyncDiagnostic,
    authoritySyncBufferSnapshot,
    // 与历史重载路径共用同一信封判定（userMessageDisplayText 的 isInternalRuntimeEnvelopeText），
    // 避免 live/restore 两处守卫实现漂移。
    isInternalRuntimeUserMessage: isInternalRuntimeEnvelopeText,
    applyAuthoritativeModeState,
    addChatItem, addSystemItem,
    addAuthoritySyncNotice, timeStr,
    toolCallAlreadyStarted,
    toolCallAlreadyFinished,
    hasChatItemForTool,
    flushPendingTextBlock,
    flushAssistantMessageToHistory,
    resetPendingAssistant, flushQueued,
    isBusyFor, doSendFor,
    settleSteerCommitted,
    settleSteerDropped,
    captureSteerPositions,
    recordSteeredMessages,
    ensureSessionBufferLoaded,
    getBuffer, markRemoteTurn,
    reconcileRemoteTurn, saveWorkingSetTo,
    hydratedMessageKey,
    thinkingTool: function (...args) { return thinkingTool(...args); },
    thinkingIdle: function (...args) { return thinkingIdle(...args); },
    startThinking: function (...args) { return startThinking(...args); },
    stopThinking: function (...args) { return stopThinking(...args); },
    userMessageDisplayText,
    scheduleScheduledRunRefresh,
    handleMemoryWrite: function (...args) { return handleMemoryWrite(...args); },
    isPresentArtifactTool,
    artifactPathFromToolOutput,
    shouldUseToolOutputAsArtifact,
    presentArtifactAbsPath,
    extractArtifactPaths, fileMutationAction,
    markTurnDirtyArtifact,
    trackArtifact, untrackArtifact,
    findPresentedArtifact, isDeliverable,
    noteArtifactChange,
    persistMessagesFor,
    composePlanMarkdown,
    refreshHistoryList,
    isShellExecutionTool,
    mentionsShellTool,
    scheduleShellPoll,
    appendToolItemOutput,
    scheduleShellNotify,
    markBackgroundToolItem,
    patchLastItem,
    updatePresentedArtifact,
    updateToolItem,
    basename,
    hasUnresolvedItem,
    finishBackgroundToolItem,
    safeConsoleInfo,
    isScheduledRunSession,
    markScheduledInitialTurnTerminal,
    isAbsPath, normalizedPath,
    addOrMergePruneCompaction,
    toolResultDisplayContent,
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

  const monitorFeature = installBridgeFeature("monitor", { state, notify, invoke, bt, safeConsoleInfo, sessionStates });
  const startMonitorPolling = monitorFeature.startMonitorPolling;
  const stopMonitorPolling = monitorFeature.stopMonitorPolling;
  const clearMonitorStats = monitorFeature.clearMonitorStats;
  const pollBackendStatus = monitorFeature.pollBackendStatus;
  const settingsFeature = installBridgeFeature("settings", { state, notify, invoke, listen });
  const loadSettings = settingsFeature.loadSettings;
  const loadSelectedPet = settingsFeature.loadSelectedPet;
  const setSelectedPet = settingsFeature.setSelectedPet;
  const loadEffectiveModelConfig = settingsFeature.loadEffectiveModelConfig;
  const saveSettings = settingsFeature.saveSettings;
  const saveSettingsAndRestart = settingsFeature.saveSettingsAndRestart;
  const saveSearchSettings = settingsFeature.saveSearchSettings;
  const saveSearchSettingsAndRestart = settingsFeature.saveSearchSettingsAndRestart;
  const submitFeedback = settingsFeature.submitFeedback;
  const discoverLocalVllm = settingsFeature.discoverLocalVllm;
  const detectLocalVllmSetup = settingsFeature.detectLocalVllmSetup;
  const bootstrapLocalVllm = settingsFeature.bootstrapLocalVllm;
  const dismissVllmSetup = settingsFeature.dismissVllmSetup;
  const declineVllmSetup = settingsFeature.declineVllmSetup;
  const getEffectiveModelConfig = settingsFeature.getEffectiveModelConfig;
  const loadModels = settingsFeature.loadModels;
  const saveModel = settingsFeature.saveModel;
  const revealModelApiKey = settingsFeature.revealModelApiKey;
  const deleteModel = settingsFeature.deleteModel;
  const setActiveModel = settingsFeature.setActiveModel;
  const loadSessionModel = settingsFeature.loadSessionModel;
  const switchModel = settingsFeature.switchModel;
  const testModelConnection = settingsFeature.testModelConnection;
  const getImageInputCapability = settingsFeature.getImageInputCapability;
  const testImageInputCapability = settingsFeature.testImageInputCapability;
  const probeLocalServerKind = settingsFeature.probeLocalServerKind;
  const testSearchProvider = settingsFeature.testSearchProvider;

  const interactionFeature = installBridgeFeature("interaction", {
    state, invoke, notify, bt,
    addSystemItem, addAuthoritySyncNotice,
    addChatItem, timeStr,
    runSyncOnSession,
    modeStateEpochs, bumpModeStateEpoch,
    applyAuthoritativeModeState,
    currentDraftModeState,
    flushAssistantMessageToHistory,
    resetPendingAssistant,
    rerenderFromMessages,
    turnUsageDirty,
    sendMessage,
    sendMessageToSession,
    getBuffer,
    reconcileRemoteTurn,
    isBusyFor,
    markRemoteTurn,
    userMessageDisplayText,
    recordAuthoritySyncDiagnostic,
    authoritySyncBufferSnapshot,
    get currentStreamText() { return currentStreamText; },
    set currentStreamText(value) { currentStreamText = value; },
    get currentStreamId() { return currentStreamId; },
    set currentStreamId(value) { currentStreamId = value; },
    get itemIdSeq() { return itemIdSeq; },
    set itemIdSeq(value) { itemIdSeq = value; },
  });
  const refreshSuperPerm = interactionFeature.refreshSuperPerm;
  const toggleSuperPerm = interactionFeature.toggleSuperPerm;
  const syncModeState = interactionFeature.syncModeState;
  const patchItemById = interactionFeature.patchItemById;
  const markResolved = interactionFeature.markResolved;
  const runOnSession = interactionFeature.runOnSession;
  const patchItemByIdFor = interactionFeature.patchItemByIdFor;
  const startThinking = interactionFeature.startThinking;
  const thinkingTool = interactionFeature.thinkingTool;
  const thinkingIdle = interactionFeature.thinkingIdle;
  const stopThinking = interactionFeature.stopThinking;
  const acceptPlan = interactionFeature.acceptPlan;
  const discardPlan = interactionFeature.discardPlan;
  const exitPlanToYolo = interactionFeature.exitPlanToYolo;
  const setPlanModeNext = interactionFeature.setPlanModeNext;
  const setDraftMode = interactionFeature.setDraftMode;
  const setModeLane = interactionFeature.setModeLane;
  const refreshModeDefaults = interactionFeature.refreshModeDefaults;
  const setMultiAgentMode = interactionFeature.setMultiAgentMode;
  const planStuckReplan = interactionFeature.planStuckReplan;
  const planStuckGo = interactionFeature.planStuckGo;
  const submitUserInput = interactionFeature.submitUserInput;
  const cancelUserInput = interactionFeature.cancelUserInput;
  const editLastTurn = interactionFeature.editLastTurn;
  const compactNow = interactionFeature.compactNow;

  const memoryFeature = installBridgeFeature("memory", { state, notify, invoke, bt, addSystemItem, runSyncOnSession, patchItemById, patchItemByIdFor, runOnSession, addChatItem, timeStr });
  const handleMemoryWrite = memoryFeature.handleMemoryWrite;
  const loadMemoryOverview = memoryFeature.loadMemoryOverview;
  const saveMemoryProfilePatch = memoryFeature.saveMemoryProfilePatch;
  const deleteMemoryPreference = memoryFeature.deleteMemoryPreference;
  const updateMemoryItem = memoryFeature.updateMemoryItem;
  const deleteMemoryItem = memoryFeature.deleteMemoryItem;
  const archiveRecentWorkMemory = memoryFeature.archiveRecentWorkMemory;
  const confirmMemoryCandidate = memoryFeature.confirmMemoryCandidate;
  const ignoreMemoryCandidate = memoryFeature.ignoreMemoryCandidate;
  const neverMemoryCandidate = memoryFeature.neverMemoryCandidate;
  const artifactsFeature = installBridgeFeature("artifacts", { state, notify, invoke, bt, addSystemItem, dialogOpen, basename, isDeliverable, isAbsPath, sessionStates, discardManagedAttachment });
  const artifactInfo = artifactsFeature.artifactInfo;
  const readArtifactText = artifactsFeature.readArtifactText;
  const writeArtifactText = artifactsFeature.writeArtifactText;
  const readArtifactImageB64 = artifactsFeature.readArtifactImageB64;
  const readArtifactThumbnail = artifactsFeature.readArtifactThumbnail;
  const renderArtifactVisual = artifactsFeature.renderArtifactVisual;
  const openContainingFolder = artifactsFeature.openContainingFolder;
  const revealSessionFolder = artifactsFeature.revealSessionFolder;
  const openScheduledTaskFolder = artifactsFeature.openScheduledTaskFolder;
  const openInSystem = artifactsFeature.openInSystem;
  const openArtifactExternal = artifactsFeature.openArtifactExternal;
  const downloadArtifact = artifactsFeature.downloadArtifact;
  const listDeliverableIndex = artifactsFeature.listDeliverableIndex;
  const openExternalUrl = artifactsFeature.openExternalUrl;
  const openUserExternalUrl = artifactsFeature.openUserExternalUrl;
  const addAttachmentByPath = artifactsFeature.addAttachmentByPath;
  const addPasteImage = artifactsFeature.addPasteImage;
  const removeAttachment = artifactsFeature.removeAttachment;
  const clearAttachments = artifactsFeature.clearAttachments;
  const pickAndAttach = artifactsFeature.pickAndAttach;
  const uploadDeviceFiles = artifactsFeature.uploadDeviceFiles;
  const adoptManagedAttachments = artifactsFeature.adoptManagedAttachments;
  const resolveConversationAttachment = artifactsFeature.resolveConversationAttachment;
  const openConversationAttachment = artifactsFeature.openConversationAttachment;
  const revealConversationAttachment = artifactsFeature.revealConversationAttachment;
  const personasFeature = installBridgeFeature("personas", { state, notify, invoke, listen, bt, isDefaultChatTitle, addSystemItem, addChatItem, timeStr, ensureSession, runOnSession, personaPlaceholderTitles });
  const loadPersonas = personasFeature.loadPersonas;
  const getPersonas = personasFeature.getPersonas;
  const createPersona = personasFeature.createPersona;
  const updatePersona = personasFeature.updatePersona;
  const deletePersona = personasFeature.deletePersona;
  const equipPersona = personasFeature.equipPersona;
  const postCardCreatorIntro = personasFeature.postCardCreatorIntro;
  const unequipPersona = personasFeature.unequipPersona;
  const syncActivePersona = personasFeature.syncActivePersona;
  const mountCollection = personasFeature.mountCollection;
  const setCollectionEnabled = personasFeature.setCollectionEnabled;
  const removeCollection = personasFeature.removeCollection;
  const unmountCollection = personasFeature.unmountCollection;
  const mountRemoteCollection = personasFeature.mountRemoteCollection;
  const setRemoteCollectionEnabled = personasFeature.setRemoteCollectionEnabled;
  const removeRemoteCollection = personasFeature.removeRemoteCollection;
  const syncMountedCollection = personasFeature.syncMountedCollection;
  const updaterFeature = installBridgeFeature("updater", { state, notify, invoke, refreshHistoryList, listen, getBuffer, bt });
  const loadAppVersion = updaterFeature.loadAppVersion;
  const checkForUpdateSilently = updaterFeature.checkForUpdateSilently;
  const checkForUpdate = updaterFeature.checkForUpdate;
  const downloadAndInstallUpdate = updaterFeature.downloadAndInstallUpdate;
  const cancelUpdate = updaterFeature.cancelUpdate;
  const restartApp = updaterFeature.restartApp;
  const reportPendingUpdateResult = updaterFeature.reportPendingUpdateResult;
  const remoteControlFeature = installBridgeFeature("remote-control", {
    state,
    notify,
    invoke,
    listen,
    bt,
  });
  // 撕离窗口不是权威主 WebView，不注册桌面 RPC 代理。
  if (!isDetachedWindow) {
    Promise.resolve(remoteControlFeature.startDesktopProxy()).catch(function (error) {
      console.error("[WebAccess] desktop proxy startup failed", error);
    });
  }
  const refreshRemoteControlStatus = remoteControlFeature.refreshRemoteControlStatus;
  const startRemoteControl = remoteControlFeature.startRemoteControl;
  const stopRemoteControl = remoteControlFeature.stopRemoteControl;
  const refreshRemoteControlQr = remoteControlFeature.refreshRemoteControlQr;
  const dependenciesFeature = installBridgeFeature("dependencies", { state, notify, invoke, listen, bt });
  const checkDependencies = dependenciesFeature.checkDependencies;
  const installDependencies = dependenciesFeature.installDependencies;
  const voiceFeature = installBridgeFeature("voice", { state, notify, invoke, bt });
  const startVoiceInput = voiceFeature.startVoiceInput;
  const installVoiceAsr = voiceFeature.installVoiceAsr;
  const cancelVoiceAsrSetup = voiceFeature.cancelVoiceAsrSetup;
  const closeVoiceAsrSetup = voiceFeature.closeVoiceAsrSetup;
  const cancelVoiceInput = voiceFeature.cancelVoiceInput;
  const clearVoiceInput = voiceFeature.clearVoiceInput;
  const appendVoiceText = voiceFeature.appendVoiceText;
  const runVoiceInputDebugAssertions = voiceFeature.runVoiceInputDebugAssertions;
  const knowledgeModelFeature = installBridgeFeature("knowledge-model", { state, notify, invoke, listen });
  const downloadKbModel = knowledgeModelFeature.downloadKbModel;
  const cancelKbModel = knowledgeModelFeature.cancelKbModel;

  const multiAgentFeature = installBridgeFeature("multiagent", { state, notify, invoke, listen });
  const listMultiAgentSubagents = multiAgentFeature.listSubagentTranscripts;
  const readMultiAgentSubagent = multiAgentFeature.readSubagentTranscript;
  async function pickFiles() {
    if (!dialogOpen) return [];
    const selected = await dialogOpen({ multiple: true });
    if (!selected) return [];
    return Array.isArray(selected) ? selected : [selected];
  }
  async function pickFolder() {
    if (!dialogOpen) return null;
    const selected = await dialogOpen({ directory: true, multiple: false, title: bt("pickFolderTitle") });
    if (!selected) return null;
    return Array.isArray(selected) ? (selected[0] || null) : selected;
  }
  async function pickFolders() {
    if (!dialogOpen) return [];
    const selected = await dialogOpen({ directory: true, multiple: true, title: bt("kbPickFolderTitle") });
    if (!selected) return [];
    return Array.isArray(selected) ? selected : [selected];
  }
  async function pickFeedbackFiles() {
    if (!dialogOpen) return [];
    const selected = await dialogOpen({
      multiple: true,
      filters: [{ name: bt("fileMediaFilterName"), extensions: ["png", "jpg", "jpeg", "gif", "webp", "mp4", "mov", "webm"] }],
    });
    if (!selected) return [];
    return Array.isArray(selected) ? selected : [selected];
  }
  // ── Init ─────────────────────────────────────────────────────────
  async function init() {
    if (initPromise) return initPromise;
    initPromise = (async function () {
    startupMark("bridge:init_start");
    // Populate the global Scheduled unread summary without requiring the user
    // to visit the Scheduled page first. This stays off the startup critical path.
    if (!isDetachedWindow) {
      loadScheduledTasks().catch(function () {}).then(function () {
        loadScheduledTaskRecentRuns().catch(function () {});
      });
    }
    startupMark("bridge:monitor_polling_deferred", "starts when monitor view becomes active");
    // 启动加载各自写互不重叠的状态片、彼此无数据依赖(每个 loader 自吞 invoke
    // 错误并落兜底值),串行 await 只会把 8 个 loader 的 IPC 往返叠进首屏延迟
    // (refreshHistoryList 含 2 次 invoke,实际 9 次往返)——并行后往返宽度收敛
    // 为 1。分离会话绑定与 enterDraft 必须等本组完成后再走。
    const needsSessionRuntime = !isDetachedWindow || detachedWindowKind === "session";
    const parallelLoads = [
      startupAwait("bridge:load_platform_capabilities", loadPlatformCapabilities),
      startupAwait("bridge:load_settings", loadSettings),
      startupAwait("bridge:load_selected_pet", loadSelectedPet),
      startupAwait("bridge:load_effective_model", loadEffectiveModelConfig),
      startupAwait("bridge:load_app_version", loadAppVersion),
      startupAwait("bridge:load_models", loadModels),
      startupAwait("bridge:refresh_history", refreshHistoryList)
    ];
    if (needsSessionRuntime) {
      parallelLoads.push(startupAwait("bridge:refresh_super_permission", refreshSuperPerm));
    }
    await Promise.all(parallelLoads);
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
    if (needsSessionRuntime) {
      // lane 全局默认（work/design/code）是草稿态 mode chip 的事实源，启动即拉取。
      startupAwait("bridge:refresh_mode_defaults", refreshModeDefaults);
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
    notify();
    startupMark("bridge:init_done");
    if (window.__PINVOU_STARTUP__) window.__PINVOU_STARTUP__.flush();
    })();
    return initPromise;
  }

  // ── Expose API ───────────────────────────────────────────────────
  window.TauriBridge = {
    available: true,
    lifecycle: { init },
    state: {
      get: snapshotStateSlice,
      getMany: snapshotStateSlices,
      subscribe: subscribeStateSlice,
      subscribeMany: subscribeStateSlices,
    },
    platform: {
      refreshConnectorAuthGates,
      loadPlatformCapabilities,
    },
    chat: {
      sendMessage,
      sendMessageToSession,
      getComposerDraft,
      setComposerDraft,
      retryFirstTurn,
      prefillComposer,
      removeQueued,
      prioritizeQueued,
      editQueued,
      steer,
      interruptAndSend,
      interruptAndSendQueued,
      cancelGeneration,
      cancelShellTask,
    },
    voice: {
      startVoiceInput,
      installVoiceAsr,
      cancelVoiceAsrSetup,
      closeVoiceAsrSetup,
      cancelVoiceInput,
      clearVoiceInput,
      appendVoiceText,
      runVoiceInputDebugAssertions,
    },
    knowledge: {
      loadKnowledgeEmbedderAfterFirstFrame,
      downloadKbModel,
      cancelKbModel,
      mountCollection,
      setCollectionEnabled,
      removeCollection,
      unmountCollection,
      mountRemoteCollection,
      setRemoteCollectionEnabled,
      removeRemoteCollection,
      listCollections: function () { return invoke("kb_collection_list"); },
      kbModelStatus: function () { return invoke("kb_model_status"); },
    },
    scheduled: {
      loadScheduledTasks,
      readScheduledTask,
      loadScheduledTaskRuns,
      loadScheduledTaskRecentRuns,
      selectScheduledTask,
      refreshScheduledTaskData,
      clearScheduledTaskSelection,
      dismissScheduledTaskError,
      createScheduledTask,
      updateScheduledTask,
      pauseScheduledTask,
      resumeScheduledTask,
      toggleScheduledTaskPinned,
      deleteScheduledTask,
      runScheduledTaskNow,
      pickFolder,
      startScheduledTaskChat,
      confirmScheduledTaskDraft,
      clearScheduledTaskDraft,
      openScheduledRunChat,
      exitScheduledRunChat,
    },
    sessions: {
      createNewSession,
      switchToSession,
      deleteSession,
      renameSession,
      toggleSessionPinned,
      archiveSession,
      restoreArchivedSession,
    },
    monitor: {
      startMonitorPolling,
      stopMonitorPolling,
      clearMonitorStats,
    },
    settings: {
      setSelectedPet,
      saveSettings,
      saveSettingsAndRestart,
      saveSearchSettings,
      saveSearchSettingsAndRestart,
      testSearchProvider,
    },
    feedback: { submitFeedback },
    vllm: {
      discoverLocalVllm,
      detectLocalVllmSetup,
      bootstrapLocalVllm,
      dismissVllmSetup,
      declineVllmSetup,
    },
    models: {
      getEffectiveModelConfig,
      loadModels,
      saveModel,
      revealModelApiKey,
      deleteModel,
      setActiveModel,
      loadSessionModel,
      switchModel,
      testModelConnection,
      getImageInputCapability,
      testImageInputCapability,
      probeLocalServerKind,
    },
    interaction: { toggleSuperPerm,
      // modeState 权威读取（评审 P1 后纳入公开面：main.jsx 从 code 页切回
      // 工作/设计时拉一次实测值，避免 ChatView 挂载后显示旧 modeState）
    syncModeState,
      // Plan/YOLO
    acceptPlan,
    discardPlan,
    exitPlanToYolo,
    setPlanModeNext,
    setDraftMode,
    setModeLane,
    refreshModeDefaults,
    setMultiAgentMode,
    planStuckReplan,
    planStuckGo,
    // 用户交互
    submitUserInput,
    cancelUserInput,
    summonPinvou,
    inspectPinvou,
    resolvePinvouReview,
    dismissPinvouReview,
    // 编辑/压缩
    editLastTurn,
      compactNow,
    },
    rendering: { renderMarkdown },
    remoteControl: {
      startRemoteControl,
      stopRemoteControl,
      refreshRemoteControlQr,
      refreshRemoteControlStatus,
      getWebRelaySettings: remoteControlFeature.getWebRelaySettings,
      setWebRelayAddress: remoteControlFeature.setWebRelayAddress,
      resetWebRelayAddress: remoteControlFeature.resetWebRelayAddress,
    },
    artifacts: {
      artifactInfo,
      readArtifactText,
      writeArtifactText,
      readArtifactImageB64,
      readArtifactThumbnail,
      renderArtifactVisual,
      openContainingFolder,
      revealSessionFolder,
      openScheduledTaskFolder,
      openInSystem,
      openArtifactExternal,
      downloadArtifact,
      listDeliverableIndex,
      openExternalUrl,
      openUserExternalUrl,
    },
    attachments: {
      addAttachmentByPath,
      addPasteImage,
      removeAttachment,
      clearAttachments,
      pickAndAttach,
      uploadDeviceFiles,
      resolveConversationAttachment,
      openConversationAttachment,
      revealConversationAttachment,
    },
    resolutions: { markResolved },
    multiAgent: {
      listSubagentTranscripts: listMultiAgentSubagents,
      readSubagentTranscript: readMultiAgentSubagent,
    },
    files: {
      pickFiles,
      pickFolders,
      pickFeedbackFiles,
    },
    personas: {
      loadPersonas,
      getPersonas,
      readPersonaBody: function (id) { return invoke("read_persona_body", { personaId: id }); },
      equipPersona,
      unequipPersona,
      postCardCreatorIntro,
      createPersona,
      updatePersona,
      deletePersona,
    },
    memory: {
      loadMemoryOverview,
      saveMemoryProfilePatch,
      deleteMemoryPreference,
      updateMemoryItem,
      deleteMemoryItem,
      archiveRecentWorkMemory,
      confirmMemoryCandidate,
      ignoreMemoryCandidate,
      neverMemoryCandidate,
    },
    updater: {
      checkForUpdate,
      downloadAndInstallUpdate,
      cancelUpdate,
      restartApp,
    },
    dependencies: {
      checkDependencies,
      installDependencies,
    },
  };

  // Auto-init after DOM ready
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    setTimeout(init, 0);
  }
})();
