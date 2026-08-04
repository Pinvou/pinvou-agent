(function () {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic script; strict mode is the payload
  "use strict";

  window.PinvouWebTurnTerminal = Object.freeze({
    recordCompleted: function (state, openStart, payload) {
      const turnId = state.activeTurnTimelineId || (openStart && openStart.turn_id);
      if (!turnId) return;
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
      state.turnTimeline = [...(state.turnTimeline || []), {
        turn_id: turnId,
        event: "assistant_done",
        timestamp,
        ts: new Date(timestamp).toISOString(),
        status: payload && payload.status || (payload && payload.error ? "Failed" : "Completed"),
        error: payload && payload.error || null,
        user_error: payload && (payload.user_error || payload.userError) || null,
        ui_turn_index: start && start.ui_turn_index,
      }];
      state.activeTurnTimelineId = null;
    },
  });
})();
