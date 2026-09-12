#!/usr/bin/env node
/**
 * Behavioral tests for the computer_use bridge feature state machine
 * (src/platform/tauri/bridge/computer_use.js), run in a VM with fake
 * invoke/listen. Until the review fixes this module only had a protocol-hash
 * lock on its invoke/listen text and zero behavioral coverage, even though it
 * owns the safety-critical consent projection (staleness guard, per-session
 * pending merge, optimistic rollback, disabled-gating).
 *
 * The file also pins the computer_use protocol surface (invoke command spans
 * and listen event names, mirroring tests/bridge_domain_protocol.test.mjs's
 * extraction) so the desktop contract cannot drift silently.
 *
 * Run: node --test pinvou3-app/tests/computer_use_bridge.test.mjs
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import { computerUseConsentView } from '../src/features/computer-use/computer-use-logic.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const bridgeSource = fs.readFileSync(
  path.join(__dirname, '..', 'src', 'platform', 'tauri', 'bridge', 'computer_use.js'),
  'utf8',
);

// ── Protocol anchor (signature-set comparison) ─────────────────────────
// The computer-use contract is the invoke/listen CALL SET: desktop commands
// and event names, in order. (tests/bridge_domain_protocol.test.mjs pins a
// sha256 over the same spans, but its listen span digests the raw listener
// body too, so body-internal review fixes shift that digest while the
// contract itself stays identical — this file pins the contract surface.)
function extractCalls(source, callee) {
  const calls = [];
  const needle = `${callee}(`;
  let cursor = 0;
  while ((cursor = source.indexOf(needle, cursor)) !== -1) {
    const previous = source[cursor - 1] || '';
    if (/[A-Za-z0-9_$]/.test(previous)) {
      cursor += needle.length;
      continue;
    }
    let index = cursor + needle.length;
    let depth = 1;
    let quote = null;
    let escaped = false;
    let lineComment = false;
    let blockComment = false;
    for (; index < source.length && depth > 0; index += 1) {
      const char = source[index];
      const next = source[index + 1];
      if (lineComment) {
        if (char === '\n') lineComment = false;
        continue;
      }
      if (blockComment) {
        if (char === '*' && next === '/') { blockComment = false; index += 1; }
        continue;
      }
      if (quote) {
        if (escaped) escaped = false;
        else if (char === '\\') escaped = true;
        else if (char === quote) quote = null;
        continue;
      }
      if (char === '/' && next === '/') { lineComment = true; index += 1; continue; }
      if (char === '/' && next === '*') { blockComment = true; index += 1; continue; }
      if (char === '"' || char === "'" || char === '`') { quote = char; continue; }
      if (char === '(') depth += 1;
      else if (char === ')') depth -= 1;
    }
    assert.equal(depth, 0, `unclosed ${callee} call near offset ${cursor}`);
    calls.push(source.slice(cursor, index).replace(/\s+/g, ' ').trim());
    cursor = index;
  }
  return calls;
}

{
  assert.deepEqual(
    extractCalls(bridgeSource, 'invoke'),
    [
      'invoke("computer_use_get_status", { sessionId })',
      'invoke("computer_use_get_status", { sessionId: sid })',
      'invoke("computer_use_grant", { sessionId: sid })',
      'invoke("computer_use_revoke", { sessionId: sid })',
      'invoke("computer_use_stop")',
      'invoke("computer_use_confirm", { confirmId })',
      'invoke("computer_use_deny", { confirmId })',
      'invoke("computer_use_set_enabled", { enabled: target })',
      'invoke("computer_use_request_permissions")',
      'invoke("computer_use_request_permissions")',
    ],
    'computer_use command surface must stay unchanged',
  );
  const listenEvents = extractCalls(bridgeSource, 'listen')
    .map((span) => (span.match(/listen\((["'`])([^"'`]+)\1/) || [])[2]);
  assert.deepEqual(
    listenEvents,
    ['computer_use:grant_required', 'computer_use:confirm_required'],
    'computer_use event surface must stay unchanged',
  );
}

function createHarness({ status = {}, failInvoke = null, failMessage = '', initialState = {}, deferredStatus = null } = {}) {
  const invoked = [];
  const listeners = {};
  const published = [];
  const state = {
    activeSessionId: 's1',
    computerUse: { enabled: false, granted: false, stopped: false, platformSupported: true, ...initialState },
  };
  const harness = {
    invoked, state, listeners,
    published() { return published.map((entry) => entry.slice); },
  };
  const context = vm.createContext({
    window: {},
    console,
  });
  vm.runInContext(bridgeSource, context, { filename: 'bridge/computer_use.js' });
  const factory = context.window.__PINVOU_TAURI_BRIDGE_FEATURES__.computer_use;
  assert.equal(typeof factory, 'function', 'computer_use feature must register itself');
  const feature = factory({
    state,
    notify() { published.push({ slice: { ...state.computerUse } }); },
    async invoke(command, args) {
      if (failInvoke && failInvoke(command, args)) throw new Error(failMessage || `invoke failed: ${command}`);
      invoked.push([command, args]);
      if (command === 'computer_use_get_status') return deferredStatus || status;
      return null;
    },
    listen(event, handler) { listeners[event] = handler; },
  });
  harness.feature = feature;
  return harness;
}

function emit(harness, event, payload) {
  const handler = harness.listeners[event];
  assert.ok(handler, `listener for ${event} must be registered`);
  handler({ payload });
}

// ── 1. Stale get_status must not win ────────────────────────────────
{
  const harness = createHarness({ initialState: { enabled: true } });
  // No await between the two refreshes: both capture seq 1 and 2, the first
  // (slow) response must be dropped.
  const first = harness.feature.refreshStatus('s1');
  const second = harness.feature.refreshStatus('s1');
  await first;
  await second;
  const slices = harness.published();
  assert.equal(slices.length, 1, `stale get_status must not publish: ${JSON.stringify(slices)}`);
}

// ── 2. Background-session grant request resurfaces on refresh ──────
{
  const harness = createHarness();
  harness.state.activeSessionId = 's1';
  emit(harness, 'computer_use:grant_required', { session_id: 's2' });
  assert.equal(
    harness.published().length, 0,
    'a background session request must not publish into the active slice',
  );
  harness.state.activeSessionId = 's2';
  await harness.feature.refreshStatus('s2');
  const last = harness.published().at(-1);
  assert.ok(last && last.grantRequest && last.grantRequest.sessionId === 's2',
    `background request must resurface on refresh: ${JSON.stringify(last)}`);
}

// ── 3. setEnabled rolls back the optimistic toggle on failure ───────
{
  const harness = createHarness({ failInvoke: (command) => command === 'computer_use_set_enabled' });
  await assert.rejects(harness.feature.setEnabled(true));
  assert.equal(harness.state.computerUse.enabled, false, 'failed setEnabled must roll back');
  const last = harness.published().at(-1);
  assert.equal(last.enabled, false, 'rollback must be published');
}

// ── 4. Events stay inert while disabled, resurface after refresh ────
{
  const harness = createHarness();
  harness.state.computerUse.enabled = false;
  emit(harness, 'computer_use:grant_required', { session_id: 's1' });
  assert.equal(harness.published().length, 0, 'disabled feature must not publish dialogs');
  harness.state.computerUse.enabled = true;
  await harness.feature.refreshStatus('s1');
  const last = harness.published().at(-1);
  assert.ok(last && last.grantRequest, 'after enable, refresh must resurface the pending grant');
}

// ── 5. Explicit deny: backend command + close, new requests re-prompt ──
{
  const harness = createHarness({ initialState: { enabled: true } });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'left click', element: 'Buy now',
  });
  assert.ok(harness.published().at(-1).confirmRequest, 'confirm dialog must be shown');
  await harness.feature.deny('cu-1');
  const denyCall = harness.invoked.find(([command]) => command === 'computer_use_deny');
  assert.equal(denyCall && denyCall[1] && denyCall[1].confirmId, 'cu-1',
    'deny must reach the backend instead of only closing the dialog locally');
  assert.equal(harness.published().at(-1).confirmRequest, null, 'dialog must close after deny');
  // A genuinely new backend request shows a new dialog immediately: a deny
  // consumes only the request it answered — no cooldown, no denial memory.
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-2', action: 'left click', element: 'Buy now',
  });
  const republished = harness.published().at(-1).confirmRequest;
  assert.ok(republished, 'a new confirm_required after a deny must re-open the dialog immediately');
  assert.equal(republished.confirmId, 'cu-2', 'the new dialog must describe the new request');
}

// ── 6. A grant deny only closes the current dialog; re-prompts re-ask ──
{
  const harness = createHarness({ initialState: { enabled: true } });
  emit(harness, 'computer_use:grant_required', { session_id: 's1' });
  assert.ok(harness.published().at(-1).grantRequest);
  await harness.feature.revoke('s1');
  assert.equal(harness.published().at(-1).grantRequest, null, 'deny must close the grant dialog');
  emit(harness, 'computer_use:grant_required', { session_id: 's1' });
  assert.ok(
    harness.published().at(-1).grantRequest,
    'a new grant_required after a deny must re-open the dialog immediately',
  );
  await harness.feature.grant('s1');
  assert.ok(harness.published().at(-1).granted, 'grant must publish granted=true');
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-9', action: 'a', element: 'e',
  });
  assert.ok(harness.published().at(-1).confirmRequest, 'confirm dialog works after granting');
}

// ── 7. platform_supported flows into the published slice ────────────
{
  const harness = createHarness({ initialState: { enabled: true }, status: { enabled: true, granted: false, stopped: false, platform_supported: false } });
  await harness.feature.refreshStatus('s1');
  const last = harness.published().at(-1);
  assert.equal(last.platformSupported, false, 'settings needs platform_supported to disable the toggle');
}

// ── 8. stop() publishes the escape hatch immediately + hits the backend ─
{
  const harness = createHarness({ initialState: { enabled: true, granted: true } });
  const stopping = harness.feature.stop();
  assert.equal(harness.state.computerUse.stopped, true, 'stop must collapse the banner before the IPC resolves');
  assert.equal(harness.published().at(-1).granted, false, 'stop must drop granted immediately');
  await stopping;
  assert.ok(harness.invoked.some(([command]) => command === 'computer_use_stop'), 'stop must reach the backend');
}

// ── 9. stop() failure re-reads the authoritative status ─────────────
{
  const harness = createHarness({
    initialState: { enabled: true, granted: true },
    failInvoke: (command) => command === 'computer_use_stop',
    status: { enabled: true, granted: true, stopped: false, platform_supported: true },
  });
  await assert.rejects(harness.feature.stop());
  const last = harness.published().at(-1);
  assert.equal(last.stopped, false, 'a failed stop must re-read the authoritative status');
  assert.equal(last.granted, true, 'the re-read must restore the pre-stop slice');
}

// ── 10. confirm() approval path reaches the backend and closes the dialog ─
{
  const harness = createHarness({ initialState: { enabled: true } });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'left click', element: 'Send',
  });
  await harness.feature.confirm('cu-1');
  const call = harness.invoked.find(([command]) => command === 'computer_use_confirm');
  assert.equal(call && call[1] && call[1].confirmId, 'cu-1', 'approval must carry the confirm id');
  assert.equal(harness.published().at(-1).confirmRequest, null, 'approval closes the dialog');
}

// ── 11. stop → re-enable → grant_required must re-open the dialog ───
// Review finding: the backend only flips `enabled`, so without a status
// refresh the sticky `stopped` kept every later dialog collapsed.
{
  const harness = createHarness({
    initialState: { enabled: true, granted: true },
    status: { enabled: true, granted: false, stopped: false, platform_supported: true },
  });
  await harness.feature.stop();
  assert.equal(harness.state.computerUse.stopped, true);
  // The user re-enables the feature in settings…
  await harness.feature.setEnabled(true);
  assert.equal(harness.state.computerUse.stopped, false, 'setEnabled(true) must refresh the sticky stop away');
  // …and a fresh grant request must surface the dialog again.
  emit(harness, 'computer_use:grant_required', { session_id: 's1' });
  // JSON round-trip: the slice's nested request objects are constructed in
  // the VM realm, so deepEqual's prototype check would trip on them.
  assert.deepEqual(
    JSON.parse(JSON.stringify(computerUseConsentView(harness.state.computerUse))),
    { enabled: true, stopped: false, showBanner: false, grantRequest: { sessionId: 's1' }, confirmRequest: null },
    'after stop → re-enable, a fresh grant request must surface the dialog again',
  );
}

// ── 12. grant_required clears a latched stop even without a refresh ─
{
  const harness = createHarness({ initialState: { enabled: true, granted: true } });
  await harness.feature.stop();
  // Second line of defense: the backend never emits the event while stopped,
  // so the handler itself clears frontend `stopped` residue.
  emit(harness, 'computer_use:grant_required', { session_id: 's1' });
  const last = harness.published().at(-1);
  assert.equal(last.stopped, false, 'grant_required must clear the latched stop');
  assert.ok(last.grantRequest, 'grant_required must surface the dialog');
}

// ── 13. deny consumes the pending: no resurface after a session switch ─
{
  const harness = createHarness({ initialState: { enabled: true } });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'left click', element: 'Buy now',
  });
  await harness.feature.deny('cu-1');
  // The deny cleared the pending entry, not just the visible dialog:
  // switching away and back must not resurrect the denied request.
  harness.state.activeSessionId = 's2';
  await harness.feature.refreshStatus('s2');
  harness.state.activeSessionId = 's1';
  await harness.feature.refreshStatus('s1');
  assert.equal(
    harness.published().at(-1).confirmRequest, null,
    'a denied request must not resurface after a session switch',
  );
}

// ── 14. confirm on an expired request closes the dead-end modal (review finding) ─
{
  const harness = createHarness({
    initialState: { enabled: true },
    failInvoke: (command) => command === 'computer_use_confirm' || command === 'computer_use_deny',
    failMessage: 'computer_use_confirm: confirm request unknown or expired',
  });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'left click', element: 'Buy now',
  });
  await assert.rejects(harness.feature.confirm('cu-1'), /unknown or expired/i);
  assert.equal(harness.published().at(-1).confirmRequest, null, 'the expired modal must close instead of dead-ending');
  // Pending cleared: switching away and back must not resurface it.
  harness.state.activeSessionId = 's2';
  await harness.feature.refreshStatus('s2');
  harness.state.activeSessionId = 's1';
  await harness.feature.refreshStatus('s1');
  assert.equal(harness.published().at(-1).confirmRequest, null, 'expired requests must not resurface from the pending map');
}

// ── 15. deny on an expired request closes without recording a denial ──
{
  const harness = createHarness({
    initialState: { enabled: true },
    failInvoke: (command) => command === 'computer_use_deny',
    failMessage: 'computer_use_deny: confirm request unknown or expired',
  });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'left click', element: 'Buy now',
  });
  await assert.rejects(harness.feature.deny('cu-1'), /unknown or expired/i);
  assert.equal(harness.published().at(-1).confirmRequest, null, 'the expired modal must close after deny too');
  // An expiry is not a user decision: an immediate retry re-opens the dialog
  // like any genuinely new request.
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-2', action: 'left click', element: 'Buy now',
  });
  assert.ok(harness.published().at(-1).confirmRequest, 'a new request after an expired deny shows a fresh dialog');
}

// ── 16. session switch clears the stale dialog synchronously, keeps pending ──
// Review finding: during the async refresh window the previous session's
// grant dialog stayed clickable.
{
  let resolveStatus;
  const gate = new Promise((resolve) => { resolveStatus = resolve; });
  const harness = createHarness({ initialState: { enabled: true }, deferredStatus: gate });
  emit(harness, 'computer_use:grant_required', { session_id: 's1' });
  assert.ok(harness.published().at(-1).grantRequest, 's1 dialog must be up before the switch');
  harness.state.activeSessionId = 's2';
  // No await: the clear must happen synchronously, before the IPC resolves.
  const refreshing = harness.feature.refreshStatus('s2');
  assert.equal(
    harness.published().at(-1).grantRequest, null,
    'the previous session grant dialog must be dropped synchronously on switch',
  );
  resolveStatus({ enabled: true, granted: false, stopped: false, platform_supported: true });
  await refreshing;
  // Switching back resurfaces the request from the per-session pending map.
  harness.state.activeSessionId = 's1';
  await harness.feature.refreshStatus('s1');
  assert.ok(
    harness.published().at(-1).grantRequest,
    'the pending request must survive the switch and resurface for s1',
  );
}

// ── 17. stop() clears the per-session pending map (no phantom dialogs) ──
// Review finding: the backend's stop_all wipes every grant/confirm/token,
// so stale pending entries made a later re-enable + refresh republish
// dialogs for requests that no longer exist.
{
  const harness = createHarness({
    initialState: { enabled: true, granted: true },
    status: { enabled: true, granted: false, stopped: false, platform_supported: true },
  });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'left click', element: 'Buy now',
  });
  emit(harness, 'computer_use:grant_required', { session_id: 's2' });
  await harness.feature.stop();
  // Re-enable (the phantom path: refreshStatus merges the pending map).
  await harness.feature.setEnabled(true);
  assert.equal(
    harness.published().at(-1).confirmRequest, null,
    'a stopped session confirm must not resurface after stop + re-enable',
  );
  harness.state.activeSessionId = 's2';
  await harness.feature.refreshStatus('s2');
  assert.equal(
    harness.published().at(-1).grantRequest, null,
    'a background grant pending must not resurface after stop',
  );
}

// ── 18. setEnabled(false) clears the per-session pending map too ─────
{
  const harness = createHarness({
    initialState: { enabled: true },
    status: { enabled: true, granted: false, stopped: false, platform_supported: true },
  });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'left click', element: 'Buy now',
  });
  emit(harness, 'computer_use:grant_required', { session_id: 's2' });
  await harness.feature.setEnabled(false);
  assert.ok(
    harness.invoked.some(([command, args]) => command === 'computer_use_set_enabled' && args && args.enabled === false),
    'disable must reach the backend',
  );
  // Re-enable: the disable already wiped the backend state, so nothing may
  // resurface from the pending map.
  await harness.feature.setEnabled(true);
  assert.equal(
    harness.published().at(-1).confirmRequest, null,
    'a confirm pending must not resurface after disable + re-enable',
  );
  harness.state.activeSessionId = 's2';
  await harness.feature.refreshStatus('s2');
  assert.equal(
    harness.published().at(-1).grantRequest, null,
    'a grant pending must not resurface after disable',
  );
}

// ── 19. session switch clears the banner synchronously (review finding) ─
// The requests were already dropped pre-await; a stale `granted` kept the
// previous session's control banner up during the IPC round-trip.
{
  let resolveStatus;
  const gate = new Promise((resolve) => { resolveStatus = resolve; });
  const harness = createHarness({
    initialState: { enabled: true, granted: true, sessionId: 's1' },
    deferredStatus: gate,
  });
  harness.state.activeSessionId = 's2';
  // No await: the banner flag must clear synchronously, before the IPC lands.
  const refreshing = harness.feature.refreshStatus('s2');
  assert.equal(
    harness.published().at(-1) && harness.published().at(-1).granted, false,
    'the banner flag must clear synchronously on a session switch',
  );
  resolveStatus({ enabled: true, granted: false, stopped: false, platform_supported: true });
  await refreshing;
}

// ── 20. grant_required must not wipe a live per-action confirmation ──
// Review finding: a grant that idle-expired mid-run re-arms the grant gate
// while the backend confirm is still pending; wiping pending.confirm left
// no dialog after Allow.
{
  const harness = createHarness({ initialState: { enabled: true } });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'type', element: 'Reply box',
  });
  assert.ok(harness.published().at(-1).confirmRequest, 'confirm dialog must be up before the grant expires');
  emit(harness, 'computer_use:grant_required', { session_id: 's1' });
  assert.ok(harness.published().at(-1).grantRequest, 'the re-armed grant dialog must show');
  await harness.feature.grant('s1');
  const last = harness.published().at(-1);
  assert.ok(
    last.confirmRequest && last.confirmRequest.confirmId === 'cu-1',
    `after Allow, the confirmation must still be available: ${JSON.stringify(last)}`,
  );
  await harness.feature.refreshStatus('s1');
  assert.ok(
    harness.published().at(-1).confirmRequest,
    'the confirmation must also survive a status refresh',
  );
}

// ── 21. confirm_required passes typePreviewFull through to the dialog ─
// Backend contract: the optional full typed-text preview travels with the
// confirm payload for every non-password Type action up to 4096 chars —
// including very short texts, which the UI must show inline; old payloads
// must keep their exact shape.
{
  const harness = createHarness({ initialState: { enabled: true } });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'type', element: 'Reply box',
    typePreviewFull: 'line one\nline two',
  });
  const request = harness.published().at(-1).confirmRequest;
  assert.equal(request && request.typePreviewFull, 'line one\nline two',
    'the full typed-text preview must pass through to the published request');
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-2', action: 'type', element: 'Reply box', type_preview_full: 'snake case',
  });
  assert.equal(harness.published().at(-1).confirmRequest.typePreviewFull, 'snake case',
    'the snake_case spelling is accepted too');
  // The new backend contract: short texts ride the payload too (previously
  // only texts beyond the 12-char preview did).
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-4', action: 'type', element: 'Reply box', typePreviewFull: 'hi',
  });
  assert.equal(harness.published().at(-1).confirmRequest.typePreviewFull, 'hi',
    'a short preview must pass through to the published request too');
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-3', action: 'type', element: 'Reply box',
  });
  const plain = harness.published().at(-1).confirmRequest;
  assert.equal('typePreviewFull' in plain, false,
    'a payload without the field must not grow a typePreviewFull key');
}

// ── 22. expired confirm() cleanup must not wipe a NEWER dialog ──────
// Review finding: the catch path cleared unconditionally, so a newer request
// that arrived mid-IPC lost its dialog. It must survive, while the matching
// request still clears (dialog + pending entry).
{
  const harness = createHarness({
    initialState: { enabled: true },
    failInvoke: (command) => command === 'computer_use_confirm',
    failMessage: 'computer_use_confirm: confirm request unknown or expired',
  });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'left click', element: 'Buy now',
  });
  // cu-2 replaces the slice while confirm('cu-1') is in flight.
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-2', action: 'left click', element: 'Checkout',
  });
  await assert.rejects(harness.feature.confirm('cu-1'), /unknown or expired/i);
  const survivor = harness.published().at(-1).confirmRequest;
  assert.ok(survivor && String(survivor.confirmId) === 'cu-2',
    `the expired cleanup must not wipe the newer cu-2 dialog: ${JSON.stringify(survivor)}`);
  // The newer request's pending entry survives too: switching away and back
  // must resurface cu-2, not nothing.
  harness.state.activeSessionId = 's2';
  await harness.feature.refreshStatus('s2');
  harness.state.activeSessionId = 's1';
  await harness.feature.refreshStatus('s1');
  const resurfaced = harness.published().at(-1).confirmRequest;
  assert.ok(resurfaced && String(resurfaced.confirmId) === 'cu-2',
    `the newer request must resurface from the pending map: ${JSON.stringify(resurfaced)}`);
}

// ── 23. expired confirm() cleanup still clears the MATCHING request ──
{
  const harness = createHarness({
    initialState: { enabled: true },
    failInvoke: (command) => command === 'computer_use_confirm',
    failMessage: 'computer_use_confirm: confirm request unknown or expired',
  });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'left click', element: 'Buy now',
  });
  await assert.rejects(harness.feature.confirm('cu-1'), /unknown or expired/i);
  assert.equal(harness.published().at(-1).confirmRequest, null,
    'the matching expired dialog must still close');
  harness.state.activeSessionId = 's2';
  await harness.feature.refreshStatus('s2');
  harness.state.activeSessionId = 's1';
  await harness.feature.refreshStatus('s1');
  assert.equal(harness.published().at(-1).confirmRequest, null,
    'the matching expired request must not resurface from the pending map');
}

// ── 24. expired deny() cleanup must not wipe a NEWER dialog ─────────
{
  const harness = createHarness({
    initialState: { enabled: true },
    failInvoke: (command) => command === 'computer_use_deny',
    failMessage: 'computer_use_deny: confirm request unknown or expired',
  });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'left click', element: 'Buy now',
  });
  // cu-2 replaces the slice while deny('cu-1') is in flight.
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-2', action: 'left click', element: 'Checkout',
  });
  await assert.rejects(harness.feature.deny('cu-1'), /unknown or expired/i);
  const survivor = harness.published().at(-1).confirmRequest;
  assert.ok(survivor && String(survivor.confirmId) === 'cu-2',
    `the expired deny cleanup must not wipe the newer cu-2 dialog: ${JSON.stringify(survivor)}`);
  harness.state.activeSessionId = 's2';
  await harness.feature.refreshStatus('s2');
  harness.state.activeSessionId = 's1';
  await harness.feature.refreshStatus('s1');
  const resurfaced = harness.published().at(-1).confirmRequest;
  assert.ok(resurfaced && String(resurfaced.confirmId) === 'cu-2',
    `the newer request must resurface from the pending map: ${JSON.stringify(resurfaced)}`);
  // And the matching request still clears when denied with an expiry.
  await assert.rejects(harness.feature.deny('cu-2'), /unknown or expired/i);
  assert.equal(harness.published().at(-1).confirmRequest, null,
    'the matching expired dialog must still close after deny');
}

// ── 25. revoke() collapses a same-session confirm dialog (review finding) ─
// Backend revoke wipes the session's grant AND its pending confirmations, so
// a pending confirm dialog must not linger as a dead modal after revoke.
{
  const harness = createHarness({ initialState: { enabled: true } });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'type', element: 'Reply box',
  });
  assert.ok(harness.published().at(-1).confirmRequest, 'confirm dialog must be up before revoke');
  await harness.feature.revoke('s1');
  const last = harness.published().at(-1);
  assert.equal(last.grantRequest, null, 'revoke must close the grant dialog');
  assert.equal(last.confirmRequest, null, 'revoke must collapse the same-session confirm dialog');
  // No stale pending entry: switching away and back must not resurface
  // either request.
  harness.state.activeSessionId = 's2';
  await harness.feature.refreshStatus('s2');
  harness.state.activeSessionId = 's1';
  await harness.feature.refreshStatus('s1');
  const resurfaced = harness.published().at(-1);
  assert.equal(resurfaced.confirmRequest, null,
    'a revoked session must not resurface the confirm from the pending map');
  assert.equal(resurfaced.grantRequest, null,
    'a revoked session must not resurface the grant from the pending map');
}

// ── 26. late cleanup clears the ORIGINAL session's map across a switch ──
// Review finding: the published-slice early-return skipped the pending-map
// cleanup when a different session's dialog was published mid-IPC, so
// switching back resurfaced a phantom dialog for the decided request.
{
  const harness = createHarness({ initialState: { enabled: true } });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'left click', element: 'Buy now',
  });
  assert.ok(harness.published().at(-1).confirmRequest, 's1 dialog must be up');
  // deny() attributes the decision to s1 before the IPC; the user switches
  // to s2 mid-flight and a fresh s2 dialog is published.
  const denying = harness.feature.deny('cu-1');
  harness.state.activeSessionId = 's2';
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's2', confirm_id: 'cu-2', action: 'left click', element: 'Checkout',
  });
  assert.equal(String(harness.published().at(-1).confirmRequest.confirmId), 'cu-2',
    'the s2 dialog must be published mid-IPC');
  await denying;
  assert.equal(String(harness.published().at(-1).confirmRequest.confirmId), 'cu-2',
    'the cross-session cleanup must not close session B\'s dialog');
  // Switching back to s1 must not resurrect the denied cu-1 request.
  await harness.feature.refreshStatus('s2');
  harness.state.activeSessionId = 's1';
  await harness.feature.refreshStatus('s1');
  assert.equal(harness.published().at(-1).confirmRequest, null,
    'session A pending entry must clear despite the mid-IPC session switch');
}

// ── 27. a newer same-session dialog survives the late cleanup ───────
// The success-path cleanup of an older id must keep both the replacement's
// dialog and its pending entry (same rule as the expired path in #22/#24).
{
  const harness = createHarness({ initialState: { enabled: true } });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'left click', element: 'Buy now',
  });
  const approving = harness.feature.confirm('cu-1');
  // cu-2 replaces the dialog while confirm('cu-1') is in flight.
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-2', action: 'left click', element: 'Checkout',
  });
  await approving;
  const survivor = harness.published().at(-1).confirmRequest;
  assert.ok(survivor && String(survivor.confirmId) === 'cu-2',
    `the late cleanup of cu-1 must not wipe the newer cu-2 dialog: ${JSON.stringify(survivor)}`);
  harness.state.activeSessionId = 's2';
  await harness.feature.refreshStatus('s2');
  harness.state.activeSessionId = 's1';
  await harness.feature.refreshStatus('s1');
  const resurfaced = harness.published().at(-1).confirmRequest;
  assert.ok(resurfaced && String(resurfaced.confirmId) === 'cu-2',
    `the newer request must resurface from the pending map: ${JSON.stringify(resurfaced)}`);
}

console.log('computer use bridge behavior tests passed');
