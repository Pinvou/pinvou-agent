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
