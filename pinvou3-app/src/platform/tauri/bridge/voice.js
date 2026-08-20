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
  var VOICE_DEVICE_REQUEST_TIMEOUT_MS = 8000;
  var VOICE_RECORDING_MAX_DURATION_MS = 60 * 1000;
  var VOICE_ASR_PREWARM_DELAY_MS = 1000;

  function voicePerfNow() {
    return window.performance && typeof window.performance.now === "function"
      ? window.performance.now()
      : Date.now();
  }

  function beginVoiceTiming(session, startedPerf) {
    var timing = {
      schema: 1,
      run_id: session.id,
      time_origin_ms: window.performance && Number.isFinite(window.performance.timeOrigin)
        ? window.performance.timeOrigin
        : Date.now() - voicePerfNow(),
      started_perf_ms: startedPerf,
      events: [],
    };
    session.timing = timing;
    var history = root.__PINVOU_VOICE_TIMINGS__ = root.__PINVOU_VOICE_TIMINGS__ || [];
    history.push(timing);
    if (history.length > 10) history.splice(0, history.length - 10);
    root.__PINVOU_VOICE_TIMING__ = timing;
    markVoiceTiming(session, "click_start");
  }

  function markVoiceTiming(session, name, detail) {
    if (!session || !session.timing) return;
    var now = voicePerfNow();
    session.timing.events.push({
      name: name,
      epoch_ms: Math.round((session.timing.time_origin_ms + now) * 1000) / 1000,
      from_click_ms: Math.round((now - session.timing.started_perf_ms) * 1000) / 1000,
      detail: detail || null,
    });
  }

  function exportVoiceTiming(session) {
    if (!session || !session.timing) return;
    var timing = session.timing;
    var exportedCount = Math.max(0, Number(timing.exported_event_count) || 0);
    var pending = timing.events.slice(exportedCount);
    if (!pending.length) return;
    timing.exported_event_count = timing.events.length;
    var entries = pending.map(function (event) {
      var detail = "run_id=" + timing.run_id + " from_click_ms=" + event.from_click_ms.toFixed(3);
      if (event.detail != null) {
        try { detail += " data=" + JSON.stringify(event.detail); } catch (_) {}
      }
      return {
        stage: "voice:" + event.name,
        since_navigation_ms: timing.started_perf_ms + event.from_click_ms,
        detail: detail,
      };
    });
    // 直接复用 Rust 的批量性能日志命令，不把长会话的语音 marks 堆进
    // window.__PINVOU_STARTUP__.entries；日志只含时间、尺寸和错误类别。
    try {
      var request = invoke("report_frontend_startup", { entries: entries });
      if (request && typeof request.catch === "function") request.catch(function () {});
    } catch (_) {}
  }

  // 等录音态先完成一次渲染，再批量落盘本轮性能点。这样既不扰动
  // click → recording 的待测关键路径，也不会把语音文本写入诊断日志。
  function scheduleVoiceTimingExport(session) {
    if (!session || session.voiceTimingExportScheduled) return;
    session.voiceTimingExportScheduled = true;
    var exportAfterFrame = function () {
      setTimeout(function () {
        session.voiceTimingExportScheduled = false;
        exportVoiceTiming(session);
      }, 0);
    };
    if (typeof window.requestAnimationFrame === "function") {
      window.requestAnimationFrame(exportAfterFrame);
    } else {
      exportAfterFrame();
    }
  }

  function watchVoiceTextPaint(session, text) {
    if (!text || typeof window.requestAnimationFrame !== "function") return;
    var started = voicePerfNow();
    function check() {
      var controls = document.querySelectorAll('[data-testid="chat-composer-input"], textarea');
      for (var i = 0; i < controls.length; i++) {
        if (String(controls[i].value || "").indexOf(text) >= 0) {
          markVoiceTiming(session, "text_visible_in_dom");
          scheduleVoiceTimingExport(session);
          return;
        }
      }
      var utterances = document.querySelectorAll('.pinvou-os-utterance');
      for (var j = 0; j < utterances.length; j++) {
        if (String(utterances[j].textContent || "").indexOf(text) >= 0) {
          markVoiceTiming(session, "text_visible_in_dom");
          scheduleVoiceTimingExport(session);
          return;
        }
      }
      if (voicePerfNow() - started < 2000) {
        window.requestAnimationFrame(check);
      } else {
        markVoiceTiming(session, "text_visible_timeout");
        scheduleVoiceTimingExport(session);
      }
    }
    window.requestAnimationFrame(check);
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
    if (rawCategory === "empty_result") {
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
    if (session.prewarmTimeoutId) clearTimeout(session.prewarmTimeoutId);
    session.prewarmTimeoutId = null;
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

  function requestVoiceMedia(session, constraints, timeoutMs) {
    var abandoned = false;
    var mediaPromise = navigator.mediaDevices.getUserMedia(constraints).then(function (stream) {
      if (abandoned || activeVoiceInput !== session) {
        stopMediaTracks(stream);
        throw { category: "cancelled", stage: "permission", message: bt("voiceCancelled") };
      }
      // 在 getUserMedia 的首个成功微任务里立即取得 stream 所有权。若先把 stream
      // 返回给外层 Promise，再由外层挂到 session，状态查询/取消可能插进两个微任务
      // 之间，cleanup 看不到已经打开的 track，造成活麦克风泄漏。
      session.stream = stream;
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

  function encodeBase64Bytes(bytes) {
    var binary = "";
    var chunkSize = 0x8000;
    for (var offset = 0; offset < bytes.length; offset += chunkSize) {
      binary += String.fromCharCode.apply(
        null,
        bytes.subarray(offset, Math.min(offset + chunkSize, bytes.length))
      );
    }
    return window.btoa(binary);
  }

  async function finishVoiceInput(cancelled, timedOut) {
    var session = activeVoiceInput;
    if (!session) return;
    if (cancelled) {
      markVoiceTiming(session, "cancel_click");
      cleanupVoiceInputSession(session);
      activeVoiceInput = null;
      setVoiceInputStatus("cancelled", { message: bt("voiceCancelled"), completedAt: Date.now() });
      scheduleVoiceTimingExport(session);
      emitVoiceDiagnostic("recording", "info", "voice input cancelled", "已取消语音输入", "cancelled");
      return;
    }

    markVoiceTiming(session, "transcribing_state_start");
    setVoiceInputStatus("transcribing", { message: bt("voiceTranscribing"), stage: "transcribing" });
    markVoiceTiming(session, "audio_cleanup_start");
    cleanupVoiceInputSession(session);
    markVoiceTiming(session, "audio_cleanup_end");

    try {
      if (timedOut) {
        emitVoiceDiagnostic("recording", "warn", "recording reached max duration", "", "timeout");
      }
      markVoiceTiming(session, "pcm_merge_start", { chunks: session.chunks.length });
      var raw = mergeFloatChunks(session.chunks);
      var durationMs = raw.length / Math.max(1, session.sampleRate) * 1000;
      markVoiceTiming(session, "pcm_merge_end", { samples: raw.length, audio_ms: Math.round(durationMs) });
      if (durationMs < 300) {
        throw { category: "recording_failed", stage: "recording", message: bt("voiceRecordingTooShort") };
      }
      markVoiceTiming(session, "downsample_start", { source_rate: session.sampleRate });
      var pcm = downsamplePcm(raw, session.sampleRate, 16000);
      markVoiceTiming(session, "downsample_end", { samples: pcm.length });
      markVoiceTiming(session, "wav_encode_start");
      var wav = encodeWav(pcm, 16000);
      markVoiceTiming(session, "wav_encode_end", { bytes: wav.byteLength });
      markVoiceTiming(session, "base64_encode_start");
      var audioBase64 = encodeBase64Bytes(new Uint8Array(wav));
      markVoiceTiming(session, "base64_encode_end", { chars: audioBase64.length });
      markVoiceTiming(session, "tauri_invoke_start");
      var res = await invoke("transcribe_voice_audio", {
        request: {
          audio_base64: audioBase64,
          session_id: session.sessionId,
        },
      });
      markVoiceTiming(session, "tauri_invoke_end");
      if (activeVoiceInput !== session) return;
      var text = String((res && res.text) || "").trim();
      markVoiceTiming(session, "text_parsed");
      if (!text) throw { category: "empty_result", stage: "transcribing", message: "未识别到语音内容" };
      if (state.activeSessionId !== session.sessionId) {
        throw { category: "context_mismatch", stage: "writeback", message: "voice result discarded because active session changed" };
      }
      if (typeof session.writeback === "function") {
        markVoiceTiming(session, "writeback_start");
        session.writeback(text, session.draftBeforeStart);
        markVoiceTiming(session, "writeback_end");
        watchVoiceTextPaint(session, text);
      }
      setVoiceInputStatus("completed", { message: bt("voiceWrittenBack"), completedAt: Date.now() });
      markVoiceTiming(session, "completed_state");
      scheduleVoiceTimingExport(session);
      emitVoiceDiagnostic("writeback", "info", "voice text written back", "语音已写入输入框", "");
    } catch (err) {
      // Cancellation clears activeVoiceInput and owns the terminal `cancelled`
      // state. A late ASR rejection must not overwrite it with `failed`.
      if (activeVoiceInput !== session) return;
      var normalized = normalizeVoiceError(err, "transcribing");
      markVoiceTiming(session, "failed", { category: normalized.category, stage: normalized.stage });
      setVoiceInputStatus("failed", {
        message: normalized.message,
        error: normalized.message,
        category: normalized.category,
        stage: normalized.stage,
        completedAt: Date.now(),
      });
      scheduleVoiceTimingExport(session);
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


  async function startVoiceInput(draftText, writeback) {
    var clickPerf = voicePerfNow();
    if (activeVoiceInput && state.voiceInput.status === "recording") {
      markVoiceTiming(activeVoiceInput, "stop_click");
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

    // 点击后立即进入可见、可取消的授权态。ASR 状态检查、麦克风打开和 AudioContext
    // 启动并行进行，避免每次录音前串行等待；getUserMedia 会直接报告无设备，
    // 因此不再额外调用 enumerateDevices。
    var session = {
      id: Date.now().toString(36),
      sessionId: state.activeSessionId || null,
      draftBeforeStart: String(draftText || ""),
      writeback: writeback,
      chunks: [],
      sampleRate: 16000,
      startedAt: Date.now(),
    };
    beginVoiceTiming(session, clickPerf);
    activeVoiceInput = session;
    setVoiceInputStatus("requesting_permission", {
      message: bt("voiceRequestingPermission"),
      sessionId: session.sessionId,
      startedAt: session.startedAt,
      stage: "permission",
    });
    markVoiceTiming(session, "requesting_permission_state");
    emitVoiceDiagnostic("permission", "info", "requesting microphone permission", "", "");
    var AudioCtor = window.AudioContext || window.webkitAudioContext;

    try {
      if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
        throw { category: "device_unavailable", stage: "device", message: bt("voiceWebviewNoMic") };
      }
      if (!AudioCtor) {
        throw { category: "recording_failed", stage: "recording", message: bt("voiceWebviewNoRecording") };
      }

      markVoiceTiming(session, "asr_status_request");
      var asrStatusPromise = invoke("voice_asr_status").then(function (status) {
        markVoiceTiming(session, "asr_status_response", { ready: !!(status && status.ready) });
        return status;
      }).catch(function () {
        markVoiceTiming(session, "asr_status_error");
        return null;
      });
      markVoiceTiming(session, "microphone_request");
      var mediaOutcomePromise = requestVoiceMedia(session, {
        audio: {
          channelCount: 1,
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
        },
      }, VOICE_DEVICE_REQUEST_TIMEOUT_MS).then(
        function () {
          markVoiceTiming(session, "microphone_response");
          return { error: null };
        },
        function (error) {
          markVoiceTiming(session, "microphone_error");
          return { error: error };
        }
      );
      markVoiceTiming(session, "audio_context_create_start");
      session.audioContext = new AudioCtor();
      markVoiceTiming(session, "audio_context_create_end", { sample_rate: session.audioContext.sampleRate });

      // 首次/缺组件：状态检查与硬件启动并行；缺组件则立刻取消麦克风请求。
      var asrStatus = await asrStatusPromise;
      if (activeVoiceInput !== session) {
        cleanupVoiceInputSession(session);
        return;
      }
      if (asrStatus && !asrStatus.ready) {
        cleanupVoiceInputSession(session);
        activeVoiceInput = null;
        setVoiceInputStatus("idle", { message: "", stage: null, sessionId: null });
        state.voiceAsrSetup = { open: true, status: asrStatus, installing: false, cancelling: false, progress: null, error: null };
        notify();
        return;
      }

      var mediaOutcome = await mediaOutcomePromise;
      if (mediaOutcome.error) throw mediaOutcome.error;
      if (activeVoiceInput !== session) {
        cleanupVoiceInputSession(session);
        return;
      }
      session.sampleRate = session.audioContext.sampleRate || 16000;
      markVoiceTiming(session, "audio_graph_build_start");
      session.source = session.audioContext.createMediaStreamSource(session.stream);
      session.processor = session.audioContext.createScriptProcessor(4096, 1, 1);
      session.zeroGain = session.audioContext.createGain();
      session.zeroGain.gain.value = 0;
      session.processor.onaudioprocess = function (event) {
        if (activeVoiceInput !== session) return;
        var input = event.inputBuffer.getChannelData(0);
        if (!session.firstPcmSeen) {
          session.firstPcmSeen = true;
          markVoiceTiming(session, "first_pcm", { samples: input.length });
          scheduleVoiceTimingExport(session);
        }
        session.chunks.push(new Float32Array(input));
      };
      session.source.connect(session.processor);
      session.processor.connect(session.zeroGain);
      session.zeroGain.connect(session.audioContext.destination);
      markVoiceTiming(session, "audio_graph_build_end");
      session.timeoutId = setTimeout(function () { finishVoiceInput(false, true); }, VOICE_RECORDING_MAX_DURATION_MS);
      setVoiceInputStatus("recording", { message: bt("voiceRecording"), stage: "recording" });
      markVoiceTiming(session, "recording_state");
      session.prewarmTimeoutId = setTimeout(function () {
        session.prewarmTimeoutId = null;
        if (activeVoiceInput !== session || state.voiceInput.status !== "recording") return;
        markVoiceTiming(session, "asr_prewarm_start");
        invoke("prewarm_voice_asr").then(function (warmed) {
          markVoiceTiming(session, warmed ? "asr_prewarm_end" : "asr_prewarm_skipped");
        }).catch(function (error) {
          markVoiceTiming(session, "asr_prewarm_error");
        });
      }, VOICE_ASR_PREWARM_DELAY_MS);
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
      scheduleVoiceTimingExport(session);
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
