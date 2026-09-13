/**
 * Aggregation of consecutive spawn-type `agent` tool calls (swarm-mode display
 * layer; pure functions, no React).
 *
 * After the swarm mode (ADR-0006 swarm rework), spawns in the message stream
 * no longer render one inline expert card each: consecutive spawn calls within
 * the same assistant message (with no other content block in between) are
 * aggregated into one small count row ("Pinvou created x agents"). A newly
 * arriving spawn only increments x in place instead of adding a text row; any
 * other content block between spawns naturally breaks the group and starts a
 * new row.
 *
 * Annotation happens exactly once per lane, on the projection input:
 * ChatView annotates the main chat's visible chatItems, and the codex native
 * lane annotates its lane.items inside projectNativeLane. The projected
 * timeline items carry the whole chat item on `legacyItem` (deepseek-
 * conversation's projectItem), so ToolCard reads the annotation straight off
 * the item (`item.spawnGroup` / `item.spawnGroupHidden`). Do not annotate
 * again on the projected turns: turn.presentation holds the unannotated
 * original projected items, so a second pass never reaches the render layer.
 *
 * The predicate mirrors the conversation layer's `isExpertDelegationCall`:
 * coordination operations such as status/wait/cancel are not spawns and never
 * enter the count row.
 */

import { isAgentWaitCall, isExpertDelegationCall } from '../conversation/conversation-model.js';

/** Whether a bare chat tool item is a spawn-type agent call. */
export function isAgentSpawnChatItem(item) {
  if (!item || item.type !== 'tool') return false;
  if (isAgentWaitCall(item.name, item.args)) return false;
  return isExpertDelegationCall(item.name, item.args);
}

/**
 * Degenerate group shape for a single unannotated spawn item: the render
 * layer's fallback when an item somehow bypasses annotation (kept here so
 * the failure predicate stays in one place).
 * @returns {{count: number, failed: number}}
 */
export function spawnGroupOf(item) {
  return {
    count: 1,
    failed: item.success === false || item.state === 'failed' ? 1 : 0,
  };
}

/**
 * Aggregate consecutive spawn sequences in the bare chatItems. Group members
 * are shallow-copied and get `spawnGroup` (first of the sequence) or
 * `spawnGroupHidden: true` (the rest); all other items are returned by
 * reference to avoid re-rendering the whole list.
 *
 * @returns {Array} a new array; non-grouped items keep their references.
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
