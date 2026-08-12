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
    var sendMessage = context.sendMessage;
    var getBuffer = context.getBuffer;
    var reconcileRemoteTurn = context.reconcileRemoteTurn;
    var isBusyFor = context.isBusyFor;
    var markRemoteTurn = context.markRemoteTurn;
    // 共享的 modeState epoch 表与权威写回收敛点（bridge.js 注入，评审 P1）：
    // interaction 与 chat-events 必须用同一份 epoch 表，否则事件直写与
    // 本模块的读取校验互不感知，竞态照样敞口。
    var modeStateEpochs = context.modeStateEpochs;
    var bumpModeStateEpoch = context.bumpModeStateEpoch;
    var applyAuthoritativeModeState = context.applyAuthoritativeModeState;
    var currentDraftModeState = context.currentDraftModeState;

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
  // 会话级读取。必须防住的竞态：
  // 1. await 挂起期间用户切走：响应返回后不能写当前 active 会话的显示（否则
  //    上一个会话的开关/模式状态会串台到新会话，表现为"在一个对话打开开关，
  //    所有对话都开"）。校验发起时的 sid 仍是 active，否则把结果定向写回
  //    sid 自己的 buffer，不碰当前显示。
  // 2. 多智能体开关翻转/模式切换的本地权威改写：乐观翻转已把新状态显示出来
  //    （后端尚未落盘），或权威写回已完成，此时在途的 get_mode_state 只会
  //    拿到旧值，把用户刚打开（或关闭）的开关覆盖回去。防法：per-session
  //    epoch——每次本地权威改写 bump 对应会话的 revision，读取发起时捕获、
  //    返回后校验，epoch 变了即丢弃陈旧读取，权威值由对应操作写回。
  //    只看瞬时 in-flight 集合识别不了"toggle 已完成（集合已清空）、旧读取
  //    才返回"的顺序（审计意见），epoch 一并覆盖在途与已完成两种顺序。
  //    epoch 表与 bump/权威写回由 bridge.js 共享（评审 P1）：任何权威
  //    modeState 写回一律走 applyAuthoritativeModeState，禁止散点手工写。
  async function syncModeState() {
    var sid = state.activeSessionId;
    if (!sid) {
      // 草稿态：显示当前 lane 的全局默认（三分 lane 语义），不再恒 yolo。
      state.modeState = currentDraftModeState();
      return;
    }
    if (multiAgentToggleInFlight.has(sid)) return;
    var epoch = modeStateEpochs[sid] || 0;
    try {
      // invoke 形状保持 { sessionId: state.activeSessionId }（协议指纹按文本
      // 计算）；发起瞬间 activeSessionId === sid，await 返回后按 sid 校验归属。
      var ms = await invoke("get_mode_state", { sessionId: state.activeSessionId });
      // 响应返回后校验 epoch：期间任何本地权威改写（开关翻转、plan/模式切换）
      // 都会使该读取成为陈旧值，丢弃——权威值由对应操作写回。此检查覆盖
      // 两种返回顺序：改写仍在途（原 in-flight 闸场景）与改写已完成（旧读取
      // 最后返回的顺序，in-flight 集合已清空、仅靠集合识别不了）。
      if ((modeStateEpochs[sid] || 0) !== epoch) return;
      if (state.activeSessionId !== sid) {
        runSyncOnSession(sid, function () {
          state.modeState = { mode: ms.mode || "yolo", multiAgent: !!ms.multi_agent };
        });
        return;
      }
      state.modeState = { mode: ms.mode || "yolo", multiAgent: !!ms.multi_agent };
    } catch (e) {
      // get 失败同样不得用默认值覆盖已发生的权威改写。
      if ((modeStateEpochs[sid] || 0) !== epoch) return;
      if (state.activeSessionId !== sid) return;
      state.modeState = { mode: "yolo", multiAgent: false };
    }
  }

  // ── lane 全局默认（工作/设计/代码三分，复审拍板）─────────────────
  // 草稿态 mode = 本 lane 全局默认；草稿切换只写全局默认（不物化会话），
  // 已生成会话的切换只写会话自己的记录（set_plan_mode_next 等 per-session
  // 命令，后端不再渗全局）。
  async function refreshModeDefaults() {
    try {
      var defaults = await invoke("get_mode_defaults");
      if (defaults) state.modeDefaults = defaults;
    } catch (e) { /* 读取失败保留旧值/缺省，不打扰交互 */ }
    if (!state.activeSessionId) {
      state.modeState = currentDraftModeState();
      notify();
    }
  }
  // ChatView 随 pinvouMode 传入当前 lane；草稿态立即按新 lane 默认刷新显示。
  function setModeLane(lane) {
    var next = lane === "design" ? "design" : "work";
    if (state.modeLane === next) return;
    state.modeLane = next;
    if (!state.activeSessionId) {
      state.modeState = currentDraftModeState();
      notify();
    }
  }
  // 草稿态 chip 切换：写本 lane 全局默认（setDraftMode 不物化会话——
  // 物化时由 ensureSession 把 lane 默认应用到新会话）。
  async function setDraftMode(target) {
    var lane = state.modeLane === "design" ? "design" : "work";
    try {
      var defaults = await invoke("set_mode_default", { lane: lane, mode: target });
      if (defaults) state.modeDefaults = defaults;
      if (!state.activeSessionId) {
        state.modeState = {
          mode: target,
          multiAgent: !!(state.modeState && state.modeState.multiAgent),
        };
      }
    } catch (e) { addSystemItem(bt("switchModeFailed") + e); }
    notify();
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
      applyAuthoritativeModeState(sid, st);
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
        applyAuthoritativeModeState(sid, currentMode);
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
      applyAuthoritativeModeState(sid, st);
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
          applyAuthoritativeModeState(sid, currentMode);
        } catch (_) {}
      }
      addSystemItemFor(sid, bt("discardPlanFailed") + e);
    }
    notify();
  }
  async function exitPlanToYolo() {
    var sid = state.activeSessionId;
    // 草稿态：不物化会话，改写本 lane 全局默认（三分 lane 语义）。
    if (!sid) { await setDraftMode("yolo"); return; }
    try {
      // invoke 形状保持 { sessionId: state.activeSessionId }（协议指纹按文本
      // 计算）；发起瞬间 activeSessionId === sid，await 返回后按 sid 定向写回。
      var st = await invoke("exit_plan_to_yolo", { sessionId: state.activeSessionId });
      applyAuthoritativeModeState(sid, st);
    } catch (e) { addSystemItemFor(sid, bt("exitPlanFailed") + e); }
    notify();
  }
  // 灯泡 toggle：plan ↔ yolo
  async function setPlanModeNext() {
    // 草稿态：不物化会话，改写本 lane 全局默认（三分 lane 语义；旧实现会先
    // ensureSession 物化——草稿页点 Plan 凭空造出空会话）。
    var sid = state.activeSessionId;
    if (!sid) { await setDraftMode("plan"); return; }
    try {
      var st = await invoke("set_plan_mode_next", { sessionId: sid });
      applyAuthoritativeModeState(sid, st);
    } catch (e) { addSystemItemFor(sid, bt("switchModeFailed") + e); }
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
      // 翻转即刻 bump 该会话 epoch：让在途的 get_mode_state 读取（syncModeState）
      // 全部作废，无论其响应在 toggle 进行中还是完成后返回——陈旧读取一律
      // 丢弃，权威值由下方 set_multi_agent_mode 返回后写回（审计意见）。
      bumpModeStateEpoch(sid);
      state.modeState = {
        mode: (state.modeState && state.modeState.mode) || "yolo",
        multiAgent: !!enabled,
      };
      notify();
      try {
        var st = await invoke("set_multi_agent_mode", { sessionId: sid, enabled: !!enabled });
        applyAuthoritativeModeState(sid, st);
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
  // 卡片动作链路有 await 边界：entry 先捕获触发会话 sid，invoke 与后续全部 UI 写入
  // 都定向到 sid（runOnSession / patchItemByIdFor），避免用户提交期间切会话导致
  // echo/restoredAnswers 漏写触发会话或污染当前会话（与 acceptPlan 同一约定）。
  async function submitUserInput(itemId, toolCallId, answers, questions) {
    var sid = state.activeSessionId;
    if (!sid) return;
    patchItemByIdFor(sid, itemId, { submitting: true }); notify();
    try {
      await invoke("submit_user_input", { toolCallId: toolCallId, answers: answers, sessionId: sid });
      // 摘要按 question 分组拼接：answers 是按选项展开的（multi_select 时同一题多条），
      // 不能按 answers 索引一一对应 questions（会越界抛 TypeError，复核 P1）。
      // 用无原型对象：question id 仅后端校验非空，constructor/toString/__proto__ 是合法输入，
      // 普通 {} 会让这些键命中 Object.prototype 继承属性，.push 抛 TypeError（复核 P1）。
      var byId = Object.create(null);
      answers.forEach(function (a) { if (a && a.id != null) (byId[a.id] = byId[a.id] || []).push(a); });
      var summary = questions.map(function (q, qi) {
        var list = byId[q.id];
        if (!list || !list.length) return null;
        var header = q.header || ("Q" + (qi + 1));
        return header + ": " + list.map(function (a) {
          var text = (a.other || a.label === "其他") ? bt("echoOtherPrefix") + a.value : a.label;
          return text;
        }).join(" · ");
      }).filter(Boolean).join(" · ");
      runOnSession(sid, function () {
        pushUserEcho("✓ " + summary, false);
        flushAssistantMessageToHistory();
      });
      // 提交时即存答案：切走视图再切回（ChatView 重挂载但 bridge state 保留）时，
      // QuestionChoiceCard 用 restoredAnswers 恢复选中态；会话级 rerender 另有解析。
      patchItemByIdFor(sid, itemId, { resolved: true, cardState: "submitted", submitting: false, restoredAnswers: answers });
    } catch (e) {
      patchItemByIdFor(sid, itemId, { submitting: false, error: String(e) });
    }
    notify();
  }
  async function cancelUserInput(itemId, toolCallId) {
    var sid = state.activeSessionId;
    try { await invoke("cancel_user_input", { toolCallId: toolCallId, sessionId: sid }); } catch (_) {}
    patchItemByIdFor(sid, itemId, { resolved: true, cardState: "cancelled" });
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
      acceptPlan: acceptPlan,
      discardPlan: discardPlan,
      exitPlanToYolo: exitPlanToYolo,
      setPlanModeNext: setPlanModeNext,
      setDraftMode: setDraftMode,
      setModeLane: setModeLane,
      refreshModeDefaults: refreshModeDefaults,
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
