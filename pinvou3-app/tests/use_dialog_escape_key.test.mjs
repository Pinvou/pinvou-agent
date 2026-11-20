#!/usr/bin/env node
import assert from 'node:assert/strict';
import test from 'node:test';
import { copyFileSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const dir = mkdtempSync(path.join(tmpdir(), 'pinvou3-dialog-escape-'));
// Same harness shape as tests/use_interval_when.test.mjs: the hook source is
// copied byte-for-byte and imported for real; its dependencies ('react' and
// ../shared/ime-guard.mjs) resolve through a mirrored temp tree plus a temp
// node_modules stub (node --test has no React renderer and the repo has no
// jsdom).
mkdirSync(path.join(dir, 'src', 'hooks'), { recursive: true });
mkdirSync(path.join(dir, 'src', 'shared'), { recursive: true });
copyFileSync(path.join(here, '..', 'src', 'hooks', 'useDialogEscapeKey.js'), path.join(dir, 'src', 'hooks', 'useDialogEscapeKey.js'));
copyFileSync(path.join(here, '..', 'src', 'shared', 'ime-guard.mjs'), path.join(dir, 'src', 'shared', 'ime-guard.mjs'));
const reactDir = path.join(dir, 'node_modules', 'react');
mkdirSync(reactDir, { recursive: true });
writeFileSync(
  path.join(reactDir, 'package.json'),
  JSON.stringify({ name: 'react', type: 'module', main: 'index.mjs', exports: { default: './index.mjs' } }),
);
writeFileSync(
  path.join(reactDir, 'index.mjs'),
  [
    'const rt = () => globalThis.__pinvouReactHooks;',
    'export const useEffect = (...args) => rt().hooks.useEffect(...args);',
    '',
  ].join('\n'),
);

// ── Minimal React effect runtime stub (semantics must match React) ──
// Mount runs the effect; a rerender re-runs it only when a dep changed
// (Object.is, pairwise) with the old cleanup running first; unmount runs the
// cleanup. useDialogEscapeKey declares exactly one effect with [busy, onCancel].
function createHost() {
  const host = { cleanup: null, lastDeps: undefined, effectRuns: 0 };
  globalThis.__pinvouReactHooks = {
    hooks: {
      useEffect(fn, deps) {
        const changed =
          host.lastDeps === undefined ||
          deps === undefined ||
          deps.length !== host.lastDeps.length ||
          deps.some((d, i) => !Object.is(d, host.lastDeps[i]));
        if (!changed) return;
        if (host.cleanup) {
          host.cleanup();
          host.cleanup = null;
        }
        host.lastDeps = deps;
        host.cleanup = fn() || null;
        host.effectRuns += 1;
      },
    },
  };
  return host;
}

// ── Fake window: records keydown listeners so tests can dispatch and count ──
const registered = [];
globalThis.window = {
  addEventListener(type, handler) {
    registered.push({ type, handler });
  },
  removeEventListener(type, handler) {
    const index = registered.findIndex(entry => entry.type === type && entry.handler === handler);
    assert.notStrictEqual(index, -1, 'cleanup must remove the exact listener it registered');
    registered.splice(index, 1);
  },
};

const { useDialogEscapeKey } = await import(
  pathToFileURL(path.join(dir, 'src', 'hooks', 'useDialogEscapeKey.js'))
);

function makeEvent(overrides = {}) {
  return {
    key: 'Escape',
    isComposing: false,
    keyCode: 0,
    prevented: false,
    preventDefault() {
      this.prevented = true;
    },
    ...overrides,
  };
}

function mount(busy, onCancel) {
  registered.length = 0; // fresh fake window per test
  const host = createHost();
  useDialogEscapeKey(busy, onCancel);
  assert.strictEqual(registered.length, 1, 'mount must register exactly one keydown listener');
  return host;
}

test('plain Escape cancels through the registered window listener', () => {
  let cancelled = 0;
  mount(false, () => {
    cancelled += 1;
  });
  const event = makeEvent();
  registered[0].handler(event);
  assert.strictEqual(cancelled, 1);
  assert.strictEqual(event.prevented, true, 'a handled Escape must preventDefault');
});

test('Escape during IME composition is ignored (isComposing)', () => {
  let cancelled = 0;
  mount(false, () => {
    cancelled += 1;
  });
  registered[0].handler(makeEvent({ isComposing: true }));
  assert.strictEqual(cancelled, 0, 'the candidate-cancelling Escape must not close the dialog');
});

test('Escape with keyCode 229 is ignored (WKWebView pre-compositionend)', () => {
  let cancelled = 0;
  mount(false, () => {
    cancelled += 1;
  });
  registered[0].handler(makeEvent({ keyCode: 229 }));
  assert.strictEqual(cancelled, 0, 'the WebKit-delayed composition keydown must not close the dialog');
});

test('busy dialogs decline Escape entirely', () => {
  let cancelled = 0;
  mount(true, () => {
    cancelled += 1;
  });
  registered[0].handler(makeEvent());
  assert.strictEqual(cancelled, 0, 'an in-flight confirmation must not be dismissable from the keyboard');
});

test('non-Escape keys are ignored', () => {
  let cancelled = 0;
  mount(false, () => {
    cancelled += 1;
  });
  registered[0].handler(makeEvent({ key: 'Enter' }));
  assert.strictEqual(cancelled, 0);
});

test('identical deps keep the same listener (no re-registration churn)', () => {
  const onCancel = () => {};
  const host = mount(false, onCancel);
  const handler = registered[0].handler;
  useDialogEscapeKey(false, onCancel);
  assert.strictEqual(host.effectRuns, 1, 'no dep change must not re-run the effect');
  assert.strictEqual(registered.length, 1);
  assert.strictEqual(registered[0].handler, handler);
});

test('a fresh onCancel swaps the listener for the new closure', () => {
  let firstCalls = 0;
  let secondCalls = 0;
  mount(false, () => {
    firstCalls += 1;
  });
  const staleHandler = registered[0].handler;
  useDialogEscapeKey(false, () => {
    secondCalls += 1;
  });
  assert.strictEqual(registered.length, 1, 'the stale listener must be removed, not accumulated');
  registered[0].handler(makeEvent());
  assert.strictEqual(firstCalls, 0);
  assert.strictEqual(secondCalls, 1);
  // dispatching the removed handler directly must no longer be wired anywhere
  staleHandler(makeEvent());
  assert.strictEqual(firstCalls, 1, 'the stale closure still exists but is no longer registered');
});

test('unmount removes the listener from the window', () => {
  const host = mount(false, () => {});
  host.cleanup();
  assert.strictEqual(registered.length, 0, 'unmount must leave no keydown listener behind');
});
