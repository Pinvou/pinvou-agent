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
    window.TauriBridge = { available: false };
    return;
  }

  const { invoke } = TAURI.core;
  const { listen } = TAURI.event;
  const dialogOpen = TAURI.dialog?.open;

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
    activeSessionId: null,
    // 「新建对话」点击计数:每次 enterDraft() 自增(含已在草稿态的提前返回)。前端 welcomeToolId
    // 复位 effect 挂它 → 即便 activeSessionId 没变(draft→draft)也能重新求值,否则残留的工具欢迎卡
    // 会一直顶掉「你好」欢迎语(该 tool 无 welcomeQueries 时整块空白)。
    draftEpoch: 0,
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
        status: "idle",      // idle | running | complete | blocked
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
    updateInfo: null,
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
  };
  // 卡片池 1078 张卡的前端缓存。只读,通过 getPersonas() 取引用,不走 notify 快照。
  var personaPoolCache = [];

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
  var monitorIntervalId = null;
  var gpuUtilHistory = [];
  var maxModelLen = 32768;
  // 监控页「清除统计」基准点：vLLM 的几个累计 counter（TTFT/TPOT/tokens/prefix
  // cache）无法真正清零（它们跟随远端 vLLM 进程生命周期，归零要重启共享进程）。
  // 改为记一个基准快照，显示值 = 当前 counter − 基准。换模型 / vLLM 重启 → counter
  // 倒退到小于基准，自动判定基准失效并丢弃，回落到生命周期累计值。持久化到
  // localStorage，关掉应用再开仍保持「自某时起」的统计。
  var MONITOR_BASELINE_KEY = "pinvou3.monitorStatsBaseline";
  var monitorBaseline = null;
  try {
    var _mb = localStorage.getItem(MONITOR_BASELINE_KEY);
    if (_mb) monitorBaseline = JSON.parse(_mb);
  } catch (e) { monitorBaseline = null; }
  var attachIdSeq = 0;

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
      gpuUnavailable: "GPU info unavailable",
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
      gpuUnavailable: "GPU 情報を取得できません",
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
      gpuUnavailable: "GPU 信息不可用",
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
  var suppressNotify = false;
  // sessionId → true:标题当前是「卡牌占位名」(加卡时自动取的),可被首条用户消息覆盖。
  // 卡牌名只在「加了卡但还没开口」时当临时标题;一旦开始对话,对话内容更能区分同卡会话。
  // 内存态(不持久化):重启后丢标记仅影响「加卡→重启→才发首条消息」这一冷门路径。
  var personaPlaceholderTitles = {};
  function freshBuffer() {
    return {
      messages: [], chatItems: [], personaEvents: [], pinvouReviews: [], artifacts: [], busy: false, queued: [],
      planSnapshot: { plan: null, todos: null },
      modeState: { mode: "yolo" },
      thinking: { active: false, phase: "thinking", toolName: "", startedAt: 0 },
      tokens: { input: 0, max: maxModelLen },
      activePersona: null, // 卡片池: 该 session 加持的专家面具(挂件用)
      mountedCollection: null, // 知识库: 该 session 挂载的知识集 id 或 null

      stream: {
        currentStreamText: "", currentStreamId: 0, pendingAssistantText: "",
        pendingAssistantBlocks: [], itemIdSeq: 0, toolMeta: {},
      },
    };
  }
  function getBuffer(id) {
    if (!id) return null;
    if (!sessionStates[id]) sessionStates[id] = freshBuffer();
    return sessionStates[id];
  }
  function saveWorkingSetTo(buf) {
    if (!buf) return;
    buf.messages = state.messages; buf.chatItems = state.chatItems; buf.artifacts = state.artifacts;
    buf.personaEvents = state.personaEvents;
    buf.pinvouReviews = state.pinvouReviews;
    buf.busy = state.busy; buf.planSnapshot = state.planSnapshot; buf.modeState = state.modeState;
    buf.thinking = state.thinking; buf.tokens = state.tokens; buf.queued = state.queued;
    buf.activePersona = state.activePersona;
    buf.mountedCollection = state.mountedCollection;
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
    state.busy = buf.busy; state.planSnapshot = buf.planSnapshot; state.modeState = buf.modeState;
    state.thinking = buf.thinking; state.tokens = buf.tokens; state.queued = buf.queued || [];
    state.activePersona = buf.activePersona || null;
    state.mountedCollection = buf.mountedCollection || null;
    var s = buf.stream || {};
    currentStreamText = s.currentStreamText || ""; currentStreamId = s.currentStreamId || 0;
    pendingAssistantText = s.pendingAssistantText || ""; pendingAssistantBlocks = s.pendingAssistantBlocks || [];
    itemIdSeq = s.itemIdSeq || 0; toolMeta = s.toolMeta || {};
  }
  // 把 active 工作集存好后切到 id 的 buffer(opts.fresh=新建空 buffer)。
  function switchActiveTo(id, opts) {
    if (state.activeSessionId) saveWorkingSetTo(getBuffer(state.activeSessionId));
    state.activeSessionId = id;
    var buf = sessionStates[id];
    if (!buf || (opts && opts.fresh)) buf = sessionStates[id] = freshBuffer();
    loadWorkingSetFrom(buf);
  }
  // 在指定 session 的工作集上跑一段【同步】逻辑。sid 是 active → 直接跑(零行为变化);
  // 否则临时切到该 buffer 跑完再切回(期间不 notify)。
  function runSyncOnSession(sid, fn) {
    if (!sid || sid === state.activeSessionId) { fn(); return; }
    var bg = sessionStates[sid]; if (!bg) return;
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
    if (sid && sid !== state.activeSessionId && !sessionStates[sid]) sessionStates[sid] = freshBuffer();
    var isBg = sid && sid !== state.activeSessionId;
    runSyncOnSession(sid, fn);
    if (isBg) notify();
  }
  // 落盘指定 session 的 messages + artifacts(active 用工作集,后台用其 buffer)。
  async function persistMessagesFor(sid) {
    if (!sid) return;
    var buf = sid === state.activeSessionId ? null : sessionStates[sid];
    var msgs = buf ? buf.messages : state.messages;
    var arts = buf ? buf.artifacts : state.artifacts;
    try {
      await invoke("save_session_messages", { id: sid, messages: msgs });
      try { await invoke("save_session_artifacts", { id: sid, paths: arts.map(function (a) { return a.path; }) }); } catch (_) {}
      var meta = state.sessions.find(function (s) { return s.id === sid; });
      if (meta && (meta.title === "新对话" || meta.title === "New chat" || personaPlaceholderTitles[sid])) {
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

  // ── Pub/Sub ──────────────────────────────────────────────────────
  var subscribers = [];
  function notify() {
    if (suppressNotify) return;
    // 会话列表「工作中」指示:active 取活动工作集 state.busy,其余取各自 buffer.busy
    state.sessionBusy = {};
    for (var id in sessionStates) state.sessionBusy[id] = !!sessionStates[id].busy;
    if (state.activeSessionId) state.sessionBusy[state.activeSessionId] = !!state.busy;
    var snapshot = JSON.parse(JSON.stringify(state));
    for (var i = 0; i < subscribers.length; i++) subscribers[i](snapshot);
  }
  function subscribe(fn) {
    subscribers.push(fn);
    return function () {
      subscribers = subscribers.filter(function (f) { return f !== fn; });
    };
  }

  // ── Chat Items (display format for React) ────────────────────────
  function addChatItem(item) {
    item.id = ++itemIdSeq;
    state.chatItems.push(item);
  }
  // 成品卡是否"重复出卡":从 chatItems 末尾往前扫——先遇到该文件的修改工具(write/append/edit)
  // → 不算重复(文件改过了,该出新版卡/续卡,即"二次修改弹新卡");先遇到同名成品卡 → 算重复
  // (同一产物没改又 present 一次,模型常见啰嗦)。判据=「上一张同名卡之后有没有改过这个文件」。
  function isDuplicateArtifactCard(pathv) {
    var bn = basename(pathv);
    if (!bn) return false;
    for (var i = state.chatItems.length - 1; i >= 0; i--) {
      var it = state.chatItems[i];
      if (it.type === "tool" && (it.name === "write_file" || it.name === "append_file" || it.name === "edit_file")) {
        var ap = extractArtifactPath(it.args);
        if (ap && basename(ap) === bn) return false;
      }
      if (it.type === "artifact_card" && basename(it.path) === bn) return true;
    }
    return false;
  }
  function addSystemItem(text) {
    addChatItem({ type: "system", text: text, time: timeStr() });
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
    notify();
  }

  // 进入草稿态:不创建 session,只清空工作集 + activeSessionId=null,落在「你好」欢迎页。
  // session 在首次有实质内容(发消息 / 加卡牌,见 ensureSession)时才物化——这样会话列表里
  // 永远不会堆积没用过的空「新对话」(ChatGPT/Claude 式 lazy session)。
  function enterDraft() {
    state.draftEpoch++; // 每次点击都自增——含下面提前返回的「已在草稿态」分支,让前端能重置 welcomeToolId
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

  async function switchToSession(id) {
    if (id === state.activeSessionId) return;
    // 多 session 并发:切换【不再 cancel】旧 session —— 它在自己的 engine 上继续跑,
    // 工作集存进 sessionStates 后台累积。切回来能看到完整(含切走期间产生的)内容。
    // 已有 buffer(切过/在跑)→ 直接换工作集;没有 → load_session 建 buffer + 重渲染。
    if (sessionStates[id]) {
      switchActiveTo(id, null);
      await syncModeState();
      await syncActivePersona();
      await syncMountedCollection();
      notify();
      reconcileArtifacts(id); // 对账磁盘产物(fire-and-forget)
      return;
    }
    try {
      if (state.activeSessionId) saveWorkingSetTo(getBuffer(state.activeSessionId));
      var saved = await invoke("load_session", { id: id });
      state.activeSessionId = saved.metadata.id;
      loadWorkingSetFrom(sessionStates[id] = freshBuffer());
      state.messages = Array.isArray(saved.messages) ? saved.messages : [];
      try { state.personaEvents = await invoke("get_session_persona_events", { sessionId: id }) || []; } catch (e) { state.personaEvents = []; }
      try { state.pinvouReviews = await invoke("get_session_pinvou_reviews", { sessionId: id }) || []; } catch (e) { state.pinvouReviews = []; }
      resetPendingAssistant();
      state.chatItems = [];
      state.artifacts = Array.isArray(saved.artifacts) ? saved.artifacts.map(function (a) {
        var p = typeof a === "string" ? a : (a.storage_path || a.path || "");
        return { path: p, basename: basename(p) };
      }) : [];
      rerenderFromMessages();
      await syncModeState();
      await syncActivePersona();
      await syncMountedCollection();
      notify();
      reconcileArtifacts(id); // 对账磁盘产物(修重启/跟踪遗漏导致的面板缺文件)
    } catch (e) {
      addSystemItem(bt("loadChatFailed") + e);
    }
  }

  async function deleteSession(id) {
    try {
      await invoke("delete_session", { id: id });
      delete sessionStates[id]; // 丢掉该 session 的工作集缓冲(后端已 evict 其 engine)
      delete turnUsageDirty[id];
      state.sessions = state.sessions.filter(function (s) { return s.id !== id; });
      if (state.activeSessionId === id) {
        // 删当前会话 → 落空白草稿页(不自动切上一条/不建空 session)。被删 session 的 buffer
        // 上面已 delete,这里不 saveWorkingSetTo(否则 getBuffer 会把它复活),直接清空工作集。
        state.activeSessionId = null;
        loadWorkingSetFrom(freshBuffer());
      }
      notify();
    } catch (e) {
      addSystemItem(bt("deleteFailed") + e);
    }
  }

  async function renameSession(id, title) {
    try {
      await invoke("rename_session", { id: id, title: title });
      var s = state.sessions.find(function (s) { return s.id === id; });
      if (s) s.title = title;
      delete personaPlaceholderTitles[id]; // 用户主动命名后不再算卡牌占位,不被对话覆盖
      notify();
    } catch (e) {
      console.warn("rename failed", e);
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
          if (pap) presentedArtifacts[pap] = true;
        }
      }
    }
    for (var mi = 0; mi < state.messages.length; mi++) {
      emitPersonaAt(mi, false); // 该消息之前发生的卡牌事件先插
      var m = state.messages[mi];
      var blocks = Array.isArray(m.content) ? m.content : [];
      if (m.role === "user") {
        var textParts = blocks.filter(function (c) { return c.type === "text"; }).map(function (c) { return c.text; });
        var utext = textParts.join("");
        if (textParts.length) {
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
              updateToolItem(c.tool_use_id, c.content, !c.is_error);
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
                  description: wprev.description, time: "",
                });
              } else if (writtenArtifacts[wap] && !presentedArtifacts[wap]) {
                // AI 写了产物但全程没 present_artifact → 兜底补首卡(与实时 chat:done 对齐)
                addChatItem({ type: "artifact_card", path: wap, title: basename(wap), description: "", time: "" });
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
        break;
      }
    }
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
    try {
      var files = await invoke("list_workspace_files", { sessionId: sid });
      if (sid !== state.activeSessionId) return; // 已切走,放弃(避免写错 session)
      var byName = {};
      state.artifacts.forEach(function (a) { byName[basename(a.path)] = a; });
      var added = false;
      files.forEach(function (p) {
        var bn = basename(p);
        var ex = byName[bn];
        if (!ex) { var na = { path: p, basename: bn }; state.artifacts.push(na); byName[bn] = na; added = true; }
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
  function doSendFor(sid, text, displayText, attachmentsPayload, meta) {
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
    return invoke("chat", { message: text, attachments: attachmentsPayload, sessionId: sid })
      .catch(function (err) {
        runSyncOnSession(sid, function () {
          addSystemItem("⚠️ " + (err && err.toString ? err.toString() : err));
          state.busy = false;
          state.chatItems = state.chatItems.filter(function (item) { return item.id !== currentStreamId || item.html; });
        });
        notify();
      });
  }
  // 本轮跑完(或被停止)后,若该 session 不忙且有排队消息 → 把【整个队列】合并成一条
  // 一次性发出(Claude 式:排队的全部一起扔进下一轮,而不是一条条串行)。
  function flushQueued(sid) {
    if (isBusyFor(sid)) return;            // doFinal 等又起了新 turn → 留给那轮的 done 再 flush
    var q = sid === state.activeSessionId ? state.queued : (sessionStates[sid] && sessionStates[sid].queued);
    if (!q || q.length === 0) return;
    var items = q.splice(0, q.length);     // 排空队列
    // 发给模型用 \n\n 分隔(让它清楚是几条独立消息);气泡显示用单换行 \n(紧凑,不空行)
    var text = items.map(function (i) { return i.text; }).filter(Boolean).join("\n\n");
    var displayText = items.map(function (i) { return i.displayText; }).filter(Boolean).join("\n");
    var attachments = [];
    items.forEach(function (i) { if (i.attachments && i.attachments.length) attachments = attachments.concat(i.attachments); });
    var meta = items.length === 1 ? items[0].meta : null; // 单条(如转交)保留 meta;合并多条不标
    notify();
    doSendFor(sid, text, displayText, attachments, meta);
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

    // 展示文本：把附件 chip 名附在用户消息末尾
    var displayText = readyAttachments.length > 0
      ? text + (text ? "\n\n" : "") + "📎 " + readyAttachments.map(function (a) { return a.basename; }).join(" · ")
      : text;
    var attachmentsPayload = readyAttachments.map(function (a) { return a.result; });
    clearAttachments();

    // 排队式:当前 session 正在生成 → 这句进队列(不打断当前轮),本轮 chat:done 后自动发。
    // 输入框上方显示待发 chip(可✕撤销)。停止按钮仍只硬打断当前轮。
    if (state.busy) {
      state.queued.push({ id: ++itemIdSeq, text: text, displayText: displayText, attachments: attachmentsPayload, meta: meta || null });
      notify();
      return;
    }

    await doSendFor(state.activeSessionId, text, displayText, attachmentsPayload, meta);
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
    if (!state.busy) return;
    try {
      await invoke("cancel_generation", { sessionId: state.activeSessionId });
    } catch (e) {
      console.warn("cancel failed", e);
    }
  }

  // ── Persist messages ─────────────────────────────────────────────
  async function persistMessages() {
    if (!state.activeSessionId) return;
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
  function presentArtifactAbsPath(toolResultContent, fallbackPath) {
    fallbackPath = fallbackPath || "";
    try {
      var raw = typeof toolResultContent === "string" ? toolResultContent : JSON.stringify(toolResultContent || {});
      var obj = JSON.parse(raw);
      if (obj && typeof obj.abs_path === "string" && obj.abs_path) return obj.abs_path;
      if (obj && obj.content && obj.content[0] && typeof obj.content[0].text === "string") {
        var inner = JSON.parse(obj.content[0].text);
        if (inner && typeof inner.abs_path === "string" && inner.abs_path) return inner.abs_path;
      }
    } catch (_) {}
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

    // Add tool card
    addChatItem({
      type: "tool", toolId: p.id, name: p.name, args: p.args,
      output: null, success: null, state: "running",
    });
    notify();
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

    updateToolItem(p.id, p.output, p.success);

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
        if (meta.name !== "edit_file") trackArtifact(ap); // edit_file 只改已有,不新建产物
        // 产物(present 过的成品 或 write/append 写进产物列表的)被写/改 → turn 结束补卡。
        // 不再要求 present 过:AI 经常写完产物忘了 present_artifact → 没成品卡 = 没召唤入口。
        var isArtifact = !!findPresentedArtifact(ap) || state.artifacts.some(function (a) { return a.path === ap; });
        if (isArtifact && state.turnDirtyArtifacts.indexOf(ap) < 0) {
          state.turnDirtyArtifacts.push(ap);
        }
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

  // chat:done 特殊:同步收尾(flush/busy=false/mode 复位)走 runSyncOnSession
  // 路由到对应 session;异步收尾(discard_plan/落盘/刷新列表)按显式 sid 路由,
  // 不依赖工作集 —— 这样后台 session 跑完也能正确落盘。
  listen("chat:done", function (e) {
    var sid = (e.payload && e.payload.session_id) || state.activeSessionId;
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
        if (prev) addChatItem({ type: "artifact_card", path: prev.path, title: prev.title, description: prev.description, time: timeStr() });
        else addChatItem({ type: "artifact_card", path: ap, title: basename(ap), description: "", time: timeStr() });
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
    if (phase === "start") addSystemItem(bt("compactStart") + auto + " " + msg);
    else if (phase === "done") addSystemItem(bt("compactDone") + auto + " " + msg);
    else if (phase === "fail") addSystemItem(bt("compactFail") + auto + ": " + msg);
  }); });

  // ── request_user_input：渲染选择卡片（不进 messages.json）─────────
  // payload: { id: tool_call_id, questions: [{header, id, question, options:[{label, description}]}] }
  listen("chat:user_input_required", function (e) { onSessionEvent(e, function () {
    var p = e.payload || {};
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
      if (p.event === "removed") untrackArtifact(p.path);
      else trackArtifact(p.path);
    });
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
    if (p.all_completed) run.status = "complete";
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
        scenario: snap.scenario || null, status: snap.all_completed ? "complete" : "running",
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
    applyAgentPatch(p.role_id, { name: p.role_name || p.role_id, status: p.status || "running" });
    notify();
  });
  // [per_page] fan-out 逐页状态 → 工作流界面把该节点展开成 N 个 SubAgent chip。
  // payload: { base_role, pages:[{page,status}] }，status ∈ queued|running|done|retrying。
  listen("workflow:fanout", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    if (!state.workflow.run.fanout) state.workflow.run.fanout = {};
    var pages = p.pages || [];
    state.workflow.run.fanout[p.base_role] = { total: pages.length, pages: pages };
    notify();
  });
  listen("workflow:complete", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    state.workflow.run.status = "complete";
    // [edict-obs] 后端带回成品路径 → 弹成品卡(一键打开 deck)
    if (p.artifact) {
      pushRunCard({ kind: "artifact", path: p.artifact, text: "🎉 工作流完成，成品已生成", resolved: false });
    }
    notify();
  });
  listen("workflow:blocked", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    state.workflow.run.status = "blocked";
    // 后端 emit 的是 message(+warmup_report)，不是 reason/waiting_roles。
    pushRunCard({ kind: "system", text: "⚙️ 工作流卡住：" + (p.message || p.reason || "未知原因"), resolved: false });
    notify();
  });
  listen("workflow:gate_approval", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
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
    var qs = p.questions || []; if (!Array.isArray(qs) || !qs.length) return;
    pushRunCard({ kind: "user_input", toolCallId: p.id, questions: qs, resolved: false });
    notify();
  });

  // ── Monitor ──────────────────────────────────────────────────────
  function fmtMiB(mib) {
    if (mib == null) return "—";
    return mib >= 1024 ? (mib / 1024).toFixed(1) + " GiB" : mib + " MiB";
  }
  function fmtKiB(kib) {
    if (kib == null) return "—";
    if (kib >= 1024 * 1024) return (kib / 1024 / 1024).toFixed(1) + " GiB";
    if (kib >= 1024) return (kib / 1024).toFixed(0) + " MiB";
    return kib + " KiB";
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

  // 用基准点把 vLLM 累计 counter 换算成「自清除以来」的区间值。无基准 → 直接用
  // 生命周期累计值。检测到任一 counter 倒退（< 基准，说明 vLLM 重启 / 换模型,
  // counter 已归零）→ 丢弃失效基准,回落到累计值,避免显示负数。
  function adjustVllmCounters(v) {
    if (!v) return null;
    var b = monitorBaseline;
    if (b) {
      var reset =
        numOr0(v.ttft_sum_s) < b.ttft_sum_s ||
        numOr0(v.tpot_sum_s) < b.tpot_sum_s ||
        numOr0(v.generation_tokens_total) < b.gen_tokens ||
        numOr0(v.prompt_tokens_total) < b.prompt_tokens ||
        numOr0(v.prefix_cache_queries) < b.pc_queries;
      if (reset) { clearMonitorBaseline(); b = null; }
    }
    if (!b) {
      return {
        cleared: false,
        ttft_sum_s: v.ttft_sum_s, ttft_count: v.ttft_count,
        tpot_sum_s: v.tpot_sum_s, tpot_count: v.tpot_count,
        gen: v.generation_tokens_total, prompt: v.prompt_tokens_total,
        kvPct: v.prefix_cache_hit_pct,
      };
    }
    var hits = numOr0(v.prefix_cache_hits) - b.pc_hits;
    var queries = numOr0(v.prefix_cache_queries) - b.pc_queries;
    return {
      cleared: true,
      ttft_sum_s: numOr0(v.ttft_sum_s) - b.ttft_sum_s,
      ttft_count: numOr0(v.ttft_count) - b.ttft_count,
      tpot_sum_s: numOr0(v.tpot_sum_s) - b.tpot_sum_s,
      tpot_count: numOr0(v.tpot_count) - b.tpot_count,
      gen: numOr0(v.generation_tokens_total) - b.gen_tokens,
      prompt: numOr0(v.prompt_tokens_total) - b.prompt_tokens,
      kvPct: queries > 0 ? (hits / queries * 100) : null,
      clearedAt: b.at || null,
    };
  }

  function clearMonitorBaseline() {
    monitorBaseline = null;
    try { localStorage.removeItem(MONITOR_BASELINE_KEY); } catch (e) {}
  }

  // 把当前 vLLM counter 快照存为基准点 → 监控页「后 4 项」从此刻起重新计。
  function clearMonitorStats() {
    var v = state.monitor && state.monitor.vllm;
    if (!v) return false;
    monitorBaseline = {
      ttft_sum_s: numOr0(v.ttft_sum_s),
      ttft_count: numOr0(v.ttft_count),
      tpot_sum_s: numOr0(v.tpot_sum_s),
      tpot_count: numOr0(v.tpot_count),
      gen_tokens: numOr0(v.generation_tokens_total),
      prompt_tokens: numOr0(v.prompt_tokens_total),
      pc_hits: numOr0(v.prefix_cache_hits),
      pc_queries: numOr0(v.prefix_cache_queries),
      at: Date.now(),  // 记录清除时刻，供「统计自 HH:MM 起」状态文字
    };
    try { localStorage.setItem(MONITOR_BASELINE_KEY, JSON.stringify(monitorBaseline)); } catch (e) {}
    pollMonitor();  // 立即刷新显示，无需等下一个轮询周期
    return true;
  }

  async function pollMonitor() {
    try {
      var snap = await invoke("get_monitor_snapshot");
      // GPU util sliding window
      if (snap.gpu) {
        gpuUtilHistory.push(snap.gpu.utilization_pct);
        if (gpuUtilHistory.length > 5) gpuUtilHistory.shift();
        snap.gpu._utilMax = Math.max.apply(null, [0].concat(gpuUtilHistory));
      }
      // 监控页「后 4 项」累计指标：按「清除统计」基准点换算成区间值后再格式化。
      var vadj = adjustVllmCounters(snap.vllm);
      // Format values for display
      snap._fmt = {
        gpuName: snap.gpu ? snap.gpu.name : bt("gpuUnavailable"),
        gpuVram: snap.gpu && snap.gpu.vram_total_mib > 0
          ? fmtMiB(snap.gpu.vram_used_mib) + " / " + fmtMiB(snap.gpu.vram_total_mib) : "—",
        gpuVramPct: snap.gpu && snap.gpu.vram_total_mib > 0
          ? Math.round(snap.gpu.vram_used_mib / snap.gpu.vram_total_mib * 100) : 0,
        gpuUtil: snap.gpu ? (snap.gpu._utilMax + "%") : "—",
        gpuUtilPct: snap.gpu ? snap.gpu._utilMax : 0,
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
        vllmModel: snap.vllm ? (snap.vllm.model || "—") : "—",
        vllmConfiguredModel: snap.vllm ? (snap.vllm.configured_model || null) : null,
        vllmModelMismatch: snap.vllm && snap.vllm.configured_model && snap.vllm.model
          ? snap.vllm.configured_model !== snap.vllm.model : false,
        vllmStatus: snap.vllm ? snap.vllm.status.toUpperCase() : "OFFLINE",
        vllmOnline: snap.vllm ? (snap.vllm.status !== "offline" && snap.vllm.status !== "mismatch") : false,
        vllmUpstream: snap.vllm ? (snap.vllm.upstream || "—") : "—",
        vllmMaxLen: snap.vllm ? (snap.vllm.max_model_len || "—") : "—",
        vllmQueue: snap.vllm
          ? (snap.vllm.num_requests_running != null ? snap.vllm.num_requests_running : "—") + " / " +
            (snap.vllm.num_requests_waiting != null ? snap.vllm.num_requests_waiting : "—") : "— / —",
        vllmKv: vadj && vadj.kvPct != null
          ? vadj.kvPct.toFixed(1) + "%" : "—",
        vllmTtft: vadj && vadj.ttft_count > 0
          ? (vadj.ttft_sum_s / vadj.ttft_count).toFixed(2) + " s" : "—",
        vllmTps: vadj && vadj.tpot_sum_s > 0
          ? (vadj.tpot_count / vadj.tpot_sum_s).toFixed(1) + " tok/s" : "—",
        vllmTokTotal: vadj && vadj.gen != null
          ? fmtTok(vadj.gen) + " / " + fmtTok(vadj.prompt) : "—",
        vllmStatsCleared: !!(vadj && vadj.cleared),
        vllmClearedAt: vadj && vadj.cleared ? (vadj.clearedAt || null) : null,
        // 区间原始数值（已扣基准），供前端「长按清除」的数字归零插值动画用。
        vllmRaw: vadj ? {
          kvPct: vadj.kvPct,
          ttftS: vadj.ttft_count > 0 ? vadj.ttft_sum_s / vadj.ttft_count : null,
          tps: vadj.tpot_sum_s > 0 ? vadj.tpot_count / vadj.tpot_sum_s : null,
          gen: vadj.gen != null ? vadj.gen : null,
          prompt: vadj.prompt != null ? vadj.prompt : null,
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
      console.warn("monitor poll failed", e);
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
    state.settings = prefs;
    try {
      await invoke("update_settings", { prefs: prefs });
    } catch (e) {
      console.warn("save settings failed", e);
    }
    notify();
  }
  async function saveSettingsAndRestart(prefs) {
    state.settings = prefs;
    try {
      await invoke("save_settings_and_restart", { prefs: prefs });
    } catch (e) {
      console.warn("save settings and restart failed", e);
    }
  }
  async function discoverLocalVllm(request) {
    return await invoke("discover_local_vllm", { request: request || null });
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
  // model 对象字段须是 snake_case(SavedModel serde): {id,name,preset,model,base_url,api_key}
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
  async function testModelConnection(baseUrl, apiKey) {
    return await invoke("test_model_connection", { baseUrl: baseUrl, apiKey: apiKey });
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
  function readArtifactImageB64(path) { return invoke("read_artifact_image_b64", { path: path }); }
  // pptx 封面缩略图：读 docProps/thumbnail.jpeg → data URL（无则 null）。本地数据、无外链。
  function readArtifactThumbnail(path) { return invoke("read_artifact_thumbnail", { path: path }).catch(function () { return null; }); }
  function renderArtifactVisual(path) { return invoke("render_artifact_visual", { path: path }); }
  function openContainingFolder(path) { return invoke("open_containing_folder", { path: path }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  function openInSystem(path) { return invoke("open_in_system", { path: path }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  // 仅放白名单 URL (metaso.cn / open.bochaai.com),后端 open_external_url 强制校验。
  function openExternalUrl(url) { return invoke("open_external_url", { url: url }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  // 奏折宝箱:列 run 成品文档(deliverables/ 下文件,二进制成品排前)
  function listDeliverables(projectDir) {
    return invoke("list_deliverables", { projectDir: projectDir }).catch(function () { return []; });
  }
  // 外部打开产物：HTML 走 Tauri 独立窗口（绕沙箱），其他走系统应用
  // 相对路径(write_file 兜底补卡的相对文件名)由后端 open_in_system/open_artifact_window
  // 内的 resolve_artifact_path 按 active session workspace 解析,前端无需预处理。
  function openArtifactExternal(path) {
    var ext = (String(path).split(".").pop() || "").toLowerCase();
    var cmd = (ext === "html" || ext === "htm") ? "open_artifact_window" : "open_in_system";
    return invoke(cmd, { path: path }).catch(function (e) { addSystemItem(bt("openFailed") + e); });
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
  // 启动静默检查: 失败全吞(网络差/更新源挂了不打扰用户)。结果不管新旧都存——
  // available 驱动红点,current_version 给设置页显示当前版本用。
  async function checkForUpdateSilently() {
    try {
      var info = await invoke("check_for_update");
      if (info) { state.updateInfo = info; notify(); }
    } catch (e) { /* 静默 */ }
  }
  // 设置页手动检查: 错误和「已是最新」都要反馈。
  async function checkForUpdate() {
    state.updateChecking = true; state.updateCheckError = null; notify();
    try {
      var info = await invoke("check_for_update");
      state.updateInfo = info;
      if (!info.available) state.updateCheckError = "latest"; // 前端按 i18n 显示「已是最新」
    } catch (e) {
      state.updateCheckError = String(e);
    }
    state.updateChecking = false; notify();
  }
  // 下载+安装一条龙: 下载完 pkexec 弹系统密码框,装完置 updateReady 等用户点重启。
  async function downloadAndInstallUpdate() {
    if (!state.updateInfo || !state.updateInfo.available || state.updateDownloading) return;
    state.updateDownloading = true; state.updateCancelling = false;
    state.updateProgress = 0; state.updateError = null; notify();
    try {
      var debPath = await invoke("download_update", { info: state.updateInfo });
      state.updateProgress = 100; notify();
      await invoke("install_update", { debPath: debPath });
      state.updateReady = true;
    } catch (e) {
      // 用户主动取消下载时后端返回「已取消下载」,当正常处理不弹错误
      if (state.updateCancelling) state.updateProgress = 0;
      else state.updateError = String(e);
    }
    state.updateDownloading = false; state.updateCancelling = false; notify();
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
      String(d.apt).split(/\s+/).forEach(function (p) {
        if (p && pkgs.indexOf(p) < 0) pkgs.push(p);
      });
    });
    state.depsInstalling = true; state.depsInstallError = null; notify();
    try {
      await invoke("install_dependencies", { packages: pkgs });
      state.deps = await invoke("check_dependencies"); // 装完实时重检,缺失项应清空
    } catch (e) {
      state.depsInstallError = String(e);
    }
    state.depsInstalling = false; notify();
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
    await loadSettings();
    await loadEffectiveModelConfig();
    await loadModels();
    await refreshHistoryList();
    enterDraft(); // 启动落空白草稿页(lazy session:不自动选/建会话)
    await refreshSuperPerm();
    loadPersonas(); // 预载卡池(让聊天里草稿"已存入"判定能查到同名自制卡), fire-and-forget
    pollBackendStatus();
    setInterval(pollBackendStatus, 10000);
    checkForUpdateSilently(); // fire-and-forget,不阻塞启动
    await resumeWorkflowOnBoot(); // [2026-06-06] 有进行中的工作流 run 就自动挂回看板
    notify();
  }

  // ── Expose API ───────────────────────────────────────────────────
  window.TauriBridge = {
    available: true,
    subscribe: subscribe,
    getState: function () { return JSON.parse(JSON.stringify(state)); },
    init: init,
    sendMessage: sendMessage,
    removeQueued: removeQueued,
    cancelGeneration: cancelGeneration,
    createNewSession: createNewSession,
    switchToSession: switchToSession,
    deleteSession: deleteSession,
    renameSession: renameSession,
    startMonitorPolling: startMonitorPolling,
    stopMonitorPolling: stopMonitorPolling,
    clearMonitorStats: clearMonitorStats,
    saveSettings: saveSettings,
    saveSettingsAndRestart: saveSettingsAndRestart,
    discoverLocalVllm: discoverLocalVllm,
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
    readArtifactImageB64: readArtifactImageB64,
    readArtifactThumbnail: readArtifactThumbnail,
    renderArtifactVisual: renderArtifactVisual,
    openContainingFolder: openContainingFolder,
    openInSystem: openInSystem,
    openArtifactExternal: openArtifactExternal,
    listDeliverables: listDeliverables,
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
