import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';

const read = path => fs.readFileSync(new URL(path, import.meta.url), 'utf8');
const rust = path => read(`../src-tauri/src/${path}`).split('\n')
  .map(line => line.split('//')[0]).join('\n');

test('main-window presentation resolves the native parent when browser child webviews exist', () => {
  for (const path of [
    'features/system_tray/platform/windows.rs',
    'features/global_shortcut/platform/mod.rs',
    'platform/window_startup.rs',
    'features/pet/pet_window.rs',
    'features/pet/platform/detach.rs',
  ]) {
    const source = rust(path);
    assert.match(source, /get_window\("main"\)/, path);
    assert.doesNotMatch(source, /get_webview_window\("main"\)/, path);
  }
  assert.match(rust('lib.rs'), /single_instance::init\([\s\S]*?app\.get_window\("main"\)[\s\S]*?activate_main_window\(window, "single_instance"\)/);
});

test('native presentation retains restore order and platform activation policies', () => {
  const source = rust('platform/window_startup.rs');
  assert.match(source, /fn activate_main_window\(window: tauri::Window, source: &str\)/);
  assert.match(source, /window\.show\(\);\s*let _ = window\.unminimize\(\);\s*let _ = window\.set_focus\(\);/);
  assert.match(source, /should_defer_main_window_activation\(startup_ready, is_visible\)/);
  assert.match(source, /gtk_window\.show_all\(\);\s*gtk_window\.deiconify\(\);\s*gtk_window\.present\(\);/);
  const unhide = source.indexOf('app_handle().show()');
  const focus = source.indexOf('window.set_focus()');
  assert.ok(unhide >= 0 && focus > unhide);
  assert.match(rust('app/commands/startup.rs'), /reveal_startup_window\(window: tauri::Window\)/);
  for (const platform of ['linux', 'macos', 'standard']) {
    assert.match(rust(`features/pet/platform/${platform}.rs`), /fn finish_main_focus_raise\(window: &tauri::Window\)/);
  }
});

test('webview-specific setup retains its webview handle', () => {
  assert.match(rust('lib.rs'), /get_webview_window\("main"\)[\s\S]*?with_webview\(/);
});

test('voice and web-bridge commands stay callable while browser tabs attach child webviews', () => {
  // Issue 840: with an agent-browser child WebView attached to the main
  // window, Tauri's single-WebView `WebviewWindow` injection fails and
  // `webview_windows()` drops main entirely — which broke Alt-tap dictation,
  // the recording claim, microphone permission reset, and the Web access
  // bridge while a tab was open. The injected handles must stay `Webview`
  // and the voice router HWND cache must resolve main natively.
  for (const path of ['app/commands/voice.rs', 'app/commands/remote_control.rs']) {
    const source = rust(path);
    // Any by-value `WebviewWindow` command param breaks argument injection
    // once a browser tab attaches, whatever the param is called; `&`-borrowed
    // handles and `WebviewWindowBuilder` stay allowed.
    assert.doesNotMatch(source, /:\s*(?:tauri::)?WebviewWindow\b/, path);
  }
  assert.match(
    rust('app/commands/voice.rs'),
    /set_voice_shortcut_recording\(\s*webview: tauri::Webview,/,
  );
  assert.match(
    rust('app/commands/voice.rs'),
    /reset_microphone_permission\(webview: tauri::Webview\)/,
  );
  for (const [platform, param] of [
    ['windows', 'webview'],
    ['macos', 'webview'],
    ['linux', '_webview'],
    ['unsupported', '_webview'],
  ]) {
    assert.match(
      rust(`features/voice/platform/${platform}.rs`),
      new RegExp(`reset_microphone_permission\\(${param}: tauri::Webview\\)`),
      platform,
    );
  }
  for (const fn of [
    'web_access_enable',
    'web_access_disable',
    'web_access_status',
    'web_access_rotate',
    'web_access_revoke_host_workspace',
    'web_access_bridge_ready',
    'web_access_rpc_begin',
    'web_access_rpc_respond',
    'web_access_publish_event',
    'web_access_list_host_files',
  ]) {
    assert.match(
      rust('app/commands/remote_control.rs'),
      new RegExp(`pub fn ${fn}\\([^)]*?webview: Webview,`),
      fn,
    );
  }
  assert.match(
    rust('features/voice_shortcut/platform/mod.rs'),
    /refresh_router_window_hwnds[\s\S]*?app\.get_window\("main"\)/,
  );
});
