import { useCallback, useEffect, useRef, useState } from 'react';
import { Quote } from '../../components/icons.jsx';
import { stageAuxQuote } from './aux-quote.mjs';
import {
  quoteChipPosition,
  resolveDismissedSelection,
  selectionRangeDescriptor,
} from './aux-quote-selection-state.mjs';

/**
 * Selection-to-quote bridge on the MAIN conversation timeline: selecting text
 * inside `containerRef` surfaces a small floating action that stages the
 * excerpt as a pending aux-chat quote for this task and opens the aux panel.
 *
 * Rendering mirrors the existing SelectionCopyButton vocabulary: an absolute
 * floating chip positioned INSIDE the conversation column, clamped to the
 * container's rect (the mount sites mark the container `relative`). A
 * text-selection affordance is not modal and never covers the right dock, so
 * it deliberately stays out of the dock-occlusion registry — registering
 * would hide every dock panel for the duration of a selection. The button is
 * suppressed entirely while the task has no aux chat (null sessionId: no
 * session / sched- runs / external ACP).
 */

export function AuxQuoteSelection({ containerRef, sessionId, copy, onQuote }) {
  const [popover, setPopover] = useState(null);
  const hideTimerRef = useRef(null);
  // The mouseup/keyup evaluation is deferred by one macrotask; without
  // tracking that timer, the mouseup that clicked the quote button schedules
  // an evaluation which runs AFTER handleQuote's hide/error state and
  // resurrects the popover over a successful quote (or overwrites the
  // over-limit error before its window).
  const evaluateTimerRef = useRef(null);
  // Escape dismiss latch: Escape does not collapse the DOM selection, so the
  // always-live keyup evaluation would re-derive the popover from the
  // unchanged selection one macrotask after hidePopover(). The descriptor of
  // the dismissed range suppresses that re-derivation until the selection
  // genuinely changes or collapses (see aux-quote-selection-state.mjs).
  const dismissedRangeRef = useRef(null);

  const hidePopover = useCallback(() => {
    if (hideTimerRef.current) {
      clearTimeout(hideTimerRef.current);
      hideTimerRef.current = null;
    }
    if (evaluateTimerRef.current) {
      clearTimeout(evaluateTimerRef.current);
      evaluateTimerRef.current = null;
    }
    setPopover((current) => (current ? null : current));
  }, []);

  useEffect(() => () => {
    if (hideTimerRef.current) clearTimeout(hideTimerRef.current);
    if (evaluateTimerRef.current) clearTimeout(evaluateTimerRef.current);
  }, []);

  // Evaluate the live selection against the timeline container. Everything
  // runs in a macrotask after mouseup/keyup so the browser has already
  // settled the final selection range.
  const evaluateSelection = useCallback(() => {
    const container = containerRef && containerRef.current;
    if (!sessionId || !container || typeof window === 'undefined' || !window.getSelection) return;
    const selection = window.getSelection();
    const descriptor = selectionRangeDescriptor(selection);
    const latch = resolveDismissedSelection(dismissedRangeRef.current, descriptor);
    dismissedRangeRef.current = latch.dismissed;
    if (latch.suppress) return;
    if (!descriptor) {
      hidePopover();
      return;
    }
    // toString() is layout-backed: a WebView whose window is hidden (suspended
    // rendering) returns an empty string for a perfectly valid range. Fall
    // back to the range's own text, which is pure DOM and always available
    // (rangeCount > 0 is already guaranteed above).
    const text = selection.toString() || selection.getRangeAt(0).toString();
    if (!text || !text.trim()) {
      hidePopover();
      return;
    }
    if (!container.contains(descriptor.anchorNode) || !container.contains(descriptor.focusNode)) {
      hidePopover();
      return;
    }
    const rect = selection.getRangeAt(0).getBoundingClientRect();
    if (!rect) {
      hidePopover();
      return;
    }
    const containerRect = container.getBoundingClientRect();
    if (!containerRect) {
      hidePopover();
      return;
    }
    const { left, top } = quoteChipPosition(containerRect, rect);
    setPopover({ text: text.trim(), left, top });
  }, [containerRef, hidePopover, sessionId]);

  useEffect(() => {
    dismissedRangeRef.current = null;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- one-shot mirror: dismiss the popover synchronously whenever the task changes, so no stale selection from the previous task outlives the switch
    hidePopover();
    if (!sessionId) return;
    const scheduleEvaluation = () => {
      // setTimeout, not rAF: the WebView suspends animation frames while its
      // window is hidden, and a selection made headlessly (automation) would
      // never surface the action; a macrotask fires in every visibility state.
      // The timer is tracked so hidePopover can cancel a pending evaluation
      // that would resurrect the popover.
      evaluateTimerRef.current = setTimeout(() => {
        evaluateTimerRef.current = null;
        evaluateSelection();
      }, 0);
    };
    const onMouseUp = (event) => {
      if (event.button !== 0) return;
      scheduleEvaluation();
    };
    const onKeyUp = () => {
      // No key whitelist: a whitelist misses selection-changing keys outside
      // the shift/arrow family — most importantly Ctrl+A ("a"), the standard
      // keyboard path to select a whole assistant reply. Evaluating on every
      // keyup mirrors the unconditional mouseup handler and is cheap: a
      // collapsed selection just hides (or no-ops) the popover, and a range
      // the user dismissed with Escape is suppressed by the latch.
      scheduleEvaluation();
    };
    document.addEventListener('mouseup', onMouseUp);
    document.addEventListener('keyup', onKeyUp);
    return () => {
      document.removeEventListener('mouseup', onMouseUp);
      document.removeEventListener('keyup', onKeyUp);
    };
  }, [evaluateSelection, hidePopover, sessionId]);

  // Auto-dismiss: outside mousedown, Escape, any scroll, viewport resize, or
  // the selection collapsing (e.g. a click elsewhere or streaming DOM churn).
  useEffect(() => {
    if (!popover) return;
    const onMouseDown = (event) => {
      if (event.target && event.target.closest && event.target.closest('[data-aux-quote-selection]')) return;
      hidePopover();
    };
    const onKeyDown = (event) => {
      if (event.key !== 'Escape') return;
      // Latch the dismissed range BEFORE hiding: the keyup that follows this
      // keydown schedules an evaluation, and Escape leaves the DOM selection
      // intact, so without the latch the chip would reappear immediately.
      dismissedRangeRef.current = selectionRangeDescriptor(
        window.getSelection ? window.getSelection() : null,
      );
      hidePopover();
    };
    const onSelectionChange = () => {
      const selection = window.getSelection ? window.getSelection() : null;
      if (!selection || selection.isCollapsed || selection.rangeCount === 0) hidePopover();
    };
    document.addEventListener('mousedown', onMouseDown, true);
    document.addEventListener('keydown', onKeyDown, true);
    document.addEventListener('selectionchange', onSelectionChange);
    window.addEventListener('scroll', hidePopover, true);
    window.addEventListener('resize', hidePopover);
    return () => {
      document.removeEventListener('mousedown', onMouseDown, true);
      document.removeEventListener('keydown', onKeyDown, true);
      document.removeEventListener('selectionchange', onSelectionChange);
      window.removeEventListener('scroll', hidePopover, true);
      window.removeEventListener('resize', hidePopover);
    };
  }, [hidePopover, popover]);

  const handleQuote = useCallback(() => {
    if (!popover || !sessionId) return;
    const result = stageAuxQuote(sessionId, popover.text);
    if (!result.ok) {
      const errorByReason = {
        single: copy.quoteLimitSingle,
        count: copy.quoteLimitCount,
        total: copy.quoteLimitTotal,
      };
      setPopover({ ...popover, error: errorByReason[result.reason] || copy.quoteLimitSingle });
      if (hideTimerRef.current) clearTimeout(hideTimerRef.current);
      hideTimerRef.current = setTimeout(hidePopover, 1800);
      return;
    }
    if (result.duplicate) {
      // The exact excerpt is already staged: adding it would be a no-op, so
      // say so instead of letting the opening panel read as "added a second
      // chip". The panel still opens — the quote IS part of the next
      // message — and the notice explains why the count did not change.
      setPopover({ ...popover, error: copy.quoteDuplicate });
      if (hideTimerRef.current) clearTimeout(hideTimerRef.current);
      hideTimerRef.current = setTimeout(hidePopover, 1800);
      if (onQuote) onQuote();
      return;
    }
    hidePopover();
    if (onQuote) onQuote();
  }, [copy, hidePopover, onQuote, popover, sessionId]);

  if (!popover) return null;
  return (
    <button
      type="button"
      data-aux-quote-selection="true"
      data-testid="aux-quote-selection-button"
      title={popover.error || copy.quoteAction}
      onMouseDown={(event) => { event.preventDefault(); event.stopPropagation(); }}
      onClick={(event) => { event.preventDefault(); event.stopPropagation(); handleQuote(); }}
      className={`absolute z-40 h-8 max-w-[280px] truncate rounded-[10px] px-3 flex items-center gap-1.5 text-[12px] font-medium shadow-lg backdrop-blur transition-colors ${
        popover.error
          ? 'bg-white text-red-600 border border-red-500/30 dark:bg-[#2B2C2F] dark:text-red-300 dark:border-red-400/30'
          : 'bg-white text-[#1F1F1F] hover:bg-[#F8FAFF] border border-black/10 dark:bg-[#2B2C2F] dark:text-[#E3E3E3] dark:hover:bg-[#34363A] dark:border-white/10'
      }`}
      // margin: 0: the ChatView column is a `space-y-4` stack whose sibling
      // margin rule also matches absolutely positioned children.
      style={{ left: popover.left + 'px', top: popover.top + 'px', margin: 0 }}
    >
      <Quote size={13} className="shrink-0" />
      <span className="truncate">{popover.error || copy.quoteAction}</span>
    </button>
  );
}
