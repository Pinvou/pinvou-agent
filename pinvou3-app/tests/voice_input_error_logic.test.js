#!/usr/bin/env node
const assert = require("assert");
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const bridgePath = path.join(__dirname, "..", "src", "platform", "tauri", "bridge", "voice.js");
const source = fs.readFileSync(bridgePath, "utf8");
const chatPath = path.join(__dirname, "..", "src", "features", "chat", "ChatView.jsx");
const chatSource = fs.readFileSync(chatPath, "utf8");
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

const ruleStart = source.indexOf("  function normalizeVoiceMode(mode) {");
const ruleEnd = source.indexOf("\n  function logVoicePipeline(", ruleStart);
assert.notStrictEqual(ruleStart, -1, "voice normalize strategy helpers must exist");
assert.notStrictEqual(ruleEnd, -1, "voice normalize strategy helper boundary must exist");
const ruleContext = { performance: { now: () => 0 }, Date };
vm.createContext(ruleContext);
vm.runInContext(
  `${source.slice(ruleStart, ruleEnd)}
this.classifyVoiceText = classifyVoiceText;
this.hasVoiceHighRiskResidual = hasVoiceHighRiskResidual;
this.applyVoiceDeterministicCorrections = applyVoiceDeterministicCorrections;
this.voicePostprocessTimeoutMs = voicePostprocessTimeoutMs;`,
  ruleContext,
  { filename: bridgePath },
);
assert.deepStrictEqual(
  ruleContext.classifyVoiceText("嗯。", "dictation").strategy,
  "skip_empty",
  "pure filler must be dropped before LLM",
);
assert.deepStrictEqual(
  ruleContext.classifyVoiceText("今天天气怎么样？", "dictation").strategy,
  "use_asr",
  "short clear dictation should skip LLM",
);
assert.deepStrictEqual(
  ruleContext.classifyVoiceText("查一下今日进价，并生成数据分析图标。", "task").strategy,
  "run_llm",
  "task mode with suspicious ASR terms should run LLM",
);
assert.deepStrictEqual(
  Array.from(ruleContext.hasVoiceHighRiskResidual("查一下今日进价。")),
  ["进价"],
  "task send guard must detect high-risk residual terms",
);
assert.strictEqual(
  ruleContext.applyVoiceDeterministicCorrections("比较一下 g p t five 和克劳德 sonnet 的代码嫩力。", ""),
  "比较一下 GPT-5 和 Claude Sonnet 的代码能力。",
);
assert.strictEqual(
  ruleContext.applyVoiceDeterministicCorrections("把这段内容整理成表格，列出负责任、截止事件和状态。", ""),
  "把这段内容整理成表格，列出负责人、截止时间和状态。",
);
assert.strictEqual(ruleContext.voicePostprocessTimeoutMs("task", "查一下今日金价。"), 2500);
assert.doesNotMatch(source, /raw_text:\s*text/, "voice diagnostics must not persist raw ASR text");
assert.doesNotMatch(source, /final_text:\s*finalText/, "voice diagnostics must not persist postprocessed text");
assert.match(source, /raw_text_length:\s*text\.length/, "voice diagnostics should retain raw text length");
assert.match(source, /final_text_length:\s*finalText\.length/, "voice diagnostics should retain final text length");
assert.match(
  source,
  /fallbackHighRisk[\s\S]*taskSendBlocked[\s\S]*fallbackHighRisk/,
  "task mode must block sending when LLM fallback leaves high-risk ASR terms",
);
assert.match(
  chatSource,
  /if \(!voiceShortcutEnabledRef\.current && !\(event && event\.key === 'Escape'\)\) return;/,
  "web voice shortcut keydown must be gated by explicit user opt-in",
);
assert.match(
  chatSource,
  /if \(!voiceShortcutEnabledRef\.current\) \{[\s\S]*?clearPendingShortcutFlag\('space'\);[\s\S]*?return;[\s\S]*?\}/,
  "web voice shortcut keyup must ignore disabled shortcuts and clear pending space state",
);
assert.match(
  chatSource,
  /voiceShortcutPendingRef\.current = null;\s*if \(!voiceShortcutEnabledRef\.current\) return;[\s\S]*?const payload/,
  "native voice shortcut trigger must be gated by explicit user opt-in",
);
assert.match(
  chatSource,
  /pendingVoiceAfterIntroRef = useRef\(null\)/,
  "mic-click intro must keep a pending voice action",
);
assert.match(
  chatSource,
  /if \(!voiceIntroSeenState && !voiceShortcutEnabledRef\.current\) \{[\s\S]*?pendingVoiceAfterIntroRef\.current = \{ mode: 'dictation' \};[\s\S]*?setVoiceIntroOpen\(true\);[\s\S]*?return;/,
  "first mic click must show the shortcut intro before continuing voice input",
);
assert.match(
  chatSource,
  /function continuePendingVoiceAfterIntro\(\) \{[\s\S]*?pendingVoiceAfterIntroRef\.current = null;[\s\S]*?handleVoiceTrigger\(pending\.mode\);[\s\S]*?\}/,
  "closing or enabling the intro must continue the pending voice action exactly once",
);
assert.doesNotMatch(
  chatSource,
  /VoiceShortcutIntroModal[\s\S]*?role="switch"/,
  "voice intro modal must not show a second enable switch that conflicts with the primary enable button",
);
assert.doesNotMatch(
  chatSource,
  /<VoiceShortcutIntroModal[\s\S]*?shortcutEnabled=/,
  "voice intro modal should make the enable decision through the footer button only",
);

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
      enumerateDevices: async () => [],
      getUserMedia: (...args) => getUserMedia(...args),
    },
  },
  setTimeout,
  clearTimeout,
};
vm.createContext(mediaContext);
vm.runInContext(
  `${source.slice(mediaStart, mediaEnd)}\nthis.probeVoiceAudioInput = probeVoiceAudioInput; this.requestVoiceMedia = requestVoiceMedia;`,
  mediaContext,
  { filename: bridgePath },
);

(async () => {
  assert.strictEqual(await mediaContext.probeVoiceAudioInput(20), false);

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

  assert.match(chatSource, /const voiceBusy = voiceInput\.status === 'transcribing' \|\| voiceInput\.status === 'postprocessing'/);
  assert.match(
    chatSource,
    /if \(voiceInput\.status === 'requesting_permission'\) \{[\s\S]*?bridge\.voice\.cancelVoiceInput\(\);[\s\S]*?return;/,
  );

  const startVoiceInputAt = source.indexOf("  async function startVoiceInput(");
  const installStatusAt = source.indexOf('await invoke("voice_asr_status")', startVoiceInputAt);
  const requestingStatusAt = source.indexOf('setVoiceInputStatus("requesting_permission"', startVoiceInputAt);
  const activeSessionAt = source.indexOf("activeVoiceInput = session", startVoiceInputAt);
  assert.ok(startVoiceInputAt >= 0 && installStatusAt >= 0, "voice input start flow must exist");
  assert.ok(activeSessionAt < installStatusAt, "voice session must become cancellable before dependency status query");
  assert.ok(requestingStatusAt < installStatusAt, "voice input must show immediate feedback before dependency status query");
  assert.match(
    source.slice(installStatusAt, installStatusAt + 300),
    /if \(activeVoiceInput !== session\) return;/,
    "cancelled dependency status query must not resume microphone acquisition",
  );
  const notReadyAt = source.indexOf("if (asrStatus && !asrStatus.ready)", installStatusAt);
  assert.ok(notReadyAt > installStatusAt, "voice input start must handle missing ASR dependency");
  assert.match(
    source.slice(notReadyAt, notReadyAt + 800),
    /open:\s*!asrStatus\.installable/,
    "installable ASR should auto-install without opening the setup prompt first",
  );
  assert.match(
    source.slice(notReadyAt, notReadyAt + 800),
    /if \(asrStatus\.installable\) \{[\s\S]*?installVoiceAsr\(\);[\s\S]*?return;/,
    "installable ASR should start installation automatically and return before recording",
  );
  const installVoiceAsrAt = source.indexOf("async function installVoiceAsr()");
  assert.ok(installVoiceAsrAt >= 0, "installVoiceAsr must exist");
  assert.match(
    source.slice(installVoiceAsrAt, installVoiceAsrAt + 400),
    /open:\s*false,[\s\S]*?installing:\s*true/,
    "automatic ASR install must use the mic loading button instead of opening the centered setup dialog",
  );
  assert.match(
    chatSource,
    /voiceAsrBusy && voiceAsrPopoverOpen[\s\S]*?bridge\.voice\.cancelVoiceAsrSetup\(\)/,
    "ASR install progress and cancellation should be available from the mic loading popover",
  );

  const permissionCatchAt = source.indexOf('if (normalized.category === "permission_denied")', startVoiceInputAt);
  assert.ok(permissionCatchAt > startVoiceInputAt, "permission denial recovery must exist in voice input flow");
  assert.match(
    source.slice(permissionCatchAt, permissionCatchAt + 900),
    /await invoke\("reset_microphone_permission"\)/,
    "microphone denial must reset the saved permission before retry",
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
