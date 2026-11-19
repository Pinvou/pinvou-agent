#!/usr/bin/env node
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const dir = mkdtempSync(path.join(tmpdir(), 'pinvou3-interval-when-'));
// Same harness shape as tests/use_throttled_value.test.mjs: the hook source is
// copied byte-for-byte and imported for real; its only dependency 'react'
// resolves through a temp node_modules stub (node --test has no React renderer
// and the repo has no jsdom).
const hookTmp = path.join(dir, 'useIntervalWhen.mjs');
copyFileSync(path.join(here, '..', 'src', 'hooks', 'useIntervalWhen.js'), hookTmp);
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
    'export const useRef = (...args) => rt().hooks.useRef(...args);',
    'export const useEffect = (...args) => rt().hooks.useEffect(...args);',
    '',
  ].join('\n'),
);

// ── Minimal React hooks runtime stub (semantics must match React) ──
// - useRef: same instance across renders;
// - useEffect: deps compared pairwise with Object.is; deps === undefined
//   re-runs every commit (the latest-ref sync effect relies on this); within
//   one commit effects flush in declaration order, old cleanup before re-run.
function createRuntime() {
  const refs = [];
  const slots = [];
  let refIndex = 0;
  let effectIndex = 0;
  let queue = [];

  const useRef = (initial) => {
    const i = refIndex++;
    if (refs.length <= i) refs.push({ current: initial });
    return refs[i];
  };

  const useEffect = (fn, deps) => {
    const i = effectIndex++;
    if (slots.length <= i) slots.push({ deps: undefined, cleanup: undefined, ran: false });
    const slot = slots[i];
    const changed = !slot.ran
      || deps === undefined
      || deps.length !== slot.deps.length
      || deps.some((d, k) => !Object.is(d, slot.deps[k]));
    if (changed) queue.push({ fn, slot, deps });
  };

  return {
    hooks: { useRef, useEffect },
    render(hook, callback, delayMs, enabled) {
      refIndex = 0;
      effectIndex = 0;
      queue = [];
      const ret = hook(callback, delayMs, enabled);
      for (const { fn, slot, deps } of queue) {
        if (typeof slot.cleanup === 'function') slot.cleanup();
        const result = fn();
        slot.cleanup = typeof result === 'function' ? result : undefined;
        slot.deps = deps;
        slot.ran = true;
      }
      return ret;
    },
    unmount() {
      for (const slot of slots) {
        if (typeof slot.cleanup === 'function') slot.cleanup();
      }
    },
  };
}

function createHarness(hook) {
  const fakeWindow = {
    intervals: new Map(),
    nextId: 1,
    setInterval(cb) {
      const id = this.nextId++;
      this.intervals.set(id, cb);
      return id;
    },
    clearInterval(id) {
      this.intervals.delete(id);
    },
  };
  const runtime = createRuntime();
  globalThis.__pinvouReactHooks = runtime;
  globalThis.window = fakeWindow;

  return {
    fakeWindow,
    render(callback, delayMs = 1000, enabled = true) {
      // The hook and the react stub read `window` / the hooks runtime at call
      // time; re-point both at THIS harness (multiple harnesses coexist in
      // one file).
      globalThis.window = fakeWindow;
      globalThis.__pinvouReactHooks = runtime;
      return runtime.render(hook, callback, delayMs, enabled);
    },
    tick() {
      globalThis.window = fakeWindow;
      for (const cb of [...fakeWindow.intervals.values()]) cb();
    },
    unmount() {
      globalThis.window = fakeWindow;
      globalThis.__pinvouReactHooks = runtime;
      runtime.unmount();
    },
  };
}

try {
  const { useIntervalWhen } = await import(pathToFileURL(hookTmp).href);

  // T1: enabled mount schedules exactly one interval; ticks fire the callback.
  let calls = 0;
  const h1 = createHarness(useIntervalWhen);
  h1.render(() => { calls += 1; }, 1000, true);
  assert.equal(h1.fakeWindow.intervals.size, 1, 'an enabled mount schedules exactly one interval');
  h1.tick();
  h1.tick();
  assert.equal(calls, 2, 'each tick fires the callback once');

  // T2: disabled mount schedules nothing.
  const h2 = createHarness(useIntervalWhen);
  h2.render(() => {}, 1000, false);
  assert.equal(h2.fakeWindow.intervals.size, 0, 'a disabled mount must not schedule a timer');

  // T3: disabling clears the running interval.
  h1.render(() => {}, 1000, false);
  assert.equal(h1.fakeWindow.intervals.size, 0, 'disabling must clear the running interval');

  // T4: re-renders with a fresh closure must NOT reset the timer (the interval
  // keeps its id), yet ticks must see the latest closure — this is the
  // load-bearing latest-ref contract: poll closures reading fresh state must
  // not depend on timer restarts, and polling cadence survives re-renders.
  const h4 = createHarness(useIntervalWhen);
  let seen = 'old';
  h4.render(() => { seen = 'old'; }, 1000, true);
  const idBefore = [...h4.fakeWindow.intervals.keys()][0];
  h4.render(() => { seen = 'new'; }, 1000, true);
  assert.deepEqual(
    [...h4.fakeWindow.intervals.keys()],
    [idBefore],
    'a re-render with a fresh closure must keep the same interval (no reset)',
  );
  h4.tick();
  assert.equal(seen, 'new', 'the tick must invoke the latest closure, not the captured one');

  // T5: a null/undefined delay disables the timer like enabled:false; a delay
  // change replaces the timer (old one cleared, a fresh one created).
  const h5 = createHarness(useIntervalWhen);
  h5.render(() => {}, null, true);
  assert.equal(h5.fakeWindow.intervals.size, 0, 'null delay must not schedule a timer');
  h5.render(() => {}, 500, true);
  assert.equal(h5.fakeWindow.intervals.size, 1, 'a real delay schedules the timer');
  const idAt500 = [...h5.fakeWindow.intervals.keys()][0];
  h5.render(() => {}, 2000, true);
  const idsAfterDelayChange = [...h5.fakeWindow.intervals.keys()];
  assert.equal(idsAfterDelayChange.length, 1, 'a delay change keeps exactly one timer');
  assert.notEqual(
    idsAfterDelayChange[0],
    idAt500,
    'a delay change must clear the old timer and create a new one',
  );

  // T6: unmount clears the interval.
  const h6 = createHarness(useIntervalWhen);
  h6.render(() => {}, 1000, true);
  assert.equal(h6.fakeWindow.intervals.size, 1);
  h6.unmount();
  assert.equal(h6.fakeWindow.intervals.size, 0, 'unmount cleanup must clear the interval');

  console.log('useIntervalWhen regression tests passed');
} finally {
  rmSync(dir, { recursive: true, force: true });
}
