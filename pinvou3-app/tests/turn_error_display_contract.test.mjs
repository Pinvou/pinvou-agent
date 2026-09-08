import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

// Regression for the turn-error display chain: stable backend error codes
// (session_model_binding_stale:<id>, image_input_unsupported) must reach the
// tri-lingual recovery copy when the rejection arrives as an Error object —
// the shape the real Web RPC client produces (bootstrap.js wraps message.error
// in new Error). Error.toString() prepends "Error: ", so prefix matching must
// run on Error.message; this file executes the shipped bridge code to prove
// the localized copy is reachable in all three locales.

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, '..');
const read = (...parts) => fs.readFileSync(path.join(root, ...parts), 'utf8');

const bridgeMessagesSource = read('src', 'shared', 'bridge-messages.js');
const rawBridgeSource = read('src', 'platform', 'web', 'bridge.js');
const chatSource = read('src', 'platform', 'tauri', 'bridge', 'chat.js');

assert.equal(
  rawBridgeSource.split('function displayTurnError(err)').length,
  2,
  'displayTurnError must stay a single function so the hook injection is unambiguous',
);
const bridgeSource = rawBridgeSource.replace(
  'function displayTurnError(err)',
  'window.__displayTurnError = function displayTurnError(err)',
);

const storage = new Map();
const localStorage = {
  getItem(key) { return storage.has(key) ? storage.get(key) : null; },
  setItem(key, value) { storage.set(key, String(value)); },
  removeItem(key) { storage.delete(key); },
};
const documentObject = {
  readyState: 'loading',
  addEventListener() {},
  createElement() {
    return { click() {}, remove() {}, style: {}, setAttribute() {} };
  },
  body: { appendChild() {} },
};
let invokeResponse = async () => null;
const windowObject = {
  PinvouPlatform: {
    kind: 'web',
    isWeb: true,
    capabilities: {},
    can: () => false,
    canInvoke: () => false,
  },
  __TAURI__: {
    core: { invoke: (...args) => invokeResponse(...args) },
    event: { listen: async () => function () {} },
    dialog: { open: async () => null },
  },
  location: { search: '', href: 'https://example.test/pinvou3/remote/' },
  localStorage,
  crypto: { randomUUID: () => '00000000-0000-4000-8000-000000000000' },
  performance: { now: () => 0 },
  addEventListener() {},
  setTimeout,
  clearTimeout,
  setInterval,
  clearInterval,
};
const context = vm.createContext({
  window: windowObject,
  document: documentObject,
  navigator: { mediaDevices: null },
  localStorage,
  console,
  setTimeout,
  clearTimeout,
  setInterval,
  clearInterval,
  structuredClone: value => structuredClone(value),
  URL,
  URLSearchParams,
  Blob,
  Uint8Array,
  ArrayBuffer,
  TextEncoder,
  TextDecoder,
});

vm.runInContext(bridgeMessagesSource, context, { filename: 'src/shared/bridge-messages.js' });
vm.runInContext(bridgeSource, context, { filename: 'src/platform/web/bridge.js' });
vm.runInContext(read('src', 'platform', 'web', 'bridge', 'domain-adapter.js'), context, {
  filename: 'src/platform/web/bridge/domain-adapter.js',
});

const displayTurnError = windowObject.__displayTurnError;
assert.equal(typeof displayTurnError, 'function', 'displayTurnError hook must be exposed');
const api = windowObject.TauriBridge;
assert.equal(typeof api.settings.saveSettings, 'function', 'settings API must be available');

const DEFAULT_SETTINGS = { theme: 'genesis', color_scheme: 'system', language: 'zh-Hans' };
invokeResponse = async (command, args) => command === 'web_access_update_settings'
  ? { ...DEFAULT_SETTINGS, language: (args && args.patch && args.patch.language) || DEFAULT_SETTINGS.language }
  : null;

async function withLanguage(language, run) {
  await api.settings.saveSettings({ language });
  try {
    await run();
  } finally {
    await api.settings.saveSettings({ language: DEFAULT_SETTINGS.language });
  }
}

const STALE_ID = 'removed-model';
const LOCALE_MARKERS = {
  en: { marker: 'no longer available', tail: 'pick a model again' },
  ja: { marker: '無効になりました', tail: '選び直し' },
  'zh-Hans': { marker: '已失效', tail: '重新选择模型' },
};

for (const [language, { marker, tail }] of Object.entries(LOCALE_MARKERS)) {
  await withLanguage(language, () => {
    // The exact rejection shape produced by the Web RPC client (bootstrap.js
    // wraps the backend message in new Error).
    const text = displayTurnError(new Error(`session_model_binding_stale:${STALE_ID}`));
    assert.match(text, new RegExp(marker), `${language}: stale-binding copy must be shown`);
    assert.match(text, new RegExp(tail), `${language}: recovery instruction must be shown`);
    assert.ok(text.includes(STALE_ID), `${language}: the missing model id must be carried`);
    assert.doesNotMatch(text, /^Error:/, `${language}: toString() prefix must not leak`);
    assert.ok(!text.includes('session_model_binding_stale'), `${language}: raw code must not leak`);

    // Plain strings (Tauri invoke rejections) take the same path.
    assert.equal(
      displayTurnError(`session_model_binding_stale:${STALE_ID}`),
      text,
      `${language}: Error-object and string rejections must localize identically`,
    );
  });
}

await withLanguage('en', () => {
  // image_input_unsupported had the same toString() blind spot before the
  // Error.message normalization; pin it so it cannot regress either.
  const text = displayTurnError(new Error('image_input_unsupported'));
  assert.match(text, /does not support images/, 'en: image-unsupported copy must be shown');
  assert.doesNotMatch(text, /^Error:/, 'en: toString() prefix must not leak');
});

await withLanguage('en', () => {
  assert.equal(
    displayTurnError(new Error('disk full')),
    'disk full',
    'unknown errors pass through (redactRawError keeps text when no classifier helper is loaded)',
  );
});

// Tauri counterpart audit pinned as a source contract: the display chain must
// derive its text the same way the concurrentTurn check does (Error.message
// first) and must match session_model_binding_stale before the raw-body
// redact fallback so the recovery copy is never swallowed.
const displaySection = chatSource.slice(
  chatSource.indexOf('// 稳定错误码'),
  chatSource.indexOf('addSystemItem(concurrentTurn'),
);
assert.match(displaySection, /String\(err && err\.message \? err\.message : err \|\| ""\)/);
assert.match(displaySection, /indexOf\("session_model_binding_stale"\) === 0/);
assert.ok(
  displaySection.indexOf('session_model_binding_stale') <
  displaySection.indexOf('redactRawError'),
  'stable-code localization must run before the redact fallback',
);

console.log('turn_error_display_contract: ok');
