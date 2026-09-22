/**
 * bridge-shared-helpers.js — 供 plain-script bridge（platform/web、platform/tauri）共用的桥接工具函数集。
 *
 * 背景：platform/{web,tauri}/bridge*.js 是以 <script src> 加载的普通脚本（非 ES module）。
 * 此前数百个逐字相同（byte-identical）的桥接工具函数在两条 lane 里各存一份，维护时必须双改；
 * 现已合并：238 个共享函数只在 sharedBridgeBase 定义一次（正文取自原 web 侧逐字镜像），
 * 每个 cluster 只是薄包装：
 *   - web / tauriMain: 两条 lane 主 IIFE 的共享函数；
 *   - tauri<X>:        tauri/bridge/<feature>.js feature 工厂的共享函数；
 *   - <base>:<offset>: 个别嵌套宿主函数内部的共享函数（web/tauri 成对逐字相同，共用工厂注册两个 key）。
 * 各 lane 在首次调用时通过 window.PinvouBridgeShared.create(cluster, deps) 注入本作用域绑定：
 *   - 普通/单元格语义不变：会被重新赋值或有初始化时序约束的绑定仍以 get/set value 单元格传入保持活性；
 *   - 12 个 web 侧以 get 单元格传入、tauri 侧以普通值传入的依赖（CELL_DEP_NAMES）由 ensureCells
     统一补齐为只读单元格，合并后的函数体统一按 .value 读取，lane 侧 deps 字面量零改动；
 *   - FORWARDER_DEP_NAMES：部分 lane 会把其它 cluster 的共享函数以同名 deps（lane 内转发函数）传入；
 *     有传入时优先使用传入值，保持与合并前完全一致的跨 cluster 解析路径。
 * 每个 cluster 的导出名单与合并前逐一同名同序，导出的函数正文与某一侧原镜像逐字一致。
 * 与 markdown-bridge-fallback.js 相同：必须随 index.html 在两条 bridge 之前以普通脚本加载。
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";

  // 12 个依赖在 web lane 以 get 单元格传入、tauri lane 以普通值传入；合并后的函数体统一按 .value 读取。
  // ensureCells 只在这 12 个名字上补齐单元格：已是单元格（对象且含 value 属性）原样透传，
  // 普通值包一层只读单元格 —— lane 侧 deps 字面量保持零改动。
  const CELL_DEP_NAMES = [
    "BT_TABLE",
    "PINVOU_SCENE_EVENTS_STORAGE_PREFIX",
    "sessionStates",
    "scheduledRunSessionOwners",
    "MAX_SCHEDULED_RUN_SESSION_OWNERS",
    "scheduledRunOpenInFlight",
    "personaPlaceholderTitles",
    "SHELL_TOOL_NAMES",
    "shellPollState",
    "DELIVERABLE_EXTS",
    "deletedPersonaIds",
    "VOICE_DEVICE_REQUEST_TIMEOUT_MS",
  ];

  function isValueCell(candidate) {
    return !!candidate && typeof candidate === "object" && "value" in candidate;
  }

  function ensureCells(deps) {
    if (deps == null) throw new TypeError("bridge shared deps object required");
    const wrapped = Object.assign({}, deps);
    for (const name of CELL_DEP_NAMES) {
      if (!isValueCell(wrapped[name])) wrapped[name] = { get value() { return deps[name]; } };
    }
    return wrapped;
  }

  // 共享基础工厂：所有 cluster 的函数体只在这里定义一次。
  // deps 解构 = 各 cluster 依赖名的并集（去掉与函数声明同名的名字）。
  function sharedBridgeBase(deps) {
    // 全量并集，按 cluster 首次出现排序；与函数声明同名的依赖名不在此解构（见 FORWARDER_DEP_NAMES）。
    const {
      state, invoke, freshBuffer, restoreEvictedSessionDraft, pruneScheduledSessionBuffers, pruneSessionBuffers,
      recordAuthoritySyncDiagnostic, runSyncOnSession, notify, SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY, persistScheduledTaskTemplateSources,
      scheduledTaskBackendInput, forgetScheduledTaskTemplateSource, messageHasToolBlock, addChatItem, enterDraft, hydratedMessageKey,
      switchToSessionInternal, openScheduledRunChatOnce, loadWorkingSetFrom, purgeSessionBuffer, refreshHistoryList, saveWorkingSetTo,
      normalizeTerminalTail, latestShellToolIsWaitObserver, extractArtifactPaths, summonPinvou, parseToolResultPayload, clearMonitorBaseline,
      pollMonitor, loadSessionModel, setActiveModel, currentDraftModeState, loadMemoryOverview, setDraftMode,
      applyAuthoritativeModeState, sendMessage, pushUserEcho, flushAssistantMessageToHistory, dialogOpen, addAttachmentByPath,
      ensureSession, stopMediaTracks, finishVoiceInput, BT_TABLE, PINVOU_SCENE_EVENTS_STORAGE_PREFIX, sessionStates,
      sessionBufferTouchClock, scheduledRunSessionOwners, scheduledRunOwnerTouchClock, MAX_SCHEDULED_RUN_SESSION_OWNERS, scheduledTaskTemplateSources, scheduledTaskRequestTokens,
      scheduledTaskRefreshInFlight, scheduledRecentRunsRequestToken, scheduledRunEventRefreshTimer, scheduledTaskPendingLoads, scheduledTaskSelectionGeneration, scheduledRunShortcutRefreshes,
      SCHEDULED_LINK_POLL_DEADLINE_MS, SCHEDULED_LINK_POLL_FAST_ATTEMPTS, SCHEDULED_LINK_POLL_FAST_MS, SCHEDULED_LINK_POLL_SLOW_MS, scheduledTaskAutoCreateSeq, scheduledRunOpenInFlight,
      personaPlaceholderTitles, sessionSwitchRequestToken, SHELL_TOOL_NAMES, shellPollState, DELIVERABLE_EXTS, monitorBaseline,
      monitorIntervalId, gpuUtilHistory, settingsWriteQueue, modelsLoadSeq, memoryOverviewSeq, personaPoolCache,
      deletedPersonaIds, lastEquippedSid, mountedCollectionDraftTarget, mountedCollectionUpdate, activeVoiceInput, VOICE_DEVICE_REQUEST_TIMEOUT_MS,
      pe, sid
    } = deps;

  // web+tauriMain 共享
  // biome-ignore lint/suspicious/noFunctionAssign: forwarder-dep routing reassigns the shared impl when the lane passes its own (see FORWARDER_DEP_NAMES)
  function bt(key) {
    const lang = state.settings && state.settings.language;
    const m = lang === "en" ? BT_TABLE.value.en : lang === "ja" ? BT_TABLE.value.ja : BT_TABLE.value.zh;
    return m[key] === undefined ? BT_TABLE.value.zh[key] : m[key];
  }

  // web+tauriMain 共享
  function textMatchesBtKey(text, key) {
    return text.includes(BT_TABLE.value.zh[key]) || text.includes(BT_TABLE.value.en[key]) || text.includes(BT_TABLE.value.ja[key]);
  }

  // web+tauriMain 共享
  function isDefaultChatTitle(title) {
    return [BT_TABLE.value.zh.newChatFallbackTitle, BT_TABLE.value.en.newChatFallbackTitle, BT_TABLE.value.ja.newChatFallbackTitle]
      .includes(title);
  }

  // web+tauriMain 共享
  function authoritySyncBufferSnapshot(sid, buf) {
    return {
      session_id: sid || "",
      active_session_id: state.activeSessionId || "",
      buffer_present: !!buf,
      local_turn_owned: !!(buf && buf.localTurnOwned),
      remote_turn_active: !!(buf && buf.remoteTurnActive),
      remote_terminal_seen: !!(buf && buf.remoteTerminalSeen),
      loaded_from_disk: !!(buf && buf.loadedFromDisk),
      buffer_busy: !!(buf && buf.busy),
      ui_busy: !!state.busy,
      message_count: buf && Array.isArray(buf.messages) ? buf.messages.length : null,
      chat_item_count: buf && Array.isArray(buf.chatItems) ? buf.chatItems.length : null,
      queued_count: buf && Array.isArray(buf.queued) ? buf.queued.length : null,
      session_revision: String(buf && buf.sessionRevision || ""),
      committed_revision: String(buf && buf.remoteCommittedRevision || ""),
      expected_assistant_key_length: String(buf && buf.remoteExpectedAssistantKey || "").length,
      baseline_message_count: buf && buf.remoteBaselineMessageCount != null
        ? Number(buf.remoteBaselineMessageCount)
        : null,
      baseline_trusted: !!(buf && buf.remoteBaselineTrusted),
    };
  }

  // web+tauriMain 共享
  function normalizePinvouScene(scene) {
    scene = String(scene || "").trim();
    return /^(work:document-writing|work:personal-workbench|design:poster|design:data-visualization|design:ppt)$/.test(scene) ? scene : "";
  }

  // web+tauriMain 共享
  function pinvouSceneStorageKey(sid) {
    return PINVOU_SCENE_EVENTS_STORAGE_PREFIX.value + String(sid || "").trim();
  }

  // web+tauriMain 共享
  function normalizePinvouSceneEvents(events) {
    return (Array.isArray(events) ? events : []).map(function (event) {
      const pos = Number(event && event.pos);
      const scene = normalizePinvouScene(event && event.scene);
      if (!Number.isFinite(pos) || pos < 0 || !scene) return null;
      return { pos: Math.floor(pos), scene };
    }).filter(Boolean).sort(function (left, right) { return left.pos - right.pos; });
  }

  // web+tauriMain 共享
  function loadPinvouSceneEventsForSession(sid) {
    if (!sid || !window.localStorage) return [];
    try {
      return normalizePinvouSceneEvents(JSON.parse(window.localStorage.getItem(pinvouSceneStorageKey(sid)) || "[]"));
    } catch {
      return [];
    }
  }

  // web+tauriMain 共享
  function savePinvouSceneEventsForSession(sid, events) {
    if (!sid) return;
    const normalized = normalizePinvouSceneEvents(events);
    try {
      if (window.localStorage) {
        window.localStorage.setItem(pinvouSceneStorageKey(sid), JSON.stringify(normalized));
      }
    } catch {
      // localStorage 只作旧版本迁移和离线缓存，写失败不影响后端 sidecar。
    }
    Promise.resolve().then(function () {
      return invoke("save_session_pinvou_scene_events", {
        sessionId: sid,
        events: normalized,
      });
    }).catch(function () {});
  }

  // web+tauriMain 共享
  function recordPinvouSceneForMessage(sid, pos, scene) {
    scene = normalizePinvouScene(scene);
    pos = Number(pos);
    if (!sid || !scene || !Number.isFinite(pos) || pos < 0) return;
    pos = Math.floor(pos);
    let events = normalizePinvouSceneEvents(state.pinvouSceneEvents)
      .filter(function (event) { return event.pos !== pos; });
    events.push({ pos, scene });
    events = normalizePinvouSceneEvents(events);
    state.pinvouSceneEvents = events;
    savePinvouSceneEventsForSession(sid, events);
  }

  // web+tauriMain 共享
  function pinvouSceneForMessagePos(pos) {
    const events = normalizePinvouSceneEvents(state.pinvouSceneEvents);
    for (let i = 0; i < events.length; i++) {
      if (events[i].pos === pos) return events[i].scene;
    }
    return "";
  }

  // web+tauriSessions 共享
  // biome-ignore lint/suspicious/noFunctionAssign: forwarder-dep routing reassigns the shared impl when the lane passes its own (see FORWARDER_DEP_NAMES)
  function getBuffer(id) {
    if (!id) return null;
    if (!sessionStates.value[id]) {
      sessionStates.value[id] = freshBuffer();
      restoreEvictedSessionDraft(id, sessionStates.value[id]);
    }
    return touchSessionBuffer(id, sessionStates.value[id], id.indexOf("sched-") === 0);
  }

  // web+tauriSessions 共享
  function isProtectedScheduledBuffer(id, buf) {
    return id === state.activeSessionId ||
      !!buf.busy ||
      !!buf.remoteTurnActive ||
      buf.scheduledInitialTurnPhase === "active" ||
      !!(buf.queued && buf.queued.length) ||
      !!(state.scheduledRunContext && state.scheduledRunContext.sessionId === id) ||
      state.scheduledTaskCreationSessionId === id;
  }

  // web+tauriSessions 共享
  function touchSessionBuffer(id, buf, scheduled) {
    if (!buf) return null;
    if (scheduled) buf.scheduledRunSession = true;
    buf.lastTouched = ++sessionBufferTouchClock.value;
    if (buf.scheduledRunSession) pruneScheduledSessionBuffers(id);
    pruneSessionBuffers(id);
    return buf;
  }

  // web+tauriSessions 共享
  function registerScheduledRunOwner(id, phase) {
    if (typeof id !== "string" || !id) return null;
    let owner = scheduledRunSessionOwners.value[id];
    if (!owner) owner = scheduledRunSessionOwners.value[id] = { phase: null, lastTouched: 0 };
    if (owner.phase !== "terminal" && phase) owner.phase = phase;
    owner.lastTouched = ++scheduledRunOwnerTouchClock.value;
    pruneScheduledRunSessionOwners();
    return owner;
  }

  // web+tauriSessions 共享
  function scheduledRunOwnerVisibleRank(id) {
    const runs = state.scheduledTaskRuns || [];
    for (let i = 0; i < runs.length; i++) {
      if (runs[i] && runs[i].sessionId === id) return i;
    }
    return -1;
  }

  // web+tauriSessions 共享
  function scheduledRunOwnerPriority(id) {
    if (id === state.activeSessionId ||
        (state.scheduledRunContext && state.scheduledRunContext.sessionId === id)) return 3;
    if (scheduledRunOwnerVisibleRank(id) >= 0) return 2;
    return 1;
  }

  // web+tauriSessions 共享
  function pruneScheduledRunSessionOwners() {
    const ids = Object.keys(scheduledRunSessionOwners.value);
    if (ids.length <= MAX_SCHEDULED_RUN_SESSION_OWNERS.value) return;
    ids.sort(function (left, right) {
      const priorityDelta = scheduledRunOwnerPriority(right) - scheduledRunOwnerPriority(left);
      if (priorityDelta) return priorityDelta;
      const leftVisibleRank = scheduledRunOwnerVisibleRank(left);
      const rightVisibleRank = scheduledRunOwnerVisibleRank(right);
      if (leftVisibleRank >= 0 || rightVisibleRank >= 0) {
        if (leftVisibleRank < 0) return 1;
        if (rightVisibleRank < 0) return -1;
        if (leftVisibleRank !== rightVisibleRank) return leftVisibleRank - rightVisibleRank;
      }
      const touchDelta = (scheduledRunSessionOwners.value[right].lastTouched || 0) -
        (scheduledRunSessionOwners.value[left].lastTouched || 0);
      return touchDelta || left.localeCompare(right);
    });
    for (let i = MAX_SCHEDULED_RUN_SESSION_OWNERS.value; i < ids.length; i++) {
      delete scheduledRunSessionOwners.value[ids[i]];
    }
  }

  // web+tauriSessions 共享
  // biome-ignore lint/suspicious/noFunctionAssign: forwarder-dep routing reassigns the shared impl when the lane passes its own (see FORWARDER_DEP_NAMES)
  function isScheduledRunTerminal(status) {
    const value = String(status || "").toLowerCase();
    return ["completed", "failed", "canceled"].includes(value);
  }

  // web+tauriSessions 共享
  // biome-ignore lint/suspicious/noFunctionAssign: forwarder-dep routing reassigns the shared impl when the lane passes its own (see FORWARDER_DEP_NAMES)
  function rememberScheduledRunOwner(run) {
    if (!run) return;
    const id = typeof run.sessionId === "string" ? run.sessionId.trim() : "";
    if (!id) return;
    const status = String(run.status || "").toLowerCase();
    const phase = isScheduledRunTerminal(status)
      ? "terminal"
      : (status === "queued" || status === "running" ? "active" : null);
    registerScheduledRunOwner(id, phase);
  }

  // web+tauriSessions 共享
  function scheduledRunBuffer(id) {
    const buf = getBuffer(id);
    if (!buf) return null;
    registerScheduledRunOwner(id, null);
    return touchSessionBuffer(id, buf, true);
  }

  // web+tauriSessions 共享
  function markScheduledInitialTurnActive(id) {
    const buf = scheduledRunBuffer(id);
    const owner = registerScheduledRunOwner(id, "active");
    if (!buf) return buf;
    if (buf.scheduledInitialTurnPhase === "terminal" || (owner && owner.phase === "terminal")) {
      buf.scheduledInitialTurnPhase = "terminal";
      buf.busy = false;
      if (state.activeSessionId === id) state.busy = false;
      return buf;
    }
    buf.scheduledInitialTurnPhase = "active";
    buf.busy = true;
    if (state.activeSessionId === id) state.busy = true;
    return buf;
  }

  // web+tauriSessions 共享
  function markScheduledInitialTurnTerminal(id) {
    const buf = scheduledRunBuffer(id);
    registerScheduledRunOwner(id, "terminal");
    if (!buf || buf.scheduledInitialTurnPhase === "terminal") return buf;
    if (buf.scheduledInitialTurnPhase !== "active") {
      buf.scheduledInitialTurnPhase = "active";
    }
    buf.scheduledInitialTurnPhase = "terminal";
    return buf;
  }

  // web+tauriSessions 共享
  function beginScheduledOpenActivation(id) {
    const previous = sessionStates.value[id] || null;
    const snapshot = {
      id,
      existed: !!previous,
      previousPhase: previous && previous.scheduledInitialTurnPhase,
      previousBusy: previous ? !!previous.busy : false,
      previousStateBusy: state.activeSessionId === id ? !!state.busy : null,
    };
    const buf = markScheduledInitialTurnActive(id);
    snapshot.buffer = buf;
    snapshot.activationTouch = buf && buf.lastTouched;
    snapshot.changed = !!buf && (
      !snapshot.existed ||
      snapshot.previousPhase !== buf.scheduledInitialTurnPhase ||
      snapshot.previousBusy !== !!buf.busy
    );
    return snapshot;
  }

  // web+tauriSessions 共享
  function rollbackScheduledOpenActivation(snapshot) {
    if (!snapshot || !snapshot.changed) return;
    const current = sessionStates.value[snapshot.id];
    if (!current || current !== snapshot.buffer) return;
    if (current.scheduledInitialTurnPhase === "terminal") return;
    if (current.lastTouched !== snapshot.activationTouch) return;
    if (snapshot.existed) {
      current.scheduledInitialTurnPhase = snapshot.previousPhase;
      current.busy = snapshot.previousBusy;
    } else {
      delete sessionStates.value[snapshot.id];
    }
    if (state.activeSessionId === snapshot.id && snapshot.previousStateBusy !== null) {
      state.busy = snapshot.previousStateBusy;
    }
  }

  // web+tauriMain 共享
  function markRemoteTurn(sid, buf, preserveCommittedRevision, cause) {
    if (!sid || !buf || buf.localTurnOwned) return;
    const wasActive = !!buf.remoteTurnActive;
    if (!buf.remoteTurnActive) {
      const meta = state.sessions.find(function (session) { return session.id === sid; });
      buf.remoteBaselineTrusted = !!buf.loadedFromDisk;
      buf.remoteBaselineMessageCount = buf.loadedFromDisk
        ? (buf.messages || []).length
        : Number(meta && meta.message_count);
      if (!Number.isFinite(buf.remoteBaselineMessageCount)) buf.remoteBaselineMessageCount = null;
      buf.remoteExpectedAssistantKey = "";
      if (!preserveCommittedRevision) buf.remoteCommittedRevision = "";
      buf.remoteTerminalSeen = false;
    }
    buf.remoteTurnActive = true;
    buf.busy = true;
    if (sid === state.activeSessionId) {
      state.busy = true;
      if (!state.thinking.active) startThinking();
    }
    if (!wasActive) {
      recordAuthoritySyncDiagnostic("remote_turn_marked", Object.assign({
        cause: String(cause || "unspecified"),
        preserve_committed_revision: !!preserveCommittedRevision,
      }, authoritySyncBufferSnapshot(sid, buf)));
    }
  }

  // web+tauriMain 共享
  function onSessionEvent(e, fn) {
    const sid = (e && e.payload && e.payload.session_id) || state.activeSessionId;
    if (sid) {
      const eventBuffer = getBuffer(sid);
      const eventName = String((e && e.event) || "");
      const isTurnEvent = /chat:(user_message|turn_started|delta|reasoning_start|reasoning_delta|reasoning_done|tool_start|tool_end|user_input_required|transient_error)$/.test(eventName);
      if (eventBuffer && !eventBuffer.localTurnOwned && (eventBuffer.busy || isTurnEvent)) {
        markRemoteTurn(sid, eventBuffer, false, "event:" + eventName);
      }
    }
    const isBg = sid && sid !== state.activeSessionId;
    runSyncOnSession(sid, fn);
    if (isBg) notify();
  }

  // web+tauriMain 共享
  function isScheduledRunSession(sid) {
    return !!sid && (
      sid.indexOf("sched-") === 0 ||
      !!scheduledRunSessionOwners.value[sid] ||
      !!(sessionStates.value[sid] && sessionStates.value[sid].scheduledRunSession) ||
      !!(state.scheduledRunContext && state.scheduledRunContext.sessionId === sid)
    );
  }

  // web+tauriMain 共享
  function defineSubscriptionStateProperty(target, key, value) {
    Object.defineProperty(target, key, {
      configurable: true,
      enumerable: true,
      value,
      writable: true,
    });
  }

  // web+tauriMain 共享
  function copySubscriptionStateObject(source) {
    const result = {};
    Object.keys(source).forEach(function (key) {
      defineSubscriptionStateProperty(result, key, source[key]);
    });
    return result;
  }

  // web+tauriScheduled 共享
  function loadScheduledTaskTemplateSources() {
    try {
      const parsed = JSON.parse(window.localStorage.getItem(SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY) || "{}");
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return Object.create(null);
      return Object.keys(parsed).reduce(function (result, taskId) {
        if (typeof parsed[taskId] === "string" && parsed[taskId].trim()) {
          result[taskId] = parsed[taskId].trim();
        }
        return result;
      }, Object.create(null));
    } catch {
      return Object.create(null);
    }
  }

  // web+tauriScheduled 共享
  function rememberScheduledTaskTemplateSource(taskId, templateId) {
    if (!taskId || !templateId) return;
    scheduledTaskTemplateSources.value[taskId] = templateId;
    persistScheduledTaskTemplateSources();
  }

  // web+tauriScheduled 共享
  function attachScheduledTaskTemplateSource(task) {
    if (!task || !task.id) return task;
    const templateId = task.templateId || scheduledTaskTemplateSources.value[task.id] || null;
    if (templateId) {
      task.templateId = templateId;
      if (scheduledTaskTemplateSources.value[task.id] !== templateId) {
        rememberScheduledTaskTemplateSource(task.id, templateId);
      }
    }
    return task;
  }

  // web+tauriScheduled 共享
  function attachAndPruneScheduledTaskTemplateSources(tasks) {
    const activeIds = Object.create(null);
    (tasks || []).forEach(function (task) {
      if (!task || !task.id) return;
      activeIds[task.id] = true;
      attachScheduledTaskTemplateSource(task);
    });
    let changed = false;
    Object.keys(scheduledTaskTemplateSources.value).forEach(function (taskId) {
      if (activeIds[taskId]) return;
      delete scheduledTaskTemplateSources.value[taskId];
      changed = true;
    });
    if (changed) persistScheduledTaskTemplateSources();
    return tasks;
  }

  // web+tauriScheduled 共享
  function upsertScheduledTask(task) {
    if (!task || !task.id) return;
    attachScheduledTaskTemplateSource(task);
    let found = false;
    state.scheduledTasks = (state.scheduledTasks || []).map(function (item) {
      if (item.id !== task.id) return item;
      found = true;
      return task;
    });
    if (!found) state.scheduledTasks = [task, ...(state.scheduledTasks || [])];
  }

  // web+tauriScheduled 共享
  function applyScheduledRunViewed(automationId, runId, receipt) {
    function markRunViewed(item) {
      const itemAutomationId = item.automationId || state.selectedScheduledTaskId;
      if (itemAutomationId !== automationId || item.id !== runId) return item;
      return Object.assign({}, item, { unread: false });
    }
    state.scheduledTaskRuns = (state.scheduledTaskRuns || []).map(markRunViewed);
    state.scheduledTaskRecentRuns = (state.scheduledTaskRecentRuns || []).map(markRunViewed);
    const hasUnreadRuns = receipt && typeof receipt.hasUnreadRuns === "boolean"
      ? receipt.hasUnreadRuns
      : (state.scheduledTaskRuns || []).some(function (item) {
          return (item.automationId || state.selectedScheduledTaskId) === automationId && !!item.unread;
        });
    state.scheduledTasks = (state.scheduledTasks || []).map(function (task) {
      return task.id === automationId
        ? Object.assign({}, task, { hasUnreadRuns })
        : task;
    });
    if (state.scheduledTaskDetail && state.scheduledTaskDetail.id === automationId) {
      state.scheduledTaskDetail = Object.assign({}, state.scheduledTaskDetail, {
        hasUnreadRuns,
      });
    }
  }

  // web+tauriScheduled 共享
  function invalidateScheduledTaskReads(automationId) {
    scheduledTaskRequestTokens.value.tasks += 1;
    if (state.selectedScheduledTaskId === automationId) {
      scheduledTaskRequestTokens.value.detail += 1;
      scheduledTaskRequestTokens.value.runs += 1;
    }
    scheduledTaskRefreshInFlight.value = null;
  }

  // web+tauriScheduled 共享
  function invalidateScheduledRecentRuns() {
    scheduledRecentRunsRequestToken.value += 1;
  }

  // web+tauriScheduled 共享
  // biome-ignore lint/suspicious/noFunctionAssign: forwarder-dep routing reassigns the shared impl when the lane passes its own (see FORWARDER_DEP_NAMES)
  function invalidateScheduledRecentRunsForSession(id) {
    if (String(id || "").indexOf("sched-") === 0) invalidateScheduledRecentRuns();
  }

  // web+tauriScheduled 共享
  function scheduleScheduledRunRefresh() {
    if (scheduledRunEventRefreshTimer.value) clearTimeout(scheduledRunEventRefreshTimer.value);
    scheduledRunEventRefreshTimer.value = setTimeout(function () {
      scheduledRunEventRefreshTimer.value = null;
      // Refresh task badges/detail first, then replace the global run list from
      // the same retained backend state. The aggregate request has its own stale
      // response guard, so a concurrent archive/delete cannot resurrect a row.
      Promise.resolve(refreshScheduledTaskData(20))
        .catch(function () {})
        .then(function () { return loadScheduledTaskRecentRuns(); })
        .catch(function () {});
    }, 400);
  }

  // web+tauriScheduled 共享
  function scheduledTaskErrorText(error) {
    return String(error && error.message ? error.message : error);
  }

  // web+tauriScheduled 共享
  // biome-ignore lint/suspicious/noFunctionAssign: forwarder-dep routing reassigns the shared impl when the lane passes its own (see FORWARDER_DEP_NAMES)
  function setScheduledTaskError(error, kind) {
    state.scheduledTaskError = error ? scheduledTaskErrorText(error) : null;
    state.scheduledTaskErrorKind = error ? (kind || "load") : null;
  }

  // web+tauriScheduled 共享
  function dismissScheduledTaskError() {
    setScheduledTaskError(null);
    notify();
  }

  // web+tauriScheduled 共享
  function clearScheduledTaskLoadError() {
    if (state.scheduledTaskErrorKind === "load") setScheduledTaskError(null);
  }

  // web+tauriScheduled 共享
  function beginScheduledTaskLoad(stamp) {
    const generation = stamp.generation;
    scheduledTaskPendingLoads.value[generation] = (scheduledTaskPendingLoads.value[generation] || 0) + 1;
    if (generation === scheduledTaskSelectionGeneration.value) {
      state.scheduledTaskLoading = true;
      clearScheduledTaskLoadError();
      notify();
    }
  }

  // web+tauriScheduled 共享
  function endScheduledTaskLoad(stamp) {
    const generation = stamp.generation;
    scheduledTaskPendingLoads.value[generation] = Math.max(0, (scheduledTaskPendingLoads.value[generation] || 0) - 1);
    if (!scheduledTaskPendingLoads.value[generation]) delete scheduledTaskPendingLoads.value[generation];
    if (generation === scheduledTaskSelectionGeneration.value) {
      state.scheduledTaskLoading = !!scheduledTaskPendingLoads.value[generation];
      notify();
    }
  }

  // web+tauriScheduled 共享
  function scheduledTaskRequestStamp(kind, id) {
    scheduledTaskRequestTokens.value[kind] += 1;
    return {
      kind,
      token: scheduledTaskRequestTokens.value[kind],
      generation: scheduledTaskSelectionGeneration.value,
      id: id || null,
    };
  }

  // web+tauriScheduled 共享
  function isCurrentScheduledTaskRequest(stamp) {
    if (!stamp || stamp.generation !== scheduledTaskSelectionGeneration.value) return false;
    if (scheduledTaskRequestTokens.value[stamp.kind] !== stamp.token) return false;
    // id 检查省略：selectedScheduledTaskId 唯一写者是 selectScheduledTask（每次
    // 改写前 generation+1），id 变化必然被上方 generation 检查拦截（审计清理）。
    return true;
  }

  // web+tauriScheduled 共享
// eslint-disable-next-line sonarjs/no-invariant-returns -- echoing back the normalized id is a deliberate API contract
  function selectScheduledTask(id) {
    const nextId = typeof id === "string" && id.trim() ? id.trim() : null;
    if (state.selectedScheduledTaskId === nextId) return nextId;
    scheduledTaskSelectionGeneration.value += 1;
    state.scheduledTaskSelectionGeneration = scheduledTaskSelectionGeneration.value;
    state.selectedScheduledTaskId = nextId;
    state.scheduledTaskDetail = null;
    state.scheduledTaskRuns = [];
    state.scheduledTaskLoading = !!scheduledTaskPendingLoads.value[scheduledTaskSelectionGeneration.value];
    setScheduledTaskError(null);
    notify();
    return nextId;
  }

  // web+tauriScheduled 共享
  function clearScheduledTaskSelection() {
    selectScheduledTask(null);
  }

  // web+tauriScheduled 共享
  function extractBalancedJsonObject(text) {
    const start = String(text || "").indexOf("{");
    if (start < 0) return null;
    let depth = 0;
    let inString = false;
    let escaping = false;
    for (let i = start; i < text.length; i++) {
      const ch = text.charAt(i);
      if (inString) {
        if (escaping) escaping = false;
        else if (ch === "\\") escaping = true;
        else if (ch === "\"") inString = false;
        continue;
      }
      if (ch === "\"") { inString = true; continue; }
      if (ch === "{") depth++;
      else if (ch === "}") {
        depth--;
        if (depth === 0) return text.slice(start, i + 1);
      }
    }
    return null;
  }

  // web+tauriScheduled 共享
  function normalizeScheduledTaskDraft(value) {
    if (!value || typeof value !== "object") return null;
    if (!value.name || !value.prompt || !value.rrule) return null;
    return {
      name: String(value.name),
      prompt: String(value.prompt),
      rrule: String(value.rrule),
      model: value.model ? String(value.model) : null,
      modelId: value.modelId ? String(value.modelId) : (value.model_id ? String(value.model_id) : null),
      mode: "yolo",
      paused: !!value.paused,
    };
  }

  // web+tauriScheduled 共享
  function activeScheduledTaskModelConfig() {
    return (state.savedModels || []).find(function (model) {
      return model && model.id === state.activeModelId;
    }) || null;
  }

  // web+tauriScheduled 共享
  function lockScheduledTaskDraftModel(draft) {
    if (!draft) return null;
    const active = activeScheduledTaskModelConfig();
    draft.model = draft.model || (active && active.model) || null;
    draft.modelId = draft.modelId || (active && active.id) || null;
    return draft;
  }

  // web+tauriScheduled 共享
  function scheduledTaskInputFromDraft(draft) {
    return {
      name: draft.name,
      prompt: draft.prompt,
      rrule: draft.rrule,
      model: draft.model || null,
      modelId: draft.modelId || null,
      mode: "yolo",
      paused: draft.paused,
    };
  }

  // web+tauriScheduled 共享
  async function loadScheduledTasks() {
    const stamp = scheduledTaskRequestStamp("tasks", null);
    beginScheduledTaskLoad(stamp);
    try {
      const tasks = await invoke("list_scheduled_tasks");
      if (!isCurrentScheduledTaskRequest(stamp)) return state.scheduledTasks;
      state.scheduledTasks = attachAndPruneScheduledTaskTemplateSources(
        Array.isArray(tasks) ? tasks : []
      );
      if (
        state.selectedScheduledTaskId &&
        (state.scheduledTasks || []).every(function (task) { return task.id !== state.selectedScheduledTaskId; })
      ) {
        selectScheduledTask(null);
      }
    } catch (e) {
      if (isCurrentScheduledTaskRequest(stamp)) setScheduledTaskError(e, "load");
    } finally {
      endScheduledTaskLoad(stamp);
    }
    return state.scheduledTasks;
  }

  // web+tauriScheduled 共享
  async function readScheduledTask(id) {
    if (!id) {
      clearScheduledTaskSelection();
      return null;
    }
    if (state.selectedScheduledTaskId !== id) selectScheduledTask(id);
    const stamp = scheduledTaskRequestStamp("detail", id);
    beginScheduledTaskLoad(stamp);
    try {
      const detail = await invoke("read_scheduled_task", { id });
      if (!isCurrentScheduledTaskRequest(stamp)) return state.scheduledTaskDetail;
      state.scheduledTaskDetail = attachScheduledTaskTemplateSource(detail) || null;
      upsertScheduledTask(detail);
    } catch (e) {
      if (isCurrentScheduledTaskRequest(stamp)) setScheduledTaskError(e, "load");
    } finally {
      endScheduledTaskLoad(stamp);
    }
    return state.scheduledTaskDetail;
  }

  // web+tauriScheduled 共享
  function mergeScheduledTaskRecentRuns(task, runs) {
    if (!task || !task.id) return state.scheduledTaskRecentRuns || [];
    invalidateScheduledRecentRuns();
    let rows = [...(state.scheduledTaskRecentRuns || [])];
    (Array.isArray(runs) ? runs : []).forEach(function (run) {
      if (!run) return;
      rememberScheduledRunOwner(run);
      const merged = Object.assign({}, run, {
        automationId: run.automationId || task.id,
        taskName: task.name || bt("scheduledTaskFallbackName"),
        taskModel: task.model || null,
      });
      const index = rows.findIndex(function (row) { return row && row.id === merged.id; });
      if (index >= 0) rows[index] = merged;
      else rows.push(merged);
    });
    rows = rows.filter(function (run) { return run && run.sessionId && !run.archived; });
    rows.sort(function (a, b) {
      return new Date(b.scheduledFor || b.createdAt || 0).getTime() -
        new Date(a.scheduledFor || a.createdAt || 0).getTime();
    });
    state.scheduledTaskRecentRuns = rows;
    return state.scheduledTaskRecentRuns;
  }

  // web+tauriScheduled 共享
  async function loadScheduledTaskRuns(id, limit) {
    if (!id) {
      clearScheduledTaskSelection();
      return [];
    }
    if (state.selectedScheduledTaskId !== id) selectScheduledTask(id);
    const stamp = scheduledTaskRequestStamp("runs", id);
    beginScheduledTaskLoad(stamp);
    try {
      const runs = await invoke("list_scheduled_task_runs", { id, limit });
      if (!isCurrentScheduledTaskRequest(stamp)) return state.scheduledTaskRuns;
      state.scheduledTaskRuns = Array.isArray(runs) ? runs : [];
      state.scheduledTaskRuns.forEach(rememberScheduledRunOwner);
      mergeScheduledTaskRecentRuns(
        (state.scheduledTasks || []).find(function (task) { return task && task.id === id; }),
        state.scheduledTaskRuns
      );
    } catch (e) {
      if (isCurrentScheduledTaskRequest(stamp)) setScheduledTaskError(e, "load");
    } finally {
      endScheduledTaskLoad(stamp);
    }
    return state.scheduledTaskRuns;
  }

  // web+tauriScheduled 共享
  // biome-ignore lint/suspicious/noFunctionAssign: forwarder-dep routing reassigns the shared impl when the lane passes its own (see FORWARDER_DEP_NAMES)
  async function loadScheduledTaskRecentRuns() {
    const requestToken = ++scheduledRecentRunsRequestToken.value;
    try {
      const tasks = state.scheduledTasks && state.scheduledTasks.length
        ? state.scheduledTasks
        : await loadScheduledTasks();
      if (requestToken !== scheduledRecentRunsRequestToken.value) {
        return state.scheduledTaskRecentRuns || [];
      }
      const runs = await invoke("list_scheduled_runs");
      if (requestToken !== scheduledRecentRunsRequestToken.value) {
        return state.scheduledTaskRecentRuns || [];
      }
      const tasksById = Object.create(null);
      (tasks || []).forEach(function (task) {
        if (task && task.id) tasksById[task.id] = task;
      });
      const rows = (Array.isArray(runs) ? runs : []).map(function (run) {
        if (!run) return null;
        rememberScheduledRunOwner(run);
        const automationId = run.automationId || run.automation_id;
        const task = tasksById[automationId] || null;
        return Object.assign({}, run, {
          automationId,
          taskName: task && task.name || run.taskName || bt("scheduledTaskFallbackName"),
          taskModel: task && task.model || run.taskModel || null,
        });
      }).filter(function (run) {
        return run && run.sessionId && !run.archived;
      });
      rows.sort(function (a, b) {
        return new Date(b.scheduledFor || b.createdAt || 0).getTime() -
          new Date(a.scheduledFor || a.createdAt || 0).getTime();
      });
      state.scheduledTaskRecentRuns = rows;
      notify();
      return state.scheduledTaskRecentRuns;
    } catch (e) {
      if (requestToken !== scheduledRecentRunsRequestToken.value) {
        return state.scheduledTaskRecentRuns || [];
      }
      console.warn("loadScheduledTaskRecentRuns failed", e);
      state.scheduledTaskRecentRuns = state.scheduledTaskRecentRuns || [];
      notify();
      return state.scheduledTaskRecentRuns;
    }
  }

  // web+tauriScheduled 共享
  function refreshScheduledTaskData(limit) {
    const generation = scheduledTaskSelectionGeneration.value;
    if (scheduledTaskRefreshInFlight.value && scheduledTaskRefreshInFlight.value.generation === generation) {
      return scheduledTaskRefreshInFlight.value.promise;
    }
    const selectedId = state.selectedScheduledTaskId;
    const requests = [loadScheduledTasks()];
    if (selectedId) {
      requests.push(readScheduledTask(selectedId));
      requests.push(loadScheduledTaskRuns(selectedId, limit || 20));
    }
    const promise = Promise.all(requests).finally(function () {
      if (scheduledTaskRefreshInFlight.value && scheduledTaskRefreshInFlight.value.promise === promise) {
        scheduledTaskRefreshInFlight.value = null;
      }
    });
    scheduledTaskRefreshInFlight.value = { generation, promise };
    return promise;
  }

  // web+tauriScheduled 共享
  function refreshScheduledRunShortcutUntilLinked(automationId, runId) {
    if (!automationId || !runId) return;
    const key = automationId + ":" + runId;
    if (scheduledRunShortcutRefreshes.value[key]) return;
    scheduledRunShortcutRefreshes.value[key] = true;
    const deadline = Date.now() + SCHEDULED_LINK_POLL_DEADLINE_MS.value;

    function stop() {
      delete scheduledRunShortcutRefreshes.value[key];
    }
    function again(attempt) {
      if (Date.now() >= deadline) {
        stop();
        return;
      }
      setTimeout(function () { poll(attempt + 1); }, attempt < SCHEDULED_LINK_POLL_FAST_ATTEMPTS.value
        ? SCHEDULED_LINK_POLL_FAST_MS.value
        : SCHEDULED_LINK_POLL_SLOW_MS.value);
    }

    function taskStillListed() {
      return (state.scheduledTasks || []).some(function (item) {
        return item && item.id === automationId;
      });
    }

    function poll(attempt) {
      invoke("list_scheduled_task_runs", { id: automationId }).then(function (runs) {
        // 任务已被删除时不再回填：陈旧轮询响应会把已删任务以 fallback 名
        // 复活回侧边栏（审计 R1）。任务不在列表即收工，不 merge、不续排。
        const task = (state.scheduledTasks || []).find(function (item) {
          return item && item.id === automationId;
        });
        if (!task) {
          stop();
          return;
        }
        mergeScheduledTaskRecentRuns(task, runs);
        notify();
        // 必须看原始响应:mergeScheduledTaskRecentRuns 会滤掉尚无 sessionId 的记录,
        // 从合并结果里读不到目标 run 的状态。
        const target = (Array.isArray(runs) ? runs : []).find(function (run) {
          return run && run.id === runId;
        });
        // 会话已挂上 → 记录已进侧边栏;run 已终态却仍无会话 → 会话没建起来,再等也不会有;
        // run 记录消失(被删或被 retention 清掉)→ 没有等待对象。三种情况都收工。
        if (!target || target.sessionId || isScheduledRunTerminal(target.status)) {
          stop();
          return;
        }
        again(attempt);
      }).catch(function () {
        // 已删任务的后端响应是 Err 而非空列表（get_automation 文件已移除）。
        // 任务不在列表即收工，否则会以 1s/5s 空转重试到 30 分钟兜底。
        if (!taskStillListed()) {
          stop();
          return;
        }
        again(attempt);
      });
    }

    poll(0);
  }

  // web+tauriScheduled 共享
  function upsertScheduledTaskRun(run) {
    if (!run || !run.id) return;
    rememberScheduledRunOwner(run);
    if (state.selectedScheduledTaskId && run.automationId && state.selectedScheduledTaskId !== run.automationId) return;
    let found = false;
    state.scheduledTaskRuns = (state.scheduledTaskRuns || []).map(function (item) {
      if (item.id === run.id) {
        found = true;
        return run;
      }
      return item;
    });
    if (!found) state.scheduledTaskRuns = [run, ...(state.scheduledTaskRuns || [])];
  }

  // web+tauriScheduled 共享
  async function runScheduledTaskAction(action, operation) {
    if (state.scheduledTaskBusyAction) {
      throw new Error(bt("scheduledActionBusy"));
    }
    state.scheduledTaskBusyAction = action;
    setScheduledTaskError(null);
    notify();
    try {
      return await operation();
    } catch (e) {
      setScheduledTaskError(e, "action");
      throw e;
    } finally {
      state.scheduledTaskBusyAction = null;
      notify();
    }
  }

  // web+tauriScheduled 共享
  async function updateScheduledTask(id, input) {
    return runScheduledTaskAction("update", async function () {
      const backendInput = scheduledTaskBackendInput(input);
      const updated = await invoke("update_scheduled_task", { id, input: backendInput });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  // web+tauriScheduled 共享
  async function pauseScheduledTask(id) {
    return runScheduledTaskAction("pause", async function () {
      const updated = await invoke("pause_scheduled_task", { id });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  // web+tauriScheduled 共享
  async function resumeScheduledTask(id) {
    return runScheduledTaskAction("resume", async function () {
      const updated = await invoke("resume_scheduled_task", { id });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  // web+tauriScheduled 共享
  async function toggleScheduledTaskPinned(id, pinned) {
    return runScheduledTaskAction(pinned ? "pin" : "unpin", async function () {
      const updated = await invoke("set_scheduled_task_pinned", { id, pinned: !!pinned });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  // web+tauriScheduled 共享
  async function deleteScheduledTask(id) {
    return runScheduledTaskAction("delete", async function () {
      invalidateScheduledRecentRuns();
      const deleted = await invoke("delete_scheduled_task", { id });
      // 作废删除前在途的整表 list / detail / runs 读（与 run-now 同模式）：否则
      // 3 秒轮询的旧 list 响应落地时会把刚删的任务复活回侧边栏（含本 feature
      // run-now 轮询依赖的 taskStillListed 判断，幽灵窗口会击穿 R1 守卫）。
      invalidateScheduledTaskReads(id);
      forgetScheduledTaskTemplateSource(id);
      state.scheduledTasks = (state.scheduledTasks || []).filter(function (task) { return task.id !== id; });
      if (state.selectedScheduledTaskId === id) selectScheduledTask(null);
      notify();
      return deleted;
    });
  }

  // web+tauriScheduled 共享
  async function runScheduledTaskNow(id) {
    return runScheduledTaskAction("run-now", async function () {
      const run = await invoke("run_scheduled_task_now", { id });
      invalidateScheduledTaskReads(id);
      upsertScheduledTaskRun(run);
      const runStatus = String(run && run.status || "").toLowerCase();
      if (runStatus === "queued" || runStatus === "running") {
        state.scheduledTasks = (state.scheduledTasks || []).map(function (task) {
          return task.id === id ? Object.assign({}, task, { isRunning: true }) : task;
        });
        if (state.scheduledTaskDetail && state.scheduledTaskDetail.id === id) {
          state.scheduledTaskDetail = Object.assign({}, state.scheduledTaskDetail, { isRunning: true });
        }
      }
      notify();
      refreshScheduledRunShortcutUntilLinked(id, run && run.id);
      return run;
    });
  }

  // web+tauriScheduled 共享
  async function startScheduledTaskChat() {
    return runScheduledTaskAction("chat-create", async function () {
      const prompt = await invoke("scheduled_task_chat_prompt");
      state.scheduledTaskCreationSessionId = null;
      scheduledTaskAutoCreateSeq.value++; // 清空意图：作废在途 auto-create 的陈旧 completion（审计 f）
      state.scheduledTaskAutoOpenId = null;
      await createNewSession();
      state.scheduledTaskPendingGuide = prompt;
      prefillComposer(bt("scheduledChatPrefill"));
      notify();
      return prompt;
    });
  }

  // web+tauriChat 共享
  function toolCallAlreadyFinished(toolCallId) {
    return messageHasToolBlock("tool_result", toolCallId);
  }

  // web+tauriChat 共享
  function hasChatItemForTool(type, toolCallId) {
    return !!toolCallId && state.chatItems.some(function (item) {
      return item && item.type === type && item.toolCallId === toolCallId;
    });
  }

  // web+tauriChat 共享
  // biome-ignore lint/suspicious/noFunctionAssign: forwarder-dep routing reassigns the shared impl when the lane passes its own (see FORWARDER_DEP_NAMES)
  function addSystemItem(text, meta) {
    const item = { type: "system", text, time: timeStr() };
    if (meta) {
      for (const k in meta) item[k] = meta[k];
    }
    addChatItem(item);
    notify();
  }

  // web+tauriChat 共享
  function addAuthoritySyncNotice(text) {
    if (state.chatItems.some(function (item) {
      return item && item.authoritySyncNotice;
    })) return;
    addSystemItem(text, { authoritySyncNotice: true });
  }

  // web+tauriChat 共享
  function compactPruneRollupText(count) {
    return bt("compactDone") + bt("compactAuto") + " " +
      bt("compactPruneMerged") + " ×" + count;
  }

  // web+tauriChat 共享
  function removeCompactionStartItem(compactId) {
    if (!compactId) return;
    for (let i = state.chatItems.length - 1; i >= 0; i--) {
      const it = state.chatItems[i];
      if (it.type === "system" && it.compactId === compactId && it.compactPhase === "start") {
        state.chatItems.splice(i, 1);
        return;
      }
    }
  }

  // web+tauriChat 共享
  function addOrMergePruneCompaction(compactId) {
    removeCompactionStartItem(compactId);
    const last = state.chatItems[state.chatItems.length - 1];
    if (last && last.type === "system" && last.compactPruneRollup) {
      last.compactPruneCount = (last.compactPruneCount || 1) + 1;
      last.text = compactPruneRollupText(last.compactPruneCount);
      last.time = timeStr();
      notify();
      return;
    }
    addChatItem({
      type: "system",
      text: compactPruneRollupText(1),
      time: timeStr(),
      compactPruneRollup: true,
      compactPruneCount: 1,
    });
    notify();
  }

  // web+tauriChat 共享
  // biome-ignore lint/suspicious/noFunctionAssign: forwarder-dep routing reassigns the shared impl when the lane passes its own (see FORWARDER_DEP_NAMES)
  function timeStr() {
    return new Date().toTimeString().slice(0, 5);
  }

  // web+tauriSessions 共享
  // biome-ignore lint/suspicious/noFunctionAssign: forwarder-dep routing reassigns the shared impl when the lane passes its own (see FORWARDER_DEP_NAMES)
  async function createNewSession() { enterDraft(); }

  // web+tauriSessions 共享
  function reportSessionSwitchFailure(error, errorScope) {
    if (errorScope === "scheduled") {
      setScheduledTaskError(error, "navigation");
      notify();
      return;
    }
    addSystemItem(bt("loadChatFailed") + error);
  }

  // web+tauriSessions 共享
  function mergeHydratedMessages(durableMessages, liveMessages, hideInternalEnvelope) {
    const durable = Array.isArray(durableMessages) ? [...durableMessages] : [];
    const counts = Object.create(null);
    durable.forEach(function (message) {
      const key = hydratedMessageKey(message, hideInternalEnvelope);
      counts[key] = (counts[key] || 0) + 1;
    });
    (Array.isArray(liveMessages) ? liveMessages : []).forEach(function (message) {
      const key = hydratedMessageKey(message, hideInternalEnvelope);
      if (counts[key]) {
        counts[key] -= 1;
      } else {
        durable.push(message);
      }
    });
    return durable;
  }

  // web+tauriSessions 共享
  function hydratedChatItemKey(item) {
    if (!item || !item.type) return "";
    if (item.type === "assistant") return "assistant:" + String(item.html || item.text || "");
    if (item.type === "reasoning") return "reasoning:" + String(item.text || "");
    if (item.type === "tool" && item.toolId) return "tool:" + item.toolId;
    if (item.type === "artifact_card") return "artifact:" + basename(item.path);
    if (item.type === "user_input" && item.toolCallId) return "user_input:" + item.toolCallId;
    if (item.type === "careful_blocked" && item.toolCallId) return "careful_blocked:" + item.toolCallId;
    if (item.type === "plan_card" && item.planId) return "plan:" + item.planId;
    if (item.type === "user") return "user:" + String(item.text || item.html || "");
    if (item.type === "system") return "system:" + String(item.text || "");
    const stable = Object.assign({}, item);
    delete stable.id;
    delete stable.time;
    delete stable.streaming;
    try { return item.type + ":" + JSON.stringify(stable); } catch { return item.type + ":" + String(stable); }
  }

  // web+tauriSessions 共享
  async function switchToSession(id) {
    return switchToSessionInternal(id, false, "chat");
  }

  // web+tauriSessions 共享
  function openScheduledRunChat(run, task) {
    const sessionId = run && typeof run.sessionId === "string" ? run.sessionId.trim() : "";
    if (!sessionId) return openScheduledRunChatOnce(run, task);
    if (scheduledRunOpenInFlight.value[sessionId]) return scheduledRunOpenInFlight.value[sessionId];
    const opening = openScheduledRunChatOnce(run, task);
    scheduledRunOpenInFlight.value[sessionId] = opening;
    function clearOpening() {
      if (scheduledRunOpenInFlight.value[sessionId] === opening) {
        delete scheduledRunOpenInFlight.value[sessionId];
      }
    }
    opening.then(clearOpening, clearOpening);
    return opening;
  }

  // web+tauriSessions 共享
  async function exitScheduledRunChat() {
    const context = state.scheduledRunContext;
    if (!context) return false;
    if (context.returnSessionId && context.returnSessionId !== context.sessionId) {
      const restored = await switchToSessionInternal(context.returnSessionId, true, "scheduled");
      if (restored) {
        state.scheduledRunContext = null;
        notify();
        return true;
      }
      return false;
    }
    enterDraft();
    return true;
  }

  // web+tauriSessions 共享
  function recentScheduledRunForSession(id) {
    return (state.scheduledTaskRecentRuns || []).find(function (run) {
      return run && run.sessionId === id;
    }) || null;
  }

  // web+tauriSessions 共享
  function leaveSessionView(id) {
    if (state.scheduledRunContext && state.scheduledRunContext.sessionId === id) {
      state.scheduledRunContext = null;
    }
    if (state.activeSessionId !== id) return;
    state.activeSessionId = null;
    loadWorkingSetFrom(freshBuffer());
  }

  // web+tauriSessions 共享
  function applyDeletedSession(id) {
    if (typeof id !== "string" || !id) return false;
    invalidateScheduledRecentRunsForSession(id);
    purgeSessionBuffer(id);
    state.sessions = state.sessions.filter(function (session) { return session.id !== id; });
    state.archivedSessions = (state.archivedSessions || []).filter(function (session) {
      return session.id !== id;
    });
    state.scheduledTaskRecentRuns = (state.scheduledTaskRecentRuns || []).filter(function (run) {
      return !run || run.sessionId !== id;
    });
    state.scheduledTaskRuns = (state.scheduledTaskRuns || []).filter(function (run) {
      return !run || run.sessionId !== id;
    });
    notify();
    return true;
  }

  // web+tauriSessions 共享
  async function renameSession(id, title) {
    invalidateScheduledRecentRunsForSession(id);
    try {
      await invoke("rename_session", { id, title });
      const s = state.sessions.find(function (s) { return s.id === id; });
      if (s) s.title = title;
      state.scheduledTaskRecentRuns = (state.scheduledTaskRecentRuns || []).map(function (run) {
        return run && run.sessionId === id ? Object.assign({}, run, { sessionTitle: title }) : run;
      });
      delete personaPlaceholderTitles.value[id]; // 用户主动命名后不再算卡牌占位,不被对话覆盖
      notify();
    } catch (e) {
      console.warn("rename failed", e);
    }
  }

  // web+tauriSessions 共享
  async function toggleSessionPinned(id, pinned) {
    invalidateScheduledRecentRunsForSession(id);
    const s = state.sessions.find(function (s) { return s.id === id; });
    const scheduledRun = recentScheduledRunForSession(id);
    const prev = s ? !!s.pinned : false;
    const prevPinnedAt = s ? s.pinned_at : null;
    const previousRunPinned = scheduledRun ? !!scheduledRun.pinned : false;
    const previousRunPinnedAt = scheduledRun ? scheduledRun.pinnedAt : null;
    if (s) {
      s.pinned = !!pinned;
      s.pinned_at = pinned ? new Date().toISOString() : null;
    }
    if (scheduledRun) {
      scheduledRun.pinned = !!pinned;
      scheduledRun.pinnedAt = pinned ? new Date().toISOString() : null;
    }
    notify();
    try {
      await invoke("set_session_pinned", { id, pinned: !!pinned });
      await refreshHistoryList();
    } catch (e) {
      if (s) {
        s.pinned = prev;
        s.pinned_at = prevPinnedAt;
      }
      if (scheduledRun) {
        scheduledRun.pinned = previousRunPinned;
        scheduledRun.pinnedAt = previousRunPinnedAt;
      }
      console.warn("set_session_pinned failed", e);
      await refreshHistoryList();
    }
  }

  // web+tauriSessions 共享
  async function archiveSession(id) {
    invalidateScheduledRecentRunsForSession(id);
    const idx = state.sessions.findIndex(function (s) { return s.id === id; });
    if (idx < 0) {
      // 定时运行会话不在 state.sessions;收起 = 从侧边栏记录移除,进设置页归档列表。
      const scheduledRun = recentScheduledRunForSession(id);
      // Codex 等独立会话也不在 state.sessions；交给后端判定并刷新统一历史列表。
      if (!scheduledRun) {
        try {
          await invoke("set_session_archived", { id, archived: true });
          await refreshHistoryList();
          return true;
        } catch (e) {
          console.warn("set_session_archived failed", e);
          return false;
        }
      }
      const previousRuns = state.scheduledTaskRecentRuns || [];
      const wasViewingRun = state.activeSessionId === id;
      const previousContext = state.scheduledRunContext;
      // 归档等待期间的导航 token：失败回滚时「activeSessionId === null」不足以
      // 证明无新导航——用户再进草稿也保持 null（enterDraft 只推进 token），
      // 仅 token 未前移才允许把 active 拽回归档会话（三审 P1）。
      const navToken = sessionSwitchRequestToken.value;
      // 与普通会话收纳同语义:保留 buffer(还能从设置页还原后重开),但要离开当前视图。
      if (wasViewingRun) saveWorkingSetTo(getBuffer(id));
      state.scheduledTaskRecentRuns = previousRuns.filter(function (run) {
        return !run || run.sessionId !== id;
      });
      leaveSessionView(id);
      notify();
      try {
        await invoke("set_session_archived", { id, archived: true });
        await refreshHistoryList();
        return true;
      } catch (e) {
        state.scheduledTaskRecentRuns = previousRuns;
        // 回滚 active 仅当用户没有新导航（leaveSessionView 已置 null）：
        // await 期间切到别的会话/再进草稿都不得劫持 active（审计、三审 P1）。
        if (wasViewingRun && state.activeSessionId === null
            && navToken === sessionSwitchRequestToken.value) {
          // active 与 scheduledRunContext 必须成对回滚,否则会落到
          // 「active 有值但 context 空」的错位态(界面回任务列表却仍持有会话)。
          state.activeSessionId = id;
          state.scheduledRunContext = previousContext;
          loadWorkingSetFrom(getBuffer(id));
        }
        console.warn("set_session_archived failed", e);
        notify();
        return false;
      }
    }
    const s = state.sessions[idx];
    const archived = Object.assign({}, s, { archived: true, archived_at: new Date().toISOString(), pinned: false, pinned_at: null });
    const wasActive = state.activeSessionId === id;
    // 与 scheduled 分支同源：失败回滚须以导航 token 证明「无新导航」——
    // 归档等待期间再进草稿 activeSessionId 仍为 null（三审 P1）。
    const navToken = sessionSwitchRequestToken.value;
    if (wasActive) saveWorkingSetTo(getBuffer(id));
    state.sessions.splice(idx, 1);
    state.archivedSessions = [archived, ...(state.archivedSessions || []).filter(function (x) { return x.id !== id; })];
    leaveSessionView(id);
    notify();
    try {
      await invoke("set_session_archived", { id, archived: true });
      await refreshHistoryList();
      return true;
    } catch (e) {
      state.sessions.splice(idx, 0, s);
      state.archivedSessions = (state.archivedSessions || []).filter(function (x) { return x.id !== id; });
      // 回滚 active 仅当用户没有新导航（leaveSessionView 已置 null）：
      // await 期间切到别的会话/再进草稿都不得劫持 active（审计、三审 P1）。
      if (wasActive && state.activeSessionId === null
          && navToken === sessionSwitchRequestToken.value) {
        state.activeSessionId = id;
        loadWorkingSetFrom(getBuffer(id));
      }
      console.warn("set_session_archived failed", e);
      notify();
      return false;
    }
  }

  // web+tauriSessions 共享
  async function restoreArchivedSession(id) {
    const idx = (state.archivedSessions || []).findIndex(function (s) { return s.id === id; });
    if (idx < 0) return false;
    const s = state.archivedSessions[idx];
    invalidateScheduledRecentRunsForSession(id);
    const restored = Object.assign({}, s, { archived: false, archived_at: null });
    state.archivedSessions.splice(idx, 1);
    state.sessions = [restored, ...(state.sessions || [])];
    notify();
    try {
      await invoke("set_session_archived", { id, archived: false });
      await refreshHistoryList();
      // 还原的定时运行会话回侧边栏"定时任务记录"(refreshHistoryList 只管普通会话)。
      if (String(id).indexOf("sched-") === 0) loadScheduledTaskRecentRuns().catch(function () {});
      return true;
    } catch (e) {
      state.archivedSessions.splice(idx, 0, s);
      state.sessions = (state.sessions || []).filter(function (x) { return x.id !== id; });
      console.warn("restore archived session failed", e);
      notify();
      return false;
    }
  }

  // web+tauriMain 共享
  function toolResultText(content) {
    if (typeof content === "string") return content;
    if (Array.isArray(content)) {
      return content.map(function (b) { return b && typeof b.text === "string" ? b.text : ""; }).join("");
    }
    return "";
  }

  // web+tauriMain 共享
  function stripInternalToolRuntimeSuffix(value) {
    let text = String(value == null ? "" : value);
    const marker = "\n\n<codewhale:runtime_event";
    while (true) {
      const start = text.lastIndexOf(marker);
      if (start < 0) return text;
      const suffix = text.slice(start + 2);
      const opening = suffix.match(/^<codewhale:runtime_event\b[^>]*>/i);
      if (!opening || !/<\/codewhale:runtime_event>\s*$/i.test(suffix)) return text;
      const tag = opening[0];
      const knownKind = /\bkind=(["'])(?:stuck_guard|tool_error_degradation)\1/i.test(tag);
      const internal = /\bvisibility=(["'])internal\1/i.test(tag);
      if (!knownKind || !internal) return text;
      text = text.slice(0, start);
    }
  }

  // web+tauriMain 共享
  function toolResultDisplayContent(content) {
    if (typeof content === "string") return stripInternalToolRuntimeSuffix(content);
    if (!Array.isArray(content)) return content;
    return content.map(function (block) {
      if (!block || typeof block.text !== "string") return block;
      return Object.assign({}, block, { text: stripInternalToolRuntimeSuffix(block.text) });
    });
  }

  // web+tauriMain 共享
  function parsePlanSnapshot(content) {
    const txt = toolResultText(content);
    const i = txt.indexOf("\n");
    if (i < 0) return null;
    try { return JSON.parse(txt.slice(i + 1)); } catch { return null; }
  }

  // web+tauriMain 共享
  function parseUserAnswers(content, questions) {
    let ans;
    try { ans = JSON.parse(toolResultText(content)).answers; } catch { return null; }
    if (!Array.isArray(ans)) return null;
    // 用无原型对象：question id 仅后端校验非空，constructor/toString/__proto__ 是合法输入，
    // 普通 {} 会让这些键命中 Object.prototype 继承属性，.push 抛 TypeError（复核 P1）。
    const byId = Object.create(null);
    ans.forEach(function (a) {
      if (a && a.id != null) {
        byId[a.id] = byId[a.id] || [];
        byId[a.id].push(a);
      }
    });
    const out = [];
    for (let qi = 0; qi < questions.length; qi++) {
      const q = questions[qi];
      const matches = byId[q.id];
      if (!matches || !matches.length) { out.push(null); continue; }
      matches.forEach(function (a) { out.push({ id: q.id, label: a.label, value: a.value }); });
    }
    return out;
  }

  // web+tauriMain 共享
  function parseCarefulBlocked(text) {
    if (typeof text !== "string" || text.indexOf("BLOCKED: This command was blocked for safety reasons") !== 0) return null;
    const rm = text.match(/Reasons: ([^\n]*)/);
    const sm = text.match(/Suggestions: ([^\n]*)/);
    return {
      safety_level: "dangerous", blocked: true,
      reasons: rm && rm[1] ? rm[1].split("; ") : [],
      suggestions: sm && sm[1] ? sm[1].split("; ") : [],
    };
  }

  // web+tauriMain 共享
  function userMessageInputProvenance(blocks) {
    return window.PinvouBridgeMessages.userMessageInputProvenance(blocks);
  }

  // web+tauriMain 共享
  function isInternalUserMessageProvenance(provenance) {
    return window.PinvouBridgeMessages.isInternalUserMessageProvenance(provenance);
  }

  // web+tauriTerminal 共享
  function isShellExecutionTool(name) {
    return SHELL_TOOL_NAMES.value.includes(name);
  }

  // web+tauriTerminal 共享
  function utf8Length(text) {
    try { return new TextEncoder().encode(String(text || "")).length; }
    catch { return String(text || "").length; }
  }

  // web+tauriTerminal 共享
  function formatShellSnapshot(job) {
    function section(raw, total, kind) {
      raw = String(raw || "");
      const visibleRaw = raw.replace(/^\.\.\.\s*/, "");
      const omitted = /^\.\.\./.test(raw) || Number(total || 0) > utf8Length(visibleRaw);
      let body = normalizeTerminalTail(visibleRaw);
      if (omitted) body = bt("shellOutputOmitted")(kind) + "\n" + body;
      return body;
    }
    const stdout = section(job.stdout_tail, job.stdout_len, "stdout");
    const stderr = section(job.stderr_tail, job.stderr_len, "stderr");
    const parts = [];
    if (stdout) parts.push(stdout);
    if (stderr) parts.push((stdout ? "[STDERR]\n" : "") + stderr);
    if (String(job.status || "").toLowerCase() !== "running") {
      const code = job.exit_code == null ? bt("shellUnknownExit") : String(job.exit_code);
      parts.push(bt("shellTaskFinished")(code));
    }
    return parts.join("\n");
  }

  // web+tauriTerminal 共享
  function shellCommandForItem(item) {
    return item && item.args && typeof item.args.command === "string" ? item.args.command : "";
  }

  // web+tauriTerminal 共享
  function shellSnapshotKey(job) {
    return JSON.stringify([
      job.id, job.status, job.exit_code, job.stdout_len, job.stderr_len,
      job.stdout_tail, job.stderr_tail,
    ]);
  }

  // web+tauriTerminal 共享
  function terminalShellHistoryMatch(item, job) {
    if (!item || item.type !== "tool" || item.taskId || item.state === "running" ||
        !isShellExecutionTool(item.name) || shellCommandForItem(item) !== String(job.command || "")) {
      return false;
    }
    const output = normalizeTerminalTail(String(item.output || ""));
    if (output.includes(String(job.id || "")) && job.id) return true;
    const evidence = [job.stdout_tail, job.stderr_tail].map(function (raw) {
      return normalizeTerminalTail(String(raw || "").replace(/^\.\.\.\s*/, "")).trim();
    }).filter(Boolean);
    if (evidence.length) return evidence.every(function (text) { return output.includes(text); });
    return /\(no output\)|no output|无输出|出力なし/i.test(output);
  }

  // web+tauriTerminal 共享
  function applyShellSnapshots(sid, jobs) {
    let anyRunning = false;
    let changed = false;
    const runningCommandCounts = {};
    (jobs || []).forEach(function (job) {
      if (String(job.status || "").toLowerCase() !== "running") return;
      const command = String(job.command || "");
      runningCommandCounts[command] = (runningCommandCounts[command] || 0) + 1;
    });
    runSyncOnSession(sid, function () {
      // A wait tool only observes existing work and cannot create a job, and
      // the manager retains completed jobs across later waits, so an
      // unmatched terminal snapshot beside a trailing wait card belongs to
      // earlier work and must not be appended after newer results. Decide
      // once per poll from the pre-poll timeline: the synthetic card of a
      // running job from this same batch (the manager lists running jobs
      // first) would otherwise disarm the guard for the jobs after it.
      // Accepted limits when no card binds: a start tool can still race with
      // a very short detached job whose first snapshot is terminal (the guard
      // is off when the latest card is a start tool; origin identity shields
      // root jobs there, but subagent-owned and legacy origin-less jobs can
      // still append), and a brand-new subagent job started after the wait
      // card is conservatively hidden like retained older work.
      const suppressUnmatchedTerminal = latestShellToolIsWaitObserver();
      (jobs || []).forEach(function (job) {
        const status = String(job.status || "").toLowerCase();
        const running = status === "running";
        if (running) anyRunning = true;
        let item = state.chatItems.find(function (it) {
          return it.type === "tool" && it.taskId === job.id;
        });
        if (!item && job.origin_tool_call_id) {
          // Never steal a card already bound to another job: origins are
          // unique per root job on the current engine, and if an engine ever
          // shares one, the later job must fall through to a synthetic card
          // or the terminal suppression guard instead of redirecting output.
          item = state.chatItems.find(function (it) {
            return it.type === "tool" && it.toolId === job.origin_tool_call_id &&
              (!it.taskId || it.taskId === job.id);
          });
        }
        // Only legacy snapshots without an origin may match by command or
        // output. A missing origin card must not redirect another tool call.
        if (!item && running && !job.origin_tool_call_id) {
          const command = String(job.command || "");
          const candidates = state.chatItems.filter(function (it) {
            return it.type === "tool" && isShellExecutionTool(it.name) && !it.taskId &&
              it.state === "running" && shellCommandForItem(it) === command;
          });
          // Command text is only a temporary bridge until tool_end exposes the
          // task id. Never guess when identical commands are concurrent.
          if (runningCommandCounts[command] === 1 && candidates.length === 1) item = candidates[0];
        }
        if (!item && !running && !job.origin_tool_call_id) {
          item = state.chatItems.find(function (it) {
            return terminalShellHistoryMatch(it, job);
          });
          if (item) item.shellHistoryReconciled = true;
        }
        if (!item && !running && suppressUnmatchedTerminal) return;
        // An identified completed root job must only update its origin card.
        // If compaction or reload removed that card, do not append historical
        // output at the current tail. Keep running jobs visible through a
        // synthetic card; their live status must not disappear after reload.
        if (!item && !running && job.origin_tool_call_id && !job.owner_agent_id) return;
        if (!item) {
          item = {
            type: "tool", toolId: "shell-task:" + job.id, name: "bash",
            args: { command: job.command || "" }, output: null, success: null,
            state: running ? "running" : "failed", shellSnapshot: true,
          };
          addChatItem(item);
          changed = true;
        }
        const snapshotKey = shellSnapshotKey(job);
        if (item.shellSnapshotKey === snapshotKey) return;
        item.taskId = job.id;
        item.sessionId = sid;
        item.shellStatus = job.status;
        item.originToolCallId = job.origin_tool_call_id || null;
        item.originTurnId = job.origin_turn_id || null;
        item.exitCode = job.exit_code;
        item.elapsedMs = job.elapsed_ms;
        if (!item.shellHistoryReconciled || item.output == null || running) {
          item.output = formatShellSnapshot(job);
        }
        item.state = running ? "running" : (status === "completed" ? "done" : "failed");
        item.success = running ? null : status === "completed";
        item.shellSnapshotKey = snapshotKey;
        changed = true;
      });
    });
    if (changed) notify();
    return anyRunning;
  }

  // web+tauriTerminal 共享
  function scheduleShellPoll(sid, immediate) {
    if (!sid) return;
    if (!shellPollState.value[sid]) shellPollState.value[sid] = {
      timer: null, inFlight: false, waitBudget: 0,
    };
    const poll = shellPollState.value[sid];
    poll.waitBudget = Math.max(poll.waitBudget, 12);
    if (poll.timer || poll.inFlight) return;
    poll.timer = setTimeout(function () { runShellPoll(sid); }, immediate ? 0 : 250);
  }

  // web+tauriTerminal 共享
  async function runShellPoll(sid) {
    const poll = shellPollState.value[sid];
    if (!poll || poll.inFlight) return;
    poll.timer = null;
    poll.inFlight = true;
    let running = false;
    try {
      const jobs = await invoke("list_shell_tasks", { sessionId: sid });
      running = applyShellSnapshots(sid, Array.isArray(jobs) ? jobs : []);
      if (!running) poll.waitBudget = Math.max(0, poll.waitBudget - 1);
    } catch (error) {
      console.warn("shell task polling failed", error);
      poll.waitBudget = Math.max(0, poll.waitBudget - 1);
    } finally {
      poll.inFlight = false;
    }
    if (running || poll.waitBudget > 0) {
      poll.timer = setTimeout(function () { runShellPoll(sid); }, 250);
    } else {
      delete shellPollState.value[sid];
    }
  }

  // web+tauriMain 共享
  function patchLastItem(pred, patch) {
    for (let i = state.chatItems.length - 1; i >= 0; i--) {
      if (pred(state.chatItems[i])) {
        Object.assign(state.chatItems[i], patch);
        return state.chatItems[i];
      }
    }
    return null;
  }

  // web+tauriMain 共享
  function hasUnresolvedItem(type) {
    return state.chatItems.some(function (it) { return it.type === type && !it.resolved; });
  }

  // web+tauriArtifactTracker 共享
  // biome-ignore lint/suspicious/noFunctionAssign: forwarder-dep routing reassigns the shared impl when the lane passes its own (see FORWARDER_DEP_NAMES)
  function basename(p) {
    if (!p) return "";
    const parts = String(p).split(/[\\/]/);
    return parts[parts.length - 1] || p;
  }

  // web+tauriArtifactTracker 共享
  function isAbsPath(p) {
    return typeof p === "string" && (p.charAt(0) === "/" || /^[A-Za-z]:[\\/]/.test(p));
  }

  // web+tauriArtifactTracker 共享
  function normalizedPath(p) {
    return String(p || "").replaceAll('\\', "/");
  }

  // Shared by web + tauriSessions (review #463 round-13: the block used to be
  // byte-duplicated between the two session:list_changed listeners, and the
  // round-D chain fix had to land twice). Stamps the workspace_rebound mark
  // the rebind command emits for the sessions whose persisted artifact paths
  // its lanes rebased (rebound, failed AND post-busy ids — review #463
  // round-10 Major 2 + round-B Major 1). The mark carries the rebind geometry
  // as an ordered SEGMENT CHAIN: the artifact reconcile's stale-absolute
  // rebase arm is gated on it (view healing, freshness window), and the
  // wholesale artifact saves rebase along the chain while the mark exists — a
  // chat turn's buffer save must not durably revert the backend rebase.
  // Chained rebinds APPEND a segment (A→B then B→C): the transform resolves
  // in order, so an A-era path maps A→B→C while a buffer re-vintaged from the
  // durable JSON between the two rebinds (B-era) still maps B→C — a composed
  // single segment {A→C} would strand the B-era vintage (review #463 round-D
  // Major 1). Marks are memory-only and never pruned: the save transform is
  // prefix-exact and must outlive the reconcile window for the whole process
  // lifetime (a restart starts from the already-rebased JSON with no marks).
  // Consumed by rebaseArtifactPathsForRebind / sessionRecentlyRebound below.
  function applyWorkspaceReboundMark(payload) {
    if (!payload || payload.action !== "workspace_rebound" || !payload.id || !payload.from || !payload.to) {
      return;
    }
    state.reboundSessionIds = state.reboundSessionIds || {};
    const existing = state.reboundSessionIds[payload.id];
    const last = existing && existing.chain && existing.chain[existing.chain.length - 1];
    if (existing && last && last.from === payload.from && last.to === payload.to) {
      // Identical retry of the last segment: refresh the view-heal window
      // only (an older vintage may still be buffered; the chain must
      // survive).
      existing.at = Date.now();
    } else if (existing) {
      // Chained (last.to === payload.from) or non-contiguous: append. A
      // non-matching segment is inert for the ordered prefix transform, and
      // appending keeps every older vintage resolvable (review #463 round-E
      // minor — the previous replace branch dropped them).
      existing.chain.push({ from: payload.from, to: payload.to });
      existing.at = Date.now();
    } else {
      state.reboundSessionIds[payload.id] = {
        at: Date.now(),
        chain: [{ from: payload.from, to: payload.to }],
      };
    }
  }

  // Shared by web + tauriArtifactTracker. Freshness window for the workspace_rebound
  // mark's VIEW heal (the basename-based reconcile arm): generous enough to
  // cover the rebind dialog's retry flow, short enough that an old mark
  // cannot misfire the arm on an unrelated later basename collision.
  // Deliberately does NOT delete the mark on expiry (review #463 round-C
  // Major 2): the save-path transform (rebaseArtifactPathsForRebind) is
  // prefix-exact and shares the mark, and its "whole process lifetime"
  // contract requires the mark to survive while the session's buffer may
  // still hold stale paths — pruning here re-armed the durable revert the
  // transform exists to prevent.
  const REBIND_RECONCILE_WINDOW_MS = 10 * 60 * 1000;
  function sessionRecentlyRebound(sid) {
    const marks = state.reboundSessionIds;
    if (!marks || !sid) return false;
    const mark = marks[sid];
    if (!mark || !mark.at) return false;
    return Date.now() - mark.at <= REBIND_RECONCILE_WINDOW_MS;
  }
  // Rebases absolute artifact paths along the session's rebind SEGMENT CHAIN
  // while a workspace_rebound mark exists (review #463 round-B Major 1 + the
  // round-D vintage fix): a post-rebind wholesale save of a chat turn's
  // buffer would otherwise durably revert the backend lane's rebase of
  // SavedSession.artifacts[].storage_path. Segments apply in order — an
  // A-era path resolves A→B→C, a buffer re-vintaged from the durable JSON
  // between chained rebinds (B-era) resolves B→C, a C-era path matches
  // nothing. Prefix-exact on the folded normalized form; the suffix keeps
  // the original casing. Relative paths already resolve against the CURRENT
  // workspace and are untouched; marks are memory-only, so a restart starts
  // from the already-rebased JSON with no marks and this is a no-op. Strips
  // trailing separators without a regex (the ESLint deny gate flags the
  // previous /\/+$/ form as super-linear).
  function trimTrailingSlashes(p) {
    let s = normalizedPath(p);
    while (s.endsWith("/")) s = s.slice(0, -1);
    return s;
  }
  // Locates the folded prefix match in the ORIGINAL string and returns the
  // code-unit length to cut there, or -1 when no prefix of `original` folds
  // to `foldedKey`. The fold is for the comparison only — the cut must NOT
  // use the folded prefix's length: expanding case mappings (U+0130 "İ"
  // lowercases to "i" + U+0307, two code units for one) make the folded form
  // longer than the original prefix, and slicing the original at the folded
  // length eats a character of the saved suffix (review #463 round-13:
  // "/data/İstanbul/a.txt" used to save as "<to>a.txt"). Lowercasing never
  // shrinks a string, so the scan can stop once the folded prefix outgrows
  // the key.
  function foldedPrefixCutLength(original, foldedKey) {
    for (let cut = 0; cut <= original.length; cut++) {
      const folded = original.slice(0, cut).toLowerCase();
      if (folded === foldedKey) return cut;
      if (folded.length >= foldedKey.length) return -1;
    }
    return -1;
  }
  function rebaseArtifactPathsForRebind(sid, paths) {
    const marks = state.reboundSessionIds;
    const mark = marks && sid ? marks[sid] : null;
    if (!mark || !Array.isArray(mark.chain) || !mark.chain.length || !Array.isArray(paths)) {
      return paths;
    }
    const segments = mark.chain
      .map(function (segment) {
        return {
          // The case fold is unconditional (review #463 round-13): the
          // bridges have no filesystem-case-sensitivity capability to gate
          // it on — adding one would span the Rust capability surface, both
          // lanes' platform wiring and this factory's deps, which is not a
          // small change. On a case-sensitive filesystem two roots differing
          // only in case are indistinguishable here, matching the folded
          // domain rule the backend lanes already apply on Windows.
          fromKey: trimTrailingSlashes(segment.from).toLowerCase(),
          toKey: trimTrailingSlashes(segment.to),
        };
      })
      .filter(function (segment) { return segment.fromKey; });
    if (!segments.length) return paths;
    return paths.map(function (p) {
      if (typeof p !== "string" || !isAbsPath(p)) return p;
      let norm = normalizedPath(p);
      let mapped = false;
      for (let i = 0; i < segments.length; i++) {
        const cut = foldedPrefixCutLength(norm, segments[i].fromKey);
        if (cut >= 0 && (cut === norm.length || norm.charAt(cut) === "/")) {
          norm = segments[i].toKey + norm.slice(cut);
          mapped = true;
        }
      }
      return mapped ? norm : p;
    });
  }

  // web+tauriArtifactTracker 共享
  function noteArtifactChange(path, event, sessionId) {
    if (!path) return;
    state.artifactChange = {
      seq: (state.artifactChange && state.artifactChange.seq || 0) + 1,
      path,
      event: event || "modified",
      sessionId: sessionId || "",
      at: Date.now(),
    };
    notify();
  }

  // web+tauriArtifactTracker 共享
  function isSharedMcpArtifactPath(path) {
    return normalizedPath(path).includes("/sessions/default/artifacts/");
  }

  // web+tauriArtifactTracker 共享
  function artifactBelongsToSession(path, sid) {
    if (!path || !sid) return false;
    if (!isAbsPath(path)) return true;
    if (isSharedMcpArtifactPath(path)) return true;
    const normalized = normalizedPath(path);
    if (normalized.includes("/sessions/")) {
      return normalized.includes("/sessions/" + sid + "/workspace/") ||
        normalized.includes("/sessions/" + sid + "/artifacts/");
    }
    return true;
  }

  // web+tauriArtifactTracker 共享
  function filterSessionArtifacts(artifacts, sid) {
    return (Array.isArray(artifacts) ? artifacts : []).filter(function (a) {
      return artifactBelongsToSession(a && a.path, sid);
    });
  }

  // web+tauriArtifactTracker 共享
  function isTmpPath(path) {
    const segs = normalizedPath(path).split("/");
    for (let i = 0; i < segs.length; i++) {
      if (segs[i] === "tmp") return true;
    }
    return false;
  }

  // web+tauriArtifactTracker 共享
  // biome-ignore lint/suspicious/noFunctionAssign: forwarder-dep routing reassigns the shared impl when the lane passes its own (see FORWARDER_DEP_NAMES)
  function isDeliverable(path) {
    if (isTmpPath(path)) return false;
    const ext = (String(path || "").split(".").pop() || "").toLowerCase();
    return DELIVERABLE_EXTS.value.has(ext);
  }

  // web+tauriArtifactTracker 共享
  function markTurnDirtyArtifact(path) {
    const bn = basename(path);
    if (!bn) return;
    if ((state.turnDirtyArtifacts || []).some(function (p) { return basename(p) === bn; })) return;
    state.turnDirtyArtifacts.push(path);
  }

  // web+tauriArtifactTracker 共享
  function untrackArtifact(path) {
    const before = state.artifacts.length;
    state.artifacts = state.artifacts.filter(function (a) { return a.path !== path; });
    if (state.artifacts.length !== before) notify();
  }

  // web+tauriArtifactTracker 共享
  function findPresentedArtifact(path) {
    const bn = basename(path);
    if (!bn) return null;
    const normalized = normalizedPath(path);
    let basenameMatch = null;
    for (let i = state.chatItems.length - 1; i >= 0; i--) {
      const it = state.chatItems[i];
      if (it.type !== "artifact_card" || basename(it.path) !== bn) continue;
      if (normalizedPath(it.path) === normalized) return it;
      if (!basenameMatch) basenameMatch = it;
    }
    return basenameMatch;
  }

  // web+tauriArtifactTracker 共享
  function updatePresentedArtifact(card) {
    if (!card || !card.path) return null;
    const existing = findPresentedArtifact(card.path);
    if (!existing) return null;
    // A relative tool path may be the only bridge between a persisted relative
    // card and its absolute watcher path. When it contains no resolvable
    // directory, same-named files remain ambiguous, so retain the basename
    // fallback for backward compatibility and rely on abs_path when available.
    if (isAbsPath(existing.path) && isAbsPath(card.path) &&
        normalizedPath(existing.path) !== normalizedPath(card.path)) return null;
    const bn = basename(card.path);
    for (let i = state.chatItems.length - 1; i >= 0; i--) {
      const it = state.chatItems[i];
      if (it === existing) break;
      if (it.type === "user") return null;
      if (it.type === "tool" && fileMutationAction(it.name, it.args) &&
          extractArtifactPaths(it.args).some(function (ap) { return basename(ap) === bn; })) break;
    }
    const stableId = existing.id;
    const stableAbsolutePath = isAbsPath(existing.path) && !isAbsPath(card.path)
      ? existing.path
      : null;
    Object.assign(existing, card, { type: "artifact_card" });
    if (stableId !== undefined) existing.id = stableId;
    if (stableAbsolutePath) existing.path = stableAbsolutePath;
    return existing;
  }

  // web+tauriArtifactTracker 共享
  function pushArtifactPath(paths, path) {
    if (typeof path !== "string" || !path.trim()) return;
    path = path.trim();
    if (!paths.includes(path)) paths.push(path);
  }

  // web+tauriArtifactTracker 共享
  function extractArtifactPath(args) {
    return extractArtifactPaths(args)[0] || null;
  }

  // web+tauriArtifactTracker 共享
  function fileMutationAction(name, args) {
    if (typeof args === "string") {
      try { args = JSON.parse(args); } catch { args = null; }
    }
    if (String(name || "").toLowerCase() === "file") {
      const action = String(args && args.action || "").toLowerCase();
      return ["write", "edit", "patch"].includes(action) ? action : null;
    }
    if (name === "write" || name === "write_file") return "write";
    if (name === "edit" || name === "edit_file") return "edit";
    return null;
  }

  // web+tauriMain 共享
  function composePlanMarkdown(snapshots) {
    const lines = [];
    const plan = snapshots && snapshots.plan;
    const todos = snapshots && snapshots.todos;
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

  // web+tauriChat 共享
  function isBusyFor(sid) {
    return sid === state.activeSessionId ? state.busy : !!(sessionStates.value[sid] && sessionStates.value[sid].busy);
  }

  // web+tauriChat 共享
  function formatAttachmentDisplayText(text, attachments) {
    const names = (attachments || []).map(function (attachment) {
      return typeof attachment === "string" ? attachment : attachment && attachment.basename;
    }).filter(Boolean).map(String);
    if (!names.length) return String(text || "");
    const attachmentLine = "📎 " + JSON.stringify(names);
    return String(text || "").trim()
      ? String(text) + "\n\n" + attachmentLine
      : attachmentLine;
  }

  // web+tauriChat 共享
  function queuedPayloadEnvelope(userText, payloadText, meta) {
    const user = String(userText || "");
    const payload = String(payloadText == null ? user : payloadText);
    if (!user) return { before: payload, after: "" };
    const requested = meta && meta.pinvouPayloadText
      ? String(meta.pinvouPayloadText).trim()
      : "";
    let index = -1;
    if (requested) {
      const requestedIndex = payload.indexOf(requested);
      let userIndex = -1;
      if (requested.startsWith(user)) userIndex = 0;
      else if (requested.endsWith(user)) userIndex = requested.length - user.length;
      else if (requested.indexOf(user) === requested.lastIndexOf(user)) userIndex = requested.indexOf(user);
      if (requestedIndex >= 0 && userIndex >= 0) index = requestedIndex + userIndex;
    } else if (payload === user || payload.endsWith(user)) {
      index = payload.length - user.length;
    } else if (payload.indexOf(user) === payload.lastIndexOf(user)) {
      index = payload.indexOf(user);
    }
    if (index < 0) return payload === user ? { before: "", after: "" } : null;
    return {
      before: payload.slice(0, index),
      after: payload.slice(index + user.length),
    };
  }

  // web+tauriChat 共享
  function makeQueuedMessage(id, userText, payloadText, displayText, attachments, meta, restrictTools) {
    return {
      id,
      text: userText,
      payloadText,
      payloadEnvelope: queuedPayloadEnvelope(userText, payloadText, meta),
      metaPayloadEnvelope: meta && meta.pinvouPayloadText
        ? queuedPayloadEnvelope(userText, meta.pinvouPayloadText, meta)
        : null,
      displayText,
      attachments,
      meta,
      restrictTools,
    };
  }

  // web+tauriChat 共享
  function rebuiltQueuedPayload(item, userText) {
    const envelope = item && item.payloadEnvelope;
    if (!envelope || typeof envelope.before !== "string" || typeof envelope.after !== "string") return null;
    return envelope.before + userText + envelope.after;
  }

  // web+tauriChat 共享
  function rebuiltQueuedMetaPayload(item, userText) {
    const envelope = item && item.metaPayloadEnvelope;
    if (!envelope || typeof envelope.before !== "string" || typeof envelope.after !== "string") return null;
    return envelope.before + userText + envelope.after;
  }

  // web+tauriChat 共享
  function getComposerDraft() {
    return String(state.composerDraft || "");
  }

  // web+tauriChat 共享
  function setComposerDraft(value) {
    const text = value == null ? "" : String(value);
    state.composerDraft = text;
    const activeBuffer = state.activeSessionId && sessionStates.value[state.activeSessionId];
    if (activeBuffer) activeBuffer.composerDraft = text;
    return text;
  }

  // web+tauriChat 共享
  // biome-ignore lint/suspicious/noFunctionAssign: forwarder-dep routing reassigns the shared impl when the lane passes its own (see FORWARDER_DEP_NAMES)
  function prefillComposer(text, append) {
    state.composerPrefill = {
      id: (state.composerPrefill.id || 0) + 1,
      text: String(text || ""),
      append: !!append,
    };
    notify();
  }

  // web+tauriChat 共享
  function inspectPinvou(focus) {
    return summonPinvou(focus, "coverage");
  }

  // web+tauriChat 共享
  function recordPinvouReview(review) {
    if (!state.activeSessionId || !review) return null;
    const pos = state.messages.length;
    state.pinvouReviews.push({ pos, review });
    const sid = state.activeSessionId;
    const snapshot = JSON.parse(JSON.stringify(state.pinvouReviews));
    invoke("save_session_pinvou_reviews", { sessionId: sid, reviews: snapshot }).catch(function () {});
    return pos; // 供卡片记 reviewPos,裁决时按 pos 定位原 state 写 resolution
  }

  // web+tauriChat 共享
  function dismissPinvouReview() {
    // 关窗即解召唤守卫:否则若在 await 期间被关(切 session 等路径),会留下"窗没了但
    // pinvouSummoning 仍 held"的死区——重复点品/悟在守卫处(summonPinvou 开头)被吞,要等
    // 整个直连 vLLM 调用(≤30s)返回才解锁。in-flight 结果靠 summonPinvou 内 `if (state.pinvouModal)` 守卫自然丢弃。
    state.pinvouModal = null;
    state.pinvouSummoning = false;
    notify();
  }

  // web+tauriChat 共享
  function persistPinvouReviews() {
    if (!state.activeSessionId) return Promise.resolve();
    const snapshot = JSON.parse(JSON.stringify(state.pinvouReviews));
    return invoke("save_session_pinvou_reviews", { sessionId: state.activeSessionId, reviews: snapshot }).catch(function () {});
  }

  // web+tauriMain 共享
  function planCardHydrationKey(item) {
    if (!item || item.type !== "plan_card") return "";
    if (item.planMarkdown) return "markdown:" + String(item.planMarkdown);
    try {
      return "snapshot:" + JSON.stringify({ plan: item.plan || null, todos: item.todos || null });
    } catch {
      return "";
    }
  }

  // web+tauriChatEvents 共享
  function reasoningEventIndex(e) {
    const value = e && e.payload && e.payload.index;
    if ([undefined, null, ""].includes(value)) return null;
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : String(value);
  }

  // web+tauriChatEvents 共享
  function streamingReasoningItem(index) {
    for (let itemIndex = state.chatItems.length - 1; itemIndex >= 0; itemIndex--) {
      const item = state.chatItems[itemIndex];
      if (!item || item.type !== "reasoning" || !item.streaming) continue;
      if ([undefined, null].includes(index) || item.reasoningIndex === index) return item;
    }
    return null;
  }

  // web+tauriChatEvents 共享
  function finalizeStreamingReasoning(index) {
    const completedAt = Date.now();
    for (let itemIndex = state.chatItems.length - 1; itemIndex >= 0; itemIndex--) {
      const item = state.chatItems[itemIndex];
      if (!item || item.type !== "reasoning" || !item.streaming) continue;
      if (index !== undefined && index !== null && item.reasoningIndex !== index) continue;
      item.streaming = false;
      item.completedAt = completedAt;
    }
  }

  // web+tauriArtifactTracker 共享
  function isPresentArtifactTool(name) {
    return name === "present_artifact" ||
      (typeof name === "string" && name.endsWith("present_artifact"));
  }

  // web+tauriArtifactTracker 共享
  function artifactPathFromToolOutput(toolResultContent) {
    const obj = parseToolResultPayload(toolResultContent);
    if (!obj || typeof obj !== "object") return null;
    const p = obj.abs_path || obj.path || obj.file_path || obj.local_path;
    return typeof p === "string" && p ? p : null;
  }

  // web+tauriArtifactTracker 共享
  function shouldUseToolOutputAsArtifact(name) {
    if (!name || isPresentArtifactTool(name)) return false;
    // Only MCP-style producer tools should be parsed from result JSON. Shell/read
    // tools often return diagnostic JSON with a `path` field, which is not a
    // newly created artifact.
    return typeof name === "string" && name.indexOf("mcp_") === 0;
  }

  // web+tauriArtifactTracker 共享
  function presentArtifactAbsPath(toolResultContent, fallbackPath) {
    fallbackPath = fallbackPath || "";
    const parsed = artifactPathFromToolOutput(toolResultContent);
    if (parsed) return parsed;
    return fallbackPath;
  }

  // web+tauriMonitor 共享
  function numOr0(x) { return (typeof x === "number" && Number.isFinite(x)) ? x : 0; }

  // web+tauriMonitor 共享
  function adjustCounters(sp, v) {
    sp = sp || {};
    const kvRatio = function (hit, miss) {
      const d = hit + miss;
      return d > 0 ? (hit / d * 100) : null;
    };
    let b = monitorBaseline.value;
    if (b) {
      const reset =
        numOr0(sp.ttft_sum_s) < b.ttft_sum_s ||
        numOr0(sp.tps_time_s) < b.tps_time_s ||
        numOr0(sp.gen_tokens_total) < b.gen_tokens ||
        numOr0(sp.prompt_tokens_total) < b.prompt_tokens ||
        numOr0(sp.cache_hit_tokens) < b.cache_hit ||
        numOr0(sp.cache_miss_tokens) < b.cache_miss ||
        (v && numOr0(v.prefix_cache_queries) < numOr0(b.pc_queries));
      if (reset) { clearMonitorBaseline(); b = null; }
    }
    const base = function (k) { return b ? numOr0(b[k]) : 0; };
    let vllmKvPct = null;
    if (v) {
      const pcH = numOr0(v.prefix_cache_hits) - base("pc_hits");
      const pcQ = numOr0(v.prefix_cache_queries) - base("pc_queries");
      vllmKvPct = pcQ > 0 ? (pcH / pcQ * 100) : null;
    }
    return {
      cleared: !!b,
      ttft_sum_s: numOr0(sp.ttft_sum_s) - base("ttft_sum_s"),
      ttft_count: numOr0(sp.ttft_count) - base("ttft_count"),
      tps_tokens: numOr0(sp.tps_tokens) - base("tps_tokens"),
      tps_time_s: numOr0(sp.tps_time_s) - base("tps_time_s"),
      gen: numOr0(sp.gen_tokens_total) - base("gen_tokens"),
      prompt: numOr0(sp.prompt_tokens_total) - base("prompt_tokens"),
      vllmKvPct,
      selfKvPct: kvRatio(
        numOr0(sp.cache_hit_tokens) - base("cache_hit"),
        numOr0(sp.cache_miss_tokens) - base("cache_miss")
      ),
      clearedAt: b ? (b.at || null) : null,
    };
  }

  // web+tauriMonitor 共享
  function startMonitorPolling() {
    if (monitorIntervalId.value) return;
    gpuUtilHistory.value = [];
    pollMonitor();
    monitorIntervalId.value = setInterval(pollMonitor, 1000);
  }

  // web+tauriMonitor 共享
  function stopMonitorPolling() {
    if (monitorIntervalId.value) {
      clearInterval(monitorIntervalId.value);
      monitorIntervalId.value = null;
    }
  }

  // web+tauriSettings 共享
  async function loadSettings() {
    try {
      state.settings = await invoke("get_settings");
      // The computer_use master switch is persisted via the dedicated command through
      // prefs (not the update_settings patch); at cold start with no session yet, the
      // persisted value lands first (a session-less computer_use_get_status read fills
      // the rest of the slice from the settings page's mount refresh).
      // Note: this mirror runs on every settings reload (loadSettings is
      // re-invoked after model save/delete/switch too) — it stays idempotent
      // because the persisted value always matches the last accepted
      // computer_use_set_enabled, and the runtime toggle's optimistic flip is
      // reverted by the bridge on failure. The state.computerUse slice only
      // exists on the Tauri host, so this is a no-op on web.
      const persistedEnabled = !!(state.settings && state.settings.computer_use && state.settings.computer_use.enabled);
      if (state.computerUse && state.computerUse.enabled !== persistedEnabled) {
        state.computerUse = Object.assign({}, state.computerUse, { enabled: persistedEnabled });
      }
    } catch {
      // Backend unreachable = nothing to judge; fall back to following the
      // system for the color scheme (color_scheme: system).
      state.settings = { theme: "genesis", color_scheme: "system", language: "zh-Hans" };
    }
    notify();
  }

  // web+tauriSettings 共享
  async function loadSelectedPet() {
    try {
      state.selectedPet = await invoke("get_selected_pet");
    } catch {
      state.selectedPet = "lingling";
    }
    notify();
  }

  // web+tauriSettings 共享
  async function setSelectedPet(id) {
    return invoke("set_selected_pet", { id });
  }

  // web+tauriSettings 共享
  function enqueueSettingsWrite(write) {
    const pending = settingsWriteQueue.value.then(write, write);
    settingsWriteQueue.value = pending.then(function () {}, function () {});
    return pending;
  }

  // web+tauriSettings 共享
  async function submitFeedback(request) {
    return invoke("submit_feedback", { request });
  }

  // web+tauriSettings 共享
  async function discoverLocalVllm(request) {
    return invoke("discover_local_vllm", { request: request || null });
  }

  // web+tauriSettings 共享
  function dismissVllmSetup() {
    state.vllmSetupDismissed = true;
    notify();
  }

  // web+tauriSettings 共享
  async function getEffectiveModelConfig(sessionId) {
    return invoke("get_effective_model_config", {
      sessionId: arguments.length ? (sessionId || null) : (state.activeSessionId || null),
    });
  }

  // web+tauriSettings 共享
  async function getImageInputCapability(sessionId) {
    return invoke("get_image_input_capability", {
      sessionId: arguments.length ? (sessionId || null) : (state.activeSessionId || null),
    });
  }

  // web+tauriSettings 共享
  async function loadModels() {
    const seq = ++modelsLoadSeq.value;
    try {
      const v = await invoke("list_models");
      if (seq !== modelsLoadSeq.value) return;
      state.savedModels = (v && v.models) || [];
      state.activeModelId = (v && v.active_model_id) || null;
    } catch {
      if (seq !== modelsLoadSeq.value) return;
      state.savedModels = []; state.activeModelId = null;
    }
    notify();
  }

  // web+tauriSettings 共享
  async function revealModelApiKey(id) {
   return invoke("reveal_model_api_key", { id });
 }

  // web+tauriSettings 共享
  async function switchModel(sessionId, modelId) {
    if (sessionId) {
      await invoke("set_session_model", { sessionId, modelId });
      await loadSessionModel(sessionId);
    } else {
      await setActiveModel(modelId);
    }
  }

  // web+tauriSettings 共享
  async function testModelConnection(baseUrl, apiKey, modelId) {
    return invoke("test_model_connection", { baseUrl, apiKey, modelId: modelId || null });
  }

  // web+tauriSettings 共享
  async function testImageInputCapability(model, baseUrl, apiKey, modelId) {
    return invoke("test_image_input_capability", { model, baseUrl, apiKey, modelId: modelId || null });
  }


  // web+tauriInteraction 共享
  async function refreshSuperPerm() {
    try {
      state.superPermEnabled = !!(await invoke("get_super_permission_status"));
    } catch {
      state.superPermEnabled = false;
    }
    notify();
  }

  // web+tauriInteraction 共享
  function setModeLane(lane) {
    const next = lane === "code" ? "code" : "work";
    if (state.modeLane === next) return;
    state.modeLane = next;
    if (!state.activeSessionId) {
      state.modeState = currentDraftModeState();
      notify();
    }
  }

  // web+tauriInteraction 共享
  function patchItemById(id, patch) {
    for (let i = 0; i < state.chatItems.length; i++) {
      if (state.chatItems[i].id === id) { Object.assign(state.chatItems[i], patch); break; }
    }
  }

  // web+tauriInteraction 共享
  function markResolved(id, statusLabel) { patchItemById(id, { resolved: true, statusLabel: statusLabel || "" }); notify(); }

  // web+tauriInteraction 共享
  // biome-ignore lint/suspicious/noFunctionAssign: forwarder-dep routing reassigns the shared impl when the lane passes its own (see FORWARDER_DEP_NAMES)
  function runOnSession(sid, fn) { runSyncOnSession(sid || state.activeSessionId, fn); }

  // web+tauriInteraction 共享
  function addSystemItemFor(sid, text) { runOnSession(sid, function () { addSystemItem(text); }); }

  // web+tauriInteraction 共享
  function patchItemByIdFor(sid, id, patch) { runOnSession(sid, function () { patchItemById(id, patch); }); }

  // web+tauriMemory 共享
  function memoryWriteLabel(event) {
    const text = event && event.text || "";
    if (!text) return "记忆已更新";
    return text;
  }

  // web+tauriMemory 共享
  function memoryWriteStatusLabel(event) {
    const action = event && event.action || "";
    if (action === "confirmed" || action === "remembered") return "记忆已更新";
    if (action === "archived") return "记忆已归档";
    if (action === "deleted") return "记忆已删除";
    return "记忆已更新";
  }

  // web+tauriMemory 共享
  function normalizeMemoryCandidateText(text) {
    return String(text || "").replaceAll(/\s+/g, " ").trim().toLowerCase();
  }

  // web+tauriMemory 共享
  function handleMemoryWrite(payload) {
    const sid = payload && payload.session_id || state.activeSessionId;
    const events = payload && Array.isArray(payload.events) ? payload.events : [];
    if (!sid || !events.length) return;
    runOnSession(sid, function () {
      events.forEach(function (event) {
        if (!event) return;
        if (event.action === "pending") {
          const label = memoryWriteLabel(event);
          const labelKey = normalizeMemoryCandidateText(label);
          const existing = state.chatItems.find(function (it) {
            return it.type === "memory_candidate" && !it.resolved && (
              (event.id && it.memoryId === event.id) ||
              (labelKey && normalizeMemoryCandidateText(it.text) === labelKey)
            );
          });
          if (existing) {
            existing.memoryId = event.id || existing.memoryId;
            existing.kind = event.kind || existing.kind || "preference";
            existing.text = label;
            existing.time = timeStr();
            return;
          }
          addChatItem({
            type: "memory_candidate",
            memoryId: event.id,
            kind: event.kind || "preference",
            text: label,
            time: timeStr(),
            resolved: false,
          });
          return;
        }
        const label = memoryWriteLabel(event);
        const labelKey = normalizeMemoryCandidateText(label);
        const existing = state.chatItems.find(function (it) {
          return it.type === "memory_candidate" && (
            (event.id && it.memoryId === event.id) ||
            (labelKey && normalizeMemoryCandidateText(it.text) === labelKey)
          );
        });
        if (existing) {
          if (event.action === "ignored" || event.action === "never") {
            state.chatItems = state.chatItems.filter(function (it) { return it !== existing; });
            return;
          }
          existing.resolved = true;
          existing.statusLabel = event.action === "ignored" ? "已忽略"
            : event.action === "never" ? "不再提示"
            : event.action === "archived" ? "已归档"
            : event.action === "deleted" ? "已删除"
            : "已记住";
          existing.kind = event.kind || existing.kind || "preference";
          existing.text = label;
          existing.time = timeStr();
          return;
        }
        if (event.action === "ignored" || event.action === "never") {
          return;
        }
        addChatItem({
          type: "memory_notice",
          memoryId: event.id,
          kind: event.kind || "preference",
          text: label,
          statusLabel: memoryWriteStatusLabel(event),
          time: timeStr(),
        });
      });
      notify();
    });
    if (invoke) {
      setTimeout(function () {
        loadMemoryOverview({ rehydratePending: true });
      }, 0);
    }
  }

  // web+tauriMemory 共享
  function orderedMemoryWarnings(warnings) {
    const items = Array.isArray(warnings) ? warnings : [];
    return [
      ...items.filter(function (warning) {
        return warning && warning.code === "memory_topic_cleanup_required";
      }),
      ...items.filter(function (warning) {
        return !warning || warning.code !== "memory_topic_cleanup_required";
      }),
    ];
  }

  // web+tauriMemory 共享
  function applyMemoryProfileState(result) {
    if (!result || !result.profile) return;
    state.memory = Object.assign({}, state.memory, {
      loading: false,
      error: null,
      profile: result.profile,
      runtime: result.runtime || null,
      warnings: orderedMemoryWarnings(result.warnings),
    });
  }

  // web+tauriMemory 共享
  function applyMemoryWriteState(result, update) {
    if (!result) return;
    const next = Object.assign({}, state.memory, {
      loading: false,
      error: null,
      runtime: result.runtime || null,
      warnings: orderedMemoryWarnings(result.warnings),
    });
    if (update) update(next, result.value);
    state.memory = next;
    notify();
  }

  // web+tauriMemory 共享
  function upsertMemoryValue(items, value, replacedId) {
    if (!value) return items || [];
    const next = (items || []).filter(function (item) {
      return item && item.id !== value.id && item.id !== replacedId;
    });
    next.push(value);
    return next;
  }

  // web+tauriMemory 共享
  function upsertPendingMemoryCandidate(item) {
    if (!item || item.status !== "pending_confirm") return;
    const label = item.content || item.text || "";
    if (!label) return;
    const labelKey = normalizeMemoryCandidateText(label);
    const existing = state.chatItems.find(function (it) {
      return it.type === "memory_candidate" && !it.resolved && (
        (item.id && it.memoryId === item.id) ||
        (labelKey && normalizeMemoryCandidateText(it.text) === labelKey)
      );
    });
    if (existing) {
      existing.memoryId = item.id || existing.memoryId;
      existing.kind = item.kind || existing.kind || "preference";
      existing.text = label;
      return;
    }
    addChatItem({
      type: "memory_candidate",
      memoryId: item.id,
      kind: item.kind || "preference",
      text: label,
      time: timeStr(),
      resolved: false,
    });
  }

  // web+tauriMemory 共享
  function rehydratePendingMemoryCandidates(overview) {
    const pending = overview && Array.isArray(overview.pending) ? overview.pending : [];
    pending.forEach(upsertPendingMemoryCandidate);
  }

  // web+tauriMemory 共享
  function discardStaleLoad(seq) {
    if (seq === memoryOverviewSeq.value) {
      state.memory = Object.assign({}, state.memory, { loading: false });
      notify();
    }
    return null;
  }

  // web+tauriMemory 共享
  async function loadOrganizeHistory() {
    if (!invoke) return [];
    return invoke("get_memory_organize_history");
  }

  // web+tauriInteraction 共享
  // biome-ignore lint/suspicious/noFunctionAssign: forwarder-dep routing reassigns the shared impl when the lane passes its own (see FORWARDER_DEP_NAMES)
  function startThinking() { state.thinking = { active: true, phase: "thinking", toolName: "", startedAt: Date.now() }; }

  // web+tauriInteraction 共享
  function thinkingTool(name) { state.thinking = { active: true, phase: "tool", toolName: name || "", startedAt: Date.now() }; }

  // web+tauriInteraction 共享
  function thinkingIdle() { state.thinking = { active: true, phase: "thinking", toolName: "", startedAt: Date.now() }; }

  // web+tauriInteraction 共享
  function stopThinking() { state.thinking = { active: false, phase: "thinking", toolName: "", startedAt: 0 }; }

  // web+tauriInteraction 共享
  function isActionablePlanCard(sid, itemId, planId) {
    if (!sid || sid !== state.activeSessionId || !itemId || !planId) return false;
    return state.chatItems.some(function (item) {
      return item && item.id === itemId && item.type === "plan_card" &&
        item.cardState === "active" && !item.resolved && String(item.planId || "") === planId;
    });
  }

  // web+tauriInteraction 共享
  async function setPlanModeNext() {
    // Draft state: do not materialize a session; rewrite this lane's global
    // default (two-lane semantics; the old implementation called ensureSession
    // first — clicking Plan on the draft page conjured an empty session).
    const sid = state.activeSessionId;
    if (!sid) { await setDraftMode("plan"); return; }
    try {
      const st = await invoke("set_plan_mode_next", { sessionId: sid });
      applyAuthoritativeModeState(sid, st);
    } catch (e) { addSystemItemFor(sid, bt("switchModeFailed") + e); }
    notify();
  }

  // web+tauriInteraction 共享
  async function planStuckReplan(itemId) {
    patchItemById(itemId, { resolved: true, statusLabel: bt("replanRequested") }); notify();
    await sendMessage(bt("planStuckReplanPrompt"));
  }

  // web+tauriInteraction 共享
  async function submitUserInput(itemId, toolCallId, answers, questions) {
    const sid = state.activeSessionId;
    if (!sid) return;
    patchItemByIdFor(sid, itemId, { submitting: true }); notify();
    try {
      await invoke("submit_user_input", { toolCallId, answers, sessionId: sid });
      // 摘要按 question 分组拼接：answers 是按选项展开的（multi_select 时同一题多条），
      // 不能按 answers 索引一一对应 questions（会越界抛 TypeError，复核 P1）。
      // 用无原型对象：question id 仅后端校验非空，constructor/toString/__proto__ 是合法输入，
      // 普通 {} 会让这些键命中 Object.prototype 继承属性，.push 抛 TypeError（复核 P1）。
      const byId = Object.create(null);
      answers.forEach(function (a) {
        if (a && a.id != null) {
          byId[a.id] = byId[a.id] || [];
          byId[a.id].push(a);
        }
      });
      const summary = questions.map(function (q, qi) {
        const list = byId[q.id];
        if (!list || !list.length) return null;
        const header = q.header || ("Q" + (qi + 1));
        return header + ": " + list.map(function (a) {
          const text = (a.other || a.label === "其他") ? bt("echoOtherPrefix") + a.value : a.label;
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

  // web+tauriInteraction 共享
  async function compactNow() {
    const sid = state.activeSessionId;
    if (!sid) return;
    try { await invoke("compact_now", { sessionId: state.activeSessionId }); } catch (e) {
      const compactErr = String(e || "");
      addSystemItemFor(sid, bt("compactFail") + ": " + (compactErr.includes("session_engine_not_running") ? bt("compactInactive") : compactErr));
    }
  }

  // web+tauriArtifacts 共享
  function openContainingFolder(path) { return invoke("open_containing_folder", { path }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }

  // web+tauriArtifacts 共享
  function revealSessionFolder(sessionId) { return invoke("reveal_session_folder", { sessionId }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }

  // web+tauriArtifacts 共享
  function openScheduledTaskFolder(automationId) { return invoke("open_scheduled_task_folder", { automationId }).catch(function (e) { addSystemItem(bt("openFailed") + e); }); }

  // web+tauriArtifacts 共享
  function deliverableCategory(path) {
    const ext = (String(path || "").split(".").pop() || "").toLowerCase();
    if (["html", "htm", "mhtml", "mht"].includes(ext)) return "web";
    if (["ppt", "pptx", "odp", "dps"].includes(ext)) return "ppt";
    if (["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "heic"].includes(ext)) return "img";
    return "doc";
  }

  // web+tauriArtifacts 共享
  function sessionTitleById(sid) {
    const m = state.sessions.find(function (s) { return s.id === sid; });
    return (m && m.title) || "";
  }

  // web+tauriArtifacts 共享
  function currentMemoryArtifacts() {
    const rows = [];
    function addFrom(sid, arts) {
      (arts || []).forEach(function (a) {
        const path = a && a.path;
        if (!path || !isDeliverable(path)) return;
        rows.push({ path, sessionId: sid || state.activeSessionId, source: sessionTitleById(sid || state.activeSessionId), name: basename(path) });
      });
    }
    addFrom(state.activeSessionId, state.artifacts);
    Object.keys(sessionStates.value).forEach(function (sid) { addFrom(sid, sessionStates.value[sid] && sessionStates.value[sid].artifacts); });
    return rows;
  }

  // web+tauriArtifacts 共享
  function conversationAttachmentArgs(reference) {
    reference = reference || {};
    return {
      sessionId: reference.sessionId || state.activeSessionId,
      messageIndex: Number(reference.messageIndex),
      attachmentIndex: Number(reference.attachmentIndex),
      basename: String(reference.basename || ""),
      displayText: String(reference.displayText || ""),
    };
  }

  // web+tauriArtifacts 共享
  async function pickAndAttach() {
    if (!dialogOpen) { addSystemItem(bt("filePickUnavailable")); return; }
    try {
      const selected = await dialogOpen({ multiple: true });
      if (!selected) return;
      const paths = Array.isArray(selected) ? selected : [selected];
      for (let i = 0; i < paths.length; i++) { await addAttachmentByPath(paths[i]); }
    } catch (e) { addSystemItem(bt("filePickFailed") + e); }
  }

  // web+tauriPersonas 共享
  async function loadPersonas() {
    if (state.personaPool.loadState === "ready" || state.personaPool.loadState === "loading") return;
    await refreshPersonas();
  }

  // web+tauriPersonas 共享
  async function refreshPersonas() {
    state.personaPool.loadState = "loading"; notify();
    try {
      personaPoolCache.value = await invoke("list_personas");
      state.personaPool.loadState = "ready";
    } catch (e) {
      personaPoolCache.value = []; state.personaPool.loadState = "error";
      console.warn("list_personas failed", e);
    }
    notify();
  }

  // web+tauriPersonas 共享
  async function createPersona(input) {
    const sum = await invoke("create_persona", { input });
    deletedPersonaIds.value.delete(sum.id);
    await refreshPersonas();
    return sum;
  }

  // web+tauriPersonas 共享
  async function updatePersona(personaId, input) {
    const sum = await invoke("update_persona", { personaId, input });
    await refreshPersonas();
    if (deletedPersonaIds.value.has(personaId)) return null;
    // 若改的正是当前 session 加持的卡, 同步挂件显示
    if (state.activePersona && state.activePersona.id === personaId) { state.activePersona = sum; notify(); }
    return sum;
  }

  // web+tauriPersonas 共享
  async function deletePersona(personaId) {
    await invoke("delete_persona", { personaId });
    // Invalidate live and cached selections only after deletion succeeds.
    // Late reads/equip responses must not restore a card that no longer exists.
    deletedPersonaIds.value.add(personaId);
    if (state.activePersona && state.activePersona.id === personaId) state.activePersona = null;
    Object.values(sessionStates.value).forEach(function (buffer) {
      if (buffer.activePersona && buffer.activePersona.id === personaId) buffer.activePersona = null;
    });
    notify();
    await refreshPersonas();
  }

  // web+tauriPersonas 共享
  function personaName(p) {
    if (!p) return "";
    // 内置卡名按 UI 语言显示(personas-i18n.js overlay),中文兜底;自制卡不翻
    const lang = state.settings && state.settings.language;
    const L = lang === "en" ? "en" : lang === "ja" ? "ja" : null;
    const tr = L && p.source !== "user" && window.PERSONA_I18N && window.PERSONA_I18N[p.id] && window.PERSONA_I18N[p.id][L];
    if (tr && tr.name) return tr.name;
    return (p.name || p.cn_name) || "";
  }

  // web+tauriPersonas 共享
  function recordPersonaEvent(ev) {
    if (!state.activeSessionId) return;
    ev.pos = state.messages.length;
    state.personaEvents.push(ev);
    const sid = state.activeSessionId;
    const snapshot = JSON.parse(JSON.stringify(state.personaEvents));
    invoke("save_session_persona_events", { sessionId: sid, events: snapshot }).catch(function () {});
  }

  // web+tauriPersonas 共享
  function postCardCreatorIntro(sid) {
    const target = sid || lastEquippedSid.value || state.activeSessionId;
    if (!target) return;
    runOnSession(target, function () {
      addChatItem({ type: "card_creator_intro", time: "" });
      recordPersonaEvent({ kind: "card_creator_intro" });
      notify();
    });
  }

  // web+tauriChatEvents+tauriPersonas 共享
  function normalizeMountedCollections(value) {
    if (!Array.isArray(value)) return [];
    const seen = Object.create(null);
    return value.map(function (entry) {
      if (entry == null) return null;
      const collectionId = typeof entry === "object"
        ? (entry.collectionId == null ? entry.collection_id : entry.collectionId)
        : entry;
      if (collectionId == null || seen[String(collectionId)]) return null;
      seen[String(collectionId)] = true;
      return { collectionId, enabled: typeof entry === "object" ? entry.enabled !== false : true };
    }).filter(Boolean);
  }

  // web+tauriPersonas 共享
  function applyMountedCollections(value) {
    const hasSnapshot = value && !Array.isArray(value) && Array.isArray(value.collections);
    const revision = hasSnapshot ? Number(value.revision || 0) : Number(state.mountedCollectionsRevision || 0);
    if (hasSnapshot && revision < Number(state.mountedCollectionsRevision || 0)) {
      return normalizeMountedCollections(state.mountedCollections);
    }
    const normalized = normalizeMountedCollections(hasSnapshot ? value.collections : value);
    state.mountedCollections = normalized;
    state.mountedCollectionsRevision = revision;
    const firstEnabled = normalized.find(function (entry) { return entry.enabled; });
    state.mountedCollection = firstEnabled ? firstEnabled.collectionId : null;
    return normalized;
  }

  // web+tauriPersonas 共享
  function mountedCollectionTargetAtEnqueue() {
    if (state.activeSessionId) return { draft: false, promise: Promise.resolve(state.activeSessionId) };
    const draftEpoch = Number(state.draftEpoch || 0);
    if (!mountedCollectionDraftTarget.value || mountedCollectionDraftTarget.value.epoch !== draftEpoch || mountedCollectionDraftTarget.value.failed) {
      const target = { draft: true, epoch: draftEpoch, failed: false, pending: 0, promise: null };
      target.promise = Promise.resolve().then(async function () {
        // Navigation before draft materialization cancels this batch instead of
        // silently retargeting it to the newly active session.
        if (state.activeSessionId) return null;
        const sessionId = await ensureSession();
        if (!sessionId) target.failed = true;
        return sessionId;
      });
      mountedCollectionDraftTarget.value = target;
    }
    mountedCollectionDraftTarget.value.pending += 1;
    return mountedCollectionDraftTarget.value;
  }

  // web+tauriPersonas 共享
  function updateMountedCollections(command, args) {
    const requestedTarget = mountedCollectionTargetAtEnqueue();
    mountedCollectionUpdate.value = mountedCollectionUpdate.value.catch(function () {}).then(async function () {
      // The target is captured at click time. Rapid draft actions share one
      // materialization promise and remain bound to that session after navigation.
      const sessionId = await requestedTarget.promise;
      if (!sessionId) return null;
      try {
        const saved = await invoke(command, Object.assign({ sessionId }, args || {}));
        const normalized = normalizeMountedCollections(saved && saved.collections);
        if (state.activeSessionId === sessionId) {
          applyMountedCollections(saved);
          notify();
        }
        return normalized;
      } catch (e) {
        addSystemItem(bt("mountCollectionFailed") + e);
        return null;
      }
    });
    if (requestedTarget.draft) {
      mountedCollectionUpdate.value = mountedCollectionUpdate.value.finally(function () {
        requestedTarget.pending -= 1;
        if (requestedTarget.pending === 0 && mountedCollectionDraftTarget.value === requestedTarget) {
          mountedCollectionDraftTarget.value = null;
        }
      });
    }
    return mountedCollectionUpdate.value;
  }

  // web+tauriPersonas 共享
  async function mountCollection(collectionId) {
    if (collectionId == null) return null;
    const saved = await updateMountedCollections("session_add_mounted_collection", { collectionId });
    return saved ? collectionId : null;
  }

  // web+tauriPersonas 共享
  async function setCollectionEnabled(collectionId, enabled) {
    return updateMountedCollections("session_set_mounted_collection_enabled", {
      collectionId,
      enabled: !!enabled,
    });
  }

  // web+tauriPersonas 共享
  async function removeCollection(collectionId) {
    return updateMountedCollections("session_remove_mounted_collection", { collectionId });
  }

  // web+tauriUpdater 共享
  async function checkForUpdate() {
    state.updateChecking = true; state.updateCheckError = null; notify();
    try {
      const info = await invoke("check_for_update");
      if (info && info.current_version) state.appVersion = info.current_version;
      state.updateInfo = info;
      if (!info.available) state.updateCheckError = "latest"; // 前端按 i18n 显示「已是最新」
    } catch (e) {
      state.updateCheckError = String(e);
    }
    state.updateChecking = false; notify();
  }

  // web+tauriDependencies 共享
  async function checkDependencies() {
    if (state.depsChecking) return;
    state.depsChecking = true; state.depsInstallError = null; notify();
    try {
      state.deps = await invoke("check_dependencies");
    } catch { state.deps = []; }
    state.depsChecking = false; notify();
  }

  // web+tauriVoice 共享
  function setVoiceInputStatus(status, patch) {
    const next = Object.assign({}, state.voiceInput, patch || {});
    next.status = status;
    if (status !== "failed") {
      next.error = null;
      next.category = null;
    }
    state.voiceInput = next;
    notify();
  }

  // web+tauriVoice 共享
  function emitVoiceDiagnostic(stage, level, message, userMessage, category) {
    const event = {
      stage,
      level,
      message,
      user_message: userMessage || "",
      category: category || "",
    };
    const fn = level === "error" ? console.error : level === "warn" ? console.warn : console.info;
    fn.call(console, "[voice-input]", event);
  }

  // web+tauriVoice 共享
  function voiceFlowError(category, stage, message) {
    const error = new Error(message);
    error.category = category;
    error.stage = stage;
    return error;
  }

  // web+tauriVoice 共享
  function requestVoiceMedia(session, constraints, timeoutMs) {
    let abandoned = false;
    const mediaPromise = navigator.mediaDevices.getUserMedia(constraints).then(function (stream) {
      if (abandoned || activeVoiceInput.value !== session) {
        stopMediaTracks(stream);
        throw voiceFlowError("cancelled", "permission", bt("voiceCancelled"));
      }
      return stream;
    });
    const timeoutPromise = new Promise(function (_, reject) {
      session.permissionTimeoutId = setTimeout(function () {
        abandoned = true;
        reject(voiceFlowError("device_unavailable", "device", bt("voiceDeviceTimeout")));
      }, timeoutMs || VOICE_DEVICE_REQUEST_TIMEOUT_MS.value);
    });
    const cancelPromise = new Promise(function (_, reject) {
      session.cancelPermissionRequest = function () {
        abandoned = true;
        reject(voiceFlowError("cancelled", "permission", bt("voiceCancelled")));
      };
    });
    return Promise.race([mediaPromise, timeoutPromise, cancelPromise]).finally(function () {
      if (session.permissionTimeoutId) clearTimeout(session.permissionTimeoutId);
      session.permissionTimeoutId = null;
      session.cancelPermissionRequest = null;
    });
  }

  // web+tauriVoice 共享
  function mergeFloatChunks(chunks) {
    const total = chunks.reduce(function (sum, chunk) { return sum + chunk.length; }, 0);
    const out = new Float32Array(total);
    let offset = 0;
    chunks.forEach(function (chunk) {
      out.set(chunk, offset);
      offset += chunk.length;
    });
    return out;
  }

  // web+tauriVoice 共享
  function downsamplePcm(samples, sourceRate, targetRate) {
    if (!samples.length || sourceRate === targetRate) return samples;
    const ratio = sourceRate / targetRate;
    const len = Math.max(1, Math.round(samples.length / ratio));
    const out = new Float32Array(len);
    for (let i = 0; i < len; i++) {
      const start = Math.floor(i * ratio);
      const end = Math.min(samples.length, Math.floor((i + 1) * ratio));
      let sum = 0;
      let count = 0;
      for (let j = start; j < end; j++) { sum += samples[j]; count++; }
      out[i] = count ? sum / count : samples[Math.min(start, samples.length - 1)];
    }
    return out;
  }

  // web+tauriVoice 共享
  function closeVoiceAsrSetup() {
    state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, { open: false });
    notify();
  }

  // web+tauriKnowledgeModel 共享
  async function downloadKbModel(repair) {
    if (state.kbModelSetup.downloading) return state.kbModelSetup.status;
    state.kbModelSetup = Object.assign({}, state.kbModelSetup, { downloading: true, error: null, progress: { stage: "start" } });
    notify();
    try {
      const st = await invoke("kb_model_download", { repair: !!repair });
      state.kbModelSetup = Object.assign({}, state.kbModelSetup, {
        downloading: false,
        startupLoading: false,
        startupReady: st && typeof st.ready === "boolean" ? st.ready : true,
        status: st,
        progress: { stage: "done" },
      });
      notify();
      return st;
    } catch (e) {
      const failedStatus = await invoke("kb_model_status").catch(function () { return null; });
      state.kbModelSetup = Object.assign({}, state.kbModelSetup, {
        downloading: false,
        startupLoading: false,
        startupReady: failedStatus && typeof failedStatus.ready === "boolean" ? failedStatus.ready : false,
        status: failedStatus || state.kbModelSetup.status,
        error: String(e),
      });
      notify();
      throw e;
    }
  }

  // web+tauriVoice 共享
  function cancelVoiceInput() {
    finishVoiceInput(true, false);
  }

  // web+tauriVoice 共享
  function clearVoiceInput() {
    if (activeVoiceInput.value) {
      finishVoiceInput(true, false);
      return;
    }
    setVoiceInputStatus("idle", {
      message: "",
      error: null,
      category: null,
      stage: null,
      sessionId: null,
    });
  }

  // web+tauriVoice 共享
  function appendVoiceText(base, text) {
    const left = String(base || "").trimEnd();
    const right = String(text || "").trim();
    if (!left) return right;
    if (!right) return left;
    return left + (/[。！？.!?，,;；:]$/.test(left) ? " " : "\n") + right;
  }

  // web:158313+tauriSessions:47342 共享
  function interruptedDisplayRange(item) {
      if (!item || item.interruptedDisplayOnly !== true) return null;
      let anchorIndex = -1;
      let nextUserIndex = -1;
      const afterMessageIndex = Number(item.afterMessageIndex);
      if (Number.isFinite(afterMessageIndex) && afterMessageIndex >= 0) {
        for (let index = 0; index < state.chatItems.length; index++) {
          const candidate = state.chatItems[index];
          if (!candidate || candidate.type !== "user") continue;
          const candidateMessageIndex = Number(candidate.messageIndex);
          if (candidateMessageIndex === afterMessageIndex) anchorIndex = index;
          else if (anchorIndex >= 0 && candidateMessageIndex > afterMessageIndex) {
            nextUserIndex = index;
            break;
          }
        }
      }
      const afterUserOrdinal = Number(item.afterUserOrdinal);
      if (anchorIndex < 0 && Number.isSafeInteger(afterUserOrdinal) && afterUserOrdinal >= 0) {
        let userOrdinal = -1;
        for (let fallbackIndex = 0; fallbackIndex < state.chatItems.length; fallbackIndex++) {
          const fallback = state.chatItems[fallbackIndex];
          if (!fallback || fallback.type !== "user") continue;
          userOrdinal += 1;
          if (userOrdinal === afterUserOrdinal) anchorIndex = fallbackIndex;
          else if (userOrdinal > afterUserOrdinal) {
            nextUserIndex = fallbackIndex;
            break;
          }
        }
      }
      if (anchorIndex < 0) {
        return { start: state.chatItems.length, end: state.chatItems.length };
      }
      return {
        start: anchorIndex + 1,
        end: nextUserIndex >= 0 ? nextUserIndex : state.chatItems.length,
      };
    }

  // web:189622+tauriMain:99856 共享
  function emitPersonaAt(atOrAfter, isTail) {
      for (let k = 0; k < pe.length; k++) {
        const ev = pe[k];
        if (isTail ? (ev.pos < atOrAfter) : (ev.pos !== atOrAfter)) continue;
        if (ev.kind === "equip" && ev.card) addChatItem({ type: "persona_equip", card: ev.card, time: "" });
        else if (ev.kind === "unequip") addChatItem({ type: "system", text: bt("personaUnequipped") + (ev.name || ""), time: "" });
        else if (ev.kind === "card_creator_intro") addChatItem({ type: "card_creator_intro", time: "" });
      }
    }

  // web:247496+tauriChat:31666 共享
  function restoreUiTurnState(consumed) {
      if (!consumed || state.activeSessionId !== sid.value) return;
      state.scheduledTaskPendingGuide = consumed.scheduledTaskPendingGuide;
      state.scheduledTaskCreationSessionId = consumed.scheduledTaskCreationSessionId;
      state.activeSkill = consumed.activeSkill;
    }

  // lane 转发优先：以下名字在部分 lane 会以 deps（lane 内转发函数）传入；
  // 有传入时保持合并前的解析路径（转发到原 cluster 实例），未传入时用上方共享实现。
  const FORWARDER_DEP_NAMES = [
    "startThinking",
    "getBuffer",
    "setScheduledTaskError",
    "addSystemItem",
    "bt",
    "basename",
    "invalidateScheduledRecentRunsForSession",
    "loadScheduledTaskRecentRuns",
    "rememberScheduledRunOwner",
    "isScheduledRunTerminal",
    "createNewSession",
    "prefillComposer",
    "runOnSession",
    "timeStr",
    "isDeliverable",
  ];
  /* eslint-disable no-func-assign -- 转发函数优先：按 lane 传入值重新指向函数声明绑定，保持合并前解析路径 */
  const forwarderSetters = {
    startThinking(v) { startThinking = v; },
    getBuffer(v) { getBuffer = v; },
    setScheduledTaskError(v) { setScheduledTaskError = v; },
    addSystemItem(v) { addSystemItem = v; },
    bt(v) { bt = v; },
    basename(v) { basename = v; },
    invalidateScheduledRecentRunsForSession(v) { invalidateScheduledRecentRunsForSession = v; },
    loadScheduledTaskRecentRuns(v) { loadScheduledTaskRecentRuns = v; },
    rememberScheduledRunOwner(v) { rememberScheduledRunOwner = v; },
    isScheduledRunTerminal(v) { isScheduledRunTerminal = v; },
    createNewSession(v) { createNewSession = v; },
    prefillComposer(v) { prefillComposer = v; },
    runOnSession(v) { runOnSession = v; },
    timeStr(v) { timeStr = v; },
    isDeliverable(v) { isDeliverable = v; },
  };
  /* eslint-enable no-func-assign */
  for (const name of FORWARDER_DEP_NAMES) {
    if (deps[name] !== undefined) forwarderSetters[name](deps[name]);
  }

    return { bt, textMatchesBtKey, isDefaultChatTitle, authoritySyncBufferSnapshot, normalizePinvouScene, pinvouSceneStorageKey, normalizePinvouSceneEvents, loadPinvouSceneEventsForSession, savePinvouSceneEventsForSession, recordPinvouSceneForMessage, pinvouSceneForMessagePos, getBuffer, isProtectedScheduledBuffer, touchSessionBuffer, registerScheduledRunOwner, scheduledRunOwnerVisibleRank, scheduledRunOwnerPriority, pruneScheduledRunSessionOwners, isScheduledRunTerminal, rememberScheduledRunOwner, scheduledRunBuffer, markScheduledInitialTurnActive, markScheduledInitialTurnTerminal, beginScheduledOpenActivation, rollbackScheduledOpenActivation, markRemoteTurn, onSessionEvent, isScheduledRunSession, defineSubscriptionStateProperty, copySubscriptionStateObject, loadScheduledTaskTemplateSources, rememberScheduledTaskTemplateSource, attachScheduledTaskTemplateSource, attachAndPruneScheduledTaskTemplateSources, upsertScheduledTask, applyScheduledRunViewed, invalidateScheduledTaskReads, invalidateScheduledRecentRuns, invalidateScheduledRecentRunsForSession, scheduleScheduledRunRefresh, scheduledTaskErrorText, setScheduledTaskError, dismissScheduledTaskError, clearScheduledTaskLoadError, beginScheduledTaskLoad, endScheduledTaskLoad, scheduledTaskRequestStamp, isCurrentScheduledTaskRequest, selectScheduledTask, clearScheduledTaskSelection, extractBalancedJsonObject, normalizeScheduledTaskDraft, activeScheduledTaskModelConfig, lockScheduledTaskDraftModel, scheduledTaskInputFromDraft, loadScheduledTasks, readScheduledTask, mergeScheduledTaskRecentRuns, loadScheduledTaskRuns, loadScheduledTaskRecentRuns, refreshScheduledTaskData, refreshScheduledRunShortcutUntilLinked, upsertScheduledTaskRun, runScheduledTaskAction, updateScheduledTask, pauseScheduledTask, resumeScheduledTask, toggleScheduledTaskPinned, deleteScheduledTask, runScheduledTaskNow, startScheduledTaskChat, toolCallAlreadyFinished, hasChatItemForTool, addSystemItem, addAuthoritySyncNotice, compactPruneRollupText, removeCompactionStartItem, addOrMergePruneCompaction, timeStr, createNewSession, reportSessionSwitchFailure, mergeHydratedMessages, hydratedChatItemKey, switchToSession, openScheduledRunChat, exitScheduledRunChat, recentScheduledRunForSession, leaveSessionView, applyDeletedSession, renameSession, toggleSessionPinned, archiveSession, restoreArchivedSession, toolResultText, stripInternalToolRuntimeSuffix, toolResultDisplayContent, parsePlanSnapshot, parseUserAnswers, parseCarefulBlocked, userMessageInputProvenance, isInternalUserMessageProvenance, isShellExecutionTool, utf8Length, formatShellSnapshot, shellCommandForItem, shellSnapshotKey, terminalShellHistoryMatch, applyShellSnapshots, scheduleShellPoll, runShellPoll, patchLastItem, hasUnresolvedItem, basename, isAbsPath, normalizedPath, applyWorkspaceReboundMark, sessionRecentlyRebound, rebaseArtifactPathsForRebind, noteArtifactChange, isSharedMcpArtifactPath, artifactBelongsToSession, filterSessionArtifacts, isTmpPath, isDeliverable, markTurnDirtyArtifact, untrackArtifact, findPresentedArtifact, updatePresentedArtifact, pushArtifactPath, extractArtifactPath, fileMutationAction, composePlanMarkdown, isBusyFor, formatAttachmentDisplayText, queuedPayloadEnvelope, makeQueuedMessage, rebuiltQueuedPayload, rebuiltQueuedMetaPayload, getComposerDraft, setComposerDraft, prefillComposer, inspectPinvou, recordPinvouReview, dismissPinvouReview, persistPinvouReviews, planCardHydrationKey, reasoningEventIndex, streamingReasoningItem, finalizeStreamingReasoning, isPresentArtifactTool, artifactPathFromToolOutput, shouldUseToolOutputAsArtifact, presentArtifactAbsPath, numOr0, adjustCounters, startMonitorPolling, stopMonitorPolling, loadSettings, loadSelectedPet, setSelectedPet, enqueueSettingsWrite, submitFeedback, discoverLocalVllm, dismissVllmSetup, getEffectiveModelConfig, getImageInputCapability, loadModels, revealModelApiKey, switchModel, testModelConnection, testImageInputCapability, refreshSuperPerm, setModeLane, patchItemById, markResolved, runOnSession, addSystemItemFor, patchItemByIdFor, memoryWriteLabel, memoryWriteStatusLabel, normalizeMemoryCandidateText, handleMemoryWrite, orderedMemoryWarnings, applyMemoryProfileState, applyMemoryWriteState, upsertMemoryValue, upsertPendingMemoryCandidate, rehydratePendingMemoryCandidates, discardStaleLoad, loadOrganizeHistory, startThinking, thinkingTool, thinkingIdle, stopThinking, isActionablePlanCard, setPlanModeNext, planStuckReplan, submitUserInput, compactNow, openContainingFolder, revealSessionFolder, openScheduledTaskFolder, deliverableCategory, sessionTitleById, currentMemoryArtifacts, conversationAttachmentArgs, pickAndAttach, loadPersonas, refreshPersonas, createPersona, updatePersona, deletePersona, personaName, recordPersonaEvent, postCardCreatorIntro, normalizeMountedCollections, applyMountedCollections, mountedCollectionTargetAtEnqueue, updateMountedCollections, mountCollection, setCollectionEnabled, removeCollection, checkForUpdate, checkDependencies, setVoiceInputStatus, emitVoiceDiagnostic, voiceFlowError, requestVoiceMedia, mergeFloatChunks, downsamplePcm, closeVoiceAsrSetup, downloadKbModel, cancelVoiceInput, clearVoiceInput, appendVoiceText, interruptedDisplayRange, emitPersonaAt, restoreUiTurnState };
  }

  // 嵌套 cluster（<base>:<offset>）：web/tauri 两侧正文逐字相同，每对共用一个工厂并注册在两个 key 下。
  const nestedInterruptedDisplayRange = function (deps) {
    const b = sharedBridgeBase(ensureCells(deps));
    return Object.freeze({ interruptedDisplayRange: b.interruptedDisplayRange });
  };

  const nestedEmitPersonaAt = function (deps) {
    const b = sharedBridgeBase(ensureCells(deps));
    return Object.freeze({ emitPersonaAt: b.emitPersonaAt });
  };

  const nestedRestoreUiTurnState = function (deps) {
    const b = sharedBridgeBase(ensureCells(deps));
    return Object.freeze({ restoreUiTurnState: b.restoreUiTurnState });
  };

  const CLUSTERS = {
    "web": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ bt: b.bt, textMatchesBtKey: b.textMatchesBtKey, isDefaultChatTitle: b.isDefaultChatTitle, authoritySyncBufferSnapshot: b.authoritySyncBufferSnapshot, normalizePinvouScene: b.normalizePinvouScene, pinvouSceneStorageKey: b.pinvouSceneStorageKey, normalizePinvouSceneEvents: b.normalizePinvouSceneEvents, loadPinvouSceneEventsForSession: b.loadPinvouSceneEventsForSession, savePinvouSceneEventsForSession: b.savePinvouSceneEventsForSession, recordPinvouSceneForMessage: b.recordPinvouSceneForMessage, pinvouSceneForMessagePos: b.pinvouSceneForMessagePos, getBuffer: b.getBuffer, isProtectedScheduledBuffer: b.isProtectedScheduledBuffer, touchSessionBuffer: b.touchSessionBuffer, registerScheduledRunOwner: b.registerScheduledRunOwner, scheduledRunOwnerVisibleRank: b.scheduledRunOwnerVisibleRank, scheduledRunOwnerPriority: b.scheduledRunOwnerPriority, pruneScheduledRunSessionOwners: b.pruneScheduledRunSessionOwners, isScheduledRunTerminal: b.isScheduledRunTerminal, rememberScheduledRunOwner: b.rememberScheduledRunOwner, scheduledRunBuffer: b.scheduledRunBuffer, markScheduledInitialTurnActive: b.markScheduledInitialTurnActive, markScheduledInitialTurnTerminal: b.markScheduledInitialTurnTerminal, beginScheduledOpenActivation: b.beginScheduledOpenActivation, rollbackScheduledOpenActivation: b.rollbackScheduledOpenActivation, markRemoteTurn: b.markRemoteTurn, onSessionEvent: b.onSessionEvent, isScheduledRunSession: b.isScheduledRunSession, defineSubscriptionStateProperty: b.defineSubscriptionStateProperty, copySubscriptionStateObject: b.copySubscriptionStateObject, loadScheduledTaskTemplateSources: b.loadScheduledTaskTemplateSources, rememberScheduledTaskTemplateSource: b.rememberScheduledTaskTemplateSource, attachScheduledTaskTemplateSource: b.attachScheduledTaskTemplateSource, attachAndPruneScheduledTaskTemplateSources: b.attachAndPruneScheduledTaskTemplateSources, upsertScheduledTask: b.upsertScheduledTask, applyScheduledRunViewed: b.applyScheduledRunViewed, invalidateScheduledTaskReads: b.invalidateScheduledTaskReads, invalidateScheduledRecentRuns: b.invalidateScheduledRecentRuns, invalidateScheduledRecentRunsForSession: b.invalidateScheduledRecentRunsForSession, scheduleScheduledRunRefresh: b.scheduleScheduledRunRefresh, scheduledTaskErrorText: b.scheduledTaskErrorText, setScheduledTaskError: b.setScheduledTaskError, dismissScheduledTaskError: b.dismissScheduledTaskError, clearScheduledTaskLoadError: b.clearScheduledTaskLoadError, beginScheduledTaskLoad: b.beginScheduledTaskLoad, endScheduledTaskLoad: b.endScheduledTaskLoad, scheduledTaskRequestStamp: b.scheduledTaskRequestStamp, isCurrentScheduledTaskRequest: b.isCurrentScheduledTaskRequest, selectScheduledTask: b.selectScheduledTask, clearScheduledTaskSelection: b.clearScheduledTaskSelection, extractBalancedJsonObject: b.extractBalancedJsonObject, normalizeScheduledTaskDraft: b.normalizeScheduledTaskDraft, activeScheduledTaskModelConfig: b.activeScheduledTaskModelConfig, lockScheduledTaskDraftModel: b.lockScheduledTaskDraftModel, scheduledTaskInputFromDraft: b.scheduledTaskInputFromDraft, loadScheduledTasks: b.loadScheduledTasks, readScheduledTask: b.readScheduledTask, mergeScheduledTaskRecentRuns: b.mergeScheduledTaskRecentRuns, loadScheduledTaskRuns: b.loadScheduledTaskRuns, loadScheduledTaskRecentRuns: b.loadScheduledTaskRecentRuns, refreshScheduledTaskData: b.refreshScheduledTaskData, refreshScheduledRunShortcutUntilLinked: b.refreshScheduledRunShortcutUntilLinked, upsertScheduledTaskRun: b.upsertScheduledTaskRun, runScheduledTaskAction: b.runScheduledTaskAction, updateScheduledTask: b.updateScheduledTask, pauseScheduledTask: b.pauseScheduledTask, resumeScheduledTask: b.resumeScheduledTask, toggleScheduledTaskPinned: b.toggleScheduledTaskPinned, deleteScheduledTask: b.deleteScheduledTask, runScheduledTaskNow: b.runScheduledTaskNow, startScheduledTaskChat: b.startScheduledTaskChat, toolCallAlreadyFinished: b.toolCallAlreadyFinished, hasChatItemForTool: b.hasChatItemForTool, addSystemItem: b.addSystemItem, addAuthoritySyncNotice: b.addAuthoritySyncNotice, compactPruneRollupText: b.compactPruneRollupText, removeCompactionStartItem: b.removeCompactionStartItem, addOrMergePruneCompaction: b.addOrMergePruneCompaction, timeStr: b.timeStr, createNewSession: b.createNewSession, reportSessionSwitchFailure: b.reportSessionSwitchFailure, mergeHydratedMessages: b.mergeHydratedMessages, hydratedChatItemKey: b.hydratedChatItemKey, switchToSession: b.switchToSession, openScheduledRunChat: b.openScheduledRunChat, exitScheduledRunChat: b.exitScheduledRunChat, recentScheduledRunForSession: b.recentScheduledRunForSession, leaveSessionView: b.leaveSessionView, applyDeletedSession: b.applyDeletedSession, renameSession: b.renameSession, toggleSessionPinned: b.toggleSessionPinned, archiveSession: b.archiveSession, restoreArchivedSession: b.restoreArchivedSession, toolResultText: b.toolResultText, stripInternalToolRuntimeSuffix: b.stripInternalToolRuntimeSuffix, toolResultDisplayContent: b.toolResultDisplayContent, parsePlanSnapshot: b.parsePlanSnapshot, parseUserAnswers: b.parseUserAnswers, parseCarefulBlocked: b.parseCarefulBlocked, userMessageInputProvenance: b.userMessageInputProvenance, isInternalUserMessageProvenance: b.isInternalUserMessageProvenance, isShellExecutionTool: b.isShellExecutionTool, utf8Length: b.utf8Length, formatShellSnapshot: b.formatShellSnapshot, shellCommandForItem: b.shellCommandForItem, shellSnapshotKey: b.shellSnapshotKey, terminalShellHistoryMatch: b.terminalShellHistoryMatch, applyShellSnapshots: b.applyShellSnapshots, scheduleShellPoll: b.scheduleShellPoll, runShellPoll: b.runShellPoll, patchLastItem: b.patchLastItem, hasUnresolvedItem: b.hasUnresolvedItem, basename: b.basename, isAbsPath: b.isAbsPath, normalizedPath: b.normalizedPath, applyWorkspaceReboundMark: b.applyWorkspaceReboundMark, sessionRecentlyRebound: b.sessionRecentlyRebound, rebaseArtifactPathsForRebind: b.rebaseArtifactPathsForRebind, noteArtifactChange: b.noteArtifactChange, isSharedMcpArtifactPath: b.isSharedMcpArtifactPath, artifactBelongsToSession: b.artifactBelongsToSession, filterSessionArtifacts: b.filterSessionArtifacts, isTmpPath: b.isTmpPath, isDeliverable: b.isDeliverable, markTurnDirtyArtifact: b.markTurnDirtyArtifact, untrackArtifact: b.untrackArtifact, findPresentedArtifact: b.findPresentedArtifact, updatePresentedArtifact: b.updatePresentedArtifact, pushArtifactPath: b.pushArtifactPath, extractArtifactPath: b.extractArtifactPath, fileMutationAction: b.fileMutationAction, composePlanMarkdown: b.composePlanMarkdown, isBusyFor: b.isBusyFor, formatAttachmentDisplayText: b.formatAttachmentDisplayText, queuedPayloadEnvelope: b.queuedPayloadEnvelope, makeQueuedMessage: b.makeQueuedMessage, rebuiltQueuedPayload: b.rebuiltQueuedPayload, rebuiltQueuedMetaPayload: b.rebuiltQueuedMetaPayload, getComposerDraft: b.getComposerDraft, setComposerDraft: b.setComposerDraft, prefillComposer: b.prefillComposer, inspectPinvou: b.inspectPinvou, recordPinvouReview: b.recordPinvouReview, dismissPinvouReview: b.dismissPinvouReview, persistPinvouReviews: b.persistPinvouReviews, planCardHydrationKey: b.planCardHydrationKey, reasoningEventIndex: b.reasoningEventIndex, streamingReasoningItem: b.streamingReasoningItem, finalizeStreamingReasoning: b.finalizeStreamingReasoning, isPresentArtifactTool: b.isPresentArtifactTool, artifactPathFromToolOutput: b.artifactPathFromToolOutput, shouldUseToolOutputAsArtifact: b.shouldUseToolOutputAsArtifact, presentArtifactAbsPath: b.presentArtifactAbsPath, numOr0: b.numOr0, adjustCounters: b.adjustCounters, startMonitorPolling: b.startMonitorPolling, stopMonitorPolling: b.stopMonitorPolling, loadSettings: b.loadSettings, loadSelectedPet: b.loadSelectedPet, setSelectedPet: b.setSelectedPet, enqueueSettingsWrite: b.enqueueSettingsWrite, submitFeedback: b.submitFeedback, discoverLocalVllm: b.discoverLocalVllm, dismissVllmSetup: b.dismissVllmSetup, getEffectiveModelConfig: b.getEffectiveModelConfig, getImageInputCapability: b.getImageInputCapability, loadModels: b.loadModels, revealModelApiKey: b.revealModelApiKey, switchModel: b.switchModel, testModelConnection: b.testModelConnection, testImageInputCapability: b.testImageInputCapability, refreshSuperPerm: b.refreshSuperPerm, setModeLane: b.setModeLane, patchItemById: b.patchItemById, markResolved: b.markResolved, runOnSession: b.runOnSession, addSystemItemFor: b.addSystemItemFor, patchItemByIdFor: b.patchItemByIdFor, memoryWriteLabel: b.memoryWriteLabel, memoryWriteStatusLabel: b.memoryWriteStatusLabel, normalizeMemoryCandidateText: b.normalizeMemoryCandidateText, handleMemoryWrite: b.handleMemoryWrite, orderedMemoryWarnings: b.orderedMemoryWarnings, applyMemoryProfileState: b.applyMemoryProfileState, applyMemoryWriteState: b.applyMemoryWriteState, upsertMemoryValue: b.upsertMemoryValue, upsertPendingMemoryCandidate: b.upsertPendingMemoryCandidate, rehydratePendingMemoryCandidates: b.rehydratePendingMemoryCandidates, discardStaleLoad: b.discardStaleLoad, loadOrganizeHistory: b.loadOrganizeHistory, startThinking: b.startThinking, thinkingTool: b.thinkingTool, thinkingIdle: b.thinkingIdle, stopThinking: b.stopThinking, isActionablePlanCard: b.isActionablePlanCard, setPlanModeNext: b.setPlanModeNext, planStuckReplan: b.planStuckReplan, submitUserInput: b.submitUserInput, compactNow: b.compactNow, openContainingFolder: b.openContainingFolder, revealSessionFolder: b.revealSessionFolder, openScheduledTaskFolder: b.openScheduledTaskFolder, deliverableCategory: b.deliverableCategory, sessionTitleById: b.sessionTitleById, currentMemoryArtifacts: b.currentMemoryArtifacts, conversationAttachmentArgs: b.conversationAttachmentArgs, pickAndAttach: b.pickAndAttach, loadPersonas: b.loadPersonas, refreshPersonas: b.refreshPersonas, createPersona: b.createPersona, updatePersona: b.updatePersona, deletePersona: b.deletePersona, personaName: b.personaName, recordPersonaEvent: b.recordPersonaEvent, postCardCreatorIntro: b.postCardCreatorIntro, normalizeMountedCollections: b.normalizeMountedCollections, applyMountedCollections: b.applyMountedCollections, mountedCollectionTargetAtEnqueue: b.mountedCollectionTargetAtEnqueue, updateMountedCollections: b.updateMountedCollections, mountCollection: b.mountCollection, setCollectionEnabled: b.setCollectionEnabled, removeCollection: b.removeCollection, checkForUpdate: b.checkForUpdate, checkDependencies: b.checkDependencies, setVoiceInputStatus: b.setVoiceInputStatus, emitVoiceDiagnostic: b.emitVoiceDiagnostic, voiceFlowError: b.voiceFlowError, requestVoiceMedia: b.requestVoiceMedia, mergeFloatChunks: b.mergeFloatChunks, downsamplePcm: b.downsamplePcm, closeVoiceAsrSetup: b.closeVoiceAsrSetup, downloadKbModel: b.downloadKbModel, cancelVoiceInput: b.cancelVoiceInput, clearVoiceInput: b.clearVoiceInput, appendVoiceText: b.appendVoiceText });
    },
    "tauriMain": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ authoritySyncBufferSnapshot: b.authoritySyncBufferSnapshot, bt: b.bt, textMatchesBtKey: b.textMatchesBtKey, isDefaultChatTitle: b.isDefaultChatTitle, normalizePinvouScene: b.normalizePinvouScene, pinvouSceneStorageKey: b.pinvouSceneStorageKey, normalizePinvouSceneEvents: b.normalizePinvouSceneEvents, loadPinvouSceneEventsForSession: b.loadPinvouSceneEventsForSession, savePinvouSceneEventsForSession: b.savePinvouSceneEventsForSession, recordPinvouSceneForMessage: b.recordPinvouSceneForMessage, pinvouSceneForMessagePos: b.pinvouSceneForMessagePos, markRemoteTurn: b.markRemoteTurn, onSessionEvent: b.onSessionEvent, isScheduledRunSession: b.isScheduledRunSession, planCardHydrationKey: b.planCardHydrationKey, defineSubscriptionStateProperty: b.defineSubscriptionStateProperty, copySubscriptionStateObject: b.copySubscriptionStateObject, toolResultText: b.toolResultText, stripInternalToolRuntimeSuffix: b.stripInternalToolRuntimeSuffix, toolResultDisplayContent: b.toolResultDisplayContent, parsePlanSnapshot: b.parsePlanSnapshot, parseUserAnswers: b.parseUserAnswers, parseCarefulBlocked: b.parseCarefulBlocked, userMessageInputProvenance: b.userMessageInputProvenance, isInternalUserMessageProvenance: b.isInternalUserMessageProvenance, patchLastItem: b.patchLastItem, hasUnresolvedItem: b.hasUnresolvedItem, composePlanMarkdown: b.composePlanMarkdown });
    },
    "tauriSessions": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ getBuffer: b.getBuffer, isProtectedScheduledBuffer: b.isProtectedScheduledBuffer, touchSessionBuffer: b.touchSessionBuffer, registerScheduledRunOwner: b.registerScheduledRunOwner, scheduledRunOwnerVisibleRank: b.scheduledRunOwnerVisibleRank, scheduledRunOwnerPriority: b.scheduledRunOwnerPriority, pruneScheduledRunSessionOwners: b.pruneScheduledRunSessionOwners, isScheduledRunTerminal: b.isScheduledRunTerminal, rememberScheduledRunOwner: b.rememberScheduledRunOwner, scheduledRunBuffer: b.scheduledRunBuffer, markScheduledInitialTurnActive: b.markScheduledInitialTurnActive, markScheduledInitialTurnTerminal: b.markScheduledInitialTurnTerminal, beginScheduledOpenActivation: b.beginScheduledOpenActivation, rollbackScheduledOpenActivation: b.rollbackScheduledOpenActivation, createNewSession: b.createNewSession, reportSessionSwitchFailure: b.reportSessionSwitchFailure, mergeHydratedMessages: b.mergeHydratedMessages, hydratedChatItemKey: b.hydratedChatItemKey, switchToSession: b.switchToSession, openScheduledRunChat: b.openScheduledRunChat, exitScheduledRunChat: b.exitScheduledRunChat, recentScheduledRunForSession: b.recentScheduledRunForSession, leaveSessionView: b.leaveSessionView, applyDeletedSession: b.applyDeletedSession, renameSession: b.renameSession, toggleSessionPinned: b.toggleSessionPinned, archiveSession: b.archiveSession, restoreArchivedSession: b.restoreArchivedSession, applyWorkspaceReboundMark: b.applyWorkspaceReboundMark });
    },
    "tauriScheduled": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ loadScheduledTaskTemplateSources: b.loadScheduledTaskTemplateSources, rememberScheduledTaskTemplateSource: b.rememberScheduledTaskTemplateSource, attachScheduledTaskTemplateSource: b.attachScheduledTaskTemplateSource, attachAndPruneScheduledTaskTemplateSources: b.attachAndPruneScheduledTaskTemplateSources, upsertScheduledTask: b.upsertScheduledTask, applyScheduledRunViewed: b.applyScheduledRunViewed, invalidateScheduledTaskReads: b.invalidateScheduledTaskReads, invalidateScheduledRecentRuns: b.invalidateScheduledRecentRuns, invalidateScheduledRecentRunsForSession: b.invalidateScheduledRecentRunsForSession, scheduleScheduledRunRefresh: b.scheduleScheduledRunRefresh, scheduledTaskErrorText: b.scheduledTaskErrorText, setScheduledTaskError: b.setScheduledTaskError, dismissScheduledTaskError: b.dismissScheduledTaskError, clearScheduledTaskLoadError: b.clearScheduledTaskLoadError, beginScheduledTaskLoad: b.beginScheduledTaskLoad, endScheduledTaskLoad: b.endScheduledTaskLoad, scheduledTaskRequestStamp: b.scheduledTaskRequestStamp, isCurrentScheduledTaskRequest: b.isCurrentScheduledTaskRequest, selectScheduledTask: b.selectScheduledTask, clearScheduledTaskSelection: b.clearScheduledTaskSelection, extractBalancedJsonObject: b.extractBalancedJsonObject, normalizeScheduledTaskDraft: b.normalizeScheduledTaskDraft, activeScheduledTaskModelConfig: b.activeScheduledTaskModelConfig, lockScheduledTaskDraftModel: b.lockScheduledTaskDraftModel, scheduledTaskInputFromDraft: b.scheduledTaskInputFromDraft, loadScheduledTasks: b.loadScheduledTasks, readScheduledTask: b.readScheduledTask, mergeScheduledTaskRecentRuns: b.mergeScheduledTaskRecentRuns, loadScheduledTaskRuns: b.loadScheduledTaskRuns, loadScheduledTaskRecentRuns: b.loadScheduledTaskRecentRuns, refreshScheduledTaskData: b.refreshScheduledTaskData, refreshScheduledRunShortcutUntilLinked: b.refreshScheduledRunShortcutUntilLinked, upsertScheduledTaskRun: b.upsertScheduledTaskRun, runScheduledTaskAction: b.runScheduledTaskAction, updateScheduledTask: b.updateScheduledTask, pauseScheduledTask: b.pauseScheduledTask, resumeScheduledTask: b.resumeScheduledTask, toggleScheduledTaskPinned: b.toggleScheduledTaskPinned, deleteScheduledTask: b.deleteScheduledTask, runScheduledTaskNow: b.runScheduledTaskNow, startScheduledTaskChat: b.startScheduledTaskChat });
    },
    "tauriChat": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ getComposerDraft: b.getComposerDraft, setComposerDraft: b.setComposerDraft, toolCallAlreadyFinished: b.toolCallAlreadyFinished, hasChatItemForTool: b.hasChatItemForTool, addSystemItem: b.addSystemItem, addAuthoritySyncNotice: b.addAuthoritySyncNotice, compactPruneRollupText: b.compactPruneRollupText, removeCompactionStartItem: b.removeCompactionStartItem, addOrMergePruneCompaction: b.addOrMergePruneCompaction, timeStr: b.timeStr, isBusyFor: b.isBusyFor, formatAttachmentDisplayText: b.formatAttachmentDisplayText, queuedPayloadEnvelope: b.queuedPayloadEnvelope, makeQueuedMessage: b.makeQueuedMessage, rebuiltQueuedPayload: b.rebuiltQueuedPayload, rebuiltQueuedMetaPayload: b.rebuiltQueuedMetaPayload, prefillComposer: b.prefillComposer, inspectPinvou: b.inspectPinvou, recordPinvouReview: b.recordPinvouReview, dismissPinvouReview: b.dismissPinvouReview, persistPinvouReviews: b.persistPinvouReviews });
    },
    "web:158313": nestedInterruptedDisplayRange,
    "tauriSessions:47342": nestedInterruptedDisplayRange,
    "web:189622": nestedEmitPersonaAt,
    "tauriMain:99856": nestedEmitPersonaAt,
    "tauriTerminal": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ isShellExecutionTool: b.isShellExecutionTool, utf8Length: b.utf8Length, formatShellSnapshot: b.formatShellSnapshot, shellCommandForItem: b.shellCommandForItem, shellSnapshotKey: b.shellSnapshotKey, terminalShellHistoryMatch: b.terminalShellHistoryMatch, applyShellSnapshots: b.applyShellSnapshots, scheduleShellPoll: b.scheduleShellPoll, runShellPoll: b.runShellPoll });
    },
    "tauriArtifactTracker": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ basename: b.basename, isAbsPath: b.isAbsPath, normalizedPath: b.normalizedPath, sessionRecentlyRebound: b.sessionRecentlyRebound, rebaseArtifactPathsForRebind: b.rebaseArtifactPathsForRebind, noteArtifactChange: b.noteArtifactChange, isSharedMcpArtifactPath: b.isSharedMcpArtifactPath, artifactBelongsToSession: b.artifactBelongsToSession, filterSessionArtifacts: b.filterSessionArtifacts, isTmpPath: b.isTmpPath, isDeliverable: b.isDeliverable, markTurnDirtyArtifact: b.markTurnDirtyArtifact, untrackArtifact: b.untrackArtifact, findPresentedArtifact: b.findPresentedArtifact, updatePresentedArtifact: b.updatePresentedArtifact, pushArtifactPath: b.pushArtifactPath, extractArtifactPath: b.extractArtifactPath, fileMutationAction: b.fileMutationAction, isPresentArtifactTool: b.isPresentArtifactTool, artifactPathFromToolOutput: b.artifactPathFromToolOutput, shouldUseToolOutputAsArtifact: b.shouldUseToolOutputAsArtifact, presentArtifactAbsPath: b.presentArtifactAbsPath });
    },
    "web:247496": nestedRestoreUiTurnState,
    "tauriChat:31666": nestedRestoreUiTurnState,
    "tauriChatEvents": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ reasoningEventIndex: b.reasoningEventIndex, streamingReasoningItem: b.streamingReasoningItem, finalizeStreamingReasoning: b.finalizeStreamingReasoning, normalizeMountedCollections: b.normalizeMountedCollections });
    },
    "tauriMonitor": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ numOr0: b.numOr0, adjustCounters: b.adjustCounters, startMonitorPolling: b.startMonitorPolling, stopMonitorPolling: b.stopMonitorPolling });
    },
    "tauriSettings": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ loadSettings: b.loadSettings, loadSelectedPet: b.loadSelectedPet, setSelectedPet: b.setSelectedPet, enqueueSettingsWrite: b.enqueueSettingsWrite, submitFeedback: b.submitFeedback, discoverLocalVllm: b.discoverLocalVllm, dismissVllmSetup: b.dismissVllmSetup, getEffectiveModelConfig: b.getEffectiveModelConfig, getImageInputCapability: b.getImageInputCapability, loadModels: b.loadModels, revealModelApiKey: b.revealModelApiKey, switchModel: b.switchModel, testModelConnection: b.testModelConnection, testImageInputCapability: b.testImageInputCapability });
    },
    "tauriInteraction": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ refreshSuperPerm: b.refreshSuperPerm, setModeLane: b.setModeLane, patchItemById: b.patchItemById, markResolved: b.markResolved, runOnSession: b.runOnSession, addSystemItemFor: b.addSystemItemFor, patchItemByIdFor: b.patchItemByIdFor, startThinking: b.startThinking, thinkingTool: b.thinkingTool, thinkingIdle: b.thinkingIdle, stopThinking: b.stopThinking, isActionablePlanCard: b.isActionablePlanCard, setPlanModeNext: b.setPlanModeNext, planStuckReplan: b.planStuckReplan, submitUserInput: b.submitUserInput, compactNow: b.compactNow });
    },
    "tauriMemory": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ memoryWriteLabel: b.memoryWriteLabel, memoryWriteStatusLabel: b.memoryWriteStatusLabel, normalizeMemoryCandidateText: b.normalizeMemoryCandidateText, handleMemoryWrite: b.handleMemoryWrite, orderedMemoryWarnings: b.orderedMemoryWarnings, applyMemoryProfileState: b.applyMemoryProfileState, applyMemoryWriteState: b.applyMemoryWriteState, upsertMemoryValue: b.upsertMemoryValue, upsertPendingMemoryCandidate: b.upsertPendingMemoryCandidate, rehydratePendingMemoryCandidates: b.rehydratePendingMemoryCandidates, discardStaleLoad: b.discardStaleLoad, loadOrganizeHistory: b.loadOrganizeHistory });
    },
    "tauriArtifacts": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ openContainingFolder: b.openContainingFolder, revealSessionFolder: b.revealSessionFolder, openScheduledTaskFolder: b.openScheduledTaskFolder, deliverableCategory: b.deliverableCategory, sessionTitleById: b.sessionTitleById, currentMemoryArtifacts: b.currentMemoryArtifacts, conversationAttachmentArgs: b.conversationAttachmentArgs, pickAndAttach: b.pickAndAttach });
    },
    "tauriPersonas": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ loadPersonas: b.loadPersonas, refreshPersonas: b.refreshPersonas, createPersona: b.createPersona, updatePersona: b.updatePersona, deletePersona: b.deletePersona, personaName: b.personaName, recordPersonaEvent: b.recordPersonaEvent, postCardCreatorIntro: b.postCardCreatorIntro, normalizeMountedCollections: b.normalizeMountedCollections, applyMountedCollections: b.applyMountedCollections, mountedCollectionTargetAtEnqueue: b.mountedCollectionTargetAtEnqueue, updateMountedCollections: b.updateMountedCollections, mountCollection: b.mountCollection, setCollectionEnabled: b.setCollectionEnabled, removeCollection: b.removeCollection });
    },
    "tauriUpdater": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ checkForUpdate: b.checkForUpdate });
    },
    "tauriDependencies": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ checkDependencies: b.checkDependencies });
    },
    "tauriVoice": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ setVoiceInputStatus: b.setVoiceInputStatus, emitVoiceDiagnostic: b.emitVoiceDiagnostic, voiceFlowError: b.voiceFlowError, requestVoiceMedia: b.requestVoiceMedia, mergeFloatChunks: b.mergeFloatChunks, downsamplePcm: b.downsamplePcm, closeVoiceAsrSetup: b.closeVoiceAsrSetup, cancelVoiceInput: b.cancelVoiceInput, clearVoiceInput: b.clearVoiceInput, appendVoiceText: b.appendVoiceText });
    },
    "tauriKnowledgeModel": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ downloadKbModel: b.downloadKbModel });
    },
  };

  root.PinvouBridgeShared = Object.freeze({
    create: function (cluster, deps) {
      const factory = CLUSTERS[cluster];
      if (!factory) throw new Error("Unknown bridge shared cluster: " + cluster);
      return factory(deps);
    },
  });
// eslint-disable-next-line unicorn/no-this-outside-of-class -- UMD root reference
})(typeof window === "undefined" ? this : window);
