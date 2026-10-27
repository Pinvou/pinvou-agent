import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { VIEW_LOADERS } from '../src/app/view-loaders.js';
import createViteConfig, {
  MAIN_ENTRY_BUDGET_BYTES,
  WEB_MAIN_ENTRY_BUDGET_BYTES,
  assertLazyChunks,
  assertMainEntryBudget,
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
  // PR #787's 485_000 desktop / 128_000 web re-cut is measured per policy;
  // the config comment preserves both platforms' measurements.
  assert.equal(MAIN_ENTRY_BUDGET_BYTES, 485_000);
  assert.equal(WEB_MAIN_ENTRY_BUDGET_BYTES, 128_000);
  for (const mode of ['production', 'web']) {
    const pluginNames = new Set(buildPlugins(mode).map(plugin => plugin.name));
    assert.ok(pluginNames.has('pinvou-enforce-lazy-chunks'), `${mode} must enforce lazy chunks`);
    assert.ok(pluginNames.has('pinvou-enforce-main-entry-budget'), `${mode} must enforce the entry budget`);
  }
});

test('each build mode enforces its own main entry ceiling', () => {
  // The web entry is a quarter of the desktop entry after the web groups, so
  // a shared ceiling would never fire for web; the plugin must bind the
  // ceiling to the mode it was created for.
  for (const [mode, budget] of [['production', MAIN_ENTRY_BUDGET_BYTES], ['web', WEB_MAIN_ENTRY_BUDGET_BYTES]]) {
    const plugin = buildPlugins(mode).find(candidate => candidate.name === 'pinvou-enforce-main-entry-budget');
    const atBudget = chunk({ name: 'main', isEntry: true, code: 'x'.repeat(budget) });
    assert.doesNotThrow(() => plugin.writeBundle({}, { main: atBudget }), `${mode} must accept an entry at its ceiling`);
    const overBudget = chunk({ name: 'main', isEntry: true, code: `${atBudget.code}x` });
    assert.throws(
      () => plugin.writeBundle({}, { main: overBudget }),
      new RegExp(`Main entry chunk ${budget + 1} B exceeds the ${budget} B startup budget`, 'u'),
      `${mode} must reject an entry one byte over its ceiling`,
    );
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
  // The entry-budget gate only measures the entry FILE, not the startup
  // closure (entry + statically imported chunks + index.html modulepreload),
  // so these pins carry the structural protection the byte gate cannot.
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

test('main entry gate measures bytes and enforces the exact budget', () => {
  const atBudget = chunk({ name: 'main', isEntry: true, code: 'x'.repeat(MAIN_ENTRY_BUDGET_BYTES) });
  assert.equal(assertMainEntryBudget({ main: atBudget }), MAIN_ENTRY_BUDGET_BYTES);

  const overBudget = chunk({ name: 'main', isEntry: true, code: `${atBudget.code}界` });
  assert.throws(
    () => assertMainEntryBudget({ main: overBudget }),
    new RegExp(`Main entry chunk ${MAIN_ENTRY_BUDGET_BYTES + 3} B exceeds`, 'u'),
  );
  assert.throws(() => assertMainEntryBudget({}), /did not emit the main entry chunk/u);
});
