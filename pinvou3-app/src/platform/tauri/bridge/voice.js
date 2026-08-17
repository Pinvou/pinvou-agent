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
  var VOICE_MIN_ASR_DURATION_MS = 1200;
  var VOICE_SILENCE_RMS = 0.0025;
  var VOICE_SILENCE_PEAK = 0.015;
  var VOICE_POSTPROCESS_TIMEOUT_MS = 12000;

  function normalizeVoiceMode(mode) {
    return mode === "edit" ? "edit" : "dictation";
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
    if (rawCategory === "empty_result" || /ASR empty result|0 vad segments|no usable text/i.test(rawMessage)) {
      return { category: "empty_result", stage: rawStage, message: bt("voiceEmptyResult") };
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

  function analyzeVoiceSamples(samples) {
    var peak = 0;
    var sumSquares = 0;
    for (var i = 0; i < samples.length; i++) {
      var value = Math.abs(samples[i] || 0);
      if (value > peak) peak = value;
      sumSquares += value * value;
    }
    var rms = samples.length ? Math.sqrt(sumSquares / samples.length) : 0;
    return { peak: peak, rms: rms };
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

  function withVoiceTimeout(promise, timeoutMs, category, stage, message) {
    var timer = null;
    return Promise.race([
      promise,
      new Promise(function (_, reject) {
        timer = setTimeout(function () {
          reject({ category: category || "timeout", stage: stage || "postprocessing", message: message || bt("voicePostprocessFailed") });
        }, timeoutMs);
      }),
    ]).finally(function () {
      if (timer) clearTimeout(timer);
    });
  }

  async function postprocessVoiceText(rawText, mode, draftText, sessionId) {
    var normalizedMode = normalizeVoiceMode(mode);
    var res = await withVoiceTimeout(invoke("postprocess_voice_text", {
      request: {
        text: String(rawText || ""),
        mode: normalizedMode,
        session_id: sessionId || null,
        draft_text: String(draftText || ""),
      },
    }), VOICE_POSTPROCESS_TIMEOUT_MS, "postprocess_failed", "postprocessing", bt("voicePostprocessFailed"));
    return {
      text: String((res && res.text) || "").trim(),
      mode: normalizedMode,
      source: String((res && res.source) || ""),
    };
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
      if (timedOut) {
        emitVoiceDiagnostic("recording", "warn", "recording reached max duration", "", "timeout");
      }
      var raw = mergeFloatChunks(session.chunks);
      var durationMs = raw.length / Math.max(1, session.sampleRate) * 1000;
      if (durationMs < VOICE_MIN_ASR_DURATION_MS) {
        throw { category: "recording_failed", stage: "recording", message: bt("voiceRecordingTooShort") };
      }
      var metrics = analyzeVoiceSamples(raw);
      emitVoiceDiagnostic(
        "recording",
        "info",
        "voice sample metrics durationMs=" + Math.round(durationMs) + " peak=" + metrics.peak.toFixed(4) + " rms=" + metrics.rms.toFixed(4),
        "",
        ""
      );
      if (metrics.peak < VOICE_SILENCE_PEAK && metrics.rms < VOICE_SILENCE_RMS) {
        throw { category: "empty_result", stage: "recording", message: bt("voiceEmptyResult") };
      }
      var pcm = downsamplePcm(raw, session.sampleRate, 16000);
      var wav = encodeWav(pcm, 16000);
      var bytes = Array.from(new Uint8Array(wav));
      var res = await invoke("transcribe_voice_audio", {
        request: {
          audio_bytes: bytes,
          session_id: session.sessionId,
        },
      });
      if (activeVoiceInput !== session) return;
      var text = String((res && res.text) || "").trim();
      if (!text) throw { category: "empty_result", stage: "transcribing", message: "未识别到语音内容" };
      if (state.activeSessionId !== session.sessionId) {
        throw { category: "context_mismatch", stage: "writeback", message: "voice result discarded because active session changed" };
      }
      var mode = normalizeVoiceMode(session.mode);
      var finalText = text;
      var writebackContext = { mode: mode, rawText: text, source: "asr" };
      if (mode === "edit") {
        setVoiceInputStatus("postprocessing", { message: bt("voiceEditPostprocessing"), stage: "postprocessing", mode: mode });
        var processed = await postprocessVoiceText(text, mode, session.draftBeforeStart, session.sessionId);
        if (activeVoiceInput !== session) return;
        finalText = String((processed && processed.text) || "").trim();
        writebackContext = { mode: mode, rawText: text, source: processed.source || "llm" };
        if (!finalText || finalText === String(session.draftBeforeStart || "").trim()) {
          setVoiceInputStatus("completed", { message: bt("voiceEditNoChange"), completedAt: Date.now(), mode: mode });
          emitVoiceDiagnostic("writeback", "warn", "voice edit produced no change", bt("voiceEditNoChange"), "no_change");
          return;
        }
      }
      if (typeof session.writeback === "function") {
        session.writeback(finalText, session.draftBeforeStart, writebackContext);
      }
      setVoiceInputStatus("completed", {
        message: mode === "edit" ? bt("voiceEditApplied") : bt("voiceWrittenBack"),
        completedAt: Date.now(),
        mode: mode,
      });
      emitVoiceDiagnostic("writeback", "info", mode === "edit" ? "voice edit applied" : "voice text written back", mode === "edit" ? bt("voiceEditApplied") : "语音已写入输入框", "");
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
    state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, { installing: true, cancelling: false, error: null, progress: { stage: "start" } });
    notify();
    try {
      var st = await invoke("install_voice_asr");
      var patch = { installing: false, cancelling: false, status: st, progress: { stage: "done" } };
      if (st && st.ready) patch.open = false;
      state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, patch);
      notify();
    } catch (e) {
      var cancelled = state.voiceAsrSetup.cancelling || String(e).indexOf("已取消") >= 0;
      var failedPatch = {
        installing: false,
        cancelling: false,
        progress: cancelled ? { stage: "cancelled" } : state.voiceAsrSetup.progress,
        error: cancelled ? null : String(e),
      };
      if (cancelled) failedPatch.open = false;
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

    // 模型下载期间再次点麦克风时保留原下载会话，不能用新的依赖检测结果
    // 覆盖 installing/cancelling/progress，否则新引导框的“取消”只会关 UI，
    // 后端下载仍会继续。
    if (state.voiceAsrSetup.installing) {
      state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, { open: true });
      notify();
      return;
    }

    // 点击后立即进入可见、可取消的检测态。模型状态查询首次可能需要读取模型文件，
    // 如果等查询结束后才更新 UI，Windows 上会表现为按钮点击后没有任何反馈。
    var session = {
      id: Date.now().toString(36),
      sessionId: state.activeSessionId || null,
      draftBeforeStart: String(draftText || ""),
      mode: normalizeVoiceMode(options && options.mode),
      writeback: writeback,
      chunks: [],
      sampleRate: 16000,
      startedAt: Date.now(),
    };
    activeVoiceInput = session;
    setVoiceInputStatus("requesting_permission", {
      message: bt("voiceCheckingDevice"),
      sessionId: session.sessionId,
      mode: session.mode,
      startedAt: session.startedAt,
      stage: "device",
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
        state.voiceAsrSetup = { open: true, status: asrStatus, installing: false, cancelling: false, progress: null, error: null };
        notify();
        return;
      }
    } catch (e) {
      if (activeVoiceInput !== session) return;
      // 检测失败（如 mock 环境/旧后端）不阻塞，继续走原录音路径（环境变量/兜底引擎）
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
      session.timeoutId = setTimeout(function () { finishVoiceInput(false, true); }, 10000);
      setVoiceInputStatus("recording", { message: bt("voiceRecording"), stage: "recording", mode: session.mode });
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

    return {
      startVoiceInput: startVoiceInput,
      installVoiceAsr: installVoiceAsr,
      cancelVoiceAsrSetup: cancelVoiceAsrSetup,
      closeVoiceAsrSetup: closeVoiceAsrSetup,
      cancelVoiceInput: cancelVoiceInput,
      clearVoiceInput: clearVoiceInput,
      appendVoiceText: appendVoiceText,
      runVoiceInputDebugAssertions: runVoiceInputDebugAssertions
    };
  };
})(window);
