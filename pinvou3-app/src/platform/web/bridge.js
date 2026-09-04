/**
 * tauri-bridge.js — Tauri 后端通信桥
 *
 * 封装所有 invoke/listen，维护前端状态，通过 pub/sub 推给 React。
 * 浏览器预览时（无 window.__TAURI__）自动降级。
 */
(function () {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic script; strict mode is part of the payload
  "use strict";

  if (!window.PinvouPlatform || (window.PinvouPlatform.kind !== "web" && window.PinvouPlatform.isWeb !== true)) return;

  const TAURI = window.__TAURI__;
  if (!TAURI) {
    console.warn("[TauriBridge] Tauri not available — browser preview mode");
    window.TauriBridge = {
      available: false,
      getState: function () { return {}; },
    };
    return;
  }

  const { invoke } = TAURI.core;
  const invokeWithRequestId = typeof TAURI.core.invokeWithRequestId === "function"
    ? TAURI.core.invokeWithRequestId
    : function (command, args) { return invoke(command, args); };
  const { listen } = TAURI.event;
  const dialogOpen = TAURI.dialog?.open;
  const PLATFORM = window.PinvouPlatform || { kind: "desktop", capabilities: {} };
  const IS_WEB = PLATFORM.kind === "web" || PLATFORM.isWeb === true;
  function hasCapability(name) {
    if (IS_WEB && typeof PLATFORM.can === "function") return PLATFORM.can(name) === true;
    if (IS_WEB) return !!(PLATFORM.capabilities && PLATFORM.capabilities[name] === true);
    return !PLATFORM.capabilities || PLATFORM.capabilities[name] !== false;
  }
  function canInvoke(command) {
    return !IS_WEB || (typeof PLATFORM.canInvoke === "function" && PLATFORM.canInvoke(command) === true);
  }
  const WEB_CAPABILITIES_WAIT_TIMEOUT_MS = 10_000;
  function webInvokeCapabilitiesReady() {
    if (!IS_WEB) return true;
    if (typeof PLATFORM.areInvokeCapabilitiesReady === "function") {
      return PLATFORM.areInvokeCapabilitiesReady() === true;
    }
    return !!(window.PinvouWebClient && window.PinvouWebClient.desktopCapabilitiesReady === true);
  }
  function webConnectionState() {
    if (typeof PLATFORM.getConnectionState !== "function") return null;
    try {
      return PLATFORM.getConnectionState();
    } catch {
      return null;
    }
  }
  function webInvokeCapabilitiesUnavailable(connection) {
    return !!(connection && connection.desktop_online === false && connection.status !== "connecting");
  }
  function unavailableWebCapabilitiesError() {
    const error = new Error("desktop disconnected before command capabilities were ready");
    error.code = "desktop_capabilities_unavailable";
    return error;
  }
  function waitForWebInvokeCapabilities() {
    if (webInvokeCapabilitiesReady()) return Promise.resolve();
    if (webInvokeCapabilitiesUnavailable(webConnectionState())) {
      return Promise.reject(unavailableWebCapabilitiesError());
    }
    return new Promise(function (resolve, reject) {
      let settled = false;
      let timeout = null;
      function finish(error) {
        if (settled) return;
        settled = true;
        if (timeout != null) clearTimeout(timeout);
        window.removeEventListener("pinvou:web-capabilities", onCapabilities);
        window.removeEventListener("pinvou:web-connection", onConnection);
        if (error) reject(error); else resolve();
      }
      function onCapabilities() {
        if (webInvokeCapabilitiesReady()) finish();
      }
      function onConnection(event) {
        const connection = event && event.detail;
        if (webInvokeCapabilitiesUnavailable(connection)) finish(unavailableWebCapabilitiesError());
      }
      window.addEventListener("pinvou:web-capabilities", onCapabilities);
      window.addEventListener("pinvou:web-connection", onConnection);
      timeout = setTimeout(function () {
        const error = new Error("desktop command capability snapshot timed out");
        error.code = "desktop_capabilities_timeout";
        finish(error);
      }, WEB_CAPABILITIES_WAIT_TIMEOUT_MS);
      // Close the race where the snapshot arrives between the initial check
      // and listener registration, and the equivalent race where a disconnect
      // happens before its event listener is attached.
      if (webInvokeCapabilitiesReady()) finish();
      else if (webInvokeCapabilitiesUnavailable(webConnectionState())) {
        finish(unavailableWebCapabilitiesError());
      }
    });
  }
  function normalizeVoiceMode(mode) {
    return mode === "task" ? "task" : mode === "edit" ? "edit" : "dictation";
  }
  function webRequestId(prefix) {
    if (window.crypto && typeof window.crypto.randomUUID === "function") {
      return prefix + "_" + window.crypto.randomUUID(); // safari14-ok: guarded above
    }
    // eslint-disable-next-line sonarjs/pseudo-random -- not security-sensitive: only generates request dedup IDs; collisions are safely retryable
    return prefix + "_" + Date.now().toString(36) + "_" + Math.random().toString(36).slice(2);
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
  // 最末级 fallback 必须自带 escapeHtml：远程 Web 部署缓存错配/资源缺失导致共享脚本未加载时，
  // renderMarkdown 仍会被 ChatView.jsx 的 dangerouslySetInnerHTML 消费，原文返回即 fail-open。
  // 因此 escapeHtml 作为安全原语保留在本文件（不依赖任何外部脚本），仅 marked.parse+sanitize
  // 这段较重的兜底被抽到共享文件。
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
  if (isPetWindow) {
    window.TauriBridge = {
      available: false,
      renderMarkdown,
    };
    return;
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
    depsInstalling: false,    // 一键安装进行中(pkexec apt)
    depsInstallError: null,   // 安装失败原因(apt stderr 透传/取消/pkexec 不可用)
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
      progress: null,     // { stage:'ffmpeg'|'model'|'done', downloaded, total }
      error: null,
    },
    // 知识库 embedding 模型按需下载引导（知识库页未装模型时显 gate）
    kbModelSetup: {
      downloading: false, // 下载/部署中
      startupLoading: false,
      startupReady: null,
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
  let webInitRetryArmed = false;
  let webInitRetryHandler = null;
  // 卡片池 1078 张卡的前端缓存。只读,通过 getPersonas() 取引用,不走 notify 快照。
  let personaPoolCache = [];
  const SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY = "pinvou3-scheduled-task-template-sources-v1";
  const scheduledTaskTemplateSources = loadScheduledTaskTemplateSources();

  // internal streaming state
  let currentStreamText = "";
  let currentStreamId = 0;
  let pendingAssistantText = "";
  let pendingAssistantBlocks = [];
  let itemIdSeq = 0;
  let toolMeta = {};       // id → { name, args }
  const shellPollState = Object.create(null); // session_id → { timer, inFlight, waitBudget }
  // 上下文行口径保护：TurnComplete 的 usage.input_tokens 是本轮所有请求的累加
  // （计费口径）。只有单请求的"干净轮"该值才等于当前上下文占用；本轮一旦出现
  // 工具调用/重试/压缩（= 多请求），就跳过这次 tokens 更新，保留上一个准确值。
  const turnUsageDirty = {};  // session_id → bool
  let monitorIntervalId = null;
  let monitorPollInFlight = false;
  let gpuUtilHistory = [];
  // 0 = no real max_model_len received yet from get_backend_status or the
  // monitor snapshot; write state.tokens.max back only for real values
  // (both assignment sites are truthiness-guarded).
  let maxModelLen = 0;
  // 监控页「清除统计」基准点：vLLM 的几个累计 counter（TTFT/TPOT/tokens/prefix
  // cache）无法真正清零（它们跟随远端 vLLM 进程生命周期，归零要重启共享进程）。
  // 改为记一个基准快照，显示值 = 当前 counter − 基准。换模型 / vLLM 重启 → counter
  // 倒退到小于基准，自动判定基准失效并丢弃，回落到生命周期累计值。持久化到
  // localStorage，关掉应用再开仍保持「自某时起」的统计。
  const MONITOR_BASELINE_KEY = "pinvou3.monitorStatsBaseline.self";
  let monitorBaseline = null;
  try {
    const _mb = localStorage.getItem(MONITOR_BASELINE_KEY);
    if (_mb) monitorBaseline = JSON.parse(_mb);
  } catch { monitorBaseline = null; }
  let attachIdSeq = 0;
  let scheduledTaskSelectionGeneration = 0;
  const scheduledTaskRequestTokens = { tasks: 0, detail: 0, runs: 0 };
  let scheduledTaskRefreshInFlight = null;
  let scheduledRecentRunsRequestToken = 0;
  let scheduledRunEventRefreshTimer = null;
  const scheduledTaskPendingLoads = Object.create(null);
  const scheduledTaskAutoCreateInFlight = Object.create(null);
  let sessionSwitchRequestToken = 0;
  let pendingCapabilitySessionSwitch = null;
  // 能力快照迟到时的切换宽限：等待期间不向用户报错，快照到达即自动重试；
  // 宽限期内快照仍未到达才报错并按失败收口（见 switchToSessionInternal 的 catch）。
  const WEB_CAPABILITY_SWITCH_RETRY_GRACE_MS = 5000;
  // modeState 读取请求序号（评审 P1，定义前置供 syncSessionPresentationState
  // 与权威写回收敛点共用）：任何权威 modeState 写回（invoke 返回 / 事件负载 /
  // 会话切换状态恢复）都必须 bump 它，作废在途 syncModeState 的旧读取——
  // 否则旧读返回时序号未变、校验通过，把刚写回的权威值覆盖回去。
  let modeSyncSeq = 0;

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
      attachStillUploading: "⚠️ Attachment still uploading, try again shortly",
      deviceUploadTooLarge: name => `⚠️ ${name} exceeds the 20 MB attachment limit`,
      archiveTooManyEntries: "the archive contains more than 50 entries and cannot be attached",
      archiveExpandedTooLarge: "the archive expands beyond 100 MB and cannot be attached",
      archiveUnsafeEntry: "the archive contains unsafe links or paths and cannot be attached",
      deviceUploadEmpty: name => `⚠️ ${name} is empty and cannot be attached`,
      deviceUploadFailed: "⚠️ Upload failed: ",
      deviceUploadDigestInvalid: "the attachment integrity digest was invalid. Try again.",
      deviceUploadIntegrityMismatch: "the attachment content was corrupted in transit. Upload it again.",
      turnAlreadyInProgress: "⚠️ This chat is already processing a turn. The duplicate send was not executed.",
      compactStart: "⏳ Compacting context", compactDone: "✓ Context compacted", compactFail: "⚠️ Compaction failed", compactAuto: " (auto)",
      compactPruneMerged: "Auto-compaction: tool-result cleanup, messages unchanged",
      compactInactive: "The session engine is not running yet. Send a message before compacting the context",
      gpuUnavailable: "GPU info unavailable",
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
      sessionChunkInvalid: "The desktop app returned an invalid session chunk",
      sessionChunkChanged: "Session data changed while reading. Please try again",
      sessionChunkOverflow: "Session chunk exceeds the declared length",
      sessionChunkEarlyEnd: "Session chunks ended prematurely",
      sessionChunkNoProgress: "Session chunks made no progress",
      scheduledDraftInvalid: "The scheduled task draft is missing a name, task description, or schedule",
      scheduledCreateFailed: "Failed to create scheduled task: ",
      scheduledTaskFallbackName: "Scheduled task",
      scheduledActionBusy: "Another scheduled task operation is still in progress",
      scheduledCreateNoId: "Failed to create scheduled task: the backend returned no task ID",
      scheduledChatPrefill: "I'd like to create a scheduled task: ",
      runNoSession: "This run has no session to open",
      sessionDataInvalid: "Invalid session data",
      skillContentHidden: "(Skill loaded; content hidden)",
      turnSyncRejected: "This session is syncing a turn completed elsewhere. Please try again shortly",
      targetSessionMissing: "Target session does not exist",
      replyContentEmpty: "Reply content is empty",
      targetSessionSyncing: "The target session is still syncing a turn completed elsewhere",
      sessionIdMissing: "The desktop app returned no new session ID",
      turnSyncRetry: "⚠️ This session is still syncing a turn completed elsewhere. Please try again shortly",
      pinvouNeedSession: "Start a chat first, then summon Pinvou for review.",
      remoteDoneUnsynced: "⚠️ The chat finished on the desktop, but the authoritative record is not synced yet. Retry after reconnecting.",
      unknownReason: "unknown reason",
      materialsAdded: (count, names) => "✅ Added " + count + " materials to run materials: " + names.join(", "),
      folderPickerUnavailable: "The folder picker cannot be opened in this environment",
      pickFolderTitle: "Choose a working directory",
      kbPickFolderTitle: "Choose a folder to import into the knowledge base",
      gateApproveFailed: "⚠️ Approval failed: ",
      gateRejectFailed: "⚠️ Rejection failed: ",
      roleRetried: (roleId, result) => "🔄 Rerunning " + roleId + ": " + result,
      roleRetryFailed: "⚠️ Rerun failed: ",
      metricNotApplicable: "N/A", metricUnavailable: "Not available",
      targetKindRemote: "Remote model",
      targetKindLocal: "Local model",
      targetKindInvalid: "Invalid configuration",
      betaTag: " (Beta)",
      memoryWriteFailed: "Failed to write memory: ",
      memoryIgnoreFailed: "Failed to ignore memory: ",
      memoryNeverFailed: "Failed to update the never-ask setting: ",
      planTicketExpired: "⚠️ The plan ticket has expired. Regenerate the plan before running it",
      downloadLimitSuffix: size => " (current file " + size + " MiB)",
      downloadLimitError: suffix => "Remote artifact downloads are limited to 256 MiB" + suffix + ". Open the file directly on the desktop.",
      downloadUnsupported: "⚠️ This desktop version does not support remote artifact download. Update the desktop app and try again.",
      downloadFailed: "⚠️ Artifact download failed: ",
      downloadNotEnabled: "Remote artifact download is not enabled in this environment",
      artifactMissing: "The artifact does not exist or has been removed",
      artifactSizeInvalid: "The desktop app returned an invalid artifact size. Download stopped",
      artifactChunkInvalid: "The desktop app returned an invalid artifact chunk. Download stopped",
      artifactChanged: "The artifact changed during download. Please try again",
      artifactOverflow: "The desktop app sent more artifact data than declared. Download stopped",
      artifactIncomplete: "The artifact download is incomplete. Please try again",
      attachPathUnavailable: "WebUI does not expose desktop attachment paths",
      attachDownloadUnsupported: "⚠️ This desktop version does not support remote attachment download. Update the desktop app and try again.",
      attachChunkInvalid: "The desktop app returned an invalid attachment chunk. Download stopped",
      attachChanged: "The attachment changed during download. Please try again",
      attachOverflow: "The desktop app sent more attachment data than declared. Download stopped",
      attachIncomplete: "The attachment download is incomplete. Please try again",
      attachNoData: "Attachment download returned no data",
      attachNoProgress: "Attachment download made no progress",
      artifactNoProgress: "Artifact download made no progress",
      newChatFallbackTitle: "New chat",
      echoOtherPrefix: "(Other) ",
      mountCollectionFailed: "Failed to mount knowledge collection: ",
      depsNotInstallable: "The missing items cannot be installed automatically. Install the offline components per the dependency notes, then re-check.",
      voicePermissionDenied: "Microphone access was denied. Allow this app to use the microphone in system settings, then try again.",
      voiceNoDevice: "No available microphone detected. Check that the recording device is enabled and not in use.",
      voiceDeviceTimeout: "Microphone detection timed out; no recording device found. Check the device connection and the system microphone settings, then try again.",
      voiceConstraintUnsupported: "Could not start recording: the current microphone or WebView does not support the required audio configuration. Try again; if it still fails, check microphone settings or update system components.",
      voiceEmptyResult: "No speech recognized. Move closer to the microphone and try again.",
      voiceContextMismatch: "Recognition finished, but the active session changed, so the result was not inserted.",
      voiceTimeout: "Voice input timed out. Please try again.",
      voiceRecognitionFailed: "Speech recognition failed. Please try again later.",
      voiceRecordingTooLong: "Recording is too long. Please shorten it and try again.",
      voiceAudioInvalid: "The recorded audio is invalid. Please record again.",
      voiceMicUnavailable: "The microphone is in use by another app. Close it or pick another microphone, then try again.",
      voiceInputFailed: "Voice input failed. Check the microphone and try again.",
      voiceCancelled: "Voice input cancelled",
      voiceTranscribing: "Recognizing speech…",
      voiceTooShort: "Recording too short. Please try again.",
      voiceWritten: "Voice text inserted into the input box",
      voiceTaskSent: "Voice task sent",
      voiceEditNoChange: "Voice edit made no changes; original text kept",
      voiceEditPostprocessFailed: "Voice edit processing failed; your input was not changed. Please try again.",
      voiceNeedDesktopAsr: "Install the speech recognition component on the desktop first, then use the microphone from the browser.",
      voiceRequestingPermission: "Requesting microphone permission…",
      voiceNoMicCapture: "This WebView does not support microphone capture.",
      voiceNoAudioRecording: "This WebView does not support audio recording.",
      voiceNeedSession: "Select or start a session before using voice input.",
      voiceAudioStartBlocked: "The browser did not allow audio capture to start. Click the microphone again.",
      voiceRecording: "Recording… press again to stop",
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
      attachStillUploading: "⚠️ 添付ファイルをアップロード中です。少し待ってから送信してください",
      deviceUploadTooLarge: name => `⚠️ ${name} は添付の上限 20 MB を超えています`,
      archiveTooManyEntries: "アーカイブに 50 個を超える項目が含まれているため添付できません",
      archiveExpandedTooLarge: "アーカイブの展開後サイズが 100 MB を超えるため添付できません",
      archiveUnsafeEntry: "アーカイブに安全でないリンクまたはパスが含まれているため添付できません",
      deviceUploadEmpty: name => `⚠️ ${name} は空のため添付できません`,
      deviceUploadFailed: "⚠️ アップロードに失敗: ",
      deviceUploadDigestInvalid: "添付ファイルの整合性ダイジェストが無効です。もう一度お試しください。",
      deviceUploadIntegrityMismatch: "添付ファイルの内容が転送中に破損しました。再度アップロードしてください。",
      turnAlreadyInProgress: "⚠️ このチャットでは別のターンを処理中です。重複した送信は実行されませんでした。",
      compactStart: "⏳ コンテキストを圧縮中", compactDone: "✓ コンテキスト圧縮完了", compactFail: "⚠️ 圧縮に失敗", compactAuto: "（自動）",
      compactPruneMerged: "自動圧縮: ツール結果を整理、メッセージ数は不変",
      compactInactive: "セッション Engine はまだ起動していません。メッセージを送信してからコンテキストを圧縮してください",
      gpuUnavailable: "GPU 情報を取得できません",
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
      sessionChunkInvalid: "デスクトップ側が無効なセッションチャンクを返しました",
      sessionChunkChanged: "読み込み中にセッションデータが変更されました。もう一度お試しください",
      sessionChunkOverflow: "セッションチャンクが宣言された長さを超えています",
      sessionChunkEarlyEnd: "セッションチャンクが途中で終了しました",
      sessionChunkNoProgress: "セッションチャンクが進みませんでした",
      scheduledDraftInvalid: "スケジュールタスクの下書きに名前・タスク説明・時間ルールのいずれかが不足しています",
      scheduledCreateFailed: "スケジュールタスクの作成に失敗：",
      scheduledTaskFallbackName: "スケジュールタスク",
      scheduledActionBusy: "別のスケジュールタスク操作がまだ進行中です",
      scheduledCreateNoId: "スケジュールタスクの作成に失敗：バックエンドがタスク ID を返しませんでした",
      scheduledChatPrefill: "スケジュールタスクを作成したいです：",
      runNoSession: "この実行記録には開けるセッションがありません",
      sessionDataInvalid: "セッションデータが無効です",
      skillContentHidden: "（スキルを読み込みました。内容は表示しません）",
      turnSyncRejected: "このセッションは別端末で完了したターンを同期中です。しばらくしてから再試行してください",
      targetSessionMissing: "対象のセッションが存在しません",
      replyContentEmpty: "返信内容が空です",
      targetSessionSyncing: "対象のセッションは別端末で完了したターンをまだ同期中です",
      sessionIdMissing: "デスクトップ側が新しいセッション ID を返しませんでした",
      turnSyncRetry: "⚠️ このセッションは別端末で完了したターンをまだ同期中です。しばらくしてから再試行してください",
      pinvouNeedSession: "先にチャットを開始してから Pinvou レビューを呼び出してください。",
      remoteDoneUnsynced: "⚠️ チャットはデスクトップ側で完了しましたが、正式な記録がまだ同期されていません。接続回復後に再試行できます。",
      unknownReason: "不明な原因",
      materialsAdded: (count, names) => "✅ 素材を " + count + " 件、配套材料に追加しました：" + names.join("、"),
      folderPickerUnavailable: "現在の環境ではフォルダー選択を開けません",
      pickFolderTitle: "作業ディレクトリを選択",
      kbPickFolderTitle: "知識ベースにインポートするフォルダーを選択",
      gateApproveFailed: "⚠️ 承認に失敗: ",
      gateRejectFailed: "⚠️ 差し戻しに失敗: ",
      roleRetried: (roleId, result) => "🔄 再実行 " + roleId + ": " + result,
      roleRetryFailed: "⚠️ 再実行に失敗: ",
      metricNotApplicable: "対象外", metricUnavailable: "未提供",
      targetKindRemote: "リモートモデル",
      targetKindLocal: "ローカルモデル",
      targetKindInvalid: "構成エラー",
      betaTag: " (ベータ版)",
      memoryWriteFailed: "メモリの書き込みに失敗：",
      memoryIgnoreFailed: "メモリの無視に失敗：",
      memoryNeverFailed: "「今後表示しない」の設定に失敗：",
      planTicketExpired: "⚠️ プランの認証情報が失効しました。プランを再生成してから実行してください",
      downloadLimitSuffix: size => "（現在のファイル " + size + " MiB）",
      downloadLimitError: suffix => "リモート制御での成果物ダウンロード上限は 256 MiB です" + suffix + "。デスクトップ側で直接開いてください。",
      downloadUnsupported: "⚠️ 現在のデスクトップ側はリモート制御による成果物ダウンロードに対応していません。デスクトップを更新して再試行してください。",
      downloadFailed: "⚠️ 成果物のダウンロードに失敗: ",
      downloadNotEnabled: "現在の環境ではリモート制御による成果物ダウンロードが有効になっていません",
      artifactMissing: "成果物が存在しないか、削除されています",
      artifactSizeInvalid: "デスクトップ側が無効な成果物サイズを返しました。ダウンロードを中止しました",
      artifactChunkInvalid: "デスクトップ側が無効な成果物チャンクを返しました。ダウンロードを中止しました",
      artifactChanged: "ダウンロード中に成果物が変更されました。もう一度お試しください",
      artifactOverflow: "デスクトップ側が宣言サイズを超える成果物データを返しました。ダウンロードを中止しました",
      artifactIncomplete: "成果物のダウンロードが不完全です。もう一度お試しください",
      attachPathUnavailable: "WebUI はデスクトップ側の添付ファイルパスを公開していません",
      attachDownloadUnsupported: "⚠️ 現在のデスクトップ側はリモート添付ファイルのダウンロードに対応していません。デスクトップを更新して再試行してください。",
      attachChunkInvalid: "デスクトップ側が無効な添付ファイルチャンクを返しました。ダウンロードを中止しました",
      attachChanged: "ダウンロード中に添付ファイルが変更されました。もう一度お試しください",
      attachOverflow: "デスクトップ側が宣言サイズを超える添付ファイルデータを返しました。ダウンロードを中止しました",
      attachIncomplete: "添付ファイルのダウンロードが不完全です。もう一度お試しください",
      attachNoData: "添付ファイルのダウンロードがデータを返しませんでした",
      attachNoProgress: "添付ファイルのダウンロードが進みませんでした",
      artifactNoProgress: "成果物のダウンロードが進みませんでした",
      newChatFallbackTitle: "新しいチャット",
      echoOtherPrefix: "(その他) ",
      mountCollectionFailed: "ナレッジセットのマウントに失敗: ",
      depsNotInstallable: "不足項目はワンクリックでインストールできません。依存関係の案内に従ってオフラインコンポーネントをインストールし、再検出してください。",
      voicePermissionDenied: "マイクへのアクセスが拒否されました。システム設定でこのアプリのマイク使用を許可してから再試行してください。",
      voiceNoDevice: "利用可能なマイクが検出されませんでした。録音デバイスが有効か、他で使用されていないか確認してください。",
      voiceDeviceTimeout: "マイク検出がタイムアウトし、録音デバイスが見つかりませんでした。デバイスの接続とシステムのマイク設定を確認して再試行してください。",
      voiceConstraintUnsupported: "録音を開始できません：現在のマイクまたは WebView が必要な録音設定に対応していません。再試行し、それでも失敗する場合はマイク設定を確認するかシステムコンポーネントを更新してください。",
      voiceEmptyResult: "音声を認識できませんでした。マイクに近づいて再試行してください。",
      voiceContextMismatch: "認識は完了しましたが、セッションが切り替わったため結果は自動入力されませんでした。",
      voiceTimeout: "音声入力がタイムアウトしました。もう一度お試しください。",
      voiceRecognitionFailed: "音声認識に失敗しました。しばらくしてから再試行してください。",
      voiceRecordingTooLong: "録音が長すぎます。短くして再試行してください。",
      voiceAudioInvalid: "録音データが無効です。もう一度録音してください。",
      voiceMicUnavailable: "マイクは他のアプリで使用中です。使用中のアプリを終了するか、別のマイクを選んでから再試行してください。",
      voiceInputFailed: "音声入力に失敗しました。マイクを確認して再試行してください。",
      voiceCancelled: "音声入力をキャンセルしました",
      voiceTranscribing: "音声を認識中…",
      voiceTooShort: "録音が短すぎます。もう一度お試しください。",
      voiceWritten: "音声を入力欄に書き込みました",
      voiceTaskSent: "音声タスクを送信しました",
      voiceEditNoChange: "音声編集による変更はありません。原文を保持しました",
      voiceEditPostprocessFailed: "音声編集の処理に失敗しました。入力は変更されていません。再試行してください。",
      voiceNeedDesktopAsr: "先にデスクトップ側で音声認識コンポーネントをインストールしてから、ブラウザーでマイクを使用してください。",
      voiceRequestingPermission: "マイクの権限を要求中…",
      voiceNoMicCapture: "現在の WebView はマイク入力に対応していません。",
      voiceNoAudioRecording: "現在の WebView は音声録音に対応していません。",
      voiceNeedSession: "音声入力を使う前に、セッションを選択または新規作成してください。",
      voiceAudioStartBlocked: "ブラウザーが音声キャプチャの開始を許可しませんでした。マイクをもう一度クリックしてください。",
      voiceRecording: "録音中です。もう一度押すと終了します",
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
      attachStillUploading: "⚠️ 附件还在上传,请稍后再发",
      deviceUploadTooLarge: name => `⚠️ ${name} 超过附件 20 MB 上限`,
      archiveTooManyEntries: "压缩包包含超过 50 个条目，无法添加",
      archiveExpandedTooLarge: "压缩包解压后超过 100 MB，无法添加",
      archiveUnsafeEntry: "压缩包包含不安全的链接或路径，无法添加",
      deviceUploadEmpty: name => `⚠️ ${name} 是空文件，无法添加`,
      deviceUploadFailed: "⚠️ 上传失败: ",
      deviceUploadDigestInvalid: "附件完整性校验值无效，请重试",
      deviceUploadIntegrityMismatch: "附件内容在传输中损坏，请重新上传",
      turnAlreadyInProgress: "⚠️ 当前会话已有一轮正在处理，本次重复发送未执行。",
      compactStart: "⏳ 正在压缩上下文", compactDone: "✓ 上下文压缩完成", compactFail: "⚠️ 压缩失败", compactAuto: "（自动）",
      compactPruneMerged: "自动压缩：已整理工具结果，消息数不变",
      compactInactive: "会话引擎尚未运行。请先发送一条消息，再压缩上下文",
      gpuUnavailable: "GPU 信息不可用",
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
      sessionChunkInvalid: "桌面端返回了无效的会话分块",
      sessionChunkChanged: "读取期间会话数据发生变化，请重试",
      sessionChunkOverflow: "会话分块超出声明长度",
      sessionChunkEarlyEnd: "会话分块提前结束",
      sessionChunkNoProgress: "会话分块没有前进",
      scheduledDraftInvalid: "定时任务草稿缺少名称、任务说明或时间规则",
      scheduledCreateFailed: "定时任务创建失败：",
      scheduledTaskFallbackName: "定时任务",
      scheduledActionBusy: "另一个定时任务操作仍在进行中",
      scheduledCreateNoId: "创建定时任务失败：后端未返回任务 ID",
      scheduledChatPrefill: "我想创建一个定时任务：",
      runNoSession: "该运行记录没有可打开的会话",
      sessionDataInvalid: "会话数据无效",
      skillContentHidden: "（技能已加载，内容不展示）",
      turnSyncRejected: "该会话正在同步另一端完成的回合，请稍后重试",
      targetSessionMissing: "目标会话不存在",
      replyContentEmpty: "回复内容为空",
      targetSessionSyncing: "目标会话仍在同步另一端完成的回合",
      sessionIdMissing: "桌面端未返回新会话 ID",
      turnSyncRetry: "⚠️ 该会话仍在同步另一端完成的回合，请稍后重试",
      pinvouNeedSession: "先开始一个对话,再召唤 Pinvou 检阅。",
      remoteDoneUnsynced: "⚠️ 对话已在桌面端完成，但权威记录暂未同步；恢复连接后可重试。",
      unknownReason: "未知原因",
      materialsAdded: (count, names) => "✅ 已添加 " + count + " 个素材到配套材料：" + names.join("、"),
      folderPickerUnavailable: "当前环境无法打开文件夹选择器",
      pickFolderTitle: "选择工作目录",
      kbPickFolderTitle: "选择要导入知识库的文件夹",
      gateApproveFailed: "⚠️ 通过失败: ",
      gateRejectFailed: "⚠️ 打回失败: ",
      roleRetried: (roleId, result) => "🔄 重跑 " + roleId + ": " + result,
      roleRetryFailed: "⚠️ 重跑失败: ",
      metricNotApplicable: "不适用", metricUnavailable: "未提供",
      targetKindRemote: "远端模型",
      targetKindLocal: "本地模型",
      targetKindInvalid: "配置异常",
      betaTag: " (内测版)",
      memoryWriteFailed: "记忆写入失败：",
      memoryIgnoreFailed: "忽略记忆失败：",
      memoryNeverFailed: "设置不再提示失败：",
      planTicketExpired: "⚠️ 方案凭证已失效，请重新生成方案后再执行",
      downloadLimitSuffix: size => "（当前文件 " + size + " MiB）",
      downloadLimitError: suffix => "远程控制单个产物下载上限为 256 MiB" + suffix + "，请在桌面端直接打开该文件。",
      downloadUnsupported: "⚠️ 当前桌面端不支持远程控制产物下载，请更新桌面端后重试。",
      downloadFailed: "⚠️ 产物下载失败: ",
      downloadNotEnabled: "当前环境未启用远程控制产物下载能力",
      artifactMissing: "产物不存在或已被移除",
      artifactSizeInvalid: "桌面端返回了无效的产物大小，已停止下载",
      artifactChunkInvalid: "桌面端返回了无效的产物分块，已停止下载",
      artifactChanged: "产物在下载期间发生变化，请重试",
      artifactOverflow: "桌面端返回的产物数据超过声明大小，已停止下载",
      artifactIncomplete: "产物下载不完整，请重试",
      attachPathUnavailable: "WebUI 无法访问桌面端附件路径",
      attachDownloadUnsupported: "⚠️ 当前桌面端不支持远程附件下载，请更新桌面端后重试。",
      attachChunkInvalid: "桌面端返回了无效的附件分块，已停止下载",
      attachChanged: "附件在下载期间发生变化，请重试",
      attachOverflow: "桌面端返回的附件数据超过声明大小，已停止下载",
      attachIncomplete: "附件下载不完整，请重试",
      attachNoData: "附件下载未返回数据",
      attachNoProgress: "附件下载没有进展",
      artifactNoProgress: "产物下载没有进展",
      newChatFallbackTitle: "新对话",
      echoOtherPrefix: "(其他) ",
      mountCollectionFailed: "挂载知识集失败: ",
      depsNotInstallable: "当前缺失项无法一键安装，请按依赖说明安装离线组件后重新检测。",
      voicePermissionDenied: "麦克风权限被拒绝，请在系统设置中允许本应用访问麦克风后重试。",
      voiceNoDevice: "未检测到可用麦克风，请检查录音设备是否启用或被占用。",
      voiceDeviceTimeout: "麦克风检测超时，未发现可用录音设备。请检查设备连接和系统麦克风设置后重试。",
      voiceConstraintUnsupported: "无法启动录音：当前麦克风或 WebView 不支持所需的录音配置。请重试；若仍失败，请检查麦克风设置或更新系统组件。",
      voiceEmptyResult: "未识别到语音内容，请靠近麦克风后重试。",
      voiceContextMismatch: "识别已完成，但当前会话已切换，结果未自动写入。",
      voiceTimeout: "本次语音输入超时，请重试。",
      voiceRecognitionFailed: "语音识别失败，请稍后重试。",
      voiceRecordingTooLong: "录音过长，请缩短后重试。",
      voiceAudioInvalid: "录音数据无效，请重新录制。",
      voiceMicUnavailable: "麦克风被其他应用占用，请关闭占用它的应用或更换麦克风后重试。",
      voiceInputFailed: "语音输入失败，请检查麦克风后重试。",
      voiceCancelled: "已取消语音输入",
      voiceTranscribing: "正在识别语音…",
      voiceTooShort: "录音时间过短，请重试。",
      voiceWritten: "语音已写入输入框",
      voiceTaskSent: "语音任务已发送",
      voiceEditNoChange: "语音编辑无变化，已保留原文",
      voiceEditPostprocessFailed: "语音编辑整理失败，当前输入未修改，请重试。",
      voiceNeedDesktopAsr: "请先在桌面端安装语音识别组件，再从浏览器使用麦克风。",
      voiceRequestingPermission: "正在请求麦克风权限…",
      voiceNoMicCapture: "当前 WebView 不支持麦克风采集。",
      voiceNoAudioRecording: "当前 WebView 不支持音频录制。",
      voiceNeedSession: "请先选择或新建一个会话，再使用语音输入。",
      voiceAudioStartBlocked: "浏览器未允许启动音频采集，请再次点击麦克风。",
      voiceRecording: "正在录音，再按一次结束",
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
  // 与 tauri 桥和 main.jsx 的同款判断保持一致。
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
  // Web 草稿首条消息的本地提交记录。内容只留在当前页面内，用固定 RPC ID
  // 支撑断线重发与人工重试；切走草稿后不会把待发内容泄漏到其他会话。
  const firstTurnSubmissions = Object.create(null);
  const authoritativeTranscriptSyncs = Object.create(null);
  let authoritySyncTraceSequence = 0;
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
  const scheduledRunSessionOwners = Object.create(null);
  const scheduledRunOpenInFlight = Object.create(null);
  const MAX_SCHEDULED_SESSION_BUFFERS = 64;
  const MAX_SCHEDULED_RUN_SESSION_OWNERS = 64;
  // All-session buffer cap: each sessionStates entry holds the full
  // messages+chatItems (heavy sessions run 1-4MB each); previously only
  // scheduled sessions had a 64-entry LRU — normal sessions stayed
  // resident forever once visited. Cap at 32: typical users actively
  // switch among single-digit counts, 32 × 1-4MB worst case is
  // ~32-128MB, and cold sessions beyond the cap have near-zero hit
  // rate; revisiting an evicted session goes through load_session disk
  // rehydration, costing one reload.
  const MAX_SESSION_BUFFERS = 32;
  // Unsent composer drafts are the one piece of a working set that cannot be
  // rebuilt from disk (this bridge never persists drafts; transcripts hold
  // committed content only): before a buffer is dropped, its draft moves to
  // this side table and every rebuild path restores it. The table is bounded
  // (256 entries, 1M chars per draft — 10x the composer input cap), and when
  // a bound would be exceeded the eviction is refused instead: the buffer
  // stays resident rather than silently losing user input. Real session
  // deletion (purgeSessionBuffer) invalidates stashed drafts so they never
  // flow back into a recycled session id.
  const MAX_EVICTED_SESSION_DRAFTS = 256;
  const MAX_EVICTED_SESSION_DRAFT_CHARS = 1000000;
  const evictedSessionDrafts = Object.create(null);
  // Returns true when the buffer's non-rehydratable state is safely retained
  // (or empty) and the caller may drop the buffer; false means eviction must
  // be skipped so the draft stays in the live buffer.
  function stashEvictedSessionDraft(id, buf) {
    if (!id || !buf) return true;
    const draft = String(buf.composerDraft || "");
    if (!draft) {
      delete evictedSessionDrafts[id]; // input was cleared; a stale stash must not resurrect it
      return true;
    }
    if (draft.length > MAX_EVICTED_SESSION_DRAFT_CHARS) return false;
    if (!evictedSessionDrafts[id]
        && Object.keys(evictedSessionDrafts).length >= MAX_EVICTED_SESSION_DRAFTS) return false;
    delete evictedSessionDrafts[id]; // re-stashing moves the entry to the table tail
    evictedSessionDrafts[id] = draft;
    return true;
  }
  function restoreEvictedSessionDraft(id, buf) {
    if (!id || !buf || buf.composerDraft) return;
    const draft = evictedSessionDrafts[id];
    if (draft) buf.composerDraft = draft;
  }
  let sessionBufferTouchClock = 0;
  let scheduledRunOwnerTouchClock = 0;
  let suppressNotify = false;
  // sessionId → true:标题当前是「卡牌占位名」(加卡时自动取的),可被首条用户消息覆盖。
  // 卡牌名只在「加了卡但还没开口」时当临时标题;一旦开始对话,对话内容更能区分同卡会话。
  // 内存态(不持久化):重启后丢标记仅影响「加卡→重启→才发首条消息」这一冷门路径。
  const personaPlaceholderTitles = {};
  const PINVOU_SCENE_EVENTS_STORAGE_PREFIX = "pinvou_scene_events_v1:";
  function normalizePinvouScene(scene) {
    scene = String(scene || "").trim();
    return /^(work:document-writing|work:personal-workbench|design:poster|design:data-visualization)$/.test(scene) ? scene : "";
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
        } catch { /* on localStorage write failure, fall back to remote data */ }
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
  function recordPinvouSceneForBufferMessage(sid, buffer, pos, scene) {
    scene = normalizePinvouScene(scene);
    if (!sid || !buffer || !scene) return;
    pos = Number(pos);
    if (!Number.isFinite(pos) || pos < 0) return;
    let events = normalizePinvouSceneEvents(buffer.pinvouSceneEvents)
      .filter(function (event) { return event.pos !== Math.floor(pos); });
    events.push({ pos: Math.floor(pos), scene });
    events = normalizePinvouSceneEvents(events);
    buffer.pinvouSceneEvents = events;
    savePinvouSceneEventsForSession(sid, events);
  }
  function pinvouSceneForMessagePos(pos) {
    const events = normalizePinvouSceneEvents(state.pinvouSceneEvents);
    for (let i = 0; i < events.length; i++) {
      if (events[i].pos === pos) return events[i].scene;
    }
    return "";
  }
  function freshBuffer() {
    return {
      messages: [], chatItems: [], composerDraft: "", turnTimeline: [], activeTurnTimelineId: null, personaEvents: [], pinvouReviews: [], pinvouSceneEvents: [], artifacts: [], busy: false, queued: [],
      loadedFromDisk: false,
      localTurnOwned: false,
      remoteTurnActive: false,
      remoteTerminalSeen: false,
      remoteAdmissionKeys: [],
      deferredRemoteUserEvent: null,
      remoteBaselineMessageCount: null,
      remoteBaselineTrusted: false,
      remoteExpectedAssistantKey: "",
      remoteCommittedRevision: "",
      sessionRevision: "",
      planSnapshot: { plan: null, todos: null },
      modeState: { mode: "yolo" },
      thinking: { active: false, phase: "thinking", toolName: "", startedAt: 0 },
      tokens: { input: 0, max: maxModelLen },
      activePersona: null, // 卡片池: 该 session 加持的专家面具(挂件用)
      mountedCollection: null, // 知识库: 该 session 挂载的知识集 id 或 null
      mountedCollections: [], // 多知识库挂载项 [{ collectionId, enabled }]
      mountedCollectionsRevision: 0,
      scheduledTaskDraft: null,
      scheduledRunSession: false,
      scheduledInitialTurnPhase: null,
      lastTouched: 0,

      stream: {
        currentStreamText: "", currentStreamId: 0, pendingAssistantText: "",
        pendingAssistantBlocks: [], itemIdSeq: 0, toolMeta: {},
      },
    };
  }
  function getBuffer(id) {
    if (!id) return null;
    if (!sessionStates[id]) {
      sessionStates[id] = freshBuffer();
      restoreEvictedSessionDraft(id, sessionStates[id]);
    }
    return touchSessionBuffer(id, sessionStates[id], id.indexOf("sched-") === 0);
  }
  function isProtectedScheduledBuffer(id, buf) {
    return id === state.activeSessionId ||
      !!buf.busy ||
      !!buf.remoteTurnActive ||
      buf.scheduledInitialTurnPhase === "active" ||
      !!(buf.queued && buf.queued.length) ||
      !!(state.scheduledRunContext && state.scheduledRunContext.sessionId === id) ||
      state.scheduledTaskCreationSessionId === id;
  }
  function pruneScheduledSessionBuffers(keepId) {
    const scheduledIds = Object.keys(sessionStates).filter(function (id) {
      return !!sessionStates[id].scheduledRunSession;
    });
    let overflow = scheduledIds.length - MAX_SCHEDULED_SESSION_BUFFERS;
    if (overflow <= 0) return;
    scheduledIds.sort(function (left, right) {
      const delta = (sessionStates[left].lastTouched || 0) - (sessionStates[right].lastTouched || 0);
      return delta || left.localeCompare(right);
    });
    for (let i = 0; i < scheduledIds.length && overflow > 0; i++) {
      const id = scheduledIds[i];
      const buf = sessionStates[id];
      if (!buf || id === keepId || isProtectedScheduledBuffer(id, buf)) continue;
      if (!stashEvictedSessionDraft(id, buf)) continue; // draft cannot be safely retained; keep the buffer
      delete sessionStates[id];
      delete turnUsageDirty[id];
      // personaPlaceholderTitles is lightweight session metadata (the marker
      // for placeholder titles that auto-rename may override); rehydration
      // never restores it, so capacity eviction must keep it and only real
      // session deletion (purgeSessionBuffer) cleans it.
      // The localStorage cache of scene events is the only recovery copy
      // when the sidecar save fails or we are offline
      // (savePinvouSceneEventsForSession intentionally swallows backend
      // failures; syncPinvouSceneEventsForSession replays from this
      // cache); capacity eviction must not delete the key — only real
      // session deletion (purgeSessionBuffer) cleans it.
      // The owner tombstone has its own bounded LRU and must outlive a
      // presentation-buffer eviction. Otherwise a stale `running` list row can
      // resurrect a run that already emitted chat:done.
      overflow -= 1;
    }
  }
  function touchSessionBuffer(id, buf, scheduled) {
    if (!buf) return null;
    if (scheduled) buf.scheduledRunSession = true;
    buf.lastTouched = ++sessionBufferTouchClock;
    if (buf.scheduledRunSession) pruneScheduledSessionBuffers(id);
    pruneSessionBuffers(id);
    return buf;
  }
  // All-session LRU: shares the scheduled eviction's protection
  // predicates (busy/queued/remote turns are never reclaimed); only
  // idle buffers are evicted. messages/chatItems rehydrate from disk;
  // non-rehydratable drafts such as composerDraft move to the
  // evictedSessionDrafts side table first and are restored on rebuild;
  // when the table cannot hold a draft the eviction is refused (see
  // stashEvictedSessionDraft).
  function pruneSessionBuffers(keepId) {
    const ids = Object.keys(sessionStates);
    let overflow = ids.length - MAX_SESSION_BUFFERS;
    if (overflow <= 0) return;
    ids.sort(function (left, right) {
      const delta = (sessionStates[left].lastTouched || 0) - (sessionStates[right].lastTouched || 0);
      return delta || left.localeCompare(right);
    });
    for (let i = 0; i < ids.length && overflow > 0; i++) {
      const id = ids[i];
      const buf = sessionStates[id];
      if (!buf || id === keepId || isProtectedScheduledBuffer(id, buf)) continue;
      if (!stashEvictedSessionDraft(id, buf)) continue; // draft cannot be safely retained; keep the buffer
      delete sessionStates[id];
      delete turnUsageDirty[id];
      // personaPlaceholderTitles survives capacity eviction (see the
      // comment at the scheduled eviction site); only purgeSessionBuffer
      // (real deletion) cleans it.
      // The scene cache key survives capacity eviction (the only offline
      // recovery copy), see the comment at the scheduled eviction site
      // above; only purgeSessionBuffer (real session deletion) cleans
      // it.
      overflow -= 1;
    }
  }
  function purgeSessionBuffer(id) {
    if (typeof id !== "string" || !id) return;
    delete sessionStates[id];
    // Real session deletion: any stashed draft is invalidated too and must not flow back into a rebuilt buffer with the same id.
    delete evictedSessionDrafts[id];
    delete turnUsageDirty[id];
    delete personaPlaceholderTitles[id];
    delete scheduledRunSessionOwners[id];
    // The scene-events localStorage key is cleaned together with session deletion, avoiding unbounded accumulation across historical sessions.
    if (window.localStorage) {
      try { window.localStorage.removeItem(PINVOU_SCENE_EVENTS_STORAGE_PREFIX + id); } catch { /* localStorage may be unavailable or full; the key is a cache and its loss is non-fatal */ }
    }
    if (state.scheduledRunContext && state.scheduledRunContext.sessionId === id) {
      state.scheduledRunContext = null;
    }
    if (state.scheduledTaskCreationSessionId === id) {
      state.scheduledTaskCreationSessionId = null;
    }
    if (state.activeSessionId === id) {
      state.activeSessionId = null;
      loadWorkingSetFrom(freshBuffer());
    }
  }
  function registerScheduledRunOwner(id, phase) {
    if (typeof id !== "string" || !id) return null;
    let owner = scheduledRunSessionOwners[id];
    if (!owner) owner = scheduledRunSessionOwners[id] = { phase: null, lastTouched: 0 };
    if (owner.phase !== "terminal" && phase) owner.phase = phase;
    owner.lastTouched = ++scheduledRunOwnerTouchClock;
    pruneScheduledRunSessionOwners();
    return owner;
  }
  function scheduledRunOwnerVisibleRank(id) {
    const runs = state.scheduledTaskRuns || [];
    for (let i = 0; i < runs.length; i++) {
      if (runs[i] && runs[i].sessionId === id) return i;
    }
    return -1;
  }
  function scheduledRunOwnerPriority(id) {
    if (id === state.activeSessionId ||
        (state.scheduledRunContext && state.scheduledRunContext.sessionId === id)) return 3;
    if (scheduledRunOwnerVisibleRank(id) >= 0) return 2;
    return 1;
  }
  function pruneScheduledRunSessionOwners() {
    const ids = Object.keys(scheduledRunSessionOwners);
    if (ids.length <= MAX_SCHEDULED_RUN_SESSION_OWNERS) return;
    ids.sort(function (left, right) {
      const priorityDelta = scheduledRunOwnerPriority(right) - scheduledRunOwnerPriority(left);
      if (priorityDelta) return priorityDelta;
      const leftVisibleRank = scheduledRunOwnerVisibleRank(left);
      const rightVisibleRank = scheduledRunOwnerVisibleRank(right);
      if (leftVisibleRank >= 0 || rightVisibleRank >= 0) {
        if (leftVisibleRank < 0) return 1;
        if (rightVisibleRank < 0) return -1;
        if (leftVisibleRank !== rightVisibleRank) return leftVisibleRank - rightVisibleRank;
      }
      const touchDelta = (scheduledRunSessionOwners[right].lastTouched || 0) -
        (scheduledRunSessionOwners[left].lastTouched || 0);
      return touchDelta || left.localeCompare(right);
    });
    for (let i = MAX_SCHEDULED_RUN_SESSION_OWNERS; i < ids.length; i++) {
      delete scheduledRunSessionOwners[ids[i]];
    }
  }
  function isScheduledRunTerminal(status) {
    const value = String(status || "").toLowerCase();
    return ["completed", "failed", "canceled"].includes(value);
  }
  function rememberScheduledRunOwner(run) {
    if (!run) return;
    const id = typeof run.sessionId === "string" ? run.sessionId.trim() : "";
    if (!id) return;
    const status = String(run.status || "").toLowerCase();
    const phase = isScheduledRunTerminal(status)
      ? "terminal"
      : (status === "queued" || status === "running" ? "active" : null);
    registerScheduledRunOwner(id, phase);
  }
  function scheduledRunBuffer(id) {
    const buf = getBuffer(id);
    if (!buf) return null;
    registerScheduledRunOwner(id, null);
    return touchSessionBuffer(id, buf, true);
  }
  function markScheduledInitialTurnActive(id) {
    const buf = scheduledRunBuffer(id);
    const owner = registerScheduledRunOwner(id, "active");
    if (!buf) return buf;
    if (buf.scheduledInitialTurnPhase === "terminal" || (owner && owner.phase === "terminal")) {
      buf.scheduledInitialTurnPhase = "terminal";
      buf.busy = false;
      if (state.activeSessionId === id) state.busy = false;
      return buf;
    }
    buf.scheduledInitialTurnPhase = "active";
    buf.busy = true;
    if (state.activeSessionId === id) state.busy = true;
    return buf;
  }
  function markScheduledInitialTurnTerminal(id) {
    const buf = scheduledRunBuffer(id);
    registerScheduledRunOwner(id, "terminal");
    if (!buf || buf.scheduledInitialTurnPhase === "terminal") return buf;
    if (buf.scheduledInitialTurnPhase !== "active") {
      buf.scheduledInitialTurnPhase = "active";
    }
    buf.scheduledInitialTurnPhase = "terminal";
    return buf;
  }
  function beginScheduledOpenActivation(id) {
    const previous = sessionStates[id] || null;
    const snapshot = {
      id,
      existed: !!previous,
      previousPhase: previous && previous.scheduledInitialTurnPhase,
      previousBusy: previous ? !!previous.busy : false,
      previousStateBusy: state.activeSessionId === id ? !!state.busy : null,
    };
    const buf = markScheduledInitialTurnActive(id);
    snapshot.buffer = buf;
    snapshot.activationTouch = buf && buf.lastTouched;
    snapshot.changed = !!buf && (
      !snapshot.existed ||
      snapshot.previousPhase !== buf.scheduledInitialTurnPhase ||
      snapshot.previousBusy !== !!buf.busy
    );
    return snapshot;
  }
  function rollbackScheduledOpenActivation(snapshot) {
    if (!snapshot || !snapshot.changed) return;
    const current = sessionStates[snapshot.id];
    if (!current || current !== snapshot.buffer) return;
    if (current.scheduledInitialTurnPhase === "terminal") return;
    if (current.lastTouched !== snapshot.activationTouch) return;
    if (snapshot.existed) {
      current.scheduledInitialTurnPhase = snapshot.previousPhase;
      current.busy = snapshot.previousBusy;
    } else {
      delete sessionStates[snapshot.id];
    }
    if (state.activeSessionId === snapshot.id && snapshot.previousStateBusy !== null) {
      state.busy = snapshot.previousStateBusy;
    }
  }
  function saveWorkingSetTo(buf) {
    if (!buf) return;
    buf.messages = state.messages; buf.chatItems = state.chatItems; buf.artifacts = state.artifacts;
    buf.composerDraft = state.composerDraft || "";
    buf.turnTimeline = state.turnTimeline;
    buf.activeTurnTimelineId = state.activeTurnTimelineId;
    buf.personaEvents = state.personaEvents;
    buf.pinvouReviews = state.pinvouReviews;
    buf.pinvouSceneEvents = state.pinvouSceneEvents;
    buf.busy = buf.scheduledInitialTurnPhase === "active" ? true : state.busy;
    buf.planSnapshot = state.planSnapshot; buf.modeState = state.modeState;
    buf.thinking = state.thinking; buf.tokens = state.tokens; buf.queued = state.queued;
    buf.activePersona = state.activePersona;
    buf.mountedCollection = state.mountedCollection;
    buf.mountedCollections = state.mountedCollections;
    buf.mountedCollectionsRevision = state.mountedCollectionsRevision;
    buf.scheduledTaskDraft = state.scheduledTaskDraft;
    buf.stream = {
      currentStreamText, currentStreamId,
      pendingAssistantText, pendingAssistantBlocks,
      itemIdSeq, toolMeta,
    };
  }
  function loadWorkingSetFrom(buf) {
    if (!buf) return;
    state.messages = buf.messages; state.chatItems = buf.chatItems; state.artifacts = buf.artifacts;
    state.composerDraft = buf.composerDraft || "";
    state.turnTimeline = buf.turnTimeline || [];
    state.activeTurnTimelineId = buf.activeTurnTimelineId || null;
    state.personaEvents = buf.personaEvents || [];
    state.pinvouReviews = buf.pinvouReviews || [];
    state.pinvouSceneEvents = buf.pinvouSceneEvents || [];
    state.pinvouModal = null; // 切 session 关掉检阅弹窗
    state.turnDirtyArtifacts = []; // turn 临时态,切 session 清空,别串到新 session
    state.turnPresentedArtifacts = [];
    state.busy = buf.scheduledInitialTurnPhase === "active" ? true : buf.busy;
    state.planSnapshot = buf.planSnapshot; state.modeState = buf.modeState;
    state.thinking = buf.thinking; state.tokens = buf.tokens; state.queued = buf.queued || [];
    state.activePersona = buf.activePersona || null;
    state.mountedCollection = buf.mountedCollection || null;
    state.mountedCollections = Array.isArray(buf.mountedCollections)
      ? buf.mountedCollections
      : (state.mountedCollection == null ? [] : [{ collectionId: state.mountedCollection, enabled: true }]);
    state.mountedCollectionsRevision = Number(buf.mountedCollectionsRevision || 0);
    state.scheduledTaskDraft = buf.scheduledTaskDraft || null;
    const s = buf.stream || {};
    currentStreamText = s.currentStreamText || ""; currentStreamId = s.currentStreamId || 0;
    pendingAssistantText = s.pendingAssistantText || ""; pendingAssistantBlocks = s.pendingAssistantBlocks || [];
    itemIdSeq = s.itemIdSeq || 0; toolMeta = s.toolMeta || {};
  }
  function hydrateWorkingSetFromSaved(buf, saved) {
    if (!buf || !saved) return;
    const completedRemoteTurn = !!buf.remoteTerminalSeen || (!!buf.remoteTurnActive && !buf.busy);
    buf.messages = Array.isArray(saved.messages) ? saved.messages : [];
    buf.sessionRevision = String(saved.transcript_revision || saved.transcriptRevision || "");
    buf.chatItems = [];
    buf.turnTimeline = [];
    buf.activeTurnTimelineId = null;
    buf.artifacts = Array.isArray(saved.artifacts) ? saved.artifacts.map(function (a) {
      const p = typeof a === "string" ? a : (a.storage_path || a.path || "");
      return { path: p, basename: basename(p) };
    }) : [];
    buf.artifacts = filterSessionArtifacts(buf.artifacts, saved.metadata && saved.metadata.id);
    buf.personaEvents = [];
    buf.pinvouReviews = [];
    buf.pinvouSceneEvents = loadPinvouSceneEventsForSession(saved.metadata && saved.metadata.id);
    if (completedRemoteTurn) {
      buf.remoteTurnActive = false;
      buf.remoteTerminalSeen = false;
      buf.remoteBaselineMessageCount = null;
      buf.remoteBaselineTrusted = false;
      buf.remoteExpectedAssistantKey = "";
      buf.remoteCommittedRevision = "";
      buf.deferredRemoteUserEvent = null;
    }
    buf.stream = {
      currentStreamText: "", currentStreamId: 0, pendingAssistantText: "",
      pendingAssistantBlocks: [], itemIdSeq: 0, toolMeta: {},
    };
  }
  function decodeBase64Bytes(encoded) {
    const binary = window.atob(String(encoded || ""));
    const bytes = new Uint8Array(binary.length);
// binary is a single-byte Latin-1 string produced by atob; charCode is the byte value. codePointAt is equivalent here but gains nothing.
    for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i); // eslint-disable-line unicorn/prefer-code-point
    return bytes;
  }
  function encodeBase64Bytes(bytes) {
    let binary = "";
    const chunkSize = 0x8000;
    for (let offset = 0; offset < bytes.length; offset += chunkSize) {
      const chunk = bytes.subarray(offset, Math.min(offset + chunkSize, bytes.length));
      // chunk only holds 0-255 byte values; fromCharCode/fromCodePoint are equivalent. Keep the apply-chunked hot path.
      binary += String.fromCharCode.apply(null, chunk); // eslint-disable-line unicorn/prefer-code-point
    }
    return window.btoa(binary);
  }
  const SESSION_DOWNLOAD_LEASES_KEY = "pinvou.web_session_download_leases.v1";
  const activeSessionDownloads = Object.create(null);
  let sessionDownloadLeases = null;
  let sessionDownloadCleanupPromise = null;
  function readSessionDownloadLeases() {
    if (sessionDownloadLeases !== null) return [...sessionDownloadLeases];
    try {
      const raw = window.sessionStorage && window.sessionStorage.getItem(SESSION_DOWNLOAD_LEASES_KEY);
      let parsed = raw ? JSON.parse(raw) : [];
      if (!Array.isArray(parsed)) parsed = [];
      sessionDownloadLeases = parsed.filter(function (entry) {
        return entry && typeof entry.download_id === "string" && entry.download_id &&
          typeof entry.session_id === "string" && entry.session_id;
      }).slice(-16);
      return [...sessionDownloadLeases];
    } catch {
      sessionDownloadLeases = [];
      return [...sessionDownloadLeases];
    }
  }
  function writeSessionDownloadLeases(entries) {
    sessionDownloadLeases = entries.slice(-16);
    try {
      if (!window.sessionStorage) return;
      if (sessionDownloadLeases.length) {
        window.sessionStorage.setItem(SESSION_DOWNLOAD_LEASES_KEY, JSON.stringify(sessionDownloadLeases));
      } else {
        window.sessionStorage.removeItem(SESSION_DOWNLOAD_LEASES_KEY);
      }
    } catch { /* when storage is unavailable the lease stays in memory only */ }
  }
  function rememberSessionDownloadLease(downloadId, sid) {
    const entries = readSessionDownloadLeases().filter(function (entry) {
      return entry.download_id !== downloadId;
    });
    entries.push({ download_id: downloadId, session_id: sid });
    writeSessionDownloadLeases(entries);
  }
  function forgetSessionDownloadLease(downloadId) {
    writeSessionDownloadLeases(readSessionDownloadLeases().filter(function (entry) {
      return entry.download_id !== downloadId;
    }));
  }
  async function cancelSessionDownloadLease(downloadId, sid) {
    return invoke("web_access_cancel_session_download", {
      id: sid,
      downloadId,
    });
  }
  async function cleanupAbandonedSessionDownloads(diagnostics) {
    const entries = readSessionDownloadLeases().filter(function (entry) {
      return !activeSessionDownloads[entry.download_id];
    });
    if (!entries.length) return;
    diagnostics.cleanup_requested_count = entries.length;
    const failed = [];
    for (let index = 0; index < entries.length; index++) {
      const entry = entries[index];
      try {
        await cancelSessionDownloadLease(entry.download_id, entry.session_id);
        forgetSessionDownloadLease(entry.download_id);
      } catch {
        failed.push(entry.download_id);
      }
    }
    diagnostics.cleanup_failed_count = failed.length;
    diagnostics.cleanup_succeeded_count = entries.length - failed.length;
  }
  function validSessionDownloadId(downloadId) {
    return typeof downloadId === "string" && downloadId.length >= 8 && downloadId.length <= 128 &&
      /^download_[A-Za-z0-9_-]+$/.test(downloadId);
  }
  function scheduleAbandonedSessionDownloadCleanup() {
    if (!IS_WEB || sessionDownloadCleanupPromise) return sessionDownloadCleanupPromise;
    const diagnostics = {
      transport_kind: "web_chunked_rpc",
      cleanup_requested_count: 0,
      cleanup_failed_count: 0,
      cleanup_succeeded_count: 0,
    };
    sessionDownloadCleanupPromise = waitForWebInvokeCapabilities().then(async function () {
      if (!canInvoke("web_access_cancel_session_download")) return false;
      await cleanupAbandonedSessionDownloads(diagnostics);
      recordAuthoritySyncDiagnostic("session_download_cleanup", diagnostics);
      return true;
    }).catch(function () {
      return false;
    }).finally(function () {
      sessionDownloadCleanupPromise = null;
    });
    return sessionDownloadCleanupPromise;
  }
  function newSessionDownloadId() {
    let token = "";
    try {
      if (window.crypto && typeof window.crypto.randomUUID === "function") {
        token = window.crypto.randomUUID().replaceAll('-', ""); // safari14-ok: guarded above
      }
    } catch { /* on UUID generation failure, fall through to the fallback below */ }
    // eslint-disable-next-line sonarjs/pseudo-random -- not security-sensitive: download ID dedup; failures are safely retryable
    if (!token) token = Date.now().toString(36) + Math.random().toString(36).slice(2);
    return "download_web_" + token;
  }
  // eslint-disable-next-line sonarjs/cognitive-complexity -- legacy bridge; refactor tracked separately
  async function loadSessionForClient(sid, setActive, diagnostics) {
    diagnostics = diagnostics || {};
    diagnostics.transport_kind = IS_WEB ? "web_chunked_rpc" : "desktop_invoke";
    diagnostics.started_at_ms = Date.now();
    diagnostics.chunk_count = 0;
    diagnostics.bytes_received = 0;
    if (!IS_WEB) {
      const localSaved = await invoke("load_session", { id: sid, setActive: !!setActive });
      diagnostics.elapsed_ms = Date.now() - diagnostics.started_at_ms;
      return localSaved;
    }
    // New WebUI can run against an older desktop. The cancel command is the
    // capability boundary for client-selected/persisted leases: older
    // desktops keep their server-generated download id protocol and must not
    // be blocked by cleanup RPCs they do not implement.
    try {
      await waitForWebInvokeCapabilities();
    } catch (capabilityError) {
      diagnostics.elapsed_ms = Date.now() - diagnostics.started_at_ms;
      diagnostics.error_present = true;
      diagnostics.error_category = capabilityError && capabilityError.code === "desktop_capabilities_timeout"
        ? "capability_snapshot_timeout"
        : "capability_snapshot_unavailable";
      recordAuthoritySyncDiagnostic("session_download_capability_wait_failed", {
        session_id: sid,
        transport: diagnostics,
      });
      throw capabilityError;
    }
    const supportsSessionDownloadCancellation = canInvoke("web_access_cancel_session_download");
    if (supportsSessionDownloadCancellation) {
      await cleanupAbandonedSessionDownloads(diagnostics);
    }
    let offset = 0;
    let total = null;
    let payload = null;
    let downloadId = supportsSessionDownloadCancellation ? newSessionDownloadId() : "";
    let unexpectedDownloadId = "";
    const maxSessionBytes = 256 * 1024 * 1024;
    diagnostics.cancellable_lease = supportsSessionDownloadCancellation;
    if (supportsSessionDownloadCancellation) {
      activeSessionDownloads[downloadId] = true;
      rememberSessionDownloadLease(downloadId, sid);
      diagnostics.download_id = downloadId;
    }
    try {
      while (true) {
        const chunkArgs = {
          id: sid,
          downloadId: offset ? downloadId : null,
          offset,
          // 不传 limit，由桌面端按自身版本上限决定块大小；新 WebUI
          // 先于桌面部署时也不会因为块大小超过旧桌面上限而被拒绝。
        };
        if (supportsSessionDownloadCancellation && !offset) {
          chunkArgs.requestedDownloadId = downloadId;
        }
        const chunk = await invoke("web_access_load_session_chunk", chunkArgs);
        diagnostics.chunk_count += 1;
        const chunkOffset = Number(chunk && chunk.offset);
        const chunkTotal = Number(chunk && chunk.total);
        const chunkDownloadId = String((chunk && (chunk.download_id || chunk.downloadId)) || "");
        if (supportsSessionDownloadCancellation && downloadId &&
            chunkDownloadId !== downloadId && validSessionDownloadId(chunkDownloadId)) {
          unexpectedDownloadId = chunkDownloadId;
          rememberSessionDownloadLease(unexpectedDownloadId, sid);
          diagnostics.error_category = "download_id_mismatch";
        }
        if (!Number.isSafeInteger(chunkOffset) || chunkOffset !== offset ||
            !Number.isSafeInteger(chunkTotal) || chunkTotal < 0 || chunkTotal > maxSessionBytes ||
            !chunkDownloadId || (downloadId && chunkDownloadId !== downloadId)) {
          throw new Error(bt("sessionChunkInvalid"));
        }
        if (!downloadId) downloadId = chunkDownloadId;
        diagnostics.download_id = downloadId;
        if (total === null) {
          total = chunkTotal;
          payload = new Uint8Array(total);
        } else if (chunkTotal !== total) {
          throw new Error(bt("sessionChunkChanged"));
        }
        const data = decodeBase64Bytes(chunk.data_base64 || chunk.dataBase64);
        diagnostics.bytes_received += data.length;
        diagnostics.declared_total_bytes = total;
        if (offset + data.length > total) throw new Error(bt("sessionChunkOverflow"));
        payload.set(data, offset);
        offset += data.length;
        if (chunk.eof) {
          if (offset !== total) throw new Error(bt("sessionChunkEarlyEnd"));
          if (supportsSessionDownloadCancellation) {
            delete activeSessionDownloads[downloadId];
            forgetSessionDownloadLease(downloadId);
          }
          break;
        }
        if (!data.length) throw new Error(bt("sessionChunkNoProgress"));
      }
    } catch (error) {
      if (supportsSessionDownloadCancellation && downloadId) {
        const cancellationIds = [downloadId];
        if (unexpectedDownloadId && unexpectedDownloadId !== downloadId) {
          cancellationIds.push(unexpectedDownloadId);
        }
        cancellationIds.forEach(function (id) { delete activeSessionDownloads[id]; });
        diagnostics.cancel_requested = true;
        diagnostics.cancel_succeeded = true;
        for (let cancelIndex = 0; cancelIndex < cancellationIds.length; cancelIndex++) {
          const cancellationId = cancellationIds[cancelIndex];
          try {
            await cancelSessionDownloadLease(cancellationId, sid);
            forgetSessionDownloadLease(cancellationId);
          } catch {
            diagnostics.cancel_succeeded = false;
            diagnostics.error_category = "cancel_rpc_failed";
          }
        }
      }
      throw error;
    }
    diagnostics.elapsed_ms = Date.now() - diagnostics.started_at_ms;
    return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(payload));
  }
  async function ensureSessionBufferLoaded(sid) {
    if (!sid) return;
    if (sid === state.activeSessionId) return;
    const buf = getBuffer(sid);
    const meta = state.sessions.find(function (s) { return s.id === sid; }) || {};
    const knownCount = Number(meta.message_count || 0);
    if (buf.busy) return;
    if (buf.loadedFromDisk && (!knownCount || buf.messages.length >= knownCount)) return;
    if (!buf.loadedFromDisk && (buf.messages.length || buf.chatItems.length) && (!knownCount || buf.messages.length >= knownCount)) return;
    const saved = await loadSessionForClient(sid, false);
    const savedCount = saved && saved.metadata ? Number(saved.metadata.message_count || 0) : 0;
    if ((buf.messages.length || buf.chatItems.length) && savedCount <= buf.messages.length) {
      buf.loadedFromDisk = true;
      return;
    }
    // 下载挂起期间后台回合可能已开始（busy 置位、直播流写入中）：此时用磁盘
    // 快照 hydrate 会截断正在流式生成的内容，必须复检后放弃（审计）。
    if (buf.busy || buf.remoteTurnActive) return;
    hydrateWorkingSetFromSaved(buf, saved);
    try { buf.personaEvents = await invoke("get_session_persona_events", { sessionId: sid }) || []; } catch { buf.personaEvents = []; }
    try { buf.pinvouReviews = await invoke("get_session_pinvou_reviews", { sessionId: sid }) || []; } catch { buf.pinvouReviews = []; }
    buf.pinvouSceneEvents = await syncPinvouSceneEventsForSession(sid);
    try { buf.turnTimeline = await invoke("get_session_timeline", { sessionId: sid }) || []; } catch { buf.turnTimeline = []; }
    // 手机可能在桌面仍停留草稿页/其他 session 时先唤醒这个后台 session。
    // 仅 hydrate messages 而把 chatItems 留空，会让后续 switchToSession 命中缓存快路径，
    // 不再 rerenderFromMessages，桌面便只看得到手机唤醒后的新内容，历史像是“丢了”。
    // 在首次磁盘 hydration 后先完整重建展示层，再由 mobile_user_message 追加当前轮；
    // buf.busy 时上方已提前返回，不会覆盖正在流式生成的实时 chatItems。
    runSyncOnSession(sid, function () {
      resetPendingAssistant();
      rerenderFromMessages();
    });
    buf.loadedFromDisk = true;
  }
  // 把 active 工作集存好后切到 id 的 buffer(opts.fresh=新建空 buffer)。
  function switchActiveTo(id, opts) {
    // Set the new active id before touching the old buffer: the LRU prune
    // triggered by that touch protects the target via activeSessionId. Touching
    // the old buffer first (active id still old) could evict an idle target as
    // the oldest entry, and the freshBuffer() fallback below would silently
    // show an empty session.
    const previousActiveId = state.activeSessionId;
    state.activeSessionId = id;
    if (previousActiveId) saveWorkingSetTo(getBuffer(previousActiveId));
    let buf = sessionStates[id];
    if (!buf || (opts && opts.fresh)) {
      buf = sessionStates[id] = freshBuffer();
      // `fresh` is used for a Session this client just created, so its empty
      // buffer is authoritative rather than an event-created partial cache —
      // and it must not resurrect a draft stashed from an earlier eviction of
      // the same (recycled) id. Non-fresh fallback recreates an evicted
      // buffer, so restore its stashed draft.
      if (opts && opts.fresh) buf.loadedFromDisk = true;
      else restoreEvictedSessionDraft(id, buf);
    }
    touchSessionBuffer(id, buf, id.indexOf("sched-") === 0);
    loadWorkingSetFrom(buf);
    state.artifacts = filterSessionArtifacts(state.artifacts, id);
    scheduleShellPoll(id, true);
  }
  // 在指定 session 的工作集上跑一段【同步】逻辑。sid 是 active → 直接跑(零行为变化);
  // 否则临时切到该 buffer 跑完再切回(期间不 notify)。
  function runSyncOnSession(sid, fn) {
    if (!sid || sid === state.activeSessionId) { fn(); return; }
    const bg = sessionStates[sid]; if (!bg) return;
    touchSessionBuffer(sid, bg, isScheduledRunSession(sid));
    const realId = state.activeSessionId;
    const draftComposer = realId ? "" : (state.composerDraft || "");
    // A null active id no longer guarantees an empty draft: WebUI can already
    // be showing an optimistic first message while its Session is being
    // materialized. Snapshot the complete draft just like a regular Session
    // before routing a late event from another Session in the background.
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
      // Restore the exact draft/Session working set that was visible before
      // the background event; replacing a draft with freshBuffer() would erase
      // its optimistic first message or unsent composer text.
      if (!realId) restoreBuffer.composerDraft = draftComposer;
      loadWorkingSetFrom(restoreBuffer);
    }
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
      try { await invoke("save_session_artifacts", { id: sid, paths: arts.map(function (a) { return a.path; }) }); } catch { /* persistence failure must not block session switching */ }
      if (isDefaultChatTitle(meta.title) || personaPlaceholderTitles[sid]) {
        const firstUser = msgs.find(function (m) { return m.role === "user"; });
        // 自动标题复用展示层过滤：内部信封/子智能体交接不参与命名，避免 XML 痕迹进
        // sidebar。hideInternalEnvelope=true 剥离 turn_meta/system-reminder 元数据块，
        // 否则普通消息的标题会拼入尾随 turn_meta（引擎持久化为独立 text block）。
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
    // chat:done 与远端 load_session 分属两条异步通道。尤其是 WebUI 刚创建的
    // Session，第一份可读快照可能仍停在本轮 user，若立即拿它重建展示层，会把
    // 已完整显示的流式 assistant 气泡一闪覆盖掉。优先用后端已提交 revision
    // 建立 authority barrier；旧桌面端未提供 revision 时，才回退到消息数与
    // 最后一条 assistant 的展示身份校验。
    const expectedAssistantKey = buf.remoteTerminalSeen
      ? String(buf.remoteExpectedAssistantKey || "")
      : "";
    // The committed revision identifies the canonical terminal transcript.
    // Streamed/native-tool blocks may be normalized before persistence, so a
    // presentation-derived message key is only a fallback for older desktops.
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
        // Relay replay may deliver the commit marker immediately after done,
        // and a newer turn's commit can land while this retry window is open.
        // Re-read the live revision every attempt so a bumped expected value
        // converges instead of comparing a stale one (which would report a
        // false unsynced warning and block queued sends until the next event).
        if (buf.remoteTerminalSeen) {
          expectedCommittedRevision = String(buf.remoteCommittedRevision || "");
        }
        const attemptStartedAt = Date.now();
        const transfer = {};
        try {
          const saved = await loadSessionForClient(sid, false, transfer);
          if (!saved || !Array.isArray(saved.messages)) {
            recordAuthoritySyncDiagnostic("reconcile_attempt_rejected", {
              trace_id: traceId, session_id: sid, attempt: attempt + 1,
              reason: "invalid_snapshot", elapsed_ms: Date.now() - attemptStartedAt,
              snapshot_present: !!saved, transport: transfer,
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
                saved_revision: savedRevision, saved_message_count: saved.messages.length,
                transport: transfer,
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
                saved_message_count: saved.messages.length, transport: transfer,
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
                transport: transfer,
              });
              continue;
            }
          }
          // 写入前回合归属校验（审计）：重试窗口内新回合可能已开始
          // （markRemoteTurn 置 busy/remoteTurnActive、重置 revision），此时用
          // 旧终稿重建工作集会截断新回合直播流——放弃本轮对账，由新回合自己的
          // done 事件重新对账。放弃条件只用 busy：不能用 remoteTurnActive（正常
          // 远端回合 done 后它恒为 true，会拦死所有对账），也不能用
          // !remoteTerminalSeen（tauri 版 scheduled run 不置 terminalSeen）。
          if (buf.busy) return false;
          runSyncOnSession(sid, function () {
            // The durable transcript already reconstructs user/assistant/tool
            // items. Preserve only presentation-side cards; otherwise a client
            // joining mid-turn appends its replayed tail after the full answer.
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
              if (!item || item.type === "user" || item.type === "tool") return false;
              if (item.type === "assistant") return item.interruptedDisplayOnly === true;
              if (item.turnErrorNotice && !item.legacyConversationOnly) return false;
              // Plan cards need semantic matching by their plan snapshot. Their
              // generic hydration key includes ticket/action state and would
              // append an active live duplicate after the durable frozen card.
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
            // A plan ticket is transport metadata rather than model context, so
            // the durable transcript cannot reconstruct it on its own. Carry
            // active state or resolved tickets onto the matching canonical card,
            // newest-to-newest for repeated identical plans.
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
            transport: transfer,
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
            transport: transfer,
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
  let subscribers = [];
  function snapshotState() {
    if (typeof structuredClone === "function") {
      try { return structuredClone(state); } catch { /* silently fall back to JSON */ } // safari14-ok: typeof-guarded with JSON fallback
    }
    return JSON.parse(JSON.stringify(state));
  }
  // Build immutable persistent subscription snapshots. Reconcile nested values
  // so unchanged transcript history is shared across notifications, while each
  // in-place streaming mutation gets a detached changed path.
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
        // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 floor: Object.hasOwn is unavailable; this call is already the safe form
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
        // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 floor: Object.hasOwn is unavailable; this call is already the safe form
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
  let subscriptionSnapshot = null;
  function subscriptionState() {
    subscriptionSnapshot = subscriptionStateValue(state, subscriptionSnapshot);
    return subscriptionSnapshot;
  }
  let notificationQueue = [];
  let notificationDispatching = false;
  function notify() {
    if (suppressNotify) return;
    // 会话列表「工作中」指示:active 取活动工作集 state.busy,其余取各自 buffer.busy
    state.sessionBusy = {};
    for (const id in sessionStates) state.sessionBusy[id] = !!sessionStates[id].busy;
    if (state.activeSessionId) state.sessionBusy[state.activeSessionId] = !!state.busy;
    const snapshot = subscriptionState();
    // Each queued round fixes both state and membership. Callback-time
    // subscription changes apply only to rounds queued afterwards.
    notificationQueue.push({ snapshot, subscribers: [...subscribers] });
    if (notificationDispatching) return;
    notificationDispatching = true;
    try {
      while (notificationQueue.length) {
        const round = notificationQueue.shift();
        for (let i = 0; i < round.subscribers.length; i++) round.subscribers[i](round.snapshot);
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
  function subscribe(fn) {
    subscribers.push(fn);
    return function () {
      subscribers = subscribers.filter(function (f) { return f !== fn; });
    };
  }

  function loadScheduledTaskTemplateSources() {
    try {
      const parsed = JSON.parse(window.localStorage.getItem(SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY) || "{}");
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return Object.create(null);
      return Object.keys(parsed).reduce(function (result, taskId) {
        if (typeof parsed[taskId] === "string" && parsed[taskId].trim()) {
          result[taskId] = parsed[taskId].trim();
        }
        return result;
      }, Object.create(null));
    } catch {
      return Object.create(null);
    }
  }

  function persistScheduledTaskTemplateSources() {
    try {
      window.localStorage.setItem(
        SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY,
        JSON.stringify(scheduledTaskTemplateSources)
      );
    } catch { /* ignore when localStorage is unavailable */ }
  }

  function rememberScheduledTaskTemplateSource(taskId, templateId) {
    if (!taskId || !templateId) return;
    scheduledTaskTemplateSources[taskId] = templateId;
    persistScheduledTaskTemplateSources();
  }

  function forgetScheduledTaskTemplateSource(taskId) {
    // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 floor: Object.hasOwn is unavailable; this call is already the safe form
    if (!taskId || !Object.prototype.hasOwnProperty.call(scheduledTaskTemplateSources, taskId)) return;
    delete scheduledTaskTemplateSources[taskId];
    persistScheduledTaskTemplateSources();
  }

  function attachScheduledTaskTemplateSource(task) {
    if (!task || !task.id) return task;
    const templateId = task.templateId || scheduledTaskTemplateSources[task.id] || null;
    if (templateId) {
      task.templateId = templateId;
      if (scheduledTaskTemplateSources[task.id] !== templateId) {
        rememberScheduledTaskTemplateSource(task.id, templateId);
      }
    }
    return task;
  }

  function attachAndPruneScheduledTaskTemplateSources(tasks) {
    const activeIds = Object.create(null);
    (tasks || []).forEach(function (task) {
      if (!task || !task.id) return;
      activeIds[task.id] = true;
      attachScheduledTaskTemplateSource(task);
    });
    let changed = false;
    Object.keys(scheduledTaskTemplateSources).forEach(function (taskId) {
      if (activeIds[taskId]) return;
      delete scheduledTaskTemplateSources[taskId];
      changed = true;
    });
    if (changed) persistScheduledTaskTemplateSources();
    return tasks;
  }

  function upsertScheduledTask(task) {
    if (!task || !task.id) return;
    attachScheduledTaskTemplateSource(task);
    let found = false;
    state.scheduledTasks = (state.scheduledTasks || []).map(function (item) {
      if (item.id !== task.id) return item;
      found = true;
      return task;
    });
    if (!found) state.scheduledTasks = [task, ...(state.scheduledTasks || [])];
  }

  function applyScheduledRunViewed(automationId, runId, receipt) {
    function markRunViewed(item) {
      const itemAutomationId = item.automationId || state.selectedScheduledTaskId;
      if (itemAutomationId !== automationId || item.id !== runId) return item;
      return Object.assign({}, item, { unread: false });
    }
    state.scheduledTaskRuns = (state.scheduledTaskRuns || []).map(markRunViewed);
    state.scheduledTaskRecentRuns = (state.scheduledTaskRecentRuns || []).map(markRunViewed);
    const hasUnreadRuns = receipt && typeof receipt.hasUnreadRuns === "boolean"
      ? receipt.hasUnreadRuns
      : (state.scheduledTaskRuns || []).some(function (item) {
          return (item.automationId || state.selectedScheduledTaskId) === automationId && !!item.unread;
        });
    state.scheduledTasks = (state.scheduledTasks || []).map(function (task) {
      return task.id === automationId
        ? Object.assign({}, task, { hasUnreadRuns })
        : task;
    });
    if (state.scheduledTaskDetail && state.scheduledTaskDetail.id === automationId) {
      state.scheduledTaskDetail = Object.assign({}, state.scheduledTaskDetail, {
        hasUnreadRuns,
      });
    }
  }

  function invalidateScheduledTaskReads(automationId) {
    scheduledTaskRequestTokens.tasks += 1;
    if (state.selectedScheduledTaskId === automationId) {
      scheduledTaskRequestTokens.detail += 1;
      scheduledTaskRequestTokens.runs += 1;
    }
    scheduledTaskRefreshInFlight = null;
  }

  function invalidateScheduledRecentRuns() {
    scheduledRecentRunsRequestToken += 1;
  }

  function invalidateScheduledRecentRunsForSession(id) {
    if (String(id || "").indexOf("sched-") === 0) invalidateScheduledRecentRuns();
  }

  function scheduleScheduledRunRefresh() {
    if (scheduledRunEventRefreshTimer) clearTimeout(scheduledRunEventRefreshTimer);
    scheduledRunEventRefreshTimer = setTimeout(function () {
      scheduledRunEventRefreshTimer = null;
      // Refresh task badges/detail first, then replace the global run list from
      // the same retained backend state. The aggregate request has its own stale
      // response guard, so a concurrent archive/delete cannot resurrect a row.
      Promise.resolve(refreshScheduledTaskData(20))
        .catch(function () {})
        .then(function () { return loadScheduledTaskRecentRuns(); })
        .catch(function () {});
    }, 400);
  }

  function scheduledTaskErrorText(error) {
    return String(error && error.message ? error.message : error);
  }

  function setScheduledTaskError(error, kind) {
    state.scheduledTaskError = error ? scheduledTaskErrorText(error) : null;
    state.scheduledTaskErrorKind = error ? (kind || "load") : null;
  }

  function dismissScheduledTaskError() {
    setScheduledTaskError(null);
    notify();
  }

  function clearScheduledTaskLoadError() {
    if (state.scheduledTaskErrorKind === "load") setScheduledTaskError(null);
  }

  function beginScheduledTaskLoad(stamp) {
    const generation = stamp.generation;
    scheduledTaskPendingLoads[generation] = (scheduledTaskPendingLoads[generation] || 0) + 1;
    if (generation === scheduledTaskSelectionGeneration) {
      state.scheduledTaskLoading = true;
      clearScheduledTaskLoadError();
      notify();
    }
  }

  function endScheduledTaskLoad(stamp) {
    const generation = stamp.generation;
    scheduledTaskPendingLoads[generation] = Math.max(0, (scheduledTaskPendingLoads[generation] || 0) - 1);
    if (!scheduledTaskPendingLoads[generation]) delete scheduledTaskPendingLoads[generation];
    if (generation === scheduledTaskSelectionGeneration) {
      state.scheduledTaskLoading = !!scheduledTaskPendingLoads[generation];
      notify();
    }
  }

  function scheduledTaskRequestStamp(kind, id) {
    scheduledTaskRequestTokens[kind] += 1;
    return {
      kind,
      token: scheduledTaskRequestTokens[kind],
      generation: scheduledTaskSelectionGeneration,
      id: id || null,
    };
  }

  function isCurrentScheduledTaskRequest(stamp) {
    if (!stamp || stamp.generation !== scheduledTaskSelectionGeneration) return false;
    if (scheduledTaskRequestTokens[stamp.kind] !== stamp.token) return false;
    // id 检查省略：selectedScheduledTaskId 唯一写者是 selectScheduledTask（每次
    // 改写前 generation+1），id 变化必然被上方 generation 检查拦截（审计清理）。
    return true;
  }

  // eslint-disable-next-line sonarjs/no-invariant-returns -- echoing back the normalized id is a deliberate API contract
  function selectScheduledTask(id) {
    const nextId = typeof id === "string" && id.trim() ? id.trim() : null;
    if (state.selectedScheduledTaskId === nextId) return nextId;
    scheduledTaskSelectionGeneration += 1;
    state.scheduledTaskSelectionGeneration = scheduledTaskSelectionGeneration;
    state.selectedScheduledTaskId = nextId;
    state.scheduledTaskDetail = null;
    state.scheduledTaskRuns = [];
    state.scheduledTaskLoading = !!scheduledTaskPendingLoads[scheduledTaskSelectionGeneration];
    setScheduledTaskError(null);
    notify();
    return nextId;
  }

  function clearScheduledTaskSelection() {
    selectScheduledTask(null);
  }

  function extractBalancedJsonObject(text) {
    const start = String(text || "").indexOf("{");
    if (start < 0) return null;
    let depth = 0;
    let inString = false;
    let escaping = false;
    for (let i = start; i < text.length; i++) {
      const ch = text.charAt(i);
      if (inString) {
        if (escaping) escaping = false;
        else if (ch === "\\") escaping = true;
        else if (ch === "\"") inString = false;
        continue;
      }
      if (ch === "\"") { inString = true; continue; }
      if (ch === "{") depth++;
      else if (ch === "}") {
        depth--;
        if (depth === 0) return text.slice(start, i + 1);
      }
    }
    return null;
  }

  function parseLooseJsonObject(text) {
    try { return JSON.parse(text); } catch { /* invalid JSON: the caller falls back to the raw text */ }
    try { return JSON.parse(String(text || "").replaceAll(/,(\s*[}\]])/g, "$1")); } catch { /* invalid JSON: the caller falls back to the raw text */ }
    const balanced = extractBalancedJsonObject(String(text || ""));
    if (!balanced) return null;
    try { return JSON.parse(balanced); } catch { /* invalid JSON: the caller falls back to the raw text */ }
    try { return JSON.parse(balanced.replaceAll(/,(\s*[}\]])/g, "$1")); } catch { /* invalid JSON: the caller falls back to the raw text */ }
    return null;
  }

  function normalizeScheduledTaskDraft(value) {
    if (!value || typeof value !== "object") return null;
    if (!value.name || !value.prompt || !value.rrule) return null;
    return {
      name: String(value.name),
      prompt: String(value.prompt),
      rrule: String(value.rrule),
      model: value.model ? String(value.model) : null,
      modelId: value.modelId ? String(value.modelId) : (value.model_id ? String(value.model_id) : null),
      mode: "yolo",
      paused: !!value.paused,
    };
  }

  function activeScheduledTaskModelConfig() {
    return (state.savedModels || []).find(function (model) {
      return model && model.id === state.activeModelId;
    }) || null;
  }

  function lockScheduledTaskDraftModel(draft) {
    if (!draft) return null;
    const active = activeScheduledTaskModelConfig();
    draft.model = draft.model || (active && active.model) || null;
    draft.modelId = draft.modelId || (active && active.id) || null;
    return draft;
  }

  function parseScheduledTaskDraftFromText(text) {
    if (!text || !text.includes("{")) return null;
    let preferred = null;
    let fallback = null;
    const re = /```([^\n`]*)\n([\s\S]*?)```/g;
    let match;
    // biome-ignore lint/suspicious/noAssignInExpressions: the assignment is the loop condition; refactoring would hurt readability
    while ((match = re.exec(text))) {
      const label = String(match[1] || "").trim().toLowerCase();
      const raw = String(match[2] || "").trim();
      if (!raw || raw.charAt(0) !== "{") continue;
      const candidate = normalizeScheduledTaskDraft(parseLooseJsonObject(raw));
      if (!candidate) continue;
      if (label === "scheduled-task-draft") return candidate;
      if ((label === "json" || !label) && !fallback) fallback = candidate;
      if (!preferred) preferred = candidate;
    }
    return fallback || preferred;
  }

  function clearScheduledTaskDraft() {
    state.scheduledTaskDraft = null;
    if (state.activeSessionId === state.scheduledTaskCreationSessionId) {
      state.scheduledTaskCreationSessionId = null;
    }
    notify();
  }

  async function confirmScheduledTaskDraft(editedDraft) {
    if (!state.scheduledTaskDraft || state.activeSessionId !== state.scheduledTaskCreationSessionId) return null;
    const active = activeScheduledTaskModelConfig();
    const lockedModel = state.scheduledTaskDraft.model || (active && active.model) || null;
    const lockedModelId = state.scheduledTaskDraft.modelId || (active && active.id) || null;
    const draft = normalizeScheduledTaskDraft(Object.assign({}, state.scheduledTaskDraft, editedDraft || {}, {
      model: lockedModel,
      modelId: lockedModelId,
    }));
    if (!draft) {
      const invalidDraftError = new Error(bt("scheduledDraftInvalid"));
      setScheduledTaskError(invalidDraftError, "action");
      notify();
      throw invalidDraftError;
    }
    const created = await createScheduledTask({
      name: draft.name,
      prompt: draft.prompt,
      rrule: draft.rrule,
      model: lockedModel,
      modelId: lockedModelId,
      mode: "yolo",
      paused: draft.paused,
    });
    state.scheduledTaskDraft = null;
    state.scheduledTaskCreationSessionId = null;
    notify();
    return created;
  }

  function scheduledTaskInputFromDraft(draft) {
    return {
      name: draft.name,
      prompt: draft.prompt,
      rrule: draft.rrule,
      model: draft.model || null,
      modelId: draft.modelId || null,
      mode: "yolo",
      paused: draft.paused,
    };
  }

  // 聊天创建拿到合法参数后立即落成任务。草稿不会进入可渲染 state，避免再出现一层确认卡。
  // autoOpenId 全局 last-writer：两会话并发创建时后完成者覆盖，且 startScheduledTaskChat
  // 清空后陈旧 completion 会复活 auto-open（审计 f）。全局单调创建序号，仅最新意图可写。
  let scheduledTaskAutoCreateSeq = 0;
  function autoCreateScheduledTaskDraft(draft, creationSessionId) {
    if (!draft || !creationSessionId || scheduledTaskAutoCreateInFlight[creationSessionId]) return;
    const lockedDraft = lockScheduledTaskDraftModel(draft);
    state.scheduledTaskDraft = null;
    const creationSeq = ++scheduledTaskAutoCreateSeq;
    const creation = Promise.resolve()
      .then(function () {
        return createScheduledTask(scheduledTaskInputFromDraft(lockedDraft));
      })
      .then(function (created) {
        if (state.scheduledTaskCreationSessionId === creationSessionId) {
          state.scheduledTaskCreationSessionId = null;
        }
        const creationBuffer = sessionStates[creationSessionId];
        if (creationBuffer) creationBuffer.scheduledTaskDraft = null;
        // 仅最新创建意图可写 autoOpenId（陈旧 completion 不得复活 auto-open，审计 f）
        if (created && created.id && creationSeq === scheduledTaskAutoCreateSeq) state.scheduledTaskAutoOpenId = created.id;
        notify();
        return created;
      })
      .catch(function (error) {
        // createScheduledTask 通常已记录错误；忙锁在进入 action 前抛出时在这里补记，且不产生未处理 Promise。
        if (!state.scheduledTaskError) setScheduledTaskError(error, "action");
        runSyncOnSession(creationSessionId, function () {
          addSystemItem(bt("scheduledCreateFailed") + scheduledTaskErrorText(error), {
            scheduledTaskCreationError: true,
          });
        });
        notify();
        return null;
      })
      .finally(function () {
        if (scheduledTaskAutoCreateInFlight[creationSessionId] === creation) {
          delete scheduledTaskAutoCreateInFlight[creationSessionId];
        }
      });
    scheduledTaskAutoCreateInFlight[creationSessionId] = creation;
  }

  async function loadScheduledTasks() {
    const stamp = scheduledTaskRequestStamp("tasks", null);
    beginScheduledTaskLoad(stamp);
    try {
      const tasks = await invoke("list_scheduled_tasks");
      if (!isCurrentScheduledTaskRequest(stamp)) return state.scheduledTasks;
      state.scheduledTasks = attachAndPruneScheduledTaskTemplateSources(
        Array.isArray(tasks) ? tasks : []
      );
      if (
        state.selectedScheduledTaskId &&
        (state.scheduledTasks || []).every(function (task) { return task.id !== state.selectedScheduledTaskId; })
      ) {
        selectScheduledTask(null);
      }
    } catch (e) {
      if (isCurrentScheduledTaskRequest(stamp)) setScheduledTaskError(e, "load");
    } finally {
      endScheduledTaskLoad(stamp);
    }
    return state.scheduledTasks;
  }

  async function readScheduledTask(id) {
    if (!id) {
      clearScheduledTaskSelection();
      return null;
    }
    if (state.selectedScheduledTaskId !== id) selectScheduledTask(id);
    const stamp = scheduledTaskRequestStamp("detail", id);
    beginScheduledTaskLoad(stamp);
    try {
      const detail = await invoke("read_scheduled_task", { id });
      if (!isCurrentScheduledTaskRequest(stamp)) return state.scheduledTaskDetail;
      state.scheduledTaskDetail = attachScheduledTaskTemplateSource(detail) || null;
      upsertScheduledTask(detail);
    } catch (e) {
      if (isCurrentScheduledTaskRequest(stamp)) setScheduledTaskError(e, "load");
    } finally {
      endScheduledTaskLoad(stamp);
    }
    return state.scheduledTaskDetail;
  }

  // 按 run.id upsert 单个任务的运行到侧边栏快捷列表。不裁剪条数(侧边栏显示所有
  // 现存定时运行,后端 retention 已按 automation 限制终态运行上限);传入窗口有限
  // (如任务详情页只拉了前 N 条)时不会误删其余任务或本任务的更早记录。
  function mergeScheduledTaskRecentRuns(task, runs) {
    if (!task || !task.id) return state.scheduledTaskRecentRuns || [];
    invalidateScheduledRecentRuns();
    let rows = [...(state.scheduledTaskRecentRuns || [])];
    (Array.isArray(runs) ? runs : []).forEach(function (run) {
      if (!run) return;
      rememberScheduledRunOwner(run);
      const merged = Object.assign({}, run, {
        automationId: run.automationId || task.id,
        taskName: task.name || bt("scheduledTaskFallbackName"),
        taskModel: task.model || null,
      });
      const index = rows.findIndex(function (row) { return row && row.id === merged.id; });
      if (index >= 0) rows[index] = merged;
      else rows.push(merged);
    });
    rows = rows.filter(function (run) { return run && run.sessionId && !run.archived; });
    rows.sort(function (a, b) {
      return new Date(b.scheduledFor || b.createdAt || 0).getTime() -
        new Date(a.scheduledFor || a.createdAt || 0).getTime();
    });
    state.scheduledTaskRecentRuns = rows;
    return state.scheduledTaskRecentRuns;
  }

  async function loadScheduledTaskRuns(id, limit) {
    if (!id) {
      clearScheduledTaskSelection();
      return [];
    }
    if (state.selectedScheduledTaskId !== id) selectScheduledTask(id);
    const stamp = scheduledTaskRequestStamp("runs", id);
    beginScheduledTaskLoad(stamp);
    try {
      const runs = await invoke("list_scheduled_task_runs", { id, limit });
      if (!isCurrentScheduledTaskRequest(stamp)) return state.scheduledTaskRuns;
      state.scheduledTaskRuns = Array.isArray(runs) ? runs : [];
      state.scheduledTaskRuns.forEach(rememberScheduledRunOwner);
      mergeScheduledTaskRecentRuns(
        (state.scheduledTasks || []).find(function (task) { return task && task.id === id; }),
        state.scheduledTaskRuns
      );
    } catch (e) {
      if (isCurrentScheduledTaskRequest(stamp)) setScheduledTaskError(e, "load");
    } finally {
      endScheduledTaskLoad(stamp);
    }
    return state.scheduledTaskRuns;
  }

  // 侧边栏"定时任务记录"一次读取所有保留的运行。后端只做一次 reconcile 和
  // Session 元数据扫描，避免任务数增长后形成 N 次命令调用与重复完整会话读取。
  async function loadScheduledTaskRecentRuns() {
    const requestToken = ++scheduledRecentRunsRequestToken;
    try {
      const tasks = state.scheduledTasks && state.scheduledTasks.length
        ? state.scheduledTasks
        : await loadScheduledTasks();
      if (requestToken !== scheduledRecentRunsRequestToken) {
        return state.scheduledTaskRecentRuns || [];
      }
      const runs = await invoke("list_scheduled_runs");
      if (requestToken !== scheduledRecentRunsRequestToken) {
        return state.scheduledTaskRecentRuns || [];
      }
      const tasksById = Object.create(null);
      (tasks || []).forEach(function (task) {
        if (task && task.id) tasksById[task.id] = task;
      });
      const rows = (Array.isArray(runs) ? runs : []).map(function (run) {
        if (!run) return null;
        rememberScheduledRunOwner(run);
        const automationId = run.automationId || run.automation_id;
        const task = tasksById[automationId] || null;
        return Object.assign({}, run, {
          automationId,
          taskName: task && task.name || run.taskName || bt("scheduledTaskFallbackName"),
          taskModel: task && task.model || run.taskModel || null,
        });
      }).filter(function (run) {
        return run && run.sessionId && !run.archived;
      });
      rows.sort(function (a, b) {
        return new Date(b.scheduledFor || b.createdAt || 0).getTime() -
          new Date(a.scheduledFor || a.createdAt || 0).getTime();
      });
      state.scheduledTaskRecentRuns = rows;
      notify();
      return state.scheduledTaskRecentRuns;
    } catch (e) {
      if (requestToken !== scheduledRecentRunsRequestToken) {
        return state.scheduledTaskRecentRuns || [];
      }
      console.warn("loadScheduledTaskRecentRuns failed", e);
      state.scheduledTaskRecentRuns = state.scheduledTaskRecentRuns || [];
      notify();
      return state.scheduledTaskRecentRuns;
    }
  }

  function refreshScheduledTaskData(limit) {
    const generation = scheduledTaskSelectionGeneration;
    if (scheduledTaskRefreshInFlight && scheduledTaskRefreshInFlight.generation === generation) {
      return scheduledTaskRefreshInFlight.promise;
    }
    const selectedId = state.selectedScheduledTaskId;
    const requests = [loadScheduledTasks()];
    if (selectedId) {
      requests.push(readScheduledTask(selectedId));
      requests.push(loadScheduledTaskRuns(selectedId, limit || 20));
    }
    const promise = Promise.all(requests).finally(function () {
      if (scheduledTaskRefreshInFlight && scheduledTaskRefreshInFlight.promise === promise) {
        scheduledTaskRefreshInFlight = null;
      }
    });
    scheduledTaskRefreshInFlight = { generation, promise };
    return promise;
  }

  const scheduledRunShortcutRefreshes = Object.create(null);
  const SCHEDULED_LINK_POLL_FAST_MS = 1000;
  const SCHEDULED_LINK_POLL_SLOW_MS = 5000;
  const SCHEDULED_LINK_POLL_FAST_ATTEMPTS = 15;
  // 兜底上限:只在 run 卡在 queued/running 且永不终态时才会走到,正常路径靠下面
  // 「拿到 sessionId」或「进入终态」提前收工。
  const SCHEDULED_LINK_POLL_DEADLINE_MS = 30 * 60 * 1000;

  // Fallback for run-now:正常路径由 sched-* 文件 watcher 推送刷新；但文件事件可能
  // 早于 ThreadCreated / ThreadLinked 被 run 记录吸收，或 watcher 本身不可用，因此
  // 仍定向轮询本次 run，直到拿到 sessionId 或进入终态。它独立于页面生命周期，
  // 用户立即切走也不会让侧边栏永远漏掉这条记录。
  //
  // 停止条件按 run 自身状态,不用固定次数:TaskManager 只有 1 个 worker,前一个任务
  // 正在跑 LLM turn 时,新 run 排队几分钟是常态,固定 20 次(20 秒)会提前放弃,
  // watcher 是主路径；这里保留较长窗口只为覆盖事件丢失和链接时序空窗。
  function refreshScheduledRunShortcutUntilLinked(automationId, runId) {
    if (!automationId || !runId) return;
    const key = automationId + ":" + runId;
    if (scheduledRunShortcutRefreshes[key]) return;
    scheduledRunShortcutRefreshes[key] = true;
    const deadline = Date.now() + SCHEDULED_LINK_POLL_DEADLINE_MS;

    function stop() {
      delete scheduledRunShortcutRefreshes[key];
    }
    function again(attempt) {
      if (Date.now() >= deadline) {
        stop();
        return;
      }
      setTimeout(function () { poll(attempt + 1); }, attempt < SCHEDULED_LINK_POLL_FAST_ATTEMPTS
        ? SCHEDULED_LINK_POLL_FAST_MS
        : SCHEDULED_LINK_POLL_SLOW_MS);
    }

    function taskStillListed() {
      return (state.scheduledTasks || []).some(function (item) {
        return item && item.id === automationId;
      });
    }

    function poll(attempt) {
      invoke("list_scheduled_task_runs", { id: automationId }).then(function (runs) {
        // 任务已被删除时不再回填：陈旧轮询响应会把已删任务以 fallback 名
        // 复活回侧边栏（审计 R1）。任务不在列表即收工，不 merge、不续排。
        const task = (state.scheduledTasks || []).find(function (item) {
          return item && item.id === automationId;
        });
        if (!task) {
          stop();
          return;
        }
        mergeScheduledTaskRecentRuns(task, runs);
        notify();
        // 必须看原始响应:mergeScheduledTaskRecentRuns 会滤掉尚无 sessionId 的记录,
        // 从合并结果里读不到目标 run 的状态。
        const target = (Array.isArray(runs) ? runs : []).find(function (run) {
          return run && run.id === runId;
        });
        // 会话已挂上 → 记录已进侧边栏;run 已终态却仍无会话 → 会话没建起来,再等也不会有;
        // run 记录消失(被删或被 retention 清掉)→ 没有等待对象。三种情况都收工。
        if (!target || target.sessionId || isScheduledRunTerminal(target.status)) {
          stop();
          return;
        }
        again(attempt);
      }).catch(function () {
        // 已删任务的后端响应是 Err 而非空列表（get_automation 文件已移除）。
        // 任务不在列表即收工，否则会以 1s/5s 空转重试到 30 分钟兜底。
        if (!taskStillListed()) {
          stop();
          return;
        }
        again(attempt);
      });
    }

    poll(0);
  }

  function upsertScheduledTaskRun(run) {
    if (!run || !run.id) return;
    rememberScheduledRunOwner(run);
    if (state.selectedScheduledTaskId && run.automationId && state.selectedScheduledTaskId !== run.automationId) return;
    let found = false;
    state.scheduledTaskRuns = (state.scheduledTaskRuns || []).map(function (item) {
      if (item.id === run.id) {
        found = true;
        return run;
      }
      return item;
    });
    if (!found) state.scheduledTaskRuns = [run, ...(state.scheduledTaskRuns || [])];
  }

  async function runScheduledTaskAction(action, operation) {
    if (state.scheduledTaskBusyAction) {
      throw new Error(bt("scheduledActionBusy"));
    }
    state.scheduledTaskBusyAction = action;
    setScheduledTaskError(null);
    notify();
    try {
      return await operation();
    } catch (e) {
      setScheduledTaskError(e, "action");
      throw e;
    } finally {
      state.scheduledTaskBusyAction = null;
      notify();
    }
  }

  const SCHEDULED_TASK_WRITABLE_FIELDS = ["name", "prompt", "rrule", "model", "modelId", "paused"];

  // Scheduled tasks always run as Yolo. Keep the wire boundary intentionally narrow so
  // legacy callers cannot reintroduce task-level permissions or external directories.
  function scheduledTaskBackendInput(input) {
    const source = input || {};
    const backendInput = { mode: "yolo" };
    SCHEDULED_TASK_WRITABLE_FIELDS.forEach(function (field) {
      // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 floor: Object.hasOwn is unavailable; this call is already the safe form
      if (Object.prototype.hasOwnProperty.call(source, field)) backendInput[field] = source[field];
    });
    return backendInput;
  }

  async function createScheduledTask(input) {
    return runScheduledTaskAction("create", async function () {
      const templateId = input && typeof input.templateId === "string" ? input.templateId.trim() : "";
      const selectAfterCreate = !input || input.selectAfterCreate !== false;
      const backendInput = scheduledTaskBackendInput(input);
      const created = await invoke("create_scheduled_task", { input: backendInput });
      if (!created || !created.id) {
        throw new Error(bt("scheduledCreateNoId"));
      }
      if (templateId) rememberScheduledTaskTemplateSource(created.id, templateId);
      attachScheduledTaskTemplateSource(created);
      // 立即重拉任务列表:新 stamp 会使创建前仍在途的 list_scheduled_tasks 响应失效,
      // 防止旧结果落地时把刚创建的任务从列表里覆盖掉。
      await loadScheduledTasks();
      upsertScheduledTask(created);
      if (selectAfterCreate) selectScheduledTask(created.id);
      if (selectAfterCreate) state.scheduledTaskDetail = created;
      notify();
      return created;
    });
  }

  async function updateScheduledTask(id, input) {
    return runScheduledTaskAction("update", async function () {
      const backendInput = scheduledTaskBackendInput(input);
      const updated = await invoke("update_scheduled_task", { id, input: backendInput });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  async function pauseScheduledTask(id) {
    return runScheduledTaskAction("pause", async function () {
      const updated = await invoke("pause_scheduled_task", { id });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  async function resumeScheduledTask(id) {
    return runScheduledTaskAction("resume", async function () {
      const updated = await invoke("resume_scheduled_task", { id });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  async function toggleScheduledTaskPinned(id, pinned) {
    return runScheduledTaskAction(pinned ? "pin" : "unpin", async function () {
      const updated = await invoke("set_scheduled_task_pinned", { id, pinned: !!pinned });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  async function deleteScheduledTask(id) {
    return runScheduledTaskAction("delete", async function () {
      invalidateScheduledRecentRuns();
      const deleted = await invoke("delete_scheduled_task", { id });
      // 作废删除前在途的整表 list / detail / runs 读（与 run-now 同模式）：否则
      // 3 秒轮询的旧 list 响应落地时会把刚删的任务复活回侧边栏（含本 feature
      // run-now 轮询依赖的 taskStillListed 判断，幽灵窗口会击穿 R1 守卫）。
      invalidateScheduledTaskReads(id);
      forgetScheduledTaskTemplateSource(id);
      state.scheduledTasks = (state.scheduledTasks || []).filter(function (task) { return task.id !== id; });
      if (state.selectedScheduledTaskId === id) selectScheduledTask(null);
      notify();
      return deleted;
    });
  }

  async function runScheduledTaskNow(id) {
    return runScheduledTaskAction("run-now", async function () {
      const run = await invoke("run_scheduled_task_now", { id });
      invalidateScheduledTaskReads(id);
      upsertScheduledTaskRun(run);
      const runStatus = String(run && run.status || "").toLowerCase();
      if (runStatus === "queued" || runStatus === "running") {
        state.scheduledTasks = (state.scheduledTasks || []).map(function (task) {
          return task.id === id ? Object.assign({}, task, { isRunning: true }) : task;
        });
        if (state.scheduledTaskDetail && state.scheduledTaskDetail.id === id) {
          state.scheduledTaskDetail = Object.assign({}, state.scheduledTaskDetail, { isRunning: true });
        }
      }
      notify();
      refreshScheduledRunShortcutUntilLinked(id, run && run.id);
      return run;
    });
  }

  // 不直接替用户发消息:引导词存为 pending,预填一句短话进输入框,由用户编辑后自己发送。
  async function startScheduledTaskChat() {
    return runScheduledTaskAction("chat-create", async function () {
      const prompt = await invoke("scheduled_task_chat_prompt");
      state.scheduledTaskDraft = null;
      state.scheduledTaskCreationSessionId = null;
      scheduledTaskAutoCreateSeq++; // 清空意图：作废在途 auto-create 的陈旧 completion（审计 f）
      state.scheduledTaskAutoOpenId = null;
      await createNewSession();
      state.scheduledTaskPendingGuide = prompt;
      prefillComposer(bt("scheduledChatPrefill"));
      notify();
      return prompt;
    });
  }

  // ── Chat Items (display format for React) ────────────────────────
  function addChatItem(item) {
    item.id = ++itemIdSeq;
    state.chatItems.push(item);
  }
  function messageHasToolBlock(type, toolCallId) {
    if (!toolCallId) return false;
    // Replayed events belong to the current tail in practice. Scan backwards so
    // long-running Sessions do not pay a full-history walk on every tool frame.
    for (let i = state.messages.length - 1; i >= 0; i--) {
      const blocks = state.messages[i] && state.messages[i].content;
      if (!Array.isArray(blocks)) continue;
      for (let j = blocks.length - 1; j >= 0; j--) {
        const block = blocks[j];
        if (!block || block.type !== type) continue;
        if ((type === "tool_use" ? block.id : block.tool_use_id) === toolCallId) return true;
      }
    }
    return false;
  }
  function toolCallAlreadyStarted(toolCallId) {
    if (!toolCallId) return false;
    if ((pendingAssistantBlocks || []).some(function (block) {
      return block && block.type === "tool_use" && block.id === toolCallId;
    })) return true;
    if (state.chatItems.some(function (item) {
      return item && item.type === "tool" && item.toolId === toolCallId;
    })) return true;
    return messageHasToolBlock("tool_use", toolCallId);
  }
  function toolCallAlreadyFinished(toolCallId) {
    return messageHasToolBlock("tool_result", toolCallId);
  }
  function hasChatItemForTool(type, toolCallId) {
    return !!toolCallId && state.chatItems.some(function (item) {
      return item && item.type === type && item.toolCallId === toolCallId;
    });
  }
  function addSystemItem(text, meta) {
    const item = { type: "system", text, time: timeStr() };
    if (meta) {
      for (const k in meta) item[k] = meta[k];
    }
    addChatItem(item);
    notify();
  }
  function addAuthoritySyncNotice(text) {
    if (state.chatItems.some(function (item) {
      return item && item.authoritySyncNotice;
    })) return;
    addSystemItem(text, { authoritySyncNotice: true });
  }
  function compactPruneRollupText(count) {
    return bt("compactDone") + bt("compactAuto") + " " +
      bt("compactPruneMerged") + " ×" + count;
  }
  function removeCompactionStartItem(compactId) {
    if (!compactId) return;
    for (let i = state.chatItems.length - 1; i >= 0; i--) {
      const it = state.chatItems[i];
      if (it.type === "system" && it.compactId === compactId && it.compactPhase === "start") {
        state.chatItems.splice(i, 1);
        return;
      }
    }
  }
  function addOrMergePruneCompaction(compactId) {
    removeCompactionStartItem(compactId);
    const last = state.chatItems[state.chatItems.length - 1];
    if (last && last.type === "system" && last.compactPruneRollup) {
      last.compactPruneCount = (last.compactPruneCount || 1) + 1;
      last.text = compactPruneRollupText(last.compactPruneCount);
      last.time = timeStr();
      notify();
      return;
    }
    addChatItem({
      type: "system",
      text: compactPruneRollupText(1),
      time: timeStr(),
      compactPruneRollup: true,
      compactPruneCount: 1,
    });
    notify();
  }
  function timeStr() {
    return new Date().toTimeString().slice(0, 5);
  }

  // ── Flush helpers (same as main.js) ──────────────────────────────
  function flushPendingTextBlock() {
    if (pendingAssistantText) {
      pendingAssistantBlocks.push({ type: "text", text: pendingAssistantText });
      pendingAssistantText = "";
    }
  }
  function flushAssistantMessageToHistory() {
    flushPendingTextBlock();
    if (pendingAssistantBlocks.length) {
      const assistantText = pendingAssistantBlocks
        .filter(function (block) { return block && block.type === "text" && block.text; })
        .map(function (block) { return block.text; })
        .join("\n\n");
      if (state.activeSessionId && state.activeSessionId === state.scheduledTaskCreationSessionId) {
        const scheduledTaskDraft = parseScheduledTaskDraftFromText(assistantText);
        if (scheduledTaskDraft) {
          autoCreateScheduledTaskDraft(scheduledTaskDraft, state.activeSessionId);
        }
      }
      state.messages.push({ role: "assistant", content: pendingAssistantBlocks });
      pendingAssistantBlocks = [];
    }
  }
  function resetPendingAssistant() {
    pendingAssistantText = "";
    pendingAssistantBlocks = [];
    currentStreamText = "";
    currentStreamId = 0;
  }

  // ── Session management ───────────────────────────────────────────
  let refreshHistoryInflight = null;
  let refreshHistoryQueued = false;
  async function refreshHistoryList() {
    if (refreshHistoryInflight) {
      refreshHistoryQueued = true;
      return refreshHistoryInflight;
    }
    refreshHistoryInflight = (async function () {
      try {
        do {
          refreshHistoryQueued = false;
          try {
            state.sessions = await invoke(IS_WEB ? "web_access_list_sessions" : "list_sessions");
          } catch (e) {
            console.warn("list_sessions failed", e);
            state.sessions = [];
          }
          try {
            state.archivedSessions = await invoke(
              IS_WEB ? "web_access_list_archived_sessions" : "list_archived_sessions",
            );
          } catch {
            state.archivedSessions = state.archivedSessions || [];
          }
          notify();
        } while (refreshHistoryQueued);
      } finally {
        refreshHistoryInflight = null;
      }
    })();
    return refreshHistoryInflight;
  }

  // 进入草稿态:不创建 session,只清空工作集 + activeSessionId=null,落在「你好」欢迎页。
  // session 在首次有实质内容(发消息 / 加卡牌,见 ensureSession)时才物化——这样会话列表里
  // 永远不会堆积没用过的空「新对话」(ChatGPT/Claude 式 lazy session)。
  function enterDraft() {
    state.chatItems.forEach(function (item) {
      if (item && item.clientMessageId) delete firstTurnSubmissions[item.clientMessageId];
    });
    sessionSwitchRequestToken += 1; // 新建/返回草稿会话使任何仍在等待的 load_session 结果失效
    state.scheduledRunContext = null;
    state.draftEpoch++; // 每次点击都自增——含下面提前返回的「已在草稿态」分支,让前端能重置 welcomeToolId
    state.scheduledTaskPendingGuide = null; // 换了对话,未发送的定时任务引导词作废

    // 已在干净草稿态 → 只 notify(epoch 已自增)。注意要连 chatItems 一起判空:messages 与 chatItems
    // 会背离(persona 气泡 / ensureSession 失败的 system 报错卡只进 chatItems),否则残留卡顶掉「你好」。
    if (!state.activeSessionId && state.messages.length === 0 && state.chatItems.length === 0) {
      state.composerDraft = "";
      // 草稿 mode 显示 = 当前 lane 全局默认（三分 lane 语义）。
      state.modeState = currentDraftModeState();
      notify();
      return;
    }
    if (state.activeSessionId) saveWorkingSetTo(getBuffer(state.activeSessionId));
    state.activeSessionId = null;
    loadWorkingSetFrom(freshBuffer());
    // freshBuffer 的 modeState 是通用缺省（yolo）；草稿显示须覆盖为本 lane
    // 全局默认（work/design 各自的 last_mode）。
    state.modeState = currentDraftModeState();
    notify();
  }
  // 公开「新建对话」入口(侧边栏按钮)= 进草稿态。名字保留以兼容前端调用。
  async function createNewSession() { enterDraft(); }

  // 草稿态首次有实质内容时真正向后端创建 session 并切为 active;已有 active 直接返回。
  // 返回新 session id,创建失败返回 null。调用方:sendMessage(首条消息) / equipPersona(加卡)。
  // 并发防护（审计）：草稿态双击发送会并发 create_session，导致两条消息分家到两个新
  // 会话——in-flight 复用同一 promise；create_session await 期间用户切走会物化在错误
  // 会话（导航被劫持）——物化前校验 activeSessionId 仍为空，已切走则只登记后台 buffer。
  let ensureSessionInFlight = null;
  async function ensureSession() {
    if (state.activeSessionId) return state.activeSessionId;
    if (ensureSessionInFlight) return ensureSessionInFlight;
    // 捕获导航 token：仅判 activeSessionId 覆盖不了「再进草稿」——enterDraft
    // 只推进 token 不改 activeSessionId（仍为 null），在途 create_session 返回
    // 后必须连同 token 一起校验，否则会劫持用户新进的草稿（三审 P1）。
    const navToken = sessionSwitchRequestToken;
    const p = (async function () {
      // 多 session 并发:不预热 engine。新建空 session 的 buffer 由 switchActiveTo({fresh}) 起。
      try {
        const meta = await invoke(IS_WEB ? "web_access_create_session" : "create_session");
        // create_session 等待期间用户可能已发送/清空输入，迁移当下的最新值。
        const composerDraft = state.composerDraft || "";
        // create_session 等待期间用户可能已退出草稿（切到既有会话或再进草稿）：
        // 物化不得劫持 active（审计），新会话登记为后台 buffer 等下次切换，
        // 调用方按 null 处理不发送本条消息。「切到既有会话」→ activeSessionId
        // 非空；「再进草稿」→ activeSessionId 仍为 null 但导航 token 已前移——
        // 两种导航都中止物化（三审 P1）。
        if (state.activeSessionId || navToken !== sessionSwitchRequestToken) {
          const bg = freshBuffer();
          sessionStates[meta.id] = bg;
          bg.loadedFromDisk = true;
          bg.sessionRevision = String(meta.transcript_revision || meta.transcriptRevision || "");
          return null;
        }
        switchActiveTo(meta.id, { fresh: true });
        state.composerDraft = composerDraft;
        sessionStates[meta.id].composerDraft = composerDraft;
        getBuffer(meta.id).sessionRevision = String(meta.transcript_revision || meta.transcriptRevision || "");
        await refreshHistoryList();
        await syncModeState();
        // 三分 lane 语义：后端 plain 缺省恒 Yolo、不区分 work/design 两个 lane；
        // 新会话所在 lane 的全局默认为 plan 时，在物化此刻显式应用（写入即成为
        // 该会话自己的 per-session 记录，全局默认不受影响）。
        const laneDefault = state.modeDefaults
          && state.modeDefaults[state.modeLane === "design" ? "design" : "work"];
        // 用物化时捕获的 meta.id 而非 activeSessionId：上面的 await 期间用户
        // 可能已切走，对当前 active 会话执行 set_plan_mode_next 会改错对象。
        if (laneDefault === "plan") {
          try {
            const laneModeState = await invoke("set_plan_mode_next", { sessionId: meta.id });
            applyAuthoritativeModeState(meta.id, laneModeState);
          } catch (laneModeError) {
            runSyncOnSession(meta.id, function () {
              addSystemItem(bt("switchModeFailed") + laneModeError);
            });
          }
        }
        await syncActivePersona();
        await syncMountedCollection();
        notify();
        // 尾部这些 await 期间用户仍可能切走（activeSessionId 已是别的会话）或
        // 再进草稿（activeSessionId 仍为 null 但 token 已前移）：与 create_session
        // 窗口同一契约——导航即物化中止，返回 null 让调用方放弃，不得返回切走后
        // 的 active 让操作漂进新会话（二审 F1、三审 P1）。返回非 null 时 active
        // 必等于 meta.id 且无任何新导航，调用方重读 state.activeSessionId
        // 即为目标会话。
        return navToken === sessionSwitchRequestToken
          && state.activeSessionId === meta.id ? meta.id : null;
      } catch (e) {
        addSystemItem(bt("newChatFailed") + e);
        return null;
      }
    })();
    ensureSessionInFlight = p;
    p.then(
      function () { if (ensureSessionInFlight === p) ensureSessionInFlight = null; },
      function () { if (ensureSessionInFlight === p) ensureSessionInFlight = null; }
    );
    return p;
  }

  function reportSessionSwitchFailure(error, errorScope) {
    if (errorScope === "scheduled") {
      setScheduledTaskError(error, "navigation");
      notify();
      return;
    }
    addSystemItem(bt("loadChatFailed") + error);
  }

  function invokeOutcome(command, args) {
    return invoke(command, args).then(function (value) {
      return { ok: true, value };
    }, function (error) {
      return { ok: false, error };
    });
  }

  // The transcript is the only navigation-critical payload. Fetch the four
  // presentation-only states in parallel and commit them once, guarded by the
  // switch token so a slow response cannot leak into a newer active Session.
  async function syncSessionPresentationState(sessionId, requestToken) {
    const results = await Promise.all([
      invokeOutcome("get_mode_state", { sessionId }),
      invokeOutcome("get_active_persona", { sessionId }),
      invokeOutcome("session_mounted_collections_snapshot", { sessionId }),
      invokeOutcome("session_mounted_collections", { sessionId }),
      invokeOutcome("session_mounted_collection", { sessionId }),
      invokeOutcome("get_memory_overview", { sessionId }),
    ]);
    if (requestToken !== sessionSwitchRequestToken || state.activeSessionId !== sessionId) return false;

    // 本链路的 modeState 写回同样 bump modeSyncSeq：与会话切换并发在途的
    // syncModeState 读取互不感知（各自独立校验），不 bump 则两条读取链可
    // 互相覆盖（评审 P1）。
    modeSyncSeq += 1;
    // persona/memory 写回同理 bump 各自序号：本通道与会话切换并发在途的
    // syncActivePersona / loadMemoryOverview 读取互不感知（各自独立校验
    // sid+seq），不 bump 则 A→B→A 快速切回时可被旧 sync 响应覆盖(二审补充)。
    personaSyncSeq += 1;
    memoryOverviewSeq += 1;
    const mode = results[0];
    const persona = results[1];
    const snapshot = results[2];
    const collections = results[3];
    const legacyCollection = results[4];
    const memory = results[5];
    state.modeState = mode.ok && mode.value
      ? { mode: mode.value.mode || "yolo", multiAgent: !!mode.value.multi_agent }
      : { mode: "yolo", multiAgent: false };
    state.activePersona = persona.ok ? (persona.value || null) : null;
    if (snapshot.ok && snapshot.value && Array.isArray(snapshot.value.collections)) {
      applyMountedCollections(snapshot.value);
    } else if (collections.ok && Array.isArray(collections.value)) {
      applyMountedCollections(collections.value);
    } else {
      applyMountedCollections(legacyCollection.ok && legacyCollection.value != null ? [legacyCollection.value] : []);
    }
    if (memory.ok) {
      applyMemoryOverview(memory.value);
      rehydratePendingMemoryCandidates(memory.value);
    } else {
      state.memory = Object.assign({}, state.memory, {
        loading: false,
        error: String(memory.error || "memory overview unavailable"),
      });
    }
    notify();
    return true;
  }

  function beginSessionPresentationSync(sessionId, requestToken) {
    state.memory = Object.assign({}, state.memory, { loading: true, error: null });
    return syncSessionPresentationState(sessionId, requestToken).catch(function (error) {
      if (requestToken !== sessionSwitchRequestToken || state.activeSessionId !== sessionId) return;
      state.memory = Object.assign({}, state.memory, { loading: false, error: String(error) });
      notify();
      return false;
    });
  }

  function publishSessionTranscriptReady(options) {
    if (options && typeof options.onTranscriptReady === "function") {
      options.onTranscriptReady();
    }
    notify();
  }

  function hydratedMessageKey(message, hideInternalEnvelope) {
    let blocks = message && Array.isArray(message.content) ? message.content : [];
    if (message && message.role === "user") {
      const resultIds = blocks.filter(function (block) {
        return block && block.type === "tool_result" && block.tool_use_id;
      }).map(function (block) { return block.tool_use_id; }).sort(function (a, b) { return a < b ? -1 : a > b ? 1 : 0; }); // key normalization needs lexicographic order; declare the semantics explicitly
      if (resultIds.length) return "user:tool_results:" + resultIds.join("|");
      return "user:text:" + userMessageDisplayText(blocks, hideInternalEnvelope);
    }
    if (message && message.role === "assistant") {
      const toolIds = blocks.filter(function (block) {
        return block && block.type === "tool_use" && block.id;
      }).map(function (block) { return block.id; }).sort(function (a, b) { return a < b ? -1 : a > b ? 1 : 0; }); // key normalization needs lexicographic order; declare the semantics explicitly
      if (toolIds.length) return "assistant:tool_uses:" + toolIds.join("|");
      blocks = blocks.filter(function (block) { return !block || block.type !== "thinking"; });
      try { return "assistant:" + JSON.stringify(blocks); } catch { /* on serialization failure, persist the raw text */ }
    }
    try { return JSON.stringify(message); } catch { return String(message); }
  }

  function mergeHydratedMessages(durableMessages, liveMessages, hideInternalEnvelope) {
    const durable = Array.isArray(durableMessages) ? [...durableMessages] : [];
    const counts = Object.create(null);
    durable.forEach(function (message) {
      const key = hydratedMessageKey(message, hideInternalEnvelope);
      counts[key] = (counts[key] || 0) + 1;
    });
    (Array.isArray(liveMessages) ? liveMessages : []).forEach(function (message) {
      const key = hydratedMessageKey(message, hideInternalEnvelope);
      if (counts[key]) {
        counts[key] -= 1;
      } else {
        durable.push(message);
      }
    });
    return durable;
  }

  function mergeHydratedArtifacts(durableArtifacts, liveArtifacts) {
    const merged = [];
    const seen = Object.create(null);
    [...(durableArtifacts || []), ...(liveArtifacts || [])].forEach(function (artifact) {
      const path = typeof artifact === "string" ? artifact : (artifact && (artifact.path || artifact.storage_path)) || "";
      const identity = basename(path);
      if (!path || !identity) return;
      if (seen[identity] !== undefined) {
        // The durable transcript commonly stores a relative write_file path while
        // artifact:disk has already supplied the absolute path. They are the same
        // presentation object; retain the path that can actually be opened.
        const existingIndex = seen[identity];
        if (isAbsPath(path) && !isAbsPath(merged[existingIndex].path)) {
          merged[existingIndex] = { path, basename: identity };
        }
        return;
      }
      seen[identity] = merged.length;
      merged.push({ path, basename: identity });
    });
    return merged;
  }

  function hydratedChatItemKey(item) {
    if (!item || !item.type) return "";
    if (item.type === "assistant") return "assistant:" + String(item.html || item.text || "");
    if (item.type === "reasoning") return "reasoning:" + String(item.text || "");
    if (item.type === "tool" && item.toolId) return "tool:" + item.toolId;
    if (item.type === "artifact_card") return "artifact:" + basename(item.path);
    if (item.type === "user_input" && item.toolCallId) return "user_input:" + item.toolCallId;
    if (item.type === "careful_blocked" && item.toolCallId) return "careful_blocked:" + item.toolCallId;
    if (item.type === "plan_card" && item.planId) return "plan:" + item.planId;
    if (item.type === "user") return "user:" + String(item.text || item.html || "");
    if (item.type === "system") return "system:" + String(item.text || "");
    const stable = Object.assign({}, item);
    delete stable.id;
    delete stable.time;
    delete stable.streaming;
    try { return item.type + ":" + JSON.stringify(stable); } catch { return item.type + ":" + String(stable); }
  }

  function mergeHydratedChatItems(liveChatItems, liveCurrentStreamId) {
    let remappedCurrentStreamId = 0;
    const availableByKey = Object.create(null);
    function interruptedDisplayRange(item) {
      if (!item || item.interruptedDisplayOnly !== true) return null;
      let anchorIndex = -1;
      let nextUserIndex = -1;
      const afterMessageIndex = Number(item.afterMessageIndex);
      if (Number.isFinite(afterMessageIndex) && afterMessageIndex >= 0) {
        for (let index = 0; index < state.chatItems.length; index++) {
          const candidate = state.chatItems[index];
          if (!candidate || candidate.type !== "user") continue;
          const candidateMessageIndex = Number(candidate.messageIndex);
          if (candidateMessageIndex === afterMessageIndex) anchorIndex = index;
          else if (anchorIndex >= 0 && candidateMessageIndex > afterMessageIndex) {
            nextUserIndex = index;
            break;
          }
        }
      }
      const afterUserOrdinal = Number(item.afterUserOrdinal);
      if (anchorIndex < 0 && Number.isSafeInteger(afterUserOrdinal) && afterUserOrdinal >= 0) {
        let userOrdinal = -1;
        for (let fallbackIndex = 0; fallbackIndex < state.chatItems.length; fallbackIndex++) {
          const fallback = state.chatItems[fallbackIndex];
          if (!fallback || fallback.type !== "user") continue;
          userOrdinal += 1;
          if (userOrdinal === afterUserOrdinal) anchorIndex = fallbackIndex;
          else if (userOrdinal > afterUserOrdinal) {
            nextUserIndex = fallbackIndex;
            break;
          }
        }
      }
      if (anchorIndex < 0) {
        return { start: state.chatItems.length, end: state.chatItems.length };
      }
      return {
        start: anchorIndex + 1,
        end: nextUserIndex >= 0 ? nextUserIndex : state.chatItems.length,
      };
    }
    state.chatItems.forEach(function (item, index) {
      const key = hydratedChatItemKey(item);
      if (!key) return;
      if (!availableByKey[key]) availableByKey[key] = [];
      availableByKey[key].push(index);
    });
    (liveChatItems || []).forEach(function (item) {
      const key = hydratedChatItemKey(item);
      const range = interruptedDisplayRange(item);
      let existingIndex = -1;
      if (range) {
        for (let rangeIndex = range.start; rangeIndex < range.end; rangeIndex++) {
          const rangeItem = state.chatItems[rangeIndex];
          if (rangeItem && rangeItem.interruptedDisplayOnly !== true &&
              hydratedChatItemKey(rangeItem) === key) {
            existingIndex = rangeIndex;
            break;
          }
        }
      } else {
        const matches = key && availableByKey[key];
        existingIndex = matches && matches.length ? matches.shift() : -1;
      }
      if (existingIndex >= 0) {
        const existingId = state.chatItems[existingIndex].id;
        state.chatItems[existingIndex] = Object.assign({}, state.chatItems[existingIndex], item, {
          id: existingId,
        });
        if (item && item.id === liveCurrentStreamId) remappedCurrentStreamId = existingId;
        return;
      }
      const clone = Object.assign({}, item, { id: ++itemIdSeq });
      if (item && item.id === liveCurrentStreamId) remappedCurrentStreamId = clone.id;
      if (range && range.end < state.chatItems.length) {
        state.chatItems.splice(range.end, 0, clone);
        Object.keys(availableByKey).forEach(function (availableKey) {
          availableByKey[availableKey] = availableByKey[availableKey].map(function (index) {
            return index >= range.end ? index + 1 : index;
          });
        });
      } else state.chatItems.push(clone);
    });
    return remappedCurrentStreamId;
  }

  // eslint-disable-next-line sonarjs/cognitive-complexity -- legacy bridge; refactor tracked separately
  async function switchToSessionInternal(id, preserveScheduledRunContext, errorScope, options) {
    const requestToken = ++sessionSwitchRequestToken;
    settlePendingCapabilitySessionSwitch(false);
    const forceDurableLoad = !!(options && options.forceDurableLoad);
    let hydrateLiveSession = !!(options && options.hydrateLiveSession);
    if (!id) {
      reportSessionSwitchFailure(new Error(bt("runNoSession")), errorScope);
      return false;
    }
    if (hydrateLiveSession && !sessionStates[id]) {
      sessionStates[id] = freshBuffer();
      restoreEvictedSessionDraft(id, sessionStates[id]);
    }
    const existingBuffer = sessionStates[id];
    if (existingBuffer && (existingBuffer.remoteTurnActive ||
        (!existingBuffer.loadedFromDisk &&
          (existingBuffer.busy || existingBuffer.messages.length || existingBuffer.chatItems.length)))) {
      // Events can arrive for a running background Session before this Web
      // client has loaded its durable history. Merge that live tail onto a
      // desktop snapshot instead of treating the partial buffer as complete.
      hydrateLiveSession = true;
    }
    if (id === state.activeSessionId && !forceDurableLoad && !hydrateLiveSession) {
      if (!preserveScheduledRunContext) state.scheduledRunContext = null;
      state.scheduledTaskPendingGuide = null;
      notify();
      return true;
    }
    // 多 session 并发:切换【不再 cancel】旧 session —— 它在自己的 engine 上继续跑,
    // 工作集存进 sessionStates 后台累积。切回来能看到完整(含切走期间产生的)内容。
    // 已有 buffer(切过/在跑)→ 直接换工作集;没有 → load_session 建 buffer + 重渲染。
    if (sessionStates[id] && sessionStates[id].loadedFromDisk &&
        !forceDurableLoad && !hydrateLiveSession) {
      if (!preserveScheduledRunContext) state.scheduledRunContext = null;
      state.scheduledTaskPendingGuide = null; // 仅在目标会话已确认可用后提交导航状态
      switchActiveTo(id, null);
      const cachedPresentationSync = beginSessionPresentationSync(id, requestToken);
      publishSessionTranscriptReady(options);
      await cachedPresentationSync;
      if (requestToken !== sessionSwitchRequestToken || state.activeSessionId !== id) return false;
      reconcileArtifacts(id); // 对账磁盘产物(fire-and-forget)
      return true;
    }
    let saved;
    let personaEvents;
    let pinvouReviews;
    const pinvouSceneEvents = await syncPinvouSceneEventsForSession(id);
    let turnTimeline;
    try {
      const primary = await Promise.all([
        loadSessionForClient(id, !IS_WEB),
        invoke("get_session_persona_events", { sessionId: id }).catch(function () { return []; }),
        invoke("get_session_pinvou_reviews", { sessionId: id }).catch(function () { return []; }),
        invoke("get_session_timeline", { sessionId: id }).catch(function () { return []; }),
      ]);
      saved = primary[0];
      personaEvents = primary[1] || [];
      pinvouReviews = primary[2] || [];
      turnTimeline = primary[3] || [];
    } catch (e) {
      if (IS_WEB && e && e.code === "desktop_capabilities_timeout" &&
          requestToken === sessionSwitchRequestToken) {
        // 能力快照迟到的切换进入宽限等待：快照到达就自动重试，并让原调用方
        // await 到重试的最终结果——scheduled 收尾（terminal 标记/已查看回执/
        // 返回上下文）与 UI 错误状态始终与最终结果一致；宽限期内快照仍未到
        // 达才报错并按失败收口，避免“先报错、后台静默重试成功”的状态分裂。
        const capabilityRetry = {
          spec: {
            id,
            preserveScheduledRunContext,
            errorScope,
            options,
            requestToken,
          },
          resolve: null,
          timer: null,
        };
        // await is redundant in an async function return; passing the Promise through has identical semantics.
        return new Promise(function (resolve) {
          capabilityRetry.resolve = resolve;
          capabilityRetry.timer = setTimeout(function () {
            if (pendingCapabilitySessionSwitch !== capabilityRetry) return;
            if (capabilityRetry.spec.requestToken !== sessionSwitchRequestToken) {
              // 等待期间用户已进入草稿（enterDraft 作废旧切换但未收口 pending）：
              // 按失败静默收口，不在草稿页误报加载失败，与快照到达路径的
              // “被取代的等待不报错”语义保持一致。
              settlePendingCapabilitySessionSwitch(false);
              return;
            }
            reportSessionSwitchFailure(e, errorScope);
            settlePendingCapabilitySessionSwitch(false);
          }, WEB_CAPABILITY_SWITCH_RETRY_GRACE_MS);
          pendingCapabilitySessionSwitch = capabilityRetry;
        });
      }
      if (requestToken === sessionSwitchRequestToken) reportSessionSwitchFailure(e, errorScope);
      return false;
    }
    if (requestToken !== sessionSwitchRequestToken) return false;
    if (!saved || !saved.metadata || !saved.metadata.id) {
      reportSessionSwitchFailure(new Error(bt("sessionDataInvalid")), errorScope);
      return false;
    }

    // load_session 与必要的直接会话数据均成功后，才一次性提交 active/context。
    if (state.activeSessionId) saveWorkingSetTo(getBuffer(state.activeSessionId));
    if (!preserveScheduledRunContext) state.scheduledRunContext = null;
    state.scheduledTaskPendingGuide = null;
    state.activeSessionId = saved.metadata.id;
    if (hydrateLiveSession) {
      const liveBuffer = sessionStates[id] || freshBuffer();
      loadWorkingSetFrom(liveBuffer);
      const liveMessages = Array.isArray(state.messages) ? [...state.messages] : [];
      const liveChatItems = Array.isArray(state.chatItems) ? [...state.chatItems] : [];
      const liveArtifacts = Array.isArray(state.artifacts) ? [...state.artifacts] : [];
      const liveCurrentStreamId = currentStreamId;
      const hasLivePresentation = !!state.busy || !!currentStreamText || !!pendingAssistantText ||
        (Array.isArray(pendingAssistantBlocks) && pendingAssistantBlocks.length > 0);
      state.messages = mergeHydratedMessages(
        saved.messages,
        liveMessages,
        isScheduledRunSession(id)
      );
      state.personaEvents = personaEvents.length ? personaEvents : (liveBuffer.personaEvents || []);
      state.pinvouReviews = pinvouReviews.length ? pinvouReviews : (liveBuffer.pinvouReviews || []);
      state.pinvouSceneEvents = pinvouSceneEvents.length ? pinvouSceneEvents : (liveBuffer.pinvouSceneEvents || []);
      state.turnTimeline = turnTimeline.length ? turnTimeline : (liveBuffer.turnTimeline || []);
      state.artifacts = filterSessionArtifacts(
        mergeHydratedArtifacts(saved.artifacts, liveArtifacts),
        state.activeSessionId
      );
      // Live hydration: toolMeta may hold in-flight tool entries
      // (tool_use not yet in messages); keep them for the later
      // chat:tool_end.
      rerenderFromMessages({ keepLiveToolMeta: true });
      if (hasLivePresentation) {
        currentStreamId = mergeHydratedChatItems(liveChatItems, liveCurrentStreamId);
      } else {
        resetPendingAssistant();
      }
      liveBuffer.loadedFromDisk = true;
      liveBuffer.sessionRevision = String(saved.transcript_revision || saved.transcriptRevision || "");
      saveWorkingSetTo(liveBuffer);
    } else {
      sessionStates[id] = freshBuffer();
      // The slow-path disk rehydration bypasses getBuffer; the draft stashed at eviction time must be restored here.
      restoreEvictedSessionDraft(id, sessionStates[id]);
      loadWorkingSetFrom(sessionStates[id]);
      state.messages = Array.isArray(saved.messages) ? saved.messages : [];
      sessionStates[id].loadedFromDisk = true;
      sessionStates[id].sessionRevision = String(saved.transcript_revision || saved.transcriptRevision || "");
      state.personaEvents = personaEvents;
      state.pinvouReviews = pinvouReviews;
      state.pinvouSceneEvents = pinvouSceneEvents;
      state.turnTimeline = turnTimeline;
      resetPendingAssistant();
      state.chatItems = [];
      state.artifacts = mergeHydratedArtifacts(saved.artifacts, []);
      state.artifacts = filterSessionArtifacts(state.artifacts, state.activeSessionId);
      rerenderFromMessages();
    }
    // The full-load path rebuilds tool cards after the active workspace is
    // selected. Start Shell reconciliation only after that rebuild; polling
    // earlier can attach task ids to a transient card which hydration replaces.
    scheduleShellPoll(id, true);
    const presentationSync = beginSessionPresentationSync(id, requestToken);
    publishSessionTranscriptReady(options);
    await presentationSync;
    if (requestToken !== sessionSwitchRequestToken || state.activeSessionId !== saved.metadata.id) return false;
    reconcileArtifacts(id); // 对账磁盘产物(修重启/跟踪遗漏导致的面板缺文件)
    return true;
  }
  function settlePendingCapabilitySessionSwitch(outcome) {
    const pending = pendingCapabilitySessionSwitch;
    if (!pending) return;
    if (pending.timer) clearTimeout(pending.timer);
    pendingCapabilitySessionSwitch = null;
    pending.resolve(outcome);
  }
  function retryCapabilityBlockedSessionSwitch() {
    const pending = pendingCapabilitySessionSwitch;
    if (!pending || !webInvokeCapabilitiesReady()) return;
    if (pending.spec.requestToken !== sessionSwitchRequestToken) {
      // 已有更新的切换请求：按失败收口旧调用方，不让其悬停等待。
      settlePendingCapabilitySessionSwitch(false);
      return;
    }
    if (pending.timer) clearTimeout(pending.timer);
    pendingCapabilitySessionSwitch = null;
    switchToSessionInternal(
      pending.spec.id,
      pending.spec.preserveScheduledRunContext,
      pending.spec.errorScope,
      pending.spec.options
    ).catch(function () { return false; }).then(function (switched) {
      pending.resolve(switched);
    });
  }

  async function switchToSession(id) {
    return switchToSessionInternal(id, false, "chat");
  }

  async function openScheduledRunChatOnce(run, task) {
    const sessionId = run && typeof run.sessionId === "string" ? run.sessionId.trim() : "";
    if (!sessionId) {
      reportSessionSwitchFailure(new Error(bt("runNoSession")), "scheduled");
      return false;
    }
    rememberScheduledRunOwner(run);
    const runStatus = String(run && run.status || "").toLowerCase();
    let openActivation = null;
    if (runStatus === "queued" || runStatus === "running") {
      openActivation = beginScheduledOpenActivation(sessionId);
    } else {
      scheduledRunBuffer(sessionId);
    }
    setScheduledTaskError(null);
    notify();
    const returnSessionId = state.scheduledRunContext
      ? state.scheduledRunContext.returnSessionId
      : state.activeSessionId;
    const automationId = (run && run.automationId) || (task && task.id) || null;
    const runId = (run && (run.runId || run.id)) || null;
    const scheduledContext = {
      sessionId,
      returnSessionId,
      automationId,
      runId,
      taskName: (task && task.name) || (run && (run.taskName || run.name)) || "",
      model: (task && task.model) || (run && run.taskModel) || null,
      mode: "yolo",
    };
    const liveBuffer = sessionStates[sessionId];
    const hasLiveTurn = !!(liveBuffer && (
      liveBuffer.busy ||
      liveBuffer.scheduledInitialTurnPhase === "active" ||
      (liveBuffer.queued && liveBuffer.queued.length) ||
      (liveBuffer.thinking && liveBuffer.thinking.active)
    ));
    const isTerminalRun = ["completed", "failed", "canceled"].includes(runStatus);
    const forceDurableLoad = isTerminalRun && !hasLiveTurn;
    const switched = await switchToSessionInternal(sessionId, true, "scheduled", {
      forceDurableLoad,
      hydrateLiveSession: !isTerminalRun,
      onTranscriptReady: function () {
        state.scheduledRunContext = scheduledContext;
      },
    });
    if (!switched) {
      rollbackScheduledOpenActivation(openActivation);
      notify();
      return false;
    }
    if (forceDurableLoad) markScheduledInitialTurnTerminal(sessionId);
    else scheduledRunBuffer(sessionId);
    // 先发布完整会话视图；只有已完成的运行才持久化为已查看。
    notify();
    if (automationId && runId && runStatus === "completed") {
      try {
        const receipt = await invoke("mark_scheduled_run_viewed", {
          automationId,
          runId,
        });
        invalidateScheduledTaskReads(automationId);
        applyScheduledRunViewed(automationId, runId, receipt);
      } catch (e) {
        setScheduledTaskError(e, "action");
      }
    }
    notify();
    return true;
  }

  function openScheduledRunChat(run, task) {
    const sessionId = run && typeof run.sessionId === "string" ? run.sessionId.trim() : "";
    if (!sessionId) return openScheduledRunChatOnce(run, task);
    if (scheduledRunOpenInFlight[sessionId]) return scheduledRunOpenInFlight[sessionId];
    const opening = openScheduledRunChatOnce(run, task);
    scheduledRunOpenInFlight[sessionId] = opening;
    function clearOpening() {
      if (scheduledRunOpenInFlight[sessionId] === opening) {
        delete scheduledRunOpenInFlight[sessionId];
      }
    }
    opening.then(clearOpening, clearOpening);
    return opening;
  }

  async function exitScheduledRunChat() {
    const context = state.scheduledRunContext;
    if (!context) return false;
    if (context.returnSessionId && context.returnSessionId !== context.sessionId) {
      const restored = await switchToSessionInternal(context.returnSessionId, true, "scheduled");
      if (restored) {
        state.scheduledRunContext = null;
        notify();
        return true;
      }
      return false;
    }
    enterDraft();
    return true;
  }

  function recentScheduledRunForSession(id) {
    return (state.scheduledTaskRecentRuns || []).find(function (run) {
      return run && run.sessionId === id;
    }) || null;
  }

  // 离开正在查看的会话:清 active + 换空工作集,并清掉指向它的定时运行上下文。
  // 必须连 scheduledRunContext 一起清 —— main.jsx 只按该字段真值决定渲染
  // ChatView 还是 ScheduledTasksView,而 ChatView 内部还要求 sessionId===activeSessionId
  // 才渲染返回按钮;只清 active 会卡在「定时路由下的空白页且没有返回按钮」。
  // 清掉之后 currentView 仍是 'scheduled',界面自然落回定时任务列表。
  // 不负责 buffer:删除要丢弃 buffer,收纳要保留 buffer,由调用方各自处理。
  function leaveSessionView(id) {
    if (state.scheduledRunContext && state.scheduledRunContext.sessionId === id) {
      state.scheduledRunContext = null;
    }
    if (state.activeSessionId !== id) return;
    state.activeSessionId = null;
    loadWorkingSetFrom(freshBuffer());
  }

  // Session storage is authoritative in Rust, but every desktop/Web frontend
  // owns a separate presentation index. Apply the committed deletion event
  // idempotently in either client so a remote delete cannot leave an ENOENT
  // sidebar row behind in the other one.
  function applyDeletedSession(id) {
    if (typeof id !== "string" || !id) return false;
    invalidateScheduledRecentRunsForSession(id);
    purgeSessionBuffer(id);
    state.sessions = state.sessions.filter(function (session) { return session.id !== id; });
    state.archivedSessions = (state.archivedSessions || []).filter(function (session) {
      return session.id !== id;
    });
    state.scheduledTaskRecentRuns = (state.scheduledTaskRecentRuns || []).filter(function (run) {
      return !run || run.sessionId !== id;
    });
    state.scheduledTaskRuns = (state.scheduledTaskRuns || []).filter(function (run) {
      return !run || run.sessionId !== id;
    });
    notify();
    return true;
  }

  async function deleteSession(id) {
    invalidateScheduledRecentRunsForSession(id);
    try {
      // 后端按 SessionKind 分发:定时运行会话在 delete_session 里联动删除
      // 该次 Session、Run 与底座 Task,任务定义与共享工作间保留。
      await invoke("delete_session", { id });
      applyDeletedSession(id);
      return true;
    } catch (e) {
      addSystemItem(bt("deleteFailed") + e);
      return false;
    }
  }

  async function renameSession(id, title) {
    invalidateScheduledRecentRunsForSession(id);
    try {
      await invoke("rename_session", { id, title });
      const s = state.sessions.find(function (s) { return s.id === id; });
      if (s) s.title = title;
      state.scheduledTaskRecentRuns = (state.scheduledTaskRecentRuns || []).map(function (run) {
        return run && run.sessionId === id ? Object.assign({}, run, { sessionTitle: title }) : run;
      });
      delete personaPlaceholderTitles[id]; // 用户主动命名后不再算卡牌占位,不被对话覆盖
      notify();
    } catch (e) {
      console.warn("rename failed", e);
    }
  }

  async function toggleSessionPinned(id, pinned) {
    invalidateScheduledRecentRunsForSession(id);
    const s = state.sessions.find(function (s) { return s.id === id; });
    const scheduledRun = recentScheduledRunForSession(id);
    const prev = s ? !!s.pinned : false;
    const prevPinnedAt = s ? s.pinned_at : null;
    const previousRunPinned = scheduledRun ? !!scheduledRun.pinned : false;
    const previousRunPinnedAt = scheduledRun ? scheduledRun.pinnedAt : null;
    if (s) {
      s.pinned = !!pinned;
      s.pinned_at = pinned ? new Date().toISOString() : null;
    }
    if (scheduledRun) {
      scheduledRun.pinned = !!pinned;
      scheduledRun.pinnedAt = pinned ? new Date().toISOString() : null;
    }
    notify();
    try {
      await invoke("set_session_pinned", { id, pinned: !!pinned });
      await refreshHistoryList();
    } catch (e) {
      if (s) {
        s.pinned = prev;
        s.pinned_at = prevPinnedAt;
      }
      if (scheduledRun) {
        scheduledRun.pinned = previousRunPinned;
        scheduledRun.pinnedAt = previousRunPinnedAt;
      }
      console.warn("set_session_pinned failed", e);
      await refreshHistoryList();
    }
  }

  async function archiveSession(id) {
    invalidateScheduledRecentRunsForSession(id);
    const idx = state.sessions.findIndex(function (s) { return s.id === id; });
    if (idx < 0) {
      // 定时运行会话不在 state.sessions;收起 = 从侧边栏记录移除,进设置页归档列表。
      const scheduledRun = recentScheduledRunForSession(id);
      // Codex 等独立会话也不在 state.sessions；交给后端判定并刷新统一历史列表。
      if (!scheduledRun) {
        try {
          await invoke("set_session_archived", { id, archived: true });
          await refreshHistoryList();
          return true;
        } catch (e) {
          console.warn("set_session_archived failed", e);
          return false;
        }
      }
      const previousRuns = state.scheduledTaskRecentRuns || [];
      const wasViewingRun = state.activeSessionId === id;
      const previousContext = state.scheduledRunContext;
      // 归档等待期间的导航 token：失败回滚时「activeSessionId === null」不足以
      // 证明无新导航——用户再进草稿也保持 null（enterDraft 只推进 token），
      // 仅 token 未前移才允许把 active 拽回归档会话（三审 P1）。
      const navToken = sessionSwitchRequestToken;
      // 与普通会话收纳同语义:保留 buffer(还能从设置页还原后重开),但要离开当前视图。
      if (wasViewingRun) saveWorkingSetTo(getBuffer(id));
      state.scheduledTaskRecentRuns = previousRuns.filter(function (run) {
        return !run || run.sessionId !== id;
      });
      leaveSessionView(id);
      notify();
      try {
        await invoke("set_session_archived", { id, archived: true });
        await refreshHistoryList();
        return true;
      } catch (e) {
        state.scheduledTaskRecentRuns = previousRuns;
        // 回滚 active 仅当用户没有新导航（leaveSessionView 已置 null）：
        // await 期间切到别的会话/再进草稿都不得劫持 active（审计、三审 P1）。
        if (wasViewingRun && state.activeSessionId === null
            && navToken === sessionSwitchRequestToken) {
          // active 与 scheduledRunContext 必须成对回滚,否则会落到
          // 「active 有值但 context 空」的错位态(界面回任务列表却仍持有会话)。
          state.activeSessionId = id;
          state.scheduledRunContext = previousContext;
          loadWorkingSetFrom(getBuffer(id));
        }
        console.warn("set_session_archived failed", e);
        notify();
        return false;
      }
    }
    const s = state.sessions[idx];
    const archived = Object.assign({}, s, { archived: true, archived_at: new Date().toISOString(), pinned: false, pinned_at: null });
    const wasActive = state.activeSessionId === id;
    // 与 scheduled 分支同源：失败回滚须以导航 token 证明「无新导航」——
    // 归档等待期间再进草稿 activeSessionId 仍为 null（三审 P1）。
    const navToken = sessionSwitchRequestToken;
    if (wasActive) saveWorkingSetTo(getBuffer(id));
    state.sessions.splice(idx, 1);
    state.archivedSessions = [archived, ...(state.archivedSessions || []).filter(function (x) { return x.id !== id; })];
    leaveSessionView(id);
    notify();
    try {
      await invoke("set_session_archived", { id, archived: true });
      await refreshHistoryList();
      return true;
    } catch (e) {
      state.sessions.splice(idx, 0, s);
      state.archivedSessions = (state.archivedSessions || []).filter(function (x) { return x.id !== id; });
      // 回滚 active 仅当用户没有新导航（leaveSessionView 已置 null）：
      // await 期间切到别的会话/再进草稿都不得劫持 active（审计、三审 P1）。
      if (wasActive && state.activeSessionId === null
          && navToken === sessionSwitchRequestToken) {
        state.activeSessionId = id;
        loadWorkingSetFrom(getBuffer(id));
      }
      console.warn("set_session_archived failed", e);
      notify();
      return false;
    }
  }

  async function restoreArchivedSession(id) {
    const idx = (state.archivedSessions || []).findIndex(function (s) { return s.id === id; });
    if (idx < 0) return false;
    const s = state.archivedSessions[idx];
    invalidateScheduledRecentRunsForSession(id);
    const restored = Object.assign({}, s, { archived: false, archived_at: null });
    state.archivedSessions.splice(idx, 1);
    state.sessions = [restored, ...(state.sessions || [])];
    notify();
    try {
      await invoke("set_session_archived", { id, archived: false });
      await refreshHistoryList();
      // 还原的定时运行会话回侧边栏"定时任务记录"(refreshHistoryList 只管普通会话)。
      if (String(id).indexOf("sched-") === 0) loadScheduledTaskRecentRuns().catch(function () {});
      return true;
    } catch (e) {
      state.archivedSessions.splice(idx, 0, s);
      state.sessions = (state.sessions || []).filter(function (x) { return x.id !== id; });
      console.warn("restore archived session failed", e);
      notify();
      return false;
    }
  }

  // 实时态有专属气泡的工具（方案卡），重建时要还原成原卡而非普通工具卡。
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
  // isInternalRuntimeUserMessage（下方 live 路径同一判定）与本函数等价：
  // 历史重载与实时事件两条展示路径共用同一信封判定，避免两处实现漂移。

  // Engine 的运行时恢复提示为了兼容模型协议会以 role=user 持久化，但它不是用户输入。
  // 子智能体完成交接同理：结果必须留在父模型上下文，但不能冒充用户消息上屏。
  // 原始 blocks 必须保留给模型续聊；展示层只隐藏该内部消息，避免伪装成用户气泡/新 Turn。
  // 定时会话还会过滤送模 envelope，只投影真实任务正文。
  function userMessageDisplayText(blocks, hideInternalEnvelope) {
    const textParts = (Array.isArray(blocks) ? blocks : [])
      .filter(function (block) { return block && block.type === "text"; })
      .map(function (block) { return String(block.text || ""); });
    if (textParts.some(isInternalRuntimeUserMessage)) return "";
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
  // (hydrateLiveSession) — toolMeta may hold entries for in-flight
  // tools (tool_use not yet in messages), and clearing it would leave
  // the later chat:tool_end without its meta (stuck selection card /
  // degraded artifact card).
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
        const utext = userMessageDisplayText(blocks, isScheduledRunSession(state.activeSessionId));
        if (utext) {
          // pinvouTransfer 是展示层标记、不在 messages → rerender 从转交固定措辞还原品/悟样式
          const uitem2 = { type: "user", text: utext, time: "", messageIndex: mi };
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
              addChatItem({
                type: "careful_blocked", toolCallId: c.tool_use_id,
                args: tm.args, metadata: blockedMd, time: "",
              });
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

  function updateToolItem(toolId, output, success) {
    for (let i = 0; i < state.chatItems.length; i++) {
      if (state.chatItems[i].type === "tool" && state.chatItems[i].toolId === toolId) {
        state.chatItems[i].output = output;
        state.chatItems[i].success = success;
        state.chatItems[i].state = success ? "done" : "failed";
        return state.chatItems[i];
      }
    }
    return null;
  }

  const SHELL_TOOL_NAMES = ["exec_shell", "task_shell_start", "shell", "Bash"];

  function isShellExecutionTool(name) {
    return SHELL_TOOL_NAMES.includes(name);
  }

  function mentionsShellTool(text) {
    // 子智能体的工具调用不产生 chat:tool_start，forwarder 把 mailbox 的
    // ToolCallStarted 转成 multiagent:agent_progress（status 形如
    // "🔧 exec_shell (step 3)"）。据此调度快照轮询，让子 agent 的后台
    // shell 任务被 applyShellSnapshots 发现。
    const raw = String(text || "");
    return SHELL_TOOL_NAMES.some((name) => raw.includes(name));
  }

  function utf8Length(text) {
    try { return new TextEncoder().encode(String(text || "")).length; }
    catch { return String(text || "").length; }
  }

  // Shell snapshots are a tail view, not an append-only byte stream. Normalize
  // terminal control sequences and state omissions explicitly instead of
  // pretending the visible tail is the complete log.
  function normalizeTerminalTail(text) {
    // terminal control sequences are protocol content themselves (OSC/CSI/DEL); match them verbatim. See the eslint-disable-line comments below.
    const value = String(text || "")
      // biome-ignore lint/suspicious/noControlCharactersInRegex: protocol delimiters: ESC/BEL/ST are terminal protocol bytes, same rationale as the eslint comment on the same line
      .replaceAll(/\x1B\][^\x07]*(?:\x07|\x1B\\)/g, "") // eslint-disable-line no-control-regex, sonarjs/no-control-regex -- OSC terminator (BEL/ST) and ESC are terminal protocol bytes
      // biome-ignore lint/suspicious/noControlCharactersInRegex: protocol delimiters: CSI sequences follow the terminal protocol definition
      .replaceAll(/\x1B\[[0-?]*[ -/]*[@-~]/g, ""); // eslint-disable-line no-control-regex -- CSI sequences follow the protocol definition
    const out = [];
    value.split("\n").forEach(function (line) {
      // After splitting on LF, a normal Windows CRLF line still ends in CR.
      // Remove that delimiter first; only an *internal* CR means a terminal
      // progress line overwrote earlier content on the same row.
      let visible = line.endsWith("\r") ? line.slice(0, -1) : line;
      const overwriteAt = visible.lastIndexOf("\r");
      if (overwriteAt >= 0) visible = visible.slice(overwriteAt + 1);
      while (visible.includes("\x08")) {
        // biome-ignore lint/suspicious/noControlCharactersInRegex: protocol delimiters: backspace is a terminal protocol byte
        visible = visible.replaceAll(/[^\x08]\x08/g, "").replace(/^\x08+/, ""); // eslint-disable-line no-control-regex, sonarjs/no-control-regex -- backspace is a terminal protocol byte
      }
      out.push(visible);
    });
    return out.join("\n");
  }

  function formatShellSnapshot(job) {
    function section(raw, total, kind) {
      raw = String(raw || "");
      const visibleRaw = raw.replace(/^\.\.\.\s*/, "");
      const omitted = /^\.\.\./.test(raw) || Number(total || 0) > utf8Length(visibleRaw);
      let body = normalizeTerminalTail(visibleRaw);
      if (omitted) body = bt("shellOutputOmitted")(kind) + "\n" + body;
      return body;
    }
    const stdout = section(job.stdout_tail, job.stdout_len, "stdout");
    const stderr = section(job.stderr_tail, job.stderr_len, "stderr");
    const parts = [];
    if (stdout) parts.push(stdout);
    if (stderr) parts.push((stdout ? "[STDERR]\n" : "") + stderr);
    if (String(job.status || "").toLowerCase() !== "running") {
      const code = job.exit_code == null ? bt("shellUnknownExit") : String(job.exit_code);
      parts.push(bt("shellTaskFinished")(code));
    }
    return parts.join("\n");
  }

  function shellCommandForItem(item) {
    return item && item.args && typeof item.args.command === "string" ? item.args.command : "";
  }

  function shellSnapshotKey(job) {
    return JSON.stringify([
      job.id, job.status, job.exit_code, job.stdout_len, job.stderr_len,
      job.stdout_tail, job.stderr_tail,
    ]);
  }

  function terminalShellHistoryMatch(item, job) {
    if (!item || item.type !== "tool" || item.taskId || item.state === "running" ||
        !isShellExecutionTool(item.name) || shellCommandForItem(item) !== String(job.command || "")) {
      return false;
    }
    const output = normalizeTerminalTail(String(item.output || ""));
    if (output.includes(String(job.id || "")) && job.id) return true;
    const evidence = [job.stdout_tail, job.stderr_tail].map(function (raw) {
      return normalizeTerminalTail(String(raw || "").replace(/^\.\.\.\s*/, "")).trim();
    }).filter(Boolean);
    if (evidence.length) return evidence.every(function (text) { return output.includes(text); });
    return /\(no output\)|no output|无输出|出力なし/i.test(output);
  }

  function applyShellSnapshots(sid, jobs) {
    let anyRunning = false;
    let changed = false;
    const runningCommandCounts = {};
    (jobs || []).forEach(function (job) {
      if (String(job.status || "").toLowerCase() !== "running") return;
      const command = String(job.command || "");
      runningCommandCounts[command] = (runningCommandCounts[command] || 0) + 1;
    });
    runSyncOnSession(sid, function () {
      (jobs || []).forEach(function (job) {
        const status = String(job.status || "").toLowerCase();
        const running = status === "running";
        if (running) anyRunning = true;
        let item = state.chatItems.find(function (it) {
          return it.type === "tool" && it.taskId === job.id;
        });
        if (!item && running) {
          const command = String(job.command || "");
          const candidates = state.chatItems.filter(function (it) {
            return it.type === "tool" && isShellExecutionTool(it.name) && !it.taskId &&
              it.state === "running" && shellCommandForItem(it) === command;
          });
          // Command text is only a temporary bridge until tool_end exposes the
          // task id. Never guess when identical commands are concurrent.
          if (runningCommandCounts[command] === 1 && candidates.length === 1) item = candidates[0];
        }
        if (!item && !running) {
          item = state.chatItems.find(function (it) {
            return terminalShellHistoryMatch(it, job);
          });
          if (item) item.shellHistoryReconciled = true;
        }
        // A detached job may have been started by a subagent, so no matching
        // top-level tool card exists. Completed jobs must also get a card: the
        // first poll may happen after a short detached process already exited.
        if (!item) {
          item = {
            type: "tool", toolId: "shell-task:" + job.id, name: "exec_shell",
            args: { command: job.command || "" }, output: null, success: null,
            state: running ? "running" : "failed", shellSnapshot: true,
          };
          addChatItem(item);
          changed = true;
        }
        const snapshotKey = shellSnapshotKey(job);
        if (item.shellSnapshotKey === snapshotKey) return;
        item.taskId = job.id;
        item.sessionId = sid;
        item.shellStatus = job.status;
        item.exitCode = job.exit_code;
        item.elapsedMs = job.elapsed_ms;
        if (!item.shellHistoryReconciled || item.output == null || running) {
          item.output = formatShellSnapshot(job);
        }
        item.state = running ? "running" : (status === "completed" ? "done" : "failed");
        item.success = running ? null : status === "completed";
        item.shellSnapshotKey = snapshotKey;
        changed = true;
      });
    });
    if (changed) notify();
    return anyRunning;
  }

  function scheduleShellPoll(sid, immediate) {
    if (!sid) return;
    if (!shellPollState[sid]) shellPollState[sid] = {
      timer: null, inFlight: false, waitBudget: 0,
    };
    const poll = shellPollState[sid];
    poll.waitBudget = Math.max(poll.waitBudget, 12);
    if (poll.timer || poll.inFlight) return;
    poll.timer = setTimeout(function () { runShellPoll(sid); }, immediate ? 0 : 250);
  }

  async function runShellPoll(sid) {
    const poll = shellPollState[sid];
    if (!poll || poll.inFlight) return;
    poll.timer = null;
    poll.inFlight = true;
    let running = false;
    try {
      const jobs = await invoke("list_shell_tasks", { sessionId: sid });
      running = applyShellSnapshots(sid, Array.isArray(jobs) ? jobs : []);
      if (!running) poll.waitBudget = Math.max(0, poll.waitBudget - 1);
    } catch (error) {
      console.warn("shell task polling failed", error);
      poll.waitBudget = Math.max(0, poll.waitBudget - 1);
    } finally {
      poll.inFlight = false;
    }
    if (running || poll.waitBudget > 0) {
      poll.timer = setTimeout(function () { runShellPoll(sid); }, 250);
    } else {
      delete shellPollState[sid];
    }
  }

  async function cancelShellTask(sessionId, taskId) {
    const sid = sessionId || state.activeSessionId;
    if (!sid || !taskId) return;
    try {
      await invoke("cancel_shell_task", { sessionId: sid, taskId });
    } finally {
      scheduleShellPoll(sid, true);
    }
  }

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

  // ── 产物跟踪 ─────────────────────────────────────────────────────
  function basename(p) {
    if (!p) return "";
    const parts = String(p).split(/[\\/]/);
    return parts[parts.length - 1] || p;
  }
  function isAbsPath(p) {
    return typeof p === "string" && (p.charAt(0) === "/" || /^[A-Za-z]:[\\/]/.test(p));
  }
  function normalizedPath(p) {
    return String(p || "").replaceAll('\\', "/");
  }
  function noteArtifactChange(path, event, sessionId) {
    if (!path) return;
    state.artifactChange = {
      seq: (state.artifactChange && state.artifactChange.seq || 0) + 1,
      path,
      event: event || "modified",
      sessionId: sessionId || "",
      at: Date.now(),
    };
    notify();
  }
  function isSharedMcpArtifactPath(path) {
    return normalizedPath(path).includes("/sessions/default/artifacts/");
  }
  function artifactBelongsToSession(path, sid) {
    if (!path || !sid) return false;
    if (!isAbsPath(path)) return true;
    if (isSharedMcpArtifactPath(path)) return true;
    const normalized = normalizedPath(path);
    if (normalized.includes("/sessions/")) {
      return normalized.includes("/sessions/" + sid + "/workspace/") ||
        normalized.includes("/sessions/" + sid + "/artifacts/");
    }
    return true;
  }
  function filterSessionArtifacts(artifacts, sid) {
    return (Array.isArray(artifacts) ? artifacts : []).filter(function (a) {
      return artifactBelongsToSession(a && a.path, sid);
    });
  }
  // 「成品型」扩展名:write_file 写出这类文件即自动当成品进面板(模型常忘 present_artifact)。
  // 办公文档 + markdown 报告 + 数据表 + 图片 + 打包件都算成品(覆盖 AI 常见产出格式)。
  // 中间/草稿(.txt/.json/.xml 等)刻意不在此列 → 不进面板,避免一堆过程文件污染产物列表;
  // 这类格式若确是成品,靠模型 present_artifact 显式挂出(present 过的不受扩展名门控)。
  const DELIVERABLE_EXTS = new Set([
    "pptx", "ppt", "docx", "doc", "pdf", "html", "htm", "xlsx", "xls",
    "md", "csv", "png", "jpg", "jpeg", "svg", "gif", "webp", "zip",
  ]);
  // tmp/ 是提示词约定的中间文件目录(instructions.md:中间/临时文件一律写 tmp/,
  // 不进产出物列表)。tmp/ 下的文件即使扩展名是成品型(.md/.html 等)也不算自动成品;
  // 确需展示只能靠模型显式 present_artifact(显式 present 不经 isDeliverable 门控)。
  function isTmpPath(path) {
    const segs = normalizedPath(path).split("/");
    for (let i = 0; i < segs.length; i++) {
      if (segs[i] === "tmp") return true;
    }
    return false;
  }
  function isDeliverable(path) {
    if (isTmpPath(path)) return false;
    const ext = (String(path || "").split(".").pop() || "").toLowerCase();
    return DELIVERABLE_EXTS.has(ext);
  }
  function trackArtifact(path) {
    if (!path) return;
    if (state.activeSessionId && !artifactBelongsToSession(path, state.activeSessionId)) return;
    const bn = basename(path);
    for (let i = 0; i < state.artifacts.length; i++) {
      if (basename(state.artifacts[i].path) === bn) {
        // 已有同名:write_file 跟踪的是相对路径、disk watcher 推的是绝对路径——同一文件
        // 两种 path 会重复。新 path 绝对而旧的相对则用绝对替换(open 可靠),否则忽略重复。
        if (isAbsPath(path) && !isAbsPath(state.artifacts[i].path)) {
          state.artifacts[i] = { path, basename: bn };
          notify();
        }
        return;
      }
    }
    state.artifacts.push({ path, basename: bn });
    notify();
  }
  function markTurnDirtyArtifact(path) {
    const bn = basename(path);
    if (!bn) return;
    if ((state.turnDirtyArtifacts || []).some(function (p) { return basename(p) === bn; })) return;
    state.turnDirtyArtifacts.push(path);
  }
  function untrackArtifact(path) {
    const before = state.artifacts.length;
    state.artifacts = state.artifacts.filter(function (a) { return a.path !== path; });
    if (state.artifacts.length !== before) notify();
  }
  // Prefer an exact normalized path so an older card is not hidden by a newer
  // same-named artifact from another directory. Fall back to the basename for
  // persisted relative paths that must reconcile with an absolute watcher path.
  function findPresentedArtifact(path) {
    const bn = basename(path);
    if (!bn) return null;
    const normalized = normalizedPath(path);
    let basenameMatch = null;
    for (let i = state.chatItems.length - 1; i >= 0; i--) {
      const it = state.chatItems[i];
      if (it.type !== "artifact_card" || basename(it.path) !== bn) continue;
      if (normalizedPath(it.path) === normalized) return it;
      if (!basenameMatch) basenameMatch = it;
    }
    return basenameMatch;
  }
  // Updates an existing presentation card in place (stable id and position)
  // instead of appending a duplicate card. Returns null when the caller must
  // append a fresh card instead:
  // - no card for this basename yet, or the matching card's absolute path
  //   differs from the presented path (same-named files in different
  //   directories are distinct artifacts, never rewritten into each other);
  // - a user message is newer than the existing card with no file mutation
  //   after it: the model is answering a fresh "show it again" request, and
  //   replaying that turn must stay a visible new card.
  function updatePresentedArtifact(card) {
    if (!card || !card.path) return null;
    const existing = findPresentedArtifact(card.path);
    if (!existing) return null;
    // A relative tool path may be the only bridge between a persisted relative
    // card and its absolute watcher path. When it contains no resolvable
    // directory, same-named files remain ambiguous, so retain the basename
    // fallback for backward compatibility and rely on abs_path when available.
    if (isAbsPath(existing.path) && isAbsPath(card.path) &&
        normalizedPath(existing.path) !== normalizedPath(card.path)) return null;
    const bn = basename(card.path);
    for (let i = state.chatItems.length - 1; i >= 0; i--) {
      const it = state.chatItems[i];
      if (it === existing) break;
      if (it.type === "user") return null;
      if (it.type === "tool" && fileMutationAction(it.name, it.args) &&
          extractArtifactPaths(it.args).some(function (ap) { return basename(ap) === bn; })) break;
    }
    const stableId = existing.id;
    const stableAbsolutePath = isAbsPath(existing.path) && !isAbsPath(card.path)
      ? existing.path
      : null;
    Object.assign(existing, card, { type: "artifact_card" });
    if (stableId !== undefined) existing.id = stableId;
    if (stableAbsolutePath) existing.path = stableAbsolutePath;
    return existing;
  }
  // 切换 session 时对账:扫 workspace 磁盘,把实际存在、但跟踪列表里没有的文件补进来。
  // 修「文件已生成在盘上、却因 app 中途重启/跟踪遗漏而不在产物面板」(以磁盘为准)。
  async function reconcileArtifacts(sid) {
    if (!sid) return;
    if (isScheduledRunSession(sid)) return;
    try {
      const files = await invoke("list_workspace_files", { sessionId: sid });
      if (sid !== state.activeSessionId) return; // 已切走,放弃(避免写错 session)
      const byName = {};
      state.artifacts.forEach(function (a) { byName[basename(a.path)] = a; });
      let added = false;
      files.forEach(function (p) {
        const bn = basename(p);
        const ex = byName[bn];
        // 已 present_artifact 过的成品在 saved.artifacts(ex 命中);扫盘只「新增」成品型文件,
        // 不再把所有过程文件全扫进面板(修「飞书 CLI scratch 全暴露成产物」)。
        if (!ex) {
          if (!isDeliverable(p)) return;
          const na = { path: p, basename: bn }; state.artifacts.push(na); byName[bn] = na; added = true;
        }
        else if (isAbsPath(p) && !isAbsPath(ex.path)) { ex.path = p; added = true; } // 相对→绝对,open 可靠
      });
      if (added) {
        notify();
        try { await invoke("save_session_artifacts", { id: sid, paths: state.artifacts.map(function (a) { return a.path; }) }); } catch { /* persistence failure must not block frontend updates */ }
      }
    } catch { /* workspace 不存在(新 session)等,忽略 */ }
  }
  function pushArtifactPath(paths, path) {
    if (typeof path !== "string" || !path.trim()) return;
    path = path.trim();
    if (!paths.includes(path)) paths.push(path);
  }
  function extractArtifactPaths(args) {
    if (!args) return [];
    if (typeof args === "string") {
      try { args = JSON.parse(args); } catch { return []; }
    }
    const paths = [];
    pushArtifactPath(paths, args.path || args.file_path || args.filename);
    [args.replace, args.changes].forEach(function (changes) {
      if (!Array.isArray(changes)) return;
      changes.forEach(function (change) {
        if (change && typeof change === "object") pushArtifactPath(paths, change.path || change.file_path || change.filename);
      });
    });
    String(args.patch || "").split(/\r?\n/).forEach(function (line) {
      // eslint-disable-next-line sonarjs/super-linear-regex -- input is split by line and line length is bounded by the patch header, so the backtracking upper bound is negligible
      const custom = /^\*\*\* (?:Add|Update|Delete) File:\s*(.+?)\s*$/.exec(line);
      if (custom) { pushArtifactPath(paths, custom[1]); return; }
      // eslint-disable-next-line sonarjs/super-linear-regex -- input is split by line and line length is bounded by the patch header, so the backtracking upper bound is negligible
      const unified = /^\+\+\+\s+(?:b\/)?(.+?)\s*$/.exec(line);
      if (unified && unified[1] !== "/dev/null") pushArtifactPath(paths, unified[1]);
    });
    return paths;
  }
  function extractArtifactPath(args) {
    return extractArtifactPaths(args)[0] || null;
  }

  function fileMutationAction(name, args) {
    if (typeof args === "string") {
      try { args = JSON.parse(args); } catch { args = null; }
    }
    if (String(name || "").toLowerCase() === "file") {
      const action = String(args && args.action || "").toLowerCase();
      return ["write", "edit", "patch"].includes(action) ? action : null;
    }
    if (name === "write_file") return "write";
    if (name === "edit_file") return "edit";
    return null;
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

  // ── Send message ─────────────────────────────────────────────────
  // 指定 session 是否正在生成(active 看工作集 busy,后台看其 buffer)。
  function isBusyFor(sid) {
    return sid === state.activeSessionId ? state.busy : !!(sessionStates[sid] && sessionStates[sid].busy);
  }
  function formatAttachmentDisplayText(text, attachments) {
    const names = (attachments || []).map(function (attachment) {
      return typeof attachment === "string" ? attachment : attachment && attachment.basename;
    }).filter(Boolean).map(String);
    if (!names.length) return String(text || "");
    const attachmentLine = "📎 " + JSON.stringify(names);
    return String(text || "").trim()
      ? String(text) + "\n\n" + attachmentLine
      : attachmentLine;
  }
  function queuedPayloadEnvelope(userText, payloadText, meta) {
    const user = String(userText || "");
    const payload = String(payloadText == null ? user : payloadText);
    if (!user) return { before: payload, after: "" };
    const requested = meta && meta.pinvouPayloadText
      ? String(meta.pinvouPayloadText).trim()
      : "";
    let index = -1;
    if (requested) {
      const requestedIndex = payload.indexOf(requested);
      let userIndex = -1;
      if (requested.startsWith(user)) userIndex = 0;
      else if (requested.endsWith(user)) userIndex = requested.length - user.length;
      else if (requested.indexOf(user) === requested.lastIndexOf(user)) userIndex = requested.indexOf(user);
      if (requestedIndex >= 0 && userIndex >= 0) index = requestedIndex + userIndex;
    } else if (payload === user || payload.endsWith(user)) {
      index = payload.length - user.length;
    } else if (payload.indexOf(user) === payload.lastIndexOf(user)) {
      index = payload.indexOf(user);
    }
    if (index < 0) return payload === user ? { before: "", after: "" } : null;
    return {
      before: payload.slice(0, index),
      after: payload.slice(index + user.length),
    };
  }
  function makeQueuedMessage(id, userText, payloadText, displayText, attachments, meta, restrictTools) {
    return {
      id,
      text: userText,
      payloadText,
      payloadEnvelope: queuedPayloadEnvelope(userText, payloadText, meta),
      metaPayloadEnvelope: meta && meta.pinvouPayloadText
        ? queuedPayloadEnvelope(userText, meta.pinvouPayloadText, meta)
        : null,
      displayText,
      attachments,
      meta,
      restrictTools,
    };
  }
  function rebuiltQueuedPayload(item, userText) {
    const envelope = item && item.payloadEnvelope;
    if (!envelope || typeof envelope.before !== "string" || typeof envelope.after !== "string") return null;
    return envelope.before + userText + envelope.after;
  }
  function rebuiltQueuedMetaPayload(item, userText) {
    const envelope = item && item.metaPayloadEnvelope;
    if (!envelope || typeof envelope.before !== "string" || typeof envelope.after !== "string") return null;
    return envelope.before + userText + envelope.after;
  }
  // 桌宠窗口靠全局事件感知回合起止。turn_start 补齐"发送 → 首 token"的空窗
  // (chat:delta 之前引擎在思考,宠物不该干站着);turn_end 只兜 invoke 直接失败
  // 这种不会有 chat:done 的路径。JS emit 是全局广播,宠物窗口 listen 收得到。
  function emitPetEvent(name, sid) {
    try {
      if (TAURI && TAURI.event && TAURI.event.emit) TAURI.event.emit(name, { session_id: sid });
    } catch { /* 桌宠是纯装饰,广播失败不影响对话 */ }
  }

  // 后端命令错误的展示文本:稳定错误码(如 image_input_unsupported,与
  // src-tauri chat.rs IMAGE_INPUT_*_ERROR 对应)按码替换为三语指引,而非剥前缀
  // 透传后端硬编码中文——英/日界面不该看到中文结论;文案与 ChatView 前置警告
  // (t.uiAttachments.*)同源语义。与 tauri bridge chat.js 同一口径。
  function displayTurnError(err) {
    const text = String(err && err.toString ? err.toString() : err || "");
    if (text.indexOf("image_input_unsupported") === 0) {
      return text.includes("能力未知") ? bt("imageUnknown") : bt("imageUnsupported");
    }
    return text;
  }

  // 真正发送:在 sid 的工作集上加 user 气泡 + 流式占位 + busy,然后 invoke chat。
  // active/后台通用(后台走 runSyncOnSession 临时切工作集)。
  function doSendFor(sid, text, displayText, attachmentsPayload, meta, restrictTools, surfaceFailure) {
    turnUsageDirty[sid] = false; // 新一轮开始，重置口径保护
    const turnOwnerBuffer = getBuffer(sid);
    let submittedMessage = null;
    let submittedMessagePos = -1;
    let submittedUserItemId = 0;
    let submittedStreamId = 0;
    if (turnOwnerBuffer && turnOwnerBuffer.remoteTurnActive) {
      recordAuthoritySyncDiagnostic("local_send_blocked_by_remote_sync", authoritySyncBufferSnapshot(sid, turnOwnerBuffer));
      return Promise.reject(new Error(bt("turnSyncRejected")));
    }
    if (turnOwnerBuffer) {
      turnOwnerBuffer.localTurnOwned = true;
      turnOwnerBuffer.remoteTurnActive = false;
      turnOwnerBuffer.remoteTerminalSeen = false;
      turnOwnerBuffer.remoteCommittedRevision = "";
      recordAuthoritySyncDiagnostic("local_turn_claimed", Object.assign({
        operation: "send",
      }, authoritySyncBufferSnapshot(sid, turnOwnerBuffer)));
    }
    runSyncOnSession(sid, function () {
      state.chatItems = state.chatItems.filter(function (item) {
        return !item.turnErrorNotice && !item.authoritySyncNotice;
      });
      const uitem = {
        type: "user",
        text: displayText,
        time: timeStr(),
        messageIndex: state.messages.length,
      };
      if (meta && meta.pinvouTransfer) uitem.pinvouTransfer = meta.pinvouTransfer; // 仅展示层,不进 messages/LLM
      if (meta && meta.pinvouScene) uitem.pinvouScene = meta.pinvouScene; // 仅展示层,不进 messages/LLM
      addChatItem(uitem);
      submittedUserItemId = uitem.id;
      submittedMessage = { role: "user", content: [{ type: "text", text: displayText }] };
      submittedMessagePos = state.messages.length;
      state.messages.push(submittedMessage);
      state.busy = true;
      startThinking();
      currentStreamText = "";
      currentStreamId = ++itemIdSeq;
      submittedStreamId = currentStreamId;
      state.chatItems.push({ id: currentStreamId, type: "assistant", text: "", html: "", time: timeStr(), streaming: true });
    });
    notify();
    emitPetEvent("pet:turn_start", sid);
    const chatCommand = IS_WEB ? "web_access_chat" : "chat";
    const chatArgs = IS_WEB
      ? {
          message: text,
          attachmentHandles: (attachmentsPayload || []).map(function (attachment) {
            return attachment && attachment.handle;
          }).filter(Boolean),
          sessionId: sid,
          restrictTools: !!restrictTools,
        }
      : { message: text, attachments: attachmentsPayload, sessionId: sid, restrictTools: !!restrictTools };
    return invoke(chatCommand, chatArgs)
      .then(function () {
        // 新一轮已被后端受理：会话中未提交的「打开」（pending enable）自此进入
        // 上下文并锁死（ComposerToolMenu 监听）。bridge 层不反向依赖 features，
        // 与 pinvou:tools-changed 一样内联派发。
        try { window.dispatchEvent(new CustomEvent("pinvou:chat-round-committed", { detail: { scope: "plain" } })); } catch { /* silently ignored */ }
        recordAuthoritySyncDiagnostic("local_turn_admitted", Object.assign({
          operation: "send",
        }, authoritySyncBufferSnapshot(sid, turnOwnerBuffer)));
        if (turnOwnerBuffer) turnOwnerBuffer.deferredRemoteUserEvent = null;
        if (meta && meta.pinvouScene) {
          runSyncOnSession(sid, function () {
            recordPinvouSceneForMessage(sid, submittedMessagePos, meta.pinvouScene);
          });
        }
        return true;
      })
      .catch(function (err) {
        const errorText = String(err && err.message ? err.message : err || "");
        const concurrentTurn = errorText.includes("session_turn_in_progress");
        recordAuthoritySyncDiagnostic("local_turn_admission_failed", Object.assign({
          operation: "send",
          concurrent_turn: concurrentTurn,
          error_category: concurrentTurn ? "session_turn_in_progress" : "command_rejected",
          error_present: true,
        }, authoritySyncBufferSnapshot(sid, turnOwnerBuffer)));
        if (turnOwnerBuffer) turnOwnerBuffer.localTurnOwned = false;
        emitPetEvent("pet:turn_end", sid);
        runSyncOnSession(sid, function () {
          // 按引用移除本地乐观提交：buffer 权威重载后引用自然消失，无需按位置
          // 二次限定（位置过滤在"引用仍在但位置移动"时会残留本地消息，且与
          // tauri 端纯身份过滤分叉——审计后回退该谓词微调）。
          state.messages = state.messages.filter(function (message) { return message !== submittedMessage; });
          state.chatItems = state.chatItems.filter(function (item) {
            return item.id !== submittedUserItemId && item.id !== submittedStreamId;
          });
          resetPendingAssistant();
          state.busy = false;
          stopThinking();
        });
        const deferredApplied = applyDeferredRemoteUserMessage(sid, turnOwnerBuffer);
        if (concurrentTurn && turnOwnerBuffer && !deferredApplied) {
          markRemoteTurn(sid, turnOwnerBuffer, false, "local_send_concurrent_turn");
        }
        runSyncOnSession(sid, function () {
          addSystemItem(concurrentTurn
            ? bt("turnAlreadyInProgress")
            : "⚠️ " + displayTurnError(err), {
            turnErrorNotice: true,
          });
        });
        notify();
        if (surfaceFailure) throw err;
        return false;
      });
  }
  // 本轮跑完(或被停止)后,若该 session 不忙且有排队消息 → 严格按 FIFO
  // 只发送队首一条。剩余消息留给后续 turn 的 done 继续逐条触发，避免把用户
  // 连续输入的多个独立任务合并成一个模型请求。
  function flushQueued(sid) {
    const pendingBuffer = sessionStates[sid];
    if (pendingBuffer && pendingBuffer.remoteTurnActive) {
      reconcileRemoteTurn(sid).then(function (ready) {
        if (ready) flushQueued(sid);
      }).catch(function () {});
      return;
    }
    if (isBusyFor(sid)) return;            // doFinal 等又起了新 turn → 留给那轮的 done 再 flush
    const q = sid === state.activeSessionId ? state.queued : (sessionStates[sid] && sessionStates[sid].queued);
    if (!q || q.length === 0) return;
    const item = q.shift();
    const attachments = item.attachments || [];
    const displayText = item.displayText == null
      ? formatAttachmentDisplayText(item.text, attachments)
      : item.displayText;
    notify();
    doSendFor(sid, item.payloadText == null ? item.text : item.payloadText, displayText, attachments, item.meta || null, !!item.restrictTools, true)
      .catch(function () {
        const retryQueue = sid === state.activeSessionId
          ? state.queued
          : (sessionStates[sid] && sessionStates[sid].queued);
        if (!retryQueue) return;
        retryQueue.unshift(item);
        notify();
      });
  }

  async function sendMessageToSession(sessionId, text, meta) {
    const sid = String(sessionId || "").trim();
    const content = String(text || "").trim();
    if (!sid) throw new Error(bt("targetSessionMissing"));
    if (!content) throw new Error(bt("replyContentEmpty"));
    const exists = state.sessions.some(function (session) { return String(session.id) === sid; });
    if (!exists) throw new Error(bt("targetSessionMissing"));

    await ensureSessionBufferLoaded(sid);
    let targetBuffer = getBuffer(sid);
    const targetQueue = targetBuffer && targetBuffer.queued;
    if (isBusyFor(sid) || (targetQueue && targetQueue.length > 0)) {
      runSyncOnSession(sid, function () {
        state.queued.push(makeQueuedMessage(
          ++itemIdSeq, content, content, content, [], meta || null, false
        ));
      });
      notify();
      if (!isBusyFor(sid)) flushQueued(sid);
      return { accepted: true, queued: true };
    }
    if (targetBuffer && targetBuffer.remoteTurnActive && !(await reconcileRemoteTurn(sid))) {
      recordAuthoritySyncDiagnostic("remote_sync_blocked_action", Object.assign({
        operation: "send_to_session",
      }, authoritySyncBufferSnapshot(sid, targetBuffer)));
      throw new Error(bt("targetSessionSyncing"));
    }
    targetBuffer = getBuffer(sid);
    if (isBusyFor(sid) || (targetBuffer.queued && targetBuffer.queued.length > 0)) {
      runSyncOnSession(sid, function () {
        state.queued.push(makeQueuedMessage(
          ++itemIdSeq, content, content, content, [], meta || null, false
        ));
      });
      notify();
      if (!isBusyFor(sid)) flushQueued(sid);
      return { accepted: true, queued: true };
    }
    const completion = doSendFor(sid, content, content, [], meta || null, false, true)
      .then(
        function () { return { ok: true }; },
        function (error) { return { ok: false, error }; }
      );
    return { accepted: true, queued: false, completion };
  }

  function findFirstTurnItem(clientMessageId) {
    return state.chatItems.find(function (item) {
      return item && item.clientMessageId === clientMessageId;
    }) || null;
  }

  function firstTurnStillVisible(submission) {
    return !!submission &&
      !state.activeSessionId &&
      state.draftEpoch === submission.draftEpoch &&
      !!findFirstTurnItem(submission.clientMessageId);
  }

  function restoreFirstTurnUiState(submission) {
    if (!submission || !submission.uiSnapshot || !firstTurnStillVisible(submission)) return;
    const snapshot = submission.uiSnapshot;
    state.scheduledTaskPendingGuide = snapshot.scheduledTaskPendingGuide;
    state.scheduledTaskCreationSessionId = snapshot.scheduledTaskCreationSessionId;
    state.scheduledTaskDraft = snapshot.scheduledTaskDraft;
    state.activeSkill = snapshot.activeSkill;
  }

  function consumeFirstTurnUiState(text, meta) {
    const snapshot = {
      scheduledTaskPendingGuide: state.scheduledTaskPendingGuide,
      scheduledTaskCreationSessionId: state.scheduledTaskCreationSessionId,
      scheduledTaskDraft: state.scheduledTaskDraft,
      activeSkill: state.activeSkill,
    };
    const requestedPayloadText = meta && meta.pinvouPayloadText
      ? String(meta.pinvouPayloadText || "").trim()
      : "";
    let payloadText = requestedPayloadText || text;
    let restrictTools = false;
    if (state.scheduledTaskPendingGuide) {
      payloadText = state.scheduledTaskPendingGuide + "\n\n" + (requestedPayloadText || text);
      restrictTools = true;
      state.scheduledTaskPendingGuide = null;
      state.scheduledTaskDraft = null;
    }
    state.activeSkill = null;
    return { snapshot, payloadText, restrictTools };
  }

  function seedAcceptedFirstTurn(sessionId, buffer, submission) {
    let existingUser = null;
    for (let index = buffer.chatItems.length - 1; index >= 0; index--) {
      if (buffer.chatItems[index] && buffer.chatItems[index].type === "user") {
        existingUser = buffer.chatItems[index];
        break;
      }
    }
    if (existingUser) {
      existingUser.deliveryState = "accepted";
      existingUser.clientMessageId = submission.clientMessageId;
      if (submission.pinvouScene) existingUser.pinvouScene = submission.pinvouScene;
      recordPinvouSceneForBufferMessage(sessionId, buffer, 0, submission.pinvouScene);
      return;
    }

    const nextItemId = Math.max(
      Number(buffer.stream && buffer.stream.itemIdSeq || 0),
      Number(submission.optimisticItemId || 0),
    ) + 1;
    const userItem = {
      id: nextItemId,
      type: "user",
      text: submission.displayText,
      time: submission.time,
      messageIndex: 0,
      deliveryState: "accepted",
      clientMessageId: submission.clientMessageId,
    };
    if (submission.pinvouScene) userItem.pinvouScene = submission.pinvouScene;
    recordPinvouSceneForBufferMessage(sessionId, buffer, buffer.messages.length, submission.pinvouScene);
    buffer.chatItems.push(userItem);
    buffer.messages.push({
      role: "user",
      content: [{ type: "text", text: submission.displayText }],
    });
    buffer.localTurnOwned = true;
    buffer.remoteTurnActive = false;
    buffer.remoteTerminalSeen = false;
    buffer.remoteCommittedRevision = "";
    buffer.busy = true;
    buffer.thinking = {
      active: true,
      phase: "thinking",
      toolName: "",
      startedAt: Date.now(),
    };
    if (!buffer.stream) buffer.stream = {};
    buffer.stream.itemIdSeq = nextItemId;
  }

  function syncNewFirstTurnSessionInBackground() {
    // The new session already starts with the local default mode/persona/KB
    // state. Only refresh the sidebar here: the other sync helpers read and
    // write the globally active session, so running them after the user has
    // navigated elsewhere could overwrite the newly selected session's UI.
    Promise.resolve(refreshHistoryList()).then(function () {
      notify();
    }, function (error) {
      console.warn("[sessions] first-turn history refresh failed", error);
      notify();
    });
  }

  function acceptFirstTurnSubmission(submission, metadata) {
    const sessionId = String(metadata && metadata.id || "").trim();
    if (!sessionId) throw new Error(bt("sessionIdMissing"));
    const existingMetaIndex = state.sessions.findIndex(function (session) {
      return session && session.id === sessionId;
    });
    if (existingMetaIndex >= 0) state.sessions[existingMetaIndex] = metadata;
    else state.sessions.unshift(metadata);

    const buffer = getBuffer(sessionId);
    buffer.loadedFromDisk = true;
    buffer.sessionRevision = String(
      metadata.transcript_revision || metadata.transcriptRevision || buffer.sessionRevision || "",
    );
    seedAcceptedFirstTurn(sessionId, buffer, submission);

    // The desktop consumed these one-shot handles even if the user navigated
    // away while the atomic first turn was in flight. Remove the exact
    // attachment objects from whichever composer is now visible so a later
    // draft cannot retry already-consumed handles.
    state.attachments = state.attachments.filter(function (attachment) {
      return !submission.readyAttachments.includes(attachment);
    });
    const shouldActivate = firstTurnStillVisible(submission);
    if (shouldActivate) {
      if (submission.uiSnapshot && submission.uiSnapshot.scheduledTaskPendingGuide) {
        state.scheduledTaskCreationSessionId = sessionId;
      }
      delete firstTurnSubmissions[submission.clientMessageId];
      switchActiveTo(sessionId);
      notify();
    } else {
      delete firstTurnSubmissions[submission.clientMessageId];
    }
    syncNewFirstTurnSessionInBackground();
  }

  async function runFirstTurnSubmission(submission) {
    if (!submission || submission.inFlight) return;
    submission.inFlight = true;
    const item = findFirstTurnItem(submission.clientMessageId);
    if (item) {
      item.deliveryState = "sending";
      item.deliveryError = "";
      notify();
    }
    try {
      const metadata = await invokeWithRequestId(
        "web_access_create_session_and_chat",
        submission.args,
        submission.requestId,
      );
      // 首轮提交成功 = 新一轮已受理：未提交的「打开」转正锁死（同 doSendFor）。
      try { window.dispatchEvent(new CustomEvent("pinvou:chat-round-committed", { detail: { scope: "plain" } })); } catch { /* silently ignored */ }
      acceptFirstTurnSubmission(submission, metadata);
    } catch (error) {
      submission.inFlight = false;
      submission.lastErrorCode = String(error && error.code || "rpc_failed");
      submission.lastError = String(error && error.message ? error.message : error || "");
      if (!firstTurnStillVisible(submission)) return;
      const failedItem = findFirstTurnItem(submission.clientMessageId);
      if (failedItem) {
        failedItem.deliveryState = submission.lastErrorCode === "outcome_unknown"
          ? "unknown"
          : "failed";
        failedItem.deliveryError = submission.lastError;
      }
      restoreFirstTurnUiState(submission);
      notify();
    }
  }

  function submitFirstWebTurn(text, displayText, readyAttachments, attachmentsPayload, meta) {
    const prepared = consumeFirstTurnUiState(text, meta);
    const clientMessageId = webRequestId("chat");
    const requestId = "first_turn_" + clientMessageId;
    const time = timeStr();
    const optimisticItem = {
      type: "user",
      text: displayText,
      time,
      messageIndex: 0,
      deliveryState: "sending",
      deliveryError: "",
      clientMessageId,
    };
    if (meta && meta.pinvouScene) optimisticItem.pinvouScene = meta.pinvouScene;
    addChatItem(optimisticItem);
    const submission = {
      clientMessageId,
      requestId,
      draftEpoch: state.draftEpoch,
      optimisticItemId: optimisticItem.id,
      time,
      displayText,
      pinvouScene: meta && meta.pinvouScene,
      readyAttachments: [...readyAttachments],
      uiSnapshot: prepared.snapshot,
      args: {
        message: prepared.payloadText,
        attachmentHandles: attachmentsPayload.map(function (attachment) {
          return attachment && attachment.handle;
        }).filter(Boolean),
        restrictTools: !!prepared.restrictTools,
      },
      inFlight: false,
      lastErrorCode: "",
      lastError: "",
    };
    firstTurnSubmissions[clientMessageId] = submission;
    notify();
    runFirstTurnSubmission(submission);
  }

  function retryFirstTurn(clientMessageId) {
    const submission = firstTurnSubmissions[String(clientMessageId || "")];
    if (!submission || submission.inFlight || !firstTurnStillVisible(submission)) return;
    if (submission.lastErrorCode !== "rpc_timeout") {
      submission.requestId = "first_turn_" + webRequestId("retry");
    }
    runFirstTurnSubmission(submission);
  }

  // Return protocol (issue #406; mirrors the tauri bridge's sendMessage):
  // - true         dispatched: sent or queued for delivery.
  // - "restored"   nothing dispatched, but the text is already back in the
  //                composer (bridge-side restore) — the caller must not
  //                restore again, it would duplicate the draft.
  // - false        nothing dispatched and the text was NOT restored
  //                (notice-only early returns / admission rejected) — the
  //                caller owns putting the draft back.
  async function sendMessage(text, meta) {
    text = (text || "").trim();
    const readyAttachments = state.attachments.filter(function (a) { return a.status === "ready" && a.result; });
    if (!text && readyAttachments.length === 0) return false;
    // 还有解析中/上传中的附件 → 等
    if (state.attachments.some(function (a) { return a.status === "parsing"; })) {
      addSystemItem(bt("attachStillParsing"));
      return false;
    }
    if (state.attachments.some(function (a) { return a.status === "uploading"; })) {
      addSystemItem(bt("attachStillUploading"));
      return false;
    }

    const displayText = formatAttachmentDisplayText(text, readyAttachments);
    const attachmentsPayload = readyAttachments.map(function (a) { return a.result; });

    if (!state.activeSessionId && IS_WEB && canInvoke("web_access_create_session_and_chat")) {
      const existingFirstTurn = state.chatItems.some(function (item) {
        return item && item.type === "user" && !!item.deliveryState;
      });
      if (existingFirstTurn) return false;
      submitFirstWebTurn(text, displayText, readyAttachments, attachmentsPayload, meta);
      return true;
    }

    if (!state.activeSessionId) {
      // 草稿态首条消息 → 物化 session(命名靠下方 persistSession auto-title)。
      // 必须用返回值判空：切走场景 ensureSession 返回 null 但 activeSessionId
      // 非空（用户已切到别的会话），按 activeSessionId 继续会把本条消息发进
      // 错误会话（审计 #257）。
      const materialized = await ensureSession();
      // 物化中止（await 期间切走）→ 把输入放回输入框，不静默丢字
      // （与 tauri 版对齐，二审 F3；错误提示由 ensureSession 内如实给出）。
      // append=true: failure-recovery semantics — the user may have started
      // the next message during the await.
      if (!materialized) {
        prefillComposer(text, true);
        // The prefill IS the restore; "restored" stops the caller from doing
        // it a second time (the prefill lands asynchronously and would then
        // append a duplicate).
        return "restored";
      }
    }
    const sid = state.activeSessionId;
    const activeTurnBuffer = getBuffer(sid);
    function consumeUiTurnState() {
      const consumed = {
        scheduledTaskPendingGuide: state.scheduledTaskPendingGuide,
        scheduledTaskCreationSessionId: state.scheduledTaskCreationSessionId,
        scheduledTaskDraft: state.scheduledTaskDraft,
        activeSkill: state.activeSkill,
      };
      const requestedPayloadText = meta && meta.pinvouPayloadText
        ? String(meta.pinvouPayloadText || "").trim()
        : "";
      let payloadText = requestedPayloadText || text;
      let restrictTools = false;
      // 定时任务引导只进入模型 payload；准入失败时由下面的 snapshot 恢复。
      if (state.scheduledTaskPendingGuide) {
        payloadText = state.scheduledTaskPendingGuide + "\n\n" + text;
        if (requestedPayloadText) payloadText = state.scheduledTaskPendingGuide + "\n\n" + requestedPayloadText;
        restrictTools = true;
        state.scheduledTaskPendingGuide = null;
        state.scheduledTaskCreationSessionId = sid;
        state.scheduledTaskDraft = null;
      }
      // 新一轮先熄灭技能标；本轮 load_skill 会重新点亮。
      state.activeSkill = null;
      return { snapshot: consumed, payloadText, restrictTools };
    }
    function restoreUiTurnState(consumed) {
      if (!consumed || state.activeSessionId !== sid) return;
      state.scheduledTaskPendingGuide = consumed.scheduledTaskPendingGuide;
      state.scheduledTaskCreationSessionId = consumed.scheduledTaskCreationSessionId;
      state.scheduledTaskDraft = consumed.scheduledTaskDraft;
      state.activeSkill = consumed.activeSkill;
    }
    function queuePrepared(prepared) {
      state.queued.push(makeQueuedMessage(
        ++itemIdSeq,
        text,
        prepared.payloadText,
        displayText,
        attachmentsPayload,
        meta || null,
        prepared.restrictTools,
      ));
      state.attachments = state.attachments.filter(function (attachment) {
        return !readyAttachments.includes(attachment);
      });
      notify();
    }

    // 排队式:当前 session 正在生成 → 这句进队列(不打断当前轮),本轮 chat:done 后自动发。
    // 输入框上方显示待发 chip(可✕撤销)。停止按钮仍只硬打断当前轮。
    if (isBusyFor(sid) || state.queued.length > 0) {
      const queuedPreparation = consumeUiTurnState();
      queuePrepared(queuedPreparation);
      if (!isBusyFor(sid)) flushQueued(sid);
      return true;
    }
    if (activeTurnBuffer && activeTurnBuffer.remoteTurnActive &&
        !(await reconcileRemoteTurn(sid))) {
      if (state.activeSessionId !== sid) {
        // The user navigated away during the hydrate: the text goes back to
        // the session it was typed in (buffer draft), never into the session
        // now on screen — "restored" keeps the caller from prefilling it
        // there (issue #406).
        restoreComposerText(sid, text);
        return "restored";
      }
      recordAuthoritySyncDiagnostic("remote_sync_blocked_action", Object.assign({
        operation: "send",
      }, authoritySyncBufferSnapshot(sid, activeTurnBuffer)));
      addAuthoritySyncNotice(bt("turnSyncRetry"));
      return false;
    }
    // The authoritative hydrate above is asynchronous. Never let an input that
    // originated in Session A drift into Session B if the user navigated away.
    if (state.activeSessionId !== sid) {
      restoreComposerText(sid, text);
      return "restored";
    }
    if (isBusyFor(sid) || state.queued.length > 0) {
      const racedQueuePreparation = consumeUiTurnState();
      queuePrepared(racedQueuePreparation);
      if (!isBusyFor(sid)) flushQueued(sid);
      return true;
    }

    const preparation = consumeUiTurnState();
    const accepted = await doSendFor(
      sid,
      preparation.payloadText,
      displayText,
      attachmentsPayload,
      meta,
      preparation.restrictTools,
    );
    if (accepted) {
      // Do not clear files selected while the admission RPC was in flight.
      state.attachments = state.attachments.filter(function (attachment) {
        return !readyAttachments.includes(attachment);
      });
      notify();
      return true;
    }
    // Admission rejected (notice already surfaced by doSendFor): nothing was
    // dispatched, so the cleared composer draft is the caller's to restore
    // (issue #406 — it used to resolve undefined here and the draft was
    // silently lost).
    restoreUiTurnState(preparation.snapshot);
    notify();
    return false;
  }
  function getComposerDraft() {
    return String(state.composerDraft || "");
  }
  function setComposerDraft(value) {
    const text = value == null ? "" : String(value);
    state.composerDraft = text;
    const activeBuffer = state.activeSessionId && sessionStates[state.activeSessionId];
    if (activeBuffer) activeBuffer.composerDraft = text;
    return text;
  }
  // Mirrors the tauri bridge: template/navigation prefills replace the draft;
  // failure recovery passes append=true for separator-joined appending
  // (re-review #4 parity).
  function prefillComposer(text, append) {
    state.composerPrefill = {
      id: (state.composerPrefill.id || 0) + 1,
      text: String(text || ""),
      append: !!append,
    };
    notify();
  }
  // Session-scoped composer text restore for sends abandoned by a session
  // switch mid-send (issue #406; mirrors the tauri bridge's restoreSteerText).
  // A bare setComposerDraft is invisible (the composer is React-local state
  // that only re-reads the store on [activeSessionId, draftEpoch]). Active
  // session: append at the store level with a "\n" separator and bump
  // draftEpoch so the draft-restore effect re-reads the accumulated draft.
  // Background session: write the session buffer only — setComposerDraft
  // targets the active working set and would leak background text into the
  // active draft.
  function restoreComposerText(sid, text) {
    const value = String(text || "");
    if (!sid || !value) return;
    if (sid === state.activeSessionId) {
      const current = String(state.composerDraft || "");
      setComposerDraft(current ? current + "\n" + value : value);
      state.draftEpoch = (state.draftEpoch || 0) + 1;
      notify();
      return;
    }
    const buffer = sessionStates[sid];
    if (!buffer) return;
    const current = String(buffer.composerDraft || "");
    buffer.composerDraft = current ? current + "\n" + value : value;
  }
  // Undo one queued message (the ✕ on its chip). Attachment handles carried
  // by the queued item are released in lockstep, matching the discard
  // semantics of the desktop removeQueued path.
  function removeQueued(id) {
    let removed = null;
    state.queued = state.queued.filter(function (q) {
      if (q.id !== id) return true;
      removed = q;
      return false;
    });
    if (removed && Array.isArray(removed.attachments)) {
      removed.attachments.forEach(releaseAttachmentOnDesktop);
    }
    notify();
  }

  function queueForSession(sid) {
    return sid === state.activeSessionId
      ? state.queued
      : (sessionStates[sid] && sessionStates[sid].queued);
  }

  function prioritizeQueued(sid, queuedId) {
    const queue = queueForSession(sid);
    if (!queue) return false;
    const index = queue.findIndex(function (item) { return item && item.id === queuedId; });
    if (index < 0) return false;
    const item = queue.splice(index, 1)[0];
    queue.unshift(item);
    notify();
    if (!isBusyFor(sid)) flushQueued(sid);
    return true;
  }

  function editQueued(sid, queuedId, nextText) {
    const queue = queueForSession(sid);
    if (!queue) return false;
    const item = queue.find(function (queued) { return queued && queued.id === queuedId; });
    if (!item) return false;
    const text = String(nextText == null ? "" : nextText).trim();
    if (!text && !(item.attachments || []).length) return false;
    const payloadText = rebuiltQueuedPayload(item, text);
    if (payloadText === null) return false;
    let metaPayloadText = null;
    // Match send-time admission: only a non-empty meta payload gets an
    // envelope, so only that shape can be rebuilt around the new text.
    const hasMetaPayload = !!(item.meta && item.meta.pinvouPayloadText);
    if (hasMetaPayload) {
      metaPayloadText = rebuiltQueuedMetaPayload(item, text);
      if (metaPayloadText === null) return false;
    }
    item.text = text;
    item.payloadText = payloadText;
    item.displayText = formatAttachmentDisplayText(text, item.attachments || []);
    if (hasMetaPayload) {
      item.meta = Object.assign({}, item.meta, { pinvouPayloadText: metaPayloadText });
    }
    notify();
    // Parity with the desktop bridge: both mutations drain an idle queue.
    if (!isBusyFor(sid)) flushQueued(sid);
    return true;
  }

  // ── Pinvou v4 召唤式检阅:Boss 主动呼叫,审当前 session 前面的工作 ──
  // 纯召唤、不替 Boss 决策。
  // 审查卡进 chatItems(当前会话可见);跨会话持久化(进 messages/独立存储)是后续增强。
  async function summonPinvou(focus, mode) {
    if (!state.activeSessionId) { addSystemItem(bt("pinvouNeedSession")); return; }
    if (state.pinvouSummoning) return;
    state.pinvouSummoning = true;
    const sid = state.activeSessionId; // 召唤发起时的 session;await 返回后校验,防跨 session 串(召唤慢+切走)
    // 检阅结果弹 modal(不进对话流):一次只一个,裁决/跳过直接操作 state.pinvouModal.review、
    // 不靠 pos 定位(根治连续召唤 pos 重复串卡)。
    state.pinvouModal = { loading: true, coverage: mode === "coverage" };
    notify();
    try {
      // focus=产出物 path(品=审产物); mode="coverage"=悟(通盘体检)。
      const review = await invoke("summon_pinvou", { sessionId: sid, focus: focus || null, mode: mode || null });
      if (state.activeSessionId !== sid) return; // 召唤期间切了 session → 丢弃,绝不 record/写进别的 session
      recordPinvouReview(review); // 存 sidecar(供核账读上轮账目);modal.review 同引用,裁决写它=写 sidecar
      if (state.pinvouModal) { state.pinvouModal.loading = false; state.pinvouModal.review = review; }
    } catch (e) {
      if (state.activeSessionId === sid && state.pinvouModal) { state.pinvouModal.loading = false; state.pinvouModal.error = String(e && e.message ? e.message : e); }
    } finally {
      state.pinvouSummoning = false;
      notify();
    }
  }

  // 通盘体检(覆盖镜头):查产物"全不全"=缺哪些完整性维度。独立入口,走 mode=coverage。
  function inspectPinvou(focus) {
    return summonPinvou(focus, "coverage");
  }

  // B2: 审查卡进 sidecar 时间线(pos=当前 messages 数),落盘。同 recordPersonaEvent
  // 范式,**不进 messages/LLM**;rerenderFromMessages 按 pos 插回,切会话/重载不丢。
  function recordPinvouReview(review) {
    if (!state.activeSessionId || !review) return null;
    const pos = state.messages.length;
    state.pinvouReviews.push({ pos, review });
    const sid = state.activeSessionId;
    const snapshot = JSON.parse(JSON.stringify(state.pinvouReviews));
    invoke("save_session_pinvou_reviews", { sessionId: sid, reviews: snapshot }).catch(function () {});
    return pos; // 供卡片记 reviewPos,裁决时按 pos 定位原 state 写 resolution
  }

  // §2 按勾选裁决:resolution 已由前端写回 review 对象(引用→sidecar),这里持久化 +
  // 把勾「让AI改」的条目走 B1 发定向修订指令(只改对应段落、禁全文重写)。Boss 驾驶,非自动。
  async function resolvePinvouReview(resolutions, actions) {
    // 检阅发生的会话归属捕获：persist 挂起期间用户可能切走，修订指令必须发回
    // 检阅会话，不得漂进当前 active 会话（与 tauri 版对齐，二审 F2）。
    const reviewSid = state.activeSessionId;
    // 弹窗只一个 review(state.pinvouModal.review),直接在它上面写 resolution——不靠 pos 定位
    // (根治连续召唤 pos 重复串卡)。它和 sidecar entry.review 同引用,写它=写 sidecar。
    const isWu = !!(state.pinvouModal && state.pinvouModal.coverage); // 关窗前取,供转交标品/悟
    const review = state.pinvouModal && state.pinvouModal.review;
    if (review && resolutions) {
      (review.recommendations || []).forEach(function (r, k) { if (resolutions.recs && resolutions.recs[k]) r.resolution = resolutions.recs[k]; });
      (review.issues || []).forEach(function (x, k) { if (resolutions.issues && resolutions.issues[k]) x.resolution = resolutions.issues[k]; });
      (review.coverage || []).forEach(function (g, k) { if (resolutions.coverage && resolutions.coverage[k]) g.resolution = resolutions.coverage[k]; });
    }
    await persistPinvouReviews(); // 落盘,配合后端 preserve_resolutions 防覆盖
    state.pinvouModal = null; // 裁决完关窗
    notify();
    if (!actions || !actions.length) return;
    // 按动作类型分组,组装一条 Boss 消息发给主 AI(Boss 驾驶,非自动回传):
    //   fix/verify=产物缺陷定向修订(verify 先核实);adopt=Boss 已定的决策;ask=让 AI 正式问。
    const fix = actions.filter(function (a) { return a.t === "fix"; });
    const verify = actions.filter(function (a) { return a.t === "verify"; });
    const adopt = actions.filter(function (a) { return a.t === "adopt"; });
    const ask = actions.filter(function (a) { return a.t === "ask"; });
    const parts = [];
    if (fix.length) {
      parts.push(bt("reviewFixHeader"));
      fix.forEach(function (a) { parts.push("- " + a.text); });
    }
    if (verify.length) {
      if (parts.length) parts.push("");
      parts.push(bt("reviewVerifyHeader"));
      verify.forEach(function (a) { parts.push("- " + a.text); });
    }
    if (adopt.length) {
      if (parts.length) parts.push("");
      parts.push(bt("reviewAdoptHeader"));
      adopt.forEach(function (a) { parts.push("- " + (a.topic ? a.topic + "：" : "") + a.pick); });
    }
    if (ask.length) {
      if (parts.length) parts.push("");
      parts.push(bt("reviewAskHeader"));
      ask.forEach(function (a) { parts.push("- " + a.topic); });
    }
    const fill = actions.filter(function (a) { return a.t === "fill"; });
    if (fill.length) {
      if (parts.length) parts.push("");
      parts.push(bt("reviewFillHeader"));
      fill.forEach(function (a) { parts.push("- " + a.dimension + (a.suggestion ? "：" + a.suggestion : "")); });
      parts.push(bt("reviewFillFooter"));
    }
    // 已切走则放弃发指令（修订指令属于检阅会话，漂进别的会话会误导其上下文）；
    // reviewSid 为 null 的防御：草稿态本不该有检阅，双 null 通过会把指令发给
    // ensureSession 新建的空会话。
    if (parts.length && reviewSid && state.activeSessionId === reviewSid) sendMessage(parts.join("\n"), { pinvouTransfer: isWu ? "悟" : "品" });
  }

  // 整卡跳过:Boss 看了不处理这次检阅 → 直接关窗(sidecar entry 留着、无 resolution,无害)。
  function dismissPinvouReview() {
    // 关窗即解召唤守卫:否则若在 await 期间被关(切 session 等路径),会留下"窗没了但
    // pinvouSummoning 仍 held"的死区——重复点品/悟在守卫处(summonPinvou 开头)被吞,要等
    // 整个直连 vLLM 调用(≤30s)返回才解锁。in-flight 结果靠 summonPinvou 内 `if (state.pinvouModal)` 守卫自然丢弃。
    state.pinvouModal = null;
    state.pinvouSummoning = false;
    notify();
  }
  // 把当前 session 的审查时间线(含勾选写回的 resolution)重新落盘。返回 promise 供 await。
  function persistPinvouReviews() {
    if (!state.activeSessionId) return Promise.resolve();
    const snapshot = JSON.parse(JSON.stringify(state.pinvouReviews));
    return invoke("save_session_pinvou_reviews", { sessionId: state.activeSessionId, reviews: snapshot }).catch(function () {});
  }

  async function cancelGeneration() {
    if (!state.busy) return;
    try {
      await invoke("cancel_generation", { sessionId: state.activeSessionId });
    } catch (e) {
      console.warn("cancel failed", e);
    }
  }

  // ── Event listeners ──────────────────────────────────────────────
  listen("session:deleted", function (e) {
    applyDeletedSession(e && e.payload && e.payload.id);
  });
  listen("session:list_changed", function () {
    refreshHistoryList().catch(function (error) {
      console.error("[sessions] session:list_changed refresh failed", error);
    });
  });
  listen("session:model_changed", function (e) {
    const payload = e && e.payload || {};
    if (payload.id !== state.activeSessionId) return;
    loadSessionModel(payload.id).catch(function (error) {
      console.error("[sessions] session:model_changed refresh failed", error);
    });
  });
  listen("session:persona_changed", function (e) {
    const payload = e && e.payload || {};
    if (payload.id !== state.activeSessionId) return;
    Promise.resolve(syncActivePersona()).then(notify).catch(function (error) {
      console.error("[sessions] session:persona_changed refresh failed", error);
    });
  });

  // 所有 chat:* 事件都带 session_id(后端 spawn_event_forwarder 打的 tag)。
  // onSessionEvent 按 session_id 把同步逻辑路由到对应 session 的工作集:active 直接跑,
  // 后台临时切工作集跑完再切回。下面每个监听器的 body 与旧单 session 版逐字一致,
  // 只是包了一层路由,所以 active session 行为零变化。
  function isInternalRuntimeUserMessage(value) {
    return window.PinvouBridgeMessages.isInternalRuntimeUserMessage(value);
  }

  function applyRemoteUserMessageEvent(e, force) {
    const payload = e && e.payload || {};
    const sid = payload.session_id || state.activeSessionId;
    if (!sid) return false;
    const userBuffer = getBuffer(sid);
    if (!userBuffer) return false;
    if (userBuffer.localTurnOwned && !force) {
      // Usually this is the acknowledgement for our optimistic bubble. Keep
      // it briefly so a losing admission race can replay the competing UI's
      // event after its own RPC returns session_turn_in_progress.
      userBuffer.deferredRemoteUserEvent = e;
      return false;
    }
    const content = String(payload.content || "");
    const hideInternalRuntimeMessage = isInternalRuntimeUserMessage(content);
    const operation = String(payload.operation || "append");
    const action = String(payload.action || "");
    const actionPlanId = String(payload.plan_id || payload.planId || "").trim();
    const baseRevision = String(payload.base_transcript_revision || "");
    const admissionKey = baseRevision
      ? operation + ":" + baseRevision
      : (e && e.id ? "event:" + e.id : "");
    if (admissionKey && userBuffer.remoteAdmissionKeys.includes(admissionKey)) return false;
    if (admissionKey) {
      userBuffer.remoteAdmissionKeys.push(admissionKey);
      if (userBuffer.remoteAdmissionKeys.length > 32) userBuffer.remoteAdmissionKeys.shift();
    }
    let lastUserText = "";
    for (let messageIndex = userBuffer.messages.length - 1; messageIndex >= 0; messageIndex--) {
      const candidate = userBuffer.messages[messageIndex];
      if (candidate && candidate.role === "user") {
        lastUserText = userMessageDisplayText(candidate.content || [], false);
        break;
      }
    }
    const snapshotAlreadyCoversTurn = !!(
      userBuffer.loadedFromDisk && baseRevision && userBuffer.sessionRevision &&
      userBuffer.sessionRevision !== baseRevision && lastUserText === content
    );
    markRemoteTurn(sid, userBuffer, false, "remote_user_message_event");
    runSyncOnSession(sid, function () {
      if (action === "accept_plan") {
        state.chatItems.forEach(function (item) {
          if (item && item.type === "plan_card" && item.cardState === "active" && !item.resolved &&
              (!actionPlanId || String(item.planId || "") === actionPlanId)) {
            item.cardState = "approved";
            item.resolved = true;
            item.statusLabel = bt("approved");
          }
        });
        const acceptedMode = payload.mode_state || payload.modeState;
        // 事件负载的权威 mode 写回走收敛点（bump seq 防在途旧读覆盖；
        // 此回调在 runSyncOnSession(sid) 内，sid 即触发会话）。
        if (acceptedMode) applyAuthoritativeModeState(sid, acceptedMode);
      }
      state.chatItems = state.chatItems.filter(function (item) { return !item.turnErrorNotice; });
      if (!snapshotAlreadyCoversTurn && !hideInternalRuntimeMessage) {
        if (operation === "edit_last") {
          for (let index = state.chatItems.length - 1; index >= 0; index--) {
            if (state.chatItems[index] && state.chatItems[index].type === "user") {
              state.chatItems.splice(index);
              break;
            }
          }
          resetPendingAssistant();
        }
        addChatItem({ type: "user", text: content, time: timeStr() });
      }
      state.busy = true;
      if (!state.thinking.active) startThinking();
      currentStreamText = "";
      currentStreamId = 0;
    });
    notify();
    return true;
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

  function applyDeferredRemoteUserMessage(_sid, buf) {
    if (!buf || !buf.deferredRemoteUserEvent) return false;
    const deferredEvent = buf.deferredRemoteUserEvent;
    buf.deferredRemoteUserEvent = null;
    return applyRemoteUserMessageEvent(deferredEvent, true);
  }

  listen("chat:user_message", function (e) {
    applyRemoteUserMessageEvent(e, false);
  });

  listen("chat:transcript_committed", function (e) {
    const payload = e && e.payload || {};
    const sid = payload.session_id || state.activeSessionId;
    if (!sid) return;
    const committedBuffer = getBuffer(sid);
    if (!committedBuffer) return;
    const revision = String(payload.transcript_revision || payload.transcriptRevision || "");
    if (revision) {
      committedBuffer.sessionRevision = revision;
      committedBuffer.remoteCommittedRevision = revision;
    }
    recordAuthoritySyncDiagnostic("transcript_committed_event_received", Object.assign({
      event_revision: revision,
      terminal_seen_before_event: !!committedBuffer.remoteTerminalSeen,
    }, authoritySyncBufferSnapshot(sid, committedBuffer)));
    if (committedBuffer.remoteTerminalSeen && !isBusyFor(sid)) {
      reconcileRemoteTurn(sid).then(function (ready) {
        if (ready) flushQueued(sid);
      }).catch(function () {});
    }
  });

  function visibleUserTurnIndex() {
    const count = state.chatItems.filter(function (item) { return item && item.type === "user"; }).length;
    return Math.max(0, count - 1);
  }

  function latestOpenTimelineStart() {
    const events = state.turnTimeline || [];
    const completed = Object.create(null);
    events.forEach(function (event) {
      if (event && event.event === "assistant_done") completed[event.turn_id] = true;
    });
    for (let index = events.length - 1; index >= 0; index--) {
      const event = events[index];
      if (event && event.event === "user_start" && !completed[event.turn_id]) return event;
    }
    return null;
  }

  function recordTurnStarted() {
    if (state.activeTurnTimelineId) return;
    const timestamp = Date.now();
    const turnIndex = visibleUserTurnIndex();
    const existing = latestOpenTimelineStart();
    if (existing && Math.abs(timestamp - Number(existing.timestamp || 0)) < 60000) {
      existing.ui_turn_index = turnIndex;
      state.activeTurnTimelineId = existing.turn_id;
      return;
    }
    const turnId = "ui_" + String(state.activeSessionId || "session") + "_" + timestamp + "_" + turnIndex;
    state.activeTurnTimelineId = turnId;
    state.turnTimeline = [...(state.turnTimeline || []), {
      turn_id: turnId,
      event: "user_start",
      timestamp,
      ts: new Date(timestamp).toISOString(),
      ui_turn_index: turnIndex,
    }];
  }

  function preserveInterruptedAssistantPresentation() {
    let userItemIndex = -1;
    let afterMessageIndex = -1;
    let afterUserOrdinal = -1;
    for (let index = 0; index < state.chatItems.length; index++) {
      const candidate = state.chatItems[index];
      if (!candidate || candidate.type !== "user") continue;
      afterUserOrdinal += 1;
      userItemIndex = index;
      afterMessageIndex = -1;
      if (Number.isFinite(Number(candidate.messageIndex))) {
        afterMessageIndex = Number(candidate.messageIndex);
      }
    }
    // 没有任何 user 气泡时无法锚定轮次;若把全部历史 assistant 项都标记为
    // 仅展示,下次权威重载会在末尾追加整段历史的重复副本。此时放弃保留,
    // 退化为修复前行为(重载后消失),但必须清空 pending 以免污染下一轮。
    if (userItemIndex < 0) {
      pendingAssistantText = "";
      pendingAssistantBlocks = [];
      return;
    }
    for (let itemIndex = userItemIndex + 1; itemIndex < state.chatItems.length; itemIndex++) {
      const item = state.chatItems[itemIndex];
      if (!item || item.type !== "assistant" || !item.html) continue;
      item.interruptedDisplayOnly = true;
      item.afterMessageIndex = afterMessageIndex;
      item.afterUserOrdinal = afterUserOrdinal;
    }
    pendingAssistantText = "";
    pendingAssistantBlocks = [];
  }

  listen("chat:turn_started", function (e) { onSessionEvent(e, function () {
    state.busy = true;
    if (!state.thinking.active) startThinking();
    recordTurnStarted();
    notify();
  }); });

  function reasoningEventIndex(e) {
    const value = e && e.payload && e.payload.index;
    if ([undefined, null, ""].includes(value)) return null;
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : String(value);
  }

  function streamingReasoningItem(index) {
    for (let itemIndex = state.chatItems.length - 1; itemIndex >= 0; itemIndex--) {
      const item = state.chatItems[itemIndex];
      if (!item || item.type !== "reasoning" || !item.streaming) continue;
      if ([undefined, null].includes(index) || item.reasoningIndex === index) return item;
    }
    return null;
  }

  function finalizeStreamingReasoning(index) {
    const completedAt = Date.now();
    for (let itemIndex = state.chatItems.length - 1; itemIndex >= 0; itemIndex--) {
      const item = state.chatItems[itemIndex];
      if (!item || item.type !== "reasoning" || !item.streaming) continue;
      if (index !== undefined && index !== null && item.reasoningIndex !== index) continue;
      item.streaming = false;
      item.completedAt = completedAt;
    }
  }

  function finalizeAssistantStreamBeforeReasoning() {
    flushPendingTextBlock();
    const item = state.chatItems.find(function (it) { return it.id === currentStreamId; });
    if (item) {
      if (item.html) item.streaming = false;
      else state.chatItems = state.chatItems.filter(function (it) { return it !== item; });
    }
    currentStreamText = "";
    currentStreamId = 0;
  }

  function startReasoningBlock(index) {
    const existing = streamingReasoningItem(index);
    if (existing) return existing;
    finalizeStreamingReasoning();
    finalizeAssistantStreamBeforeReasoning();
    const item = {
      type: "reasoning",
      text: "",
      time: timeStr(),
      streaming: true,
      startedAt: Date.now(),
      completedAt: null,
      reasoningIndex: index,
    };
    addChatItem(item);
    pendingAssistantBlocks.push({ type: "thinking", thinking: "" });
    return item;
  }

  function appendReasoningBlock(text) {
    const last = pendingAssistantBlocks[pendingAssistantBlocks.length - 1];
    if (last && last.type === "thinking") last.thinking += text;
    else pendingAssistantBlocks.push({ type: "thinking", thinking: text });
  }

  listen("chat:reasoning_start", function (e) { onSessionEvent(e, function () {
    startReasoningBlock(reasoningEventIndex(e));
    notify();
  }); });

  listen("chat:reasoning_delta", function (e) { onSessionEvent(e, function () {
    const text = String(e.payload && e.payload.text || "");
    if (!text) return;
    const index = reasoningEventIndex(e);
    let item = streamingReasoningItem(index);
    if (!item) {
      item = startReasoningBlock(index);
    }
    item.text += text;
    appendReasoningBlock(text);
    notify();
  }); });

  listen("chat:reasoning_done", function (e) { onSessionEvent(e, function () {
    const index = reasoningEventIndex(e);
    const item = streamingReasoningItem(index);
    finalizeStreamingReasoning(index);
    if (item && !item.text) {
      state.chatItems = state.chatItems.filter(function (candidate) { return candidate !== item; });
      const last = pendingAssistantBlocks[pendingAssistantBlocks.length - 1];
      if (last && last.type === "thinking" && !last.thinking) pendingAssistantBlocks.pop();
    }
    notify();
  }); });

  listen("chat:delta", function (e) { onSessionEvent(e, function () {
    finalizeStreamingReasoning();
    const text = e.payload && e.payload.text || "";
    pendingAssistantText += text;
    currentStreamText += text;
    // Update the streaming chat item
    const item = state.chatItems.find(function (it) { return it.id === currentStreamId; });
    if (item) {
      item.text = currentStreamText;
      item.html = renderMarkdown(currentStreamText);
      item.streaming = true;
    } else {
      // New bubble needed (after tool card)
      currentStreamId = ++itemIdSeq;
      state.chatItems.push({
        id: currentStreamId,
        type: "assistant",
        text: currentStreamText,
        html: renderMarkdown(currentStreamText),
        time: timeStr(),
        streaming: true,
      });
    }
    notify();
  }); });

  listen("scheduled_task:run_updated", function () {
    scheduleScheduledRunRefresh();
  });

  listen("chat:memory_write", function (e) {
    handleMemoryWrite(e && e.payload);
  });

  // present_artifact MCP 工具名匹配:兼容底座 MCP adapter 可能加的 server 前缀
  // (实测透传名若带前缀仍命中)。命中则渲染成品卡而非灰色工具卡。
  function isPresentArtifactTool(name) {
    return name === "present_artifact" ||
      (typeof name === "string" && name.endsWith("present_artifact"));
  }

  // 成品卡路径:优先用 server(present_artifact_server.py)解析并验证过的绝对路径 abs_path——
  // 模型常给相对路径,直接拿 args.path 渲染会让卡片 path 是相对,点 Open 报「path must be
  // absolute」,且模型可能重试再 present 一次出双卡。取不到 abs_path 才回退原始 path。
  // 兼容两种结果格式:直接 payload {abs_path} / MCP content 数组 {content:[{text}]} 包一层。
  function parseToolResultPayload(toolResultContent) {
    try {
      const raw = typeof toolResultContent === "string" ? toolResultContent : JSON.stringify(toolResultContent || {});
      const obj = JSON.parse(raw);
      if (obj && obj.content && obj.content[0] && typeof obj.content[0].text === "string") {
        try {
          const inner = JSON.parse(obj.content[0].text);
          if (inner && typeof inner === "object") return inner;
        } catch { /* when the inner value is not JSON, use the outer object as-is */ }
      }
      return obj;
    } catch {
      return null;
    }
  }
  function artifactPathFromToolOutput(toolResultContent) {
    const obj = parseToolResultPayload(toolResultContent);
    if (!obj || typeof obj !== "object") return null;
    const p = obj.abs_path || obj.path || obj.file_path || obj.local_path;
    return typeof p === "string" && p ? p : null;
  }
  function shouldUseToolOutputAsArtifact(name) {
    if (!name || isPresentArtifactTool(name)) return false;
    // Only MCP-style producer tools should be parsed from result JSON. Shell/read
    // tools often return diagnostic JSON with a `path` field, which is not a
    // newly created artifact.
    return typeof name === "string" && name.indexOf("mcp_") === 0;
  }
  function presentArtifactAbsPath(toolResultContent, fallbackPath) {
    fallbackPath = fallbackPath || "";
    const parsed = artifactPathFromToolOutput(toolResultContent);
    if (parsed) return parsed;
    return fallbackPath;
  }

  // 子智能体不产生 chat:tool_start/chat:tool_end（forwarder 只转发 mailbox 进展），
  // 它启动的后台 shell 任务若无人调度轮询，就要等到会话切换或顶层 shell 调用
  // 才会被 applyShellSnapshots 发现。从进展文本认出 shell 工具即补一次调度；
  // 轮询自身会在没有运行中任务时自停，不会常驻。
  listen("multiagent:agent_progress", function (e) {
    const p = e.payload || {};
    if (p.session_id && mentionsShellTool(p.status)) {
      scheduleShellPoll(p.session_id, true);
    }
  });

  listen("chat:tool_start", function (e) { onSessionEvent(e, function () {
    const p = e.payload || {};
    // Relay reconnects and the desktop event bridge may replay the last frame.
    // Tool-call ids are durable identities, so never create a second message/card
    // for the same call. Normal repeated calls have distinct ids and remain visible.
    if (toolCallAlreadyStarted(p.id) || toolCallAlreadyFinished(p.id)) return;
    if (p.session_id) turnUsageDirty[p.session_id] = true; // 多请求轮，usage 累加值不可当占用
    toolMeta[p.id] = { name: p.name, args: p.args };
    finalizeStreamingReasoning();
    thinkingTool(p.name);
    flushPendingTextBlock();
    pendingAssistantBlocks.push({ type: "tool_use", id: p.id, name: p.name, input: p.args || {} });

    // Finalize current streaming bubble
    const streamItem = state.chatItems.find(function (it) { return it.id === currentStreamId; });
    if (streamItem) {
      streamItem.streaming = false;
    }
    currentStreamText = "";
    currentStreamId = 0;

    // request_user_input：不渲染默认工具卡，等 chat:user_input_required 单独渲染选择卡片
    if (p.name === "request_user_input") { notify(); return; }

    // present_artifact：不渲染灰色工具卡，等 tool_end 成功时渲染成品卡
    if (isPresentArtifactTool(p.name)) { notify(); return; }

    // load_skill：模型加载技能 → 点亮 composer 技能标（内置自动技能"正在使用"指示）。
    if (p.name === "load_skill") {
      const skArg = ((p.args && (p.args.name || p.args.skill)) || "").toString();
      const skLower = skArg.toLowerCase();
      if (skArg.includes("视觉设计") || skLower.includes("visual-design")) state.activeSkill = "visual-design";
      else if (skArg.includes("插件包标准化") || skLower.includes("package-author")) state.activeSkill = "package-author";
      else if (skArg.includes("技能创建") || skLower.includes("skill-author")) state.activeSkill = "skill-author";
      else if (skArg.includes("公文写作") || skLower.includes("government-writing")) state.activeSkill = "government-writing";
      else if (skArg.includes("PPT") || skArg.includes("幻灯片") || skLower.includes("pptx")) state.activeSkill = "pptx";
      else if (skArg.includes("数据分析可视化") || skArg.includes("数据可视化") || skLower.includes("visualizer")) state.activeSkill = "visualizer";
      // 不 return：照常出工具卡。卡内容在 tool_end / rerender 处脱敏成占位，
      // 展开看不到 SKILL.md 全文（防设计系统泄露），但保留"加载了技能"的痕迹。
    }

    // Add tool card
    addChatItem({
      type: "tool", toolId: p.id, name: p.name, args: p.args,
      output: null, success: null, state: "running", sessionId: p.session_id || state.activeSessionId,
    });
    if (isShellExecutionTool(p.name)) {
      scheduleShellPoll(p.session_id || state.activeSessionId, true);
    }
    notify();
  }); });

  // eslint-disable-next-line sonarjs/cognitive-complexity -- legacy bridge; refactor tracked separately
  listen("chat:tool_end", function (e) { onSessionEvent(e, function () {
    const p = e.payload || {};
    if (toolCallAlreadyFinished(p.id)) return;
    const meta = toolMeta[p.id];
    thinkingIdle();
    const resultContent = typeof p.output === "string" ? p.output : JSON.stringify(p.output);
    flushAssistantMessageToHistory();
    const trBlock = { type: "tool_result", tool_use_id: p.id, content: resultContent };
    if (!p.success) trBlock.is_error = true;
    state.messages.push({ role: "user", content: [trBlock] });

    // request_user_input 结束：把选择卡片标记为已提交/取消，不渲染工具卡
    if (meta && meta.name === "request_user_input") {
      patchLastItem(
        function (it) { return it.type === "user_input" && it.toolCallId === p.id && !it.resolved; },
        { resolved: true, cardState: p.success ? "submitted" : "cancelled" }
      );
      delete toolMeta[p.id];
      currentStreamText = ""; currentStreamId = 0;
      notify();
      return;
    }

    // present_artifact 结束：成功 → 弹成品卡(点击打开);失败 → 落普通工具卡显错误,
    // 让 AI 从 tool_result 看到错误自行重试。成品卡是真工具调用,tool_use 已进
    // messages(tool_start line 784),rerenderFromMessages 按 name 还原,切会话不丢。
    if (meta && isPresentArtifactTool(meta.name)) {
      if (p.success) {
        // 用 server 解析好的绝对路径(present_artifact_server.py 的 abs_path),而非模型可能
        // 给的相对 args.path → 卡片 path 绝对,点 Open 不再报「path must be absolute」。
        const presentedPath = presentArtifactAbsPath(p.output, meta.args && meta.args.path);
        const presentedCard = {
          type: "artifact_card",
          path: presentedPath,
          title: (meta.args && meta.args.title) || "",
          description: (meta.args && meta.args.description) || "",
          time: timeStr(),
          sessionId: p.session_id || state.activeSessionId,
        };
        const presentationCard = updatePresentedArtifact(presentedCard) || presentedCard;
        if (presentationCard === presentedCard) addChatItem(presentedCard);
        if (presentedPath) state.turnPresentedArtifacts.push(presentedPath); // 本 turn 已出成品卡,chat:done 不再兜底补
        // 同步进产物面板:present_artifact 出卡的产物也算「产出物」。修「自己生成文件、
        // 不走 write_file 的工具(如 make_pptx)→ 卡有、面板无」。trackArtifact 已去重。
        if (presentedPath) trackArtifact(presentedPath);
        delete toolMeta[p.id];
        currentStreamText = ""; currentStreamId = 0;
        notify();
        // Keep the web adapter aligned with desktop: an in-place card update does
        // not change artifactCount, but an explicit presentation must remain visible.
        try {
          window.dispatchEvent(new CustomEvent("pinvou:present-artifact", { detail: {
            sessionId: presentedCard.sessionId,
            path: presentationCard.path,
            toolCallId: p.id,
          } }));
        } catch { /* DOM event dispatch failure leaves the artifact available from the dock */ }
        return;
      }
      // 失败:补一张工具卡承载错误输出(tool_start 时跳过了灰卡)
      addChatItem({
        type: "tool", toolId: p.id, name: meta.name, args: meta.args,
        output: p.output, success: false, state: "done",
      });
      delete toolMeta[p.id];
      currentStreamText = ""; currentStreamId = 0;
      notify();
      return;
    }

    // 通用工具产物兜底：PPT / 公文等 MCP 工具会先返回 {path: "..."}，
    // 随后模型按约定再调 present_artifact。若模型漏调，仍把该成品归到当前
    // tool_end 所属 session，并在 chat:done 统一补一张成品卡。
    if (p.success && meta && shouldUseToolOutputAsArtifact(meta.name)) {
      const producedPath = artifactPathFromToolOutput(p.output);
      if (producedPath && isDeliverable(producedPath)) {
        trackArtifact(producedPath);
        markTurnDirtyArtifact(producedPath);
      }
    }

    // load_skill：卡照出，但不把返回的 SKILL.md 全文写进卡，展开只见占位（防设计系统泄露）。
    const outForCard = (meta && meta.name === "load_skill")
      ? bt("skillContentHidden")
      : toolResultDisplayContent(p.output);
    const updatedToolItem = updateToolItem(p.id, outForCard, p.success);
    const shellTaskId = p.metadata && (p.metadata.task_id || p.metadata.taskId);
    if (updatedToolItem && shellTaskId) {
      const syntheticShellItem = state.chatItems.find(function (it) {
        return it !== updatedToolItem && it.shellSnapshot === true && it.taskId === shellTaskId;
      });
      if (syntheticShellItem) {
        ["shellStatus", "exitCode", "elapsedMs", "output", "state", "success", "shellSnapshotKey"]
          .forEach(function (key) {
            if (syntheticShellItem[key] !== undefined) updatedToolItem[key] = syntheticShellItem[key];
          });
        const syntheticIndex = state.chatItems.indexOf(syntheticShellItem);
        if (syntheticIndex >= 0) state.chatItems.splice(syntheticIndex, 1);
      }
      updatedToolItem.taskId = shellTaskId;
      updatedToolItem.sessionId = p.session_id || state.activeSessionId;
      // 后台 shell 任务标记：供后台任务指示器区分"真后台"与被轮询命令匹配
      // 回退挂上 taskId 的前台卡（桌面端 backgrounded 快速通道等价逻辑）。
      if (p.metadata && p.metadata.backgrounded === true) updatedToolItem.background = true;
      const shellStatus = String((p.metadata && p.metadata.status) || "").toLowerCase();
      if (shellStatus === "running" || /running|background/i.test(String(p.output || ""))) {
        updatedToolItem.state = "running";
        updatedToolItem.success = null;
      }
      scheduleShellPoll(updatedToolItem.sessionId, true);
    }

    // Careful hook：CodeWhale shell.rs 拦截 Dangerous → 红色拦截卡
    const md = p.metadata;
    if (md && md.safety_level === "dangerous" && md.blocked && !hasChatItemForTool("careful_blocked", p.id)) {
        addChatItem({
          type: "careful_blocked", toolCallId: p.id,
          args: meta && meta.args, metadata: md, time: timeStr(),
        });
      }

    // File.write/File.edit/File.patch 改了产物 → 记账,turn 结束(chat:done)统一补成品卡。
    // 改成记账+去重:AI 一个 turn 会 edit_file 改很多次,实时续会刷出一堆卡;且 edit_file
    // 之前不触发续卡 → 改完没新卡片 → 没法对改后产物再召唤 pinvou(核账闭环断裂)。
    const mutationAction = meta && fileMutationAction(meta.name, meta.args);
    if (p.success && mutationAction) {
      extractArtifactPaths(meta.args).forEach(function (ap) {
        // 面板只收「成品」:成品型扩展名(自动当成品)或之前 present_artifact 过的文件;
        // 中间草稿(content_p1.txt / *_params.json 等)不进面板。edit_file 只改已有不新建。
        if (mutationAction !== "edit" && (isDeliverable(ap) || findPresentedArtifact(ap))) trackArtifact(ap);
        // 产物(present 过的成品 或 write/append 写进产物列表的)被写/改 → turn 结束补卡。
        // 不再要求 present 过:AI 经常写完产物忘了 present_artifact → 没成品卡 = 没召唤入口。
        // 按 basename 比对:disk watcher(artifact:disk)写盘后抢先用**绝对**路径 trackArtifact
        // 占了名额,而这里 ap 是 write_file 的**相对**参数 —— 用 a.path===ap 比绝对≠相对永远落空,
        // turnDirty 收不到 → 实时不补成品卡(只能靠重启 rerender 才出)。basename 比对消除该竞态。
        const _apbn = basename(ap);
        const isArtifact = !!findPresentedArtifact(ap) || state.artifacts.some(function (a) { return basename(a.path) === _apbn; });
        if (isArtifact) markTurnDirtyArtifact(ap);
      });
    }

    // 兜底：Plan 模式下 AI 调了被白名单/sandbox 拦的工具 → 弹兜底卡，给两条出路
    if (!p.success && state.modeState.mode === "plan" && typeof p.output === "string" &&
        (p.output.includes("not available in the current tool catalog") ||
         p.output.includes("unavailable in Plan mode") ||
         p.output.includes("PermissionDenied")) && !hasUnresolvedItem("plan_stuck")) {
        addChatItem({ type: "plan_stuck", toolName: meta && meta.name, resolved: false, time: timeStr() });
      }

    delete toolMeta[p.id];
    currentStreamText = "";
    currentStreamId = 0;
    notify();
  }); });

  // chat:done 特殊:同步收尾(flush/busy=false/mode 复位)走 runSyncOnSession
  // 路由到对应 session;异步收尾(discard_plan/落盘/刷新列表)按显式 sid 路由,
  // 不依赖工作集 —— 这样后台 session 跑完也能正确落盘。
  listen("chat:done", function (e) {
    const sid = (e.payload && e.payload.session_id) || state.activeSessionId;
    const knownDoneSession = !!sid && state.sessions.some(function (session) { return session.id === sid; });
    const scheduledDoneSession = isScheduledRunSession(sid);
    if (sid && sid !== state.activeSessionId && !sessionStates[sid] &&
        !knownDoneSession && !scheduledDoneSession) {
      // A stale/malformed terminal for an unknown ordinary Session must not
      // allocate a background buffer or persist the active Session into it.
      return;
    }
    const doneBuffer = sid ? getBuffer(sid) : null;
    // Match the Tauri client: rejected optimistic edits must hydrate the
    // unchanged authoritative transcript before another local turn starts.
    const operationRejected = !!(e.payload && e.payload.operation_rejected);
    const completedLocalTurn = !!(
      doneBuffer && doneBuffer.localTurnOwned && !isScheduledRunSession(sid) && !operationRejected
    );
    recordAuthoritySyncDiagnostic("chat_done_classified", Object.assign({
      completed_local_turn: completedLocalTurn,
      operation_rejected: operationRejected,
      requires_authority_reconcile: !isScheduledRunSession(sid),
      terminal_status: String(e.payload && e.payload.status || ""),
      terminal_error_present: !!(e.payload && e.payload.error),
    }, authoritySyncBufferSnapshot(sid, doneBuffer)));
    if (doneBuffer && !doneBuffer.localTurnOwned && !isScheduledRunSession(sid)) {
      // transcript_committed precedes chat:done. Preserve its revision when a
      // reconnecting client first materializes the turn at the terminal tail.
      markRemoteTurn(sid, doneBuffer, true, "chat_done_without_local_owner");
    }
    if (isScheduledRunSession(sid)) markScheduledInitialTurnTerminal(sid);
    runSyncOnSession(sid, function () {
      if (isScheduledRunSession(sid)) markScheduledInitialTurnTerminal(sid);
      const error = e.payload && e.payload.error;
      window.PinvouWebTurnTerminal.recordCompleted(
        state,
        latestOpenTimelineStart(),
        e.payload || {}
      );
      // 401/鉴权失败:刷新 effectiveModelConfig → 前端拦截遮罩自动弹出引导配置。
      // \b401\b 词边界锚定,避免误匹配 "port 4014"/"row 401" 等含 401 子串的无关报错。
      if (error && /\b401\b|unauthorized|authentication/i.test(String(error))) loadEffectiveModelConfig();
      if (error) {
        const finalNotice = "⚠️ " + error;
        const finalNoticeItem = state.chatItems.find(function (item) {
          return item && item.turnErrorNotice && item.text === finalNotice;
        });
        if (finalNoticeItem) {
          finalNoticeItem.legacyConversationOnly = true;
        } else {
          addSystemItem(finalNotice, {
            turnErrorNotice: true,
            legacyConversationOnly: true,
          });
        }
      }
      window.PinvouBridgeMessages.showShellCleanupFailure(e.payload, state, addSystemItem);
      const terminalStatus = String(e.payload && e.payload.status || "").toLowerCase();
      const interrupted = ["interrupted", "cancelled", "canceled"].includes(terminalStatus);
      if (interrupted) preserveInterruptedAssistantPresentation();
      else flushAssistantMessageToHistory();
      // Refresh artifacts written in this turn in place when already presented.
      // Add only the first card for artifacts the model did not present, and
      // skip artifacts already presented in this turn or changed repeatedly.
      (state.turnDirtyArtifacts || []).forEach(function (ap) {
        // 按 basename 比对:present 存 server 绝对路径、turnDirty 存 write 相对路径,
        // 直接 indexOf 比不中 → present 过的文件会被兜底再补一张(重复)。
        const _apbn = basename(ap);
        if ((state.turnPresentedArtifacts || []).some(function (pp) { return basename(pp) === _apbn; })) return;
        const prev = findPresentedArtifact(ap);
        // 补卡 path 优先用 disk watcher 落进产物列表的同名**绝对**路径(open 可靠、跨 session 稳);
        // 没有再退回 write_file 的相对 ap(由 sessionId 兜底解析)。
        const tracked = state.artifacts.find(function (a) {
          return normalizedPath(a.path) === normalizedPath(ap) && isAbsPath(a.path);
        }) || state.artifacts.find(function (a) {
          return basename(a.path) === _apbn && isAbsPath(a.path);
        });
        const cardPath = (tracked && tracked.path) || ap;
        if (prev) {
          // A user re-request after the previous card returns null and still
          // receives a fresh visible card.
          const refreshed = updatePresentedArtifact({ type: "artifact_card", path: cardPath, title: prev.title, description: prev.description, time: timeStr(), sessionId: sid });
          if (!refreshed) addChatItem({ type: "artifact_card", path: cardPath, title: prev.title, description: prev.description, time: timeStr(), sessionId: sid });
        } else {
          addChatItem({ type: "artifact_card", path: cardPath, title: basename(ap), description: "", time: timeStr(), sessionId: sid });
        }
      });
      state.turnDirtyArtifacts = [];
      state.turnPresentedArtifacts = [];
      finalizeStreamingReasoning();
      // Finalize streaming bubble
      const streamItem = state.chatItems.find(function (it) { return it.id === currentStreamId; });
      if (streamItem) streamItem.streaming = false;
      // Remove empty assistant bubbles
      state.chatItems = state.chatItems.filter(function (it) {
        return !(it.type === "assistant" && !it.html);
      });
      state.busy = false;
      stopThinking();
      currentStreamText = "";
      currentStreamId = 0;
    });
    if (doneBuffer && !completedLocalTurn && !isScheduledRunSession(sid)) {
      // Rust has already committed the final transcript before chat:done. Keep
      // both UIs behind a short authority barrier until that snapshot is loaded.
      let finalAssistantMessage = null;
      for (let doneMessageIndex = doneBuffer.messages.length - 1; doneMessageIndex >= 0; doneMessageIndex--) {
        if (doneBuffer.messages[doneMessageIndex] && doneBuffer.messages[doneMessageIndex].role === "assistant") {
          finalAssistantMessage = doneBuffer.messages[doneMessageIndex];
          break;
        }
      }
      doneBuffer.remoteExpectedAssistantKey = finalAssistantMessage
        ? hydratedMessageKey(finalAssistantMessage, isScheduledRunSession(sid))
        : "";
      if (doneBuffer.localTurnOwned) doneBuffer.deferredRemoteUserEvent = null;
      doneBuffer.localTurnOwned = false;
      doneBuffer.remoteTurnActive = true;
      doneBuffer.remoteTerminalSeen = true;
      doneBuffer.busy = false;
      if (sid === state.activeSessionId) saveWorkingSetTo(doneBuffer);
    } else if (completedLocalTurn || isScheduledRunSession(sid)) {
      // A local turn is already authoritative in this desktop process. Saved
      // transcript verification remains best-effort and must not lock the next
      // local message behind a cross-client synchronization state. Scheduled
      // runs skip transcript reconciliation entirely (Rust owns the durable
      // transcript), so the same full release applies: markRemoteTurn may have
      // armed the remote-authority gate during the streamed turn, and a stale
      // gate would send flushQueued into reconcileRemoteTurn, whose
      // write-ownership busy check then deadlocks the queued follow-up forever.
      doneBuffer.deferredRemoteUserEvent = null;
      doneBuffer.localTurnOwned = false;
      doneBuffer.remoteTurnActive = false;
      doneBuffer.remoteTerminalSeen = false;
      doneBuffer.remoteBaselineMessageCount = null;
      doneBuffer.remoteBaselineTrusted = false;
      doneBuffer.remoteExpectedAssistantKey = "";
      doneBuffer.remoteCommittedRevision = "";
      doneBuffer.busy = false;
      if (sid === state.activeSessionId) saveWorkingSetTo(doneBuffer);
    }
    notify();
    // 异步收尾(按 sid 路由,active/后台通用)
    (async function () {
      await persistMessagesFor(sid);
      // Scheduled runs skip transcript reconciliation entirely, mirroring the
      // Tauri bridge: Rust owns the durable transcript for a scheduled session,
      // and the gate was already fully released in the synchronous tail above.
      const reconciled = (completedLocalTurn || isScheduledRunSession(sid))
        ? true
        : await reconcileRemoteTurn(sid);
      if (reconciled) await persistMessagesFor(sid);
      await refreshHistoryList();
      if (!reconciled) {
        recordAuthoritySyncDiagnostic("authority_sync_notice_shown", Object.assign({
          notice: "remote_done_unsynced",
        }, authoritySyncBufferSnapshot(sid, doneBuffer)));
        runSyncOnSession(sid, function () {
          addAuthoritySyncNotice(bt("remoteDoneUnsynced"));
        });
      }
      notify();
      // 排队式:本轮跑完,若该 session 不忙且有待发消息 → 自动发下一条
      if (reconciled) flushQueued(sid);
    })();
  });

  listen("chat:usage", function (e) { onSessionEvent(e, function () {
    const sid = e.payload && e.payload.session_id;
    // 真实窗口是模型能力常量，不随轮内请求数变化，必须先于 dirty guard 消费：
    // 工具轮（最常见的 Agent 场景）只跳过不可信的累计 input，分母仍要更新。
    const windowTok = Number(e.payload && e.payload.context_window) || 0;
    if (windowTok > 0 && windowTok !== state.tokens.max) {
      state.tokens.max = windowTok; // 云端真实窗口，替代 32K 假分母
      notify(); // 窗口变化也要通知 UI（即使本轮 input 不可信）
    }
    if (sid && turnUsageDirty[sid]) return; // 本轮多请求，累加 input 不可信，保留上个准确值
    const input = Number(e.payload && e.payload.input_tokens || 0);
    // 累加值超过窗口说明仍有多请求（内部重试等无事件轮），跳过避免显示超上限
    if (input > 0 && input <= state.tokens.max) {
      state.tokens = { input, max: state.tokens.max };
      notify();
    }
  }); });

  listen("chat:compaction", function (e) { onSessionEvent(e, function () {
    if (e.payload && e.payload.session_id) turnUsageDirty[e.payload.session_id] = true; // 压缩轮 usage 含摘要请求
    const phase = e.payload && e.payload.phase;
    const msg = e.payload && e.payload.message || "";
    const auto = e.payload && e.payload.auto ? bt("compactAuto") : "";
    const compactId = e.payload && e.payload.id;
    const before = Number(e.payload && e.payload.messages_before);
    const after = Number(e.payload && e.payload.messages_after);
    const looksLikePruneOnly = /0 removed|messages unchanged|tool results pruned/i.test(msg);
    const pruneOnlyAuto = !!(e.payload && e.payload.auto) &&
      phase === "done" &&
      Number.isFinite(before) &&
      Number.isFinite(after) &&
      before === after &&
      looksLikePruneOnly &&
      msg.indexOf("Emergency compaction") !== 0;
    if (phase === "start") addSystemItem(bt("compactStart") + auto + " " + msg, { compactId, compactPhase: "start" });
    else if (phase === "done" && pruneOnlyAuto) addOrMergePruneCompaction(compactId);
    else if (phase === "done") addSystemItem(bt("compactDone") + auto + " " + msg);
    else if (phase === "fail") addSystemItem(bt("compactFail") + auto + ": " + msg);
  }); });

  // ── request_user_input：渲染选择卡片（不进 messages.json）─────────
  // payload: { id: tool_call_id, questions: [{header, id, question, options:[{label, description}]}] }
  listen("chat:user_input_required", function (e) { onSessionEvent(e, function () {
    const p = e.payload || {};
    const questions = p.questions || [];
    if (!Array.isArray(questions) || questions.length === 0) return;
    if (hasChatItemForTool("user_input", p.id)) return;
    addChatItem({
      type: "user_input", toolCallId: p.id, questions,
      resolved: false, cardState: "active", time: timeStr(),
    });
    notify();
  }); });

  // 可恢复的瞬态错误（SSE idle timeout / 瞬态工具失败）：turn 没结束，引擎会 retry，
  // 绝不 setBusy(false)，只飘一条 ⚠️ 提示。
  listen("chat:transient_error", function (e) { onSessionEvent(e, function () {
    if (e.payload && e.payload.session_id) turnUsageDirty[e.payload.session_id] = true; // 重试轮 usage 含重发请求
    const error = e.payload && e.payload.error;
    if (error) {
      const notice = "⚠️ " + error;
      const duplicate = state.chatItems.some(function (item) {
        return item && item.turnErrorNotice && item.text === notice;
      });
      if (!duplicate) addSystemItem(notice, { turnErrorNotice: true });
    }
    // 401/鉴权失败:刷新 effectiveModelConfig → 前端拦截遮罩自动弹出引导配置。
    // 兜底启动检测被绕过/中途删 key 的场景。
    // \b401\b 词边界锚定,避免误匹配 "port 4014"/"row 401" 等含 401 子串的无关报错。
    if (error && /\b401\b|unauthorized|authentication/i.test(String(error))) loadEffectiveModelConfig();
  }); });

  // File watcher 推送的产物事件：session workspace 下新文件/修改/删除。
  // 路由到对应 session 的产物列表(后台 session 的产物也跟踪)。
  listen("artifact:disk", function (e) {
    const p = e.payload || {};
    if (!p.path) return;
    // 公共 MCP 产物目录没有真实 session 归属；归属由对应 chat:tool_end
    // 的 session_id 决定。这里跳过，避免文件系统事件把 PPT/公文错塞到 default/当前会话。
    if ((p.session_id === "default" || !p.session_id) && isSharedMcpArtifactPath(p.path)) return;
    onSessionEvent(e, function () {
      noteArtifactChange(p.path, p.event || "modified", p.session_id || state.activeSessionId || "");
      if (p.event === "removed") { untrackArtifact(p.path); return; }
      // 面板只收成品:成品型扩展名 或 present_artifact 过的;中间 / infra / 目录不进面板
      // (file_watcher 递归会推 tmp/ _state/ 等子目录与 infra 文件 → 此处兜住)。
      if (isDeliverable(p.path) || findPresentedArtifact(p.path)) trackArtifact(p.path);
    });
  });

  // 本地语音识别依赖安装进度（模型下载 / ffmpeg 安装）
  listen("voice_asr:progress", function (e) {
    const p = e && e.payload;
    if (!p) return;
    state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, { progress: p });
    notify();
  });

  // vllm-setup:phase —— MegaCube 本地大模型引导阶段(authorizing→waiting{attempt}→ready),驱动引导框步骤指示。
  listen("vllm-setup:phase", function (e) {
    const p = e.payload || {};
    if (!p.phase) return;
    state.vllmSetupPhase = p.phase;
    if (typeof p.attempt === "number") state.vllmSetupAttempt = p.attempt;
    notify();
  });

  // 知识库 embedding 模型下载进度（download → verify → prepare → done）
  listen("kb_model:progress", function (e) {
    const p = e && e.payload;
    if (!p) return;
    state.kbModelSetup = Object.assign({}, state.kbModelSetup, { progress: p });
    notify();
  });

  // A second local process (the bundled shared-knowledge host) can install the
  // managed model after startup. Replace the cached snapshot when the backend
  // publishes a newly observed status.
  listen("kb_model:status", function (e) {
    const status = e && e.payload;
    if (!status) return;
    state.kbModelSetup = Object.assign({}, state.kbModelSetup, {
      startupLoading: !!status.loading,
      startupReady: typeof status.ready === "boolean" ? status.ready : state.kbModelSetup.startupReady,
      status,
    });
    notify();
  });

  // chat:plan_snapshot —— update_plan/checklist_write 后实时更新进度，与 plan_ready 解耦
  listen("chat:plan_snapshot", function (e) { onSessionEvent(e, function () {
    const p = e.payload || {};
    if (p.plan_snapshot) state.planSnapshot.plan = p.plan_snapshot;
    if (p.todos_snapshot) state.planSnapshot.todos = p.todos_snapshot;
    notify();
  }); });

  // chat:plan_ready —— 底座式:Plan 模式调过 update_plan 即弹方案卡(快照非空)
  listen("chat:plan_ready", function (e) { onSessionEvent(e, function () {
    const p = e.payload || {};
    const planId = String(p.plan_id || p.planId || "").trim();
    const readyMode = p.mode_state || p.modeState;
    // 事件负载的权威 mode 写回走收敛点（bump seq 防在途旧读覆盖）。
    if (readyMode) applyAuthoritativeModeState(state.activeSessionId, readyMode);
    if (planId && state.chatItems.some(function (item) {
      return item && item.type === "plan_card" && String(item.planId || "") === planId;
    })) return;
    // 新方案出现 → 旧的 active 方案卡冻结
    state.chatItems.forEach(function (it) {
      if (it.type === "plan_card" && it.cardState === "active") {
        it.cardState = "frozen"; it.statusLabel = bt("planSuperseded");
      }
    });
    const snaps = { plan: p.plan_snapshot || null, todos: p.todos_snapshot || null };
    addChatItem({
      type: "plan_card", plan: snaps.plan, todos: snaps.todos,
      planMarkdown: composePlanMarkdown(snaps), planId: planId || null,
      cardState: planId ? "active" : "frozen", resolved: !planId,
      planResolutionConfirmed: false,
      statusLabel: planId ? "" : bt("planHistorical"), time: timeStr(),
    });
    notify();
  }); });

  // A discard is a shared plan-state transition but not a model turn. Apply it
  // directly to the matching ticket without marking the Session busy or adding
  // a synthetic user bubble.
  listen("chat:plan_resolved", function (e) {
    const p = e && e.payload || {};
    const sid = p.session_id || state.activeSessionId;
    const planId = String(p.plan_id || p.planId || "").trim();
    if (!sid || !planId) return;
    runSyncOnSession(sid, function () {
      state.chatItems.forEach(function (item) {
        if (item && item.type === "plan_card" && String(item.planId || "") === planId) {
          item.cardState = "frozen";
          item.resolved = true;
          item.planResolutionConfirmed = true;
          item.statusLabel = bt("planDiscarded");
        }
      });
      const resolvedMode = p.mode_state || p.modeState;
      // 事件负载的权威 mode 写回走收敛点（bump seq 防在途旧读覆盖）。
      if (resolvedMode) applyAuthoritativeModeState(sid, resolvedMode);
    });
    notify();
  });

  // ── Monitor ──────────────────────────────────────────────────────
  const PinvouFU = window.PinvouFormatUtils || {};
  const fmtMiB = PinvouFU.fmtMiB || function (mib) { return mib == null ? "—" : String(mib); };
  const fmtKiB = PinvouFU.fmtKiB || function (kib) { return kib == null ? "—" : String(kib); };
  const fmtDuration = PinvouFU.fmtDuration || function (secs) { return secs == null ? "—" : String(secs); };
  const fmtTok = PinvouFU.fmtTok || function (n) { return n == null ? "—" : String(n); };


  function numOr0(x) { return (typeof x === "number" && Number.isFinite(x)) ? x : 0; }

  // 用基准点把累计 counter 换算成「自清除以来」的区间值。sp=app 自测(snap.self_perf,
  // TTFT/TPS/tokens 全从这);v=vllm(仅 KV 的本地 prefix_cache 分支要它)。无基准 → 直接
  // 用进程生命周期累计值。任一 counter 倒退（< 基准：app 或 vLLM 重启、counter 归零）
  // → 丢弃失效基准，回落到累计值，避免负数。
  // KV 命中率(混合):本地 vLLM 用 /metrics prefix_cache(vllmKvPct);拿不到再用 usage 的
  // cache token 口径(selfKvPct,给云端/D3)。二者都按区间(扣基准)重算。
  function adjustCounters(sp, v) {
    sp = sp || {};
    const kvRatio = function (hit, miss) {
      const d = hit + miss;
      return d > 0 ? (hit / d * 100) : null;
    };
    let b = monitorBaseline;
    if (b) {
      const reset =
        numOr0(sp.ttft_sum_s) < b.ttft_sum_s ||
        numOr0(sp.tps_time_s) < b.tps_time_s ||
        numOr0(sp.gen_tokens_total) < b.gen_tokens ||
        numOr0(sp.prompt_tokens_total) < b.prompt_tokens ||
        numOr0(sp.cache_hit_tokens) < b.cache_hit ||
        numOr0(sp.cache_miss_tokens) < b.cache_miss ||
        (v && numOr0(v.prefix_cache_queries) < numOr0(b.pc_queries));
      if (reset) { clearMonitorBaseline(); b = null; }
    }
    const base = function (k) { return b ? numOr0(b[k]) : 0; };
    let vllmKvPct = null;
    if (v) {
      const pcH = numOr0(v.prefix_cache_hits) - base("pc_hits");
      const pcQ = numOr0(v.prefix_cache_queries) - base("pc_queries");
      vllmKvPct = pcQ > 0 ? (pcH / pcQ * 100) : null;
    }
    return {
      cleared: !!b,
      ttft_sum_s: numOr0(sp.ttft_sum_s) - base("ttft_sum_s"),
      ttft_count: numOr0(sp.ttft_count) - base("ttft_count"),
      tps_tokens: numOr0(sp.tps_tokens) - base("tps_tokens"),
      tps_time_s: numOr0(sp.tps_time_s) - base("tps_time_s"),
      gen: numOr0(sp.gen_tokens_total) - base("gen_tokens"),
      prompt: numOr0(sp.prompt_tokens_total) - base("prompt_tokens"),
      vllmKvPct,
      selfKvPct: kvRatio(
        numOr0(sp.cache_hit_tokens) - base("cache_hit"),
        numOr0(sp.cache_miss_tokens) - base("cache_miss")
      ),
      clearedAt: b ? (b.at || null) : null,
    };
  }

  function clearMonitorBaseline() {
    monitorBaseline = null;
    try { localStorage.removeItem(MONITOR_BASELINE_KEY); } catch { /* ignore when localStorage is unavailable */ }
  }

  // 把当前 counter 快照存为基准点 → 监控页「后 4 项」从此刻起重新计。
  // 自测计数(TTFT/TPS/tokens/usage-cache)+ vLLM prefix_cache(供本地 KV 分支)一起存。
  function clearMonitorStats() {
    const sp = state.monitor && state.monitor.self_perf;
    if (!sp) return false;
    const v = (state.monitor && state.monitor.vllm) || {};
    monitorBaseline = {
      ttft_sum_s: numOr0(sp.ttft_sum_s),
      ttft_count: numOr0(sp.ttft_count),
      tps_tokens: numOr0(sp.tps_tokens),
      tps_time_s: numOr0(sp.tps_time_s),
      gen_tokens: numOr0(sp.gen_tokens_total),
      prompt_tokens: numOr0(sp.prompt_tokens_total),
      cache_hit: numOr0(sp.cache_hit_tokens),
      cache_miss: numOr0(sp.cache_miss_tokens),
      pc_hits: numOr0(v.prefix_cache_hits),
      pc_queries: numOr0(v.prefix_cache_queries),
      at: Date.now(),  // 记录清除时刻，供「统计自 HH:MM 起」状态文字
    };
    try { localStorage.setItem(MONITOR_BASELINE_KEY, JSON.stringify(monitorBaseline)); } catch { /* ignore when localStorage is unavailable */ }
    pollMonitor();  // 立即刷新显示，无需等下一个轮询周期
    return true;
  }

  function appQueueSnapshot() {
    let waiting = state.queued ? state.queued.length : 0;
    const busyMap = {};
    for (const id in sessionStates) {
      // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 floor: Object.hasOwn is unavailable; this call is already the safe form
      if (!Object.prototype.hasOwnProperty.call(sessionStates, id)) continue;
      if (id === state.activeSessionId) continue;
      const buf = sessionStates[id] || {};
      if (buf.busy) busyMap[id] = true;
      if (Array.isArray(buf.queued)) waiting += buf.queued.length;
    }
    if (state.activeSessionId && state.busy) busyMap[state.activeSessionId] = true;
    const running = Object.keys(busyMap).length;
    return { running, waiting };
  }

  // eslint-disable-next-line sonarjs/cognitive-complexity -- legacy bridge; refactor tracked separately
  async function pollMonitor() {
    if (monitorPollInFlight) return;
    monitorPollInFlight = true;
    try {
      const snap = await invoke("get_monitor_snapshot");
      state.monitorError = null;
      // GPU util sliding window
      if (snap.gpu) {
        gpuUtilHistory.push(snap.gpu.utilization_pct);
        if (gpuUtilHistory.length > 5) gpuUtilHistory.shift();
        snap.gpu._utilMax = Math.max(...[0, ...gpuUtilHistory]);
      }
      // 监控页「后 4 项」累计指标：TTFT/TPS/tokens 来自 app 侧自测(snap.self_perf,
      // 任何后端都有);KV 混合(本地 vLLM prefix_cache 优先,否则 usage 口径)。
      // 按「清除统计」基准点换算成区间值后再格式化。
      const sadj = adjustCounters(snap.self_perf, snap.vllm);
      // KV 显示值:本地 vLLM 的 /metrics prefix_cache 优先,拿不到用 usage cache 口径(云端)。
      const kvShown = sadj ? (sadj.vllmKvPct == null ? (sadj.selfKvPct == null ? null : sadj.selfKvPct)
        : sadj.vllmKvPct) : null;
      // Format values for display
      const vllm = snap.vllm || null;
      const metricsApplicable = vllm ? vllm.metrics_applicable !== false : false;
      const metricUnavailableText = bt("metricUnavailable");
      const diagnostic = vllm && vllm.diagnostic ? vllm.diagnostic : null;
      const metricDiagnostic = vllm && vllm.metric_diagnostics && vllm.metric_diagnostics.length
        ? vllm.metric_diagnostics[0] : null;
      const targetKind = vllm && vllm.target_kind ? vllm.target_kind : "invalid";
      const targetKindLabel = targetKind === "remote" ? bt("targetKindRemote") : (targetKind === "local" ? bt("targetKindLocal") : bt("targetKindInvalid"));
      const vllmDisplayModel = vllm ? (vllm.model || vllm.configured_model || "—") : "—";
      const healthStatus = vllm && vllm.health_status ? vllm.health_status : (vllm ? "verified" : "offline");
      const appQueue = appQueueSnapshot();
      snap._fmt = {
        gpuName: snap.gpu ? snap.gpu.name : bt("gpuUnavailable"),
        gpuVram: snap.gpu && snap.gpu.vram_total_mib > 0
          ? fmtMiB(snap.gpu.vram_used_mib) + " / " + fmtMiB(snap.gpu.vram_total_mib) : "—",
        gpuVramPct: snap.gpu && snap.gpu.vram_total_mib > 0
          ? Math.round(snap.gpu.vram_used_mib / snap.gpu.vram_total_mib * 100) : 0,
        gpuUtil: snap.gpu ? (snap.gpu._utilMax + "%") : "—",
        gpuUtilPct: snap.gpu ? snap.gpu._utilMax : 0,
        processorUtil: snap.gpu && snap.gpu.processor_utilization_pct != null ? snap.gpu.processor_utilization_pct + "%" : "—",
        processorUtilPct: snap.gpu && snap.gpu.processor_utilization_pct != null ? snap.gpu.processor_utilization_pct : 0,
        gpuSharedMemory: snap.gpu && snap.gpu.shared_memory_used_mib != null ? fmtMiB(snap.gpu.shared_memory_used_mib) : "—",
        gpuTemp: snap.gpu && snap.gpu.temperature_c != null ? snap.gpu.temperature_c + "°C" : null,
        gpuPower: snap.gpu && snap.gpu.power_w != null ? snap.gpu.power_w.toFixed(1) + " W" : null,
        gpuAvailable: !!snap.gpu,
        gpuHasVram: !!(snap.gpu && snap.gpu.vram_total_mib > 0),
        ramUsed: snap.ram ? fmtKiB(snap.ram.used_kib) : "—",
        ramTotal: snap.ram ? fmtKiB(snap.ram.total_kib) : "—",
        ramPct: snap.ram && snap.ram.total_kib > 0 ? Math.round(snap.ram.used_kib / snap.ram.total_kib * 100) : 0,
        ramUsedGiB: snap.ram ? (snap.ram.used_kib / 1024 / 1024).toFixed(1) : "—",
        swapUsed: snap.ram ? fmtKiB(snap.ram.swap_used_kib) : "—",
        swapTotal: snap.ram ? fmtKiB(snap.ram.swap_total_kib) : "—",
        swapPct: snap.ram && snap.ram.swap_total_kib > 0 ? Math.round(snap.ram.swap_used_kib / snap.ram.swap_total_kib * 100) : 0,
        vllmModel: vllmDisplayModel,
        vllmConfiguredModel: vllm ? (vllm.configured_model || null) : null,
        vllmModelMismatch: vllm && vllm.configured_model && vllm.model
          ? vllm.configured_model !== vllm.model : false,
        vllmStatus: vllm ? vllm.status.toUpperCase() : "OFFLINE",
        vllmHealthStatus: healthStatus,
        vllmOnline: vllm ? (healthStatus === "verified" && (vllm.status === "ready" || vllm.status === "busy")) : false,
        vllmUpstream: vllm ? (vllm.upstream || "—") : "—",
        vllmTargetKind: targetKindLabel,
        // 云端(remote)不做健康探测(无 auth 的 /v1/models 必 401)→ 不显示 OFFLINE。
        // 暴露原始 kind 供前端判定(别比本地化 label)。
        vllmIsRemote: targetKind === "remote",
        vllmDiagnostic: diagnostic ? diagnostic.message : null,
        vllmDiagnosticCode: diagnostic ? diagnostic.code : null,
        vllmMetricsApplicable: metricsApplicable,
        vllmMetricDiagnostic: metricDiagnostic ? metricDiagnostic.message : null,
        vllmMaxLen: vllm ? (metricsApplicable ? (vllm.max_model_len || "—") : (vllm.max_model_len || metricUnavailableText)) : "—",
        // 本地推理引擎(target_kind=local)且探测窗口 < 128k(131072):监控卡给告警。
        // 云端(remote)/v1/models 不返回 max_model_len,自然不触发。传原始值供前端拼文案。
        vllmCtxWarn: (vllm && targetKind === "local" && vllm.max_model_len && vllm.max_model_len < 131072)
          ? vllm.max_model_len : null,
        vllmQueue: appQueue.running + " / " + appQueue.waiting,
        vllmQueueSource: "app",
        // TTFT/TPS/tokens 一律用 app 侧自测——任何后端(vLLM/LM Studio/Ollama/云端)都有值,
        // 不再受 metricsApplicable 门控。KV 见 kvShown(本地 prefix_cache / 云端 usage 口径),
        // 拿不到则 "—"。队列仍归 vLLM(见 vllmQueue)。
        vllmKv: kvShown == null ? "0%" : kvShown.toFixed(1) + "%",
        vllmKvHasData: kvShown != null,
        vllmTtft: sadj && sadj.ttft_count > 0
          ? (sadj.ttft_sum_s / sadj.ttft_count).toFixed(2) + " s" : "—",
        vllmTps: sadj && sadj.tps_time_s > 0
          ? (sadj.tps_tokens / sadj.tps_time_s).toFixed(1) + " tok/s" : "—",
        vllmTokTotal: sadj
          ? fmtTok(sadj.gen) + " / " + fmtTok(sadj.prompt) : "—",
        vllmStatsCleared: !!(sadj && sadj.cleared),
        vllmClearedAt: sadj && sadj.cleared ? (sadj.clearedAt || null) : null,
        // 区间原始数值（已扣基准），供前端「长按清除」的数字归零插值动画用。
        vllmRaw: sadj ? {
          kvPct: kvShown,
          ttftS: sadj.ttft_count > 0 ? sadj.ttft_sum_s / sadj.ttft_count : null,
          tps: sadj.tps_time_s > 0 ? sadj.tps_tokens / sadj.tps_time_s : null,
          gen: sadj.gen == null ? null : sadj.gen,
          prompt: sadj.prompt == null ? null : sadj.prompt,
        } : null,
        appVersion: snap.app ? snap.app.pinvou3_version + bt("betaTag") : "—",
        dtVersion: snap.app ? snap.app.deepseek_tui_version : "—",
        uptime: snap.app ? fmtDuration(snap.app.session_uptime_secs) : "—",
        updatedAt: snap.generated_at_ms ? new Date(snap.generated_at_ms).toLocaleTimeString() : "—",
      };
      if (snap.vllm && snap.vllm.max_model_len) {
        maxModelLen = snap.vllm.max_model_len;
        state.tokens.max = maxModelLen;
      }
      state.monitor = snap;
      notify();
    } catch (e) {
      state.monitorError = e && e.message ? e.message : String(e || "monitor poll failed");
      console.warn("monitor poll failed", e);
      notify();
    } finally {
      monitorPollInFlight = false;
    }
  }

  function startMonitorPolling() {
    if (monitorIntervalId) return;
    gpuUtilHistory = [];
    pollMonitor();
    monitorIntervalId = setInterval(pollMonitor, 1000);
  }
  function stopMonitorPolling() {
    if (monitorIntervalId) {
      clearInterval(monitorIntervalId);
      monitorIntervalId = null;
    }
  }

  // ── Backend status (live dot) ────────────────────────────────────
  let backendStatusPollInFlight = false;
  async function pollBackendStatus() {
    if (backendStatusPollInFlight) return;
    backendStatusPollInFlight = true;
    try {
      const s = await invoke("get_backend_status");
      state.backendOnline = !!s.vllm_online;
      // 修 token 分母时机 bug：不再依赖用户打开监控页才拿到真实 max_model_len
      if (s.max_model_len) {
        maxModelLen = s.max_model_len;
        state.tokens.max = maxModelLen;
      }
    } catch {
      state.backendOnline = false;
    } finally {
      backendStatusPollInFlight = false;
    }
    notify();
  }

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
      // 快速切会话时，旧请求可能晚于新请求返回；禁止旧会话配置覆盖当前遮罩状态。
      if ((state.activeSessionId || null) !== requestedSessionId) return;
      state.effectiveModelConfig = config;
    } catch {
      if ((state.activeSessionId || null) !== requestedSessionId) return;
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
        state.settings = await invoke(IS_WEB ? "web_access_update_settings" : "update_settings", { patch });
        notify();
        return true;
      } catch (e) {
        console.warn("save settings failed", e);
        return false;
      }
    });
  }
  async function saveSettingsAndRestart(patch) {
    if (IS_WEB) {
      console.warn("saveSettingsAndRestart is unsupported by the Web host");
      return false;
    }
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
        if (IS_WEB) {
          state.settings = await invoke("web_access_update_settings", { patch: { search } });
        } else {
          state.settings = await invoke("update_search_settings", { search });
        }
        notify();
        return true;
      } catch (e) {
        console.warn("save search settings failed", e);
        return false;
      }
    });
  }
  async function saveSearchSettingsAndRestart(search) {
    if (IS_WEB) {
      console.warn("saveSearchSettingsAndRestart is unsupported by the Web host");
      return false;
    }
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
  async function detectLocalVllmSetup(options) {
    const autoPoll = !!(options && options.autoPoll);
    if (vllmSetupPollTimer) {
      clearTimeout(vllmSetupPollTimer);
      vllmSetupPollTimer = null;
    }
    if (!autoPoll) vllmSetupPollStartedAt = Date.now();
    try {
      state.vllmSetup = await invoke("detect_local_vllm_setup");
    } catch {
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
  // （审计 b）。请求序号后发者胜（与 tauri settings 侧一致）。
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
   await loadEffectiveModelConfig();
 }
 async function revealModelApiKey(id) {
   return invoke("reveal_model_api_key", { id });
 }
 async function deleteModel(id) {
   await invoke("delete_model", { id });
   await loadModels();
   await loadEffectiveModelConfig();
  }
  async function setActiveModel(id) {
    await invoke("set_active_model", { id });
    await loadModels();
    await loadEffectiveModelConfig();
  }
  // 读某会话当前绑定的模型 id(切会话时刷新 chip)。
  async function loadSessionModel(sessionId) {
    const requestedSessionId = sessionId || null;
    let modelId = null;
    let config = null;
    try {
      const results = await Promise.all([
        requestedSessionId
          ? invoke("get_session_model_id", { sessionId: requestedSessionId }).catch(function () { return null; })
          : Promise.resolve(null),
        invoke("get_effective_model_config", { sessionId: requestedSessionId }).catch(function () { return null; }),
      ]);
      modelId = results[0];
      config = results[1];
    } catch {
      // on read failure, treat as unconfigured (null is submitted uniformly below).
    }
    // ChatView effect 可能并发加载相邻两个会话；只提交仍为当前会话的结果。
    if ((state.activeSessionId || null) !== requestedSessionId) return;
    state.currentSessionModelId = modelId;
    state.effectiveModelConfig = config;
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
    // Rust 侧按 base_url TTL 缓存；命令失败（web 白名单不含该命令/老版本桌面）
    // 在这里 reject，由消费方 catch 降级为「未知」——吞错伪造成 generic 会让 UI
    // 误报「该端点不支持思考档位调节」（localProbeTiersForKind('generic') 为 null）。
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
  async function testSearchProvider(provider, apiKey) {
    return invoke("test_search_provider", { provider, apiKey: apiKey || null });
  }

  // ── Super permission ─────────────────────────────────────────────
  async function refreshSuperPerm() {
    try {
      state.superPermEnabled = !!(await invoke("get_super_permission_status"));
    } catch {
      state.superPermEnabled = false;
    }
    notify();
  }
  async function toggleSuperPerm() {
    const target = !state.superPermEnabled;
    try {
      state.superPermEnabled = !!(await invoke("set_super_permission", { enabled: target }));
      addSystemItem(state.superPermEnabled
        ? bt("superOn")
        : bt("superOff"));
      notify();
      return { ok: state.superPermEnabled === target, enabled: state.superPermEnabled };
    } catch (e) {
      addSystemItem("⚠️ " + e);
      try { state.superPermEnabled = !!(await invoke("get_super_permission_status")); } catch { /* on query failure, treat as not enabled */ }
      notify();
      return { ok: false, enabled: state.superPermEnabled, error: String(e) };
    }
  }

  // ── Mode state ───────────────────────────────────────────────────
  // 会话级读取：await 挂起期间用户可能已切走，响应返回后必须校验发起时的
  // sid 仍是 active，否则把结果定向写回 sid 自己的 buffer，不污染当前显示。
  // 另加请求序号（modeSyncSeq 声明见文件顶部）：同一会话内并发读取乱序返回时，
  // 旧响应不得覆盖新响应（A→B→A 快速切换时 #1 的慢响应覆盖 #3 的新值，审计；
  // tauri 版 epoch 对齐）。权威写回一律 bump 该序号，见 applyAuthoritativeModeState。
  async function syncModeState() {
    const sid = state.activeSessionId;
    if (!sid) {
      // 草稿态：显示当前 lane 的全局默认（三分 lane 语义），不再恒 yolo。
      state.modeState = currentDraftModeState();
      return;
    }
    const seq = ++modeSyncSeq;
    try {
      const ms = await invoke("get_mode_state", { sessionId: state.activeSessionId });
      if (seq !== modeSyncSeq) return; // 已有更新的读取发起，本响应陈旧
      if (state.activeSessionId !== sid) {
        runSyncOnSession(sid, function () {
          state.modeState = { mode: ms.mode || "yolo", multiAgent: !!ms.multi_agent };
        });
        return;
      }
      state.modeState = { mode: ms.mode || "yolo", multiAgent: !!ms.multi_agent };
    } catch {
      if (seq !== modeSyncSeq) return;
      if (state.activeSessionId !== sid) return;
      state.modeState = { mode: "yolo", multiAgent: false };
    }
  }

  // ── lane 全局默认（工作/设计/代码三分，与 tauri bridge 对齐）────────
  // 草稿态（无 active 会话）的 modeState：取当前 lane 的全局默认，缺省 yolo。
  function currentDraftModeState() {
    const lane = state.modeLane === "design" ? "design" : "work";
    const d = state.modeDefaults && state.modeDefaults[lane];
    return { mode: d || "yolo", multiAgent: false };
  }
  async function refreshModeDefaults() {
    try {
      const defaults = await invoke("get_mode_defaults");
      if (defaults) state.modeDefaults = defaults;
    } catch { /* 读取失败保留旧值/缺省，不打扰交互 */ }
    if (!state.activeSessionId) {
      state.modeState = currentDraftModeState();
      notify();
    }
  }
  // ChatView 随 pinvouMode 传入当前 lane；草稿态立即按新 lane 默认刷新显示。
  function setModeLane(lane) {
    const next = lane === "design" ? "design" : "work";
    if (state.modeLane === next) return;
    state.modeLane = next;
    if (!state.activeSessionId) {
      state.modeState = currentDraftModeState();
      notify();
    }
  }
  // 草稿态 chip 切换：写本 lane 全局默认（不物化会话——物化时由
  // ensureSession 把 lane 默认应用到新会话）。
  async function setDraftMode(target) {
    const lane = state.modeLane === "design" ? "design" : "work";
    try {
      const defaults = await invoke("set_mode_default", { lane, mode: target });
      if (defaults) state.modeDefaults = defaults;
      if (!state.activeSessionId) {
        state.modeState = {
          mode: target,
          multiAgent: !!(state.modeState && state.modeState.multiAgent),
        };
      }
    } catch (e) { addSystemItem(bt("switchModeFailed") + e); }
    notify();
  }

  // ── 卡片动作辅助 ─────────────────────────────────────────────────
  function patchItemById(id, patch) {
    for (let i = 0; i < state.chatItems.length; i++) {
      if (state.chatItems[i].id === id) { Object.assign(state.chatItems[i], patch); break; }
    }
  }
  function pushUserEcho(text, persist) {
    const item = { type: "user", text, time: timeStr() };
    let message = null;
    addChatItem(item);
    if (persist) {
      message = { role: "user", content: [{ type: "text", text }] };
      state.messages.push(message);
    }
    return { item, message };
  }
  function markResolved(id, statusLabel) { patchItemById(id, { resolved: true, statusLabel: statusLabel || "" }); notify(); }

  // ── Per-session UI 路由 ─────────────────────────────────────────
  // 卡片动作链路有多个 await 边界,用户可能中途切 session。所有 UI 写入(chatItem 增改、
  // pending* 标记、modeState 同步)必须落在【触发 session】的 buffer 上,不能跟着
  // state.activeSessionId 漂走。一律 wrap 进 runSyncOnSession 是因为:sid === active
  // 时它是 no-op 直通,sid !== active 时它 swap-load-fn-save 回 sid 的 buffer。
  function runOnSession(sid, fn) { runSyncOnSession(sid || state.activeSessionId, fn); }
  function addSystemItemFor(sid, text) { runOnSession(sid, function () { addSystemItem(text); }); }
  function patchItemByIdFor(sid, id, patch) { runOnSession(sid, function () { patchItemById(id, patch); }); }

  // 记忆状态值是固定中文数据（会被持久化，且 React 侧按固定键做逻辑判断与本地化映射），
  // 因此这里刻意不走 bt()，与 tauri 端 memory.js 保持一致。
  function memoryWriteLabel(event) {
    const text = event && event.text || "";
    if (!text) return "记忆已更新";
    return text;
  }
  function memoryWriteStatusLabel(event) {
    const action = event && event.action || "";
    if (action === "confirmed" || action === "remembered") return "记忆已更新";
    if (action === "archived") return "记忆已归档";
    if (action === "deleted") return "记忆已删除";
    return "记忆已更新";
  }
  function normalizeMemoryCandidateText(text) {
    return String(text || "").replaceAll(/\s+/g, " ").trim().toLowerCase();
  }
  function handleMemoryWrite(payload) {
    const sid = payload && payload.session_id || state.activeSessionId;
    const events = payload && Array.isArray(payload.events) ? payload.events : [];
    if (!sid || !events.length) return;
    runOnSession(sid, function () {
      events.forEach(function (event) {
        if (!event) return;
        if (event.action === "pending") {
          const label = memoryWriteLabel(event);
          const labelKey = normalizeMemoryCandidateText(label);
          const existing = state.chatItems.find(function (it) {
            return it.type === "memory_candidate" && !it.resolved && (
              (event.id && it.memoryId === event.id) ||
              (labelKey && normalizeMemoryCandidateText(it.text) === labelKey)
            );
          });
          if (existing) {
            existing.memoryId = event.id || existing.memoryId;
            existing.kind = event.kind || existing.kind || "preference";
            existing.text = label;
            existing.time = timeStr();
            return;
          }
          addChatItem({
            type: "memory_candidate",
            memoryId: event.id,
            kind: event.kind || "preference",
            text: label,
            time: timeStr(),
            resolved: false,
          });
          return;
        }
        const label = memoryWriteLabel(event);
        const labelKey = normalizeMemoryCandidateText(label);
        const existing = state.chatItems.find(function (it) {
          return it.type === "memory_candidate" && (
            (event.id && it.memoryId === event.id) ||
            (labelKey && normalizeMemoryCandidateText(it.text) === labelKey)
          );
        });
        if (existing) {
          if (event.action === "ignored" || event.action === "never") {
            state.chatItems = state.chatItems.filter(function (it) { return it !== existing; });
            return;
          }
          existing.resolved = true;
          existing.statusLabel = event.action === "ignored" ? "已忽略"
            : event.action === "never" ? "不再提示"
            : event.action === "archived" ? "已归档"
            : event.action === "deleted" ? "已删除"
            : "已记住";
          existing.kind = event.kind || existing.kind || "preference";
          existing.text = label;
          existing.time = timeStr();
          return;
        }
        if (event.action === "ignored" || event.action === "never") {
          return;
        }
        addChatItem({
          type: "memory_notice",
          memoryId: event.id,
          kind: event.kind || "preference",
          text: label,
          statusLabel: memoryWriteStatusLabel(event),
          time: timeStr(),
        });
      });
      notify();
    });
    if (invoke) {
      setTimeout(function () {
        loadMemoryOverview({ rehydratePending: true });
      }, 0);
    }
  }

  function applyMemoryOverview(overview) {
    const previous = state.memory || {};
    const sourceStates = overview && overview.sources || {};
    // stateKey:后端 source 名与前端 state 字段名通常一致,但 snapshot 源对应
    // state.memory.snapshot_path,两者不同;保留上次值时按 state 字段名查找。
    function sourceValue(source, value, fallback, stateKey) {
      const status = sourceStates[source];
      if (status && status.available === false) {
        const key = stateKey || source;
        // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 floor: Object.hasOwn is unavailable; this call is already the safe form
        return Object.prototype.hasOwnProperty.call(previous, key) ? previous[key] : fallback;
      }
      return value;
    }
    state.memory = {
      loading: false,
      error: null,
      profile: sourceValue("profile", overview && overview.profile || null, null),
      preferences: sourceValue("preferences", overview && Array.isArray(overview.preferences) ? overview.preferences : [], []),
      work_context: sourceValue("work_context", overview && Array.isArray(overview.work_context) ? overview.work_context : [], []),
      current_focus: sourceValue("current_focus", overview && Array.isArray(overview.current_focus) ? overview.current_focus : [], []),
      recent_activity: sourceValue("recent_activity", overview && Array.isArray(overview.recent_activity) ? overview.recent_activity : [], []),
      recent_work: sourceValue("recent_work", overview && Array.isArray(overview.recent_work) ? overview.recent_work : [], []),
      pending: sourceValue("pending", overview && Array.isArray(overview.pending) ? overview.pending : [], []),
      never: sourceValue("never", overview && Array.isArray(overview.never) ? overview.never : [], []),
      runtime: sourceValue("runtime", overview && overview.runtime || null, null),
      snapshot_path: sourceValue("snapshot", overview && overview.snapshot_path || "", "", "snapshot_path"),
      warnings: orderedMemoryWarnings(overview && overview.warnings),
      sources: sourceStates,
    };
  }
  function orderedMemoryWarnings(warnings) {
    const items = Array.isArray(warnings) ? warnings : [];
    return [
      ...items.filter(function (warning) {
        return warning && warning.code === "memory_topic_cleanup_required";
      }),
      ...items.filter(function (warning) {
        return !warning || warning.code !== "memory_topic_cleanup_required";
      }),
    ];
  }
  function applyMemoryProfileState(result) {
    if (!result || !result.profile) return;
    state.memory = Object.assign({}, state.memory, {
      loading: false,
      error: null,
      profile: result.profile,
      runtime: result.runtime || null,
      warnings: orderedMemoryWarnings(result.warnings),
    });
  }
  function applyMemoryWriteState(result, update) {
    if (!result) return;
    const next = Object.assign({}, state.memory, {
      loading: false,
      error: null,
      runtime: result.runtime || null,
      warnings: orderedMemoryWarnings(result.warnings),
    });
    if (update) update(next, result.value);
    state.memory = next;
    notify();
  }
  function upsertMemoryValue(items, value, replacedId) {
    if (!value) return items || [];
    const next = (items || []).filter(function (item) {
      return item && item.id !== value.id && item.id !== replacedId;
    });
    next.push(value);
    return next;
  }
  function upsertPendingMemoryCandidate(item) {
    if (!item || item.status !== "pending_confirm") return;
    const label = item.content || item.text || "";
    if (!label) return;
    const labelKey = normalizeMemoryCandidateText(label);
    const existing = state.chatItems.find(function (it) {
      return it.type === "memory_candidate" && !it.resolved && (
        (item.id && it.memoryId === item.id) ||
        (labelKey && normalizeMemoryCandidateText(it.text) === labelKey)
      );
    });
    if (existing) {
      existing.memoryId = item.id || existing.memoryId;
      existing.kind = item.kind || existing.kind || "preference";
      existing.text = label;
      return;
    }
    addChatItem({
      type: "memory_candidate",
      memoryId: item.id,
      kind: item.kind || "preference",
      text: label,
      time: timeStr(),
      resolved: false,
    });
  }
  function rehydratePendingMemoryCandidates(overview) {
    const pending = overview && Array.isArray(overview.pending) ? overview.pending : [];
    pending.forEach(upsertPendingMemoryCandidate);
  }
  // 记忆面板混合两类数据：runtime 按 session 分文件，profile/preferences/
  // pending 等为全局单文件(见后端 paths.rs)。加载仍必须带归属+序号校验
  // (与 tauri memory.js 对齐，审计)：await 挂起期间切会话或再次加载，旧
  // 响应不得覆盖当前显示(尤其 runtime 属于别的会话)，也不得把候选卡
  // rehydrate 进当前对话流。
  let memoryOverviewSeq = 0;
  async function loadMemoryOverview(options) {
    if (!invoke) return null;
    options = options || {};
    const sid = state.activeSessionId;
    const seq = ++memoryOverviewSeq;
    state.memory = Object.assign({}, state.memory, { loading: true, error: null });
    notify();
    try {
      const overview = await invoke("get_memory_overview", { sessionId: state.activeSessionId });
      if (sid !== state.activeSessionId || seq !== memoryOverviewSeq) return discardStaleLoad(seq);
      applyMemoryOverview(overview);
      if (options.rehydratePending) rehydratePendingMemoryCandidates(overview);
      notify();
      return overview;
    } catch (e) {
      if (sid !== state.activeSessionId || seq !== memoryOverviewSeq) return discardStaleLoad(seq);
      state.memory = Object.assign({}, state.memory, { loading: false, error: String(e) });
      notify();
      return null;
    }
  }
  // 守卫命中的善后：序号已被更新加载接管时由它负责收尾 loading；仅会话
  // 变化、无人接管时(如切草稿不续发加载)必须自己清掉 loading，否则面板
  // 永远停在"同步中"(与 tauri 对齐，审计补充)。
  function discardStaleLoad(seq) {
    if (seq === memoryOverviewSeq) {
      state.memory = Object.assign({}, state.memory, { loading: false });
      notify();
    }
    return null;
  }
  async function saveMemoryProfilePatch(patch) {
    if (!invoke) return null;
    // 入口捕获触发会话：invoke 往返期间切走，A 的写结果/错误不得渲染进
    // B 的面板(与 tauri memory.js 对齐，审计补充)。
    const sid = state.activeSessionId;
    try {
      const result = await invoke("update_memory_profile", { patch: patch || {}, sessionId: state.activeSessionId });
      if (sid === state.activeSessionId) { applyMemoryProfileState(result); notify(); }
      const overview = await loadMemoryOverview();
      return overview || result;
    } catch (e) {
      if (sid === state.activeSessionId) {
        state.memory = Object.assign({}, state.memory, { error: String(e) });
        notify();
      }
      throw e;
    }
  }
  async function deleteMemoryPreference(id) {
    if (!id || !invoke) return false;
    const sid = state.activeSessionId; // 同 saveMemoryProfilePatch：切走后不写 B 的面板(与 tauri 对齐，审计补充)
    try {
      const res = await invoke("delete_memory_preference", { id, sessionId: state.activeSessionId });
      if (sid === state.activeSessionId) {
        applyMemoryWriteState(res, function (next, changed) {
          if (changed) next.preferences = (next.preferences || []).filter(function (item) { return item.id !== id; });
        });
      }
      await loadMemoryOverview();
      return !!(res && res.value);
    } catch (e) {
      if (sid === state.activeSessionId) {
        state.memory = Object.assign({}, state.memory, { error: String(e) });
        notify();
      }
      throw e;
    }
  }
  async function updateMemoryItem(kind, id, patch) {
    if (!id || !invoke) return null;
    const sid = state.activeSessionId; // 同 saveMemoryProfilePatch：切走后不写 B 的面板(与 tauri 对齐，审计补充)
    try {
      const command = kind === "preference" ? "update_memory_preference"
        : kind === "work_context" ? "update_work_context_memory"
        : (kind === "current_focus" || kind === "recent_activity") ? "update_timed_memory"
        : null;
      if (!command) return null;
      const args = { id, patch: patch || {}, sessionId: state.activeSessionId };
      if (command === "update_timed_memory") args.kind = kind;
      const res = await invoke(command, args);
      if (sid === state.activeSessionId) {
        applyMemoryWriteState(res, function (next, value) {
          if (!value) return;
          const source = kind === "preference" ? "preferences" : kind;
          next[source] = upsertMemoryValue(next[source], value, id);
        });
      }
      await loadMemoryOverview();
      return res && res.value;
    } catch (e) {
      if (sid === state.activeSessionId) {
        state.memory = Object.assign({}, state.memory, { error: String(e) });
        notify();
      }
      throw e;
    }
  }
  async function deleteMemoryItem(kind, id) {
    if (!id || !invoke) return false;
    const sid = state.activeSessionId; // 同 saveMemoryProfilePatch：切走后不写 B 的面板(与 tauri 对齐，审计补充)
    try {
      const command = kind === "preference" ? "delete_memory_preference"
        : kind === "work_context" ? "delete_work_context_memory"
        : (kind === "current_focus" || kind === "recent_activity") ? "delete_timed_memory"
        : null;
      if (!command) return false;
      const args = { id, sessionId: state.activeSessionId };
      if (command === "delete_timed_memory") args.kind = kind;
      const res = await invoke(command, args);
      if (sid === state.activeSessionId) {
        applyMemoryWriteState(res, function (next, changed) {
          if (!changed) return;
          const source = kind === "preference" ? "preferences" : kind;
          next[source] = (next[source] || []).filter(function (item) { return item.id !== id; });
        });
      }
      await loadMemoryOverview();
      return !!(res && res.value);
    } catch (e) {
      if (sid === state.activeSessionId) {
        state.memory = Object.assign({}, state.memory, { error: String(e) });
        notify();
      }
      throw e;
    }
  }
  async function archiveRecentWorkMemory(id) {
    if (!id || !invoke) return false;
    const sid = state.activeSessionId; // 同 saveMemoryProfilePatch：切走后不写 B 的面板(与 tauri 对齐，审计补充)
    try {
      const res = await invoke("archive_recent_work_memory", { id, sessionId: state.activeSessionId });
      if (sid === state.activeSessionId) {
        applyMemoryWriteState(res, function (next, changed) {
          if (changed) next.recent_work = (next.recent_work || []).filter(function (item) { return item.id !== id; });
        });
      }
      await loadMemoryOverview();
      return !!(res && res.value);
    } catch (e) {
      if (sid === state.activeSessionId) {
        state.memory = Object.assign({}, state.memory, { error: String(e) });
        notify();
      }
      throw e;
    }
  }
  async function confirmMemoryCandidate(memoryId, chatItemId) {
    if (!memoryId) return;
    const sid = state.activeSessionId; // 入口捕获：候选卡 patch 与面板写入都定向回发起会话(与 tauri 对齐，审计补充)
    try {
      const result = await invoke("confirm_pending_memory", { id: memoryId, sessionId: sid });
      if (sid === state.activeSessionId) {
        applyMemoryWriteState(result, function (next) {
          next.pending = (next.pending || []).filter(function (item) { return item.id !== memoryId; });
        });
      }
      // patch 必须按发起会话路由(而非当前显示)：切走后写 B 的 chatItems 是
      // no-op，A 的候选卡会永远停留在"可点击未决"态，切回再点会二次提交。
      if (chatItemId) patchItemByIdFor(sid, chatItemId, { resolved: true, statusLabel: "已记住" });
      await loadMemoryOverview();
      notify();
    } catch (e) {
      if (sid === state.activeSessionId) addSystemItem(bt("memoryWriteFailed") + e);
    }
  }
  async function ignoreMemoryCandidate(memoryId, chatItemId) {
    if (!memoryId) return;
    const sid = state.activeSessionId; // 同 confirmMemoryCandidate：定向回发起会话(与 tauri 对齐，审计补充)
    try {
      const result = await invoke("ignore_pending_memory", { id: memoryId, sessionId: sid });
      if (sid === state.activeSessionId) {
        applyMemoryWriteState(result, function (next) {
          next.pending = (next.pending || []).filter(function (item) { return item.id !== memoryId; });
        });
      }
      if (chatItemId) patchItemByIdFor(sid, chatItemId, { resolved: true, statusLabel: "已忽略" });
      await loadMemoryOverview();
      notify();
    } catch (e) {
      if (sid === state.activeSessionId) addSystemItem(bt("memoryIgnoreFailed") + e);
    }
  }
  async function neverMemoryCandidate(memoryId, chatItemId) {
    if (!memoryId) return;
    const sid = state.activeSessionId; // 同 confirmMemoryCandidate：定向回发起会话(与 tauri 对齐，审计补充)
    try {
      const result = await invoke("never_pending_memory", { id: memoryId, reason: "user_selected", sessionId: sid });
      if (sid === state.activeSessionId) {
        applyMemoryWriteState(result, function (next) {
          next.pending = (next.pending || []).filter(function (item) { return item.id !== memoryId; });
        });
      }
      if (chatItemId) patchItemByIdFor(sid, chatItemId, { resolved: true, statusLabel: "不再提示" });
      await loadMemoryOverview();
      notify();
    } catch (e) {
      if (sid === state.activeSessionId) addSystemItem(bt("memoryNeverFailed") + e);
    }
  }
  // ── 思考指示器状态（每次阶段切换重置计时）──────────────────────
  function startThinking() { state.thinking = { active: true, phase: "thinking", toolName: "", startedAt: Date.now() }; }
  function thinkingTool(name) { state.thinking = { active: true, phase: "tool", toolName: name || "", startedAt: Date.now() }; }
  function thinkingIdle() { state.thinking = { active: true, phase: "thinking", toolName: "", startedAt: Date.now() }; }
  function stopThinking() { state.thinking = { active: false, phase: "thinking", toolName: "", startedAt: 0 }; }
  function applyModeFromState(st) {
    state.modeState = { mode: st.mode || "yolo", multiAgent: !!st.multi_agent };
  }

  // 权威 modeState 写回收敛点（评审 P1，与 tauri 端对齐）：任何「invoke 返回 /
  // 事件负载」带来的权威 modeState 更新都必须走这里——bump modeSyncSeq 作废
  // 在途 syncModeState 读取，再定向写回触发会话（await 期间用户可能已切走）。
  // 散点直写漏 bump 一处，就重现「旧读取覆盖权威值」竞态。
  function applyAuthoritativeModeState(sid, st) {
    modeSyncSeq += 1;
    runOnSession(sid, function () { applyModeFromState(st); });
  }

  function isActionablePlanCard(sid, itemId, planId) {
    if (!sid || sid !== state.activeSessionId || !itemId || !planId) return false;
    return state.chatItems.some(function (item) {
      return item && item.id === itemId && item.type === "plan_card" &&
        item.cardState === "active" && !item.resolved && String(item.planId || "") === planId;
    });
  }

  // ── Plan/YOLO 命令 ───────────────────────────────────────────────
  // sid 在 entry 捕获一次,thread through 所有 await —— 防用户切 session 后,
  // 后续 UI 写入/IPC 把卡片塞到错误的 session。
  async function acceptPlan(itemId, planMarkdown, echo, planId) {
    const sid = state.activeSessionId;
    if (!sid) return;
    const planTicket = String(planId || "").trim();
    if (!planTicket) {
      if (itemId) patchItemByIdFor(sid, itemId, { cardState: "frozen", statusLabel: bt("planHistorical"), resolved: true });
      addSystemItemFor(sid, bt("planTicketExpired"));
      notify();
      return;
    }
    const planBuffer = getBuffer(sid);
    if (planBuffer && planBuffer.remoteTurnActive && !(await reconcileRemoteTurn(sid))) {
      recordAuthoritySyncDiagnostic("remote_sync_blocked_action", Object.assign({
        operation: "accept_plan",
      }, authoritySyncBufferSnapshot(sid, planBuffer)));
      runOnSession(sid, function () { addAuthoritySyncNotice(bt("turnSyncRetry")); });
      notify();
      return;
    }
    // sid 前缀省略：isActionablePlanCard 首判已校验 sid === active（审计清理）。
    if (isBusyFor(sid) || !isActionablePlanCard(sid, itemId, planTicket)) return;
    if (planBuffer) {
      planBuffer.localTurnOwned = true;
      planBuffer.remoteTurnActive = false;
      planBuffer.remoteTerminalSeen = false;
      planBuffer.remoteCommittedRevision = "";
    }
    if (itemId) patchItemByIdFor(sid, itemId, { cardState: "approved", statusLabel: bt("approved"), resolved: true });
    let echoEntry = null;
    const displayEcho = echo || bt("echoGo");
    runOnSession(sid, function () {
      echoEntry = pushUserEcho(displayEcho, true);
      state.busy = true;
      startThinking();
    });
    notify();
    try {
      const st = await invoke("accept_plan", {
        sessionId: sid,
        planMarkdown: planMarkdown || "",
        displayMessage: displayEcho,
        planId: planTicket,
      });
      // 接受计划 = 后端受理新一轮（reserve_turn + 重跑）：未提交的「打开」转正锁死。
      try { window.dispatchEvent(new CustomEvent("pinvou:chat-round-committed", { detail: { scope: "plain" } })); } catch { /* silently ignored */ }
      if (planBuffer) planBuffer.deferredRemoteUserEvent = null;
      applyAuthoritativeModeState(sid, st);
    } catch (e) {
      const errorText = String(e && e.message ? e.message : e || "");
      const concurrentTurn = errorText.includes("session_turn_in_progress");
      const planNotActive = errorText.includes("plan_not_active");
      if (planBuffer) planBuffer.localTurnOwned = false;
      if (itemId) {
        patchItemByIdFor(sid, itemId, planNotActive
          ? { cardState: "frozen", statusLabel: bt("planHistorical"), resolved: true }
          : { cardState: "active", statusLabel: "", resolved: false });
      }
      runOnSession(sid, function () {
        if (echoEntry) {
          state.chatItems = state.chatItems.filter(function (item) { return item !== echoEntry.item; });
          state.messages = state.messages.filter(function (message) { return message !== echoEntry.message; });
        }
        state.busy = false;
        stopThinking();
      });
      const deferredApplied = applyDeferredRemoteUserMessage(sid, planBuffer);
      if (concurrentTurn && planBuffer && !deferredApplied) {
        markRemoteTurn(sid, planBuffer, false, "accept_plan_concurrent_turn");
      }
      try {
        const currentMode = await invoke("get_mode_state", { sessionId: sid });
        applyAuthoritativeModeState(sid, currentMode);
      } catch { /* status re-read failure must not mask the original error */ }
      addSystemItemFor(sid, bt("acceptPlanFailed") + e);
    }
    notify();
  }
  async function discardPlan(itemId, planId) {
    const sid = state.activeSessionId;
    const planTicket = String(planId || "").trim();
    if (!sid || !isActionablePlanCard(sid, itemId, planTicket)) return;
    patchItemByIdFor(sid, itemId, {
      cardState: "frozen", statusLabel: bt("planDiscarded"), resolved: true,
      planResolutionConfirmed: false,
    });
    // Reflect the user's decision immediately. The remote invoke may remain
    // pending until its transport timeout when the desktop disconnects.
    notify();
    try {
      const st = await invoke("discard_plan", { sessionId: sid, planId: planTicket });
      applyAuthoritativeModeState(sid, st);
    } catch (e) {
      const errorText = String(e && e.message ? e.message : e || "");
      const planNotActive = errorText.includes("plan_not_active");
      runOnSession(sid, function () {
        const card = state.chatItems.find(function (item) {
          return item && item.id === itemId && item.type === "plan_card" &&
            String(item.planId || "") === planTicket;
        });
        if (!card) return;
        if (planNotActive) {
          card.cardState = "frozen";
          card.resolved = true;
          card.statusLabel = bt("planHistorical");
        } else if (!card.planResolutionConfirmed) {
          card.cardState = "active";
          card.resolved = false;
          card.statusLabel = "";
        }
      });
      if (planNotActive) {
        try {
          const currentMode = await invoke("get_mode_state", { sessionId: sid });
          applyAuthoritativeModeState(sid, currentMode);
        } catch { /* status re-read failure must not mask the original error */ }
      }
      addSystemItemFor(sid, bt("discardPlanFailed") + e);
    }
    notify();
  }
  async function exitPlanToYolo() {
    const sid = state.activeSessionId;
    // 草稿态：不物化会话，改写本 lane 全局默认（三分 lane 语义）。
    if (!sid) { await setDraftMode("yolo"); return; }
    try {
      // invoke 形状保持 { sessionId: state.activeSessionId }（协议指纹按文本
      // 计算）；await 返回后按发起时 sid 定向写回并 bump modeSyncSeq。
      const st = await invoke("exit_plan_to_yolo", { sessionId: state.activeSessionId });
      applyAuthoritativeModeState(sid, st);
    } catch (e) { addSystemItemFor(sid, bt("exitPlanFailed") + e); }
    notify();
  }
  // 灯泡 toggle：plan ↔ yolo
  async function setPlanModeNext() {
    // 草稿态：不物化会话，改写本 lane 全局默认（三分 lane 语义；旧实现会先
    // ensureSession 物化——草稿页点 Plan 凭空造出空会话）。
    const sid = state.activeSessionId;
    if (!sid) { await setDraftMode("plan"); return; }
    try {
      const st = await invoke("set_plan_mode_next", { sessionId: sid });
      applyAuthoritativeModeState(sid, st);
    } catch (e) { addSystemItemFor(sid, bt("switchModeFailed") + e); }
    notify();
  }
  // plan-stuck / fallback / execution-stuck 卡片动作
  async function planStuckReplan(itemId) {
    patchItemById(itemId, { resolved: true, statusLabel: bt("replanRequested") }); notify();
    await sendMessage(bt("planStuckReplanPrompt"));
  }
  async function planStuckGo(itemId) {
    const sid = state.activeSessionId;
    if (!sid) return;
    patchItemById(itemId, { resolved: true }); notify();
    await exitPlanToYolo();
    // 补充指令必须发往触发会话：await exitPlanToYolo 期间用户可能已切走，
    // 直接 sendMessage 会把"继续执行"发到切换后的会话（审计遗漏补修）。
    // sendMessageToSession 校验失败（会话已删/对账中）会 throw，必须接住并
    // 定向提示，否则成为 React onClick 上的 unhandled rejection，用户无感知。
    try {
      await sendMessageToSession(sid, bt("planStuckGoPrompt"));
    } catch (e) { addSystemItemFor(sid, bt("planContinueFailed") + e); notify(); }
  }

  // ── 用户交互卡 ───────────────────────────────────────────────────
  // 卡片动作链路有 await 边界：entry 先捕获触发会话 sid，invoke 与后续全部 UI 写入
  // 都定向到 sid（runOnSession / patchItemByIdFor），避免用户提交期间切会话导致
  // echo/restoredAnswers 漏写触发会话或污染当前会话（与 acceptPlan 同一约定）。
  async function submitUserInput(itemId, toolCallId, answers, questions) {
    const sid = state.activeSessionId;
    if (!sid) return;
    patchItemByIdFor(sid, itemId, { submitting: true }); notify();
    try {
      await invoke("submit_user_input", { toolCallId, answers, sessionId: sid });
      // 摘要按 question 分组拼接：answers 是按选项展开的（multi_select 时同一题多条），
      // 不能按 answers 索引一一对应 questions（会越界抛 TypeError，复核 P1）。
      // 用无原型对象：question id 仅后端校验非空，constructor/toString/__proto__ 是合法输入，
      // 普通 {} 会让这些键命中 Object.prototype 继承属性，.push 抛 TypeError（复核 P1）。
      const byId = Object.create(null);
      answers.forEach(function (a) {
        if (a && a.id != null) {
          byId[a.id] = byId[a.id] || [];
          byId[a.id].push(a);
        }
      });
      const summary = questions.map(function (q, qi) {
        const list = byId[q.id];
        if (!list || !list.length) return null;
        const header = q.header || ("Q" + (qi + 1));
        return header + ": " + list.map(function (a) {
          const text = (a.other || a.label === "其他") ? bt("echoOtherPrefix") + a.value : a.label;
          return text;
        }).join(" · ");
      }).filter(Boolean).join(" · ");
      runOnSession(sid, function () {
        pushUserEcho("✓ " + summary, false);
        flushAssistantMessageToHistory();
      });
      // 提交时即存答案：切走视图再切回（ChatView 重挂载但 bridge state 保留）时，
      // QuestionChoiceCard 用 restoredAnswers 恢复选中态；会话级 rerender 另有解析。
      patchItemByIdFor(sid, itemId, { resolved: true, cardState: "submitted", submitting: false, restoredAnswers: answers });
    } catch (e) {
      patchItemByIdFor(sid, itemId, { submitting: false, error: String(e) });
    }
    notify();
  }
  async function cancelUserInput(itemId, toolCallId) {
    const sid = state.activeSessionId;
    if (!sid) return;
    try { await invoke("cancel_user_input", { toolCallId, sessionId: sid }); } catch { /* on cancel failure, wait for the backend timeout to reclaim it */ }
    patchItemByIdFor(sid, itemId, { resolved: true, cardState: "cancelled" });
    notify();
  }

  // ── 编辑上一轮 / 手动压缩 ─────────────────────────────────────────
  async function editLastTurn(newText) {
    if (state.busy || !state.activeSessionId) return;
    newText = (newText || "").trim();
    if (!newText) return;
    const sid = state.activeSessionId;
    const editBuffer = getBuffer(sid);
    if (editBuffer && editBuffer.remoteTurnActive && !(await reconcileRemoteTurn(sid))) {
      recordAuthoritySyncDiagnostic("remote_sync_blocked_action", Object.assign({
        operation: "edit_last_turn",
      }, authoritySyncBufferSnapshot(sid, editBuffer)));
      // 对账失败通知定向触发会话：await 期间切走后不得落进当前显示（与 acceptPlan 同）。
      runOnSession(sid, function () { addAuthoritySyncNotice(bt("turnSyncRetry")); });
      notify();
      return;
    }
    // Re-check after the asynchronous reconciliation: the user may have
    // switched Session or another turn may have started in the meantime.
    if (state.activeSessionId !== sid || state.busy) return;
    const previous = {
      messages: [...state.messages],
      chatItems: [...state.chatItems],
      busy: state.busy,
      thinking: Object.assign({}, state.thinking),
      currentStreamText,
      currentStreamId,
      pendingAssistantText,
      pendingAssistantBlocks: [...pendingAssistantBlocks],
    };
    if (editBuffer) {
      editBuffer.localTurnOwned = true;
      editBuffer.remoteTurnActive = false;
      editBuffer.remoteTerminalSeen = false;
      editBuffer.remoteCommittedRevision = "";
    }
    // Remove the latest displayable user turn and everything after it, then
    // append the replacement. Tool results and internal runtime envelopes also
    // use role="user", so a bare role scan would cut at the wrong boundary.
    let cut = -1;
    for (let i = state.messages.length - 1; i >= 0; i--) {
      const editCandidate = state.messages[i];
      if (editCandidate.role === "user" && userMessageDisplayText(editCandidate.content)) { cut = i; break; }
    }
    if (cut >= 0) state.messages.splice(cut);
    state.messages.push({ role: "user", content: [{ type: "text", text: newText }] });
    resetPendingAssistant();
    state.chatItems = [];
    rerenderFromMessages();
    state.busy = true;
    startThinking();
    currentStreamText = "";
    currentStreamId = ++itemIdSeq;
    state.chatItems.push({ id: currentStreamId, type: "assistant", text: "", html: "", time: timeStr(), streaming: true });
    if (editBuffer) saveWorkingSetTo(editBuffer);
    notify();
    turnUsageDirty[sid] = false; // 编辑重跑=新一轮，同 doSendFor 重置口径保护
    emitPetEvent("pet:turn_start", sid);
    try {
      await invoke("edit_last_turn", { newMessage: newText, sessionId: sid });
      // 编辑重跑 = 后端受理新一轮：未提交的「打开」转正锁死（同 doSendFor）。
      try { window.dispatchEvent(new CustomEvent("pinvou:chat-round-committed", { detail: { scope: "plain" } })); } catch { /* silently ignored */ }
      if (editBuffer) editBuffer.deferredRemoteUserEvent = null;
    } catch (e) {
      const errorText = String(e && e.message ? e.message : e || "");
      const concurrentTurn = errorText.includes("session_turn_in_progress");
      emitPetEvent("pet:turn_end", sid);
      if (editBuffer) editBuffer.localTurnOwned = false;
      runSyncOnSession(sid, function () {
        state.messages = previous.messages;
        state.chatItems = previous.chatItems;
        state.busy = previous.busy;
        state.thinking = previous.thinking;
        currentStreamText = previous.currentStreamText;
        currentStreamId = previous.currentStreamId;
        pendingAssistantText = previous.pendingAssistantText;
        pendingAssistantBlocks = previous.pendingAssistantBlocks;
      });
      if (state.activeSessionId === sid && editBuffer) saveWorkingSetTo(editBuffer);
      const deferredApplied = applyDeferredRemoteUserMessage(sid, editBuffer);
      if (concurrentTurn && editBuffer && !deferredApplied) {
        markRemoteTurn(sid, editBuffer, false, "edit_last_turn_concurrent_turn");
      }
      runSyncOnSession(sid, function () { addSystemItem("⚠️ " + e); });
      notify();
      flushQueued(sid);
    }
  }
  async function compactNow() {
    const sid = state.activeSessionId;
    if (!sid) return;
    try { await invoke("compact_now", { sessionId: state.activeSessionId }); } catch (e) {
      const compactErr = String(e || "");
      addSystemItemFor(sid, bt("compactFail") + ": " + (compactErr.includes("session_engine_not_running") ? bt("compactInactive") : compactErr));
    }
  }

  // ── 产物面板 ─────────────────────────────────────────────────────
  function invokeArtifact(nativeCommand, webCommand, path, sessionId, extra) {
    const args = Object.assign({ path }, extra || {});
    if (IS_WEB) {
      args.sessionId = sessionId || state.activeSessionId;
      return invoke(webCommand, args);
    }
    return invoke(nativeCommand, args);
  }
  function artifactInfo(path, sessionId) {
    return invokeArtifact("artifact_info", "web_access_artifact_info", path, sessionId);
  }
  function readArtifactText(path, sessionId) {
    return invokeArtifact("read_artifact_text", "web_access_read_artifact_text", path, sessionId);
  }
  function writeArtifactText(path, content, sessionId) {
    return invokeArtifact("write_artifact_text", "web_access_write_artifact_text", path, sessionId, { content });
  }
  function readArtifactImageB64(path, sessionId) {
    return invokeArtifact("read_artifact_image_b64", "web_access_read_artifact_image_b64", path, sessionId);
  }
  // pptx 封面缩略图：读 docProps/thumbnail.jpeg → data URL（无则 null）。本地数据、无外链。
  function readArtifactThumbnail(path, sessionId) {
    return invokeArtifact("read_artifact_thumbnail", "web_access_read_artifact_thumbnail", path, sessionId).catch(function () { return null; });
  }
  function renderArtifactVisual(path, sessionId) {
    return invokeArtifact("render_artifact_visual", "web_access_render_artifact_visual", path, sessionId);
  }
  function openContainingFolder(path) { return invoke("open_containing_folder", { path }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  function revealSessionFolder(sessionId) { return invoke("reveal_session_folder", { sessionId }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  function openScheduledTaskFolder(automationId) { return invoke("open_scheduled_task_folder", { automationId }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  function openInSystem(path) {
    if (IS_WEB) return downloadArtifact(path, null);
    return invoke("open_in_system", { path }).catch(function (e) { addSystemItem(bt("openFailed") + e); });
  }
  // 仅放白名单 URL (metaso.cn / open.bochaai.com),后端 open_external_url 强制校验。
  function openExternalUrl(url) {
    if (IS_WEB) {
      const opened = window.open(url, "_blank", "noopener,noreferrer");
      return Promise.resolve(!!opened);
    }
    return invoke("open_external_url", { url }).catch(function (e) { addSystemItem(bt("openFailed") + e); });
  }
  function openUserExternalUrl(url) {
    try {
      const rawUrl = String(url || "").trim();
      if (!/^https?:\/\/[^/\\\s]/i.test(rawUrl)) {
        return Promise.reject(new Error("only credential-free HTTP(S) links are supported"));
      }
      const parsed = new URL(rawUrl);
      if ((parsed.protocol !== "http:" && parsed.protocol !== "https:") || !parsed.hostname || parsed.username || parsed.password) {
        return Promise.reject(new Error("only credential-free HTTP(S) links are supported"));
      }
      if (IS_WEB) {
        const opened = window.open(parsed.href, "_blank", "noopener,noreferrer");
        return Promise.resolve(!!opened);
      }
      return invoke("open_user_external_url", { url: parsed.href }).catch(function (e) { addSystemItem(bt("openFailed") + e); });
    } catch {
      return Promise.reject(new Error("invalid external link"));
    }
  }
  function deliverableCategory(path) {
    const ext = (String(path || "").split(".").pop() || "").toLowerCase();
    if (["html", "htm", "mhtml", "mht"].includes(ext)) return "web";
    if (["ppt", "pptx", "odp", "dps"].includes(ext)) return "ppt";
    if (["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "heic"].includes(ext)) return "img";
    return "doc";
  }
  function sessionTitleById(sid) {
    const m = state.sessions.find(function (s) { return s.id === sid; });
    return (m && m.title) || "";
  }
  function currentMemoryArtifacts() {
    const rows = [];
    function addFrom(sid, arts) {
      (arts || []).forEach(function (a) {
        const path = a && a.path;
        if (!path || !isDeliverable(path)) return;
        rows.push({ path, sessionId: sid || state.activeSessionId, source: sessionTitleById(sid || state.activeSessionId), name: basename(path) });
      });
    }
    addFrom(state.activeSessionId, state.artifacts);
    Object.keys(sessionStates).forEach(function (sid) { addFrom(sid, sessionStates[sid] && sessionStates[sid].artifacts); });
    return rows;
  }
  // 跨会话产出物索引:磁盘 session JSON 为主,再合并当前内存工作集。
  // 新产物在 chat:done/save_session_artifacts 前也能立刻出现在「产出物」一级入口。
  async function listDeliverableIndex() {
    const disk = await invoke("list_deliverable_index").catch(function () { return []; });
    const byPath = {};
    (disk || []).forEach(function (x) { if (x && x.path) byPath[x.path] = x; });
    const mem = currentMemoryArtifacts().filter(function (x) { return x.path && !byPath[x.path]; });
    const hydrated = await Promise.all(mem.map(async function (x) {
      let path = x.path;
      if (!isAbsPath(path) && x.sessionId) {
        try {
          const ws = await invoke("list_workspace_files", { sessionId: x.sessionId });
          const bn = basename(path);
          const resolved = (ws || []).find(function (p) { return basename(p) === bn; });
          if (resolved) path = resolved;
        } catch { /* on parse failure, keep the original path */ }
      }
      let info = null;
      try { info = await artifactInfo(path, x.sessionId); } catch { /* missing info degrades to no-details */ }
      const ext = (String(path).split(".").pop() || "").toLowerCase();
      return {
        name: x.name || basename(path),
        path,
        ext,
        category: deliverableCategory(path),
        sessionId: x.sessionId || "",
        source: x.source || sessionTitleById(x.sessionId) || "",
        mtime: info && info.modified ? info.modified : 0,
        size: info && info.size ? info.size : 0,
      };
    }));
    hydrated.forEach(function (x) { if (x && x.path) byPath[x.path] = x; });
    return Object.keys(byPath).map(function (p) { return byPath[p]; }).sort(function (a, b) {
      return (b.mtime || 0) - (a.mtime || 0) || String(a.name || "").localeCompare(String(b.name || ""));
    });
  }
  // 外部打开产物：HTML 走 Tauri 独立窗口（绕沙箱），其他走系统应用。
  // sessionId = 卡片携带的产物所属 session。后端 resolve_artifact_path 用它(而非全局
  // active_id)解析相对路径 —— 切回「有 buffer」的会话后端 active 不更新,只有卡片自带
  // session 才解析得准(否则相对路径被拼到错的 workspace 报 not a file)。绝对路径无视它。
  function openArtifactExternal(path, sessionId) {
    if (IS_WEB) return downloadArtifact(path, sessionId);
    const ext = (String(path).split(".").pop() || "").toLowerCase();
    const cmd = (ext === "html" || ext === "htm") ? "open_artifact_window" : "open_in_system";
    return invoke(cmd, { path, sessionId: sessionId || null }).catch(function (e) { addSystemItem(bt("openFailed") + e); });
  }

  const MAX_WEB_ARTIFACT_DOWNLOAD_BYTES = 256 * 1024 * 1024;
  function webArtifactDownloadLimitError(size) {
    const suffix = Number.isSafeInteger(size) && size >= 0
      ? bt("downloadLimitSuffix")((size / (1024 * 1024)).toFixed(1))
      : "";
    return new Error(bt("downloadLimitError")(suffix));
  }

  async function downloadArtifact(path, sessionId) {
    if (IS_WEB && !hasCapability("artifactDownload")) {
      addSystemItem(bt("downloadUnsupported"));
      return false;
    }
    try {
      return await downloadArtifactRaw(path, sessionId);
    } catch (e) {
      addSystemItem(bt("downloadFailed") + String((e && e.message) || e));
      return false;
    }
  }

  async function downloadArtifactRaw(path, sessionId) {
    if (!IS_WEB || !hasCapability("artifactDownload")) {
      throw new Error(bt("downloadNotEnabled"));
    }
    const resolvedSessionId = sessionId || state.activeSessionId || null;
    const info = await artifactInfo(path, resolvedSessionId);
    if (!info || info.exists === false) throw new Error(bt("artifactMissing"));
    const expectedSize = Number(info.size);
    if (!Number.isSafeInteger(expectedSize) || expectedSize < 0) {
      throw new Error(bt("artifactSizeInvalid"));
    }
    if (expectedSize > MAX_WEB_ARTIFACT_DOWNLOAD_BYTES) {
      throw webArtifactDownloadLimitError(expectedSize);
    }
    let offset = 0;
    const chunks = [];
    let filename = basename(path) || "artifact";
    while (true) {
      const part = await invoke("web_access_read_artifact_chunk", {
        path,
        sessionId: resolvedSessionId,
        offset,
        limit: 262144,
      });
      if (!part) throw new Error("Artifact download returned no data");
      const partOffset = Number(part.offset);
      const partSize = Number(part.size);
      if (!Number.isSafeInteger(partOffset) || partOffset !== offset ||
          !Number.isSafeInteger(partSize) || partSize < 0) {
        throw new Error(bt("artifactChunkInvalid"));
      }
      if (partSize > MAX_WEB_ARTIFACT_DOWNLOAD_BYTES) {
        throw webArtifactDownloadLimitError(partSize);
      }
      if (partSize !== expectedSize) {
        throw new Error(bt("artifactChanged"));
      }
      filename = part.name || filename;
      const encoded = String(part.data_base64 || part.dataBase64 || "");
      const binary = atob(encoded);
      const bytes = new Uint8Array(binary.length);
// binary is a single-byte Latin-1 string produced by atob; charCode is the byte value. codePointAt is equivalent here but gains nothing.
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i); // eslint-disable-line unicorn/prefer-code-point
      if (!bytes.length && !part.eof) throw new Error(bt("artifactNoProgress"));
      if (bytes.length > MAX_WEB_ARTIFACT_DOWNLOAD_BYTES - offset) {
        throw webArtifactDownloadLimitError(offset + bytes.length);
      }
      if (offset + bytes.length > expectedSize) {
        throw new Error(bt("artifactOverflow"));
      }
      chunks.push(bytes);
      offset += bytes.length;
      if (part.eof) {
        if (offset !== expectedSize) throw new Error(bt("artifactIncomplete"));
        break;
      }
    }
    const blob = new Blob(chunks, { type: "application/octet-stream" });
    const objectUrl = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = objectUrl;
    anchor.download = filename;
    anchor.style.display = "none";
    document.body.append(anchor);
    anchor.click();
    anchor.remove();
    setTimeout(function () { URL.revokeObjectURL(objectUrl); }, 30000);
    return true;
  }

  // ── 附件 ────────────────────────────────────────────────────────
  function conversationAttachmentArgs(reference) {
    reference = reference || {};
    return {
      sessionId: reference.sessionId || state.activeSessionId,
      messageIndex: Number(reference.messageIndex),
      attachmentIndex: Number(reference.attachmentIndex),
      basename: String(reference.basename || ""),
      displayText: String(reference.displayText || ""),
    };
  }
  function resolveConversationAttachment(reference) {
    if (!IS_WEB) {
      return invoke("resolve_conversation_attachment", conversationAttachmentArgs(reference));
    }
    return Promise.reject(new Error(bt("attachPathUnavailable")));
  }
  async function downloadConversationAttachment(reference) {
    if (!hasCapability("artifactDownload")) {
      throw new Error(bt("attachDownloadUnsupported"));
    }
    const args = conversationAttachmentArgs(reference);
    let expectedSize = null;
    let offset = 0;
    const chunks = [];
    let filename = args.basename || "attachment";
    while (true) {
      const part = await invoke("web_access_read_conversation_attachment_chunk", Object.assign({
        offset,
        limit: 262144,
      }, args));
      if (!part) throw new Error(bt("attachNoData"));
      const partOffset = Number(part.offset);
      const partSize = Number(part.size);
      if (!Number.isSafeInteger(partOffset) || partOffset !== offset ||
          !Number.isSafeInteger(partSize) || partSize < 0) {
        throw new Error(bt("attachChunkInvalid"));
      }
      if (expectedSize === null) {
        expectedSize = partSize;
        if (expectedSize > MAX_WEB_ARTIFACT_DOWNLOAD_BYTES) {
          throw webArtifactDownloadLimitError(expectedSize);
        }
      } else if (partSize !== expectedSize) {
        throw new Error(bt("attachChanged"));
      }
      filename = part.name || filename;
      const encoded = String(part.data_base64 || part.dataBase64 || "");
      const binary = atob(encoded);
      const bytes = new Uint8Array(binary.length);
// binary is a single-byte Latin-1 string produced by atob; charCode is the byte value. codePointAt is equivalent here but gains nothing.
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i); // eslint-disable-line unicorn/prefer-code-point
      if (!bytes.length && !part.eof) throw new Error(bt("attachNoProgress"));
      if (offset + bytes.length > expectedSize) {
        throw new Error(bt("attachOverflow"));
      }
      chunks.push(bytes);
      offset += bytes.length;
      if (part.eof) {
        if (offset !== expectedSize) throw new Error(bt("attachIncomplete"));
        break;
      }
    }
    const blob = new Blob(chunks, { type: "application/octet-stream" });
    const objectUrl = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = objectUrl;
    anchor.download = filename;
    anchor.style.display = "none";
    document.body.append(anchor);
    anchor.click();
    anchor.remove();
    setTimeout(function () { URL.revokeObjectURL(objectUrl); }, 30000);
    return true;
  }
  async function openConversationAttachment(reference) {
    try {
      if (!IS_WEB) {
        await invoke("open_conversation_attachment", conversationAttachmentArgs(reference));
        return true;
      }
      return await downloadConversationAttachment(reference);
    } catch (e) {
      addSystemItem(bt("openFailed") + e);
      return false;
    }
  }
  async function revealConversationAttachment(reference) {
    if (IS_WEB) return false;
    try {
      await invoke("reveal_conversation_attachment", conversationAttachmentArgs(reference));
      return true;
    } catch (e) {
      addSystemItem(bt("openFailed") + e);
      return false;
    }
  }

  async function addAttachmentByPath(path) {
    const id = ++attachIdSeq;
    const att = { id, basename: basename(path), status: "parsing", result: null, error: null };
    state.attachments.push(att); notify();
    try {
      const result = await invoke(IS_WEB ? "web_access_ingest_file" : "ingest_file", { path });
      att.status = "ready"; att.result = result;
    } catch (e) { att.status = "error"; att.error = String(e); }
    notify();
  }

  function attachmentLimitDisplayError(error, fileName) {
    const raw = String(error && error.message ? error.message : error);
    if ((error && error.code === "device_upload_too_large") || raw === "attachment_file_too_large") {
      return { code: "attachment_file_too_large", message: bt("deviceUploadTooLarge")(fileName) };
    }
    if (raw === "attachment_archive_too_many_entries") {
      return { code: raw, message: bt("archiveTooManyEntries") };
    }
    if (raw === "attachment_archive_expanded_too_large") {
      return { code: raw, message: bt("archiveExpandedTooLarge") };
    }
    if (raw === "attachment_archive_unsafe_entry") {
      return { code: raw, message: bt("archiveUnsafeEntry") };
    }
    return null;
  }

  function deviceUploadDisplayError(error, fileName) {
    const limitError = attachmentLimitDisplayError(error, fileName);
    if (limitError) return limitError;
    const rawUploadError = String(error && error.message ? error.message : error);
    if (error && error.code === "device_upload_empty") {
      const message = bt("deviceUploadEmpty")(fileName);
      return { code: message, message };
    }
    if (rawUploadError === "web_attachment_digest_invalid") {
      const message = bt("deviceUploadDigestInvalid");
      return { code: message, message };
    }
    if (rawUploadError === "web_attachment_integrity_mismatch") {
      const message = bt("deviceUploadIntegrityMismatch");
      return { code: message, message };
    }
    return { code: rawUploadError, message: rawUploadError };
  }
  async function addPasteImage(filename, bytes) {
    try {
      const path = await invoke("save_paste_image", { filename, bytes });
      await addAttachmentByPath(path);
    } catch (e) {
      const limitError = attachmentLimitDisplayError(e, filename);
      addSystemItem(limitError ? limitError.message : bt("pasteImageFailed") + e);
    }
  }
  function releaseAttachmentOnDesktop(attachment) {
    // Accepts a composer attachment ({ result, uploadId }) or a bare
    // WebAttachmentSummary ({ handle }) carried by a queued message.
    const handle = attachment
      && (attachment.result ? attachment.result.handle : attachment.handle);
    if (handle && canInvoke("web_access_discard_attachment")) {
      invoke("web_access_discard_attachment", { handle }).catch(function () {});
      return;
    }
    if (attachment && attachment.uploadId && canInvoke("web_access_abort_attachment_upload")) {
      invoke("web_access_abort_attachment_upload", { uploadId: attachment.uploadId }).catch(function () {});
    }
  }
  function removeAttachment(id) {
    const removed = state.attachments.find(function (attachment) { return attachment.id === id; });
    state.attachments = state.attachments.filter(function (a) { return a.id !== id; });
    releaseAttachmentOnDesktop(removed);
    notify();
  }
  function clearAttachments() {
    const removed = [...state.attachments];
    state.attachments = [];
    removed.forEach(releaseAttachmentOnDesktop);
  }
  // 打开系统文件选择器并摄入为附件
  async function pickAndAttach() {
    if (!dialogOpen) { addSystemItem(bt("filePickUnavailable")); return; }
    try {
      const selected = await dialogOpen({ multiple: true });
      if (!selected) return;
      const paths = Array.isArray(selected) ? selected : [selected];
      for (let i = 0; i < paths.length; i++) { await addAttachmentByPath(paths[i]); }
    } catch (e) { addSystemItem(bt("filePickFailed") + e); }
  }

  // ── 浏览器本机文件上传 ──────────────────────────────────────────
  // 「从此设备上传」入口:文件按 256KB 分块经 Relay 转发(Relay 只转发不保存),
  // 桌面端最后一块落盘 + ingest 后返回与桌面文件附件相同的 WebAttachmentSummary。
  async function uploadDeviceFile(file) {
    const uploader = window.PinvouChunkedFileUpload;
    const id = ++attachIdSeq;
    const uploadId = uploader && typeof uploader.uploadId === "function"
      ? uploader.uploadId("webatt")
      // eslint-disable-next-line sonarjs/pseudo-random -- not security-sensitive: temporary attachment upload ID; the sequence prefix already guarantees uniqueness
      : "webatt_" + id + "_" + Math.random().toString(36).slice(2, 12);
    const att = {
      id, uploadId, basename: file.name,
      status: "uploading", progress: 0, result: null, error: null,
    };
    state.attachments.push(att); notify();
    try {
      if (!uploader || typeof uploader.uploadFile !== "function") {
        throw new Error("chunked attachment uploader is unavailable");
      }
      const completed = await uploader.uploadFile({
        file,
        uploadId,
        isCancelled: function () { return !state.attachments.includes(att); },
        sendChunk: function (chunk) {
          return invoke("web_access_upload_attachment_chunk", {
            uploadId: chunk.uploadId,
            fileName: chunk.fileName,
            offset: chunk.offset,
            total: chunk.total,
            dataBase64: chunk.dataBase64,
            commit: chunk.commit,
            ...(chunk.sha256 ? { sha256: chunk.sha256 } : {}),
          });
        },
        onProgress: function (progress) { att.progress = progress; notify(); },
        validateResult: function (result) { return Boolean(result && result.handle); },
        cleanup: function (upload) {
          if (upload.result && upload.result.handle && canInvoke("web_access_discard_attachment")) {
            return invoke("web_access_discard_attachment", { handle: upload.result.handle });
          }
          return invoke("web_access_abort_attachment_upload", { uploadId: upload.uploadId });
        },
      });
      const summary = completed.result;
      att.status = "ready"; att.progress = 100; att.result = summary;
      att.basename = summary.basename || att.basename;
    } catch (e) {
      if (!e || e.code !== "device_upload_cancelled") {
        att.status = "error";
        // Desktop preflight and integrity failures come back as stable wire
        // codes. Keep limit codes on the chip so React can translate them when
        // the active language changes; system notices are translated here.
        const displayError = deviceUploadDisplayError(e, file.name);
        att.error = displayError.code;
        addSystemItem(bt("deviceUploadFailed") + displayError.message);
      }
    }
    notify();
  }

  // 顺序处理选中的多个文件;桌面端另有并发缓冲与总量上限兜底。
  async function uploadDeviceFiles(files) {
    const list = Array.prototype.slice.call(files || []).filter(Boolean);
    for (let i = 0; i < list.length; i++) await uploadDeviceFile(list[i]);
  }


  // ── 卡片池: 专家面具加持 ─────────────────────────────────────────
  // 懒加载全部专家卡(1078 张),前端缓存供 facet/搜索。只拉一次。
  async function loadPersonas() {
    if (state.personaPool.loadState === "ready" || state.personaPool.loadState === "loading") return;
    await refreshPersonas();
  }
  // 强制重拉卡牌列表(自创卡增删改后调,让池子立即反映)。
  async function refreshPersonas() {
    state.personaPool.loadState = "loading"; notify();
    try {
      personaPoolCache = await invoke("list_personas");
      state.personaPool.loadState = "ready";
    } catch (e) {
      personaPoolCache = []; state.personaPool.loadState = "error";
      console.warn("list_personas failed", e);
    }
    notify();
  }
  // ── 用户自创卡 CRUD(写盘后刷新缓存) ──
  async function createPersona(input) {
    const sum = await invoke("create_persona", { input });
    await refreshPersonas();
    return sum;
  }
  async function updatePersona(personaId, input) {
    const sum = await invoke("update_persona", { personaId, input });
    await refreshPersonas();
    // 若改的正是当前 session 加持的卡, 同步挂件显示
    if (state.activePersona && state.activePersona.id === personaId) { state.activePersona = sum; notify(); }
    return sum;
  }
  async function deletePersona(personaId) {
    await invoke("delete_persona", { personaId });
    await refreshPersonas();
  }
  // 给当前 session 加持一张专家面具。后端存 persona_id + 每 turn 注入人设;
  // 前端记 activePersona(挂件) + 发一条系统消息播报。
  // 取专家显示名(兼容 Side A 的 cn_name / Side B 的 name)。
  function personaName(p) {
    if (!p) return "";
    // 内置卡名按 UI 语言显示(personas-i18n.js overlay),中文兜底;自制卡不翻
    const lang = state.settings && state.settings.language;
    const L = lang === "en" ? "en" : lang === "ja" ? "ja" : null;
    const tr = L && p.source !== "user" && window.PERSONA_I18N && window.PERSONA_I18N[p.id] && window.PERSONA_I18N[p.id][L];
    if (tr && tr.name) return tr.name;
    return (p.name || p.cn_name) || "";
  }
  // 记一条卡牌事件到时间线 sidecar(pos=当前 messages 数),并落盘。重载历史时按 pos 插回。
  function recordPersonaEvent(ev) {
    if (!state.activeSessionId) return;
    ev.pos = state.messages.length;
    state.personaEvents.push(ev);
    const sid = state.activeSessionId;
    const snapshot = JSON.parse(JSON.stringify(state.personaEvents));
    invoke("save_session_persona_events", { sessionId: sid, events: snapshot }).catch(function () {});
  }
  async function equipPersona(personaId) {
    if (!state.activeSessionId) {
      // 草稿态加卡 → 先物化 session(lazy session)。用返回值判空：切走场景
      // ensureSession 返回 null 但 activeSessionId 非空，会把卡加进新会话。
      const materialized = await ensureSession();
      if (!materialized) return; // 物化失败/切走,放弃
    }
    // 入口捕获触发会话：await 期间用户可能切走，UI 写入不得落进别的会话
    // （错误会话被重命名/插卡是持久化污染，不可自愈）。后端已按发起会话
    // 定向；切走后放弃前端播报，挂件靠 syncActivePersona 恢复（与 tauri
    // personas.js 对齐，审计）。
    const sid = state.activeSessionId;
    try {
      const card = await invoke("equip_persona", { sessionId: state.activeSessionId, personaId });
      lastEquippedSid = sid; // 成功加持的目标会话(即使已切走)：供紧随其后的引导卡定向(与 tauri 对齐，审计补充)
      if (sid !== state.activeSessionId) return card; // 已切走：不写当前显示
      // 标题仍是默认占位(三语哨兵,见 isDefaultChatTitle)→ 用卡牌名命名(无论草稿态物化还是遗留空会话;
      // 用户已主动改名 / 已被首条消息命名的会话不动)。决策:卡牌优先于首条消息。
      const m = state.sessions.find(function (s) { return s.id === sid; });
      // 标题还是默认值 / 仍是卡牌占位(换卡场景)→ 用(新)卡牌名命名,并标记为占位。
      // 占位名会被首条用户消息覆盖(见 persistMessages*),让同卡会话靠对话内容区分。
      if (m && (isDefaultChatTitle(m.title) || personaPlaceholderTitles[sid])) {
        const newTitle = personaName(card);
        if (newTitle) {
          try { await invoke("rename_session", { id: sid, title: newTitle }); } catch { /* rename failure does not affect local display */ }
          if (sid !== state.activeSessionId) return card; // rename 挂起期间切走:放弃后续 UI 写入(审计补充)
          m.title = newTitle;
          personaPlaceholderTitles[sid] = true;
        }
      }
      // 同 session 换了一张不同的卡 → 先弹一条"已卸下旧专家",再弹新加持。
      // 旧专家在写点复核而非入口捕获：同会话快速连续换卡时,入口值可能已被
      // 上一次 equip 的权威写覆盖,陈旧值会播报错误的"已卸下"(与 tauri 对齐,二审补充)。
      const prev = state.activePersona;
      if (prev && prev.id !== card.id) {
        addChatItem({ type: "system", text: bt("personaUnequipped") + personaName(prev), time: timeStr() });
        recordPersonaEvent({ kind: "unequip", name: personaName(prev) });
      }
      personaSyncSeq++; // 权威写前 bump：作废在途 syncActivePersona 的旧快照(与 tauri 对齐，审计补充)
      state.activePersona = card;
      addChatItem({ type: "persona_equip", card, time: timeStr() });
      recordPersonaEvent({ kind: "equip", card });
      notify();
      return card;
    } catch (e) {
      // 失败也守住归属：失败气泡只落发起会话,不得插进 await 窗口内切到的
      // 会话(addSystemItem 随消息流持久化);同时作废 lastEquippedSid——失败
      // equip 后紧随的引导卡不得回退定向到历史成功 equip 的无关会话(与 tauri 对齐,二审补充)。
      lastEquippedSid = null;
      if (sid === state.activeSessionId) addSystemItem(bt("equipFailed") + e);
      return null;
    }
  }
  // 摘下当前 session 的专家面具。
  async function unequipPersona() {
    if (!state.activeSessionId) return;
    // 入口捕获触发会话：await 期间切走，卸下播报不得写进别的会话（与 tauri 对齐，审计）。
    const sid = state.activeSessionId;
    try { await invoke("unequip_persona", { sessionId: state.activeSessionId }); } catch { /* 忽略,前端照样摘 */ }
    if (sid !== state.activeSessionId) return; // 已切走：不写当前显示
    personaSyncSeq++; // 权威写前 bump：作废在途 syncActivePersona 的旧快照(与 tauri 对齐，审计补充)
    // 旧专家同样在写点复核：await 窗口内若已被 equip 换成新卡,播报新卡,
    // 入口捕获的陈旧值会重复播报早已卸下的旧卡(与 tauri 对齐,二审补充)。
    const prev = state.activePersona;
    state.activePersona = null;
    if (prev) { addChatItem({ type: "system", text: bt("personaUnequipped") + personaName(prev), time: timeStr() }); recordPersonaEvent({ kind: "unequip", name: personaName(prev) }); }
    notify();
  }
  // 挂件还原的请求序号 + 最近成功加持的目标会话(与 tauri personas.js 对齐)：
  // - syncActivePersona 仅 sid 校验挡不住 A→B→A 的 ABA 与同会话乱序(慢响应
  // 返回 null 会把刚加持的挂件覆盖掉,且无人再纠正)。序号在每次 sync 发起
  // 与 equip/unequip 权威写时递增,旧快照一律作废(审计补充)。
  // - lastEquippedSid 供 equip 后紧随的播报(如卡牌制造者引导卡)定向回
  //   发起会话——equip 的 await 窗口用户可能已切走。
  let personaSyncSeq = 0;
  let lastEquippedSid = null;
  // 切换/重载 session 后,从后端拉该 session 的加持状态还原挂件(backend 是真相)。
  async function syncActivePersona() {
    if (!state.activeSessionId) { state.activePersona = null; return; }
    const sid = state.activeSessionId;
    const seq = ++personaSyncSeq;
    try {
      const persona = await invoke("get_active_persona", { sessionId: state.activeSessionId }) || null;
      if (sid !== state.activeSessionId || seq !== personaSyncSeq) return; // 已切走或被权威写/新 sync 作废
      state.activePersona = persona;
    } catch { /* 旧 session 无加持,忽略 */ }
  }
  // 在【指定 session】追加卡牌制造者引导卡并落 sidecar(持久化,重载按 pos 插回)。
  // 默认定向最近一次成功 equip 的目标会话：AI 造卡链路 equip→intro 之间用户
  // 切走时,intro 必须仍落在发起(已加持)会话,而不是写进切走后的当前显示
  // (错误会话被插卡是持久化污染,不可自愈)。显式传 sid 可覆盖(与 tauri 对齐，
  // 审计补充)。
  function postCardCreatorIntro(sid) {
    const target = sid || lastEquippedSid || state.activeSessionId;
    if (!target) return;
    runOnSession(target, function () {
      addChatItem({ type: "card_creator_intro", time: "" });
      recordPersonaEvent({ kind: "card_creator_intro" });
      notify();
    });
  }

  // ── 多知识库挂载(会话级粘连,仿 persona) ──
  function normalizeMountedCollections(value) {
    if (!Array.isArray(value)) return [];
    const seen = Object.create(null);
    return value.map(function (entry) {
      if (entry == null) return null;
      const collectionId = typeof entry === "object"
        ? (entry.collectionId == null ? entry.collection_id : entry.collectionId)
        : entry;
      if (collectionId == null || seen[String(collectionId)]) return null;
      seen[String(collectionId)] = true;
      return { collectionId, enabled: typeof entry === "object" ? entry.enabled !== false : true };
    }).filter(Boolean);
  }
  function applyMountedCollections(value) {
    const hasSnapshot = value && !Array.isArray(value) && Array.isArray(value.collections);
    const revision = hasSnapshot ? Number(value.revision || 0) : Number(state.mountedCollectionsRevision || 0);
    if (hasSnapshot && revision < Number(state.mountedCollectionsRevision || 0)) {
      return normalizeMountedCollections(state.mountedCollections);
    }
    const normalized = normalizeMountedCollections(hasSnapshot ? value.collections : value);
    state.mountedCollections = normalized;
    state.mountedCollectionsRevision = revision;
    const firstEnabled = normalized.find(function (entry) { return entry.enabled; });
    state.mountedCollection = firstEnabled ? firstEnabled.collectionId : null;
    return normalized;
  }
  let mountedCollectionUpdate = Promise.resolve();
  let mountedCollectionDraftTarget = null;
  function mountedCollectionTargetAtEnqueue() {
    if (state.activeSessionId) return { draft: false, promise: Promise.resolve(state.activeSessionId) };
    const draftEpoch = Number(state.draftEpoch || 0);
    if (!mountedCollectionDraftTarget || mountedCollectionDraftTarget.epoch !== draftEpoch || mountedCollectionDraftTarget.failed) {
      const target = { draft: true, epoch: draftEpoch, failed: false, pending: 0, promise: null };
      target.promise = Promise.resolve().then(async function () {
        // Navigation before draft materialization cancels this batch instead of
        // silently retargeting it to the newly active session.
        if (state.activeSessionId) return null;
        const sessionId = await ensureSession();
        if (!sessionId) target.failed = true;
        return sessionId;
      });
      mountedCollectionDraftTarget = target;
    }
    mountedCollectionDraftTarget.pending += 1;
    return mountedCollectionDraftTarget;
  }
  function updateMountedCollections(command, args) {
    const requestedTarget = mountedCollectionTargetAtEnqueue();
    mountedCollectionUpdate = mountedCollectionUpdate.catch(function () {}).then(async function () {
      // The target is captured at click time. Rapid draft actions share one
      // materialization promise and remain bound to that session after navigation.
      const sessionId = await requestedTarget.promise;
      if (!sessionId) return null;
      try {
        const saved = await invoke(command, Object.assign({ sessionId }, args || {}));
        const normalized = normalizeMountedCollections(saved && saved.collections);
        if (state.activeSessionId === sessionId) {
          applyMountedCollections(saved);
          notify();
        }
        return normalized;
      } catch (e) {
        addSystemItem(bt("mountCollectionFailed") + e);
        return null;
      }
    });
    if (requestedTarget.draft) {
      mountedCollectionUpdate = mountedCollectionUpdate.finally(function () {
        requestedTarget.pending -= 1;
        if (requestedTarget.pending === 0 && mountedCollectionDraftTarget === requestedTarget) {
          mountedCollectionDraftTarget = null;
        }
      });
    }
    return mountedCollectionUpdate;
  }
  // 添加知识集；已挂载但停用时重新启用，不覆盖其他挂载项。
  async function mountCollection(collectionId) {
    if (collectionId == null) return null;
    const saved = await updateMountedCollections("session_add_mounted_collection", { collectionId });
    return saved ? collectionId : null;
  }
  async function setCollectionEnabled(collectionId, enabled) {
    return updateMountedCollections("session_set_mounted_collection_enabled", {
      collectionId,
      enabled: !!enabled,
    });
  }
  async function removeCollection(collectionId) {
    return updateMountedCollections("session_remove_mounted_collection", { collectionId });
  }
  // 兼容旧入口：摘下当前对话的全部知识集挂载。
  async function unmountCollection() {
    if (!state.activeSessionId) { applyMountedCollections([]); notify(); return; }
    return updateMountedCollections("session_unmount_collection", null);
  }
  // 切换/重载 session 后从后端还原挂载状态(backend 是真相;仅驻内存,重启后为 null)。
  async function syncMountedCollection() {
    if (!state.activeSessionId) { applyMountedCollections([]); return; }
    const sessionId = state.activeSessionId;
    try {
      const snapshot = await invoke("session_mounted_collections_snapshot", { sessionId });
      if (state.activeSessionId !== sessionId) return;
      if (snapshot && Array.isArray(snapshot.collections)) { applyMountedCollections(snapshot); return; }
      const mounted = await invoke("session_mounted_collections", { sessionId });
      if (state.activeSessionId !== sessionId) return;
      if (Array.isArray(mounted)) { applyMountedCollections(mounted); return; }
      const legacy = await invoke("session_mounted_collection", { sessionId });
      if (state.activeSessionId !== sessionId) return;
      applyMountedCollections(legacy == null ? [] : [legacy]);
    } catch {
      try {
        const cid = await invoke("session_mounted_collection", { sessionId });
        if (state.activeSessionId !== sessionId) return;
        applyMountedCollections(cid == null ? [] : [cid]);
      } catch { if (state.activeSessionId === sessionId) applyMountedCollections([]); }
    }
  }

  // ── 应用内升级 ───────────────────────────────────────────────────
  // 链路: check_for_update(对比服务器 latest.json) → download_update(流式下载+sha256,
  // 进度走 update:progress 事件) → install_update(pkexec apt) → restart_app。
  listen("update:progress", function (e) {
    const p = e.payload || {};
    state.updateProgress = p.total ? Math.round((p.downloaded / p.total) * 100) : 0;
    notify();
  });
  listen("web_access:status", function (e) {
    state.webAccess = Object.assign({}, state.webAccess, e.payload || {});
    notify();
  });
  async function loadAppVersion() {
    try {
      state.appVersion = await invoke("get_app_version");
    } catch { /* version read failure: leaving it empty is fine */ }
  }
  // 启动静默检查: 失败全吞(网络差/更新源挂了不打扰用户)。结果不管新旧都存——
  // available 驱动红点,current_version 给设置页显示当前版本用。
  async function checkForUpdateSilently() {
    try {
      const info = await invoke("check_for_update");
      if (info && info.current_version) state.appVersion = info.current_version;
      if (info) { state.updateInfo = info; notify(); }
    } catch { /* 静默 */ }
  }
  // 设置页手动检查: 错误和「已是最新」都要反馈。
  async function checkForUpdate() {
    state.updateChecking = true; state.updateCheckError = null; notify();
    try {
      const info = await invoke("check_for_update");
      if (info && info.current_version) state.appVersion = info.current_version;
      state.updateInfo = info;
      if (!info.available) state.updateCheckError = "latest"; // 前端按 i18n 显示「已是最新」
    } catch (e) {
      state.updateCheckError = String(e);
    }
    state.updateChecking = false; notify();
  }
  // 下载+安装一条龙: Linux 下载 deb 后 pkexec apt 并自动重启;macOS 下载 dmg 后
  // hdiutil attach + cp -R 并自动重启(与 Linux 同型);Windows 下载 zip 后解析 MSI,
  // 安装器启动成功后后端退出当前进程。返回 true 表示安装链路已成功走完。
  async function downloadAndInstallUpdate() {
    if (!state.updateInfo || !state.updateInfo.available || state.updateDownloading) return false;
    // macOS 与 Linux 一样安装后自动重启:app.restart() 按路径 exec,
    // bundle 被替换后该路径已指向新文件,spawn 新进程即加载新版(inode 语义与 Linux 同)。
    // Ok(false) 表示「安装完成,进程未退出,由前端决定 restart」,不是「需手动重启」。
    // 唯一不自动重启的是 Windows(MSI 安装器接管,后端 Ok(true)→app.exit)。
    const shouldRestartAfterInstall =
      state.updateInfo.platform === "linux" || state.updateInfo.platform === "macos";
    let installed = false;
    state.updateDownloading = true; state.updateCancelling = false;
    state.updateProgress = 0; state.updateError = null; notify();
    try {
      const downloadResult = await invoke("download_update", { info: state.updateInfo });
      state.updateProgress = 100; notify();
      if (downloadResult && typeof downloadResult === "object" && downloadResult.installer_path) {
        await invoke("install_update", { installerPath: downloadResult.installer_path, info: state.updateInfo });
      } else {
        // Linux/macOS:download_update 返回纯路径字符串(JSON untagged),走 debPath 分支。
        // 传 info 让 macOS 后端做安装前 sha256 复验(TOCTOU 纵深防御);Linux 后端目前忽略此参数。
        await invoke("install_update", { debPath: downloadResult, info: state.updateInfo });
      }
      state.updateReady = true;
      installed = true;
    } catch (e) {
      // 用户主动取消下载时后端返回「已取消下载」,当正常处理不弹错误
      if (state.updateCancelling) state.updateProgress = 0;
      else state.updateError = String(e);
    }
    state.updateDownloading = false; state.updateCancelling = false; notify();
    if (installed && shouldRestartAfterInstall) restartApp();
    return installed;
  }
  // 取消进行中的下载: 置前端标志 + 通知后端中断下载循环。仅下载阶段有效;
  // 已进入 install(pkexec/apt)则无效(系统接管,装一半不能停)。
  function cancelUpdate() {
    if (!state.updateDownloading || state.updateCancelling) return;
    state.updateCancelling = true; notify();
    invoke("cancel_download").catch(function () { /* 忽略,下载循环超时也会退 */ });
  }
  function restartApp() {
    invoke("restart_app").catch(function () { /* restart 成功不会返回 */ });
  }
  function reportPendingUpdateResult() {
    invoke("report_pending_update_result").catch(function () { /* 静默重试,不阻塞启动 */ });
  }

  // ── Persistent instance-scoped Web access ──────────────────────
  async function refreshWebAccessStatus() {
    try {
      const status = await invoke("web_access_status");
      state.webAccess = Object.assign({}, state.webAccess, status || {});
    } catch (e) {
      state.webAccess = Object.assign({}, state.webAccess, { last_error: String(e) });
    }
    notify();
  }
  async function enableWebAccess() {
    state.webAccess = Object.assign({}, state.webAccess, { starting: true, last_error: null });
    notify();
    try {
      const info = await invoke("web_access_enable");
      state.webAccess = Object.assign({}, state.webAccess, info || {}, { active: true, starting: false, last_error: null });
      await refreshWebAccessStatus();
      return info;
    } catch (e) {
      state.webAccess = Object.assign({}, state.webAccess, { active: false, starting: false, status: "error", last_error: String(e) });
      notify();
      throw e;
    }
  }
  async function disableWebAccess() {
    try {
      await invoke("web_access_disable");
    } catch (e) {
      state.webAccess = Object.assign({}, state.webAccess, { status: "error", last_error: String(e) });
      notify();
      throw e;
    }
    state.webAccess = Object.assign({}, state.webAccess, { active: false, endpoint_id: null, url: null, qr_data_url: null, status: "stopped" });
    notify();
  }
  async function rotateWebAccessLink() {
    try {
      const info = await invoke("web_access_rotate");
      state.webAccess = Object.assign({}, state.webAccess, info || {}, { active: true, last_error: null });
      await refreshWebAccessStatus();
      return info;
    } catch (e) {
      state.webAccess = Object.assign({}, state.webAccess, { status: "error", last_error: String(e) });
      notify();
      throw e;
    }
  }
  // 自定义 Relay 服务器：查询/保存/恢复默认。保存与恢复在已启用时会触发后端
  // refresh（旧链接失效、新链接换服务器），所以随后同步一次 webAccess 状态。
  async function getWebRelaySettings() {
    return invoke("web_access_relay_settings");
  }
  async function setWebRelayAddress(address) {
    const info = await invoke("web_access_set_relay", { address });
    await refreshWebAccessStatus();
    return info;
  }
  async function resetWebRelayAddress() {
    const info = await invoke("web_access_reset_relay");
    await refreshWebAccessStatus();
    return info;
  }

  // ── 依赖体检 ─────────────────────────────────────────────────────
  // 实时检测各文件解析能力(PDF/Office/OCR/压缩包/邮件)的系统依赖是否齐全,
  // 设置页展示缺失项 + 一键 apt 命令。后端 check_dependencies 不走缓存,装完可复检。
  async function checkDependencies() {
    if (state.depsChecking) return;
    state.depsChecking = true; state.depsInstallError = null; notify();
    try {
      state.deps = await invoke("check_dependencies");
    } catch { state.deps = []; }
    state.depsChecking = false; notify();
  }
  // 一键安装缺失依赖: 收集缺失项的包名 → 后端 pkexec apt 提权安装 → 装完实时重检。
  async function installDependencies() {
    const deps = state.deps || [];
    const missing = deps.filter(function (d) { return !d.installed; });
    if (!missing.length || state.depsInstalling) return;
    const pkgs = [];
    const actions = [];
    missing.forEach(function (d) {
      const action = String(d.install_action || "").trim();
      if (/^[a-z0-9_]+$/i.test(action) && !actions.includes(action)) {
        actions.push(action);
      }
      const parts = String(d.apt).trim().split(/\s+/).filter(Boolean);
      if (!parts.length || parts.some(function (p) { return !/^[a-z0-9][a-z0-9+.-]*$/i.test(p); })) {
        return;
      }
      parts.forEach(function (p) {
        if (!pkgs.includes(p)) pkgs.push(p);
      });
    });
    if (!pkgs.length && !actions.length) {
      state.depsInstallError = bt("depsNotInstallable");
      notify();
      return;
    }
    state.depsInstalling = true; state.depsInstallError = null; notify();
    try {
      await invoke("install_dependencies", { packages: pkgs, actions });
    } catch (e) {
      state.depsInstallError = String(e);
    }
    try {
      state.deps = await invoke("check_dependencies"); // 成功或部分成功后均实时反映当前状态
    } catch { /* keep the last successful dependency snapshot */ }
    state.depsInstalling = false; notify();
  }

  // ── 语音输入（WebView one-shot 录音 → 本地 SenseVoice/FunASR ASR；Linux webview 录音授权见 lib.rs setup）──────────────
  let activeVoiceInput = null;

  function setVoiceInputStatus(status, patch) {
    const next = Object.assign({}, state.voiceInput, patch || {});
    next.status = status;
    if (status !== "failed") {
      next.error = null;
      next.category = null;
    }
    state.voiceInput = next;
    notify();
  }

  function emitVoiceDiagnostic(stage, level, message, userMessage, category) {
    const event = {
      stage,
      level,
      message,
      user_message: userMessage || "",
      category: category || "",
    };
    const fn = level === "error" ? console.error : level === "warn" ? console.warn : console.info;
    fn.call(console, "[voice-input]", event);
  }

  // error carrier for the voice flow: an Error instance plus category/stage extra fields, classified by normalizeVoiceError.
  function voiceFlowError(category, stage, message) {
    const error = new Error(message);
    error.category = category;
    error.stage = stage;
    return error;
  }
  // 与 tauri 车道同款:Rust 远控语音命令的稳定错误码 → 三语文案 key,错误码
  // 优先于 rawMessage(中文工程原文只进诊断)。
  const VOICE_ERROR_CODE_KEYS = {
    asr_timeout: "voiceTimeout",
    asr_no_speech: "voiceEmptyResult",
    asr_engine_missing: "voiceRecognitionFailed",
    asr_engine_start_failed: "voiceRecognitionFailed",
    asr_engine_error: "voiceRecognitionFailed",
    asr_runtime_error: "voiceRecognitionFailed",
    asr_cli_failed: "voiceRecognitionFailed",
    asr_parse_failed: "voiceRecognitionFailed",
    asr_join_failed: "voiceInputFailed",
    recording_too_long: "voiceRecordingTooLong",
    audio_invalid: "voiceAudioInvalid",
    audio_empty: "voiceAudioInvalid",
    temp_file_unavailable: "voiceInputFailed",
    temp_file_write_failed: "voiceInputFailed",
    session_mismatch: "voiceContextMismatch",
    session_load_failed: "voiceContextMismatch",
  };

  function normalizeVoiceError(err, fallbackStage) {
    const name = String((err && err.name) || "");
    const rawCategory = (err && err.category) || "";
    const rawStage = (err && err.stage) || fallbackStage || "recording";
    const rawMessage = String((err && (err.message || err.toString && err.toString())) || err || "");
    const constraint = String((err && err.constraint) || "");
    const emptyResultLike = /ASR empty result|empty result|backend returned no usable|no usable result|failed \(exit 6\)|exit 6/i.test(rawMessage);
    const codeKey = (err && err.code && VOICE_ERROR_CODE_KEYS[err.code]) || "";
    if (name === "NotAllowedError" || name === "SecurityError" || rawCategory === "permission_denied") {
      return { category: "permission_denied", stage: "permission", message: bt("voicePermissionDenied") };
    }
    if (rawCategory === "device_unavailable") {
      // 带自定义 message 的 device_unavailable（如检测超时）优先展示原文，与 tauri 车道一致。
      return { category: "device_unavailable", stage: "device", message: rawMessage || bt("voiceNoDevice") };
    }
    if (name === "NotFoundError" || name === "DevicesNotFoundError") {
      // 浏览器 DOMException 的 message 恒非空且未本地化;与 tauri 车道同口径,
      // 恒用三语指引文案而非浏览器原文。
      return { category: "device_unavailable", stage: "device", message: bt("voiceNoDevice") };
    }
    // Chrome 把"麦克风被其他应用占用"报为 NotReadableError / TrackStartError,
    // 与"没有设备"不同;浏览器原文是英文,映射成三语专用文案。
    if (name === "NotReadableError" || name === "TrackStartError") {
      return { category: "device_unavailable", stage: "device", message: bt("voiceMicUnavailable") };
    }
    // WebKitGTK 可能把不支持的音频约束报为 OverconstrainedError / "Invalid constraint"。
    // 这和没有录音设备不同：设备可能存在，只是不支持 channelCount、降噪等配置。
    if (name === "OverconstrainedError" || name === "ConstraintNotSatisfiedError" || /invalid constraint/i.test(rawMessage)) {
      return {
        category: "constraint_unsupported",
        stage: "device",
        message: bt("voiceConstraintUnsupported"),
        diagnostic: constraint ? "unsupported media constraint: " + constraint : "unsupported media constraint",
      };
    }
    if (rawCategory === "empty_result" || emptyResultLike) {
      return {
        category: "empty_result",
        stage: rawStage,
        message: bt("voiceEmptyResult"),
        diagnostic: rawMessage || "",
      };
    }
    if (rawCategory === "context_mismatch") {
      return { category: "context_mismatch", stage: "writeback", message: bt("voiceContextMismatch") };
    }
    if (rawCategory === "timeout") {
      return { category: "timeout", stage: "recording", message: bt("voiceTimeout") };
    }
    if (rawCategory === "recognition_failed") {
      if (codeKey) {
        return { category: rawCategory, stage: rawStage, message: bt(codeKey), diagnostic: rawMessage };
      }
      return { category: rawCategory, stage: rawStage, message: rawMessage || bt("voiceRecognitionFailed") };
    }
    // 远控 RPC 错误经 bootstrap 重建后带 code/category,但可能不属于上述任何
    // category 分支;有稳定错误码按码映射三语文案,中文原文降级为诊断(与 tauri
    // 车道收尾分支同口径)。
    if (codeKey) {
      return { category: rawCategory || "recording_failed", stage: rawStage, message: bt(codeKey), diagnostic: rawMessage };
    }
    return {
      category: rawCategory || "recording_failed",
      stage: rawStage,
      message: rawMessage || bt("voiceInputFailed"),
    };
  }

  function stopMediaTracks(stream) {
    if (!stream) return;
    stream.getTracks().forEach(function (track) { try { track.stop(); } catch { /* already-stopped tracks need no handling */ } });
  }

  function cleanupVoiceInputSession(session) {
    if (!session) return;
    if (session.timeoutId) clearTimeout(session.timeoutId);
    if (session.permissionTimeoutId) clearTimeout(session.permissionTimeoutId);
    session.permissionTimeoutId = null;
    if (session.cancelPermissionRequest) {
      const cancelPermissionRequest = session.cancelPermissionRequest;
      session.cancelPermissionRequest = null;
      try { cancelPermissionRequest(); } catch { /* permission request already settled */ }
    }
    // 先摘掉音频回调：webkit2gtk 的 WebAudio 是 GStreamer 后端，ScriptProcessorNode 的
    // onaudioprocess 跑在音频线程，若在 disconnect/close 期间再触发一次、访问已释放的
    // 缓冲，会让 WebProcess 段错误（表现为「识别出文字后 app 崩溃」）。务必先置 null。
    try { if (session.processor) session.processor.onaudioprocess = null; } catch { /* release failure only affects this page's audio */ }
    try { if (session.processor) session.processor.disconnect(); } catch { /* release failure only affects this page's audio */ }
    try { if (session.source) session.source.disconnect(); } catch { /* release failure only affects this page's audio */ }
    try { if (session.zeroGain) session.zeroGain.disconnect(); } catch { /* release failure only affects this page's audio */ }
    stopMediaTracks(session.stream);
    session.processor = null;
    session.source = null;
    session.zeroGain = null;
    session.stream = null;
    // close() 触发 GStreamer 管线异步拆解，与上面的 disconnect/track.stop 在同一拍里竞争最易崩；
    // 摘干净节点后挪到下一个事件循环再关，并吞掉 close 的异常。
    const ctx = session.audioContext;
    session.audioContext = null;
    if (ctx && ctx.state !== "closed") {
      setTimeout(function () { try { ctx.close().catch(function () {}); } catch { /* audio context already closed */ } }, 0);
    }
  }

  // 远控车道有两道 1MiB 闸:第一道是 RPC 分发层对序列化入参的信封预检
  // (manager/mod.rs MAX_RPC_REQUEST_BYTES),第二道才是 remote_control.rs 的
  // 解码后校验。base64 膨胀 4/3 后,30s WAV(0.92MiB→约 1.22MiB 字符)必被
  // 第一道拒收(request_too_large);20s≈0.64MiB→约 0.85MiB 字符,留足信封
  // 余量,故本车道上限 20s(桌面车道无该中继限制,保持 60s)。
  const VOICE_RECORDING_MAX_DURATION_MS = 20000;
  const VOICE_DEVICE_REQUEST_TIMEOUT_MS = 8000;

  function requestVoiceMedia(session, constraints, timeoutMs) {
    let abandoned = false;
    const mediaPromise = navigator.mediaDevices.getUserMedia(constraints).then(function (stream) {
      if (abandoned || activeVoiceInput !== session) {
        stopMediaTracks(stream);
        throw voiceFlowError("cancelled", "permission", bt("voiceCancelled"));
      }
      return stream;
    });
    const timeoutPromise = new Promise(function (_, reject) {
      session.permissionTimeoutId = setTimeout(function () {
        abandoned = true;
        reject(voiceFlowError("device_unavailable", "device", bt("voiceDeviceTimeout")));
      }, timeoutMs || VOICE_DEVICE_REQUEST_TIMEOUT_MS);
    });
    const cancelPromise = new Promise(function (_, reject) {
      session.cancelPermissionRequest = function () {
        abandoned = true;
        reject(voiceFlowError("cancelled", "permission", bt("voiceCancelled")));
      };
    });
    return Promise.race([mediaPromise, timeoutPromise, cancelPromise]).finally(function () {
      if (session.permissionTimeoutId) clearTimeout(session.permissionTimeoutId);
      session.permissionTimeoutId = null;
      session.cancelPermissionRequest = null;
    });
  }

  function mergeFloatChunks(chunks) {
    const total = chunks.reduce(function (sum, chunk) { return sum + chunk.length; }, 0);
    const out = new Float32Array(total);
    let offset = 0;
    chunks.forEach(function (chunk) {
      out.set(chunk, offset);
      offset += chunk.length;
    });
    return out;
  }

  function downsamplePcm(samples, sourceRate, targetRate) {
    if (!samples.length || sourceRate === targetRate) return samples;
    const ratio = sourceRate / targetRate;
    const len = Math.max(1, Math.round(samples.length / ratio));
    const out = new Float32Array(len);
    for (let i = 0; i < len; i++) {
      const start = Math.floor(i * ratio);
      const end = Math.min(samples.length, Math.floor((i + 1) * ratio));
      let sum = 0;
      let count = 0;
      for (let j = start; j < end; j++) { sum += samples[j]; count++; }
      out[i] = count ? sum / count : samples[Math.min(start, samples.length - 1)];
    }
    return out;
  }

  function encodeWav(samples, sampleRate) {
    const dataSize = samples.length * 2;
    const buffer = new ArrayBuffer(44 + dataSize);
    const view = new DataView(buffer);
    function writeString(offset, value) {
// the WAV header only writes ASCII; charCode is the target byte value. fromCodePoint/codePointAt gain nothing here.
      for (let i = 0; i < value.length; i++) view.setUint8(offset + i, value.charCodeAt(i)); // eslint-disable-line unicorn/prefer-code-point
    }
    writeString(0, "RIFF");
    view.setUint32(4, 36 + dataSize, true);
    writeString(8, "WAVE");
    writeString(12, "fmt ");
    view.setUint32(16, 16, true);
    view.setUint16(20, 1, true);
    view.setUint16(22, 1, true);
    view.setUint32(24, sampleRate, true);
    view.setUint32(28, sampleRate * 2, true);
    view.setUint16(32, 2, true);
    view.setUint16(34, 16, true);
    writeString(36, "data");
    view.setUint32(40, dataSize, true);
    let offset = 44;
    for (let i = 0; i < samples.length; i++, offset += 2) {
      const s = Math.max(-1, Math.min(1, samples[i]));
      view.setInt16(offset, s < 0 ? s * 0x8000 : s * 0x7fff, true);
    }
    return buffer;
  }

  async function finishVoiceInput(cancelled, timedOut) {
    const session = activeVoiceInput;
    if (!session) return;
    if (cancelled) {
      cleanupVoiceInputSession(session);
      activeVoiceInput = null;
      setVoiceInputStatus("cancelled", { message: bt("voiceCancelled"), completedAt: Date.now() });
      emitVoiceDiagnostic("recording", "info", "voice input cancelled", "已取消语音输入", "cancelled");
      return;
    }

    let mode = normalizeVoiceMode(session.mode);
    // 纵深防御:会话创建时已降级(edit 在 start 侧改写为 dictation),这里保留
    // 兜底,避免未纠错的 ASR 原文经替换预览整体覆盖草稿。
    if (mode === "edit") mode = "dictation";
    setVoiceInputStatus("transcribing", { message: bt("voiceTranscribing"), stage: "transcribing", mode });
    cleanupVoiceInputSession(session);

    try {
      if (timedOut) {
        emitVoiceDiagnostic("recording", "warn", "recording reached max duration", "", "timeout");
      }
      const raw = mergeFloatChunks(session.chunks);
      const durationMs = raw.length / Math.max(1, session.sampleRate) * 1000;
      if (durationMs < 300) {
        throw voiceFlowError("recording_failed", "recording", bt("voiceTooShort"));
      }
      const pcm = downsamplePcm(raw, session.sampleRate, 16000);
      const wav = encodeWav(pcm, 16000);
      const wavBytes = new Uint8Array(wav);
      // 本文件仅 web 远控车道加载;后端 transcribe_voice_audio 已改为 base64
      // 入参,旧的 audio_bytes JSON 数组分支已从后端契约移除,不再保留死分支。
      const res = await invoke("web_access_transcribe_voice_audio", {
        audioBase64: encodeBase64Bytes(wavBytes),
        sessionId: session.sessionId,
      });
      if (activeVoiceInput !== session) return;
      const text = String((res && res.text) || "").trim();
      if (!text) throw voiceFlowError("empty_result", "transcribing", "未识别到语音内容");
      if (state.activeSessionId !== session.sessionId) {
        throw voiceFlowError("context_mismatch", "writeback", "voice result discarded because active session changed");
      }
      if (typeof session.writeback === "function") {
        await session.writeback(text, session.draftBeforeStart, {
          mode,
          rawText: text,
          diagnostic: {
            mode,
            normalize_strategy: "web_asr_only",
            // 与桌面端一致：诊断只持久化长度等元信息，不落识别全文。
            raw_text_length: text.length,
            final_text_length: text.length,
            // web 车道没有本地高危词表与 LLM 后处理（web_asr_only），无法计算桌面端的
            // task_send_blocked 高危拦截；此处固定 false 仅保持诊断字段形状与桌面端一致。
            // 注意：当前调用方 writeback 链路只拦截多智能体只读与超长截断，并没有
            // 高危词筛查——与桌面车道存在潜伏分叉。今天没有任何 UI 入口以 task 模式
            // 进入 web 语音（麦克风按钮固定 dictation、手势自动升级只到 edit），分叉
            // 不可达；若未来放开 web task 入口，必须先把桌面
            // VOICE_SUSPICIOUS_ASR_TERMS/hasVoiceHighRiskResidual 复制进本桥（经典
            // 脚本无法共享模块）并按其结果计算 task_send_blocked。
            task_send_blocked: false,
          },
        });
        // 与桌面车道同口径:writeback await 窗口内用户可能已取消(取消把
        // activeVoiceInput 置空并置 cancelled),不得再用 completed 覆盖取消终态。
        if (activeVoiceInput !== session) return;
      }
      setVoiceInputStatus("completed", {
        message: mode === "task" ? (bt("voiceTaskSent") || bt("voiceWritten")) : bt("voiceWritten"),
        completedAt: Date.now(),
        mode,
      });
      emitVoiceDiagnostic("writeback", "info", "voice text written back", "语音已写入输入框", "");
    } catch (err) {
      const normalized = normalizeVoiceError(err, "transcribing");
      setVoiceInputStatus("failed", {
        message: normalized.message,
        error: normalized.message,
        category: normalized.category,
        stage: normalized.stage,
        completedAt: Date.now(),
      });
      emitVoiceDiagnostic(normalized.stage, "error", normalized.diagnostic || normalized.category, normalized.message, normalized.category);
    } finally {
      if (activeVoiceInput === session) activeVoiceInput = null;
    }
  }

  // 一键安装本地语音识别依赖（模型下载 + 缺 ffmpeg 走 pkexec apt），进度走
  // voice_asr:progress 事件。装完 ready 自动关框。
  async function installVoiceAsr() {
    if (state.voiceAsrSetup.installing) return;
    state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, { installing: true, error: null, progress: { stage: "start" } });
    notify();
    try {
      const st = await invoke("install_voice_asr");
      const patch = { installing: false, status: st, progress: { stage: "done" } };
      if (st && st.ready) patch.open = false;
      state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, patch);
      notify();
    } catch (e) {
      state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, { installing: false, error: String(e) });
      notify();
    }
  }

  function closeVoiceAsrSetup() {
    state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, { open: false });
    notify();
  }

  // Web 车道没有本地 ASR 模型下载（缺失时 startVoiceInput 直接报 dependency_unavailable），
  // 不存在可中断的安装进程；保留与桌面端同名的取消入口，收起安装框后直接 resolve，
  // 避免 ChatView 在 web 上调用时抛 TypeError。
  async function cancelVoiceAsrSetup() {
    closeVoiceAsrSetup();
  }

  // 知识库 embedding 模型按需下载（下载 → 校验 → 解压部署 → 热加载），进度走
  // kb_model:progress 事件。resolve 时模型已就绪，调用方据 status.installed 收起 gate。
  async function downloadKbModel(repair) {
    if (state.kbModelSetup.downloading) return state.kbModelSetup.status;
    state.kbModelSetup = Object.assign({}, state.kbModelSetup, { downloading: true, error: null, progress: { stage: "start" } });
    notify();
    try {
      const st = await invoke("kb_model_download", { repair: !!repair });
      state.kbModelSetup = Object.assign({}, state.kbModelSetup, {
        downloading: false,
        startupLoading: false,
        startupReady: st && typeof st.ready === "boolean" ? st.ready : true,
        status: st,
        progress: { stage: "done" },
      });
      notify();
      return st;
    } catch (e) {
      const failedStatus = await invoke("kb_model_status").catch(function () { return null; });
      state.kbModelSetup = Object.assign({}, state.kbModelSetup, {
        downloading: false,
        startupLoading: false,
        startupReady: failedStatus && typeof failedStatus.ready === "boolean" ? failedStatus.ready : false,
        status: failedStatus || state.kbModelSetup.status,
        error: String(e),
      });
      notify();
      throw e;
    }
  }

  function cancelKbModel() {
    invoke("kb_model_cancel").catch(function () {});
  }

  // 与桌面端一致：申请麦克风前先回调 beforePermission（shortcut-intro 首用门控）。
  // 返回 true 继续录音；false 表示门控拒绝（已清理会话并复位 idle）或会话已被取消。
  async function passVoiceBeforePermissionGate(session, options) {
    if (!options || typeof options.beforePermission !== "function") return true;
    const shouldContinue = await options.beforePermission({
      mode: session.mode,
      sessionId: session.sessionId,
      draftBeforeStart: session.draftBeforeStart,
    });
    if (activeVoiceInput !== session) return false;
    if (shouldContinue !== false) return true;
    cleanupVoiceInputSession(session);
    activeVoiceInput = null;
    setVoiceInputStatus("idle", { message: "", stage: null, sessionId: null });
    return false;
  }

  async function startVoiceInput(draftText, writeback, options) {
    if (activeVoiceInput && state.voiceInput.status === "recording") {
      finishVoiceInput(false, false);
      return;
    }
    if (activeVoiceInput) {
      finishVoiceInput(true, false);
      return;
    }
    // 远控车道后端对该命令要求显式 sessionId(rpc.rs Required("sessionId"));
    // 欢迎页(尚无会话)录音必然失败,且错误是未翻译的英文原文。开麦前直接以
    // 三语文案快速失败,不录白录。
    if (!state.activeSessionId) {
      setVoiceInputStatus("failed", {
        message: bt("voiceNeedSession"),
        error: "session_required",
        category: "session_unavailable",
        stage: "permission",
        completedAt: Date.now(),
      });
      return;
    }

    // iOS/WebKit 只允许在用户点击的同步调用栈里启动 AudioContext。Web 端先在任何
    // await 之前创建并 resume，后续依赖检测和麦克风授权完成后复用这个 context。
    const AudioCtor = window.AudioContext || window.webkitAudioContext; // eslint-disable-line compat/compat -- Safari 14.0 ships webkitAudioContext; the || fallback above selects it
    let primedAudioContext = null;
    let primedAudioResume = null;
    if (IS_WEB && AudioCtor) {
      try {
        primedAudioContext = new AudioCtor();
        primedAudioResume = primedAudioContext.state === "suspended"
          ? primedAudioContext.resume().catch(function () {})
          : Promise.resolve();
      } catch {
        primedAudioContext = null;
        primedAudioResume = null;
      }
    }

    // 首次/缺组件：先检测本地语音识别依赖，缺则弹安装框、不进录音。
    try {
      const asrStatus = await invoke("voice_asr_status");
      // VoiceAsrStatus 只有 engine/ffmpeg/model/ready/missing,无 installable 字段。
      // 未装好即弹安装引导;平台 gating 若要做,需先给后端补 installable(当前无此需求)。
      if (asrStatus && !asrStatus.ready) {
        if (IS_WEB) {
          if (primedAudioContext) primedAudioContext.close().catch(function () {});
          setVoiceInputStatus("failed", {
            message: bt("voiceNeedDesktopAsr"),
            error: "voice_asr_not_ready",
            category: "dependency_unavailable",
            stage: "dependency",
            completedAt: Date.now(),
          });
          return;
        }
        state.voiceAsrSetup = { open: true, status: asrStatus, installing: false, progress: null, error: null };
        notify();
        return;
      }
    } catch {
      // 检测失败（如 mock 环境/旧后端）不阻塞，继续走原录音路径（环境变量/兜底引擎）
    }

    // Web 车道没有 LLM 后处理（web_asr_only），edit 指令无从执行:在会话创建
    // 时就降级为听写(状态展示、写回路径全程一致),不再等录完整个会话到
    // finish 侧才降级——那会让用户带着「语音编辑」的预期录完一整条。
    const sessionMode = normalizeVoiceMode(options && options.mode);
    const session = {
      id: Date.now().toString(36),
      sessionId: state.activeSessionId || null,
      draftBeforeStart: String(draftText || ""),
      writeback,
      mode: sessionMode === "edit" ? "dictation" : sessionMode,
      chunks: [],
      sampleRate: 16000,
      startedAt: Date.now(),
      audioContext: primedAudioContext,
    };
    activeVoiceInput = session;
    if (!await passVoiceBeforePermissionGate(session, options)) return;
    setVoiceInputStatus("requesting_permission", {
      message: bt("voiceRequestingPermission"),
      sessionId: session.sessionId,
      startedAt: session.startedAt,
      stage: "permission",
      mode: session.mode,
    });
    emitVoiceDiagnostic("permission", "info", "requesting microphone permission", "", "");

    try {
      if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
        throw voiceFlowError("device_unavailable", "device", bt("voiceNoMicCapture"));
      }
      if (!AudioCtor) {
        throw voiceFlowError("recording_failed", "recording", bt("voiceNoAudioRecording"));
      }
      session.stream = await requestVoiceMedia(session, {
        audio: {
          channelCount: 1,
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
        },
      });
      if (activeVoiceInput !== session) {
        cleanupVoiceInputSession(session);
        return;
      }
      session.audioContext = session.audioContext || new AudioCtor();
      if (primedAudioResume) await primedAudioResume;
      if (session.audioContext.state === "suspended") await session.audioContext.resume();
      if (session.audioContext.state !== "running") {
        throw voiceFlowError("recording_failed", "recording", bt("voiceAudioStartBlocked"));
      }
      session.sampleRate = session.audioContext.sampleRate || 16000;
      session.source = session.audioContext.createMediaStreamSource(session.stream);
      session.processor = session.audioContext.createScriptProcessor(4096, 1, 1);
      session.zeroGain = session.audioContext.createGain();
      session.zeroGain.gain.value = 0;
      session.processor.onaudioprocess = function (event) {
        if (activeVoiceInput !== session) return;
        const input = event.inputBuffer.getChannelData(0);
        session.chunks.push(new Float32Array(input));
      };
      session.source.connect(session.processor);
      session.processor.connect(session.zeroGain);
      session.zeroGain.connect(session.audioContext.destination);
      session.timeoutId = setTimeout(function () { finishVoiceInput(false, true); }, VOICE_RECORDING_MAX_DURATION_MS);
      // 拔麦/设备断开时 track 结束(stop() 不触发 onended,收尾无递归),
      // 与桌面车道同款:用已录内容立即收尾,不再空转到时长上限。
      session.stream.getTracks().forEach(function (track) {
        track.onended = function () {
          if (activeVoiceInput === session) finishVoiceInput(false);
        };
      });
      setVoiceInputStatus("recording", { message: bt("voiceRecording"), stage: "recording", mode: session.mode });
      emitVoiceDiagnostic("recording", "info", "recording started", "", "");
    } catch (err) {
      cleanupVoiceInputSession(session);
      // finishVoiceInput(cancelled) 已把会话收尾为 cancelled 并清掉 activeVoiceInput；
      // 权限挂起期间用户取消时本 catch 会在其后到达，不带 session 早退会把取消覆盖成 failed。
      if (activeVoiceInput !== session) return;
      activeVoiceInput = null;
      const normalized = normalizeVoiceError(err, "recording");
      setVoiceInputStatus("failed", {
        message: normalized.message,
        error: normalized.message,
        category: normalized.category,
        stage: normalized.stage,
        completedAt: Date.now(),
      });
      emitVoiceDiagnostic(normalized.stage, "error", normalized.diagnostic || normalized.category, normalized.message, normalized.category);
    }
  }

  function cancelVoiceInput() {
    finishVoiceInput(true, false);
  }

  function clearVoiceInput() {
    if (activeVoiceInput) {
      finishVoiceInput(true, false);
      return;
    }
    setVoiceInputStatus("idle", {
      message: "",
      error: null,
      category: null,
      stage: null,
      sessionId: null,
    });
  }

  function appendVoiceText(base, text) {
    const left = String(base || "").trimEnd();
    const right = String(text || "").trim();
    if (!left) return right;
    if (!right) return left;
    return left + (/[。！？.!?，,;；:]$/.test(left) ? " " : "\n") + right;
  }

  function runVoiceInputDebugAssertions() {
    const denied = normalizeVoiceError({ name: "NotAllowedError" });
    const noDevice = normalizeVoiceError({ name: "NotFoundError" });
    const unsupportedConstraint = normalizeVoiceError({ name: "OverconstrainedError", message: "Invalid constraint", constraint: "channelCount" });
    const mismatch = normalizeVoiceError({ category: "context_mismatch" });
    console.assert(denied.category === "permission_denied", "permission error classified");
    console.assert(noDevice.category === "device_unavailable", "device error classified");
    console.assert(unsupportedConstraint.category === "constraint_unsupported", "unsupported constraint classified");
    console.assert(unsupportedConstraint.diagnostic === "unsupported media constraint: channelCount", "unsupported constraint diagnostic");
    console.assert(mismatch.stage === "writeback", "context mismatch classified");
    console.assert(appendVoiceText("草稿", "识别文本") === "草稿\n识别文本", "voice text appended");
    return true;
  }

  async function pickFiles() {
    if (!dialogOpen) { addSystemItem(bt("filePickUnavailable")); return []; }
    const selected = await dialogOpen({ multiple: true });
    if (!selected) return [];
    return Array.isArray(selected) ? selected : [selected];
  }
  async function pickFolder() {
    if (!dialogOpen) throw new Error(bt("folderPickerUnavailable"));
    const selected = await dialogOpen({
      directory: true,
      multiple: false,
      title: bt("pickFolderTitle"),
    });
    if (!selected) return null;
    return Array.isArray(selected) ? (selected[0] || null) : selected;
  }
  // 知识库「添加文件夹」：host-file-picker 目录模式返回单个目录路径，
  // 包成数组交给后端 kb_collection_add_sources（在桌面进程用 WalkDir 递归展开）。
  async function pickFolders() {
    if (!dialogOpen) { addSystemItem(bt("filePickUnavailable")); return []; }
    const selected = await dialogOpen({ directory: true, multiple: false, title: bt("kbPickFolderTitle") });
    if (!selected) return [];
    const p = Array.isArray(selected) ? selected[0] : selected;
    return p ? [p] : [];
  }
  async function pickFeedbackFiles() {
    if (!dialogOpen) return [];
    const selected = await dialogOpen({
      multiple: true,
      filters: [
        { name: "Images and videos", extensions: ["png", "jpg", "jpeg", "gif", "webp", "mp4", "mov", "webm"] },
      ],
    });
    if (!selected) return [];
    return Array.isArray(selected) ? selected : [selected];
  }
  // ── Init ─────────────────────────────────────────────────────────
  function disarmWebInitRetry() {
    if (!webInitRetryArmed || !webInitRetryHandler) return;
    window.removeEventListener("pinvou:web-connection", webInitRetryHandler);
    webInitRetryArmed = false;
    webInitRetryHandler = null;
  }

  function armWebInitRetry() {
    if (!IS_WEB || webInitRetryArmed) return;
    webInitRetryArmed = true;
    webInitRetryHandler = function (event) {
      const status = event && event.detail && event.detail.status;
      if (status !== "connected") return;
      disarmWebInitRetry();
      window.setTimeout(function () {
        init().catch(function (error) {
          console.warn("[TauriBridge] Web init retry failed", error);
        });
      }, 0);
    };
    window.addEventListener("pinvou:web-connection", webInitRetryHandler);
  }

  async function init() {
    if (initPromise) return initPromise;
    const attempt = (async function () {
    // 启动加载各自写互不重叠的状态片、彼此无数据依赖(每个 loader 自吞 invoke
    // 错误并落兜底值),串行 await 会把多个 RPC 往返叠进首屏延迟——并行后往返
    // 宽度收敛为 1。enterDraft/markStateReady 必须等本组完成后才走(durable
    // state 未就绪前不得放行桌面事件重放,这是 web 重试契约的前提)。
    const parallelLoads = [
      loadSettings(),
      hasCapability("pet") ? loadSelectedPet() : Promise.resolve(),
      loadEffectiveModelConfig(),
      loadAppVersion(),
      loadModels(),
      refreshHistoryList()
    ];
    await Promise.all(parallelLoads);
    // Populate the global Scheduled unread summary without requiring the user
    // to visit the Scheduled page first.
    loadScheduledTasks().catch(function () {}).then(function () {
      loadScheduledTaskRecentRuns().catch(function () {});
    });
    enterDraft(); // 启动落空白草稿页(lazy session:不自动选/建会话)
    // Browser readiness is two-phase: the first barrier negotiates installed
    // desktop capabilities so these RPCs can run; only after the durable
    // Session index is loaded may the desktop replay live turn events.
    if (IS_WEB && window.PinvouWebClient && typeof window.PinvouWebClient.markStateReady === "function") {
      window.PinvouWebClient.markStateReady();
    }
    if (hasCapability("superPermission")) await refreshSuperPerm();
    // lane 全局默认（work/design/code）是草稿态 mode chip 的事实源，启动即拉取。
    refreshModeDefaults().catch(function () {});
    loadPersonas(); // 预载卡池(让聊天里草稿"已存入"判定能查到同名自制卡), fire-and-forget
    pollBackendStatus();
    setInterval(pollBackendStatus, 10000);
    if (hasCapability("appUpdate")) {
      reportPendingUpdateResult();
      checkForUpdateSilently();
    }
    if (hasCapability("webAccessAdmin")) refreshWebAccessStatus();
    notify();
    })();
    initPromise = attempt.then(function (result) {
      disarmWebInitRetry();
      return result;
    }, function (error) {
      // A rejected Promise must not permanently poison every later init call.
      // Keep replay closed and retry only after the browser observes a fresh
      // connected transition; durable state must succeed before state_ready.
      const client = IS_WEB && window.PinvouWebClient;
      if (client && !client.stateReady) {
        initPromise = null;
        armWebInitRetry();
      }
      throw error;
    });
    return initPromise;
  }

  // ── Expose API ───────────────────────────────────────────────────
  window.TauriBridge = {
    available: true,
    platform: PLATFORM.kind || "desktop",
    capabilities: PLATFORM.capabilities || {},
    hasCapability,
    subscribe,
    getState: function () { return snapshotState(); },
    init,
    sendMessage,
    sendMessageToSession,
    getComposerDraft,
    setComposerDraft,
    retryFirstTurn,
    prefillComposer,
    removeQueued,
    prioritizeQueued,
    editQueued,
    startVoiceInput,
    installVoiceAsr,
    cancelVoiceAsrSetup,
    closeVoiceAsrSetup,
    downloadKbModel,
    cancelKbModel,
    cancelVoiceInput,
    clearVoiceInput,
    appendVoiceText,
    runVoiceInputDebugAssertions,
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
    cancelGeneration,
    cancelShellTask,
    createNewSession,
    switchToSession,
    openScheduledRunChat,
    exitScheduledRunChat,
    deleteSession,
    renameSession,
    toggleSessionPinned,
    archiveSession,
    restoreArchivedSession,
    startMonitorPolling,
    stopMonitorPolling,
    clearMonitorStats,
    setSelectedPet,
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
   loadModels,
   saveModel,
   revealModelApiKey,
   deleteModel,
    getImageInputCapability,
    testImageInputCapability,
    setActiveModel,
    loadSessionModel,
    switchModel,
    testModelConnection,
    probeLocalServerKind,
    testSearchProvider,
    toggleSuperPerm,
    renderMarkdown,
    enableWebAccess,
    disableWebAccess,
    rotateWebAccessLink,
    refreshWebAccessStatus,
    getWebRelaySettings,
    setWebRelayAddress,
    resetWebRelayAddress,
    // modeState 权威读取（评审 P1 后纳入公开面，与 tauri 端对齐）
    syncModeState,
    // Plan/YOLO
    acceptPlan,
    discardPlan,
    exitPlanToYolo,
    setPlanModeNext,
    setDraftMode,
    setModeLane,
    refreshModeDefaults,
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
    // 产物
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
    // 附件
    addAttachmentByPath,
    addPasteImage,
    removeAttachment,
    clearAttachments,
    pickAndAttach,
    uploadDeviceFiles,
    resolveConversationAttachment,
    openConversationAttachment,
    revealConversationAttachment,
    markResolved,
    // 通用宿主文件选择器（知识库、反馈等功能继续复用）。
    pickFiles,
    pickFolders,
    pickFeedbackFiles,
    // 卡片池: 专家面具
    loadPersonas,
    getPersonas: function () { return personaPoolCache; }, // 返回引用(只读),不进 notify 快照
    readPersonaBody: function (id) { return invoke("read_persona_body", { personaId: id }); }, // Side B: 详情拉完整正文
    equipPersona,
    unequipPersona,
    // 知识库挂载(会话级)
    mountCollection,
    setCollectionEnabled,
    removeCollection,
    unmountCollection,
    listCollections: function () { return invoke("kb_collection_list"); }, // 挂载选择器用
    kbModelStatus: function () { return invoke("kb_model_status"); }, // 挂载选择器门控:模型未装则不可选
    loadMemoryOverview,
    saveMemoryProfilePatch,
    deleteMemoryPreference,
    updateMemoryItem,
    deleteMemoryItem,
    archiveRecentWorkMemory,
    confirmMemoryCandidate,
    ignoreMemoryCandidate,
    neverMemoryCandidate,
    // AI 造卡开场引导卡:落一条展示气泡 + 记一条 persona 事件(随会话持久化)。
    // 走 personaEvents 时间线,冷重载时 rerenderFromMessages 按 pos 还原 → 切会话/重启不丢。
    postCardCreatorIntro,
    // 用户自创卡
    createPersona,
    updatePersona,
    deletePersona,
    // 应用内升级
    checkForUpdate,
    downloadAndInstallUpdate,
    cancelUpdate,
    restartApp,
    checkDependencies,
    installDependencies,
  };

  function retryWebAuthoritySynchronization() {
    Object.keys(sessionStates).forEach(function (sid) {
      const buf = sessionStates[sid];
      if (!buf) return;
      if (buf.remoteTerminalSeen || (buf.remoteTurnActive && !buf.busy)) {
        reconcileRemoteTurn(sid).then(function (ready) {
          if (ready) flushQueued(sid);
        }).catch(function () {});
      } else if (!buf.busy && buf.queued && buf.queued.length) {
        flushQueued(sid);
      }
    });
  }
  if (IS_WEB) {
    window.addEventListener("pinvou:web-connection", function (event) {
      if (!event || !event.detail || event.detail.status !== "connected") return;
      scheduleAbandonedSessionDownloadCleanup();
      retryWebAuthoritySynchronization();
    });
    window.addEventListener("pinvou:web-capabilities", function () {
      if (!webInvokeCapabilitiesReady()) return;
      scheduleAbandonedSessionDownloadCleanup();
      retryCapabilityBlockedSessionSwitch();
      retryWebAuthoritySynchronization();
    });
  }

  if (IS_WEB && window.PinvouWebClient && typeof window.PinvouWebClient.markFrontendReady === "function") {
    window.PinvouWebClient.markFrontendReady();
  }

  // Auto-init after DOM ready
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    setTimeout(init, 0);
  }
})();
