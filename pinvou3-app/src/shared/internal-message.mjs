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
  let foundInternal = false;
  for (const block of textBlocks) {
    if (!String(block.text || '').trim()) continue;
    if (!isKnownInternalMetadataBlock(block.text)) return false;
    foundInternal = true;
  }
  return foundInternal;
}

/**
 * 压缩载体消息判定（评审 r5 M2 / r10 M2；镜像 CodeWhale history_recognition：
 * 摘要头前缀（当前/历史）+ 其余为空或仅 provenance 注释）。载体是运行时内部
 * 消息、is_user_turn_prompt 判 NotPrompt 不计 turn——展示层渲染它会造成幻影
 * 「用户轮」并把回退序号顶偏（调和门随之隐藏全部入口）。
 */
export function isCompactionCarrierBlocks(blocks) {
  if (!Array.isArray(blocks) || !blocks.length || !(blocks[0] && blocks[0].type === 'text')) return false;
  const text = String(blocks[0].text || '');
  if (!(text.startsWith('Another language model started to solve this problem')
    || text.startsWith('Conversation Summary (Auto-Generated)'))) return false;
  const rest = blocks.slice(1);
  return rest.length === 0 || (rest.length === 1 && rest[0].type === 'text'
    && String(rest[0].text || '').trim() === '<!-- codewhale.compaction-checkpoint.v1 -->');
}
/**
 * 步数预算通知判定（评审 r10 M3；与 Rust 侧 turns::is_step_budget_notification
 * 前缀镜像）：底座以 ExternalUser provenance 落盘这两条引擎通知，展示层过滤
 * 气泡、计数侧（count_user_turns 家族）同步剔除——否则活会话调和门隐藏全部
 * 回退入口、重载后渲染成用户亲发的假气泡。
 */
// NotPrompt 同族 tool-result 块类型全集（CodeWhale request.rs ContentBlock
// 变体；r20 minor 4 / r25 MAJOR-2 与后端 classify 及 bridge-shared-helpers 的
// classic 副本保持同集——classic script 无法 import ESM，副本各自成文）。
export const TOOL_RESULT_BLOCK_TYPES = [
  'tool_result',
  'tool_search_tool_result',
  'code_execution_tool_result',
];

// 含 tool-result 族块的用户消息不是用户轮（classify 按 ToolResult-first 判
// NotPrompt）——重载投影只跳过其用户气泡，工具卡回填照跑（r25 MAJOR-2）。
export function isToolResultCarrierBlocks(blocks) {
  return (Array.isArray(blocks) ? blocks : []).some(
    block => block && TOOL_RESULT_BLOCK_TYPES.includes(block.type),
  );
}

export function isStepBudgetNoticeText(text) {
  const value = String(text || '');
  return value.startsWith('Step budget soft landing:')
    || value.startsWith('Your model-step budget was exhausted');
}
