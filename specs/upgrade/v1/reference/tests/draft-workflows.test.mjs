import test from 'node:test';
import assert from 'node:assert/strict';
import { API_SCHEMAS, OPERATIONS, openapi } from '../build-api.mjs';
import { assertApiContract, assertApiShape } from '../api-registry.mjs';
import { canonicalize, parseJson, canonicalizePrivateFrame, parsePrivateFrame } from '../canonical-json.mjs';
import { sha256 } from '../digests.mjs';

const hash = 'a'.repeat(64);
const scope = { product: 'pinvou', component: 'desktop', channel: 'stable', targetKey: 'linux-x86_64-deb-perMachine' };
const ref = (kind, id, revision = 1) => ({ kind, id, revision, sha256: hash });
const thresholds = { download: { numerator: 1, denominator: 100 }, verification: { numerator: 1, denominator: 100 },
  installation: { numerator: 1, denominator: 100 }, health: { numerator: 1, denominator: 100 }, preinstallation: null, activation: null };
const draftPlan = { stages: [{ stageId: 'all', percentage: 100, minimumObservationMs: 60000, minimumSamples: 20, thresholds }],
  snIncludeSetRef: null, snExcludeSetRef: null, progressReportIntervalMs: 1000, lossIntervalMs: 10000 };
function context(authorization) {
  return { protocolVersion: 1, deployment: { recordId: 'deployment-1', revision: 1, state: 'draft', scope,
    draft: { releaseId: 'release-1', releaseRevision: 1, releaseTargetId: 'target-1', releaseTargetRevision: 1,
      upgradeType: 'normal', releaseVisibleAt: 1, installNotBefore: 1, installNotAfter: null,
      plan: draftPlan, responsibilityIdentity: 'owner-1' }, draftSha256: hash, createdBy: 'owner-1', updatedBy: 'owner-1', reason: 'prepare', createdSequence: 1 },
    reference: ref('deployment', 'deployment-1'), plan: { planId: 'plan-1', revision: 2, policyRevision: 1, scope, state: 'frozen',
      plan: { upgradeType: 'normal', stages: [{ stageId: 'all', percentage: 100, minimumObservationMs: 60000,
        requiredMetrics: ['download'], metrics: { download: { minimumSamples: 20, threshold: { numerator: 1, denominator: 100 } } } }],
        snIncludeSetRef: null, snExcludeSetRef: null, progressReportIntervalMs: 1000, lossIntervalMs: 10000 } },
    planReference: ref('rolloutPlan', 'plan-1', 2), occupancy: ref('rolloutScope', hash, 0),
    authorization, canCreate: authorization !== null, openRolloutId: null };
}
function proof() {
  return { protocolVersion: 1, status: 'not_committable', proof: { operation: 'create', command: 'deployment.create.v1',
    actorId: 'actor-1', scope, commandKey: 'original-1', publicRequestSha256: hash, internalWireSha256: 'b'.repeat(64),
    basis: { kind: 'parent_advanced', original: ref('deployment', 'deployment-1'), current: ref('deployment', 'deployment-1', 2) } } };
}
const invalid = fn => assert.throws(fn, { code: 'API_CONTRACT_INVALID' });

test('fixed draft routes resolve their actual owner schema aliases', () => {
  const selected = OPERATIONS.filter(op => op.path.startsWith('/v1/admin/path-policy/floor-drafts/') || op.path.startsWith('/v1/admin/deployment-rollouts/'));
  assert.equal(selected.length, 18);
  for (const op of selected) {
    assert(API_SCHEMAS.has(op.requestShape)); assert(API_SCHEMAS.has(op.responseShape));
    assert.equal(openapi.paths[op.path].post.operationId, op.name);
  }
});
test('management context authorization references and null are not DSSE credentials', () => {
  for (const authority of [null, ref('authorization', 'actor-1')]) {
    const value = context(authority);
    assert.equal(assertApiContract('deployment-rollout-context', 'response', value), value);
  }
  const wrong = context(ref('authorization', 'actor-1')); wrong.authorization.signed = {};
  invalid(() => assertApiContract('deployment-rollout-context', 'response', wrong));
});
test('create terminal observations stay distinct from ACKs and unresolved locators', () => {
  const value = proof(); assert.equal(assertApiContract('deployment-rollout-lookup', 'response', value), value);
  const unexpected = structuredClone(value); unexpected.resultId = hash;
  invalid(() => assertApiContract('deployment-rollout-lookup', 'response', unexpected));
  const wrong = structuredClone(value); wrong.proof.command = 'rollout.abort.v1';
  invalid(() => assertApiContract('deployment-rollout-lookup', 'response', wrong));
  const badBasis = structuredClone(value); badBasis.proof.basis.current.kind = 'authorization';
  invalid(() => assertApiContract('deployment-rollout-lookup', 'response', badBasis));
  const unresolved = { protocolVersion: 1, status: 'unresolved', resultId: null, rollout: null, deployment: null, plan: null };
  assert.equal(assertApiContract('deployment-rollout-lookup', 'response', unresolved), unresolved);
});
test('draft HTTP ACKs require actual wrapper fields and no running lifecycle', () => {
  const ack = { protocolVersion: 1, requestId: 'request-1', resultId: hash, revision: 1, rolloutId: 'rollout-1', stageFactRevision: null, state: 'draft' };
  assert.equal(assertApiContract('deployment-rollout-create', 'response', ack), ack);
  const missing = { ...ack }; delete missing.requestId;
  invalid(() => assertApiContract('deployment-rollout-create', 'response', missing));
  invalid(() => assertApiContract('deployment-rollout-create', 'response', { ...ack, state: 'running' }));
  invalid(() => assertApiContract('deployment-rollout-create', 'response', { ...ack, stageFactRevision: 1 }));
});

// Adapted from the existing floor-draft-fixtures.ts concrete empty-census
// record. These are structural/capacity controls, never native/MAC evidence.
function floorRecord(version) {
  const scope = { product: 'test-product', component: 'agent', channel: 'stable', targetKey: 'linux-x86_64-deb-perMachine' };
  const inventory = { schemaVersion: 1, scope, snapshotId: 'test-census', revision: 1, observedAt: 100, sources: [] };
  const request = { protocolVersion: 1, scope, expectedBasis: { authorization: { recordKind: 'authorization', recordId: 'test-actor', revision: 1, sha256: hash }, head: null },
    declaredBase: null, inventory, proposedVersion: version, reason: 'planning-only', commandKey: 'original-key' };
  return { protocolVersion: 1, purpose: 'support-floor-planning-draft.v1', scope, draftId: 'floor-draft-' + hash, revision: 1,
    authorId: 'test-actor', savedAt: 101, resultId: hash, requestSha256: sha256(canonicalize(request)), inputEvidence: 'declared-unverified', policyEffect: 'none', request,
    proposal: { schemaVersion: 1, purpose: 'support-floor-proposal-data.v1', reason: request.reason,
      assessment: { scope, direction: 'initial', base: null, proposedVersion: version, inventory: { id: 'test-census', revision: 1, sha256: sha256(canonicalize(inventory)) },
        impact: { observedInstallations: 0, beforeInFloorRange: 0, afterInFloorRange: 0, removedFromFloorRange: 0, addedToFloorRange: 0, sources: [] },
        reviewRequirement: { command: 'ChangeSupportFloor', permission: 'commands.policy', reviews: 2, mfa: true },
        missingEvidence: ['current-scope-and-floor-source-required', 'current-complete-census-and-registry-source-required', 'actual-two-review-mfa-floor-approval-required',
          'complete-current-paths-and-atomic-publication-required', 'actual-migration-bootstrap-source-not-installed'] } } };
}
function floorHistory(records) {
  return { protocolVersion: 1, requestId: 'request-1', result: { protocolVersion: 1, scope: records[0].scope, records,
    nextAfterRevision: records.length, more: false } };
}
test('private floor history accepts a lawful aggregate larger than one metadata frame', () => {
  const record = floorRecord('1'.repeat(60000) + '.0.0');
  assert(canonicalize(record).length < 1024 * 1024);
  const records = Array.from({ length: 32 }, (_, index) => ({ ...record, revision: index + 1 }));
  const value = floorHistory(records), bytes = canonicalizePrivateFrame(value);
  assert(bytes.length > 1024 * 1024 && bytes.length < 33 * 1024 * 1024);
  assert.equal(assertApiContract('support-floor-draft-history', 'response', value), value);
  assert.throws(() => canonicalize(value), { code: 'JSON_INPUT_LIMIT' });
  assert.throws(() => parseJson(bytes), { code: 'JSON_INPUT_LIMIT' });
  assert.throws(() => assertApiContract('support-floor-draft-history', 'request', value), { code: 'JSON_INPUT_LIMIT' });
});
test('private floor framing cannot hide an oversized individual record', () => {
  const record = floorRecord('1.0.0');
  record.proposal.assessment.impact.sources = Array.from({ length: 24 }, (_, index) => ({ sourceProfileId: index.toString(16).padStart(64, '0'),
    canonicalAppVersion: '1'.repeat(60000) + '.0.0', installations: 0, beforeInFloorRange: false, afterInFloorRange: false }));
  const value = floorHistory([record]);
  assertApiShape('floor-draft.history-response', value);
  assert(canonicalizePrivateFrame(record).length > 1024 * 1024);
  assert.throws(() => assertApiContract('support-floor-draft-history', 'response', value), { code: 'JSON_INPUT_LIMIT' });
});
test('private frame helpers retain strict input and object safety with a fixed total cap', () => {
  const bytes = new TextEncoder().encode('{"a":1,"a":2}');
  assert.throws(() => parsePrivateFrame(bytes), { code: 'JSON_DUPLICATE_KEY' });
  assert.throws(() => canonicalizePrivateFrame({ get a() { throw new Error('getter ran'); } }), { code: 'JSON_INVALID_VALUE' });
  const large = Array(600).fill('a'.repeat(60000));
  assert.throws(() => canonicalizePrivateFrame(large), { code: 'JSON_INPUT_LIMIT' });
  assert.throws(() => parseJson(new Uint8Array(), 33 * 1024 * 1024), { code: 'JSON_LIMIT_INVALID' });
});
