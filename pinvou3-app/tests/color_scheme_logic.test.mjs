import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  COLOR_SCHEME_STORAGE_KEY,
  normalizeColorScheme,
  resolveTheme,
  systemPrefersDark,
} from '../src/shared/color-scheme.js';

// 产品口径:首次安装跟随系统;判不出系统偏好按浅色;显式 light/dark 不再跟随系统。

test('normalizeColorScheme maps explicit light/dark and normalizes everything else to system', () => {
  assert.equal(normalizeColorScheme('light'), 'light');
  assert.equal(normalizeColorScheme('dark'), 'dark');
  assert.equal(normalizeColorScheme('system'), 'system');
  // 旧档缺失字段、后端兜底对象、未来未知档位都归跟随系统。
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
  // 未知值与 system 同口径:判不出偏好即跟随系统 → 判不出系统 → 浅色。
  assert.equal(resolveTheme(undefined, false), 'light');
  assert.equal(resolveTheme(undefined, true), 'dark');
});

test('resolveTheme detects the system live when systemDark is omitted', () => {
  const original = globalThis.window;
  try {
    // 无 window(matchMedia 不可用)→ 浅色兜底。
    delete globalThis.window;
    assert.equal(resolveTheme('system'), 'light');
    assert.equal(systemPrefersDark(), false);

    // 有 matchMedia:按查询结果判定。
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
