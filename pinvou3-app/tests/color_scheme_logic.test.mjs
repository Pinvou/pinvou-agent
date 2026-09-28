import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';

import {
  COLOR_SCHEME_STORAGE_KEY,
  normalizeColorScheme,
  resolveTheme,
  systemPrefersDark,
} from '../src/shared/color-scheme.js';

// Product rules: fresh installs follow the system; an undeterminable system
// preference means light; explicit light/dark stop following the system.

test('normalizeColorScheme maps explicit light/dark and normalizes everything else to system', () => {
  assert.equal(normalizeColorScheme('light'), 'light');
  assert.equal(normalizeColorScheme('dark'), 'dark');
  assert.equal(normalizeColorScheme('system'), 'system');
  // Missing legacy fields, bridge fallback objects, and unknown future values
  // all normalize to following the system.
  for (const value of [undefined, null, '', 'unknown', 'genesis']) {
    assert.equal(normalizeColorScheme(value), 'system', `value: ${String(value)}`);
  }
});

test('resolveTheme maps explicit schemes without consulting the system', () => {
  assert.equal(resolveTheme('light', true), 'light');
  assert.equal(resolveTheme('dark', false), 'dark');
});

test('resolveTheme follows system for the system preference, light when undeterminable', () => {
  assert.equal(resolveTheme('system', true), 'dark');
  assert.equal(resolveTheme('system', false), 'light');
  // Unknown values share the system semantics: undeterminable preference
  // means follow the system → undeterminable system → light.
  assert.equal(resolveTheme(undefined, false), 'light');
  assert.equal(resolveTheme(undefined, true), 'dark');
});

test('resolveTheme detects the system live when systemDark is omitted', () => {
  const original = globalThis.window;
  try {
    // No window (matchMedia unavailable) → light fallback.
    delete globalThis.window;
    assert.equal(resolveTheme('system'), 'light');
    assert.equal(systemPrefersDark(), false);

    // matchMedia present: decide by the query result.
    globalThis.window = {
      matchMedia: (query) => ({
        matches: query === '(prefers-color-scheme: dark)',
      }),
    };
    assert.equal(resolveTheme('system'), 'dark');
    assert.equal(systemPrefersDark(), true);
  } finally {
    if (original === undefined) delete globalThis.window;
    else globalThis.window = original;
  }
});

test('web storage key stays stable: older builds wrote light/dark under the same key', () => {
  assert.equal(COLOR_SCHEME_STORAGE_KEY, 'pinvou.web.theme');
});

test('desktop theme selection persists the current OS snapshot before its React event arrives', () => {
  // Exercise the real App callback with a deliberately stale hook value. This
  // avoids relying on browser/CI timing when a media event races a user click.
  const source = readFileSync(new URL('../src/app/main.jsx', import.meta.url), 'utf8');
  const start = source.indexOf('      function handleSetTheme(scheme) {');
  const end = source.indexOf('      function handleSetSearchProvider(', start);
  assert.ok(start >= 0 && end > start, 'the settings callback boundary must exist');
  const handler = source.slice(start, end);
  const original = globalThis.window;
  try {
    for (const [scheme, currentDark, staleDark, expectedTheme] of [
      ['system', true, false, 'genesis'],
      ['system', false, true, 'liquid-light'],
      ['light', true, false, 'liquid-light'],
      ['dark', false, true, 'genesis'],
    ]) {
      globalThis.window = { matchMedia: () => ({ matches: currentDark }) };
      let selected;
      let saved;
      runInNewContext(`${handler}\nhandleSetTheme(scheme);`, {
        scheme,
        systemDark: staleDark,
        resolveTheme,
        isWeb: false,
        setColorScheme: (value) => { selected = value; },
        bridge: {
          available: true,
          settings: { saveSettings: (value) => { saved = { ...value }; } },
        },
      });
      assert.equal(selected, scheme);
      assert.deepEqual(saved, { theme: expectedTheme, color_scheme: scheme });
    }
  } finally {
    if (original === undefined) delete globalThis.window;
    else globalThis.window = original;
  }
});
