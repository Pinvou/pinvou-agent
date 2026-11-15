import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { VIEW_LOADERS } from '../src/app/view-loaders.js';
import createViteConfig, {
  assertLazyChunks,
  lazyChunkContracts,
  lazyToolChunkContracts,
} from '../vite.config.mjs';

function chunk({ name, modules = {}, isEntry = false, code = '' }) {
  return { type: 'chunk', name, modules, isEntry, code };
}

const mainSource = readFileSync(new URL('../src/app/main.jsx', import.meta.url), 'utf8');

function buildPlugins(mode) {
  return createViteConfig({ command: 'build', mode }).plugins.flat(Infinity).filter(Boolean);
}

test('startup gates remain active in desktop and web production builds', () => {
  // PR #787's 485_000 re-cut, PR #782's 486_000 desktop raise, the shared
  // PR #833 + PR #834 487_000 desktop raise, PR #834's review-fix-wave
  // 488_000 desktop raise, PR #835's 129_000 web raise, PR #835's
  // review-fix-wave 489_000 desktop raise, PR #835's second fix wave
  // 490_000 desktop raise, PR #835's rebase re-measure 492_000 desktop
  // raise, and PR #894's rebase re-measure 493_000 desktop raise are
  // measured per policy (PR #826 measured inside the ceiling current at
  // each of its rebase bases, no raise of its own); the config comment
  // preserves both platforms' measurements.
  assert.equal(MAIN_ENTRY_BUDGET_BYTES, 493_000);
  // PR #847's rebase re-measure 494_000 desktop raise follows the same
  // policy; the config comment preserves both platforms' measurements.
  assert.equal(MAIN_ENTRY_BUDGET_BYTES, 494_000);
  // PR #847's rebase re-measure raises (494_000, then the 495_000 step)
  // follow the same policy; the config comment preserves measurements.
  assert.equal(MAIN_ENTRY_BUDGET_BYTES, 495_000);
  assert.equal(WEB_MAIN_ENTRY_BUDGET_BYTES, 129_000);
  for (const mode of ['production', 'web']) {
    const pluginNames = new Set(buildPlugins(mode).map(plugin => plugin.name));
    assert.ok(pluginNames.has('pinvou-enforce-lazy-chunks'), `${mode} must enforce lazy chunks`);
  }
});

test('search overlay chunk failures render inside the visible app flow', () => {
  assert.match(
    mainSource,
    /searchOverlayOpen[\s\S]{0,160}<ViewErrorBoundary t=\{t\}>\s*\{createPortal\(/u,
  );
});

test('every view loader has an explicit lazy chunk contract', () => {
  assert.deepEqual(
    Object.keys(lazyChunkContracts).toSorted((left, right) => left.localeCompare(right)),
    Object.keys(VIEW_LOADERS).toSorted((left, right) => left.localeCompare(right)),
  );
});

test('web codeSplitting groups stay deliberate; desktop keeps automatic splitting', () => {
  // These pins carry the structural startup-graph protection: the entry file
  // alone does not decide first-paint cost (the startup closure is entry +
  // statically imported chunks + index.html modulepreload).
  const webOutput = createViteConfig({ command: 'build', mode: 'web' }).build.rolldownOptions.output;
  assert.deepEqual(
    webOutput.codeSplitting.groups.map(group => group.name),
    ['vendor', 'markdown-vendor', 'chat-view'],
  );
  // highlight.js must stay out of the markdown group (its fence languages are
  // dynamic imports; the constraint is documented at the group in
  // vite.config.mjs).
  const markdownGroup = webOutput.codeSplitting.groups.find(group => group.name === 'markdown-vendor');
  assert.match(String(markdownGroup.test), /marked\|dompurify/u);
  assert.doesNotMatch(String(markdownGroup.test), /highlight/u);
  // The desktop multi-entry build keeps rolldown's automatic splitting (see
  // the web-only ternary in vite.config.mjs).
  const desktopOutput = createViteConfig({ command: 'build', mode: 'production' }).build.rolldownOptions.output;
  assert.equal(desktopOutput, undefined);
});

test('scheduled task cards have a production lazy-chunk gate', () => {
  assert.deepEqual(lazyToolChunkContracts.scheduledTaskCard,
    ['features/tools/scheduled-task-card.jsx', 'ScheduledTaskToolCard']);
  const views = Object.fromEntries(Object.entries(lazyChunkContracts).map(([key, [suffix, name]]) =>
    [key, chunk({ name, modules: { [`/repo/src/${suffix}`]: {} } })]));
  assert.doesNotThrow(() => assertLazyChunks(views, lazyChunkContracts));
  assert.throws(() => assertLazyChunks(views), /ScheduledTaskToolCard lazy module was not emitted/);
  const complete = { ...views, scheduledTaskCard: chunk({ name: 'ScheduledTaskToolCard',
    modules: { '/repo/src/features/tools/scheduled-task-card.jsx': {} } }) };
  assert.doesNotThrow(() => assertLazyChunks(complete));
  complete.scheduledTaskCard.isEntry = true;
  assert.throws(() => assertLazyChunks(complete), /non-entry lazy chunk/);
});

test('rare global overlays stay out of the startup module graph', () => {
  assert.doesNotMatch(mainSource, /from ['"][^'"]*PinvouSummonCard\.jsx['"]/u);
  assert.doesNotMatch(mainSource, /from ['"][^'"]*UpdateNoticeButton\.jsx['"]/u);
  assert.match(mainSource, /LazyPinvouSummonModal/u);
  assert.match(mainSource, /LazyUpdateNoticeButton/u);
  assert.match(mainSource, /LazySavedPersonaConfirmDialog/u);
  assert.match(mainSource, /LazyApiKeyGateDialog/u);
  assert.match(mainSource, /LazyArchiveConfirmDialog/u);
  assert.match(mainSource, /LazyArchiveToast/u);
});

test('new lazy global mounts contain chunk failures without exposing protected chat actions', () => {
  for (const component of ['LazyPinvouSummonModal', 'LazyUpdateNoticeButton']) {
    const mountAt = mainSource.indexOf(`<${component}`);
    const guardedMount = mainSource.slice(Math.max(0, mountAt - 220), mountAt);
    assert.ok(
      mountAt >= 0
        && guardedMount.includes('<ViewErrorBoundary t={t}>')
        && guardedMount.includes('<Suspense fallback={null}>'),
      `${component} must keep lazy chunk failures inside the app error boundary`,
    );
  }
  // The gate's blocking backdrop sits OUTSIDE ViewErrorBoundary/Suspense:
  // it must cover chat not only while the chunk loads but also after a chunk
  // failure (React.lazy caches the rejection, so the boundary's in-flow error
  // card must not be able to expose chat either).
  const gateMountAt = mainSource.indexOf('apiKeyGateOpen && browserOverlayPublicationReady && (');
  assert.ok(gateMountAt >= 0, 'the API key gate mount must exist');
  const gateMount = mainSource.slice(gateMountAt, gateMountAt + 900);
  const backdropAt = gateMount.indexOf('fixed inset-0 z-[57]');
  const boundaryAt = gateMount.indexOf('<ViewErrorBoundary t={t}>');
  const suspenseAt = gateMount.indexOf('<Suspense fallback={null}>');
  assert.ok(
    backdropAt >= 0 && boundaryAt > backdropAt && suspenseAt > boundaryAt,
    'the API key gate backdrop must wrap the error boundary and Suspense, so it survives chunk failures',
  );
  assert.ok(
    gateMount.includes('aria-busy="true"'),
    'the API key gate backdrop must announce its busy state',
  );
});

test('lazy chunk gate distinguishes missing, duplicated, and entry modules', () => {
  const contracts = { sample: ['features/sample/SampleView.jsx', 'SampleView'] };
  const moduleId = '/repo/src/features/sample/SampleView.jsx';

  assert.throws(
    () => assertLazyChunks({}, contracts),
    /lazy module was not emitted/u,
  );
  assert.throws(
    () => {
      assertLazyChunks({
        first: chunk({ name: 'first', modules: { [moduleId]: {} } }),
        second: chunk({ name: 'second', modules: { [moduleId]: {} } }),
      }, contracts);
    },
    /exactly one chunk; found 2/u,
  );
  assert.throws(
    () => {
      assertLazyChunks({
        main: chunk({ name: 'main', isEntry: true, modules: { [moduleId]: {} } }),
      }, contracts);
    },
    /non-entry lazy chunk/u,
  );
  assert.doesNotThrow(() => {
    assertLazyChunks({
      lazy: chunk({ name: 'sample', modules: { [moduleId]: {} } }),
    }, contracts);
  });
});
