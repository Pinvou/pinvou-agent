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
  var MAX_SCHEDULED_SESSION_BUFFERS = 64;
  var MAX_SCHEDULED_RUN_SESSION_OWNERS = 64;
  var suppressNotify = false;
  // sessionId → true:标题当前是「卡牌占位名」(加卡时自动取的),可被首条用户消息覆盖。
  // 卡牌名只在「加了卡但还没开口」时当临时标题;一旦开始对话,对话内容更能区分同卡会话。
  // 内存态(不持久化):重启后丢标记仅影响「加卡→重启→才发首条消息」这一冷门路径。
  var personaPlaceholderTitles = {};
  var chatFeature = installBridgeFeature("chat", {
    state: state, invoke: invoke, TAURI: TAURI,
    sessionStates: sessionStates, turnUsageDirty: turnUsageDirty,
    personaPlaceholderTitles: personaPlaceholderTitles,
    renderMarkdown: renderMarkdown, safeConsoleInfo: safeConsoleInfo, bt: bt,
    notify: function () { return notify.apply(null, arguments); },
    runSyncOnSession: function () { return runSyncOnSession.apply(null, arguments); },
    startThinking: function () { return startThinking.apply(null, arguments); },
    ensureSessionBufferLoaded: function () { return ensureSessionBufferLoaded.apply(null, arguments); },
    ensureSession: function () { return ensureSession.apply(null, arguments); },
    clearAttachments: function () { return clearAttachments.apply(null, arguments); },
    isScheduledRunSession: function () { return isScheduledRunSession.apply(null, arguments); },
    basename: basename,
    extractArtifactPath: extractArtifactPath,
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
  var isDuplicateArtifactCard = chatFeature.isDuplicateArtifactCard;
  var addSystemItem = chatFeature.addSystemItem;
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
    state: state, invoke: invoke, notify: notify,
    sessionStates: sessionStates, scheduledRunSessionOwners: scheduledRunSessionOwners,
    personaPlaceholderTitles: personaPlaceholderTitles, turnUsageDirty: turnUsageDirty,
    runSyncOnSession: runSyncOnSession, persistMessagesFor: persistMessagesFor,
    resetPendingAssistant: resetPendingAssistant, stopThinking: stopThinking,
    rerenderFromMessages: rerenderFromMessages, syncModeState: syncModeState,
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
    filterSessionArtifacts: filterSessionArtifacts,
    scheduleShellPoll: function () { return scheduleShellPoll.apply(null, arguments); },
    bt: bt, userMessageDisplayText: userMessageDisplayText,
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

  var scheduledFeature = installBridgeFeature("scheduled", { state: state, notify: notify, invoke: invoke, runSyncOnSession: runSyncOnSession, addSystemItem: addSystemItem, rememberScheduledRunOwner: rememberScheduledRunOwner, isScheduledRunTerminal: isScheduledRunTerminal, purgeSessionBuffer: purgeSessionBuffer, createNewSession: createNewSession, prefillComposer: prefillComposer, sessionStates: sessionStates });
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

  async function cancelShellTask(sessionId, taskId) {
    if (!sessionId || !taskId) throw new Error("Missing shell task identity");
    return invoke("cancel_shell_task", { sessionId: sessionId, taskId: taskId });
  }

  installBridgeFeature("chat-events", {
    state: state, listen: listen, invoke: invoke, turnUsageDirty: turnUsageDirty,
    sessionStates: sessionStates, renderMarkdown: renderMarkdown, bt: bt,
    notify: notify, onSessionEvent: onSessionEvent, runSyncOnSession: runSyncOnSession,
    addChatItem: addChatItem, addSystemItem: addSystemItem, timeStr: timeStr,
    flushPendingTextBlock: flushPendingTextBlock,
    flushAssistantMessageToHistory: flushAssistantMessageToHistory,
    resetPendingAssistant: resetPendingAssistant, flushQueued: flushQueued,
    isBusyFor: isBusyFor, doSendFor: doSendFor,
    ensureSessionBufferLoaded: ensureSessionBufferLoaded,
    thinkingTool: thinkingTool, thinkingIdle: thinkingIdle, stopThinking: stopThinking,
    scheduleScheduledRunRefresh: scheduleScheduledRunRefresh,
    handleMemoryWrite: function () { return handleMemoryWrite.apply(null, arguments); },
    isPresentArtifactTool: isPresentArtifactTool,
    artifactPathFromToolOutput: artifactPathFromToolOutput,
    shouldUseToolOutputAsArtifact: shouldUseToolOutputAsArtifact,
    presentArtifactAbsPath: presentArtifactAbsPath,
    extractArtifactPath: extractArtifactPath, markTurnDirtyArtifact: markTurnDirtyArtifact,
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
