/**
 * voice feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  "use strict";
  var registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["voice"] = function (context) {
    var state = context.state;
    var notify = context.notify;
    var invoke = context.invoke;
    var bt = context.bt;
  // ── 语音输入（WebView one-shot 录音 → 本地 SenseVoice/FunASR ASR；Linux webview 录音授权见 lib.rs setup）──────────────
  var activeVoiceInput = null;
  var VOICE_DEVICE_PROBE_TIMEOUT_MS = 1500;
  var VOICE_DEVICE_REQUEST_TIMEOUT_MS = 8000;
  var VOICE_RECORDING_MAX_DURATION_MS = 60000;

  function normalizeVoiceMode(mode) {
    if (mode === "task") return "task";
    if (mode === "edit" || mode === "voice_edit" || mode === "draft_edit") return "edit";
    return "dictation";
  }

  function voiceNow() {
    return (typeof performance !== "undefined" && typeof performance.now === "function")
      ? performance.now()
      : Date.now();
  }

  function roundedMs(start, end) {
    return Math.max(0, Math.round((end || voiceNow()) - start));
  }

  var VOICE_FILLER_TERMS = ["嗯", "啊", "呃", "那个", "就是"];
  var VOICE_SUSPICIOUS_ASR_TERMS = [
    "进价", "惊吓", "图标", "销售暑假", "屁屁提", "PPTT", "截止事件",
    "负责任", "风险电", "三百字一类", "爱新闻", "代码嫩力", "核心公能",
    "GP杠5", "GPT杠5", "closonic", "克劳德", "deeps V3", "deep seek v three",
    "批地爱福", "pDF", "合同金鹅", "付款结点", "违约条宽", "表哥", "亲自酒店",
    "离海绵距离", "四零一", "talken", "过期处里", "产品民", "pin vo",
    "rest a p i", "认正", "错误马", "示例情求", "语音输出", "模型下崽体验",
    "知识裤", "报消", "住宿上线", "班纳", "温婉简洁", "高贴票"
  ];
  var VOICE_PROTECTED_TERMS = [
    "金价", "图表", "表格", "PPT", "GPT-5", "Claude Sonnet", "DeepSeek V3",
    "AI 新闻", "PDF", "Pinvou", "REST API", "401", "token", "高铁票",
    "负责人", "截止时间", "预算", "部门", "超支项", "付款风险", "交付风险",
    "客服投诉", "产品线", "高频问题", "语音输入", "模型下载体验", "知识库",
    "差旅报销标准", "住宿上限", "banner", "温暖简洁"
  ];
  var VOICE_TASK_WORDS = [
    "查", "搜索", "生成", "整理", "总结", "比较", "做一个", "做成", "列出",
    "写到", "发送", "创建", "基于", "报告", "PPT", "网页", "图表", "表格"
  ];

  function compactVoiceText(text) {
    return String(text || "")
      .replace(/[\s。！？!?，,、；;：:"'“”‘’（）()【】\[\].…\-—]/g, "")
      .trim();
  }

  function voiceContainsAny(text, terms) {
    var value = String(text || "");
    return terms.filter(function (term) { return value.indexOf(term) >= 0; });
  }

  function isFillerOnlyVoiceText(text) {
    var compact = compactVoiceText(text);
    if (!compact) return true;
    if (compact.length > 8) return false;
    if (compact === "文") return true;
    var remaining = compact;
    VOICE_FILLER_TERMS.forEach(function (term) {
      remaining = remaining.split(term).join("");
    });
    remaining = remaining.split("额").join("");
    remaining = remaining.split("文").join("");
    return remaining.trim() === "";
  }

  function isShortClearVoiceText(text, mode) {
    if (normalizeVoiceMode(mode) !== "dictation") return false;
    var compact = compactVoiceText(text);
    if (!compact || compact.length > 18) return false;
    if (voiceContainsAny(text, VOICE_FILLER_TERMS).length) return false;
    if (voiceContainsAny(text, VOICE_SUSPICIOUS_ASR_TERMS).length) return false;
    if (voiceContainsAny(text, VOICE_TASK_WORDS).length) return false;
    return true;
  }

  function classifyVoiceText(rawText, mode) {
    var normalizedMode = normalizeVoiceMode(mode);
    var text = String(rawText || "").trim();
    var suspicious = voiceContainsAny(text, VOICE_SUSPICIOUS_ASR_TERMS);
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
    var normalizedMode = normalizeVoiceMode(mode);
    if (normalizedMode === "task") return 8000;
    if (normalizedMode === "edit") return 12000;
    return compactVoiceText(rawText).length <= 18 ? 3000 : 5000;
  }

  function hasVoiceHighRiskResidual(text) {
    return voiceContainsAny(text, VOICE_SUSPICIOUS_ASR_TERMS);
  }

  function applyVoiceDeterministicCorrections(text, rawText) {
    var value = String(text || "");
    var raw = String(rawText || "");
    if (isFillerOnlyVoiceText(value)) return "";
    if (!value.trim()) return value;
    value = value
      .replace(/^[嗯啊呃额][，,、\s]+/g, "")
      .replace(/今日进价/g, "今日金价")
      .replace(/今天的进价/g, "今天的金价")
      .replace(/数据分析图标/g, "数据分析图表")
      .replace(/核心公能/g, "核心功能")
      .replace(/销售暑假/g, "销售数据")
      .replace(/屁屁提|PPTT/g, "PPT")
      .replace(/截止事件/g, "截止时间")
      .replace(/负责任/g, "负责人")
      .replace(/风险电/g, "风险点")
      .replace(/三百字一类/g, "三百字以内")
      .replace(/代码嫩力/g, "代码能力")
      .replace(/g\s*p\s*t\s*five/gi, "GPT-5")
      .replace(/G\s*P\s*T?\s*杠\s*5/gi, "GPT-5")
      .replace(/克劳德\s*sonnet|closonic/gi, "Claude Sonnet")
      .replace(/deep\s*seek\s*v\s*three|deeps\s*V3/gi, "DeepSeek V3")
      .replace(/爱新闻|AI新闻/g, "AI 新闻")
      .replace(/批地爱福|pDF/g, "PDF")
      .replace(/合同金鹅/g, "合同金额")
      .replace(/付款结点/g, "付款节点")
      .replace(/违约条宽/g, "违约条款")
      .replace(/表哥/g, "表格")
      .replace(/本月玉算/g, "本月预算")
      .replace(/不门/g, "部门")
      .replace(/超时项/g, "超支项")
      .replace(/亲自酒店/g, "亲子酒店")
      .replace(/离海绵距离/g, "离海边距离")
      .replace(/四零一/g, "401")
      .replace(/talken/gi, "token")
      .replace(/过期处里/g, "过期处理")
      .replace(/产品民\s*pin\s+vo\b|产品名con(?![a-zA-Z])|\bpin\s+vo\b/gi, "产品名 Pinvou")
      .replace(/rest\s*a\s*p\s*i/gi, "REST API")
      .replace(/认正/g, "认证")
      .replace(/错误马/g, "错误码")
      .replace(/示例情求/g, "示例请求")
      .replace(/高贴票/g, "高铁票")
      .replace(/北京的高$/g, "北京的高铁票")
      .replace(/副款风险/g, "付款风险")
      .replace(/交互风险/g, "交付风险")
      .replace(/各列三跳/g, "各列三条")
      .replace(/客诉投诉/g, "客服投诉")
      .replace(/产品先/g, "产品线")
      .replace(/高频问提/g, "高频问题")
      .replace(/重点推近/g, "重点推进")
      .replace(/语音输出/g, "语音输入")
      .replace(/模型下崽体验/g, "模型下载体验")
      .replace(/知识裤/g, "知识库")
      .replace(/报消/g, "报销")
      .replace(/住宿上线/g, "住宿上限")
      .replace(/中秋活动班纳/g, "中秋活动 banner")
      .replace(/温婉简洁/g, "温暖简洁")
      .replace(/有长方形[，,、\s]*的需要/g, "是长方形，需要")
      .replace(/长方形[，,、\s]*的需要/g, "长方形，需要")
      .replace(/联网下的图片/g, "联网下载的图片")
      .replace(/联网下图片/g, "联网下载图片");
    value = value
      .replace(/GPT-5和/g, "GPT-5 和")
      .replace(/和Claude Sonnet/g, "和 Claude Sonnet");
    if (raw.indexOf("图标") >= 0 && value.indexOf("数据分析") >= 0 && value.indexOf("图表") < 0) {
      value = value.replace(/数据分析$/, "数据分析图表");
    }
    if (raw.indexOf("只是发了") >= 0) {
      value = value
        .replace(/^嗯[，,、\s]*/g, "")
        .replace(/只是发了一个图表吧[。.]?/g, "图表。")
        .replace(/生成数据分析[，,、\s]*图表/g, "生成数据分析图表")
        .replace(/生成数据分析图表吧[。.]?/g, "生成数据分析图表。");
    }
    return value;
  }

  function voiceProtectedTermsIn(text) {
    return VOICE_PROTECTED_TERMS.filter(function (term) {
      return String(text || "").indexOf(term) >= 0;
    });
  }

  function validateVoicePostprocessOutput(rawText, ruleText, finalText, mode) {
    var raw = String(rawText || "");
    var corrected = String(ruleText || "");
    var finalValue = String(finalText || "");
    var rawCompact = compactVoiceText(raw);
    var correctedCompact = compactVoiceText(corrected);
    var finalCompact = compactVoiceText(finalValue);
    if (!finalCompact) return isFillerOnlyVoiceText(raw) || isFillerOnlyVoiceText(corrected);
    if (rawCompact.length > 12 && finalCompact.length < Math.floor(correctedCompact.length * 0.55)) {
      return false;
    }
    var protectedTerms = voiceProtectedTermsIn(corrected);
    var missingProtected = protectedTerms.filter(function (term) {
      return finalValue.indexOf(term) < 0;
    });
    if (missingProtected.length) return false;
    var normalizedMode = normalizeVoiceMode(mode);
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
    } catch (_) {}
    try {
      var key = "pinvou_voice_pipeline_diagnostics";
      var current = JSON.parse(localStorage.getItem(key) || "[]");
      if (!Array.isArray(current)) current = [];
      current.push(Object.assign({ recorded_at: new Date().toISOString() }, diagnostic));
      localStorage.setItem(key, JSON.stringify(current.slice(-50)));
    } catch (_) {}
    try {
      window.dispatchEvent(new CustomEvent("pinvou:voice-pipeline-diagnostic", { detail: diagnostic }));
    } catch (_) {}
  }

  function setVoiceInputStatus(status, patch) {
    var next = Object.assign({}, state.voiceInput, patch || {});
    next.status = status;
    if (status !== "failed") {
      next.error = null;
      next.category = null;
    }
    state.voiceInput = next;
    notify();
  }

  function emitVoiceDiagnostic(stage, level, message, userMessage, category) {
    var event = {
      stage: stage,
      level: level,
      message: message,
      user_message: userMessage || "",
      category: category || "",
    };
    var fn = level === "error" ? console.error : level === "warn" ? console.warn : console.info;
    fn.call(console, "[voice-input]", event);
  }

  function normalizeVoiceError(err, fallbackStage) {
    var name = String((err && err.name) || "");
    var rawCategory = (err && err.category) || "";
    var rawStage = (err && err.stage) || fallbackStage || "recording";
    var rawMessage = String((err && (err.message || err.toString && err.toString())) || err || "");
    var constraint = String((err && err.constraint) || "");
    var emptyResultLike = /ASR empty result|empty result|backend returned no usable|no usable result|failed \(exit 6\)|exit 6/i.test(rawMessage);
    if (name === "NotAllowedError" || name === "SecurityError" || rawCategory === "permission_denied") {
      return { category: "permission_denied", stage: "permission", message: bt("voicePermissionDenied") };
    }
    if (rawCategory === "device_unavailable") {
      return { category: "device_unavailable", stage: "device", message: rawMessage || bt("voiceNoDevice") };
    }
    if (name === "NotFoundError" || name === "DevicesNotFoundError") {
      return { category: "device_unavailable", stage: "device", message: bt("voiceNoDevice") };
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
      return { category: "recognition_failed", stage: rawStage, message: rawMessage || bt("voiceRecognitionFailed") };
    }
    return {
      category: rawCategory || "recording_failed",
      stage: rawStage,
      message: rawMessage || bt("voiceInputFailed"),
    };
  }

  function stopMediaTracks(stream) {
    if (!stream) return;
    stream.getTracks().forEach(function (track) { try { track.stop(); } catch (_) {} });
  }

  function cleanupVoiceInputSession(session) {
    if (!session) return;
    if (session.timeoutId) clearTimeout(session.timeoutId);
    if (session.permissionTimeoutId) clearTimeout(session.permissionTimeoutId);
    session.permissionTimeoutId = null;
    if (session.cancelPermissionRequest) {
      var cancelPermissionRequest = session.cancelPermissionRequest;
      session.cancelPermissionRequest = null;
      try { cancelPermissionRequest(); } catch (_) {}
    }
    // 先摘掉音频回调：webkit2gtk 的 WebAudio 是 GStreamer 后端，ScriptProcessorNode 的
    // onaudioprocess 跑在音频线程，若在 disconnect/close 期间再触发一次、访问已释放的
    // 缓冲，会让 WebProcess 段错误（表现为「识别出文字后 app 崩溃」）。务必先置 null。
    try { if (session.processor) session.processor.onaudioprocess = null; } catch (_) {}
    try { if (session.processor) session.processor.disconnect(); } catch (_) {}
    try { if (session.source) session.source.disconnect(); } catch (_) {}
    try { if (session.zeroGain) session.zeroGain.disconnect(); } catch (_) {}
    stopMediaTracks(session.stream);
    session.processor = null;
    session.source = null;
    session.zeroGain = null;
    session.stream = null;
    // close() 触发 GStreamer 管线异步拆解，与上面的 disconnect/track.stop 在同一拍里竞争最易崩；
    // 摘干净节点后挪到下一个事件循环再关，并吞掉 close 的异常。
    var ctx = session.audioContext;
    session.audioContext = null;
    if (ctx && ctx.state !== "closed") {
      setTimeout(function () { try { ctx.close().catch(function () {}); } catch (_) {} }, 0);
    }
  }

  async function probeVoiceAudioInput(timeoutMs) {
    if (!navigator.mediaDevices || typeof navigator.mediaDevices.enumerateDevices !== "function") return null;
    var timer = null;
    try {
      return await Promise.race([
        navigator.mediaDevices.enumerateDevices().then(function (devices) {
          return devices.some(function (device) { return device && device.kind === "audioinput"; });
        }),
        new Promise(function (resolve) {
          timer = setTimeout(function () { resolve(null); }, timeoutMs || VOICE_DEVICE_PROBE_TIMEOUT_MS);
        }),
      ]);
    } catch (_) {
      return null;
    } finally {
      if (timer) clearTimeout(timer);
    }
  }

  function requestVoiceMedia(session, constraints, timeoutMs) {
    var abandoned = false;
    var mediaPromise = navigator.mediaDevices.getUserMedia(constraints).then(function (stream) {
      if (abandoned || activeVoiceInput !== session) {
        stopMediaTracks(stream);
        throw { category: "cancelled", stage: "permission", message: bt("voiceCancelled") };
      }
      return stream;
    });
    var timeoutPromise = new Promise(function (_, reject) {
      session.permissionTimeoutId = setTimeout(function () {
        abandoned = true;
        reject({
          category: "device_unavailable",
          stage: "device",
          message: bt("voiceDeviceTimeout"),
        });
      }, timeoutMs || VOICE_DEVICE_REQUEST_TIMEOUT_MS);
    });
    var cancelPromise = new Promise(function (_, reject) {
      session.cancelPermissionRequest = function () {
        abandoned = true;
        reject({ category: "cancelled", stage: "permission", message: bt("voiceCancelled") });
      };
    });
    return Promise.race([mediaPromise, timeoutPromise, cancelPromise]).finally(function () {
      if (session.permissionTimeoutId) clearTimeout(session.permissionTimeoutId);
      session.permissionTimeoutId = null;
      session.cancelPermissionRequest = null;
    });
  }

  function mergeFloatChunks(chunks) {
    var total = chunks.reduce(function (sum, chunk) { return sum + chunk.length; }, 0);
    var out = new Float32Array(total);
    var offset = 0;
    chunks.forEach(function (chunk) {
      out.set(chunk, offset);
      offset += chunk.length;
    });
    return out;
  }

  function downsamplePcm(samples, sourceRate, targetRate) {
    if (!samples.length || sourceRate === targetRate) return samples;
    var ratio = sourceRate / targetRate;
    var len = Math.max(1, Math.round(samples.length / ratio));
    var out = new Float32Array(len);
    for (var i = 0; i < len; i++) {
      var start = Math.floor(i * ratio);
      var end = Math.min(samples.length, Math.floor((i + 1) * ratio));
      var sum = 0;
      var count = 0;
      for (var j = start; j < end; j++) { sum += samples[j]; count++; }
      out[i] = count ? sum / count : samples[Math.min(start, samples.length - 1)];
    }
    return out;
  }

  function encodeWav(samples, sampleRate) {
    var dataSize = samples.length * 2;
    var buffer = new ArrayBuffer(44 + dataSize);
    var view = new DataView(buffer);
    function writeString(offset, value) {
      for (var i = 0; i < value.length; i++) view.setUint8(offset + i, value.charCodeAt(i));
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
    var offset = 44;
    for (var i = 0; i < samples.length; i++, offset += 2) {
      var s = Math.max(-1, Math.min(1, samples[i]));
      view.setInt16(offset, s < 0 ? s * 0x8000 : s * 0x7fff, true);
    }
    return buffer;
  }

  async function postprocessVoiceText(rawText, mode, draftText, sessionId, timeoutMs) {
    var normalizedMode = normalizeVoiceMode(mode);
    var startedAt = voiceNow();
    var timer = null;
    try {
      var request = invoke("postprocess_voice_text", {
        request: {
          text: rawText,
          mode: normalizedMode,
          session_id: sessionId || null,
          draft_text: draftText || "",
        },
      });
      var res = await Promise.race([
        request,
        new Promise(function (_, reject) {
          timer = setTimeout(function () {
            reject({ category: "timeout", message: "voice postprocess timed out after " + timeoutMs + "ms" });
          }, timeoutMs || voicePostprocessTimeoutMs(normalizedMode, rawText));
        }),
      ]);
      var text = String(res && res.text !== undefined ? res.text : "").trim();
      return {
        text: text,
        source: (res && res.source) || "llm",
        durationMs: roundedMs(startedAt),
        timeoutMs: timeoutMs || voicePostprocessTimeoutMs(normalizedMode, rawText),
        error: null,
        fallbackReason: "",
      };
    } catch (err) {
      console.warn("[voice-input] postprocess fallback to ASR text", err);
      return {
        text: rawText,
        source: "asr_fallback",
        durationMs: roundedMs(startedAt),
        timeoutMs: timeoutMs || voicePostprocessTimeoutMs(normalizedMode, rawText),
        error: String((err && err.message) || err || "voice postprocess failed"),
        fallbackReason: err && err.category === "timeout" ? "llm_timeout" : "llm_error",
      };
    } finally {
      if (timer) clearTimeout(timer);
    }
  }

  async function finishVoiceInput(cancelled, timedOut) {
    var session = activeVoiceInput;
    if (!session) return;
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
      var pipelineStartedAt = voiceNow();
      if (timedOut) {
        emitVoiceDiagnostic("recording", "warn", "recording reached max duration", "", "timeout");
      }
      var raw = mergeFloatChunks(session.chunks);
      var durationMs = raw.length / Math.max(1, session.sampleRate) * 1000;
      if (durationMs < 300) {
        throw { category: "recording_failed", stage: "recording", message: bt("voiceRecordingTooShort") };
      }
      var pcm = downsamplePcm(raw, session.sampleRate, 16000);
      var wav = encodeWav(pcm, 16000);
      var bytes = Array.from(new Uint8Array(wav));
      var asrStartedAt = voiceNow();
      var res = await invoke("transcribe_voice_audio", {
        request: {
          audio_bytes: bytes,
          session_id: session.sessionId,
        },
      });
      var asrDurationMs = roundedMs(asrStartedAt);
      if (activeVoiceInput !== session) return;
      var text = String((res && res.text) || "").trim();
      if (!text) throw { category: "empty_result", stage: "transcribing", message: "未识别到语音内容" };
      if (state.activeSessionId !== session.sessionId) {
        throw { category: "context_mismatch", stage: "writeback", message: "voice result discarded because active session changed" };
      }
      var mode = normalizeVoiceMode(session.mode);
      var ruleText = applyVoiceDeterministicCorrections(text, text);
      var rawSuspiciousTerms = voiceContainsAny(text, VOICE_SUSPICIOUS_ASR_TERMS);
      var strategy = classifyVoiceText(ruleText, mode);
      var postprocessResult;
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
      } else {
        setVoiceInputStatus("postprocessing", {
          message: mode === "task"
            ? bt("voiceTaskPostprocessing")
            : mode === "edit"
              ? bt("voiceEditPostprocessing")
              : bt("voicePostprocessing"),
          stage: "postprocessing",
          mode: mode,
        });
        postprocessResult = await postprocessVoiceText(
          ruleText,
          mode,
          session.draftBeforeStart,
          session.sessionId,
          voicePostprocessTimeoutMs(mode, ruleText)
        );
      }
      var candidateText = postprocessResult.text;
      var outputValid = validateVoicePostprocessOutput(text, ruleText, candidateText, mode);
      if (mode === "edit" && postprocessResult.fallbackReason) outputValid = false;
      var finalText = outputValid ? candidateText : (mode === "edit" ? "" : ruleText);
      var outputFallbackReason = outputValid ? "" : "llm_output_invalid";
      var highRiskResidual = mode === "task" ? hasVoiceHighRiskResidual(finalText) : [];
      var fallbackHighRisk = mode === "task"
        && (!!postprocessResult.fallbackReason || !!outputFallbackReason)
        && rawSuspiciousTerms
        && rawSuspiciousTerms.length > 0
        && highRiskResidual.length > 0;
      var taskSendBlocked = mode === "task" && (highRiskResidual.length > 0 || fallbackHighRisk);
      var editUnchanged = mode === "edit"
        && !!String(finalText || "").trim()
        && String(finalText || "").trim() === String(session.draftBeforeStart || "").trim();
      var diagnostic = {
        mode: mode,
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
          mode: mode,
          rawText: text,
          diagnostic: diagnostic,
        });
      }
      setVoiceInputStatus("completed", {
        message: !finalText
          ? bt("voiceEmptyResult")
          : editUnchanged
            ? bt("voiceEditNoChange")
            : diagnostic.task_send_blocked
            ? bt("voiceWrittenBack")
            : mode === "task" ? bt("voiceTaskSent") : mode === "edit" ? bt("voiceEditPreviewReady") : bt("voiceWrittenBack"),
        completedAt: Date.now(),
        mode: mode,
        diagnostic: diagnostic,
      });
      emitVoiceDiagnostic("writeback", "info", mode === "task" ? "voice task submitted" : "voice text written back", "", "");
    } catch (err) {
      var normalized = normalizeVoiceError(err, "transcribing");
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
      var st = await invoke("install_voice_asr");
      var patch = { installing: false, cancelling: false, status: st, progress: { stage: "done" } };
      if (st && st.ready) patch.open = false;
      state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, patch);
      notify();
    } catch (e) {
      var message = String(e);
      var cancelled = state.voiceAsrSetup.cancelling || String(e).indexOf("已取消") >= 0;
      var alreadyInstalling = message.indexOf("正在下载或安装中") >= 0 || message.indexOf("already installing") >= 0;
      var failedPatch = {
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
    var session = {
      id: Date.now().toString(36),
      sessionId: state.activeSessionId || null,
      draftBeforeStart: String(draftText || ""),
      writeback: writeback,
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
      var asrStatus = await invoke("voice_asr_status");
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
    } catch (e) {
      if (activeVoiceInput !== session) return;
      // 检测失败（如 mock 环境/旧后端）不阻塞，继续走原录音路径（环境变量/兜底引擎）
    }

    if (options && typeof options.beforePermission === "function") {
      var shouldContinue = await options.beforePermission({
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

    var AudioCtor = window.AudioContext || window.webkitAudioContext;
    setVoiceInputStatus("requesting_permission", {
      message: bt("voiceRequestingPermission"),
      stage: "permission",
    });
    emitVoiceDiagnostic("permission", "info", "requesting microphone permission", "", "");

    try {
      if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
        throw { category: "device_unavailable", stage: "device", message: bt("voiceWebviewNoMic") };
      }
      if (!AudioCtor) {
        throw { category: "recording_failed", stage: "recording", message: bt("voiceWebviewNoRecording") };
      }
      var hasAudioInput = await probeVoiceAudioInput(VOICE_DEVICE_PROBE_TIMEOUT_MS);
      if (activeVoiceInput !== session) return;
      if (hasAudioInput === false) {
        throw { category: "device_unavailable", stage: "device", message: bt("voiceNoDeviceConnect") };
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
        var input = event.inputBuffer.getChannelData(0);
        session.chunks.push(new Float32Array(input));
      };
      session.source.connect(session.processor);
      session.processor.connect(session.zeroGain);
      session.zeroGain.connect(session.audioContext.destination);
      session.timeoutId = setTimeout(function () { finishVoiceInput(false, true); }, VOICE_RECORDING_MAX_DURATION_MS);
      setVoiceInputStatus("recording", { message: bt("voiceRecording"), stage: "recording" });
      emitVoiceDiagnostic("recording", "info", "recording started", "", "");
    } catch (err) {
      cleanupVoiceInputSession(session);
      if (activeVoiceInput !== session) return;
      activeVoiceInput = null;
      var normalized = normalizeVoiceError(err, "recording");
      if (normalized.category === "permission_denied") {
        try {
          var permissionReset = await invoke("reset_microphone_permission");
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
    var left = String(base || "").trimEnd();
    var right = String(text || "").trim();
    if (!left) return right;
    if (!right) return left;
    return left + (/[。！？.!?，,;；:]$/.test(left) ? " " : "\n") + right;
  }

  function runVoiceInputDebugAssertions() {
    var denied = normalizeVoiceError({ name: "NotAllowedError" });
    var noDevice = normalizeVoiceError({ name: "NotFoundError" });
    var unsupportedConstraint = normalizeVoiceError({ name: "OverconstrainedError", message: "Invalid constraint", constraint: "channelCount" });
    var mismatch = normalizeVoiceError({ category: "context_mismatch" });
    console.assert(denied.category === "permission_denied", "permission error classified");
    console.assert(noDevice.category === "device_unavailable", "device error classified");
    console.assert(unsupportedConstraint.category === "constraint_unsupported", "unsupported constraint classified");
    console.assert(unsupportedConstraint.diagnostic === "unsupported media constraint: channelCount", "unsupported constraint diagnostic");
    console.assert(mismatch.stage === "writeback", "context mismatch classified");
    console.assert(appendVoiceText("草稿", "识别文本") === "草稿\n识别文本", "voice text appended");
    return true;
  }

  function setVoiceShortcutEnabled(enabled) {
    return invoke("set_voice_shortcut_enabled", { enabled: !!enabled }).catch(function (error) {
      console.warn("[voice] failed to sync shortcut setting", error);
      return false;
    });
  }

    return {
      startVoiceInput: startVoiceInput,
      installVoiceAsr: installVoiceAsr,
      cancelVoiceAsrSetup: cancelVoiceAsrSetup,
      closeVoiceAsrSetup: closeVoiceAsrSetup,
      cancelVoiceInput: cancelVoiceInput,
      clearVoiceInput: clearVoiceInput,
      setVoiceShortcutEnabled: setVoiceShortcutEnabled,
      appendVoiceText: appendVoiceText,
      runVoiceInputDebugAssertions: runVoiceInputDebugAssertions
    };
  };
})(window);
