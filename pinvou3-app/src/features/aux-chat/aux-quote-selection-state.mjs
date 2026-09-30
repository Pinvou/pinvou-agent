/**
 * Pure decision layer for AuxQuoteSelection.jsx: where the quote chip lands
 * inside the conversation column, and the Escape-dismiss latch.
 *
 * Everything here is DOM-free (selection endpoints are opaque tokens compared
 * by identity, rects are plain objects) so the executing tests can drive the
 * decisions without a browser; the JSX only wires listeners and state.
 */

export const QUOTE_CHIP_GAP = 8;
export const QUOTE_CHIP_ESTIMATED_WIDTH = 168;
export const QUOTE_CHIP_HEIGHT = 36;

/**
 * Chip position relative to the conversation container (the element the chip
 * is absolutely positioned inside). The horizontal clamp keeps the chip
 * inside the column, so it can never overlap the right dock — the property
 * that lets a selection affordance stay out of the dock-occlusion registry.
 * A zero-size range rect (hidden/suspended WebView: the selection is still
 * valid but layout is not) falls back to a container-centered anchor instead
 * of dropping the quote action.
 *
 * @param {{ left: number, top: number, width: number, height: number }} containerRect
 * @param {{ left: number, top: number, width: number, height: number } | null} rangeRect
 * @returns {{ left: number, top: number }}
 */
export function quoteChipPosition(containerRect, rangeRect) {
  const hasWidth = rangeRect && rangeRect.width > 0;
  const hasHeight = rangeRect && rangeRect.height > 0;
  const anchorWidth = hasWidth ? rangeRect.width : 120;
  const anchorLeft = hasWidth
    ? rangeRect.left - containerRect.left
    : containerRect.width / 2 - anchorWidth / 2;
  const anchorTop = hasHeight
    ? rangeRect.top - containerRect.top
    : Math.min(containerRect.height - 60, 120);
  const left = Math.max(
    QUOTE_CHIP_GAP,
    Math.min(
      anchorLeft + anchorWidth / 2 - QUOTE_CHIP_ESTIMATED_WIDTH / 2,
      containerRect.width - QUOTE_CHIP_ESTIMATED_WIDTH - QUOTE_CHIP_GAP,
    ),
  );
  const top = Math.max(QUOTE_CHIP_GAP, anchorTop - QUOTE_CHIP_HEIGHT - QUOTE_CHIP_GAP);
  return { left, top };
}

/**
 * Identity of the current selection range: the anchor/focus endpoints. Nodes
 * are compared by identity only, so plain objects stand in for DOM nodes in
 * tests. Returns null for a missing or collapsed selection.
 */
export function selectionRangeDescriptor(selection) {
  if (!selection || selection.rangeCount === 0 || selection.isCollapsed) return null;
  const { anchorNode, anchorOffset, focusNode, focusOffset } = selection;
  if (!anchorNode || !focusNode) return null;
  return { anchorNode, anchorOffset, focusNode, focusOffset };
}

export function sameRangeDescriptor(left, right) {
  if (!left || !right) return false;
  return left.anchorNode === right.anchorNode
    && left.anchorOffset === right.anchorOffset
    && left.focusNode === right.focusNode
    && left.focusOffset === right.focusOffset;
}

/**
 * Escape-dismiss latch state machine. Escape does not collapse a DOM text
 * selection, so the always-live mouseup/keyup evaluation would re-derive the
 * chip from the unchanged selection one macrotask after hidePopover(). The
 * latch records the range that was dismissed; while the current selection
 * still equals it the evaluation is suppressed. The latch clears as soon as
 * the selection genuinely changes (a new range — re-selecting counts) or
 * collapses, so the chip is only ever suppressed for the exact gesture the
 * user dismissed.
 *
 * @param {object | null} dismissed - latched descriptor from the Escape dismiss
 * @param {object | null} current - descriptor of the live selection (null when collapsed)
 * @returns {{ dismissed: object | null, suppress: boolean }}
 */
export function resolveDismissedSelection(dismissed, current) {
  if (!dismissed) return { dismissed: null, suppress: false };
  if (!current) return { dismissed: null, suppress: false };
  if (sameRangeDescriptor(dismissed, current)) return { dismissed, suppress: true };
  return { dismissed: null, suppress: false };
}
