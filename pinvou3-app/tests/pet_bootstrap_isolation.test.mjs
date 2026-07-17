import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const bridgeSource = readFileSync(new URL('../src/tauri-bridge.js', import.meta.url), 'utf8');
const indexSource = readFileSync(new URL('../src/index.html', import.meta.url), 'utf8');
const petIndexSource = readFileSync(new URL('../src/pet.html', import.meta.url), 'utf8');
const petMainSource = readFileSync(new URL('../src/pet-main.jsx', import.meta.url), 'utf8');
const petMenuIndexSource = readFileSync(new URL('../src/pet-menu.html', import.meta.url), 'utf8');
const petMenuMainSource = readFileSync(new URL('../src/pet-menu-main.js', import.meta.url), 'utf8');
const mainSource = readFileSync(new URL('../src/main.jsx', import.meta.url), 'utf8');
const rustPetWindow = readFileSync(new URL('../src-tauri/src/pet_window.rs', import.meta.url), 'utf8');
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
assert.equal(typeof window.TauriBridge?.renderMarkdown, 'function');
assert.equal(window.TauriBridge?.init, undefined);
assert.match(
  indexSource,
  /const isPetWindow = new URLSearchParams\(window\.location\.search\)\.get\('window'\) === 'pet';[\s\S]*?if \(isPetWindow\) return;/,
  'index boot work must return before main-window probes and Bridge initialization',
);
assert.match(petIndexSource, /src="\/pet-main\.jsx"/);
assert.match(petMainSource, /allowResize:\s*false/);
assert.match(petMainSource, /scale:\s*0\.5/);
assert.match(
  petMainSource,
  /<PetWindow[\s\S]{0,120}?allowResize=\{PET_WINDOW_CONFIG\.allowResize\}[\s\S]{0,120}?configuredScale=\{PET_WINDOW_CONFIG\.scale\}/,
);
assert.doesNotMatch(
  petIndexSource,
  /tauri-bridge|tailwind|personas-i18n|update-notice|src="\/main\.jsx"/i,
);
assert.match(petMenuIndexSource, /src="\/pet-menu-main\.js"/);
assert.doesNotMatch(petMenuIndexSource, /tauri-bridge|tailwind|src="\/main\.jsx"/i);
assert.match(petMenuMainSource, /addEventListener\('blur'/);
assert.match(petMenuMainSource, /classList\.add\('pet-menu-hidden'\)/);
assert.match(petMenuMainSource, /invoke\('set_pet_enabled', \{ enabled: false \}\)/);
assert.doesNotMatch(mainSource, /import PetWindow|window'\) === 'pet'/);
assert.match(rustPetWindow, /WebviewUrl::App\("pet\.html"\.into\(\)\)/);

console.log('pet bootstrap isolation tests passed');
