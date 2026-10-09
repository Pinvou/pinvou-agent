import test from 'node:test';
import assert from 'node:assert/strict';
import { canonicalize, parseJson } from '../canonical-json.mjs';
import { envelopeReference, verifyArchivedPackage, verifyArchivedRelease } from '../signatures.mjs';
import { applyAtomically, captureRecord } from '../models/atomic.mjs';
import { publicationParts, planPublish } from '../models/publication.mjs';
import { planRenewPrivateSelection } from '../models/private-renewal.mjs';
import { planRolloutCommand } from '../models/rollout.mjs';
import { currentReleaseEnvelope } from '../models/metadata-inputs.mjs';
import { objectHash } from '../models/inputs.mjs';
import { publicationFixture, rolloutCommand, proposedPublication, refreshPaths, catchUpQuality } from './publication-fixtures.mjs';
import { operation, operationCommand } from './model-fixtures.mjs';
import { NOW } from './fixtures.mjs';
import { DAY } from '../semantics.mjs';

function activeFixture() {
  const fixture = publicationFixture();
  const command = rolloutCommand(fixture, fixture.records, 'start', NOW + 1);
  fixture.records = applyAtomically(fixture.records, planRolloutCommand(fixture.records, command, NOW + 1), NOW + 1).records;
  return fixture;
}
function renewCommand(fixture, now = NOW + 2, { expired = false } = {}) {
  const records = fixture.records; records.operation = operation(now);
  refreshPaths(records, { pathKey: 'renew-path', now });
  const release = { ...structuredClone(fixture.candidateReleaseEnvelope.signed), revision: 2, issuedAt: now, expiresAt: now + 90 * DAY };
  const envelope = parseJson(fixture.set.sign(release));
  const opening = { ...structuredClone(records.scope.currentCandidateOpening), releaseEnvelopeSha256: objectHash(envelope),
    leafSalt: Buffer.alloc(32, 99).toString('hex') };
  const publication = proposedPublication(fixture, records, { now, opening });
  publication.privateReleaseUpdates = [{ releaseKey: 'release-3', envelope }];
  if (expired) {
    const baseline = { ...structuredClone(fixture.set.metadata.release), revision: 2, issuedAt: now, expiresAt: now + 90 * DAY };
    const baselineEnvelope = parseJson(fixture.set.sign(baseline));
    const target = publication.bundle.members.find((item) => item.signed.role === 'target').signed;
    target.baselineRelease = envelopeReference(canonicalize(baselineEnvelope));
    const members = [parseJson(fixture.set.sign(target)), baselineEnvelope, parseJson(fixture.set.bytes.package)];
    const snapshot = { ...publication.bundle.snapshot.signed, entries: members.map((item) => envelopeReference(canonicalize(item))) };
    const snapshotEnvelope = parseJson(fixture.set.sign(snapshot));
    const timestamp = { ...publication.bundle.timestamp.signed, snapshot: envelopeReference(canonicalize(snapshotEnvelope)) };
    publication.bundle = { members, snapshot: snapshotEnvelope, timestamp: parseJson(fixture.set.sign(timestamp)) };
  }
  const parts = publicationParts(records, publication, now);
  const scope = structuredClone(parts.writes.find((write) => write.key === 'scope').value); scope.currentCandidateOpening = opening;
  const view = { scope, head: parts.writes.find((write) => write.key === 'head').value };
  records['renew-path'].projectedWritesSha256 = objectHash(view);
  return { ...operationCommand, scopeKey: 'scope', futurePathKey: 'renew-path', publication };
}

test('private Release renewal atomically updates opening/generation without leaking the candidate or resetting quality', () => {
  for (const expired of [false, true]) {
    const fixture = activeFixture(); const records = fixture.records; const now = expired ? NOW + 91 * DAY : NOW + 2;
    const command = renewCommand(fixture, now, { expired }); const oldRollout = structuredClone(records.rollout);
    const plan = planRenewPrivateSelection(records, command, now); const result = applyAtomically(records, plan, now).records;
    assert.equal(result.head.releaseEnvelopes['release-3'].signed.revision, 2);
    assert.equal(result.scope.selectionGeneration, records.scope.selectionGeneration + 1);
    assert.equal(result.scope.currentCandidateOpening.releaseEnvelopeSha256, objectHash(result.head.releaseEnvelopes['release-3']));
    assert.deepEqual(result.rollout, oldRollout); assert.equal(result['release-3'].revision, 1);
    assert.deepEqual(result['stage-fact'], records['stage-fact']);
    assert(!result.head.bundle.members.some((item) => item.signed.role === 'release' && item.signed.scope.releaseId === 'release-3'));
    assert(result['verification-archive'].entries.some((item) => item.release.signed.scope.releaseId === 'release-3'));
    for (const key of ['root', 'head', 'scope', 'rollout', 'candidate', 'release-3', 'target-3', 'verification-archive',
      records.root.rootLineageKey, 'trust-anchor', records[records.root.rootLineageKey].chainMaterialKey]) {
      const raced = structuredClone(records); raced[key].revision++;
      assert.throws(() => applyAtomically(raced, plan, now), { code: 'MODEL_CAS_CONFLICT' });
    }
  }
});

test('private renewal rejects immutable changes, stale or unbound openings and plain metadata refresh', () => {
  const fixture = activeFixture(); const records = fixture.records; const command = renewCommand(fixture);
  for (const mutate of [(copy) => { copy.publication.privateReleaseUpdates[0].envelope.signed.appVersion = '3.1.0'; },
    (copy) => { copy.publication.candidateOpenings[0].opening.releaseEnvelopeSha256 = 'b'.repeat(64); },
    (copy) => { copy.publication.candidateOpenings[0].opening.leafSalt = records.scope.currentCandidateOpening.leafSalt; },
    (copy) => { copy.publication.candidateOpenings[0].opening.rolloutRevision++; },
    (copy) => { copy.publication.privateReleaseUpdates.push(copy.publication.privateReleaseUpdates[0]); },
    (copy) => { copy.publication.readSet = copy.publication.readSet.filter((read) => read.key !== 'verification-archive'); }]) {
    const copy = structuredClone(command); mutate(copy); assert.throws(() => planRenewPrivateSelection(records, copy, NOW + 2));
  }
  const refresh = structuredClone(command); refresh.publication.mode = 'refresh'; refresh.publication.affectedScopeKeys = [];
  assert.throws(() => planPublish(records, refresh, NOW + 2));
});

test('Rollout advance composes a renewed hidden Release with its actual stage publication', () => {
  const fixture = activeFixture(); const records = fixture.records; const now = NOW + 1001;
  records.operation = operation(now); catchUpQuality(records);
  const command = rolloutCommand(fixture, records, 'advance', now);
  const renewed = { ...structuredClone(fixture.candidateReleaseEnvelope.signed), revision: 2, issuedAt: now, expiresAt: now + 90 * DAY };
  const envelope = parseJson(fixture.set.sign(renewed)); command.opening.releaseEnvelopeSha256 = objectHash(envelope);
  command.publication = proposedPublication(fixture, records, { now, opening: command.opening });
  command.publication.privateReleaseUpdates = [{ releaseKey: 'release-3', envelope }];
  const parts = publicationParts(records, command.publication, now);
  const view = { scope: parts.writes.find((write) => write.key === 'scope').value, head: parts.writes.find((write) => write.key === 'head').value };
  records.path.projectedWritesSha256 = objectHash(view);
  const result = applyAtomically(records, planRolloutCommand(records, command, now), now).records;
  assert.equal(result.rollout.percentage, 100); assert.equal(result.head.releaseEnvelopes['release-3'].signed.revision, 2);
  assert.equal(result.scope.currentCandidateOpening.releaseEnvelopeSha256, objectHash(envelope));
});

test('candidate exit archives its Package verification chain and removes only unreferenced current material', () => {
  const fixture = activeFixture(); let records = fixture.records;
  const renew = renewCommand(fixture); records = applyAtomically(records, planRenewPrivateSelection(records, renew, NOW + 2), NOW + 2).records;
  fixture.records = records; records.operation = operation(NOW + 3);
  const abort = rolloutCommand(fixture, records, 'abort', NOW + 3); Object.assign(abort, { actor: 'operator', abortCause: 'operator_abort' });
  const result = applyAtomically(records, planRolloutCommand(records, abort, NOW + 3), NOW + 3).records;
  assert.equal(result.head.releaseEnvelopes['release-3'], undefined);
  assert(result.head.releaseEnvelopes['release-2']); assert.throws(() => currentReleaseEnvelope(result, 'release-3'));
  const archived = result['verification-archive'].entries.find((entry) => entry.release.signed.scope.releaseId === 'release-3' && entry.release.signed.revision === 2);
  assert(archived);
  assert.equal(archived.lineage.rootBodySha256, objectHash(archived.root));
  verifyArchivedRelease(canonicalize(archived.release), { archivedRoot: archived.root,
    expected: { role: 'release', product: 'pinvou', component: 'app', scope: archived.release.signed.scope } });
  for (const envelope of archived.packages) verifyArchivedPackage(canonicalize(envelope), { archivedRoot: archived.root, now: NOW + 1000 * DAY,
    expected: { role: 'package', product: 'pinvou', component: 'app', scope: envelope.signed.scope } });
});
