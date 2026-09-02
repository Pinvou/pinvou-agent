(function () {
  "use strict";

  // 脱敏占位符按界面语言选择:技术详情在 en/ja 界面不得混入中文。
  const SENSITIVE_PLACEHOLDERS = {
    zh: "[敏感信息已隐藏]",
    en: "[redacted]",
    ja: "[秘匿済み]",
  };
  // 品牌标签按界面语言提供:中文品牌名(通义千问/智谱/豆包)只用于 zh,
  // en/ja 用拉丁名,避免 "The 通义千问 API quota ..." 这类中英混排。
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

  // 底座(CodeWhale)模型调用失败的固定错误前缀,出现即可接管,分两组:
  // ①SSE 流式链路(chat.rs/stream_entry.rs):SSE stream request failed /
  //   idle timeout / headers timed out / buffer exceeded、Stream read error、
  //   Failed to call ... Chat API;
  // ②LlmError Display 引导词(llm_client/mod.rs,传输层立即失败——DNS/连接
  //   拒绝/TLS——不带 SSE 前缀直接上抛):经 llm_client 独产、与本地工具
  //   错误文案无碰撞,且冒号/括号锚定("rate limit exceeded:" 不会命中
  //   gh CLI 的 "API rate limit exceeded for ...")。
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

  // 泛化信号词分三档:①无条件词——模型 API 计费/额度词与 "invalid api key"
  // ("incorrect api key" 是 OpenAI 真实措辞)等,本地工具错误几乎不会出现;
  // ②带上下文词——泛中文支付语("账户余额/余额不足/欠费"在本地支付/转账
  // 错误里同样常见,须叠加 API/厂商上下文才接管);
  // ③网络/服务/超时词(timeout、connection refused、server error 等)在本地
  // 工具错误(git、ssh、npm、docker、脚本退出码)里同样常见,必须叠加
  // API/厂商上下文(hasApiSignal/hasProviderNameSignal)才允许接管。
  const STRONG_MODEL_ERROR_KEYWORDS = [
    "invalid api key",
    "incorrect api key",
    "insufficient quota",
    "quota exceeded",
    "quota exhausted",
    "quota has been exceeded",
    "exceeded your current quota",
    "payment required",
    "额度不足",
    "额度用尽",
    "额度耗尽",
    "用量超出",
    "resource exhausted",
  ];

  // 泛中文支付语:语义不足以单独接管(本地支付/转账错误同款措辞),
  // 但叠加 API/厂商上下文后是强计费信号(如 "GLM 400 余额不足")。
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
    // 注意:"api key" 不在此列——它是 hasApiSignal 的上下文名词而非错误语义,
    // 留在词表里会让门控形同虚设("failed to save api key to config: disk full"
    // 这类本地错误被劫持)。强语义的 "invalid api key" 由 STRONG 词表接管。
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
        networkMessage: "无法连接到当前模型服务。{stop}请检查网络、代理或服务地址后重试。",
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
        rateMessage: "The current model service is receiving too many requests, {stop}Try again later or switch to another model.",
        authTitle: p + " API key is invalid",
        authMessage: "The API key for the current model service is invalid or expired. Check it in model settings.",
        permissionTitle: p + " access is not allowed",
        permissionMessage: "The current API key does not have access to this model service. Check account permissions or switch models.",
        serverTitle: p + " is temporarily unavailable",
        serverMessage: "The current model service is temporarily unavailable, {stop}Try again later or switch to another model.",
        networkTitle: "Network connection failed",
        networkMessage: "Pinvou could not connect to the current model service. {stop}Check network, proxy, or endpoint settings and retry.",
        contextTitle: "Context is too long",
        contextMessage: "This conversation is longer than the model can handle. Compact context, reduce input, or start a new session.",
        unknownTitle: "Current model service is unavailable",
        unknownMessage: "The current model service returned an error, {stop}Try again later or switch to another model in settings.",
      },
      ja: {
        billingTitle: p + " のアカウント残高が不足しています",
        billingMessage: "現在使用している " + p + " API アカウントの残高が不足しているため、{stop}プロバイダー側でチャージするか、モデル設定で別のモデルに切り替えてください。",
        quotaTitle: p + " API の割り当てが不足しています",
        quotaMessage: "現在使用している " + p + " API の割り当てが不足しているため、{stop}割り当てを確認するか、別のモデルに切り替えてください。",
        rateTitle: p + " のリクエストが多すぎます",
        rateMessage: "現在のモデルサービスへのリクエストが多すぎます。{stop}しばらくしてから再試行するか、別のモデルに切り替えてください。",
        authTitle: p + " API Key が無効です",
        authMessage: "現在のモデルサービスの API Key が無効、または期限切れです。モデル設定で確認して再入力してください。",
        permissionTitle: p + " にアクセスできません",
        permissionMessage: "現在の API Key にはこのモデルサービスへのアクセス権がありません。アカウント権限を確認するか、別のモデルに切り替えてください。",
        serverTitle: p + " は一時的に利用できません",
        serverMessage: "現在のモデルサービスは一時的に利用できません。{stop}しばらくしてから再試行するか、別のモデルに切り替えてください。",
        networkTitle: "ネットワーク接続に失敗しました",
        networkMessage: "現在のモデルサービスに接続できません。{stop}ネットワーク、プロキシ、またはエンドポイント設定を確認して再試行してください。",
        contextTitle: "コンテキストが長すぎます",
        contextMessage: "この会話はモデルが処理できる範囲を超えています。コンテキストを圧縮する、入力を減らす、または新しい会話で再試行してください。",
        unknownTitle: "現在のモデルサービスを利用できません",
        unknownMessage: "現在のモデルサービスでエラーが発生しました。{stop}しばらくしてから再試行するか、別のモデルに切り替えてください。",
      },
    };
    return copy[lang][key];
  }

  function extractHttpStatus(text) {
    // HTTP/1.1 429、HTTP/2 503(带版本段)与 "status code 429"(axios)同属常见形态。
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

  // 关键词匹配带词边界:裸 includes 会让 "chat api" 命中 "chat apiary"、
  // "api key" 命中 "api-keys.yaml"(归一化后 "api keys"),把本地工具错误
  // 误判成模型服务故障。正则按词编译并缓存(分类在每条错误上高频调用)。
  const KEYWORD_REGEX_CACHE = new Map();
  function keywordRegex(word) {
    let re = KEYWORD_REGEX_CACHE.get(word);
    if (!re) {
      const escaped = String(word).replaceAll(/[.*+?^${}()|[\]\\]/g, String.raw`\$&`);
      // 尾部词边界只对字母/数字结尾的关键词有意义:以 "(" 或空格收尾的
      // 锚定形态(如 "server error ("/"request timed out after ")本身已
      // 与后续字符隔离,加前瞻反而会拒绝紧随的数字(状态码/时长)。
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

  // 本地 CLI 工具的固定错误形态,先于语义词表排除:git 的 "fatal:" 行与
  // ssh/curl 的 "connect to host … port N" 在 remote URL/主机名里带厂商名时
  // (github.com/openai、git.openai.com)叠加网络歧义词,会被误判成模型服务
  // 故障并劫持成"请重试"卡。底座模型错误恒带 MODEL_CALL_PREFIXES 并已在
  // 上方接管,不会落到这里,故排除不伤及真实模型服务错误。
  const LOCAL_TOOL_SHAPES = [
    /(?:^|[\s(@])fatal(?: error)?:/i,
    /(?:connect to host|failed to connect to)\s+\S+\s+port\s+\d+/i,
    /failed to push some refs/i,
  ];

  // 分类器只接管两类错误:①底座模型调用的固定前缀(见 MODEL_CALL_PREFIXES);
  // ②模型服务语义词,且必须叠加 API/厂商上下文——裸的 timeout/connection
  // refused/server error 等词在本地工具错误(git、ssh、npm、docker)里同样
  // 常见,不能仅凭它们断言"模型服务故障"。
  const MEMORY_EXHAUSTION_RE = /(?:^|[^a-z0-9])(?:out\s+of\s+memory|oom|内存(?:耗尽|不足|溢出))(?![a-z0-9])/i;

  function isModelServiceError(raw) {
    if (raw && typeof raw === "object" && raw.kind && raw.title && raw.message) return true;
    const text = String(raw || "");
    const lower = text.toLowerCase();
    const normalized = normalizeForMatch(text);
    if (hasAny(lower, normalized, MODEL_CALL_PREFIXES)) return true;
    // 本地 CLI 工具形态(见 LOCAL_TOOL_SHAPES)先于全部语义词表排除。
    if (LOCAL_TOOL_SHAPES.some(function (re) { return re.test(text); })) return false;
    // POSIX 磁盘/卷配额满(EDQUOT 的标准 strerror 就是 "Disk quota exceeded",
    // 卷挂载报 "user quota exceeded")先于计费强词排除,否则本地写盘失败会被
    // 提示"请充值或切换模型"。
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
    // 泛中文支付语须叠加 API/厂商上下文:"支付失败:账户余额不足"这类本地
    // 支付错误不得引导用户去充值模型 API;带上下文的("GLM 400 余额不足")
    // 仍是强计费信号。
    if ((apiSignal || providerSignal) && hasAny(lower, normalized, STRONG_WITH_CONTEXT_KEYWORDS)) return true;
    const status = extractHttpStatus(text);
    // extractHttpStatus 只在文本含 HTTP/status 字样时才返回非空,无需再验上下文词。
    const statusIsModelLike = status !== null
      && (status === 401 || status === 402 || status === 403 || status === 429 || (status >= 500 && status <= 599));
    // apiSignal 与 providerSignal 走同一规则:必须叠加歧义错误词或 model-like
    // 状态码。裸的 "api key"/"model service" 字样只是上下文名词而非错误语义,
    // 无叠加条件时接管会把 "failed to save api key to config: disk full" 这类
    // 本地错误劫持成模型服务错误卡;取舍是错过 "api key 配置有误" 这类无错误
    // 动词的文本,但 STRONG 词表("invalid api key" 等)仍保证强语义接管。
    if ((apiSignal || providerSignal)
        && (statusIsModelLike || hasAny(lower, normalized, AMBIGUOUS_MODEL_ERROR_KEYWORDS))) return true;
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
    if (status === 401 || hasAny(lower, normalized, ["unauthorized", "authentication", "authorization failed", "invalid api key", "incorrect api key", "invalid key", "invalid token", "bearer token"])) {
      return { kind: "auth", httpStatus: status };
    }
    if (status === 402 || hasAny(lower, normalized, ["payment required", "insufficient balance", "余额不足", "欠费", "账户余额"])) {
      return { kind: "billing", httpStatus: status };
    }
    if (hasAny(lower, normalized, ["quota exceeded", "insufficient quota", "quota exhausted", "quota has been exceeded", "exceeded your current quota", "额度不足", "额度用尽", "额度耗尽", "用量超出"])
      || (hasAny(lower, normalized, ["resource exhausted"]) && !MEMORY_EXHAUSTION_RE.test(normalized))) {
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

  function redactTechnicalDetail(raw, language) {
    const sensitive = SENSITIVE_PLACEHOLDERS[languageTag(language)];
    let text = String(raw || "");
    const keepPrefix = (prefix) => prefix + sensitive;
    // Authorization/Proxy-Authorization 整段吞值:任意 scheme(scheme 词表
    // 枚举追不完,API-Key/HMAC 等非标 scheme 曾整体漏掉凭证段)、可选 JSON
    // 引号;值一段吞到首个分隔符(引号/逗号/分号/括号/&/换行),允许值内
    // 空格以覆盖 "Digest username=x, ..." 之外的两段式凭证。
    // Digest parameter blobs (username="x", realm=y) are swallowed whole
    // before the generic value: the generic class stops at the first quote
    // and would leak the comma-separated params. A single-pass alternation
    // also keeps the generic branch from re-masking the Digest placeholder.
    text = text.replaceAll(
      /((?:proxy-)?authorization[\s"']*[:=][\s"']*)(?:digest\s+[^;\r\n]+|[^"'),;}&\][\r\n]+)/gi,
      (m, p1) => keepPrefix(p1),
    );
    // Cookie/Set-Cookie 头整段吞值:多对凭证以 "; " 分隔,下方 kv 规则的值
    // 字符类在首个空格处截断,只能吞第一对(SID 吞掉、HSID/SSID 原样泄漏)。
    // HTTP 头一行一对,按头名吞到行尾不越界;前缀限 [空格/引号/行首/JSON
    // 分隔符],把 "document.cookie = …" 之外的普通说明词挡在外面。
    text = text.replaceAll(
      /((?:^|[\s"',;[])(?:set[\s_-]?cookie|cookie)[\s"']*[:=][\s"']*)[^\r\n]*/gi,
      (m, p1) => keepPrefix(p1),
    );
    // 裸 Bearer <token>(无 Authorization 头名,配置回显/排错日志常见形态)。
    text = text.replaceAll(/\b(Bearer\s+)[a-z0-9._~+=/-]{12,}/gi, (m, p1) => keepPrefix(p1));
    // Bare Basic/Digest <base64> (same shapes without a header name); the
    // threshold of 12 covers the shortest "user:pass" form (dXNlcjpwYXNz is
    // exactly 12 chars, which the previous 16-char threshold let through)
    // at the cost of occasionally masking a plain word after "basic".
    text = text.replaceAll(/\b((?:Basic|Digest)\s+)[a-z0-9+/=]{12,}/gi, (m, p1) => keepPrefix(p1));
    // 强凭证键(password/passphrase/secret)两种形态都整段吞值:
    // ①引号值允许空格("correct horse battery staple" 这类多词口令);
    // ②未加引号时吞到行尾或首个 ,/;/&,否则多词口令只吞首词、其余词泄漏。
    text = text.replaceAll(
      /(["']?\b(?:password|passphrase|secret)\b["']?\s*[:=]\s*["'])([^"']{2,})(["'])/gi,
      (m, p1, p2, p3) => p1 + sensitive + p3,
    );
    text = text.replaceAll(
      /(["']?\b(?:password|passphrase|secret)\b["']?\s*[:=]\s*)([^"'\r\n,;&]{2,})/gi,
      (m, p1) => keepPrefix(p1),
    );
    // kv 形态的凭证键:左侧词边界 + 键名白名单(含 refresh_token/client_secret
    // 等复合名——\b 在下划线旁不成立,必须整体枚举;cookie/set-cookie 头整段
    // 按凭证处理)。值取「字母开头」或「≥10 位、可含 -/_/./~ 的字母数字」
    // (数字开头或带连字符的会话凭证,如 8f3k9d2l-4abc-… / 1234-5678-…);
    // "token: 15000" 用量计数是纯数字且不足 10 位,不命中,context 错误的
    // 排查信息得以保留。
    /* eslint-disable sonarjs/regex-complexity, sonarjs/duplicates-in-character-class -- the credential-key whitelist is deliberately exhaustive; splitting it would reduce auditability */
    text = text.replaceAll(
      /(["']?\b(?:api[_-]?key|api[_-]?secret|api[_-]?token|authorization|token|password|secret|access[_-]?key|access[_-]?token|refresh[_-]?token|id[_-]?token|auth[_-]?token|client[_-]?secret|app[_-]?secret|consumer[_-]?key|secret[_-]?key|secret[_-]?token|ssh[_-]?key|private[_-]?key|session[_-]?id|session[_-]?token|sessionid|jsessionid|cookie|set[_-]?cookie)\b["']?\s*[:=]\s*["']?)(?:[A-Za-z][^"',\s&}]*|[A-Za-z0-9][A-Za-z0-9._~-]{9,})/gi,
      (m, p1) => keepPrefix(p1),
    );
    /* eslint-enable sonarjs/regex-complexity, sonarjs/duplicates-in-character-class */
    // 裸 key 键另行收紧(值要求字母开头且含数字):"key": "model-name"
    // 这类非凭证值不再被误吞,而 "key": "abc123def456" 仍被吞掉。
    text = text.replaceAll(
      /(["']?\bkey\b["']?\s*[:=]\s*["']?)(?=[a-z][^"',\s&}]*\d)[a-z][^"',\s&}]+/gi,
      (m, p1) => keepPrefix(p1),
    );
    // URL 查询参数形态(query 里的值几乎必然是凭证,不加形态条件)。
    /* eslint-disable sonarjs/regex-complexity -- same whitelist rationale as above */
    text = text.replaceAll(
      /([?&](?:api[_-]?key|api[_-]?secret|api[_-]?token|key|authorization|token|password|secret|access[_-]?key|access[_-]?token|refresh[_-]?token|id[_-]?token|auth[_-]?token|client[_-]?secret|app[_-]?secret|consumer[_-]?key|ssh[_-]?key|session[_-]?id|sessionid)=)[^&#\s]+/gi,
      (m, p1) => keepPrefix(p1),
    );
    /* eslint-enable sonarjs/regex-complexity */
    text = text.replaceAll(/\bsk-[A-Za-z0-9][A-Za-z0-9._-]{10,}\b/g, () => sensitive);
    // 裸 Gemini API Key(AIza 前缀 + 35 位左右,无键名/配置回显常见形态)。
    text = text.replaceAll(/\bAIza[0-9A-Za-z_-]{30,}/g, () => sensitive);
    // 裸 JWT/JWS(eyJ 开头三段式,无键名/无 Bearer 前缀的形态)。
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
    "deepseek", "anthropic", "claude", "moonshot", "kimi", "dashscope", "qwen",
    "doubao", "volcengine", "zhipu", "zai", "glm", "minimax", "xai",
    "openai", "gemini",
  ];
  function providerLabelFromErrorText(raw, language) {
    const text = String(raw || "");
    const lower = text.toLowerCase();
    const normalized = normalizeForMatch(text);
    // 与 hasProviderNameSignal 同款词边界匹配:裸 includes 会让 zai/xai/glm/
    // kimi 这类短键被无关子串(如 "exhibit"/"kaiming")误命中。
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
    // 错误文本里的 provider 信号优先于调用方从"当前"模型配置推导的标签
    // (历史回合重建时当前模型≠出错回合的模型)。
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
    // 聊天气泡是 pill 样式、无 whitespace-pre-wrap,"\n" 会被 HTML 折叠成
    // 空格,标题与消息连成一行;改用单行分隔符。
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
