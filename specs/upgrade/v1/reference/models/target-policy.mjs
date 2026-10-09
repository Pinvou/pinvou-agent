import { requireCondition } from '../errors.mjs';
import { sameJson } from '../semantics.mjs';
import { readRecord, objectHash } from './inputs.mjs';
import { denyRecord } from './metadata-inputs.mjs';

// These publication compositions do not own OS policy or support-floor edits.
// A future owning command must submit their protected facts and approvals too.
export function targetConfiguration({ version, issuedAt, expiresAt, selectionGeneration,
  baselineRelease, rolloutSetCommitment, ordinaryPaths, bridgePaths, ...configuration }) {
  return configuration;
}

export function assertUnchangedPaths(previous, next) {
  const identities = (paths) => paths.map(({ release, ...path }) => ({ ...path,
    release: { role: release.role, scope: release.scope } }));
  requireCondition(sameJson(identities(previous.ordinaryPaths), identities(next.ordinaryPaths))
    && sameJson(identities(previous.bridgePaths), identities(next.bridgePaths)), 'MODEL_SELECTION_CHANGED');
}

/** Reconciliation may remove a previously published path only when protected
 * current facts prove it unavailable. Missing records, changed references or a
 * stale global metadata head are not proof of a path-specific safety loss.
 */
export function reconciliationPathReads(records, proposal, scope, previous, next, now) {
  const inventory = readRecord(records, proposal.inventoryKey); const deny = denyRecord(records, proposal.denyKey);
  const reads = [];
  const take = (key) => { reads.push(key); return readRecord(records, key); };
  const unique = (predicate) => {
    const keys = inventory.requiredEntityKeys.filter((key) => predicate(records[key]));
    requireCondition(keys.length === 1, 'MODEL_RECONCILIATION_CHANGED'); return keys[0];
  };
  const identity = ({ release, ...path }) => ({ ...path, release: { role: release.role, scope: release.scope } });
  for (const kind of ['ordinaryPaths', 'bridgePaths']) {
    const originals = previous[kind]; const retained = next[kind];
    requireCondition(retained.every((path) => originals.some((old) => sameJson(identity(old), identity(path))))
      && new Set(retained.map((path) => objectHash(identity(path)))).size === retained.length, 'MODEL_SELECTION_CHANGED');
    for (const old of originals) {
      if (retained.some((path) => sameJson(identity(old), identity(path)))) continue;
      const deploymentKey = unique((record) => record.recordKind === 'deployment' && record.deploymentId === old.deploymentId
        && record.product === scope.product && record.component === scope.component
        && record.channel === scope.channel && record.targetKey === scope.targetKey);
      const deployment = take(deploymentKey); const chain = deployment.chain;
      const release = take(chain.releaseKey); const target = take(chain.releaseTargetKey); const supply = take(chain.supplyChainKey);
      requireCondition(deployment.releaseKey === chain.releaseKey && deployment.releaseTargetKey === chain.releaseTargetKey
        && release.releaseId === old.release.scope.releaseId && target.releaseKey === chain.releaseKey
        && supply.releaseTargetKey === chain.releaseTargetKey && supply.channel === scope.channel,
      'MODEL_RECONCILIATION_CHANGED');
      const artifacts = chain.artifactKeys.map(take); const envelope = records[release.metadataHeadKey].releaseEnvelopes[release.releaseId];
      reads.push(release.metadataHeadKey);
      requireCondition(envelope !== undefined, 'MODEL_RECONCILIATION_CHANGED');
      const subjects = [['deployment', deployment.deploymentId], ['release', release.releaseId], ['releaseTarget', target.releaseTargetId],
        ['package', target.packageEnvelope.signed.fullPackage.packageId], ...chain.artifactKeys.map((key) => ['artifact', key])];
      const denied = subjects.some(([subjectKind, subjectId]) => deny.entries.some((entry) => entry.subjectKind === subjectKind && entry.subjectId === subjectId))
        || [envelope, target.packageEnvelope].some((item) => item.signatures.some((signature) => deny.entries.some((entry) =>
          entry.subjectKind === 'signingKey' && entry.subjectId === signature.keyId && entry.roles.includes(item.signed.role))));
      let unavailable = denied || deployment.state !== (kind === 'bridgePaths' ? 'superseded' : 'active')
        || deployment.revision !== old.deploymentRevision
        || release.state !== 'closed' || target.state !== 'approved' || supply.state !== 'approved'
        || artifacts.some((artifact) => artifact.state !== 'valid')
        || deployment.installNotAfter !== null && now >= deployment.installNotAfter
        || supply.effectiveExpiresAt !== null && now >= supply.effectiveExpiresAt;
      if (kind === 'bridgePaths') {
        const eligibility = take(chain.bridgeKey);
        requireCondition(eligibility.eligibilityId === old.eligibilityId && eligibility.deploymentKey === deploymentKey,
          'MODEL_RECONCILIATION_CHANGED');
        unavailable ||= eligibility.state !== 'enabled' || eligibility.expiresAt !== null && now >= eligibility.expiresAt;
        unavailable ||= eligibility.revision !== old.eligibilityRevision;
      } else {
        const approval = take(unique((record) => record.approvalId === old.approvalId && record.deploymentKey === deploymentKey));
        unavailable ||= approval.state !== 'approved' || approval.revision !== old.approvalRevision;
        if (deployment.rolloutKey !== null) {
          const rollout = take(deployment.rolloutKey);
          requireCondition(rollout.deploymentKey === deploymentKey, 'MODEL_RECONCILIATION_CHANGED');
          unavailable ||= ['paused', 'aborted'].includes(rollout.state);
        }
      }
      requireCondition(unavailable, 'MODEL_SELECTION_CHANGED');
    }
  }
  requireCondition(reads.every((key) => proposal.readSet.some((read) => read.key === key)), 'MODEL_READ_SET_INCOMPLETE');
  return reads;
}
