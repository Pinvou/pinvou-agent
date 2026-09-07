/** Session working sets, switching, hydration, and lifecycle operations. */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting statements would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry.sessions = function (context) {
    const state = context.state;
    // Optional hook: clean host-side per-session side tables when a
    // session buffer is reclaimed/deleted (bridge.js's modeStateEpochs,
    // the scene-events localStorage key).
    // reason === "evict" is an LRU capacity eviction, "delete" a real
    // session deletion; the host distinguishes them to keep recoverable
    // data (e.g. the scene cache key) on eviction. No return value.
    const onSessionBufferPurged = context.onSessionBufferPurged || null;
    const invoke = context.invoke;
    const listen = context.listen;
    const notify = context.notify;
    // 系统目录选择对话框（bridge.js 注入 TAURI.dialog.open；React 不得直接
    // 触碰 Tauri 全局，草稿工作区选择由此封装）。不可用时为 undefined，
    // pickDraftWorkspace 以 null 早退。
    const dialogOpen = context.dialogOpen || null;
    const sessionStates = context.sessionStates;
    const scheduledRunSessionOwners = context.scheduledRunSessionOwners;
    const personaPlaceholderTitles = context.personaPlaceholderTitles;
    const runSyncOnSession = context.runSyncOnSession;
    const resetPendingAssistant = context.resetPendingAssistant;
    const rerenderFromMessages = context.rerenderFromMessages;
    const syncModeState = context.syncModeState;
    const applyAuthoritativeModeState = context.applyAuthoritativeModeState;
    const currentDraftModeState = context.currentDraftModeState;
    const syncActivePersona = context.syncActivePersona;
    const syncMountedCollection = context.syncMountedCollection;
    const reconcileArtifacts = context.reconcileArtifacts;
    const loadSessionModel = context.loadSessionModel;
    const invalidateScheduledRecentRunsForSession = context.invalidateScheduledRecentRunsForSession;
    const addSystemItem = context.addSystemItem;
    const turnUsageDirty = context.turnUsageDirty;
    const basename = context.basename;
    const isAbsPath = context.isAbsPath;
    const filterSessionArtifacts = context.filterSessionArtifacts;
    const scheduleShellPoll = context.scheduleShellPoll;
    const bt = context.bt;
    const setScheduledTaskError = context.setScheduledTaskError;
    const userMessageDisplayText = context.userMessageDisplayText;
    const loadMemoryOverview = context.loadMemoryOverview;
    const isScheduledRunSession = context.isScheduledRunSession;
    const invalidateScheduledTaskReads = context.invalidateScheduledTaskReads;
    const applyScheduledRunViewed = context.applyScheduledRunViewed;
    const loadScheduledTaskRecentRuns = context.loadScheduledTaskRecentRuns;
    const loadPinvouSceneEventsForSession = context.loadPinvouSceneEventsForSession || function () { return []; };
    const syncPinvouSceneEventsForSession = context.syncPinvouSceneEventsForSession ||
      function (sid) { return Promise.resolve(loadPinvouSceneEventsForSession(sid)); };
    const loadSteeredMessagesForSession = context.loadSteeredMessagesForSession || function () { return []; };
    const syncSteeredMessagesForSession = context.syncSteeredMessagesForSession ||
      function (sid) { return Promise.resolve(loadSteeredMessagesForSession(sid)); };
    const MAX_SCHEDULED_SESSION_BUFFERS = 64;
    const MAX_SCHEDULED_RUN_SESSION_OWNERS = 64;
    // All-session buffer cap: each sessionStates entry holds the full
    // messages+chatItems (with rendered html)+artifacts (heavy sessions
    // run 1-4MB each); previously only scheduled sessions had a 64-entry
    // LRU — normal sessions stayed resident forever once visited. Cap at
    // 32: typical users actively switch among single-digit session
    // counts, 32 × 1-4MB worst case is ~32-128MB, and cold sessions
    // beyond 32 have near-zero hit rate; revisiting an evicted session
    // goes through load_session disk rehydration (already supported by
    // ensureSessionBufferLoaded/switchTo), costing one reload.
    const MAX_SESSION_BUFFERS = 32;
    // Unsent composer drafts are the one piece of a working set that cannot be
    // rebuilt from disk (transcripts hold committed content only), so eviction
    // must never drop them: before a buffer is dropped, its draft moves to this
    // side table and every rebuild path restores it. The table is bounded
    // (256 entries, 1M chars per draft — 10x the composer input cap), and when
    // a bound would be exceeded the eviction is refused instead: the buffer
    // stays resident rather than silently losing user input. Real session
    // deletion (purgeSessionBuffer) invalidates stashed drafts so they never
    // flow back into a recycled session id.
    const MAX_EVICTED_SESSION_DRAFTS = 256;
    const MAX_EVICTED_SESSION_DRAFT_CHARS = 1000000;
    let sessionBufferTouchClock = 0;
    let scheduledRunOwnerTouchClock = 0;
    const scheduledRunOpenInFlight = Object.create(null);
    let sessionSwitchRequestToken = 0;
    const evictedSessionDrafts = Object.create(null);
    // Returns true when the buffer's non-rehydratable state is safely retained
    // (or empty) and the caller may drop the buffer; false means eviction must
    // be skipped so the draft stays in the live buffer.
    function stashEvictedSessionDraft(id, buf) {
      if (!id || !buf) return true;
      const draft = String(buf.composerDraft || "");
      if (!draft) {
        delete evictedSessionDrafts[id]; // input was cleared; a stale stash must not resurrect it
        return true;
      }
      if (draft.length > MAX_EVICTED_SESSION_DRAFT_CHARS) return false;
      if (!evictedSessionDrafts[id]
          && Object.keys(evictedSessionDrafts).length >= MAX_EVICTED_SESSION_DRAFTS) return false;
      delete evictedSessionDrafts[id]; // re-stashing moves the entry to the table tail
      evictedSessionDrafts[id] = draft;
      return true;
    }
    function restoreEvictedSessionDraft(id, buf) {
      if (!id || !buf || buf.composerDraft) return;
      const draft = evictedSessionDrafts[id];
      if (draft) buf.composerDraft = draft;
    }
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
    if (!sessionStates[id]) {
      sessionStates[id] = freshBuffer();
      restoreEvictedSessionDraft(id, sessionStates[id]);
    }
    return touchSessionBuffer(id, sessionStates[id], id.indexOf("sched-") === 0);
  }
  function isProtectedScheduledBuffer(id, buf) {
    return id === state.activeSessionId ||
      !!buf.busy ||
      !!buf.remoteTurnActive ||
      buf.scheduledInitialTurnPhase === "active" ||
      !!(buf.queued && buf.queued.length) ||
      !!(state.scheduledRunContext && state.scheduledRunContext.sessionId === id) ||
      state.scheduledTaskCreationSessionId === id;
  }
  function pruneScheduledSessionBuffers(keepId) {
    const scheduledIds = Object.keys(sessionStates).filter(function (id) {
      return !!sessionStates[id].scheduledRunSession;
    });
    let overflow = scheduledIds.length - MAX_SCHEDULED_SESSION_BUFFERS;
    if (overflow <= 0) return;
    scheduledIds.sort(function (left, right) {
      const delta = (sessionStates[left].lastTouched || 0) - (sessionStates[right].lastTouched || 0);
      return delta || left.localeCompare(right);
    });
    for (let i = 0; i < scheduledIds.length && overflow > 0; i++) {
      const id = scheduledIds[i];
      const buf = sessionStates[id];
      if (!buf || id === keepId || isProtectedScheduledBuffer(id, buf)) continue;
      if (!stashEvictedSessionDraft(id, buf)) continue; // draft cannot be safely retained; keep the buffer
      delete sessionStates[id];
      delete turnUsageDirty[id];
      // personaPlaceholderTitles is lightweight session metadata (the marker
      // for placeholder titles that auto-rename may override); rehydration
      // never restores it, so capacity eviction must keep it and only real
      // session deletion (purgeSessionBuffer) cleans it.
      pruneScheduledRunSessionOwner(id);
      if (onSessionBufferPurged) onSessionBufferPurged(id, "evict");
      overflow -= 1;
    }
  }
  function touchSessionBuffer(id, buf, scheduled) {
    if (!buf) return null;
    if (scheduled) buf.scheduledRunSession = true;
    buf.lastTouched = ++sessionBufferTouchClock;
    if (buf.scheduledRunSession) pruneScheduledSessionBuffers(id);
    pruneSessionBuffers(id);
    return buf;
  }
  // All-session LRU: the scheduled protection predicates still apply (busy/
  // queued/remote turns are never reclaimed); only idle buffers are evicted.
  // messages/chatItems rehydrate from disk; non-rehydratable drafts such as
  // composerDraft move to the evictedSessionDrafts side table first and are
  // restored on rebuild. The active session is covered by isProtected as a
  // final backstop, and a buffer whose draft cannot be safely stashed is kept
  // resident rather than evicted.
  function pruneSessionBuffers(keepId) {
    const ids = Object.keys(sessionStates);
    let overflow = ids.length - MAX_SESSION_BUFFERS;
    if (overflow <= 0) return;
    ids.sort(function (left, right) {
      const delta = (sessionStates[left].lastTouched || 0) - (sessionStates[right].lastTouched || 0);
      return delta || left.localeCompare(right);
    });
    for (let i = 0; i < ids.length && overflow > 0; i++) {
      const id = ids[i];
      const buf = sessionStates[id];
      if (!buf || id === keepId || isProtectedScheduledBuffer(id, buf)) continue;
      if (!stashEvictedSessionDraft(id, buf)) continue; // draft cannot be safely retained; keep the buffer
      delete sessionStates[id];
      delete turnUsageDirty[id];
      // personaPlaceholderTitles survives capacity eviction (see the comment
      // at the scheduled eviction site); only purgeSessionBuffer (real
      // deletion) cleans it.
      if (onSessionBufferPurged) onSessionBufferPurged(id, "evict");
      overflow -= 1;
    }
  }
  function purgeSessionBuffer(id) {
    if (typeof id !== "string" || !id) return;
    delete sessionStates[id];
    // Real session deletion: any stashed draft is invalidated too and must not flow back into a rebuilt buffer with the same id.
    delete evictedSessionDrafts[id];
    if (onSessionBufferPurged) onSessionBufferPurged(id, "delete");
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
    let owner = scheduledRunSessionOwners[id];
    if (!owner) owner = scheduledRunSessionOwners[id] = { phase: null, lastTouched: 0 };
    if (owner.phase !== "terminal" && phase) owner.phase = phase;
    owner.lastTouched = ++scheduledRunOwnerTouchClock;
    pruneScheduledRunSessionOwners();
    return owner;
  }
  function scheduledRunOwnerVisibleRank(id) {
    const runs = state.scheduledTaskRuns || [];
    for (let i = 0; i < runs.length; i++) {
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
    if (!scheduledRunSessionOwners[id] || isProtectedScheduledRunOwner(id)) return;
    delete scheduledRunSessionOwners[id];
  }
  function pruneScheduledRunSessionOwners() {
    const ids = Object.keys(scheduledRunSessionOwners);
    if (ids.length <= MAX_SCHEDULED_RUN_SESSION_OWNERS) return;
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
      const touchDelta = (scheduledRunSessionOwners[right].lastTouched || 0) -
        (scheduledRunSessionOwners[left].lastTouched || 0);
      return touchDelta || left.localeCompare(right);
    });
    for (let i = MAX_SCHEDULED_RUN_SESSION_OWNERS; i < ids.length; i++) {
      delete scheduledRunSessionOwners[ids[i]];
    }
  }
  function isScheduledRunTerminal(status) {
    const value = String(status || "").toLowerCase();
    return ["completed", "failed", "canceled"].includes(value);
  }
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
  function scheduledRunBuffer(id) {
    const buf = getBuffer(id);
    if (!buf) return null;
    registerScheduledRunOwner(id, null);
    return touchSessionBuffer(id, buf, true);
  }
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
  function beginScheduledOpenActivation(id) {
    const previous = sessionStates[id] || null;
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
  function rollbackScheduledOpenActivation(snapshot) {
    if (!snapshot || !snapshot.changed) return;
    const current = sessionStates[snapshot.id];
    if (!current || current !== snapshot.buffer) return;
    if (current.scheduledInitialTurnPhase === "terminal") return;
    if (current.lastTouched !== snapshot.activationTouch) return;
    if (snapshot.existed) {
      current.scheduledInitialTurnPhase = snapshot.previousPhase;
      current.busy = snapshot.previousBusy;
    } else {
      delete sessionStates[snapshot.id];
    }
    if (state.activeSessionId === snapshot.id && snapshot.previousStateBusy !== null) {
      state.busy = snapshot.previousStateBusy;
    }
  }
  function saveWorkingSetTo(buf) {
    if (!buf) return;
    buf.messages = state.messages; buf.chatItems = state.chatItems; buf.artifacts = state.artifacts;
    buf.composerDraft = state.composerDraft || "";
    buf.turnTimeline = state.turnTimeline;
    buf.activeTurnTimelineId = state.activeTurnTimelineId;
    buf.personaEvents = state.personaEvents;
    buf.pinvouReviews = state.pinvouReviews;
    buf.pinvouSceneEvents = state.pinvouSceneEvents;
    buf.steeredMessages = state.steeredMessages;
    buf.busy = buf.scheduledInitialTurnPhase === "active" ? true : state.busy;
    buf.planSnapshot = state.planSnapshot; buf.modeState = state.modeState;
    buf.thinking = state.thinking; buf.tokens = state.tokens; buf.queued = state.queued;
    buf.activePersona = state.activePersona;
    buf.mountedCollection = state.mountedCollection;
    buf.mountedCollections = state.mountedCollections;
    buf.mountedCollectionsRevision = state.mountedCollectionsRevision;
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
    state.composerDraft = buf.composerDraft || "";
    state.turnTimeline = buf.turnTimeline || [];
    state.activeTurnTimelineId = buf.activeTurnTimelineId || null;
    state.personaEvents = buf.personaEvents || [];
    state.pinvouReviews = buf.pinvouReviews || [];
    state.pinvouSceneEvents = buf.pinvouSceneEvents || [];
    state.steeredMessages = buf.steeredMessages || [];
    state.pinvouModal = null; // 切 session 关掉检阅弹窗
    state.turnDirtyArtifacts = []; // turn 临时态,切 session 清空,别串到新 session
    state.turnPresentedArtifacts = [];
    state.busy = buf.scheduledInitialTurnPhase === "active" ? true : buf.busy;
    state.planSnapshot = buf.planSnapshot; state.modeState = buf.modeState;
    state.thinking = buf.thinking; state.tokens = buf.tokens; state.queued = buf.queued || [];
    state.activePersona = buf.activePersona || null;
    state.mountedCollection = buf.mountedCollection || null;
    state.mountedCollections = Array.isArray(buf.mountedCollections)
      ? buf.mountedCollections
      : (state.mountedCollection == null ? [] : [{ collectionId: state.mountedCollection, enabled: true }]);
    state.mountedCollectionsRevision = Number(buf.mountedCollectionsRevision || 0);
    state.scheduledTaskDraft = buf.scheduledTaskDraft || null;
    const s = buf.stream || {};
    context.currentStreamText = s.currentStreamText || ""; context.currentStreamId = s.currentStreamId || 0;
    context.pendingAssistantText = s.pendingAssistantText || ""; context.pendingAssistantBlocks = s.pendingAssistantBlocks || [];
    context.itemIdSeq = s.itemIdSeq || 0; context.toolMeta = s.toolMeta || {};
  }
  function hydrateWorkingSetFromSaved(buf, saved) {
    if (!buf || !saved) return;
    const completedRemoteTurn = !!buf.remoteTerminalSeen || (!!buf.remoteTurnActive && !buf.busy);
    buf.messages = Array.isArray(saved.messages) ? saved.messages : [];
    buf.sessionRevision = String(saved.transcript_revision || saved.transcriptRevision || "");
    buf.chatItems = [];
    buf.turnTimeline = [];
    buf.activeTurnTimelineId = null;
    buf.artifacts = Array.isArray(saved.artifacts) ? saved.artifacts.map(function (a) {
      const p = typeof a === "string" ? a : (a.storage_path || a.path || "");
      return { path: p, basename: basename(p) };
    }) : [];
    buf.artifacts = filterSessionArtifacts(buf.artifacts, saved.metadata && saved.metadata.id);
    buf.personaEvents = [];
    buf.pinvouReviews = [];
    buf.pinvouSceneEvents = loadPinvouSceneEventsForSession(saved.metadata && saved.metadata.id);
    buf.steeredMessages = loadSteeredMessagesForSession(saved.metadata && saved.metadata.id);
    if (completedRemoteTurn) {
      buf.remoteTurnActive = false;
      buf.remoteTerminalSeen = false;
      buf.remoteBaselineMessageCount = null;
      buf.remoteBaselineTrusted = false;
      buf.remoteExpectedAssistantKey = "";
      buf.remoteCommittedRevision = "";
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
    const buf = getBuffer(sid);
    const meta = state.sessions.find(function (s) { return s.id === sid; }) || {};
    const knownCount = Number(meta.message_count || 0);
    if (buf.busy) return;
    if (buf.loadedFromDisk && (!knownCount || buf.messages.length >= knownCount)) return;
    if (!buf.loadedFromDisk && buf.messages.length && (!knownCount || buf.messages.length >= knownCount)) return;
    const saved = await invoke("load_session", { id: sid, setActive: false });
    const savedMessages = saved && Array.isArray(saved.messages) ? saved.messages : [];
    const savedMetadataCount = saved && saved.metadata ? Number(saved.metadata.message_count || 0) : 0;
    const savedCount = Math.max(Number.isFinite(savedMetadataCount) ? savedMetadataCount : 0, savedMessages.length);
    // Shell 轮询等后台展示项会先写入 chatItems，但不代表会话正文已经加载。
    // 只有内存里确有 transcript messages 且不短于磁盘版本时，才能跳过 hydration。
    if (buf.messages.length && savedCount <= buf.messages.length) {
      buf.loadedFromDisk = true;
      return;
    }
    // 下载挂起期间后台回合可能已开始（busy 置位、直播流写入中）：此时用磁盘
    // 快照 hydrate 会截断正在流式生成的内容，必须复检后放弃（审计）。
    if (buf.busy || buf.remoteTurnActive) return;
    hydrateWorkingSetFromSaved(buf, saved);
    try { buf.personaEvents = await invoke("get_session_persona_events", { sessionId: sid }) || []; } catch { buf.personaEvents = []; }
    try { buf.pinvouReviews = await invoke("get_session_pinvou_reviews", { sessionId: sid }) || []; } catch { buf.pinvouReviews = []; }
    buf.pinvouSceneEvents = await syncPinvouSceneEventsForSession(sid);
    buf.steeredMessages = await syncSteeredMessagesForSession(sid);
    try { buf.turnTimeline = await invoke("get_session_timeline", { sessionId: sid }) || []; } catch { buf.turnTimeline = []; }
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
    // 离开草稿（无论物化还是切去既有会话），未消费的开关寄存意图作废。
    state.pendingDraftMultiAgent = false;
    // Set the new active before touching the old buffer: the LRU
    // eviction triggered by touch relies on activeSessionId as the
    // backstop protecting the target session; touching the old one first
    // (active still pointing at the old value) would evict an idle target
    // that happens to be oldest, and the subsequent freshBuffer()
    // replacement would silently show an empty session.
    const previousActiveId = state.activeSessionId;
    state.activeSessionId = id;
    if (previousActiveId) saveWorkingSetTo(getBuffer(previousActiveId));
    let buf = sessionStates[id];
    if (!buf || (opts && opts.fresh)) {
      buf = sessionStates[id] = freshBuffer();
      // fresh is for sessions just materialized on this side: the empty
      // buffer is the authoritative view and must be marked
      // loadedFromDisk — otherwise switchToSessionInternal's fast-path
      // gate routes it to the slow path as an event-rebuilt incomplete
      // buffer, the freshBuffer() replacement bypasses the side-table
      // stash, and the buffer's unsent draft is silently dropped (same
      // gate as the web bridge). fresh does not restore a side-table
      // draft: id-reuse scenarios must not resurrect an old stash.
      if (opts && opts.fresh) buf.loadedFromDisk = true;
      else restoreEvictedSessionDraft(id, buf);
    }
    touchSessionBuffer(id, buf, id.indexOf("sched-") === 0);
    loadWorkingSetFrom(buf);
    state.artifacts = filterSessionArtifacts(state.artifacts, id);
    scheduleShellPoll(id, true);
  }
  // 在指定 session 的工作集上跑一段【同步】逻辑。sid 是 active → 直接跑(零行为变化);
  // 否则临时切到该 buffer 跑完再切回(期间不 notify)。
  // 整表覆盖式刷新：并发调用（list_changed 事件、chat:done 收尾、归档/改名等操作）
  // 乱序返回时旧列表会覆盖新列表（如刚删除的会话复活、改名被回退）。用请求序号
  // 做后发者胜（审计）。
  let historyListSeq = 0;
  async function refreshHistoryList() {
    const seq = ++historyListSeq;
    try {
      const sessions = await invoke("list_sessions");
      if (seq !== historyListSeq) return;
      state.sessions = sessions;
    } catch (e) {
      if (seq !== historyListSeq) return;
      console.warn("list_sessions failed", e);
      state.sessions = [];
    }
    try {
      const archivedSessions = await invoke("list_archived_sessions");
      if (seq !== historyListSeq) return;
      state.archivedSessions = archivedSessions;
    } catch {
      if (seq !== historyListSeq) return;
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
    // 新草稿从关闭状态开始：寄存意图作废，开关行显示同步复位。
    state.pendingDraftMultiAgent = false;
    // 绑定草稿的显式 mode 暂存同属寄存意图，随草稿一并作废。
    state.pendingDraftMode = null;
    // 新草稿回到默认工作区：上一份草稿的目录选择不带入（两个提前返回分支共用此复位）。
    state.draftWorkspacePath = null;

    // 已在干净草稿态 → 只 notify(epoch 已自增)。注意要连 chatItems 一起判空:messages 与 chatItems
    // 会背离(persona 气泡 / ensureSession 失败的 system 报错卡只进 chatItems),否则残留卡顶掉「你好」。
    if (!state.activeSessionId && state.messages.length === 0 && state.chatItems.length === 0) {
      state.composerDraft = "";
      // 草稿 mode 显示 = 当前 lane 全局默认（三分 lane 语义）。
      state.modeState = currentDraftModeState();
      notify();
      return;
    }
    if (state.activeSessionId) saveWorkingSetTo(getBuffer(state.activeSessionId));
    state.activeSessionId = null;
    loadWorkingSetFrom(freshBuffer());
    // freshBuffer 的 modeState 是通用缺省（yolo）；草稿显示须覆盖为本 lane
    // 全局默认（work/design 各自的 last_mode）。
    state.modeState = currentDraftModeState();
    notify();
  }
  // 公开「新建对话」入口(侧边栏按钮)= 进草稿态。名字保留以兼容前端调用。
  async function createNewSession() { enterDraft(); }

  // ── 草稿态工作目录选择（普通聊天，对齐 code 模式草稿选择器）────────────
  // 最近列表与 src/shared/workspace-recents.js 同 key 同语义：本文件是
  // <script src> 经典脚本，无法 import 该 ES 模块，下面是它的逐字镜像，
  // 改动任一侧必须同步另一侧（tests/chat_draft_workspace_logic.test.mjs
  // 锁定桥侧行为，tests/workspace_recents_logic.test.mjs 锁定共享模块）。
  const DRAFT_WORKSPACE_RECENTS_KEY = "pinvou_codex_recent_workspaces";
  function rememberDraftWorkspaceRecent(path) {
    let list;
    try {
      const value = JSON.parse(localStorage.getItem(DRAFT_WORKSPACE_RECENTS_KEY) || "[]");
      list = Array.isArray(value) ? value.filter(function (item) { return typeof item === "string"; }).slice(0, 6) : [];
    } catch {
      list = [];
    }
    const next = [path, ...list.filter(function (item) { return item !== path; })].slice(0, 6);
    try {
      localStorage.setItem(DRAFT_WORKSPACE_RECENTS_KEY, JSON.stringify(next));
    } catch {
      // localStorage 不可用时仅本次不记忆，不影响选目录本身。
    }
  }
  // 仅草稿态生效；path = null 表示回到默认（会话私有目录）。
  function setDraftWorkspace(path) {
    if (state.activeSessionId) return;
    state.draftWorkspacePath = path || null;
    // 绑定/解绑即切换草稿 mode 显示 lane（绑定 → code lane，解绑 → 回本 lane
    // 默认）；解绑时上一份绑定草稿的显式 mode 暂存一并作废，不带入未绑定草稿。
    if (!state.draftWorkspacePath) state.pendingDraftMode = null;
    state.modeState = currentDraftModeState();
    notify();
  }
  // 系统目录选择对话框：选中后记入最近列表并写回草稿选择，返回选中的 path；
  // 用户取消（或对话框不可用/非草稿态）返回 null，不改变现有选择。
  async function pickDraftWorkspace() {
    if (state.activeSessionId || !dialogOpen) return null;
    const selected = await dialogOpen({ directory: true, multiple: false, title: bt("pickFolderTitle") });
    const path = Array.isArray(selected) ? selected[0] : selected;
    if (!path) return null;
    rememberDraftWorkspaceRecent(path);
    setDraftWorkspace(path);
    return path;
  }

  // 已生成会话的工作目录绑定（普通聊天绑定目录会话，安全姿态对齐 code 模式）：
  // 返回绑定的完整路径；未绑定 / Web 与远程端无此命令 / 查询失败一律按 null
  // 处理（UI 不显示绑定指示，YOLO 确认门也不因此误触发）。
  async function getSessionWorkspaceBinding(sessionId) {
    if (!sessionId) return null;
    try {
      const binding = await invoke("get_session_workspace_binding", { sessionId });
      return typeof binding === "string" && binding ? binding : null;
    } catch {
      return null;
    }
  }

  // 草稿态首次有实质内容时真正向后端创建 session 并切为 active;已有 active 直接返回。
  // 返回新 session id,创建失败返回 null。调用方:sendMessage(首条消息) / equipPersona(加卡)。
  // 并发防护（审计）：草稿态双击发送会并发 create_session，导致两条消息分家到两个新
  // 会话——in-flight 复用同一 promise；create_session await 期间用户切走会物化在错误
  // 会话（导航被劫持）——物化前校验 activeSessionId 仍为空，已切走则只登记后台 buffer。
  let ensureSessionInFlight = null;
  async function ensureSession() {
    if (state.activeSessionId) return state.activeSessionId;
    if (ensureSessionInFlight) return ensureSessionInFlight;
    // 捕获导航 token：仅判 activeSessionId 覆盖不了「再进草稿」——enterDraft
    // 只推进 token 不改 activeSessionId（仍为 null），在途 create_session 返回
    // 后必须连同 token 一起校验，否则会劫持用户新进的草稿（三审 P1）。
    const navToken = sessionSwitchRequestToken;
    const p = (async function () {
      // 多 session 并发:不预热 engine。新建空 session 的 buffer 由 switchActiveTo({fresh}) 起。
      try {
        // 草稿选定的工作目录随物化一并下发；null = 后端现状（会话私有目录）。
        // 参数在 invoke 同步求值时捕获，await 期间的后续选择不影响本次创建。
        // boundWorkspace 同步捕获：物化后的 lane 默认应用以本次创建是否绑定为准。
        const boundWorkspace = state.draftWorkspacePath || null;
        const meta = await invoke("create_session", { workspacePath: boundWorkspace });
        // create_session 等待期间用户可能已发送/清空输入，必须读取最新值，
        // 不能把 await 前的已发送文本带入新 session。
        const composerDraft = state.composerDraft || "";
        // create_session 等待期间用户可能已退出草稿（切到既有会话或再进草稿）：
        // 物化不得劫持 active（审计 F1），新会话登记为后台 buffer 等下次切换，
        // 调用方按 null 处理不发送本条消息。离开草稿的寄存开关意图一并作废。
        // 「切到既有会话」→ activeSessionId 非空；「再进草稿」→ activeSessionId
        // 仍为 null 但导航 token 已前移——两种导航都中止物化（三审 P1）。
        if (state.activeSessionId || navToken !== sessionSwitchRequestToken) {
          state.pendingDraftMultiAgent = false;
          state.pendingDraftMode = null;
          sessionStates[meta.id] = freshBuffer();
          sessionStates[meta.id].loadedFromDisk = true;
          return null;
        }
        // 草稿期开的多智能体开关此刻才落后端（开关本身不物化会话）。先取后
        // 清：switchActiveTo 会把寄存意图当作已消费。
        const pendingMultiAgent = state.pendingDraftMultiAgent === true;
        state.pendingDraftMultiAgent = false;
        // 绑定草稿的显式 mode 暂存同样先取后清（读取最新值：await 期间的
        // 显式切换也算用户意图，与 pendingMultiAgent 同一约定）。
        const stagedDraftMode = state.pendingDraftMode;
        state.pendingDraftMode = null;
        // 物化已提交：目录选择随会话落地，清除草稿选择；create_session 失败
        // （外层 catch 路径）则保留选择以便用户重试。
        state.draftWorkspacePath = null;
        switchActiveTo(meta.id, { fresh: true });
        // 草稿态因首条消息/加卡等实质操作物化为 session 时，输入草稿也要
        // 跟随迁移；这不是用户主动切换到另一个已有会话。
        state.composerDraft = composerDraft;
        sessionStates[meta.id].composerDraft = composerDraft;
        if (pendingMultiAgent) {
          try {
            await invoke("set_multi_agent_mode", { sessionId: meta.id, enabled: true });
          } catch (toggleError) {
            // 开关落盘失败不得让首条消息静默退化成普通对话（复核 P1）：
            // 中止物化——删掉刚建的空会话、回到草稿并保留开关意图，等用户
            // 处理环境或权限问题后重试。调用方以 activeSessionId 为空判定
            // 中止，不发送本条消息。
            try {
              await invoke("delete_session", { id: meta.id });
            } catch {
              // 空会话残留可手动删除，不掩盖主错误。
            }
            enterDraft();
            // 回退草稿保留寄存意图：绑定与显式 mode 暂存被 enterDraft 复位，
            // 须按失败前取到的值原样恢复——重试物化不偏离用户显式选择。
            state.pendingDraftMultiAgent = true;
            state.draftWorkspacePath = boundWorkspace;
            state.pendingDraftMode = stagedDraftMode || null;
            state.modeState = {
              mode: stagedDraftMode || currentDraftModeState().mode,
              multiAgent: true,
            };
            addSystemItem(bt("switchModeFailed") + toggleError);
            await refreshHistoryList();
            notify();
            return null;
          }
        }
        await refreshHistoryList();
        await syncModeState();
        if (boundWorkspace) {
          // 绑定工作目录的会话安全姿态对齐 code 模式：后端已为绑定会话按
          // code lane 全局默认解析 mode，此处不再把 work/design lane 默认经
          // set_plan_mode_next 套用；仅当用户在草稿态显式暂存过 mode 选择时
          // 按暂存值应用（切 yolo 的一次性确认门在草稿切换时已由 ChatView 过过）。
          if (stagedDraftMode === "plan" || stagedDraftMode === "yolo") {
            // 用物化时捕获的 meta.id 而非 activeSessionId：上面的 await 期间
            // 用户可能已切走，对当前 active 会话执行 mode 命令会改错对象。
            try {
              const stagedModeState = stagedDraftMode === "plan"
                ? await invoke("set_plan_mode_next", { sessionId: meta.id })
                : await invoke("exit_plan_to_yolo", { sessionId: meta.id });
              applyAuthoritativeModeState(meta.id, stagedModeState);
            } catch (stagedModeError) {
              runSyncOnSession(meta.id, function () {
                addSystemItem(bt("switchModeFailed") + stagedModeError);
              });
            }
          }
        } else {
        // 三分 lane 语义：后端 plain 缺省恒 Yolo、不区分 work/design 两个 lane；
        // 新会话所在 lane 的全局默认为 plan 时，在物化此刻显式应用（写入即成为
        // 该会话自己的 per-session 记录，全局默认不受影响）。
        const laneDefault = state.modeDefaults
          && state.modeDefaults[state.modeLane === "design" ? "design" : "work"];
        // 用物化时捕获的 meta.id 而非 activeSessionId：上面的 await 期间用户
        // 可能已切走，对当前 active 会话执行 set_plan_mode_next 会改错对象。
        if (laneDefault === "plan") {
          try {
            const laneModeState = await invoke("set_plan_mode_next", { sessionId: meta.id });
            applyAuthoritativeModeState(meta.id, laneModeState);
          } catch (laneModeError) {
            runSyncOnSession(meta.id, function () {
              addSystemItem(bt("switchModeFailed") + laneModeError);
            });
          }
        }
        }
        await syncActivePersona();
        await syncMountedCollection();
        notify();
        // 尾部这些 await 期间用户仍可能切走（activeSessionId 已是别的会话）或
        // 再进草稿（activeSessionId 仍为 null 但 token 已前移）：与 create_session
        // 窗口同一契约——导航即物化中止，返回 null 让调用方放弃（消息回填输入框），
        // 不得返回切走后的 active 让操作漂进新会话（二审 F1、三审 P1）。
        // 返回非 null 时 active 必等于 meta.id 且无任何新导航，调用方重读
        // state.activeSessionId 即为目标会话。
        return navToken === sessionSwitchRequestToken
          && state.activeSessionId === meta.id ? meta.id : null;
      } catch (e) {
        addSystemItem(bt("newChatFailed") + e);
        return null;
      }
    })();
    ensureSessionInFlight = p;
    p.then(
      function () { if (ensureSessionInFlight === p) ensureSessionInFlight = null; },
      function () { if (ensureSessionInFlight === p) ensureSessionInFlight = null; }
    );
    return p;
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

  function mergeHydratedChatItems(liveChatItems, liveCurrentStreamId) {
    let remappedCurrentStreamId = 0;
    const availableByKey = Object.create(null);
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
    state.chatItems.forEach(function (item, index) {
      const key = hydratedChatItemKey(item);
      if (!key) return;
      if (!availableByKey[key]) availableByKey[key] = [];
      availableByKey[key].push(index);
    });
    (liveChatItems || []).forEach(function (item) {
      const key = hydratedChatItemKey(item);
      const range = interruptedDisplayRange(item);
      let existingIndex = -1;
      if (range) {
        for (let rangeIndex = range.start; rangeIndex < range.end; rangeIndex++) {
          const rangeItem = state.chatItems[rangeIndex];
          if (rangeItem && rangeItem.interruptedDisplayOnly !== true &&
              hydratedChatItemKey(rangeItem) === key) {
            existingIndex = rangeIndex;
            break;
          }
        }
      } else {
        const matches = key && availableByKey[key];
        existingIndex = matches && matches.length ? matches.shift() : -1;
      }
      if (existingIndex >= 0) {
        const existingId = state.chatItems[existingIndex].id;
        state.chatItems[existingIndex] = Object.assign({}, state.chatItems[existingIndex], item, {
          id: existingId,
        });
        if (item && item.id === liveCurrentStreamId) remappedCurrentStreamId = existingId;
        return;
      }
      const clone = Object.assign({}, item, { id: ++context.itemIdSeq });
      if (item && item.id === liveCurrentStreamId) remappedCurrentStreamId = clone.id;
      if (range && range.end < state.chatItems.length) {
        state.chatItems.splice(range.end, 0, clone);
        Object.keys(availableByKey).forEach(function (availableKey) {
          availableByKey[availableKey] = availableByKey[availableKey].map(function (index) {
            return index >= range.end ? index + 1 : index;
          });
        });
      } else state.chatItems.push(clone);
    });
    return remappedCurrentStreamId;
  }

  // eslint-disable-next-line sonarjs/cognitive-complexity -- legacy bridge; refactor tracked separately
  async function switchToSessionInternal(id, preserveScheduledRunContext, errorScope, options) {
    const requestToken = ++sessionSwitchRequestToken;
    const forceDurableLoad = !!(options && options.forceDurableLoad);
    const hydrateLiveSession = !!(options && options.hydrateLiveSession);
    if (!id) {
      reportSessionSwitchFailure(new Error(bt("runHasNoSession")), errorScope);
      return false;
    }
    if (hydrateLiveSession && !sessionStates[id]) {
      sessionStates[id] = freshBuffer();
      restoreEvictedSessionDraft(id, sessionStates[id]);
    }
    if (id === state.activeSessionId && !forceDurableLoad && !hydrateLiveSession) {
      if (!preserveScheduledRunContext) state.scheduledRunContext = null;
      state.scheduledTaskPendingGuide = null;
      notify();
      return true;
    }
    // 多 session 并发:切换【不再 cancel】旧 session —— 它在自己的 engine 上继续跑,
    // 工作集存进 sessionStates 后台累积。切回来能看到完整(含切走期间产生的)内容。
    // 已有 buffer(切过/在跑)→ 直接换工作集;没有 → load_session 建 buffer + 重渲染。
    // Event listeners rebuild an unsaved empty buffer via getBuffer for
    // any session an event names (e.g. a late chat:usage/artifact:disk
    // after eviction); taking the fast path with such a buffer shows an
    // empty conversation, so it must fall through to the disk reload
    // below to self-heal (same loadedFromDisk gate as the web bridge).
    // Buffers that are busy/remote/have messages still take the fast
    // path, showing the live partial view completed by the chat:done
    // reconcile. chatItems must not be the usability check: an empty
    // buffer rebuilt by a non-turn event (e.g. chat:compaction) carries
    // only one system chatItem (no busy flag, no messages), and admitting
    // it by chatItems would permanently show a history-less view with no
    // self-heal.
    const existingBuffer = sessionStates[id];
    const cachedBufferUsable = existingBuffer && (existingBuffer.loadedFromDisk ||
      existingBuffer.busy || existingBuffer.remoteTurnActive ||
      (existingBuffer.messages && existingBuffer.messages.length));
    if (cachedBufferUsable && !forceDurableLoad && !hydrateLiveSession) {
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
    let saved;
    try {
      saved = await invoke("load_session", { id });
    } catch (e) {
      if (requestToken === sessionSwitchRequestToken) reportSessionSwitchFailure(e, errorScope);
      return false;
    }
    if (requestToken !== sessionSwitchRequestToken) return false;
    if (!saved || !saved.metadata || !saved.metadata.id) {
      reportSessionSwitchFailure(new Error(bt("sessionDataInvalid")), errorScope);
      return false;
    }

    let personaEvents = [];
    let pinvouReviews = [];
    const pinvouSceneEvents = await syncPinvouSceneEventsForSession(id);
    const steeredMessages = await syncSteeredMessagesForSession(id);
    let turnTimeline = [];
    try { personaEvents = await invoke("get_session_persona_events", { sessionId: id }) || []; } catch { /* optional data; default to empty */ }
    try { pinvouReviews = await invoke("get_session_pinvou_reviews", { sessionId: id }) || []; } catch { /* optional data; default to empty */ }
    try { turnTimeline = await invoke("get_session_timeline", { sessionId: id }) || []; } catch { /* optional data; default to empty */ }
    if (requestToken !== sessionSwitchRequestToken) return false;

    // load_session 与必要的直接会话数据均成功后，才一次性提交 active/context。
    if (state.activeSessionId) saveWorkingSetTo(getBuffer(state.activeSessionId));
    if (!preserveScheduledRunContext) state.scheduledRunContext = null;
    state.scheduledTaskPendingGuide = null;
    state.activeSessionId = saved.metadata.id;
    if (hydrateLiveSession) {
      const liveBuffer = sessionStates[id] || freshBuffer();
      loadWorkingSetFrom(liveBuffer);
      const liveMessages = Array.isArray(state.messages) ? [...state.messages] : [];
      const liveChatItems = Array.isArray(state.chatItems) ? [...state.chatItems] : [];
      const liveArtifacts = Array.isArray(state.artifacts) ? [...state.artifacts] : [];
      const liveCurrentStreamId = context.currentStreamId;
      const hasLivePresentation = !!state.busy || !!context.currentStreamText || !!context.pendingAssistantText ||
        (Array.isArray(context.pendingAssistantBlocks) && context.pendingAssistantBlocks.length > 0);
      state.messages = mergeHydratedMessages(
        saved.messages,
        liveMessages,
        isScheduledRunSession(id)
      );
      state.personaEvents = personaEvents.length ? personaEvents : (liveBuffer.personaEvents || []);
      state.pinvouReviews = pinvouReviews.length ? pinvouReviews : (liveBuffer.pinvouReviews || []);
      state.pinvouSceneEvents = pinvouSceneEvents.length ? pinvouSceneEvents : (liveBuffer.pinvouSceneEvents || []);
      state.steeredMessages = steeredMessages.length ? steeredMessages : (liveBuffer.steeredMessages || []);
      state.turnTimeline = turnTimeline.length ? turnTimeline : (liveBuffer.turnTimeline || []);
      state.artifacts = filterSessionArtifacts(
        mergeHydratedArtifacts(saved.artifacts, liveArtifacts),
        state.activeSessionId
      );
      // Live hydration: the buffer's toolMeta may hold in-flight tool
      // entries (tool_use not yet in messages); keep them for the later
      // chat:tool_end.
      rerenderFromMessages({ keepLiveToolMeta: true });
      if (hasLivePresentation) {
        context.currentStreamId = mergeHydratedChatItems(liveChatItems, liveCurrentStreamId);
      } else {
        resetPendingAssistant();
      }
      saveWorkingSetTo(liveBuffer);
    } else {
      sessionStates[id] = freshBuffer();
      // The slow-path disk rehydration bypasses getBuffer; the draft stashed at eviction time must be restored here.
      restoreEvictedSessionDraft(id, sessionStates[id]);
      loadWorkingSetFrom(sessionStates[id]);
      state.messages = Array.isArray(saved.messages) ? saved.messages : [];
      sessionStates[id].loadedFromDisk = true;
      state.personaEvents = personaEvents;
      state.pinvouReviews = pinvouReviews;
      state.pinvouSceneEvents = pinvouSceneEvents;
      state.steeredMessages = steeredMessages;
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
    const sessionId = run && typeof run.sessionId === "string" ? run.sessionId.trim() : "";
    if (!sessionId) {
      reportSessionSwitchFailure(new Error(bt("runHasNoSession")), "scheduled");
      return false;
    }
    rememberScheduledRunOwner(run);
    const runStatus = String(run && run.status || "").toLowerCase();
    let openActivation = null;
    if (runStatus === "queued" || runStatus === "running") {
      openActivation = beginScheduledOpenActivation(sessionId);
    } else {
      scheduledRunBuffer(sessionId);
    }
    setScheduledTaskError(null);
    notify();
    const returnSessionId = state.scheduledRunContext
      ? state.scheduledRunContext.returnSessionId
      : state.activeSessionId;
    const liveBuffer = sessionStates[sessionId];
    const hasLiveTurn = !!(liveBuffer && (
      liveBuffer.busy ||
      liveBuffer.scheduledInitialTurnPhase === "active" ||
      (liveBuffer.queued && liveBuffer.queued.length) ||
      (liveBuffer.thinking && liveBuffer.thinking.active)
    ));
    const isTerminalRun = ["completed", "failed", "canceled"].includes(runStatus);
    const forceDurableLoad = isTerminalRun && !hasLiveTurn;
    const switched = await switchToSessionInternal(sessionId, true, "scheduled", {
      forceDurableLoad,
      hydrateLiveSession: !isTerminalRun,
    });
    if (!switched) {
      rollbackScheduledOpenActivation(openActivation);
      notify();
      return false;
    }
    if (forceDurableLoad) markScheduledInitialTurnTerminal(sessionId);
    else scheduledRunBuffer(sessionId);
    const automationId = (run && run.automationId) || (task && task.id) || null;
    const runId = (run && (run.runId || run.id)) || null;
    state.scheduledRunContext = {
      sessionId,
      returnSessionId,
      automationId,
      runId,
      taskName: (task && task.name) || (run && (run.taskName || run.name)) || "",
      model: (task && task.model) || (run && run.taskModel) || null,
      mode: "yolo",
    };
    // 先发布完整会话视图；只有已完成的运行才持久化为已查看。
    notify();
    if (automationId && runId && runStatus === "completed") {
      try {
        const receipt = await invoke("mark_scheduled_run_viewed", {
          automationId,
          runId,
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
    const sessionId = run && typeof run.sessionId === "string" ? run.sessionId.trim() : "";
    if (!sessionId) return openScheduledRunChatOnce(run, task);
    if (scheduledRunOpenInFlight[sessionId]) return scheduledRunOpenInFlight[sessionId];
    const opening = openScheduledRunChatOnce(run, task);
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

  if (typeof listen === "function") {
    listen("session:deleted", function (event) {
      const payload = event && event.payload || {};
      applyDeletedSession(payload.id);
    }).catch(function (error) {
      console.error("[sessions] session:deleted listener failed", error);
    });
    listen("session:list_changed", function () {
      refreshHistoryList().catch(function (error) {
        console.error("[sessions] session:list_changed refresh failed", error);
      });
    }).catch(function (error) {
      console.error("[sessions] session:list_changed listener failed", error);
    });
    listen("session:model_changed", function (event) {
      const payload = event && event.payload || {};
      if (payload.id !== state.activeSessionId) return;
      Promise.resolve(loadSessionModel(payload.id)).catch(function (error) {
        console.error("[sessions] session:model_changed refresh failed", error);
      });
    }).catch(function (error) {
      console.error("[sessions] session:model_changed listener failed", error);
    });
    listen("session:persona_changed", function (event) {
      const payload = event && event.payload || {};
      if (payload.id !== state.activeSessionId) return;
      Promise.resolve(syncActivePersona()).then(notify).catch(function (error) {
        console.error("[sessions] session:persona_changed refresh failed", error);
      });
    }).catch(function (error) {
      console.error("[sessions] session:persona_changed listener failed", error);
    });
  }

  async function deleteSession(id) {
    try {
      // 后端按 SessionKind 分发:定时运行会话在 delete_session 里联动删除
      // 该次 Session、Run 与底座 Task,任务定义与共享工作间保留。
      await invoke("delete_session", { id });
      // 复用远端事件与本地操作的统一清理路径，并保留批量操作所需的结果语义。
      return applyDeletedSession(id);
    } catch (e) {
      addSystemItem(bt("deleteFailed") + e);
      return false;
    }
  }

  async function renameSession(id, title) {
    invalidateScheduledRecentRunsForSession(id);
    try {
      await invoke("rename_session", { id, title });
      const s = state.sessions.find(function (s) { return s.id === id; });
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
      const navToken = sessionSwitchRequestToken;
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
            && navToken === sessionSwitchRequestToken) {
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
    const navToken = sessionSwitchRequestToken;
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
          && navToken === sessionSwitchRequestToken) {
        state.activeSessionId = id;
        loadWorkingSetFrom(getBuffer(id));
      }
      console.warn("set_session_archived failed", e);
      notify();
      return false;
    }
  }

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

  // 实时态有专属气泡的工具（方案卡），重建时要还原成原卡而非普通工具卡。

    return {
      freshBuffer,
      getBuffer,
      isProtectedScheduledBuffer,
      pruneScheduledSessionBuffers,
      touchSessionBuffer,
      purgeSessionBuffer,
      registerScheduledRunOwner,
      scheduledRunOwnerVisibleRank,
      scheduledRunOwnerPriority,
      isProtectedScheduledRunOwner,
      pruneScheduledRunSessionOwner,
      pruneScheduledRunSessionOwners,
      isScheduledRunTerminal,
      rememberScheduledRunOwner,
      scheduledRunBuffer,
      markScheduledInitialTurnActive,
      markScheduledInitialTurnTerminal,
      beginScheduledOpenActivation,
      rollbackScheduledOpenActivation,
      saveWorkingSetTo,
      loadWorkingSetFrom,
      hydrateWorkingSetFromSaved,
      ensureSessionBufferLoaded,
      switchActiveTo,
      refreshHistoryList,
      enterDraft,
      createNewSession,
      setDraftWorkspace,
      pickDraftWorkspace,
      getSessionWorkspaceBinding,
      ensureSession,
      reportSessionSwitchFailure,
      hydratedMessageKey,
      mergeHydratedMessages,
      mergeHydratedArtifacts,
      hydratedChatItemKey,
      mergeHydratedChatItems,
      switchToSessionInternal,
      switchToSession,
      openScheduledRunChatOnce,
      openScheduledRunChat,
      exitScheduledRunChat,
      recentScheduledRunForSession,
      leaveSessionView,
      applyDeletedSession,
      deleteSession,
      renameSession,
      toggleSessionPinned,
      archiveSession,
      restoreArchivedSession
    };
  };
})(window);
