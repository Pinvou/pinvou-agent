#!/usr/bin/env node
// 通知开关契约：三语 key、主开关派生显示、payload 形状与 Rust 侧默认值同步。
// global_shortcut_settings.test.mjs 是同类先例。
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
// 「需要输入」行的生效显示从主开关派生：主开关关闭时不得显示为开、也不得可点
// （Rust 侧投递门槛是 enabled && needs_input，否则出现"显示开着却永不触发"的死开关）。
assert.match(settingsView, /checked=\{inputNeededNotif && taskCompletedNotif\}/);
assert.match(settingsView, /disabled=\{!taskCompletedNotif\}/);
assert.ok(
  settingsView.indexOf('taskNotice') < settingsView.indexOf('inputNotice'),
  'the input row must render in the same section, after the master row',
);
assert.ok(
  settingsView.indexOf('canConfigureDesktopNotifications &&') < settingsView.indexOf('inputNotice'),
  'the input row must live inside the canConfigureDesktopNotifications gate',
);

const mainApp = stripComments(source('app/main.jsx'));
// Payload 形状即字段级合并契约：任务完成开关携带主开关（enabled），
// 「需要输入」只携带自己的开关——整表替换会复位未携带的开关。
assert.match(mainApp, /notifications: payloadFor\(nextEnabled\)/);
assert.match(mainApp, /\(nextEnabled\) => \(\{ enabled: nextEnabled, task_completed: nextEnabled \}\)/);
assert.match(mainApp, /\(nextEnabled\) => \(\{ needs_input: nextEnabled \}\)/);
// 设置回填只恢复持久意图（不折叠 enabled）：主开关关→开后靠派生恢复已持久化的
// needs_input，折叠会让本会话永远显示关闭。
assert.match(mainApp, /setInputNeededNotif\(notifications\.needs_input !== false\)/);
// 两行都必须接到共用的保存管道。
assert.match(mainApp, /setTaskCompletedNotif=\{handleSetTaskCompletedNotif\}/);
assert.match(mainApp, /setInputNeededNotif=\{handleSetInputNeededNotif\}/);

// ── Rust sync: 前端初始默认必须对齐 NotificationPrefs::default ──
// （跨语言钉：needs_input 跟随平台主开关默认，Linux opt-in。）
const prefsSource = readFileSync(new URL('../src-tauri/src/platform/prefs/mod.rs', import.meta.url), 'utf8');
assert.match(
  prefsSource,
  /let enabled = !cfg!\(target_os = "linux"\);[\s\S]*?needs_input: enabled,/,
  'Rust needs_input default must follow the platform master switch',
);
