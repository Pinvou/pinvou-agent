// Fixed public contract-model observations, never native or protected evidence.
export const modelContext = Object.freeze({ targetKey: 'windows-x86_64-nsis-perMachine', os: 'windows',
  hostArch: 'x86_64', upgradeType: 'normal', installationScopeId: 'synthetic-installation' });
const h = 'a'.repeat(64), other = 'b'.repeat(64);
const artifact = { packageId: 'package', size: 12, sha256: h, metadataSha256: other };
const object = { objectId: 'object', generation: 'generation', sha256: h, size: 12, metadataSha256: other };
const owner = { preparationId: 'preparation', ownerEpoch: 2, scopeSha256: h };
const held = { ...owner, exclusive: true, durable: true };
const slot = { slotId: 'slot', slotRevision: 3, sha256: h };
const execution = { transactionId: 'transaction', ownerEpoch: 2, ...object };
const backup = { backupId: 'backup', preparationId: 'preparation', freezeEpoch: 'freeze', scopeSha256: h,
  manifestSha256: other, recordId: 'record', keyReferenceId: 'key-reference', originalCleanupDeadline: 8000 };
const window = { windowId: 'window', stage: 'coordination', firstStartedAt: 1000, originalDurationMs: 120000, durationMs: 120000,
  originalDeadline: 121000, sessionDeadline: 10000, effectiveDeadline: 10000, spentMs: 100, revision: 1 };
const partition = { owner: 'preparation', scopeId: 'synthetic-installation', recordId: 'record' };

// Each row fixes a valid producer observation and a distinguishing invalid fact.
// Requests are expected observations from the test owner, not untrusted permits.
const rows = [
  ['detectInstallation', { expectedScopeId: 'synthetic-installation' }, { installed: true, scopeId: 'synthetic-installation', version: '1.2.3', identitySha256: h }, ['identitySha256', null]],
  ['normalizeVersion', { productVersion: '1.2.3' }, { productVersion: '1.2.3', fileVersion: '1.2.3.0' }, ['fileVersion', '1.2.3.1']],
  ['verifyArtifact', artifact, { ...artifact, completeRead: true, formatValid: true }, ['sha256', other]],
  ['verifyExecutableIdentity', { ...artifact, generation: 'generation', role: 'helper' },
    { ...artifact, generation: 'generation', role: 'helper', retainedObject: true, immediateRecheck: true }, ['generation', 'replaced-generation']],
  ['acquirePrivilege', { requiredRole: 'helper' }, { role: 'helper', verified: true, consentObserved: true }, ['consentObserved', false]],
  ['verifyExistingHelperPrivilege', { helperIdentitySha256: h }, { helperIdentitySha256: h, sufficient: true, interactivePrompt: false }, ['interactivePrompt', true]],
  ['readProtectedPartition', partition, { ...partition, revision: 1, contentSha256: h, custodyVerified: true }, ['custodyVerified', false]],
  ['commitProtectedPartition', { ...partition, expectedRevision: 1, contentSha256: h },
    { ...partition, revision: 2, contentSha256: h, precommitRecheck: true, committedReadback: true }, ['committedReadback', false], 'committed'],
  ['acquireGlobalLock', owner, held, ['exclusive', false], 'committed'],
  ['persistPreparation', { ...owner, expectedRevision: 1, originalStartedAt: 1000 },
    { ...owner, revision: 2, originalStartedAt: 1000, readbackVerified: true }, ['originalStartedAt', 2000], 'committed'],
  ['transferOwnership', { ...owner, nextEpoch: 3 }, { ...held, ownerEpoch: 3, previousOwnerInvalidated: true }, ['ownerEpoch', 2], 'committed'],
  ['recoverOwnership', owner, held, ['durable', false], 'committed'],
  ['fenceOwner', { ...owner, staleEpoch: 1 }, { ...owner, rejectedEpoch: 1, staleActionsRejected: true }, ['rejectedEpoch', 2], 'committed'],
  ['coordinateWriters', { ...owner, expectedWriterIds: ['user', 'service'] },
    { ...owner, writerIds: ['service', 'user'], inventoryComplete: true, allSavable: true }, ['allSavable', false]],
  ['freezeAffectedScopes', { ...owner, freezeEpoch: 'freeze', expectedWriterIds: ['user', 'service'] },
    { ...owner, freezeEpoch: 'freeze', writers: ['user', 'service'].map(writerId => ({ writerId, epoch: 'freeze', saved: true, stopped: true, consistent: true })),
      admissionFenced: true, readbackVerified: true }, ['admissionFenced', false], 'committed'],
  ['verifyDataConsistency', { ...owner, freezeEpoch: 'freeze' }, { ...owner, freezeEpoch: 'freeze', completeScopeRead: true, consistent: true }, ['completeScopeRead', false]],
  ['TrustedDeadlineProvider', { expectedBootId: 'boot', policySha256: h },
    { bootId: 'boot', policySha256: h, lowerUtc: 1000, upperUtc: 1001, continuityVerified: true, cleanupBarrier: false }, ['cleanupBarrier', true]],
  ['persistStageBudget', { previous: window, proposed: { ...window, spentMs: 200, revision: 2 } },
    { retained: { ...window, spentMs: 200, revision: 2 }, readbackVerified: true }, ['readbackVerified', false], 'committed'],
  ['handoffVerifiedExecutionObject', object, { ...object, previousOwnerInvalidated: true, custodyRetained: true }, ['previousOwnerInvalidated', false], 'committed'],
  ['recheckBeforeAction', object, { ...object, sameActualObject: true, retainedThroughAction: true }, ['sameActualObject', false]],
  ['runDirectInstaller', execution, { ...execution, installerInvokedAt: 1200, executionCommitBoundary: 1200 }, ['executionCommitBoundary', 1201], 'committed'],
  ['inspectInstalledVersion', { expectedIdentitySha256: h }, { activeIdentitySha256: h, completeRead: true }, ['activeIdentitySha256', other]],
  ['preinstallInactiveSlot', slot, { ...slot, activeMutationCount: 0, isolated: true }, ['activeMutationCount', 1], 'committed'],
  ['verifyStagedSlot', slot, { ...slot, completeRead: true, formatValid: true }, ['slotRevision', 4]],
  ['activateStagedSlot', { ...slot, transactionId: 'transaction', ownerEpoch: 2 },
    { ...slot, transactionId: 'transaction', ownerEpoch: 2, firstActiveWriteAt: 1100, pointerSwitchedAt: 1200, executionCommitBoundary: 1100 }, ['executionCommitBoundary', 1200], 'committed'],
  ['cleanInactiveSlot', slot, { ...slot, activeSlotTouched: false, deletionConfirmed: true }, ['activeSlotTouched', true], 'committed'],
  ['launchAndHealthCheck', { transactionId: 'transaction', expectedActiveIdentitySha256: h, scopeSha256: other, originalHealthStartedAt: 1000 },
    { transactionId: 'transaction', activeIdentitySha256: h, scopeSha256: other, healthStartedAt: 1000, finishedAt: 2000,
      attempts: 1, attemptDurationsMs: [1000], requiredMigrationsComplete: true, completeDataConsistent: true, localReady: true, storageOpen: true,
      localComponentsReady: true, fatalOrLoop: false }, ['localReady', false]],
  ['registerBackup', backup, { ...backup, protectedReadback: true, bytesWritten: 0 }, ['bytesWritten', 1], 'committed'],
  ['createBackup', backup, { ...backup, registrationBeforeFirstByte: true, scopeComplete: true, durableBytes: true }, ['registrationBeforeFirstByte', false], 'committed'],
  ['verifyBackup', backup, { ...backup, formatSupported: true, aeadFinal: true, completeReadback: true, manifestExact: true }, ['aeadFinal', false]],
  ['exportBackup', { ...backup, exportId: 'export' }, { ...backup, exportId: 'export', independentCopy: true, userDestinationConfirmed: true, auditRecorded: true }, ['originalCleanupDeadline', 9000], 'committed'],
  ['deleteBackup', backup, { ...backup, containerAbsent: true, keyAbsent: true, cleanupReadback: true }, ['keyAbsent', false], 'committed'],
];
export function vectors() {
  return rows.map(([operation, request, facts, invalid, effect = 'none']) => ({
    record: structuredClone({ operation, context: modelContext, request, result: { kind: 'observed', effect, facts } }),
    invalid: structuredClone(invalid),
  }));
}
