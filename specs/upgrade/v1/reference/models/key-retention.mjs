import { requireCondition } from '../errors.mjs';
import { sameJson } from '../semantics.mjs';
import { validateTrustedRoot } from '../signatures.mjs';
import { assertApiShape } from '../api-registry.mjs';
import { readRecord } from './inputs.mjs';
import { captureRecord } from './atomic.mjs';

/** T07 maintains this protected inventory with actual issuer last-use facts.
 * It is a finite online verification obligation, not historical Package storage.
 * Owners must join last-use/index writes to issuance before publishing a token.
 */
export function keyRetentionReads(records, indexKey, product, successor, now) {
  const index = readRecord(records, indexKey); assertApiShape('key-use-index', index);
  const actual = Object.entries(records).filter(([, value]) => value.recordKind === 'keyUse' && value.product === product)
    .map(([key]) => key).sort();
  requireCondition(index.product === product && sameJson(actual, [...index.usageKeys].sort()), 'MODEL_KEY_USE_INCOMPLETE');
  const successorKeys = validateTrustedRoot(successor); const reads = [indexKey];
  for (const key of index.usageKeys) {
    const use = readRecord(records, key); assertApiShape('key-use', use); reads.push(key);
    requireCondition(use.product === product && use.threshold <= use.keys.length
      && use.lastIssuedAt <= now && use.lastIssuedAt <= use.objectValidUntil
      && use.lastIssuedAt <= use.uploadUntil && use.lastIssuedAt <= use.recoveryUntil
      && Number.isSafeInteger(Math.max(use.objectValidUntil, use.uploadUntil, use.recoveryUntil) + 120_000)
      && use.retainUntil >= Math.max(use.objectValidUntil, use.uploadUntil, use.recoveryUntil) + 120_000
      && new Set(use.keys.map((item) => item.keyId)).size === use.keys.length
      && new Set(use.readSet.map((read) => read.key)).size === use.readSet.length, 'MODEL_KEY_RETENTION_INVALID');
    for (const read of use.readSet) {
      requireCondition(sameJson(captureRecord(records, read.key), { key: read.key, revision: read.revision, sha256: read.sha256 })
        && records[read.key].recordKind === read.recordKind, 'MODEL_CAS_CONFLICT');
      reads.push(read.key);
    }
    if (use.issuanceState === 'stopped' && now >= use.retainUntil) continue;
    const policy = successor.roles.find((entry) => entry.role === use.role && entry.component === use.component
      && entry.channel === use.channel && entry.targetKey === use.targetKey);
    requireCondition(policy !== undefined && policy.threshold <= use.threshold
      && use.keys.every((oldKey) => policy.keyIds.includes(oldKey.keyId) && sameJson(successorKeys.get(oldKey.keyId), oldKey)),
    'MODEL_KEY_RETENTION_INVALID');
  }
  return [...new Set(reads)];
}
