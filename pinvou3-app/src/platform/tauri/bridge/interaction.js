(function () {
  "use strict";

  var registry = window.__PINVOU_TAURI_BRIDGE_FEATURES__ = window.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry.interaction = function (context) {
    var state = context.state;
    var invoke = context.invoke;
    var notify = context.notify;
    var bt = context.bt;
    var addSystemItem = context.addSystemItem;
    var addChatItem = context.addChatItem;
    var timeStr = context.timeStr;
    var runSyncOnSession = context.runSyncOnSession;
    var flushAssistantMessageToHistory = context.flushAssistantMessageToHistory;
    var resetPendingAssistant = context.resetPendingAssistant;
    var rerenderFromMessages = context.rerenderFromMessages;
    var turnUsageDirty = context.turnUsageDirty;
    var ensureSession = context.ensureSession;
    var sendMessage = context.sendMessage;

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
    context.currentStreamText = "";
    context.currentStreamId = ++context.itemIdSeq;
    state.chatItems.push({ id: context.currentStreamId, type: "assistant", html: "", time: timeStr(), streaming: true });
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


    return {
      refreshSuperPerm: refreshSuperPerm,
      toggleSuperPerm: toggleSuperPerm,
      syncModeState: syncModeState,
      patchItemById: patchItemById,
      pushUserEcho: pushUserEcho,
      markResolved: markResolved,
      runOnSession: runOnSession,
      addSystemItemFor: addSystemItemFor,
      patchItemByIdFor: patchItemByIdFor,
      startThinking: startThinking,
      thinkingTool: thinkingTool,
      thinkingIdle: thinkingIdle,
      stopThinking: stopThinking,
      applyModeFromState: applyModeFromState,
      acceptPlan: acceptPlan,
      discardPlan: discardPlan,
      exitPlanToYolo: exitPlanToYolo,
      setPlanModeNext: setPlanModeNext,
      planStuckReplan: planStuckReplan,
      planStuckGo: planStuckGo,
      submitUserInput: submitUserInput,
      cancelUserInput: cancelUserInput,
      editLastTurn: editLastTurn,
      compactNow: compactNow,
    };
  };
})();
