import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const bridgeRoot = path.join(root, 'src', 'platform', 'tauri');
const chunkedUpload = fs.readFileSync(
  path.join(root, 'src', 'shared', 'chunked-file-upload.js'),
  'utf8',
);
const webBridge = fs.readFileSync(path.join(root, 'src', 'platform', 'web', 'bridge.js'), 'utf8');
const webDomainAdapter = fs.readFileSync(
  path.join(root, 'src', 'platform', 'web', 'bridge', 'domain-adapter.js'),
  'utf8',
);
const attachmentDropController = fs.readFileSync(
  path.join(root, 'src', 'features', 'attachments', 'attachment-drop-controller.js'),
  'utf8',
);
const attachmentDropHook = fs.readFileSync(
  path.join(root, 'src', 'features', 'attachments', 'useAttachmentDrop.js'),
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
    .sort() // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic order of the string array is the asserted expectation
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
  .sort() // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic order of the string array is the asserted expectation
  .map(name => fs.readFileSync(path.join(commandsRoot, name), 'utf8'))
  .join('\n');
const remoteControlCommands = fs.readFileSync(path.join(commandsRoot, 'remote_control.rs'), 'utf8');
const remoteControlManagerRoot = path.join(
  root,
  'src-tauri',
  'src',
  'features',
  'remote_control',
  'manager',
);
const remoteControlManager = fs.readdirSync(remoteControlManagerRoot)
  .filter(name => name.endsWith('.rs'))
  .sort() // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic order of the string array is the asserted expectation
  .map(name => fs.readFileSync(path.join(remoteControlManagerRoot, name), 'utf8'))
  .join('\n');
const remoteControlPlatformRoot = path.join(
  root,
  'src-tauri',
  'src',
  'features',
  'remote_control',
  'platform',
);
const remoteControlPlatform = fs.readdirSync(remoteControlPlatformRoot)
  .filter(name => name.endsWith('.rs'))
  .sort() // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic order of the string array is the asserted expectation
  .map(name => fs.readFileSync(path.join(remoteControlPlatformRoot, name), 'utf8'))
  .join('\n');
const settingsView = fs.readFileSync(path.join(root, 'src', 'features', 'settings', 'SettingsView.jsx'), 'utf8');
const artifactsPanel = fs.readFileSync(path.join(root, 'src', 'features', 'artifacts', 'ArtifactsPanel.jsx'), 'utf8');
const toolStoreView = fs.readFileSync(path.join(root, 'src', 'features', 'tools', 'ToolStoreView.jsx'), 'utf8');
const toolRenderers = fs.readFileSync(path.join(root, 'src', 'features', 'tools', 'tool-renderers.jsx'), 'utf8');
const knowledgeView = fs.readFileSync(path.join(root, 'src', 'features', 'knowledge', 'KnowledgeView.jsx'), 'utf8');
const toolCommon = fs.readFileSync(path.join(root, 'src', 'features', 'tools', 'tool-common.jsx'), 'utf8');
const connectionStatus = fs.readFileSync(path.join(root, 'src', 'features', 'web', 'WebConnectionStatus.jsx'), 'utf8');
const chatView = fs.readFileSync(path.join(root, 'src', 'features', 'chat', 'ChatView.jsx'), 'utf8');
const codexView = fs.readFileSync(path.join(root, 'src', 'features', 'codex', 'CodexAcpView.jsx'), 'utf8');
const acpRuntimeNotices = fs.readFileSync(
  path.join(root, 'src', 'features', 'codex', 'AcpRuntimeNotices.jsx'),
  'utf8',
);
const codexWorkspacePanel = fs.readFileSync(
  path.join(root, 'src', 'features', 'codex', 'CodexWorkspacePanel.jsx'),
  'utf8',
);
const codeViewerModal = fs.readFileSync(
  path.join(root, 'src', 'features', 'codex', 'CodeViewerModal.jsx'),
  'utf8',
);
const acpPlatformClient = fs.readFileSync(path.join(root, 'src', 'features', 'codex', 'acpClient.js'), 'utf8');
const acpErrors = fs.readFileSync(path.join(root, 'src', 'features', 'codex', 'acpErrors.js'), 'utf8');
const i18n = ['zh', 'en', 'ja'].map((l) => fs.readFileSync(path.join(root, 'src', 'shared', 'i18n', `${l}.js`), 'utf8')).join('\n'); // 拆分后三语在 i18n/ 目录
const appMain = fs.readFileSync(path.join(root, 'src', 'app', 'main.jsx'), 'utf8');
const policy = JSON.parse(fs.readFileSync(path.join(root, 'src', 'platform', 'web', 'access-policy.json'), 'utf8'));
const allowed = new Set(policy.allowed_commands);
const allowedEvents = new Set(policy.allowed_events);

function rustCommandBlock(source, command) {
  const signature = source.indexOf(`fn ${command}(`);
  assert.notEqual(signature, -1, `missing Rust command ${command}`);
  const start = source.lastIndexOf('#[tauri::command]', signature);
  const next = source.indexOf('#[tauri::command]', signature + command.length);
  return source.slice(start, next === -1 ? source.length : next);
}

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
  'install_marketplace_skill',
  'install_marketplace_tool',
  'uninstall_marketplace_skill',
  'uninstall_marketplace_tool',
  'import_skill_package',
  'import_skill_package_bytes',
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
  'weibo_status',
  'weibo_skills_state',
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
for (const connector of ["feishu", "wecom", "dingtalk", "tmeet", "weibo"]) {
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
  'web_access_list_sessions',
  'web_access_list_archived_sessions',
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
assert.doesNotMatch(
  acpPlatformClient.match(/export function createAcpSession[\s\S]*?\n\}/)?.[0] || '',
  /web_access_create_codex_acp_session[\s\S]*?workspacePath/,
  'Web code-session creation must submit only the opaque workspace handle',
);
// Web 列表必须走投影命令（主机绝对路径降级为目录名），不得直接调用桌面原命令。
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
assert.match(codeViewerModal, /\{!diff && onReveal && \(/);
assert.match(codeViewerModal, /\{!diff && onOpen && \(/,
  'the shared code preview must omit desktop system actions when callbacks are unavailable');

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
assert.match(remoteControlCommands, /stage_uploaded_attachments\(attachments, &session_id, &?store\)/,
  'uploaded attachments must be staged into the Session workspace before the engine sees their paths');
// Agent 安装命令行/输出可能含内部镜像源或主机路径，Web status 投影必须清除。
assert.match(remoteControlCommands,
  /project_acp_status_for_web[\s\S]*?status\.install_command = None[\s\S]*?status\.install_latest_line = None/,
  'the Web agent-status projection must strip install command and output lines');
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
  /IS_WEB \? "web_access_list_sessions" : "list_sessions"[\s\S]*?IS_WEB \? "web_access_list_archived_sessions" : "list_archived_sessions"/,
  'Web history refreshes must use path-redacted session list commands');
assert.match(remoteControlCommands,
  /fn web_workspace_result[\s\S]*?web_workspace_\{operation\}_failed[\s\S]*?web_access_list_codex_workspace[\s\S]*?web_workspace_result\("listing", result\)[\s\S]*?web_access_search_codex_workspace[\s\S]*?web_workspace_result\("search", result\)[\s\S]*?web_access_preview_codex_workspace_file[\s\S]*?web_workspace_result\("preview", result\)[\s\S]*?web_access_get_codex_workspace_changes[\s\S]*?web_workspace_result\("changes", result\)[\s\S]*?web_access_get_codex_workspace_diff[\s\S]*?web_workspace_result\("diff", result\)/,
  'Web workspace RPC failures must not return host paths embedded in native errors');
assert.match(remoteControlCommands,
  /web_access_get_codex_acp_timeline[\s\S]*?web_acp_result\([\s\S]*?"timeline"/,
  'Web ACP timeline failures must cross Relay as a controlled error code');
for (const [command, operation] of Object.entries({
  web_access_create_codex_acp_session: 'session_create',
  web_access_cancel_codex_acp: 'cancel',
  web_access_codex_acp_prompt: 'prompt',
  web_access_get_codex_acp_timeline: 'timeline',
  web_access_get_codex_acp_session_info: 'session_info',
  web_access_set_codex_acp_model: 'set_model',
  web_access_set_codex_acp_mode: 'set_mode',
  web_access_set_codex_acp_config_option: 'set_config_option',
  web_access_get_codex_acp_pending_permissions: 'pending_permissions',
  web_access_respond_codex_acp_permission: 'respond_permission',
  web_access_get_codex_acp_pending_elicitations: 'pending_elicitations',
  web_access_respond_codex_acp_elicitation: 'respond_elicitation',
  web_access_list_codex_acp_sessions: 'list_sessions',
  web_access_list_acp_agents: 'list_agents',
  web_access_get_acp_agent_status: 'agent_status',
})) {
  assert.match(
    rustCommandBlock(remoteControlCommands, command),
    new RegExp(`web_acp_result\\("${operation}"`),
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
assert.match(codexView,
  /isWeb \? 'web_access_respond_codex_acp_permission' : 'respond_codex_acp_permission'/,
  'permission responses must preserve the native command and use a stable Web wrapper');
assert.match(codexView,
  /isWeb \? 'web_access_respond_codex_acp_elicitation' : 'respond_codex_acp_elicitation'/,
  'elicitation responses must preserve the native command and use a stable Web wrapper');
assert.match(acpErrors, /CONTROLLED_WEB_ERROR[\s\S]*?copy\.operationFailed/,
  'controlled Web error codes must become localized UI copy instead of raw browser text');

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
assert.match(hostFilePicker, /issueWorkspaceHandle:\s*options\.workspaceGrant === true/,
  'only workspace selection should request a one-shot host capability');
assert.match(hostFilePicker, /workspaceHandle:\s*currentWorkspaceHandle/,
  'workspace selection must return the host-issued handle with its display path');
assert.match(hostFilePicker, /message === "host_workspace_not_authorized"[\s\S]{0,120}workspaceNotAuthorized/,
  'an unapproved legacy endpoint must receive a localized desktop-authorization prompt');
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
assert.match(appMain, /PinvouHostFilePickerStrings\s*=\s*misc\.hostFilePicker/,
  'the active locale must reach the Web host picker error branch');
assert.match(remoteControlCommands,
  /reserve_web_workspace_grant[\s\S]*?create_codex_acp_session[\s\S]*?restore_web_workspace_grant/,
  'failed Web ACP Session creation must restore its same-endpoint one-shot workspace reservation');
assert.match(hostFilePicker, /initialPathPending[\s\S]*?path === initialPath[\s\S]*?load\(null\)/,
  'a stale recent workspace must fall back to the normal host-file root instead of trapping the picker');
// HTML5 拖放由当前可见输入框认领，再复用对应平台的上传通道。
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
assert.match(webBridge, /web_access_update_settings", \{ patch: \{ (?:search: search|search) \} \}/,
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
assert.match(bridge, /(?:var|const|let) sid = state\.activeSessionId;/);
assert.match(bridge, /if \(state\.activeSessionId !== sid\) return;/);
assert.match(bridge, /remoteAdmissionKeys/);
assert.match(bridge, /(?:var|const|let) activePlanCards = Object\.create\(null\)/);
assert.match(bridge, /(?:var|const|let) hydratedKey = planCardHydrationKey\(hydratedPlan\)/);
assert.match(bridge, /hydratedPlan\.cardState = "active"/);
assert.match(bridge, /if \(item\.type === "plan_card"\) return false/);
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
assert.ok(
  bridge.indexOf('await waitForWebInvokeCapabilities();') <
    bridge.indexOf('supportsSessionDownloadCancellation = canInvoke("web_access_cancel_session_download")'),
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
const composerShared = fs.readFileSync(path.join(root, 'src', 'features', 'settings', 'composer-shared.jsx'), 'utf8');
assert.match(composerShared, /const canSwitchModels = can\('sessionModelSwitch'\);/);
assert.match(composerShared, /const canMutateToolStore = can\('toolStoreMutations'\);/);
assert.match(composerShared, /const toolSwitchDisabled = !canMutateToolStore;/);
// 只增不减 + 未提交可撤销：会话中阻隔「关闭」，但本会话内刚打开（pending）、
// 尚未随新一轮对话进入上下文的允许改回；新一轮被后端受理后才锁死。
assert.match(composerShared, /if \(toolSwitchDisabled \|\| \(hasActiveSession && enabled && !pending\.ids\.has\(id\)\)\) return;/);
assert.match(composerShared, /if \(toolSwitchDisabled \|\| \(hasActiveSession && projectSkillsEnabled && !pending\.projectSkills\)\) return;/);
assert.match(composerShared, /if \(enabled\) pending\.ids\.delete\(id\); else pending\.ids\.add\(id\);/);
assert.match(composerShared, /window\.addEventListener\('pinvou:chat-round-committed', onCommitted\)/);
assert.match(composerShared, /pending\.ids\.clear\(\);\s*\n\s*pending\.projectSkills = false;/);
// 转正（清空 pending）必须在模块级监听里做，不能只在组件 effect 里：受理事件
// 可能在菜单组件已卸载时到达（排队消息在用户切页后 flush、后台定向发送），
// 组件级监听会漏清，重挂载后 pending 悬空、已进上下文的工具仍显示可关。
// 模块级监听注册点还必须先于组件定义，保证清空发生在组件 bump 重渲染之前。
const moduleListenerIdx = composerShared.indexOf(
  "window.addEventListener('pinvou:chat-round-committed', (event)",
);
assert.ok(
  moduleListenerIdx >= 0 && moduleListenerIdx < composerShared.indexOf('const ComposerToolMenu'),
  'round-committed pending-clear listener must be registered at module level (before ComposerToolMenu), not only inside the component',
);
// 提交信号由发送链路在后端受理新一轮后派发：常规发送（desktop/web doSendFor）、
// Web 首轮提交、接受计划（desktop/web acceptPlan）、编辑重跑（desktop/web
// editLastTurn）、原生代码车道发送与接受方案，缺任一处 pending 都不会转正。
assert.match(bridge, /window\.dispatchEvent\(new CustomEvent\("pinvou:chat-round-committed", \{ detail: \{ scope: "plain" \} \}\)\)/);
assert.ok(
  (bridge.match(/pinvou:chat-round-committed/g) || []).length >= 7,
  'desktop doSendFor/acceptPlan/editLastTurn and web doSendFor/first-turn/acceptPlan/editLastTurn must each dispatch the round-committed event',
);
assert.ok(
  (codexView.match(/notifyChatRoundCommitted\('code'\);/g) || []).length >= 2,
  'native code lane send and accept-plan must each commit pending enables',
);
const toolEvents = fs.readFileSync(path.join(root, 'src', 'features', 'tools', 'tool-events.js'), 'utf8');
assert.match(toolEvents, /export \{ notifyComposerToolsChanged, notifyChatRoundCommitted \};/);
assert.match(composerShared, /bridge\.models\.switchModel\(activeSessionId, id\)/);
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
// Note: on main, the isWeb fallback guard inside LocalFilePreview sat in dead code (no call sites) and was removed along with it;
// the live-path Web guard for OutputLivePreview is the session-authorized read asserted above (o.path, outputSessionId).
assert.match(settingsView, /const canPickHostFiles = can\('hostFilePicker'\);/);
assert.match(toolCommon, /const canOpenArtifact = !isWeb \|\| can\('artifactDownload'\);/);
assert.match(connectionStatus, /incompatible_desktop/);
assert.match(connectionStatus, /BLOCKING[\s\S]*incompatible_desktop/);
assert.match(settingsView, /remoteCopy = t\.uiRemote/);
assert.match(settingsView, /\{remoteCopy\.title\}/);
assert.match(settingsView, /\{remoteCopy\.link\}/);
assert.match(settingsView, /\{remoteCopy\.refresh\}/);
assert.match(settingsView, /startRemoteControl\(\{ allowHostWorkspace: true \}\)/,
  'host workspace access must follow an explicit action in the desktop modal');
assert.doesNotMatch(settingsView, /useEffect\(\(\) => \{[\s\S]{0,240}startRemoteControl/,
  'opening the remote-control modal must not silently authorize host workspace access');
assert.match(desktopRemoteControlBridge, /allowHostWorkspace: !!\(options && options\.allowHostWorkspace\)/,
  'the desktop bridge must carry explicit host-workspace consent');
assert.match(remoteControlCommands, /web_access_enable\([\s\S]{0,240}require_main_webview\(&window\)\?/,
  'only the main desktop WebView may enable a persistent remote endpoint');
assert.match(remoteControlManager, /require_host_workspace_authorization\(endpoint\.config\.allow_host_workspace\)\?/,
  'workspace capabilities must fail closed until desktop authorization is persisted');
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
assert.match(desktopRemoteControlBridge, /web_client_connected: false, host_workspace_authorized: false, status: "stopped"/,
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
