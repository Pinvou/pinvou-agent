import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import {
  adaptBrowserCatalog,
  adaptHostedBrowserToolCall,
  assertAllowedBrowserToolCall,
  assertAllowedHostedNavigation,
  browserHostBackendPolicy,
  browserToolMayMutate,
  buildBijectivePageTokenMaps,
  createHostCallerHeartbeat,
  createHostCancellationTombstone,
  createHostLeaseAssertionRequest,
  createHostRequestEnvelope,
  effectiveNavigateType,
  explicitOwnedPageId,
  filterPagesResult,
  findHostedSessionPage,
  findHostedTabPage,
  findHostedWorkspacePage,
  HOST_CALLER_HEARTBEAT_INTERVAL_MS,
  HOST_CALLER_HEARTBEAT_TTL_MS,
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
  remapCancellationNotification,
  routeToolCallToPage,
  runLeasedHostDispatch,
  runVisiblePageOperation,
  uncancelledBufferedRequests,
} from '../src-tauri/resources/common/bundle/mcp-servers/browser-wrapper-protocol.mjs';

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

test('Windows 原生工作区保留多标签工具并隐藏不可读的截图工具', () => {
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

test('shim 工具目录暴露的 pageId schema 与上游 experimental routing 一致', () => {
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

test('最终多标签模式不再把 new_page 改写成当前页导航', () => {
  const line = JSON.stringify({
    jsonrpc: '2.0',
    id: 7,
    method: 'tools/call',
    params: { name: 'new_page', arguments: { url: 'https://example.com' } },
  });
  assert.equal(adaptHostedBrowserToolCall(line), line);
});

test('background new_page 保留后台预创建语义', () => {
  const line = JSON.stringify({
    jsonrpc: '2.0',
    id: 8,
    method: 'tools/call',
    params: {
      name: 'new_page',
      arguments: { url: 'https://example.com', background: true },
    },
  });
  assert.equal(adaptHostedBrowserToolCall(line), line);
});

test('浏览器宿主策略不包含外部 Chrome 回退', () => {
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

test('ensureBrowserRunning 控制流不能调用外部浏览器辅助函数', () => {
  const source = readFileSync(WRAPPER_URL, 'utf8');
  const start = source.indexOf('async function ensureBrowserRunning()');
  const end = source.indexOf('// MCP 目录', start);
  assert.ok(start >= 0 && end > start, '应能定位 ensureBrowserRunning 实现');
  const body = source.slice(start, end);
  assert.match(body, /browserHostBackendPolicy\(process\.platform\)/);
  assert.doesNotMatch(body, /\b(?:startChrome|findChrome|pickFreePort)\s*\(/);
  assert.doesNotMatch(body, /owner\s*===\s*['"]mcp['"]/);
  assert.doesNotMatch(
    source,
    /\b(?:startChrome|findChrome|pickFreePort|writePortFile|killChromeChild)\s*\(/,
    '外部浏览器启动与清理辅助函数应从产品代码中彻底移除',
  );
  assert.doesNotMatch(source, /PINVOU_BROWSER_CHROME_PATH/);
});

test('宿主请求使用唯一响应路径、幂等键和超时取消 tombstone', () => {
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
  assert.equal(HOST_CALLER_HEARTBEAT_TTL_MS, 5_000);
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
    '超时时必须先提交取消 tombstone，再撤回尚未领取的请求',
  );
  assert.match(timeoutBody, /quarantineTimedOutHostResponse\(responsePath, tombstonePath\)/);
  const quarantineStart = source.indexOf('function quarantineTimedOutHostResponse(');
  const quarantineEnd = source.indexOf('function cancelTimedOutHostRequest(', quarantineStart);
  const quarantineBody = source.slice(quarantineStart, quarantineEnd);
  assert.ok(quarantineStart >= 0 && quarantineEnd > quarantineStart);
  assert.doesNotMatch(
    quarantineBody,
    /unlinkSync\(tombstonePath\)/,
    '调用方不得按 TTL 删除取消权威；只能等待宿主消费',
  );
  const requestStart = source.indexOf('async function requestHost(');
  const requestEnd = source.indexOf('/**\n * Windows 主应用负责创建', requestStart);
  const requestBody = source.slice(requestStart, requestEnd);
  assert.match(requestBody, /return parseHostResponseEnvelope\(response/);
  assert.doesNotMatch(requestBody, /response\?\.request_id != null/);
});

test('v3 host response 身份字段缺失或不匹配时必须拒绝', () => {
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

test('启动失败不会给启动期间已取消的 buffered 请求回错误', () => {
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
  assert.match(body, /const failed = uncancelledBufferedRequests\(bufferedLines, cancelledIds\)/);
  assert.ok(
    body.indexOf('const failed = uncancelledBufferedRequests') < body.indexOf('cancelledIds.clear()'),
    '取消集合必须在失败请求过滤之后才能清空',
  );
});

test('解析 structuredContent 页面并保留稳定 pageId', () => {
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

test('list_pages 可接收宿主权威 targetId 字段', () => {
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

test('旧 MCP 协议可从文本结果解析页面', () => {
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

test('第一次前台 new_page 只复用宿主初始化空白页', () => {
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
  }), false, '用户创建的普通空白标签不能被当成初始化占位页覆盖');
});

test('list_pages 只向 Agent 返回当前对话允许的标签', () => {
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

test('wrapper 过滤用于 target join 的同一份 list_pages 结果', () => {
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

test('识别初始对话页和新建内嵌标签 marker', () => {
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

test('MCP 重启后按宿主 active target 恢复已导航页面，不依赖 URL marker', () => {
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
    /重复 target_id/,
  );
});

test('URL fragment 消失后只接受已验证的宿主页映射', () => {
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

test('远程 URL marker 或不匹配的 structured target 不能冒充宿主页面', () => {
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

test('wrapper 不向远程页面主世界写入会话或标签 token', () => {
  const source = readFileSync(WRAPPER_URL, 'utf8');
  assert.doesNotMatch(source, /__PINVOU_BROWSER_TAB_TOKEN__|markerInitScript/);
});

test('wrapper 运行时严格消费 v2 映射并完整接线宿主 lease 协议', () => {
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
  assert.match(routeBody, /onRefreshFailure:[\s\S]{0,500}signalManagedUpstreamCancellation/);
  assert.match(routeBody, /emitsTrustedInput \? 'refresh_agent_input' : 'refresh_agent_operation'/);
  assert.match(
    routeBody,
    /heartbeatIntervalMs: emitsTrustedInput[\s\S]{0,120}WINDOWS_AGENT_OPERATION_HEARTBEAT_INTERVAL_MS/,
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
    '初始化空白页复用必须在可见页 lease 内执行导航',
  );
  assert.match(newBody, /URL 首航由宿主在未发布的 staging 标签内完成/);
  const createStart = newBody.indexOf('const creationAuthorization');
  assert.doesNotMatch(
    newBody.slice(createStart),
    /callUpstreamTool\(\s*'navigate_page'/,
    '真正创建标签后的 lease 已失效，前后台 new_page 都不得由 wrapper 直连 target 首航',
  );
});

test('pageId 与 tabToken 映射强制双射，重复项不能任取首个', () => {
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
    /重复 pageId/,
  );
  assert.throws(
    () => buildBijectivePageTokenMaps([
      [7, '0123456789abcdef'],
      [8, '0123456789abcdef'],
    ]),
    /重复 tabToken/,
  );
  assert.throws(
    () => buildBijectivePageTokenMaps([
      [7, '0123456789abcdef'],
      [7, '0123456789abcdef'],
    ]),
    /重复 pageId/,
  );
});

test('显式 pageId 必须在同步或选择前通过当前对话归属校验', () => {
  const pages = new Map([[7, '0123456789abcdef']]);
  assert.equal(explicitOwnedPageId({}, pages), null);
  assert.equal(explicitOwnedPageId({ pageId: 7 }, pages), 7);
  assert.throws(() => explicitOwnedPageId({ pageId: '7' }, pages), /不属于当前对话/);
  assert.throws(() => explicitOwnedPageId({ pageId: 8 }, pages), /不属于当前对话/);

  const source = readFileSync(WRAPPER_URL, 'utf8');
  const ordinaryStart = source.indexOf('// 所有普通页面工具执行前');
  const ordinaryEnd = source.indexOf('// 与 MCP 子进程的握手 id', ordinaryStart);
  const ordinaryBody = source.slice(ordinaryStart, ordinaryEnd);
  assert.ok(
    ordinaryBody.indexOf('explicitOwnedPageId(args, pageIdToTabToken)') <
      ordinaryBody.indexOf('await syncWorkspacePagesBeforeDispatch(false, msg.id)'),
    '显式 pageId 归属必须早于任何页面同步/选择',
  );
  assert.match(
    ordinaryBody,
    /const requestedTabToken =[\s\S]*?await syncWorkspacePagesBeforeDispatch[\s\S]*?pageIdToTabToken\.get\(requestedPageId\) !== requestedTabToken/,
    '同步后必须拒绝复用同一 numeric pageId 的另一标签',
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
  const closeEnd = source.indexOf('// 所有普通页面工具执行前', closeStart);
  const closeBody = source.slice(closeStart, closeEnd);
  assert.match(
    closeBody,
    /const closingTabToken =[\s\S]*?await syncWorkspacePagesBeforeDispatch[\s\S]*?pageIdToTabToken\.get\(closingPageId\) !== closingTabToken/,
  );
});

test('v2 宿主权威映射必须完整，残缺状态不得回落网页 marker', () => {
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
    /未提供 v2 权威 target 映射/,
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
    /缺少权威 target_id/,
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
    /重复 target_id/,
  );
});

test('activate_tab lease schema 严格生成 assert_host_lease 参数', () => {
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
  assert.deepEqual(createHostLeaseAssertionRequest(lease), {
    operation: 'assert_host_lease',
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
    /缺少 dispatch lease/,
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
    /targetId 与宿主映射不一致/,
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
    /未把控制权授予 Agent/,
  );
});

test('create/close mutation 使用独立 authorization_tab_token，createId 绑定 request_id', () => {
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
    /creationId 与 request_id 不一致/,
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

test('new_page/close_page 的 v3 CAS 接线与精确补偿不可退化', () => {
  const source = readFileSync(WRAPPER_URL, 'utf8');
  const newStart = source.indexOf("if (name === 'new_page')");
  const closeStart = source.indexOf("if (name === 'close_page')", newStart);
  const newBody = source.slice(newStart, closeStart);
  assert.ok(newStart >= 0 && closeStart > newStart);
  assert.ok(
    newBody.indexOf('const creationAuthorization = await runOnVisibleHostedPage') <
      newBody.indexOf("'create_tab'"),
    'create_tab 前必须先激活当前 active 页取得 lease',
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

  const ordinaryStart = source.indexOf('// 所有普通页面工具执行前', closeStart);
  const closeBody = source.slice(closeStart, ordinaryStart);
  assert.match(closeBody, /tab_token: closingToken/);
  assert.match(closeBody, /\.\.\.hostMutationAuthorizationPayload\(aligned\.activationResult\)/);
  assert.match(closeBody, /refreshOperation:[\s\S]*?'refresh_agent_operation'/);
  assert.match(closeBody, /heartbeatIntervalMs: WINDOWS_AGENT_OPERATION_HEARTBEAT_INTERVAL_MS/);
  assert.match(closeBody, /\(\) => cancelledProxyRequestIds\.has\(msg\.id\)/);
});

test('lease dispatch 在执行失败或取消时也会 finally 关闭宿主操作窗口', async (t) => {
  const activationLease = {
    sessionId: 'session-a',
    tabToken: '0123456789abcdef',
    targetId: 'target-a',
    revision: 12,
    owner: 'agent',
    lease: '0123456789abcdef0123456789abcdef',
  };
  await t.test('工具执行失败后关闭', async () => {
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

  await t.test('begin 后收到取消也关闭且不执行工具', async () => {
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

  await t.test('begin acknowledgement 丢失时也尽力撤销同一 lease', async () => {
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

  await t.test('工具已提交后 end 清理失败保留成功结果且只记录告警', async () => {
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

  await t.test('非输入工具也续租，且 heartbeat 必须在 end 前完全停止', async () => {
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

  await t.test('heartbeat 失败但上游已成功时保留已提交结果', async () => {
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

  await t.test('heartbeat 失败且上游取消时返回不可重放的未知提交结果', async () => {
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

  await t.test('heartbeat 失败且上游返回 tool error 时也标记为未知且不可重放', async () => {
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

test('识别 experimental page-id-routing 中的页面级工具', () => {
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

test('按上游工具注解识别需要 trusted-input suppression 的操作', () => {
  const names = inputToolNames({
    tools: [
      { name: 'click', annotations: { category: 'input' } },
      { name: 'type_text', annotations: { category: 'input' } },
      { name: 'navigate_page', annotations: { category: 'navigation' } },
    ],
  });
  assert.deepEqual([...names], ['click', 'type_text']);
});

test('页面级工具注入当前对话的 pageId 并保留原参数', () => {
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
  assert.equal(message.params.arguments.pageId, undefined, '不得原地修改请求');
});

test('显式后台 pageId 必须先激活宿主标签、选择 Target、复核后才执行', async () => {
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

test('页面归属、宿主激活、Target 选择或复核失败时不得执行工具', async (t) => {
  await t.test('跨对话 pageId 在任何副作用前拒绝', async () => {
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
      /不属于当前对话/,
    );
    assert.equal(called, false);
  });

  await t.test('宿主 lease 复核失败时不选择 Target', async () => {
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
    await t.test(`${failedPhase} 失败不执行`, async () => {
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

test('排队调用在激活后收到取消时不会继续选择或执行', async () => {
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

test('复核后页面归属发生变化时仍不得进入实际执行', async () => {
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
    /不属于当前对话|归属在执行前发生变化/,
  );
  assert.equal(executed, false);
});

test('受管工具取消通知改写为当前内部请求 id 且不修改原消息', () => {
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
  const childStart = source.indexOf('function onProxyChildData(', queueStart);
  const queueBody = source.slice(queueStart, childStart);
  assert.match(queueBody, /cancelManagedUpstreamRequest\(/);
  assert.match(queueBody, /msg\.params\?\.reason \|\| '浏览器工具调用已取消'/);
  const cancelStart = source.indexOf('function cancelManagedUpstreamRequest(');
  const cancelEnd = source.indexOf('function queueProxyLine(', cancelStart);
  const cancelBody = source.slice(cancelStart, cancelEnd);
  assert.match(cancelBody, /signalManagedUpstreamCancellation\(externalRequestId, reason\)/);
  assert.match(cancelBody, /internalRequests\.delete\(internalRequestId\)/);
  assert.match(cancelBody, /pending\.reject\(new Error\(reason\)\)/);
  assert.ok(
    cancelBody.indexOf('if (pending?.awaitRealSettlement)') <
      cancelBody.indexOf('internalRequests.delete(internalRequestId)'),
    '已 begin 的受管 dispatch 只能合作取消并等待真实上游终态',
  );
  const childEnd = source.indexOf('function callUpstreamRequest(', childStart);
  const childBody = source.slice(childStart, childEnd);
  assert.match(childBody, /discardedInternalRequestIds\.delete\(msg\.id\)/);
  assert.ok(
    childBody.indexOf('discardedInternalRequestIds.delete') <
      childBody.lastIndexOf("process.stdout.write(line + '\\n')"),
    '取消/超时后的内部晚响应必须先丢弃，不能泄漏给外部引擎',
  );
});

test('受管 dispatch 的取消、超时与 child exit 都在真实终态后结束宿主操作', () => {
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
    '必须先确认上游停止，再结算仍在执行的内部请求',
  );
  assert.ok(
    shutdownBody.indexOf('settleInternalRequestsAfterUpstreamStopped(reason)') <
      shutdownBody.indexOf('await Promise.allSettled([proxyQueue, hostCoreQueue])'),
    '结算上游请求后仍须等待 leased dispatch finally/end 完成',
  );
  assert.match(source, /child\.on\('exit',[\s\S]*?gracefulShutdown\(/);
  assert.match(source, /process\.stdin\.on\('end',[\s\S]*?gracefulShutdown\(/);
});

test('应用内导航拒绝本地文件和脚本协议', () => {
  assert.equal(isAllowedBrowserUrl('https://example.com/path'), true);
  assert.equal(isAllowedBrowserUrl('http://127.0.0.1:3000/'), true);
  assert.equal(isAllowedBrowserUrl('about:blank'), true);
  assert.equal(isAllowedBrowserUrl('file:///C:/Users/example/secrets.txt'), false);
  assert.equal(isAllowedBrowserUrl('javascript:alert(1)'), false);
  assert.equal(isAllowedBrowserUrl('data:text/html,unsafe'), false);
  assert.equal(isAllowedBrowserUrl('example.com'), false);
});

test('navigate_page 省略 type 但携带 url 时仍走严格 URL 白名单', () => {
  assert.equal(effectiveNavigateType({ url: 'https://example.com' }), 'url');
  assert.equal(effectiveNavigateType({ type: 'url', url: 'https://example.com' }), 'url');
  assert.equal(effectiveNavigateType({ type: 'reload' }), 'reload');
  assert.equal(assertAllowedHostedNavigation({ url: 'https://example.com' }), 'url');
  assert.throws(
    () => assertAllowedHostedNavigation({ url: 'file:///C:/Users/example/secrets.txt' }),
    /仅支持 http\/https\/about:blank/,
  );
  assert.throws(
    () => assertAllowedHostedNavigation({ url: 'javascript:alert(1)' }),
    /仅支持 http\/https\/about:blank/,
  );
  // 显式 reload 不会使用多余 url 参数执行导航；显式 type 优先。
  assert.equal(
    assertAllowedHostedNavigation({ type: 'reload', url: 'javascript:ignored()' }),
    'reload',
  );

  const source = readFileSync(WRAPPER_URL, 'utf8');
  const routeStart = source.indexOf('async function routeHostedToolCall(');
  const routeEnd = source.indexOf('// 与 MCP 子进程的握手 id', routeStart);
  const routeBody = source.slice(routeStart, routeEnd);
  assert.ok(
    routeBody.indexOf("if (name === 'navigate_page') assertAllowedHostedNavigation(args)") <
      routeBody.indexOf('if (!runtimePageScopedTools.has(name))'),
    'URL 白名单必须先于可能的非受管透传分支',
  );
});
