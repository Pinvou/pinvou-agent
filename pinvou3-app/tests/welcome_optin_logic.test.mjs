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
vm.runInContext(`${code}\nthis.consumeWelcomeOptIn = consumeWelcomeOptIn;\nthis.resolveSendCapabilityStatus = resolveSendCapabilityStatus;`, ctx, { filename: logicPath });
const { consumeWelcomeOptIn, resolveSendCapabilityStatus } = ctx;

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
{
  const calls = [];
  const result = await consumeWelcomeOptIn({
    getToolId: () => 'gongwen',
    consume: () => calls.push('consume'),
    invoke: async (cmd, args) => {
      calls.push([cmd, args]);
      return null;
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

// 4. Explicitly-disabled pack: backend returns blocked ids → surfaced, not failed.
{
  const result = await consumeWelcomeOptIn({
    getToolId: () => 'gongwen',
    consume: () => {},
    invoke: async () => ['gongwen'],
  });
  assert.strictEqual(result.attempted, true);
  assert.deepStrictEqual([...result.blocked], ['gongwen']);
  assert.strictEqual(result.failed, undefined);
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

console.log('welcome_optin_logic: ok');
