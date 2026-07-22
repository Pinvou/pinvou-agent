import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const bridge = fs.readFileSync(path.join(root, 'src', 'tauri-bridge.js'), 'utf8');
const bootstrap = fs.readFileSync(path.join(root, 'src', 'web-bootstrap.js'), 'utf8');
const commands = fs.readFileSync(path.join(root, 'src-tauri', 'src', 'commands.rs'), 'utf8');
const settingsView = fs.readFileSync(path.join(root, 'src', 'features', 'settings', 'SettingsView.jsx'), 'utf8');
const artifactsPanel = fs.readFileSync(path.join(root, 'src', 'features', 'artifacts', 'ArtifactsPanel.jsx'), 'utf8');
const toolStoreView = fs.readFileSync(path.join(root, 'src', 'features', 'tools', 'ToolStoreView.jsx'), 'utf8');
const toolRenderers = fs.readFileSync(path.join(root, 'src', 'features', 'tools', 'tool-renderers.jsx'), 'utf8');
const workflowView = fs.readFileSync(path.join(root, 'src', 'features', 'workflow', 'WorkflowView.jsx'), 'utf8');
const knowledgeView = fs.readFileSync(path.join(root, 'src', 'features', 'knowledge', 'KnowledgeView.jsx'), 'utf8');
const toolCommon = fs.readFileSync(path.join(root, 'src', 'features', 'tools', 'tool-common.jsx'), 'utf8');
const connectionStatus = fs.readFileSync(path.join(root, 'src', 'features', 'web', 'WebConnectionStatus.jsx'), 'utf8');
const chatView = fs.readFileSync(path.join(root, 'src', 'features', 'chat', 'ChatView.jsx'), 'utf8');
const policy = JSON.parse(fs.readFileSync(path.join(root, 'src', 'web-access-policy.json'), 'utf8'));
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
]) {
  assert.equal(allowed.has(command), false, `${command} must remain desktop-only`);
}

for (const command of [
  'web_access_chat',
  'web_access_ingest_file',
  'web_access_load_session_chunk',
  'web_access_transcribe_voice_audio',
]) {
  assert.equal(allowed.has(command), true, `${command} must be the bounded Web wrapper`);
}

assert.match(bootstrap, /sendRaw\(\{ \.\.\.value, v: protocolVersion, lease_id: this\.leaseId \}\)/);
assert.match(bootstrap, /desktopCapabilitiesReady/);
assert.match(bootstrap, /SEMANTIC_COMMAND_REQUIREMENTS/);
assert.match(bootstrap, /supportsCapability\(capability\)/);
assert.match(bootstrap, /if \(!this\.desktopCapabilitiesReady\) return false/);
assert.match(bridge, /if \(IS_WEB && typeof PLATFORM\.can === "function"\) return PLATFORM\.can\(name\) === true/);
assert.match(bootstrap, /sendReady\(false\)/);
assert.match(bootstrap, /state_ready: stateReady/);
assert.match(bootstrap, /markStateReady\(\)/);
assert.match(bootstrap, /if \(!this\.frontendReady \|\| !this\.stateReady\)/);
assert.match(bootstrap, /if \(!this\.frontendReady \|\| !this\.stateReady\)/);
const main = fs.readFileSync(path.join(root, 'src', 'main.jsx'), 'utf8');
assert.match(main, /pinvou:web-capabilities/);
assert.match(bridge, /if \(!IS_WEB && !isDetachedWindow\) registerWebAccessDesktopProxy\(\)/);
assert.match(bridge, /eventForwardersReady/);
assert.match(bridge, /listen\("chat:user_message"/);
assert.match(bridge, /listen\("chat:transcript_committed"/);
assert.match(bridge, /Transcript persistence is authoritative in Rust/);
assert.doesNotMatch(bridge, /saveSessionMessagesForClient/);
assert.match(bridge, /session_turn_in_progress/);
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
assert.match(settingsView, /bridge\.switchModel\(activeSessionId, id\)/);
assert.match(settingsView, /\{canManageModels && editingModel && \(/);
assert.match(toolStoreView, /if \(!can\('toolStoreMutations'\)\) \{/);
assert.match(toolStoreView, /const canMutateToolStore = can\('toolStoreMutations'\);/);
assert.ok((toolStoreView.match(/if \(!canMutateToolStore\) return;/g) || []).length >= 4,
  'all tool install, uninstall, and import handlers must fail closed in WebUI');
assert.match(workflowView, /can\('artifactDownload'\)/);
assert.match(workflowView, /can\('hostFilePicker'\)/);
assert.match(knowledgeView, /const canDownloadArtifacts = !isWeb \|\| can\('artifactDownload'\);/);
assert.match(knowledgeView, /const canPickHostFiles = !isWeb \|\| can\('hostFilePicker'\);/);
assert.match(settingsView, /const canPickHostFiles = can\('hostFilePicker'\);/);
assert.match(toolCommon, /const canOpenArtifact = !isWeb \|\| can\('artifactDownload'\);/);
assert.match(connectionStatus, /incompatible_desktop/);
assert.match(connectionStatus, /BLOCKING[\s\S]*incompatible_desktop/);
assert.match(chatView, /data-testid="chat-bottom-spacer"[\s\S]{0,180}className="w-full shrink-0"/,
  'WebUI must use a real flex item for composer clearance because iOS Safari may omit trailing overflow padding');
assert.match(chatView, /style=\{\(isWeb && hasMessages\) \? undefined : \{ paddingBottom:/,
  'WebUI messages must use the real spacer while the non-scrolling empty state retains centering clearance');

console.log('web access contract tests passed');
