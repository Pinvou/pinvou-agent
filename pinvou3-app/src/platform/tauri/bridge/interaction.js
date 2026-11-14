(function () {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";

  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting statements would diverge from the artifact
  const registry = window.__PINVOU_TAURI_BRIDGE_FEATURES__ = window.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry.interaction = function (context) {let pinvouSharedtauriInteractionCache = null;
function pinvouSharedtauriInteraction() {
  if (!pinvouSharedtauriInteractionCache) pinvouSharedtauriInteractionCache = window.PinvouBridgeShared.create("tauriInteraction", { state, invoke, notify, currentDraftModeState, runSyncOnSession, addSystemItem, setDraftMode, applyAuthoritativeModeState, bt, sendMessage, pushUserEcho, flushAssistantMessageToHistory });
  return pinvouSharedtauriInteractionCache;
}


    const state = context.state;
    const recordAuthoritySyncDiagnostic = context.recordAuthoritySyncDiagnostic || function () {};
    const authoritySyncBufferSnapshot = context.authoritySyncBufferSnapshot || function () { return {}; };
    const invoke = context.invoke;
    const notify = context.notify;
    const bt = context.bt;
    const addSystemItem = context.addSystemItem;
    const addAuthoritySyncNotice = context.addAuthoritySyncNotice;
    const addChatItem = context.addChatItem;
    const timeStr = context.timeStr;
    const runSyncOnSession = context.runSyncOnSession;
    const flushAssistantMessageToHistory = context.flushAssistantMessageToHistory;
    const resetPendingAssistant = context.resetPendingAssistant;
    const rerenderFromMessages = context.rerenderFromMessages;
    const turnUsageDirty = context.turnUsageDirty;
    const sendMessage = context.sendMessage;
    const getBuffer = context.getBuffer;
    const reconcileRemoteTurn = context.reconcileRemoteTurn;
    const isBusyFor = context.isBusyFor;
    const markRemoteTurn = context.markRemoteTurn;
    const userMessageDisplayText = context.userMessageDisplayText;
    const sendMessageToSession = context.sendMessageToSession;
    // 共享的 modeState epoch 表与权威写回收敛点（bridge.js 注入，评审 P1）：
    // interaction 与 chat-events 必须用同一份 epoch 表，否则事件直写与
    // 本模块的读取校验互不感知，竞态照样敞口。
    const modeStateEpochs = context.modeStateEpochs;
    const bumpModeStateEpoch = context.bumpModeStateEpoch;
    const applyAuthoritativeModeState = context.applyAuthoritativeModeState;
    const currentDraftModeState = context.currentDraftModeState;

  // ── Super permission ─────────────────────────────────────────────
async function refreshSuperPerm() { return pinvouSharedtauriInteraction().refreshSuperPerm(); }
async function toggleSuperPerm() { return pinvouSharedtauriInteraction().toggleSuperPerm(); }

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
    const sid = state.activeSessionId;
    if (!sid) {
      // Draft state: show the current lane's global default (two-lane
      // semantics), no longer a constant yolo.
      state.modeState = currentDraftModeState();
      return;
    }
    if (multiAgentToggleInFlight.has(sid)) return;
    const epoch = modeStateEpochs[sid] || 0;
    try {
      // invoke 形状保持 { sessionId: state.activeSessionId }（协议指纹按文本
      // 计算）；发起瞬间 activeSessionId === sid，await 返回后按 sid 校验归属。
      const ms = await invoke("get_mode_state", { sessionId: state.activeSessionId });
      // 响应返回后校验 epoch：期间任何本地权威改写（开关翻转、plan/模式切换）
      // 都会使该读取成为陈旧值，丢弃——权威值由对应操作写回。此检查覆盖
      // 两种返回顺序：改写仍在途（原 in-flight 闸场景）与改写已完成（旧读取
      // 最后返回的顺序，in-flight 集合已清空、仅靠集合识别不了）。
      if ((modeStateEpochs[sid] || 0) !== epoch) return;
      if (state.activeSessionId !== sid) {
        runSyncOnSession(sid, function () {
          state.modeState = {
            // tri-mode: the targeted-write branch's fallback uses the same worst-direction-safe
            // auto as the active branch, no longer falling back to the widest yolo (round-9, aligned with the sibling branch).
            mode: ms.mode || "auto",
            multiAgent: !!ms.multi_agent,
            askOnGuardrail: ms.ask_on_guardrail == null ? true : !!ms.ask_on_guardrail,
          };
        });
        return;
      }
      state.modeState = {
        // tri-mode: the default fallback matches the backend's resolved default (unbound
        // plain → auto), no longer displaying the widest yolo (round-2 M family: a read-failure fallback is the worst-direction lie).
        mode: ms.mode || "auto",
        multiAgent: !!ms.multi_agent,
        askOnGuardrail: ms.ask_on_guardrail == null ? true : !!ms.ask_on_guardrail,
      };
    } catch {
      // get 失败同样不得用默认值覆盖已发生的权威改写。
      if ((modeStateEpochs[sid] || 0) !== epoch) return;
      if (state.activeSessionId !== sid) return;
      state.modeState = { mode: "auto", multiAgent: false, askOnGuardrail: true };
    }
  }

  // ── lane global defaults (work/code split; the design lane was merged into work) ───────
  // Draft mode = this lane's global default; a draft switch writes only the
  // global default (no session materialization), and a switch inside an
  // already-materialized session writes only that session's own record
  // (set_plan_mode_next and other per-session commands no longer leak into
  // globals in the backend).
  // Draft-defaults writes get an epoch like the session reads: a
  // get_mode_defaults issued BEFORE an in-flight setDraftMode (or
  // setMultiAgentMode's draft branch) must not land after it and overwrite
  // the just-written default with the pre-switch value — same staleness
  // class the syncModeState epoch guards cover, on the draft surface
  // (round-8 L7-2; the body's disclosure named only setDraftMode).
  let modeDefaultsEpoch = 0;
  async function refreshModeDefaults() {
    const epochAtRead = ++modeDefaultsEpoch;
    try {
      const defaults = await invoke("get_mode_defaults");
      if (defaults && epochAtRead === modeDefaultsEpoch) state.modeDefaults = defaults;
    } catch { /* 读取失败保留旧值/缺省，不打扰交互 */ }
    if (!state.activeSessionId && epochAtRead === modeDefaultsEpoch) {
      state.modeState = currentDraftModeState();
      notify();
    }
  }
  // ChatView 随 pinvouMode 传入当前 lane；草稿态立即按新 lane 默认刷新显示。
function setModeLane(lane) { return pinvouSharedtauriInteraction().setModeLane(lane); }
  // 草稿态 chip 切换：写本 lane 全局默认（setDraftMode 不物化会话——
  // 物化时由 ensureSession 把 lane 默认应用到新会话）。
  // Bound-workspace drafts share the code mode's safety posture: a switch
  // writes the code lane global default (not the work lane), and stages the
  // explicit choice in pendingDraftMode to be applied per session at
  // materialization (the backend resolves only the code lane default for bound
  // sessions, never the work one).
  async function setDraftMode(target) {
    // Any draft-default WRITE invalidates in-flight defaults reads (round-8
    // L7-2 staleness discipline).
    modeDefaultsEpoch += 1;
    // The write's own response obeys the same discipline as the sibling read
    // path above: capture the epoch after the invalidating bump and verify
    // before applying — a defaults read issued after this write bumped the
    // epoch again, and this stale response must not overwrite the fresher
    // value that read lands (round-9).
    const epochAtWrite = modeDefaultsEpoch;
    const boundDraft = !!state.draftWorkspacePath;
    // Bound drafts always write the code lane; unbound drafts follow the
    // current lane (two lanes, work/code; design was merged into work, #428).
    const lane = boundDraft || state.modeLane === "code" ? "code" : "work";
    try {
      const defaults = await invoke("set_mode_default", { lane, mode: target });
      // The whole post-await body obeys the epoch, not just the defaults
      // write (round-10 review): an out-of-order earlier write's response
      // must not re-pin pendingDraftMode or flip the draft display back to
      // its target after a fresher write already landed.
      if (epochAtWrite === modeDefaultsEpoch) {
        if (defaults) state.modeDefaults = defaults;
        if (boundDraft) state.pendingDraftMode = target;
        if (!state.activeSessionId) {
          state.modeState = {
            mode: target,
            multiAgent: !!(state.modeState && state.modeState.multiAgent),
          };
        }
      }
    } catch (e) { addSystemItem(bt("switchModeFailed") + e); }
    notify();
  }

  // ── Code permission prefs (one-shot YOLO confirmation gate) ──────
  // Plain sessions with a bound working directory share the code mode's
  // confirmation gate source of truth before switching to YOLO. Read failures
  // return null (needsYoloConfirmation treats null as unconfirmed — the safe
  // direction: prefer one extra prompt); confirm failures propagate to the UI
  // instead of being swallowed.
  async function getCodePermissionPrefs() {
    try {
      return await invoke("get_code_permission_prefs");
    } catch {
      return null;
    }
  }
  async function confirmCodeYolo() {
    return invoke("confirm_code_yolo");
  }

  // ── 卡片动作辅助 ─────────────────────────────────────────────────
function patchItemById(id, patch) { return pinvouSharedtauriInteraction().patchItemById(id, patch); }
  function pushUserEcho(text, persist) {
    const item = { type: "user", text, time: timeStr() };
    addChatItem(item);
    let message = null;
    if (persist) {
      message = { role: "user", content: [{ type: "text", text }] };
      state.messages.push(message);
    }
    return { item, message };
  }

  // ── Per-session UI 路由 ─────────────────────────────────────────
  // 卡片动作链路有多个 await 边界,用户可能中途切 session。所有 UI 写入(chatItem 增改、
  // pending* 标记、modeState 同步)必须落在【触发 session】的 buffer 上,不能跟着
  // state.activeSessionId 漂走。一律 wrap 进 runSyncOnSession 是因为:sid === active
  // 时它是 no-op 直通,sid !== active 时它 swap-load-fn-save 回 sid 的 buffer。
function runOnSession(sid, fn) { return pinvouSharedtauriInteraction().runOnSession(sid, fn); }
function addSystemItemFor(sid, text) { return pinvouSharedtauriInteraction().addSystemItemFor(sid, text); }
  function addAuthoritySyncNoticeFor(sid, text) {
    runOnSession(sid, function () { addAuthoritySyncNotice(text); });
  }
function patchItemByIdFor(sid, id, patch) { return pinvouSharedtauriInteraction().patchItemByIdFor(sid, id, patch); }


  // ── 思考指示器状态（每次阶段切换重置计时）──────────────────────
function startThinking() { return pinvouSharedtauriInteraction().startThinking(); }
function thinkingTool(name) { return pinvouSharedtauriInteraction().thinkingTool(name); }
function thinkingIdle() { return pinvouSharedtauriInteraction().thinkingIdle(); }
function stopThinking() { return pinvouSharedtauriInteraction().stopThinking(); }

function isActionablePlanCard(sid, itemId, planId) { return pinvouSharedtauriInteraction().isActionablePlanCard(sid, itemId, planId); }

  // ── Plan/YOLO 命令 ───────────────────────────────────────────────
  // sid 在 entry 捕获一次,thread through 所有 await —— 防用户切 session 后,
  // 后续 UI 写入/IPC 把卡片塞到错误的 session。
  async function acceptPlan(itemId, planMarkdown, echo, planId) {
    const sid = state.activeSessionId;
    if (!sid) return;
    const planTicket = String(planId || "").trim();
    if (!planTicket) {
      if (itemId) patchItemByIdFor(sid, itemId, { cardState: "frozen", statusLabel: bt("planHistorical"), resolved: true });
      addSystemItemFor(sid, bt("planTicketInvalid"));
      notify();
      return;
    }
    const planBuffer = getBuffer(sid);
    if (planBuffer && planBuffer.remoteTurnActive && !(await reconcileRemoteTurn(sid))) {
      recordAuthoritySyncDiagnostic("remote_sync_blocked_action", Object.assign({
        operation: "accept_plan",
      }, authoritySyncBufferSnapshot(sid, planBuffer)));
      addAuthoritySyncNoticeFor(sid, bt("remoteTurnSyncing"));
      notify();
      return;
    }
    // sid 前缀省略：isActionablePlanCard 首判已校验 sid === active（审计清理）。
    if (isBusyFor(sid) || !isActionablePlanCard(sid, itemId, planTicket)) return;
    if (planBuffer) {
      planBuffer.localTurnOwned = true;
      planBuffer.remoteTurnActive = false;
      planBuffer.remoteTerminalSeen = false;
      planBuffer.remoteCommittedRevision = "";
    }
    if (itemId) patchItemByIdFor(sid, itemId, { cardState: "approved", statusLabel: bt("approved"), resolved: true });
    let echoEntry = null;
    const displayEcho = echo || bt("echoGo");
    runOnSession(sid, function () { echoEntry = pushUserEcho(displayEcho, true); state.busy = true; startThinking(); });
    notify();
    try {
      const st = await invoke("accept_plan", {
        sessionId: sid,
        planId: planTicket,
        planMarkdown: planMarkdown || "",
        displayMessage: displayEcho,
      });
      // 接受计划 = 后端受理新一轮（reserve_turn + 重跑）：未提交的「打开」转正锁死。
      try { window.PinvouToolStoreEvents.notifyChatRoundCommitted(); } catch { /* silently ignored */ }
      if (planBuffer) planBuffer.deferredRemoteUserEvent = null;
      applyAuthoritativeModeState(sid, st);
    } catch (e) {
      const errorText = String(e && e.message ? e.message : e || "");
      const concurrentTurn = errorText.includes("session_turn_in_progress");
      const planNotActive = errorText.includes("plan_not_active");
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
      if (concurrentTurn && planBuffer) {
        markRemoteTurn(sid, planBuffer, false, "accept_plan_concurrent_turn");
      }
      try {
        const currentMode = await invoke("get_mode_state", { sessionId: sid });
        applyAuthoritativeModeState(sid, currentMode);
      } catch { /* status re-read failure must not mask the original error */ }
      addSystemItemFor(sid, bt("acceptPlanFailed") + e);
    }
    notify();
  }
  async function discardPlan(itemId, planId) {
    const sid = state.activeSessionId;
    const planTicket = String(planId || "").trim();
    if (!sid || !isActionablePlanCard(sid, itemId, planTicket)) return;
    patchItemByIdFor(sid, itemId, {
      cardState: "frozen", statusLabel: bt("planDiscarded"), resolved: true,
      planResolutionConfirmed: false,
    });
    notify();
    try {
      const st = await invoke("discard_plan", { sessionId: sid, planId: planTicket });
      applyAuthoritativeModeState(sid, st);
      patchItemByIdFor(sid, itemId, { planResolutionConfirmed: true });
    } catch (e) {
      const errorText = String(e && e.message ? e.message : e || "");
      const planNotActive = errorText.includes("plan_not_active");
      runOnSession(sid, function () {
        const card = state.chatItems.find(function (item) {
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
          const currentMode = await invoke("get_mode_state", { sessionId: sid });
          applyAuthoritativeModeState(sid, currentMode);
        } catch { /* status re-read failure must not mask the original error */ }
      }
      addSystemItemFor(sid, bt("discardPlanFailed") + e);
    }
    notify();
  }
  async function exitPlanToYolo(targetSessionId) {
    // The caller may pin the target session explicitly: the YOLO confirmation
    // gate issues the switch only after several await round-trips, by which
    // time the user may have switched away and the live active session is no
    // longer the ruling one (same precedent as sessions.js passing meta.id on
    // its materialization path). No-argument calls keep the original semantics:
    // act on the live active session at invocation time (bulb / plan-stuck
    // card).
    const sid = typeof targetSessionId === 'string' && targetSessionId
      ? targetSessionId
      : state.activeSessionId;
    // Draft state: do not materialize a session; rewrite this lane's global
    // default (two-lane semantics).
    if (!sid) { await setDraftMode("yolo"); return; }
    try {
      // The invoke targets sid directly at issue time; after the await, the
      // result is written back to the same sid (the protocol fingerprint is
      // computed from the call text; parameterizing it requires recomputing the
      // interaction-feature digest in step).
      const st = await invoke("exit_plan_to_yolo", { sessionId: sid });
      applyAuthoritativeModeState(sid, st);
    } catch (e) { addSystemItemFor(sid, bt("exitPlanFailed") + e); }
    notify();
  }
  // 灯泡 toggle：plan ↔ yolo
async function setPlanModeNext() { return pinvouSharedtauriInteraction().setPlanModeNext(); }
  // 多智能体开关（ADR-0006）：模型列表下方的会话级开关。后端做名册装配
  // + 名册装配与即时推送；前端只认返回的权威状态。
  // in-flight 期间丢弃**同会话**的后续调用（防重入兜底）：第二次点击会带
  // 着旧的 multiAgentOn 重复提交，其中一次失败的回滚还会覆盖另一次的新
  // 状态。按会话记账而非全局布尔：A 开启在途时不得殃及 B 的开关（复核 P3）。
  const multiAgentToggleInFlight = new Set();
  async function setMultiAgentMode(enabled) {
    const flightKey = state.activeSessionId || "__draft__";
    if (multiAgentToggleInFlight.has(flightKey)) return;
    multiAgentToggleInFlight.add(flightKey);
    try {
      const sid = state.activeSessionId;
      if (!sid) {
        // 草稿态**不物化会话**：否则开个开关就在左侧列表凭空造出一条空
        // 对话（真机反馈）。意图寄存在草稿上，首条消息经 ensureSession
        // 创建会话时才落后端；这里只翻开关行的显示，权威状态以物化时的
        // 后端返回为准。
        state.pendingDraftMultiAgent = !!enabled;
        state.modeState = {
          // tri-mode: the default fallback uses the same worst-direction-safe auto as the active branch (round-9).
          mode: (state.modeState && state.modeState.mode) || "auto",
          multiAgent: !!enabled,
          askOnGuardrail: (state.modeState && state.modeState.askOnGuardrail) !== false,
        };
        // 草稿分支会从 try 内提前返回，走不到函数末尾的 notify()。
        // 必须在这里主动发布快照，否则拨杆只能等下一次无关状态事件才刷新。
        notify();
        return;
      }
      // 乐观翻转：开启在后端要做名册装配与引擎同步（可能耗时数百毫秒），
      // 等返回再翻拨杆会像"点了没反应"。先翻显示并 notify，成功后用后端
      // 权威状态复核；失败回滚显示并提示。in-flight 闸已挡并发重入。
      const previousMultiAgent = !!(state.modeState && state.modeState.multiAgent);
      // 翻转即刻 bump 该会话 epoch：让在途的 get_mode_state 读取（syncModeState）
      // 全部作废，无论其响应在 toggle 进行中还是完成后返回——陈旧读取一律
      // 丢弃，权威值由下方 set_multi_agent_mode 返回后写回（审计意见）。
      bumpModeStateEpoch(sid);
      state.modeState = {
        // tri-mode: the default fallback uses the same worst-direction-safe auto as the active branch (round-9).
        mode: (state.modeState && state.modeState.mode) || "auto",
        multiAgent: !!enabled,
        askOnGuardrail: (state.modeState && state.modeState.askOnGuardrail) !== false,
      };
      notify();
      try {
        const st = await invoke("set_multi_agent_mode", { sessionId: sid, enabled: !!enabled });
        applyAuthoritativeModeState(sid, st);
      } catch (invokeError) {
        // 回滚与报错必须定向回触发会话：await 期间用户可能已切走，直接改
        // 全局 modeState 会把回滚砸进别的会话、报错落错聊天（复核 P1）。
        runOnSession(sid, function () {
          state.modeState = {
            // tri-mode: the rollback fallback likewise takes the worst-direction-safe auto (round-9).
            mode: (state.modeState && state.modeState.mode) || "auto",
            multiAgent: previousMultiAgent,
            askOnGuardrail: (state.modeState && state.modeState.askOnGuardrail) !== false,
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
async function planStuckReplan(itemId) { return pinvouSharedtauriInteraction().planStuckReplan(itemId); }
  async function planStuckGo(itemId, targetSessionId) {
    // The plan-stuck card's session is captured when the card is rendered;
    // gate callers pass it explicitly. A no-arg call (legacy) targets
    // live-active at invoke time. The explicit form keeps exitPlanToYolo and
    // the follow-up prompt on the same adjudicated session even if the user
    // switches away mid-flight (#445 R9: a no-arg read here bypassed both
    // the gate's sid threading and, for a bound session launched from
    // ChatView, the one-time YOLO confirmation decided in the UI layer).
    const sid = typeof targetSessionId === 'string' && targetSessionId
      ? targetSessionId
      : state.activeSessionId;
    if (!sid) return;
    // In-flight re-entry guard + mark handled only after the submit (same discipline as planStuckGoAuto
    // below, review P2 shape): the wait window spans one backend round trip; without a synchronous latch a
    // second click dispatches a duplicate mode switch / duplicate follow-up turn. The old shape set resolved:true
    // before the await — a rejected exitPlanToYolo left the card action-stripped and the exception unhandled (an
    // unhandled onClick rejection); failure now restores clickability and surfaces a targeted notice.
    let stuck = null;
    runSyncOnSession(sid, function () {
      // The type must match the producer verbatim (chat-events drops the card as "plan_stuck"; the
      // web lane consumes the same string): a "planStuck" spelling drift here once left the latch
      // permanently dead on production data with double-click double-dispatch while tests seeded the
      // fake shape and stayed green. tests/interaction_write_routing's latch-producer type pin locks this contract.
      stuck = state.chatItems.find(function (candidate) {
        return candidate && candidate.type === "plan_stuck" && candidate.id === itemId;
      }) || null;
    });
    if (stuck && (stuck.submitting || stuck.resolved)) return;
    patchItemByIdFor(sid, itemId, { submitting: true }); notify();
    try {
      // Direct invoke (the planStuckGoAuto pattern): the shared exitPlanToYolo
      // wrapper swallows failures internally (exitPlanFailed system item),
      // so a rejected switch is undetectable through it — commit-marking
      // needs the rejection to keep the card actionable.
      const st = await invoke("exit_plan_to_yolo", { sessionId: sid });
      applyAuthoritativeModeState(sid, st);
    } catch (e) {
      patchItemByIdFor(sid, itemId, { submitting: false });
      addSystemItemFor(sid, bt("exitPlanFailed") + e); notify();
      return;
    }
    patchItemByIdFor(sid, itemId, { resolved: true, submitting: false }); notify();
    // 补充指令必须发往触发会话：await exitPlanToYolo 期间用户可能已切走，
    // 直接 sendMessage 会把"继续执行"发到切换后的会话（审计遗漏补修）。
    // sendMessageToSession 校验失败（会话已删/对账中）会 throw，必须接住并
    // 定向提示，否则成为 React onClick 上的 unhandled rejection，用户无感知。
    try {
      await sendMessageToSession(sid, bt("planStuckGoPrompt"));
    } catch (e) { addSystemItemFor(sid, bt("planContinueFailed") + e); notify(); }
  }
  // The plan-stuck card's continue-in-Auto (tri-mode): the same follow-up instruction as planStuckGo,
  // but the switch target is Auto (per-action approval) instead of Yolo — the UI layer's one-time
  // confirmation gate only applies to yolo, Auto passes no gate. The sid capture / targeted write-back follow the same contract as planStuckGo.
  async function planStuckGoAuto(itemId, targetSessionId) {
    const sid = typeof targetSessionId === 'string' && targetSessionId
      ? targetSessionId
      : state.activeSessionId;
    if (!sid) return;
    // In-flight re-entry guard (the decideApproval/setMultiAgentMode pattern): the wait window of
    // "mark handled only after the switch commits" (reviewer P2) spans one hot-sync round trip;
    // without a synchronous latch a second click dispatches a duplicate set_session_mode and a
    // duplicate queued follow-up turn. The catch keeps the card actionable and surfaces the error.
    let stuck = null;
    runSyncOnSession(sid, function () {
      stuck = state.chatItems.find(function (candidate) {
        return candidate && candidate.type === "plan_stuck" && candidate.id === itemId;
      }) || null;
    });
    if (stuck && (stuck.submitting || stuck.resolved)) return;
    patchItemByIdFor(sid, itemId, { submitting: true }); notify();
    try {
      const st = await invoke("set_session_mode", { sessionId: sid, mode: "auto" });
      applyAuthoritativeModeState(sid, st);
      patchItemByIdFor(sid, itemId, { resolved: true, submitting: false }); notify();
    } catch (e) {
      patchItemByIdFor(sid, itemId, { submitting: false });
      addSystemItemFor(sid, bt("switchModeFailed") + e); notify(); return;
    }
    try { await sendMessageToSession(sid, bt("planStuckGoPrompt")); }
    catch (e) { addSystemItemFor(sid, bt("planContinueFailed") + e); notify(); }
  }

  // ── tri-mode unified mode switch / per-session ask-on-guardrail / approval-card decision ──────────
  // The sid capture follows the exitPlanToYolo contract: callers may pin the target session
  // explicitly; a no-arg call reads live-active. Authoritative write-backs all go through applyAuthoritativeModeState (the shared epoch table).
  async function setSessionMode(targetSessionId, mode, askOnGuardrail) {
    const sid = typeof targetSessionId === 'string' && targetSessionId
      ? targetSessionId
      : state.activeSessionId;
    if (!sid) return;
    try {
      const args = { sessionId: sid, mode };
      // Optional parameter: carried only when passed explicitly (undefined never hits the wire, keeping the protocol fingerprint minimal).
      if (askOnGuardrail === true || askOnGuardrail === false) args.askOnGuardrail = askOnGuardrail;
      const st = await invoke("set_session_mode", args);
      applyAuthoritativeModeState(sid, st);
    } catch (e) { addSystemItemFor(sid, bt("switchModeFailed") + e); }
    notify();
  }
  // Per-session "ask on guardrail hit" toggle: optimistic flip (the backend hot-syncs the engine's
  // safety posture, which can take a while); on failure, roll back and notify on the triggering
  // session (the setMultiAgentMode optimistic pattern). The optimistic write goes through
  // runSyncOnSession(sid): an explicit sid parameter is also valid (not just live-active) — a bare
  // global write would land the flip on the new session's face in the one-beat-lagging switch window (round-13 review).
  async function setAskOnGuardrail(targetSessionId, enabled) {
    const sid = typeof targetSessionId === 'string' && targetSessionId
      ? targetSessionId
      : state.activeSessionId;
    if (!sid) return;
    let previousAsk = true;
    runSyncOnSession(sid, function () {
      previousAsk = (state.modeState && state.modeState.askOnGuardrail) !== false;
      bumpModeStateEpoch(sid);
      state.modeState = {
        mode: (state.modeState && state.modeState.mode) || "auto",
        multiAgent: !!(state.modeState && state.modeState.multiAgent),
        askOnGuardrail: !!enabled,
      };
    });
    notify();
    try {
      const st = await invoke("set_ask_on_guardrail", { sessionId: sid, enabled: !!enabled });
      applyAuthoritativeModeState(sid, st);
    } catch (e) {
      runOnSession(sid, function () {
        state.modeState = {
          mode: (state.modeState && state.modeState.mode) || "auto",
          multiAgent: !!(state.modeState && state.modeState.multiAgent),
          askOnGuardrail: previousAsk,
        };
        addSystemItem(bt("switchModeFailed") + e);
      });
      // Reconcile from the persisted source of truth (reviewer P3): two
      // opposite toggles that BOTH fail would each restore their captured
      // optimistic value, leaving the chip showing enabled while the durable
      // setting is disabled. The authoritative re-read converges the chip to
      // what the backend actually kept; a re-read failure must not mask the
      // original error, so the rollback value above stays the fallback.
      // The syncModeState epoch discipline: captured before the re-read is issued, verified after
      // the await returns — an authoritative chat:mode_changed broadcast landing in the window has
      // bumped the epoch, making this read stale: skip the write-back (keep the rollback value above; the
      // authoritative value already landed via the convergence point, else the old snapshot would punch it back until the next event — round-7 recheck).
      const epoch = modeStateEpochs[sid] || 0;
      try {
        const ms = await invoke("get_mode_state", { sessionId: sid });
        if ((modeStateEpochs[sid] || 0) === epoch) applyAuthoritativeModeState(sid, ms);
      } catch { /* keep the optimistic rollback value */ }
    }
    notify();
  }
  // Approval-card decision round-trip (decide_approval): sid is captured at entry (the user may switch
  // sessions during the await; writes must be targeted back to the triggering session — the acceptPlan
  // contract). Optimistically marked submitting; success writes the terminal state (localized terminal copy mapped from cardState by the render layer), failure restores active.
  async function decideApproval(approvalId, decision, targetSessionId) {
    const sid = typeof targetSessionId === 'string' && targetSessionId
      ? targetSessionId
      : state.activeSessionId;
    if (!sid || !approvalId) return;
    let card = null;
    runSyncOnSession(sid, function () {
      card = state.chatItems.find(function (candidate) {
        return candidate && candidate.type === "approval" && candidate.approvalId === approvalId;
      }) || null;
    });
    if (!card || card.resolved) return;
    // In-flight re-entry guard (the setMultiAgentMode pattern): a double-click faster than the
    // re-render dispatches a second decide_approval; once the first has succeeded, the second
    // only gets "approval not found" and pushes a failure notice into the session stream.
    if (card.submitting) return;
    const previous = { resolved: !!card.resolved, cardState: card.cardState || "active" };
    runSyncOnSession(sid, function () { card.submitting = true; });
    notify();
    try {
      await invoke("decide_approval", { sessionId: sid, approvalId, decision });
      runSyncOnSession(sid, function () {
        card.submitting = false;
        card.resolved = true;
        card.cardState = decision === "deny" ? "denied"
          : decision === "allow_for_session" ? "allowed_session" : "allowed";
        card.statusLabel = "";
      });
    } catch (e) {
      runSyncOnSession(sid, function () {
        card.submitting = false;
        // M5: terminal cleanup (chat:done) may already have settled the card to cancelled in
        // place during the await. Before restoring pre-call state we must yield to the terminal
        // state — otherwise a cancelled card revives clickable and every later click appends
        // another failure notice. A chat:approval_resolved echo likewise does not override decided
        // cards (echo handling applies to undecided cards only).
        // Unknown terminal (host P2): the backend broadcasts the decision "unknown" convergence
        // event before returning Err; when the event lands first the card is already in the undecidable
        // unknown state — restoring pre-call active would flip it back to clickable (re-inviting a click
        // on a possibly consumed decision), so we must yield. When the event lands later this restores as usual and the convergence event then rewrites the card to unknown (undecided cards only); both arrival orders converge to undecidable.
        if (card.cardState === "unknown") return;
        if (!card.resolved) {
          card.resolved = previous.resolved;
          card.cardState = previous.cardState;
        }
      });
      addSystemItemFor(sid, bt("approvalDecisionFailed") + e);
    }
    notify();
  }
  // The session's currently pending approval cards (for remount / session-switch recovery).
  // Read failures are treated as empty — recovery is best-effort and must not block session switches.
  async function getPendingApprovals(targetSessionId) {
    const sid = typeof targetSessionId === 'string' && targetSessionId
      ? targetSessionId
      : state.activeSessionId;
    if (!sid) return [];
    try { return await invoke("get_pending_approvals", { sessionId: sid }) || []; }
    catch { return []; }
  }

  // ── 用户交互卡 ───────────────────────────────────────────────────
  // 卡片动作链路有 await 边界：entry 先捕获触发会话 sid，invoke 与后续全部 UI 写入
  // 都定向到 sid（runOnSession / patchItemByIdFor），避免用户提交期间切会话导致
  // echo/restoredAnswers 漏写触发会话或污染当前会话（与 acceptPlan 同一约定）。
async function submitUserInput(itemId, toolCallId, answers, questions) { return pinvouSharedtauriInteraction().submitUserInput(itemId, toolCallId, answers, questions); }
  async function cancelUserInput(itemId, toolCallId) {
    const sid = state.activeSessionId;
    if (!sid) return;
    try { await invoke("cancel_user_input", { toolCallId, sessionId: sid }); } catch { /* on cancel failure, wait for backend timeout cleanup */ }
    patchItemByIdFor(sid, itemId, { resolved: true, cardState: "cancelled" });
    notify();
  }

  // ── 编辑上一轮 / 手动压缩 ─────────────────────────────────────────
  async function editLastTurn(newText) {
    if (state.busy || !state.activeSessionId) return;
    newText = (newText || "").trim();
    if (!newText) return;
    const sid = state.activeSessionId;
    const editBuffer = getBuffer(sid);
    // 编辑前先收敛远端对账(与 web bridge 的 editLastTurn 对齐):失败对账
    // 状态下编辑会被陈旧 committed 事件重武装旧 revision,污染新一轮。
    if (editBuffer && editBuffer.remoteTurnActive && !(await reconcileRemoteTurn(sid))) {
      recordAuthoritySyncDiagnostic("remote_sync_blocked_action", Object.assign({
        operation: "edit_last_turn",
      }, authoritySyncBufferSnapshot(sid, editBuffer)));
      addAuthoritySyncNoticeFor(sid, bt("remoteTurnSyncing"));
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
    // 失败回滚快照（与 web bridge 的 editLastTurn 对齐）：await 期间可能
    // 切走，恢复必须定向回 sid 的 buffer，不能直接改全局显示。
    const previous = {
      messages: [...state.messages],
      chatItems: [...state.chatItems],
      busy: state.busy,
      thinking: Object.assign({}, state.thinking),
      currentStreamText: context.currentStreamText,
      currentStreamId: context.currentStreamId,
    };
    // Remove the latest displayable user turn and everything after it, then
    // append the replacement. Tool results and internal runtime envelopes also
    // use role="user", so a bare role scan would cut at the wrong boundary.
    let cut = -1;
    for (let i = state.messages.length - 1; i >= 0; i--) {
      const candidate = state.messages[i];
      if (candidate.role === "user" && userMessageDisplayText(candidate.content)) { cut = i; break; }
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
    turnUsageDirty[sid] = false; // 编辑重跑=新一轮，同 doSendFor 重置口径保护（用捕获的 sid，web 对齐）
    try {
      await invoke("edit_last_turn", { newMessage: newText, sessionId: state.activeSessionId });
      // 编辑重跑 = 后端受理新一轮：未提交的「打开」转正锁死（同 doSendFor）。
      try { window.PinvouToolStoreEvents.notifyChatRoundCommitted(); } catch { /* silently ignored */ }
    } catch (e) {
      // 失败恢复必须定向触发会话（web 对齐）：直接写全局会把 busy/错误提示
      // 砸进别的会话（编辑是在 sid 上发起的）。
      if (editBuffer) editBuffer.localTurnOwned = false;
      runSyncOnSession(sid, function () {
        state.messages = previous.messages;
        state.chatItems = previous.chatItems;
        state.busy = previous.busy;
        state.thinking = previous.thinking;
        context.currentStreamText = previous.currentStreamText;
        context.currentStreamId = previous.currentStreamId;
        addSystemItem("⚠️ " + e);
      });
      notify();
    }
  }

    return {
      refreshSuperPerm,
      toggleSuperPerm,
      syncModeState,
      patchItemById,
      pushUserEcho,
      runOnSession,
      addSystemItemFor,
      patchItemByIdFor,
      startThinking,
      thinkingTool,
      thinkingIdle,
      stopThinking,
      acceptPlan,
      discardPlan,
      exitPlanToYolo,
      setPlanModeNext,
      setDraftMode,
      setModeLane,
      refreshModeDefaults,
      getCodePermissionPrefs,
      confirmCodeYolo,
      setMultiAgentMode,
      planStuckReplan,
      planStuckGo,
      planStuckGoAuto,
      setSessionMode,
      setAskOnGuardrail,
      decideApproval,
      getPendingApprovals,
      submitUserInput,
      cancelUserInput,
      editLastTurn,
    };
  };
})();
