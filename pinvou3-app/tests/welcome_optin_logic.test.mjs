#!/usr/bin/env node
// consumeWelcomeOptIn contract (review #455 R9 coverage note):
// - no welcome card → no attempt, no consumption, no backend call;
// - welcome card present → single consumption + a single enable_marketplace_packages (plain scope);
// - enable failure → failed:true + error, and already consumed (no repeated attempts).
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const logicPath = path.join(__dirname, '..', 'src', 'features', 'chat', 'welcome-optin.js');
const code = fs.readFileSync(logicPath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '');

const ctx = {};
vm.createContext(ctx);
vm.runInContext(`${code}\nthis.consumeWelcomeOptIn = consumeWelcomeOptIn;\nthis.resolveSendCapabilityStatus = resolveSendCapabilityStatus;\nthis.runSharedWelcomeOptIn = runSharedWelcomeOptIn;`, ctx, { filename: logicPath });
const { consumeWelcomeOptIn, resolveSendCapabilityStatus, runSharedWelcomeOptIn } = ctx;

// 1. No welcome card: zero actions.
{
  const calls = [];
  const result = await consumeWelcomeOptIn({
    getToolId: () => null,
    consume: () => calls.push('consume'),
    invoke: async (cmd) => { calls.push(cmd); },
  });
  assert.strictEqual(result.attempted, false);
  assert.strictEqual(result.failed, undefined);
  assert.deepStrictEqual(calls, []);
}

// 2. Happy path: one consumption + a single batched enable (plain).
// The backend answers with the explicit outcome shape (round-11 m11);
// an install-default off lifts freely → enabled with empty blocked (B2).
{
  const calls = [];
  const result = await consumeWelcomeOptIn({
    getToolId: () => 'gongwen',
    consume: () => calls.push('consume'),
    invoke: async (cmd, args) => {
      calls.push([cmd, args]);
      return { enabled: true, blocked: [] };
    },
  });
  assert.strictEqual(result.attempted, true);
  assert.strictEqual(result.failed, false);
  // vm cross-realm objects do not compare by reference: assert field by field.
  assert.strictEqual(calls[0], 'consume');
  assert.strictEqual(calls[1][0], 'enable_marketplace_packages');
  assert.deepStrictEqual([...calls[1][1].packageIds], ['gongwen']);
  assert.strictEqual(calls[1][1].scope, 'plain');
  assert.strictEqual(calls.length, 2);
}

// 3. enable failure: failed:true + error, already consumed.
{
  const calls = [];
  const result = await consumeWelcomeOptIn({
    getToolId: () => 'pptx',
    consume: () => calls.push('consume'),
    invoke: async () => { throw new Error('backend locked'); },
  });
  assert.strictEqual(result.attempted, true);
  assert.strictEqual(result.failed, true);
  assert.match(result.error, /backend locked/);
  assert.deepStrictEqual(calls, ['consume']);
}

// 4. Explicitly-disabled pack: backend refuses via outcome.blocked → surfaced, not failed.
{
  const result = await consumeWelcomeOptIn({
    getToolId: () => 'gongwen',
    consume: () => {},
    invoke: async () => ({ enabled: false, blocked: ['gongwen'] }),
  });
  assert.strictEqual(result.attempted, true);
  assert.deepStrictEqual([...result.blocked], ['gongwen']);
  assert.strictEqual(result.failed, undefined);
}

// 4b. Round-13 m3: not_applied non-empty = the id matched nothing in the
// DenyAll expansion — nothing was enabled; fail-visible, not a silent success.
{
  const result = await consumeWelcomeOptIn({
    getToolId: () => 'pptx',
    consume: () => {},
    invoke: async () => ({ enabled: false, blocked: [], not_applied: ['pptx'] }),
  });
  assert.strictEqual(result.attempted, true);
  assert.strictEqual(result.failed, true);
  assert.match(result.error, /not applied/);
}

// 5. resolveSendCapabilityStatus: welcome failure beats scene status; otherwise passthrough.
{
  const got = resolveSendCapabilityStatus({
    welcomeFailed: true,
    welcomeText: 'enable failed',
    sceneStatus: { kind: 'ready', text: 'ok' },
  });
  assert.strictEqual(got.kind, 'error');
  assert.strictEqual(got.text, 'enable failed');
  const ready = resolveSendCapabilityStatus({ welcomeFailed: false, welcomeText: '', sceneStatus: { kind: 'ready', text: 'ok' } });
  assert.strictEqual(ready.kind, 'ready');
  assert.strictEqual(ready.text, 'ok');
  assert.strictEqual(resolveSendCapabilityStatus({ welcomeFailed: false, welcomeText: '', sceneStatus: null }), null);
}

// 6. Round-16 minor 13: a send arriving while the enable invoke is in flight
// shares the SAME attempt — run starts once, both callers get the same
// outcome, and the slot clears after settle (a later send starts fresh).
// The second send passes toolId null: consume() already cleared the ref at
// attempt start, and a null id with an in-flight attempt must still join it.
{
  const slot = { current: null };
  let runs = 0;
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const attemptWith = (toolId) => runSharedWelcomeOptIn(slot, {
    toolId,
    run: () => {
      runs += 1;
      return gate.then(() => ({ attempted: true, failed: false }));
    },
  });
  const first = attemptWith('gongwen');
  await Promise.resolve();
  await Promise.resolve();
  assert.strictEqual(runs, 1, 'the first send starts exactly one attempt');
  const second = attemptWith(null);
  const third = attemptWith(null);
  assert.strictEqual(runs, 1, 'concurrent sends share one in-flight attempt');
  release();
  const [r1, r2, r3] = await Promise.all([first, second, third]);
  assert.strictEqual(r1.attempted, true);
  assert.strictEqual(r2, r1, 'the second send gets the same outcome object');
  assert.strictEqual(r3, r1);
  assert.strictEqual(slot.current, null, 'the slot clears once the attempt settles');
  const after = await attemptWith('gongwen');
  assert.strictEqual(runs, 2, 'a post-settle send starts a fresh attempt');
  assert.strictEqual(after.attempted, true);
}

// 6b. Session switch mid-flight: a non-null tool id that differs from the
// in-flight one starts a fresh attempt instead of joining the stale one.
{
  const slot = { current: null };
  let runs = 0;
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const runFor = (toolId) => runSharedWelcomeOptIn(slot, {
    toolId,
    run: () => {
      runs += 1;
      return gate.then(() => ({ attempted: true, toolId }));
    },
  });
  const first = runFor('gongwen');
  await Promise.resolve();
  await Promise.resolve();
  const second = runFor('visualizer');
  await Promise.resolve();
  await Promise.resolve();
  assert.strictEqual(runs, 2, 'a different pack mid-flight starts its own attempt');
  release();
  const r1 = await first;
  const r2 = await second;
  assert.strictEqual(r1.toolId, 'gongwen');
  assert.strictEqual(r2.toolId, 'visualizer');
  assert.strictEqual(slot.current, null);
}

// 6c. The slot clear is identity-guarded: settling attempt A must not clear
// the slot while attempt B (started later) is still in flight — a third
// B-send joins B instead of re-running it (the unguarded-clear mutation is
// the exact concurrency class round-16 minor 13 fixes).
{
  const slot = { current: null };
  const runs = [];
  const releases = {};
  const gatedRun = (toolId) => new Promise((resolve) => {
    releases[toolId] = () => resolve({ attempted: true, toolId });
    runs.push(toolId);
  });
  const runFor = (toolId) => runSharedWelcomeOptIn(slot, { toolId, run: () => gatedRun(toolId) });
  const a = runFor('gongwen');
  await Promise.resolve();
  await Promise.resolve();
  let releaseB;
  const bGate = new Promise((resolve) => { releaseB = resolve; });
  const bAttempt = runSharedWelcomeOptIn(slot, {
    toolId: 'visualizer',
    run: () => { runs.push('visualizer-b'); return bGate.then(() => ({ attempted: true, toolId: 'visualizer' })); },
  });
  await Promise.resolve();
  await Promise.resolve();
  releases.gongwen();
  await a;
  assert.strictEqual(slot.current && slot.current.toolId, 'visualizer', 'settling A must not clear B\'s slot entry');
  const bJoin = runFor('visualizer');
  await Promise.resolve();
  await Promise.resolve();
  releaseB();
  const [, bOutcome] = await Promise.all([bJoin, bAttempt]);
  assert.strictEqual(bOutcome.toolId, 'visualizer');
  assert.strictEqual(runs.filter((id) => id === 'visualizer-b').length, 1, 'the B-send joined the in-flight attempt instead of re-running');
}

console.log('welcome_optin_logic: ok');
