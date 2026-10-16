#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dict } from './helpers/i18n-all.js'; // aggregate shim: browser entry must not import this (tests only)
import {
  GLOBAL_SHORTCUT_DEFAULT_ACCELERATOR,
  foldGlobalShortcutDraft,
  formatGlobalShortcutForDisplay,
  isPlausibleGlobalShortcutAccelerator,
  normalizeGlobalShortcutDraft,
} from '../src/features/settings/globalShortcutSettings.mjs';

// ── Pure client-side pre-validation helper ────────────────────────────
// Rust (set_global_shortcut_config) stays authoritative; the helper only
// filters combinations that can never register before an IPC round-trip.

assert.equal(GLOBAL_SHORTCUT_DEFAULT_ACCELERATOR, 'CmdOrCtrl+Alt+P', 'fallback combo must match the settings.json default');

assert.equal(normalizeGlobalShortcutDraft('  Ctrl+Alt+P  '), 'Ctrl+Alt+P');
assert.equal(normalizeGlobalShortcutDraft(null), '');
assert.equal(normalizeGlobalShortcutDraft(), '');
assert.equal(normalizeGlobalShortcutDraft(42), '42');

const plausible = ['Ctrl+Alt+P', 'CmdOrCtrl+Alt+P', 'Cmd+Shift+K', 'Ctrl+P', 'super+X', ' ALT+q ', ' CTRL + ALT + P '];
for (const value of plausible) {
  assert.equal(isPlausibleGlobalShortcutAccelerator(value), true, `${JSON.stringify(value)} must pre-validate as plausible`);
}

const implausible = [
  '', '   ',            // empty
  'P', 'Ctrl', 'Shift', // single token
  'Ctrl+', '+P', 'Ctrl+Alt+', // dangling separator
  'Ctrl++P',            // empty middle token
  'Shift+Alt', 'Ctrl+Cmd', // modifier-only combo
  'Foo+P',              // no modifier
];
for (const value of implausible) {
  assert.equal(isPlausibleGlobalShortcutAccelerator(value), false, `${JSON.stringify(value)} must be rejected client-side`);
}

// ── Machine-canonical accelerator → friendly display form ─────────────
// The input holds display form; saving still sends the draft as-is and Rust
// parses friendly input, so only presentation maps tokens here.

assert.equal(formatGlobalShortcutForDisplay('control+alt+KeyP'), 'Ctrl+Alt+P');
assert.equal(formatGlobalShortcutForDisplay('alt+super+KeyP'), 'Alt+Cmd+P');
assert.equal(formatGlobalShortcutForDisplay('shift+ctrl+Digit3'), 'Shift+Ctrl+3');
// cmdorctrl is an alias Rust only resolves at parse time: until the first
// successful apply the persisted value IS the alias, and a platform-partial
// name would mislabel it on Windows/Linux — display it platform-neutrally.
assert.equal(formatGlobalShortcutForDisplay('cmdorctrl+alt+p'), 'Ctrl/Cmd+Alt+P');
assert.equal(formatGlobalShortcutForDisplay('CmdOrCtrl+Alt+P'), 'Ctrl/Cmd+Alt+P');
// Key/Digit prefixes are stripped even without modifiers; F-keys pass through.
assert.equal(formatGlobalShortcutForDisplay('KeyF9'), 'F9');
assert.equal(formatGlobalShortcutForDisplay('ctrl+KeyF9'), 'Ctrl+F9');
// Numpad/Keypad tokens are never key-stripped (the generic Key rule would
// mangle keypad5 into pad5).
assert.equal(formatGlobalShortcutForDisplay('ctrl+alt+keypad5'), 'Ctrl+Alt+Keypad5');
assert.equal(formatGlobalShortcutForDisplay('ctrl+NumpadAdd'), 'Ctrl+NumpadAdd');
// Unknown named keys (F-keys, Media*, Minus/Equal, ...) pass through untouched.
assert.equal(formatGlobalShortcutForDisplay('Ctrl+Minus'), 'Ctrl+Minus');
assert.equal(formatGlobalShortcutForDisplay('Media+Play'), 'Media+Play');
// Token mapping is case-insensitive and tolerates surrounding whitespace.
assert.equal(formatGlobalShortcutForDisplay('  CONTROL + Alt + keyp  '), 'Ctrl+Alt+P');
// Empty input formats to '' so callers can fall back to the default constant.
assert.equal(formatGlobalShortcutForDisplay(''), '');
assert.equal(formatGlobalShortcutForDisplay('   '), '');
assert.equal(formatGlobalShortcutForDisplay(null), '');
assert.equal(formatGlobalShortcutForDisplay(), '');

// ── Alias-spelling fold: same combo modulo case/whitespace ────────────
// String-level folding only (case + whitespace); token-level aliasing
// (keyp ≡ p) stays Rust's parser job. Used for the post-save draft sync
// decision, never for the value sent to the command.
assert.equal(foldGlobalShortcutDraft('  Ctrl + alt + p  '), foldGlobalShortcutDraft('ctrl+alt+p'));
assert.equal(foldGlobalShortcutDraft('CONTROL + Alt + KEYP'), foldGlobalShortcutDraft('control+alt+keyp'));
assert.notEqual(foldGlobalShortcutDraft('Ctrl+Alt+P'), foldGlobalShortcutDraft('Ctrl+Alt+F9'));

// ── i18n: every new uiSettings key exists in all three dictionaries, same type ──
const keys = [
  'globalShortcuts',
  'globalShortcutEnable',
  'globalShortcutEnableDesc',
  'globalShortcutAccelerator',
  'globalShortcutPlaceholder',
  'globalShortcutInvalid',
  'globalShortcutConflict',
  'globalShortcutUnsupportedDesc',
  'globalShortcutSaveFailed',
];
for (const language of ['zh', 'en', 'ja']) {
  const section = dict[language].uiSettings;
  assert.ok(section, `${language}.uiSettings must exist`);
  for (const key of keys) {
    assert.equal(typeof section[key], 'string', `${language}.uiSettings.${key} must be a localized string`);
    assert.ok(section[key].length > 0, `${language}.uiSettings.${key} must not be empty`);
  }
}
// zh baseline must be mirrored by en and ja (missing keys render literal `undefined`).
for (const key of keys) {
  assert.equal(typeof dict.en.uiSettings[key], typeof dict.zh.uiSettings[key], `en.uiSettings.${key} type must match zh`);
  assert.equal(typeof dict.ja.uiSettings[key], typeof dict.zh.uiSettings[key], `ja.uiSettings.${key} type must match zh`);
}

// ── Source pins: section, bridge facade and capability default ────────
const source = relative => readFileSync(new URL(`../src/${relative}`, import.meta.url), 'utf8');
const settingsView = source('features/settings/SettingsView.jsx');
// Section renders and reads the UserPrefs snapshot fields.
assert.match(settingsView, /GlobalShortcutSettingSection/);
assert.match(settingsView, /copy\.globalShortcuts/);
assert.match(settingsView, /global_shortcut_enabled/);
assert.match(settingsView, /global_shortcut_accelerator/);
assert.match(settingsView, /setGlobalShortcutConfig/);
// The section must be reachable from the general tab render tree.
assert.match(settingsView, /<GlobalShortcutSettingSection t=\{t\} platformCapabilities=\{platformCapabilities\} \/>/);
// The accelerator input is programmatically labelled and pinned for smoke
// selectors (sibling convention: localPortPlaceholder input).
assert.match(settingsView, /aria-label=\{copy\.globalShortcutAccelerator\}/);
assert.match(settingsView, /data-testid="global-shortcut-accelerator"/);

const bridge = source('platform/tauri/bridge.js');
// Fail-closed capability default (Wayland reports the real value via get_platform_capabilities).
assert.match(bridge, /globalShortcutsAvailable: false/);
// Facade wiring: the settings feature function is destructured and exported on bridge.settings.
assert.match(bridge, /const setGlobalShortcutConfig = settingsFeature\.setGlobalShortcutConfig;/);
assert.match(bridge, /setGlobalShortcutConfig,\n {6}listBuiltinFeatures,/);

const bridgeSettings = source('platform/tauri/bridge/settings.js');
// The dedicated command owns persistence (never update_settings for this feature).
assert.match(bridgeSettings, /invoke\("set_global_shortcut_config"/);
assert.match(bridgeSettings, /setGlobalShortcutConfig,/);
// The snapshot mirror never regresses the accelerator to the raw input:
// Rust reports null (disabled/unsupported or unparsable draft) and disk
// keeps the existing combo.
assert.match(bridgeSettings, /typeof result\.accelerator === "string" && result\.accelerator/);

console.log('global_shortcut_settings: ok');
