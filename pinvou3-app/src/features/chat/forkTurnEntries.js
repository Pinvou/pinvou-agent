import { turnHasUserText } from '../conversation/deepseek-conversation.js';

/**
 * 时间线 turn 边界 → 分叉入口（Map<turnId, keepTurns>）。
 * keepTurns = 该 turn 的 1-based 序号（「从此轮分叉」保留到本轮末尾，含本轮
 * 回复；与 rewind 的 `ordinal - 1` 差一位是有意为之：rewind 边界丢弃该轮，
 * fork 边界保留该轮）。序号口径与 rewindEntriesByTurnId 一致，且与 Rust
 * fork_session 的底座 user_turn_prompt_indices（Editable 分类）对齐：
 * - 纯图片 turn 与压缩摘要头 turn（stamped 载体/粘贴摘要头，见
 *   turnHasUserText）不计序号；
 * - 引擎 `<context_update>` 运行时消息（pinned 上下文漂移，投影层不产生
 *   turn）按后端 era 口径计边界：bridge 层在投递/重放两路把每个落下的
 *   context_update 计入待消费计数、随下一个用户气泡下发
 *   （runtimeContextUpdatesBefore），在此并入序号——否则每个漂移点之后
 *   的序号整体少一，分叉把被点击的轮留在源会话。
 * 已知残留（与 Rust 侧 fork.rs 同一口径披露）：post-provenance 历史
 * （会话内压缩过）里粘贴的摘要头 turn，后端 era 列表计入而本表跳过——
 * 此时点击的序号比后端少一，分叉会把被点击的那轮留在源会话（分支少含
 * 一轮）。该场景罕见；根治是把序号由后端随消息下发，前端不再本地计数。
 * 抽成纯函数与 checkpoints.js 的 rewindEntriesByTurnId 同款，可被
 * DOM-free 的逻辑测试直接钉住序号映射。
 */
export function forkEntriesByTurnId(turns) {
  const entries = new Map();
  let ordinal = 0;
  for (const turn of turns || []) {
    if (!turn || turn.id == null || !turnHasUserText(turn)) continue;
    ordinal += 1 + ((turn.userItem && turn.userItem.runtimeContextUpdatesBefore) || 0);
    entries.set(turn.id, ordinal);
  }
  return entries;
}
