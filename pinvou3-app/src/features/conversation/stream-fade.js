// Streaming fade-in for assistant responses.
//
// Streamed markdown arrives as a sanitized HTML string that is wholesale
// re-injected into the container (dangerouslySetInnerHTML) on every render
// tick, so per-node CSS animations would restart for the whole message each
// tick. Instead, a DOM pass runs after each replacement and wraps ONLY text
// beyond a per-container committed character offset in one-shot fade spans;
// already-seen text is left as plain text nodes and never re-animates.
//
// The first pass on a container is a baseline commit (no animation), so
// hydration paths (opening a session mid-stream, detached windows, lane
// remounts) never replay a fade over existing content.
//
// Wiring contract: applyStreamFade must be called from React effects only,
// never from MutationObserver callbacks — the wrap itself mutates the
// container, so an observer callback would re-trigger itself. The code-copy
// observer may still see the inserted spans; ensureCodeCopyButtons is
// idempotent and ignores them.

// One-shot fade span class; must match src/styles/base.css (.stream-fade-seg).
const SEGMENT_CLASS = 'stream-fade-seg';

// Text inside these element subtrees neither counts toward the committed
// offset nor fades: code blocks must appear instantly (and host the code-copy
// self-heal buttons), script/style text is not rendered.
const SKIP_ELEMENT_TAGS = new Set(['PRE', 'CODE', 'SCRIPT', 'STYLE']);

// TreeWalker/NodeFilter constants, inlined numerically so the module also runs
// in DOM engines that expose no NodeFilter global.
const NODE_ELEMENT = 1;
const NODE_TEXT = 3;
const NODE_FILTER_SHOW_ELEMENT = 0x1;
const NODE_FILTER_SHOW_TEXT = 0x4;
const NODE_FILTER_ACCEPT = 1;
const NODE_FILTER_REJECT = 3;

// Stagger and hard cap: segment N of a pass starts N*12ms late, at most 48ms.
// Delay plus the 150ms CSS animation must stay inside one markdown re-parse
// tick (~180-200ms): spans are wholesale-deleted by the next innerHTML
// replacement, and a span wiped at >=88% opacity never visibly pops. At most
// 40 segments animate per pass and any remaining region text appears
// instantly, bounding DOM growth on fast streams.
const SEGMENT_DELAY_STEP_MS = 12;
const SEGMENT_DELAY_MAX_MS = 48;
const SEGMENT_MAX_PER_PASS = 40;

// Fallback segmentation when Intl.Segmenter is unavailable (Safari 14.0):
// fixed 4-character chunks; the u flag keeps surrogate pairs intact.
const FALLBACK_SEGMENT_RE = /[\s\S]{1,4}/gu;
const WHITESPACE_ONLY_RE = /^\s*$/;
// Han ideographs fade one character at a time: word segmentation splits them
// into dictionary words on full-ICU runtimes (你好) and into single characters
// on sparse-ICU ones, so they are re-split per code point for identical
// granularity everywhere. Kana joins the rule for the same reason; other
// scripts keep their segmenter words.
const HAN_RE = /[\u3400-\u9FFF\uF900-\uFAFF\u{20000}-\u{2FA1F}\u3040-\u30FF]/u;

let preferenceEnabled = true;

// User preference (settings.chat.streaming_animation). Wired from the app
// shell; defaults to enabled until the real settings snapshot arrives.
export function setStreamFadePreference(enabled) {
  preferenceEnabled = enabled !== false;
}

export function isStreamFadePreferenceEnabled() {
  return preferenceEnabled;
}

// Per-container bookkeeping: committed counts visible characters already seen
// by the user; everything before it never animates again.
const fadeStateByContainer = new WeakMap();

// PURE: decide what a fade pass must do from the previous bookkeeping and the
// current visible text length. `from` is the animation region start for
// 'animate' and the resulting committed offset for 'baseline'/'none'. A pass
// on a container without state and a shrink (markdown re-parse rendered
// shorter text) both re-baseline instead of animating.
export function planFadePass({ started, committed, total }) {
  if (!started || total < committed) return { action: 'baseline', from: total };
  if (total === committed) return { action: 'none', from: committed };
  return { action: 'animate', from: committed };
}

// PURE: split region text into fade segments. Prefers an Intl.Segmenter-style
// word segmenter (CJK ideographs are normalized to one segment per character,
// see HAN_RE); passing a falsy segmenter forces the fixed-size fallback, so
// both branches stay deterministic for unit tests. Whitespace-only pieces
// never stand alone: trailing whitespace attaches to the preceding segment,
// leading whitespace to the following one, and an entirely whitespace text
// yields no segments.
export function splitFadeSegments(text, segmenter) {
  const value = text == null ? '' : String(text);
  if (!value) return [];
  const raw = [];
  if (segmenter && typeof segmenter.segment === 'function') {
    for (const piece of segmenter.segment(value)) splitHanPerCodePoint(raw, piece.segment);
  } else {
    for (const piece of value.match(FALLBACK_SEGMENT_RE) || []) splitHanPerCodePoint(raw, piece);
  }
  return mergeWhitespaceSegments(raw);
}

// Append one raw segment, re-splitting it per code point when it contains a
// per-character CJK script (see HAN_RE).
function splitHanPerCodePoint(raw, piece) {
  if (!HAN_RE.test(piece)) {
    raw.push(piece);
    return;
  }
  for (const char of piece) raw.push(char);
}

function mergeWhitespaceSegments(raw) {
  const merged = [];
  let leading = '';
  for (const piece of raw) {
    if (WHITESPACE_ONLY_RE.test(piece)) {
      if (merged.length > 0) merged[merged.length - 1] += piece;
      else leading += piece;
      continue;
    }
    merged.push(leading + piece);
    leading = '';
  }
  return merged;
}

// One shared segmenter per runtime; cached lazily (null = unsupported).
let cachedWordSegmenter;
function getWordSegmenter() {
  if (cachedWordSegmenter === undefined) {
    cachedWordSegmenter = typeof Intl === 'object' && Intl !== null && typeof Intl.Segmenter === 'function'
      ? new Intl.Segmenter(undefined, { granularity: 'word' })
      : null;
  }
  return cachedWordSegmenter;
}

function prefersReducedMotion() {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return false;
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches === true;
}

function acceptFadeNode(node) {
  if (node.nodeType === NODE_ELEMENT && SKIP_ELEMENT_TAGS.has(String(node.tagName).toUpperCase())) {
    // FILTER_REJECT: skip the whole subtree (spec TreeWalker semantics).
    return NODE_FILTER_REJECT;
  }
  return NODE_FILTER_ACCEPT;
}

// Some DOM engines (domino, used by unit tests) do not implement
// FILTER_REJECT subtree skipping, so ancestors are re-checked as well; cheap
// at message DOM sizes and keeps counted offsets identical in every engine.
function isInsideSkippedSubtree(node, root) {
  for (let parent = node.parentNode; parent && parent !== root; parent = parent.parentNode) {
    if (parent.nodeType === NODE_ELEMENT && SKIP_ELEMENT_TAGS.has(String(parent.tagName).toUpperCase())) return true;
  }
  return false;
}

// Visible text nodes of the container in document order (skipped subtrees
// excluded); null when the environment has no DOM walker, in which case the
// caller leaves its bookkeeping untouched.
function collectVisibleTextNodes(container) {
  const doc = container.ownerDocument || (typeof document === 'undefined' ? null : document);
  if (!doc || typeof doc.createTreeWalker !== 'function') return null;
  const walker = doc.createTreeWalker(container, NODE_FILTER_SHOW_ELEMENT | NODE_FILTER_SHOW_TEXT, { acceptNode: acceptFadeNode });
  const nodes = [];
  for (let current = walker.nextNode(); current; current = walker.nextNode()) {
    if (current.nodeType !== NODE_TEXT) continue;
    if (isInsideSkippedSubtree(current, container)) continue;
    nodes.push(current);
  }
  return nodes;
}

// DOM pass: wrap newly streamed text in fade spans. Idempotent and safe to
// call after every render; after a pass the committed offset equals the
// current text length, so a repeated call on the same DOM is a no-op.
export function applyStreamFade(container, { streaming } = {}) {
  if (!container) return;
  // Completed render: if the final html string equals the last throttled
  // string, React skips the dangerouslySetInnerHTML write and leftover fade
  // spans would persist in the completed message. Unwrap them — by the time
  // streaming ends, every span is at least one re-parse tick old (wraps only
  // happen on html-changing renders, >=180ms apart) so its 150ms animation
  // has finished and the unwrap is invisible.
  if (!streaming) {
    unwrapFadeSpans(container);
    fadeStateByContainer.delete(container);
    return;
  }
  // Preference off or reduced motion: forget the bookkeeping, so a mid-stream
  // re-enable re-baselines the container instead of re-fading text the user
  // already watched appear while the animation was off. The CSS reduced-motion
  // override covers the animation side regardless.
  if (!isStreamFadePreferenceEnabled() || prefersReducedMotion()) {
    fadeStateByContainer.delete(container);
    return;
  }
  const textNodes = collectVisibleTextNodes(container);
  if (!textNodes) return;
  let total = 0;
  for (const node of textNodes) total += node.nodeValue.length;
  const state = fadeStateByContainer.get(container) || null;
  const plan = planFadePass({
    started: Boolean(state && state.started),
    committed: state ? state.committed : 0,
    total,
  });
  if (plan.action === 'none') return;
  if (plan.action === 'animate') wrapFadeRegion(textNodes, plan.from, total);
  fadeStateByContainer.set(container, { started: true, committed: total });
}

// Completion hygiene: replace every fade span with its text so the completed
// message DOM is pristine regardless of whether React rewrote the html.
function unwrapFadeSpans(container) {
  if (typeof container.querySelectorAll !== 'function') return;
  const spans = container.querySelectorAll(`.${SEGMENT_CLASS}`);
  for (const span of spans) {
    const parent = span.parentNode;
    if (!parent) continue;
    // Manual unwrap of children: ParentNode#append/replaceWith are missing on
    // old WebKit and on the domino engine the unit tests use (same exemption
    // as code-block-copy.js); ChildNode#remove is universally available.
    while (span.firstChild) parent.insertBefore(span.firstChild, span);
    span.remove();
  }
}

// Wrap the visible-text region [from, total) across the pre-collected nodes:
// split each overlapping node so the region sits in its own text node, then
// wrap that node's slices in staggered fade spans.
function wrapFadeRegion(textNodes, from, total) {
  const doc = textNodes.length > 0 ? textNodes[0].ownerDocument : null;
  if (!doc || typeof doc.createElement !== 'function') return;
  const segmenter = getWordSegmenter();
  let animatedSegments = 0;
  let offset = 0;
  for (const node of textNodes) {
    const value = node.nodeValue || '';
    const nodeStart = offset;
    offset += value.length;
    if (offset <= from) continue; // entirely seen text
    if (nodeStart >= total) break; // beyond the region (defensive)
    animatedSegments += wrapNodeRegion(
      node,
      Math.max(from, nodeStart) - nodeStart,
      Math.min(total, offset) - nodeStart,
      doc,
      segmenter,
      animatedSegments,
    );
  }
}

// Wrap one node's [startInNode, endInNode) slice in fade spans; returns the
// number of spans created so the caller keeps one stagger/cap budget per pass.
function wrapNodeRegion(node, startInNode, endInNode, doc, segmenter, animatedSegments) {
  let region = node;
  if (startInNode > 0) {
    if (typeof region.splitText !== 'function') return 0;
    region = region.splitText(startInNode);
    endInNode -= startInNode;
  }
  if (endInNode < (region.nodeValue || '').length) {
    if (typeof region.splitText !== 'function') return 0;
    region.splitText(endInNode);
  }
  const regionText = region.nodeValue || '';
  // A whitespace-only slice is invisible: leave it unwrapped instead of
  // animating an empty span (same trailing-whitespace rule as Streamdown).
  if (WHITESPACE_ONLY_RE.test(regionText)) return 0;
  if (animatedSegments >= SEGMENT_MAX_PER_PASS) return 0;
  let created = 0;
  let cursor = region;
  for (const segment of splitFadeSegments(regionText, segmenter)) {
    if (animatedSegments + created >= SEGMENT_MAX_PER_PASS) break;
    const text = cursor.nodeValue || '';
    if (text.length < segment.length) break; // segments must tile the region
    const target = cursor;
    if (text.length > segment.length) {
      if (typeof cursor.splitText !== 'function') break;
      // splitText leaves the leading segment.length characters in the
      // original node and returns the remainder, so the head is the piece
      // that gets wrapped and the remainder carries the following segments.
      cursor = cursor.splitText(segment.length);
    } else {
      cursor = null; // region fully consumed by this segment
    }
    if (!target.parentNode) break;
    const span = doc.createElement('span');
    span.className = SEGMENT_CLASS;
    const delay = Math.min((animatedSegments + created) * SEGMENT_DELAY_STEP_MS, SEGMENT_DELAY_MAX_MS);
    if (delay > 0) span.setAttribute('style', `--stream-fade-delay:${delay}ms`);
    target.parentNode.replaceChild(span, target);
    // eslint-disable-next-line unicorn/prefer-dom-node-append -- unit tests build DOM with domino, which lacks ParentNode#append (same exemption as code-block-copy.js)
    span.appendChild(target);
    created += 1;
    if (cursor === null) break;
  }
  return created;
}
