#!/usr/bin/env node
const { test } = require('node:test');
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const logicPath = path.join(__dirname, '..', 'src', 'features', 'tools', 'oauth-marketplace-logic.js');
const code = fs.readFileSync(logicPath, 'utf8').replace(/\bexport\s+/g, '');
const toolStoreView = fs.readFileSync(path.join(__dirname, '..', 'src', 'features', 'tools', 'ToolStoreView.jsx'), 'utf8');
const ctx = {};
vm.createContext(ctx);
vm.runInContext(`${code}\nthis.resolveOAuthInstallOutcome = resolveOAuthInstallOutcome;`, ctx, {
  filename: logicPath,
});

const { resolveOAuthInstallOutcome } = ctx;
const toolName = 'Canva 可画';
const pendingAuth = { mcp_configured: true, oauth_token_present: false, status: 'config_installed_auth_pending', server_name: 'canva_mcp' };
const connectedAuth = { mcp_configured: true, oauth_token_present: true, status: 'connected', server_name: 'canva_mcp', message: 'OAuth 授权已完成。' };

function assertNoYuandian(value) {
  assert.doesNotMatch(JSON.stringify(value), /元典|华宇元典/);
}

assert.match(toolStoreView, /oauthServerName = t\?\.oauthMcp \? \(t\?\.oauthServerName \|\| null\) : null;/);
assert.match(toolStoreView, /server_name: oauthServerName/);
assert.match(toolStoreView, /oauthUiTimeoutResult\(oauthServerName\)/);
assert.doesNotMatch(toolStoreView, /oauthUiTimeoutResult\('yuandian_mcp'\)/);
assert.doesNotMatch(toolStoreView, /尚未完成元典 OAuth|尚未连接华宇元典/);

const pending = resolveOAuthInstallOutcome(
  toolName,
  { status: 'timeout', message: '授权超时', server_name: 'canva_mcp' },
  pendingAuth
);
assert.strictEqual(pending.connected, false);
assert.strictEqual(pending.authState.status, 'timeout');
assert.strictEqual(pending.authState.oauth_token_present, false);
assert.strictEqual(pending.selectedToolPatch.installed, false);
assert.strictEqual(pending.alert.isError, true);
assert.strictEqual(pending.alert.title, 'Canva 可画授权超时');
assertNoYuandian(pending);

const falseSuccess = resolveOAuthInstallOutcome(
  toolName,
  { status: 'connected', message: 'ok', server_name: 'canva_mcp' },
  pendingAuth
);
assert.strictEqual(falseSuccess.connected, false);
assert.strictEqual(falseSuccess.authState.status, 'auth_failed');
assert.strictEqual(falseSuccess.authState.oauth_token_present, false);
assert.strictEqual(falseSuccess.selectedToolPatch.installed, false);
assert.match(falseSuccess.alert.subtitle, /未检测到 OAuth token/);
assertNoYuandian(falseSuccess);

const connected = resolveOAuthInstallOutcome(
  toolName,
  { status: 'connected', message: 'ok', server_name: 'canva_mcp' },
  connectedAuth
);
assert.strictEqual(connected.connected, true);
assert.strictEqual(connected.authState.status, 'connected');
assert.strictEqual(connected.authState.oauth_token_present, true);
assert.strictEqual(connected.selectedToolPatch.installed, true);
assert.strictEqual(connected.alert.isError, false);
assertNoYuandian(connected);

const timeoutAtCallbackBoundary = resolveOAuthInstallOutcome(
  toolName,
  { status: 'timeout', message: '授权超时', server_name: 'canva_mcp' },
  connectedAuth
);
assert.strictEqual(timeoutAtCallbackBoundary.connected, true);
assert.strictEqual(timeoutAtCallbackBoundary.authState.oauth_token_present, true);
assert.strictEqual(timeoutAtCallbackBoundary.selectedToolPatch.installed, true);
assertNoYuandian(timeoutAtCallbackBoundary);

const cancelled = resolveOAuthInstallOutcome(
  toolName,
  { status: 'cancelled', message: '已取消等待浏览器授权', server_name: 'canva_mcp' },
  pendingAuth
);
assert.strictEqual(cancelled.connected, false);
assert.strictEqual(cancelled.authState.status, 'cancelled');
assert.strictEqual(cancelled.authState.oauth_token_present, false);
assert.strictEqual(cancelled.selectedToolPatch.installed, false);
assert.match(cancelled.alert.title, /授权已取消/);
assert.match(cancelled.alert.subtitle, /已取消/);
assertNoYuandian(cancelled);

const serviceError = resolveOAuthInstallOutcome(
  toolName,
  { status: 'service_error', message: 'OAuth 授权服务返回错误或 404', server_name: 'canva_mcp' },
  pendingAuth
);
assert.strictEqual(serviceError.connected, false);
assert.strictEqual(serviceError.authState.status, 'service_error');
assert.strictEqual(serviceError.authState.oauth_token_present, false);
assert.match(serviceError.alert.title, /授权服务错误/);
assertNoYuandian(serviceError);

const providerError = resolveOAuthInstallOutcome(
  toolName,
  { status: 'provider_error', message: 'OAuth 授权服务拒绝了本次授权', server_name: 'canva_mcp' },
  pendingAuth
);
assert.strictEqual(providerError.connected, false);
assert.strictEqual(providerError.authState.status, 'provider_error');
assert.strictEqual(providerError.authState.oauth_token_present, false);
assert.match(providerError.alert.subtitle, /拒绝/);
assertNoYuandian(providerError);

// 安装失败路径的 OAuth 请求归属必须在 await 之后复检：catch 入口的检查只覆盖
// 进入时刻，loadBackendState 等待期间 cancelOAuthLoading 可能完成取消并展示取消
// 结果，或用户已发起新一次登录——旧请求此时若仍无条件 setAlert，会用陈旧失败
// 覆盖取消/新登录界面。这里把 ToolStoreView.jsx 的真实 catch 体抽出来在 vm 里
// 执行，用延迟兑现的 loadBackendState 复现两个失去归属窗口（取消/换新）与
// 仍持归属的对照情形。
const storeView = fs.readFileSync(
  path.join(__dirname, '..', 'src', 'features', 'tools', 'ToolStoreView.jsx'),
  'utf8',
);
const installFailAnchor = storeView.indexOf("console.error('install failed:', e);");
assert.ok(installFailAnchor > 0, 'install error-path anchor must exist in ToolStoreView.jsx');
const catchOpen = storeView.lastIndexOf('} catch (e) {', installFailAnchor);
const finallyIdx = storeView.indexOf('} finally {', installFailAnchor);
assert.ok(catchOpen > 0 && finallyIdx > catchOpen, 'install catch body boundaries must exist');
const installCatchBody = storeView.slice(
  catchOpen + '} catch (e) {'.length,
  finallyIdx,
);

const runInstallCatch = async ({ loadBackendState, isCurrentOAuthRequest, setAlert }) => {
  const sandbox = {
    t: { oauthMcp: true },
    backendId: 'canva',
    oauthRequestId: 'req-1',
    e: new Error('consent sync failed'),
    detailCopy: { actions: { operationFailed: '操作失败' } },
    console: { error: () => {} },
    loadBackendState,
    isCurrentOAuthRequest,
    setAlert,
  };
  vm.createContext(sandbox);
  vm.runInContext(
    `this.run = async () => {\n${installCatchBody}\n}`,
    sandbox,
    { filename: 'install-catch-body' },
  );
  await sandbox.run();
};

const deferred = () => {
  let resolve;
  const promise = new Promise((res) => { resolve = res; });
  return { promise, resolve };
};

// 抽出的 catch 体必须真的走到 await 之后才决定是否发布失败提示（否则下面的
// 失去归属场景检测不到任何差异——守卫放这里而不是靠 setAlert 计数巧合）。
assert.match(
  installCatchBody,
  /await loadBackendState\(\);\s*\n\s*if \(t\?\.oauthMcp && !isCurrentOAuthRequest\(backendId, oauthRequestId\)\) return;/,
  'the install catch must recheck OAuth request ownership after the awaited refresh',
);

test('oauth install catch: a cancelled request must not publish the stale failure alert', async () => {
  // 取消竞态：cancelOAuthLoading 在 refresh 等待期间完成，清掉了归属并展示取消
  // 结果；旧失败不得再覆盖它。
  const gate = deferred();
  const alerts = [];
  let owner = 'req-1';
  const pending = runInstallCatch({
    loadBackendState: () => gate.promise,
    isCurrentOAuthRequest: (_id, requestId) => requestId !== null && requestId === owner,
    setAlert: (a) => alerts.push(a),
  });
  owner = undefined; // clearOAuthRequest 已删条目
  gate.resolve();
  await pending;
  assert.strictEqual(alerts.length, 0);
});

test('oauth install catch: a superseded request must not publish the stale failure alert', async () => {
  // 换新竞态：用户在 refresh 等待期间发起新登录（归属条目被替换）；旧失败不得
  // 干扰新登录的界面。
  const gate = deferred();
  const alerts = [];
  let owner = 'req-1';
  const pending = runInstallCatch({
    loadBackendState: () => gate.promise,
    isCurrentOAuthRequest: (_id, requestId) => requestId !== null && requestId === owner,
    setAlert: (a) => alerts.push(a),
  });
  owner = 'req-2'; // beginOAuthRequest 已替换条目
  gate.resolve();
  await pending;
  assert.strictEqual(alerts.length, 0);
});

test('oauth install catch: the current request still publishes the failure alert', async () => {
  // 对照：归属始终有效时，失败照常发布（复现路径保持原语义，不是一刀切吞错）。
  const gate = deferred();
  const alerts = [];
  const pending = runInstallCatch({
    loadBackendState: () => gate.promise,
    isCurrentOAuthRequest: (_id, requestId) => requestId === 'req-1',
    setAlert: (a) => alerts.push(a),
  });
  gate.resolve();
  await pending;
  assert.strictEqual(alerts.length, 1);
  assert.strictEqual(alerts[0].isError, true);
});
