/** Session working sets, switching, hydration, and lifecycle operations. */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting statements would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry.sessions = function (context) {let pinvouSharedtauriSessionsCache = null;
function pinvouSharedtauriSessions() {
  if (!pinvouSharedtauriSessionsCache) pinvouSharedtauriSessionsCache = window.PinvouBridgeShared.create("tauriSessions", { sessionStates, restoreEvictedSessionDraft, state, userMessageDisplayText, evictedSessionDrafts: { get value() { return evictedSessionDrafts; } }, MAX_EVICTED_SESSION_DRAFT_CHARS: { get value() { return MAX_EVICTED_SESSION_DRAFT_CHARS; } }, MAX_EVICTED_SESSION_DRAFTS: { get value() { return MAX_EVICTED_SESSION_DRAFTS; } }, sessionBufferTouchClock: { get value() { return sessionBufferTouchClock; }, set value(v) { sessionBufferTouchClock = v; } }, pruneScheduledSessionBuffers, pruneSessionBuffers, scheduledRunSessionOwners, scheduledRunOwnerTouchClock: { get value() { return scheduledRunOwnerTouchClock; }, set value(v) { scheduledRunOwnerTouchClock = v; } }, MAX_SCHEDULED_RUN_SESSION_OWNERS, enterDraft, setScheduledTaskError, notify, addSystemItem, bt, basename, switchToSessionInternal, openScheduledRunChatOnce, scheduledRunOpenInFlight, invalidateScheduledRecentRunsForSession, purgeSessionBuffer, invoke, personaPlaceholderTitles, refreshHistoryList, sessionSwitchRequestToken: { get value() { return sessionSwitchRequestToken; }, set value(v) { sessionSwitchRequestToken = v; } }, loadScheduledTaskRecentRuns, reportSidecarReadFailure, historyRefreshTimer: { get value() { return historyRefreshTimer; }, set value(v) { historyRefreshTimer = v; } }, historyRefreshPending: { get value() { return historyRefreshPending; }, set value(v) { historyRefreshPending = v; } }, currentStreamText: { get value() { return context.currentStreamText; }, set value(v) { context.currentStreamText = v; } }, currentStreamId: { get value() { return context.currentStreamId; }, set value(v) { context.currentStreamId = v; } }, pendingAssistantText: { get value() { return context.pendingAssistantText; }, set value(v) { context.pendingAssistantText = v; } }, pendingAssistantBlocks: { get value() { return context.pendingAssistantBlocks; }, set value(v) { context.pendingAssistantBlocks = v; } }, itemIdSeq: { get value() { return context.itemIdSeq; }, set value(v) { context.itemIdSeq = v; } }, toolMeta: { get value() { return context.toolMeta; }, set value(v) { context.toolMeta = v; } }, restoreSteeredWorkingSet, saveSteeredWorkingSet });
  return pinvouSharedtauriSessionsCache;
}


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
    // System directory picker dialog (injected by bridge.js as TAURI.dialog.open;
    // React must not touch Tauri globals directly — draft workspace selection goes
    // through this wrapper). undefined when unavailable; pickDraftWorkspace
    // returns null early in that case.
    const dialogOpen = context.dialogOpen || null;
    // Shared normalized directory-picker scaffold (same bridge.js helper used
    // by pickFolders); returns null when unavailable or cancelled.
    const pickDirectory = context.pickDirectory;
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
    const reportSidecarReadFailure = context.reportSidecarReadFailure;
    // Persona events and pinvou reviews have no local migration cache, but the
    // backend now fails a corrupt sidecar explicitly (missing stays []). Degrade
    // to [] exactly as before, just not silently — report like the scene/steered
    // readers so a durable-data defect cannot pose as "no data".
    function loadPersonaEventsForSession(sid) { return pinvouSharedtauriSessions().loadPersonaEventsForSession(sid); }
    function loadPinvouReviewsForSession(sid) { return pinvouSharedtauriSessions().loadPinvouReviewsForSession(sid); }
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
    // Burst coalescing (review #463 round-20 R3): the rebind command emits one
    // session:list_changed per affected id, and each undebounced refresh costs
    // an IPC round trip + an O(n) backend list_sessions + a React commit — a
    // large rebind burst translated into hundreds of full history refreshes
    // exactly while the report dialog opens. Leading + trailing: the first
    // event refreshes immediately (single-event semantics unchanged), events
    // inside the window set a pending flag collapsed into ONE trailing
    // refresh. Harnesses without timers fall back to the immediate refresh.
    // Factory scope so the shared scheduleHistoryRefresh cell (deps literal)
    // can close over them.
    let historyRefreshTimer = null;
    let historyRefreshPending = false;
    const evictedSessionDrafts = Object.create(null);
    // Returns true when the buffer's non-rehydratable state is safely retained
    // (or empty) and the caller may drop the buffer; false means eviction must
    // be skipped so the draft stays in the live buffer.
    function stashEvictedSessionDraft(id, buf) { return pinvouSharedtauriSessions().stashEvictedSessionDraft(id, buf); }
    function restoreEvictedSessionDraft(id, buf) {
      if (!id || !buf || buf.composerDraft) return;
      const draft = evictedSessionDrafts[id];
      if (draft) buf.composerDraft = draft;
    }
function freshBuffer() { return pinvouSharedtauriSessions().freshBuffer(); }
function getBuffer(id) { return pinvouSharedtauriSessions().getBuffer(id); }
function isProtectedScheduledBuffer(id, buf) { return pinvouSharedtauriSessions().isProtectedScheduledBuffer(id, buf); }
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
function touchSessionBuffer(id, buf, scheduled) { return pinvouSharedtauriSessions().touchSessionBuffer(id, buf, scheduled); }
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
  // 回退/撤销在途切走后跳过重载（useRewindController 的归属检查）时，被跳过
  // 会话的缓存 buffer 停留在回退前快照，而 switchToSessionInternal 的快速路径
  // 会直接复用缓存（不读盘）——切回即展示陈旧时间线、后续发送续写在旧
  // buffer 上（评审 r8 M1）。本函数只丢弃该缓存（不做删除回调、不动暂存
  // 草稿），使下次激活落到 durable load；与 purgeSessionBuffer 的删除语义
  // 刻意不同。
  function invalidateSessionBuffer(id) {
    if (typeof id !== "string" || !id) return;
    // 评审 r9 minor 1：暂存未发送草稿再丢缓存（本文件既定原则——淘汰不得
    // 丢草稿）；保持与 purgeSessionBuffer 的差异：无删除回调、不动其它记账。
    // r39 minor 9：暂存失败（超过上限、不可安全保留的草稿）时拒绝失效——
    // 此前无条件丢缓存会连带丢掉草稿，违反 pruneSessionBuffers 的同一取舍
    // （keep the buffer）。代价：该会话的压缩重同步（r39 M1 失效臂）与回退
    // 跳过失效推迟到草稿清掉之后的下一次失效/切换，调和门维持诚实隐藏——
    // 「宁可入口暂缺、不可静默丢草稿」。r40 minor 7 界定覆盖范围：本拒绝
    // 只保护「失效丢缓存」路径；压缩前台臂随后的 forceDurableLoad 重载不走
    // 本函数的草稿暂存——草稿由重载自身的工作集保存/恢复编排保留（安全，
    // 与本拒绝无关），注释不得声称本拒绝覆盖该路径。
    if (sessionStates[id]) {
      if (typeof stashEvictedSessionDraft !== "function") return;
      if (!stashEvictedSessionDraft(id, sessionStates[id])) return;
    }
    delete sessionStates[id];
    // 活动会话的失效必须同时作用于 live 工作集（评审 r11 M2）：只删
    // sessionStates 条目对前台会话无效——切走时 saveWorkingSetTo 会把 live
    // 陈旧消息回存为复活条目，切回归快路径。清 live 投影并通知重渲染：
    // 活动场景下调用方（useRewindController 的 reloadError 路径）随后会走
    // 重试/自然激活的 durable load 重建时间线；清空画面优于陈旧画面。
    if (state.activeSessionId === id) {
      state.messages = [];
      state.chatItems = [];
      if (typeof notify === "function") notify();
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
function registerScheduledRunOwner(id, phase) { return pinvouSharedtauriSessions().registerScheduledRunOwner(id, phase); }
function scheduledRunOwnerVisibleRank(id) { return pinvouSharedtauriSessions().scheduledRunOwnerVisibleRank(id); }
function scheduledRunOwnerPriority(id) { return pinvouSharedtauriSessions().scheduledRunOwnerPriority(id); }
  function isProtectedScheduledRunOwner(id) {
    return scheduledRunOwnerPriority(id) > 1;
  }
  function pruneScheduledRunSessionOwner(id) {
    if (!scheduledRunSessionOwners[id] || isProtectedScheduledRunOwner(id)) return;
    delete scheduledRunSessionOwners[id];
  }
function pruneScheduledRunSessionOwners() { return pinvouSharedtauriSessions().pruneScheduledRunSessionOwners(); }
function isScheduledRunTerminal(status) { return pinvouSharedtauriSessions().isScheduledRunTerminal(status); }
function rememberScheduledRunOwner(run) { return pinvouSharedtauriSessions().rememberScheduledRunOwner(run); }
function scheduledRunBuffer(id) { return pinvouSharedtauriSessions().scheduledRunBuffer(id); }
function markScheduledInitialTurnActive(id) { return pinvouSharedtauriSessions().markScheduledInitialTurnActive(id); }
function markScheduledInitialTurnTerminal(id) { return pinvouSharedtauriSessions().markScheduledInitialTurnTerminal(id); }
function beginScheduledOpenActivation(id) { return pinvouSharedtauriSessions().beginScheduledOpenActivation(id); }
function rollbackScheduledOpenActivation(snapshot) { return pinvouSharedtauriSessions().rollbackScheduledOpenActivation(snapshot); }
  // FE14 残余批：steered sidecar 是桌面 lane 独有的工作集字段（web 无此概念）；
  // 共享 loadWorkingSetFrom/saveWorkingSetTo 经这两个可选钩子挂接，写入位置
  // 与合并前逐行一致（pinvouSceneEvents 之后、其余字段之前）。
  function restoreSteeredWorkingSet(buf) { state.steeredMessages = buf.steeredMessages || []; }
  function saveSteeredWorkingSet(buf) { buf.steeredMessages = state.steeredMessages; }
  function saveWorkingSetTo(buf) { return pinvouSharedtauriSessions().saveWorkingSetTo(buf); }
  function loadWorkingSetFrom(buf) { return pinvouSharedtauriSessions().loadWorkingSetFrom(buf); }
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
    buf.personaEvents = await loadPersonaEventsForSession(sid);
    buf.pinvouReviews = await loadPinvouReviewsForSession(sid);
    buf.pinvouSceneEvents = await syncPinvouSceneEventsForSession(sid);
    buf.steeredMessages = await syncSteeredMessagesForSession(sid);
    try { buf.turnTimeline = await invoke("get_session_timeline", { sessionId: sid }) || []; } catch { buf.turnTimeline = []; }
    // 手机可能在桌面仍停留草稿页/其他 session 时先唤醒这个后台 session。
    // 仅 hydrate messages 而把 chatItems 留空，会让后续 switchToSession 命中缓存快路径，
    // 不再 rerenderFromMessages，桌面便只看得到手机唤醒后的新内容，历史像是“丢了”。
    // 在首次磁盘 hydration 后先完整重建展示层，流式增量随后照常追加；
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
      // Keep the previously observed list on a transient refresh failure:
      // publishing an empty snapshot here would read as "every task deleted"
      // to the aux-chat purge reconcile (wireAuxSessionPurge), which would
      // then purge every task's unsent drafts and staged quotes. The archived
      // leg below keeps its previous list on failure for the same reason.
      state.sessions = state.sessions || [];
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
    // The staged explicit mode of a bound draft is registered intent too and is
    // invalidated together with the draft.
    state.pendingDraftMode = null;
    // A new draft resets to the default workspace: the previous draft's directory
    // choice must not carry over (shared by both early-return branches).
    state.draftWorkspacePath = null;
    state.draftWorkspaceRoots = [];
    state.draftProjectId = null;

    // 已在干净草稿态 → 只 notify(epoch 已自增)。注意要连 chatItems 一起判空:messages 与 chatItems
    // 会背离(persona 气泡 / ensureSession 失败的 system 报错卡只进 chatItems),否则残留卡顶掉「你好」。
    if (!state.activeSessionId && state.messages.length === 0 && state.chatItems.length === 0) {
      state.composerDraft = "";
      // Draft mode display = the current lane's global default (two-lane
      // semantics).
      state.modeState = currentDraftModeState();
      notify();
      return;
    }
    if (state.activeSessionId) saveWorkingSetTo(getBuffer(state.activeSessionId));
    state.activeSessionId = null;
    loadWorkingSetFrom(freshBuffer());
    // freshBuffer's modeState is the generic default (yolo); the draft
    // display must be overridden with this lane's global default (the work
    // lane's last_mode).
    state.modeState = currentDraftModeState();
    notify();
  }
  // 公开「新建对话」入口(侧边栏按钮)= 进草稿态。名字保留以兼容前端调用。
async function createNewSession() { return pinvouSharedtauriSessions().createNewSession(); }

  // ── Draft-state working directory selection (plain chat, mirroring the code
  // mode draft selector) ────────────────────────────────────────────────
  // The recents list shares key and semantics with src/shared/workspace-recents.js.
  // This file is a <script src> classic script and cannot import that ES module,
  // so the codec lives in the shared bridge payload
  // (src/shared/bridge-shared-helpers.js, loaded before both bridges); tests/
  // chat_draft_workspace_logic.test.mjs locks the bridge-side behavior,
  // tests/workspace_recents_logic.test.mjs the shared module.
  function rememberDraftWorkspaceRecent(path) { return pinvouSharedtauriSessions().rememberDraftWorkspaceRecent(path); }
  // Recents-list cleanup policy after create_session fails: prune only when the
  // backend explicitly rejected the path (invalid workspace_path — the directory
  // is stale or deleted); transient errors must not remove valid entries. Prune
  // by the boundWorkspace captured at materialization, not the live
  // draftWorkspacePath — if the user re-picks X2 while X1's creation is in
  // flight, X1 is the one that fails and the valid X2 must survive.
  function maybePruneFailedWorkspaceRecent(boundWorkspace, error) { return pinvouSharedtauriSessions().maybePruneFailedWorkspaceRecent(boundWorkspace, error); }
  // Effective in draft state only; path = null means back to the default
  // (session-private directory).
  // Returns whether the draft was staged: an active session owns the
  // composer's binding, so a call during one is a documented silent no-op.
  // The container reads this to keep the grant notice honest (a toast for a
  // staging that never happened would claim access the session does not
  // have; review #484 MINOR).
  function setDraftWorkspace(path, extras) {
    if (state.activeSessionId) return false;
    state.draftWorkspacePath = path || null;
    state.draftProjectId = (extras && extras.projectId) || null;
    state.draftWorkspaceRoots = (extras && Array.isArray(extras.workspaceRoots))
      ? extras.workspaceRoots
      : (path ? [String(path)] : []);
    // Binding/unbinding switches the draft mode display lane (bound → code lane,
    // unbound → back to this lane's default); on unbind, the previous bound
    // draft's staged explicit mode is invalidated and must not carry into an
    // unbound draft.
    if (!state.draftWorkspacePath) state.pendingDraftMode = null;
    state.modeState = currentDraftModeState();
    notify();
    return true;
  }
  // System directory picker: on success, records the path in the recents list,
  // writes it back to the draft selection, and returns the selected path;
  // returns null on user cancel (or dialog unavailable / not in draft state)
  // without changing the current selection.
  async function pickDraftWorkspace() {
    if (state.activeSessionId || !dialogOpen) return null;
    const selected = await pickDirectory({ directory: true, multiple: false, title: bt("pickFolderTitle") });
    const path = (selected && selected[0]) || null;
    if (!path) return null;
    rememberDraftWorkspaceRecent(path);
    setDraftWorkspace(path);
    return path;
  }

  // Tauri's rejection message for an unregistered command (compat detection for
  // older backends without the command); an unavailable invoke itself (web-side
  // stub) is treated the same way as "command missing".
  function isCommandMissingError(error) {
    const message = String((error && error.message) || error || "");
    return /unknown command|command not found|not implemented|invoke is unavailable/i.test(message);
  }

  // Working directory binding of a materialized session (plain-chat bound
  // workspace sessions; safety posture aligned with the code mode): returns the
  // full bound path, null when unbound, and null for older backends without the
  // command (UI shows no binding indicator). Other query failures (transient
  // errors) propagate to the caller — the YOLO confirmation gate relies on that
  // to fail closed with one extra confirmation rather than silently skipping a
  // bound session.
  async function getSessionWorkspaceBinding(sessionId) {
    if (!sessionId) return null;
    try {
      const binding = await invoke("get_session_workspace_binding", { sessionId });
      return typeof binding === "string" && binding ? binding : null;
    } catch (error) {
      if (isCommandMissingError(error)) return null;
      throw error;
    }
  }

  // 草稿态首次有实质内容时真正向后端创建 session 并切为 active;已有 active 直接返回。
  // 返回新 session id,创建失败返回 null。调用方:sendMessage(首条消息) / equipPersona(加卡)。
  // 并发防护（审计）：草稿态双击发送会并发 create_session，导致两条消息分家到两个新
  // 会话——in-flight 复用同一 promise；create_session await 期间用户切走会物化在错误
  // 会话（导航被劫持）——物化前校验 activeSessionId 仍为空，已切走则只登记后台 buffer。
  // Synchronous capture point for the materialization payload (workspace
  // binding + keychain snapshot + project ownership); a standalone small
  // function so ensureSession's decision complexity does not keep growing.
  function captureDraftWorkspaceBinding() {
    const boundWorkspace = state.draftWorkspacePath || null;
    const roots = state.draftWorkspaceRoots || [];
    return {
      boundWorkspace,
      // The keychain/project ownership is only passed with a bound draft (a
      // temporary draft is always null); the materialization-failure rollback
      // path restores the draft by value, so this returns the raw values, not
      // read-only copies.
      boundRoots: roots,
      boundProjectId: state.draftProjectId || null,
      payloadRoots: boundWorkspace && roots.length ? roots : null,
      payloadProjectId: boundWorkspace ? (state.draftProjectId || null) : null,
    };
  }
  let ensureSessionInFlight = null;
  let ensureSessionDraftOutcome = null;
  async function ensureSession(draftOwner) {
    function applyDraftOutcome(outcome) {
      if (!draftOwner || !outcome || draftOwner.draftEpoch !== outcome.epoch) return;
      draftOwner.createdSessionId = outcome.createdSessionId;
      if (outcome.rollbackEpoch !== undefined) {
        draftOwner.rollbackFromDraftEpoch = outcome.epoch;
        draftOwner.draftEpoch = outcome.rollbackEpoch;
      }
    }
    if (state.activeSessionId) return state.activeSessionId;
    if (ensureSessionInFlight) {
      const pendingOutcome = ensureSessionDraftOutcome;
      const pendingResult = await ensureSessionInFlight;
      if (draftOwner && pendingOutcome && draftOwner.draftEpoch === pendingOutcome.epoch) {
        applyDraftOutcome(pendingOutcome);
      }
      return pendingResult;
    }
    // 捕获导航 token：仅判 activeSessionId 覆盖不了「再进草稿」——enterDraft
    // 只推进 token 不改 activeSessionId（仍为 null），在途 create_session 返回
    // 后必须连同 token 一起校验，否则会劫持用户新进的草稿（三审 P1）。
    const navToken = sessionSwitchRequestToken;
    const draftOutcome = { epoch: Number(state.draftEpoch || 0), createdSessionId: null };
    ensureSessionDraftOutcome = draftOutcome;
    // Capture the materialization payload synchronously, outside the try: the
    // catch-side rollback and the recents cleanup both need the values bound by
    // THIS creation, and a try-scoped const would be out of scope there. Later
    // selections made during the await cannot affect this creation. The
    // post-materialization lane default application likewise keys off whether
    // this creation was bound.
    const { boundWorkspace, boundRoots, boundProjectId, payloadRoots, payloadProjectId } =
      captureDraftWorkspaceBinding();
    // eslint-disable-next-line sonarjs/cognitive-complexity -- single materialization state machine (navigation guards + multi-agent rollback + lane defaults); split tracked separately
    const p = (async function () {
      // 多 session 并发:不预热 engine。新建空 session 的 buffer 由 switchActiveTo({fresh}) 起。
      try {
        const meta = await invoke("create_session", {
          workspacePath: boundWorkspace,
          workspaceRoots: payloadRoots,
          projectId: payloadProjectId,
        });
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
          draftOutcome.createdSessionId = meta.id;
          return null;
        }
        // 草稿期开的多智能体开关此刻才落后端（开关本身不物化会话）。先取后
        // 清：switchActiveTo 会把寄存意图当作已消费。
        const pendingMultiAgent = state.pendingDraftMultiAgent === true;
        state.pendingDraftMultiAgent = false;
        // The bound draft's staged explicit mode is read-then-cleared the
        // same way (an explicit switch during the await is user intent too).
        const stagedDraftMode = state.pendingDraftMode;
        state.pendingDraftMode = null;
        // Materialization committed: clear the draft directory; on
        // create_session failure (outer catch) it is kept for retry.
        state.draftWorkspacePath = null;
        state.draftWorkspaceRoots = [];
        state.draftProjectId = null;
        switchActiveTo(meta.id, { fresh: true });
        draftOutcome.createdSessionId = meta.id;
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
            draftOutcome.createdSessionId = null;
            // A real navigation during the toggle/cleanup awaits is not a
            // rollback: never let it masquerade as the same draft.
            if (navToken !== sessionSwitchRequestToken || state.activeSessionId !== meta.id) return null;
            const rollbackComposerDraft = state.composerDraft || "";
            enterDraft();
            state.composerDraft = rollbackComposerDraft;
            // This rollback is still the same logical draft, not user
            // navigation: record the epoch transition for the voice retry
            // association (restoreTaskDraft rebinds via rollbackFromDraftEpoch).
            draftOutcome.rollbackEpoch = Number(state.draftEpoch || 0);
            // Restore the registered intent captured before the failure: the
            // retry must keep the user's explicit toggle.
            state.pendingDraftMultiAgent = true;
            state.draftWorkspacePath = boundWorkspace;
            state.draftWorkspaceRoots = boundRoots;
            state.draftProjectId = boundProjectId;
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
          // Bound-workspace sessions share the code mode's safety posture: the
          // backend already resolves mode for bound sessions via the code lane
          // global default, so do not apply the work lane default through
          // set_plan_mode_next here; apply only a mode the user explicitly
          // staged in draft state (the one-shot YOLO confirmation gate was
          // already passed by ChatView at draft switch time).
          if (stagedDraftMode === "plan" || stagedDraftMode === "yolo") {
            // Use the meta.id captured at materialization, not activeSessionId:
            // during the awaits above the user may have switched away, and
            // running the mode command against the current active session would
            // target the wrong one.
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
        // Two-lane semantics (#428 merged design into work): the backend's
        // plain default is always Yolo and lanes are only work/code; when the
        // materializing session's lane global default is plan, apply it right
        // now (the write becomes that session's own per-session record; the
        // global default is unaffected).
        const laneDefault = state.modeDefaults
          && state.modeDefaults[state.modeLane === "code" ? "code" : "work"];
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
        // Recents-list cleanup is centralized in maybePruneFailedWorkspaceRecent
        // (prune only when the path itself was rejected, keyed by the captured
        // bound directory). The draft selection itself is kept per the existing
        // contract so the user can fix the directory and retry.
        maybePruneFailedWorkspaceRecent(boundWorkspace, e);
        return null;
      }
    })();
    ensureSessionInFlight = p;
    try {
      const result = await p;
      applyDraftOutcome(draftOutcome);
      return result;
    } finally {
      // Reset on every path: a lingering in-flight entry would make every
      // future draft send await a dead promise forever.
      if (ensureSessionInFlight === p) ensureSessionInFlight = null;
    }
  }

function reportSessionSwitchFailure(error, errorScope) { return pinvouSharedtauriSessions().reportSessionSwitchFailure(error, errorScope); }

function hydratedMessageKey(message, hideInternalEnvelope) { return pinvouSharedtauriSessions().hydratedMessageKey(message, hideInternalEnvelope); }

function mergeHydratedMessages(durableMessages, liveMessages, hideInternalEnvelope) { return pinvouSharedtauriSessions().mergeHydratedMessages(durableMessages, liveMessages, hideInternalEnvelope); }

function mergeHydratedArtifacts(durableArtifacts, liveArtifacts) { return pinvouSharedtauriSessions().mergeHydratedArtifacts(durableArtifacts, liveArtifacts); }

function hydratedChatItemKey(item) { return pinvouSharedtauriSessions().hydratedChatItemKey(item); }

  function mergeHydratedChatItems(liveChatItems, liveCurrentStreamId) {let pinvouSharedtauriSessionsN47342Cache = null;
function pinvouSharedtauriSessionsN47342() {
  if (!pinvouSharedtauriSessionsN47342Cache) pinvouSharedtauriSessionsN47342Cache = window.PinvouBridgeShared.create("tauriSessions:47342", { state });
  return pinvouSharedtauriSessionsN47342Cache;
}


    let remappedCurrentStreamId = 0;
    const availableByKey = Object.create(null);
function interruptedDisplayRange(item) { return pinvouSharedtauriSessionsN47342().interruptedDisplayRange(item); }
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

    const pinvouSceneEvents = await syncPinvouSceneEventsForSession(id);
    const steeredMessages = await syncSteeredMessagesForSession(id);
    const personaEvents = await loadPersonaEventsForSession(id);
    const pinvouReviews = await loadPinvouReviewsForSession(id);
    let turnTimeline = [];
    try { turnTimeline = await invoke("get_session_timeline", { sessionId: id }) || []; } catch { /* optional data; default to empty */ }
    if (requestToken !== sessionSwitchRequestToken) return false;

    // load_session 与必要的直接会话数据均成功后，才一次性提交 active/context。
    // r14 B1：在保存/切换前捕获「本次切换目标是否为活动会话」——durable 分支
    // 的草稿携带只在 wasActive 时合法（回退/撤销重载的目标就是活动会话）。
    // 跨会话冷切换（switchToSession(B) / scheduled run 打开非活动会话）时
    // state.composerDraft 是切出会话 A 的草稿，不带门控的携带会把 A 的草稿
    // 注入 B 的输入框（r13-M2 修复的回归形状）。
    const wasActive = state.activeSessionId === id;
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
      // r13 M2 → r14 B1：durable 分支不得丢未发送草稿——但携带仅限 wasActive
      // （回退/撤销的 forceDurableLoad 目标就是活动会话，state.composerDraft
      // 是用户「打了一半先回退」的最新键击）。跨会话冷切换时切出会话的草稿
      // 不得注入目标会话的 fresh buffer（那正是 r13 修复在共享路径上引入的反向
      // 回归）；目标会话自己的驱逐暂存草稿由下方 restoreEvictedSessionDraft 恢复。
      const liveDraft = wasActive ? (state.composerDraft || "") : "";
      sessionStates[id] = freshBuffer();
      if (liveDraft) sessionStates[id].composerDraft = liveDraft;
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

async function switchToSession(id) { return pinvouSharedtauriSessions().switchToSession(id); }

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

function openScheduledRunChat(run, task) { return pinvouSharedtauriSessions().openScheduledRunChat(run, task); }

async function exitScheduledRunChat() { return pinvouSharedtauriSessions().exitScheduledRunChat(); }

function recentScheduledRunForSession(id) { return pinvouSharedtauriSessions().recentScheduledRunForSession(id); }

  // 离开正在查看的会话:清 active + 换空工作集,并清掉指向它的定时运行上下文。
  // 必须连 scheduledRunContext 一起清 —— main.jsx 只按该字段真值决定渲染
  // ChatView 还是 ScheduledTasksView,而 ChatView 内部还要求 sessionId===activeSessionId
  // 才渲染返回按钮;只清 active 会卡在「定时路由下的空白页且没有返回按钮」。
  // 清掉之后 currentView 仍是 'scheduled',界面自然落回定时任务列表。
  // 不负责 buffer:删除要丢弃 buffer,收纳要保留 buffer,由调用方各自处理。
function leaveSessionView(id) { return pinvouSharedtauriSessions().leaveSessionView(id); }

function applyDeletedSession(id) { return pinvouSharedtauriSessions().applyDeletedSession(id); }

function applyWorkspaceReboundMark(payload) { return pinvouSharedtauriSessions().applyWorkspaceReboundMark(payload); }

function onSessionDeleted(listener) { return pinvouSharedtauriSessions().onSessionDeleted(listener); }

  if (typeof listen === "function") {
    listen("session:deleted", function (event) {
      const payload = event && event.payload || {};
      applyDeletedSession(payload.id);
    }).catch(function (error) {
      console.error("[sessions] session:deleted listener failed", error);
    });
    // Burst coalescing (review #463 round-20 R3): the rebind command emits one
    // session:list_changed per affected id, and each undebounced refresh costs
    // an IPC round trip + an O(n) backend list_sessions + a React commit — a
    // large rebind burst translated into hundreds of full history refreshes
    // exactly while the report dialog opens. Leading + trailing: the first
    // event refreshes immediately (single-event semantics unchanged), events
    // inside the window set a pending flag collapsed into ONE trailing
    // refresh. Harnesses without timers fall back to the immediate refresh.
    function scheduleHistoryRefresh() { return pinvouSharedtauriSessions().scheduleHistoryRefresh(); }
    listen("session:list_changed", function (event) {
      const payload = event && event.payload || {};
      // The rebind command stamps the sessions whose persisted artifact paths
      // its lanes rebased (rebound, failed AND post-busy ids — review #463
      // round-10 Major 2 + round-B Major 1). The mark carries the rebind
      // geometry as an ordered segment chain consumed by
      // bridge/artifact-tracker.js (rebaseArtifactPathsForRebind /
      // sessionRecentlyRebound); the stamp/append/refresh semantics live in
      // the shared applyWorkspaceReboundMark (round-13 — previously
      // byte-duplicated with the web lane).
      applyWorkspaceReboundMark(payload);
      scheduleHistoryRefresh();
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

async function renameSession(id, title) { return pinvouSharedtauriSessions().renameSession(id, title); }

async function toggleSessionPinned(id, pinned) { return pinvouSharedtauriSessions().toggleSessionPinned(id, pinned); }

  // One-click full session log export: the backend opens the native save
  // dialog and packs the full-fidelity session record (system prompt, all
  // turns, tool calls and results) together with artifacts into a .tar.xz
  // archive. Resolves to { path, ... }; resolves to null when the user
  // cancels. The web build has no local session files, so the bridge does
  // not expose this entry point (the frontend hides the menu item based on
  // capabilities).
  async function exportSessionArchive(id, defaultName) {
    return invoke("export_session", { id, defaultName, includeArtifacts: true });
  }

async function archiveSession(id) { return pinvouSharedtauriSessions().archiveSession(id); }

async function restoreArchivedSession(id) { return pinvouSharedtauriSessions().restoreArchivedSession(id); }

  // 实时态有专属气泡的工具（方案卡），重建时要还原成原卡而非普通工具卡。

    return {
      freshBuffer,
      getBuffer,
      isProtectedScheduledBuffer,
      pruneScheduledSessionBuffers,
      touchSessionBuffer,
      // r9 B1：必须随返回对象导出——r8 只加了函数定义与共享依赖表、漏了此处，
      // bridge.js 拿到 undefined、消费端 typeof 守卫静默吞掉整个修复（协议测试
      // 的 Proxy 桩对 undefined 值键不敏感，只有执行真工厂才能锚定）。
      invalidateSessionBuffer,
      purgeSessionBuffer,
      registerScheduledRunOwner,
      scheduledRunOwnerVisibleRank,
      scheduledRunOwnerPriority,
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
      rememberDraftWorkspaceRecent,
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
      onSessionDeleted,
      deleteSession,
      renameSession,
      toggleSessionPinned,
      archiveSession,
      restoreArchivedSession,
      exportSessionArchive
    };
  };
})(window);
