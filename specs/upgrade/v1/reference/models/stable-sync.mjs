import { requireCondition } from '../errors.mjs';
import { compareVersions, sameJson } from '../semantics.mjs';
import { assertApiContract } from '../api-registry.mjs';
import { atomicPlan, nextRecord } from './atomic.mjs';
import { commandPlan } from './operation.mjs';
import { readRecord, objectHash } from './inputs.mjs';
import { chainReads } from './entities.mjs';
import { currentReleaseEnvelope, currentReleaseHash, deniedKeyOptions } from './metadata-inputs.mjs';
import { verifyPublicChain } from '../metadata-chain.mjs';
import { canonicalize } from '../canonical-json.mjs';
import { assertOpening } from '../semantics.mjs';

/** Draft-only T16 composition. Permanent batch/target mappings outlive the
 * ordinary replay window and never grant target-channel approval or eligibility.
 */
export function planSynchronizeStable(records, command, now) {
  const request = assertApiContract('synchronize-stable', 'request', command.request);
  const index = readRecord(records, command.syncIndexKey);
  const permission = readRecord(records, command.permissionKey);
  requireCondition(typeof command.operatorId === 'string' && request.scope.component !== null
    && permission.operatorId === command.operatorId && permission.product === request.scope.product
    && permission.component === request.scope.component && permission.state === 'active' && now < permission.expiresAt
    && request.selections.every((selection) => permission.grants.some((grant) => grant.channel === selection.channel
      && grant.targetKey === selection.targetKey)), 'MODEL_SYNC_PERMISSION_INVALID');
  const digest = objectHash({ request, operatorId: command.operatorId });
  requireCondition(command.requestDigest === digest, 'IDEMPOTENCY_CONFLICT');
  if (Object.hasOwn(index.bySyncKey, request.commandKey)) {
    const mapping = index.bySyncKey[request.commandKey];
    requireCondition(mapping.requestDigest === digest, 'IDEMPOTENCY_CONFLICT');
    const batch = readRecord(records, mapping.batchKey);
    return atomicPlan(records, [command.syncIndexKey, command.permissionKey, mapping.batchKey], [],
      [{ kind: 'stable-sync-replayed', batchId: batch.batchId, draftKeys: batch.draftKeys }], now);
  }
  requireCondition(permission.state === 'active' && now < permission.expiresAt && command.items.length === request.selections.length,
    'MODEL_SYNC_PERMISSION_INVALID');
  const reads = [command.syncIndexKey, command.permissionKey]; const writes = [];
  const pairs = new Set(); const targetMappings = []; const draftKeys = [];
  for (let i = 0; i < command.items.length; i++) {
    const item = command.items[i]; const selection = request.selections[i];
    const source = readRecord(records, item.sourceChain.deploymentKey); const scope = readRecord(records, item.targetScopeKey);
    const sourceScope = readRecord(records, item.sourceScopeKey);
    const sourceRelease = readRecord(records, source.releaseKey);
    const sourceHead = readRecord(records, sourceRelease.metadataHeadKey);
    const current = verifyPublicChain({ trustedRoot: records[item.sourceChain.rootKey].body,
      product: sourceScope.product, component: sourceScope.component, now, timestampBytes: canonicalize(sourceHead.bundle.timestamp),
      snapshotBytes: canonicalize(sourceHead.bundle.snapshot), members: sourceHead.bundle.members.map(canonicalize),
      ...deniedKeyOptions(records, item.sourceChain.denyKey) });
    const currentTarget = current.targets.find((target) => target.scope.channel === 'stable' && target.scope.targetKey === sourceScope.targetKey);
    requireCondition(sourceScope.state === 'active' && sourceScope.channel === 'stable' && sourceScope.metadataSyncPending === false
      && sourceScope.product === request.scope.product && sourceScope.component === request.scope.component
      && sourceScope.targetKey === source.targetKey && sourceScope.targetKey === selection.targetKey
      && currentTarget?.selectionGeneration === sourceScope.selectionGeneration
      && currentTarget.supportFloorVersion === sourceScope.supportFloorVersion, 'MODEL_CHAIN_INELIGIBLE');
    reads.push(item.sourceScopeKey, sourceRelease.metadataHeadKey);
    const sourceHash = currentReleaseHash(records, source.releaseKey);
    if (sourceScope.baselineDeploymentKey === item.sourceChain.deploymentKey) {
      requireCondition(currentTarget.baselineRelease.envelopeSha256 === sourceHash, 'MODEL_CHAIN_INELIGIBLE');
    } else if (sourceScope.runningRolloutKey !== null && sourceScope.runningRolloutKey === source.rolloutKey) {
      const rollout = readRecord(records, source.rolloutKey); reads.push(source.rolloutKey);
      const opening = sourceScope.currentCandidateOpening;
      requireCondition(rollout.state === 'running' && rollout.deploymentKey === item.sourceChain.deploymentKey
        && rollout.scopeKey === item.sourceScopeKey && opening?.deploymentId === source.deploymentId
        && opening.deploymentRevision === source.revision && opening.rolloutId === rollout.rolloutId
        && opening.rolloutRevision === rollout.revision, 'MODEL_CHAIN_INELIGIBLE');
      assertOpening(currentTarget, sourceHash, opening);
    } else {
      const approval = readRecord(records, item.sourceApprovalKey); reads.push(item.sourceApprovalKey);
      requireCondition(approval.state === 'approved' && approval.deploymentKey === item.sourceChain.deploymentKey
        && approval.deploymentRevision === source.revision && approval.channel === 'stable'
        && currentTarget.ordinaryPaths.some((path) => path.deploymentId === source.deploymentId && path.deploymentRevision === source.revision
          && path.approvalId === approval.approvalId && path.approvalRevision === approval.revision
          && path.release.envelopeSha256 === sourceHash), 'MODEL_CHAIN_INELIGIBLE');
      if (source.rolloutKey !== null) {
        const child = readRecord(records, source.rolloutKey); reads.push(source.rolloutKey);
        requireCondition(!['paused', 'aborted'].includes(child.state), 'MODEL_CHAIN_INELIGIBLE');
      }
    }
    const targetIndex = readRecord(records, scope.deploymentIndexKey);
    const existing = Object.entries(records).filter(([, record]) => record.recordKind === 'deployment'
      && record.product === scope.product && record.component === scope.component
      && record.channel === scope.channel && record.targetKey === scope.targetKey).map(([key]) => key).sort();
    requireCondition(targetIndex.scopeKey === item.targetScopeKey && sameJson(existing, [...targetIndex.memberKeys].sort()),
      'MODEL_SYNC_INDEX_INCOMPLETE');
    requireCondition(existing.every((key) => records[key].appVersion !== source.appVersion), 'MODEL_SYNC_DUPLICATE');
    reads.push(scope.deploymentIndexKey, ...existing);
    writes.push({ key: scope.deploymentIndexKey, value: nextRecord(targetIndex, { memberKeys: [...targetIndex.memberKeys, item.draftKey] }) });
    requireCondition(source.deploymentId === selection.sourceStableDeploymentId && source.revision === selection.sourceStableDeploymentRevision
      && source.channel === 'stable' && source.releaseKey === command.sourceReleaseKey
      && source.component === request.scope.component && records[item.sourceChain.rootKey].product === request.scope.product
      && currentReleaseEnvelope(records, command.sourceReleaseKey).signed.scope.releaseId === request.sourceReleaseId
      && scope.channel === selection.channel && scope.targetKey === selection.targetKey
      && scope.product === request.scope.product && scope.component === request.scope.component
      && records[source.releaseTargetKey].packageEnvelope.signed.scope.targetKey === scope.targetKey
      && selection.configuration.releaseId === request.sourceReleaseId
      && selection.configuration.releaseRevision === records[source.releaseKey].revision
      && selection.configuration.releaseTargetId === records[source.releaseTargetKey].releaseTargetId
      && selection.configuration.releaseTargetRevision === records[source.releaseTargetKey].revision
      && permission.grants.some((grant) => grant.channel === scope.channel && grant.targetKey === scope.targetKey), 'MODEL_SYNC_PERMISSION_INVALID');
    const pair = `${scope.product}:${scope.component}:${scope.channel}:${scope.targetKey}`;
    requireCondition(!pairs.has(pair), 'MODEL_SYNC_DUPLICATE'); pairs.add(pair);
    const unique = `${pair}:${source.releaseTargetKey}`;
    requireCondition(!Object.hasOwn(index.byTarget, unique)
      && !Object.values(records).some((record) => record.recordKind === 'deployment'
        && record.channel === scope.channel && record.releaseTargetKey === source.releaseTargetKey), 'MODEL_SYNC_DUPLICATE');
    reads.push(item.targetScopeKey, ...chainReads(records, item.sourceChain, now, {
      expectedScope: { ...scope, channel: 'stable' } }));
    if (scope.state === 'active') {
      const baseline = readRecord(records, scope.baselineDeploymentKey); reads.push(scope.baselineDeploymentKey);
      requireCondition(compareVersions(source.appVersion, baseline.appVersion) > 0, 'CANDIDATE_NOT_ABOVE_BASELINE');
    } else requireCondition(scope.state === 'unactivated' && scope.baselineDeploymentKey === null, 'MODEL_BASELINE_INVALID');
    const deployment = { revision: 1, recordKind: 'deployment', product: scope.product, component: scope.component,
      state: 'draft', channel: scope.channel, targetKey: scope.targetKey,
      releaseKey: source.releaseKey, releaseTargetKey: source.releaseTargetKey, appVersion: source.appVersion,
      configuration: structuredClone(selection.configuration), supplyChainKey: null, approvalKey: null,
      rolloutKey: item.rolloutKey, syncBatchKey: command.batchKey };
    writes.push({ key: item.draftKey, value: deployment }, { key: item.rolloutKey, value: { revision: 1, recordKind: 'rollout', state: 'draft',
      deploymentKey: item.draftKey, scopeKey: item.targetScopeKey, planApprovalKey: null, percentage: 0, syncBatchKey: command.batchKey } });
    writes.push({ key: item.targetScopeKey, value: nextRecord(scope, { rolloutKeys: [...scope.rolloutKeys, item.rolloutKey] }) });
    draftKeys.push({ channel: scope.channel, targetKey: scope.targetKey, deploymentKey: item.draftKey, rolloutKey: item.rolloutKey });
    targetMappings.push([unique, item.draftKey]);
  }
  requireCondition(draftKeys.length > 0, 'MODEL_SYNC_DUPLICATE');
  writes.push({ key: command.batchKey, value: { revision: 1, batchId: command.batchId, requestDigest: digest, draftKeys, committedAt: now } });
  writes.push({ key: command.syncIndexKey, value: nextRecord(index, {
    bySyncKey: { ...index.bySyncKey, [request.commandKey]: { requestDigest: digest, batchKey: command.batchKey } },
    byTarget: { ...index.byTarget, ...Object.fromEntries(targetMappings) },
  }) });
  return commandPlan(records, command, now, [...new Set(reads)], writes,
    [{ kind: 'stable-drafts-created', batchId: command.batchId, committedAt: now }], { batchId: command.batchId, draftKeys });
}
