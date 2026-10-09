import { createFixtureSet, NOW, HASH, TARGET } from './fixtures.mjs';
import { parseJson, canonicalize } from '../canonical-json.mjs';
import { envelopeReference } from '../signatures.mjs';
import { rolloutCommitment } from '../digests.mjs';
import { captureRecord, nextRecord } from '../models/atomic.mjs';
import { publicationParts } from '../models/publication.mjs';
import { objectHash } from '../models/inputs.mjs';
import { operation, operationCommand } from './model-fixtures.mjs';
import { DAY, compareVersions } from '../semantics.mjs';
import { approvalContext } from '../models/entities.mjs';
import { releaseBusinessHash } from '../models/metadata-inputs.mjs';
import { rootChangeBody, signingKeyDenyBody } from '../models/key-approval.mjs';

function lineagePort(records, key, body, anchorKey, envelopeSha256, previousRootBodySha256) {
  const chainMaterialKey = `${key}-material`;
  records[chainMaterialKey] = { revision: 1, recordKind: 'rootChainMaterial', projectionOwner: 'T07', product: body.product,
    materialId: `${key}-history`, materialSha256: objectHash({ body, envelopeSha256, previousRootBodySha256 }),
    rootVersion: body.version, rootBodySha256: objectHash(body), previousRootBodySha256, rootEnvelopeSha256: envelopeSha256 };
  records[key] = { revision: 1, recordKind: 'rootLineage', projectionOwner: 'T07', product: body.product,
    anchorKey, anchorBodySha256: records[anchorKey].bodySha256, rootVersion: body.version, rootBodySha256: objectHash(body),
    chainMaterialKey, chainMaterialId: records[chainMaterialKey].materialId, chainMaterialSha256: records[chainMaterialKey].materialSha256,
    readSet: [anchorKey, chainMaterialKey].map((ref) => ({ ...captureRecord(records, ref), recordKind: records[ref].recordKind })) };
}
// Test-only protected T07 projections, not a physical archive or production key service.
export function initialLineagePort(records) {
  if (records.root.body.version !== 1) return;
  records['trust-anchor'] = { revision: 1, recordKind: 'initialRootAnchor', state: 'provisioned',
    product: 'pinvou', bodySha256: objectHash(records.root.body) };
  lineagePort(records, 'root-lineage-1', records.root.body, 'trust-anchor', null, null);
}

export function approveKeyCommand(records, command, now = NOW) {
  const rootChange = command.successorBytes !== undefined;
  command.actorId = 'operator'; command.approvalKey = 'key-approval'; command.managementAuthorizationKey = 'key-permission';
  if (rootChange) {
    command.denyKey = 'deny'; command.keyUseIndexKey = 'key-uses';
    records['key-uses'] ??= { revision: 1, recordKind: 'keyUseIndex', projectionOwner: 'T07', product: 'pinvou', usageKeys: [] };
    const next = parseJson(command.successorBytes); const anchorKey = records.root.revision === 0 ? command.anchorKey
      : records[records.root.rootLineageKey].anchorKey;
    if (records.root.body.version === 1 && records.root.revision > 0) {
      records[anchorKey].bodySha256 = objectHash(records.root.body);
      lineagePort(records, records.root.rootLineageKey, records.root.body, anchorKey, null, null);
    }
    command.successorLineageKey = `prepared-root-${objectHash(next).slice(0, 16)}`;
    lineagePort(records, command.successorLineageKey, next.signed, anchorKey, objectHash(next),
      records.root.revision === 0 ? null : objectHash(records.root.body));
  }
  const body = rootChange ? rootChangeBody(records, command) : signingKeyDenyBody(records, command);
  const objectKey = rootChange ? command.rootKey : command.denyKey;
  const context = approvalContext(body.command, { product: 'pinvou', component: null }, objectKey, records[objectKey].revision + 1);
  records['key-approval'] = { revision: 1, projectionOwner: 'T05', state: 'approved', bodySha256: objectHash(body),
    authorId: 'operator', reviewerIds: ['reviewer-a', 'reviewer-b'], context };
  records['key-permission'] = { revision: 1, projectionOwner: 'T05', state: 'authorized', actorId: 'operator', mfaVerified: true,
    permissions: ['key-management'], authorizedAt: now, expiresAt: now + 300_000, bodySha256: objectHash(body), context };
}

export function publicationFixture() {
  const set = createFixtureSet();
  const candidatePackage = structuredClone(set.metadata.package); candidatePackage.appVersion = '3.0.0'; candidatePackage.fullPackage.packageId = 'package-3';
  const candidatePackageEnvelope = parseJson(set.sign(candidatePackage));
  const candidateRelease = structuredClone(set.metadata.release); candidateRelease.scope.releaseId = 'release-3'; candidateRelease.appVersion = '3.0.0';
  candidateRelease.targets[0].releaseTargetId = 'target-3'; candidateRelease.targets[0].packageManifest = envelopeReference(canonicalize(candidatePackageEnvelope));
  const candidateReleaseEnvelope = parseJson(set.sign(candidateRelease));
  const bundle = { timestamp: parseJson(set.bytes.timestamp), snapshot: parseJson(set.bytes.snapshot),
    members: ['target', 'release', 'package'].map((role) => parseJson(set.bytes[role])) };
  const chain = (suffix) => ({ rootKey: 'root', denyKey: 'deny', deploymentKey: suffix === '2' ? 'baseline' : 'candidate', releaseKey: `release-${suffix}`,
    releaseTargetKey: `target-${suffix}`, artifactKeys: [`artifact-${suffix}`], supplyChainKey: `supply-${suffix}` });
  const records = {
    root: { revision: 1, published: true, recordKind: 'rootHead', product: 'pinvou', body: set.metadata.root, rootLineageKey: 'root-lineage-1' },
    head: { revision: 1, recordKind: 'metadataHead', product: 'pinvou', component: 'app', published: true, bundle, rootHeadRevisionAtPublish: 1,
      verificationArchiveKey: 'verification-archive',
      releaseEnvelopes: { [set.metadata.release.scope.releaseId]: parseJson(set.bytes.release), 'release-3': candidateReleaseEnvelope } },
    'verification-archive': { revision: 1, recordKind: 'verificationArchive', projectionOwner: 'T07', product: 'pinvou', component: 'app', entries: [] },
    scope: { revision: 1, recordKind: 'selectionScope', product: 'pinvou', component: 'app', state: 'active', channel: 'stable', targetKey: TARGET,
      selectionGeneration: 1, baselineDeploymentKey: 'baseline', runningRolloutKey: null, rolloutKeys: ['rollout'], hasCandidate: false,
      contributionHeadKey: 'contribution-head', metadataSyncPending: false, supportFloorVersion: '1.0.0' },
    baseline: { revision: 1, recordKind: 'deployment', product: 'pinvou', component: 'app', targetKey: TARGET,
      state: 'active', deploymentId: 'deployment-2', appVersion: '2.0.0', releaseKey: 'release-2',
      releaseTargetKey: 'target-2', upgradeType: 'normal', activationMode: 'directInstall',
      releaseVisibleAt: NOW, installNotBefore: NOW, installNotAfter: null, channel: 'stable', chain: chain('2'), rolloutKey: null },
    candidate: { revision: 1, recordKind: 'deployment', product: 'pinvou', component: 'app', targetKey: TARGET,
      state: 'active', deploymentId: 'deployment-3', appVersion: '3.0.0', releaseKey: 'release-3',
      releaseTargetKey: 'target-3', upgradeType: 'normal', activationMode: 'directInstall',
      releaseVisibleAt: NOW, installNotBefore: NOW, installNotAfter: null, channel: 'stable', chain: chain('3'), rolloutKey: 'rollout' },
    'release-2': { revision: 1, recordKind: 'release', state: 'closed', releaseId: set.metadata.release.scope.releaseId,
      frozenBusinessSha256: releaseBusinessHash(set.metadata.release),
      metadataHeadKey: 'head', appVersion: '2.0.0', envelope: parseJson(set.bytes.release), envelopeSha256: objectHash(parseJson(set.bytes.release)) },
    'release-3': { revision: 1, recordKind: 'release', state: 'closed', releaseId: 'release-3', metadataHeadKey: 'head',
      frozenBusinessSha256: releaseBusinessHash(candidateRelease),
      appVersion: '3.0.0', envelope: candidateReleaseEnvelope, envelopeSha256: objectHash(candidateReleaseEnvelope) },
    'target-2': { revision: 1, recordKind: 'releaseTarget', state: 'approved', releaseKey: 'release-2', artifactKeys: ['artifact-2'],
      releaseTargetId: set.metadata.release.targets[0].releaseTargetId, packageEnvelope: parseJson(set.bytes.package) },
    'target-3': { revision: 1, recordKind: 'releaseTarget', state: 'approved', releaseKey: 'release-3', artifactKeys: ['artifact-3'],
      releaseTargetId: 'target-3', packageEnvelope: candidatePackageEnvelope },
    'artifact-2': { revision: 1, recordKind: 'artifact', state: 'valid' }, 'artifact-3': { revision: 1, recordKind: 'artifact', state: 'valid' },
    'supply-2': { revision: 1, recordKind: 'supplyChain', state: 'approved', channel: 'stable', releaseTargetKey: 'target-2', effectiveExpiresAt: null },
    'supply-3': { revision: 1, recordKind: 'supplyChain', state: 'approved', channel: 'stable', releaseTargetKey: 'target-3', effectiveExpiresAt: null },
    operation: operation(), registry: { revision: 1, recordKind: 'registry', registryVersion: 1,
      profiles: [{ sourceProfileId: HASH, canonicalAppVersion: '1.0.0', state: 'selectable', targetKey: TARGET, forwardEdges: [] }] },
    deny: { revision: 1, recordKind: 'deny', entries: [] },
    'contribution-head': { revision: 1, watermark: 0, windowGroups: {} },
  };
  const definition = { minimumSamples: 10, threshold: { numerator: 1, denominator: 10 } };
  const stage = (stageId, percentage) => ({ stageId, percentage, minimumObservationMs: 1000, requiredMetrics: ['download', 'verification', 'installation', 'health'],
    metrics: Object.fromEntries(['download', 'verification', 'installation', 'health'].map((metric) => [metric, definition])) });
  const body = { upgradeType: 'normal', stages: [stage('first', 10), stage('final', 100)], snIncludeSetRef: null, snExcludeSetRef: null,
    progressReportIntervalMs: 30_000, lossIntervalMs: 120_000 };
  records.plan = { revision: 1, body, approvalKey: 'plan-approval' };
  records['plan-approval'] = { revision: 1, projectionOwner: 'T05', state: 'approved', bodySha256: objectHash(body),
    authorId: 'author', reviewerIds: ['reviewer-a', 'reviewer-b'], context: approvalContext('FreezeRolloutPlan', records.scope, 'plan', 1) };
  records.rollout = { revision: 1, recordKind: 'rollout', state: 'draft', rolloutId: 'rollout-3', deploymentKey: 'candidate', scopeKey: 'scope',
    planKey: 'plan', planRevision: 1, planSha256: objectHash(body), stageIndex: null, percentage: 0, stageFactKey: null,
    qualityFrozen: false, targetIdentitySha256: HASH };
  records.inventory = { revision: 1, product: 'pinvou', component: 'app', scopeKeys: ['scope'],
    requiredEntityKeys: ['baseline', 'candidate', 'rollout', 'verification-archive', 'root-lineage-1', 'root-lineage-1-material', 'trust-anchor',
      'release-2', 'release-3', 'target-2', 'target-3', 'artifact-2', 'artifact-3', 'supply-2', 'supply-3', 'registry', 'deny'] };
  initialLineagePort(records);
  return { set, records, candidatePackageEnvelope, candidateReleaseEnvelope };
}

export function refreshPaths(records, { deploymentKey = 'candidate', pathKey = 'path', anticipatedRevision = null, now = NOW } = {}) {
  const deployment = records[deploymentKey]; const target = records[deployment.releaseTargetKey];
  const certificateKey = `${deploymentKey}-certificate`;
  const targetSourceProfileId = objectHash({ packageEnvelopeSha256: objectHash(target.packageEnvelope), version: deployment.appVersion });
  if (!records.registry.profiles.some((profile) => profile.sourceProfileId === targetSourceProfileId)) {
    records.registry.profiles.push({ sourceProfileId: targetSourceProfileId, canonicalAppVersion: deployment.appVersion,
      state: 'selectable', targetKey: TARGET, forwardEdges: [] });
  }
  records[certificateKey] = { revision: 1, recordKind: 'certification', projectionOwner: 'T46', state: 'passed', product: 'pinvou', component: 'app',
    targetKey: TARGET, packageEnvelopeSha256: objectHash(target.packageEnvelope), sourceProfileId: HASH,
    targetSourceProfileId,
    activationMode: deployment.activationMode, upgradeTypes: ['normal', 'forced'] };
  const sources = records.registry.profiles.filter((profile) => profile.state === 'selectable' && profile.targetKey === TARGET
    && compareVersions(profile.canonicalAppVersion, deployment.appVersion) < 0).map((profile) => {
    const sourceCertificateKey = `${certificateKey}-${profile.sourceProfileId}`;
    records[sourceCertificateKey] = { ...records[certificateKey], sourceProfileId: profile.sourceProfileId };
    return { sourceProfileId: profile.sourceProfileId, currentVersion: profile.canonicalAppVersion, actualHopIndex: 0,
      groupIdentity: profile.sourceProfileId, steps: [{ kind: 'ordinary', chain: deployment.chain,
        fromSourceProfileId: profile.sourceProfileId, toSourceProfileId: targetSourceProfileId, activationMode: deployment.activationMode,
        certificationKey: sourceCertificateKey, ordinaryApprovalKey: null, transformKey: null }] };
  });
  const keys = [...new Set(['root', 'head', 'deny', 'scope', 'registry', ...(records.scope.baselineDeploymentKey === null ? [] : [records.scope.baselineDeploymentKey]),
    deploymentKey, deployment.releaseKey, deployment.releaseTargetKey, deployment.chain.supplyChainKey, ...deployment.chain.artifactKeys,
    ...sources.map((source) => source.steps[0].certificationKey)])];
  const future = anticipatedRevision === null ? records : { ...records, [deploymentKey]: { ...deployment, revision: anticipatedRevision, state: 'scheduled' } };
  records[pathKey] = { revision: (records[pathKey]?.revision ?? 0) + 1, projectionOwner: 'T13', state: 'approved',
    deploymentKey, deploymentRevision: anticipatedRevision ?? deployment.revision, scopeKey: 'scope', rootKey: 'root', headKey: 'head', denyKey: 'deny', registryKey: 'registry',
    projectedWritesSha256: null,
    qualifiedAt: now, expiresAt: now + DAY,
    sources,
    readSet: keys.map((key) => ({ ...captureRecord(future, key), recordKind: future[key].recordKind })) };
  return pathKey;
}

export function proposedPublication(fixture, records, { now = NOW + 1, opening, mode = 'selection', complete = false } = {}) {
  initialLineagePort(records);
  opening ??= mode === 'refresh' ? records.scope.currentCandidateOpening ?? null : null;
  const current = records.head.published ? records.head.bundle : null;
  const target = structuredClone(current?.members.find((member) => member.signed.role === 'target').signed ?? fixture.set.metadata.target);
  target.version = current === null ? 1 : target.version + 1; target.issuedAt = now; target.expiresAt = now + DAY;
  target.selectionGeneration = records.scope.selectionGeneration + (mode === 'selection' ? 1 : 0);
  target.rolloutSetCommitment = rolloutCommitment(opening);
  if (complete) target.baselineRelease = envelopeReference(canonicalize(fixture.candidateReleaseEnvelope));
  const targetEnvelope = parseJson(fixture.set.sign(target));
  const members = [targetEnvelope, complete ? fixture.candidateReleaseEnvelope : records['release-2'].envelope,
    complete ? fixture.candidatePackageEnvelope : parseJson(fixture.set.bytes.package)];
  const snapshot = { ...structuredClone(fixture.set.metadata.snapshot), version: current === null ? 1 : current.snapshot.signed.version + 1,
    issuedAt: now, expiresAt: now + DAY, entries: members.map((member) => envelopeReference(canonicalize(member))) };
  const snapshotEnvelope = parseJson(fixture.set.sign(snapshot));
  const timestamp = { ...structuredClone(fixture.set.metadata.timestamp), version: current === null ? 1 : current.timestamp.signed.version + 1,
    issuedAt: now, expiresAt: now + DAY, snapshot: envelopeReference(canonicalize(snapshotEnvelope)) };
  const lineage = records[records.root.rootLineageKey];
  const readKeys = [...new Set(['root', 'head', 'inventory', records.root.rootLineageKey, lineage.anchorKey, lineage.chainMaterialKey,
    ...records.inventory.scopeKeys, ...records.inventory.requiredEntityKeys])];
  return { rootKey: 'root', headKey: 'head', denyKey: 'deny', inventoryKey: 'inventory', mode, affectedScopeKeys: mode === 'refresh' ? [] : ['scope'],
    baseRoot: captureRecord(records, 'root'), baseHead: captureRecord(records, 'head'), readSet: readKeys.map((key) => captureRecord(records, key)),
    candidateOpenings: opening === null ? [] : [{ scopeKey: 'scope', opening }],
    bundle: { timestamp: parseJson(fixture.set.sign(timestamp)), snapshot: snapshotEnvelope, members } };
}
export function openingFor(records, action) {
  if (['complete', 'abort'].includes(action)) return null;
  return { targetKey: TARGET, deploymentId: records.candidate.deploymentId,
    deploymentRevision: records.candidate.revision + (['pause', 'resume'].includes(action) ? 1 : 0),
    rolloutId: records.rollout.rolloutId, rolloutRevision: records.rollout.revision + 1,
    releaseEnvelopeSha256: records['release-3'].envelopeSha256, leafSalt: Buffer.alloc(32, records.rollout.revision).toString('hex') };
}
export function rolloutCommand(fixture, records, action, now = NOW + 1) {
  if (['start', 'advance', 'complete', 'resume'].includes(action)) refreshPaths(records, { now });
  const opening = openingFor(records, action);
  const resume = action === 'resume' ? prepareResume(records, 'candidate') : {};
  const publication = proposedPublication(fixture, records, { opening, now, complete: action === 'complete' });
  if (action === 'complete') {
    refreshPaths(records, { pathKey: 'future-path', now });
    // The read-only T13 fixture certifies the proposed complete view separately.
    const parts = publicationParts(records, publication, now);
    const scope = parts.writes.find((write) => write.key === 'scope').value;
    Object.assign(scope, { baselineDeploymentKey: 'candidate', runningRolloutKey: null, hasCandidate: false, currentCandidateOpening: null });
    const view = { scope, head: parts.writes.find((write) => write.key === 'head').value,
      baseline: nextRecord(records.baseline, { state: 'superseded' }) };
    records['future-path'].projectedWritesSha256 = objectHash(view);
  }
  return { ...operationCommand, ...resume, reason: 'pause-investigation', rolloutKey: 'rollout', expectedRevision: records.rollout.revision, action, actor: 'system', opening, pathKey: 'path',
    futurePathKey: 'future-path',
    qualityKey: 'quality', freezeIndexKey: 'freeze-index', newStageFactKey: 'stage-fact', windowId: `window-${records.rollout.revision + 1}`,
    publication };
}
export function prepareResume(records, deploymentKey) {
  const deployment = records[deploymentKey];
  records['pause-resolution'] = { revision: 1, projectionOwner: 'T14', state: 'resolved', reason: 'verified-resolution',
    deploymentKey, deploymentRevision: deployment.revision, pauseReasonSha256: objectHash(deployment.pauseReason) };
  records['resume-approval'] = { revision: 1, projectionOwner: 'T05', state: 'approved', authorId: 'author', reviewerIds: ['reviewer-a', 'reviewer-b'],
    context: approvalContext('ResumeDeployment', records.scope, deploymentKey, deployment.revision),
    bodySha256: objectHash({ command: 'ResumeDeployment', deploymentKey, deploymentRevision: deployment.revision,
      pauseReasonSha256: objectHash(deployment.pauseReason), resolutionSha256: objectHash(records['pause-resolution']) }) };
  return { pauseResolutionKey: 'pause-resolution', resumeApprovalKey: 'resume-approval' };
}
export function catchUpQuality(records) {
  const fact = records['stage-fact']; const stage = records.plan.body.stages[records.rollout.stageIndex];
  records['freeze-index'] = { revision: records['freeze-index']?.revision ?? 1, reasonKeys: [] };
  records['contribution-head'].windowGroups[fact.windowId] = [HASH];
  records.quality = { revision: records.quality?.revision ?? 1, rolloutKey: 'rollout', planSha256: records.rollout.planSha256,
    stageId: stage.stageId, windowId: fact.windowId, stageFactRevision: fact.revision, sourceHeadKey: 'contribution-head',
    sourceWatermark: records['contribution-head'].watermark, processedWatermark: records['contribution-head'].watermark,
    freezeIndexRevision: records['freeze-index'].revision,
    observation: { startedAt: fact.effectiveAt, groups: [{ groupIdentity: HASH,
      metrics: Object.fromEntries(stage.requiredMetrics.map((metric) => [metric, { succeeded: 10, failed: 0, incomplete: 0, unknown: 0 }])) }] } };
}
