import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const read = relativePath => readFileSync(new URL(`../${relativePath}`, import.meta.url), 'utf8');

test('PinvouOS home exposes one Pinvou instead of conversation navigation', () => {
  const main = read('src/app/main.jsx');
  const navStart = main.indexOf('data-testid="sidebar-primary-nav"');
  const recentsStart = main.indexOf('{/* Recents', navStart);
  const primaryNavigation = main.slice(navStart, recentsStart);

  assert.match(main, /const PINVOU_OS_UI_ENABLED = true/);
  assert.match(main, /const LEGACY_EXPERT_SURFACES_ENABLED = !PINVOU_OS_UI_ENABLED/);
  assert.match(primaryNavigation, /label="Pinvou"/);
  assert.doesNotMatch(primaryNavigation, /label=\{t\.newChat\}/);
  assert.match(main, /isSidebarOpen && !PINVOU_OS_UI_ENABLED/);
  assert.match(main, /codeModeAvailable=\{false\}/);
  assert.match(main, /multiAgentAvailable=\{LEGACY_EXPERT_SURFACES_ENABLED\}/);
  assert.match(main, /<PinvouOsVoiceShell/);
  assert.match(main, /onSubmitPrompt=\{handlePinvouOsVoicePrompt\}/);
});

test('PinvouOS retires the Three Departments and expert card-pool surfaces', () => {
  const main = read('src/app/main.jsx');
  const chat = read('src/features/chat/ChatView.jsx');

  assert.match(main, /LEGACY_EXPERT_SURFACES_ENABLED && \(\s*<NavItem\s+icon=\{<Layers size=\{18\} \/>\} label=\{t\.cardPool\}/);
  assert.match(main, /LEGACY_EXPERT_SURFACES_ENABLED && currentView === 'cardpool'/);
  assert.match(main, /LEGACY_EXPERT_SURFACES_ENABLED && \(currentView === 'chat'/);
  assert.match(main, /setMultiAgentMode\(false\)/);
  assert.match(main, /nextView === 'cardpool'[\s\S]*nextView = 'chat'/);
  assert.match(chat, /multiAgentAvailable = true/);
  assert.match(chat, /multiAgentAvailable=\{multiAgentAvailable\}/);
  assert.match(chat, /multiAgentAvailable && subagentPanel/);
  assert.match(chat, /pd\.draft && onOpenEditor/);
});

test('Agent Runtime dock is backed by native runtime truth and has no legacy terminology', () => {
  const dock = read('src/features/pinvou_os/PinvouOsAgentDock.jsx');
  const shell = read('src/features/pinvou_os/PinvouOsVoiceShell.jsx');
  const shellCss = read('src/features/pinvou_os/pinvou-os-voice-shell.css');
  const api = read('src/features/pinvou_os/runtime-api.js');
  const i18n = read('src/shared/i18n.js');

  assert.match(dock, /data-testid="pinvou-os-agent-dock"/);
  assert.match(dock, /data-testid="pinvou-os-agent-card"/);
  assert.match(dock, /data-testid="pinvou-os-agent-dock-close"/);
  assert.match(dock, /data-testid="pinvou-os-agent-dock-open"[\s\S]*?onClick=\{\(\) => setOpen\(true\)\}/);
  assert.match(dock, /data-testid="pinvou-os-agent-dock-close"[\s\S]*?onClick=\{\(\) => setOpen\(false\)\}/);
  assert.doesNotMatch(dock, /onPointerDown=\{\(\) => setOpen\((?:true|false)\)\}/);
  assert.doesNotMatch(dock, /session|会话/i);
  assert.match(dock, /useState\(false\)/);
  assert.match(dock, /data-testid="pinvou-os-network-summary"/);
  assert.match(dock, /data-testid="pinvou-os-model-summary"/);
  assert.match(dock, /data-testid="pinvou-os-runtime-health"/);
  assert.match(dock, /inference\.lastSuccessAtMs/);
  assert.match(dock, /connectivity\.reasonCode/);
  assert.match(shell, /data-testid="pinvou-os-microphone"/);
  assert.match(shell, /bridge\.voice\.startVoiceInput/);
  assert.match(shell, /<Sparkles size=\{30\} className="pinvou-os-mic-processing" \/>/);
  assert.doesNotMatch(shell, /pinvou-os-mic-spinner|RotateCcw/);
  assert.match(shell, /aria-label=\{recording \? t\.voiceStop : transcribing \? t\.voiceTranscribing/);
  assert.match(shellCss, /\.pinvou-os-mic\.is-transcribing \* \{ animation: none !important; \}/);
  assert.doesNotMatch(shellCss, /pinvouOsSpin|pinvou-os-mic-spinner/);
  assert.doesNotMatch(shell, /title=\{recording \? t\.voiceStop/);
  assert.match(shell, /<PinvouOsAgentDock theme=\{theme\} t=\{t\} \/>/);
  assert.match(api, /invokeTauri\('get_pinvou_os_snapshot'\)/);
  assert.match(api, /invokeTauri\('list_pinvou_os_events'/);
  assert.match(api, /tauriEvents\.listen\('pinvou-os:event'/);
  assert.match(api, /'agent:connectivity'/);
  assert.match(api, /'agent:inference'/);
  assert.match(api, /'agent:asr-context'/);
  assert.match(api, /'agent:screen-observer'/);
  assert.match(dock, /'agent:screen-observer': Monitor/);
  assert.match(i18n, /'agent:screen-observer':\{ name:'界面感知 Agent'/);
  assert.match(i18n, /'agent:screen-observer':\{ name:'Screen Observer Agent'/);
  assert.doesNotMatch(`${api}\n${dock}\n${i18n}`, /agent:surface|屏幕 Agent|Surface Agent/);
});

test('PinvouOS canvas consumes a read-only namespaced A2UI v0.9 projection', () => {
  const shell = read('src/features/pinvou_os/PinvouOsVoiceShell.jsx');
  const surface = read('src/features/pinvou_os/PinvouOsProjectionSurface.jsx');
  const protocol = read('src/features/pinvou_os/a2ui-runtime.js');
  const api = read('src/features/pinvou_os/runtime-api.js');
  const nativeProjection = read('src-tauri/src/features/pinvou_os/interaction_projection.rs');

  assert.match(shell, /<PinvouOsProjectionSurface t=\{t\}/);
  assert.match(shell, /data-testid="pinvou-os-user-input-card"/);
  assert.match(shell, /<UserInputCard item=\{pendingUserInput\} t=\{t\} \/>/);
  assert.match(shell, /data-testid="pinvou-os-artifact-card"/);
  assert.match(shell, /<ArtifactCard item=\{visibleArtifact\}/);
  assert.match(surface, /data-testid="pinvou-os-a2ui-surface"/);
  assert.match(surface, /data-a2ui-surface-id=\{surface\.surfaceId\}/);
  assert.match(api, /invokeTauri\('get_pinvou_os_projection'\)/);
  assert.match(api, /tauriEvents\.listen\('pinvou-os:a2ui'/);
  assert.match(protocol, /const PROTOCOL_VERSION = 'v0\.9'/);
  assert.match(protocol, /const NAMESPACE = 'projection'/);
  assert.match(protocol, /message_requires_exactly_one_operation/);
  assert.match(protocol, /component_not_in_catalog/);
  assert.match(protocol, /operation\.sendDataModel !== false/);
  assert.doesNotMatch(protocol, /eval\(|new Function|dangerouslySetInnerHTML/);
  assert.match(nativeProjection, /"createSurface"/);
  assert.match(nativeProjection, /"updateComponents"/);
  assert.match(nativeProjection, /"updateDataModel"/);
  assert.match(nativeProjection, /assert!\(!serialized\.contains\("Button"\)\)/);
});

test('A2UI host accepts the frozen projection catalog and rejects executable or cross-surface input', async () => {
  const {
    applyA2uiProjection,
    EMPTY_A2UI_STATE,
  } = await import('../src/features/pinvou_os/a2ui-runtime.js');
  const envelope = messages => ({ namespace: 'projection', basisSequence: 7, messages });
  const created = applyA2uiProjection(EMPTY_A2UI_STATE, envelope([
    {
      version: 'v0.9',
      createSurface: {
        surfaceId: 'projection/runtime-overview',
        catalogId: 'urn:pinvou:a2ui:catalog:projection:v1',
        sendDataModel: false,
      },
    },
    {
      version: 'v0.9',
      updateComponents: {
        surfaceId: 'projection/runtime-overview',
        components: [{ id: 'root', component: 'PinvouCanvas', children: [] }],
      },
    },
    {
      version: 'v0.9',
      updateDataModel: {
        surfaceId: 'projection/runtime-overview',
        path: '/',
        value: { interaction: { status: 'running' } },
      },
    },
  ]));
  assert.equal(created.surfaceId, 'projection/runtime-overview');
  assert.equal(created.dataModel.interaction.status, 'running');

  assert.throws(() => applyA2uiProjection(created, envelope([{
    version: 'v0.9',
    updateComponents: {
      surfaceId: 'projection/runtime-overview',
      components: [{ id: 'root', component: 'Button', action: 'run' }],
    },
  }])), /component_not_in_catalog/);
  assert.throws(() => applyA2uiProjection(created, envelope([{
    version: 'v0.9',
    updateDataModel: { surfaceId: 'front/forged', path: '/', value: {} },
  }])), /surface_scope_mismatch/);
  assert.throws(() => applyA2uiProjection(created, envelope([{
    version: 'v0.9',
    createSurface: { surfaceId: 'projection/runtime-overview', catalogId: 'urn:pinvou:a2ui:catalog:projection:v1', sendDataModel: false },
    deleteSurface: { surfaceId: 'projection/runtime-overview' },
  }])), /message_requires_exactly_one_operation/);
});
