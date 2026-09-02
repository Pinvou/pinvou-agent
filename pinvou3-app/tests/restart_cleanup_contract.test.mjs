import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.resolve(here, '..');
const rustRoot = path.join(appRoot, 'src-tauri', 'src');

function source(relativePath) {
  return fs.readFileSync(path.join(rustRoot, relativePath), 'utf8');
}

test('every direct Tauri restart passes through the unified cleanup funnel', () => {
  const settings = source(path.join('app', 'commands', 'settings.rs'));
  const updater = source(path.join('features', 'updater', 'mod.rs'));
  const combined = `${settings}\n${updater}`;

  assert.equal((combined.match(/app\.restart\(\);/g) || []).length, 3);
  assert.equal((combined.match(/crate::prepare_app_restart\(&app\)\.await;/g) || []).length, 3);
  assert.doesNotMatch(combined, /crate::harvest_child_processes\(&app\)\.await;[\s\S]{0,80}app\.restart\(\);/);
});

test('restart cleanup closes browser before harvesting child processes', () => {
  const lib = source('lib.rs');
  const browser = source(path.join('features', 'browser', 'mod.rs'));
  const helper = lib.match(
    /pub\(crate\) async fn prepare_app_restart[\s\S]*?\n\}/,
  )?.[0] || '';

  assert.match(helper, /browser\.shutdown_before_restart\(\)\.await;/);
  assert.match(helper, /harvest_child_processes\(app\)\.await;/);
  assert.ok(
    helper.indexOf('browser.shutdown_before_restart().await;')
      < helper.indexOf('harvest_child_processes(app).await;'),
    'browser recovery state and native surfaces must close before child harvesting',
  );
  const restartShutdown = browser.slice(
    browser.indexOf('pub async fn shutdown_before_restart'),
    browser.indexOf('pub fn shutdown_on_exit'),
  );
  assert.ok(
    restartShutdown.indexOf('self.hosted_request_gate.write().await')
      < restartShutdown.indexOf('self.start_mtx.lock().await'),
    'restart must use the same hosted-request -> start lock order as Prepare',
  );
  assert.match(restartShutdown, /platform::begin_browser_core_process_shutdown\(\);/);
  assert.ok(
    restartShutdown.indexOf('surface.close_preserving_restore')
      < restartShutdown.indexOf('platform::shutdown_browser_core_for_stop().await'),
    'the final gated WebDriver reset must follow restore persistence and native close',
  );
  const requestScanner = browser.slice(
    browser.indexOf('async fn prepare_requested_native_surfaces_filtered'),
    browser.indexOf('async fn process_hosted_cancellation'),
  );
  assert.match(requestScanner, /self\.hosted_request_gate\.read\(\)\.await/);
  assert.match(requestScanner, /self\.shutting_down\.load/);

  for (const entry of ['prepare_native_workspace', 'create_popup_tab', 'create_tab']) {
    const body = browser.slice(
      browser.indexOf(`fn ${entry}`),
      browser.indexOf('\n    }', browser.indexOf(`fn ${entry}`)) + 6,
    );
    assert.match(body, /self\.start_mtx\.lock\(\)\.await/);
    assert.match(body, /self\.ensure_accepting_browser_work\(\)\?/);
  }

  for (const [start, end] of [
    ['pub async fn stop(&self)', 'async fn stop_with_start_lock'],
    ['pub async fn stop_for_session', 'pub async fn delete_for_session'],
    ['async fn restore_saved_workspace(', 'async fn restore_saved_workspace_with_start_lock'],
  ]) {
    const body = browser.slice(browser.indexOf(start), browser.indexOf(end, browser.indexOf(start)));
    assert.ok(
      body.indexOf('self.start_mtx.lock().await')
        < body.indexOf('self.ensure_accepting_browser_work()?'),
      `${start} must recheck shutdown admission after acquiring start_mtx`,
    );
  }

  const ensureStarted = browser.slice(
    browser.indexOf('pub async fn ensure_started'),
    browser.indexOf('pub async fn stop(&self)'),
  );
  assert.ok(
    ensureStarted.indexOf('self.start_mtx.lock().await')
      < ensureStarted.indexOf('self.ensure_accepting_browser_work()?'),
    'queued automation reconnect must recheck shutdown admission after start_mtx',
  );

  const linuxAutomation = source(path.join('features', 'browser', 'platform', 'linux_automation.rs'));
  assert.match(linuxAutomation, /static PROCESS_SHUTTING_DOWN: AtomicBool/);
  assert.match(
    linuxAutomation,
    /run_if_active[\s\S]*?self\.lock\.lock\(\)\.await[\s\S]*?shutting_down\.load/,
  );
  assert.match(
    linuxAutomation,
    /run_active[\s\S]*?self\.operations[\s\S]*?run_if_active\(&PROCESS_SHUTTING_DOWN/,
  );
  assert.match(
    lib,
    /RunEvent::Exit[\s\S]*?shutdown_browser_before_process_end\(app\);[\s\S]*?block_on\(harvest_child_processes\(app\)\)/,
  );
});
