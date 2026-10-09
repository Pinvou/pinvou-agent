import { requireCondition } from '../errors.mjs';
import { canonicalize } from '../canonical-json.mjs';
import { sha256 } from '../digests.mjs';
import { sameJson } from '../semantics.mjs';
import { captureRecord } from './atomic.mjs';
import { assertApiShape } from '../api-registry.mjs';

export function assertInteger(value, minimum = 0) {
  requireCondition(Number.isSafeInteger(value) && value >= minimum, 'MODEL_INPUT_INVALID');
  return value;
}
export function assertIdentity(value) {
  requireCondition(typeof value === 'string' && /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/.test(value), 'MODEL_INPUT_INVALID');
  return value;
}
export function assertHash(value) {
  requireCondition(typeof value === 'string' && /^[0-9a-f]{64}$/.test(value), 'MODEL_INPUT_INVALID');
  return value;
}
export function readRecord(records, key) {
  assertIdentity(key); captureRecord(records, key); return records[key];
}
export function assertFields(record, fields) {
  requireCondition(record !== null && typeof record === 'object', 'MODEL_INPUT_INVALID');
  for (const [key, type] of Object.entries(fields)) {
    requireCondition(Object.hasOwn(record, key), 'MODEL_INPUT_INVALID');
    const value = record[key];
    if (type === 'integer') assertInteger(value);
    else if (type === 'positive') assertInteger(value, 1);
    else if (type === 'id') assertIdentity(value);
    else if (type === 'hash') assertHash(value);
    else if (type === 'nullable-integer') { if (value !== null) assertInteger(value); }
    else if (type === 'nullable-hash') { if (value !== null) assertHash(value); }
    else if (Array.isArray(type)) requireCondition(type.includes(value), 'MODEL_INPUT_INVALID');
    else requireCondition(false, 'MODEL_INPUT_INVALID');
  }
}
export const objectHash = (value) => sha256(canonicalize(value));
export function selectionIdentitySha256(update) {
  return objectHash({ endpointId: update.endpointChain.deploymentId, hopId: update.hopChain.deploymentId,
    releaseTargetId: update.hopChain.releaseTargetId, packageId: update.packageId,
    packageManifestSha256: update.hopChain.packageManifest.envelopeSha256, package: update.hopChain.package,
    finalInstaller: update.finalInstaller, targetVersion: update.targetVersion, upgradeType: update.upgradeType,
    activationMode: update.activationMode, migrationMode: update.migrationMode, backupPolicy: update.backupPolicy, plans: update.plans });
}

/** A projection is a server-owned immutable snapshot. Models compare its
 * complete typed dependency set; wire clients cannot supply a qualification.
 * projectionOwner and dependencies are registered by its owning module.
 */
export function projectionReads(records, key, { owner, now, context, requiredKinds }) {
  const projection = readRecord(records, key);
  assertFields(projection, { projectionOwner: [owner], state: ['qualified'], qualifiedAt: 'integer', expiresAt: 'integer' });
  assertInteger(now);
  requireCondition(now >= projection.qualifiedAt && now < projection.expiresAt
    && sameJson(projection.context, context) && Array.isArray(projection.readSet)
    && projection.readSet.length > 0 && new Set(projection.readSet.map((item) => item.key)).size === projection.readSet.length,
  'MODEL_QUALIFICATION_LOST');
  const kinds = new Set();
  for (const read of projection.readSet) {
    requireCondition(sameJson(captureRecord(records, read.key), { key: read.key, revision: read.revision, sha256: read.sha256 }),
      'MODEL_QUALIFICATION_LOST');
    const record = records[read.key];
    requireCondition(record.recordKind === read.recordKind, 'MODEL_READ_SET_INCOMPLETE');
    kinds.add(read.recordKind);
  }
  requireCondition(requiredKinds.every((kind) => kinds.has(kind)), 'MODEL_READ_SET_INCOMPLETE');
  return [key, ...projection.readSet.map((read) => read.key)];
}
export const QUALIFICATION_KINDS = Object.freeze(['rootHead', 'metadataHead', 'channel', 'selectionScope', 'registry',
  'deny', 'baseline', 'endpoint', 'hop', 'artifacts', 'supplyChains']);

export function qualificationReads(records, key, now, context, owner = 'T19', claims = null, requiredUpdate = null) {
  const projection = readRecord(records, key);
  assertApiShape('qualification-projection', projection);
  const kinds = [...QUALIFICATION_KINDS];
  const update = claims?.update ?? requiredUpdate;
  if (update !== null) requireCondition(projection.endpointKind === update.endpointKind
    && projection.hopKind === update.hopKind && projection.ordinaryPathRequired === (update.ordinaryPathApproval !== null),
  'MODEL_READ_SET_INCOMPLETE');
  if (projection.endpointKind === 'candidate') kinds.push('rollout');
  if (projection.hopKind === 'bridge') kinds.push('bridgeEligibility');
  if (projection.ordinaryPathRequired === true) kinds.push('ordinaryPathApproval');
  requireCondition(['baseline', 'candidate'].includes(projection.endpointKind)
    && ['ordinary', 'bridge'].includes(projection.hopKind)
    && typeof projection.ordinaryPathRequired === 'boolean', 'MODEL_INPUT_INVALID');
  assertHash(projection.qualifiedClaimsSha256);
  if (claims !== null) requireCondition(projection.qualifiedClaimsSha256 === objectHash(claims), 'MODEL_QUALIFICATION_LOST');
  return projectionReads(records, key, { owner, now, context, requiredKinds: kinds });
}
