import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const linuxSurface = readFileSync(
  path.join(
    projectRoot,
    'src-tauri',
    'src',
    'features',
    'browser',
    'platform',
    'linux_surface.rs',
  ),
  'utf8',
);
const nativeHost = readFileSync(
  path.join(projectRoot, 'src-tauri', 'src', 'features', 'browser', 'platform', 'host.rs'),
  'utf8',
);
const browserCommands = readFileSync(
  path.join(projectRoot, 'src-tauri', 'src', 'app', 'commands', 'browser.rs'),
  'utf8',
);

test('Linux GTK prepare/attach/show/hide return execution ACK instead of enqueue ACK', () => {
  assert.match(linuxSurface, /fn with_webview_result\(/);
  assert.match(linuxSurface, /let result = operation\(&native\);/);
  assert.match(linuxSurface, /sender\.send\(result\)/);
  assert.match(linuxSurface, /receive_webview_result\(&receiver, &state, action, GTK_DISPATCH_TIMEOUT\)/);
  for (const operation of ['prepare', 'attach', 'show', 'hide']) {
    const start = linuxSurface.indexOf(`pub(super) fn ${operation}`);
    const end = linuxSurface.indexOf('\npub(super) fn ', start + 1);
    const body = linuxSurface.slice(start, end < 0 ? undefined : end);
    assert.match(body, /with_webview_result\(/, `${operation} must wait for the GTK result`);
  }
});

test('timed-out queued GTK mutation is cancelled and show remains fail-closed', () => {
  assert.match(
    linuxSurface,
    /compare_exchange\([\s\S]{0,120}DISPATCH_PENDING,[\s\S]{0,80}DISPATCH_CANCELLED/,
  );
  assert.match(linuxSurface, /if !begin_webview_operation\(&closure_state\) \{\s*return;/);
  const showStart = linuxSurface.indexOf('pub(super) fn show');
  const hideStart = linuxSurface.indexOf('pub(super) fn hide', showStart);
  const show = linuxSurface.slice(showStart, hideStart);
  assert.match(show, /result\.map_err\(\|error\| \{[\s\S]*native\.hide\(\);/);
  assert.match(show, /hide_empty_overlay\(&fixed\)/);
});

test('host visibility state changes only after physical hide succeeds', () => {
  assert.match(nativeHost, /fn hide_workspace\([\s\S]{0,120}-> Result<\(\), String>/);
  assert.match(nativeHost, /fn hide_all\([\s\S]{0,160}-> Result<\(\), String>/);
  assert.match(nativeHost, /hide_all\(window\.app_handle\(\), &self\.workspaces\)[\s\S]{0,160}\?/);
  assert.match(nativeHost, /Some\(session_id\) => \{[\s\S]{0,180}hide_workspace\(app, workspace\)\?/);
  assert.match(nativeHost, /None => hide_all\(app, &self\.workspaces\)\?/);
  assert.match(nativeHost, /fn show_active_workspace[\s\S]{0,120}hide_workspace\(app, workspace\)\?/);
  assert.match(
    browserCommands,
    /pub fn browser_hide_native_surface[\s\S]{0,300}-> Result<\(\), String>[\s\S]{0,240}mgr\.hide_native_surface/,
  );
});
