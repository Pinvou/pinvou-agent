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
  return { window, document, storage, dispatched, timers, MockWebSocket };
}

// 重连退避定时器（<10s）与 30s 看门狗 interval、180s RPC 超时共存于 timers，
// 断言重连行为时必须只取出退避定时器。
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

// 首条消息使用调用方提供的稳定请求 ID。WebSocket 重连后必须复用同一 ID，
// 桌面端才能从持久 RPC ledger 返回原结果而不重复创建 Session 或发送消息。
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

// TTL 到期的 endpoint_revoked 不是永久吊销（PROTOCOL.md）：桌面端可能稍后用
// 持久凭证重建同一 endpoint，浏览器必须进入 desktop_offline 等待并走普通重连
// 路径自动重新 join，而不是永久死路覆盖层。
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
  // 退避窗口内保持 desktop_offline 专属文案(「桌面端离线,保持页面打开」):
  // 紧随其后的 close→scheduleReconnect 不得把该状态覆盖成「连接中」,
  // 否则在最长 60s 的退避里用户看到的是误导性的转圈。
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

// 异常断线先触发 error 再触发 close:error 监听同样不得把 desktop_offline
// 的专属文案覆盖成「连接中」——close→scheduleReconnect 有门,round-4 复审
// 发现 error 监听没有,离线窗口内的 TCP reset 会把诚实文案换成转圈。
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

// 真正的吊销（其他 reason 或缺失 reason）保持今天的永久关闭行为。
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

// 半开连接看门狗：OPEN 且静默超过阈值必须投机重连（游标重放使重连幂等）；
// 有近期入站流量或 socket 未 OPEN 时不得误杀连接。staleness 只是怀疑而非证
// 据：投机重连验证期间 UI 保持 connected、不广播 connecting；验证失败（重连
// socket 在 join 前再断开）才清除投机状态并升级为可见的 connecting。
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
  // A verified join resets the failure ladder: the next silent cycle is
  // speculative again instead of escalating off stale failures.
  stale.client.lastInboundAt = Date.now() - 121_000;
  stale.client._livenessCheck();
  assert.equal(stale.client.speculativeReconnect, true,
    'a verified join must reset the speculative failure ladder');

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

// 看门狗 interval 必须真实注册并驱动 _livenessCheck，重连后不得重复注册。
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

// 网络恢复（online 事件）：断开时立即重连并清退避；OPEN 连接视为可疑并重连。
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

// 看门狗必须救出卡在 CONNECTING 的 wedge 连接:超过 wedge 窗口的 CONNECTING
// socket 被中止走重连漏斗;窗口内的年轻 CONNECTING socket 不受影响。
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

// 页面重新可见：未连接时立即重连；不得凭空丢弃健康的 OPEN 连接（后台标签页
// 定时器被节流，唤醒后的存活判断交给看门狗与 online 事件）。
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

// online / visibilitychange 必须中止卡在 CONNECTING 的僵尸 socket：否则恢复要
// 等浏览器 TCP 超时。关闭后走普通 close -> 重连漏斗，connect() 在 socket 存在
// 期间早退，因此不会双重连接。
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

// 桌面端不在类失败（endpoint_not_found / desktop_offline_ttl_expired）：前 5 次
// 重试保持既有 ≤10s 快节奏（TTL 恢复流程要快），之后逐次翻倍、60s 封顶；成功
// join 后计数清零，重试节奏回到最初的快速档。
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

// desktop_snapshot 缺失或空 seq 不得把游标重置为 0（否则下次 join 触发全量重放）。
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

console.log('web bootstrap transport tests passed');

// 缺凭证的截断链接必须停在阻断卡:online/visibilitychange 看门狗等恢复路径
// 不得绕过凭证 gate,把 credentials_missing 转成注定失败的 join 重试循环
// (非阻断小转圈覆盖阻断卡,永远连不上却一直在重试)。
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
