import { define, shape, id, hash, uint, positive, boolean, ids, list, requireFact, sameSet } from './values.mjs';

const owner = { preparationId: id, ownerEpoch: positive, scopeSha256: hash };
const held = { ...owner, exclusive: boolean, durable: boolean };
const writer = shape({ writerId: id, epoch: id, saved: boolean, stopped: boolean, consistent: boolean });
function matches(r, f) { requireFact(Object.keys(owner).every(key => r[key] === f[key])); }
function heldMatches(r, f) { matches(r, f); requireFact(f.exclusive && f.durable); }
export const ownershipOperations = {
  acquireGlobalLock: define(owner, held, heldMatches, 'committed'),
  persistPreparation: define({ ...owner, expectedRevision: uint, originalStartedAt: uint },
    { ...owner, revision: positive, originalStartedAt: uint, readbackVerified: boolean }, (r, f) => {
      matches(r, f); requireFact(f.revision === r.expectedRevision + 1 && f.originalStartedAt === r.originalStartedAt && f.readbackVerified);
    }, 'committed'),
  transferOwnership: define({ ...owner, nextEpoch: positive }, { ...held, previousOwnerInvalidated: boolean }, (r, f) => {
    requireFact(r.nextEpoch === r.ownerEpoch + 1 && f.ownerEpoch === r.nextEpoch
      && f.preparationId === r.preparationId && f.scopeSha256 === r.scopeSha256 && f.exclusive && f.durable && f.previousOwnerInvalidated);
  }, 'committed'),
  recoverOwnership: define(owner, held, heldMatches, 'committed'),
  fenceOwner: define({ ...owner, staleEpoch: positive }, { ...owner, rejectedEpoch: positive, staleActionsRejected: boolean }, (r, f) => {
    matches(r, f); requireFact(r.staleEpoch < r.ownerEpoch && f.rejectedEpoch === r.staleEpoch && f.staleActionsRejected);
  }, 'committed'),
  coordinateWriters: define({ ...owner, expectedWriterIds: ids }, { ...owner, writerIds: ids, inventoryComplete: boolean, allSavable: boolean }, (r, f) => {
    matches(r, f); requireFact(f.inventoryComplete && f.allSavable && sameSet(r.expectedWriterIds, f.writerIds));
  }),
  freezeAffectedScopes: define({ ...owner, freezeEpoch: id, expectedWriterIds: ids },
    { ...owner, freezeEpoch: id, writers: list(writer), admissionFenced: boolean, readbackVerified: boolean }, (r, f) => {
      matches(r, f); requireFact(f.freezeEpoch === r.freezeEpoch && f.admissionFenced && f.readbackVerified);
      const names = f.writers.map(item => item.writerId);
      requireFact(new Set(names).size === names.length && sameSet(r.expectedWriterIds, names)
        && f.writers.every(item => item.epoch === r.freezeEpoch && item.saved && item.stopped && item.consistent));
    }, 'committed'),
  verifyDataConsistency: define({ ...owner, freezeEpoch: id }, { ...owner, freezeEpoch: id, completeScopeRead: boolean, consistent: boolean }, (r, f) => {
    matches(r, f); requireFact(f.freezeEpoch === r.freezeEpoch && f.completeScopeRead && f.consistent);
  }),
};
