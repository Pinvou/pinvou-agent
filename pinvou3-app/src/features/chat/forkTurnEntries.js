import { turnHasUserText } from '../conversation/deepseek-conversation.js';

/**
 * 时间线 turn 边界 → 分叉入口（Map<turnId, keepTurns>）。
 * keepTurns = 该 turn 的 1-based 序号（「从此轮分叉」保留到本轮末尾，含本轮
 * 回复；与 rewind 的 `ordinal - 1` 差一位是有意为之：rewind 边界丢弃该轮，
 * fork 边界保留该轮）。序号口径与 rewindEntriesByTurnId 一致，且与 Rust
 * fork_session 的底座 user_turn_prompt_indices（Editable 分类）对齐：
 * 纯图片 turn 与压缩摘要头 turn（stamped 载体/粘贴摘要头，见
 * turnHasUserText）不计序号。
 * 抽成纯函数与 checkpoints.js 的 rewindEntriesByTurnId 同款，可被
 * DOM-free 的逻辑测试直接钉住序号映射。
 */
export function forkEntriesByTurnId(turns) {
  const entries = new Map();
  let ordinal = 0;
  for (const turn of turns || []) {
    if (!turn || turn.id == null || !turnHasUserText(turn)) continue;
    ordinal += 1;
    entries.set(turn.id, ordinal);
  }
  return entries;
}
