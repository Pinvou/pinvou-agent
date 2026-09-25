import {
  auxChatBusy,
  auxSnapshotsEqual,
  normalizeAuxSnapshot,
} from './aux-chat-state.mjs';
import {
  buildAuxQuoteBlock,
  dropAuxQuotes,
  getAuxQuotes,
  subscribeAuxQuotes,
} from './aux-quote.mjs';

// Re-exported so the panel adapter (already at its import-count lint ceiling)
// can drop its direct aux-quote import; the chip remove button is a plain
// store action, not controller state.
export { removeAuxQuote } from './aux-quote.mjs';

/**
 * Async controller for the aux chat panel — the pure, React-free state
 * machine AuxChatPanel.jsx used to carry inline (extracted in round-30 B1 so
 * the restart/latch/binding interleavings are drivable from plain node
 * tests; see tests/aux_chat_controller.test.mjs).
 *
 * Ownership split: this module owns every module-scoped registry, the send
 * path (entry guards, registry ownership, watchdog, ack consumption), the
 * restart path (entry guards, epoch bump, the single atomic reset call and
 * its settle bound) and the bind flow with its latch releases. The JSX keeps
 * rendering, scroll management, composer auto-grow and the
 * useState/useEffect wiring only.
 *
 * Restart is ONE backend command (`reset_aux_session`, M6): the backend
 * discards the old aux session through the turn gate and creates the fresh
 * one atomically, so there is no two-invoke window left to police — the
 * whole "stuck" family the two-invoke interleaving used to require (the
 * discard settle watchdog, the N1 guard, the stuck suppression in bind/send,
 * the stuck listeners and the re-entrant bind) is gone. What remains is what
 * stays genuinely needed: the restarting latch, the restart epoch (draft
 * preservation across a restart), the generation guards and the two-step
 * restartArmed confirm.
 *
 * Send-ack × restart truth table (why a late ack can never mis-consume a
 * draft):
 * - Ack rejects (before, during or after the reset): the dispatch was
 *   refused — nothing was delivered → the draft stays.
 * - Ack resolves before the reset completes: the delivery reached the old
 *   transcript, but the reset's delete half runs under the aux turn gate,
 *   which serializes against the turn the accepted dispatch started — so
 *   the delete strictly follows the delivery and destroys it with the old
 *   transcript → the draft stays as recovery material.
 * - A SUCCESSFUL ack landing after the reset completed is unreachable: the
 *   backend send path holds the same turn gate, so a dispatch queued behind
 *   the delete finds the session gone and rejects (the gate's "no queued
 *   sender resurrects the session" property).
 * - Reset rejects (delete half OR create half failed): the folded backend
 *   error cannot say which half ran, so the old transcript — and a delivery
 *   in it — MAY have survived. The controller deliberately keeps the draft
 *   here too: worst case the delivered message is visible in the surviving
 *   transcript while its text also sits in the composer (the user resends
 *   or deletes it). The alternative — consuming on this ambiguous path —
 *   could silently eat an unsent draft, which this module never does.
 * Net rule: an ack whose task restarted since its dispatch NEVER consumes
 * the draft; every other successful ack consumes exactly what it sent.
 *
 * createAuxChatController() is a factory so tests can drive a fresh instance
 * with a fake bridge and fake timers; the panel module instantiates exactly
 * one module-scope singleton, because the registries are module-scoped ON
 * PURPOSE (see the registry comments below): they track backend-scoped
 * operations that outlive any panel instance. Side effects (timers, the
 * quote store) are injectable and default to the real implementations.
 */

const RESTART_CONFIRM_MS = 4000;

// Settle bound for the atomic reset call (was ENSURE_WATCHDOG_MS over the
// two restart-stage ensures): the web lane's invoke transport rejects a
// pending invoke at 180 s on its own, while the desktop invoke has no
// transport timeout, so a wedged reset would otherwise latch
// restarting/bindingPending on the current task forever — a rebind or
// unmount recovers, but New Topic (the only in-panel recovery) is disabled
// while restarting. A timed-out reset surfaces as the reset-failure state
// (whose copy points at New Topic, re-enabled by the outer finally); the
// late resolution stays inert behind the generation checks.
const SETTLE_WATCHDOG_MS = 180_000;

// Same bound for the send registry's failsafe (round-23 should-fix 1): a
// turn_started that never becomes observable must not latch the composer and
// the send registry forever either.
const SEND_WATCHDOG_MS = 180_000;

// A quote-only send is valid — the excerpts alone are the question — so the
// dispatch needs either text or a captured quote block.
export const hasSendContent = (text, quoteBlock) => Boolean(text || quoteBlock);

// The visible composer clear only eats text that still equals the delivered
// message; anything typed since the dispatch belongs to the next one.
export const clearedIfSent = (current, text) => (current.trim() === text ? '' : current);

export function createAuxChatController(options = {}) {
  const {
    setTimeout: setTimeoutFn = setTimeout,
    clearTimeout: clearTimeoutFn = clearTimeout,
    buildQuoteBlock = buildAuxQuoteBlock,
    dropQuotes = dropAuxQuotes,
    getQuotes = getAuxQuotes,
    subscribeQuotes = subscribeAuxQuotes,
    sendWatchdogMs = SEND_WATCHDOG_MS,
    settleWatchdogMs = SETTLE_WATCHDOG_MS,
    restartConfirmMs = RESTART_CONFIRM_MS,
  } = options;

  const withSettleBound = (promise) => new Promise((resolve, reject) => {
    const timer = setTimeoutFn(() => reject(new Error('reset settle bound exceeded')), settleWatchdogMs);
    promise.then(
      (value) => { clearTimeoutFn(timer); resolve(value); },
      (error) => { clearTimeoutFn(timer); reject(error); },
    );
  });

  // taskId -> pending send promise, module-scoped on purpose: the send it
  // tracks is backend-scoped (the dispatch and the turn_started event lag
  // each other by a relay round trip), while the panel unmounts on close and
  // on sched- session switches. A component-level registry would die with
  // the instance and let a remounted panel double-dispatch into the same aux
  // session (round-15 MAJOR-2). Keyed by task — aux ids are 1:1 with tasks
  // and ensure is idempotent — and entries are removed by the exact send
  // that registered them once it settles.
  const sendInFlightByTask = new Map();

  // taskId -> pending atomic-reset promise (M6), module-scoped for the same
  // reason as the send registry: the backend reset can hold the aux turn
  // gate for seconds, outliving any panel instance. The bind flow awaits a
  // registered reset before re-ensuring the same task — otherwise its
  // ensure could return the old aux session the in-flight reset then deletes
  // behind the panel's back. One entry per task (the restart entry guard
  // refuses a second reset while one is registered — the N1 intent without
  // the stuck family: withSettleBound guarantees every registered reset
  // settles, so no entry can wedge the map), and entries are removed by the
  // exact restart that registered them once it settles.
  const resetInFlightByTask = new Map();

  // taskId -> restart epoch, module-scoped with the registries above: a send
  // dispatched before a restart must know, at ack time, whether a restart
  // for this task was initiated since the dispatch — the keep-draft skip in
  // send() keys on it (see the truth table in the module header).
  // restart() bumps the epoch in its synchronous entry block, before the
  // reset is issued, so every ack ordering inside and after the restart
  // window reads "restarted"; the count survives rebinds, closes and
  // remounts that a component-level flag could not. Entries leave through
  // purgeTask (the sessions domain reporting the task deleted).
  const restartEpochByTask = new Map();

  // taskId -> listener sets for the controller-state transition a mounted
  // panel must follow but that can originate on a DEAD instance (the panel
  // was closed and remounted while the transition ran — round-25
  // MAJOR-24-2): draft-store deletions (an ack consuming the entry the
  // mounted composer was restored from). Listeners are registered while a
  // panel is bound and removed by dispose()/re-bind, so a dead instance
  // never holds one; each listener re-checks the live task mirror before
  // touching state.
  const draftDeleteListenersByTask = new Map();

  const notifyTaskListeners = (listenersByTask, taskId) => {
    const listeners = listenersByTask.get(taskId);
    // Iterating the Set directly is deliberate: a listener unsubscribing
    // mid-notify (re-bind/dispose) is skipped for the remaining visits, which
    // is exactly the semantics an unmounted instance needs.
    if (listeners) for (const listener of listeners) listener();
  };

  const subscribeTaskListeners = (listenersByTask, taskId, listener) => {
    let listeners = listenersByTask.get(taskId);
    if (!listeners) {
      listeners = new Set();
      listenersByTask.set(taskId, listeners);
    }
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
      if (listeners.size === 0) listenersByTask.delete(taskId);
    };
  };

  // taskId -> unsent composer draft, module-scoped for the same reason as
  // the send registry above: a draft belongs to the task, not to this
  // instance, while the panel unmounts on close and on sched- switches.
  // Wiping it on a task switch, a panel close or a new-topic confirm
  // discarded text the user had typed and never sent — it is now only
  // cleared once a send actually succeeded (or the user deletes it).
  const draftByTask = new Map();

  const deleteDraftAndNotify = (taskId) => {
    draftByTask.delete(taskId);
    notifyTaskListeners(draftDeleteListenersByTask, taskId);
  };

  // Consume what the delivered send owned: the stored draft only when it
  // still equals the sent text (typing since belongs to the next message),
  // with the deletion broadcast to any mounted panel (round-25 MAJOR-24-2).
  const consumeSentDraft = (taskId, text) => {
    const storedDraft = draftByTask.get(taskId);
    if (storedDraft !== undefined && storedDraft.trim() !== text) return;
    deleteDraftAndNotify(taskId);
  };

  // Registry ownership helpers: a send's entry may only be removed by the
  // exact send that registered it (promise identity) — a stale settle must
  // not resurrect or double-clear anything (round-15 MAJOR-2, round-24
  // minor-9).
  const removeSendIfOwner = (taskId, sendPromise) => {
    if (sendInFlightByTask.get(taskId) === sendPromise) sendInFlightByTask.delete(taskId);
  };

  // Round-29 B1: the failsafe must also run after the invoke settles — the
  // coalesced-turn case it was built for (turn_started and the turn-terminal
  // events landing in one render batch, so busy is never observed) still
  // resolves the dispatch ack, so gating the callback on the registry entry
  // left exactly that case with zero recovery. The entry delete stays
  // identity-gated (only a never-settling invoke can still own it here); the
  // latch release is the callback's own responsibility, guarded there.
  const armSendWatchdog = (taskId, sendPromise, onFailsafe) => setTimeoutFn(() => {
    if (sendInFlightByTask.get(taskId) === sendPromise) sendInFlightByTask.delete(taskId);
    onFailsafe();
  }, sendWatchdogMs);

  /**
   * One mounted panel instance. Holds the per-instance state the async
   * continuations read (binding, generation, live task mirror, send latch)
   * and the renderable view the JSX mirrors into useState. Mutations emit a
   * shallow copy to subscribers, so React re-renders exactly where the old
   * setState calls did.
   */
  const createPanel = () => {
    const view = {
      sessionId: null,
      auxId: null,
      snapshot: normalizeAuxSnapshot(null),
      draft: '',
      quotes: [],
      sendFailed: false,
      ensureFailed: false,
      discardFailed: false,
      restartArmed: false,
      restarting: false,
      bindingPending: false,
      sending: false,
      sendInFlight: false,
    };
    // The current bridge and chat-domain subscription, refreshed by setBridge
    // (bridge.available flips at bootstrap, and the subscription effect keyed
    // on auxChat must re-run with it).
    let auxChat = null;
    let subscribeChat = null;
    let chatSubscription = null;
    // Per-instance async anchors: the generation stops late-arriving ensure
    // results from binding the panel back to a previous task, and the task
    // mirror tells a send settling after a task switch which task the panel
    // shows NOW (round-17 M-A). Both replace the component's refs verbatim.
    let generation = 0;
    let sessionIdMirror = null;
    // In-flight send latch: the bridge marks the session busy only when the
    // backend turn_started event lands, so snapshot-busy lags a dispatch by
    // the relay round trip — without this latch a double Enter fires a
    // duplicate turn and its rejection would surface as a bogus "send failed"
    // banner.
    let sendingLatch = false;
    let disposed = false;
    let quoteSubscription = null;
    let draftDeleteUnsubscribe = null;
    let restartArmTimer = null;
    const listeners = new Set();

    const emit = () => {
      // The sending hint must also cover the cross-switch in-flight window
      // (round-20 minor-4): a send on task A → switch to B → back to A resets
      // the panel-level sending flag (bind) while the registry still holds A's
      // send, so Enter no-ops at the registry guard — without the
      // registry-derived hint that window looked like a silently dead composer.
      view.sendInFlight = view.sending
        || !!(sessionIdMirror && sendInFlightByTask.has(sessionIdMirror));
      if (disposed) return;
      const copy = { ...view };
      for (const listener of listeners) listener(copy);
    };

    // Keep the old state when the snapshot is unchanged (the functional
    // setState returned the same value and React skipped the re-render),
    // blocking the useless re-pulls that the main session's streaming ticks
    // trigger through chat-domain notifies.
    const setSnapshot = (next) => {
      view.snapshot = next;
      // Send-latch release at turn_started, not at the dispatch ack (round-20
      // minor-4): the invoke resolving only means the backend accepted the
      // command — turn_started still lags it by one event round trip, so
      // releasing the latch in the send's finally re-opened the duplicate-send
      // window exactly where the latch claims coverage (fresh input there dies
      // at the backend turn gate as a misleading "send failed, retry" banner).
      // Hold the latch until snapshot-busy proves the backend took the turn;
      // the failure path releases it directly, and rebind/restart reset it as
      // before. (This is the busy-gated release effect: the view's sending
      // flag and the snapshot both live here now, so the check runs exactly
      // where either input changes.)
      if (view.sending && auxChatBusy(view.snapshot)) {
        sendingLatch = false;
        view.sending = false;
      }
      emit();
    };

    const pullSnapshot = (id) => {
      const raw = id && auxChat ? auxChat.snapshot(id) : null;
      const next = normalizeAuxSnapshot(raw);
      if (auxSnapshotsEqual(view.snapshot, next)) return;
      setSnapshot(next);
    };

    const clearRestartArmTimer = () => {
      if (restartArmTimer !== null) {
        clearTimeoutFn(restartArmTimer);
        restartArmTimer = null;
      }
    };

    // First open and main-session rebind: drop the old binding and
    // idempotently ensure the new task's aux session. The generation guard
    // stops a late-arriving ensure result from binding the panel back to the
    // previous task.
    const bind = (sessionId) => {
      generation += 1;
      const bindGen = generation;
      sessionIdMirror = sessionId;
      view.sessionId = sessionId;
      view.auxId = null;
      view.snapshot = normalizeAuxSnapshot(null);
      view.sendFailed = false;
      view.ensureFailed = false;
      view.discardFailed = false;
      clearRestartArmTimer();
      view.restartArmed = false;
      // Reset restarting on every rebind: the reset invoke has no transport
      // timeout of its own, so a promise that never settles would otherwise
      // latch the new task's panel disabled forever (the old restart's
      // finally can no longer be relied on once its generation went stale).
      view.restarting = false;
      // Same latch class for sends: a never-settling auxChat.send invoke must
      // not permanently block sends across later task rebinds either. Only the
      // panel-level latch resets here — the duplicate-send guard itself lives
      // in sendInFlightByTask, which survives the rebind by design.
      sendingLatch = false;
      view.sending = false;
      // Restore this task's unsent draft instead of wiping the composer.
      view.draft = sessionId ? (draftByTask.get(sessionId) || '') : '';
      view.quotes = sessionId ? getQuotes(sessionId) : [];
      view.bindingPending = !!(auxChat && sessionId);
      // Quotes staged while this panel is mounted (the selection popover runs
      // in the main view, not here) arrive through the store subscription; the
      // re-read on subscribe also closes the gap between mount and the first
      // stageAuxQuote call. (Was a standalone effect keyed on sessionId.)
      if (quoteSubscription) quoteSubscription();
      quoteSubscription = null;
      if (sessionId) {
        quoteSubscription = subscribeQuotes(sessionId, (next) => {
          view.quotes = next.map((quote) => ({ text: quote.text }));
          emit();
        });
      }
      // Follow draft-store deletions made by another instance (a send ack that
      // settled on a dead instance after close/reopen — round-25 MAJOR-24-2):
      // the composer was restored from the entry the ack consumed, so without
      // this mirror it keeps a delivered message staged for a duplicate send.
      // The delete only fires when the stored draft still equals the sent
      // text, and typing since would have re-populated the store before the
      // ack's delete check, so the clear never eats newer text (the composer
      // mirrors the store through setDraftText on the instance that owns it).
      if (draftDeleteUnsubscribe) draftDeleteUnsubscribe();
      draftDeleteUnsubscribe = null;
      if (sessionId) {
        draftDeleteUnsubscribe = subscribeTaskListeners(draftDeleteListenersByTask, sessionId, () => {
          if (sessionIdMirror !== sessionId) return;
          view.draft = '';
          emit();
        });
      }
      emit();
      if (!auxChat || !sessionId) return;
      const bindStale = () => disposed || generation !== bindGen;
      const ensureTask = () => {
        if (bindStale()) return;
        auxChat.ensure(sessionId)
          .then((nextAuxId) => {
            if (bindStale()) return;
            view.bindingPending = false;
            view.auxId = nextAuxId;
            pullSnapshot(nextAuxId);
            emit();
          })
          .catch((error) => {
            console.warn('[pinvou3][aux-chat] ensure failed', error);
            // When ensure fails the composer is disabled via the empty auxId,
            // but the reason is invisible; an inline hint tells the user the
            // initialization did not succeed instead of facing a dead panel.
            if (bindStale()) return;
            view.bindingPending = false;
            view.ensureFailed = true;
            emit();
          });
      };
      // An in-flight atomic reset for this same task must settle first (M6):
      // while its backend turn gate waits out a running turn, the old mapping
      // is still live — an ensure issued now would idempotently return the
      // doomed aux session, which the reset then deletes behind the panel's
      // back, leaving a dead binding. Await the in-flight promise either way:
      // on success the ensure returns the fresh session the reset just made;
      // on failure it re-binds the surviving old transcript or recreates the
      // session — the "re-ensure on next bind" recovery for a failed reset.
      const pendingReset = resetInFlightByTask.get(sessionId);
      if (pendingReset) pendingReset.then(ensureTask, ensureTask);
      else ensureTask();
    };

    // Background aux-session turn events already land in the per-session
    // buffer and trigger notifies; subscribing to the chat domain and
    // re-pulling the snapshot is enough — no extra event listener. The
    // re-pull doubles as the LRU touch of an always-open panel (snapshot()
    // refreshes recency): buffer capacity eviction relies on this
    // subscription being unconditionally delivered — if the subscription ever
    // gains a change gate, the touch needs another resident driver.
    const ensureChatSubscription = () => {
      if (chatSubscription || !auxChat || !subscribeChat) return;
      chatSubscription = subscribeChat(() => {
        if (view.auxId) pullSnapshot(view.auxId);
      });
    };

    // The JSX refreshes the bridge on every bind-effect run (bridge.available
    // flips once at bootstrap); the chat-domain subscription re-runs with it
    // (its old effect keyed on auxChat).
    const setBridge = (nextAuxChat, nextSubscribeChat) => {
      if (nextAuxChat !== auxChat || nextSubscribeChat !== subscribeChat) {
        if (chatSubscription) chatSubscription();
        chatSubscription = null;
      }
      auxChat = nextAuxChat || null;
      subscribeChat = nextSubscribeChat || null;
      ensureChatSubscription();
    };

    const setDraftText = (next) => {
      view.draft = next;
      // Remember per task: a task switch, a close/reopen or a new topic must
      // not silently drop text the user has not sent.
      if (sessionIdMirror) draftByTask.set(sessionIdMirror, next);
      if (view.sendFailed) view.sendFailed = false;
      emit();
    };

    // The send guards mirror restart(): the panel does not close on a main
    // task switch, so the binding may have changed while a send was in flight
    // — snapshot the auxId on entry and re-check before touching the UI, so
    // the old task's outcome (draft clear / failure banner) never lands on
    // the new task's panel. Also reject while restarting: between the restart
    // confirm and the reset completing, Enter must not submit into the old
    // session that is about to be discarded (the disabled composer only locks
    // the button, not this direct Enter path). sendingLatch closes the
    // remaining gap: snapshot-busy lags the dispatch by one event round trip,
    // so without the latch a double Enter fires a duplicate turn whose
    // backend rejection surfaces as a bogus "send failed, retry" banner while
    // the first reply is actually streaming.
    const send = async () => {
      const text = view.draft.trim();
      // Pending conversation quotes ride inline with the message as a fenced
      // userselect block: the engine sees plain message text (it has no
      // concept of quotes) while the timeline projection parses the block
      // back into chips. A quote-only send (empty draft) is valid — the
      // excerpts alone are a question about "what does this mean".
      const quotes = view.quotes;
      const quoteBlock = buildQuoteBlock(quotes);
      const sentAuxId = view.auxId;
      // Staleness boundary for both outcomes: the task id plus the restart
      // epoch (captured below). A restart on the same task re-binds to a *new*
      // aux, so a send issued before it must surface nothing at all — neither
      // the contradictory "retry send" banner next to the restart's own
      // state, nor a draft clear that would eat recovery material.
      const sentTaskId = sessionIdMirror;
      // The restart boundary for the keep-draft skip below: "a restart was
      // initiated for this task since this dispatch", not binding equality
      // (round-23 MAJOR-4).
      const sentEpoch = restartEpochByTask.get(sentTaskId) || 0;
      // Failsafe ownership token (round-29 B1): a rebind or restart resets
      // the send latch, so the watchdog may only release it while the
      // generation and binding captured here still own the panel.
      const sendGeneration = generation;
      if (!auxChat || !sentAuxId || !hasSendContent(text, quoteBlock)
        || auxChatBusy(view.snapshot) || view.restarting || sendingLatch) return;
      // The registry is the guard that survives rebinds: the bind flow resets
      // the send latch on every task switch, so a switch away and back while
      // this send is still in flight would otherwise re-open the
      // duplicate-send window (turn_started lags the dispatch by one event
      // round trip).
      if (sendInFlightByTask.has(sentTaskId)) return;
      sendingLatch = true;
      view.sending = true;
      view.sendFailed = false;
      emit();
      const sendPromise = auxChat.send(sentAuxId, quoteBlock ? text + quoteBlock : text);
      sendInFlightByTask.set(sentTaskId, sendPromise);
      // Send-latch failsafe (round-23 should-fix 1, round-29 B1): the
      // busy-gated latch release in setSnapshot only fires if a snapshot
      // update observes busy=true — when turn_started and the turn-terminal
      // events coalesce into one render batch (fast-failing turns, relay
      // event bursts), busy is never true and the latch and this registry
      // entry would stick with no recovery path. The timer is NOT cancelled
      // when the invoke settles: the coalesced case resolves the dispatch ack
      // like any other, so cancelling on settle left exactly the named case
      // dead (round-29 B1). At the bound (the web lane's invoke timeout) the
      // failsafe releases the latch only while this send still owns it —
      // same generation and binding, latch still held, and the snapshot not
      // busy (a busy snapshot means turn_started did land and the busy-gated
      // release owns the latch). A still-running turn makes the next dispatch
      // surface the backend's honest busy rejection, a finished one just
      // un-deads the composer; New Topic remains the recovery for the
      // never-settling invoke itself (whose registry entry the watchdog
      // deletes by identity inside armSendWatchdog).
      armSendWatchdog(sentTaskId, sendPromise, () => {
        if (!sendingLatch) return;
        if (generation !== sendGeneration || view.auxId !== sentAuxId) return;
        if (auxChatBusy(normalizeAuxSnapshot(auxChat.snapshot(sentAuxId)))) return;
        sendingLatch = false;
        view.sending = false;
        emit();
      });
      try {
        await sendPromise;
        // The keep-draft skip is the restart case only, keyed on the restart
        // epoch captured at dispatch (see the truth table in the module
        // header): the epoch lives in the controller exactly because it must
        // survive rebinds, closes and remounts, while view.auxId /
        // sessionIdMirror read per-instance state that a close/reopen freezes
        // at their last values. An ack settling under a changed epoch NEVER
        // consumes: the delivery — if it landed at all — was destroyed with
        // the discarded transcript, so the draft (and the staged quotes)
        // stays as recovery material.
        if ((restartEpochByTask.get(sentTaskId) || 0) !== sentEpoch) return;
        if (sentTaskId) {
          // Consume only what was actually sent: text typed after this send
          // started belongs to the next message, and quotes staged from the
          // main view during the in-flight window survive the success
          // callback (they were not part of the captured quote block).
          consumeSentDraft(sentTaskId, text);
          dropQuotes(sentTaskId, quotes);
        }
        // The visible composer clear is task-gated, not binding-gated
        // (round-24 Major): a send ack settling inside the same-task rebind's
        // ensure window reads view.auxId null, while the rebind's restore has
        // just re-filled the composer from draftByTask — a binding gate would
        // skip the only composer clear there and leave already-delivered text
        // staged for a duplicate Enter. On the same task and past the restart
        // skip above, clearing a composer that still equals the sent text is
        // safe whether the binding is live or mid-ensure: the functional
        // check never touches text typed since, and a panel showing another
        // task keeps its own draft untouched.
        const sameBinding = view.auxId === sentAuxId;
        if (sessionIdMirror !== sentTaskId) return;
        view.draft = clearedIfSent(view.draft, text);
        // The snapshot pull needs a live binding; inside the rebind window
        // the ensure resolution pulls the fresh snapshot instead.
        if (sameBinding) pullSnapshot(view.auxId);
        emit();
      } catch (error) {
        console.warn('[pinvou3][aux-chat] send failed', error);
        // Superseded outcomes must not manage newer state (round-24 minor-9):
        // a rejection settling after the watchdog fired (or a restart entry)
        // released this entry means either a newer send owns the latch — a
        // stale release would re-open the duplicate-send window — or the
        // outcome is long-stale; both stay silent, mirroring the identity
        // checks the watchdog and the finally already apply.
        // A plain rebind round trip (A→B→A, no restart) must still surface
        // the failure banner: the dispatch genuinely rejected and the panel
        // is back on this task — the old generation gate silenced that banner
        // too (round-24 minor-8). The restart case stays silent: the
        // restart-entry flow owns the panel state and keeps the draft as
        // recovery material (a "retry send" banner would contradict the
        // restart's own copy).
        if (sendInFlightByTask.get(sentTaskId) !== sendPromise
          || (restartEpochByTask.get(sentTaskId) || 0) !== sentEpoch
          || sessionIdMirror !== sentTaskId) return;
        view.sendFailed = true;
        // A failed dispatch never reaches turn_started, so the busy-gated
        // latch release in setSnapshot would never fire — release the latch
        // here or the composer stays locked behind the failure banner. The
        // gates above plus this binding check mean this is the exact send
        // that set the latch on the binding that still owns it (round-14 B2);
        // inside the rebind window the bind flow already reset the latch, so
        // skipping the release there is a no-op.
        if (view.auxId === sentAuxId) {
          sendingLatch = false;
          view.sending = false;
        }
        emit();
      } finally {
        // The watchdog is deliberately NOT cancelled here (round-29 B1): this
        // resolve is only the dispatch ack, and the coalesced turn-events case
        // the failsafe covers settles the same way — cancelling re-deadened
        // exactly that path. The fired failsafe no-ops once the latch has
        // been released by the busy-gated release or the failure path above.
        // The registry entry is removed by the exact send that registered it,
        // unconditionally — unlike the panel latch it must not depend on the
        // binding state, or a send settling after a rebind would leak the
        // entry and block this task's sends forever. The latch itself is NOT
        // released here (round-20 minor-4): this resolve is only the dispatch
        // ack and turn_started still lags it by one event round trip, so
        // releasing now would re-open the duplicate-send window exactly where
        // the latch claims coverage.
        removeSendIfOwner(sentTaskId, sendPromise);
        emit();
      }
    };

    // New topic: two-step lightweight confirm (the system window.confirm does
    // not pop under Tauri WebView2; the repo precedent is a self-drawn
    // confirm) → ONE atomic backend reset (M6: discard through the turn gate
    // + recreate in a single `reset_aux_session` command, one promise with
    // one outcome — no two-invoke window for an orphaned discard to destroy
    // the fresh session in) → rebind the fresh session. Success and failure
    // share **one** outer try/finally that resets restarting: no early return
    // (reset failure, generation mismatch) may leave the panel latched in the
    // restarting state — otherwise every button sits disabled under a
    // "please retry" hint.
    const restart = async () => {
      const sessionId = sessionIdMirror;
      if (!auxChat || !sessionId || view.restarting) return;
      // A reset for this task can still be in flight while `restarting` is
      // false: a task switch resets that latch (bind) but leaves the backend
      // turn gate holding the previous new-topic reset for seconds. Issuing a
      // second reset here would serialize behind the first one server-side
      // and delete the fresh session the first reset just created — the N1
      // intent, kept without the stuck family (the settle bound guarantees
      // every registered reset settles, so this guard can never wedge). The
      // bind flow already re-ensures this task once the pending reset
      // settles, which is precisely the fresh session this action asks for,
      // so stop here — but not silently: a remounted panel lost the armed
      // confirm, so un-arm here (the binding hint the rebind shows while the
      // reset is in flight is the visible "new topic in preparation" feedback
      // for this window).
      if (resetInFlightByTask.has(sessionId)) {
        clearRestartArmTimer();
        view.restartArmed = false;
        emit();
        return;
      }
      if (!view.restartArmed) {
        view.restartArmed = true;
        clearRestartArmTimer();
        restartArmTimer = setTimeoutFn(() => {
          restartArmTimer = null;
          view.restartArmed = false;
          emit();
        }, restartConfirmMs);
        emit();
        return;
      }
      clearRestartArmTimer();
      view.restartArmed = false;
      view.restarting = true;
      view.discardFailed = false;
      // A failed send's banner must not survive into the restart: when the
      // reset fails, the binding stays cleared and the composer disabled —
      // showing "send failed, retry" next to the reset failure is a
      // contradictory double banner. ensureFailed is the same class (a stale
      // init failure next to the fresh restart outcome).
      view.sendFailed = false;
      view.ensureFailed = false;
      // Release the send latch at restart entry (round-12 N2): a send issued
      // for the old binding can settle after this restart re-bound view.auxId,
      // and its finally deliberately refuses to clear the latch once the
      // binding moved (see send()). Only the bind flow resets it otherwise,
      // and a same-task restart does not re-run that flow — so without this
      // reset a late-settling send would leave sendingLatch stuck true and
      // every later Enter would silently no-op behind a visually enabled
      // composer.
      sendingLatch = false;
      view.sending = false;
      emit();
      // The send registry entry is deliberately KEPT through the restart
      // (round-25 MAJOR-24-3, reshaping the round-16 B1 clear): the round-23
      // SEND_WATCHDOG_MS failsafe provides the never-settling recovery the
      // entry-delete served, and a pending ack surviving the restart is what
      // the keep-draft classification keys on. While the entry stands, new
      // sends on this task wait at the registry guard with the sending hint
      // visible; the entry leaves through the send's own identity-checked
      // finally or the watchdog, nothing else.
      // Null the binding at restart entry (round-18 B-1): a send settling
      // inside the reset window must read as the restart case — otherwise
      // its success continuation still sees the old aux id, consumes the
      // draft and staged quotes as "delivered", and the reset then destroys
      // both the transcript and the recovery material. With the binding
      // nulled (same reset as the bind flow), every late settle takes the
      // keep-draft skip; the reset resolution writes the fresh id back.
      view.auxId = null;
      // Feedback for the reset window (the backend turn gate can hold the
      // delete half for seconds, and `restarting` only disables controls):
      // the composer and timeline would otherwise just sit there with no
      // hint that a new topic is being prepared.
      view.bindingPending = true;
      // Clear the snapshot at entry too (round-20 minor-5), mirroring the
      // bind flow: restart otherwise kept the old transcript rendered through
      // the whole reset window, so hasContent stayed true — and the
      // bindingPending hint renders only in the !hasContent branch, leaving a
      // stale timeline, disabled controls and a stale busyHint with no
      // "preparing" feedback.
      setSnapshot(normalizeAuxSnapshot(null));
      // Bump the generation at restart entry: only the bind flow increments
      // it otherwise, so an ensure issued by the current bind that is still
      // in flight would resolve after this restart's reset with a matching
      // generation and rebind the panel to the just-discarded aux session.
      // Advancing the generation here makes every such stale continuation
      // inert.
      generation += 1;
      const restartGeneration = generation;
      // Restart epoch (round-23 MAJOR-4): bumped in the synchronous restart-
      // entry block, before the reset is issued, so every ack continuation —
      // which can only run once this block yields — reads "restarted".
      // Controller-scoped with the registries so the signal survives rebinds
      // and remounts.
      restartEpochByTask.set(sessionId, (restartEpochByTask.get(sessionId) || 0) + 1);
      // Register the in-flight reset by task id BEFORE awaiting it: while
      // its backend turn gate waits out a running turn, the old mapping is
      // still live, and the bind flow must await this promise before
      // re-ensuring the same task — otherwise it would bind the doomed aux
      // session that this reset deletes behind its back.
      const resetPromise = withSettleBound(auxChat.reset(sessionId));
      resetInFlightByTask.set(sessionId, resetPromise);
      try {
        try {
          const nextAuxId = await resetPromise;
          // The binding may have changed during the reset round trip: never
          // bind the fresh session onto a panel that has moved on.
          if (generation !== restartGeneration) return;
          view.auxId = nextAuxId;
          setSnapshot(normalizeAuxSnapshot(nextAuxId ? auxChat.snapshot(nextAuxId) : null));
          // The draft deliberately survives a new topic: the text was typed
          // but never sent, so discarding it would silently eat user input.
          view.sendFailed = false;
          view.ensureFailed = false;
          emit();
        } catch (error) {
          console.warn('[pinvou3][aux-chat] restart reset failed', error);
          if (generation !== restartGeneration) return;
          // Honest failure surface (M6): the folded backend error cannot say
          // whether the delete half ran, so the safest assumption is "the old
          // transcript was possibly discarded". The binding stays cleared and
          // the composer honestly disabled (no restore of a session that may
          // not exist); the discardFailed copy says exactly this. Recovery is
          // the next bind — any rebind re-ensures, and get-or-create rebinds
          // the old transcript when it survived or creates a fresh session —
          // or a New Topic retry, which the outer finally re-enables.
          view.auxId = null;
          setSnapshot(normalizeAuxSnapshot(null));
          view.discardFailed = true;
          emit();
        }
      } finally {
        // Entry removal is identity-gated like the send registry's: a stale
        // restart continuation must not clear a newer reset's entry.
        if (resetInFlightByTask.get(sessionId) === resetPromise) {
          resetInFlightByTask.delete(sessionId);
        }
        // A stale continuation must not clear a newer restart's latch:
        // without the generation check, R1's finally would re-enable the
        // composer and the new-topic button while R2's reset is still in
        // flight, letting a send slip into the session being reset. The bind
        // flow resets the flag itself, so skipping the reset here cannot leak
        // the state.
        if (generation === restartGeneration) {
          view.restarting = false;
          view.bindingPending = false;
          emit();
        }
      }
    };

    const dispose = () => {
      disposed = true;
      if (chatSubscription) chatSubscription();
      chatSubscription = null;
      if (quoteSubscription) quoteSubscription();
      quoteSubscription = null;
      if (draftDeleteUnsubscribe) draftDeleteUnsubscribe();
      draftDeleteUnsubscribe = null;
      clearRestartArmTimer();
      listeners.clear();
    };

    return {
      view,
      bind,
      setBridge,
      setDraftText,
      send,
      restart,
      dispose,
      subscribe(listener) {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
    };
  };

  return { createPanel };
}
