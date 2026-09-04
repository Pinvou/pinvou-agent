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
  .replace(/import[\s\S]+?from '\.\/personal-workbench-scene\.js';\r?\n/, '')
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '');
const personalWorkbenchPath = path.join(__dirname, '..', 'src', 'features', 'chat', 'personal-workbench-scene.js');
const personalWorkbenchCode = fs.readFileSync(personalWorkbenchPath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '');

const ctx = {};
vm.createContext(ctx);
vm.runInContext(`${code}
this.PINVOU_MODE_STORAGE_KEY = PINVOU_MODE_STORAGE_KEY;
this.PINVOU_MODES = PINVOU_MODES;
this.SUBTABS = SUBTABS;
this.UNROUTED_SUBTAB = UNROUTED_SUBTAB;
this.createPinvouModeScopeKey = createPinvouModeScopeKey;
this.createPinvouModeState = createPinvouModeState;
this.hasPinvouModeState = hasPinvouModeState;
this.loadPinvouModeState = loadPinvouModeState;
this.normalizePinvouMode = normalizePinvouMode;
this.normalizeSubtab = normalizeSubtab;
this.reducePinvouModeState = reducePinvouModeState;
this.savePinvouModeState = savePinvouModeState;
${personalWorkbenchCode}
${workSceneCode}
this.shouldUseDocumentWritingScene = shouldUseDocumentWritingScene;
this.shouldUseDataVisualizationScene = shouldUseDataVisualizationScene;
this.shouldUsePersonalWorkbenchScene = shouldUsePersonalWorkbenchScene;`, ctx, {
  filename: logicPath,
});

const {
  PINVOU_MODE_STORAGE_KEY,
  PINVOU_MODES,
  SUBTABS,
  UNROUTED_SUBTAB,
  createPinvouModeScopeKey,
  createPinvouModeState,
  hasPinvouModeState,
  loadPinvouModeState,
  normalizePinvouMode,
  normalizeSubtab,
  reducePinvouModeState,
  savePinvouModeState,
  shouldUseDocumentWritingScene,
  shouldUseDataVisualizationScene,
  shouldUsePersonalWorkbenchScene,
} = ctx;

const plain = (value) => JSON.parse(JSON.stringify(value));

// The design lane has been merged into work: only work remains, and any
// historical value (including design) folds into work.
assert.deepStrictEqual(plain(PINVOU_MODES), ['work']);
assert.strictEqual(PINVOU_MODE_STORAGE_KEY, 'pinvou_mode_state_v4');
assert.deepStrictEqual(plain(SUBTABS), [
  'general',
  'personal-workbench',
  'document-writing',
  'poster',
  'data-visualization',
]);
assert.strictEqual(UNROUTED_SUBTAB, 'general');

assert.strictEqual(normalizePinvouMode('work'), 'work');
assert.strictEqual(normalizePinvouMode('design'), 'work');
assert.strictEqual(normalizePinvouMode('code'), 'work');
assert.strictEqual(normalizePinvouMode('invalid'), 'work');
assert.strictEqual(normalizeSubtab('invalid'), 'general');
assert.strictEqual(normalizeSubtab('personal-workbench'), 'personal-workbench');
assert.strictEqual(normalizeSubtab('poster'), 'poster');
assert.strictEqual(normalizeSubtab('data-visualization'), 'data-visualization');

let state = createPinvouModeState();
assert.strictEqual(state.mode, 'work');
assert.strictEqual(state.subtab, 'general');
assert.strictEqual(state.workSubtab, undefined);
assert.strictEqual(state.designSubtab, undefined);
assert.strictEqual(state.selectedDesignElementId, undefined);
assert.strictEqual(state.designRuntimeStatus, undefined);
assert.strictEqual(
  shouldUseDocumentWritingScene(state.subtab),
  false,
);
assert.strictEqual(
  shouldUsePersonalWorkbenchScene('personal-workbench'),
  true,
);
assert.strictEqual(
  shouldUseDataVisualizationScene('data-visualization'),
  true,
);

state = reducePinvouModeState(state, { type: 'set-mode', mode: 'design' });
assert.strictEqual(state.mode, 'work', 'historical design values must fold into work');

state = reducePinvouModeState(state, { type: 'set-subtab', subtab: 'data-visualization' });
assert.strictEqual(state.subtab, 'data-visualization');

state = reducePinvouModeState(state, { type: 'set-mode', mode: 'work' });
assert.strictEqual(state.mode, 'work');
assert.strictEqual(state.subtab, 'data-visualization');

state = reducePinvouModeState(state, { type: 'set-design-subtab', subtab: 'poster' });
assert.strictEqual(state.subtab, 'data-visualization', 'the removed action type must have no effect');

const memoryStorage = {
  values: {},
  getItem(key) { return this.values[key] || null; },
  setItem(key, value) { this.values[key] = value; },
};
savePinvouModeState(state, memoryStorage);
assert.deepStrictEqual(JSON.parse(memoryStorage.values[PINVOU_MODE_STORAGE_KEY]).draft, {
  mode: 'work',
  subtab: 'data-visualization',
});
state = loadPinvouModeState(memoryStorage);
assert.deepStrictEqual(plain(state), { mode: 'work', subtab: 'data-visualization' });

const posterScope = createPinvouModeScopeKey('session-poster');
const dataScope = createPinvouModeScopeKey('session-data');
savePinvouModeState({ mode: 'work', subtab: 'poster' }, memoryStorage, posterScope);
savePinvouModeState({ mode: 'work', subtab: 'data-visualization' }, memoryStorage, dataScope);
assert.strictEqual(hasPinvouModeState(memoryStorage, posterScope), true);
assert.strictEqual(hasPinvouModeState(memoryStorage, dataScope), true);
assert.strictEqual(hasPinvouModeState(memoryStorage), false);
assert.strictEqual(loadPinvouModeState(memoryStorage, posterScope).subtab, 'poster');
assert.strictEqual(loadPinvouModeState(memoryStorage, dataScope).subtab, 'data-visualization');
const unknownSessionState = loadPinvouModeState(memoryStorage, createPinvouModeScopeKey('unknown'));
assert.strictEqual(unknownSessionState.mode, 'work');
assert.strictEqual(unknownSessionState.subtab, 'general');
assert.strictEqual(
  shouldUseDocumentWritingScene(unknownSessionState.subtab),
  false,
);
assert.strictEqual(
  shouldUseDocumentWritingScene('document-writing'),
  true,
);

// v3 → v4: mode:'design' folds into mode:'work' + the old designSubtab;
// mode:'work' takes the old workSubtab.
const v3Storage = {
  values: {
    pinvou_mode_state_v3: JSON.stringify({
      draft: { mode: 'design', workSubtab: 'personal-workbench', designSubtab: 'poster' },
      sessions: {
        'session-document': { mode: 'work', workSubtab: 'document-writing', designSubtab: 'poster' },
        'session-data': { mode: 'design', workSubtab: 'document-writing', designSubtab: 'data-visualization' },
      },
      sessionOrder: ['session-document', 'session-data'],
    }),
  },
  getItem(key) { return this.values[key] || null; },
  setItem(key, value) { this.values[key] = value; },
};
const migratedV3Draft = loadPinvouModeState(v3Storage);
assert.strictEqual(migratedV3Draft.mode, 'work');
assert.strictEqual(migratedV3Draft.subtab, 'poster');
const migratedV3Document = loadPinvouModeState(v3Storage, 'session-document');
assert.strictEqual(migratedV3Document.mode, 'work');
assert.strictEqual(migratedV3Document.subtab, 'document-writing');
assert.strictEqual(loadPinvouModeState(v3Storage, 'session-data').subtab, 'data-visualization');

// v3 draft's work branch: mode:'work' takes the old workSubtab directly
// (symmetric with the design branch).
const v3WorkDraftStorage = {
  values: {
    pinvou_mode_state_v3: JSON.stringify({
      draft: { mode: 'work', workSubtab: 'document-writing', designSubtab: 'poster' },
      sessions: {},
      sessionOrder: [],
    }),
  },
  getItem(key) { return this.values[key] || null; },
  setItem(key, value) { this.values[key] = value; },
};
const migratedV3WorkDraft = loadPinvouModeState(v3WorkDraftStorage);
assert.strictEqual(migratedV3WorkDraft.mode, 'work');
assert.strictEqual(migratedV3WorkDraft.subtab, 'document-writing');

// v2 → v4: first apply the old v2→v3 semantics (draft-scoped
// document-writing/poster reset to general, session scopes untouched),
// then the v3 fold.
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
assert.strictEqual(migratedDraft.mode, 'work');
assert.strictEqual(migratedDraft.subtab, 'general');
assert.strictEqual(loadPinvouModeState(previousStorage, 'session-document').subtab, 'document-writing');
assert.strictEqual(loadPinvouModeState(previousStorage, 'session-poster').subtab, 'poster');

// v1 legacy draft: readable even with only the oldest single-value draft,
// with design folding into work.
const legacyStorage = {
  values: {
    pinvou_mode_state_v1: JSON.stringify({ mode: 'design' }),
  },
  getItem(key) { return this.values[key] || null; },
  setItem(key, value) { this.values[key] = value; },
};
const legacyDraft = loadPinvouModeState(legacyStorage);
assert.strictEqual(legacyDraft.mode, 'work');
assert.strictEqual(legacyDraft.subtab, 'general');

memoryStorage.values[PINVOU_MODE_STORAGE_KEY] = '{bad json';
assert.strictEqual(loadPinvouModeState(memoryStorage).mode, 'work');

console.log('pinvou_mode_state: ok');
