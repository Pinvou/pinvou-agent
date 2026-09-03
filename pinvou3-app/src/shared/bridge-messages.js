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
    // 门控漏判的错误文本(网关/代理自定义 body、provider 原始报文)不走
    // model-service notice,但还会以裸串/红字展示。展示面前无条件脱敏:
    // 分类允许漏判,凭证不允许漏。helper 缺失时原样返回(降级为既有行为)。
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
      // forwarder 透传的底座结构化 code/category 保留在用户卡上:流式路径
      // 当前多为通用 "transient",仅作受控语义留存/诊断,不参与门控与分类。
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
      // 去重按错误身份(kind+技术详情),不按最终措辞:同一回合先 transient
      // (recoverable 文案)后 done(terminal 文案)时措辞必然不同,按文本全等
      // 去重会漏,导致同一错误双气泡且措辞互相矛盾。
      const existing = chatItems.find(function (item) {
        if (!item || !item.turnErrorNotice || !item.userError) return false;
        if (item.userError.kind !== userError.kind) return false;
        const existingDetail = item.userError.technicalDetail;
        if (existingDetail || nextDetail) return existingDetail === nextDetail;
        return true;
      });
      // 终态隐藏气泡的前提:时间线终态记录确实写入且带 error(错误卡接管)。
      // recordTurnCompleted 在 openStart/turnId 缺失时不写记录,此时若仍隐藏,
      // 错误将完全不可见(静默吞错回归)。
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
      // 终态接管时,同回合其余模型服务 transient 气泡(身份与终态不同,如先
      // network idle timeout 后 billing done)一并隐藏:它们的"系统会继续重试"
      // 措辞与终态"已停止"矛盾。bridge 在每次发送时清空 turnErrorNotice 项,
      // 现存项均属于当前回合,不会误伤上一回合。
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
    // chat:done 成功(无 error)到达时,同回合的 transient 模型服务气泡("系统
    // 会继续重试")已过时:回合恢复完成,过时声明不能比它描述的恢复活得更久。
    // 仅隐藏带 userError 的项(其措辞含"继续重试"的未来承诺);裸串回退项是
    // 对已发生错误的陈述,保留既有行为。错误终态不走此路径——
    // addModelServiceErrorNotice 已按身份升级/隐藏。
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
