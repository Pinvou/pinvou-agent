#!/usr/bin/env node
// Notification-switch contract: trilingual keys, master-derived display,
// payload shapes, and sync with the Rust-side defaults and delivery gate.
// global_shortcut_settings.test.mjs is the same-kind precedent.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dict } from './helpers/i18n-all.js'; // aggregate shim: browser entry must not import this (tests only)

// ── i18n: inputNotice / inputNoticeDesc exist in all three dictionaries ──
for (const language of ['zh', 'en', 'ja']) {
  const section = dict[language].uiSettings;
  assert.ok(section, `${language}.uiSettings must exist`);
  for (const key of ['inputNotice', 'inputNoticeDesc']) {
    assert.equal(typeof section[key], 'string', `${language}.uiSettings.${key} must be a localized string`);
    assert.ok(section[key].length > 0, `${language}.uiSettings.${key} must not be empty`);
  }
}

// ── Source pins (comment-stripped; positive pins only, over-stripping fails loudly) ──
const source = relative => readFileSync(new URL(`../src/${relative}`, import.meta.url), 'utf8');
const stripComments = text => text
  .replace(/\/\*[\s\S]*?\*\//g, comment => comment.replace(/[^\n]/g, ''))
  .split('\n')
  .map(line => line.replace(/\/\/.*$/, ''))
  .join('\n');

const settingsView = stripComments(source('features/settings/SettingsView.jsx'));
// The input row's effective display derives from the master switch: it must
// neither show on nor stay clickable while the master is off. The Rust
// delivery gate mirrors the same effective rule
// (enabled && task_completed && needs_input), so a CLI-authored split
// (enabled=true, task_completed=false — where an older Windows/macOS file
// also defaults needs_input to true) cannot keep delivering while the row
// displays off.
assert.match(settingsView, /checked=\{inputNeededNotif && taskCompletedNotif\}/);
assert.match(settingsView, /disabled=\{!taskCompletedNotif\}/);
// Anchor existence first: a bare indexOf comparison passes vacuously once
// its needle disappears (-1 < any positive index), so a renamed row or
// gate would silently stop pinning anything.
const masterRowAt = settingsView.indexOf('taskNotice');
const gateAt = settingsView.indexOf('canConfigureDesktopNotifications &&');
const inputRowAt = settingsView.indexOf('inputNotice');
assert.ok(masterRowAt >= 0, 'the master row (taskNotice) must exist for the ordering pin');
assert.ok(gateAt >= 0, 'the canConfigureDesktopNotifications gate must exist for the placement pin');
assert.ok(inputRowAt >= 0, 'the input row (inputNotice) must exist for the ordering pins');
assert.ok(
  masterRowAt < inputRowAt,
  'the input row must render in the same section, after the master row',
);
assert.ok(
  gateAt < inputRowAt,
  'the input row must live inside the canConfigureDesktopNotifications gate',
);

const mainApp = stripComments(source('app/main.jsx'));
// Payload shape is the field-level merge contract: the master switch carries
// enabled, the input switch carries only its own field — a whole-struct
// replace would reset unlisted switches.
assert.match(mainApp, /notifications: payloadFor\(nextEnabled\)/);
assert.match(mainApp, /\(nextEnabled\) => \(\{ enabled: nextEnabled, task_completed: nextEnabled \}\)/);
assert.match(mainApp, /\(nextEnabled\) => \(\{ needs_input: nextEnabled \}\)/);
// Settings backfill restores only the persisted intent (no enabled folding
// for the input switch): after the master goes off->on the display derivation
// restores the persisted needs_input; folding here would leave the row shown
// off for the whole session.
assert.match(mainApp, /setInputNeededNotif\(notifications\.needs_input !== false\)/);
// Both rows must be wired to the shared save pipeline.
assert.match(mainApp, /setTaskCompletedNotif=\{handleSetTaskCompletedNotif\}/);
assert.match(mainApp, /setInputNeededNotif=\{handleSetInputNeededNotif\}/);

// ── Rust sync: the frontend initial default must align with NotificationPrefs::default ──
// (Cross-language pin: needs_input follows the platform master-switch
// default; Linux opt-in.)
const prefsSource = readFileSync(new URL('../src-tauri/src/platform/prefs/mod.rs', import.meta.url), 'utf8');
assert.match(
  prefsSource,
  /let enabled = !cfg!\(target_os = "linux"\);[\s\S]*?needs_input: enabled,/,
  'Rust needs_input default must follow the platform master switch',
);

// ── Rust sync: the delivery gate must equal the UI's effective rule ──
// The Settings row shows needs_input && task_completed (and the task row
// itself backfills as enabled && task_completed), so the native gate must
// require all three fields; ordering them differently or dropping one would
// desynchronize display and delivery.
const gateSource = readFileSync(new URL('../src-tauri/src/platform/notifications.rs', import.meta.url), 'utf8');
assert.match(
  gateSource,
  /pub fn needs_input_enabled\(\) -> bool \{[\s\S]*?prefs\.notifications\.enabled[\s\S]*?prefs\.notifications\.task_completed[\s\S]*?prefs\.notifications\.needs_input[\s\S]*?\n\}/,
  'input delivery must require enabled && task_completed && needs_input (the UI effective rule)',
);
// The sent copy must resolve per saved UI language (zh/en/ja), not hardcode
// one locale on every delivery path.
assert.match(
  gateSource,
  /fn input_needed_copy\(language: Language\)[\s\S]*?Language::ZhHans[\s\S]*?Language::En[\s\S]*?Language::Ja/,
  'input-needed copy must be keyed by the saved UI language',
);
