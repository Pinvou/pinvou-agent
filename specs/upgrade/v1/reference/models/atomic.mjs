import { canonicalize } from '../canonical-json.mjs';
import { sha256 } from '../digests.mjs';
import { requireCondition } from '../errors.mjs';
import { sameJson } from '../semantics.mjs';

const planned = new WeakMap();

/** Executable specification of an atomic compare-and-write, not a database.
 * Evidence is immutable server/protected-platform input, never client flags.
 * Production commands must obtain the same read set under their DB transaction.
 */
export function captureRecord(records, key) {
  requireCondition(Object.hasOwn(records, key), 'MODEL_RECORD_MISSING');
  const value = records[key];
  requireCondition(Number.isSafeInteger(value.revision) && value.revision >= 0, 'MODEL_REVISION_INVALID');
  return { key, revision: value.revision, sha256: sha256(canonicalize(value)) };
}

export function nextRecord(record, fields) {
  requireCondition(Number.isSafeInteger(record.revision) && record.revision >= 0
    && Number.isSafeInteger(record.revision + 1), 'MODEL_REVISION_INVALID');
  return { ...structuredClone(record), ...structuredClone(fields), revision: record.revision + 1 };
}

export function atomicPlan(records, reads, writes, facts, commitAt) {
  requireCondition(Number.isSafeInteger(commitAt) && commitAt >= 0, 'TRUSTED_TIME_REQUIRED');
  requireCondition(new Set(reads).size === reads.length
    && new Set(writes.map((write) => write.key)).size === writes.length, 'MODEL_WRITE_SET_INVALID');
  const readSet = reads.map((key) => captureRecord(records, key));
  for (const write of writes) {
    requireCondition(reads.includes(write.key) || !Object.hasOwn(records, write.key), 'MODEL_READ_SET_INCOMPLETE');
    requireCondition(Number.isSafeInteger(write.value.revision) && (Object.hasOwn(records, write.key)
      ? write.value.revision === records[write.key].revision + 1 : write.value.revision >= 1), 'MODEL_REVISION_INVALID');
  }
  const plan = { commitAt, readSet, absentKeys: writes.filter((write) => !Object.hasOwn(records, write.key)).map((write) => write.key),
    writes: structuredClone(writes), facts: structuredClone(facts) };
  planned.set(plan, sha256(canonicalize(plan)));
  return plan;
}

/** Refuse every conflict before applying any write. Faults before commitment
 * leave the input untouched; responses can be lost after this indivisible step.
 */
export function applyAtomically(records, plan, now) {
  requireCondition(planned.has(plan) && planned.get(plan) === sha256(canonicalize(plan)), 'MODEL_PLAN_CHANGED');
  // A plan represents evaluation at the final atomic boundary, not a prepared
  // durable DB command. If commitment happens later, evaluate the command again
  // with that time and the current snapshot. No precomputed timed guard survives.
  requireCondition(Number.isSafeInteger(now) && now === plan.commitAt, 'MODEL_FINAL_GUARDS_REQUIRED');
  for (const read of plan.readSet) requireCondition(Object.hasOwn(records, read.key)
    && sameJson(captureRecord(records, read.key), read), 'MODEL_CAS_CONFLICT');
  requireCondition(plan.absentKeys.every((key) => !Object.hasOwn(records, key)), 'MODEL_CAS_CONFLICT');
  const result = structuredClone(records);
  for (const write of plan.writes) result[write.key] = structuredClone(write.value);
  return { records: result, facts: structuredClone(plan.facts) };
}

export function assertWorkerLease(operation, expectedEpoch, now) {
  requireCondition(operation.state === 'processing' && operation.ownerEpoch === expectedEpoch
    && Number.isSafeInteger(now) && now >= operation.ownerStartedAt
    && now < operation.leaseExpiresAt && now < operation.ownerStartedAt + 120_000,
    'MODEL_OWNER_FENCED');
}

export function operationReplay(operation, requestDigest, now) {
  requireCondition(operation.requestDigest === requestDigest, 'IDEMPOTENCY_CONFLICT');
  if (operation.state !== 'committed') return { kind: 'processing' };
  return now < operation.committedAt + 86_400_000
    ? { kind: 'complete', result: structuredClone(operation.result) }
    : { kind: 'minimal', resultId: operation.resultId };
}
