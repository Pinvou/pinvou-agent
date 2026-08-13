#!/usr/bin/env node
const assert = require("assert");
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const bridgePath = path.join(__dirname, "..", "src", "platform", "tauri", "bridge", "voice.js");
const source = fs.readFileSync(bridgePath, "utf8");
const rustVoicePath = path.join(__dirname, "..", "src-tauri", "src", "app", "commands", "voice.rs");
const rustVoiceSource = fs.readFileSync(rustVoicePath, "utf8");
const chatPath = path.join(__dirname, "..", "src", "features", "chat", "ChatView.jsx");
const chatSource = fs.readFileSync(chatPath, "utf8");
const routerPath = path.join(__dirname, "..", "src", "features", "voice-composer", "VoiceShortcutRouter.jsx");
const routerSource = fs.readFileSync(routerPath, "utf8");
const voiceControlsPath = path.join(__dirname, "..", "src", "features", "voice-composer", "VoiceComposerControls.jsx");
const voiceControlsSource = fs.readFileSync(voiceControlsPath, "utf8");
const voiceNoticePath = path.join(__dirname, "..", "src", "features", "voice-composer", "VoiceNoticeBar.jsx");
const voiceNoticeSource = fs.readFileSync(voiceNoticePath, "utf8");
const voiceHookPath = path.join(__dirname, "..", "src", "features", "voice-composer", "useComposerVoiceInput.js");
const voiceHookSource = fs.readFileSync(voiceHookPath, "utf8");
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

const localAsrEmptyResult = normalizeVoiceError({
  category: "recognition_failed",
  stage: "transcribing",
  message: "Local SenseVoice/FunASR ASR failed (exit 6): ASR empty result: backend returned no usable result",
});
assert.strictEqual(localAsrEmptyResult.category, "empty_result");
assert.match(localAsrEmptyResult.message, /未识别到语音内容/);
assert.match(localAsrEmptyResult.diagnostic, /FunASR ASR failed/);

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
  "run_llm",
  "non-empty dictation should run LLM even when short and clear",
);
assert.deepStrictEqual(
  ruleContext.classifyVoiceText("查一下今日进价，并生成数据分析图标。", "task").strategy,
  "run_llm",
  "task mode with suspicious ASR terms should run LLM",
);
assert.strictEqual(ruleContext.classifyVoiceText("查天气", "task").strategy, "run_llm");
assert.deepStrictEqual(
  ruleContext.classifyVoiceText("多一个海报，这个海报用于公司的年会海报里面需要有一些文字说清楚地点和时间，在北京下午3点12月16号需要联网下载一张图片，海报都足够的好看。然后海报查一下。", "task").strategy,
  "run_llm",
  "long multi-condition poster task must run LLM instead of using raw ASR",
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
assert.strictEqual(
  ruleContext.applyVoiceDeterministicCorrections("嗯，做一张海报，这个海报有长方形，的需要联网下的图片。用于公司的下午茶需要有一些文字的内容。", ""),
  "做一张海报，这个海报是长方形，需要联网下载的图片。用于公司的下午茶需要有一些文字的内容。",
);
assert.strictEqual(ruleContext.voicePostprocessTimeoutMs("task", "查一下今日金价。"), 8000);
assert.strictEqual(ruleContext.voicePostprocessTimeoutMs("structured", "整理一下今天的会议待办。"), 3000);
assert.strictEqual(ruleContext.voicePostprocessTimeoutMs("edit", "把它改成三条要点。"), 12000);
assert.deepStrictEqual(
  ruleContext.classifyVoiceText("把这段改成三条要点。", "edit").strategy,
  "run_llm",
  "edit mode must always run LLM instead of appending the edit instruction",
);
assert.deepStrictEqual(
  ruleContext.classifyVoiceText("第一整理会议第二提取风险第三明天发送。", "structured").strategy,
  "run_llm",
  "legacy structured mode must fold into the dictation main path",
);
assert.deepStrictEqual(
  ruleContext.classifyVoiceText("一张用于公司年会的海报，时间是下午3点，12月36日需要联网下载一张图片，然后这个图片要尽量的好看呃，突出员工协作。这个海报是长方形的，上面需要有一点点文字，然后是红色背景。", "dictation").strategy,
  "run_llm",
  "long poster dictation from Alt or mic must run LLM",
);
assert.match(rustVoiceSource, /除极短自然句外，默认整理成结构化 Markdown 列表/);
assert.match(rustVoiceSource, /内容包含目标、用途、功能、字段、截止时间、进度、多个事项、多个条件、步骤、约束或明显需求表达时，必须整理成 Markdown 列表/);
assert.match(rustVoiceSource, /制作一个个人工作台，用于企业录入工作事项进度，包括截止时间/);
assert.match(rustVoiceSource, /12月36日下午3点/);
assert.match(rustVoiceSource, /model returned empty output/);
assert.doesNotMatch(
  chatSource,
  /key: 'structured'[\s\S]*handleVoiceMenuTrigger\('structured'\)/,
  "chat voice menu must not expose structured as a separate user mode",
);
assert.doesNotMatch(
  rustVoiceSource,
  /mode == "structured"/,
  "structured must not remain a standalone voice postprocess chain",
);
assert.match(
  rustVoiceSource,
  /VoiceReasoningDialect::ThinkingDisabled[\s\S]*body\["thinking"\] = json!\(\{ "type": "disabled" \}\)/,
  "voice postprocess must disable thinking for DeepSeek-style providers",
);
assert.match(
  rustVoiceSource,
  /VoiceReasoningDialect::QwenEnableThinking[\s\S]*body\["enable_thinking"\] = json!\(false\)/,
  "voice postprocess must disable Qwen thinking with enable_thinking=false",
);
assert.match(
  rustVoiceSource,
  /retry_empty_output[\s\S]*call_voice_postprocess_model\([\s\S]*true,[\s\S]*2,/,
  "voice postprocess must retry once with the retry prompt after empty LLM output",
);
assert.match(
  rustVoiceSource,
  /request attempt=\{\}/,
  "voice postprocess logs must include request attempt number",
);
assert.match(
  rustVoiceSource,
  /retry_unchanged_output/,
  "edit postprocess must retry when the model returns the original draft unchanged",
);
assert.match(
  rustVoiceSource,
  /completed mode=\{\} raw_len=\{\} draft_len=\{\} output_len=\{\} changed=\{\}/,
  "voice postprocess completed logs must include whether edit output changed",
);
assert.match(
  source,
  /editUnchanged[\s\S]*edit_unchanged: editUnchanged[\s\S]*voiceEditNoChange/,
  "voice bridge must expose unchanged edit diagnostics and a visible no-change status",
);
assert.doesNotMatch(source, /raw_text:\s*text/, "voice diagnostics must not persist raw ASR text");
assert.doesNotMatch(source, /final_text:\s*finalText/, "voice diagnostics must not persist postprocessed text");
assert.match(source, /raw_text_length:\s*text\.length/, "voice diagnostics should retain raw text length");
assert.match(source, /final_text_length:\s*finalText\.length/, "voice diagnostics should retain final text length");
assert.match(
  source,
  /VOICE_RECORDING_MAX_DURATION_MS = 60000/,
  "single voice recording must allow up to 60 seconds before auto-finish",
);
assert.match(
  source,
  /setTimeout\(function \(\) \{ finishVoiceInput\(false, true\); \}, VOICE_RECORDING_MAX_DURATION_MS\)/,
  "recording auto-finish must use the named 60s max duration constant",
);
assert.match(
  source,
  /fallbackHighRisk[\s\S]*taskSendBlocked[\s\S]*fallbackHighRisk/,
  "task mode must block sending when LLM fallback leaves high-risk ASR terms",
);
assert.match(
  source,
  /mode === "edit" && postprocessResult\.fallbackReason[\s\S]*outputValid = false/,
  "edit mode must not write the edit instruction back when LLM postprocess fails",
);
assert.match(
  routerSource,
  /const recording = status === 'recording';[\s\S]*?if \(!shortcutEnabled && !\(event && \(event\.key === 'Escape' \|\| \(event\.key === 'Alt' && recording\)\)\)\) return;/,
  "web voice shortcut keydown must gate startup by explicit opt-in but allow recording Alt stop",
);
assert.match(
  routerSource,
  /if \(!voiceShortcutEnabled\(\) && !\(event && event\.key === 'Alt' && recording\)\) \{[\s\S]*?clearPendingShortcut\(\);[\s\S]*?return;[\s\S]*?\}/,
  "web voice shortcut keyup must ignore disabled startup shortcuts but allow recording Alt stop",
);
assert.match(
  routerSource,
  /function triggerVoiceShortcutTarget\(target, actionMode, status, activeMode\) \{[\s\S]*?if \(status === 'recording'\) \{[\s\S]*?target\.trigger\(activeMode \|\| 'dictation', \{ source: 'shortcut-stop', preserveMode: true \}\);/,
  "web voice shortcut stop must preserve the active recording mode instead of re-resolving from draft text",
);
assert.match(
  routerSource,
  /listenTauri\('voice-shortcut:trigger'[\s\S]*?const recording = status === 'recording';[\s\S]*?if \(!voiceShortcutEnabled\(\) && !recording\) return;[\s\S]*?target\.trigger\(mode, \{ source: 'shortcut-stop', preserveMode: true \}\);[\s\S]*?target\.trigger\('dictation'\)/,
  "native voice shortcut trigger must gate startup by opt-in but allow recording Alt stop",
);
assert.match(
  chatSource,
  /pendingVoiceAfterIntroRef = useRef\(null\)/,
  "mic-click intro must keep a pending voice action",
);
assert.match(
  chatSource,
  /voiceIntroResolveRef = useRef\(null\)/,
  "shortcut intro must be awaitable after ASR is ready",
);
assert.match(
  chatSource,
  /beforePermission: context => \{[\s\S]*?pendingVoiceAfterIntroRef\.current = null;[\s\S]*?return requestVoiceShortcutIntroAfterAsr\(context && context\.mode\);[\s\S]*?\}/,
  "shortcut intro must run after ASR readiness and before microphone permission",
);
assert.match(
  chatSource,
  /if \(wasActive && !voiceAsrBusy && ready && done\) \{[\s\S]*?const pendingVoice = pendingVoiceAfterIntroRef\.current;[\s\S]*?requestVoiceShortcutIntroAfterAsr\(pendingVoice\.mode\)\.then[\s\S]*?handleVoiceTrigger\(pendingVoice\.mode, \{ source: pendingVoice\.source \|\| 'button' \}\);/,
  "after automatic ASR install, shortcut intro must appear before continuing the original mic action",
);
assert.match(
  voiceHookSource,
  /const sessionId = createVoiceSessionId\(current\.targetId\);[\s\S]*?voiceSessionIdRef\.current = sessionId;[\s\S]*?setVoiceSessionId\(sessionId\);/,
  "shared composer voice hook must create a voiceSessionId when recording starts",
);
assert.match(
  voiceHookSource,
  /if \(!targetId \|\| !isActiveVoiceTarget\(targetId, sessionId\)\) \{[\s\S]*?clearStaleVoiceState\(targetId, sessionId\);[\s\S]*?return;/,
  "shared composer voice hook must reject late writeback for inactive targets",
);
assert.match(
  voiceHookSource,
  /voiceSessionId,[\s\S]*?workspaceId: current\.workspaceId,[\s\S]*?sessionId: current\.sessionId,/,
  "registered voice targets must carry voiceSessionId and context",
);
assert.match(
  voiceHookSource,
  /beforePermission: typeof current\.beforePermission === 'function'[\s\S]*?current\.beforePermission/,
  "shared composer voice hook must pass the post-ASR shortcut gate into the bridge",
);
assert.match(
  voiceHookSource,
  /const preserveActiveMode = options\.preserveMode && voiceInput\.status === 'recording';[\s\S]*?let nextMode = preserveActiveMode \? normalizeMode\(voiceInput\.mode\) : normalizeMode\(mode\);[\s\S]*?!preserveActiveMode && typeof current\.resolveMode === 'function'/,
  "recording Alt stop must preserve the active mode and skip draft-based mode resolution",
);
const voiceRecordingStopAt = voiceHookSource.indexOf("if (voiceInput.status === 'recording') {");
const voiceBusyGateAt = voiceHookSource.indexOf("if (voiceBusy) return;", voiceRecordingStopAt);
const voiceBeforeStartAt = voiceHookSource.indexOf("current.onBeforeStart(nextMode)", voiceRecordingStopAt);
assert.ok(voiceRecordingStopAt >= 0 && voiceBusyGateAt > voiceRecordingStopAt,
  "recording stop must run before busy startup gates");
assert.ok(voiceRecordingStopAt >= 0 && voiceBeforeStartAt > voiceRecordingStopAt,
  "recording stop must run before the ASR-ready shortcut intro gate");
assert.match(
  voiceHookSource,
  /if \(mode === 'edit'\) \{[\s\S]*?current\.setDraft\(next\);[\s\S]*?setEditPreview\(null\);[\s\S]*?return;/,
  "edit mode must directly replace the draft after LLM succeeds",
);
assert.match(
  voiceHookSource,
  /next === original[\s\S]*onEditUnchanged/,
  "edit mode must not silently swallow unchanged model output",
);
assert.match(
  voiceHookSource,
  /applyVoiceEditPreview[\s\S]*?current\.setDraft\(next\)/,
  "legacy voice edit preview apply should remain available for stale preview state",
);
assert.match(
  voiceHookSource,
  /const cancelVoiceOrPreview = useCallback[\s\S]*?editPreviewRef\.current[\s\S]*?setEditPreview\(null\)[\s\S]*?cancelVoiceInput\(\)/,
  "global voice cancel must dismiss an edit preview before cancelling an active recording",
);
assert.match(
  chatSource,
  /resolveMode: \(mode, context\) => \{[\s\S]*?mode === 'dictation'[\s\S]*?context\.source !== 'button'[\s\S]*?return 'edit';/,
  "Alt dictation with existing chat draft must enter voice edit mode",
);
assert.match(
  chatSource,
  /<VoiceEditPreview[\s\S]*?onApply=\{\(\) => chatVoice\.applyVoiceEditPreview\(\)\}[\s\S]*?onApplyAndSend=\{\(\) => chatVoice\.applyVoiceEditPreview\(\{ send: true \}\)\}/,
  "chat composer must render the voice edit confirmation preview",
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

  assert.match(chatSource, /const voiceBusy = isVoiceBusy\(voiceInput\)/);
  assert.match(
    voiceHookSource,
    /if \(voiceInput\.status === 'requesting_permission'\) \{[\s\S]*?bridge\.voice\.cancelVoiceInput\(\);[\s\S]*?return;/,
  );

  const startVoiceInputAt = source.indexOf("  async function startVoiceInput(");
  const installStatusAt = source.indexOf('await invoke("voice_asr_status")', startVoiceInputAt);
  const requestingStatusAt = source.indexOf('setVoiceInputStatus("requesting_permission"', startVoiceInputAt);
  const activeSessionAt = source.indexOf("activeVoiceInput = session", startVoiceInputAt);
  const beforePermissionAt = source.indexOf('typeof options.beforePermission === "function"', installStatusAt);
  const permissionStatusAt = source.indexOf('message: bt("voiceRequestingPermission")', installStatusAt);
  assert.ok(startVoiceInputAt >= 0 && installStatusAt >= 0, "voice input start flow must exist");
  assert.ok(activeSessionAt < installStatusAt, "voice session must become cancellable before dependency status query");
  assert.ok(requestingStatusAt < installStatusAt, "voice input must show immediate feedback before dependency status query");
  assert.ok(
    beforePermissionAt > installStatusAt && beforePermissionAt < permissionStatusAt,
    "shortcut intro gate must run after ASR readiness and before microphone permission",
  );
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
    source.slice(installVoiceAsrAt, installVoiceAsrAt + 1000),
    /alreadyInstalling[\s\S]*?open:\s*false,[\s\S]*?installing:\s*alreadyInstalling && !cancelled/,
    "duplicate ASR install attempts must stay in mic loading state instead of reopening the setup dialog",
  );
  assert.match(
    chatSource,
    /voiceAsrSetup\.open && canInstallLocalAsr && !voiceAsrSetup\.status\?\.installable/,
    "installable ASR downloads must not render the centered setup confirmation dialog",
  );
assert.match(
  chatSource + voiceControlsSource,
  /voiceAsrPopoverOpen[\s\S]*?onCancelAsr[\s\S]*?bridge\.voice\.cancelVoiceAsrSetup\(\)/,
  "ASR install progress and cancellation should be available from the mic loading popover",
);
assert.match(
  voiceNoticeSource,
  /voiceInput\.category === 'empty_result'[\s\S]*?voiceEmptyResultTitle[\s\S]*?voiceEmptyResultHint[\s\S]*?voiceRetryAgain/,
  "ASR empty result must use user-facing iOS-style copy instead of raw backend errors",
);
const emptyNoticeStart = voiceNoticeSource.indexOf("voiceInput.category === 'empty_result'");
const emptyNoticeEnd = voiceNoticeSource.indexOf("return (", emptyNoticeStart + 1);
assert.ok(emptyNoticeStart >= 0 && emptyNoticeEnd > emptyNoticeStart, "empty-result voice notice branch must exist");
assert.doesNotMatch(
  voiceNoticeSource.slice(emptyNoticeStart, emptyNoticeEnd),
  /voiceGotoDeps/,
  "ASR empty result must not show dependency-check guidance",
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
