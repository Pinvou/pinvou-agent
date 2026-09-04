/**
 * voice feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting the statement would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["voice"] = function (context) {
    const state = context.state;
    const notify = context.notify;
    const invoke = context.invoke;
    const bt = context.bt;
  // ── 语音输入（WebView one-shot 录音 → 本地 SenseVoice/FunASR ASR；Linux webview 录音授权见 lib.rs setup）──────────────
  let activeVoiceInput = null;
  const VOICE_DEVICE_PROBE_TIMEOUT_MS = 1500;
  const VOICE_DEVICE_REQUEST_TIMEOUT_MS = 8000;

  const VOICE_RECORDING_MAX_DURATION_MS = 60000;

  function normalizeVoiceMode(mode) {
    if (mode === "task") return "task";
    if (["edit", "voice_edit", "draft_edit"].includes(mode)) return "edit";
    return "dictation";
  }

  function voiceNow() {
    return (typeof performance !== "undefined" && typeof performance.now === "function")
      ? performance.now()
      : Date.now();
  }

  // 智能整理开关,默认开启;key 与 features/chat/voice-shortcut-settings.mjs
  // 保持一致(经典脚本无法 import 该模块),改 key 时必须两侧同步。
  function isVoicePostprocessEnabled() {
    try {
      return !localStorage || localStorage.getItem("pinvou_voice_postprocess_enabled_v1") !== "false";
    } catch {
      return true;
    }
  }

  function roundedMs(start, end) {
    return Math.max(0, Math.round((end || voiceNow()) - start));
  }

  const VOICE_FILLER_TERMS = ["嗯", "啊", "呃", "那个", "就是"];
  const VOICE_SUSPICIOUS_ASR_TERMS = [
    "进价", "惊吓", "图标", "销售暑假", "屁屁提", "PPTT", "截止事件",
    "负责任", "风险电", "三百字一类", "爱新闻", "代码嫩力", "核心公能",
    "GP杠5", "GPT杠5", "closonic", "克劳德", "deeps V3", "deep seek v three",
    "批地爱福", "pDF", "合同金鹅", "付款结点", "违约条宽", "表哥", "亲自酒店",
    "离海绵距离", "四零一", "talken", "过期处里", "产品民", "pin vo",
    "rest a p i", "认正", "错误马", "示例情求", "语音输出", "模型下崽体验",
    "知识裤", "报消", "住宿上线", "班纳", "温婉简洁", "高贴票"
  ];
  const VOICE_PROTECTED_TERMS = [
    "金价", "图表", "表格", "PPT", "GPT-5", "Claude Sonnet", "DeepSeek V3",
    "AI 新闻", "PDF", "Pinvou", "REST API", "401", "token", "高铁票",
    "负责人", "截止时间", "预算", "部门", "超支项", "付款风险", "交付风险",
    "客服投诉", "产品线", "高频问题", "语音输入", "模型下载体验", "知识库",
    "差旅报销标准", "住宿上限", "banner", "温暖简洁"
  ];
  const VOICE_TASK_WORDS = [
    "查", "搜索", "生成", "整理", "总结", "比较", "做一个", "做成", "列出",
    "写到", "发送", "创建", "基于", "报告", "PPT", "网页", "图表", "表格"
  ];

  function compactVoiceText(text) {
    return String(text || "")
      .replaceAll(/[\s。！？!?，,、；;：:"'“”‘’（）()【】\][.…—-]/g, "")
      .trim();
  }

  function voiceContainsAny(text, terms) {
    const value = String(text || "");
    return terms.filter(function (term) { return value.includes(term); });
  }

  function isFillerOnlyVoiceText(text) {
    const compact = compactVoiceText(text);
    if (!compact) return true;
    if (compact.length > 8) return false;
    if (compact === "文") return true;
    let remaining = compact;
    VOICE_FILLER_TERMS.forEach(function (term) {
      remaining = remaining.split(term).join("");
    });
    remaining = remaining.replaceAll('额', '');
    remaining = remaining.replaceAll('文', '');
    return remaining.trim() === "";
  }

  function isShortClearVoiceText(text, mode) {
    if (normalizeVoiceMode(mode) !== "dictation") return false;
    const compact = compactVoiceText(text);
    if (!compact || compact.length > 18) return false;
    if (voiceContainsAny(text, VOICE_FILLER_TERMS).length) return false;
    if (voiceContainsAny(text, VOICE_SUSPICIOUS_ASR_TERMS).length) return false;
    if (voiceContainsAny(text, VOICE_TASK_WORDS).length) return false;
    return true;
  }

  function classifyVoiceText(rawText, mode) {
    const normalizedMode = normalizeVoiceMode(mode);
    const text = String(rawText || "").trim();
    const suspicious = voiceContainsAny(text, VOICE_SUSPICIOUS_ASR_TERMS);
    if (!text || isFillerOnlyVoiceText(text)) {
      return {
        strategy: "skip_empty",
        reason: "filler_only",
        suspicious_terms: suspicious,
      };
    }
    if (normalizedMode === "edit") {
      return {
        strategy: "run_llm",
        reason: normalizedMode + "_mode",
        suspicious_terms: suspicious,
      };
    }
    if (normalizedMode === "task") {
      return {
        strategy: "run_llm",
        reason: suspicious.length ? "task_suspicious_asr" : "task_long_or_noisy",
        suspicious_terms: suspicious,
      };
    }
    if (isShortClearVoiceText(text, normalizedMode)) {
      return {
        strategy: "use_asr",
        reason: "dictation_short_clear",
        suspicious_terms: suspicious,
      };
    }
    return {
      strategy: "run_llm",
      reason: suspicious.length ? "suspicious_asr" : "dictation_llm",
      suspicious_terms: suspicious,
    };
  }

  function voicePostprocessTimeoutMs(mode, rawText) {
    const normalizedMode = normalizeVoiceMode(mode);
    if (normalizedMode === "task") return 8000;
    if (normalizedMode === "edit") return 12000;
    return compactVoiceText(rawText).length <= 18 ? 3000 : 5000;
  }

  function hasVoiceHighRiskResidual(text) {
    return voiceContainsAny(text, VOICE_SUSPICIOUS_ASR_TERMS);
  }

  function applyVoiceDeterministicCorrections(text, rawText) {
    let value = String(text || "");
    const raw = String(rawText || "");
    if (isFillerOnlyVoiceText(value)) return "";
    if (!value.trim()) return value;
    value = value
      .replaceAll(/^[嗯啊呃额][，,、\s]+/g, "")
      // 进价→金价只覆盖行情查询语境；「今天的进价比昨天低」这类真实比价输入不得误纠。
      .replaceAll(/今日进价(?![比高低更涨跌贵])/g, "今日金价")
      .replaceAll(/今天的进价(?![比高低更涨跌贵])/g, "今天的金价")
      .replaceAll('数据分析图标', "数据分析图表")
      .replaceAll('核心公能', "核心功能")
      .replaceAll('销售暑假', "销售数据")
      .replaceAll(/屁屁提|PPTT/g, "PPT")
      .replaceAll('截止事件', "截止时间")
      // 「负责任」多为形容词（他很负责任/负责任的老师），仅在不带程度副词前缀、不接「的」时
      // 才按名词误识别纠正为「负责人」。
      .replaceAll('负责任', function (match, offset, whole) {
        const prev = whole.charAt(offset - 1);
        const next = whole.charAt(offset + match.length);
        if ("很不真更最挺太".includes(prev) || next === "的") return match;
        return "负责人";
      })
      .replaceAll('风险电', "风险点")
      .replaceAll('三百字一类', "三百字以内")
      .replaceAll('代码嫩力', "代码能力")
      .replaceAll(/g\s*p\s*t\s*five/gi, "GPT-5")
      .replaceAll(/G\s*P\s*(?:T\s*)?杠\s*5/gi, "GPT-5")
      .replaceAll(/克劳德\s*sonnet|closonic/gi, "Claude Sonnet")
      .replaceAll(/deep\s*seek\s*v\s*three|deeps\s*V3/gi, "DeepSeek V3")
      .replaceAll(/爱新闻(?!联播)|AI新闻/g, "AI 新闻")
      .replaceAll(/批地爱福|pDF/g, "PDF")
      .replaceAll('合同金鹅', "合同金额")
      .replaceAll('付款结点', "付款节点")
      .replaceAll('违约条宽', "违约条款")
      .replaceAll('表哥', "表格")
      .replaceAll('本月玉算', "本月预算")
      .replaceAll('不门', "部门")
      .replaceAll('超时项', "超支项")
      .replaceAll('亲自酒店', "亲子酒店")
      .replaceAll('离海绵距离', "离海边距离")
      .replaceAll('四零一', "401")
      .replaceAll(/talken/gi, "token")
      .replaceAll('过期处里', "过期处理")
      // eslint-disable-next-line sonarjs/duplicates-in-character-class -- single lookahead guard per branch; sonarjs miscounts the escaped-class dupes here
      .replaceAll(/产品民\s*pin\s+vo\b|产品名con(?![a-zA-Z])|\bpin\s+vo\b/gi, "产品名 Pinvou")
      .replaceAll(/rest\s*a\s*p\s*i/gi, "REST API")
      .replaceAll('认正', "认证")
      .replaceAll(/错误马(?!上)/g, "错误码")
      .replaceAll('示例情求', "示例请求")
      .replaceAll('高贴票', "高铁票")
      .replaceAll('副款风险', "付款风险")
      // 「交互风险」本身是合法表述（交互设计风险），确定性规则无法区分，不在规则层纠正，
      // 交由 LLM 对照原始 ASR 判断。
      .replaceAll('各列三跳', "各列三条")
      .replaceAll('客诉投诉', "客服投诉")
      .replaceAll(/产品先(?![上发做进推试跑])/g, "产品线")
      .replaceAll('高频问提', "高频问题")
      .replaceAll('重点推近', "重点推进")
      .replaceAll(/语音输出(?!的)/g, "语音输入")
      .replaceAll('模型下崽体验', "模型下载体验")
      .replaceAll('知识裤', "知识库")
      // 「报消」常是跨词误命中（上报|消费者、预报|消息），前缀为上/预/补/申时不纠正。
      .replaceAll('报消', function (match, offset, whole) {
        if ("上预补申".includes(whole.charAt(offset - 1))) return match;
        return "报销";
      })
      .replaceAll('住宿上线', "住宿上限")
      .replaceAll('中秋活动班纳', "中秋活动 banner")
      .replaceAll('温婉简洁', "温暖简洁")
      .replaceAll(/有长方形[，,、\s]*的需要/g, "是长方形，需要")
      .replaceAll(/长方形[，,、\s]*的需要/g, "长方形，需要")
      .replaceAll('联网下的图片', "联网下载的图片")
      .replaceAll('联网下图片', "联网下载图片");
    value = value
      .replaceAll('GPT-5和', "GPT-5 和")
      .replaceAll('和Claude Sonnet', "和 Claude Sonnet");
    if (raw.includes("图标") && value.includes("数据分析") && !value.includes("图表")) {
      value = value.replace(/数据分析$/, "数据分析图表");
    }
    if (raw.includes("只是发了")) {
      value = value
        .replaceAll(/^嗯[，,、\s]*/g, "")
        .replaceAll(/只是发了一个图表吧[。.]?/g, "图表。")
        .replaceAll(/生成数据分析[，,、\s]*图表/g, "生成数据分析图表")
        .replaceAll(/生成数据分析图表吧[。.]?/g, "生成数据分析图表。");
    }
    return value;
  }

  function voiceProtectedTermsIn(text) {
    return VOICE_PROTECTED_TERMS.filter(function (term) {
      return String(text || "").includes(term);
    });
  }

  function validateVoicePostprocessOutput(rawText, ruleText, finalText, mode) {
    const raw = String(rawText || "");
    const corrected = String(ruleText || "");
    const finalValue = String(finalText || "");
    const rawCompact = compactVoiceText(raw);
    const correctedCompact = compactVoiceText(corrected);
    const finalCompact = compactVoiceText(finalValue);
    if (!finalCompact) return isFillerOnlyVoiceText(raw) || isFillerOnlyVoiceText(corrected);
    if (rawCompact.length > 12 && finalCompact.length < Math.floor(correctedCompact.length * 0.55)) {
      return false;
    }
    const protectedTerms = voiceProtectedTermsIn(corrected);
    // LLM 把规则误纠恢复为原始 ASR 用词时（如「表哥」被规则改成「表格」又被 LLM 改回），
    // 最终文本天然缺少规则产物。以原始 ASR 为基线对最终文本重放同一套确定性规则再校验：
    // 语义等价的恢复被放行，真正丢掉术语的输出仍会拒绝。
    const finalChecked = applyVoiceDeterministicCorrections(finalValue, raw);
    const missingProtected = protectedTerms.filter(function (term) {
      return !finalChecked.includes(term);
    });
    if (missingProtected.length) return false;
    const normalizedMode = normalizeVoiceMode(mode);
    if (normalizedMode === "edit") {
      return finalValue.trim().length > 0;
    }
    if (normalizedMode === "task" && hasVoiceHighRiskResidual(finalValue).length) {
      return false;
    }
    return true;
  }

  function logVoicePipeline(diagnostic) {
    try {
      console.info("[voice-input] pipeline", diagnostic);
    } catch { /* console unavailable in some webviews */ }
    try {
      const key = "pinvou_voice_pipeline_diagnostics";
      let current = JSON.parse(localStorage.getItem(key) || "[]");
      if (!Array.isArray(current)) current = [];
      current.push(Object.assign({ recorded_at: new Date().toISOString() }, diagnostic));
      localStorage.setItem(key, JSON.stringify(current.slice(-50)));
    } catch { /* diagnostics persistence is best-effort */ }
    try {
      window.dispatchEvent(new CustomEvent("pinvou:voice-pipeline-diagnostic", { detail: diagnostic }));
    } catch { /* event dispatch is best-effort */ }
  }

  function setVoiceInputStatus(status, patch) {
    const next = Object.assign({}, state.voiceInput, patch || {});
    next.status = status;
    if (status !== "failed") {
      next.error = null;
      next.category = null;
    }
    state.voiceInput = next;
    notify();
  }

  function emitVoiceDiagnostic(stage, level, message, userMessage, category) {
    const event = {
      stage,
      level,
      message,
      user_message: userMessage || "",
      category: category || "",
    };
    const fn = level === "error" ? console.error : level === "warn" ? console.warn : console.info;
    fn.call(console, "[voice-input]", event);
  }

  // Rust 侧 VoiceCommandError 的稳定错误码 → 桥内三语文案 key。错误码优先于
  // rawMessage:Rust 的 message 是中文工程原文,只应进日志/诊断,不应直通
  // en/ja 用户界面(评审遗留:约 12 条中文文案直通)。
  const VOICE_ERROR_CODE_KEYS = {
    asr_timeout: "voiceTimeout",
    asr_no_speech: "voiceEmptyResult",
    asr_engine_missing: "voiceRecognitionFailed",
    asr_engine_start_failed: "voiceRecognitionFailed",
    asr_engine_error: "voiceRecognitionFailed",
    asr_runtime_error: "voiceRecognitionFailed",
    asr_cli_failed: "voiceRecognitionFailed",
    asr_parse_failed: "voiceRecognitionFailed",
    asr_join_failed: "voiceInputFailed",
    recording_too_long: "voiceRecordingTooLong",
    audio_invalid: "voiceAudioInvalid",
    audio_empty: "voiceAudioInvalid",
    temp_file_unavailable: "voiceInputFailed",
    temp_file_write_failed: "voiceInputFailed",
    session_mismatch: "voiceContextMismatch",
    session_load_failed: "voiceContextMismatch",
  };

  function normalizeVoiceError(err, fallbackStage) {
    const name = String((err && err.name) || "");
    const rawCategory = (err && err.category) || "";
    const rawStage = (err && err.stage) || fallbackStage || "recording";
    const rawMessage = String((err && (err.message || err.toString && err.toString())) || err || "");
    const constraint = String((err && err.constraint) || "");
    const codeKey = (err && err.code && VOICE_ERROR_CODE_KEYS[err.code]) || "";
    const emptyResultLike = /ASR empty result|empty result|backend returned no usable|no usable result|failed \(exit 6\)|exit 6/i.test(rawMessage);
    if (name === "NotAllowedError" || name === "SecurityError" || rawCategory === "permission_denied") {
      return { category: "permission_denied", stage: "permission", message: bt("voicePermissionDenied") };
    }
    if (rawCategory === "device_unavailable") {
      return { category: "device_unavailable", stage: "device", message: rawMessage || bt("voiceNoDevice") };
    }
    if (name === "NotFoundError" || name === "DevicesNotFoundError") {
      return { category: "device_unavailable", stage: "device", message: bt("voiceNoDevice") };
    }
    // Chrome 把"麦克风被其他应用占用"报为 NotReadableError / TrackStartError,
    // 与"没有设备"不同;浏览器原文是英文,映射成三语专用文案。
    if (name === "NotReadableError" || name === "TrackStartError") {
      return { category: "device_unavailable", stage: "device", message: bt("voiceMicUnavailable") };
    }
    // WebKitGTK 可能把不支持的音频约束报为 OverconstrainedError / "Invalid constraint"。
    // 这和没有录音设备不同：设备可能存在，只是不支持 channelCount、降噪等配置。
    if (name === "OverconstrainedError" || name === "ConstraintNotSatisfiedError" || /invalid constraint/i.test(rawMessage)) {
      return {
        category: "constraint_unsupported",
        stage: "device",
        message: bt("voiceConstraintUnsupported"),
        diagnostic: constraint ? "unsupported media constraint: " + constraint : "unsupported media constraint",
      };
    }
    if (rawCategory === "empty_result" || emptyResultLike) {
      return {
        category: "empty_result",
        stage: rawStage,
        message: bt("voiceEmptyResult"),
        diagnostic: rawMessage || "",
      };
    }
    if (rawCategory === "context_mismatch") {
      return { category: "context_mismatch", stage: "writeback", message: bt("voiceContextMismatch") };
    }
    if (rawCategory === "timeout") {
      return { category: "timeout", stage: "recording", message: bt("voiceTimeout") };
    }
    if (rawCategory === "recognition_failed") {
      // 有稳定错误码按码映射三语文案,中文原文降级为诊断;无码的历史错误才
      // 退回 rawMessage(仅中文工程原文,历史行为)。
      if (codeKey) {
        return { category: rawCategory, stage: rawStage, message: bt(codeKey), diagnostic: rawMessage };
      }
      return { category: rawCategory, stage: rawStage, message: rawMessage || bt("voiceRecognitionFailed") };
    }
    if (codeKey) {
      return { category: rawCategory || "recording_failed", stage: rawStage, message: bt(codeKey), diagnostic: rawMessage };
    }
    return {
      category: rawCategory || "recording_failed",
      stage: rawStage,
      message: rawMessage || bt("voiceInputFailed"),
    };
  }

  function stopMediaTracks(stream) {
    if (!stream) return;
    stream.getTracks().forEach(function (track) { try { track.stop(); } catch { /* already-stopped tracks need no handling */ } });
  }

  // error carrier for the voice flow: an Error instance with extra category/stage fields for normalizeVoiceError classification.
  // (the original threw a bare object literal, violating no-throw-literal; consolidated here into an Error factory,
  // keeping the semantics of normalizeVoiceError's category/stage/message classification fields unchanged.)
  function voiceFlowError(category, stage, message) {
    const error = new Error(message);
    error.category = category;
    error.stage = stage;
    return error;
  }

  function cleanupVoiceInputSession(session) {
    if (!session) return;
    if (session.timeoutId) clearTimeout(session.timeoutId);
    if (session.permissionTimeoutId) clearTimeout(session.permissionTimeoutId);
    session.permissionTimeoutId = null;
    if (session.cancelPermissionRequest) {
      const cancelPermissionRequest = session.cancelPermissionRequest;
      session.cancelPermissionRequest = null;
      try { cancelPermissionRequest(); } catch { /* a cancel-callback error must not block cleanup */ }
    }
    // 先摘掉音频回调：webkit2gtk 的 WebAudio 是 GStreamer 后端，ScriptProcessorNode 的
    // onaudioprocess 跑在音频线程，若在 disconnect/close 期间再触发一次、访问已释放的
    // 缓冲，会让 WebProcess 段错误（表现为「识别出文字后 app 崩溃」）。务必先置 null。
    try { if (session.processor) session.processor.onaudioprocess = null; } catch { /* release failure only affects this page's audio */ }
    try { if (session.processor) session.processor.disconnect(); } catch { /* release failure only affects this page's audio */ }
    try { if (session.source) session.source.disconnect(); } catch { /* release failure only affects this page's audio */ }
    try { if (session.zeroGain) session.zeroGain.disconnect(); } catch { /* release failure only affects this page's audio */ }
    stopMediaTracks(session.stream);
    session.processor = null;
    session.source = null;
    session.zeroGain = null;
    session.stream = null;
    // close() 触发 GStreamer 管线异步拆解，与上面的 disconnect/track.stop 在同一拍里竞争最易崩；
    // 摘干净节点后挪到下一个事件循环再关，并吞掉 close 的异常。
    const ctx = session.audioContext;
    session.audioContext = null;
    if (ctx && ctx.state !== "closed") {
      setTimeout(function () { try { ctx.close().catch(function () {}); } catch { /* audio context already closed */ } }, 0);
    }
  }

  async function probeVoiceAudioInput(timeoutMs) {
    if (!navigator.mediaDevices || typeof navigator.mediaDevices.enumerateDevices !== "function") return null;
    let timer = null;
    try {
      return await Promise.race([
        navigator.mediaDevices.enumerateDevices().then(function (devices) {
          return devices.some(function (device) { return device && device.kind === "audioinput"; });
        }),
        new Promise(function (resolve) {
          timer = setTimeout(function () { resolve(null); }, timeoutMs || VOICE_DEVICE_PROBE_TIMEOUT_MS);
        }),
      ]);
    } catch {
      return null;
    } finally {
      if (timer) clearTimeout(timer);
    }
  }

  function requestVoiceMedia(session, constraints, timeoutMs) {
    let abandoned = false;
    const mediaPromise = navigator.mediaDevices.getUserMedia(constraints).then(function (stream) {
      if (abandoned || activeVoiceInput !== session) {
        stopMediaTracks(stream);
        throw voiceFlowError("cancelled", "permission", bt("voiceCancelled"));
      }
      return stream;
    });
    const timeoutPromise = new Promise(function (_, reject) {
      session.permissionTimeoutId = setTimeout(function () {
        abandoned = true;
        reject(voiceFlowError("device_unavailable", "device", bt("voiceDeviceTimeout")));
      }, timeoutMs || VOICE_DEVICE_REQUEST_TIMEOUT_MS);
    });
    const cancelPromise = new Promise(function (_, reject) {
      session.cancelPermissionRequest = function () {
        abandoned = true;
        reject(voiceFlowError("cancelled", "permission", bt("voiceCancelled")));
      };
    });
    return Promise.race([mediaPromise, timeoutPromise, cancelPromise]).finally(function () {
      if (session.permissionTimeoutId) clearTimeout(session.permissionTimeoutId);
      session.permissionTimeoutId = null;
      session.cancelPermissionRequest = null;
    });
  }

  function mergeFloatChunks(chunks) {
    const total = chunks.reduce(function (sum, chunk) { return sum + chunk.length; }, 0);
    const out = new Float32Array(total);
    let offset = 0;
    chunks.forEach(function (chunk) {
      out.set(chunk, offset);
      offset += chunk.length;
    });
    return out;
  }

  function downsamplePcm(samples, sourceRate, targetRate) {
    if (!samples.length || sourceRate === targetRate) return samples;
    const ratio = sourceRate / targetRate;
    const len = Math.max(1, Math.round(samples.length / ratio));
    const out = new Float32Array(len);
    for (let i = 0; i < len; i++) {
      const start = Math.floor(i * ratio);
      const end = Math.min(samples.length, Math.floor((i + 1) * ratio));
      let sum = 0;
      let count = 0;
      for (let j = start; j < end; j++) { sum += samples[j]; count++; }
      out[i] = count ? sum / count : samples[Math.min(start, samples.length - 1)];
    }
    return out;
  }

  function encodeWav(samples, sampleRate) {
    const dataSize = samples.length * 2;
    const buffer = new ArrayBuffer(44 + dataSize);
    const view = new DataView(buffer);
    function writeString(offset, value) {
      // WAV header writes ASCII only; charCode is the target byte value, fromCodePoint/codePointAt add nothing here.
      for (let i = 0; i < value.length; i++) view.setUint8(offset + i, value.charCodeAt(i)); // eslint-disable-line unicorn/prefer-code-point
    }
    writeString(0, "RIFF");
    view.setUint32(4, 36 + dataSize, true);
    writeString(8, "WAVE");
    writeString(12, "fmt ");
    view.setUint32(16, 16, true);
    view.setUint16(20, 1, true);
    view.setUint16(22, 1, true);
    view.setUint32(24, sampleRate, true);
    view.setUint32(28, sampleRate * 2, true);
    view.setUint16(32, 2, true);
    view.setUint16(34, 16, true);
    writeString(36, "data");
    view.setUint32(40, dataSize, true);
    let offset = 44;
    for (let i = 0; i < samples.length; i++, offset += 2) {
      const s = Math.max(-1, Math.min(1, samples[i]));
      view.setInt16(offset, s < 0 ? s * 0x8000 : s * 0x7fff, true);
    }
    return buffer;
  }

  // 60s 16kHz 16bit WAV 约 1.9MB；按 JSON 数字数组跨 IPC 会产生约 192 万元素、多份拷贝，
  // 峰值内存 ~25MB。改为标准 base64 字符串（带 padding），与 web 车道 encodeBase64Bytes 同款分块编码。
  function encodeVoiceBase64Bytes(bytes) {
    let binary = "";
    const chunkSize = 0x8000;
    for (let offset = 0; offset < bytes.length; offset += chunkSize) {
      const chunk = bytes.subarray(offset, Math.min(offset + chunkSize, bytes.length));
      // chunk only holds 0-255 byte values; fromCharCode/fromCodePoint are equivalent. Keep the apply-chunked hot path.
      binary += String.fromCharCode.apply(null, chunk); // eslint-disable-line unicorn/prefer-code-point
    }
    return window.btoa(binary);
  }

  // 模型偶尔用 ``` 围栏包裹整段输出；只剥整包围栏，保留 dictation 合法的 Markdown 列表内容。
  function stripVoicePostprocessFences(text) {
    const value = String(text || "").trim();
    const fenced = value.match(/^```[^\n]*\r?\n([\s\S]*?)```\s*$/);
    return fenced ? fenced[1].trim() : value;
  }

  async function postprocessVoiceText(correctedText, mode, draftText, sessionId, timeoutMs, rawText) {
    const normalizedMode = normalizeVoiceMode(mode);
    const startedAt = voiceNow();
    let timer = null;
    try {
      const request = invoke("postprocess_voice_text", {
        request: {
          text: correctedText,
          // 原始 ASR 一并下发：模型需要对照原文才能撤销确定性规则的误纠（如「表哥」→「表格」）。
          // 旧后端结构体未加 deny_unknown_fields，会安全忽略该字段。
          raw_text: String(rawText || correctedText || ""),
          mode: normalizedMode,
          session_id: sessionId || null,
          draft_text: draftText || "",
        },
      });
      const res = await Promise.race([
        request,
        new Promise(function (_, reject) {
          timer = setTimeout(function () {
            reject(voiceFlowError("postprocess_timeout", "postprocess", "voice postprocess timed out after " + timeoutMs + "ms"));
          }, timeoutMs || voicePostprocessTimeoutMs(normalizedMode, correctedText));
        }),
      ]);
      // finish_reason=length 的截断输出不允许静默写回（可能截到 60-70% 绕过 55% 收缩红线），回退规则文本。
      if (res && res.truncated) {
        throw voiceFlowError("postprocess_truncated", "postprocess", "voice postprocess output truncated (finish_reason=length)");
      }
      const text = stripVoicePostprocessFences(res && res.text !== undefined ? res.text : "");
      return {
        text,
        source: (res && res.source) || "llm",
        durationMs: roundedMs(startedAt),
        timeoutMs: timeoutMs || voicePostprocessTimeoutMs(normalizedMode, correctedText),
        error: null,
        fallbackReason: "",
      };
    } catch (err) {
      console.warn("[voice-input] postprocess fallback to ASR text", err);
      return {
        text: correctedText,
        source: "asr_fallback",
        durationMs: roundedMs(startedAt),
        timeoutMs: timeoutMs || voicePostprocessTimeoutMs(normalizedMode, correctedText),
        error: String((err && err.message) || err || "voice postprocess failed"),
        fallbackReason: err && (err.category === "timeout" || err.category === "postprocess_timeout")
          ? "llm_timeout"
          : "llm_error",
      };
    } finally {
      if (timer) clearTimeout(timer);
    }
  }

  // eslint-disable-next-line sonarjs/cognitive-complexity -- single state machine covering record/transcribe/postprocess/edit lanes; split tracked separately
  async function finishVoiceInput(cancelled, timedOut) {
    const session = activeVoiceInput;
    if (!session) return;
    // 录音会话结束（取消/完成/出错统一收口于此），解除跨窗录音互斥的本窗占用。
    // 带 session token:迟到收尾不得抹掉新会话的登记。
    syncVoiceShortcutRecording(null, session.sessionId);
    if (cancelled) {
      cleanupVoiceInputSession(session);
      activeVoiceInput = null;
      setVoiceInputStatus("cancelled", { message: bt("voiceCancelled"), completedAt: Date.now() });
      emitVoiceDiagnostic("recording", "info", "voice input cancelled", "已取消语音输入", "cancelled");
      return;
    }

    setVoiceInputStatus("transcribing", { message: bt("voiceTranscribing"), stage: "transcribing" });
    cleanupVoiceInputSession(session);

    try {
      const pipelineStartedAt = voiceNow();
      if (timedOut) {
        emitVoiceDiagnostic("recording", "warn", "recording reached max duration", "", "timeout");
      }
      const raw = mergeFloatChunks(session.chunks);
      const durationMs = raw.length / Math.max(1, session.sampleRate) * 1000;
      if (durationMs < 300) {
        throw voiceFlowError("recording_failed", "recording", bt("voiceRecordingTooShort"));
      }
      const pcm = downsamplePcm(raw, session.sampleRate, 16000);
      const wav = encodeWav(pcm, 16000);
      const audioBase64 = encodeVoiceBase64Bytes(new Uint8Array(wav));
      const asrStartedAt = voiceNow();
      const res = await invoke("transcribe_voice_audio", {
        request: {
          audio_base64: audioBase64,
          session_id: session.sessionId,
        },
      });
      const asrDurationMs = roundedMs(asrStartedAt);
      if (activeVoiceInput !== session) return;
      const text = String((res && res.text) || "").trim();
      if (!text) throw voiceFlowError("empty_result", "transcribing", "未识别到语音内容");
      if (state.activeSessionId !== session.sessionId) {
        throw voiceFlowError("context_mismatch", "writeback", "voice result discarded because active session changed");
      }
      const mode = normalizeVoiceMode(session.mode);
      const ruleText = applyVoiceDeterministicCorrections(text, text);
      const rawSuspiciousTerms = voiceContainsAny(text, VOICE_SUSPICIOUS_ASR_TERMS);
      // 分类必须基于原始 ASR：若用纠错后文本分类，带确定性规则的 suspicious 词（负责任、
      // 表哥、语音输出…）在分类前已消失，短句 dictation 会走 use_asr 把误纠静默写回输入框。
      const strategy = classifyVoiceText(text, mode);
      let postprocessResult;
      if (strategy.strategy === "skip_empty") {
        postprocessResult = {
          text: "",
          source: "skipped",
          durationMs: 0,
          timeoutMs: 0,
          error: null,
          fallbackReason: "",
        };
      } else if (strategy.strategy === "use_asr") {
        postprocessResult = {
          text: ruleText,
          source: "skipped_" + strategy.reason,
          durationMs: 0,
          timeoutMs: 0,
          error: null,
          fallbackReason: "",
        };
      } else if (isVoicePostprocessEnabled()) {
        setVoiceInputStatus("postprocessing", {
          message: mode === "task"
            ? bt("voiceTaskPostprocessing")
            : mode === "edit"
              ? bt("voiceEditPostprocessing")
              : bt("voicePostprocessing"),
          stage: "postprocessing",
          mode,
        });
        postprocessResult = await postprocessVoiceText(
          ruleText,
          mode,
          session.draftBeforeStart,
          session.sessionId,
          voicePostprocessTimeoutMs(mode, ruleText),
          text
        );
      } else {
        // 用户关闭智能整理:不把识别文本发给模型服务,只保留本地规则纠错结果。
        // edit 车道没有 LLM 参与就不存在“编辑结果”——规则纠错修的是口述指令
        // 本身,把它当改写产物送进预览、确认后整段替换草稿是错误语义,因此与
        // LLM 失败同口径标记 fallbackReason,走 editPostprocessFailed 通知,
        // 草稿保持不动。听写车道不受影响,仍写回规则纠错文本。
        postprocessResult = {
          text: ruleText,
          source: "skipped_postprocess_disabled",
          durationMs: 0,
          timeoutMs: 0,
          error: null,
          fallbackReason: mode === "edit" ? "postprocess_disabled" : "",
        };
      }
      const candidateText = postprocessResult.text;
      let outputValid = validateVoicePostprocessOutput(text, ruleText, candidateText, mode);
      if (mode === "edit" && postprocessResult.fallbackReason) outputValid = false;
      const finalText = outputValid ? candidateText : (mode === "edit" ? "" : ruleText);
      const outputFallbackReason = outputValid ? "" : "llm_output_invalid";
      const highRiskResidual = mode === "task" ? hasVoiceHighRiskResidual(finalText) : [];
      const fallbackHighRisk = mode === "task"
        && (!!postprocessResult.fallbackReason || !!outputFallbackReason)
        && rawSuspiciousTerms
        && rawSuspiciousTerms.length > 0
        && highRiskResidual.length > 0;
      const taskSendBlocked = mode === "task" && (highRiskResidual.length > 0 || fallbackHighRisk);
      const editUnchanged = mode === "edit"
        && !!String(finalText || "").trim()
        && String(finalText || "").trim() === String(session.draftBeforeStart || "").trim();
      // edit 模式 LLM 失败时 finalText 为空，不能复用「未识别到语音内容」的空结果文案。
      const editPostprocessFailed = mode === "edit" && !!postprocessResult.fallbackReason;
      const diagnostic = {
        mode,
        recording_ms: Math.round(durationMs),
        asr_ms: asrDurationMs,
        llm_ms: postprocessResult.durationMs,
        total_ms: roundedMs(pipelineStartedAt),
        asr_source: (res && res.source) || "",
        llm_source: postprocessResult.source,
        llm_error: postprocessResult.error || "",
        llm_timeout_ms: postprocessResult.timeoutMs || 0,
        normalize_strategy: strategy.strategy,
        skip_reason: strategy.strategy === "run_llm" ? "" : strategy.reason,
        fallback_reason: outputFallbackReason || postprocessResult.fallbackReason || "",
        suspicious_asr_terms: rawSuspiciousTerms,
        high_risk_residual_terms: highRiskResidual,
        task_send_blocked: taskSendBlocked,
        edit_unchanged: editUnchanged,
        raw_text_length: text.length,
        final_text_length: finalText.length,
      };
      logVoicePipeline(diagnostic);
      if (activeVoiceInput !== session) return;
      if (finalText && !editUnchanged && typeof session.writeback === "function") {
        await session.writeback(finalText, session.draftBeforeStart, {
          mode,
          rawText: text,
          diagnostic,
        });
        // writeback await 窗口内用户可能已取消(cancel 把 activeVoiceInput 置空并置 cancelled),
        // 此时不得再用 completed 覆盖取消终态。
        if (activeVoiceInput !== session) return;
      }
      setVoiceInputStatus("completed", {
        message: finalText
          ? editUnchanged
            ? bt("voiceEditNoChange")
            : diagnostic.task_send_blocked
            ? bt("voiceWrittenBack")
            : mode === "task" ? bt("voiceTaskSent") : mode === "edit" ? bt("voiceEditPreviewReady") : bt("voiceWrittenBack")
          : editPostprocessFailed
            ? postprocessResult.fallbackReason === "postprocess_disabled"
              ? bt("voiceEditPostprocessDisabled")
              : bt("voiceEditPostprocessFailed")
            : bt("voiceEmptyResult"),
        completedAt: Date.now(),
        mode,
        diagnostic,
      });
      emitVoiceDiagnostic("writeback", "info", mode === "task" ? "voice task submitted" : "voice text written back", "", "");
    } catch (err) {
      const normalized = normalizeVoiceError(err, "transcribing");
      setVoiceInputStatus("failed", {
        message: normalized.message,
        error: normalized.message,
        category: normalized.category,
        stage: normalized.stage,
        completedAt: Date.now(),
      });
      emitVoiceDiagnostic(normalized.stage, "error", normalized.diagnostic || normalized.category, normalized.message, normalized.category);
    } finally {
      if (activeVoiceInput === session) activeVoiceInput = null;
    }
  }

  // 一键安装本地语音识别依赖（模型下载 + 缺 ffmpeg 走 pkexec apt），进度走
  // voice_asr:progress 事件。装完 ready 自动关框。
  async function installVoiceAsr() {
    if (state.voiceAsrSetup.installing) return;
    state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, { open: false, installing: true, cancelling: false, error: null, progress: { stage: "start" } });
    notify();
    try {
      const st = await invoke("install_voice_asr");
      const patch = { installing: false, cancelling: false, status: st, progress: { stage: "done" } };
      if (st && st.ready) patch.open = false;
      state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, patch);
      notify();
    } catch (e) {
      const message = String(e);
      const cancelled = state.voiceAsrSetup.cancelling || message.includes("已取消");
      const alreadyInstalling = message.includes("正在下载或安装中") || message.includes("already installing");
      const failedPatch = {
        open: false,
        installing: alreadyInstalling && !cancelled,
        cancelling: false,
        progress: cancelled ? { stage: "cancelled" } : (state.voiceAsrSetup.progress || { stage: "start" }),
        error: cancelled || alreadyInstalling ? null : message,
      };
      state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, failedPatch);
      notify();
    }
  }

  async function cancelVoiceAsrSetup() {
    if (!state.voiceAsrSetup.installing) {
      closeVoiceAsrSetup();
      return;
    }
    if (state.voiceAsrSetup.cancelling) return;
    state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, {
      cancelling: true,
      progress: { stage: "cancelling" },
      error: null,
    });
    notify();
    try {
      await invoke("cancel_voice_asr");
      state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, { open: false });
      notify();
    } catch (e) {
      state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, {
        cancelling: false,
        error: String(e),
      });
      notify();
    }
  }

  function closeVoiceAsrSetup() {
    state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, { open: false });
    notify();
  }


  // eslint-disable-next-line sonarjs/cognitive-complexity -- single entry covering permission/recording/mode branches; split tracked separately
  async function startVoiceInput(draftText, writeback, options) {
    if (activeVoiceInput && state.voiceInput.status === "recording") {
      finishVoiceInput(false, false);
      return;
    }
    if (activeVoiceInput) {
      finishVoiceInput(true, false);
      return;
    }

    // 模型下载期间再次触发语音时保留原下载会话，不能用新的依赖检测结果
    // 覆盖 installing/cancelling/progress。open 状态也保持原样：
    // 自动下载走按钮 loading + 小 popover，手动修复安装才保留安装框。
    if (state.voiceAsrSetup.installing) {
      notify();
      return;
    }

    // 点击后立即进入可见、可取消的检测态。模型状态查询首次可能需要读取模型文件，
    // 如果等查询结束后才更新 UI，Windows 上会表现为按钮点击后没有任何反馈。
    const session = {
      id: Date.now().toString(36),
      sessionId: state.activeSessionId || null,
      draftBeforeStart: String(draftText || ""),
      writeback,
      mode: normalizeVoiceMode(options && options.mode),
      chunks: [],
      sampleRate: 16000,
      startedAt: Date.now(),
    };
    activeVoiceInput = session;
    setVoiceInputStatus("requesting_permission", {
      message: bt("voiceCheckingDevice"),
      sessionId: session.sessionId,
      startedAt: session.startedAt,
      stage: "device",
      mode: session.mode,
    });
    emitVoiceDiagnostic("device", "info", "checking voice input environment", "", "");

    // 首次/缺组件：先检测本地语音识别依赖，缺则弹安装框、不进录音。
    try {
      const asrStatus = await invoke("voice_asr_status");
      if (activeVoiceInput !== session) return;
      // 未装好即弹安装引导；installable 决定当前平台是否提供内置安装入口。
      if (asrStatus && !asrStatus.ready) {
        cleanupVoiceInputSession(session);
        activeVoiceInput = null;
        setVoiceInputStatus("idle", { message: "", stage: null, sessionId: null });
        state.voiceAsrSetup = {
          open: !asrStatus.installable,
          status: asrStatus,
          installing: false,
          cancelling: false,
          progress: null,
          error: null,
        };
        notify();
        if (asrStatus.installable) {
          installVoiceAsr();
        }
        return;
      }
    } catch {
      if (activeVoiceInput !== session) return;
      // 检测失败（如 mock 环境/旧后端）不阻塞，继续走原录音路径（环境变量/兜底引擎）
    }

    if (options && typeof options.beforePermission === "function") {
      const shouldContinue = await options.beforePermission({
        mode: session.mode,
        sessionId: session.sessionId,
        draftBeforeStart: session.draftBeforeStart,
      });
      if (activeVoiceInput !== session) return;
      if (shouldContinue === false) {
        cleanupVoiceInputSession(session);
        activeVoiceInput = null;
        setVoiceInputStatus("idle", { message: "", stage: null, sessionId: null });
        return;
      }
    }

    const AudioCtor = window.AudioContext || window.webkitAudioContext; // eslint-disable-line compat/compat -- Safari 14.0 ships webkitAudioContext; the || fallback above selects it
    setVoiceInputStatus("requesting_permission", {
      message: bt("voiceRequestingPermission"),
      stage: "permission",
    });
    emitVoiceDiagnostic("permission", "info", "requesting microphone permission", "", "");

    try {
      if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
        throw voiceFlowError("device_unavailable", "device", bt("voiceWebviewNoMic"));
      }
      if (!AudioCtor) {
        throw voiceFlowError("recording_failed", "recording", bt("voiceWebviewNoRecording"));
      }
      const hasAudioInput = await probeVoiceAudioInput(VOICE_DEVICE_PROBE_TIMEOUT_MS);
      if (activeVoiceInput !== session) return;
      if (hasAudioInput === false) {
        throw voiceFlowError("device_unavailable", "device", bt("voiceNoDeviceConnect"));
      }
      session.stream = await requestVoiceMedia(session, {
        audio: {
          channelCount: 1,
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
        },
      }, VOICE_DEVICE_REQUEST_TIMEOUT_MS);
      if (activeVoiceInput !== session) {
        cleanupVoiceInputSession(session);
        return;
      }
      session.audioContext = new AudioCtor();
      session.sampleRate = session.audioContext.sampleRate || 16000;
      session.source = session.audioContext.createMediaStreamSource(session.stream);
      session.processor = session.audioContext.createScriptProcessor(4096, 1, 1);
      session.zeroGain = session.audioContext.createGain();
      session.zeroGain.gain.value = 0;
      session.processor.onaudioprocess = function (event) {
        if (activeVoiceInput !== session) return;
        const input = event.inputBuffer.getChannelData(0);
        session.chunks.push(new Float32Array(input));
      };
      session.source.connect(session.processor);
      session.processor.connect(session.zeroGain);
      session.zeroGain.connect(session.audioContext.destination);
      session.timeoutId = setTimeout(function () { finishVoiceInput(false, true); }, VOICE_RECORDING_MAX_DURATION_MS);
      // 拔麦/设备断开时 track 结束(stop() 不触发 onended,收尾无递归),
      // 用已录内容立即收尾,不再空转到时长上限。
      session.stream.getTracks().forEach(function (track) {
        track.onended = function () {
          if (activeVoiceInput === session) finishVoiceInput(false);
        };
      });
      setVoiceInputStatus("recording", { message: bt("voiceRecording"), stage: "recording" });
      // 录音正式开始后登记本窗 label;A 窗录音中 B 窗的 Alt 手势会被定向到 A 窗停止。
      syncVoiceShortcutRecording(currentVoiceWindowLabel(), session.sessionId);
      invoke("track_behavior_event", {
        request: {
          eventName: "voice_started",
          sessionId: session.sessionId,
          stage: "recording",
        },
      }).catch(function () {});
      emitVoiceDiagnostic("recording", "info", "recording started", "", "");
    } catch (err) {
      cleanupVoiceInputSession(session);
      if (activeVoiceInput !== session) return;
      activeVoiceInput = null;
      const normalized = normalizeVoiceError(err, "recording");
      if (normalized.category === "permission_denied") {
        try {
          const permissionReset = await invoke("reset_microphone_permission");
          if (permissionReset) {
            normalized.message = bt("voicePermissionDeniedRetry");
            emitVoiceDiagnostic("permission", "info", "microphone permission reset to default", normalized.message, normalized.category);
          }
        } catch (resetError) {
          emitVoiceDiagnostic("permission", "warn", "failed to reset microphone permission: " + String(resetError), normalized.message, normalized.category);
        }
      }
      setVoiceInputStatus("failed", {
        message: normalized.message,
        error: normalized.message,
        category: normalized.category,
        stage: normalized.stage,
        completedAt: Date.now(),
      });
      emitVoiceDiagnostic(normalized.stage, "error", normalized.diagnostic || normalized.category, normalized.message, normalized.category);
    }
  }

  function cancelVoiceInput() {
    finishVoiceInput(true, false);
  }

  function clearVoiceInput() {
    if (activeVoiceInput) {
      finishVoiceInput(true, false);
      return;
    }
    setVoiceInputStatus("idle", {
      message: "",
      error: null,
      category: null,
      stage: null,
      sessionId: null,
    });
  }

  function appendVoiceText(base, text) {
    const left = String(base || "").trimEnd();
    const right = String(text || "").trim();
    if (!left) return right;
    if (!right) return left;
    return left + (/[。！？.!?，,;；:]$/.test(left) ? " " : "\n") + right;
  }

  function runVoiceInputDebugAssertions() {
    const denied = normalizeVoiceError({ name: "NotAllowedError" });
    const noDevice = normalizeVoiceError({ name: "NotFoundError" });
    const unsupportedConstraint = normalizeVoiceError({ name: "OverconstrainedError", message: "Invalid constraint", constraint: "channelCount" });
    const mismatch = normalizeVoiceError({ category: "context_mismatch" });
    console.assert(denied.category === "permission_denied", "permission error classified");
    console.assert(noDevice.category === "device_unavailable", "device error classified");
    console.assert(unsupportedConstraint.category === "constraint_unsupported", "unsupported constraint classified");
    console.assert(unsupportedConstraint.diagnostic === "unsupported media constraint: channelCount", "unsupported constraint diagnostic");
    console.assert(mismatch.stage === "writeback", "context mismatch classified");
    console.assert(appendVoiceText("草稿", "识别文本") === "草稿\n识别文本", "voice text appended");
    return true;
  }

  // 跨窗录音互斥:录音生命周期把本窗口 label 同步给原生快捷键钩子,Rust 侧据此把
  // 其他窗口的 Alt 手势定向到录音窗停止,绝不双开。旧后端未注册该命令时静默忽略。
  function currentVoiceWindowLabel() {
    try {
      const windowApi = window.__TAURI__ && window.__TAURI__.window;
      if (!windowApi || typeof windowApi.getCurrentWindow !== "function") return "";
      return String(windowApi.getCurrentWindow().label || "");
    } catch { /* window label lookup is best-effort */ }
    return "";
  }

  // 会话 token:录音登记时带上会话 id,收尾清除时仅当 token 仍匹配才下发,
  // 防止上一会话迟到的收尾(异步窗口/竞态)把新会话刚登记的互斥 label 抹掉。
  // 无 token 的清除(快捷键 Router 清陈旧登记)始终放行。
  let voiceShortcutRecordingToken = null;

  function syncVoiceShortcutRecording(label, token) {
    try {
      if (label) {
        voiceShortcutRecordingToken = token || null;
      } else if (token && voiceShortcutRecordingToken && token !== voiceShortcutRecordingToken) {
        return;
      } else {
        voiceShortcutRecordingToken = null;
      }
      Promise.resolve(invoke("set_voice_shortcut_recording", { label: label || null })).catch(function () {});
    } catch { /* old backend without the command */ }
  }

  function setVoiceShortcutEnabled(enabled) {
    const sync = function () {
      return invoke("set_voice_shortcut_enabled", { enabled: !!enabled });
    };
    // Retry once on failure so the UI/localStorage mirror does not drift
    // from the native hook for long; on persistent failure return false so
    // the caller can tell. There is no mount-time replay: settings.json is
    // authoritative and Rust replays it into the native layer at startup,
    // so a failed sync here only heals on the user's next explicit toggle.
    return sync().catch(function (error) {
      console.warn("[voice] failed to sync shortcut setting, retrying once", error);
      return sync().catch(function (retryError) {
        console.warn("[voice] failed to sync shortcut setting after retry", retryError);
        return false;
      });
    });
  }

    return {
      startVoiceInput,
      installVoiceAsr,
      cancelVoiceAsrSetup,
      closeVoiceAsrSetup,
      cancelVoiceInput,
      clearVoiceInput,
      setVoiceShortcutEnabled,
      syncVoiceShortcutRecording,
      appendVoiceText,
      runVoiceInputDebugAssertions
    };
  };
})(window);
