import { define, shape, id, hash, uint, positive, boolean, oneOf, nullable, requireFact } from './values.mjs';

const kinds = oneOf('coordination', 'backup', 'validation', 'preinstall', 'install', 'activate', 'health');
const window = shape({ windowId: id, stage: kinds, firstStartedAt: uint, originalDurationMs: positive,
  durationMs: positive, originalDeadline: uint, sessionDeadline: nullable(uint), effectiveDeadline: uint, spentMs: uint, revision: positive });
function validWindow(w) {
  requireFact(w.durationMs <= w.originalDurationMs && Number.isSafeInteger(w.firstStartedAt + w.originalDurationMs)
    && Number.isSafeInteger(w.firstStartedAt + w.spentMs));
  requireFact(w.originalDeadline === w.firstStartedAt + w.originalDurationMs);
  requireFact(w.effectiveDeadline === Math.min(w.firstStartedAt + w.durationMs, w.sessionDeadline ?? Infinity));
  if (w.stage === 'validation') requireFact(w.originalDurationMs <= 1800000);
  if (['preinstall', 'install', 'activate'].includes(w.stage)) requireFact(w.originalDurationMs <= 7200000);
  if (w.stage === 'health') requireFact(w.originalDurationMs <= 300000);
}
export const timeOperations = {
  TrustedDeadlineProvider: define({ expectedBootId: id, policySha256: hash },
    { bootId: id, policySha256: hash, lowerUtc: uint, upperUtc: uint, continuityVerified: boolean, cleanupBarrier: boolean }, (r, f) => {
      requireFact(f.bootId === r.expectedBootId && f.policySha256 === r.policySha256
        && f.lowerUtc <= f.upperUtc && f.continuityVerified && !f.cleanupBarrier);
    }),
  persistStageBudget: define({ previous: nullable(window), proposed: window }, { retained: window, readbackVerified: boolean }, (r, f) => {
    validWindow(r.proposed); validWindow(f.retained);
    requireFact(f.readbackVerified && Object.keys(r.proposed).every(key => r.proposed[key] === f.retained[key]));
    if (r.previous !== null) {
      validWindow(r.previous);
      for (const key of ['windowId', 'stage', 'firstStartedAt', 'originalDurationMs', 'originalDeadline'])
        requireFact(r.previous[key] === r.proposed[key]);
      requireFact(r.proposed.spentMs >= r.previous.spentMs && r.proposed.revision === r.previous.revision + 1
        && r.proposed.durationMs <= r.previous.durationMs);
      requireFact(r.previous.sessionDeadline === null ? r.proposed.sessionDeadline === null
        : r.proposed.sessionDeadline !== null && r.proposed.sessionDeadline <= r.previous.sessionDeadline);
    } else requireFact(r.proposed.revision === 1);
    if (['coordination', 'backup'].includes(r.proposed.stage)) requireFact(r.proposed.sessionDeadline !== null);
  }, 'committed'),
};
