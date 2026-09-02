#!/usr/bin/env node
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const dir = mkdtempSync(path.join(tmpdir(), 'pinvou3-throttle-'));
// The hook source is copied byte-for-byte and imported for real; its only
// dependency 'react' resolves through a temp node_modules stub (node --test has
// no React renderer and the repo has no jsdom), so the real hook implementation
// can be driven without modifying the source under test.
const hookTmp = path.join(dir, 'useThrottledValue.mjs');
copyFileSync(path.join(here, '..', 'src', 'features', 'conversation', 'useThrottledValue.js'), hookTmp);
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
    'export const useState = (...args) => rt().hooks.useState(...args);',
    'export const useRef = (...args) => rt().hooks.useRef(...args);',
    'export const useEffect = (...args) => rt().hooks.useEffect(...args);',
    '',
  ].join('\n'),
);

// ── Minimal React hooks runtime stub ──
// Only the subset useThrottledValue uses is implemented, but the semantics must
// match React:
// - useState: Object.is idempotent writes; a write sets dirty and the driver
//   re-renders;
// - useRef: returns the same instance across renders;
// - useEffect: deps compared pairwise with Object.is; within one commit,
//   effects flush in declaration order and the old cleanup runs before a
//   re-run. "Effects flush in declaration order within one commit" is the
//   load-bearing property that keeps this hook correct on the coalesced final
//   commit (text and the streaming flag changing in the same commit): the ref
//   sync effect must run before the convergence effect.
function createRuntime() {
  const states = [];
  const refs = [];
  const slots = [];
  let stateIndex = 0;
  let refIndex = 0;
  let effectIndex = 0;
  let dirty = false;
  let queue = [];

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
    if (slots.length <= i) slots.push({ deps: undefined, cleanup: undefined, ran: false });
    const slot = slots[i];
    const changed = !slot.ran
      || deps === undefined
      || deps.length !== slot.deps.length
      || deps.some((d, k) => !Object.is(d, slot.deps[k]));
    if (changed) queue.push({ fn, slot, deps });
  };

  return {
    hooks: { useState, useRef, useEffect },
    render(hook, value, delayMs, active) {
      stateIndex = 0;
      refIndex = 0;
      effectIndex = 0;
      dirty = false;
      queue = [];
      const ret = hook(value, delayMs, active);
      for (const { fn, slot, deps } of queue) {
        if (typeof slot.cleanup === 'function') slot.cleanup();
        const result = fn();
        slot.cleanup = typeof result === 'function' ? result : undefined;
        slot.deps = deps;
        slot.ran = true;
      }
      return ret;
    },
    get dirty() {
      return dirty;
    },
  };
}

// Driver: after render/state writes, loop re-renders until stable (mirroring
// React's post-effect batched re-renders). tick() manually fires the sampling
// interval callback, equivalent to the delayMs clock expiring.
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
  let current = { value: undefined, delayMs: 200, active: true };
  let lastReturn;

  const settle = () => {
    // The hook's sampler reads the global window; each harness mounts its own fakeWindow.
    globalThis.window = fakeWindow;
    let guard = 0;
    let ret = runtime.render(hook, current.value, current.delayMs, current.active);
    while (runtime.dirty) {
      if (++guard > 25) throw new Error('useThrottledValue did not settle within 25 render passes');
      ret = runtime.render(hook, current.value, current.delayMs, current.active);
    }
    lastReturn = ret;
    return ret;
  };

  return {
    fakeWindow,
    get return() {
      return lastReturn;
    },
    render(value, delayMs = 200, active = true) {
      current = { value, delayMs, active };
      return settle();
    },
    tick() {
      for (const cb of [...fakeWindow.intervals.values()]) cb();
      return settle();
    },
  };
}

try {
  const { useThrottledValue } = await import(pathToFileURL(hookTmp).href);

  // T1: the 200ms budget while active and streaming — before the fix, the effect
  // flushed pass-through on every commit and the budget had no effect; after the
  // fix, the returned value trails on the interval cadence.
  const streaming = createHarness(useThrottledValue);
  assert.equal(streaming.render('v1'), 'v1', 'first commit adopts the initial value');
  assert.equal(
    streaming.render('v2'),
    'v1',
    'a new delta must not flush immediately before the interval expires (time budget in effect)',
  );
  assert.equal(streaming.tick(), 'v2', 'the interval tick publishes the latest sampled value');
  streaming.render('v3');
  streaming.render('v4');
  assert.equal(streaming.return, 'v2', 'the returned value keeps trailing through a burst');
  assert.equal(
    streaming.tick(),
    'v4',
    'the ref re-syncs on every commit, so a burst cannot starve trailing updates',
  );

  // T2: a coalesced final commit (final text and the streaming flag arriving in
  // the same commit) must render the full text verbatim — the pass-through is
  // structurally guaranteed by the flag flip in that same commit and does not
  // depend on any tick. This was the blocker fixed here: before the fix, this
  // scenario rendered the truncated old sample forever.
  const finalText = 'The full final answer, with Markdown.';
  assert.equal(
    streaming.render(finalText, 200, false),
    finalText,
    'the final commit must render the full text immediately, without waiting for a tick',
  );
  assert.equal(streaming.fakeWindow.intervals.size, 0, 'ending the stream must tear down the sampling interval');
  assert.equal(
    streaming.render(finalText, 200, false),
    finalText,
    'once inactive it has converged to the final text and must never resurrect the pre-final stale sample v4',
  );

  // T3: reactivation — starts from the converged final text (showing the final
  // text during the first <delayMs window is a documented trade-off; if that ever
  // changes to a resync on activation, this assertion must be updated
  // deliberately), and after the first tick it must publish the new stream text.
  assert.equal(
    streaming.render('r1', 200, true),
    finalText,
    'reactivation starts from the converged final text, not the pre-final sample',
  );
  assert.equal(streaming.fakeWindow.intervals.size, 1, 'reactivation recreates the sampling interval');
  assert.equal(streaming.tick(), 'r1', 'the first tick publishes the new stream text');

  // T4: inactive mount — pass-through and no sampler scheduled.
  const idle = createHarness(useThrottledValue);
  assert.equal(idle.render('x', 200, false), 'x');
  assert.equal(idle.fakeWindow.intervals.size, 0, 'an inactive mount must not schedule a sampling interval');
  assert.equal(idle.render('y', 200, false), 'y', 'subsequent commits keep passing through while inactive');

  console.log('useThrottledValue regression tests passed');
} finally {
  rmSync(dir, { recursive: true, force: true });
}
