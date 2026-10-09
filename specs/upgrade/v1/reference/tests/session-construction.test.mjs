import test from 'node:test';
import assert from 'node:assert/strict';
import { planCreateSession, hopIdentitySha256, downloadObservation, workflowContext } from '../models/workflow.mjs';
import { planEvent } from '../models/events.mjs';
import { applyAtomically } from '../models/atomic.mjs';
import { objectHash } from '../models/inputs.mjs';
import { selectionIdentitySha256 } from '../models/inputs.mjs';
import { planSessionTimeout } from '../models/lifecycle.mjs';
import { parseJson } from '../canonical-json.mjs';
import { createFixtureSet, NOW, HASH, TARGET } from './fixtures.mjs';
import { addQualification, operation, operationCommand, modelFacts } from './model-fixtures.mjs';

import { sessionFixture, emit } from './session-fixtures.mjs';

test('actual session constructor reaches required backup and preparation using persisted freeze identity', () => {
  const fixture = sessionFixture({ required: true }); let records = fixture.records;
  const facts = { ...modelFacts(), confirmedHopSha256: hopIdentitySha256(fixture.decision) };
  const route = ['update_offered', 'user_confirmed', 'download_started', 'download_succeeded', 'verification_started',
    'verification_succeeded', 'preflight_started', 'preflight_succeeded', 'permission_checked', 'permission_granted',
    'preparation_started', 'writers_frozen', 'backup_started', 'backup_succeeded', 'preparation_ready'];
  for (const [index, state] of route.entries()) records = emit(fixture, records, state, facts, NOW + index + 1);
  assert.equal(records.session.state, 'preparation_ready'); assert.equal(records.session.freezeEpoch, 1);
  assert.equal(records.workflow.confirmedHopSha256, hopIdentitySha256(fixture.decision));
});
test('actual activation constructor preserves waiting revision and reaches preparation without downloading', () => {
  const fixture = sessionFixture({ activate: true }); let records = fixture.records;
  const facts = { ...modelFacts(), hopSha256: hopIdentitySha256(fixture.decision) };
  for (const [index, state] of ['update_offered', 'staged_verified', 'preflight_started', 'preflight_succeeded', 'permission_checked',
    'permission_granted', 'preparation_started', 'writers_frozen', 'preparation_ready'].entries())
    records = emit(fixture, records, state, facts, NOW + index + 1);
  assert.equal(records.session.stagedRevision, 1); assert.equal(records.session.state, 'preparation_ready');
  assert.equal(records.workflow.writableSessionId, 'activate-session'); assert.equal(records.workflow.firstStartedAt, NOW - 1000);
});
test('expired required-backup sessions can lawfully report historical freeze and backup without restoring rights', () => {
  const fixture = sessionFixture({ required: true }); let records = fixture.records;
  const facts = { ...modelFacts(), confirmedHopSha256: hopIdentitySha256(fixture.decision) };
  const route = ['update_offered', 'user_confirmed', 'download_started', 'download_succeeded', 'verification_started',
    'verification_succeeded', 'preflight_started', 'preflight_succeeded', 'permission_checked', 'permission_granted', 'preparation_started'];
  for (const [index, state] of route.entries()) records = emit(fixture, records, state, facts, NOW + index + 1);
  records = applyAtomically(records, planSessionTimeout(records, { sessionKey: 'session' }, NOW + 86_400_000), NOW + 86_400_000).records;
  for (const [index, state] of ['writers_frozen', 'backup_started', 'backup_succeeded', 'preparation_ready'].entries())
    records = emit(fixture, records, state, facts, NOW + 100 + index, NOW + 86_400_001 + index);
  assert.equal(records.session.state, 'expired'); assert.equal(records.session.freezeEpoch, null);
  assert.equal(records['session-diagnostic'].freezeEpoch, 1); assert.equal(records['session-diagnostic'].lastState, 'preparation_ready');
});
test('negative signed decision creates a no-update session without workflow or quality contribution', () => {
  const fixture = sessionFixture({ noUpdate: true }); const result = emit(fixture, fixture.records, 'no_update', { factType: 'local-state' }, NOW + 1);
  assert.equal(result.session.state, 'no_update'); assert.equal(result['contribution-head'].watermark, 0);
  assert.equal(Object.hasOwn(result, 'workflow'), false);
});
test('progress keeps the logical download observable; fenced sessions accept only prior historical actions', () => {
  const fixture = sessionFixture(); let records = fixture.records;
  const facts = { ...modelFacts(), confirmedHopSha256: hopIdentitySha256(fixture.decision) };
  for (const [index, state] of ['update_offered', 'user_confirmed', 'download_started'].entries()) records = emit(fixture, records, state, facts, NOW + index + 1);
  const claims = fixture.set.claims['telemetry-event']; claims.allowedEvents = ['download_progress'];
  const progress = { factType: 'download-progress', bytesReceived: 1, totalBytes: 123, observation: 'waiting', updatedAt: NOW + 119_999, fileSourceClass: 'primary' };
  const event = { eventId: 'progress-1', sequence: records.session.sequence + 1, lineageId: 'session-1', executionPurpose: 'install',
    occurredAt: progress.updatedAt, kind: 'download_progress', fromState: 'download_started', toState: 'download_started', facts: progress,
    factsSha256: objectHash(progress), targetIdentitySha256: HASH };
  const command = { rootKey: 'root', denyKey: 'deny', lineageKey: 'session', ledgerKey: 'progress-ledger', outboxKey: 'progress-outbox',
    credential: parseJson(fixture.set.sign(claims)), expectedContext: { role: claims.role, product: claims.product, component: claims.component, scope: claims.scope },
    scope: fixture.scope, event };
  const observed = applyAtomically(records, planEvent(records, command, NOW + 120_000), NOW + 120_000).records;
  assert.equal(downloadObservation(observed.workflow, NOW + 120_000), 'incomplete');
  records.session.futureActions = 'fenced'; records.session.fencedAt = NOW + 120_001; records.workflow.writableSessionId = 'session-2';
  const late = applyAtomically(records, planEvent(records, command, NOW + 120_002), NOW + 120_002).records;
  assert.equal(late['progress-ledger'].disposition, 'late-diagnostic'); assert.equal(late.session.sequence, records.session.sequence);
  const after = structuredClone(command); after.event.occurredAt = NOW + 120_001; after.event.facts.updatedAt = after.event.occurredAt;
  after.event.factsSha256 = objectHash(after.event.facts);
  assert.throws(() => planEvent(records, after, NOW + 120_002), { code: 'MODEL_EVENT_FENCED' });
});
