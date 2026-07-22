import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const bridgeSource = readFileSync(new URL('../src/platform/tauri/bridge.js', import.meta.url), 'utf8');
const indexSource = readFileSync(new URL('../src/index.html', import.meta.url), 'utf8');
const petIndexSource = readFileSync(new URL('../src/pet.html', import.meta.url), 'utf8');
const petMainSource = readFileSync(new URL('../src/app/pet-main.jsx', import.meta.url), 'utf8');
const mainSource = readFileSync(new URL('../src/app/main.jsx', import.meta.url), 'utf8');
const rustPetWindow = readFileSync(new URL('../src-tauri/src/features/pet/pet_window.rs', import.meta.url), 'utf8');
const calls = { invoke: 0, listen: 0 };
const window = {
  location: { search: '?window=pet' },
  __TAURI__: {
    core: {
      invoke() {
        calls.invoke += 1;
        throw new Error('pet bootstrap must not invoke main-window commands');
      },
    },
    event: {
      listen() {
        calls.listen += 1;
        throw new Error('pet bootstrap must not register main-window listeners');
      },
    },
  },
};

vm.runInNewContext(bridgeSource, {
  window,
  URLSearchParams,
  console,
  setInterval() {
    throw new Error('pet bootstrap must not start main-window polling');
  },
  clearInterval,
  setTimeout,
  clearTimeout,
}, { filename: 'tauri-bridge.js' });

assert.deepEqual(calls, { invoke: 0, listen: 0 });
assert.equal(window.TauriBridge?.available, false);
assert.equal(typeof window.TauriBridge?.rendering?.renderMarkdown, 'function');
assert.equal(window.TauriBridge?.lifecycle, undefined);
assert.match(
  indexSource,
  /const isPetWindow = new URLSearchParams\(window\.location\.search\)\.get\('window'\) === 'pet';[\s\S]*?if \(isPetWindow\) return;/,
  'index boot work must return before main-window probes and Bridge initialization',
);
assert.match(petIndexSource, /src="\/app\/pet-main\.jsx"/);
assert.match(petMainSource, /allowResize:\s*false/);
assert.match(petMainSource, /scale:\s*0\.5/);
assert.match(petMainSource, /verticalAlignment:\s*query\.get\('verticalAlignment'\)/);
assert.match(
  petMainSource,
  /<PetWindow[\s\S]{0,160}?allowResize=\{PET_WINDOW_CONFIG\.allowResize\}[\s\S]{0,160}?configuredScale=\{PET_WINDOW_CONFIG\.scale\}[\s\S]{0,160}?configuredVerticalAlignment=\{PET_WINDOW_CONFIG\.verticalAlignment\}/,
);
assert.doesNotMatch(
  petIndexSource,
  /platform\/tauri\/bridge|tailwind|personas-i18n|update-notice|src="\/app\/main\.jsx"/i,
);
assert.doesNotMatch(mainSource, /import PetWindow|window'\) === 'pet'/);
assert.match(rustPetWindow, /pet\.html\?verticalAlignment=\{\}/);

console.log('pet bootstrap isolation tests passed');
