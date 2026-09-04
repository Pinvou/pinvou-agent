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
  // ── Voice input (WebView one-shot recording → local SenseVoice/FunASR ASR; Linux webview mic permission setup see lib.rs)──────────────
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

  // Smart post-process toggle, enabled by default; the key must stay in sync with
  // features/chat/voice-shortcut-settings.mjs (classic scripts cannot import that module),
  // so any key change must be applied on both sides.
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
      // The 进价→金价 correction only covers market-quote context; genuine price-comparison
      // input like "今天的进价比昨天低" must not be miscorrected.
      .replaceAll(/今日进价(?![比高低更涨跌贵])/g, "今日金价")
      .replaceAll(/今天的进价(?![比高低更涨跌贵])/g, "今天的金价")
      .replaceAll('数据分析图标', "数据分析图表")
      .replaceAll('核心公能', "核心功能")
      .replaceAll('销售暑假', "销售数据")
      .replaceAll(/屁屁提|PPTT/g, "PPT")
      .replaceAll('截止事件', "截止时间")
      // "负责任" is usually adjectival (他很负责任 / 负责任的老师); only correct it to the noun
      // "负责人" when no degree-adverb prefix precedes it and it is not followed by "的".
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
      // "交互风险" (interaction risk) is a valid phrase on its own (as in interaction-design
      // risk); deterministic rules cannot tell them apart, so it is not corrected at the rule
      // layer and the LLM judges it against the raw ASR text.
      .replaceAll('各列三跳', "各列三条")
      .replaceAll('客诉投诉', "客服投诉")
      .replaceAll(/产品先(?![上发做进推试跑])/g, "产品线")
      .replaceAll('高频问提', "高频问题")
      .replaceAll('重点推近', "重点推进")
      .replaceAll(/语音输出(?!的)/g, "语音输入")
      .replaceAll('模型下崽体验', "模型下载体验")
      .replaceAll('知识裤', "知识库")
      // "报消" is often a cross-word false hit (上报|消费者, 预报|消息); skip the correction
      // when preceded by 上/预/补/申.
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
    // When the LLM reverts a rule correction back to the original ASR wording (e.g. "表哥"
    // corrected to "表格" by the rules, then changed back by the LLM), the final text
    // naturally lacks the rule output. Re-apply the same deterministic rules to the final
    // text against the raw ASR baseline before validating: semantically equivalent
    // restorations pass, outputs that truly dropped a term are still rejected.
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

  // Stable VoiceCommandError codes from the Rust side → trilingual copy keys inside the
  // bridge. Codes take precedence over rawMessage: the Rust message is Chinese engineering
  // prose, meant for logs/diagnostics only, never passed straight to en/ja UIs
  // (review leftover: ~12 Chinese strings still pass through).
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
    // Chrome reports "microphone busy with another app" as NotReadableError / TrackStartError,
    // distinct from "no device"; the browser message is English, so map it to dedicated
    // trilingual copy.
    if (name === "NotReadableError" || name === "TrackStartError") {
      return { category: "device_unavailable", stage: "device", message: bt("voiceMicUnavailable") };
    }
    // WebKitGTK may report unsupported audio constraints as OverconstrainedError / "Invalid constraint".
    // This is different from having no recording device: the device may exist, it just does
    // not support channelCount, noise suppression, and similar configurations.
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
      // With a stable error code, map to trilingual copy by code and demote the Chinese
      // original to diagnostics; only legacy errors without a code fall back to rawMessage
      // (Chinese engineering prose only, kept for historical behavior).
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
    // Detach the audio callback first: webkit2gtk's WebAudio is backed by GStreamer, and
    // ScriptProcessorNode's onaudioprocess runs on the audio thread. If it fires once more
    // during disconnect/close and touches freed buffers, WebProcess segfaults (seen as
    // "the app crashes after text was recognized"). Always null it first.
    try { if (session.processor) session.processor.onaudioprocess = null; } catch { /* release failure only affects this page's audio */ }
    try { if (session.processor) session.processor.disconnect(); } catch { /* release failure only affects this page's audio */ }
    try { if (session.source) session.source.disconnect(); } catch { /* release failure only affects this page's audio */ }
    try { if (session.zeroGain) session.zeroGain.disconnect(); } catch { /* release failure only affects this page's audio */ }
    stopMediaTracks(session.stream);
    session.processor = null;
    session.source = null;
    session.zeroGain = null;
    session.stream = null;
    // close() tears the GStreamer pipeline down asynchronously and races with the
    // disconnect/track.stop above in the same tick, which crashes most easily; once the
    // nodes are fully detached, close on the next event-loop turn and swallow close errors.
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

  // A 60s 16kHz 16bit WAV is ~1.9MB; shipping it across IPC as a JSON number array means
  // ~1.92M elements, multiple copies, ~25MB peak memory. Use a standard base64 string
  // (with padding), the same chunked encoding as the web lane's encodeBase64Bytes.
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

  // The model occasionally wraps the whole output in a ``` fence; strip only fully enclosing
  // fences, keeping legitimate Markdown list content for dictation.
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
          // Send the raw ASR text along too: the model needs the original to undo deterministic
          // rule miscorrections (e.g. "表哥"→"表格"). Old backend structs lack deny_unknown_fields
          // and safely ignore this field.
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
      // Truncated output (finish_reason=length) must not be written back silently (it may cut
      // to 60-70% and slip past the 55% shrink red line); fall back to the rule text.
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
    // The recording session ends here (cancel/finish/error all funnel through this point);
    // release this window's cross-window recording mutex claim.
    // Carries a session token: a late teardown must not erase a new session's registration.
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
      // Classification must use the raw ASR text: if the corrected text were classified, the
      // suspicious terms covered by deterministic rules (负责任, 表哥, 语音输出, ...) would be
      // gone before classification, and short dictation would take the use_asr path, silently
      // writing miscorrections back into the input box.
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
        // Smart post-processing is off: do not send the recognized text to the model service;
        // keep only the local rule-based correction result. On the edit lane there is no
        // "edit result" without the LLM — rule correction fixes the spoken instruction itself,
        // and feeding that into the replace preview as a rewrite product that overwrites the
        // whole draft on confirm would be wrong semantics. So mark fallbackReason the same way
        // as an LLM failure, surface editPostprocessFailed, and leave the draft untouched.
        // The dictation lane is unaffected and still writes back the rule-corrected text.
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
      // When the LLM fails in edit mode, finalText is empty; do not reuse the
      // "未识别到语音内容" empty-result copy.
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
        // The user may have cancelled during the writeback await window (cancel clears
        // activeVoiceInput and sets cancelled); never overwrite that terminal cancelled
        // state with completed here.
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

  // One-click install of the local ASR dependencies (model download; missing ffmpeg goes
  // through pkexec apt), progress via the voice_asr:progress event. Auto-close the dialog
  // once the status is ready.
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

    // While a model download is running, re-triggering voice input keeps the original
    // download session; a fresh dependency probe must not overwrite
    // installing/cancelling/progress. Keep the open state as-is too: auto-install uses the
    // button loading state + small popover, only manual repair keeps the install dialog.
    if (state.voiceAsrSetup.installing) {
      notify();
      return;
    }

    // Enter a visible, cancellable probing state immediately on click. The first model status
    // query may need to read model files; updating the UI only after the query finishes makes
    // the button look unresponsive on Windows.
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

    // First run / missing components: probe local ASR dependencies first; if anything is
    // missing, show the install dialog instead of recording.
    try {
      const asrStatus = await invoke("voice_asr_status");
      if (activeVoiceInput !== session) return;
      // Not ready: pop the install guide; installable decides whether this platform offers
      // the built-in install entry.
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
      // Probe failure (e.g. mock environment / old backend) must not block; continue with the
      // original recording path (env vars / fallback engine)
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
      // Unplugging / device disconnect ends the track (stop() does not fire onended, so no
      // recursive teardown); finish immediately with what was recorded instead of idling
      // until the duration cap.
      session.stream.getTracks().forEach(function (track) {
        track.onended = function () {
          if (activeVoiceInput === session) finishVoiceInput(false);
        };
      });
      setVoiceInputStatus("recording", { message: bt("voiceRecording"), stage: "recording" });
      // Once recording actually starts, register this window's label; while window A records,
      // window B's Alt gesture is routed to window A to stop it.
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

  // Cross-window recording mutex: the recording lifecycle syncs this window's label to the
  // native shortcut hook, and Rust uses it to route other windows' Alt gestures to the
  // recording window as a stop, never double-starting. Silently ignored by old backends
  // without the command.
  function currentVoiceWindowLabel() {
    try {
      const windowApi = window.__TAURI__ && window.__TAURI__.window;
      if (!windowApi || typeof windowApi.getCurrentWindow !== "function") return "";
      return String(windowApi.getCurrentWindow().label || "");
    } catch { /* window label lookup is best-effort */ }
    return "";
  }

  // Session token: registration carries the session id, and teardown only dispatches the
  // clear when the token still matches, preventing a previous session's late teardown
  // (async window/race) from wiping the mutex label the new session just registered.
  // Token-less clears (the shortcut Router purging stale registrations) always pass.
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
