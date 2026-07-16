/**
 * tauri-bridge.js — Tauri 后端通信桥
 *
 * 封装所有 invoke/listen，维护前端状态，通过 pub/sub 推给 React。
 * 浏览器预览时（无 window.__TAURI__）自动降级。
 */
(function () {
  "use strict";

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

  async function loadKnowledgeEmbedderAfterFirstFrame() {
    startupMark("bridge:knowledge_embedder_async:start");
    try {
      var ready = await invoke("kb_model_load_after_first_frame");
      state.kbModelSetup = Object.assign({}, state.kbModelSetup, { startupReady: !!ready });
      notify();
      startupMark("bridge:knowledge_embedder_async:done", "ready=" + !!ready);
      if (window.__PINVOU_STARTUP__) window.__PINVOU_STARTUP__.flush();
      return !!ready;
    } catch (error) {
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
    // 跨页面预填输入框请求。比如本地知识 → 产出物点击「续写/新项目」：
    // 只把草稿放进 composer，不自动发送给模型。
    composerPrefill: { id: 0, text: "" },
    messages: [],      // Anthropic Messages schema
    chatItems: [],     // display items for React
    // 卡牌加持/卸下事件时间线(sidecar, 不进 messages/LLM)。每项 {kind,pos,...}。
    // pos = 事件发生时的 messages 数, rerender 时按 pos 插回原位, 让重载历史不割裂。
    personaEvents: [],
    // Pinvou 召唤检阅时间线(sidecar, 同 personaEvents, 不进 messages/LLM)。每项 {pos, review}。
    pinvouReviews: [],
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
    llmApiStatus: null,
    llmApiModels: null,
    // 「添加模型」方案:已保存模型列表 + 全局默认 id + 当前会话绑定的模型 id
    savedModels: [],
    activeModelId: null,
    currentSessionModelId: null, // 当前 active session 显式绑定的模型;null=跟随全局默认
    superPermEnabled: false,
    modeState: { mode: "yolo" },
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
    // personaPool 只放轻量元信息(loadState),1078 张卡放模块级 personaPoolCache,
    // 不进 notify() 的 JSON 深拷贝(否则每个流式 token 都克隆 ~950KB,卡顿)。
    personaPool: { loadState: "idle" }, // idle | loading | ready | error
    // 应用内升级: updateInfo = check_for_update 返回值(available=true 才有意义)
    appVersion: null,
    updateInfo: null,
    remoteControl: {
      active: false,
      room_id: null,
      session_id: null,
      url: null,
      status: "idle",
      relay_url: "",
      last_error: null,
      pairing: null,
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
    vllmSetup: null,          // {eligible, is_megacube, has_packages, vllm_online, already_bootstrapped}, null=未检测
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
      status: null,       // kb_model_status 返回 { installed, downloading, sizeBytes, installedBytes, version }
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
  var personaPoolCache = [];
  var SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY = "pinvou3-scheduled-task-template-sources-v1";
  var scheduledTaskTemplateSources = loadScheduledTaskTemplateSources();

  // internal streaming state
  var currentStreamText = "";
  var currentStreamId = 0;
  var pendingAssistantText = "";
  var pendingAssistantBlocks = [];
  var itemIdSeq = 0;
  var toolMeta = {};       // id → { name, args }
  var shellNotifyTimer = null;
  // 上下文行口径保护：TurnComplete 的 usage.input_tokens 是本轮所有请求的累加
  // （计费口径）。只有单请求的"干净轮"该值才等于当前上下文占用；本轮一旦出现
  // 工具调用/重试/压缩（= 多请求），就跳过这次 tokens 更新，保留上一个准确值。
  var turnUsageDirty = {};  // session_id → bool
  var monitorIntervalId = null;
  var monitorPollInFlight = false;
  var gpuUtilHistory = [];
  var maxModelLen = 32768;
  // 监控页「清除统计」基准点：vLLM 的几个累计 counter（TTFT/TPOT/tokens/prefix
  // cache）无法真正清零（它们跟随远端 vLLM 进程生命周期，归零要重启共享进程）。
  // 改为记一个基准快照，显示值 = 当前 counter − 基准。换模型 / vLLM 重启 → counter
  // 倒退到小于基准，自动判定基准失效并丢弃，回落到生命周期累计值。持久化到
  // localStorage，关掉应用再开仍保持「自某时起」的统计。
  var MONITOR_BASELINE_KEY = "pinvou3.monitorStatsBaseline.self";
  var monitorBaseline = null;
  try {
    var _mb = localStorage.getItem(MONITOR_BASELINE_KEY);
    if (_mb) monitorBaseline = JSON.parse(_mb);
  } catch (e) { monitorBaseline = null; }
  var attachIdSeq = 0;
  var scheduledTaskSelectionGeneration = 0;
  var scheduledTaskRequestTokens = { tasks: 0, detail: 0, runs: 0 };
  var scheduledTaskRefreshInFlight = null;
  var scheduledRecentRunsRequestToken = 0;
  var scheduledRunEventRefreshTimer = null;
  var scheduledTaskPendingLoads = Object.create(null);
  var scheduledTaskAutoCreateInFlight = Object.create(null);
  var sessionSwitchRequestToken = 0;

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
    },
    ja: {
      newChatFailed: "⚠️ 新規チャットの作成に失敗: ", loadChatFailed: "⚠️ チャットの読み込みに失敗: ", deleteFailed: "⚠️ 削除に失敗: ",
      personaUnequipped: "🎴 エキスパートカードを外しました: ",
      planHistorical: "📜 過去のプラン", planSuperseded: "📜 新しいプランで上書きされました",
      attachStillParsing: "⚠️ 添付ファイルを解析中です。少し待ってから送信してください",
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
    },
    zh: {
      newChatFailed: "⚠️ 新建对话失败: ", loadChatFailed: "⚠️ 加载对话失败: ", deleteFailed: "⚠️ 删除失败: ",
      personaUnequipped: "🎴 已卸下专家卡牌: ",
      planHistorical: "📜 历史方案", planSuperseded: "📜 已被新方案覆盖",
      attachStillParsing: "⚠️ 附件还在解析,请稍后再发",
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
    },
  };
  function bt(key) {
    var lang = state.settings && state.settings.language;
    var m = lang === "en" ? BT_TABLE.en : lang === "ja" ? BT_TABLE.ja : BT_TABLE.zh;
    return m[key] !== undefined ? m[key] : BT_TABLE.zh[key];
  }

  // ── Per-session 工作集缓冲（多 session 并发）────────────────────
  // active session 的工作集 = state.* + 上面那批模块级 stream 变量(保持原逻辑零改动)。
  // 后台 session 的工作集存在 sessionStates[id];后台事件进来时临时把工作集切到对应
  // buffer 跑同步逻辑再切回(saveWorkingSetTo/loadWorkingSetFrom),期间 suppressNotify
  // 避免把后台渲染成 active。异步收尾(落盘)按显式 session_id 路由,不依赖工作集。
  var sessionStates = {};
  var scheduledRunSessionOwners = Object.create(null);
  var scheduledRunOpenInFlight = Object.create(null);
  var MAX_SCHEDULED_SESSION_BUFFERS = 64;
  var MAX_SCHEDULED_RUN_SESSION_OWNERS = 64;
  var sessionBufferTouchClock = 0;
  var scheduledRunOwnerTouchClock = 0;
  var suppressNotify = false;
  // sessionId → true:标题当前是「卡牌占位名」(加卡时自动取的),可被首条用户消息覆盖。
  // 卡牌名只在「加了卡但还没开口」时当临时标题;一旦开始对话,对话内容更能区分同卡会话。
  // 内存态(不持久化):重启后丢标记仅影响「加卡→重启→才发首条消息」这一冷门路径。
  var personaPlaceholderTitles = {};
  function freshBuffer() {
    return {
      messages: [], chatItems: [], personaEvents: [], pinvouReviews: [], artifacts: [], busy: false, queued: [],
      loadedFromDisk: false,
      planSnapshot: { plan: null, todos: null },
      modeState: { mode: "yolo" },
      thinking: { active: false, phase: "thinking", toolName: "", startedAt: 0 },
      tokens: { input: 0, max: maxModelLen },
      activePersona: null, // 卡片池: 该 session 加持的专家面具(挂件用)
      mountedCollection: null, // 知识库: 该 session 挂载的知识集 id 或 null
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
    if (!sessionStates[id]) sessionStates[id] = freshBuffer();
    return touchSessionBuffer(id, sessionStates[id], id.indexOf("sched-") === 0);
  }
  function isProtectedScheduledBuffer(id, buf) {
    return id === state.activeSessionId ||
      !!buf.busy ||
      buf.scheduledInitialTurnPhase === "active" ||
      !!(buf.queued && buf.queued.length) ||
      !!(state.scheduledRunContext && state.scheduledRunContext.sessionId === id) ||
      state.scheduledTaskCreationSessionId === id;
  }
  function pruneScheduledSessionBuffers(keepId) {
    var scheduledIds = Object.keys(sessionStates).filter(function (id) {
      return !!sessionStates[id].scheduledRunSession;
    });
    var overflow = scheduledIds.length - MAX_SCHEDULED_SESSION_BUFFERS;
    if (overflow <= 0) return;
    scheduledIds.sort(function (left, right) {
      var delta = (sessionStates[left].lastTouched || 0) - (sessionStates[right].lastTouched || 0);
      return delta || left.localeCompare(right);
    });
    for (var i = 0; i < scheduledIds.length && overflow > 0; i++) {
      var id = scheduledIds[i];
      var buf = sessionStates[id];
      if (!buf || id === keepId || isProtectedScheduledBuffer(id, buf)) continue;
      delete sessionStates[id];
      delete turnUsageDirty[id];
      pruneScheduledRunSessionOwner(id);
      overflow -= 1;
    }
  }
  function touchSessionBuffer(id, buf, scheduled) {
    if (!buf) return null;
    if (scheduled) buf.scheduledRunSession = true;
    buf.lastTouched = ++sessionBufferTouchClock;
    if (buf.scheduledRunSession) pruneScheduledSessionBuffers(id);
    return buf;
  }
  function purgeSessionBuffer(id) {
    if (typeof id !== "string" || !id) return;
    delete sessionStates[id];
    delete turnUsageDirty[id];
    delete personaPlaceholderTitles[id];
    delete scheduledRunSessionOwners[id];
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
    var owner = scheduledRunSessionOwners[id];
    if (!owner) owner = scheduledRunSessionOwners[id] = { phase: null, lastTouched: 0 };
    if (owner.phase !== "terminal" && phase) owner.phase = phase;
    owner.lastTouched = ++scheduledRunOwnerTouchClock;
    pruneScheduledRunSessionOwners();
    return owner;
  }
  function scheduledRunOwnerVisibleRank(id) {
    var runs = state.scheduledTaskRuns || [];
    for (var i = 0; i < runs.length; i++) {
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
  function isProtectedScheduledRunOwner(id) {
    return scheduledRunOwnerPriority(id) > 1;
  }
  function pruneScheduledRunSessionOwner(id) {
    if (!scheduledRunSessionOwners[id] || isProtectedScheduledRunOwner(id, null)) return;
    delete scheduledRunSessionOwners[id];
  }
  function pruneScheduledRunSessionOwners() {
    var ids = Object.keys(scheduledRunSessionOwners);
    if (ids.length <= MAX_SCHEDULED_RUN_SESSION_OWNERS) return;
    ids.sort(function (left, right) {
      var priorityDelta = scheduledRunOwnerPriority(right) - scheduledRunOwnerPriority(left);
      if (priorityDelta) return priorityDelta;
      var leftVisibleRank = scheduledRunOwnerVisibleRank(left);
      var rightVisibleRank = scheduledRunOwnerVisibleRank(right);
      if (leftVisibleRank >= 0 || rightVisibleRank >= 0) {
        if (leftVisibleRank < 0) return 1;
        if (rightVisibleRank < 0) return -1;
        if (leftVisibleRank !== rightVisibleRank) return leftVisibleRank - rightVisibleRank;
      }
      var touchDelta = (scheduledRunSessionOwners[right].lastTouched || 0) -
        (scheduledRunSessionOwners[left].lastTouched || 0);
      return touchDelta || left.localeCompare(right);
    });
    for (var i = MAX_SCHEDULED_RUN_SESSION_OWNERS; i < ids.length; i++) {
      delete scheduledRunSessionOwners[ids[i]];
    }
  }
  function isScheduledRunTerminal(status) {
    var value = String(status || "").toLowerCase();
    return value === "completed" || value === "failed" || value === "canceled";
  }
  function rememberScheduledRunOwner(run) {
    if (!run) return;
    var id = typeof run.sessionId === "string" ? run.sessionId.trim() : "";
    if (!id) return;
    var status = String(run.status || "").toLowerCase();
    var phase = isScheduledRunTerminal(status)
      ? "terminal"
      : (status === "queued" || status === "running" ? "active" : null);
    registerScheduledRunOwner(id, phase);
  }
  function scheduledRunBuffer(id) {
    var buf = getBuffer(id);
    if (!buf) return null;
    registerScheduledRunOwner(id, null);
    return touchSessionBuffer(id, buf, true);
  }
  function markScheduledInitialTurnActive(id) {
    var buf = scheduledRunBuffer(id);
    var owner = registerScheduledRunOwner(id, "active");
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
    var buf = scheduledRunBuffer(id);
    registerScheduledRunOwner(id, "terminal");
    if (!buf || buf.scheduledInitialTurnPhase === "terminal") return buf;
    if (buf.scheduledInitialTurnPhase !== "active") {
      buf.scheduledInitialTurnPhase = "active";
    }
    buf.scheduledInitialTurnPhase = "terminal";
    return buf;
  }
  function beginScheduledOpenActivation(id) {
    var previous = sessionStates[id] || null;
    var snapshot = {
      id: id,
      existed: !!previous,
      previousPhase: previous && previous.scheduledInitialTurnPhase,
      previousBusy: previous ? !!previous.busy : false,
      previousStateBusy: state.activeSessionId === id ? !!state.busy : null,
    };
    var buf = markScheduledInitialTurnActive(id);
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
    var current = sessionStates[snapshot.id];
    if (!current || current !== snapshot.buffer) return;
    if (current.scheduledInitialTurnPhase === "terminal") return;
    if (current.lastTouched !== snapshot.activationTouch) return;
    if (!snapshot.existed) {
      delete sessionStates[snapshot.id];
    } else {
      current.scheduledInitialTurnPhase = snapshot.previousPhase;
      current.busy = snapshot.previousBusy;
    }
    if (state.activeSessionId === snapshot.id && snapshot.previousStateBusy !== null) {
      state.busy = snapshot.previousStateBusy;
    }
  }
  function saveWorkingSetTo(buf) {
    if (!buf) return;
    buf.messages = state.messages; buf.chatItems = state.chatItems; buf.artifacts = state.artifacts;
    buf.personaEvents = state.personaEvents;
    buf.pinvouReviews = state.pinvouReviews;
    buf.busy = buf.scheduledInitialTurnPhase === "active" ? true : state.busy;
    buf.planSnapshot = state.planSnapshot; buf.modeState = state.modeState;
    buf.thinking = state.thinking; buf.tokens = state.tokens; buf.queued = state.queued;
    buf.activePersona = state.activePersona;
    buf.mountedCollection = state.mountedCollection;
    buf.scheduledTaskDraft = state.scheduledTaskDraft;
    buf.stream = {
      currentStreamText: currentStreamText, currentStreamId: currentStreamId,
      pendingAssistantText: pendingAssistantText, pendingAssistantBlocks: pendingAssistantBlocks,
      itemIdSeq: itemIdSeq, toolMeta: toolMeta,
    };
  }
  function loadWorkingSetFrom(buf) {
    if (!buf) return;
    state.messages = buf.messages; state.chatItems = buf.chatItems; state.artifacts = buf.artifacts;
    state.personaEvents = buf.personaEvents || [];
    state.pinvouReviews = buf.pinvouReviews || [];
    state.pinvouModal = null; // 切 session 关掉检阅弹窗
    state.turnDirtyArtifacts = []; // turn 临时态,切 session 清空,别串到新 session
    state.turnPresentedArtifacts = [];
    state.busy = buf.scheduledInitialTurnPhase === "active" ? true : buf.busy;
    state.planSnapshot = buf.planSnapshot; state.modeState = buf.modeState;
    state.thinking = buf.thinking; state.tokens = buf.tokens; state.queued = buf.queued || [];
    state.activePersona = buf.activePersona || null;
    state.mountedCollection = buf.mountedCollection || null;
    state.scheduledTaskDraft = buf.scheduledTaskDraft || null;
    var s = buf.stream || {};
    currentStreamText = s.currentStreamText || ""; currentStreamId = s.currentStreamId || 0;
    pendingAssistantText = s.pendingAssistantText || ""; pendingAssistantBlocks = s.pendingAssistantBlocks || [];
    itemIdSeq = s.itemIdSeq || 0; toolMeta = s.toolMeta || {};
  }
  function hydrateWorkingSetFromSaved(buf, saved) {
    if (!buf || !saved) return;
    buf.messages = Array.isArray(saved.messages) ? saved.messages : [];
    buf.chatItems = [];
    buf.artifacts = Array.isArray(saved.artifacts) ? saved.artifacts.map(function (a) {
      var p = typeof a === "string" ? a : (a.storage_path || a.path || "");
      return { path: p, basename: basename(p) };
    }) : [];
    buf.artifacts = filterSessionArtifacts(buf.artifacts, saved.metadata && saved.metadata.id);
    buf.personaEvents = [];
    buf.pinvouReviews = [];
    buf.stream = {
      currentStreamText: "", currentStreamId: 0, pendingAssistantText: "",
      pendingAssistantBlocks: [], itemIdSeq: 0, toolMeta: {},
    };
  }
  async function ensureSessionBufferLoaded(sid) {
    if (!sid) return;
    if (sid === state.activeSessionId) return;
    var buf = getBuffer(sid);
    var meta = state.sessions.find(function (s) { return s.id === sid; }) || {};
    var knownCount = Number(meta.message_count || 0);
    if (buf.busy) return;
    if (buf.loadedFromDisk && (!knownCount || buf.messages.length >= knownCount)) return;
    if (!buf.loadedFromDisk && (buf.messages.length || buf.chatItems.length) && (!knownCount || buf.messages.length >= knownCount)) return;
    var saved = await invoke("load_session", { id: sid, setActive: false });
    var savedCount = saved && saved.metadata ? Number(saved.metadata.message_count || 0) : 0;
    if ((buf.messages.length || buf.chatItems.length) && savedCount <= buf.messages.length) {
      buf.loadedFromDisk = true;
      return;
    }
    hydrateWorkingSetFromSaved(buf, saved);
    try { buf.personaEvents = await invoke("get_session_persona_events", { sessionId: sid }) || []; } catch (e) { buf.personaEvents = []; }
    try { buf.pinvouReviews = await invoke("get_session_pinvou_reviews", { sessionId: sid }) || []; } catch (e) { buf.pinvouReviews = []; }
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
    if (state.activeSessionId) saveWorkingSetTo(getBuffer(state.activeSessionId));
    state.activeSessionId = id;
    var buf = sessionStates[id];
    if (!buf || (opts && opts.fresh)) buf = sessionStates[id] = freshBuffer();
    touchSessionBuffer(id, buf, id.indexOf("sched-") === 0);
    loadWorkingSetFrom(buf);
  }
  // 在指定 session 的工作集上跑一段【同步】逻辑。sid 是 active → 直接跑(零行为变化);
  // 否则临时切到该 buffer 跑完再切回(期间不 notify)。
  function runSyncOnSession(sid, fn) {
    if (!sid || sid === state.activeSessionId) { fn(); return; }
    var bg = sessionStates[sid]; if (!bg) return;
    touchSessionBuffer(sid, bg, isScheduledRunSession(sid));
    var realId = state.activeSessionId;
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
      // 召唤检阅等依赖 activeSessionId 的操作随之错乱。草稿态须切回干净的空工作集。
      loadWorkingSetFrom(realId ? getBuffer(realId) : freshBuffer());
    }
  }
  // 事件监听器统一入口:按 payload.session_id 路由同步逻辑;后台变更后补一次 notify 刷新列表。
  function onSessionEvent(e, fn) {
    var sid = (e && e.payload && e.payload.session_id) || state.activeSessionId;
    if (sid && sid !== state.activeSessionId) getBuffer(sid);
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

  // 落盘指定 session 的 messages + artifacts(active 用工作集,后台用其 buffer)。
  async function persistMessagesFor(sid) {
    if (!sid) return;
    if (isScheduledRunSession(sid)) return;
    var buf = sid === state.activeSessionId ? null : sessionStates[sid];
    var msgs = buf ? buf.messages : state.messages;
    var arts = filterSessionArtifacts(buf ? buf.artifacts : state.artifacts, sid);
    if (buf) buf.artifacts = arts;
    else state.artifacts = arts;
    var backendOwnsTranscript = isScheduledRunSession(sid);
    try {
      if (!backendOwnsTranscript) {
        await invoke("save_session_messages", { id: sid, messages: msgs });
      }
      try { await invoke("save_session_artifacts", { id: sid, paths: arts.map(function (a) { return a.path; }) }); } catch (_) {}
      var meta = state.sessions.find(function (s) { return s.id === sid; });
      if (!backendOwnsTranscript && (!meta || meta.title === "新对话" || meta.title === "New chat" || personaPlaceholderTitles[sid])) {
        var firstUser = msgs.find(function (m) { return m.role === "user"; });
        var text = firstUser && firstUser.content && firstUser.content.find(function (c) { return c.type === "text"; });
        if (text && text.text) {
          var newTitle = text.text.slice(0, 20);
          await invoke("rename_session", { id: sid, title: newTitle });
          if (meta) meta.title = newTitle;
          delete personaPlaceholderTitles[sid]; // 已被对话内容命名,卸下占位标记
        }
      }
    } catch (e) { console.warn("persist failed", e); }
  }

  // ── Pub/Sub ──────────────────────────────────────────────────────
  var subscribers = [];
  function snapshotState() {
    if (typeof structuredClone === "function") {
      try { return structuredClone(state); } catch (_) {}
    }
    return JSON.parse(JSON.stringify(state));
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
        title: meta.title || "新对话",
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

  function loadScheduledTaskTemplateSources() {
    try {
      var parsed = JSON.parse(window.localStorage.getItem(SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY) || "{}");
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return Object.create(null);
      return Object.keys(parsed).reduce(function (result, taskId) {
        if (typeof parsed[taskId] === "string" && parsed[taskId].trim()) {
          result[taskId] = parsed[taskId].trim();
        }
        return result;
      }, Object.create(null));
    } catch (_) {
      return Object.create(null);
    }
  }

  function persistScheduledTaskTemplateSources() {
    try {
      window.localStorage.setItem(
        SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY,
        JSON.stringify(scheduledTaskTemplateSources)
      );
    } catch (_) {}
  }

  function rememberScheduledTaskTemplateSource(taskId, templateId) {
    if (!taskId || !templateId) return;
    scheduledTaskTemplateSources[taskId] = templateId;
    persistScheduledTaskTemplateSources();
  }

  function forgetScheduledTaskTemplateSource(taskId) {
    if (!taskId || !Object.prototype.hasOwnProperty.call(scheduledTaskTemplateSources, taskId)) return;
    delete scheduledTaskTemplateSources[taskId];
    persistScheduledTaskTemplateSources();
  }

  function attachScheduledTaskTemplateSource(task) {
    if (!task || !task.id) return task;
    var templateId = task.templateId || scheduledTaskTemplateSources[task.id] || null;
    if (templateId) {
      task.templateId = templateId;
      if (scheduledTaskTemplateSources[task.id] !== templateId) {
        rememberScheduledTaskTemplateSource(task.id, templateId);
      }
    }
    return task;
  }

  function attachAndPruneScheduledTaskTemplateSources(tasks) {
    var activeIds = Object.create(null);
    (tasks || []).forEach(function (task) {
      if (!task || !task.id) return;
      activeIds[task.id] = true;
      attachScheduledTaskTemplateSource(task);
    });
    var changed = false;
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
    var found = false;
    state.scheduledTasks = (state.scheduledTasks || []).map(function (item) {
      if (item.id !== task.id) return item;
      found = true;
      return task;
    });
    if (!found) state.scheduledTasks = [task].concat(state.scheduledTasks || []);
  }

  function applyScheduledRunViewed(automationId, runId, receipt) {
    function markRunViewed(item) {
      var itemAutomationId = item.automationId || state.selectedScheduledTaskId;
      if (itemAutomationId !== automationId || item.id !== runId) return item;
      return Object.assign({}, item, { unread: false });
    }
    state.scheduledTaskRuns = (state.scheduledTaskRuns || []).map(markRunViewed);
    state.scheduledTaskRecentRuns = (state.scheduledTaskRecentRuns || []).map(markRunViewed);
    var hasUnreadRuns = receipt && typeof receipt.hasUnreadRuns === "boolean"
      ? receipt.hasUnreadRuns
      : (state.scheduledTaskRuns || []).some(function (item) {
          return (item.automationId || state.selectedScheduledTaskId) === automationId && !!item.unread;
        });
    state.scheduledTasks = (state.scheduledTasks || []).map(function (task) {
      return task.id === automationId
        ? Object.assign({}, task, { hasUnreadRuns: hasUnreadRuns })
        : task;
    });
    if (state.scheduledTaskDetail && state.scheduledTaskDetail.id === automationId) {
      state.scheduledTaskDetail = Object.assign({}, state.scheduledTaskDetail, {
        hasUnreadRuns: hasUnreadRuns,
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
    var generation = stamp.generation;
    scheduledTaskPendingLoads[generation] = (scheduledTaskPendingLoads[generation] || 0) + 1;
    if (generation === scheduledTaskSelectionGeneration) {
      state.scheduledTaskLoading = true;
      clearScheduledTaskLoadError();
      notify();
    }
  }

  function endScheduledTaskLoad(stamp) {
    var generation = stamp.generation;
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
      kind: kind,
      token: scheduledTaskRequestTokens[kind],
      generation: scheduledTaskSelectionGeneration,
      id: id || null,
    };
  }

  function isCurrentScheduledTaskRequest(stamp) {
    if (!stamp || stamp.generation !== scheduledTaskSelectionGeneration) return false;
    if (scheduledTaskRequestTokens[stamp.kind] !== stamp.token) return false;
    if (stamp.kind !== "tasks" && state.selectedScheduledTaskId !== stamp.id) return false;
    return true;
  }

  function selectScheduledTask(id) {
    var nextId = typeof id === "string" && id.trim() ? id.trim() : null;
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
    var start = String(text || "").indexOf("{");
    if (start < 0) return null;
    var depth = 0;
    var inString = false;
    var escaping = false;
    for (var i = start; i < text.length; i++) {
      var ch = text.charAt(i);
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
    try { return JSON.parse(text); } catch (_) {}
    try { return JSON.parse(String(text || "").replace(/,(\s*[}\]])/g, "$1")); } catch (_) {}
    var balanced = extractBalancedJsonObject(String(text || ""));
    if (!balanced) return null;
    try { return JSON.parse(balanced); } catch (_) {}
    try { return JSON.parse(balanced.replace(/,(\s*[}\]])/g, "$1")); } catch (_) {}
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

  function activeScheduledTaskModel() {
    var model = activeScheduledTaskModelConfig();
    return model && model.model || null;
  }

  function lockScheduledTaskDraftModel(draft) {
    if (!draft) return null;
    var active = activeScheduledTaskModelConfig();
    draft.model = draft.model || (active && active.model) || null;
    draft.modelId = draft.modelId || (active && active.id) || null;
    return draft;
  }

  function parseScheduledTaskDraftFromText(text) {
    if (!text || text.indexOf("{") < 0) return null;
    var preferred = null;
    var fallback = null;
    var re = /```([^\n`]*)\n([\s\S]*?)```/g;
    var match;
    while ((match = re.exec(text))) {
      var label = String(match[1] || "").trim().toLowerCase();
      var raw = String(match[2] || "").trim();
      if (!raw || raw.charAt(0) !== "{") continue;
      var candidate = normalizeScheduledTaskDraft(parseLooseJsonObject(raw));
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
    var active = activeScheduledTaskModelConfig();
    var lockedModel = state.scheduledTaskDraft.model || (active && active.model) || null;
    var lockedModelId = state.scheduledTaskDraft.modelId || (active && active.id) || null;
    var draft = normalizeScheduledTaskDraft(Object.assign({}, state.scheduledTaskDraft, editedDraft || {}, {
      model: lockedModel,
      modelId: lockedModelId,
    }));
    if (!draft) {
      var invalidDraftError = new Error("定时任务草稿缺少名称、任务说明或时间规则");
      setScheduledTaskError(invalidDraftError, "action");
      notify();
      throw invalidDraftError;
    }
    var created = await createScheduledTask({
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
  function autoCreateScheduledTaskDraft(draft, creationSessionId) {
    if (!draft || !creationSessionId || scheduledTaskAutoCreateInFlight[creationSessionId]) return;
    var lockedDraft = lockScheduledTaskDraftModel(draft);
    state.scheduledTaskDraft = null;
    var creation = Promise.resolve()
      .then(function () {
        return createScheduledTask(scheduledTaskInputFromDraft(lockedDraft));
      })
      .then(function (created) {
        if (state.scheduledTaskCreationSessionId === creationSessionId) {
          state.scheduledTaskCreationSessionId = null;
        }
        var creationBuffer = sessionStates[creationSessionId];
        if (creationBuffer) creationBuffer.scheduledTaskDraft = null;
        if (created && created.id) state.scheduledTaskAutoOpenId = created.id;
        notify();
        return created;
      })
      .catch(function (error) {
        // createScheduledTask 通常已记录错误；忙锁在进入 action 前抛出时在这里补记，且不产生未处理 Promise。
        if (!state.scheduledTaskError) setScheduledTaskError(error, "action");
        runSyncOnSession(creationSessionId, function () {
          addSystemItem("定时任务创建失败：" + scheduledTaskErrorText(error), {
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
    var stamp = scheduledTaskRequestStamp("tasks", null);
    beginScheduledTaskLoad(stamp);
    try {
      var tasks = await invoke("list_scheduled_tasks");
      if (!isCurrentScheduledTaskRequest(stamp)) return state.scheduledTasks;
      state.scheduledTasks = attachAndPruneScheduledTaskTemplateSources(
        Array.isArray(tasks) ? tasks : []
      );
      if (
        state.selectedScheduledTaskId &&
        !(state.scheduledTasks || []).some(function (task) { return task.id === state.selectedScheduledTaskId; })
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
    var stamp = scheduledTaskRequestStamp("detail", id);
    beginScheduledTaskLoad(stamp);
    try {
      var detail = await invoke("read_scheduled_task", { id: id });
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
    var rows = (state.scheduledTaskRecentRuns || []).slice();
    (Array.isArray(runs) ? runs : []).forEach(function (run) {
      if (!run) return;
      rememberScheduledRunOwner(run);
      var merged = Object.assign({}, run, {
        automationId: run.automationId || task.id,
        taskName: task.name || "定时任务",
        taskModel: task.model || null,
      });
      var index = rows.findIndex(function (row) { return row && row.id === merged.id; });
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
    var stamp = scheduledTaskRequestStamp("runs", id);
    beginScheduledTaskLoad(stamp);
    try {
      var runs = await invoke("list_scheduled_task_runs", { id: id, limit: limit });
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
    var requestToken = ++scheduledRecentRunsRequestToken;
    try {
      var tasks = state.scheduledTasks && state.scheduledTasks.length
        ? state.scheduledTasks
        : await loadScheduledTasks();
      if (requestToken !== scheduledRecentRunsRequestToken) {
        return state.scheduledTaskRecentRuns || [];
      }
      var runs = await invoke("list_scheduled_runs");
      if (requestToken !== scheduledRecentRunsRequestToken) {
        return state.scheduledTaskRecentRuns || [];
      }
      var tasksById = Object.create(null);
      (tasks || []).forEach(function (task) {
        if (task && task.id) tasksById[task.id] = task;
      });
      var rows = (Array.isArray(runs) ? runs : []).map(function (run) {
        if (!run) return null;
        rememberScheduledRunOwner(run);
        var automationId = run.automationId || run.automation_id;
        var task = tasksById[automationId] || null;
        return Object.assign({}, run, {
          automationId: automationId,
          taskName: task && task.name || "定时任务",
          taskModel: task && task.model || null,
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
    var generation = scheduledTaskSelectionGeneration;
    if (scheduledTaskRefreshInFlight && scheduledTaskRefreshInFlight.generation === generation) {
      return scheduledTaskRefreshInFlight.promise;
    }
    var selectedId = state.selectedScheduledTaskId;
    var requests = [loadScheduledTasks()];
    if (selectedId) {
      requests.push(readScheduledTask(selectedId));
      requests.push(loadScheduledTaskRuns(selectedId, limit || 20));
    }
    var promise = Promise.all(requests).finally(function () {
      if (scheduledTaskRefreshInFlight && scheduledTaskRefreshInFlight.promise === promise) {
        scheduledTaskRefreshInFlight = null;
      }
    });
    scheduledTaskRefreshInFlight = { generation: generation, promise: promise };
    return promise;
  }

  var scheduledRunShortcutRefreshes = Object.create(null);
  var SCHEDULED_LINK_POLL_FAST_MS = 1000;
  var SCHEDULED_LINK_POLL_SLOW_MS = 5000;
  var SCHEDULED_LINK_POLL_FAST_ATTEMPTS = 15;
  // 兜底上限:只在 run 卡在 queued/running 且永不终态时才会走到,正常路径靠下面
  // 「拿到 sessionId」或「进入终态」提前收工。
  var SCHEDULED_LINK_POLL_DEADLINE_MS = 30 * 60 * 1000;

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
    var key = automationId + ":" + runId;
    if (scheduledRunShortcutRefreshes[key]) return;
    scheduledRunShortcutRefreshes[key] = true;
    var deadline = Date.now() + SCHEDULED_LINK_POLL_DEADLINE_MS;

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

    function poll(attempt) {
      invoke("list_scheduled_task_runs", { id: automationId }).then(function (runs) {
        var task = (state.scheduledTasks || []).find(function (item) {
          return item && item.id === automationId;
        }) || { id: automationId, name: "定时任务" };
        mergeScheduledTaskRecentRuns(task, runs);
        notify();
        // 必须看原始响应:mergeScheduledTaskRecentRuns 会滤掉尚无 sessionId 的记录,
        // 从合并结果里读不到目标 run 的状态。
        var target = (Array.isArray(runs) ? runs : []).find(function (run) {
          return run && run.id === runId;
        });
        // 会话已挂上 → 记录已进侧边栏;run 已终态却仍无会话 → 会话没建起来,再等也不会有;
        // run 记录消失(被删或被 retention 清掉)→ 没有等待对象。三种情况都收工。
        if (!target || target.sessionId || isScheduledRunTerminal(target.status)) {
          stop();
          return;
        }
        again(attempt);
      }).catch(function () { again(attempt); });
    }

    poll(0);
  }

  function upsertScheduledTaskRun(run) {
    if (!run || !run.id) return;
    rememberScheduledRunOwner(run);
    if (state.selectedScheduledTaskId && run.automationId && state.selectedScheduledTaskId !== run.automationId) return;
    var found = false;
    state.scheduledTaskRuns = (state.scheduledTaskRuns || []).map(function (item) {
      if (item.id === run.id) {
        found = true;
        return run;
      }
      return item;
    });
    if (!found) state.scheduledTaskRuns = [run].concat(state.scheduledTaskRuns || []);
  }

  async function runScheduledTaskAction(action, operation) {
    if (state.scheduledTaskBusyAction) {
      throw new Error("另一个定时任务操作仍在进行中");
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

  var SCHEDULED_TASK_WRITABLE_FIELDS = ["name", "prompt", "rrule", "model", "modelId", "paused"];

  // Scheduled tasks always run as Yolo. Keep the wire boundary intentionally narrow so
  // legacy callers cannot reintroduce task-level permissions or external directories.
  function scheduledTaskBackendInput(input) {
    var source = input || {};
    var backendInput = { mode: "yolo" };
    SCHEDULED_TASK_WRITABLE_FIELDS.forEach(function (field) {
      if (Object.prototype.hasOwnProperty.call(source, field)) backendInput[field] = source[field];
    });
    return backendInput;
  }

  async function createScheduledTask(input) {
    return runScheduledTaskAction("create", async function () {
      var templateId = input && typeof input.templateId === "string" ? input.templateId.trim() : "";
      var selectAfterCreate = !input || input.selectAfterCreate !== false;
      var backendInput = scheduledTaskBackendInput(input);
      var created = await invoke("create_scheduled_task", { input: backendInput });
      if (!created || !created.id) {
        throw new Error("创建定时任务失败：后端未返回任务 ID");
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
      var backendInput = scheduledTaskBackendInput(input);
      var updated = await invoke("update_scheduled_task", { id: id, input: backendInput });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  async function pauseScheduledTask(id) {
    return runScheduledTaskAction("pause", async function () {
      var updated = await invoke("pause_scheduled_task", { id: id });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  async function resumeScheduledTask(id) {
    return runScheduledTaskAction("resume", async function () {
      var updated = await invoke("resume_scheduled_task", { id: id });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  async function toggleScheduledTaskPinned(id, pinned) {
    return runScheduledTaskAction(pinned ? "pin" : "unpin", async function () {
      var updated = await invoke("set_scheduled_task_pinned", { id: id, pinned: !!pinned });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  async function deleteScheduledTask(id) {
    return runScheduledTaskAction("delete", async function () {
      invalidateScheduledRecentRuns();
      var deleted = await invoke("delete_scheduled_task", { id: id });
      var deletedSessionIds = deleted && Array.isArray(deleted.deletedSessionIds)
        ? deleted.deletedSessionIds
        : [];
      var deletedSessionSet = Object.create(null);
      deletedSessionIds.forEach(function (sessionId) {
        deletedSessionSet[sessionId] = true;
        purgeSessionBuffer(sessionId);
      });
      forgetScheduledTaskTemplateSource(id);
      state.scheduledTasks = (state.scheduledTasks || []).filter(function (task) { return task.id !== id; });
      state.scheduledTaskRecentRuns = (state.scheduledTaskRecentRuns || []).filter(function (run) {
        return run && run.automationId !== id && !deletedSessionSet[run.sessionId];
      });
      state.scheduledTaskRuns = (state.scheduledTaskRuns || []).filter(function (run) {
        return run && run.automationId !== id && !deletedSessionSet[run.sessionId];
      });
      if (state.selectedScheduledTaskId === id) selectScheduledTask(null);
      notify();
      return deleted;
    });
  }

  async function runScheduledTaskNow(id) {
    return runScheduledTaskAction("run-now", async function () {
      var run = await invoke("run_scheduled_task_now", { id: id });
      invalidateScheduledTaskReads(id);
      upsertScheduledTaskRun(run);
      var runStatus = String(run && run.status || "").toLowerCase();
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
      var prompt = await invoke("scheduled_task_chat_prompt");
      state.scheduledTaskDraft = null;
      state.scheduledTaskCreationSessionId = null;
      state.scheduledTaskAutoOpenId = null;
      await createNewSession();
      state.scheduledTaskPendingGuide = prompt;
      prefillComposer("我想创建一个定时任务：");
      notify();
      return prompt;
    });
  }

  // ── Chat Items (display format for React) ────────────────────────
  function addChatItem(item) {
    item.id = ++itemIdSeq;
    state.chatItems.push(item);
  }
  // 成品卡是否"重复出卡":从 chatItems 末尾往前扫——先遇到该文件的修改工具(write/append/edit)
  // → 不算重复(文件改过了,该出新版卡/续卡,即"二次修改弹新卡");先遇到同名成品卡 → 算重复
  // (同一产物没改又 present 一次,模型常见啰嗦)。判据=「上一张同名卡之后有没有改过这个文件」。
  // 例外:扫到**用户发言**就放行——用户在上一张卡之后又开了口(典型「再推一次」「没看到」),
  // 这次 present 是新请求的响应,不是模型自发啰嗦;再去重 = 用户主动要却看不到任何反馈(实测 bug)。
  function isDuplicateArtifactCard(pathv) {
    var bn = basename(pathv);
    if (!bn) return false;
    for (var i = state.chatItems.length - 1; i >= 0; i--) {
      var it = state.chatItems[i];
      if (it.type === "tool" && (it.name === "write_file" || it.name === "append_file" || it.name === "edit_file")) {
        var ap = extractArtifactPath(it.args);
        if (ap && basename(ap) === bn) return false;
      }
      if (it.type === "user") return false;
      if (it.type === "artifact_card" && basename(it.path) === bn) return true;
    }
    return false;
  }
  function addSystemItem(text, meta) {
    var item = { type: "system", text: text, time: timeStr() };
    if (meta) {
      for (var k in meta) item[k] = meta[k];
    }
    addChatItem(item);
    notify();
  }
  function compactPruneRollupText(count) {
    return bt("compactDone") + bt("compactAuto") + " " +
      bt("compactPruneMerged") + " ×" + count;
  }
  function removeCompactionStartItem(compactId) {
    if (!compactId) return;
    for (var i = state.chatItems.length - 1; i >= 0; i--) {
      var it = state.chatItems[i];
      if (it.type === "system" && it.compactId === compactId && it.compactPhase === "start") {
        state.chatItems.splice(i, 1);
        return;
      }
    }
  }
  function addOrMergePruneCompaction(compactId) {
    removeCompactionStartItem(compactId);
    var last = state.chatItems[state.chatItems.length - 1];
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
      var assistantText = pendingAssistantBlocks
        .filter(function (block) { return block && block.type === "text" && block.text; })
        .map(function (block) { return block.text; })
        .join("\n\n");
      if (state.activeSessionId && state.activeSessionId === state.scheduledTaskCreationSessionId) {
        var scheduledTaskDraft = parseScheduledTaskDraftFromText(assistantText);
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
  async function refreshHistoryList() {
    try {
      state.sessions = await invoke("list_sessions");
    } catch (e) {
      console.warn("list_sessions failed", e);
      state.sessions = [];
    }
    try {
      state.archivedSessions = await invoke("list_archived_sessions");
    } catch (e) {
      state.archivedSessions = state.archivedSessions || [];
    }
    notify();
  }

  // 进入草稿态:不创建 session,只清空工作集 + activeSessionId=null,落在「你好」欢迎页。
  // session 在首次有实质内容(发消息 / 加卡牌,见 ensureSession)时才物化——这样会话列表里
  // 永远不会堆积没用过的空「新对话」(ChatGPT/Claude 式 lazy session)。
  function enterDraft() {
    sessionSwitchRequestToken += 1; // 新建/返回草稿会话使任何仍在等待的 load_session 结果失效
    state.scheduledRunContext = null;
    state.draftEpoch++; // 每次点击都自增——含下面提前返回的「已在草稿态」分支,让前端能重置 welcomeToolId
    state.scheduledTaskPendingGuide = null; // 换了对话,未发送的定时任务引导词作废

    // 已在干净草稿态 → 只 notify(epoch 已自增)。注意要连 chatItems 一起判空:messages 与 chatItems
    // 会背离(persona 气泡 / ensureSession 失败的 system 报错卡只进 chatItems),否则残留卡顶掉「你好」。
    if (!state.activeSessionId && state.messages.length === 0 && state.chatItems.length === 0) { notify(); return; }
    if (state.activeSessionId) saveWorkingSetTo(getBuffer(state.activeSessionId));
    state.activeSessionId = null;
    loadWorkingSetFrom(freshBuffer());
    notify();
  }
  // 公开「新建对话」入口(侧边栏按钮)= 进草稿态。名字保留以兼容前端调用。
  async function createNewSession() { enterDraft(); }

  // 草稿态首次有实质内容时真正向后端创建 session 并切为 active;已有 active 直接返回。
  // 返回新 session id,创建失败返回 null。调用方:sendMessage(首条消息) / equipPersona(加卡)。
  async function ensureSession() {
    if (state.activeSessionId) return state.activeSessionId;
    // 多 session 并发:不预热 engine。新建空 session 的 buffer 由 switchActiveTo({fresh}) 起。
    try {
      var meta = await invoke("create_session");
      switchActiveTo(meta.id, { fresh: true });
      await refreshHistoryList();
      await syncModeState();
      await syncActivePersona();
      await syncMountedCollection();
      notify();
      return state.activeSessionId;
    } catch (e) {
      addSystemItem(bt("newChatFailed") + e);
      return null;
    }
  }

  function reportSessionSwitchFailure(error, errorScope) {
    if (errorScope === "scheduled") {
      setScheduledTaskError(error, "navigation");
      notify();
      return;
    }
    addSystemItem(bt("loadChatFailed") + error);
  }

  function hydratedMessageKey(message, hideInternalEnvelope) {
    var blocks = message && Array.isArray(message.content) ? message.content : [];
    if (message && message.role === "user") {
      var resultIds = blocks.filter(function (block) {
        return block && block.type === "tool_result" && block.tool_use_id;
      }).map(function (block) { return block.tool_use_id; }).sort();
      if (resultIds.length) return "user:tool_results:" + resultIds.join("|");
      return "user:text:" + userMessageDisplayText(blocks, hideInternalEnvelope);
    }
    if (message && message.role === "assistant") {
      var toolIds = blocks.filter(function (block) {
        return block && block.type === "tool_use" && block.id;
      }).map(function (block) { return block.id; }).sort();
      if (toolIds.length) return "assistant:tool_uses:" + toolIds.join("|");
      blocks = blocks.filter(function (block) { return !block || block.type !== "thinking"; });
      try { return "assistant:" + JSON.stringify(blocks); } catch (_) {}
    }
    try { return JSON.stringify(message); } catch (_) { return String(message); }
  }

  function mergeHydratedMessages(durableMessages, liveMessages, hideInternalEnvelope) {
    var durable = Array.isArray(durableMessages) ? durableMessages.slice() : [];
    var counts = Object.create(null);
    durable.forEach(function (message) {
      var key = hydratedMessageKey(message, hideInternalEnvelope);
      counts[key] = (counts[key] || 0) + 1;
    });
    (Array.isArray(liveMessages) ? liveMessages : []).forEach(function (message) {
      var key = hydratedMessageKey(message, hideInternalEnvelope);
      if (counts[key]) {
        counts[key] -= 1;
      } else {
        durable.push(message);
      }
    });
    return durable;
  }

  function mergeHydratedArtifacts(durableArtifacts, liveArtifacts) {
    var merged = [];
    var seen = Object.create(null);
    (durableArtifacts || []).concat(liveArtifacts || []).forEach(function (artifact) {
      var path = typeof artifact === "string" ? artifact : (artifact && (artifact.path || artifact.storage_path)) || "";
      if (!path || seen[path]) return;
      seen[path] = true;
      merged.push({ path: path, basename: basename(path) });
    });
    return merged;
  }

  function hydratedChatItemKey(item) {
    if (!item || !item.type) return "";
    if (item.type === "assistant") return "assistant:" + String(item.html || item.text || "");
    if (item.type === "tool" && item.toolId) return "tool:" + item.toolId;
    if (item.type === "artifact_card") return "artifact:" + String(item.path || "");
    if (item.type === "user") return "user:" + String(item.text || item.html || "");
    if (item.type === "system") return "system:" + String(item.text || "");
    var stable = Object.assign({}, item);
    delete stable.id;
    delete stable.time;
    delete stable.streaming;
    try { return item.type + ":" + JSON.stringify(stable); } catch (_) { return item.type + ":" + String(stable); }
  }

  function mergeHydratedChatItems(liveChatItems, liveCurrentStreamId) {
    var remappedCurrentStreamId = 0;
    (liveChatItems || []).forEach(function (item) {
      var key = hydratedChatItemKey(item);
      var existingIndex = -1;
      if (key) {
        for (var i = state.chatItems.length - 1; i >= 0; i--) {
          if (hydratedChatItemKey(state.chatItems[i]) === key) {
            existingIndex = i;
            break;
          }
        }
      }
      if (existingIndex >= 0) {
        var existingId = state.chatItems[existingIndex].id;
        state.chatItems[existingIndex] = Object.assign({}, state.chatItems[existingIndex], item, {
          id: existingId,
        });
        if (item && item.id === liveCurrentStreamId) remappedCurrentStreamId = existingId;
        return;
      }
      var clone = Object.assign({}, item, { id: ++itemIdSeq });
      if (item && item.id === liveCurrentStreamId) remappedCurrentStreamId = clone.id;
      state.chatItems.push(clone);
    });
    return remappedCurrentStreamId;
  }

  async function switchToSessionInternal(id, preserveScheduledRunContext, errorScope, options) {
    var requestToken = ++sessionSwitchRequestToken;
    var forceDurableLoad = !!(options && options.forceDurableLoad);
    var hydrateLiveSession = !!(options && options.hydrateLiveSession);
    if (!id) {
      reportSessionSwitchFailure(new Error("该运行记录没有可打开的会话"), errorScope);
      return false;
    }
    if (hydrateLiveSession && !sessionStates[id]) sessionStates[id] = freshBuffer();
    if (id === state.activeSessionId && !forceDurableLoad && !hydrateLiveSession) {
      if (!preserveScheduledRunContext) state.scheduledRunContext = null;
      state.scheduledTaskPendingGuide = null;
      notify();
      return true;
    }
    // 多 session 并发:切换【不再 cancel】旧 session —— 它在自己的 engine 上继续跑,
    // 工作集存进 sessionStates 后台累积。切回来能看到完整(含切走期间产生的)内容。
    // 已有 buffer(切过/在跑)→ 直接换工作集;没有 → load_session 建 buffer + 重渲染。
    if (sessionStates[id] && !forceDurableLoad && !hydrateLiveSession) {
      if (!preserveScheduledRunContext) state.scheduledRunContext = null;
      state.scheduledTaskPendingGuide = null; // 仅在目标会话已确认可用后提交导航状态
      switchActiveTo(id, null);
      await syncModeState();
      await syncActivePersona();
      await syncMountedCollection();
      await loadMemoryOverview({ rehydratePending: true });
      if (requestToken !== sessionSwitchRequestToken || state.activeSessionId !== id) return false;
      notify();
      reconcileArtifacts(id); // 对账磁盘产物(fire-and-forget)
      return true;
    }
    var saved;
    try {
      saved = await invoke("load_session", { id: id });
    } catch (e) {
      if (requestToken === sessionSwitchRequestToken) reportSessionSwitchFailure(e, errorScope);
      return false;
    }
    if (requestToken !== sessionSwitchRequestToken) return false;
    if (!saved || !saved.metadata || !saved.metadata.id) {
      reportSessionSwitchFailure(new Error("会话数据无效"), errorScope);
      return false;
    }

    var personaEvents = [];
    var pinvouReviews = [];
    try { personaEvents = await invoke("get_session_persona_events", { sessionId: id }) || []; } catch (_) {}
    try { pinvouReviews = await invoke("get_session_pinvou_reviews", { sessionId: id }) || []; } catch (_) {}
    if (requestToken !== sessionSwitchRequestToken) return false;

    // load_session 与必要的直接会话数据均成功后，才一次性提交 active/context。
    if (state.activeSessionId) saveWorkingSetTo(getBuffer(state.activeSessionId));
    if (!preserveScheduledRunContext) state.scheduledRunContext = null;
    state.scheduledTaskPendingGuide = null;
    state.activeSessionId = saved.metadata.id;
    if (hydrateLiveSession) {
      var liveBuffer = sessionStates[id] || freshBuffer();
      loadWorkingSetFrom(liveBuffer);
      var liveMessages = Array.isArray(state.messages) ? state.messages.slice() : [];
      var liveChatItems = Array.isArray(state.chatItems) ? state.chatItems.slice() : [];
      var liveArtifacts = Array.isArray(state.artifacts) ? state.artifacts.slice() : [];
      var liveCurrentStreamId = currentStreamId;
      var hasLivePresentation = !!state.busy || !!currentStreamText || !!pendingAssistantText ||
        (Array.isArray(pendingAssistantBlocks) && pendingAssistantBlocks.length > 0);
      state.messages = mergeHydratedMessages(
        saved.messages,
        liveMessages,
        isScheduledRunSession(id)
      );
      state.personaEvents = personaEvents.length ? personaEvents : (liveBuffer.personaEvents || []);
      state.pinvouReviews = pinvouReviews.length ? pinvouReviews : (liveBuffer.pinvouReviews || []);
      state.artifacts = filterSessionArtifacts(
        mergeHydratedArtifacts(saved.artifacts, liveArtifacts),
        state.activeSessionId
      );
      rerenderFromMessages();
      if (hasLivePresentation) {
        currentStreamId = mergeHydratedChatItems(liveChatItems, liveCurrentStreamId);
      } else {
        resetPendingAssistant();
      }
      saveWorkingSetTo(liveBuffer);
    } else {
      loadWorkingSetFrom(sessionStates[id] = freshBuffer());
      state.messages = Array.isArray(saved.messages) ? saved.messages : [];
      sessionStates[id].loadedFromDisk = true;
      state.personaEvents = personaEvents;
      state.pinvouReviews = pinvouReviews;
      resetPendingAssistant();
      state.chatItems = [];
      state.artifacts = mergeHydratedArtifacts(saved.artifacts, []);
      state.artifacts = filterSessionArtifacts(state.artifacts, state.activeSessionId);
      rerenderFromMessages();
    }
    await syncModeState();
    await syncActivePersona();
    await syncMountedCollection();
    await loadMemoryOverview({ rehydratePending: true });
    if (requestToken !== sessionSwitchRequestToken || state.activeSessionId !== saved.metadata.id) return false;
    notify();
    reconcileArtifacts(id); // 对账磁盘产物(修重启/跟踪遗漏导致的面板缺文件)
    return true;
  }

  async function switchToSession(id) {
    return switchToSessionInternal(id, false, "chat");
  }

  async function openScheduledRunChatOnce(run, task) {
    var sessionId = run && typeof run.sessionId === "string" ? run.sessionId.trim() : "";
    if (!sessionId) {
      reportSessionSwitchFailure(new Error("该运行记录没有可打开的会话"), "scheduled");
      return false;
    }
    rememberScheduledRunOwner(run);
    var runStatus = String(run && run.status || "").toLowerCase();
    var openActivation = null;
    if (runStatus === "queued" || runStatus === "running") {
      openActivation = beginScheduledOpenActivation(sessionId);
    } else {
      scheduledRunBuffer(sessionId);
    }
    setScheduledTaskError(null);
    notify();
    var returnSessionId = state.scheduledRunContext
      ? state.scheduledRunContext.returnSessionId
      : state.activeSessionId;
    var liveBuffer = sessionStates[sessionId];
    var hasLiveTurn = !!(liveBuffer && (
      liveBuffer.busy ||
      liveBuffer.scheduledInitialTurnPhase === "active" ||
      (liveBuffer.queued && liveBuffer.queued.length) ||
      (liveBuffer.thinking && liveBuffer.thinking.active)
    ));
    var isTerminalRun = runStatus === "completed" || runStatus === "failed" || runStatus === "canceled";
    var forceDurableLoad = isTerminalRun && !hasLiveTurn;
    var switched = await switchToSessionInternal(sessionId, true, "scheduled", {
      forceDurableLoad: forceDurableLoad,
      hydrateLiveSession: !isTerminalRun,
    });
    if (!switched) {
      rollbackScheduledOpenActivation(openActivation);
      notify();
      return false;
    }
    if (forceDurableLoad) markScheduledInitialTurnTerminal(sessionId);
    else scheduledRunBuffer(sessionId);
    var automationId = (run && run.automationId) || (task && task.id) || null;
    var runId = (run && (run.runId || run.id)) || null;
    state.scheduledRunContext = {
      sessionId: sessionId,
      returnSessionId: returnSessionId,
      automationId: automationId,
      runId: runId,
      taskName: (task && task.name) || (run && (run.taskName || run.name)) || "",
      model: (task && task.model) || null,
      mode: "yolo",
    };
    // 先发布完整会话视图；只有已完成的运行才持久化为已查看。
    notify();
    if (automationId && runId && runStatus === "completed") {
      try {
        var receipt = await invoke("mark_scheduled_run_viewed", {
          automationId: automationId,
          runId: runId,
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
    var sessionId = run && typeof run.sessionId === "string" ? run.sessionId.trim() : "";
    if (!sessionId) return openScheduledRunChatOnce(run, task);
    if (scheduledRunOpenInFlight[sessionId]) return scheduledRunOpenInFlight[sessionId];
    var opening = openScheduledRunChatOnce(run, task);
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
    var context = state.scheduledRunContext;
    if (!context) return false;
    if (context.returnSessionId && context.returnSessionId !== context.sessionId) {
      var restored = await switchToSessionInternal(context.returnSessionId, true, "scheduled");
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

  async function deleteSession(id) {
    invalidateScheduledRecentRunsForSession(id);
    try {
      // 后端按 SessionKind 分发:定时运行会话在 delete_session 里联动删除
      // 该次 Session、Run 与底座 Task,任务定义与共享工作间保留。
      await invoke("delete_session", { id: id });
      // 统一清理工作集、实时状态、定时创建上下文与当前视图，避免手写字段漂移。
      purgeSessionBuffer(id);
      state.sessions = state.sessions.filter(function (s) { return s.id !== id; });
      state.archivedSessions = (state.archivedSessions || []).filter(function (s) { return s.id !== id; });
      state.scheduledTaskRecentRuns = (state.scheduledTaskRecentRuns || []).filter(function (run) {
        return !run || run.sessionId !== id;
      });
      state.scheduledTaskRuns = (state.scheduledTaskRuns || []).filter(function (run) {
        return !run || run.sessionId !== id;
      });
      notify();
    } catch (e) {
      addSystemItem(bt("deleteFailed") + e);
    }
  }

  async function renameSession(id, title) {
    invalidateScheduledRecentRunsForSession(id);
    try {
      await invoke("rename_session", { id: id, title: title });
      var s = state.sessions.find(function (s) { return s.id === id; });
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
    var s = state.sessions.find(function (s) { return s.id === id; });
    var scheduledRun = recentScheduledRunForSession(id);
    var prev = s ? !!s.pinned : false;
    var prevPinnedAt = s ? s.pinned_at : null;
    var previousRunPinned = scheduledRun ? !!scheduledRun.pinned : false;
    var previousRunPinnedAt = scheduledRun ? scheduledRun.pinnedAt : null;
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
      await invoke("set_session_pinned", { id: id, pinned: !!pinned });
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
    var idx = state.sessions.findIndex(function (s) { return s.id === id; });
    if (idx < 0) {
      // 定时运行会话不在 state.sessions;收起 = 从侧边栏记录移除,进设置页归档列表。
      var scheduledRun = recentScheduledRunForSession(id);
      if (!scheduledRun) return;
      var previousRuns = state.scheduledTaskRecentRuns || [];
      var wasViewingRun = state.activeSessionId === id;
      var previousContext = state.scheduledRunContext;
      // 与普通会话收纳同语义:保留 buffer(还能从设置页还原后重开),但要离开当前视图。
      if (wasViewingRun) saveWorkingSetTo(getBuffer(id));
      state.scheduledTaskRecentRuns = previousRuns.filter(function (run) {
        return !run || run.sessionId !== id;
      });
      leaveSessionView(id);
      notify();
      try {
        await invoke("set_session_archived", { id: id, archived: true });
        await refreshHistoryList();
      } catch (e) {
        state.scheduledTaskRecentRuns = previousRuns;
        if (wasViewingRun) {
          // active 与 scheduledRunContext 必须成对回滚,否则会落到
          // 「active 有值但 context 空」的错位态(界面回任务列表却仍持有会话)。
          state.activeSessionId = id;
          state.scheduledRunContext = previousContext;
          loadWorkingSetFrom(getBuffer(id));
        }
        console.warn("set_session_archived failed", e);
        notify();
      }
      return;
    }
    var s = state.sessions[idx];
    var archived = Object.assign({}, s, { archived: true, archived_at: new Date().toISOString(), pinned: false, pinned_at: null });
    var wasActive = state.activeSessionId === id;
    if (wasActive) saveWorkingSetTo(getBuffer(id));
    state.sessions.splice(idx, 1);
    state.archivedSessions = [archived].concat((state.archivedSessions || []).filter(function (x) { return x.id !== id; }));
    leaveSessionView(id);
    notify();
    try {
      await invoke("set_session_archived", { id: id, archived: true });
      await refreshHistoryList();
    } catch (e) {
      state.sessions.splice(idx, 0, s);
      state.archivedSessions = (state.archivedSessions || []).filter(function (x) { return x.id !== id; });
      if (wasActive) {
        state.activeSessionId = id;
        loadWorkingSetFrom(getBuffer(id));
      }
      console.warn("set_session_archived failed", e);
      notify();
    }
  }

  async function restoreArchivedSession(id) {
    var idx = (state.archivedSessions || []).findIndex(function (s) { return s.id === id; });
    if (idx < 0) return;
    var s = state.archivedSessions[idx];
    invalidateScheduledRecentRunsForSession(id);
    var restored = Object.assign({}, s, { archived: false, archived_at: null });
    state.archivedSessions.splice(idx, 1);
    state.sessions = [restored].concat(state.sessions || []);
    notify();
    try {
      await invoke("set_session_archived", { id: id, archived: false });
      await refreshHistoryList();
      // 还原的定时运行会话回侧边栏"定时任务记录"(refreshHistoryList 只管普通会话)。
      if (String(id).indexOf("sched-") === 0) loadScheduledTaskRecentRuns().catch(function () {});
    } catch (e) {
      state.archivedSessions.splice(idx, 0, s);
      state.sessions = (state.sessions || []).filter(function (x) { return x.id !== id; });
      console.warn("restore archived session failed", e);
      notify();
    }
  }

  // 实时态有专属气泡的工具（方案卡），重建时要还原成原卡而非普通工具卡。
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

  // 定时会话由 Engine 持久化原始送模消息；展示时只投影真实任务正文。
  // 原始 blocks 保持不变，供模型续聊；普通会话也不受内部标签过滤影响。
  function userMessageDisplayText(blocks, hideInternalEnvelope) {
    var textParts = (Array.isArray(blocks) ? blocks : [])
      .filter(function (block) { return block && block.type === "text"; })
      .map(function (block) { return String(block.text || ""); });
    if (!hideInternalEnvelope) return textParts.join("");

    return textParts.filter(function (text) {
      var trimmed = text.trim();
      return !(
        (trimmed.indexOf("<turn_meta>") === 0 && trimmed.lastIndexOf("</turn_meta>") === trimmed.length - "</turn_meta>".length) ||
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
        if (db.type === "tool_use" && (db.name === "write_file" || db.name === "append_file" || db.name === "edit_file")) {
          var dap = extractArtifactPath(db.input);
          if (dap) {
            lastDirtyArtifactId[dap] = db.id;
            if (db.name !== "edit_file") writtenArtifacts[dap] = true;
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
          var uitem2 = { type: "user", text: utext, time: "" };
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
              var contentForCard = (tm.name === "load_skill") ? "（技能已加载，内容不展示）" : c.content;
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
        } else if (b.type === "tool_use") {
          if (textBuf) {
            addChatItem({ type: "assistant", html: renderMarkdown(textBuf), time: "", streaming: false });
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
          // 还原"自动续卡":write_file/append_file 改的文件之前 present 过 → 续一张
          // 成品卡(与实时 tool_end 的自动续逻辑对齐,切会话不丢)。present 的卡按
          // 顺序在前(必须先 present 才进集合),此处 findPresentedArtifact 能命中。
          if (b.name === "write_file" || b.name === "append_file" || b.name === "edit_file") {
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
        addChatItem({ type: "assistant", html: renderMarkdown(textBuf), time: "", streaming: false });
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

  function updateToolItem(toolId, output, success) {
    for (var i = 0; i < state.chatItems.length; i++) {
      if (state.chatItems[i].type === "tool" && state.chatItems[i].toolId === toolId) {
        state.chatItems[i].output = output;
        state.chatItems[i].success = success;
        state.chatItems[i].state = success ? "done" : "failed";
        delete state.chatItems[i]._terminalParser;
        break;
      }
    }
  }
  function scheduleShellNotify() {
    if (shellNotifyTimer != null) return;
    shellNotifyTimer = window.setTimeout(function () {
      shellNotifyTimer = null;
      notify();
    }, 50);
  }

  function markBackgroundToolItem(toolId, sessionId, taskId, fallbackOutput) {
    for (var i = 0; i < state.chatItems.length; i++) {
      var item = state.chatItems[i];
      if (item.type !== "tool" || item.toolId !== toolId) continue;
      if (!item.liveOutput && fallbackOutput != null) item.output = fallbackOutput;
      item.success = null;
      item.state = "running";
      item.background = true;
      item.sessionId = sessionId || state.activeSessionId;
      item.taskId = taskId;
      return true;
    }
    return false;
  }

  function finishBackgroundToolItem(toolId, payload) {
    for (var i = 0; i < state.chatItems.length; i++) {
      var item = state.chatItems[i];
      if (item.type !== "tool" || item.toolId !== toolId) continue;
      var status = payload.status || "Failed";
      var success = status === "Completed";
      item.success = success;
      item.state = success ? "done" : "failed";
      item.background = false;
      item.shellStatus = status;
      item.exitCode = payload.exit_code;
      item.output = reconcileBackgroundTerminalOutput(item.output, payload);
      delete item._terminalParser;
      return true;
    }
    return false;
  }

  var MAX_PENDING_TERMINAL_SEQUENCE_CHARS = 16 * 1024;
  function rememberPendingTerminalSequence(parserState, input, start) {
    var pending = input.slice(start);
    // A malformed unterminated OSC/DCS sequence must not bypass the live
    // output tail limit and grow renderer memory without bound.
    parserState.pendingAnsi = pending.length <= MAX_PENDING_TERMINAL_SEQUENCE_CHARS ? pending : "";
  }

  function stripTerminalSequences(text, parserState) {
    var input = String((parserState.pendingAnsi || "") + (text || ""));
    parserState.pendingAnsi = "";
    var clean = "";
    for (var i = 0; i < input.length; i++) {
      if (input[i] !== "\x1b") {
        clean += input[i];
        continue;
      }
      if (i + 1 >= input.length) {
        rememberPendingTerminalSequence(parserState, input, i);
        break;
      }

      var kind = input[i + 1];
      if (kind === "[") {
        var csiEnd = i + 2;
        var malformedCsi = false;
        while (csiEnd < input.length) {
          var csiCode = input.charCodeAt(csiEnd);
          if (csiCode >= 0x40 && csiCode <= 0x7e) break;
          if (csiCode < 0x20 || csiCode > 0x3f) {
            malformedCsi = true;
            break;
          }
          csiEnd += 1;
        }
        if (malformedCsi) {
          i += 1;
          continue;
        }
        if (csiEnd >= input.length) {
          rememberPendingTerminalSequence(parserState, input, i);
          break;
        }
        i = csiEnd;
        continue;
      }

      // OSC/DCS/SOS/PM/APC are terminated by ST (ESC \); OSC also accepts BEL.
      if (kind === "]" || kind === "P" || kind === "X" || kind === "^" || kind === "_") {
        var stringEnd = i + 2;
        var terminated = false;
        while (stringEnd < input.length) {
          if (kind === "]" && input[stringEnd] === "\x07") {
            terminated = true;
            break;
          }
          if (input[stringEnd] === "\x1b" && input[stringEnd + 1] === "\\") {
            stringEnd += 1;
            terminated = true;
            break;
          }
          stringEnd += 1;
        }
        if (!terminated) {
          rememberPendingTerminalSequence(parserState, input, i);
          break;
        }
        i = stringEnd;
        continue;
      }

      // Generic two-or-more-byte escape sequence: optional intermediate
      // bytes followed by a final byte.
      var escapeEnd = i + 1;
      while (escapeEnd < input.length) {
        var escapeCode = input.charCodeAt(escapeEnd);
        if (escapeCode < 0x20 || escapeCode > 0x2f) break;
        escapeEnd += 1;
      }
      if (escapeEnd >= input.length) {
        rememberPendingTerminalSequence(parserState, input, i);
        break;
      }
      var finalCode = input.charCodeAt(escapeEnd);
      if (finalCode >= 0x30 && finalCode <= 0x7e) i = escapeEnd;
    }
    return clean;
  }

  function terminalParserState(item, stream) {
    if (!item._terminalParser) {
      Object.defineProperty(item, "_terminalParser", {
        value: {},
        writable: true,
        configurable: true,
      });
    }
    var key = stream === "stderr" ? "stderr" : "stdout";
    if (!item._terminalParser[key]) {
      item._terminalParser[key] = { pendingCR: false, pendingAnsi: "" };
    }
    return item._terminalParser[key];
  }

  // A standalone carriage return resets the current terminal line. WinGet
  // uses this for progress frames, so keep the newest frame instead of
  // appending hundreds of nearly identical lines.
  function mergeTerminalChunk(previous, chunk, parserState, prefix) {
    var output = String(previous == null ? "" : previous);
    var clean = stripTerminalSequences(chunk, parserState);
    var i = 0;
    if (parserState.pendingCR && clean) {
      if (clean[0] === "\n") {
        output += "\n";
        i = 1;
      } else {
        output = output.slice(0, output.lastIndexOf("\n") + 1);
      }
      parserState.pendingCR = false;
    }
    var needsPrefix = !!prefix;
    for (; i < clean.length; i++) {
      var ch = clean[i];
      if (ch === "\r") {
        if (clean[i + 1] === "\n") {
          output += "\n";
          i += 1;
        } else if (i + 1 >= clean.length) {
          parserState.pendingCR = true;
        } else {
          output = output.slice(0, output.lastIndexOf("\n") + 1);
        }
      } else if (ch === "\b") {
        var lineStart = output.lastIndexOf("\n") + 1;
        if (output.length > lineStart) output = output.slice(0, -1);
      } else {
        if (needsPrefix) {
          output += prefix;
          needsPrefix = false;
        }
        output += ch;
      }
    }
    return output;
  }

  function mergeTerminalTail(previous, tail) {
    var output = String(previous == null ? "" : previous);
    var suffix = String(tail == null ? "" : tail);
    if (!suffix) return output;
    if (!output) return suffix;
    if (output.indexOf(suffix) >= 0) return output;

    var maxOverlap = Math.min(output.length, suffix.length);
    for (var overlap = maxOverlap; overlap > 0; overlap--) {
      if (output.slice(-overlap) === suffix.slice(0, overlap)) {
        return output + suffix.slice(overlap);
      }
    }
    return output + (output.endsWith("\n") || suffix.startsWith("\n") ? "" : "\n") + suffix;
  }

  function normalizeTerminalTail(tail, prefix) {
    if (!tail) return "";
    return mergeTerminalChunk(
      "",
      tail,
      { pendingCR: false, pendingAnsi: "" },
      prefix || ""
    );
  }

  function reconcileBackgroundTerminalOutput(previous, payload) {
    var output = String(previous == null ? "" : previous);
    output = mergeTerminalTail(output, normalizeTerminalTail(payload.stdout_tail, ""));
    output = mergeTerminalTail(output, normalizeTerminalTail(payload.stderr_tail, "[STDERR] "));
    return output;
  }

  // Live shell output is display-only. The completed tool result remains the
  // authoritative value written to conversation history/model context.
  function appendToolItemOutput(toolId, content, stream) {
    var chunk = typeof content === "string" ? content : String(content == null ? "" : content);
    if (!chunk) return false;
    for (var i = 0; i < state.chatItems.length; i++) {
      var item = state.chatItems[i];
      if (item.type !== "tool" || item.toolId !== toolId) continue;
      var parserState = terminalParserState(item, stream);
      var output = mergeTerminalChunk(
        item.output,
        chunk,
        parserState,
        stream === "stderr" ? "[STDERR] " : ""
      );
      // A verbose long-running process must not grow renderer memory without
      // bound. Completion replaces this tail with the normal full result.
      var maxLiveChars = 128 * 1024;
      if (output.length > maxLiveChars) output = "…\n" + output.slice(-maxLiveChars);
      item.output = output;
      item.liveOutput = true;
      return true;
    }
    return false;
  }

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

  // ── 产物跟踪 ─────────────────────────────────────────────────────
  function basename(p) {
    if (!p) return "";
    var parts = String(p).split(/[\\/]/);
    return parts[parts.length - 1] || p;
  }
  function isAbsPath(p) {
    return typeof p === "string" && (p.charAt(0) === "/" || /^[A-Za-z]:[\\/]/.test(p));
  }
  function normalizedPath(p) {
    return String(p || "").replace(/\\/g, "/");
  }
  function noteArtifactChange(path, event, sessionId) {
    if (!path) return;
    state.artifactChange = {
      seq: (state.artifactChange && state.artifactChange.seq || 0) + 1,
      path: path,
      event: event || "modified",
      sessionId: sessionId || "",
      at: Date.now(),
    };
    notify();
  }
  function isSharedMcpArtifactPath(path) {
    return normalizedPath(path).indexOf("/sessions/default/artifacts/") >= 0;
  }
  function artifactBelongsToSession(path, sid) {
    if (!path || !sid) return false;
    if (!isAbsPath(path)) return true;
    if (isSharedMcpArtifactPath(path)) return true;
    var normalized = normalizedPath(path);
    if (normalized.indexOf("/sessions/") >= 0) {
      return normalized.indexOf("/sessions/" + sid + "/workspace/") >= 0 ||
        normalized.indexOf("/sessions/" + sid + "/artifacts/") >= 0;
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
  var DELIVERABLE_EXTS = [
    "pptx", "ppt", "docx", "doc", "pdf", "html", "htm", "xlsx", "xls",
    "md", "csv", "png", "jpg", "jpeg", "svg", "gif", "webp", "zip",
  ];
  function isDeliverable(path) {
    var ext = (String(path || "").split(".").pop() || "").toLowerCase();
    return DELIVERABLE_EXTS.indexOf(ext) >= 0;
  }
  function trackArtifact(path) {
    if (!path) return;
    var bn = basename(path);
    for (var i = 0; i < state.artifacts.length; i++) {
      if (basename(state.artifacts[i].path) === bn) {
        // 已有同名:write_file 跟踪的是相对路径、disk watcher 推的是绝对路径——同一文件
        // 两种 path 会重复。新 path 绝对而旧的相对则用绝对替换(open 可靠),否则忽略重复。
        if (isAbsPath(path) && !isAbsPath(state.artifacts[i].path)) {
          state.artifacts[i] = { path: path, basename: bn };
          notify();
        }
        return;
      }
    }
    state.artifacts.push({ path: path, basename: bn });
    notify();
  }
  function markTurnDirtyArtifact(path) {
    var bn = basename(path);
    if (!bn) return;
    if ((state.turnDirtyArtifacts || []).some(function (p) { return basename(p) === bn; })) return;
    state.turnDirtyArtifacts.push(path);
  }
  function untrackArtifact(path) {
    var before = state.artifacts.length;
    state.artifacts = state.artifacts.filter(function (a) { return a.path !== path; });
    if (state.artifacts.length !== before) notify();
  }
  // 自动续卡支撑:这个文件之前是否被 present_artifact 展示过(同 basename)。
  // 已 present 过 = 用户已确认是成品,后续 write_file/append_file 修改它就自动
  // 再弹一张成品卡 —— 不靠 agent 第二次主动调(Qwen3.6 迭代后常漏)。信息直接
  // 从 chatItems 里的成品卡推导,无需单独 per-session map(chatItems 已按 session
  // 隔离 + rerender 重建)。返回最近一张同名成品卡(取 title/description 复用)。
  function findPresentedArtifact(path) {
    var bn = basename(path);
    if (!bn) return null;
    for (var i = state.chatItems.length - 1; i >= 0; i--) {
      var it = state.chatItems[i];
      if (it.type === "artifact_card" && basename(it.path) === bn) return it;
    }
    return null;
  }
  // 切换 session 时对账:扫 workspace 磁盘,把实际存在、但跟踪列表里没有的文件补进来。
  // 修「文件已生成在盘上、却因 app 中途重启/跟踪遗漏而不在产物面板」(以磁盘为准)。
  async function reconcileArtifacts(sid) {
    if (!sid) return;
    if (isScheduledRunSession(sid)) return;
    try {
      var files = await invoke("list_workspace_files", { sessionId: sid });
      if (sid !== state.activeSessionId) return; // 已切走,放弃(避免写错 session)
      var byName = {};
      state.artifacts.forEach(function (a) { byName[basename(a.path)] = a; });
      var added = false;
      files.forEach(function (p) {
        var bn = basename(p);
        var ex = byName[bn];
        // 已 present_artifact 过的成品在 saved.artifacts(ex 命中);扫盘只「新增」成品型文件,
        // 不再把所有过程文件全扫进面板(修「飞书 CLI scratch 全暴露成产物」)。
        if (!ex) {
          if (!isDeliverable(p)) return;
          var na = { path: p, basename: bn }; state.artifacts.push(na); byName[bn] = na; added = true;
        }
        else if (isAbsPath(p) && !isAbsPath(ex.path)) { ex.path = p; added = true; } // 相对→绝对,open 可靠
      });
      if (added) {
        notify();
        try { await invoke("save_session_artifacts", { id: sid, paths: state.artifacts.map(function (a) { return a.path; }) }); } catch (_) {}
      }
    } catch (e) { /* workspace 不存在(新 session)等,忽略 */ }
  }
  // write_file / append_file 的 args 里提取产物路径
  function extractArtifactPath(args) {
    if (!args) return null;
    if (typeof args === "string") {
      try { args = JSON.parse(args); } catch (e) { return null; }
    }
    return args.path || args.file_path || args.filename || null;
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

  // ── Send message ─────────────────────────────────────────────────
  // 指定 session 是否正在生成(active 看工作集 busy,后台看其 buffer)。
  function isBusyFor(sid) {
    return sid === state.activeSessionId ? state.busy : !!(sessionStates[sid] && sessionStates[sid].busy);
  }
  // 真正发送:在 sid 的工作集上加 user 气泡 + 流式占位 + busy,然后 invoke chat。
  // active/后台通用(后台走 runSyncOnSession 临时切工作集)。
  function doSendFor(sid, text, displayText, attachmentsPayload, meta, restrictTools) {
    safeConsoleInfo("[pinvou3][chat-ui] send start", {
      sid: sid,
      textLen: (text || "").length,
      attachments: attachmentsPayload ? attachmentsPayload.length : 0,
    });
    turnUsageDirty[sid] = false; // 新一轮开始，重置口径保护
    runSyncOnSession(sid, function () {
      var uitem = { type: "user", text: displayText, time: timeStr() };
      if (meta && meta.pinvouTransfer) uitem.pinvouTransfer = meta.pinvouTransfer; // 仅展示层,不进 messages/LLM
      addChatItem(uitem);
      state.messages.push({ role: "user", content: [{ type: "text", text: displayText }] });
      state.busy = true;
      startThinking();
      currentStreamText = "";
      currentStreamId = ++itemIdSeq;
      state.chatItems.push({ id: currentStreamId, type: "assistant", html: "", time: timeStr(), streaming: true });
    });
    notify();
    publishRemoteUserMessage(sid, displayText, meta && meta.remoteClientMessageId);
    return invoke("chat", { message: text, attachments: attachmentsPayload, sessionId: sid, restrictTools: !!restrictTools })
      .catch(function (err) {
        console.warn("[pinvou3][chat-ui] send failed", {
          sid: sid,
          error: err && err.toString ? err.toString() : err,
        });
        runSyncOnSession(sid, function () {
          addSystemItem("⚠️ " + (err && err.toString ? err.toString() : err));
          state.busy = false;
          state.chatItems = state.chatItems.filter(function (item) { return item.id !== currentStreamId || item.html; });
        });
        notify();
        flushQueued(sid);
      });
  }
  function publishRemoteUserMessage(sid, content, clientMessageId) {
    if (!sid || !content) return;
    invoke("remote_control_publish_user_message", {
      sessionId: sid,
      content: content,
      clientMessageId: clientMessageId || null,
    }).catch(function () { /* 没开远控时静默跳过 */ });
  }
  // 本轮跑完(或被停止)后,若该 session 不忙且有排队消息 → 把【整个队列】合并成一条
  // 一次性发出(Claude 式:排队的全部一起扔进下一轮,而不是一条条串行)。
  function flushQueued(sid) {
    if (isBusyFor(sid)) return;            // doFinal 等又起了新 turn → 留给那轮的 done 再 flush
    var q = sid === state.activeSessionId ? state.queued : (sessionStates[sid] && sessionStates[sid].queued);
    if (!q || q.length === 0) return;
    var items = q.splice(0, q.length);
    // 发给模型用 \n\n 分隔(让它清楚是几条独立消息);气泡显示用单换行 \n(紧凑,不空行)
    var text = items.map(function (i) { return i.text; }).filter(Boolean).join("\n\n");
    var displayText = items.map(function (i) { return i.displayText; }).filter(Boolean).join("\n");
    var attachments = [];
    items.forEach(function (i) { if (i.attachments && i.attachments.length) attachments = attachments.concat(i.attachments); });
    var meta = items.length === 1 ? items[0].meta : null; // 单条(如转交)保留 meta;合并多条不标
    var restrictTools = items.some(function (i) { return !!i.restrictTools; });
    notify();
    doSendFor(sid, text, displayText, attachments, meta, restrictTools);
  }

  async function sendMessage(text, meta) {
    text = (text || "").trim();
    var readyAttachments = state.attachments.filter(function (a) { return a.status === "ready" && a.result; });
    if (!text && readyAttachments.length === 0) return;
    // 还有解析中的附件 → 等
    if (state.attachments.some(function (a) { return a.status === "parsing"; })) {
      addSystemItem(bt("attachStillParsing"));
      return;
    }

    if (!state.activeSessionId) {
      await ensureSession(); // 草稿态首条消息 → 物化 session(命名靠下方 persistSession auto-title)
      if (!state.activeSessionId) return;
    }
    // 定时任务引导:引导词只拼进发给模型的 payload,气泡/历史仍只显示用户输入。
    var payloadText = text;
    var restrictTools = false;
    if (state.scheduledTaskPendingGuide) {
      payloadText = state.scheduledTaskPendingGuide + "\n\n" + text;
      restrictTools = true;
      state.scheduledTaskPendingGuide = null;
      state.scheduledTaskCreationSessionId = state.activeSessionId;
      state.scheduledTaskDraft = null;
      notify();
    }

    // 新一轮用户消息 → 先熄灭技能标；本轮若模型再 load_skill 会重新点亮（sticky-ish 生命周期）。
    state.activeSkill = null;

    // 展示文本：把附件 chip 名附在用户消息末尾
    var displayText = readyAttachments.length > 0
      ? text + (text ? "\n\n" : "") + "📎 " + readyAttachments.map(function (a) { return a.basename; }).join(" · ")
      : text;
    var attachmentsPayload = readyAttachments.map(function (a) { return a.result; });
    clearAttachments();

    // 排队式:当前 session 正在生成 → 这句进队列(不打断当前轮),本轮 chat:done 后自动发。
    // 输入框上方显示待发 chip(可✕撤销)。停止按钮仍只硬打断当前轮。
    if (state.busy) {
      state.queued.push({ id: ++itemIdSeq, text: payloadText, displayText: displayText, attachments: attachmentsPayload, meta: meta || null, restrictTools: restrictTools });
      notify();
      return;
    }

    await doSendFor(state.activeSessionId, payloadText, displayText, attachmentsPayload, meta, restrictTools);
  }
  function prefillComposer(text) {
    state.composerPrefill = { id: (state.composerPrefill.id || 0) + 1, text: String(text || "") };
    notify();
  }
  // 撤销一条待发消息(点 chip 的 ✕)。
  function removeQueued(id) {
    state.queued = state.queued.filter(function (q) { return q.id !== id; });
    notify();
  }

  // ── Pinvou v4 召唤式检阅:Boss 主动呼叫,审当前 session 前面的工作 ──
  // 设计 docs/品悟v4-常驻检阅助手设计.md。纯召唤、不替 Boss 决策。
  // 审查卡进 chatItems(当前会话可见);跨会话持久化(进 messages/独立存储)是 §6 后续增强。
  async function summonPinvou(focus, mode) {
    if (!state.activeSessionId) { addSystemItem("先开始一个对话,再召唤 Pinvou 检阅。"); return; }
    if (state.pinvouSummoning) return;
    state.pinvouSummoning = true;
    var sid = state.activeSessionId; // 召唤发起时的 session;await 返回后校验,防跨 session 串(召唤慢+切走)
    // 检阅结果弹 modal(不进对话流):一次只一个,裁决/跳过直接操作 state.pinvouModal.review、
    // 不靠 pos 定位(根治连续召唤 pos 重复串卡)。
    state.pinvouModal = { loading: true, coverage: mode === "coverage" };
    notify();
    try {
      // focus=产出物 path(品=审产物); mode="coverage"=悟(通盘体检)。
      var review = await invoke("summon_pinvou", { sessionId: sid, focus: focus || null, mode: mode || null });
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
    var pos = state.messages.length;
    state.pinvouReviews.push({ pos: pos, review: review });
    var sid = state.activeSessionId;
    var snapshot = JSON.parse(JSON.stringify(state.pinvouReviews));
    invoke("save_session_pinvou_reviews", { sessionId: sid, reviews: snapshot }).catch(function () {});
    return pos; // 供卡片记 reviewPos,裁决时按 pos 定位原 state 写 resolution
  }

  // §2 按勾选裁决:resolution 已由前端写回 review 对象(引用→sidecar),这里持久化 +
  // 把勾「让AI改」的条目走 B1 发定向修订指令(只改对应段落、禁全文重写)。Boss 驾驶,非自动。
  async function resolvePinvouReview(resolutions, actions) {
    // 弹窗只一个 review(state.pinvouModal.review),直接在它上面写 resolution——不靠 pos 定位
    // (根治连续召唤 pos 重复串卡)。它和 sidecar entry.review 同引用,写它=写 sidecar。
    var isWu = !!(state.pinvouModal && state.pinvouModal.coverage); // 关窗前取,供转交标品/悟
    var review = state.pinvouModal && state.pinvouModal.review;
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
    var fix = actions.filter(function (a) { return a.t === "fix"; });
    var verify = actions.filter(function (a) { return a.t === "verify"; });
    var adopt = actions.filter(function (a) { return a.t === "adopt"; });
    var ask = actions.filter(function (a) { return a.t === "ask"; });
    var parts = [];
    if (fix.length) {
      parts.push("请按下面的检阅意见，**只定向修改对应段落，不要全文重写**：");
      fix.forEach(function (a) { parts.push("- " + a.text); });
    }
    if (verify.length) {
      if (parts.length) parts.push("");
      parts.push("以下几条涉及外部事实，**先查证再改、标明依据，别凭记忆直接改**：");
      verify.forEach(function (a) { parts.push("- " + a.text); });
    }
    if (adopt.length) {
      if (parts.length) parts.push("");
      parts.push("以下事项我已拍板，按此更新产物：");
      adopt.forEach(function (a) { parts.push("- " + (a.topic ? a.topic + "：" : "") + a.pick); });
    }
    if (ask.length) {
      if (parts.length) parts.push("");
      parts.push("以下待定项请用 request_user_input 正式问我，别自己猜：");
      ask.forEach(function (a) { parts.push("- " + a.topic); });
    }
    var fill = actions.filter(function (a) { return a.t === "fill"; });
    if (fill.length) {
      if (parts.length) parts.push("");
      parts.push("以下维度产物还缺，请补充进去（保留其余、只增不改）：");
      fill.forEach(function (a) { parts.push("- " + a.dimension + (a.suggestion ? "：" + a.suggestion : "")); });
      parts.push("（涉及外部事实的，先查证再写、标依据，别凭记忆编。）");
    }
    if (parts.length) sendMessage(parts.join("\n"), { pinvouTransfer: isWu ? "悟" : "品" });
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
    var snapshot = JSON.parse(JSON.stringify(state.pinvouReviews));
    return invoke("save_session_pinvou_reviews", { sessionId: state.activeSessionId, reviews: snapshot }).catch(function () {});
  }

  async function cancelGeneration() {
    safeConsoleInfo("[pinvou3][chat-ui] cancel clicked", {
      sid: state.activeSessionId,
      busy: state.busy,
    });
    if (!state.busy) return;
    try {
      safeConsoleInfo("[pinvou3][chat-ui] cancel invoke start", { sid: state.activeSessionId });
      await invoke("cancel_generation", { sessionId: state.activeSessionId });
      safeConsoleInfo("[pinvou3][chat-ui] cancel invoke ok", { sid: state.activeSessionId });
    } catch (e) {
      console.warn("[pinvou3][chat-ui] cancel invoke failed", {
        sid: state.activeSessionId,
        error: e && e.toString ? e.toString() : e,
      });
      console.warn("cancel failed", e);
    }
  }

  async function cancelShellTask(sessionId, taskId) {
    if (!sessionId || !taskId) throw new Error("Missing shell task identity");
    return invoke("cancel_shell_task", { sessionId: sessionId, taskId: taskId });
  }

  // ── Persist messages ─────────────────────────────────────────────
  async function persistMessages() {
    if (!state.activeSessionId) return;
    if (isScheduledRunSession(state.activeSessionId)) return;
    try {
      await invoke("save_session_messages", { id: state.activeSessionId, messages: state.messages });
      // artifacts 一起落盘，重启/切换 session 后能恢复
      try { await invoke("save_session_artifacts", { id: state.activeSessionId, paths: state.artifacts.map(function (a) { return a.path; }) }); } catch (_) {}
      // Auto-title
      var meta = state.sessions.find(function (s) { return s.id === state.activeSessionId; });
      if (meta && (meta.title === "新对话" || meta.title === "New chat" || personaPlaceholderTitles[state.activeSessionId])) {
        var firstUser = state.messages.find(function (m) { return m.role === "user"; });
        var text = firstUser && firstUser.content && firstUser.content.find(function (c) { return c.type === "text"; });
        if (text && text.text) {
          var newTitle = text.text.slice(0, 20);
          await invoke("rename_session", { id: state.activeSessionId, title: newTitle });
          meta.title = newTitle;
          delete personaPlaceholderTitles[state.activeSessionId]; // 已被对话内容命名,卸下占位标记
        }
      }
    } catch (e) {
      console.warn("persist failed", e);
    }
  }

  // ── Event listeners ──────────────────────────────────────────────
  // 所有 chat:* 事件都带 session_id(后端 spawn_event_forwarder 打的 tag)。
  // onSessionEvent 按 session_id 把同步逻辑路由到对应 session 的工作集:active 直接跑,
  // 后台临时切工作集跑完再切回。下面每个监听器的 body 与旧单 session 版逐字一致,
  // 只是包了一层路由,所以 active session 行为零变化。
  listen("chat:delta", function (e) { onSessionEvent(e, function () {
    var text = e.payload && e.payload.text || "";
    pendingAssistantText += text;
    currentStreamText += text;
    // Update the streaming chat item
    var item = state.chatItems.find(function (it) { return it.id === currentStreamId; });
    if (item) {
      item.html = renderMarkdown(currentStreamText);
      item.streaming = true;
    } else {
      // New bubble needed (after tool card)
      currentStreamId = ++itemIdSeq;
      state.chatItems.push({
        id: currentStreamId,
        type: "assistant",
        html: renderMarkdown(currentStreamText),
        time: timeStr(),
        streaming: true,
      });
    }
    notify();
  }); });

  listen("scheduled_task:run_updated", function (e) {
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
      var raw = typeof toolResultContent === "string" ? toolResultContent : JSON.stringify(toolResultContent || {});
      var obj = JSON.parse(raw);
      if (obj && obj.content && obj.content[0] && typeof obj.content[0].text === "string") {
        try {
          var inner = JSON.parse(obj.content[0].text);
          if (inner && typeof inner === "object") return inner;
        } catch (_) {}
      }
      return obj;
    } catch (_) {
      return null;
    }
  }
  function artifactPathFromToolOutput(toolResultContent) {
    var obj = parseToolResultPayload(toolResultContent);
    if (!obj || typeof obj !== "object") return null;
    var p = obj.abs_path || obj.path || obj.file_path || obj.local_path;
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
    var parsed = artifactPathFromToolOutput(toolResultContent);
    if (parsed) return parsed;
    return fallbackPath;
  }

  listen("chat:tool_start", function (e) { onSessionEvent(e, function () {
    var p = e.payload || {};
    if (p.session_id) turnUsageDirty[p.session_id] = true; // 多请求轮，usage 累加值不可当占用
    toolMeta[p.id] = { name: p.name, args: p.args };
    thinkingTool(p.name);
    flushPendingTextBlock();
    pendingAssistantBlocks.push({ type: "tool_use", id: p.id, name: p.name, input: p.args || {} });

    // Finalize current streaming bubble
    var streamItem = state.chatItems.find(function (it) { return it.id === currentStreamId; });
    if (streamItem) {
      streamItem.streaming = false;
    }
    currentStreamText = "";
    currentStreamId = 0;

    // request_user_input：不渲染默认工具卡，等 chat:user_input_required 单独渲染选择卡片
    if (p.name === "request_user_input") { notify(); return; }

    // present_artifact：不渲染灰色工具卡，等 tool_end 成功时渲染成品卡
    if (isPresentArtifactTool(p.name)) { notify(); return; }

    // load_skill：模型加载技能 → 点亮 composer 技能标（内置自动技能"正在使用"指示），
    // 不渲染裸工具卡（用药丸指示器替代）。当前只识别视觉设计。
    if (p.name === "load_skill") {
      var skArg = ((p.args && (p.args.name || p.args.skill)) || "").toString();
      if (skArg.indexOf("视觉设计") >= 0 || skArg.toLowerCase().indexOf("visual-design") >= 0) {
        state.activeSkill = "visual-design";
      }
      // 不 return：照常出工具卡。卡内容在 tool_end / rerender 处脱敏成占位，
      // 展开看不到 SKILL.md 全文（防设计系统泄露），但保留"加载了技能"的痕迹。
    }

    // Add tool card
    addChatItem({
      type: "tool", toolId: p.id, name: p.name, args: p.args,
      output: null, success: null, state: "running",
      sessionId: p.session_id || state.activeSessionId,
    });
    notify();
  }); });

  listen("chat:tool_delta", function (e) { onSessionEvent(e, function () {
    var p = e.payload || {};
    appendToolItemOutput(p.id, p.content, p.stream);
    scheduleShellNotify();
  }); });

  listen("chat:tool_end", function (e) { onSessionEvent(e, function () {
    var p = e.payload || {};
    var meta = toolMeta[p.id];
    thinkingIdle();
    var resultContent = typeof p.output === "string" ? p.output : JSON.stringify(p.output);
    flushAssistantMessageToHistory();
    var trBlock = { type: "tool_result", tool_use_id: p.id, content: resultContent };
    if (!p.success) trBlock.is_error = true;
    state.messages.push({ role: "user", content: [trBlock] });

    var backgroundTaskId = p.metadata && p.metadata.backgrounded === true &&
      p.metadata.status === "Running" && p.metadata.task_id;
    if (meta && meta.name === "exec_shell" && backgroundTaskId) {
      markBackgroundToolItem(p.id, p.session_id, backgroundTaskId, p.output);
      delete toolMeta[p.id];
      currentStreamText = ""; currentStreamId = 0;
      notify();
      return;
    }

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
        var presentedPath = presentArtifactAbsPath(p.output, meta.args && meta.args.path);
        // 同一产物没改又 present 一次 → 跳过出卡(防模型啰嗦重复);改完再 present/续卡会保留。
        if (!isDuplicateArtifactCard(presentedPath)) {
          addChatItem({
            type: "artifact_card",
            path: presentedPath,
            title: (meta.args && meta.args.title) || "",
            description: (meta.args && meta.args.description) || "",
            time: timeStr(),
            sessionId: p.session_id || state.activeSessionId,
          });
        }
        if (presentedPath) state.turnPresentedArtifacts.push(presentedPath); // 本 turn 已出成品卡,chat:done 不再兜底补
        // 同步进产物面板:present_artifact 出卡的产物也算「产出物」。修「自己生成文件、
        // 不走 write_file 的工具(如 make_pptx)→ 卡有、面板无」。trackArtifact 已去重。
        if (presentedPath) trackArtifact(presentedPath);
        delete toolMeta[p.id];
        currentStreamText = ""; currentStreamId = 0;
        notify();
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
      var producedPath = artifactPathFromToolOutput(p.output);
      if (producedPath && isDeliverable(producedPath)) {
        trackArtifact(producedPath);
        markTurnDirtyArtifact(producedPath);
      }
    }

    // load_skill：卡照出，但不把返回的 SKILL.md 全文写进卡，展开只见占位（防设计系统泄露）。
    var outForCard = (meta && meta.name === "load_skill") ? "（技能已加载，内容不展示）" : p.output;
    updateToolItem(p.id, outForCard, p.success);

    // Careful hook：DeepSeek-TUI shell.rs 拦截 Dangerous → 红色拦截卡
    var md = p.metadata;
    if (md && md.safety_level === "dangerous" && md.blocked) {
      addChatItem({ type: "careful_blocked", args: meta && meta.args, metadata: md, time: timeStr() });
    }

    // write_file/append_file/edit_file 改了产物 → 记账,turn 结束(chat:done)统一补成品卡。
    // 改成记账+去重:AI 一个 turn 会 edit_file 改很多次,实时续会刷出一堆卡;且 edit_file
    // 之前不触发续卡 → 改完没新卡片 → 没法对改后产物再召唤 pinvou(核账闭环断裂)。
    if (p.success && meta && (meta.name === "write_file" || meta.name === "append_file" || meta.name === "edit_file")) {
      var ap = extractArtifactPath(meta.args);
      if (ap) {
        // 面板只收「成品」:成品型扩展名(自动当成品)或之前 present_artifact 过的文件;
        // 中间草稿(content_p1.txt / *_params.json 等)不进面板。edit_file 只改已有不新建。
        if (meta.name !== "edit_file" && (isDeliverable(ap) || findPresentedArtifact(ap))) trackArtifact(ap);
        // 产物(present 过的成品 或 write/append 写进产物列表的)被写/改 → turn 结束补卡。
        // 不再要求 present 过:AI 经常写完产物忘了 present_artifact → 没成品卡 = 没召唤入口。
        // 按 basename 比对:disk watcher(artifact:disk)写盘后抢先用**绝对**路径 trackArtifact
        // 占了名额,而这里 ap 是 write_file 的**相对**参数 —— 用 a.path===ap 比绝对≠相对永远落空,
        // turnDirty 收不到 → 实时不补成品卡(只能靠重启 rerender 才出)。basename 比对消除该竞态。
        var _apbn = basename(ap);
        var isArtifact = !!findPresentedArtifact(ap) || state.artifacts.some(function (a) { return basename(a.path) === _apbn; });
        if (isArtifact) markTurnDirtyArtifact(ap);
      }
    }

    // 兜底：Plan 模式下 AI 调了被白名单/sandbox 拦的工具 → 弹兜底卡，给两条出路
    if (!p.success && state.modeState.mode === "plan" && typeof p.output === "string" &&
        (p.output.includes("not available in the current tool catalog") ||
         p.output.includes("unavailable in Plan mode") ||
         p.output.includes("PermissionDenied"))) {
      if (!hasUnresolvedItem("plan_stuck")) {
        addChatItem({ type: "plan_stuck", toolName: meta && meta.name, resolved: false, time: timeStr() });
      }
    }

    delete toolMeta[p.id];
    currentStreamText = "";
    currentStreamId = 0;
    notify();
  }); });

  listen("chat:shell_task_status", function (e) { onSessionEvent(e, function () {
    var p = e.payload || {};
    finishBackgroundToolItem(p.tool_id, p);
    notify();
  }); });

  // chat:done 特殊:同步收尾(flush/busy=false/mode 复位)走 runSyncOnSession
  // 路由到对应 session;异步收尾(discard_plan/落盘/刷新列表)按显式 sid 路由,
  // 不依赖工作集 —— 这样后台 session 跑完也能正确落盘。
  listen("chat:done", function (e) {
    var sid = (e.payload && e.payload.session_id) || state.activeSessionId;
    safeConsoleInfo("[pinvou3][chat-ui] chat done event", {
      sid: sid,
      error: e.payload && e.payload.error || null,
    });
    if (isScheduledRunSession(sid)) markScheduledInitialTurnTerminal(sid);
    runSyncOnSession(sid, function () {
      var error = e.payload && e.payload.error;
      if (error) addSystemItem("⚠️ " + error);
      flushAssistantMessageToHistory();
      // 本 turn 写/改过的产物 → 末尾补一张成品卡(带召唤图标),让 Boss 就近召唤 pinvou。
      // present 过的复用其 title/desc;AI 没 present 的兜底用文件名补首卡(否则没召唤入口=这次的 bug)。
      // 本 turn 刚 present_artifact 出过卡的跳过,不重复。edit/append 改多次也只补一张。
      (state.turnDirtyArtifacts || []).forEach(function (ap) {
        // 按 basename 比对:present 存 server 绝对路径、turnDirty 存 write 相对路径,
        // 直接 indexOf 比不中 → present 过的文件会被兜底再补一张(重复)。
        var _apbn = basename(ap);
        if ((state.turnPresentedArtifacts || []).some(function (pp) { return basename(pp) === _apbn; })) return;
        var prev = findPresentedArtifact(ap);
        // 补卡 path 优先用 disk watcher 落进产物列表的同名**绝对**路径(open 可靠、跨 session 稳);
        // 没有再退回 write_file 的相对 ap(由 sessionId 兜底解析)。
        var tracked = state.artifacts.find(function (a) { return basename(a.path) === _apbn && isAbsPath(a.path); });
        var cardPath = (tracked && tracked.path) || ap;
        if (prev) addChatItem({ type: "artifact_card", path: prev.path, title: prev.title, description: prev.description, time: timeStr(), sessionId: sid });
        else addChatItem({ type: "artifact_card", path: cardPath, title: basename(ap), description: "", time: timeStr(), sessionId: sid });
      });
      state.turnDirtyArtifacts = [];
      state.turnPresentedArtifacts = [];
      // Finalize streaming bubble
      var streamItem = state.chatItems.find(function (it) { return it.id === currentStreamId; });
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
    notify();
    // 异步收尾(按 sid 路由,active/后台通用)
    (async function () {
      await persistMessagesFor(sid);
      await refreshHistoryList();
      notify();
      publishRemoteLiveSnapshot(sid).catch(function () {});
      // 排队式:本轮跑完,若该 session 不忙且有待发消息 → 自动发下一条
      flushQueued(sid);
    })();
  });

  listen("chat:usage", function (e) { onSessionEvent(e, function () {
    var sid = e.payload && e.payload.session_id;
    if (sid && turnUsageDirty[sid]) return; // 本轮多请求，累加值≠占用，保留上个准确值
    var input = Number(e.payload && e.payload.input_tokens || 0);
    if (input > 0) {
      state.tokens = { input: input, max: maxModelLen };
      notify();
    }
  }); });

  listen("chat:compaction", function (e) { onSessionEvent(e, function () {
    if (e.payload && e.payload.session_id) turnUsageDirty[e.payload.session_id] = true; // 压缩轮 usage 含摘要请求
    var phase = e.payload && e.payload.phase;
    var msg = e.payload && e.payload.message || "";
    var auto = e.payload && e.payload.auto ? bt("compactAuto") : "";
    var compactId = e.payload && e.payload.id;
    var before = Number(e.payload && e.payload.messages_before);
    var after = Number(e.payload && e.payload.messages_after);
    var looksLikePruneOnly = /0 removed|messages unchanged|tool results pruned/i.test(msg);
    var pruneOnlyAuto = !!(e.payload && e.payload.auto) &&
      phase === "done" &&
      Number.isFinite(before) &&
      Number.isFinite(after) &&
      before === after &&
      looksLikePruneOnly &&
      msg.indexOf("Emergency compaction") !== 0;
    if (phase === "start") addSystemItem(bt("compactStart") + auto + " " + msg, { compactId: compactId, compactPhase: "start" });
    else if (phase === "done" && pruneOnlyAuto) addOrMergePruneCompaction(compactId);
    else if (phase === "done") addSystemItem(bt("compactDone") + auto + " " + msg);
    else if (phase === "fail") addSystemItem(bt("compactFail") + auto + ": " + msg);
  }); });

  // ── request_user_input：渲染选择卡片（不进 messages.json）─────────
  // payload: { id: tool_call_id, questions: [{header, id, question, options:[{label, description}]}] }
  listen("chat:user_input_required", function (e) { onSessionEvent(e, function () {
    var p = e.payload || {};
    if (state.workflow.run.status === "stopped" &&
        state.workflow.run.sessionId && p.session_id === state.workflow.run.sessionId) return;
    var questions = p.questions || [];
    if (!Array.isArray(questions) || questions.length === 0) return;
    addChatItem({
      type: "user_input", toolCallId: p.id, questions: questions,
      resolved: false, cardState: "active", time: timeStr(),
    });
    notify();
  }); });

  // 可恢复的瞬态错误（SSE idle timeout / 瞬态工具失败）：turn 没结束，引擎会 retry，
  // 绝不 setBusy(false)，只飘一条 ⚠️ 提示。
  listen("chat:transient_error", function (e) { onSessionEvent(e, function () {
    if (e.payload && e.payload.session_id) turnUsageDirty[e.payload.session_id] = true; // 重试轮 usage 含重发请求
    var error = e.payload && e.payload.error;
    if (error) addSystemItem("⚠️ " + error);
  }); });

  // File watcher 推送的产物事件：session workspace 下新文件/修改/删除。
  // 路由到对应 session 的产物列表(后台 session 的产物也跟踪)。
  listen("artifact:disk", function (e) {
    var p = e.payload || {};
    if (!p.path) return;
    onSessionEvent(e, function () {
      noteArtifactChange(p.path, p.event || "modified", p.session_id || state.activeSessionId || "");
      if (p.event === "removed") { untrackArtifact(p.path); return; }
      // 面板只收成品:成品型扩展名 或 present_artifact 过的;中间 / infra / 目录不进面板
      // (file_watcher 递归会推 tmp/ _state/ 等子目录与 infra 文件 → 此处兜住)。
      if (isDeliverable(p.path) || findPresentedArtifact(p.path)) trackArtifact(p.path);
    });
  });

  listen("remote_control:mobile_user_message", async function (e) {
    var p = e.payload || {};
    var sid = p.session_id;
    var content = (p.content || "").trim();
    if (!sid || !content) return;
    try { await ensureSessionBufferLoaded(sid); }
    catch (err) {
      console.warn("remote session hydrate failed", err);
      return;
    }
    if (isBusyFor(sid)) {
      runSyncOnSession(sid, function () {
        state.queued.push({
          id: ++itemIdSeq,
          text: content,
          displayText: content,
          attachments: [],
          meta: { remoteClientMessageId: p.client_message_id || null },
        });
      });
      notify();
      return;
    }
    doSendFor(sid, content, content, [], { remoteClientMessageId: p.client_message_id || null });
  });

  // 本地语音识别依赖安装进度（模型下载 / ffmpeg 安装）
  listen("voice_asr:progress", function (e) {
    var p = e && e.payload;
    if (!p) return;
    state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, { progress: p });
    notify();
  });

  // vllm-setup:phase —— MegaCube 本地大模型引导阶段(authorizing→waiting{attempt}→ready),驱动引导框步骤指示。
  listen("vllm-setup:phase", function (e) {
    var p = e.payload || {};
    if (!p.phase) return;
    state.vllmSetupPhase = p.phase;
    if (typeof p.attempt === "number") state.vllmSetupAttempt = p.attempt;
    notify();
  });

  // 知识库 embedding 模型下载进度（download → verify → extract → done）
  listen("kb_model:progress", function (e) {
    var p = e && e.payload;
    if (!p) return;
    state.kbModelSetup = Object.assign({}, state.kbModelSetup, { progress: p });
    notify();
  });

  // chat:plan_snapshot —— update_plan/checklist_write 后实时更新进度，与 plan_ready 解耦
  listen("chat:plan_snapshot", function (e) { onSessionEvent(e, function () {
    var p = e.payload || {};
    if (p.plan_snapshot) state.planSnapshot.plan = p.plan_snapshot;
    if (p.todos_snapshot) state.planSnapshot.todos = p.todos_snapshot;
    notify();
  }); });

  // chat:plan_ready —— 底座式:Plan 模式调过 update_plan 即弹方案卡(快照非空)
  listen("chat:plan_ready", function (e) { onSessionEvent(e, function () {
    var p = e.payload || {};
    // 新方案出现 → 旧的 active 方案卡冻结
    state.chatItems.forEach(function (it) {
      if (it.type === "plan_card" && it.cardState === "active") {
        it.cardState = "frozen"; it.statusLabel = bt("planSuperseded");
      }
    });
    var snaps = { plan: p.plan_snapshot || null, todos: p.todos_snapshot || null };
    addChatItem({
      type: "plan_card", plan: snaps.plan, todos: snaps.todos,
      planMarkdown: composePlanMarkdown(snaps), cardState: "active", statusLabel: "", time: timeStr(),
    });
    notify();
  }); });

  // workflow:project_started —— start_workflow 后端建项目+绑定 session 后 emit。
  // 必须真正 switchToSession 切过去（load 新 session 的空 messages + sync engine +
  // syncSessionSkill），否则只设 activeSessionId 会让旧对话的 messages 残留在屏上，
  // 顶部又叠加 PhaseChips，看起来像"旧对话被 append 了项目名"（Phase A 关键 bug）。
  // refreshHistoryList 先跑让新 session 进 sidebar 列表 + 刷 bindings(🧭)。
  // switchToSession 内部已调 syncSessionSkill，切完 App useEffect 自动 setCurrentView('chat')。
  // [卡片流] start_workflow 后端建项目+绑定 session 后 emit。
  // 新设计：**不再 switchToSession 跳聊天页** —— 用户停在工作流看板，
  // 工作流 session 作为后台 session 跑，看板靠下面的 workflow:* 事件按 session_id 驱动。
  listen("workflow:project_started", async function (e) {
    var p = e.payload || {};
    state.workflow.run = {
      active: true, sessionId: p.session_id || null, projectDir: p.project_dir || null,
      scenario: p.scenario || null, status: "running", agents: {}, cards: [], selectedRole: null,
    };
    await refreshHistoryList();
    notify();
  });

  // ── 卡片流工作流：运行态 helper ──────────────────────────────────
  // 事件只认本次 run 的 session（payload 无 session_id 时放行，兼容）。
  function isRunSession(p) {
    var s = state.workflow.run.sessionId;
    return !p || !p.session_id || !s || p.session_id === s;
  }
  function applyAgentPatch(roleId, patch) {
    if (!roleId) return;
    var agents = state.workflow.run.agents;
    agents[roleId] = Object.assign(agents[roleId] || { id: roleId }, patch);
  }
  function markWorkflowRunStopped() {
    state.workflow.run.status = "stopped";
    Object.keys(state.workflow.run.agents || {}).forEach(function (id) {
      var agent = state.workflow.run.agents[id];
      if (agent && (agent.status === "running" || agent.status === "reviewing" || agent.status === "briefing")) {
        agent.status = "stopped";
      }
    });
    (state.workflow.run.cards || []).forEach(function (card) {
      if (!card.resolved) { card.resolved = true; card.cardState = "cancelled"; }
    });
  }
  function mergeFullState(p) {
    var run = state.workflow.run;
    if (p.project_dir) run.projectDir = p.project_dir;
    if (p.scenario) run.scenario = p.scenario;
    // [工作流分离] 后端按 scenario 解析出 workflow_id + workflow.json 的 ui 块,
    // 前端泳道/表单/标题/奏折全按 run.ui 渲染(不再硬编码各工作流)。
    if (p.workflow_id) run.workflowId = p.workflow_id;
    if (p.ui) run.ui = p.ui;
    var roles = p.roles || {};
    // [B2 修] full_state 是权威全量快照:快照里没有的角色条目要删——尚书省派单后
    // 静态六部被差事节点取代,留着陈旧条目会让泳道误判"六部在场"而不插差事批次泳道
    // (实测:六部卡全员显示"等待尚书省交付",而差事其实已经在跑)。
    Object.keys(state.workflow.run.agents).forEach(function (rid) {
      if (!(rid in roles)) delete state.workflow.run.agents[rid];
    });
    Object.keys(roles).forEach(function (rid) {
      var r = roles[rid] || {};
      applyAgentPatch(rid, {
        id: rid, name: r.name || rid, status: r.status || "pending",
        last_gate_verdict: r.last_gate_verdict || null,
        outputs_present: r.outputs_present || 0,
        last_run_ts: r.last_run_ts || null,
        depends_on: r.depends_on || [],
        wave: r.wave, bu: r.bu,   // [B2 E1] 差事分层 + 取头像/配色
      });
    });
    // stop marker 是最终状态。迟到或手动刷新的 full_state 仍可能携带盘上旧的
    // running/reviewing 状态，不能让已停止的角色卡回跳成“执行中”。
    if (p.stopped) markWorkflowRunStopped();
    else if (p.all_completed) run.status = "complete";
  }
  // [2026-06-06] 快照恢复：把前端 run 态挂回一个已存在的工作流 run（app 重启/切会话后）。
  // 拉后端快照(get_workflow_state) → 点亮 run 态(复刻 project_started 结构) → mergeFullState
  // 填角色 → 看板和「🔄 重跑」按钮全部恢复。非工作流会话(无 roles)返回 false、无副作用。
  async function attachRun(sessionId, staleRunning) {
    try {
      var snap = await invoke("get_workflow_state", { sessionId: sessionId });
      if (!snap || !snap.roles || Object.keys(snap.roles).length === 0) return false;
      state.workflow.run = {
        active: true, sessionId: sessionId, projectDir: snap.project_dir || null,
        scenario: snap.scenario || null,
        status: snap.stopped ? "stopped" : (snap.all_completed ? "complete" : "running"),
        agents: {}, cards: [], selectedRole: null,
      };
      mergeFullState(snap);
      // [恢复] app 重启后,盘上记 running/reviewing 的角色其 SubAgent 已随重启死亡 →
      // 标记 stale(需要重跑),卡片才显示「🔄 重跑」按钮;否则卡在"在工作"无出口。
      // staleRunning 只在启动恢复(resumeWorkflowOnBoot)传 true;app 内切活跃 run 不传。
      if (staleRunning) {
        var ag = state.workflow.run.agents;
        Object.keys(ag).forEach(function (rid) {
          var s = ag[rid] && ag[rid].status;
          if (s === "running" || s === "reviewing" || s === "briefing") ag[rid].status = "stale";
        });
      }
      notify();
      return true;
    } catch (e) { console.warn("attachRun failed", e); return false; }
  }
  // app 启动后自动恢复最近一个进行中的工作流 run（后端扫 binding 找）。
  async function resumeWorkflowOnBoot() {
    try {
      var r = await invoke("find_resumable_run");
      if (r && r.session_id) {
        // [方案A] 不再 switchToSession 劫持聊天会话——启动恒落干净草稿页。
        // 只把工作流看板挂回(attachRun 填 state.workflow.run,不动 activeSessionId
        // 也不切 currentView),用户主动切「工作流」tab 才看到那个 run。
        await attachRun(r.session_id, true); // 僵死 running → stale,露出重跑按钮
      }
    } catch (e) { console.warn("resumeWorkflowOnBoot failed", e); }
  }
  function pushRunCard(card) { card.cardId = ++itemIdSeq; state.workflow.run.cards.push(card); }
  function resolveRunCard(cardId, cardState) {
    state.workflow.run.cards.forEach(function (c) { if (c.cardId === cardId) { c.resolved = true; c.cardState = cardState; } });
    notify();
  }
  // 按角色 resolve 所有未处理的 gate 卡(去重 + 兜底:即便 cardId 对不上也清干净)。
  function resolveRunCardsForRole(roleId, cardState) {
    if (!roleId) return;
    state.workflow.run.cards.forEach(function (c) {
      if (c.kind === "gate" && c.roleId === roleId && !c.resolved) { c.resolved = true; c.cardState = cardState; }
    });
  }
  // 批准/打回后拉一次后端真实状态,把看板角色态(gate_waiting→completed/running)刷正确。
  async function refreshRunState() {
    try {
      var sid = state.workflow.run.sessionId; if (!sid) return;
      var snap = await invoke("get_workflow_state", { sessionId: sid });
      if (snap && snap.roles) mergeFullState(snap);
    } catch (e) { console.warn("refreshRunState failed", e); }
  }

  // ── 卡片流工作流：事件监听 ───────────────────────────────────────
  listen("workflow:full_state", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    mergeFullState(p); notify();
  });
  listen("workflow:agent_state_changed", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    if (state.workflow.run.status === "stopped") return;
    applyAgentPatch(p.role_id, { name: p.role_name || p.role_id, status: p.status || "running" });
    notify();
  });
  // [per_page] fan-out 逐页状态 → 工作流界面把该节点展开成 N 个 SubAgent chip。
  // payload: { base_role, pages:[{page,status}] }，status ∈ queued|running|done|retrying。
  listen("workflow:fanout", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    if (state.workflow.run.status === "stopped") return;
    if (!state.workflow.run.fanout) state.workflow.run.fanout = {};
    var pages = p.pages || [];
    state.workflow.run.fanout[p.base_role] = { total: pages.length, pages: pages };
    notify();
  });
  listen("workflow:complete", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    if (state.workflow.run.status === "stopped") return;
    state.workflow.run.status = "complete";
    // [edict-obs] 后端带回成品路径 → 弹成品卡(一键打开 deck)
    if (p.artifact) {
      pushRunCard({ kind: "artifact", path: p.artifact, text: "🎉 工作流完成，成品已生成", resolved: false });
    }
    notify();
  });
  listen("workflow:blocked", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    if (state.workflow.run.status === "stopped") return;
    state.workflow.run.status = "blocked";
    // 后端 emit 的是 message(+warmup_report)，不是 reason/waiting_roles。
    pushRunCard({ kind: "system", text: "⚙️ 工作流卡住：" + (p.message || p.reason || "未知原因"), resolved: false });
    notify();
  });
  listen("workflow:stopped", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    markWorkflowRunStopped();
    notify();
  });
  listen("workflow:gate_approval", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    if (state.workflow.run.status === "stopped") return;
    var findings = p.findings || (p.gate_description ? [p.gate_description] : []);
    // 去重:同一角色已有未处理的 gate 卡 → 只更新 findings,不叠新卡(huizou 反复过闸会重复 emit)。
    var dup = (state.workflow.run.cards || []).find(function (c) { return c.kind === "gate" && c.roleId === p.role_id && !c.resolved; });
    if (dup) { dup.findings = findings; notify(); return; }
    // 后端 emit 的是 gate_description(单串)，不是 findings —— 兜底收进 findings。
    pushRunCard({ kind: "gate", roleId: p.role_id, roleName: p.role_name || p.role_id, findings: findings, resolved: false });
    notify();
  });
  // [edict-obs] SubAgent 实时进展(底座每步/每个工具调用自动发)。
  listen("workflow:agent_progress", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    if (state.workflow.run.status === "stopped") return;
    if (!state.workflow.run.progress) state.workflow.run.progress = {};
    var key = p.role_id || p.agent_id;
    if (!key) return;
    // per_page 成员 "<role>#p01" 归并到基础节点显示
    var base = key.indexOf("#") > -1 ? key.split("#")[0] : key;
    state.workflow.run.progress[base] = p.status || "";
    notify();
  });
  // [edict-obs] per-role token 账本快照(每次 LLM 调用后推一次累计值)。
  listen("workflow:token_usage", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    if (state.workflow.run.status === "stopped") return;
    if (!state.workflow.run.tokens) state.workflow.run.tokens = {};
    var key = p.role_id || p.agent_id;
    if (!key) return;
    var base = key.indexOf("#") > -1 ? key.split("#")[0] : key;
    state.workflow.run.tokens[base] = {
      input: p.input_tokens_total || 0, output: p.output_tokens_total || 0, calls: p.calls || 0,
    };
    notify();
  });
  // request_user_input：本次 run 的 session 弹问答卡到看板底部交互区
  // （与上面 chat:user_input_required 的 chat 渲染并存，互不影响——工作流页看 run.cards）。
  listen("chat:user_input_required", function (e) {
    var p = e.payload || {};
    if (!state.workflow.run.sessionId || p.session_id !== state.workflow.run.sessionId) return;
    if (state.workflow.run.status === "stopped") return;
    var qs = p.questions || []; if (!Array.isArray(qs) || !qs.length) return;
    pushRunCard({ kind: "user_input", toolCallId: p.id, questions: qs, resolved: false });
    notify();
  });

  // ── Monitor ──────────────────────────────────────────────────────
  function fmtMiB(mib) {
    if (mib == null) return "—";
    return mib >= 1024 ? (mib / 1024).toFixed(1) + " GB" : mib + " MB";
  }
  function fmtKiB(kib) {
    if (kib == null) return "—";
    if (kib >= 1024 * 1024) return (kib / 1024 / 1024).toFixed(1) + " GB";
    if (kib >= 1024) return (kib / 1024).toFixed(0) + " MB";
    return kib + " KB";
  }
  function fmtDuration(secs) {
    if (secs == null || secs < 0) return "—";
    var h = Math.floor(secs / 3600), m = Math.floor((secs % 3600) / 60);
    if (h > 0) return h + "h " + m + "m";
    if (m > 0) return m + "m " + (secs % 60) + "s";
    return secs + "s";
  }
  function fmtTok(n) {
    if (n == null) return "—";
    if (n >= 1e6) return (n / 1e6).toFixed(1) + "M";
    if (n >= 1e3) return (n / 1e3).toFixed(1) + "k";
    return String(Math.round(n));
  }

  function numOr0(x) { return (typeof x === "number" && isFinite(x)) ? x : 0; }

  // 用基准点把累计 counter 换算成「自清除以来」的区间值。sp=app 自测(snap.self_perf,
  // TTFT/TPS/tokens 全从这);v=vllm(仅 KV 的本地 prefix_cache 分支要它)。无基准 → 直接
  // 用进程生命周期累计值。任一 counter 倒退（< 基准：app 或 vLLM 重启、counter 归零）
  // → 丢弃失效基准，回落到累计值，避免负数。
  // KV 命中率(混合):本地 vLLM 用 /metrics prefix_cache(vllmKvPct);拿不到再用 usage 的
  // cache token 口径(selfKvPct,给云端/D3)。二者都按区间(扣基准)重算。
  function adjustCounters(sp, v) {
    sp = sp || {};
    var kvRatio = function (hit, miss) {
      var d = hit + miss;
      return d > 0 ? (hit / d * 100) : null;
    };
    var b = monitorBaseline;
    if (b) {
      var reset =
        numOr0(sp.ttft_sum_s) < b.ttft_sum_s ||
        numOr0(sp.tps_time_s) < b.tps_time_s ||
        numOr0(sp.gen_tokens_total) < b.gen_tokens ||
        numOr0(sp.prompt_tokens_total) < b.prompt_tokens ||
        numOr0(sp.cache_hit_tokens) < b.cache_hit ||
        numOr0(sp.cache_miss_tokens) < b.cache_miss ||
        (v && numOr0(v.prefix_cache_queries) < numOr0(b.pc_queries));
      if (reset) { clearMonitorBaseline(); b = null; }
    }
    var base = function (k) { return b ? numOr0(b[k]) : 0; };
    var vllmKvPct = null;
    if (v) {
      var pcH = numOr0(v.prefix_cache_hits) - base("pc_hits");
      var pcQ = numOr0(v.prefix_cache_queries) - base("pc_queries");
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
      vllmKvPct: vllmKvPct,
      selfKvPct: kvRatio(
        numOr0(sp.cache_hit_tokens) - base("cache_hit"),
        numOr0(sp.cache_miss_tokens) - base("cache_miss")
      ),
      clearedAt: b ? (b.at || null) : null,
    };
  }

  function clearMonitorBaseline() {
    monitorBaseline = null;
    try { localStorage.removeItem(MONITOR_BASELINE_KEY); } catch (e) {}
  }

  // 把当前 counter 快照存为基准点 → 监控页「后 4 项」从此刻起重新计。
  // 自测计数(TTFT/TPS/tokens/usage-cache)+ vLLM prefix_cache(供本地 KV 分支)一起存。
  function clearMonitorStats() {
    var sp = state.monitor && state.monitor.self_perf;
    if (!sp) return false;
    var v = (state.monitor && state.monitor.vllm) || {};
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
    try { localStorage.setItem(MONITOR_BASELINE_KEY, JSON.stringify(monitorBaseline)); } catch (e) {}
    pollMonitor();  // 立即刷新显示，无需等下一个轮询周期
    return true;
  }

  function appQueueSnapshot() {
    var running = 0;
    var waiting = state.queued ? state.queued.length : 0;
    var busyMap = {};
    for (var id in sessionStates) {
      if (!Object.prototype.hasOwnProperty.call(sessionStates, id)) continue;
      if (id === state.activeSessionId) continue;
      var buf = sessionStates[id] || {};
      if (buf.busy) busyMap[id] = true;
      if (Array.isArray(buf.queued)) waiting += buf.queued.length;
    }
    if (state.activeSessionId && state.busy) busyMap[state.activeSessionId] = true;
    running = Object.keys(busyMap).length;
    return { running: running, waiting: waiting };
  }

  async function pollMonitor() {
    if (monitorPollInFlight) return;
    monitorPollInFlight = true;
    try {
      var snap = await invoke("get_monitor_snapshot");
      state.monitorError = null;
      // GPU util sliding window
      if (snap.gpu) {
        gpuUtilHistory.push(snap.gpu.utilization_pct);
        if (gpuUtilHistory.length > 5) gpuUtilHistory.shift();
        snap.gpu._utilMax = Math.max.apply(null, [0].concat(gpuUtilHistory));
      }
      // 监控页「后 4 项」累计指标：TTFT/TPS/tokens 来自 app 侧自测(snap.self_perf,
      // 任何后端都有);KV 混合(本地 vLLM prefix_cache 优先,否则 usage 口径)。
      // 按「清除统计」基准点换算成区间值后再格式化。
      var sadj = adjustCounters(snap.self_perf, snap.vllm);
      // KV 显示值:本地 vLLM 的 /metrics prefix_cache 优先,拿不到用 usage cache 口径(云端)。
      var kvShown = sadj ? (sadj.vllmKvPct != null ? sadj.vllmKvPct
        : (sadj.selfKvPct != null ? sadj.selfKvPct : null)) : null;
      // Format values for display
      var vllm = snap.vllm || null;
      var metricsApplicable = vllm ? vllm.metrics_applicable !== false : false;
      var metricNotApplicableText = "不适用";
      var metricUnavailableText = "未提供";
      var diagnostic = vllm && vllm.diagnostic ? vllm.diagnostic : null;
      var metricDiagnostic = vllm && vllm.metric_diagnostics && vllm.metric_diagnostics.length
        ? vllm.metric_diagnostics[0] : null;
      var targetKind = vllm && vllm.target_kind ? vllm.target_kind : "invalid";
      var targetKindLabel = targetKind === "remote" ? "远端模型" : (targetKind === "local" ? "本地模型" : "配置异常");
      var vllmDisplayModel = vllm ? (vllm.model || vllm.configured_model || "—") : "—";
      var healthStatus = vllm && vllm.health_status ? vllm.health_status : (vllm ? "verified" : "offline");
      var appQueue = appQueueSnapshot();
      var cpu = snap.cpu || null;
      var cpuUsage = cpu && typeof cpu.total_usage_pct === "number" && isFinite(cpu.total_usage_pct)
        ? Math.round(Math.max(0, Math.min(100, cpu.total_usage_pct)))
        : null;
      var computeName = snap.gpu ? snap.gpu.name : (cpu && cpu.name ? cpu.name : bt("gpuUnavailable"));
      snap._fmt = {
        gpuName: computeName,
        cpuName: cpu && cpu.name ? cpu.name : "",
        cpuAvailable: !!cpu,
        computeAvailable: !!(snap.gpu || cpu),
        computeName: computeName,
        gpuVram: snap.gpu && snap.gpu.vram_total_mib > 0
          ? fmtMiB(snap.gpu.vram_used_mib) + " / " + fmtMiB(snap.gpu.vram_total_mib) : "—",
        gpuVramPct: snap.gpu && snap.gpu.vram_total_mib > 0
          ? Math.round(snap.gpu.vram_used_mib / snap.gpu.vram_total_mib * 100) : 0,
        gpuUtil: snap.gpu ? (snap.gpu._utilMax + "%") : "—",
        gpuUtilPct: snap.gpu ? snap.gpu._utilMax : 0,
        processorUtil: cpuUsage != null
          ? cpuUsage + "%"
          : (snap.gpu && snap.gpu.processor_utilization_pct != null ? snap.gpu.processor_utilization_pct + "%" : "—"),
        processorUtilPct: cpuUsage != null
          ? cpuUsage
          : (snap.gpu && snap.gpu.processor_utilization_pct != null ? snap.gpu.processor_utilization_pct : 0),
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
        vllmKv: kvShown != null ? kvShown.toFixed(1) + "%" : "0%",
        vllmKvHasData: kvShown != null,
        vllmTtft: sadj && sadj.ttft_count > 0
          ? (sadj.ttft_sum_s / sadj.ttft_count).toFixed(2) + " s" : "0 s",
        vllmTps: sadj && sadj.tps_time_s > 0
          ? (sadj.tps_tokens / sadj.tps_time_s).toFixed(1) + " tok/s" : "0 tok/s",
        vllmTokTotal: sadj
          ? fmtTok(sadj.gen) + " / " + fmtTok(sadj.prompt) : "—",
        vllmStatsCleared: !!(sadj && sadj.cleared),
        vllmClearedAt: sadj && sadj.cleared ? (sadj.clearedAt || null) : null,
        // 区间原始数值（已扣基准），供前端「长按清除」的数字归零插值动画用。
        vllmRaw: sadj ? {
          kvPct: kvShown,
          ttftS: sadj.ttft_count > 0 ? sadj.ttft_sum_s / sadj.ttft_count : null,
          tps: sadj.tps_time_s > 0 ? sadj.tps_tokens / sadj.tps_time_s : null,
          gen: sadj.gen != null ? sadj.gen : null,
          prompt: sadj.prompt != null ? sadj.prompt : null,
        } : null,
        appVersion: snap.app ? snap.app.pinvou3_version + " (内测版)" : "—",
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
  async function pollBackendStatus() {
    try {
      var s = await invoke("get_backend_status");
      state.backendOnline = !!s.vllm_online;
      // 修 token 分母时机 bug：不再依赖用户打开监控页才拿到真实 max_model_len
      if (s.max_model_len) {
        maxModelLen = s.max_model_len;
        state.tokens.max = maxModelLen;
      }
    } catch (e) {
      state.backendOnline = false;
    }
    notify();
  }

  // ── Settings ─────────────────────────────────────────────────────
  async function loadSettings() {
    try {
      state.settings = await invoke("get_settings");
    } catch (e) {
      state.settings = { theme: "genesis", language: "zh-Hans" };
    }
    notify();
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

  async function refreshLlmApiState(options) {
    options = options || {};
    var refreshModels = options.refreshModels !== false;
    var refreshSavedModels = !!options.refreshSavedModels;
    var status = null;
    var models = null;
    try {
      status = await invoke("get_llmapi_status");
      state.llmApiStatus = status;
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
  // 首屏检测「预装但未启用」状态;eligible 时前端弹引导框。普通机/已配好后端会短路秒回。
  async function detectLocalVllmSetup() {
    try {
      state.vllmSetup = await invoke("detect_local_vllm_setup");
    } catch (e) {
      state.vllmSetup = null; // 检测失败静默,不打扰(等同不弹)
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
  // model 对象字段须是 snake_case(SavedModel serde): {id,name,preset,model,base_url,api_key,credential_action}
  async function saveModel(model) {
    await invoke("save_model", { model: model });
    await loadModels();
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

  // ── Super permission ─────────────────────────────────────────────
  async function refreshSuperPerm() {
    try {
      state.superPermEnabled = !!(await invoke("get_super_permission_status"));
    } catch (e) {
      state.superPermEnabled = false;
    }
    notify();
  }
  async function toggleSuperPerm() {
    var target = !state.superPermEnabled;
    try {
      state.superPermEnabled = !!(await invoke("set_super_permission", { enabled: target }));
      addSystemItem(state.superPermEnabled
        ? bt("superOn")
        : bt("superOff"));
    } catch (e) {
      addSystemItem("⚠️ " + e);
      try { state.superPermEnabled = !!(await invoke("get_super_permission_status")); } catch (e2) {}
    }
    notify();
  }

  // ── Mode state ───────────────────────────────────────────────────
  async function syncModeState() {
    if (!state.activeSessionId) {
      state.modeState = { mode: "yolo" };
      return;
    }
    try {
      var ms = await invoke("get_mode_state", { sessionId: state.activeSessionId });
      state.modeState = { mode: ms.mode || "yolo" };
    } catch (e) {
      state.modeState = { mode: "yolo" };
    }
  }

  // ── 卡片动作辅助 ─────────────────────────────────────────────────
  function patchItemById(id, patch) {
    for (var i = 0; i < state.chatItems.length; i++) {
      if (state.chatItems[i].id === id) { Object.assign(state.chatItems[i], patch); break; }
    }
  }
  function pushUserEcho(text, persist) {
    addChatItem({ type: "user", text: text, time: timeStr() });
    if (persist) state.messages.push({ role: "user", content: [{ type: "text", text: text }] });
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

  function memoryWriteLabel(event) {
    var text = event && event.text || "";
    if (!text) return "记忆已更新";
    return text;
  }
  function memoryWriteStatusLabel(event) {
    var action = event && event.action || "";
    if (action === "confirmed" || action === "remembered") return "记忆已更新";
    if (action === "archived") return "记忆已归档";
    if (action === "deleted") return "记忆已删除";
    return "记忆已更新";
  }
  function normalizeMemoryCandidateText(text) {
    return String(text || "").replace(/\s+/g, " ").trim().toLowerCase();
  }
  function handleMemoryWrite(payload) {
    var sid = payload && payload.session_id || state.activeSessionId;
    var events = payload && Array.isArray(payload.events) ? payload.events : [];
    if (!sid || !events.length) return;
    runOnSession(sid, function () {
      events.forEach(function (event) {
        if (!event) return;
        if (event.action === "pending") {
          var label = memoryWriteLabel(event);
          var labelKey = normalizeMemoryCandidateText(label);
          var existing = state.chatItems.find(function (it) {
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
        var label = memoryWriteLabel(event);
        var labelKey = normalizeMemoryCandidateText(label);
        var existing = state.chatItems.find(function (it) {
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
    state.memory = {
      loading: false,
      error: null,
      profile: overview && overview.profile || null,
      preferences: overview && Array.isArray(overview.preferences) ? overview.preferences : [],
      work_context: overview && Array.isArray(overview.work_context) ? overview.work_context : [],
      current_focus: overview && Array.isArray(overview.current_focus) ? overview.current_focus : [],
      recent_activity: overview && Array.isArray(overview.recent_activity) ? overview.recent_activity : [],
      recent_work: overview && Array.isArray(overview.recent_work) ? overview.recent_work : [],
      pending: overview && Array.isArray(overview.pending) ? overview.pending : [],
      never: overview && Array.isArray(overview.never) ? overview.never : [],
      runtime: overview && overview.runtime || null,
      snapshot_path: overview && overview.snapshot_path || "",
    };
  }
  function upsertPendingMemoryCandidate(item) {
    if (!item || item.status !== "pending_confirm") return;
    var label = item.content || item.text || "";
    if (!label) return;
    var labelKey = normalizeMemoryCandidateText(label);
    var existing = state.chatItems.find(function (it) {
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
    var pending = overview && Array.isArray(overview.pending) ? overview.pending : [];
    pending.forEach(upsertPendingMemoryCandidate);
  }
  async function loadMemoryOverview(options) {
    if (!invoke) return null;
    options = options || {};
    state.memory = Object.assign({}, state.memory, { loading: true, error: null });
    notify();
    try {
      var overview = await invoke("get_memory_overview", { sessionId: state.activeSessionId });
      applyMemoryOverview(overview);
      if (options.rehydratePending) rehydratePendingMemoryCandidates(overview);
      notify();
      return overview;
    } catch (e) {
      state.memory = Object.assign({}, state.memory, { loading: false, error: String(e) });
      notify();
      return null;
    }
  }
  async function saveMemoryProfilePatch(patch) {
    if (!invoke) return null;
    try {
      await invoke("update_memory_profile", { patch: patch || {}, sessionId: state.activeSessionId });
      return await loadMemoryOverview();
    } catch (e) {
      state.memory = Object.assign({}, state.memory, { error: String(e) });
      notify();
      throw e;
    }
  }
  async function deleteMemoryPreference(id) {
    if (!id || !invoke) return false;
    try {
      var res = await invoke("delete_memory_preference", { id: id, sessionId: state.activeSessionId });
      await loadMemoryOverview();
      return !!(res && res.value);
    } catch (e) {
      state.memory = Object.assign({}, state.memory, { error: String(e) });
      notify();
      throw e;
    }
  }
  async function updateMemoryItem(kind, id, patch) {
    if (!id || !invoke) return null;
    try {
      var command = kind === "preference" ? "update_memory_preference"
        : kind === "work_context" ? "update_work_context_memory"
        : (kind === "current_focus" || kind === "recent_activity") ? "update_timed_memory"
        : null;
      if (!command) return null;
      var args = { id: id, patch: patch || {}, sessionId: state.activeSessionId };
      if (command === "update_timed_memory") args.kind = kind;
      var res = await invoke(command, args);
      await loadMemoryOverview();
      return res && res.value;
    } catch (e) {
      state.memory = Object.assign({}, state.memory, { error: String(e) });
      notify();
      throw e;
    }
  }
  async function deleteMemoryItem(kind, id) {
    if (!id || !invoke) return false;
    try {
      var command = kind === "preference" ? "delete_memory_preference"
        : kind === "work_context" ? "delete_work_context_memory"
        : (kind === "current_focus" || kind === "recent_activity") ? "delete_timed_memory"
        : null;
      if (!command) return false;
      var args = { id: id, sessionId: state.activeSessionId };
      if (command === "delete_timed_memory") args.kind = kind;
      var res = await invoke(command, args);
      await loadMemoryOverview();
      return !!(res && res.value);
    } catch (e) {
      state.memory = Object.assign({}, state.memory, { error: String(e) });
      notify();
      throw e;
    }
  }
  async function archiveRecentWorkMemory(id) {
    if (!id || !invoke) return false;
    try {
      var res = await invoke("archive_recent_work_memory", { id: id, sessionId: state.activeSessionId });
      await loadMemoryOverview();
      return !!(res && res.value);
    } catch (e) {
      state.memory = Object.assign({}, state.memory, { error: String(e) });
      notify();
      throw e;
    }
  }
  async function confirmMemoryCandidate(memoryId, chatItemId) {
    if (!memoryId) return;
    var sid = state.activeSessionId;
    try {
      await invoke("confirm_pending_memory", { id: memoryId, sessionId: sid });
      if (chatItemId) patchItemById(chatItemId, { resolved: true, statusLabel: "已记住" });
      await loadMemoryOverview();
      notify();
    } catch (e) {
      addSystemItem("记忆写入失败：" + e);
    }
  }
  async function ignoreMemoryCandidate(memoryId, chatItemId) {
    if (!memoryId) return;
    var sid = state.activeSessionId;
    try {
      await invoke("ignore_pending_memory", { id: memoryId, sessionId: sid });
      if (chatItemId) patchItemById(chatItemId, { resolved: true, statusLabel: "已忽略" });
      await loadMemoryOverview();
      notify();
    } catch (e) {
      addSystemItem("忽略记忆失败：" + e);
    }
  }
  async function neverMemoryCandidate(memoryId, chatItemId) {
    if (!memoryId) return;
    var sid = state.activeSessionId;
    try {
      await invoke("never_pending_memory", { id: memoryId, reason: "user_selected", sessionId: sid });
      if (chatItemId) patchItemById(chatItemId, { resolved: true, statusLabel: "不再提示" });
      await loadMemoryOverview();
      notify();
    } catch (e) {
      addSystemItem("设置不再提示失败：" + e);
    }
  }
  // ── 思考指示器状态（每次阶段切换重置计时）──────────────────────
  function startThinking() { state.thinking = { active: true, phase: "thinking", toolName: "", startedAt: Date.now() }; }
  function thinkingTool(name) { state.thinking = { active: true, phase: "tool", toolName: name || "", startedAt: Date.now() }; }
  function thinkingIdle() { state.thinking = { active: true, phase: "thinking", toolName: "", startedAt: Date.now() }; }
  function stopThinking() { state.thinking = { active: false, phase: "thinking", toolName: "", startedAt: 0 }; }
  function applyModeFromState(st) {
    state.modeState = { mode: st.mode || "yolo" };
  }

  // ── Plan/YOLO 命令 ───────────────────────────────────────────────
  // sid 在 entry 捕获一次,thread through 所有 await —— 防用户切 session 后,
  // 后续 UI 写入/IPC 把卡片塞到错误的 session。
  async function acceptPlan(itemId, planMarkdown, echo) {
    var sid = state.activeSessionId;
    if (!sid) return;
    if (itemId) patchItemByIdFor(sid, itemId, { cardState: "approved", statusLabel: bt("approved"), resolved: true });
    runOnSession(sid, function () { pushUserEcho(echo || bt("echoGo"), true); state.busy = true; startThinking(); });
    notify();
    try {
      var st = await invoke("accept_plan", { sessionId: sid, planMarkdown: planMarkdown || "" });
      runOnSession(sid, function () { applyModeFromState(st); });
    } catch (e) {
      if (itemId) patchItemByIdFor(sid, itemId, { cardState: "active", statusLabel: "", resolved: false });
      runOnSession(sid, function () { state.busy = false; });
      addSystemItemFor(sid, bt("acceptPlanFailed") + e);
    }
    notify();
  }
  async function discardPlan(itemId) {
    if (itemId) patchItemById(itemId, { cardState: "frozen", statusLabel: bt("planDiscarded"), resolved: true });
    if (!state.activeSessionId) { notify(); return; }
    try {
      var st = await invoke("discard_plan", { sessionId: state.activeSessionId });
      applyModeFromState(st);
    } catch (e) { addSystemItem(bt("discardPlanFailed") + e); }
    notify();
  }
  async function exitPlanToYolo() {
    if (!state.activeSessionId) return;
    try {
      var st = await invoke("exit_plan_to_yolo", { sessionId: state.activeSessionId });
      applyModeFromState(st);
    } catch (e) { addSystemItem(bt("exitPlanFailed") + e); }
    notify();
  }
  // 灯泡 toggle：plan ↔ yolo
  async function setPlanModeNext() {
    // 草稿态(无 session)先物化:mode 是 per-session 状态,进 Plan 必须先有 session,
    // 否则草稿页点 Plan 会静默 return 不切换(composer chip 入口暴露的缺陷)。
    var sid = await ensureSession();
    if (!sid) return;
    try {
      var st = await invoke("set_plan_mode_next", { sessionId: sid });
      applyModeFromState(st);
    } catch (e) { addSystemItem(bt("switchModeFailed") + e); }
    notify();
  }
  // plan-stuck / fallback / execution-stuck 卡片动作
  async function planStuckReplan(itemId) {
    patchItemById(itemId, { resolved: true, statusLabel: bt("replanRequested") }); notify();
    await sendMessage("请用 update_plan 工具输出完整方案,不要直接调写工具。");
  }
  async function planStuckGo(itemId) {
    patchItemById(itemId, { resolved: true }); notify();
    await exitPlanToYolo();
    await sendMessage("按上面讨论的方案继续执行任务,直接写文件/跑命令,不要再讨论方案。");
  }

  // ── 用户交互卡 ───────────────────────────────────────────────────
  async function submitUserInput(itemId, toolCallId, answers, questions) {
    patchItemById(itemId, { submitting: true }); notify();
    try {
      await invoke("submit_user_input", { toolCallId: toolCallId, answers: answers, sessionId: state.activeSessionId });
      var summary = answers.map(function (a, i) {
        var text = a.label === "其他" ? "(其他) " + a.value : a.label;
        return (questions[i].header || ("Q" + (i + 1))) + ": " + text;
      }).join(" · ");
      pushUserEcho("✓ " + summary, false);
      flushAssistantMessageToHistory();
      patchItemById(itemId, { resolved: true, cardState: "submitted", submitting: false });
    } catch (e) {
      patchItemById(itemId, { submitting: false, error: String(e) });
    }
    notify();
  }
  async function cancelUserInput(itemId, toolCallId) {
    try { await invoke("cancel_user_input", { toolCallId: toolCallId, sessionId: state.activeSessionId }); } catch (_) {}
    patchItemById(itemId, { resolved: true, cardState: "cancelled" });
    notify();
  }

  // ── 编辑上一轮 / 手动压缩 ─────────────────────────────────────────
  async function editLastTurn(newText) {
    if (state.busy || !state.activeSessionId) return;
    newText = (newText || "").trim();
    if (!newText) return;
    // 删除末尾最近的 user 及之后所有，push 新 user，重渲染
    var cut = -1;
    for (var i = state.messages.length - 1; i >= 0; i--) {
      if (state.messages[i].role === "user") { cut = i; break; }
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
    state.chatItems.push({ id: currentStreamId, type: "assistant", html: "", time: timeStr(), streaming: true });
    notify();
    turnUsageDirty[state.activeSessionId] = false; // 编辑重跑=新一轮，同 doSendFor 重置口径保护
    try {
      await invoke("edit_last_turn", { newMessage: newText, sessionId: state.activeSessionId });
    } catch (e) {
      addSystemItem("⚠️ " + e);
      state.busy = false;
      notify();
    }
  }
  async function compactNow() {
    try { await invoke("compact_now", { sessionId: state.activeSessionId }); } catch (e) { addSystemItem(bt("compactFail") + ": " + e); }
  }

  // ── 产物面板 ─────────────────────────────────────────────────────
  function artifactInfo(path) { return invoke("artifact_info", { path: path }); }
  function readArtifactText(path) { return invoke("read_artifact_text", { path: path }); }
  function writeArtifactText(path, content) { return invoke("write_artifact_text", { path: path, content: content }); }
  function readArtifactImageB64(path) { return invoke("read_artifact_image_b64", { path: path }); }
  // pptx 封面缩略图：读 docProps/thumbnail.jpeg → data URL（无则 null）。本地数据、无外链。
  function readArtifactThumbnail(path) { return invoke("read_artifact_thumbnail", { path: path }).catch(function () { return null; }); }
  function renderArtifactVisual(path) { return invoke("render_artifact_visual", { path: path }); }
  function openContainingFolder(path) { return invoke("open_containing_folder", { path: path }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  function revealSessionFolder(sessionId) { return invoke("reveal_session_folder", { sessionId: sessionId }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  function openScheduledTaskFolder(automationId) { return invoke("open_scheduled_task_folder", { automationId: automationId }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  function openInSystem(path) { return invoke("open_in_system", { path: path }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  // 仅放白名单 URL (metaso.cn / open.bochaai.com),后端 open_external_url 强制校验。
  function openExternalUrl(url) { return invoke("open_external_url", { url: url }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  // 奏折宝箱:列 run 成品文档(deliverables/ 下文件,二进制成品排前)
  function listDeliverables(projectDir) {
    return invoke("list_deliverables", { projectDir: projectDir }).catch(function () { return []; });
  }
  function deliverableCategory(path) {
    var ext = (String(path || "").split(".").pop() || "").toLowerCase();
    if (ext === "html" || ext === "htm" || ext === "mhtml" || ext === "mht") return "web";
    if (ext === "ppt" || ext === "pptx" || ext === "odp" || ext === "dps") return "ppt";
    if (["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "heic"].indexOf(ext) >= 0) return "img";
    return "doc";
  }
  function sessionTitleById(sid) {
    var m = state.sessions.find(function (s) { return s.id === sid; });
    return (m && m.title) || "";
  }
  function currentMemoryArtifacts() {
    var rows = [];
    function addFrom(sid, arts) {
      (arts || []).forEach(function (a) {
        var path = a && a.path;
        if (!path || !isDeliverable(path)) return;
        rows.push({ path: path, sessionId: sid || state.activeSessionId, source: sessionTitleById(sid || state.activeSessionId), name: basename(path) });
      });
    }
    addFrom(state.activeSessionId, state.artifacts);
    Object.keys(sessionStates).forEach(function (sid) { addFrom(sid, sessionStates[sid] && sessionStates[sid].artifacts); });
    return rows;
  }
  // 跨会话产出物索引:磁盘 session JSON 为主,再合并当前内存工作集。
  // 新产物在 chat:done/save_session_artifacts 前也能立刻出现在「本地知识 → 产出物」。
  async function listDeliverableIndex() {
    var disk = await invoke("list_deliverable_index").catch(function () { return []; });
    var byPath = {};
    (disk || []).forEach(function (x) { if (x && x.path) byPath[x.path] = x; });
    var mem = currentMemoryArtifacts().filter(function (x) { return x.path && !byPath[x.path]; });
    var hydrated = await Promise.all(mem.map(async function (x) {
      var path = x.path;
      if (!isAbsPath(path) && x.sessionId) {
        try {
          var ws = await invoke("list_workspace_files", { sessionId: x.sessionId });
          var bn = basename(path);
          var resolved = (ws || []).find(function (p) { return basename(p) === bn; });
          if (resolved) path = resolved;
        } catch (_) {}
      }
      var info = null;
      try { info = await artifactInfo(path); } catch (_) {}
      var ext = (String(path).split(".").pop() || "").toLowerCase();
      return {
        name: x.name || basename(path),
        path: path,
        ext: ext,
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
    var ext = (String(path).split(".").pop() || "").toLowerCase();
    var cmd = (ext === "html" || ext === "htm") ? "open_artifact_window" : "open_in_system";
    return invoke(cmd, { path: path, sessionId: sessionId || null }).catch(function (e) { addSystemItem(bt("openFailed") + e); });
  }

  // ── 附件 ────────────────────────────────────────────────────────
  async function addAttachmentByPath(path) {
    var id = ++attachIdSeq;
    var att = { id: id, basename: basename(path), status: "parsing", result: null, error: null };
    state.attachments.push(att); notify();
    try {
      var result = await invoke("ingest_file", { path: path });
      att.status = "ready"; att.result = result;
    } catch (e) { att.status = "error"; att.error = String(e); }
    notify();
  }
  var recentDroppedPaths = {};
  var DROP_DEDUP_MS = 1500;
  function dropPathKey(path) {
    return String(path || "").toLowerCase();
  }
  function droppedFilePaths(payload) {
    if (!payload) return [];
    if (Array.isArray(payload)) return payload.filter(Boolean);
    if (payload.payload) return droppedFilePaths(payload.payload);
    if (payload.type && payload.type !== "drop") return [];
    if (Array.isArray(payload.paths)) return payload.paths.filter(Boolean);
    if (Array.isArray(payload.files)) return payload.files.filter(Boolean);
    if (typeof payload.path === "string") return [payload.path];
    if (typeof payload === "string") return [payload];
    return [];
  }
  async function addDroppedAttachments(paths) {
    var now = Date.now();
    var seen = {};
    var list = (paths || []).filter(function (p) {
      var key = dropPathKey(p);
      if (!p || seen[key]) return false;
      seen[key] = true;
      if (recentDroppedPaths[key] && now - recentDroppedPaths[key] < DROP_DEDUP_MS) return false;
      recentDroppedPaths[key] = now;
      return true;
    });
    Object.keys(recentDroppedPaths).forEach(function (key) {
      if (now - recentDroppedPaths[key] > DROP_DEDUP_MS * 4) delete recentDroppedPaths[key];
    });
    for (var i = 0; i < list.length; i++) {
      await addAttachmentByPath(list[i]);
    }
  }
  function initAttachmentDrop() {
    if (initAttachmentDrop.done) return;
    initAttachmentDrop.done = true;

    var currentWindow = TAURI.window && TAURI.window.getCurrentWindow ? TAURI.window.getCurrentWindow() : null;
    if (currentWindow && typeof currentWindow.onDragDropEvent === "function") {
      currentWindow.onDragDropEvent(function (event) {
        var paths = droppedFilePaths(event);
        if (paths.length) addDroppedAttachments(paths);
      }).catch(function (e) { console.warn("[attachment] drag-drop listener failed", e); });
    }

    listen("tauri://file-drop", function (event) {
      var paths = droppedFilePaths(event);
      if (paths.length) addDroppedAttachments(paths);
    }).catch(function () {});
    listen("tauri://drag-drop", function (event) {
      var paths = droppedFilePaths(event);
      if (paths.length) addDroppedAttachments(paths);
    }).catch(function () {});

    document.addEventListener("dragover", function (e) {
      if (e.dataTransfer && Array.prototype.indexOf.call(e.dataTransfer.types || [], "Files") >= 0) {
        e.preventDefault();
        e.dataTransfer.dropEffect = "copy";
      }
    });
    document.addEventListener("drop", function (e) {
      var files = e.dataTransfer && e.dataTransfer.files;
      if (!files || files.length === 0) return;
      e.preventDefault();
      var paths = [];
      for (var i = 0; i < files.length; i++) {
        if (files[i] && files[i].path) paths.push(files[i].path);
      }
      if (paths.length) addDroppedAttachments(paths);
    });
  }
  async function addPasteImage(filename, bytes) {
    try {
      var path = await invoke("save_paste_image", { filename: filename, bytes: bytes });
      await addAttachmentByPath(path);
    } catch (e) { addSystemItem(bt("pasteImageFailed") + e); }
  }
  function removeAttachment(id) {
    state.attachments = state.attachments.filter(function (a) { return a.id !== id; });
    notify();
  }
  function clearAttachments() { state.attachments = []; }
  // 打开系统文件选择器并摄入为附件
  async function pickAndAttach() {
    if (!dialogOpen) { addSystemItem(bt("filePickUnavailable")); return; }
    try {
      var selected = await dialogOpen({ multiple: true });
      if (!selected) return;
      var paths = Array.isArray(selected) ? selected : [selected];
      for (var i = 0; i < paths.length; i++) { await addAttachmentByPath(paths[i]); }
    } catch (e) { addSystemItem(bt("filePickFailed") + e); }
  }
  initAttachmentDrop();


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
    var sum = await invoke("create_persona", { input: input });
    await refreshPersonas();
    return sum;
  }
  async function updatePersona(personaId, input) {
    var sum = await invoke("update_persona", { personaId: personaId, input: input });
    await refreshPersonas();
    // 若改的正是当前 session 加持的卡, 同步挂件显示
    if (state.activePersona && state.activePersona.id === personaId) { state.activePersona = sum; notify(); }
    return sum;
  }
  async function deletePersona(personaId) {
    await invoke("delete_persona", { personaId: personaId });
    await refreshPersonas();
  }
  // 给当前 session 加持一张专家面具。后端存 persona_id + 每 turn 注入人设;
  // 前端记 activePersona(挂件) + 发一条系统消息播报。
  // 取专家显示名(兼容 Side A 的 cn_name / Side B 的 name)。
  function personaName(p) {
    if (!p) return "";
    // 内置卡名按 UI 语言显示(personas-i18n.js overlay),中文兜底;自制卡不翻
    var lang = state.settings && state.settings.language;
    var L = lang === "en" ? "en" : lang === "ja" ? "ja" : null;
    var tr = L && p.source !== "user" && window.PERSONA_I18N && window.PERSONA_I18N[p.id] && window.PERSONA_I18N[p.id][L];
    if (tr && tr.name) return tr.name;
    return (p.name || p.cn_name) || "";
  }
  // 记一条卡牌事件到时间线 sidecar(pos=当前 messages 数),并落盘。重载历史时按 pos 插回。
  function recordPersonaEvent(ev) {
    if (!state.activeSessionId) return;
    ev.pos = state.messages.length;
    state.personaEvents.push(ev);
    var sid = state.activeSessionId;
    var snapshot = JSON.parse(JSON.stringify(state.personaEvents));
    invoke("save_session_persona_events", { sessionId: sid, events: snapshot }).catch(function () {});
  }
  async function equipPersona(personaId) {
    if (!state.activeSessionId) {
      await ensureSession(); // 草稿态加卡 → 先物化 session(lazy session)
      if (!state.activeSessionId) return; // 物化失败,放弃
    }
    var prev = state.activePersona; // 换卡前的旧专家(同 session 切换时先播报卸下)
    try {
      var card = await invoke("equip_persona", { sessionId: state.activeSessionId, personaId: personaId });
      // 标题仍是默认值「新对话」→ 用卡牌名命名(无论草稿态物化还是遗留空会话;
      // 用户已主动改名 / 已被首条消息命名的会话不动)。决策:卡牌优先于首条消息。
      var sid = state.activeSessionId;
      var m = state.sessions.find(function (s) { return s.id === sid; });
      // 标题还是默认值 / 仍是卡牌占位(换卡场景)→ 用(新)卡牌名命名,并标记为占位。
      // 占位名会被首条用户消息覆盖(见 persistMessages*),让同卡会话靠对话内容区分。
      if (m && (m.title === "新对话" || m.title === "New chat" || personaPlaceholderTitles[sid])) {
        var newTitle = personaName(card);
        if (newTitle) {
          try { await invoke("rename_session", { id: sid, title: newTitle }); } catch (_) {}
          m.title = newTitle;
          personaPlaceholderTitles[sid] = true;
        }
      }
      // 同 session 换了一张不同的卡 → 先弹一条"已卸下旧专家",再弹新加持。
      if (prev && prev.id !== card.id) {
        addChatItem({ type: "system", text: bt("personaUnequipped") + personaName(prev), time: timeStr() });
        recordPersonaEvent({ kind: "unequip", name: personaName(prev) });
      }
      state.activePersona = card;
      addChatItem({ type: "persona_equip", card: card, time: timeStr() });
      recordPersonaEvent({ kind: "equip", card: card });
      notify();
      return card;
    } catch (e) { addSystemItem(bt("equipFailed") + e); return null; }
  }
  // 摘下当前 session 的专家面具。
  async function unequipPersona() {
    if (!state.activeSessionId) return;
    var prev = state.activePersona;
    try { await invoke("unequip_persona", { sessionId: state.activeSessionId }); } catch (e) { /* 忽略,前端照样摘 */ }
    state.activePersona = null;
    if (prev) { addChatItem({ type: "system", text: bt("personaUnequipped") + personaName(prev), time: timeStr() }); recordPersonaEvent({ kind: "unequip", name: personaName(prev) }); }
    notify();
  }
  // 切换/重载 session 后,从后端拉该 session 的加持状态还原挂件(backend 是真相)。
  async function syncActivePersona() {
    if (!state.activeSessionId) { state.activePersona = null; return; }
    try {
      state.activePersona = await invoke("get_active_persona", { sessionId: state.activeSessionId }) || null;
    } catch (e) { /* 旧 session 无加持,忽略 */ }
  }

  // ── 知识库挂载(会话级粘连,仿 persona) ──
  // 给当前对话挂一个知识集;草稿态先物化 session(同 equipPersona)。挂上后每条消息
  // 发送前后端自动检索注入(commands::chat)。返回挂载的 id 或 null(失败)。
  async function mountCollection(collectionId) {
    if (collectionId == null) return null;
    if (!state.activeSessionId) {
      await ensureSession();
      if (!state.activeSessionId) return null;
    }
    try {
      await invoke("session_mount_collection", { sessionId: state.activeSessionId, collectionId: collectionId });
      state.mountedCollection = collectionId;
      notify();
      return collectionId;
    } catch (e) { addSystemItem("挂载知识集失败: " + e); return null; }
  }
  // 摘下当前对话的知识集挂载。
  async function unmountCollection() {
    if (!state.activeSessionId) { state.mountedCollection = null; notify(); return; }
    try { await invoke("session_unmount_collection", { sessionId: state.activeSessionId }); } catch (e) { /* 前端照样摘 */ }
    state.mountedCollection = null;
    notify();
  }
  // 切换/重载 session 后从后端还原挂载状态(backend 是真相;仅驻内存,重启后为 null)。
  async function syncMountedCollection() {
    if (!state.activeSessionId) { state.mountedCollection = null; return; }
    try {
      var cid = await invoke("session_mounted_collection", { sessionId: state.activeSessionId });
      state.mountedCollection = (cid == null) ? null : cid;
    } catch (e) { state.mountedCollection = null; }
  }

  // ── 应用内升级 ───────────────────────────────────────────────────
  // 链路: check_for_update(对比服务器 latest.json) → download_update(流式下载+sha256,
  // 进度走 update:progress 事件) → install_update(pkexec apt) → restart_app。
  listen("update:progress", function (e) {
    var p = e.payload || {};
    state.updateProgress = p.total ? Math.round((p.downloaded / p.total) * 100) : 0;
    notify();
  });
  listen("remote_control:status", function (e) {
    state.remoteControl = Object.assign({}, state.remoteControl, e.payload || {});
    notify();
  });
  listen("remote_control:snapshot_requested", function (e) {
    var sid = e && e.payload && e.payload.session_id;
    if (!sid) return;
    publishRemoteLiveSnapshot(sid).catch(function () {});
  });
  listen("remote_control:session_created", function (e) {
    var s = e && e.payload && e.payload.session;
    if (s && s.id) {
      getBuffer(s.id);
      if (!state.sessions.some(function (item) { return item.id === s.id; })) {
        state.sessions.unshift({
          id: s.id,
          title: s.title || "新对话",
          updated_at: s.updated_at || "",
          message_count: s.message_count || 0,
        });
      }
      notify();
    }
    refreshHistoryList().then(function () { notify(); }).catch(function () {});
  });
  async function loadAppVersion() {
    try {
      state.appVersion = await invoke("get_app_version");
    } catch (_) {}
  }
  // 启动静默检查: 失败全吞(网络差/更新源挂了不打扰用户)。结果不管新旧都存——
  // available 驱动红点,current_version 给设置页显示当前版本用。
  async function checkForUpdateSilently() {
    try {
      var info = await invoke("check_for_update");
      if (info && info.current_version) state.appVersion = info.current_version;
      if (info) { state.updateInfo = info; notify(); }
    } catch (e) { /* 静默 */ }
  }
  // 设置页手动检查: 错误和「已是最新」都要反馈。
  async function checkForUpdate() {
    state.updateChecking = true; state.updateCheckError = null; notify();
    try {
      var info = await invoke("check_for_update");
      if (info && info.current_version) state.appVersion = info.current_version;
      state.updateInfo = info;
      if (!info.available) state.updateCheckError = "latest"; // 前端按 i18n 显示「已是最新」
    } catch (e) {
      state.updateCheckError = String(e);
    }
    state.updateChecking = false; notify();
  }
  // 下载+安装一条龙: Linux 下载 deb 后 pkexec apt 并自动重启;Windows 下载 zip 后解析 MSI,
  // 安装器启动成功后后端退出当前进程。返回 true 表示安装链路已成功走完。
  async function downloadAndInstallUpdate() {
    if (!state.updateInfo || !state.updateInfo.available || state.updateDownloading) return false;
    var shouldRestartAfterInstall = state.updateInfo.platform === "linux";
    var installed = false;
    state.updateDownloading = true; state.updateCancelling = false;
    state.updateProgress = 0; state.updateError = null; notify();
    try {
      var downloadResult = await invoke("download_update", { info: state.updateInfo });
      state.updateProgress = 100; notify();
      if (downloadResult && typeof downloadResult === "object" && downloadResult.installer_path) {
        await invoke("install_update", { installerPath: downloadResult.installer_path, info: state.updateInfo });
      } else {
        await invoke("install_update", { debPath: downloadResult });
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

  // ── Remote Control: 当前 session 手机远控 ───────────────────────
  async function refreshRemoteControlStatus() {
    try {
      var status = await invoke("remote_control_status");
      state.remoteControl = Object.assign({}, state.remoteControl, status || {});
    } catch (e) {
      state.remoteControl = Object.assign({}, state.remoteControl, { last_error: String(e) });
    }
    notify();
  }
  async function startRemoteControl(sessionId) {
    state.remoteControl = Object.assign({}, state.remoteControl, { starting: true, last_error: null });
    notify();
    try {
      var info = await invoke("remote_control_start", { sessionId: sessionId || null });
      state.remoteControl = Object.assign({}, state.remoteControl, info || {}, { active: true, pairing: info, starting: false, last_error: null });
      await refreshRemoteControlStatus();
      return info;
    } catch (e) {
      state.remoteControl = Object.assign({}, state.remoteControl, { active: false, starting: false, status: "error", last_error: String(e) });
      notify();
      throw e;
    }
  }
  async function stopRemoteControl() {
    try {
      await invoke("remote_control_stop");
    } catch (e) {
      state.remoteControl = Object.assign({}, state.remoteControl, { status: "error", last_error: String(e) });
      notify();
      throw e;
    }
    state.remoteControl = Object.assign({}, state.remoteControl, { active: false, pairing: null, status: "stopped" });
    notify();
  }
  async function refreshRemoteControlQr(sessionId) {
    try {
      var info = await invoke("remote_control_refresh_qr", { sessionId: sessionId || null });
      state.remoteControl = Object.assign({}, state.remoteControl, info || {}, { active: true, pairing: info, last_error: null });
      await refreshRemoteControlStatus();
      return info;
    } catch (e) {
      state.remoteControl = Object.assign({}, state.remoteControl, { status: "error", last_error: String(e) });
      notify();
      throw e;
    }
  }

  // ── 依赖体检 ─────────────────────────────────────────────────────
  // 实时检测各文件解析能力(PDF/Office/OCR/压缩包/邮件)的系统依赖是否齐全,
  // 设置页展示缺失项 + 一键 apt 命令。后端 check_dependencies 不走缓存,装完可复检。
  async function checkDependencies() {
    if (state.depsChecking) return;
    state.depsChecking = true; state.depsInstallError = null; notify();
    try {
      state.deps = await invoke("check_dependencies");
    } catch (e) { state.deps = []; }
    state.depsChecking = false; notify();
  }
  // 一键安装缺失依赖: 收集缺失项的包名 → 后端 pkexec apt 提权安装 → 装完实时重检。
  async function installDependencies() {
    var deps = state.deps || [];
    var missing = deps.filter(function (d) { return !d.installed; });
    if (!missing.length || state.depsInstalling) return;
    var pkgs = [];
    missing.forEach(function (d) {
      var parts = String(d.apt).trim().split(/\s+/).filter(Boolean);
      if (!parts.length || !parts.every(function (p) { return /^[a-z0-9][a-z0-9+.-]*$/i.test(p); })) {
        return;
      }
      parts.forEach(function (p) {
        if (pkgs.indexOf(p) < 0) pkgs.push(p);
      });
    });
    if (!pkgs.length) {
      state.depsInstallError = "当前缺失项无法一键安装，请按依赖说明安装离线组件后重新检测。";
      notify();
      return;
    }
    state.depsInstalling = true; state.depsInstallError = null; notify();
    try {
      await invoke("install_dependencies", { packages: pkgs });
      state.deps = await invoke("check_dependencies"); // 装完实时重检,缺失项应清空
    } catch (e) {
      state.depsInstallError = String(e);
    }
    state.depsInstalling = false; notify();
  }

  // ── 语音输入（WebView one-shot 录音 → 本地 SenseVoice/FunASR ASR；Linux webview 录音授权见 lib.rs setup）──────────────
  var activeVoiceInput = null;

  function setVoiceInputStatus(status, patch) {
    var next = Object.assign({}, state.voiceInput, patch || {});
    next.status = status;
    if (status !== "failed") {
      next.error = null;
      next.category = null;
    }
    state.voiceInput = next;
    notify();
  }

  function emitVoiceDiagnostic(stage, level, message, userMessage, category) {
    var event = {
      stage: stage,
      level: level,
      message: message,
      user_message: userMessage || "",
      category: category || "",
    };
    var fn = level === "error" ? console.error : level === "warn" ? console.warn : console.info;
    fn.call(console, "[voice-input]", event);
  }

  function normalizeVoiceError(err, fallbackStage) {
    var name = String((err && err.name) || "");
    var rawCategory = (err && err.category) || "";
    var rawStage = (err && err.stage) || fallbackStage || "recording";
    var rawMessage = String((err && (err.message || err.toString && err.toString())) || err || "");
    if (name === "NotAllowedError" || name === "SecurityError" || rawCategory === "permission_denied") {
      return { category: "permission_denied", stage: "permission", message: "麦克风权限被拒绝，请在系统设置中允许本应用访问麦克风后重试。" };
    }
    if (name === "NotFoundError" || name === "DevicesNotFoundError" || rawCategory === "device_unavailable") {
      return { category: "device_unavailable", stage: "device", message: "未检测到可用麦克风，请检查录音设备是否启用或被占用。" };
    }
    if (rawCategory === "empty_result") {
      return { category: "empty_result", stage: rawStage, message: "未识别到语音内容，请靠近麦克风后重试。" };
    }
    if (rawCategory === "context_mismatch") {
      return { category: "context_mismatch", stage: "writeback", message: "识别已完成，但当前会话已切换，结果未自动写入。" };
    }
    if (rawCategory === "timeout") {
      return { category: "timeout", stage: "recording", message: "本次语音输入超时，请重试。" };
    }
    if (rawCategory === "recognition_failed") {
      return { category: "recognition_failed", stage: rawStage, message: rawMessage || "语音识别失败，请稍后重试。" };
    }
    return {
      category: rawCategory || "recording_failed",
      stage: rawStage,
      message: rawMessage || "语音输入失败，请检查麦克风后重试。",
    };
  }

  function stopMediaTracks(stream) {
    if (!stream) return;
    stream.getTracks().forEach(function (track) { try { track.stop(); } catch (_) {} });
  }

  function cleanupVoiceInputSession(session) {
    if (!session) return;
    if (session.timeoutId) clearTimeout(session.timeoutId);
    // 先摘掉音频回调：webkit2gtk 的 WebAudio 是 GStreamer 后端，ScriptProcessorNode 的
    // onaudioprocess 跑在音频线程，若在 disconnect/close 期间再触发一次、访问已释放的
    // 缓冲，会让 WebProcess 段错误（表现为「识别出文字后 app 崩溃」）。务必先置 null。
    try { if (session.processor) session.processor.onaudioprocess = null; } catch (_) {}
    try { if (session.processor) session.processor.disconnect(); } catch (_) {}
    try { if (session.source) session.source.disconnect(); } catch (_) {}
    try { if (session.zeroGain) session.zeroGain.disconnect(); } catch (_) {}
    stopMediaTracks(session.stream);
    session.processor = null;
    session.source = null;
    session.zeroGain = null;
    session.stream = null;
    // close() 触发 GStreamer 管线异步拆解，与上面的 disconnect/track.stop 在同一拍里竞争最易崩；
    // 摘干净节点后挪到下一个事件循环再关，并吞掉 close 的异常。
    var ctx = session.audioContext;
    session.audioContext = null;
    if (ctx && ctx.state !== "closed") {
      setTimeout(function () { try { ctx.close().catch(function () {}); } catch (_) {} }, 0);
    }
  }

  function mergeFloatChunks(chunks) {
    var total = chunks.reduce(function (sum, chunk) { return sum + chunk.length; }, 0);
    var out = new Float32Array(total);
    var offset = 0;
    chunks.forEach(function (chunk) {
      out.set(chunk, offset);
      offset += chunk.length;
    });
    return out;
  }

  function downsamplePcm(samples, sourceRate, targetRate) {
    if (!samples.length || sourceRate === targetRate) return samples;
    var ratio = sourceRate / targetRate;
    var len = Math.max(1, Math.round(samples.length / ratio));
    var out = new Float32Array(len);
    for (var i = 0; i < len; i++) {
      var start = Math.floor(i * ratio);
      var end = Math.min(samples.length, Math.floor((i + 1) * ratio));
      var sum = 0;
      var count = 0;
      for (var j = start; j < end; j++) { sum += samples[j]; count++; }
      out[i] = count ? sum / count : samples[Math.min(start, samples.length - 1)];
    }
    return out;
  }

  function encodeWav(samples, sampleRate) {
    var dataSize = samples.length * 2;
    var buffer = new ArrayBuffer(44 + dataSize);
    var view = new DataView(buffer);
    function writeString(offset, value) {
      for (var i = 0; i < value.length; i++) view.setUint8(offset + i, value.charCodeAt(i));
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
    var offset = 44;
    for (var i = 0; i < samples.length; i++, offset += 2) {
      var s = Math.max(-1, Math.min(1, samples[i]));
      view.setInt16(offset, s < 0 ? s * 0x8000 : s * 0x7fff, true);
    }
    return buffer;
  }

  async function finishVoiceInput(cancelled, timedOut) {
    var session = activeVoiceInput;
    if (!session) return;
    if (cancelled) {
      cleanupVoiceInputSession(session);
      activeVoiceInput = null;
      setVoiceInputStatus("cancelled", { message: "已取消语音输入", completedAt: Date.now() });
      emitVoiceDiagnostic("recording", "info", "voice input cancelled", "已取消语音输入", "cancelled");
      return;
    }

    setVoiceInputStatus("transcribing", { message: "正在识别语音…", stage: "transcribing" });
    cleanupVoiceInputSession(session);

    try {
      if (timedOut) {
        emitVoiceDiagnostic("recording", "warn", "recording reached max duration", "", "timeout");
      }
      var raw = mergeFloatChunks(session.chunks);
      var durationMs = raw.length / Math.max(1, session.sampleRate) * 1000;
      if (durationMs < 300) {
        throw { category: "recording_failed", stage: "recording", message: "录音时间过短，请重试。" };
      }
      var pcm = downsamplePcm(raw, session.sampleRate, 16000);
      var wav = encodeWav(pcm, 16000);
      var bytes = Array.from(new Uint8Array(wav));
      var res = await invoke("transcribe_voice_audio", {
        request: {
          audio_bytes: bytes,
          session_id: session.sessionId,
        },
      });
      if (activeVoiceInput !== session) return;
      var text = String((res && res.text) || "").trim();
      if (!text) throw { category: "empty_result", stage: "transcribing", message: "未识别到语音内容" };
      if (state.activeSessionId !== session.sessionId) {
        throw { category: "context_mismatch", stage: "writeback", message: "voice result discarded because active session changed" };
      }
      if (typeof session.writeback === "function") {
        session.writeback(text, session.draftBeforeStart);
      }
      setVoiceInputStatus("completed", { message: "语音已写入输入框", completedAt: Date.now() });
      emitVoiceDiagnostic("writeback", "info", "voice text written back", "语音已写入输入框", "");
    } catch (err) {
      var normalized = normalizeVoiceError(err, "transcribing");
      setVoiceInputStatus("failed", {
        message: normalized.message,
        error: normalized.message,
        category: normalized.category,
        stage: normalized.stage,
        completedAt: Date.now(),
      });
      emitVoiceDiagnostic(normalized.stage, "error", normalized.category, normalized.message, normalized.category);
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
      var st = await invoke("install_voice_asr");
      var patch = { installing: false, status: st, progress: { stage: "done" } };
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

  // 知识库 embedding 模型按需下载（下载 → 校验 → 解压部署 → 热加载），进度走
  // kb_model:progress 事件。resolve 时模型已就绪，调用方据 status.installed 收起 gate。
  async function downloadKbModel() {
    if (state.kbModelSetup.downloading) return state.kbModelSetup.status;
    state.kbModelSetup = Object.assign({}, state.kbModelSetup, { downloading: true, error: null, progress: { stage: "start" } });
    notify();
    try {
      var st = await invoke("kb_model_download");
      state.kbModelSetup = Object.assign({}, state.kbModelSetup, { downloading: false, status: st, progress: { stage: "done" } });
      notify();
      return st;
    } catch (e) {
      state.kbModelSetup = Object.assign({}, state.kbModelSetup, { downloading: false, error: String(e) });
      notify();
      throw e;
    }
  }

  function cancelKbModel() {
    invoke("kb_model_cancel").catch(function () {});
  }

  async function startVoiceInput(draftText, writeback) {
    if (activeVoiceInput && state.voiceInput.status === "recording") {
      finishVoiceInput(false, false);
      return;
    }
    if (activeVoiceInput) {
      finishVoiceInput(true, false);
      return;
    }

    // 首次/缺组件：先检测本地语音识别依赖，缺则弹安装框、不进录音。
    try {
      var asrStatus = await invoke("voice_asr_status");
      // VoiceAsrStatus 只有 engine/ffmpeg/model/ready/missing,无 installable 字段。
      // 未装好即弹安装引导;平台 gating 若要做,需先给后端补 installable(当前无此需求)。
      if (asrStatus && !asrStatus.ready) {
        state.voiceAsrSetup = { open: true, status: asrStatus, installing: false, progress: null, error: null };
        notify();
        return;
      }
    } catch (e) {
      // 检测失败（如 mock 环境/旧后端）不阻塞，继续走原录音路径（环境变量/兜底引擎）
    }

    var AudioCtor = window.AudioContext || window.webkitAudioContext;
    var session = {
      id: Date.now().toString(36),
      sessionId: state.activeSessionId || null,
      draftBeforeStart: String(draftText || ""),
      writeback: writeback,
      chunks: [],
      sampleRate: 16000,
      startedAt: Date.now(),
    };
    activeVoiceInput = session;
    setVoiceInputStatus("requesting_permission", {
      message: "正在请求麦克风权限…",
      sessionId: session.sessionId,
      startedAt: session.startedAt,
      stage: "permission",
    });
    emitVoiceDiagnostic("permission", "info", "requesting microphone permission", "", "");

    try {
      if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
        throw { category: "device_unavailable", stage: "device", message: "当前 WebView 不支持麦克风采集。" };
      }
      if (!AudioCtor) {
        throw { category: "recording_failed", stage: "recording", message: "当前 WebView 不支持音频录制。" };
      }
      session.stream = await navigator.mediaDevices.getUserMedia({
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
      session.audioContext = new AudioCtor();
      session.sampleRate = session.audioContext.sampleRate || 16000;
      session.source = session.audioContext.createMediaStreamSource(session.stream);
      session.processor = session.audioContext.createScriptProcessor(4096, 1, 1);
      session.zeroGain = session.audioContext.createGain();
      session.zeroGain.gain.value = 0;
      session.processor.onaudioprocess = function (event) {
        if (activeVoiceInput !== session) return;
        var input = event.inputBuffer.getChannelData(0);
        session.chunks.push(new Float32Array(input));
      };
      session.source.connect(session.processor);
      session.processor.connect(session.zeroGain);
      session.zeroGain.connect(session.audioContext.destination);
      session.timeoutId = setTimeout(function () { finishVoiceInput(false, true); }, 10000);
      setVoiceInputStatus("recording", { message: "正在录音，再点一次结束", stage: "recording" });
      emitVoiceDiagnostic("recording", "info", "recording started", "", "");
    } catch (err) {
      cleanupVoiceInputSession(session);
      if (activeVoiceInput === session) activeVoiceInput = null;
      var normalized = normalizeVoiceError(err, "recording");
      setVoiceInputStatus("failed", {
        message: normalized.message,
        error: normalized.message,
        category: normalized.category,
        stage: normalized.stage,
        completedAt: Date.now(),
      });
      emitVoiceDiagnostic(normalized.stage, "error", normalized.category, normalized.message, normalized.category);
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
    var left = String(base || "").trimEnd();
    var right = String(text || "").trim();
    if (!left) return right;
    if (!right) return left;
    return left + (/[。！？.!?，,;；:]$/.test(left) ? " " : "\n") + right;
  }

  function runVoiceInputDebugAssertions() {
    var denied = normalizeVoiceError({ name: "NotAllowedError" });
    var noDevice = normalizeVoiceError({ name: "NotFoundError" });
    var mismatch = normalizeVoiceError({ category: "context_mismatch" });
    console.assert(denied.category === "permission_denied", "permission error classified");
    console.assert(noDevice.category === "device_unavailable", "device error classified");
    console.assert(mismatch.stage === "writeback", "context mismatch classified");
    console.assert(appendVoiceText("草稿", "识别文本") === "草稿\n识别文本", "voice text appended");
    return true;
  }

  // ── skill 工作流：动作（invoke 包装）[2026-06-06 恢复] ────────────
  // 合并 973a6f0 把这 6 个函数定义弄丢了(导出/UI调用/后端命令都在,独缺定义)→
  // 导出对象构建撞 ReferenceError(loadSkills undefined)→ window.TauriBridge 整个没装上。
  // 从 943af78 原样恢复。
  function setCurrentPhase(phaseId, source) {
    if (!phaseId) return;
    var wf = state.workflow;
    // 大小写归一匹配 phases
    var match = wf.phases.find(function (p) { return String(p.id).toLowerCase() === String(phaseId).toLowerCase(); });
    var canonical = match ? match.id : phaseId;
    wf.currentPhaseId = canonical;
    if (wf.reachedPhaseIds.indexOf(canonical) < 0) wf.reachedPhaseIds.push(canonical);
    notify();
  }
  async function loadSkills() {
    state.workflow.loadState = "loading"; notify();
    try {
      state.workflow.skills = await invoke("list_skills_v2");
      state.workflow.loadState = "ready";
    } catch (e) { state.workflow.skills = []; state.workflow.loadState = "error"; }
    notify();
  }
  async function activateSkill(name) {
    try {
      var res = await invoke("start_skill_session", { name: name });
      var skill = res.skill || {};
      var meta = res.session || res.metadata || {};
      state.activeSessionId = meta.id || state.activeSessionId;
      state.messages = []; state.chatItems = []; resetPendingAssistant();
      state.workflow.activeSkillName = skill.name || name;
      state.workflow.phases = skill.phases || [];
      state.workflow.currentPhaseId = skill.current_phase_id || (skill.phases && skill.phases[0] && skill.phases[0].id) || null;
      state.workflow.reachedPhaseIds = state.workflow.currentPhaseId ? [state.workflow.currentPhaseId] : [];
      if (meta.id) state.workflow.bindings[meta.id] = skill.name || name;
      await refreshHistoryList();
      await syncModeState();
      notify();
      return res;
    } catch (e) { addSystemItem("⚠️ 启用工作流失败: " + e); notify(); return null; }
  }
  async function deactivateSkill() {
    if (state.activeSessionId) {
      try { await invoke("unbind_session_skill", { sessionId: state.activeSessionId }); } catch (_) {}
      delete state.workflow.bindings[state.activeSessionId];
    }
    state.workflow.activeSkillName = null;
    state.workflow.phases = [];
    state.workflow.currentPhaseId = null;
    state.workflow.reachedPhaseIds = [];
    notify();
  }
  async function openDemo(name) {
    state.workflow.demo = { open: true, name: name, loading: true, kind: null, content: null, error: null, description: null, duration: null };
    notify();
    try {
      var d = await invoke("read_skill_demo", { name: name });
      state.workflow.demo = {
        open: true, name: name, loading: false,
        kind: d.file_kind, path: d.file_path, content: d.content,
        error: null, description: d.description, duration: d.duration,
      };
    } catch (e) {
      state.workflow.demo = { open: true, name: name, loading: false, kind: null, content: null, error: String(e) };
    }
    notify();
  }
  function closeDemo() { state.workflow.demo = null; notify(); }

  // ── 卡片流工作流：动作（invoke 包装）────────────────────────────
  // 新建任务：建项目（project_started 事件设 run 态）→ kick 派发首个 agent（无聊天）。
  async function startWorkflowTask(scenario, brief) {
    try {
      var res = await invoke("start_workflow", { scenario: scenario, briefInit: brief || null });
      try { await invoke("kick_workflow", { sessionId: res.session_id }); }
      catch (e) { addSystemItem("⚠️ kick_workflow 失败: " + e); }
      return res;
    } catch (e) { addSystemItem("⚠️ 启动工作流失败: " + e); return null; }
  }
  // 停止整个 run：后端先落 stop marker 再取消所有后台 SubAgent；返回旧 brief，
  // 供工作流页打开“修改需求并重新开始”的预填表单。
  async function stopWorkflowTask(reason) {
    var sid = state.workflow.run.sessionId;
    if (!sid) throw new Error("当前没有可停止的工作流");
    var result = await invoke("stop_workflow", {
      sessionId: sid,
      reason: reason || "user_stopped",
    });
    markWorkflowRunStopped();
    notify();
    return result || {};
  }
  // 模板页数据源:已发现且 enabled 的工作流(含 ui 块)。失败回空数组(模板页显示空态)。
  async function listWorkflows() {
    try { return (await invoke("list_workflows")) || []; }
    catch (e) { console.warn("list_workflows failed", e); return []; }
  }
  function selectWorkflowRole(roleId) { state.workflow.run.selectedRole = roleId; notify(); }
  function closeWorkflowDrawer() { state.workflow.run.selectedRole = null; notify(); }
  function resetWorkflowRun() {
    state.workflow.run = { active: false, sessionId: null, projectDir: null, scenario: null, status: "idle", agents: {}, cards: [], selectedRole: null };
    notify();
  }
  // 抽屉数据：按 role 拉产出 / gate / 日志（projectDir 来自 run）。
  function getRolePrompt(roleId, projectDir) { return invoke("get_role_prompt", { roleId: roleId, projectDir: projectDir || null }); }
  function getRoleOutputs(roleId) { return invoke("get_role_outputs", { roleId: roleId, projectDir: state.workflow.run.projectDir }); }
  function getGateReport(roleId) { return invoke("get_gate_report", { roleId: roleId, projectDir: state.workflow.run.projectDir }); }
  function getRoleLogs(roleId, tail) { return invoke("get_role_logs", { roleId: roleId, projectDir: state.workflow.run.projectDir, tail: tail || 50 }); }
  // 交互卡动作
  async function submitWorkflowUserInput(cardId, toolCallId, answers) {
    try { await invoke("submit_user_input", { toolCallId: toolCallId, answers: answers, sessionId: state.workflow.run.sessionId }); resolveRunCard(cardId, "submitted"); }
    catch (e) { addSystemItem("⚠️ 提交失败: " + e); }
  }
  // [2026-06-06] 素材上传：复用系统文件选择器(dialogOpen) → 拷进当前 run 的 配套材料/。
  // 返回落盘文件名数组(含同名去重);失败 throw 给调用方(卡片上报错)。
  async function pickAndAddMaterials() {
    if (!dialogOpen) { addSystemItem(bt("filePickUnavailable")); return []; }
    var selected = await dialogOpen({ multiple: true });
    if (!selected) return [];
    var paths = Array.isArray(selected) ? selected : [selected];
    var added = await invoke("add_run_materials", { sessionId: state.workflow.run.sessionId, paths: paths });
    addSystemItem("✅ 已添加 " + added.length + " 个素材到配套材料：" + added.join("、"));
    return added;
  }
  // [新建任务模态] 只弹系统选择器拿路径,不拷贝(run 还没建)。返回路径数组。
  async function pickFiles() {
    if (!dialogOpen) { addSystemItem(bt("filePickUnavailable")); return []; }
    var selected = await dialogOpen({ multiple: true });
    if (!selected) return [];
    return Array.isArray(selected) ? selected : [selected];
  }
  async function pickFolder() {
    if (!dialogOpen) throw new Error("当前环境无法打开文件夹选择器");
    var selected = await dialogOpen({
      directory: true,
      multiple: false,
      title: "选择工作目录",
    });
    if (!selected) return null;
    return Array.isArray(selected) ? (selected[0] || null) : selected;
  }
  async function pickFeedbackFiles() {
    if (!dialogOpen) return [];
    var selected = await dialogOpen({
      multiple: true,
      filters: [
        { name: "Images and videos", extensions: ["png", "jpg", "jpeg", "gif", "webp", "mp4", "mov", "webm"] },
      ],
    });
    if (!selected) return [];
    return Array.isArray(selected) ? selected : [selected];
  }
  // [新建任务模态] start_workflow 建好 run 后,把已选路径拷进该 session 的配套材料/。
  async function addMaterialsToSession(sessionId, paths) {
    if (!paths || !paths.length) return [];
    return invoke("add_run_materials", { sessionId: sessionId, paths: paths });
  }
  // cardId 可为 null:看板 agent 卡上的"确认通过"只有 roleId(刷新后内存 gate 卡已清空)。
  // 批准只需 roleId + sessionId,绝不依赖前端那张内存卡是否存在(那正是"按钮点了没反应"的 bug)。
  async function approveWorkflowGate(cardId, roleId) {
    try {
      await invoke("approve_workflow_gate", { roleId: roleId, sessionId: state.workflow.run.sessionId });
      if (cardId) resolveRunCard(cardId, "approved");
      resolveRunCardsForRole(roleId, "approved");
      await refreshRunState();   // 刷新真实状态:huizou gate_waiting→completed,看板按钮随之消失
      notify();
    } catch (e) { addSystemItem("⚠️ 通过失败: " + e); }
  }
  async function rejectWorkflowGate(cardId, roleId, reason) {
    try {
      await invoke("reject_workflow_gate", { roleId: roleId, reason: reason || "用户打回，请改进后重试", sessionId: state.workflow.run.sessionId });
      if (cardId) resolveRunCard(cardId, "rejected");
      resolveRunCardsForRole(roleId, "rejected");
      await refreshRunState();
      notify();
    } catch (e) { addSystemItem("⚠️ 打回失败: " + e); }
  }
  // 从失败节点续跑:重置该角色为 pending(清重试)后重新调度,上游已完成节点不重跑。
  async function retryWorkflowRole(roleId) {
    try {
      const r = await invoke("retry_workflow_role", { roleId: roleId, sessionId: state.workflow.run.sessionId });
      addSystemItem("🔄 重跑 " + roleId + ": " + r);
    } catch (e) { addSystemItem("⚠️ 重跑失败: " + e); }
  }

  // ── Init ─────────────────────────────────────────────────────────
  async function init() {
    if (initPromise) return initPromise;
    initPromise = (async function () {
    startupMark("bridge:init_start");
    // Populate the global Scheduled unread summary without requiring the user
    // to visit the Scheduled page first. This stays off the startup critical path.
    loadScheduledTasks().catch(function () {}).then(function () {
      loadScheduledTaskRecentRuns().catch(function () {});
    });
    startupMark("bridge:monitor_polling_deferred", "starts when monitor view becomes active");
    await startupAwait("bridge:load_settings", loadSettings);
    await startupAwait("bridge:load_effective_model", loadEffectiveModelConfig);
    await startupAwait("bridge:load_app_version", loadAppVersion);
    await startupAwait("bridge:load_models", loadModels);
    getLlmApiStatus()
      .then(function (status) { return llmApiAccountKnownExists(status) ? getLlmApiModels() : null; })
      .then(loadModels)
      .catch(function (e) { console.warn("load llmapi account/models failed", e); });
    startupMark("bridge:llmapi_refresh_started");
    await startupAwait("bridge:refresh_history", refreshHistoryList);
    enterDraft(); // 启动落空白草稿页(lazy session:不自动选/建会话)
    startupMark("bridge:draft_entered");
    await startupAwait("bridge:refresh_super_permission", refreshSuperPerm);
    loadPersonas(); // 预载卡池(让聊天里草稿"已存入"判定能查到同名自制卡), fire-and-forget
    startupMark("bridge:personas_load_started");
    pollBackendStatus();
    setInterval(pollBackendStatus, 10000);
    reportPendingUpdateResult(); // Windows OTA 升级后反馈,失败保留记录下次再试
    checkForUpdateSilently(); // fire-and-forget,不阻塞启动
    startupMark("bridge:background_checks_started");
    refreshRemoteControlStatus(); // fire-and-forget
    await startupAwait("bridge:resume_workflow", resumeWorkflowOnBoot); // [2026-06-06] 有进行中的工作流 run 就自动挂回看板
    notify();
    startupMark("bridge:init_done");
    if (window.__PINVOU_STARTUP__) window.__PINVOU_STARTUP__.flush();
    })();
    return initPromise;
  }

  // ── Expose API ───────────────────────────────────────────────────
  window.TauriBridge = {
    available: true,
    subscribe: subscribe,
    getState: function () { return snapshotState(); },
    init: init,
    refreshConnectorAuthGates: refreshConnectorAuthGates,
    loadKnowledgeEmbedderAfterFirstFrame: loadKnowledgeEmbedderAfterFirstFrame,
    sendMessage: sendMessage,
    prefillComposer: prefillComposer,
    removeQueued: removeQueued,
    startVoiceInput: startVoiceInput,
    installVoiceAsr: installVoiceAsr,
    closeVoiceAsrSetup: closeVoiceAsrSetup,
    downloadKbModel: downloadKbModel,
    cancelKbModel: cancelKbModel,
    cancelVoiceInput: cancelVoiceInput,
    clearVoiceInput: clearVoiceInput,
    appendVoiceText: appendVoiceText,
    runVoiceInputDebugAssertions: runVoiceInputDebugAssertions,
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
    cancelGeneration: cancelGeneration,
    cancelShellTask: cancelShellTask,
    createNewSession: createNewSession,
    switchToSession: switchToSession,
    openScheduledRunChat: openScheduledRunChat,
    exitScheduledRunChat: exitScheduledRunChat,
    deleteSession: deleteSession,
    renameSession: renameSession,
    toggleSessionPinned: toggleSessionPinned,
    archiveSession: archiveSession,
    restoreArchivedSession: restoreArchivedSession,
    startMonitorPolling: startMonitorPolling,
    stopMonitorPolling: stopMonitorPolling,
    clearMonitorStats: clearMonitorStats,
    saveSettings: saveSettings,
    saveSettingsAndRestart: saveSettingsAndRestart,
    submitFeedback: submitFeedback,
    discoverLocalVllm: discoverLocalVllm,
    getLlmApiStatus: getLlmApiStatus,
    getLlmApiModels: getLlmApiModels,
    setLlmApiDefaultModel: setLlmApiDefaultModel,
    ensureLlmApiBinding: ensureLlmApiBinding,
    loginLlmApiUser: loginLlmApiUser,
    saveLlmApiUserSession: saveLlmApiUserSession,
    retryLlmApiProvisioning: retryLlmApiProvisioning,
    setLlmApiUserEnabled: setLlmApiUserEnabled,
    getLlmApiAdminOverview: getLlmApiAdminOverview,
    detectLocalVllmSetup: detectLocalVllmSetup,
    bootstrapLocalVllm: bootstrapLocalVllm,
    dismissVllmSetup: dismissVllmSetup,
    declineVllmSetup: declineVllmSetup,
    getEffectiveModelConfig: getEffectiveModelConfig,
    loadModels: loadModels,
    saveModel: saveModel,
    deleteModel: deleteModel,
    setActiveModel: setActiveModel,
    loadSessionModel: loadSessionModel,
    switchModel: switchModel,
    testModelConnection: testModelConnection,
    toggleSuperPerm: toggleSuperPerm,
    renderMarkdown: renderMarkdown,
    startRemoteControl: startRemoteControl,
    stopRemoteControl: stopRemoteControl,
    refreshRemoteControlQr: refreshRemoteControlQr,
    refreshRemoteControlStatus: refreshRemoteControlStatus,
    // Plan/YOLO
    acceptPlan: acceptPlan,
    discardPlan: discardPlan,
    exitPlanToYolo: exitPlanToYolo,
    setPlanModeNext: setPlanModeNext,
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
    // 产物
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
    listDeliverables: listDeliverables,
    listDeliverableIndex: listDeliverableIndex,
    openExternalUrl: openExternalUrl,
    // 附件
    addAttachmentByPath: addAttachmentByPath,
    addPasteImage: addPasteImage,
    removeAttachment: removeAttachment,
    clearAttachments: clearAttachments,
    pickAndAttach: pickAndAttach,
    markResolved: markResolved,
    // 工作流
    loadSkills: loadSkills,
    activateSkill: activateSkill,
    deactivateSkill: deactivateSkill,
    openDemo: openDemo,
    closeDemo: closeDemo,
    setCurrentPhase: setCurrentPhase,
    // 卡片流工作流
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
    pickFiles: pickFiles,
    pickFeedbackFiles: pickFeedbackFiles,
    addMaterialsToSession: addMaterialsToSession,
    attachRun: attachRun,
    resumeWorkflowOnBoot: resumeWorkflowOnBoot,
    approveWorkflowGate: approveWorkflowGate,
    rejectWorkflowGate: rejectWorkflowGate,
    retryWorkflowRole: retryWorkflowRole,
    // 卡片池: 专家面具
    loadPersonas: loadPersonas,
    getPersonas: function () { return personaPoolCache; }, // 返回引用(只读),不进 notify 快照
    readPersonaBody: function (id) { return invoke("read_persona_body", { personaId: id }); }, // Side B: 详情拉完整正文
    equipPersona: equipPersona,
    unequipPersona: unequipPersona,
    // 知识库挂载(会话级)
    mountCollection: mountCollection,
    unmountCollection: unmountCollection,
    listCollections: function () { return invoke("kb_collection_list"); }, // 挂载选择器用
    kbModelStatus: function () { return invoke("kb_model_status"); }, // 挂载选择器门控:模型未装则不可选
    loadMemoryOverview: loadMemoryOverview,
    saveMemoryProfilePatch: saveMemoryProfilePatch,
    deleteMemoryPreference: deleteMemoryPreference,
    updateMemoryItem: updateMemoryItem,
    deleteMemoryItem: deleteMemoryItem,
    archiveRecentWorkMemory: archiveRecentWorkMemory,
    confirmMemoryCandidate: confirmMemoryCandidate,
    ignoreMemoryCandidate: ignoreMemoryCandidate,
    neverMemoryCandidate: neverMemoryCandidate,
    // AI 造卡开场引导卡:落一条展示气泡 + 记一条 persona 事件(随会话持久化)。
    // 走 personaEvents 时间线,冷重载时 rerenderFromMessages 按 pos 还原 → 切会话/重启不丢。
    postCardCreatorIntro: function () { addChatItem({ type: "card_creator_intro", time: "" }); recordPersonaEvent({ kind: "card_creator_intro" }); notify(); },
    // 用户自创卡
    createPersona: createPersona,
    updatePersona: updatePersona,
    deletePersona: deletePersona,
    // 应用内升级
    checkForUpdate: checkForUpdate,
    downloadAndInstallUpdate: downloadAndInstallUpdate,
    cancelUpdate: cancelUpdate,
    restartApp: restartApp,
    checkDependencies: checkDependencies,
    installDependencies: installDependencies,
  };

  // Auto-init after DOM ready
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    setTimeout(init, 0);
  }
})();
