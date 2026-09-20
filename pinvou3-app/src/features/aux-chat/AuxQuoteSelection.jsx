import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Quote } from '../../components/icons.jsx';
import { stageAuxQuote } from './aux-quote.mjs';

/**
 * Selection-to-quote bridge on the MAIN conversation timeline: selecting text
 * inside `containerRef` surfaces a small floating action that stages the
 * excerpt as a pending aux-chat quote for this task and opens the aux panel.
 *
 * Rendering mirrors the existing SelectionCopyButton vocabulary (absolute
 * floating chip, fixed positioning through a body portal so scroll containers
 * cannot clip it). The button is suppressed entirely while the task has no
 * aux chat (null sessionId: no session / sched- runs / external ACP).
 */

const POPOVER_GAP = 8;
const POPOVER_ESTIMATED_WIDTH = 168;

export function AuxQuoteSelection({ containerRef, sessionId, copy, onQuote }) {
  const [popover, setPopover] = useState(null);
  const hideTimerRef = useRef(null);

  const hidePopover = useCallback(() => {
    if (hideTimerRef.current) {
      clearTimeout(hideTimerRef.current);
      hideTimerRef.current = null;
    }
    setPopover((current) => (current ? null : current));
  }, []);

  useEffect(() => () => {
    if (hideTimerRef.current) clearTimeout(hideTimerRef.current);
  }, []);

  // Evaluate the live selection against the timeline container. Everything
  // runs in a rAF after mouseup/keyup so the browser has already settled the
  // final selection range.
  const evaluateSelection = useCallback(() => {
    const container = containerRef && containerRef.current;
    if (!sessionId || !container || typeof window === 'undefined' || !window.getSelection) return;
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0 || selection.isCollapsed) {
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
    const { anchorNode, focusNode } = selection;
    if (!anchorNode || !focusNode || !container.contains(anchorNode) || !container.contains(focusNode)) {
      hidePopover();
      return;
    }
    const range = selection.getRangeAt(0);
    const rect = range.getBoundingClientRect();
    if (!rect) {
      hidePopover();
      return;
    }
    // A hidden/unlaid-out window (minimized, WebView suspended) reports a
    // zero-size rect; the selection itself is still valid, so fall back to a
    // viewport-clamped anchor instead of dropping the quote action.
    const anchorWidth = rect.width > 0 ? rect.width : 120;
    const anchorLeft = rect.width > 0 ? rect.left : window.innerWidth / 2 - anchorWidth / 2;
    const anchorTop = rect.height > 0 ? rect.top : Math.min(window.innerHeight - 60, 120);
    const left = Math.max(
      POPOVER_GAP,
      Math.min(anchorLeft + anchorWidth / 2 - POPOVER_ESTIMATED_WIDTH / 2, window.innerWidth - POPOVER_ESTIMATED_WIDTH - POPOVER_GAP),
    );
    const top = Math.max(POPOVER_GAP, anchorTop - 36 - POPOVER_GAP);
    setPopover({ text: text.trim(), left, top });
  }, [containerRef, hidePopover, sessionId]);

  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- one-shot mirror: dismiss the popover synchronously whenever the task changes, so no stale selection from the previous task outlives the switch
    hidePopover();
    if (!sessionId) return;
    const onMouseUp = (event) => {
      if (event.button !== 0) return;
      // setTimeout, not rAF: the WebView suspends animation frames while its
      // window is hidden, and a selection made headlessly (automation) would
      // never surface the action; a macrotask fires in every visibility state.
      setTimeout(evaluateSelection, 0);
    };
    const onKeyUp = () => {
      // No key whitelist: a whitelist misses selection-changing keys outside
      // the shift/arrow family — most importantly Ctrl+A ("a"), the standard
      // keyboard path to select a whole assistant reply. Evaluating on every
      // keyup mirrors the unconditional mouseup handler and is cheap: a
      // collapsed selection just hides (or no-ops) the popover.
      setTimeout(evaluateSelection, 0);
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
      if (event.key === 'Escape') hidePopover();
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
    hidePopover();
    if (onQuote) onQuote();
  }, [copy, hidePopover, onQuote, popover, sessionId]);

  if (!popover) return null;
  return createPortal(
    <button
      type="button"
      data-aux-quote-selection="true"
      data-testid="aux-quote-selection-button"
      title={popover.error || copy.quoteAction}
      onMouseDown={(event) => { event.preventDefault(); event.stopPropagation(); }}
      onClick={(event) => { event.preventDefault(); event.stopPropagation(); handleQuote(); }}
      className={`fixed z-40 h-8 max-w-[280px] truncate rounded-[10px] px-3 flex items-center gap-1.5 text-[12px] font-medium shadow-lg backdrop-blur transition-colors ${
        popover.error
          ? 'bg-white text-red-600 border border-red-500/30 dark:bg-[#2B2C2F] dark:text-red-300 dark:border-red-400/30'
          : 'bg-white text-[#1F1F1F] hover:bg-[#F8FAFF] border border-black/10 dark:bg-[#2B2C2F] dark:text-[#E3E3E3] dark:hover:bg-[#34363A] dark:border-white/10'
      }`}
      style={{ left: popover.left + 'px', top: popover.top + 'px' }}
    >
      <Quote size={13} className="shrink-0" />
      <span className="truncate">{popover.error || copy.quoteAction}</span>
    </button>,
    document.body,
  );
}
