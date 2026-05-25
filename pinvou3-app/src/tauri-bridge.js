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
  function renderMarkdown(text) {
    if (!window.marked || !window.DOMPurify) return escapeHtml(text);
    return DOMPurify.sanitize(marked.parse(text || ""));
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
    // 输入框待发附件 [{ id, basename, status:'parsing'|'ready'|'error', result, error }]
    attachments: [],
    // token 预算（input_tokens / maxModelLen）
    tokens: { input: 0, max: 32768 },
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
  };

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

  // ── Pub/Sub ──────────────────────────────────────────────────────
  var subscribers = [];
  function notify() {
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
    if (state.busy) { addSystemItem("⚠️ 正在响应中，请等完成后再新建"); return; }
    try {
      var meta = await invoke("create_session");
      state.activeSessionId = meta.id;
      state.messages = [];
      state.chatItems = [];
      resetPendingAssistant();
      await refreshHistoryList();
      await syncModeState();
      await syncSessionSkill();
      notify();
    } catch (e) {
      addSystemItem("⚠️ 新建对话失败: " + e);
    }
  }

  async function switchToSession(id) {
    if (id === state.activeSessionId) return;
    if (state.busy) {
      try {
        await invoke("cancel_generation");
        await new Promise(function (r) { setTimeout(r, 500); });
      } catch (e) {
        addSystemItem("⚠️ 切换失败: " + e);
        return;
      }
    }
    try {
      var saved = await invoke("load_session", { id: id });
      state.activeSessionId = saved.metadata.id;
      state.messages = Array.isArray(saved.messages) ? saved.messages : [];
      resetPendingAssistant();
      state.chatItems = [];
      rerenderFromMessages();
      await syncModeState();
      await syncSessionSkill();
      notify();
    } catch (e) {
      addSystemItem("⚠️ 加载对话失败: " + e);
    }
  }

  async function deleteSession(id) {
    try {
      await invoke("delete_session", { id: id });
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

  // ── Rerender from messages (session restore) ─────────────────────
  function rerenderFromMessages() {
    state.chatItems = [];
    itemIdSeq = 0;
    for (var mi = 0; mi < state.messages.length; mi++) {
      var m = state.messages[mi];
      var blocks = Array.isArray(m.content) ? m.content : [];
      if (m.role === "user") {
        var textParts = blocks.filter(function (c) { return c.type === "text"; }).map(function (c) { return c.text; });
        if (textParts.length) {
          addChatItem({ type: "user", text: textParts.join(""), time: "" });
        }
        // tool_result
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
      var textBuf = "";
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
          addChatItem({ type: "tool", toolId: b.id, name: b.name, args: b.input, output: null, success: null, state: "pending" });
        }
      }
      if (textBuf) {
        addChatItem({ type: "assistant", html: renderMarkdown(textBuf), time: "", streaming: false });
      }
    }
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
  async function sendMessage(text) {
    text = (text || "").trim();
    var readyAttachments = state.attachments.filter(function (a) { return a.status === "ready" && a.result; });
    if (!text && readyAttachments.length === 0) return;
    if (state.busy) return;
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

    // Add user message
    addChatItem({ type: "user", text: displayText, time: timeStr() });
    state.messages.push({ role: "user", content: [{ type: "text", text: displayText }] });

    // Start streaming
    state.busy = true;
    currentStreamText = "";
    currentStreamId = ++itemIdSeq;
    state.chatItems.push({
      id: currentStreamId,
      type: "assistant",
      html: "",
      time: timeStr(),
      streaming: true,
    });
    notify();

    try {
      await invoke("chat", { message: text, attachments: attachmentsPayload });
    } catch (err) {
      addSystemItem("⚠️ " + (err && err.toString ? err.toString() : err));
      state.busy = false;
      // Remove empty streaming bubble
      state.chatItems = state.chatItems.filter(function (item) { return item.id !== currentStreamId || item.html; });
      notify();
    }
  }

  async function cancelGeneration() {
    if (!state.busy) return;
    try {
      await invoke("cancel_generation");
    } catch (e) {
      console.warn("cancel failed", e);
    }
  }

  // ── Persist messages ─────────────────────────────────────────────
  async function persistMessages() {
    if (!state.activeSessionId) return;
    try {
      await invoke("save_session_messages", { id: state.activeSessionId, messages: state.messages });
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
  listen("chat:delta", function (e) {
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
  });

  listen("chat:tool_start", function (e) {
    var p = e.payload || {};
    toolMeta[p.id] = { name: p.name, args: p.args };
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

    // Add tool card
    addChatItem({
      type: "tool", toolId: p.id, name: p.name, args: p.args,
      output: null, success: null, state: "running",
    });
    notify();
  });

  listen("chat:tool_end", function (e) {
    var p = e.payload || {};
    var meta = toolMeta[p.id];
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

    updateToolItem(p.id, p.output, p.success);

    // Careful hook：DeepSeek-TUI shell.rs 拦截 Dangerous → 红色拦截卡
    var md = p.metadata;
    if (md && md.safety_level === "dangerous" && md.blocked) {
      addChatItem({ type: "careful_blocked", args: meta && meta.args, metadata: md, time: timeStr() });
    }

    // write_file / append_file 成功 → 加入产物列表
    if (p.success && meta && (meta.name === "write_file" || meta.name === "append_file")) {
      var ap = extractArtifactPath(meta.args);
      if (ap) trackArtifact(ap);
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
  });

  listen("chat:done", async function (e) {
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
    currentStreamText = "";
    currentStreamId = 0;

    // 执行 plan 完成 → 回 yolo 默认态(plan_phase 从 executing → none)，同步后端 store
    if (state.modeState.plan_phase === "executing") {
      state.modeState = { mode: "yolo", plan_phase: "none", pinvou_review_enabled: state.modeState.pinvou_review_enabled };
      if (state.activeSessionId) {
        try { await invoke("discard_plan", { sessionId: state.activeSessionId }); } catch (_) {}
      }
    }

    await persistMessages();
    await refreshHistoryList();
    notify();
  });

  listen("chat:usage", function (e) {
    var input = Number(e.payload && e.payload.input_tokens || 0);
    if (input > 0) {
      state.tokens = { input: input, max: maxModelLen };
      notify();
    }
  });

  listen("chat:compaction", function (e) {
    var phase = e.payload && e.payload.phase;
    var msg = e.payload && e.payload.message || "";
    var auto = e.payload && e.payload.auto ? "（自动）" : "";
    if (phase === "start") addSystemItem("⏳ 正在压缩上下文" + auto + " " + msg);
    else if (phase === "done") addSystemItem("✓ 上下文压缩完成" + auto + " " + msg);
    else if (phase === "fail") addSystemItem("⚠️ 压缩失败" + auto + ": " + msg);
  });

  // ── request_user_input：渲染选择卡片（不进 messages.json）─────────
  // payload: { id: tool_call_id, questions: [{header, id, question, options:[{label, description}]}] }
  listen("chat:user_input_required", function (e) {
    var p = e.payload || {};
    var questions = p.questions || [];
    if (!Array.isArray(questions) || questions.length === 0) return;
    addChatItem({
      type: "user_input", toolCallId: p.id, questions: questions,
      resolved: false, cardState: "active", time: timeStr(),
    });
    notify();
  });

  // 可恢复的瞬态错误（SSE idle timeout / 瞬态工具失败）：turn 没结束，引擎会 retry，
  // 绝不 setBusy(false)，只飘一条 ⚠️ 提示。
  listen("chat:transient_error", function (e) {
    var error = e.payload && e.payload.error;
    if (error) addSystemItem("⚠️ " + error);
  });

  // File watcher 推送的产物事件：当前 session workspace 下新文件/修改/删除
  listen("artifact:disk", function (e) {
    var p = e.payload || {};
    if (p.session_id !== state.activeSessionId) return;
    if (!p.path) return;
    if (p.event === "removed") untrackArtifact(p.path);
    else trackArtifact(p.path);
  });

  // chat:plan_snapshot —— update_plan/checklist_write 后实时更新进度，与 plan_ready 解耦
  listen("chat:plan_snapshot", function (e) {
    var p = e.payload || {};
    if (p.session_id && p.session_id !== state.activeSessionId) return;
    if (p.plan_snapshot) state.planSnapshot.plan = p.plan_snapshot;
    if (p.todos_snapshot) state.planSnapshot.todos = p.todos_snapshot;
    notify();
  });

  // chat:plan_ready —— 任一层快照非空就渲染方案卡（plan_phase → ready）
  listen("chat:plan_ready", function (e) {
    var p = e.payload || {};
    if (p.session_id && p.session_id !== state.activeSessionId) return;
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
  });

  // chat:plan_text_fallback —— Planning 态 AI 没调 plan 工具但 text 写了方案
  listen("chat:plan_text_fallback", function (e) {
    var p = e.payload || {};
    if (p.session_id && p.session_id !== state.activeSessionId) return;
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
  });

  // chat:execution_stuck —— Executing 自驱 N 次后仍卡
  listen("chat:execution_stuck", function (e) {
    var p = e.payload || {};
    if (p.session_id && p.session_id !== state.activeSessionId) return;
    if (hasUnresolvedItem("execution_stuck")) return;
    addChatItem({ type: "execution_stuck", tries: p.auto_continue_tried || 0, resolved: false, time: timeStr() });
    notify();
  });

  // chat:phase_changed —— 底座从 LLM 回复抽 <phase id="..."/> marker 触发
  listen("chat:phase_changed", function (e) {
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
  function applyModeFromState(st) {
    state.modeState = {
      mode: st.mode || "yolo",
      plan_phase: st.plan_phase || "none",
      pinvou_review_enabled: st.pinvou_review_enabled != null ? !!st.pinvou_review_enabled : state.modeState.pinvou_review_enabled,
    };
  }

  // ── Plan/YOLO 命令 ───────────────────────────────────────────────
  async function acceptPlan(itemId, planMarkdown, echo) {
    if (!state.activeSessionId) return;
    if (itemId) patchItemById(itemId, { cardState: "approved", statusLabel: "✅ 已批准", resolved: true });
    pushUserEcho(echo || "✅ 就这么干", true);
    state.busy = true; notify();
    try {
      var st = await invoke("accept_plan", { sessionId: state.activeSessionId, planMarkdown: planMarkdown || "" });
      applyModeFromState(st);
    } catch (e) {
      if (itemId) patchItemById(itemId, { cardState: "active", statusLabel: "", resolved: false });
      state.busy = false;
      addSystemItem("⚠️ accept_plan 失败: " + e);
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
      await invoke("submit_user_input", { toolCallId: toolCallId, answers: answers });
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
    try { await invoke("cancel_user_input", { toolCallId: toolCallId }); } catch (_) {}
    patchItemById(itemId, { resolved: true, cardState: "cancelled" });
    notify();
  }

  // ── 编辑上一轮 / 手动压缩 ─────────────────────────────────────────
  async function editLastTurn(newText) {
    try { await invoke("edit_last_turn", { newMessage: newText }); } catch (e) { addSystemItem("⚠️ 编辑失败: " + e); }
  }
  async function compactNow() {
    try { await invoke("compact_now"); } catch (e) { addSystemItem("⚠️ 压缩失败: " + e); }
  }

  // ── 产物面板 ─────────────────────────────────────────────────────
  function artifactInfo(path) { return invoke("artifact_info", { path: path }); }
  function readArtifactText(path) { return invoke("read_artifact_text", { path: path }); }
  function openContainingFolder(path) { return invoke("open_containing_folder", { path: path }).catch(function (e) { addSystemItem("⚠️ 打开目录失败: " + e); }); }
  function openInSystem(path) { return invoke("open_in_system", { path: path }).catch(function (e) { addSystemItem("⚠️ 打开失败: " + e); }); }

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
    cancelGeneration: cancelGeneration,
    createNewSession: createNewSession,
    switchToSession: switchToSession,
    deleteSession: deleteSession,
    renameSession: renameSession,
    startMonitorPolling: startMonitorPolling,
    stopMonitorPolling: stopMonitorPolling,
    saveSettings: saveSettings,
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
    // 附件
    addAttachmentByPath: addAttachmentByPath,
    addPasteImage: addPasteImage,
    removeAttachment: removeAttachment,
    clearAttachments: clearAttachments,
    // 品悟审批
    readSkillBody: readSkillBody,
    setPinvouReview: setPinvouReview,
    // 工作流
    loadSkills: loadSkills,
    activateSkill: activateSkill,
    deactivateSkill: deactivateSkill,
    openDemo: openDemo,
    closeDemo: closeDemo,
    setCurrentPhase: setCurrentPhase,
  };

  // Auto-init after DOM ready
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    setTimeout(init, 0);
  }
})();
