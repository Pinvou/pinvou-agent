(function () {
  "use strict";

  var shellCleanupFailed = {
    zh: "⚠️ 部分后台任务未能停止，可在后台任务列表中逐个停止。",
    en: "⚠️ Some background tasks could not be stopped. You can stop them individually from the background task list.",
    ja: "⚠️ 一部のバックグラウンドタスクを停止できませんでした。バックグラウンドタスク一覧から個別に停止できます。",
  };

  window.PinvouBridgeMessages = Object.freeze({
    shellCleanupFailed: function (language) {
      return shellCleanupFailed[language] || shellCleanupFailed.zh;
    },
    showShellCleanupFailure: function (payload, state, addSystemItem) {
      if (!payload || !payload.shell_cleanup_failed) return;
      var notice = shellCleanupFailed[state.settings && state.settings.language]
        || shellCleanupFailed.zh;
      var existing = state.chatItems.find(function (item) {
        return item && item.turnErrorNotice && item.text === notice;
      });
      if (existing) {
        existing.legacyConversationOnly = true;
      } else {
        addSystemItem(notice, {
          turnErrorNotice: true,
          legacyConversationOnly: true,
        });
      }
    },
  });
})();
