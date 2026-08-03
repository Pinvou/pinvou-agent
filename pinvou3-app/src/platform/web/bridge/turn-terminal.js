(function () {
  "use strict";

  window.PinvouWebTurnTerminal = Object.freeze({
    recordCompleted: function (state, openStart, payload) {
      var turnId = state.activeTurnTimelineId || (openStart && openStart.turn_id);
      if (!turnId) return;
      var timestamp = Date.now();
      var start = openStart || (state.turnTimeline || []).find(function (event) {
        return event && event.turn_id === turnId && event.event === "user_start";
      });
      state.turnTimeline = (state.turnTimeline || []).concat([{
        turn_id: turnId,
        event: "assistant_done",
        timestamp: timestamp,
        ts: new Date(timestamp).toISOString(),
        status: payload && payload.status || (payload && payload.error ? "Failed" : "Completed"),
        error: payload && payload.error || null,
        ui_turn_index: start && start.ui_turn_index,
      }]);
      state.activeTurnTimelineId = null;
    },
  });
})();
