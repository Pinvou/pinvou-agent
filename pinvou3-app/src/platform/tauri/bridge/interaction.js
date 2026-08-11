(function () {
  "use strict";

  var registry = window.__PINVOU_TAURI_BRIDGE_FEATURES__ = window.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry.interaction = function (context) {
    var state = context.state;
    var invoke = context.invoke;
    var notify = context.notify;
    var bt = context.bt;
    var addSystemItem = context.addSystemItem;
    var addAuthoritySyncNotice = context.addAuthoritySyncNotice;
    var addChatItem = context.addChatItem;
    var timeStr = context.timeStr;
    var runSyncOnSession = context.runSyncOnSession;
    var flushAssistantMessageToHistory = context.flushAssistantMessageToHistory;
    var resetPendingAssistant = context.resetPendingAssistant;
    var rerenderFromMessages = context.rerenderFromMessages;
    var turnUsageDirty = context.turnUsageDirty;
    var ensureSession = context.ensureSession;
    var sendMessage = context.sendMessage;
    var getBuffer = context.getBuffer;
    var reconcileRemoteTurn = context.reconcileRemoteTurn;
    var isBusyFor = context.isBusyFor;
    var markRemoteTurn = context.markRemoteTurn;

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
      state.modeState = { mode: "yolo", multiAgent: false };
      return;
    }
    try {
      var ms = await invoke("get_mode_state", { sessionId: state.activeSessionId });
      state.modeState = { mode: ms.mode || "yolo", multiAgent: !!ms.multi_agent };
    } catch (e) {
      state.modeState = { mode: "yolo", multiAgent: false };
    }
  }

  // ── 卡片动作辅助 ─────────────────────────────────────────────────
  function patchItemById(id, patch) {
    for (var i = 0; i < state.chatItems.length; i++) {
      if (state.chatItems[i].id === id) { Object.assign(state.chatItems[i], patch); break; }
    }
  }
  function pushUserEcho(text, persist) {
    var item = { type: "user", text: text, time: timeStr() };
    addChatItem(item);
    var message = null;
    if (persist) {
      message = { role: "user", content: [{ type: "text", text: text }] };
      state.messages.push(message);
    }
    return { item: item, message: message };
  }
  function markResolved(id, statusLabel) { patchItemById(id, { resolved: true, statusLabel: statusLabel || "" }); notify(); }

  // ── Per-session UI 路由 ─────────────────────────────────────────
  // 卡片动作链路有多个 await 边界,用户可能中途切 session。所有 UI 写入(chatItem 增改、
  // pending* 标记、modeState 同步)必须落在【触发 session】的 buffer 上,不能跟着
  // state.activeSessionId 漂走。一律 wrap 进 runSyncOnSession 是因为:sid === active
  // 时它是 no-op 直通,sid !== active 时它 swap-load-fn-save 回 sid 的 buffer。
  function runOnSession(sid, fn) { runSyncOnSession(sid || state.activeSessionId, fn); }
  function addSystemItemFor(sid, text) { runOnSession(sid, function () { addSystemItem(text); }); }
  function addAuthoritySyncNoticeFor(sid, text) {
    runOnSession(sid, function () { addAuthoritySyncNotice(text); });
  }
  function patchItemByIdFor(sid, id, patch) { runOnSession(sid, function () { patchItemById(id, patch); }); }


  // ── 思考指示器状态（每次阶段切换重置计时）──────────────────────
  function startThinking() { state.thinking = { active: true, phase: "thinking", toolName: "", startedAt: Date.now() }; }
  function thinkingTool(name) { state.thinking = { active: true, phase: "tool", toolName: name || "", startedAt: Date.now() }; }
  function thinkingIdle() { state.thinking = { active: true, phase: "thinking", toolName: "", startedAt: Date.now() }; }
  function stopThinking() { state.thinking = { active: false, phase: "thinking", toolName: "", startedAt: 0 }; }
  function applyModeFromState(st) {
    state.modeState = { mode: st.mode || "yolo", multiAgent: !!st.multi_agent };
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
    var sid = state.activeSessionId;
    if (!sid) return;
    var planTicket = String(planId || "").trim();
    if (!planTicket) {
      if (itemId) patchItemByIdFor(sid, itemId, { cardState: "frozen", statusLabel: bt("planHistorical"), resolved: true });
      addSystemItemFor(sid, bt("planTicketInvalid"));
      notify();
      return;
    }
    var planBuffer = getBuffer(sid);
    if (planBuffer && planBuffer.remoteTurnActive && !(await reconcileRemoteTurn(sid))) {
      addAuthoritySyncNoticeFor(sid, bt("remoteTurnSyncing"));
      notify();
      return;
    }
    if (state.activeSessionId !== sid || isBusyFor(sid) || !isActionablePlanCard(sid, itemId, planTicket)) return;
    if (planBuffer) {
      planBuffer.localTurnOwned = true;
      planBuffer.remoteTurnActive = false;
      planBuffer.remoteTerminalSeen = false;
      planBuffer.remoteCommittedRevision = "";
    }
    if (itemId) patchItemByIdFor(sid, itemId, { cardState: "approved", statusLabel: bt("approved"), resolved: true });
    var echoEntry = null;
    var displayEcho = echo || bt("echoGo");
    runOnSession(sid, function () { echoEntry = pushUserEcho(displayEcho, true); state.busy = true; startThinking(); });
    notify();
    try {
      var st = await invoke("accept_plan", {
        sessionId: sid,
        planId: planTicket,
        planMarkdown: planMarkdown || "",
        displayMessage: displayEcho,
      });
      if (planBuffer) planBuffer.deferredRemoteUserEvent = null;
      runOnSession(sid, function () { applyModeFromState(st); });
    } catch (e) {
      var errorText = String(e && e.message ? e.message : e || "");
      var concurrentTurn = errorText.indexOf("session_turn_in_progress") >= 0;
      var planNotActive = errorText.indexOf("plan_not_active") >= 0;
      if (planBuffer) planBuffer.localTurnOwned = false;
      if (itemId) patchItemByIdFor(sid, itemId, planNotActive
        ? { cardState: "frozen", statusLabel: bt("planHistorical"), resolved: true }
        : { cardState: "active", statusLabel: "", resolved: false });
      runOnSession(sid, function () {
        if (echoEntry) {
          state.chatItems = state.chatItems.filter(function (item) { return item !== echoEntry.item; });
          state.messages = state.messages.filter(function (message) { return message !== echoEntry.message; });
        }
        state.busy = false;
        stopThinking();
      });
      if (concurrentTurn && planBuffer) markRemoteTurn(sid, planBuffer);
      try {
        var currentMode = await invoke("get_mode_state", { sessionId: sid });
        runOnSession(sid, function () { applyModeFromState(currentMode); });
      } catch (_) {}
      addSystemItemFor(sid, bt("acceptPlanFailed") + e);
    }
    notify();
  }
  async function discardPlan(itemId, planId) {
    var sid = state.activeSessionId;
    var planTicket = String(planId || "").trim();
    if (!sid || !isActionablePlanCard(sid, itemId, planTicket)) return;
    patchItemByIdFor(sid, itemId, {
      cardState: "frozen", statusLabel: bt("planDiscarded"), resolved: true,
      planResolutionConfirmed: false,
    });
    notify();
    try {
      var st = await invoke("discard_plan", { sessionId: sid, planId: planTicket });
      runOnSession(sid, function () { applyModeFromState(st); });
      patchItemByIdFor(sid, itemId, { planResolutionConfirmed: true });
    } catch (e) {
      var errorText = String(e && e.message ? e.message : e || "");
      var planNotActive = errorText.indexOf("plan_not_active") >= 0;
      runOnSession(sid, function () {
        var card = state.chatItems.find(function (item) {
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
          var currentMode = await invoke("get_mode_state", { sessionId: sid });
          runOnSession(sid, function () { applyModeFromState(currentMode); });
        } catch (_) {}
      }
      addSystemItemFor(sid, bt("discardPlanFailed") + e);
    }
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
  // 多智能体开关（ADR-0006）：模型列表下方的会话级开关。后端做名册装配
  // + 名册装配与即时推送；前端只认返回的权威状态。
  // in-flight 期间丢弃**同会话**的后续调用（防重入兜底）：第二次点击会带
  // 着旧的 multiAgentOn 重复提交，其中一次失败的回滚还会覆盖另一次的新
  // 状态。按会话记账而非全局布尔：A 开启在途时不得殃及 B 的开关（复核 P3）。
  var multiAgentToggleInFlight = new Set();
  async function setMultiAgentMode(enabled) {
    var flightKey = state.activeSessionId || "__draft__";
    if (multiAgentToggleInFlight.has(flightKey)) return;
    multiAgentToggleInFlight.add(flightKey);
    try {
      var sid = state.activeSessionId;
      if (!sid) {
        // 草稿态**不物化会话**：否则开个开关就在左侧列表凭空造出一条空
        // 对话（真机反馈）。意图寄存在草稿上，首条消息经 ensureSession
        // 创建会话时才落后端；这里只翻开关行的显示，权威状态以物化时的
        // 后端返回为准。
        state.pendingDraftMultiAgent = !!enabled;
        state.modeState = {
          mode: (state.modeState && state.modeState.mode) || "yolo",
          multiAgent: !!enabled,
        };
        // 草稿分支会从 try 内提前返回，走不到函数末尾的 notify()。
        // 必须在这里主动发布快照，否则拨杆只能等下一次无关状态事件才刷新。
        notify();
        return;
      }
      // 乐观翻转：开启在后端要做名册装配与引擎同步（可能耗时数百毫秒），
      // 等返回再翻拨杆会像"点了没反应"。先翻显示并 notify，成功后用后端
      // 权威状态复核；失败回滚显示并提示。in-flight 闸已挡并发重入。
      var previousMultiAgent = !!(state.modeState && state.modeState.multiAgent);
      state.modeState = {
        mode: (state.modeState && state.modeState.mode) || "yolo",
        multiAgent: !!enabled,
      };
      notify();
      try {
        var st = await invoke("set_multi_agent_mode", { sessionId: sid, enabled: !!enabled });
        runOnSession(sid, function () { applyModeFromState(st); });
      } catch (invokeError) {
        // 回滚与报错必须定向回触发会话：await 期间用户可能已切走，直接改
        // 全局 modeState 会把回滚砸进别的会话、报错落错聊天（复核 P1）。
        runOnSession(sid, function () {
          state.modeState = {
            mode: (state.modeState && state.modeState.mode) || "yolo",
            multiAgent: previousMultiAgent,
          };
          addSystemItem(bt("switchModeFailed") + invokeError);
        });
      }
    } catch (e) {
      addSystemItem(bt("switchModeFailed") + e);
    } finally {
      multiAgentToggleInFlight.delete(flightKey);
    }
    notify();
  }
  // plan-stuck / fallback / execution-stuck 卡片动作
  async function planStuckReplan(itemId) {
    patchItemById(itemId, { resolved: true, statusLabel: bt("replanRequested") }); notify();
    await sendMessage("请用 todo_write 工具输出完整方案步骤,不要直接调写工具。");
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
        var text = (a.other || a.label === "其他") ? bt("echoOtherPrefix") + a.value : a.label;
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
    var sid = state.activeSessionId;
    var editBuffer = getBuffer(sid);
    // 编辑前先收敛远端对账(与 web bridge 的 editLastTurn 对齐):失败对账
    // 状态下编辑会被陈旧 committed 事件重武装旧 revision,污染新一轮。
    if (editBuffer && editBuffer.remoteTurnActive && !(await reconcileRemoteTurn(sid))) {
      addAuthoritySyncNotice(bt("remoteTurnSyncing"));
      notify();
      return;
    }
    // await 期间可能切会话或开始新回合,二次确认(与 web bridge 对齐)。
    if (state.activeSessionId !== sid || state.busy) return;
    // 编辑=新一轮:接管本地回合并清零 remote 对账状态,避免失败对账
    // 状态下跨回合串用(与 web bridge 的 editLastTurn 对齐)。
    if (editBuffer) {
      editBuffer.localTurnOwned = true;
      editBuffer.remoteTurnActive = false;
      editBuffer.remoteTerminalSeen = false;
      editBuffer.remoteCommittedRevision = "";
    }
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
    state.chatItems.push({ id: context.currentStreamId, type: "assistant", text: "", html: "", time: timeStr(), streaming: true });
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
      setMultiAgentMode: setMultiAgentMode,
      planStuckReplan: planStuckReplan,
      planStuckGo: planStuckGo,
      submitUserInput: submitUserInput,
      cancelUserInput: cancelUserInput,
      editLastTurn: editLastTurn,
      compactNow: compactNow,
    };
  };
})();
