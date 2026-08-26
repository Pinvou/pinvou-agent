#!/usr/bin/env node
/**
 * browser-wrapper.mjs -- stdio coordination wrapper for the Pinvou browser
 * MCP server (lazy-start proxy).
 *
 * Responsibilities:
 *  1. Lazy startup. CodeWhale connects every MCP server on a session's first
 *     turn (`McpPool::connect_all`). Preparing the browser at process startup
 *     would therefore keep a browser runtime alive after every first Work-mode
 *     message. This wrapper initially acts as a shim and directly answers MCP
 *     `initialize`, `ping`, and `tools/list` from the build-time
 *     catalog-shim.json. It prepares the platform's in-app browser backend only
 *     when the first `tools/call` or other real request arrives. Windows then
 *     proxies the official chrome-devtools-mcp; Linux and macOS continue by
 *     forwarding BrowserCore requests.
 *  2. Coordinate task-owned native-page lifecycles with the Pinvou desktop
 *     Rust BrowserManager:
 *     - Windows creates conversation-scoped WebView2 instances through
 *       host-requests/*.json and operates them over CDP;
 *     - Linux uses WebKitGTK with WebKitWebDriver; macOS uses WKWebView/AppKit;
 *     - unavailable native hosts fail explicitly and never start or reuse an
 *       external Chrome instance;
 *     - all three platforms share one Agent tool contract, session isolation,
 *       control leases, and host-request protocol.
 *  3. On Windows only, point the official chrome-devtools-mcp at the app-owned
 *     WebView2 CDP port using `--browser-url`, with telemetry, update checks,
 *     and CrUX reporting disabled.
 *
 * Protocol constraint: MCP uses JSON-RPC over stdin/stdout with NDJSON line
 * framing. The wrapper writes protocol messages only to stdout and logs only
 * to stderr.
 *
 * Usage:
 *   node browser-wrapper.mjs <chrome-devtools-mcp-bin|@pinvou/browser-core> <host-state-json> [extra-args...]
 *
 * Exit: the wrapper lifetime is the MCP server lifetime. On Windows it also
 * owns the chrome-devtools-mcp child process; BrowserCore platforms create no
 * additional MCP child process.
 */

import { execFile, spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import {
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs';
import { dirname, join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

import {
  adaptBrowserCatalog,
  assertAllowedBrowserToolCall,
  assertAllowedHostedNavigation,
  BROWSER_PROTOCOL_FRAME_MAX_BYTES,
  browserHostBackendPolicy,
  browserToolMayMutate,
  buildBijectivePageTokenMaps,
  createBoundedLineBacklog,
  createBoundedNdjsonDecoder,
  createHostCallerHeartbeat,
  createHostCancellationTombstone,
  createHostRequestEnvelope,
  createOrderedWritableQueue,
  explicitOwnedPageId,
  filterPagesResult,
  findHostedTabPage,
  findHostedWorkspacePage,
  HOST_CALLER_HEARTBEAT_INTERVAL_MS,
  hostLeaseAssertionPayload,
  hostCallerHeartbeatArtifactName,
  hostMutationAuthorizationPayload,
  hostRequestArtifactNames,
  inputToolNames,
  isRecoverableHostCoreWorkspaceError,
  isReusableBootstrapBlankPage,
  pageScopedToolNames,
  parseAuthoritativeHostWorkspace,
  parseBrowserPages,
  parseHostActivationLease,
  parseHostResponseEnvelope,
  PERSISTED_BROWSER_LAST_ERROR_CODES,
  remapCancellationNotification,
  routeToolCallToPage,
  runLeasedHostDispatch,
  runVisiblePageOperation,
  uncancelledBufferedRequests,
} from './browser-wrapper-protocol.mjs';
import { createPinvouBrowserCoreCatalog } from './browser-core-protocol.mjs';

const log = (...args) => console.error('[browser-wrapper]', ...args);
const BROWSER_PERF_LOG_ENABLED = process.env.PINVOU3_BROWSER_PERF_LOG === '1';
const WINDOWS_TRUSTED_INPUT_WINDOW_MS = 750;
const WINDOWS_TRUSTED_INPUT_HEARTBEAT_INTERVAL_MS = 250;
// Fail the cooperative refresh before the current Rust suppression window can
// expire. The 100ms reserve absorbs timer and host-file scheduling jitter.
const WINDOWS_TRUSTED_INPUT_REFRESH_TIMEOUT_MS =
  WINDOWS_TRUSTED_INPUT_WINDOW_MS - WINDOWS_TRUSTED_INPUT_HEARTBEAT_INTERVAL_MS - 100;
const WINDOWS_AGENT_OPERATION_WINDOW_MS = 30_000;
const WINDOWS_AGENT_OPERATION_HEARTBEAT_INTERVAL_MS = 5_000;
// Keep a failed refresh observable well before the 30s operation lease can
// expire. interval + timeout is deliberately bounded by the operation window.
const WINDOWS_AGENT_OPERATION_REFRESH_TIMEOUT_MS = Math.min(
  5_000,
  WINDOWS_AGENT_OPERATION_WINDOW_MS - WINDOWS_AGENT_OPERATION_HEARTBEAT_INTERVAL_MS,
);
const STARTUP_PENDING_REQUEST_MAX_COUNT = 1024;
const STARTUP_PENDING_REQUEST_MAX_BYTES = BROWSER_PROTOCOL_FRAME_MAX_BYTES;
const STARTUP_PENDING_OUTPUT_MAX_COUNT = 1024;
const STARTUP_PENDING_OUTPUT_MAX_BYTES = BROWSER_PROTOCOL_FRAME_MAX_BYTES;
function recordBrowserPerformance(metric, durationMs) {
  if (!BROWSER_PERF_LOG_ENABLED || !Number.isFinite(durationMs) || durationMs < 0) return;
  process.stderr.write(`[browser-perf] ${JSON.stringify({
    metric,
    durationMs,
    sessionId: SESSION_ID,
    at: new Date().toISOString(),
  })}\n`);
}

// ---------------------------------------------------------------------------
// Arguments: node browser-wrapper.mjs <mcp-bin> <cdp-port-json> [extra...]
// ---------------------------------------------------------------------------
const [, , MCP_BIN_ARG, CDP_PORT_JSON, ...EXTRA_ARGS] = process.argv;
if (!MCP_BIN_ARG || !CDP_PORT_JSON) {
  console.error(
    '[browser-wrapper] usage: node browser-wrapper.mjs <mcp-bin> <cdp-port-json> [extra-args...]'
  );
  process.exit(2);
}

const HOST_CORE_MODE = MCP_BIN_ARG === '@pinvou/browser-core';
const HOST_CORE_UNSUPPORTED_TOOLS = process.platform === 'linux'
  ? new Set(['drag', 'hover', 'resize_page'])
  : process.platform === 'darwin'
    ? new Set(['drag', 'hover', 'resize_page', 'handle_dialog'])
    : new Set();
const HOST_CORE_NON_MUTATING_TOOLS = new Set(['list_pages', 'take_snapshot', 'wait_for']);
const configuredHostCoreTimeout = Number(
  process.env.PINVOU3_BROWSER_HOST_CORE_REQUEST_TIMEOUT_MS,
);
const HOST_CORE_REQUEST_TIMEOUT_MS = Number.isSafeInteger(configuredHostCoreTimeout) &&
  configuredHostCoreTimeout >= 50 && configuredHostCoreTimeout <= 25_000
  ? configuredHostCoreTimeout
  : 25_000;

// Tauri may produce `\\?\C:\...` paths in Windows development and install
// directories. Node's fs APIs accept many verbatim paths, but another Node
// process may misparse one used as its entry-script argument and report
// EISDIR. Normalize here as well so older session configuration recovers.
function nodeCompatibleEntryPath(value) {
  if (process.platform !== 'win32') return value;
  if (value.startsWith('\\\\?\\UNC\\')) return `\\\\${value.slice(8)}`;
  if (value.startsWith('\\\\?\\')) return value.slice(4);
  return value;
}

const MCP_BIN = nodeCompatibleEntryPath(MCP_BIN_ARG);

// chrome-devtools-mcp runtime requirement (upstream package.json engines):
// ^20.19.0 || ^22.12.0 || >=23. With an older system Node, the build-time
// catalog still lets the shim answer handshakes and tools/list, but the first
// real request fails with an actionable reason.
function nodeTooOld() {
  const [major, minor] = process.versions.node.split('.').map(Number);
  return !(major >= 23 || (major === 22 && minor >= 12) || (major === 20 && minor >= 19));
}

// ---------------------------------------------------------------------------
// CDP liveness probe (GET /json/version). Run the subprocess asynchronously and
// yield between retries so a synchronous busy loop cannot starve MCP handshakes,
// timeouts, or host requests during startup.
// ---------------------------------------------------------------------------
function probeCdpOnce(port) {
  return new Promise((resolve) => {
    execFile(
      process.execPath,
      [
        '-e',
        [
          `const http=require('http');`,
          `http.get({host:'127.0.0.1',port:${port},path:'/json/version',timeout:1000},r=>{`,
          `  r.resume();`,
          `  process.exit(r.statusCode===200?0:1)`,
          `}).on('error',()=>process.exit(1));`,
        ].join('\n'),
      ],
      { timeout: 2500, windowsHide: true },
      (error) => resolve(!error),
    );
  });
}

async function probeCdp(port, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await probeCdpOnce(port)) return true;
    await sleep(100);
  }
  return false;
}

// ---------------------------------------------------------------------------
// The port file is read-only here. Only the app host can publish
// `{ port, owner: "app" }`.
// ---------------------------------------------------------------------------
function readPortFile() {
  try {
    const data = JSON.parse(readFileSync(CDP_PORT_JSON, 'utf8'));
    if (typeof data.port === 'number' && data.port > 0 && data.port < 65536) return data;
  } catch {
    /* Missing file or invalid JSON. */
  }
  return null;
}

// Most recent startup failure ({ code, at }). Persist only allowlisted stable
// codes, never host exception text, user paths, or transport details that would
// reach the next session's model instructions. Rust maps these codes to static
// messages. Clear the record after a successful connection.
const LAST_ERROR_JSON = join(dirname(CDP_PORT_JSON), 'last-error.json');
const PERSISTED_LAST_ERROR_CODES = new Set(PERSISTED_BROWSER_LAST_ERROR_CODES);
const HOST_REQUEST_DIR = join(dirname(CDP_PORT_JSON), 'host-requests');
const SESSION_ID = process.env.PINVOU3_BROWSER_SESSION_ID || '';
const SESSION_TOKEN = process.env.PINVOU3_BROWSER_SESSION_TOKEN || '';
const WRAPPER_INSTANCE_NONCE = randomBytes(16).toString('hex');
const WORKSPACE_STATE_JSON = join(dirname(CDP_PORT_JSON), 'workspaces', `${SESSION_TOKEN}.json`);
let hostCallerHeartbeatPath = null;
let hostCallerHeartbeatTimer = null;
const WINDOWS_RENAME_RETRY_DELAYS_MS = [7, 13, 23, 37];
const WINDOWS_RENAME_RETRY_WAIT = new Int32Array(new SharedArrayBuffer(4));

function renameReplaceSync(source, destination) {
  for (let attempt = 0; ; attempt += 1) {
    try {
      renameSync(source, destination);
      return;
    } catch (error) {
      const retryDelay = WINDOWS_RENAME_RETRY_DELAYS_MS[attempt];
      const transientWindowsLock = process.platform === 'win32'
        && ['EACCES', 'EBUSY', 'EPERM'].includes(error?.code);
      if (!transientWindowsLock || retryDelay == null) throw error;
      // Windows can briefly deny replace-rename while Rust, antivirus, or a
      // lifecycle probe has the destination open. Non-round delays also avoid
      // phase-locking a 1s heartbeat writer with a periodic reader.
      Atomics.wait(WINDOWS_RENAME_RETRY_WAIT, 0, 0, retryDelay);
    }
  }
}

function atomicWriteJson(path, value) {
  const tmp = `${path}.tmp`;
  writeFileSync(tmp, JSON.stringify(value), { mode: 0o600 });
  renameReplaceSync(tmp, path);
}

function writeHostCallerHeartbeat() {
  if (!SESSION_ID || !/^[0-9a-f]{16}$/.test(SESSION_TOKEN)) {
    throw new Error('Browser host caller heartbeat is missing a valid session identity');
  }
  mkdirSync(HOST_REQUEST_DIR, { recursive: true });
  hostCallerHeartbeatPath ||= join(
    HOST_REQUEST_DIR,
    hostCallerHeartbeatArtifactName(SESSION_TOKEN, WRAPPER_INSTANCE_NONCE),
  );
  atomicWriteJson(hostCallerHeartbeatPath, createHostCallerHeartbeat({
    sessionId: SESSION_ID,
    sessionToken: SESSION_TOKEN,
    callerPid: process.pid,
    wrapperInstanceNonce: WRAPPER_INSTANCE_NONCE,
  }));
}

function ensureHostCallerHeartbeat() {
  writeHostCallerHeartbeat();
  if (hostCallerHeartbeatTimer) return;
  hostCallerHeartbeatTimer = setInterval(() => {
    try {
      writeHostCallerHeartbeat();
    } catch (error) {
      // A stale/missing heartbeat makes the Rust host reject authority-bearing
      // work. Keep the wrapper alive so a later request can repair a transient
      // filesystem failure by synchronously refreshing before publication.
      log('Failed to refresh browser host caller heartbeat:', error?.message || error);
    }
  }, HOST_CALLER_HEARTBEAT_INTERVAL_MS);
  hostCallerHeartbeatTimer.unref?.();
}

function stopHostCallerHeartbeat() {
  if (hostCallerHeartbeatTimer) clearInterval(hostCallerHeartbeatTimer);
  hostCallerHeartbeatTimer = null;
  if (hostCallerHeartbeatPath) {
    try { unlinkSync(hostCallerHeartbeatPath); } catch { /* already removed */ }
    try { unlinkSync(`${hostCallerHeartbeatPath}.tmp`); } catch { /* no partial write */ }
  }
  hostCallerHeartbeatPath = null;
}

function quarantineTimedOutHostResponse(responsePath, tombstonePath) {
  const sweep = () => {
    try { unlinkSync(responsePath); } catch { /* No late response yet. */ }
    // A cancellation tombstone is a host commit boundary and cannot be revoked
    // by caller-side TTL. Late-response isolation ends only after the host
    // consumes and removes it. A host crash leaves it for the next startup gate.
    if (!existsSync(tombstonePath)) clearInterval(timer);
  };
  const timer = setInterval(sweep, 500);
  timer.unref?.();
  sweep();
}

function cancelTimedOutHostRequest({
  requestId,
  requestPath,
  responsePath,
  tombstonePath,
  reason,
}) {
  // Publish the tombstone before deleting the request. The host must check it
  // before execution and response publication and deduplicate by idempotency_key.
  // This wrapper also quarantines every late response from later requests.
  try {
    atomicWriteJson(tombstonePath, createHostCancellationTombstone({
      requestId,
      sessionId: SESSION_ID,
      sessionToken: SESSION_TOKEN,
      callerPid: process.pid,
      wrapperInstanceNonce: WRAPPER_INSTANCE_NONCE,
      reason,
    }));
  } catch (error) {
    log('Failed to write browser host cancellation tombstone:', error.message);
  }
  try { unlinkSync(requestPath); } catch { /* The host may already own it. */ }
  quarantineTimedOutHostResponse(responsePath, tombstonePath);
}

function createHostRequestId() {
  return `${process.pid}-${Date.now()}-${randomBytes(4).toString('hex')}`;
}

class HostRequestTimeoutError extends Error {
  constructor(operation, timeoutMs) {
    super(`browser/host-request-timeout: ${operation} exceeded ${timeoutMs}ms`);
    this.name = 'HostRequestTimeoutError';
    this.code = 'browser/host-request-timeout';
    this.operation = operation;
    this.timeoutMs = timeoutMs;
    this.commitState = 'unknown';
    this.hostRequestDispatched = true;
  }
}

function markHostRequestAcknowledgementUnknown(error, operation) {
  const marked = error instanceof Error ? error : new Error(String(error));
  marked.operation = operation;
  marked.commitState = 'unknown';
  marked.hostRequestDispatched = true;
  return marked;
}

function isConclusiveHostRejection(response, requestId, idempotencyKey) {
  return response?.protocol_version === 3 &&
    response?.request_id === requestId &&
    response?.idempotency_key === idempotencyKey &&
    response?.ok === false;
}

async function requestHost(
  operation,
  payload = {},
  timeoutMs = 12_000,
  requireHosted = true,
  requestedId = null,
  isCancelled = null,
) {
  if (
    (requireHosted && !hostedWebView2) ||
    !SESSION_ID ||
    !/^[0-9a-f]{16}$/.test(SESSION_TOKEN)
  ) {
    throw new Error('The current session is not a managed WebView2 browser session');
  }
  const requestId = requestedId || createHostRequestId();
  const names = hostRequestArtifactNames(SESSION_TOKEN, requestId);
  const requestPath = join(HOST_REQUEST_DIR, names.request);
  const responsePath = join(HOST_REQUEST_DIR, names.response);
  const tombstonePath = join(HOST_REQUEST_DIR, names.cancelled);
  // Publish/refresh the caller lease synchronously before every request. The
  // periodic heartbeat bounds hard-crash artifacts, while this write prevents
  // event-loop scheduling jitter from making a live request look stale.
  ensureHostCallerHeartbeat();
  atomicWriteJson(requestPath, createHostRequestEnvelope({
    requestId,
    sessionId: SESSION_ID,
    sessionToken: SESSION_TOKEN,
    callerPid: process.pid,
    wrapperInstanceNonce: WRAPPER_INSTANCE_NONCE,
    operation,
    payload,
  }));

  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    // MCP cancellation must reach the host artifact while the request is in
    // flight. Merely suppressing the eventual stdio response would still let
    // a slow BrowserCore resolve finish and dispatch native input afterwards.
    // The host owns the commit/rollback boundary and acknowledges this
    // durable tombstone; an operation already atomically committed is handled
    // by its idempotent compensation record.
    if (typeof isCancelled === 'function' && isCancelled()) {
      cancelTimedOutHostRequest({
        requestId,
        requestPath,
        responsePath,
        tombstonePath,
        reason: 'client-cancelled',
      });
      throw markHostRequestAcknowledgementUnknown(
        new Error('Browser tool call was cancelled'),
        operation,
      );
    }
    if (existsSync(responsePath)) {
      let response;
      try {
        response = JSON.parse(readFileSync(responsePath, 'utf8'));
        const idempotencyKey = `${SESSION_TOKEN}/${requestId}`;
        try {
          return parseHostResponseEnvelope(response, {
            requestId,
            idempotencyKey,
            operation,
            requestedTabToken: payload?.tab_token,
          });
        } catch (error) {
          // A valid negative ledger response is a conclusive host rejection.
          // Any malformed/mismatched acknowledgement after the request file
          // was published leaves a mutation's commit state unknown.
          if (isConclusiveHostRejection(response, requestId, idempotencyKey)) throw error;
          throw markHostRequestAcknowledgementUnknown(error, operation);
        }
      } catch (error) {
        if (error?.hostRequestDispatched || isConclusiveHostRejection(
          response,
          requestId,
          `${SESSION_TOKEN}/${requestId}`,
        )) {
          throw error;
        }
        throw markHostRequestAcknowledgementUnknown(error, operation);
      } finally {
        try { unlinkSync(responsePath); } catch { /* ignore */ }
        try { unlinkSync(requestPath); } catch { /* The host usually removed it. */ }
        try { unlinkSync(tombstonePath); } catch { /* Expected to be absent. */ }
      }
    }
    await sleep(50);
  }

  cancelTimedOutHostRequest({
    requestId,
    requestPath,
    responsePath,
    tombstonePath,
    reason: 'timeout',
  });
  throw new HostRequestTimeoutError(operation, timeoutMs);
}

/**
 * The Windows main application owns the real embedded WebView2. The wrapper
 * only issues an on-demand request and waits for the shared CDP port. Creation
 * fails explicitly so no legacy screenshot mode or second browser window can
 * enter the interface.
 */
async function requestHostedBrowser() {
  if (process.platform !== 'win32') return 0;
  if (!SESSION_ID || !/^[0-9a-f]{16}$/.test(SESSION_TOKEN)) {
    return 0;
  }
  try {
    await requestHost('prepare', { pid: process.pid }, 25_000, false);
  } catch (error) {
    log('On-demand WebView2 startup request failed:', error.message);
    return 0;
  }
  const portFile = readPortFile();
  if (portFile?.port && (await probeCdp(portFile.port, 1000))) return portFile.port;
  return 0;
}

function readWorkspaceState() {
  let value;
  try {
    value = JSON.parse(readFileSync(WORKSPACE_STATE_JSON, 'utf8'));
  } catch {
    /* The workspace may be atomically replaced or not yet created. */
    return null;
  }
  // Accept only the complete authoritative v2 mapping published by the host.
  // Legacy v1 page markers lack target identity and would force URL/order
  // guessing after navigation or an MCP restart, so fail instead of falling back.
  return parseAuthoritativeHostWorkspace(value, SESSION_TOKEN);
}

async function requestHostedOperation(
  operation,
  payload = {},
  timeoutMs = 12_000,
  requestedId = null,
  isCancelled = null,
) {
  return requestHost(
    operation,
    payload,
    timeoutMs,
    true,
    requestedId,
    isCancelled,
  );
}

function stableLastErrorCode(value) {
  const message = value?.message || String(value || '');
  for (const code of PERSISTED_LAST_ERROR_CODES) {
    if (message === code || message.startsWith(`${code}:`)) return code;
  }
  return null;
}

function writeLastErrorCode(code) {
  if (!PERSISTED_LAST_ERROR_CODES.has(code)) return;
  try {
    mkdirSync(dirname(LAST_ERROR_JSON), { recursive: true });
    // `at` uses seconds, matching Rust `browser_unavailability_reason` and its
    // `duration_since(UNIX_EPOCH).as_secs()`. Milliseconds from Date.now() would
    // make `now.saturating_sub(at)` always zero, defeating the 24-hour freshness
    // gate and injecting expired failures indefinitely.
    atomicWriteJson(LAST_ERROR_JSON, { code, at: Math.floor(Date.now() / 1000) });
  } catch {
    /* Persistence failure must not affect the main flow. */
  }
}

function persistKnownLastError(value) {
  const code = stableLastErrorCode(value);
  if (code) writeLastErrorCode(code);
}

function clearLastError() {
  try {
    unlinkSync(LAST_ERROR_JSON);
  } catch {
    /* Absence is already the desired state. */
  }
}

let hostedWebView2 = false;

function isHostedWebView2Port(portFile) {
  return (
    process.platform === 'win32' &&
    portFile?.owner === 'app' &&
    !portFile?.browser_pid
  );
}

// ---------------------------------------------------------------------------
// Native browser-host coordination. Success returns the app-owned CDP port;
// failure throws a readable stable-code error and remains in shim state. This
// path must never discover, start, or reuse external Chrome.
// ---------------------------------------------------------------------------
async function ensureBrowserRunning() {
  const policy = browserHostBackendPolicy(process.platform);
  if (policy.action !== 'request-native-host') {
    hostedWebView2 = false;
    const reason = `${policy.code}: ${policy.message}; external Chrome will not be started`;
    throw new Error(reason);
  }

  // Every Windows task conversation must register with the host and create its
  // own child WebView first. cdp-port.json must name a live endpoint owned by
  // this app; otherwise the wrapper could attach to external pages or identity.
  const hostedPort = await requestHostedBrowser();
  const portFile = readPortFile();
  if (hostedPort > 0 && isHostedWebView2Port(portFile)) {
    hostedWebView2 = true;
    clearLastError();
    return hostedPort;
  }

  hostedWebView2 = false;
  const reason = 'browser/host-backend-unavailable: in-app WebView2 is not ready; restart PINVOU and retry; external Chrome will not be started';
  throw new Error(reason);
}

// ---------------------------------------------------------------------------
// MCP catalog (source for initialize and tools/list responses)
//
// The build-time vendor script captures `catalog-shim.json` beside the MCP bin.
// The official server registers its catalog statically without a browser
// connection, so the shim can capture and answer it offline. If the file is
// absent (for example, development points at a custom bin), probe once at
// runtime without starting Chrome; upstream only connects through getContext()
// on tools/call.
// ---------------------------------------------------------------------------
const CATALOG_JSON = join(dirname(MCP_BIN), '..', '..', '..', 'catalog-shim.json');
let catalog = null;

function validCatalog(value) {
  return (
    value &&
    typeof value === 'object' &&
    value.initializeResult &&
    typeof value.initializeResult === 'object' &&
    value.toolsListResult &&
    Array.isArray(value.toolsListResult.tools)
  );
}

// Windows native mode keeps the complete catalog. Workspace routing below owns
// new/list/select/close, and every page-scoped tool receives the current
// conversation pageId so the Agent cannot access another conversation target.
function loadCatalogFile() {
  try {
    const data = JSON.parse(readFileSync(CATALOG_JSON, 'utf8'));
    if (validCatalog(data)) return data;
    log('catalog-shim.json has an invalid shape; falling back to runtime probing');
  } catch {
    /* Missing file or invalid JSON: probe at runtime. */
  }
  return null;
}

async function probeCatalog() {
  return new Promise((resolve) => {
    let child;
    try {
      child = spawn(
        process.execPath,
        [MCP_BIN, '--no-usage-statistics', '--no-performance-crux'],
        {
          stdio: ['pipe', 'pipe', 'ignore'],
          env: {
            ...process.env,
            CHROME_DEVTOOLS_MCP_NO_UPDATE_CHECKS: '1',
            CI: '1',
          },
        }
      );
    } catch {
      resolve(null);
      return;
    }
    const outputDecoder = createBoundedNdjsonDecoder({
      source: 'browser catalog probe stdout',
    });
    let initializeResult = null;
    const tools = [];
    let done = false;
    let listId = 100;
    const finish = (value) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      try {
        child.kill('SIGKILL');
      } catch {
        /* ignore */
      }
      resolve(value);
    };
    const timer = setTimeout(() => finish(null), 20000);
    child.on('error', () => finish(null));
    child.on('exit', () =>
      finish(
        initializeResult && tools.length > 0
          ? { initializeResult, toolsListResult: { tools } }
          : null
      )
    );
    child.stdout.on('data', (chunk) => {
      let lines;
      try {
        lines = outputDecoder.push(chunk);
      } catch {
        finish(null);
        return;
      }
      for (const line of lines) {
        let msg;
        try {
          msg = JSON.parse(line);
        } catch {
          continue;
        }
        if (msg.id === 1 && msg.result) {
          initializeResult = msg.result;
          child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
          child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: listId, method: 'tools/list', params: {} }) + '\n');
        } else if (msg.id === listId && msg.result) {
          tools.push(...(msg.result.tools ?? []));
          if (msg.result.nextCursor) {
            listId += 1;
            child.stdin.write(
              JSON.stringify({ jsonrpc: '2.0', id: listId, method: 'tools/list', params: { cursor: msg.result.nextCursor } }) + '\n'
            );
          } else {
            finish({ initializeResult, toolsListResult: { tools } });
          }
        }
      }
    });
    child.stdin.write(
      JSON.stringify({
        jsonrpc: '2.0',
        id: 1,
        method: 'initialize',
        params: {
          protocolVersion: '2024-11-05',
          capabilities: {},
          clientInfo: { name: 'pinvou-browser-wrapper', version: '0' },
        },
      }) + '\n'
    );
  });
}

// ---------------------------------------------------------------------------
// stdio shim / transparent proxy state machine
// ---------------------------------------------------------------------------
// shim:     wrapper directly answers initialize/ping/tools/list without
//           preparing the browser host; every other request triggers startup.
// starting: native host and MCP child are being prepared. Buffer request lines
//           and record cancellations. Startup failure answers buffered requests
//           and returns to the retryable shim state.
// proxy:    bidirectional passthrough (stdin to child, child stdout to stdout).
let state = 'shim';
let startPromise = null;
let mcpChild = null;
let clientInitializeParams = null;
const bufferedLines = createBoundedLineBacklog({
  maxLines: STARTUP_PENDING_REQUEST_MAX_COUNT,
  maxBytes: STARTUP_PENDING_REQUEST_MAX_BYTES,
  source: 'browser wrapper startup request backlog',
});
const cancelledIds = new Set();
const hostCoreRequestIds = new Set();
let hostCorePrepared = false;
let hostCoreQueue = Promise.resolve();
let shuttingDown = false;
let shutdownPromise = null;
const WRAPPER_SHUTTING_DOWN_ERROR =
  'browser/wrapper-shutting-down: browser wrapper is shutting down';

const protocolOutput = createOrderedWritableQueue(process.stdout, {
  onError(error) {
    console.error('[browser-wrapper] protocol stdout failed:', error);
    void gracefulShutdown(1, `browser wrapper stdout failed: ${error.message}`, {
      cancelAcceptedRequests: true,
    });
  },
});

function writeRawOut(value) {
  void protocolOutput.write(value);
}

function writeOut(msg) {
  writeRawOut(JSON.stringify(msg) + '\n');
}

function respondError(id, message) {
  writeOut({ jsonrpc: '2.0', id, error: { code: -32000, message } });
}

function bufferStartupRequest(line) {
  try {
    bufferedLines.push(line);
    return true;
  } catch (error) {
    let requestId = null;
    try {
      requestId = JSON.parse(line)?.id ?? null;
    } catch {
      // A malformed line has no response identity, but the bounded shutdown
      // still protects every request already accepted during startup.
    }
    const reason = error?.message || String(error);
    if (requestId != null) respondError(requestId, reason);
    log('Browser wrapper startup backlog limit reached:', reason);
    void gracefulShutdown(1, reason, { cancelAcceptedRequests: true });
    return false;
  }
}

async function ensureHostedBrowserCore(isCancelled = null) {
  if (hostCorePrepared) return;
  await requestHost('prepare', { pid: process.pid }, 25_000, false, null, isCancelled);
  hostedWebView2 = true;
  hostCorePrepared = true;
  clearLastError();
}

function resetHostedBrowserCorePreparation() {
  hostCorePrepared = false;
  hostedWebView2 = false;
}

function assertHostCoreRequestActive(requestId) {
  if (shuttingDown || cancelledIds.has(requestId)) {
    throw new Error('Browser tool call was cancelled');
  }
}

function hostMutationCommitUnknownOutcome(
  message,
  error,
  {
    errorCode = 'browser/action-commit-unknown-after-host-acknowledgement-loss',
    hostOperation = error?.operation,
  } = {},
) {
  const toolName = message.params?.name;
  const hostError = error?.message || String(error);
  return {
    content: [{
      type: 'text',
      text:
        `The ${toolName || 'browser'} action crossed the native-host dispatch boundary, but its ` +
        'final acknowledgement or exact compensation was not proven, so Pinvou cannot prove ' +
        'that the page mutation did not occur. ' +
        'Do not repeat the action; inspect the page state before continuing. ' +
        `Host error: ${hostError}`,
    }],
    isError: true,
    structuredContent: {
      errorCode,
      outcome: 'unknown',
      actionCommitted: true,
      actionCommitState: 'unknown',
      actionMayHaveCommitted: true,
      retryable: false,
      toolName,
      hostOperation,
      hostError,
    },
  };
}

function committedActionFollowupFailureOutcome(
  message,
  error,
  {
    errorCode = 'browser/action-committed-but-post-sync-failed',
    actionOperation = message.params?.name,
  } = {},
) {
  const toolName = message.params?.name;
  const followupError = error?.message || String(error);
  return {
    content: [{
      type: 'text',
      text:
        `The ${toolName || 'browser'} action was committed, but Pinvou could not refresh the ` +
        'page view afterwards. Do not repeat the action; refresh or inspect the page state before ' +
        `continuing. Follow-up error: ${followupError}`,
    }],
    isError: true,
    structuredContent: {
      errorCode,
      outcome: 'committed',
      actionCommitted: true,
      actionCommitState: 'committed',
      actionMayHaveCommitted: true,
      retryable: false,
      toolName,
      actionOperation,
      followupError,
    },
  };
}

function hostCommitUnknownErrorCode(error) {
  const message = error?.message || String(error);
  for (const code of [
    'browser/action-commit-unknown-after-tab-navigation',
    'browser/action-commit-unknown-after-tab-close',
  ]) {
    if (message === code) return code;
    if (!message.startsWith(code)) continue;
    const separator = message.charAt(code.length);
    if (separator === ':' || /\s/.test(separator)) return code;
  }
  return null;
}

function hostCoreMutationTimeoutOutcome(message, error) {
  const toolName = message.params?.name;
  if (
    !(error instanceof HostRequestTimeoutError) ||
    HOST_CORE_NON_MUTATING_TOOLS.has(toolName)
  ) {
    return null;
  }
  return hostMutationCommitUnknownOutcome(message, error, {
    errorCode: 'browser/action-commit-unknown-after-host-timeout',
  });
}

async function requestHostedBrowserCoreTool(message) {
  const payload = {
    tool_name: message.params?.name,
    tool_arguments: message.params?.arguments ?? {},
  };

  const isCancelled = () => shuttingDown || cancelledIds.has(message.id);
  await ensureHostedBrowserCore(isCancelled);
  assertHostCoreRequestActive(message.id);
  try {
    return await requestHost(
      'core_tool',
      payload,
      HOST_CORE_REQUEST_TIMEOUT_MS,
      true,
      null,
      () => cancelledIds.has(message.id),
    );
  } catch (error) {
    const explicitCommitUnknown = hostCommitUnknownErrorCode(error);
    if (explicitCommitUnknown) {
      return hostMutationCommitUnknownOutcome(message, error, {
        errorCode: explicitCommitUnknown,
        hostOperation: 'core_tool',
      });
    }
    const timeoutOutcome = hostCoreMutationTimeoutOutcome(message, error);
    if (timeoutOutcome) return timeoutOutcome;
    if (!isRecoverableHostCoreWorkspaceError(error)) throw error;

    // browser_stop is owned by the Tauri host, so a long-lived wrapper learns
    // about it only from this pre-dispatch workspace lifecycle error. Refresh
    // the cached preparation once, then retry the still-uncommitted tool once.
    resetHostedBrowserCorePreparation();
    assertHostCoreRequestActive(message.id);
    await ensureHostedBrowserCore(isCancelled);
    assertHostCoreRequestActive(message.id);
    try {
      return await requestHost(
        'core_tool',
        payload,
        HOST_CORE_REQUEST_TIMEOUT_MS,
        true,
        null,
        () => cancelledIds.has(message.id),
      );
    } catch (retryError) {
      const explicitCommitUnknown = hostCommitUnknownErrorCode(retryError);
      if (explicitCommitUnknown) {
        return hostMutationCommitUnknownOutcome(message, retryError, {
          errorCode: explicitCommitUnknown,
          hostOperation: 'core_tool',
        });
      }
      const timeoutOutcome = hostCoreMutationTimeoutOutcome(message, retryError);
      if (timeoutOutcome) return timeoutOutcome;
      if (isRecoverableHostCoreWorkspaceError(retryError)) {
        // Keep the cache honest for the next distinct request, but never loop
        // or dispatch this mutation a third time.
        resetHostedBrowserCorePreparation();
      }
      throw retryError;
    }
  }
}

function queueHostedBrowserCoreCall(message) {
  hostCoreRequestIds.add(message.id);
  hostCoreQueue = hostCoreQueue.then(async () => {
    try {
      if (shuttingDown || cancelledIds.delete(message.id)) return;
      if (HOST_CORE_UNSUPPORTED_TOOLS.has(message.params?.name)) {
        throw new Error(`browser/core-tool-unavailable-on-${process.platform}: ${message.params.name}`);
      }
      const result = await requestHostedBrowserCoreTool(message);
      // Any completed BrowserCore request proves the persistent backend is
      // reachable again, even when the caller cancelled before consuming it.
      clearLastError();
      if (!cancelledIds.delete(message.id)) {
        writeOut({ jsonrpc: '2.0', id: message.id, result });
      }
    } catch (error) {
      const reason = error?.message || String(error);
      const wasCancelled = cancelledIds.delete(message.id);
      // Cancellation, stdin closure and one-off transport/action errors must
      // not poison the next conversation. Persist only explicit long-lived
      // backend availability codes from an uncancelled request.
      if (!wasCancelled && !shuttingDown) {
        persistKnownLastError(error);
        respondError(message.id, reason);
      }
    } finally {
      hostCoreRequestIds.delete(message.id);
      cancelledIds.delete(message.id);
    }
  });
}

function handleShimRequest(msg, raw) {
  // If the catalog file is absent and runtime probing failed, answer handshake
  // and catalog requests truthfully while keeping shim state alive for a later
  // engine reconnect.
  if (!catalog && (msg.method === 'initialize' || msg.method === 'tools/list')) {
    respondError(msg.id, 'Browser MCP catalog is unavailable: catalog-shim.json is missing and probing failed');
    return;
  }
  switch (msg.method) {
    case 'initialize': {
      clientInitializeParams = msg.params ?? null;
      // Echo the requested protocolVersion, matching upstream SDK negotiation.
      // chrome-devtools-mcp returns 2024-11-05 for a 2024-11-05 request.
      const result = { ...catalog.initializeResult };
      if (typeof msg.params?.protocolVersion === 'string') {
        result.protocolVersion = msg.params.protocolVersion;
      }
      writeOut({ jsonrpc: '2.0', id: msg.id, result });
      return;
    }
    case 'ping':
      writeOut({ jsonrpc: '2.0', id: msg.id, result: {} });
      return;
    case 'tools/list':
      writeOut({ jsonrpc: '2.0', id: msg.id, result: catalog.toolsListResult });
      return;
    default:
      triggerStart(raw);
  }
}

function handleLine(line) {
  let msg = null;
  try {
    msg = JSON.parse(line);
  } catch {
    /* Drop malformed engine input; this should not occur in normal operation. */
  }
  if (shuttingDown) {
    if (msg?.id != null) respondError(msg.id, WRAPPER_SHUTTING_DOWN_ERROR);
    return;
  }
  try {
    // Fail closed before preparing the host, buffering startup, or forwarding
    // upstream. Catalog hiding is only a discovery constraint and cannot replace
    // the execution boundary for a direct tools/call.
    assertAllowedBrowserToolCall(msg);
  } catch (error) {
    if (msg?.id != null) respondError(msg.id, error?.message || String(error));
    return;
  }
  if (HOST_CORE_MODE) {
    if (msg?.method === 'notifications/cancelled') {
      if (msg.params?.requestId != null) cancelledIds.add(msg.params.requestId);
      return;
    }
    if (!msg || msg.id == null) return;
    if (msg.method === 'tools/call') {
      queueHostedBrowserCoreCall(msg);
      return;
    }
    handleShimRequest(msg, line);
    return;
  }
  if (state === 'proxy') {
    queueProxyLine(line);
    return;
  }
  if (state === 'starting') {
    // During startup, record cancellations so flush skips those requests. Buffer
    // other requests and discard notifications.
    if (msg && msg.method === 'notifications/cancelled' && msg.params?.requestId != null) {
      cancelledIds.add(msg.params.requestId);
    } else if (msg && msg.id != null) {
      bufferStartupRequest(line);
    }
    return;
  }
  // shim state
  if (!msg) return;
  if (msg.id == null) return; // Notifications such as initialized need no reply.
  handleShimRequest(msg, line);
}

function triggerStart(raw) {
  if (!bufferStartupRequest(raw)) return;
  if (startPromise) return;
  state = 'starting';
  startPromise = startProxy();
}

async function startProxy() {
  let port = 0;
  let startupChild = null;
  try {
    if (nodeTooOld()) {
      const reason = `browser/node-runtime-too-old: current ${process.versions.node}; chrome-devtools-mcp requires ^20.19 || ^22.12 || >=23`;
      throw new Error(reason);
    }
    port = await ensureBrowserRunning();
    // WebView2 /json/version may become available before the DevTools WebSocket
    // is fully ready. The official MCP exits with code 1 in this narrow window.
    // Treat it as transient, confirm CDP remains alive, and retry after a short
    // backoff so the first real browser call does not fail while the second works.
    startupChild = await spawnMcpChildWithRetry(port);
    startupChild.assertAlive();
    if (hostedWebView2) {
      const runtimeCatalog = await callUpstreamRequest('tools/list', {});
      runtimePageScopedTools = pageScopedToolNames(runtimeCatalog);
      runtimeInputTools = inputToolNames(runtimeCatalog);
      await syncWorkspacePages(true);
    }
    if (shuttingDown) {
      throw new Error('browser wrapper stopped during upstream startup');
    }
    // Only a child that survived every post-handshake catalog/workspace check
    // is allowed to own wrapper shutdown. A setup child remains disposable so
    // a recoverable failure can return to the reusable shim without orphans.
    startupChild.activate();
  } catch (e) {
    await startupChild?.retireForReusableShim();
    const reason = e?.message || String(e);
    log('Browser startup failed:', reason);
    const failed = uncancelledBufferedRequests(bufferedLines.drain(), cancelledIds);
    if (!shuttingDown && failed.length > 0) {
      writeLastErrorCode(stableLastErrorCode(e) || 'browser/mcp-runtime-start-failed');
    }
    state = 'shim';
    startPromise = null;
    for (const raw of failed) {
      try {
        const m = JSON.parse(raw);
        if (m.id != null) respondError(m.id, `Browser unavailable: ${reason}`);
      } catch {
        /* ignore */
      }
    }
    cancelledIds.clear();
    return;
  }
  state = 'proxy';
  startHostedBrowserWatchdog(port);
  const pending = uncancelledBufferedRequests(bufferedLines.drain(), cancelledIds);
  startPromise = null;
  for (const raw of pending) {
    queueProxyLine(raw);
  }
  cancelledIds.clear();
}

function writeChildRaw(line) {
  try {
    mcpChild?.stdin.write(line + '\n');
  } catch {
    /* The exit handler owns an already-dead child. */
  }
}

let proxyQueue = Promise.resolve();
const proxyChildDecoder = createBoundedNdjsonDecoder({
  source: 'chrome-devtools-mcp stdout',
});
let internalRequestSeq = 0;
const internalRequests = new Map();
const discardedInternalRequestIds = new Set();
const managedToolRequestIds = new Set();
const cancelledProxyRequestIds = new Set();
const externalToInternalRequestIds = new Map();
const MANAGED_UPSTREAM_SETTLEMENT_GRACE_MS = 5_000;
const pageIdToTabToken = new Map();
const tabTokenToPageId = new Map();
let runtimePageScopedTools = new Set();
let runtimeInputTools = new Set();
let selectedPageId = null;
let workspaceRevision = -1;
const FORWARDED_TO_UPSTREAM = Symbol('forwarded-to-upstream');

function discardLateInternalResponse(requestId) {
  discardedInternalRequestIds.add(requestId);
  const timer = setTimeout(() => discardedInternalRequestIds.delete(requestId), 5 * 60_000);
  timer.unref?.();
}

function unknownManagedDispatchOutcome(
  reason,
  upstreamError = null,
  errorCode = 'browser/action-commit-unknown-after-upstream-interruption',
) {
  const detail = upstreamError ? ` Upstream result: ${upstreamError}` : '';
  return {
    content: [{
      type: 'text',
      text:
        'The browser action was already in flight, but its upstream result does not prove ' +
        'whether the action occurred. Do not repeat the action; ' +
        `inspect the page state before continuing. ${reason}.${detail}`,
    }],
    isError: true,
    structuredContent: {
      errorCode,
      outcome: 'unknown',
      actionCommitState: 'unknown',
      actionCommitted: true,
      actionMayHaveCommitted: true,
      retryable: false,
      reason,
      ...(upstreamError ? { upstreamError } : {}),
    },
  };
}

function armManagedUpstreamSettlementDeadline(internalRequestId, pending, reason) {
  if (!pending?.awaitRealSettlement) return;
  // External cancellation owns the first bounded settlement deadline. A later
  // host-heartbeat failure is a consequence of that same cancellation and
  // must not extend the window by clearing/rearming its timer.
  if (pending.commitStateUnknown) return;
  pending.commitStateUnknown = true;
  pending.settlementReason = reason;
  clearTimeout(pending.timer);
  pending.timer = setTimeout(() => {
    if (internalRequests.get(internalRequestId) !== pending) return;
    log('Managed browser tool did not settle after cooperative cancellation; closing upstream:', reason);
    void gracefulShutdown(1, `${reason}; upstream did not settle`);
  }, MANAGED_UPSTREAM_SETTLEMENT_GRACE_MS);
}

function signalManagedUpstreamCancellation(externalRequestId, reason) {
  const internalRequestId = externalToInternalRequestIds.get(externalRequestId);
  if (internalRequestId == null) return false;
  writeChildRaw(JSON.stringify(remapCancellationNotification({
    jsonrpc: '2.0',
    method: 'notifications/cancelled',
    params: { requestId: externalRequestId, reason },
  }, internalRequestId)));
  const pending = internalRequests.get(internalRequestId);
  if (pending?.awaitRealSettlement) {
    armManagedUpstreamSettlementDeadline(internalRequestId, pending, reason);
  }
  return true;
}

function cancelManagedUpstreamRequest(externalRequestId, reason) {
  const internalRequestId = externalToInternalRequestIds.get(externalRequestId);
  if (internalRequestId == null) return false;
  const pending = internalRequests.get(internalRequestId);
  // Once begin_agent_operation has succeeded, a cancellation notification is
  // cooperative only. Keep the promise registered until the official MCP
  // returns or its child has definitely exited; ending earlier would let late
  // trusted input escape the host operation. Pre-dispatch list/select calls
  // remain eagerly cancellable because they have not committed a page action.
  if (pending?.awaitRealSettlement) {
    signalManagedUpstreamCancellation(externalRequestId, reason);
    return true;
  }
  signalManagedUpstreamCancellation(externalRequestId, reason);
  if (pending) {
    internalRequests.delete(internalRequestId);
    clearTimeout(pending.timer);
    externalToInternalRequestIds.delete(externalRequestId);
    discardLateInternalResponse(internalRequestId);
    pending.reject(new Error(reason));
  }
  return true;
}

function queueProxyLine(line) {
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    writeChildRaw(line);
    return;
  }
  // Catalog refresh after connection must use the same adapted catalog;
  // otherwise upstream tools/list would re-expose take_screenshot/upload_file
  // that the shim hid from the model.
  if (msg?.method === 'tools/list' && msg.id != null && catalog) {
    writeOut({ jsonrpc: '2.0', id: msg.id, result: catalog.toolsListResult });
    return;
  }
  if (hostedWebView2 && msg?.method === 'notifications/cancelled') {
    const requestId = msg.params?.requestId;
    if (requestId != null && managedToolRequestIds.has(requestId)) {
      cancelledProxyRequestIds.add(requestId);
      cancelManagedUpstreamRequest(
        requestId,
        msg.params?.reason || 'Browser tool call was cancelled',
      );
      return;
    }
    // Unmanaged tools retain and forward their external IDs. Never remap one to
    // another internal call.
    writeChildRaw(line);
    return;
  }
  if (!hostedWebView2 || msg?.method !== 'tools/call' || msg.id == null) {
    writeChildRaw(line);
    return;
  }
  managedToolRequestIds.add(msg.id);
  proxyQueue = proxyQueue
    .then(async () => {
      try {
        if (shuttingDown) throw new Error(WRAPPER_SHUTTING_DOWN_ERROR);
        throwIfProxyRequestCancelled(msg.id);
        const result = await routeHostedToolCall(msg, line);
        if (result !== FORWARDED_TO_UPSTREAM && !cancelledProxyRequestIds.has(msg.id)) {
          respondResult(msg.id, result);
        }
      } catch (error) {
        if (!cancelledProxyRequestIds.has(msg.id)) {
          log('Managed tab routing failed:', error?.message || error);
          respondError(msg.id, error?.message || String(error));
        }
      } finally {
        managedToolRequestIds.delete(msg.id);
        cancelledProxyRequestIds.delete(msg.id);
        externalToInternalRequestIds.delete(msg.id);
      }
    });
}

function processProxyChildLine(line) {
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    writeRawOut(line + '\n');
    return;
  }
  const pending = internalRequests.get(msg.id);
  if (pending) {
    internalRequests.delete(msg.id);
    clearTimeout(pending.timer);
    if (
      pending.externalRequestId != null &&
      externalToInternalRequestIds.get(pending.externalRequestId) === msg.id
    ) {
      externalToInternalRequestIds.delete(pending.externalRequestId);
    }
    if (msg.error) {
      const upstreamError = msg.error.message || 'Internal chrome-devtools-mcp call failed';
      if (pending.awaitRealSettlement && pending.commitStateUnknown) {
        pending.resolve(unknownManagedDispatchOutcome(
          pending.settlementReason || 'upstream lifecycle interrupted',
          upstreamError,
        ));
      } else if (pending.awaitRealSettlement && pending.mutationMayHaveCommitted) {
        pending.resolve(unknownManagedDispatchOutcome(
          `upstream ${pending.dispatchedToolName || 'browser mutation'} returned a JSON-RPC error after dispatch`,
          upstreamError,
          'browser/action-commit-unknown-after-upstream-error',
        ));
      } else {
        pending.reject(new Error(upstreamError));
      }
    } else if (msg.result?.isError && pending.awaitRealSettlement && pending.commitStateUnknown) {
      const upstreamError = Array.isArray(msg.result.content)
        ? msg.result.content.map((item) => item?.text || '').filter(Boolean).join('\n')
        : 'chrome-devtools-mcp returned a tool error';
      pending.resolve(unknownManagedDispatchOutcome(
        pending.settlementReason || 'upstream lifecycle interrupted',
        upstreamError,
      ));
    } else if (
      msg.result?.isError &&
      pending.awaitRealSettlement &&
      pending.mutationMayHaveCommitted
    ) {
      const upstreamError = Array.isArray(msg.result.content)
        ? msg.result.content.map((item) => item?.text || '').filter(Boolean).join('\n')
        : 'chrome-devtools-mcp returned a tool error';
      pending.resolve(unknownManagedDispatchOutcome(
        `upstream ${pending.dispatchedToolName || 'browser mutation'} returned a tool error after dispatch`,
        upstreamError,
        'browser/action-commit-unknown-after-upstream-error',
      ));
    } else if (msg.result?.isError && !pending.allowToolErrorResult) {
      const message = Array.isArray(msg.result.content)
        ? msg.result.content.map((item) => item?.text || '').filter(Boolean).join('\n')
        : '';
      pending.reject(new Error(message || 'Internal chrome-devtools-mcp tool call failed'));
    } else {
      pending.resolve(msg.result);
    }
    return;
  }
  // Cancelled or timed-out internal calls can still receive late upstream
  // responses. Internal IDs occupy the wrapper's reserved namespace and must
  // never leak to the engine as external JSON-RPC responses.
  if (discardedInternalRequestIds.delete(msg.id)) return;
  writeRawOut(line + '\n');
}

function onProxyChildData(chunk) {
  let lines;
  try {
    lines = proxyChildDecoder.push(chunk);
  } catch (error) {
    mcpChild?.stdout?.off('data', onProxyChildData);
    const reason = error?.message || String(error);
    log('chrome-devtools-mcp protocol output rejected:', reason);
    void gracefulShutdown(1, reason, { cancelAcceptedRequests: true });
    return;
  }
  for (const line of lines) processProxyChildLine(line);
}

function callUpstreamRequest(
  method,
  params = {},
  timeoutMs = 15_000,
  externalRequestId = null,
  allowToolErrorResult = false,
  awaitRealSettlement = false,
  mutationMayHaveCommitted = false,
  dispatchedToolName = null,
) {
  return new Promise((resolve, reject) => {
    const id = `pinvou-wrapper-internal-${process.pid}-${++internalRequestSeq}`;
    const timer = setTimeout(() => {
      const pending = internalRequests.get(id);
      if (pending?.awaitRealSettlement) {
        const reason = `Internal chrome-devtools-mcp call ${method} timed out`;
        if (externalRequestId != null) {
          signalManagedUpstreamCancellation(externalRequestId, reason);
        } else {
          armManagedUpstreamSettlementDeadline(id, pending, reason);
          writeChildRaw(JSON.stringify({
            jsonrpc: '2.0',
            method: 'notifications/cancelled',
            params: { requestId: id, reason },
          }));
        }
        return;
      }
      internalRequests.delete(id);
      discardLateInternalResponse(id);
      if (
        externalRequestId != null &&
        externalToInternalRequestIds.get(externalRequestId) === id
      ) {
        externalToInternalRequestIds.delete(externalRequestId);
      }
      reject(new Error(`Internal chrome-devtools-mcp call ${method} timed out`));
    }, timeoutMs);
    internalRequests.set(id, {
      resolve,
      reject,
      timer,
      externalRequestId,
      allowToolErrorResult,
      awaitRealSettlement,
      mutationMayHaveCommitted,
      dispatchedToolName,
      commitStateUnknown: false,
      settlementReason: null,
    });
    if (externalRequestId != null) externalToInternalRequestIds.set(externalRequestId, id);
    writeChildRaw(JSON.stringify({
      jsonrpc: '2.0',
      id,
      method,
      params,
    }));
  });
}

function callUpstreamTool(
  name,
  args = {},
  timeoutMs = 15_000,
  externalRequestId = null,
  allowToolErrorResult = false,
  awaitRealSettlement = false,
) {
  return callUpstreamRequest(
    'tools/call',
    { name, arguments: args },
    timeoutMs,
    externalRequestId,
    allowToolErrorResult,
    awaitRealSettlement,
    awaitRealSettlement && browserToolMayMutate(name),
    name,
  );
}

function throwIfProxyRequestCancelled(requestId) {
  if (cancelledProxyRequestIds.has(requestId)) {
    throw new Error('Browser tool call was cancelled');
  }
}

function replacePageTokenMappings(entries) {
  const { pageToToken, tokenToPage } = buildBijectivePageTokenMaps(entries);
  pageIdToTabToken.clear();
  tabTokenToPageId.clear();
  for (const [pageId, tabToken] of pageToToken) pageIdToTabToken.set(pageId, tabToken);
  for (const [tabToken, pageId] of tokenToPage) tabTokenToPageId.set(tabToken, pageId);
}

function removePageTokenMapping(pageId) {
  const tabToken = pageIdToTabToken.get(pageId);
  pageIdToTabToken.delete(pageId);
  if (tabToken != null && tabTokenToPageId.get(tabToken) === pageId) {
    tabTokenToPageId.delete(tabToken);
  }
}

async function discoverWorkspacePages(listResult, state, externalRequestId = null) {
  if (state?.version !== 2 || state?.mapping_authority !== 'host') {
    throw new Error('Page discovery is missing the authoritative host v2 target mapping');
  }
  const pages = parseBrowserPages(listResult);
  const wanted = new Set((state.tabs || []).map((tab) => tab?.token).filter(Boolean));
  const tokenByTarget = new Map((state.tabs || []).map((tab) => [tab.target_id, tab.token]));
  const next = [];
  for (const page of pages) {
    const authoritativeToken = tokenByTarget.get(page.targetId);
    if (authoritativeToken && wanted.has(authoritativeToken)) {
      const known = pageIdToTabToken.get(page.id);
      if (known != null && known !== authoritativeToken) {
        throw new Error('MCP pageId conflicts with the authoritative host target mapping');
      }
      next.push([page.id, authoritativeToken]);
      continue;
    }

    // A non-empty structured target is authoritative. If it does not occur in
    // the host workspace, neither an old pageId binding nor a page-controlled
    // URL fragment may turn it into an owned target.
    if (page.targetId) continue;

    // Official chrome-devtools-mcp@1.7.0 does not yet expose raw targetId in
    // list_pages. Within one MCP process, retain only pageId bindings established
    // by an authoritative target or a controlled about:blank marker. After a
    // wrapper/MCP restart, fail when neither targetId nor marker exists; never
    // guess by URL/title or inspect the remote page main world. Once the vendor
    // adapter supplies targetId, this branch only handles first about:blank boot.
    const known = pageIdToTabToken.get(page.id);
    if (known && wanted.has(known)) {
      next.push([page.id, known]);
      continue;
    }
    let bootstrapToken = null;
    if (page.url === `about:blank#pinvou-session-${SESSION_TOKEN}`) {
      bootstrapToken = SESSION_TOKEN;
    }
    const marker = page.url.match(/^about:blank#pinvou-tab-([0-9a-f]{16})$/);
    if (!bootstrapToken && marker && wanted.has(marker[1])) bootstrapToken = marker[1];
    if (bootstrapToken && wanted.has(bootstrapToken)) {
      next.push([page.id, bootstrapToken]);
    }
  }
  replacePageTokenMappings(next);
}

function pageIdForToken(token) {
  return tabTokenToPageId.get(token) ?? null;
}

async function selectUpstreamPage(pageId, externalRequestId = null, force = false) {
  if (!Number.isInteger(pageId)) throw new Error('Browser tab is not yet bound to MCP');
  if (!force && selectedPageId === pageId) return null;
  const result = await callUpstreamTool(
    'select_page',
    { pageId, bringToFront: false },
    15_000,
    externalRequestId,
  );
  selectedPageId = pageId;
  return result;
}

async function syncWorkspacePages(force = false, externalRequestId = null) {
  const workspace = readWorkspaceState();
  if (!workspace) {
    throw new Error('browser/workspace-missing: browser workspace state for this conversation is not ready');
  }
  const activeKnown = pageIdForToken(workspace.active_tab);
  if (!force && workspace.revision === workspaceRevision && activeKnown != null) {
    await selectUpstreamPage(activeKnown, externalRequestId);
    return { workspace, listResult: null };
  }
  const listResult = await callUpstreamTool('list_pages', {}, 15_000, externalRequestId);
  await discoverWorkspacePages(listResult, workspace, externalRequestId);
  workspaceRevision = workspace.revision;
  const activePageId = pageIdForToken(workspace.active_tab);
  if (activePageId == null) {
    throw new Error('No WebView2 page matches the active tab for this conversation');
  }
  await selectUpstreamPage(activePageId, externalRequestId);
  return { workspace, listResult };
}

function resetHostedWebView2WorkspaceRouting() {
  pageIdToTabToken.clear();
  tabTokenToPageId.clear();
  selectedPageId = null;
  workspaceRevision = -1;
}

async function prepareHostedWebView2WorkspaceForRetry(externalRequestId) {
  throwIfProxyRequestCancelled(externalRequestId);
  const port = await requestHostedBrowser();
  const portFile = readPortFile();
  if (!(port > 0 && isHostedWebView2Port(portFile))) {
    throw new Error('browser/workspace-unavailable: failed to prepare the in-app WebView2 workspace again');
  }
  hostedWebView2 = true;
  clearLastError();
  throwIfProxyRequestCancelled(externalRequestId);
}

/**
 * browser_stop destroys only the current conversation workspace. The shared
 * CDP port remains healthy while other conversations live, so a long-running
 * Windows wrapper cannot equate CDP liveness with this session's workspace
 * liveness. Before any lease, host operation, or upstream page side effect,
 * retry an explicit workspace-lifecycle failure once through reset -> prepare
 * -> sync. A failed retry remains unprepared and never enters a third attempt.
 */
async function syncWorkspacePagesBeforeDispatch(force = false, externalRequestId = null) {
  try {
    return await syncWorkspacePages(force, externalRequestId);
  } catch (error) {
    if (!isRecoverableHostCoreWorkspaceError(error)) throw error;
  }

  resetHostedWebView2WorkspaceRouting();
  await prepareHostedWebView2WorkspaceForRetry(externalRequestId);
  try {
    return await syncWorkspacePages(true, externalRequestId);
  } catch (retryError) {
    if (isRecoverableHostCoreWorkspaceError(retryError)) {
      resetHostedWebView2WorkspaceRouting();
    }
    throw retryError;
  }
}

function respondResult(id, result) {
  writeOut({ jsonrpc: '2.0', id, result });
}

function filteredPagesResult(listResult) {
  return filterPagesResult(
    listResult,
    new Set(pageIdToTabToken.keys()),
    selectedPageId,
  );
}

async function verifyHostedPageAlignment(pageId, tabToken, externalRequestId) {
  throwIfProxyRequestCancelled(externalRequestId);
  const beforeList = readWorkspaceState();
  if (
    !beforeList ||
    beforeList.active_tab !== tabToken ||
    !(beforeList.tabs || []).some((tab) => tab?.token === tabToken)
  ) {
    throw new Error('The host-visible tab does not match the Agent target page');
  }

  const listResult = await callUpstreamTool('list_pages', {}, 15_000, externalRequestId);
  const workspace = readWorkspaceState();
  if (
    !workspace ||
    workspace.active_tab !== tabToken ||
    !(workspace.tabs || []).some((tab) => tab?.token === tabToken)
  ) {
    throw new Error('The host-visible tab changed after the Agent selected a Target');
  }
  await discoverWorkspacePages(listResult, workspace, externalRequestId);
  workspaceRevision = workspace.revision;
  const selected = parseBrowserPages(listResult).find((page) => page.id === pageId);
  if (pageIdToTabToken.get(pageId) !== tabToken || selected?.selected !== true) {
    throw new Error('The Agent Target could not be aligned with the host-visible tab');
  }
  selectedPageId = pageId;
  return { workspace, listResult };
}

async function runOnVisibleHostedPage(
  msg,
  pageId,
  execute = null,
  { emitsTrustedInput = false, observationalOnly = false } = {},
) {
  return runVisiblePageOperation({
    pageId,
    pageTokens: pageIdToTabToken,
    ensureActive: () => throwIfProxyRequestCancelled(msg.id),
    activateTab: async (tabToken) => {
      const workspace = readWorkspaceState();
      const targetId = workspace?.tabs
        ?.find((tab) => tab?.token === tabToken)
        ?.target_id;
      if (typeof targetId !== 'string' || !targetId) {
        throw new Error('Target tab is missing the authoritative host target mapping');
      }
      const activation = await requestHostedOperation(
        'activate_tab',
        { tab_token: tabToken },
        12_000,
        null,
        () => cancelledProxyRequestIds.has(msg.id),
      );
      workspaceRevision = -1;
      return parseHostActivationLease(activation, {
        sessionId: SESSION_ID,
        tabToken,
        targetId,
      });
    },
    assertLease: ({ activationResult }) => requestHostedOperation(
      'assert_host_lease',
      hostLeaseAssertionPayload(activationResult),
    ),
    selectPage: (targetPageId) => selectUpstreamPage(targetPageId, msg.id, true),
    verify: ({ pageId: targetPageId, tabToken }) =>
      verifyHostedPageAlignment(targetPageId, tabToken, msg.id),
    recordAlignment: (durationMs) =>
      recordBrowserPerformance('agent_target_alignment_ms', durationMs),
    execute: execute
      ? async (target) => {
          const ensureDispatchActive = () => {
            throwIfProxyRequestCancelled(msg.id);
            const workspace = readWorkspaceState();
            if (workspace?.active_tab !== target.tabToken) {
              throw new Error('The host-visible tab changed before tool execution');
            }
          };
          return runLeasedHostDispatch({
            activationLease: target.activationResult,
            emitsTrustedInput,
            ensureActive: ensureDispatchActive,
            beginOperation: ({ lease, emitsTrustedInput: emitsInput }) =>
              requestHostedOperation('begin_agent_operation', {
                ...hostLeaseAssertionPayload(lease),
                emits_trusted_input: emitsInput,
                observational_only: observationalOnly,
              }),
            refreshOperation: (lease) => requestHostedOperation(
              emitsTrustedInput ? 'refresh_agent_input' : 'refresh_agent_operation',
              hostLeaseAssertionPayload(lease),
              emitsTrustedInput
                ? WINDOWS_TRUSTED_INPUT_REFRESH_TIMEOUT_MS
                : WINDOWS_AGENT_OPERATION_REFRESH_TIMEOUT_MS,
            ),
            onRefreshFailure: (error) => {
              // Cooperative cancellation deliberately leaves the pending
              // request registered. The leased dispatch waits for the real
              // upstream response (or its existing timeout) before ending the
              // host operation, so late trusted input cannot escape after end.
              signalManagedUpstreamCancellation(
                msg.id,
                `${emitsTrustedInput ? 'trusted-input' : 'agent-operation'} heartbeat failed: ${error?.message || error}`,
              );
            },
            endOperation: (lease) => requestHostedOperation(
              'end_agent_operation',
              hostLeaseAssertionPayload(lease),
            ),
            heartbeatIntervalMs: emitsTrustedInput
              ? WINDOWS_TRUSTED_INPUT_HEARTBEAT_INTERVAL_MS
              : WINDOWS_AGENT_OPERATION_HEARTBEAT_INTERVAL_MS,
            onEndFailure: (error) => log(
              'end_agent_operation cleanup failed after committed tool result:',
              error?.message || error,
            ),
            execute: () => execute(target),
          });
        }
      : null,
  });
}

async function routeHostedToolCall(msg, raw) {
  const name = msg.params?.name;
  const args = msg.params?.arguments ?? {};
  // Validate URLs before any upstream passthrough or page alignment. Legacy
  // clients may supply url without type; assertAllowedHostedNavigation treats
  // such a call as type=url.
  if (name === 'navigate_page') assertAllowedHostedNavigation(args);
  if (name === 'list_pages') {
    const { listResult } = await syncWorkspacePagesBeforeDispatch(true, msg.id);
    return filteredPagesResult(listResult);
  }
  if (name === 'select_page') {
    // Explicit pageId must pass current-conversation ownership before any
    // list/select/host side effect.
    const selectedPage = explicitOwnedPageId(args, pageIdToTabToken);
    if (selectedPage == null) {
      throw new Error('Tab does not exist or does not belong to this conversation');
    }
    const selectedTabToken = pageIdToTabToken.get(selectedPage);
    await syncWorkspacePagesBeforeDispatch(false, msg.id);
    if (pageIdToTabToken.get(selectedPage) !== selectedTabToken) {
      throw new Error('Page ownership changed during synchronization');
    }
    const aligned = await runOnVisibleHostedPage(msg, selectedPage);
    return filterPagesResult(
      aligned.verificationResult.listResult,
      new Set(pageIdToTabToken.keys()),
      selectedPageId,
    );
  }
  if (name === 'new_page') {
    if (typeof args.url !== 'string') throw new Error('new_page is missing url');
    assertAllowedHostedNavigation({ url: args.url });
    if (args.isolatedContext) {
      throw new Error('The in-app browser does not support isolatedContext; tabs share the current sign-in state');
    }
    // new_page needs a fresh list result so the one-time bootstrap blank can
    // be distinguished from a real page without guessing from title/history.
    const previous = await syncWorkspacePagesBeforeDispatch(true, msg.id);
    const previousActiveToken = previous.workspace.active_tab;
    const previousPageId = pageIdForToken(previousActiveToken);
    if (previousPageId == null) {
      throw new Error('Could not authorize the current host page before creating a tab');
    }
    const previousPage = parseBrowserPages(previous.listResult)
      .find((page) => page.id === previousPageId);
    if (isReusableBootstrapBlankPage({
      workspace: previous.workspace,
      page: previousPage,
      pageToken: pageIdToTabToken.get(previousPageId),
      background: args.background === true,
    })) {
      // The initial page is already the visible, authoritative host target.
      // Reuse it inside the normal lease window so takeover/cancellation rules
      // stay identical to an ordinary navigate_page call.
      await runOnVisibleHostedPage(
        msg,
        previousPageId,
        ({ pageId }) => callUpstreamTool(
          'navigate_page',
          { type: 'url', url: args.url, pageId },
          120_000,
          msg.id,
          false,
          true,
        ),
      );
      workspaceRevision = -1;
      try {
        const { listResult } = await syncWorkspacePages(true, msg.id);
        return filteredPagesResult(listResult);
      } catch (error) {
        // The navigate_page result above is the authoritative commit ACK. A
        // later list/select failure cannot make replaying new_page safe: the
        // same retry would now create a second tab instead of reusing blank.
        return committedActionFollowupFailureOutcome(msg, error, {
          actionOperation: 'navigate_page',
        });
      }
    }
    // create_tab mutates host state. First acquire a CAS lease on the active
    // page; the host atomically verifies authorization_tab_token, target,
    // revision, and lease before creating the tab.
    const creationAuthorization = await runOnVisibleHostedPage(msg, previousPageId);
    const tabToken = randomBytes(8).toString('hex');
    const creationId = createHostRequestId();
    let createAttempted = false;
    let createAcknowledged = false;
    try {
      const createdTab = await runLeasedHostDispatch({
        activationLease: creationAuthorization.activationResult,
        ensureActive: () => throwIfProxyRequestCancelled(msg.id),
        beginOperation: ({ lease }) => requestHostedOperation(
          'begin_agent_operation',
          {
            ...hostLeaseAssertionPayload(lease),
            emits_trusted_input: false,
          },
        ),
        refreshOperation: (lease) => requestHostedOperation(
          'refresh_agent_operation',
          hostLeaseAssertionPayload(lease),
          WINDOWS_AGENT_OPERATION_REFRESH_TIMEOUT_MS,
        ),
        onRefreshFailure: (error) => {
          signalManagedUpstreamCancellation(
            msg.id,
            `agent-operation heartbeat failed: ${error?.message || error}`,
          );
        },
        endOperation: (lease) => requestHostedOperation(
          'end_agent_operation',
          hostLeaseAssertionPayload(lease),
        ),
        heartbeatIntervalMs: WINDOWS_AGENT_OPERATION_HEARTBEAT_INTERVAL_MS,
        onEndFailure: (error) => log(
          'end_agent_operation cleanup failed after committed create_tab:',
          error?.message || error,
        ),
        execute: () => {
          createAttempted = true;
          return requestHostedOperation(
            'create_tab',
            {
              tab_token: tabToken,
              ...hostMutationAuthorizationPayload(creationAuthorization.activationResult),
              url: args.url,
              background: args.background === true,
            },
            12_000,
            creationId,
            () => cancelledProxyRequestIds.has(msg.id),
          );
        },
      });
      createAcknowledged = true;
      if (createdTab.creationId !== creationId) {
        throw new Error('create_tab returned a mismatched creationId');
      }
      workspaceRevision = -1;
      throwIfProxyRequestCancelled(msg.id);
      let page = null;
      for (let attempt = 0; attempt < 60 && !page; attempt += 1) {
        throwIfProxyRequestCancelled(msg.id);
        const listResult = await callUpstreamTool('list_pages', {}, 15_000, msg.id);
        const workspace = readWorkspaceState();
        if (workspace) {
          const authoritativeTarget = workspace.tabs
            .find((tab) => tab.token === tabToken)
            ?.target_id;
          if (authoritativeTarget && authoritativeTarget !== createdTab.targetId) {
            throw new Error('create_tab targetId does not match the authoritative host mapping');
          }
          await discoverWorkspacePages(listResult, workspace, msg.id);
        }
        page = findHostedTabPage(listResult, tabToken, pageIdToTabToken);
        if (!page) await sleep(50);
      }
      if (!page) throw new Error('The newly embedded tab did not appear in the MCP page list');
      if (
        pageIdToTabToken.get(page.id) !== tabToken ||
        tabTokenToPageId.get(tabToken) !== page.id
      ) {
        throw new Error('The new tab did not produce a unique pageId-to-tabToken mapping');
      }
      // The host performs initial URL navigation inside an unpublished staging tab
      // and commits it with the create CAS. The lease expires after create
      // succeeds. Direct wrapper navigation would let a background tab bypass
      // user takeover, so only discover the authoritative target here and then
      // synchronize selection with the host active state.
      const { listResult } = await syncWorkspacePages(true, msg.id);
      return filteredPagesResult(listResult);
    } catch (error) {
      const navigationCommitUnknown = hostCommitUnknownErrorCode(error);
      if (
        createAttempted &&
        navigationCommitUnknown === 'browser/action-commit-unknown-after-tab-navigation'
      ) {
        // Rust reached the hidden page-navigation side-effect boundary but
        // could not prove the final creation CAS. Closing/rolling back the
        // staging WebView cannot undo HTTP or script effects, so wrapper-side
        // compensation must not downgrade this to an ordinary retryable error.
        return hostMutationCommitUnknownOutcome(msg, error, {
          errorCode: navigationCommitUnknown,
          hostOperation: 'create_tab',
        });
      }
      let rollbackProved = false;
      if (createAttempted) {
        // A create-response validation, target discovery, or navigation failure
        // must compensate precisely by creation_id. Ordinary close_tab could
        // close a concurrently replaced or taken-over same-token tab and is
        // forbidden for rollback.
        if (previousPageId != null) {
          try { await selectUpstreamPage(previousPageId); } catch { /* Best effort. */ }
        }
        try {
          await requestHostedOperation('rollback_created_tab', {
            tab_token: tabToken,
            creation_id: creationId,
          });
          rollbackProved = true;
        } catch (rollbackError) {
          log('Failed to roll back an embedded tab whose creation failed:', rollbackError.message);
        }
        const mappedPageId = pageIdForToken(tabToken);
        if (mappedPageId != null) removePageTokenMapping(mappedPageId);
        workspaceRevision = -1;
      }
      if (
        createAttempted &&
        !rollbackProved &&
        (createAcknowledged || error?.hostRequestDispatched)
      ) {
        return hostMutationCommitUnknownOutcome(msg, error, {
          hostOperation: 'create_tab',
        });
      }
      throw error;
    }
  }
  if (name === 'close_page') {
    // Use the same fail-closed ordering as ordinary page tools: a
    // cross-conversation pageId cannot trigger synchronization first.
    const closingPageId = explicitOwnedPageId(args, pageIdToTabToken);
    if (closingPageId == null) {
      throw new Error('Tab does not exist or does not belong to this conversation');
    }
    const closingTabToken = pageIdToTabToken.get(closingPageId);
    const { workspace, listResult } = await syncWorkspacePagesBeforeDispatch(true, msg.id);
    if (pageIdToTabToken.get(closingPageId) !== closingTabToken) {
      throw new Error('Page ownership changed during synchronization');
    }
    if ((workspace.tabs || []).length <= 1) {
      const result = filteredPagesResult(listResult);
      result.content = [{ type: 'text', text: `The last open page cannot be closed.\n\n${result.content?.[0]?.text || ''}` }];
      return result;
    }
    const aligned = await runOnVisibleHostedPage(msg, closingPageId);
    const closingToken = aligned.tabToken;
    const fallbackToken = workspace.tabs
      .map((tab) => tab?.token)
      .find((token) => token && token !== closingToken);
    const fallbackPageId = pageIdForToken(fallbackToken);
    if (fallbackPageId == null) {
      throw new Error('No fallback page is available before closing the tab');
    }
    let closeAcknowledged = false;
    try {
      return await runLeasedHostDispatch({
        activationLease: aligned.activationResult,
        ensureActive: () => throwIfProxyRequestCancelled(msg.id),
        beginOperation: ({ lease }) => requestHostedOperation(
          'begin_agent_operation',
          {
            ...hostLeaseAssertionPayload(lease),
            emits_trusted_input: false,
          },
        ),
        refreshOperation: (lease) => requestHostedOperation(
          'refresh_agent_operation',
          hostLeaseAssertionPayload(lease),
          WINDOWS_AGENT_OPERATION_REFRESH_TIMEOUT_MS,
        ),
        onRefreshFailure: (error) => {
          signalManagedUpstreamCancellation(
            msg.id,
            `agent-operation heartbeat failed: ${error?.message || error}`,
          );
        },
        endOperation: (lease) => requestHostedOperation(
          'end_agent_operation',
          hostLeaseAssertionPayload(lease),
        ),
        heartbeatIntervalMs: WINDOWS_AGENT_OPERATION_HEARTBEAT_INTERVAL_MS,
        onEndFailure: (error) => log(
          'end_agent_operation cleanup failed after committed tool result:',
          error?.message || error,
        ),
        execute: async () => {
          // chrome-devtools-mcp list_pages also reads the selected page. If the
          // selected WebView is destroyed first, later list_pages cannot recover
          // the mapping because the selected page is closed.
          await selectUpstreamPage(fallbackPageId, msg.id, true);
          throwIfProxyRequestCancelled(msg.id);
          await requestHostedOperation(
            'assert_host_lease',
            hostLeaseAssertionPayload(aligned.activationResult),
          );
          await requestHostedOperation('close_tab', {
            tab_token: closingToken,
            ...hostMutationAuthorizationPayload(aligned.activationResult),
          }, 12_000, null, () => cancelledProxyRequestIds.has(msg.id));
          // A valid v3 close acknowledgement is the irreversible mutation
          // boundary. Mapping/list refreshes below are only post-commit view
          // reconciliation and must never turn this call into a retryable
          // JSON-RPC error if they fail.
          closeAcknowledged = true;
          removePageTokenMapping(closingPageId);
          if (selectedPageId === closingPageId) selectedPageId = null;
          workspaceRevision = -1;
          const { listResult: finalListResult } = await syncWorkspacePages(true, msg.id);
          return filteredPagesResult(finalListResult);
        },
      });
    } catch (error) {
      if (closeAcknowledged) {
        return committedActionFollowupFailureOutcome(msg, error, {
          actionOperation: 'close_tab',
        });
      }
      const closeCommitUnknown = hostCommitUnknownErrorCode(error);
      if (closeCommitUnknown) {
        return hostMutationCommitUnknownOutcome(msg, error, {
          errorCode: closeCommitUnknown,
          hostOperation: 'close_tab',
        });
      }
      if (error?.hostRequestDispatched && error?.operation === 'close_tab') {
        return hostMutationCommitUnknownOutcome(msg, error, {
          hostOperation: 'close_tab',
        });
      }
      throw error;
    }
  }

  // Before every ordinary page-scoped tool, align with the user's active tab.
  // After the user clicks a tab, the Agent's next read, click, input, or script
  // operation therefore lands on that same page.
  if (!runtimePageScopedTools.has(name)) {
    // These calls retain their external JSON-RPC ID for direct upstream response;
    // later cancellation notifications pass through unchanged as well.
    throwIfProxyRequestCancelled(msg.id);
    managedToolRequestIds.delete(msg.id);
    writeChildRaw(raw);
    return FORWARDED_TO_UPSTREAM;
  }
  const requestedPageId = explicitOwnedPageId(args, pageIdToTabToken);
  const requestedTabToken = requestedPageId == null
    ? null
    : pageIdToTabToken.get(requestedPageId);
  const { workspace } = await syncWorkspacePagesBeforeDispatch(false, msg.id);
  if (
    requestedPageId != null &&
    pageIdToTabToken.get(requestedPageId) !== requestedTabToken
  ) {
    throw new Error('Page ownership changed during synchronization');
  }
  const pageId = requestedPageId ?? pageIdForToken(workspace.active_tab);
  if (!Number.isInteger(pageId) || !pageIdToTabToken.has(pageId)) {
    throw new Error('Page does not exist or does not belong to this conversation');
  }
  const routed = routeToolCallToPage(msg, pageId);
  const timeoutMs = Number.isInteger(args.timeout)
    ? Math.max(120_000, args.timeout + 5_000)
    : 120_000;
  const aligned = await runOnVisibleHostedPage(
    msg,
    pageId,
    ({ pageId: targetPageId }) => callUpstreamTool(
      name,
      { ...routed.params.arguments, pageId: targetPageId },
      timeoutMs,
      msg.id,
      true,
      true,
    ),
    {
      emitsTrustedInput: runtimeInputTools.has(name),
      observationalOnly: !browserToolMayMutate(name),
    },
  );
  return aligned.executionResult;
}

// Handshake IDs used with the MCP child process are strings and cannot collide
// with the engine's numeric IDs.
const HANDSHAKE_ID = 'pinvou-wrapper-handshake';
const SESSION_LIST_ID_PREFIX = 'pinvou-wrapper-list-';
const SESSION_SELECT_ID = 'pinvou-wrapper-select';

function retryableMcpStartError(error) {
  const message = error?.message || String(error);
  return /chrome-devtools-mcp exited before handshake code=(?:1|null)/.test(message);
}

async function spawnMcpChildWithRetry(port) {
  const maxAttempts = 4;
  let lastError = null;
  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    try {
      return await spawnMcpChild(port);
    } catch (error) {
      lastError = error;
      if (!retryableMcpStartError(error) || attempt === maxAttempts) throw error;
      const delayMs = 200 * attempt;
      log(`chrome-devtools-mcp started too early; retrying in ${delayMs}ms (${attempt}/${maxAttempts})`);
      await sleep(delayMs);
      if (!(await probeCdp(port, 2_000))) {
        throw new Error('WebView2 CDP stopped responding before the MCP retry');
      }
    }
  }
  throw lastError;
}

function spawnMcpChild(port) {
  const mcpArgs = [
    MCP_BIN,
    '--browser-url',
    `http://127.0.0.1:${port}`,
    '--no-usage-statistics',
    '--no-performance-crux',
    // Session isolation depends on the vendor adapter adding
    // structuredContent.pages[].target_id. The official server emits that field
    // in tools/call responses only when structured content is enabled. Enabling
    // page-id routing alone can list pages but cannot build an authoritative host
    // targetId mapping, falsely reporting a successfully shown new tab as failed.
    ...(hostedWebView2
      ? ['--experimental-page-id-routing', '--experimental-structured-content']
      : []),
    ...EXTRA_ARGS,
  ];
  log('Starting chrome-devtools-mcp:', process.execPath, mcpArgs.join(' '));
  return new Promise((resolve, reject) => {
    let child;
    try {
      child = spawn(process.execPath, mcpArgs, {
        stdio: ['pipe', 'pipe', 'inherit'], // Pass stderr logs through.
        env: {
          ...process.env,
          CHROME_DEVTOOLS_MCP_NO_UPDATE_CHECKS: '1', // Offline: disable update checks.
          CI: '1', // Offline: disable usage statistics.
        },
      });
    } catch (e) {
      reject(e);
      return;
    }
    mcpChild = child;
    proxyChildDecoder.reset();
    const pendingOutput = createBoundedLineBacklog({
      maxLines: STARTUP_PENDING_OUTPUT_MAX_COUNT,
      maxBytes: STARTUP_PENDING_OUTPUT_MAX_BYTES,
      source: 'chrome-devtools-mcp startup output backlog',
    });
    let settled = false;
    let startupFailureCleanup = false;
    let activeProxyChild = false;
    let listAttempt = 0;
    const timer = setTimeout(() => {
      if (!settled) {
        fail(new Error('chrome-devtools-mcp handshake or session page binding timed out'));
      }
    }, 25000);

    const fail = (error) => {
      if (settled) return;
      settled = true;
      startupFailureCleanup = true;
      clearTimeout(timer);
      child.stdout.off('data', onData);
      if (mcpChild === child) mcpChild = null;
      try {
        child.kill('SIGKILL');
      } catch {
        /* ignore */
      }
      reject(error);
    };

    const childLifecycle = {
      assertAlive() {
        if (child.exitCode != null || child.signalCode != null || child.killed) {
          throw new Error('chrome-devtools-mcp exited before proxy takeover');
        }
      },
      activate() {
        this.assertAlive();
        activeProxyChild = true;
      },
      async retireForReusableShim() {
        startupFailureCleanup = true;
        if (mcpChild === child) mcpChild = null;
        child.stdout.off('data', onProxyChildData);
        if (child.exitCode == null && child.signalCode == null) {
          try {
            child.kill('SIGKILL');
          } catch {
            /* already stopped */
          }
          await waitExit(child, 1_000);
        }
      },
    };

    const finish = () => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      child.stdout.off('data', onData);
      const initializationOutput = pendingOutput.drain();
      if (initializationOutput.length) writeRawOut(initializationOutput.join(''));
      // The multiplexer now owns stdout so internal list/select/evaluate
      // responses cannot leak to the engine.
      child.stdout.on('data', onProxyChildData);
      resolve(childLifecycle);
    };

    const finishAndForwardRemaining = (lines, currentIndex) => {
      finish();
      for (let index = currentIndex + 1; index < lines.length; index += 1) {
        processProxyChildLine(lines[index]);
      }
    };

    const requestSessionPage = () => {
      if (settled) return;
      listAttempt += 1;
      child.stdin.write(JSON.stringify({
        jsonrpc: '2.0',
        id: `${SESSION_LIST_ID_PREFIX}${listAttempt}`,
        method: 'tools/call',
        params: { name: 'list_pages', arguments: {} },
      }) + '\n');
    };

    const onData = (chunk) => {
      let lines;
      try {
        lines = proxyChildDecoder.push(chunk);
      } catch (error) {
        fail(error);
        return;
      }
      try {
        for (let lineIndex = 0; lineIndex < lines.length; lineIndex += 1) {
          const line = lines[lineIndex];
        let msg;
        try {
          msg = JSON.parse(line);
        } catch {
          pendingOutput.push(line + '\n');
          continue;
        }

        if (msg.id === HANDSHAKE_ID) {
          if (msg.error) {
            fail(new Error(`chrome-devtools-mcp handshake failed: ${msg.error.message}`));
            return;
          }
          writeChildRaw(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }));
          if (hostedWebView2 && SESSION_TOKEN) {
            requestSessionPage();
          } else {
            finishAndForwardRemaining(lines, lineIndex);
          }
          return;
        }

        if (typeof msg.id === 'string' && msg.id.startsWith(SESSION_LIST_ID_PREFIX)) {
          if (msg.error) {
            fail(new Error(`Failed to enumerate conversation browser pages: ${msg.error.message}`));
            return;
          }
          let page = null;
          try {
            const workspace = readWorkspaceState();
            page = workspace
              ? findHostedWorkspacePage(msg.result, workspace, SESSION_TOKEN)
              : null;
          } catch (error) {
            fail(new Error(`Failed to bind the authoritative host page: ${error?.message || error}`));
            return;
          }
          if (!page) {
            if (listAttempt >= 80) {
              fail(new Error('No WebView2 page matches the host active tab'));
            } else {
              setTimeout(requestSessionPage, 150);
            }
            return;
          }
          child.stdin.write(JSON.stringify({
            jsonrpc: '2.0',
            id: SESSION_SELECT_ID,
            method: 'tools/call',
            params: {
              name: 'select_page',
              arguments: { pageId: page.id, bringToFront: false },
            },
          }) + '\n');
          return;
        }

        if (msg.id === SESSION_SELECT_ID) {
          if (msg.error) {
            fail(new Error(`Failed to bind the conversation browser page: ${msg.error.message}`));
          } else {
            log('Bound conversation browser page:', SESSION_TOKEN);
            finishAndForwardRemaining(lines, lineIndex);
          }
          return;
        }

        // Forward other initialization notifications and responses unchanged
        // after binding completes.
        pendingOutput.push(line + '\n');
        }
      } catch (error) {
        fail(error);
      }
    };
    child.stdout.on('data', onData);
    child.on('error', (err) => {
      log('chrome-devtools-mcp startup failed:', err.message);
      if (mcpChild === child) mcpChild = null;
      if (!settled) {
        fail(err);
      }
    });
    child.on('exit', (code, signal) => {
      log('chrome-devtools-mcp exited', { code, signal });
      if (mcpChild === child) mcpChild = null;
      // `fail()` deliberately kills an attempt that never became the active
      // proxy. Its eventual exit belongs only to startup cleanup: treating it
      // as a runtime child crash would shut down the reusable shim and can
      // also clobber a newer retry's `mcpChild` handle.
      if (startupFailureCleanup) return;
      if (!settled) {
        fail(new Error(`chrome-devtools-mcp exited before handshake code=${code}`));
        return;
      }
      if (!activeProxyChild) {
        // Handshake/page binding completed, but the runtime catalog/workspace
        // checks have not promoted this child yet. Settle their internal
        // requests so startProxy can retire the child and return to shim.
        settleInternalRequestsAfterUpstreamStopped(
          `chrome-devtools-mcp exited during post-handshake setup code=${code} signal=${signal}`,
        );
        return;
      }
      void gracefulShutdown(
        code ?? (signal ? 1 : 0),
        `chrome-devtools-mcp exited code=${code} signal=${signal}`,
        { upstreamAlreadyStopped: true },
      );
    });
    // Match the engine initialize arguments, including protocolVersion
    // negotiation and clientInfo.
    const params = clientInitializeParams ?? {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'pinvou', version: '0' },
    };
    try {
      child.stdin.write(
        JSON.stringify({ jsonrpc: '2.0', id: HANDSHAKE_ID, method: 'initialize', params }) + '\n'
      );
    } catch (e) {
      fail(e);
    }
  });
}

// ---------------------------------------------------------------------------
// Child cleanup (SIGTERM -> 3-second grace -> SIGKILL escalation). The app host
// owns browser surfaces; the wrapper only reaps the MCP adapter it started.
// ---------------------------------------------------------------------------
function waitExit(child, timeoutMs) {
  return new Promise((resolve) => {
    if (child.exitCode != null || child.signalCode != null) {
      resolve();
      return;
    }
    const timer = setTimeout(resolve, timeoutMs);
    child.once('exit', () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

async function cleanup() {
  if (mcpChild && !mcpChild.killed) {
    const victim = mcpChild;
    try {
      victim.kill('SIGTERM');
    } catch {
      /* ignore */
    }
    await waitExit(victim, 3000);
    if (victim.exitCode == null && victim.signalCode == null) {
      try {
        victim.kill('SIGKILL');
      } catch {
        /* ignore */
      }
      await waitExit(victim, 1000);
    }
  }
}

function settleInternalRequestsAfterUpstreamStopped(reason) {
  for (const [id, pending] of internalRequests) {
    internalRequests.delete(id);
    clearTimeout(pending.timer);
    if (
      pending.externalRequestId != null &&
      externalToInternalRequestIds.get(pending.externalRequestId) === id
    ) {
      externalToInternalRequestIds.delete(pending.externalRequestId);
    }
    if (pending.awaitRealSettlement) {
      pending.resolve(unknownManagedDispatchOutcome(reason));
    } else {
      pending.reject(new Error(reason));
    }
  }
}

/**
 * Stop accepting protocol work, make the upstream incapable of dispatching any
 * additional browser input, settle its pending promises, and only then wait for
 * every leased dispatch's finally/end_agent_operation before this wrapper exits.
 * All exit paths share this barrier so cancellation, timeout, child crashes,
 * stdin closure, watchdog shutdown, and signals cannot leak a stale Rust op.
 */
function gracefulShutdown(
  exitCode,
  reason,
  { upstreamAlreadyStopped = false, cancelAcceptedRequests = false } = {},
) {
  if (shutdownPromise) return shutdownPromise;
  shuttingDown = true;
  state = 'stopping';
  if (cancelAcceptedRequests) {
    // A caller disconnect/signal abandons every request already accepted from
    // stdin before an awaited startup/queue can make further progress.
    // Internal upstream crashes deliberately do not enter this branch: their
    // claimed request must still receive the structured commit-unknown result.
    for (const requestId of hostCoreRequestIds) cancelledIds.add(requestId);
    for (const raw of bufferedLines.snapshot()) {
      try {
        const message = JSON.parse(raw);
        if (message?.id != null) cancelledIds.add(message.id);
      } catch {
        // Malformed buffered input has no executable request identity.
      }
    }
    for (const requestId of managedToolRequestIds) {
      cancelledProxyRequestIds.add(requestId);
      cancelManagedUpstreamRequest(requestId, reason);
    }
  }
  shutdownPromise = (async () => {
    if (startPromise) {
      try { await startPromise; } catch { /* startup failure is settled below */ }
    }
    if (!HOST_CORE_MODE) {
      if (!upstreamAlreadyStopped) await cleanup();
      settleInternalRequestsAfterUpstreamStopped(reason);
    }
    await Promise.allSettled([proxyQueue, hostCoreQueue]);
    // End/rollback/tombstone requests need the same live epoch during cleanup.
    // Revoke the epoch only after both queues reached their real terminal state.
    stopHostCallerHeartbeat();
    // Do not exit ahead of queued JSON-RPC output. When the peer closes stdout,
    // the queue fails instead of waiting forever and forces a non-zero exit.
    try {
      await protocolOutput.flush();
    } catch (error) {
      console.error('[browser-wrapper] failed to flush protocol stdout:', error);
    }
    process.exit(protocolOutput.failure ? 1 : exitCode);
  })();
  return shutdownPromise;
}

// ---------------------------------------------------------------------------
// The app host owns the browser runtime, so the wrapper has no process-exit
// event for it. After host shutdown or crash, this session's --browser-url is
// permanently stale. Probe periodically and exit after consecutive failures so
// the engine's next wrapper requests a fresh workspace from the native host.
// ---------------------------------------------------------------------------
function startHostedBrowserWatchdog(port) {
  let misses = 0;
  let probing = false;
  const timer = setInterval(() => {
    if (probing) return;
    probing = true;
    void probeCdp(port, 1000)
      .then((alive) => {
        if (alive) {
          misses = 0;
          return;
        }
        misses += 1;
        if (misses >= 2) {
          clearInterval(timer);
          log('In-app browser CDP is disconnected; exiting to renegotiate the native host');
          void gracefulShutdown(1, 'In-app browser CDP is disconnected');
        }
      })
      .catch((error) => {
        log('In-app browser CDP probe failed:', error?.message || error);
      })
      .finally(() => {
        probing = false;
      });
  }, 10000);
  timer.unref();
}

// ---------------------------------------------------------------------------
// Main flow: load catalog -> wait in shim -> request native host and start MCP
// only for the first real request.
// ---------------------------------------------------------------------------
const engineInputDecoder = createBoundedNdjsonDecoder({
  source: 'browser wrapper engine stdin',
});

async function main() {
  if (HOST_CORE_MODE) {
    catalog = createPinvouBrowserCoreCatalog({
      includeAdvancedPointerInput: !['linux', 'darwin'].includes(process.platform),
      // WebDriver's Set Window Rect operates on a top-level native window. A
      // Pinvou browser page is an embedded child surface whose bounds are
      // owned by the right Dock, so advertising resize_page on Linux would
      // either resize the app window or be overwritten by the layout host.
      includeViewportResize: !['linux', 'darwin'].includes(process.platform),
      includeDialog: process.platform !== 'darwin',
    });
  } else {
    catalog = loadCatalogFile();
    if (!catalog) {
      log('catalog-shim.json is missing; probing the catalog without starting a browser');
      catalog = await probeCatalog();
    }
  }
  catalog = adaptBrowserCatalog(catalog);
  if (!catalog) {
    // When the catalog is unavailable, tools/list reports the failure while the
    // process stays alive. Exiting during lazy connect would make every engine
    // reconnect emit another failure.
    log('Tool catalog unavailable: catalog file is missing and runtime probing failed');
    catalog = null;
  }

  const onEngineInputData = (chunk) => {
    let lines;
    try {
      lines = engineInputDecoder.push(chunk);
    } catch (error) {
      process.stdin.off('data', onEngineInputData);
      process.stdin.pause();
      const reason = error?.message || String(error);
      log('Engine protocol input rejected:', reason);
      void gracefulShutdown(1, reason, { cancelAcceptedRequests: true });
      return;
    }
    for (const line of lines) {
      if (line.trim()) handleLine(line);
    }
  };
  process.stdin.on('data', onEngineInputData);
  // Engine stdin closure means disconnect/session end. Exit and reap children
  // from every state.
  process.stdin.on('end', () => {
    void gracefulShutdown(0, 'browser wrapper stdin closed', { cancelAcceptedRequests: true });
  });
  process.stdin.resume();
}

process.on('SIGINT', () => {
  void gracefulShutdown(130, 'browser wrapper received SIGINT', { cancelAcceptedRequests: true });
});
process.on('SIGTERM', () => {
  void gracefulShutdown(143, 'browser wrapper received SIGTERM', { cancelAcceptedRequests: true });
});
process.on('SIGHUP', () => {
  void gracefulShutdown(129, 'browser wrapper received SIGHUP', { cancelAcceptedRequests: true });
});
process.on('uncaughtException', (error) => {
  console.error('[browser-wrapper] uncaught exception:', error);
  void gracefulShutdown(1, `browser wrapper uncaught exception: ${error?.message || error}`, {
    cancelAcceptedRequests: true,
  });
});
process.on('unhandledRejection', (reason) => {
  console.error('[browser-wrapper] unhandled rejection:', reason);
  void gracefulShutdown(1, `browser wrapper unhandled rejection: ${reason?.message || reason}`, {
    cancelAcceptedRequests: true,
  });
});

main().catch((e) => {
  console.error('[browser-wrapper] fatal error:', e);
  void gracefulShutdown(1, `browser wrapper fatal error: ${e?.message || e}`, {
    cancelAcceptedRequests: true,
  });
});
