/**
 * Received cross-session messages: the sender header block contract.
 *
 * Contract (paired with src-tauri/src/features/messaging/mod.rs, the
 * delivery side of session-reader's send_message_to_session): a delivered
 * message enters the target session as a user turn whose text starts with
 * a machine-readable sender header — header line, one JSON line
 * ({sessionId, title}, either null when the sender was unattributed), a
 * blank separator, then the body. This module is the single source of the
 * receive-side parsing: UserBubble renders the sender card from it, and the
 * auto-title paths strip it via the window global (classic-script bridges
 * do not import features back — same pattern as session-mention.js).
 *
 * Tolerance mirrors the mention block: only a block at the very start is
 * recognized; a hand-written lookalike (tampered JSON, missing blank line
 * before a body, absurdly long JSON line) passes through unchanged.
 */

/** Header line of the delivered block — mirrored verbatim by features/messaging's MESSAGE_BLOCK_HEADER (Rust) and sessions.rs's SESSION_MESSAGE_BLOCK_HEADER (titler strip). */
export const MESSAGE_BLOCK_HEADER = '## Message from another session';

/** Spoofed-block hardening: a genuine sender line is bounded by one id + one capped title; anything absurdly long is dirty data and skipped before JSON.parse. */
const MAX_BLOCK_JSON_LINE_LENGTH = 64 * 1024;
/** Per-title cap when parsing the sender out of stored messages. */
const MAX_SENDER_TITLE_LENGTH = 200;

/**
 * Strip a sender header block from a user message text.
 * @param {string} text raw user message text
 * @returns {{ sender: {sessionId: string | null, title: string | null}, text: string }}
 *   parsed sender (null fields = unattributed) and the stripped body; an
 *   unparseable lookalike returns sender null and the text unchanged.
 */
export function splitSessionMessageBlock(text) {
  // CRLF-tolerant: history normalized by external tooling may carry \r\n,
  // which would defeat both the startsWith check and the blank-line compare.
  const raw = String(text || '').replace(/\r\n/g, '\n');
  const untouched = { sender: null, text: String(text || '') };
  if (!raw.startsWith(MESSAGE_BLOCK_HEADER + '\n')) return untouched;
  const lines = raw.split('\n');
  // Block layout: header + JSON line + blank line + body; the JSON line may
  // also terminate the string (empty body — tolerated, never produced by the
  // delivery side which validates a non-empty text).
  if (lines.length < 2) return untouched;
  if (lines[1].length > MAX_BLOCK_JSON_LINE_LENGTH) return untouched;
  let parsed;
  try {
    parsed = JSON.parse(lines[1]);
  } catch {
    return untouched;
  }
  if (!Array.isArray(parsed) && parsed !== null && typeof parsed === 'object') {
    const sessionId = typeof parsed.sessionId === 'string' && parsed.sessionId ? parsed.sessionId : null;
    const rawTitle = typeof parsed.title === 'string' ? parsed.title : '';
    const sender = { sessionId, title: rawTitle.slice(0, MAX_SENDER_TITLE_LENGTH) || null };
    if (lines.length === 2) return { sender, text: '' };
    if (lines[2] !== '') return untouched;
    return { sender, text: lines.slice(3).join('\n') };
  }
  return untouched;
}

// Classic-script bridges (platform/{tauri,web}) reuse the same parser for
// auto-titling via the window global — bridges cannot import features back,
// so the global publication keeps a single source of truth for the block
// format (same pattern as session-mention.js).
if (typeof window !== 'undefined') {
  window.__PINVOU_SESSION_MESSAGE__ = { splitSessionMessageBlock };
}
