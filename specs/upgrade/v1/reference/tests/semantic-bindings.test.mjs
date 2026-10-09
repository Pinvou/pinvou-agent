import test from 'node:test';
import assert from 'node:assert/strict';
import { sha256, consumeRequestDigest } from '../digests.mjs';
import { verifyEnvelope, assertConsumeBinding, envelopeReference } from '../signatures.mjs';
import { assertBindings, assertOccurrence, DAY } from '../semantics.mjs';
import { verifyPublicChain } from '../metadata-chain.mjs';
import { createFixtureSet, NOW, TARGET, HASH } from './fixtures.mjs';

const fixtures = createFixtureSet();
const options = (claims, now = NOW) => ({ trustedRoot: fixtures.metadata.root, now,
  expected: { role: claims.role, product: claims.product, component: claims.component, scope: claims.scope } });
const verify = (claims, now = NOW) => verifyEnvelope(fixtures.sign(claims), options(claims, now));
const rejectChanged = (role, mutate, code) => {
  const claims = structuredClone(fixtures.claims[role]); mutate(claims);
  assert.throws(() => verify(claims), { code });
};

test('newly signed type/mode/purpose, host, baseline and same-Deployment contradictions fail', () => {
  for (const role of ['decision', 'download', 'authorization-install']) {
    rejectChanged(role, (c) => { c.update.upgradeType = 'silent'; }, 'UPGRADE_MODE_INVALID');
    for (const field of ['baselineDeploymentId', 'baselineRevision']) {
      rejectChanged(role, (c) => { c.update[field] = field.endsWith('Id') ? 'wrong-baseline' : 2; }, 'BASELINE_BINDING_INVALID');
    }
    rejectChanged(role, (c) => { c.update.hopChain.deploymentRevision++; }, 'ENTITY_CHAIN_MISMATCH');
    rejectChanged(role, (c) => { c.host.os = 'windows'; }, 'HOST_TARGET_INVALID');
    rejectChanged(role, (c) => { c.host.arch = 'arm64'; }, 'HOST_TARGET_INVALID');
  }
  for (const role of ['authorization-preinstall', 'authorization-activate']) {
    for (const type of ['normal', 'forced']) {
      rejectChanged(role, (c) => { c.update.upgradeType = type; }, 'UPGRADE_MODE_INVALID');
    }
  }
  const forced = structuredClone(fixtures.claims['authorization-install']); forced.update.upgradeType = 'forced';
  verify(forced);
});

test('null supply-chain expiry and half-open non-null approval expiry are supported', () => {
  const claims = structuredClone(fixtures.claims.decision);
  for (const name of ['endpointChain', 'hopChain']) claims.update[name].supplyChain.effectiveExpiresAt = null;
  verify(claims);
  for (const name of ['endpointChain', 'hopChain']) claims.update[name].supplyChain.effectiveExpiresAt = NOW + 1000;
  verify(claims, NOW + 999);
  for (const now of [NOW + 1000, NOW + 1001]) assert.throws(() => verify(claims, now), { code: 'SUPPLY_CHAIN_EXPIRED' });
});

test('plans bind migration/backup policy and the exact consume request, outside stable source identity', () => {
  rejectChanged('decision', (c) => { c.update.plans.backup = { planId: 'backup', revision: 1, sha256: HASH }; }, 'PLAN_BINDING_INVALID');
  rejectChanged('decision', (c) => { c.update.migrationMode = 'backwardCompatible'; }, 'PLAN_BINDING_INVALID');
  for (const purpose of ['preinstall', 'install', 'activate']) {
    const claims = fixtures.claims[`authorization-${purpose}`]; const bytes = fixtures.sign(claims);
    const request = { ...structuredClone(fixtures.consume), purpose, transactionId: claims.transactionId,
      authorizationEnvelopeSha256: sha256(bytes), preparation: claims.preparation, freezeEpoch: claims.freezeEpoch,
      backup: claims.backup, staged: claims.staged, preinstallSlot: claims.preinstallSlot, plans: claims.update.plans };
    assertConsumeBinding(request, bytes, options(claims));
    for (const field of Object.keys(request.plans)) {
      const changed = structuredClone(request);
      changed.plans[field] = { planId: 'different-plan', revision: 2, sha256: 'b'.repeat(64) };
      assert.notEqual(consumeRequestDigest(changed), consumeRequestDigest(request));
      assert.throws(() => assertConsumeBinding(changed, bytes, options(claims)), { code: 'CONSUME_BINDING_INVALID' });
    }
    if (purpose === 'preinstall') {
      const changed = structuredClone(request); changed.preinstallSlot.slotIdentity = 'another-slot';
      assert.throws(() => assertConsumeBinding(changed, bytes, options(claims)), { code: 'CONSUME_BINDING_INVALID' });
    }
  }
});

test('preinstall binds an inactive slot and activate binds a past completed slot with a new transaction', () => {
  rejectChanged('authorization-preinstall', (c) => { c.preinstallSlot = null; }, 'SLOT_BINDING_INVALID');
  rejectChanged('authorization-preinstall', (c) => { c.preinstallSlot.state = 'active'; }, 'SCHEMA_INVALID');
  rejectChanged('authorization-install', (c) => { c.preinstallSlot = fixtures.claims['authorization-preinstall'].preinstallSlot; }, 'SLOT_BINDING_INVALID');
  rejectChanged('authorization-activate', (c) => { c.staged.stagedAt = NOW + 1; }, 'STAGED_WINDOW_INVALID');
  rejectChanged('authorization-activate', (c) => { c.transactionId = c.staged.preinstallTransactionId; }, 'STAGED_TRANSACTION_REUSED');
  rejectChanged('authorization-activate', (c) => { c.staged.stagedValidUntil--; }, 'STAGED_WINDOW_INVALID');
  const claims = structuredClone(fixtures.claims['authorization-activate']);
  for (const name of ['endpointChain', 'hopChain']) claims.update[name].installNotAfter = null;
  claims.staged.stagedValidUntil = claims.staged.stagedAt + 30 * DAY;
  verify(claims);
});

test('both task event roles fit their observation window while accepting legal late uploads', () => {
  for (const prefix of ['reconciliation', 'recovery-review']) {
    const role = `${prefix}-event`; const claims = structuredClone(fixtures.claims[role]);
    claims.task.windowStart = NOW + 1000; claims.task.windowEnd = NOW + 2000;
    claims.nbf = NOW + 1000; claims.eventNotAfter = NOW + 2000;
    verify(claims, NOW + 1000);
    assertOccurrence(claims, NOW + 1999, NOW + DAY);
    for (const occurredAt of [NOW + 999, NOW + 2000, NOW + 2001]) {
      assert.throws(() => assertOccurrence(claims, occurredAt, NOW + DAY), { code: 'EVENT_OCCURRENCE_INVALID' });
    }
    for (const [field, value] of [['nbf', NOW + 999], ['eventNotAfter', NOW + 2001]]) {
      const changed = structuredClone(claims); changed[field] = value;
      assert.throws(() => verify(changed, NOW + 1000), { code: 'TASK_WINDOW_INVALID' });
    }
  }
});

test('task immutable target projection is signed and compared with trusted task facts', () => {
  for (const prefix of ['reconciliation', 'recovery-review']) {
    for (const suffix of ['action', 'event']) {
      const role = `${prefix}-${suffix}`;
      rejectChanged(role, (c) => { c.task.packageManifest.envelopeSha256 = 'b'.repeat(64); }, 'TASK_TARGET_INVALID');
      rejectChanged(role, (c) => { c.task.package.sha256 = 'b'.repeat(64); }, 'TASK_TARGET_INVALID');
      rejectChanged(role, (c) => { c.task.targetVersion = c.task.sourceVersion; }, 'TASK_TARGET_INVALID');
      // Internal coherence cannot establish that a signed object is the caller's
      // original task. Compare every immutable claim with protected task facts.
      const original = fixtures.claims[role];
      for (const mutate of [(c) => { c.task.originalStage.stageId = 'another-stage'; },
        (c) => { c.task.hopChain.deploymentId = 'another-hop'; },
        (c) => { c.task.targetVersion = '3.0.0'; }]) {
        const changed = structuredClone(original); mutate(changed);
        const actual = verify(changed).signed;
        assert.throws(() => assertBindings(actual.task, original.task), { code: 'BINDING_MISMATCH' });
      }
    }
  }
});

function publicSet({ target, releases, packages }) {
  const targetBytes = fixtures.sign(target);
  const members = [targetBytes, ...releases, ...packages];
  const snapshot = { ...fixtures.metadata.snapshot, entries: members.map(envelopeReference) };
  const snapshotBytes = fixtures.sign(snapshot);
  const timestampBytes = fixtures.sign({ ...fixtures.metadata.timestamp, snapshot: envelopeReference(snapshotBytes) });
  return { trustedRoot: fixtures.metadata.root, product: 'pinvou', component: 'app', now: NOW,
    timestampBytes, snapshotBytes, members };
}

test('public graph admits separate same-target packages and explicit historical bridge references', () => {
  const nextPackage = structuredClone(fixtures.metadata.package);
  nextPackage.appVersion = '3.0.0'; nextPackage.fullPackage.packageId = 'package-3';
  const packageBytes = fixtures.sign(nextPackage);
  const nextRelease = structuredClone(fixtures.metadata.release);
  nextRelease.scope.releaseId = 'release-3'; nextRelease.appVersion = '3.0.0';
  nextRelease.targets[0].packageManifest = envelopeReference(packageBytes);
  const releaseBytes = fixtures.sign(nextRelease);
  for (const pathKind of ['ordinaryPaths', 'bridgePaths']) {
    const target = structuredClone(fixtures.metadata.target); target.baselineRelease = envelopeReference(releaseBytes);
    target[pathKind] = [pathKind === 'bridgePaths'
      ? { deploymentId: 'deployment-2', deploymentRevision: 2, deploymentState: 'superseded',
        eligibilityId: 'bridge-2', eligibilityRevision: 1, release: envelopeReference(fixtures.bytes.release) }
      : { deploymentId: 'deployment-2', deploymentRevision: 1, approvalId: 'path-2', approvalRevision: 1,
        release: envelopeReference(fixtures.bytes.release) }];
    const args = publicSet({ target, releases: [fixtures.bytes.release, releaseBytes], packages: [fixtures.bytes.package, packageBytes] });
    assert.equal(verifyPublicChain(args).packages.length, 2);
    const duplicate = { ...fixtures.metadata.snapshot, entries: [...args.members.map(envelopeReference), envelopeReference(packageBytes)] };
    assert.throws(() => verify(duplicate), { code: 'REFERENCE_DUPLICATE' });
    target[pathKind] = [];
    assert.throws(() => verifyPublicChain(publicSet({ target, releases: [fixtures.bytes.release, releaseBytes],
      packages: [fixtures.bytes.package, packageBytes] })), { code: 'PUBLIC_CANDIDATE_LEAK' });
  }
});

test('a public Target rejects a validly signed Release without its own targetKey', () => {
  const release = structuredClone(fixtures.metadata.release);
  const windows = 'windows-x86_64-nsis-perMachine';
  release.targets[0].targetKey = windows; release.targets[0].packageManifest.scope.targetKey = windows;
  const releaseBytes = fixtures.sign(release);
  const target = structuredClone(fixtures.metadata.target); target.baselineRelease = envelopeReference(releaseBytes);
  assert.throws(() => verifyPublicChain(publicSet({ target, releases: [releaseBytes], packages: [fixtures.bytes.package] })),
    { code: 'PUBLIC_TARGET_MISMATCH' });
});

test('package helper selection, relative targets and explicit source revision algorithm are closed', () => {
  const claims = structuredClone(fixtures.metadata.package); verify(claims);
  for (const path of ['../escape', '/absolute', 'a/../escape', 'C:/absolute', 'a\\escape']) {
    const changed = structuredClone(claims); changed.helpers[0].installationTarget = path;
    assert.throws(() => fixtures.sign(changed), { code: 'SCHEMA_INVALID' });
  }
  const wrong = structuredClone(claims); wrong.executionHelperId = 'missing';
  assert.throws(() => verify(wrong), { code: 'HELPER_BINDING_INVALID' });
  wrong.executionHelperId = claims.executionHelperId; wrong.provenance.sourceRevision.value = HASH;
  assert.throws(() => verify(wrong), { code: 'SOURCE_REVISION_INVALID' });
  wrong.provenance.sourceRevision.algorithm = 'git-sha256'; verify(wrong);
});
