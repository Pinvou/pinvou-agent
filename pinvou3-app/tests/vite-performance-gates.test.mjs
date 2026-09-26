import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { VIEW_LOADERS } from '../src/app/view-loaders.js';
import createViteConfig, {
  MAIN_ENTRY_BUDGET_BYTES,
  assertLazyChunks,
  assertMainEntryBudget,
  lazyChunkContracts,
} from '../vite.config.mjs';

function chunk({ name, modules = {}, isEntry = false, code = '' }) {
  return { type: 'chunk', name, modules, isEntry, code };
}

const mainSource = readFileSync(new URL('../src/app/main.jsx', import.meta.url), 'utf8');

test('startup gates remain active in desktop and web production builds', () => {
  assert.equal(MAIN_ENTRY_BUDGET_BYTES, 500_000);
  for (const mode of ['production', 'web']) {
    const config = createViteConfig({ command: 'build', mode });
    const pluginNames = new Set(config.plugins.flat(Infinity).filter(Boolean).map(plugin => plugin.name));
    assert.ok(pluginNames.has('pinvou-enforce-lazy-chunks'), `${mode} must enforce lazy chunks`);
    assert.ok(pluginNames.has('pinvou-enforce-main-entry-budget'), `${mode} must enforce the entry budget`);
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
