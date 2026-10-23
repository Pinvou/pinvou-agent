/**
 * 内部运行时消息判定（ESM 共享版）。
 *
 * CodeWhale 把内部运行时信封（子智能体交接 subagent_handoff / 后台 shell 完成
 * shell_completion / 运行时恢复提示 runtime）以 role=user 持久化供父模型上下文
 * 使用，展示层不得渲染为用户气泡。本模块识别两种形态：
 * - 完整信封：<codewhale:runtime_event ... visibility="internal">...</codewhale:runtime_event>
 * - 非权威 provenance：以前导 <turn_meta> 开头的块内 "Input provenance: <id>"，
 *   白名单 {runtime, subagent_handoff, shell_completion}
 *
 * 本模块供 ESM 车道（CodeX 原生车道 code-native-lane.js、子智能体 transcript
 * 面板 subagent-conversation.mjs）共享。tauri/web bridge 的同源判定已收拢到
 * src/shared/bridge-messages.js（经典脚本，index.html 先于两个 bridge 加载），
 * 其识别是本模块的超集——额外识别 envelope/正文与尾随 turn_meta 摊平进同一
 * 文本块的形态。本模块仍保持保守的前导形态：当前 ESM 车道的消息源不产生摊平
 * 形态，若引擎侧出现须先在此对齐。扩展白名单或信封形态时须同步两处。
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
