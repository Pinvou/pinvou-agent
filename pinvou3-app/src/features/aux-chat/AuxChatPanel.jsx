import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { MessageSquare, Quote, RotateCcw, Send, X } from '../../components/icons.jsx';
import { RightDockPanel } from '../../components/layout/RightDock.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import { ConversationTimeline } from '../conversation/ConversationTimeline.jsx';
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

// taskId -> pending discard promise, module-scoped on purpose: the discard it
// tracks is backend-scoped (the turn gate can hold it for seconds), while the
// panel unmounts on close and on sched- session switches. A component-level
// registry would die with the instance and let a remounted panel rebind to
// the still-mapped aux session the in-flight discard then deletes — the exact
// M-B hole re-opened through unmount/remount. Entries are removed when the
// discard settles, so the map holds at most one pending promise per task and
// only for the discard's lifetime.
const discardInFlightByTask = new Map();

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
  const scrollRef = useRef(null);
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
    // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously reset binding state on session switch; one-shot mirror, same pattern as SubagentTranscriptPanel
    setAuxId(null);
    setSnapshot(normalizeAuxSnapshot(null));
    setSendFailed(false);
    setEnsureFailed(false);
    setDiscardFailed(false);
    setRestartArmed(false);
    // Reset restarting on every rebind: the two restart invokes have no
    // transport timeout, so a promise that never settles would otherwise
    // latch the new task's panel disabled forever (the old restart's finally
    // can no longer be relied on once its generation went stale).
    setRestarting(false);
    // Same latch class for sends: a never-settling auxChat.send invoke must
    // not permanently block sends across later task rebinds either.
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
    // (errors surface on the restart path) and re-check the generation so a
    // further rebind during the wait aborts this ensure entirely.
    const pendingDiscard = discardInFlightByTask.get(sessionId);
    const ensureAfterDiscard = () => {
      if (disposed || generationRef.current !== generation) return;
      auxChat.ensure(sessionId)
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
    if (pendingDiscard) {
      pendingDiscard.then(ensureAfterDiscard, ensureAfterDiscard);
    } else {
      ensureAfterDiscard();
    }
    return () => { disposed = true; };
  }, [auxChat, sessionId, pullSnapshot]);

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
  const turns = useMemo(
    () => (auxId ? projectAuxChatTurns(snapshot, auxId) : []),
    [snapshot, auxId],
  );

  // Snap to the bottom when a new turn appears; streaming deltas do not force
  // scrolling, to avoid interrupting a user reading back through history.
  const itemCount = snapshot.chatItems.length;
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [auxId, itemCount]);

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
    // Two independent staleness boundaries: the generation is the restart
    // boundary and the auxId is the rebind boundary. A restart on the same
    // task re-binds to a *new* aux, so a send issued before it must surface
    // nothing at all — neither the contradictory "retry send" banner next to
    // ensureFailed, nor a draft clear that would eat text typed since.
    const sentGeneration = generationRef.current;
    const sentTaskId = sessionId;
    if (!auxChat || !sentAuxId || (!text && !quoteBlock) || busy || restarting || sendingRef.current) return;
    sendingRef.current = true;
    setSending(true);
    setSendFailed(false);
    try {
      await auxChat.send(sentAuxId, quoteBlock ? text + quoteBlock : text);
      // The message was delivered to sentAuxId. A switch away and back
      // re-binds the *same* aux id (ensure is idempotent) and aux ids are
      // 1:1 with tasks, so when the binding still resolves to sentAuxId the
      // visible composer shows this task — consumption must proceed even
      // across a generation bump, or the composer keeps the delivered text
      // and the next Enter re-sends it with its quote block (round-14 B3).
      // The restart case (re-bind to a *new* aux) is excluded by this
      // binding check, so the generation guard is not needed here.
      if (auxIdRef.current !== sentAuxId) return;
      if (sentTaskId) {
        // Consume only what was actually sent: text typed after this send
        // started belongs to the next message, and quotes staged from the
        // main view during the in-flight window survive the success
        // callback (they were not part of the captured quote block).
        const storedDraft = draftByTask.get(sentTaskId);
        if (storedDraft === undefined || storedDraft.trim() === text) {
          draftByTask.delete(sentTaskId);
        }
        dropAuxQuotes(sentTaskId, quotes);
      }
      setDraft((current) => (current.trim() === text ? '' : current));
      pullSnapshot(auxIdRef.current);
    } catch (error) {
      console.warn('[pinvou3][aux-chat] send failed', error);
      if (generationRef.current !== sentGeneration) return;
      if (auxIdRef.current !== sentAuxId) return;
      setSendFailed(true);
    } finally {
      // The latch may be released only by the exact send that set it: a
      // same-id rebind (switch away and back) keeps auxIdRef equal to
      // sentAuxId, so a binding-only gate would let a stale send's late
      // finally clear the latch a newer send on the rebound task relies on
      // (round-14 B2). The rebind effect resets the latch on every switch,
      // so refusing here is safe.
      if (auxIdRef.current === sentAuxId && generationRef.current === sentGeneration) {
        sendingRef.current = false;
        setSending(false);
      }
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
    // precisely the fresh session this action asks for, so stop here.
    if (discardInFlightByTask.has(sessionId)) return;
    if (!restartArmed) {
      setRestartArmed(true);
      return;
    }
    setRestartArmed(false);
    setRestarting(true);
    setDiscardFailed(false);
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
    // Feedback for the discard+ensure window (the backend turn gate can hold
    // the discard for seconds, and `restarting` only disables controls): the
    // composer and timeline would otherwise just sit there with no hint that a
    // new topic is being prepared.
    setBindingPending(true);
    // Bump the generation at restart entry: only the rebind effect increments
    // it otherwise, so an ensure issued by the current rebind that is still in
    // flight (including its ensureSessionBufferLoaded chain) would resolve
    // after this restart's discard+ensure with a matching generation and
    // rebind the panel to the just-discarded aux session. Advancing the
    // generation here makes every such stale continuation inert.
    generationRef.current += 1;
    const generation = generationRef.current;
    try {
      try {
        // Register the in-flight discard by task id: while its backend turn
        // gate waits out a running turn, the old mapping is still live, and
        // the rebind effect must await this promise before re-ensuring the
        // same task — otherwise it would bind the doomed aux session that
        // this discard deletes behind its back.
        const discardPromise = auxChat.discard(sessionId);
        discardInFlightByTask.set(sessionId, discardPromise);
        try {
          await discardPromise;
        } finally {
          if (discardInFlightByTask.get(sessionId) === discardPromise) {
            discardInFlightByTask.delete(sessionId);
          }
        }
      } catch (error) {
        console.warn('[pinvou3][aux-chat] restart discard failed', error);
        // Discard failed: the old aux session is still fully usable, the
        // binding and snapshot stay as-is, and only a retry hint is shown
        // (unlike an ensure failure — that is the binding-lost situation that
        // must be recovered via a new topic).
        if (generationRef.current !== generation) return;
        setDiscardFailed(true);
        return;
      }
      // The binding may have changed during the discard round trip: never
      // issue an ensure for the old sessionId now, or the backend would
      // idempotently recreate the aux session that was just discarded (the UI
      // refuses to bind it, but the record already landed on disk).
      if (generationRef.current !== generation) return;
      try {
        const nextAuxId = await auxChat.ensure(sessionId);
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
  }, [auxChat, sessionId, restartArmed, restarting]);

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
        {sending && !busy && (
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
            value={draft}
            data-testid="aux-chat-input"
            onChange={(event) => {
              const next = event.target.value;
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
