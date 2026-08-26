/**
 * Windows 原生多标签协议的纯函数部分。wrapper 只向当前对话暴露其工作区页面，
 * 但保留 chrome-devtools-mcp 原生 pageId，避免重写所有页面级工具。
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

const RECOVERABLE_HOST_CORE_WORKSPACE_ERROR_CODES = new Set([
  'browser/workspace-unavailable',
  'browser/workspace-missing',
  'browser/workspace-stopped',
]);

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
      // CodeWhale 当前把 MCP image content 序列化进纯文本 ToolResult，模型只会
      // 收到一段 base64，而不会收到可视觉理解的图像块。内嵌浏览器本身已经实时
      // 给用户显示页面；继续向 Agent 暴露截图只会浪费上下文并诱发“截图成功 =
      // 已完成视觉确认”的错误结论。视觉结果管线真正接通前先从目录隐藏。
      // catalog-shim 在浏览器启动前捕获，未携带运行期 experimental-page-id-routing
      // 开关；这里镜像上游 1.7.0 的 schema 变换，让 Agent 真正看得到显式 pageId。
      tools: tools
        .filter((tool) => !isDisabledBrowserToolName(tool?.name))
        .map(exposePageIdRouting),
    },
  };
}

// 兼容旧调用方；最终多标签模式不再把 new_page/close_page 改写为 navigate_page。
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
    message: `当前平台尚无原生浏览器自动化后端 (${platform || 'unknown'})`,
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
    throw new Error('应用内浏览器仅支持 http/https/about:blank 协议');
  }
  return effectiveType;
}

/**
 * 启动阶段的请求与取消通知可能在同一个 stdin chunk 内到达。失败应答前必须基于
 * 取消集合做快照过滤，不能先 clear 再遍历，否则已取消请求会收到伪造的启动错误。
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
    throw new Error('浏览器宿主请求的 sessionToken 无效');
  }
  if (!HOST_REQUEST_ID_PATTERN.test(requestId || '')) {
    throw new Error('浏览器宿主请求的 requestId 无效');
  }
}

function assertWrapperInstanceNonce(wrapperInstanceNonce) {
  if (!WRAPPER_INSTANCE_NONCE_PATTERN.test(wrapperInstanceNonce || '')) {
    throw new Error('浏览器宿主请求的 wrapperInstanceNonce 无效');
  }
}

function assertHostCallerIdentity(callerPid, wrapperInstanceNonce) {
  if (!Number.isSafeInteger(callerPid) || callerPid <= 0 || callerPid > 0xffff_ffff) {
    throw new Error('浏览器宿主请求的 callerPid 无效');
  }
  assertWrapperInstanceNonce(wrapperInstanceNonce);
}

export function hostCallerHeartbeatArtifactName(sessionToken, wrapperInstanceNonce) {
  if (!TAB_TOKEN_PATTERN.test(sessionToken || '')) {
    throw new Error('浏览器宿主调用方心跳的 sessionToken 无效');
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
    throw new Error('浏览器宿主调用方心跳的 sessionId 无效');
  }
  if (!TAB_TOKEN_PATTERN.test(sessionToken || '')) {
    throw new Error('浏览器宿主调用方心跳的 sessionToken 无效');
  }
  assertHostCallerIdentity(callerPid, wrapperInstanceNonce);
  if (!Number.isSafeInteger(heartbeatAt) || heartbeatAt <= 0) {
    throw new Error('浏览器宿主调用方心跳时间无效');
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
    throw new Error('浏览器宿主请求的 sessionId 无效');
  }
  if (typeof operation !== 'string' || !operation) {
    throw new Error('浏览器宿主请求的 operation 无效');
  }
  if (!payload || typeof payload !== 'object' || Array.isArray(payload)) {
    throw new Error('浏览器宿主请求 payload 必须是对象');
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
    throw new Error('浏览器宿主取消记录的 sessionId 无效');
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
 * pageId 与宿主 tabToken 必须是一一对应关系。输入保留为 entries，而不是先转
 * Map，确保重复 pageId 不会在校验前被 Map 的覆盖语义吞掉。
 */
export function buildBijectivePageTokenMaps(entries) {
  if (!entries || typeof entries[Symbol.iterator] !== 'function') {
    throw new Error('页面映射必须是可迭代的 [pageId, tabToken] 列表');
  }
  const pageToToken = new Map();
  const tokenToPage = new Map();
  for (const entry of entries) {
    if (!Array.isArray(entry) || entry.length < 2) {
      throw new Error('页面映射项格式无效');
    }
    const [pageId, tabToken] = entry;
    if (!Number.isInteger(pageId) || !TAB_TOKEN_PATTERN.test(tabToken || '')) {
      throw new Error('页面映射项包含无效 pageId 或 tabToken');
    }
    if (pageToToken.has(pageId)) {
      throw new Error(`页面映射包含重复 pageId: ${pageId}`);
    }
    if (tokenToPage.has(tabToken)) {
      throw new Error(`页面映射包含重复 tabToken: ${tabToken}`);
    }
    pageToToken.set(pageId, tabToken);
    tokenToPage.set(tabToken, pageId);
  }
  return { pageToToken, tokenToPage };
}

export function assertBijectivePageTokenMap(pageTokens) {
  if (!(pageTokens instanceof Map)) throw new Error('页面映射必须是 Map');
  return buildBijectivePageTokenMaps(pageTokens.entries());
}

/**
 * 返回调用方显式提供且属于当前对话的 pageId；未提供时返回 null。属性存在但值
 * 非整数时必须拒绝，不能把畸形/跨对话 pageId 静默降级成“使用当前页”。
 */
export function explicitOwnedPageId(args, pageTokens) {
  if (!Object.prototype.hasOwnProperty.call(args || {}, 'pageId')) return null;
  assertBijectivePageTokenMap(pageTokens);
  const pageId = args.pageId;
  if (!Number.isInteger(pageId) || !pageTokens.has(pageId)) {
    throw new Error('页面不存在或不属于当前对话');
  }
  return pageId;
}

/**
 * 宿主权威 target 映射只接受显式 v2 schema。不得把 v1 的页面脚本 marker、URL
 * fragment 或残缺 v2 状态静默解释成权威映射。
 */
export function parseAuthoritativeHostWorkspace(value, expectedSessionToken = null) {
  if (value?.version !== 2 || value.mapping_authority !== 'host') {
    throw new Error('宿主工作区未提供 v2 权威 target 映射');
  }
  if (!TAB_TOKEN_PATTERN.test(value.session_token || '')) {
    throw new Error('宿主工作区 session_token 无效');
  }
  if (expectedSessionToken != null && value.session_token !== expectedSessionToken) {
    throw new Error('宿主工作区 session_token 不匹配');
  }
  if (!Number.isInteger(value.revision) || value.revision < 0) {
    throw new Error('宿主工作区 revision 无效');
  }
  if (!TAB_TOKEN_PATTERN.test(value.active_tab || '') || !Array.isArray(value.tabs)) {
    throw new Error('宿主工作区 active_tab 或 tabs 无效');
  }

  const tokens = new Set();
  const targets = new Set();
  const tabs = value.tabs.map((tab) => {
    const token = tab?.token;
    const targetId = tab?.target_id;
    if (!TAB_TOKEN_PATTERN.test(token || '')) {
      throw new Error('宿主权威 target 映射包含无效 tabToken');
    }
    if (typeof targetId !== 'string' || !targetId.trim()) {
      throw new Error(`宿主标签 ${token} 缺少权威 target_id`);
    }
    if (tokens.has(token)) throw new Error(`宿主工作区包含重复 tabToken: ${token}`);
    if (targets.has(targetId)) throw new Error(`宿主工作区包含重复 target_id: ${targetId}`);
    tokens.add(token);
    targets.add(targetId);
    return { token, target_id: targetId };
  });
  if (!tokens.has(value.active_tab)) {
    throw new Error('宿主工作区 active_tab 不在权威 target 映射中');
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
    throw new Error('activate_tab 响应缺少有效 sessionId');
  }
  if (!TAB_TOKEN_PATTERN.test(tabToken || '')) {
    throw new Error('activate_tab 响应缺少有效 tabToken');
  }
  if (typeof targetId !== 'string' || !targetId.trim()) {
    throw new Error('activate_tab 响应缺少权威 targetId');
  }
  if (!Number.isInteger(value?.revision) || value.revision < 0) {
    throw new Error('activate_tab 响应缺少有效 revision');
  }
  if (!/^[0-9a-f]{32}$/.test(value?.lease || '')) {
    throw new Error('activate_tab 响应缺少 dispatch lease');
  }
  if (value?.owner !== 'agent') {
    throw new Error('activate_tab 响应未把控制权授予 Agent');
  }
  if (expected.sessionId != null && expected.sessionId !== sessionId) {
    throw new Error('activate_tab 响应的 sessionId 与请求不一致');
  }
  if (expected.tabToken != null && expected.tabToken !== tabToken) {
    throw new Error('activate_tab 响应的 tabToken 与请求不一致');
  }
  if (expected.targetId != null && expected.targetId !== targetId) {
    throw new Error('activate_tab 响应的 targetId 与宿主映射不一致');
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
 * create/close 是宿主状态 mutation，tab_token 可能表示“新标签”，不能与授权来源
 * 混用。authorization_tab_token 明确指出 lease 所属的当前/目标标签，其余 CAS
 * 字段保持与 NativeTabLease 一致。
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
  // v3 的 create result 是安全协议的一部分，不接受旧字段别名。否则一个只实现
  // 了一半的新宿主会被 wrapper 当成 CAS 已完成，后续补偿却无法可靠绑定创建代次。
  const resultTabToken = value?.tabToken;
  const targetId = value?.targetId;
  const resultCreationId = value?.creationId;
  if (!TAB_TOKEN_PATTERN.test(resultTabToken || '')) {
    throw new Error('create_tab 响应缺少有效 tabToken');
  }
  if (typeof targetId !== 'string' || !targetId.trim()) {
    throw new Error('create_tab 响应缺少有效 targetId');
  }
  if (!HOST_REQUEST_ID_PATTERN.test(resultCreationId || '')) {
    throw new Error('create_tab 响应缺少有效 creationId');
  }
  if (tabToken != null && resultTabToken !== tabToken) {
    throw new Error('create_tab 响应的 tabToken 与请求不一致');
  }
  if (creationId != null && resultCreationId !== creationId) {
    throw new Error('create_tab 响应的 creationId 与 request_id 不一致');
  }
  return {
    tabToken: resultTabToken,
    targetId,
    creationId: resultCreationId,
  };
}

/**
 * v3 response envelope 的身份字段是必填而非“存在时才验证”。否则过期/伪造的
 * 响应可能被另一个 CAS mutation 接受。create_tab 还强制 creationId=request_id。
 */
export function parseHostResponseEnvelope(value, {
  requestId,
  idempotencyKey,
  operation,
  requestedTabToken = null,
} = {}) {
  if (value?.protocol_version !== HOST_REQUEST_PROTOCOL_VERSION) {
    throw new Error('浏览器宿主响应缺少有效 protocol_version=3');
  }
  if (typeof requestId !== 'string' || value?.request_id !== requestId) {
    throw new Error('浏览器宿主响应缺少或返回了不匹配的 request_id');
  }
  if (typeof idempotencyKey !== 'string' || value?.idempotency_key !== idempotencyKey) {
    throw new Error('浏览器宿主响应缺少或返回了不匹配的 idempotency_key');
  }
  if (typeof value?.ok !== 'boolean') {
    throw new Error('浏览器宿主响应缺少布尔 ok 字段');
  }
  if (!value.ok) {
    throw new Error(value?.error || `浏览器宿主操作 ${operation || 'unknown'} 失败`);
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
 * 在受管浏览器中执行页面操作前，将 Agent 的目标页与用户可见标签原子对齐。
 *
 * pageTokens 是当前任务对话唯一允许的 pageId -> tabToken 映射。调用方负责把
 * activate/select/verify/execute 都放在同一条串行队列中；这里固定各阶段顺序，
 * 并在每个有副作用的阶段前重新确认归属及取消状态。任何一步失败时 execute
 * 都不会被调用，避免工具静默落到后台页或其他任务对话的页面。
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
      throw new Error('页面不存在或不属于当前对话');
    }
    // 每个阶段都重验双射，防止并发刷新映射时把一个 token 留给多个 pageId。
    assertBijectivePageTokenMap(pageTokens);
    const token = pageTokens.get(pageId);
    if (typeof token !== 'string' || !token) {
      throw new Error('页面不存在或不属于当前对话');
    }
    return token;
  };

  const tabToken = ownedTabToken();
  await ensureActive();
  const activationResult = await activateTab(tabToken);

  // 宿主切换期间用户可能关闭标签，不能依赖第一次检查的陈旧映射。
  await ensureActive();
  if (ownedTabToken() !== tabToken) throw new Error('页面归属在激活过程中发生变化');
  await assertLease({ pageId, tabToken, activationResult, phase: 'select' });
  const selectionResult = await selectPage(pageId);

  await ensureActive();
  if (ownedTabToken() !== tabToken) throw new Error('页面归属在选择过程中发生变化');
  await assertLease({ pageId, tabToken, activationResult, phase: 'verify' });
  const verificationResult = await verify({ pageId, tabToken, activationResult });

  await ensureActive();
  if (ownedTabToken() !== tabToken) throw new Error('页面归属在执行前发生变化');
  if (recordAlignment) {
    try {
      recordAlignment(Math.max(0, now() - alignmentStartedAt));
    } catch {
      // 性能诊断绝不能改变页面操作结果。
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
  if (matches.length > 1) throw new Error('MCP 页面列表包含重复 session marker');
  return matches[0] || null;
}

export function findHostedTabPage(result, tabToken, pageTokens = new Map()) {
  if (!/^[0-9a-f]{16}$/.test(tabToken || '')) return null;
  const marker = `about:blank#pinvou-tab-${tabToken}`;
  const matches = parseBrowserPages(result).filter(
    (page) => pageTokens.get(page.id) === tabToken ||
      (!page.targetId && page.url === marker),
  );
  if (matches.length > 1) throw new Error(`MCP 页面列表包含重复 tab marker: ${tabToken}`);
  return matches[0] || null;
}

/**
 * MCP 握手时也必须使用宿主权威 target join。仅新建 about:blank 尚未导航时允许
 * fragment bootstrap；已导航页面或 wrapper/MCP 重启后由 target_id 直接恢复。
 */
export function findHostedWorkspacePage(result, workspace, expectedSessionToken) {
  const state = parseAuthoritativeHostWorkspace(workspace, expectedSessionToken);
  const active = state.tabs.find((tab) => tab.token === state.active_tab);
  if (!active) throw new Error('宿主工作区缺少当前激活标签');
  const pages = parseBrowserPages(result);
  const targetMatches = pages.filter((page) => page.targetId === active.target_id);
  if (targetMatches.length > 1) {
    throw new Error(`MCP 页面列表包含重复 target_id: ${active.target_id}`);
  }
  if (targetMatches.length === 1) return targetMatches[0];
  return active.token === expectedSessionToken
    ? findHostedSessionPage(result, expectedSessionToken)
    : findHostedTabPage(result, active.token);
}
