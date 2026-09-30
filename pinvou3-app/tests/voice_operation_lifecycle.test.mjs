#!/usr/bin/env node
// Voice operation lifecycle contract tests.
//
// Voice ownership outlives the recording: the operationId handed to the
// composer before the async writeback stays associated with exactly the
// draft/session that recorded it, until the draft is really sent or
// abandoned (docs/voice-input-compatibility.md). Each test extracts the real
// code from the bridge/hook and drives it in a vm sandbox. Both lanes keep
// terminal bookkeeping (end/dedup of the operation state machine) without a
// statistics sink, so the assertions pin the state machine itself.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import test from "node:test";

const root = path.resolve(import.meta.dirname, "..");
// Normalize CRLF like the other source-slicing suites do (web_access_contract,
// project_session_drag_contract, …): the slice anchors below search LF-only
// delimiters, and a Windows checkout with core.autocrlf=true must not break
// the suite.
const read = (file) => fs.readFileSync(path.join(root, file), "utf8").replace(/\r\n/g, "\n");

const sources = {
  desktop: read("src/platform/tauri/bridge/voice.js"),
  web: read("src/platform/web/bridge.js"),
};

// Both lanes declare the operation map; the desktop block spans the map up to
// normalizeVoiceMode (abandonVoiceResult sits elsewhere in the file and is
// appended separately), the web block spans up to the forwarder border.
function operationCore(source, lane) {
  // The desktop lifecycle sits from the voiceToken head through
  // normalizeVoiceMode (plus the distant abandonVoiceResult), the web block
  // from webVoiceToken to the forwarder border.
  const blockStart = lane === "desktop"
    ? source.indexOf("  // Opaque token for recording sessions and operations")
    : source.indexOf("const voiceOperations = new Map();");
  if (blockStart < 0) throw new Error(`${lane} bridge must define the voice operation lifecycle`);
  if (lane === "desktop") {
    const blockEnd = source.indexOf("  function normalizeVoiceMode(mode) {", blockStart);
    const abandonStart = source.indexOf("  function abandonVoiceResult(operationId) {", blockStart);
    const anchor = source.indexOf("if (!operationId) abandonCompletedVoiceResult();", abandonStart);
    // Two newlines past the anchor statement to include the closing "  }".
    const abandonEnd = source.indexOf("\n", source.indexOf("\n", anchor) + 1) + 1;
    if (blockEnd < 0 || abandonStart < 0 || abandonEnd < abandonStart) {
      throw new Error("desktop bridge must keep the voice operation lifecycle block");
    }
    return source.slice(blockStart, blockEnd) + source.slice(abandonStart, abandonEnd);
  }
  const blockEnd = source.indexOf("function requestVoiceMedia(session, constraints, timeoutMs) { return pinvouSharedweb()", blockStart);
  if (blockEnd < 0) throw new Error("web bridge must define the voice operation map");
  return source.slice(blockStart, blockEnd);
}

function baseSandbox(state) {
  const sandbox = { console, Math, Date, Promise, Object, Array, JSON, Set, Map };
  vm.createContext(sandbox);
  sandbox.state = state;
  sandbox.notify = () => {};
  sandbox.invoke = async () => null;
  // Stubs for module-level helpers the extracted slices reference but do not
  // define. (voiceToken/getVoiceOperationId/abandon* are defined inside the
  // desktop slice itself and are intentionally not stubbed here.)
  sandbox.activeVoiceInput = null;
  sandbox.bt = (key) => key;
  sandbox.webVoiceToken = (prefix) => `${prefix}1`;
  sandbox.clearVoiceInput = () => {};
  sandbox.setVoiceInputStatus = () => {};
  sandbox.emitVoiceDiagnostic = () => {};
  return sandbox;
}

function withOperations(h) {
  h.operation = (id, ownerKind = "chat", sessionId = null) => {
    const value = { operationId: id, ownerKind, sessionId, draftEpoch: 4, startedAt: Date.now(), telemetryTerminal: false };
    h.api.rememberVoiceOperation(value);
    value.voiceResultReady = true;
    return value;
  };
  return h;
}

function operationHarness(lane) {
  const state = { activeSessionId: null, draftEpoch: 4, composerDraft: "", voiceInput: null };
  const sandbox = baseSandbox(state);
  vm.runInContext(operationCore(sources[lane], lane), sandbox);
  vm.runInContext(`this.api = {
    rememberVoiceOperation, getVoiceOperationId, beginVoiceSubmission,
    voiceOperationSessionId, completeVoiceSubmission, trackVoiceTerminal,
    abandonCompletedVoiceResult, abandonVoiceResult, dismissVoiceInput,
    hasVoiceSubmissionPending, rebindVoiceOperationToDraft,
    ...(typeof rebindVoiceDraftAfterRollback === "function" ? { rebindVoiceDraftAfterRollback } : {}),
  };`, sandbox);
  return withOperations({ api: sandbox.api, state });
}

test("desktop: a materialized first turn binds its real session, never another draft", () => {
  const h = operationHarness("desktop");
  const operation = h.operation("voiceop-materialized");
  h.api.beginVoiceSubmission(operation.operationId);
  h.api.completeVoiceSubmission(operation.operationId, "new-real-session", false);
  assert.equal(h.api.getVoiceOperationId(null, "chat"), null);
  assert.equal(h.api.getVoiceOperationId("new-real-session", "chat"), operation.operationId);
  h.api.beginVoiceSubmission(operation.operationId);
  h.api.completeVoiceSubmission(operation.operationId, "new-real-session", true);
  assert.equal(operation.telemetryTerminal, true, "acceptance ends the operation");
  assert.equal(h.api.getVoiceOperationId("new-real-session", "chat"), null, "a terminal operation is never adopted again");
});

test("web: a materialized first turn keeps its binding without a statistics sink", () => {
  const h = operationHarness("web");
  const operation = h.operation("voiceop-materialized");
  h.api.beginVoiceSubmission(operation.operationId, "web-session");
  assert.equal(h.api.voiceOperationSessionId(operation.operationId), "web-session");
  h.api.completeVoiceSubmission(operation.operationId, "web-session", false);
  assert.equal(h.api.getVoiceOperationId("web-session", "chat"), operation.operationId);
});

test("desktop: a cancel during admission waits for the admission result", () => {
  const h = operationHarness("desktop");
  const operation = h.operation("voiceop-queued");
  h.api.beginVoiceSubmission(operation.operationId);
  // The queued terminal is held, not recorded; the admission outcome wins.
  h.api.trackVoiceTerminal("voice_cancelled", operation, { stage: "recognition" });
  assert.equal(operation.telemetryTerminal, false);
  // Accepted: the queued cancel is never recorded as a cancelled operation;
  // acceptance consumes it and ends the operation in one piece.
  h.api.completeVoiceSubmission(operation.operationId, null, true);
  assert.equal(operation.telemetryTerminal, true);
  assert.equal(operation.pendingTerminal, null, "acceptance consumes the queued cancel");
});

test("web: terminal bookkeeping consumes the queued terminal on rejection", () => {
  const h = operationHarness("web");
  const operation = h.operation("voiceop-terminal");
  h.api.beginVoiceSubmission(operation.operationId);
  h.api.trackVoiceTerminal("voice_cancelled", operation, { stage: "recognition" });
  assert.equal(operation.telemetryTerminal, false);
  h.api.completeVoiceSubmission(operation.operationId, null, false);
  assert.equal(operation.telemetryTerminal, true);
});

test("desktop: draft epoch isolation — a new draft cannot inherit voice provenance", () => {
  const h = operationHarness("desktop");
  const operation = h.operation("voiceop-oldepoch");
  assert.equal(h.api.getVoiceOperationId(null, "chat"), operation.operationId);
  // A materialized session must not adopt the draft's association either.
  assert.equal(h.api.getVoiceOperationId("some-session", "chat"), null);
  // Epoch moves on (enterDraft): the old draft's operation is no longer found.
  h.state.draftEpoch = 5;
  assert.equal(h.api.getVoiceOperationId(null, "chat"), null, "new draft cannot inherit retained voice provenance");
});

test("desktop: only a proven same-draft rollback may rebind the draft association", () => {
  const h = operationHarness("desktop");
  const operation = h.operation("voiceop-rollback");
  // The web lane intentionally does not expose rebind (no same-draft rollback
  // semantics there); the desktop owns it.
  assert.equal(typeof h.api.rebindVoiceDraftAfterRollback, "function");
  h.state.draftEpoch = 5;
  // Compare-and-set: the from-epoch must match, and only +1 is accepted.
  assert.equal(h.api.rebindVoiceDraftAfterRollback(operation.operationId, 4, 5), true);
  assert.equal(operation.draftEpoch, 5);
  assert.equal(h.api.getVoiceOperationId(null, "chat"), operation.operationId);
  // A second migration attempt must not advance again.
  assert.equal(h.api.rebindVoiceDraftAfterRollback(operation.operationId, 4, 5), false);
  // A non-adjacent epoch jump is never a rollback.
  assert.equal(h.api.rebindVoiceDraftAfterRollback(operation.operationId, 5, 7), false);
  assert.equal(operation.draftEpoch, 5);
});

// ── The departed-draft rebind: a restored text carries its operation into the live epoch ──
test("desktop: a draft-return rebind re-adopts the restored operation, and only that shape", () => {
  const h = operationHarness("desktop");
  const operation = h.operation("voiceop-departed");
  h.state.draftEpoch = 9;
  assert.equal(h.api.getVoiceOperationId(null, "chat"), null, "the dead recording-time epoch cannot adopt");
  assert.equal(h.api.rebindVoiceOperationToDraft(operation.operationId, 9), true);
  assert.equal(operation.draftEpoch, 9);
  assert.equal(h.api.getVoiceOperationId(null, "chat"), operation.operationId, "the restored text's manual retry keeps its provenance");
  // Refusals: a terminal operation, a session-bound operation, a non-chat operation.
  h.api.trackVoiceTerminal("voice_cancelled", operation);
  assert.equal(h.api.rebindVoiceOperationToDraft(operation.operationId, 9), false, "terminal operations never rebind");
  const bound = h.operation("voiceop-bound", "chat", "session-a");
  assert.equal(h.api.rebindVoiceOperationToDraft(bound.operationId, 9), false, "session-bound operations never rebind");
  const codex = h.operation("voiceop-codex", "codex", null);
  assert.equal(h.api.rebindVoiceOperationToDraft(codex.operationId, 9), false, "non-chat operations never rebind");
});

test("web: the departed-draft rebind keeps the restored operation adoptable too", () => {
  const h = operationHarness("web");
  const operation = h.operation("voiceop-webdeparted");
  h.state.draftEpoch = 8;
  assert.equal(h.api.rebindVoiceOperationToDraft(operation.operationId, 8), true);
  assert.equal(h.api.getVoiceOperationId(null, "chat"), operation.operationId);
});

// ── Desktop ownership claim: the microphone must stay closed until the claim lands ──

// Builds a sandbox whose startVoiceInput slice passes the mediaDevices and
// AudioContext guards, so the test exercises the actual claim gate instead
// of failing at the first guard (a stub gap here used to make this suite
// pass while a claim-after-microphone reordering went undetected).
function startVoiceInputSandbox(state, handlers) {
  const sandbox = baseSandbox(state);
  sandbox.bt = (key) => key;
  sandbox.window = { AudioContext: function AudioContext() {} };
  sandbox.navigator = { mediaDevices: { getUserMedia: async () => ({}) } };
  sandbox.invoke = async () => ({ ready: true });
  sandbox.emitVoiceDiagnostic = () => {};
  sandbox.normalizeVoiceMode = (mode) => mode;
  sandbox.normalizeVoiceError = (error) => error;
  sandbox.voiceFlowError = (category, stage, message) =>
    Object.assign(new Error(message), { category, stage });
  sandbox.currentVoiceWindowLabel = () => "detached-b";
  sandbox.voiceToken = (prefix) => `${prefix}1`;
  sandbox.VOICE_DEVICE_PROBE_TIMEOUT_MS = 5;
  sandbox.VOICE_DEVICE_REQUEST_TIMEOUT_MS = 5;
  sandbox.probeVoiceAudioInput = handlers.probe;
  sandbox.requestVoiceMedia = handlers.requestMedia;
  sandbox.syncVoiceShortcutRecording = handlers.sync;
  sandbox.cleanupVoiceInputSession = () => {};
  sandbox.rememberVoiceOperation = () => {};
  sandbox.getVoiceOperationId = () => null;
  sandbox.abandonVoiceResult = () => {};
  sandbox.abandonCompletedVoiceResult = () => {};
  sandbox.trackVoiceTerminal = () => {};
  sandbox.statuses = [];
  sandbox.statusPatches = [];
  sandbox.setVoiceInputStatus = (status, patch) => {
    sandbox.statuses.push(status);
    sandbox.statusPatches.push(patch || null);
  };
  const source = sources.desktop;
  const start = source.indexOf("  async function startVoiceInput(");
  const end = source.indexOf("function cancelVoiceInput()", start);
  assert.ok(start >= 0 && end > start, "desktop startVoiceInput must exist");
  vm.runInContext(`${source.slice(start, end)}\nthis.startVoiceInput = startVoiceInput;`, sandbox);
  return sandbox;
}

test("desktop: no microphone probe before the Rust ownership claim resolves", async () => {
  let resolveClaim;
  const claim = new Promise((resolve) => { resolveClaim = resolve; });
  const claimCalls = [];
  let microphoneProbes = 0;
  let mediaRequests = 0;
  const sandbox = startVoiceInputSandbox({
    activeSessionId: null,
    draftEpoch: 4,
    composerDraft: "",
    voiceAsrSetup: { installing: false },
    voiceInput: { status: "idle" },
  }, {
    sync: (label, token) => { claimCalls.push([label, token]); return claim; },
    probe: () => { microphoneProbes += 1; return true; },
    requestMedia: async () => { mediaRequests += 1; return {}; },
  });
  const starting = sandbox.startVoiceInput("draft text", () => {}, { mode: "dictation" });
  await new Promise((resolve) => { setImmediate(resolve); });
  assert.deepEqual(claimCalls, [["detached-b", "voice_1"]], "the claim must carry this window's label and the operation token");
  assert.equal(microphoneProbes, 0, "the device probe must wait for the ownership claim");
  assert.equal(mediaRequests, 0, "the microphone must not open before the ownership claim");
  resolveClaim(false);
  await starting;
  assert.equal(mediaRequests, 0, "a rejected claim must fail the start before any microphone");
  assert.equal(sandbox.statuses[sandbox.statuses.length - 1], "failed");
});

test("desktop: a start that fails after claiming releases the ownership claim", async () => {
  const ownershipCalls = [];
  const sandbox = startVoiceInputSandbox({
    activeSessionId: null,
    draftEpoch: 4,
    composerDraft: "",
    voiceAsrSetup: { installing: false },
    voiceInput: { status: "idle" },
  }, {
    sync: (label, token) => { ownershipCalls.push([label, token]); return Promise.resolve(true); },
    probe: () => false,
    requestMedia: async () => ({}),
  });
  await sandbox.startVoiceInput("draft text", () => {}, { mode: "dictation" });
  assert.deepEqual(
    ownershipCalls,
    [["detached-b", "voice_1"], [null, "voice_1"]],
    "the failed start must release its claim, or every later start in every window fails closed",
  );
  assert.equal(sandbox.statuses[sandbox.statuses.length - 1], "failed");
});

test("desktop: a claim IPC failure fails closed without probing and reports the check, not another window", async () => {
  let microphoneProbes = 0;
  const sandbox = startVoiceInputSandbox({
    activeSessionId: null,
    draftEpoch: 4,
    composerDraft: "",
    voiceAsrSetup: { installing: false },
    voiceInput: { status: "idle" },
  }, {
    sync: () => Promise.resolve("error"),
    probe: () => { microphoneProbes += 1; return true; },
    requestMedia: async () => ({}),
  });
  await sandbox.startVoiceInput("draft text", () => {}, { mode: "dictation" });
  assert.equal(microphoneProbes, 0, "an unverifiable claim must still fail closed before the microphone");
  assert.equal(sandbox.statuses[sandbox.statuses.length - 1], "failed");
  assert.equal(
    sandbox.statusPatches[sandbox.statusPatches.length - 1].message,
    "voiceMicOwnershipUnavailable",
    "a claim IPC failure must not be misattributed to another window recording",
  );
});

// ── Web lane parity: clearing the notice ends the unsent operation ──
test("web: clearing the finished notice abandons the unsent operation like the desktop lane", () => {
  const webSource = sources.web;
  const start = webSource.indexOf("  // Local override (mirrors the desktop lane)");
  const bodyStart = webSource.indexOf("function clearVoiceInput() {", start);
  assert.ok(start >= 0 && bodyStart > start, "web bridge must keep the local clearVoiceInput override");
  const end = webSource.indexOf("\n  }\n", bodyStart);
  assert.ok(end > bodyStart, "web clearVoiceInput override must stay a self-contained slice");
  const state = { activeSessionId: "web-session", draftEpoch: 4, composerDraft: "", voiceInput: null };
  const sandbox = baseSandbox(state);
  const calls = { finished: [], abandoned: 0, stage: null, statuses: [] };
  sandbox.activeVoiceInput = null;
  sandbox.finishVoiceInput = (cancelled) => { calls.finished.push(cancelled); };
  sandbox.abandonCompletedVoiceResult = (stage) => { calls.abandoned += 1; calls.stage = stage; };
  sandbox.setVoiceInputStatus = (status) => { calls.statuses.push(status); };
  vm.runInContext(`${webSource.slice(bodyStart, end + 4)}\nthis.clearVoiceInput = clearVoiceInput;`, sandbox);
  // An idle-notice clear (nothing recording) ends the unsent operation.
  sandbox.clearVoiceInput();
  assert.equal(calls.abandoned, 1, "clearing the idle notice must abandon the unsent operation");
  assert.equal(calls.stage, "recognition");
  assert.deepEqual(calls.statuses, ["idle"], "the notice resets to idle");
  assert.deepEqual(calls.finished, [], "nothing is recording, so no teardown fires");
  // A live recording still tears down instead of abandoning.
  sandbox.activeVoiceInput = { id: "voice_2" };
  sandbox.clearVoiceInput();
  assert.deepEqual(calls.finished, [true], "a live recording is cancelled by the clear");
  assert.equal(calls.abandoned, 1, "no extra abandon while recording");
});

// ── Web first-turn admission certainty ──
test("web: outcome_unknown keeps the first-turn retry association, explicit rejection consumes it", async () => {
  const source = sources.web;
  for (const code of ["permission_denied", "outcome_unknown"]) {
    const state = { activeSessionId: null, draftEpoch: 4, composerDraft: "" };
    const sandbox = baseSandbox(state);
    sandbox.bt = (key) => key;
    sandbox.IS_WEB = true;
    sandbox.findFirstTurnItem = () => ({});
    sandbox.firstTurnStillVisible = () => true;
    sandbox.restoreFirstTurnUiState = () => {};
    sandbox.beginVoiceSubmission = (operationId) => { sandbox.lastBegin = operationId; };
    sandbox.completeVoiceSubmission = (operationId, sessionId, accepted) => {
      sandbox.lastComplete = { operationId, sessionId, accepted };
    };
    sandbox.invokeWithRequestId = async () => {
      throw Object.assign(new Error(code), { code });
    };
    const start = source.indexOf("  async function runFirstTurnSubmission(submission) {");
    const end = source.indexOf("  function submitFirstWebTurn(", start);
    assert.ok(start >= 0 && end > start, "web runFirstTurnSubmission must exist");
    vm.runInContext(`${source.slice(start, end)}\nthis.runFirstTurnSubmission = runFirstTurnSubmission;`, sandbox);
    const submission = { voiceOperationId: `first-turn-${code}`, clientMessageId: "first-turn" };
    await sandbox.runFirstTurnSubmission(submission);
    if (code === "outcome_unknown") {
      assert.equal(sandbox.lastBegin, submission.voiceOperationId);
      assert.equal(sandbox.lastComplete, undefined, "an unknown outcome must not consume the admission");
    } else {
      assert.equal(sandbox.lastComplete.accepted, false, "an explicit rejection completes as not accepted");
    }
  }
});

// ── The composer adapter must abandon the result exactly on the discard paths ──
test("hook: discarding a stale edit preview abandons the operation", async () => {
  const hookSource = read("src/features/voice-composer/useComposerVoiceInput.js");
  const discarded = [];
  const dismissals = [];
  const sandbox = { console, Math, Date, Promise, Object, Array, JSON };
  vm.createContext(sandbox);
  const editPreviewRef = { current: null };
  const state = { activeSessionId: null, draftEpoch: 4, composerDraft: "", voiceInput: null };
  sandbox.state = state;
  sandbox.notify = () => {};
  const adapterRef = { current: {
    bridge: { available: true, voice: {
      abandonVoiceResult: (id) => discarded.push(id),
      cancelVoiceInput() {}, clearVoiceInput() {},
      dismissVoiceInput: () => dismissals.push("dismiss"),
    } },
  } };
  sandbox.adapterRef = adapterRef;
  sandbox.editPreviewRef = editPreviewRef;
  sandbox.editPreview = null;
  sandbox.setEditPreview = () => {};
  sandbox.closeVoice = () => {};
  sandbox.trimDraft = (value) => String(value || "").trim();
  sandbox.useCallback = (fn) => fn;
  // The slice starts at dismissVoice: applyVoiceEditPreview closes over it,
  // and its wiring (dismiss — not closeVoice/abandon) is part of the
  // contract under test.
  const discardStart = hookSource.indexOf("  // Dismissing the finished notice");
  const applyEnd = hookSource.indexOf("  const clearStaleVoiceState = useCallback(");
  assert.ok(discardStart >= 0 && applyEnd > discardStart, "the hook keeps its dismiss/discard/apply block");
  vm.runInContext(`${hookSource.slice(discardStart, applyEnd)}\nthis.applyPreview = applyVoiceEditPreview;`, sandbox);
  const preview = { original: "original", next: "edited", context: { operationId: "voiceop-preview" } };
  let draft = "manually changed";
  adapterRef.current.getDraft = () => draft;
  adapterRef.current.setDraft = (value) => { draft = value; };
  editPreviewRef.current = preview;
  sandbox.editPreview = preview;
  const applied = await sandbox.applyPreview({});
  assert.equal(applied, false);
  assert.deepEqual(discarded, ["voiceop-preview"]);
  assert.equal(draft, "manually changed", "a drifted draft must not be replaced by the preview");

  // Applying a matching preview (without sending) dismisses the notice and
  // keeps the operation alive: no abandon, no cancel.
  draft = "original";
  discarded.length = 0;
  editPreviewRef.current = preview;
  sandbox.editPreview = preview;
  const appliedClean = await sandbox.applyPreview({});
  assert.equal(appliedClean, true);
  assert.deepEqual(dismissals, ["dismiss"], "applying a preview dismisses without ending the operation");
  assert.deepEqual(discarded, [], "applying a preview must not abandon its operation");
  assert.equal(draft, "edited", "the preview text is applied to the draft");
});

// ── ChatView source contracts for the submission protocol ──
test("chatview: the task lane consumes only its own draft and maps restored to not-accepted", () => {
  const chatViewSource = read("src/features/chat/ChatView.jsx");
  assert.match(
    chatViewSource,
    /sendChatMessage\(outgoingText, \{ \.\.\.context, draftOwner: owner \}\)/,
    // outgoingText = the mention block (when the gate is on and chips are
    // picked) ahead of constrained.text — the mention feature extends the
    // task lane; the draft owner still rides through sendChatMessage.
    "the task lane must pass the draft owner through sendChatMessage",
  );
  assert.match(
    chatViewSource,
    /return result === 'restored' \? false : result;/,
    "a restored delivery must read as not accepted, without restoring twice",
  );
  assert.match(
    chatViewSource,
    /if \(result === false && bridge\.chat\.restoreTaskDraft\) \{\s*\n\s*bridge\.chat\.restoreTaskDraft\(constrained\.text, owner\);/,
    "an explicitly rejected send restores the draft through the scoped restore",
  );
});

// ── Rust ownership claim release/spoof rules (pure state machine mirror) ──
test("ownership: the JS sync rejects tokenless clears; the claim/release rules stay in the Rust tests", async () => {
  // The real stale-teardown/spoof rules are pinned by the Rust unit tests
  // (update_recording_owner); this pin only covers the shipped JS guard
  // surface: tokenless syncs are rejected outright and the IPC carries the
  // operation token.
  const rustSource = read("src-tauri/src/features/voice_shortcut.rs");
  const bridgeSource = sources.desktop;
  assert.match(
    bridgeSource,
    /function syncVoiceShortcutRecording\(label, token\) \{[\s\S]*?if \(!token\) return false;/,
    "tokenless ownership syncs must be rejected rather than clearing another WebView's claim",
  );
  assert.match(
    bridgeSource,
    /invoke\("set_voice_shortcut_recording", \{ label: label \|\| null, token \}\)/,
    "the ownership IPC must carry the operation token",
  );
  assert.match(
    rustSource,
    /fn update_recording_owner\(/,
    "Rust keeps the claim/release state machine testable (update_recording_owner)",
  );
});

// ── Lane identity and cross-context cancellation wiring ──
test("identity: the hook forwards lane identity and cancels an in-flight recording on context change", () => {
  const hookSource = read("src/features/voice-composer/useComposerVoiceInput.js");
  // startVoiceInput must carry the adapter's lane identity: without it every
  // operation is booked under the chat lane, the bridge's ownerKind
  // exemption is dead code, and the codex session binding silently no-ops.
  const startCall = hookSource.indexOf("bridge.voice.startVoiceInput(");
  const ownerOption = hookSource.indexOf("ownerKind: current.ownerKind || 'chat',", startCall);
  const sessionOption = hookSource.indexOf("sessionId: current.sessionId || null,", startCall);
  assert.ok(startCall >= 0, "the hook must start recordings through the bridge");
  assert.ok(ownerOption > startCall, "the start options must forward the adapter's ownerKind");
  assert.ok(sessionOption > ownerOption, "the start options must forward the adapter's sessionId");
  // An identity change must cancel an in-flight recording: the seconds-long
  // transcription window otherwise delivers (and in task mode auto-sends)
  // into whichever context is active at writeback time. The cancel must be
  // gated on the parked-submission check: first-turn materialization flips
  // this adapter's identity mid-send, and an ungated cancel would kill the
  // completion notice of a message that still lands.
  const identityAnchor = hookSource.indexOf("const previous = voiceContextIdentityRef.current;");
  const activeCheck = hookSource.indexOf("isVoiceActive({ status: voiceInput && voiceInput.status })", identityAnchor);
  const parkedGate = hookSource.indexOf("hasVoiceSubmissionPending", activeCheck);
  const cancelCall = hookSource.indexOf("if (!parked) cancelVoice();", activeCheck);
  assert.ok(identityAnchor >= 0, "the identity-change effect must exist");
  assert.ok(activeCheck > identityAnchor, "the identity-change effect must check for an in-flight recording");
  assert.ok(parkedGate > activeCheck, "the identity-change cancel must consult the parked-submission gate");
  assert.ok(cancelCall > parkedGate, "the identity-change effect must cancel only when no submission is parked");
  // The web session creation must honor the caller's identity exactly like
  // the desktop lane (explicit null included).
  assert.match(
    sources.web,
    /sessionId: options && Object\.prototype\.hasOwnProperty\.call\(options, "sessionId"\)\s*\?\s*options\.sessionId\s*:\s*state\.activeSessionId \|\| null,/,
    "the web lane must read the caller's sessionId like the desktop lane",
  );
  // The claim must never carry an unreadable window label: the Rust command
  // treats it as a release and the user would be told another window is
  // recording.
  assert.match(
    sources.desktop,
    /if \(label === ""\) return "error";/,
    "an empty window label must report the ownership check as unavailable",
  );
  for (const [lane, source] of [["desktop", sources.desktop], ["web", sources.web]]) {
    // A failed start must not leave partial PCM pinned on the terminal
    // operation record; the release must follow the teardown (which detaches
    // the audio callback first).
    assert.match(
      source,
      /catch \(err\) \{\s*cleanupVoiceInputSession\(session\);[\s\S]{0,200}?session\.chunks = null;/,
      `${lane} start failures must release the partial PCM after the teardown`,
    );
  }
  // Web parity: a terminalized operation must not be overwritten by the
  // completed notice during the writeback await.
  assert.match(
    sources.web,
    /if \(session\.telemetryTerminal\) return;\s*setVoiceInputStatus\("completed"/,
    "the web completed notice must respect the terminal latch like the desktop lane",
  );
  // A transcription that lands after supersession must terminalize its
  // operation (the continuation can never deliver; an early plain return
  // would leave the record non-terminal, unadoptable and unswept forever).
  for (const [lane, source] of [["desktop", sources.desktop], ["web", sources.web]]) {
    assert.match(
      source,
      /if \(activeVoiceInput !== session\) \{\s*\/\/ Superseded mid-transcription[\s\S]{0,200}?trackVoiceTerminal\("voice_cancelled", session\);\s*return;\s*\}/,
      `${lane} must terminalize an operation whose transcription lands after supersession`,
    );
  }
  // The web enterDraft discard is exercised against the real bridge slice in
  // the dedicated test below (a source-regex pin here broke on a comment edit
  // inside its own window).
});

test("desktop: an unreadable window label reports the ownership check unavailable instead of another window", async () => {
  const source = sources.desktop;
  // The slice must include the serialization chain variable: the sync helper
  // queues every attempt through it.
  const start = source.indexOf("  let voiceOwnershipSyncTail = Promise.resolve();");
  const end = source.indexOf("  function setVoiceShortcutEnabled(enabled) {", start);
  assert.ok(start >= 0 && end > start, "desktop must keep the ownership sync helper");
  const invokeCalls = [];
  const sandbox = baseSandbox({ activeSessionId: null, draftEpoch: 4, composerDraft: "", voiceInput: null });
  // Copy the args into the host realm: objects created inside the vm context
  // carry the sandbox's Object.prototype, and deepStrictEqual compares
  // prototypes across realms.
  sandbox.invoke = async (command, args) => {
    invokeCalls.push([command, { label: args.label, token: args.token }]);
    return true;
  };
  vm.runInContext(`${source.slice(start, end)}\nthis.sync = syncVoiceShortcutRecording;`, sandbox);
  assert.equal(
    await sandbox.sync("", "voice_1"), "error",
    "an unreadable window label must not go over the wire as a claim",
  );
  assert.deepEqual(invokeCalls, [], "no ownership IPC may fire without a window label");
  assert.equal(await sandbox.sync(null, "voice_1"), true, "a null label is a release and still dispatches");
  assert.deepEqual(
    invokeCalls,
    [["set_voice_shortcut_recording", { label: null, token: "voice_1" }]],
    "the release path keeps dispatching with an explicit null label",
  );
});

test("desktop: ownership syncs reach the backend in call order", async () => {
  // The release-then-reclaim handoff relies on backend ordering: the chain
  // must not resolve a later sync before the earlier one finished its IPC.
  const source = sources.desktop;
  const start = source.indexOf("  let voiceOwnershipSyncTail = Promise.resolve();");
  const end = source.indexOf("  function setVoiceShortcutEnabled(enabled) {", start);
  const sandbox = baseSandbox({ activeSessionId: null, draftEpoch: 4, composerDraft: "", voiceInput: null });
  const order = [];
  let releaseIpc;
  const releaseGate = new Promise((resolve) => { releaseIpc = resolve; });
  sandbox.invoke = async (command, args) => {
    order.push(`start:${args.label}`);
    if (args.label === null) await releaseGate;
    order.push(`end:${args.label}`);
    return true;
  };
  vm.runInContext(`${source.slice(start, end)}\nthis.sync = syncVoiceShortcutRecording;`, sandbox);
  const claim = sandbox.sync("detached-b", "voice_1");
  await claim;
  const release = sandbox.sync(null, "voice_1");
  const reclaim = sandbox.sync("detached-b", "voice_2");
  let reclaimSettled = false;
  reclaim.then(() => { reclaimSettled = true; });
  await new Promise((resolve) => { setImmediate(resolve); });
  assert.equal(reclaimSettled, false, "the reclaim must wait behind the in-flight release IPC");
  releaseIpc();
  await Promise.all([release, reclaim]);
  assert.deepEqual(order, [
    "start:detached-b", "end:detached-b",
    "start:null", "end:null",
    "start:detached-b", "end:detached-b",
  ], "syncs must run strictly one at a time, in call order");
});

// ── Web enterDraft discard: executed against the real bridge slice ──
test("web: enterDraft closes the operation of a finished first-turn submission and leaves an in-flight one to its RPC", () => {
  const webSource = sources.web;
  const start = webSource.indexOf("  function enterDraft() {");
  const end = webSource.indexOf("  let ensureSessionInFlight = null;", start);
  assert.ok(start >= 0 && end > start, "web bridge must keep the enterDraft function");
  const state = {
    chatItems: [{ clientMessageId: "c-finished" }, { clientMessageId: "c-inflight" }, { clientMessageId: "c-novoice" }],
    activeSessionId: "web-session",
    messages: [{ id: "m1" }],
    draftEpoch: 4,
    composerDraft: "",
    scheduledRunContext: null,
    scheduledTaskPendingGuide: null,
    modeState: null,
  };
  const sandbox = baseSandbox(state);
  const calls = { complete: [], abandoned: [] };
  sandbox.firstTurnSubmissions = Object.create(null);
  sandbox.firstTurnSubmissions["c-finished"] = { voiceOperationId: "voiceop-finished", inFlight: false };
  sandbox.firstTurnSubmissions["c-inflight"] = { voiceOperationId: "voiceop-inflight", inFlight: true };
  sandbox.sessionSwitchRequestToken = 0;
  sandbox.completeVoiceSubmission = (operationId, sessionId, accepted) => calls.complete.push({ operationId, sessionId, accepted });
  sandbox.abandonVoiceResult = (operationId) => calls.abandoned.push(operationId);
  sandbox.saveWorkingSetTo = () => {};
  sandbox.loadWorkingSetFrom = () => {};
  sandbox.freshBuffer = () => ({});
  sandbox.getBuffer = () => ({});
  sandbox.currentDraftModeState = () => "chat";
  vm.runInContext(`${webSource.slice(start, end)}\nthis.enterDraft = enterDraft;`, sandbox);
  sandbox.enterDraft();
  assert.deepEqual(
    calls.complete,
    [{ operationId: "voiceop-finished", sessionId: null, accepted: false }],
    "a finished submission's voice operation is un-parked and closed",
  );
  assert.deepEqual(calls.abandoned, ["voiceop-finished"], "the closed operation is abandoned outright");
  assert.equal("c-finished" in sandbox.firstTurnSubmissions, false, "the submission record is discarded");
  assert.equal("c-inflight" in sandbox.firstTurnSubmissions, false, "the in-flight record is discarded too");
  assert.equal(
    calls.complete.some((call) => call.operationId === "voiceop-inflight"),
    false,
    "an in-flight submission is left to its own RPC settlement (closing it here would mark a delivered message cancelled)",
  );
  assert.equal(state.draftEpoch, 5, "enterDraft still advances the draft epoch");
});

test("desktop: the identity-cancel gate reports exactly a parked submission", () => {
  const h = operationHarness("desktop");
  const operation = h.operation("voiceop-gate");
  h.state.voiceInput = { status: "transcribing", operationId: operation.operationId };
  assert.equal(h.api.hasVoiceSubmissionPending(), false, "an unparked operation does not gate the cancel");
  h.api.beginVoiceSubmission(operation.operationId);
  assert.equal(h.api.hasVoiceSubmissionPending(), true, "a parked submission gates the identity-change cancel");
  h.api.completeVoiceSubmission(operation.operationId, null, true);
  assert.equal(h.api.hasVoiceSubmissionPending(), false, "settlement un-parks the gate");
});

test("web: an accepted submission ends the operation like the desktop lane", () => {
  const h = operationHarness("web");
  const operation = h.operation("voiceop-web-accepted");
  h.api.beginVoiceSubmission(operation.operationId, "web-session");
  h.api.completeVoiceSubmission(operation.operationId, "web-session", true);
  assert.equal(operation.telemetryTerminal, true, "acceptance ends the operation");
  assert.equal(h.api.getVoiceOperationId("web-session", "chat"), null, "a terminal operation is never adopted again");
});

test("desktop: dismissing the finished notice keeps the operation until the next recording", () => {
  const h = operationHarness("desktop");
  const operation = h.operation("voiceop-dismissed");
  h.state.voiceInput = { status: "idle", operationId: operation.operationId };
  h.api.dismissVoiceInput();
  assert.equal(operation.telemetryTerminal, false, "dismissing must not end the unsent operation");
  assert.equal(operation.dismissed, true, "the dismissal is marked so the next start can sweep it");
  assert.equal(h.api.getVoiceOperationId(null, "chat"), operation.operationId, "a dismissed operation stays adoptable by a manual send");
  const next = { operationId: "voiceop-next", ownerKind: "chat", sessionId: null, startedAt: Date.now(), telemetryTerminal: false };
  h.api.rememberVoiceOperation(next);
  next.voiceResultReady = true;
  assert.equal(h.api.getVoiceOperationId(null, "chat"), "voiceop-next", "the next recording start sweeps the never-sent dismissal");
});

// ── Scoped task-draft restore (real chat.js code): retention, consumption, settlement ──
const chatSource = read("src/platform/tauri/bridge/chat.js");

function restoreHarness(stateOverrides) {
  const state = { activeSessionId: null, draftEpoch: 4, composerDraft: "", ...stateOverrides };
  const sandbox = { console, Math, Date, Promise, Object, Array, JSON, String, Number, Boolean };
  vm.createContext(sandbox);
  sandbox.state = state;
  const calls = { complete: [], steer: [], prefill: [], rebind: [] };
  sandbox.voice = () => ({
    rebindVoiceDraftAfterRollback: () => false,
    rebindVoiceOperationToDraft: (operationId, toEpoch) => calls.rebind.push({ operationId, toEpoch }),
    completeVoiceSubmission: (operationId, sessionId, accepted) => calls.complete.push({ operationId, sessionId, accepted }),
    voiceOperationSessionId: () => null,
  });
  sandbox.restoreSteerText = (sid, text) => calls.steer.push({ sid, text });
  sandbox.prefillComposer = (text, append) => calls.prefill.push({ text, append });
  const start = chatSource.indexOf("  const pendingTaskDraftRecovery = { buffer: null };");
  const end = chatSource.indexOf("  // Per-session in-flight interrupt flag", start);
  assert.ok(start >= 0 && end > start, "chat bridge must keep the scoped task-draft restore block");
  vm.runInContext(`${chatSource.slice(start, end)}
    this.restoreTaskDraft = restoreTaskDraft;
    this.readComposerDraftWithRecovery = readComposerDraftWithRecovery;`, sandbox);
  return { sandbox, state, calls };
}

test("restore: a rejected send with an active session retains once and consumes on draft return", () => {
  const { sandbox, state, calls } = restoreHarness({ activeSessionId: "session-b", draftEpoch: 5 });
  const owner = { sessionId: null, draftEpoch: 4, operationId: "voiceop-restore", restored: false };
  assert.equal(sandbox.restoreTaskDraft("dictated text", owner), true);
  assert.deepEqual(calls.prefill, [], "the text must never land in the unrelated active composer");
  assert.deepEqual(calls.complete, [{ operationId: "voiceop-restore", sessionId: null, accepted: false }], "the restore settles the parked submission");
  // Returning to the draft allocates a NEW epoch (enterDraft increments
  // unconditionally) — the retained text must still be consumed exactly once.
  state.activeSessionId = null;
  state.draftEpoch = 7;
  assert.equal(sandbox.readComposerDraftWithRecovery(), "dictated text");
  assert.equal(state.composerDraft, "dictated text", "the retained text lands in the composer draft");
  // The text is visibly back: the voice association follows into the new epoch.
  assert.deepEqual(calls.rebind, [{ operationId: "voiceop-restore", toEpoch: 7 }]);
  state.composerDraft = "";
  assert.equal(sandbox.readComposerDraftWithRecovery(), "", "the retained draft is consumed once");
  assert.equal(sandbox.restoreTaskDraft("again", owner), false, "the restore is once-only");
});

test("restore: back in the draft it prefills directly regardless of the epoch", () => {
  const { sandbox, calls } = restoreHarness({ activeSessionId: null, draftEpoch: 9 });
  const owner = { sessionId: null, draftEpoch: 4, operationId: "voiceop-prefill", restored: false };
  assert.equal(sandbox.restoreTaskDraft("dictated text", owner), true);
  assert.deepEqual(calls.prefill, [{ text: "dictated text", append: true }]);
  assert.deepEqual(calls.complete, [{ operationId: "voiceop-prefill", sessionId: null, accepted: false }]);
  // The text is visibly back in the live draft: the operation must follow it
  // to the new epoch, or a manual retry is orphaned in the dead epoch.
  assert.deepEqual(calls.rebind, [{ operationId: "voiceop-prefill", toEpoch: 9 }]);
});

test("restore: a bound session steers into its own session and settles there", () => {
  const { sandbox, calls } = restoreHarness({ activeSessionId: "session-b", draftEpoch: 5 });
  sandbox.voice = () => ({
    rebindVoiceDraftAfterRollback: () => false,
    completeVoiceSubmission: (operationId, sessionId, accepted) => calls.complete.push({ operationId, sessionId, accepted }),
    voiceOperationSessionId: () => "voice-bound-session",
  });
  const owner = { sessionId: null, draftEpoch: 4, operationId: "voiceop-bound", restored: false };
  assert.equal(sandbox.restoreTaskDraft("dictated text", owner), true);
  assert.deepEqual(calls.steer, [{ sid: "voice-bound-session", text: "dictated text" }]);
  assert.deepEqual(calls.complete, [{ operationId: "voiceop-bound", sessionId: "voice-bound-session", accepted: false }]);
});

// ── Scoped task-draft restore, web lane (real web bridge code): the restore
// branches settle the parked submission with the same polarity as desktop ──
function webRestoreHarness(stateOverrides, boundSessionId = null) {
  const state = { activeSessionId: null, draftEpoch: 4, composerDraft: "", ...stateOverrides };
  const sandbox = { console, Math, Date, Promise, Object, Array, JSON, String, Number, Boolean };
  vm.createContext(sandbox);
  sandbox.state = state;
  const calls = { complete: [], restore: [], prefill: [], rebind: [] };
  sandbox.completeVoiceSubmission = (operationId, sessionId, accepted) => calls.complete.push({ operationId, sessionId, accepted });
  sandbox.rebindVoiceOperationToDraft = (operationId, toEpoch) => calls.rebind.push({ operationId, toEpoch });
  sandbox.voiceOperationSessionId = () => boundSessionId;
  sandbox.restoreComposerText = (sid, text) => calls.restore.push({ sid, text });
  sandbox.prefillComposer = (text, append) => calls.prefill.push({ text, append });
  const start = sources.web.indexOf("  const pendingTaskDraftRecovery = { buffer: null };");
  const end = sources.web.indexOf("  // Undo one queued message", start);
  assert.ok(start >= 0 && end > start, "web bridge must keep the scoped task-draft restore block");
  vm.runInContext(`${sources.web.slice(start, end)}
    this.restoreTaskDraft = restoreTaskDraft;
    this.readComposerDraftWithRecovery = readComposerDraftWithRecovery;`, sandbox);
  return { sandbox, state, calls };
}

test("web restore: a bound session settles and restores into its own session", () => {
  const { sandbox, calls } = webRestoreHarness({ activeSessionId: "web-session-b" }, "web-voice-bound-session");
  const owner = { sessionId: null, createdSessionId: null, draftEpoch: 4, operationId: "voiceop-webrestore", restored: false };
  assert.equal(sandbox.restoreTaskDraft("dictated text", owner), true);
  assert.deepEqual(
    calls.complete,
    [{ operationId: "voiceop-webrestore", sessionId: "web-voice-bound-session", accepted: false }],
    "the web restore settles the parked submission via the operation's session binding",
  );
  assert.deepEqual(calls.restore, [{ sid: "web-voice-bound-session", text: "dictated text" }]);
});

test("web restore: a departed draft retains once, settles unparked, and consumes on return", () => {
  const { sandbox, state, calls } = webRestoreHarness({ activeSessionId: "web-unrelated" });
  const owner = { sessionId: null, createdSessionId: null, draftEpoch: 4, operationId: "voiceop-webgone", restored: false };
  assert.equal(sandbox.restoreTaskDraft("gone text", owner), true);
  assert.deepEqual(
    calls.complete,
    [{ operationId: "voiceop-webgone", sessionId: null, accepted: false }],
    "the departed-draft branch still settles — a rejected send must never stay parked",
  );
  assert.deepEqual(calls.prefill, [], "nothing lands in the unrelated active session");
  state.activeSessionId = null;
  assert.equal(sandbox.readComposerDraftWithRecovery(), "gone text", "the retained text is consumed once on the draft return");
  assert.equal(state.composerDraft, "gone text");
  // The voice association follows the restored text into the live epoch.
  assert.deepEqual(calls.rebind, [{ operationId: "voiceop-webgone", toEpoch: 4 }]);
  state.composerDraft = "";
  assert.equal(sandbox.readComposerDraftWithRecovery(), "", "the recovery slot is single-shot");
});

// ── Sent operations really end: the bridges settle acceptance at the point of truth ──
test("sends: dispatched sends settle their voice operation as accepted in both lanes", () => {
  const desktopSettles = chatSource.match(/settleAcceptedVoiceSubmission\(meta, sid\);/g) || [];
  // Exact count, not a lower bound: dropping one dispatched exit must go red,
  // and adding an exit must force this pin to be revisited.
  assert.equal(desktopSettles.length, 4, "every dispatched exit of the desktop sendMessage must settle its voice operation");
  const webSource = sources.web;
  const webSettles = webSource.match(/settleAcceptedVoiceSubmission\(meta, sid\);/g) || [];
  assert.equal(webSettles.length, 3, "every dispatched exit of the web sendMessage must settle its voice operation");
  assert.match(
    webSource,
    /acceptFirstTurnSubmission\(submission, metadata\);[\s\S]*?completeVoiceSubmission\(submission\.voiceOperationId, metadata\.id, true\);/,
    "web first-turn admission truth — not the optimistic resolve — ends the operation",
  );
  const codexSource = read("src/features/codex/CodexAcpView.jsx");
  assert.match(
    codexSource,
    /await sendBody\(\{ targetId, operation \}\);[\s\S]*?completeVoiceSubmission\(voiceOperationId, targetId, true\);/,
    "an accepted ACP send ends its voice operation",
  );
  assert.match(
    codexSource,
    /} catch \(err\) \{[\s\S]*?completeVoiceSubmission\(voiceOperationId, targetId \|\| null, false\);/,
    "a failed ACP send un-parks its voice operation and keeps the retryable association",
  );
  assert.match(
    codexSource,
    /} else if \(voiceOperationId && bridge\.voice && typeof bridge\.voice\.beginVoiceSubmission === 'function'\) \{[\s\S]*?bridge\.voice\.beginVoiceSubmission\(voiceOperationId, targetId\);/,
    "an existing-session ACP send parks its voice operation before dispatch too",
  );
  assert.match(
    codexSource,
    /if \(!targetId\) \{[\s\S]{0,500}?bridge\.voice\.beginVoiceSubmission\(voiceOperationId\);[\s\S]{0,500}?const created = await createSession\(\{/,
    "a first-turn ACP send parks before createSession so a cancel during creation waits for admission",
  );
  // The ChatView funnel is the only path that carries the operation id into
  // bridge.sendMessage: dispatchChatMessage must merge voiceMeta into meta
  // before the send, or every dispatched-exit settlement above is dead code
  // (a source-regex count of the settle calls cannot see that — this pin can).
  const chatViewSource = read("src/features/chat/ChatView.jsx");
  const mergeAnchor = chatViewSource.indexOf("meta = Object.assign({}, meta, voiceMeta);");
  const sendAnchor = chatViewSource.indexOf("bridge.chat.sendMessage(visibleOutgoing, meta, voiceOwner)");
  assert.ok(mergeAnchor >= 0, "dispatchChatMessage must merge voiceMeta into meta");
  assert.ok(sendAnchor > mergeAnchor, "the voiceMeta merge must happen before the sendMessage call");
  // Above the merge: sendChatMessage must resolve the parked operation id,
  // begin the submission, and pass the id into dispatchChatMessage — a
  // regression there nulls voiceMeta, the merge silently becomes a no-op,
  // and every dispatched-exit settlement above dies while this source still
  // matches (the executed settle test drives bridge.sendMessage directly, so
  // it cannot see this funnel hop either).
  const funnelStart = chatViewSource.indexOf("const sendChatMessage = useCallback(async (text, voiceContext) => {");
  const resolveAnchor = chatViewSource.indexOf("bridge.voice.getVoiceOperationId(activeSessionId || null, 'chat')", funnelStart);
  const beginAnchor = chatViewSource.indexOf("bridge.voice.beginVoiceSubmission(operationId)", funnelStart);
  const dispatchAnchor = chatViewSource.indexOf("dispatchChatMessage(text, { voiceOperationId: operationId }, voiceContext?.draftOwner)", funnelStart);
  assert.ok(funnelStart >= 0, "the ChatView send funnel must exist");
  assert.ok(resolveAnchor > funnelStart, "sendChatMessage must resolve the parked chat-lane operation id");
  assert.ok(beginAnchor > resolveAnchor, "sendChatMessage must begin the submission before dispatching");
  assert.ok(dispatchAnchor > beginAnchor, "sendChatMessage must pass the operation id into dispatchChatMessage");
  assert.match(
    chatViewSource,
    /!== voiceOwner\.draftEpoch\)\)\) \{[\s\S]{0,300}?pendingModeScopeMigrationRef\.current = null;[\s\S]{0,100}?return false;/,
    "a refused dispatch must clear the stale mode-scope migration payload like the catch path",
  );
  // Accepted operations must not keep pinning the raw recording PCM for the
  // rest of the app session: both lanes drop the chunks as soon as the merged
  // buffer exists (and on the cancelled teardown).
  for (const [lane, source] of [["desktop", sources.desktop], ["web", sources.web]]) {
    assert.match(
      source,
      /const raw = mergeFloatChunks\(session\.chunks\);[\s\S]{0,400}?session\.chunks = null;/,
      `${lane} lane must release the PCM chunks right after the merge`,
    );
    assert.match(
      source,
      /trackVoiceTerminal\("voice_cancelled", session(?:, \{[^}]*\})?\);[\s\S]{0,300}?cleanupVoiceInputSession\(session\);[\s\S]{0,300}?session\.chunks = null;/,
      `${lane} cancelled teardown must detach the audio callback before releasing the PCM chunks`,
    );
  }
  assert.match(
    sources.web,
    /const sid = state\.activeSessionId;[\s\S]{0,400}?beginVoiceMetaSubmission\(meta, sid\);/,
    "the web sendMessage must keep the in-bridge submission gate so future dispatch paths park too",
  );
  // A re-trigger during an ASR model install must not end the previous
  // adoptable operation: its text is still sitting in the composer. Both
  // needles are asserted present — a bare indexOf comparison goes green when
  // a line is deleted (both sides become -1).
  const installGuard = sources.desktop.indexOf("if (state.voiceAsrSetup.installing) {");
  const previousAbandon = sources.desktop.indexOf("if (previousOperationId) abandonVoiceResult(previousOperationId);");
  assert.ok(
    installGuard >= 0 && previousAbandon > installGuard,
    "the install guard must exist and precede the previous-operation abandon",
  );
  for (const [lane, source] of [["desktop", sources.desktop], ["web", sources.web]]) {
    assert.match(
      source,
      /normalizeVoiceError\(err, "transcribing"\);\s*if \(!session\.voiceResultReady\) trackVoiceTerminal\("voice_recognition_failed", session\);/,
      `${lane} transcription failures must terminalize the operation`,
    );
  }
  assert.match(
    read("src/features/chat/ChatView.jsx"),
    /if \(voiceOwner && \(\(activeSessionIdRef\.current \|\| null\) !== voiceOwner\.sessionId/,
    "the ChatView pre-guard must reject sends whose ownership moved on",
  );
  assert.match(
    read("src/features/voice-composer/useComposerVoiceInput.js"),
    /const dismissVoice = useCallback\(\(\) => \{[\s\S]*?dismissVoiceInput\(\);/,
    "the hook exposes the dismiss-without-ending primitive",
  );
});
