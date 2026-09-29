/**
 * Aux-chat controller contract: the async state machine extracted from
 * AuxChatPanel.jsx (round-30 B1), driven through the interleavings that the
 * review rounds hardened — send/restart guard ordering, the duplicate-send
 * lattice, ack-consumption classification and the atomic-reset (M6) paths —
 * with a fake bridge and fake timers, no React.
 *
 * M6 rewrite: restart is one backend `reset_aux_session` call (discard
 * through the turn gate + recreate atomically), so the two-invoke "stuck"
 * family is gone and the D1/D2 stuck-recovery tests were replaced by their
 * atomic-reset equivalents: a failed reset preserves the draft and surfaces
 * the honest ambiguous outcome, and a late send ack after a completed reset
 * structurally cannot mis-delete the draft (the truth table lives in the
 * controller's module header).
 */
import assert from 'node:assert/strict';
import test from 'node:test';
import {
  clearedIfSent,
  createAuxChatController,
  hasSendContent,
  reconcileLiveTaskIds,
} from '../src/features/aux-chat/aux-chat-controller.mjs';
import {
  clearAuxQuotes,
  getAuxQuotes,
  stageAuxQuote,
} from '../src/features/aux-chat/aux-quote.mjs';

const SEND_WATCHDOG_MS = 1_000;
const SETTLE_WATCHDOG_MS = 1_000;

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

// Deterministic timers: advance(ms) fires every timer due inside the window,
// including timers armed by callbacks that run during the advance.
function createFakeTimers() {
  let now = 0;
  let nextId = 1;
  const timers = new Map();
  const setTimeoutFn = (fn, ms) => {
    const id = nextId;
    nextId += 1;
    timers.set(id, { fn, at: now + ms });
    return id;
  };
  const clearTimeoutFn = (id) => { timers.delete(id); };
  const advance = (ms) => {
    now += ms;
    for (;;) {
      let dueId = null;
      let dueAt = Infinity;
      for (const [id, entry] of timers) {
        if (entry.at <= now && (entry.at < dueAt || (entry.at === dueAt && id < dueId))) {
          dueId = id;
          dueAt = entry.at;
        }
      }
      if (dueId === null) return;
      const entry = timers.get(dueId);
      timers.delete(dueId);
      entry.fn();
    }
  };
  return { setTimeout: setTimeoutFn, clearTimeout: clearTimeoutFn, advance };
}

// Fake bridge: sends and resets return manually-settled deferreds (the
// interleavings under test live exactly in their settle ordering), while
// ensure auto-resolves one aux id per task (idempotent, like the real
// mapping) and snapshot reads a per-aux map the test writes to simulate
// turn_started / turn-terminal events landing in the buffer.
function createFakeAuxChat() {
  const calls = { send: [], reset: [], ensure: [] };
  const snapshots = new Map();
  return {
    calls,
    snapshots,
    send(auxId, text) {
      const d = deferred();
      calls.send.push({ auxId, text, ...d });
      return d.promise;
    },
    reset(taskId) {
      const d = deferred();
      calls.reset.push({ taskId, ...d });
      return d.promise;
    },
    ensure(taskId) {
      calls.ensure.push({ taskId });
      return Promise.resolve(`aux-${taskId}`);
    },
    snapshot(auxId) {
      return snapshots.get(auxId) || null;
    },
  };
}

const flush = async (ticks = 30) => {
  for (let i = 0; i < ticks; i += 1) await Promise.resolve();
};

// turn_started lands and the turn ends: the busy-gated release owns the
// latch between the two notifies.
const observeTurnStarted = (harness, auxId) => {
  harness.auxChat.snapshots.set(auxId, { chatItems: [], busy: true, queued: [] });
  harness.notifyChat();
  harness.auxChat.snapshots.set(auxId, { chatItems: [], busy: false, queued: [] });
  harness.notifyChat();
};

function createHarness() {
  const timers = createFakeTimers();
  const auxChat = createFakeAuxChat();
  const chatListeners = new Set();
  const controller = createAuxChatController({
    setTimeout: timers.setTimeout,
    clearTimeout: timers.clearTimeout,
    sendWatchdogMs: SEND_WATCHDOG_MS,
    settleWatchdogMs: SETTLE_WATCHDOG_MS,
    restartConfirmMs: 50,
  });
  const mountPanel = (taskId) => {
    const panel = controller.createPanel();
    panel.setBridge(
      auxChat,
      (callback) => { chatListeners.add(callback); return () => chatListeners.delete(callback); },
    );
    panel.bind(taskId);
    return panel;
  };
  const notifyChat = () => { for (const callback of [...chatListeners]) callback(); };
  return { timers, auxChat, controller, mountPanel, notifyChat, flush, chatListeners };
}

// A bound panel: mounted, ensure resolved, auxId live.
async function mountBoundPanel(harness, taskId) {
  const panel = harness.mountPanel(taskId);
  await harness.flush();
  assert.equal(panel.view.auxId, `aux-${taskId}`, 'bind must ensure and bind the aux session');
  return panel;
}

// A confirmed restart: the two-step arm + confirm issues exactly one reset.
const confirmRestart = (panel) => {
  void panel.restart();
  void panel.restart();
};

test('round-29 B1: the send watchdog releases the latch when turn events coalesce (busy never observed)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'b1-main');
  panel.setDraftText('question one');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 1);
  assert.equal(panel.view.sending, true);
  // The dispatch ack resolves, but turn_started and the turn-terminal events
  // coalesce into one render batch — busy is never observed by a snapshot.
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  assert.equal(
    panel.view.sending,
    true,
    'the latch must survive the dispatch ack: turn_started still lags it (round-20 minor-4)',
  );
  h.timers.advance(SEND_WATCHDOG_MS);
  assert.equal(
    panel.view.sending,
    false,
    'the failsafe must un-dead the composer when busy was never observable (round-29 B1)',
  );
  panel.setDraftText('question two');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 2, 'the composer is usable again after the failsafe');
});

test('round-29 B1 control: busy observed → the busy-gated release owns the latch and the watchdog no-ops', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'b1-control');
  panel.setDraftText('question');
  void panel.send();
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  assert.equal(panel.view.sending, true);
  // turn_started lands: the busy-gated release owns the latch.
  h.auxChat.snapshots.set('aux-b1-control', { chatItems: [], busy: true, queued: [] });
  h.notifyChat();
  assert.equal(panel.view.sending, false, 'the latch releases when the snapshot proves the turn started');
  // The turn finishes; the watchdog firing later must be a no-op.
  h.auxChat.snapshots.set('aux-b1-control', { chatItems: [], busy: false, queued: [] });
  h.notifyChat();
  h.timers.advance(SEND_WATCHDOG_MS);
  assert.equal(panel.view.sending, false);
  panel.setDraftText('next');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 2, 'the watchdog no-op must not disturb later sends');
});

test('M6 truth table: a send ack landing inside or after a completed reset can never consume the recovery draft', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'm6-ack');
  panel.setDraftText('recovery material');
  void panel.send(); // dispatched at epoch 0, still in flight
  assert.equal(h.auxChat.calls.send.length, 1);
  confirmRestart(panel);
  assert.equal(h.auxChat.calls.reset.length, 1, 'the confirmed restart issues the atomic reset');
  // The reset completes FIRST: fresh binding, draft deliberately preserved.
  h.auxChat.calls.reset[0].resolve('aux-m6-ack');
  await h.flush();
  assert.equal(panel.view.auxId, 'aux-m6-ack', 'the reset success path binds the fresh session');
  assert.equal(panel.view.restarting, false);
  // The pre-restart send's ack settles LATE — after the reset completed.
  // The epoch changed since dispatch, so the ack is classified keep-draft:
  // no consumption, no banner, no mis-delete (the D1/D2 replacement: this
  // interleaving is structurally a no-op now, where the old two-invoke
  // lattice needed the survival-marker machinery to classify it).
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  assert.equal(panel.view.draft, 'recovery material', 'a late ack after the reset keeps the draft');
  assert.equal(panel.view.sendFailed, false, 'a late ack after the reset raises no banner');
  // The store entry survives too — a task round trip restores it.
  panel.bind('m6-other');
  await h.flush();
  panel.bind('m6-ack');
  await h.flush();
  assert.equal(panel.view.draft, 'recovery material', 'the draft store keeps the recovery text');
  // A late REJECTED ack after a completed reset is equally inert: nothing
  // was delivered, the draft stays, and the epoch gate silences the banner.
  panel.setDraftText('will be refused');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 2);
  confirmRestart(panel);
  h.auxChat.calls.reset[1].resolve('aux-m6-ack');
  await h.flush();
  h.auxChat.calls.send[1].reject(new Error('session being reset'));
  await h.flush();
  assert.equal(panel.view.draft, 'will be refused', 'a refused dispatch keeps its text');
  assert.equal(panel.view.sendFailed, false, 'the restart-window rejection stays silent');
});

test('a settle-bound reset keeps the failure banner across rebinds until the backend settles (round-35 MAJOR-1)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'settle-bound');
  confirmRestart(panel);
  assert.equal(h.auxChat.calls.reset.length, 1);
  // The backend reset is still alive when the 180 s UI bound fires (a long
  // aux answer alone can outrun it via the turn gate): the bound rejects,
  // the honest failure banner shows, and the task is MARKED.
  h.timers.advance(SETTLE_WATCHDOG_MS);
  await h.flush();
  assert.equal(panel.view.discardFailed, true, 'the bound surfaces the failure state');
  // The banner's own advice (switch tasks / reopen) must not walk the user
  // silently back into the pending delete: the rebind keeps the banner.
  panel.bind('settle-bound-other');
  await h.flush();
  panel.bind('settle-bound');
  await h.flush();
  assert.equal(
    panel.view.discardFailed,
    true,
    'a rebind must NOT auto-clear the banner while the backend reset is still pending',
  );
  assert.equal(panel.view.auxId, 'aux-settle-bound', 'the ensure still rebinds the surviving record');
  // The backend finally settles (either outcome): the hazard marker retires
  // and the next bind shows the truthful state.
  h.auxChat.calls.reset[0].resolve('aux-settle-bound');
  await h.flush();
  panel.bind('settle-bound-other');
  await h.flush();
  panel.bind('settle-bound');
  await h.flush();
  assert.equal(panel.view.discardFailed, false, 'the banner retires once the backend settles');
});

test('M6: a failed reset preserves the draft, surfaces the honest ambiguous banner, and the next bind re-ensures', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'm6-fail');
  panel.setDraftText('keep me');
  const ensuresAfterMount = h.auxChat.calls.ensure.length;
  confirmRestart(panel);
  h.auxChat.calls.reset[0].reject(new Error('web_session_reset_aux_session_failed'));
  await h.flush();
  assert.equal(panel.view.discardFailed, true, 'the failed reset surfaces its banner');
  assert.equal(panel.view.auxId, null, 'the binding stays cleared — the old transcript may be gone');
  assert.equal(panel.view.restarting, false, 'the outer finally frees the restart latch');
  assert.equal(panel.view.bindingPending, false);
  assert.equal(panel.view.draft, 'keep me', 'a failed reset never eats the draft');
  assert.equal(
    h.auxChat.calls.ensure.length,
    ensuresAfterMount,
    'no restore ensure fires inside the failed-reset path — recovery is the next bind',
  );
  // The re-ensure-on-next-bind recovery: any rebind re-ensures, and the
  // backend get-or-create rebinds the old transcript when it survived or
  // creates a fresh session. The banner clears with the rebind.
  panel.bind('m6-fail-other');
  await h.flush();
  panel.bind('m6-fail');
  await h.flush();
  assert.ok(h.auxChat.calls.ensure.length > ensuresAfterMount, 'the rebind re-ensures');
  assert.equal(panel.view.auxId, 'aux-m6-fail', 'the panel is bound again');
  assert.equal(panel.view.discardFailed, false, 'the banner clears on rebind');
  assert.equal(panel.view.draft, 'keep me', 'the draft store still holds the text');
});

test('M6: a hung reset surfaces the failure state at the settle bound and frees the latches; the late resolution stays inert', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'm6-hung');
  confirmRestart(panel);
  assert.equal(panel.view.restarting, true);
  // The reset invoke never settles: at the settle bound the withSettleBound
  // rejection surfaces as the reset-failure state and the outer finally
  // frees the latches (the settle watchdog's only surviving role).
  h.timers.advance(SETTLE_WATCHDOG_MS);
  await h.flush();
  assert.equal(panel.view.discardFailed, true, 'a hung reset surfaces as the reset-failure state');
  assert.equal(panel.view.auxId, null, 'the binding is honestly cleared');
  assert.equal(panel.view.restarting, false, 'the outer finally releases the restart latch');
  assert.equal(panel.view.bindingPending, false);
  // The registry entry left with the settle, so New Topic is usable again.
  confirmRestart(panel);
  assert.equal(h.auxChat.calls.reset.length, 2, 'the freed task can restart again');
  h.auxChat.calls.reset[1].resolve('aux-m6-hung');
  await h.flush();
  // The FIRST reset's late resolution stays inert behind the generation
  // check and the registry identity gate.
  h.auxChat.calls.reset[0].resolve('aux-m6-stale');
  await h.flush();
  assert.equal(panel.view.auxId, 'aux-m6-hung', 'the late resolution of the wedged reset is inert');
  assert.equal(panel.view.discardFailed, false);
});

test('restart gating: Enter is rejected once the restart entry runs, and New Topic is refused behind a healthy in-flight reset (the N1 intent)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'gated-task');
  panel.setDraftText('blocked text');
  // Enter after the restart entry is rejected: the confirmed restart is about
  // to reset the session this send would land in. The OPERATIVE guard is the
  // nulled binding (round-18 B-1): the entry clears view.auxId in the same
  // synchronous block that sets restarting, so `!sentAuxId` — not the
  // restarting conjunct — is what blocks this dispatch (mutation-verified:
  // deleting the restarting conjunct leaves this test green; the conjunct is
  // belt-and-braces over the nulled binding and stays as defense in depth).
  confirmRestart(panel);
  assert.equal(panel.view.restarting, true);
  assert.equal(panel.view.auxId, null, 'the restart entry nulls the binding — the operative Enter guard');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 0, 'a send past the restart entry never dispatches');
  assert.equal(h.auxChat.calls.reset.length, 1);
  // A→B→A resets `restarting` but leaves the healthy reset in flight: New
  // Topic must refuse a second reset (and un-arm the confirm, round-12 N1).
  panel.bind('gated-other');
  await h.flush();
  panel.bind('gated-task');
  await h.flush();
  assert.equal(panel.view.restarting, false);
  void panel.restart();
  assert.equal(h.auxChat.calls.reset.length, 1, 'no second reset while a healthy one is in flight');
  assert.equal(panel.view.restartArmed, false, 'the refusal un-arms the confirm');
  void panel.restart();
  assert.equal(h.auxChat.calls.reset.length, 1, 'the guard runs before arming, so arming is refused too');
  // The reset settles: the guard frees and the next New Topic goes through.
  h.auxChat.calls.reset[0].resolve('aux-gated-task');
  await h.flush();
  confirmRestart(panel);
  assert.equal(h.auxChat.calls.reset.length, 2, 'once the reset settles, New Topic works again');
});

test('bind awaits an in-flight reset before re-ensuring the same task (M6: no binding to the doomed aux session)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'await-task');
  const ensuresAfterMount = h.auxChat.calls.ensure.length;
  confirmRestart(panel);
  assert.equal(h.auxChat.calls.reset.length, 1);
  // The panel remounts behind the in-flight reset: the rebind must NOT
  // ensure yet — the old mapping is still live and the reset deletes it.
  panel.dispose();
  const panel2 = h.mountPanel('await-task');
  await h.flush();
  assert.equal(
    h.auxChat.calls.ensure.length,
    ensuresAfterMount,
    'the rebind waits for the in-flight reset instead of binding the doomed aux session',
  );
  assert.equal(panel2.view.auxId, null);
  // The reset settles: the awaiting rebind re-ensures and binds the fresh
  // session the reset just made.
  h.auxChat.calls.reset[0].resolve('aux-await-task');
  await h.flush();
  assert.ok(h.auxChat.calls.ensure.length > ensuresAfterMount, 'the settled reset releases the rebind ensure');
  assert.equal(panel2.view.auxId, 'aux-await-task', 'the binding is restored');
  assert.equal(panel2.view.bindingPending, false);
  // Same on the failure arm: the rebind still re-ensures (get-or-create
  // rebinds the surviving transcript or recreates), without a banner.
  confirmRestart(panel2);
  panel2.bind('await-other');
  await h.flush();
  panel2.bind('await-task');
  await h.flush();
  const ensuresBeforeFailure = h.auxChat.calls.ensure.length;
  h.auxChat.calls.reset[1].reject(new Error('reset failed'));
  await h.flush();
  assert.ok(h.auxChat.calls.ensure.length > ensuresBeforeFailure, 'the failed reset still releases the rebind ensure');
  assert.equal(panel2.view.auxId, 'aux-await-task', 'the rebind recovers the panel');
  assert.equal(panel2.view.discardFailed, false, 'the remounted panel shows no stale banner');
});

test('duplicate-send lattice: double Enter, an A→B→A switch mid-flight and the turn_started lag all stay single-dispatch', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'lattice-a');
  panel.setDraftText('first');
  // Double Enter inside the dispatch window: the synchronous latch swallows
  // the second one before the registry even sees it.
  void panel.send();
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 1, 'a double Enter fires exactly one bridge send');
  // The ack settles, but turn_started still lags: the same-panel latch keeps
  // blocking until busy is observed (round-20 minor-4).
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  panel.setDraftText('second');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 1, 'the latch holds between the ack and turn_started');
  observeTurnStarted(h, 'aux-lattice-a');
  assert.equal(panel.view.sending, false, 'turn_started releases the latch');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 2, 'once busy proved the turn started, the next send goes through');
  // A→B→A mid-flight: the bind resets the panel latch, so only the
  // module-scoped registry still guards the duplicate (round-15 MAJOR-2).
  panel.bind('lattice-b');
  await h.flush();
  panel.bind('lattice-a');
  await h.flush();
  assert.equal(panel.view.sending, false, 'the rebind resets the panel-level latch by design');
  assert.equal(panel.view.sendInFlight, true, 'the registry-derived hint covers the cross-switch window');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 2, 'the registry blocks the duplicate across the rebind');
  h.auxChat.calls.send[1].resolve({});
  await h.flush();
});

test('ack consumption: a same-task ack consumes only the draft that still equals the sent text, plus the captured quotes', async () => {
  const h = createHarness();
  stageAuxQuote('ack-task', 'excerpt one');
  const panel = await mountBoundPanel(h, 'ack-task');
  assert.equal(panel.view.quotes.length, 1, 'the staged quote rides into the composer');
  panel.setDraftText('what does this mean?');
  void panel.send();
  assert.match(h.auxChat.calls.send[0].text, /# userselect:/, 'the quote block travels inline');
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  assert.equal(panel.view.draft, '', 'the delivered draft is consumed');
  assert.equal(getAuxQuotes('ack-task').length, 0, 'only the captured quotes are dropped');
  // Text typed after the dispatch belongs to the next message and is never consumed.
  observeTurnStarted(h, 'aux-ack-task');
  panel.setDraftText('second message');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 2);
  panel.setDraftText('second message, edited');
  h.auxChat.calls.send[1].resolve({});
  await h.flush();
  assert.equal(panel.view.draft, 'second message, edited', 'typing since the dispatch survives the ack');
  panel.bind('ack-other');
  await h.flush();
  panel.bind('ack-task');
  await h.flush();
  assert.equal(panel.view.draft, 'second message, edited', 'the draft store keeps the newer text');
});

test('ack consumption: a restart-window ack keeps the recovery draft through a successful reset', async () => {
  const h = createHarness();
  stageAuxQuote('kept-task', 'staged excerpt');
  const panel = await mountBoundPanel(h, 'kept-task');
  panel.setDraftText('delivered before the restart');
  void panel.send();
  // The restart bumps the epoch and nulls the binding before the reset.
  confirmRestart(panel);
  // The ack settles inside the reset window: classified keep-draft, the text
  // stays as recovery material (the delivery, if it landed, dies with the
  // old transcript — see the module-header truth table).
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  assert.equal(panel.view.draft, 'delivered before the restart', 'the epoch-skipped ack keeps the draft');
  assert.equal(getAuxQuotes('kept-task').length, 1, 'the captured quotes stay staged as recovery material');
  // The reset SUCCEEDS: with the atomic reset there is no restore path that
  // could reclassify the delivery — the draft simply stays.
  h.auxChat.calls.reset[0].resolve('aux-kept-task');
  await h.flush();
  assert.equal(panel.view.auxId, 'aux-kept-task', 'the fresh session binds');
  assert.equal(panel.view.draft, 'delivered before the restart', 'a successful reset keeps the recovery draft');
  assert.equal(getAuxQuotes('kept-task').length, 1, 'a successful reset keeps the staged quotes');
  // And the draft store keeps it across a task round trip.
  panel.bind('kept-other');
  await h.flush();
  panel.bind('kept-task');
  await h.flush();
  assert.equal(panel.view.draft, 'delivered before the restart');
});

test('ack consumption: a re-typed or never-sent draft survives a failed reset', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'unmarked-task');
  panel.setDraftText('same question');
  void panel.send();
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  assert.equal(panel.view.draft, '', 'the ordinary ack consumes the original');
  // An ordinary re-ask: the user re-types the identical text, then the reset
  // fails. No ack is pending and nothing may consume the re-typed draft.
  panel.setDraftText('same question');
  confirmRestart(panel);
  h.auxChat.calls.reset[0].reject(new Error('reset failed'));
  await h.flush();
  assert.equal(
    panel.view.draft,
    'same question',
    'a draft re-typed after its ack was consumed must survive the failed reset',
  );
  // A never-sent draft survives a failed reset the same way.
  panel.setDraftText('never sent');
  confirmRestart(panel);
  h.auxChat.calls.reset[1].reject(new Error('reset failed again'));
  await h.flush();
  assert.equal(panel.view.draft, 'never sent');
});

test('bind ensure rejection surfaces ensureFailed and frees the pending hint', async () => {
  const h = createHarness();
  const ensureCalls = [];
  h.auxChat.ensure = (taskId) => {
    const d = deferred();
    ensureCalls.push({ taskId, ...d });
    return d.promise;
  };
  const panel = h.mountPanel('ensure-fail');
  await h.flush();
  assert.equal(panel.view.bindingPending, true, 'the pending hint shows while ensure is in flight');
  ensureCalls[0].reject(new Error('web_session_get_or_create_aux_session_failed'));
  await h.flush();
  assert.equal(panel.view.ensureFailed, true, 'a rejected bind ensure surfaces the init-failure state');
  assert.equal(panel.view.bindingPending, false);
  assert.equal(panel.view.auxId, null, 'the composer stays honestly disabled');
});

test('restart entry clears stale failure banners and a successful reset binds the fresh session', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'banner-task');
  // A failed send raises its banner first: the restart entry must clear it,
  // or it renders next to the reset outcome as a contradictory double
  // banner (round-9 minor-1).
  panel.setDraftText('will fail');
  void panel.send();
  h.auxChat.calls.send[0].reject(new Error('send failed'));
  await h.flush();
  assert.equal(panel.view.sendFailed, true);
  assert.equal(panel.view.sending, false, 'the failure path releases the latch directly');
  // The live rejection path keeps the draft (round-32 review M2): same
  // epoch, same task, the catch's gates all pass — nothing was delivered,
  // so the typed text stays staged. A mutation that wipes the draft in the
  // catch fails exactly here.
  assert.equal(panel.view.draft, 'will fail', 'an ordinary failed send keeps the typed draft');
  // The store entry survives too: a bind round trip restores it.
  panel.bind('banner-other');
  await h.flush();
  panel.bind('banner-task');
  await h.flush();
  assert.equal(panel.view.draft, 'will fail', 'the draft store restores the failed send\'s text');
  confirmRestart(panel);
  assert.equal(panel.view.sendFailed, false, 'the restart entry clears the stale send banner');
  h.auxChat.calls.reset[0].resolve('aux-banner-task');
  await h.flush();
  assert.equal(panel.view.auxId, 'aux-banner-task', 'the fresh session binds from the reset result');
  assert.equal(panel.view.restarting, false);
  assert.equal(panel.view.bindingPending, false);
  assert.equal(panel.view.discardFailed, false, 'a successful reset shows no failure banner');
});

test('restart entry bumps the generation so a stale in-flight bind ensure cannot rebind the discarded session (round-11 B3)', async () => {
  const h = createHarness();
  const ensureCalls = [];
  h.auxChat.ensure = (taskId) => {
    const d = deferred();
    ensureCalls.push({ taskId, ...d });
    return d.promise;
  };
  const panel = h.mountPanel('gen-task');
  // The initial bind's ensure is still in flight when the restart runs.
  confirmRestart(panel);
  assert.equal(h.auxChat.calls.reset.length, 1, 'the restart issued its atomic reset');
  h.auxChat.calls.reset[0].resolve('aux-fresh');
  await h.flush();
  // The stale bind ensure resolves LAST: it must not rebind the panel to
  // the just-discarded aux session.
  ensureCalls[0].resolve('aux-stale');
  await h.flush();
  assert.equal(panel.view.auxId, 'aux-fresh', 'the stale bind ensure is inert past the entry bump');
});

test('watchdog identity: a stale settle after the failsafe cannot remove a newer send\'s registry entry or release its latch', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'stale-task');
  panel.setDraftText('first');
  void panel.send();
  // The invoke never settles inside the bound: the watchdog deletes the
  // registry entry by identity and releases the latch.
  h.timers.advance(SEND_WATCHDOG_MS);
  assert.equal(panel.view.sending, false);
  // A newer send owns the registry slot and the latch now.
  panel.setDraftText('second');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 2);
  assert.equal(panel.view.sending, true);
  // The stale invoke resolves late: its finally must not remove the newer
  // entry (promise identity), and its success path touches no latch.
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  assert.equal(panel.view.sending, true, 'the stale settle must not release the newer send\'s latch');
  assert.equal(panel.view.draft, 'second', 'the stale ack must not consume the newer send\'s draft');
  // With the latch released by turn_started, only the registry still guards
  // the duplicate window — the newer entry must have survived the stale settle.
  h.auxChat.snapshots.set('aux-stale-task', { chatItems: [], busy: true, queued: [] });
  h.notifyChat();
  assert.equal(panel.view.sending, false);
  // End the turn: the third send must now pass the busy precheck and the
  // (released) latch, so the registry guard is the ONLY gate left — this
  // assertion genuinely pins the promise identity (round-33 MAJOR-4: with
  // the snapshot left busy the dispatch died at the busy precheck first and
  // the identity check was never consulted).
  h.auxChat.snapshots.set('aux-stale-task', { chatItems: [], busy: false, queued: [] });
  h.notifyChat();
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 2, 'the newer registry entry survives the stale settle');
  h.auxChat.calls.send[1].resolve({});
  await h.flush();
});

test('watchdog identity: a stale rejection after the failsafe stays silent', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'stale-reject');
  panel.setDraftText('first');
  void panel.send();
  h.timers.advance(SEND_WATCHDOG_MS);
  panel.setDraftText('second');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 2);
  // The stale invoke rejects late: the banner gates (registry identity,
  // epoch, live task) keep it silent and the latch with the newer send.
  h.auxChat.calls.send[0].reject(new Error('stale failure'));
  await h.flush();
  assert.equal(panel.view.sendFailed, false, 'a superseded rejection never raises the banner (round-24 minor-9)');
  assert.equal(panel.view.sending, true, 'a superseded rejection never releases the newer latch');
  h.auxChat.calls.send[1].resolve({});
  await h.flush();
  assert.equal(panel.view.sendFailed, false);
  assert.equal(panel.view.draft, '', 'the newer send\'s own ack still consumes its draft');
});

// ── Round-33 MAJOR-4: the cross-task / cross-instance settle class ──

test('cross-task settle: an A ack after binding B leaves B\'s staged quotes and composer untouched (m55 quotes leg)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'quote-cross-a');
  panel.setDraftText('shared text');
  stageAuxQuote('quote-cross-a', 'excerpt from A');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 1);
  // Switch to B, stage a B quote and retype the same text: a mutated ack
  // that drops the task gate would consume B's composer AND drop B's quotes
  // as if they rode the A dispatch.
  panel.bind('quote-cross-b');
  await h.flush();
  panel.setDraftText('shared text');
  stageAuxQuote('quote-cross-b', 'excerpt from B');
  h.notifyChat();
  const bQuotes = () => panel.view.quotes.map((quote) => quote.text);
  assert.deepEqual(bQuotes(), ['excerpt from B'], 'B stages its own quote');
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  assert.equal(panel.view.draft, 'shared text', 'the old task\'s ack must not clear the new task\'s composer');
  assert.deepEqual(bQuotes(), ['excerpt from B'], 'the old task\'s ack must not drop the new task\'s staged quotes');
  // A's own store entry WAS consumed; B's quote inventory is untouched.
  panel.bind('quote-cross-a');
  await h.flush();
  assert.equal(panel.view.draft, '', 'the delivered draft left the old task\'s store');
  panel.bind('quote-cross-b');
  await h.flush();
  assert.deepEqual(panel.view.quotes.map((quote) => quote.text), ['excerpt from B']);
});

test('restart + in-flight send: the late ack after the completed reset keeps the recovery quotes and leaves the fresh binding dispatchable', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'restart-inflight');
  panel.setDraftText('recovery material');
  stageAuxQuote('restart-inflight', 'recovery excerpt');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 1);
  confirmRestart(panel);
  h.auxChat.calls.reset[0].resolve('aux-restart-inflight');
  await h.flush();
  assert.equal(panel.view.auxId, 'aux-restart-inflight');
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  assert.equal(panel.view.draft, 'recovery material', 'the late ack keeps the draft (truth table)');
  assert.deepEqual(
    panel.view.quotes.map((quote) => quote.text),
    ['recovery excerpt'],
    'the late ack keeps the staged quotes: they were not part of the destroyed dispatch',
  );
  assert.equal(panel.view.sendFailed, false);
  assert.equal(panel.view.sending, false);
  // The late ack left no registry or latch residue: the fresh binding
  // dispatches a new turn normally.
  panel.setDraftText('after the restart');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 2, 'the fresh binding accepts a new dispatch after the late ack');
  h.auxChat.calls.send[1].resolve({});
  await h.flush();
});

test('watchdog after a restart: a never-settling pre-restart send frees the registry at the bound and the post-restart composer dispatches (round-25 MAJOR-24-3 × round-29 B1)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'watch-restart');
  panel.setDraftText('pre-restart question');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 1);
  // The pre-restart invoke never settles; the restart completes around it
  // (the send registry entry is deliberately KEPT through the restart).
  confirmRestart(panel);
  h.auxChat.calls.reset[0].resolve('aux-watch-restart');
  await h.flush();
  assert.equal(panel.view.auxId, 'aux-watch-restart');
  // While the stale entry stands, the registry guard blocks the fresh
  // binding's composer — the round-25 keep is what makes the entry the
  // blocker, and the watchdog is its only bounded exit.
  panel.setDraftText('post-restart question');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 1, 'the kept entry still blocks new dispatches');
  assert.equal(panel.view.sendInFlight, true, 'the registry-derived hint covers the blocked window');
  assert.equal(panel.view.sending, false, 'a registry-blocked dispatch never sets the panel latch');
  // The stale failsafe fires: it deletes the pre-restart entry by identity
  // and must leave the fresh panel's latches alone (the captured generation
  // and binding belong to the pre-restart send — the release halves are
  // defense-in-depth here: the restart entry already released this latch).
  h.timers.advance(SEND_WATCHDOG_MS);
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 2, 'the freed registry lets the post-restart composer dispatch');
  h.auxChat.calls.send[1].resolve({});
  await h.flush();
  assert.equal(panel.view.draft, '', 'the post-restart send consumes its own draft normally');
});

test('a failsafe firing after the busy-gated release is fully inert: no extra emission, no state churn (round-33 MAJOR-4)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'inert-watch');
  const emissions = [];
  const unsubscribe = panel.subscribe((snapshot) => emissions.push(snapshot.sending));
  panel.setDraftText('question');
  void panel.send();
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  assert.equal(panel.view.sending, true);
  // turn_started lands and the turn ends in one batch: the busy-gated
  // release owns the latch; the composer is idle again.
  h.auxChat.snapshots.set('aux-inert-watch', { chatItems: [], busy: true, queued: [] });
  h.notifyChat();
  h.auxChat.snapshots.set('aux-inert-watch', { chatItems: [], busy: false, queued: [] });
  h.notifyChat();
  assert.equal(panel.view.sending, false);
  const emissionsBeforeWatchdog = emissions.length;
  h.timers.advance(SEND_WATCHDOG_MS);
  assert.equal(panel.view.sending, false);
  assert.equal(
    emissions.length,
    emissionsBeforeWatchdog,
    'the late failsafe must return at the latch half: a redundant emit would churn every mounted subscriber',
  );
  unsubscribe();
});

test('hasSendContent accepts text-only, quote-only and both — and only those (round-30 B1 canary)', async () => {
  assert.equal(hasSendContent('', ''), false, 'neither text nor quotes: no dispatch');
  assert.equal(hasSendContent('hello', ''), true, 'text-only is a valid send');
  assert.equal(hasSendContent('', '\n\n# userselect:\n```userselect\n[]\n```'), true, 'quote-only is a valid send');
  assert.equal(hasSendContent('hello', 'block'), true, 'text plus quotes is a valid send');
  // End-to-end through the controller: a quote-only send must dispatch (the
  // `||` → `&&` mutation killed exactly this and every text-only send while
  // the whole suite stayed green).
  const h = createHarness();
  stageAuxQuote('canary-task', 'const x = 1;');
  const panel = await mountBoundPanel(h, 'canary-task');
  assert.equal(panel.view.draft, '');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 1, 'a quote-only send dispatches with an empty draft');
  assert.match(h.auxChat.calls.send[0].text, /# userselect:/);
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  observeTurnStarted(h, 'aux-canary-task');
  panel.setDraftText('plain text');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 2, 'a text-only send dispatches with no staged quotes');
  h.auxChat.calls.send[1].resolve({});
  await h.flush();
  // Neither: the composer guards stay closed.
  assert.equal(panel.view.draft, '');
  assert.equal(panel.view.quotes.length, 0);
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 2, 'an empty composer never dispatches');
});

test('M5: purgeTask drops the restart epoch, the stored draft and the staged quotes of a deleted task', async () => {
  const h = createHarness();
  stageAuxQuote('purge-task', 'staged excerpt');
  const panel = await mountBoundPanel(h, 'purge-task');
  panel.setDraftText('unsent draft');
  confirmRestart(panel);
  h.auxChat.calls.reset[0].resolve('aux-purge-task');
  await h.flush();
  assert.equal(panel.view.quotes.length, 1);
  // Sanity: a task round trip restores both the draft and the quotes.
  panel.bind('purge-other');
  await h.flush();
  panel.bind('purge-task');
  await h.flush();
  assert.equal(panel.view.draft, 'unsent draft');
  assert.equal(panel.view.quotes.length, 1);
  // The sessions domain reports the task deleted: every per-task registry
  // entry goes (restart epoch, stored draft, staged quotes).
  h.controller.purgeTask('purge-task');
  panel.bind('purge-other');
  await h.flush();
  panel.bind('purge-task');
  await h.flush();
  assert.equal(panel.view.draft, '', 'the stored draft is purged');
  assert.equal(panel.view.quotes.length, 0, 'the staged quotes are purged');
  // The epoch purge is observable in the ack classification: a send
  // dispatched BEFORE a restart, whose task was then purged, classifies
  // against a fresh epoch — the keep-draft skip no longer fires (the
  // recreated task starts at epoch 0 like any new task). Without the purge
  // the stale epoch would keep this ack's draft.
  panel.setDraftText('in flight');
  void panel.send();
  confirmRestart(panel);
  h.controller.purgeTask('purge-task');
  panel.setDraftText('in flight');
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  assert.equal(panel.view.draft, '', 'the purged epoch reads as a fresh task: the ack consumes normally');
  h.auxChat.calls.reset[1].resolve('aux-purge-task');
  await h.flush();
});

test('reconcileLiveTaskIds: a mid-life empty listing is skipped, not treated as mass deletion (round-32 minor 14)', () => {
  const known = new Set(['kept-a', 'kept-b']);
  const purged = [];
  reconcileLiveTaskIds(known, new Set(), (id) => purged.push(id));
  assert.deepEqual(purged, [], 'an empty live listing must not purge every known task');
  assert.deepEqual(
    [...known].sort((a, b) => a.localeCompare(b)),
    ['kept-a', 'kept-b'],
    'known ids survive the empty listing',
  );
  // The guard is one-way: a non-empty listing still diffs normally, and the
  // initial-empty case (nothing known) never bailed in the first place.
  reconcileLiveTaskIds(known, new Set(['kept-a']), (id) => purged.push(id));
  assert.deepEqual(purged, ['kept-b'], 'a real listing still purges genuinely deleted ids');
});

test('purgeTask leaves the in-flight send registry to its own settle path (round-34 minor 19)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'purge-inflight');
  panel.setDraftText('in flight');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 1);
  // The M5 purge drops the draft and the epoch, but the documented
  // invariant says the in-flight registries belong to the exact operation
  // that registered them — a deletion-during-flight must not free the
  // duplicate-send window.
  h.controller.purgeTask('purge-inflight');
  panel.setDraftText('again');
  void panel.send();
  assert.equal(
    h.auxChat.calls.send.length,
    1,
    'the in-flight entry still blocks a second dispatch after the purge',
  );
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  // turn_started lands and the turn ends: the busy-gated release owns the
  // latch (the ack alone deliberately leaves it held, round-20 minor-4).
  h.auxChat.snapshots.set('aux-purge-inflight', { chatItems: [], busy: true, queued: [] });
  h.notifyChat();
  h.auxChat.snapshots.set('aux-purge-inflight', { chatItems: [], busy: false, queued: [] });
  h.notifyChat();
  panel.setDraftText('after settle');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 2, 'the composer reopens when the send settles');
  h.auxChat.calls.send[1].resolve({});
  await h.flush();
});

test('M5: reconcileLiveTaskIds purges only ids that disappeared after being seen', () => {
  const known = new Set();
  const purged = [];
  const purge = (taskId) => purged.push(taskId);
  // The initial empty snapshot is "never seen", not "everything deleted":
  // ids are only registered as they appear.
  reconcileLiveTaskIds(known, new Set(['a', 'b']), purge);
  assert.deepEqual(purged, []);
  assert.deepEqual([...known], ['a', 'b']);
  // 'a' disappears (deleted), 'c' appears: only 'a' is purged.
  reconcileLiveTaskIds(known, new Set(['b', 'c']), purge);
  assert.deepEqual(purged, ['a']);
  assert.deepEqual([...known], ['b', 'c']);
  // A steady state purges nothing, and a purged id stays forgotten.
  reconcileLiveTaskIds(known, new Set(['b', 'c']), purge);
  assert.deepEqual(purged, ['a']);
});

test('clearedIfSent clears only a composer that still equals the sent text', () => {
  assert.equal(clearedIfSent('delivered', 'delivered'), '');
  assert.equal(clearedIfSent('delivered, edited', 'delivered'), 'delivered, edited');
  assert.equal(clearedIfSent('  delivered  ', 'delivered'), '');
});

// ── Round-31 M9: executing coverage for the survivor classes the mutation
// sweep measured on the post-M6 controller (emit/subscribe/dispose, the
// per-branch guards whose deletion used to stay green). Each test names the
// mutation id(s) it kills.

test('subscribe/emit: mutations reach subscribers as fresh copies and unsubscribe stops them (m16/m16b/m91/m92)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'emit-a');
  const seen = [];
  const unsubscribe = panel.subscribe((copy) => seen.push(copy));
  panel.setDraftText('one');
  panel.setDraftText('two');
  assert.equal(seen.length, 2, 'each state mutation emits exactly once');
  assert.equal(seen[0].draft, 'one');
  assert.equal(seen[1].draft, 'two');
  assert.notEqual(seen[0], seen[1], 'subscribers receive a fresh copy, not the live view');
  panel.setDraftText('three');
  assert.equal(seen[0].draft, 'one', 'an earlier copy is not mutated in place');
  unsubscribe();
  panel.setDraftText('four');
  assert.equal(seen.length, 3, 'an unsubscribed listener is never invoked again');
});

test('emit: an unchanged snapshot pull does not re-notify subscribers (m18 anti-rerender short-circuit)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'rerender-a');
  let emissions = 0;
  panel.subscribe(() => { emissions += 1; });
  h.notifyChat();
  assert.equal(emissions, 0, 'an unchanged snapshot does not re-render (the main session streams through this notify)');
  h.auxChat.snapshots.set('aux-rerender-a', { chatItems: [], busy: true, queued: [] });
  h.notifyChat();
  assert.equal(emissions, 1, 'a changed snapshot re-renders exactly once');
  h.notifyChat();
  assert.equal(emissions, 1, 'a repeated unchanged pull stays silent');
});

test('dispose() tears down every subscription, the arm timer and the listener set (m85/m86/m87/m88/m89)', async () => {
  const h = createHarness();
  // A late ensure resolution after dispose stays inert (the disposed leg of
  // bindStale): a dead panel must never re-bind itself.
  const ensureCalls = [];
  h.auxChat.ensure = () => {
    const d = deferred();
    ensureCalls.push(d);
    return d.promise;
  };
  const panel = h.mountPanel('disp-a');
  assert.equal(h.chatListeners.size, 1);
  panel.dispose();
  assert.equal(h.chatListeners.size, 0, 'the chat-domain subscription is released');
  ensureCalls[0].resolve('aux-disp-a');
  await h.flush();
  assert.equal(panel.view.auxId, null, 'a late ensure after dispose is inert');
  h.auxChat.ensure = (taskId) => Promise.resolve(`aux-${taskId}`);

  // The quote-store subscription is released: quotes staged for the old task
  // after dispose must not land on the dead panel's view.
  stageAuxQuote('disp-b', 'staged');
  const panel2 = await mountBoundPanel(h, 'disp-b');
  assert.equal(panel2.view.quotes.length, 1);
  panel2.dispose();
  stageAuxQuote('disp-b', 'second');
  assert.equal(panel2.view.quotes.length, 1, 'the quote subscription is released');
  clearAuxQuotes('disp-b');

  // The draft-delete mirror is released: a store purge after dispose must not
  // touch the dead panel's composer.
  const panel3 = await mountBoundPanel(h, 'disp-c');
  panel3.setDraftText('keep');
  panel3.dispose();
  h.controller.purgeTask('disp-c');
  assert.equal(panel3.view.draft, 'keep', 'the draft-delete subscription is released');

  // The armed New Topic confirm timer is cleared at dispose.
  const panel4 = await mountBoundPanel(h, 'disp-d');
  void panel4.restart();
  assert.equal(panel4.view.restartArmed, true);
  panel4.dispose();
  h.timers.advance(50);
  assert.equal(panel4.view.restartArmed, true, 'the armed confirm timer was cleared at dispose');

  // Subscribers are never notified after dispose.
  const panel5 = await mountBoundPanel(h, 'disp-e');
  let calls = 0;
  panel5.subscribe(() => { calls += 1; });
  panel5.dispose();
  panel5.setDraftText('x');
  assert.equal(calls, 0, 'no emission reaches subscribers after dispose');
});

test('a send ack settling on a dead instance clears the remounted composer through the draft-delete mirror (m32, round-25 MAJOR-24-2)', async () => {
  const h = createHarness();
  const panel1 = await mountBoundPanel(h, 'mirror-a');
  panel1.setDraftText('question');
  void panel1.send();
  assert.equal(h.auxChat.calls.send.length, 1);
  // The panel closes while the send is in flight; the registry entry survives
  // (module-scoped by design) and the remounted panel restores the draft from
  // the store — it still equals the in-flight text.
  panel1.dispose();
  const panel2 = await mountBoundPanel(h, 'mirror-a');
  assert.equal(panel2.view.draft, 'question', 'the remount restores the unsent-looking draft');
  // The ack settles on the dead instance: its consumption deletes the store
  // entry and the mirror clears the MOUNTED composer's copy — otherwise the
  // delivered message sits there staged for a duplicate Enter.
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  assert.equal(panel2.view.draft, '', 'the mirror clears the remounted composer when the ack consumes the store entry');
});

test('two live panels on one task: an ack on one clears both composers through the shared draft-delete mirror (round-33 MAJOR-4)', async () => {
  const h = createHarness();
  const panel1 = await mountBoundPanel(h, 'twin-a');
  const panel2 = await mountBoundPanel(h, 'twin-a');
  panel1.setDraftText('question');
  // The store entry is set by panel1's typing; panel2's bind ran before it,
  // so pull its copy forward to mirror a genuine two-mounted window.
  panel2.setDraftText('question');
  assert.equal(panel2.view.draft, 'question');
  void panel1.send();
  assert.equal(h.auxChat.calls.send.length, 1);
  // The ack consumes the store entry and notifies the task's listener SET:
  // every mounted instance's mirror must clear, not just the sender's —
  // panel2 keeps the delivered text staged for a duplicate Enter otherwise.
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  assert.equal(panel1.view.draft, '', 'the sender\'s composer clears through its own ack path');
  assert.equal(panel2.view.draft, '', 'the second live panel clears through the draft-delete mirror');
});

test('bind resets the binding and the stale banners for the new task (m21/m22)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'rebind-a');
  panel.setDraftText('will fail');
  void panel.send();
  h.auxChat.calls.send[0].reject(new Error('send failed'));
  await h.flush();
  assert.equal(panel.view.sendFailed, true);
  // The rebind synchronously drops the old binding and the old task's banner:
  // neither may linger into the new task's ensure window.
  panel.bind('rebind-b');
  assert.equal(panel.view.auxId, null, 'the old binding is dropped while the new ensure runs');
  assert.equal(panel.view.sendFailed, false, 'the old task\'s failure banner does not follow the rebind');
  assert.equal(panel.view.bindingPending, true);
  await h.flush();
  assert.equal(panel.view.auxId, 'aux-rebind-b');
});

test('bind resets the send latch: the new task sends while the old task\'s send is in flight (m26)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'latch-a');
  panel.setDraftText('a in flight');
  void panel.send();
  assert.equal(panel.view.sending, true);
  panel.bind('latch-b');
  assert.equal(panel.view.sending, false, 'the panel-level send latch resets on rebind');
  await h.flush();
  assert.equal(panel.view.auxId, 'aux-latch-b');
  panel.setDraftText('b message');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 2, 'the new task can send while the old task\'s send is still in flight');
  h.auxChat.calls.send[0].resolve({});
  h.auxChat.calls.send[1].resolve({});
  await h.flush();
});

test('bind clears a stale ensureFailed banner (m23)', async () => {
  const h = createHarness();
  const ensureCalls = [];
  h.auxChat.ensure = () => {
    const d = deferred();
    ensureCalls.push(d);
    return d.promise;
  };
  const panel = h.mountPanel('ef-a');
  await h.flush();
  ensureCalls[0].reject(new Error('ensure failed'));
  await h.flush();
  assert.equal(panel.view.ensureFailed, true);
  h.auxChat.ensure = (taskId) => Promise.resolve(`aux-${taskId}`);
  panel.bind('ef-b');
  assert.equal(panel.view.ensureFailed, false, 'the init-failure banner does not follow the rebind');
  await h.flush();
  assert.equal(panel.view.auxId, 'aux-ef-b');
});

test('bind renews the quote subscription per task: the old task\'s quotes stop landing (m30)', async () => {
  const h = createHarness();
  stageAuxQuote('quote-a', 'excerpt a1');
  const panel = await mountBoundPanel(h, 'quote-a');
  assert.equal(panel.view.quotes.length, 1);
  panel.bind('quote-b');
  assert.equal(panel.view.quotes.length, 0, 'the new task starts with its own (empty) quotes');
  await h.flush();
  stageAuxQuote('quote-a', 'excerpt a2');
  assert.equal(panel.view.quotes.length, 0, 'the dropped subscription no longer feeds the rebound panel');
  panel.bind('quote-a');
  assert.equal(panel.view.quotes.length, 2, 'rebinding restores the old task\'s staged quotes');
  clearAuxQuotes('quote-a');
});

test('send entry: no dispatch without a binding or while the snapshot is busy (m42/m43)', async () => {
  const h = createHarness();
  const panel = h.mountPanel('guard-a');
  panel.setDraftText('too early');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 0, 'no dispatch while the ensure is still in flight');
  await h.flush();
  assert.equal(panel.view.auxId, 'aux-guard-a');
  h.auxChat.snapshots.set('aux-guard-a', { chatItems: [], busy: true, queued: [] });
  h.notifyChat();
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 0, 'no dispatch while the session is busy — the backend would reject it as a bogus failure');
  h.auxChat.snapshots.set('aux-guard-a', { chatItems: [], busy: false, queued: [] });
  h.notifyChat();
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 1, 'the idle session accepts the send');
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
});

test('setDraftText clears the send-failure banner on the next keystroke (m41)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'banner-clear');
  panel.setDraftText('will fail');
  void panel.send();
  h.auxChat.calls.send[0].reject(new Error('send failed'));
  await h.flush();
  assert.equal(panel.view.sendFailed, true);
  panel.setDraftText('will fail, edited');
  assert.equal(panel.view.sendFailed, false, 'typing after a failure clears the stale retry banner');
});

test('a stale watchdog failsafe after a rebind never releases the newer send\'s latch (m50/m51)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'own-a');
  panel.setDraftText('a in flight');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 1);
  panel.bind('own-b');
  await h.flush();
  panel.setDraftText('b in flight');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 2);
  assert.equal(panel.view.sending, true);
  // The raw buffer says B's turn started, but no notify delivered it (the
  // coalesced-events premise the failsafe exists for): both failsafes fire at
  // the bound — A's must die at the generation gate, B's at the busy re-read.
  h.auxChat.snapshots.set('aux-own-b', { chatItems: [], busy: true, queued: [] });
  h.timers.advance(SEND_WATCHDOG_MS);
  assert.equal(panel.view.sending, true, 'neither stale failsafe may release the newer send\'s latch');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 2, 'the duplicate-send window stays closed');
  // Once the busy snapshot is observed, the busy-gated release owns the latch.
  h.notifyChat();
  assert.equal(panel.view.sending, false);
  h.auxChat.calls.send[1].resolve({});
  await h.flush();
});

test('a rejection settling after a task switch never banners the new task (m60)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'switch-a');
  panel.setDraftText('a will fail');
  void panel.send();
  panel.bind('switch-b');
  await h.flush();
  h.auxChat.calls.send[0].reject(new Error('a failed late'));
  await h.flush();
  assert.equal(panel.view.sendFailed, false, 'the old task\'s failure does not banner the new task');
  assert.equal(panel.view.auxId, 'aux-switch-b');
});

test('an ack settling after a task switch never touches the new task\'s composer (m55)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'cross-a');
  panel.setDraftText('shared text');
  void panel.send();
  panel.bind('cross-b');
  await h.flush();
  panel.setDraftText('shared text');
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  assert.equal(panel.view.draft, 'shared text', 'the old task\'s ack must not clear the new task\'s composer');
  // The old task's own store entry WAS consumed: a round trip restores nothing.
  panel.bind('cross-a');
  await h.flush();
  assert.equal(panel.view.draft, '', 'the delivered draft left the old task\'s store');
});

test('ack consumption: a clean ack purges the store so a task round trip restores an empty composer (m10/m53)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'store-a');
  panel.setDraftText('delivered');
  void panel.send();
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  assert.equal(panel.view.draft, '');
  panel.bind('store-b');
  await h.flush();
  panel.bind('store-a');
  await h.flush();
  assert.equal(panel.view.draft, '', 'a delivered message never re-appears from the draft store');
});

test('restart entry shows the preparing state and the nulled binding keeps the old transcript out (m73/m74/m75)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'win-a');
  h.auxChat.snapshots.set('aux-win-a', {
    chatItems: [{ type: 'user', text: 'hello' }],
    busy: false,
    queued: [],
  });
  h.notifyChat();
  assert.equal(panel.view.snapshot.chatItems.length, 1, 'the transcript renders before the restart');
  confirmRestart(panel);
  assert.equal(panel.view.auxId, null);
  assert.equal(panel.view.bindingPending, true, 'the preparing hint covers the reset window');
  assert.equal(panel.view.snapshot.chatItems.length, 0, 'the old transcript clears at entry');
  // A streaming notify during the reset window must not re-pull the doomed
  // session's transcript: with the binding nulled, the chat subscription has
  // nothing to pull.
  h.notifyChat();
  assert.equal(panel.view.snapshot.chatItems.length, 0, 'no stale re-pull during the reset window');
  h.auxChat.snapshots.delete('aux-win-a');
  h.auxChat.calls.reset[0].resolve('aux-win-a');
  await h.flush();
  assert.equal(panel.view.bindingPending, false);
  assert.equal(panel.view.auxId, 'aux-win-a');
});

test('restart entry clears a stale ensureFailed banner (m71)', async () => {
  const h = createHarness();
  const ensureCalls = [];
  h.auxChat.ensure = () => {
    const d = deferred();
    ensureCalls.push(d);
    return d.promise;
  };
  const panel = h.mountPanel('ref-a');
  await h.flush();
  ensureCalls[0].reject(new Error('ensure failed'));
  await h.flush();
  assert.equal(panel.view.ensureFailed, true);
  confirmRestart(panel);
  assert.equal(panel.view.ensureFailed, false, 'the stale init failure does not render next to the reset outcome');
  h.auxChat.calls.reset[0].resolve('aux-ref-a');
  await h.flush();
  assert.equal(panel.view.auxId, 'aux-ref-a');
});

test('restart entry releases the send latch so a late-settling send cannot dead the composer (m72, round-12 N2)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'n2-a');
  panel.setDraftText('in flight');
  void panel.send();
  assert.equal(panel.view.sending, true);
  confirmRestart(panel);
  // The send settles late inside the reset window: classified keep-draft by
  // the epoch, its finally deliberately does not touch the latch.
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  h.auxChat.calls.reset[0].resolve('aux-n2-a');
  await h.flush();
  panel.setDraftText('after the restart');
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 2, 'the composer is usable after the restart');
  h.auxChat.calls.send[1].resolve({});
  await h.flush();
});

test('a reset success landing after a rebind stays inert (m79)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'gen-a');
  confirmRestart(panel);
  panel.bind('gen-b');
  await h.flush();
  assert.equal(panel.view.auxId, 'aux-gen-b');
  h.auxChat.calls.reset[0].resolve('aux-gen-a');
  await h.flush();
  assert.equal(panel.view.auxId, 'aux-gen-b', 'the moved-on panel never binds the reset\'s fresh session');
  assert.equal(panel.view.discardFailed, false);
});

test('the armed New Topic confirm expires and the next click re-arms instead of confirming (m67)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'arm-a');
  void panel.restart();
  assert.equal(panel.view.restartArmed, true);
  assert.equal(h.auxChat.calls.reset.length, 0);
  h.timers.advance(50);
  assert.equal(panel.view.restartArmed, false, 'the armed confirm expires after the confirm window');
  void panel.restart();
  assert.equal(panel.view.restartArmed, true, 'a click after expiry re-arms');
  assert.equal(h.auxChat.calls.reset.length, 0, 'an expired arm never confirms on its own');
  void panel.restart();
  assert.equal(h.auxChat.calls.reset.length, 1, 'the second click inside the window confirms');
  h.auxChat.calls.reset[0].resolve('aux-arm-a');
  await h.flush();
});

test('setBridge refreshes the chat subscription on a bridge flip and never double-subscribes (m37/m39)', async () => {
  const h = createHarness();
  const panel = h.controller.createPanel();
  const set1 = new Set();
  const set2 = new Set();
  const sub1 = (callback) => { set1.add(callback); return () => set1.delete(callback); };
  const sub2 = (callback) => { set2.add(callback); return () => set2.delete(callback); };
  panel.setBridge(h.auxChat, sub1);
  panel.bind('flip-a');
  assert.equal(set1.size, 1);
  panel.setBridge(h.auxChat, sub1);
  assert.equal(set1.size, 1, 'a same-bridge refresh must not double-subscribe');
  panel.setBridge(h.auxChat, sub2);
  assert.equal(set1.size, 0, 'the old subscription is released on a bridge flip');
  assert.equal(set2.size, 1, 'the new subscription is live');
  await h.flush();
  assert.equal(panel.view.auxId, 'aux-flip-a');
  // The live subscription still drives snapshot pulls.
  h.auxChat.snapshots.set('aux-flip-a', { chatItems: [], busy: true, queued: [] });
  for (const callback of [...set2]) callback();
  assert.equal(panel.view.snapshot.busy, true);
});
