/** Session working sets, switching, hydration, and lifecycle operations. */
(function (root) {
  "use strict";
  var registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry.sessions = function (context) {
    var state = context.state;
    var invoke = context.invoke;
    var notify = context.notify;
    var sessionStates = context.sessionStates;
    var scheduledRunSessionOwners = context.scheduledRunSessionOwners;
    var personaPlaceholderTitles = context.personaPlaceholderTitles;
    var runSyncOnSession = context.runSyncOnSession;
    var persistMessagesFor = context.persistMessagesFor;
    var resetPendingAssistant = context.resetPendingAssistant;
    var stopThinking = context.stopThinking;
    var rerenderFromMessages = context.rerenderFromMessages;
    var syncModeState = context.syncModeState;
    var syncActivePersona = context.syncActivePersona;
    var syncMountedCollection = context.syncMountedCollection;
    var reconcileArtifacts = context.reconcileArtifacts;
    var loadSessionModel = context.loadSessionModel;
    var clearScheduledTaskSelection = context.clearScheduledTaskSelection;
    var invalidateScheduledRecentRunsForSession = context.invalidateScheduledRecentRunsForSession;
    var refreshHistoryListForRun = context.refreshHistoryListForRun;
    var addSystemItem = context.addSystemItem;
    var turnUsageDirty = context.turnUsageDirty;
    var basename = context.basename;
    var isAbsPath = context.isAbsPath;
    var filterSessionArtifacts = context.filterSessionArtifacts;
    var scheduleShellPoll = context.scheduleShellPoll;
    var bt = context.bt;
    var setScheduledTaskError = context.setScheduledTaskError;
    var userMessageDisplayText = context.userMessageDisplayText;
    var loadMemoryOverview = context.loadMemoryOverview;
    var isScheduledRunSession = context.isScheduledRunSession;
    var invalidateScheduledTaskReads = context.invalidateScheduledTaskReads;
    var applyScheduledRunViewed = context.applyScheduledRunViewed;
    var loadScheduledTaskRecentRuns = context.loadScheduledTaskRecentRuns;
    var MAX_SCHEDULED_SESSION_BUFFERS = 64;
    var MAX_SCHEDULED_RUN_SESSION_OWNERS = 64;
    var sessionBufferTouchClock = 0;
    var scheduledRunOwnerTouchClock = 0;
    var scheduledRunOpenInFlight = Object.create(null);
    var sessionSwitchRequestToken = 0;
  function freshBuffer() {
    return {
      messages: [], chatItems: [], turnTimeline: [], activeTurnTimelineId: null, personaEvents: [], pinvouReviews: [], artifacts: [], busy: false, queued: [],
      loadedFromDisk: false,
      localTurnOwned: false,
      remoteTurnActive: false,
      remoteTerminalSeen: false,
      remoteAdmissionKeys: [],
      deferredRemoteUserEvent: null,
      remoteBaselineMessageCount: null,
      remoteBaselineTrusted: false,
      remoteExpectedAssistantKey: "",
      sessionRevision: "",
      planSnapshot: { plan: null, todos: null },
      modeState: { mode: "yolo" },
      thinking: { active: false, phase: "thinking", toolName: "", startedAt: 0 },
      tokens: { input: 0, max: state.tokens.max },
      activePersona: null, // 卡片池: 该 session 加持的专家面具(挂件用)
      mountedCollection: null, // 知识库: 该 session 挂载的知识集 id 或 null
      scheduledTaskDraft: null,
      scheduledRunSession: false,
      scheduledInitialTurnPhase: null,
      lastTouched: 0,

      stream: {
        currentStreamText: "", currentStreamId: 0, pendingAssistantText: "",
        pendingAssistantBlocks: [], itemIdSeq: 0, toolMeta: {},
      },
    };
  }
  function getBuffer(id) {
    if (!id) return null;
    if (!sessionStates[id]) sessionStates[id] = freshBuffer();
    return touchSessionBuffer(id, sessionStates[id], id.indexOf("sched-") === 0);
  }
  function isProtectedScheduledBuffer(id, buf) {
    return id === state.activeSessionId ||
      !!buf.busy ||
      buf.scheduledInitialTurnPhase === "active" ||
      !!(buf.queued && buf.queued.length) ||
      !!(state.scheduledRunContext && state.scheduledRunContext.sessionId === id) ||
      state.scheduledTaskCreationSessionId === id;
  }
  function pruneScheduledSessionBuffers(keepId) {
    var scheduledIds = Object.keys(sessionStates).filter(function (id) {
      return !!sessionStates[id].scheduledRunSession;
    });
    var overflow = scheduledIds.length - MAX_SCHEDULED_SESSION_BUFFERS;
    if (overflow <= 0) return;
    scheduledIds.sort(function (left, right) {
      var delta = (sessionStates[left].lastTouched || 0) - (sessionStates[right].lastTouched || 0);
      return delta || left.localeCompare(right);
    });
    for (var i = 0; i < scheduledIds.length && overflow > 0; i++) {
      var id = scheduledIds[i];
      var buf = sessionStates[id];
      if (!buf || id === keepId || isProtectedScheduledBuffer(id, buf)) continue;
      delete sessionStates[id];
      delete turnUsageDirty[id];
      pruneScheduledRunSessionOwner(id);
      overflow -= 1;
    }
  }
  function touchSessionBuffer(id, buf, scheduled) {
    if (!buf) return null;
    if (scheduled) buf.scheduledRunSession = true;
    buf.lastTouched = ++sessionBufferTouchClock;
    if (buf.scheduledRunSession) pruneScheduledSessionBuffers(id);
    return buf;
  }
  function purgeSessionBuffer(id) {
    if (typeof id !== "string" || !id) return;
    delete sessionStates[id];
    delete turnUsageDirty[id];
    delete personaPlaceholderTitles[id];
    delete scheduledRunSessionOwners[id];
    if (state.scheduledRunContext && state.scheduledRunContext.sessionId === id) {
      state.scheduledRunContext = null;
    }
    if (state.scheduledTaskCreationSessionId === id) {
      state.scheduledTaskCreationSessionId = null;
    }
    if (state.activeSessionId === id) {
      state.activeSessionId = null;
      loadWorkingSetFrom(freshBuffer());
    }
  }
  function registerScheduledRunOwner(id, phase) {
    if (typeof id !== "string" || !id) return null;
    var owner = scheduledRunSessionOwners[id];
    if (!owner) owner = scheduledRunSessionOwners[id] = { phase: null, lastTouched: 0 };
    if (owner.phase !== "terminal" && phase) owner.phase = phase;
    owner.lastTouched = ++scheduledRunOwnerTouchClock;
    pruneScheduledRunSessionOwners();
    return owner;
  }
  function scheduledRunOwnerVisibleRank(id) {
    var runs = state.scheduledTaskRuns || [];
    for (var i = 0; i < runs.length; i++) {
      if (runs[i] && runs[i].sessionId === id) return i;
    }
    return -1;
  }
  function scheduledRunOwnerPriority(id) {
    if (id === state.activeSessionId ||
        (state.scheduledRunContext && state.scheduledRunContext.sessionId === id)) return 3;
    if (scheduledRunOwnerVisibleRank(id) >= 0) return 2;
    return 1;
  }
  function isProtectedScheduledRunOwner(id) {
    return scheduledRunOwnerPriority(id) > 1;
  }
  function pruneScheduledRunSessionOwner(id) {
    if (!scheduledRunSessionOwners[id] || isProtectedScheduledRunOwner(id, null)) return;
    delete scheduledRunSessionOwners[id];
  }
  function pruneScheduledRunSessionOwners() {
    var ids = Object.keys(scheduledRunSessionOwners);
    if (ids.length <= MAX_SCHEDULED_RUN_SESSION_OWNERS) return;
    ids.sort(function (left, right) {
      var priorityDelta = scheduledRunOwnerPriority(right) - scheduledRunOwnerPriority(left);
      if (priorityDelta) return priorityDelta;
      var leftVisibleRank = scheduledRunOwnerVisibleRank(left);
      var rightVisibleRank = scheduledRunOwnerVisibleRank(right);
      if (leftVisibleRank >= 0 || rightVisibleRank >= 0) {
        if (leftVisibleRank < 0) return 1;
        if (rightVisibleRank < 0) return -1;
        if (leftVisibleRank !== rightVisibleRank) return leftVisibleRank - rightVisibleRank;
      }
      var touchDelta = (scheduledRunSessionOwners[right].lastTouched || 0) -
        (scheduledRunSessionOwners[left].lastTouched || 0);
      return touchDelta || left.localeCompare(right);
    });
    for (var i = MAX_SCHEDULED_RUN_SESSION_OWNERS; i < ids.length; i++) {
      delete scheduledRunSessionOwners[ids[i]];
    }
  }
  function isScheduledRunTerminal(status) {
    var value = String(status || "").toLowerCase();
    return value === "completed" || value === "failed" || value === "canceled";
  }
  function rememberScheduledRunOwner(run) {
    if (!run) return;
    var id = typeof run.sessionId === "string" ? run.sessionId.trim() : "";
    if (!id) return;
    var status = String(run.status || "").toLowerCase();
    var phase = isScheduledRunTerminal(status)
      ? "terminal"
      : (status === "queued" || status === "running" ? "active" : null);
    registerScheduledRunOwner(id, phase);
  }
  function scheduledRunBuffer(id) {
    var buf = getBuffer(id);
    if (!buf) return null;
    registerScheduledRunOwner(id, null);
    return touchSessionBuffer(id, buf, true);
  }
  function markScheduledInitialTurnActive(id) {
    var buf = scheduledRunBuffer(id);
    var owner = registerScheduledRunOwner(id, "active");
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
  function markScheduledInitialTurnTerminal(id) {
    var buf = scheduledRunBuffer(id);
    registerScheduledRunOwner(id, "terminal");
    if (!buf || buf.scheduledInitialTurnPhase === "terminal") return buf;
    if (buf.scheduledInitialTurnPhase !== "active") {
      buf.scheduledInitialTurnPhase = "active";
    }
    buf.scheduledInitialTurnPhase = "terminal";
    return buf;
  }
  function beginScheduledOpenActivation(id) {
    var previous = sessionStates[id] || null;
    var snapshot = {
      id: id,
      existed: !!previous,
      previousPhase: previous && previous.scheduledInitialTurnPhase,
      previousBusy: previous ? !!previous.busy : false,
      previousStateBusy: state.activeSessionId === id ? !!state.busy : null,
    };
    var buf = markScheduledInitialTurnActive(id);
    snapshot.buffer = buf;
    snapshot.activationTouch = buf && buf.lastTouched;
    snapshot.changed = !!buf && (
      !snapshot.existed ||
      snapshot.previousPhase !== buf.scheduledInitialTurnPhase ||
      snapshot.previousBusy !== !!buf.busy
    );
    return snapshot;
  }
  function rollbackScheduledOpenActivation(snapshot) {
    if (!snapshot || !snapshot.changed) return;
    var current = sessionStates[snapshot.id];
    if (!current || current !== snapshot.buffer) return;
    if (current.scheduledInitialTurnPhase === "terminal") return;
    if (current.lastTouched !== snapshot.activationTouch) return;
    if (!snapshot.existed) {
      delete sessionStates[snapshot.id];
    } else {
      current.scheduledInitialTurnPhase = snapshot.previousPhase;
      current.busy = snapshot.previousBusy;
    }
    if (state.activeSessionId === snapshot.id && snapshot.previousStateBusy !== null) {
      state.busy = snapshot.previousStateBusy;
    }
  }
  function saveWorkingSetTo(buf) {
    if (!buf) return;
    buf.messages = state.messages; buf.chatItems = state.chatItems; buf.artifacts = state.artifacts;
    buf.turnTimeline = state.turnTimeline;
    buf.activeTurnTimelineId = state.activeTurnTimelineId;
    buf.personaEvents = state.personaEvents;
    buf.pinvouReviews = state.pinvouReviews;
    buf.busy = buf.scheduledInitialTurnPhase === "active" ? true : state.busy;
    buf.planSnapshot = state.planSnapshot; buf.modeState = state.modeState;
    buf.thinking = state.thinking; buf.tokens = state.tokens; buf.queued = state.queued;
    buf.activePersona = state.activePersona;
    buf.mountedCollection = state.mountedCollection;
    buf.scheduledTaskDraft = state.scheduledTaskDraft;
    buf.stream = {
      currentStreamText: context.currentStreamText, currentStreamId: context.currentStreamId,
      pendingAssistantText: context.pendingAssistantText, pendingAssistantBlocks: context.pendingAssistantBlocks,
      itemIdSeq: context.itemIdSeq, toolMeta: context.toolMeta,
    };
  }
  function loadWorkingSetFrom(buf) {
    if (!buf) return;
    state.messages = buf.messages; state.chatItems = buf.chatItems; state.artifacts = buf.artifacts;
    state.turnTimeline = buf.turnTimeline || [];
    state.activeTurnTimelineId = buf.activeTurnTimelineId || null;
    state.personaEvents = buf.personaEvents || [];
    state.pinvouReviews = buf.pinvouReviews || [];
    state.pinvouModal = null; // 切 session 关掉检阅弹窗
    state.turnDirtyArtifacts = []; // turn 临时态,切 session 清空,别串到新 session
    state.turnPresentedArtifacts = [];
    state.busy = buf.scheduledInitialTurnPhase === "active" ? true : buf.busy;
    state.planSnapshot = buf.planSnapshot; state.modeState = buf.modeState;
    state.thinking = buf.thinking; state.tokens = buf.tokens; state.queued = buf.queued || [];
    state.activePersona = buf.activePersona || null;
    state.mountedCollection = buf.mountedCollection || null;
    state.scheduledTaskDraft = buf.scheduledTaskDraft || null;
    var s = buf.stream || {};
    context.currentStreamText = s.currentStreamText || ""; context.currentStreamId = s.currentStreamId || 0;
    context.pendingAssistantText = s.pendingAssistantText || ""; context.pendingAssistantBlocks = s.pendingAssistantBlocks || [];
    context.itemIdSeq = s.itemIdSeq || 0; context.toolMeta = s.toolMeta || {};
  }
  function hydrateWorkingSetFromSaved(buf, saved) {
    if (!buf || !saved) return;
    var completedRemoteTurn = !!buf.remoteTerminalSeen || (!!buf.remoteTurnActive && !buf.busy);
    buf.messages = Array.isArray(saved.messages) ? saved.messages : [];
    buf.sessionRevision = String(saved.transcript_revision || saved.transcriptRevision || "");
    buf.chatItems = [];
    buf.turnTimeline = [];
    buf.activeTurnTimelineId = null;
    buf.artifacts = Array.isArray(saved.artifacts) ? saved.artifacts.map(function (a) {
      var p = typeof a === "string" ? a : (a.storage_path || a.path || "");
      return { path: p, basename: basename(p) };
    }) : [];
    buf.artifacts = filterSessionArtifacts(buf.artifacts, saved.metadata && saved.metadata.id);
    buf.personaEvents = [];
    buf.pinvouReviews = [];
    if (completedRemoteTurn) {
      buf.remoteTurnActive = false;
      buf.remoteTerminalSeen = false;
      buf.remoteBaselineMessageCount = null;
      buf.remoteBaselineTrusted = false;
      buf.remoteExpectedAssistantKey = "";
      buf.deferredRemoteUserEvent = null;
    }
    buf.stream = {
      currentStreamText: "", currentStreamId: 0, pendingAssistantText: "",
      pendingAssistantBlocks: [], itemIdSeq: 0, toolMeta: {},
    };
  }
  async function ensureSessionBufferLoaded(sid) {
    if (!sid) return;
    if (sid === state.activeSessionId) return;
    var buf = getBuffer(sid);
    var meta = state.sessions.find(function (s) { return s.id === sid; }) || {};
    var knownCount = Number(meta.message_count || 0);
    if (buf.busy) return;
    if (buf.loadedFromDisk && (!knownCount || buf.messages.length >= knownCount)) return;
    if (!buf.loadedFromDisk && buf.messages.length && (!knownCount || buf.messages.length >= knownCount)) return;
    var saved = await invoke("load_session", { id: sid, setActive: false });
    var savedMessages = saved && Array.isArray(saved.messages) ? saved.messages : [];
    var savedMetadataCount = saved && saved.metadata ? Number(saved.metadata.message_count || 0) : 0;
    var savedCount = Math.max(Number.isFinite(savedMetadataCount) ? savedMetadataCount : 0, savedMessages.length);
    // Shell 轮询等后台展示项会先写入 chatItems，但不代表会话正文已经加载。
    // 只有内存里确有 transcript messages 且不短于磁盘版本时，才能跳过 hydration。
    if (buf.messages.length && savedCount <= buf.messages.length) {
      buf.loadedFromDisk = true;
      return;
    }
    hydrateWorkingSetFromSaved(buf, saved);
    try { buf.personaEvents = await invoke("get_session_persona_events", { sessionId: sid }) || []; } catch (e) { buf.personaEvents = []; }
    try { buf.pinvouReviews = await invoke("get_session_pinvou_reviews", { sessionId: sid }) || []; } catch (e) { buf.pinvouReviews = []; }
    try { buf.turnTimeline = await invoke("get_session_timeline", { sessionId: sid }) || []; } catch (e) { buf.turnTimeline = []; }
    // 手机可能在桌面仍停留草稿页/其他 session 时先唤醒这个后台 session。
    // 仅 hydrate messages 而把 chatItems 留空，会让后续 switchToSession 命中缓存快路径，
    // 不再 rerenderFromMessages，桌面便只看得到手机唤醒后的新内容，历史像是“丢了”。
    // 在首次磁盘 hydration 后先完整重建展示层，再由 mobile_user_message 追加当前轮；
    // buf.busy 时上方已提前返回，不会覆盖正在流式生成的实时 chatItems。
    runSyncOnSession(sid, function () {
      resetPendingAssistant();
      rerenderFromMessages();
    });
    buf.loadedFromDisk = true;
  }
  // 把 active 工作集存好后切到 id 的 buffer(opts.fresh=新建空 buffer)。
  function switchActiveTo(id, opts) {
    if (state.activeSessionId) saveWorkingSetTo(getBuffer(state.activeSessionId));
    state.activeSessionId = id;
    var buf = sessionStates[id];
    if (!buf || (opts && opts.fresh)) buf = sessionStates[id] = freshBuffer();
    touchSessionBuffer(id, buf, id.indexOf("sched-") === 0);
    loadWorkingSetFrom(buf);
    state.artifacts = filterSessionArtifacts(state.artifacts, id);
    scheduleShellPoll(id, true);
  }
  // 在指定 session 的工作集上跑一段【同步】逻辑。sid 是 active → 直接跑(零行为变化);
  // 否则临时切到该 buffer 跑完再切回(期间不 notify)。
  async function refreshHistoryList() {
    try {
      state.sessions = await invoke("list_sessions");
    } catch (e) {
      console.warn("list_sessions failed", e);
      state.sessions = [];
    }
    try {
      state.archivedSessions = await invoke("list_archived_sessions");
    } catch (e) {
      state.archivedSessions = state.archivedSessions || [];
    }
    notify();
  }

  // 进入草稿态:不创建 session,只清空工作集 + activeSessionId=null,落在「你好」欢迎页。
  // session 在首次有实质内容(发消息 / 加卡牌,见 ensureSession)时才物化——这样会话列表里
  // 永远不会堆积没用过的空「新对话」(ChatGPT/Claude 式 lazy session)。
  function enterDraft() {
    sessionSwitchRequestToken += 1; // 新建/返回草稿会话使任何仍在等待的 load_session 结果失效
    state.scheduledRunContext = null;
    state.draftEpoch++; // 每次点击都自增——含下面提前返回的「已在草稿态」分支,让前端能重置 welcomeToolId
    state.scheduledTaskPendingGuide = null; // 换了对话,未发送的定时任务引导词作废

    // 已在干净草稿态 → 只 notify(epoch 已自增)。注意要连 chatItems 一起判空:messages 与 chatItems
    // 会背离(persona 气泡 / ensureSession 失败的 system 报错卡只进 chatItems),否则残留卡顶掉「你好」。
    if (!state.activeSessionId && state.messages.length === 0 && state.chatItems.length === 0) { notify(); return; }
    if (state.activeSessionId) saveWorkingSetTo(getBuffer(state.activeSessionId));
    state.activeSessionId = null;
    loadWorkingSetFrom(freshBuffer());
    notify();
  }
  // 公开「新建对话」入口(侧边栏按钮)= 进草稿态。名字保留以兼容前端调用。
  async function createNewSession() { enterDraft(); }

  // 草稿态首次有实质内容时真正向后端创建 session 并切为 active;已有 active 直接返回。
  // 返回新 session id,创建失败返回 null。调用方:sendMessage(首条消息) / equipPersona(加卡)。
  async function ensureSession() {
    if (state.activeSessionId) return state.activeSessionId;
    // 多 session 并发:不预热 engine。新建空 session 的 buffer 由 switchActiveTo({fresh}) 起。
    try {
      var meta = await invoke("create_session");
      switchActiveTo(meta.id, { fresh: true });
      await refreshHistoryList();
      await syncModeState();
      await syncActivePersona();
      await syncMountedCollection();
      notify();
      return state.activeSessionId;
    } catch (e) {
      addSystemItem(bt("newChatFailed") + e);
      return null;
    }
  }

  function reportSessionSwitchFailure(error, errorScope) {
    if (errorScope === "scheduled") {
      setScheduledTaskError(error, "navigation");
      notify();
      return;
    }
    addSystemItem(bt("loadChatFailed") + error);
  }

  function hydratedMessageKey(message, hideInternalEnvelope) {
    var blocks = message && Array.isArray(message.content) ? message.content : [];
    if (message && message.role === "user") {
      var resultIds = blocks.filter(function (block) {
        return block && block.type === "tool_result" && block.tool_use_id;
      }).map(function (block) { return block.tool_use_id; }).sort();
      if (resultIds.length) return "user:tool_results:" + resultIds.join("|");
      return "user:text:" + userMessageDisplayText(blocks, hideInternalEnvelope);
    }
    if (message && message.role === "assistant") {
      var toolIds = blocks.filter(function (block) {
        return block && block.type === "tool_use" && block.id;
      }).map(function (block) { return block.id; }).sort();
      if (toolIds.length) return "assistant:tool_uses:" + toolIds.join("|");
      blocks = blocks.filter(function (block) { return !block || block.type !== "thinking"; });
      try { return "assistant:" + JSON.stringify(blocks); } catch (_) {}
    }
    try { return JSON.stringify(message); } catch (_) { return String(message); }
  }

  function mergeHydratedMessages(durableMessages, liveMessages, hideInternalEnvelope) {
    var durable = Array.isArray(durableMessages) ? durableMessages.slice() : [];
    var counts = Object.create(null);
    durable.forEach(function (message) {
      var key = hydratedMessageKey(message, hideInternalEnvelope);
      counts[key] = (counts[key] || 0) + 1;
    });
    (Array.isArray(liveMessages) ? liveMessages : []).forEach(function (message) {
      var key = hydratedMessageKey(message, hideInternalEnvelope);
      if (counts[key]) {
        counts[key] -= 1;
      } else {
        durable.push(message);
      }
    });
    return durable;
  }

  function mergeHydratedArtifacts(durableArtifacts, liveArtifacts) {
    var merged = [];
    var seen = Object.create(null);
    (durableArtifacts || []).concat(liveArtifacts || []).forEach(function (artifact) {
      var path = typeof artifact === "string" ? artifact : (artifact && (artifact.path || artifact.storage_path)) || "";
      var identity = basename(path);
      if (!path || !identity) return;
      if (seen[identity] !== undefined) {
        var existingIndex = seen[identity];
        if (isAbsPath(path) && !isAbsPath(merged[existingIndex].path)) {
          merged[existingIndex] = { path: path, basename: identity };
        }
        return;
      }
      seen[identity] = merged.length;
      merged.push({ path: path, basename: identity });
    });
    return merged;
  }

  function hydratedChatItemKey(item) {
    if (!item || !item.type) return "";
    if (item.type === "assistant") return "assistant:" + String(item.html || item.text || "");
    if (item.type === "tool" && item.toolId) return "tool:" + item.toolId;
    if (item.type === "artifact_card") return "artifact:" + basename(item.path);
    if (item.type === "user_input" && item.toolCallId) return "user_input:" + item.toolCallId;
    if (item.type === "careful_blocked" && item.toolCallId) return "careful_blocked:" + item.toolCallId;
    if (item.type === "plan_card" && item.planId) return "plan:" + item.planId;
    if (item.type === "user") return "user:" + String(item.text || item.html || "");
    if (item.type === "system") return "system:" + String(item.text || "");
    var stable = Object.assign({}, item);
    delete stable.id;
    delete stable.time;
    delete stable.streaming;
    try { return item.type + ":" + JSON.stringify(stable); } catch (_) { return item.type + ":" + String(stable); }
  }

  function mergeHydratedChatItems(liveChatItems, liveCurrentStreamId) {
    var remappedCurrentStreamId = 0;
    var availableByKey = Object.create(null);
    state.chatItems.forEach(function (item, index) {
      var key = hydratedChatItemKey(item);
      if (!key) return;
      if (!availableByKey[key]) availableByKey[key] = [];
      availableByKey[key].push(index);
    });
    (liveChatItems || []).forEach(function (item) {
      var key = hydratedChatItemKey(item);
      var matches = key && availableByKey[key];
      var existingIndex = matches && matches.length ? matches.shift() : -1;
      if (existingIndex >= 0) {
        var existingId = state.chatItems[existingIndex].id;
        state.chatItems[existingIndex] = Object.assign({}, state.chatItems[existingIndex], item, {
          id: existingId,
        });
        if (item && item.id === liveCurrentStreamId) remappedCurrentStreamId = existingId;
        return;
      }
      var clone = Object.assign({}, item, { id: ++context.itemIdSeq });
      if (item && item.id === liveCurrentStreamId) remappedCurrentStreamId = clone.id;
      state.chatItems.push(clone);
    });
    return remappedCurrentStreamId;
  }

  async function switchToSessionInternal(id, preserveScheduledRunContext, errorScope, options) {
    var requestToken = ++sessionSwitchRequestToken;
    var forceDurableLoad = !!(options && options.forceDurableLoad);
    var hydrateLiveSession = !!(options && options.hydrateLiveSession);
    if (!id) {
      reportSessionSwitchFailure(new Error("该运行记录没有可打开的会话"), errorScope);
      return false;
    }
    if (hydrateLiveSession && !sessionStates[id]) sessionStates[id] = freshBuffer();
    if (id === state.activeSessionId && !forceDurableLoad && !hydrateLiveSession) {
      if (!preserveScheduledRunContext) state.scheduledRunContext = null;
      state.scheduledTaskPendingGuide = null;
      notify();
      return true;
    }
    // 多 session 并发:切换【不再 cancel】旧 session —— 它在自己的 engine 上继续跑,
    // 工作集存进 sessionStates 后台累积。切回来能看到完整(含切走期间产生的)内容。
    // 已有 buffer(切过/在跑)→ 直接换工作集;没有 → load_session 建 buffer + 重渲染。
    if (sessionStates[id] && !forceDurableLoad && !hydrateLiveSession) {
      if (!preserveScheduledRunContext) state.scheduledRunContext = null;
      state.scheduledTaskPendingGuide = null; // 仅在目标会话已确认可用后提交导航状态
      switchActiveTo(id, null);
      await syncModeState();
      await syncActivePersona();
      await syncMountedCollection();
      await loadMemoryOverview({ rehydratePending: true });
      if (requestToken !== sessionSwitchRequestToken || state.activeSessionId !== id) return false;
      notify();
      reconcileArtifacts(id); // 对账磁盘产物(fire-and-forget)
      return true;
    }
    var saved;
    try {
      saved = await invoke("load_session", { id: id });
    } catch (e) {
      if (requestToken === sessionSwitchRequestToken) reportSessionSwitchFailure(e, errorScope);
      return false;
    }
    if (requestToken !== sessionSwitchRequestToken) return false;
    if (!saved || !saved.metadata || !saved.metadata.id) {
      reportSessionSwitchFailure(new Error("会话数据无效"), errorScope);
      return false;
    }

    var personaEvents = [];
    var pinvouReviews = [];
    var turnTimeline = [];
    try { personaEvents = await invoke("get_session_persona_events", { sessionId: id }) || []; } catch (_) {}
    try { pinvouReviews = await invoke("get_session_pinvou_reviews", { sessionId: id }) || []; } catch (_) {}
    try { turnTimeline = await invoke("get_session_timeline", { sessionId: id }) || []; } catch (_) {}
    if (requestToken !== sessionSwitchRequestToken) return false;

    // load_session 与必要的直接会话数据均成功后，才一次性提交 active/context。
    if (state.activeSessionId) saveWorkingSetTo(getBuffer(state.activeSessionId));
    if (!preserveScheduledRunContext) state.scheduledRunContext = null;
    state.scheduledTaskPendingGuide = null;
    state.activeSessionId = saved.metadata.id;
    if (hydrateLiveSession) {
      var liveBuffer = sessionStates[id] || freshBuffer();
      loadWorkingSetFrom(liveBuffer);
      var liveMessages = Array.isArray(state.messages) ? state.messages.slice() : [];
      var liveChatItems = Array.isArray(state.chatItems) ? state.chatItems.slice() : [];
      var liveArtifacts = Array.isArray(state.artifacts) ? state.artifacts.slice() : [];
      var liveCurrentStreamId = context.currentStreamId;
      var hasLivePresentation = !!state.busy || !!context.currentStreamText || !!context.pendingAssistantText ||
        (Array.isArray(context.pendingAssistantBlocks) && context.pendingAssistantBlocks.length > 0);
      state.messages = mergeHydratedMessages(
        saved.messages,
        liveMessages,
        isScheduledRunSession(id)
      );
      state.personaEvents = personaEvents.length ? personaEvents : (liveBuffer.personaEvents || []);
      state.pinvouReviews = pinvouReviews.length ? pinvouReviews : (liveBuffer.pinvouReviews || []);
      state.turnTimeline = turnTimeline.length ? turnTimeline : (liveBuffer.turnTimeline || []);
      state.artifacts = filterSessionArtifacts(
        mergeHydratedArtifacts(saved.artifacts, liveArtifacts),
        state.activeSessionId
      );
      rerenderFromMessages();
      if (hasLivePresentation) {
        context.currentStreamId = mergeHydratedChatItems(liveChatItems, liveCurrentStreamId);
      } else {
        resetPendingAssistant();
      }
      saveWorkingSetTo(liveBuffer);
    } else {
      loadWorkingSetFrom(sessionStates[id] = freshBuffer());
      state.messages = Array.isArray(saved.messages) ? saved.messages : [];
      sessionStates[id].loadedFromDisk = true;
      state.personaEvents = personaEvents;
      state.pinvouReviews = pinvouReviews;
      state.turnTimeline = turnTimeline;
      resetPendingAssistant();
      state.chatItems = [];
      state.artifacts = mergeHydratedArtifacts(saved.artifacts, []);
      state.artifacts = filterSessionArtifacts(state.artifacts, state.activeSessionId);
      rerenderFromMessages();
    }
    await syncModeState();
    await syncActivePersona();
    await syncMountedCollection();
    await loadMemoryOverview({ rehydratePending: true });
    if (requestToken !== sessionSwitchRequestToken || state.activeSessionId !== saved.metadata.id) return false;
    notify();
    reconcileArtifacts(id); // 对账磁盘产物(修重启/跟踪遗漏导致的面板缺文件)
    return true;
  }

  async function switchToSession(id) {
    return switchToSessionInternal(id, false, "chat");
  }

  async function openScheduledRunChatOnce(run, task) {
    var sessionId = run && typeof run.sessionId === "string" ? run.sessionId.trim() : "";
    if (!sessionId) {
      reportSessionSwitchFailure(new Error("该运行记录没有可打开的会话"), "scheduled");
      return false;
    }
    rememberScheduledRunOwner(run);
    var runStatus = String(run && run.status || "").toLowerCase();
    var openActivation = null;
    if (runStatus === "queued" || runStatus === "running") {
      openActivation = beginScheduledOpenActivation(sessionId);
    } else {
      scheduledRunBuffer(sessionId);
    }
    setScheduledTaskError(null);
    notify();
    var returnSessionId = state.scheduledRunContext
      ? state.scheduledRunContext.returnSessionId
      : state.activeSessionId;
    var liveBuffer = sessionStates[sessionId];
    var hasLiveTurn = !!(liveBuffer && (
      liveBuffer.busy ||
      liveBuffer.scheduledInitialTurnPhase === "active" ||
      (liveBuffer.queued && liveBuffer.queued.length) ||
      (liveBuffer.thinking && liveBuffer.thinking.active)
    ));
    var isTerminalRun = runStatus === "completed" || runStatus === "failed" || runStatus === "canceled";
    var forceDurableLoad = isTerminalRun && !hasLiveTurn;
    var switched = await switchToSessionInternal(sessionId, true, "scheduled", {
      forceDurableLoad: forceDurableLoad,
      hydrateLiveSession: !isTerminalRun,
    });
    if (!switched) {
      rollbackScheduledOpenActivation(openActivation);
      notify();
      return false;
    }
    if (forceDurableLoad) markScheduledInitialTurnTerminal(sessionId);
    else scheduledRunBuffer(sessionId);
    var automationId = (run && run.automationId) || (task && task.id) || null;
    var runId = (run && (run.runId || run.id)) || null;
    state.scheduledRunContext = {
      sessionId: sessionId,
      returnSessionId: returnSessionId,
      automationId: automationId,
      runId: runId,
      taskName: (task && task.name) || (run && (run.taskName || run.name)) || "",
      model: (task && task.model) || null,
      mode: "yolo",
    };
    // 先发布完整会话视图；只有已完成的运行才持久化为已查看。
    notify();
    if (automationId && runId && runStatus === "completed") {
      try {
        var receipt = await invoke("mark_scheduled_run_viewed", {
          automationId: automationId,
          runId: runId,
        });
        invalidateScheduledTaskReads(automationId);
        applyScheduledRunViewed(automationId, runId, receipt);
      } catch (e) {
        setScheduledTaskError(e, "action");
      }
    }
    notify();
    return true;
  }

  function openScheduledRunChat(run, task) {
    var sessionId = run && typeof run.sessionId === "string" ? run.sessionId.trim() : "";
    if (!sessionId) return openScheduledRunChatOnce(run, task);
    if (scheduledRunOpenInFlight[sessionId]) return scheduledRunOpenInFlight[sessionId];
    var opening = openScheduledRunChatOnce(run, task);
    scheduledRunOpenInFlight[sessionId] = opening;
    function clearOpening() {
      if (scheduledRunOpenInFlight[sessionId] === opening) {
        delete scheduledRunOpenInFlight[sessionId];
      }
    }
    opening.then(clearOpening, clearOpening);
    return opening;
  }

  async function exitScheduledRunChat() {
    var context = state.scheduledRunContext;
    if (!context) return false;
    if (context.returnSessionId && context.returnSessionId !== context.sessionId) {
      var restored = await switchToSessionInternal(context.returnSessionId, true, "scheduled");
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

  function recentScheduledRunForSession(id) {
    return (state.scheduledTaskRecentRuns || []).find(function (run) {
      return run && run.sessionId === id;
    }) || null;
  }

  // 离开正在查看的会话:清 active + 换空工作集,并清掉指向它的定时运行上下文。
  // 必须连 scheduledRunContext 一起清 —— main.jsx 只按该字段真值决定渲染
  // ChatView 还是 ScheduledTasksView,而 ChatView 内部还要求 sessionId===activeSessionId
  // 才渲染返回按钮;只清 active 会卡在「定时路由下的空白页且没有返回按钮」。
  // 清掉之后 currentView 仍是 'scheduled',界面自然落回定时任务列表。
  // 不负责 buffer:删除要丢弃 buffer,收纳要保留 buffer,由调用方各自处理。
  function leaveSessionView(id) {
    if (state.scheduledRunContext && state.scheduledRunContext.sessionId === id) {
      state.scheduledRunContext = null;
    }
    if (state.activeSessionId !== id) return;
    state.activeSessionId = null;
    loadWorkingSetFrom(freshBuffer());
  }

  async function deleteSession(id) {
    invalidateScheduledRecentRunsForSession(id);
    try {
      // 后端按 SessionKind 分发:定时运行会话在 delete_session 里联动删除
      // 该次 Session、Run 与底座 Task,任务定义与共享工作间保留。
      await invoke("delete_session", { id: id });
      // 统一清理工作集、实时状态、定时创建上下文与当前视图，避免手写字段漂移。
      purgeSessionBuffer(id);
      state.sessions = state.sessions.filter(function (s) { return s.id !== id; });
      state.archivedSessions = (state.archivedSessions || []).filter(function (s) { return s.id !== id; });
      state.scheduledTaskRecentRuns = (state.scheduledTaskRecentRuns || []).filter(function (run) {
        return !run || run.sessionId !== id;
      });
      state.scheduledTaskRuns = (state.scheduledTaskRuns || []).filter(function (run) {
        return !run || run.sessionId !== id;
      });
      notify();
      return true;
    } catch (e) {
      addSystemItem(bt("deleteFailed") + e);
      return false;
    }
  }

  async function renameSession(id, title) {
    invalidateScheduledRecentRunsForSession(id);
    try {
      await invoke("rename_session", { id: id, title: title });
      var s = state.sessions.find(function (s) { return s.id === id; });
      if (s) s.title = title;
      state.scheduledTaskRecentRuns = (state.scheduledTaskRecentRuns || []).map(function (run) {
        return run && run.sessionId === id ? Object.assign({}, run, { sessionTitle: title }) : run;
      });
      delete personaPlaceholderTitles[id]; // 用户主动命名后不再算卡牌占位,不被对话覆盖
      notify();
    } catch (e) {
      console.warn("rename failed", e);
    }
  }

  async function toggleSessionPinned(id, pinned) {
    invalidateScheduledRecentRunsForSession(id);
    var s = state.sessions.find(function (s) { return s.id === id; });
    var scheduledRun = recentScheduledRunForSession(id);
    var prev = s ? !!s.pinned : false;
    var prevPinnedAt = s ? s.pinned_at : null;
    var previousRunPinned = scheduledRun ? !!scheduledRun.pinned : false;
    var previousRunPinnedAt = scheduledRun ? scheduledRun.pinnedAt : null;
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
      await invoke("set_session_pinned", { id: id, pinned: !!pinned });
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

  async function archiveSession(id) {
    invalidateScheduledRecentRunsForSession(id);
    var idx = state.sessions.findIndex(function (s) { return s.id === id; });
    if (idx < 0) {
      // 定时运行会话不在 state.sessions;收起 = 从侧边栏记录移除,进设置页归档列表。
      var scheduledRun = recentScheduledRunForSession(id);
      // Codex 等独立会话也不在 state.sessions；交给后端判定并刷新统一历史列表。
      if (!scheduledRun) {
        try {
          await invoke("set_session_archived", { id: id, archived: true });
          await refreshHistoryList();
          return true;
        } catch (e) {
          console.warn("set_session_archived failed", e);
          return false;
        }
      }
      var previousRuns = state.scheduledTaskRecentRuns || [];
      var wasViewingRun = state.activeSessionId === id;
      var previousContext = state.scheduledRunContext;
      // 与普通会话收纳同语义:保留 buffer(还能从设置页还原后重开),但要离开当前视图。
      if (wasViewingRun) saveWorkingSetTo(getBuffer(id));
      state.scheduledTaskRecentRuns = previousRuns.filter(function (run) {
        return !run || run.sessionId !== id;
      });
      leaveSessionView(id);
      notify();
      try {
        await invoke("set_session_archived", { id: id, archived: true });
        await refreshHistoryList();
        return true;
      } catch (e) {
        state.scheduledTaskRecentRuns = previousRuns;
        if (wasViewingRun) {
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
    var s = state.sessions[idx];
    var archived = Object.assign({}, s, { archived: true, archived_at: new Date().toISOString(), pinned: false, pinned_at: null });
    var wasActive = state.activeSessionId === id;
    if (wasActive) saveWorkingSetTo(getBuffer(id));
    state.sessions.splice(idx, 1);
    state.archivedSessions = [archived].concat((state.archivedSessions || []).filter(function (x) { return x.id !== id; }));
    leaveSessionView(id);
    notify();
    try {
      await invoke("set_session_archived", { id: id, archived: true });
      await refreshHistoryList();
      return true;
    } catch (e) {
      state.sessions.splice(idx, 0, s);
      state.archivedSessions = (state.archivedSessions || []).filter(function (x) { return x.id !== id; });
      if (wasActive) {
        state.activeSessionId = id;
        loadWorkingSetFrom(getBuffer(id));
      }
      console.warn("set_session_archived failed", e);
      notify();
      return false;
    }
  }

  async function restoreArchivedSession(id) {
    var idx = (state.archivedSessions || []).findIndex(function (s) { return s.id === id; });
    if (idx < 0) return false;
    var s = state.archivedSessions[idx];
    invalidateScheduledRecentRunsForSession(id);
    var restored = Object.assign({}, s, { archived: false, archived_at: null });
    state.archivedSessions.splice(idx, 1);
    state.sessions = [restored].concat(state.sessions || []);
    notify();
    try {
      await invoke("set_session_archived", { id: id, archived: false });
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

  // 实时态有专属气泡的工具（方案卡），重建时要还原成原卡而非普通工具卡。

    return {
      freshBuffer: freshBuffer,
      getBuffer: getBuffer,
      isProtectedScheduledBuffer: isProtectedScheduledBuffer,
      pruneScheduledSessionBuffers: pruneScheduledSessionBuffers,
      touchSessionBuffer: touchSessionBuffer,
      purgeSessionBuffer: purgeSessionBuffer,
      registerScheduledRunOwner: registerScheduledRunOwner,
      scheduledRunOwnerVisibleRank: scheduledRunOwnerVisibleRank,
      scheduledRunOwnerPriority: scheduledRunOwnerPriority,
      isProtectedScheduledRunOwner: isProtectedScheduledRunOwner,
      pruneScheduledRunSessionOwner: pruneScheduledRunSessionOwner,
      pruneScheduledRunSessionOwners: pruneScheduledRunSessionOwners,
      isScheduledRunTerminal: isScheduledRunTerminal,
      rememberScheduledRunOwner: rememberScheduledRunOwner,
      scheduledRunBuffer: scheduledRunBuffer,
      markScheduledInitialTurnActive: markScheduledInitialTurnActive,
      markScheduledInitialTurnTerminal: markScheduledInitialTurnTerminal,
      beginScheduledOpenActivation: beginScheduledOpenActivation,
      rollbackScheduledOpenActivation: rollbackScheduledOpenActivation,
      saveWorkingSetTo: saveWorkingSetTo,
      loadWorkingSetFrom: loadWorkingSetFrom,
      hydrateWorkingSetFromSaved: hydrateWorkingSetFromSaved,
      ensureSessionBufferLoaded: ensureSessionBufferLoaded,
      switchActiveTo: switchActiveTo,
      refreshHistoryList: refreshHistoryList,
      enterDraft: enterDraft,
      createNewSession: createNewSession,
      ensureSession: ensureSession,
      reportSessionSwitchFailure: reportSessionSwitchFailure,
      hydratedMessageKey: hydratedMessageKey,
      mergeHydratedMessages: mergeHydratedMessages,
      mergeHydratedArtifacts: mergeHydratedArtifacts,
      hydratedChatItemKey: hydratedChatItemKey,
      mergeHydratedChatItems: mergeHydratedChatItems,
      switchToSessionInternal: switchToSessionInternal,
      switchToSession: switchToSession,
      openScheduledRunChatOnce: openScheduledRunChatOnce,
      openScheduledRunChat: openScheduledRunChat,
      exitScheduledRunChat: exitScheduledRunChat,
      recentScheduledRunForSession: recentScheduledRunForSession,
      leaveSessionView: leaveSessionView,
      deleteSession: deleteSession,
      renameSession: renameSession,
      toggleSessionPinned: toggleSessionPinned,
      archiveSession: archiveSession,
      restoreArchivedSession: restoreArchivedSession
    };
  };
})(window);
