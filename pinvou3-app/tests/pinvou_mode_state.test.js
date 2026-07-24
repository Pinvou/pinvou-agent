#!/usr/bin/env node
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const logicPath = path.join(__dirname, '..', 'src', 'features', 'chat', 'pinvou-mode-state.js');
const code = fs.readFileSync(logicPath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '');

const ctx = {};
vm.createContext(ctx);
vm.runInContext(`${code}
this.CODE_AGENT_PROVIDERS = CODE_AGENT_PROVIDERS;
this.CODE_AGENT_PROVIDER_LABELS = CODE_AGENT_PROVIDER_LABELS;
this.PINVOU_MODE_LABELS = PINVOU_MODE_LABELS;
this.PINVOU_MODES = PINVOU_MODES;
this.createPinvouModeState = createPinvouModeState;
this.loadPinvouModeState = loadPinvouModeState;
this.normalizeCodeAgentProvider = normalizeCodeAgentProvider;
this.normalizePinvouMode = normalizePinvouMode;
this.reducePinvouModeState = reducePinvouModeState;
this.savePinvouModeState = savePinvouModeState;`, ctx, {
  filename: logicPath,
});

const {
  CODE_AGENT_PROVIDERS,
  CODE_AGENT_PROVIDER_LABELS,
  PINVOU_MODE_LABELS,
  PINVOU_MODES,
  createPinvouModeState,
  loadPinvouModeState,
  normalizeCodeAgentProvider,
  normalizePinvouMode,
  reducePinvouModeState,
  savePinvouModeState,
} = ctx;

const plain = (value) => JSON.parse(JSON.stringify(value));

assert.deepStrictEqual(plain(PINVOU_MODES), ['work', 'design', 'code']);
assert.strictEqual(PINVOU_MODE_LABELS.work, '工作');
assert.strictEqual(PINVOU_MODE_LABELS.design, '设计');
assert.strictEqual(PINVOU_MODE_LABELS.code, '代码');

assert.deepStrictEqual(plain(CODE_AGENT_PROVIDERS), ['codex', 'claude-code', 'kimi-code']);
assert.strictEqual(CODE_AGENT_PROVIDER_LABELS.codex, 'Codex');
assert.strictEqual(CODE_AGENT_PROVIDER_LABELS['claude-code'], 'Claude Code');
assert.strictEqual(CODE_AGENT_PROVIDER_LABELS['kimi-code'], 'Kimi Code');

assert.strictEqual(normalizePinvouMode('design'), 'design');
assert.strictEqual(normalizePinvouMode('invalid'), 'work');
assert.strictEqual(normalizeCodeAgentProvider('kimi-code'), 'kimi-code');
assert.strictEqual(normalizeCodeAgentProvider('cursor'), undefined);
assert.strictEqual(normalizeCodeAgentProvider('unknown'), undefined);

let state = createPinvouModeState();
assert.strictEqual(state.mode, 'work');
assert.strictEqual(state.codeProvider, undefined);
assert.strictEqual(state.selectedDesignElementId, undefined);
assert.strictEqual(state.designRuntimeStatus, 'idle');

state = reducePinvouModeState(state, { type: 'set-mode', mode: 'design' });
assert.strictEqual(state.mode, 'design');
assert.strictEqual(state.designRuntimeStatus, 'idle');

state = reducePinvouModeState(state, { type: 'set-selected-design-element', elementId: 'hero-title' });
assert.strictEqual(state.selectedDesignElementId, 'hero-title');

state = reducePinvouModeState(state, { type: 'set-mode', mode: 'code' });
assert.strictEqual(state.mode, 'code');
assert.strictEqual(state.selectedDesignElementId, undefined);
assert.strictEqual(state.designRuntimeStatus, 'idle');

state = reducePinvouModeState(state, { type: 'set-code-provider', provider: 'codex' });
assert.strictEqual(state.codeProvider, 'codex');

const memoryStorage = {
  value: null,
  getItem() { return this.value; },
  setItem(_key, value) { this.value = value; },
};
savePinvouModeState(state, memoryStorage);
assert.deepStrictEqual(JSON.parse(memoryStorage.value), { mode: 'code', codeProvider: 'codex' });
state = loadPinvouModeState(memoryStorage);
assert.strictEqual(state.mode, 'code');
assert.strictEqual(state.codeProvider, 'codex');
assert.strictEqual(state.selectedDesignElementId, undefined);
assert.strictEqual(state.designRuntimeStatus, 'idle');

memoryStorage.value = '{bad json';
assert.strictEqual(loadPinvouModeState(memoryStorage).mode, 'work');

console.log('pinvou_mode_state: ok');
