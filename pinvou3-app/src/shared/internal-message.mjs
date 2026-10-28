/**
 * Internal runtime message classification (ESM shared version).
 *
 * CodeWhale persists internal runtime envelopes (sub-agent handoff
 * subagent_handoff / background shell completion shell_completion / runtime
 * recovery hint runtime) as role=user messages for the parent model's
 * context; the presentation layer must never render them as user bubbles.
 * This module recognizes three forms:
 * - Full envelope: <codewhale:runtime_event ... visibility="internal">...</codewhale:runtime_event>
 * - Non-authoritative provenance: "Input provenance: <id>" inside a block
 *   starting with (or trailing) <turn_meta>, whitelist
 *   {runtime, subagent_handoff, shell_completion}
 * - Canonical context update: a complete message wrapped in
 *   <context_update> with the fixed runtime preamble
 *
 * This module is shared by the ESM lanes (the CodeX native lane
 * code-native-lane.js and the sub-agent transcript panel
 * subagent-conversation.mjs). The tauri/web bridge equivalent lives in
 * src/shared/bridge-messages.js (classic script, loaded by index.html
 * before both bridges). Both implementations keep the same envelope,
 * provenance, context-update and multi-block semantics; extend the
 * whitelist or the internal message forms in both places.
 */

const INTERNAL_PROVENANCE = new Set(['runtime', 'subagent_handoff', 'shell_completion']);

/** Whether a single text block is a complete internal runtime envelope. */
export function isInternalRuntimeEnvelopeText(value) {
  const text = String(value || '').trim();
  return /^<codewhale:runtime_event\b[^>]*\bvisibility=(["'])internal\1[^>]*>/i.test(text)
    && /<\/codewhale:runtime_event>\s*$/i.test(text);
}

/**
 * CodeWhale's canonical prompt-cache delta is durable model context rather
 * than a user-authored turn. Deliberately require the full wrapper and fixed
 * preamble so ordinary mentions or quoted fragments remain visible.
 */
export function isContextUpdateRuntimeMessage(value) {
  const text = String(value || '').trim().replace(/\r\n/g, '\n');
  const prefix = [
    '<context_update>',
    'Session context changed since it was pinned; the pinned system prompt is unchanged. Delta (+ added, - removed):',
    '',
  ].join('\n');
  return text.startsWith(prefix) && text.endsWith('\n</context_update>');
}

function consumeLeadingInternalRuntimeEnvelope(value) {
  const text = String(value || '').trim();
  const opening = text.match(/^<codewhale:runtime_event\b[^>]*\bvisibility=(["'])internal\1[^>]*>/i);
  if (!opening) return null;
  const remainder = text.slice(opening[0].length);
  const closing = remainder.match(/<\/codewhale:runtime_event\s*>/i);
  if (!closing) return null;
  return remainder.slice(closing.index + closing[0].length).trim();
}

function containsOnlyInternalRuntimeMetadata(value) {
  let remainder = String(value || '').trim();
  let foundEnvelope = false;
  while (remainder) {
    const afterEnvelope = consumeLeadingInternalRuntimeEnvelope(remainder);
    if (afterEnvelope === null) break;
    foundEnvelope = true;
    remainder = afterEnvelope;
  }
  if (!foundEnvelope) return false;
  if (!remainder || /^<turn_meta_unchanged\s*\/>$/i.test(remainder)) return true;
  const lowerRemainder = remainder.toLowerCase();
  return lowerRemainder.startsWith('<turn_meta>') && lowerRemainder.endsWith('</turn_meta>');
}

function inputProvenanceFromText(value) {
  const text = String(value || '').trim();
  const lowerText = text.toLowerCase();
  const openingTag = '<turn_meta>';
  const closingTag = '</turn_meta>';
  const openingIndex = lowerText.lastIndexOf(openingTag);
  const closingIndex = lowerText.indexOf(closingTag, openingIndex + openingTag.length);
  const hasTrailingMetadata = openingIndex > 0 && text[openingIndex - 1] === '\n'
    && closingIndex >= 0 && !text.slice(closingIndex + closingTag.length).trim();
  const metadata = openingIndex === 0
    ? text
    : (hasTrailingMetadata ? text.slice(openingIndex, closingIndex + closingTag.length) : '');
  if (!metadata) return '';
  const match = metadata.match(/(?:^|\n)Input provenance:\s*([a-z0-9_-]+)/i);
  return match && match[1] ? match[1].toLowerCase() : '';
}

/** Whether a single text block consists entirely of known internal runtime content. */
export function isInternalRuntimeUserMessage(value) {
  return isContextUpdateRuntimeMessage(value)
    || containsOnlyInternalRuntimeMetadata(value)
    || INTERNAL_PROVENANCE.has(inputProvenanceFromText(value));
}

export function isKnownInternalMetadataBlock(value) {
  return isInternalRuntimeUserMessage(value);
}

function isTurnMetadata(value) {
  const text = String(value || '').trim().toLowerCase();
  return /^<turn_meta_unchanged\s*\/>$/.test(text)
    || (text.startsWith('<turn_meta>') && text.endsWith('</turn_meta>'));
}

// A standalone metadata-shaped block may be user text. Only use the saved
// message's structural metadata position; preserve non-text blocks for callers.
export function visibleUserMessageBlocks(blocks) {
  const input = Array.isArray(blocks) ? blocks : [];
  const index = userMetadataIndex(input);
  return input.filter((_, position) => position !== index);
}

function userMetadataIndex(blocks) {
  if (blocks.length < 2) return -1;
  const first = blocks[0];
  const last = blocks[blocks.length - 1];
  if (first && first.type === 'text' && isTurnMetadata(first.text)
      && last && last.type === 'text' && isTurnMetadata(last.text)) return -1;
  if (last && last.type === 'text' && isTurnMetadata(last.text)) return blocks.length - 1;
  if (first && first.type === 'text' && isTurnMetadata(first.text)
      && last && last.type === 'text' && !isTurnMetadata(last.text)) return 0;
  return -1;
}

/** Parse the stable provenance id (lowercase) from message blocks; empty string when absent. */
export function userMessageInputProvenance(blocks) {
  const input = Array.isArray(blocks) ? blocks : [];
  const index = userMetadataIndex(input);
  return index < 0 ? '' : inputProvenanceFromText(input[index].text);
}

/**
 * Whether the blocks contain only internal runtime content. Any ordinary
 * text block keeps the message visible; empty text blocks may accompany
 * internal metadata but alone never prove the message is internal.
 */
export function isInternalUserMessage(blocks) {
  if (Array.isArray(blocks) && blocks.some(block => block && block.type === 'tool_result')) return false;
  return INTERNAL_PROVENANCE.has(userMessageInputProvenance(blocks));
}
