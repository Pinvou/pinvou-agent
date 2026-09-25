import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { MessageSquare, Quote, RotateCcw, Send, X } from '../../components/icons.jsx';
import { RightDockPanel } from '../../components/layout/RightDock.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import { constrainChatInput } from '../chat/chat-input-limit.js';
import { ConversationTimeline } from '../conversation/ConversationTimeline.jsx';
import { transitionConversationScrollState } from '../conversation/conversation-scroll.js';
import {
  auxChatBusy,
  auxChatHasContent,
  projectAuxChatTurns,
} from './aux-chat-state.mjs';
import { createAuxChatController, removeAuxQuote } from './aux-chat-controller.mjs';

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
 *
 * This file is the thin React adapter: rendering, scroll management,
 * composer auto-grow and the useState/useEffect wiring. Every async state
 * transition (the send/restart guards, the in-flight registries, the
 * watchdogs, the ack/discard interleaving logic) lives in the pure
 * controller — extracted in round-30 B1 so the interleavings are covered by
 * executing tests (tests/aux_chat_controller.test.mjs). The registries are
 * module-scoped ON PURPOSE (they track backend-scoped operations that
 * outlive any panel instance), so the panel module holds exactly one
 * controller singleton shared by every mounted panel.
 */
const auxChatController = createAuxChatController();

export function AuxChatPanel({ sessionId, activationKey, t, theme, onClose, onActiveChange }) {
  const copy = t.uiAuxChat;
  const conversationCopy = t.uiConversation;
  const auxChat = bridge.available ? bridge.auxChat : null;

  // One controller panel per mounted instance; the async machine lives
  // inside it and the component only mirrors its view into React state.
  // useState's lazy initializer keeps the instance stable across renders.
  const [panel] = useState(() => auxChatController.createPanel());
  const [view, setView] = useState(panel.view);
  useEffect(() => panel.subscribe(setView), [panel]);
  useEffect(() => () => panel.dispose(), [panel]);

  // First open and main-session rebind: refresh the bridge (bridge.available
  // flips once at bootstrap, and the controller's chat-domain subscription
  // re-runs with it) and bind the new task's aux session. The round-30 D2
  // recovery re-bind runs inside the controller (the stuck-notify listener
  // re-invokes bind), so no re-run token reaches this effect.
  useEffect(() => {
    panel.setBridge(
      auxChat,
      (callback) => bridge.state.subscribeMany(['chat'], callback),
    );
    panel.bind(sessionId);
  }, [panel, auxChat, sessionId]);

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

  const busy = auxChatBusy(view.snapshot);
  const hasContent = auxChatHasContent(view.snapshot);
  const sendInFlight = view.sendInFlight;
  const turns = useMemo(
    () => (view.auxId ? projectAuxChatTurns(view.snapshot, view.auxId) : []),
    [view.snapshot, view.auxId],
  );

  // Composer auto-grow, same mechanism as the main conversation composer:
  // grow with the content up to the max-h-32 cap (then scroll internally) and
  // shrink back when the draft clears. Re-runs on rebind so a restored draft
  // sizes the box immediately.
  useEffect(() => {
    const el = composerRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = Math.min(Math.max(el.scrollHeight, 24), 128) + 'px';
  }, [view.draft, sessionId]);

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
  }, [view.auxId]);

  // A rebind always opens the (fresh or switched) aux transcript at its tail.
  useEffect(() => {
    autoScrollRef.current = true;
  }, [view.auxId]);

  // Snap to the tail on any content growth — new turns and streaming deltas
  // alike (a delta re-pulls the snapshot, so depending on the snapshot covers
  // both) — but only while following: the old unconditional snap yanked a
  // scrolled-up reader on every new item (round-20 minor-6).
  useEffect(() => {
    const el = scrollRef.current;
    if (el && autoScrollRef.current) el.scrollTop = el.scrollHeight;
  }, [view.auxId, view.snapshot]);

  const handleComposerKeyDown = useCallback((event) => {
    if (event.repeat) return;
    if (event.key !== 'Enter' || event.shiftKey || isImeComposing(event)) return;
    event.preventDefault();
    void panel.send();
  }, [panel]);

  // sendInFlight included (round-29 M2): the send latch already makes Enter
  // a silent no-op through the dispatch window; an enabled-looking button
  // doing the same just hid that state.
  const composerDisabled = !auxChat || !view.auxId || busy || view.restarting || sendInFlight;

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
          onClick={() => { void panel.restart(); }}
          disabled={!auxChat || view.restarting}
          className={`shrink-0 h-7 rounded-lg px-2 flex items-center gap-1 text-[11px] transition-colors ${
            view.restartArmed
              ? 'bg-red-500/10 text-red-600 dark:text-red-400'
              : 'text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]'
          }`}
          aria-label={view.restartArmed ? copy.newTopicConfirm : copy.newTopic}
          aria-pressed={view.restartArmed}
          title={view.restartArmed ? copy.newTopicConfirm : copy.newTopic}
        >
          <RotateCcw size={13} />
          <span>{view.restartArmed ? copy.newTopicConfirm : copy.newTopic}</span>
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
              {view.bindingPending ? copy.bindingHint : copy.emptyState}
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
        {view.sendFailed && (
          <div className="mb-2 text-[11px] text-red-600 dark:text-red-400" role="alert">{copy.sendFailed}</div>
        )}
        {view.ensureFailed && (
          <div className="mb-2 text-[11px] text-red-600 dark:text-red-400" role="alert">{copy.ensureFailed}</div>
        )}
        {view.discardFailed && (
          <div className="mb-2 text-[11px] text-red-600 dark:text-red-400" role="alert">{copy.discardFailed}</div>
        )}
        {view.discardStuck && (
          <div data-testid="aux-chat-discard-stuck" className="mb-2 text-[11px] text-red-600 dark:text-red-400" role="alert">{copy.discardStuck}</div>
        )}
        <div className={`rounded-xl border px-3 py-2 ${
          theme === 'dark' ? 'border-white/[0.08] bg-white/[0.03]' : 'border-black/[0.08] bg-white/60'
        }`}>
          {view.quotes.length > 0 && (
            <div data-testid="aux-quote-chips" className="mb-2 space-y-1.5">
              <div className="flex items-center gap-1.5 text-[11px] text-gray-400">
                <Quote size={12} className="shrink-0" />
                <span>{copy.quoteChipCount(view.quotes.length)}</span>
              </div>
              {view.quotes.map((quote, index) => (
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
            value={view.draft}
            data-testid="aux-chat-input"
            onChange={(event) => {
              // Same 100k cap as the main composer (chat-input-limit.js):
              // drafts persist per task in the controller, so an unbounded
              // paste would otherwise live in memory for the SPA's lifetime.
              panel.setDraftText(constrainChatInput(event.target.value).text);
            }}
            onKeyDown={handleComposerKeyDown}
            placeholder={copy.inputPlaceholder}
            aria-label={copy.inputPlaceholder}
            className="custom-scrollbar max-h-32 flex-1 resize-none bg-transparent text-[13px] leading-5 outline-none placeholder:text-gray-400"
          />
          <button
            type="button"
            data-testid="aux-chat-send"
            onClick={() => { void panel.send(); }}
            disabled={composerDisabled || (!view.draft.trim() && view.quotes.length === 0)}
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
