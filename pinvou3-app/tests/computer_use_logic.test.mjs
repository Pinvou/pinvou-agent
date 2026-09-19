import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  bannerErrorReset,
  computerUseConsentView,
  extractComputerUseScreenshotPath,
  formatComputerUseConfirmAction,
} from '../src/features/computer-use/computer-use-logic.js';

// ── Screenshot path extraction ──────────────────────────────────────────────────
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
  extractComputerUseScreenshotPath('shot saved C:\\Users\\John Smith\\.pinvou3\\sessions\\s1\\attachments\\computer_use\\shot.png'),
  'C:\\Users\\John Smith\\.pinvou3\\sessions\\s1\\attachments\\computer_use\\shot.png',
  'a space INSIDE the path (walk-back extends past inner boundaries) must not lose the card',
);
assert.equal(
  extractComputerUseScreenshotPath('saved /mnt/John Smith/.pinvou3/sessions/s1/attachments/computer_use/shot.png'),
  '/mnt/John Smith/.pinvou3/sessions/s1/attachments/computer_use/shot.png',
  'unix absolute paths with inner spaces must be extracted too',
);
// Relative mentions stay rejected no matter how far the walk-back extends:
// only a head genuinely reaching '/' or '<drive>:/' wins.
assert.equal(
  extractComputerUseScreenshotPath('evil src/attachments/computer_use/shot.png'),
  null,
  'a relative mention after prose must still be rejected',
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
// the last-match rule and produce a doubled-separator path.
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
// slash-doubled paths and non-text blocks must not surface either.
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
assert.equal(
  extractComputerUseScreenshotPath('saved /ws/attachments/computer_use/../../secrets/inner.png'),
  null,
  'a traversal segment inside the attachments dir must reject the whole mention',
);
assert.equal(
  extractComputerUseScreenshotPath('saved C:/ws/attachments/computer_use/../..\\payload.png'),
  null,
  'traversal with backslash separators must be rejected too',
);

// ── Consent UI visibility state machine ─────────────────────────────────────────────────────
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
  'stop() collapses the banner and the dialogs: stop_all already killed the pending confirm, so its approve button must not stay live',
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

// ── Structured confirm description rendering ─────────────────────────────
// Backend contract: the confirm payload carries the action name plus
// structured fields, with the English summary kept for fallback. The
// formatter localizes the structured shape and reports the preview block /
// too-long hint separately.
const zhConfirmCopy = {
  textTooLongToPreview: '文本过长，无法完整预览',
  buttonName: { left: '左键', right: '右键', middle: '中键' },
  confirmClick1: '单击', confirmClick2: '双击', confirmClick3: '三击',
  confirmClick: (button, verb) => `${button}${verb}`,
  confirmClickAt: (button, verb, point) => `在 ${point} ${button}${verb}`,
  confirmTypeCount: (count) => `输入 ${count} 个字符`,
  confirmKeyChord: (chord) => `按下组合键 ${chord}`,
  confirmHoldKey: (chord, ms) => `按住 ${chord} ${ms} 毫秒`,
  confirmKeyChordMasked: (chord, count) => chord ? `按下组合键 ${chord} + ${count} 个隐藏字符` : `按下 ${count} 个隐藏字符`,
  confirmHoldKeyMasked: (chord, count, ms) => chord ? `按住 ${chord} + ${count} 个隐藏字符 ${ms} 毫秒` : `按住 ${count} 个隐藏字符 ${ms} 毫秒`,
  confirmDrag: (from, to) => `从 ${from} 拖拽到 ${to}`,
  confirmMouseMove: (point) => `移动鼠标到 ${point}`,
  confirmMouseDown: (button) => `按下${button}`,
  confirmMouseUp: (button) => `松开${button}`,
};
const structured = (fields) => ({ sessionId: 's1', confirmId: 'c1', summary: 'english fallback', actionName: null, button: null, clickCount: null, point: null, endPoint: null, textLength: null, textPreview: null, textPreviewTruncated: false, chord: null, holdMs: null, ...fields });

assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, structured({ actionName: 'click', button: 'left', clickCount: 1, point: { x: 5, y: 6 } })),
  { description: '在 (5, 6) 左键单击', preview: null, previewTooLong: false },
  'a structured single click at a point renders the localized template',
);
assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, structured({ actionName: 'click', button: 'right', clickCount: 3 })),
  { description: '右键三击', preview: null, previewTooLong: false },
  'a structured triple click without a point renders the bare template',
);
assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, structured({ actionName: 'click', button: 'middle', clickCount: 2, point: { x: 1, y: 2 } })),
  { description: '在 (1, 2) 中键双击', preview: null, previewTooLong: false },
  'button × count combinations compose',
);
assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, structured({ actionName: 'type', textLength: 12 })),
  { description: '输入 12 个字符', preview: null, previewTooLong: false },
  'a type action renders the character count',
);
assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, structured({ actionName: 'type', textLength: 12, textPreviewTruncated: true })),
  { description: '输入 12 个字符', preview: null, previewTooLong: true },
  'a truncated type action with no preview flags the too-long hint',
);
assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, structured({ actionName: 'type', textLength: 3, textPreview: 'abc' })),
  { description: '输入 3 个字符', preview: 'abc', previewTooLong: false },
  'a type action with a preview feeds the inline preview block',
);
assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, structured({ actionName: 'key', chord: 'Control+C' })),
  { description: '按下组合键 Control+C', preview: null, previewTooLong: false },
  'a key chord renders the chord string',
);
assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, structured({ actionName: 'hold_key', chord: 'Shift', holdMs: 500 })),
  { description: '按住 Shift 500 毫秒', preview: null, previewTooLong: false },
  'a hold-key action renders chord and duration',
);
assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, structured({ actionName: 'key', chord: null, chordMaskedChars: 4 })),
  { description: '按下 4 个隐藏字符', preview: null, previewTooLong: false },
  'a masked chord on a secure target renders the count, never the characters',
);
assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, structured({ actionName: 'key', chord: 'Control', chordMaskedChars: 1 })),
  { description: '按下组合键 Control + 1 个隐藏字符', preview: null, previewTooLong: false },
  'a masked mixed chord keeps the named keys and the count',
);
assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, structured({ actionName: 'hold_key', chord: null, chordMaskedChars: 2, holdMs: 500 })),
  { description: '按住 2 个隐藏字符 500 毫秒', preview: null, previewTooLong: false },
  'a masked hold-key renders count and duration',
);
assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, structured({ actionName: 'drag', point: { x: 1, y: 2 }, endPoint: { x: 3, y: 4 } })),
  { description: '从 (1, 2) 拖拽到 (3, 4)', preview: null, previewTooLong: false },
  'a drag renders both endpoints',
);
assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, structured({ actionName: 'scroll', point: { x: 7, y: 8 } })),
  { description: 'english fallback', preview: null, previewTooLong: false },
  'a scroll confirm falls back to the summary (scroll is never screened/blocked)',
);
assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, structured({ actionName: 'mouse_move', point: { x: 9, y: 10 } })),
  { description: '移动鼠标到 (9, 10)', preview: null, previewTooLong: false },
  'a mouse move renders the target point',
);
assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, structured({ actionName: 'mouse_down', button: 'left' })),
  { description: '按下左键', preview: null, previewTooLong: false },
  'a mouse down renders the button',
);
assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, structured({ actionName: 'mouse_up', button: 'right' })),
  { description: '松开右键', preview: null, previewTooLong: false },
  'a mouse up renders the button',
);
// Legacy payload generation: no structured fields — the English summary is
// the only content and must pass through verbatim.
assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, { sessionId: 's1', confirmId: 'c1', summary: 'left click x1 at Some((5, 6))' }),
  { description: 'left click x1 at Some((5, 6))', preview: null, previewTooLong: false },
  'a legacy request falls back to the English summary',
);
// Structured shape with missing required data falls back to the summary too.
assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, structured({ actionName: 'drag', point: { x: 1, y: 2 } })),
  { description: 'english fallback', preview: null, previewTooLong: false },
  'an incomplete structured shape falls back to the English summary',
);
assert.deepEqual(
  formatComputerUseConfirmAction(zhConfirmCopy, structured({ actionName: 'teleport', point: { x: 1, y: 2 } })),
  { description: 'english fallback', preview: null, previewTooLong: false },
  'an unknown action name falls back to the English summary',
);
// A missing/legacy copy (no template keys) must not throw and must fall back.
assert.deepEqual(
  formatComputerUseConfirmAction({}, structured({ actionName: 'click', button: 'left', clickCount: 1 })),
  { description: 'english fallback', preview: null, previewTooLong: false },
  'a copy without template keys falls back to the English summary',
);

console.log('computer use logic tests passed');

// ── Consent dialog UI projection (real component, stubbed React runtime) ──
// node --test has no DOM renderer and the repo has no jsdom, so the dialog
// component is loaded through a bare Vite SSR server whose 'react' imports
// resolve to a minimal hooks runtime (same approach as
// tests/use_throttled_value.test.mjs) and whose JSX runtime produces plain
// element trees the assertions walk directly. This keeps the review-critical
// render behavior (always-inline full-text previews, secure-target omission,
// error leakage) covered without a browser while the real component source is
// exercised unmodified.

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
  confirmOnce: 'Allow this once',
  confirmDeny: 'Deny',
  fullTextWarning: 'The agent will type exactly this text.',
  textTooLongToPreview: 'Text too long to preview in full',
  buttonName: { left: 'Left', right: 'Right', middle: 'Middle' },
  confirmClick1: 'click', confirmClick2: 'double-click', confirmClick3: 'triple-click',
  confirmClick: (button, verb) => `${button} ${verb}`,
  confirmClickAt: (button, verb, point) => `${button} ${verb} at ${point}`,
  confirmTypeCount: (count) => `Type ${count} characters`,
  confirmKeyChord: (chord) => `Press ${chord}`,
  confirmHoldKey: (chord, ms) => `Hold ${chord} for ${ms} ms`,
  confirmDrag: (from, to) => `Drag from ${from} to ${to}`,
  confirmMouseMove: (point) => `Move mouse to ${point}`,
  confirmMouseDown: (button) => `Press and hold the ${button} mouse button`,
  confirmMouseUp: (button) => `Release the ${button} mouse button`,
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

  // ── UI-1. Full typePreviewFull always renders inline, approval stays enabled ──
  // The old reveal gate locked approval behind a "show full text" click; the
  // exact typed text must be visible without any click (mainstream behavior)
  // and approval must never depend on reveal state — the preview scrolls
  // instead of gating.
  runtime.reset();
  tree = render(confirmSlice('cu-1', 'Hello 三'));
  const inlinePre = findByTestId(tree, 'computer-use-confirm-full-text');
  assert.ok(inlinePre, 'a short preview must render the full text inline');
  assert.ok(allText(inlinePre).includes('Hello 三'), 'the inline text must be the full preview');
  assert.equal(findByTestId(tree, 'computer-use-confirm-show-full'), null,
    'no reveal step exists anymore');
  assert.equal(findByTestId(tree, 'computer-use-confirm-once').props.disabled, false,
    '"Allow this once" must be enabled while the short text is visible');
  assert.ok(allText(tree).includes(dialogCopy.fullTextWarning),
    'the warning line stays for the inline case');
  // Texts beyond the old 200-char inline cap render inline too — visible
  // without any click and "Allow this once" enabled (no reveal gate).
  const longText = `${'a'.repeat(200)}b`;
  tree = render(confirmSlice('cu-2', longText));
  const longPre = findByTestId(tree, 'computer-use-confirm-full-text');
  assert.ok(longPre, 'a >200-char preview must render inline without a reveal click');
  assert.ok(allText(longPre).includes(longText), 'the inline element must contain the full long text');
  assert.equal(findByTestId(tree, 'computer-use-confirm-show-full'), null,
    'long texts must not bring back the reveal button');
  assert.equal(findByTestId(tree, 'computer-use-confirm-once').props.disabled, false,
    '"Allow this once" must never be disabled by preview visibility');
  assert.ok(allText(tree).includes(dialogCopy.fullTextWarning),
    'the warning line stays for the long-text case');
  // The backend contract caps the preview at 4096 chars; a max-size preview
  // stays inline as well (the scrollable container keeps the dialog sized).
  tree = render(confirmSlice('cu-3', 'b'.repeat(4096)));
  assert.ok(findByTestId(tree, 'computer-use-confirm-full-text'),
    'a 4096-char preview renders inline in the scrollable container');
  assert.equal(findByTestId(tree, 'computer-use-confirm-once').props.disabled, false,
    '"Allow this once" stays enabled for a max-size preview');

  // ── UI-2. A failed action's error must not leak into the next dialog ──
  // actionError used to survive after a dialog closed and was then rendered
  // inside the next, unrelated dialog as a message about a different
  // confirm_id.
  runtime.reset();
  bridgeMock.computerUse.deny = () => Promise.reject(new Error('backend exploded'));
  const failingSlice = confirmSlice('cu-1');
  tree = render(failingSlice);
  assert.equal(findByTestId(tree, 'computer-use-confirm-once').props.disabled, false,
    'a request without typePreviewFull must not gate "Allow this once" behind a reveal');
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

  // ── UI-3. Secure targets (no typePreviewFull) show no preview at all ──
  // Backend contract: password/secure Type confirmations ship no preview, so
  // neither the typed-text block nor its warning may render.
  runtime.reset();
  tree = render(confirmSlice('cu-secure'));
  assert.equal(findByTestId(tree, 'computer-use-confirm-full-text'), null,
    'a secure target must not render any typed-text preview');
  assert.equal(allText(tree).includes(dialogCopy.fullTextWarning), false,
    'the exact-text warning belongs to the preview block and stays hidden for secure targets');
  assert.equal(findByTestId(tree, 'computer-use-confirm-once').props.disabled, false,
    'a missing preview must not disable "Allow this once"');

  // ── UI-4. Structured payloads render localized; too-long texts hint ──
  // The dialog must not surface the raw English summary when the structured
  // fields are present, and a truncated text without a preview shows the
  // too-long hint bar instead of the preview block.
  runtime.reset();
  const structuredSlice = (confirmId, fields) => ({
    enabled: true, granted: true, stopped: false,
    confirmRequest: {
      sessionId: 's1', confirmId, element: 'Buy now',
      summary: 'left click x2 at Some((5, 6))',
      actionName: null, button: null, clickCount: null, point: null, endPoint: null,
      textLength: null, textPreview: null, textPreviewTruncated: false,
      chord: null, holdMs: null,
      ...fields,
    },
  });
  tree = render(structuredSlice('cu-1', { actionName: 'click', button: 'left', clickCount: 2, point: { x: 5, y: 6 } }));
  assert.ok(allText(tree).includes('Left double-click at (5, 6)'),
    'a structured click renders the localized template, not the English summary');
  assert.equal(allText(tree).includes('left click x2'), false,
    'the English summary must not leak through when the structured fields exist');
  tree = render(structuredSlice('cu-2', { actionName: 'type', textLength: 12, textPreviewTruncated: true }));
  assert.ok(allText(tree).includes('Type 12 characters'),
    'a structured type renders the localized count');
  assert.ok(findByTestId(tree, 'computer-use-confirm-text-too-long'),
    'a truncated text without a preview shows the too-long hint bar');
  assert.equal(findByTestId(tree, 'computer-use-confirm-full-text'), null,
    'no preview block renders when the backend shipped no preview');
  assert.equal(allText(tree).includes(dialogCopy.fullTextWarning), false,
    'the exact-text warning stays hidden without a preview');
  tree = render(structuredSlice('cu-3', { actionName: 'type', textLength: 3, textPreview: 'abc' }));
  const textPre = findByTestId(tree, 'computer-use-confirm-full-text');
  assert.ok(textPre && allText(textPre).includes('abc'),
    'a structured text_preview feeds the inline preview block');
  assert.equal(findByTestId(tree, 'computer-use-confirm-text-too-long'), null,
    'a shipped preview suppresses the too-long hint');
} finally {
  await vite.close();
  rmSync(stubDir, { recursive: true, force: true });
  if (!hadWindow) delete globalThis.window;
  if (!hadDocument) delete globalThis.document;
  delete globalThis.__pinvouConsentReact;
}

// ── Banner epoch gate (round-14: stale-error reset, was untested) ──
{
  // Rising edge of a NEW grant: the previous epoch's error is dropped.
  assert.deepEqual(bannerErrorReset(false, true), { resetError: true, wasShown: true });
  // An error raised DURING the shown phase survives (same epoch).
  assert.deepEqual(bannerErrorReset(true, true), { resetError: false, wasShown: true });
  // Hiding re-arms the gate without touching errors.
  assert.deepEqual(bannerErrorReset(true, false), { resetError: false, wasShown: false });
  assert.deepEqual(bannerErrorReset(false, false), { resetError: false, wasShown: false });
}

console.log('computer use consent dialog UI tests passed');
