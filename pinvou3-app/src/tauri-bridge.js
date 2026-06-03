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
  // 然后 DOMPurify 把整段 script 连同被卷进去的内容一起剥掉。后果:Pinvou 表格里 LLM
  // 写"在同一个 <script> 标签内……| CRITICAL | RAISED |"会让 CRITICAL/RAISED 那几格空掉。
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
    modeState: { mode: "yolo", plan_phase: "none", pinvou_review_enabled: false },
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
    },
    // 卡片池: 专家面具。activePersona = 当前 session 加持的专家卡(完整对象)或 null,
    // 驱动聊天室右上角挂件。
    activePersona: null,
    // personaPool 只放轻量元信息(loadState),1078 张卡放模块级 personaPoolCache,
    // 不进 notify() 的 JSON 深拷贝(否则每个流式 token 都克隆 ~950KB,卡顿)。
    personaPool: { loadState: "idle" }, // idle | loading | ready | error
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

  // ── Per-session 工作集缓冲（多 session 并发）────────────────────
  // active session 的工作集 = state.* + 上面那批模块级 stream 变量(保持原逻辑零改动)。
  // 后台 session 的工作集存在 sessionStates[id];后台事件进来时临时把工作集切到对应
  // buffer 跑同步逻辑再切回(saveWorkingSetTo/loadWorkingSetFrom),期间 suppressNotify
  // 避免把后台渲染成 active。异步收尾(落盘/品悟)按显式 session_id 路由,不依赖工作集。
  var sessionStates = {};
  var suppressNotify = false;
  function freshBuffer() {
    return {
      messages: [], chatItems: [], personaEvents: [], artifacts: [], busy: false, queued: [],
      planSnapshot: { plan: null, todos: null },
      modeState: { mode: "yolo", plan_phase: "none", pinvou_review_enabled: false },
      thinking: { active: false, phase: "thinking", toolName: "", startedAt: 0 },
      tokens: { input: 0, max: maxModelLen },
      activePersona: null, // 卡片池: 该 session 加持的专家面具(挂件用)

      stream: {
        currentStreamText: "", currentStreamId: 0, pendingAssistantText: "",
        pendingAssistantBlocks: [], itemIdSeq: 0, toolMeta: {},
        pendingAssistantPersona: null, pendingPinvouReview: null, pendingFinalReview: false,
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
      pendingAssistantPersona: pendingAssistantPersona, pendingPinvouReview: pendingPinvouReview,
      pendingFinalReview: pendingFinalReview,
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
    pendingAssistantPersona = s.pendingAssistantPersona || null;
    pendingPinvouReview = s.pendingPinvouReview || null;
    pendingFinalReview = s.pendingFinalReview || false;
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
    try {
      state.workflow.bindings = await invoke("list_session_skill_bindings");
    } catch (e) { /* 无绑定 */ }
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
      await syncSessionSkill();
      await syncActivePersona();
      notify();
    } catch (e) {
      addSystemItem("⚠️ 新建对话失败: " + e);
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
      await syncSessionSkill();
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
      await syncSessionSkill();
      await syncActivePersona();
      notify();
      reconcileArtifacts(id); // 对账磁盘产物(修重启/跟踪遗漏导致的面板缺文件)
    } catch (e) {
      addSystemItem("⚠️ 加载对话失败: " + e);
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
      addSystemItem("⚠️ 删除失败: " + e);
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
  // 品悟触发 echo（dispatchPinvouTrigger persist 进 messages）是重建品悟紫色气泡的锚点：
  // 紧跟其后的 assistant 消息即对应 persona 的品悟回复。
  var PINVOU_TRIGGER_PERSONA = { "🟣 触发品悟审方案": "pinvou-plan", "🟣 触发品悟验收": "pinvou-final" };

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
        else if (ev.kind === "unequip") addChatItem({ type: "system", text: "🎴 已卸下专家卡牌: " + (ev.name || ""), time: "" });
      }
    }
    var pendingPersona = null; // 品悟触发 echo 命中后，标记下一条 assistant 为品悟回复
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
        if (PINVOU_TRIGGER_PERSONA[utext]) pendingPersona = PINVOU_TRIGGER_PERSONA[utext];
        // tool_result（只回填普通工具卡；选择卡/方案卡的结果已在 tool_use 处还原）
        for (var ci = 0; ci < blocks.length; ci++) {
          var c = blocks[ci];
          if (c.type !== "tool_result") continue;
          var tm = toolMeta[c.tool_use_id];
          if (tm) {
            updateToolItem(c.tool_use_id, c.content, !c.is_error);
          }
        }
        continue;
      }
      if (m.role !== "assistant") continue;
      var msgPersona = pendingPersona; // 本条若是品悟回复，气泡带 persona 还原紫色样式
      pendingPersona = null;
      var textBuf = "";
      var planSnap = null, todosSnap = null, sawPlanTool = false;
      for (var bi = 0; bi < blocks.length; bi++) {
        var b = blocks[bi];
        if (b.type === "text") {
          textBuf += b.text;
        } else if (b.type === "tool_use") {
          if (textBuf) {
            addChatItem({ type: "assistant", html: renderMarkdown(textBuf), time: "", streaming: false, persona: msgPersona });
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
        addChatItem({ type: "assistant", html: renderMarkdown(textBuf), time: "", streaming: false, persona: msgPersona });
      }
      // 本条 assistant 消息用过 plan 工具 → 还原一张只读历史方案卡
      if (sawPlanTool && (planSnap || todosSnap)) {
        var snaps = { plan: planSnap, todos: todosSnap };
        addChatItem({
          type: "plan_card", plan: planSnap, todos: todosSnap,
          planMarkdown: composePlanMarkdown(snaps),
          cardState: "frozen", resolved: true, statusLabel: "📜 历史方案", time: "",
        });
      }
      // 品悟审方案回复后 → 还原只读操作卡（决策痕迹由后续 user echo 体现，按钮不可再点）
      if (msgPersona === "pinvou-plan") {
        addChatItem({ type: "pinvou_actions", resolved: true, statusLabel: "📜 已审阅", planMarkdown: null, report: null, time: "" });
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

  // ── 品悟审批辅助 ─────────────────────────────────────────────────
  var PINVOU_REVIEW_RE = /## PINVOU REVIEW REPORT[\s\S]*$/m;
  function extractPinvouReport(text) {
    if (!text) return null;
    var m = text.match(PINVOU_REVIEW_RE);
    return m ? m[0].trim() : null;
  }
  function overrideAllCriticalInReport(report) {
    return report.replace(
      /^(\|[^|]*\|\s*CRITICAL\s*\|\s*)RAISED(\s*\|[^|]*\|)$/gim, "$1OVERRIDDEN_BY_USER$2"
    ).replace(
      /^(\|[^|]*\|\s*CRITICAL\s*\|\s*OVERRIDDEN_BY_USER\s*\|\s*)[^|]*(\|)$/gim, "$1用户拍板继续$2"
    );
  }
  function synthesizeOverriddenReport(hint) {
    return "## PINVOU REVIEW REPORT\n\n| Finding | Severity | Status | User Decision |\n|---------|----------|--------|---------------|\n| " +
      (hint || "Pinvou 输出未按表格格式,用户已阅读并 override") + " | CRITICAL | OVERRIDDEN_BY_USER | 用户拍板继续 |\n\n**VERDICT**: user override —— Pinvou 未按格式输出表格,用户已读完意见后强制放行";
  }
  function buildPinvouPlanPrompt(body, planMarkdown) {
    return "[品悟自动触发 /pinvou-review-plan,完整角色定义如下]\n\n" + body +
      "\n\n---\n\n你现在只审下面这份 plan。不要把触发语或历史里的按钮文案当成用户批准;只有后续明确的用户决策才算批准。\n\n<plan_markdown>\n" +
      (planMarkdown || "（plan 为空）") +
      "\n</plan_markdown>\n\n按上面的 /pinvou-review-plan 格式输出。";
  }
  function buildPinvouFinalPrompt(body, artifacts) {
    var artifactLines = (artifacts || []).map(function (a) { return "- " + (a.path || a.basename || "unknown"); }).join("\n");
    return "[品悟自动触发 /pinvou-review-final,完整角色定义如下]\n\n" + body +
      "\n\n---\n\n这次是任务收口验收。你通过只读工具核验真实产物,不要修改文件、不要继续执行任务。\n\n当前前端跟踪到的产物:\n" +
      (artifactLines || "（无前端跟踪产物;请用只读工具按上下文核验）") +
      "\n\n按上面的 /pinvou-review-final 格式输出。";
  }
  function parseGateError(err) {
    var s = typeof err === "string" ? err : (err && err.toString ? err.toString() : "");
    if (s.indexOf("gate_error") < 0) return null;
    try { return JSON.parse(s); } catch (e) { return null; }
  }
  function lastAssistantText() {
    for (var i = state.messages.length - 1; i >= 0; i--) {
      if (state.messages[i].role === "assistant") {
        var parts = state.messages[i].content || [];
        var buf = "";
        for (var k = 0; k < parts.length; k++) { if (parts[k].type === "text" && parts[k].text) buf += parts[k].text; }
        return buf;
      }
    }
    return "";
  }
  // 品悟触发态（内部）
  var pendingAssistantPersona = null;   // null | "pinvou-plan" | "pinvou-final"
  var pendingPinvouReview = null;       // { planMarkdown }
  var pendingFinalReview = false;

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
      addSystemItem("⚠️ 附件还在解析,请稍后再发");
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
        persona: pendingAssistantPersona,
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

  // chat:done 特殊:同步收尾(flush/busy=false/品悟卡/mode 复位)走 runSyncOnSession
  // 路由到对应 session;异步收尾(discard_plan/品悟终审/落盘/刷新列表)按显式 sid 路由,
  // 不依赖工作集 —— 这样后台 session 跑完也能正确落盘 + 触发终审。
  listen("chat:done", function (e) {
    var sid = (e.payload && e.payload.session_id) || state.activeSessionId;
    var flags = { wasExecuting: false, doFinal: false };
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
      pendingAssistantPersona = null;
      // 品悟审方案完成 → 在紫色品悟气泡后附 3 按钮卡
      if (pendingPinvouReview) {
        var report = extractPinvouReport(lastAssistantText());
        addChatItem({ type: "pinvou_actions", planMarkdown: pendingPinvouReview.planMarkdown, report: report, resolved: false, statusLabel: "", time: timeStr() });
        pendingPinvouReview = null;
      }
      // final review 是 advisory，跑完只清 flag，不附按钮
      if (pendingFinalReview) { pendingFinalReview = false; }
      // 执行 plan 完成 → 回 yolo 默认态(plan_phase 从 executing → none)
      if (state.modeState.plan_phase === "executing") {
        flags.wasExecuting = true;
        state.modeState = { mode: "yolo", plan_phase: "none", pinvou_review_enabled: state.modeState.pinvou_review_enabled };
      }
      // 任务收口:开了品悟审批 → 自动 advisory final review(防递归靠 wasExecuting:终审 turn 不在 executing 态)
      flags.doFinal = flags.wasExecuting && state.modeState.pinvou_review_enabled;
    });
    notify();
    // 异步收尾(按 sid 路由,active/后台通用)
    (async function () {
      if (flags.wasExecuting) { try { await invoke("discard_plan", { sessionId: sid }); } catch (_) {} }
      if (flags.doFinal) { await autoTriggerPinvouFinalFor(sid); }
      await persistMessagesFor(sid);
      await refreshHistoryList();
      notify();
      // 排队式:本轮跑完,若该 session 不忙(没被 doFinal 又起新 turn)且有待发消息 → 自动发下一条
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
    var auto = e.payload && e.payload.auto ? "（自动）" : "";
    if (phase === "start") addSystemItem("⏳ 正在压缩上下文" + auto + " " + msg);
    else if (phase === "done") addSystemItem("✓ 上下文压缩完成" + auto + " " + msg);
    else if (phase === "fail") addSystemItem("⚠️ 压缩失败" + auto + ": " + msg);
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
        it.cardState = "frozen"; it.statusLabel = "📜 已被新方案覆盖";
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

  // chat:phase_changed —— 底座从 LLM 回复抽 <phase id="..."/> marker 触发。
  // workflow phase chips 是全局(跟 active skill 走),后台 session 的 phase 变更不动 active chips。
  listen("chat:phase_changed", function (e) {
    var sid = e.payload && e.payload.session_id;
    if (sid && sid !== state.activeSessionId) return;
    var phaseId = e.payload && (e.payload.phase_id || e.payload.phaseId);
    setCurrentPhase(phaseId, "llm");
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
        gpuName: snap.gpu ? snap.gpu.name : "GPU 信息不可用",
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
        ? "⚠️ 超级权限已开启"
        : "超级权限已关闭");
    } catch (e) {
      addSystemItem("⚠️ " + e);
      try { state.superPermEnabled = !!(await invoke("get_super_permission_status")); } catch (e2) {}
    }
    notify();
  }

  // ── Mode state ───────────────────────────────────────────────────
  async function syncModeState() {
    if (!state.activeSessionId) {
      state.modeState = { mode: "yolo", plan_phase: "none", pinvou_review_enabled: false };
      return;
    }
    try {
      var ms = await invoke("get_mode_state", { sessionId: state.activeSessionId });
      state.modeState = { mode: ms.mode || "yolo", plan_phase: ms.plan_phase || "none", pinvou_review_enabled: !!ms.pinvou_review_enabled };
    } catch (e) {
      state.modeState = { mode: "yolo", plan_phase: "none", pinvou_review_enabled: false };
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
  // 品悟链路有多个 await 边界,用户可能中途切 session。所有 UI 写入(chatItem 增改、
  // pending* 标记、modeState 同步)必须落在【触发 session】的 buffer 上,不能跟着
  // state.activeSessionId 漂走。一律 wrap 进 runSyncOnSession 是因为:sid === active
  // 时它是 no-op 直通,sid !== active 时它 swap-load-fn-save 回 sid 的 buffer。
  function runOnSession(sid, fn) { runSyncOnSession(sid || state.activeSessionId, fn); }
  function addSystemItemFor(sid, text) { runOnSession(sid, function () { addSystemItem(text); }); }
  function patchItemByIdFor(sid, id, patch) { runOnSession(sid, function () { patchItemById(id, patch); }); }
  function modeStateFor(sid) {
    if (!sid || sid === state.activeSessionId) return state.modeState;
    var buf = sessionStates[sid];
    return buf ? buf.modeState : state.modeState;
  }
  // unresolved_critical / accept_plan 二次失败时,把品悟决策入口重新摆到用户面前。
  // 优先复活最近的 pinvou_actions 卡(让 override/revise/加一句 按钮重新可点);没卡就
  // 新插一张 —— 避免用户落在「一行红字 + 没按钮」的死胡同。
  function surfacePinvouActionsCardFor(sid, planMarkdown, report, hint) {
    runOnSession(sid, function () {
      var revived = false;
      for (var i = state.chatItems.length - 1; i >= 0; i--) {
        if (state.chatItems[i].type === "pinvou_actions") {
          Object.assign(state.chatItems[i], {
            resolved: false, statusLabel: "",
            planMarkdown: planMarkdown,
            report: report || state.chatItems[i].report,
          });
          revived = true;
          break;
        }
      }
      if (!revived) {
        addChatItem({
          type: "pinvou_actions",
          planMarkdown: planMarkdown, report: report,
          resolved: false, statusLabel: "", time: timeStr(),
        });
      }
      if (hint) addSystemItem(hint);
    });
  }

  // ── 思考指示器状态（每次阶段切换重置计时）──────────────────────
  function startThinking() { state.thinking = { active: true, phase: "thinking", toolName: "", startedAt: Date.now() }; }
  function thinkingTool(name) { state.thinking = { active: true, phase: "tool", toolName: name || "", startedAt: Date.now() }; }
  function thinkingIdle() { state.thinking = { active: true, phase: "thinking", toolName: "", startedAt: Date.now() }; }
  function stopThinking() { state.thinking = { active: false, phase: "thinking", toolName: "", startedAt: 0 }; }
  function applyModeFromState(st) {
    state.modeState = {
      mode: st.mode || "yolo",
      plan_phase: st.plan_phase || "none",
      pinvou_review_enabled: st.pinvou_review_enabled != null ? !!st.pinvou_review_enabled : state.modeState.pinvou_review_enabled,
    };
  }
  async function preflightPinvouGate(sid, planMarkdown) {
    if (!sid || !modeStateFor(sid).pinvou_review_enabled) return null;
    try {
      await invoke("check_pinvou_exit_gate", { sessionId: sid, planMarkdown: planMarkdown || "" });
      return null;
    } catch (e) {
      return parseGateError(e) || { gate_error: "unknown", message: String(e) };
    }
  }

  // ── Plan/YOLO 命令 ───────────────────────────────────────────────
  // sid 在 entry 捕获一次,thread through 所有 await —— 防用户切 session 后,
  // 后续 UI 写入/IPC 把卡片塞到错误的 session。
  async function acceptPlan(itemId, planMarkdown, echo) {
    var sid = state.activeSessionId;
    if (!sid) return;
    var gate = await preflightPinvouGate(sid, planMarkdown || "");
    if (gate) {
      if (itemId) patchItemByIdFor(sid, itemId, { cardState: "active", statusLabel: "", resolved: false });
      if (gate.gate_error === "missing_review_report" || gate.gate_error === "malformed_report") {
        notify();
        await autoTriggerPinvouReview(sid, planMarkdown || "");
        return;
      }
      if (gate.gate_error === "unresolved_critical") {
        // planMarkdown 已带旧 report 但 CRITICAL 还在 RAISED —— 复活品悟决策卡,
        // 让用户走 override / revise / 加一句出口,而不是落在系统提示死胡同。
        var n = (gate.detail && gate.detail.unresolved_count) || "?";
        surfacePinvouActionsCardFor(sid, planMarkdown || "", null,
          "⚠️ 品悟还有 " + n + " 个 CRITICAL 没拍板 —— 已重新打开决策卡");
        notify();
        return;
      }
      addSystemItemFor(sid, "⚠️ 品悟放行检查阻塞: " + (gate.message || ""));
      notify();
      return;
    }
    if (itemId) patchItemByIdFor(sid, itemId, { cardState: "approved", statusLabel: "✅ 已批准", resolved: true });
    runOnSession(sid, function () { pushUserEcho(echo || "✅ 就这么干", true); state.busy = true; startThinking(); });
    notify();
    try {
      var st = await invoke("accept_plan", { sessionId: sid, planMarkdown: planMarkdown || "" });
      runOnSession(sid, function () { applyModeFromState(st); });
    } catch (e) {
      // Pinvou EXIT GATE：accept 前必须先有 review report
      var gate2 = parseGateError(e);
      if (itemId) patchItemByIdFor(sid, itemId, { cardState: "active", statusLabel: "", resolved: false });
      runOnSession(sid, function () { state.busy = false; });
      if (gate2 && gate2.gate_error === "missing_review_report") {
        notify();
        await autoTriggerPinvouReview(sid, planMarkdown || "");
        return;
      }
      if (gate2 && gate2.gate_error === "unresolved_critical") {
        var n2 = (gate2.detail && gate2.detail.unresolved_count) || "?";
        surfacePinvouActionsCardFor(sid, planMarkdown || "", null,
          "⚠️ Pinvou EXIT GATE: 还有 " + n2 + " 个 CRITICAL 待拍板 —— 已重新打开决策卡");
        notify();
        return;
      }
      addSystemItemFor(sid, gate2 ? ("⚠️ Pinvou EXIT GATE 阻塞: " + (gate2.message || "")) : ("⚠️ accept_plan 失败: " + e));
    }
    notify();
  }
  async function discardPlan(itemId) {
    if (itemId) patchItemById(itemId, { cardState: "frozen", statusLabel: "🚪 已退出 Plan", resolved: true });
    if (!state.activeSessionId) { notify(); return; }
    try {
      var st = await invoke("discard_plan", { sessionId: state.activeSessionId });
      applyModeFromState(st);
    } catch (e) { addSystemItem("⚠️ discard_plan 失败: " + e); }
    notify();
  }
  async function exitPlanToYolo() {
    if (!state.activeSessionId) return;
    try {
      var st = await invoke("exit_plan_to_yolo", { sessionId: state.activeSessionId });
      applyModeFromState(st);
    } catch (e) { addSystemItem("⚠️ 退出 Plan 失败: " + e); }
    notify();
  }
  // 灯泡 toggle：plan ↔ yolo
  async function setPlanModeNext() {
    if (!state.activeSessionId) return;
    try {
      var st = await invoke("set_plan_mode_next", { sessionId: state.activeSessionId });
      applyModeFromState(st);
    } catch (e) { addSystemItem("⚠️ 切换模式失败: " + e); }
    notify();
  }
  // plan-stuck / fallback / execution-stuck 卡片动作
  async function planStuckReplan(itemId) {
    patchItemById(itemId, { resolved: true, statusLabel: "📋 让 AI 重出方案…" }); notify();
    await sendMessage("请用 update_plan 工具输出完整方案,不要直接调写工具。");
  }
  async function planStuckGo(itemId) {
    patchItemById(itemId, { resolved: true }); notify();
    await exitPlanToYolo();
    await sendMessage("按上面讨论的方案继续执行任务,直接写文件/跑命令,不要再讨论方案。");
  }
  async function planFallbackAccept(itemId, text) {
    patchItemById(itemId, { resolved: true, statusLabel: "✅ 采纳中..." }); notify();
    await acceptPlan(null, text || "", "✅ 采纳此方案");
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
    try { await invoke("compact_now", { sessionId: state.activeSessionId }); } catch (e) { addSystemItem("⚠️ 压缩失败: " + e); }
  }

  // ── 产物面板 ─────────────────────────────────────────────────────
  function artifactInfo(path) { return invoke("artifact_info", { path: path }); }
  function readArtifactText(path) { return invoke("read_artifact_text", { path: path }); }
  function openContainingFolder(path) { return invoke("open_containing_folder", { path: path }).catch(function (e) { addSystemItem("⚠️ 打开目录失败: " + e); }); }
  function openInSystem(path) { return invoke("open_in_system", { path: path }).catch(function (e) { addSystemItem("⚠️ 打开失败: " + e); }); }
  // 仅放白名单 URL (metaso.cn / open.bochaai.com),后端 open_external_url 强制校验。
  function openExternalUrl(url) { return invoke("open_external_url", { url: url }).catch(function (e) { addSystemItem("⚠️ 打开链接失败: " + e); }); }
  // 外部打开产物：HTML 走 Tauri 独立窗口（绕沙箱），其他走系统应用
  function openArtifactExternal(path) {
    var ext = (String(path).split(".").pop() || "").toLowerCase();
    var cmd = (ext === "html" || ext === "htm") ? "open_artifact_window" : "open_in_system";
    return invoke(cmd, { path: path }).catch(function (e) { addSystemItem("⚠️ 打开失败: " + e); });
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
    } catch (e) { addSystemItem("⚠️ 粘贴图片失败: " + e); }
  }
  function removeAttachment(id) {
    state.attachments = state.attachments.filter(function (a) { return a.id !== id; });
    notify();
  }
  function clearAttachments() { state.attachments = []; }
  // 打开系统文件选择器并摄入为附件
  async function pickAndAttach() {
    if (!dialogOpen) { addSystemItem("⚠️ 文件选择不可用"); return; }
    try {
      var selected = await dialogOpen({ multiple: true });
      if (!selected) return;
      var paths = Array.isArray(selected) ? selected : [selected];
      for (var i = 0; i < paths.length; i++) { await addAttachmentByPath(paths[i]); }
    } catch (e) { addSystemItem("⚠️ 选择文件失败: " + e); }
  }

  // ── 品悟审批（基础封装；GATE 编排在 React 卡片层）────────────────
  function readSkillBody(name) { return invoke("read_skill_body", { name: name }); }
  async function setPinvouReview(enabled) {
    if (!state.activeSessionId) return;
    try {
      var st = await invoke("set_pinvou_review", { sessionId: state.activeSessionId, enabled: !!enabled });
      if (st && st.mode) applyModeFromState(st);
      else state.modeState.pinvou_review_enabled = !!enabled;
    } catch (e) { addSystemItem("⚠️ 设置品悟审批失败: " + e); }
    notify();
  }
  function togglePinvouReview() { return setPinvouReview(!state.modeState.pinvou_review_enabled); }

  // 共用底层：前端 user 气泡只显示简短摘要，完整 prompt 发给后端（本地小模型 eager loading）。
  // *For(sid) 变体支持后台 session(chat:done 触发的自动终审):同步设置发送态走
  // runSyncOnSession 路由到对应 session,invoke chat 带显式 session_id。
  function dispatchPinvouTriggerFor(sid, persona, summary, fullPrompt) {
    if (!sid) return Promise.resolve();
    var canSend = true;
    runSyncOnSession(sid, function () {
      if (state.busy) { canSend = false; return; }
      pendingAssistantPersona = persona;
      pushUserEcho(summary, true);
      state.busy = true;
      startThinking();
      currentStreamText = "";
      currentStreamId = ++itemIdSeq;
      state.chatItems.push({ id: currentStreamId, type: "assistant", html: "", time: timeStr(), streaming: true, persona: persona });
    });
    if (!canSend) return Promise.resolve();
    notify();
    return invoke("pinvou_review_chat", { message: fullPrompt, sessionId: sid })
      .catch(function (e) { runSyncOnSession(sid, function () { addSystemItem("⚠️ " + e); state.busy = false; }); notify(); });
  }
  function dispatchPinvouTrigger(persona, summary, fullPrompt) {
    return dispatchPinvouTriggerFor(state.activeSessionId, persona, summary, fullPrompt);
  }
  async function autoTriggerPinvouReview(sid, planMarkdown) {
    if (!sid) return;
    runOnSession(sid, function () {
      pendingPinvouReview = { planMarkdown: planMarkdown };
      addSystemItem("🟣 品悟还没看过这个方案 —— 自动让品悟先看一眼...");
    });
    var fullPrompt = "/pinvou-review-plan";
    try { var body = await invoke("read_skill_body", { name: "pinvou-review-plan" }); fullPrompt = buildPinvouPlanPrompt(body, planMarkdown); }
    catch (e) { addSystemItemFor(sid, "⚠️ 加载 pinvou-review-plan skill 失败: " + e); }
    await dispatchPinvouTriggerFor(sid, "pinvou-plan", "🟣 触发品悟审方案", fullPrompt);
  }
  async function autoTriggerPinvouFinalFor(sid) {
    if (!sid) return;
    var artifacts = [];
    runSyncOnSession(sid, function () {
      pendingFinalReview = true;
      artifacts = (state.artifacts || []).slice();
      addSystemItem("🟣 任务完成 —— 让品悟核验一下产出...");
    });
    notify();
    var fullPrompt = "/pinvou-review-final";
    try { var body = await invoke("read_skill_body", { name: "pinvou-review-final" }); fullPrompt = buildPinvouFinalPrompt(body, artifacts); }
    catch (e) { runSyncOnSession(sid, function () { addSystemItem("⚠️ 加载 pinvou-review-final skill 失败: " + e); }); }
    await dispatchPinvouTriggerFor(sid, "pinvou-final", "🟣 触发品悟验收", fullPrompt);
  }
  async function autoTriggerPinvouFinal() {
    await autoTriggerPinvouFinalFor(state.activeSessionId);
  }
  // 品悟 3 按钮之「✅ 确认继续执行」：override 所有 CRITICAL 后 accept_plan
  async function pinvouAcceptOverride(itemId, planMarkdown, report) {
    var sid = state.activeSessionId;
    patchItemByIdFor(sid, itemId, { resolved: true, statusLabel: "👍 已确认品悟的顾虑,继续执行..." });
    if (!sid) { notify(); return; }
    var eff = report ? overrideAllCriticalInReport(report) : synthesizeOverriddenReport("Pinvou 用自然语言提了意见(见上方),用户阅读后决策");
    var fullMd = (planMarkdown || "") + "\n\n" + eff;
    var gate = await preflightPinvouGate(sid, fullMd);
    // malformed_report / unresolved_critical 都说明 override regex 没拿下整张表
    // (字段拼写偏、列序换、行数对不齐)。用户已经按下"确认继续",synthesize 一张全新的
    // OVERRIDDEN_BY_USER 单行表二次冲门 —— 把第一次的形式失败治住。
    if (gate && report && (gate.gate_error === "malformed_report" || gate.gate_error === "unresolved_critical")) {
      eff = synthesizeOverriddenReport("Pinvou 报告无法自动 override(格式异常 / 部分行未对齐),用户阅读后强制放行");
      fullMd = (planMarkdown || "") + "\n\n" + eff;
      gate = await preflightPinvouGate(sid, fullMd);
    }
    if (gate) {
      surfacePinvouActionsCardFor(sid, planMarkdown || "", report,
        "⚠️ 品悟放行检查仍阻塞: " + (gate.message || "") + " —— 已重新打开决策卡");
      notify();
      return;
    }
    runOnSession(sid, function () { pushUserEcho("✅ 就这么干(已确认品悟的顾虑)", true); state.busy = true; startThinking(); });
    notify();
    try {
      var st = await invoke("accept_plan", { sessionId: sid, planMarkdown: fullMd });
      runOnSession(sid, function () { applyModeFromState(st); });
    } catch (e) {
      addSystemItemFor(sid, "⚠️ accept_plan 仍失败: " + e);
      runOnSession(sid, function () { state.busy = false; });
    }
    notify();
  }

  // 品悟 3 按钮之「↻ AI 改方案」/「⊕ 我加一句」共用:user 已表达修订意图。
  // userComment 非空 = 「⊕ 我加一句」路径,把用户那句话也喂进修订指令(不是落进普通 chat)。
  // 仍要求模型只调用 update_plan 重出方案卡,等用户下一次拍板,不直接执行。
  async function pinvouRevisePlan(itemId, planMarkdown, report, userComment) {
    var sid = state.activeSessionId;
    var hasComment = !!(userComment && userComment.trim());
    patchItemByIdFor(sid, itemId, {
      resolved: true,
      statusLabel: hasComment ? "⊕ 已带上你的意见,正在让 AI 修订..." : "↻ 正在让 AI 修订方案...",
    });
    if (!sid) { notify(); return; }
    var displayText = hasComment ? ("⊕ 我也担心: " + userComment.trim()) : "↻ 按品悟意见改方案";
    var instruction =
      "根据品悟意见修订当前方案。只更新方案,不要执行文件写入或命令。\n" +
      "你必须调用 update_plan 输出新版方案卡,然后停下来等用户拍板。\n\n" +
      "当前方案:\n" + (planMarkdown || "（plan 为空）") + "\n\n" +
      "品悟意见:\n" + (report || "见上方品悟审查意见") +
      (hasComment ? ("\n\n用户补充意见(必须在新方案里回应):\n" + userComment.trim()) : "");
    try {
      var st = await invoke("set_plan_mode_next", { sessionId: sid });
      runOnSession(sid, function () { applyModeFromState(st); });
      // 排队判定按 sid 自己的 busy(不是 active 的),避免「触发 session 后台还在跑」时
      // 误把 instruction 当成立即可发。
      var sidBusy = sid === state.activeSessionId ? state.busy : !!(sessionStates[sid] && sessionStates[sid].busy);
      if (sidBusy) {
        runOnSession(sid, function () { state.queued.push({ id: ++itemIdSeq, text: instruction, displayText: displayText, attachments: [] }); });
        notify();
        return;
      }
      await doSendFor(sid, instruction, displayText, []);
    } catch (e) {
      patchItemByIdFor(sid, itemId, { resolved: false, statusLabel: "" });
      addSystemItemFor(sid, "⚠️ 触发方案修订失败: " + e);
      notify();
    }
  }

  // ── 工作流 ───────────────────────────────────────────────────────
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
  // 切换 session 后同步该 session 的 skill 绑定到 workflow 高亮
  async function syncSessionSkill() {
    if (!state.activeSessionId) return;
    try {
      var info = await invoke("get_session_active_skill", { sessionId: state.activeSessionId });
      if (info && info.name) {
        state.workflow.activeSkillName = info.name;
        state.workflow.phases = info.phases || [];
        state.workflow.currentPhaseId = info.current_phase_id || null;
        state.workflow.reachedPhaseIds = info.current_phase_id ? [info.current_phase_id] : [];
        state.workflow.bindings[state.activeSessionId] = info.name;
      } else {
        state.workflow.activeSkillName = null;
        state.workflow.phases = [];
        state.workflow.currentPhaseId = null;
        state.workflow.reachedPhaseIds = [];
      }
    } catch (e) { /* 旧 session 无绑定，忽略 */ }
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
  function personaName(p) { return (p && (p.name || p.cn_name)) || ""; }
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
    if (!state.activeSessionId) { addSystemItem("⚠️ 请先打开或新建一个对话再加持专家"); return; }
    var prev = state.activePersona; // 换卡前的旧专家(同 session 切换时先播报卸下)
    try {
      var card = await invoke("equip_persona", { sessionId: state.activeSessionId, personaId: personaId });
      // 同 session 换了一张不同的卡 → 先弹一条"已卸下旧专家",再弹新加持。
      if (prev && prev.id !== card.id) {
        addChatItem({ type: "system", text: "🎴 已卸下专家卡牌: " + personaName(prev), time: timeStr() });
        recordPersonaEvent({ kind: "unequip", name: personaName(prev) });
      }
      state.activePersona = card;
      addChatItem({ type: "persona_equip", card: card, time: timeStr() });
      recordPersonaEvent({ kind: "equip", card: card });
      notify();
      return card;
    } catch (e) { addSystemItem("⚠️ 加持失败: " + e); return null; }
  }
  // 摘下当前 session 的专家面具。
  async function unequipPersona() {
    if (!state.activeSessionId) return;
    var prev = state.activePersona;
    try { await invoke("unequip_persona", { sessionId: state.activeSessionId }); } catch (e) { /* 忽略,前端照样摘 */ }
    state.activePersona = null;
    if (prev) { addChatItem({ type: "system", text: "🎴 已卸下专家卡牌: " + personaName(prev), time: timeStr() }); recordPersonaEvent({ kind: "unequip", name: personaName(prev) }); }
    notify();
  }
  // 切换/重载 session 后,从后端拉该 session 的加持状态还原挂件(backend 是真相)。
  async function syncActivePersona() {
    if (!state.activeSessionId) { state.activePersona = null; return; }
    try {
      state.activePersona = await invoke("get_active_persona", { sessionId: state.activeSessionId }) || null;
    } catch (e) { /* 旧 session 无加持,忽略 */ }
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
    pollBackendStatus();
    setInterval(pollBackendStatus, 10000);
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
    // 品悟审批
    readSkillBody: readSkillBody,
    setPinvouReview: setPinvouReview,
    togglePinvouReview: togglePinvouReview,
    pinvouAcceptOverride: pinvouAcceptOverride,
    pinvouRevisePlan: pinvouRevisePlan,
    markResolved: markResolved,
    // 工作流
    loadSkills: loadSkills,
    activateSkill: activateSkill,
    deactivateSkill: deactivateSkill,
    openDemo: openDemo,
    closeDemo: closeDemo,
    setCurrentPhase: setCurrentPhase,
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
  };

  // Auto-init after DOM ready
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    setTimeout(init, 0);
  }
})();
