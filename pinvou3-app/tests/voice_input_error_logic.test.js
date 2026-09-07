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
const voiceIntroModalPath = path.join(__dirname, "..", "src", "features", "voice-composer", "VoiceShortcutIntroModal.jsx");
const voiceIntroModalSource = fs.readFileSync(voiceIntroModalPath, "utf8");
const voiceNoticePath = path.join(__dirname, "..", "src", "features", "voice-composer", "VoiceNoticeBar.jsx");
const voiceNoticeSource = fs.readFileSync(voiceNoticePath, "utf8");
const voiceHookPath = path.join(__dirname, "..", "src", "features", "voice-composer", "useComposerVoiceInput.js");
const voiceHookSource = fs.readFileSync(voiceHookPath, "utf8");
const settingsPath = path.join(__dirname, "..", "src", "features", "settings", "SettingsView.jsx");
const settingsSource = fs.readFileSync(settingsPath, "utf8");
const webBridgeSource = fs.readFileSync(path.join(__dirname, "..", "src", "platform", "web", "bridge.js"), "utf8");
const rustShortcutPlatformPath = path.join(__dirname, "..", "src-tauri", "src", "features", "voice_shortcut", "platform", "mod.rs");
const rustShortcutPlatformSource = fs.existsSync(rustShortcutPlatformPath) ? fs.readFileSync(rustShortcutPlatformPath, "utf8") : "";
// voice.js copy comes from bridge.js's BT_TABLE (bt(key): per-language lookup
// with a Chinese fallback); extract the zh table from bridge.js here to build
// bt so assertions target real copy.
const bridgeMainSource = fs.readFileSync(path.join(__dirname, "..", "src", "platform", "tauri", "bridge.js"), "utf8");
const zhTableMatch = bridgeMainSource.match(/ {4}zh: \{([\s\S]*?)\r?\n {4}\},\r?\n {2}\};/);
assert.notStrictEqual(zhTableMatch, null, "bridge.js BT_TABLE zh block must exist");
const zhTable = new Function(`return ({${zhTableMatch[1]}});`)();
const bt = (key) => zhTable[key] !== undefined ? zhTable[key] : key;
// The slice must include VOICE_ERROR_CODE_KEYS: code-bearing error assertions
// depend on that table (short-circuit evaluation once masked its absence).
const start = source.indexOf("  const VOICE_ERROR_CODE_KEYS = {");
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
this.validateVoicePostprocessOutput = validateVoicePostprocessOutput;
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
  "short clear dictation should keep ASR text instead of running LLM",
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
  [...ruleContext.hasVoiceHighRiskResidual("查一下今日进价。")],
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
assert.strictEqual(
  ruleContext.applyVoiceDeterministicCorrections("帮我调用 more API 文档", ""),
  "帮我调用 more API 文档",
);
assert.strictEqual(
  ruleContext.applyVoiceDeterministicCorrections("configure API", ""),
  "configure API",
);
assert.strictEqual(
  ruleContext.applyVoiceDeterministicCorrections("查一下 pinvou 的下载地址", ""),
  "查一下 pinvou 的下载地址",
);
assert.strictEqual(
  ruleContext.applyVoiceDeterministicCorrections("pin voltage", ""),
  "pin voltage",
);
assert.strictEqual(
  ruleContext.applyVoiceDeterministicCorrections("产品名config 是什么", ""),
  "产品名config 是什么",
  "产品名con correction alternative must have a trailing boundary and not eat half an English word",
);
assert.strictEqual(
  ruleContext.applyVoiceDeterministicCorrections("产品名con 是什么", ""),
  "产品名 Pinvou 是什么",
  "产品名con misrecognition must still be corrected",
);
const evalScriptSource = fs.readFileSync(path.join(__dirname, "..", "scripts", "voice-normalize-eval.mjs"), "utf8");
assert.match(
  evalScriptSource,
  /产品名con\(\?!\[a-zA-Z\]\)/,
  "eval copy of the Pinvou correction must not lag behind the bounded production rule",
);
assert.match(source, /const candidateText = postprocessResult\.text;/);
assert.doesNotMatch(
  source,
  /applyVoiceDeterministicCorrections\(postprocessResult\.text/,
  "LLM postprocess output must not receive a second deterministic ASR correction pass",
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
assert.match(
  rustVoiceSource,
  /\[voice_postprocess\] empty_output[\s\S]{0,800}\(String::new\(\), false\)/,
  "empty final postprocess output must return Ok (discard semantics), not an error: the frontend validator accepts an empty candidate exactly for filler-only input",
);
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
  /bridge\.voice\.setVoiceShortcutEnabled\(voiceShortcutEnabled\(\)\)/,
  "native voice shortcut hook must receive the explicit settings toggle",
);
assert.doesNotMatch(
  routerSource,
  /voice-shortcut:cancel/,
  "native voice shortcut router must not listen for stale cancel events after Escape is state-gated in the frontend",
);
assert.match(
  rustShortcutPlatformSource,
  /static SHORTCUT_ENABLED: AtomicBool = AtomicBool::new\(false\);/,
  "native voice shortcut hook must default off",
);
// After the tap-hold state machine refactor, the enable gate moved up to the
// keyboard_hook_proc entry: when disabled, global keystrokes pass straight
// through and never reach gesture handling.
assert.match(
  rustShortcutPlatformSource,
  /fn keyboard_hook_proc[\s\S]*?if !shortcut_enabled\(\) \{[\s\S]*?return call_next_hook[\s\S]*?handle_voice_shortcut_key\(/,
  "native voice shortcut hook must gate all keystrokes by the synced settings state before gesture handling",
);
// Cross-window recording mutual exclusion: the trigger target resolves to the recording window first.
assert.match(
  rustShortcutPlatformSource,
  /resolve_trigger_target\(recording_label\(\)\.as_deref\(\)/,
  "native voice shortcut trigger must route to the recording window first",
);
assert.match(
  routerSource,
  /function isVoiceShortcutEventForThisWindow\(payload\) \{[\s\S]*?typeof payload\.window_label === 'string'[\s\S]*?ownLabel === eventLabel/,
  "voice shortcut router must only consume trigger events addressed to this window label",
);
assert.match(
  routerSource,
  /listenTauri\('voice-shortcut:trigger'[\s\S]*?isVoiceShortcutEventForThisWindow\(payload\)[\s\S]*?const recording = status === 'recording';[\s\S]*?if \(recording\) \{[\s\S]*?target\.trigger\(mode, \{ source: 'shortcut-stop', preserveMode: true \}\);[\s\S]*?target\.trigger\('dictation'\)/,
  "native voice shortcut trigger must check the window label and allow recording Alt stop",
);
// Native routed the event to this window as the "recording window", but this
// window has no active recording: after a WebView reload/restore the JS
// session was rebuilt while the native registration was never cleared
// (forget_recording_window only covers window destruction). The stale
// registration must be cleared and the gesture dropped; otherwise Alt in any
// window keeps getting routed into a background window, ghost-starting a mic
// session until that session naturally ends. Normal route==='focused'
// triggers are unaffected.
assert.match(
  routerSource,
  /payload\.route === 'recording' && !recording[\s\S]*?syncVoiceShortcutRecording\(null\)[\s\S]*?return;/,
  "voice shortcut router must clear a stale native recording registration instead of ghost-starting a recording in a background window",
);
// Native events are only emitted when the Rust-side toggle is on (the hook
// entry short-circuits on !shortcut_enabled()), so the authoritative gate has
// already run at the native layer; the localStorage mirror only serves the
// in-window key-gesture channel. If the native event path also checked the
// mirror, then after the WebView storage is cleared (mirror defaults to
// false) an authoritatively enabled shortcut would be swallowed by the native
// hook yet its events dropped by the frontend, silently failing until the
// toggle is flipped again.
assert.doesNotMatch(
  routerSource,
  /listenTauri\('voice-shortcut:trigger'[\s\S]*voiceShortcutEnabled\(\)/,
  "native voice shortcut trigger must trust the Rust-side authoritative gate, not the localStorage mirror",
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
// Session/workspace switches do not clear the current view, so a leftover
// voice rewrite preview belongs to the old context: applying its next into
// the new session's draft — or sending it into the new session via sendTask —
// is cross-context data pollution. An identity change must auto-cancel the
// preview (explicit user apply/cancel is unaffected).
assert.match(
  voiceHookSource,
  /voiceContextIdentityRef[\s\S]*?previous === null \|\| previous === identity[\s\S]*?if \(editPreviewRef\.current\) \{[\s\S]*?setEditPreview\(null\);[\s\S]*?closeVoice\(\);/,
  "voice hook must cancel a stale edit preview when the session/workspace identity changes",
);
// A pending edit preview is keyed to the old draft snapshot; starting a fresh
// voice session abandons it. Without this disposal, a preview left hanging
// while a new dictation appends to the draft would let the global Enter
// handler replace the draft wholesale with a rewrite based on the stale
// original — silently discarding the newly dictated content.
const triggerBodyStart = voiceHookSource.indexOf("const triggerVoice = useCallback");
const triggerBodyEnd = voiceHookSource.indexOf("const sessionId = createVoiceSessionId(", triggerBodyStart);
assert.ok(
  triggerBodyStart >= 0 && triggerBodyEnd > triggerBodyStart,
  "triggerVoice fresh-session path must exist in the shared voice hook",
);
const triggerBody = voiceHookSource.slice(triggerBodyStart, triggerBodyEnd);
assert.match(
  triggerBody,
  /if \(editPreviewRef\.current\) setEditPreview\(null\);/,
  "triggerVoice must dispose a pending edit preview before starting a fresh session",
);
// With smart organize off, the edit lane has no LLM available
// (postprocess_disabled is guaranteed to fail), so the dictation→edit auto
// upgrade must be suppressed: stay in dictation and write back the
// rule-corrected text, matching the web lane's asr_only downgrade. Covers
// both upgrade paths: resolveMode and the built-in fallback.
assert.match(
  triggerBody,
  /resolved === 'edit' && nextMode === 'dictation' && !voicePostprocessEnabled\(\)\) \{\s*nextMode = 'dictation';/,
  "smart organize off must suppress the resolveMode dictation→edit upgrade",
);
assert.match(
  triggerBody,
  /nextMode = voicePostprocessEnabled\(\) \? 'edit' : 'dictation';/,
  "smart organize off must suppress the fallback dictation→edit upgrade",
);
// The preview replaces the whole draft from the recording-start snapshot
// (original), while the input stays hand-editable while the preview hangs.
// If the draft has drifted from original, apply must be intercepted first
// (discard the preview, keep the draft); otherwise manually typed/pasted
// content would be silently overwritten by a rewrite based on the old
// original (same family as "a new session discards a stale preview").
const applyBodyStart = voiceHookSource.indexOf("const applyVoiceEditPreview = useCallback");
const applyBodyEnd = voiceHookSource.indexOf("current.setDraft(next)", applyBodyStart);
assert.ok(
  applyBodyStart >= 0 && applyBodyEnd > applyBodyStart,
  "applyVoiceEditPreview with its draft write must exist in the shared voice hook",
);
const applyBody = voiceHookSource.slice(applyBodyStart, applyBodyEnd);
assert.match(
  applyBody,
  /trimDraft\(current\.getDraft\(\)\) !== preview\.original/,
  "applying the preview must first verify the draft still matches the preview's original snapshot",
);
assert.match(
  applyBody,
  /setEditPreview\(null\)/,
  "a drifted draft must dispose the stale preview instead of replacing the draft",
);
// Exercise the real randomness of createVoiceSessionRandomPart directly (the
// old assertion checked the template wrapper createVoiceSessionId, whose
// optional match degrades to an empty string on failure, making doesNotMatch
// vacuously true).
const randomPartStart = voiceHookSource.indexOf("let fallbackVoiceSessionCounter = 0;");
const randomPartEnd = voiceHookSource.indexOf("\nfunction createVoiceSessionId(", randomPartStart);
assert.ok(
  randomPartStart >= 0 && randomPartEnd > randomPartStart,
  "createVoiceSessionRandomPart must exist in the shared voice hook",
);
const randomPartSource = voiceHookSource.slice(randomPartStart, randomPartEnd);
assert.doesNotMatch(
  randomPartSource,
  /Math\.random/,
  "voiceSessionId must not use Math.random because it guards stale cross-target voice writes",
);
const randomPartContext = { crypto: require("crypto").webcrypto, Uint32Array, Date };
vm.createContext(randomPartContext);
vm.runInContext(`${randomPartSource}\nthis.createVoiceSessionRandomPart = createVoiceSessionRandomPart;`, randomPartContext, {
  filename: voiceHookPath,
});
const firstRandomPart = randomPartContext.createVoiceSessionRandomPart();
const secondRandomPart = randomPartContext.createVoiceSessionRandomPart();
assert.notStrictEqual(firstRandomPart, secondRandomPart, "voice session random part must differ across calls");
assert.match(firstRandomPart, /^[0-9a-z-]+$/, "voice session random part must be a URL-safe token");
assert.match(
  voiceHookSource,
  /cryptoApi\.randomUUID|cryptoApi\.getRandomValues/,
  "voiceSessionId must prefer Web Crypto randomness",
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
const voiceBusyGateAt = voiceHookSource.indexOf("if (voiceBusy) return false;", voiceRecordingStopAt);
const voiceBeforeStartAt = voiceHookSource.indexOf("current.onBeforeStart(nextMode)", voiceRecordingStopAt);
assert.ok(voiceRecordingStopAt >= 0 && voiceBusyGateAt > voiceRecordingStopAt,
  "recording stop must run before busy startup gates");
assert.ok(voiceRecordingStopAt >= 0 && voiceBeforeStartAt > voiceRecordingStopAt,
  "recording stop must run before the ASR-ready shortcut intro gate");
assert.match(
  voiceHookSource,
  /if \(mode === 'edit'\) \{[\s\S]*?setEditPreview\(\{[\s\S]*?original,[\s\S]*?next,[\s\S]*?instruction:[\s\S]*?context,[\s\S]*?\}\);[\s\S]*?return;/,
  "edit mode must show a confirmation preview after LLM succeeds",
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
assert.match(
  webBridgeSource,
  /function normalizeVoiceMode\(mode\) \{[\s\S]*?mode === "edit" \? "edit"/,
  "web bridge must preserve edit mode instead of folding it into dictation",
);
// The web lane has no LLM postprocess, so edit must be downgraded to
// appended dictation before writeback; otherwise the uncorrected raw ASR
// text would replace the whole draft via the replace preview.
assert.match(
  webBridgeSource,
  /finishVoiceInput[\s\S]*?if \(mode === "edit"\) mode = "dictation";[\s\S]*?session\.writeback/,
  "web voice writeback must downgrade edit to dictation instead of replacing the draft with raw ASR",
);
// edit writeback only sets a pending preview and does not commit, so the completion state must use PreviewReady, not Applied.
assert.match(
  source,
  /mode === "edit" \? bt\("voiceEditPreviewReady"\)/,
  "edit completion notice must say the result is ready to review, not applied",
);
assert.doesNotMatch(
  source,
  /mode === "edit" \? bt\("voiceEditApplied"\)/,
  "edit writeback only opens a preview, so the applied notice must not fire at writeback time",
);
// After the preview apply/cancel finishes, clear the completed notice so a leftover "pending review" message cannot mislead the user.
assert.match(
  voiceHookSource,
  /const applyVoiceEditPreview[\s\S]*?setEditPreview\(null\);[\s\S]*?closeVoice\(\);[\s\S]*?if \(!options\.send\) return true;/,
  "applying or canceling the voice edit preview must clear the stale voice notice",
);
// Windows low-level hook callbacks are bound by LowLevelHooksTimeout; synchronous stderr printing is forbidden.
assert.doesNotMatch(
  rustShortcutPlatformSource,
  /eprintln!/,
  "voice shortcut hook callbacks must not write to stderr synchronously",
);
// Voice's reasoning dialect delegates to core like review/memory; no more hand-copied fourth version.
assert.match(
  rustVoiceSource,
  /crate::core::reasoning_dialect::reasoning_dialect_from_base_url\(base_url, model\)/,
  "voice reasoning dialect must delegate to core/reasoning_dialect.rs",
);
assert.doesNotMatch(
  rustVoiceSource,
  /fn voice_reasoning_dialect_from_base_url|fn voice_kimi_supports_disabled_thinking/,
  "voice must not keep its own copy of the shared reasoning dialect sniffing",
);
assert.doesNotMatch(
  voiceControlsSource,
  /voiceEdit(?:PreviewTitle|Apply|ApplyAndSend|Cancel|Original|Result)[^;\n]*\|\| '[^']+'/,
  "voice edit preview must not add single-language fallback copy in the component",
);
assert.match(rustVoiceSource, /struct VoiceTempWav/);
// Temp WAVs use tempfile::NamedTempFile: unpredictable name, 0600 perms,
// deleted on drop; never revert to self-built pid+millisecond names
// (predictable and world-readable at 0644).
assert.match(
  rustVoiceSource,
  /file: tempfile::NamedTempFile[\s\S]*?tempfile::Builder::new\(\)[\s\S]*?\.tempfile\(\)\?/,
  "temp wav must use tempfile::NamedTempFile",
);
assert.doesNotMatch(
  rustVoiceSource,
  /std::process::id\(\)/,
  "temp wav name must not be self-built from pid and milliseconds",
);
const committedAudioDir = path.join(__dirname, "fixtures", "voice-audio-samples");
assert.ok(
  !fs.existsSync(committedAudioDir) || fs.readdirSync(committedAudioDir).filter(file => file.endsWith(".wav")).length === 0,
  "voice audio fixture binaries must not be committed when tests only consume text fixtures",
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
assert.match(
  settingsSource,
  /data-testid="voice-shortcut-info"/,
  "settings voice shortcut row must expose a lightweight info button",
);
assert.match(
  settingsSource,
  /function handleVoiceShortcutInfoClose\(\) \{[\s\S]*?setVoiceShortcutIntroOpen\(false\);[\s\S]*?\}/,
  "settings voice shortcut info close must only dismiss the guide after marking it seen",
);
assert.match(
  settingsSource,
  /function markVoiceShortcutIntroSeen\(\) \{[\s\S]*?setVoiceShortcutIntroSeen\(true\);[\s\S]*?\}/,
  "settings voice shortcut guide must mark the shortcut intro as seen",
);
const infoOpenHandler = settingsSource.match(/function handleVoiceShortcutInfoOpen[\s\S]*?\n {6}\}/);
assert.ok(infoOpenHandler, "settings voice shortcut info open handler must exist");
assert.doesNotMatch(
  infoOpenHandler[0],
  /checkDependencies|startVoiceInput|requestVoice|transcribe|download/i,
  "settings voice shortcut info button must not check ASR, download models, or start recording",
);
assert.match(
  settingsSource,
  /<VoiceShortcutIntroModal[\s\S]*?shortcutEnabled=\{voiceShortcutsEnabled\}[\s\S]*?primaryLabel=\{voiceShortcutsEnabled \? t\.voiceIntroDone : \(t\.voiceShortcutEnableTitle \|\| t\.uiSettings\.voiceShortcutEnable\)\}/,
  "settings voice shortcut guide must use settings-specific button copy instead of the first-use continue copy",
);
assert.match(
  voiceIntroModalSource,
  /const canEnable = !shortcutEnabled && typeof onToggleShortcut === 'function';/,
  "shared voice shortcut intro modal must hide the enable action when shortcuts are already enabled",
);
assert.doesNotMatch(
  voiceIntroModalSource,
  /role="switch"/,
  "shared voice intro modal must not reintroduce a second enable switch",
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
  await new Promise((resolve) => { setTimeout(resolve, 0); });
  assert.strictEqual(stoppedTracks, 1, "late microphone stream must be stopped after timeout");

  // User cancels while permission is pending: cancelPromise finalizes the
  // session first (cancelled + activeVoiceInput=null); the startVoiceInput
  // catch that lands afterwards must exit early via the session check and
  // must not overwrite the state with failed.
  getUserMedia = () => new Promise(() => {});
  const cancelSession = {};
  mediaContext.activeVoiceInput = cancelSession;
  const cancelCase = mediaContext.requestVoiceMedia(cancelSession, { audio: true }, 60);
  cancelSession.cancelPermissionRequest();
  await assert.rejects(
    cancelCase,
    (error) => error && error.category === "cancelled",
  );
  mediaContext.activeVoiceInput = null;
  const webStartAt = webBridgeSource.indexOf("  async function startVoiceInput(");
  // In the web bridge cancelVoiceInput is a plain function; the old anchor
  // wrote "async", which indexOf'd to -1 and sliced to EOF.
  const webCatchEnd = webBridgeSource.indexOf("  function cancelVoiceInput", webStartAt);
  assert.ok(webCatchEnd > webStartAt, "web cancelVoiceInput anchor must exist after startVoiceInput");
  const webCatch = webBridgeSource.slice(
    webBridgeSource.indexOf("} catch (err) {", webStartAt),
    webCatchEnd,
  );
  assert.ok(webStartAt >= 0, "web startVoiceInput must exist");
  assert.match(
    webCatch,
    /if \(activeVoiceInput !== session\) return;/,
    "web startVoiceInput catch must exit early when the session was already cancelled",
  );
  // The requestVoiceMedia timeout references bt("voiceDeviceTimeout"); all three language tables on the web lane must define that key.
  for (const lang of ["en", "ja", "zh"]) {
    const tableMatch = webBridgeSource.match(new RegExp(`^    ${lang}: \\{([\\s\\S]*?)\\r?\\n    \\},`, "m"));
    assert.notStrictEqual(tableMatch, null, `web BT_TABLE ${lang} block must exist`);
    assert.match(
      tableMatch[1],
      /voiceDeviceTimeout:\s*"/,
      `web BT_TABLE ${lang} must define voiceDeviceTimeout used by requestVoiceMedia`,
    );
  }

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
  /voiceAsrPopoverOpen[\s\S]*?onCancelAsr[\s\S]*?bridge\.voice\.cancelVoiceAsrSetup\?\.\(\)/,
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

// ── PR#248 review regression: minimal-context guard for high-collateral
// deterministic rules ──
// Every input below is legitimate; deterministic corrections must preserve
// them verbatim (real false-positive list from review).
const legitimateSentences = [
  "他很负责任。",
  "爷爷是位很负责任的老师。",
  "这家店今天的进价比昨天低。",
  "请调高语音输出的音量。",
  "这个设计有交互风险。",
  "把产品先上线，再慢慢迭代。",
  "他爱新闻联播里的天气预报。",
  "把问题上报消费者协会。",
  "发现错误马上修复。",
  "查看预报消息。",
  "列出负责任的负责人。",
  "我在北京的高",
];
legitimateSentences.forEach(function (sentence) {
  assert.strictEqual(
    ruleContext.applyVoiceDeterministicCorrections(sentence, sentence),
    sentence,
    "legitimate input must not be mangled by deterministic corrections: " + sentence,
  );
});
// The guard must not kill genuine correction scenarios.
assert.strictEqual(
  ruleContext.applyVoiceDeterministicCorrections("查一下今日进价。", ""),
  "查一下今日金价。",
  "gold-price query correction must survive the comparison guard",
);
assert.strictEqual(
  ruleContext.applyVoiceDeterministicCorrections("差旅报消标准是什么", ""),
  "差旅报销标准是什么",
  "genuine 报消 misrecognition must still be corrected",
);
assert.strictEqual(
  ruleContext.applyVoiceDeterministicCorrections("请用语音输出功能", ""),
  "请用语音输入功能",
  "standalone 语音输出 must still be corrected",
);
// Classification must run on the raw ASR text; otherwise rule-suspicious words disappear before classification and miscorrections reach the input box.
assert.match(
  source,
  /classifyVoiceText\(text, mode\)/,
  "voice classification must use the raw ASR text",
);
assert.doesNotMatch(
  source,
  /classifyVoiceText\(ruleText/,
  "voice classification must not run on rule-corrected text",
);
// Protected validation: when the LLM reverts a rule miscorrection (表哥→表格) to the original wording it must pass; genuinely dropping the word is still rejected.
assert.strictEqual(
  ruleContext.validateVoicePostprocessOutput("我表哥", "我表格", "我表哥", "dictation"),
  true,
  "LLM restoring a rule-miscorrected term to the raw ASR wording must pass validation",
);
assert.strictEqual(
  ruleContext.validateVoicePostprocessOutput("我表哥", "我表格", "我", "dictation"),
  false,
  "dropping the corrected term entirely must still be rejected",
);
assert.strictEqual(
  ruleContext.validateVoicePostprocessOutput(
    "查一下今日进价，并生成数据分析图标",
    "查一下今日金价，并生成数据分析图表",
    "查一下今日金价，并生成数据分析图表",
    "task",
  ),
  true,
  "LLM keeping the corrected protected terms must pass validation",
);
// The postprocess request must carry the raw ASR so the model can undo deterministic-rule miscorrections.
assert.match(
  source,
  /raw_text: String\(rawText \|\| correctedText \|\| ""\)/,
  "postprocess request must carry the raw ASR text alongside the corrected text",
);
// LLM output wrapped in a full markdown fence must be stripped before writeback.
assert.match(
  source,
  /stripVoicePostprocessFences\(res && res\.text/,
  "LLM postprocess output must be stripped of wrapping markdown fences",
);
// Output truncated with finish_reason=length must fall back and never be silently written back.
assert.match(
  source,
  /res && res\.truncated[\s\S]*?postprocess_truncated/,
  "truncated LLM output must fall back instead of being written back",
);
// An LLM failure in edit mode must not reuse the empty-result copy.
assert.match(
  source,
  /editPostprocessFailed[\s\S]*?bt\("voiceEditPostprocessFailed"\)/,
  "edit-mode LLM failure must not reuse the empty-result copy",
);
// With the smart organize toggle off, the bridge must not send recognized text to the model service (rule corrections only).
assert.match(
  source,
  /else if \(isVoicePostprocessEnabled\(\)\)[\s\S]*?postprocessVoiceText\(/,
  "smart organize must run only when the settings toggle is on",
);
assert.match(
  source,
  /skipped_postprocess_disabled/,
  "disabling smart organize must skip the LLM round-trip with rule-corrected text only",
);
// With the toggle off there is no LLM, so an edit session has no rewrite to
// preview: the rule-corrected spoken instruction must not pose as the edit
// result (confirming would wholesale-replace the draft). It must take the
// same fallbackReason path as an LLM failure (editPostprocessFailed notice,
// draft untouched). Dictation keeps writing the rule-corrected text back.
assert.match(
  source,
  /fallbackReason: mode === "edit" \? "postprocess_disabled" : "",/,
  "edit sessions with smart organize off must fail like postprocess failures instead of previewing rule text",
);
// The postprocess_disabled failure is deterministic (the generic "please
// retry" copy would just fail again), so it needs its own honest copy that
// explains the cause and the way out.
assert.match(
  source,
  /postprocessResult\.fallbackReason === "postprocess_disabled"\s*\?\s*bt\("voiceEditPostprocessDisabled"\)/,
  "the deterministic postprocess_disabled edit failure must show its own honest copy instead of 'please retry'",
);
assert.strictEqual(
  (bridgeMainSource.match(/voiceEditPostprocessDisabled:/g) || []).length,
  3,
  "voiceEditPostprocessDisabled must exist in all three BT_TABLE languages",
);
assert.match(
  source,
  /pinvou_voice_postprocess_enabled_v1/,
  "bridge gate must read the same localStorage key as the settings module",
);

// 60s recordings must cross IPC as base64; JSON number arrays are forbidden.
assert.match(
  source,
  /encodeVoiceBase64Bytes\(new Uint8Array\(wav\)\)[\s\S]*?audio_base64: audioBase64/,
  "voice audio must cross IPC as a base64 string",
);
assert.doesNotMatch(
  source,
  /audio_bytes/,
  "voice audio must not cross IPC as a JSON number array",
);

// Cross-window recording mutex wiring: recording start registers this window's label, finishVoiceInput clears it in one place, and old backends silently ignore it.
assert.match(
  source,
  /invoke\("set_voice_shortcut_recording", \{ label: label \|\| null \}\)\)\.catch\(function \(\) \{\}\)/,
  "recording label sync must tolerate old backends without the command",
);
assert.match(
  source,
  /setVoiceInputStatus\("recording"[\s\S]*?syncVoiceShortcutRecording\(currentVoiceWindowLabel\(\), session\.sessionId\)/,
  "recording start must register this window label (with the session token) for cross-window mutual exclusion",
);
assert.match(
  source,
  /async function finishVoiceInput\(cancelled, timedOut\) \{[\s\S]*?if \(!session\) return;[\s\S]*?syncVoiceShortcutRecording\(null, session\.sessionId\)/,
  "finishVoiceInput must clear the recording label (token-guarded) on every exit path",
);
// Storage listeners filter by exact key (three places): a null key
// (localStorage.clear()) and unrelated keys say nothing about the
// authoritative setting and must be ignored; otherwise the mirror's default
// false at the instant storage is cleared would overwrite the displayed state.
assert.match(
  routerSource,
  /function handleShortcutStorageEvent\(event\) \{[\s\S]*?event\.key !== VOICE_SHORTCUT_ENABLED_KEY\) return;/,
  "router storage mirror must ignore null-key and unrelated-key storage events",
);
assert.match(
  chatSource,
  /function handleVoiceShortcutStorage\(event\) \{[\s\S]*?event\.key !== VOICE_SHORTCUT_ENABLED_KEY\) return;/,
  "chat voice storage mirror must ignore null-key and unrelated-key storage events",
);
assert.match(
  settingsSource,
  /if \(event && event\.type === 'storage' && event\.key[\s\S]*?event\.key !== VOICE_POSTPROCESS_ENABLED_KEY\) return;[\s\S]*?if \(event && event\.type === 'storage' && !event\.key\) return;/,
  "settings voice storage mirror must ignore null-key and unrelated-key storage events",
);

// Token guard: registration carries a token and the clear is only sent when the token matches — a late finish must not erase a newer session's registration.
assert.match(
  source,
  /let voiceShortcutRecordingToken = null;[\s\S]*?if \(label\) \{[\s\S]*?voiceShortcutRecordingToken = token \|\| null;[\s\S]*?\} else if \(token && voiceShortcutRecordingToken && token !== voiceShortcutRecordingToken\) \{[\s\S]*?return;/,
  "recording label clear must be token-guarded so a late finish cannot erase a newer session registration",
);

// ===== Error codes end to end: desktop→web RPC passthrough + both-lane mapping guards =====
// (1) tauri lane: the stable error code takes precedence over the Chinese rawMessage (direct behavior assertions).
const codedJoin = normalizeVoiceError({
  code: "asr_join_failed",
  category: "recognition_failed",
  stage: "transcribing",
  message: "ASR 输出解析失败：意外的文件结构",
});
assert.strictEqual(
  codedJoin.message,
  bt("voiceInputFailed"),
  "stable code must map to trilingual copy even inside recognition_failed",
);
assert.strictEqual(
  codedJoin.diagnostic,
  "ASR 输出解析失败：意外的文件结构",
  "Chinese raw message must be demoted to diagnostic",
);
const codedTooLong = normalizeVoiceError({
  code: "recording_too_long",
  message: "录音时长超过上限（60 秒）",
});
assert.strictEqual(
  codedTooLong.message,
  bt("voiceRecordingTooLong"),
  "a coded error without a known category must still map by code",
);
assert.strictEqual(codedTooLong.diagnostic, "录音时长超过上限（60 秒）");

// (2) web lane: after remote-control RPC passthrough, the bootstrap-rebuilt
// Error carries code/category; normalizeVoiceError must map by code to
// trilingual copy — with a category it takes the dedicated branch, and with
// only a code the final codeKey branch catches it (same semantics as the
// tauri lane).
const webVoiceStart = webBridgeSource.indexOf("  const VOICE_ERROR_CODE_KEYS = {");
const webVoiceEnd = webBridgeSource.indexOf("\n  function stopMediaTracks(", webVoiceStart);
assert.ok(webVoiceStart >= 0 && webVoiceEnd > webVoiceStart, "web voice error-mapping block must exist");
const webTableMatch = webBridgeSource.match(/^ {4}zh: \{([\s\S]*?)\r?\n {4}\},/m);
assert.notStrictEqual(webTableMatch, null, "web BT_TABLE zh block must exist");
const webZhTable = new Function(`return ({${webTableMatch[1]}});`)();
const webBt = (key) => (webZhTable[key] !== undefined ? webZhTable[key] : key);
const webVoiceContext = { bt: webBt };
vm.createContext(webVoiceContext);
vm.runInContext(
  `${webBridgeSource.slice(webVoiceStart, webVoiceEnd)}\nthis.normalizeVoiceError = normalizeVoiceError;`,
  webVoiceContext,
  { filename: "web/bridge.js" },
);
const webNormalizeVoiceError = webVoiceContext.normalizeVoiceError;
const webCodedEmpty = webNormalizeVoiceError({
  code: "asr_no_speech",
  category: "empty_result",
  stage: "transcribing",
  message: "识别完成，但没有识别到语音内容",
});
assert.strictEqual(
  webCodedEmpty.category,
  "empty_result",
  "relayed Rust category must drive the empty-result friendly card on the web lane",
);
assert.strictEqual(webCodedEmpty.message, webBt("voiceEmptyResult"));
const webCodedTimeout = webNormalizeVoiceError({
  code: "asr_timeout",
  message: "识别超时（120 秒），请缩短录音后重试",
});
assert.strictEqual(
  webCodedTimeout.message,
  webBt("voiceTimeout"),
  "web lane must map a coded error even without a relayed category",
);
assert.strictEqual(webCodedTimeout.diagnostic, "识别超时（120 秒），请缩短录音后重试");
assert.strictEqual(
  webNormalizeVoiceError({ code: "session_mismatch", message: "invalid Session id" }).message,
  webBt("voiceContextMismatch"),
);

// (3) Drift guard: every Rust VoiceCommandError error code must appear in
// both lanes' mapping tables, and the two key sets must stay identical —
// adding a code without a mapping turns this red instead of silently falling
// back to the Chinese raw message.
const rustVoiceErrorSources = `${rustVoiceSource}\n${fs.readFileSync(
  path.join(__dirname, "..", "src-tauri", "src", "app", "commands", "remote_control.rs"),
  "utf8",
)}`;
const rustVoiceCodes = new Set();
for (const match of rustVoiceErrorSources.matchAll(/VoiceCommandError::new\(\s*"([a-z0-9_]+)"/g)) {
  rustVoiceCodes.add(match[1]);
}
assert.ok(
  rustVoiceCodes.size >= 14,
  "drift guard must see the full Rust voice error-code inventory",
);
function extractVoiceCodeKeys(laneSource, laneName) {
  const mapMatch = laneSource.match(/const VOICE_ERROR_CODE_KEYS = \{([\s\S]*?)\n {2}\};/);
  assert.notStrictEqual(mapMatch, null, `${laneName} VOICE_ERROR_CODE_KEYS must exist`);
  return new Set([...mapMatch[1].matchAll(/^\s{4}([a-z0-9_]+):/gm)].map((entry) => entry[1]));
}
const tauriVoiceCodeKeys = extractVoiceCodeKeys(source, "tauri");
const webVoiceCodeKeys = extractVoiceCodeKeys(webBridgeSource, "web");
for (const code of rustVoiceCodes) {
  assert.ok(tauriVoiceCodeKeys.has(code), `tauri VOICE_ERROR_CODE_KEYS is missing Rust code "${code}"`);
  assert.ok(webVoiceCodeKeys.has(code), `web VOICE_ERROR_CODE_KEYS is missing Rust code "${code}"`);
}
assert.strictEqual(
  tauriVoiceCodeKeys.size,
  webVoiceCodeKeys.size,
  "tauri and web VOICE_ERROR_CODE_KEYS must stay in lockstep",
);
for (const code of tauriVoiceCodeKeys) {
  assert.ok(webVoiceCodeKeys.has(code), `web VOICE_ERROR_CODE_KEYS is missing tauri code "${code}"`);
}

// (4) Passthrough-chain guard: the desktop proxy must hand the structured
// code/category to respondToWebAccess, and the Rust command must receive them
// and write them into rpc_response — with any link missing, the web mapping
// is unreachable for Rust errors.
const remoteControlSource = fs.readFileSync(
  path.join(__dirname, "..", "src", "platform", "tauri", "bridge", "remote-control.js"),
  "utf8",
);
assert.match(
  remoteControlSource,
  /typeof error\.code === "string"[\s\S]*?respondToWebAccess\(\s*requestId,\s*false,\s*null,\s*error && error\.message \? error\.message : error,\s*structured \? error\.code : null,\s*errorCategory/,
  "desktop RPC proxy must forward the structured error code and category",
);
assert.match(
  remoteControlSource,
  /errorCode: errorCode \|\| null,[\s\S]*?errorCategory: errorCategory \|\| null,/,
  "respondToWebAccess must pass errorCode/errorCategory to web_access_rpc_respond",
);
const bootstrapSource = fs.readFileSync(
  path.join(__dirname, "..", "src", "platform", "web", "bootstrap.js"),
  "utf8",
);
assert.match(
  bootstrapSource,
  /error\.code = message\.error_code \|\| "rpc_failed";\s*\n\s*if \(message\.error_category\) error\.category = message\.error_category;/,
  "browser RPC client must restore the relayed category onto the reconstructed Error",
);
assert.match(
  fs.readFileSync(
    path.join(__dirname, "..", "src-tauri", "src", "features", "remote_control", "manager", "rpc.rs"),
    "utf8",
  ),
  /"error_code": completion\.error_code,\s*\n\s*"error_category": completion\.error_category,/,
  "rpc_response must carry error_code/error_category to the browser",
);

  console.log("voice_input_error_logic: ok");
// eslint-disable-next-line unicorn/prefer-top-level-await -- smoke script keeps its existing async main() structure
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
