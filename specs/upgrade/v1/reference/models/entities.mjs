import { requireCondition } from '../errors.mjs';
import { compareVersions, sameJson } from '../semantics.mjs';
import { assertEntityEdge } from './state-graphs.mjs';
import { nextRecord } from './atomic.mjs';
import { commandPlan } from './operation.mjs';
import { readRecord, assertFields, objectHash } from './inputs.mjs';
import { canonicalize } from '../canonical-json.mjs';
import { verifyEnvelope } from '../signatures.mjs';
import { verifyReleasePackage, verifyPublicChain } from '../metadata-chain.mjs';
import { assertApiShape } from '../api-registry.mjs';
import { currentReleaseEnvelope, deniedKeys, deniedKeyOptions, assertNotDenied } from './metadata-inputs.mjs';

/** Typed read-only chain interface. Its keys point to independently owned
 * records. Every member joins the final read set; no 'eligible' boolean suffices.
 */
export function chainReads(records, chain, now, { phase = 'display', bridge = false, scheduled = false, resuming = false, expectedScope } = {}) {
  const deployment = readRecord(records, chain.deploymentKey); const release = readRecord(records, chain.releaseKey);
  const target = readRecord(records, chain.releaseTargetKey); const supply = readRecord(records, chain.supplyChainKey);
  const root = readRecord(records, chain.rootKey);
  const head = readRecord(records, release.metadataHeadKey);
  requireCondition(root.published === true && root.revision > 0 && expectedScope !== undefined && root.product === expectedScope.product
    && deployment.component === expectedScope.component && deployment.channel === expectedScope.channel
    && deployment.targetKey === expectedScope.targetKey
    && target.packageEnvelope.signed.scope.targetKey === expectedScope.targetKey, 'MODEL_SCOPE_INVALID');
  const envelope = currentReleaseEnvelope(records, chain.releaseKey);
  const releaseClaims = verifyEnvelope(canonicalize(envelope), { trustedRoot: root.body, now,
    deniedKeyIds: deniedKeys(records, chain.denyKey, 'release'),
    expected: { role: 'release', product: root.product, component: deployment.component,
      scope: envelope.signed.scope } }).signed;
  const targetEntry = releaseClaims.targets.find((entry) => entry.releaseTargetId === target.releaseTargetId);
  requireCondition(releaseClaims.appVersion === release.appVersion
    && targetEntry !== undefined && targetEntry.releaseTargetRevision === target.revision
    && targetEntry.targetKey === target.packageEnvelope.signed.scope.targetKey, 'MODEL_MANIFEST_BINDING_INVALID');
  verifyReleasePackage({ trustedRoot: root.body, product: root.product, component: deployment.component, now,
    release: releaseClaims, targetKey: target.packageEnvelope.signed.scope.targetKey,
    packageBytes: canonicalize(target.packageEnvelope), deniedKeyIds: deniedKeys(records, chain.denyKey, 'package') });
  if (head.published) {
    const verified = verifyPublicChain({ trustedRoot: root.body, product: root.product, component: deployment.component, now,
      timestampBytes: canonicalize(head.bundle.timestamp), snapshotBytes: canonicalize(head.bundle.snapshot),
      members: head.bundle.members.map(canonicalize), ...deniedKeyOptions(records, chain.denyKey) });
    requireCondition(scheduled || verified.targets.some((item) => item.scope.channel === deployment.channel
      && item.scope.targetKey === expectedScope.targetKey), 'MODEL_CHAIN_INELIGIBLE');
  } else requireCondition(scheduled, 'MODEL_CHAIN_INELIGIBLE');
  assertNotDenied(records, chain.denyKey, [['deployment', deployment.deploymentId], ['release', release.releaseId],
    ['releaseTarget', target.releaseTargetId], ['package', target.packageEnvelope.signed.fullPackage.packageId],
    ...chain.artifactKeys.map((key) => ['artifact', key])]);
  assertFields(deployment, { releaseVisibleAt: 'integer', installNotBefore: 'integer', installNotAfter: 'nullable-integer',
    channel: ['stable', 'beta', 'internal'], releaseKey: 'id', releaseTargetKey: 'id' });
  requireCondition((deployment.installNotAfter === null || deployment.releaseVisibleAt < deployment.installNotAfter
    && deployment.installNotBefore < deployment.installNotAfter)
    && (deployment.upgradeType !== 'forced' || deployment.releaseVisibleAt <= deployment.installNotBefore), 'MODEL_CONFIGURATION_INVALID');
  requireCondition(deployment.state === (bridge ? 'superseded' : scheduled ? 'scheduled' : resuming ? 'paused' : 'active')
    && deployment.releaseKey === chain.releaseKey && deployment.releaseTargetKey === chain.releaseTargetKey
    && release.state === 'closed' && target.state === 'approved' && target.releaseKey === chain.releaseKey
    && supply.state === 'approved' && supply.channel === deployment.channel && supply.releaseTargetKey === chain.releaseTargetKey
    && release.appVersion === deployment.appVersion
    && (phase === 'schedule' || now >= deployment.releaseVisibleAt) && (deployment.installNotAfter === null || now < deployment.installNotAfter)
    && (supply.effectiveExpiresAt === null || Number.isSafeInteger(supply.effectiveExpiresAt) && now < supply.effectiveExpiresAt)
    && (phase !== 'install' || now >= deployment.installNotBefore), 'MODEL_CHAIN_INELIGIBLE');
  requireCondition(Array.isArray(chain.artifactKeys) && chain.artifactKeys.length > 0
    && sameJson([...chain.artifactKeys].sort(), [...target.artifactKeys].sort()), 'MODEL_READ_SET_INCOMPLETE');
  const reads = [chain.rootKey, chain.denyKey, release.metadataHeadKey, chain.deploymentKey, chain.releaseKey,
    chain.releaseTargetKey, chain.supplyChainKey, ...chain.artifactKeys];
  for (const key of chain.artifactKeys) requireCondition(readRecord(records, key).state === 'valid', 'MODEL_CHAIN_INELIGIBLE');
  if (bridge) {
    const eligibility = readRecord(records, chain.bridgeKey); reads.push(chain.bridgeKey);
    requireCondition(eligibility.state === 'enabled' && eligibility.deploymentKey === chain.deploymentKey
      && eligibility.deploymentRevision === deployment.revision
      && eligibility.channel === deployment.channel && now >= eligibility.validFrom
      && (eligibility.expiresAt === null || now < eligibility.expiresAt), 'MODEL_BRIDGE_INELIGIBLE');
  }
  return reads;
}
export function assertCandidateAboveBaseline(records, scope, deployment) {
  requireCondition(scope.state === 'active' && scope.baselineDeploymentKey !== null, 'MODEL_BASELINE_INVALID');
  const baseline = readRecord(records, scope.baselineDeploymentKey);
  requireCondition(compareVersions(deployment.appVersion, baseline.appVersion) > 0, 'CANDIDATE_NOT_ABOVE_BASELINE');
  return scope.baselineDeploymentKey;
}
export function approvalContext(command, scope, objectId, objectRevision) {
  return { command, scope: { product: scope.product, component: scope.component, channel: scope.channel ?? null,
    targetKey: scope.targetKey ?? null }, objectId, objectRevision };
}
export function approvalReads(records, key, approvedBodyHash, requiredReviews, context) {
  const approval = readRecord(records, key);
  assertApiShape('approval-evidence', approval);
  requireCondition(approval.state === 'approved' && approval.bodySha256 === approvedBodyHash
    && context !== undefined && sameJson(approval.context, context)
    && Array.isArray(approval.reviewerIds) && new Set(approval.reviewerIds).size === approval.reviewerIds.length
    && approval.reviewerIds.length >= requiredReviews && !approval.reviewerIds.includes(approval.authorId), 'MODEL_APPROVAL_INVALID');
  return [key];
}
export function releaseCancellationBody(records, releaseKey) {
  const release = readRecord(records, releaseKey);
  return { command: 'CancelRelease', releaseKey, releaseRevision: release.revision,
    targets: release.targetKeys.map((key) => ({ key, revision: readRecord(records, key).revision })) };
}

export function planReleaseTransition(records, command, now) {
  const parent = readRecord(records, command.releaseKey);
  requireCondition(Array.isArray(parent.targetKeys) && parent.targetKeys.length > 0
    && new Set(parent.targetKeys).size === parent.targetKeys.length, 'MODEL_RELEASE_INVALID');
  const targets = parent.targetKeys.map((key) => readRecord(records, key));
  requireCondition(targets.every((target) => target.releaseKey === command.releaseKey), 'MODEL_RELEASE_INVALID');
  const reads = [command.releaseKey, ...parent.targetKeys]; const writes = [];
  assertEntityEdge('release', parent.state, command.toState);
  if (command.toState === 'assembled') {
    requireCondition(parent.frozenContentSha256 === objectHash(parent.content)
      && targets.every((target) => target.state === 'draft'), 'MODEL_RELEASE_INVALID');
  } else if (command.toState === 'closed') {
    requireCondition(targets.every((target) => ['approved', 'rejected', 'revoked', 'cancelled'].includes(target.state))
      && targets.some((target) => target.state === 'approved'), 'MODEL_RELEASE_NOT_TERMINAL');
  } else {
    if (command.actor === 'automatic') requireCondition(targets.every((target) => ['rejected', 'revoked', 'cancelled'].includes(target.state)),
      'MODEL_RELEASE_NOT_TERMINAL');
    else {
      reads.push(...approvalReads(records, command.approvalKey, objectHash(releaseCancellationBody(records, command.releaseKey)), 2,
        approvalContext('CancelRelease', parent, command.releaseKey, parent.revision)));
      for (let i = 0; i < targets.length; i++) if (['draft', 'in_review', 'approved'].includes(targets[i].state)) writes.push({
        key: parent.targetKeys[i], value: nextRecord(targets[i], { state: targets[i].state === 'approved' ? 'revoked' : 'cancelled' }) });
    }
  }
  writes.push({ key: command.releaseKey, value: nextRecord(parent, { state: command.toState }) });
  return commandPlan(records, command, now, reads, writes,
    [{ kind: 'release-state', releaseKey: command.releaseKey, state: command.toState, committedAt: now }], { state: command.toState });
}
