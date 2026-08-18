import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const source = fs.readFileSync(path.join(root, 'src', 'platform', 'web', 'bootstrap.js'), 'utf8');

function bootClient(policy) {
  const storage = new Map();
  const dispatched = [];
  const timers = [];
  let timerId = 0;

  class MockWebSocket {
    static OPEN = 1;
    static instances = [];

    constructor(url) {
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
      hash: '#endpoint=endpoint_test&token=access_test',
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
  };
  window.window = window;
  const deterministicMath = Object.create(Math);
  deterministicMath.random = () => 0.5;

  const context = {
    window,
    document: { currentScript: { src: 'https://relay.test/pinvou3/remote/platform/web/bootstrap.js' } },
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
  return { window, storage, dispatched, timers, MockWebSocket };
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
  assert.equal(timers.length, 1);
  assert.equal(timers[0].delay, 500);
  timers.shift().callback();

  const secondSocket = MockWebSocket.instances[1];
  secondSocket.open();
  secondSocket.message({ type: 'error', code: 'endpoint_not_found' });
  assert.equal(timers.length, 1);
  assert.equal(timers[0].delay, 1000);
  timers.shift().callback();

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
  assert.equal(timers.length, 1);
  assert.equal(timers[0].delay, 500);
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
  const { client, socket, storage } = await connectWithCapabilities(['chat:delta'], () => {
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

// 设备文件上传能力:未收到快照前 fail closed;旧桌面缺任一命令保持关闭,
// 两条命令齐备才开放,与桌面附件双入口的显示/隐藏协商一致。
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

// 代码模式只在目标桌面完整实现 Web-safe ACP 命令与事件合同时开放。
// 任一命令或 acp:event 缺失都代表旧桌面，入口必须 fail closed。
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
    'cancel_codex_acp',
    'web_access_get_codex_acp_timeline',
    'web_access_get_codex_acp_pending_permissions',
    'respond_codex_acp_permission',
    'web_access_get_codex_acp_pending_elicitations',
    'respond_codex_acp_elicitation',
    'web_access_list_codex_workspace',
    'web_access_search_codex_workspace',
    'web_access_preview_codex_workspace_file',
    'get_codex_workspace_changes',
    'get_codex_workspace_diff',
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

// 首条消息使用调用方提供的稳定请求 ID。WebSocket 重连后必须复用同一 ID，
// 桌面端才能从持久 RPC ledger 返回原结果而不重复创建 Session 或发送消息。
{
  const command = 'web_access_create_session_and_chat';
  const harness = bootClient({ allowed_commands: [command], allowed_events: [] });
  const { window, MockWebSocket, timers } = harness;
  const client = window.PinvouWebClient;
  await client.policyPromise;
  client.markFrontendReady();
  await new Promise(resolve => setImmediate(resolve));
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
  await new Promise(resolve => setImmediate(resolve));
  const firstRequest = firstSocket.sent.find(message => message.type === 'rpc_request');
  assert.ok(firstRequest, JSON.stringify({
    sent: firstSocket.sent,
    frontendReady: client.frontendReady,
    stateReady: client.stateReady,
    desktopCapabilitiesReady: client.desktopCapabilitiesReady,
    allowedCommands: Array.from(client.allowedCommands || []),
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

// 结构化错误码必须传到 Bridge，才能区分可安全重试的超时和结果未知。
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
  await new Promise(resolve => setImmediate(resolve));
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

console.log('web bootstrap transport tests passed');
