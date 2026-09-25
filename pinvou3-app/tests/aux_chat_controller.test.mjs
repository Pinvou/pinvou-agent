/**
 * Aux-chat controller contract: the async state machine extracted from
 * AuxChatPanel.jsx (round-30 B1), driven through the interleavings that 29
 * review rounds hardened — send/restart guard ordering, the duplicate-send
 * lattice, ack-consumption classification, the discard stuck/recovery paths
 * and the watchdogs — with a fake bridge and fake timers, no React.
 */
import assert from 'node:assert/strict';
import test from 'node:test';
import {
  clearedIfSent,
  createAuxChatController,
  hasSendContent,
} from '../src/features/aux-chat/aux-chat-controller.mjs';
import {
  getAuxQuotes,
  stageAuxQuote,
} from '../src/features/aux-chat/aux-quote.mjs';

const SEND_WATCHDOG_MS = 1_000;
const DISCARD_WATCHDOG_MS = 1_000;
const ENSURE_WATCHDOG_MS = 1_000;

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

// Fake bridge: sends and discards return manually-settled deferreds (the
// interleavings under test live exactly in their settle ordering), while
// ensure auto-resolves one aux id per task (idempotent, like the real
// mapping) and snapshot reads a per-aux map the test writes to simulate
// turn_started / turn-terminal events landing in the buffer.
function createFakeAuxChat() {
  const calls = { send: [], discard: [], ensure: [] };
  const snapshots = new Map();
  return {
    calls,
    snapshots,
    send(auxId, text) {
      const d = deferred();
      calls.send.push({ auxId, text, ...d });
      return d.promise;
    },
    discard(taskId) {
      const d = deferred();
      calls.discard.push({ taskId, ...d });
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
    discardWatchdogMs: DISCARD_WATCHDOG_MS,
    ensureWatchdogMs: ENSURE_WATCHDOG_MS,
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

test('round-30 D1: a newer restart completing behind an orphaned stuck discard must not resurrect the survival marker — the kept ack keeps its draft', async () => {
  const h = createHarness();
  const panel1 = await mountBoundPanel(h, 'd1-task');
  panel1.setDraftText('recovery material');
  void panel1.send(); // sent at epoch 0, stays in flight through both restarts
  assert.equal(h.auxChat.calls.send.length, 1);
  // R1: the first restart issues discard d1 (epoch 1) and parks on it.
  void panel1.restart();
  void panel1.restart();
  assert.equal(h.auxChat.calls.discard.length, 1);
  // The panel remounts behind the pending discard: the rebind awaits d1 and
  // captures the pre-await epoch (round-30 D1).
  panel1.dispose();
  const panel2 = h.mountPanel('d1-task');
  await h.flush();
  // d1's settle-watchdog fires → stuck: the rebind stops awaiting it and the
  // stuck-escape re-arms New Topic (round-22 Major).
  h.timers.advance(DISCARD_WATCHDOG_MS);
  assert.equal(panel2.view.discardStuck, true, 'the remounted panel must mirror the stuck marker');
  // R2 enters through the stuck-escape (epoch 2), discards and re-ensures.
  void panel2.restart();
  void panel2.restart();
  assert.equal(h.auxChat.calls.discard.length, 2, 'the stuck-escape must issue a fresh discard');
  h.auxChat.calls.discard[1].resolve({});
  await h.flush();
  assert.equal(panel2.view.restarting, false, 'R2 completed: fresh binding restored');
  // The orphaned d1 rejects LATE — after R2 already completed. Its rebind
  // rejection arm runs with a stale epoch capture and must NOT re-add the
  // survival marker R2's entry cleared.
  h.auxChat.calls.discard[0].reject(new Error('orphaned stuck discard'));
  await h.flush();
  // The pre-restart send's ack now settles inside R2's window: without the
  // epoch gate the resurrected marker would misclassify this kept ack as
  // delivered and consumeSentDraft would delete the recovery draft.
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  assert.equal(
    panel2.view.draft,
    'recovery material',
    'the restart-preserved draft must survive the epoch-skipped ack (round-30 D1)',
  );
  // The store entry survives too — a task round trip restores it.
  panel2.bind('d1-other');
  await h.flush();
  panel2.bind('d1-task');
  await h.flush();
  assert.equal(panel2.view.draft, 'recovery material', 'the draft store must keep the recovery text');
});

test('round-30 D2: a stuck discard settling after a remount re-binds the panel (binding restored, banner gone)', async () => {
  const h = createHarness();
  const panel1 = await mountBoundPanel(h, 'd2-task');
  void panel1.restart();
  void panel1.restart();
  assert.equal(h.auxChat.calls.discard.length, 1);
  // The discard wedges, its watchdog marks it stuck, and the panel remounts:
  // the rebind skips ensure behind the stuck banner, so R1's generation is
  // dead and its own re-ensure is unreachable.
  h.timers.advance(DISCARD_WATCHDOG_MS);
  panel1.dispose();
  const panel2 = h.mountPanel('d2-task');
  await h.flush();
  assert.equal(panel2.view.discardStuck, true);
  assert.equal(panel2.view.auxId, null, 'the stuck rebind must not ensure past the orphan (round-23 MAJOR-2)');
  const ensuresBefore = h.auxChat.calls.ensure.length;
  // The orphaned discard finally settles. The settle clears entry and marker
  // before notifying, so the mounted panel sees "no discard in flight" and
  // re-runs its bind — the round-30 D2 recovery.
  h.auxChat.calls.discard[0].resolve({});
  await h.flush();
  assert.ok(
    h.auxChat.calls.ensure.length > ensuresBefore,
    'the settle must re-arm the bind effect — its ensure recovers the panel (round-30 D2)',
  );
  assert.equal(panel2.view.auxId, 'aux-d2-task', 'the binding is restored');
  assert.equal(panel2.view.discardStuck, false, 'the banner clears with the marker');
  assert.equal(panel2.view.bindingPending, false);
});

test('round-30 D2 rejection variant: a stuck discard rejecting after a remount recovers to the bound old transcript', async () => {
  const h = createHarness();
  const panel1 = await mountBoundPanel(h, 'd2-reject');
  void panel1.restart();
  void panel1.restart();
  h.timers.advance(DISCARD_WATCHDOG_MS);
  panel1.dispose();
  const panel2 = h.mountPanel('d2-reject');
  await h.flush();
  assert.equal(panel2.view.auxId, null);
  h.auxChat.calls.discard[0].reject(new Error('stuck discard rejected'));
  await h.flush();
  assert.equal(
    panel2.view.auxId,
    'aux-d2-reject',
    'the re-armed bind must re-ensure the surviving old transcript',
  );
  assert.equal(panel2.view.discardStuck, false);
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

test('ack consumption: a restart-window ack keeps the recovery draft and the failed-discard restore consumes exactly that marked kept ack', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'kept-task');
  panel.setDraftText('delivered before the restart');
  void panel.send();
  // The restart bumps the epoch and nulls the binding before the discard.
  void panel.restart();
  void panel.restart();
  // The ack settles inside the discard window: classified keep-draft, the
  // window is marked (round-26 MAJOR-2), the text stays as recovery material.
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  assert.equal(panel.view.draft, 'delivered before the restart', 'the epoch-skipped ack keeps the draft');
  // The discard FAILS: the old transcript survived, so the delivery reached a
  // live transcript — the restore fixup consumes the marked kept ack's draft.
  h.auxChat.calls.discard[0].reject(new Error('discard failed'));
  await h.flush();
  assert.equal(panel.view.discardFailed, true);
  assert.equal(panel.view.auxId, 'aux-kept-task', 'the restore re-binds the surviving session');
  assert.equal(
    panel.view.draft,
    '',
    'the delivered draft is consumed once the failed discard proves the delivery survived (round-25 MAJOR-24-3)',
  );
});

test('ack consumption: an unmarked restore never eats a re-typed or never-sent draft', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'unmarked-task');
  panel.setDraftText('same question');
  void panel.send();
  h.auxChat.calls.send[0].resolve({});
  await h.flush();
  assert.equal(panel.view.draft, '', 'the ordinary ack consumes the original');
  // An ordinary re-ask: the user re-types the identical text. No ack settles
  // inside the coming restart window, so the kept-ack marker is absent.
  panel.setDraftText('same question');
  void panel.restart();
  void panel.restart();
  h.auxChat.calls.discard[0].reject(new Error('discard failed'));
  await h.flush();
  assert.equal(
    panel.view.draft,
    'same question',
    'a draft re-typed after its ack was consumed carries no kept-ack marker and must survive (round-26 MAJOR-2)',
  );
  // A never-sent draft survives a failed restart the same way.
  panel.setDraftText('never sent');
  void panel.restart();
  void panel.restart();
  h.auxChat.calls.discard[1].reject(new Error('discard failed again'));
  await h.flush();
  assert.equal(panel.view.draft, 'never sent');
});

test('discard-stuck gating: Enter is rejected while restarting, while a discard is stuck, and New Topic is refused behind a healthy in-flight discard (round-24 N1)', async () => {
  const h = createHarness();
  const panel = await mountBoundPanel(h, 'gated-task');
  panel.setDraftText('blocked text');
  // Enter during restarting is rejected: the confirmed restart is about to
  // discard the session this send would land in.
  void panel.restart();
  void panel.restart();
  assert.equal(panel.view.restarting, true);
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 0, 'a send during restarting never dispatches');
  assert.equal(h.auxChat.calls.discard.length, 1);
  // A→B→A resets `restarting` but leaves the healthy discard in flight: New
  // Topic must refuse a second discard (and un-arm the confirm, round-12 N1).
  panel.bind('gated-other');
  await h.flush();
  panel.bind('gated-task');
  await h.flush();
  assert.equal(panel.view.restarting, false);
  void panel.restart();
  assert.equal(h.auxChat.calls.discard.length, 1, 'no second discard while a healthy one is in flight');
  assert.equal(panel.view.restartArmed, false, 'the refusal un-arms the confirm');
  void panel.restart();
  assert.equal(h.auxChat.calls.discard.length, 1, 'the guard runs before arming, so arming is refused too');
  // The discard wedges past its watchdog: sends are now refused behind the
  // stuck marker (round-23 MAJOR-2) and the stuck-escape re-arms New Topic.
  h.timers.advance(DISCARD_WATCHDOG_MS);
  assert.equal(panel.view.discardStuck, true);
  void panel.send();
  assert.equal(h.auxChat.calls.send.length, 0, 'a send while the discard is stuck never dispatches');
  void panel.restart();
  void panel.restart();
  assert.equal(h.auxChat.calls.discard.length, 2, 'the stuck-escape issues a fresh discard (round-22 Major)');
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

test('watchdog identity: a stale rejection after the failsafe stays silent and keeps the newer send\'s records', async () => {
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
  assert.equal(panel.view.draft, '', 'the newer send\'s own ack still consumes its draft (round-26 minor M1)');
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

test('clearedIfSent clears only a composer that still equals the sent text', () => {
  assert.equal(clearedIfSent('delivered', 'delivered'), '');
  assert.equal(clearedIfSent('delivered, edited', 'delivered'), 'delivered, edited');
  assert.equal(clearedIfSent('  delivered  ', 'delivered'), '');
});
