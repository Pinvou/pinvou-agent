#!/usr/bin/env node
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

assert.match(toolStoreView, /const oauthServerNameForTool = \(tool\) => tool\?\.oauthServerName \|\| tool\?\.serverName \|\| null;/);
assert.match(toolStoreView, /server_name: oauthServerName/);
assert.match(toolStoreView, /oauthUiTimeoutResult\(oauthServerName\)/);
assert.doesNotMatch(toolStoreView, /oauthUiTimeoutResult\('yuandian_mcp'\)/);
assert.doesNotMatch(toolStoreView, /尚未完成元典 OAuth|尚未连接华宇元典/);

let pending = resolveOAuthInstallOutcome(
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

let falseSuccess = resolveOAuthInstallOutcome(
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

let connected = resolveOAuthInstallOutcome(
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

let timeoutAtCallbackBoundary = resolveOAuthInstallOutcome(
  toolName,
  { status: 'timeout', message: '授权超时', server_name: 'canva_mcp' },
  connectedAuth
);
assert.strictEqual(timeoutAtCallbackBoundary.connected, true);
assert.strictEqual(timeoutAtCallbackBoundary.authState.oauth_token_present, true);
assert.strictEqual(timeoutAtCallbackBoundary.selectedToolPatch.installed, true);
assertNoYuandian(timeoutAtCallbackBoundary);

let cancelled = resolveOAuthInstallOutcome(
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

let serviceError = resolveOAuthInstallOutcome(
  toolName,
  { status: 'service_error', message: 'OAuth 授权服务返回错误或 404', server_name: 'canva_mcp' },
  pendingAuth
);
assert.strictEqual(serviceError.connected, false);
assert.strictEqual(serviceError.authState.status, 'service_error');
assert.strictEqual(serviceError.authState.oauth_token_present, false);
assert.match(serviceError.alert.title, /授权服务错误/);
assertNoYuandian(serviceError);

let providerError = resolveOAuthInstallOutcome(
  toolName,
  { status: 'provider_error', message: 'OAuth 授权服务拒绝了本次授权', server_name: 'canva_mcp' },
  pendingAuth
);
assert.strictEqual(providerError.connected, false);
assert.strictEqual(providerError.authState.status, 'provider_error');
assert.strictEqual(providerError.authState.oauth_token_present, false);
assert.match(providerError.alert.subtitle, /拒绝/);
assertNoYuandian(providerError);

console.log('marketplace_oauth_logic: ok');
