/**
 * aux-chat feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 *
 * "Aux chat" bridge: each task (taskId) gets one background aux chat session
 * (id derived as aux-<taskId>). Aux sessions are
 * filtered out of list_sessions
 * by the backend (they never enter state.sessions), so the chat domain's
 * sendMessageToSession (which requires sid ∈ state.sessions) cannot be reused.
 * This domain is a thin wrapper: session creation/loading reuses the sessions
 * domain's per-session buffer path, and turn events are still routed into the
 * buffer by chat-events keyed on session_id (this domain adds no event listeners).
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting statements would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry.auxChat = function (context) {
    const state = context.state;
    const invoke = context.invoke;
    const bt = context.bt;
    const sessionStates = context.sessionStates;
    const ensureSessionBufferLoaded = context.ensureSessionBufferLoaded;
    const purgeSessionBuffer = context.purgeSessionBuffer || function () {};
    const touchSessionBuffer = context.touchSessionBuffer || function () {};
    const isBusyFor = context.isBusyFor;

    const AUX_SESSION_ID_PATTERN = /^aux-/;

    function isAuxSession(id) {
      return typeof id === "string" && AUX_SESSION_ID_PATTERN.test(id);
    }

    function emptySnapshot() {
      return { chatItems: [], busy: false, queued: [] };
    }

    async function ensure(taskId) {
      const task = String(taskId || "").trim();
      if (!task) throw new Error(bt("targetSessionMissing"));
      const metadata = await invoke("get_or_create_aux_session", { sessionId: task });
      const auxId = metadata && typeof metadata.id === "string" ? metadata.id : "";
      if (!isAuxSession(auxId)) throw new Error(bt("sessionDataInvalid"));
      // Aux sessions never become active: they use the background-buffer
      // load_session(setActive:false) path.
      await ensureSessionBufferLoaded(auxId);
      return auxId;
    }

    async function send(auxId, text) {
      const sid = String(auxId || "").trim();
      const message = String(text || "").trim();
      if (!isAuxSession(sid)) throw new Error(bt("targetSessionMissing"));
      if (!message) throw new Error(bt("replyContentEmpty"));
      await ensureSessionBufferLoaded(sid);
      const buf = sessionStates[sid];
      // Aux sessions never queue (queue is user-input semantics): reject
      // outright when busy or queued messages exist; the caller retries.
      if (isBusyFor(sid) || (buf && Array.isArray(buf.queued) && buf.queued.length > 0)) {
        throw new Error(bt("turnAlreadyInProgress"));
      }
      return invoke("chat", { message, attachments: [], sessionId: sid, restrictTools: true });
    }

    // Synchronous snapshot: when not loaded (no buffer) returns an empty
    // structure — never throws and never triggers a load. Items are shallow-
    // copied one by one: streaming deltas mutate buffer items in place (the
    // item.text/html assignments in chat-events), so copying only the array
    // would share object references and a caller comparing field by field
    // could not detect changes; after copying, every poll gets fresh
    // references, making field comparison a true content comparison.
    function snapshotItems(items) {
      return (Array.isArray(items) ? items : []).map(function (item) {
        return item && typeof item === "object" ? Object.assign({}, item) : item;
      });
    }
    function snapshot(auxId) {
      const sid = String(auxId || "").trim();
      if (!sid) return emptySnapshot();
      if (sid === state.activeSessionId) {
        return {
          chatItems: snapshotItems(state.chatItems),
          busy: !!state.busy,
          queued: snapshotItems(state.queued),
        };
      }
      const buf = sessionStates[sid];
      if (!buf) return emptySnapshot();
      // An always-open panel polling snapshot() counts as "reading": refresh
      // LRU recency consistently with the getBuffer read paths, otherwise
      // after 32+ session switches the buffer is evicted by capacity and the
      // panel wrongly shows the empty state.
      touchSessionBuffer(sid, buf, false);
      return {
        chatItems: snapshotItems(buf.chatItems),
        busy: !!buf.busy,
        queued: snapshotItems(buf.queued),
      };
    }

    async function discard(taskId) {
      const task = String(taskId || "").trim();
      if (!task) throw new Error(bt("targetSessionMissing"));
      await invoke("discard_aux_session", { sessionId: task });
      // The aux id is a pure function of the task id (aux-<taskId>, round-30
      // B8), so the buffer purge derives it — the old per-task id map was
      // redundant state that was never pruned (M5). The session:deleted event
      // fired by backend deletion is handled by the sessions domain as a
      // fallback — belt and suspenders, and idempotent.
      purgeSessionBuffer(`aux-${task}`);
    }

    // Atomic restart (M6): one backend command discards the old aux session
    // (gated against its turns) and creates the fresh one — no two-invoke
    // window for an orphaned discard to land in. The aux id is derived
    // (aux-<taskId>, round-30 B8), so the stale buffer purge needs no
    // per-task id map; the backend's session:deleted also purges through
    // the sessions domain as a fallback.
    async function reset(taskId) {
      const task = String(taskId || "").trim();
      if (!task) throw new Error(bt("targetSessionMissing"));
      const metadata = await invoke("reset_aux_session", { sessionId: task });
      const auxId = metadata && typeof metadata.id === "string" ? metadata.id : "";
      if (!isAuxSession(auxId)) throw new Error(bt("sessionDataInvalid"));
      purgeSessionBuffer(auxId);
      // Aux sessions never become active: they use the background-buffer
      // load_session(setActive:false) path.
      await ensureSessionBufferLoaded(auxId);
      return auxId;
    }

    // isAuxSession stays domain-private: both facades only validate their own
    // ensure/send inputs with it, and no feature ever needed it across the
    // bridge — the dead public export (pinned by the domain contract) is gone
    // instead of being maintained on both sides forever.
    return { ensure, send, snapshot, discard, reset };
  };
})(typeof window === "undefined" ? globalThis : window);
