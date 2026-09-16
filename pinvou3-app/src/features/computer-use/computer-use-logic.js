/**
 * Computer-use consent/screenshot pure logic — no bridge or DOM access, so the
 * node --test suite can exercise it directly (tests/computer_use_logic.test.mjs).
 */

// The computer_use tool saves screenshots under the session workspace at
// attachments/computer_use/*.png and mentions the absolute path in its text
// output. Backslashes are normalized to '/' for matching only (indices are
// preserved, the returned path keeps the original separators), and the match
// must reach back to an absolute anchor ('/' or '<drive>:/') so a relative
// mention cannot trick the renderer into loading an arbitrary file.
const SCREENSHOT_DIR_MARKER = '/attachments/computer_use/';
const LEFT_STOP = /[\s"'`<>|]/;
const RIGHT_STOP = /["'`<>|\n\r]/;

function screenshotSpanAtMarker(normalized, markerIndex) {
  // Walk left to the first stop boundary; if the head does not reach an
  // absolute anchor, keep extending past earlier boundaries (bounded) and
  // retry the anchor test. A space INSIDE the path (e.g.
  // `C:/Users/John Smith/.pinvou3/...` — Windows account names with spaces
  // are common) is otherwise indistinguishable from prose before the path,
  // and the extraction silently lost the screenshot card for that whole
  // class of users. The anchor requirement itself is what
  // blocks relative-path trickery: a head only wins when it genuinely
  // reaches '/' or '<drive>:/', so prose + a relative mention still fails
  // no matter how far the walk extends.
  let start = markerIndex;
  let head = normalized.slice(start, markerIndex + 1);
  for (let attempts = 0; attempts < 4; attempts += 1) {
    while (start > 0 && !LEFT_STOP.test(normalized[start - 1])) start -= 1;
    head = normalized.slice(start, markerIndex + 1);
    if (head.startsWith('/') || /^[A-Za-z]:\//.test(head)) break;
    if (start === 0) break;
    start -= 1;
  }
  if (!(head.startsWith('/') || /^[A-Za-z]:\//.test(head))) return null;
  const tail = markerIndex + SCREENSHOT_DIR_MARKER.length;
  let end = tail;
  while (end < normalized.length && !RIGHT_STOP.test(normalized[end])) end += 1;
  const basename = normalized.slice(tail, end);
  // Take the LAST ".png" occurrence: trailing prose after the path is allowed
  // (RIGHT_STOP doesn't stop at whitespace, so "shot 2.png done" is normal),
  // but the basename itself ending in ".png.png" must not be truncated at the
  // first occurrence.
  const pngIndex = basename.toLowerCase().lastIndexOf('.png');
  if (pngIndex < 0) return null;
  return { start, end: tail + pngIndex + 4 };
}

function toolOutputText(output) {
  if (output == null) return '';
  if (typeof output === 'string') return output;
  try {
    return JSON.stringify(output);
  } catch {
    return '';
  }
}

/**
 * Extract the screenshot PNG path from a computer_use tool output.
 * The output may be plain text or an MCP-style JSON envelope
 * ({ content: [{ type: 'text', text }] }); both spellings are searched.
 * Returns the LAST match (the newest screenshot in a multi-step result) or
 * null when no screenshot is referenced — callers fall back to the default
 * tool card in that case.
 */
export function extractComputerUseScreenshotPath(output) {
  let text = toolOutputText(output);
  const trimmed = text.trim();
  if (trimmed.startsWith('{') || trimmed.startsWith('[')) {
    try {
      const envelope = JSON.parse(trimmed);
      const blocks = envelope && Array.isArray(envelope.content) ? envelope.content : [];
      const textParts = blocks
        .filter(block => block && block.type === 'text' && typeof block.text === 'string')
        .map(block => block.text);
      // When an envelope parses, search ONLY its text blocks — even when
      // there are none: falling back to the raw JSON surfaced Windows paths
      // with their escaped backslashes (\\) as doubled separators, and let
      // paths from non-text blocks win the "last match wins" rule.
      // Empty parts yield '' and the function returns null
      // via the empty-text path below.
      text = textParts.join('\n');
    } catch {
      // Not JSON — search the raw text as-is.
    }
  }
  if (!text) return null;
  const normalized = text.replaceAll('\\', '/'); // safari14-ok: replaceAll ships since Safari 13.1
  const searchable = normalized.toLowerCase();
  let best = null;
  let searchFrom = 0;
  for (;;) {
    const markerIndex = searchable.indexOf(SCREENSHOT_DIR_MARKER, searchFrom);
    if (markerIndex === -1) break;
    const span = screenshotSpanAtMarker(normalized, markerIndex);
    if (span) best = span;
    searchFrom = markerIndex + SCREENSHOT_DIR_MARKER.length;
  }
  return best ? text.slice(best.start, best.end) : null;
}

/**
 * Consent surface visibility for the current computer-use bridge slice.
 * Everything collapses to hidden while the feature toggle is off: no banner,
 * no dialogs, and the tool card stays the default rendering. The banner is
 * independent of the dialogs: a per-action confirmation can sit on top of an
 * already-granted session, and the banner must remain visible underneath.
 */
export function computerUseConsentView(slice) {
  const enabled = !!(slice && slice.enabled);
  if (!enabled) {
    return { enabled: false, stopped: false, showBanner: false, grantRequest: null, confirmRequest: null };
  }
  // Stop collapses the dialogs too: the backend's stop_all clears every
  // pending confirmation and grant, so a dialog that stayed up would offer an
  // approve button for an already-dead request.
  const stopped = !!slice.stopped;
  return {
    enabled: true,
    stopped,
    showBanner: !!slice.granted && !stopped,
    grantRequest: stopped ? null : (slice.grantRequest || null),
    confirmRequest: stopped ? null : (slice.confirmRequest || null),
  };
}

/**
 * Localized rendering of a per-action confirm request. Newer backends send a
 * structured payload (action name + optional button/click_count/point/
 * text_length/text_preview/text_preview_truncated, with the original English
 * summary kept in `summary` as fallback); legacy payloads carried only the
 * English summary string. When the structured fields are missing or
 * incomplete, the English summary is returned verbatim so old backends keep
 * working.
 *
 * Returns { description, preview, previewTooLong }:
 * - description: the action line for the dialog (never null; falls back to
 *   the English summary, then '').
 * - preview: the full typed-text preview for the inline block, or null.
 * - previewTooLong: true when the backend flagged the text as too long to
 *   preview and shipped no preview — the dialog shows the hint bar then.
 */
export function formatComputerUseConfirmAction(copy, request) {
  const fallback = (request && request.summary) || '';
  const preview = request && request.typePreviewFull != null
    ? request.typePreviewFull
    : ((request && request.textPreview) || null);
  const previewTooLong = !!(request && request.textPreviewTruncated && preview == null);
  const description = describeConfirmAction(copy, request) || fallback;
  return { description, preview, previewTooLong };
}

// Legacy action-name spellings (pre-structured payload): map them onto the
// structured field set so both payload generations render localized.
const LEGACY_ACTION_FIELDS = {
  left_click: { kind: 'click', button: 'left', clickCount: 1 },
  right_click: { kind: 'click', button: 'right', clickCount: 1 },
  middle_click: { kind: 'click', button: 'middle', clickCount: 1 },
  double_click: { kind: 'click', button: 'left', clickCount: 2 },
  triple_click: { kind: 'click', button: 'left', clickCount: 3 },
  left_mouse_down: { kind: 'mouse_down', button: 'left' },
  left_mouse_up: { kind: 'mouse_up', button: 'left' },
  mouse_down: { kind: 'mouse_down', button: 'left' },
  mouse_up: { kind: 'mouse_up', button: 'left' },
  left_click_drag: { kind: 'drag' },
};

function formatConfirmPoint(point) {
  if (!point || typeof point.x !== 'number' || typeof point.y !== 'number') return null;
  return `(${point.x}, ${point.y})`;
}

function describeConfirmPointAction(template, point) {
  if (typeof template !== 'function') return null;
  const formatted = formatConfirmPoint(point);
  if (!formatted) return null;
  return template(formatted);
}

function describeClickAction(copy, request, fields) {
  const button = request.button || (fields && fields.button) || 'left';
  const count = request.clickCount || (fields && fields.clickCount) || 1;
  const buttonName = copy.buttonName && copy.buttonName[button];
  const verbs = [copy.confirmClick1, copy.confirmClick2, copy.confirmClick3];
  const verb = verbs[Math.min(Math.max(count, 1), 3) - 1];
  if (!buttonName || typeof verb !== 'string' || !verb) return null;
  const point = formatConfirmPoint(request.point);
  const template = point ? copy.confirmClickAt : copy.confirmClick;
  if (typeof template !== 'function') return null;
  return template(buttonName, verb, point);
}

function describeConfirmAction(copy, request) {
  if (!request || typeof copy !== 'object' || copy == null) return null;
  const fn = (key) => (typeof copy[key] === 'function' ? copy[key] : null);
  let name = typeof request.actionName === 'string' ? request.actionName : '';
  let fields = null;
  if (LEGACY_ACTION_FIELDS[name]) {
    fields = LEGACY_ACTION_FIELDS[name];
    name = fields.kind;
  }
  switch (name) {
    case 'click':
      return describeClickAction(copy, request, fields);
    case 'type':
      return typeof request.textLength === 'number' ? applyTemplate(fn('confirmTypeCount'), request.textLength) : null;
    case 'key':
      return typeof request.chord === 'string' && request.chord ? applyTemplate(fn('confirmKeyChord'), request.chord) : null;
    case 'hold_key':
      return typeof request.chord === 'string' && request.chord && typeof request.holdMs === 'number'
        ? applyTemplate(fn('confirmHoldKey'), request.chord, request.holdMs)
        : null;
    case 'drag': {
      const from = formatConfirmPoint(request.point);
      const to = formatConfirmPoint(request.endPoint);
      return from && to ? applyTemplate(fn('confirmDrag'), from, to) : null;
    }
    case 'scroll': {
      const direction = copy.scrollDirection && copy.scrollDirection[request.scrollDirection || ''];
      if (!direction || typeof request.scrollAmount !== 'number') return null;
      return applyTemplate(fn('confirmScroll'), direction, request.scrollAmount, formatConfirmPoint(request.point));
    }
    case 'mouse_move':
      return describeConfirmPointAction(fn('confirmMouseMove'), request.point);
    case 'mouse_down':
    case 'mouse_up': {
      const button = request.button || (fields && fields.button) || 'left';
      const buttonName = copy.buttonName && copy.buttonName[button];
      if (!buttonName) return null;
      return applyTemplate(fn(name === 'mouse_down' ? 'confirmMouseDown' : 'confirmMouseUp'), buttonName);
    }
    default:
      return null;
  }
}

// Template call guarded so a copy missing the key falls back to the summary
// instead of throwing.
function applyTemplate(template, ...args) {
  return typeof template === 'function' ? template(...args) : null;
}
