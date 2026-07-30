#!/usr/bin/env node
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const logicPath = path.join(__dirname, '..', 'src', 'features', 'chat', 'pinvou-mode-state.js');
const code = fs.readFileSync(logicPath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '');
const workScenePath = path.join(__dirname, '..', 'src', 'features', 'chat', 'work-scene-routes.js');
const workSceneCode = fs.readFileSync(workScenePath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '');

const ctx = {};
vm.createContext(ctx);
vm.runInContext(`${code}
this.PINVOU_MODE_LABELS = PINVOU_MODE_LABELS;
this.PINVOU_MODE_STORAGE_KEY = PINVOU_MODE_STORAGE_KEY;
this.PINVOU_MODES = PINVOU_MODES;
this.createPinvouModeScopeKey = createPinvouModeScopeKey;
this.createPinvouModeState = createPinvouModeState;
this.hasPinvouModeState = hasPinvouModeState;
this.loadPinvouModeState = loadPinvouModeState;
this.normalizeDesignSubtab = normalizeDesignSubtab;
this.normalizePinvouMode = normalizePinvouMode;
this.normalizeWorkSubtab = normalizeWorkSubtab;
this.reducePinvouModeState = reducePinvouModeState;
this.savePinvouModeState = savePinvouModeState;
${workSceneCode}
this.shouldUseDocumentWritingScene = shouldUseDocumentWritingScene;`, ctx, {
  filename: logicPath,
});

const {
  PINVOU_MODE_LABELS,
  PINVOU_MODE_STORAGE_KEY,
  PINVOU_MODES,
  createPinvouModeScopeKey,
  createPinvouModeState,
  hasPinvouModeState,
  loadPinvouModeState,
  normalizeDesignSubtab,
  normalizePinvouMode,
  normalizeWorkSubtab,
  reducePinvouModeState,
  savePinvouModeState,
  shouldUseDocumentWritingScene,
} = ctx;

const plain = (value) => JSON.parse(JSON.stringify(value));

assert.deepStrictEqual(plain(PINVOU_MODES), ['work', 'design']);
assert.strictEqual(PINVOU_MODE_LABELS.work, '工作');
assert.strictEqual(PINVOU_MODE_LABELS.design, '设计');
assert.strictEqual(PINVOU_MODE_LABELS.code, '代码');

assert.strictEqual(normalizePinvouMode('design'), 'design');
assert.strictEqual(normalizePinvouMode('code'), 'work');
assert.strictEqual(normalizePinvouMode('invalid'), 'work');
assert.strictEqual(normalizeWorkSubtab('invalid'), 'general');
assert.strictEqual(normalizeDesignSubtab('invalid'), 'general');

let state = createPinvouModeState();
assert.strictEqual(state.mode, 'work');
assert.strictEqual(state.workSubtab, 'general');
assert.strictEqual(state.designSubtab, 'general');
assert.strictEqual(
  shouldUseDocumentWritingScene(state.mode, state.workSubtab),
  false,
);
assert.strictEqual(state.selectedDesignElementId, undefined);
assert.strictEqual(state.designRuntimeStatus, 'idle');

state = reducePinvouModeState(state, { type: 'set-mode', mode: 'design' });
assert.strictEqual(state.mode, 'design');
assert.strictEqual(state.designRuntimeStatus, 'idle');

state = reducePinvouModeState(state, { type: 'set-design-subtab', subtab: 'data-visualization' });
assert.strictEqual(state.designSubtab, 'data-visualization');

state = reducePinvouModeState(state, { type: 'set-selected-design-element', elementId: 'hero-title' });
assert.strictEqual(state.selectedDesignElementId, 'hero-title');

state = reducePinvouModeState(state, { type: 'set-mode', mode: 'code' });
assert.strictEqual(state.mode, 'work');
assert.strictEqual(state.selectedDesignElementId, undefined);
assert.strictEqual(state.designRuntimeStatus, 'idle');

const memoryStorage = {
  values: {},
  getItem(key) { return this.values[key] || null; },
  setItem(key, value) { this.values[key] = value; },
};
savePinvouModeState(state, memoryStorage);
assert.deepStrictEqual(JSON.parse(memoryStorage.values[PINVOU_MODE_STORAGE_KEY]).draft, {
  mode: 'work',
  workSubtab: 'general',
  designSubtab: 'data-visualization',
});
state = loadPinvouModeState(memoryStorage);
assert.strictEqual(state.mode, 'work');
assert.strictEqual(state.designSubtab, 'data-visualization');
assert.strictEqual(state.selectedDesignElementId, undefined);
assert.strictEqual(state.designRuntimeStatus, 'idle');

const posterScope = createPinvouModeScopeKey('session-poster');
const dataScope = createPinvouModeScopeKey('session-data');
savePinvouModeState({ mode: 'design', designSubtab: 'poster' }, memoryStorage, posterScope);
savePinvouModeState({ mode: 'design', designSubtab: 'data-visualization' }, memoryStorage, dataScope);
assert.strictEqual(hasPinvouModeState(memoryStorage, posterScope), true);
assert.strictEqual(hasPinvouModeState(memoryStorage, dataScope), true);
assert.strictEqual(loadPinvouModeState(memoryStorage, posterScope).designSubtab, 'poster');
assert.strictEqual(loadPinvouModeState(memoryStorage, dataScope).designSubtab, 'data-visualization');
const unknownSessionState = loadPinvouModeState(memoryStorage, createPinvouModeScopeKey('unknown'));
assert.strictEqual(unknownSessionState.mode, 'work');
assert.strictEqual(unknownSessionState.workSubtab, 'general');
assert.strictEqual(
  shouldUseDocumentWritingScene(unknownSessionState.mode, unknownSessionState.workSubtab),
  false,
);
assert.strictEqual(
  shouldUseDocumentWritingScene(state.mode, 'document-writing'),
  true,
);

const previousStorage = {
  values: {
    pinvou_mode_state_v2: JSON.stringify({
      draft: { mode: 'work', workSubtab: 'document-writing', designSubtab: 'poster' },
      sessions: {
        'session-document': { mode: 'work', workSubtab: 'document-writing', designSubtab: 'poster' },
        'session-poster': { mode: 'design', workSubtab: 'document-writing', designSubtab: 'poster' },
      },
      sessionOrder: ['session-document', 'session-poster'],
    }),
  },
  getItem(key) { return this.values[key] || null; },
  setItem(key, value) { this.values[key] = value; },
};
const migratedDraft = loadPinvouModeState(previousStorage);
assert.strictEqual(migratedDraft.workSubtab, 'general');
assert.strictEqual(migratedDraft.designSubtab, 'general');
assert.strictEqual(loadPinvouModeState(previousStorage, 'session-document').workSubtab, 'document-writing');
assert.strictEqual(loadPinvouModeState(previousStorage, 'session-poster').designSubtab, 'poster');

memoryStorage.values[PINVOU_MODE_STORAGE_KEY] = '{bad json';
assert.strictEqual(loadPinvouModeState(memoryStorage).mode, 'work');

console.log('pinvou_mode_state: ok');
