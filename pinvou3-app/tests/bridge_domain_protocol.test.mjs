import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const bridgeRoot = path.join(root, 'src', 'platform', 'tauri');

function read(relativePath) {
  return fs.readFileSync(path.join(bridgeRoot, relativePath), 'utf8');
}

function extractCalls(source, callee) {
  const calls = [];
  const needle = `${callee}(`;
  let cursor = 0;
  while ((cursor = source.indexOf(needle, cursor)) !== -1) {
    const previous = source[cursor - 1] || '';
    if (/[A-Za-z0-9_$]/.test(previous)) {
      cursor += needle.length;
      continue;
    }
    let index = cursor + needle.length;
    let depth = 1;
    let quote = null;
    let escaped = false;
    let lineComment = false;
    let blockComment = false;
    for (; index < source.length && depth > 0; index += 1) {
      const char = source[index];
      const next = source[index + 1];
      if (lineComment) {
        if (char === '\n') lineComment = false;
        continue;
      }
      if (blockComment) {
        if (char === '*' && next === '/') { blockComment = false; index += 1; }
        continue;
      }
      if (quote) {
        if (escaped) escaped = false;
        else if (char === '\\') escaped = true;
        else if (char === quote) quote = null;
        continue;
      }
      if (char === '/' && next === '/') { lineComment = true; index += 1; continue; }
      if (char === '/' && next === '*') { blockComment = true; index += 1; continue; }
      if (char === '"' || char === "'" || char === '`') { quote = char; continue; }
      if (char === '(') depth += 1;
      else if (char === ')') depth -= 1;
    }
    assert.equal(depth, 0, `unclosed ${callee} call near offset ${cursor}`);
    calls.push(source.slice(cursor, index).replace(/\s+/g, ' ').trim());
    cursor = index;
  }
  return calls;
}

const protocolSources = {
  orchestration: ['bridge.js'],
  artifacts: ['bridge/artifact-tracker.js', 'bridge/artifacts.js'],
  chat: ['bridge/chat.js', 'bridge/chat-events.js', 'bridge/terminal.js'],
  dependencies: ['bridge/dependencies.js'],
  interaction: ['bridge/interaction.js'],
  knowledge: ['bridge/knowledge-model.js'],
  memory: ['bridge/memory.js'],
  monitor: ['bridge/monitor.js'],
  personas: ['bridge/personas.js'],
  remoteControl: ['bridge/remote-control.js'],
  scheduled: ['bridge/scheduled.js'],
  sessions: ['bridge/sessions.js'],
  settings: ['bridge/settings.js'],
  updater: ['bridge/updater.js'],
  voice: ['bridge/voice.js'],
  workflow: ['bridge/workflow-runtime.js', 'bridge/workflow.js'],
};

const expectedProtocolHashes = {
  orchestration: '978048c1070c0d876fa2cd1b0493b46f973d46d9ed8bf1b937dbe36b5fe6a9b3',
  artifacts: 'cbb7f68ec32ead55ad759859e2bb2df6af5eb9a649e985c313e665aee7c2f0af',
  chat: '38ce6059420b902eff83cd4801284a42ac1aa53950ff92ef8a8d72c0eefe0492',
  dependencies: '53dc5f9fa4245b065c27904068fa15d8fee0492abf21f0cbc1d91f5dd0a89bb9',
  interaction: 'db1647d6c406d6c34c1ac33a914797bfb3effde0c5d5b2670581a3cc35aa6993',
  knowledge: 'f1e6bf2e21474ba5573e9411e5c5e32d63ecdb0517b42320232ccd0940a59b69',
  memory: 'd92cbabf27c277a64b743e7af25b48d8b8b65513e33aeb0f38c906d4b300616b',
  monitor: '01bf9a7c9b9b3f313cf49e975e6503627ff373caed0f4b3be07a6a98492a7c43',
  personas: '5959bca3e4169cd3136db9dfff145370f1019ffeb865b357d2982b4d877fdf7e',
  remoteControl: '86e9f18726ad1302d4aed4fb5b8035a7b8daf1eab79466f60af5708cfe646a2b',
  scheduled: '239292d75c308973053cc0091e0ac9437191bf2375fd5fd8181ea26f4f749900',
  sessions: '88e0af710e27c0347eec38ea48e666763ce947e12111e6f1128b230c8533adf5',
  settings: '72840144d60884fcbeb7dd0744de830060ae33207ccabfc8b5cb8d9afc23766e',
  updater: 'b4a287c32fc618553aa40d3fac078e9dc8536acefa4e064274e5766ec5cd88bc',
  voice: '2e6789eca3969f27e8e0fd9f034bd82e0b0e1f302152efc65c5714839fbf5b72',
  workflow: '602a275bbfa7e8dfa0a95f52dc82e5c340d8514b9ebc795572515948ef487aaf',
};

for (const [domain, files] of Object.entries(protocolSources)) {
  const signatures = files.flatMap(file => {
    const source = read(file);
    return [
      ...extractCalls(source, 'invoke').map(call => `${file}:invoke:${call}`),
      ...extractCalls(source, 'listen').map(call => `${file}:listen:${call}`),
    ];
  });
  const hash = crypto.createHash('sha256').update(signatures.join('\n')).digest('hex');
  if (!expectedProtocolHashes[domain]) console.log(`${domain}: ${hash}`);
  else assert.equal(hash, expectedProtocolHashes[domain], `${domain} bridge protocol changed`);
}

const featureRegistry = new Proxy({}, {
  get() {
    return () => new Proxy({}, { get: () => function () {} });
  },
});
const windowObject = {
  __TAURI__: {
    core: { invoke: async () => null },
    event: { listen: async () => function () {} },
    dialog: { open: async () => null },
  },
  __PINVOU_TAURI_BRIDGE_FEATURES__: featureRegistry,
  location: { search: '' },
  performance: { now: () => 0 },
  setTimeout,
  clearTimeout,
};
const context = vm.createContext({
  window: windowObject,
  document: { readyState: 'loading', addEventListener() {} },
  console,
  setTimeout,
  clearTimeout,
  structuredClone,
  URL,
  Blob,
});
vm.runInContext(read('bridge.js'), context, { filename: 'bridge.js' });

const api = windowObject.TauriBridge;
const expectedApi = {
  lifecycle: ['init'], state: ['get', 'getMany', 'subscribe', 'subscribeMany'], platform: ['loadPlatformCapabilities', 'refreshConnectorAuthGates'],
  chat: ['cancelGeneration', 'cancelShellTask', 'prefillComposer', 'removeQueued', 'sendMessage', 'sendMessageToSession'],
  voice: ['appendVoiceText', 'cancelVoiceAsrSetup', 'cancelVoiceInput', 'clearVoiceInput', 'closeVoiceAsrSetup', 'installVoiceAsr', 'runVoiceInputDebugAssertions', 'startVoiceInput'],
  knowledge: ['cancelKbModel', 'downloadKbModel', 'kbModelStatus', 'listCollections', 'loadKnowledgeEmbedderAfterFirstFrame', 'mountCollection', 'unmountCollection'],
  scheduled: ['clearScheduledTaskDraft', 'clearScheduledTaskSelection', 'confirmScheduledTaskDraft', 'createScheduledTask', 'deleteScheduledTask', 'dismissScheduledTaskError', 'exitScheduledRunChat', 'loadScheduledTaskRecentRuns', 'loadScheduledTaskRuns', 'loadScheduledTasks', 'openScheduledRunChat', 'pauseScheduledTask', 'pickFolder', 'readScheduledTask', 'refreshScheduledTaskData', 'resumeScheduledTask', 'runScheduledTaskNow', 'selectScheduledTask', 'startScheduledTaskChat', 'toggleScheduledTaskPinned', 'updateScheduledTask'],
  sessions: ['archiveSession', 'createNewSession', 'deleteSession', 'renameSession', 'restoreArchivedSession', 'switchToSession', 'toggleSessionPinned'],
  monitor: ['clearMonitorStats', 'startMonitorPolling', 'stopMonitorPolling'],
  settings: ['saveSettings', 'saveSettingsAndRestart', 'setSelectedPet', 'testSearchProvider'], feedback: ['submitFeedback'],
  llmapi: ['ensureLlmApiBinding', 'getLlmApiAdminOverview', 'getLlmApiModels', 'getLlmApiStatus', 'loginLlmApiUser', 'retryLlmApiProvisioning', 'saveLlmApiUserSession', 'setLlmApiDefaultModel', 'setLlmApiUserEnabled'],
  vllm: ['bootstrapLocalVllm', 'declineVllmSetup', 'detectLocalVllmSetup', 'dismissVllmSetup', 'discoverLocalVllm'],
  models: ['deleteModel', 'getEffectiveModelConfig', 'loadModels', 'loadSessionModel', 'revealModelApiKey', 'saveModel', 'setActiveModel', 'switchModel', 'testModelConnection'],
  interaction: ['acceptPlan', 'cancelUserInput', 'compactNow', 'discardPlan', 'dismissPinvouReview', 'editLastTurn', 'exitPlanToYolo', 'inspectPinvou', 'planStuckGo', 'planStuckReplan', 'resolvePinvouReview', 'setPlanModeNext', 'submitUserInput', 'summonPinvou', 'toggleSuperPerm'],
  rendering: ['renderMarkdown'], remoteControl: ['getWebRelaySettings', 'refreshRemoteControlQr', 'refreshRemoteControlStatus', 'resetWebRelayAddress', 'setWebRelayAddress', 'startRemoteControl', 'stopRemoteControl'],
  artifacts: ['artifactInfo', 'downloadArtifact', 'listDeliverableIndex', 'listDeliverables', 'openArtifactExternal', 'openContainingFolder', 'openExternalUrl', 'openInSystem', 'openScheduledTaskFolder', 'readArtifactImageB64', 'readArtifactText', 'readArtifactThumbnail', 'renderArtifactVisual', 'revealSessionFolder', 'writeArtifactText'],
  attachments: ['addAttachmentByPath', 'addPasteImage', 'clearAttachments', 'pickAndAttach', 'removeAttachment'], resolutions: ['markResolved'],
  workflow: ['activateSkill', 'addMaterialsToSession', 'approveWorkflowGate', 'attachRun', 'closeDemo', 'closeWorkflowDrawer', 'deactivateSkill', 'getGateReport', 'getRoleLogs', 'getRoleOutputs', 'getRolePrompt', 'listWorkflows', 'loadSkills', 'openDemo', 'pickAndAddMaterials', 'rejectWorkflowGate', 'resetWorkflowRun', 'resumeWorkflowOnBoot', 'retryWorkflowRole', 'selectWorkflowRole', 'setCurrentPhase', 'startWorkflowTask', 'stopWorkflowTask', 'submitWorkflowUserInput'],
  files: ['pickFeedbackFiles', 'pickFiles'],
  personas: ['createPersona', 'deletePersona', 'equipPersona', 'getPersonas', 'loadPersonas', 'postCardCreatorIntro', 'readPersonaBody', 'unequipPersona', 'updatePersona'],
  memory: ['archiveRecentWorkMemory', 'confirmMemoryCandidate', 'deleteMemoryItem', 'deleteMemoryPreference', 'ignoreMemoryCandidate', 'loadMemoryOverview', 'neverMemoryCandidate', 'saveMemoryProfilePatch', 'updateMemoryItem'],
  updater: ['cancelUpdate', 'checkForUpdate', 'downloadAndInstallUpdate', 'restartApp'], dependencies: ['checkDependencies', 'installDependencies'],
};

assert.deepEqual(Object.keys(api).sort(), ['available', ...Object.keys(expectedApi)].sort());
for (const [domain, methods] of Object.entries(expectedApi)) {
  assert.deepEqual(Object.keys(api[domain]).sort(), methods.sort(), `${domain} API surface changed`);
}
assert.equal(api.sendMessage, undefined, 'flat compatibility facade must not return');
assert.equal(api.getState, undefined, 'flat state facade must not return');

function sourceFiles(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const absolute = path.join(directory, entry.name);
    if (entry.isDirectory()) return sourceFiles(absolute);
    return /\.(?:js|jsx)$/.test(entry.name) ? [absolute] : [];
  });
}
for (const file of sourceFiles(path.join(root, 'src'))) {
  if (file.startsWith(bridgeRoot)) continue;
  const source = fs.readFileSync(file, 'utf8');
  if (file.startsWith(path.join(root, 'src', 'features'))) {
    assert.doesNotMatch(
      source,
      /\b(?:window|globalThis)\s*\.\s*__TAURI__\b/,
      `${path.relative(root, file)} must use the platform Tauri client`,
    );
  }
  for (const match of source.matchAll(/\bbridge\.([A-Za-z_$][\w$]*)\.([A-Za-z_$][\w$]*)/g)) {
    const [, domain, method] = match;
    assert.equal(typeof api[domain]?.[method], 'function', `${path.relative(root, file)} uses unknown bridge API ${domain}.${method}`);
  }
}

const clientSource = read('client.js');
const client = await import(`data:text/javascript;base64,${Buffer.from(clientSource).toString('base64')}`);
const previousTauri = globalThis.__TAURI__;
const nativeCalls = [];
class PhysicalPosition {
  constructor(x, y) { this.x = x; this.y = y; }
}
const currentWindow = { label: 'main' };
globalThis.__TAURI__ = {
  core: { invoke: async (command, payload) => { nativeCalls.push(['invoke', command, payload]); return 'ok'; } },
  event: {
    listen: async (name, handler) => { nativeCalls.push(['listen', name, handler]); return () => {}; },
    emit: async (name, payload) => { nativeCalls.push(['emit', name, payload]); },
  },
  window: {
    getCurrentWindow: () => currentWindow,
    currentMonitor: async () => ({ name: 'primary' }),
    availableMonitors: async () => [{ name: 'primary' }],
    PhysicalPosition,
  },
};
try {
  assert.equal(client.isTauriAvailable(), true);
  assert.equal(await client.invokeTauri('protocol_probe', { value: 1 }), 'ok');
  await client.listenTauri('protocol:event', () => {});
  await client.emitTauri('protocol:emit', { value: 2 });
  assert.equal(client.getCurrentTauriWindow(), currentWindow);
  assert.deepEqual(await client.currentTauriMonitor(), { name: 'primary' });
  assert.deepEqual(await client.availableTauriMonitors(), [{ name: 'primary' }]);
  const position = client.createPhysicalPosition(10.6, -2.4);
  assert.equal(position.x, 11);
  assert.equal(position.y, -2);
  assert.deepEqual(nativeCalls.slice(0, 3).map(call => call.slice(0, 2)), [
    ['invoke', 'protocol_probe'],
    ['listen', 'protocol:event'],
    ['emit', 'protocol:emit'],
  ]);
} finally {
  if (previousTauri === undefined) delete globalThis.__TAURI__;
  else globalThis.__TAURI__ = previousTauri;
}
console.log('bridge domain API and protocol contracts passed');
