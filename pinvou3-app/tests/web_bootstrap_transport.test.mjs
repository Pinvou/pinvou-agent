import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const source = fs.readFileSync(path.join(root, 'src', 'platform', 'web', 'bootstrap.js'), 'utf8');

function bootClient(policy, { hash = '#endpoint=endpoint_test&token=access_test' } = {}) {
  const storage = new Map();
  const dispatched = [];
  const timers = [];
  const windowListeners = new Map();
  let timerId = 0;

  class MockWebSocket {
    static CONNECTING = 0;
    static OPEN = 1;
    static CLOSING = 2;
    static CLOSED = 3;
    static instances = [];
    // Round-9 review: the throwing-constructor path (real browsers do throw
    // on an invalid URL / policy block) previously had zero coverage — the
    // default fixture's constructor cannot throw. Setting this field to N
    // makes construction number N+1 (index >= N) throw, without registering
    // the instance.
    static throwFrom = null;

    constructor(url) {
      if (MockWebSocket.throwFrom !== null && MockWebSocket.instances.length >= MockWebSocket.throwFrom) {
        throw new TypeError('constructor blocked by the harness');
      }
      this.url = url;
      this.readyState = 0;
      this.listeners = new Map();
      this.sent = [];
      this.closeCalled = false;
      MockWebSocket.instances.push(this);
    }

    addEventListener(name, listener) {
      const listeners = this.listeners.get(name) || [];
      listeners.push(listener);
      this.listeners.set(name, listeners);
    }

    emit(name, event = {}) {
      for (const listener of this.listeners.get(name) || []) listener(event);
    }

    open() {
      this.readyState = MockWebSocket.OPEN;
      this.emit('open');
    }

    message(value) {
      this.emit('message', { data: JSON.stringify(value) });
    }

    send(encoded) {
      this.sent.push(JSON.parse(encoded));
    }

    close() {
      if (this.closeCalled) return;
      this.closeCalled = true;
      this.readyState = 3;
      this.emit('close');
    }
  }

  class CustomEvent {
    constructor(type, init = {}) {
      this.type = type;
      this.detail = init.detail;
    }
  }

  const window = {
    location: {
      href: 'https://relay.test/pinvou3/remote/',
      hash,
      reload() {},
    },
    crypto: {
      randomUUID: () => '00000000-0000-4000-8000-000000000000',
      getRandomValues(bytes) { bytes.fill(7); return bytes; },
    },
    dispatchEvent(event) { dispatched.push(event); },
    setTimeout(callback, delay) {
      timerId += 1;
      timers.push({ id: timerId, callback, delay });
      return timerId;
    },
    clearTimeout(id) {
      const index = timers.findIndex(timer => timer.id === id);
      if (index >= 0) timers.splice(index, 1);
    },
    setInterval(callback, delay) {
      timerId += 1;
      timers.push({ id: timerId, callback, delay });
      return timerId;
    },
    clearInterval(id) {
      const index = timers.findIndex(timer => timer.id === id);
      if (index >= 0) timers.splice(index, 1);
    },
    addEventListener(name, listener) {
      const listeners = windowListeners.get(name) || [];
      listeners.push(listener);
      windowListeners.set(name, listeners);
    },
    emit(name, event = {}) {
      for (const listener of windowListeners.get(name) || []) listener(event);
    },
  };
  window.window = window;
  // Round-20 review: an advanceable clock (the Date the shimmed production
  // code reads). When not advanced, each call passes through the host's real
  // clock (plus offset), so existing fixture behavior is unchanged; new
  // fixtures advance whole stretches via __clock.advance to drive the
  // wedge-window / silence-window judgments. Clock-driven production lines
  // (closingStartedAt/lastInboundAt resets and gate ordering) previously
  // could only be exercised by hand-injecting values in fixtures, where the
  // corresponding mutations all stayed green and the behavior was
  // unobservable (S07 evidence).
  const clock = { offsetMs: 0 };
  const shimDate = { now: () => Date.now() + clock.offsetMs };
  window.__clock = { advance: ms => { clock.offsetMs += ms; } };
  const deterministicMath = Object.create(Math);
  deterministicMath.random = () => 0.5;

  const document = {
    currentScript: { src: 'https://relay.test/pinvou3/remote/platform/web/bootstrap.js' },
    visibilityState: 'visible',
  };

  const context = {
    window,
    document,
    sessionStorage: {
      getItem(key) { return storage.has(key) ? storage.get(key) : null; },
      setItem(key, value) { storage.set(key, String(value)); },
    },
    fetch: async () => ({
      ok: true,
      async json() {
        return policy || { allowed_commands: [], allowed_events: ['chat:delta'] };
      },
    }),
    WebSocket: MockWebSocket,
    CustomEvent,
    URL,
    URLSearchParams,
    Uint8Array,
    Promise,
    Map,
    Set,
    Object,
    Array,
    Number,
    String,
    Math: deterministicMath,
    Date: shimDate,
    JSON,
    Error,
    console: {
      log: console.log.bind(console),
      warn: console.warn.bind(console),
      error() {},
    },
    queueMicrotask,
  };
  vm.runInNewContext(source, context, { filename: 'platform/web/bootstrap.js' });
  return { window, document, storage, dispatched, timers, MockWebSocket, clock };
}

// Reconnect backoff timers (<10s) share the timers list with the 30s watchdog
// interval and the 180s RPC timeout, so reconnect assertions must pick out
// only the backoff timers.
function takeReconnectTimer(timers) {
  const index = timers.findIndex(timer => timer.delay < 10_000);
  assert.ok(index >= 0, 'expected a pending reconnect timer');
  return timers.splice(index, 1)[0];
}

async function connectWithCapabilities(events, listener) {
  const harness = bootClient();
  const client = harness.window.PinvouWebClient;
  await client.policyPromise;
  if (listener) await client.listen('chat:delta', listener);
  client.markFrontendReady();
  await Promise.resolve();
  client.markStateReady();

  const socket = harness.MockWebSocket.instances[0];
  socket.open();
  socket.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_test',
    desktop_connected: true,
  });
  const readinessBeforeSnapshot = socket.sent.filter(message => message.type === 'client_ready');
  assert.equal(readinessBeforeSnapshot.length, 1);
  assert.equal(readinessBeforeSnapshot[0].state_ready, false,
    'a hydrated UI must still negotiate desktop capabilities before replay');
  socket.message({
    v: 2,
    type: 'desktop_snapshot',
    stream_epoch: 'epoch_test',
    seq: 0,
    snapshot: {
      capabilities: { protocol_version: 2, commands: [], events },
    },
  });
  const readinessAfterSnapshot = socket.sent.filter(message => message.type === 'client_ready');
  assert.deepEqual(readinessAfterSnapshot.map(message => message.state_ready), [false, true]);
  return { ...harness, client, socket };
}

{
  const { window, MockWebSocket, timers } = bootClient();
  const client = window.PinvouWebClient;
  await client.policyPromise;

  const firstSocket = MockWebSocket.instances[0];
  assert.equal(firstSocket.url, 'wss://relay.test/pinvou3/remote/ws');
  firstSocket.open();
  firstSocket.message({ type: 'error', code: 'endpoint_not_found' });
  assert.equal(timers.filter(timer => timer.delay < 10_000).length, 1);
  const firstReconnect = takeReconnectTimer(timers);
  assert.equal(firstReconnect.delay, 500);
  firstReconnect.callback();

  const secondSocket = MockWebSocket.instances[1];
  secondSocket.open();
  secondSocket.message({ type: 'error', code: 'endpoint_not_found' });
  assert.equal(timers.filter(timer => timer.delay < 10_000).length, 1);
  const secondReconnect = takeReconnectTimer(timers);
  assert.equal(secondReconnect.delay, 1000);
  secondReconnect.callback();

  const registeredSocket = MockWebSocket.instances[2];
  registeredSocket.open();
  registeredSocket.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_test',
    desktop_connected: true,
  });
  registeredSocket.close();
  assert.equal(timers.filter(timer => timer.delay < 10_000).length, 1);
  assert.equal(takeReconnectTimer(timers).delay, 500);
}

{
  let handled = 0;
  const { client, socket, storage } = await connectWithCapabilities([], () => { handled += 1; });
  assert.equal(socket.sent.some(message => message.type === 'event_subscribe'), false);
  socket.message({
    v: 2,
    type: 'event',
    event: 'chat:delta',
    stream_epoch: 'epoch_test',
    seq: 1,
    payload: { text: 'must not dispatch' },
  });
  await client.eventDispatch;
  assert.equal(handled, 0);
  assert.equal(client.connectionState.status, 'incompatible_desktop');
  assert.equal(JSON.parse(storage.get('pinvou.web.cursor.endpoint_test')).after_seq, 0);
}

{
  let handled = 0;
  const { client, socket, storage, timers } = await connectWithCapabilities(['chat:delta'], () => {
    handled += 1;
    throw new Error('listener failed');
  });
  assert.equal(socket.sent.filter(message => message.type === 'event_subscribe').length, 1);
  socket.message({
    v: 2,
    type: 'event',
    event: 'chat:delta',
    stream_epoch: 'epoch_test',
    seq: 1,
    payload: { text: 'must replay' },
  });
  await client.eventDispatch;
  assert.equal(handled, 1);
  assert.equal(socket.closeCalled, true);
  assert.equal(JSON.parse(storage.get('pinvou.web.cursor.endpoint_test')).after_seq, 0);
  // The catch path invalidates the socket itself, so the close event is noise
  // under the close handler's identity gate: the recovery must schedule the
  // replay reconnect directly, not wait for a close event that never runs.
  assert.equal(timers.filter(timer => timer.delay < 10_000).length, 1,
    'event-processing failure must schedule a reconnect for replay');
  // Round-19 review: the catch path's four cycle-flag resets previously had
  // zero pins (deleting all four lines kept this file and the contract suite
  // green — probe evidence). Residue of the old cycle's joined/leaseId would
  // let a sendJoined frame sent after the successor socket OPENs but before
  // its join ack carry the previous cycle's lease (same family as the
  // cross-cycle leak the handleMessage identity gate shuts off).
  assert.equal(client.joined, false,
    'the catch path must reset the joined flag');
  assert.equal(client.desktopOnline, false,
    'the catch path must reset desktopOnline');
  assert.equal(client.desktopCapabilitiesReady, false,
    'the catch path must reset desktopCapabilitiesReady');
  assert.equal(client.leaseId, null,
    'the catch path must clear the cycle lease');
}

{
  let handled = 0;
  const { client, socket, storage } = await connectWithCapabilities(['chat:delta'], async () => {
    await Promise.resolve();
    handled += 1;
  });
  socket.message({
    v: 2,
    type: 'event',
    event: 'chat:delta',
    stream_epoch: 'epoch_test',
    seq: 1,
    payload: { text: 'commit after listener' },
  });
  await client.eventDispatch;
  assert.equal(handled, 1);
  assert.equal(JSON.parse(storage.get('pinvou.web.cursor.endpoint_test')).after_seq, 1);
}

// Device file upload capability: fail closed until a snapshot arrives; older
// desktops missing either command stay closed, and the surface opens only once
// both commands exist — matching the desktop attachment dual-entry show/hide
// negotiation.
{
  const uploadCommands = [
    'web_access_upload_attachment_chunk',
    'web_access_abort_attachment_upload',
    'web_access_discard_attachment',
  ];
  const negotiate = (snapshotCommands) => {
    const harness = bootClient({ allowed_commands: uploadCommands, allowed_events: ['chat:delta'] });
    const client = harness.window.PinvouWebClient;
    return client.policyPromise.then(() => {
      client.markFrontendReady();
      return Promise.resolve().then(() => {
        client.markStateReady();
        assert.equal(harness.window.PinvouPlatform.can('deviceFileUpload'), false,
          'device upload must fail closed before the desktop capability snapshot arrives');
        const socket = harness.MockWebSocket.instances[0];
        socket.open();
        socket.message({
          v: 2,
          type: 'web_client_joined',
          endpoint_id: 'endpoint_test',
          lease_id: 'lease_test',
          desktop_connected: true,
        });
        socket.message({
          v: 2,
          type: 'desktop_snapshot',
          stream_epoch: 'epoch_test',
          seq: 0,
          snapshot: {
            capabilities: { protocol_version: 2, commands: snapshotCommands, events: ['chat:delta'] },
          },
        });
        return harness.window.PinvouPlatform.can('deviceFileUpload');
      });
    });
  };
  assert.equal(await negotiate(uploadCommands), true,
    'a desktop advertising the complete upload lifecycle must enable the device upload entry');
  assert.equal(await negotiate(uploadCommands.slice(0, 2)), false,
    'a desktop missing attachment discard must keep the device upload entry hidden');
  assert.equal(await negotiate(uploadCommands.slice(0, 1)), false,
    'an older desktop missing the abort command must keep the device upload entry hidden');
  assert.equal(await negotiate([]), false,
    'an older desktop without upload support must keep the device upload entry hidden');
}

// Code mode opens only when the target desktop implements the complete
// Web-safe ACP command and event contract. Any missing command or acp:event
// marks an older desktop, so the entry must fail closed.
{
  const acpCommands = [
    'web_access_list_acp_agents',
    'web_access_get_acp_agent_status',
    'web_access_list_codex_acp_sessions',
    'web_access_create_codex_acp_session',
    'web_access_get_codex_acp_session_info',
    'web_access_set_codex_acp_model',
    'web_access_set_codex_acp_mode',
    'web_access_set_codex_acp_config_option',
    'web_access_codex_acp_prompt',
    'web_access_cancel_codex_acp',
    'web_access_get_codex_acp_timeline',
    'web_access_get_codex_acp_pending_permissions',
    'web_access_respond_codex_acp_permission',
    'web_access_get_codex_acp_pending_elicitations',
    'web_access_respond_codex_acp_elicitation',
    'web_access_list_codex_workspace',
    'web_access_search_codex_workspace',
    'web_access_preview_codex_workspace_file',
    'web_access_get_codex_workspace_changes',
    'web_access_get_codex_workspace_diff',
    'web_access_list_host_files',
    'web_access_ingest_file',
    'web_access_discard_attachment',
  ];
  const negotiate = async (snapshotCommands, snapshotEvents) => {
    const harness = bootClient({
      allowed_commands: acpCommands,
      allowed_events: ['acp:event'],
    });
    const client = harness.window.PinvouWebClient;
    await client.policyPromise;
    client.markFrontendReady();
    await Promise.resolve();
    client.markStateReady();
    assert.equal(harness.window.PinvouPlatform.can('acpCodeMode'), false,
      'ACP code mode must fail closed before capability negotiation');
    const socket = harness.MockWebSocket.instances[0];
    socket.open();
    socket.message({
      v: 2,
      type: 'web_client_joined',
      endpoint_id: 'endpoint_test',
      lease_id: 'lease_test',
      desktop_connected: true,
    });
    socket.message({
      v: 2,
      type: 'desktop_snapshot',
      stream_epoch: 'epoch_test',
      seq: 0,
      snapshot: {
        capabilities: {
          protocol_version: 2,
          commands: snapshotCommands,
          events: snapshotEvents,
        },
      },
    });
    const supported = harness.window.PinvouPlatform.can('acpCodeMode');
    socket.close();
    assert.equal(harness.window.PinvouPlatform.can('acpCodeMode'), supported,
      'a transient disconnect must retain the last negotiated compatibility result');
    assert.equal(harness.window.PinvouPlatform.canInvoke('web_access_list_acp_agents'), false,
      'transport-backed commands must remain unavailable while disconnected');
    return supported;
  };

  assert.equal(await negotiate(acpCommands, ['acp:event']), true,
    'the complete Web ACP contract must enable code mode');
  assert.equal(await negotiate(acpCommands.slice(0, -1), ['acp:event']), false,
    'a desktop missing one required ACP command must keep code mode hidden');
  assert.equal(await negotiate(acpCommands, []), false,
    'a desktop missing acp:event must keep code mode hidden');
}

// The first message uses the caller-provided stable request ID. After a
// WebSocket reconnect the same ID must be reused so the desktop can return the
// original result from its persistent RPC ledger instead of creating another
// session or sending the message again.
{
  const command = 'web_access_create_session_and_chat';
  const harness = bootClient({ allowed_commands: [command], allowed_events: [] });
  const { window, MockWebSocket, timers } = harness;
  const client = window.PinvouWebClient;
  await client.policyPromise;
  client.markFrontendReady();
  await new Promise(resolve => { setImmediate(resolve); });
  client.markStateReady();

  const firstSocket = MockWebSocket.instances[0];
  firstSocket.open();
  firstSocket.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_one',
    desktop_connected: true,
  });
  firstSocket.message({
    v: 2,
    type: 'desktop_snapshot',
    stream_epoch: 'epoch_test',
    seq: 0,
    snapshot: { capabilities: { protocol_version: 2, commands: [command], events: [] } },
  });

  const stableId = 'first_turn_chat_00000000-0000-4000-8000-000000000000';
  const resultPromise = window.__TAURI__.core.invokeWithRequestId(
    command,
    { message: 'hello', attachmentHandles: [], restrictTools: false },
    stableId,
  );
  await new Promise(resolve => { setImmediate(resolve); });
  const firstRequest = firstSocket.sent.find(message => message.type === 'rpc_request');
  assert.ok(firstRequest, JSON.stringify({
    sent: firstSocket.sent,
    frontendReady: client.frontendReady,
    stateReady: client.stateReady,
    desktopCapabilitiesReady: client.desktopCapabilitiesReady,
    allowedCommands: [...client.allowedCommands || []],
  }));
  assert.equal(firstRequest.id, stableId);
  assert.equal(firstRequest.client_request_id, stableId);

  firstSocket.close();
  const reconnectTimer = timers.find(timer => timer.delay < 10_000);
  assert.ok(reconnectTimer, 'disconnect must schedule a reconnect while the RPC remains pending');
  reconnectTimer.callback();
  const secondSocket = MockWebSocket.instances[1];
  secondSocket.open();
  secondSocket.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_two',
    desktop_connected: true,
  });
  secondSocket.message({
    v: 2,
    type: 'desktop_snapshot',
    stream_epoch: 'epoch_test',
    seq: 0,
    snapshot: { capabilities: { protocol_version: 2, commands: [command], events: [] } },
  });
  const retriedRequest = secondSocket.sent.find(message => message.type === 'rpc_request');
  assert.equal(retriedRequest.id, stableId, 'reconnect retry must preserve the idempotency key');
  assert.deepEqual(retriedRequest.args, firstRequest.args, 'reconnect retry must preserve the command fingerprint');
  secondSocket.message({
    v: 2,
    type: 'rpc_response',
    id: stableId,
    ok: true,
    result: { id: 'session_created_once' },
  });
  assert.equal((await resultPromise).id, 'session_created_once');
}

// Structured error codes must reach the Bridge so it can tell a safely
// retryable timeout from an unknown-outcome failure.
{
  const command = 'web_access_create_session_and_chat';
  const harness = bootClient({ allowed_commands: [command], allowed_events: [] });
  const client = harness.window.PinvouWebClient;
  await client.policyPromise;
  client.markFrontendReady();
  await Promise.resolve();
  client.markStateReady();
  const socket = harness.MockWebSocket.instances[0];
  socket.open();
  socket.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_test',
    desktop_connected: true,
  });
  socket.message({
    v: 2,
    type: 'desktop_snapshot',
    stream_epoch: 'epoch_test',
    seq: 0,
    snapshot: { capabilities: { protocol_version: 2, commands: [command], events: [] } },
  });
  const requestId = 'first_turn_outcome_unknown';
  const rejected = harness.window.__TAURI__.core.invokeWithRequestId(command, { message: 'hello' }, requestId);
  await new Promise(resolve => { setImmediate(resolve); });
  socket.message({
    v: 2,
    type: 'rpc_response',
    id: requestId,
    ok: false,
    error: 'outcome cannot be replayed safely',
    error_code: 'outcome_unknown',
  });
  await assert.rejects(rejected, error => (
    error.code === 'outcome_unknown' && error.requestId === requestId
  ));
}

// A TTL-expired endpoint_revoked is not a permanent revocation (PROTOCOL.md):
// the desktop may recreate the same endpoint later with its persistent
// credentials, so the browser must enter desktop_offline, wait, and rejoin
// automatically through the ordinary reconnect path instead of a permanent
// dead-end overlay.
{
  const { client, socket, timers, MockWebSocket, dispatched } = await connectWithCapabilities(['chat:delta'], () => {});
  socket.message({
    v: 2,
    type: 'endpoint_revoked',
    endpoint_id: 'endpoint_test',
    reason: 'desktop_offline_ttl_expired',
  });
  assert.equal(client.closedPermanently, false,
    'TTL expiry must not poison the tab: the desktop may recreate the endpoint');
  assert.equal(client.desktopOnline, false);
  assert.ok(dispatched.some(event => (
    event.type === 'pinvou:web-connection' && event.detail.status === 'desktop_offline'
  )), 'TTL expiry must surface the desktop-offline state');
  assert.equal(socket.closeCalled, true, 'the expired socket must be closed normally');
  // Keep the dedicated desktop_offline copy ("desktop offline, keep this page
  // open") through the backoff window: the immediately following
  // close→scheduleReconnect must not overwrite the status with "connecting",
  // or the user would stare at a misleading spinner for the whole up-to-60s
  // backoff.
  assert.equal(client.connectionState.status, 'desktop_offline',
    'the dedicated desktop-offline copy must survive the close/reconnect clobber');
  takeReconnectTimer(timers).callback();
  assert.equal(client.connectionState.status, 'connecting',
    'the ordinary reconnect path takes over when the retry actually fires');
  const rejoined = MockWebSocket.instances[1];
  rejoined.open();
  assert.ok(rejoined.sent.some(message => message.type === 'web_client_join'),
    'the browser must rejoin automatically after TTL expiry');
  rejoined.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_next',
    desktop_connected: true,
  });
  assert.equal(client.joined, true);
  assert.equal(timers.some(timer => timer.delay === 30_000), true,
    'the liveness watchdog must keep running across a TTL-expiry reconnect');
}

// An abnormal disconnect fires error before close: the error listener must
// likewise not overwrite the dedicated desktop_offline copy with "connecting"
// — the close→scheduleReconnect side has the guard, but a round-4 review found
// the error listener did not, and a TCP reset inside the offline window would
// swap the honest copy for a spinner.
{
  const { client, socket } = await connectWithCapabilities(['chat:delta'], () => {});
  socket.message({
    v: 2,
    type: 'endpoint_revoked',
    endpoint_id: 'endpoint_test',
    reason: 'desktop_offline_ttl_expired',
  });
  assert.equal(client.connectionState.status, 'desktop_offline');
  socket.emit('error', {});
  assert.equal(client.connectionState.status, 'desktop_offline',
    'an abnormal socket error during the offline window must keep the dedicated offline copy');

  const healthy = await connectWithCapabilities(['chat:delta'], () => {});
  healthy.socket.emit('error', {});
  assert.equal(healthy.client.connectionState.status, 'connecting',
    'an error on an ordinary socket must still escalate to the connecting state');
}

// Round-10 review minor: the half-open socket closed speculatively by the
// watchdog surfaces as error first on some browsers (a failed closing
// handshake is reported as error+close). That is part of the closing
// handshake, not a verification failure — it must not count a speculative
// failure nor light up connecting early (otherwise "the UI stays connected
// until verification fails" forks across browser engines). The mock's close
// never emits error, so this interleaving was previously unreachable.
{
  const speculative = await connectWithCapabilities(['chat:delta'], () => {});
  speculative.client.lastInboundAt = Date.now() - 121_000;
  speculative.client._livenessCheck();
  assert.equal(speculative.socket.closeCalled, true,
    'the watchdog must have closed the silent socket');
  assert.equal(speculative.client.speculativeReconnect, true,
    'the first ackless cycle stays under the failure cap');
  assert.equal(speculative.client.connectionState.status, 'connected',
    'the reconnect after a speculative close is not visible yet');
  speculative.socket.emit('error', {});
  assert.equal(speculative.client.speculativeFailures, 0,
    'an error on the speculative source is the close handshake, not a verification failure');
  assert.equal(speculative.client.connectionState.status, 'connected',
    'the speculative source error must not surface the connecting banner');
}

// A real revocation (any other reason, or a missing reason) keeps today's
// permanent-close behavior.
{
  const revoke = async message => {
    const harness = await connectWithCapabilities(['chat:delta'], () => {});
    harness.socket.message(message);
    return harness;
  };
  const revoked = await revoke({
    v: 2,
    type: 'endpoint_revoked',
    endpoint_id: 'endpoint_test',
    reason: 'revoked',
  });
  assert.equal(revoked.client.closedPermanently, true);
  assert.equal(revoked.client.connectionState.status, 'revoked');
  assert.equal(revoked.socket.closeCalled, true);
  assert.equal(revoked.timers.some(timer => timer.delay < 10_000), false,
    'a real revoke must not schedule reconnects');
  assert.equal(revoked.timers.some(timer => timer.delay === 30_000), false,
    'a real revoke must tear down the liveness watchdog');

  const unreasoned = await revoke({ v: 2, type: 'endpoint_revoked', endpoint_id: 'endpoint_test' });
  assert.equal(unreasoned.client.closedPermanently, true,
    'an endpoint_revoked without a TTL reason stays a permanent revoke');
  assert.equal(unreasoned.client.connectionState.status, 'revoked');
}

// Half-open connection watchdog: an OPEN socket silent past the threshold must
// be speculatively reconnected (cursor replay makes reconnects idempotent);
// recent inbound traffic or a non-OPEN socket must not be killed by mistake.
// Staleness is suspicion, not proof: the UI stays connected and no connecting
// status is broadcast while the speculative reconnect verifies; only a failed
// verification (the reconnect socket dying again before its join) clears the
// speculative state and escalates to a visible connecting status.
{
  const stale = await connectWithCapabilities(['chat:delta'], () => {});
  stale.client.lastInboundAt = Date.now() - 121_000;
  const statusEventsBefore = stale.dispatched.filter(event => event.type === 'pinvou:web-connection').length;
  stale.client._livenessCheck();
  assert.equal(stale.socket.closeCalled, true,
    'a silent OPEN socket past the liveness threshold must be speculatively closed');
  assert.equal(stale.client.socket, null);
  assert.equal(stale.client.joined, false);
  assert.ok(stale.timers.find(timer => timer.delay < 10_000),
    'the speculative close must fall back to the reconnect path');
  assert.equal(stale.client.connectionState.status, 'connected',
    'a speculative reconnect must keep the UI on connected while verification is pending');
  assert.equal(stale.dispatched.filter(event => event.type === 'pinvou:web-connection').length,
    statusEventsBefore,
    'speculative recovery must not broadcast a reconnecting/connecting status');
  takeReconnectTimer(stale.timers).callback();
  // Round-5 W1a pin: the speculative spawn itself must not flip the UI to
  // connecting — only a failed verification may.
  assert.equal(stale.client.connectionState.status, 'connected',
    'the speculative spawn itself must not flip the UI to connecting');
  const rejoin = stale.MockWebSocket.instances[1];
  rejoin.open();
  rejoin.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_next',
    desktop_connected: true,
  });
  assert.equal(rejoin.sent.some(message => message.type === 'web_client_join'), true,
    'the speculative reconnect must rejoin');
  assert.equal(stale.client.speculativeReconnect, false,
    'a successful speculative rejoin must clear the speculative flag silently');
  assert.equal(stale.client.connectionState.status, 'connected');
  // Round-11: only a fully verified cycle (join ack + capability snapshot
  // arrival — only a real desktop frame can reach that point) resets the
  // ladder; the ack alone must not. Before the change, the ladder reset at
  // the ack, so a relay minting its own acks zeroed the ladder every cycle
  // and wedged the UI on connected forever. A silent death in an ack-only
  // cycle counts toward the failure cap (still hidden below the cap).
  stale.client.lastInboundAt = Date.now() - 121_000;
  stale.client._livenessCheck();
  assert.equal(stale.client.speculativeFailures, 1,
    'an ack-only silent cycle counts toward the failure cap');
  assert.equal(stale.client.speculativeReconnect, true,
    'the first failed cycle stays under the inclusive cap');
  takeReconnectTimer(stale.timers).callback();
  const reverified = stale.MockWebSocket.instances[2];
  reverified.open();
  reverified.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_next',
    desktop_connected: true,
  });
  reverified.message({
    v: 2,
    type: 'desktop_snapshot',
    stream_epoch: 'epoch_test',
    seq: 0,
    snapshot: { capabilities: { protocol_version: 2, commands: [], events: ['chat:delta'] } },
  });
  assert.equal(stale.client.speculativeFailures, 0,
    'a fully verified cycle (ack + capability snapshot) resets the ladder');
  assert.equal(stale.client.speculativeReconnect, false,
    'a fully verified join clears the speculative flag');
  // A silent death after full verification: a healthy idle cycle (snapshot
  // arrived) must not count a failure.
  stale.client.lastInboundAt = Date.now() - 121_000;
  stale.client._livenessCheck();
  assert.equal(stale.client.speculativeReconnect, true,
    'a silent fully-verified cycle must not escalate');
  // Round-6 discriminator (migrated to the snapshot reset): accumulate one
  // failure, reset it via full verification, then go through an ackless
  // cycle — with the reset working, 1→0→1 stays hidden; without it, 1→2
  // crosses the cap, that cycle must become visible, and the assertion goes
  // red.
  takeReconnectTimer(stale.timers).callback();
  const ackless = stale.MockWebSocket.instances[3];
  ackless.open();
  stale.client.lastInboundAt = Date.now() - 121_000;
  stale.client._livenessCheck();
  assert.equal(ackless.closeCalled, true,
    'the ackless cycle must be closed to route recovery');
  assert.equal(stale.client.speculativeReconnect, true,
    'the first ackless cycle after a verified reset stays under the failure cap');

  const failing = await connectWithCapabilities(['chat:delta'], () => {});
  failing.client.lastInboundAt = Date.now() - 121_000;
  failing.client._livenessCheck();
  assert.equal(failing.client.connectionState.status, 'connected');
  takeReconnectTimer(failing.timers).callback();
  const dead = failing.MockWebSocket.instances[1];
  dead.close();
  assert.equal(failing.client.connectionState.status, 'connecting',
    'a failed speculative verification must escalate to the visible reconnecting status');
  assert.equal(failing.client.speculativeReconnect, false,
    'a failed speculative verification must clear the speculative flag');

  // Round-5 wedge pin: a relay that completes the handshake but never
  // delivers the join ack must not wedge the UI on "connected" forever.
  // Every watchdog close matches its own speculativeSource, so the
  // close-side verification-failed branch can never fire; past the failure
  // cap the reconnect becomes visible instead.
  {
    const wedged = await connectWithCapabilities(['chat:delta'], () => {});
    wedged.client.lastInboundAt = Date.now() - 121_000;
    wedged.client._livenessCheck();
    assert.equal(wedged.client.connectionState.status, 'connected',
      'the first cycle closes a joined socket: still a speculative idle cycle');
    takeReconnectTimer(wedged.timers).callback();
    const second = wedged.MockWebSocket.instances[1];
    second.open();
    wedged.client.lastInboundAt = Date.now() - 121_000;
    wedged.client._livenessCheck();
    assert.equal(second.closeCalled, true,
      'the first ackless cycle must still be closed to route recovery');
    assert.equal(wedged.client.speculativeReconnect, true,
      'the first ackless cycle stays under the failure cap');
    assert.equal(wedged.client.connectionState.status, 'connected',
      'the second cycle is still speculative');
    takeReconnectTimer(wedged.timers).callback();
    const third = wedged.MockWebSocket.instances[2];
    third.open();
    wedged.client.lastInboundAt = Date.now() - 121_000;
    wedged.client._livenessCheck();
    assert.equal(third.closeCalled, true,
      'the second ackless cycle must still be closed to route recovery');
    assert.equal(wedged.client.speculativeReconnect, false,
      'past the failure cap the watchdog reconnect must not be speculative');
    assert.equal(wedged.client.connectionState.status, 'connecting',
      'the capped watchdog reconnect must be visible, not silently repeated');
  }

  // Round-11 wedge pin: a relay that ACKS every join but never delivers the
  // capability snapshot (the ack is minted on the relay's own code path and
  // cannot vouch for the desktop-bound forwarding path) must escalate to
  // visible reconnecting instead of wedging the UI on "connected" forever.
  // Before round-11 the failure count was gated on !joined and the join ack
  // reset it, so every cycle re-zeroed the ladder.
  {
    const frameless = bootClient();
    const framelessClient = frameless.window.PinvouWebClient;
    await framelessClient.policyPromise;
    framelessClient.markFrontendReady();
    await Promise.resolve();
    framelessClient.markStateReady();
    const first = frameless.MockWebSocket.instances[0];
    first.open();
    first.message({
      v: 2,
      type: 'web_client_joined',
      endpoint_id: 'endpoint_test',
      lease_id: 'lease_test',
      desktop_connected: true,
    });
    assert.equal(framelessClient.connectionState.status, 'connected');
    // Cycle 1: the ack-only socket goes silent; the close counts (snapshot
    // still due) and reconnects speculatively.
    framelessClient.lastInboundAt = Date.now() - 121_000;
    framelessClient._livenessCheck();
    assert.equal(first.closeCalled, true);
    assert.equal(framelessClient.speculativeFailures, 1,
      'an acked-but-frameless cycle counts toward the cap');
    assert.equal(framelessClient.connectionState.status, 'connected',
      'the first frameless cycle is still speculative');
    takeReconnectTimer(frameless.timers).callback();
    const second = frameless.MockWebSocket.instances[1];
    second.open();
    second.message({
      v: 2,
      type: 'web_client_joined',
      endpoint_id: 'endpoint_test',
      lease_id: 'lease_next',
      desktop_connected: true,
    });
    // Cycle 2: another ack with no snapshot — the relay re-acked but must
    // not re-zero the ladder; the cycle crosses the cap and becomes visible.
    framelessClient.lastInboundAt = Date.now() - 121_000;
    framelessClient._livenessCheck();
    assert.equal(second.closeCalled, true);
    assert.equal(framelessClient.speculativeFailures, 2,
      'the join ack must not reset the failure ladder');
    assert.equal(framelessClient.speculativeReconnect, false,
      'the second frameless cycle must cross the cap');
    assert.equal(framelessClient.connectionState.status, 'connecting',
      'the frameless relay must escalate to visible reconnecting, not wedge on connected');
  }

  const fresh = await connectWithCapabilities(['chat:delta'], () => {});
  fresh.client.lastInboundAt = Date.now();
  fresh.client._livenessCheck();
  assert.equal(fresh.socket.closeCalled, false,
    'recent inbound traffic must keep a healthy socket');

  const idle = bootClient();
  const idleClient = idle.window.PinvouWebClient;
  await idleClient.policyPromise;
  idleClient._livenessCheck();
  assert.equal(idle.MockWebSocket.instances[0].closeCalled, false,
    'a socket that is not OPEN must be left to the connect path');
  assert.equal(idle.timers.some(timer => timer.delay < 10_000), false);
}

// The watchdog interval must be genuinely registered and drive _livenessCheck,
// and rejoining must not register a second one.
{
  const wired = await connectWithCapabilities(['chat:delta'], () => {});
  const watchdog = wired.timers.find(timer => timer.delay === 30_000);
  assert.ok(watchdog, 'the liveness watchdog interval must be registered');
  wired.client.lastInboundAt = Date.now() - 121_000;
  watchdog.callback();
  assert.equal(wired.socket.closeCalled, true,
    'the registered interval must drive the liveness check');
  takeReconnectTimer(wired.timers).callback();
  const next = wired.MockWebSocket.instances[1];
  next.open();
  next.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_next',
    desktop_connected: true,
  });
  assert.equal(wired.timers.filter(timer => timer.delay === 30_000).length, 1,
    'rejoining must not register a second watchdog');
}

// Network recovery (online event): reconnect immediately when disconnected and
// clear the backoff; treat an OPEN connection as suspicious and reconnect.
{
  const offline = bootClient();
  const offlineClient = offline.window.PinvouWebClient;
  await offlineClient.policyPromise;
  offline.MockWebSocket.instances[0].close();
  const backoff = offline.timers.find(timer => timer.delay < 10_000);
  assert.ok(backoff);
  offlineClient.reconnectAttempt = 3;
  offline.window.emit('online');
  assert.equal(offline.timers.includes(backoff), false,
    'stale backoff must be discarded when connectivity returns');
  assert.equal(offlineClient.reconnectAttempt, 0,
    'backoff must reset on a network change');
  assert.equal(offline.MockWebSocket.instances.length, 2,
    'connectivity restored must reconnect immediately');

  const live = await connectWithCapabilities(['chat:delta'], () => {});
  const broadcastsBefore = live.dispatched.filter(event => event.type === 'pinvou:web-connection').length;
  live.window.emit('online');
  assert.equal(live.socket.closeCalled, true,
    'an OPEN socket must be treated as suspicion when the OS reports a network change');
  assert.ok(live.timers.find(timer => timer.delay < 10_000),
    'the suspicion close must schedule a reconnect');
  assert.equal(live.client.connectionState.status, 'connected',
    'the online-triggered suspicion close is speculative: the UI stays on connected');
  assert.equal(live.dispatched.filter(event => event.type === 'pinvou:web-connection').length,
    broadcastsBefore,
    'the online-triggered speculative close must not broadcast a connecting status');
  // A failed speculative verification still escalates.
  takeReconnectTimer(live.timers).callback();
  live.MockWebSocket.instances[1].close();
  assert.equal(live.client.connectionState.status, 'connecting',
    'a failed speculative verification must escalate to the visible status');
}

// Round-8 review: the online path's speculative close (the block inside
// handleNetworkBecomeOnline that is a byte-level twin of the watchdog's) must
// likewise count a failure for a socket that never received its join ack.
// Without that line, OS network flips repeatedly interrupting an ackless
// socket keep the failure count below the cap, the reconnect stays speculative
// forever, and the UI stays on connected forever — same sin as the round-5
// watchdog wedged-relay scenario.
{
  const harness = bootClient();
  const client = harness.window.PinvouWebClient;
  await client.policyPromise;
  const ackless = harness.MockWebSocket.instances[0];
  ackless.open();
  harness.window.emit('online');
  assert.equal(ackless.closeCalled, true,
    'the online-triggered suspicion close must fire on an ackless socket');
  assert.equal(client.speculativeFailures, 1,
    'the online close must count the ackless cycle toward the failure cap');
  assert.equal(client.speculativeReconnect, true,
    'the first ackless cycle has only reached the cap and stays speculative');
  takeReconnectTimer(harness.timers).callback();
  const second = harness.MockWebSocket.instances[1];
  second.open();
  client.lastInboundAt = Date.now() - 121_000;
  client._livenessCheck();
  assert.equal(second.closeCalled, true,
    'the further ackless cycle must still be closed to route recovery');
  assert.equal(client.speculativeReconnect, false,
    'a further ackless cycle past the cap must make the online-path reconnect visible');
  assert.equal(client.connectionState.status, 'connecting',
    'the capped online-path reconnect must surface the visible reconnecting state');
}

// The watchdog must rescue a connect wedged in CONNECTING: a CONNECTING socket
// past the wedge window is aborted into the reconnect funnel; a young
// CONNECTING socket inside the window is untouched.
{
  const wedged = bootClient();
  const wedgedClient = wedged.window.PinvouWebClient;
  await wedgedClient.policyPromise;
  const wedgedSocket = wedged.MockWebSocket.instances[0];
  assert.equal(wedgedSocket.readyState, 0, 'fixture starts CONNECTING');
  wedgedClient.connectStartedAt = Date.now() - 46_000;
  wedgedClient._livenessCheck();
  assert.equal(wedgedSocket.closeCalled, true,
    'a CONNECTING socket wedged past the window must be abandoned by the watchdog');

  const young = bootClient();
  const youngClient = young.window.PinvouWebClient;
  await youngClient.policyPromise;
  youngClient.connectStartedAt = Date.now();
  youngClient._livenessCheck();
  assert.equal(young.MockWebSocket.instances[0].closeCalled, false,
    'a young CONNECTING socket must keep its full wedge window');
}

// Round-15 review: CLOSING used to be ignored by the watchdog forever — a
// speculative close on a silently dropped link may never receive a close
// event, leaving "connected but dead" wedged beyond the client's own
// constants. A CLOSING socket past the same wedge window must be dropped and
// routed through the backoff reconnect; a young CLOSING socket inside the
// window is unaffected; OPEN sockets are unaffected.
{
  const stuck = bootClient();
  const stuckClient = stuck.window.PinvouWebClient;
  await stuckClient.policyPromise;
  const stuckSocket = stuck.MockWebSocket.instances[0];
  stuckSocket.open();
  stuckClient.lastInboundAt = Date.now();
  stuckSocket.readyState = stuck.MockWebSocket.CLOSING;

  stuckClient._livenessCheck();
  assert.equal(stuck.MockWebSocket.instances.length, 1,
    'the first observation of a CLOSING socket only starts its wedge clock');
  assert.equal(stuckClient.closingStartedAt > 0, true,
    'the wedge clock must be recorded for the CLOSING socket');

  stuckClient.closingStartedAt = Date.now() - 46_000;
  stuckClient._livenessCheck();
  assert.equal(stuckClient.socket, null,
    'a CLOSING socket wedged past the window must be dropped');
  takeReconnectTimer(stuck.timers).callback();
  assert.equal(stuck.MockWebSocket.instances.length, 2,
    'the rescue must funnel through the ordinary backoff reconnect');
  assert.notEqual(stuck.MockWebSocket.instances[1], stuckSocket,
    'the dead socket must not be reused');

  const young = bootClient();
  const youngClosing = young.window.PinvouWebClient;
  await youngClosing.policyPromise;
  const youngSocket = young.MockWebSocket.instances[0];
  youngSocket.open();
  youngClosing.lastInboundAt = Date.now();
  youngSocket.readyState = young.MockWebSocket.CLOSING;
  youngClosing.closingStartedAt = Date.now();
  youngClosing._livenessCheck();
  assert.equal(young.MockWebSocket.instances.length, 1,
    'a young CLOSING socket must keep its full wedge window');
  // Round-19 review: the instance-count assertion above has no teeth — a
  // premature rescue only detaches the socket reference and schedules a
  // reconnect timer, no new instance is spawned until the timer fires, and
  // this fixture never fires it, so deleting the window check kept this block
  // green (probe evidence). Pin the real payload directly: the socket
  // reference must be untouched and no reconnect timer may be scheduled.
  assert.equal(youngClosing.socket, youngSocket,
    'a young CLOSING socket must keep its socket reference');
  assert.equal(young.timers.some(timer => timer.delay < 10_000), false,
    'a young CLOSING socket must not trigger a reconnect');
}

// Round-20 review: connect()'s closingStartedAt reset used to be a mutation
// hole — every young-CLOSING fixture hand-injected the field before the
// check, so deleting the reset (all three reset sites deleted together) kept
// the whole suite green: a stale wedge clock would make the new socket's
// first CLOSING observation look "already past the window" and rescue it
// instantly, denying the new socket its own full wedge window. This fixture
// lets the production code stamp the timestamps itself (no injection):
// S1 wedges → first observation stamps the clock → clock advances past the
// window → the close event delivers normally and detaches → the reconnect
// timer fires S2 → S2 wedges → the very next watchdog tick must restamp from
// "the new socket's first observation" and return, not rescue immediately off
// the stale stamp. (The speculativeClose/rescue resets are redundant defenses
// behind connect's reset; this pin holds the load-bearing site at connect, so
// a joint-deletion mutation's remaining sites go red too.)
{
  const env = bootClient();
  const client = env.window.PinvouWebClient;
  await client.policyPromise;
  const s1 = env.MockWebSocket.instances[0];
  s1.open();
  client.lastInboundAt = Date.now();
  s1.readyState = 2; // CLOSING: wedged shape; the close event is never delivered
  client._livenessCheck();
  const stampedAt = client.closingStartedAt;
  assert.notEqual(stampedAt, null,
    'the first CLOSING observation must stamp the wedge clock');
  env.window.__clock.advance(46_000);
  s1.close(); // the close event delivers normally here: detach the socket, take the ordinary backoff reconnect
  const reconnect = takeReconnectTimer(env.timers);
  reconnect.callback();
  assert.equal(env.MockWebSocket.instances.length, 2,
    'the reconnect must spawn the successor');
  const s2 = env.MockWebSocket.instances[1];
  assert.equal(client.socket, s2, 'the successor must be current');
  assert.equal(client.closingStartedAt, null,
    'connect() must give the successor a fresh wedge clock');
  s2.readyState = 2; // the successor wedges the same way
  client._livenessCheck();
  assert.equal(client.socket, s2,
    'a fresh successor must keep its full wedge window (a stale clock must not rescue it instantly)');
  assert.equal(client.closingStartedAt > stampedAt, true,
    'the fresh observation must restamp, not inherit the predecessor clock');
  assert.equal(env.timers.some(timer => timer.delay < 10_000), false,
    'no instant rescue reconnect may be scheduled for the fresh successor');
}

// Round-20 review: the post-gate liveness-evidence update previously had no
// pin — moving the lastInboundAt update ahead of the identity gate (the
// mutation where a stale frame refreshes the watchdog clock) kept the whole
// suite green, and an in-flight frame on a detached socket could postpone the
// silence judgment by a full silence window. Pin both directions: a detached
// socket's frame must not refresh; a legitimate frame on the current socket
// must still refresh (the positive leg gives "delete the update" mutations a
// signal too; the negative leg is this pin's payload).
{
  const env = bootClient();
  const client = env.window.PinvouWebClient;
  await client.policyPromise;
  const dead = env.MockWebSocket.instances[0];
  dead.open();
  client.lastInboundAt = Date.now();
  dead.readyState = 2;
  client._livenessCheck();
  env.window.__clock.advance(46_000);
  client._livenessCheck(); // rescue: the socket reference is cleared
  assert.equal(client.socket, null, 'setup: the wedged socket must be rescued');
  const t0 = Date.now() - 5;
  client.lastInboundAt = t0;
  dead.message({ v: 2, type: 'event', event: 'chat:delta', stream_epoch: 'epoch_dead', seq: 1, payload: { text: 'stale' } });
  assert.equal(client.lastInboundAt, t0,
    'a gated stale frame must not refresh liveness evidence');
  const reconnect = takeReconnectTimer(env.timers);
  reconnect.callback();
  const successor = env.MockWebSocket.instances[1];
  assert.equal(client.socket, successor, 'setup: the successor must be current');
  successor.message({ v: 2, type: 'event', event: 'chat:delta', stream_epoch: 'epoch_live', seq: 1, payload: { text: 'live' } });
  assert.ok(client.lastInboundAt > t0,
    'a current-socket frame must refresh liveness evidence');
}

// Round-20 review: a non-TTL endpoint_revoked bypasses the identity gate
// (a link-level fact; see the comment beside the gate in bootstrap.js).
// Pin both directions: (a) a real revocation frame on a detached socket must
// reach the terminal state — the gate used to drop the whole frame, so when
// its only copy landed inside the detach window the revocation degenerated
// into the endpoint_not_found "desktop offline" retry loop (that code is
// deliberately classified as transient, leaving the revocation card
// unreachable); (b) an expired TTL-arm copy stays gated — letting it through
// would knock later healthy cycles back to desktop_offline (a fresh copy
// after the rejoin will reach the current socket anyway).
{
  const env = bootClient();
  const client = env.window.PinvouWebClient;
  await client.policyPromise;
  const dead = env.MockWebSocket.instances[0];
  dead.open();
  client.lastInboundAt = Date.now();
  dead.readyState = 2;
  client._livenessCheck();
  env.window.__clock.advance(46_000);
  client._livenessCheck(); // rescue-detach: the socket reference is cleared
  dead.message({ v: 2, type: 'endpoint_revoked', reason: 'manual_revoke' });
  assert.equal(client.closedPermanently, true,
    'a real revoke must surface even when its only copy arrives on a detached socket');
  assert.equal(env.timers.some(timer => timer.delay < 10_000), false,
    'the revoked terminal state must not schedule reconnects');
}
{
  const env = bootClient();
  const client = env.window.PinvouWebClient;
  await client.policyPromise;
  const dead = env.MockWebSocket.instances[0];
  dead.open();
  client.lastInboundAt = Date.now();
  dead.readyState = 2;
  client._livenessCheck();
  env.window.__clock.advance(46_000);
  client._livenessCheck();
  const statusBefore = client.connectionState.status;
  dead.message({ v: 2, type: 'endpoint_revoked', reason: 'desktop_offline_ttl_expired' });
  assert.equal(client.closedPermanently, false,
    'a stale TTL-expiry copy must stay gated');
  assert.equal(client.connectionState.status, statusBefore,
    'the gated TTL copy must not flip the cycle state');
}

// Round-22 review: once the terminal state is established (closedPermanently)
// the bypass must stop — closePermanently is idempotent state-wise, but a late
// non-TTL revocation copy would re-enter it: re-dispatching connection events
// and overwriting the established terminal card (endpoint_replaced's "opened
// in another browser") with the revocation copy. One guard line; deleting the
// guard turns this fixture red (both the re-dispatch and the card flip are
// asserted).
{
  const env = bootClient();
  const client = env.window.PinvouWebClient;
  await client.policyPromise;
  const dead = env.MockWebSocket.instances[0];
  dead.open();
  client.lastInboundAt = Date.now();
  dead.message({ v: 2, type: 'endpoint_replaced' });
  assert.equal(client.closedPermanently, true,
    'fixture: the replaced terminal card must be established first');
  const eventsBefore = env.dispatched.filter(event => event.type === 'pinvou:web-connection').length;
  const statusAfterTerminal = client.connectionState.status;
  dead.message({ v: 2, type: 'endpoint_revoked', reason: 'manual_revoke' });
  assert.equal(
    env.dispatched.filter(event => event.type === 'pinvou:web-connection').length,
    eventsBefore,
    'a late non-TTL revoke must not re-dispatch the terminal connection event');
  assert.equal(client.connectionState.status, statusAfterTerminal,
    'a late non-TTL revoke must not overwrite the established terminal card');
}

// Round-16 review MAJOR-1: after the rescue detaches the socket reference, its
// close event may only arrive after the successor cycle has finished joining
// and negotiating. A late close must not clear the current cycle's flags, must
// not schedule another reconnect, and above all must not knock a healthy
// successor back to the unnegotiated state (otherwise the next inbound event
// hits !desktopCapabilitiesReady and processRemoteEvent permanently shuts the
// door as incompatible_desktop — probe end-to-end reproduction from before the
// fix; the identity gate is the only barrier).
{
  const late = bootClient();
  const lateClient = late.window.PinvouWebClient;
  await lateClient.policyPromise;
  const events = [];
  await lateClient.listen('chat:delta', (event) => events.push(event));
  lateClient.markFrontendReady();
  await Promise.resolve();
  lateClient.markStateReady();

  const deadSocket = late.MockWebSocket.instances[0];
  deadSocket.open();
  deadSocket.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_test',
    desktop_connected: true,
  });
  lateClient.lastInboundAt = Date.now();

  // Silent link drop: the socket enters CLOSING and its close event never
  // arrives, so the watchdog rescues it.
  deadSocket.readyState = late.MockWebSocket.CLOSING;
  lateClient._livenessCheck();
  lateClient.closingStartedAt = Date.now() - 46_000;
  lateClient._livenessCheck();
  assert.equal(lateClient.socket, null, 'the wedged socket must be rescued');
  takeReconnectTimer(late.timers).callback();

  // The successor connects and completes the full negotiation (joined +
  // capability snapshot).
  const successor = late.MockWebSocket.instances[1];
  successor.open();
  successor.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_test_b',
    desktop_connected: true,
  });
  successor.message({
    v: 2,
    type: 'desktop_snapshot',
    stream_epoch: 'epoch_b',
    seq: 0,
    snapshot: {
      capabilities: { protocol_version: 2, commands: [], events: ['chat:delta'] },
    },
  });
  assert.equal(lateClient.joined, true, 'the successor cycle must be fully joined');
  assert.equal(lateClient.desktopCapabilitiesReady, true,
    'the successor cycle must have negotiated capabilities');

  // The old socket's close event only arrives now: the identity gate must
  // ignore it wholesale.
  deadSocket.emit('close', {});
  assert.equal(lateClient.socket, successor,
    'a late close of a rescued socket must not detach its successor');
  assert.equal(lateClient.joined, true,
    'a late close of a rescued socket must not clobber the successor join state');
  assert.equal(lateClient.desktopCapabilitiesReady, true,
    'a late close of a rescued socket must not wipe negotiated capabilities');
  assert.equal(lateClient.leaseId, 'lease_test_b',
    'a late close of a rescued socket must not clear the successor lease');
  assert.equal(late.timers.filter(timer => timer.delay < 10_000).length, 0,
    'a late close of a rescued socket must not schedule a second reconnect');

  // A late error is likewise noise: the identity gate must ignore it
  // wholesale — no speculative failure counted, no connected flipped back to
  // connecting (deleting the error handler's identity gate turns the status
  // flip assertion below red; the gate previously had no mutation coverage).
  // In this fixture the count assertion is only sensitive to a coarse mutation
  // (unconditional +1): noteSpeculativeFailure only counts inside a
  // speculative window, and this fixture's rescue takes the CLOSING arm
  // without setting speculativeReconnect — making the count assertion
  // sensitive to gate deletion would require constructing the rare race of "a
  // non-source late error colliding with an active speculative window", which
  // has no fixture coverage (disclosed).
  const failuresBeforeLateError = lateClient.speculativeFailures;
  deadSocket.emit('error', {});
  assert.equal(lateClient.speculativeFailures, failuresBeforeLateError,
    'a late error of a rescued socket must not count a speculative failure');
  assert.equal(lateClient.connectionState.status, 'connected',
    'a late error of a rescued socket must not flip the UI back to connecting');

  // The successor is still healthy: inbound events dispatch normally, with no
  // incompatible_desktop permanent shutdown.
  successor.message({
    v: 2,
    type: 'event',
    event: 'chat:delta',
    stream_epoch: 'epoch_b',
    seq: 1,
    payload: { text: 'after-late-close' },
  });
  await lateClient.eventDispatch;
  assert.equal(lateClient.closedPermanently, false,
    'a healthy successor must never be closed as incompatible_desktop by a stale close');
  assert.equal(events.length, 1, 'the successor must keep dispatching inbound events');
}

// Round-16 review: CLOSED wedges exactly like CLOSING — the close event may
// never be delivered (the browser can reclaim the socket without delivering),
// so past the same wedge window it must be rescued too, not ignored forever.
{
  const closed = bootClient();
  const closedClient = closed.window.PinvouWebClient;
  await closedClient.policyPromise;
  const closedSocket = closed.MockWebSocket.instances[0];
  closedSocket.open();
  closedClient.lastInboundAt = Date.now();
  closedSocket.readyState = closed.MockWebSocket.CLOSED;

  closedClient._livenessCheck();
  assert.equal(closedClient.closingStartedAt > 0, true,
    'the wedge clock must be recorded for a CLOSED socket too');
  closedClient.closingStartedAt = Date.now() - 46_000;
  closedClient._livenessCheck();
  assert.equal(closedClient.socket, null,
    'a CLOSED socket wedged past the window must be dropped like CLOSING');
  takeReconnectTimer(closed.timers).callback();
  assert.equal(closed.MockWebSocket.instances.length, 2,
    'the CLOSED rescue must funnel through the ordinary backoff reconnect');
}

// Round-16 follow-up review N1: the event-failure path detaches the socket
// deliberately (the catch handler sets this.socket = null then closes), so its
// close event is judged noise by the identity gate — the reconnect must be
// scheduled by the catch handler itself; delegating to the close event would
// leave a zombie "connected with no transport" UI whenever close() delivered
// normally (probe end-to-end reproduction from before the fix, and the
// watchdog no longer backstops a missing !this.socket either). Late
// close/error events still must not disturb the successor cycle.
{
  const failed = bootClient();
  const failedClient = failed.window.PinvouWebClient;
  await failedClient.policyPromise;
  const events = [];
  await failedClient.listen('chat:delta', (event) => {
    events.push(event);
    throw new Error('listener failed');
  });
  failedClient.markFrontendReady();
  await Promise.resolve();
  failedClient.markStateReady();

  const deadSocket = failed.MockWebSocket.instances[0];
  deadSocket.open();
  deadSocket.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_test',
    desktop_connected: true,
  });
  deadSocket.message({
    v: 2,
    type: 'desktop_snapshot',
    stream_epoch: 'epoch_test',
    seq: 0,
    snapshot: {
      capabilities: { protocol_version: 2, commands: [], events: ['chat:delta'] },
    },
  });
  assert.equal(failedClient.connectionState.status, 'connected');

  // Event-processing failure: the socket is detached, so the reconnect must be
  // scheduled on the spot, not left waiting for a close event the identity
  // gate will swallow.
  deadSocket.message({
    v: 2,
    type: 'event',
    event: 'chat:delta',
    stream_epoch: 'epoch_test',
    seq: 1,
    payload: { text: 'poison' },
  });
  await failedClient.eventDispatch;
  assert.equal(deadSocket.closeCalled, true,
    'the failed socket must be detached immediately');
  assert.equal(failedClient.socket, null,
    'the failed socket must not stay registered');
  assert.equal(failed.timers.filter(timer => timer.delay < 10_000).length, 1,
    'the catch path must schedule its own reconnect (the gated close event cannot)');
  const reconnect = takeReconnectTimer(failed.timers);
  reconnect.callback();

  // The successor connects and completes negotiation; the dead socket's
  // close/error only arrive now: they must be ignored wholesale, neither
  // smearing the successor state nor appending a second reconnect timer.
  const successor = failed.MockWebSocket.instances[1];
  successor.open();
  successor.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_test_b',
    desktop_connected: true,
  });
  successor.message({
    v: 2,
    type: 'desktop_snapshot',
    stream_epoch: 'epoch_b',
    seq: 0,
    snapshot: {
      capabilities: { protocol_version: 2, commands: [], events: ['chat:delta'] },
    },
  });
  assert.equal(failedClient.joined, true, 'the successor cycle must be fully joined');
  deadSocket.emit('close', {});
  deadSocket.emit('error', {});
  assert.equal(failedClient.socket, successor,
    'the late close/error of a replay-detached socket must not detach its successor');
  assert.equal(failedClient.joined, true, 'the late events must not clobber the join state');
  assert.equal(failedClient.desktopCapabilitiesReady, true,
    'the late events must not wipe negotiated capabilities');
  assert.equal(failedClient.connectionState.status, 'connected',
    'the late error must not flip the healthy successor back to connecting');
  assert.equal(failed.timers.filter(timer => timer.delay < 10_000).length, 0,
    'the late events must not schedule a second reconnect');
}

// Round-16 follow-up review M8: the error gate previously had no test pinning
// it (deleting the error gate alone kept the whole suite green). A late error
// from a rescue-detached socket must not knock a healthy successor back to
// connecting and must not pollute the speculative failure count — the same
// identity semantics as the close gate.
{
  const late = bootClient();
  const lateClient = late.window.PinvouWebClient;
  await lateClient.policyPromise;
  const deadSocket = late.MockWebSocket.instances[0];
  deadSocket.open();
  deadSocket.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_test',
    desktop_connected: true,
  });
  lateClient.lastInboundAt = Date.now();

  deadSocket.readyState = late.MockWebSocket.CLOSING;
  lateClient._livenessCheck();
  lateClient.closingStartedAt = Date.now() - 46_000;
  lateClient._livenessCheck();
  assert.equal(lateClient.socket, null, 'the wedged socket must be rescued');
  takeReconnectTimer(late.timers).callback();

  const successor = late.MockWebSocket.instances[1];
  successor.open();
  successor.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_test_b',
    desktop_connected: true,
  });
  assert.equal(lateClient.connectionState.status, 'connected');
  assert.equal(lateClient.joined, true);

  // Round-25 review MINOR: the three late-error assertions here (socket kept /
  // status not flipped / no reconnect scheduled) duplicate the MAJOR-1 block's
  // late-error assertions verbatim — the error gate's deletion mutation goes
  // red at the earlier assertion, so this block adds no new mutation coverage
  // for the gate itself. After deduplication only this block's unique payload
  // remains: with the gate in place the speculative count stays unpolluted —
  // a genuine natural death immediately afterwards still counts under the
  // original semantics.
  deadSocket.emit('error', {});
  successor.close();
  takeReconnectTimer(late.timers);
  assert.equal(late.timers.some(timer => timer.delay < 10_000), false,
    'a real death after the gated late error must still reach the reconnect funnel (exactly one timer)');
}

// Round-28 review M1: the catch path's identity gate (the
// closedPermanently || this.socket !== sourceSocket check at the top of the
// event-failure handler) previously had no fixture at all — narrowing it to
// closedPermanently-only kept the whole suite green. This fixture builds a
// genuinely detectable scenario: the poisoned event passes the :623 frame gate
// and the handleRemoteEvent gate while the socket is still current (dispatch
// already past the gates), the listener detaches the socket via a rescue
// mid-execution (simulating the rescue/reconnect race), and then throws — the
// catch's identity gate must judge "the event belongs to a socket that has
// already been swapped out" and return wholesale: it must not wipe the cycle
// flags and must not append a second reconnect. Under the narrowing mutation
// the flag-reset assertion goes red (the reconnect timer self-guards, so
// timer counts alone cannot tell the difference).
{
  const midSwap = bootClient();
  const midClient = midSwap.window.PinvouWebClient;
  await midClient.policyPromise;
  await midClient.listen('chat:delta', () => {
    // Swap the socket mid-dispatch: CLOSING wedge + past-window rescue, with
    // the reconnect timer scheduled on the spot.
    const dying = midClient.socket;
    dying.readyState = midSwap.MockWebSocket.CLOSING;
    midClient.closingStartedAt = Date.now() - 46_000;
    midClient._livenessCheck();
    throw new Error('listener failed after mid-dispatch swap');
  });
  midClient.markFrontendReady();
  await Promise.resolve();
  midClient.markStateReady();

  const swapSocket = midSwap.MockWebSocket.instances[0];
  swapSocket.open();
  swapSocket.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_test',
    desktop_connected: true,
  });
  swapSocket.message({
    v: 2,
    type: 'desktop_snapshot',
    stream_epoch: 'epoch_swap',
    seq: 0,
    snapshot: {
      capabilities: { protocol_version: 2, commands: [], events: ['chat:delta'] },
    },
  });
  assert.equal(midClient.joined, true, 'the cycle must be fully joined before the poison event');
  assert.equal(midClient.desktopCapabilitiesReady, true,
    'capabilities must be negotiated before the poison event');

  // The poisoned event during dispatch: the listener swaps the socket first,
  // then throws.
  swapSocket.message({
    v: 2,
    type: 'event',
    event: 'chat:delta',
    stream_epoch: 'epoch_swap',
    seq: 1,
    payload: { text: 'mid-swap poison' },
  });
  await midClient.eventDispatch;

  // The catch's identity gate must return wholesale: the successor cycle's
  // flags stay untouched, and the reconnect funnel holds only the one timer
  // the rescue scheduled itself.
  assert.equal(midClient.joined, true,
    'the catch path must not reset the cycle flags when the event belongs to a swapped-out socket');
  assert.equal(midClient.desktopCapabilitiesReady, true,
    'the catch path must not wipe the post-swap cycle negotiated capabilities');
  assert.equal(midClient.leaseId, 'lease_test',
    'the catch path must not clear the post-swap cycle lease');
  assert.equal(midClient.socket, null, 'the rescued socket must stay detached');
  assert.equal(midSwap.timers.filter(timer => timer.delay < 10_000).length, 1,
    'exactly the rescue reconnect timer must exist — the catch path must not add a second');
}

// Round-18 review: in an all-rescue topology (every close wedged and never
// delivered) the close handler's verification branch can never run — the
// rescue must carry that same branch along. The death of a successor rescued
// inside a speculative cycle (not the speculativeSource; its join ack has not
// arrived) must count toward the failure ladder and make the recovery visible;
// rescuing the speculative source socket itself is still "the suspicion being
// verified", keeping the original no-count contract (verbatim-identical to
// the close handler's identity contract).
{
  const chained = bootClient();
  const chainedClient = chained.window.PinvouWebClient;
  await chainedClient.policyPromise;
  const source = chained.MockWebSocket.instances[0];
  source.open();
  source.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_a',
    desktop_connected: true,
  });
  // Speculative window: the previous socket was speculatively closed by the
  // watchdog (the speculativeSource) and the successor is issued by the
  // speculative reconnect — the copy is suppressed by the speculative
  // semantics and the UI stays on connected.
  chainedClient.speculativeReconnect = true;
  chainedClient.speculativeFailures = 0;
  chainedClient.speculativeSource = source;
  chainedClient.socket = null;
  chainedClient.scheduleReconnect('连接已断开，正在重试…');
  assert.equal(chainedClient.connectionState.status, 'connected',
    'a speculative reconnect must keep the UI on connected');
  takeReconnectTimer(chained.timers).callback();
  const successor = chained.MockWebSocket.instances[1];
  assert.equal(chainedClient.socket, successor);
  successor.open();
  chainedClient.lastInboundAt = Date.now();
  // The successor also wedges in CLOSING (its close event is never delivered);
  // the watchdog rescues it:
  successor.readyState = chained.MockWebSocket.CLOSING;
  chainedClient._livenessCheck();
  chainedClient.closingStartedAt = Date.now() - 46_000;
  chainedClient._livenessCheck();
  assert.equal(chainedClient.socket, null, 'the wedged successor must be rescued');
  assert.equal(chainedClient.speculativeFailures, 1,
    'a rescued successor whose join ack never arrived counts as a verification failure');
  assert.equal(chainedClient.speculativeReconnect, false,
    'the ladder must stop hiding recovery after the failed verification');
  assert.equal(chainedClient.connectionState.status, 'connecting',
    'the rescue reconnect must become visible once verification failed');
  takeReconnectTimer(chained.timers).callback();

  // Contrast: rescuing the speculative source socket itself still does not
  // count and stays suppressed (the original contract preserved).
  const forgiven = bootClient();
  const forgivenClient = forgiven.window.PinvouWebClient;
  await forgivenClient.policyPromise;
  const forgivenSocket = forgiven.MockWebSocket.instances[0];
  forgivenSocket.open();
  forgivenSocket.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_a',
    desktop_connected: true,
  });
  forgivenClient.joined = true;
  forgivenClient.speculativeReconnect = true;
  forgivenClient.speculativeFailures = 0;
  forgivenClient.speculativeSource = forgivenSocket;
  forgivenClient.lastInboundAt = Date.now();
  forgivenSocket.readyState = forgiven.MockWebSocket.CLOSING;
  forgivenClient._livenessCheck();
  forgivenClient.closingStartedAt = Date.now() - 46_000;
  forgivenClient._livenessCheck();
  assert.equal(forgivenClient.socket, null, 'the speculative source is still rescued');
  assert.equal(forgivenClient.speculativeFailures, 0,
    'rescuing the speculative source itself is the suspicion being verified, not a failed one');
  assert.equal(forgivenClient.speculativeReconnect, true,
    'the source rescue must keep the speculative window');
  assert.equal(forgivenClient.connectionState.status, 'connected',
    'the source rescue reconnect must stay suppressed');
}

// Round-19 review: when an event failure inside a speculative cycle goes
// through the catch path, it must carry the same verification branch as the
// rescue / close handler — a proven processing failure of a non-speculative-
// source socket counts toward the ladder and lifts the copy suppression; the
// speculative source itself is still forgiven (window kept, count 0, copy
// stays suppressed). Without that branch, a proven failure is suppressed as
// "suspected" for one cycle (a probe once classified this as a UI-honesty
// gap).
{
  const spec = bootClient();
  const specClient = spec.window.PinvouWebClient;
  await specClient.policyPromise;
  const prev = spec.MockWebSocket.instances[0];
  prev.open();
  prev.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_a',
    desktop_connected: true,
  });
  // Speculative window: the previous cycle was speculatively closed and the
  // successor was issued by the speculative reconnect, not yet joined.
  specClient.speculativeReconnect = true;
  specClient.speculativeFailures = 0;
  specClient.speculativeSource = prev;
  specClient.socket = null;
  specClient.scheduleReconnect('连接已断开，正在重试…');
  takeReconnectTimer(spec.timers).callback();
  const successor = spec.MockWebSocket.instances[1];
  assert.equal(specClient.socket, successor);
  successor.open();
  // The successor has not joined and state_ready is not marked —
  // processRemoteEvent throws stateNotReady on the event frame, and the catch
  // path takes over recovery.
  successor.message({
    v: 2,
    type: 'event',
    event: 'chat:delta',
    stream_epoch: 'epoch_test',
    seq: 1,
    payload: { text: 'x' },
  });
  await specClient.eventDispatch;
  assert.equal(specClient.speculativeFailures, 1,
    'an event failure on a speculative successor counts as a verification failure');
  assert.equal(specClient.speculativeReconnect, false,
    'the speculative window must close once the failure is proven');
  assert.equal(specClient.socket, null,
    'the catch path must detach the failed socket');
  assert.equal(spec.timers.some(timer => timer.delay < 10_000), true,
    'the catch path must schedule the replay reconnect');
  assert.equal(specClient.connectionState.status, 'connecting',
    'a proven failure must make the reconnect visible, not stay suppressed');
}
{
  // Contrast: an event failure on the speculative source itself is still
  // forgiven under the original contract.
  const forg = bootClient();
  const forgClient = forg.window.PinvouWebClient;
  await forgClient.policyPromise;
  const src = forg.MockWebSocket.instances[0];
  src.open();
  src.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_a',
    desktop_connected: true,
  });
  forgClient.joined = true;
  forgClient.speculativeReconnect = true;
  forgClient.speculativeFailures = 0;
  forgClient.speculativeSource = src;
  forgClient.lastInboundAt = Date.now();
  src.message({
    v: 2,
    type: 'event',
    event: 'chat:delta',
    stream_epoch: 'epoch_test',
    seq: 1,
    payload: { text: 'x' },
  });
  await forgClient.eventDispatch;
  assert.equal(forgClient.speculativeFailures, 0,
    'the speculative source stays forgiven on the event-failure path too');
  assert.equal(forgClient.speculativeReconnect, true,
    'the source failure keeps the speculative window');
  assert.equal(forgClient.connectionState.status, 'connected',
    'the source failure reconnect stays suppressed');
}

// Round-18 review (the Round-20 review repaired this comment, which a round-19
// insertion had cut in half): cycle-state frames previously had no identity
// gate — a join ack still in flight on a rescue-detached socket would arrive
// into the successor cycle and knock the already-negotiated successor back to
// the unnegotiated state (the next inbound event hits !desktopCapabilitiesReady
// → incompatible_desktop permanent shutdown, same family as round-16 MAJOR-1);
// a late error/endpoint_replaced would this.socket.close()/closePermanently a
// healthy successor (the non-TTL arm of endpoint_revoked already bypasses as a
// deliberate exception; see the same-named comment in bootstrap.js). Frames
// from an old socket must be dropped wholesale.
{
  const stale = bootClient();
  const staleClient = stale.window.PinvouWebClient;
  await staleClient.policyPromise;
  const events = [];
  await staleClient.listen('chat:delta', (event) => events.push(event));
  staleClient.markFrontendReady();
  await Promise.resolve();
  staleClient.markStateReady();

  const deadSocket = stale.MockWebSocket.instances[0];
  deadSocket.open();
  deadSocket.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_a',
    desktop_connected: true,
  });
  deadSocket.message({
    v: 2,
    type: 'desktop_snapshot',
    stream_epoch: 'epoch_a',
    seq: 4,
    snapshot: { capabilities: { protocol_version: 2, commands: [], events: ['chat:delta'] } },
  });
  assert.equal(staleClient.desktopCapabilitiesReady, true, 'cycle A must complete its handshake');

  // Cycle A's socket wedges → rescue → the successor (B) connects and
  // completes negotiation:
  deadSocket.readyState = stale.MockWebSocket.CLOSING;
  staleClient._livenessCheck();
  staleClient.closingStartedAt = Date.now() - 46_000;
  staleClient._livenessCheck();
  takeReconnectTimer(stale.timers).callback();
  const successor = stale.MockWebSocket.instances[1];
  successor.open();
  successor.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_b',
    desktop_connected: true,
  });
  successor.message({
    v: 2,
    type: 'desktop_snapshot',
    stream_epoch: 'epoch_b',
    seq: 5,
    snapshot: { capabilities: { protocol_version: 2, commands: [], events: ['chat:delta'] } },
  });
  assert.equal(staleClient.desktopCapabilitiesReady, true, 'cycle B must complete its handshake');

  // Cycle A's join ack arrives late: it must not knock the successor back to
  // the unnegotiated state.
  deadSocket.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_c',
    desktop_connected: true,
  });
  assert.equal(staleClient.leaseId, 'lease_b',
    'a stale join ack must not clobber the current cycle lease');
  assert.equal(staleClient.desktopCapabilitiesReady, true,
    'a stale join ack must not wipe negotiated capabilities');

  // A real event on the successor must still dispatch normally (before the
  // fix: it hit the !desktopCapabilitiesReady knocked back by the old ack →
  // incompatible_desktop permanent shutdown).
  successor.message({
    v: 2,
    type: 'event',
    event: 'chat:delta',
    stream_epoch: 'epoch_b',
    seq: 6,
    payload: { text: 'live' },
  });
  await staleClient.eventDispatch;
  assert.equal(events.length, 1, 'the live event must dispatch despite the stale ack');
  assert.equal(staleClient.closedPermanently, false,
    'the stale ack must not brick the successor into incompatible_desktop');

  // Same family: a late endpoint_revoked must not close the healthy successor.
  deadSocket.message({ v: 2, type: 'endpoint_revoked', reason: 'desktop_offline_ttl_expired' });
  assert.equal(successor.closeCalled, false,
    'a stale endpoint_revoked must not close the healthy successor');
  assert.equal(staleClient.connectionState.status, 'connected',
    'a stale endpoint_revoked must not flip the successor to desktop_offline');
  assert.equal(staleClient.reconnectForDesktopGone, false,
    'a stale endpoint_revoked must not poison the fast-reconnect pacing');
}

// Page visible again: reconnect immediately when disconnected; never drop a
// healthy OPEN connection on a whim (background-tab timers are throttled, so
// the post-wake liveness judgment belongs to the watchdog and the online
// event).
{
  const hidden = bootClient();
  const hiddenClient = hidden.window.PinvouWebClient;
  await hiddenClient.policyPromise;
  hidden.document.visibilityState = 'hidden';
  hidden.MockWebSocket.instances[0].close();
  const hiddenBackoff = hidden.timers.find(timer => timer.delay < 10_000);
  assert.ok(hiddenBackoff);
  hidden.window.emit('visibilitychange');
  assert.equal(hidden.timers.includes(hiddenBackoff), true,
    'visibility must not trigger a reconnect while the page is hidden');
  hidden.document.visibilityState = 'visible';
  hidden.window.emit('visibilitychange');
  assert.equal(hidden.timers.includes(hiddenBackoff), false,
    'becoming visible must reconnect immediately when the socket is gone');
  assert.equal(hidden.MockWebSocket.instances.length, 2);

  const live = await connectWithCapabilities(['chat:delta'], () => {});
  live.window.emit('visibilitychange');
  assert.equal(live.socket.closeCalled, false,
    'becoming visible must not drop a healthy OPEN socket');
}

// online / visibilitychange must abort a zombie socket stuck in CONNECTING:
// otherwise recovery would wait for the browser's TCP timeout. After closing,
// recovery flows through the ordinary close -> reconnect funnel, and connect()
// early-returns while a socket still exists, so no double connect.
{
  const online = bootClient();
  const onlineClient = online.window.PinvouWebClient;
  await onlineClient.policyPromise;
  const zombie = online.MockWebSocket.instances[0];
  assert.equal(zombie.readyState, online.MockWebSocket.CONNECTING);
  online.window.emit('online');
  assert.equal(zombie.closeCalled, true,
    'becoming online must abort a socket wedged in CONNECTING');
  const reconnect = takeReconnectTimer(online.timers);
  reconnect.callback();
  assert.equal(online.MockWebSocket.instances.length, 2,
    'closing the zombie must route recovery through the single reconnect funnel');

  const visible = bootClient();
  const visibleClient = visible.window.PinvouWebClient;
  await visibleClient.policyPromise;
  const wedged = visible.MockWebSocket.instances[0];
  visible.document.visibilityState = 'visible';
  visible.window.emit('visibilitychange');
  assert.equal(wedged.closeCalled, true,
    'becoming visible must abort a socket wedged in CONNECTING');
  takeReconnectTimer(visible.timers).callback();
  assert.equal(visible.MockWebSocket.instances.length, 2,
    'the aborted socket must be replaced through the reconnect funnel');
}

// Desktop-gone failures (endpoint_not_found / desktop_offline_ttl_expired):
// the first 5 retries keep the historical <=10s fast pace (TTL recovery must be
// quick), then double per attempt capped at 60s; a successful join zeroes the
// counter and the retry pace returns to the original fast regime.
{
  const { window, MockWebSocket, timers } = bootClient();
  const client = window.PinvouWebClient;
  await client.policyPromise;
  const miss = (socket) => {
    socket.open();
    socket.message({ type: 'error', code: 'endpoint_not_found' });
  };
  for (let attempt = 0; attempt < 5; attempt += 1) {
    miss(MockWebSocket.instances[attempt]);
    const reconnect = takeReconnectTimer(timers);
    assert.equal(reconnect.delay, 500 * 2 ** attempt,
      'the first desktop-gone retries must keep the historical fast pace');
    reconnect.callback();
  }
  miss(MockWebSocket.instances[5]);
  const grown = timers.find(timer => timer.delay > 10_000 && timer.delay !== 30_000);
  assert.ok(grown, 'desktop-gone backoff must grow past the old 10s ceiling');
  assert.equal(grown.delay, 20_000, 'the sixth desktop-gone retry must double to 20s');
  grown.callback();
  miss(MockWebSocket.instances[6]);
  assert.equal(timers.some(timer => timer.delay === 40_000), true,
    'desktop-gone backoff must keep doubling per attempt');
  timers.find(timer => timer.delay === 40_000).callback();
  miss(MockWebSocket.instances[7]);
  assert.equal(timers.some(timer => timer.delay === 60_000), true,
    'desktop-gone backoff must cap at 60s');
  timers.find(timer => timer.delay === 60_000).callback();
  const recovered = MockWebSocket.instances[8];
  recovered.open();
  recovered.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_back',
    desktop_connected: true,
  });
  assert.equal(client.desktopGoneRetries, 0,
    'a successful join must reset the desktop-gone retry counter');
  recovered.close();
  assert.equal(takeReconnectTimer(timers).delay, 500,
    'after a successful join the retry pace must restart fast');
}

// Round-8 review: the desktop-gone count must belong only to desktop-gone
// failures — an ordinary transport drop (no desktop-gone/TTL reason) zeroes
// the counter in scheduleReconnect and the backoff returns to the fast regime.
// If the grown value were kept, one ordinary drop would inherit the offline
// scenario's 20s+ backoff, making the user wait out the whole slow window even
// though the desktop is clearly online.
{
  const { window, MockWebSocket, timers } = bootClient();
  const client = window.PinvouWebClient;
  await client.policyPromise;
  const miss = (socket) => {
    socket.open();
    socket.message({ type: 'error', code: 'endpoint_not_found' });
  };
  for (let attempt = 0; attempt < 5; attempt += 1) {
    miss(MockWebSocket.instances[attempt]);
    takeReconnectTimer(timers).callback();
  }
  miss(MockWebSocket.instances[5]);
  const grown = timers.find(timer => timer.delay > 10_000 && timer.delay !== 30_000);
  assert.equal(grown.delay, 20_000,
    'the desktop-gone ladder must first grow past the fast window');
  timers.splice(timers.indexOf(grown), 1);
  grown.callback();
  // Ordinary transport drop: the socket opens but never joins, with no
  // desktop-gone/TTL reason attached.
  const takeFastTimer = () => {
    const index = timers.findIndex(timer => timer.delay === 10_000);
    assert.ok(index >= 0, 'expected the fast-regime reconnect timer');
    return timers.splice(index, 1)[0];
  };
  const ordinary = MockWebSocket.instances[6];
  ordinary.open();
  ordinary.close();
  assert.equal(client.desktopGoneRetries, 0,
    'an ordinary transport close must reset the desktop-gone retry streak');
  const reset = takeFastTimer();
  assert.equal(reset.delay, 10_000,
    'the next reconnect delay must leave the grown desktop-gone ladder for the fast regime (fast-formula cap), not the grown 20s value');
  reset.callback();
  const again = MockWebSocket.instances[7];
  again.open();
  again.close();
  assert.equal(client.desktopGoneRetries, 0,
    'the streak must stay reset across further ordinary closes');
  assert.equal(takeFastTimer().delay, 10_000,
    'the following ordinary reconnect must stay in the fast regime');
}

// Round-8 review: the desktop_offline_ttl_expired branch must set
// reconnectForDesktopGone so consecutive TTL-recovery retries participate in
// the desktop-gone ladder (20s→60s cap). Without it, every TTL close zeroes
// the count through the ordinary ladder and an offline desktop is interrupted
// endlessly by <10s fast retries — while the existing TTL tests only assert a
// <10s timer exists, so deleting the line stayed green.
{
  const { window, MockWebSocket, timers } = bootClient();
  const client = window.PinvouWebClient;
  await client.policyPromise;
  assert.equal(client.closedPermanently, false);
  const ttlGone = (socket) => {
    socket.open();
    socket.message({
      v: 2,
      type: 'endpoint_revoked',
      endpoint_id: 'endpoint_test',
      reason: 'desktop_offline_ttl_expired',
    });
  };
  for (let attempt = 0; attempt < 5; attempt += 1) {
    ttlGone(MockWebSocket.instances[attempt]);
    const reconnect = takeReconnectTimer(timers);
    assert.equal(reconnect.delay, 500 * 2 ** attempt,
      'TTL-expiry retries keep the fast pace for the first attempts');
    reconnect.callback();
  }
  const takeGrownTimer = () => {
    const index = timers.findIndex(timer => timer.delay > 10_000 && timer.delay !== 30_000);
    assert.ok(index >= 0, 'expected the grown desktop-gone reconnect timer');
    return timers.splice(index, 1)[0];
  };
  ttlGone(MockWebSocket.instances[5]);
  const doubled = takeGrownTimer();
  assert.equal(doubled.delay, 20_000,
    'the TTL path must join the desktop-gone ladder and double past the fast window');
  doubled.callback();
  ttlGone(MockWebSocket.instances[6]);
  const quadrupled = takeGrownTimer();
  assert.equal(quadrupled.delay, 40_000,
    'the TTL ladder must keep doubling per attempt');
  quadrupled.callback();
  ttlGone(MockWebSocket.instances[7]);
  const capped = takeGrownTimer();
  assert.equal(capped.delay, 60_000,
    'the TTL ladder must reach the desktop-gone cap');
  capped.callback();
  ttlGone(MockWebSocket.instances[8]);
  assert.equal(takeGrownTimer().delay, 60_000,
    'the TTL ladder must stay capped at 60s');
}

// A missing or empty snapshot seq must not reset the cursor to 0 (otherwise
// the next join would trigger a full replay).
{
  const { client, socket } = await connectWithCapabilities(['chat:delta'], () => {});
  const snapshot = seq => ({
    v: 2,
    type: 'desktop_snapshot',
    stream_epoch: 'epoch_test',
    ...(seq === undefined ? {} : { seq }),
    snapshot: { capabilities: { protocol_version: 2, commands: [], events: ['chat:delta'] } },
  });
  client.lastSeq = 4;
  socket.message(snapshot());
  assert.equal(client.lastSeq, 4, 'a snapshot without seq must keep the cursor');
  socket.message(snapshot(null));
  assert.equal(client.lastSeq, 4, 'Number(null) === 0 must not reset the cursor');
  socket.message(snapshot(''));
  assert.equal(client.lastSeq, 4, 'Number("") === 0 must not reset the cursor');
  socket.message(snapshot(6));
  assert.equal(client.lastSeq, 6, 'an explicit seq must still advance the cursor');
}


// A truncated link missing credentials must stay parked on the blocking card:
// recovery paths such as online/visibilitychange and the watchdog must not
// bypass the credentials gate and turn credentials_missing into a join retry
// loop that is doomed to fail (a non-blocking spinner covering the blocking
// card, retrying forever without ever connecting).
{
  const harness = bootClient(undefined, { hash: '#endpoint=endpoint_test' });
  const client = harness.window.PinvouWebClient;
  await client.policyPromise;
  assert.equal(client.connectionState.status, 'credentials_missing');
  assert.equal(harness.MockWebSocket.instances.length, 0,
    'a truncated link must not open a socket');
  harness.window.emit('online');
  await new Promise(resolve => { setImmediate(resolve); });
  harness.window.emit('visibilitychange');
  await new Promise(resolve => { setImmediate(resolve); });
  assert.equal(harness.MockWebSocket.instances.length, 0,
    'online/visible recovery must not bypass the credentials gate');
  assert.equal(client.connectionState.status, 'credentials_missing',
    'the truncated link must stay parked on the blocking card');
}

// Round-9 review: the throwing-constructor path had zero coverage
// (noteSpeculativeFailure is the only counting point on the throw arm, and the
// fixture constructor previously could not throw). Discriminating scenario:
// after a healthy join the cycle goes silently speculative, then constructors
// start throwing from the second one — the first throwing spawn must count the
// failure toward the speculative ladder (without that count, the second
// throwing spawn would still be treated as speculative and the UI would stay
// on connected forever); the second throwing spawn pushes the failure past the
// cap, so the reconnect must become visible.
{
  const blocked = await connectWithCapabilities(['chat:delta'], () => {});
  assert.equal(blocked.client.speculativeFailures, 0,
    'a verified join must leave the failure ladder at zero');
  blocked.MockWebSocket.throwFrom = 1;
  blocked.client.lastInboundAt = Date.now() - 121_000;
  blocked.client._livenessCheck();
  assert.equal(blocked.MockWebSocket.instances.length, 1,
    'the watchdog closes the joined socket before the throwing spawns');
  takeReconnectTimer(blocked.timers).callback();
  assert.equal(blocked.MockWebSocket.instances.length, 1,
    'a throwing constructor must not register a socket instance');
  assert.equal(blocked.client.speculativeFailures, 1,
    'a thrown constructor on a speculative spawn must count the failed cycle');
  // The first throw clears the speculative flag: the next spawn is a visible
  // reconnect and a further throw no longer counts (counting happens only on
  // speculative spawns) — the escalation works not by "pushing past the cap"
  // but by clearing the speculative flag.
  assert.equal(blocked.client.speculativeReconnect, false,
    'a thrown speculative spawn must clear the speculative flag');
  assert.equal(blocked.client.connectionState.status, 'connecting',
    'the post-throw reconnect must be visible, not silently repeated');
  takeReconnectTimer(blocked.timers).callback();
  assert.equal(blocked.client.speculativeFailures, 1,
    'only speculative spawns count; the visible retry cycle does not');
  assert.equal(blocked.client.connectionState.status, 'connecting',
    'the visible retry stays visible across repeated throw cycles');
}

// Round-23 review hardening pin: open is the last of the four socket events
// without an identity gate — a stale socket's open listener survives the
// rescue/rebuild, and firing late it would sendRaw the join frame onto the
// current this.socket (the successor). A spec-compliant browser does not
// deliver open after a CONNECTING-phase close(), so this shape is unreachable
// in the existing topology; the gate is defense symmetric with the
// close/error/message trio. Deleting the identity gate goes red (the successor
// sees a second join frame).
{
  const staleOpen = bootClient();
  const staleOpenClient = staleOpen.window.PinvouWebClient;
  await staleOpenClient.policyPromise;
  const first = staleOpen.MockWebSocket.instances[0];
  first.open();
  first.message({
    v: 2,
    type: 'web_client_joined',
    endpoint_id: 'endpoint_test',
    lease_id: 'lease_test',
    desktop_connected: true,
  });
  first.readyState = staleOpen.MockWebSocket.CLOSING;
  staleOpenClient._livenessCheck();
  staleOpenClient.closingStartedAt = Date.now() - 46_000;
  staleOpenClient._livenessCheck();
  assert.equal(staleOpenClient.socket, null, 'the wedged socket must be rescued');
  takeReconnectTimer(staleOpen.timers).callback();
  const successor = staleOpen.MockWebSocket.instances[1];
  successor.open();
  first.open();
  const joinFrames = successor.sent.filter((frame) => frame.type === 'web_client_join');
  assert.equal(joinFrames.length, 1,
    'a stale socket open event must not send its join frame on the successor');
}

// Round-9 review: the silence window (120s) and the CONNECTING wedge window
// (45s) previously had only one-sided behavior pins (a probe crossing the
// threshold from one side kept every probe green even with the constants
// shrunk) — pin the constant literals at source level, complementing the vm
// behavior pins; changing a constant requires explicitly re-reviewing the
// behavior pins on both sides.
assert.match(source, /const LIVENESS_WATCHDOG_INTERVAL_MS = 30_000;/);
assert.match(source, /const LIVENESS_SILENCE_TIMEOUT_MS = 120_000;/);
assert.match(source, /const LIVENESS_MAX_SPECULATIVE_FAILURES = 1;/);
assert.match(source, /const LIVENESS_CONNECT_WEDGE_TIMEOUT_MS = 45_000;/);
assert.match(source, /const DESKTOP_GONE_FAST_RETRIES = 5;/);
assert.match(source, /const DESKTOP_GONE_MAX_DELAY_MS = 60_000;/);

// Round-11 review NIT: move the pass banner to the true end of the file — it
// previously printed before the credentials-gate / throwing-constructor /
// constant-pin blocks, so a failing run announced "passed" first too.
console.log('web bootstrap transport tests passed');
