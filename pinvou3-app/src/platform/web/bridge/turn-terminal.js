(function () {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic script; strict mode is the payload
  "use strict";

  window.PinvouWebTurnTerminal = Object.freeze({
    recordCompleted: function (state, openStart, payload) {
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
    },
  });
})();
