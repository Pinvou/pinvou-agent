import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import { dict } from './helpers/i18n-all.js';

const read = path => fs.readFileSync(new URL(path, import.meta.url), 'utf8');

test('native tray and UI dictionaries share explicit translations', () => {
  const exits = [];
  for (const language of ['zh', 'en', 'ja']) {
    // Native Rust strips this module wrapper before parsing the JSON literal.
    const module = read(`../src/shared/i18n/tray/${language}.js`).trim();
    assert.ok(module.startsWith('export const trayLabels = ') && module.endsWith(';'));
    const labels = JSON.parse(module.slice('export const trayLabels = '.length, -1));
    assert.deepEqual(dict[language].tray, labels);
    assert.ok(labels.open.trim());
    assert.ok(labels.exit.trim());
    exits.push(labels.exit);
  }
  assert.equal(new Set(exits).size, 3);
});

test('Windows tray lifecycle is wired into native close, activation and orderly exit', () => {
  const lib = read('../src-tauri/src/lib.rs');
  const adapter = read('../src-tauri/src/features/system_tray/platform/windows.rs');
  assert.match(lib, /system_tray::install\(app\.handle\(\), prefs\.language\)/);
  assert.match(lib, /\.on_window_event\(\|window, event\|\s*\{\s*crate::features::system_tray::on_window_event\(window, event\)/);
  assert.match(adapter, /Ok\(true\) => api\.prevent_close\(\)/);
  assert.match(adapter, /activate_main_window\(window, "system_tray"\)/);
  assert.match(adapter, /"tray-exit" => app\.exit\(0\)/);
  assert.match(lib, /RunEvent::Exit =>[\s\S]*system_tray::on_exit\(\)/);
  assert.match(lib, /prepare_app_restart[\s\S]*system_tray::before_restart\(app\)\.await/);
  assert.match(lib, /RunEvent::Exit =>[\s\S]*browser\.shutdown_on_exit\(\)[\s\S]*harvest_child_processes\(app\)/);
});

test('native lifecycle regressions have an effective Windows execution filter', () => {
  const workflow = read('../../.github/workflows/pr-check.yml');
  for (const filter of [
    'features::system_tray::',
    'features::codex_acp::shutdown::tests::',
    'features::codex_acp::platform::windows::shutdown_tests::',
  ]) {
    assert.ok(workflow.includes(`'${filter}' \\`), `${filter} must be in the execution loop`);
  }
});

test('tray feature is enabled only for Windows and language changes reach the menu', () => {
  const cargo = read('../src-tauri/Cargo.toml');
  assert.match(cargo, /\[target\.'cfg\(windows\)'\.dependencies\]\s*tauri = \{[^\n]*"tray-icon"/);
  const settings = read('../src-tauri/src/app/commands/settings.rs');
  assert.match(settings, /system_tray::refresh_language\(&app, prefs\.language\)/);
});
