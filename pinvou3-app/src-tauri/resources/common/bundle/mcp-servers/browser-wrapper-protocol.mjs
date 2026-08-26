/**
 * Pure functions for the native Windows multi-tab protocol. The wrapper only
 * exposes workspace pages owned by the current conversation while preserving
 * chrome-devtools-mcp's native pageId values for page-scoped tools.
 */

import {
  mergePinvouBrowserCatalog,
} from './browser-core-protocol.mjs';

const PAGE_ROUTING_EXEMPT_TOOLS = new Set(['list_pages', 'new_page']);
const DISABLED_BROWSER_TOOLS = new Set(['take_screenshot', 'upload_file']);
// These tools only observe the current browser/page state. Everything not on
// this allowlist is conservatively treated as a mutation: evaluate_script can
// run arbitrary JavaScript, diagnostics such as Lighthouse/trace can reload or
// alter the target, and future upstream tools must not become retryable merely
// because this wrapper has not learned their semantics yet.
const NON_MUTATING_BROWSER_TOOLS = new Set([
  'list_pages',
  'select_page',
  'take_snapshot',
  'wait_for',
  'get_console_message',
  'get_network_request',
  'list_console_messages',
  'list_network_requests',
  'performance_analyze_insight',
]);

export const DISABLED_BROWSER_TOOL_ERROR_CODE = 'permission/browser-tool-disabled';
export const BROWSER_PROTOCOL_FRAME_MAX_BYTES = 64 * 1024 * 1024;
export const BROWSER_PROTOCOL_FRAME_TOO_LARGE_ERROR_CODE =
  'browser/protocol-frame-too-large';
export const BROWSER_STARTUP_BACKLOG_EXCEEDED_ERROR_CODE =
  'browser/startup-backlog-exceeded';

const RECOVERABLE_HOST_CORE_WORKSPACE_ERROR_CODES = new Set([
  'browser/workspace-unavailable',
  'browser/workspace-missing',
  'browser/workspace-stopped',
]);

function normalizeWritableError(error) {
  return error instanceof Error ? error : new Error(String(error));
}

function boundedProtocolError(code, message) {
  const error = new Error(`${code}: ${message}`);
  error.code = code;
  return error;
}

/**
 * Incrementally split a byte stream into NDJSON frames without retaining an
 * unbounded partial line. Newline is a single ASCII byte in UTF-8, so scanning
 * the raw bytes preserves multibyte characters split across stream chunks.
 */
export function createBoundedNdjsonDecoder({
  maxFrameBytes = BROWSER_PROTOCOL_FRAME_MAX_BYTES,
  source = 'protocol input',
} = {}) {
  if (!Number.isSafeInteger(maxFrameBytes) || maxFrameBytes < 1) {
    throw new TypeError('maxFrameBytes must be a positive safe integer');
  }
  if (typeof source !== 'string' || !source) {
    throw new TypeError('source must be a non-empty string');
  }

  let chunks = [];
  let pendingBytes = 0;
  let failure = null;

  const rejectOversizedFrame = (nextBytes) => {
    failure ||= boundedProtocolError(
      BROWSER_PROTOCOL_FRAME_TOO_LARGE_ERROR_CODE,
      `${source} exceeded the ${maxFrameBytes}-byte NDJSON frame limit (${nextBytes} bytes)`,
    );
    throw failure;
  };

  const append = (segment) => {
    if (segment.length === 0) return;
    const nextBytes = pendingBytes + segment.length;
    if (nextBytes > maxFrameBytes) rejectOversizedFrame(nextBytes);
    // Copy the partial segment so a short tail does not retain an entire large
    // stream chunk. Compact occasionally to bound per-chunk object overhead.
    chunks.push(Buffer.from(segment));
    pendingBytes = nextBytes;
    if (chunks.length >= 1024) {
      chunks = [Buffer.concat(chunks, pendingBytes)];
    }
  };

  const finishLine = () => {
    const line = chunks.length === 0
      ? ''
      : chunks.length === 1
        ? chunks[0].toString('utf8')
        : Buffer.concat(chunks, pendingBytes).toString('utf8');
    chunks = [];
    pendingBytes = 0;
    return line;
  };

  return {
    push(chunk) {
      if (failure) throw failure;
      const bytes = Buffer.isBuffer(chunk)
        ? chunk
        : chunk instanceof Uint8Array
          ? Buffer.from(chunk.buffer, chunk.byteOffset, chunk.byteLength)
          : Buffer.from(String(chunk), 'utf8');
      const lines = [];
      let cursor = 0;
      for (;;) {
        const newline = bytes.indexOf(0x0a, cursor);
        if (newline < 0) break;
        append(bytes.subarray(cursor, newline));
        lines.push(finishLine());
        cursor = newline + 1;
      }
      append(bytes.subarray(cursor));
      return lines;
    },
    reset() {
      chunks = [];
      pendingBytes = 0;
      failure = null;
    },
    get pendingBytes() {
      return pendingBytes;
    },
    get failure() {
      return failure;
    },
  };
}

/**
 * Bound requests accepted while the lazy MCP child is starting. A count limit
 * prevents tiny-message floods and a byte limit covers a few very large calls.
 */
export function createBoundedLineBacklog({
  maxLines,
  maxBytes,
  source = 'startup request backlog',
} = {}) {
  if (!Number.isSafeInteger(maxLines) || maxLines < 1) {
    throw new TypeError('maxLines must be a positive safe integer');
  }
  if (!Number.isSafeInteger(maxBytes) || maxBytes < 1) {
    throw new TypeError('maxBytes must be a positive safe integer');
  }
  if (typeof source !== 'string' || !source) {
    throw new TypeError('source must be a non-empty string');
  }

  let lines = [];
  let bytes = 0;

  return {
    push(line) {
      const value = String(line);
      const lineBytes = Buffer.byteLength(value);
      const nextLines = lines.length + 1;
      const nextBytes = bytes + lineBytes;
      if (nextLines > maxLines || nextBytes > maxBytes) {
        throw boundedProtocolError(
          BROWSER_STARTUP_BACKLOG_EXCEEDED_ERROR_CODE,
          `${source} exceeded its ${maxLines}-request/${maxBytes}-byte limit`,
        );
      }
      lines.push(value);
      bytes = nextBytes;
      return lines.length;
    },
    snapshot() {
      return [...lines];
    },
    drain() {
      const drained = lines;
      lines = [];
      bytes = 0;
      return drained;
    },
    clear() {
      lines = [];
      bytes = 0;
    },
    get length() {
      return lines.length;
    },
    get bytes() {
      return bytes;
    },
  };
}

/**
 * Serialize writes to a Node writable stream without allowing queued backlog
 * to grow without bound. One active response may exceed the backlog limit
 * because valid DOM/evaluate JSON-RPC results can be large; later responses are
 * bounded while that write drains. A stream error permanently closes the queue,
 * reports the failure once, and makes `flush()` reject deterministically.
 */
export function createOrderedWritableQueue(
  stream,
  { onError = () => {}, maxPendingBytes = 1024 * 1024 } = {},
) {
  if (!stream || typeof stream.write !== 'function' || typeof stream.on !== 'function') {
    throw new TypeError('createOrderedWritableQueue requires a writable stream');
  }
  if (!Number.isSafeInteger(maxPendingBytes) || maxPendingBytes < 1) {
    throw new TypeError('maxPendingBytes must be a positive safe integer');
  }

  let tail = Promise.resolve();
  let failure = null;
  let activeWriteFailure = null;
  let errorReported = false;
  let activeBytes = 0;
  let queuedBytes = 0;
  let scheduledWrites = 0;

  const recordFailure = (error) => {
    if (!failure) failure = normalizeWritableError(error);
    if (activeWriteFailure) {
      const rejectActiveWrite = activeWriteFailure;
      activeWriteFailure = null;
      rejectActiveWrite(failure);
    }
    if (!errorReported) {
      errorReported = true;
      try {
        onError(failure);
      } catch {
        // Error reporting must not replace the original stream failure.
      }
    }
  };

  stream.on('error', recordFailure);
  stream.on('close', () => {
    recordFailure(new Error('Writable stream closed before queued output drained'));
  });

  const writeChunk = (chunk) => new Promise((resolve, reject) => {
    let waitingForDrain = false;
    let settled = false;

    const cleanup = () => {
      if (waitingForDrain && typeof stream.off === 'function') stream.off('drain', onDrain);
      if (activeWriteFailure === fail) activeWriteFailure = null;
    };
    const finish = () => {
      if (settled) return;
      settled = true;
      cleanup();
      resolve();
    };
    const fail = (error) => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(normalizeWritableError(error));
    };
    const onDrain = () => finish();

    activeWriteFailure = fail;
    try {
      const accepted = stream.write(String(chunk));
      if (settled) return;
      if (accepted) {
        finish();
      } else {
        waitingForDrain = true;
        stream.once('drain', onDrain);
      }
    } catch (error) {
      recordFailure(error);
    }
  });

  return {
    write(chunk) {
      const value = String(chunk);
      const bytes = Buffer.byteLength(value);
      if (failure) return tail;
      const becomesActive = scheduledWrites === 0;
      if (!becomesActive && bytes > maxPendingBytes - queuedBytes) {
        recordFailure(new Error(
          `Writable queue exceeded its ${maxPendingBytes}-byte queued backlog limit`,
        ));
        return tail;
      }
      scheduledWrites += 1;
      if (becomesActive) activeBytes = bytes;
      else queuedBytes += bytes;
      tail = tail.then(async () => {
        if (!becomesActive) {
          queuedBytes -= bytes;
          activeBytes = bytes;
        }
        try {
          if (!failure) await writeChunk(value);
        } catch (error) {
          recordFailure(error);
        } finally {
          activeBytes = 0;
          scheduledWrites -= 1;
        }
      });
      return tail;
    },
    async flush() {
      await tail;
      if (failure) throw failure;
    },
    get failure() {
      return failure;
    },
    get pendingBytes() {
      return activeBytes + queuedBytes;
    },
    get queuedBytes() {
      return queuedBytes;
    },
  };
}

/**
 * Only lifecycle errors that guarantee BrowserCore rejected the request before
 * dispatch are safe to retry. In particular, native-surface and lease errors
 * may be reported after state has changed and must never enter this path.
 */
export function isRecoverableHostCoreWorkspaceError(error) {
  const message = typeof error === 'string' ? error : error?.message;
  if (typeof message !== 'string') return false;
  return [...RECOVERABLE_HOST_CORE_WORKSPACE_ERROR_CODES].some((code) => {
    if (message === code) return true;
    if (!message.startsWith(code)) return false;
    const separator = message.charAt(code.length);
    return separator === ':' || /\s/.test(separator);
  });
}

export function isDisabledBrowserToolName(name) {
  return DISABLED_BROWSER_TOOLS.has(name);
}

export function browserToolMayMutate(name) {
  return !NON_MUTATING_BROWSER_TOOLS.has(name);
}

export function assertAllowedBrowserToolCall(message) {
  if (message?.method !== 'tools/call') return message;
  const name = message.params?.name;
  if (isDisabledBrowserToolName(name)) {
    throw new Error(`${DISABLED_BROWSER_TOOL_ERROR_CODE}: ${name}`);
  }
  return message;
}

function exposePageIdRouting(tool) {
  if (
    !tool ||
    typeof tool.name !== 'string' ||
    PAGE_ROUTING_EXEMPT_TOOLS.has(tool.name)
  ) {
    return tool;
  }
  const schema = tool.inputSchema && typeof tool.inputSchema === 'object'
    ? tool.inputSchema
    : { type: 'object' };
  const properties = schema.properties && typeof schema.properties === 'object'
    ? schema.properties
    : {};
  const required = Array.isArray(schema.required) ? schema.required : [];
  return {
    ...tool,
    inputSchema: {
      ...schema,
      properties: {
        pageId: {
          type: 'number',
          description: 'Targets a specific page by ID.',
        },
        ...properties,
      },
      required: required.includes('pageId') ? required : ['pageId', ...required],
    },
  };
}

export function adaptBrowserCatalog(value) {
  if (!value?.toolsListResult || !Array.isArray(value.toolsListResult.tools)) return value;
  const tools = mergePinvouBrowserCatalog(value.toolsListResult.tools, {
    // The official Chrome MCP remains a private Windows implementation for
    // diagnostics only. The common BrowserCore catalog is owned by Pinvou.
    includeChromeDiagnostics: process.platform === 'win32',
    // A captured vendor catalog is complete in production. Keeping partial
    // fixture catalogs partial also prevents a schema from being advertised
    // when its current backend has not supplied an implementation.
    synthesizeMissingCore: false,
    preserveUpstreamOrder: true,
  });
  return {
    ...value,
    toolsListResult: {
      ...value.toolsListResult,
      // CodeWhale currently serializes MCP image content into a text-only
      // ToolResult, so the model receives base64 text rather than a visually
      // interpretable image. The embedded browser already renders the page for
      // the user. Keep screenshots hidden until the visual-result pipeline is
      // connected, avoiding wasted context and false claims of visual review.
      // catalog-shim is captured before browser startup and does not include
      // the runtime experimental-page-id-routing flag. Mirror the upstream
      // 1.7.0 schema transformation so the Agent sees the explicit pageId.
      tools: tools
        .filter((tool) => !isDisabledBrowserToolName(tool?.name))
        .map(exposePageIdRouting),
    },
  };
}

// Compatibility export for older callers. Final multi-tab mode no longer
// rewrites new_page or close_page into navigate_page.
export function adaptHostedBrowserToolCall(line) {
  return line;
}

export function browserHostBackendPolicy(platform) {
  if (platform === 'win32') {
    return {
      action: 'request-native-host',
      backend: 'webview2',
      code: null,
      message: null,
    };
  }
  if (platform === 'darwin') {
    return {
      action: 'request-browser-core',
      backend: 'wkwebview',
      code: null,
      message: null,
    };
  }
  if (platform === 'linux') {
    return {
      action: 'request-browser-core',
      backend: 'webkitgtk',
      code: null,
      message: null,
    };
  }
  return {
    action: 'unsupported',
    backend: null,
    code: 'unsupported/host-backend-unavailable',
    message: `No native browser automation backend is available on this platform (${platform || 'unknown'})`,
  };
}

const TAB_TOKEN_PATTERN = /^[0-9a-f]{16}$/;
const HOST_REQUEST_ID_PATTERN = /^[A-Za-z0-9._-]{1,160}$/;
const WRAPPER_INSTANCE_NONCE_PATTERN = /^[0-9a-f]{32}$/;
export const HOST_REQUEST_PROTOCOL_VERSION = 3;
export const HOST_CALLER_HEARTBEAT_INTERVAL_MS = 1_000;
export const HOST_CALLER_HEARTBEAT_TTL_MS = 5_000;

export function effectiveNavigateType(args = {}) {
  if (typeof args?.type === 'string' && args.type.trim()) return args.type;
  return Object.prototype.hasOwnProperty.call(args || {}, 'url') ? 'url' : null;
}

export function assertAllowedHostedNavigation(args = {}) {
  const effectiveType = effectiveNavigateType(args);
  if (effectiveType === 'url' && !isAllowedBrowserUrl(args?.url)) {
    throw new Error('The in-app browser only supports http, https, and about:blank URLs');
  }
  return effectiveType;
}

/**
 * Requests and cancellation notifications can arrive in the same stdin chunk
 * during startup. Filter against a snapshot of the cancellation set before
 * emitting failures; clearing it first would fabricate startup errors for
 * already-cancelled requests.
 */
export function uncancelledBufferedRequests(lines, cancelledRequestIds) {
  const cancelled = cancelledRequestIds instanceof Set
    ? cancelledRequestIds
    : new Set(cancelledRequestIds || []);
  return (Array.isArray(lines) ? lines : []).filter((line) => {
    try {
      const message = JSON.parse(line);
      return message?.id == null || !cancelled.has(message.id);
    } catch {
      return true;
    }
  });
}

function assertHostRequestIdentity(sessionToken, requestId) {
  if (!TAB_TOKEN_PATTERN.test(sessionToken || '')) {
    throw new Error('Browser host request has an invalid sessionToken');
  }
  if (!HOST_REQUEST_ID_PATTERN.test(requestId || '')) {
    throw new Error('Browser host request has an invalid requestId');
  }
}

function assertWrapperInstanceNonce(wrapperInstanceNonce) {
  if (!WRAPPER_INSTANCE_NONCE_PATTERN.test(wrapperInstanceNonce || '')) {
    throw new Error('Browser host request has an invalid wrapperInstanceNonce');
  }
}

function assertHostCallerIdentity(callerPid, wrapperInstanceNonce) {
  if (!Number.isSafeInteger(callerPid) || callerPid <= 0 || callerPid > 0xffff_ffff) {
    throw new Error('Browser host request has an invalid callerPid');
  }
  assertWrapperInstanceNonce(wrapperInstanceNonce);
}

export function hostCallerHeartbeatArtifactName(sessionToken, wrapperInstanceNonce) {
  if (!TAB_TOKEN_PATTERN.test(sessionToken || '')) {
    throw new Error('Browser host caller heartbeat has an invalid sessionToken');
  }
  assertWrapperInstanceNonce(wrapperInstanceNonce);
  return `${sessionToken}-${wrapperInstanceNonce}.heartbeat`;
}

export function createHostCallerHeartbeat({
  sessionId,
  sessionToken,
  callerPid,
  wrapperInstanceNonce,
  heartbeatAt = Date.now(),
}) {
  if (typeof sessionId !== 'string' || !sessionId) {
    throw new Error('Browser host caller heartbeat has an invalid sessionId');
  }
  if (!TAB_TOKEN_PATTERN.test(sessionToken || '')) {
    throw new Error('Browser host caller heartbeat has an invalid sessionToken');
  }
  assertHostCallerIdentity(callerPid, wrapperInstanceNonce);
  if (!Number.isSafeInteger(heartbeatAt) || heartbeatAt <= 0) {
    throw new Error('Browser host caller heartbeat has an invalid timestamp');
  }
  return {
    protocol_version: HOST_REQUEST_PROTOCOL_VERSION,
    kind: 'host_caller_heartbeat',
    session_id: sessionId,
    session_token: sessionToken,
    caller_pid: callerPid,
    wrapper_instance_nonce: wrapperInstanceNonce,
    heartbeat_at: heartbeatAt,
  };
}

export function hostRequestArtifactNames(sessionToken, requestId) {
  assertHostRequestIdentity(sessionToken, requestId);
  const stem = `${sessionToken}-${requestId}`;
  return {
    request: `${stem}.json`,
    response: `${stem}.response`,
    cancelled: `${stem}.cancelled`,
  };
}

export function createHostRequestEnvelope({
  requestId,
  sessionId,
  sessionToken,
  callerPid,
  wrapperInstanceNonce,
  operation,
  payload = {},
  requestedAt = Date.now(),
}) {
  assertHostRequestIdentity(sessionToken, requestId);
  assertHostCallerIdentity(callerPid, wrapperInstanceNonce);
  if (typeof sessionId !== 'string' || !sessionId) {
    throw new Error('Browser host request has an invalid sessionId');
  }
  if (typeof operation !== 'string' || !operation) {
    throw new Error('Browser host request has an invalid operation');
  }
  if (!payload || typeof payload !== 'object' || Array.isArray(payload)) {
    throw new Error('Browser host request payload must be an object');
  }
  return {
    ...payload,
    protocol_version: HOST_REQUEST_PROTOCOL_VERSION,
    request_id: requestId,
    idempotency_key: `${sessionToken}/${requestId}`,
    session_id: sessionId,
    session_token: sessionToken,
    caller_pid: callerPid,
    wrapper_instance_nonce: wrapperInstanceNonce,
    operation,
    requested_at: requestedAt,
  };
}

export function createHostCancellationTombstone({
  requestId,
  sessionId,
  sessionToken,
  callerPid,
  wrapperInstanceNonce,
  reason = 'timeout',
  cancelledAt = Date.now(),
}) {
  assertHostRequestIdentity(sessionToken, requestId);
  assertHostCallerIdentity(callerPid, wrapperInstanceNonce);
  if (typeof sessionId !== 'string' || !sessionId) {
    throw new Error('Browser host cancellation record has an invalid sessionId');
  }
  return {
    protocol_version: HOST_REQUEST_PROTOCOL_VERSION,
    kind: 'host_request_cancelled',
    request_id: requestId,
    idempotency_key: `${sessionToken}/${requestId}`,
    session_id: sessionId,
    session_token: sessionToken,
    caller_pid: callerPid,
    wrapper_instance_nonce: wrapperInstanceNonce,
    reason,
    cancelled_at: cancelledAt,
  };
}

/**
 * pageId and host tabToken must form a bijection. Keep the input as entries
 * until validation so Map overwrite semantics cannot hide duplicate pageIds.
 */
export function buildBijectivePageTokenMaps(entries) {
  if (!entries || typeof entries[Symbol.iterator] !== 'function') {
    throw new Error('Page mapping must be an iterable list of [pageId, tabToken] entries');
  }
  const pageToToken = new Map();
  const tokenToPage = new Map();
  for (const entry of entries) {
    if (!Array.isArray(entry) || entry.length < 2) {
      throw new Error('Page mapping entry has an invalid shape');
    }
    const [pageId, tabToken] = entry;
    if (!Number.isInteger(pageId) || !TAB_TOKEN_PATTERN.test(tabToken || '')) {
      throw new Error('Page mapping entry contains an invalid pageId or tabToken');
    }
    if (pageToToken.has(pageId)) {
      throw new Error(`Page mapping contains duplicate pageId: ${pageId}`);
    }
    if (tokenToPage.has(tabToken)) {
      throw new Error(`Page mapping contains duplicate tabToken: ${tabToken}`);
    }
    pageToToken.set(pageId, tabToken);
    tokenToPage.set(tabToken, pageId);
  }
  return { pageToToken, tokenToPage };
}

export function assertBijectivePageTokenMap(pageTokens) {
  if (!(pageTokens instanceof Map)) throw new Error('Page mapping must be a Map');
  return buildBijectivePageTokenMaps(pageTokens.entries());
}

/**
 * Return the explicitly supplied pageId when it belongs to this conversation,
 * or null when omitted. Reject present non-integer values rather than silently
 * degrading malformed or cross-conversation IDs to "use the current page."
 */
export function explicitOwnedPageId(args, pageTokens) {
  if (!Object.prototype.hasOwnProperty.call(args || {}, 'pageId')) return null;
  assertBijectivePageTokenMap(pageTokens);
  const pageId = args.pageId;
  if (!Number.isInteger(pageId) || !pageTokens.has(pageId)) {
    throw new Error('Page does not exist or does not belong to this conversation');
  }
  return pageId;
}

/**
 * The authoritative host target mapping only accepts the explicit v2 schema.
 * Never interpret a v1 page-script marker, URL fragment, or partial v2 state
 * as an authoritative mapping.
 */
export function parseAuthoritativeHostWorkspace(value, expectedSessionToken = null) {
  if (value?.version !== 2 || value.mapping_authority !== 'host') {
    throw new Error('Host workspace did not provide an authoritative v2 target mapping');
  }
  if (!TAB_TOKEN_PATTERN.test(value.session_token || '')) {
    throw new Error('Host workspace has an invalid session_token');
  }
  if (expectedSessionToken != null && value.session_token !== expectedSessionToken) {
    throw new Error('Host workspace session_token does not match');
  }
  if (!Number.isInteger(value.revision) || value.revision < 0) {
    throw new Error('Host workspace has an invalid revision');
  }
  if (!TAB_TOKEN_PATTERN.test(value.active_tab || '') || !Array.isArray(value.tabs)) {
    throw new Error('Host workspace has invalid active_tab or tabs data');
  }

  const tokens = new Set();
  const targets = new Set();
  const tabs = value.tabs.map((tab) => {
    const token = tab?.token;
    const targetId = tab?.target_id;
    if (!TAB_TOKEN_PATTERN.test(token || '')) {
      throw new Error('Authoritative host target mapping contains an invalid tabToken');
    }
    if (typeof targetId !== 'string' || !targetId.trim()) {
      throw new Error(`Host tab ${token} is missing an authoritative target_id`);
    }
    if (tokens.has(token)) throw new Error(`Host workspace contains duplicate tabToken: ${token}`);
    if (targets.has(targetId)) throw new Error(`Host workspace contains duplicate target_id: ${targetId}`);
    tokens.add(token);
    targets.add(targetId);
    return { token, target_id: targetId };
  });
  if (!tokens.has(value.active_tab)) {
    throw new Error('Host workspace active_tab is absent from the authoritative target mapping');
  }
  return {
    ...value,
    tabs,
    mapping_authority: 'host',
  };
}

export function parseHostActivationLease(value, expected = {}) {
  const sessionId = value?.sessionId ?? value?.session_id;
  const tabToken = value?.tabToken ?? value?.tab_token;
  const targetId = value?.targetId ?? value?.target_id;
  if (typeof sessionId !== 'string' || !sessionId) {
    throw new Error('activate_tab response is missing a valid sessionId');
  }
  if (!TAB_TOKEN_PATTERN.test(tabToken || '')) {
    throw new Error('activate_tab response is missing a valid tabToken');
  }
  if (typeof targetId !== 'string' || !targetId.trim()) {
    throw new Error('activate_tab response is missing an authoritative targetId');
  }
  if (!Number.isInteger(value?.revision) || value.revision < 0) {
    throw new Error('activate_tab response is missing a valid revision');
  }
  if (!/^[0-9a-f]{32}$/.test(value?.lease || '')) {
    throw new Error('activate_tab response is missing a dispatch lease');
  }
  if (value?.owner !== 'agent') {
    throw new Error('activate_tab response did not grant control to the Agent');
  }
  if (expected.sessionId != null && expected.sessionId !== sessionId) {
    throw new Error('activate_tab response sessionId does not match the request');
  }
  if (expected.tabToken != null && expected.tabToken !== tabToken) {
    throw new Error('activate_tab response tabToken does not match the request');
  }
  if (expected.targetId != null && expected.targetId !== targetId) {
    throw new Error('activate_tab response targetId does not match the host mapping');
  }
  return {
    sessionId,
    tabToken,
    targetId,
    revision: value.revision,
    owner: 'agent',
    lease: value.lease,
  };
}

export function hostLeaseAssertionPayload(activationLease) {
  const lease = parseHostActivationLease(activationLease);
  return {
    tab_token: lease.tabToken,
    target_id: lease.targetId,
    revision: lease.revision,
    lease: lease.lease,
  };
}

/**
 * create and close mutate host state, and tab_token may denote a new tab, so it
 * cannot also identify the authorization source. authorization_tab_token
 * explicitly identifies the current or target tab that owns the lease; the
 * remaining CAS fields match NativeTabLease.
 */
export function hostMutationAuthorizationPayload(activationLease) {
  const lease = parseHostActivationLease(activationLease);
  return {
    authorization_tab_token: lease.tabToken,
    target_id: lease.targetId,
    revision: lease.revision,
    lease: lease.lease,
  };
}

export function parseCreatedTabResult(value, { tabToken, creationId } = {}) {
  // The v3 create result is part of the security protocol and does not accept
  // legacy aliases. Otherwise the wrapper could accept a partially upgraded
  // host as CAS-complete while compensation cannot bind the creation epoch.
  const resultTabToken = value?.tabToken;
  const targetId = value?.targetId;
  const resultCreationId = value?.creationId;
  if (!TAB_TOKEN_PATTERN.test(resultTabToken || '')) {
    throw new Error('create_tab response is missing a valid tabToken');
  }
  if (typeof targetId !== 'string' || !targetId.trim()) {
    throw new Error('create_tab response is missing a valid targetId');
  }
  if (!HOST_REQUEST_ID_PATTERN.test(resultCreationId || '')) {
    throw new Error('create_tab response is missing a valid creationId');
  }
  if (tabToken != null && resultTabToken !== tabToken) {
    throw new Error('create_tab response tabToken does not match the request');
  }
  if (creationId != null && resultCreationId !== creationId) {
    throw new Error('create_tab response creationId does not match request_id');
  }
  return {
    tabToken: resultTabToken,
    targetId,
    creationId: resultCreationId,
  };
}

/**
 * Identity fields in a v3 response envelope are mandatory, not conditionally
 * validated. Otherwise another CAS mutation could accept a stale or forged
 * response. create_tab additionally requires creationId=request_id.
 */
export function parseHostResponseEnvelope(value, {
  requestId,
  idempotencyKey,
  operation,
  requestedTabToken = null,
} = {}) {
  if (value?.protocol_version !== HOST_REQUEST_PROTOCOL_VERSION) {
    throw new Error('Browser host response is missing valid protocol_version=3');
  }
  if (typeof requestId !== 'string' || value?.request_id !== requestId) {
    throw new Error('Browser host response is missing request_id or returned a mismatch');
  }
  if (typeof idempotencyKey !== 'string' || value?.idempotency_key !== idempotencyKey) {
    throw new Error('Browser host response is missing idempotency_key or returned a mismatch');
  }
  if (typeof value?.ok !== 'boolean') {
    throw new Error('Browser host response is missing a boolean ok field');
  }
  if (!value.ok) {
    throw new Error(value?.error || `Browser host operation ${operation || 'unknown'} failed`);
  }
  const result = value.result ?? {};
  return operation === 'create_tab'
    ? parseCreatedTabResult(result, {
        tabToken: requestedTabToken,
        creationId: requestId,
      })
    : result;
}

export function createHostLeaseAssertionRequest(activationLease) {
  return {
    operation: 'assert_host_lease',
    ...hostLeaseAssertionPayload(activationLease),
  };
}

export async function runLeasedHostDispatch({
  activationLease,
  emitsTrustedInput = false,
  ensureActive = () => {},
  beginOperation,
  refreshOperation = null,
  onRefreshFailure = () => {},
  heartbeatIntervalMs = 250,
  endOperation,
  onEndFailure = () => {},
  execute,
}) {
  const lease = parseHostActivationLease(activationLease);
  await ensureActive();
  try {
    await beginOperation({ lease, emitsTrustedInput });
  } catch (beginError) {
    // The host may have committed begin_agent_operation even when its file
    // acknowledgement was lost. End the exact lease before surfacing the
    // original error; with a generation-consuming host this also prevents a
    // delayed begin artifact from reopening authorization after cancellation.
    try {
      await endOperation(lease);
    } catch (endError) {
      try {
        await onEndFailure(endError, lease, {
          executionSucceeded: false,
          heartbeatFailed: false,
          beginFailed: true,
        });
      } catch {
        // Cleanup diagnostics must not replace the begin failure.
      }
    }
    throw beginError;
  }

  let heartbeatStopped = false;
  let wakeHeartbeat = null;
  let heartbeatError = null;
  let heartbeatTask = null;
  // Every begun host operation needs a bounded liveness signal. Trusted input
  // uses a short suppression-window refresh, while read/DOM operations use the
  // slower generic operation refresh supplied by the wrapper.
  if (typeof refreshOperation === 'function') {
    const intervalMs = Math.max(1, Number(heartbeatIntervalMs) || 250);
    heartbeatTask = (async () => {
      while (!heartbeatStopped) {
        await new Promise((resolve) => {
          const timer = setTimeout(resolve, intervalMs);
          wakeHeartbeat = () => {
            clearTimeout(timer);
            resolve();
          };
        });
        wakeHeartbeat = null;
        if (heartbeatStopped) return;
        try {
          await ensureActive();
          await refreshOperation(lease);
        } catch (error) {
          heartbeatError = error;
          heartbeatStopped = true;
          try {
            await onRefreshFailure(error, lease);
          } catch {
            // Cancelling the in-flight upstream request is best-effort. The
            // original refresh failure remains authoritative and the logical
            // tool is never dispatched a second time.
          }
          return;
        }
      }
    })();
  }

  let executionResult;
  let executionError = null;
  let endError = null;
  try {
    await ensureActive();
    executionResult = await execute(lease);
  } catch (error) {
    executionError = error;
  } finally {
    heartbeatStopped = true;
    wakeHeartbeat?.();
    await heartbeatTask;
    try {
      await endOperation(lease);
    } catch (error) {
      endError = error;
      try {
        await onEndFailure(error, lease, {
          executionSucceeded: executionError == null,
          heartbeatFailed: heartbeatError != null,
        });
      } catch {
        // Cleanup diagnostics are best-effort and must not replace either the
        // committed tool result or the original dispatch/heartbeat failure.
      }
    }
  }

  // A rejected heartbeat revokes the authorization for this one logical
  // dispatch. If the upstream nevertheless completed successfully, that
  // committed result stays authoritative. If cooperative cancellation made
  // the upstream fail, the page-side commit state is unknown: return a
  // tool-level, explicitly non-retryable outcome instead of a JSON-RPC error
  // that could make the Agent replay a click or keystroke.
  if (heartbeatError != null) {
    if (executionError == null && executionResult?.isError !== true) return executionResult;
    const authorizationError = heartbeatError?.message || String(heartbeatError);
    const upstreamError = executionError != null
      ? executionError?.message || String(executionError)
      : Array.isArray(executionResult?.content)
        ? executionResult.content.map((item) => item?.text || '').filter(Boolean).join('\n') ||
          'upstream returned a tool-level error'
        : 'upstream returned a tool-level error';
    return {
      content: [{
        type: 'text',
        text:
          'The browser authorization heartbeat failed while this tool was in flight, and Pinvou ' +
          'cannot prove that the page action did not occur. Do not repeat the action; inspect the ' +
          `page state before continuing. Authorization error: ${authorizationError}. ` +
          `Upstream result: ${upstreamError}`,
      }],
      isError: true,
      structuredContent: {
        errorCode: 'browser/action-commit-unknown-after-authorization-loss',
        outcome: 'unknown',
        actionCommitted: true,
        actionMayHaveCommitted: true,
        retryable: false,
        authorizationError,
        upstreamError,
      },
    };
  }
  if (executionError != null) throw executionError;
  // execute() has already committed exactly once. A transport/cleanup failure
  // here must not be exposed as an ordinary retryable tool failure: an Agent
  // could replay a click/type action. The heartbeat is already stopped and the
  // host's trusted-input window expires independently; report the cleanup via
  // onEndFailure while preserving the authoritative committed result.
  void endError;
  return executionResult;
}

export function pageScopedToolNames(toolsListResult) {
  const tools = Array.isArray(toolsListResult?.tools) ? toolsListResult.tools : [];
  return new Set(
    tools
      .filter((tool) => {
        const schema = tool?.inputSchema;
        return schema?.properties?.pageId &&
          Array.isArray(schema.required) &&
          schema.required.includes('pageId');
      })
      .map((tool) => tool.name)
      .filter((name) => typeof name === 'string')
  );
}

export function inputToolNames(toolsListResult) {
  const tools = Array.isArray(toolsListResult?.tools) ? toolsListResult.tools : [];
  return new Set(
    tools
      .filter((tool) => tool?.annotations?.category === 'input')
      .map((tool) => tool.name)
      .filter((name) => typeof name === 'string')
  );
}

export function routeToolCallToPage(message, pageId, argumentPatch = {}) {
  return {
    ...message,
    params: {
      ...message.params,
      arguments: {
        ...(message.params?.arguments || {}),
        ...argumentPatch,
        pageId,
      },
    },
  };
}

/**
 * Atomically align the Agent's target page with the user-visible tab before
 * performing a managed browser page operation.
 *
 * pageTokens is the only allowed pageId -> tabToken mapping for the current
 * task conversation. The caller serializes activate/select/verify/execute;
 * this function fixes their order and rechecks ownership and cancellation
 * before each side effect. execute is never called after a failed phase, so a
 * tool cannot silently land on a background page or another task's page.
 */
export async function runVisiblePageOperation({
  pageId,
  pageTokens,
  ensureActive = () => {},
  activateTab,
  assertLease = () => {},
  selectPage,
  verify,
  execute = null,
  now = () => globalThis.performance?.now?.() ?? Date.now(),
  recordAlignment = null,
}) {
  const alignmentStartedAt = now();
  const ownedTabToken = () => {
    if (!Number.isInteger(pageId) || !(pageTokens instanceof Map)) {
      throw new Error('Page does not exist or does not belong to this conversation');
    }
    // Revalidate the bijection at every phase so a concurrent refresh cannot
    // leave one token assigned to multiple pageIds.
    assertBijectivePageTokenMap(pageTokens);
    const token = pageTokens.get(pageId);
    if (typeof token !== 'string' || !token) {
      throw new Error('Page does not exist or does not belong to this conversation');
    }
    return token;
  };

  const tabToken = ownedTabToken();
  await ensureActive();
  const activationResult = await activateTab(tabToken);

  // The user may close a tab during host activation; do not trust the mapping
  // captured by the initial check.
  await ensureActive();
  if (ownedTabToken() !== tabToken) throw new Error('Page ownership changed during activation');
  await assertLease({ pageId, tabToken, activationResult, phase: 'select' });
  const selectionResult = await selectPage(pageId);

  await ensureActive();
  if (ownedTabToken() !== tabToken) throw new Error('Page ownership changed during selection');
  await assertLease({ pageId, tabToken, activationResult, phase: 'verify' });
  const verificationResult = await verify({ pageId, tabToken, activationResult });

  await ensureActive();
  if (ownedTabToken() !== tabToken) throw new Error('Page ownership changed before execution');
  if (recordAlignment) {
    try {
      recordAlignment(Math.max(0, now() - alignmentStartedAt));
    } catch {
      // Performance diagnostics must never change the page-operation result.
    }
  }
  const executionResult = execute
    ? await execute({ pageId, tabToken, activationResult })
    : undefined;
  return {
    pageId,
    tabToken,
    activationResult,
    selectionResult,
    verificationResult,
    executionResult,
  };
}

export function remapCancellationNotification(message, requestId) {
  if (message?.method !== 'notifications/cancelled' || requestId == null) return message;
  return {
    ...message,
    params: {
      ...(message.params || {}),
      requestId,
    },
  };
}

export function isAllowedBrowserUrl(value) {
  if (value === 'about:blank') return true;
  if (typeof value !== 'string') return false;
  try {
    const protocol = new URL(value).protocol;
    return protocol === 'http:' || protocol === 'https:';
  } catch {
    return false;
  }
}

export function parseBrowserPages(result) {
  const structured = result?.structuredContent?.pages;
  if (Array.isArray(structured)) {
    return structured
      .filter((page) => Number.isInteger(page?.id))
      .map((page) => {
        const targetId = page.targetId ?? page.target_id;
        return {
          id: page.id,
          url: typeof page.url === 'string' ? page.url : '',
          title: typeof page.title === 'string' ? page.title : '',
          selected: page.selected === true,
          ...(typeof targetId === 'string' && targetId ? { targetId } : {}),
        };
      });
  }
  const text = Array.isArray(result?.content)
    ? result.content.filter((item) => item?.type === 'text').map((item) => item.text || '').join('\n')
    : '';
  const pages = [];
  for (const line of text.split(/\r?\n/)) {
    const match = line.match(/^\s*(\d+):\s*(.*?)(\s+\[selected\])?\s*$/);
    if (!match) continue;
    const label = match[2];
    const urlMatch = label.match(/\((https?:\/\/[^)]*|about:blank[^)]*)\)\s*$/);
    pages.push({
      id: Number(match[1]),
      url: urlMatch ? urlMatch[1] : label,
      title: urlMatch ? label.slice(0, urlMatch.index).trim() : '',
      selected: Boolean(match[3]),
      rawLine: line,
    });
  }
  return pages;
}

/**
 * The task browser starts with one host-owned about:blank page. The first
 * foreground new_page may reuse only that bootstrap page; an ordinary blank
 * tab must keep normal multi-tab semantics.
 */
export function isReusableBootstrapBlankPage({
  workspace,
  page,
  pageToken,
  background = false,
} = {}) {
  if (background || !workspace || !page) return false;
  const tabs = Array.isArray(workspace.tabs) ? workspace.tabs : [];
  if (tabs.length !== 1) return false;

  const bootstrapTab = tabs[0];
  const sessionToken = workspace.session_token;
  if (
    typeof sessionToken !== 'string' ||
    bootstrapTab?.token !== sessionToken ||
    workspace.active_tab !== sessionToken ||
    pageToken !== sessionToken
  ) {
    return false;
  }

  return page.url === 'about:blank' ||
    page.url === `about:blank#pinvou-session-${sessionToken}`;
}

export function filterPagesResult(result, allowedPageIds, selectedPageId) {
  const allowed = allowedPageIds instanceof Set ? allowedPageIds : new Set(allowedPageIds || []);
  const pages = parseBrowserPages(result).filter((page) => allowed.has(page.id));
  const lines = ['## Pages'];
  for (const page of pages) {
    const label = page.title ? `${page.title} (${page.url})` : page.url;
    lines.push(`${page.id}: ${label}${page.id === selectedPageId ? ' [selected]' : ''}`);
  }
  const filtered = {
    ...result,
    content: [{ type: 'text', text: lines.join('\n') }],
  };
  if (result?.structuredContent && typeof result.structuredContent === 'object') {
    filtered.structuredContent = {
      ...result.structuredContent,
      pages: pages.map((page) => ({
        id: page.id,
        url: page.url,
        title: page.title,
        selected: page.id === selectedPageId,
      })),
    };
  }
  return filtered;
}

export function findHostedSessionPage(result, sessionToken) {
  if (!/^[0-9a-f]{16}$/.test(sessionToken || '')) return null;
  const marker = `about:blank#pinvou-session-${sessionToken}`;
  const matches = parseBrowserPages(result).filter(
    (page) => !page.targetId && page.url === marker,
  );
  if (matches.length > 1) throw new Error('MCP page list contains duplicate session markers');
  return matches[0] || null;
}

export function findHostedTabPage(result, tabToken, pageTokens = new Map()) {
  if (!/^[0-9a-f]{16}$/.test(tabToken || '')) return null;
  const marker = `about:blank#pinvou-tab-${tabToken}`;
  const matches = parseBrowserPages(result).filter(
    (page) => pageTokens.get(page.id) === tabToken ||
      (!page.targetId && page.url === marker),
  );
  if (matches.length > 1) throw new Error(`MCP page list contains duplicate tab marker: ${tabToken}`);
  return matches[0] || null;
}

/**
 * MCP handshake must also use the authoritative host target join. Fragment
 * bootstrap is allowed only for a newly created, not-yet-navigated about:blank;
 * navigated pages and wrapper/MCP restarts recover directly by target_id.
 */
export function findHostedWorkspacePage(result, workspace, expectedSessionToken) {
  const state = parseAuthoritativeHostWorkspace(workspace, expectedSessionToken);
  const active = state.tabs.find((tab) => tab.token === state.active_tab);
  if (!active) throw new Error('Host workspace has no active tab');
  const pages = parseBrowserPages(result);
  const targetMatches = pages.filter((page) => page.targetId === active.target_id);
  if (targetMatches.length > 1) {
    throw new Error(`MCP page list contains duplicate target_id: ${active.target_id}`);
  }
  if (targetMatches.length === 1) return targetMatches[0];
  return active.token === expectedSessionToken
    ? findHostedSessionPage(result, expectedSessionToken)
    : findHostedTabPage(result, active.token);
}
