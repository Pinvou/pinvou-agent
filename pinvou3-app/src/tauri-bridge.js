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
    messages: [],      // Anthropic Messages schema
    chatItems: [],     // display items for React
    // 卡牌加持/卸下事件时间线(sidecar, 不进 messages/LLM)。每项 {kind,pos,...}。
    // pos = 事件发生时的 messages 数, rerender 时按 pos 插回原位, 让重载历史不割裂。
    personaEvents: [],
    busy: false,
    monitor: null,
    backendOnline: null, // null=checking, true, false
    settings: null,
    superPermEnabled: false,
    modeState: { mode: "yolo", plan_phase: "none" },
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
    // 卡片池: 专家面具。activePersona = 当前 session 加持的专家卡(完整对象)或 null,
    // 驱动聊天室右上角挂件。
    activePersona: null,
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
  var monitorIntervalId = null;
  var gpuUtilHistory = [];
  var maxModelLen = 32768;
  var attachIdSeq = 0;
  // Plan 文本兜底卡片命中关键词（与 main.js 对齐）
  var PLAN_FALLBACK_KEYWORDS = ["方案", "步骤", "以下", "技术栈", "实现", "设计", "**"];

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
      exitedPlan: "🚪 Exited Plan", discardPlanFailed: "⚠️ discard_plan failed: ", exitPlanFailed: "⚠️ Failed to exit Plan: ", switchModeFailed: "⚠️ Failed to switch mode: ",
      replanRequested: "📋 Asking the AI to re-plan…", adoptingPlan: "✅ Adopting...", adoptEcho: "✅ Adopt this plan",
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
      exitedPlan: "🚪 Plan を終了", discardPlanFailed: "⚠️ discard_plan に失敗: ", exitPlanFailed: "⚠️ Plan の終了に失敗: ", switchModeFailed: "⚠️ モード切替に失敗: ",
      replanRequested: "📋 AI にプランを出し直させています…", adoptingPlan: "✅ 採用中...", adoptEcho: "✅ このプランを採用",
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
      exitedPlan: "🚪 已退出 Plan", discardPlanFailed: "⚠️ discard_plan 失败: ", exitPlanFailed: "⚠️ 退出 Plan 失败: ", switchModeFailed: "⚠️ 切换模式失败: ",
      replanRequested: "📋 让 AI 重出方案…", adoptingPlan: "✅ 采纳中...", adoptEcho: "✅ 采纳此方案",
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
  function freshBuffer() {
    return {
      messages: [], chatItems: [], personaEvents: [], artifacts: [], busy: false, queued: [],
      planSnapshot: { plan: null, todos: null },
      modeState: { mode: "yolo", plan_phase: "none" },
      thinking: { active: false, phase: "thinking", toolName: "", startedAt: 0 },
      tokens: { input: 0, max: maxModelLen },
      activePersona: null, // 卡片池: 该 session 加持的专家面具(挂件用)

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
    buf.busy = state.busy; buf.planSnapshot = state.planSnapshot; buf.modeState = state.modeState;
    buf.thinking = state.thinking; buf.tokens = state.tokens; buf.queued = state.queued;
    buf.activePersona = state.activePersona;
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
    state.busy = buf.busy; state.planSnapshot = buf.planSnapshot; state.modeState = buf.modeState;
    state.thinking = buf.thinking; state.tokens = buf.tokens; state.queued = buf.queued || [];
    state.activePersona = buf.activePersona || null;
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
      loadWorkingSetFrom(getBuffer(realId));
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
      if (meta && (meta.title === "新对话" || meta.title === "New chat")) {
        var firstUser = msgs.find(function (m) { return m.role === "user"; });
        var text = firstUser && firstUser.content && firstUser.content.find(function (c) { return c.type === "text"; });
        if (text && text.text) {
          var newTitle = text.text.slice(0, 20);
          await invoke("rename_session", { id: sid, title: newTitle });
          meta.title = newTitle;
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

  async function createNewSession() {
    if (state.activeSessionId && state.messages.length === 0) return;
    // 多 session 并发:不再因「正在响应中」拦截新建。旧 session 转入后台,在自己的
    // engine 上继续跑(其工作集已存进 sessionStates),新 session 用全新工作集。
    try {
      var meta = await invoke("create_session");
      switchActiveTo(meta.id, { fresh: true });
      await refreshHistoryList();
      await syncModeState();
      await syncActivePersona();
      notify();
    } catch (e) {
      addSystemItem(bt("newChatFailed") + e);
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
      resetPendingAssistant();
      state.chatItems = [];
      state.artifacts = Array.isArray(saved.artifacts) ? saved.artifacts.map(function (a) {
        var p = typeof a === "string" ? a : (a.storage_path || a.path || "");
        return { path: p, basename: basename(p) };
      }) : [];
      rerenderFromMessages();
      await syncModeState();
      await syncActivePersona();
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
      state.sessions = state.sessions.filter(function (s) { return s.id !== id; });
      if (state.activeSessionId === id) {
        state.activeSessionId = null;
        state.messages = [];
        state.chatItems = [];
        if (state.sessions.length > 0) {
          await switchToSession(state.sessions[0].id);
        } else {
          await createNewSession();
        }
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
    for (var mi = 0; mi < state.messages.length; mi++) {
      emitPersonaAt(mi, false); // 该消息之前发生的卡牌事件先插
      var m = state.messages[mi];
      var blocks = Array.isArray(m.content) ? m.content : [];
      if (m.role === "user") {
        var textParts = blocks.filter(function (c) { return c.type === "text"; }).map(function (c) { return c.text; });
        var utext = textParts.join("");
        if (textParts.length) {
          addChatItem({ type: "user", text: utext, time: "" });
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
              addChatItem({
                type: "artifact_card",
                path: (b.input && b.input.path) || "",
                title: (b.input && b.input.title) || "",
                description: (b.input && b.input.description) || "",
                time: "",
              });
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
          if ((b.name === "write_file" || b.name === "append_file")) {
            var wres = resultById[b.id];
            if (!(wres && wres.is_error)) {
              var wap = extractArtifactPath(b.input);
              var wprev = wap ? findPresentedArtifact(wap) : null;
              if (wprev) {
                addChatItem({
                  type: "artifact_card", path: wprev.path, title: wprev.title,
                  description: wprev.description, time: "",
                });
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
  function trackArtifact(path) {
    if (!path) return;
    if (state.artifacts.some(function (a) { return a.path === path; })) return;
    state.artifacts.push({ path: path, basename: basename(path) });
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
      var have = {};
      state.artifacts.forEach(function (a) { have[a.path] = true; });
      var added = false;
      files.forEach(function (p) { if (!have[p]) { state.artifacts.push({ path: p, basename: basename(p) }); added = true; } });
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
  function doSendFor(sid, text, displayText, attachmentsPayload) {
    runSyncOnSession(sid, function () {
      addChatItem({ type: "user", text: displayText, time: timeStr() });
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
    notify();
    doSendFor(sid, text, displayText, attachments);
  }

  async function sendMessage(text) {
    text = (text || "").trim();
    var readyAttachments = state.attachments.filter(function (a) { return a.status === "ready" && a.result; });
    if (!text && readyAttachments.length === 0) return;
    // 还有解析中的附件 → 等
    if (state.attachments.some(function (a) { return a.status === "parsing"; })) {
      addSystemItem(bt("attachStillParsing"));
      return;
    }

    if (!state.activeSessionId) {
      await createNewSession();
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
      state.queued.push({ id: ++itemIdSeq, text: text, displayText: displayText, attachments: attachmentsPayload });
      notify();
      return;
    }

    await doSendFor(state.activeSessionId, text, displayText, attachmentsPayload);
  }
  // 撤销一条待发消息(点 chip 的 ✕)。
  function removeQueued(id) {
    state.queued = state.queued.filter(function (q) { return q.id !== id; });
    notify();
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
      if (meta && (meta.title === "新对话" || meta.title === "New chat")) {
        var firstUser = state.messages.find(function (m) { return m.role === "user"; });
        var text = firstUser && firstUser.content && firstUser.content.find(function (c) { return c.type === "text"; });
        if (text && text.text) {
          var newTitle = text.text.slice(0, 20);
          await invoke("rename_session", { id: state.activeSessionId, title: newTitle });
          meta.title = newTitle;
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

  listen("chat:tool_start", function (e) { onSessionEvent(e, function () {
    var p = e.payload || {};
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
        addChatItem({
          type: "artifact_card",
          path: (meta.args && meta.args.path) || "",
          title: (meta.args && meta.args.title) || "",
          description: (meta.args && meta.args.description) || "",
          time: timeStr(),
        });
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

    // write_file / append_file 成功 → 加入产物列表
    if (p.success && meta && (meta.name === "write_file" || meta.name === "append_file")) {
      var ap = extractArtifactPath(meta.args);
      if (ap) {
        trackArtifact(ap);
        // 自动续卡:之前 present 过的成品被改了 → 再弹一张新成品卡(每次新卡,对齐
        // pinvou2),复用首次的 title/description(也复用首次可打开的 path)。
        var prevCard = findPresentedArtifact(ap);
        if (prevCard) {
          addChatItem({
            type: "artifact_card", path: prevCard.path, title: prevCard.title,
            description: prevCard.description, time: timeStr(),
          });
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
    var flags = { wasExecuting: false };
    runSyncOnSession(sid, function () {
      var error = e.payload && e.payload.error;
      if (error) addSystemItem("⚠️ " + error);
      flushAssistantMessageToHistory();
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
      // 执行 plan 完成 → 回 yolo 默认态(plan_phase 从 executing → none)
      if (state.modeState.plan_phase === "executing") {
        flags.wasExecuting = true;
        state.modeState = { mode: "yolo", plan_phase: "none" };
      }
    });
    notify();
    // 异步收尾(按 sid 路由,active/后台通用)
    (async function () {
      if (flags.wasExecuting) { try { await invoke("discard_plan", { sessionId: sid }); } catch (_) {} }
      await persistMessagesFor(sid);
      await refreshHistoryList();
      notify();
      // 排队式:本轮跑完,若该 session 不忙且有待发消息 → 自动发下一条
      flushQueued(sid);
    })();
  });

  listen("chat:usage", function (e) { onSessionEvent(e, function () {
    var input = Number(e.payload && e.payload.input_tokens || 0);
    if (input > 0) {
      state.tokens = { input: input, max: maxModelLen };
      notify();
    }
  }); });

  listen("chat:compaction", function (e) { onSessionEvent(e, function () {
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

  // chat:plan_ready —— 任一层快照非空就渲染方案卡（plan_phase → ready）
  listen("chat:plan_ready", function (e) { onSessionEvent(e, function () {
    var p = e.payload || {};
    state.modeState.plan_phase = "ready";
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

  // chat:plan_text_fallback —— Planning 态 AI 没调 plan 工具但 text 写了方案
  listen("chat:plan_text_fallback", function (e) { onSessionEvent(e, function () {
    var lastText = "";
    for (var i = state.messages.length - 1; i >= 0; i--) {
      if (state.messages[i].role === "assistant") {
        var parts = state.messages[i].content || [];
        for (var k = 0; k < parts.length; k++) { if (parts[k].type === "text" && parts[k].text) lastText += parts[k].text; }
        break;
      }
    }
    if (!lastText) return;
    var hit = PLAN_FALLBACK_KEYWORDS.some(function (kw) { return lastText.includes(kw); });
    if (!hit) return;
    if (hasUnresolvedItem("plan_text_fallback")) return;
    addChatItem({ type: "plan_text_fallback", text: lastText, resolved: false, time: timeStr() });
    notify();
  }); });

  // chat:execution_stuck —— Executing 自驱 N 次后仍卡
  listen("chat:execution_stuck", function (e) { onSessionEvent(e, function () {
    var p = e.payload || {};
    if (hasUnresolvedItem("execution_stuck")) return;
    addChatItem({ type: "execution_stuck", tries: p.auto_continue_tried || 0, resolved: false, time: timeStr() });
    notify();
  }); });

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

  async function pollMonitor() {
    try {
      var snap = await invoke("get_monitor_snapshot");
      // GPU util sliding window
      if (snap.gpu) {
        gpuUtilHistory.push(snap.gpu.utilization_pct);
        if (gpuUtilHistory.length > 5) gpuUtilHistory.shift();
        snap.gpu._utilMax = Math.max.apply(null, [0].concat(gpuUtilHistory));
      }
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
        vllmStatus: snap.vllm ? snap.vllm.status.toUpperCase() : "OFFLINE",
        vllmOnline: snap.vllm ? snap.vllm.status !== "offline" : false,
        vllmUpstream: snap.vllm ? (snap.vllm.upstream || "—") : "—",
        vllmMaxLen: snap.vllm ? (snap.vllm.max_model_len || "—") : "—",
        vllmQueue: snap.vllm
          ? (snap.vllm.num_requests_running != null ? snap.vllm.num_requests_running : "—") + " / " +
            (snap.vllm.num_requests_waiting != null ? snap.vllm.num_requests_waiting : "—") : "— / —",
        vllmKv: snap.vllm && snap.vllm.prefix_cache_hit_pct != null
          ? snap.vllm.prefix_cache_hit_pct.toFixed(1) + "%" : "—",
        appVersion: snap.app ? snap.app.pinvou3_version : "—",
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
      state.modeState = { mode: "yolo", plan_phase: "none" };
      return;
    }
    try {
      var ms = await invoke("get_mode_state", { sessionId: state.activeSessionId });
      state.modeState = { mode: ms.mode || "yolo", plan_phase: ms.plan_phase || "none" };
    } catch (e) {
      state.modeState = { mode: "yolo", plan_phase: "none" };
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
    state.modeState = {
      mode: st.mode || "yolo",
      plan_phase: st.plan_phase || "none",
    };
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
    if (itemId) patchItemById(itemId, { cardState: "frozen", statusLabel: bt("exitedPlan"), resolved: true });
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
    if (!state.activeSessionId) return;
    try {
      var st = await invoke("set_plan_mode_next", { sessionId: state.activeSessionId });
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
  async function planFallbackAccept(itemId, text) {
    patchItemById(itemId, { resolved: true, statusLabel: bt("adoptingPlan") }); notify();
    await acceptPlan(null, text || "", bt("adoptEcho"));
  }
  async function planFallbackRetry(itemId) {
    patchItemById(itemId, { resolved: true }); notify();
    await sendMessage("请用 update_plan 工具把上面的方案重新输出一遍,我才能在卡片上决策。");
  }
  async function executionStuckReplan(itemId) {
    patchItemById(itemId, { resolved: true }); notify();
    await sendMessage("你卡住了。请重新用 update_plan 工具列方案,我们再开始。");
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
  function openContainingFolder(path) { return invoke("open_containing_folder", { path: path }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  function openInSystem(path) { return invoke("open_in_system", { path: path }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  // 仅放白名单 URL (metaso.cn / open.bochaai.com),后端 open_external_url 强制校验。
  function openExternalUrl(url) { return invoke("open_external_url", { url: url }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }
  // 外部打开产物：HTML 走 Tauri 独立窗口（绕沙箱），其他走系统应用
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
    if (!state.activeSessionId) { addSystemItem(bt("equipNoSession")); return; }
    var prev = state.activePersona; // 换卡前的旧专家(同 session 切换时先播报卸下)
    try {
      var card = await invoke("equip_persona", { sessionId: state.activeSessionId, personaId: personaId });
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
    state.updateDownloading = true; state.updateProgress = 0; state.updateError = null; notify();
    try {
      var debPath = await invoke("download_update", { info: state.updateInfo });
      state.updateProgress = 100; notify();
      await invoke("install_update", { debPath: debPath });
      state.updateReady = true;
    } catch (e) {
      state.updateError = String(e);
    }
    state.updateDownloading = false; notify();
  }
  function restartApp() {
    invoke("restart_app").catch(function () { /* restart 成功不会返回 */ });
  }

  // ── Init ─────────────────────────────────────────────────────────
  async function init() {
    await loadSettings();
    await refreshHistoryList();
    if (state.sessions.length > 0) {
      await switchToSession(state.sessions[0].id);
    } else {
      await createNewSession();
    }
    await refreshSuperPerm();
    loadPersonas(); // 预载卡池(让聊天里草稿"已存入"判定能查到同名自制卡), fire-and-forget
    pollBackendStatus();
    setInterval(pollBackendStatus, 10000);
    checkForUpdateSilently(); // fire-and-forget,不阻塞启动
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
    saveSettings: saveSettings,
    saveSettingsAndRestart: saveSettingsAndRestart,
    toggleSuperPerm: toggleSuperPerm,
    renderMarkdown: renderMarkdown,
    // Plan/YOLO
    acceptPlan: acceptPlan,
    discardPlan: discardPlan,
    exitPlanToYolo: exitPlanToYolo,
    setPlanModeNext: setPlanModeNext,
    planStuckReplan: planStuckReplan,
    planStuckGo: planStuckGo,
    planFallbackAccept: planFallbackAccept,
    planFallbackRetry: planFallbackRetry,
    executionStuckReplan: executionStuckReplan,
    // 用户交互
    submitUserInput: submitUserInput,
    cancelUserInput: cancelUserInput,
    // 编辑/压缩
    editLastTurn: editLastTurn,
    compactNow: compactNow,
    // 产物
    artifactInfo: artifactInfo,
    readArtifactText: readArtifactText,
    openContainingFolder: openContainingFolder,
    openInSystem: openInSystem,
    openArtifactExternal: openArtifactExternal,
    openExternalUrl: openExternalUrl,
    // 附件
    addAttachmentByPath: addAttachmentByPath,
    addPasteImage: addPasteImage,
    removeAttachment: removeAttachment,
    clearAttachments: clearAttachments,
    pickAndAttach: pickAndAttach,
    markResolved: markResolved,
    // 卡片池: 专家面具
    loadPersonas: loadPersonas,
    getPersonas: function () { return personaPoolCache; }, // 返回引用(只读),不进 notify 快照
    readPersonaBody: function (id) { return invoke("read_persona_body", { personaId: id }); }, // Side B: 详情拉完整正文
    equipPersona: equipPersona,
    unequipPersona: unequipPersona,
    // 用户自创卡
    createPersona: createPersona,
    updatePersona: updatePersona,
    deletePersona: deletePersona,
    // 应用内升级
    checkForUpdate: checkForUpdate,
    downloadAndInstallUpdate: downloadAndInstallUpdate,
    restartApp: restartApp,
  };

  // Auto-init after DOM ready
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    setTimeout(init, 0);
  }
})();
