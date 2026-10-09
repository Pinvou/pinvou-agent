import { requireCondition } from '../errors.mjs';
import { sameJson } from '../semantics.mjs';
import { assertApiShape } from '../api-registry.mjs';
import { readRecord, objectHash } from './inputs.mjs';
import { captureRecord } from './atomic.mjs';

/** T07 supplies immutable historical-chain identity from protected storage.
 * This freezes exact bindings/CAS; physical chain storage/verification remains
 * T07's responsibility, and an untrusted archive cannot establish a new anchor.
 */
export function rootLineageReads(records, key, rootBody) {
  const lineage = readRecord(records, key); assertApiShape('root-lineage', lineage);
  const anchor = readRecord(records, lineage.anchorKey); const material = readRecord(records, lineage.chainMaterialKey);
  assertApiShape('root-chain-material', material);
  requireCondition(lineage.product === rootBody.product && lineage.rootVersion === rootBody.version
    && lineage.rootBodySha256 === objectHash(rootBody) && anchor.recordKind === 'initialRootAnchor'
    && anchor.product === rootBody.product && anchor.state === 'provisioned' && anchor.bodySha256 === lineage.anchorBodySha256
    && material.product === rootBody.product && material.materialId === lineage.chainMaterialId
    && material.materialSha256 === lineage.chainMaterialSha256 && material.rootVersion === lineage.rootVersion
    && material.rootBodySha256 === lineage.rootBodySha256
    && (rootBody.version === 1 ? lineage.anchorBodySha256 === lineage.rootBodySha256 && material.previousRootBodySha256 === null
      : material.previousRootBodySha256 !== null && material.rootEnvelopeSha256 !== null)
    && [lineage.anchorKey, lineage.chainMaterialKey].every((ref) => lineage.readSet.some((read) => read.key === ref))
    && new Set(lineage.readSet.map((read) => read.key)).size === lineage.readSet.length, 'MODEL_ROOT_LINEAGE_INVALID');
  for (const read of lineage.readSet) requireCondition(sameJson(captureRecord(records, read.key),
    { key: read.key, revision: read.revision, sha256: read.sha256 }) && records[read.key].recordKind === read.recordKind,
  'MODEL_CAS_CONFLICT');
  return [key, ...lineage.readSet.map((read) => read.key)];
}
