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
 * - Compaction checkpoint: the summary header in the first text block
 *   followed by the engine-written provenance marker (see the checkpoint
 *   family below)
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

// ── CodeWhale compaction checkpoint ────────────────────────────────
// Compaction commits its summary into the saved history as a role="user"
// message (the model reads it as a user-history item, Codex parity), so the
// engine's context survives a reload. It is engine-written runtime state,
// not a user turn: a projection that renders it as typed input shows the
// model-facing summary under the user's name. Recognition mirrors the
// foundation's structural predicates (CodeWhale
// crates/tui/src/history_recognition.rs) rather than a substring scan,
// because a user may legitimately paste or quote the summary header.

export const COMPACTION_SUMMARY_MARKERS = [
  'Another language model started to solve this problem',
  'Conversation Summary (Auto-Generated)',
];

export const COMPACTION_CHECKPOINT_PROVENANCE = '<!-- codewhale.compaction-checkpoint.v1 -->';

function isCheckpointTextBox(block) {
  return !!block && block.type === 'text' && block.cache_control == null;
}

/**
 * Loose form (`is_compaction_checkpoint_message`): the first text block
 * begins with the summary header (current or pre-v0.9.6 legacy), and the
 * only other block, if any, is the provenance marker.
 */
export function isCompactionCheckpointBlocks(blocks) {
  if (!Array.isArray(blocks) || blocks.length === 0 || blocks.length > 2) return false;
  const first = blocks[0];
  if (!isCheckpointTextBox(first) || typeof first.text !== 'string') return false;
  if (COMPACTION_SUMMARY_MARKERS.every(marker => !first.text.startsWith(marker))) return false;
  return blocks.length === 1
    || (isCheckpointTextBox(blocks[1]) && blocks[1].text === COMPACTION_CHECKPOINT_PROVENANCE);
}

/**
 * Stamped form (`is_generated_compaction_checkpoint`): summary block plus
 * the engine-written provenance block. Typed input cannot produce this
 * shape — the marker is its own content block — so it is an unambiguous
 * checkpoint.
 *
 * Display reads ONLY this form as a checkpoint. The bare form matches a
 * user turn that merely pastes the summary header; the foundation still
 * reads it loosely in a history with no stamped carrier because a missed
 * carrier would stack a second summary, but hiding a user's own message
 * has no such correctness need behind it. A pre-stamp bare carrier turns
 * stamped on the engine's next restore and disappears from the bubbles
 * then.
 */
export function isGeneratedCompactionCheckpointBlocks(blocks) {
  return Array.isArray(blocks) && blocks.length === 2 && isCompactionCheckpointBlocks(blocks);
}

/** Whether a single text block consists entirely of known internal runtime content. */
export function isInternalRuntimeUserMessage(value) {
  return isContextUpdateRuntimeMessage(value)
    || containsOnlyInternalRuntimeMetadata(value)
    || INTERNAL_PROVENANCE.has(inputProvenanceFromText(value));
}

export function isKnownInternalMetadataBlock(value) {
  if (isInternalRuntimeUserMessage(value)) return true;
  const text = String(value || '').trim().toLowerCase();
  return /^<turn_meta_unchanged\s*\/>$/.test(text)
    || (text.startsWith('<turn_meta>') && text.endsWith('</turn_meta>'));
}

/** Parse the stable provenance id (lowercase) from message blocks; empty string when absent. */
export function userMessageInputProvenance(blocks) {
  for (const block of blocks || []) {
    if (!block || block.type !== 'text') continue;
    const provenance = inputProvenanceFromText(block.text);
    if (provenance) return provenance;
  }
  return '';
}

/**
 * Whether the blocks contain only internal runtime content. Any ordinary
 * text block keeps the message visible; empty text blocks may accompany
 * internal metadata but alone never prove the message is internal.
 */
export function isInternalUserMessage(blocks) {
  const textBlocks = (Array.isArray(blocks) ? blocks : [])
    .filter(block => block && block.type === 'text');
  if (INTERNAL_PROVENANCE.has(userMessageInputProvenance(textBlocks))) return true;
  // The stamped checkpoint is engine-written by construction, so every
  // consumer reads it as runtime metadata. The bare form matches pasted
  // headers and stays displayable. Structural, like the foundation: any
  // extra (non-text) block disqualifies, so the raw blocks are checked.
  if (isGeneratedCompactionCheckpointBlocks(blocks)) return true;
  let foundInternal = false;
  for (const block of textBlocks) {
    if (!String(block.text || '').trim()) continue;
    if (!isKnownInternalMetadataBlock(block.text)) return false;
    foundInternal = true;
  }
  return foundInternal;
}
