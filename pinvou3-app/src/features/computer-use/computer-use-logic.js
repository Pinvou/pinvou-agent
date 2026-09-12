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
  let start = markerIndex;
  while (start > 0 && !LEFT_STOP.test(normalized[start - 1])) start -= 1;
  const head = normalized.slice(start, markerIndex + 1);
  if (!(head.startsWith('/') || /^[A-Za-z]:\//.test(head))) return null;
  const tail = markerIndex + SCREENSHOT_DIR_MARKER.length;
  let end = tail;
  while (end < normalized.length && !RIGHT_STOP.test(normalized[end])) end += 1;
  const basename = normalized.slice(tail, end);
  // Take the LAST ".png" occurrence: trailing prose after the path is allowed
  // (RIGHT_STOP doesn't stop at whitespace, so "shot 2.png done" is normal),
  // but the basename itself ending in ".png.png" must not be truncated at the
  // first occurrence (review fix: indexOf found the first one).
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
      // paths from non-text blocks win the "last match wins" rule
      // (review finding). Empty parts yield '' and the function returns null
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
  // approve button for an already-dead request (review finding).
  const stopped = !!slice.stopped;
  return {
    enabled: true,
    stopped,
    showBanner: !!slice.granted && !stopped,
    grantRequest: stopped ? null : (slice.grantRequest || null),
    confirmRequest: stopped ? null : (slice.confirmRequest || null),
  };
}
