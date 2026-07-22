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

  // The pet is a separate WebView and must not own a second copy of the main
  // application state. Keep only the renderer used by its activity cards and
  // return before chat listeners, session loading, polling, or update checks.
  const locationSearch = String((window.location && window.location.search) || "");
  const isPetWindow = /(?:^|[?&])window=pet(?:&|$)/.test(locationSearch);
  if (isPetWindow) {
    window.TauriBridge = {
      available: false,
      renderMarkdown: renderMarkdown,
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
    platformCapabilities: {
      loaded: false,
      os: "unknown",
      showMegacubeSite: false,
      showSuperPermissionSettings: false,
      usesBundledDependencyInstaller: false,
      taskCompletionNotificationsDefault: true,
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
  var shellPollState = Object.create(null); // session_id → { timer, inFlight, waitBudget }
  // 上下文行口径保护：TurnComplete 的 usage.input_tokens 是本轮所有请求的累加
  // （计费口径）。只有单请求的"干净轮"该值才等于当前上下文占用；本轮一旦出现
  // 工具调用/重试/压缩（= 多请求），就跳过这次 tokens 更新，保留上一个准确值。
  var turnUsageDirty = {};  // session_id → bool
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
      shellOutputOmitted: kind => `[Earlier ${kind} output omitted]`, shellUnknownExit: "unknown",
      shellTaskFinished: code => `[Task finished, exit code: ${code}]`,
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
      shellOutputOmitted: kind => `[途中の${kind === "stderr" ? "標準エラー" : "標準出力"}を省略]`, shellUnknownExit: "不明",
      shellTaskFinished: code => `[タスク終了、終了コード: ${code}]`,
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
      shellOutputOmitted: kind => `[中间${kind === "stderr" ? "错误" : "标准"}输出已省略]`, shellUnknownExit: "未知",
      shellTaskFinished: code => `[任务已结束，退出码: ${code}]`,
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
      tokens: { input: 0, max: state.tokens.max },
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
    state.artifacts = filterSessionArtifacts(state.artifacts, id);
    scheduleShellPoll(id, true);
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
        return state.chatItems[i];
      }
    }
    return null;
  }

  function isShellExecutionTool(name) {
    return ["exec_shell", "exec_shell_wait", "exec_wait", "task_shell_start", "task_shell_wait", "shell"].indexOf(name) >= 0;
  }

  function utf8Length(text) {
    try { return new TextEncoder().encode(String(text || "")).length; }
    catch (_) { return String(text || "").length; }
  }

  // Shell snapshots are a tail view, not an append-only byte stream. Normalize
  // terminal control sequences and state omissions explicitly instead of
  // pretending the visible tail is the complete log.
  function normalizeTerminalTail(text) {
    var value = String(text || "")
      .replace(/\x1b\][^\x07]*(?:\x07|\x1b\\)/g, "")
      .replace(/\x1b\[[0-?]*[ -\/]*[@-~]/g, "");
    var out = [];
    value.split("\n").forEach(function (line) {
      // After splitting on LF, a normal Windows CRLF line still ends in CR.
      // Remove that delimiter first; only an *internal* CR means a terminal
      // progress line overwrote earlier content on the same row.
      var visible = line.endsWith("\r") ? line.slice(0, -1) : line;
      var overwriteAt = visible.lastIndexOf("\r");
      if (overwriteAt >= 0) visible = visible.slice(overwriteAt + 1);
      while (visible.indexOf("\x08") >= 0) {
        visible = visible.replace(/[^\x08]\x08/g, "").replace(/^\x08+/, "");
      }
      out.push(visible);
    });
    return out.join("\n");
  }

  function formatShellSnapshot(job) {
    function section(raw, total, kind) {
      raw = String(raw || "");
      var visibleRaw = raw.replace(/^\.\.\.\s*/, "");
      var omitted = /^\.\.\./.test(raw) || Number(total || 0) > utf8Length(visibleRaw);
      var body = normalizeTerminalTail(visibleRaw);
      if (omitted) body = bt("shellOutputOmitted")(kind) + "\n" + body;
      return body;
    }
    var stdout = section(job.stdout_tail, job.stdout_len, "stdout");
    var stderr = section(job.stderr_tail, job.stderr_len, "stderr");
    var parts = [];
    if (stdout) parts.push(stdout);
    if (stderr) parts.push((stdout ? "[STDERR]\n" : "") + stderr);
    if (String(job.status || "").toLowerCase() !== "running") {
      var code = job.exit_code == null ? bt("shellUnknownExit") : String(job.exit_code);
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
    var output = normalizeTerminalTail(String(item.output || ""));
    if (output.indexOf(String(job.id || "")) >= 0 && job.id) return true;
    var evidence = [job.stdout_tail, job.stderr_tail].map(function (raw) {
      return normalizeTerminalTail(String(raw || "").replace(/^\.\.\.\s*/, "")).trim();
    }).filter(Boolean);
    if (evidence.length) return evidence.every(function (text) { return output.indexOf(text) >= 0; });
    return /\(no output\)|no output|无输出|出力なし/i.test(output);
  }

  function applyShellSnapshots(sid, jobs) {
    var anyRunning = false;
    var changed = false;
    var runningCommandCounts = {};
    (jobs || []).forEach(function (job) {
      if (String(job.status || "").toLowerCase() !== "running") return;
      var command = String(job.command || "");
      runningCommandCounts[command] = (runningCommandCounts[command] || 0) + 1;
    });
    runSyncOnSession(sid, function () {
      (jobs || []).forEach(function (job) {
        var status = String(job.status || "").toLowerCase();
        var running = status === "running";
        if (running) anyRunning = true;
        var item = state.chatItems.find(function (it) {
          return it.type === "tool" && it.taskId === job.id;
        });
        if (!item && running) {
          var command = String(job.command || "");
          var candidates = state.chatItems.filter(function (it) {
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
        var snapshotKey = shellSnapshotKey(job);
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
    var poll = shellPollState[sid] || (shellPollState[sid] = {
      timer: null, inFlight: false, waitBudget: 0,
    });
    poll.waitBudget = Math.max(poll.waitBudget, 12);
    if (poll.timer || poll.inFlight) return;
    poll.timer = setTimeout(function () { runShellPoll(sid); }, immediate ? 0 : 250);
  }

  async function runShellPoll(sid) {
    var poll = shellPollState[sid];
    if (!poll || poll.inFlight) return;
    poll.timer = null;
    poll.inFlight = true;
    var running = false;
    try {
      var jobs = await invoke("list_shell_tasks", { sessionId: sid });
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
    var sid = sessionId || state.activeSessionId;
    if (!sid || !taskId) return;
    try {
      await invoke("cancel_shell_task", { sessionId: sid, taskId: taskId });
    } finally {
      scheduleShellPoll(sid, true);
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
  // 桌宠窗口靠全局事件感知回合起止。turn_start 补齐"发送 → 首 token"的空窗
  // (chat:delta 之前引擎在思考,宠物不该干站着);turn_end 只兜 invoke 直接失败
  // 这种不会有 chat:done 的路径。JS emit 是全局广播,宠物窗口 listen 收得到。
  function emitPetEvent(name, sid) {
    try {
      if (TAURI && TAURI.event && TAURI.event.emit) TAURI.event.emit(name, { session_id: sid });
    } catch (_) { /* 桌宠是纯装饰,广播失败不影响对话 */ }
  }

  // 真正发送:在 sid 的工作集上加 user 气泡 + 流式占位 + busy,然后 invoke chat。
  // active/后台通用(后台走 runSyncOnSession 临时切工作集)。
  function doSendFor(sid, text, displayText, attachmentsPayload, meta, restrictTools, surfaceFailure) {
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
    emitPetEvent("pet:turn_start", sid);
    publishRemoteUserMessage(sid, displayText, meta && meta.remoteClientMessageId);
    return invoke("chat", { message: text, attachments: attachmentsPayload, sessionId: sid, restrictTools: !!restrictTools })
      .catch(function (err) {
        console.warn("[pinvou3][chat-ui] send failed", {
          sid: sid,
          error: err && err.toString ? err.toString() : err,
        });
        emitPetEvent("pet:turn_end", sid);
        runSyncOnSession(sid, function () {
          addSystemItem("⚠️ " + (err && err.toString ? err.toString() : err));
          state.busy = false;
          state.chatItems = state.chatItems.filter(function (item) { return item.id !== currentStreamId || item.html; });
        });
        notify();
        flushQueued(sid);
        if (surfaceFailure) throw err;
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

  async function sendMessageToSession(sessionId, text, meta) {
    var sid = String(sessionId || "").trim();
    var content = String(text || "").trim();
    if (!sid) throw new Error("目标会话不存在");
    if (!content) throw new Error("回复内容为空");
    var exists = state.sessions.some(function (session) { return String(session.id) === sid; });
    if (!exists) throw new Error("目标会话不存在");

    await ensureSessionBufferLoaded(sid);
    if (isBusyFor(sid)) {
      runSyncOnSession(sid, function () {
        state.queued.push({
          id: ++itemIdSeq,
          text: content,
          displayText: content,
          attachments: [],
          meta: meta || null,
          restrictTools: false,
        });
      });
      notify();
      return { accepted: true, queued: true };
    }
    var completion = doSendFor(sid, content, content, [], meta || null, false, true)
      .then(
        function () { return { ok: true }; },
        function (error) { return { ok: false, error: error }; }
      );
    return { accepted: true, queued: false, completion: completion };
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
    if (isShellExecutionTool(p.name)) {
      scheduleShellPoll(p.session_id || state.activeSessionId, true);
    }
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
    var updatedToolItem = updateToolItem(p.id, outForCard, p.success);
    var shellTaskId = p.metadata && (p.metadata.task_id || p.metadata.taskId);
    if (updatedToolItem && shellTaskId) {
      var syntheticShellItem = state.chatItems.find(function (it) {
        return it !== updatedToolItem && it.shellSnapshot === true && it.taskId === shellTaskId;
      });
      if (syntheticShellItem) {
        ["shellStatus", "exitCode", "elapsedMs", "output", "state", "success", "shellSnapshotKey"]
          .forEach(function (key) {
            if (syntheticShellItem[key] !== undefined) updatedToolItem[key] = syntheticShellItem[key];
          });
        var syntheticIndex = state.chatItems.indexOf(syntheticShellItem);
        if (syntheticIndex >= 0) state.chatItems.splice(syntheticIndex, 1);
      }
      updatedToolItem.taskId = shellTaskId;
      updatedToolItem.sessionId = p.session_id || state.activeSessionId;
      var shellStatus = String((p.metadata && p.metadata.status) || "").toLowerCase();
      if (shellStatus === "running" || /running|background/i.test(String(p.output || ""))) {
        updatedToolItem.state = "running";
        updatedToolItem.success = null;
      }
      scheduleShellPoll(updatedToolItem.sessionId, true);
    }

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
      state.tokens = { input: input, max: state.tokens.max };
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
  function markWorkflowRunBlocked(payload) {
    var p = payload || {};
    var run = state.workflow.run;
    run.status = "blocked";
    var text = "⚙️ 工作流卡住：" + (p.message || p.blocked_reason || p.reason || "未知原因");
    var existing = (run.cards || []).find(function (card) { return card.workflowBlocked; });
    if (existing) {
      existing.text = text;
      existing.resolved = false;
    } else {
      pushRunCard({ kind: "system", text: text, resolved: false, workflowBlocked: true });
    }
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
        error: typeof r.error === "string" && r.error.trim() ? r.error : null,
        retries: r.retries == null ? 0 : r.retries,
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
    else if (p.blocked || p.status === "blocked") markWorkflowRunBlocked(p);
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
        status: snap.stopped ? "stopped" : (snap.blocked ? "blocked" : (snap.all_completed ? "complete" : "running")),
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
    markWorkflowRunBlocked(p);
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
  var refreshLlmApiOnStartup = settingsFeature.refreshLlmApiOnStartup;
  var getLlmApiStatus = settingsFeature.getLlmApiStatus;
  var getLlmApiModels = settingsFeature.getLlmApiModels;
  var setLlmApiDefaultModel = settingsFeature.setLlmApiDefaultModel;
  var ensureLlmApiBinding = settingsFeature.ensureLlmApiBinding;
  var loginLlmApiUser = settingsFeature.loginLlmApiUser;
  var saveLlmApiUserSession = settingsFeature.saveLlmApiUserSession;
  var retryLlmApiProvisioning = settingsFeature.retryLlmApiProvisioning;
  var setLlmApiUserEnabled = settingsFeature.setLlmApiUserEnabled;
  var getLlmApiAdminOverview = settingsFeature.getLlmApiAdminOverview;
  var submitFeedback = settingsFeature.submitFeedback;
  var discoverLocalVllm = settingsFeature.discoverLocalVllm;
  var detectLocalVllmSetup = settingsFeature.detectLocalVllmSetup;
  var bootstrapLocalVllm = settingsFeature.bootstrapLocalVllm;
  var dismissVllmSetup = settingsFeature.dismissVllmSetup;
  var declineVllmSetup = settingsFeature.declineVllmSetup;
  var getEffectiveModelConfig = settingsFeature.getEffectiveModelConfig;
  var loadModels = settingsFeature.loadModels;
  var saveModel = settingsFeature.saveModel;
  var revealModelApiKey = settingsFeature.revealModelApiKey;
  var deleteModel = settingsFeature.deleteModel;
  var setActiveModel = settingsFeature.setActiveModel;
  var loadSessionModel = settingsFeature.loadSessionModel;
  var switchModel = settingsFeature.switchModel;
  var testModelConnection = settingsFeature.testModelConnection;
  var testSearchProvider = settingsFeature.testSearchProvider;
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
      notify();
      return { ok: state.superPermEnabled === target, enabled: state.superPermEnabled };
    } catch (e) {
      addSystemItem("⚠️ " + e);
      try { state.superPermEnabled = !!(await invoke("get_super_permission_status")); } catch (e2) {}
      notify();
      return { ok: false, enabled: state.superPermEnabled, error: String(e) };
    }
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

  var artifactsFeature = installBridgeFeature("artifacts", { state: state, notify: notify, invoke: invoke, bt: bt, addSystemItem: addSystemItem, dialogOpen: dialogOpen, basename: basename, isDeliverable: isDeliverable, isAbsPath: isAbsPath, sessionStates: sessionStates, TAURI: TAURI, listen: listen });
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
  var listDeliverables = artifactsFeature.listDeliverables;
  var listDeliverableIndex = artifactsFeature.listDeliverableIndex;
  var openExternalUrl = artifactsFeature.openExternalUrl;
  var addAttachmentByPath = artifactsFeature.addAttachmentByPath;
  var addPasteImage = artifactsFeature.addPasteImage;
  var removeAttachment = artifactsFeature.removeAttachment;
  var clearAttachments = artifactsFeature.clearAttachments;
  var pickAndAttach = artifactsFeature.pickAndAttach;
  var personasFeature = installBridgeFeature("personas", { state: state, notify: notify, invoke: invoke, bt: bt, addSystemItem: addSystemItem, addChatItem: addChatItem, timeStr: timeStr, ensureSession: ensureSession, personaPlaceholderTitles: personaPlaceholderTitles });
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
  var unmountCollection = personasFeature.unmountCollection;
  var syncMountedCollection = personasFeature.syncMountedCollection;
  var updaterFeature = installBridgeFeature("updater", { state: state, notify: notify, invoke: invoke, refreshHistoryList: refreshHistoryList, listen: listen, publishRemoteLiveSnapshot: publishRemoteLiveSnapshot, getBuffer: getBuffer });
  var loadAppVersion = updaterFeature.loadAppVersion;
  var checkForUpdateSilently = updaterFeature.checkForUpdateSilently;
  var checkForUpdate = updaterFeature.checkForUpdate;
  var downloadAndInstallUpdate = updaterFeature.downloadAndInstallUpdate;
  var cancelUpdate = updaterFeature.cancelUpdate;
  var restartApp = updaterFeature.restartApp;
  var reportPendingUpdateResult = updaterFeature.reportPendingUpdateResult;
  var remoteControlFeature = installBridgeFeature("remote-control", { state: state, notify: notify, invoke: invoke });
  var refreshRemoteControlStatus = remoteControlFeature.refreshRemoteControlStatus;
  var startRemoteControl = remoteControlFeature.startRemoteControl;
  var stopRemoteControl = remoteControlFeature.stopRemoteControl;
  var refreshRemoteControlQr = remoteControlFeature.refreshRemoteControlQr;
  var dependenciesFeature = installBridgeFeature("dependencies", { state: state, notify: notify, invoke: invoke });
  var checkDependencies = dependenciesFeature.checkDependencies;
  var installDependencies = dependenciesFeature.installDependencies;
  var voiceFeature = installBridgeFeature("voice", { state: state, notify: notify, invoke: invoke });
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
    loadScheduledTasks().catch(function () {}).then(function () {
      loadScheduledTaskRecentRuns().catch(function () {});
    });
    startupMark("bridge:monitor_polling_deferred", "starts when monitor view becomes active");
    await startupAwait("bridge:load_settings", loadSettings);
    await startupAwait("bridge:load_selected_pet", loadSelectedPet);
    await startupAwait("bridge:load_effective_model", loadEffectiveModelConfig);
    await startupAwait("bridge:load_app_version", loadAppVersion);
    await startupAwait("bridge:load_models", loadModels);
    refreshLlmApiOnStartup()
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
    loadPlatformCapabilities: loadPlatformCapabilities,
    loadKnowledgeEmbedderAfterFirstFrame: loadKnowledgeEmbedderAfterFirstFrame,
    sendMessage: sendMessage,
    sendMessageToSession: sendMessageToSession,
    prefillComposer: prefillComposer,
    removeQueued: removeQueued,
    startVoiceInput: startVoiceInput,
    installVoiceAsr: installVoiceAsr,
    cancelVoiceAsrSetup: cancelVoiceAsrSetup,
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
    setSelectedPet: setSelectedPet,
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
   revealModelApiKey: revealModelApiKey,
   deleteModel: deleteModel,
    setActiveModel: setActiveModel,
    loadSessionModel: loadSessionModel,
    switchModel: switchModel,
    testModelConnection: testModelConnection,
    testSearchProvider: testSearchProvider,
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
    getPersonas: getPersonas, // 返回引用(只读),不进 notify 快照
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
