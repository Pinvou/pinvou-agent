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

console.log('marketplace_oauth_logic: ok');
