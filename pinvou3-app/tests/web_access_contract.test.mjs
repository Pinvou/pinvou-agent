import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

// Source-regex assertions use bounded character gaps (e.g. [\s\S]{0,200});
// a Windows checkout with core.autocrlf=true inflates those gaps with \r
// characters, so normalize line endings before matching.
const readSource = (filePath, encoding) =>
  fs.readFileSync(filePath, encoding).replace(/\r\n/g, '\n');

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const bridgeRoot = path.join(root, 'src', 'platform', 'tauri');
const chunkedUpload = readSource(
  path.join(root, 'src', 'shared', 'chunked-file-upload.js'),
  'utf8',
);
const webBridge = readSource(path.join(root, 'src', 'platform', 'web', 'bridge.js'), 'utf8');
const webDomainAdapter = readSource(
  path.join(root, 'src', 'platform', 'web', 'bridge', 'domain-adapter.js'),
  'utf8',
);
const attachmentDropController = readSource(
  path.join(root, 'src', 'features', 'attachments', 'attachment-drop-controller.js'),
  'utf8',
);
const attachmentDropHook = readSource(
  path.join(root, 'src', 'features', 'attachments', 'useAttachmentDrop.js'),
  'utf8',
);
const desktopRemoteControlBridge = readSource(
  path.join(bridgeRoot, 'bridge', 'remote-control.js'),
  'utf8',
);
const desktopSessionsBridge = readSource(
  path.join(bridgeRoot, 'bridge', 'sessions.js'),
  'utf8',
);
const desktopAuxChatBridge = readSource(
  path.join(bridgeRoot, 'bridge', 'aux-chat.js'),
  'utf8',
);
const sharedBridgeHelpers = readSource(
  path.join(root, 'src', 'shared', 'bridge-shared-helpers.js'),
  'utf8',
);
const desktopBridgeSources = [
  readSource(path.join(bridgeRoot, 'bridge.js'), 'utf8'),
  ...fs.readdirSync(path.join(bridgeRoot, 'bridge'))
    .filter(name => name.endsWith('.js'))
    .sort() // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic order of the string array is the asserted expectation
    .map(name => readSource(path.join(bridgeRoot, 'bridge', name), 'utf8')),
];
const bridge = [
  webBridge,
  webDomainAdapter,
  ...desktopBridgeSources,
].join('\n');
const bootstrap = readSource(path.join(root, 'src', 'platform', 'web', 'bootstrap.js'), 'utf8');
const hostFilePicker = readSource(
  path.join(root, 'src', 'platform', 'web', 'host-file-picker.js'),
  'utf8',
);
const commandsRoot = path.join(root, 'src-tauri', 'src', 'app', 'commands');
// Round-20 review (S14): the aggregation previously covered only top-level
// files, so `dependencies/` subdirectories escaped the scan. Switched to
// recursive collection (sorted by full path; ordering stays deterministic).
// Round-21 review: readdirSync's default sort triggers
// unicorn/require-array-sort-compare (CI frontend-test has been red since the
// round-20 wave) — use an explicit comparator instead of relying on the
// implicit behavior of elements happening to be strings. Round-22 review:
// switched to readdirSync's withFileTypes (Dirent.isDirectory has lstat
// semantics) — statSync follows directory symlinks, so a linked directory
// under any of the three scan roots would recurse across trees (false red) or
// even loop; same shape as the bridge_domain_protocol suite's sourceFiles.
function walkRustSources(rootDir) {
  const files = [];
  const visit = dir => {
    for (const entry of fs
      .readdirSync(dir, { withFileTypes: true })
      .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) visit(full);
      else if (entry.name.endsWith('.rs')) files.push(full);
    }
  };
  visit(rootDir);
  return files;
}
const commands = walkRustSources(commandsRoot)
  .map(fullPath => readSource(fullPath, 'utf8'))
  .join('\n');
const remoteControlCommands = readSource(path.join(commandsRoot, 'remote_control.rs'), 'utf8');
const remoteControlManagerRoot = path.join(
  root,
  'src-tauri',
  'src',
  'features',
  'remote_control',
  'manager',
);
// Round-21 review: the two aggregations also go through walkRustSources
// recursion — manager/ and platform/ are flat directories today, and a
// non-recursive enumeration would silently miss subdirectories appearing in
// the future (same class as the commands S14). While flat, the DFS order
// matches the old name sort, so existing assertions are unaffected.
const remoteControlManager = walkRustSources(remoteControlManagerRoot)
  .map(fullPath => readSource(fullPath, 'utf8'))
  .join('\n');
const remoteControlPlatformRoot = path.join(
  root,
  'src-tauri',
  'src',
  'features',
  'remote_control',
  'platform',
);
const remoteControlPlatform = walkRustSources(remoteControlPlatformRoot)
  .map(fullPath => readSource(fullPath, 'utf8'))
  .join('\n');
const settingsView = readSource(path.join(root, 'src', 'features', 'settings', 'SettingsView.jsx'), 'utf8');
const artifactsPanel = readSource(path.join(root, 'src', 'features', 'artifacts', 'ArtifactsPanel.jsx'), 'utf8');
const toolStoreView = readSource(path.join(root, 'src', 'features', 'tools', 'ToolStoreView.jsx'), 'utf8');
const toolRenderers = readSource(path.join(root, 'src', 'features', 'tools', 'tool-renderers.jsx'), 'utf8');
const knowledgeView = readSource(path.join(root, 'src', 'features', 'knowledge', 'KnowledgeView.jsx'), 'utf8');
const toolCommon = readSource(path.join(root, 'src', 'features', 'tools', 'tool-common.jsx'), 'utf8');
const connectionStatus = readSource(path.join(root, 'src', 'features', 'web', 'WebConnectionStatus.jsx'), 'utf8');
const chatView = readSource(path.join(root, 'src', 'features', 'chat', 'ChatView.jsx'), 'utf8');
const codexView = readSource(path.join(root, 'src', 'features', 'codex', 'CodexAcpView.jsx'), 'utf8');
const acpRuntimeNotices = readSource(
  path.join(root, 'src', 'features', 'codex', 'AcpRuntimeNotices.jsx'),
  'utf8',
);
const codexWorkspacePanel = readSource(
  path.join(root, 'src', 'features', 'codex', 'CodexWorkspacePanel.jsx'),
  'utf8',
);
const codeViewerContent = readSource(
  path.join(root, 'src', 'features', 'codex', 'CodeViewerContent.jsx'),
  'utf8',
);
const acpPlatformClient = readSource(path.join(root, 'src', 'features', 'codex', 'acpClient.js'), 'utf8');
const acpErrors = readSource(path.join(root, 'src', 'features', 'codex', 'acpErrors.js'), 'utf8');
const i18n = ['zh', 'en', 'ja'].map((l) => readSource(path.join(root, 'src', 'shared', 'i18n', `${l}.js`), 'utf8')).join('\n'); // after the split the three languages live in the i18n/ directory
const appMain = readSource(path.join(root, 'src', 'app', 'main.jsx'), 'utf8');
const policy = JSON.parse(readSource(path.join(root, 'src', 'platform', 'web', 'access-policy.json'), 'utf8'));
const allowed = new Set(policy.allowed_commands);
const allowedEvents = new Set(policy.allowed_events);

function rustCommandBlock(source, command) {
  const signature = source.indexOf(`fn ${command}(`);
  assert.notEqual(signature, -1, `missing Rust command ${command}`);
  const start = source.lastIndexOf('#[tauri::command]', signature);
  const next = source.indexOf('#[tauri::command]', signature + command.length);
  return source.slice(start, next === -1 ? source.length : next);
}

// Round-11 review M4: the desktop-only anti-pins for web_access_* management
// commands converge into a single list.
// Namespace convention: web_access_* is management-plane only (endpoint
// lifecycle, pairing state, RPC proxying, bridge readiness); except for
// web_access_list_host_files, none of them may enter the Web policy.
const desktopOnlyWebAccessCommands = [
  'web_access_enable',
  'web_access_disable',
  'web_access_status',
  'web_access_rotate',
  'web_access_revoke_host_workspace',
  // Round-10 review minor: the main WebView event channel is management-only
  // too — if it entered the Web policy, the phone could trigger
  // publish_frontend_event via RPC→bridge invoke (passing only the
  // policy.events allowlist, so allowed events could be forged); absence is
  // the entire defense.
  'web_access_publish_event',
  // Round-11 review M4: the RPC proxy and bridge readiness commands — their
  // doc comments declare desktop-only, but they were on no anti-pin/gate-pin
  // list, so adding them to access-policy or deleting their
  // require_main_webview gate would have stayed silently green (the policy
  // side of rpc_begin/rpc_respond is the whole defense: the relay path's
  // execution window is main anyway).
  'web_access_bridge_ready',
  'web_access_rpc_begin',
  'web_access_rpc_respond',
];

for (const command of [
  'chat',
  'ingest_file',
  'transcribe_voice_audio',
  'save_model',
  'delete_model',
  'set_active_model',
  'test_model_connection',
  'set_disabled_connectors',
  'install_marketplace_skill',
  'install_marketplace_tool',
  'uninstall_marketplace_skill',
  'uninstall_marketplace_tool',
  'codex_acp_prompt',
  'get_codex_acp_timeline',
  'get_codex_acp_session_info',
  'get_codex_acp_pending_permissions',
  'get_codex_acp_pending_elicitations',
  'list_acp_agents',
  'get_acp_agent_status',
  'set_codex_acp_model',
  'set_codex_acp_mode',
  'set_codex_acp_config_option',
  'create_codex_acp_session',
  'cancel_codex_acp',
  'list_codex_acp_sessions',
  'list_codex_workspace',
  'search_codex_workspace',
  'preview_codex_workspace_file',
  'get_codex_workspace_changes',
  'get_codex_workspace_diff',
  'install_acp_agent',
  'login_acp_agent',
  'switch_acp_agent_account',
  'open_acp_agent_login_url',
  'submit_acp_agent_login_code',
  'web_access_enable',
  // Round-9 review: web_access_* management commands are desktop-only too —
  // phone-side reachability goes only through the main WebView's proxied RPC
  // (where the require_main_webview gate passes by design), so the real gate
  // is absence from the policy allowlist; anti-pinning only enable would let
  // "add the remaining commands to access-policy.json" silently pass every
  // test. (web_access_list_host_files deliberately stays in the Web policy —
  // the phone file picker's RPC is dispatched by the main WebView and is not
  // on this anti-pin list; the require_main_webview gate blocks the aux
  // webview separately at the command layer.)
  // Round-11 review M4: the web_access_* management commands converge into one
  // list, and the structural mutual-exclusion pin below judges "the remaining
  // commands" against it — the union of the list and the Web policy must
  // cover every web_access_* command defined in remote_control.rs, and a new
  // command added to neither side goes red (the structural escapees — the rpc
  // proxy trio and bridge_ready — are caught this way).
  ...desktopOnlyWebAccessCommands,
  // Editing an uploaded bundle's display name/description writes bundles.json
  // and may rewrite SKILL.md: a write operation, desktop-only.
  'update_bundle_display_meta',
  // Recycle-bin restore/purge and both bundle exports are desktop write
  // operations (native save dialog + filesystem changes): the Web-side recycle
  // bin is read-only, only list_recycled_plugins is allowed, and the write
  // commands stay on the desktop.
  'restore_recycled_plugin',
  'purge_recycled_plugin',
  'export_recycled_plugin',
  'export_installed_plugin',
  // Workspace branch view/checkout run local git mutations (add -A / commit /
  // stash / checkout): desktop-only, the web client rejects them explicitly.
  'list_codex_workspace_branches',
  'checkout_codex_workspace_branch',
  // Clipboard reads are desktop-privacy surface: the paste fallback runs on
  // the desktop host only; web pastes go through the device-upload path.
  'paste_clipboard_image',
  // Dependency detection shells out to system package managers and is
  // deliberately not allowlisted on the web; the web hides the whole
  // dependency section behind the dependencyInstall capability bit.
  'check_dependencies',
]) {
  assert.equal(allowed.has(command), false, `${command} must remain desktop-only`);
}

// Round-11 review M4: structural mutual-exclusion pin — every web_access_*
// command defined in remote_control.rs must be either in the Web policy
// (subject to the positive wrapper pins) or in the desktop-only anti-pin list
// above. The structural escapees of the hand-maintained anti-pin list
// (web_access_bridge_ready / web_access_rpc_begin / web_access_rpc_respond
// once all three slipped through) are permanently backstopped by this pin:
// any new web_access_* command added to neither side goes red.
{
  const defined = new Set();
  // Round-19 review M1: the regex must be anchored at line start. The earlier
  // unanchored version also counted the 8 signature-shaped strings in the
  // `#[cfg(test)]` folded-error pin data table ("pub async fn
  // web_access_chat(" and friends) as definitions — after a real command was
  // deleted or renamed, the Set was still propped up by those phantom hits and
  // the reverse-existence pin was blind for the other 8/42 policy commands
  // (probe evidence). Production signatures all sit at column 0 while table
  // data is indented, so line-start anchoring separates them precisely.
  for (const match of remoteControlCommands.matchAll(/^pub(?:\([\w:]+\))? (?:async )?fn (web_access_\w+)\(/gm)) {
    defined.add(match[1]);
  }
  assert.ok(defined.size >= 50,
    `expected the full web_access_* command surface, found ${defined.size}`);
  for (const command of defined) {
    assert.ok(
      allowed.has(command) || desktopOnlyWebAccessCommands.includes(command),
      `${command} must be either in the web policy or explicitly anti-pinned as desktop-only`,
    );
  }
  for (const command of desktopOnlyWebAccessCommands) {
    assert.ok(defined.has(command),
      `${command} is anti-pinned but no longer defined in remote_control.rs`);
  }
  // Round-15 review M1: reverse-existence pin — every web_access_* allowed by
  // the Web policy must still be defined in remote_control.rs. Deleting only
  // the Rust command while the policy kept the name left the forward pins all
  // green while the phone invoked a nonexistent command at runtime (until then
  // only the Rust→policy direction existed).
  for (const command of allowed) {
    if (command.startsWith('web_access_')) {
      assert.ok(defined.has(command),
        `${command} is allowed by the web policy but no longer defined in remote_control.rs`);
    }
  }
}

// Round-11 review M4: the anti-pin for the event half — every web_access:*
// event is management-only (status carries the pairing URL/QR, rpc_request
// carries the proxied command and args, event_subscribe is subscription
// control). Structurally scan the web_access:* literals in the Rust manager
// and the JS bridge: none of them may appear in the Web policy event list,
// and a new same-named event is swept straight into the anti-pin (the event
// half previously had no reverse pin at all).
// Round-15 review M5: include the command layer in the scan. Round-18 review
// correction of the terms: the command layer has zero hits today — this
// source now contributes zero assertions, as forward-looking backstop: if the
// command layer ever adds a same-named event literal (forwarding/status
// channel), the scan immediately catches it into the anti-pin instead of
// letting it slip through.
// Round-19 review: widen the scan surface from the single remote_control.rs
// to the whole app/commands aggregation (aggregated at the top of this file;
// reused); relax the regex to any quote style (single quotes/template
// literals) and allow digits in segment names ([a-z0-9_]+), closing three
// bypass spellings proven by probe; strip comment lines first so a doc
// mention cannot create phantom assertions (the phantom direction is already
// fail-closed; stripping only avoids a false red if the policy ever changes).
// Round-20 review: make the aggregation recursive (dependencies/
// subdirectories escaped the scan before) and bring the
// features/remote_control/platform aggregation into the scan too (it
// previously served only other pins); both new surfaces have zero
// web_access:* hits today — purely forward-looking backstop.
// Strip only pure comment lines (starting with // or /*): code lines with
// trailing comments must be kept, otherwise a `web_access:x` literal hidden in
// the code before a trailing comment would be deleted with the whole line,
// making the scan fail-open (the same class the Round-17 correction fixed).
// Block-comment continuation lines (`* …`) are kept — text there can only
// create fail-closed phantom assertions, never let a real violation through.
// Round-29 review MINOR-5: a line OPENING with a block comment is pure only
// when that comment closes on the same line with nothing but whitespace
// after it — `/* note */ emit("web_access:x");` used to be stripped whole
// (the opener regex matched), deleting a real literal from the scan; the
// fail-open class this function exists to close. An unclosed opener line is
// still pure (the whole line is comment); a closed opener followed by code
// is now kept, so its literals stay fail-closed-scanned.
// Round-30 review P2 (JensenChen28): the scan STOPS at the first close —
// after a leading block comment closes, delimiters in the remainder are
// executable/string text, not comment syntax (JS block comments do not nest
// either). Parsing them let `/* note */ const s = "/*"; emit("web_access:x");`
// re-open a phantom comment (depth stuck > 0 → whole line stripped) and a
// trailing `/* tail */` move closedAt past the code (slice empty → stripped):
// both fail-open, letting a forbidden literal evade the contract scan.
// Freezing the close point keeps the whole remainder — conservative in the
// only changed direction: a chained `/* a */ /* b */` line is now kept too,
// which can only add fail-closed phantom assertions, never drop one.
function stripPureCommentLines(source) {
  return source.split('\n')
    .filter(line => {
      if (/^\s*\/\//.test(line)) return false;
      if (!/^\s*\/\*/.test(line)) return true;
      let inBlockComment = false;
      let closedAt = -1;
      for (let index = 0; index + 1 < line.length; index += 1) {
        const two = line.slice(index, index + 2);
        if (!inBlockComment && two === '/*') {
          inBlockComment = true;
          index += 1;
          continue;
        }
        if (inBlockComment && two === '*/') {
          // First close ends the comment AND the scan (round-30): freeze the
          // remainder as code, do not parse further delimiters.
          closedAt = index + 2;
          break;
        }
      }
      // Unclosed block opener (closedAt never set): the whole line is comment.
      if (closedAt === -1) return false;
      // Closed on this line: keep only when code follows the comment (the
      // probe-executed round-29 direction — an inverted predicate here
      // strips exactly the code-bearing lines the fix exists to keep).
      return line.slice(closedAt).trim() !== '';
    })
    .join('\n');
}
// Round-30 review P2 regressions (trailing-code forms after a closed leading
// block comment): each kept string must survive the strip VERBATIM — a
// regression that re-parses delimiters (nested-depth counter or continued
// scanning) deletes the line and silently green-lights its forbidden literal.
assert.equal(
  stripPureCommentLines('/* note */ const s = "/*"; emit("web_access:bad");'),
  '/* note */ const s = "/*"; emit("web_access:bad");',
  'delimiters inside trailing code are not comment syntax: the code-bearing line must survive the strip whole',
);
assert.equal(
  stripPureCommentLines('/* note */ emit("web_access:bad"); /* tail */'),
  '/* note */ emit("web_access:bad"); /* tail */',
  'a trailing block comment must not move the close point past executable code: the line must survive the strip whole',
);
// Guard the direction the round-30 fix changed AWAY from the old depth
// counter: JS block comments do not nest, so the first `*/` closes even
// after an inner `/*` — code after it must stay in the scan.
assert.equal(
  stripPureCommentLines('/* /* nested */ emit("web_access:bad");'),
  '/* /* nested */ emit("web_access:bad");',
  'JS block comments do not nest: the first close ends the scan and the trailing code must survive',
);
// Unchanged contract: pure comment lines and unclosed openers stay stripped,
// so doc mentions cannot create phantom scan hits.
assert.equal(stripPureCommentLines('/* note */'), '', 'a closed pure-comment line is still stripped');
assert.equal(stripPureCommentLines('/* note'), '', 'an unclosed opener line is still stripped');
assert.equal(stripPureCommentLines('// note'), '', 'a // line is still stripped');
assert.equal(
  stripPureCommentLines('const s = 1; /* tail */'),
  'const s = 1; /* tail */',
  'a code-opening line with a trailing comment is kept (round-17 class)',
);
for (const source of [remoteControlManager, desktopRemoteControlBridge, commands, remoteControlPlatform].map(stripPureCommentLines)) {
  for (const match of source.matchAll(/["'`]web_access:([a-z0-9_]+)["'`]/g)) {
    const eventName = `web_access:${match[1]}`;
    assert.equal(allowedEvents.has(eventName), false,
      `${eventName} is a management channel and must never enter the web event policy`);
  }
}

// Round-11 review minor: the cross-language contract of the JS↔Rust error
// code table. The `WEB_TURN_ERROR_KEYS` keys in bridge.js are stable wire
// codes folded out on the Rust side; Rust-side pins such as
// stable_web_error_codes_are_locked hold the producer side, and this is the
// consumer-side reverse pin: every JS key must still appear in some Rust
// command source — renaming a Rust code while missing the JS side leaves the
// key mapped to no copy and silently falling into the redacted fallback,
// which turns this pin red.
{
  const tableStart = webBridge.indexOf('const WEB_TURN_ERROR_KEYS = {');
  assert.notEqual(tableStart, -1, 'WEB_TURN_ERROR_KEYS must exist in web/bridge.js');
  const tableEnd = webBridge.indexOf('};', tableStart);
  const table = webBridge.slice(tableStart, tableEnd);
  const jsKeys = [...table.matchAll(/^\s{4}([a-z_0-9]+): "/gm)].map(match => match[1]);
  assert.ok(jsKeys.length >= 14,
    `expected the full turn-error code table, found ${jsKeys.length} keys`);
  // Producer code lives in app/commands/* (the folding points) and
  // features/remote_control/manager/* (rpc admission rejections); scan both.
  const rustProducers = commands + remoteControlManager;
  for (const code of jsKeys) {
    assert.ok(rustProducers.includes(code),
      `WEB_TURN_ERROR_KEYS entry "${code}" has no Rust producer; rename one side or the other`);
  }
}

// Knowledge-base bulk import's progress viewing and resume/cancel/retry/
// failed-file paging are a coordinated command group: the Web-side knowledge
// base is already open (kb_collection_add_sources etc.), so missing any import
// control command would make the corresponding button fail silently.
for (const command of [
  'kb_index_status',
  'kb_index_cancel',
  'kb_index_resume',
  'kb_index_retry_file',
  'kb_index_failed_files',
]) {
  assert.equal(allowed.has(command), true, `${command} must be allowed on Web (KB import controls)`);
}

// Builtin feature switches (docs/builtin-toolset-contract.md §3.3): the WebUI
// proxies to the same desktop host, so the feature-off state must be readable
// from the browser too — otherwise a browser client keeps a switched-off
// feature (session mention) fully live while read_session is gone on the
// host. list_builtin_features is lane-agnostic and read-only; the write side
// stays desktop-only.
for (const command of ['list_builtin_features']) {
  assert.equal(allowed.has(command), true, `${command} must be allowed on Web (builtin feature registry read)`);
}
for (const command of ['set_builtin_feature_enabled']) {
  assert.equal(allowed.has(command), false, `${command} must remain desktop-only (builtin feature toggles)`);
}
// Hot refresh: switch toggles broadcast remote_control:tools_changed; the relay
// must forward it (allowlist) and the web bridge must re-dispatch the shared
// pinvou:tools-changed DOM event so ChatView's registry subscription updates
// on browser clients too (desktop gets the same event via chat-events.js).
assert.equal(
  allowedEvents.has('remote_control:tools_changed'), true,
  'remote_control:tools_changed must be allowed on Web (builtin feature switch hot refresh)',
);
assert.match(
  webBridge,
  /listen\("remote_control:tools_changed", function \(\) \{\s*try \{ window\.PinvouToolStoreEvents\.notifyComposerToolsChanged\(\); \}/,
  'web bridge must re-dispatch pinvou:tools-changed through the shared module from remote_control:tools_changed (bounded to the adjacent dispatch line — round-23 asto minor 3: the old unbounded gap stayed green on a deleted dispatch; round-9: FFE4-1 conversion completed, the web lane now matches the desktop chat-events.js twin)',
);

// Memory organize is a global action (no session scope, same surface as
// get_memory_overview): the settings "AI organize memory" button and the
// "last organized" history read must work on WebUI too; missing
// either one makes the button fail silently with command_not_allowed.
for (const command of ['organize_memory', 'get_memory_organize_history']) {
  assert.equal(allowed.has(command), true, `${command} must be allowed on Web (memory organize)`);
}

// Scene-label sidecars sync per session switch on Web exactly like the
// persona-events / pinvou-reviews siblings; dropping either entry logs a
// sidecar failure per session switch and silently stops label sync.
for (const command of ['get_session_pinvou_scene_events', 'save_session_pinvou_scene_events']) {
  assert.equal(allowed.has(command), true, `${command} must be allowed on Web (scene-label sidecars)`);
}

// Authorized-connector toggle/skill state queries are part of the WebUI surface
// (SettingsView's composer tool menu calls *_skills_state; authorization-state
// injection happens inside the command layer via get_marketplace_tool_auth_status,
// so the frontend no longer calls *_status directly). Missing any one leaves the
// connector permanently showing "disconnected" on Web: the card depends on the
// installed badge because externalAuth is unavailable.
// Connector toggling and skill install/remove (*_ensure_cli, *_apply_skills, ...)
// remain desktop-only.
for (const command of [
  'feishu_skills_state',
  'wecom_skills_state',
  'dingtalk_skills_state',
  'tmeet_skills_state',
]) {
  assert.equal(allowed.has(command), true, `${command} must be allowed on Web (authorized connector status queries)`);
}
// Connector mutation surfaces stay desktop-only: connect/disconnect
// (*_connect_begin/*_logout, ima_connect/ima_logout), native CLI install
// (*_ensure_cli triggers download + materialization), skill install/remove
// (*_apply_skills materializes skill packs into ~/.pinvou3), OAuth abort
// (*_cancel), authorization-gate recomputation (refresh_connector_auth_gates).
// The per-connector toggles (set_*_enabled) and the skill-level disable lists
// (set_disabled_skills/get_disabled_skills) were removed with the dead-command
// cleanup: connector toggles go through set_disabled_connectors.
// This list must stay in sync with the connector registration surface in lib.rs.
// The retired *_status commands left the frontend (ToolStoreView now uses
// *_skills_state / bundle_readiness); they were never web-allowed (not in the
// allowlist on main either) and are reverse-pinned here so they cannot
// quietly return.
for (const command of [
  'feishu_status',
  'wecom_status',
  'dingtalk_status',
  'tmeet_status',
  'ima_status',
]) {
  assert.equal(allowed.has(command), false, `retired ${command} must not return to the Web allowlist`);
}
const deniedConnectorMutations = [];
for (const connector of ["feishu", "wecom", "dingtalk", "tmeet"]) {
  deniedConnectorMutations.push(
    `${connector}_connect_begin`,
    `${connector}_logout`,
    `${connector}_ensure_cli`,
    `${connector}_cancel`,
    `${connector}_apply_skills`,
  );
}
deniedConnectorMutations.push(
  "ima_connect", "ima_logout", "set_disabled_connectors", "refresh_connector_auth_gates",
  // Project-level skills toggle (settings admin surface; both read and write are
  // desktop-only. It used to be covered by neither list, so adding it to the
  // policy would not have triggered any test — contradicting the "list must stay
  // in sync with the registration surface" promise).
  "set_project_skills_enabled", "get_project_skills_enabled",
);
for (const command of deniedConnectorMutations) {
  assert.equal(allowed.has(command), false, `${command} must remain desktop-only (connector mutations)`);
}

for (const command of [
  'web_access_chat',
  'web_access_list_sessions',
  'web_access_list_archived_sessions',
  'web_access_create_session',
  // Round-11 review M4: create_session belongs in the same batch as
  // create_session_and_chat — it returns session metadata carrying the
  // transcript revision, so silently removing it from the policy would break
  // Web session creation and the optimistic concurrency token, and no pin
  // would have noticed.
  'web_access_create_session_and_chat',
  'web_access_ingest_file',
  'web_access_upload_attachment_chunk',
  'web_access_abort_attachment_upload',
  'web_access_discard_attachment',
  'web_access_cancel_session_download',
  'web_access_load_session_chunk',
  'web_access_transcribe_voice_audio',
  'web_access_codex_acp_prompt',
  'web_access_cancel_codex_acp',
  'web_access_get_codex_acp_timeline',
  'web_access_get_codex_acp_session_info',
  'web_access_get_codex_acp_pending_permissions',
  'web_access_get_codex_acp_pending_elicitations',
  'web_access_list_acp_agents',
  'web_access_list_codex_acp_sessions',
  'web_access_get_acp_agent_status',
  'web_access_set_codex_acp_model',
  'web_access_set_codex_acp_mode',
  'web_access_set_codex_acp_config_option',
  'web_access_create_codex_acp_session',
  'web_access_list_codex_workspace',
  'web_access_search_codex_workspace',
  'web_access_preview_codex_workspace_file',
  'web_access_get_codex_workspace_changes',
  'web_access_get_codex_workspace_diff',
]) {
  assert.equal(allowed.has(command), true, `${command} must be the bounded Web wrapper`);
}
assert.equal(allowed.has('list_sessions'), false,
  'Web must not call the native session list that exposes host workspace metadata');
assert.equal(allowed.has('list_archived_sessions'), false,
  'Web must not call the native archived list that exposes host workspace metadata');

// Aux chat domain: platform/web/bridge.js's auxChatEnsure/
// auxChatReset invoke these commands directly; sending goes through the
// existing web_access_chat (an aux session can be store.load'ed by id and is
// not affected by the list_sessions filter). Missing any entry would make
// the Web aux chat fail silently.
for (const command of [
  'get_or_create_aux_session',
  'reset_aux_session',
]) {
  assert.equal(allowed.has(command), true, `${command} must be allowed on Web (aux chat)`);
}
assert.equal(allowed.has('get_aux_session'), false,
  'the dead get_aux_session command must stay removed from the Web surface');

// get_or_create_aux_session and reset_aux_session cross the Web/Relay
// boundary, so they must return the minimal AuxSessionBinding projection (id
// only) — never the full SessionMetadata, whose inherited host workspace
// path would leak to the browser (the redaction invariant behind
// redact_session_metadata_for_web).
assert.match(commands, /pub struct AuxSessionBinding \{\s*pub id: String,?\s*\}/,
  'the aux session command must return an id-only projection');
assert.match(commands, /fn get_or_create_aux_session\([\s\S]{0,300}?-> Result<AuxSessionBinding, String>/,
  'get_or_create_aux_session must not return raw SessionMetadata across the Web boundary');
assert.match(commands, /get_or_create_aux_session\(&session_id\)[\s\S]{0,200}?AuxSessionBinding \{ id: metadata\.id \}/,
  'the aux session projection must be built from the store metadata without exposing it');
assert.match(commands, /fn reset_aux_session\([\s\S]{0,400}?-> Result<AuxSessionBinding, String>/,
  'reset_aux_session must not return raw SessionMetadata across the Web boundary');
assert.match(commands, /fn reset_aux_session[\s\S]{0,600}?WebSessionOperation::ResetAuxSession/,
  'reset_aux_session errors must fold into the stable web_session code');

assert.equal(allowedEvents.has('acp:event'), true,
  'the shared ACP timeline must reach WebUI through the normal event transport');
assert.match(bootstrap, /acpCodeMode:\s*\{[\s\S]*?commands:\s*\[[\s\S]*?web_access_codex_acp_prompt[\s\S]*?events:\s*\["acp:event"\]/,
  'ACP code mode must require the complete Web-safe command and event contract');
assert.match(acpPlatformClient, /web_access_codex_acp_prompt/);
assert.match(acpPlatformClient, /attachmentHandles/);
assert.match(acpPlatformClient, /web_access_create_codex_acp_session/);
assert.match(acpPlatformClient, /workspaceHandle/);
assert.match(acpPlatformClient, /web_access_get_codex_workspace_changes/);
assert.match(acpPlatformClient, /web_access_get_codex_workspace_diff/);
assert.match(acpPlatformClient, /web_access_cancel_codex_acp/);
assert.doesNotMatch(codexWorkspacePanel, /invoke\('get_codex_workspace_(?:changes|diff)'/,
  'the shared workspace UI must not bypass the Web-safe command adapter');
assert.doesNotMatch(codexView, /invoke\('cancel_codex_acp'/,
  'the shared ACP UI must not bypass the Web-safe cancellation adapter');
// Round-15 review HIGH (mutation evidence): `?.[0] || ''` turns extraction
// drift (a rename, conversion to an arrow function, reformatting that removes
// the column-0 `\n}`) into a trivially-true doesNotMatch against the empty
// string — the workspacePath leak defense silently goes dark while the whole
// suite stays green. Assert the extraction is non-empty before running the
// anti-pin.
const createAcpSessionSource =
  acpPlatformClient.match(/export function createAcpSession\b[\s\S]*?\n\}/)?.[0];
assert.ok(
  createAcpSessionSource,
  'createAcpSession must be extractable for the workspacePath anti-pin; if this \
   assertion fires the function was renamed or restructured and the pin below \
   must be re-anchored',
);
// Round-29 review MINOR-6: the `\n}` cut truncates at the FIRST column-0
// closing brace — `assert.ok` covers non-existence but not truncation, so a
// restructure that introduces an early column-0 brace would silently scan
// only the function's head and let a leak below the cut escape. Pin a floor
// length under the real function (755 chars at extraction time): a truncated
// extraction lands far below it and turns red, while ordinary growth keeps
// passing. Re-measure this floor if the function is legitimately rewritten.
assert.ok(
  createAcpSessionSource.length >= 600,
  `the createAcpSession extraction is ${createAcpSessionSource.length} chars — \
   suspiciously below the known function size (755+), meaning the column-0 \
   brace cut truncated the scan window and the anti-pin below no longer \
   covers the whole function`,
);
assert.doesNotMatch(
  createAcpSessionSource,
  /web_access_create_codex_acp_session[\s\S]*?workspacePath/,
  'Web code-session creation must submit only the opaque workspace handle',
);
// The Web list must go through the projection command (host absolute paths
// downgraded to directory names) and must not call the desktop's original
// command directly.
assert.match(acpPlatformClient, /web_access_list_codex_acp_sessions/,
  'the Web session list must go through the path-redacting wrapper');
assert.doesNotMatch(codexView, /list_codex_acp_sessions/,
  'the shared code UI must list sessions through the platform ACP adapter');
assert.doesNotMatch(codexView, /invoke\('codex_acp_prompt'/,
  'the shared code UI must submit through the platform ACP adapter');
assert.match(acpRuntimeNotices, /manageAgentOnDesktop/,
  'WebUI must explain that install and login actions happen on the target desktop');
assert.match(codexWorkspacePanel, /can\('externalSystemOpen'\)/,
  'desktop-only open and reveal actions must stay hidden in WebUI');
assert.match(codexWorkspacePanel, /onOpen=\{systemOpenAvailable/);
assert.match(codexWorkspacePanel, /onReveal=\{systemOpenAvailable/);
assert.match(codexWorkspacePanel, /onOpenInNewWindow=\{systemOpenAvailable/);
assert.match(codeViewerContent, /\{!hideDesktopActions && onReveal && \(/);
assert.match(codeViewerContent, /\{!hideDesktopActions && onOpen && \(/,
  'the shared code preview must omit desktop system actions when callbacks are unavailable');

// Browser-side device upload: the dual entry is gated by capability
// negotiation, chunks are bounded, and the cancel/failure paths are complete.
assert.match(
  bootstrap,
  /deviceFileUpload:\s*\[[\s\S]*?"web_access_upload_attachment_chunk"[\s\S]*?"web_access_abort_attachment_upload"[\s\S]*?"web_access_discard_attachment"[\s\S]*?\]/,
  'the device upload capability must require chunk, abort, and discard commands',
);
assert.match(chatView, /can\('deviceFileUpload'\)/,
  'the attach button must gate the dual-entry menu on the negotiated capability');
assert.match(chatView, /bridge\.attachments\.uploadDeviceFiles\(files\)/);
assert.match(chatView, /bridge\.attachments\.pickAndAttach\(\)/,
  'the desktop-instance picker entry must keep using the existing remote browser');
assert.match(chunkedUpload, /CHUNK_BYTES = 256 \* 1024/,
  'upload chunks must stay aligned with the desktop MAX_TRANSFER_CHUNK_BYTES limit');
assert.match(chunkedUpload, /MAX_FILE_BYTES = 20 \* 1024 \* 1024/,
  'the browser preflight must mirror file_ingest::MAX_FILE_BYTES');
assert.match(webBridge, /PinvouChunkedFileUpload/,
  'Web chat uploads must use the shared chunk uploader');
assert.doesNotMatch(webBridge, /function bytesToBase64/,
  'Web chat must not carry a second base64 chunk implementation');
assert.match(webBridge, /web_access_abort_attachment_upload/,
  'cancelled or failed uploads must release the desktop buffer');
assert.match(webBridge, /web_access_discard_attachment/,
  'removed or late-cancelled attachments must release their opaque desktop handle');
assert.match(remoteControlCommands, /stage_uploaded_attachments\(attachments, session_id, &?store\)/,
  'uploaded attachments must be staged into the Session workspace before the engine sees their paths');
// Agent install command lines/output may contain internal mirror sources or
// host paths; the Web status projection must strip them. The install-outcome
// error field is part of the same error chain and is likewise desktop-only.
assert.match(remoteControlCommands,
  /project_acp_status_for_web[\s\S]*?status\.install_command = None[\s\S]*?status\.install_latest_line = None[\s\S]*?status\.install_finished = None/,
  'the Web agent-status projection must strip install command/output lines and the install outcome');
assert.match(remoteControlCommands, /redact_workspace_path_for_web|list_codex_acp_sessions_for_web/,
  'the Web session list must redact host workspace paths to a directory name');
assert.match(remoteControlCommands,
  /fn web_acp_agent_id[\s\S]*?or\(Some\("codex"\)\)[\s\S]*?support ACP agents only[\s\S]*?pub async fn web_access_create_codex_acp_session[\s\S]*?web_acp_agent_id\(agent_id\)\?/,
  'Web session creation must default to ACP and reject the desktop-only native agent before consuming a workspace grant');
assert.match(commands,
  /pub async fn list_codex_acp_sessions_for_web[\s\S]*?acp_pool\.is_acp_metadata\(metadata\)[\s\S]*?redact_codex_session_list_item_for_web/,
  'the Web code-session list must contain ACP sessions only');
assert.match(remoteControlCommands,
  /struct WebSavedSession[\s\S]*?metadata: deepseek_tui::session_manager::SessionMetadata[\s\S]*?messages:[\s\S]*?artifacts:[\s\S]*?transcript_revision:/,
  'chunked Web session downloads must use an explicit browser field allowlist');
assert.doesNotMatch(remoteControlCommands,
  /struct WebSavedSession[\s\S]{0,300}#\[serde\(flatten\)\]/,
  'the Web session projection must not flatten the full desktop SavedSession');
assert.match(remoteControlCommands,
  /impl<'a> WebSavedSession<'a>[\s\S]*?redact_session_metadata_for_web\(session\.metadata\.clone\(\)\)[\s\S]*?web_artifact_storage_path[\s\S]*?ledger_root\(&session_id\)[\s\S]*?session_artifacts_dir\(&session_id\)[\s\S]*?WebSavedSession::project\(&saved, &artifact_roots, &revision\)/,
  'the Web session projection must redact metadata and scope legacy artifact paths before serialization');
assert.match(webBridge,
  /invoke\("web_access_list_sessions"\)[\s\S]*?"web_access_list_archived_sessions"/,
  'Web history refreshes must use path-redacted session list commands');
assert.match(remoteControlCommands,
  /fn web_operation_result[\s\S]*?format!\("\{prefix\}_\{operation\}_failed"\)[\s\S]*?fn web_workspace_result[\s\S]*?web_operation_result\([\s\S]*?"web_workspace"[\s\S]*?web_access_list_codex_workspace[\s\S]*?web_workspace_result\(WebWorkspaceOperation::Listing, result\)[\s\S]*?web_access_search_codex_workspace[\s\S]*?web_workspace_result\(WebWorkspaceOperation::Search, result\)[\s\S]*?web_access_preview_codex_workspace_file[\s\S]*?web_workspace_result\(WebWorkspaceOperation::Preview, result\)[\s\S]*?web_access_get_codex_workspace_changes[\s\S]*?web_workspace_result\(WebWorkspaceOperation::Changes, result\)[\s\S]*?web_access_get_codex_workspace_diff[\s\S]*?web_workspace_result\(WebWorkspaceOperation::Diff, result\)/,
  'Web workspace RPC failures must not return host paths embedded in native errors');
assert.match(remoteControlCommands,
  /web_access_get_codex_acp_timeline[\s\S]*?web_acp_result\(WebAcpOperation::Timeline/,
  'Web ACP timeline failures must cross Relay as a controlled error code');
// Every web_access_* ACP command maps failures through a WebAcpOperation enum
// variant; the Rust test stable_web_error_codes_are_locked pins the wire codes.
for (const [command, variant] of Object.entries({
  web_access_create_codex_acp_session: 'SessionCreate',
  web_access_cancel_codex_acp: 'Cancel',
  web_access_codex_acp_prompt: 'Prompt',
  web_access_get_codex_acp_timeline: 'Timeline',
  web_access_get_codex_acp_session_info: 'SessionInfo',
  web_access_set_codex_acp_model: 'SetModel',
  web_access_set_codex_acp_mode: 'SetMode',
  web_access_set_codex_acp_config_option: 'SetConfigOption',
  web_access_get_codex_acp_pending_permissions: 'PendingPermissions',
  web_access_respond_codex_acp_permission: 'RespondPermission',
  web_access_get_codex_acp_pending_elicitations: 'PendingElicitations',
  web_access_respond_codex_acp_elicitation: 'RespondElicitation',
  web_access_list_codex_acp_sessions: 'ListSessions',
  web_access_list_acp_agents: 'ListAgents',
  web_access_get_acp_agent_status: 'AgentStatus',
})) {
  assert.match(
    rustCommandBlock(remoteControlCommands, command),
    new RegExp(`web_acp_result\\(WebAcpOperation::${variant}`),
    `${command} must map every failure to its stable Web ACP error code`,
  );
}
assert.match(remoteControlCommands,
  /project_acp_session_info_for_web[\s\S]*?project_acp_value_for_web\(serde_json::to_value\(info\)\?\)/,
  'successful Web SessionInfo responses must reuse the recursive ACP Web sanitizer');
assert.match(remoteControlPlatform,
  /struct WorkspaceIdentity[\s\S]*?device:\s*u64[\s\S]*?inode:\s*u64[\s\S]*?volume_serial:\s*u32[\s\S]*?file_index:\s*u64/,
  'the platform adapter must snapshot native directory identity on Unix and Windows');
assert.match(remoteControlManager,
  /fn verify_bound_path\([\s\S]*?bound_path != self\.path[\s\S]*?validate_browsable_path[\s\S]*?current != self\.path[\s\S]*?current_identity != self\.identity[\s\S]*?fn revalidate\(self\)[\s\S]*?self\.verify_bound_path/,
  'workspace grant redemption must reapply path policy and reject directory replacement');
assert.match(remoteControlCommands,
  /reservation\.path\(\)\.to_path_buf\(\)[\s\S]*?verify_bound_path\(path\)[\s\S]*?create_codex_acp_session_with_workspace_binding/,
  'Web Session creation must preserve the native path and carry its identity verifier into the final binding');
assert.match(commands,
  /create_codex_acp_session_with_workspace_binding[\s\S]*?verify_workspace_binding\(project_workspace\.as_deref\(\), workspace_verifier\)[\s\S]*?set_acp_workspace[\s\S]*?verify_workspace_binding\(project_workspace\.as_deref\(\), workspace_verifier\)[\s\S]*?capture_baseline[\s\S]*?verify_workspace_binding\(project_workspace\.as_deref\(\), workspace_verifier\)/,
  'workspace identity must be checked before persistence, after binding, and after baseline capture');
assert.equal(allowed.has('respond_codex_acp_permission'), false,
  'Web must not call the native ACP permission response command');
assert.equal(allowed.has('respond_codex_acp_elicitation'), false,
  'Web must not call the native ACP elicitation response command');
assert.equal(allowed.has('web_access_respond_codex_acp_permission'), true);
assert.equal(allowed.has('web_access_respond_codex_acp_elicitation'), true);
assert.match(acpPlatformClient,
  /respond_codex_acp_permission',[\s\S]*?'web_access_respond_codex_acp_permission'/,
  'permission responses must preserve the native command and use a stable Web wrapper');
assert.match(acpPlatformClient,
  /respond_codex_acp_elicitation',[\s\S]*?'web_access_respond_codex_acp_elicitation'/,
  'elicitation responses must preserve the native command and use a stable Web wrapper');
assert.doesNotMatch(codexView,
  /isWeb \? 'web_access_respond_/,
  'the view must route permission/elicitation responses through the acp client');
assert.match(acpErrors, /CONTROLLED_WEB_ERROR[\s\S]*?copy\.operationFailed/,
  'controlled Web error codes must become localized UI copy instead of raw browser text');

// Upload integrity failures must surface a stable wire code that the web
// client maps to trilingual copy; the Chinese raw text must no longer pass
// through the Relay to en/ja users (review P2).
// web_access_upload_attachment_chunk serves both plain sessions and the ACP
// code mode, so both display paths must recognize these two codes.
assert.match(remoteControlManager, /WEB_ATTACHMENT_DIGEST_INVALID: &str = "web_attachment_digest_invalid"/,
  'the malformed-digest failure must be a stable wire code');
assert.match(remoteControlManager, /WEB_ATTACHMENT_INTEGRITY_MISMATCH: &str = "web_attachment_integrity_mismatch"/,
  'the digest-mismatch failure must be a stable wire code');
assert.doesNotMatch(remoteControlManager, /远程控制附件完整性校验/,
  'single-language integrity errors must not cross the Relay');
assert.match(acpErrors, /'web_attachment_digest_invalid'[\s\S]*?'web_attachment_integrity_mismatch'/,
  'the session-level fallback must fold both codes into localized copy');
assert.match(webBridge, /rawUploadError === "web_attachment_digest_invalid"[\s\S]*?bt\("deviceUploadDigestInvalid"\)/,
  'the Web chat upload path must localize the malformed-digest code');
assert.match(webBridge, /rawUploadError === "web_attachment_integrity_mismatch"[\s\S]*?bt\("deviceUploadIntegrityMismatch"\)/,
  'the Web chat upload path must localize the mismatch code');
assert.equal((webBridge.match(/deviceUploadDigestInvalid:/g) || []).length, 3,
  'the malformed-digest copy must exist in all three BT_TABLE language blocks');
assert.equal((webBridge.match(/deviceUploadIntegrityMismatch:/g) || []).length, 3,
  'the mismatch copy must exist in all three BT_TABLE language blocks');
assert.match(codexView, /uploadErrorText === 'web_attachment_digest_invalid'[\s\S]*?uiAttachments\.deviceUploadDigestInvalid/,
  'the ACP attachment path must localize the malformed-digest code');
assert.match(codexView, /uploadErrorText === 'web_attachment_integrity_mismatch'[\s\S]*?uiAttachments\.deviceUploadIntegrityMismatch/,
  'the ACP attachment path must localize the mismatch code');
assert.equal((i18n.match(/deviceUploadDigestInvalid:/g) || []).length, 3,
  'the ACP malformed-digest copy must exist in all three i18n dictionaries');
assert.equal((i18n.match(/deviceUploadIntegrityMismatch:/g) || []).length, 3,
  'the ACP mismatch copy must exist in all three i18n dictionaries');

// After the watchdog skips a stalled predecessor, the web live stream shows
// an envelope-seq hole; the listener must detect the jump and debounce-refetch
// the authoritative timeline to self-heal (instead of waiting for a
// reconnect/session reopen), merging the snapshot and rebasing the tracker.
assert.match(codexView, /acpEventSeqTrackerRef\.current\.note\(incoming\.sessionId, incoming\.seq\) === 'gap'[\s\S]*?acpGapResyncRef\.current\.schedule\(incoming\.sessionId\)/,
  'the live acp:event listener must detect envelope-seq gaps and schedule a resync');
assert.match(codexView, /createAcpGapResyncScheduler\(sessionId => \{[\s\S]*?resyncAcpSessionAfterGap\(sessionId\);[\s\S]*?\}, \{/,
  'the gap resync must go through the bounded-retry scheduler wrapping resyncAcpSessionAfterGap');
assert.match(codexView, /mergeAcpTimelineSnapshot\(timeline, current, sessionId\)[\s\S]*?rebaseAcpEventSeqTracker\(sessionId, timeline\)/,
  'the gap resync must merge the authoritative snapshot and rebase the tracker');
assert.match(codexView, /acpGapResyncRef\.current\.cancel\(\)/,
  'the pending gap resync must be cancelled on unmount');

assert.match(bootstrap, /sendRaw\(\{ \.\.\.value, v: protocolVersion, lease_id: this\.leaseId \}\)/);
assert.match(bootstrap, /desktopCapabilitiesReady/);
assert.match(bootstrap, /SEMANTIC_COMMAND_REQUIREMENTS/);
assert.match(bootstrap, /supportsCapability\(capability\)/);
assert.match(bootstrap, /supportsCommand\(command\) \{\s*return this\.desktopCapabilitiesReady/,
  'individual RPC commands must remain unavailable while the desktop is offline');
assert.match(bootstrap, /if \(!this\.negotiatedCapabilitiesKnown\) return false/,
  'semantic capabilities must fail closed until the first authoritative snapshot');
assert.match(bootstrap, /this\.negotiatedCommands = new Set\(this\.allowedCommands\)/,
  'a negotiated compatibility snapshot must survive transient reconnects');
assert.match(bridge, /if \(IS_WEB && typeof PLATFORM\.can === "function"\) return PLATFORM\.can\(name\) === true/);
assert.match(hostFilePicker, /function rememberRoots\(listing\)/,
  'the Web host picker must retain the desktop-provided root inventory');
assert.match(hostFilePicker, /function showRoots\(\)/,
  'the Web host picker must expose an explicit root view');
assert.match(hostFilePicker, /rootsButton\.addEventListener\("click", showRoots\)/,
  'the root view must remain directly reachable from nested folders');
assert.match(hostFilePicker, /if \(parentPath\) load\(parentPath\);[\s\S]{0,100}else if \(!showingRoots\) showRoots\(\);/,
  'up from a filesystem root must return to the root inventory');
assert.doesNotMatch(hostFilePicker, /Array\.isArray\(listing\.roots\) && !parentPath/,
  'filesystem roots must not be mixed into a drive directory listing');
assert.match(hostFilePicker, /openWorkspace:/,
  'the host picker must expose a dedicated code-workspace selection flow');
assert.match(hostFilePicker, /issueWorkspaceHandle:\s*true/,
  'the one-shot host capability must be minted on confirm');
assert.match(hostFilePicker, /issueWorkspaceHandle:\s*false/,
  'directory browsing must not mint one-shot workspace handles');
assert.match(hostFilePicker, /(?:var|const|let) confirmedPath = currentPath;[\s\S]{0,1000}finish\(\{ path: confirmedPath, workspaceHandle: handle \}\);/,
  'workspace selection must finish with the click-time path and the handle minted for it');
assert.doesNotMatch(hostFilePicker, /currentWorkspaceHandle/,
  'browsing never carries a workspace handle, so no stale-handle fallback path may remain');
assert.match(hostFilePicker, /localizedPickerError/,
  'both the listing and confirm-mint failures must map stable authorization codes to localized copy');
assert.match(hostFilePicker, /web_session_list_host_files_failed[\s\S]{0,200}labels\.listingFailed/,
  'the round-21 stable browse/mint failure code must map to localized picker copy, not render as a bare code');
assert.match(hostFilePicker,
  /(?:var|const|let) confirmedGeneration = loadGeneration;[\s\S]{0,900}if \(disposed \|\| confirmedGeneration !== loadGeneration\) return;/,
  'a late confirm-mint failure must not touch the UI after the picker closed or a newer listing rendered');
assert.match(hostFilePicker,
  /\.then\(function \(listing\) \{[\s\S]{0,600}if \(disposed \|\| confirmedGeneration !== loadGeneration\) return;[\s\S]{0,200}finish\(\{ path: confirmedPath, workspaceHandle: handle \}\);/,
  'a late confirm-mint success must not close the picker on a directory the user already navigated away from');
assert.match(hostFilePicker, /mintInFlight = true;/,
  'the confirm handler must mark the mint as in flight before its RPC');
assert.match(hostFilePicker,
  /confirm\.disabled = mintInFlight \|\| \(directoryMode \? !currentPath : count === 0\);/,
  'navigation must not re-enable the confirm button while a mint is still in flight');
assert.match(hostFilePicker, /(?:var|const|let) generation = \+\+loadGeneration;\s*\n\s*mintInFlight = false;/,
  'navigation must supersede any in-flight mint instead of racing a second one');
assert.match(remoteControlManager,
  /HOST_WORKSPACE_NOT_AUTHORIZED:\s*&str\s*=\s*"host_workspace_not_authorized"[\s\S]*?Err\(HOST_WORKSPACE_NOT_AUTHORIZED\.to_string\(\)\)/,
  'the desktop and browser must share the stable host-workspace authorization error code');
for (const label of [
  '请先在桌面端允许远程访问本机目录',
  'Allow remote access to local folders on the desktop first',
  '先にデスクトップでローカルフォルダーへの遠隔アクセスを許可してください',
]) {
  assert.equal(i18n.includes(label), true, `missing localized host workspace error: ${label}`);
}
// Round-23 review MINOR: listingFailed previously existed only in the picker's
// built-in Chinese fallback table — the en/ja dictionaries lacked the key, so
// every language rendered the Chinese fallback. Pin the key's presence in all
// three languages on the same terms as workspaceNotAuthorized; deleting the
// dictionary key goes red.
for (const label of [
  'Listing desktop files failed. Please try again.',
  'デスクトップのファイル一覧の読み込みに失敗しました。もう一度お試しください。',
  '读取桌面端文件列表失败，请重试。',
]) {
  assert.equal(i18n.includes(label), true, `missing localized host file listing failure: ${label}`);
}
assert.match(appMain, /PinvouHostFilePickerStrings\s*=\s*misc\.hostFilePicker/,
  'the active locale must reach the Web host picker error branch');
assert.match(remoteControlCommands,
  /reserve_web_workspace_grant[\s\S]*?create_codex_acp_session[\s\S]*?restore_web_workspace_grant/,
  'failed Web ACP Session creation must restore its same-endpoint one-shot workspace reservation');
assert.match(hostFilePicker, /initialPathPending[\s\S]*?path === initialPath[\s\S]*?load\(null\)/,
  'a stale recent workspace must fall back to the normal host-file root instead of trapping the picker');
// HTML5 drop is claimed by the currently visible composer input, which then
// reuses the corresponding platform's upload channel.
assert.match(chatView, /enabled=\{bridge\.available && \(!isWeb \|\| can\('deviceFileUpload'\)\)\}/,
  'browser drop must gate on the negotiated device upload capability');
assert.match(chatView, /onFiles=\{files => bridge\.attachments\.uploadDeviceFiles\(files\)\}/,
  'normal chat drop must reuse the device upload pipeline');
assert.match(codexView, /onFiles=\{files => uploadDeviceFiles\(files, attachmentKey\)\}/,
  'ACP Code must own drops while its composer is visible');
assert.match(attachmentDropHook, /PinvouAttachmentDropController/);
assert.doesNotMatch(webBridge, /PinvouAttachmentDropController\.install/,
  'the Web bridge must not route Code drops into the hidden normal-chat draft');
assert.match(attachmentDropController, /dataTransfer\.dropEffect = "copy"/);
assert.match(attachmentDropController, /setActive\(true\)/);
assert.match(attachmentDropController, /setActive\(false\)/);
assert.match(bootstrap, /sendReady\(false\)/);
assert.match(bootstrap, /state_ready: stateReady/);
assert.match(bootstrap, /markStateReady\(\)/);
assert.match(bootstrap, /if \(!this\.frontendReady \|\| !this\.stateReady\)/);
const main = readSource(path.join(root, 'src', 'app', 'main.jsx'), 'utf8');
// The desktop-only saveSearchSettingsAndRestart wrapper (and its
// "unsupported by the Web host" stub) was removed from the web bridge as dead
// surface; the surviving invariant is that the web host never invokes the
// native-only restart command at all.
assert.doesNotMatch(webBridge, /invoke\("restart_app"/,
  'Web settings bridge must not invoke the native-only restart command');
assert.doesNotMatch(webBridge, /async function saveSearchSettingsAndRestart/,
  'the desktop-only restart wrapper must stay off the web bridge');
assert.match(main, /const saved = isWeb[\s\S]{0,180}saveSearchSettings\(search\)[\s\S]{0,180}saveSearchSettingsAndRestart\(search\)/,
  'the shared UI must save without requesting a desktop restart in WebUI');
assert.match(webBridge, /state\.settings = await invoke\("web_access_update_settings"/,
  'WebUI must keep the canonical settings returned by the desktop backend');
assert.match(webBridge, /web_access_update_settings", \{ patch: \{ (?:search: search|search) \} \}/,
  'WebUI search saves must send a narrow patch instead of a full settings snapshot');
assert.match(remoteControlCommands, /web_access_update_settings\([\s\S]{0,120}patch: super::settings::WebSettingsPatch,[\s\S]{0,80}\) -> Result<UserPrefs, String>/,
  'the bounded Web settings command must return canonical preferences');
assert.match(bootstrap, /pinvou:web-capabilities/);
// The collapsed/expanded footer buttons were consolidated into one shared
// renderer: one definition plus exactly one call per sidebar layout. A layout
// that stops calling the renderer, or a second inline copy of the buttons,
// changes the count.
assert.ok((main.match(/renderFooterButtons\(/g) || []).length === 2,
  'footer buttons must stay consolidated: one shared renderer called from both sidebar layouts');
assert.match(main, /\{!isSidebarOpen && renderFooterButtons\(true\)\}/,
  'the collapsed sidebar layout must render the shared footer buttons');
assert.match(main, /\{\s*renderFooterButtons\(false\)\s*\}/,
  'the expanded sidebar layout must render the shared footer buttons');
assert.match(main, /const renderFooterButtons = \(collapsed\) => \{[\s\S]{0,900}\{can\('webAccessAdmin'\) && <button[\s\S]{0,220}handleOpenWebAccess/,
  'desktop Web-access controls must stay hidden inside WebUI in both sidebar layouts');
assert.match(main, /const renderFooterButtons = \(collapsed\) => \{[\s\S]{0,1200}\{can\('pet'\) && <button[\s\S]{0,220}handleSetPetEnabled/,
  'desktop pet controls must stay hidden inside WebUI in both sidebar layouts');
assert.doesNotMatch(webBridge, /registerWebAccessDesktopProxy|web_access:rpc_request/,
  'the browser-only bridge must not own the desktop RPC proxy');
assert.match(desktopRemoteControlBridge, /async function startDesktopProxy\(\)/);
assert.match(desktopRemoteControlBridge, /listen\("web_access:rpc_request"/);
assert.match(desktopRemoteControlBridge, /invoke\("web_access_bridge_ready"/);
assert.match(desktopRemoteControlBridge, /eventForwardersReady/);
assert.match(bridge, /listen\("chat:user_message"/);
assert.match(bridge, /listen\("chat:transcript_committed"/);
assert.equal(allowedEvents.has('session:deleted'), true,
  'committed session deletion must reach every WebUI client');
assert.match(webBridge, /listen\("session:deleted"/);
assert.match(desktopSessionsBridge, /listen\("session:deleted"/,
  'the desktop session store must apply deletions initiated by WebUI');
assert.match(commands, /app\.emit\("session:deleted"/);
assert.match(commands, /forward_app_event\(&app, "session:deleted"/);
assert.equal(allowedEvents.has('session:list_changed'), true,
  'session list mutations must reach both WebUI and desktop clients');
assert.match(webBridge, /listen\("session:list_changed"/);
assert.match(desktopSessionsBridge, /listen\("session:list_changed"/);
assert.match(commands, /app\.emit\(event, payload\.clone\(\)\)/);
assert.match(commands, /forward_app_event\(app, event, payload\)/);
assert.match(webBridge, /composerDraft: ""/,
  'WebUI must keep a per-session in-memory composer draft');
assert.match(webDomainAdapter, /chat: domain\(\["sendMessage", "sendMessageToSession", "getComposerDraft", "setComposerDraft"/,
  'WebUI domain facade must expose the same composer draft API as desktop');
assert.match(webDomainAdapter, /auxChat: domain\(\[\], \{\s*ensure: "auxChatEnsure",\s*send: "auxChatSend",\s*snapshot: "auxChatSnapshot",\s*reset: "auxChatReset"/,
  'WebUI domain facade must expose the same auxChat domain as desktop');
// M7: the auxChat domain bodies live in the shared lane (cluster "auxChat");
// both lanes consume it and keep only their send dispatch. Renaming an
// operation in the shared cluster must turn these pins red.
assert.match(sharedBridgeHelpers, /"auxChat": function \(deps\)/,
  'the shared bridge base must register the auxChat cluster (M7)');
assert.match(sharedBridgeHelpers, /async function auxChatEnsure\(taskId\)/);
assert.match(webBridge, /PinvouBridgeShared\.create\("auxChat", \{/,
  'the Web lane must consume the shared auxChat cluster (M7)');
assert.match(desktopAuxChatBridge, /PinvouBridgeShared\.create\("auxChat", \{/,
  'the desktop lane must consume the shared auxChat cluster (M7)');
assert.match(sharedBridgeHelpers, /invoke\("get_or_create_aux_session", \{ sessionId: task \}\)/,
  'aux chat must create-or-fetch the aux session by task id');
assert.match(webBridge, /invoke\("web_access_chat", \{ message, attachmentHandles: \[\], sessionId: sid, restrictTools: true \}\)/,
  'WebUI aux chat sends must ride the bounded web chat command with tools restricted');
assert.match(sharedBridgeHelpers, /async function auxChatReset\(taskId\)/);
assert.match(sharedBridgeHelpers, /invoke\("reset_aux_session", \{ sessionId: task \}\)/,
  'aux chat restart must ride the atomic reset command (M6)');
// M5: the aux id is a pure function of the task id (aux-<taskId>, round-30
// B8), so both lanes derive it at the purge site — the redundant, never-pruned
// auxIdByTask maps are gone.
assert.doesNotMatch(webBridge, /auxIdByTask/,
  'the WebUI aux id must be derived, not stored in an unbounded per-task map (M5)');
assert.doesNotMatch(desktopAuxChatBridge, /auxIdByTask/,
  'the desktop aux id must be derived, not stored in an unbounded per-task map (M5)');
assert.match(bridge, /registry\.auxChat = function \(context\)/,
  'the desktop bridge must register the auxChat feature module');
assert.match(desktopAuxChatBridge, /invoke\("chat", \{ message, attachments: \[\], sessionId: sid, restrictTools: true \}\)/,
  'desktop aux chat sends must restrict tools and skip attachments');
// The lane-merge residual wave moved the working-set save/restore bodies into
// the shared base; the Web lane executes them through its cluster instance.
assert.match(sharedBridgeHelpers, /buf\.composerDraft = state\.composerDraft/,
  'WebUI session switching must save the active composer draft');
assert.match(sharedBridgeHelpers, /state\.composerDraft = buf\.composerDraft/,
  'WebUI session switching must restore the destination composer draft');
assert.match(webBridge, /(?:var|const|let) draftComposer = realId \? "" : \(state\.composerDraft \|\| ""\)/,
  'WebUI background session events must snapshot an unmaterialized draft');
assert.match(webBridge, /if \(!realId\) restoreBuffer\.composerDraft = draftComposer/,
  'WebUI background session events must restore an unmaterialized draft');
for (const eventName of ['session:model_changed', 'session:persona_changed']) {
  assert.equal(allowedEvents.has(eventName), true, `${eventName} must reach both clients`);
  assert.match(webBridge, new RegExp(`listen\\("${eventName.replace(':', '\\:')}"`));
  assert.match(desktopSessionsBridge, new RegExp(`listen\\("${eventName.replace(':', '\\:')}"`));
  assert.match(commands, new RegExp(`"${eventName.replace(':', '\\:')}"`));
}

function literalListeners(source) {
  return new Set([...source.matchAll(/\blisten\(\s*["']([^"']+)["']/g)].map(match => match[1]));
}
const webListenerNames = literalListeners(webBridge);
// Round-25 review MINOR: a non-empty extraction was never asserted — if the
// listen( wrapper drifted off the literal regex, webListenerNames silently
// became an empty set and the desktop-coverage cross-check below degenerated
// into a vacuous loop (the same hole as the createAcpSession anti-pin).
// Round-27 review m13: a floor of >=3 only buys "non-empty" — measured with
// 26 listeners, dropping 23 still kept the suite green, consistent with this
// file's round-15 M2 precedent of converging to an exact count (a floor
// tolerates deleting most of the guards while staying green). Switched to an
// exact census; adding/removing a listener must update this count.
assert.equal(webListenerNames.size, 26,
  'webBridge listener extraction must resolve the full literal-listener \
   census (26 at round-27); an empty set means the listen( wrapper \
   drifted off the regex and the desktop-coverage cross-check below is \
   vacuous, a smaller set means listeners were removed and the census \
   must be updated in the same commit');
const desktopListenerNames = literalListeners(desktopBridgeSources.join('\n'));
for (const eventName of webListenerNames) {
  assert.equal(desktopListenerNames.has(eventName), true,
    `desktop bridge must handle Web bridge event ${eventName}`);
}
assert.match(bridge, /Transcript persistence is authoritative in Rust/);
assert.doesNotMatch(bridge, /saveSessionMessagesForClient/);
assert.match(bridge, /session_turn_in_progress/);
assert.match(bridge, /turnAlreadyInProgress/);
assert.match(bridge, /addSystemItem\(concurrentTurn[\s\S]{0,120}bt\("turnAlreadyInProgress"\)/,
  'turn admission conflicts must show product copy instead of an internal reservation error');
// The fold must live in the WEB bridge itself: pins scoped to the concatenated
// `bridge` are satisfied by the desktop twin even with the web mapping deleted
// (round-4 mutation M7 stayed green).
assert.match(webBridge, /web_session_turn_in_progress:\s*"turnAlreadyInProgress"/,
  'the web bridge itself must map the turn-in-progress code to product copy');
assert.match(webBridge, /addSystemItem\(concurrentTurn[\s\S]{0,120}bt\("turnAlreadyInProgress"\)/,
  'the web-side turn admission conflict must show product copy');
// Round-10 review minor: accept_plan/edit_last_turn do not fold; their error
// chains carry the caller's optional session id — a bare
// `session_turn_in_progress` substring match would fold a hard failure of a
// same-named session into a retryable busy (deadlocking reconciliation). The
// two catches must anchor on their own reservation segments (introduced
// exclusively by the producer's map_err; the session id character set cannot
// forge the space/colon).
assert.match(webBridge,
  /includes\("reserve accept_plan turn: session_turn_in_progress"\)/,
  'the accept_plan catch must anchor on its reservation segment');
assert.match(webBridge,
  /includes\("reserve edit_last_turn: session_turn_in_progress"\)/,
  'the edit_last_turn catch must anchor on its reservation segment');
assert.doesNotMatch(webBridge,
  /includes\("session_turn_in_progress"\)/,
  'no web-bridge busy match may stay on the bare, session-id-forgeable marker');
// Round-9 review: the download lane's fold previously had zero test pins —
// removing the fallback code-table lookup or any table entry kept the suite
// green. Pin: the download catch goes through displayTurnError (unrecognized
// text enters the same redacted fallback as the turn lane, no longer
// concatenating the raw code/admission text); the two codes reachable from
// the admission layer are in the code table; new entries carry all three
// languages.
assert.match(
  webBridge,
  /addSystemItem\(bt\("downloadFailed"\) \+ displayTurnError\(e\)\)/,
  'the download lane must fold through displayTurnError instead of appending the raw code',
);
assert.doesNotMatch(
  webBridge,
  /copyKey \? bt\(copyKey\) : code/,
  'the download lane must not keep the raw-code fallback',
);
assert.match(webBridge, /web_session_unavailable:\s*"webSessionUnavailable"/,
  'the transient scope rejection must map to trilingual download copy');
assert.match(webBridge, /invalid_web_session_id:\s*"webSessionUnavailable"/,
  'the invalid-session rejection must map to trilingual download copy');
for (const source of [webBridge]) {
  const count = [...source.matchAll(/webSessionUnavailable:/g)].length;
  assert.equal(count, 3,
    'the webSessionUnavailable copy must exist in all three inline languages');
}
// Round-12 review P2: of the WEB_TURN_ERROR_KEYS mapping keys, previously only
// webSessionUnavailable above had a single-key count pin; for the other keys
// (the sentences added for en/ja), a missing entry in any language block made
// bt() silently fall back to zh — en/ja users saw the Chinese fallback with no
// test going red. Parse the code-table key list from the bridge source and pin,
// key by key, that the BT_TABLE's three language blocks each contain a
// `key:` definition.
{
  const turnErrorTableStart = webBridge.indexOf('const WEB_TURN_ERROR_KEYS = {');
  assert.notEqual(turnErrorTableStart, -1,
    'the WEB_TURN_ERROR_KEYS mapping must exist in the web bridge');
  const turnErrorTableEnd = webBridge.indexOf('};', turnErrorTableStart);
  assert.notEqual(turnErrorTableEnd, -1,
    'the WEB_TURN_ERROR_KEYS mapping must have a closing brace');
  const turnErrorCopyKeys = [
    ...new Set(
      [
        ...webBridge
          .slice(turnErrorTableStart, turnErrorTableEnd)
          .matchAll(/:\s*"([A-Za-z]+)"/g),
      ].map((match) => match[1]),
    ),
  ];
  assert.ok(turnErrorCopyKeys.length >= 11,
    `expected the full WEB_TURN_ERROR_KEYS copy surface, found ${turnErrorCopyKeys.length}`);
  const btTableStart = webBridge.indexOf('const BT_TABLE = {');
  assert.notEqual(btTableStart, -1,
    'the BT_TABLE copy table must exist in the web bridge');
  const btTableEnd = webBridge.indexOf('\n  };', btTableStart);
  assert.notEqual(btTableEnd, -1,
    'the BT_TABLE copy table must have a closing brace');
  const btTable = webBridge.slice(btTableStart, btTableEnd);
  const btEnStart = btTable.indexOf('    en: {');
  const btJaStart = btTable.indexOf('    ja: {');
  const btZhStart = btTable.indexOf('    zh: {');
  assert.notEqual(btEnStart, -1, 'the BT_TABLE en block marker must exist');
  assert.notEqual(btJaStart, -1, 'the BT_TABLE ja block marker must exist');
  assert.notEqual(btZhStart, -1, 'the BT_TABLE zh block marker must exist');
  const btLanguageBlocks = [
    ['en', btTable.slice(btEnStart, btJaStart)],
    ['ja', btTable.slice(btJaStart, btZhStart)],
    ['zh', btTable.slice(btZhStart)],
  ];
  for (const [language, block] of btLanguageBlocks) {
    for (const key of turnErrorCopyKeys) {
      assert.match(
        block,
        new RegExp(`\\b${key}:`),
        `WEB_TURN_ERROR_KEYS maps to copy key "${key}", but the BT_TABLE ${language} block has no ${key}: definition (bt() silently falls back to zh for a missing key)`,
      );
    }
  }
}
// The JS half of the edit_last_turn revision guard: dropping or renaming the
// arg would silently degrade the stale-view protection to fail-open, and the
// Rust-side Option param alone cannot catch it (round-4 minor).
assert.match(webBridge, /expectedTranscriptRevision: expectedRevision \|\| null/,
  'the web bridge must send its transcript revision with edit_last_turn');
const memoryCommands = readSource(
  path.join(root, 'src-tauri', 'src', 'app', 'commands', 'memory.rs'),
  'utf8',
);
assert.match(memoryCommands, /expected_transcript_revision: Option<String>/,
  'edit_last_turn must keep the optional transcript-revision guard parameter');
assert.match(bridge, /(?:var|const|let) sid = state\.activeSessionId;/);
assert.match(bridge, /if \(state\.activeSessionId !== sid\) return;/);
assert.match(bridge, /remoteAdmissionKeys/);
assert.match(bridge, /(?:var|const|let) activePlanCards = Object\.create\(null\)/);
assert.match(bridge, /(?:var|const|let) hydratedKey = planCardHydrationKey\(hydratedPlan\)/);
assert.match(bridge, /hydratedPlan\.cardState = "active"/);
// Wave 5 moved the hydration live-card filter into the shared base
// (liveChatItemKeptForHydration); pin the shared body + the lane delegation.
assert.match(bridge, /liveChatItemKeptForHydration/);
assert.match(readSource(path.join(root, 'src', 'shared', 'bridge-shared-helpers.js'), 'utf8'), /if \(item\.type === "plan_card"\) return false/);
assert.match(bridge, /action === "accept_plan"/);
assert.match(bridge, /acceptedMode = payload\.mode_state \|\| payload\.modeState/);
assert.match(bridge, /planNotActive = errorText\.(?:indexOf\("plan_not_active"\)|includes\("plan_not_active"\))/);
assert.match(bridge, /planId = String\(p\.plan_id \|\| p\.planId \|\| ""\)\.trim\(\)/);
assert.match(bridge, /readyMode = p\.mode_state \|\| p\.modeState/);
assert.match(bridge, /listen\("chat:plan_resolved"/);
assert.equal(allowedEvents.has('chat:plan_resolved'), true, 'plan resolution must reach the WebUI event bridge');
assert.match(bridge, /planId: planTicket/);
assert.match(bridge, /invoke\("discard_plan", \{ sessionId: sid, planId: planTicket \}\)/);
const discardPlanBody = bridge.slice(bridge.indexOf('async function discardPlan'), bridge.indexOf('async function exitPlanToYolo'));
assert.ok(discardPlanBody.indexOf('notify();') < discardPlanBody.indexOf('await invoke("discard_plan"'),
  'discard Plan must notify the frozen card before waiting on the remote invoke');
assert.match(bridge, /function isActionablePlanCard\(sid, itemId, planId\)/);
assert.match(bridge, /else if \(!card\.planResolutionConfirmed\)/);
assert.match(toolRenderers, /!item\.resolved && !!item\.planId/);
assert.match(toolRenderers, /acceptPlan\(item\.id, item\.planMarkdown, undefined, item\.planId\)/);
assert.match(toolRenderers, /discardPlan\(item\.id, item\.planId\)/);
assert.match(bridge, /restoreUiTurnState\(preparation\.snapshot\)/);
assert.match(bridge, /attachmentHandles:/);
assert.match(bridge, /web_access_load_session_chunk/);
assert.match(bridge, /web_access_cancel_session_download/);
assert.match(bootstrap, /areInvokeCapabilitiesReady\(\) \{ return client\.desktopCapabilitiesReady === true; \}/,
  'the shared bridge must reuse bootstrap capability-handshake state');
const capabilitiesWaitAt = webBridge.indexOf('await waitForWebInvokeCapabilities();');
const downloadChoiceAt = webBridge.indexOf(
  'supportsSessionDownloadCancellation = canInvoke("web_access_cancel_session_download")',
);
assert.ok(capabilitiesWaitAt >= 0, 'the capability wait must exist in the web bridge');
assert.ok(downloadChoiceAt >= 0, 'the download protocol choice must exist in the web bridge');
assert.ok(
  capabilitiesWaitAt < downloadChoiceAt,
  'session downloads must not choose new or legacy protocol before the desktop snapshot',
);
assert.match(bridge, /pinvou:web-capabilities/);
assert.match(bridge, /desktop_capabilities_timeout/);
assert.match(bridge, /desktop_capabilities_unavailable/);
assert.match(bridge, /scheduleAbandonedSessionDownloadCleanup\(\)/,
  'connection recovery must retry persisted lease cleanup without another session switch');
assert.match(bridge, /retryCapabilityBlockedSessionSwitch\(\)/,
  'a late capability snapshot must retry the switch that timed out');
assert.match(bridge, /supportsSessionDownloadCancellation = canInvoke\("web_access_cancel_session_download"\)/,
  'new lease behavior must be gated by the installed desktop command capability');
assert.match(bridge, /if \(supportsSessionDownloadCancellation\) \{[\s\S]{0,120}await cleanupAbandonedSessionDownloads/,
  'legacy desktops must skip persisted lease cleanup they cannot implement');
assert.match(bridge, /downloadId = supportsSessionDownloadCancellation \? newSessionDownloadId\(\) : ""/,
  'legacy desktops must begin without a client-selected download id');
assert.match(bridge, /if \(supportsSessionDownloadCancellation && !offset\) \{[\s\S]{0,100}chunkArgs\.requestedDownloadId = downloadId/,
  'requestedDownloadId must only be sent to desktops that support cancellable leases');
assert.match(bridge, /if \(!downloadId\) downloadId = chunkDownloadId/,
  'the legacy path must adopt the server-generated first-chunk download id');
assert.match(bridge, /if \(supportsSessionDownloadCancellation && downloadId\) \{/,
  'legacy failures must not enter cancellable lease cleanup');
assert.match(bridge, /cancellationIds[\s\S]{0,500}cancelSessionDownloadLease\(cancellationId, sid\)/,
  'cancellable failures must release every known lease id');
assert.doesNotMatch(
  webBridge,
  /web_access_load_session_chunk[\s\S]{0,220}\blimit\s*:/,
  'WebUI must let each desktop version choose its supported session chunk size',
);
assert.match(bridge, /MAX_WEB_ARTIFACT_DOWNLOAD_BYTES = 256 \* 1024 \* 1024/);
assert.match(bridge, /if \(IS_WEB && !hasCapability\("artifactDownload"\)\)/);
assert.match(bridge, /(?:var|const|let) info = await artifactInfo\(path, resolvedSessionId\)/);
assert.match(bridge, /if \(expectedSize > MAX_WEB_ARTIFACT_DOWNLOAD_BYTES\)/);
// F-XT7-7: the per-part overflow re-check lives in the shared
// streamBase64PartsToBlob core (maxBytes-parameterized); both download
// pipelines must route through it.
assert.match(bridge, /streamBase64PartsToBlob\(function \(offset\)/);
assert.match(bridge, /if \(bytes\.length > maxBytes - offset\)/);
// Round-2 review minor: the maxBytes ARGUMENT at each call site is
// load-bearing — the core re-checks are only as strong as the binding passed
// in (e.g. Infinity would silently disarm them), so both download pipelines
// must pass the module cap explicitly, exactly once each.
assert.equal(
  (bridge.match(/\}, expectedSize, MAX_WEB_ARTIFACT_DOWNLOAD_BYTES, \{/g) || []).length,
  1,
  'the artifact download pipeline must pass MAX_WEB_ARTIFACT_DOWNLOAD_BYTES as maxBytes',
);
assert.equal(
  (bridge.match(/\}, null, MAX_WEB_ARTIFACT_DOWNLOAD_BYTES, \{/g) || []).length,
  1,
  'the attachment download pipeline must pass MAX_WEB_ARTIFACT_DOWNLOAD_BYTES as maxBytes',
);
assert.match(bridge, /streamBase64PartsToBlob\(function \(offset\) \{\n {6}return invoke\("web_access_read_conversation_attachment_chunk"/);
assert.match(artifactsPanel, /const canDownloadArtifacts = can\('artifactDownload'\);/);
assert.ok((artifactsPanel.match(/\(!isWeb \|\| canDownloadArtifacts\)/g) || []).length >= 2,
  'WebUI artifact download buttons must hide when the installed desktop lacks download support');
assert.match(commands, /claim_pending_plan\(&session_id, &plan_id\)/);
assert.match(commands, /restore plan claim failed/);
assert.match(bridge, /function armWebInitRetry\(\)/);
assert.match(bridge, /window\.addEventListener\("pinvou:web-connection", webInitRetryHandler\)/);
assert.match(bridge, /if \(client && !client\.stateReady\) \{[\s\S]{0,120}initPromise = null/);

// UI mutation affordances must follow the browser capability allowlist while
// leaving desktop defaults and per-session model switching intact.
assert.match(settingsView, /const canManageModels = can\('modelManagement'\);/);
const composerShared = readSource(path.join(root, 'src', 'features', 'settings', 'composer-shared.jsx'), 'utf8');
assert.match(composerShared, /const canSwitchModels = can\('sessionModelSwitch'\);/);
assert.match(composerShared, /const canMutateToolStore = can\('toolStoreMutations'\);/);
assert.match(composerShared, /const toolSwitchDisabled = !canMutateToolStore;/);
// Add-only plus revocable-before-commit: while a session is active, switching
// a tool "off" is blocked, but a tool opened within the current session
// (pending) that has not yet entered context with a new round may be switched
// back; it locks only after the backend accepts the new round.
assert.match(composerShared, /if \(toolSwitchDisabled \|\| \(hasActiveSession && enabled && !pending\.ids\.has\(id\)\)\) return;/);
assert.match(composerShared, /if \(toolSwitchDisabled \|\| \(hasActiveSession && projectSkillsEnabled && !pending\.projectSkills\)\) return;/);
assert.match(composerShared, /if \(enabled\) pending\.ids\.delete\(id\); else pending\.ids\.add\(id\);/);
assert.match(composerShared, /window\.addEventListener\('pinvou:chat-round-committed', onCommitted\)/);
assert.match(composerShared, /pending\.ids\.clear\(\);\s*\n\s*pending\.projectSkills = false;/);
// The commit (clearing pending) must happen in a module-level listener, not
// only inside a component effect: the acceptance event can arrive after the
// menu component has unmounted (queued messages flushing after the user
// switches pages, background directed sends), and a component-level listener
// would miss the clear, leaving pending dangling after remount with tools
// already in context still showing as switchable.
// The module-level listener registration must also precede the component
// definition so the clear happens before the component's bump re-render.
const moduleListenerIdx = composerShared.indexOf(
  "window.addEventListener('pinvou:chat-round-committed', (event)",
);
assert.ok(
  moduleListenerIdx >= 0 && moduleListenerIdx < composerShared.indexOf('const ComposerToolMenu'),
  'round-committed pending-clear listener must be registered at module level (before ComposerToolMenu), not only inside the component',
);
// The commit signal is dispatched by the send chain after the backend accepts
// the new round: ordinary sends (desktop/web doSendFor), the Web first-turn
// commit, accepting a plan (desktop/web acceptPlan), edit-and-rerun
// (desktop/web editLastTurn), and the native code lane's send and accept-plan
// — missing any of these sites leaves pending uncommitted.
// FFE4-1: the raw dispatch exists only in shared/tool-store-events.js; the
// platform bridge layer's 7 send/accept/rerun paths must call through
// window.PinvouToolStoreEvents.
const toolStoreEventsShared = readSource(path.join(root, 'src', 'shared', 'tool-store-events.js'), 'utf8');
assert.match(toolStoreEventsShared, /pinvou:chat-round-committed/);
assert.match(toolStoreEventsShared, /pinvou:tools-changed/);
assert.match(
  bridge,
  /window\.PinvouToolStoreEvents\.notifyChatRoundCommitted\(\)/,
  'platform bridges must dispatch the round-committed event through the shared helper',
);
assert.ok(
  (bridge.match(/PinvouToolStoreEvents\.notifyChatRoundCommitted\(\)/g) || []).length >= 7,
  'desktop doSendFor/acceptPlan/editLastTurn and web doSendFor/first-turn/acceptPlan/editLastTurn must each dispatch the round-committed event',
);
assert.ok(
  (bridge.match(/PinvouToolStoreEvents\.notifyComposerToolsChanged\(\)/g) || []).length >= 1,
  'the desktop bridge must dispatch the tools-changed refresh through the shared helper',
);
assert.ok(
  (codexView.match(/notifyChatRoundCommitted\('code'\);/g) || []).length >= 2,
  'native code lane send and accept-plan must each commit pending enables',
);
const toolEvents = readSource(path.join(root, 'src', 'features', 'tools', 'tool-events.js'), 'utf8');
assert.match(toolEvents, /export \{ notifyComposerToolsChanged, notifyChatRoundCommitted \};/);
assert.match(composerShared, /bridge\.models\.switchModel\(activeSessionId, id\)/);
assert.match(settingsView, /\{canManageModels && editingModel && \(/);
assert.match(toolStoreView, /if \(!can\('toolStoreMutations'\)\) \{/);
assert.match(toolStoreView, /const canMutateToolStore = can\('toolStoreMutations'\);/);
// Round-15 review M2: a floor of >=4 would tolerate deleting 4 of the 8 guards
// while staying green; converge to an exact count and update the pin when a
// guard is added (the same convention as this file's other exact-count pins).
// Round-21 review (disclosed blind spots): this census counts only the two
// "pure" guard shapes (`!canMutateToolStore` and `|| busyRef.current`);
// compound-predicate guards (`|| !somethingElse`, 5 present today) and
// brace-form guards (`{ return; }`) are not counted; and the count is
// file-wide, so a deletion at one site cancels an addition at another. New
// guards landing in those shapes need their own pins; this count cannot be
// relied on as a backstop for them.
assert.equal(
  (toolStoreView.match(/if \(!canMutateToolStore(\s*\|\|\s*busyRef\.current)?\) return;/g) || []).length,
  8,
  'all tool install, uninstall, and import handlers must fail closed in WebUI; \
   if a guard was deliberately removed or added, update this count consciously');
// Recycle-bin Web read-only downgrade: list_recycled_plugins is a read-only
// command (allowed by the access policy); the restore/export/purge action
// buttons are gated wholesale behind canMutateToolStore, and the handlers
// themselves fail closed (including purge — its confirmation dialog entry is
// triggered by a gated button, but the handler must still guard itself).
assert.match(toolStoreView,
  /\{canMutateToolStore && \(\s*<div className="flex items-center gap-2 shrink-0">[\s\S]{0,900}recycled-restore-[\s\S]{0,900}recycled-export-[\s\S]{0,900}recycled-purge-/,
  'recycle bin restore/export/purge buttons must be gated behind canMutateToolStore');
assert.match(toolStoreView,
  /const handleRestoreRecycled = async \(item\) => \{\s*if \(!canMutateToolStore/,
  'restore handler must fail closed in WebUI');
assert.match(toolStoreView,
  /const handleExportRecycled = async \(item\) => \{\s*if \(!canMutateToolStore/,
  'recycled export handler must fail closed in WebUI');
assert.match(toolStoreView,
  /const doPurgeRecycled = async \(\) => \{\s*if \(!canMutateToolStore/,
  'purge handler must fail closed in WebUI');
assert.match(knowledgeView, /const canDownloadArtifacts = !isWeb \|\| can\('artifactDownload'\);/);
assert.match(knowledgeView, /const canPickHostFiles = !isWeb \|\| can\('hostFilePicker'\);/);
assert.match(knowledgeView, /const outputSessionId = o\.sessionId \|\| null;/);
assert.match(knowledgeView, /const cacheKey = `\$\{outputSessionId \|\| ''\}\|\$\{o\.path\}\|\$\{o\.mtime \|\| 0\}`;/);
assert.ok((knowledgeView.match(/o\.path, outputSessionId/g) || []).length >= 5,
  'output previews must authorize every Web artifact read with the owning session');
assert.match(knowledgeView, /<FilePreviewModal path=\{outputPreview\.path\} sessionId=\{outputPreview\.sessionId\}/);
assert.match(
  knowledgeView,
  /if \(isWeb\) \{ setPv\(\{ kind: 'fallback' \}\); return \(\) => \{ alive = false; \}; \}/,
  'Web builds must render the fallback card instead of reading local-knowledge host paths',
);
// Note: the Web gate above keeps the grid's OutputLivePreview from scheduling artifact
// reads at all; the session-authorized reads asserted above stay desktop-only for the
// grid and remain server-scoped (web_access_* commands) for the user-initiated
// FilePreviewModal, whose reads the backend still authorizes against the session.
assert.match(settingsView, /const canPickHostFiles = can\('hostFilePicker'\);/);
assert.match(toolCommon, /const canOpenArtifact = !isWeb \|\| can\('artifactDownload'\);/);
assert.match(connectionStatus, /incompatible_desktop/);
assert.match(connectionStatus, /BLOCKING[\s\S]*incompatible_desktop/);
assert.match(connectionStatus, /copy\[connection\.status\] \? fallback : \(connection\.message \|\| fallback\)/,
  'known connection states must use localized copy while unknown states retain diagnostics');
assert.doesNotMatch(connectionStatus, /connection\.status === 'error'.*connection\.message/s,
  'known error states must not expose unlocalized backend diagnostics');
assert.match(settingsView, /remoteCopy = t\.uiRemote/);
assert.match(settingsView, /\{remoteCopy\.title\}/);
assert.match(settingsView, /\{remoteCopy\.link\}/);
assert.match(settingsView, /\{remoteCopy\.refresh\}/);
// Enable and allow-folders must stay on separate handlers (regression: one shared
// handler made a single Enable click silently latch allow_host_workspace consent):
// the Enable handler starts with an empty options object, and the explicit
// allow-folders handler is the only place that passes allowHostWorkspace: true.
assert.match(settingsView, /async function handleEnableWebAccess\(\) \{[\s\S]{0,220}startRemoteControl\(\{\}\)/,
  'Enable Access must start remote control without host workspace consent');
assert.match(settingsView, /async function handleAllowHostWorkspace\(\) \{[\s\S]{0,220}startRemoteControl\(\{ allowHostWorkspace: true \}\)/,
  'host workspace access must follow an explicit allow-folders action in the desktop modal');
assert.match(settingsView, /onClick=\{handleAllowHostWorkspace\}[\s\S]{0,400}\{remoteCopy\.allowWorkspace\}/,
  'the allow-folders button must keep its dedicated consent handler');
assert.match(settingsView, /onClick=\{handleEnableWebAccess\}[\s\S]{0,400}\{remoteCopy\.enable\}/,
  'the Enable button must use the consent-free start handler');
assert.doesNotMatch(settingsView, /useEffect\(\(\) => \{[\s\S]{0,240}startRemoteControl/,
  'opening the remote-control modal must not silently authorize host workspace access');
assert.match(desktopRemoteControlBridge, /allowHostWorkspace: !!\(options && options\.allowHostWorkspace\)/,
  'the desktop bridge must carry explicit host-workspace consent');
// Folder consent must stay visible and individually revocable while the link
// is active: consent latched by the pre-split single-button UI can be seen
// and taken back without stopping or rotating the whole link.
assert.match(settingsView, /async function handleRevokeHostWorkspace\(\) \{[\s\S]{0,220}revokeRemoteHostWorkspace\(\)/,
  'the modal must expose a dedicated revoke-folder-consent handler');
assert.match(settingsView, /webAccessActive && hostWorkspaceAuthorized && \([\s\S]{0,420}\{remoteCopy\.workspaceAllowed\}[\s\S]{0,300}onClick=\{handleRevokeHostWorkspace\}/,
  'folder authorization must render an affirmative indicator wired to the revoke handler');
assert.match(settingsView, /\{remoteCopy\.revokeWorkspace\}/,
  'the revoke control must carry explicit trilingual copy');
assert.doesNotMatch(settingsView, /handleRevokeHostWorkspace[\s\S]{0,200}allowHostWorkspace: true/,
  'revoking folder consent must never re-grant it');
// Round-12 review P3: the uiRemote close / workspaceAllowed / revokeWorkspace
// keys previously had no trilingual in-place pins. `close:` shares its name
// with other sections in each language file (uiToolStore, the subagent drawer,
// uiCodexWorkspace, etc. — 7 occurrences per file), so a bare contains would
// mis-match — slice each uiRemote section by its braces and pin each key's
// presence; deleting any key in any language goes red.
const i18nByLanguage = {
  en: readSource(path.join(root, 'src', 'shared', 'i18n', 'en.js'), 'utf8'),
  ja: readSource(path.join(root, 'src', 'shared', 'i18n', 'ja.js'), 'utf8'),
  zh: readSource(path.join(root, 'src', 'shared', 'i18n', 'zh.js'), 'utf8'),
};
for (const [language, source] of Object.entries(i18nByLanguage)) {
  const uiRemoteStart = source.indexOf('uiRemote: {');
  assert.notEqual(uiRemoteStart, -1,
    `the ${language} i18n dictionary must define a uiRemote section`);
  const uiRemoteEnd = source.indexOf('\n        },', uiRemoteStart);
  assert.notEqual(uiRemoteEnd, -1,
    `the uiRemote section of the ${language} i18n dictionary must have a closing brace`);
  const uiRemoteBlock = source.slice(uiRemoteStart, uiRemoteEnd);
  for (const key of ['close', 'workspaceAllowed', 'revokeWorkspace']) {
    assert.match(
      uiRemoteBlock,
      new RegExp(`\\b${key}:`),
      `the uiRemote section of the ${language} i18n dictionary must define ${key}: (missing copy breaks the settings remote-control modal)`,
    );
  }
}
// Registration rejections must surface trilingual recovery copy through the
// stable last_error_code; the raw English last_error is fallback only
// (regression: the actionable hints shipped as English-only raw strings).
assert.match(settingsView, /webAccess\.last_error_code\s*\?\s*remoteCopy\.registerHints\?\.\[webAccess\.last_error_code\]/,
  'the settings modal must map the registration error code to localized recovery copy');
assert.match(settingsView, /registerHint \|\| webAccess\.last_error/,
  'unmapped errors must fall back to the raw last_error text');
assert.equal((i18n.match(/rotate_credentials:/g) || []).length, 3,
  'the rotate-credentials recovery copy must exist in all three languages');
assert.equal((i18n.match(/rotate_recover_config:/g) || []).length, 3,
  'the rotate-recover recovery copy must exist in all three languages');
assert.equal((i18n.match(/update_desktop:/g) || []).length, 3,
  'the update-desktop recovery copy must exist in all three languages');
assert.match(desktopRemoteControlBridge, /invoke\("web_access_revoke_host_workspace"\)/,
  'the desktop bridge must call the desktop-only revoke command');
// Round-6 review M6: round-5's "optimistic merge clears the code" ledger must
// hold branch by branch — error writes and code clearing come in pairs. All
// ten code-clearing points in remote-control.js are pinned point by point
// here: refresh failure, the optimistic start, start success/failure, stop
// success/failure, rotate success/failure, revoke merge/failure. Two of them
// are byte-identical twin shapes — the { last_error: String(error), ... }
// single line shared by the refresh/revoke failures, and the status: "error"...
// single line shared by the stop/rotate failures — which cannot be aimed at
// one side by text, so a global count assertion guards both sides and deleting
// either goes red; the remaining points match the unique shape of each merge
// object. The revoke merge must additionally derive active from whether the
// url exists — marking an inactive state as online must not happen, and
// reverting to a forced active: true goes red.
assert.match(desktopRemoteControlBridge, /\{ starting: true, last_error: null, last_error_code: null \}/,
  'the optimistic start merge must clear the stale recovery code');
assert.match(desktopRemoteControlBridge,
  /active: true, starting: false, last_error: null, last_error_code: null,/,
  'the start success merge must clear the stale recovery code');
assert.match(desktopRemoteControlBridge,
  /active: wasActive, web_client_connected: wasActive && !!state\.webAccess\.web_client_connected, starting: false,\n {10}status: "error", last_error: String\(error\), last_error_code: null,/,
  'the start failure merge must clear the stale recovery code');
assert.equal(
  (desktopRemoteControlBridge.match(/status: "error", last_error: String\(error\), last_error_code: null, starting: false \}/g) || []).length, 2,
  'the stop and rotate failure merges are byte-identical twins; both must keep clearing the stale recovery code');
assert.match(desktopRemoteControlBridge,
  /status: "stopped", starting: false,\n {8}last_error: null, last_error_code: null,/,
  'the stop success merge must clear the stale recovery code');
assert.match(desktopRemoteControlBridge,
  /active: true, web_client_connected: false, last_error: null, last_error_code: null, starting: false,/,
  'the rotate success merge must clear the stale recovery code');
assert.match(desktopRemoteControlBridge,
  /active: !!\(info && info\.url\), starting: false,\n {10}host_workspace_authorized: false, last_error: null, last_error_code: null,/,
  'the revoke merge must derive active from the url and clear the stale recovery code');
assert.equal(
  (desktopRemoteControlBridge.match(/\{ last_error: String\(error\), last_error_code: null \}/g) || []).length, 2,
  'the status-refresh failure merge and the revoke failure merge must both clear the stale code');
assert.match(remoteControlCommands, /web_access_revoke_host_workspace\([\s\S]{0,240}require_main_webview\(&window\)\?/,
  'only the main desktop WebView may revoke folder consent');
assert.match(remoteControlCommands, /web_access_enable\([\s\S]{0,240}require_main_webview\(&window\)\?/,
  'only the main desktop WebView may enable a persistent remote endpoint');
assert.match(
  remoteControlCommands,
  /web_access_status\([\s\S]{0,260}require_main_webview\(&window\)\?/,
  'web_access_status carries the token-bearing pairing URL and must stay main-WebView only',
);
assert.match(
  remoteControlCommands,
  /web_access_rotate\([\s\S]{0,240}require_main_webview\(&window\)\?/,
  'rotation rewrites link credentials and must stay main-WebView only',
);
// Round-8 review: the main-WebView gates of disable / relay_settings /
// list_host_files previously had no source-shape pin, so deleting any gate
// line stayed green; fill in the same shape as the four pins above.
assert.match(remoteControlCommands, /web_access_disable\([\s\S]{0,240}require_main_webview\(&window\)\?/,
  'only the main desktop WebView may stop a persistent remote endpoint');
assert.match(
  remoteControlCommands,
  /web_access_list_host_files\([\s\S]{0,400}require_main_webview\(&window\)\?/,
  'host filesystem browsing must stay main-WebView only',
);
// Round-11 review M4: publish_event received the Round-10 policy anti-pin,
// but its main-WebView gate line is the only management command without a
// Round-8-style source-shape pin — the gate is the only line of defense on
// the desktop aux WebView side (the policy anti-pin guards the other side's
// callers).
assert.match(
  remoteControlCommands,
  /web_access_publish_event\([\s\S]{0,240}require_main_webview\(&window\)\?/,
  'the management event-publish channel must stay main-WebView only',
);
// Round-12 review P3: the RPC proxy trio (bridge_ready / rpc_begin /
// rpc_respond) previously had no Round-8-style source-shape pin on their
// main-WebView gate lines — the policy anti-pin guards the phone-side callers,
// and that gate line is the aux desktop WebView's only defense; deleting the
// line stayed green. Fill in the same shape: rpc_respond has a long parameter
// list (actual signature-to-gate distance ~311), so the gap cap is relaxed to
// 400 (same as list_host_files), still far smaller than the post-deletion
// distance to the next command's gate (~837), making the deletion red.
assert.match(
  remoteControlCommands,
  /web_access_bridge_ready\([\s\S]{0,240}require_main_webview\(&window\)\?/,
  'the bridge readiness handshake must stay main-WebView only',
);
assert.match(
  remoteControlCommands,
  /web_access_rpc_begin\([\s\S]{0,240}require_main_webview\(&window\)\?/,
  'the RPC dispatch ACK must stay main-WebView only',
);
assert.match(
  remoteControlCommands,
  /web_access_rpc_respond\([\s\S]{0,400}require_main_webview\(&window\)\?/,
  'the RPC completion handoff must stay main-WebView only',
);
assert.match(remoteControlManager, /require_host_workspace_authorization\(endpoint\.config\.allow_host_workspace\)\?/,
  'workspace capabilities must fail closed until desktop authorization is persisted');
assert.doesNotMatch(settingsView, />刷新链接</);
assert.doesNotMatch(settingsView, /Relay 服务器/);
assert.doesNotMatch(settingsView, /getWebRelaySettings/);
assert.match(main, /title=\{t\.uiRemote\.title\}/);
assert.match(main, /const isWebAccessConnected = !!\(bs && bs\.webAccess && bs\.webAccess\.web_client_connected\);/,
  'desktop indicator must reflect an actual browser connection, not a persistent access link');
// The collapsed/expanded footer consolidation shares one indicator render:
// the indicator must live inside the shared renderer (distance-bounded match),
// so neither layout can silently lose it.
assert.match(main, /const renderFooterButtons = \(collapsed\) => \{[\s\S]{0,900}isWebAccessConnected && <span/,
  'expanded and collapsed navigation must use the actual connection indicator');
assert.doesNotMatch(main, /bs\.webAccess\.active && <span/,
  'an enabled access link must not be presented as a connected phone');
assert.match(desktopRemoteControlBridge, /listen\("web_access:status"/,
  'desktop bridge must consume live browser connection status events');
assert.match(desktopRemoteControlBridge, /web_client_connected: false, host_workspace_authorized: false, status: "stopped"/,
  'stopping remote access must clear any stale connected state');
const desktopBridge = readSource(path.join(bridgeRoot, 'bridge.js'), 'utf8');
assert.match(desktopBridge, /web_client_connected: false/,
  'desktop bridge state must start with an explicit disconnected browser state');
for (const source of [settingsView, connectionStatus]) {
  assert.doesNotMatch(source, /WebUI/,
    'user-facing remote control copy must not expose the WebUI implementation name');
}
assert.match(chatView, /data-testid="chat-bottom-spacer"[\s\S]{0,180}className="w-full shrink-0"/,
  'all chat surfaces must use a real flex item because WebKit may omit trailing overflow padding');
assert.match(chatView, /\.\.\.\(hasMessages \? \{\} : \{ paddingBottom:/,
  'message lists must use the real spacer while the non-scrolling empty state retains centering clearance');
assert.match(chatView, /composerH \? composerH \+ 64 : 176/,
  'the bottom spacer must clear both the floating composer and its fade mask');
assert.match(chatView, /composerH \? composerH \+ 48 : 172/,
  'the fade mask must remain shorter than the bottom spacer');


// review #464 round-5 item 3: the web session list must delegate the
// whole-list projection (metadata + workspace_binding) to the extracted
// project_session_list_for_web — reverting that one line must not stay green.
{
  const start = remoteControlCommands.indexOf('pub async fn web_access_list_sessions');
  assert.notStrictEqual(start, -1);
  const body = remoteControlCommands.slice(start, start + 900);
  assert.ok(
    body.includes('project_session_list_for_web(&mut sessions)'),
    'web_access_list_sessions must delegate the web projection to project_session_list_for_web',
  );
}

console.log('web access contract tests passed');
