(function () {
  "use strict";

  var SENSITIVE_VALUE = "[敏感信息已隐藏]";
  var PROVIDER_LABELS = {
    deepseek: "DeepSeek",
    openai: "OpenAI",
    moonshot: "Kimi",
    kimi: "Kimi",
    qwen: "通义千问",
    dashscope: "通义千问",
    doubao: "豆包",
    volcengine: "豆包",
    minimax: "MiniMax",
    glm: "智谱",
    zai: "智谱",
    zhipu: "智谱",
    anthropic: "Claude",
    xai: "xAI",
    gemini: "Gemini",
    openai_compatible: "当前模型服务",
  };

  function languageTag(language) {
    return language === "en" ? "en" : language === "ja" ? "ja" : "zh";
  }

  function textFor(language, key, provider) {
    var lang = languageTag(language);
    var p = provider || (lang === "en" ? "current model service" : lang === "ja" ? "現在のモデルサービス" : "当前模型服务");
    var copy = {
      zh: {
        billingTitle: p + "账户余额不足",
        billingMessage: "当前使用的 " + p + " API 账户余额不足，本次回复已停止。请充值对应平台账户，或在模型设置中切换到其他可用模型。",
        quotaTitle: p + " API 额度不足",
        quotaMessage: "当前使用的 " + p + " API 额度不足，本次回复已停止。请检查额度，或在模型设置中切换到其他可用模型。",
        rateTitle: p + "请求过于频繁",
        rateMessage: "当前模型服务请求过于频繁，本次回复已停止。请稍后重试，或切换到其他可用模型。",
        authTitle: p + " API Key 无效",
        authMessage: "当前模型服务的 API Key 无效或已失效。请在模型设置中检查并重新填写。",
        permissionTitle: p + "没有访问权限",
        permissionMessage: "当前 API Key 没有访问该模型服务的权限。请检查账号权限，或切换到其他可用模型。",
        serverTitle: p + "服务暂时不可用",
        serverMessage: "当前模型服务暂时不可用，本次回复已停止。请稍后重试，或切换到其他可用模型。",
        networkTitle: "网络连接失败",
        networkMessage: "无法连接到当前模型服务。请检查网络、代理或服务地址后重试。",
        contextTitle: "上下文太长",
        contextMessage: "当前对话内容超过模型可处理范围。请压缩上下文、减少输入内容，或开启新会话后重试。",
        unknownTitle: "当前模型服务不可用",
        unknownMessage: "当前模型服务返回异常，本次回复已停止。请稍后重试，或在模型设置中切换到其他可用模型。",
      },
      en: {
        billingTitle: p + " account balance is insufficient",
        billingMessage: "The " + p + " API account does not have enough balance, so this reply stopped. Add balance with the provider or switch to another model in settings.",
        quotaTitle: p + " API quota is insufficient",
        quotaMessage: "The " + p + " API quota is insufficient, so this reply stopped. Check quota or switch to another model in settings.",
        rateTitle: p + " is rate-limiting requests",
        rateMessage: "The current model service is receiving too many requests. Try again later or switch to another model.",
        authTitle: p + " API key is invalid",
        authMessage: "The API key for the current model service is invalid or expired. Check it in model settings.",
        permissionTitle: p + " access is not allowed",
        permissionMessage: "The current API key does not have access to this model service. Check account permissions or switch models.",
        serverTitle: p + " is temporarily unavailable",
        serverMessage: "The current model service is temporarily unavailable. Try again later or switch to another model.",
        networkTitle: "Network connection failed",
        networkMessage: "Pinvou could not connect to the current model service. Check network, proxy, or endpoint settings and retry.",
        contextTitle: "Context is too long",
        contextMessage: "This conversation is longer than the model can handle. Compact context, reduce input, or start a new session.",
        unknownTitle: "Current model service is unavailable",
        unknownMessage: "The current model service returned an error. Try again later or switch to another model in settings.",
      },
      ja: {
        billingTitle: p + " のアカウント残高が不足しています",
        billingMessage: "現在使用している " + p + " API アカウントの残高が不足しているため、この応答は停止しました。プロバイダー側でチャージするか、モデル設定で別のモデルに切り替えてください。",
        quotaTitle: p + " API の割り当てが不足しています",
        quotaMessage: "現在使用している " + p + " API の割り当てが不足しているため、この応答は停止しました。割り当てを確認するか、別のモデルに切り替えてください。",
        rateTitle: p + " のリクエストが多すぎます",
        rateMessage: "現在のモデルサービスへのリクエストが多すぎます。しばらくしてから再試行するか、別のモデルに切り替えてください。",
        authTitle: p + " API Key が無効です",
        authMessage: "現在のモデルサービスの API Key が無効、または期限切れです。モデル設定で確認して再入力してください。",
        permissionTitle: p + " にアクセスできません",
        permissionMessage: "現在の API Key にはこのモデルサービスへのアクセス権がありません。アカウント権限を確認するか、別のモデルに切り替えてください。",
        serverTitle: p + " は一時的に利用できません",
        serverMessage: "現在のモデルサービスは一時的に利用できません。しばらくしてから再試行するか、別のモデルに切り替えてください。",
        networkTitle: "ネットワーク接続に失敗しました",
        networkMessage: "現在のモデルサービスに接続できません。ネットワーク、プロキシ、またはエンドポイント設定を確認して再試行してください。",
        contextTitle: "コンテキストが長すぎます",
        contextMessage: "この会話はモデルが処理できる範囲を超えています。コンテキストを圧縮する、入力を減らす、または新しい会話で再試行してください。",
        unknownTitle: "現在のモデルサービスを利用できません",
        unknownMessage: "現在のモデルサービスでエラーが発生しました。しばらくしてから再試行するか、別のモデルに切り替えてください。",
      },
    };
    return copy[lang][key];
  }

  function extractHttpStatus(text) {
    var match = String(text || "").match(/\bHTTP\s*(\d{3})\b/i)
      || String(text || "").match(/\bstatus[=:\s]+(\d{3})\b/i);
    return match ? Number(match[1]) : null;
  }

  function normalizeForMatch(text) {
    return String(text || "")
      .toLowerCase()
      .replace(/[_-]+/g, " ")
      .replace(/\s+/g, " ")
      .trim();
  }

  function hasAny(lower, normalized, words) {
    return words.some(function (word) {
      var normalizedWord = normalizeForMatch(word);
      return lower.indexOf(String(word).toLowerCase()) >= 0
        || normalized.indexOf(normalizedWord) >= 0;
    });
  }

  function hasProviderOrApiSignal(lower, normalized) {
    return hasAny(lower, normalized, [
      "api key", "model service", "language model", "llm", "sse stream request failed",
      "openai", "deepseek", "anthropic", "claude", "moonshot", "kimi",
      "dashscope", "qwen", "doubao", "volcengine", "glm", "zhipu", "gemini", "xai",
    ]);
  }

  function isModelServiceError(raw) {
    if (raw && typeof raw === "object" && raw.kind && raw.title && raw.message) return true;
    var text = String(raw || "");
    var lower = text.toLowerCase();
    var normalized = normalizeForMatch(text);
    var status = extractHttpStatus(text);
    if ([401, 402, 403, 429].indexOf(status) >= 0) return true;
    if (status >= 500 && status <= 599 && hasProviderOrApiSignal(lower, normalized)) return true;
    return hasAny(lower, normalized, [
      "sse stream request failed",
      "api key",
      "invalid api key",
      "invalid token",
      "payment required",
      "insufficient balance",
      "insufficient quota",
      "quota exceeded",
      "quota has been exceeded",
      "exceeded your current quota",
      "rate limit",
      "too many requests",
      "context length",
      "context window",
      "maximum context",
      "prompt is too long",
      "模型服务",
      "账户余额",
      "余额不足",
      "欠费",
      "额度不足",
      "额度用尽",
      "用量超出",
      "请求过于频繁",
    ]) || hasProviderOrApiSignal(lower, normalized);
  }

  function classify(raw) {
    var text = String(raw || "");
    var lower = text.toLowerCase();
    var normalized = normalizeForMatch(text);
    var status = extractHttpStatus(text);

    if (hasAny(lower, normalized, ["context length", "maximum context", "prompt is too long", "context window"])) {
      return { kind: "context", httpStatus: status };
    }
    if (status === 401 || hasAny(lower, normalized, ["unauthorized", "authentication", "invalid api key", "invalid key", "invalid token", "bearer token"])) {
      return { kind: "auth", httpStatus: status };
    }
    if (status === 403 || hasAny(lower, normalized, ["forbidden", "没有访问权限", "没有权限"])
        || (hasProviderOrApiSignal(lower, normalized) && hasAny(lower, normalized, ["permission denied", "authorization", "access denied"]))) {
      return { kind: "permission", httpStatus: status };
    }
    if (status === 402 || hasAny(lower, normalized, ["payment required", "insufficient balance", "余额不足", "欠费", "账户余额"])) {
      return { kind: "billing", httpStatus: status };
    }
    if (hasAny(lower, normalized, ["quota exceeded", "insufficient quota", "quota has been exceeded", "exceeded your current quota", "额度不足", "额度用尽", "用量超出", "耗尽"])) {
      return { kind: "quota", httpStatus: status };
    }
    if (status === 429 || hasAny(lower, normalized, ["rate limit", "too many requests", "请求过于频繁"])) {
      return { kind: "rate_limit", httpStatus: status };
    }
    if (status >= 500 && status <= 599 || hasAny(lower, normalized, ["server error", "temporarily unavailable", "service unavailable"])) {
      return { kind: "server", httpStatus: status };
    }
    if (hasAny(lower, normalized, ["timeout", "timed out", "dns", "connection", "network", "tls", "stream read error", "chunk decode", "连接失败"])) {
      return { kind: "network", httpStatus: status };
    }
    return { kind: "unknown", httpStatus: status };
  }

  function redactTechnicalDetail(raw) {
    var text = String(raw || "");
    text = text.replace(/(Authorization\s*[:=]\s*Bearer\s+)[^\s,;"}]+/ig, "$1" + SENSITIVE_VALUE);
    text = text.replace(/\b(Bearer\s+)[A-Za-z0-9._~+\/=-]{12,}/g, "$1" + SENSITIVE_VALUE);
    text = text.replace(/(["']?(?:api[_-]?key|token|password|secret|access[_-]?token)["']?\s*[:=]\s*["']?)[^"',\s&}]+/ig, "$1" + SENSITIVE_VALUE);
    text = text.replace(/([?&](?:api[_-]?key|token|password|secret|access[_-]?token)=)[^&#\s]+/ig, "$1" + encodeURIComponent(SENSITIVE_VALUE));
    if (text.length > 2000) text = text.slice(0, 2000) + "...";
    return text;
  }

  function providerLabelFrom(value) {
    if (!value || typeof value !== "object") return "";
    var explicit = value.providerLabel || value.provider_label || value.name || value.display_name || value.title;
    if (explicit && String(explicit).trim()) return String(explicit).trim();
    var keys = [value.vendor, value.provider, value.preset, value.provider_kind, value.model]
      .map(function (entry) { return String(entry || "").trim().toLowerCase(); })
      .filter(Boolean);
    for (var i = 0; i < keys.length; i++) {
      var key = keys[i].replace(/\s+/g, "_");
      if (PROVIDER_LABELS[key]) return PROVIDER_LABELS[key];
      if (key.indexOf("deepseek") >= 0) return "DeepSeek";
      if (key.indexOf("openai") >= 0) return "OpenAI";
      if (key.indexOf("kimi") >= 0 || key.indexOf("moonshot") >= 0) return "Kimi";
      if (key.indexOf("qwen") >= 0 || key.indexOf("dashscope") >= 0) return "通义千问";
      if (key.indexOf("doubao") >= 0 || key.indexOf("volc") >= 0) return "豆包";
      if (key.indexOf("minimax") >= 0) return "MiniMax";
      if (key.indexOf("glm") >= 0 || key.indexOf("zai") >= 0 || key.indexOf("zhipu") >= 0) return "智谱";
      if (key.indexOf("anthropic") >= 0 || key.indexOf("claude") >= 0) return "Claude";
    }
    return "";
  }

  function providerLabelFromState(state, fallbackRoute) {
    var saved = null;
    var activeId = state && (state.currentSessionModelId || state.activeModelId);
    if (state && Array.isArray(state.savedModels) && activeId) {
      saved = state.savedModels.find(function (model) { return model && model.id === activeId; }) || null;
    }
    return providerLabelFrom(fallbackRoute)
      || providerLabelFrom(saved)
      || providerLabelFrom(state && state.effectiveModelConfig)
      || providerLabelFrom(state && state.activeProvider)
      || "";
  }

  function defaultProviderLabel(language) {
    var lang = languageTag(language);
    return lang === "en" ? "current model service" : lang === "ja" ? "現在のモデルサービス" : "当前模型服务";
  }

  function build(raw, options) {
    options = options || {};
    if (raw && typeof raw === "object" && raw.kind && raw.title && raw.message) {
      return Object.assign({}, raw, {
        technicalDetail: redactTechnicalDetail(raw.technicalDetail || raw.technical_detail || raw.detail || ""),
      });
    }
    var technicalDetail = redactTechnicalDetail(raw);
    var classified = classify(raw);
    var language = options.language || "zh-Hans";
    var provider = options.providerLabel || providerLabelFrom(options.provider) || defaultProviderLabel(language);
    var key = {
      billing: ["billingTitle", "billingMessage", false],
      quota: ["quotaTitle", "quotaMessage", false],
      rate_limit: ["rateTitle", "rateMessage", true],
      auth: ["authTitle", "authMessage", false],
      permission: ["permissionTitle", "permissionMessage", false],
      server: ["serverTitle", "serverMessage", true],
      network: ["networkTitle", "networkMessage", true],
      context: ["contextTitle", "contextMessage", false],
      unknown: ["unknownTitle", "unknownMessage", true],
    }[classified.kind] || ["unknownTitle", "unknownMessage", true];
    return {
      kind: classified.kind,
      providerLabel: provider,
      title: textFor(language, key[0], provider),
      message: textFor(language, key[1], provider),
      retryable: key[2],
      technicalDetail: technicalDetail,
      httpStatus: classified.httpStatus || undefined,
    };
  }

  function noticeText(userError) {
    if (!userError) return "";
    return "⚠️ " + userError.title + "\n" + userError.message;
  }

  var api = {
    classify: classify,
    build: build,
    isModelServiceError: isModelServiceError,
    noticeText: noticeText,
    redactTechnicalDetail: redactTechnicalDetail,
    providerLabelFromState: providerLabelFromState,
  };

  if (typeof window !== "undefined") window.PinvouModelServiceErrors = Object.freeze(api);
  if (typeof globalThis !== "undefined") globalThis.PinvouModelServiceErrors = globalThis.PinvouModelServiceErrors || api;
})();
