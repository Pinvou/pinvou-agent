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
 *
 * The domain bodies live in the shared lane (cluster "auxChat" in
 * src/shared/bridge-shared-helpers.js, round-31 M7); this lane keeps only its
 * genuine difference — the send dispatch (desktop goes through chat with an
 * empty attachments list; Web goes through web_access_chat with attachment
 * handles). The web-only session_turn_in_progress translation wrapper was
 * dropped with the move (M7): send errors only reach console.warn and the
 * panel's static localized sendFailed copy on either lane, so the translated
 * text never reached the user.
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting statements would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry.auxChat = function (context) {
    // Lane-specific send dispatch (M7); every other line of the domain is the
    // shared cluster's.
    function auxChatDispatch(sid, message) {
      return context.invoke("chat", { message, attachments: [], sessionId: sid, restrictTools: true });
    }
    const shared = window.PinvouBridgeShared.create("auxChat", {
      state: context.state,
      invoke: context.invoke,
      bt: context.bt,
      sessionStates: context.sessionStates,
      ensureSessionBufferLoaded: context.ensureSessionBufferLoaded,
      purgeSessionBuffer: context.purgeSessionBuffer || function () {},
      touchSessionBuffer: context.touchSessionBuffer || function () {},
      isBusyFor: context.isBusyFor,
      auxChatDispatch,
    });

    // isAuxSession stays domain-private: both facades only validate their own
    // ensure/send inputs with it, and no feature ever needed it across the
    // bridge — the dead public export (pinned by the domain contract) is gone
    // instead of being maintained on both sides forever.
    return {
      ensure: shared.auxChatEnsure,
      send: shared.auxChatSend,
      snapshot: shared.auxChatSnapshot,
      discard: shared.auxChatDiscard,
      reset: shared.auxChatReset,
    };
  };
})(typeof window === "undefined" ? globalThis : window);
