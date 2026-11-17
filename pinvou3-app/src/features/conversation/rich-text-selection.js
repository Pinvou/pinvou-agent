import { isCodeCopyControl } from './code-block-copy.js';
import { createTextOffsetMapper } from './rich-text-offsets.js';

// Coordinates use DOM text (UTF-16, like Range), excluding renderer-owned
// controls. They survive Markdown structure changes and highlight/fade spans.
function indexText(root) {
  const entries = [];
  const boundaries = new Map();
  let text = '';
  const roles = [];
  const excluded = [];
  const literal = [];
  const visit = node => {
    if (isCodeCopyControl(node)) return;
    if (node.nodeType === 1 && node.nodeName === 'IMG') {
      roles.push({ kind: 'image', start: text.length, end: text.length,
        label: node.getAttribute('alt') || '', destination: node.getAttribute('src') || '' });
    }
    if (node.nodeType === 3) {
      const entry = { node, start: text.length, end: text.length + node.length };
      entries.push(entry);
      boundaries.set(node, entry);
      text += node.data;
      return;
    }
    const start = text.length;
    const link = node.nodeType === 1 && node.nodeName === 'A'
      ? { kind: 'link', start, end: start, label: '', destination: node.getAttribute('href') || '' } : null;
    if (link) roles.push(link);
    const offsets = [start];
    for (const child of node.childNodes) {
      visit(child);
      offsets.push(text.length);
    }
    boundaries.set(node, offsets);
    if (link) { link.end = text.length; link.label = text.slice(start); }
    if (node.nodeType === 1 && ['A', 'CODE', 'PRE'].includes(node.nodeName)) excluded.push({ start, end: text.length });
    if (node.nodeType === 1 && ['CODE', 'PRE'].includes(node.nodeName)) literal.push({ start, end: text.length });
  };
  visit(root);
  excluded.sort((left, right) => left.start - right.start || right.end - left.end);
  literal.sort((left, right) => left.start - right.start || right.end - left.end);
  return { entries, boundaries, text, roles, excluded, literal };
}

function readPoint(root, index, node, offset) {
  if (!root.contains(node)) return { node, offset, outside: true };
  const position = index.boundaries.get(node);
  if (!position) return null;
  const absolute = node.nodeType === 3 ? position.start + offset : position[offset];
  const tablePart = (node.nodeType === 3 ? node.parentElement : node)?.closest?.('pre, td, th, caption, table');
  const bounds = tablePart && index.boundaries.get(tablePart);
  return absolute === undefined ? null : {
    node, offset, absolute, outside: false, text: node.nodeType === 3 ? node.data : null,
    tablePart, partOffset: bounds ? absolute - bounds[0] : null,
    partText: bounds ? index.text.slice(bounds[0], bounds.at(-1)) : null,
  };
}

function resolvePoint(root, index, point, mapOffset, isStart) {
  if (point.outside) return point;
  const preferNext = point.text === null ? isStart
    : point.offset === 0 || (point.offset < point.text.length && isStart);
  const absolute = mapOffset(point.absolute, preferNext, point);
  // Prefer the original text node at block boundaries. Moving an endpoint
  // from the next paragraph's start to the previous one's end changes the
  // browser's selected line breaks even though its text offset is identical.
  const original = index.boundaries.get(point.node);
  if (point.node.nodeType === 3 && original && point.node.data === point.text
    && absolute === original.start + point.offset) {
    // A node can be reused for unrelated new text. It may carry the already
    // resolved coordinate, but must never change which occurrence is selected.
    return { node: point.node, offset: absolute - original.start };
  }
  // Moving a table/code part after fostered prose can change the native line
  // break represented by its element boundary. Resolve its retained characters
  // to text points when the part's global coordinate moved.
  const movedSourcePart = point.tablePart && absolute !== point.absolute;
  if (Array.isArray(original) && original[point.offset] === absolute && !movedSourcePart) return point;
  // At a moved part's opening, the preceding fostered prose ends at the
  // same coordinate. Keep the endpoint inside its retained source part.
  const partBounds = movedSourcePart && index.boundaries.get(point.tablePart);
  const entries = Array.isArray(partBounds) && absolute >= partBounds[0] && absolute <= partBounds.at(-1)
    ? index.entries.filter(entry => point.tablePart.contains(entry.node)) : index.entries;
  for (const entry of entries) {
    if (absolute < entry.end || (absolute === entry.end && !preferNext)) {
      return { node: entry.node, offset: Math.max(0, absolute - entry.start) };
    }
  }
  const last = entries[entries.length - 1];
  return last ? { node: last.node, offset: last.node.length } : { node: root, offset: 0 };
}

// HTML foster parenting inserts appended prose before an existing table.
// Restrict alignment to the original retained part so its own growing text or
// closing Markdown cannot compete with new prose outside that part.
function retainedTablePoints(before, after, anchor, focus, selectedRange, context) {
  if (typeof context.previousSource !== 'string' || !context.source?.startsWith(context.previousSource)) return [];
  const points = [];
  const mappings = new Map();
  for (const point of [anchor, focus]) {
    const bounds = point.tablePart && after.boundaries.get(point.tablePart);
    if (!Array.isArray(bounds)) continue;
    const partStart = point.absolute - point.partOffset;
    const length = point.partText.length;
    points.push({ before: point.absolute, identity: point, resolve: mask => {
      if (!mappings.has(point.tablePart)) {
        const range = { start: Math.max(0, selectedRange.start - partStart),
          end: Math.min(length, selectedRange.end - partStart) };
        const literal = before.literal.filter(span => span.end > partStart && span.start < partStart + length)
          .map(span => ({ start: Math.max(0, span.start - partStart), end: Math.min(length, span.end - partStart) }));
        mappings.set(point.tablePart, createTextOffsetMapper(point.partText, after.text.slice(bounds[0], bounds.at(-1)), range, {
          retirementMask: mask.subarray(partStart, partStart + length), before: { literal },
        }));
      }
      const map = mappings.get(point.tablePart);
      return bounds[0] + map(point.partOffset, point.absolute === selectedRange.start);
    } });
  }
  return points;
}

// External endpoints are clipped only for internal alignment. Their ordering
// sentinels track the new root boundaries; the external DOM points never move.
// A zero-length clipped range must not override its real internal endpoint.
function retainedExternalBoundaries(anchor, focus, start, end, afterLength, backward) {
  if (start === end) return [];
  const points = [];
  if (anchor.outside) points.push({ before: start, after: backward ? afterLength : 0 });
  if (focus.outside) points.push({ before: end, after: backward ? 0 : afterLength });
  return points;
}

/** Preserve native selection, including backward and cross-message ranges. */
export function preserveRichTextSelection(root, update, context = {}) {
  const document = root?.ownerDocument;
  const selection = document?.getSelection?.();
  if (!selection || selection.rangeCount !== 1
    || (!root.contains(selection.anchorNode) && !root.contains(selection.focusNode))) return update();
  const before = indexText(root);
  const anchor = readPoint(root, before, selection.anchorNode, selection.anchorOffset);
  const focus = readPoint(root, before, selection.focusNode, selection.focusOffset);
  if (!anchor || !focus) return update();
  const range = selection.getRangeAt(0);
  const backward = selection.anchorNode !== range.startContainer || selection.anchorOffset !== range.startOffset;
  try {
    return update();
  } finally {
    const after = indexText(root);
    const start = anchor.outside ? (backward ? before.text.length : 0) : anchor.absolute;
    const end = focus.outside ? (backward ? 0 : before.text.length) : focus.absolute;
    const selectedRange = { start: Math.min(start, end), end: Math.max(start, end) };
    const retainedPoints = [
      ...retainedTablePoints(before, after, anchor, focus, selectedRange, context),
      ...retainedExternalBoundaries(anchor, focus, start, end, after.text.length, backward),
    ];
    const mapOffset = createTextOffsetMapper(before.text, after.text, selectedRange, { ...context, before, after,
      retainedPoints,
      decodeEntity: value => {
        // Input is a single alphanumeric/numeric entity token, never markup.
        const element = document.createElement('textarea');
        element.innerHTML = value;
        return element.value;
      },
    });
    let nextAnchor = resolvePoint(root, after, anchor, mapOffset, !backward);
    let nextFocus = resolvePoint(root, after, focus, mapOffset, backward);
    // A collapsed element caret can resolve to opposite sides of a native
    // paragraph break. Keep both endpoints at the same actual DOM point.
    if (anchor.node === focus.node && anchor.offset === focus.offset) nextFocus = nextAnchor;
    const anchorPosition = readPoint(root, after, nextAnchor.node, nextAnchor.offset);
    const focusPosition = readPoint(root, after, nextFocus.node, nextFocus.offset);
    // Distinct DOM block boundaries can have the same text coordinate yet
    // select a native paragraph break (notably in WebKit). Removed content
    // must collapse to one actual point, rather than leave that visual range.
    const collapsedContent = selectedRange.start !== selectedRange.end
      && anchorPosition?.absolute === focusPosition?.absolute;
    if (anchorPosition && focusPosition && !anchorPosition.outside && !focusPosition.outside
      && (collapsedContent || (backward ? anchorPosition.absolute < focusPosition.absolute : anchorPosition.absolute > focusPosition.absolute))) {
      const earlier = anchorPosition.absolute < focusPosition.absolute ? nextAnchor : nextFocus;
      nextAnchor = earlier;
      nextFocus = earlier;
    }
    if (selection.anchorNode !== nextAnchor.node || selection.anchorOffset !== nextAnchor.offset
      || selection.focusNode !== nextFocus.node || selection.focusOffset !== nextFocus.offset) {
      if (typeof selection.setBaseAndExtent === 'function') {
        selection.setBaseAndExtent(nextAnchor.node, nextAnchor.offset, nextFocus.node, nextFocus.offset);
      } else {
        const restored = document.createRange();
        const start = backward ? nextFocus : nextAnchor;
        const end = backward ? nextAnchor : nextFocus;
        restored.setStart(start.node, start.offset);
        restored.setEnd(end.node, end.offset);
        selection.removeAllRanges();
        selection.addRange(restored);
      }
    }
  }
}

/** Reading streamed output suspends scroll-follow without stopping rendering. */
export function hasRichTextSelection(root) {
  const selection = root?.ownerDocument?.getSelection?.();
  return Boolean(selection && !selection.isCollapsed
    && (root.contains(selection.anchorNode) || root.contains(selection.focusNode)));
}
