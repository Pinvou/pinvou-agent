#!/usr/bin/env node
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const logicPath = path.join(__dirname, '..', 'src', 'features', 'tools', 'oauth-marketplace-logic.js');
const code = fs.readFileSync(logicPath, 'utf8').replace(/\bexport\s+/g, '');
const ctx = {};
vm.createContext(ctx);
vm.runInContext(`${code}\nthis.resolveOAuthInstallOutcome = resolveOAuthInstallOutcome;`, ctx, {
  filename: logicPath,
});

const { resolveOAuthInstallOutcome } = ctx;

let pending = resolveOAuthInstallOutcome(
  '华宇元典法律数据',
  { status: 'timeout', message: '授权超时' },
  { mcp_configured: true, oauth_token_present: false, status: 'config_installed_auth_pending' }
);
assert.strictEqual(pending.connected, false);
assert.strictEqual(pending.authState.status, 'timeout');
assert.strictEqual(pending.authState.oauth_token_present, false);
assert.strictEqual(pending.selectedToolPatch.installed, false);
assert.strictEqual(pending.alert.isError, true);

let falseSuccess = resolveOAuthInstallOutcome(
  '华宇元典法律数据',
  { status: 'connected', message: 'ok' },
  { mcp_configured: true, oauth_token_present: false, status: 'config_installed_auth_pending' }
);
assert.strictEqual(falseSuccess.connected, false);
assert.strictEqual(falseSuccess.authState.status, 'auth_failed');
assert.strictEqual(falseSuccess.authState.oauth_token_present, false);
assert.strictEqual(falseSuccess.selectedToolPatch.installed, false);
assert.match(falseSuccess.alert.subtitle, /未检测到 OAuth token/);

let connected = resolveOAuthInstallOutcome(
  '华宇元典法律数据',
  { status: 'connected', message: 'ok' },
  {
    mcp_configured: true,
    oauth_token_present: true,
    status: 'connected',
    message: '已完成元典 OAuth 授权。',
  }
);
assert.strictEqual(connected.connected, true);
assert.strictEqual(connected.authState.status, 'connected');
assert.strictEqual(connected.authState.oauth_token_present, true);
assert.strictEqual(connected.selectedToolPatch.installed, true);
assert.strictEqual(connected.alert.isError, false);

let timeoutAtCallbackBoundary = resolveOAuthInstallOutcome(
  '华宇元典法律数据',
  { status: 'timeout', message: '授权超时' },
  {
    mcp_configured: true,
    oauth_token_present: true,
    status: 'connected',
    message: '已完成元典 OAuth 授权。',
  }
);
assert.strictEqual(timeoutAtCallbackBoundary.connected, true);
assert.strictEqual(timeoutAtCallbackBoundary.authState.oauth_token_present, true);
assert.strictEqual(timeoutAtCallbackBoundary.selectedToolPatch.installed, true);

let cancelled = resolveOAuthInstallOutcome(
  '华宇元典法律数据',
  { status: 'cancelled', message: '已取消等待浏览器授权' },
  { mcp_configured: true, oauth_token_present: false, status: 'config_installed_auth_pending' }
);
assert.strictEqual(cancelled.connected, false);
assert.strictEqual(cancelled.authState.status, 'cancelled');
assert.strictEqual(cancelled.authState.oauth_token_present, false);
assert.strictEqual(cancelled.selectedToolPatch.installed, false);
assert.match(cancelled.alert.title, /授权已取消/);
assert.match(cancelled.alert.subtitle, /已取消/);

let serviceError = resolveOAuthInstallOutcome(
  '华宇元典法律数据',
  { status: 'service_error', message: '元典授权服务返回错误或 404' },
  { mcp_configured: true, oauth_token_present: false, status: 'config_installed_auth_pending' }
);
assert.strictEqual(serviceError.connected, false);
assert.strictEqual(serviceError.authState.status, 'service_error');
assert.strictEqual(serviceError.authState.oauth_token_present, false);
assert.match(serviceError.alert.title, /授权服务错误/);

let providerError = resolveOAuthInstallOutcome(
  '华宇元典法律数据',
  { status: 'provider_error', message: '元典 OAuth 授权服务拒绝了本次授权' },
  { mcp_configured: true, oauth_token_present: false, status: 'config_installed_auth_pending' }
);
assert.strictEqual(providerError.connected, false);
assert.strictEqual(providerError.authState.status, 'provider_error');
assert.strictEqual(providerError.authState.oauth_token_present, false);
assert.match(providerError.alert.subtitle, /拒绝/);

console.log('marketplace_oauth_logic: ok');
