#!/usr/bin/env node
/**
 * Behavioral tests for the computer_use bridge feature state machine
 * (src/platform/tauri/bridge/computer_use.js), run in a VM with fake
 * invoke/listen. Until the review fixes this module only had a protocol-hash
 * lock on its invoke/listen text and zero behavioral coverage, even though it
 * owns the safety-critical consent projection (staleness guard, per-session
 * pending merge, optimistic rollback, deny cooldown, disabled-gating).
 *
 * Run: node --test pinvou3-app/tests/computer_use_bridge.test.mjs
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

function createHarness({ status = {}, failInvoke = null, initialState = {} } = {}) {
  const invoked = [];
  const listeners = {};
  const published = [];
  let now = 1_000_000;
  const state = {
    activeSessionId: 's1',
    computerUse: { enabled: false, granted: false, stopped: false, platformSupported: true, ...initialState },
  };
  const harness = {
    invoked, state, listeners,
    published() { return published.map(entry => entry.slice); },
    advanceMs(ms) { now += ms; },
  };
  const context = vm.createContext({
    window: {},
    Date: {
      now() { return now; },
    },
    console,
  });
  vm.runInContext(fs.readFileSync(path.join(__dirname, '..', 'src', 'platform', 'tauri', 'bridge', 'computer_use.js'), 'utf8'), context, { filename: 'bridge/computer_use.js' });
  const factory = context.window.__PINVOU_TAURI_BRIDGE_FEATURES__.computer_use;
  assert.equal(typeof factory, 'function', 'computer_use feature must register itself');
  const feature = factory({
    state,
    notify() { published.push({ slice: { ...state.computerUse } }); },
    async invoke(command, args) {
      if (failInvoke && failInvoke(command, args)) throw new Error(`invoke failed: ${command}`);
      invoked.push([command, args]);
      if (command === 'computer_use_get_status') return status;
      return null;
    },
    listen(event, handler) { listeners[event] = handler; },
  });
  harness.feature = feature;
  return harness;
}

function emit(harness, event, payload) {
  const handler = harness.listeners[event];
  assert.ok(handler, `listener for ${event} must be registered`);
  handler({ payload });
}

(async () => {
  // ── 1. Stale get_status must not win ────────────────────────────────
  {
    const harness = createHarness({ initialState: { enabled: true } });
    // No await between the two refreshes: both capture seq 1 and 2, the first
    // (slow) response must be dropped.
    const first = harness.feature.refreshStatus('s1');
    const second = harness.feature.refreshStatus('s1');
    await first;
    await second;
    const slices = harness.published();
    assert.equal(slices.length, 1, `stale get_status must not publish: ${JSON.stringify(slices)}`);
  }

  // ── 2. Background-session grant request resurfaces on refresh ──────
  {
    const harness = createHarness();
    harness.state.activeSessionId = 's1';
    emit(harness, 'computer_use:grant_required', { session_id: 's2' });
    assert.equal(
      harness.published().length, 0,
      'a background session request must not publish into the active slice',
    );
    harness.state.activeSessionId = 's2';
    await harness.feature.refreshStatus('s2');
    const last = harness.published().at(-1);
    assert.ok(last && last.grantRequest && last.grantRequest.sessionId === 's2',
      `background request must resurface on refresh: ${JSON.stringify(last)}`);
  }

  // ── 3. setEnabled rolls back the optimistic toggle on failure ───────
  {
    const harness = createHarness({ failInvoke: (command) => command === 'computer_use_set_enabled' });
    await assert.rejects(harness.feature.setEnabled(true));
    assert.equal(harness.state.computerUse.enabled, false, 'failed setEnabled must roll back');
    const last = harness.published().at(-1);
    assert.equal(last.enabled, false, 'rollback must be published');
  }

  // ── 4. Events stay inert while disabled, resurface after refresh ────
  {
    const harness = createHarness();
    harness.state.computerUse.enabled = false;
    emit(harness, 'computer_use:grant_required', { session_id: 's1' });
    assert.equal(harness.published().length, 0, 'disabled feature must not publish dialogs');
    harness.state.computerUse.enabled = true;
    await harness.feature.refreshStatus('s1');
    const last = harness.published().at(-1);
    assert.ok(last && last.grantRequest, 'after enable, refresh must resurface the pending grant');
  }

  // ── 5. Explicit deny: backend command + cooldown on re-prompts ──────
  {
    const harness = createHarness({ initialState: { enabled: true } });
    emit(harness, 'computer_use:confirm_required', {
      session_id: 's1', confirm_id: 'cu-1', action: 'left click', element: 'Buy now',
    });
    assert.ok(harness.published().at(-1).confirmRequest, 'confirm dialog must be shown');
    await harness.feature.deny('cu-1');
    const denyCall = harness.invoked.find(([command]) => command === 'computer_use_deny');
    assert.equal(denyCall && denyCall[1] && denyCall[1].confirmId, 'cu-1',
      'deny must reach the backend instead of only closing the dialog locally');
    assert.equal(harness.published().at(-1).confirmRequest, null, 'dialog must close after deny');
    // Model retries immediately → the re-emitted request must NOT re-open the
    // modal (consent-fatigue guard), and the backend call still happens.
    emit(harness, 'computer_use:confirm_required', {
      session_id: 's1', confirm_id: 'cu-2', action: 'left click', element: 'Buy now',
    });
    assert.equal(
      harness.published().at(-1).confirmRequest, null,
      'a re-prompt inside the deny cooldown must not re-open the dialog',
    );
    // After the cooldown the dialog may re-appear.
    harness.advanceMs(30_001);
    emit(harness, 'computer_use:confirm_required', {
      session_id: 's1', confirm_id: 'cu-3', action: 'left click', element: 'Buy now',
    });
    assert.ok(harness.published().at(-1).confirmRequest, 'after the cooldown the dialog may re-open');
  }

  // ── 6. Grant deny latches too; a successful grant clears the latch ──
  {
    const harness = createHarness({ initialState: { enabled: true } });
    emit(harness, 'computer_use:grant_required', { session_id: 's1' });
    assert.ok(harness.published().at(-1).grantRequest);
    await harness.feature.revoke('s1');
    emit(harness, 'computer_use:grant_required', { session_id: 's1' });
    assert.equal(
      harness.published().at(-1).grantRequest, null,
      're-denied grant must not re-open the dialog inside the cooldown',
    );
    await harness.feature.grant('s1');
    assert.ok(harness.published().at(-1).granted, 'grant must publish granted=true');
    // The latch is cleared by a successful grant: a later request re-opens.
    emit(harness, 'computer_use:confirm_required', {
      session_id: 's1', confirm_id: 'cu-9', action: 'a', element: 'e',
    });
    assert.ok(harness.published().at(-1).confirmRequest, 'latch cleared by grant');
  }

  // ── 7. platform_supported flows into the published slice ────────────
  {
    const harness = createHarness({ initialState: { enabled: true }, status: { enabled: true, granted: false, stopped: false, platform_supported: false } });
    await harness.feature.refreshStatus('s1');
    const last = harness.published().at(-1);
    assert.equal(last.platformSupported, false, 'settings needs platform_supported to disable the toggle');
  }

  console.log('computer use bridge behavior tests passed');
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
