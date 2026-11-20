#!/usr/bin/env node
import assert from 'node:assert/strict';
import test from 'node:test';
import { copyFileSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const dir = mkdtempSync(path.join(tmpdir(), 'pinvou3-focus-restore-'));
// Same harness shape as tests/use_interval_when.test.mjs: the hook source is
// copied byte-for-byte and imported for real; its only dependency 'react'
// resolves through a temp node_modules stub (node --test has no React renderer
// and the repo has no jsdom).
copyFileSync(path.join(here, '..', 'src', 'hooks', 'useDialogFocusRestore.js'), path.join(dir, 'useDialogFocusRestore.mjs'));
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
// The hook declares one effect with ref deps (stable identities across a
// dialog's lifetime), so mount runs it once and unmount runs its cleanup.
function createHost() {
  const host = { cleanup: null };
  globalThis.__pinvouReactHooks = {
    hooks: {
      useEffect(fn) {
        host.cleanup = fn() || null;
      },
    },
  };
  return host;
}

// ── Fake DOM: the hook talks to document.activeElement, focus(), and
// isConnected, and checks `instanceof HTMLElement` before restoring — a
// minimal stand-in for all four keeps the byte-for-byte source honest. ──
class FakeElement {
  constructor() {
    this.focusCalls = 0;
    this.connected = true;
  }
  focus() {
    this.focusCalls += 1;
  }
  get isConnected() {
    return this.connected;
  }
}
globalThis.HTMLElement = FakeElement;

const { useDialogFocusRestore } = await import(pathToFileURL(path.join(dir, 'useDialogFocusRestore.mjs')));

function mount({ active, dialog, initial, override }) {
  const previous = active;
  globalThis.document = { activeElement: previous };
  const host = createHost();
  useDialogFocusRestore(dialog, initial, override);
  return { host, previous };
}

test('mount takes focus onto the dialog container', () => {
  const previous = new FakeElement();
  const dialog = { current: new FakeElement() };
  const { host } = mount({ active: previous, dialog });
  assert.strictEqual(dialog.current.focusCalls, 1, 'opening the dialog must focus it');
  host.cleanup();
});

test('initialFocusRef wins over the dialog container', () => {
  const dialog = { current: new FakeElement() };
  const input = { current: new FakeElement() };
  mount({ active: new FakeElement(), dialog, initial: input });
  assert.strictEqual(dialog.current.focusCalls, 0);
  assert.strictEqual(input.current.focusCalls, 1, 'an initial-focus target must take the focus first');
});

test('unmount restores the previously focused element', () => {
  const previous = new FakeElement();
  const dialog = { current: new FakeElement() };
  const { host } = mount({ active: previous, dialog });
  assert.strictEqual(previous.focusCalls, 0);
  host.cleanup();
  assert.strictEqual(previous.focusCalls, 1, 'closing the dialog must hand focus back');
});

test('a detached previous element is not restored', () => {
  const previous = new FakeElement();
  previous.connected = false;
  const dialog = { current: new FakeElement() };
  const { host } = mount({ active: previous, dialog });
  host.cleanup();
  assert.strictEqual(previous.focusCalls, 0, 'restoring into a detached node is skipped, not crashed');
});

test('a connected override target wins the restore at close time', () => {
  const previous = new FakeElement();
  const dialog = { current: new FakeElement() };
  const overrideTarget = new FakeElement();
  const override = { current: () => overrideTarget };
  const { host } = mount({ active: previous, dialog, override });
  host.cleanup();
  assert.strictEqual(overrideTarget.focusCalls, 1, 'the close-time override must take the restore');
  assert.strictEqual(previous.focusCalls, 0, 'previous must not double-restore behind an override');
});

test('a disconnected override falls back to the previous element', () => {
  const previous = new FakeElement();
  const dialog = { current: new FakeElement() };
  const detached = new FakeElement();
  detached.connected = false;
  const override = { current: () => detached };
  const { host } = mount({ active: previous, dialog, override });
  host.cleanup();
  assert.strictEqual(previous.focusCalls, 1, 'a missing override target must fall back, not cancel the restore');
});
