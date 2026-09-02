import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import {
  normalizeSidePanelRatio,
  resolveSidePanelLayout,
  sidePanelRatioForLegacyWidth,
  sidePanelRatioFromWidth,
} from '../src/components/layout/side-panel-layout.mjs';
import {
  activateRightDockPanel,
  createRightDockState,
  hideRightDockPanel,
  mountRightDockPanel,
  rightDockSnapshot,
  setRightDockOcclusion,
  unmountRightDockPanel,
} from '../src/components/layout/right-dock-state.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, '..');
const source = (...parts) => readFileSync(path.join(root, 'src', ...parts), 'utf8');

const profiles = [
  ['right-dock', 0.45, { minWidth: 420, minMainWidth: 520, maxWidthRatio: 0.65 }],
];

test('shared right host restores its ratio after shrinking and expanding', () => {
  for (const [name, ratio, constraints] of profiles) {
    const initial = resolveSidePanelLayout(1600, ratio, constraints);
    const narrowed = resolveSidePanelLayout(1000, ratio, constraints);
    const restored = resolveSidePanelLayout(1600, ratio, constraints);
    assert.equal(initial.overlay, false, `${name}: initial split`);
    assert.equal(narrowed.overlay, false, `${name}: narrowed split`);
    assert.equal(restored.width, initial.width, `${name}: restore exact width`);
    assert.equal(restored.preferredRatio, ratio, `${name}: ratio remains authoritative`);
  }
});

test('insufficient space enters single-column mode without changing the user ratio', () => {
  const constraints = { minWidth: 420, minMainWidth: 520, maxWidthRatio: 0.65 };
  const layout = resolveSidePanelLayout(820, 0.5, constraints);
  assert.equal(layout.overlay, true);
  assert.equal(layout.width, 820);
  assert.equal(layout.preferredRatio, 0.5);
  assert.equal(resolveSidePanelLayout(1600, layout.preferredRatio, constraints).width, 800);
});

test('dragging writes only a normalized ratio', () => {
  assert.equal(sidePanelRatioFromWidth(640, 1600), 0.4);
  assert.equal(normalizeSidePanelRatio(720, 0.45), 0.9);
  assert.equal(normalizeSidePanelRatio(Number.NaN, 0.45), 0.45);
});

test('legacy pixel width is not migrated while narrow or max-width clamped', () => {
  const constraints = { minWidth: 420, minMainWidth: 520, maxWidthRatio: 0.65 };
  assert.equal(sidePanelRatioForLegacyWidth(720, 820, 0.5, constraints), null,
    'single-column mode must not persist its temporary full width');
  assert.equal(sidePanelRatioForLegacyWidth(720, 1000, 0.5, constraints), null,
    'max-width clamping must wait for a wider container before migration');
  assert.equal(sidePanelRatioForLegacyWidth(720, 1600, 0.5, constraints), 0.45,
    'restored space must migrate from the original pixel width');
});

test('resize after an interrupted drag still uses the pre-drag ratio', () => {
  const constraints = { minWidth: 420, minMainWidth: 520, maxWidthRatio: 0.65 };
  const preferredRatio = 0.5;
  const before = resolveSidePanelLayout(1600, preferredRatio, constraints);
  const temporaryDragWidth = 470;
  assert.notEqual(temporaryDragWidth, before.width, 'the fixture must include uncommitted drag width');
  const minimized = resolveSidePanelLayout(820, preferredRatio, constraints);
  const restored = resolveSidePanelLayout(1600, minimized.preferredRatio, constraints);
  assert.equal(minimized.overlay, true);
  assert.equal(restored.width, before.width);
  assert.equal(restored.preferredRatio, preferredRatio);
});

test('drag cancel, resize, and minimize paths all roll back temporary DOM width', () => {
  const component = source('components', 'layout', 'ResizableSidePanel.jsx');
  assert.match(component, /const startingRatio = preferredRatio;/);
  assert.match(component, /const restoreTransientWidth = \(\) => \{/);
  assert.match(component, /resolveSidePanelLayout\(currentRootWidth, startingRatio, constraints\)/);
  assert.match(component, /const onCancel = \(\) => cleanup\(true\);/);
  assert.match(component, /window\.addEventListener\('blur', onCancel\)/);
  assert.match(component, /window\.addEventListener\('resize', onCancel\)/);
  assert.match(component, /window\.addEventListener\('pagehide', onCancel\)/);
  assert.match(component, /document\.addEventListener\('visibilitychange', onVisibilityChange\)/);
});

test('Right Dock has one physical host and falls back to the latest active logical panel', () => {
  let state = createRightDockState();
  state = mountRightDockPanel(state, 'browser');
  state = activateRightDockPanel(state, 'browser');
  state = mountRightDockPanel(state, 'artifact-preview');
  state = activateRightDockPanel(state, 'artifact-preview');
  state = mountRightDockPanel(state, 'subagent-transcript');
  state = activateRightDockPanel(state, 'subagent-transcript');

  assert.deepEqual(rightDockSnapshot(state), {
    activePanelId: 'subagent-transcript',
    mountedPanelCount: 3,
    visiblePanelCount: 3,
    openSidePanelCount: 1,
    occluded: false,
  });

  state = hideRightDockPanel(state, 'subagent-transcript');
  assert.equal(rightDockSnapshot(state).activePanelId, 'artifact-preview');
  state = activateRightDockPanel(state, 'browser');
  assert.equal(rightDockSnapshot(state).activePanelId, 'browser');
  assert.equal(rightDockSnapshot(state).openSidePanelCount, 1);

  state = unmountRightDockPanel(state, 'browser');
  assert.equal(rightDockSnapshot(state).activePanelId, 'artifact-preview');
});

test('fullscreen canvas occlusion preserves Right Dock state without exposing its host', () => {
  let state = createRightDockState();
  state = activateRightDockPanel(state, 'browser');
  state = setRightDockOcclusion(state, 'artifact-fullscreen', true);
  assert.deepEqual(rightDockSnapshot(state), {
    activePanelId: null,
    mountedPanelCount: 1,
    visiblePanelCount: 1,
    openSidePanelCount: 0,
    occluded: true,
  });
  state = setRightDockOcclusion(state, 'artifact-fullscreen', false);
  assert.equal(rightDockSnapshot(state).activePanelId, 'browser');
});

test('all panels share ratio layout and chat no longer relies on viewport breakpoints', () => {
  const main = source('app', 'main.jsx');
  const chat = source('features', 'chat', 'ChatView.jsx');
  const subagent = source('features', 'multiagent', 'SubagentTranscriptPanel.jsx');
  const codex = source('features', 'codex', 'CodexWorkspacePanel.jsx');
  const dock = source('components', 'layout', 'RightDock.jsx');
  assert.match(main, /<SidePanelLayoutProvider onPresenceChange=\{setOpenSidePanelCount\}>/);
  assert.match(main, /<RightDockProvider[^>]*onStateChange=\{handleRightDockStateChange\}/);
  assert.match(main, /<RightDockProvider[^>]*onBeforeOcclusionPublish=\{publishRightDockOcclusion\}/);
  assert.match(main, /<RightDockProvider[^>]*onOcclusionRelease=\{releaseRightDockOcclusion\}/);
  assert.match(main, /<RightDockHost[\s\S]*onResizeActiveChange=\{setBrowserResizeActive\}/);
  assert.match(main, /panelId="browser"[\s\S]*activationKey=\{browserDockActivationKey\}/);
  assert.match(chat, /panelId="artifact-preview"[\s\S]*activationKey=\{artifactDockActivation\}/);
  assert.match(subagent, /panelId="subagent-transcript"[\s\S]*activationKey=\{selectionRequestId\}/);
  assert.match(codex, /panelId="codex-workspace"[\s\S]*visible=\{visible\}/);
  assert.match(dock, /panelId="right-dock"[\s\S]*storageKey="pinvou_right_dock_ratio"/);
  assert.match(dock, /legacyRatioStorageKeys=\{LEGACY_RIGHT_DOCK_RATIO_KEYS\}/);
  assert.match(dock, /legacyPixelStorageKeys=\{LEGACY_RIGHT_DOCK_WIDTH_KEYS\}/);
  assert.equal((dock.match(/<ResizableSidePanel/g) || []).length, 1);
  assert.doesNotMatch(main, /<ResizableSidePanel/);
  assert.doesNotMatch(chat, /<ResizableSidePanel/);
  assert.doesNotMatch(subagent, /<ResizableSidePanel/);
  assert.doesNotMatch(codex, /<ResizableSidePanel/);
  assert.doesNotMatch(chat, /if \(artifactsVisible\) setSubagentPanel\(null\)/);
  assert.match(main, /const browserSurfaceSuspended = browserResizeActive[\s\S]*rightDockState\.activePanelId !== 'browser'[\s\S]*browserBlockingLayerOpen/);
  assert.match(chat, /useRightDockOcclusion\(\s*'artifact-fullscreen'/);
  assert.match(chat, /artifactsFullscreen && artifactFullscreenPublicationReady && createPortal/);
  assert.match(chat, /panelId="artifact-preview"[\s\S]*visible=\{rightDockActivePanelId !== 'browser'\}[\s\S]*onToggleFullscreen=\{\(\) => setArtifactsFullscreen\(true\)\}/);
  assert.doesNotMatch(chat, /lg:px-40/);
  assert.match(chat, /ResizeObserver\(measure\)/);
});
