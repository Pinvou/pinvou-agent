import { requireCondition } from '../errors.mjs';
import { assertInteger, assertFields, readRecord } from './inputs.mjs';
import { assertWorkerLease, atomicPlan, nextRecord } from './atomic.mjs';

// Only T06's executable contract, not persistence/encryption or a scheduler.
export function planAcquireOwner(records, { operationKey, expectedRevision }, now) {
  const operation = readRecord(records, operationKey); assertInteger(now);
  assertFields(operation, { revision: 'integer', ownerEpoch: 'integer', ownerStartedAt: 'integer',
    leaseExpiresAt: 'integer', state: ['processing'] });
  requireCondition(operation.revision === expectedRevision && now >= operation.leaseExpiresAt, 'MODEL_OWNER_FENCED');
  return atomicPlan(records, [operationKey], [{ key: operationKey, value: nextRecord(operation,
    { ownerEpoch: operation.ownerEpoch + 1, ownerStartedAt: now, lastRenewedAt: now, leaseExpiresAt: now + 30_000 }) }], [], now);
}
export function planRenewOwner(records, { operationKey, ownerEpoch }, now) {
  const operation = readRecord(records, operationKey);
  assertWorkerLease(operation, ownerEpoch, now);
  assertInteger(operation.lastRenewedAt);
  requireCondition(now >= operation.lastRenewedAt + 10_000, 'MODEL_RENEWAL_EARLY');
  return atomicPlan(records, [operationKey], [{ key: operationKey, value: nextRecord(operation,
    { lastRenewedAt: now, leaseExpiresAt: Math.min(now + 30_000, operation.ownerStartedAt + 120_000) }) }], [], now);
}
export function operationWrite(records, command, now, result) {
  const { operationKey, ownerEpoch, requestDigest, resultId } = command;
  const operation = readRecord(records, operationKey);
  assertFields(operation, { requestDigest: 'hash', ownerEpoch: 'positive', ownerStartedAt: 'integer',
    leaseExpiresAt: 'integer', state: ['processing'] });
  assertWorkerLease(operation, ownerEpoch, now);
  requireCondition(operation.requestDigest === requestDigest, 'IDEMPOTENCY_CONFLICT');
  assertFields({ resultId }, { resultId: 'id' });
  return { key: operationKey, value: nextRecord(operation, { state: 'committed', committedAt: now,
    resultId, result: structuredClone(result) }) };
}
export function commandPlan(records, command, now, reads, writes, facts, result) {
  const operation = operationWrite(records, command, now, result);
  return atomicPlan(records, [...new Set([...reads, command.operationKey])], [...writes, operation], facts, now);
}
