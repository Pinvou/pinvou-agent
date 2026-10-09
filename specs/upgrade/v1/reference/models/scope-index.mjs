import { requireCondition } from '../errors.mjs';
import { sameJson } from '../semantics.mjs';
import { nextRecord } from './atomic.mjs';
import { readRecord } from './inputs.mjs';
import { TRANSACTION_TERMINALS } from './state-graphs.mjs';

export const SCOPE_KINDS = Object.freeze(['session', 'authorization', 'activeValidate', 'transaction', 'staged']);
export function assertScopeIndex(records, key, scopeId) {
  const index = readRecord(records, key);
  const members = Object.entries(records).filter(([, record]) => SCOPE_KINDS.includes(record.recordKind)
    && record.installationScopeId === scopeId).map(([recordKey]) => recordKey).sort();
  requireCondition(index.installationScopeId === scopeId && Array.isArray(index.memberKeys)
    && sameJson(members, [...index.memberKeys].sort()), 'MODEL_SCOPE_INDEX_INCOMPLETE');
  return index;
}
export function scopeIndexWrite(records, key, scopeId, addedKeys) {
  const index = assertScopeIndex(records, key, scopeId);
  requireCondition(addedKeys.length > 0 && new Set(addedKeys).size === addedKeys.length
    && addedKeys.every((member) => !Object.hasOwn(records, member)), 'MODEL_EXECUTION_DUPLICATE');
  return { key, value: nextRecord(index, { memberKeys: [...index.memberKeys, ...addedKeys] }) };
}

// The index serializes creations, while every existing member also joins CAS:
// a concurrent state change must invalidate a precomputed uniqueness decision.
export function scopeOccupancyReads(records, key, scopeId, kind) {
  const index = assertScopeIndex(records, key, scopeId);
  requireCondition(['transaction', 'waiting'].includes(kind), 'MODEL_INPUT_INVALID');
  for (const memberKey of index.memberKeys) {
    const member = readRecord(records, memberKey);
    const occupied = kind === 'transaction'
      ? member.recordKind === 'transaction' && !TRANSACTION_TERMINALS.includes(member.state)
      : member.recordKind === 'staged' && member.state === 'waiting';
    requireCondition(!occupied, 'MODEL_SCOPE_OCCUPIED');
  }
  return [key, ...index.memberKeys];
}
