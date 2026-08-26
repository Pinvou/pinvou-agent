#!/usr/bin/env node
/**
 * browser-wrapper.mjs —— 品悟浏览器 MCP server 的 stdio 协调包装（懒启动代理）。
 *
 * 职责：
 *  1. 懒启动：引擎在会话首个 turn 即 connect 全部 MCP server（CodeWhale
 *     `McpPool::connect_all`），若本包装在进程启动时就准备浏览器，则每个工作
 *     模式会话的首条消息都会常驻浏览器运行时。因此本包装先以
 *     shim 身份直接应答 MCP 握手（`initialize` / `ping` / `tools/list`，目录
 *     来自构建期捕获的 catalog-shim.json），**直到首个 `tools/call`（或其他真实
 *     请求）到达**才准备当前平台的应用内浏览器后端。Windows 随后代理官方
 *     chrome-devtools-mcp；Linux/macOS 则继续由本包装转发 BrowserCore 请求。
 *  2. 与品悟桌面端（Rust BrowserManager）协调任务自有原生页面的生命周期：
 *     - Windows 通过对话级 host-requests/*.json 创建 WebView2，并以 CDP 操作；
 *     - Linux 使用 WebKitGTK + WebKitWebDriver，macOS 使用 WKWebView + AppKit；
 *     - 任一平台的原生宿主不可用时明确报错，绝不启动或复用外部 Chrome；
 *     - 三端共享同一 Agent 工具契约、会话隔离、控制租约与宿主请求协议。
 *  3. 仅在 Windows 以 `--browser-url` 把官方 chrome-devtools-mcp 指向应用持有的
 *     WebView2 CDP 端口，并关闭其遥测、更新检查和 CrUX 上报。
 *
 * 协议约束：MCP 走 stdin/stdout（JSON-RPC over stdio，NDJSON 行分帧），本包装
 * 往 stdout 只写协议消息；日志一律走 stderr。
 *
 * 用法：
 *   node browser-wrapper.mjs <chrome-devtools-mcp-bin|@pinvou/browser-core> <host-state-json> [extra-args...]
 *
 * 退出：wrapper 自身生命周期即 MCP server 生命周期；Windows 启动后还负责托管
 * chrome-devtools-mcp 子进程，BrowserCore 平台不创建额外 MCP 子进程。
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
  browserHostBackendPolicy,
  browserToolMayMutate,
  buildBijectivePageTokenMaps,
  createHostCallerHeartbeat,
  createHostCancellationTombstone,
  createHostRequestEnvelope,
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
// 参数：node browser-wrapper.mjs <mcp-bin> <cdp-port-json> [extra...]
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

// Tauri 在 Windows 开发/安装目录上可能产出 `\\?\C:\...`。Node 的 fs API 能
// 处理不少 verbatim 路径，但把它作为另一个 Node 进程的入口脚本参数时会误解析
// 盘符并报 EISDIR。wrapper 自身再做一层兼容，旧会话配置也能在重启后恢复。
function nodeCompatibleEntryPath(value) {
  if (process.platform !== 'win32') return value;
  if (value.startsWith('\\\\?\\UNC\\')) return `\\\\${value.slice(8)}`;
  if (value.startsWith('\\\\?\\')) return value.slice(4);
  return value;
}

const MCP_BIN = nodeCompatibleEntryPath(MCP_BIN_ARG);

// chrome-devtools-mcp 的运行时要求（上游 package.json engines）：
// ^20.19.0 || ^22.12.0 || >=23。系统 node 过旧时 shim 仍能应答握手/工具目录
// （构建期捕获的 catalog 文件），但首个真实请求会失败并给出可读原因。
function nodeTooOld() {
  const [major, minor] = process.versions.node.split('.').map(Number);
  return !(major >= 23 || (major === 22 && minor >= 12) || (major === 20 && minor >= 19));
}

// ---------------------------------------------------------------------------
// CDP 存活探测（GET /json/version）。异步执行子进程并在重试间让出事件循环，
// 避免同步忙循环压住 MCP 握手、超时与宿主请求等启动期异步逻辑。
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
// 端口文件只读：只有应用宿主可发布 `{ port, owner: "app" }`。
// ---------------------------------------------------------------------------
function readPortFile() {
  try {
    const data = JSON.parse(readFileSync(CDP_PORT_JSON, 'utf8'));
    if (typeof data.port === 'number' && data.port > 0 && data.port < 65536) return data;
  } catch {
    /* 无文件/坏 json */
  }
  return null;
}

// 最近一次启动失败记录（{ reason, at }）：Rust 侧（browser_unavailability_reason）
// 在下次会话把原因注入模型可见的 instructions，让模型能精确引导用户修复。
// 成功启动（CDP 就绪）时清除。
const LAST_ERROR_JSON = join(dirname(CDP_PORT_JSON), 'last-error.json');
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
    throw new Error('浏览器宿主调用方心跳缺少有效会话身份');
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
      log('刷新浏览器宿主调用方心跳失败:', error?.message || error);
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
    try { unlinkSync(responsePath); } catch { /* 尚无晚响应 */ }
    // 取消 tombstone 是宿主提交边界，不能由调用方按 TTL 撤销。宿主消费并
    // 删除 tombstone 后才结束晚响应隔离；若宿主崩溃，它会留到下次启动屏障清理。
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
  // 先提交 tombstone 再删除请求。配套宿主必须在执行操作和写响应前检查它，并按
  // idempotency_key 去重；当前 wrapper 同时隔离任何晚响应，绝不让它进入后续请求。
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
    log('写入浏览器宿主取消 tombstone 失败:', error.message);
  }
  try { unlinkSync(requestPath); } catch { /* 宿主可能已经领取 */ }
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
    throw new Error('当前不是受管 WebView2 浏览器会话');
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
        new Error('浏览器工具调用已取消'),
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
        try { unlinkSync(requestPath); } catch { /* 宿主通常已经删除 */ }
        try { unlinkSync(tombstonePath); } catch { /* 不应存在 */ }
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
 * Windows 主应用负责创建真实嵌入的 WebView2。wrapper 只发按需请求并等待统一
 * CDP 端口；创建失败时明确报错，避免旧截图模式或第二个浏览器窗口混入界面。
 */
async function requestHostedBrowser() {
  if (process.platform !== 'win32') return 0;
  if (!SESSION_ID || !/^[0-9a-f]{16}$/.test(SESSION_TOKEN)) {
    writeLastError('浏览器会话身份缺失，无法创建对话级 WebView2');
    return 0;
  }
  try {
    await requestHost('prepare', { pid: process.pid }, 25_000, false);
  } catch (error) {
    log('WebView2 按需启动请求失败:', error.message);
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
    /* 工作区可能正在原子替换或尚未创建，调用方按未就绪处理 */
    return null;
  }
  // 只接受宿主发布的完整 v2 权威映射。旧 v1 页面 marker 没有 target 身份，若在
  // 已导航页面或 MCP 重启后继续使用只能按 URL/顺序猜测，必须明确失败而非回落。
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

function writeLastError(reason) {
  try {
    mkdirSync(dirname(LAST_ERROR_JSON), { recursive: true });
    // at 用 **秒**（与 Rust 侧 `browser_unavailability_reason` 的
    // `duration_since(UNIX_EPOCH).as_secs()` 同单位）：若写毫秒（Date.now()），
    // Rust 侧 `now.saturating_sub(at)` 恒为 0，「24h 内新鲜才注入」门禁成死代码，
    // 过期失败原因会无限期注入。
    writeFileSync(LAST_ERROR_JSON, JSON.stringify({ reason, at: Math.floor(Date.now() / 1000) }));
  } catch {
    /* 写失败不影响主流程 */
  }
}
function clearLastError() {
  try {
    unlinkSync(LAST_ERROR_JSON);
  } catch {
    /* 不存在就算了 */
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
// 原生浏览器宿主协调。成功返回应用拥有的 CDP 端口；失败抛出带稳定错误码的
// 可读错误并留在 shim 态。这里不得读取、启动或复用外部 Chrome。
// ---------------------------------------------------------------------------
async function ensureBrowserRunning() {
  const policy = browserHostBackendPolicy(process.platform);
  if (policy.action !== 'request-native-host') {
    hostedWebView2 = false;
    const reason = `${policy.code}: ${policy.message}；不会启动外部 Chrome`;
    writeLastError(reason);
    throw new Error(reason);
  }

  // Windows 每个任务对话都必须先向宿主登记并创建自己的子 WebView。不能仅因
  // cdp-port.json 必须指向当前应用持有的存活 CDP 端点，否则会接入应用外页面或其他身份。
  const hostedPort = await requestHostedBrowser();
  const portFile = readPortFile();
  if (hostedPort > 0 && isHostedWebView2Port(portFile)) {
    hostedWebView2 = true;
    clearLastError();
    return hostedPort;
  }

  hostedWebView2 = false;
  const reason = 'host-backend-unavailable: 应用内 WebView2 未就绪，请重新启动 PINVOU 后重试；不会启动外部 Chrome';
  writeLastError(reason);
  throw new Error(reason);
}

// ---------------------------------------------------------------------------
// MCP 目录（initialize / tools/list 应答来源）
//
// 构建期 vendor 脚本捕获 `catalog-shim.json`（与 MCP bin 同级的包根目录）：
// 官方 server 的工具目录是静态注册、无需浏览器连接，因此可以离线捕获并在 shim
// 阶段原样应答。文件缺失（开发环境直接指向自编译 bin 等）时运行时探测一次
// （不启动 Chrome：上游仅在 tools/call 时才经 getContext() 连接浏览器）。
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

// Windows 原生模式保留完整目录；new/list/select/close 由下方工作区路由接管，
// 每个页面级工具自动补当前对话的 pageId，不让 Agent 接触其他对话的 target。
function loadCatalogFile() {
  try {
    const data = JSON.parse(readFileSync(CATALOG_JSON, 'utf8'));
    if (validCatalog(data)) return data;
    log('catalog-shim.json 形状不符，回退运行时探测');
  } catch {
    /* 无文件/坏 json → 运行时探测 */
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
    let buf = '';
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
      buf += chunk;
      let idx;
      while ((idx = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, idx);
        buf = buf.slice(idx + 1);
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
// stdio shim / 透明代理状态机
// ---------------------------------------------------------------------------
// shim     ：wrapper 直接应答 initialize/ping/tools/list（不准备浏览器宿主）；
//            其余一切请求触发启动。
// starting ：原生浏览器宿主 + MCP 子进程准备中，到达的请求行缓冲、取消通知登记；
//            启动失败 → 缓冲请求统一报错，回到 shim（可重试）。
// proxy    ：双向透传（stdin 行 → 子进程 stdin；子进程 stdout → stdout）。
let state = 'shim';
let startPromise = null;
let mcpChild = null;
let clientInitializeParams = null;
let bufferedLines = [];
const cancelledIds = new Set();
const hostCoreRequestIds = new Set();
let hostCorePrepared = false;
let hostCoreQueue = Promise.resolve();
let shuttingDown = false;
let shutdownPromise = null;
const WRAPPER_SHUTTING_DOWN_ERROR =
  'browser/wrapper-shutting-down: browser wrapper is shutting down';

function writeOut(msg) {
  process.stdout.write(JSON.stringify(msg) + '\n');
}

function respondError(id, message) {
  writeOut({ jsonrpc: '2.0', id, error: { code: -32000, message } });
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
    throw new Error('浏览器工具调用已取消');
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
      if (!cancelledIds.delete(message.id)) {
        writeOut({ jsonrpc: '2.0', id: message.id, result });
      }
    } catch (error) {
      const reason = error?.message || String(error);
      writeLastError(reason);
      if (!cancelledIds.delete(message.id)) respondError(message.id, reason);
    } finally {
      hostCoreRequestIds.delete(message.id);
      cancelledIds.delete(message.id);
    }
  });
}

function handleShimRequest(msg, raw) {
  // 目录不可得（catalog 文件缺失且运行时探测失败）：握手/目录如实报错，
  // 进程保持 shim 态存活，引擎下轮重连可恢复。
  if (!catalog && (msg.method === 'initialize' || msg.method === 'tools/list')) {
    respondError(msg.id, 'browser MCP 工具目录不可用（catalog-shim.json 缺失且探测失败）');
    return;
  }
  switch (msg.method) {
    case 'initialize': {
      clientInitializeParams = msg.params ?? null;
      // protocolVersion 回显客户端请求值（上游 SDK 同款协商行为；实测
      // chrome-devtools-mcp 对 2024-11-05 请求应答 2024-11-05）。
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
    /* 坏行丢弃（协议对端是引擎，正常不会发生） */
  }
  if (shuttingDown) {
    if (msg?.id != null) respondError(msg.id, WRAPPER_SHUTTING_DOWN_ERROR);
    return;
  }
  try {
    // 必须在触发宿主准备、进入启动缓冲或透传给上游前 fail-closed。
    // 目录隐藏只是可发现性约束，不能代替直接 tools/call 的执行边界。
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
    // 启动期：取消通知登记（flush 时跳过该请求），其余请求缓冲，通知丢弃。
    if (msg && msg.method === 'notifications/cancelled' && msg.params?.requestId != null) {
      cancelledIds.add(msg.params.requestId);
    } else if (msg && msg.id != null) {
      bufferedLines.push(line);
    }
    return;
  }
  // shim 态
  if (!msg) return;
  if (msg.id == null) return; // initialized 等通知：无需处理
  handleShimRequest(msg, line);
}

function triggerStart(raw) {
  bufferedLines.push(raw);
  if (startPromise) return;
  state = 'starting';
  startPromise = startProxy();
}

async function startProxy() {
  let port = 0;
  let startupChild = null;
  try {
    if (nodeTooOld()) {
      const reason = `Node.js 版本过低（当前 ${process.versions.node}，chrome-devtools-mcp 要求 ^20.19 || ^22.12 || >=23）`;
      writeLastError(reason);
      throw new Error(reason);
    }
    port = await ensureBrowserRunning();
    // WebView2 的 /json/version 可能先于 DevTools WebSocket 完全就绪。官方 MCP
    // 在这个很窄的启动窗口里会直接以 code=1 退出；把它当成瞬态启动失败，先确认
    // CDP 仍存活再短退避重试，避免首个真实浏览器调用稳定失败、第二次却已就绪。
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
    log('浏览器启动失败:', reason);
    writeLastError(reason);
    const failed = uncancelledBufferedRequests(bufferedLines, cancelledIds);
    bufferedLines = [];
    state = 'shim';
    startPromise = null;
    for (const raw of failed) {
      try {
        const m = JSON.parse(raw);
        if (m.id != null) respondError(m.id, `浏览器不可用: ${reason}`);
      } catch {
        /* ignore */
      }
    }
    cancelledIds.clear();
    return;
  }
  state = 'proxy';
  startHostedBrowserWatchdog(port);
  const pending = uncancelledBufferedRequests(bufferedLines, cancelledIds);
  bufferedLines = [];
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
    /* 子进程已死由 exit 处理器兜底 */
  }
}

let proxyQueue = Promise.resolve();
let proxyChildBuffer = '';
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
    log('受管浏览器工具在合作取消后仍未终止，关闭上游以收口原子操作:', reason);
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
  // 连接建立后的目录刷新也必须使用同一份适配目录；否则 shim 阶段隐藏的
  // take_screenshot/upload_file 会在上游 tools/list 刷新后重新暴露给模型。
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
        msg.params?.reason || '浏览器工具调用已取消',
      );
      return;
    }
    // 非受管工具仍以外部 id 原样透传，不能把它误映射到另一条内部调用。
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
          log('受管标签页路由失败:', error?.message || error);
          respondError(msg.id, error?.message || String(error));
        }
      } finally {
        managedToolRequestIds.delete(msg.id);
        cancelledProxyRequestIds.delete(msg.id);
        externalToInternalRequestIds.delete(msg.id);
      }
    });
}

function onProxyChildData(chunk) {
  proxyChildBuffer += chunk;
  let idx;
  while ((idx = proxyChildBuffer.indexOf('\n')) >= 0) {
    const line = proxyChildBuffer.slice(0, idx);
    proxyChildBuffer = proxyChildBuffer.slice(idx + 1);
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      process.stdout.write(line + '\n');
      continue;
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
        const upstreamError = msg.error.message || 'chrome-devtools-mcp 内部调用失败';
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
        pending.reject(new Error(message || 'chrome-devtools-mcp 内部工具调用失败'));
      } else {
        pending.resolve(msg.result);
      }
      continue;
    }
    // 已取消或超时的内部调用仍可能收到上游晚响应。内部 id 是 wrapper 的保留
    // 命名空间，绝不能把晚响应当作外部 JSON-RPC 应答泄漏给引擎。
    if (discardedInternalRequestIds.delete(msg.id)) {
      continue;
    }
    process.stdout.write(line + '\n');
  }
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
        const reason = `chrome-devtools-mcp 内部调用 ${method} 超时`;
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
      reject(new Error(`chrome-devtools-mcp 内部调用 ${method} 超时`));
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
    throw new Error('浏览器工具调用已取消');
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
    throw new Error('页面发现缺少宿主 v2 权威 target 映射');
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
        throw new Error('MCP pageId 与宿主权威 target 映射发生冲突');
      }
      next.push([page.id, authoritativeToken]);
      continue;
    }

    // A non-empty structured target is authoritative. If it does not occur in
    // the host workspace, neither an old pageId binding nor a page-controlled
    // URL fragment may turn it into an owned target.
    if (page.targetId) continue;

    // 官方 chrome-devtools-mcp@1.7.0 暂未在 list_pages 输出 raw targetId。
    // 同一 MCP 进程内仅保留已经由权威 target 或受控 about:blank marker 建立的
    // pageId 绑定；wrapper/MCP 重启后既无 targetId 又无 marker 时会明确失败，
    // 不按 URL/标题猜测，也不读取远程页面主世界。vendor adapter 接通 targetId
    // 后该分支只承担首次 about:blank bootstrap。
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
  if (!Number.isInteger(pageId)) throw new Error('浏览器标签页尚未绑定到 MCP');
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
    throw new Error('browser/workspace-missing: 当前对话的浏览器工作区状态尚未就绪');
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
    throw new Error('未找到当前对话激活标签对应的 WebView2 页面');
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
    throw new Error('browser/workspace-unavailable: 应用内 WebView2 工作区重新准备失败');
  }
  hostedWebView2 = true;
  clearLastError();
  throwIfProxyRequestCancelled(externalRequestId);
}

/**
 * browser_stop 只销毁当前对话工作区；其他对话存活时，共享 CDP 端口仍然健康，
 * 因此长驻 Windows wrapper 不能把“CDP 存活”当成“本会话工作区存活”。仅在工具
 * 发生任何 lease/宿主操作/上游页面动作之前，对明确的工作区生命周期错误做一次
 * reset → prepare → sync 重试。重试仍失败时保持未准备状态，但本次调用绝不进入第三次。
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
    throw new Error('宿主当前可见标签与 Agent 目标页不一致');
  }

  const listResult = await callUpstreamTool('list_pages', {}, 15_000, externalRequestId);
  const workspace = readWorkspaceState();
  if (
    !workspace ||
    workspace.active_tab !== tabToken ||
    !(workspace.tabs || []).some((tab) => tab?.token === tabToken)
  ) {
    throw new Error('宿主当前可见标签在 Agent 选择 Target 后发生变化');
  }
  await discoverWorkspacePages(listResult, workspace, externalRequestId);
  workspaceRevision = workspace.revision;
  const selected = parseBrowserPages(listResult).find((page) => page.id === pageId);
  if (pageIdToTabToken.get(pageId) !== tabToken || selected?.selected !== true) {
    throw new Error('Agent 当前 Target 与宿主可见标签未能对齐');
  }
  selectedPageId = pageId;
  return { workspace, listResult };
}

async function runOnVisibleHostedPage(
  msg,
  pageId,
  execute = null,
  { emitsTrustedInput = false } = {},
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
        throw new Error('目标标签缺少宿主权威 target 映射');
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
              throw new Error('宿主当前可见标签在工具执行前发生变化');
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
  // 必须在任何上游透传/页面对齐之前完成 URL 校验。旧客户端会省略 type 只传
  // url；assertAllowedHostedNavigation 会把这种调用按 type=url 处理。
  if (name === 'navigate_page') assertAllowedHostedNavigation(args);
  if (name === 'list_pages') {
    const { listResult } = await syncWorkspacePagesBeforeDispatch(true, msg.id);
    return filteredPagesResult(listResult);
  }
  if (name === 'select_page') {
    // 显式 pageId 必须在任何 list/select/host 副作用前先通过当前对话归属校验。
    const selectedPage = explicitOwnedPageId(args, pageIdToTabToken);
    if (selectedPage == null) {
      throw new Error('标签页不存在或不属于当前对话');
    }
    const selectedTabToken = pageIdToTabToken.get(selectedPage);
    await syncWorkspacePagesBeforeDispatch(false, msg.id);
    if (pageIdToTabToken.get(selectedPage) !== selectedTabToken) {
      throw new Error('页面归属在同步过程中发生变化');
    }
    const aligned = await runOnVisibleHostedPage(msg, selectedPage);
    return filterPagesResult(
      aligned.verificationResult.listResult,
      new Set(pageIdToTabToken.keys()),
      selectedPageId,
    );
  }
  if (name === 'new_page') {
    if (typeof args.url !== 'string') throw new Error('new_page 缺少 url');
    assertAllowedHostedNavigation({ url: args.url });
    if (args.isolatedContext) {
      throw new Error('应用内浏览器暂不支持 isolatedContext；标签页默认共享当前登录状态');
    }
    // new_page needs a fresh list result so the one-time bootstrap blank can
    // be distinguished from a real page without guessing from title/history.
    const previous = await syncWorkspacePagesBeforeDispatch(true, msg.id);
    const previousActiveToken = previous.workspace.active_tab;
    const previousPageId = pageIdForToken(previousActiveToken);
    if (previousPageId == null) {
      throw new Error('新建标签页前无法取得当前宿主页面授权');
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
    // create_tab 是宿主状态 mutation：先在当前 active 页取得 CAS lease，宿主会
    // 原子复核 authorization_tab_token/target/revision/lease 后才创建新标签。
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
        throw new Error('create_tab 返回了不匹配的 creationId');
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
            throw new Error('create_tab 返回的 targetId 与宿主权威映射不一致');
          }
          await discoverWorkspacePages(listResult, workspace, msg.id);
        }
        page = findHostedTabPage(listResult, tabToken, pageIdToTabToken);
        if (!page) await sleep(50);
      }
      if (!page) throw new Error('新建的内嵌标签页未出现在 MCP 页面列表中');
      if (
        pageIdToTabToken.get(page.id) !== tabToken ||
        tabTokenToPageId.get(tabToken) !== page.id
      ) {
        throw new Error('新建标签页未形成唯一的 pageId ↔ tabToken 映射');
      }
      // URL 首航由宿主在未发布的 staging 标签内完成，并与 create CAS 一起提交。
      // create 成功后 lease 已失效；wrapper 若再直接导航这个 target，会让后台标签
      // 绕过用户接管，因此这里只发现权威 target，随后按宿主 active 状态同步选择。
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
        // create 响应校验、target 发现或导航失败都只能按 creation_id 精确补偿；
        // 普通 close_tab 会误关被并发替换/接管的同 token 标签，禁止用于回滚。
        if (previousPageId != null) {
          try { await selectUpstreamPage(previousPageId); } catch { /* 尽力恢复 */ }
        }
        try {
          await requestHostedOperation('rollback_created_tab', {
            tab_token: tabToken,
            creation_id: creationId,
          });
          rollbackProved = true;
        } catch (rollbackError) {
          log('回滚创建失败的内嵌标签页失败:', rollbackError.message);
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
    // 与普通页面工具保持同一 fail-closed 顺序：跨对话 pageId 不能先触发同步选择。
    const closingPageId = explicitOwnedPageId(args, pageIdToTabToken);
    if (closingPageId == null) {
      throw new Error('标签页不存在或不属于当前对话');
    }
    const closingTabToken = pageIdToTabToken.get(closingPageId);
    const { workspace, listResult } = await syncWorkspacePagesBeforeDispatch(true, msg.id);
    if (pageIdToTabToken.get(closingPageId) !== closingTabToken) {
      throw new Error('页面归属在同步过程中发生变化');
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
      throw new Error('关闭标签页前无法找到可用的回退页面');
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
          // chrome-devtools-mcp 的 list_pages 响应也会读取“当前选中页”。若先销毁
          // 选中 WebView，后续 list_pages 会因 selected page is closed 无法恢复映射。
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

  // 所有普通页面工具执行前都与用户当前激活标签对齐；因此用户点击标签后，Agent
  // 的下一次页面读取、点击、输入或脚本操作会自然落在同一个页面。
  if (!runtimePageScopedTools.has(name)) {
    // 这类调用保留外部 JSON-RPC id，由上游直接应答；后续取消通知也应原样透传。
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
    throw new Error('页面归属在同步过程中发生变化');
  }
  const pageId = requestedPageId ?? pageIdForToken(workspace.active_tab);
  if (!Number.isInteger(pageId) || !pageIdToTabToken.has(pageId)) {
    throw new Error('页面不存在或不属于当前对话');
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
    { emitsTrustedInput: runtimeInputTools.has(name) },
  );
  return aligned.executionResult;
}

// 与 MCP 子进程的握手 id：字符串形式，与引擎的数字 id 不会冲突。
const HANDSHAKE_ID = 'pinvou-wrapper-handshake';
const SESSION_LIST_ID_PREFIX = 'pinvou-wrapper-list-';
const SESSION_SELECT_ID = 'pinvou-wrapper-select';

function retryableMcpStartError(error) {
  const message = error?.message || String(error);
  return /chrome-devtools-mcp 握手前退出 code=(?:1|null)/.test(message);
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
      log(`chrome-devtools-mcp 启动过早，${delayMs}ms 后重试 (${attempt}/${maxAttempts})`);
      await sleep(delayMs);
      if (!(await probeCdp(port, 2_000))) {
        throw new Error('WebView2 CDP 在 MCP 重试前失去响应');
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
    // 会话隔离依赖 vendor adapter 写入 structuredContent.pages[].target_id。
    // 官方 server 只有显式开启 structured content 才会把该字段放进 tools/call
    // 响应；仅开启 page-id routing 时虽然能看到页面，wrapper 却无法按宿主
    // targetId 建立权威映射，最终会把已成功显示的新标签误报为创建失败。
    ...(hostedWebView2
      ? ['--experimental-page-id-routing', '--experimental-structured-content']
      : []),
    ...EXTRA_ARGS,
  ];
  log('启动 chrome-devtools-mcp:', process.execPath, mcpArgs.join(' '));
  return new Promise((resolve, reject) => {
    let child;
    try {
      child = spawn(process.execPath, mcpArgs, {
        stdio: ['pipe', 'pipe', 'inherit'], // stderr 日志透传
        env: {
          ...process.env,
          CHROME_DEVTOOLS_MCP_NO_UPDATE_CHECKS: '1', // 离线：禁用更新检查
          CI: '1', // 离线：禁用 usage statistics
        },
      });
    } catch (e) {
      reject(e);
      return;
    }
    mcpChild = child;
    let stdoutBuf = '';
    const pendingOutput = [];
    let settled = false;
    let startupFailureCleanup = false;
    let activeProxyChild = false;
    let listAttempt = 0;
    const timer = setTimeout(() => {
      if (!settled) {
        fail(new Error('chrome-devtools-mcp 握手或会话页面绑定超时'));
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
          throw new Error('chrome-devtools-mcp 在代理接管前退出');
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
      if (pendingOutput.length) process.stdout.write(pendingOutput.join(''));
      // 统一由多路复用器接管 stdout：内部 list/select/evaluate 响应不能泄漏给引擎。
      proxyChildBuffer = stdoutBuf;
      stdoutBuf = '';
      child.stdout.on('data', onProxyChildData);
      resolve(childLifecycle);
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
      stdoutBuf += chunk;
      let idx;
      while ((idx = stdoutBuf.indexOf('\n')) >= 0) {
        const line = stdoutBuf.slice(0, idx);
        stdoutBuf = stdoutBuf.slice(idx + 1);
        let msg;
        try {
          msg = JSON.parse(line);
        } catch {
          pendingOutput.push(line + '\n');
          continue;
        }

        if (msg.id === HANDSHAKE_ID) {
          if (msg.error) {
            fail(new Error(`chrome-devtools-mcp 握手失败: ${msg.error.message}`));
            return;
          }
          writeChildRaw(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }));
          if (hostedWebView2 && SESSION_TOKEN) {
            requestSessionPage();
          } else {
            finish();
          }
          return;
        }

        if (typeof msg.id === 'string' && msg.id.startsWith(SESSION_LIST_ID_PREFIX)) {
          if (msg.error) {
            fail(new Error(`枚举对话浏览器页面失败: ${msg.error.message}`));
            return;
          }
          let page = null;
          try {
            const workspace = readWorkspaceState();
            page = workspace
              ? findHostedWorkspacePage(msg.result, workspace, SESSION_TOKEN)
              : null;
          } catch (error) {
            fail(new Error(`绑定宿主权威页面失败: ${error?.message || error}`));
            return;
          }
          if (!page) {
            if (listAttempt >= 80) {
              fail(new Error('未找到宿主当前激活标签对应的 WebView2 页面'));
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
            fail(new Error(`绑定对话浏览器页面失败: ${msg.error.message}`));
          } else {
            log('已绑定对话浏览器页面:', SESSION_TOKEN);
            finish();
          }
          return;
        }

        // 初始化阶段收到的其他通知/响应在绑定完成后原样转发给引擎。
        pendingOutput.push(line + '\n');
      }
    };
    child.stdout.on('data', onData);
    child.on('error', (err) => {
      log('chrome-devtools-mcp 启动失败:', err.message);
      if (mcpChild === child) mcpChild = null;
      if (!settled) {
        fail(err);
      }
    });
    child.on('exit', (code, signal) => {
      log('chrome-devtools-mcp 退出', { code, signal });
      if (mcpChild === child) mcpChild = null;
      // `fail()` deliberately kills an attempt that never became the active
      // proxy. Its eventual exit belongs only to startup cleanup: treating it
      // as a runtime child crash would shut down the reusable shim and can
      // also clobber a newer retry's `mcpChild` handle.
      if (startupFailureCleanup) return;
      if (!settled) {
        fail(new Error(`chrome-devtools-mcp 握手前退出 code=${code}`));
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
    // 与引擎一致的 initialize 参数（含 protocolVersion 协商与 clientInfo）。
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
// 子进程回收（SIGTERM → 3s 宽限 → SIGKILL 升级）。浏览器表面由应用宿主拥有，
// wrapper 只负责回收自己启动的 MCP adapter。
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
    for (const raw of bufferedLines) {
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
    // Let the final JSON-RPC diagnostic/end host request flush before forcing
    // process termination; watchdog/signal timers otherwise keep Node alive.
    await new Promise((resolve) => setImmediate(resolve));
    process.exit(exitCode);
  })();
  return shutdownPromise;
}

// ---------------------------------------------------------------------------
// 应用宿主拥有浏览器运行时，wrapper 没有其进程退出事件。宿主关闭或崩溃后，
// 本会话的 --browser-url 会永久失效；周期探测并在连续失败后退出，让引擎下次
// 拉起 wrapper 时重新向原生宿主申请工作区。
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
          log('应用内浏览器 CDP 已失联，退出以待重新协调原生宿主');
          void gracefulShutdown(1, '应用内浏览器 CDP 已失联');
        }
      })
      .catch((error) => {
        log('应用内浏览器 CDP 探测失败:', error?.message || error);
      })
      .finally(() => {
        probing = false;
      });
  }, 10000);
  timer.unref();
}

// ---------------------------------------------------------------------------
// 主流程：加载目录 → shim 待命（首个真实请求才申请原生宿主并启动 MCP）
// ---------------------------------------------------------------------------
let stdinBuf = '';

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
      log('catalog-shim.json 缺失，运行时探测工具目录（不启动浏览器）…');
      catalog = await probeCatalog();
    }
  }
  catalog = adaptBrowserCatalog(catalog);
  if (!catalog) {
    // 目录不可得：tools/list 直接报错（引擎记录失败状态），进程保持存活。
    // 不退出——懒启动语义下 connect 阶段的退出会让引擎每次重连都刷失败噪音。
    log('工具目录不可用（catalog 文件缺失且运行时探测失败）');
    catalog = null;
  }

  process.stdin.on('data', (chunk) => {
    stdinBuf += chunk;
    let idx;
    while ((idx = stdinBuf.indexOf('\n')) >= 0) {
      const line = stdinBuf.slice(0, idx);
      stdinBuf = stdinBuf.slice(idx + 1);
      if (line.trim()) handleLine(line);
    }
  });
  // 引擎关闭 stdin（断开/会话结束）：无论处于哪一态都退出并回收子进程。
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

main().catch((e) => {
  console.error('[browser-wrapper] 致命错误:', e);
  void gracefulShutdown(1, `browser wrapper fatal error: ${e?.message || e}`, {
    cancelAcceptedRequests: true,
  });
});
