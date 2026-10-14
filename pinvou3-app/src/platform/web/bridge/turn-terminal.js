(function () {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic script; strict mode is the payload
  "use strict";

  // F-FE13-2：assistant_done 时间线记录构建器统一在
  // shared/bridge-shared-helpers.js（desktop chat-events.js 同源转发）；
  // 本模块只保留 web lane 的 recordCompleted 调用面。
  window.PinvouWebTurnTerminal = Object.freeze({
    recordCompleted: function (state, openStart, payload) {
      return window.PinvouBridgeShared.buildAssistantDoneTimelineRecord(state, openStart, payload);
    },
  });
})();
