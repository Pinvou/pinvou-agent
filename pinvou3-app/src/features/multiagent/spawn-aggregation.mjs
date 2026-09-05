/**
 * spawn 型 `agent` 工具调用的连续序列聚合（蜂群模式展示层，纯函数无 React）。
 *
 * 蜂群模式（ADR-0006 蜂群改造）之后，消息流里的 spawn 不再逐个渲染行内
 * 专家卡，而是把"同一条 assistant 消息里连续的 spawn 调用（中间没有其他
 * 内容块）"聚合为一条小字计数行：「品悟创建了 x 个智能体」。新 spawn 到达
 * 时只让计数 x 原地递增，不产生新的文本行；spawn 之间夹着任何其他内容块
 * 时自然断组开新行。
 *
 * 标注只做一次：在投影输入（可见 chatItems）上执行
 * `annotateAgentSpawnGroups`。两条车道都经它拿到结果——legacy ChatBubble
 * 车道直接读条目上的 spawnGroup / spawnGroupHidden；统一
 * ConversationTimeline 车道的投影条目把整条 chat item 挂在 `legacyItem`
 * 上（deepseek-conversation 的 projectItem），ToolCard 读
 * `item.legacyItem.spawnGroup`。不要在投影后的 turns 上再补一次标注：
 * turn.presentation 持有的是未标注的原始投影条目，二次标注到不了渲染层。
 *
 * 判定与 conversation 层的 `isExpertDelegationCall` 同源：status/wait/cancel
 * 等协调操作不是 spawn，永远不进计数行。
 */

import { isAgentWaitCall, isExpertDelegationCall } from '../conversation/conversation-model.js';

/** 裸 chat tool 条目是否是 spawn 型 agent 调用。 */
export function isAgentSpawnChatItem(item) {
  if (!item || item.type !== 'tool') return false;
  if (isAgentWaitCall(item.name, item.args)) return false;
  return isExpertDelegationCall(item.name, item.args);
}

function spawnGroupOf(item) {
  return {
    count: 1,
    failed: item.success === false || item.state === 'failed' ? 1 : 0,
  };
}

/**
 * 聚合裸 chatItems 中的连续 spawn 序列。只对进组的条目做浅拷贝并附加
 * `spawnGroup`（序列首条）或 `spawnGroupHidden: true`（其余），其余条目
 * 按引用原样返回，避免整表重渲染。
 *
 * @returns {Array} 新数组；未进组的条目保持原引用。
 */
export function annotateAgentSpawnGroups(items) {
  if (!Array.isArray(items)) return items;
  const result = [];
  let group = null;
  for (const item of items) {
    if (isAgentSpawnChatItem(item)) {
      if (!group) {
        group = spawnGroupOf(item);
        result.push({ ...item, spawnGroup: group });
      } else {
        group.count += 1;
        if (item.success === false || item.state === 'failed') group.failed += 1;
        result.push({ ...item, spawnGroupHidden: true });
      }
      continue;
    }
    group = null;
    result.push(item);
  }
  return result;
}
