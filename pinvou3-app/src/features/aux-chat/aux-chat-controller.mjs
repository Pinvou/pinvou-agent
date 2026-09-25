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
 * restart path (entry guards, epoch bump, discard→ensure chain, stuck
 * watchdog, settle notify, binding-retry recovery), the bind flow and the
 * latch release. The JSX keeps rendering, scroll management, composer
 * auto-grow and the useState/useEffect wiring only.
 *
 * createAuxChatController() is a factory so tests can drive a fresh instance
 * with a fake bridge and fake timers; the panel module instantiates exactly
 * one module-scope singleton, because the registries are module-scoped ON
 * PURPOSE (see the registry comments below): they track backend-scoped
 * operations that outlive any panel instance. Side effects (timers, the
 * quote store) are injectable and default to the real implementations.
 */

const RESTART_CONFIRM_MS = 4000;

// Mirrors the web lane's invoke timeout (web/bootstrap.js rejects a pending
// invoke at 180 s): there a wedged discard settles on its own as a rejection,
// while the desktop invoke has no transport timeout, so a never-settling
// discard would otherwise keep its registry entry — and every guard keyed on
// it — forever. The threat model treats hung invokes as real, so the discard
// registry gets the same never-settling recovery the send registry got in
// round-16 B1, just with a different shape (see the watchdog in restart():
// deleting the entry would re-open the N1 race, so it is marked stuck instead).
const DISCARD_WATCHDOG_MS = 180_000;

// Same bound for the send registry's failsafe (round-23 should-fix 1): a
// turn_started that never becomes observable must not latch the composer and
// the send registry forever either.
const SEND_WATCHDOG_MS = 180_000;

// Same bound for the two restart-stage ensures (round-25 minor): a hung
// ensure would otherwise latch restarting/bindingPending on the current task
// forever — a rebind or unmount recovers, but New Topic (the only in-panel
// recovery) is disabled while restarting. A timed-out ensure surfaces as the
// ensure-failure state (whose copy points at New Topic, re-enabled by the
// outer finally); the late resolution stays inert behind the generation
// checks.
const ENSURE_WATCHDOG_MS = 180_000;

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
    discardWatchdogMs = DISCARD_WATCHDOG_MS,
    ensureWatchdogMs = ENSURE_WATCHDOG_MS,
    restartConfirmMs = RESTART_CONFIRM_MS,
  } = options;

  const withSettleBound = (promise) => new Promise((resolve, reject) => {
    const timer = setTimeoutFn(() => reject(new Error('ensure settle bound exceeded')), ensureWatchdogMs);
    promise.then(
      (value) => { clearTimeoutFn(timer); resolve(value); },
      (error) => { clearTimeoutFn(timer); reject(error); },
    );
  });

  // taskId -> pending discard promise, module-scoped on purpose: the discard it
  // tracks is backend-scoped (the turn gate can hold it for seconds), while the
  // panel unmounts on close and on sched- session switches. A component-level
  // registry would die with the instance and let a remounted panel rebind to
  // the still-mapped aux session the in-flight discard then deletes — the exact
  // M-B hole re-opened through unmount/remount. Entries are removed when the
  // discard settles (or survive, marked stuck, past the settle-watchdog), so
  // the map holds at most one pending promise per task.
  const discardInFlightByTask = new Map();

  // taskId set of registry entries whose settle-watchdog fired (round-22
  // Major), module-scoped with the registry it annotates. A stuck entry stays
  // registered — the orphaned discard can still settle, and its late
  // continuations must keep finding their promise here for the identity checks
  // — but it is no longer awaited by the bind flow and no longer blocks New
  // Topic: awaiting/refusing forever was the dead end (eternal "preparing"
  // hint, dead composer, N1 guard refusing every restart until an app reload).
  // A task's marker is cleared when the owning discard settles or when a
  // re-armed restart registers a fresh discard for that task.
  // Disposition while the marker stands (round-23 MAJOR-2): the panel neither
  // re-binds (the bind flow skips ensure) nor sends — the orphaned discard
  // COMMAND can still execute server-side, where it deletes whatever aux
  // session is mapped for this task at execution time, so nothing new may be
  // put in front of it. The re-armed New Topic is the recovery: it re-issues a
  // fresh discard, awaits it, and only then re-ensures.
  const discardStuckByTask = new Set();

  // taskId -> pending send promise, module-scoped for the same reason as the
  // discard registry above: the duplicate-send window outlives the component
  // instance. The bind flow resets the panel-level send latch on every task
  // switch (a never-settling invoke must not latch the next task), so a send on
  // task A → switch to B → back to A would otherwise pass every guard before
  // turn_started lands in the buffer, firing a duplicate turn on the same aux
  // session (round-15 MAJOR-2). Keyed by task — aux ids are 1:1 with tasks and
  // ensure is idempotent — and entries are removed by the exact send that
  // registered them once it settles.
  const sendInFlightByTask = new Map();

  // taskId -> restart epoch, module-scoped with the registries above: a send
  // dispatched before a restart must know, at ack time, whether a restart for
  // this task was initiated since the dispatch — the keep-draft skip in send()
  // is the restart case only (the delivery was destroyed with the discarded
  // transcript, so the text stays as recovery material), while a same-task
  // rebind re-ensures the SAME live aux session and an ack landing inside its
  // null-binding window must consume the draft (the delivery reached a live
  // transcript; keeping it restores already-delivered text for a duplicate
  // send, round-23 MAJOR-4). restart() bumps the epoch in its synchronous
  // entry block, before the discard is issued, so every ack ordering inside
  // and after the restart window reads "restarted"; the count survives
  // rebinds, closes and remounts that a component-level flag could not.
  const restartEpochByTask = new Map();

  // taskId -> the exact text captured at the last dispatch, module-scoped with
  // the registries: the failed-discard restore fixup (round-25 MAJOR-24-3)
  // needs the sent text to consume the delivered draft precisely, and the
  // restore can run after the ack has already settled — when the text is not
  // otherwise available. Overwritten by the next dispatch on the same task;
  // joins the registered per-task map sweep.
  const sentTextByTask = new Map();

  // taskId -> the quote capture (array reference) of the last dispatch, the
  // quote twin of sentTextByTask (round-26 minor M2): an ack landing
  // epoch-skipped inside the discard window deliberately skips dropAuxQuotes
  // (the quotes stay staged as recovery material while the discard's outcome
  // is unknown), so the failed-discard restore fixup — the point where the
  // delivery is finally known to have survived — must drop exactly that
  // capture, or the delivered send's excerpts attach again to the next
  // message. Same lifecycle as the sent text: overwritten by the next
  // dispatch, cleared on failure by capture identity.
  const sentQuotesByTask = new Map();

  // taskId set marking that the task's LATEST restart failed its discard and
  // restored the SAME live aux session (round-25 MAJOR-24-3): the keep-draft
  // premise "the delivery was destroyed with the discarded transcript" is then
  // false, and an ack settling under this marker must fall through to normal
  // consumption. Set when the failed-discard restore re-binds, cleared at
  // restart entry (a fresh discard destroys the transcript unless it fails
  // again) and single-shot when an ack consumes under it. "Latest" is enforced
  // at every add site by the restart-epoch gate (round-30 D1): the round-22
  // stuck-escape lets a newer restart enter (and clear the marker) BEFORE an
  // orphaned older discard's rejection continuation runs, so an ungated add
  // would resurrect the marker against a transcript the newer restart already
  // destroyed and misclassify the fresh window's next kept ack as delivered,
  // deleting the draft from both transcript and composer.
  const restartDiscardFailedByTask = new Set();

  // taskId set: an ack settled inside this task's CURRENT restart window and
  // was classified keep-draft (round-26 MAJOR-2). The failed-discard restore
  // fixup may consume the staged draft only under this marker — without it the
  // fixup matched ANY stored draft trim-equal to the last sent text, so a draft
  // the user re-typed verbatim after the ack consumed the original (an ordinary
  // re-ask), or recovery material a successful restart deliberately kept, was
  // silently deleted by a later failed-discard restore whose window never
  // contained that delivery. Set by the epoch-skipped keep path of the send
  // ack, cleared at restart entry (a fresh window reclassifies from scratch)
  // and consumed single-shot by the fixup.
  const restartWindowKeptAckByTask = new Set();

  // taskId -> listener sets for the two controller-state transitions a mounted
  // panel must follow but that can originate on a DEAD instance (the panel was
  // closed and remounted while the transition ran — round-25 MAJOR-24-2 and
  // should-fix-24-2): draft-store deletions (an ack consuming the entry the
  // mounted composer was restored from) and stuck-marker changes (a watchdog
  // firing, or a pending discard settling, behind the remount). Listeners are
  // registered while a panel is bound and removed by dispose()/re-bind, so a
  // dead instance never holds one; each listener re-checks the live task mirror
  // before touching state.
  const draftDeleteListenersByTask = new Map();
  const discardStuckListenersByTask = new Map();

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

  // taskId -> unsent composer draft, module-scoped for the same reason as the
  // discard registry above: a draft belongs to the task, not to this instance,
  // while the panel unmounts on close and on sched- switches. Wiping it on a
  // task switch, a panel close or a new-topic confirm discarded text the user
  // had typed and never sent — it is now only cleared once a send actually
  // succeeded (or the user deletes it).
  const draftByTask = new Map();

  const deleteDraftAndNotify = (taskId) => {
    draftByTask.delete(taskId);
    notifyTaskListeners(draftDeleteListenersByTask, taskId);
  };

  // The keep-draft decision of a send ack, controller state only (round-25
  // MAJOR-24-1): "a restart was initiated for this task since the dispatch".
  // The single-shot survival marker (round-25 MAJOR-24-3) falls an ack through
  // when the restart's discard failed and the same live transcript was
  // restored — there the delivery survived and the draft must consume.
  const restartKeptDraft = (taskId, sentEpoch) => {
    if ((restartEpochByTask.get(taskId) || 0) === sentEpoch) return false;
    return !restartDiscardFailedByTask.delete(taskId);
  };

  // Consume what the delivered send owned: the stored draft only when it still
  // equals the sent text (typing since belongs to the next message), with the
  // deletion broadcast to any mounted panel (round-25 MAJOR-24-2).
  const consumeSentDraft = (taskId, text) => {
    const storedDraft = draftByTask.get(taskId);
    if (storedDraft !== undefined && storedDraft.trim() !== text) return;
    deleteDraftAndNotify(taskId);
  };

  // The failed-discard restore fixup (round-25 MAJOR-24-3): with no ack
  // pending there is no later classification, so the delivered draft is
  // consumed here — precisely, only when an ack settled inside THIS restart
  // window and was classified keep-draft (the round-26 MAJOR-2 gate: a draft
  // re-typed after the original was consumed, or recovery material an earlier
  // successful restart kept, carries no such marker and must survive), and
  // only while the stored draft still equals the recorded sent text (typing
  // between the dispatch and the restart entry belongs to the next message).
  // Quotes staged mid-flight were never captured by the send and stay; a hung
  // ack's captured quotes stay staged indefinitely — its 180 s watchdog
  // releases the registry entry and the latch, not the sentQuotesByTask/
  // sentTextByTask records, which only an ack continuation identity-deletes
  // (round-28 minor N1: the earlier "bounded by the watchdog" phrasing was
  // wrong).
  const consumeDeliveredDraftAfterFailedRestart = (sessionId) => {
    if (sendInFlightByTask.has(sessionId)) return;
    if (!restartWindowKeptAckByTask.delete(sessionId)) return;
    // The kept ack's delivery survived in this restored transcript, so its
    // captured quotes were delivered with it — drop exactly that capture
    // (round-26 minor M2); excerpts staged from the main view during the
    // in-flight window were never part of it and stay.
    const sentQuotes = sentQuotesByTask.get(sessionId);
    if (sentQuotes) dropQuotes(sessionId, sentQuotes);
    const sentText = sentTextByTask.get(sessionId);
    const storedDraft = draftByTask.get(sessionId);
    if (sentText !== undefined && storedDraft !== undefined
      && storedDraft.trim() === sentText.trim()) {
      deleteDraftAndNotify(sessionId);
    }
  };

  // Registry ownership helpers: a send's entry may only be removed by the
  // exact send that registered it (promise identity) — a stale settle must not
  // resurrect or double-clear anything (round-15 MAJOR-2, round-24 minor-9).
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
      discardStuck: false,
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
    let stuckUnsubscribe = null;
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
    // previous task. A re-bind is also the round-30 D2 recovery: the
    // stuck-notify listener re-runs this when a stuck discard settles with
    // nothing left to re-bind the panel — see the listener below.
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
      // Mirror the task's stuck marker (the watchdog may have fired while this
      // panel was unmounted; a switch from another task must not inherit or
      // keep that task's banner).
      view.discardStuck = !!(sessionId && discardStuckByTask.has(sessionId));
      // Reset restarting on every rebind: the two restart invokes have no
      // transport timeout, so a promise that never settles would otherwise
      // latch the new task's panel disabled forever (the old restart's finally
      // can no longer be relied on once its generation went stale).
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
      // Re-mirror the task's stuck marker on controller-state transitions
      // (round-25 should-fix-24-2): the settle-watchdog can fire, or a pending
      // discard can settle, while this panel was closed and remounted — the
      // transition then runs on the dead instance (whose state writes are
      // lost) and the remounted panel would sit in the eternal "preparing"
      // state with no banner until a task switch. Transitions notify this
      // listener, which re-mirrors the marker and, on stuck, applies the
      // watchdog's latch releases the dead instance could no longer deliver.
      if (stuckUnsubscribe) stuckUnsubscribe();
      stuckUnsubscribe = null;
      if (sessionId) {
        stuckUnsubscribe = subscribeTaskListeners(discardStuckListenersByTask, sessionId, () => {
          if (sessionIdMirror !== sessionId) return;
          const stuck = discardStuckByTask.has(sessionId);
          view.discardStuck = stuck;
          if (stuck) {
            view.restarting = false;
            view.bindingPending = false;
            emit();
            return;
          }
          // Round-30 D2: the marker was just CLEARED. A re-armed restart
          // clears it right after registering its fresh discard and owns the
          // re-ensure itself — but when the notification is the orphaned
          // stuck discard's late SETTLE, the restart's own re-ensure is
          // unreachable whenever a rebind/remount intervened (its generation
          // went stale at the post-discard gate, or its instance is dead),
          // while the settle finally just removed the one banner that told
          // the user how to recover. The panel would end with a null binding,
          // no banner and an emptyState inviting an Enter that silently
          // no-ops. The settle deletes the registry entry before clearing the
          // marker and notifying, so "no discard in flight" here means
          // exactly that case: re-run the bind, whose ensure (or its honest
          // ensureFailed surface) recovers the panel. (Was the
          // bindingRetryTick token re-running the bind effect.)
          if (!discardInFlightByTask.has(sessionId)) {
            bind(sessionId);
            return;
          }
          emit();
        });
      }
      emit();
      if (!auxChat || !sessionId) return;
      // An in-flight discard for this same task must settle first: its
      // backend turn gate waits out the running turn (seconds), and while it
      // is pending the old mapping is still live — an ensure issued now would
      // idempotently return the doomed aux session, which the discard then
      // deletes behind the panel's back, leaving a dead binding. Await the
      // in-flight promise (rejections surface below and on the restart path)
      // and re-check the generation so a further rebind during the wait aborts
      // this ensure entirely. A stuck entry (round-22 Major: its
      // settle-watchdog fired, so it may never settle) is neither awaited nor
      // ensured at all (round-23 MAJOR-2): the orphaned discard command can
      // still execute server-side against the current mapping, so binding or
      // sending now would put a live transcript in front of it. The panel
      // stays unbound behind the stuck banner; the re-armed New Topic is the
      // recovery (fresh discard, awaited, before its ensure).
      const pendingDiscard = discardInFlightByTask.get(sessionId);
      // The restart that issued the awaited discard bumped the task's restart
      // epoch before dispatching it; the rejection arm below compares against
      // this capture to tell "the task's LATEST restart failed its discard"
      // from a stale orphan (round-30 D1, see restartDiscardFailedByTask).
      const awaitedDiscardEpoch = restartEpochByTask.get(sessionId) || 0;
      const bindStale = () => disposed || generation !== bindGen;
      const ensureAfterDiscard = () => {
        if (bindStale()) return Promise.resolve();
        // The chain is returned so the awaited-discard rejection arm can run
        // its post-restore fixup after the binding settles (round-28 B1); the
        // fire-and-forget callers ignore it.
        return auxChat.ensure(sessionId)
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
      if (discardStuckByTask.has(sessionId)) {
        // Stuck: skip ensure and drop the "preparing" hint — the stuck banner
        // (mirrored above) is the state the panel shows, and the composer
        // stays disabled behind the null binding until New Topic recovers.
        view.bindingPending = false;
        emit();
      } else if (pendingDiscard) {
        pendingDiscard.then(ensureAfterDiscard, (error) => {
          // The awaited discard was issued by a restart that may have run on
          // another instance whose catch is generation-gated — after a task
          // round trip that catch returns silently (round-25 minor), and this
          // rebind would then re-bind the SURVIVING old transcript with no
          // word that the requested new topic failed. Surface discardFailed
          // here too: the copy (current topic still usable, New Topic can be
          // retried) is exactly the state this rejection leaves behind.
          console.warn('[pinvou3][aux-chat] awaited discard failed on rebind', error);
          if (!bindStale()) {
            view.discardFailed = true;
            emit();
          }
          // Bookkeeping parity with the restart's own discard-failure catch
          // (round-28 B1, the round-27 Major α): the transcript SURVIVED this
          // failed discard, so the keep-draft premise no longer holds for an
          // ack settling under this window, and a kept ack's delivered draft
          // must be consumed by the restore fixup — omitting either here
          // re-opened the duplicate-send class through the rebind arm. The
          // marker is task-level state, so it is added even when this
          // instance's generation went stale — but only while the epoch
          // captured before the await still stands (round-30 D1): the
          // round-22 stuck-escape lets a newer restart enter and COMPLETE
          // before this orphaned discard's rejection lands, and the newer
          // restart's entry already cleared the marker — an ungated re-add
          // would misclassify the fresh window's next kept ack as delivered
          // and delete the draft the restart kept as recovery material. The
          // fixup needs no such gate of its own: it is chained behind this
          // arm's ensure and generation-gated like the bind (a newer restart
          // on this instance bumped the generation; on another instance this
          // one is disposed).
          if ((restartEpochByTask.get(sessionId) || 0) === awaitedDiscardEpoch) {
            restartDiscardFailedByTask.add(sessionId);
          }
          Promise.resolve(ensureAfterDiscard()).then(
            () => {
              if (!bindStale()) consumeDeliveredDraftAfterFailedRestart(sessionId);
            },
            () => {},
          );
        });
      } else {
        ensureAfterDiscard();
      }
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
    // confirm and the discard completing, Enter must not submit into the old
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
      // A stuck discard for this task may still execute server-side (round-23
      // MAJOR-2): it deletes whatever aux session is mapped at execution time,
      // so a message sent now could be destroyed with the session it landed
      // in. The stuck banner is the visible state; the re-armed New Topic
      // (fresh awaited discard) is the recovery.
      if (discardStuckByTask.has(sentTaskId)) return;
      sendingLatch = true;
      view.sending = true;
      view.sendFailed = false;
      emit();
      const sendPromise = auxChat.send(sentAuxId, quoteBlock ? text + quoteBlock : text);
      sendInFlightByTask.set(sentTaskId, sendPromise);
      // Record the exact sent text for the failed-discard restore fixup
      // (round-25 MAJOR-24-3): the restore can run after this ack already
      // settled, so the delivered text must be recoverable without the
      // closure. Overwritten by the next dispatch; a failed send clears it
      // below (a failed dispatch delivered nothing). The captured quotes ride
      // alongside (round-26 minor M2): the epoch-skipped ack path deliberately
      // keeps them staged while the discard outcome is unknown, so the
      // restore fixup needs the capture identity to drop them when the
      // delivery is known to have survived.
      sentTextByTask.set(sentTaskId, text);
      sentQuotesByTask.set(sentTaskId, quotes);
      // Send-latch failsafe (round-23 should-fix 1, round-29 B1), the
      // send-side twin of the discard watchdog: the busy-gated latch release
      // in setSnapshot only fires if a snapshot update observes busy=true —
      // when turn_started and the turn-terminal events coalesce into one
      // render batch (fast-failing turns, relay event bursts), busy is never
      // true and the latch and this registry entry would stick with no
      // recovery path. The timer is NOT cancelled when the invoke settles:
      // the coalesced case resolves the dispatch ack like any other, so
      // cancelling on settle left exactly the named case dead (round-29 B1).
      // At the bound (the web lane's invoke timeout, same as the discard
      // watchdog) the failsafe releases the latch only while this send still
      // owns it — same generation and binding, latch still held, and the
      // snapshot not busy (a busy snapshot means turn_started did land and
      // the busy-gated release owns the latch). A still-running turn makes
      // the next dispatch surface the backend's honest busy rejection, a
      // finished one just un-deads the composer; New Topic remains the
      // recovery for the never-settling invoke itself (whose registry entry
      // the watchdog deletes by identity inside armSendWatchdog).
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
        // Same binding (aux ids are 1:1 with tasks and ensure is idempotent):
        // the visible composer shows this task and the message was delivered —
        // consume, even across a generation bump (round-14 B3). Same task but
        // a *different* binding is the restart case: the old aux was
        // discarded, so the delivery is gone and the draft stays as recovery
        // material. Anything else means the panel moved to another task while
        // this send settled into A's *live* transcript — the store maps must
        // still be consumed, or returning to A restores already-delivered
        // text and staged quotes for a duplicate send (round-17 M-A).
        const sameBinding = view.auxId === sentAuxId;
        const onSameTask = sessionIdMirror === sentTaskId;
        // The keep-draft skip is the restart case only, keyed on the restart
        // epoch captured at dispatch — and consulted UNCONDITIONALLY via the
        // controller-state helper (round-25 MAJOR-24-1): the epoch lives in
        // the controller exactly because it must survive rebinds, closes and
        // remounts, while sameBinding/onSameTask read per-instance state that
        // a close/reopen freezes at their last values — a frozen
        // view.auxId === sentAuxId read as sameBinding true on the dead
        // instance and short-circuited this skip, so the ack deleted the
        // draft a restart had deliberately preserved as recovery material.
        // Binding equality stays meaningful only for the visible half below.
        if (restartKeptDraft(sentTaskId, sentEpoch)) {
          // This restart window kept a delivered ack — mark it so the
          // failed-discard restore fixup can tell "draft staged by the
          // delivered send this window" apart from "text the user re-typed
          // after the ack consumed the original" (round-26 MAJOR-2).
          restartWindowKeptAckByTask.add(sentTaskId);
          return;
        }
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
        // ensure window reads view.auxId null → sameBinding false, while the
        // rebind's restore has just re-filled the composer from draftByTask —
        // the old binding gate skipped the only composer clear there and left
        // already-delivered text staged for a duplicate Enter. On the same
        // task and past the restart skip above, clearing a composer that
        // still equals the sent text is safe whether the binding is live or
        // mid-ensure: the functional check never touches text typed since,
        // and a panel showing another task keeps its own draft untouched.
        if (!onSameTask) return;
        view.draft = clearedIfSent(view.draft, text);
        // The snapshot pull needs a live binding; inside the rebind window
        // the ensure resolution pulls the fresh snapshot instead.
        if (sameBinding) pullSnapshot(view.auxId);
        emit();
      } catch (error) {
        console.warn('[pinvou3][aux-chat] send failed', error);
        // A failed dispatch delivered nothing — the failed-discard restore
        // fixup must not treat this text as delivered (round-25 MAJOR-24-3).
        // Only delete while the entry still records THIS send's text
        // (round-26 minor M1): a rejection settling after the watchdog (or a
        // restart entry) released the registry entry may race a newer send
        // that already recorded its own text — an unconditional delete would
        // erase the newer send's record and skip its fixup classification.
        if (sentTextByTask.get(sentTaskId) === text) sentTextByTask.delete(sentTaskId);
        // Same ownership for the quote capture (round-26 minor M2): reference
        // identity — a newer send captured a fresh array.
        if (sentQuotesByTask.get(sentTaskId) === quotes) sentQuotesByTask.delete(sentTaskId);
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
    // confirm) → discard the old aux session → ensure a fresh one → clear the
    // local snapshot. discard and ensure are handled in separate stages
    // (their failure semantics differ, see below) but share **one** outer
    // try/finally that resets restarting: no early return (discard failure,
    // generation mismatch) may leave the panel latched in the restarting
    // state — otherwise every button sits disabled under a "please retry"
    // hint.
    const restart = async () => {
      const sessionId = sessionIdMirror;
      if (!auxChat || !sessionId || view.restarting) return;
      // A discard for this task can still be in flight while `restarting` is
      // false: a task switch resets that latch (bind) but leaves the backend
      // turn gate holding the previous new-topic discard for seconds. Issuing
      // a second discard here would overwrite the registry entry, so nobody
      // would await the first one anymore — and on the web relay (invoke
      // responses are not FIFO) the orphaned first discard can land
      // server-side after this restart's recreate, deleting the aux session
      // the panel just bound to, with no JS continuation left to notice. The
      // bind flow already re-ensures this task once the pending discard
      // settles, which is precisely the fresh session this action asks for,
      // so stop here — but not silently: a remounted panel lost the armed
      // confirm, so un-arm here (the binding hint the rebind shows while the
      // discard is parked is the visible "new topic in preparation" feedback
      // for this window).
      // Escape hatch (round-22 Major): an entry whose settle-watchdog fired
      // is stuck — it may never settle, so refusing here forever was the dead
      // end (N1 guard rejecting every New Topic until an app reload). A stuck
      // entry stays registered (the orphan can still settle, and its late
      // settle must keep failing the identity checks below) but no longer
      // blocks this action. Round-23 MAJOR-2 corrected the old "the
      // generation bump makes every continuation of the orphaned discard
      // inert" claim: that holds for its JS continuations only. The orphaned
      // discard COMMAND can still execute server-side, where
      // `discard_aux_session` reads the current mapping at execution time —
      // the stuck suppression (no rebind ensure, no sends while the marker
      // stands) is what keeps that window empty of new transcripts, and the
      // fresh discard here is awaited before the ensure recreates. Documented
      // residual: under the web relay's non-FIFO premise the orphan can still
      // execute after this recreate, destroying the fresh session's
      // transcript up to that point — unremovable frontend-side.
      const registeredDiscard = discardInFlightByTask.get(sessionId);
      if (registeredDiscard && !discardStuckByTask.has(sessionId)) {
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
      // The confirmed restart IS the recovery the stuck banner asks for, so
      // the banner clears when the user acts on it (the controller marker is
      // cleared at the fresh discard's registration below).
      view.discardStuck = false;
      // A failed send's banner must not survive into the restart: when the
      // discard succeeds but the ensure rebuild fails, the binding is cleared
      // and the composer disabled — showing "send failed, retry" next to the
      // ensure failure is a contradictory double banner. ensureFailed is the
      // same class (a stale init failure next to the fresh restart outcome).
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
      // The send registry entry is deliberately KEPT through the restart now
      // (round-25 MAJOR-24-3, reshaping the round-16 B1 clear): the round-23
      // SEND_WATCHDOG_MS failsafe provides the never-settling recovery the
      // entry-delete served, and a pending ack surviving the restart is what
      // lets the failed-discard restore classify the staged draft — deleting
      // the entry here orphaned exactly that classification. While the entry
      // stands, new sends on this task wait at the registry guard with the
      // sending hint visible; the entry leaves through the send's own
      // identity-checked finally or the watchdog, nothing else.
      // The failed-restart marker, however, is cleared: this fresh discard
      // destroys the transcript unless it fails again (re-set in the restore
      // below), so a stale survival must not leak into the new restart window.
      restartDiscardFailedByTask.delete(sessionId);
      // The kept-ack gate is cleared with it (round-26 MAJOR-2): a kept-ack
      // classification from an earlier restart window must not leak into this
      // one, or the new window's failed-discard restore would consume
      // recovery material whose delivery the earlier discard already
      // destroyed.
      restartWindowKeptAckByTask.delete(sessionId);
      // Null the binding at restart entry (round-18 B-1): a send settling
      // inside the discard window must read as the restart case — otherwise
      // its success continuation still sees the old aux id, consumes the
      // draft and staged quotes as "delivered", and the discard then destroys
      // both the transcript and the recovery material. With the binding
      // nulled (same reset as the bind flow), every late settle takes the
      // keep-draft skip; the re-ensure below writes the fresh id back, and
      // the discard-failure path restores the old binding because that
      // session is still alive then.
      view.auxId = null;
      // Feedback for the discard+ensure window (the backend turn gate can
      // hold the discard for seconds, and `restarting` only disables
      // controls): the composer and timeline would otherwise just sit there
      // with no hint that a new topic is being prepared.
      view.bindingPending = true;
      // Clear the snapshot at entry too (round-20 minor-5), mirroring the
      // bind flow: restart otherwise kept the old transcript rendered through
      // the whole discard window, so hasContent stayed true — and the
      // bindingPending hint renders only in the !hasContent branch, leaving a
      // stale timeline, disabled controls and a stale busyHint with no
      // "preparing" feedback. The discard-failure restore re-pulls the
      // snapshot, so a refused restart gets its transcript back.
      setSnapshot(normalizeAuxSnapshot(null));
      // Bump the generation at restart entry: only the bind flow increments
      // it otherwise, so an ensure issued by the current bind that is still
      // in flight would resolve after this restart's discard+ensure with a
      // matching generation and rebind the panel to the just-discarded aux
      // session. Advancing the generation here makes every such stale
      // continuation inert.
      generation += 1;
      const restartGeneration = generation;
      // Restart epoch (round-23 MAJOR-4): bumped in the synchronous restart-
      // entry block, before the discard is issued, so every ack continuation
      // — which can only run once this block yields — reads "restarted".
      // Controller-scoped with the registries so the signal survives rebinds
      // and remounts. The new value is captured: the failed-discard restore
      // below may only re-mark survival while THIS restart is still the
      // task's latest (round-30 D1).
      const restartEpoch = (restartEpochByTask.get(sessionId) || 0) + 1;
      restartEpochByTask.set(sessionId, restartEpoch);
      try {
        try {
          // Register the in-flight discard by task id: while its backend turn
          // gate waits out a running turn, the old mapping is still live, and
          // the bind flow must await this promise before re-ensuring the same
          // task — otherwise it would bind the doomed aux session that this
          // discard deletes behind its back.
          const discardPromise = auxChat.discard(sessionId);
          discardInFlightByTask.set(sessionId, discardPromise);
          // A re-armed restart replaces the stuck entry it escaped: the stale
          // marker referred to the orphaned discard, not this fresh one —
          // leaving it would let the N1 guard wave a third restart through
          // while this healthy discard is still in flight.
          discardStuckByTask.delete(sessionId);
          // A remounted panel may still mirror the stale marker this re-arm
          // replaces (round-25 should-fix-24-2); notify it alongside.
          notifyTaskListeners(discardStuckListenersByTask, sessionId);
          // Settle-watchdog (round-22 Major): the send registry's B1 recovery
          // (clear the entry at restart entry) cannot apply here — deleting
          // the entry is the N1 race itself, because the orphaned discard can
          // still land server-side after a recreate — so a discard that
          // outlives DISCARD_WATCHDOG_MS is *marked* stuck instead: the bind
          // flow stops awaiting it, the N1 guard re-arms New Topic, and the
          // panel surfaces the stuck state. The entry itself stays until the
          // discard settles, so a late settle keeps failing the identity
          // checks; its server-side execution cannot be recalled — the stuck
          // suppression is what empties that window of new transcripts
          // (round-23 MAJOR-2).
          const watchdog = setTimeoutFn(() => {
            // A re-armed restart may have replaced this entry while the
            // orphan was still in flight; only mark while this discard still
            // owns it.
            if (discardInFlightByTask.get(sessionId) !== discardPromise) return;
            discardStuckByTask.add(sessionId);
            // The marker may have been added while the panel was closed and
            // remounted (the watchdog then fires on the dead instance —
            // round-25 should-fix-24-2): notify whichever instance is mounted
            // now so the banner and the latch releases are not lost; the
            // listener re-checks the live task mirror.
            notifyTaskListeners(discardStuckListenersByTask, sessionId);
            // Surface the stuck state on the panel still showing this task,
            // keyed on the live task mirror — NOT on the generation
            // (round-23 MAJOR-3): an A→B→A round-trip re-awaits the
            // still-pending discard under a fresh generation, and a
            // generation gate here would suppress the banner and leave that
            // rebind's bindingPending uncleared — the eternal "preparing"
            // state the watchdog exists to break. A panel showing another
            // task picks the marker up through the bind flow's mirror instead.
            if (sessionIdMirror !== sessionId) return;
            view.discardStuck = true;
            // Release the dead restart latches, or New Topic stays disabled
            // behind `restarting` and the re-arm the marker grants is
            // unreachable (the binding hint would sit there forever too).
            view.restarting = false;
            view.bindingPending = false;
            emit();
          }, discardWatchdogMs);
          try {
            await discardPromise;
          } finally {
            clearTimeoutFn(watchdog);
            // Entry and stuck marker clear by promise identity only: once a
            // re-armed restart owns the registry slot, the orphaned discard's
            // late settle must not touch either — any marker there belongs to
            // the fresh discard. The banner state follows only when a marker
            // was actually cleared AND this panel still shows the task (the
            // settle twin of the watchdog's sessionIdRef gate, round-24
            // minor-7): with two stuck discards (A and B), A's late settle
            // deletes A's marker while the panel shows B — an ungated clear
            // would hide a stuck state the controller marker still records.
            // The bind flow re-mirrors the marker in both directions.
            const ownsEntry = discardInFlightByTask.get(sessionId) === discardPromise;
            if (ownsEntry) discardInFlightByTask.delete(sessionId);
            if (ownsEntry && discardStuckByTask.delete(sessionId)) {
              // Mirror the removal onto whichever instance is mounted now —
              // the settle can run on a dead instance after close/reopen
              // (round-25 should-fix-24-2). The listener re-checks the live
              // task mirror before touching state, which covers this
              // instance's own banner clear too.
              notifyTaskListeners(discardStuckListenersByTask, sessionId);
            }
          }
        } catch (error) {
          console.warn('[pinvou3][aux-chat] restart discard failed', error);
          // Discard failed: the old aux session is still fully alive, so the
          // binding nulled at entry (round-18 B-1) must be restored —
          // otherwise the panel sits send-dead behind the discardFailed copy
          // that says the current topic is still usable (round-20 Major-2),
          // and the only in-panel "retry" (New Topic) would destroy the
          // perfectly alive session. ensure is idempotent and returns that
          // same session; if the restore itself fails, surface the
          // binding-lost state instead. The restore is awaited so the outer
          // finally cannot release the restarting latch before the binding is
          // back.
          if (generation !== restartGeneration) return;
          view.discardFailed = true;
          emit();
          try {
            const restoredAuxId = await withSettleBound(auxChat.ensure(sessionId));
            if (generation !== restartGeneration) return;
            view.auxId = restoredAuxId;
            pullSnapshot(restoredAuxId);
            // The discard failed, so this restore re-bound the SAME live
            // transcript the pre-restart send was delivered into — the
            // keep-draft premise ("the delivery was destroyed") no longer
            // holds for an ack settling under the restart's changed epoch
            // (round-25 MAJOR-24-3). The single-shot marker falls that ack
            // through to normal consumption; with no ack pending, the
            // delivered draft is consumed right here instead. Both steps are
            // gated on this restart still being the task's LATEST (round-30
            // D1): the generation gates are per-instance, so a newer restart
            // that ran on a REMOUNTED panel (the round-22 stuck-escape makes
            // that window reachable) leaves this stale continuation live —
            // its ungated marker add would stand against a transcript the
            // newer restart already destroyed, and its fixup could consume
            // the newer window's kept ack.
            if (restartEpochByTask.get(sessionId) === restartEpoch) {
              restartDiscardFailedByTask.add(sessionId);
              consumeDeliveredDraftAfterFailedRestart(sessionId);
            }
            emit();
          } catch (restoreError) {
            console.warn('[pinvou3][aux-chat] restore after discard failure failed', restoreError);
            if (generation !== restartGeneration) return;
            view.ensureFailed = true;
            emit();
          }
          return;
        }
        // The binding may have changed during the discard round trip: never
        // issue an ensure for the old sessionId now, or the backend would
        // idempotently recreate the aux session that was just discarded (the
        // UI refuses to bind it, but the record already landed on disk).
        if (generation !== restartGeneration) return;
        try {
          const nextAuxId = await withSettleBound(auxChat.ensure(sessionId));
          if (generation !== restartGeneration) return;
          view.auxId = nextAuxId;
          setSnapshot(normalizeAuxSnapshot(nextAuxId ? auxChat.snapshot(nextAuxId) : null));
          // The draft deliberately survives a new topic: the text was typed
          // but never sent, so discarding it would silently eat user input.
          view.sendFailed = false;
          view.ensureFailed = false;
          emit();
        } catch (error) {
          console.warn('[pinvou3][aux-chat] restart ensure failed', error);
          if (generation !== restartGeneration) return;
          // When the discard succeeded but the ensure rebuild fails, the old
          // auxId points at a deleted session: the binding must be cleared so
          // the composer is honestly disabled; show ensureFailed ("recover
          // via new topic"), not sendFailed's "retry send" — a send can no
          // longer succeed at all.
          view.auxId = null;
          setSnapshot(normalizeAuxSnapshot(null));
          view.ensureFailed = true;
          emit();
        }
      } finally {
        // A stale continuation must not clear a newer restart's latch:
        // without the generation check, R1's finally would re-enable the
        // composer and the new-topic button while R2's discard is still in
        // flight, letting a send slip into the session being discarded. The
        // bind flow resets the flag itself, so skipping the reset here cannot
        // leak the state.
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
      if (stuckUnsubscribe) stuckUnsubscribe();
      stuckUnsubscribe = null;
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
