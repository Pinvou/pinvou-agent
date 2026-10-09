import test from 'node:test';
import assert from 'node:assert/strict';
import { applyAtomically } from '../models/atomic.mjs';
import { planRolloutCommand } from '../models/rollout.mjs';
import { planBaselineCommand } from '../models/deployment.mjs';
import { planEmergencyDeny, planReconcileMetadata } from '../models/publication.mjs';
import { canonicalize, parseJson } from '../canonical-json.mjs';
import { envelopeReference } from '../signatures.mjs';
import { rolloutCommitment } from '../digests.mjs';
import { objectHash } from '../models/inputs.mjs';
import { publicationFixture, proposedPublication, rolloutCommand, initialLineagePort } from './publication-fixtures.mjs';
import { operation, operationCommand } from './model-fixtures.mjs';
import { NOW } from './fixtures.mjs';
import { DAY } from '../semantics.mjs';

function resign(fixture, proposal) {
  const members = proposal.bundle.members.map((envelope) => envelope.signed.role === 'target'
    ? parseJson(fixture.set.sign(envelope.signed)) : envelope);
  const snapshot = { ...proposal.bundle.snapshot.signed, entries: members.map((item) => envelopeReference(canonicalize(item))) };
  const signedSnapshot = parseJson(fixture.set.sign(snapshot));
  const timestamp = { ...proposal.bundle.timestamp.signed, snapshot: envelopeReference(canonicalize(signedSnapshot)) };
  proposal.bundle = { members, snapshot: signedSnapshot, timestamp: parseJson(fixture.set.sign(timestamp)) };
}
function activeFixture({ beta = false } = {}) {
  const fixture = publicationFixture();
  fixture.records = applyAtomically(fixture.records,
    planRolloutCommand(fixture.records, rolloutCommand(fixture, fixture.records, 'start'), NOW + 1), NOW + 1).records;
  if (beta) {
    const records = fixture.records;
    records.root.body.roles.push({ ...records.root.body.roles.find((role) => role.role === 'target'), channel: 'beta' });
    initialLineagePort(records);
    records.betaBaseline = { ...structuredClone(records.baseline), channel: 'beta', deploymentId: 'beta-baseline' };
    records.betaCandidate = { ...structuredClone(records.candidate), channel: 'beta', deploymentId: 'beta-candidate', rolloutKey: 'betaRollout' };
    records.betaRollout = { ...structuredClone(records.rollout), rolloutId: 'beta-rollout', scopeKey: 'betaScope', deploymentKey: 'betaCandidate' };
    const opening = { ...records.scope.currentCandidateOpening, deploymentId: 'beta-candidate', rolloutId: 'beta-rollout' };
    records.betaScope = { ...structuredClone(records.scope), channel: 'beta', baselineDeploymentKey: 'betaBaseline',
      runningRolloutKey: 'betaRollout', rolloutKeys: ['betaRollout'], currentCandidateOpening: opening };
    records.inventory.scopeKeys.push('betaScope'); records.inventory.requiredEntityKeys.push('betaBaseline', 'betaCandidate', 'betaRollout');
    const proposal = { bundle: structuredClone(records.head.bundle) };
    const target = { ...structuredClone(proposal.bundle.members.find((item) => item.signed.role === 'target').signed),
      scope: { channel: 'beta', targetKey: records.scope.targetKey }, rolloutSetCommitment: rolloutCommitment(opening) };
    proposal.bundle.members.push(parseJson(fixture.set.sign(target))); resign(fixture, proposal); records.head.bundle = proposal.bundle;
  }
  return fixture;
}
function appendBeta(fixture, records, proposal, opening, { affected = false } = {}) {
  const current = records.head.bundle.members.find((item) => item.signed.role === 'target' && item.signed.scope.channel === 'beta').signed;
  const target = { ...structuredClone(current), version: current.version + 1, issuedAt: proposal.bundle.timestamp.signed.issuedAt,
    selectionGeneration: records.betaScope.selectionGeneration + (proposal.mode === 'selection' && affected ? 1 : 0),
    rolloutSetCommitment: rolloutCommitment(opening) };
  proposal.bundle.members.push(parseJson(fixture.set.sign(target)));
  proposal.candidateOpenings.push({ scopeKey: 'betaScope', opening });
  if (affected) proposal.affectedScopeKeys.push('betaScope');
  resign(fixture, proposal);
}
function deniedRecords(fixture, { beta = false } = {}) {
  const records = fixture.records; records.operation = operation(NOW + 2);
  const affectedScopeKeys = beta ? ['scope', 'betaScope'] : ['scope'];
  records.denyInventory = { revision: 1, product: 'pinvou', component: 'app', affectedScopeKeys };
  const command = { ...operationCommand, denyKey: 'deny', inventoryKey: 'denyInventory', jobKey: 'job', affectedScopeKeys,
    denyEntries: [{ subjectKind: 'artifact', subjectId: 'unrelated-artifact', roles: [] }] };
  const result = applyAtomically(records, planEmergencyDeny(records, command, NOW + 2), NOW + 2).records;
  result.operation = operation(NOW + 3); return result;
}
function reconcileCommand(fixture, records, { beta = false } = {}) {
  const opening = { ...records.scope.currentCandidateOpening, leafSalt: '99'.repeat(32) };
  const publication = proposedPublication(fixture, records, { mode: 'reconcile', now: NOW + 3, opening });
  if (beta) appendBeta(fixture, records, publication, { ...records.betaScope.currentCandidateOpening, leafSalt: '98'.repeat(32) }, { affected: true });
  return { ...operationCommand, denyKey: 'deny', jobKey: 'job', expectedJobRevision: records.job.revision, publication };
}

test('legally re-signed pause metadata cannot change support floor, host policy or unrelated paths', () => {
  const fixture = activeFixture(); const records = fixture.records; records.operation = operation(NOW + 2);
  const command = rolloutCommand(fixture, records, 'pause', NOW + 2);
  for (const mutate of [(target) => { target.supportFloorVersion = '2.0.0'; },
    (target) => { target.minimumOsVersion = '99.0'; },
    (target) => { target.ordinaryPaths.push({ deploymentId: 'unknown', deploymentRevision: 1, approvalId: 'unknown',
      approvalRevision: 1, release: structuredClone(target.baselineRelease) }); }]) {
    const copy = structuredClone(command); mutate(copy.publication.bundle.members.find((item) => item.signed.role === 'target').signed);
    resign(fixture, copy.publication); assert.throws(() => planRolloutCommand(records, copy, NOW + 2));
  }
  assert.equal(applyAtomically(records, planRolloutCommand(records, command, NOW + 2), NOW + 2).records.rollout.state, 'paused');
});

test('single-scope Rollout and baseline commands reject an otherwise valid second affected scope', () => {
  const fixture = activeFixture({ beta: true }); const records = fixture.records; records.operation = operation(NOW + 2);
  const command = rolloutCommand(fixture, records, 'pause', NOW + 2);
  appendBeta(fixture, records, command.publication, { ...records.betaScope.currentCandidateOpening, leafSalt: '97'.repeat(32) }, { affected: true });
  assert.throws(() => planRolloutCommand(records, command, NOW + 2), { code: 'MODEL_PUBLICATION_REQUIRED' });
  const baseline = { ...operationCommand, action: 'pause', reason: 'maintenance', expectedRevision: records.baseline.revision,
    deploymentKey: 'baseline', scopeKey: 'scope', opening: command.opening, publication: command.publication };
  assert.throws(() => planBaselineCommand(records, baseline, NOW + 2), { code: 'MODEL_PUBLICATION_REQUIRED' });
  assert.equal(records.betaScope.selectionGeneration, 2); assert.equal(records.betaBaseline.state, 'active');
});

test('candidate deny reconciliation atomically saves every opening and rejects stale fanout', () => {
  for (const beta of [false, true]) {
    const fixture = activeFixture({ beta }); const records = deniedRecords(fixture, { beta });
    const command = reconcileCommand(fixture, records, { beta }); const plan = planReconcileMetadata(records, command, NOW + 3);
    const result = applyAtomically(records, plan, NOW + 3).records;
    for (const scopeKey of beta ? ['scope', 'betaScope'] : ['scope']) {
      const scope = result[scopeKey]; const target = result.head.bundle.members.find((item) => item.signed.role === 'target'
        && item.signed.scope.channel === scope.channel).signed;
      assert.equal(scope.metadataSyncPending, false); assert.equal(scope.selectionGeneration, records[scopeKey].selectionGeneration);
      assert.equal(target.rolloutSetCommitment, rolloutCommitment(scope.currentCandidateOpening));
      assert.notEqual(scope.currentCandidateOpening.leafSalt, records[scopeKey].currentCandidateOpening.leafSalt);
      const raced = structuredClone(records); raced[scopeKey].revision++;
      assert.throws(() => applyAtomically(raced, plan, NOW + 3), { code: 'MODEL_CAS_CONFLICT' });
    }
    assert.equal(result.job.state, 'completed');
    if (!beta) {
      result.operation = operation(NOW + 4);
      const next = rolloutCommand(fixture, result, 'pause', NOW + 4);
      assert.equal(applyAtomically(result, planRolloutCommand(result, next, NOW + 4), NOW + 4).records.rollout.state, 'paused');
    }
  }
});

test('reconciliation cannot change floor, baseline, candidate revisions or introduce a path', () => {
  const fixture = activeFixture(); const records = deniedRecords(fixture); const command = reconcileCommand(fixture, records);
  for (const mutate of [(copy) => { copy.publication.bundle.members[0].signed.supportFloorVersion = '2.0.0'; },
    (copy) => { copy.publication.candidateOpenings[0].opening.rolloutRevision++; },
    (copy) => { const target = copy.publication.bundle.members[0].signed; target.ordinaryPaths.push({ deploymentId: 'unknown',
      deploymentRevision: 1, approvalId: 'unknown', approvalRevision: 1, release: structuredClone(target.baselineRelease) }); }]) {
    const copy = structuredClone(command); mutate(copy);
    copy.publication.bundle.members[0].signed.rolloutSetCommitment = rolloutCommitment(copy.publication.candidateOpenings[0].opening);
    resign(fixture, copy.publication); assert.throws(() => planReconcileMetadata(records, copy, NOW + 3));
  }
  const changedBaseline = structuredClone(command);
  changedBaseline.publication.bundle.members[0].signed.baselineRelease = envelopeReference(canonicalize(fixture.candidateReleaseEnvelope));
  changedBaseline.publication.bundle.members = [changedBaseline.publication.bundle.members[0],
    fixture.candidateReleaseEnvelope, fixture.candidatePackageEnvelope];
  resign(fixture, changedBaseline.publication);
  assert.throws(() => planReconcileMetadata(records, changedBaseline, NOW + 3), { code: 'MODEL_SELECTION_CHANGED' });
});

test('multi-scope reconciliation can renew one shared private Release with both openings atomically', () => {
  const fixture = activeFixture({ beta: true }); const records = deniedRecords(fixture, { beta: true });
  const command = reconcileCommand(fixture, records, { beta: true });
  const renewed = parseJson(fixture.set.sign({ ...fixture.candidateReleaseEnvelope.signed, revision: 2,
    issuedAt: NOW + 3, expiresAt: NOW + 90 * DAY }));
  command.publication.privateReleaseUpdates = [{ releaseKey: 'release-3', envelope: renewed }];
  for (const item of command.publication.candidateOpenings) {
    item.opening.releaseEnvelopeSha256 = objectHash(renewed);
    const target = command.publication.bundle.members.find((envelope) => envelope.signed.role === 'target'
      && envelope.signed.scope.channel === records[item.scopeKey].channel).signed;
    target.rolloutSetCommitment = rolloutCommitment(item.opening);
  }
  resign(fixture, command.publication);
  const plan = planReconcileMetadata(records, command, NOW + 3); const result = applyAtomically(records, plan, NOW + 3).records;
  for (const scopeKey of ['scope', 'betaScope']) {
    assert.equal(result[scopeKey].currentCandidateOpening.releaseEnvelopeSha256, objectHash(renewed));
    assert.equal(result[scopeKey].metadataSyncPending, false);
  }
  assert.equal(result.head.releaseEnvelopes['release-3'].signed.revision, 2);
  assert.deepEqual(result.rollout, records.rollout); assert.deepEqual(result.betaRollout, records.betaRollout);
  assert(result['verification-archive'].entries.some((entry) => entry.release.signed.scope.releaseId === 'release-3'));
  const raced = structuredClone(records); raced.betaCandidate.revision++;
  assert.throws(() => applyAtomically(raced, plan, NOW + 3), { code: 'MODEL_CAS_CONFLICT' });
  // A changed Release hash already changes the commitment; that still cannot
  // stand in for a fresh salt on either leaf of this fanout.
  for (const scopeKey of ['scope', 'betaScope']) {
    const copy = structuredClone(command); const item = copy.publication.candidateOpenings.find((entry) => entry.scopeKey === scopeKey);
    item.opening.leafSalt = records[scopeKey].currentCandidateOpening.leafSalt;
    copy.publication.bundle.members.find((envelope) => envelope.signed.role === 'target'
      && envelope.signed.scope.channel === records[scopeKey].channel).signed.rolloutSetCommitment = rolloutCommitment(item.opening);
    resign(fixture, copy.publication); const before = objectHash(records);
    assert.throws(() => planReconcileMetadata(records, copy, NOW + 3), { code: 'MODEL_CANDIDATE_SALT_REUSED' });
    assert.equal(objectHash(records), before);
  }
});

test('single-scope hidden renewal cannot use a changed envelope hash to reuse the old candidate salt', () => {
  const fixture = activeFixture(); const records = deniedRecords(fixture); const command = reconcileCommand(fixture, records);
  const envelope = parseJson(fixture.set.sign({ ...fixture.candidateReleaseEnvelope.signed, revision: 2,
    issuedAt: NOW + 3, expiresAt: NOW + 90 * DAY }));
  command.publication.privateReleaseUpdates = [{ releaseKey: 'release-3', envelope }];
  const opening = command.publication.candidateOpenings[0].opening;
  opening.releaseEnvelopeSha256 = objectHash(envelope); opening.leafSalt = records.scope.currentCandidateOpening.leafSalt;
  command.publication.bundle.members[0].signed.rolloutSetCommitment = rolloutCommitment(opening); resign(fixture, command.publication);
  const before = objectHash(records);
  assert.throws(() => planReconcileMetadata(records, command, NOW + 3), { code: 'MODEL_CANDIDATE_SALT_REUSED' });
  assert.equal(objectHash(records), before);
});

test('reconciliation removes only a protected unavailable path and compares its safety facts in the final CAS', () => {
  const fixture = activeFixture(); const records = fixture.records;
  records.pathDeployment = { ...structuredClone(records.baseline), deploymentId: 'path-deployment',
    chain: { ...records.baseline.chain, deploymentKey: 'pathDeployment' } };
  records.pathApproval = { revision: 1, recordKind: 'ordinaryPathApproval', approvalId: 'path-approval', state: 'approved',
    deploymentKey: 'pathDeployment', deploymentRevision: 1, channel: 'stable' };
  records.inventory.requiredEntityKeys.push('pathDeployment', 'pathApproval');
  const target = records.head.bundle.members.find((item) => item.signed.role === 'target').signed;
  target.ordinaryPaths = [{ deploymentId: 'path-deployment', deploymentRevision: 1, approvalId: 'path-approval', approvalRevision: 1,
    release: structuredClone(target.baselineRelease) }];
  const current = { bundle: records.head.bundle }; resign(fixture, current); records.head.bundle = current.bundle;
  const denied = deniedRecords(fixture); const command = reconcileCommand(fixture, denied);
  command.publication.bundle.members[0].signed.ordinaryPaths = []; resign(fixture, command.publication);
  assert.throws(() => planReconcileMetadata(denied, command, NOW + 3), { code: 'MODEL_SELECTION_CHANGED' });
  // Test-only current safety fact: this path's parent was independently withdrawn.
  denied.pathDeployment.state = 'withdrawn'; denied.pathDeployment.revision++;
  command.publication.readSet = command.publication.readSet.map((read) => read.key === 'pathDeployment'
    ? { ...read, revision: denied.pathDeployment.revision, sha256: objectHash(denied.pathDeployment) } : read);
  const plan = planReconcileMetadata(denied, command, NOW + 3);
  assert.deepEqual(applyAtomically(denied, plan, NOW + 3).records.head.bundle.members[0].signed.ordinaryPaths, []);
  for (const key of ['pathDeployment', 'pathApproval', 'target-2', 'supply-2', 'artifact-2', 'deny']) {
    const raced = structuredClone(denied); raced[key].revision++;
    assert.throws(() => applyAtomically(raced, plan, NOW + 3), { code: 'MODEL_CAS_CONFLICT' });
  }
});
