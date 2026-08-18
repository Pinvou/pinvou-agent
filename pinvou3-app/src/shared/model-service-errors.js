(function () {
  "use strict";

  const SENSITIVE_VALUE = "[敏感信息已隐藏]";
  const PROVIDER_LABELS = {
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
  };

  // 底座(CodeWhale)模型调用失败的固定错误前缀(SSE stream request failed /
  // idle timeout / headers timed out / buffer exceeded、Stream read error、
  // Failed to call ... Chat API)。这些前缀只由模型请求链路产生,出现即可接管。
  const MODEL_CALL_PREFIXES = [
    "sse stream",
    "stream read error",
    "chat api",
  ];

  // 泛化信号词分两档:计费/额度/频控词(余额不足、quota exceeded、请求过于
  // 频繁等)强指向模型 API 计费语义,本地工具错误几乎不会出现,可无条件接管;
  // 网络/服务/超时词(timeout、connection refused、server error 等)在本地
  // 工具错误(git、ssh、npm、docker、脚本退出码)里同样常见,必须叠加
  // API/厂商上下文(hasApiSignal/hasProviderNameSignal)才允许接管。
  const STRONG_MODEL_ERROR_KEYWORDS = [
    "invalid api key",
    "insufficient balance",
    "insufficient quota",
    "quota exceeded",
    "quota exhausted",
    "quota has been exceeded",
    "exceeded your current quota",
    "payment required",
    "账户余额",
    "余额不足",
    "欠费",
    "额度不足",
    "额度用尽",
    "额度耗尽",
    "用量超出",
  ];

  const AMBIGUOUS_MODEL_ERROR_KEYWORDS = [
    "api key",
    "invalid token",
    "rate limit",
    "too many requests",
    "请求过于频繁",
    "context length",
    "context window",
    "maximum context",
    "prompt is too long",
    "timeout",
    "timed out",
    "econnrefused",
    "connection refused",
    "connection reset",
    "service unavailable",
    "temporarily unavailable",
    "server error",
  ];

  function languageTag(language) {
    return language === "en" ? "en" : language === "ja" ? "ja" : "zh";
  }

  function textFor(language, key, provider) {
    const lang = languageTag(language);
    const p = provider || (lang === "en" ? "current model service" : lang === "ja" ? "現在のモデルサービス" : "当前模型服务");
    const copy = {
      zh: {
        billingTitle: p + "账户余额不足",
        billingMessage: "当前使用的 " + p + " API 账户余额不足，{stop}请充值对应平台账户，或在模型设置中切换到其他可用模型。",
        quotaTitle: p + " API 额度不足",
        quotaMessage: "当前使用的 " + p + " API 额度不足，{stop}请检查额度，或在模型设置中切换到其他可用模型。",
        rateTitle: p + "请求过于频繁",
        rateMessage: "当前模型服务请求过于频繁，{stop}请稍后重试，或切换到其他可用模型。",
        authTitle: p + " API Key 无效",
        authMessage: "当前模型服务的 API Key 无效或已失效。请在模型设置中检查并重新填写。",
        permissionTitle: p + "没有访问权限",
        permissionMessage: "当前 API Key 没有访问该模型服务的权限。请检查账号权限，或切换到其他可用模型。",
        serverTitle: p + "服务暂时不可用",
        serverMessage: "当前模型服务暂时不可用，{stop}请稍后重试，或切换到其他可用模型。",
        networkTitle: "网络连接失败",
        networkMessage: "无法连接到当前模型服务。请检查网络、代理或服务地址后重试。",
        contextTitle: "上下文太长",
        contextMessage: "当前对话内容超过模型可处理范围。请压缩上下文、减少输入内容，或开启新会话后重试。",
        unknownTitle: "当前模型服务不可用",
        unknownMessage: "当前模型服务返回异常，{stop}请稍后重试，或在模型设置中切换到其他可用模型。",
      },
      en: {
        billingTitle: p + " account balance is insufficient",
        billingMessage: "The " + p + " API account does not have enough balance, {stop}Add balance with the provider or switch to another model in settings.",
        quotaTitle: p + " API quota is insufficient",
        quotaMessage: "The " + p + " API quota is insufficient, {stop}Check quota or switch to another model in settings.",
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
        billingMessage: "現在使用している " + p + " API アカウントの残高が不足しているため、{stop}プロバイダー側でチャージするか、モデル設定で別のモデルに切り替えてください。",
        quotaTitle: p + " API の割り当てが不足しています",
        quotaMessage: "現在使用している " + p + " API の割り当てが不足しているため、{stop}割り当てを確認するか、別のモデルに切り替えてください。",
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
    const match = String(text || "").match(/\bHTTP\s*(\d{3})\b/i)
      || String(text || "").match(/\bstatus[=:\s]+(\d{3})\b/i);
    return match ? Number(match[1]) : null;
  }

  function normalizeForMatch(text) {
    return String(text || "")
      .toLowerCase()
      .replaceAll(/[_-]+/g, " ")
      .replaceAll(/\s+/g, " ")
      .trim();
  }

  function hasAny(lower, normalized, words) {
    return words.some(function (word) {
      const normalizedWord = normalizeForMatch(word);
      return lower.includes(String(word).toLowerCase())
        || normalized.includes(normalizedWord);
    });
  }

  function hasApiSignal(lower, normalized) {
    return hasAny(lower, normalized, [
      "api key", "api account", "api quota", "model service",
      "model endpoint", "provider endpoint", "sse stream",
    ]);
  }

  function hasProviderNameSignal(lower, normalized) {
    return hasAny(lower, normalized, [
      "openai", "deepseek", "anthropic", "moonshot", "kimi", "dashscope",
      "qwen", "doubao", "volcengine", "zhipu", "gemini",
    ]);
  }

  // 分类器只接管两类错误:①底座模型调用的固定前缀(见 MODEL_CALL_PREFIXES);
  // ②模型服务语义词,且必须叠加 API/厂商上下文——裸的 timeout/connection
  // refused/server error 等词在本地工具错误(git、ssh、npm、docker)里同样
  // 常见,不能仅凭它们断言"模型服务故障"。
  function isModelServiceError(raw) {
    if (raw && typeof raw === "object" && raw.kind && raw.title && raw.message) return true;
    const text = String(raw || "");
    const lower = text.toLowerCase();
    const normalized = normalizeForMatch(text);
    if (hasAny(lower, normalized, MODEL_CALL_PREFIXES)) return true;
    if (hasAny(lower, normalized, STRONG_MODEL_ERROR_KEYWORDS)) return true;
    const apiSignal = hasApiSignal(lower, normalized);
    const providerSignal = hasProviderNameSignal(lower, normalized);
    const status = extractHttpStatus(text);
    const statusIsModelLike = status !== null
      && (status === 401 || status === 402 || status === 403 || status === 429 || (status >= 500 && status <= 599))
      && /http|status|请求|响应/i.test(text);
    if (statusIsModelLike && (apiSignal || providerSignal)) return true;
    if (apiSignal) return true;
    if (providerSignal && hasAny(lower, normalized, AMBIGUOUS_MODEL_ERROR_KEYWORDS)) return true;
    return false;
  }

  function classify(raw) {
    const text = String(raw || "");
    const lower = text.toLowerCase();
    const normalized = normalizeForMatch(text);
    const status = extractHttpStatus(text);

    if (hasAny(lower, normalized, ["context length", "maximum context", "prompt is too long", "context window"])) {
      return { kind: "context", httpStatus: status };
    }
    if (status === 401 || hasAny(lower, normalized, ["unauthorized", "authentication", "invalid api key", "invalid key", "invalid token", "bearer token"])) {
      return { kind: "auth", httpStatus: status };
    }
    if (status === 402 || hasAny(lower, normalized, ["payment required", "insufficient balance", "余额不足", "欠费", "账户余额"])) {
      return { kind: "billing", httpStatus: status };
    }
    if (hasAny(lower, normalized, ["quota exceeded", "insufficient quota", "quota exhausted", "quota has been exceeded", "exceeded your current quota", "额度不足", "额度用尽", "额度耗尽", "用量超出"])) {
      return { kind: "quota", httpStatus: status };
    }
    // 403 与频控词共存时按频控分(GitHub/OpenAI 风格 "403 forbidden: rate limit exceeded")
    if (status === 429 || hasAny(lower, normalized, ["rate limit", "too many requests", "请求过于频繁"])) {
      return { kind: "rate_limit", httpStatus: status };
    }
    if (status === 403 || hasAny(lower, normalized, ["forbidden", "没有访问权限", "没有权限"])
        || (hasApiSignal(lower, normalized) && hasAny(lower, normalized, ["permission denied", "authorization", "access denied"]))) {
      return { kind: "permission", httpStatus: status };
    }
    if ((status >= 500 && status <= 599) || hasAny(lower, normalized, ["server error", "temporarily unavailable", "service unavailable"])) {
      return { kind: "server", httpStatus: status };
    }
    if (hasAny(lower, normalized, ["timeout", "timed out", "dns", "connection", "network", "tls", "econnrefused", "connection refused", "connection reset", "stream read error", "chunk decode", "连接失败"])) {
      return { kind: "network", httpStatus: status };
    }
    return { kind: "unknown", httpStatus: status };
  }

  function redactTechnicalDetail(raw) {
    let text = String(raw || "");
    // Bearer/Basic 及任意 Authorization scheme 的凭证统一吞掉值本身
    // (Basic <base64> 以前只吞掉 scheme 词,凭证明文保留)。
    const keepPrefix = (prefix) => prefix + SENSITIVE_VALUE;
    text = text.replaceAll(/((?:authorization|proxy-authorization)\s*[:=]\s*(?:bearer|basic|digest|token)\s+)[^\s,;"}]+/ig, (m, p1) => keepPrefix(p1));
    text = text.replaceAll(/(Authorization\s*[:=]\s*)[^\s,;"}]+/ig, (m, p1) => keepPrefix(p1));
    text = text.replaceAll(/\b(Bearer\s+)[A-Za-z0-9._~+=/-]{12,}/g, (m, p1) => keepPrefix(p1));
    // kv 形态的 key 名要求左侧词边界,且值必须像凭证(字母开头的非纯数字串):
    // "monkey:bar"(词尾含 key)与 "token: 15000"/"total_token: 15000"(用量
    // 计数)不能被当凭证脱敏,否则恰好毁掉 context 错误的排查信息。
    // eslint-disable-next-line sonarjs/regex-complexity, sonarjs/duplicates-in-character-class -- the credential-key whitelist is deliberately exhaustive; splitting it would reduce auditability
    text = text.replaceAll(/(["']?\b(?:api[_-]?key|key|authorization|token|password|secret|access[_-]?token)\b["']?\s*[:=]\s*["']?)[A-Za-z][^"',\s&}]*/ig, (m, p1) => keepPrefix(p1));
    text = text.replaceAll(/([?&](?:api[_-]?key|key|authorization|token|password|secret|access[_-]?token)=)[^&#\s]+/ig, (m, p1) => keepPrefix(p1));
    text = text.replaceAll(/\bsk-[A-Za-z0-9][A-Za-z0-9._-]{10,}\b/g, () => SENSITIVE_VALUE);
    if (text.length > 2000) text = [...text].slice(0, 2000).join("") + "...";
    return text;
  }

  function providerLabelFrom(value, language) {
    if (!value || typeof value !== "object") return "";
    const explicit = value.providerLabel || value.provider_label || value.name || value.display_name || value.title;
    if (explicit && String(explicit).trim()) return String(explicit).trim();
    const keys = [value.vendor, value.provider, value.preset, value.provider_kind, value.model]
      .map(function (entry) { return String(entry || "").trim().toLowerCase(); })
      .filter(Boolean);
    for (let i = 0; i < keys.length; i++) {
      const key = keys[i].replaceAll(/\s+/g, "_");
      if (key === "openai_compatible") return defaultProviderLabel(language);
      if (PROVIDER_LABELS[key]) return PROVIDER_LABELS[key];
      if (key.includes("deepseek")) return "DeepSeek";
      if (key.includes("openai")) return "OpenAI";
      if (key.includes("kimi") || key.includes("moonshot")) return "Kimi";
      if (key.includes("qwen") || key.includes("dashscope")) return "通义千问";
      if (key.includes("doubao") || key.includes("volc")) return "豆包";
      if (key.includes("minimax")) return "MiniMax";
      if (key.includes("glm") || key.includes("zai") || key.includes("zhipu")) return "智谱";
      if (key.includes("anthropic") || key.includes("claude")) return "Claude";
    }
    return "";
  }

  function providerLabelFromState(state, fallbackRoute, language) {
    let saved = null;
    const activeId = state && (state.currentSessionModelId || state.activeModelId);
    if (state && Array.isArray(state.savedModels) && activeId) {
      saved = state.savedModels.find(function (model) { return model && model.id === activeId; }) || null;
    }
    return providerLabelFrom(fallbackRoute, language)
      || providerLabelFrom(saved, language)
      || providerLabelFrom(state && state.effectiveModelConfig, language)
      || providerLabelFrom(state && state.activeProvider, language)
      || "";
  }

  function defaultProviderLabel(language) {
    const lang = languageTag(language);
    return lang === "en" ? "current model service" : lang === "ja" ? "現在のモデルサービス" : "当前模型服务";
  }

  // 从错误文本自身提取 provider 名。历史回合重建友好提示时(重启/切会话后),
  // bridge state 里的 currentSessionModelId 是"当前"模型而不是出错回合当时的
  // 模型;provider 信号(如 URL 里的 api.deepseek.com)比当前模型配置更可信,
  // 因此它优先于 providerLabelFromState 的推导。
  const PROVIDER_SIGNAL_ORDER = [
    "deepseek", "anthropic", "moonshot", "kimi", "dashscope", "qwen",
    "doubao", "volcengine", "zhipu", "glm", "minimax", "openai", "gemini",
  ];
  function providerLabelFromErrorText(raw) {
    const lower = String(raw || "").toLowerCase();
    for (let i = 0; i < PROVIDER_SIGNAL_ORDER.length; i++) {
      const key = PROVIDER_SIGNAL_ORDER[i];
      if (lower.includes(key)) return PROVIDER_LABELS[key] || key;
    }
    return "";
  }

  function build(raw, options) {
    options = options || {};
    const language = options.language || "zh-Hans";
    if (raw && typeof raw === "object" && raw.kind && raw.title && raw.message) {
      const allowedKind = {
        billing: true,
        quota: true,
        rate_limit: true,
        auth: true,
        permission: true,
        server: true,
        network: true,
        context: true,
        unknown: true,
      };
      const kind = allowedKind[raw.kind] ? raw.kind : "unknown";
      return Object.assign({}, raw, {
        kind,
        title: redactTechnicalDetail(raw.title),
        message: redactTechnicalDetail(raw.message),
        retryable: raw.retryable === true,
        technicalDetail: redactTechnicalDetail(raw.technicalDetail || raw.technical_detail || raw.detail || ""),
      });
    }
    const technicalDetail = redactTechnicalDetail(raw);
    const classified = classify(raw);
    // 错误文本里的 provider 信号优先于调用方从"当前"模型配置推导的标签
    // (历史回合重建时当前模型≠出错回合的模型)。
    const provider = providerLabelFromErrorText(raw)
      || options.providerLabel
      || providerLabelFrom(options.provider, language)
      || defaultProviderLabel(language);
    const key = {
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
    let message = textFor(language, key[1], provider);
    const stopPhrase = stopPhraseFor(language, options.terminal !== false);
    message = message.split("{stop}").join(stopPhrase);
    return {
      kind: classified.kind,
      providerLabel: provider,
      title: textFor(language, key[0], provider),
      message,
      retryable: key[2],
      technicalDetail,
      httpStatus: classified.httpStatus || undefined,
    };
  }

  // terminal 与 transient(recoverable)共用同一批 message 模板,差异只在
  // {stop} 占位符:终态声明"本次回复已停止",瞬态声明"系统会继续重试"。
  // 旧实现按措辞字符串 replace 改写,文案一变就静默失效,故改为占位符参数化。
  function stopPhraseFor(language, terminal) {
    const lang = languageTag(language);
    if (lang === "en") return terminal ? "so this reply stopped. " : "Pinvou will keep retrying this reply. ";
    if (lang === "ja") return terminal ? "この応答は停止しました。" : "Pinvou は現在の応答を再試行しています。";
    return terminal ? "本次回复已停止。" : "系统会继续重试当前回复。";
  }

  function noticeText(userError) {
    if (!userError) return "";
    return "⚠️ " + userError.title + "\n" + userError.message;
  }

  const api = {
    classify,
    build,
    isModelServiceError,
    noticeText,
    redactTechnicalDetail,
    providerLabelFromState,
  };

  if (typeof window !== "undefined") window.PinvouModelServiceErrors = Object.freeze(api);
  if (typeof globalThis !== "undefined") globalThis.PinvouModelServiceErrors = globalThis.PinvouModelServiceErrors || api;
})();
