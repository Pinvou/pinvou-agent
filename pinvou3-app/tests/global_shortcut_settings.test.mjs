#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { dict } from './helpers/i18n-all.js'; // aggregate shim: browser entry must not import this (tests only)
import {
  GLOBAL_SHORTCUT_DEFAULT_ACCELERATOR,
  foldGlobalShortcutDraft,
  formatGlobalShortcutForDisplay,
  globalShortcutDraftToRust,
  isPlausibleGlobalShortcutAccelerator,
  normalizeGlobalShortcutDraft,
} from '../src/features/settings/globalShortcutSettings.mjs';

// ── Pure client-side pre-validation helper ────────────────────────────
// Rust (set_global_shortcut_config) stays authoritative; the helper only
// filters combinations that can never register before an IPC round-trip.
// The ≥1-modifier product policy is enforced on both sides: the Rust layer
// rejects bare keys too (pinned by single_key_without_modifier_is_rejected
// in features/global_shortcut.rs), this gate just explains them without an
// IPC round-trip.

assert.equal(GLOBAL_SHORTCUT_DEFAULT_ACCELERATOR, 'CmdOrCtrl+Alt+P', 'fallback combo must match the settings.json default');
// Cross-language default pin: the Rust-side default const is the same combo.
// (Neither side structurally sees the other; this read keeps a Rust-side
// default change from silently desyncing the frontend fallback.)
const prefsSource = readFileSync(new URL('../src-tauri/src/platform/prefs/mod.rs', import.meta.url), 'utf8');
assert.match(prefsSource, /DEFAULT_GLOBAL_SHORTCUT_ACCELERATOR: &str = "CmdOrCtrl\+Alt\+P";/, 'the Rust default accelerator must stay in sync with the frontend fallback');

assert.equal(normalizeGlobalShortcutDraft('  Ctrl+Alt+P  '), 'Ctrl+Alt+P');
assert.equal(normalizeGlobalShortcutDraft(null), '');
assert.equal(normalizeGlobalShortcutDraft(), '');
assert.equal(normalizeGlobalShortcutDraft(42), '42');

// All four CmdOrCtrl alias spellings the Rust parser accepts must
// pre-validate as the sole modifier (a missing spelling here would disable
// Save for Rust-valid pasted input).
const plausible = ['Ctrl+Alt+P', 'CmdOrCtrl+Alt+P', 'CommandOrControl+P', 'Cmd+Shift+K', 'Ctrl+P', 'super+X', ' ALT+q ', ' CTRL + ALT + P ', 'CmdOrControl+P', 'CommandOrCtrl+P'];
for (const value of plausible) {
  assert.equal(isPlausibleGlobalShortcutAccelerator(value), true, `${JSON.stringify(value)} must pre-validate as plausible`);
}

// ── Display form → Rust-parseable string ──────────────────────────────
// 'Ctrl/Cmd' is display-only (Rust cannot parse "CTRL/CMD"); the save path
// and the plausibility check must fold it back to the CmdOrCtrl alias so an
// edit of just the trailing key does not guarantee an invalid error.

assert.equal(globalShortcutDraftToRust('Ctrl/Cmd+Alt+O'), 'CmdOrCtrl+Alt+O');
assert.equal(globalShortcutDraftToRust('  ctrl/cmd + alt + o  '), 'CmdOrCtrl+alt+o');
assert.equal(globalShortcutDraftToRust('Ctrl / Cmd + Alt + O'), 'CmdOrCtrl+Alt+O', 'internal whitespace in the display token folds too');
assert.equal(globalShortcutDraftToRust('Ctrl+Alt+P'), 'Ctrl+Alt+P', 'non-display drafts pass through untouched');
assert.equal(isPlausibleGlobalShortcutAccelerator('Ctrl/Cmd+Alt+O'), true, 'the untouched-default display form with an edited key must be saveable');

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
// All four alias spellings map the same way.
assert.equal(formatGlobalShortcutForDisplay('cmdorctrl+alt+p'), 'Ctrl/Cmd+Alt+P');
assert.equal(formatGlobalShortcutForDisplay('CmdOrCtrl+Alt+P'), 'Ctrl/Cmd+Alt+P');
assert.equal(formatGlobalShortcutForDisplay('CmdOrControl+Alt+P'), 'Ctrl/Cmd+Alt+P');
assert.equal(formatGlobalShortcutForDisplay('CommandOrCtrl+Alt+P'), 'Ctrl/Cmd+Alt+P');
assert.equal(formatGlobalShortcutForDisplay('CommandOrControl+Alt+P'), 'Ctrl/Cmd+Alt+P');
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
// Shape pins must not match text inside comments: a guard commented out in
// the source would otherwise still satisfy every anchor (repo convention,
// ui_language_coverage.test.mjs). Block-comment contents are blanked with
// newlines preserved; `//` tails are dropped per line. All pins below are
// positive, where over-stripping can only fail loudly.
const stripComments = text => text
  .replace(/\/\*[\s\S]*?\*\//g, comment => comment.replace(/[^\n]/g, ''))
  .split('\n')
  .map(line => line.replace(/\/\/.*$/, ''))
  .join('\n');
const settingsView = stripComments(source('features/settings/SettingsView.jsx'));
// Section renders and reads the UserPrefs snapshot fields.
assert.match(settingsView, /GlobalShortcutSettingSection/);
assert.match(settingsView, /copy\.globalShortcuts/);
// Live disk-mirror state model: enabled derives from the snapshot (never a
// seeded local copy that can desync from disk).
assert.match(settingsView, /const enabled = prefs \? prefs\.global_shortcut_enabled === true : false;/);
// The disk-truth gate requires the shortcut fields: a failed get_settings
// leaves a truthy {theme, color_scheme, language} fallback in the slice,
// which must keep the section disabled instead of letting a toggle submit
// the default combo over disk (review round 5).
assert.match(settingsView, /const prefs = rawSettings && typeof rawSettings\.global_shortcut_enabled === 'boolean'\s*\?\s*rawSettings\s*:\s*null;/);
assert.match(settingsView, /global_shortcut_accelerator/);
assert.match(settingsView, /setGlobalShortcutConfig/);
// The section must be reachable from the general tab render tree.
assert.match(settingsView, /<GlobalShortcutSettingSection t=\{t\} platformCapabilities=\{platformCapabilities\} \/>/);
// The accelerator input is programmatically labelled and pinned for smoke
// selectors (sibling convention: localPortPlaceholder input).
assert.match(settingsView, /aria-label=\{copy\.globalShortcutAccelerator\}/);
assert.match(settingsView, /data-testid="global-shortcut-accelerator"/);
// The enable switch carries an accessible name too (role="switch" button;
// the row label alone is not a programmatic name).
assert.match(settingsView, /aria-label=\{copy\.globalShortcutEnable\}/);
// The section stays disabled until the settings snapshot exists: a toggle
// before the snapshot loads would submit the default combo over whatever
// disk holds.
assert.match(settingsView, /disabled=\{!prefs \|\| \(!available && !enabled\) \|\| pending\}/);
assert.match(settingsView, /disabled=\{!prefs \|\| !available \|\| pending\}/);
// Same synchronous single-flight as the ComputerUse section: the disabled
// attribute lags one render, so the ref must intercept inside the handler.
assert.match(settingsView, /if \(flightRef\.current\.busy\) return;/);
// The save path folds the display form back to a Rust-parseable string
// (Ctrl/Cmd → CmdOrCtrl); sending the raw display draft would guarantee an
// invalid error for edits of the untouched default.
assert.match(settingsView, /applyConfig\(enabled, globalShortcutDraftToRust\(draftAccelerator\), 'accelerator'\)/);
// A never-valid draft keeps Save enabled and explains itself on click
// (localPort-row pattern) instead of wedging the button silently.
assert.match(settingsView, /if \(!acceleratorPlausible\) \{/);
assert.match(settingsView, /setErrorSlot\('accelerator'\)/);
// Error-slot routing: outcome errors land in the owning row's slot — the
// ternary is unique to the .then outcome arm and the routing
// setErrorSlot(slot) must DIRECTLY follow it (a pin satisfied by the .catch
// arm's identical pair would let outcome errors route to the wrong row,
// review round 5); the unavailable-session notice lands in the toggle slot.
assert.match(settingsView, /setActionError\(status === 'invalid' \? copy\.globalShortcutInvalid\s*:\s*status === 'conflict' \? copy\.globalShortcutConflict\s*:\s*copy\.globalShortcutSaveFailed\);\s*setErrorSlot\(slot\);/);
assert.match(settingsView, /setErrorSlot\('toggle'\)/);
// Errors render in the toggle slot on unavailable sessions too: switching a
// residual shortcut off is the only operable action there, and a failed
// disable must be visible instead of console-only (review round 5).
assert.match(settingsView, /const toggleDesc = actionError && errorSlot === 'toggle'\s*\?\s*actionError\s*:\s*available \? copy\.globalShortcutEnableDesc : copy\.globalShortcutUnsupportedDesc;/);
// The draft follows the saved combo while untouched and the post-save resync
// folds it back in sync when it matches the submitted combo modulo
// case/whitespace/alias spelling — the phantom-dirty-flag fix. Each
// load-bearing piece is pinned so the machinery cannot be disabled silently
// (review round 5: disabling the resync with `if (false)` kept the suite
// green).
assert.match(settingsView, /if \(editedRef\.current\) return;/);
assert.match(settingsView, /const fold = foldGlobalShortcutDraft\(display\);\s*if \(fold === syncedFoldRef\.current\) return;\s*syncedFoldRef\.current = fold;\s*updateDraft\(display\);/);
assert.match(settingsView, /foldGlobalShortcutDraft\(draft\) === foldGlobalShortcutDraft\(formatGlobalShortcutForDisplay\(nextAccelerator\)\)/);
assert.match(settingsView, /editedRef\.current = false;\s*const display = formatGlobalShortcutForDisplay\(normalized\);\s*syncedFoldRef\.current = foldGlobalShortcutDraft\(display\);\s*updateDraft\(display\);/);
// The draft seeds from the SAVED combo, not the default, so a mount with a
// loaded snapshot never paints the default combo for a frame (review
// round 5).
assert.match(settingsView, /useState\(\(\) => formatGlobalShortcutForDisplay\(savedAccelerator\)\)/);
// Typing invalidates the accelerator slot's error (localPort-row pattern):
// a fixed-up draft must not keep showing the stale invalid copy (review
// round 5).
assert.match(settingsView, /if \(errorSlot === 'accelerator'\) setActionError\(''\);/);
// The toggle submits the SAVED combo (live mirror state), not the draft.
assert.match(settingsView, /onChange=\{value => applyConfig\(!!value, savedAccelerator, 'toggle'\)\}/);
// A persisted-on switch stays switchable-OFF on unavailable sessions (the
// Rust disable path is availability-independent); only turning ON requires a
// capable session.
assert.match(settingsView, /!available && !enabled/);
// The section is desktop-only: hiding it behind !isWeb is part of the
// bridge contract's desktop-only classification (review round 5).
assert.match(settingsView, /!\s*isWeb &&\s*\(\s*<GlobalShortcutSettingSection/);

const bridge = source('platform/tauri/bridge.js');
// Fail-closed capability default (Wayland reports the real value via get_platform_capabilities).
assert.match(bridge, /globalShortcutsAvailable: false/);
// Facade wiring: the settings feature function is destructured and exported on bridge.settings.
assert.match(bridge, /const setGlobalShortcutConfig = settingsFeature\.setGlobalShortcutConfig;/);
// \r?\n: a normal Windows checkout has CRLF line endings (core.autocrlf=true);
// a literal \n here would fail on every source line the pattern spans.
assert.match(bridge, /setGlobalShortcutConfig,\r?\n {6}listBuiltinFeatures,/);

const bridgeSettings = source('platform/tauri/bridge/settings.js');
// The dedicated command owns persistence (never update_settings for this feature).
assert.match(bridgeSettings, /invoke\("set_global_shortcut_config"/);
assert.match(bridgeSettings, /setGlobalShortcutConfig,/);
// The snapshot mirror follows every accelerator the response reports — a
// successful disabled save reports one too (it may have changed the stored
// combo); the string guard only shields against a response shape without it.
assert.match(bridgeSettings, /typeof result\.accelerator === "string" && result\.accelerator/);

// ── Behavioral: bridge mirror + toggle flow, over the real module ─────
// settings.js is a classic-script feature factory; evaluate it in a VM with
// a stubbed invoke and drive the review-round-3 scenario end to end: a save
// while the shortcut is DISABLED persists a new combo and the Rust response
// reports it, so the mirrors must follow — the later toggle reads the saved
// mirror and would otherwise re-submit the stale combo over this save.
const bridgeSettingsSource = readFileSync(new URL('../src/platform/tauri/bridge/settings.js', import.meta.url), 'utf8');

function createShortcutBridgeHarness({ respond }) {
  const state = { settings: { global_shortcut_enabled: false, global_shortcut_accelerator: 'control+alt+KeyP' } };
  const invoked = [];
  const context = vm.createContext({ window: {}, console });
  vm.runInContext(bridgeSettingsSource, context, { filename: 'bridge/settings.js' });
  const feature = context.window.__PINVOU_TAURI_BRIDGE_FEATURES__.settings({
    state,
    notify() {},
    listen() {},
    async invoke(command, args) {
      invoked.push([command, args]);
      if (command === 'set_global_shortcut_config') return respond(args);
      return null;
    },
  });
  assert.equal(typeof feature.setGlobalShortcutConfig, 'function', 'the settings feature must expose setGlobalShortcutConfig');
  return { state, invoked, feature };
}

// Faithful Rust-side stub for the inputs used: canonical display forms (like
// normalize_accelerator) and the fixed round-3 contract — a committed save
// reports the persisted accelerator, registered or disabled.
const CANONICAL = { 'ctrl+alt+p': 'control+alt+KeyP', 'ctrl+alt+o': 'control+alt+KeyO' };
const rustSuccessStub = (args) => {
  const accelerator = CANONICAL[String(args.accelerator || '').trim().toLowerCase()] || args.accelerator;
  return { status: args.enabled ? 'registered' : 'disabled', accelerator, message: null };
};

{
  const harness = createShortcutBridgeHarness({ respond: rustSuccessStub });
  // Disabled save of a new combo: the response carries the persisted combo,
  // and the bridge snapshot must land on it.
  await harness.feature.setGlobalShortcutConfig(false, 'Ctrl+Alt+O');
  assert.equal(harness.state.settings.global_shortcut_enabled, false);
  assert.equal(
    harness.state.settings.global_shortcut_accelerator,
    'control+alt+KeyO',
    'a disabled save that changed the combo must update the bridge mirror',
  );
  // The toggle path submits the saved accelerator: it must reach Rust as the
  // combo the save persisted, not the stale default.
  await harness.feature.setGlobalShortcutConfig(true, harness.state.settings.global_shortcut_accelerator);
  assert.equal(
    harness.invoked[1][1].accelerator,
    'control+alt+KeyO',
    'the later enable must submit the combo the disabled save persisted',
  );
  assert.equal(harness.state.settings.global_shortcut_enabled, true);
}

{
  // Retry-once contract: a failed invoke re-applies the same config (the
  // Rust apply is idempotent per enabled+accelerator), the retry's response
  // mirrors normally, and a persistently failing command resolves null with
  // the snapshot untouched so the caller rolls its toggle back.
  let attempts = 0;
  const flaky = createShortcutBridgeHarness({
    respond: (args) => {
      attempts += 1;
      if (attempts === 1) throw new Error('ipc hiccup');
      return rustSuccessStub(args);
    },
  });
  const retried = await flaky.feature.setGlobalShortcutConfig(true, 'Ctrl+Alt+O');
  assert.equal(attempts, 2, 'a failed invoke must be retried exactly once');
  assert.deepEqual(flaky.invoked[1][1], flaky.invoked[0][1], 'the retry must re-apply the identical config');
  assert.equal(retried && retried.status, 'registered', 'the retry response is the wrapper result');
  assert.equal(
    flaky.state.settings.global_shortcut_accelerator,
    'control+alt+KeyO',
    'a retry success must mirror into the snapshot like a first-try success',
  );

  const dead = createShortcutBridgeHarness({
    respond: () => { throw new Error('down'); },
  });
  const failed = await dead.feature.setGlobalShortcutConfig(true, 'Ctrl+Alt+O');
  assert.equal(failed, null, 'a persistent failure must resolve null so the caller rolls its toggle back');
  assert.equal(dead.state.settings.global_shortcut_enabled, false, 'a failed save must not touch the snapshot');
}

{
  // Cross-language regression guard, NOT a producible wire shape: the Rust
  // command persists on every registered/disabled outcome and answers with
  // the persisted accelerator, so a committed save without one cannot happen
  // today. The mirror must still not regress to the raw draft input if the
  // committed-persist arm ever regressed to echoing the bare outcome (the
  // Rust-side pin global_shortcut_persist_arm_reports_the_persisted_accelerator
  // guards the same contract from the other side of the boundary).
  const harness = createShortcutBridgeHarness({
    respond: () => ({ status: 'disabled', accelerator: null, message: 'global shortcuts are unavailable on this session' }),
  });
  await harness.feature.setGlobalShortcutConfig(false, 'Ctrl+Alt+O');
  assert.equal(
    harness.state.settings.global_shortcut_accelerator,
    'control+alt+KeyP',
    'a null-accelerator response must not write the raw draft into the mirror',
  );
  assert.equal(harness.state.settings.global_shortcut_enabled, false);
}

{
  // Producible failed-save shapes (review round 5): invalid/conflict carry
  // accelerator: null and persist NOTHING — the mirror must stay untouched
  // so the UI keeps describing the still-active state and the next toggle
  // re-submits what is actually active. The harness previously produced only
  // registered/disabled/throws, so a regression that mirrored failed saves
  // passed the whole suite.
  const conflict = createShortcutBridgeHarness({
    respond: () => ({ status: 'conflict', accelerator: null, message: 'owned by another app' }),
  });
  const conflictResult = await conflict.feature.setGlobalShortcutConfig(true, 'Ctrl+Alt+O');
  assert.equal(conflictResult && conflictResult.status, 'conflict', 'the conflict response passes through to the caller');
  assert.equal(conflict.state.settings.global_shortcut_accelerator, 'control+alt+KeyP', 'a conflict must not touch the snapshot');
  assert.equal(conflict.state.settings.global_shortcut_enabled, false, 'a conflict must not mirror the requested enabled');

  const invalidDraft = createShortcutBridgeHarness({
    respond: () => ({ status: 'invalid', accelerator: null, message: 'unparsable accelerator' }),
  });
  const invalidResult = await invalidDraft.feature.setGlobalShortcutConfig(true, 'Ctrl+Alt+O');
  assert.equal(invalidResult && invalidResult.status, 'invalid', 'the invalid response passes through to the caller');
  assert.equal(invalidDraft.state.settings.global_shortcut_accelerator, 'control+alt+KeyP', 'an invalid response must not touch the snapshot');
  assert.equal(invalidDraft.state.settings.global_shortcut_enabled, false, 'an invalid response must not mirror the requested enabled');
}

console.log('global_shortcut_settings: ok');
