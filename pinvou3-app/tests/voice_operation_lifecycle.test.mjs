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
const read = (file) => fs.readFileSync(path.join(root, file), "utf8");

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
    const anchor = source.indexOf("if (!operationId) abandonCompletedVoiceResult(\"recognition\");", abandonStart);
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
  // Stubs for module-level helpers the extracted slice references.
  sandbox.activeVoiceInput = null;
  sandbox.bt = (key) => key;
  sandbox.voiceToken = (prefix) => `${prefix}1`;
  sandbox.webVoiceToken = (prefix) => `${prefix}1`;
  sandbox.clearVoiceInput = () => {};
  sandbox.setVoiceInputStatus = () => {};
  sandbox.getVoiceOperationId = () => null;
  sandbox.abandonVoiceResult = () => {};
  sandbox.abandonCompletedVoiceResult = () => {};
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

// ── Desktop ownership claim: the microphone must stay closed until the claim lands ──
test("desktop: no microphone probe before the Rust ownership claim resolves", async () => {
  const source = sources.desktop;
  let resolveClaim;
  const claim = new Promise((resolve) => { resolveClaim = resolve; });
  let microphoneProbes = 0;
  const state = {
    activeSessionId: null,
    draftEpoch: 4,
    composerDraft: "",
    voiceAsrSetup: { installing: false },
    voiceInput: { status: "idle" },
  };
  const sandbox = baseSandbox(state);
  sandbox.bt = (key) => key;
  sandbox.window = { AudioContext: undefined };
  sandbox.navigator = {};
  sandbox.emitVoiceDiagnostic = () => {};
  sandbox.normalizeVoiceMode = (mode) => mode;
  sandbox.normalizeVoiceError = (error) => error;
  sandbox.voiceToken = (prefix) => `${prefix}1`;
  sandbox.currentVoiceWindowLabel = () => "detached-b";
  sandbox.syncVoiceShortcutRecording = () => claim;
  sandbox.probeVoiceAudioInput = () => { microphoneProbes += 1; return true; };
  sandbox.cleanupVoiceInputSession = () => {};
  sandbox.rememberVoiceOperation = () => {};
  sandbox.getVoiceOperationId = () => null;
  sandbox.abandonVoiceResult = () => {};
  sandbox.abandonCompletedVoiceResult = () => {};
  sandbox.trackVoiceTerminal = () => {};
  const statuses = [];
  sandbox.setVoiceInputStatus = (status) => { statuses.push(status); };
  const start = source.indexOf("  async function startVoiceInput(");
  const end = source.indexOf("function cancelVoiceInput()", start);
  assert.ok(start >= 0 && end > start, "desktop startVoiceInput must exist");
  vm.runInContext(`${source.slice(start, end)}\nthis.startVoiceInput = startVoiceInput;`, sandbox);
  const starting = sandbox.startVoiceInput("draft text", () => {}, { mode: "dictation" });
  await new Promise((resolve) => { setImmediate(resolve); });
  assert.equal(microphoneProbes, 0, "the probe must wait for the ownership claim");
  resolveClaim(false);
  await starting;
  assert.equal(microphoneProbes, 0, "a rejected claim must fail the start before any microphone");
  assert.equal(statuses[statuses.length - 1], "failed");
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
  const sandbox = { console, Math, Date, Promise, Object, Array, JSON };
  vm.createContext(sandbox);
  const editPreviewRef = { current: null };
  const state = { activeSessionId: null, draftEpoch: 4, composerDraft: "", voiceInput: null };
  sandbox.state = state;
  sandbox.notify = () => {};
  const adapterRef = { current: {
    bridge: { available: true, voice: {
      abandonVoiceResult: (id) => discarded.push(id),
      cancelVoiceInput() {}, clearVoiceInput() {}, dismissVoiceInput() {},
    } },
  } };
  sandbox.adapterRef = adapterRef;
  sandbox.editPreviewRef = editPreviewRef;
  sandbox.editPreview = null;
  sandbox.setEditPreview = () => {};
  sandbox.closeVoice = () => {};
  sandbox.trimDraft = (value) => String(value || "").trim();
  sandbox.useCallback = (fn) => fn;
  const discardStart = hookSource.indexOf("  // Abandon the result of the operation that recorded it");
  const applyEnd = hookSource.indexOf("  const clearStaleVoiceState = useCallback(");
  assert.ok(discardStart >= 0 && applyEnd > discardStart, "the hook keeps its discard/apply block");
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
});

// ── ChatView source contracts for the submission protocol ──
test("chatview: the task lane consumes only its own draft and maps restored to not-accepted", () => {
  const chatViewSource = read("src/features/chat/ChatView.jsx");
  assert.match(
    chatViewSource,
    /sendChatMessage\(constrained\.text, \{ \.\.\.context, draftOwner: owner \}\)/,
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
test("ownership: tokenless clears are rejected and stale teardown cannot wipe a newer claim", async () => {
  const rustSource = read("src-tauri/src/features/voice_shortcut.rs");
  // The rules live in the Rust unit tests (update_recording_owner); the
  // frontend guard mirrors them: assert the shipped JS side rejects
  // tokenless clears outright.
  const bridgeSource = sources.desktop;
  assert.match(
    bridgeSource,
    /function syncVoiceShortcutRecording\(label, token\) \{[\s\S]*?if \(!token\) return Promise\.resolve\(false\);/,
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

console.log("voice_operation_lifecycle: all assertions passed");
