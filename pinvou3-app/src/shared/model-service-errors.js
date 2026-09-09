(function () {
  "use strict";

  // Redaction placeholder follows the UI language: technical details must
  // not mix Chinese into en/ja interfaces.
  const SENSITIVE_PLACEHOLDERS = {
    zh: "[敏感信息已隐藏]",
    en: "[redacted]",
    ja: "[秘匿済み]",
  };
  // Brand labels follow the UI language: Chinese brand names (Qwen/Zhipu/
  // Doubao) are only used for zh; en/ja use the latin names so cards never
  // mix scripts ("The 通义千问 API quota ..." style mixing).
  const PROVIDER_LABELS = {
    zh: {
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
      claude: "Claude",
      xai: "xAI",
      gemini: "Gemini",
    },
    en: {
      deepseek: "DeepSeek",
      openai: "OpenAI",
      moonshot: "Kimi",
      kimi: "Kimi",
      qwen: "Qwen",
      dashscope: "Qwen",
      doubao: "Doubao",
      volcengine: "Doubao",
      minimax: "MiniMax",
      glm: "Zhipu",
      zai: "Zhipu",
      zhipu: "Zhipu",
      anthropic: "Claude",
      claude: "Claude",
      xai: "xAI",
      gemini: "Gemini",
    },
    ja: {
      deepseek: "DeepSeek",
      openai: "OpenAI",
      moonshot: "Kimi",
      kimi: "Kimi",
      qwen: "Qwen",
      dashscope: "Qwen",
      doubao: "Doubao",
      volcengine: "Doubao",
      minimax: "MiniMax",
      glm: "Zhipu",
      zai: "Zhipu",
      zhipu: "Zhipu",
      anthropic: "Claude",
      claude: "Claude",
      xai: "xAI",
      gemini: "Gemini",
    },
  };

  function providerLabel(key, language) {
    const table = PROVIDER_LABELS[languageTag(language)] || PROVIDER_LABELS.zh;
    return table[key] || PROVIDER_LABELS.zh[key] || "";
  }

  // Fixed error prefixes of base (CodeWhale) model-call failures; their
  // presence alone authorizes takeover, in two groups:
  // ① SSE streaming path (chat.rs/stream_entry.rs): SSE stream request
  //    failed / idle timeout / headers timed out / buffer exceeded,
  //    Stream read error, Failed to call ... Chat API;
  // ② LlmError Display lead-ins (llm_client/mod.rs; transport-level
  //    immediate failures — DNS/connection refused/TLS — propagate without
  //    an SSE prefix): produced solely by llm_client, they cannot collide
  //    with local tool error wording, and the colon/parenthesis anchoring
  //    means "rate limit exceeded:" never matches the gh CLI's
  //    "API rate limit exceeded for ...".
  const MODEL_CALL_PREFIXES = [
    "sse stream",
    "sse buffer",
    "stream read error",
    "chat api",
    "rate limit exceeded:",
    "authentication failed:",
    "authorization failed:",
    "context length exceeded:",
    "network error:",
    "server error (",
    "request timed out after ",
    "invalid request (",
    "llm error:",
    "model error:",
    "response parsing error:",
    "content policy violation:",
    "provider stream connection dropped",
  ];

  // Generic signal words in three tiers:
  // ① Unconditional — model API billing/quota words and "invalid api key"
  //    ("incorrect api key" is OpenAI's real wording); local tool errors
  //    practically never produce them;
  // ② Context-gated — generic Chinese payment phrases (账户余额/余额不足/
  //    欠费 are equally common in local payment/transfer errors, so they
  //    only take over with an API/provider context);
  // ③ Network/service/timeout words (timeout, connection refused,
  //    server error, ...) are just as common in local tool errors (git,
  //    ssh, npm, docker, script exit codes) and require an API/provider
  //    context (hasApiSignal/hasProviderNameSignal) before takeover.
  const STRONG_MODEL_ERROR_KEYWORDS = [
    "invalid api key",
    "incorrect api key",
    "insufficient quota",
    "quota exceeded",
    "quota exhausted",
    "quota has been exceeded",
    "exceeded your current quota",
    "payment required",
    // Anthropic's official 402 wording ("Your credit balance is too low")
    // and Tongyi's "Arrearage" error code: pure vendor billing language
    // that local tool errors practically never produce, so bare strings
    // without a status code (ACP lane / subagent panel) also take over.
    "credit balance",
    "arrearage",
    // Content-policy rejections are model-service-specific semantics
    // (OpenAI moderation/safety system, Anthropic content filtering);
    // local tools never produce them, so they take over unconditionally.
    // They are deterministic failures and must not tell users to "retry
    // later" (see the content copy).
    "content policy",
    "content filter",
    "content filtering",
    "safety system",
    "额度不足",
    "额度用尽",
    "额度耗尽",
    "用量超出",
    "resource exhausted",
  ];

  // Generic Chinese payment phrases: their semantics alone do not justify
  // takeover (local payment/transfer errors use the same words), but with
  // an API/provider context they are a strong billing signal
  // ("GLM 400 余额不足").
  const STRONG_WITH_CONTEXT_KEYWORDS = [
    "账户余额",
    "余额不足",
    "欠费",
    // "insufficient balance" stays in the same tier as the Chinese payment
    // phrases above: DeepSeek's real 402 wording is already taken over via
    // MODEL_CALL_PREFIXES / status codes, while local wallet/payment failures
    // say the same words and must not be told to top up the model API.
    "insufficient balance",
  ];

  const AMBIGUOUS_MODEL_ERROR_KEYWORDS = [
    // Note: "api key" is deliberately absent — it is a context noun for
    // hasApiSignal, not error semantics; keeping it here would gut the
    // gate ("failed to save api key to config: disk full" style local
    // errors would be hijacked). The strong "invalid api key" is covered
    // by the STRONG list.
    "invalid token",
    "unauthorized",
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

  function defaultProviderLabel(language) {
    const lang = languageTag(language);
    return lang === "en" ? "current model service" : lang === "ja" ? "現在のモデルサービス" : "当前模型服务";
  }

  // The tri-lingual copy table is hoisted to a module constant: each build
  // does two table lookups instead of rebuilding a ~60-entry object literal
  // (error classification runs on every event). {provider} is substituted
  // at lookup time; the {stop} placeholder is replaced by build() with the
  // terminal or transient wording.
  const COPY = {
    zh: {
      billingTitle: "{provider}账户余额不足",
      billingMessage: "当前使用的 {provider} API 账户余额不足，{stop}请充值对应平台账户，或在模型设置中切换到其他可用模型。",
      quotaTitle: "{provider} API 额度不足",
      quotaMessage: "当前使用的 {provider} API 额度不足，{stop}请检查额度，或在模型设置中切换到其他可用模型。",
      rateTitle: "{provider}请求过于频繁",
      rateMessage: "当前模型服务请求过于频繁，{stop}请稍后重试，或切换到其他可用模型。",
      authTitle: "{provider} API Key 无效",
      authMessage: "当前模型服务的 API Key 无效或已失效。请在模型设置中检查并重新填写。",
      permissionTitle: "{provider}没有访问权限",
      permissionMessage: "当前 API Key 没有访问该模型服务的权限。请检查账号权限，或切换到其他可用模型。",
      serverTitle: "{provider}服务暂时不可用",
      serverMessage: "当前模型服务暂时不可用，{stop}请稍后重试，或切换到其他可用模型。",
      networkTitle: "网络连接失败",
      networkMessage: "无法连接到当前模型服务。{stop}请检查网络、代理或服务地址后重试。",
      contextTitle: "上下文太长",
      contextMessage: "当前对话内容超过模型可处理范围。请压缩上下文、减少输入内容，或开启新会话后重试。",
      contentTitle: "内容被模型服务拒绝",
      contentMessage: "当前内容被模型服务的内容政策拒绝。请调整表述或拆分内容后重试，或在模型设置中切换到其他模型。",
      unknownTitle: "当前模型服务不可用",
      unknownMessage: "当前模型服务返回异常，{stop}请稍后重试，或在模型设置中切换到其他可用模型。",
    },
    en: {
      billingTitle: "{provider} account balance is insufficient",
      billingMessage: "The {provider} API account does not have enough balance, {stop}Add balance with the provider or switch to another model in settings.",
      quotaTitle: "{provider} API quota is insufficient",
      quotaMessage: "The {provider} API quota is insufficient, {stop}Check quota or switch to another model in settings.",
      rateTitle: "{provider} is rate-limiting requests",
      rateMessage: "The current model service is receiving too many requests, {stop}Try again later or switch to another model.",
      authTitle: "{provider} API key is invalid",
      authMessage: "The API key for the current model service is invalid or expired. Check it in model settings.",
      permissionTitle: "{provider} access is not allowed",
      permissionMessage: "The current API key does not have access to this model service. Check account permissions or switch models.",
      serverTitle: "{provider} is temporarily unavailable",
      serverMessage: "The current model service is temporarily unavailable, {stop}Try again later or switch to another model.",
      networkTitle: "Network connection failed",
      networkMessage: "鲜小助 could not connect to the current model service. {stop}Check network, proxy, or endpoint settings and retry.",
      contextTitle: "Context is too long",
      contextMessage: "This conversation is longer than the model can handle. Compact context, reduce input, or start a new session.",
      contentTitle: "Content rejected by the model service",
      contentMessage: "The model service declined this content under its content policy. Rephrase or split the content, or switch to another model in settings.",
      unknownTitle: "Current model service is unavailable",
      unknownMessage: "The current model service returned an error, {stop}Try again later or switch to another model in settings.",
    },
    ja: {
      billingTitle: "{provider} のアカウント残高が不足しています",
      billingMessage: "現在使用している {provider} API アカウントの残高が不足しているため、{stop}プロバイダー側でチャージするか、モデル設定で別のモデルに切り替えてください。",
      quotaTitle: "{provider} API の割り当てが不足しています",
      quotaMessage: "現在使用している {provider} API の割り当てが不足しているため、{stop}割り当てを確認するか、別のモデルに切り替えてください。",
      rateTitle: "{provider} のリクエストが多すぎます",
      rateMessage: "現在のモデルサービスへのリクエストが多すぎます。{stop}しばらくしてから再試行するか、別のモデルに切り替えてください。",
      authTitle: "{provider} API Key が無効です",
      authMessage: "現在のモデルサービスの API Key が無効、または期限切れです。モデル設定で確認して再入力してください。",
      permissionTitle: "{provider} にアクセスできません",
      permissionMessage: "現在の API Key にはこのモデルサービスへのアクセス権がありません。アカウント権限を確認するか、別のモデルに切り替えてください。",
      serverTitle: "{provider} は一時的に利用できません",
      serverMessage: "現在のモデルサービスは一時的に利用できません。{stop}しばらくしてから再試行するか、別のモデルに切り替えてください。",
      networkTitle: "ネットワーク接続に失敗しました",
      networkMessage: "現在のモデルサービスに接続できません。{stop}ネットワーク、プロキシ、またはエンドポイント設定を確認して再試行してください。",
      contextTitle: "コンテキストが長すぎます",
      contextMessage: "この会話はモデルが処理できる範囲を超えています。コンテキストを圧縮する、入力を減らす、または新しい会話で再試行してください。",
      contentTitle: "コンテンツがモデルサービスに拒否されました",
      contentMessage: "現在のコンテンツはモデルサービスのコンテンツポリシーによって拒否されました。表現を変えるか内容を分割して再試行するか、モデル設定で別のモデルに切り替えてください。",
      unknownTitle: "現在のモデルサービスを利用できません",
      unknownMessage: "現在のモデルサービスでエラーが発生しました。{stop}しばらくしてから再試行するか、別のモデルに切り替えてください。",
    },
  };

  function textFor(language, key, provider) {
    const lang = languageTag(language);
    const p = provider || defaultProviderLabel(language);
    let out = String(COPY[lang][key]).split("{provider}").join(p);
    // The zh default label already ends with 服务 ("service"), so appending
    // the serverTitle suffix would double it ("当前模型服务服务暂时不可用");
    // brand labels (DeepSeek/Zhipu/...) keep the suffix.
    if (lang === "zh" && key === "serverTitle" && p.endsWith("服务")) {
      out = p + "暂时不可用";
    }
    return out;
  }

  function extractHttpStatus(text) {
    // HTTP/1.1 429 and HTTP/2 503 (with version segment) plus axios's
    // "status code 429" are the common shapes.
    const match = String(text || "").match(/\bHTTPS?\/?[\d.]*\s*(\d{3})\b/i)
      || String(text || "").match(/\bstatus(?:\s+code)?[=:\s]+(\d{3})\b/i);
    return match ? Number(match[1]) : null;
  }

  function normalizeForMatch(text) {
    return String(text || "")
      .toLowerCase()
      .replaceAll(/[_-]+/g, " ")
      .replaceAll(/\s+/g, " ")
      .trim();
  }

  // Keyword matching is word-bounded: a bare includes() would let
  // "chat api" hit "chat apiary" or "api key" hit "api-keys.yaml"
  // ("api keys" after normalization), misclassifying local tool errors as
  // model service failures. Regexes are compiled per keyword and cached
  // (classification runs on every error).
  const KEYWORD_REGEX_CACHE = new Map();
  function keywordRegex(word) {
    let re = KEYWORD_REGEX_CACHE.get(word);
    if (!re) {
      const escaped = String(word).replaceAll(/[.*+?^${}()|[\]\\]/g, String.raw`\$&`);
      // The trailing word boundary only makes sense for keywords ending in
      // a letter/digit: anchored forms ending in "(" or a space ("server
      // error (", "request timed out after ") are already separated from
      // what follows, and a lookahead there would reject the number that
      // legitimately follows (status code / duration).
      const tail = /[a-z0-9]$/i.test(word) ? "(?![a-z0-9])" : "";
      re = new RegExp("(?:^|[^a-z0-9])" + escaped + tail, "i");
      KEYWORD_REGEX_CACHE.set(word, re);
    }
    return re;
  }

  function hasAny(lower, normalized, words) {
    return words.some(function (word) {
      const re = keywordRegex(word);
      return re.test(lower) || re.test(normalized);
    });
  }

  function hasApiSignal(lower, normalized) {
    return hasAny(lower, normalized, [
      "api key", "api account", "api quota", "api error", "model service",
      "model endpoint", "provider endpoint", "sse stream",
    ]);
  }

  function hasProviderNameSignal(lower, normalized) {
    return hasAny(lower, normalized, [
      "openai", "deepseek", "anthropic", "claude", "moonshot", "kimi", "dashscope",
      "qwen", "doubao", "volcengine", "zhipu", "gemini",
      "glm", "zai", "minimax", "xai",
    ]);
  }

  // Fixed error shapes of local CLI tools, excluded before the semantic
  // keyword lists: git's "fatal:" lines and ssh/curl's "connect to host
  // ... port N" carry vendor names inside remote URLs/hostnames
  // (github.com/openai, git.openai.com) and, combined with ambiguous
  // network words, would be misjudged as model service failures and
  // hijacked into "please retry" cards; npm/pnpm CLI output headers are
  // the same — the package name in the registry URL (e.g. .../openai)
  // would masquerade as a provider signal. Base model errors always carry
  // a MODEL_CALL_PREFIXES prefix and are taken over above, so they never
  // reach these exclusions and real model service errors are unaffected.
  const LOCAL_TOOL_SHAPES = [
    /(?:^|[\s(@])fatal(?: error)?:/i,
    /(?:connect to host|failed to connect to)\s+\S+\s+port\s+\d+/i,
    /failed to push some refs/i,
    /(?:^|[\r\n])\s{0,4}npm err(?:or)?!/i,
    /err_pnpm_[a-z0-9_]+/i,
  ];

  // The classifier only takes over two error families: ① fixed prefixes of
  // base model calls (see MODEL_CALL_PREFIXES); ② model-service semantic
  // words that must come with an API/provider context — bare timeout/
  // connection refused/server error words are equally common in local tool
  // errors (git, ssh, npm, docker) and cannot assert a model service
  // failure on their own.
  const MEMORY_EXHAUSTION_RE = /(?:^|[^a-z0-9])(?:out\s+of\s+memory|oom|内存(?:耗尽|不足|溢出))(?![a-z0-9])/i;

  function isModelServiceError(raw) {
    if (raw && typeof raw === "object" && raw.kind && raw.title && raw.message) return true;
    const text = String(raw || "");
    const lower = text.toLowerCase();
    const normalized = normalizeForMatch(text);
    if (hasAny(lower, normalized, MODEL_CALL_PREFIXES)) return true;
    // Local CLI tool shapes (see LOCAL_TOOL_SHAPES) are excluded before all
    // semantic keyword lists.
    if (LOCAL_TOOL_SHAPES.some(function (re) { return re.test(text); })) return false;
    // POSIX disk/volume quota exhaustion (EDQUOT's standard strerror is
    // "Disk quota exceeded"; volume mounts report "user quota exceeded")
    // is excluded before the billing strong words, or a local disk-write
    // failure would be answered with "top up or switch models".
    if (/(?:^|[^a-z0-9])(?:disk|nfs|inode|filesystem|storage|user)\s+quota(?![a-z0-9])/i.test(normalized)) return false;
    // Local inference/training memory exhaustion (vLLM/PyTorch "CUDA out of
    // memory", worker OOM) shares its shape with gRPC RESOURCE_EXHAUSTED.
    // Exclude it before the quota keywords so a local OOM is not answered
    // with "check your API quota"; genuine provider quota errors never carry
    // memory-exhaustion wording. classify applies the same rule so prefixed
    // transport errors do not fall into quota either.
    if (MEMORY_EXHAUSTION_RE.test(normalized)) return false;
    if (hasAny(lower, normalized, STRONG_MODEL_ERROR_KEYWORDS)) return true;
    const apiSignal = hasApiSignal(lower, normalized);
    const providerSignal = hasProviderNameSignal(lower, normalized);
    // Generic Chinese payment phrases need an API/provider context: local
    // payment errors like "支付失败:账户余额不足" must not steer users to
    // top up the model API; with context ("GLM 400 余额不足") they remain a
    // strong billing signal.
    if ((apiSignal || providerSignal) && hasAny(lower, normalized, STRONG_WITH_CONTEXT_KEYWORDS)) return true;
    const status = extractHttpStatus(text);
    // extractHttpStatus only returns non-null when the text mentions
    // HTTP/status, but a status code alone is not model service context —
    // local services/proxies emit the same "HTTP/1.1 500" — so it must
    // still combine with an api/provider signal (combined conditions
    // below); a bare status line does not take over.
    const statusIsModelLike = status !== null
      && (status === 401 || status === 402 || status === 403 || status === 429 || (status >= 500 && status <= 599));
    // apiSignal and providerSignal follow the same rule: they require an
    // ambiguous error word or a model-like status code on top. Bare "api
    // key"/"model service" words are context nouns, not error semantics;
    // unconditional takeover would hijack "failed to save api key to
    // config: disk full" style local errors into model service cards. The
    // trade-off is missing texts without an error verb ("api key 配置有
    // 误"), but the STRONG list ("invalid api key", ...) still guarantees
    // takeover for strong semantics.
    if ((apiSignal || providerSignal)
        && (statusIsModelLike || hasAny(lower, normalized, AMBIGUOUS_MODEL_ERROR_KEYWORDS))) return true;
    return false;
  }

  function classify(raw) {
    const text = String(raw || "");
    const lower = text.toLowerCase();
    const normalized = normalizeForMatch(text);
    const status = extractHttpStatus(text);

    // Content-policy rejections (OpenAI moderation/safety system,
    // Anthropic content filtering) are the most specific and come before
    // the other kinds; deterministic failure, no {stop} placeholder.
    if (hasAny(lower, normalized, ["content policy", "content filter", "content filtering", "safety system", "内容政策", "内容安全策略"])) {
      return { kind: "content", httpStatus: status };
    }
    if (hasAny(lower, normalized, ["context length", "maximum context", "prompt is too long", "context window"])) {
      return { kind: "context", httpStatus: status };
    }
    if (status === 401 || hasAny(lower, normalized, ["unauthorized", "authentication", "authorization failed", "invalid api key", "incorrect api key", "invalid key", "invalid token", "bearer token"])) {
      return { kind: "auth", httpStatus: status };
    }
    if (status === 402 || hasAny(lower, normalized, ["payment required", "insufficient balance", "credit balance", "arrearage", "余额不足", "欠费", "账户余额"])) {
      return { kind: "billing", httpStatus: status };
    }
    if (hasAny(lower, normalized, ["quota exceeded", "insufficient quota", "quota exhausted", "quota has been exceeded", "exceeded your current quota", "额度不足", "额度用尽", "额度耗尽", "用量超出"])
      || (hasAny(lower, normalized, ["resource exhausted"]) && !MEMORY_EXHAUSTION_RE.test(normalized))) {
      return { kind: "quota", httpStatus: status };
    }
    // A 403 co-occurring with rate-limit words classifies as rate limiting
    // (GitHub/OpenAI style "403 forbidden: rate limit exceeded").
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

  function redactTechnicalDetail(raw, language) {
    const sensitive = SENSITIVE_PLACEHOLDERS[languageTag(language)];
    let text = String(raw || "");
    const keepPrefix = (prefix) => prefix + sensitive;
    // Authorization/Proxy-Authorization: swallow the whole value for any
    // scheme (the scheme list can never be exhaustive — non-standard
    // schemes like API-Key/HMAC used to leak the credential part
    // entirely), with optional JSON quotes; the value is swallowed up to
    // the first separator (quote/comma/semicolon/paren/&/newline), and
    // spaces inside the value are allowed to cover two-token credentials
    // beyond "Digest username=x, ...". The value part does not consume
    // `[`/`\`/leading whitespace: an already-redacted "[redacted]"
    // placeholder must not re-match — otherwise repeat redaction degrades
    // it into "[redacted][redacted]" and drifts the dedup key (greedy
    // separator backtracking would re-swallow from the leading space, so
    // the first value character is forced to be non-whitespace and the
    // placeholder stays unmatched even after backtracking); excluding `\
    // preserves the trailing `\"` of escaped JSON.
    // Digest parameter blobs (username="x", realm=y) are swallowed whole
    // before the generic value: the generic class stops at the first quote
    // and would leak the comma-separated params. A single-pass alternation
    // also keeps the generic branch from re-masking the Digest placeholder.
    text = text.replaceAll(
      /((?:proxy-)?authorization[\s"'\\]*[:=][\s"'\\]*)(?:digest\s+[^;\r\n]+|(?:[^"'),;}&\][\s\\])(?:[^"'),;}&\][\r\n\\]*))/gi,
      (m, p1) => keepPrefix(p1),
    );
    // Cookie/Set-Cookie headers: swallow the whole line. Multiple
    // credential pairs are "; "-separated and the kv rule's value class
    // below stops at the first space, so only the first pair would be
    // consumed (SID redacted, HSID/SSID leaking in the clear). One HTTP
    // header per line, so swallowing to end-of-line cannot overshoot; the
    // prefix is restricted to [space/quote/line start/JSON separators] to
    // keep ordinary prose like "document.cookie = ..." out; the separator
    // also accepts `\`, covering the escaped-JSON \"cookie\": \"...\"
    // shape.
    text = text.replaceAll(
      /((?:^|[\s"',;\\[])(?:set[\s_-]?cookie|cookie)[\s"'\\]*[:=][\s"'\\]*)[^\r\n]*/gi,
      (m, p1) => keepPrefix(p1),
    );
    // Bare Bearer <token> (no Authorization header name; common in config
    // echoes and troubleshooting logs).
    text = text.replaceAll(/\b(Bearer\s+)[a-z0-9._~+=/-]{12,}/gi, (m, p1) => keepPrefix(p1));
    // Bare Basic/Digest <base64> (same shapes without a header name); the
    // threshold of 12 covers the shortest "user:pass" form (dXNlcjpwYXNz is
    // exactly 12 chars, which the previous 16-char threshold let through)
    // at the cost of occasionally masking a plain word after "basic".
    text = text.replaceAll(/\b((?:Basic|Digest)\s+)[a-z0-9+/=]{12,}/gi, (m, p1) => keepPrefix(p1));
    // Strong credential keys (password/passphrase/secret) swallow the
    // whole value in both shapes:
    // ① quoted values may contain spaces ("correct horse battery staple"
    //    style multi-word passphrases), quotes may carry a `\` prefix
    //    (the {\"password\": \"...\"} escaped-JSON shape after a gateway
    //    embeds the body in an outer envelope); the value matches lazily
    //    and a trailing `\` is returned to the closing-quote group;
    // ② unquoted values are swallowed to end-of-line or the first ,/;/&
    //    — otherwise only the first word of a multi-word passphrase is
    //    consumed and the rest leaks.
    text = text.replaceAll(
      /((?:\\?["'])?\b(?:password|passphrase|secret)\b(?:\\?["'])?\s*[:=]\s*(?:\\?["']))([^"']{2,}?)(\\?["'])/gi,
      (m, p1, p2, p3) => p1 + sensitive + p3,
    );
    text = text.replaceAll(
      /((?:\\?["'])?\b(?:password|passphrase|secret)\b(?:\\?["'])?\s*[:=]\s*)([^"'\r\n,;&]{2,})/gi,
      (m, p1) => keepPrefix(p1),
    );
    // kv-shaped credential keys with a key-name whitelist (including
    // compound names like refresh_token/client_secret).
    // The key's leading boundary cannot be \b — \b does not hold next to
    // an underscore, and the env-var shapes OPENAI_API_KEY /
    // ZHIPUAI_API_KEY end in exactly that `_` prefix; a lookbehind
    // (?<![A-Za-z0-9]) is also out — the static runtime script is bound to
    // the Safari 14 syntax baseline (compat audit; lookbehind needs
    // Safari 16.4). So the boundary is captured into the prefix group:
    // either the quote branch (including escaped-JSON \") or the
    // line-start/non-alphanumeric branch, with keepPrefix preserving the
    // prefix verbatim so the replacement output is unchanged while
    // substring accidents like "x99api_key" stay excluded.
    // The value is either "starts with a letter" or "at least 10
    // alphanumeric characters, possibly containing -/_/./~/+//" (leading
    // digit or hyphenated session credentials such as 8f3k9d2l-4abc-... /
    // 1234-5678-...; `/` and `+` cover Google OAuth refresh tokens
    // ("1//0abc...") and base64 shapes). The "token: 15000" usage counter
    // is pure digits below 10 chars and does not match, preserving the
    // troubleshooting info of context-length errors. An already-redacted
    // placeholder starts with `[` and matches neither value shape, so
    // repeat redaction is idempotent.
    /* eslint-disable sonarjs/regex-complexity, sonarjs/duplicates-in-character-class -- the credential-key whitelist is deliberately exhaustive; splitting it would reduce auditability */
    text = text.replaceAll(
      /((?:\\?["'])|(?:^|[^A-Za-z0-9"']))((?:api[_-]?key|api[_-]?secret|api[_-]?token|authorization|token|password|secret|access[_-]?key|access[_-]?token|refresh[_-]?token|id[_-]?token|auth[_-]?token|client[_-]?secret|app[_-]?secret|consumer[_-]?key|secret[_-]?key|secret[_-]?token|ssh[_-]?key|private[_-]?key|session[_-]?id|session[_-]?token|sessionid|jsessionid|cookie|set[_-]?cookie)\b(?:\\?["'])?\s*[:=]\s*(?:\\?['"])?)((?:[A-Za-z][^"',\s&}]*|[A-Za-z0-9][A-Za-z0-9._~+/-]{9,}))/gi,
      (m, p1, p2) => p1 + p2 + sensitive,
    );
    /* eslint-enable sonarjs/regex-complexity, sonarjs/duplicates-in-character-class */
    // The bare `key` key is tightened separately (value must start with a
    // letter and contain a digit): non-credential values like
    // "key": "model-name" are no longer swallowed while
    // "key": "abc123def456" still is; quotes allow a `\` prefix (escaped
    // JSON).
    /* eslint-disable sonarjs/regex-complexity -- key + optional escaped quotes + separator + digit-lookahead value shape is inherently multi-part; splitting it would obscure the single-match contract */
    text = text.replaceAll(
      /((?:\\?["'])?\bkey\b(?:\\?["'])?\s*[:=]\s*(?:\\?["'])?)((?=[a-z][^"',\s&}]*\d)[a-z][^"',\s&}]+)/gi,
      (m, p1) => keepPrefix(p1),
    );
    /* eslint-enable sonarjs/regex-complexity */
    // URL query-parameter shape (a value in a query string is almost
    // certainly a credential, so no value-shape condition).
    /* eslint-disable sonarjs/regex-complexity -- same whitelist rationale as above */
    text = text.replaceAll(
      /([?&](?:api[_-]?key|api[_-]?secret|api[_-]?token|key|authorization|token|password|secret|access[_-]?key|access[_-]?token|refresh[_-]?token|id[_-]?token|auth[_-]?token|client[_-]?secret|app[_-]?secret|consumer[_-]?key|ssh[_-]?key|session[_-]?id|sessionid)=)[^&#\s]+/gi,
      (m, p1) => keepPrefix(p1),
    );
    /* eslint-enable sonarjs/regex-complexity */
    text = text.replaceAll(/\bsk-[A-Za-z0-9][A-Za-z0-9._-]{10,}\b/g, () => sensitive);
    // Bare Gemini API key (AIza prefix plus ~35 chars; common without a
    // key name in config echoes).
    text = text.replaceAll(/\bAIza[0-9A-Za-z_-]{30,}/g, () => sensitive);
    // Bare JWT/JWS (three eyJ-led segments; the shape without a key name
    // or Bearer prefix).
    text = text.replaceAll(/\b(eyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{4,}\.[A-Za-z0-9_-]*)/g, () => sensitive);
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
      const direct = providerLabel(key, language);
      if (direct) return direct;
      if (key.includes("deepseek")) return providerLabel("deepseek", language);
      if (key.includes("openai")) return providerLabel("openai", language);
      if (key.includes("kimi") || key.includes("moonshot")) return providerLabel("kimi", language);
      if (key.includes("qwen") || key.includes("dashscope")) return providerLabel("qwen", language);
      if (key.includes("doubao") || key.includes("volc")) return providerLabel("doubao", language);
      if (key.includes("minimax")) return providerLabel("minimax", language);
      if (key.includes("glm") || key.includes("zai") || key.includes("zhipu")) return providerLabel("zhipu", language);
      if (key.includes("anthropic") || key.includes("claude")) return providerLabel("anthropic", language);
      if (key.includes("gemini")) return providerLabel("gemini", language);
      if (key.includes("xai")) return providerLabel("xai", language);
    }
    return "";
  }

  function providerLabelFromState(state, language) {
    let saved = null;
    const activeId = state && (state.currentSessionModelId || state.activeModelId);
    if (state && Array.isArray(state.savedModels) && activeId) {
      saved = state.savedModels.find(function (model) { return model && model.id === activeId; }) || null;
    }
    return providerLabelFrom(saved, language)
      || providerLabelFrom(state && state.effectiveModelConfig, language)
      || providerLabelFrom(state && state.activeProvider, language)
      || "";
  }

  // Extract the provider name from the error text itself. When friendly
  // notices are rebuilt for historical turns (after restart/session
  // switch), the bridge state's currentSessionModelId is the *current*
  // model, not the one at the time of the failure; a provider signal in
  // the text (e.g. api.deepseek.com in a URL) is more trustworthy than
  // the current model config, so it takes priority over
  // providerLabelFromState.
  const PROVIDER_SIGNAL_ORDER = [
    "deepseek", "anthropic", "claude", "moonshot", "kimi", "dashscope", "qwen",
    "doubao", "volcengine", "zhipu", "zai", "glm", "minimax", "xai",
    "openai", "gemini",
  ];
  function providerLabelFromErrorText(raw, language) {
    const text = String(raw || "");
    const lower = text.toLowerCase();
    const normalized = normalizeForMatch(text);
    // Same word-boundary matching as hasProviderNameSignal: a bare
    // includes() would let short keys like zai/xai/glm/kimi hit unrelated
    // substrings ("exhibit", "kaiming").
    for (let i = 0; i < PROVIDER_SIGNAL_ORDER.length; i++) {
      const key = PROVIDER_SIGNAL_ORDER[i];
      const re = keywordRegex(key);
      if (re.test(lower) || re.test(normalized)) return providerLabel(key, language) || key;
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
        content: true,
        unknown: true,
      };
      const kind = allowedKind[raw.kind] ? raw.kind : "unknown";
      return Object.assign({}, raw, {
        kind,
        title: redactTechnicalDetail(raw.title, language),
        message: redactTechnicalDetail(raw.message, language),
        retryable: raw.retryable === true,
        technicalDetail: redactTechnicalDetail(raw.technicalDetail || raw.technical_detail || raw.detail || "", language),
      });
    }
    const technicalDetail = redactTechnicalDetail(raw, language);
    const classified = classify(raw);
    // A provider signal in the error text wins over the label derived by
    // the caller from the *current* model config (on historical rebuilds
    // the current model differs from the one that failed).
    const provider = providerLabelFromErrorText(raw, language)
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
      content: ["contentTitle", "contentMessage", false],
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

  // Terminal and transient (recoverable) wordings share the same message
  // templates; the only difference is the {stop} placeholder: terminal
  // states "this reply has stopped", transient states "the system will
  // keep retrying". The earlier implementation rewrote wording via string
  // replace, which silently stopped working whenever copy changed — hence
  // the placeholder parameterization.
  function stopPhraseFor(language, terminal) {
    const lang = languageTag(language);
    if (lang === "en") return terminal ? "so this reply stopped. " : "鲜小助 will keep retrying this reply. ";
    if (lang === "ja") return terminal ? "この応答は停止しました。" : "鲜小助 は現在の応答を再試行しています。";
    return terminal ? "本次回复已停止。" : "系统会继续重试当前回复。";
  }

  function noticeText(userError) {
    if (!userError) return "";
    // Chat bubbles are pill-styled without whitespace-pre-wrap, so "\n"
    // collapses to a space in HTML and title/message would run together;
    // use a single-line separator instead.
    return "⚠️ " + userError.title + " — " + userError.message;
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
