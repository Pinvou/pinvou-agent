import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as sleep } from 'node:timers/promises';
import test from 'node:test';

const WRAPPER = fileURLToPath(new URL(
  '../src-tauri/resources/common/bundle/mcp-servers/browser-wrapper.mjs',
  import.meta.url,
));
const SESSION_ID = 'host-core-lifecycle-test';
const SESSION_TOKEN = '0123456789abcdef';

function atomicWriteJson(path, value) {
  const temporary = `${path}.${process.pid}.tmp`;
  writeFileSync(temporary, JSON.stringify(value));
  renameSync(temporary, path);
}

async function waitUntil(predicate, message, timeoutMs = 2_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await sleep(10);
  }
  assert.fail(typeof message === 'function' ? message() : message);
}

function createFakeHost(root) {
  const requestDirectory = join(root, 'host-requests');
  const handled = new Set();
  const state = {
    mode: 'normal',
    workspaceRunning: false,
    prepareCalls: 0,
    coreCalls: 0,
    committedEffects: 0,
    terminalError: 'browser/control-lease-lost',
    lastCoreRequestId: null,
    lastRequest: null,
    failure: null,
  };

  const respond = (request, response) => {
    const responsePath = join(
      requestDirectory,
      `${SESSION_TOKEN}-${request.request_id}.response`,
    );
    atomicWriteJson(responsePath, {
      protocol_version: request.protocol_version,
      request_id: request.request_id,
      idempotency_key: request.idempotency_key,
      ...response,
    });
  };

  const timer = setInterval(() => {
    if (!existsSync(requestDirectory) || state.failure) return;
    try {
      for (const name of readdirSync(requestDirectory)) {
        if (!name.endsWith('.json') || handled.has(name)) continue;
        const request = JSON.parse(readFileSync(join(requestDirectory, name), 'utf8'));
        handled.add(name);
        state.lastRequest = request;

        if (request.operation === 'prepare') {
          state.prepareCalls += 1;
          state.workspaceRunning = true;
          respond(request, { ok: true, result: { ready: true } });
          continue;
        }

        if (request.operation !== 'core_tool') {
          respond(request, { ok: false, error: `unexpected operation: ${request.operation}` });
          continue;
        }

        state.coreCalls += 1;
        state.lastCoreRequestId = request.request_id;
        if (state.mode === 'blocked-core') continue;
        if (state.mode === 'lifecycle-failure' || !state.workspaceRunning) {
          respond(request, { ok: false, error: 'browser/workspace-unavailable' });
          continue;
        }
        if (state.mode === 'terminal-failure-after-effect') {
          state.committedEffects += 1;
          respond(request, { ok: false, error: state.terminalError });
          continue;
        }
        if (state.mode === 'commit-without-response') {
          state.committedEffects += 1;
          continue;
        }

        state.committedEffects += 1;
        respond(request, {
          ok: true,
          result: { content: [{ type: 'text', text: 'host core ok' }] },
        });
      }
    } catch (error) {
      state.failure = error;
    }
  }, 5);

  return {
    state,
    stopWorkspace() {
      state.workspaceRunning = false;
    },
    close() {
      clearInterval(timer);
      if (state.failure) throw state.failure;
    },
  };
}

function driveWrapper(root, env = {}) {
  const child = spawn(
    process.execPath,
    [WRAPPER, '@pinvou/browser-core', join(root, 'cdp-port.json')],
    {
      stdio: ['pipe', 'pipe', 'pipe'],
      env: {
        ...process.env,
        PINVOU3_BROWSER_SESSION_ID: SESSION_ID,
        PINVOU3_BROWSER_SESSION_TOKEN: SESSION_TOKEN,
        ...env,
      },
    },
  );
  let stdout = '';
  let stderr = '';
  let nextId = 1;
  const pending = new Map();

  child.stderr.on('data', (chunk) => { stderr += chunk; });
  child.stdout.on('data', (chunk) => {
    stdout += chunk;
    let newline;
    while ((newline = stdout.indexOf('\n')) >= 0) {
      const line = stdout.slice(0, newline);
      stdout = stdout.slice(newline + 1);
      if (!line.trim()) continue;
      const message = JSON.parse(line);
      const waiter = pending.get(message.id);
      if (!waiter) continue;
      pending.delete(message.id);
      clearTimeout(waiter.timeout);
      waiter.resolve(message);
    }
  });

  const requestWithId = (method, params = {}) => {
    const id = nextId++;
    const response = new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (!pending.delete(id)) return;
        reject(new Error(`${method} timed out; wrapper stderr: ${stderr}`));
      }, 10_000);
      pending.set(id, { resolve, reject, timeout });
    });
    child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`);
    const stopWaiting = () => {
      const waiter = pending.get(id);
      if (!waiter) return;
      pending.delete(id);
      clearTimeout(waiter.timeout);
    };
    return { id, response, stopWaiting };
  };

  return {
    child,
    stderr: () => stderr,
    notify(method, params = {}) {
      child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', method, params })}\n`);
    },
    requestWithId,
    request(method, params = {}) {
      return requestWithId(method, params).response;
    },
  };
}

async function stopWrapper(child) {
  if (child.exitCode == null) child.stdin.end();
  if (child.exitCode == null) {
    await Promise.race([
      new Promise((resolve) => child.once('exit', resolve)),
      sleep(500),
    ]);
  }
  if (child.exitCode == null) child.kill('SIGKILL');
}

test('same Host Core wrapper recovers once after host stop without retrying committed failures', async () => {
  const root = mkdtempSync(join(tmpdir(), 'pinvou-host-core-lifecycle-'));
  mkdirSync(root, { recursive: true });
  const host = createFakeHost(root);
  const wrapper = driveWrapper(root);

  try {
    const initialized = await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'host-core-lifecycle-test', version: '0' },
    });
    assert.equal(initialized.result.serverInfo.name, 'pinvou-browser-core');
    wrapper.notify('notifications/initialized');

    const first = await wrapper.request('tools/call', {
      name: 'list_pages',
      arguments: {},
    });
    assert.equal(first.result.content[0].text, 'host core ok');
    assert.deepEqual(
      [host.state.prepareCalls, host.state.coreCalls, host.state.committedEffects],
      [1, 1, 1],
    );
    const caller = host.state.lastRequest;
    assert.equal(caller.caller_pid, wrapper.child.pid);
    assert.match(caller.wrapper_instance_nonce, /^[0-9a-f]{32}$/);
    const heartbeatPath = join(
      root,
      'host-requests',
      `${SESSION_TOKEN}-${caller.wrapper_instance_nonce}.heartbeat`,
    );
    assert.equal(existsSync(heartbeatPath), true);
    const firstHeartbeat = JSON.parse(readFileSync(heartbeatPath, 'utf8'));
    assert.deepEqual(
      {
        kind: firstHeartbeat.kind,
        sessionId: firstHeartbeat.session_id,
        sessionToken: firstHeartbeat.session_token,
        callerPid: firstHeartbeat.caller_pid,
        nonce: firstHeartbeat.wrapper_instance_nonce,
      },
      {
        kind: 'host_caller_heartbeat',
        sessionId: SESSION_ID,
        sessionToken: SESSION_TOKEN,
        callerPid: caller.caller_pid,
        nonce: caller.wrapper_instance_nonce,
      },
    );
    await waitUntil(
      () => JSON.parse(readFileSync(heartbeatPath, 'utf8')).heartbeat_at >
        firstHeartbeat.heartbeat_at,
      () => `the live wrapper did not renew its instance heartbeat: ${wrapper.stderr()}`,
      4_500,
    );

    // Simulate browser_stop in Tauri while this Engine-owned wrapper remains alive.
    host.stopWorkspace();
    const beforeRecoveryEffects = host.state.committedEffects;
    const recovered = await wrapper.request('tools/call', {
      name: 'list_pages',
      arguments: {},
    });
    assert.equal(recovered.result.content[0].text, 'host core ok');
    assert.deepEqual(
      [host.state.prepareCalls, host.state.coreCalls],
      [2, 3],
      'the stale call fails before dispatch, then exactly one prepare and one retry run',
    );
    assert.equal(
      host.state.committedEffects - beforeRecoveryEffects,
      1,
      'the recovered logical tool commits only once',
    );

    host.state.mode = 'lifecycle-failure';
    const beforeRepeatedFailure = { ...host.state };
    const failed = await wrapper.request('tools/call', {
      name: 'list_pages',
      arguments: {},
    });
    assert.match(failed.error.message, /^browser\/workspace-unavailable/);
    assert.equal(host.state.prepareCalls - beforeRepeatedFailure.prepareCalls, 1);
    assert.equal(host.state.coreCalls - beforeRepeatedFailure.coreCalls, 2);
    assert.equal(host.state.committedEffects, beforeRepeatedFailure.committedEffects);
    await sleep(100);
    assert.equal(
      host.state.coreCalls - beforeRepeatedFailure.coreCalls,
      2,
      'a repeated lifecycle failure must not start an unbounded retry loop',
    );

    // The repeated lifecycle failure invalidated the cache. Preparation is
    // allowed, but a lease error after a simulated commit must never retry.
    host.state.mode = 'terminal-failure-after-effect';
    for (const [index, terminalError] of [
      'browser/control-lease-lost',
      'browser/native-surface-missing',
    ].entries()) {
      host.state.terminalError = terminalError;
      const beforeTerminalFailure = { ...host.state };
      const terminalFailure = await wrapper.request('tools/call', {
        name: 'list_pages',
        arguments: {},
      });
      assert.equal(terminalFailure.error.message, terminalError);
      assert.equal(
        host.state.prepareCalls - beforeTerminalFailure.prepareCalls,
        index === 0 ? 1 : 0,
      );
      assert.equal(host.state.coreCalls - beforeTerminalFailure.coreCalls, 1);
      assert.equal(host.state.committedEffects - beforeTerminalFailure.committedEffects, 1);
    }
  } finally {
    host.close();
    await stopWrapper(wrapper.child);
    if (host.state.lastRequest?.wrapper_instance_nonce) {
      assert.equal(existsSync(join(
        root,
        'host-requests',
        `${SESSION_TOKEN}-${host.state.lastRequest.wrapper_instance_nonce}.heartbeat`,
      )), false, 'graceful shutdown must revoke the wrapper instance heartbeat');
    }
    rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('Host Core cancellation writes a durable tombstone for an in-flight native tool', async () => {
  const root = mkdtempSync(join(tmpdir(), 'pinvou-host-core-cancel-'));
  mkdirSync(root, { recursive: true });
  const host = createFakeHost(root);
  const wrapper = driveWrapper(root);

  try {
    await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'host-core-cancel-test', version: '0' },
    });
    wrapper.notify('notifications/initialized');
    host.state.mode = 'blocked-core';

    const call = wrapper.requestWithId('tools/call', {
      name: 'click',
      arguments: { pageId: 0, uid: 'button-1' },
    });
    // Cancellation intentionally suppresses the JSON-RPC response. Prevent
    // the harness timeout from becoming an unhandled rejection while the
    // wrapper is stopped in finally.
    void call.response.catch(() => {});
    await waitUntil(
      () => typeof host.state.lastCoreRequestId === 'string',
      'the fake host never observed the core_tool request',
    );

    wrapper.notify('notifications/cancelled', { requestId: call.id });
    const stem = `${SESSION_TOKEN}-${host.state.lastCoreRequestId}`;
    const tombstonePath = join(root, 'host-requests', `${stem}.cancelled`);
    const requestPath = join(root, 'host-requests', `${stem}.json`);
    await waitUntil(
      () => existsSync(tombstonePath),
      'the wrapper did not publish a cancellation tombstone',
    );
    assert.equal(existsSync(requestPath), false, 'an unclaimed request should be removed');
    const tombstone = JSON.parse(readFileSync(tombstonePath, 'utf8'));
    call.stopWaiting();
    assert.equal(tombstone.kind, 'host_request_cancelled');
    assert.equal(tombstone.reason, 'client-cancelled');
    assert.equal(tombstone.request_id, host.state.lastCoreRequestId);
    assert.equal(host.state.committedEffects, 0);
  } finally {
    host.close();
    await stopWrapper(wrapper.child);
    rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('Host Core stdin shutdown tombstones an in-flight native tool before exit', async () => {
  const root = mkdtempSync(join(tmpdir(), 'pinvou-host-core-stdin-shutdown-'));
  mkdirSync(root, { recursive: true });
  const host = createFakeHost(root);
  const wrapper = driveWrapper(root);

  try {
    await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'host-core-stdin-shutdown-test', version: '0' },
    });
    wrapper.notify('notifications/initialized');
    host.state.mode = 'blocked-core';

    const call = wrapper.requestWithId('tools/call', {
      name: 'click',
      arguments: { pageId: 0, uid: 'button-before-shutdown' },
    });
    void call.response.catch(() => {});
    await waitUntil(
      () => typeof host.state.lastCoreRequestId === 'string',
      'the fake host never observed the core_tool request',
    );
    wrapper.child.stdin.end();

    const stem = `${SESSION_TOKEN}-${host.state.lastCoreRequestId}`;
    const tombstonePath = join(root, 'host-requests', `${stem}.cancelled`);
    await waitUntil(
      () => existsSync(tombstonePath),
      'stdin shutdown did not publish the in-flight host tombstone',
    );
    call.stopWaiting();
    if (wrapper.child.exitCode == null && wrapper.child.signalCode == null) {
      await Promise.race([
        new Promise((resolve) => wrapper.child.once('exit', resolve)),
        sleep(2_000).then(() => { throw new Error('Host Core wrapper did not exit promptly'); }),
      ]);
    }
    assert.equal(JSON.parse(readFileSync(tombstonePath, 'utf8')).reason, 'client-cancelled');
    assert.equal(host.state.committedEffects, 0);
  } finally {
    host.close();
    await stopWrapper(wrapper.child);
    rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('Host Core mutation timeout is a non-retryable commit-unknown result', async () => {
  const root = mkdtempSync(join(tmpdir(), 'pinvou-host-core-timeout-'));
  mkdirSync(root, { recursive: true });
  const host = createFakeHost(root);
  const wrapper = driveWrapper(root, {
    PINVOU3_BROWSER_HOST_CORE_REQUEST_TIMEOUT_MS: '100',
  });

  try {
    await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'host-core-timeout-test', version: '0' },
    });
    wrapper.notify('notifications/initialized');
    host.state.mode = 'commit-without-response';

    const before = {
      coreCalls: host.state.coreCalls,
      committedEffects: host.state.committedEffects,
    };
    const timedOut = await wrapper.request('tools/call', {
      name: 'click',
      arguments: { pageId: 0, uid: 'committed-button' },
    });

    assert.equal(timedOut.result.isError, true);
    assert.equal(
      timedOut.result.structuredContent.errorCode,
      'browser/action-commit-unknown-after-host-timeout',
    );
    assert.equal(timedOut.result.structuredContent.actionCommitState, 'unknown');
    assert.equal(timedOut.result.structuredContent.actionMayHaveCommitted, true);
    assert.equal(timedOut.result.structuredContent.retryable, false);
    assert.equal(timedOut.result.structuredContent.toolName, 'click');
    assert.equal(timedOut.result.structuredContent.hostOperation, 'core_tool');
    assert.match(timedOut.result.content[0].text, /Do not repeat the action/);
    assert.equal(host.state.coreCalls - before.coreCalls, 1);
    assert.equal(host.state.committedEffects - before.committedEffects, 1);

    const stem = `${SESSION_TOKEN}-${host.state.lastCoreRequestId}`;
    await waitUntil(
      () => existsSync(join(root, 'host-requests', `${stem}.cancelled`)),
      'the timed-out mutation did not publish a host tombstone',
    );
    await sleep(250);
    assert.equal(host.state.coreCalls - before.coreCalls, 1, 'timeout must not redispatch');

    host.state.mode = 'normal';
    const followingRead = await wrapper.request('tools/call', {
      name: 'list_pages',
      arguments: {},
    });
    assert.equal(followingRead.result.content[0].text, 'host core ok');
  } finally {
    host.close();
    await stopWrapper(wrapper.child);
    rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('Host Core tab-navigation uncertainty is a structured non-retryable outcome', async () => {
  const root = mkdtempSync(join(tmpdir(), 'pinvou-host-core-navigation-unknown-'));
  mkdirSync(root, { recursive: true });
  const host = createFakeHost(root);
  const wrapper = driveWrapper(root);

  try {
    await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'host-core-navigation-unknown-test', version: '0' },
    });
    wrapper.notify('notifications/initialized');
    host.state.mode = 'terminal-failure-after-effect';
    host.state.terminalError =
      'browser/action-commit-unknown-after-tab-navigation: final create CAS was rejected';

    const before = {
      coreCalls: host.state.coreCalls,
      committedEffects: host.state.committedEffects,
    };
    const outcome = await wrapper.request('tools/call', {
      name: 'new_page',
      arguments: { url: 'https://navigation-unknown.example/' },
    });

    assert.equal(outcome.result.isError, true);
    assert.equal(
      outcome.result.structuredContent.errorCode,
      'browser/action-commit-unknown-after-tab-navigation',
    );
    assert.equal(outcome.result.structuredContent.actionCommitState, 'unknown');
    assert.equal(outcome.result.structuredContent.retryable, false);
    assert.equal(outcome.result.structuredContent.hostOperation, 'core_tool');
    assert.equal(host.state.coreCalls - before.coreCalls, 1);
    assert.equal(host.state.committedEffects - before.committedEffects, 1);
  } finally {
    host.close();
    await stopWrapper(wrapper.child);
    rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});
