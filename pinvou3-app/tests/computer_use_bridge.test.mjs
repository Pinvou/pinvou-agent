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
      'invoke("computer_use_get_status", { sessionId: sid })',
      'invoke("computer_use_grant", { sessionId: sid })',
      'invoke("computer_use_revoke", { sessionId: sid })',
      'invoke("computer_use_stop")',
      'invoke("computer_use_confirm", { confirmId })',
      'invoke("computer_use_deny", { confirmId })',
      'invoke("computer_use_set_enabled", { enabled: target })',
      'invoke("computer_use_request_permissions")',
      'invoke("computer_use_request_permissions")',
      'invoke("computer_use_set_virtual_display_preference", { enabled: target })',
      'invoke("computer_use_permission_status")',
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
    // preferVirtualDisplay mirrors the shipped bridge.js slice default (true,
    // the backend default) — this VM does not load bridge.js, so the default
    // must be restated here for the preference-mapping pins below.
    computerUse: {
      enabled: false,
      granted: false,
      stopped: false,
      platformSupported: true,
      preferVirtualDisplay: true,
      ...initialState,
    },
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

// ── 35d. a deleted session's parked request must not resurface ──
// The per-session pending map is what makes switch-back resurfacing work,
// but nothing dropped an entry when its session was deleted, so a request
// (including its full type preview) outlived the session for the whole
// webview lifetime. The prune rides the status refresh — the deleted-
// session helper in the shared bridge knows nothing about this module — and
// must leave live sessions' entries untouched (the control half below).
{
  const harness = createHarness({
    initialState: { enabled: true },
    status: { enabled: true, granted: false, stopped: false, platform_supported: true },
  });
  harness.state.sessions = [{ id: 's1' }, { id: 's2' }];
  emit(harness, 'computer_use:grant_required', { session_id: 's2' });
  // Control: a live background request resurfaces on switch-back.
  harness.state.activeSessionId = 's2';
  await harness.feature.refreshStatus('s2');
  const resurfaced = harness.published().at(-1);
  assert.ok(
    resurfaced && resurfaced.grantRequest && resurfaced.grantRequest.sessionId === 's2',
    `control: a live parked request must resurface: ${JSON.stringify(resurfaced)}`,
  );
  // Delete s2 from the frontend state, run the session-less refresh the
  // prune rides, and switch back: the deleted session's request must be gone.
  harness.state.sessions = [{ id: 's1' }];
  await harness.feature.refreshStatus(null);
  await harness.feature.refreshStatus('s2');
  const after = harness.published().at(-1);
  assert.equal(
    after && after.grantRequest,
    null,
    'a deleted session’s parked request must not resurface',
  );
}

// ── 35e. the session-less refresh must keep the switch-back pending map ──
// 35c pins the slice nulling; this pins the other half of that branch's
// contract: the per-session pending map itself must survive, so leaving a
// session with an unanswered request for the draft and coming back
// resurfaces the still-live request immediately (not just after the
// reconciler's next tick). The request must arrive through the event path —
// a slice-seeded request like 35c's never enters the map, so that shape
// cannot see an over-clear.
{
  const harness = createHarness({
    initialState: { enabled: true },
    status: { enabled: true, granted: false, stopped: false, platform_supported: true },
  });
  harness.state.sessions = [{ id: 's1' }];
  emit(harness, 'computer_use:grant_required', { session_id: 's1' });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1',
    confirm_id: 'c1',
    action: 'left_click',
    element: 'Buy now',
  });
  // Leave for the draft: the session-less refresh nulls the live slice.
  harness.state.activeSessionId = null;
  await harness.feature.refreshStatus(null);
  assert.equal(
    harness.state.computerUse.grantRequest,
    null,
    'the leave-to-draft refresh must clear the live grant request',
  );
  assert.equal(
    harness.state.computerUse.confirmRequest,
    null,
    'the leave-to-draft refresh must clear the live confirm request',
  );
  // Switch back: the parked entries must resurface from the map.
  harness.state.activeSessionId = 's1';
  await harness.feature.refreshStatus('s1');
  const back = harness.published().at(-1);
  assert.ok(
    back && back.grantRequest && back.grantRequest.sessionId === 's1',
    `switch-back must resurface the parked grant request: ${JSON.stringify(back)}`,
  );
  assert.ok(
    back && back.confirmRequest && back.confirmRequest.confirmId === 'c1',
    `switch-back must resurface the parked confirm request: ${JSON.stringify(back)}`,
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

// ── 37. virtualDisplayState/detail flow into the slice (both refresh paths) ──
// The settings row reads the projection; the session-less answer (settings
// cold start) and the session answer must both carry it. A status WITHOUT
// the fields (older backend) must leave the fail-closed defaults untouched,
// and an unrecognized state string maps to "unknown", never verbatim.
{
  const virtualStatus = {
    enabled: true,
    granted: false,
    stopped: false,
    platform_supported: true,
    virtualDisplayState: 'ready',
    virtualDisplayDetail: 'Xorg dummy driver detected',
  };
  const sessionLess = createHarness({ initialState: { enabled: true }, status: virtualStatus });
  sessionLess.state.activeSessionId = null;
  await sessionLess.feature.refreshStatus(null);
  let last = sessionLess.published().at(-1);
  assert.equal(last.virtualDisplayState, 'ready', 'the session-less read must publish virtualDisplayState');
  assert.equal(last.virtualDisplayDetail, 'Xorg dummy driver detected', 'the detail rides along');

  const perSession = createHarness({ initialState: { enabled: true }, status: virtualStatus });
  await perSession.feature.refreshStatus('s1');
  last = perSession.published().at(-1);
  assert.equal(last.virtualDisplayState, 'ready', 'the session read must publish virtualDisplayState too');

  const legacy = createHarness({ initialState: { enabled: true }, status: { enabled: true, platform_supported: true } });
  await legacy.feature.refreshStatus('s1');
  last = legacy.published().at(-1);
  assert.equal('virtualDisplayState' in last, false,
    'a status without the fields must not grow the key (defaults stay fail-closed)');

  const garbage = createHarness({
    initialState: { enabled: true, virtualDisplayState: 'unknown', virtualDisplayDetail: '' },
    status: { enabled: true, platform_supported: true, virtualDisplayState: 'sort-of-ready', virtualDisplayDetail: 'x' },
  });
  await garbage.feature.refreshStatus('s1');
  assert.equal(garbage.state.computerUse.virtualDisplayState, 'unknown',
    'an unrecognized state string must map back to the fail-closed default');
  assert.equal(garbage.state.computerUse.virtualDisplayDetail, '',
    'an unrecognized state must not adopt the detail either');
}

// ── 38. setVirtualDisplayPreference reaches the backend and refreshes ──
{
  const harness = createHarness({
    initialState: { enabled: true },
    status: {
      enabled: true, granted: false, stopped: false, platform_supported: true,
      virtualDisplayState: 'disabled', virtualDisplayDetail: '',
    },
  });
  await harness.feature.setVirtualDisplayPreference(true);
  const call = harness.invoked.find(([command]) => command === 'computer_use_set_virtual_display_preference');
  assert.ok(call, 'the preference write must reach the backend');
  assert.equal(call[1] && call[1].enabled, true, 'the payload must carry the boolean');
  // The wrapper re-reads status before resolving: the fake status still says
  // "disabled" here, which pins that the UI moves via the refreshed slice,
  // not via a local guess.
  assert.equal(harness.state.computerUse.virtualDisplayState, 'disabled',
    'the resolution must follow the re-read authoritative status');
  // A failing write surfaces the error instead of touching the slice.
  const failing = createHarness({
    failInvoke: (command) => command === 'computer_use_set_virtual_display_preference',
  });
  await assert.rejects(failing.feature.setVirtualDisplayPreference(false), /invoke failed/);
}

// ── 39. grant/confirm targets ride the pending map and the granted slice ──
{
  // A real status answer keeps `enabled` true across the mid-test refreshes:
  // the harness's empty default would flip the slice to disabled and push
  // the later grant_required into its inert re-read branch.
  const harness = createHarness({
    initialState: { enabled: true },
    status: { enabled: true, granted: false, stopped: false, platform_supported: true },
  });
  emit(harness, 'computer_use:grant_required', { session_id: 's1', target: 'virtual' });
  let last = harness.published().at(-1);
  assert.equal(last.grantRequest && last.grantRequest.target, 'virtual',
    'a virtual grant request must carry the target');
  // refreshStatus rebuilds the request from the pending entry: the target
  // must survive the switch-away/switch-back resurfacing.
  harness.state.activeSessionId = 's2';
  await harness.feature.refreshStatus('s2');
  harness.state.activeSessionId = 's1';
  await harness.feature.refreshStatus('s1');
  last = harness.published().at(-1);
  assert.equal(last.grantRequest && last.grantRequest.target, 'virtual',
    'the virtual target must survive the refresh-resurfacing');
  await harness.feature.grant('s1');
  last = harness.published().at(-1);
  assert.equal(last.grantedTarget, 'virtual', 'Allow must record the virtual target for the banner');
  // A later PHYSICAL re-request must not inherit the stale virtual target.
  emit(harness, 'computer_use:grant_required', { session_id: 's1' });
  last = harness.published().at(-1);
  assert.equal(last.grantRequest && last.grantRequest.target, undefined,
    'a request without the field keeps the old { sessionId } shape');
  await harness.feature.grant('s1');
  last = harness.published().at(-1);
  assert.equal(last.grantedTarget, 'physical',
    'a physical grant must overwrite the recorded target (fail-neutral)');
  // Stop resets it with the banner.
  await harness.feature.stop();
  assert.equal(harness.published().at(-1).grantedTarget, 'physical',
    'stop must reset the recorded target');
}
{
  const harness = createHarness({ initialState: { enabled: true } });
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-v', summary: 'left click', action: 'left_click',
    element: 'Buy now', target: 'virtual',
  });
  assert.equal(harness.published().at(-1).confirmRequest.target, 'virtual',
    'a virtual confirm payload must carry the target');
  emit(harness, 'computer_use:confirm_required', {
    session_id: 's1', confirm_id: 'cu-p', summary: 'left click', action: 'left_click',
    element: 'Buy now',
  });
  assert.equal('target' in harness.published().at(-1).confirmRequest, false,
    'a payload without the field must not grow a target key');
}

// ── 40. sessionDisplayTarget reconciles the banner truth from status ──
// The banner reads grantedTarget, which the grant_required EVENT derived —
// knowledge a window reload loses. The status now carries the guard's
// per-session registry answer (sessionDisplayTarget): a concrete value wins
// over the event-derived one, null ("no live row") clears the mirror but
// keeps the event-derived value (fail-neutral — the banner must never guess
// "virtual"), and an older backend without the key touches nothing.
{
  // Reload reconciliation: no event was ever seen in this window, yet the
  // status alone flips both the mirror and the banner variant.
  const reload = createHarness({
    initialState: { enabled: true },
    status: { enabled: true, granted: true, stopped: false, platform_supported: true, sessionDisplayTarget: 'virtual' },
  });
  await reload.feature.refreshStatus('s1');
  let last = reload.published().at(-1);
  assert.equal(last.sessionDisplayTarget, 'virtual',
    'a concrete status target must land in the slice');
  assert.equal(last.grantedTarget, 'virtual',
    'the banner variant must reconcile from status after a reload');

  // Status wins over a stale event-derived value.
  const wins = createHarness({
    initialState: { enabled: true, grantedTarget: 'virtual' },
    status: { enabled: true, granted: true, stopped: false, platform_supported: true, session_display_target: 'physical' },
  });
  await wins.feature.refreshStatus('s1');
  last = wins.published().at(-1);
  assert.equal(last.grantedTarget, 'physical',
    'a contradicting status must win over the event-derived value (snake_case accepted)');

  // Null row: the mirror clears, the event-derived value stands.
  const noRow = createHarness({
    initialState: { enabled: true, grantedTarget: 'virtual' },
    status: { enabled: true, granted: false, stopped: false, platform_supported: true, sessionDisplayTarget: null },
  });
  await noRow.feature.refreshStatus('s1');
  last = noRow.published().at(-1);
  assert.equal(last.sessionDisplayTarget, null,
    'a null registry row must clear the mirror');
  assert.equal(last.grantedTarget, 'virtual',
    'null carries no target information: the banner must not be re-guessed');

  // Unrecognized value: fail-neutral like the state vocabulary above.
  const garbage = createHarness({
    initialState: { enabled: true, grantedTarget: 'virtual', sessionDisplayTarget: 'physical' },
    status: { enabled: true, granted: false, stopped: false, platform_supported: true, sessionDisplayTarget: 'sort-of-virtual' },
  });
  await garbage.feature.refreshStatus('s1');
  assert.equal(garbage.state.computerUse.sessionDisplayTarget, null,
    'an unrecognized value must clear the mirror');
  assert.equal(garbage.state.computerUse.grantedTarget, 'virtual',
    'an unrecognized value must leave the banner fail-neutral');

  // Old backend: no key, no growth, banner untouched.
  const legacy = createHarness({
    initialState: { enabled: true, grantedTarget: 'virtual' },
    status: { enabled: true, granted: true, stopped: false, platform_supported: true },
  });
  await legacy.feature.refreshStatus('s1');
  last = legacy.published().at(-1);
  assert.equal('sessionDisplayTarget' in last, false,
    'a status without the field must not grow the key (defaults stay fail-closed)');
  assert.equal(last.grantedTarget, 'virtual',
    'without the field the event-derived value stays authoritative');

  // Session-less branch: a null answer clears a stale mirror (same field
  // hygiene as granted/requests there); an absent key touches nothing.
  const sessionLess = createHarness({
    initialState: { enabled: true, grantedTarget: 'virtual', sessionDisplayTarget: 'virtual' },
    status: { enabled: true, stopped: false, platform_supported: true, sessionDisplayTarget: null },
  });
  sessionLess.state.activeSessionId = null;
  await sessionLess.feature.refreshStatus(null);
  last = sessionLess.published().at(-1);
  assert.equal(last.sessionDisplayTarget, null,
    'a session-less refresh must clear a stale mirror');
  assert.equal(last.grantedTarget, 'virtual',
    'the session-less null answer must not re-guess the banner either');
  const legacyDraft = createHarness({
    initialState: { enabled: true, grantedTarget: 'virtual', sessionDisplayTarget: 'virtual' },
    status: { enabled: true, stopped: false, platform_supported: true },
  });
  legacyDraft.state.activeSessionId = null;
  await legacyDraft.feature.refreshStatus(null);
  assert.equal(legacyDraft.state.computerUse.sessionDisplayTarget, 'virtual',
    'an old backend must not have its mirror disturbed by a draft refresh');
}

// ── 41. preferVirtualDisplay: key-only-when-present, fail-neutral ──
// The frozen status contract always carries the persisted preference; the
// settings switch binds to THIS field (not the derived virtualDisplayState).
// Absent (older backend) ⇒ keep the current value; a non-boolean ⇒ ignored;
// a real boolean ⇒ mirrored, on both refresh paths.
{
  const virtualStatus = {
    enabled: true,
    granted: false,
    stopped: false,
    platform_supported: true,
    virtualDisplayState: 'ready',
    virtualDisplayDetail: '',
    preferVirtualDisplay: false,
  };
  // Session path mirrors the preference.
  const perSession = createHarness({ initialState: { enabled: true }, status: virtualStatus });
  await perSession.feature.refreshStatus('s1');
  const last = perSession.published().at(-1);
  assert.equal(last.preferVirtualDisplay, false,
    'the session read must publish preferVirtualDisplay');
  assert.equal(last.virtualDisplayState, 'ready',
    'the state fields keep riding the same patch');

  // Session-less path (settings cold start) mirrors it too.
  const sessionLess = createHarness({ initialState: { enabled: true }, status: virtualStatus });
  sessionLess.state.activeSessionId = null;
  await sessionLess.feature.refreshStatus(null);
  assert.equal(sessionLess.published().at(-1).preferVirtualDisplay, false,
    'the session-less read must publish preferVirtualDisplay too');

  // Absent field ⇒ keep the current value (fail-neutral; never guessed).
  const legacy = createHarness({
    initialState: { enabled: true, preferVirtualDisplay: false },
    status: { enabled: true, granted: false, stopped: false, platform_supported: true },
  });
  await legacy.feature.refreshStatus('s1');
  assert.equal(legacy.state.computerUse.preferVirtualDisplay, false,
    'a status without the field must keep the current value');
  const legacyDefault = createHarness({
    initialState: { enabled: true },
    status: { enabled: true, granted: false, stopped: false, platform_supported: true },
  });
  await legacyDefault.feature.refreshStatus('s1');
  assert.equal(legacyDefault.state.computerUse.preferVirtualDisplay, true,
    'a status without the field keeps the slice default (true, the backend default)');

  // Non-boolean (unrecognized) ⇒ ignored, never coerced.
  const garbage = createHarness({
    initialState: { enabled: true, preferVirtualDisplay: true },
    status: {
      enabled: true, granted: false, stopped: false, platform_supported: true,
      virtualDisplayState: 'ready', virtualDisplayDetail: '', preferVirtualDisplay: 'yes',
    },
  });
  await garbage.feature.refreshStatus('s1');
  assert.equal(garbage.state.computerUse.preferVirtualDisplay, true,
    'a non-boolean preference must be ignored');
}

// ── 42. virtualDisplayPatch RESETS on unrecognized input ──
// (review finding) The previous value must not survive an answer the backend
// cannot justify: a carried-but-unrecognized state resets the projection to
// the fail-closed defaults instead of keeping a stale "ready"/"active" from
// an earlier snapshot. Key-absent (older backend) still touches nothing.
{
  const stale = createHarness({
    initialState: { enabled: true, virtualDisplayState: 'ready', virtualDisplayDetail: 'was ready' },
    status: {
      enabled: true, granted: false, stopped: false, platform_supported: true,
      virtualDisplayState: 'sort-of-ready', virtualDisplayDetail: 'x',
    },
  });
  await stale.feature.refreshStatus('s1');
  assert.equal(stale.state.computerUse.virtualDisplayState, 'unknown',
    'an unrecognized state must reset the projection, not keep the stale value');
  assert.equal(stale.state.computerUse.virtualDisplayDetail, '',
    'the reset must clear the detail too');

  // A carried non-string (hand-rolled/garbage payload) resets the same way.
  const nonString = createHarness({
    initialState: { enabled: true, virtualDisplayState: 'active', virtualDisplayDetail: 'live' },
    status: {
      enabled: true, granted: false, stopped: false, platform_supported: true,
      virtualDisplayState: 42, virtualDisplayDetail: 'x',
    },
  });
  await nonString.feature.refreshStatus('s1');
  assert.equal(nonString.state.computerUse.virtualDisplayState, 'unknown',
    'a carried non-string state must reset the projection too');
  assert.equal(nonString.state.computerUse.virtualDisplayDetail, '',
    'and clear the stale detail with it');

  // Key absent: still a null patch (older backend, defaults untouched) —
  // pinned in test 37; repeated here so the reset rule cannot silently
  // swallow the absent-key rule.
  const legacy = createHarness({
    initialState: { enabled: true, virtualDisplayState: 'ready', virtualDisplayDetail: 'kept' },
    status: { enabled: true, granted: false, stopped: false, platform_supported: true },
  });
  await legacy.feature.refreshStatus('s1');
  assert.equal(legacy.state.computerUse.virtualDisplayState, 'ready',
    'a status without the fields must leave the current value untouched');
}

// ── 43. sameRequest includes target: a flipped-target re-request republishes ──
// (review finding, reproduced by the reviewer) A virtual grant dialog is up;
// the same session re-requests with the target flipped (e.g. xvfb vanished
// mid-epoch and the backend now asks for the physical desktop). The dialog
// must be RE-published with the physical copy — without the target term in
// sameRequest the patch counts as unchanged and the stale virtual copy
// stays on screen.
{
  const harness = createHarness({
    initialState: { enabled: true },
    status: { enabled: true, granted: false, stopped: false, platform_supported: true },
  });
  emit(harness, 'computer_use:grant_required', { session_id: 's1', target: 'virtual' });
  assert.equal(harness.published().at(-1).grantRequest.target, 'virtual',
    'fixture: the virtual dialog is up');
  const before = harness.published().length;
  emit(harness, 'computer_use:grant_required', { session_id: 's1', target: 'physical' });
  assert.ok(harness.published().length > before,
    'a flipped-target re-request must count as a change and publish');
  const last = harness.published().at(-1);
  assert.ok(last.grantRequest, 'the re-request dialog must be up');
  assert.equal(last.grantRequest.target, undefined,
    'the dialog copy must flip to the fail-neutral physical shape (no target key)');
  // And an identical re-send stays a no-op (the unchanged-request rule the
  // target term must not break).
  const stable = harness.published().length;
  emit(harness, 'computer_use:grant_required', { session_id: 's1', target: 'physical' });
  assert.equal(harness.published().length, stable,
    'an identical re-request must stay a publish no-op');
}

// ── 44. pending.grantTarget dies with its epoch ──
// (review finding) Wherever pending.grant transitions to false (grant
// success, revoke, reconcile-false), grantTarget must be deleted: the
// reconcile-TRUE branch never writes a target (that is the event's job), so
// a stale target would resurface virtual copy for a LATER event-less
// request. With the key gone, the resurfaced dialog renders the fail-neutral
// physical copy. Each case: virtual request → epoch ends → a new request is
// raised in another window (this window never sees its event) → server-truth
// reconciliation resurfaces it → the copy must be physical.
async function assertEventLessResurfacingRendersPhysical(harness, sharedStatus) {
  sharedStatus.pending_grant = true;
  sharedStatus.granted = false;
  await harness.feature.refreshStatus('s1');
  const last = harness.published().at(-1);
  assert.ok(last.grantRequest, 'the event-less request must resurface');
  assert.equal(last.grantRequest.target, undefined,
    'the resurfaced dialog must render the fail-neutral physical copy (no target key)');
}
{
  // (a) grant success ends the epoch.
  const sharedStatus = { enabled: true, granted: false, stopped: false, platform_supported: true };
  const granted = createHarness({ initialState: { enabled: true }, status: sharedStatus });
  emit(granted, 'computer_use:grant_required', { session_id: 's1', target: 'virtual' });
  assert.equal(granted.published().at(-1).grantRequest.target, 'virtual', 'fixture: virtual dialog up');
  await granted.feature.grant('s1');
  await assertEventLessResurfacingRendersPhysical(granted, sharedStatus);
}
{
  // (b) revoke ends the epoch.
  const sharedStatus = { enabled: true, granted: false, stopped: false, platform_supported: true };
  const revoked = createHarness({ initialState: { enabled: true }, status: sharedStatus });
  emit(revoked, 'computer_use:grant_required', { session_id: 's1', target: 'virtual' });
  await revoked.feature.revoke('s1');
  await assertEventLessResurfacingRendersPhysical(revoked, sharedStatus);
}
{
  // (c) reconcile-false ends the epoch (resolved in another window).
  const sharedStatus = { enabled: true, granted: false, stopped: false, platform_supported: true };
  const reconciled = createHarness({ initialState: { enabled: true }, status: sharedStatus });
  emit(reconciled, 'computer_use:grant_required', { session_id: 's1', target: 'virtual' });
  sharedStatus.pending_grant = false;
  await reconciled.feature.refreshStatus('s1');
  assert.equal(reconciled.published().at(-1).grantRequest, null,
    'fixture: the other-window resolution collapsed the dialog');
  await assertEventLessResurfacingRendersPhysical(reconciled, sharedStatus);
}

// ── 45. an event-less pending grant seeds its target from status truth ──
// (review finding) A window reloaded with an unanswered grant never saw the
// grant_required event. The reconciler's grant-true branch now seeds
// pending.grantTarget from the SAME status payload's per-session display
// target — server truth for the live request (written once at backend
// creation), never a previous epoch's event value — so a virtual session's
// dialog renders the virtual copy instead of the physical copy that
// promises per-action confirmations a virtual grant structurally never
// raises. Absent/unknown targets stay fail-neutral physical; a physical
// truth keeps the plain (no-key) shape, which renders the default copy.
{
  const sharedStatus = {
    enabled: true, granted: false, stopped: false, platform_supported: true,
    pending_grant: true, session_display_target: 'virtual',
  };
  const harness = createHarness({ initialState: { enabled: true }, status: sharedStatus });
  await harness.feature.refreshStatus('s1');
  const last = harness.published().at(-1);
  assert.ok(last.grantRequest, 'the reloaded pending grant must resurface');
  assert.equal(last.grantRequest.target, 'virtual',
    'the dialog must render the virtual copy seeded from the status session truth');
  // The seed dies with the epoch like any event-derived target: after the
  // grant resolves elsewhere, a fresh event-less request with no session
  // truth falls back to physical.
  sharedStatus.pending_grant = false;
  await harness.feature.refreshStatus('s1');
  assert.equal(harness.published().at(-1).grantRequest, null,
    'fixture: the resolved epoch collapsed the dialog');
  const laterStatus = {
    enabled: true, granted: false, stopped: false, platform_supported: true,
    pending_grant: true,
  };
  const later = createHarness({ initialState: { enabled: true }, status: laterStatus });
  await later.feature.refreshStatus('s1');
  assert.equal(later.published().at(-1).grantRequest.target, undefined,
    'a session truth without the field stays fail-neutral physical');

  // A concrete physical truth seeds too — but "physical" renders the
  // default (no-target-key) copy shape.
  const physical = createHarness({
    initialState: { enabled: true },
    status: { ...sharedStatus, pending_grant: true, session_display_target: 'physical' },
  });
  await physical.feature.refreshStatus('s1');
  assert.equal(physical.published().at(-1).grantRequest.target, undefined,
    'a physical-seeded target renders the default physical copy shape');

  // A carried UNKNOWN target value must not be copied into the request.
  const unknown = createHarness({
    initialState: { enabled: true },
    status: { ...sharedStatus, pending_grant: true, session_display_target: 'weird' },
  });
  await unknown.feature.refreshStatus('s1');
  assert.equal(unknown.published().at(-1).grantRequest.target, undefined,
    'an unknown session target must stay fail-neutral physical');
}

console.log('computer use bridge behavior tests passed');
