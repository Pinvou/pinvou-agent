/**
 * voice feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: classic script 直拷产物,严格模式是载荷
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: 直拷载荷的注册表引导,拆分语句会偏离产物原貌
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

  function normalizeVoiceError(err, fallbackStage) {
    const name = String((err && err.name) || "");
    const rawCategory = (err && err.category) || "";
    const rawStage = (err && err.stage) || fallbackStage || "recording";
    const rawMessage = String((err && (err.message || err.toString && err.toString())) || err || "");
    const constraint = String((err && err.constraint) || "");
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
    stream.getTracks().forEach(function (track) { try { track.stop(); } catch { /* 已停止的轨道无需处理 */ } });
  }

  // 语音流程的错误载体:Error 实例 + category/stage 附加字段,供 normalizeVoiceError 分类。
  // (原实现抛裸对象字面量,违反 no-throw-literal;此处收拢为 Error 工厂,
  // normalizeVoiceError 的分类字段 category/stage/message 语义不变。)
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
      try { cancelPermissionRequest(); } catch { /* 取消回调异常不阻断清理 */ }
    }
    // 先摘掉音频回调：webkit2gtk 的 WebAudio 是 GStreamer 后端，ScriptProcessorNode 的
    // onaudioprocess 跑在音频线程，若在 disconnect/close 期间再触发一次、访问已释放的
    // 缓冲，会让 WebProcess 段错误（表现为「识别出文字后 app 崩溃」）。务必先置 null。
    try { if (session.processor) session.processor.onaudioprocess = null; } catch { /* 释放失败仅影响本页音频 */ }
    try { if (session.processor) session.processor.disconnect(); } catch { /* 释放失败仅影响本页音频 */ }
    try { if (session.source) session.source.disconnect(); } catch { /* 释放失败仅影响本页音频 */ }
    try { if (session.zeroGain) session.zeroGain.disconnect(); } catch { /* 释放失败仅影响本页音频 */ }
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
      setTimeout(function () { try { ctx.close().catch(function () {}); } catch { /* 音频上下文已关闭 */ } }, 0);
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
      // WAV 头只写 ASCII,charCode 即目标字节值;fromCodePoint/codePointAt 在此无增益。
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

  async function finishVoiceInput(cancelled, timedOut) {
    const session = activeVoiceInput;
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
      const raw = mergeFloatChunks(session.chunks);
      const durationMs = raw.length / Math.max(1, session.sampleRate) * 1000;
      if (durationMs < 300) {
        throw voiceFlowError("recording_failed", "recording", bt("voiceRecordingTooShort"));
      }
      const pcm = downsamplePcm(raw, session.sampleRate, 16000);
      const wav = encodeWav(pcm, 16000);
      const bytes = [...new Uint8Array(wav)];
      const res = await invoke("transcribe_voice_audio", {
        request: {
          audio_bytes: bytes,
          session_id: session.sessionId,
        },
      });
      if (activeVoiceInput !== session) return;
      const text = String((res && res.text) || "").trim();
      if (!text) throw voiceFlowError("empty_result", "transcribing", "未识别到语音内容");
      if (state.activeSessionId !== session.sessionId) {
        throw voiceFlowError("context_mismatch", "writeback", "voice result discarded because active session changed");
      }
      if (typeof session.writeback === "function") {
        session.writeback(text, session.draftBeforeStart);
      }
      setVoiceInputStatus("completed", { message: bt("voiceWrittenBack"), completedAt: Date.now() });
      emitVoiceDiagnostic("writeback", "info", "voice text written back", "语音已写入输入框", "");
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
    state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, { installing: true, cancelling: false, error: null, progress: { stage: "start" } });
    notify();
    try {
      const st = await invoke("install_voice_asr");
      const patch = { installing: false, cancelling: false, status: st, progress: { stage: "done" } };
      if (st && st.ready) patch.open = false;
      state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, patch);
      notify();
    } catch (e) {
      const cancelled = state.voiceAsrSetup.cancelling || String(e).includes("已取消");
      const failedPatch = {
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
    const session = {
      id: Date.now().toString(36),
      sessionId: state.activeSessionId || null,
      draftBeforeStart: String(draftText || ""),
      writeback,
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
        state.voiceAsrSetup = { open: true, status: asrStatus, installing: false, cancelling: false, progress: null, error: null };
        notify();
        return;
      }
    } catch {
      if (activeVoiceInput !== session) return;
      // 检测失败（如 mock 环境/旧后端）不阻塞，继续走原录音路径（环境变量/兜底引擎）
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
      session.timeoutId = setTimeout(function () { finishVoiceInput(false, true); }, 10000);
      setVoiceInputStatus("recording", { message: bt("voiceRecording"), stage: "recording" });
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

    return {
      startVoiceInput,
      installVoiceAsr,
      cancelVoiceAsrSetup,
      closeVoiceAsrSetup,
      cancelVoiceInput,
      clearVoiceInput,
      appendVoiceText,
      runVoiceInputDebugAssertions
    };
  };
})(window);
