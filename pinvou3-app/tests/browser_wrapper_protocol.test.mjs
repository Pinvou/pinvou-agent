/* eslint-disable no-promise-executor-return -- Promise executors adapt timer and callback APIs whose registration handles are intentionally ignored. */
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import {
  adaptBrowserCatalog,
  assertAllowedBrowserToolCall,
  assertAllowedHostedNavigation,
  browserHostBackendPolicy,
  browserToolMayMutate,
  buildBijectivePageTokenMaps,
  BROWSER_PROTOCOL_FRAME_MAX_BYTES,
  BROWSER_PROTOCOL_FRAME_TOO_LARGE_ERROR_CODE,
  BROWSER_STARTUP_BACKLOG_EXCEEDED_ERROR_CODE,
  createBoundedLineBacklog,
  createBoundedNdjsonDecoder,
  createHostCallerHeartbeat,
  createHostCancellationTombstone,
  createHostRequestEnvelope,
  createOrderedWritableQueue,
  effectiveNavigateType,
  explicitOwnedPageId,
  filterPagesResult,
  findHostedSessionPage,
  findHostedTabPage,
  findHostedWorkspacePage,
  HOST_CALLER_HEARTBEAT_INTERVAL_MS,
  HOST_REQUEST_PROTOCOL_VERSION,
  hostCallerHeartbeatArtifactName,
  hostLeaseAssertionPayload,
  hostMutationAuthorizationPayload,
  hostRequestArtifactNames,
  inputToolNames,
  isAllowedBrowserUrl,
  isRecoverableHostCoreWorkspaceError,
  isReusableBootstrapBlankPage,
  pageScopedToolNames,
  parseAuthoritativeHostWorkspace,
  parseBrowserPages,
  parseCreatedTabResult,
  parseHostActivationLease,
  parseHostResponseEnvelope,
  PERSISTED_BROWSER_LAST_ERROR_CODES,
  remapCancellationNotification,
  routeToolCallToPage,
  runLeasedHostDispatch,
  runVisiblePageOperation,
  uncancelledBufferedRequests,
} from '../src-tauri/resources/common/bundle/mcp-servers/browser-wrapper-protocol.mjs';

test('ordered writable queue preserves protocol order across backpressure', async () => {
  class ControlledWritable extends EventEmitter {
    chunks = [];

    write(chunk) {
      this.chunks.push(chunk);
      return this.chunks.length !== 1;
    }
  }

  const stream = new ControlledWritable();
  const queue = createOrderedWritableQueue(stream);
  const first = queue.write('first\n');
  const second = queue.write('second\n');
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(stream.chunks, ['first\n']);

  stream.emit('drain');
  await Promise.all([first, second]);
  await queue.flush();
  assert.deepEqual(stream.chunks, ['first\n', 'second\n']);
});

test('ordered writable queue reports stdout failure once and fails flush', async () => {
  class BlockedWritable extends EventEmitter {
    write() {
      return false;
    }
  }

  const failures = [];
  const stream = new BlockedWritable();
  const queue = createOrderedWritableQueue(stream, {
    onError: (error) => failures.push(error.message),
  });
  const pending = queue.write('response\n');
  await new Promise((resolve) => setImmediate(resolve));
  stream.emit('error', new Error('broken pipe'));
  await pending;

  await assert.rejects(queue.flush(), /broken pipe/);
  stream.emit('error', new Error('later failure'));
  assert.deepEqual(failures, ['broken pipe']);
});

test('ordered writable queue fails deterministically when a blocked stream closes', async () => {
  class BlockedWritable extends EventEmitter {
    write() {
      return false;
    }
  }

  const stream = new BlockedWritable();
  const queue = createOrderedWritableQueue(stream);
  const pending = queue.write('response\n');
  await new Promise((resolve) => setImmediate(resolve));
  stream.emit('close');
  await pending;
  await assert.rejects(queue.flush(), /closed before queued output drained/);
});

test('ordered writable queue permits one oversized active response', async () => {
  class AcceptingWritable extends EventEmitter {
    chunks = [];

    write(chunk) {
      this.chunks.push(chunk);
      return true;
    }
  }

  const stream = new AcceptingWritable();
  const queue = createOrderedWritableQueue(stream, { maxPendingBytes: 8 });
  const oversized = 'x'.repeat(1024 * 1024 + 1);
  await queue.write(oversized);
  await queue.flush();

  assert.equal(stream.chunks.length, 1);
  assert.equal(stream.chunks[0].length, oversized.length);
  assert.equal(queue.pendingBytes, 0);
  assert.equal(queue.queuedBytes, 0);
});

test('ordered writable queue bounds follow-up backlog behind an oversized stalled response', async () => {
  class BlockedWritable extends EventEmitter {
    chunks = [];

    write(chunk) {
      this.chunks.push(chunk);
      return false;
    }
  }

  const failures = [];
  const stream = new BlockedWritable();
  const queue = createOrderedWritableQueue(stream, {
    maxPendingBytes: 8,
    onError: (error) => failures.push(error.message),
  });
  const oversized = 'x'.repeat(32);
  const first = queue.write(oversized);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(stream.chunks, [oversized]);

  const second = queue.write('12345678');
  assert.equal(queue.queuedBytes, 8);
  const saturated = queue.write('9');
  await Promise.all([first, second, saturated]);

  assert.deepEqual(stream.chunks, [oversized]);
  assert.equal(queue.pendingBytes, 0);
  assert.equal(queue.queuedBytes, 0);
  await assert.rejects(queue.flush(), /8-byte queued backlog limit/);
  assert.deepEqual(failures, ['Writable queue exceeded its 8-byte queued backlog limit']);
});

test('bounded NDJSON decoder rejects a no-newline frame as soon as it crosses the cap', () => {
  const decoder = createBoundedNdjsonDecoder({
    maxFrameBytes: 8,
    source: 'test stdin',
  });

  assert.deepEqual(decoder.push(Buffer.from('1234')), []);
  assert.deepEqual(decoder.push(Buffer.from('5678')), []);
  assert.throws(
    () => decoder.push(Buffer.from('9')),
    new RegExp(`${BROWSER_PROTOCOL_FRAME_TOO_LARGE_ERROR_CODE}.*8-byte`),
  );
  assert.equal(decoder.pendingBytes, 8);
});

test('bounded NDJSON decoder rejects an oversized complete line before publication', () => {
  const decoder = createBoundedNdjsonDecoder({
    maxFrameBytes: 8,
    source: 'test child stdout',
  });

  assert.throws(
    () => decoder.push(Buffer.from('123456789\n')),
    new RegExp(`${BROWSER_PROTOCOL_FRAME_TOO_LARGE_ERROR_CODE}.*9 bytes`),
  );
  assert.ok(decoder.failure);
});

test('startup request backlog rejects count and byte floods with a stable error', () => {
  const countBounded = createBoundedLineBacklog({
    maxLines: 2,
    maxBytes: 32,
    source: 'test startup backlog',
  });
  countBounded.push('one');
  countBounded.push('two');
  assert.throws(
    () => countBounded.push('three'),
    new RegExp(BROWSER_STARTUP_BACKLOG_EXCEEDED_ERROR_CODE),
  );

  const byteBounded = createBoundedLineBacklog({
    maxLines: 4,
    maxBytes: 5,
    source: 'test startup backlog',
  });
  byteBounded.push('12345');
  assert.throws(
    () => byteBounded.push('6'),
    new RegExp(BROWSER_STARTUP_BACKLOG_EXCEEDED_ERROR_CODE),
  );
  assert.deepEqual(byteBounded.drain(), ['12345']);
  assert.equal(byteBounded.bytes, 0);
});

test('bounded NDJSON decoder preserves a normal response larger than one MiB', () => {
  const decoder = createBoundedNdjsonDecoder({
    maxFrameBytes: BROWSER_PROTOCOL_FRAME_MAX_BYTES,
    source: 'test large response',
  });
  const response = JSON.stringify({
    jsonrpc: '2.0',
    id: 1,
    result: { content: 'x'.repeat(1024 * 1024 + 1) },
  });
  const lines = decoder.push(Buffer.from(`${response}\n`));

  assert.deepEqual(lines, [response]);
  assert.equal(decoder.pendingBytes, 0);
});

test('official browser tool retries use a conservative mutation allowlist', () => {
  for (const name of [
    'list_pages',
    'select_page',
    'take_snapshot',
    'wait_for',
    'get_console_message',
    'get_network_request',
    'list_console_messages',
    'list_network_requests',
    'performance_analyze_insight',
  ]) {
    assert.equal(browserToolMayMutate(name), false, `${name} should stay read-only`);
  }

  for (const name of [
    'click',
    'type_text',
    'navigate_page',
    'evaluate_script',
    'lighthouse_audit',
    'performance_start_trace',
    'unknown_future_tool',
  ]) {
    assert.equal(browserToolMayMutate(name), true, `${name} may mutate`);
  }
});

test('Host Core lifecycle retry classifier is exact and pre-dispatch only', () => {
  for (const value of [
    'browser/workspace-unavailable',
    'browser/workspace-unavailable: stopped by user',
    new Error('browser/workspace-missing task state'),
    new Error('browser/workspace-stopped\nrestart required'),
  ]) {
    assert.equal(isRecoverableHostCoreWorkspaceError(value), true);
  }

  for (const value of [
    'permission/browser-tool-disabled',
    'browser/control-lease-lost',
    'browser/user-takeover',
    'browser/native-surface-missing',
    'browser/workspace-unavailable-after-mutation',
    new Error('prefix browser/workspace-stopped'),
    null,
  ]) {
    assert.equal(isRecoverableHostCoreWorkspaceError(value), false);
  }
});

const WRAPPER_URL = new URL(
  '../src-tauri/resources/common/bundle/mcp-servers/browser-wrapper.mjs',
  import.meta.url,
);
const EXTRACTION_URL = new URL(
  '../src-tauri/src/features/runtime_bundle/platform/extraction.rs',
  import.meta.url,
);
const WRAPPER_PROTOCOL_URL = new URL(
  '../src-tauri/resources/common/bundle/mcp-servers/browser-wrapper-protocol.mjs',
  import.meta.url,
);
const BROWSER_MOD_URL = new URL(
  '../src-tauri/src/features/browser/mod.rs',
  import.meta.url,
);

test('persisted browser last-error codes stay synchronized across JavaScript and Rust', () => {
  assert.equal(Object.isFrozen(PERSISTED_BROWSER_LAST_ERROR_CODES), true);
  assert.equal(PERSISTED_BROWSER_LAST_ERROR_CODES.length, 8);
  assert.equal(
    new Set(PERSISTED_BROWSER_LAST_ERROR_CODES).size,
    PERSISTED_BROWSER_LAST_ERROR_CODES.length,
  );

  const rustSource = readFileSync(EXTRACTION_URL, 'utf8');
  const tableStart = rustSource.indexOf('pub(super) const BROWSER_LAST_ERROR_HINTS');
  const tableEnd = rustSource.indexOf('\n];', tableStart);
  assert.ok(tableStart >= 0 && tableEnd > tableStart, 'Rust hint table must be enumerable');
  const rustTable = rustSource.slice(tableStart, tableEnd + 3);
  const rustCodes = [...rustTable.matchAll(
    /\(\s*"([^"]+)"\s*,\s*"[^"]+"\s*,?\s*\)/g,
  )].map((match) => match[1]);
  assert.deepEqual(rustCodes, PERSISTED_BROWSER_LAST_ERROR_CODES);
  assert.equal(new Set(rustCodes).size, rustCodes.length);

  const wrapperSource = readFileSync(WRAPPER_URL, 'utf8');
  assert.match(wrapperSource, /PERSISTED_BROWSER_LAST_ERROR_CODES,/);
  assert.match(
    wrapperSource,
    /const PERSISTED_LAST_ERROR_CODES = new Set\(PERSISTED_BROWSER_LAST_ERROR_CODES\);/,
  );
  assert.doesNotMatch(
    wrapperSource,
    /const\s+PERSISTED_LAST_ERROR_CODES\s*=\s*new Set\(\s*\[/,
  );
});

test('Windows native workspace retains multi-tab tools and hides unusable screenshot tools', () => {
  const catalog = {
    toolsListResult: {
      tools: [
        { name: 'navigate_page' },
        { name: 'new_page' },
        { name: 'close_page' },
        { name: 'list_pages' },
        { name: 'select_page' },
        { name: 'take_screenshot' },
        { name: 'upload_file' },
      ],
    },
  };
  const adapted = adaptBrowserCatalog(catalog);
  assert.deepEqual(adapted.toolsListResult.tools.map((tool) => tool.name), [
    'navigate_page',
    'new_page',
    'close_page',
    'list_pages',
    'select_page',
  ]);
  const navigate = adapted.toolsListResult.tools.find((tool) => tool.name === 'navigate_page');
  const list = adapted.toolsListResult.tools.find((tool) => tool.name === 'list_pages');
  const create = adapted.toolsListResult.tools.find((tool) => tool.name === 'new_page');
  assert.equal(navigate.inputSchema.properties.pageId.type, 'number');
  assert.ok(navigate.inputSchema.required.includes('pageId'));
  assert.equal(list.inputSchema, undefined);
  assert.equal(create.inputSchema, undefined);
  assert.deepEqual(catalog.toolsListResult.tools.slice(-2), [
    { name: 'take_screenshot' },
    { name: 'upload_file' },
  ]);
});

test('disabled browser tools are rejected before wrapper startup or upstream proxying', () => {
  for (const name of ['take_screenshot', 'upload_file']) {
    const message = {
      jsonrpc: '2.0',
      id: 17,
      method: 'tools/call',
      params: { name, arguments: {} },
    };
    assert.throws(
      () => assertAllowedBrowserToolCall(message),
      {
        name: 'Error',
        message: `permission/browser-tool-disabled: ${name}`,
      },
    );
  }

  const allowed = {
    jsonrpc: '2.0',
    id: 18,
    method: 'tools/call',
    params: { name: 'click', arguments: {} },
  };
  assert.equal(assertAllowedBrowserToolCall(allowed), allowed);

  const source = readFileSync(WRAPPER_URL, 'utf8');
  const handleStart = source.indexOf('function handleLine(');
  const handleEnd = source.indexOf('\nfunction triggerStart(', handleStart);
  const handleBody = source.slice(handleStart, handleEnd);
  const guardAt = handleBody.indexOf('assertAllowedBrowserToolCall(msg)');
  assert.ok(guardAt >= 0, 'wrapper input boundary must call the disabled-tool guard');
  assert.ok(
    guardAt < handleBody.indexOf("if (state === 'proxy')"),
    'disabled-tool guard must run before upstream proxying',
  );
  assert.ok(
    guardAt < handleBody.indexOf("if (state === 'starting')"),
    'disabled-tool guard must run before startup buffering',
  );
  assert.ok(
    guardAt < handleBody.indexOf('handleShimRequest(msg, line)'),
    'disabled-tool guard must run before shim can trigger host startup',
  );
});

test('shim catalog exposes the upstream experimental pageId routing schema', () => {
  const catalog = {
    toolsListResult: {
      tools: [
        {
          name: 'click',
          inputSchema: {
            type: 'object',
            properties: { uid: { type: 'string' } },
            required: ['uid'],
            additionalProperties: true,
          },
        },
        {
          name: 'select_page',
          inputSchema: {
            type: 'object',
            properties: { pageId: { type: 'number', description: 'existing' } },
            required: ['pageId'],
          },
        },
        { name: 'list_pages', inputSchema: { type: 'object', properties: {} } },
        { name: 'new_page', inputSchema: { type: 'object', properties: { url: {} } } },
      ],
    },
  };
  const tools = adaptBrowserCatalog(catalog).toolsListResult.tools;
  const click = tools.find((tool) => tool.name === 'click');
  const select = tools.find((tool) => tool.name === 'select_page');
  assert.deepEqual(click.inputSchema.required, ['pageId', 'uid']);
  assert.equal(click.inputSchema.properties.uid.type, 'string');
  assert.deepEqual(select.inputSchema.required, ['pageId']);
  assert.deepEqual(tools.find((tool) => tool.name === 'list_pages').inputSchema.properties, {});
  assert.deepEqual(tools.find((tool) => tool.name === 'new_page').inputSchema.properties, { url: {} });
  assert.equal(catalog.toolsListResult.tools[0].inputSchema.properties.pageId, undefined);
});

test('browser host policy contains no external Chrome fallback', () => {
  assert.deepEqual(browserHostBackendPolicy('win32'), {
    action: 'request-native-host',
    backend: 'webview2',
    code: null,
    message: null,
  });
  assert.deepEqual(browserHostBackendPolicy('linux'), {
    action: 'request-browser-core',
    backend: 'webkitgtk',
    code: null,
    message: null,
  });
  assert.deepEqual(browserHostBackendPolicy('darwin'), {
    action: 'request-browser-core',
    backend: 'wkwebview',
    code: null,
    message: null,
  });
  for (const platform of ['freebsd']) {
    const policy = browserHostBackendPolicy(platform);
    assert.equal(policy.action, 'unsupported');
    assert.equal(policy.code, 'unsupported/host-backend-unavailable');
    assert.notEqual(policy.action, 'start-external-browser');
  }
});

test('ensureBrowserRunning control flow cannot call external-browser helpers', () => {
  const source = readFileSync(WRAPPER_URL, 'utf8');
  const start = source.indexOf('async function ensureBrowserRunning()');
  const end = source.indexOf('// MCP catalog', start);
  assert.ok(start >= 0 && end > start, 'ensureBrowserRunning implementation must be locatable');
  const body = source.slice(start, end);
  assert.match(body, /browserHostBackendPolicy\(process\.platform\)/);
  assert.doesNotMatch(body, /\b(?:startChrome|findChrome|pickFreePort)\s*\(/);
  assert.doesNotMatch(body, /owner\s*===\s*['"]mcp['"]/);
  assert.doesNotMatch(
    source,
    /\b(?:startChrome|findChrome|pickFreePort|writePortFile|killChromeChild)\s*\(/,
    'external-browser startup and cleanup helpers must be absent from product code',
  );
  assert.doesNotMatch(source, /PINVOU_BROWSER_CHROME_PATH/);
});

test('host requests use unique response paths, idempotency keys, and timeout tombstones', () => {
  const identity = {
    requestId: '123-456-a1b2c3d4',
    sessionId: 'session-a',
    sessionToken: '0123456789abcdef',
    callerPid: 4242,
    wrapperInstanceNonce: '0123456789abcdef0123456789abcdef',
  };
  assert.deepEqual(hostRequestArtifactNames(identity.sessionToken, identity.requestId), {
    request: '0123456789abcdef-123-456-a1b2c3d4.json',
    response: '0123456789abcdef-123-456-a1b2c3d4.response',
    cancelled: '0123456789abcdef-123-456-a1b2c3d4.cancelled',
  });

  const request = createHostRequestEnvelope({
    ...identity,
    operation: 'activate_tab',
    payload: {
      tab_token: 'fedcba9876543210',
      request_id: 'payload-must-not-override',
    },
    requestedAt: 100,
  });
  const tombstone = createHostCancellationTombstone({
    ...identity,
    reason: 'timeout',
    cancelledAt: 200,
  });
  assert.equal(HOST_REQUEST_PROTOCOL_VERSION, 3);
  assert.equal(request.protocol_version, 3);
  assert.equal(request.request_id, identity.requestId);
  assert.equal(request.idempotency_key, '0123456789abcdef/123-456-a1b2c3d4');
  assert.equal(request.tab_token, 'fedcba9876543210');
  assert.equal(request.caller_pid, identity.callerPid);
  assert.equal(request.wrapper_instance_nonce, identity.wrapperInstanceNonce);
  assert.equal(tombstone.kind, 'host_request_cancelled');
  assert.equal(tombstone.protocol_version, 3);
  assert.equal(tombstone.request_id, request.request_id);
  assert.equal(tombstone.idempotency_key, request.idempotency_key);
  assert.equal(tombstone.caller_pid, request.caller_pid);
  assert.equal(tombstone.wrapper_instance_nonce, request.wrapper_instance_nonce);

  assert.equal(HOST_CALLER_HEARTBEAT_INTERVAL_MS, 1_000);
  assert.equal(
    hostCallerHeartbeatArtifactName(identity.sessionToken, identity.wrapperInstanceNonce),
    '0123456789abcdef-0123456789abcdef0123456789abcdef.heartbeat',
  );
  assert.deepEqual(createHostCallerHeartbeat({
    ...identity,
    heartbeatAt: 300,
  }), {
    protocol_version: 3,
    kind: 'host_caller_heartbeat',
    session_id: identity.sessionId,
    session_token: identity.sessionToken,
    caller_pid: identity.callerPid,
    wrapper_instance_nonce: identity.wrapperInstanceNonce,
    heartbeat_at: 300,
  });
  assert.throws(
    () => createHostRequestEnvelope({
      ...identity,
      callerPid: 0,
      operation: 'prepare',
    }),
    /callerPid/,
  );
  assert.throws(
    () => createHostCancellationTombstone({
      ...identity,
      wrapperInstanceNonce: 'not-a-nonce',
    }),
    /wrapperInstanceNonce/,
  );

  const source = readFileSync(WRAPPER_URL, 'utf8');
  const timeoutStart = source.indexOf('function cancelTimedOutHostRequest(');
  const timeoutEnd = source.indexOf('async function requestHost(', timeoutStart);
  const timeoutBody = source.slice(timeoutStart, timeoutEnd);
  assert.ok(timeoutStart >= 0 && timeoutEnd > timeoutStart);
  assert.ok(
    timeoutBody.indexOf('atomicWriteJson(tombstonePath') < timeoutBody.indexOf('unlinkSync(requestPath)'),
    'timeout must publish the cancellation tombstone before withdrawing an unclaimed request',
  );
  assert.match(timeoutBody, /quarantineTimedOutHostResponse\(responsePath, tombstonePath\)/);
  const quarantineStart = source.indexOf('function quarantineTimedOutHostResponse(');
  const quarantineEnd = source.indexOf('function cancelTimedOutHostRequest(', quarantineStart);
  const quarantineBody = source.slice(quarantineStart, quarantineEnd);
  assert.ok(quarantineStart >= 0 && quarantineEnd > quarantineStart);
  assert.doesNotMatch(
    quarantineBody,
    /unlinkSync\(tombstonePath\)/,
    'the caller must not delete cancellation authority by TTL and must await host consumption',
  );
  const requestStart = source.indexOf('async function requestHost(');
  const requestEnd = source.indexOf('/**\n * The Windows main application owns', requestStart);
  const requestBody = source.slice(requestStart, requestEnd);
  assert.match(requestBody, /return parseHostResponseEnvelope\(response/);
  assert.doesNotMatch(requestBody, /response\?\.request_id != null/);
});

test('v3 host responses reject missing or mismatched identity fields', () => {
  const requestId = '123-456-a1b2c3d4';
  const idempotencyKey = `0123456789abcdef/${requestId}`;
  const response = {
    protocol_version: 3,
    request_id: requestId,
    idempotency_key: idempotencyKey,
    ok: true,
    result: { accepted: true },
  };
  assert.deepEqual(parseHostResponseEnvelope(response, {
    requestId,
    idempotencyKey,
    operation: 'assert_host_lease',
  }), { accepted: true });

  for (const field of ['protocol_version', 'request_id', 'idempotency_key']) {
    const invalid = { ...response };
    delete invalid[field];
    assert.throws(
      () => parseHostResponseEnvelope(invalid, {
        requestId,
        idempotencyKey,
        operation: 'assert_host_lease',
      }),
      new RegExp(field),
    );
  }
  assert.throws(
    () => parseHostResponseEnvelope({ ...response, request_id: 'wrong' }, {
      requestId,
      idempotencyKey,
      operation: 'assert_host_lease',
    }),
    /request_id/,
  );
  assert.throws(
    () => parseHostResponseEnvelope({ ...response, idempotency_key: 'wrong' }, {
      requestId,
      idempotencyKey,
      operation: 'assert_host_lease',
    }),
    /idempotency_key/,
  );
  assert.throws(
    () => parseHostResponseEnvelope({ ...response, ok: undefined }, {
      requestId,
      idempotencyKey,
      operation: 'assert_host_lease',
    }),
    /ok/,
  );
});

test('startup failure does not answer buffered requests cancelled during startup', () => {
  const lines = [
    JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'tools/call' }),
    JSON.stringify({ jsonrpc: '2.0', id: 2, method: 'tools/call' }),
    '{bad json',
  ];
  const pending = uncancelledBufferedRequests(lines, new Set([2]));
  assert.deepEqual(pending, [lines[0], lines[2]]);

  const source = readFileSync(WRAPPER_URL, 'utf8');
  const start = source.indexOf('async function startProxy()');
  const end = source.indexOf('function writeChildRaw(', start);
  const body = source.slice(start, end);
  assert.match(
    body,
    /const failed = uncancelledBufferedRequests\(bufferedLines\.drain\(\), cancelledIds\)/,
  );
  assert.ok(
    body.indexOf('const failed = uncancelledBufferedRequests') < body.indexOf('cancelledIds.clear()'),
    'the cancellation set must be cleared only after failed requests are filtered',
  );
});

test('structuredContent pages preserve stable pageIds', () => {
  const result = {
    structuredContent: {
      pages: [
        { id: 4, url: 'https://example.com', title: 'Example', selected: true },
        { id: 9, url: 'https://iana.org', title: 'IANA', selected: false },
      ],
    },
  };
  assert.deepEqual(parseBrowserPages(result), result.structuredContent.pages);
});

test('list_pages accepts authoritative host targetId fields', () => {
  assert.deepEqual(parseBrowserPages({
    structuredContent: {
      pages: [{
        id: 4,
        url: 'https://example.com',
        title: 'Example',
        selected: true,
        target_id: 'target-a',
      }],
    },
  }), [{
    id: 4,
    url: 'https://example.com',
    title: 'Example',
    selected: true,
    targetId: 'target-a',
  }]);
});

test('legacy MCP protocol parses pages from text results', () => {
  const pages = parseBrowserPages({
    content: [{
      type: 'text',
      text: '## Pages\n1: Example (https://example.com) [selected]\n2: about:blank',
    }],
  });
  assert.deepEqual(pages.map(({ id, url, selected }) => ({ id, url, selected })), [
    { id: 1, url: 'https://example.com', selected: true },
    { id: 2, url: 'about:blank', selected: false },
  ]);
});

test('first foreground new_page only reuses the host bootstrap blank page', () => {
  const sessionToken = '0123456789abcdef';
  const bootstrapWorkspace = {
    version: 2,
    mapping_authority: 'host',
    revision: 1,
    session_token: sessionToken,
    active_tab: sessionToken,
    tabs: [{ token: sessionToken, target_id: 'target-a' }],
  };
  const bootstrapPage = {
    id: 1,
    url: `about:blank#pinvou-session-${sessionToken}`,
    targetId: 'target-a',
  };

  assert.equal(isReusableBootstrapBlankPage({
    workspace: bootstrapWorkspace,
    page: bootstrapPage,
    pageToken: sessionToken,
  }), true);
  assert.equal(isReusableBootstrapBlankPage({
    workspace: bootstrapWorkspace,
    page: { ...bootstrapPage, url: 'about:blank' },
    pageToken: sessionToken,
  }), true);
  assert.equal(isReusableBootstrapBlankPage({
    workspace: bootstrapWorkspace,
    page: bootstrapPage,
    pageToken: sessionToken,
    background: true,
  }), false);
  assert.equal(isReusableBootstrapBlankPage({
    workspace: bootstrapWorkspace,
    page: { ...bootstrapPage, url: 'https://example.com' },
    pageToken: sessionToken,
  }), false);
  assert.equal(isReusableBootstrapBlankPage({
    workspace: {
      ...bootstrapWorkspace,
      tabs: [
        ...bootstrapWorkspace.tabs,
        { token: 'fedcba9876543210', target_id: 'target-b' },
      ],
    },
    page: bootstrapPage,
    pageToken: sessionToken,
  }), false);
  assert.equal(isReusableBootstrapBlankPage({
    workspace: {
      ...bootstrapWorkspace,
      active_tab: 'fedcba9876543210',
      tabs: [{ token: 'fedcba9876543210', target_id: 'target-b' }],
    },
    page: { id: 2, url: 'about:blank', targetId: 'target-b' },
    pageToken: 'fedcba9876543210',
  }), false, 'a normal user-created blank tab must not be overwritten as a bootstrap placeholder');
});

test('list_pages only returns tabs allowed for the current conversation', () => {
  const result = {
    content: [{ type: 'text', text: '## Pages\n1: A (https://a.test)\n2: B (https://b.test) [selected]\n3: C (https://c.test)' }],
    structuredContent: {
      pages: [
        { id: 1, url: 'https://a.test', title: 'A', selected: false },
        { id: 2, url: 'https://b.test', title: 'B', selected: true },
        { id: 3, url: 'https://c.test', title: 'C', selected: false },
      ],
    },
  };
  const filtered = filterPagesResult(result, new Set([1, 3]), 3);
  assert.deepEqual(filtered.structuredContent.pages.map((page) => page.id), [1, 3]);
  assert.equal(filtered.structuredContent.pages[1].selected, true);
  assert.match(filtered.content[0].text, /1: A/);
  assert.doesNotMatch(filtered.content[0].text, /2: B/);
  assert.match(filtered.content[0].text, /3: C.*\[selected\]/);
});

test('wrapper filters the same list_pages result used for target joining', () => {
  const source = readFileSync(WRAPPER_URL, 'utf8');
  const helperStart = source.indexOf('function filteredPagesResult(');
  const helperEnd = source.indexOf('async function verifyHostedPageAlignment(', helperStart);
  const helperBody = source.slice(helperStart, helperEnd);
  assert.match(helperBody, /filterPagesResult\(\s*listResult,/);
  assert.doesNotMatch(
    helperBody,
    /callUpstreamTool\(\s*'list_pages'/,
    'filtering must not take a second sample after target-to-pageId discovery',
  );

  const routeStart = source.indexOf('async function routeHostedToolCall(');
  const listStart = source.indexOf("if (name === 'list_pages')", routeStart);
  const selectStart = source.indexOf("if (name === 'select_page')", listStart);
  const listBody = source.slice(listStart, selectStart);
  assert.match(listBody, /const \{ listResult \} = await syncWorkspacePagesBeforeDispatch/);
  assert.match(listBody, /return filteredPagesResult\(listResult\)/);
});

test('bootstrap conversation and newly embedded tab markers are recognized', () => {
  const result = {
    structuredContent: {
      pages: [
        { id: 1, url: 'about:blank#pinvou-session-0123456789abcdef' },
        { id: 2, url: 'about:blank#pinvou-tab-fedcba9876543210' },
      ],
    },
  };
  assert.equal(findHostedSessionPage(result, '0123456789abcdef')?.id, 1);
  assert.equal(findHostedTabPage(result, 'fedcba9876543210')?.id, 2);
  assert.equal(findHostedTabPage(result, '../unsafe'), null);
});

test('MCP restart restores navigated pages from the active host target without URL markers', () => {
  const workspace = {
    version: 2,
    mapping_authority: 'host',
    revision: 11,
    session_token: '0123456789abcdef',
    active_tab: 'fedcba9876543210',
    tabs: [
      { token: '0123456789abcdef', target_id: 'target-a' },
      { token: 'fedcba9876543210', target_id: 'target-b' },
    ],
  };
  const result = {
    structuredContent: {
      pages: [
        { id: 4, target_id: 'target-a', url: 'https://a.test', selected: false },
        { id: 9, target_id: 'target-b', url: 'https://b.test', selected: true },
      ],
    },
  };
  assert.equal(
    findHostedWorkspacePage(result, workspace, '0123456789abcdef')?.id,
    9,
  );
  assert.throws(
    () => findHostedWorkspacePage({
      structuredContent: {
        pages: [
          { id: 9, target_id: 'target-b', url: 'https://b.test' },
          { id: 10, target_id: 'target-b', url: 'https://duplicate.test' },
        ],
      },
    }, workspace, '0123456789abcdef'),
    /duplicate target_id/,
  );
});

test('only verified host page mappings survive after URL fragments disappear', () => {
  const result = {
    structuredContent: {
      pages: [
        { id: 7, url: 'about:blank' },
        { id: 8, url: 'https://example.com' },
      ],
    },
  };
  const pageTokens = new Map([[7, 'fedcba9876543210']]);
  assert.equal(findHostedTabPage(result, 'fedcba9876543210', pageTokens)?.id, 7);
  assert.equal(findHostedTabPage(result, '0123456789abcdef', pageTokens), null);
});

test('remote URL markers and mismatched structured targets cannot impersonate host pages', () => {
  const sessionToken = '0123456789abcdef';
  const tabToken = 'fedcba9876543210';
  assert.equal(findHostedSessionPage({
    structuredContent: {
      pages: [{ id: 1, url: `https://evil.example/#pinvou-session-${sessionToken}` }],
    },
  }, sessionToken), null);
  assert.equal(findHostedTabPage({
    structuredContent: {
      pages: [{ id: 2, url: `https://evil.example/#pinvou-tab-${tabToken}` }],
    },
  }, tabToken), null);

  const workspace = {
    version: 2,
    mapping_authority: 'host',
    revision: 3,
    session_token: sessionToken,
    active_tab: sessionToken,
    tabs: [{ token: sessionToken, target_id: 'host-target' }],
  };
  assert.equal(findHostedWorkspacePage({
    structuredContent: {
      pages: [{
        id: 3,
        target_id: 'foreign-target',
        url: `about:blank#pinvou-session-${sessionToken}`,
      }],
    },
  }, workspace, sessionToken), null);

  const source = readFileSync(WRAPPER_URL, 'utf8');
  const discoverStart = source.indexOf('async function discoverWorkspacePages(');
  const discoverEnd = source.indexOf('function pageIdForToken(', discoverStart);
  const discoverBody = source.slice(discoverStart, discoverEnd);
  assert.match(discoverBody, /if \(page\.targetId\) continue;/);
  assert.match(discoverBody, /page\.url === `about:blank#pinvou-session-/);
  assert.doesNotMatch(discoverBody, /page\.url\.includes\(`/);
});

test('wrapper never writes session or tab tokens into the remote page main world', () => {
  const source = readFileSync(WRAPPER_URL, 'utf8');
  assert.doesNotMatch(source, /__PINVOU_BROWSER_TAB_TOKEN__|markerInitScript/);
});

test('wrapper runtime strictly consumes v2 mappings and fully wires the host lease protocol', () => {
  const source = readFileSync(WRAPPER_URL, 'utf8');
  const readStart = source.indexOf('function readWorkspaceState()');
  const readEnd = source.indexOf('async function requestHostedOperation(', readStart);
  const readBody = source.slice(readStart, readEnd);
  assert.match(readBody, /return parseAuthoritativeHostWorkspace\(value, SESSION_TOKEN\)/);
  assert.doesNotMatch(readBody, /value\?\.version === 1/);

  const routeStart = source.indexOf('async function runOnVisibleHostedPage(');
  const routeEnd = source.indexOf('async function routeHostedToolCall(', routeStart);
  const routeBody = source.slice(routeStart, routeEnd);
  for (const operation of [
    'activate_tab',
    'assert_host_lease',
    'begin_agent_operation',
    'refresh_agent_input',
    'end_agent_operation',
  ]) {
    assert.match(
      routeBody,
      new RegExp(`requestHostedOperation\\([\\s\\S]{0,40}'${operation}'`),
    );
  }
  assert.match(routeBody, /emits_trusted_input: emitsInput/);
  assert.match(routeBody, /observational_only: observationalOnly/);
  assert.match(routeBody, /onRefreshFailure:[\s\S]{0,500}signalManagedUpstreamCancellation/);
  assert.match(routeBody, /emitsTrustedInput \? 'refresh_agent_input' : 'refresh_agent_operation'/);
  assert.match(
    routeBody,
    /heartbeatIntervalMs: emitsTrustedInput[\s\S]{0,120}WINDOWS_AGENT_OPERATION_HEARTBEAT_INTERVAL_MS/,
  );
  assert.match(
    source,
    /observationalOnly: !browserToolMayMutate\(name\)/,
    'only the explicit observation allowlist may run while user navigation is pending',
  );
  const inputWindowMs = Number(
    source.match(/const WINDOWS_TRUSTED_INPUT_WINDOW_MS = (\d+);/)?.[1],
  );
  const heartbeatIntervalMs = Number(
    source.match(/const WINDOWS_TRUSTED_INPUT_HEARTBEAT_INTERVAL_MS = (\d+);/)?.[1],
  );
  const timeoutReserveMs = Number(
    source.match(/WINDOWS_TRUSTED_INPUT_HEARTBEAT_INTERVAL_MS - (\d+);/)?.[1],
  );
  assert.equal(inputWindowMs, 750);
  assert.ok(
    heartbeatIntervalMs + (inputWindowMs - heartbeatIntervalMs - timeoutReserveMs) <
      inputWindowMs,
    'refresh timeout must fail before the active trusted-input window expires',
  );
  const operationWindowMs = Number(
    source.match(/const WINDOWS_AGENT_OPERATION_WINDOW_MS = ([\d_]+);/)?.[1]?.replaceAll('_', ''),
  );
  const operationHeartbeatIntervalMs = Number(
    source.match(
      /const WINDOWS_AGENT_OPERATION_HEARTBEAT_INTERVAL_MS = ([\d_]+);/,
    )?.[1]?.replaceAll('_', ''),
  );
  const operationRefreshCapMs = Number(
    source.match(
      /const WINDOWS_AGENT_OPERATION_REFRESH_TIMEOUT_MS = Math\.min\(\s*([\d_]+),/,
    )?.[1]?.replaceAll('_', ''),
  );
  assert.equal(operationHeartbeatIntervalMs, 5_000);
  assert.ok(
    operationHeartbeatIntervalMs + operationRefreshCapMs <= operationWindowMs,
    'generic refresh interval plus timeout must stay inside the operation window',
  );

  const hostCoreStart = source.indexOf('async function requestHostedBrowserCoreTool(');
  const hostCoreEnd = source.indexOf('function queueHostedBrowserCoreCall(', hostCoreStart);
  assert.doesNotMatch(
    source.slice(hostCoreStart, hostCoreEnd),
    /refresh_agent_input/,
    'Host Core/Linux/macOS must keep their per-native-dispatch refresh path',
  );

  const handshakeStart = source.indexOf('const onData = (chunk) =>');
  const handshakeEnd = source.indexOf("child.stdout.on('data', onData)", handshakeStart);
  const handshakeBody = source.slice(handshakeStart, handshakeEnd);
  assert.match(handshakeBody, /findHostedWorkspacePage\(msg\.result, workspace, SESSION_TOKEN\)/);
  assert.doesNotMatch(handshakeBody, /findHostedSessionPage\(msg\.result/);

  const newStart = source.indexOf("if (name === 'new_page')");
  const closeStart = source.indexOf("if (name === 'close_page')", newStart);
  const newBody = source.slice(newStart, closeStart);
  assert.match(newBody, /isReusableBootstrapBlankPage\(/);
  assert.match(
    newBody,
    /runOnVisibleHostedPage\([\s\S]*?callUpstreamTool\(\s*'navigate_page'/,
    'bootstrap blank-page reuse must navigate within the visible-page lease',
  );
  assert.match(newBody, /The host performs initial URL navigation inside an unpublished staging tab/);
  const createStart = newBody.indexOf('const creationAuthorization');
  assert.doesNotMatch(
    newBody.slice(createStart),
    /callUpstreamTool\(\s*'navigate_page'/,
    'the lease is stale after physical tab creation, so the wrapper must not perform initial target navigation',
  );
});

test('pageId and tabToken mapping is bijective and rejects duplicate first-match selection', () => {
  const { pageToToken, tokenToPage } = buildBijectivePageTokenMaps([
    [7, '0123456789abcdef'],
    [8, 'fedcba9876543210'],
  ]);
  assert.equal(pageToToken.get(7), '0123456789abcdef');
  assert.equal(tokenToPage.get('fedcba9876543210'), 8);

  assert.throws(
    () => buildBijectivePageTokenMaps([
      [7, '0123456789abcdef'],
      [7, 'fedcba9876543210'],
    ]),
    /duplicate pageId/,
  );
  assert.throws(
    () => buildBijectivePageTokenMaps([
      [7, '0123456789abcdef'],
      [8, '0123456789abcdef'],
    ]),
    /duplicate tabToken/,
  );
  assert.throws(
    () => buildBijectivePageTokenMaps([
      [7, '0123456789abcdef'],
      [7, '0123456789abcdef'],
    ]),
    /duplicate pageId/,
  );
});

test('explicit pageId must pass current-conversation ownership before synchronization or selection', () => {
  const pages = new Map([[7, '0123456789abcdef']]);
  assert.equal(explicitOwnedPageId({}, pages), null);
  assert.equal(explicitOwnedPageId({ pageId: 7 }, pages), 7);
  assert.throws(() => explicitOwnedPageId({ pageId: '7' }, pages), /does not belong to this conversation/);
  assert.throws(() => explicitOwnedPageId({ pageId: 8 }, pages), /does not belong to this conversation/);

  const source = readFileSync(WRAPPER_URL, 'utf8');
  const ordinaryStart = source.indexOf('// Before every ordinary page-scoped tool');
  const ordinaryEnd = source.indexOf('// Handshake IDs used with the MCP child process', ordinaryStart);
  const ordinaryBody = source.slice(ordinaryStart, ordinaryEnd);
  assert.ok(
    ordinaryBody.indexOf('explicitOwnedPageId(args, pageIdToTabToken)') <
      ordinaryBody.indexOf('await syncWorkspacePagesBeforeDispatch(false, msg.id)'),
    'explicit pageId ownership must precede any page synchronization or selection',
  );
  assert.match(
    ordinaryBody,
    /const requestedTabToken =[\s\S]*?await syncWorkspacePagesBeforeDispatch[\s\S]*?pageIdToTabToken\.get\(requestedPageId\) !== requestedTabToken/,
    'a different tab reusing the same numeric pageId must be rejected after synchronization',
  );

  const routeStart = source.indexOf('async function routeHostedToolCall(');
  const selectStart = source.indexOf("if (name === 'select_page')", routeStart);
  const newStart = source.indexOf("if (name === 'new_page')", selectStart);
  const selectBody = source.slice(selectStart, newStart);
  assert.match(
    selectBody,
    /const selectedTabToken =[\s\S]*?await syncWorkspacePagesBeforeDispatch[\s\S]*?pageIdToTabToken\.get\(selectedPage\) !== selectedTabToken/,
  );

  const closeStart = source.indexOf("if (name === 'close_page')", newStart);
  const closeEnd = source.indexOf('// Before every ordinary page-scoped tool', closeStart);
  const closeBody = source.slice(closeStart, closeEnd);
  assert.match(
    closeBody,
    /const closingTabToken =[\s\S]*?await syncWorkspacePagesBeforeDispatch[\s\S]*?pageIdToTabToken\.get\(closingPageId\) !== closingTabToken/,
  );
});

test('authoritative v2 host mappings must be complete and never fall back to page markers', () => {
  const workspace = parseAuthoritativeHostWorkspace({
    version: 2,
    mapping_authority: 'host',
    revision: 9,
    session_token: '0123456789abcdef',
    active_tab: 'fedcba9876543210',
    tabs: [
      { token: '0123456789abcdef', target_id: 'target-a' },
      { token: 'fedcba9876543210', target_id: 'target-b' },
    ],
  }, '0123456789abcdef');
  assert.equal(workspace.tabs[1].target_id, 'target-b');

  assert.throws(
    () => parseAuthoritativeHostWorkspace({
      version: 1,
      revision: 9,
      session_token: '0123456789abcdef',
      active_tab: '0123456789abcdef',
      tabs: [{ token: '0123456789abcdef' }],
    }),
    /did not provide an authoritative v2 target mapping/,
  );
  assert.throws(
    () => parseAuthoritativeHostWorkspace({
      version: 2,
      mapping_authority: 'host',
      revision: 9,
      session_token: '0123456789abcdef',
      active_tab: '0123456789abcdef',
      tabs: [{ token: '0123456789abcdef' }],
    }),
    /missing an authoritative target_id/,
  );
  assert.throws(
    () => parseAuthoritativeHostWorkspace({
      version: 2,
      mapping_authority: 'host',
      revision: 9,
      session_token: '0123456789abcdef',
      active_tab: '0123456789abcdef',
      tabs: [
        { token: '0123456789abcdef', target_id: 'same-target' },
        { token: 'fedcba9876543210', target_id: 'same-target' },
      ],
    }),
    /duplicate target_id/,
  );
});

test('activate_tab lease schema strictly generates assert_host_lease arguments', () => {
  const lease = parseHostActivationLease({
    sessionId: 'session-a',
    tabToken: '0123456789abcdef',
    targetId: 'target-a',
    revision: 12,
    owner: 'agent',
    lease: '0123456789abcdef0123456789abcdef',
  }, {
    sessionId: 'session-a',
    tabToken: '0123456789abcdef',
    targetId: 'target-a',
  });
  assert.deepEqual(hostLeaseAssertionPayload(lease), {
    tab_token: '0123456789abcdef',
    target_id: 'target-a',
    revision: 12,
    lease: '0123456789abcdef0123456789abcdef',
  });
  assert.throws(
    () => parseHostActivationLease({
      sessionId: 'session-a',
      tabToken: '0123456789abcdef',
      targetId: 'target-a',
      revision: 12,
      owner: 'agent',
    }),
    /missing a dispatch lease/,
  );
  assert.throws(
    () => parseHostActivationLease({
      sessionId: 'session-a',
      tabToken: '0123456789abcdef',
      targetId: 'wrong-target',
      revision: 12,
      owner: 'agent',
      lease: '0123456789abcdef0123456789abcdef',
    }, { targetId: 'target-a' }),
    /targetId does not match the host mapping/,
  );
  assert.throws(
    () => parseHostActivationLease({
      sessionId: 'session-a',
      tabToken: '0123456789abcdef',
      targetId: 'target-a',
      revision: 12,
      owner: 'user',
      lease: '0123456789abcdef0123456789abcdef',
    }),
    /did not grant control to the Agent/,
  );
});

test('create and close mutations use authorization_tab_token and bind creationId to request_id', () => {
  const lease = {
    sessionId: 'session-a',
    tabToken: '0123456789abcdef',
    targetId: 'target-a',
    revision: 12,
    owner: 'agent',
    lease: '0123456789abcdef0123456789abcdef',
  };
  assert.deepEqual(hostMutationAuthorizationPayload(lease), {
    authorization_tab_token: '0123456789abcdef',
    target_id: 'target-a',
    revision: 12,
    lease: '0123456789abcdef0123456789abcdef',
  });

  const requestId = '123-456-a1b2c3d4';
  const result = {
    tabToken: 'fedcba9876543210',
    targetId: 'target-new',
    creationId: requestId,
  };
  assert.deepEqual(parseCreatedTabResult(result, {
    tabToken: 'fedcba9876543210',
    creationId: requestId,
  }), result);
  assert.deepEqual(parseHostResponseEnvelope({
    protocol_version: 3,
    request_id: requestId,
    idempotency_key: `0123456789abcdef/${requestId}`,
    ok: true,
    result,
  }, {
    requestId,
    idempotencyKey: `0123456789abcdef/${requestId}`,
    operation: 'create_tab',
    requestedTabToken: 'fedcba9876543210',
  }), result);
  assert.throws(
    () => parseHostResponseEnvelope({
      protocol_version: 3,
      request_id: requestId,
      idempotency_key: `0123456789abcdef/${requestId}`,
      ok: true,
      result: { ...result, creationId: 'another-request' },
    }, {
      requestId,
      idempotencyKey: `0123456789abcdef/${requestId}`,
      operation: 'create_tab',
      requestedTabToken: 'fedcba9876543210',
    }),
    /creationId does not match request_id/,
  );
  assert.throws(
    () => parseCreatedTabResult({
      tab_token: result.tabToken,
      target_id: result.targetId,
      creation_id: result.creationId,
    }, {
      tabToken: result.tabToken,
      creationId: requestId,
    }),
    /tabToken/,
  );
});

test('new_page and close_page retain v3 CAS wiring and exact compensation', () => {
  const source = readFileSync(WRAPPER_URL, 'utf8');
  const newStart = source.indexOf("if (name === 'new_page')");
  const closeStart = source.indexOf("if (name === 'close_page')", newStart);
  const newBody = source.slice(newStart, closeStart);
  assert.ok(newStart >= 0 && closeStart > newStart);
  assert.ok(
    newBody.indexOf('const creationAuthorization = await runOnVisibleHostedPage') <
      newBody.indexOf("'create_tab'"),
    'create_tab must first activate the current active page and acquire its lease',
  );
  assert.match(newBody, /\.\.\.hostMutationAuthorizationPayload\(creationAuthorization\.activationResult\)/);
  assert.match(
    newBody,
    /runLeasedHostDispatch\([\s\S]*?refreshOperation:[\s\S]*?'refresh_agent_operation'[\s\S]*?requestHostedOperation\(\s*'create_tab'/,
    'create_tab mutation must remain inside the generic operation heartbeat window',
  );
  const createStart = newBody.indexOf("'create_tab'");
  const createEnd = newBody.indexOf('workspaceRevision = -1', createStart);
  const createCall = newBody.slice(createStart, createEnd);
  assert.match(createCall, /url: args\.url/);
  assert.match(createCall, /background: args\.background === true/);
  assert.doesNotMatch(createCall, /creation_id\s*:/);
  assert.match(
    createCall,
    /creationId\s*,\s*\n\s*\(\) => cancelledProxyRequestIds\.has\(msg\.id\)/,
  );
  assert.match(newBody, /authoritativeTarget !== createdTab\.targetId/);
  assert.match(newBody, /'rollback_created_tab'/);
  assert.match(newBody, /creation_id: creationId/);
  assert.match(newBody, /!rollbackProved[\s\S]*?hostMutationCommitUnknownOutcome/);
  const catchStart = newBody.indexOf('} catch (error)');
  const compensationBody = newBody.slice(catchStart);
  assert.doesNotMatch(compensationBody, /requestHostedOperation\('close_tab'/);

  const ordinaryStart = source.indexOf('// Before every ordinary page-scoped tool', closeStart);
  const closeBody = source.slice(closeStart, ordinaryStart);
  assert.match(closeBody, /tab_token: closingToken/);
  assert.match(closeBody, /\.\.\.hostMutationAuthorizationPayload\(aligned\.activationResult\)/);
  assert.match(closeBody, /refreshOperation:[\s\S]*?'refresh_agent_operation'/);
  assert.match(closeBody, /heartbeatIntervalMs: WINDOWS_AGENT_OPERATION_HEARTBEAT_INTERVAL_MS/);
  assert.match(closeBody, /\(\) => cancelledProxyRequestIds\.has\(msg\.id\)/);
});

test('leased dispatch always closes the host operation window after failure or cancellation', async (t) => {
  const activationLease = {
    sessionId: 'session-a',
    tabToken: '0123456789abcdef',
    targetId: 'target-a',
    revision: 12,
    owner: 'agent',
    lease: '0123456789abcdef0123456789abcdef',
  };
  await t.test('closes after tool execution fails', async () => {
    const events = [];
    await assert.rejects(
      runLeasedHostDispatch({
        activationLease,
        emitsTrustedInput: true,
        ensureActive: () => events.push('active'),
        beginOperation: async ({ lease, emitsTrustedInput }) => {
          events.push(`begin:${lease.lease}:${emitsTrustedInput}`);
        },
        execute: async () => {
          events.push('execute');
          throw new Error('dispatch failed');
        },
        endOperation: async (lease) => events.push(`end:${lease.lease}`),
      }),
      /dispatch failed/,
    );
    assert.deepEqual(events, [
      'active',
      'begin:0123456789abcdef0123456789abcdef:true',
      'active',
      'execute',
      'end:0123456789abcdef0123456789abcdef',
    ]);
  });

  await t.test('closes without executing after cancellation follows begin', async () => {
    const events = [];
    let checks = 0;
    await assert.rejects(
      runLeasedHostDispatch({
        activationLease,
        ensureActive: () => {
          checks += 1;
          events.push('active');
          if (checks === 2) throw new Error('cancelled');
        },
        beginOperation: async () => events.push('begin'),
        execute: async () => events.push('execute'),
        endOperation: async () => events.push('end'),
      }),
      /cancelled/,
    );
    assert.deepEqual(events, ['active', 'begin', 'active', 'end']);
  });

  await t.test('best-effort revokes the same lease when begin acknowledgement is lost', async () => {
    const events = [];
    await assert.rejects(
      runLeasedHostDispatch({
        activationLease,
        ensureActive: () => events.push('active'),
        beginOperation: async () => {
          events.push('begin');
          throw new Error('begin acknowledgement lost');
        },
        execute: async () => events.push('execute'),
        endOperation: async () => events.push('end'),
      }),
      /begin acknowledgement lost/,
    );
    assert.deepEqual(events, ['active', 'begin', 'end']);
  });

  await t.test('preserves a committed tool result when end cleanup only reports a warning', async () => {
    const events = [];
    let executions = 0;
    const result = await runLeasedHostDispatch({
      activationLease,
      emitsTrustedInput: true,
      ensureActive: () => events.push('active'),
      beginOperation: async () => events.push('begin'),
      execute: async () => {
        executions += 1;
        events.push('execute');
        return { content: [{ type: 'text', text: 'clicked' }] };
      },
      endOperation: async () => {
        events.push('end');
        throw new Error('cleanup transport failed');
      },
      onEndFailure: async (error, lease, state) => {
        events.push(`warning:${error.message}:${lease.lease}:${state.executionSucceeded}`);
      },
    });
    assert.equal(result.content[0].text, 'clicked');
    assert.equal(executions, 1);
    assert.deepEqual(events, [
      'active',
      'begin',
      'active',
      'execute',
      'end',
      'warning:cleanup transport failed:0123456789abcdef0123456789abcdef:true',
    ]);
  });

  await t.test('renews non-input tool leases and fully stops heartbeat before end', async () => {
    const events = [];
    let refreshes = 0;
    const result = await runLeasedHostDispatch({
      activationLease,
      emitsTrustedInput: false,
      heartbeatIntervalMs: 1,
      ensureActive: () => {},
      beginOperation: async () => events.push('begin'),
      refreshOperation: async () => {
        refreshes += 1;
        events.push('refresh');
      },
      execute: async () => {
        events.push('execute');
        await new Promise((resolve) => setTimeout(resolve, 10));
        return { content: [{ type: 'text', text: 'snapshot' }], isError: false };
      },
      endOperation: async () => events.push('end'),
    });
    assert.equal(result.content[0].text, 'snapshot');
    assert.ok(refreshes >= 1, 'a non-input operation must receive a generic heartbeat');
    assert.ok(events.lastIndexOf('refresh') < events.lastIndexOf('end'));
    const settledRefreshes = refreshes;
    await new Promise((resolve) => setTimeout(resolve, 5));
    assert.equal(refreshes, settledRefreshes, 'no heartbeat may outlive endOperation');
  });

  await t.test('preserves a committed result when heartbeat fails after upstream success', async () => {
    let executions = 0;
    const result = await runLeasedHostDispatch({
      activationLease,
      emitsTrustedInput: true,
      heartbeatIntervalMs: 1,
      ensureActive: () => {},
      beginOperation: async () => {},
      refreshOperation: async () => {
        throw new Error('browser/agent-input-refresh-rejected');
      },
      execute: async () => {
        executions += 1;
        await new Promise((resolve) => setTimeout(resolve, 10));
        return { content: [{ type: 'text', text: 'clicked' }], isError: false };
      },
      endOperation: async () => {},
    });
    assert.equal(executions, 1);
    assert.equal(result.content[0].text, 'clicked');
    assert.equal(result.isError, false);
  });

  await t.test('returns a non-replayable unknown outcome when heartbeat and upstream cancellation fail', async () => {
    let executions = 0;
    const result = await runLeasedHostDispatch({
      activationLease,
      emitsTrustedInput: true,
      heartbeatIntervalMs: 1,
      ensureActive: () => {},
      beginOperation: async () => {},
      refreshOperation: async () => {
        throw new Error('browser/agent-input-refresh-rejected');
      },
      execute: async () => {
        executions += 1;
        await new Promise((resolve) => setTimeout(resolve, 10));
        throw new Error('upstream cancelled');
      },
      endOperation: async () => {},
    });
    assert.equal(executions, 1);
    assert.equal(result.isError, true);
    assert.equal(
      result.structuredContent.errorCode,
      'browser/action-commit-unknown-after-authorization-loss',
    );
    assert.equal(result.structuredContent.actionCommitted, true);
    assert.equal(result.structuredContent.actionMayHaveCommitted, true);
    assert.equal(result.structuredContent.retryable, false);
    assert.match(result.content[0].text, /Do not repeat the action/);
  });

  await t.test('marks upstream tool errors unknown and non-replayable after heartbeat failure', async () => {
    const result = await runLeasedHostDispatch({
      activationLease,
      emitsTrustedInput: true,
      heartbeatIntervalMs: 1,
      ensureActive: () => {},
      beginOperation: async () => {},
      refreshOperation: async () => {
        throw new Error('browser/agent-input-refresh-rejected');
      },
      execute: async () => {
        await new Promise((resolve) => setTimeout(resolve, 10));
        return { content: [{ type: 'text', text: 'upstream tool failed' }], isError: true };
      },
      endOperation: async () => {},
    });
    assert.equal(result.isError, true);
    assert.equal(
      result.structuredContent.errorCode,
      'browser/action-commit-unknown-after-authorization-loss',
    );
    assert.equal(result.structuredContent.retryable, false);
    assert.match(result.content[0].text, /Do not repeat the action/);
  });
});

test('page-scoped tools in experimental page-id-routing are recognized', () => {
  const names = pageScopedToolNames({
    tools: [
      {
        name: 'navigate_page',
        inputSchema: { properties: { pageId: { type: 'number' } }, required: ['pageId'] },
      },
      {
        name: 'list_pages',
        inputSchema: { properties: {}, required: [] },
      },
    ],
  });
  assert.deepEqual([...names], ['navigate_page']);
});

test('upstream tool annotations identify operations requiring trusted-input suppression', () => {
  const names = inputToolNames({
    tools: [
      { name: 'click', annotations: { category: 'input' } },
      { name: 'type_text', annotations: { category: 'input' } },
      { name: 'navigate_page', annotations: { category: 'navigation' } },
    ],
  });
  assert.deepEqual([...names], ['click', 'type_text']);
});

test('page-scoped tools inject the current conversation pageId without mutating arguments', () => {
  const message = {
    jsonrpc: '2.0',
    id: 8,
    method: 'tools/call',
    params: {
      name: 'navigate_page',
      arguments: { type: 'url', url: 'https://example.com', initScript: 'userScript()' },
    },
  };
  const routed = routeToolCallToPage(message, 17, { initScript: 'patchedScript();\nuserScript()' });
  assert.equal(routed.params.arguments.pageId, 17);
  assert.equal(routed.params.arguments.url, 'https://example.com');
  assert.equal(routed.params.arguments.initScript, 'patchedScript();\nuserScript()');
  assert.equal(message.params.arguments.pageId, undefined, 'the original request must not be mutated');
});

test('explicit background pageId activates host tab, selects Target, and verifies before execution', async () => {
  const events = [];
  const pageTokens = new Map([
    [11, 'aaaaaaaaaaaaaaaa'],
    [22, 'bbbbbbbbbbbbbbbb'],
  ]);
  const result = await runVisiblePageOperation({
    pageId: 22,
    pageTokens,
    ensureActive: () => events.push('active'),
    activateTab: async (tabToken) => {
      events.push(`host:${tabToken}`);
      return { lease: 'lease-a' };
    },
    assertLease: async ({ phase }) => events.push(`assert:${phase}`),
    selectPage: async (pageId) => {
      events.push(`select:${pageId}`);
      return { selected: pageId };
    },
    verify: async ({ pageId, tabToken }) => {
      events.push(`verify:${pageId}:${tabToken}`);
      return { aligned: true };
    },
    execute: async ({ pageId, tabToken }) => {
      events.push(`execute:${pageId}:${tabToken}`);
      return { ok: true };
    },
  });

  assert.deepEqual(events, [
    'active',
    'host:bbbbbbbbbbbbbbbb',
    'active',
    'assert:select',
    'select:22',
    'active',
    'assert:verify',
    'verify:22:bbbbbbbbbbbbbbbb',
    'active',
    'execute:22:bbbbbbbbbbbbbbbb',
  ]);
  assert.deepEqual(result.executionResult, { ok: true });
});

test('ownership, host activation, Target selection, and verification failures prevent execution', async (t) => {
  await t.test('rejects cross-conversation pageId before side effects', async () => {
    let called = false;
    await assert.rejects(
      runVisiblePageOperation({
        pageId: 99,
        pageTokens: new Map([[1, 'aaaaaaaaaaaaaaaa']]),
        activateTab: async () => { called = true; },
        selectPage: async () => { called = true; },
        verify: async () => { called = true; },
        execute: async () => { called = true; },
      }),
      /does not belong to this conversation/,
    );
    assert.equal(called, false);
  });

  await t.test('does not select a Target when host lease verification fails', async () => {
    const events = [];
    await assert.rejects(
      runVisiblePageOperation({
        pageId: 7,
        pageTokens: new Map([[7, 'aaaaaaaaaaaaaaaa']]),
        activateTab: async () => {
          events.push('activate');
          return { lease: 'invalidated' };
        },
        assertLease: async () => {
          events.push('assert');
          throw new Error('lease invalid');
        },
        selectPage: async () => events.push('select'),
        verify: async () => events.push('verify'),
        execute: async () => events.push('execute'),
      }),
      /lease invalid/,
    );
    assert.deepEqual(events, ['activate', 'assert']);
  });

  for (const failedPhase of ['activate', 'select', 'verify']) {
    await t.test(`${failedPhase} failure does not execute`, async () => {
      const events = [];
      const fail = (phase) => {
        events.push(phase);
        if (phase === failedPhase) throw new Error(`${phase} failed`);
      };
      await assert.rejects(
        runVisiblePageOperation({
          pageId: 7,
          pageTokens: new Map([[7, 'aaaaaaaaaaaaaaaa']]),
          activateTab: async () => fail('activate'),
          selectPage: async () => fail('select'),
          verify: async () => fail('verify'),
          execute: async () => events.push('execute'),
        }),
        new RegExp(`${failedPhase} failed`),
      );
      assert.doesNotMatch(events.join(','), /execute/);
    });
  }
});

test('queued call cancelled after activation does not continue to selection or execution', async () => {
  const events = [];
  let checks = 0;
  await assert.rejects(
    runVisiblePageOperation({
      pageId: 3,
      pageTokens: new Map([[3, 'aaaaaaaaaaaaaaaa']]),
      ensureActive: () => {
        checks += 1;
        if (checks > 1) throw new Error('cancelled');
      },
      activateTab: async () => events.push('activate'),
      selectPage: async () => events.push('select'),
      verify: async () => events.push('verify'),
      execute: async () => events.push('execute'),
    }),
    /cancelled/,
  );
  assert.deepEqual(events, ['activate']);
});

test('ownership change after verification still prevents actual execution', async () => {
  const pageTokens = new Map([[3, 'aaaaaaaaaaaaaaaa']]);
  let executed = false;
  await assert.rejects(
    runVisiblePageOperation({
      pageId: 3,
      pageTokens,
      activateTab: async () => {},
      selectPage: async () => {},
      verify: async () => pageTokens.delete(3),
      execute: async () => { executed = true; },
    }),
    /does not belong to this conversation|ownership changed before execution/i,
  );
  assert.equal(executed, false);
});

test('managed-tool cancellation remaps the internal request id without mutating input', () => {
  const message = {
    jsonrpc: '2.0',
    method: 'notifications/cancelled',
    params: { requestId: 41, reason: 'user' },
  };
  const remapped = remapCancellationNotification(message, 'pinvou-wrapper-internal-7');
  assert.equal(remapped.params.requestId, 'pinvou-wrapper-internal-7');
  assert.equal(remapped.params.reason, 'user');
  assert.equal(message.params.requestId, 41);

  const source = readFileSync(WRAPPER_URL, 'utf8');
  const queueStart = source.indexOf('function queueProxyLine(');
  const childStart = source.indexOf('function processProxyChildLine(', queueStart);
  const queueBody = source.slice(queueStart, childStart);
  assert.match(queueBody, /cancelManagedUpstreamRequest\(/);
  assert.match(queueBody, /msg\.params\?\.reason \|\| 'Browser tool call was cancelled'/);
  const cancelStart = source.indexOf('function cancelManagedUpstreamRequest(');
  const cancelEnd = source.indexOf('function queueProxyLine(', cancelStart);
  const cancelBody = source.slice(cancelStart, cancelEnd);
  assert.match(cancelBody, /signalManagedUpstreamCancellation\(externalRequestId, reason\)/);
  assert.match(cancelBody, /internalRequests\.delete\(internalRequestId\)/);
  assert.match(cancelBody, /pending\.reject\(new Error\(reason\)\)/);
  assert.ok(
    cancelBody.indexOf('if (pending?.awaitRealSettlement)') <
      cancelBody.indexOf('internalRequests.delete(internalRequestId)'),
    'a begun managed dispatch must cancel cooperatively and await the real upstream terminal state',
  );
  const childEnd = source.indexOf('function callUpstreamRequest(', childStart);
  const childBody = source.slice(childStart, childEnd);
  assert.match(childBody, /discardedInternalRequestIds\.delete\(msg\.id\)/);
  assert.ok(
    childBody.indexOf('discardedInternalRequestIds.delete') <
      childBody.lastIndexOf("writeRawOut(line + '\\n')"),
    'late internal responses after cancellation or timeout must be discarded before reaching the engine',
  );
});

test('managed dispatch cancellation, timeout, and child exit end host operations after real settlement', () => {
  const source = readFileSync(WRAPPER_URL, 'utf8');
  const requestStart = source.indexOf('function callUpstreamRequest(');
  const requestEnd = source.indexOf('function callUpstreamTool(', requestStart);
  const requestBody = source.slice(requestStart, requestEnd);
  assert.match(requestBody, /pending\?\.awaitRealSettlement/);
  assert.match(requestBody, /signalManagedUpstreamCancellation\(externalRequestId, reason\)/);
  assert.match(requestBody, /armManagedUpstreamSettlementDeadline\(id, pending, reason\)/);

  const shutdownStart = source.indexOf('function gracefulShutdown(');
  const watchdogStart = source.indexOf('function startHostedBrowserWatchdog(', shutdownStart);
  const shutdownBody = source.slice(shutdownStart, watchdogStart);
  assert.ok(
    shutdownBody.indexOf('await cleanup()') <
      shutdownBody.indexOf('settleInternalRequestsAfterUpstreamStopped(reason)'),
    'upstream must be confirmed stopped before settling in-flight internal requests',
  );
  assert.ok(
    shutdownBody.indexOf('settleInternalRequestsAfterUpstreamStopped(reason)') <
      shutdownBody.indexOf('await Promise.allSettled([proxyQueue, hostCoreQueue])'),
    'leased dispatch finally/end must complete after upstream request settlement',
  );
  assert.match(source, /child\.on\('exit',[\s\S]*?gracefulShutdown\(/);
  assert.match(source, /process\.stdin\.on\('end',[\s\S]*?gracefulShutdown\(/);
  assert.match(source, /process\.on\('SIGHUP',[\s\S]*?gracefulShutdown\(/);
  assert.match(source, /process\.on\('uncaughtException',[\s\S]*?gracefulShutdown\(/);
  assert.match(source, /process\.on\('unhandledRejection',[\s\S]*?gracefulShutdown\(/);
});

test('in-app navigation rejects local-file and script schemes', () => {
  assert.equal(isAllowedBrowserUrl('https://example.com/path'), true);
  assert.equal(isAllowedBrowserUrl('http://127.0.0.1:3000/'), true);
  assert.equal(isAllowedBrowserUrl('about:blank'), true);
  assert.equal(isAllowedBrowserUrl('file:///C:/Users/example/secrets.txt'), false);
  assert.equal(isAllowedBrowserUrl('javascript:alert(1)'), false);
  assert.equal(isAllowedBrowserUrl('data:text/html,unsafe'), false);
  assert.equal(isAllowedBrowserUrl('example.com'), false);
});

test('in-app navigation mirrors the Rust reserved-origin gate', () => {
  // The privileged release origin must never be reachable as a tab URL.
  assert.equal(isAllowedBrowserUrl('http://tauri.localhost/index.html'), false);
  assert.equal(isAllowedBrowserUrl('https://Tauri.LocalHost/app'), false);
  // The Vite dev origin is reserved; other loopback ports stay allowed for
  // local previews.
  assert.equal(isAllowedBrowserUrl('http://localhost:1420/'), false);
  assert.equal(isAllowedBrowserUrl('http://127.0.0.1:1420/'), false);
  assert.equal(isAllowedBrowserUrl('http://[::1]:1420/'), false);
  assert.equal(isAllowedBrowserUrl('http://localhost:1421/'), true);
  assert.equal(isAllowedBrowserUrl('http://localhost:5173/'), true);
  // A public site coincidentally named like the release origin is fine.
  assert.equal(isAllowedBrowserUrl('https://tauri.localhost.example.com/'), true);
});

test('observational browser tool allowlist stays synchronized across JavaScript and Rust', () => {
  // Rust treats exactly these tools as observational (no user-navigation
  // suppression); the wrapper classifies mutation for the same purpose. The
  // lists live in two languages, so pin them textually the same way the
  // last-error contract does.
  const wrapperSource = readFileSync(WRAPPER_PROTOCOL_URL, 'utf8');
  const jsSetStart = wrapperSource.indexOf('const NON_MUTATING_BROWSER_TOOLS = new Set([');
  const jsSetEnd = wrapperSource.indexOf(']);', jsSetStart);
  assert.ok(jsSetStart >= 0 && jsSetEnd > jsSetStart, 'JS allowlist must be enumerable');
  const jsTools = [...wrapperSource
    .slice(jsSetStart, jsSetEnd)
    .matchAll(/'([^']+)'/g)]
    .map((match) => match[1]);

  const rustSource = readFileSync(BROWSER_MOD_URL, 'utf8');
  const rustFnStart = rustSource.indexOf('fn browser_core_tool_is_observational');
  const rustLastTool = rustSource.indexOf('"performance_analyze_insight"', rustFnStart);
  const rustFnEnd = rustSource.indexOf('}', rustLastTool);
  assert.ok(rustFnStart >= 0 && rustFnEnd > rustFnStart, 'Rust allowlist must be enumerable');
  const rustTools = [...rustSource
    .slice(rustFnStart, rustFnEnd)
    .matchAll(/"([^"]+)"/g)]
    .map((match) => match[1]);

  const byCodePoint = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
  assert.deepEqual(jsTools.sort(byCodePoint), rustTools.sort(byCodePoint));
  assert.equal(new Set(jsTools).size, jsTools.length);
  assert.ok(jsTools.length >= 8, 'allowlist unexpectedly shrank');
});

test('navigate_page with url and omitted type still uses the strict URL allowlist', () => {
  assert.equal(effectiveNavigateType({ url: 'https://example.com' }), 'url');
  assert.equal(effectiveNavigateType({ type: 'url', url: 'https://example.com' }), 'url');
  assert.equal(effectiveNavigateType({ type: 'reload' }), 'reload');
  assert.equal(assertAllowedHostedNavigation({ url: 'https://example.com' }), 'url');
  assert.throws(
    () => assertAllowedHostedNavigation({ url: 'file:///C:/Users/example/secrets.txt' }),
    /only supports http, https, and about:blank URLs/,
  );
  assert.throws(
    () => assertAllowedHostedNavigation({ url: 'javascript:alert(1)' }),
    /only supports http, https, and about:blank URLs/,
  );
  // An explicit reload ignores an extra url argument; explicit type wins.
  assert.equal(
    assertAllowedHostedNavigation({ type: 'reload', url: 'javascript:ignored()' }),
    'reload',
  );

  const source = readFileSync(WRAPPER_URL, 'utf8');
  const routeStart = source.indexOf('async function routeHostedToolCall(');
  const routeEnd = source.indexOf('// Handshake IDs used with the MCP child process', routeStart);
  const routeBody = source.slice(routeStart, routeEnd);
  assert.ok(
    routeBody.indexOf("if (name === 'navigate_page') assertAllowedHostedNavigation(args)") <
      routeBody.indexOf('if (!runtimePageScopedTools.has(name))'),
    'URL allowlisting must happen before any unmanaged passthrough branch',
  );
});
