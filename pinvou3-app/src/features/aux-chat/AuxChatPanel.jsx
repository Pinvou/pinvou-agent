import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { MessageSquare, Quote, RotateCcw, Send, X } from '../../components/icons.jsx';
import { RightDockPanel } from '../../components/layout/RightDock.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import { constrainChatInput } from '../chat/chat-input-limit.js';
import { ConversationTimeline } from '../conversation/ConversationTimeline.jsx';
import { transitionConversationScrollState } from '../conversation/conversation-scroll.js';
import {
  buildAuxQuoteBlock,
  dropAuxQuotes,
  getAuxQuotes,
  removeAuxQuote,
  subscribeAuxQuotes,
} from './aux-quote.mjs';
import {
  auxChatBusy,
  auxChatHasContent,
  auxSnapshotsEqual,
  normalizeAuxSnapshot,
  projectAuxChatTurns,
} from './aux-chat-state.mjs';

/**
 * Auxiliary chat panel (right-side RightDock): each main task gets one
 * independent question-only session that never joins the main task's
 * execution or context (the bridge-side send pins restrictTools), never goes
 * through the subagent system, and never becomes a second task entry
 * (ADR-0006 constraint).
 *
 * Data flow: on first open / rebind, ensure(sessionId) idempotently returns
 * the auxId — locally we only keep the auxId and a snapshot; on chat-domain
 * notify (background session events are routed into the per-session buffer)
 * the snapshot is re-pulled. The app maintains no session state machine.
 */

const RESTART_CONFIRM_MS = 4000;

// Mirrors the web lane's invoke timeout (web/bootstrap.js rejects a pending
// invoke at 180 s): there a wedged discard settles on its own as a rejection,
// while the desktop invoke has no transport timeout, so a never-settling
// discard would otherwise keep its registry entry — and every guard keyed on
// it — forever. The threat model treats hung invokes as real, so the discard
// registry gets the same never-settling recovery the send registry got in
// round-16 B1, just with a different shape (see the watchdog in handleRestart:
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

const withSettleBound = (promise) => new Promise((resolve, reject) => {
  const timer = setTimeout(() => reject(new Error('ensure settle bound exceeded')), ENSURE_WATCHDOG_MS);
  promise.then(
    (value) => { clearTimeout(timer); resolve(value); },
    (error) => { clearTimeout(timer); reject(error); },
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
// — but it is no longer awaited by the rebind effect and no longer blocks New
// Topic: awaiting/refusing forever was the dead end (eternal "preparing"
// hint, dead composer, N1 guard refusing every restart until an app reload).
// A task's marker is cleared when the owning discard settles or when a
// re-armed restart registers a fresh discard for that task.
// Disposition while the marker stands (round-23 MAJOR-2): the panel neither
// re-binds (the rebind effect skips ensure) nor sends — the orphaned discard
// COMMAND can still execute server-side, where it deletes whatever aux
// session is mapped for this task at execution time, so nothing new may be
// put in front of it. The re-armed New Topic is the recovery: it re-issues a
// fresh discard, awaits it, and only then re-ensures.
const discardStuckByTask = new Set();

// taskId -> pending send promise, module-scoped for the same reason as the
// discard registry above: the duplicate-send window outlives the component
// instance. The rebind effect resets the component-level sendingRef on every
// task switch (a never-settling invoke must not latch the next task), so a
// send on task A → switch to B → back to A would otherwise pass every guard
// before turn_started lands in the buffer, firing a duplicate turn on the
// same aux session (round-15 MAJOR-2). Keyed by task — aux ids are 1:1 with
// tasks and ensure is idempotent — and entries are removed by the exact send
// that registered them once it settles.
const sendInFlightByTask = new Map();

// taskId -> restart epoch, module-scoped with the registries above: a send
// dispatched before a restart must know, at ack time, whether a restart for
// this task was initiated since the dispatch — the keep-draft skip in
// handleSend is the restart case only (the delivery was destroyed with the
// discarded transcript, so the text stays as recovery material), while a
// same-task rebind re-ensures the SAME live aux session and an ack landing
// inside its null-binding window must consume the draft (the delivery reached
// a live transcript; keeping it restores already-delivered text for a
// duplicate send, round-23 MAJOR-4). handleRestart bumps the epoch in its
// synchronous entry block, before the discard is issued, so every ack
// ordering inside and after the restart window reads "restarted"; the count
// survives rebinds, closes and remounts that a component-level flag could not.
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
// again) and single-shot when an ack consumes under it.
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

// taskId -> listener sets for the two module-state transitions a mounted
// panel must follow but that can originate on a DEAD instance (the panel was
// closed and remounted while the transition ran — round-25 MAJOR-24-2 and
// should-fix-24-2): draft-store deletions (an ack consuming the entry the
// mounted composer was restored from) and stuck-marker changes (a watchdog
// firing, or a pending discard settling, behind the remount). Listeners are
// registered while mounted and removed by the effect cleanup, so a dead
// instance never holds one; each listener re-checks the live task mirror
// before touching state.
const draftDeleteListenersByTask = new Map();
const discardStuckListenersByTask = new Map();

const notifyTaskListeners = (listenersByTask, taskId) => {
  const listeners = listenersByTask.get(taskId);
  // Iterating the Set directly is deliberate: a listener unsubscribing
  // mid-notify (effect cleanup) is skipped for the remaining visits, which
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

const deleteDraftAndNotify = (taskId) => {
  draftByTask.delete(taskId);
  notifyTaskListeners(draftDeleteListenersByTask, taskId);
};

// The keep-draft decision of a send ack, module state only (round-25
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

// A quote-only send is valid — the excerpts alone are the question — so the
// dispatch needs either text or a captured quote block.
const hasSendContent = (text, quoteBlock) => Boolean(text || quoteBlock);

// The visible composer clear only eats text that still equals the delivered
// message; anything typed since the dispatch belongs to the next one.
const clearedIfSent = (current, text) => (current.trim() === text ? '' : current);

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
  if (sentQuotes) dropAuxQuotes(sessionId, sentQuotes);
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

const armSendWatchdog = (taskId, sendPromise, onFailsafe) => setTimeout(() => {
  if (sendInFlightByTask.get(taskId) !== sendPromise) return;
  sendInFlightByTask.delete(taskId);
  onFailsafe();
}, SEND_WATCHDOG_MS);

// taskId -> unsent composer draft, module-scoped for the same reason as the
// discard registry above: a draft belongs to the task, not to this instance,
// while the panel unmounts on close and on sched- switches. Wiping it on a
// task switch, a panel close or a new-topic confirm discarded text the user
// had typed and never sent — it is now only cleared once a send actually
// succeeded (or the user deletes it).
const draftByTask = new Map();

export function AuxChatPanel({ sessionId, activationKey, t, theme, onClose, onActiveChange }) {
  const copy = t.uiAuxChat;
  const conversationCopy = t.uiConversation;
  const auxChat = bridge.available ? bridge.auxChat : null;

  const [auxId, setAuxId] = useState(null);
  const [snapshot, setSnapshot] = useState(() => normalizeAuxSnapshot(null));
  const [draft, setDraft] = useState('');
  // Pending conversation quotes staged from the main timeline ("划词引用").
  // Same task-scoped ownership as the draft: a quote selected in the main
  // conversation must survive panel close/reopen and rebinds, and updates
  // arriving while the panel is mounted are delivered through the store's
  // subscription (the selection popover lives outside this panel).
  const [quotes, setQuotes] = useState(() => (sessionId ? getAuxQuotes(sessionId) : []));
  const [sendFailed, setSendFailed] = useState(false);
  const [ensureFailed, setEnsureFailed] = useState(false);
  const [discardFailed, setDiscardFailed] = useState(false);
  // Mirrors the module-scoped discardStuckByTask marker for rendering: the
  // settle-watchdog fires from a timer (not a render), and a remount must
  // pick up a marker set while this panel was unmounted.
  const [discardStuck, setDiscardStuck] = useState(false);
  const [restartArmed, setRestartArmed] = useState(false);
  const [restarting, setRestarting] = useState(false);
  // True while the current binding has no aux session yet (first open, rebind,
  // or a rebind parked behind an in-flight discard). Without it the panel
  // renders the "nothing here yet" landing during ensure — a false empty state
  // for a conversation that is merely being prepared.
  const [bindingPending, setBindingPending] = useState(false);
  // Mirrors sendingRef for rendering: the in-flight send window has no visible
  // feedback of its own (snapshot-busy only lands with the backend
  // turn_started), so the composer looked idle while the message was gone.
  const [sending, setSending] = useState(false);
  const generationRef = useRef(0);
  const auxIdRef = useRef(null);
  // Live mirror of the sessionId prop for async continuations: the closure
  // captured at send time freezes it, but a send settling after a task switch
  // must know which task the panel shows *now* (round-17 M-A).
  const sessionIdRef = useRef(sessionId);
  const scrollRef = useRef(null);
  // Bottom-follow state, same pattern as the main conversation's
  // autoScrollRef: true while the reader is at (or returns to) the tail, so
  // content growth snaps the view only while following and never yanks
  // someone scrolled up through history (round-20 minor-6).
  const autoScrollRef = useRef(true);
  const lastScrollTopRef = useRef(0);
  const lastScrollHeightRef = useRef(0);
  // Auto-grow anchor: the composer grows with its content like the main
  // conversation composer instead of scrolling inside a one-row-tall box.
  const composerRef = useRef(null);
  // In-flight send latch: the bridge marks the session busy only when the
  // backend turn_started event lands, so snapshot-busy lags a dispatch by the
  // relay round trip — without this latch a double Enter fires a duplicate
  // turn and its rejection would surface as a bogus "send failed" banner.
  const sendingRef = useRef(false);

  // Keep the old state when the snapshot is unchanged (the functional setState
  // returns the same value and React skips the re-render), blocking the
  // useless re-pulls that the main session's streaming ticks trigger through
  // chat-domain notifies.
  const pullSnapshot = useCallback((id) => {
    const raw = id && auxChat ? auxChat.snapshot(id) : null;
    setSnapshot((current) => {
      const next = normalizeAuxSnapshot(raw);
      return auxSnapshotsEqual(current, next) ? current : next;
    });
  }, [auxChat]);

  // First open and main-session rebind: drop the old binding and idempotently
  // ensure the new task's aux session. The generation guard stops a
  // late-arriving ensure result from binding the panel back to the previous
  // task.
  useEffect(() => {
    const generation = generationRef.current + 1;
    generationRef.current = generation;
    auxIdRef.current = null;
    sessionIdRef.current = sessionId;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously reset binding state on session switch; one-shot mirror, same pattern as SubagentTranscriptPanel
    setAuxId(null);
    setSnapshot(normalizeAuxSnapshot(null));
    setSendFailed(false);
    setEnsureFailed(false);
    setDiscardFailed(false);
    setRestartArmed(false);
    // Mirror the task's stuck marker (the watchdog may have fired while this
    // panel was unmounted; a switch from another task must not inherit or
    // keep that task's banner).
    setDiscardStuck(!!(sessionId && discardStuckByTask.has(sessionId)));
    // Reset restarting on every rebind: the two restart invokes have no
    // transport timeout, so a promise that never settles would otherwise
    // latch the new task's panel disabled forever (the old restart's finally
    // can no longer be relied on once its generation went stale).
    setRestarting(false);
    // Same latch class for sends: a never-settling auxChat.send invoke must
    // not permanently block sends across later task rebinds either. Only the
    // component-level latch resets here — the duplicate-send guard itself
    // lives in sendInFlightByTask, which survives the rebind by design.
    sendingRef.current = false;
    setSending(false);
    // Restore this task's unsent draft instead of wiping the composer.
    setDraft(sessionId ? (draftByTask.get(sessionId) || '') : '');
    setQuotes(sessionId ? getAuxQuotes(sessionId) : []);
    setBindingPending(!!(auxChat && sessionId));
    if (!auxChat || !sessionId) return;
    let disposed = false;
    // An in-flight discard for this same task must settle first: its backend
    // turn gate waits out the running turn (seconds), and while it is pending
    // the old mapping is still live — an ensure issued now would idempotently
    // return the doomed aux session, which the discard then deletes behind
    // the panel's back, leaving a dead binding. Await the in-flight promise
    // (rejections surface below and on the restart path) and re-check the
    // generation so a further rebind during the wait aborts this ensure
    // entirely. A stuck
    // entry (round-22 Major: its settle-watchdog fired, so it may never
    // settle) is neither awaited nor ensured at all (round-23 MAJOR-2): the
    // orphaned discard command can still execute server-side against the
    // current mapping, so binding or sending now would put a live transcript
    // in front of it. The panel stays unbound behind the stuck banner; the
    // re-armed New Topic is the recovery (fresh discard, awaited, before its
    // ensure).
    const pendingDiscard = discardInFlightByTask.get(sessionId);
    const ensureAfterDiscard = () => {
      if (disposed || generationRef.current !== generation) return Promise.resolve();
      // The chain is returned so the awaited-discard rejection arm can run
      // its post-restore fixup after the binding settles (round-28 B1); the
      // fire-and-forget callers ignore it.
      return auxChat.ensure(sessionId)
        .then((nextAuxId) => {
          if (disposed || generationRef.current !== generation) return;
          setBindingPending(false);
          auxIdRef.current = nextAuxId;
          setAuxId(nextAuxId);
          pullSnapshot(nextAuxId);
        })
        .catch((error) => {
          console.warn('[pinvou3][aux-chat] ensure failed', error);
          // When ensure fails the composer is disabled via the empty auxId,
          // but the reason is invisible; an inline hint tells the user the
          // initialization did not succeed instead of facing a dead panel.
          if (disposed || generationRef.current !== generation) return;
          setBindingPending(false);
          setEnsureFailed(true);
        });
    };
    if (discardStuckByTask.has(sessionId)) {
      // Stuck: skip ensure and drop the "preparing" hint — the stuck banner
      // (mirrored above) is the state the panel shows, and the composer stays
      // disabled behind the null binding until New Topic recovers.
      setBindingPending(false);
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
        if (!disposed && generationRef.current === generation) setDiscardFailed(true);
        // Bookkeeping parity with the restart's own discard-failure catch
        // (round-28 B1, the round-27 Major α): the transcript SURVIVED this
        // failed discard, so the keep-draft premise no longer holds for an
        // ack settling under this window, and a kept ack's delivered draft
        // must be consumed by the restore fixup — omitting either here
        // re-opened the duplicate-send class through the rebind arm. The
        // marker is task-level state, so it is added even when this
        // instance's generation went stale (a newer restart clears it at
        // entry; a later restart clears it at entry); the fixup is chained
        // behind this arm's ensure and generation-gated like the bind.
        restartDiscardFailedByTask.add(sessionId);
        Promise.resolve(ensureAfterDiscard()).then(
          () => {
            if (!disposed && generationRef.current === generation) {
              consumeDeliveredDraftAfterFailedRestart(sessionId);
            }
          },
          () => {},
        );
      });
    } else {
      ensureAfterDiscard();
    }
    return () => { disposed = true; };
  }, [auxChat, sessionId, pullSnapshot]);

  // Composer auto-grow, same mechanism as the main conversation composer:
  // grow with the content up to the max-h-32 cap (then scroll internally) and
  // shrink back when the draft clears. Re-runs on rebind so a restored draft
  // sizes the box immediately.
  useEffect(() => {
    const el = composerRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = Math.min(Math.max(el.scrollHeight, 24), 128) + 'px';
  }, [draft, sessionId]);

  // Quotes staged while this panel is mounted (the selection popover runs in
  // the main view, not here) arrive through the store subscription; the
  // re-read on subscribe also closes the gap between mount and the first
  // stageAuxQuote call.
  useEffect(() => {
    if (!sessionId) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- one-shot mirror of the module store on (re)bind, same pattern as the draft restore; later updates arrive via the subscription callback
    setQuotes(getAuxQuotes(sessionId));
    return subscribeAuxQuotes(sessionId, (next) => {
      setQuotes(next.map((quote) => ({ text: quote.text })));
    });
  }, [sessionId]);

  // Follow draft-store deletions made by another instance (a send ack that
  // settled on a dead instance after close/reopen — round-25 MAJOR-24-2):
  // the composer was restored from the entry the ack consumed, so without
  // this mirror it keeps a delivered message staged for a duplicate send.
  // The delete only fires when the stored draft still equals the sent text,
  // and typing since would have re-populated the store before the ack's
  // delete check, so the clear never eats newer text (the composer mirrors
  // the store through onChange on the instance that owns it).
  useEffect(() => {
    if (!sessionId) return;
    return subscribeTaskListeners(draftDeleteListenersByTask, sessionId, () => {
      if (sessionIdRef.current !== sessionId) return;
      setDraft('');
    });
  }, [sessionId]);

  // Re-mirror the task's stuck marker on module-state transitions (round-25
  // should-fix-24-2): the settle-watchdog can fire, or a pending discard can
  // settle, while this panel was closed and remounted — the transition then
  // runs on the dead instance (whose state writes are lost) and the
  // remounted panel would sit in the eternal "preparing" state with no
  // banner until a task switch. Transitions notify this listener, which
  // re-mirrors the marker and, on stuck, applies the watchdog's latch
  // releases the dead instance could no longer deliver.
  useEffect(() => {
    if (!sessionId) return;
    return subscribeTaskListeners(discardStuckListenersByTask, sessionId, () => {
      if (sessionIdRef.current !== sessionId) return;
      const stuck = discardStuckByTask.has(sessionId);
      setDiscardStuck(stuck);
      if (stuck) {
        setRestarting(false);
        setBindingPending(false);
      }
    });
  }, [sessionId]);

  // Background aux-session turn events already land in the per-session buffer
  // and trigger notifies; subscribing to the chat domain and re-pulling the
  // snapshot is enough — no extra event listener. The re-pull doubles as the
  // LRU touch of an always-open panel (snapshot() refreshes recency): buffer
  // capacity eviction relies on this subscription being unconditionally
  // delivered — if the subscription ever gains a change gate, the touch needs
  // another resident driver.
  useEffect(() => {
    if (!auxChat || !bridge.state) return;
    return bridge.state.subscribeMany(['chat'], () => {
      if (auxIdRef.current) pullSnapshot(auxIdRef.current);
    });
  }, [auxChat, pullSnapshot]);

  const busy = auxChatBusy(snapshot);
  const hasContent = auxChatHasContent(snapshot);
  // The sending hint must also cover the cross-switch in-flight window
  // (round-20 minor-4): a send on task A → switch to B → back to A resets the
  // component-level sending flag (rebind effect) while the registry still
  // holds A's send, so Enter no-ops at the registry guard — without the
  // registry-derived hint that window looked like a silently dead composer.
  const sendInFlight = sending || !!(sessionId && sendInFlightByTask.has(sessionId));
  const turns = useMemo(
    () => (auxId ? projectAuxChatTurns(snapshot, auxId) : []),
    [snapshot, auxId],
  );

  // Send-latch release at turn_started, not at the dispatch ack (round-20
  // minor-4): the invoke resolving only means the backend accepted the
  // command — turn_started still lags it by one event round trip, so
  // releasing the latch in the send's finally re-opened the duplicate-send
  // window exactly where the latch claims coverage (fresh input there dies at
  // the backend turn gate as a misleading "send failed, retry" banner). Hold
  // the latch until snapshot-busy proves the backend took the turn; the
  // failure path releases it directly, and rebind/restart reset it as before.
  useEffect(() => {
    if (!sending || !busy) return;
    sendingRef.current = false;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- derived release of the in-flight send latch once turn_started marks the snapshot busy
    setSending(false);
  }, [sending, busy]);

  // Follow-state tracking, mirroring the main conversation's scroll listener:
  // scrolling up parks the follow flag, returning near the bottom resumes it.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      const transition = transitionConversationScrollState({
        scrollElement: el,
        following: autoScrollRef.current,
        previousScrollTop: lastScrollTopRef.current,
        previousScrollHeight: lastScrollHeightRef.current,
      });
      lastScrollTopRef.current = transition.scrollTop;
      lastScrollHeightRef.current = transition.scrollHeight;
      autoScrollRef.current = transition.following;
    };
    onScroll();
    el.addEventListener('scroll', onScroll, { passive: true });
    return () => el.removeEventListener('scroll', onScroll);
    // Keyed on auxId, not []: the dock portal children appear only after the
    // panel's mount dispatch re-renders the host, so the very first commit
    // still has scrollRef null and a mount-once effect would never attach
    // (round-28 B2). auxId is set only after ensure resolves, when the
    // element exists, and re-running on rebind just re-attaches idempotently
    // while refreshing the scroll-origin refs.
  }, [auxId]);

  // A rebind always opens the (fresh or switched) aux transcript at its tail.
  useEffect(() => {
    autoScrollRef.current = true;
  }, [auxId]);

  // Snap to the tail on any content growth — new turns and streaming deltas
  // alike (a delta re-pulls the snapshot, so depending on the snapshot covers
  // both) — but only while following: the old unconditional snap yanked a
  // scrolled-up reader on every new item (round-20 minor-6).
  useEffect(() => {
    const el = scrollRef.current;
    if (el && autoScrollRef.current) el.scrollTop = el.scrollHeight;
  }, [auxId, snapshot]);

  useEffect(() => {
    if (!restartArmed) return;
    const timer = setTimeout(() => setRestartArmed(false), RESTART_CONFIRM_MS);
    return () => clearTimeout(timer);
  }, [restartArmed]);

  // The send guards mirror handleRestart: the panel does not close on a main
  // task switch, so the binding may have changed while a send was in flight —
  // snapshot the auxId on entry and re-check before touching the UI, so the
  // old task's outcome (draft clear / failure banner) never lands on the new
  // task's panel. Also reject while restarting: between the restart confirm
  // and the discard completing, Enter must not submit into the old session
  // that is about to be discarded (the disabled composer only locks the
  // button, not this direct Enter path). sendingRef closes the remaining
  // gap: snapshot-busy lags the dispatch by one event round trip, so without
  // the latch a double Enter fires a duplicate turn whose backend rejection
  // surfaces as a bogus "send failed, retry" banner while the first reply is
  // actually streaming.
  const handleSend = useCallback(async () => {
    const text = draft.trim();
    // Pending conversation quotes ride inline with the message as a fenced
    // userselect block: the engine sees plain message text (it has no concept
    // of quotes) while the timeline projection parses the block back into
    // chips. A quote-only send (empty draft) is valid — the excerpts alone
    // are a question about "what does this mean".
    const quoteBlock = buildAuxQuoteBlock(quotes);
    const sentAuxId = auxIdRef.current;
    // Staleness boundary for both outcomes: the task id plus the restart
    // epoch (captured below). A restart on the same task re-binds to a *new*
    // aux, so a send issued before it must surface nothing at all — neither
    // the contradictory "retry send" banner next to the restart's own state,
    // nor a draft clear that would eat recovery material.
    const sentTaskId = sessionId;
    // The restart boundary for the keep-draft skip below: "a restart was
    // initiated for this task since this dispatch", not binding equality
    // (round-23 MAJOR-4).
    const sentEpoch = restartEpochByTask.get(sentTaskId) || 0;
    if (!auxChat || !sentAuxId || !hasSendContent(text, quoteBlock) || busy || restarting || sendingRef.current) return;
    // The registry is the guard that survives rebinds: the rebind effect
    // resets sendingRef on every task switch, so a switch away and back while
    // this send is still in flight would otherwise re-open the duplicate-send
    // window (turn_started lags the dispatch by one event round trip).
    if (sendInFlightByTask.has(sentTaskId)) return;
    // A stuck discard for this task may still execute server-side (round-23
    // MAJOR-2): it deletes whatever aux session is mapped at execution time,
    // so a message sent now could be destroyed with the session it landed in.
    // The stuck banner is the visible state; the re-armed New Topic (fresh
    // awaited discard) is the recovery.
    if (discardStuckByTask.has(sentTaskId)) return;
    sendingRef.current = true;
    setSending(true);
    setSendFailed(false);
    const sendPromise = auxChat.send(sentAuxId, quoteBlock ? text + quoteBlock : text);
    sendInFlightByTask.set(sentTaskId, sendPromise);
    // Record the exact sent text for the failed-discard restore fixup
    // (round-25 MAJOR-24-3): the restore can run after this ack already
    // settled, so the delivered text must be recoverable without the closure.
    // Overwritten by the next dispatch; a failed send clears it below (a
    // failed dispatch delivered nothing). The captured quotes ride alongside
    // (round-26 minor M2): the epoch-skipped ack path deliberately keeps them
    // staged while the discard outcome is unknown, so the restore fixup needs
    // the capture identity to drop them when the delivery is known to have
    // survived.
    sentTextByTask.set(sentTaskId, text);
    sentQuotesByTask.set(sentTaskId, quotes);
    // Send-latch failsafe (round-23 should-fix 1), the send-side twin of the
    // discard watchdog: the busy-gated latch release above the timeline only
    // fires if a render observes busy=true — when turn_started and the
    // turn-terminal events coalesce into one render batch (fast-failing
    // turns, relay event bursts), busy is never true and the latch and this
    // registry entry would stick with no recovery path. Past the bound (the
    // web lane's invoke timeout, same as the discard watchdog), release both:
    // a still-running turn makes the next dispatch surface the backend's
    // honest busy rejection, a finished one just un-deads the composer; New
    // Topic remains the recovery for the never-settling invoke itself.
    const sendWatchdog = armSendWatchdog(sentTaskId, sendPromise, () => {
      sendingRef.current = false;
      setSending(false);
    });
    try {
      await sendPromise;
      // Same binding (aux ids are 1:1 with tasks and ensure is idempotent):
      // the visible composer shows this task and the message was delivered —
      // consume, even across a generation bump (round-14 B3). Same task but a
      // *different* binding is the restart case: the old aux was discarded,
      // so the delivery is gone and the draft stays as recovery material.
      // Anything else means the panel moved to another task while this send
      // settled into A's *live* transcript — the store maps must still be
      // consumed, or returning to A restores already-delivered text and
      // staged quotes for a duplicate send (round-17 M-A).
      const sameBinding = auxIdRef.current === sentAuxId;
      const onSameTask = sessionIdRef.current === sentTaskId;
      // The keep-draft skip is the restart case only, keyed on the restart
      // epoch captured at dispatch — and consulted UNCONDITIONALLY via the
      // module-state helper (round-25 MAJOR-24-1): the epoch is module-
      // scoped exactly because it must survive rebinds, closes and remounts,
      // while sameBinding/onSameTask read per-instance refs that a
      // close/reopen freezes at their last values — a frozen
      // auxIdRef.current === sentAuxId read as sameBinding true on the dead
      // instance and short-circuited this skip, so the ack deleted the draft
      // a restart had deliberately preserved as recovery material. Binding
      // equality stays meaningful only for the visible half below.
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
        dropAuxQuotes(sentTaskId, quotes);
      }
      // The visible composer clear is task-gated, not binding-gated (round-24
      // Major): a send ack settling inside the same-task rebind's ensure
      // window reads auxIdRef null → sameBinding false, while the rebind's
      // restore has just re-filled the composer from draftByTask — the old
      // binding gate skipped the only composer clear there and left
      // already-delivered text staged for a duplicate Enter. On the same task
      // and past the restart skip above, clearing a composer that still
      // equals the sent text is safe whether the binding is live or
      // mid-ensure: the functional check never touches text typed since, and
      // a panel showing another task keeps its own draft untouched.
      if (!onSameTask) return;
      setDraft((current) => clearedIfSent(current, text));
      // The snapshot pull needs a live binding; inside the rebind window the
      // ensure resolution pulls the fresh snapshot instead.
      if (sameBinding) pullSnapshot(auxIdRef.current);
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
      // A plain rebind round trip (A→B→A, no restart) must still surface the
      // failure banner: the dispatch genuinely rejected and the panel is back
      // on this task — the old generation gate silenced that banner too
      // (round-24 minor-8). The restart case stays silent: the restart-entry
      // flow owns the panel state and keeps the draft as recovery material
      // (a "retry send" banner would contradict the restart's own copy).
      if (sendInFlightByTask.get(sentTaskId) !== sendPromise
        || (restartEpochByTask.get(sentTaskId) || 0) !== sentEpoch
        || sessionIdRef.current !== sentTaskId) return;
      setSendFailed(true);
      // A failed dispatch never reaches turn_started, so the busy-gated latch
      // release above the timeline would never fire — release the latch here
      // or the composer stays locked behind the failure banner. The gates
      // above plus this binding check mean this is the exact send that set
      // the latch on the binding that still owns it (round-14 B2); inside the
      // rebind window the rebind effect already reset the latch, so skipping
      // the release there is a no-op.
      if (auxIdRef.current === sentAuxId) {
        sendingRef.current = false;
        setSending(false);
      }
    } finally {
      clearTimeout(sendWatchdog);
      // The registry entry is removed by the exact send that registered it,
      // unconditionally — unlike the component latch it must not depend on
      // the binding state, or a send settling after a rebind would leak the
      // entry and block this task's sends forever. The latch itself is NOT
      // released here (round-20 minor-4): this resolve is only the dispatch
      // ack and turn_started still lags it by one event round trip, so
      // releasing now would re-open the duplicate-send window exactly where
      // the latch claims coverage.
      removeSendIfOwner(sentTaskId, sendPromise);
    }
  }, [auxChat, draft, quotes, busy, restarting, pullSnapshot, sessionId]);

  const handleComposerKeyDown = useCallback((event) => {
    if (event.repeat) return;
    if (event.key !== 'Enter' || event.shiftKey || isImeComposing(event)) return;
    event.preventDefault();
    void handleSend();
  }, [handleSend]);

  // New topic: two-step lightweight confirm (the system window.confirm does
  // not pop under Tauri WebView2; the repo precedent is a self-drawn confirm)
  // → discard the old aux session → ensure a fresh one → clear the local
  // snapshot. discard and ensure are handled in separate stages (their
  // failure semantics differ, see below) but share **one** outer try/finally
  // that resets restarting: no early return (discard failure, generation
  // mismatch) may leave the panel latched in the restarting state — otherwise
  // every button sits disabled under a "please retry" hint.
  const handleRestart = useCallback(async () => {
    if (!auxChat || !sessionId || restarting) return;
    // A discard for this task can still be in flight while `restarting` is
    // false: a task switch resets that latch (rebind effect) but leaves the
    // backend turn gate holding the previous new-topic discard for seconds.
    // Issuing a second discard here would overwrite the registry entry, so
    // nobody would await the first one anymore — and on the web relay (invoke
    // responses are not FIFO) the orphaned first discard can land server-side
    // after this restart's recreate, deleting the aux session the panel just
    // bound to, with no JS continuation left to notice. The rebind effect
    // already re-ensures this task once the pending discard settles, which is
    // precisely the fresh session this action asks for, so stop here — but
    // not silently: a remounted panel lost the armed confirm, so un-arm here
    // (the binding hint the rebind shows while the discard is parked is the
    // visible "new topic in preparation" feedback for this window).
    // Escape hatch (round-22 Major): an entry whose settle-watchdog fired is
    // stuck — it may never settle, so refusing here forever was the dead end
    // (N1 guard rejecting every New Topic until an app reload). A stuck entry
    // stays registered (the orphan can still settle, and its late settle must
    // keep failing the identity checks below) but no longer blocks this
    // action. Round-23 MAJOR-2 corrected the old "the generation bump makes
    // every continuation of the orphaned discard inert" claim: that holds for
    // its JS continuations only. The orphaned discard COMMAND can still
    // execute server-side, where `discard_aux_session` reads the current
    // mapping at execution time — the stuck suppression (no rebind ensure, no
    // sends while the marker stands) is what keeps that window empty of new
    // transcripts, and the fresh discard here is awaited before the ensure
    // recreates. Documented residual: under the web relay's non-FIFO premise
    // the orphan can still execute after this recreate, destroying the fresh
    // session's transcript up to that point — unremovable frontend-side.
    const registeredDiscard = discardInFlightByTask.get(sessionId);
    if (registeredDiscard && !discardStuckByTask.has(sessionId)) {
      setRestartArmed(false);
      return;
    }
    if (!restartArmed) {
      setRestartArmed(true);
      return;
    }
    setRestartArmed(false);
    setRestarting(true);
    setDiscardFailed(false);
    // The confirmed restart IS the recovery the stuck banner asks for, so the
    // banner clears when the user acts on it (the module marker is cleared at
    // the fresh discard's registration below).
    setDiscardStuck(false);
    // A failed send's banner must not survive into the restart: when the
    // discard succeeds but the ensure rebuild fails, the binding is cleared
    // and the composer disabled — showing "send failed, retry" next to the
    // ensure failure is a contradictory double banner. ensureFailed is the
    // same class (a stale init failure next to the fresh restart outcome).
    setSendFailed(false);
    setEnsureFailed(false);
    // Release the send latch at restart entry (round-12 N2): a send issued for
    // the old binding can settle after this restart re-bound auxIdRef, and its
    // finally deliberately refuses to clear the latch once the binding moved
    // (see handleSend). Only the rebind effect resets it otherwise, and a
    // same-task restart does not re-run that effect — so without this reset a
    // late-settling send would leave sendingRef stuck true and every later
    // Enter would silently no-op behind a visually enabled composer.
    sendingRef.current = false;
    setSending(false);
    // The module-scoped send registry entry is deliberately KEPT through the
    // restart now (round-25 MAJOR-24-3, reshaping the round-16 B1 clear):
    // the round-23 SEND_WATCHDOG_MS failsafe provides the never-settling
    // recovery the entry-delete served, and a pending ack surviving the
    // restart is what lets the failed-discard restore classify the staged
    // draft — deleting the entry here orphaned exactly that classification.
    // While the entry stands, new sends on this task wait at the registry
    // guard with the sending hint visible; the entry leaves through the
    // send's own identity-checked finally or the watchdog, nothing else.
    // The failed-restart marker, however, is cleared: this fresh discard
    // destroys the transcript unless it fails again (re-set in the restore
    // below), so a stale survival must not leak into the new restart window.
    restartDiscardFailedByTask.delete(sessionId);
    // The kept-ack gate is cleared with it (round-26 MAJOR-2): a kept-ack
    // classification from an earlier restart window must not leak into this
    // one, or the new window's failed-discard restore would consume recovery
    // material whose delivery the earlier discard already destroyed.
    restartWindowKeptAckByTask.delete(sessionId);
    // Null the binding at restart entry (round-18 B-1): a send settling inside
    // the discard window must read as the restart case — otherwise its
    // success continuation still sees the old aux id, consumes the draft and
    // staged quotes as "delivered", and the discard then destroys both the
    // transcript and the recovery material. With the binding nulled (same
    // reset as the rebind effect), every late settle takes the keep-draft
    // skip; the re-ensure below writes the fresh id back, and the
    // discard-failure path restores the old binding because that session is
    // still alive then.
    auxIdRef.current = null;
    setAuxId(null);
    // Feedback for the discard+ensure window (the backend turn gate can hold
    // the discard for seconds, and `restarting` only disables controls): the
    // composer and timeline would otherwise just sit there with no hint that a
    // new topic is being prepared.
    setBindingPending(true);
    // Clear the snapshot at entry too (round-20 minor-5), mirroring the rebind
    // effect: restart otherwise kept the old transcript rendered through the
    // whole discard window, so hasContent stayed true — and the bindingPending
    // hint renders only in the !hasContent branch, leaving a stale timeline,
    // disabled controls and a stale busyHint with no "preparing" feedback.
    // The discard-failure restore re-pulls the snapshot, so a refused restart
    // gets its transcript back.
    setSnapshot(normalizeAuxSnapshot(null));
    // Bump the generation at restart entry: only the rebind effect increments
    // it otherwise, so an ensure issued by the current rebind that is still in
    // flight (including its ensureSessionBufferLoaded chain) would resolve
    // after this restart's discard+ensure with a matching generation and
    // rebind the panel to the just-discarded aux session. Advancing the
    // generation here makes every such stale continuation inert.
    generationRef.current += 1;
    const generation = generationRef.current;
    // Restart epoch (round-23 MAJOR-4): bumped in the synchronous restart-
    // entry block, before the discard is issued, so every ack continuation —
    // which can only run once this block yields — reads "restarted". Module-
    // scoped with the registries so the signal survives rebinds and remounts.
    restartEpochByTask.set(sessionId, (restartEpochByTask.get(sessionId) || 0) + 1);
    try {
      try {
        // Register the in-flight discard by task id: while its backend turn
        // gate waits out a running turn, the old mapping is still live, and
        // the rebind effect must await this promise before re-ensuring the
        // same task — otherwise it would bind the doomed aux session that
        // this discard deletes behind its back.
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
        // outlives DISCARD_WATCHDOG_MS is *marked* stuck instead: the rebind
        // effect stops awaiting it, the N1 guard re-arms New Topic, and the
        // panel surfaces the stuck state. The entry itself stays until the
        // discard settles, so a late settle keeps failing the identity
        // checks; its server-side execution cannot be recalled — the stuck
        // suppression is what empties that window of new transcripts
        // (round-23 MAJOR-2).
        const watchdog = setTimeout(() => {
          // A re-armed restart may have replaced this entry while the orphan
          // was still in flight; only mark while this discard still owns it.
          if (discardInFlightByTask.get(sessionId) !== discardPromise) return;
          discardStuckByTask.add(sessionId);
          // The marker may have been added while the panel was closed and
          // remounted (the watchdog then fires on the dead instance —
          // round-25 should-fix-24-2): notify whichever instance is mounted
          // now so the banner and the latch releases are not lost; the
          // listener re-checks the live task mirror.
          notifyTaskListeners(discardStuckListenersByTask, sessionId);
          // Surface the stuck state on the panel still showing this task,
          // keyed on the live task mirror — NOT on the generation (round-23
          // MAJOR-3): an A→B→A round-trip re-awaits the still-pending discard
          // under a fresh generation, and a generation gate here would
          // suppress the banner and leave that rebind's bindingPending
          // uncleared — the eternal "preparing" state the watchdog exists to
          // break. A panel showing another task picks the marker up through
          // the rebind effect's mirror instead.
          if (sessionIdRef.current !== sessionId) return;
          setDiscardStuck(true);
          // Release the dead restart latches, or New Topic stays disabled
          // behind `restarting` and the re-arm the marker grants is
          // unreachable (the binding hint would sit there forever too).
          setRestarting(false);
          setBindingPending(false);
        }, DISCARD_WATCHDOG_MS);
        try {
          await discardPromise;
        } finally {
          clearTimeout(watchdog);
          // Entry and stuck marker clear by promise identity only: once a
          // re-armed restart owns the registry slot, the orphaned discard's
          // late settle must not touch either — any marker there belongs to
          // the fresh discard. The banner state follows only when a marker
          // was actually cleared AND this panel still shows the task (the
          // settle twin of the watchdog's sessionIdRef gate, round-24
          // minor-7): with two stuck discards (A and B), A's late settle
          // deletes A's marker while the panel shows B — an ungated clear
          // would hide a stuck state the module marker still records. The
          // rebind effect re-mirrors the marker in both directions.
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
        // binding nulled at entry (round-18 B-1) must be restored — otherwise
        // the panel sits send-dead behind the discardFailed copy that says the
        // current topic is still usable (round-20 Major-2), and the only
        // in-panel "retry" (New Topic) would destroy the perfectly alive
        // session. ensure is idempotent and returns that same session; if the
        // restore itself fails, surface the binding-lost state instead. The
        // restore is awaited so the outer finally cannot release the
        // restarting latch before the binding is back.
        if (generationRef.current !== generation) return;
        setDiscardFailed(true);
        try {
          const restoredAuxId = await withSettleBound(auxChat.ensure(sessionId));
          if (generationRef.current !== generation) return;
          auxIdRef.current = restoredAuxId;
          setAuxId(restoredAuxId);
          pullSnapshot(restoredAuxId);
          // The discard failed, so this restore re-bound the SAME live
          // transcript the pre-restart send was delivered into — the
          // keep-draft premise ("the delivery was destroyed") no longer
          // holds for an ack settling under the restart's changed epoch
          // (round-25 MAJOR-24-3). The single-shot marker falls that ack
          // through to normal consumption; with no ack pending, the
          // delivered draft is consumed right here instead.
          restartDiscardFailedByTask.add(sessionId);
          consumeDeliveredDraftAfterFailedRestart(sessionId);
        } catch (restoreError) {
          console.warn('[pinvou3][aux-chat] restore after discard failure failed', restoreError);
          if (generationRef.current !== generation) return;
          setEnsureFailed(true);
        }
        return;
      }
      // The binding may have changed during the discard round trip: never
      // issue an ensure for the old sessionId now, or the backend would
      // idempotently recreate the aux session that was just discarded (the UI
      // refuses to bind it, but the record already landed on disk).
      if (generationRef.current !== generation) return;
      try {
        const nextAuxId = await withSettleBound(auxChat.ensure(sessionId));
        if (generationRef.current !== generation) return;
        auxIdRef.current = nextAuxId;
        setAuxId(nextAuxId);
        setSnapshot(normalizeAuxSnapshot(nextAuxId ? auxChat.snapshot(nextAuxId) : null));
        // The draft deliberately survives a new topic: the text was typed but
        // never sent, so discarding it would silently eat user input.
        setSendFailed(false);
        setEnsureFailed(false);
      } catch (error) {
        console.warn('[pinvou3][aux-chat] restart ensure failed', error);
        if (generationRef.current !== generation) return;
        // When the discard succeeded but the ensure rebuild fails, the old
        // auxId points at a deleted session: the binding must be cleared so
        // the composer is honestly disabled; show ensureFailed ("recover via
        // new topic"), not sendFailed's "retry send" — a send can no longer
        // succeed at all.
        auxIdRef.current = null;
        setAuxId(null);
        setSnapshot(normalizeAuxSnapshot(null));
        setEnsureFailed(true);
      }
    } finally {
      // A stale continuation must not clear a newer restart's latch: without
      // the generation check, R1's finally would re-enable the composer and
      // the new-topic button while R2's discard is still in flight, letting a
      // send slip into the session being discarded. The rebind effect resets
      // the flag itself, so skipping the reset here cannot leak the state.
      if (generationRef.current === generation) {
        setRestarting(false);
        setBindingPending(false);
      }
    }
  // pullSnapshot is a real dependency of the discard-failure restore above;
  // oxlint's memo-dependencies rule misses the reference inside the catch
  // while eslint exhaustive-deps requires it — keep the dep, exempt oxlint.
  // oxlint-disable-next-line react/memo-dependencies -- referenced in the discard-failure restore
  }, [auxChat, sessionId, restartArmed, restarting, pullSnapshot]);

  const composerDisabled = !auxChat || !auxId || busy || restarting;

  return (
    <RightDockPanel
      panelId="aux-chat"
      activationKey={activationKey}
      onActiveChange={onActiveChange}
      className="border-l border-black/[0.06] bg-white/92 backdrop-blur-xl dark:border-white/[0.07] dark:bg-[#17181A]/96"
      dataTestId="aux-chat-panel"
    >
      <div className="h-14 shrink-0 px-3 flex items-center gap-2 border-b border-black/[0.05] dark:border-white/[0.06]">
        <MessageSquare size={15} className="shrink-0 text-gray-400" />
        <span className="min-w-0 flex-1 truncate text-[13px] font-semibold">{copy.panelTitle}</span>
        <button
          type="button"
          data-testid="aux-chat-new-topic"
          onClick={() => { void handleRestart(); }}
          disabled={!auxChat || restarting}
          className={`shrink-0 h-7 rounded-lg px-2 flex items-center gap-1 text-[11px] transition-colors ${
            restartArmed
              ? 'bg-red-500/10 text-red-600 dark:text-red-400'
              : 'text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]'
          }`}
          aria-label={restartArmed ? copy.newTopicConfirm : copy.newTopic}
          aria-pressed={restartArmed}
          title={restartArmed ? copy.newTopicConfirm : copy.newTopic}
        >
          <RotateCcw size={13} />
          <span>{restartArmed ? copy.newTopicConfirm : copy.newTopic}</span>
        </button>
        <button
          type="button"
          data-testid="aux-chat-close"
          onClick={onClose}
          className="w-7 h-7 shrink-0 rounded-lg flex items-center justify-center text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]"
          aria-label={copy.close}
        >
          <X size={14} />
        </button>
      </div>

      <div ref={scrollRef} className="custom-scrollbar flex-1 min-h-0 overflow-y-auto px-4 py-4">
        {!hasContent && (
          <div className="space-y-2">
            <div className="rounded-xl border border-black/[0.05] bg-black/[0.02] px-3 py-2.5 text-[12px] leading-5 text-gray-500 dark:border-white/[0.07] dark:bg-white/[0.03] dark:text-gray-400">
              {copy.landingHint}
            </div>
            <div className="px-1 text-[12px] text-gray-400" role="status">
              {bindingPending ? copy.bindingHint : copy.emptyState}
            </div>
          </div>
        )}
        {hasContent && (
          <ConversationTimeline
            turns={turns}
            now={0}
            copy={conversationCopy}
          />
        )}
      </div>

      <div className="shrink-0 border-t border-black/[0.05] px-3 py-3 dark:border-white/[0.06]">
        {busy && (
          <div data-testid="aux-chat-busy-hint" className="mb-2 text-[11px] text-gray-400" role="status">{copy.busyHint}</div>
        )}
        {sendInFlight && !busy && (
          <div data-testid="aux-chat-busy-hint" className="mb-2 text-[11px] text-gray-400" role="status">{copy.sendingHint}</div>
        )}
        {sendFailed && (
          <div className="mb-2 text-[11px] text-red-600 dark:text-red-400" role="alert">{copy.sendFailed}</div>
        )}
        {ensureFailed && (
          <div className="mb-2 text-[11px] text-red-600 dark:text-red-400" role="alert">{copy.ensureFailed}</div>
        )}
        {discardFailed && (
          <div className="mb-2 text-[11px] text-red-600 dark:text-red-400" role="alert">{copy.discardFailed}</div>
        )}
        {discardStuck && (
          <div data-testid="aux-chat-discard-stuck" className="mb-2 text-[11px] text-red-600 dark:text-red-400" role="alert">{copy.discardStuck}</div>
        )}
        <div className={`rounded-xl border px-3 py-2 ${
          theme === 'dark' ? 'border-white/[0.08] bg-white/[0.03]' : 'border-black/[0.08] bg-white/60'
        }`}>
          {quotes.length > 0 && (
            <div data-testid="aux-quote-chips" className="mb-2 space-y-1.5">
              <div className="flex items-center gap-1.5 text-[11px] text-gray-400">
                <Quote size={12} className="shrink-0" />
                <span>{copy.quoteChipCount(quotes.length)}</span>
              </div>
              {quotes.map((quote, index) => (
                <div
                  key={`${index}-${quote.text.slice(0, 24)}`}
                  className="group/quote flex items-start gap-1.5 rounded-lg border border-black/[0.05] bg-black/[0.02] px-2 py-1.5 dark:border-white/[0.07] dark:bg-white/[0.04]"
                >
                  <div className="min-w-0 flex-1 line-clamp-3 whitespace-pre-wrap break-words text-[12px] leading-5 text-gray-500 dark:text-gray-400">
                    {quote.text}
                  </div>
                  <button
                    type="button"
                    data-testid="aux-quote-remove"
                    onClick={() => removeAuxQuote(sessionId, index)}
                    className="h-5 w-5 shrink-0 rounded-md flex items-center justify-center text-gray-400 hover:bg-black/[0.06] dark:hover:bg-white/[0.08]"
                    aria-label={copy.quoteRemove}
                    title={copy.quoteRemove}
                  >
                    <X size={11} />
                  </button>
                </div>
              ))}
            </div>
          )}
          <div className="flex items-end gap-2">
          <textarea
            rows={1}
            ref={composerRef}
            value={draft}
            data-testid="aux-chat-input"
            onChange={(event) => {
              // Same 100k cap as the main composer (chat-input-limit.js):
              // drafts persist per task in draftByTask, so an unbounded paste
              // would otherwise live in memory for the SPA's lifetime.
              const next = constrainChatInput(event.target.value).text;
              setDraft(next);
              // Remember per task: a task switch, a close/reopen or a new
              // topic must not silently drop text the user has not sent.
              if (sessionId) draftByTask.set(sessionId, next);
              if (sendFailed) setSendFailed(false);
            }}
            onKeyDown={handleComposerKeyDown}
            placeholder={copy.inputPlaceholder}
            aria-label={copy.inputPlaceholder}
            className="custom-scrollbar max-h-32 flex-1 resize-none bg-transparent text-[13px] leading-5 outline-none placeholder:text-gray-400"
          />
          <button
            type="button"
            data-testid="aux-chat-send"
            onClick={() => { void handleSend(); }}
            disabled={composerDisabled || (!draft.trim() && quotes.length === 0)}
            className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-[#0A84FF] text-white transition-opacity hover:bg-[#1677D2] disabled:opacity-40"
            aria-label={copy.send}
            title={busy ? copy.busyHint : copy.send}
          >
            <Send size={13} />
          </button>
          </div>
        </div>
      </div>
    </RightDockPanel>
  );
}
