import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  computerUseConsentView,
  extractComputerUseScreenshotPath,
} from '../src/features/computer-use/computer-use-logic.js';

// ── 截图路径提取 ──────────────────────────────────────────────────
assert.equal(
  extractComputerUseScreenshotPath('已截图，保存至 /home/u/.pinvou3/sessions/s1/attachments/computer_use/shot_1.png'),
  '/home/u/.pinvou3/sessions/s1/attachments/computer_use/shot_1.png',
  'plain text with an absolute unix path must be extracted',
);
assert.equal(
  extractComputerUseScreenshotPath('screenshot saved: C:\\Users\\asto\\.pinvou3\\sessions\\s1\\attachments\\computer_use\\shot 2.png done'),
  'C:\\Users\\asto\\.pinvou3\\sessions\\s1\\attachments\\computer_use\\shot 2.png',
  'windows drive paths with backslash separators must be extracted',
);
assert.equal(
  extractComputerUseScreenshotPath('saved /ws/attachments/computer_use/shot.png.png and more'),
  '/ws/attachments/computer_use/shot.png.png',
  'a basename ending in .png.png must not be truncated at the first .png',
);
assert.equal(
  extractComputerUseScreenshotPath(JSON.stringify({
    content: [{ type: 'text', text: '截图已保存到 /ws/attachments/computer_use/envelope.png' }],
  })),
  '/ws/attachments/computer_use/envelope.png',
  'MCP-style JSON envelopes must be unwrapped before searching',
);
// Envelope + Windows path: the raw JSON's escaped backslashes must NOT win
// the last-match rule and produce a doubled-separator path (review finding).
assert.equal(
  extractComputerUseScreenshotPath(JSON.stringify({
    content: [{ type: 'text', text: 'saved C:\\Users\\u\\attachments\\computer_use\\win.png' }],
  })),
  'C:\\Users\\u\\attachments\\computer_use\\win.png'.replaceAll('\\\\', '\\'),
  'envelope Windows paths must resolve to the clean single-separator form',
);
assert.equal(
  extractComputerUseScreenshotPath('first /ws/attachments/computer_use/a.png then /ws/attachments/computer_use/b.png'),
  '/ws/attachments/computer_use/b.png',
  'a multi-step result resolves to the newest (last) screenshot',
);
// Envelope with NO text blocks: an envelope that parses must never fall back
// to searching the raw JSON — its escaped backslashes (\\) normalize into
// slash-doubled paths and non-text blocks must not surface either
// (review finding).
assert.equal(
  extractComputerUseScreenshotPath('{"content":[{"type":"image","text":"shot C:\\\\Users\\\\u/attachments/computer_use/mixed.png"}]}'),
  null,
  'an envelope without text blocks must not resurface the raw JSON as a slash-doubled path',
);
assert.equal(
  extractComputerUseScreenshotPath(JSON.stringify({
    content: [{ type: 'image', text: 'saved C:\\Users\\u\\attachments\\computer_use\\hidden.png' }],
  })),
  null,
  'a Windows path in a non-text block must not be extracted',
);
assert.equal(
  extractComputerUseScreenshotPath(JSON.stringify({
    content: [{ type: 'image', text: 'saved /ws/attachments/computer_use/hidden.png' }],
  })),
  null,
  'a forward-slash path in a non-text block must not be extracted either',
);
assert.equal(
  extractComputerUseScreenshotPath(JSON.stringify({ content: [] })),
  null,
  'an envelope with an empty content list resolves to no screenshot',
);
assert.equal(
  extractComputerUseScreenshotPath('SEE /WS/ATTACHMENTS/COMPUTER_USE/UP.PNG'),
  '/WS/ATTACHMENTS/COMPUTER_USE/UP.PNG',
  'matching is case-insensitive for the directory marker and extension',
);
assert.equal(
  extractComputerUseScreenshotPath('no screenshot in this step'),
  null,
  'output without a screenshot falls back to the default tool card',
);
assert.equal(
  extractComputerUseScreenshotPath('wrote /ws/attachments/notes/summary.png'),
  null,
  'PNGs outside attachments/computer_use must not be picked up',
);
assert.equal(
  extractComputerUseScreenshotPath('see attachments/computer_use/relative.png'),
  null,
  'relative paths without an absolute anchor must be rejected',
);
assert.equal(extractComputerUseScreenshotPath(null), null);
assert.equal(extractComputerUseScreenshotPath(''), null);
assert.equal(
  extractComputerUseScreenshotPath({ content: [{ type: 'text', text: 'saved /ws/attachments/computer_use/obj.png' }] }),
  '/ws/attachments/computer_use/obj.png',
  'object outputs are searched via their JSON serialization',
);

// ── 授权界面可见性状态机 ──────────────────────────────────────────
assert.deepEqual(
  computerUseConsentView(null),
  { enabled: false, stopped: false, showBanner: false, grantRequest: null, confirmRequest: null },
  'a missing slice hides every surface',
);
assert.deepEqual(
  computerUseConsentView({ enabled: false, granted: true, grantRequest: { sessionId: 's1' }, confirmRequest: { confirmId: 'c1' } }),
  { enabled: false, stopped: false, showBanner: false, grantRequest: null, confirmRequest: null },
  'feature toggle off: no banner, no dialogs even with stale requests',
);
assert.deepEqual(
  computerUseConsentView({ enabled: true, granted: false, stopped: false }),
  { enabled: true, stopped: false, showBanner: false, grantRequest: null, confirmRequest: null },
  'enabled but ungranted stays quiet until a request arrives',
);
const grantRequest = { sessionId: 's1' };
assert.deepEqual(
  computerUseConsentView({ enabled: true, grantRequest }),
  { enabled: true, stopped: false, showBanner: false, grantRequest, confirmRequest: null },
  'grant_required surfaces the grant dialog',
);
const confirmRequest = { sessionId: 's1', action: 'click', element: 'Save', confirmId: 'c1' };
assert.deepEqual(
  computerUseConsentView({ enabled: true, granted: true, stopped: false, confirmRequest }),
  { enabled: true, stopped: false, showBanner: true, grantRequest: null, confirmRequest },
  'a per-action confirmation stacks on top of the persistent banner',
);
assert.deepEqual(
  computerUseConsentView({ enabled: true, granted: true, stopped: true, confirmRequest }),
  { enabled: true, stopped: true, showBanner: false, grantRequest: null, confirmRequest: null },
  'stop() collapses the banner and the dialogs: stop_all already killed the pending confirm, so its approve button must not stay live (review finding)',
);
assert.deepEqual(
  computerUseConsentView({ enabled: true, granted: false, stopped: true, grantRequest }),
  { enabled: true, stopped: true, showBanner: false, grantRequest: null, confirmRequest: null },
  'stop() also collapses a pending grant dialog',
);
assert.deepEqual(
  computerUseConsentView({ enabled: true, granted: true, stopped: false }),
  { enabled: true, stopped: false, showBanner: true, grantRequest: null, confirmRequest: null },
  'granted and not stopped keeps the non-dismissible banner visible',
);

console.log('computer use logic tests passed');

// ── Consent dialog UI projection (real component, stubbed React runtime) ──
// node --test has no DOM renderer and the repo has no jsdom, so the dialog
// component is loaded through a bare Vite SSR server whose 'react' imports
// resolve to a minimal hooks runtime (same approach as
// tests/use_throttled_value.test.mjs) and whose JSX runtime produces plain
// element trees the assertions walk directly. This keeps the review-critical
// render behavior (inline short previews, error leakage) covered without a
// browser while the real component source is exercised unmodified.

// Minimal React replacement: only the hooks ComputerUseConsent.jsx and its
// useBridge import use, with React semantics (Object.is idempotent writes,
// per-commit hook slots, pairwise deps comparison, cleanup before re-run).
function createReactRuntime() {
  const states = [];
  const refs = [];
  const slots = [];
  let stateIndex = 0;
  let refIndex = 0;
  let effectIndex = 0;
  let dirty = false;
  const useState = (initial) => {
    const i = stateIndex++;
    if (states.length <= i) states.push(typeof initial === 'function' ? initial() : initial);
    const setState = (update) => {
      const prev = states[i];
      const next = typeof update === 'function' ? update(prev) : update;
      if (!Object.is(next, prev)) {
        states[i] = next;
        dirty = true;
      }
    };
    return [states[i], setState];
  };
  const useRef = (initial) => {
    const i = refIndex++;
    if (refs.length <= i) refs.push({ current: initial });
    return refs[i];
  };
  const useEffect = (fn, deps) => {
    const i = effectIndex++;
    if (slots.length <= i) slots.push({ deps: undefined, cleanup: undefined, ran: false, pending: null });
    const slot = slots[i];
    const changed = !slot.ran
      || deps === undefined
      || deps.length !== slot.deps.length
      || deps.some((d, k) => !Object.is(d, slot.deps[k]));
    if (changed) slot.pending = { fn, deps };
  };
  // Effects flush after commit; each slot's old cleanup runs before its
  // re-run, mirroring React's ordering guarantees.
  const flushEffects = () => {
    for (const slot of slots) {
      if (!slot.pending) continue;
      const { fn, deps } = slot.pending;
      slot.pending = null;
      if (typeof slot.cleanup === 'function') slot.cleanup();
      const result = fn();
      slot.cleanup = typeof result === 'function' ? result : undefined;
      slot.deps = deps;
      slot.ran = true;
    }
  };
  return {
    hooks: { useState, useRef, useEffect },
    render(component, props) {
      stateIndex = 0;
      refIndex = 0;
      effectIndex = 0;
      let output = component(props);
      for (let guard = 0; dirty && guard < 25; guard += 1) {
        dirty = false;
        stateIndex = 0;
        refIndex = 0;
        effectIndex = 0;
        output = component(props);
      }
      dirty = false;
      flushEffects();
      return output;
    },
    get dirty() { return dirty; },
    reset() {
      states.length = 0;
      refs.length = 0;
      slots.length = 0;
      dirty = false;
    },
  };
}

const REACT_STUB_SOURCE = [
  'const rt = () => globalThis.__pinvouConsentReact;',
  'export const useState = (...args) => rt().hooks.useState(...args);',
  'export const useRef = (...args) => rt().hooks.useRef(...args);',
  'export const useEffect = (...args) => rt().hooks.useEffect(...args);',
  '',
].join('\n');

// Plain-object JSX runtime: elements are walked directly by the assertions,
// no renderer needed. Key semantics mirror the real runtime (explicit third
// argument wins over config.key).
const JSX_STUB_SOURCE = [
  "const RE = Symbol.for('react.element');",
  "export const Fragment = Symbol.for('react.fragment');",
  'function makeKey(maybeKey, config) {',
  '  if (maybeKey !== undefined) return String(maybeKey);',
  '  return config.key !== undefined ? String(config.key) : null;',
  '}',
  'function makeElement(type, config, maybeKey) {',
  '  const { children, ...rest } = config;',
  '  return { $$typeof: RE, type, key: makeKey(maybeKey, config), ref: null, props: { ...rest, children } };',
  '}',
  'export const jsx = makeElement;',
  'export const jsxs = makeElement;',
  'export const jsxDEV = makeElement;',
  '',
].join('\n');

const hadWindow = Object.prototype.hasOwnProperty.call(globalThis, 'window');
const hadDocument = Object.prototype.hasOwnProperty.call(globalThis, 'document');
// useBridge.js reads window.TauriBridge at module load: the dialog's buttons
// call bridge.computerUse.* through it, so the mock is how tests fire actions.
const bridgeMock = { available: true, computerUse: {} };
globalThis.window = globalThis.window || { TauriBridge: bridgeMock };
globalThis.document = globalThis.document || { addEventListener() {}, removeEventListener() {} };

// The stub modules must be real files: Vite externalizes bare 'react' before
// user resolveId hooks run, but resolve.alias rewrites it into app code that
// the SSR pipeline inlines.
const stubDir = mkdtempSync(path.join(tmpdir(), 'pinvou-consent-stub-'));
const reactStubFile = path.join(stubDir, 'react-stub.mjs');
const jsxStubFile = path.join(stubDir, 'jsx-stub.mjs');
writeFileSync(reactStubFile, REACT_STUB_SOURCE);
writeFileSync(jsxStubFile, JSX_STUB_SOURCE);

const { createServer } = await import('vite');
const vite = await createServer({
  configFile: false,
  root: path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..'),
  logLevel: 'error',
  server: { middlewareMode: true, watch: null },
  optimizeDeps: { noDiscovery: true },
  resolve: {
    alias: [
      { find: /^react$/, replacement: reactStubFile },
      { find: /^react\/jsx-dev-runtime$/, replacement: jsxStubFile },
      { find: /^react\/jsx-runtime$/, replacement: jsxStubFile },
    ],
  },
});

const REACT_ELEMENT = Symbol.for('react.element');

function walkElements(node, visit) {
  if (node == null || typeof node !== 'object') return;
  if (Array.isArray(node)) {
    for (const child of node) walkElements(child, visit);
    return;
  }
  if (node.$$typeof !== REACT_ELEMENT) return;
  visit(node);
  walkElements(node.props && node.props.children, visit);
}

function findByTestId(root, testId) {
  const hits = [];
  walkElements(root, (el) => {
    if (el.props && el.props['data-testid'] === testId) hits.push(el);
  });
  return hits[0] || null;
}

function allText(root) {
  const parts = [];
  walkElements(root, (el) => {
    const collect = (node) => {
      if (node == null || typeof node === 'boolean') return;
      if (typeof node === 'string' || typeof node === 'number') {
        parts.push(String(node));
        return;
      }
      if (Array.isArray(node)) node.forEach(collect);
    };
    collect(el.props && el.props.children);
  });
  return parts.join('\n');
}

const dialogCopy = {
  bannerTitle: 'Agent is controlling your computer',
  bannerNote: 'note',
  bannerStop: 'Stop control',
  grantTitle: 'Grant',
  grantDesc: 'desc',
  grantAllow: 'Allow',
  grantDeny: 'Deny',
  confirmTitle: 'Confirm this action',
  confirmActionLabel: 'Action',
  confirmElementLabel: 'Target element',
  confirmOnce: 'Confirm once',
  confirmDeny: 'Deny',
  showFullText: 'Show full text',
  fullTextWarning: 'The agent will type exactly this text.',
  actionFailed: (error) => `Action failed: ${error}`,
};

function confirmSlice(confirmId, typePreviewFull) {
  const request = { sessionId: 's1', action: 'type', element: 'Reply box', confirmId };
  if (typePreviewFull !== undefined) request.typePreviewFull = typePreviewFull;
  return { enabled: true, granted: true, stopped: false, confirmRequest: request };
}

try {
  const { ComputerUseDialogs } = await vite.ssrLoadModule('/src/features/computer-use/ComputerUseConsent.jsx');
  const runtime = createReactRuntime();
  globalThis.__pinvouConsentReact = runtime;
  let tree = null;
  const render = (slice) => {
    tree = runtime.render(ComputerUseDialogs, { slice, copy: dialogCopy });
    return tree;
  };
  // Drives the promise chain inside useConsentAction's run() to completion
  // (setPendingAction → action → catch/finally), then re-renders until stable.
  const settle = async (slice) => {
    // setImmediate drains the microtask chain inside useConsentAction's run().
    await new Promise((resolve) => {
      setImmediate(resolve);
    });
    render(slice);
    for (let guard = 0; runtime.dirty && guard < 25; guard += 1) render(slice);
    return tree;
  };

  // ── UI-1. Short typePreviewFull renders inline, approval stays enabled ──
  // Review finding: short texts (e.g. six characters) hid behind the "show
  // full text" click-wall, so the user approved without ever seeing the
  // exact typed text.
  runtime.reset();
  tree = render(confirmSlice('cu-1', 'Hello 三'));
  const inlinePre = findByTestId(tree, 'computer-use-confirm-full-text');
  assert.ok(inlinePre, 'a short preview must render the full text inline');
  assert.ok(allText(inlinePre).includes('Hello 三'), 'the inline text must be the full preview');
  assert.equal(findByTestId(tree, 'computer-use-confirm-show-full'), null,
    'a short preview must not require the reveal step');
  assert.equal(findByTestId(tree, 'computer-use-confirm-once').props.disabled, false,
    '"Confirm once" must be enabled while the short text is visible');
  assert.ok(allText(tree).includes(dialogCopy.fullTextWarning),
    'the warning line stays for the inline case');
  // 200 chars (the threshold) is still inline; 201 keeps the reveal gate.
  tree = render(confirmSlice('cu-2', 'a'.repeat(200)));
  assert.ok(findByTestId(tree, 'computer-use-confirm-full-text'), 'the 200-char boundary renders inline');
  assert.equal(findByTestId(tree, 'computer-use-confirm-show-full'), null);
  tree = render(confirmSlice('cu-3', 'a'.repeat(201)));
  assert.ok(findByTestId(tree, 'computer-use-confirm-show-full'),
    'longer texts keep the reveal button');
  assert.equal(findByTestId(tree, 'computer-use-confirm-full-text'), null,
    'longer texts must not render before the reveal');
  assert.equal(findByTestId(tree, 'computer-use-confirm-once').props.disabled, true,
    '"Confirm once" stays locked until the long text was revealed');
  assert.ok(allText(tree).includes(dialogCopy.fullTextWarning),
    'the warning line stays for the reveal case');
  findByTestId(tree, 'computer-use-confirm-show-full').props.onClick();
  tree = render(confirmSlice('cu-3', 'a'.repeat(201)));
  assert.ok(findByTestId(tree, 'computer-use-confirm-full-text'), 'the reveal unlocks the long text');
  assert.equal(findByTestId(tree, 'computer-use-confirm-once').props.disabled, false,
    'revealing the long text unlocks "Confirm once"');

  // ── UI-2. A failed action's error must not leak into the next dialog ──
  // Review finding: actionError survived after a dialog closed and was then
  // rendered inside the next, unrelated dialog as a message about a
  // different confirm_id.
  runtime.reset();
  bridgeMock.computerUse.deny = () => Promise.reject(new Error('backend exploded'));
  const failingSlice = confirmSlice('cu-1');
  tree = render(failingSlice);
  assert.equal(findByTestId(tree, 'computer-use-confirm-once').props.disabled, false,
    'a request without typePreviewFull must not gate "Confirm once" behind a reveal');
  findByTestId(tree, 'computer-use-confirm-deny').props.onClick();
  // Same slice object: the failing action happens while THIS dialog is up, so
  // the error belongs to it (a new slice identity would legitimately reset it).
  tree = await settle(failingSlice);
  assert.ok(allText(tree).includes('Action failed: backend exploded'),
    'the failure must surface inside the dialog it happened in');
  assert.ok(findByTestId(tree, 'computer-use-confirm-deny'),
    'the dialog stays up after the failed action');
  // The request goes away, then an unrelated new request re-opens the dialog.
  tree = await settle({ enabled: true, granted: true, stopped: false });
  assert.equal(tree, null, 'without a pending request no dialog renders');
  tree = await settle(confirmSlice('cu-2', 'hi'));
  assert.ok(findByTestId(tree, 'computer-use-confirm-deny'), 'the new dialog is up');
  assert.equal(allText(tree).includes('Action failed: backend exploded'), false,
    'the stale error about cu-1 must not leak into the cu-2 dialog');
} finally {
  await vite.close();
  rmSync(stubDir, { recursive: true, force: true });
  if (!hadWindow) delete globalThis.window;
  if (!hadDocument) delete globalThis.document;
  delete globalThis.__pinvouConsentReact;
}

console.log('computer use consent dialog UI tests passed');
