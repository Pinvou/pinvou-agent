(function () {
  "use strict";

  var shellCleanupFailed = {
    zh: "⚠️ 部分后台任务未能停止，可在后台任务列表中逐个停止。",
    en: "⚠️ Some background tasks could not be stopped. You can stop them individually from the background task list.",
    ja: "⚠️ 一部のバックグラウンドタスクを停止できませんでした。バックグラウンドタスク一覧から個別に停止できます。",
  };

  // 与 bridge 层 bt() 约定一致:settings.language 存的是 tag(zh-Hans/en/ja),
  // 非 en/ja 一律回退中文。
  function shellCleanupFailedText(language) {
    return language === "en" ? shellCleanupFailed.en
      : language === "ja" ? shellCleanupFailed.ja
      : shellCleanupFailed.zh;
  }

  window.PinvouBridgeMessages = Object.freeze({
    modelServiceUserError: function (payload, state) {
      payload = payload || {};
      state = state || {};
      var raw = payload && (payload.user_error || payload.userError);
      var error = payload && payload.error;
      if (!raw && !error) return null;
      var helper = window.PinvouModelServiceErrors;
      if (!helper || typeof helper.build !== "function") return null;
      if (!raw && typeof helper.isModelServiceError === "function" && !helper.isModelServiceError(error)) {
        return null;
      }
      var language = state.settings && state.settings.language;
      return helper.build(raw || error, {
        language: language,
        providerLabel: helper.providerLabelFromState(state, null, language),
        terminal: payload.terminal !== false,
      });
    },

    addModelServiceErrorNotice: function (payload, state, addSystemItem, legacyConversationOnly) {
      payload = payload || {};
      state = state || {};
      var helper = window.PinvouModelServiceErrors;
      payload.terminal = !!legacyConversationOnly;
      var userError = window.PinvouBridgeMessages.modelServiceUserError(payload, state);
      if (!helper || !userError) return false;
      var notice = helper.noticeText(userError);
      var chatItems = Array.isArray(state.chatItems) ? state.chatItems : [];
      var existing = chatItems.find(function (item) {
        if (!item || !item.turnErrorNotice || item.text !== notice) return false;
        var existingDetail = item.userError && item.userError.technicalDetail;
        var nextDetail = userError && userError.technicalDetail;
        if (existingDetail || nextDetail) return existingDetail === nextDetail;
        return true;
      });
      if (existing) {
        if (legacyConversationOnly) existing.legacyConversationOnly = true;
        existing.userError = userError;
      } else {
        addSystemItem(notice, {
          turnErrorNotice: true,
          legacyConversationOnly: !!legacyConversationOnly,
          userError: userError,
        });
      }
      payload.user_error = userError;
      payload.userError = userError;
      return true;
    },

    showShellCleanupFailure: function (payload, state, addSystemItem) {
      if (!payload || !payload.shell_cleanup_failed) return;
      var notice = shellCleanupFailedText(state.settings && state.settings.language);
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
