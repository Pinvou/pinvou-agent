#!/usr/bin/env node
// Ledger fallback poll hook (useSubagentLedgerPoll.js): the loop must never
// fully stop, so a child that appears in the ledger after the last read is
// still discovered even when its real-time event was lost (regression for the
// review finding "initially empty → later ledger child with no real-time
// event": the loop used to stop on the first empty read and never wake up).
import assert from 'node:assert/strict';
import test, { after } from 'node:test';
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const dir = mkdtempSync(path.join(tmpdir(), 'pinvou3-ledger-poll-'));
// Shared by every test below: the hook copy and the react stub must outlive
// all imports (ESM caches by resolved path), so cleanup is file-level, not
// per-test.
after(() => rmSync(dir, { recursive: true, force: true }));
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
    // The two cadences are the documented 3s/15s contract; pin the numbers
    // themselves so a constant drift cannot satisfy the delay assertions below.
    assert.equal(LEDGER_POLL_ACTIVE_MS, 3000, 'the active cadence must stay 3s');
    assert.equal(LEDGER_POLL_IDLE_MS, 15000, 'the idle heartbeat must stay 15s');

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

    // A transient failure (null instead of an array) must not stop the loop,
    // and the guard must drop the payload instead of forwarding it: a null
    // handed to onSummaries would throw inside the merge and freeze the
    // overlay silently.
    const lastGood = received;
    ledger = null;
    timers.fireAll();
    await flushQueue();
    assert.deepEqual(timers.delays(), [LEDGER_POLL_ACTIVE_MS], 'a null read keeps the loop armed');
    assert.equal(received, lastGood, 'a null read must not reach onSummaries');
  } finally {
    timers.restore();
    delete globalThis.__pinvouReactHooks;
  }
});

test('kickRef triggers an immediate read and is in-flight safe', async () => {
  const timers = installFakeTimers();
  const runtime = createRuntime();
  globalThis.__pinvouReactHooks = runtime;
  try {
    const { useSubagentLedgerPoll } = await import(pathToFileURL(hookTmp).href);

    let reads = 0;
    const kickRef = { current: null };
    const readLedger = () => { reads += 1; return Promise.resolve([]); };
    const render = () => {
      runtime.reset();
      useSubagentLedgerPoll({
        enabled: true, sessionId: 's1', hasActive: false, readLedger, onSummaries: () => {}, kickRef,
      });
    };

    render();
    assert.equal(typeof kickRef.current, 'function', 'the hook publishes the kick on mount');
    timers.fireAll();
    await flushQueue();
    assert.equal(reads, 1, 'the initial immediate read ran');
    assert.deepEqual(timers.delays(), [15000], 'idle heartbeat armed');

    // Kick: cancels the pending tick and reads immediately (the read itself is
    // a direct async call, not a new timer).
    kickRef.current();
    assert.deepEqual(timers.delays(), [], 'the kick cancels the pending heartbeat tick');
    await flushQueue();
    assert.equal(reads, 2, 'the kick read ran');
    assert.deepEqual(timers.delays(), [15000], 'the loop re-arms after the kick read');

    // Kick while a read is in flight queues exactly one re-read: the in-flight
    // read was issued before the kick, so its snapshot may predate the change
    // the kick is about — but no second loop may be started.
    let resolveRead;
    let inFlightReads = 0;
    const blockingRead = () => {
      inFlightReads += 1;
      return new Promise(resolve => { resolveRead = resolve; });
    };
    runtime.reset();
    useSubagentLedgerPoll({
      enabled: true, sessionId: 's1', hasActive: false,
      readLedger: blockingRead, onSummaries: () => {}, kickRef,
    });
    timers.fireAll();
    await flushQueue();
    assert.equal(inFlightReads, 1, 'the new generation started one read');
    kickRef.current();
    await flushQueue();
    assert.equal(inFlightReads, 1, 'a kick during an in-flight read starts no second read yet');
    resolveRead([]);
    await flushQueue();
    assert.equal(inFlightReads, 2, 'the queued kick re-reads right after the in-flight read settles');
    resolveRead([]);
    await flushQueue();
    assert.equal(inFlightReads, 2, 'the queued kick is consumed exactly once');
    assert.deepEqual(timers.delays(), [15000], 'the loop re-arms once after the kick chain settles');

    // Unmount clears the kick.
    runtime.reset();
    useSubagentLedgerPoll({
      enabled: false, sessionId: null, hasActive: false, readLedger, onSummaries: () => {}, kickRef,
    });
    assert.equal(kickRef.current, null, 'unmount clears the published kick');
  } finally {
    timers.restore();
    delete globalThis.__pinvouReactHooks;
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

test('a session that never succeeds gives up after repeated cold failures', async () => {
  const timers = installFakeTimers();
  const runtime = createRuntime();
  globalThis.__pinvouReactHooks = runtime;
  try {
    const { useSubagentLedgerPoll, LEDGER_POLL_MAX_COLD_FAILURES } =
      await import(pathToFileURL(hookTmp).href);
    assert.equal(
      LEDGER_POLL_MAX_COLD_FAILURES,
      5,
      'the cold-failure budget is part of the documented give-up contract',
    );

    // Every read rejects — the shape of a session without a ledger data source
    // (the multi-agent switch is off), a permanent error. Past the failure
    // budget the loop must disarm entirely instead of burning an IPC round
    // trip per heartbeat forever.
    const kickRef = { current: null };
    let reads = 0;
    const render = () => {
      runtime.reset();
      useSubagentLedgerPoll({
        enabled: true, sessionId: 's1', hasActive: true, kickRef,
        readLedger: () => {
          reads += 1;
          return Promise.reject(new Error('unsupported session'));
        },
        onSummaries: () => {},
      });
    };
    render();
    for (let attempt = 1; attempt <= LEDGER_POLL_MAX_COLD_FAILURES; attempt++) {
      assert.equal(timers.pending.size, 1, `attempt ${attempt}: the loop is still armed before the budget is exhausted`);
      timers.fireAll();
      await flushQueue();
    }
    assert.equal(reads, LEDGER_POLL_MAX_COLD_FAILURES, 'each armed tick performed one read');
    assert.equal(timers.pending.size, 0, 'past the cold-failure budget no timer is re-armed');
    assert.equal(kickRef.current, null, 'the kick is disarmed for a session with no ledger source');

    // The budget counts consecutive cold failures: one success resets it and
    // afterwards failures are transient forever (the loop never gives up on a
    // session that has a working ledger source).
    let fail = false;
    let successes = 0;
    const renderOk = () => {
      runtime.reset();
      useSubagentLedgerPoll({
        enabled: true, sessionId: 's2', hasActive: false,
        readLedger: () => {
          reads += 1;
          return fail ? Promise.resolve(null) : Promise.resolve([]);
        },
        onSummaries: () => { successes += 1; },
      });
    };
    renderOk();
    timers.fireAll();
    await flushQueue();
    assert.equal(successes, 1, 'the first read succeeds');
    fail = true;
    for (let attempt = 0; attempt <= LEDGER_POLL_MAX_COLD_FAILURES + 1; attempt++) {
      timers.fireAll();
      await flushQueue();
      assert.equal(timers.pending.size, 1, 'after one success a failure streak never stops the loop');
    }
    assert.equal(successes, 1, 'failed reads deliver nothing once the payload turns null');
  } finally {
    timers.restore();
    delete globalThis.__pinvouReactHooks;
  }
});
