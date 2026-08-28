import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createPinvouBrowserCoreCatalog } from '../src-tauri/resources/common/bundle/mcp-servers/browser-core-protocol.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (...segments) => readFileSync(path.join(root, ...segments), 'utf8');
const readRepo = (...segments) => readFileSync(path.join(root, '..', ...segments), 'utf8');

const macos = read(
  'src-tauri',
  'src',
  'features',
  'browser',
  'platform',
  'macos.rs',
);
const linux = read(
  'src-tauri',
  'src',
  'features',
  'browser',
  'platform',
  'linux.rs',
);
const platform = read(
  'src-tauri',
  'src',
  'features',
  'browser',
  'platform',
  'mod.rs',
);
const system = read(
  'src-tauri',
  'src',
  'features',
  'browser',
  'platform',
  'system.rs',
);
const state = read(
  'src-tauri',
  'src',
  'features',
  'browser',
  'platform',
  'state.rs',
);
const core = read('src-tauri', 'src', 'features', 'browser', 'core.rs');
const cargo = read('src-tauri', 'Cargo.toml');
const capabilities = read('src-tauri', 'src', 'platform', 'capabilities.rs');
const extraction = read(
  'src-tauri',
  'src',
  'features',
  'runtime_bundle',
  'platform',
  'extraction.rs',
);
const browser = read('src-tauri', 'src', 'features', 'browser', 'mod.rs');
const nativeHost = read(
  'src-tauri',
  'src',
  'features',
  'browser',
  'platform',
  'host.rs',
);
const browserWrapper = read(
  'src-tauri',
  'resources',
  'common',
  'bundle',
  'mcp-servers',
  'browser-wrapper.mjs',
);
const main = read('src', 'app', 'main.jsx');
const normalMacBuildEntrypoints = [
  read('package.json'),
  read('scripts', 'tauri', 'build.js'),
  readRepo('.github', 'workflows', 'mac-build.yml'),
  readRepo('.github', 'workflows', 'release-packages.yml'),
  readRepo('scripts', 'release-macos.sh'),
].join('\n');

test('macOS BrowserCore evaluates async JSON in the exact WKWebView page world', () => {
  assert.match(platform, /#\[cfg\(target_os = "macos"\)\]\s*mod macos;/);
  assert.match(
    platform,
    /target_os = "macos"[\s\S]{0,320}macos::evaluate_json/,
  );
  assert.match(macos, /callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler/);
  assert.match(macos, /WKContentWorld::pageWorld/);
  assert.match(macos, /await \(async \(\) =>/);
  assert.match(macos, /JSON\.stringify/);
  assert.match(cargo, /objc2-web-kit[\s\S]{0,260}"WKWebView"/);
});

test('macOS publishes address state only from top-level commits, not redirects or iframe requests', () => {
  const policyHandler = nativeHost.slice(
    nativeHost.indexOf('.on_navigation'),
    nativeHost.indexOf('.on_page_load'),
  );
  const crossDocumentPolicyHandler = policyHandler.slice(
    policyHandler.indexOf('let binding_marker'),
  );
  const pageLoadHandler = nativeHost.slice(
    nativeHost.indexOf('.on_page_load'),
    nativeHost.indexOf('.on_document_title_changed'),
  );
  const committedUrlResolver = nativeHost.slice(
    nativeHost.indexOf('fn committed_top_level_url'),
    nativeHost.indexOf('fn resolve_surface_url'),
  );
  const validatedUrlResolver = nativeHost.slice(
    nativeHost.indexOf('fn validated_surface_url'),
    nativeHost.indexOf('fn committed_top_level_url'),
  );

  // The nonce-authenticated history signal may publish a same-document URL;
  // during a cross-document load it only advances that generation and waits
  // for Finished; ordinary policy/redirect callbacks still cannot publish.
  assert.match(policyHandler, /location_change_signal_nonce\(url\)/);
  assert.match(policyHandler, /navigation\.navigation_in_flight\(\)/);
  assert.match(policyHandler, /observe_same_document_during_load\(&live_url\)/);
  assert.match(policyHandler, /finish_same_document\(&live_url\)/);
  assert.doesNotMatch(
    crossDocumentPolicyHandler,
    /emit\("browser:(?:navigation|tabs-changed)"/,
  );
  assert.match(pageLoadHandler, /payload\.event\(\) != PageLoadEvent::Finished/);
  assert.match(pageLoadHandler, /payload\.event\(\) == PageLoadEvent::Started/);
  assert.match(pageLoadHandler, /observe_started\(&started_url\)/);
  assert.match(pageLoadHandler, /NavigationCommitDecision::Stale/);
  assert.match(
    pageLoadHandler,
    /committed_top_level_url\(\s*payload_url,\s*live_url\.as_deref\(\),\s*&committed_navigation_tab_token,/,
  );
  assert.doesNotMatch(pageLoadHandler, /has_internal_marker_for_token\(payload_url/);
  assert.match(pageLoadHandler, /is_browser_core_binding_url\(payload_url\)/);
  assert.match(pageLoadHandler, /emit\("browser:navigation", &payload\)/);
  assert.match(pageLoadHandler, /emit\("browser:tabs-changed", &payload\)/);
  assert.match(committedUrlResolver, /is_browser_core_binding_url\(payload_url\)/);
  assert.match(validatedUrlResolver, /sanitize_marker_url\(url, expected_tab_token\)/);
  assert.match(validatedUrlResolver, /internal_marker_token\(&url\)\.is_some\(\)/);
  assert.match(
    committedUrlResolver,
    /validated_surface_url\(payload_url\.to_string\(\), expected_tab_token\)\?/,
  );
  assert.match(
    committedUrlResolver,
    /live_url[\s\S]*\.filter\(\|url\| !super::is_browser_core_binding_url\(url\)\)/,
  );
  assert.match(
    committedUrlResolver,
    /validated_surface_url\(url\.to_string\(\), expected_tab_token\)/,
  );
  assert.match(committedUrlResolver, /\(payload_url == live_url\)\.then_some\(payload_url\)/);
});

test('macOS binding is host-authoritative and leaks no remote-page credential', () => {
  const registration = macos.slice(
    macos.indexOf('pub(super) fn register_webview_binding'),
    macos.indexOf('pub(super) fn unregister_webview_binding'),
  );
  assert.match(macos, /struct WebviewBinding[\s\S]{0,160}tab_token: String[\s\S]{0,160}Weak<WorkspaceControl>/);
  assert.match(registration, /Arc::downgrade\(control\)/);
  assert.doesNotMatch(registration, /session_id|lease|globalThis|evaluate|initialization_script/);
  assert.match(macos, /webview\.label\(\)/);
  assert.match(macos, /browser\/wkwebview-binding-not-registered/);
});

test('macOS trusted input stays app-scoped and unsupported gestures fail closed', () => {
  const nativeDispatch = macos.slice(
    macos.indexOf('async fn with_native_webview'),
    macos.indexOf('#[derive(Clone)]'),
  );
  assert.match(macos, /NSEvent::mouseEventWithType/);
  assert.match(macos, /NSEvent::keyEventWithType/);
  assert.match(macos, /hit_test_webview_local_point\([\s\S]{0,120}local_point/);
  assert.match(
    macos,
    /fn hit_test_webview_local_point[\s\S]{0,500}view\.superview\(\)[\s\S]{0,300}convertPoint_toView\(local_point, Some\(&\*superview\)\)[\s\S]{0,180}view\.hitTest\(point_in_superview\)/,
  );
  assert.doesNotMatch(macos, /view\s*\.hitTest\((?:local_point|centre)\)/);
  assert.match(macos, /target\.mouseDown\(&down\)/);
  assert.match(macos, /if !window\.makeFirstResponder/);
  assert.match(macos, /browser\/wkwebview-focus-rejected/);
  assert.match(macos, /browser_first_responder\(view, window\)/);
  assert.match(macos, /responder\.insertText\(object\)/);
  assert.match(macos, /first-responder-outside-surface/);
  assert.match(
    nativeDispatch,
    /authorize_agent_input_for_label\(\s*&label,\s*&authorization,\s*emits_takeover_signal,?\s*\)/,
  );
  assert.ok(
    nativeDispatch.indexOf('match authorize_agent_input_for_label(') <
      nativeDispatch.indexOf('operation(view, window)'),
    'the full host lease must be revalidated immediately before AppKit dispatch',
  );
  assert.doesNotMatch(
    macos,
    /CGEventPost|postEvent|mouseLocation\(|addGlobalMonitor|dispatchEvent|\.click\(\)|new MouseEvent/,
  );
  assert.match(macos, /trusted-input-gesture-unavailable-on-wkwebview/);
  assert.match(macos, /dialog-backend-unavailable-on-wkwebview/);
  const darwinCatalog = createPinvouBrowserCoreCatalog({ includeDialog: false });
  assert.equal(
    darwinCatalog.toolsListResult.tools.some((tool) => tool.name === 'handle_dialog'),
    false,
    'the macOS BrowserCore catalog must not advertise its unsupported dialog tool',
  );
  assert.match(
    browserWrapper,
    /includeDialog: process\.platform !== 'darwin'/,
    'the host-core catalog must wire the macOS platform to the dialog filter',
  );
  assert.match(
    platform,
    /target_os = "macos"[\s\S]{0,260}macos::handle_dialog\(webview, authorization, action, prompt_text\)/,
  );
  const dialog = macos.slice(
    macos.indexOf('pub(super) async fn handle_dialog'),
    macos.indexOf('async fn resolve_element_point'),
  );
  assert.match(dialog, /authorization: &NativeTabLease/);
  assert.match(dialog, /authorize_agent_input_for_label\(webview\.label\(\), authorization, false\)\?/);
  assert.ok(
    dialog.indexOf('authorize_agent_input_for_label') <
      dialog.indexOf('dialog-backend-unavailable-on-wkwebview'),
    'the unsupported dialog route must validate the current task lease without emitting input',
  );
});

test('macOS native input timeout cancels only before AppKit dispatch', () => {
  const nativeDispatch = macos.slice(
    macos.indexOf('async fn with_native_webview'),
    macos.indexOf('fn authorize_agent_input_for_label'),
  );
  assert.match(platform, /struct AsyncDispatchState/);
  assert.match(platform, /const ASYNC_DISPATCH_PENDING: u8 = 0/);
  assert.match(platform, /const ASYNC_DISPATCH_RUNNING: u8 = 1/);
  assert.match(platform, /const ASYNC_DISPATCH_CANCELLED: u8 = 2/);
  assert.match(platform, /const ASYNC_DISPATCH_FINISHED: u8 = 3/);
  assert.match(
    platform,
    /compare_exchange\([\s\S]{0,160}ASYNC_DISPATCH_PENDING,[\s\S]{0,160}ASYNC_DISPATCH_RUNNING/,
  );
  assert.match(
    platform,
    /compare_exchange\([\s\S]{0,160}ASYNC_DISPATCH_PENDING,[\s\S]{0,160}ASYNC_DISPATCH_CANCELLED/,
  );
  assert.match(
    nativeDispatch,
    /if !callback_state\.begin\(\) \{\s*return;\s*\}/,
  );
  assert.match(nativeDispatch, /browser\/wkwebview-input-timeout/);
  assert.match(nativeDispatch, /browser\/wkwebview-input-callback-closed/);
  assert.match(nativeDispatch, /Some\(ACTION_COMMIT_UNKNOWN_INPUT_INTERRUPTION\)/);
  assert.ok(
    nativeDispatch.indexOf('callback_state.begin()') <
      nativeDispatch.indexOf('authorize_agent_input_for_label(') &&
      nativeDispatch.indexOf('authorize_agent_input_for_label(') <
      nativeDispatch.indexOf('operation(view, window)'),
    'a cancelled callback must not authorize or run an AppKit operation',
  );
});

test('macOS native input waits for the WebContent process and emits complete navigation-key flags', () => {
  const nativeDispatch = macos.slice(
    macos.indexOf('async fn with_native_webview'),
    macos.indexOf('fn authorize_agent_input_for_label'),
  );
  const keyParser = macos.slice(
    macos.indexOf('impl KeyStroke'),
    macos.indexOf('fn key_code_for_character'),
  );

  assert.match(nativeDispatch, /WKWebView forwards AppKit responder calls to its WebContent process/);
  assert.match(
    nativeDispatch,
    /dispatch_state[\s\S]{0,500}\.await\?;[\s\S]{0,500}evaluate_json\([\s\S]{0,240}"return true;"/,
  );
  assert.match(nativeDispatch, /ACTION_COMMIT_UNKNOWN_INPUT_INTERRUPTION/);
  assert.match(keyParser, /NSEventModifierFlags::Function/);
  assert.match(keyParser, /NSEventModifierFlags::NumericPad/);
  assert.match(keyParser, /"end" => function\(NSEndFunctionKey, 119, false\)/);
  assert.match(keyParser, /"arrowleft" \| "left" => function\(NSLeftArrowFunctionKey, 123, true\)/);
});

test('macOS fill selects through the scoped native responder even while Pinvou is non-key', () => {
  const fill = macos.slice(
    macos.indexOf('pub(super) async fn fill_element'),
    macos.indexOf('pub(super) async fn type_text'),
  );
  const selectAll = macos.slice(
    macos.indexOf('async fn select_all'),
    macos.indexOf('async fn dispatch_key'),
  );

  assert.match(fill, /select_all\(webview, authorization\)\.await/);
  assert.doesNotMatch(fill, /KeyStroke::command|performKeyEquivalent/);
  assert.match(selectAll, /with_native_webview\(webview, authorization, false/);
  assert.match(selectAll, /browser_content_focus_target\(view\)/);
  assert.match(selectAll, /with_temporary_browser_focus/);
  assert.match(selectAll, /responder\.selectAll\(None\)/);
});

test('BrowserCore marks only dispatched evaluate_script interruptions commit-unknown', () => {
  const mutatingBranch = core.slice(
    core.indexOf('"evaluate_script" =>'),
    core.indexOf('"handle_dialog" =>'),
  );
  const macEvaluation = macos.slice(
    macos.indexOf('pub(super) async fn evaluate_json'),
    macos.indexOf('fn wrap_json_evaluation'),
  );
  const linuxEvaluation = linux.slice(
    linux.indexOf('pub(super) async fn evaluate_json'),
    linux.indexOf('pub(super) async fn bind_webview'),
  );

  assert.match(platform, /enum BrowserCoreEvaluationMode \{[\s\S]*ReadOnly,[\s\S]*MayMutate,/);
  assert.match(
    core,
    /async fn call\([\s\S]{0,400}BrowserCoreEvaluationMode::ReadOnly/,
  );
  assert.match(
    core,
    /async fn call_mutating\([\s\S]{0,240}authorization: &NativeTabLease[\s\S]{0,500}BrowserCoreEvaluationMode::MayMutate,[\s\S]{0,100}Some\(authorization\)/,
  );
  assert.match(mutatingBranch, /call_mutating\(webview, authorization, "evaluate"/);
  assert.match(mutatingBranch, /committed_platform_outcome\("Script evaluation", &error\)/);
  assert.equal(
    (core.match(/call_mutating\(webview, authorization, "evaluate"/g) || []).length,
    1,
    'only evaluate_script may opt into mutating script commit semantics',
  );

  for (const [name, evaluation, nativeCall] of [
    ['WKWebView', macEvaluation, 'callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler'],
    ['WebKitGTK', linuxEvaluation, 'call_async_javascript_function'],
  ]) {
    assert.match(
      evaluation,
      /let dispatch = \|\| \{\s*if !callback_state\.begin\(\) \{\s*return Ok\(\(\)\);\s*\}/,
      name,
    );
    assert.match(evaluation, /authorization: Option<&NativeTabLease>/, name);
    assert.match(evaluation, /evaluation_authorization\(mode, authorization\)/, name);
    assert.match(evaluation, /dispatch_script_mutation_if_authorized/, name);
    assert.ok(
      evaluation.indexOf('callback_state.begin()') < evaluation.indexOf(nativeCall),
      `${name} must cross the shared dispatch boundary before starting page JavaScript`,
    );
    assert.match(
      evaluation,
      new RegExp(`let dispatch = \\|\\| \\{[\\s\\S]*${nativeCall}[\\s\\S]*dispatch_script_mutation_if_authorized\\([\\s\\S]*dispatch`),
      `${name} must pass the native JavaScript enqueue closure through final host authorization`,
    );
    assert.match(evaluation, /BrowserCoreEvaluationMode::MayMutate/, name);
    assert.match(evaluation, /ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION/, name);
  }

  assert.match(core, /ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION/);
  assert.match(core, /"retryable": false/);
});

test('hosted navigation carries the exact lease through generation begin and native enqueue', () => {
  for (const method of [
    'navigate_tab_for_agent',
    'history_step_tab_for_agent',
    'reload_tab_for_agent',
  ]) {
    const start = nativeHost.indexOf(`pub fn ${method}`);
    assert.notEqual(start, -1, method);
    const body = nativeHost.slice(start, nativeHost.indexOf('\n    pub fn ', start + 1));
    assert.match(body, /authorization: &NativeTabLease/, method);
    assert.match(body, /dispatch_agent_tab_action\(/, method);
    assert.ok(
      body.indexOf('dispatch_agent_tab_action(') < body.indexOf('begin_external_navigation('),
      `${method} must begin its generation inside the final authorization seam`,
    );
  }
  assert.match(
    nativeHost,
    /fn dispatch_agent_tab_action[\s\S]{0,800}dispatch_if_agent_authorized\(authorization, dispatch\)/,
  );
  assert.match(
    browser,
    /navigate_tab_for_agent\([\s\S]{0,240}&authorization/,
    'initial blank reuse must pass its exact begun lease',
  );
});

test('macOS native input scopes firstResponder and never focuses a hidden task surface', () => {
  const focusScope = macos.slice(
    macos.indexOf('fn with_temporary_browser_focus'),
    macos.indexOf('async fn with_native_webview'),
  );
  const mouseDispatch = macos.slice(
    macos.indexOf('async fn dispatch_mouse_click'),
    macos.indexOf('fn mouse_event'),
  );
  const textDispatch = macos.slice(
    macos.indexOf('async fn insert_text'),
    macos.indexOf('async fn dispatch_key'),
  );
  const keyDispatch = macos.slice(
    macos.indexOf('async fn dispatch_key_event'),
    macos.indexOf('/// Return a responder'),
  );

  assert.match(mouseDispatch, /with_temporary_browser_focus\(view, window, focus_target/);
  assert.match(textDispatch, /browser_content_focus_target\(view\)\?/);
  assert.match(textDispatch, /with_temporary_browser_focus\(view, window, focus_target/);
  assert.match(keyDispatch, /browser_content_focus_target\(view\)\?/);
  assert.match(keyDispatch, /with_temporary_browser_focus\(view, window, focus_target/);
  assert.match(
    macos,
    /if !std::ptr::eq\(&\*target, webview_view\) && !target\.isDescendantOf\(webview_view\)/,
  );
  assert.match(macos, /content-hit-test-escaped-surface/);

  assert.match(focusScope, /if let Err\(error\) = ensure_interactive_webview\(view, window\)/);
  assert.match(focusScope, /view\s*\.window\(\)/);
  assert.match(focusScope, /!window\.isVisible\(\) \|\| window\.isMiniaturized\(\)/);
  assert.match(focusScope, /view\.isHiddenOrHasHiddenAncestor\(\)/);
  assert.match(focusScope, /browser\/wkwebview-surface-hidden/);
  assert.match(focusScope, /view\.visibleRect\(\)/);
  assert.match(focusScope, /browser\/wkwebview-surface-clipped/);

  const savedAt = focusScope.indexOf('let previous = window.firstResponder()');
  const focusedAt = focusScope.indexOf('window.makeFirstResponder(Some(focus_target))');
  const verifiedAt = focusScope.indexOf('browser_first_responder(view, window)');
  const operatedAt = focusScope.indexOf('operation(&actual)');
  const restoredAt = focusScope.indexOf('restore_first_responder(view, window, previous.as_deref())');
  assert.ok(savedAt >= 0 && savedAt < focusedAt, 'the prior AppKit responder must be retained before temporary focus');
  assert.ok(focusedAt < verifiedAt, 'makeFirstResponder(true) is not proof of the actual responder');
  assert.ok(verifiedAt < operatedAt, 'the action must wait for exact WKWebView ownership verification');
  assert.ok(operatedAt < restoredAt, 'every action result must flow through responder restoration');
  assert.match(focusScope, /Do not use `\?`[\s\S]{0,180}focus rejection and action failure/);

  assert.match(focusScope, /ensure_responder_is_restorable\(previous, window\)/);
  assert.match(focusScope, /previous-responder-hidden/);
  assert.match(focusScope, /reject_after_clearing_unsafe_responder\(window, error\)/);
  assert.match(focusScope, /if !unsafe_responder \{[\s\S]{0,80}return Err\(error\)/);
  assert.match(focusScope, /window\.makeFirstResponder\(None\)/);
  assert.match(focusScope, /rejected-and-unsafe-focus-clear-failed/);
  assert.match(focusScope, /clear_browser_focus\(view, window\)\?/);
  assert.match(focusScope, /focus-restore-failed/);
  assert.match(focusScope, /ACTION_COMMITTED_FOCUS_RESTORE_FAILED/);
  assert.match(focusScope, /ACTION_COMMIT_UNKNOWN_FOCUS_RESTORE_FAILED/);
});

test('macOS post-dispatch focus failures become structured non-retryable tool outcomes', () => {
  const outcome = core.slice(
    core.indexOf('fn committed_platform_outcome'),
    core.indexOf('#[derive(Debug'),
  );
  const dispatch = core.slice(
    core.indexOf('pub(crate) async fn execute_page_tool'),
    core.indexOf('#[cfg(test)]'),
  );

  assert.match(outcome, /ACTION_COMMITTED_FOCUS_RESTORE_FAILED/);
  assert.match(outcome, /ACTION_COMMIT_UNKNOWN_FOCUS_RESTORE_FAILED/);
  assert.match(outcome, /ACTION_PARTIALLY_COMMITTED/);
  assert.match(outcome, /tool_error_text/);
  assert.match(outcome, /"actionCommitted": true/);
  assert.match(outcome, /"actionCommitState": commit_state/);
  assert.match(outcome, /"actionMayHaveCommitted": action_may_have_committed/);
  assert.match(outcome, /"subActionCommitState": sub_action_commit_state/);
  assert.match(outcome, /"retryable": false/);
  assert.match(outcome, /"focusRestoreFailed": focus_restore_failed/);
  assert.match(outcome, /Do not repeat the whole/);

  for (const operation of [
    'click_browser_core_element',
    'fill_one',
    'type_browser_core_text',
    'press_browser_core_key',
  ]) {
    assert.match(
      dispatch,
      new RegExp(`native_input_dispatch_result\\([\\s\\S]{0,180}${operation}`),
      `${operation} must convert committed focus errors before they escape as host errors`,
    );
  }
  assert.match(dispatch, /committed_platform_outcome\("Dialog action", &error\)/);
  assert.match(
    macos,
    /text_committed[\s\S]{0,520}mark_action_partially_committed\([\s\S]{0,160}text was inserted before submit-key dispatch/,
  );
  assert.match(
    macos,
    /dispatch_mouse_click\(webview, authorization, x, y, 1, 1\)[\s\S]{0,240}mark_incomplete_after_possible_commit/,
  );
  assert.match(
    macos,
    /insert_text\(webview, authorization, text\)[\s\S]{0,420}text may have been inserted before submit-key dispatch began/,
  );
});

test('macOS native-input provenance refresh is strict and callback grace stays bounded', () => {
  const macAuthorization = macos.slice(
    macos.indexOf('fn registered_control_for_authorization'),
    macos.indexOf('#[derive(Clone)]'),
  );
  const authorize = state.slice(
    state.indexOf('pub(super) fn authorize_agent_dispatch'),
    state.indexOf('pub(super) fn refresh_agent_input_window'),
  );
  const refreshState = state.slice(
    state.indexOf('fn refresh_agent_operation(&mut self'),
    state.indexOf('impl WorkspaceControl'),
  );
  const refresh = state.slice(
    state.indexOf('pub(super) fn refresh_agent_input_window'),
    state.indexOf('pub(super) fn end_agent_operation'),
  );
  const end = state.slice(
    state.indexOf('pub(super) fn end_agent_operation'),
    state.indexOf('pub(super) fn begin_agent_input'),
  );
  assert.match(refresh, /state\.refresh_agent_operation\(lease, now\)/);
  assert.match(refreshState, /lease\.owner != NativeControlOwner::Agent/);
  assert.match(refreshState, /self\.snapshot\.owner != NativeControlOwner::Agent/);
  assert.match(refreshState, /self\.snapshot\.revision != lease\.revision/);
  assert.match(refreshState, /self\.active_lease\.as_deref\(\) != Some\(lease\.lease\.as_str\(\)\)/);
  assert.match(refreshState, /!self\.active_operation_matches\(lease\)/);
  assert.match(state, /POST_DISPATCH_CALLBACK_GRACE: Duration = Duration::from_millis\(100\)/);
  assert.match(end, /state\.active_agent_operation = None/);
  assert.match(end, /deadline\.min\(now \+ POST_DISPATCH_CALLBACK_GRACE\)/);
  assert.match(authorize, /state\.active_operation_matches\(lease\)/);
  assert.doesNotMatch(authorize, /agent_input_until\s*=/);
  assert.match(macAuthorization, /binding\.tab_token != authorization\.tab_token/);
  assert.match(macAuthorization, /if emits_takeover_signal/);
  assert.match(macAuthorization, /control\.refresh_agent_input_window\(authorization\)/);
  assert.match(macAuthorization, /control\.authorize_agent_dispatch\(authorization\)/);
  assert.doesNotMatch(macAuthorization, /active_agent_operation\(\)/);
  const insertText = macos.slice(
    macos.indexOf('async fn insert_text'),
    macos.indexOf('async fn dispatch_key'),
  );
  const mouseClick = macos.slice(
    macos.indexOf('async fn dispatch_mouse_click'),
    macos.indexOf('fn mouse_event'),
  );
  const keyPress = macos.slice(
    macos.indexOf('async fn dispatch_key_event'),
    macos.indexOf('/// Return a responder'),
  );
  assert.match(insertText, /with_native_webview\(webview, authorization, false/);
  assert.match(insertText, /responder\.insertText/);
  assert.match(mouseClick, /with_native_webview\(webview, authorization, true/);
  assert.match(mouseClick, /target\.mouseDown/);
  assert.match(keyPress, /with_native_webview\(webview, authorization, true/);
  assert.match(keyPress, /responder\.keyDown/);
  assert.match(core, /execute_page_tool\([\s\S]{0,160}authorization: &NativeTabLease/);
  assert.match(core, /click_browser_core_element\(webview, authorization/);
});

test('macOS system surface follows the product gate without claiming CDP', () => {
  assert.match(system, /browser_product_enabled\(\)/);
  assert.match(system, /NativeSurfaceCapabilities::new\(enabled, enabled, false\)/);
  assert.match(platform, /browser_core_available\(\)[\s\S]{0,180}browser_product_enabled\(\)/);
  assert.match(platform, /"browser-core-wkwebview"/);
  assert.doesNotMatch(macos, /reqwest|std::process::Command|remote-debugging-port/);
});

test('macOS browser release is atomic and fail-closed outside explicit preview builds', () => {
  const defaults = cargo.match(/^default\s*=\s*\[[^\n]*\]/m)?.[0] || '';
  assert.match(cargo, /^browser-macos-preview\s*=\s*\[\]/m);
  assert.doesNotMatch(defaults, /browser-macos-preview/);
  assert.match(capabilities, /const MACOS_BROWSER_RELEASED: bool = false;/);
  assert.match(capabilities, /cfg!\(feature = "browser-macos-preview"\)/);
  assert.match(
    capabilities,
    /browser_native_display: browser_product_enabled\(\)[\s\S]{0,120}browser_agent_automation: browser_product_enabled\(\)/,
  );

  for (const consumer of [platform, system, extraction, browser]) {
    assert.doesNotMatch(
      consumer,
      /cfg!\(feature = "browser-macos-preview"\)/,
      'only the central semantic capability helper may inspect the Cargo feature',
    );
  }

  const prepare = browser.slice(
    browser.indexOf('async fn prepare_native_workspace'),
    browser.indexOf('async fn prepare_browser_core_workspace'),
  );
  const restore = browser.slice(
    browser.indexOf('async fn restore_saved_workspace'),
    browser.indexOf('pub fn spawn_watch'),
  );
  const watch = browser.slice(
    browser.indexOf('pub fn spawn_watch'),
    browser.indexOf('// -----------------------------------------------------------------------\n    // Lifecycle'),
  );
  const status = browser.slice(
    browser.indexOf('pub async fn status'),
    browser.indexOf('pub fn hand_back_to_agent'),
  );
  assert.ok(prepare.indexOf('browser_product_enabled()') < prepare.indexOf('restore_saved_workspace'));
  assert.ok(restore.indexOf('browser_product_enabled()') < restore.indexOf('read_restore_workspace'));
  assert.ok(watch.indexOf('reset_host_request_directory_for_process_start') < watch.indexOf('browser_product_enabled()'));
  assert.ok(watch.indexOf('browser_product_enabled()') < watch.indexOf('recommended_watcher'));
  assert.ok(status.indexOf('browser_product_enabled()') < status.indexOf('restore_saved_workspace'));

  assert.match(
    extraction,
    /browser_mcp_entry_for_session[\s\S]{0,260}browser_product_enabled\(\)/,
  );
  assert.match(
    extraction,
    /browser_unavailability_reason[\s\S]{0,180}browser_product_enabled\(\)/,
  );
  assert.match(extraction, /reserve_work_mode_browser_server_name\(servers\)/);

  assert.match(main, /const browserNativeDisplayAvailable = !!platformCapabilities\.browserNativeDisplay/);
  assert.match(
    main,
    /if \(!browserNativeDisplayAvailable\) \{[\s\S]{0,180}setBrowserSessions\(\{\}\);[\s\S]{0,80}setBrowserPaneStates\(\{\}\)/,
  );
  assert.match(
    main,
    /if \(!browserNativeDisplayAvailable \|\| !browserSessionId\) return;[\s\S]*?const readiness = browserLifecycleListenersReadyRef\.current;[\s\S]*?Promise\.resolve\(readiness\)\.then[\s\S]*?browser_status/,
  );
  assert.match(main, /\{browserDockAvailable && browserPaneOpen/);
  assert.doesNotMatch(main, /navigator\.userAgent|target_os|browser-macos-preview/);
  assert.doesNotMatch(
    normalMacBuildEntrypoints,
    /browser-macos-preview/,
    'normal development and packaging commands must not opt into the preview backend',
  );
});
