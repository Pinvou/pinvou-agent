/**
 * 内部运行时消息判定（ESM 共享版）。
 *
 * CodeWhale 把内部运行时信封（子智能体交接 subagent_handoff / 后台 shell 完成
 * shell_completion / 运行时恢复提示 runtime）以 role=user 持久化供父模型上下文
 * 使用，展示层不得渲染为用户气泡。判定规则与
 * platform/{tauri,web}/bridge.js 的 userMessageDisplayText 保持一致：
 * - 完整信封：<codewhale:runtime_event ... visibility="internal">...</codewhale:runtime_event>
 * - 非权威 provenance：turn_meta 块内 "Input provenance: <id>"，白名单
 *   {runtime, subagent_handoff, shell_completion}
 *
 * 本模块供 ESM 车道（CodeX 原生车道 code-native-lane.js、子智能体 transcript
 * 面板 subagent-conversation.mjs）共享；tauri/web bridge 是 IIFE 自包含 bundle
 * 无法 import，各自维护等价实现（见两处 bridge.js）。扩展白名单时须同步三处。
 */

const INTERNAL_PROVENANCE = new Set(['runtime', 'subagent_handoff', 'shell_completion']);

/** 单个文本块是否为完整内部运行时信封。 */
export function isInternalRuntimeEnvelopeText(value) {
  const text = String(value || '').trim();
  return /^<codewhale:runtime_event\b[^>]*\bvisibility=(["'])internal\1[^>]*>/i.test(text)
    && /<\/codewhale:runtime_event>\s*$/i.test(text);
}

/** 从消息 blocks 解析稳定的 provenance 标识符（小写）；无则返回空串。 */
export function userMessageInputProvenance(blocks) {
  for (const block of blocks || []) {
    if (!block || block.type !== 'text') continue;
    const text = String(block.text || '').trim();
    if (text.indexOf('<turn_meta>') !== 0) continue;
    const match = text.match(/(?:^|\n)Input provenance:\s*([a-z0-9_-]+)/i);
    if (match && match[1]) return match[1].toLowerCase();
  }
  return '';
}

/** blocks 是否构成内部运行时消息（完整信封或非权威 provenance）。 */
export function isInternalUserMessage(blocks) {
  const textBlocks = Array.isArray(blocks) ? blocks : [];
  if (textBlocks.some(block => block && block.type === 'text' && isInternalRuntimeEnvelopeText(block.text))) {
    return true;
  }
  return INTERNAL_PROVENANCE.has(userMessageInputProvenance(textBlocks));
}
