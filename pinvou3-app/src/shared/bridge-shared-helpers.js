/**
 * bridge-shared-helpers.js — 供 plain-script bridge（platform/web、platform/tauri）共用的桥接工具函数集。
 *
 * 背景：platform/{web,tauri}/bridge*.js 是以 <script src> 加载的普通脚本（非 ES module）。
 * 此前数百个逐字相同（byte-identical）的桥接工具函数在两条 lane 里各存一份，维护时必须双改；
 * 现已合并：246 个共享函数只在 sharedBridgeBase 定义一次（正文取自原 web 侧逐字镜像），
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

  // FE14 wave 5：桥接状态切片字段注册表——tauri/bridge.js（桌面订阅快照）与
  // web/bridge/domain-adapter.js（Web 端 getMany/stablePick）共用同一份常量，
  // 由 root.PinvouBridgeShared.STATE_SLICE_FIELDS 静态导出（无 deps、两 lane 只读）。
  // projects 由 projects feature (bridge/projects.js) 注册；必须与 main.jsx 的
  // APP_BRIDGE_STATE_DOMAINS 保持同步 —— tests/bridge_state_domains.test.mjs 锁定。
  const STATE_SLICE_FIELDS = {
    platform: ["appVersion", "backendOnline", "platformCapabilities"],
    sessions: ["sessions", "archivedSessions", "activeSessionId", "sessionBusy", "draftEpoch", "draftWorkspacePath"],
    chat: ["activeSkill", "artifacts", "artifactChange", "attachments", "busy", "chatItems", "composerDraft", "composerPrefill", "messages", "modeState", "planSnapshot", "queued", "thinking", "tokens", "turnDirtyArtifacts", "turnPresentedArtifacts", "turnTimeline"],
    voice: ["voiceInput", "voiceAsrSetup"],
    knowledge: ["kbModelSetup", "mountedCollection", "mountedCollections", "mountedRemoteCollections", "mountedCollectionsRevision"],
    scheduled: ["scheduledRunContext", "scheduledTaskAutoOpenId", "scheduledTaskBusyAction", "scheduledTaskCreationSessionId", "scheduledTaskDetail", "scheduledTaskError", "scheduledTaskErrorKind", "scheduledTaskLoading", "scheduledTaskPendingGuide", "scheduledTaskRecentRuns", "scheduledTaskRuns", "scheduledTasks", "scheduledTaskSelectionGeneration", "selectedScheduledTaskId"],
    monitor: ["monitor", "monitorError"],
    settings: ["settings", "selectedPet"],
    models: ["activeModelId", "currentSessionModelId", "effectiveModelConfig", "savedModels"],
    vllm: [],
    interaction: ["pinvouModal", "pinvouReviews", "pinvouSummoning", "superPermEnabled"],
    computerUse: ["computerUse"],
    personas: ["activePersona", "personaEvents", "personaPool"],
    memory: ["memory"],
    remoteControl: ["webAccess"],
    updater: ["updateCancelling", "updateCheckError", "updateChecking", "updateDownloading", "updateError", "updateInfo", "updateProgress", "updateReady"],
    dependencies: ["deps", "depsChecking", "depsInstallError", "depsInstallProgress", "depsInstalling"],
    projects: ["projectsList"],
  };

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
      state, invoke, restoreEvictedSessionDraft, evictedSessionDrafts, MAX_EVICTED_SESSION_DRAFT_CHARS, MAX_EVICTED_SESSION_DRAFTS, pruneScheduledSessionBuffers, pruneSessionBuffers,
      recordAuthoritySyncDiagnostic, runSyncOnSession, notify, SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY, persistScheduledTaskTemplateSources,
      scheduledTaskBackendInput, forgetScheduledTaskTemplateSource, addChatItem, enterDraft,
      switchToSessionInternal, openScheduledRunChatOnce, loadWorkingSetFrom, purgeSessionBuffer, refreshHistoryList, saveWorkingSetTo, userMessageDisplayText,
      normalizeTerminalTail, latestShellToolIsWaitObserver, extractArtifactPaths, clearMonitorBaseline,
      loadSessionModel, setActiveModel, currentDraftModeState, setDraftMode,
      applyAuthoritativeModeState, sendMessage, pushUserEcho, dialogOpen, addAttachmentByPath,
      ensureSession, stopMediaTracks, finishVoiceInput, BT_TABLE, PINVOU_SCENE_EVENTS_STORAGE_PREFIX, sessionStates,
      sessionBufferTouchClock, scheduledRunSessionOwners, scheduledRunOwnerTouchClock, MAX_SCHEDULED_RUN_SESSION_OWNERS, scheduledTaskTemplateSources, scheduledTaskRequestTokens,
      scheduledTaskRefreshInFlight, scheduledRecentRunsRequestToken, scheduledRunEventRefreshTimer, scheduledTaskPendingLoads, scheduledTaskSelectionGeneration, scheduledRunShortcutRefreshes,
      SCHEDULED_LINK_POLL_DEADLINE_MS, SCHEDULED_LINK_POLL_FAST_ATTEMPTS, SCHEDULED_LINK_POLL_FAST_MS, SCHEDULED_LINK_POLL_SLOW_MS, scheduledTaskAutoCreateSeq, scheduledTaskAutoCreateInFlight, scheduledRunOpenInFlight,
      personaPlaceholderTitles, sessionSwitchRequestToken, SHELL_TOOL_NAMES, shellPollState, DELIVERABLE_EXTS, monitorBaseline,
      monitorIntervalId, monitorPollInFlight, gpuUtilHistory, settingsWriteQueue, modelsLoadSeq, memoryOverviewSeq, personaPoolCache, personaSyncSeq,
      deletedPersonaIds, lastEquippedSid, mountedCollectionDraftTarget, mountedCollectionUpdate, activeVoiceInput, voiceOperations, VOICE_DEVICE_REQUEST_TIMEOUT_MS,
      pe, sid, ensureSessionBufferLoaded, auxChatDispatch, candidateCardPatcher, pendingStreamNotifications, pendingAssistantText, pendingAssistantBlocks, itemIdSeq, flushQueued, doSendFor, reconcileRemoteTurn, finalizeAssistantStreamBeforeReasoning, flushPendingTextBlock
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
  // biome-ignore lint/suspicious/noFunctionAssign: forwarder-dep routing reassigns the shared impl when the lane passes its own (see FORWARDER_DEP_NAMES)
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
  // options.deferBackgroundNotify: stream deltas schedule their own coalesced
  // publication, so the per-event background notify is skipped for them.
  function onSessionEvent(e, fn, options) {
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
    if (isBg && !(options && options.deferBackgroundNotify)) notify();
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

  // web+tauriMain 共享（FE14 wave 5）
  // eslint-disable-next-line sonarjs/cognitive-complexity -- verbatim structural-sharing reconciler; splitting it would obscure the allocation invariants
  function subscriptionStateValue(value, previous, ancestors) {
    const valueType = typeof value;
    if (!value || valueType !== "object") {
      if (["function", "symbol", "bigint"].includes(valueType)) {
        throw new TypeError("Subscription state only supports JSON-like scalar values");
      }
      return value;
    }
    const isArray = Array.isArray(value);
    if (!isArray) {
      const prototype = Object.getPrototypeOf(value);
      const isPlainObject = prototype === null || (
        Object.getPrototypeOf(prototype) === null &&
        // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 floor; Object.hasOwn is unavailable and this call is already the safe form
        Object.prototype.hasOwnProperty.call(prototype, "constructor") &&
        prototype.constructor && prototype.constructor.name === "Object"
      );
      if (!isPlainObject) {
        throw new TypeError("Subscription state only supports arrays and plain objects");
      }
    }
    ancestors = ancestors || new WeakSet();
    if (ancestors.has(value)) throw new TypeError("Subscription state must not contain cycles");
    ancestors.add(value);
    try {
      if (isArray) {
        const previousArray = Array.isArray(previous) ? previous : null;
        if (!previousArray) {
          return Object.freeze(value.map(function (item) {
            return subscriptionStateValue(item, undefined, ancestors);
          }));
        }
        let nextArray = value.length === previousArray.length ? null : previousArray.slice(0, value.length);
        for (let arrayIndex = 0; arrayIndex < value.length; arrayIndex++) {
          const nextItem = subscriptionStateValue(value[arrayIndex], previousArray[arrayIndex], ancestors);
          if (!Object.is(nextItem, previousArray[arrayIndex])) {
            if (!nextArray) nextArray = [...previousArray];
            nextArray[arrayIndex] = nextItem;
          }
        }
        return nextArray ? Object.freeze(nextArray) : previousArray;
      }

      const keys = Object.keys(value);
      const previousObject = previous && typeof previous === "object" && !Array.isArray(previous)
        ? previous
        : null;
      const previousKeys = previousObject ? Object.keys(previousObject) : [];
      const sameShape = !!previousObject && keys.length === previousKeys.length && keys.every(function (key) {
        // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 floor; Object.hasOwn is unavailable and this call is already the safe form
        return Object.prototype.hasOwnProperty.call(previousObject, key);
      });
      let nextObject = sameShape ? null : {};
      for (let objectIndex = 0; objectIndex < keys.length; objectIndex++) {
        const key = keys[objectIndex];
        const nextValue = subscriptionStateValue(value[key], previousObject && previousObject[key], ancestors);
        if (!sameShape || !Object.is(nextValue, previousObject[key])) {
          if (!nextObject) nextObject = copySubscriptionStateObject(previousObject);
          defineSubscriptionStateProperty(nextObject, key, nextValue);
        }
      }
      return nextObject ? Object.freeze(nextObject) : previousObject;
    } finally {
      ancestors.delete(value);
    }
  }

  // web+tauriMain 共享（FE14 wave 6）
  function escapeHtml(s) {
    return String(s).replaceAll(/[&<>"']/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c];
    });
  }

  // web+tauriMain 共享（FE14 wave 6）
  function renderMarkdown(text) {
    if (window.PinvouMarkdownRenderer && typeof window.PinvouMarkdownRenderer.renderMarkdown === "function") {
      return window.PinvouMarkdownRenderer.renderMarkdown(text);
    }
    if (window.PinvouMarkdownBridgeFallback && typeof window.PinvouMarkdownBridgeFallback.renderMarkdown === "function") {
      return window.PinvouMarkdownBridgeFallback.renderMarkdown(text);
    }
    return escapeHtml(text || "");
  }

  // web+tauriSessions 共享（FE14 wave 6）
  function stashEvictedSessionDraft(id, buf) {
    if (!id || !buf) return true;
    const draft = String(buf.composerDraft || "");
    if (!draft) {
      delete evictedSessionDrafts.value[id]; // input was cleared; a stale stash must not resurrect it
      return true;
    }
    if (draft.length > MAX_EVICTED_SESSION_DRAFT_CHARS.value) return false;
    if (!evictedSessionDrafts.value[id]
        && Object.keys(evictedSessionDrafts.value).length >= MAX_EVICTED_SESSION_DRAFTS.value) return false;
    delete evictedSessionDrafts.value[id]; // re-stashing moves the entry to the table tail
    evictedSessionDrafts.value[id] = draft;
    return true;
  }

  // web+tauriSessions 共享（FE14 wave 6）
  function freshBuffer() {
    return {
      messages: [], chatItems: [], composerDraft: "", turnTimeline: [], activeTurnTimelineId: null, personaEvents: [], pinvouReviews: [], pinvouSceneEvents: [], artifacts: [], busy: false, queued: [],
      loadedFromDisk: false,
      localTurnOwned: false,
      remoteTurnActive: false,
      remoteTerminalSeen: false,
      remoteAdmissionKeys: [],
      deferredRemoteUserEvent: null,
      remoteBaselineMessageCount: null,
      remoteBaselineTrusted: false,
      remoteExpectedAssistantKey: "",
      remoteCommittedRevision: "",
      sessionRevision: "",
      planSnapshot: { plan: null, todos: null },
      modeState: { mode: "yolo" },
      thinking: { active: false, phase: "thinking", toolName: "", startedAt: 0 },
      tokens: { input: 0, max: state.tokens.max },
      activePersona: null, // 卡片池: 该 session 加持的专家面具(挂件用)
      mountedCollection: null, // 知识库: 该 session 挂载的知识集 id 或 null
      mountedCollections: [], // 多知识库挂载项 [{ collectionId, enabled }]
      mountedCollectionsRevision: 0,
      scheduledRunSession: false,
      scheduledInitialTurnPhase: null,
      lastTouched: 0,

      stream: {
        currentStreamText: "", currentStreamId: 0, pendingAssistantText: "",
        pendingAssistantBlocks: [], itemIdSeq: 0, toolMeta: {},
      },
    };
  }

  // web+tauriSessions 共享（FE14 wave 6）
  function hydratedMessageKey(message, hideInternalEnvelope) {
    let blocks = message && Array.isArray(message.content) ? message.content : [];
    if (message && message.role === "user") {
      const resultIds = blocks.filter(function (block) {
        return block && block.type === "tool_result" && block.tool_use_id;
      }).map(function (block) { return block.tool_use_id; }).sort(function (a, b) { return a < b ? -1 : a > b ? 1 : 0; }); // key normalization needs lexicographic order; declare the semantics explicitly
      if (resultIds.length) return "user:tool_results:" + resultIds.join("|");
      return "user:text:" + userMessageDisplayText(blocks, hideInternalEnvelope);
    }
    if (message && message.role === "assistant") {
      const toolIds = blocks.filter(function (block) {
        return block && block.type === "tool_use" && block.id;
      }).map(function (block) { return block.id; }).sort(function (a, b) { return a < b ? -1 : a > b ? 1 : 0; }); // key normalization needs lexicographic order; declare the semantics explicitly
      if (toolIds.length) return "assistant:tool_uses:" + toolIds.join("|");
      blocks = blocks.filter(function (block) { return !block || block.type !== "thinking"; });
      try { return "assistant:" + JSON.stringify(blocks); } catch { /* persist the raw text on serialization failure */ }
    }
    try { return JSON.stringify(message); } catch { return String(message); }
  }

  // web+tauriSessions 共享（FE14 wave 6）
  function mergeHydratedArtifacts(durableArtifacts, liveArtifacts) {
    const merged = [];
    const seen = Object.create(null);
    [...(durableArtifacts || []), ...(liveArtifacts || [])].forEach(function (artifact) {
      const path = typeof artifact === "string" ? artifact : (artifact && (artifact.path || artifact.storage_path)) || "";
      const identity = basename(path);
      if (!path || !identity) return;
      if (seen[identity] !== undefined) {
        const existingIndex = seen[identity];
        if (isAbsPath(path) && !isAbsPath(merged[existingIndex].path)) {
          merged[existingIndex] = { path, basename: identity };
        }
        return;
      }
      seen[identity] = merged.length;
      merged.push({ path, basename: identity });
    });
    return merged;
  }

  // web+tauriArtifactTracker 共享（FE14 wave 6）
  function parseToolResultPayload(toolResultContent) {
    try {
      const raw = typeof toolResultContent === "string" ? toolResultContent : JSON.stringify(toolResultContent || {});
      const obj = JSON.parse(raw);
      if (obj && obj.content && obj.content[0] && typeof obj.content[0].text === "string") {
        try {
          const inner = JSON.parse(obj.content[0].text);
          if (inner && typeof inner === "object") return inner;
        } catch { /* use the outer object when the inner value is not JSON */ }
      }
      return obj;
    } catch {
      return null;
    }
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
  function parseLooseJsonObject(text) {
    try { return JSON.parse(text); } catch { /* invalid JSON: the caller falls back to the raw text */ }
    try { return JSON.parse(String(text || "").replaceAll(/,(\s*[}\]])/g, "$1")); } catch { /* invalid JSON: the caller falls back to the raw text */ }
    const balanced = extractBalancedJsonObject(String(text || ""));
    if (!balanced) return null;
    try { return JSON.parse(balanced); } catch { /* invalid JSON: the caller falls back to the raw text */ }
    try { return JSON.parse(balanced.replaceAll(/,(\s*[}\]])/g, "$1")); } catch { /* invalid JSON: the caller falls back to the raw text */ }
    return null;
  }

  // web+tauriScheduled 共享
  function parseScheduledTaskDraftFromText(text) {
    if (!text || !text.includes("{")) return null;
    let preferred = null;
    let fallback = null;
    const re = /```([^\n`]*)\n([\s\S]*?)```/g;
    let match;
    // biome-ignore lint/suspicious/noAssignInExpressions: the assignment doubles as the loop condition; refactoring would hurt readability
    while ((match = re.exec(text))) {
      const label = String(match[1] || "").trim().toLowerCase();
      const raw = String(match[2] || "").trim();
      if (!raw || raw.charAt(0) !== "{") continue;
      const candidate = normalizeScheduledTaskDraft(parseLooseJsonObject(raw));
      if (!candidate) continue;
      if (label === "scheduled-task-draft") return candidate;
      if ((label === "json" || !label) && !fallback) fallback = candidate;
      if (!preferred) preferred = candidate;
    }
    return fallback || preferred;
  }

  // web+tauriScheduled 共享
  // 聊天创建拿到合法参数后立即落成任务。草稿不会进入可渲染 state，避免再出现一层确认卡。
  // autoOpenId 全局 last-writer：两会话并发创建时后完成者覆盖，且
  // startScheduledTaskChat 清空后陈旧 completion 会复活 auto-open（审计 f）。
  // 全局单调创建序号，仅最新意图可写。
  function autoCreateScheduledTaskDraft(draft, creationSessionId) {
    if (!draft || !creationSessionId || scheduledTaskAutoCreateInFlight.value[creationSessionId]) return;
    const lockedDraft = lockScheduledTaskDraftModel(draft);
    const creationSeq = ++scheduledTaskAutoCreateSeq.value;
    const creation = Promise.resolve()
      .then(function () {
        return createScheduledTask(scheduledTaskInputFromDraft(lockedDraft));
      })
      .then(function (created) {
        if (state.scheduledTaskCreationSessionId === creationSessionId) {
          state.scheduledTaskCreationSessionId = null;
        }
        if (created && created.id && creationSeq === scheduledTaskAutoCreateSeq.value) state.scheduledTaskAutoOpenId = created.id;
        notify();
        return created;
      })
      .catch(function (error) {
        // createScheduledTask 通常已记录错误；忙锁在进入 action 前抛出时在这里补记，且不产生未处理 Promise。
        if (!state.scheduledTaskError) setScheduledTaskError(error, "action");
        runSyncOnSession(creationSessionId, function () {
          addSystemItem(bt("scheduledCreateFailed") + scheduledTaskErrorText(error), {
            scheduledTaskCreationError: true,
          });
        });
        notify();
        return null;
      })
      .finally(function () {
        if (scheduledTaskAutoCreateInFlight.value[creationSessionId] === creation) {
          delete scheduledTaskAutoCreateInFlight.value[creationSessionId];
        }
      });
    scheduledTaskAutoCreateInFlight.value[creationSessionId] = creation;
  }

  // web+tauriScheduled 共享
  async function createScheduledTask(input) {
    return runScheduledTaskAction("create", async function () {
      const templateId = input && typeof input.templateId === "string" ? input.templateId.trim() : "";
      // kind is create-time task metadata (currently only "memory_organize"; the
      // backend rejects anything else). It deliberately stays out of
      // SCHEDULED_TASK_WRITABLE_FIELDS so edit flows can never resend it.
      const kind = input && typeof input.kind === "string" ? input.kind.trim() : "";
      const selectAfterCreate = !input || input.selectAfterCreate !== false;
      const backendInput = scheduledTaskBackendInput(input);
      if (kind) backendInput.kind = kind;
      const created = await invoke("create_scheduled_task", { input: backendInput });
      if (!created || !created.id) {
        throw new Error(bt("scheduledCreateNoId"));
      }
      if (templateId) rememberScheduledTaskTemplateSource(created.id, templateId);
      attachScheduledTaskTemplateSource(created);
      // 立即重拉任务列表:新 stamp 会使创建前仍在途的 list_scheduled_tasks 响应失效,
      // 防止旧结果落地时把刚创建的任务从列表里覆盖掉。
      await loadScheduledTasks();
      upsertScheduledTask(created);
      if (selectAfterCreate) selectScheduledTask(created.id);
      if (selectAfterCreate) state.scheduledTaskDetail = created;
      notify();
      return created;
    });
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

  // web+tauriScheduled shared
  // Upsert one run row into the recent-runs working set: replace by id or
  // append, then drop invalid rows (no session / archived) and keep the list
  // sorted by scheduledFor (fallback createdAt) descending. Returns a new array.
  function upsertRecentRunRow(rows, run) {
    const next = [...(rows || [])];
    const index = next.findIndex(function (row) { return row && row.id === run.id; });
    if (index >= 0) next[index] = run;
    else next.push(run);
    next.sort(function (a, b) {
      return new Date(b.scheduledFor || b.createdAt || 0).getTime() -
        new Date(a.scheduledFor || a.createdAt || 0).getTime();
    });
    return next.filter(function (row) { return row && row.sessionId && !row.archived; });
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
      rows = upsertRecentRunRow(rows, merged);
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
      let rows = [];
      (Array.isArray(runs) ? runs : []).forEach(function (run) {
        if (!run) return;
        rememberScheduledRunOwner(run);
        const automationId = run.automationId || run.automation_id;
        const task = tasksById[automationId] || null;
        rows = upsertRecentRunRow(rows, Object.assign({}, run, {
          automationId,
          taskName: task && task.name || run.taskName || bt("scheduledTaskFallbackName"),
          taskModel: task && task.model || run.taskModel || null,
        }));
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

  // web+tauriChat 共享（FE14 残余批：原 tauri chat.js / web bridge.js 逐字镜像，正文取自 web 侧）
  // Replayed events belong to the current tail in practice. Scan backwards so
  // long-running Sessions do not pay a full-history walk on every tool frame.
  // biome-ignore lint/suspicious/noFunctionAssign: forwarder-dep routing reassigns the shared impl when the lane passes its own (see FORWARDER_DEP_NAMES)
  function messageHasToolBlock(type, toolCallId) {
    if (!toolCallId) return false;
    for (let i = state.messages.length - 1; i >= 0; i--) {
      const blocks = state.messages[i] && state.messages[i].content;
      if (!Array.isArray(blocks)) continue;
      for (let j = blocks.length - 1; j >= 0; j--) {
        const block = blocks[j];
        if (!block || block.type !== type) continue;
        if ((type === "tool_use" ? block.id : block.tool_use_id) === toolCallId) return true;
      }
    }
    return false;
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

  // web+tauriChat 共享（DI：pendingAssistantBlocks 由两条 lane 以 get/set 单元格传入）
  function toolCallAlreadyStarted(toolCallId) {
    if (!toolCallId) return false;
    if ((pendingAssistantBlocks.value || []).some(function (block) {
      return block && block.type === "tool_use" && block.id === toolCallId;
    })) return true;
    if (state.chatItems.some(function (item) {
      return item && item.type === "tool" && item.toolId === toolCallId;
    })) return true;
    return messageHasToolBlock("tool_use", toolCallId);
  }

  // web+tauriChat 共享（DI：itemIdSeq 由两条 lane 以 get/set 单元格传入）
  async function sendMessageToSession(sessionId, text, meta) {
    const sid = String(sessionId || "").trim();
    const content = String(text || "").trim();
    if (!sid) throw new Error(bt("targetSessionMissing"));
    if (!content) throw new Error(bt("replyContentEmpty"));
    const exists = state.sessions.some(function (session) { return String(session.id) === sid; });
    if (!exists) throw new Error(bt("targetSessionMissing"));

    await ensureSessionBufferLoaded(sid);
    let targetBuffer = getBuffer(sid);
    const targetQueue = targetBuffer && targetBuffer.queued;
    if (isBusyFor(sid) || (targetQueue && targetQueue.length > 0)) {
      runSyncOnSession(sid, function () {
        state.queued.push(makeQueuedMessage(
          ++itemIdSeq.value, content, content, content, [], meta || null, false
        ));
      });
      notify();
      if (!isBusyFor(sid)) flushQueued(sid);
      return { accepted: true, queued: true };
    }
    if (targetBuffer && targetBuffer.remoteTurnActive && !(await reconcileRemoteTurn(sid))) {
      recordAuthoritySyncDiagnostic("remote_sync_blocked_action", Object.assign({
        operation: "send_to_session",
      }, authoritySyncBufferSnapshot(sid, targetBuffer)));
      throw new Error(bt("targetSessionSyncing"));
    }
    targetBuffer = getBuffer(sid);
    if (isBusyFor(sid) || (targetBuffer.queued && targetBuffer.queued.length > 0)) {
      runSyncOnSession(sid, function () {
        state.queued.push(makeQueuedMessage(
          ++itemIdSeq.value, content, content, content, [], meta || null, false
        ));
      });
      notify();
      if (!isBusyFor(sid)) flushQueued(sid);
      return { accepted: true, queued: true };
    }
    const completion = doSendFor(sid, content, content, [], meta || null, false, true)
      .then(
        function () { return { ok: true }; },
        function (error) { return { ok: false, error }; }
      );
    return { accepted: true, queued: false, completion };
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
  // session:deleted fan-out: applyDeletedSession is the ONE path that
  // observes every deleted id — list_sessions excludes code-mode sessions
  // (they are listed by list_codex_acp_sessions), so a slice-diff consumer
  // never learns a code task was deleted. Shared code cannot import feature
  // modules, so the feature-layer consumer (the aux-chat purge wiring)
  // registers a listener here instead of the shared side pushing.
  const sessionDeletedListeners = new Set();
  function onSessionDeleted(listener) {
    if (typeof listener !== "function") return function () {};
    sessionDeletedListeners.add(listener);
    return function () { sessionDeletedListeners.delete(listener); };
  }
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
    for (const listener of sessionDeletedListeners) {
      try { listener(id); } catch (error) { console.warn("session:deleted listener failed", error); }
    }
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

  // tauriSessions shared: recent draft-state workspace directory choices (a mirror of
  // shared/workspace-recents.js on the classic-script side; the two cannot import each other, changes must be synced).
  const DRAFT_WORKSPACE_RECENTS_KEY = "pinvou_codex_recent_workspaces";
  function readDraftWorkspaceRecents() {
    try {
      const value = JSON.parse(localStorage.getItem(DRAFT_WORKSPACE_RECENTS_KEY) || "[]");
      return Array.isArray(value) ? value.filter(function (item) { return typeof item === "string"; }) : [];
    } catch {
      return [];
    }
  }
  function writeDraftWorkspaceRecents(list) {
    try {
      localStorage.setItem(DRAFT_WORKSPACE_RECENTS_KEY, JSON.stringify(list));
    } catch {
      // When localStorage is unavailable, skip recording just this one entry;
      // the directory selection itself is unaffected.
    }
  }
  function rememberDraftWorkspaceRecent(path) {
    const list = readDraftWorkspaceRecents().slice(0, 6);
    const next = [path, ...list.filter(function (item) { return item !== path; })].slice(0, 6);
    writeDraftWorkspaceRecents(next);
  }
  // Remove a single directory from the recents list (mirror of
  // workspace-recents.js forgetWorkspace): called from the ensureSession failure
  // path when materialization fails (directory deleted/renamed) so a bad entry
  // no longer lingers forever.
  function forgetDraftWorkspaceRecent(path) {
    writeDraftWorkspaceRecents(readDraftWorkspaceRecents().filter(function (item) { return item !== path; }));
  }
  // Recents-list cleanup policy after create_session fails: prune only when the
  // backend explicitly rejected the path (invalid workspace_path — the directory
  // is stale or deleted); transient errors must not remove valid entries. Prune
  // by the boundWorkspace captured at materialization, not the live
  // draftWorkspacePath — if the user re-picks X2 while X1's creation is in
  // flight, X1 is the one that fails and the valid X2 must survive.
  function maybePruneFailedWorkspaceRecent(boundWorkspace, error) {
    if (!boundWorkspace) return;
    if (!/invalid workspace_path/.test(String((error && error.message) || error || ''))) return;
    forgetDraftWorkspaceRecent(boundWorkspace);
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
    // UNC network-share paths (\\server\\share\...) are absolute too —
    // validate_codex_project_workspace stores them after platform_compat_path,
    // and without this arm the rebase transform returned them unmapped
    // (review #463 round-20 minor 6; Windows network-share workspaces only).
    return (
      typeof p === "string"
        && (p.charAt(0) === "/" || /^[A-Za-z]:[\\/]/.test(p) || /^\\\\[^\\]/.test(p))
    );
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
          // only in case are indistinguishable HERE while the POSIX backend
          // identity key stays case-sensitive — the fold is this transform's
          // own trade-off (the backend folds unconditionally only on
          // Windows); reworded per round-24 minor 15, capability-gating
          // stays the recorded later option.
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
  // Round-24 minor 3: the view-heal take-over is basename-keyed, so inside
  // the freshness window an unrelated LIVE absolute entry whose basename
  // collides with a workspace deliverable would be taken over and the wrong
  // path persisted. An entry may only be healed when its stored path is
  // actually stale rebind geometry — i.e. the session's segment chain maps
  // it somewhere else (the same prefix test rebaseArtifactPathsForRebind
  // applies on save). Relative paths are never stale geometry here: they
  // resolve against the current workspace and are owned by the
  // relative→absolute escape hatch.
  function pathIsRebindStale(sid, path) {
    if (typeof path !== "string" || !isAbsPath(path)) return false;
    const mapped = rebaseArtifactPathsForRebind(sid, [path]);
    // Round-24 minor 14: the transform returns separator-NORMALIZED paths,
    // so compare in the normalized domain — a raw entry with mixed or
    // backslash separators would otherwise differ as a string and classify
    // stale, letting a live same-basename entry take over inside the
    // freshness window.
    return Array.isArray(mapped) && normalizedPath(mapped[0]) !== normalizedPath(path);
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
  // biome-ignore lint/suspicious/noFunctionAssign: forwarder-dep routing reassigns the shared impl when the lane passes its own (see FORWARDER_DEP_NAMES)
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
  function sessionMentionSplitter() {
    return (typeof window !== "undefined") && window.__PINVOU_SESSION_MENTION__ &&
      window.__PINVOU_SESSION_MENTION__.splitSessionMentionBlock;
  }

  // Shared by web + tauriChat. Composer restores (steer-failure, session-switch
  // mid-send, materialize abort) hand text back to the user: the
  // session-mention injection block is a machine contract and the chips were
  // consumed by the send attempt, so the block is stripped on restore instead
  // of leaking raw JSON into the input. One shared implementation for both
  // platform lanes (round-8 minor 10 — the strip-on-restore rule used to live
  // as two verbatim lane copies behind the window global); the splitter is
  // still read lazily at call time because features publish it after the
  // bridge scripts boot.
  function stripMentionBlockForComposerRestore(text) {
    const raw = String(text || "");
    const splitMention = sessionMentionSplitter();
    if (typeof splitMention !== "function") return raw;
    const split = splitMention(raw);
    // Gate on `matched`, not refs.length: a structurally valid block that
    // parses to zero refs is still a block (the Rust titler and the bubble
    // strip both treat it as one) — the restores must not hand the raw JSON
    // contract back for it.
    return split && split.matched ? split.text.trim() : raw;
  }

  // Shared by web + tauriChat. Block-aware part of queuedPayloadEnvelope: splits
  // <block><body> user text against a payload that really carries the block
  // at its head. Returns null when the shape does not apply (no parser,
  // no refs, refs-only text, prefixed payload, ambiguous body anchor).
  function blockAwarePayloadEnvelope(user, payload) {
    const splitMention = sessionMentionSplitter();
    if (typeof splitMention !== "function") return null;
    const split = splitMention(user);
    if (!split || !split.refs.length || !split.text) return null;
    const blockText = user.slice(0, user.length - split.text.length);
    // The block-aware shape only exists when the queued payload really
    // carries the block at its head: a guide/scaffold-prefixed payload
    // anchors the block later, and slicing from blockText.length then
    // cuts into the middle of the original block's JSON (round-8 M2).
    // The body anchor is the TRIMMED body — interior leading whitespace
    // in the stored body survives the whole-string trim, so the
    // untrimmed form never matched the payload's embed and made the
    // queued edit deterministically impossible. Anything else falls
    // back to the legacy refusal instead of a corrupt envelope.
    const body = split.text.trim();
    if (!body || !payload.startsWith(blockText)) return null;
    // Anchor on the FIRST occurrence after the block, and only when it
    // is unique — the legacy exact-match path refused ambiguous
    // payloads, and a body string that also occurs inside the template
    // scaffold would silently land the edit at the boilerplate spot.
    const bodyIndex = payload.indexOf(body, blockText.length);
    if (bodyIndex < blockText.length || payload.includes(body, bodyIndex + 1)) return null;
    return {
      blockPrefix: blockText,
      before: payload.slice(blockText.length, bodyIndex),
      after: payload.slice(bodyIndex + body.length),
      blockAware: true,
    };
  }

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
    if (index < 0) {
      // Session-mention block-aware fallback: a queued scene send stores the
      // text as <block><body> while a scene payload embeds the body inside
      // its template (the personal workbench default prompt sandwiches it as
      // `PROMPT…\n用户需求：\n<body>`), so the literal substring match above
      // fails. Split the envelope into the block prefix, the scaffold between
      // block and body, and the tail: the rebuild swaps in the EDITED block
      // and body (rebuiltQueuedFromEnvelope) so the payload keeps the
      // send-time shape — block at the head, body at its original anchor —
      // with the edited refs honored instead of the queued ones.
      const blockEnvelope = blockAwarePayloadEnvelope(user, payload);
      if (blockEnvelope) return blockEnvelope;
      return payload === user ? { before: "", after: "" } : null;
    }
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
  function rebuiltQueuedFromEnvelope(envelope, userText) {
    const raw = String(userText == null ? "" : userText);
    if (envelope.blockAware) {
      const splitMention = sessionMentionSplitter();
      const split = typeof splitMention === "function" ? splitMention(raw) : null;
      let block = "";
      let body = raw;
      if (split && split.matched) {
        block = raw.slice(0, raw.length - split.text.length);
        body = split.text;
      }
      return block + envelope.before + body + envelope.after;
    }
    return envelope.before + raw + envelope.after;
  }

  function rebuiltQueuedPayload(item, userText) {
    const envelope = item && item.payloadEnvelope;
    if (!envelope || typeof envelope.before !== "string" || typeof envelope.after !== "string") return null;
    return rebuiltQueuedFromEnvelope(envelope, userText);
  }

  // web+tauriChat 共享
  function rebuiltQueuedMetaPayload(item, userText) {
    const envelope = item && item.metaPayloadEnvelope;
    if (!envelope || typeof envelope.before !== "string" || typeof envelope.after !== "string") return null;
    return rebuiltQueuedFromEnvelope(envelope, userText);
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

  // web+tauriMonitor 共享
  // The shared format-utils script is loaded statically; the inline fallbacks
  // are verbatim copies of its bodies so the monitor never falls back to raw
  // numbers when the shared script is missing.
  const PinvouFU = window.PinvouFormatUtils || {};
  const fmtMiB = PinvouFU.fmtMiB || function (mib) {
    if (mib == null) return "—";
    return mib >= 1024 ? (mib / 1024).toFixed(1) + " GB" : mib + " MB";
  };
  const fmtKiB = PinvouFU.fmtKiB || function (kib) {
    if (kib == null) return "—";
    if (kib >= 1024 * 1024) return (kib / 1024 / 1024).toFixed(1) + " GB";
    if (kib >= 1024) return (kib / 1024).toFixed(0) + " MB";
    return kib + " KB";
  };
  const fmtDuration = PinvouFU.fmtDuration || function (secs) { return secs == null ? "—" : String(secs); };
  const fmtTok = PinvouFU.fmtTok || function (n) { return n == null ? "—" : String(n); };

  // web+tauriMonitor 共享
  function appQueueSnapshot() {
    let waiting = state.queued ? state.queued.length : 0;
    const busyMap = {};
    for (const id in sessionStates.value) {
      // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 is the floor and Object.hasOwn is unavailable; this call is already the safe form
      if (!Object.prototype.hasOwnProperty.call(sessionStates.value, id)) continue;
      if (id === state.activeSessionId) continue;
      const buf = sessionStates.value[id] || {};
      if (buf.busy) busyMap[id] = true;
      if (Array.isArray(buf.queued)) waiting += buf.queued.length;
    }
    if (state.activeSessionId && state.busy) busyMap[state.activeSessionId] = true;
    const running = Object.keys(busyMap).length;
    return { running, waiting };
  }

  // web+tauriMonitor 共享
  // MonitorView only consumes state.monitor._fmt (plus one level of vllmRaw
  // nesting), so shallow equality means display equivalence. The poll runs
  // once per second; when a snapshot is display-equivalent, skip the
  // assignment + notify to avoid a full App re-render every second while the
  // monitor page is open. Numeric values jitter naturally (cpu/gpu
  // percentages etc.), so comparisons allow a 0.5 tolerance (prefer one
  // extra notify over ever getting stuck); counters are mostly strings after
  // toFixed/round and compare exactly. The page clock is driven by
  // MonitorView's local 1s timer and does not depend on any poll-tick marker.
  function monitorFmtEqual(prev, next) {
    if (prev === next) return true;
    if (!prev || !next) return false;
    const numEq = function (a, b) {
      if (a === b) return true;
      return typeof a === "number" && typeof b === "number"
        && Number.isFinite(a) && Number.isFinite(b)
        && Math.abs(a - b) <= 0.5;
    };
    const keys = Object.keys(next);
    if (keys.length !== Object.keys(prev).length) return false;
    for (let i = 0; i < keys.length; i++) {
      const key = keys[i];
      const a = prev[key];
      const b = next[key];
      if (a && b && typeof a === "object" && typeof b === "object") {
        const inner = Object.keys(b);
        if (inner.length !== Object.keys(a).length) return false;
        let same = true;
        for (let j = 0; j < inner.length; j++) {
          if (!numEq(a[inner[j]], b[inner[j]])) { same = false; break; }
        }
        if (!same) return false;
        continue;
      }
      if (!numEq(a, b)) return false;
    }
    return true;
  }

  // web+tauriMonitor 共享
  // eslint-disable-next-line sonarjs/cognitive-complexity -- legacy bridge; refactor tracked separately
  async function pollMonitor() {
    if (monitorPollInFlight.value) return;
    monitorPollInFlight.value = true;
    try {
      const snap = await invoke("get_monitor_snapshot");
      // The previous round errored → this round must notify once so the
      // "read failed" banner switches back to the normal panel.
      const hadMonitorError = !!state.monitorError;
      state.monitorError = null;
      // GPU util sliding window
      if (snap.gpu) {
        gpuUtilHistory.value.push(snap.gpu.utilization_pct);
        if (gpuUtilHistory.value.length > 5) gpuUtilHistory.value.shift();
        snap.gpu._utilMax = Math.max(...[0, ...gpuUtilHistory.value]);
      }
      // 监控页「后 4 项」累计指标：TTFT/TPS/tokens 来自 app 侧自测(snap.self_perf,
      // 任何后端都有);KV 混合(本地 vLLM prefix_cache 优先,否则 usage 口径)。
      // 按「清除统计」基准点换算成区间值后再格式化。
      const sadj = adjustCounters(snap.self_perf, snap.vllm);
      // KV 显示值:本地 vLLM 的 /metrics prefix_cache 优先,拿不到用 usage cache 口径(云端)。
      const kvShown = sadj ? (sadj.vllmKvPct == null ? (sadj.selfKvPct == null ? null : sadj.selfKvPct)
        : sadj.vllmKvPct) : null;
      // Format values for display
      const vllm = snap.vllm || null;
      const metricsApplicable = vllm ? vllm.metrics_applicable !== false : false;
      const metricUnavailableText = bt("metricUnavailable");
      const targetKind = vllm && vllm.target_kind ? vllm.target_kind : "invalid";
      const vllmDisplayModel = vllm ? (vllm.model || vllm.configured_model || "—") : "—";
      const healthStatus = vllm && vllm.health_status ? vllm.health_status : (vllm ? "verified" : "offline");
      const appQueue = appQueueSnapshot();
      const cpu = snap.cpu || null;
      const cpuUsage = cpu && typeof cpu.total_usage_pct === "number" && Number.isFinite(cpu.total_usage_pct)
        ? Math.round(Math.max(0, Math.min(100, cpu.total_usage_pct)))
        : null;
      const computeName = snap.gpu ? snap.gpu.name : (cpu && cpu.name ? cpu.name : bt("gpuUnavailable"));
      snap._fmt = {
        gpuName: computeName,
        cpuName: cpu && cpu.name ? cpu.name : "",
        cpuAvailable: !!cpu,
        computeAvailable: !!(snap.gpu || cpu),
        computeName,
        gpuVramPct: snap.gpu && snap.gpu.vram_total_mib > 0
          ? Math.round(snap.gpu.vram_used_mib / snap.gpu.vram_total_mib * 100) : 0,
        gpuUtilPct: snap.gpu ? snap.gpu._utilMax : 0,
        processorUtilPct: cpuUsage == null
          ? (snap.gpu && snap.gpu.processor_utilization_pct != null ? snap.gpu.processor_utilization_pct : 0)
          : cpuUsage,
        gpuSharedMemory: snap.gpu && snap.gpu.shared_memory_used_mib != null ? fmtMiB(snap.gpu.shared_memory_used_mib) : "—",
        gpuTemp: snap.gpu && snap.gpu.temperature_c != null ? snap.gpu.temperature_c + "°C" : null,
        gpuPower: snap.gpu && snap.gpu.power_w != null ? snap.gpu.power_w.toFixed(1) + " W" : null,
        gpuAvailable: !!snap.gpu,
        gpuHasVram: !!(snap.gpu && snap.gpu.vram_total_mib > 0),
        ramTotal: snap.ram ? fmtKiB(snap.ram.total_kib) : "—",
        ramPct: snap.ram && snap.ram.total_kib > 0 ? Math.round(snap.ram.used_kib / snap.ram.total_kib * 100) : 0,
        ramUsedGiB: snap.ram ? (snap.ram.used_kib / 1024 / 1024).toFixed(1) : "—",
        swapUsed: snap.ram ? fmtKiB(snap.ram.swap_used_kib) : "—",
        swapTotal: snap.ram ? fmtKiB(snap.ram.swap_total_kib) : "—",
        swapPct: snap.ram && snap.ram.swap_total_kib > 0 ? Math.round(snap.ram.swap_used_kib / snap.ram.swap_total_kib * 100) : 0,
        vllmModel: vllmDisplayModel,
        vllmHealthStatus: healthStatus,
        vllmOnline: vllm ? (healthStatus === "verified" && (vllm.status === "ready" || vllm.status === "busy")) : false,
        // 云端(remote)不做健康探测(无 auth 的 /v1/models 必 401)→ 不显示 OFFLINE。
        // 暴露原始 kind 供前端判定(别比本地化 label)。
        vllmIsRemote: targetKind === "remote",
        vllmMaxLen: vllm ? (metricsApplicable ? (vllm.max_model_len || "—") : (vllm.max_model_len || metricUnavailableText)) : "—",
        // 本地推理引擎(target_kind=local)且探测窗口 < 128k(131072):监控卡给告警。
        // 云端(remote)/v1/models 不返回 max_model_len,自然不触发。传原始值供前端拼文案。
        vllmQueue: appQueue.running + " / " + appQueue.waiting,
        // TTFT/TPS/tokens 一律用 app 侧自测——任何后端(vLLM/LM Studio/Ollama/云端)都有值,
        // 不再受 metricsApplicable 门控。KV 见 kvShown(本地 prefix_cache / 云端 usage 口径),
        // 拿不到则 "—"。队列仍归 vLLM(见 vllmQueue)。
        vllmKv: kvShown == null ? "0%" : kvShown.toFixed(1) + "%",
        vllmKvHasData: kvShown != null,
        vllmTtft: sadj && sadj.ttft_count > 0
          ? (sadj.ttft_sum_s / sadj.ttft_count).toFixed(2) + " s" : "—",
        vllmTps: sadj && sadj.tps_time_s > 0
          ? (sadj.tps_tokens / sadj.tps_time_s).toFixed(1) + " tok/s" : "—",
        vllmTokTotal: sadj
          ? fmtTok(sadj.gen) + " / " + fmtTok(sadj.prompt) : "—",
        // 区间原始数值（已扣基准），供前端「长按清除」的数字归零插值动画用。
        vllmRaw: sadj ? {
          kvPct: kvShown,
          ttftS: sadj.ttft_count > 0 ? sadj.ttft_sum_s / sadj.ttft_count : null,
          tps: sadj.tps_time_s > 0 ? sadj.tps_tokens / sadj.tps_time_s : null,
          gen: sadj.gen == null ? null : sadj.gen,
          prompt: sadj.prompt == null ? null : sadj.prompt,
        } : null,
        appVersion: snap.app ? snap.app.pinvou3_version + bt("betaVersionSuffix") : "—",
        uptime: snap.app ? fmtDuration(snap.app.session_uptime_secs) : "—",
      };
      if (snap.vllm && snap.vllm.max_model_len) {
        state.tokens.max = snap.vllm.max_model_len;
      }
      // Display-equivalent snapshots neither overwrite state.monitor nor
      // notify (must send on the first frame or after an errored round).
      const prevFmt = state.monitor && state.monitor._fmt;
      if (hadMonitorError || !prevFmt || !monitorFmtEqual(prevFmt, snap._fmt)) {
        state.monitor = snap;
        notify();
      }
    } catch (e) {
      state.monitorError = e && e.message ? e.message : String(e || "monitor poll failed");
      console.warn("monitor poll failed", e);
      notify();
    } finally {
      monitorPollInFlight.value = false;
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
  async function loadEffectiveModelConfig(sessionId) {
    const requestedSessionId = arguments.length ? (sessionId || null) : (state.activeSessionId || null);
    try {
      const config = await invoke("get_effective_model_config", { sessionId: requestedSessionId });
      // When switching sessions quickly, an older request may return after the new one; never let the old session's config overwrite the current overlay state.
      if ((state.activeSessionId || null) !== requestedSessionId) return;
      state.effectiveModelConfig = config;
    } catch {
      if ((state.activeSessionId || null) !== requestedSessionId) return;
      state.effectiveModelConfig = null;
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

  // web+tauriMemory 共享
  function applyMemoryOverview(overview) {
    const previous = state.memory || {};
    const sourceStates = overview && overview.sources || {};
    // stateKey:后端 source 名与前端 state 字段名通常一致,但 snapshot 源对应
    // state.memory.snapshot_path,两者不同;保留上次值时按 state 字段名查找。
    function sourceValue(source, value, fallback, stateKey) {
      const status = sourceStates[source];
      if (status && status.available === false) {
        const key = stateKey || source;
        // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 is the floor and Object.hasOwn is unavailable; this call is already the safe form
        return Object.prototype.hasOwnProperty.call(previous, key) ? previous[key] : fallback;
      }
      return value;
    }
    state.memory = {
      loading: false,
      error: null,
      profile: sourceValue("profile", overview && overview.profile || null, null),
      preferences: sourceValue("preferences", overview && Array.isArray(overview.preferences) ? overview.preferences : [], []),
      work_context: sourceValue("work_context", overview && Array.isArray(overview.work_context) ? overview.work_context : [], []),
      current_focus: sourceValue("current_focus", overview && Array.isArray(overview.current_focus) ? overview.current_focus : [], []),
      recent_activity: sourceValue("recent_activity", overview && Array.isArray(overview.recent_activity) ? overview.recent_activity : [], []),
      pending: sourceValue("pending", overview && Array.isArray(overview.pending) ? overview.pending : [], []),
      runtime: sourceValue("runtime", overview && overview.runtime || null, null),
      snapshot_path: sourceValue("snapshot", overview && overview.snapshot_path || "", "", "snapshot_path"),
      warnings: orderedMemoryWarnings(overview && overview.warnings),
      sources: sourceStates,
    };
  }

  // web+tauriMemory 共享
  // 记忆面板混合两类数据：runtime 按 session 分文件，profile/preferences/
  // pending 等为全局单文件(见后端 paths.rs)。加载仍必须带归属+序号校验：
  // await 挂起期间切会话或再次加载，旧响应返回后不得覆盖当前显示(尤其
  // runtime 属于别的会话)，也不得把候选卡 rehydrate 进当前对话流(串台)。
  // 任何新加载都会递增序号使在途读取作废(审计)。
  async function loadMemoryOverview(options) {
    if (!invoke) return null;
    options = options || {};
    const sid = state.activeSessionId;
    const seq = ++memoryOverviewSeq.value;
    state.memory = Object.assign({}, state.memory, { loading: true, error: null });
    notify();
    try {
      // invoke 形状保持原样（协议指纹按文本计算）；发起瞬间 activeSessionId === sid。
      const overview = await invoke("get_memory_overview", { sessionId: state.activeSessionId });
      if (sid !== state.activeSessionId || seq !== memoryOverviewSeq.value) return discardStaleLoad(seq);
      applyMemoryOverview(overview);
      if (options.rehydratePending) rehydratePendingMemoryCandidates(overview);
      notify();
      return overview;
    } catch (e) {
      if (sid !== state.activeSessionId || seq !== memoryOverviewSeq.value) return discardStaleLoad(seq);
      state.memory = Object.assign({}, state.memory, { loading: false, error: String(e) });
      notify();
      return null;
    }
  }

  // web+tauriMemory 共享
  async function updateMemoryItem(kind, id, patch) {
    if (!id || !invoke) return null;
    const sid = state.activeSessionId; // same as saveMemoryProfilePatch: after switching away, never write to B's panel (audit follow-up)
    try {
      const command = kind === "preference" ? "update_memory_preference"
        : kind === "work_context" ? "update_work_context_memory"
        : (kind === "current_focus" || kind === "recent_activity") ? "update_timed_memory"
        : null;
      if (!command) return null;
      const args = { id, patch: patch || {}, sessionId: state.activeSessionId };
      if (command === "update_timed_memory") args.kind = kind;
      const res = await invoke(command, args);
      if (sid === state.activeSessionId) {
        applyMemoryWriteState(res, function (next, value) {
          if (!value) return;
          const source = kind === "preference" ? "preferences" : kind;
          next[source] = upsertMemoryValue(next[source], value, id);
        });
      }
      await loadMemoryOverview();
      return res && res.value;
    } catch (e) {
      if (sid === state.activeSessionId) {
        state.memory = Object.assign({}, state.memory, { error: String(e) });
        notify();
      }
      throw e;
    }
  }

  // web+tauriMemory 共享
  async function deleteMemoryItem(kind, id) {
    if (!id || !invoke) return false;
    const sid = state.activeSessionId; // same as saveMemoryProfilePatch: after switching away, never write to B's panel (audit follow-up)
    try {
      const command = kind === "preference" ? "delete_memory_preference"
        : kind === "work_context" ? "delete_work_context_memory"
        : (kind === "current_focus" || kind === "recent_activity") ? "delete_timed_memory"
        : null;
      if (!command) return false;
      const args = { id, sessionId: state.activeSessionId };
      if (command === "delete_timed_memory") args.kind = kind;
      const res = await invoke(command, args);
      if (sid === state.activeSessionId) {
        applyMemoryWriteState(res, function (next, changed) {
          if (!changed) return;
          const source = kind === "preference" ? "preferences" : kind;
          next[source] = (next[source] || []).filter(function (item) { return item.id !== id; });
        });
      }
      await loadMemoryOverview();
      return !!(res && res.value);
    } catch (e) {
      if (sid === state.activeSessionId) {
        state.memory = Object.assign({}, state.memory, { error: String(e) });
        notify();
      }
      throw e;
    }
  }

  // candidateCardPatcher = lane 的 patchItemByIdFor（deps 以非碰撞名注入，
  // 生产路径两侧分别是 tauriInteraction/web 的原实现——与 auxChatDispatch 同一
  // 注入范式，测试桩也得以保留观测点）。
  // web+tauriMemory 共享（F-XT7-6：confirm/ignore/never 是同一个可参数化流程，
  // 两侧各留三个 2 行包装；bodies 原为 ×3×2 lanes 近逐字副本）。
  async function resolveMemoryCandidate(command, args, chatItemId, statusLabel, errorKey) {
    if (!args || !args.id) return;
    const sid = state.activeSessionId; // captured at entry: both the candidate-card patch and panel writes route back to the originating session (audit follow-up)
    try {
      const result = await invoke(command, args);
      if (sid === state.activeSessionId) {
        applyMemoryWriteState(result, function (next) {
          next.pending = (next.pending || []).filter(function (item) { return item.id !== args.id; });
        });
      }
      // patch 必须按发起会话路由(而非当前显示)：切走后写 B 的 chatItems 是
      // no-op，A 的候选卡会永远停留在"可点击未决"态，切回再点会二次提交。
      if (chatItemId) candidateCardPatcher(sid, chatItemId, { resolved: true, statusLabel });
      await loadMemoryOverview();
      notify();
    } catch (e) {
      if (sid === state.activeSessionId) addSystemItem(bt(errorKey) + e);
    }
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
    return p.name || "";
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
  // 给当前 session 加持一张专家面具。后端存 persona_id + 每 turn 注入人设;
  // 前端记 activePersona(挂件) + 发一条系统消息播报。
  async function equipPersona(personaId) {
    if (!state.activeSessionId) {
      // 草稿态加卡 → 先物化 session(lazy session)。用返回值判空：切走场景
      // ensureSession 返回 null 但 activeSessionId 非空，会把卡加进新会话。
      const materialized = await ensureSession();
      if (!materialized) return; // 物化失败/切走,放弃
    }
    // 入口捕获触发会话：await 期间用户可能切走，UI 写入不得落进别的会话
    // （错误会话被重命名/插卡是持久化污染，不可自愈）。后端已按发起会话
    // 定向；切走后放弃前端播报，挂件靠 syncActivePersona 恢复（审计）。
    const sid = state.activeSessionId;
    try {
      // invoke 形状保持原样（协议指纹按文本计算）；发起瞬间 activeSessionId === sid。
      const card = await invoke("equip_persona", { sessionId: state.activeSessionId, personaId });
      if (deletedPersonaIds.value.has(personaId)) return null;
      lastEquippedSid.value = sid; // 成功加持的目标会话(即使已切走)：供紧随其后的引导卡定向(审计补充)
      if (sid !== state.activeSessionId) return card; // 已切走：不写当前显示
      // 标题仍是默认占位(三语哨兵,见 isDefaultChatTitle)→ 用卡牌名命名(无论草稿态物化还是遗留空会话;
      // 用户已主动改名 / 已被首条消息命名的会话不动)。决策:卡牌优先于首条消息。
      const m = state.sessions.find(function (s) { return s.id === sid; });
      // 标题还是默认值 / 仍是卡牌占位(换卡场景)→ 用(新)卡牌名命名,并标记为占位。
      // 占位名会被首条用户消息覆盖(见 persistMessages*),让同卡会话靠对话内容区分。
      if (m && (isDefaultChatTitle(m.title) || personaPlaceholderTitles.value[sid])) {
        const newTitle = personaName(card);
        if (newTitle) {
          try { await invoke("rename_session", { id: sid, title: newTitle }); } catch { /* rename failure does not affect local display */ }
          if (sid !== state.activeSessionId) return card; // rename 挂起期间切走:放弃后续 UI 写入(审计补充)
          m.title = newTitle;
          personaPlaceholderTitles.value[sid] = true;
        }
      }
      // 同 session 换了一张不同的卡 → 先弹一条"已卸下旧专家",再弹新加持。
      // 旧专家在写点复核而非入口捕获：同会话快速连续换卡时,入口值可能已被
      // 上一次 equip 的权威写覆盖,陈旧值会播报错误的"已卸下"(二审补充)。
      if (deletedPersonaIds.value.has(personaId)) return null;
      const prev = state.activePersona;
      if (prev && prev.id !== card.id) {
        addChatItem({ type: "system", text: bt("personaUnequipped") + personaName(prev), time: timeStr() });
        recordPersonaEvent({ kind: "unequip", name: personaName(prev) });
      }
      personaSyncSeq.value++; // 权威写前 bump：作废在途 syncActivePersona 的旧快照(审计补充)
      state.activePersona = card;
      addChatItem({ type: "persona_equip", card, time: timeStr() });
      recordPersonaEvent({ kind: "equip", card });
      notify();
      return card;
    } catch (e) {
      // 失败也守住归属：失败气泡只落发起会话,不得插进 await 窗口内切到的
      // 会话(addSystemItem 随消息流持久化);同时作废 lastEquippedSid——失败
      // equip 后紧随的引导卡不得回退定向到历史成功 equip 的无关会话(二审补充)。
      lastEquippedSid.value = null;
      if (sid === state.activeSessionId) addSystemItem(bt("equipFailed") + e);
      return null;
    }
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

  // web+tauriVoice shared (both lanes keep a local clearVoiceInput override:
  // ending the unsent voice operation is lane-owned state machine work)
  function appendVoiceText(base, text) {
    const left = String(base || "").trimEnd();
    const right = String(text || "").trim();
    if (!left) return right;
    if (!right) return left;
    return left + (/[。！？.!?，,;；:]$/.test(left) ? " " : "\n") + right;
  }

  // web+tauriVoice 共享（FE14 wave 4：voice-operation 状态机块；voiceOperations
  // Map 由各 lane 持有、以 deps 注入，Map 方法可变引用共享、从不整体重赋值）
  // Operation ownership outlives the recording/UI. Async writeback and chat admission
  // share this record; dismissing a notice cannot revive or cancel a submitted operation.
  // Each recording gets its own operationId, handed to the composer before any async
  // writeback and retained until the draft is really sent or abandoned.
  function rememberVoiceOperation(session) {
    if (!session.sessionId && session.ownerKind === "chat") {
      session.draftEpoch = Number(state.draftEpoch || 0);
    }
    for (const entry of voiceOperations.value) {
      const item = entry[1];
      // Terminal entries are done; a dismissed entry stays adoptable for a
      // manual retry until the next recording starts, then it is swept —
      // a never-sent dismissal must not leak its audio chunks forever.
      if (item.telemetryTerminal || (item.dismissed && !item.pendingSubmission)) {
        voiceOperations.value.delete(entry[0]);
      }
    }
    voiceOperations.value.set(session.operationId, session);
  }

  // web+tauriVoice 共享
  function getVoiceOperationId(sessionId, ownerKind) {
    const candidates = [...voiceOperations.value.values()].reverse();
    const operation = candidates.find(function (item) {
      return !item.telemetryTerminal && !item.pendingSubmission && item.voiceResultReady
        && (item.sessionId || null) === (sessionId || null)
        // A draft operation belongs to the draft epoch it was recorded in; a
        // materialized session must not adopt an older draft's association.
        && (item.sessionId || item.ownerKind !== "chat" || item.draftEpoch === Number(state.draftEpoch || 0))
        && item.ownerKind === (ownerKind || "chat");
    });
    return operation ? operation.operationId : null;
  }

  // web+tauriVoice 共享
  function beginVoiceSubmission(operationId, sessionId) {
    const operation = voiceOperations.value.get(operationId);
    if (!operation || operation.telemetryTerminal) return;
    operation.pendingSubmission = true;
    if (sessionId && !operation.sessionId) {
      operation.sessionId = sessionId;
    }
  }

  // web+tauriVoice 共享
  function voiceOperationSessionId(operationId) {
    return voiceOperations.value.get(operationId)?.sessionId || null;
  }

  // web+tauriVoice 共享
  // Whether the composer's current voice operation is parked on an in-flight
  // send admission. The composer hook skips its identity-change auto-cancel
  // for a parked operation: first-turn materialization flips the adapter
  // identity mid-send, and cancelling here would only kill the completion
  // notice of a message that still lands. A context switch before the park
  // still cancels — that is the wrong-session auto-send protection.
  function hasVoiceSubmissionPending() {
    const operation = voiceOperations.value.get((state.voiceInput || {}).operationId);
    return !!(operation && operation.pendingSubmission);
  }

  // web+tauriVoice 共享
  // Re-entering a departed draft hands its retained text back under a new
  // draft epoch (enterDraft increments unconditionally). Once the text is
  // visibly back in the composer the operation follows it: a manual retry
  // keeps the voice association instead of being orphaned in the dead
  // recording-time epoch (unadoptable, and unreachable by the keyed abandon).
  // Restricted to unsubmitted draft operations (no session binding, chat
  // kind), mirroring the rollback rebind; terminal operations never rebind.
  function rebindVoiceOperationToDraft(operationId, toEpoch) {
    const operation = voiceOperations.value.get(operationId);
    if (!operation || operation.telemetryTerminal || operation.pendingSubmission
      || operation.sessionId || operation.ownerKind !== "chat") return false;
    operation.draftEpoch = Number(toEpoch);
    return true;
  }

  // web+tauriVoice 共享
  // Terminal dedup bookkeeping only (same state machine on both lanes):
  // it marks the operation ended and consumes a queued terminal after a
  // rejected admission; a cancel during admission waits for the admission
  // result — an already-submitted operation is not recorded as cancelled.
  function trackVoiceTerminal(eventName, session, fields) {
    if (!session || session.telemetryTerminal) return;
    if (session.pendingSubmission) {
      session.pendingTerminal = { eventName, fields };
      return;
    }
    session.telemetryTerminal = true;
  }

  // web+tauriVoice 共享
  function completeVoiceSubmission(operationId, sessionId, accepted) {
    const operation = voiceOperations.value.get(operationId);
    if (!operation || operation.telemetryTerminal) return;
    operation.pendingSubmission = false;
    if (sessionId && !operation.sessionId) {
      operation.sessionId = sessionId;
    }
    if (accepted) {
      // Acceptance ends the operation (never adoptable again; the next
      // recording start sweeps it).
      operation.pendingTerminal = null;
      operation.telemetryTerminal = true;
      if (state.voiceInput && state.voiceInput.operationId === operationId) {
        state.voiceInput = Object.assign({}, state.voiceInput, {
          status: "idle", operationId: null, telemetryTerminal: true,
        });
        notify();
      }
    } else if (operation.pendingTerminal) {
      const terminal = operation.pendingTerminal;
      operation.pendingTerminal = null;
      trackVoiceTerminal(terminal.eventName, operation, terminal.fields);
    }
  }

  // web+tauriVoice 共享
  function encodeWav(samples, sampleRate) {
    const dataSize = samples.length * 2;
    const buffer = new ArrayBuffer(44 + dataSize);
    const view = new DataView(buffer);
    function writeString(offset, value) {
      // WAV header writes ASCII only; charCode is the target byte value, fromCodePoint/codePointAt add nothing here.
      for (let i = 0; i < value.length; i++) view.setUint8(offset + i, value.charCodeAt(i)); // eslint-disable-line unicorn/prefer-code-point
    }
    writeString(0, "RIFF");
    view.setUint32(4, 36 + dataSize, true);
    writeString(8, "WAVE");
    writeString(12, "fmt ");
    view.setUint32(16, 16, true);
    view.setUint16(20, 1, true);
    view.setUint16(22, 1, true);
    view.setUint32(24, sampleRate, true);
    view.setUint32(28, sampleRate * 2, true);
    view.setUint16(32, 2, true);
    view.setUint16(34, 16, true);
    writeString(36, "data");
    view.setUint32(40, dataSize, true);
    let offset = 44;
    for (let i = 0; i < samples.length; i++, offset += 2) {
      const s = Math.max(-1, Math.min(1, samples[i]));
      view.setInt16(offset, s < 0 ? s * 0x8000 : s * 0x7fff, true);
    }
    return buffer;
  }

  // web+tauriChat 共享（FE14 wave 4）
  // ── Pinvou v4 召唤式检阅:Boss 主动呼叫,审当前 session 前面的工作 ──
  // 纯召唤、不替 Boss 决策。
  // 审查卡进 chatItems(当前会话可见);跨会话持久化(进 messages/独立存储)是后续增强。
  async function summonPinvou(focus, mode) {
    if (!state.activeSessionId) { addSystemItem(bt("summonNeedsSession")); return; }
    if (state.pinvouSummoning) return;
    state.pinvouSummoning = true;
    const sid = state.activeSessionId; // 召唤发起时的 session;await 返回后校验,防跨 session 串(召唤慢+切走)
    // 检阅结果弹 modal(不进对话流):一次只一个,裁决/跳过直接操作 state.pinvouModal.review、
    // 不靠 pos 定位(根治连续召唤 pos 重复串卡)。
    state.pinvouModal = { loading: true, coverage: mode === "coverage" };
    notify();
    try {
      // focus=产出物 path(品=审产物); mode="coverage"=悟(通盘体检)。
      const review = await invoke("summon_pinvou", { sessionId: sid, focus: focus || null, mode: mode || null });
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

  // web+tauriChat 共享（FE14 wave 4）
  // §2 按勾选裁决:resolution 已由前端写回 review 对象(引用→sidecar),这里持久化 +
  // 把勾「让AI改」的条目走 B1 发定向修订指令(只改对应段落、禁全文重写)。Boss 驾驶,非自动。
  async function resolvePinvouReview(resolutions, actions) {
    // 检阅发生的会话归属捕获：persist 挂起期间用户可能切走，修订指令必须发回
    // 检阅会话，不得漂进当前 active 会话（审计）。
    const reviewSid = state.activeSessionId;
    // 弹窗只一个 review(state.pinvouModal.review),直接在它上面写 resolution——不靠 pos 定位
    // (根治连续召唤 pos 重复串卡)。它和 sidecar entry.review 同引用,写它=写 sidecar。
    const isWu = !!(state.pinvouModal && state.pinvouModal.coverage); // 关窗前取,供转交标品/悟
    const review = state.pinvouModal && state.pinvouModal.review;
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
    const fix = actions.filter(function (a) { return a.t === "fix"; });
    const verify = actions.filter(function (a) { return a.t === "verify"; });
    const adopt = actions.filter(function (a) { return a.t === "adopt"; });
    const ask = actions.filter(function (a) { return a.t === "ask"; });
    const parts = [];
    if (fix.length) {
      parts.push(bt("reviewFixHeader"));
      fix.forEach(function (a) { parts.push("- " + a.text); });
    }
    if (verify.length) {
      if (parts.length) parts.push("");
      parts.push(bt("reviewVerifyHeader"));
      verify.forEach(function (a) { parts.push("- " + a.text); });
    }
    if (adopt.length) {
      if (parts.length) parts.push("");
      parts.push(bt("reviewAdoptHeader"));
      adopt.forEach(function (a) { parts.push("- " + (a.topic ? a.topic + "：" : "") + a.pick); });
    }
    if (ask.length) {
      if (parts.length) parts.push("");
      parts.push(bt("reviewAskHeader"));
      ask.forEach(function (a) { parts.push("- " + a.topic); });
    }
    const fill = actions.filter(function (a) { return a.t === "fill"; });
    if (fill.length) {
      if (parts.length) parts.push("");
      parts.push(bt("reviewFillHeader"));
      fill.forEach(function (a) { parts.push("- " + a.dimension + (a.suggestion ? "：" + a.suggestion : "")); });
      parts.push(bt("reviewFillFooter"));
    }
    // 已切走则放弃发指令（修订指令属于检阅会话，漂进别的会话会误导其上下文）。
    if (parts.length && reviewSid && state.activeSessionId === reviewSid) sendMessage(parts.join("\n"), { pinvouTransfer: isWu ? "悟" : "品" });
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

  // auxChat cluster (round-31 M7): the aux bridge domain previously existed
  // twice — platform/tauri/bridge/aux-chat.js and platform/web/bridge.js
  // (~60 logic lines with only ~3 differing). The function bodies come from
  // the former web-side mirror; the two real differences stay lane-side:
  // send dispatch (the lane passes auxChatDispatch in; desktop goes through
  // chat, web through web_access_chat) and the web-side
  // session_turn_in_progress translation wrapper — dropped with the M7 move:
  // on either lane send errors only reach console.warn and the panel's
  // static localized sendFailed copy, so translated error text never reached
  // the user (the desktop side was always the bare string) — one behavior
  // for both lanes.
  // Display-only and case-sensitive ON PURPOSE: it only gates ensure/send
  // inputs on ids the backend itself minted (always lowercase `aux-`);
  // identity and destructive paths decide in Rust, whose
  // is_aux_session_id is deliberately case-insensitive. Never reuse this
  // predicate for a security or destructive decision.
  function auxChatIsAuxSession(id) {
    return typeof id === "string" && id.indexOf("aux-") === 0;
  }
  function auxChatEmptySnapshot() {
    return { chatItems: [], busy: false, queued: [] };
  }
  async function auxChatEnsure(taskId) {
    const task = String(taskId || "").trim();
    if (!task) throw new Error(bt("targetSessionMissing"));
    const metadata = await invoke("get_or_create_aux_session", { sessionId: task });
    const auxId = metadata && typeof metadata.id === "string" ? metadata.id : "";
    if (!auxChatIsAuxSession(auxId)) throw new Error(bt("sessionDataInvalid"));
    // Aux sessions never become active: they use the background-buffer
    // load_session(setActive:false) path.
    await ensureSessionBufferLoaded(auxId);
    return auxId;
  }
  async function auxChatSend(auxId, text) {
    const sid = String(auxId || "").trim();
    const message = String(text || "").trim();
    if (!auxChatIsAuxSession(sid)) throw new Error(bt("targetSessionMissing"));
    if (!message) throw new Error(bt("replyContentEmpty"));
    await ensureSessionBufferLoaded(sid);
    const buf = sessionStates.value[sid];
    // Aux sessions never queue (queue is user-input semantics): reject
    // outright when busy or queued messages exist; the caller retries.
    if (isBusyFor(sid) || (buf && Array.isArray(buf.queued) && buf.queued.length > 0)) {
      throw new Error(bt("turnAlreadyInProgress"));
    }
    // 发送命令是 lane 真实差异（desktop: chat + attachments;Web:
    // web_access_chat + attachmentHandles),lane 以 auxChatDispatch 传入。
    return auxChatDispatch(sid, message);
  }
  // Synchronous snapshot: when not loaded (no buffer) returns an empty
  // structure — never throws and never triggers a load. Items are shallow-
  // copied one by one: streaming deltas mutate buffer items in place (the
  // item.text/html assignments in chat-events), so copying only the array
  // would share object references and a caller comparing field by field
  // could not detect changes; after copying, every poll gets fresh
  // references, making field comparison a true content comparison.
  function auxChatSnapshotItems(items) {
    return (Array.isArray(items) ? items : []).map(function (item) {
      return item && typeof item === "object" ? Object.assign({}, item) : item;
    });
  }
  function auxChatSnapshot(auxId) {
    const sid = String(auxId || "").trim();
    if (!sid) return auxChatEmptySnapshot();
    if (sid === state.activeSessionId) {
      return {
        chatItems: auxChatSnapshotItems(state.chatItems),
        busy: !!state.busy,
        queued: auxChatSnapshotItems(state.queued),
      };
    }
    const buf = sessionStates.value[sid];
    if (!buf) return auxChatEmptySnapshot();
    // An always-open panel polling snapshot() counts as "reading": refresh
    // LRU recency consistently with the getBuffer read paths, otherwise
    // after 32+ session switches the buffer is evicted by capacity and the
    // panel wrongly shows the empty state.
    touchSessionBuffer(sid, buf, false);
    return {
      chatItems: auxChatSnapshotItems(buf.chatItems),
      busy: !!buf.busy,
      queued: auxChatSnapshotItems(buf.queued),
    };
  }
  // Atomic restart (M6): one backend command discards the old aux session
  // (gated against its turns) and creates the fresh one — no two-invoke
  // window for an orphaned discard to land in. The aux id is derived
  // (aux-<taskId>, round-30 B8), so the stale buffer purge needs no
  // per-task id map; the backend's session:deleted also purges through
  // the sessions domain as a fallback.
  async function auxChatReset(taskId) {
    const task = String(taskId || "").trim();
    if (!task) throw new Error(bt("targetSessionMissing"));
    const metadata = await invoke("reset_aux_session", { sessionId: task });
    const auxId = metadata && typeof metadata.id === "string" ? metadata.id : "";
    if (!auxChatIsAuxSession(auxId)) throw new Error(bt("sessionDataInvalid"));
    purgeSessionBuffer(auxId);
    // Aux sessions never become active: they use the background-buffer
    // load_session(setActive:false) path.
    await ensureSessionBufferLoaded(auxId);
    return auxId;
  }

  // web+tauriChatEvents 共享（FE14 wave 5：chat 事件 lane 的 DI 对）
  function visibleUserTurnIndex() {
    const count = state.chatItems.filter(function (item) { return item && item.type === "user"; }).length;
    return Math.max(0, count - 1);
  }

  // web+tauriChatEvents 共享
  function latestOpenTimelineStart() {
    const events = state.turnTimeline || [];
    const completed = Object.create(null);
    events.forEach(function (event) {
      if (event && event.event === "assistant_done") completed[event.turn_id] = true;
    });
    for (let index = events.length - 1; index >= 0; index--) {
      const event = events[index];
      if (event && event.event === "user_start" && !completed[event.turn_id]) return event;
    }
    return null;
  }

  // web+tauriChatEvents 共享
  function recordTurnStarted() {
    if (state.activeTurnTimelineId) return;
    const timestamp = Date.now();
    const turnIndex = visibleUserTurnIndex();
    const existing = latestOpenTimelineStart();
    if (existing && Math.abs(timestamp - Number(existing.timestamp || 0)) < 60000) {
      existing.ui_turn_index = turnIndex;
      state.activeTurnTimelineId = existing.turn_id;
      return;
    }
    const turnId = "ui_" + String(state.activeSessionId || "session") + "_" + timestamp + "_" + turnIndex;
    state.activeTurnTimelineId = turnId;
    state.turnTimeline = [...(state.turnTimeline || []), {
      turn_id: turnId,
      event: "user_start",
      timestamp,
      ts: new Date(timestamp).toISOString(),
      ui_turn_index: turnIndex,
    }];
  }

  // web+tauriChatEvents 共享
  // Every semantic notification is a synchronous flush boundary. Its
  // snapshot already contains all accumulated deltas, so the pending frame
  // must be cancelled to avoid a duplicate callback after done/error/tool/
  // permission/session events.
  function cancelPendingStreamNotify(sid) {
    const pending = sid && pendingStreamNotifications.value[sid];
    if (!pending) return;
    if (pending.frame !== null && typeof window.cancelAnimationFrame === "function") {
      window.cancelAnimationFrame(pending.frame);
    }
    if (pending.timer !== null && typeof clearTimeout === "function") clearTimeout(pending.timer);
    delete pendingStreamNotifications.value[sid];
  }

  // web+tauriChatEvents 共享
  function preserveInterruptedAssistantPresentation() {
    let userItemIndex = -1;
    let afterMessageIndex = -1;
    let afterUserOrdinal = -1;
    for (let index = 0; index < state.chatItems.length; index++) {
      const candidate = state.chatItems[index];
      if (!candidate || candidate.type !== "user") continue;
      afterUserOrdinal += 1;
      userItemIndex = index;
      afterMessageIndex = -1;
      if (Number.isFinite(Number(candidate.messageIndex))) {
        afterMessageIndex = Number(candidate.messageIndex);
      }
    }
    // 没有任何 user 气泡时无法锚定轮次;若把全部历史 assistant 项都标记为
    // 仅展示,下次权威重载会在末尾追加整段历史的重复副本。此时放弃保留,
    // 退化为修复前行为(重载后消失),但必须清空 pending 以免污染下一轮。
    if (userItemIndex < 0) {
      pendingAssistantText.value = "";
      pendingAssistantBlocks.value = [];
      return;
    }
    for (let itemIndex = userItemIndex + 1; itemIndex < state.chatItems.length; itemIndex++) {
      const item = state.chatItems[itemIndex];
      if (!item || item.type !== "assistant" || !item.html) continue;
      item.interruptedDisplayOnly = true;
      item.afterMessageIndex = afterMessageIndex;
      item.afterUserOrdinal = afterUserOrdinal;
    }
    pendingAssistantText.value = "";
    pendingAssistantBlocks.value = [];
  }

  // web+tauriChatEvents 共享
  function startReasoningBlock(index) {
    const existing = streamingReasoningItem(index);
    if (existing) return existing;
    finalizeStreamingReasoning();
    finalizeAssistantStreamBeforeReasoning();
    const item = {
      type: "reasoning",
      text: "",
      time: timeStr(),
      streaming: true,
      startedAt: Date.now(),
      completedAt: null,
      reasoningIndex: index,
    };
    addChatItem(item);
    // 用空 thinking block 记录明确的 Started 边界，使两个相邻的
    // thinking content block 不会在持久化时被误合并。
    pendingAssistantBlocks.value.push({ type: "thinking", thinking: "" });
    return item;
  }

  // web+tauriChatEvents 共享
  function appendReasoningBlock(text) {
    const blocks = pendingAssistantBlocks.value;
    const last = blocks[blocks.length - 1];
    if (last && last.type === "thinking") last.thinking += text;
    else blocks.push({ type: "thinking", thinking: text });
  }

  // web+tauriChatEvents 共享
  function flushAssistantMessageToHistory() {
    flushPendingTextBlock();
    if (pendingAssistantBlocks.value.length) {
      const assistantText = pendingAssistantBlocks.value
        .filter(function (block) { return block && block.type === "text" && block.text; })
        .map(function (block) { return block.text; })
        .join("\n\n");
      if (state.activeSessionId && state.activeSessionId === state.scheduledTaskCreationSessionId) {
        const scheduledTaskDraft = parseScheduledTaskDraftFromText(assistantText);
        if (scheduledTaskDraft) {
          autoCreateScheduledTaskDraft(scheduledTaskDraft, state.activeSessionId);
        }
      }
      state.messages.push({ role: "assistant", content: pendingAssistantBlocks.value });
      pendingAssistantBlocks.value = [];
    }
  }

  // web+tauriMain 共享（FE14 wave 5：rerenderFromMessages 共享核心）。
  // 重放主体两侧逐字一致；仅剩的 lane 差异点全部经 lane hooks 注入：
  //   itemIdSeq                重放前清零（cell）
  //   toolMeta(cell)           重放 meta 表（cell；清空时机两 lane 一致）
  //   emitPersonaAt            卡牌事件按 pos 插回（lane 以本次调用的 pe 快照构造
  //                            嵌套 cluster，见两 lane wrapper）
  //   markSteered              mid-turn steer 标记（desktop steering 专属；web 无）
  //   carefulBlockedItem       🛑 红卡形状（web 附 toolCallId 供前端联动）
  //   afterToolResultBackfill  回填后清理（tauri 删 toolMeta 条目；web 按 32 条 LRU
  //                            策略保留残差——两侧注释各自成文）
  //   rebuildToolMeta          meta 重建策略（tauri 只重建有 result 的；web 全量重建）
  //   sweepUnsettledTools      重放后未决工具卡终态（tauri 置 failed；web 无 sweep）
  // eslint-disable-next-line sonarjs/cognitive-complexity -- verbatim merged replay body; the lane hooks extracted the only lane differences
  function rerenderFromMessagesCore(lane, opts) {
    state.chatItems = [];
    lane.itemIdSeq.value = 0;
    // Replay rebuilds toolMeta only for historical tool_uses that still have a
    // matching tool_result to backfill (write/patch's large args otherwise
    // duplicate the whole transcript inside the 32-entry all-session LRU).
    // Durable replay clears first (when not live hydrating), reclaiming only
    // orphan entries left by interrupted turns (the live event path itself
    // stays insert/delete balanced); entries are deleted again right after
    // their tool_result backfill lookup — nothing reads them afterwards.
    if (!(opts && opts.keepLiveToolMeta)) lane.toolMeta.value = {};
    // 预扫 tool_result：tool_use 在 assistant 消息、result 在后续 user 消息，需提前建映射
    // 才能在还原选择卡/方案卡时拿到结果（选项/快照）。
    const resultById = {};
    for (let ri = 0; ri < state.messages.length; ri++) {
      const rc = state.messages[ri].content;
      if (!Array.isArray(rc)) continue;
      for (let rj = 0; rj < rc.length; rj++) {
        if (rc[rj].type === "tool_result") {
          resultById[rc[rj].tool_use_id] = { content: rc[rj].content, is_error: !!rc[rj].is_error };
        }
      }
    }
    // 预扫:每个产物最后一次被 write/append/edit 改的 tool_use id → rerender 只在最后一次
    // 续一张成品卡(与实时 chat:done 的一张对齐,不刷一堆)。
    const lastDirtyArtifactId = {};
    const writtenArtifacts = {}; // write/append 写过的 path=产物;没 present 时兜底补首卡
    const presentedArtifacts = {}; // 整篇 present_artifact 过的 path → 别再兜底补首卡(present 会出卡,否则重复)
    const presentedArtifactNames = {}; // path 可能一边相对一边绝对,basename 去重防重复卡
    for (let di = 0; di < state.messages.length; di++) {
      const dc = state.messages[di].content;
      if (!Array.isArray(dc)) continue;
      for (let dj = 0; dj < dc.length; dj++) {
        const db = dc[dj];
        const dbMutation = db.type === "tool_use" && fileMutationAction(db.name, db.input);
        if (dbMutation) {
          extractArtifactPaths(db.input).forEach(function (dap) {
            lastDirtyArtifactId[dap] = db.id;
            // 与实时 tool_end 同一门控:tmp/ 中间文件、非成品扩展名不记账,
            // 否则实时不进面板的文件切 session 重放后反而兜底冒出成品卡。
            if (dbMutation !== "edit" && isDeliverable(dap)) writtenArtifacts[dap] = true;
          });
        } else if (db.type === "tool_use" && isPresentArtifactTool(db.name)) {
          const pap = extractArtifactPath(db.input);
          const pres = resultById[db.id];
          const pp = presentArtifactAbsPath(pres && pres.content, pap);
          if (pp) {
            presentedArtifacts[pp] = true;
            presentedArtifactNames[basename(pp)] = true;
          }
        } else if (db.type === "tool_use" && shouldUseToolOutputAsArtifact(db.name)) {
          const gres = resultById[db.id];
          if (!(gres && gres.is_error)) {
            const gp = artifactPathFromToolOutput(gres && gres.content);
            if (gp && isDeliverable(gp)) {
              lastDirtyArtifactId[gp] = db.id;
              writtenArtifacts[gp] = true;
            }
          }
        }
      }
    }
    for (let mi = 0; mi < state.messages.length; mi++) {
      lane.emitPersonaAt(mi, false); // 该消息之前发生的卡牌事件先插
      const m = state.messages[mi];
      const blocks = Array.isArray(m.content) ? m.content : [];
      if (m.role === "user") {
        // Always strip exact internal envelopes: sessions saved before the
        // persistence alignment still carry the engine-baked <turn_meta>
        // block on mid-turn steered messages, and without the filter the
        // hydrated bubble rendered the raw envelope after a restart.
        const utext = lane.userMessageDisplayText(blocks, true);
        if (utext) {
          // pinvouTransfer 是展示层标记、不在 messages → rerender 从转交固定措辞还原品/悟样式
          const uitem2 = { type: "user", text: utext, time: "", messageIndex: mi };
          lane.markSteered(uitem2, mi, utext, blocks);
          const scene = pinvouSceneForMessagePos(mi);
          if (scene) uitem2.pinvouScene = scene;
          if (textMatchesBtKey(utext, "reviewFillHeader")) uitem2.pinvouTransfer = "悟";
          else if (textMatchesBtKey(utext, "reviewFixHeader") || textMatchesBtKey(utext, "reviewAdoptHeader") || textMatchesBtKey(utext, "reviewAskHeader")) uitem2.pinvouTransfer = "品";
          addChatItem(uitem2);
        }
        // tool_result（只回填普通工具卡；选择卡/方案卡的结果已在 tool_use 处还原）
        for (let ci = 0; ci < blocks.length; ci++) {
          const c = blocks[ci];
          if (c.type !== "tool_result") continue;
          const tm = lane.toolMeta.value[c.tool_use_id];
          if (tm) {
            // careful hook 拦截 → 还原 🛑 红卡(实时由 tool_end metadata 插,重载从文本反解)
            const blockedMd = parseCarefulBlocked(toolResultText(c.content));
            if (blockedMd) {
              lane.updateToolItem(c.tool_use_id, toolResultDisplayContent(c.content), false); // 被拦=失败态,与实时一致
              addChatItem(lane.carefulBlockedItem(c, tm, blockedMd));
            } else {
              // load_skill 同样脱敏：重载历史时也不还原 SKILL.md 全文，展开只见占位。
              const contentForCard = (tm.name === "load_skill")
                ? bt("skillContentHidden")
                : toolResultDisplayContent(c.content);
              lane.updateToolItem(c.tool_use_id, contentForCard, !c.is_error);
            }
          }
          lane.afterToolResultBackfill(c.tool_use_id);
        }
        continue;
      }
      if (m.role !== "assistant") continue;
      let textBuf = "";
      let planSnap = null, todosSnap = null, sawPlanTool = false;
      for (let bi = 0; bi < blocks.length; bi++) {
        const b = blocks[bi];
        if (b.type === "text") {
          textBuf += b.text;
        } else if (b.type === "thinking") {
          if (textBuf) {
            addChatItem({ type: "assistant", text: textBuf, html: lane.renderMarkdown(textBuf), time: "", streaming: false });
            textBuf = "";
          }
          const reasoningText = String(b.thinking || b.text || "");
          if (reasoningText) {
            addChatItem({
              type: "reasoning", text: reasoningText, time: "", streaming: false,
              startedAt: null, completedAt: null,
            });
          }
        } else if (b.type === "tool_use") {
          if (textBuf) {
            addChatItem({ type: "assistant", text: textBuf, html: lane.renderMarkdown(textBuf), time: "", streaming: false });
            textBuf = "";
          }
          lane.rebuildToolMeta(b, resultById, opts);
          // request_user_input → 还原只读选择卡（问题来自 input，选项高亮来自 result）
          if (b.name === "request_user_input") {
            const qs = (b.input && b.input.questions) || [];
            if (Array.isArray(qs) && qs.length) {
              const res = resultById[b.id];
              // 快照可能落在 turn 进行中（底座每次落盘）：tool_use 尚无对应
              // tool_result，不能按历史恢复为 submitted。跳过，等
              // chat:user_input_required 事件渲染可交互的 active 卡。
              if (!res) continue;
              addChatItem({
                type: "user_input", toolCallId: b.id, questions: qs,
                resolved: true, cardState: res.is_error ? "cancelled" : "submitted",
                restoredAnswers: parseUserAnswers(res.content, qs), time: "",
              });
            }
            continue;
          }
          // present_artifact → 还原成品卡(切会话不丢)。仅当工具成功时还原:
          // 失败的调用回退成普通工具卡(下方 default addChatItem)。
          if (isPresentArtifactTool(b.name)) {
            const pares = resultById[b.id];
            if (!(pares && pares.is_error)) {
              const rpp = presentArtifactAbsPath(pares && pares.content, b.input && b.input.path);
              const restoredCard = {
                type: "artifact_card",
                path: rpp,
                title: (b.input && b.input.title) || "",
                description: (b.input && b.input.description) || "",
                time: "",
                sessionId: state.activeSessionId,
              };
              if (!updatePresentedArtifact(restoredCard)) addChatItem(restoredCard);
              continue;
            }
          }
          // update_plan / checklist_write / todo_write → 收集快照，本条消息末尾还原方案卡
          if (lane.PLAN_TOOLS.has(b.name)) {
            const snap = parsePlanSnapshot(resultById[b.id] && resultById[b.id].content);
            if (snap) {
              if (b.name === "update_plan") planSnap = snap; else todosSnap = snap;
            }
            sawPlanTool = true;
            continue;
          }
          addChatItem({ type: "tool", toolId: b.id, name: b.name, args: b.input, output: null, success: null, state: "pending" });
          if (shouldUseToolOutputAsArtifact(b.name)) {
            const gres2 = resultById[b.id];
            const gap = artifactPathFromToolOutput(gres2 && gres2.content);
            if (!(gres2 && gres2.is_error) && gap && isDeliverable(gap) && lastDirtyArtifactId[gap] === b.id && !presentedArtifacts[gap] && !presentedArtifactNames[basename(gap)]) {
              const gprev = findPresentedArtifact(gap);
              if (gprev) {
                const gcard = {
                  type: "artifact_card", path: gprev.path, title: gprev.title,
                  description: gprev.description, time: "", sessionId: state.activeSessionId,
                };
                if (!updatePresentedArtifact(gcard)) addChatItem(gcard);
              } else if (writtenArtifacts[gap]) {
                addChatItem({ type: "artifact_card", path: gap, title: basename(gap), description: "", time: "", sessionId: state.activeSessionId });
              }
            }
          }
          // 还原"自动续卡":File.write/File.edit 改的文件之前 present 过 → 续一张
          // 成品卡(与实时 tool_end 的自动续逻辑对齐,切会话不丢)。present 的卡按
          // 顺序在前(必须先 present 才进集合),此处 findPresentedArtifact 能命中。
          if (fileMutationAction(b.name, b.input)) {
            const wres = resultById[b.id];
            extractArtifactPaths(b.input).forEach(function (wap) {
              // 去重:同产物只在最后一次修改处补一张卡(与实时对齐)。
              if ((wres && wres.is_error) || lastDirtyArtifactId[wap] !== b.id) return;
              const wprev = findPresentedArtifact(wap);
              if (wprev) {
                const wcard = {
                  type: "artifact_card", path: wprev.path, title: wprev.title,
                  description: wprev.description, time: "", sessionId: state.activeSessionId,
                };
                if (!updatePresentedArtifact(wcard)) addChatItem(wcard);
              } else if (writtenArtifacts[wap] && !presentedArtifacts[wap] && !presentedArtifactNames[basename(wap)]) {
                // AI 写了产物但全程没 present_artifact → 兜底补首卡(与实时 chat:done 对齐)
                addChatItem({ type: "artifact_card", path: wap, title: basename(wap), description: "", time: "", sessionId: state.activeSessionId });
              }
            });
          }
        }
      }
      if (textBuf) {
        addChatItem({ type: "assistant", text: textBuf, html: lane.renderMarkdown(textBuf), time: "", streaming: false });
      }
      // 本条 assistant 消息用过 plan 工具 → 还原一张只读历史方案卡
      if (sawPlanTool && (planSnap || todosSnap)) {
        const snaps = { plan: planSnap, todos: todosSnap };
        addChatItem({
          type: "plan_card", plan: planSnap, todos: todosSnap,
          planMarkdown: composePlanMarkdown(snaps),
          cardState: "frozen", resolved: true, statusLabel: bt("planHistorical"), time: "",
        });
      }
    }
    lane.sweepUnsettledTools(resultById);
    lane.emitPersonaAt(state.messages.length, true); // 最后一条消息之后发生的卡牌事件(末尾加持/卸下)
  }

  // web+tauriMain 共享（FE14 wave 5：权威重载的 live 卡片过滤——后台 Shell 保留策略）
  function liveChatItemKeptForHydration(item) {
    if (!item || item.type === "user") return false;
    if (item.type === "assistant") return item.interruptedDisplayOnly === true;
    if (item.turnErrorNotice && !item.legacyConversationOnly) return false;
    // 后台 Shell 在 chat:done 后仍继续运行；持久化 transcript 只有工具结果文本，
    // 没有 taskId/background/liveOutput，权威重载时必须保留实时卡片并按 toolId 合并。
    if (item.type === "tool") return item.background === true && item.state === "running";
    // Plan cards need semantic matching by their plan snapshot. Their generic
    // hydration key includes ticket/action state and would append an active
    // live duplicate after the durable frozen card.
    if (item.type === "plan_card") return false;
    return true;
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
    "isDefaultChatTitle",
    "autoCreateScheduledTaskDraft",
    "flushAssistantMessageToHistory",
    "isDeliverable",
    "touchSessionBuffer",
    "isBusyFor",
    "messageHasToolBlock",
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
    isDefaultChatTitle(v) { isDefaultChatTitle = v; },
    autoCreateScheduledTaskDraft(v) { autoCreateScheduledTaskDraft = v; },
    flushAssistantMessageToHistory(v) { flushAssistantMessageToHistory = v; },
    isDeliverable(v) { isDeliverable = v; },
    touchSessionBuffer(v) { touchSessionBuffer = v; },
    isBusyFor(v) { isBusyFor = v; },
    messageHasToolBlock(v) { messageHasToolBlock = v; },
  };
  /* eslint-enable no-func-assign */
  for (const name of FORWARDER_DEP_NAMES) {
    if (deps[name] !== undefined) forwarderSetters[name](deps[name]);
  }

    return {bt, textMatchesBtKey, isDefaultChatTitle, authoritySyncBufferSnapshot, normalizePinvouScene, pinvouSceneStorageKey, normalizePinvouSceneEvents, loadPinvouSceneEventsForSession, savePinvouSceneEventsForSession, recordPinvouSceneForMessage, pinvouSceneForMessagePos, getBuffer, freshBuffer, stashEvictedSessionDraft, hydratedMessageKey, mergeHydratedArtifacts, isProtectedScheduledBuffer, touchSessionBuffer, registerScheduledRunOwner, scheduledRunOwnerVisibleRank, scheduledRunOwnerPriority, pruneScheduledRunSessionOwners, isScheduledRunTerminal, rememberScheduledRunOwner, scheduledRunBuffer, markScheduledInitialTurnActive, markScheduledInitialTurnTerminal, beginScheduledOpenActivation, rollbackScheduledOpenActivation, markRemoteTurn, onSessionEvent, isScheduledRunSession, defineSubscriptionStateProperty, copySubscriptionStateObject, subscriptionStateValue, loadScheduledTaskTemplateSources, rememberScheduledTaskTemplateSource, attachScheduledTaskTemplateSource, attachAndPruneScheduledTaskTemplateSources, upsertScheduledTask, applyScheduledRunViewed, invalidateScheduledTaskReads, invalidateScheduledRecentRuns, invalidateScheduledRecentRunsForSession, scheduleScheduledRunRefresh, scheduledTaskErrorText, setScheduledTaskError, dismissScheduledTaskError, clearScheduledTaskLoadError, beginScheduledTaskLoad, endScheduledTaskLoad, scheduledTaskRequestStamp, isCurrentScheduledTaskRequest, selectScheduledTask, clearScheduledTaskSelection, extractBalancedJsonObject, parseLooseJsonObject, parseScheduledTaskDraftFromText, normalizeScheduledTaskDraft, activeScheduledTaskModelConfig, lockScheduledTaskDraftModel, scheduledTaskInputFromDraft, autoCreateScheduledTaskDraft, createScheduledTask, loadScheduledTasks, readScheduledTask, mergeScheduledTaskRecentRuns, loadScheduledTaskRuns, upsertRecentRunRow, loadScheduledTaskRecentRuns, refreshScheduledTaskData, refreshScheduledRunShortcutUntilLinked, upsertScheduledTaskRun, runScheduledTaskAction, updateScheduledTask, pauseScheduledTask, resumeScheduledTask, toggleScheduledTaskPinned, deleteScheduledTask, runScheduledTaskNow, startScheduledTaskChat, messageHasToolBlock, toolCallAlreadyStarted, sendMessageToSession, toolCallAlreadyFinished, hasChatItemForTool, addSystemItem, addAuthoritySyncNotice, compactPruneRollupText, removeCompactionStartItem, addOrMergePruneCompaction, timeStr, createNewSession, reportSessionSwitchFailure, mergeHydratedMessages, hydratedChatItemKey, switchToSession, openScheduledRunChat, exitScheduledRunChat, recentScheduledRunForSession, leaveSessionView, applyDeletedSession, onSessionDeleted, renameSession, toggleSessionPinned, archiveSession, restoreArchivedSession, toolResultText, stripInternalToolRuntimeSuffix, toolResultDisplayContent, escapeHtml, renderMarkdown, parsePlanSnapshot, parseUserAnswers, parseCarefulBlocked, userMessageInputProvenance, isInternalUserMessageProvenance, isShellExecutionTool, utf8Length, formatShellSnapshot, shellCommandForItem, shellSnapshotKey, terminalShellHistoryMatch, applyShellSnapshots, scheduleShellPoll, runShellPoll, patchLastItem, hasUnresolvedItem, basename, isAbsPath, normalizedPath, applyWorkspaceReboundMark, sessionRecentlyRebound, rebaseArtifactPathsForRebind, pathIsRebindStale, noteArtifactChange, isSharedMcpArtifactPath, artifactBelongsToSession, filterSessionArtifacts, isTmpPath, isDeliverable, markTurnDirtyArtifact, untrackArtifact, findPresentedArtifact, updatePresentedArtifact, pushArtifactPath, extractArtifactPath, fileMutationAction, composePlanMarkdown, isBusyFor, formatAttachmentDisplayText, queuedPayloadEnvelope, stripMentionBlockForComposerRestore, makeQueuedMessage, rebuiltQueuedPayload, rebuiltQueuedMetaPayload, getComposerDraft, setComposerDraft, prefillComposer, inspectPinvou, recordPinvouReview, dismissPinvouReview, summonPinvou, resolvePinvouReview, persistPinvouReviews, planCardHydrationKey, reasoningEventIndex, streamingReasoningItem, finalizeStreamingReasoning, visibleUserTurnIndex, latestOpenTimelineStart, recordTurnStarted, cancelPendingStreamNotify, preserveInterruptedAssistantPresentation, startReasoningBlock, appendReasoningBlock, flushAssistantMessageToHistory, isPresentArtifactTool, artifactPathFromToolOutput, shouldUseToolOutputAsArtifact, presentArtifactAbsPath, parseToolResultPayload, numOr0, adjustCounters, startMonitorPolling, stopMonitorPolling, pollMonitor, loadSettings, loadSelectedPet, setSelectedPet, enqueueSettingsWrite, submitFeedback, discoverLocalVllm, loadEffectiveModelConfig, getImageInputCapability, loadModels, revealModelApiKey, switchModel, testModelConnection, testImageInputCapability, refreshSuperPerm, setModeLane, patchItemById, runOnSession, addSystemItemFor, patchItemByIdFor, candidateCardPatcher, memoryWriteLabel, memoryWriteStatusLabel, normalizeMemoryCandidateText, handleMemoryWrite, orderedMemoryWarnings, applyMemoryProfileState, applyMemoryWriteState, upsertMemoryValue, upsertPendingMemoryCandidate, rehydratePendingMemoryCandidates, discardStaleLoad, loadOrganizeHistory, applyMemoryOverview, loadMemoryOverview, updateMemoryItem, deleteMemoryItem, resolveMemoryCandidate, startThinking, thinkingTool, thinkingIdle, stopThinking, isActionablePlanCard, setPlanModeNext, planStuckReplan, submitUserInput, openContainingFolder, revealSessionFolder, openScheduledTaskFolder, deliverableCategory, sessionTitleById, currentMemoryArtifacts, conversationAttachmentArgs, pickAndAttach, loadPersonas, refreshPersonas, createPersona, updatePersona, deletePersona, personaName, recordPersonaEvent, equipPersona, postCardCreatorIntro, normalizeMountedCollections, applyMountedCollections, mountedCollectionTargetAtEnqueue, updateMountedCollections, mountCollection, setCollectionEnabled, removeCollection, checkForUpdate, checkDependencies, setVoiceInputStatus, emitVoiceDiagnostic, voiceFlowError, requestVoiceMedia, mergeFloatChunks, downsamplePcm, closeVoiceAsrSetup, downloadKbModel, cancelVoiceInput, appendVoiceText, trackVoiceTerminal, rememberVoiceOperation, getVoiceOperationId, beginVoiceSubmission, voiceOperationSessionId, hasVoiceSubmissionPending, rebindVoiceOperationToDraft, completeVoiceSubmission, encodeWav, interruptedDisplayRange, emitPersonaAt, rerenderFromMessagesCore, liveChatItemKeptForHydration, restoreUiTurnState, rememberDraftWorkspaceRecent, maybePruneFailedWorkspaceRecent, forgetDraftWorkspaceRecent, auxChatEnsure, auxChatSend, auxChatSnapshot, auxChatReset };}

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

      return Object.freeze({bt: b.bt, textMatchesBtKey: b.textMatchesBtKey, isDefaultChatTitle: b.isDefaultChatTitle, authoritySyncBufferSnapshot: b.authoritySyncBufferSnapshot, normalizePinvouScene: b.normalizePinvouScene, pinvouSceneStorageKey: b.pinvouSceneStorageKey, normalizePinvouSceneEvents: b.normalizePinvouSceneEvents, loadPinvouSceneEventsForSession: b.loadPinvouSceneEventsForSession, savePinvouSceneEventsForSession: b.savePinvouSceneEventsForSession, recordPinvouSceneForMessage: b.recordPinvouSceneForMessage, pinvouSceneForMessagePos: b.pinvouSceneForMessagePos, getBuffer: b.getBuffer, freshBuffer: b.freshBuffer, stashEvictedSessionDraft: b.stashEvictedSessionDraft, hydratedMessageKey: b.hydratedMessageKey, mergeHydratedArtifacts: b.mergeHydratedArtifacts, isProtectedScheduledBuffer: b.isProtectedScheduledBuffer, touchSessionBuffer: b.touchSessionBuffer, registerScheduledRunOwner: b.registerScheduledRunOwner, scheduledRunOwnerVisibleRank: b.scheduledRunOwnerVisibleRank, scheduledRunOwnerPriority: b.scheduledRunOwnerPriority, pruneScheduledRunSessionOwners: b.pruneScheduledRunSessionOwners, isScheduledRunTerminal: b.isScheduledRunTerminal, rememberScheduledRunOwner: b.rememberScheduledRunOwner, scheduledRunBuffer: b.scheduledRunBuffer, markScheduledInitialTurnActive: b.markScheduledInitialTurnActive, markScheduledInitialTurnTerminal: b.markScheduledInitialTurnTerminal, beginScheduledOpenActivation: b.beginScheduledOpenActivation, rollbackScheduledOpenActivation: b.rollbackScheduledOpenActivation, markRemoteTurn: b.markRemoteTurn, onSessionEvent: b.onSessionEvent, isScheduledRunSession: b.isScheduledRunSession, defineSubscriptionStateProperty: b.defineSubscriptionStateProperty, copySubscriptionStateObject: b.copySubscriptionStateObject, subscriptionStateValue: b.subscriptionStateValue, rerenderFromMessagesCore: b.rerenderFromMessagesCore, liveChatItemKeptForHydration: b.liveChatItemKeptForHydration, loadScheduledTaskTemplateSources: b.loadScheduledTaskTemplateSources, rememberScheduledTaskTemplateSource: b.rememberScheduledTaskTemplateSource, attachScheduledTaskTemplateSource: b.attachScheduledTaskTemplateSource, attachAndPruneScheduledTaskTemplateSources: b.attachAndPruneScheduledTaskTemplateSources, upsertScheduledTask: b.upsertScheduledTask, applyScheduledRunViewed: b.applyScheduledRunViewed, invalidateScheduledTaskReads: b.invalidateScheduledTaskReads, invalidateScheduledRecentRuns: b.invalidateScheduledRecentRuns, invalidateScheduledRecentRunsForSession: b.invalidateScheduledRecentRunsForSession, scheduleScheduledRunRefresh: b.scheduleScheduledRunRefresh, scheduledTaskErrorText: b.scheduledTaskErrorText, setScheduledTaskError: b.setScheduledTaskError, dismissScheduledTaskError: b.dismissScheduledTaskError, clearScheduledTaskLoadError: b.clearScheduledTaskLoadError, beginScheduledTaskLoad: b.beginScheduledTaskLoad, endScheduledTaskLoad: b.endScheduledTaskLoad, scheduledTaskRequestStamp: b.scheduledTaskRequestStamp, isCurrentScheduledTaskRequest: b.isCurrentScheduledTaskRequest, selectScheduledTask: b.selectScheduledTask, clearScheduledTaskSelection: b.clearScheduledTaskSelection, extractBalancedJsonObject: b.extractBalancedJsonObject, parseLooseJsonObject: b.parseLooseJsonObject, parseScheduledTaskDraftFromText: b.parseScheduledTaskDraftFromText, normalizeScheduledTaskDraft: b.normalizeScheduledTaskDraft, activeScheduledTaskModelConfig: b.activeScheduledTaskModelConfig, lockScheduledTaskDraftModel: b.lockScheduledTaskDraftModel, scheduledTaskInputFromDraft: b.scheduledTaskInputFromDraft, autoCreateScheduledTaskDraft: b.autoCreateScheduledTaskDraft, createScheduledTask: b.createScheduledTask, loadScheduledTasks: b.loadScheduledTasks, readScheduledTask: b.readScheduledTask, mergeScheduledTaskRecentRuns: b.mergeScheduledTaskRecentRuns, loadScheduledTaskRuns: b.loadScheduledTaskRuns, loadScheduledTaskRecentRuns: b.loadScheduledTaskRecentRuns, refreshScheduledTaskData: b.refreshScheduledTaskData, refreshScheduledRunShortcutUntilLinked: b.refreshScheduledRunShortcutUntilLinked, upsertScheduledTaskRun: b.upsertScheduledTaskRun, runScheduledTaskAction: b.runScheduledTaskAction, updateScheduledTask: b.updateScheduledTask, pauseScheduledTask: b.pauseScheduledTask, resumeScheduledTask: b.resumeScheduledTask, toggleScheduledTaskPinned: b.toggleScheduledTaskPinned, deleteScheduledTask: b.deleteScheduledTask, runScheduledTaskNow: b.runScheduledTaskNow, startScheduledTaskChat: b.startScheduledTaskChat, toolCallAlreadyFinished: b.toolCallAlreadyFinished, hasChatItemForTool: b.hasChatItemForTool, messageHasToolBlock: b.messageHasToolBlock, toolCallAlreadyStarted: b.toolCallAlreadyStarted, sendMessageToSession: b.sendMessageToSession, addSystemItem: b.addSystemItem, addAuthoritySyncNotice: b.addAuthoritySyncNotice, compactPruneRollupText: b.compactPruneRollupText, removeCompactionStartItem: b.removeCompactionStartItem, addOrMergePruneCompaction: b.addOrMergePruneCompaction, timeStr: b.timeStr, createNewSession: b.createNewSession, reportSessionSwitchFailure: b.reportSessionSwitchFailure, mergeHydratedMessages: b.mergeHydratedMessages, hydratedChatItemKey: b.hydratedChatItemKey, switchToSession: b.switchToSession, openScheduledRunChat: b.openScheduledRunChat, exitScheduledRunChat: b.exitScheduledRunChat, recentScheduledRunForSession: b.recentScheduledRunForSession, leaveSessionView: b.leaveSessionView, applyDeletedSession: b.applyDeletedSession, onSessionDeleted: b.onSessionDeleted, renameSession: b.renameSession, toggleSessionPinned: b.toggleSessionPinned, archiveSession: b.archiveSession, restoreArchivedSession: b.restoreArchivedSession, toolResultText: b.toolResultText, toolResultDisplayContent: b.toolResultDisplayContent, escapeHtml: b.escapeHtml, renderMarkdown: b.renderMarkdown, escapeHtml: b.escapeHtml, renderMarkdown: b.renderMarkdown, parsePlanSnapshot: b.parsePlanSnapshot, parseUserAnswers: b.parseUserAnswers, parseCarefulBlocked: b.parseCarefulBlocked, userMessageInputProvenance: b.userMessageInputProvenance, isInternalUserMessageProvenance: b.isInternalUserMessageProvenance, isShellExecutionTool: b.isShellExecutionTool, utf8Length: b.utf8Length, formatShellSnapshot: b.formatShellSnapshot, shellCommandForItem: b.shellCommandForItem, shellSnapshotKey: b.shellSnapshotKey, terminalShellHistoryMatch: b.terminalShellHistoryMatch, applyShellSnapshots: b.applyShellSnapshots, scheduleShellPoll: b.scheduleShellPoll, runShellPoll: b.runShellPoll, patchLastItem: b.patchLastItem, hasUnresolvedItem: b.hasUnresolvedItem, basename: b.basename, isAbsPath: b.isAbsPath, normalizedPath: b.normalizedPath, applyWorkspaceReboundMark: b.applyWorkspaceReboundMark, sessionRecentlyRebound: b.sessionRecentlyRebound, rebaseArtifactPathsForRebind: b.rebaseArtifactPathsForRebind, pathIsRebindStale: b.pathIsRebindStale, noteArtifactChange: b.noteArtifactChange, isSharedMcpArtifactPath: b.isSharedMcpArtifactPath, artifactBelongsToSession: b.artifactBelongsToSession, filterSessionArtifacts: b.filterSessionArtifacts, isDeliverable: b.isDeliverable, markTurnDirtyArtifact: b.markTurnDirtyArtifact, untrackArtifact: b.untrackArtifact, findPresentedArtifact: b.findPresentedArtifact, updatePresentedArtifact: b.updatePresentedArtifact, pushArtifactPath: b.pushArtifactPath, extractArtifactPath: b.extractArtifactPath, fileMutationAction: b.fileMutationAction, composePlanMarkdown: b.composePlanMarkdown, isBusyFor: b.isBusyFor, formatAttachmentDisplayText: b.formatAttachmentDisplayText, stripMentionBlockForComposerRestore: b.stripMentionBlockForComposerRestore, makeQueuedMessage: b.makeQueuedMessage, rebuiltQueuedPayload: b.rebuiltQueuedPayload, rebuiltQueuedMetaPayload: b.rebuiltQueuedMetaPayload, getComposerDraft: b.getComposerDraft, setComposerDraft: b.setComposerDraft, prefillComposer: b.prefillComposer, inspectPinvou: b.inspectPinvou, recordPinvouReview: b.recordPinvouReview, dismissPinvouReview: b.dismissPinvouReview, summonPinvou: b.summonPinvou, resolvePinvouReview: b.resolvePinvouReview, persistPinvouReviews: b.persistPinvouReviews, planCardHydrationKey: b.planCardHydrationKey, reasoningEventIndex: b.reasoningEventIndex, streamingReasoningItem: b.streamingReasoningItem, finalizeStreamingReasoning: b.finalizeStreamingReasoning, visibleUserTurnIndex: b.visibleUserTurnIndex, latestOpenTimelineStart: b.latestOpenTimelineStart, recordTurnStarted: b.recordTurnStarted, cancelPendingStreamNotify: b.cancelPendingStreamNotify, preserveInterruptedAssistantPresentation: b.preserveInterruptedAssistantPresentation, startReasoningBlock: b.startReasoningBlock, appendReasoningBlock: b.appendReasoningBlock, flushAssistantMessageToHistory: b.flushAssistantMessageToHistory, isPresentArtifactTool: b.isPresentArtifactTool, artifactPathFromToolOutput: b.artifactPathFromToolOutput, shouldUseToolOutputAsArtifact: b.shouldUseToolOutputAsArtifact, presentArtifactAbsPath: b.presentArtifactAbsPath, parseToolResultPayload: b.parseToolResultPayload, numOr0: b.numOr0, adjustCounters: b.adjustCounters, startMonitorPolling: b.startMonitorPolling, stopMonitorPolling: b.stopMonitorPolling, pollMonitor, loadSettings: b.loadSettings, loadSelectedPet: b.loadSelectedPet, setSelectedPet: b.setSelectedPet, enqueueSettingsWrite: b.enqueueSettingsWrite, submitFeedback: b.submitFeedback, discoverLocalVllm: b.discoverLocalVllm, getImageInputCapability: b.getImageInputCapability, loadModels: b.loadModels, revealModelApiKey: b.revealModelApiKey, switchModel: b.switchModel, testModelConnection: b.testModelConnection, testImageInputCapability: b.testImageInputCapability, refreshSuperPerm: b.refreshSuperPerm, setModeLane: b.setModeLane, patchItemById: b.patchItemById, runOnSession: b.runOnSession, addSystemItemFor: b.addSystemItemFor, patchItemByIdFor: b.patchItemByIdFor, handleMemoryWrite: b.handleMemoryWrite, orderedMemoryWarnings: b.orderedMemoryWarnings, applyMemoryProfileState: b.applyMemoryProfileState, applyMemoryWriteState: b.applyMemoryWriteState, upsertMemoryValue: b.upsertMemoryValue, rehydratePendingMemoryCandidates: b.rehydratePendingMemoryCandidates, discardStaleLoad: b.discardStaleLoad, loadOrganizeHistory: b.loadOrganizeHistory, applyMemoryOverview: b.applyMemoryOverview, loadMemoryOverview: b.loadMemoryOverview, updateMemoryItem: b.updateMemoryItem, deleteMemoryItem: b.deleteMemoryItem, resolveMemoryCandidate: b.resolveMemoryCandidate, startThinking: b.startThinking, thinkingTool: b.thinkingTool, thinkingIdle: b.thinkingIdle, stopThinking: b.stopThinking, isActionablePlanCard: b.isActionablePlanCard, setPlanModeNext: b.setPlanModeNext, planStuckReplan: b.planStuckReplan, submitUserInput: b.submitUserInput, openContainingFolder: b.openContainingFolder, revealSessionFolder: b.revealSessionFolder, openScheduledTaskFolder: b.openScheduledTaskFolder, deliverableCategory: b.deliverableCategory, sessionTitleById: b.sessionTitleById, currentMemoryArtifacts: b.currentMemoryArtifacts, conversationAttachmentArgs: b.conversationAttachmentArgs, pickAndAttach: b.pickAndAttach, loadPersonas: b.loadPersonas, createPersona: b.createPersona, updatePersona: b.updatePersona, deletePersona: b.deletePersona, personaName: b.personaName, recordPersonaEvent: b.recordPersonaEvent, equipPersona: b.equipPersona, postCardCreatorIntro: b.postCardCreatorIntro, normalizeMountedCollections: b.normalizeMountedCollections, applyMountedCollections: b.applyMountedCollections, mountedCollectionTargetAtEnqueue: b.mountedCollectionTargetAtEnqueue, updateMountedCollections: b.updateMountedCollections, mountCollection: b.mountCollection, setCollectionEnabled: b.setCollectionEnabled, removeCollection: b.removeCollection, checkForUpdate: b.checkForUpdate, checkDependencies: b.checkDependencies, setVoiceInputStatus: b.setVoiceInputStatus, emitVoiceDiagnostic: b.emitVoiceDiagnostic, voiceFlowError: b.voiceFlowError, requestVoiceMedia: b.requestVoiceMedia, mergeFloatChunks: b.mergeFloatChunks, downsamplePcm: b.downsamplePcm, closeVoiceAsrSetup: b.closeVoiceAsrSetup, downloadKbModel: b.downloadKbModel, cancelVoiceInput: b.cancelVoiceInput, appendVoiceText: b.appendVoiceText, trackVoiceTerminal: b.trackVoiceTerminal, rememberVoiceOperation: b.rememberVoiceOperation, getVoiceOperationId: b.getVoiceOperationId, beginVoiceSubmission: b.beginVoiceSubmission, voiceOperationSessionId: b.voiceOperationSessionId, hasVoiceSubmissionPending: b.hasVoiceSubmissionPending, rebindVoiceOperationToDraft: b.rebindVoiceOperationToDraft, completeVoiceSubmission: b.completeVoiceSubmission, encodeWav: b.encodeWav, loadEffectiveModelConfig: b.loadEffectiveModelConfig });},
    "tauriMain": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ authoritySyncBufferSnapshot: b.authoritySyncBufferSnapshot, bt: b.bt, textMatchesBtKey: b.textMatchesBtKey, isDefaultChatTitle: b.isDefaultChatTitle, normalizePinvouScene: b.normalizePinvouScene, pinvouSceneStorageKey: b.pinvouSceneStorageKey, normalizePinvouSceneEvents: b.normalizePinvouSceneEvents, loadPinvouSceneEventsForSession: b.loadPinvouSceneEventsForSession, savePinvouSceneEventsForSession: b.savePinvouSceneEventsForSession, recordPinvouSceneForMessage: b.recordPinvouSceneForMessage, pinvouSceneForMessagePos: b.pinvouSceneForMessagePos, markRemoteTurn: b.markRemoteTurn, onSessionEvent: b.onSessionEvent, isScheduledRunSession: b.isScheduledRunSession, rerenderFromMessagesCore: b.rerenderFromMessagesCore, liveChatItemKeptForHydration: b.liveChatItemKeptForHydration, planCardHydrationKey: b.planCardHydrationKey, defineSubscriptionStateProperty: b.defineSubscriptionStateProperty, copySubscriptionStateObject: b.copySubscriptionStateObject, subscriptionStateValue: b.subscriptionStateValue, toolResultText: b.toolResultText, toolResultDisplayContent: b.toolResultDisplayContent, escapeHtml: b.escapeHtml, renderMarkdown: b.renderMarkdown, parsePlanSnapshot: b.parsePlanSnapshot, parseUserAnswers: b.parseUserAnswers, parseCarefulBlocked: b.parseCarefulBlocked, userMessageInputProvenance: b.userMessageInputProvenance, isInternalUserMessageProvenance: b.isInternalUserMessageProvenance, patchLastItem: b.patchLastItem, hasUnresolvedItem: b.hasUnresolvedItem, composePlanMarkdown: b.composePlanMarkdown });
    },
    "tauriSessions": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ getBuffer: b.getBuffer, freshBuffer: b.freshBuffer, stashEvictedSessionDraft: b.stashEvictedSessionDraft, hydratedMessageKey: b.hydratedMessageKey, mergeHydratedArtifacts: b.mergeHydratedArtifacts, isProtectedScheduledBuffer: b.isProtectedScheduledBuffer, touchSessionBuffer: b.touchSessionBuffer, registerScheduledRunOwner: b.registerScheduledRunOwner, scheduledRunOwnerVisibleRank: b.scheduledRunOwnerVisibleRank, scheduledRunOwnerPriority: b.scheduledRunOwnerPriority, pruneScheduledRunSessionOwners: b.pruneScheduledRunSessionOwners, isScheduledRunTerminal: b.isScheduledRunTerminal, rememberScheduledRunOwner: b.rememberScheduledRunOwner, scheduledRunBuffer: b.scheduledRunBuffer, markScheduledInitialTurnActive: b.markScheduledInitialTurnActive, markScheduledInitialTurnTerminal: b.markScheduledInitialTurnTerminal, beginScheduledOpenActivation: b.beginScheduledOpenActivation, rollbackScheduledOpenActivation: b.rollbackScheduledOpenActivation, createNewSession: b.createNewSession, reportSessionSwitchFailure: b.reportSessionSwitchFailure, mergeHydratedMessages: b.mergeHydratedMessages, hydratedChatItemKey: b.hydratedChatItemKey, switchToSession: b.switchToSession, openScheduledRunChat: b.openScheduledRunChat, exitScheduledRunChat: b.exitScheduledRunChat, recentScheduledRunForSession: b.recentScheduledRunForSession, leaveSessionView: b.leaveSessionView, applyDeletedSession: b.applyDeletedSession, onSessionDeleted: b.onSessionDeleted, renameSession: b.renameSession, toggleSessionPinned: b.toggleSessionPinned, archiveSession: b.archiveSession, restoreArchivedSession: b.restoreArchivedSession, applyWorkspaceReboundMark: b.applyWorkspaceReboundMark, rememberDraftWorkspaceRecent: b.rememberDraftWorkspaceRecent, maybePruneFailedWorkspaceRecent: b.maybePruneFailedWorkspaceRecent });
    },
    "tauriScheduled": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ loadScheduledTaskTemplateSources: b.loadScheduledTaskTemplateSources, rememberScheduledTaskTemplateSource: b.rememberScheduledTaskTemplateSource, attachScheduledTaskTemplateSource: b.attachScheduledTaskTemplateSource, attachAndPruneScheduledTaskTemplateSources: b.attachAndPruneScheduledTaskTemplateSources, upsertScheduledTask: b.upsertScheduledTask, applyScheduledRunViewed: b.applyScheduledRunViewed, invalidateScheduledTaskReads: b.invalidateScheduledTaskReads, invalidateScheduledRecentRuns: b.invalidateScheduledRecentRuns, invalidateScheduledRecentRunsForSession: b.invalidateScheduledRecentRunsForSession, scheduleScheduledRunRefresh: b.scheduleScheduledRunRefresh, scheduledTaskErrorText: b.scheduledTaskErrorText, setScheduledTaskError: b.setScheduledTaskError, dismissScheduledTaskError: b.dismissScheduledTaskError, clearScheduledTaskLoadError: b.clearScheduledTaskLoadError, beginScheduledTaskLoad: b.beginScheduledTaskLoad, endScheduledTaskLoad: b.endScheduledTaskLoad, scheduledTaskRequestStamp: b.scheduledTaskRequestStamp, isCurrentScheduledTaskRequest: b.isCurrentScheduledTaskRequest, selectScheduledTask: b.selectScheduledTask, clearScheduledTaskSelection: b.clearScheduledTaskSelection, extractBalancedJsonObject: b.extractBalancedJsonObject, parseLooseJsonObject: b.parseLooseJsonObject, parseScheduledTaskDraftFromText: b.parseScheduledTaskDraftFromText, normalizeScheduledTaskDraft: b.normalizeScheduledTaskDraft, activeScheduledTaskModelConfig: b.activeScheduledTaskModelConfig, lockScheduledTaskDraftModel: b.lockScheduledTaskDraftModel, scheduledTaskInputFromDraft: b.scheduledTaskInputFromDraft, autoCreateScheduledTaskDraft: b.autoCreateScheduledTaskDraft, createScheduledTask: b.createScheduledTask, loadScheduledTasks: b.loadScheduledTasks, readScheduledTask: b.readScheduledTask, mergeScheduledTaskRecentRuns: b.mergeScheduledTaskRecentRuns, loadScheduledTaskRuns: b.loadScheduledTaskRuns, loadScheduledTaskRecentRuns: b.loadScheduledTaskRecentRuns, refreshScheduledTaskData: b.refreshScheduledTaskData, refreshScheduledRunShortcutUntilLinked: b.refreshScheduledRunShortcutUntilLinked, upsertScheduledTaskRun: b.upsertScheduledTaskRun, runScheduledTaskAction: b.runScheduledTaskAction, updateScheduledTask: b.updateScheduledTask, pauseScheduledTask: b.pauseScheduledTask, resumeScheduledTask: b.resumeScheduledTask, toggleScheduledTaskPinned: b.toggleScheduledTaskPinned, deleteScheduledTask: b.deleteScheduledTask, runScheduledTaskNow: b.runScheduledTaskNow, startScheduledTaskChat: b.startScheduledTaskChat });
    },
    "tauriChat": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({getComposerDraft: b.getComposerDraft, setComposerDraft: b.setComposerDraft, summonPinvou: b.summonPinvou, resolvePinvouReview: b.resolvePinvouReview, flushAssistantMessageToHistory: b.flushAssistantMessageToHistory, messageHasToolBlock: b.messageHasToolBlock, toolCallAlreadyStarted: b.toolCallAlreadyStarted, sendMessageToSession: b.sendMessageToSession, toolCallAlreadyFinished: b.toolCallAlreadyFinished, hasChatItemForTool: b.hasChatItemForTool, addSystemItem: b.addSystemItem, addAuthoritySyncNotice: b.addAuthoritySyncNotice, compactPruneRollupText: b.compactPruneRollupText, removeCompactionStartItem: b.removeCompactionStartItem, addOrMergePruneCompaction: b.addOrMergePruneCompaction, timeStr: b.timeStr, isBusyFor: b.isBusyFor, formatAttachmentDisplayText: b.formatAttachmentDisplayText, stripMentionBlockForComposerRestore: b.stripMentionBlockForComposerRestore, makeQueuedMessage: b.makeQueuedMessage, rebuiltQueuedPayload: b.rebuiltQueuedPayload, rebuiltQueuedMetaPayload: b.rebuiltQueuedMetaPayload, prefillComposer: b.prefillComposer, inspectPinvou: b.inspectPinvou, recordPinvouReview: b.recordPinvouReview, dismissPinvouReview: b.dismissPinvouReview, persistPinvouReviews: b.persistPinvouReviews});
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
      return Object.freeze({ basename: b.basename, isAbsPath: b.isAbsPath, normalizedPath: b.normalizedPath, sessionRecentlyRebound: b.sessionRecentlyRebound, rebaseArtifactPathsForRebind: b.rebaseArtifactPathsForRebind, pathIsRebindStale: b.pathIsRebindStale, noteArtifactChange: b.noteArtifactChange, isSharedMcpArtifactPath: b.isSharedMcpArtifactPath, artifactBelongsToSession: b.artifactBelongsToSession, filterSessionArtifacts: b.filterSessionArtifacts, isDeliverable: b.isDeliverable, markTurnDirtyArtifact: b.markTurnDirtyArtifact, untrackArtifact: b.untrackArtifact, findPresentedArtifact: b.findPresentedArtifact, updatePresentedArtifact: b.updatePresentedArtifact, pushArtifactPath: b.pushArtifactPath, extractArtifactPath: b.extractArtifactPath, fileMutationAction: b.fileMutationAction, isPresentArtifactTool: b.isPresentArtifactTool, artifactPathFromToolOutput: b.artifactPathFromToolOutput, shouldUseToolOutputAsArtifact: b.shouldUseToolOutputAsArtifact, presentArtifactAbsPath: b.presentArtifactAbsPath, parseToolResultPayload: b.parseToolResultPayload });
    },
    "web:247496": nestedRestoreUiTurnState,
    "tauriChat:31666": nestedRestoreUiTurnState,
    "tauriChatEvents": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ reasoningEventIndex: b.reasoningEventIndex, streamingReasoningItem: b.streamingReasoningItem, finalizeStreamingReasoning: b.finalizeStreamingReasoning, normalizeMountedCollections: b.normalizeMountedCollections, visibleUserTurnIndex: b.visibleUserTurnIndex, latestOpenTimelineStart: b.latestOpenTimelineStart, recordTurnStarted: b.recordTurnStarted, cancelPendingStreamNotify: b.cancelPendingStreamNotify, preserveInterruptedAssistantPresentation: b.preserveInterruptedAssistantPresentation, startReasoningBlock: b.startReasoningBlock, appendReasoningBlock: b.appendReasoningBlock, flushAssistantMessageToHistory: b.flushAssistantMessageToHistory });
    },
    "tauriMonitor": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ numOr0: b.numOr0, adjustCounters: b.adjustCounters, startMonitorPolling: b.startMonitorPolling, stopMonitorPolling: b.stopMonitorPolling, pollMonitor: b.pollMonitor });
    },
    "tauriSettings": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ loadSettings: b.loadSettings, loadSelectedPet: b.loadSelectedPet, setSelectedPet: b.setSelectedPet, enqueueSettingsWrite: b.enqueueSettingsWrite, submitFeedback: b.submitFeedback, discoverLocalVllm: b.discoverLocalVllm, getImageInputCapability: b.getImageInputCapability, loadModels: b.loadModels, revealModelApiKey: b.revealModelApiKey, switchModel: b.switchModel, testModelConnection: b.testModelConnection, testImageInputCapability: b.testImageInputCapability, loadEffectiveModelConfig: b.loadEffectiveModelConfig });
    },
    "tauriInteraction": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ refreshSuperPerm: b.refreshSuperPerm, setModeLane: b.setModeLane, patchItemById: b.patchItemById, runOnSession: b.runOnSession, addSystemItemFor: b.addSystemItemFor, patchItemByIdFor: b.patchItemByIdFor, flushAssistantMessageToHistory: b.flushAssistantMessageToHistory, startThinking: b.startThinking, thinkingTool: b.thinkingTool, thinkingIdle: b.thinkingIdle, stopThinking: b.stopThinking, isActionablePlanCard: b.isActionablePlanCard, setPlanModeNext: b.setPlanModeNext, planStuckReplan: b.planStuckReplan, submitUserInput: b.submitUserInput });
    },
    "tauriMemory": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ handleMemoryWrite: b.handleMemoryWrite, orderedMemoryWarnings: b.orderedMemoryWarnings, applyMemoryProfileState: b.applyMemoryProfileState, applyMemoryWriteState: b.applyMemoryWriteState, upsertMemoryValue: b.upsertMemoryValue, rehydratePendingMemoryCandidates: b.rehydratePendingMemoryCandidates, discardStaleLoad: b.discardStaleLoad, loadOrganizeHistory: b.loadOrganizeHistory, applyMemoryOverview: b.applyMemoryOverview, loadMemoryOverview: b.loadMemoryOverview, updateMemoryItem: b.updateMemoryItem, deleteMemoryItem: b.deleteMemoryItem, resolveMemoryCandidate: b.resolveMemoryCandidate });
    },
    "tauriArtifacts": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ openContainingFolder: b.openContainingFolder, revealSessionFolder: b.revealSessionFolder, openScheduledTaskFolder: b.openScheduledTaskFolder, deliverableCategory: b.deliverableCategory, sessionTitleById: b.sessionTitleById, currentMemoryArtifacts: b.currentMemoryArtifacts, conversationAttachmentArgs: b.conversationAttachmentArgs, pickAndAttach: b.pickAndAttach });
    },
    "tauriPersonas": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ loadPersonas: b.loadPersonas, createPersona: b.createPersona, updatePersona: b.updatePersona, deletePersona: b.deletePersona, personaName: b.personaName, recordPersonaEvent: b.recordPersonaEvent, equipPersona: b.equipPersona, postCardCreatorIntro: b.postCardCreatorIntro, normalizeMountedCollections: b.normalizeMountedCollections, applyMountedCollections: b.applyMountedCollections, mountedCollectionTargetAtEnqueue: b.mountedCollectionTargetAtEnqueue, updateMountedCollections: b.updateMountedCollections, mountCollection: b.mountCollection, setCollectionEnabled: b.setCollectionEnabled, removeCollection: b.removeCollection });
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
      return Object.freeze({ setVoiceInputStatus: b.setVoiceInputStatus, emitVoiceDiagnostic: b.emitVoiceDiagnostic, voiceFlowError: b.voiceFlowError, requestVoiceMedia: b.requestVoiceMedia, mergeFloatChunks: b.mergeFloatChunks, downsamplePcm: b.downsamplePcm, closeVoiceAsrSetup: b.closeVoiceAsrSetup, cancelVoiceInput: b.cancelVoiceInput, appendVoiceText: b.appendVoiceText, trackVoiceTerminal: b.trackVoiceTerminal, rememberVoiceOperation: b.rememberVoiceOperation, getVoiceOperationId: b.getVoiceOperationId, beginVoiceSubmission: b.beginVoiceSubmission, voiceOperationSessionId: b.voiceOperationSessionId, hasVoiceSubmissionPending: b.hasVoiceSubmissionPending, rebindVoiceOperationToDraft: b.rebindVoiceOperationToDraft, completeVoiceSubmission: b.completeVoiceSubmission, encodeWav: b.encodeWav });
    },
    "tauriKnowledgeModel": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ downloadKbModel: b.downloadKbModel });
    },
    "auxChat": function (deps) {
      const b = sharedBridgeBase(ensureCells(deps));
      return Object.freeze({ auxChatEnsure: b.auxChatEnsure, auxChatSend: b.auxChatSend, auxChatSnapshot: b.auxChatSnapshot, auxChatReset: b.auxChatReset });
    },
  };

  // F-FE13-2（FE14 wave 5）：assistant_done 时间线记录构建器——此前 tauri
  // chat-events.js 与 web turn-terminal.js 各存一份逐字副本。无 lane deps，
  // 挂在 root.PinvouBridgeShared 静态面上（与 STATE_SLICE_FIELDS 同一模式），
  // web 的独立 classic 模块 turn-terminal.js 也能直接转发。
  function buildAssistantDoneTimelineRecord(state, openStart, payload) {
    const turnId = state.activeTurnTimelineId || (openStart && openStart.turn_id);
    if (!turnId) return null;
    if (payload && payload.error && !(payload.user_error || payload.userError) &&
        window.PinvouBridgeMessages &&
        typeof window.PinvouBridgeMessages.modelServiceUserError === "function") {
      const userError = window.PinvouBridgeMessages.modelServiceUserError(payload, state);
      if (userError) {
        payload.user_error = userError;
        payload.userError = userError;
      }
    }
    const timestamp = Date.now();
    const start = openStart || (state.turnTimeline || []).find(function (event) {
      return event && event.turn_id === turnId && event.event === "user_start";
    });
    const record = {
      turn_id: turnId,
      event: "assistant_done",
      timestamp,
      ts: new Date(timestamp).toISOString(),
      status: payload && payload.status || (payload && payload.error ? "Failed" : "Completed"),
      error: payload && payload.error || null,
      user_error: payload && (payload.user_error || payload.userError) || null,
      ui_turn_index: start && start.ui_turn_index,
    };
    state.turnTimeline = [...(state.turnTimeline || []), record];
    state.activeTurnTimelineId = null;
    // The return value drives the terminal-bubble hiding decision: only a
    // timeline terminal record that was actually written with an error
    // lets the timeline error card take over (hiding otherwise = silent
    // swallow).
    return record;
  }

  root.PinvouBridgeShared = Object.freeze({
    create: function (cluster, deps) {
      const factory = CLUSTERS[cluster];
      if (!factory) throw new Error("Unknown bridge shared cluster: " + cluster);
      return factory(deps);
    },
    // FE14 wave 5：双 lane 共享的桥接状态切片字段注册表（见上方注释）。
    STATE_SLICE_FIELDS,
    // F-FE13-2：assistant_done 时间线记录构建器（无 deps，静态共享）。
    buildAssistantDoneTimelineRecord,
  });
// eslint-disable-next-line unicorn/no-this-outside-of-class -- UMD root reference
})(typeof window === "undefined" ? this : window);
