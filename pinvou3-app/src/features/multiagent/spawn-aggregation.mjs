/**
 * spawn 型 `agent` 工具调用的连续序列聚合（蜂群模式展示层，纯函数无 React）。
 *
 * 蜂群模式（ADR-0006 蜂群改造）之后，消息流里的 spawn 不再逐个渲染行内
 * 专家卡，而是把"同一条 assistant 消息里连续的 spawn 调用（中间没有其他
 * 内容块）"聚合为一条小字计数行：「品悟创建了 x 个智能体」。新 spawn 到达
 * 时只让计数 x 原地递增，不产生新的文本行；spawn 之间夹着任何其他内容块
 * 时自然断组开新行。
 *
 * 两条车道共用：
 * - legacy ChatBubble 车道：对可见 chatItems（{type:'tool', name, args}）做
 *   `annotateAgentSpawnGroups`，ToolCard 直接读 item.spawnGroup /
 *   item.spawnGroupHidden；
 * - 统一 ConversationTimeline 车道：投影后的 turn items（{tool, legacyItem}）
 *   做 `annotateTurnSpawnGroups`，ChatView 的 renderToolItem 把组标记透传给
 *   ToolCard。
 *
 * 判定与 conversation 层的 `isExpertDelegationCall` 同源：status/wait/cancel
 * 等协调操作不是 spawn，永远不进计数行。
 */

import { isAgentWaitCall, isExpertDelegationCall } from '../conversation/conversation-model.js';
import { extractSubagentId } from './subagent-conversation.mjs';

/** legacy 车道的裸 chat tool 条目是否是 spawn 型 agent 调用。 */
export function isAgentSpawnChatItem(item) {
  if (!item || item.type !== 'tool') return false;
  if (isAgentWaitCall(item.name, item.args)) return false;
  return isExpertDelegationCall(item.name, item.args);
}

/** 统一时间线车道的投影 tool 条目是否是 spawn 型 agent 调用。 */
export function isAgentSpawnProjectedItem(item) {
  if (!item || item.type !== 'tool') return false;
  const legacy = item.legacyItem;
  if (legacy) return isAgentSpawnChatItem(legacy);
  const tool = item.tool || {};
  if (isAgentWaitCall(tool.name, tool.rawInput)) return false;
  return isExpertDelegationCall(tool.name, tool.rawInput);
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

/**
 * 聚合投影 turns 中的连续 spawn 序列（turn 内相邻的投影 tool 条目）。
 * 标记落在投影条目本身（`spawnGroup` / `spawnGroupHidden`），由 ChatView 的
 * renderToolItem 透传给 ToolCard；不改写 legacyItem（它可能被其他渲染路径
 * 共享）。
 *
 * @returns {Array} 新 turns 数组；turn 浅拷贝，items 数组重建。
 */
export function annotateTurnSpawnGroups(turns) {
  if (!Array.isArray(turns)) return turns;
  let changed = false;
  const nextTurns = turns.map(turn => {
    const items = Array.isArray(turn && turn.items) ? turn.items : [];
    const nextItems = [];
    let group = null;
    let turnChanged = false;
    for (const item of items) {
      if (isAgentSpawnProjectedItem(item)) {
        turnChanged = true;
        if (!group) {
          group = spawnGroupOf(item);
          nextItems.push({ ...item, spawnGroup: group });
        } else {
          group.count += 1;
          if (item.legacyItem
            ? (item.legacyItem.success === false || item.legacyItem.state === 'failed')
            : false) group.failed += 1;
          nextItems.push({ ...item, spawnGroupHidden: true });
        }
        continue;
      }
      group = null;
      nextItems.push(item);
    }
    if (!turnChanged) return turn;
    changed = true;
    return { ...turn, items: nextItems };
  });
  return changed ? nextTurns : turns;
}

/** 计数行展示用：组内已成功 spawn 的实例 id（无 id 的运行中调用不计入）。 */
export function spawnGroupAgentIds(items) {
  if (!Array.isArray(items)) return [];
  const ids = [];
  for (const item of items) {
    if (!isAgentSpawnChatItem(item)) continue;
    const agentId = extractSubagentId(item && item.output);
    if (agentId) ids.push(agentId);
  }
  return ids;
}
