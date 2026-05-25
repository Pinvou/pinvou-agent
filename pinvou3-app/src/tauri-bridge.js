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

  // ── Send message ─────────────────────────────────────────────────
  async function sendMessage(text) {
    if (!text || !text.trim()) return;
    if (state.busy) return;
    text = text.trim();

    if (!state.activeSessionId) {
      await createNewSession();
      if (!state.activeSessionId) return;
    }

    // Add user message
    addChatItem({ type: "user", text: text, time: timeStr() });
    state.messages.push({ role: "user", content: [{ type: "text", text: text }] });

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
      await invoke("chat", { message: text, attachments: [] });
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

    // Add tool card
    addChatItem({
      type: "tool", toolId: p.id, name: p.name, args: p.args,
      output: null, success: null, state: "running",
    });
    notify();
  });

  listen("chat:tool_end", function (e) {
    var p = e.payload || {};
    var resultContent = typeof p.output === "string" ? p.output : JSON.stringify(p.output);
    flushAssistantMessageToHistory();
    state.messages.push({
      role: "user",
      content: [{ type: "tool_result", tool_use_id: p.id, content: resultContent, is_error: !p.success }],
    });
    updateToolItem(p.id, p.output, p.success);
    delete toolMeta[p.id];

    // Reset stream for next assistant text
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

    await persistMessages();
    await refreshHistoryList();
    notify();
  });

  listen("chat:usage", function (e) {
    var input = Number(e.payload && e.payload.input_tokens || 0);
    if (input > 0) {
      // Could expose this for a token bar in React
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
  };

  // Auto-init after DOM ready
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    setTimeout(init, 0);
  }
})();
