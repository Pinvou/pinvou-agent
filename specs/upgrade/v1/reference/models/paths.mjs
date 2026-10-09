import { requireCondition } from '../errors.mjs';
import { compareVersions, sameJson } from '../semantics.mjs';
import { assertApiShape } from '../api-registry.mjs';
import { captureRecord } from './atomic.mjs';
import { readRecord, objectHash } from './inputs.mjs';
import { chainReads } from './entities.mjs';
import { verifyPublicChain } from '../metadata-chain.mjs';
import { canonicalize } from '../canonical-json.mjs';
import { currentReleaseEnvelope, deniedKeyOptions, assertNotDenied } from './metadata-inputs.mjs';

/** T13's read-only exact-path port. Search/physical compatibility certification
 * are owned elsewhere. This composition validates every supplied step and its
 * current dependencies; an opaque pathPassed flag cannot replace the port.
 */
export function pathReads(baseRecords, key, deploymentKey, scopeKey, now,
  { phase = 'display', resuming = false, scheduled = false, targetBundle = null, viewOverrides = null } = {}) {
  const records = viewOverrides === null ? baseRecords : { ...baseRecords, ...viewOverrides };
  const projection = readRecord(records, key); assertApiShape('path-projection', projection);
  requireCondition(projection.projectedWritesSha256 === (viewOverrides === null ? null : objectHash(viewOverrides)), 'MODEL_PATH_INVALID');
  const endpoint = readRecord(records, deploymentKey); const scope = readRecord(records, scopeKey);
  const root = readRecord(records, projection.rootKey); const head = readRecord(records, projection.headKey);
  const registry = readRecord(records, projection.registryKey);
  requireCondition(projection.deploymentKey === deploymentKey && projection.deploymentRevision === endpoint.revision
    && projection.scopeKey === scopeKey && projection.qualifiedAt <= now && now < projection.expiresAt
    && projection.rootKey === endpoint.chain.rootKey && root.product === scope.product
    && head.product === scope.product && head.component === scope.component && scope.metadataSyncPending === false,
  'MODEL_PATH_INVALID');
  requireCondition(root.published === true && root.revision > 0, 'MODEL_ROOT_NOT_PUBLISHED');
  const reads = [key, deploymentKey, scopeKey, projection.rootKey, projection.headKey, projection.denyKey, projection.registryKey];
  let applicableTarget = null;
  if (scope.baselineDeploymentKey !== null) reads.push(scope.baselineDeploymentKey);
  if (head.published) {
    const verified = verifyPublicChain({ trustedRoot: root.body, product: head.product, component: head.component, now,
      timestampBytes: canonicalize(head.bundle.timestamp), snapshotBytes: canonicalize(head.bundle.snapshot),
      members: head.bundle.members.map(canonicalize), ...deniedKeyOptions(records, projection.denyKey) });
    const target = verified.targets.find((item) => item.scope.channel === scope.channel && item.scope.targetKey === scope.targetKey);
    requireCondition(scope.state === 'unactivated' ? target === undefined
      : target !== undefined && target.selectionGeneration === scope.selectionGeneration
        && target.supportFloorVersion === scope.supportFloorVersion, 'MODEL_GENERATION_MISMATCH');
    applicableTarget = target ?? null;
  }
  if (targetBundle !== null) {
    const proposed = verifyPublicChain({ trustedRoot: root.body, product: scope.product, component: scope.component, now,
      timestampBytes: canonicalize(targetBundle.timestamp), snapshotBytes: canonicalize(targetBundle.snapshot),
      members: targetBundle.members.map(canonicalize), ...deniedKeyOptions(records, projection.denyKey) });
    applicableTarget = proposed.targets.find((item) => item.scope.channel === scope.channel && item.scope.targetKey === scope.targetKey) ?? null;
  }
  requireCondition(applicableTarget !== null && applicableTarget.supportFloorVersion === scope.supportFloorVersion,
    'MODEL_PATH_INVALID');
  requireCondition(new Set(projection.sources.map((source) => source.sourceProfileId)).size === projection.sources.length,
    'MODEL_PATH_INVALID');
  const requiredSources = registry.profiles.filter((profile) => profile.targetKey === scope.targetKey
    && ['selectable', 'forward-only'].includes(profile.state)
    && compareVersions(profile.canonicalAppVersion, scope.supportFloorVersion) >= 0
    && compareVersions(profile.canonicalAppVersion, endpoint.appVersion) < 0).map((profile) => profile.sourceProfileId).sort();
  requireCondition(sameJson(requiredSources, projection.sources.map((source) => source.sourceProfileId).sort()), 'MODEL_PATH_INVALID');
  for (const source of projection.sources) {
    const profile = registry.profiles.find((item) => item.sourceProfileId === source.sourceProfileId);
    requireCondition(profile !== undefined && ['selectable', 'forward-only'].includes(profile.state)
      && profile.canonicalAppVersion === source.currentVersion
      && compareVersions(source.currentVersion, scope.supportFloorVersion) >= 0
      && source.actualHopIndex === 0, 'MODEL_PATH_INVALID');
    let previousVersion = source.currentVersion;
    let previousProfileId = source.sourceProfileId;
    for (const [index, step] of source.steps.entries()) {
      const deployment = readRecord(records, step.chain.deploymentKey);
      const stepSource = registry.profiles.find((item) => item.sourceProfileId === step.fromSourceProfileId);
      const stepTarget = registry.profiles.find((item) => item.sourceProfileId === step.toSourceProfileId);
      requireCondition(stepSource !== undefined && stepSource.canonicalAppVersion === previousVersion
        && step.fromSourceProfileId === previousProfileId && stepTarget !== undefined
        && stepTarget.canonicalAppVersion === deployment.appVersion && stepTarget.targetKey === scope.targetKey
        && ['selectable', 'forward-only'].includes(stepTarget.state)
        && stepSource.targetKey === scope.targetKey && ['selectable', 'forward-only'].includes(stepSource.state)
        && (index !== 0 || step.fromSourceProfileId === source.sourceProfileId)
        && step.activationMode === (endpoint.upgradeType === 'silent' ? 'stagedRestart' : 'directInstall'), 'MODEL_PATH_INVALID');
      requireCondition(step.chain.rootKey === projection.rootKey && compareVersions(deployment.appVersion, previousVersion) > 0,
        'MODEL_PATH_INVALID');
      reads.push(...chainReads(records, step.chain, now, { expectedScope: scope, phase,
        bridge: step.kind === 'bridge', resuming: resuming && step.chain.deploymentKey === deploymentKey,
        scheduled: scheduled && step.chain.deploymentKey === deploymentKey }));
      requireCondition(step.chain.denyKey === projection.denyKey, 'MODEL_PATH_INVALID');
      const releaseEnvelope = currentReleaseEnvelope(records, step.chain.releaseKey);
      const release = releaseEnvelope.signed;
      const target = release.targets.find((item) => item.releaseTargetId === records[step.chain.releaseTargetKey].releaseTargetId);
      requireCondition(target !== undefined && compareVersions(previousVersion, target.minimumSourceVersion) >= 0
        && target.activationModes.includes(step.activationMode), 'MODEL_PATH_INVALID');
      const certificate = readRecord(records, step.certificationKey); reads.push(step.certificationKey);
      requireCondition(certificate.projectionOwner === 'T46' && certificate.state === 'passed'
        && certificate.product === scope.product && certificate.component === scope.component && certificate.targetKey === scope.targetKey
        && certificate.packageEnvelopeSha256 === objectHash(records[step.chain.releaseTargetKey].packageEnvelope)
        && certificate.sourceProfileId === step.fromSourceProfileId && certificate.activationMode === step.activationMode
        && certificate.targetSourceProfileId === step.toSourceProfileId
        && certificate.upgradeTypes.includes(endpoint.upgradeType),
      'MODEL_CERTIFICATION_INVALID');
      if (step.kind === 'ordinary' && step.chain.deploymentKey !== deploymentKey && step.chain.deploymentKey !== scope.baselineDeploymentKey) {
        const approval = readRecord(records, step.ordinaryApprovalKey); reads.push(step.ordinaryApprovalKey);
        requireCondition(approval.state === 'approved' && approval.deploymentKey === step.chain.deploymentKey
          && approval.deploymentRevision === deployment.revision && approval.channel === scope.channel,
        'MODEL_PATH_INVALID');
        requireCondition(applicableTarget.ordinaryPaths.some((path) => path.deploymentId === deployment.deploymentId
          && path.deploymentRevision === deployment.revision && path.approvalId === approval.approvalId
          && path.approvalRevision === approval.revision
          && path.release.envelopeSha256 === objectHash(releaseEnvelope)), 'MODEL_PATH_INVALID');
      } else if (step.kind === 'ordinary' && step.chain.deploymentKey === scope.baselineDeploymentKey) {
        requireCondition(applicableTarget.baselineRelease.envelopeSha256 === objectHash(releaseEnvelope),
          'MODEL_PATH_INVALID');
      }
      if (step.kind === 'bridge') {
        const eligibility = records[step.chain.bridgeKey];
        requireCondition(applicableTarget.bridgePaths.some((path) => path.deploymentId === deployment.deploymentId
          && path.deploymentRevision === deployment.revision && path.eligibilityId === eligibility.eligibilityId
          && path.eligibilityRevision === eligibility.revision
          && path.release.envelopeSha256 === objectHash(releaseEnvelope)), 'MODEL_PATH_INVALID');
      }
      if (step.kind === 'ordinary' && step.chain.deploymentKey !== deploymentKey && deployment.rolloutKey !== null) {
        const ownRollout = readRecord(records, deployment.rolloutKey); reads.push(deployment.rolloutKey);
        requireCondition(!['paused', 'aborted'].includes(ownRollout.state), 'MODEL_PATH_INVALID');
      }
      if (stepSource.state === 'forward-only' || step.transformKey !== null) {
        const transform = readRecord(records, step.transformKey); reads.push(step.transformKey);
        assertNotDenied(records, projection.denyKey, [['transform', transform.transformId]]);
        requireCondition(transform.state === 'approved' && transform.sourceProfileId === step.fromSourceProfileId
          && transform.targetSourceProfileId === step.toSourceProfileId && transform.targetKey === scope.targetKey
          && transform.packageEnvelopeSha256 === certificate.packageEnvelopeSha256
          && transform.activationMode === step.activationMode && transform.migrationMode === target.migrationMode
          && transform.backupPolicy === target.backupPolicy, 'MODEL_PATH_INVALID');
        if (stepSource.state === 'forward-only') requireCondition(stepSource.forwardEdges.filter((edge) => edge.fromSourceProfileId === step.fromSourceProfileId
        && edge.releaseTargetId === target.releaseTargetId && edge.releaseTargetRevision === target.releaseTargetRevision
        && edge.toCanonicalAppVersion === deployment.appVersion
        && edge.packageId === records[step.chain.releaseTargetKey].packageEnvelope.signed.fullPackage.packageId
        && edge.packageSha256 === records[step.chain.releaseTargetKey].packageEnvelope.signed.fullPackage.sha256
        && edge.transformId === transform.transformId && edge.transformRevision === transform.revision
        && edge.allowedChannels.includes(scope.channel)).length === 1, 'MODEL_PATH_INVALID');
      }
      if (endpoint.upgradeType === 'forced') requireCondition(step.activationMode === 'directInstall'
        && deployment.releaseVisibleAt <= endpoint.installNotBefore && deployment.installNotBefore <= endpoint.installNotBefore
        && (endpoint.installNotAfter === null ? deployment.installNotAfter === null
          : deployment.installNotAfter === null || deployment.installNotAfter >= endpoint.installNotAfter), 'MODEL_FORCED_PATH_INVALID');
      previousVersion = deployment.appVersion;
      previousProfileId = step.toSourceProfileId;
    }
    requireCondition(source.steps.at(-1).chain.deploymentKey === deploymentKey, 'MODEL_PATH_INVALID');
  }
  const uniqueReads = [...new Set(reads)];
  requireCondition(uniqueReads.filter((read) => read !== key).every((read) => projection.readSet.some((item) => item.key === read)),
    'MODEL_READ_SET_INCOMPLETE');
  for (const read of projection.readSet) {
    const actual = captureRecord(baseRecords, read.key);
    requireCondition(sameJson(actual, { key: read.key, revision: read.revision, sha256: read.sha256 })
      && baseRecords[read.key].recordKind === read.recordKind, 'MODEL_CAS_CONFLICT');
  }
  return [...new Set([...uniqueReads, ...projection.readSet.map((item) => item.key)])];
}
