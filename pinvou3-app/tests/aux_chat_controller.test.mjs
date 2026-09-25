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
  return { timers, auxChat, controller, mountPanel, notifyChat, flush };
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

test('restart gating: Enter is rejected while restarting, and New Topic is refused behind a healthy in-flight reset (the N1 intent)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'gated-task');
  panel.setDraftText('blocked text');
  // Enter during restarting is rejected: the confirmed restart is about to
  // reset the session this send would land in.
  confirmRestart(panel);
  assert.equal(panel.view.restarting, true);
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 0, 'a send during restarting never dispatches');
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
