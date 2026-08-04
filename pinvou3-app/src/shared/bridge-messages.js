(function () {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic script; strict mode is the payload
  "use strict";

  const shellCleanupFailed = {
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

  // Runtime-owned user-role turns may arrive with their trailing turn metadata
  // flattened into the same text block. Keep that transport detail out of UI projections.
  function inputProvenanceFromText(value) {
    const text = String(value || "").trim();
    if (!text) return "";
    const lowerText = text.toLowerCase();
    const openingTag = "<turn_meta>";
    const closingTag = "</turn_meta>";
    const openingIndex = lowerText.lastIndexOf(openingTag);
    const closingIndex = lowerText.indexOf(closingTag, openingIndex + openingTag.length);
    const hasTrailingMetadata = openingIndex > 0 && text[openingIndex - 1] === "\n" &&
      closingIndex >= 0 && !text.slice(closingIndex + closingTag.length).trim();
    const metadata = openingIndex === 0
      ? text
      : (hasTrailingMetadata ? text.slice(openingIndex, closingIndex + closingTag.length) : "");
    if (!metadata) return "";
    const match = metadata.match(/(?:^|\n)Input provenance:\s*([a-z0-9_-]+)/i);
    return match && match[1] ? match[1].toLowerCase() : "";
  }

  function isInternalUserMessageProvenance(provenance) {
    return ["runtime", "subagent_handoff", "shell_completion"].includes(provenance);
  }

  function consumeLeadingInternalRuntimeEnvelope(value) {
    const text = String(value || "").trim();
    const opening = text.match(/^<codewhale:runtime_event\b[^>]*\bvisibility=(["'])internal\1[^>]*>/i);
    if (!opening) return null;
    const remainder = text.slice(opening[0].length);
    const closing = remainder.match(/<\/codewhale:runtime_event\s*>/i);
    if (!closing) return null;
    return remainder.slice(closing.index + closing[0].length).trim();
  }

  function containsOnlyInternalRuntimeMetadata(value) {
    let remainder = String(value || "").trim();
    let foundEnvelope = false;
    while (remainder) {
      const afterEnvelope = consumeLeadingInternalRuntimeEnvelope(remainder);
      if (afterEnvelope === null) break;
      foundEnvelope = true;
      remainder = afterEnvelope;
    }
    if (!foundEnvelope) return false;
    if (!remainder || /^<turn_meta_unchanged\s*\/>$/i.test(remainder)) return true;
    const lowerRemainder = remainder.toLowerCase();
    return lowerRemainder.startsWith("<turn_meta>") && lowerRemainder.endsWith("</turn_meta>");
  }

  function userMessageInputProvenance(blocks) {
    const textBlocks = Array.isArray(blocks) ? blocks : [];
    for (let i = 0; i < textBlocks.length; i++) {
      const block = textBlocks[i];
      if (!block || block.type !== "text") continue;
      const provenance = inputProvenanceFromText(block.text);
      if (provenance) return provenance;
    }
    return "";
  }

  function isInternalRuntimeUserMessage(value) {
    return containsOnlyInternalRuntimeMetadata(value) ||
      isInternalUserMessageProvenance(inputProvenanceFromText(value));
  }

  window.PinvouBridgeMessages = Object.freeze({
    isInternalRuntimeUserMessage,
    isInternalUserMessageProvenance,
    userMessageInputProvenance,
    modelServiceUserError: function (payload, state) {
      payload = payload || {};
      state = state || {};
      var raw = payload && (payload.user_error || payload.userError);
      var error = payload && payload.error;
      if (!raw && !error) return null;
      var helper = window.PinvouModelServiceErrors;
      if (!helper || typeof helper.build !== "function") return null;
      return helper.build(raw || error, {
        language: state.settings && state.settings.language,
        providerLabel: helper.providerLabelFromState(state),
      });
    },

    addModelServiceErrorNotice: function (payload, state, addSystemItem, legacyConversationOnly) {
      payload = payload || {};
      state = state || {};
      var helper = window.PinvouModelServiceErrors;
      var userError = window.PinvouBridgeMessages.modelServiceUserError(payload, state);
      if (!helper || !userError) return false;
      var notice = helper.noticeText(userError);
      var chatItems = Array.isArray(state.chatItems) ? state.chatItems : [];
      var existing = chatItems.find(function (item) {
        return item && item.turnErrorNotice && (
          item.text === notice ||
          (item.userError && item.userError.technicalDetail === userError.technicalDetail)
        );
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
      const notice = shellCleanupFailedText(state.settings && state.settings.language);
      const existing = state.chatItems.find(function (item) {
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
