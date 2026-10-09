import { requireCondition } from '../errors.mjs';
import { canonicalize } from '../canonical-json.mjs';
import { verifyEnvelope, assertReference } from '../signatures.mjs';
import { nextRecord } from './atomic.mjs';
import { commandPlan } from './operation.mjs';
import { readRecord, objectHash } from './inputs.mjs';
import { approvalReads, approvalContext } from './entities.mjs';
import { deniedKeys, assertNotDenied } from './metadata-inputs.mjs';

/** A pre-publication Target review uses the frozen assembly and signed Package.
 * The public signed Release is produced after closing; this model does not
 * require an invented already-approved public Release during in_review.
 */
export function planReviewReleaseTarget(records, command, now) {
  const target = readRecord(records, command.targetKey); const parent = readRecord(records, target.releaseKey);
  requireCondition(parent.state === 'assembled' && parent.frozenContentSha256 === objectHash(parent.content)
    && target.revision === command.expectedRevision && target.frozenContentSha256 === objectHash(target.content)
    && parent.content.targets.some((item) => item.targetKey === command.targetKey && item.contentSha256 === target.frozenContentSha256),
  'MODEL_TARGET_REVIEW_INVALID');
  const to = command.toState;
  requireCondition(target.state === 'draft' && to === 'in_review'
    || target.state === 'in_review' && ['approved', 'rejected'].includes(to), 'MODEL_EDGE_INVALID');
  const reads = [command.targetKey, target.releaseKey];
  if (to !== 'rejected') {
    const root = readRecord(records, command.rootKey); reads.push(command.rootKey, command.denyKey);
    requireCondition(root.published === true && root.revision > 0, 'MODEL_ROOT_NOT_PUBLISHED');
    const manifest = verifyEnvelope(canonicalize(target.packageEnvelope), { trustedRoot: root.body, now,
      expected: { role: 'package', product: parent.product, component: parent.component, scope: { targetKey: target.targetKey } },
      deniedKeyIds: deniedKeys(records, command.denyKey, 'package') }).signed;
    assertNotDenied(records, command.denyKey, [['releaseTarget', target.releaseTargetId],
      ['package', manifest.fullPackage.packageId], ...target.artifactKeys.map((key) => ['artifact', key])]);
    assertReference(target.content.packageManifest, canonicalize(target.packageEnvelope));
    requireCondition(parent.content.appVersion === manifest.appVersion && target.content.activationModes.length > 0
      && target.content.activationModes.every((mode) => manifest.activationModes.includes(mode))
      && (target.content.migrationMode !== 'irreversible' || target.content.backupPolicy === 'required')
      && target.artifactKeys.length > 0 && new Set(target.artifactKeys).size === target.artifactKeys.length,
    'MODEL_TARGET_REVIEW_INVALID');
    for (const key of target.artifactKeys) {
      const artifact = readRecord(records, key); reads.push(key);
      requireCondition(artifact.state === 'valid' && artifact.packageEnvelopeSha256 === objectHash(target.packageEnvelope), 'MODEL_CHAIN_INELIGIBLE');
    }
    const supply = readRecord(records, command.supplyKey); reads.push(command.supplyKey);
    requireCondition(supply.state === 'approved' && supply.targetContentSha256 === target.frozenContentSha256
      && (supply.effectiveExpiresAt === null || now < supply.effectiveExpiresAt), 'MODEL_CHAIN_INELIGIBLE');
    if (to === 'approved') {
      const certificate = readRecord(records, command.certificationKey); reads.push(command.certificationKey);
      requireCondition(certificate.projectionOwner === 'T46' && certificate.state === 'passed'
        && certificate.targetContentSha256 === target.frozenContentSha256
        && target.content.activationModes.every((mode) => certificate.activationModes.includes(mode)), 'MODEL_CERTIFICATION_INVALID');
      reads.push(...approvalReads(records, command.approvalKey, objectHash({ command: 'ApproveReleaseTarget', targetKey: command.targetKey,
        targetRevision: target.revision, releaseRevision: parent.revision, contentSha256: target.frozenContentSha256,
        certificationSha256: objectHash(certificate), supplySha256: objectHash(supply) }), 1,
      approvalContext('ApproveReleaseTarget', { product: parent.product, component: parent.component, targetKey: target.targetKey }, command.targetKey, target.revision)));
    }
  }
  return commandPlan(records, command, now, [...new Set(reads)], [{ key: command.targetKey, value: nextRecord(target, { state: to }) }],
    [{ kind: `release-target-${to}`, committedAt: now }], { state: to });
}
