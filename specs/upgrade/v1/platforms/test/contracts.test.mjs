import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { checkObservation, operations, unsupported } from '../contract.mjs';
import { vectors } from '../vectors.mjs';
import { id, hash } from '../values.mjs';
import { runModelCases } from '../../../../../tools/upgrade-platform-conformance/model.mjs';

function record(operation) { return vectors().find(item => item.record.operation === operation).record; }
const invalid = value => assert.throws(() => checkObservation(value), { code: 'PLATFORM_FACT_INVALID' });
test('every callable operation has fixed acceptance and a distinguishing counterexample', () => {
  assert.equal(runModelCases().length, Object.keys(operations).length);
});
test('canonical IDs agree with T01 and ID/hash/version reject trailing terminators and paths', async () => {
  const common = JSON.parse(await readFile(new URL('../../schemas/common.json', import.meta.url), 'utf8'));
  const protocolIdentifier = new RegExp(common.$defs.identifier.pattern);
  for (const value of ['package:release-1', 'scope:install', 'A0._:-', 'a'.repeat(128)]) {
    assert.equal(protocolIdentifier.test(value), true); assert.equal(id(value), true);
  }
  // Read the frozen source-ID ASCII restriction directly, independently of the
  // observation helper. None of these expressions enables multiline matching.
  const disallowedSourceCharacter = new RegExp(common.$defs.downloadSourceBinding.properties.sourceId.not.pattern);
  for (const ending of ['\n', '\r', '\u2028', '\u2029']) {
    assert.equal(disallowedSourceCharacter.test(`package${ending}`), true);
    assert.equal(id(`package${ending}`), false); assert.equal(hash(`${'a'.repeat(64)}${ending}`), false);
    const version = record('normalizeVersion'); version.request.productVersion += ending;
    assert.throws(() => checkObservation(version), { code: 'PLATFORM_REQUEST_INVALID' });
  }
  for (const value of ['a'.repeat(129), '_prefix', 'a/b', 'a\\b', 'a\u0000b']) assert.equal(id(value), false);
  const artifact = record('verifyArtifact');
  artifact.request.packageId = artifact.result.facts.packageId = 'package:release-1'; checkObservation(artifact);
  artifact.request.sha256 = artifact.result.facts.sha256 = `${'a'.repeat(64)}\n`;
  assert.throws(() => checkObservation(artifact), { code: 'PLATFORM_REQUEST_INVALID' });
  const scoped = record('detectInstallation');
  scoped.context.installationScopeId = scoped.request.expectedScopeId = scoped.result.facts.scopeId = 'scope:install';
  checkObservation(scoped);
});
test('closed request/result/context and exact target/host prevent accidental portability claims', () => {
  const value = record('detectInstallation'); value.request.path = 'arbitrary';
  assert.throws(() => checkObservation(value), { code: 'PLATFORM_REQUEST_INVALID' });
  delete value.request.path; value.result.facts.unreviewed = true;
  assert.throws(() => checkObservation(value), { code: 'PLATFORM_RESULT_INVALID' });
  delete value.result.facts.unreviewed; value.context.hostArch = 'arm64';
  assert.throws(() => checkObservation(value), { code: 'PLATFORM_CONTEXT_INVALID' });
});
test('silent can inspect existing privilege but cannot acquire it or prompt', () => {
  const acquire = record('acquirePrivilege'); acquire.context.upgradeType = 'silent'; invalid(acquire);
  const existing = record('verifyExistingHelperPrivilege'); existing.context.upgradeType = 'silent'; checkObservation(existing);
  existing.result.facts.interactivePrompt = true; invalid(existing);
});
test('three-part versions map Windows fourth word to zero and keep other targets separate', () => {
  const value = record('normalizeVersion');
  value.context = { ...value.context, os: 'linux', hostArch: 'arm64', targetKey: 'linux-arm64-deb-perMachine' };
  value.result.facts.fileVersion = null; checkObservation(value);
  value.request.productVersion = '01.2.3';
  assert.throws(() => checkObservation(value), { code: 'PLATFORM_REQUEST_INVALID' });
});
test('freeze rejects missing, duplicate, stale and unsavable writers', () => {
  for (const change of [f => f.writers.pop(), f => { f.writers[1] = f.writers[0]; },
    f => { f.writers[1].epoch = 'old-freeze'; }, f => { f.writers[1].saved = false; }]) {
    const value = record('freezeAffectedScopes'); change(value.result.facts); invalid(value);
  }
});
test('budget updates preserve original identity, cutoff and nondecreasing spent across reopen/config changes', () => {
  for (const field of ['windowId', 'stage', 'firstStartedAt', 'originalDurationMs', 'durationMs', 'originalDeadline', 'sessionDeadline', 'effectiveDeadline']) {
    const value = record('persistStageBudget');
    const current = value.request.proposed[field];
    value.request.proposed[field] = field === 'stage' ? 'backup'
      : typeof current === 'number' ? current + 1 : `${current}-changed`;
    if (['firstStartedAt', 'originalDurationMs'].includes(field)) value.request.proposed.originalDeadline++;
    if (field === 'originalDeadline') value.request.proposed.originalDurationMs++;
    if (field === 'durationMs') {
      value.request.previous.durationMs = 110000; value.request.proposed.durationMs = 110001;
    }
    if (field === 'sessionDeadline') value.request.proposed.effectiveDeadline++;
    value.result.facts.retained = structuredClone(value.request.proposed); invalid(value);
  }
  const value = record('persistStageBudget'); value.request.proposed.spentMs = 99;
  value.result.facts.retained.spentMs = 99; invalid(value);
  const download = record('persistStageBudget'); download.request.proposed.stage = 'download';
  assert.throws(() => checkObservation(download), { code: 'PLATFORM_REQUEST_INVALID' });
});
test('stricter effective windows and overspent original high-water can be persisted without renewing origin', () => {
  const value = record('persistStageBudget');
  Object.assign(value.request.proposed, { durationMs: 100000, sessionDeadline: 9000, effectiveDeadline: 9000, spentMs: 130000 });
  value.result.facts.retained = structuredClone(value.request.proposed); checkObservation(value);
  value.request.previous = structuredClone(value.request.proposed);
  Object.assign(value.request.proposed, { revision: 3, sessionDeadline: 500, effectiveDeadline: 500 });
  value.result.facts.retained = structuredClone(value.request.proposed); checkObservation(value);
  const noSession = record('persistStageBudget');
  for (const w of [noSession.request.previous, noSession.request.proposed, noSession.result.facts.retained])
    Object.assign(w, { stage: 'validation', sessionDeadline: null, effectiveDeadline: w.originalDeadline });
  checkObservation(noSession);
});
test('unknown time, positively observed fault, definite refusal and uncertain commit remain distinct', () => {
  const clock = record('TrustedDeadlineProvider');
  clock.result = { kind: 'time_unknown', effect: 'none', reason: 'continuity_unproven', faultEvidenceId: null }; checkObservation(clock);
  clock.result = { kind: 'confirmed_fault', effect: 'none', reason: 'positively_observed_fault', faultEvidenceId: 'actual-evidence-reference' }; checkObservation(clock);
  clock.result.faultEvidenceId = null;
  assert.throws(() => checkObservation(clock), { code: 'PLATFORM_RESULT_INVALID' });
  const commit = record('commitProtectedPartition');
  commit.result = { kind: 'outcome_unknown', effect: 'possible', reason: 'commit_unconfirmed', faultEvidenceId: null }; checkObservation(commit);
  commit.result.kind = 'refused'; commit.result.reason = 'stale_owner';
  assert.throws(() => checkObservation(commit), { code: 'PLATFORM_RESULT_INVALID' });
});
test('direct native invocation cannot move its boundary to later writes; activation uses earliest change', () => {
  const direct = record('runDirectInstaller'); direct.result.facts.executionCommitBoundary++; invalid(direct);
  const activation = record('activateStagedSlot'); activation.result.facts.firstActiveWriteAt = null;
  activation.result.facts.executionCommitBoundary = activation.result.facts.pointerSwitchedAt; checkObservation(activation);
  activation.result.facts.pointerSwitchedAt = null; invalid(activation);
});
test('backup partial completeness and changed lineage/deadline cannot masquerade as verification or cleanup', () => {
  for (const field of ['formatSupported', 'aeadFinal', 'completeReadback', 'manifestExact']) {
    const value = record('verifyBackup'); value.result.facts[field] = false; invalid(value);
  }
  for (const operation of ['registerBackup', 'createBackup', 'verifyBackup', 'exportBackup', 'deleteBackup']) {
    const value = record(operation); value.result.facts.freezeEpoch = 'other-freeze'; invalid(value);
    const cleanup = record(operation); cleanup.result.facts.originalCleanupDeadline++; invalid(cleanup);
  }
});
test('health requires identity, consistency and local readiness within original total and attempt limits', () => {
  for (const [field, value] of [['attempts', 3], ['attemptDurationsMs', [120001]], ['finishedAt', 301001],
    ['healthStartedAt', 2000], ['requiredMigrationsComplete', false], ['completeDataConsistent', false], ['fatalOrLoop', true]]) {
    const health = record('launchAndHealthCheck'); health.result.facts[field] = value;
    if (field === 'healthStartedAt') health.result.facts.finishedAt = value + 1000;
    invalid(health);
  }
  const both = record('launchAndHealthCheck');
  Object.assign(both.result.facts, { attempts: 2, attemptDurationsMs: [120000, 120000], finishedAt: 291000 });
  checkObservation(both);
  Object.assign(both.result.facts, { attemptDurationsMs: [120001, 1], finishedAt: 121002 }); invalid(both);
});
test('unsupported is a callable refusal, not an observed success or execution capability', () => {
  for (const { record: value } of vectors()) {
    const refusal = unsupported(value.context, value.operation, value.request);
    assert.equal(refusal.result.kind, 'unsupported'); assert.equal(refusal.result.effect, 'none');
  }
  const value = record('recheckBeforeAction'), copy = checkObservation(value);
  copy.result.facts.generation = 'changed'; assert.notEqual(copy.result.facts.generation, value.result.facts.generation);
});
