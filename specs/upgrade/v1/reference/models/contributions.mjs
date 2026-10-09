import { readRecord, assertInteger, assertHash, assertIdentity } from './inputs.mjs';
import { nextRecord } from './atomic.mjs';
import { requireCondition } from '../errors.mjs';

/** A registered execution/event blocks stale quality even before projection.
 * T20 owns this head and durable outbox; T47 only reads it. Registration shares
 * the originating domain/event commit. It never fabricates success or failure.
 */
export function contributionWrite(records, headKey, { windowId, groupIdentity }) {
  const head = readRecord(records, headKey); assertInteger(head.watermark);
  assertInteger(head.watermark + 1);
  assertIdentity(windowId); assertHash(groupIdentity);
  requireCondition(head.windowGroups !== null && typeof head.windowGroups === 'object', 'MODEL_CONTRIBUTION_INVALID');
  const groups = head.windowGroups[windowId] ?? [];
  return { key: headKey, value: nextRecord(head, { watermark: head.watermark + 1,
    windowGroups: { ...head.windowGroups, [windowId]: [...new Set([...groups, groupIdentity])] } }) };
}
