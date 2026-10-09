import test from 'node:test';
import assert from 'node:assert/strict';
import { pathReads } from '../models/paths.mjs';
import { captureRecord, nextRecord, applyAtomically } from '../models/atomic.mjs';
import { objectHash } from '../models/inputs.mjs';
import { releaseBusinessHash } from '../models/metadata-inputs.mjs';
import { canonicalize, parseJson } from '../canonical-json.mjs';
import { envelopeReference } from '../signatures.mjs';
import { publicationFixture, refreshPaths, rolloutCommand, catchUpQuality, proposedPublication } from './publication-fixtures.mjs';
import { planRolloutCommand } from '../models/rollout.mjs';
import { publicationParts, planPublish } from '../models/publication.mjs';
import { bridgeUnitParts } from '../models/bridge-unit.mjs';
import { approvalContext } from '../models/entities.mjs';
import { operation, operationCommand } from './model-fixtures.mjs';
import { DAY } from '../semantics.mjs';
import { NOW, HASH } from './fixtures.mjs';

function replacePublicTarget(fixture, target) {
  const { records, set } = fixture;
  const signedTarget = parseJson(set.sign(target));
  const requiredReleaseIds = new Set([target.baselineRelease, ...target.ordinaryPaths.map((path) => path.release),
    ...target.bridgePaths.map((path) => path.release)].map((ref) => ref.scope.releaseId));
  const releases = Object.values(records.head.releaseEnvelopes).filter((item) => requiredReleaseIds.has(item.signed.scope.releaseId));
  const packageHashes = new Set(releases.flatMap((item) => item.signed.targets.map((entry) => entry.packageManifest.envelopeSha256)));
  const packages = Object.values(records).filter((item) => item.recordKind === 'releaseTarget' && packageHashes.has(objectHash(item.packageEnvelope)))
    .map((item) => item.packageEnvelope);
  const members = [signedTarget, ...releases, ...new Map(packages.map((item) => [objectHash(item), item])).values()];
  const snapshot = { ...structuredClone(set.metadata.snapshot), version: records.head.bundle.snapshot.signed.version + 1,
    entries: members.map((item) => envelopeReference(canonicalize(item))) };
  const signedSnapshot = parseJson(set.sign(snapshot));
  const timestamp = { ...structuredClone(set.metadata.timestamp), version: records.head.bundle.timestamp.signed.version + 1,
    snapshot: envelopeReference(canonicalize(signedSnapshot)) };
  records.head = nextRecord(records.head, { bundle: { timestamp: parseJson(set.sign(timestamp)), snapshot: signedSnapshot, members } });
}
function refreshReadSet(records, pathKey, extra = []) {
  const path = records[pathKey];
  const keys = [...new Set([...path.readSet.map((read) => read.key), ...extra])];
  path.readSet = keys.map((key) => ({ ...captureRecord(records, key), recordKind: records[key].recordKind }));
}
function twoHopFixture(bridge = false) {
  const fixture = publicationFixture(); const { records, set } = fixture;
  const pkg = structuredClone(set.metadata.package); pkg.appVersion = '2.5.0'; pkg.fullPackage.packageId = 'package-middle';
  const signedPackage = parseJson(set.sign(pkg));
  const release = structuredClone(set.metadata.release); release.scope.releaseId = 'release-middle'; release.appVersion = '2.5.0';
  release.targets[0].releaseTargetId = 'target-middle'; release.targets[0].packageManifest = envelopeReference(canonicalize(signedPackage));
  const signedRelease = parseJson(set.sign(release)); records.head.releaseEnvelopes['release-middle'] = signedRelease;
  records['release-middle'] = { revision: 1, recordKind: 'release', state: 'closed', releaseId: 'release-middle', appVersion: '2.5.0',
    metadataHeadKey: 'head', frozenBusinessSha256: releaseBusinessHash(release) };
  records['target-middle'] = { revision: 1, recordKind: 'releaseTarget', state: 'approved', releaseKey: 'release-middle',
    releaseTargetId: 'target-middle', packageEnvelope: signedPackage, artifactKeys: ['artifact-middle'] };
  records['artifact-middle'] = { revision: 1, recordKind: 'artifact', state: 'valid' };
  records['supply-middle'] = { revision: 1, recordKind: 'supplyChain', state: 'approved', channel: 'stable', releaseTargetKey: 'target-middle', effectiveExpiresAt: null };
  const chain = { rootKey: 'root', denyKey: 'deny', deploymentKey: 'middle', releaseKey: 'release-middle', releaseTargetKey: 'target-middle',
    artifactKeys: ['artifact-middle'], supplyChainKey: 'supply-middle', ...(bridge ? { bridgeKey: 'eligibility' } : {}) };
  records.middle = { ...structuredClone(records.baseline), deploymentId: 'deployment-middle', appVersion: '2.5.0',
    releaseKey: 'release-middle', releaseTargetKey: 'target-middle', state: bridge ? 'superseded' : 'active', chain };
  if (bridge) records.eligibility = { revision: 1, recordKind: 'bridgeEligibility', state: 'enabled', eligibilityId: 'bridge-middle',
    deploymentKey: 'middle', deploymentRevision: 1, channel: 'stable', validFrom: NOW, expiresAt: null };
  records['ordinary-approval'] = { revision: 1, recordKind: 'ordinaryPathApproval', state: 'approved', approvalId: 'approval-middle',
    deploymentKey: 'middle', deploymentRevision: 1, channel: 'stable' };
  const candidate = structuredClone(records.head.releaseEnvelopes['release-3']); candidate.signed.targets[0].minimumSourceVersion = '2.5.0';
  records.head.releaseEnvelopes['release-3'] = parseJson(set.sign(candidate.signed));
  records['release-3'].frozenBusinessSha256 = releaseBusinessHash(candidate.signed);
  refreshPaths(records, { deploymentKey: 'middle', pathKey: 'middle-path' });
  refreshPaths(records, { pathKey: 'path' });
  const first = records['middle-path'].sources[0].steps[0]; first.kind = bridge ? 'bridge' : 'ordinary';
  first.ordinaryApprovalKey = bridge ? null : 'ordinary-approval';
  records.path.sources.find((source) => source.sourceProfileId === HASH).steps.unshift(first);
  const second = records.path.sources.find((source) => source.sourceProfileId !== HASH);
  records.path.sources[0].steps[1] = structuredClone(second.steps[0]);
  refreshReadSet(records, 'path', [...records['middle-path'].readSet.map((read) => read.key), bridge ? 'eligibility' : 'ordinary-approval']);
  const target = structuredClone(set.metadata.target);
  if (bridge) target.bridgePaths = [{ deploymentId: 'deployment-middle', deploymentRevision: 1, deploymentState: 'superseded',
    eligibilityId: 'bridge-middle', eligibilityRevision: 1, release: envelopeReference(canonicalize(signedRelease)) }];
  else target.ordinaryPaths = [{ deploymentId: 'deployment-middle', deploymentRevision: 1, approvalId: 'approval-middle', approvalRevision: 1,
    release: envelopeReference(canonicalize(signedRelease)) }];
  records.inventory.requiredEntityKeys.push('middle', 'release-middle', 'target-middle', 'artifact-middle', 'supply-middle',
    bridge ? 'eligibility' : 'ordinary-approval');
  return { ...fixture, target };
}

test('multi-hop ordinary and bridge paths require exact public authorization and cannot splice same-version facts', () => {
  for (const bridge of [false, true]) {
    const fixture = twoHopFixture(bridge); const { records } = fixture;
    assert.throws(() => pathReads(records, 'path', 'candidate', 'scope', NOW + 1), { code: 'MODEL_PATH_INVALID' });
    replacePublicTarget(fixture, { ...fixture.target, version: 2 }); refreshReadSet(records, 'path');
    const reads = pathReads(records, 'path', 'candidate', 'scope', NOW + 1);
    assert(reads.includes(bridge ? 'eligibility' : 'ordinary-approval'));
    for (const alter of [(copy) => { copy.path.sources[0].steps[1].fromSourceProfileId = HASH; },
      (copy) => { copy.path.readSet = copy.path.readSet.filter((read) => read.key !== 'artifact-middle'); },
      (copy) => { copy.deny.entries.push({ subjectKind: 'artifact', subjectId: 'artifact-middle', roles: [] }); refreshReadSet(copy, 'path'); },
      (copy) => { copy.scope.metadataSyncPending = false; copy.deny.entries.push({ subjectKind: 'deployment', subjectId: 'deployment-middle', roles: [] }); refreshReadSet(copy, 'path'); }]) {
      const changed = structuredClone(records); alter(changed); assert.throws(() => pathReads(changed, 'path', 'candidate', 'scope', NOW + 1));
    }
  }
});
test('completed baseline switching rejects a future ordinary old baseline and jointly commits an exact approved bridge', () => {
  const fixture = publicationFixture(); let records = fixture.records;
  const candidate = structuredClone(records.head.releaseEnvelopes['release-3']); candidate.signed.targets[0].minimumSourceVersion = '2.0.0';
  fixture.candidateReleaseEnvelope = parseJson(fixture.set.sign(candidate.signed));
  records.head.releaseEnvelopes['release-3'] = fixture.candidateReleaseEnvelope;
  records['release-3'].envelope = fixture.candidateReleaseEnvelope; records['release-3'].envelopeSha256 = objectHash(fixture.candidateReleaseEnvelope);
  records['release-3'].frozenBusinessSha256 = releaseBusinessHash(candidate.signed);
  refreshPaths(records, { deploymentKey: 'baseline', pathKey: 'baseline-path' });
  function connectThroughBaseline(snapshot, key, bridge = false) {
    const path = snapshot[key]; const source = path.sources.find((item) => item.sourceProfileId === HASH);
    const next = path.sources.find((item) => item.sourceProfileId !== HASH);
    const first = structuredClone(snapshot['baseline-path'].sources[0].steps[0]);
    if (bridge) { first.kind = 'bridge'; first.chain.bridgeKey = 'old-bridge'; }
    source.steps = [first, structuredClone(next.steps[0])];
    refreshReadSet(snapshot, key, [...snapshot['baseline-path'].readSet.map((read) => read.key), ...(bridge ? ['old-bridge'] : [])]);
  }
  for (const [action, now] of [['start', NOW + 1], ['advance', NOW + 1001]]) {
    records.operation = operation(now);
    const command = rolloutCommand(fixture, records, action, now); connectThroughBaseline(records, 'path');
    records = applyAtomically(records, planRolloutCommand(records, command, now), now).records;
    catchUpQuality(records);
  }
  records.operation = operation(NOW + 2001);
  const complete = rolloutCommand(fixture, records, 'complete', NOW + 2001);
  connectThroughBaseline(records, 'path'); connectThroughBaseline(records, 'future-path');
  assert.throws(() => planRolloutCommand(records, complete, NOW + 2001));
  records['old-bridge'] = { revision: 1, recordKind: 'bridgeEligibility', state: 'disabled', eligibilityId: 'bridge-old',
    deploymentKey: 'baseline', deploymentRevision: 1, channel: 'stable', validFrom: NOW, expiresAt: null };
  records['bridge-unit'] = { revision: 1, projectionOwner: 'T13', state: 'prepared', deploymentKey: 'baseline', expectedDeploymentRevision: 1,
    eligibilityKey: 'old-bridge', expectedEligibilityRevision: 1, validFrom: NOW, expiresAt: null, approvalKey: 'bridge-approval' };
  records['bridge-approval'] = { revision: 1, projectionOwner: 'T05', state: 'approved', authorId: 'author', reviewerIds: ['one', 'two'],
    context: approvalContext('EnableBridge', records.scope, 'bridge-old', 1), bodySha256: objectHash({ command: 'EnableBridge',
      deploymentId: records.baseline.deploymentId, resultingDeploymentRevision: 2, eligibilityId: 'bridge-old', resultingEligibilityRevision: 2,
      validFrom: NOW, expiresAt: null }) };
  const proposedTarget = complete.publication.bundle.members.find((item) => item.signed.role === 'target').signed;
  proposedTarget.bridgePaths = [{ deploymentId: records.baseline.deploymentId, deploymentRevision: 2, deploymentState: 'superseded',
    eligibilityId: 'bridge-old', eligibilityRevision: 2, release: envelopeReference(canonicalize(records.head.releaseEnvelopes['release-2'])) }];
  const temporary = { ...fixture, records: structuredClone(records) };
  replacePublicTarget(temporary, proposedTarget); complete.publication.bundle = temporary.records.head.bundle;
  complete.bridgeUnitKey = 'bridge-unit'; connectThroughBaseline(records, 'future-path', true);
  const parts = publicationParts(records, complete.publication, NOW + 2001);
  const scope = parts.writes.find((write) => write.key === 'scope').value;
  Object.assign(scope, { baselineDeploymentKey: 'candidate', runningRolloutKey: null, hasCandidate: false, currentCandidateOpening: null });
  const bridge = bridgeUnitParts(records, 'bridge-unit', records.scope, 'baseline', NOW + 2001);
  records['future-path'].projectedWritesSha256 = objectHash({ scope, head: parts.writes.find((write) => write.key === 'head').value,
    baseline: nextRecord(records.baseline, { state: 'superseded' }), 'old-bridge': bridge.writes[0].value });
  const plan = planRolloutCommand(records, complete, NOW + 2001);
  const changed = structuredClone(records); changed['bridge-approval'].reviewerIds.pop();
  assert.throws(() => planRolloutCommand(changed, complete, NOW + 2001), { code: 'MODEL_APPROVAL_INVALID' });
  assert.throws(() => applyAtomically(changed, plan, NOW + 2001), { code: 'MODEL_CAS_CONFLICT' });
  const result = applyAtomically(records, plan, NOW + 2001).records;
  assert.equal(result.baseline.state, 'superseded'); assert.equal(result['old-bridge'].state, 'enabled');
  assert.equal(result['old-bridge'].deploymentRevision, 2); assert.equal(result.scope.baselineDeploymentKey, 'candidate');
});

test('an empty complete source set is legal and forced paths cover the whole endpoint installation window', () => {
  const fixture = publicationFixture(); const records = fixture.records;
  records.scope.supportFloorVersion = '3.0.0';
  const target = { ...structuredClone(fixture.set.metadata.target), version: 2, supportFloorVersion: '3.0.0' };
  replacePublicTarget(fixture, target); refreshPaths(records); records.path.sources = []; refreshReadSet(records, 'path');
  assert(pathReads(records, 'path', 'candidate', 'scope', NOW + 1).includes('registry'));
  const forced = twoHopFixture(); forced.records.candidate.upgradeType = 'forced'; forced.records.candidate.installNotBefore = NOW + 100;
  forced.records.candidate.installNotAfter = NOW + DAY; forced.records.middle.installNotAfter = NOW + DAY - 1;
  replacePublicTarget(forced, { ...forced.target, version: 2 }); refreshReadSet(forced.records, 'path');
  assert.throws(() => pathReads(forced.records, 'path', 'candidate', 'scope', NOW + 1), { code: 'MODEL_FORCED_PATH_INVALID' });
});
test('metadata renewal refreshes baseline, ordinary and bridge full references without changing selection', () => {
  for (const bridge of [false, true]) {
    const fixture = twoHopFixture(bridge); const { records, set } = fixture;
    replacePublicTarget(fixture, { ...fixture.target, version: 2 }); const now = NOW + 91 * DAY; records.operation = operation(now);
    const renewal = new Map();
    for (const envelope of records.head.bundle.members.filter((item) => item.signed.role === 'release')) {
      const signed = { ...structuredClone(envelope.signed), revision: 2, issuedAt: now, expiresAt: now + 90 * DAY };
      renewal.set(signed.scope.releaseId, parseJson(set.sign(signed)));
    }
    const reference = (old) => envelopeReference(canonicalize(renewal.get(old.scope.releaseId)));
    const target = { ...structuredClone(fixture.target), version: 3, issuedAt: now, expiresAt: now + 30 * DAY };
    target.baselineRelease = reference(target.baselineRelease);
    for (const path of [...target.ordinaryPaths, ...target.bridgePaths]) path.release = reference(path.release);
    const temporary = { ...fixture, records: structuredClone(records) };
    Object.assign(temporary.records.head.releaseEnvelopes, Object.fromEntries(renewal));
    replacePublicTarget(temporary, target);
    temporary.records.head.bundle.snapshot.signed.issuedAt = now; temporary.records.head.bundle.snapshot.signed.expiresAt = now + 7 * DAY;
    temporary.records.head.bundle.snapshot = parseJson(set.sign(temporary.records.head.bundle.snapshot.signed));
    temporary.records.head.bundle.timestamp.signed.issuedAt = now; temporary.records.head.bundle.timestamp.signed.expiresAt = now + DAY;
    temporary.records.head.bundle.timestamp.signed.snapshot = envelopeReference(canonicalize(temporary.records.head.bundle.snapshot));
    temporary.records.head.bundle.timestamp = parseJson(set.sign(temporary.records.head.bundle.timestamp.signed));
    const publication = proposedPublication(fixture, records, { now, mode: 'refresh' }); publication.bundle = temporary.records.head.bundle;
    const result = applyAtomically(records, planPublish(records, { ...operationCommand, publication }, now), now).records;
    assert.equal(result.scope.selectionGeneration, 1); assert.equal(result['release-middle'].revision, 1);
    assert.equal(result.head.releaseEnvelopes['release-middle'].signed.revision, 2);
    assert.equal(result.head.bundle.members.find((item) => item.signed.role === 'target').signed[bridge ? 'bridgePaths' : 'ordinaryPaths'][0].release.version, 2);
  }
});
