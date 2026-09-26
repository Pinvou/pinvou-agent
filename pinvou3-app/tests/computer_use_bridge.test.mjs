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
// The real page loads shared/bridge-messages.js before the tauri bridge
// features (order asserted in web_bridge_domain_contract.test.mjs); the VM
// mirrors that order so localizeKnownError resolves the shared copy table.
const bridgeMessagesSource = fs.readFileSync(
  path.join(__dirname, '..', 'src', 'shared', 'bridge-messages.js'),
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
    [
      'computer_use:grant_required',
      'computer_use:confirm_required',
      'computer_use:state_changed',
    ],
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
  vm.runInContext(bridgeMessagesSource, context, { filename: 'shared/bridge-messages.js' });
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
// The backend only flips `enabled`, so without a status
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

// ── 14. confirm on an expired request closes the dead-end modal ─
{
  const harness = createHarness({
    initialState: { enabled: true },
    failInvoke: (command) => command === 'computer_use_confirm' || command === 'computer_use_deny',
    failMessage: 'computer_use_confirm: confirm request unknown or expired',
  });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'left click', element: 'Buy now',
  });
  await assert.rejects(harness.feature.confirm('cu-1'), /已过期或不存在/);
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
  await assert.rejects(harness.feature.deny('cu-1'), /已过期或不存在/);
  assert.equal(harness.published().at(-1).confirmRequest, null, 'the expired modal must close after deny too');
  // An expiry is not a user decision: an immediate retry re-opens the dialog
  // like any genuinely new request.
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-2', action: 'left click', element: 'Buy now',
  });
  assert.ok(harness.published().at(-1).confirmRequest, 'a new request after an expired deny shows a fresh dialog');
}

// ── 16. session switch clears the stale dialog synchronously, keeps pending ──
// During the async refresh window the previous session's
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
// The backend's stop_all wipes every grant/confirm/token,
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

// ── 19. session switch clears the banner synchronously ─
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
// A grant gate re-armed mid-run (engine re-requested authorization) while
// the backend confirm was still pending; wiping pending.confirm left
// no dialog after Allow. (The idle-expiry model is gone — grants live until
// revoke — but the grant re-request path itself still collides.)
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
// The catch path cleared unconditionally, so a newer request
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
  await assert.rejects(harness.feature.confirm('cu-1'), /已过期或不存在/);
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
  await assert.rejects(harness.feature.confirm('cu-1'), /已过期或不存在/);
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
  await assert.rejects(harness.feature.deny('cu-1'), /已过期或不存在/);
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
  await assert.rejects(harness.feature.deny('cu-2'), /已过期或不存在/);
  assert.equal(harness.published().at(-1).confirmRequest, null,
    'the matching expired dialog must still close after deny');
}

// ── 25. revoke() collapses a same-session confirm dialog ─
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
// The published-slice early-return skipped the pending-map
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

// ── 28. inert branch re-reads status (other-window enable gap) ──────
// A detached window whose slice says disabled never learns
// the feature was enabled in the main window (no enabled-change event, and
// the reconciler skips polling while disabled) — its sessions' grant/confirm
// requests were recorded but never surfaced. The inert branch now fires one
// authoritative refreshStatus, which republishes the recorded pending.
{
  const harness = createHarness({ initialState: { enabled: false } });
  emit(harness, 'computer_use:grant_required', { session_id: 's1' });
  await Promise.resolve();
  assert.ok(
    harness.invoked.some(([cmd]) => cmd === 'computer_use_get_status'),
    'the inert grant branch must re-read authoritative status'
  );
  await new Promise((r) => {
    setTimeout(r, 0);
  });
  const grant = harness.published().at(-1).grantRequest;
  assert.ok(grant, `an enabled-elsewhere window must resurface the grant dialog: ${JSON.stringify(harness.published().at(-1))}`);
}
{
  const harness = createHarness({ initialState: { enabled: false } });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-9', action: 'type 3 characters', element: 'field',
  });
  await new Promise((r) => {
    setTimeout(r, 0);
  });
  const confirm = harness.published().at(-1).confirmRequest;
  assert.ok(
    confirm && String(confirm.confirmId) === 'cu-9',
    `the inert confirm branch must resurface the dialog after the re-read: ${JSON.stringify(harness.published().at(-1))}`
  );
}

// ── 29. a preview-only payload change re-renders the dialog ─────────
// sameRequest compares the typed-text preview too: a re-sent confirm event
// that differs only in typePreviewFull must not be swallowed as a no-op.
{
  const harness = createHarness({ initialState: { enabled: true } });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'type 3 characters',
    element: 'field', type_preview_full: 'abc',
  });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'type 3 characters',
    element: 'field', type_preview_full: 'xyz',
  });
  const last = harness.published().at(-1).confirmRequest;
  assert.equal(last.typePreviewFull, 'xyz',
    'a preview-only change must count as a change');
}

// ── 30. structured payload fields pass through to the published request ──
// Backend contract: the confirm event carries the action name plus optional
// structured fields, with the original English summary kept in `summary` as
// the renderer's fallback. Legacy payloads (summary in `action`, no `summary`
// key) must keep their exact old shape.
{
  const harness = createHarness({ initialState: { enabled: true } });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', summary: 'left click x2 at Some((5, 6))',
    action: 'click', button: 'left', click_count: 2, point: { x: 5, y: 6 },
    element: 'Buy now', text_preview_truncated: false,
  });
  const request = harness.published().at(-1).confirmRequest;
  assert.equal(request.summary, 'left click x2 at Some((5, 6))',
    'the English summary must be kept as the fallback');
  assert.equal(request.actionName, 'click');
  assert.equal(request.button, 'left');
  assert.equal(request.clickCount, 2);
  assert.equal(JSON.stringify(request.point), JSON.stringify({ x: 5, y: 6 }),
    'the structured point must pass through to the published request');
  assert.equal(request.textPreviewTruncated, false);
  // snake_case spellings are accepted too.
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-2', summary: 'type 12 characters',
    action: 'type', element: 'field', text_length: 12,
    text_preview: 'hi', text_preview_truncated: true,
  });
  const typed = harness.published().at(-1).confirmRequest;
  assert.equal(typed.textLength, 12);
  assert.equal(typed.textPreview, 'hi');
  assert.equal(typed.textPreviewTruncated, true);
  // Secure-target masking (round-15): the backend strips the character keys
  // from `chord` and sends their count instead — the bridge must pass the
  // count through so the dialog renders the masked template.
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-2b', summary: 'key 4 characters',
    action: 'key', element: 'Password', chord_masked_chars: 4,
    text_preview_truncated: false,
  });
  const masked = harness.published().at(-1).confirmRequest;
  assert.equal(masked.chord, null, 'no raw chord rides a masked payload');
  assert.equal(masked.chordMaskedChars, 4, 'the masked count must pass through');
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-2c', summary: 'key 1 character',
    action: 'key', element: 'Password', chord: 'Control', chord_masked_chars: 1,
    text_preview_truncated: false,
  });
  const maskedMixed = harness.published().at(-1).confirmRequest;
  assert.equal(maskedMixed.chord, 'Control', 'named keys survive masking');
  assert.equal(maskedMixed.chordMaskedChars, 1);
  // Legacy payload: no summary key — the action field IS the summary.
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-3', action: 'left click x1', element: 'e',
  });
  const legacy = harness.published().at(-1).confirmRequest;
  assert.equal(legacy.summary, 'left click x1');
  assert.equal(legacy.actionName, null);
  assert.equal(legacy.textPreviewTruncated, false);
}

// ── 31. session-less refreshStatus fills enabled + platform_supported ──
// Settings cold start: no session exists yet, and the settings page greys
// the toggle out only once platform_supported lands in the slice.
{
  const harness = createHarness({
    initialState: { enabled: false },
    status: { enabled: true, platform_supported: false },
  });
  harness.state.activeSessionId = null;
  const raw = await harness.feature.refreshStatus(null);
  assert.ok(raw, 'a session-less read must resolve with the backend answer');
  const getStatus = harness.invoked.find(([command]) => command === 'computer_use_get_status');
  assert.equal(getStatus[1].sessionId, '', 'the session-less read must pass an empty session id');
  const last = harness.published().at(-1);
  assert.equal(last.enabled, true, 'the session-less read must publish enabled');
  assert.equal(last.platformSupported, false, 'the session-less read must publish platform_supported');
  // Backends predating the session-less form answer null: the slice stays
  // untouched and nothing throws.
  const legacyHarness = createHarness({ initialState: { enabled: false }, status: null });
  legacyHarness.state.activeSessionId = null;
  const before = { ...legacyHarness.state.computerUse };
  await legacyHarness.feature.refreshStatus(null);
  assert.deepEqual(legacyHarness.state.computerUse, before,
    'a null session-less answer must leave the slice untouched');
}

// ── 32. known backend error strings map onto the localized settings copy ──
// Exact-equality match only: "computer use has no backend on this operating
// system" surfaces as the platformUnsupportedHint text in the persisted UI
// language; anything else passes through verbatim.
{
  const harness = createHarness({
    initialState: { enabled: false },
    failInvoke: (command) => command === 'computer_use_set_enabled',
    failMessage: 'computer use has no backend on this operating system',
  });
  harness.state.settings = { language: 'zh-Hans' };
  await assert.rejects(
    harness.feature.setEnabled(true),
    /当前平台没有电脑使用后端/,
    'the known no-backend error must surface as the localized hint',
  );
  const other = createHarness({
    initialState: { enabled: false },
    failInvoke: (command) => command === 'computer_use_set_enabled',
    failMessage: 'computer use has no backend on this operating system.',
  });
  other.state.settings = { language: 'zh-Hans' };
  await assert.rejects(other.feature.setEnabled(true), /no backend on this operating system/,
    'an unrecognized error string must pass through untouched');
}

// ── 32b. round-17: the consent action paths localize too ──
// The disabled/stopped grant refusals reach the dialog while it is on screen
// (another window stopped the feature mid-decision), and the expired pair is
// matched by its stable marker phrase because it embeds the confirm_id.
{
  const grantDenied = createHarness({
    initialState: { enabled: false },
    failInvoke: (command) => command === 'computer_use_grant',
    failMessage: 'computer use is disabled; enable it in settings before granting control',
  });
  grantDenied.state.settings = { language: 'zh-Hans' };
  await assert.rejects(
    grantDenied.feature.grant('s1'),
    /已在设置中停用/,
    'the disabled refusal must surface as trilingual copy on the grant path',
  );

  const expired = createHarness({
    initialState: { enabled: true },
    failInvoke: (command) => command === 'computer_use_confirm',
    failMessage: 'confirmation request no longer exists (unknown or expired): cu-1',
  });
  expired.state.settings = { language: 'ja' };
  await assert.rejects(
    expired.feature.confirm('cu-1'),
    /有効期限が切れています/,
    'the expired-confirmation error must localize via its stable marker',
  );
}


// ── N. state_changed reconciles cross-window ────────────────────────
{
  const harness = createHarness({ initialState: { enabled: true } });
  // A request the user resolved in ANOTHER window: the state_changed event
  // must trigger an authoritative re-read for the affected session instead
  // of leaving the phantom dialog until the next reconciler tick.
  emit(harness, 'computer_use:state_changed', { reason: 'confirmed', session_id: 's1' });
  await Promise.resolve();
  await new Promise((resolve) => { setImmediate(resolve); });
  assert.ok(
    harness.invoked.some(([command, args]) => command === 'computer_use_get_status' && args.sessionId === 's1'),
    'state_changed must re-read authoritative status for the affected session',
  );
  // A global transition (stop/enable) with no session hint falls back to the
  // active session.
  emit(harness, 'computer_use:state_changed', { reason: 'stopped' });
  await Promise.resolve();
  await new Promise((resolve) => { setImmediate(resolve); });
  const reads = harness.invoked.filter(([command]) => command === 'computer_use_get_status');
  assert.ok(reads.length >= 2, 'a session-less state_changed must refresh the active session');
}

// ── 22. Server truth: resolved requests collapse, served payloads reconstruct ──
{
  // A confirm resolved in ANOTHER window: the server status says none is
  // pending, so this window's stale dialog must collapse on refresh.
  const harness = createHarness({
    initialState: { enabled: true },
    status: { enabled: true, granted: false, stopped: false, platform_supported: true, pending_grant: false, pending_confirm: null },
  });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-old', action: 'left_click', summary: 'stale',
  });
  assert.equal(harness.published().at(-1).confirmRequest.confirmId, 'cu-old');
  await harness.feature.refreshStatus('s1');
  assert.equal(harness.published().at(-1).confirmRequest, null,
    'a resolved confirm must collapse from server truth');
}

{
  // A pending minted elsewhere (this window missed the event): the served
  // payload reconstructs the dialog.
  const payload = {
    session_id: 's1', confirm_id: 'cu-served', action: 'key', summary: 'key ctrl+s',
    element: 'editor', chord: 'ctrl+s',
  };
  const harness = createHarness({
    initialState: { enabled: true },
    status: { enabled: true, granted: true, stopped: false, platform_supported: true, pending_grant: false, pending_confirm: payload },
  });
  await harness.feature.refreshStatus('s1');
  const dialog = harness.published().at(-1).confirmRequest;
  assert.ok(dialog && dialog.confirmId === 'cu-served', 'served payload must reconstruct the dialog');
  assert.equal(dialog.chord, 'ctrl+s', 'structured fields survive reconstruction');
}

{
  // An unanswered grant request pending server-side resurfaces from status
  // alone (e.g. this window reloaded mid-request).
  const harness = createHarness({
    initialState: { enabled: true },
    status: { enabled: true, granted: false, stopped: false, platform_supported: true, pending_grant: true, pending_confirm: null },
  });
  await harness.feature.refreshStatus('s1');
  assert.ok(harness.published().at(-1).grantRequest,
    'a server-pending grant must resurface on refresh');
}

// ── 33. a background-session state_changed refresh must not collapse the active session's live dialog ──
// state_changed carries its own session_id: when session X is resolved in
// another window while THIS window's active session Y has a dialog up, the
// triggered refreshStatus("X") used to see liveRequestSession(Y) !== "X" and
// cleared Y's still-pending dialog until the 30s reconciler resurfaced it.
// The switch-drop stays reserved for refreshes FOR the active session.
{
  const harness = createHarness({ initialState: { enabled: true } });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-1', action: 'left click', element: 'Buy now',
  });
  assert.ok(harness.published().at(-1).confirmRequest, 'the active session dialog must be up');
  // Session 's2' is resolved in ANOTHER window; this window's active session
  // keeps its own dialog.
  emit(harness, 'computer_use:state_changed', { reason: 'confirmed', session_id: 's2' });
  await Promise.resolve();
  await new Promise((resolve) => { setImmediate(resolve); });
  assert.ok(
    harness.invoked.some(([command, args]) => command === 'computer_use_get_status' && args.sessionId === 's2'),
    'the cross-window state_changed must still re-read authoritative status for s2',
  );
  const dialog = harness.published().at(-1).confirmRequest;
  assert.ok(
    dialog && String(dialog.confirmId) === 'cu-1',
    `a background-session refresh must not collapse the active session's dialog: ${JSON.stringify(harness.published().at(-1))}`,
  );
}

// ── 34. a missing slice must not throw in the inert event branches ──
// A freshly opened detached window has no computerUse slice until its mount
// refreshStatus resolves; `!state.computerUse.enabled` threw a TypeError
// there, skipping both the authoritative re-read and any later publish.
{
  const harness = createHarness({ initialState: { enabled: false } });
  harness.state.computerUse = undefined;
  emit(harness, 'computer_use:grant_required', { session_id: 's1' });
  await new Promise((r) => { setTimeout(r, 0); });
  assert.ok(
    harness.invoked.some(([cmd]) => cmd === 'computer_use_get_status'),
    'a missing slice must take the inert re-read branch instead of throwing',
  );
  assert.ok(
    harness.published().at(-1) && harness.published().at(-1).grantRequest,
    'the re-read must republish the grant into the fresh slice',
  );
}
{
  const harness = createHarness({ initialState: { enabled: false } });
  harness.state.computerUse = undefined;
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-9', action: 'type 3 characters', element: 'field',
  });
  await new Promise((r) => { setTimeout(r, 0); });
  assert.ok(
    harness.published().at(-1) && harness.published().at(-1).confirmRequest &&
      String(harness.published().at(-1).confirmRequest.confirmId) === 'cu-9',
    'the confirm inert branch must handle a missing slice the same way',
  );
}

// ── 35. the global stop flag must survive a session-less status refresh ──
// `stopped` is process-global and the backend reports it for an empty session
// id too, but the session-less branch only copied enabled/platformSupported.
// On the draft screen (no active session) the settings page therefore kept
// showing the "turn it off and back on" hint after a successful resume.
// `refreshStatus` resolves `sessionId || state.activeSessionId`, so passing
// null is NOT enough to reach the session-less branch — the harness seeds an
// active session. Clearing it is what actually exercises the code under test.
{
  const harness = createHarness({
    initialState: { stopped: true },
    status: { enabled: true, stopped: false, platform_supported: true },
  });
  harness.state.activeSessionId = null;
  await harness.feature.refreshStatus(null);
  assert.equal(
    harness.state.computerUse.stopped,
    false,
    'a session-less refresh must adopt the backend stop flag',
  );
}
{
  const harness = createHarness({
    initialState: { stopped: false },
    status: { enabled: true, stopped: true, platform_supported: true },
  });
  harness.state.activeSessionId = null;
  await harness.feature.refreshStatus(null);
  assert.equal(
    harness.state.computerUse.stopped,
    true,
    'a session-less refresh must also surface a latched stop',
  );
}

// ── 35b. a session-less refresh must clear a stale per-session grant ──
// The draft screen has no session, so no grant can describe what the user is
// looking at. The session-less branch never touched `granted`, so leaving a
// granted session for the draft kept the control banner (and its Stop
// button) on the welcome page — the same stale-state class as the stop flag
// above, in the same branch.
{
  const harness = createHarness({
    initialState: { enabled: true, granted: true, stopped: false },
    status: { enabled: true, granted: false, stopped: false, platform_supported: true },
  });
  harness.state.activeSessionId = null;
  await harness.feature.refreshStatus(null);
  assert.equal(
    harness.state.computerUse.granted,
    false,
    'a session-less refresh must clear the stale per-session grant',
  );
}

// ── 35c. a session-less refresh must clear stale grant/confirm requests ──
// Same branch, one field over: a request left in the slice kept the previous
// session's consent dialog (and its working Allow button) floating over the
// draft composer. The per-session pending map is NOT the slice; nulling the
// slice fields here must not lose the request for a later switch-back — that
// resurfacing is refreshStatus's job for the active session.
{
  const harness = createHarness({
    initialState: {
      enabled: true,
      granted: true,
      stopped: false,
      grantRequest: { sessionId: 's1' },
      confirmRequest: { sessionId: 's1', action: 'click', element: 'Buy now', confirmId: 'c1' },
    },
    status: { enabled: true, granted: false, stopped: false, platform_supported: true },
  });
  harness.state.activeSessionId = null;
  await harness.feature.refreshStatus(null);
  assert.equal(
    harness.state.computerUse.grantRequest,
    null,
    'a session-less refresh must clear a stale grant request',
  );
  assert.equal(
    harness.state.computerUse.confirmRequest,
    null,
    'a session-less refresh must clear a stale confirm request',
  );
}

// ── 36. turning the feature off must clear the latched stop ──
// The documented resume path is "turn it off and back on". Keeping `stopped`
// set through the off step made the settings row tell a user who had just
// switched the toggle OFF to turn it off — halfway through that same path.
//
// The backend lowers the latch in `set_enabled(false)` (guard.rs), so the
// status this harness answers with is the corrected one. That matters: the
// local write and the authoritative read have to AGREE, or the next refresh
// silently re-latches the flag and the hint comes back.
{
  const harness = createHarness({
    initialState: { enabled: true, stopped: true },
    status: { enabled: false, stopped: false, platform_supported: true },
  });
  await harness.feature.setEnabled(false);
  assert.equal(harness.state.computerUse.enabled, false);
  assert.equal(
    harness.state.computerUse.stopped,
    false,
    'disabling must clear the latched stop: the feature is off, not stopped',
  );
  // The status read that follows any `state_changed` must not undo it.
  await harness.feature.refreshStatus('s1');
  assert.equal(
    harness.state.computerUse.stopped,
    false,
    'the authoritative refresh must agree, not re-latch the stop',
  );
}

console.log('computer use bridge behavior tests passed');
