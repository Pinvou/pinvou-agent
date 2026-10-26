import { define, id, hash, uint, positive, boolean, nullable, list, requireFact } from './values.mjs';

const object = { objectId: id, generation: id, sha256: hash, size: uint, metadataSha256: hash };
const execution = { transactionId: id, ownerEpoch: positive, ...object };
function matches(r, f, fields) { requireFact(Object.keys(fields).every(key => r[key] === f[key])); }
const slot = { slotId: id, slotRevision: positive, sha256: hash };
export const executionOperations = {
  handoffVerifiedExecutionObject: define(object, { ...object, previousOwnerInvalidated: boolean, custodyRetained: boolean }, (r, f) => {
    matches(r, f, object); requireFact(f.previousOwnerInvalidated && f.custodyRetained);
  }, 'committed'),
  recheckBeforeAction: define(object, { ...object, sameActualObject: boolean, retainedThroughAction: boolean }, (r, f) => {
    matches(r, f, object); requireFact(f.sameActualObject && f.retainedThroughAction);
  }),
  runDirectInstaller: define(execution, { ...execution, installerInvokedAt: uint, executionCommitBoundary: uint }, (r, f) => {
    matches(r, f, execution); requireFact(f.executionCommitBoundary === f.installerInvokedAt);
  }, 'committed'),
  inspectInstalledVersion: define({ expectedIdentitySha256: hash }, { activeIdentitySha256: hash, completeRead: boolean }, (r, f) => {
    requireFact(f.activeIdentitySha256 === r.expectedIdentitySha256 && f.completeRead);
  }),
  preinstallInactiveSlot: define(slot, { ...slot, activeMutationCount: uint, isolated: boolean }, (r, f) => {
    matches(r, f, slot); requireFact(f.isolated && f.activeMutationCount === 0);
  }, 'committed'),
  verifyStagedSlot: define(slot, { ...slot, completeRead: boolean, formatValid: boolean }, (r, f) => {
    matches(r, f, slot); requireFact(f.completeRead && f.formatValid);
  }),
  activateStagedSlot: define({ ...slot, transactionId: id, ownerEpoch: positive },
    { ...slot, transactionId: id, ownerEpoch: positive, firstActiveWriteAt: nullable(uint), pointerSwitchedAt: nullable(uint), executionCommitBoundary: uint }, (r, f) => {
      matches(r, f, { ...slot, transactionId: id, ownerEpoch: positive });
      requireFact(f.firstActiveWriteAt !== null || f.pointerSwitchedAt !== null);
      requireFact(f.executionCommitBoundary === Math.min(f.firstActiveWriteAt ?? Infinity, f.pointerSwitchedAt ?? Infinity));
    }, 'committed'),
  cleanInactiveSlot: define(slot, { ...slot, activeSlotTouched: boolean, deletionConfirmed: boolean }, (r, f) => {
    matches(r, f, slot); requireFact(!f.activeSlotTouched && f.deletionConfirmed);
  }, 'committed'),
  launchAndHealthCheck: define({ transactionId: id, expectedActiveIdentitySha256: hash, scopeSha256: hash, originalHealthStartedAt: uint },
    { transactionId: id, activeIdentitySha256: hash, scopeSha256: hash, healthStartedAt: uint, finishedAt: uint,
      attempts: positive, attemptDurationsMs: list(uint), requiredMigrationsComplete: boolean, completeDataConsistent: boolean, localReady: boolean, storageOpen: boolean,
      localComponentsReady: boolean, fatalOrLoop: boolean }, (r, f) => {
      requireFact(f.transactionId === r.transactionId && f.activeIdentitySha256 === r.expectedActiveIdentitySha256
        && f.scopeSha256 === r.scopeSha256 && f.healthStartedAt === r.originalHealthStartedAt
        && f.finishedAt >= f.healthStartedAt && f.finishedAt - f.healthStartedAt <= 300000
        && f.attempts <= 2 && f.attemptDurationsMs.length === f.attempts && f.attemptDurationsMs.every(ms => ms <= 120000)
        && f.attemptDurationsMs.reduce((sum, ms) => sum + ms, 0) <= f.finishedAt - f.healthStartedAt
        && f.requiredMigrationsComplete && f.completeDataConsistent && f.localReady && f.storageOpen && f.localComponentsReady && !f.fatalOrLoop);
    }),
};
