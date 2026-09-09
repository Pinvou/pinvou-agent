#!/usr/bin/env node
// Ledger fallback poll hook (useSubagentLedgerPoll.js): the loop must never
// fully stop, so a child that appears in the ledger after the last read is
// still discovered even when its real-time event was lost (regression for the
// review finding "initially empty → later ledger child with no real-time
// event": the loop used to stop on the first empty read and never wake up).
import assert from 'node:assert/strict';
import test from 'node:test';
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const dir = mkdtempSync(path.join(tmpdir(), 'pinvou3-ledger-poll-'));
// The hook source is copied byte-for-byte and imported for real; its only
// dependency 'react' resolves through a temp node_modules stub (node --test
// has no React renderer and the repo has no jsdom).
const hookTmp = path.join(dir, 'useSubagentLedgerPoll.mjs');
copyFileSync(path.join(here, '..', 'src', 'features', 'multiagent', 'useSubagentLedgerPoll.js'), hookTmp);
const reactDir = path.join(dir, 'node_modules', 'react');
mkdirSync(reactDir, { recursive: true });
writeFileSync(
  path.join(reactDir, 'package.json'),
  JSON.stringify({ name: 'react', type: 'module', main: 'index.mjs', exports: { default: './index.mjs' } }),
);
writeFileSync(
  path.join(reactDir, 'index.mjs'),
  'export const useEffect = (...args) => globalThis.__pinvouReactHooks.useEffect(...args);\n',
);

// ── Minimal React effect runtime stub ──
// Deps compared pairwise with Object.is; on change the old cleanup runs before
// the effect re-runs. Sufficient for this hook: it uses a single effect.
function createRuntime() {
  const slots = [];
  let effectIndex = 0;
  return {
    useEffect(fn, deps) {
      const i = effectIndex++;
      if (slots.length <= i) slots.push({ deps: undefined, cleanup: undefined, ran: false });
      const slot = slots[i];
      const changed = !slot.ran
        || deps.length !== slot.deps.length
        || deps.some((d, k) => !Object.is(d, slot.deps[k]));
      if (changed) {
        if (typeof slot.cleanup === 'function') slot.cleanup();
        slot.cleanup = fn() || undefined;
        slot.deps = deps;
        slot.ran = true;
      }
    },
    reset() {
      effectIndex = 0;
    },
  };
}

// Fake clock: captures (delay, callback) pairs; the test fires them manually.
function installFakeTimers() {
  const real = { setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout };
  const pending = new Map();
  let nextId = 1;
  globalThis.setTimeout = (cb, delay) => {
    const id = nextId++;
    pending.set(id, { cb, delay });
    return id;
  };
  globalThis.clearTimeout = id => { pending.delete(id); };
  return {
    pending,
    fireAll() {
      // Firing consumes the timer, matching real setTimeout semantics.
      const fired = [...pending.values()];
      pending.clear();
      for (const { cb } of fired) cb();
    },
    delays() {
      return [...pending.values()].map(t => t.delay).sort((a, b) => a - b);
    },
    restore() {
      globalThis.setTimeout = real.setTimeout;
      globalThis.clearTimeout = real.clearTimeout;
    },
  };
}

const flushQueue = async () => {
  for (let i = 0; i < 5; i++) await new Promise(resolve => { setImmediate(resolve); });
};

test('idle heartbeat discovers a later ledger child with no real-time event', async () => {
  const timers = installFakeTimers();
  const runtime = createRuntime();
  globalThis.__pinvouReactHooks = runtime;
  try {
    const { useSubagentLedgerPoll, LEDGER_POLL_ACTIVE_MS, LEDGER_POLL_IDLE_MS } =
      await import(pathToFileURL(hookTmp).href);

    let ledger = [];
    let received = [];
    let hasActive = false;
    const readLedger = () => Promise.resolve(ledger);
    const onSummaries = summaries => { received = summaries; };
    const render = () => {
      runtime.reset();
      useSubagentLedgerPoll({
        enabled: true, sessionId: 's1', hasActive, readLedger, onSummaries,
      });
    };

    // Generation 1: empty ledger. The first read happens immediately and finds
    // nothing — before the fix the loop stopped here forever.
    render();
    assert.deepEqual(timers.delays(), [0], 'the first read is immediate');
    timers.fireAll();
    await flushQueue();
    assert.deepEqual(received, []);
    assert.deepEqual(
      timers.delays(),
      [LEDGER_POLL_IDLE_MS],
      'with nothing active the loop slows down but must stay armed (idle authoritative heartbeat)',
    );

    // A child appears in the ledger later. No state change, no real-time
    // event — only the idle heartbeat fires. It must be discovered.
    ledger = [{ agent_id: 'agent_9', status: 'running', done: false }];
    timers.fireAll();
    await flushQueue();
    assert.deepEqual(received.map(item => item.agent_id), ['agent_9'], 'the idle poll discovered the late child');

    // The component recomputes hasActive=true from the discovery and renders
    // again: the new generation re-reads immediately and adopts the active
    // cadence.
    hasActive = true;
    render();
    assert.deepEqual(timers.delays(), [0], 'a hasActive flip restarts with an immediate read');
    timers.fireAll();
    await flushQueue();
    assert.deepEqual(timers.delays(), [LEDGER_POLL_ACTIVE_MS], 'active liveness polls at the active cadence');

    // A transient failure (null instead of an array) must not stop the loop.
    ledger = null;
    timers.fireAll();
    await flushQueue();
    assert.deepEqual(timers.delays(), [LEDGER_POLL_ACTIVE_MS], 'a null read keeps the loop armed');
  } finally {
    timers.restore();
    delete globalThis.__pinvouReactHooks;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('in-flight reads are discarded and the timer cleared on a generation switch', async () => {
  const timers = installFakeTimers();
  const runtime = createRuntime();
  globalThis.__pinvouReactHooks = runtime;
  try {
    const { useSubagentLedgerPoll } = await import(pathToFileURL(hookTmp).href);

    let hasActive = true;
    let resolveRead;
    let calls = 0;
    const readLedger = () => new Promise(resolve => { resolveRead = resolve; });
    const onSummaries = () => { calls += 1; };
    const render = () => {
      runtime.reset();
      useSubagentLedgerPoll({
        enabled: true, sessionId: 's1', hasActive, readLedger, onSummaries,
      });
    };

    render();
    timers.fireAll();
    const gen1Resolve = resolveRead;
    assert.ok(gen1Resolve, 'the first read is in flight');

    // Changed dep (hasActive flip): cleanup must stop generation 1 and its
    // pending timer, then generation 2 arms its own immediate read.
    hasActive = false;
    render();
    await flushQueue();
    assert.deepEqual(timers.pending.size, 1, 'only the new generation timer is armed');

    gen1Resolve([{ agent_id: 'late' }]);
    await flushQueue();
    assert.equal(calls, 0, 'a read in flight across a generation switch is discarded');

    timers.fireAll();
    await flushQueue();
    assert.notEqual(resolveRead, gen1Resolve, 'generation 2 started its own read');
    resolveRead([{ agent_id: 'current' }]);
    await flushQueue();
    assert.equal(calls, 1, 'the live generation delivers its read');
  } finally {
    timers.restore();
    delete globalThis.__pinvouReactHooks;
  }
});

test('no polling while disabled or without a session', async () => {
  const timers = installFakeTimers();
  const runtime = createRuntime();
  globalThis.__pinvouReactHooks = runtime;
  try {
    const { useSubagentLedgerPoll } = await import(pathToFileURL(hookTmp).href);
    let reads = 0;
    const render = (enabled, sessionId) => {
      runtime.reset();
      useSubagentLedgerPoll({
        enabled,
        sessionId,
        hasActive: true,
        readLedger: () => { reads += 1; return Promise.resolve([]); },
        onSummaries: () => {},
      });
    };
    render(false, 's1');
    render(true, null);
    assert.equal(timers.pending.size, 0, 'no timer is armed while disabled or session-less');
    render(true, 's1');
    timers.fireAll();
    await flushQueue();
    assert.equal(reads, 1, 'a valid mount reads immediately');
  } finally {
    timers.restore();
    delete globalThis.__pinvouReactHooks;
  }
});
