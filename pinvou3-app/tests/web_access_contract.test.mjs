import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const bridgeRoot = path.join(root, 'src', 'platform', 'tauri');
const webBridge = fs.readFileSync(path.join(root, 'src', 'platform', 'web', 'bridge.js'), 'utf8');
const webDomainAdapter = fs.readFileSync(
  path.join(root, 'src', 'platform', 'web', 'bridge', 'domain-adapter.js'),
  'utf8',
);
const attachmentDropController = fs.readFileSync(
  path.join(root, 'src', 'features', 'attachments', 'attachment-drop-controller.js'),
  'utf8',
);
const desktopRemoteControlBridge = fs.readFileSync(
  path.join(bridgeRoot, 'bridge', 'remote-control.js'),
  'utf8',
);
const desktopSessionsBridge = fs.readFileSync(
  path.join(bridgeRoot, 'bridge', 'sessions.js'),
  'utf8',
);
const desktopBridgeSources = [
  fs.readFileSync(path.join(bridgeRoot, 'bridge.js'), 'utf8'),
  ...fs.readdirSync(path.join(bridgeRoot, 'bridge'))
    .filter(name => name.endsWith('.js'))
    .sort()
    .map(name => fs.readFileSync(path.join(bridgeRoot, 'bridge', name), 'utf8')),
];
const bridge = [
  webBridge,
  webDomainAdapter,
  ...desktopBridgeSources,
].join('\n');
const bootstrap = fs.readFileSync(path.join(root, 'src', 'platform', 'web', 'bootstrap.js'), 'utf8');
const hostFilePicker = fs.readFileSync(
  path.join(root, 'src', 'platform', 'web', 'host-file-picker.js'),
  'utf8',
);
const commandsRoot = path.join(root, 'src-tauri', 'src', 'app', 'commands');
const commands = fs.readdirSync(commandsRoot)
  .filter(name => name.endsWith('.rs'))
  .sort()
  .map(name => fs.readFileSync(path.join(commandsRoot, name), 'utf8'))
  .join('\n');
const remoteControlCommands = fs.readFileSync(path.join(commandsRoot, 'remote_control.rs'), 'utf8');
const settingsView = fs.readFileSync(path.join(root, 'src', 'features', 'settings', 'SettingsView.jsx'), 'utf8');
const artifactsPanel = fs.readFileSync(path.join(root, 'src', 'features', 'artifacts', 'ArtifactsPanel.jsx'), 'utf8');
const toolStoreView = fs.readFileSync(path.join(root, 'src', 'features', 'tools', 'ToolStoreView.jsx'), 'utf8');
const toolRenderers = fs.readFileSync(path.join(root, 'src', 'features', 'tools', 'tool-renderers.jsx'), 'utf8');
const knowledgeView = fs.readFileSync(path.join(root, 'src', 'features', 'knowledge', 'KnowledgeView.jsx'), 'utf8');
const toolCommon = fs.readFileSync(path.join(root, 'src', 'features', 'tools', 'tool-common.jsx'), 'utf8');
const connectionStatus = fs.readFileSync(path.join(root, 'src', 'features', 'web', 'WebConnectionStatus.jsx'), 'utf8');
const chatView = fs.readFileSync(path.join(root, 'src', 'features', 'chat', 'ChatView.jsx'), 'utf8');
const policy = JSON.parse(fs.readFileSync(path.join(root, 'src', 'platform', 'web', 'access-policy.json'), 'utf8'));
const allowed = new Set(policy.allowed_commands);
const allowedEvents = new Set(policy.allowed_events);

for (const command of [
  'chat',
  'ingest_file',
  'save_session_messages',
  'web_access_save_session_messages_chunk',
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
  'import_skill_package',
  'import_skill_package_bytes',
]) {
  assert.equal(allowed.has(command), false, `${command} must remain desktop-only`);
}

// 知识库批量导入的进度查看与继续/取消/重试/失败文件分页是一组协同命令：Web 端知识库
// 已开放（kb_collection_add_sources 等），任一导入控制命令遗漏会让对应按钮静默失败。
for (const command of [
  'kb_index_status',
  'kb_index_cancel',
  'kb_index_resume',
  'kb_index_retry_file',
  'kb_index_failed_files',
]) {
  assert.equal(allowed.has(command), true, `${command} must be allowed on Web (KB import controls)`);
}

// 已授权连接器的只读状态查询属于 WebUI 业务面（ToolStoreView 挂载即调用 *_status，
// SettingsView 的 composer 工具菜单调用 *_skills_state）。任一遗漏会让对应连接器在
// Web 端永远显示未连接：卡片因 externalAuth 不可用而依赖 installed 徽标展示。
// 连接器开关/装卸（set_*_enabled、*_ensure_cli、*_apply_skills 等）仍保持桌面专用。
for (const command of [
  'feishu_status',
  'feishu_skills_state',
  'wecom_status',
  'wecom_skills_state',
  'dingtalk_status',
  'dingtalk_skills_state',
  'tmeet_status',
  'tmeet_skills_state',
  'ima_status',
]) {
  assert.equal(allowed.has(command), true, `${command} must be allowed on Web (authorized connector status queries)`);
}
// 连接器变更面保持桌面专用：连接/断开（*_connect_begin/*_logout、ima_connect/ima_logout）、
// 逐连接器开关（set_*_enabled）与全局清单写入（set_disabled_connectors）、原生 CLI 安装
// （*_ensure_cli 触发下载物化）、技能装卸（*_apply_skills 向 ~/.pinvou3 物化技能包）、
// OAuth 中断（*_cancel）、授权门重算（refresh_connector_auth_gates）。
// 清单须与 lib.rs 连接器注册面保持同步。
const deniedConnectorMutations = [];
for (const connector of ["feishu", "wecom", "dingtalk", "tmeet"]) {
  deniedConnectorMutations.push(
    `${connector}_connect_begin`,
    `${connector}_logout`,
    `${connector}_ensure_cli`,
    `${connector}_cancel`,
    `${connector}_apply_skills`,
    `set_${connector}_enabled`,
  );
}
deniedConnectorMutations.push(
  "ima_connect", "ima_logout", "set_disabled_connectors", "refresh_connector_auth_gates",
  // 技能级停用清单与项目技能开关（settings 管理面，读写均桌面专用；
  // 此前两头都不沾，加白名单不会触发测试——与「清单须与注册面同步」承诺矛盾）。
  "set_disabled_skills", "get_disabled_skills",
  "set_project_skills_enabled", "get_project_skills_enabled",
);
for (const command of deniedConnectorMutations) {
  assert.equal(allowed.has(command), false, `${command} must remain desktop-only (connector mutations)`);
}

for (const command of [
  'web_access_chat',
  'web_access_create_session_and_chat',
  'web_access_ingest_file',
  'web_access_upload_attachment_chunk',
  'web_access_abort_attachment_upload',
  'web_access_discard_attachment',
  'web_access_load_session_chunk',
  'web_access_transcribe_voice_audio',
]) {
  assert.equal(allowed.has(command), true, `${command} must be the bounded Web wrapper`);
}

// 浏览器本机上传:双入口按能力协商门控,分块有界,取消/失败路径完备。
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
assert.match(webBridge, /DEVICE_UPLOAD_CHUNK_BYTES = 256 \* 1024/,
  'upload chunks must stay aligned with the desktop MAX_TRANSFER_CHUNK_BYTES limit');
assert.match(webBridge, /DEVICE_UPLOAD_MAX_BYTES = 20 \* 1024 \* 1024/,
  'the browser preflight must mirror file_ingest::MAX_FILE_BYTES');
assert.match(webBridge, /web_access_abort_attachment_upload/,
  'cancelled or failed uploads must release the desktop buffer');
assert.match(webBridge, /web_access_discard_attachment/,
  'removed or late-cancelled attachments must release their opaque desktop handle');
assert.match(remoteControlCommands, /stage_uploaded_attachments\(attachments, &session_id, &?store\)/,
  'uploaded attachments must be staged into the Session workspace before the engine sees their paths');

assert.match(bootstrap, /sendRaw\(\{ \.\.\.value, v: protocolVersion, lease_id: this\.leaseId \}\)/);
assert.match(bootstrap, /desktopCapabilitiesReady/);
assert.match(bootstrap, /SEMANTIC_COMMAND_REQUIREMENTS/);
assert.match(bootstrap, /supportsCapability\(capability\)/);
assert.match(bootstrap, /if \(!this\.desktopCapabilitiesReady\) return false/);
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
// HTML5 拖放复用 deviceFileUpload 分块通道:同一能力门控、同一上传/取消/丢弃语义。
assert.match(webBridge, /canAccept: function \(\) \{ return hasCapability\("deviceFileUpload"\); \}/,
  'browser drop must gate on the negotiated device upload capability');
assert.match(webBridge, /onFiles: function \(files\) \{ return uploadDeviceFiles\(files\); \}/,
  'browser drop must reuse the device upload pipeline instead of a parallel channel');
assert.match(webBridge, /PinvouAttachmentDropController\.install/);
assert.match(attachmentDropController, /dataTransfer\.dropEffect = "copy"/);
assert.match(attachmentDropController, /setActive\(true\)/);
assert.match(attachmentDropController, /setActive\(false\)/);
assert.match(bootstrap, /sendReady\(false\)/);
assert.match(bootstrap, /state_ready: stateReady/);
assert.match(bootstrap, /markStateReady\(\)/);
assert.match(bootstrap, /if \(!this\.frontendReady \|\| !this\.stateReady\)/);
assert.match(bootstrap, /if \(!this\.frontendReady \|\| !this\.stateReady\)/);
const main = fs.readFileSync(path.join(root, 'src', 'app', 'main.jsx'), 'utf8');
const webSearchRestartBody = webBridge.slice(
  webBridge.indexOf('async function saveSearchSettingsAndRestart'),
  webBridge.indexOf('async function submitFeedback'),
);
assert.match(webSearchRestartBody, /unsupported by the Web host/,
  'Web settings bridge must report desktop restart as unsupported');
assert.doesNotMatch(webSearchRestartBody, /invoke\("restart_app"/,
  'Web settings bridge must not invoke the native-only restart command');
assert.match(main, /const saved = isWeb[\s\S]{0,180}saveSearchSettings\(search\)[\s\S]{0,180}saveSearchSettingsAndRestart\(search\)/,
  'the shared UI must save without requesting a desktop restart in WebUI');
assert.match(webBridge, /state\.settings = await invoke\(IS_WEB \? "web_access_update_settings" : "update_settings"/,
  'WebUI must keep the canonical settings returned by the desktop backend');
assert.match(webBridge, /web_access_update_settings", \{ patch: \{ search: search \} \}/,
  'WebUI search saves must send a narrow patch instead of a full settings snapshot');
assert.match(remoteControlCommands, /web_access_update_settings\([\s\S]{0,120}patch: super::settings::WebSettingsPatch,[\s\S]{0,80}\) -> Result<UserPrefs, String>/,
  'the bounded Web settings command must return canonical preferences');
assert.match(bootstrap, /pinvou:web-capabilities/);
assert.ok((main.match(/\{can\('webAccessAdmin'\) && <button[\s\S]{0,220}handleOpenWebAccess/g) || []).length >= 2,
  'desktop Web-access controls must stay hidden inside WebUI in both sidebar layouts');
assert.ok((main.match(/\{can\('pet'\) && <button[\s\S]{0,220}handleSetPetEnabled/g) || []).length >= 2,
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
assert.match(webBridge, /buf\.composerDraft = state\.composerDraft/,
  'WebUI session switching must save the active composer draft');
assert.match(webBridge, /state\.composerDraft = buf\.composerDraft/,
  'WebUI session switching must restore the destination composer draft');
assert.match(webBridge, /var draftComposer = realId \? "" : \(state\.composerDraft \|\| ""\)/,
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
assert.match(bridge, /var sid = state\.activeSessionId;/);
assert.match(bridge, /if \(state\.activeSessionId !== sid\) return;/);
assert.match(bridge, /remoteAdmissionKeys/);
assert.match(bridge, /var activePlanCards = Object\.create\(null\)/);
assert.match(bridge, /var hydratedKey = planCardHydrationKey\(hydratedPlan\)/);
assert.match(bridge, /hydratedPlan\.cardState = "active"/);
assert.match(bridge, /if \(item\.type === "plan_card"\) return false/);
assert.match(bridge, /action === "accept_plan"/);
assert.match(bridge, /acceptedMode = payload\.mode_state \|\| payload\.modeState/);
assert.match(bridge, /planNotActive = errorText\.indexOf\("plan_not_active"\)/);
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
assert.doesNotMatch(
  webBridge,
  /web_access_load_session_chunk[\s\S]{0,220}\blimit\s*:/,
  'WebUI must let each desktop version choose its supported session chunk size',
);
assert.match(bridge, /MAX_WEB_ARTIFACT_DOWNLOAD_BYTES = 256 \* 1024 \* 1024/);
assert.match(bridge, /if \(IS_WEB && !hasCapability\("artifactDownload"\)\)/);
assert.match(bridge, /var info = await artifactInfo\(path, resolvedSessionId\)/);
assert.match(bridge, /if \(expectedSize > MAX_WEB_ARTIFACT_DOWNLOAD_BYTES\)/);
assert.match(bridge, /if \(bytes\.length > MAX_WEB_ARTIFACT_DOWNLOAD_BYTES - offset\)/);
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
assert.match(settingsView, /const canSwitchModels = can\('sessionModelSwitch'\);/);
assert.match(settingsView, /const canMutateToolStore = can\('toolStoreMutations'\);/);
assert.match(settingsView, /disabled=\{!canMutateToolStore\}/);
assert.match(settingsView, /if \(!canMutateToolStore\) return;/);
assert.match(settingsView, /bridge\.models\.switchModel\(activeSessionId, id\)/);
assert.match(settingsView, /\{canManageModels && editingModel && \(/);
assert.match(toolStoreView, /if \(!can\('toolStoreMutations'\)\) \{/);
assert.match(toolStoreView, /const canMutateToolStore = can\('toolStoreMutations'\);/);
assert.ok((toolStoreView.match(/if \(!canMutateToolStore\) return;/g) || []).length >= 4,
  'all tool install, uninstall, and import handlers must fail closed in WebUI');
assert.match(knowledgeView, /const canDownloadArtifacts = !isWeb \|\| can\('artifactDownload'\);/);
assert.match(knowledgeView, /const canPickHostFiles = !isWeb \|\| can\('hostFilePicker'\);/);
assert.match(knowledgeView, /const outputSessionId = o\.sessionId \|\| o\.session_id \|\| null;/);
assert.match(knowledgeView, /const cacheKey = `\$\{outputSessionId \|\| ''\}\|\$\{o\.path\}\|\$\{o\.mtime \|\| 0\}`;/);
assert.ok((knowledgeView.match(/o\.path, outputSessionId/g) || []).length >= 5,
  'output previews must authorize every Web artifact read with the owning session');
assert.match(knowledgeView, /<FilePreviewModal path=\{outputPreview\.path\} sessionId=\{outputPreview\.sessionId\}/);
assert.match(knowledgeView, /if \(isWeb\) \{ setPv\(\{ kind: 'fallback' \}\);/,
  'WebUI must not treat arbitrary local knowledge paths as session artifacts');
assert.match(settingsView, /const canPickHostFiles = can\('hostFilePicker'\);/);
assert.match(toolCommon, /const canOpenArtifact = !isWeb \|\| can\('artifactDownload'\);/);
assert.match(connectionStatus, /incompatible_desktop/);
assert.match(connectionStatus, /BLOCKING[\s\S]*incompatible_desktop/);
assert.match(settingsView, /remoteCopy = t\.uiRemote/);
assert.match(settingsView, /\{remoteCopy\.title\}/);
assert.match(settingsView, /\{remoteCopy\.link\}/);
assert.match(settingsView, /\{remoteCopy\.refresh\}/);
assert.doesNotMatch(settingsView, />刷新链接</);
assert.doesNotMatch(settingsView, /Relay 服务器/);
assert.doesNotMatch(settingsView, /getWebRelaySettings/);
assert.match(main, /title=\{t\.uiRemote\.title\}/);
assert.match(main, /const isWebAccessConnected = !!\(bs && bs\.webAccess && bs\.webAccess\.web_client_connected\);/,
  'desktop indicator must reflect an actual browser connection, not a persistent access link');
assert.equal((main.match(/isWebAccessConnected && <span/g) || []).length, 2,
  'expanded and collapsed navigation must use the actual connection indicator');
assert.doesNotMatch(main, /bs\.webAccess\.active && <span/,
  'an enabled access link must not be presented as a connected phone');
assert.match(desktopRemoteControlBridge, /listen\("web_access:status"/,
  'desktop bridge must consume live browser connection status events');
assert.match(desktopRemoteControlBridge, /web_client_connected: false, status: "stopped"/,
  'stopping remote access must clear any stale connected state');
const desktopBridge = fs.readFileSync(path.join(bridgeRoot, 'bridge.js'), 'utf8');
assert.match(desktopBridge, /web_client_connected: false/,
  'desktop bridge state must start with an explicit disconnected browser state');
for (const source of [settingsView, connectionStatus]) {
  assert.doesNotMatch(source, /WebUI/,
    'user-facing remote control copy must not expose the WebUI implementation name');
}
assert.match(chatView, /data-testid="chat-bottom-spacer"[\s\S]{0,180}className="w-full shrink-0"/,
  'all chat surfaces must use a real flex item because WebKit may omit trailing overflow padding');
assert.match(chatView, /style=\{hasMessages \? undefined : \{ paddingBottom:/,
  'message lists must use the real spacer while the non-scrolling empty state retains centering clearance');
assert.match(chatView, /composerH \? composerH \+ 64 : 176/,
  'the bottom spacer must clear both the floating composer and its fade mask');
assert.match(chatView, /composerH \? composerH \+ 48 : 172/,
  'the fade mask must remain shorter than the bottom spacer');

console.log('web access contract tests passed');
