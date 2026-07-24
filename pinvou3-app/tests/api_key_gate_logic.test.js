#!/usr/bin/env node
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const sourcePath = path.join(__dirname, '..', 'src', 'hooks', 'useBridge.js');
const source = fs.readFileSync(sourcePath, 'utf8');
const bridgeSource = fs.readFileSync(
  path.join(__dirname, '..', 'src', 'platform', 'tauri', 'bridge', 'settings.js'),
  'utf8',
);
const start = source.indexOf('function baseUrlIsLoopback(');
const end = source.indexOf('\nexport {', start);

assert.notStrictEqual(start, -1, 'baseUrlIsLoopback must exist');
assert.notStrictEqual(end, -1, 'credential helper boundary must exist');

const context = { URL };
vm.createContext(context);
vm.runInContext(
  `${source.slice(start, end)}
this.baseUrlIsLoopback = baseUrlIsLoopback;
this.isLocalModel = isLocalModel;
this.shouldShowApiKeyGate = shouldShowApiKeyGate;`,
  context,
  { filename: sourcePath },
);

const { baseUrlIsLoopback, isLocalModel, shouldShowApiKeyGate } = context;
const state = (credentialState, config = {}) => ({
  effectiveModelConfig: {
    preset: 'deepseek',
    base_url: 'https://api.deepseek.com',
    credential_state: credentialState,
    ...config,
  },
});

assert.strictEqual(baseUrlIsLoopback('http://localhost:8000/v1'), true);
assert.strictEqual(baseUrlIsLoopback('http://localhost.:8000/v1'), true);
assert.strictEqual(baseUrlIsLoopback('http://127.0.0.42:8000/v1'), true);
assert.strictEqual(baseUrlIsLoopback('http://[::1]:8000/v1'), true);
assert.strictEqual(baseUrlIsLoopback('https://localhost.example.com/v1'), false);
assert.strictEqual(baseUrlIsLoopback('https://127.0.0.10.example.com/v1'), false);
assert.strictEqual(baseUrlIsLoopback('not a url'), false);

assert.strictEqual(isLocalModel({ preset: 'local_vllm', base_url: '' }), true);
assert.strictEqual(isLocalModel({ preset: 'openai_compatible', base_url: 'http://localhost:11434/v1' }), true);
assert.strictEqual(isLocalModel({ preset: 'openai_compatible', base_url: 'https://api.openai.com/v1' }), false);

assert.strictEqual(shouldShowApiKeyGate(state('missing'), 'chat', true), true);
assert.strictEqual(shouldShowApiKeyGate(state('unavailable'), 'chat', true), true);
assert.strictEqual(shouldShowApiKeyGate(state('configured'), 'chat', true), false);
assert.strictEqual(shouldShowApiKeyGate(state('missing'), 'settings', true), false);
assert.strictEqual(shouldShowApiKeyGate(state('missing'), 'chat', false), false);
assert.strictEqual(
  shouldShowApiKeyGate(state('missing', {
    preset: 'openai_compatible',
    base_url: 'http://127.0.0.1:11434/v1',
  }), 'chat', true),
  false,
);
assert.strictEqual(
  shouldShowApiKeyGate({
    ...state('missing'),
    scheduledRunContext: { id: 'run-1' },
  }, 'scheduled', true),
  true,
);
assert.strictEqual(shouldShowApiKeyGate(state('missing'), 'scheduled', true), false);

// 会话切换/热切模型必须把 sessionId 传给后端重新解析真正生效的模型，不能继续沿用全局默认。
assert.match(
  bridgeSource,
  /invoke\("get_effective_model_config", \{ sessionId: requestedSessionId \}\)/,
  'loadSessionModel must refresh effective config for the requested session',
);
assert.match(
  bridgeSource,
  /await loadSessionModel\(sessionId\)/,
  'switchModel must refresh the session-scoped credential gate after switching',
);

console.log('api_key_gate_logic: ok');
