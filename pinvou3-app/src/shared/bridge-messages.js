(function () {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic script; strict mode is the payload
  "use strict";

  const shellCleanupFailed = {
    zh: "⚠️ 部分后台任务未能停止，可在后台任务列表中逐个停止。",
    en: "⚠️ Some background tasks could not be stopped. You can stop them individually from the background task list.",
  };

  // 与 bridge 层 bt() 约定一致:settings.language 存的是 tag(zh-Hans/en),
  // 非 en 一律回退中文。
  function shellCleanupFailedText(language) {
    return language === "en" ? shellCleanupFailed.en : shellCleanupFailed.zh;
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
    // Error texts the gate missed (gateway/proxy custom bodies, raw
    // provider messages) do not become model-service notices but still get
    // displayed as bare strings / red text. Redact unconditionally in
    // front of every display surface: classification may miss, credentials
    // must not. Returned as-is when the helper is missing (falls back to
    // the existing behavior).
    redactRawError: function (error, state) {
      const text = String(error == null ? "" : error);
      if (!text) return text;
      const helper = window.PinvouModelServiceErrors;
      if (!helper || typeof helper.redactTechnicalDetail !== "function") return text;
      return helper.redactTechnicalDetail(text, state && state.settings && state.settings.language);
    },

    modelServiceUserError: function (payload, state) {
      payload = payload || {};
      state = state || {};
      const raw = payload && (payload.user_error || payload.userError);
      const error = payload && payload.error;
      if (!raw && !error) return null;
      const helper = window.PinvouModelServiceErrors;
      if (!helper || typeof helper.build !== "function") return null;
      if (!raw && typeof helper.isModelServiceError === "function" && !helper.isModelServiceError(error)) {
        return null;
      }
      const language = state.settings && state.settings.language;
      const userError = helper.build(raw || error, {
        language,
        providerLabel: helper.providerLabelFromState(state, language),
        terminal: payload.terminal !== false,
      });
      // The structured base code/category forwarded by the forwarder is
      // kept on the user card: the streaming path currently emits mostly
      // the generic "transient", so this is controlled semantics retained
      // for diagnostics only and does not participate in gating or
      // classification.
      if (typeof (payload && payload.code) === "string" && payload.code) userError.errorCode = payload.code;
      if (typeof (payload && payload.category) === "string" && payload.category) userError.errorCategory = payload.category;
      return userError;
    },

    addModelServiceErrorNotice: function (payload, state, addSystemItem, legacyConversationOnly, terminalRecord) {
      payload = payload || {};
      state = state || {};
      const helper = window.PinvouModelServiceErrors;
      payload.terminal = !!legacyConversationOnly;
      const userError = window.PinvouBridgeMessages.modelServiceUserError(payload, state);
      if (!helper || !userError) return false;
      const notice = helper.noticeText(userError);
      const chatItems = Array.isArray(state.chatItems) ? state.chatItems : [];
      const nextDetail = userError && userError.technicalDetail;
      // Dedup keys on the error identity (kind + technical detail), not
      // the final wording: within one turn a transient notice (recoverable
      // wording) followed by done (terminal wording) always differs in
      // text, so exact-text dedup would miss and produce two contradictory
      // bubbles for one error.
      const existing = chatItems.find(function (item) {
        if (!item || !item.turnErrorNotice || !item.userError) return false;
        if (item.userError.kind !== userError.kind) return false;
        const existingDetail = item.userError.technicalDetail;
        if (existingDetail || nextDetail) return existingDetail === nextDetail;
        return true;
      });
      // Hiding the bubble on terminal requires a timeline terminal record
      // that actually exists and carries an error (the timeline card takes
      // over). recordTurnCompleted writes no record when openStart/turnId
      // is missing; hiding anyway would make the error fully invisible
      // (silent-swallow regression).
      const timelineTakesOver = !!legacyConversationOnly
        && !!(terminalRecord && terminalRecord.error);
      let target = existing || null;
      if (target) {
        target.text = notice;
        target.userError = userError;
        if (timelineTakesOver) target.legacyConversationOnly = true;
      } else {
        target = { turnErrorNotice: true, legacyConversationOnly: timelineTakesOver, userError };
        addSystemItem(notice, target);
      }
      // On terminal takeover, hide the turn's other model-service transient
      // bubbles too (of a different identity, e.g. a network idle timeout
      // followed by a billing done): their "will keep retrying" wording
      // contradicts the terminal "has stopped". The bridge clears
      // turnErrorNotice items on every send, so everything still present
      // belongs to the current turn and the previous turn is untouched.
      if (timelineTakesOver) {
        chatItems.forEach(function (item) {
          if (item && item !== target && item.turnErrorNotice && item.userError && !item.legacyConversationOnly) {
            item.legacyConversationOnly = true;
          }
        });
      }
      payload.user_error = userError;
      payload.userError = userError;
      return true;
    },
    // When a successful chat:done (no error) arrives, the turn's transient
    // model-service bubbles ("will keep retrying") are stale: the turn has
    // recovered, and an outdated claim must not outlive the recovery it
    // described. Only items carrying a userError are hidden (their wording
    // promises future retries); bare-string fallbacks are statements about
    // errors that did happen and keep the existing behavior. Terminal
    // errors do not go through this path — addModelServiceErrorNotice
    // already upgrades/hides them by identity.
    settleModelServiceErrorNotices: function (state) {
      state = state || {};
      const chatItems = Array.isArray(state.chatItems) ? state.chatItems : [];
      let settled = false;
      chatItems.forEach(function (item) {
        if (item && item.turnErrorNotice && item.userError && !item.legacyConversationOnly) {
          item.legacyConversationOnly = true;
          settled = true;
        }
      });
      return settled;
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
