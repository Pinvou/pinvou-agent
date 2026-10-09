import { requireCondition } from '../errors.mjs';
import { nextRecord } from './atomic.mjs';
import { commandPlan } from './operation.mjs';
import { readRecord, assertInteger } from './inputs.mjs';
import { SESSION_TERMINALS, TRANSACTION_TERMINALS } from './state-graphs.mjs';
import { assertScopeIndex } from './scope-index.mjs';
import { authorizationEndWrites } from './lifecycle.mjs';

/** The scope index is the T44-owned complete record set under serialization.
 * Updating it is mandatory whenever a member is created, so omitted records
 * cannot become a second, independently writable lifecycle.
 */
export function planChannelChange(records, command, now) {
  const channel = readRecord(records, command.channelKey);
  const index = assertScopeIndex(records, command.scopeIndexKey, channel.installationScopeId);
  assertInteger(channel.channelRevision, 1);
  requireCondition(channel.channelRevision === command.expectedChannelRevision
    && ['stable', 'beta', 'internal'].includes(command.newChannel) && channel.channel !== command.newChannel
    && index.installationScopeId === channel.installationScopeId, 'MODEL_CHANNEL_CONFLICT');
  const reads = [command.channelKey, command.scopeIndexKey]; const writes = [];
  for (const key of index.memberKeys) {
    const record = readRecord(records, key); reads.push(key);
    requireCondition(record.installationScopeId === channel.installationScopeId, 'MODEL_SCOPE_INVALID');
    if (record.recordKind === 'session' && !SESSION_TERMINALS.includes(record.state)) writes.push({ key,
      value: nextRecord(record, { state: 'channel_changed', lastStateBeforeTerminal: record.state, futureActions: 'fenced', fencedAt: now }) });
    if (record.recordKind === 'activeValidate' && record.state === 'processing') writes.push({ key,
      value: nextRecord(record, { state: 'coordination_aborted', ownerEpoch: record.ownerEpoch + 1 }) });
    if (record.recordKind === 'authorization' && record.state === 'available') {
      const ended = authorizationEndWrites(records, key, record, now >= record.expiresAt ? 'expired' : 'cancelled', now);
      reads.push(...ended.reads); writes.push(...ended.writes);
    }
    if (record.recordKind === 'staged' && record.state === 'waiting') writes.push({ key,
      value: nextRecord(record, { state: 'invalidated' }) });
    if (record.recordKind === 'transaction' && record.executionPurpose === 'preinstall'
      && !TRANSACTION_TERMINALS.includes(record.state)) writes.push({ key,
      value: nextRecord(record, { safeCancellationRequested: true }) });
  }
  writes.push({ key: command.channelKey, value: nextRecord(channel,
    { channel: command.newChannel, channelRevision: channel.channelRevision + 1 }) });
  return commandPlan(records, command, now, [...new Set(reads)], writes,
    [{ kind: 'channel-changed', installationScopeId: channel.installationScopeId, committedAt: now }],
    { channel: command.newChannel, channelRevision: channel.channelRevision + 1 });
}
