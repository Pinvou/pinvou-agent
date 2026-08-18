#!/usr/bin/env node
const assert = require("assert");
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const bridgePath = path.join(__dirname, "..", "src", "platform", "tauri", "bridge", "voice.js");
const source = fs.readFileSync(bridgePath, "utf8");
assert.match(source, /function encodeBase64Bytes\(bytes\)/, "desktop voice bridge must encode compact base64 audio");
assert.match(source, /audio_base64: audioBase64/, "desktop voice invoke must use compact base64 transport");
assert.doesNotMatch(source, /audio_bytes: bytes/, "desktop voice invoke must not expand WAV into a JSON integer array");
assert.match(source, /var VOICE_RECORDING_MAX_DURATION_MS = 60 \* 1000;/, "desktop recording must allow 60 seconds");
assert.match(source, /VOICE_ASR_PREWARM_DELAY_MS = 1000/, "desktop voice bridge must delay background prewarm");
assert.match(source, /invoke\("prewarm_voice_asr"\)/, "desktop recording must request guarded ASR prewarm");
assert.match(source, /clearTimeout\(session\.prewarmTimeoutId\)/, "voice cleanup must cancel pending prewarm");
assert.match(source, /root\.__PINVOU_VOICE_TIMINGS__/, "desktop voice bridge must retain full-chain timing history");
assert.match(
  source,
  /markVoiceTiming\(session, "first_pcm"[\s\S]*?scheduleVoiceTimingExport\(session\)/,
  "desktop voice bridge must persist the first captured PCM timing after the startup critical path",
);
assert.match(
  source,
  /markVoiceTiming\(session, "text_visible_in_dom"\);[\s\S]*?scheduleVoiceTimingExport\(session\)/,
  "desktop voice bridge must persist the final DOM visibility timing",
);
assert.match(
  source,
  /markVoiceTiming\(session, "text_visible_timeout"\);[\s\S]*?scheduleVoiceTimingExport\(session\)/,
  "desktop voice bridge must persist a DOM visibility timeout",
);
// voice.js 的文案走 bridge.js 的 BT_TABLE（bt(key)，按语言取词、中文兜底）；
// 这里从 bridge.js 抽出 zh 表构造 bt，保持断言面向真实文案。
const bridgeMainSource = fs.readFileSync(path.join(__dirname, "..", "src", "platform", "tauri", "bridge.js"), "utf8");
const zhTableMatch = bridgeMainSource.match(/    zh: \{([\s\S]*?)\r?\n    \},\r?\n  \};/);
assert.notStrictEqual(zhTableMatch, null, "bridge.js BT_TABLE zh block must exist");
const zhTable = new Function(`return ({${zhTableMatch[1]}});`)();
const bt = (key) => zhTable[key] !== undefined ? zhTable[key] : key;
const start = source.indexOf("  function normalizeVoiceError(err, fallbackStage) {");
const end = source.indexOf("\n  function stopMediaTracks(", start);

assert.notStrictEqual(start, -1, "normalizeVoiceError must exist");
assert.notStrictEqual(end, -1, "normalizeVoiceError boundary must exist");

const context = { bt };
vm.createContext(context);
vm.runInContext(`${source.slice(start, end)}\nthis.normalizeVoiceError = normalizeVoiceError;`, context, {
  filename: bridgePath,
});

const { normalizeVoiceError } = context;

const denied = normalizeVoiceError({ name: "NotAllowedError" });
assert.strictEqual(denied.category, "permission_denied");

const missingDevice = normalizeVoiceError({ name: "NotFoundError" });
assert.strictEqual(missingDevice.category, "device_unavailable");
assert.match(missingDevice.message, /未检测到可用麦克风/);

const unsupportedConstraint = normalizeVoiceError({
  name: "OverconstrainedError",
  constraint: "channelCount",
});
assert.strictEqual(unsupportedConstraint.category, "constraint_unsupported");
assert.match(unsupportedConstraint.message, /不支持所需的录音配置/);
assert.strictEqual(unsupportedConstraint.diagnostic, "unsupported media constraint: channelCount");

const invalidConstraint = normalizeVoiceError({ message: "Invalid constraint: noiseSuppression" });
assert.strictEqual(invalidConstraint.category, "constraint_unsupported");

const deviceTimeout = normalizeVoiceError({
  category: "device_unavailable",
  stage: "device",
  message: "麦克风检测超时",
});
assert.strictEqual(deviceTimeout.category, "device_unavailable");
assert.match(deviceTimeout.message, /检测超时/);

const mediaStart = source.indexOf("  function stopMediaTracks(");
const mediaEnd = source.indexOf("\n  function mergeFloatChunks(", mediaStart);
assert.notStrictEqual(mediaStart, -1, "voice media helpers must exist");
assert.notStrictEqual(mediaEnd, -1, "voice media helper boundary must exist");

let getUserMedia = () => new Promise(() => {});
const mediaContext = {
  bt,
  navigator: {
    mediaDevices: {
      getUserMedia: (...args) => getUserMedia(...args),
    },
  },
  setTimeout,
  clearTimeout,
};
vm.createContext(mediaContext);
vm.runInContext(
  `${source.slice(mediaStart, mediaEnd)}\nthis.requestVoiceMedia = requestVoiceMedia;`,
  mediaContext,
  { filename: bridgePath },
);

const timingStart = source.indexOf("  function voicePerfNow() {");
const timingEnd = source.indexOf("\n  function setVoiceInputStatus(", timingStart);
assert.notStrictEqual(timingStart, -1, "voice timing helpers must exist");
assert.notStrictEqual(timingEnd, -1, "voice timing helper boundary must exist");
let perfNow = 100;
const timingFrames = [];
const timingTimers = [];
const timingInvocations = [];
const timingRoot = {
  performance: {
    timeOrigin: 1_000,
    now: () => { perfNow += 0.25; return perfNow; },
  },
  requestAnimationFrame(callback) { timingFrames.push(callback); },
};
const timingContext = {
  window: timingRoot,
  root: timingRoot,
  setTimeout(callback) { timingTimers.push(callback); },
  invoke(command, args) {
    timingInvocations.push({ command, args });
    return Promise.resolve();
  },
};
vm.createContext(timingContext);
vm.runInContext(
  `${source.slice(timingStart, timingEnd)}\nthis.beginVoiceTiming = beginVoiceTiming; this.markVoiceTiming = markVoiceTiming; this.scheduleVoiceTimingExport = scheduleVoiceTimingExport;`,
  timingContext,
  { filename: bridgePath },
);
let latestTimingSession = null;
for (let index = 0; index < 12; index += 1) {
  const startedPerf = timingRoot.performance.now();
  const session = { id: `run-${index}` };
  timingContext.beginVoiceTiming(session, startedPerf);
  timingContext.markVoiceTiming(session, "recording_state", { index });
  latestTimingSession = session;
}
assert.strictEqual(timingRoot.__PINVOU_VOICE_TIMINGS__.length, 10, "timing history must stay bounded");
assert.strictEqual(timingRoot.__PINVOU_VOICE_TIMINGS__[0].run_id, "run-2", "timing history must retain the newest runs");
assert.strictEqual(timingRoot.__PINVOU_VOICE_TIMING__.run_id, "run-11", "latest timing pointer must follow the newest run");
assert.deepStrictEqual(
  Array.from(timingRoot.__PINVOU_VOICE_TIMING__.events, event => event.name),
  ["click_start", "recording_state"],
);
assert.ok(Number.isFinite(timingRoot.__PINVOU_VOICE_TIMING__.events[0].epoch_ms));
assert.ok(Number.isFinite(timingRoot.__PINVOU_VOICE_TIMING__.events[0].from_click_ms));
timingContext.scheduleVoiceTimingExport(latestTimingSession);
timingContext.scheduleVoiceTimingExport(latestTimingSession);
assert.strictEqual(timingFrames.length, 1, "one voice round must schedule at most one timing export frame");
assert.strictEqual(timingInvocations.length, 0, "timing export must not run on the measured critical path");
timingFrames.shift()();
assert.strictEqual(timingTimers.length, 1, "timing export must yield once more after the frame");
assert.strictEqual(timingInvocations.length, 0);
timingTimers.shift()();
assert.strictEqual(timingInvocations.length, 1, "the deferred timing batch must be persisted once");
assert.strictEqual(timingInvocations[0].command, "report_frontend_startup");
assert.deepStrictEqual(
  Array.from(timingInvocations[0].args.entries, entry => entry.stage),
  ["voice:click_start", "voice:recording_state"],
  "voice timings must be exported with a dedicated namespace",
);
assert.ok(
  timingInvocations[0].args.entries.every(entry => entry.detail.includes("run_id=run-11")),
  "persisted timing details must carry a run id for per-round correlation",
);
timingContext.markVoiceTiming(latestTimingSession, "first_pcm", { samples: 4096 });
timingContext.scheduleVoiceTimingExport(latestTimingSession);
timingFrames.shift()();
timingTimers.shift()();
assert.strictEqual(timingInvocations.length, 2, "subsequent exports must persist a new incremental batch");
assert.deepStrictEqual(
  Array.from(timingInvocations[1].args.entries, entry => entry.stage),
  ["voice:first_pcm"],
);
assert.match(timingInvocations[1].args.entries[0].detail, /data=\{"samples":4096\}/);

(async () => {
  let resolveLateStream;
  let stoppedTracks = 0;
  getUserMedia = () => new Promise((resolve) => { resolveLateStream = resolve; });
  const session = {};
  mediaContext.activeVoiceInput = session;
  await assert.rejects(
    mediaContext.requestVoiceMedia(session, { audio: true }, 20),
    (error) => error && error.category === "device_unavailable" && /检测超时/.test(error.message),
  );
  resolveLateStream({
    getTracks: () => [{ stop: () => { stoppedTracks += 1; } }],
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.strictEqual(stoppedTracks, 1, "late microphone stream must be stopped after timeout");

  let resolveAsrStatus;
  let stoppedRacingTracks = 0;
  let audioGraphCreated = 0;
  const racingStream = {
    getTracks: () => [{ stop: () => { stoppedRacingTracks += 1; } }],
  };
  class FakeAudioContext {
    constructor() {
      this.sampleRate = 48_000;
      this.state = "running";
    }
    close() { this.state = "closed"; return Promise.resolve(); }
    createMediaStreamSource() { audioGraphCreated += 1; return { connect() {}, disconnect() {} }; }
    createScriptProcessor() { return { connect() {}, disconnect() {}, onaudioprocess: null }; }
    createGain() { return { gain: { value: 1 }, connect() {}, disconnect() {} }; }
    get destination() { return {}; }
  }
  const racingWindow = {
    __PINVOU_TAURI_BRIDGE_FEATURES__: {},
    AudioContext: FakeAudioContext,
    performance: { timeOrigin: Date.now(), now: () => 1 },
    requestAnimationFrame: () => 0,
    btoa: () => "",
  };
  const racingState = {
    activeSessionId: "session-1",
    voiceInput: { status: "idle" },
    voiceAsrSetup: { installing: false, cancelling: false },
  };
  const racingContext = {
    window: racingWindow,
    navigator: { mediaDevices: { getUserMedia: async () => racingStream } },
    document: { querySelectorAll: () => [] },
    console: { info() {}, warn() {}, error() {}, assert() {} },
    setTimeout,
    clearTimeout,
    Float32Array,
    Uint8Array,
    ArrayBuffer,
    DataView,
  };
  vm.createContext(racingContext);
  vm.runInContext(source, racingContext, { filename: bridgePath });
  const voiceFeature = racingWindow.__PINVOU_TAURI_BRIDGE_FEATURES__.voice({
    state: racingState,
    notify() {},
    bt: key => key,
    invoke(command) {
      if (command === "voice_asr_status") {
        return new Promise(resolve => { resolveAsrStatus = resolve; });
      }
      return Promise.resolve(null);
    },
  });
  const racingStart = voiceFeature.startVoiceInput("", () => {});
  assert.strictEqual(stoppedRacingTracks, 0, "the same-turn race must begin with a live microphone request");
  resolveAsrStatus({ ready: false });
  await racingStart;
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.strictEqual(stoppedRacingTracks, 1, "same-turn ASR setup rejection must stop the acquired microphone stream");
  assert.strictEqual(audioGraphCreated, 0, "ASR setup rejection must not build the recording graph");
  assert.strictEqual(racingState.voiceInput.status, "idle");

  const cancelledStart = voiceFeature.startVoiceInput("", () => {});
  queueMicrotask(() => voiceFeature.cancelVoiceInput());
  await Promise.resolve();
  resolveAsrStatus({ ready: true });
  await cancelledStart;
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.strictEqual(stoppedRacingTracks, 2, "microtask-window cancellation must stop the acquired microphone stream exactly once");
  assert.strictEqual(audioGraphCreated, 0, "a cancelled status query must not resume recording graph creation");
  assert.strictEqual(racingState.voiceInput.status, "cancelled");

  const chatPath = path.join(__dirname, "..", "src", "features", "chat", "ChatView.jsx");
  const chatSource = fs.readFileSync(chatPath, "utf8");
  assert.match(chatSource, /const voiceBusy = voiceInput\.status === 'transcribing'/);
  assert.match(
    chatSource,
    /if \(voiceInput\.status === 'requesting_permission'\) \{[\s\S]*?bridge\.voice\.cancelVoiceInput\(\);[\s\S]*?return;/,
  );

  const startVoiceInputAt = source.indexOf("  async function startVoiceInput(");
  const installStatusAt = source.indexOf('var asrStatusPromise = invoke("voice_asr_status")', startVoiceInputAt);
  const mediaRequestAt = source.indexOf("var mediaOutcomePromise = requestVoiceMedia", startVoiceInputAt);
  const installStatusAwaitAt = source.indexOf("var asrStatus = await asrStatusPromise", startVoiceInputAt);
  const requestingStatusAt = source.indexOf('setVoiceInputStatus("requesting_permission"', startVoiceInputAt);
  const activeSessionAt = source.indexOf("activeVoiceInput = session", startVoiceInputAt);
  assert.ok(startVoiceInputAt >= 0 && installStatusAt >= 0, "voice input start flow must exist");
  assert.ok(activeSessionAt < installStatusAt, "voice session must become cancellable before dependency status query");
  assert.ok(requestingStatusAt < installStatusAt, "voice input must show immediate feedback before dependency status query");
  assert.ok(mediaRequestAt > installStatusAt, "microphone request must start after the parallel ASR status request");
  assert.ok(mediaRequestAt < installStatusAwaitAt, "microphone request must not wait for the ASR status response");
  assert.match(
    source.slice(mediaStart, mediaEnd),
    /getUserMedia\(constraints\)\.then\(function \(stream\) \{[\s\S]*?session\.stream = stream;[\s\S]*?return stream;/,
    "getUserMedia must transfer stream ownership to the cancellable session in its first success microtask",
  );
  assert.doesNotMatch(
    source.slice(startVoiceInputAt, source.indexOf("\n  function cancelVoiceInput(", startVoiceInputAt)),
    /\.enumerateDevices\(|probeVoiceAudioInput\(/,
    "voice startup must not perform a redundant device probe before getUserMedia",
  );

  const permissionCatchAt = source.indexOf('if (normalized.category === "permission_denied")', startVoiceInputAt);
  assert.ok(permissionCatchAt > startVoiceInputAt, "permission denial recovery must exist in voice input flow");
  assert.match(
    source.slice(permissionCatchAt, permissionCatchAt + 900),
    /await invoke\("reset_microphone_permission"\)/,
    "Windows WebView2 microphone denial must reset the saved permission before retry",
  );
  assert.match(
    source.slice(permissionCatchAt, permissionCatchAt + 900),
    /bt\("voicePermissionDeniedRetry"\)/,
    "permission denial must tell the user how to trigger the prompt again",
  );
  assert.match(
    zhTable.voicePermissionDeniedRetry,
    /请再次点击语音输入并在授权提示中选择允许/,
    "permission denial retry hint must keep the actionable guidance",
  );

  console.log("voice_input_error_logic: ok");
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
