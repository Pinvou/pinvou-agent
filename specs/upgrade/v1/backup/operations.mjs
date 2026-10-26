import { define, id, hash, uint, boolean, requireFact } from '../platforms/values.mjs';

const binding = { backupId: id, preparationId: id, freezeEpoch: id, scopeSha256: hash, manifestSha256: hash };
const registered = { ...binding, recordId: id, keyReferenceId: id, originalCleanupDeadline: uint };
function matches(r, f, fields) { requireFact(Object.keys(fields).every(key => r[key] === f[key])); }
export const backupOperations = {
  registerBackup: define(registered, { ...registered, protectedReadback: boolean, bytesWritten: uint }, (r, f) => {
    matches(r, f, registered); requireFact(f.protectedReadback && f.bytesWritten === 0);
  }, 'committed'),
  createBackup: define(registered, { ...registered, registrationBeforeFirstByte: boolean, scopeComplete: boolean, durableBytes: boolean }, (r, f) => {
    matches(r, f, registered); requireFact(f.registrationBeforeFirstByte && f.scopeComplete && f.durableBytes);
  }, 'committed'),
  verifyBackup: define(registered, { ...registered, formatSupported: boolean, aeadFinal: boolean, completeReadback: boolean, manifestExact: boolean }, (r, f) => {
    matches(r, f, registered); requireFact(f.formatSupported && f.aeadFinal && f.completeReadback && f.manifestExact);
  }),
  exportBackup: define({ ...registered, exportId: id }, { ...registered, exportId: id, independentCopy: boolean, userDestinationConfirmed: boolean, auditRecorded: boolean }, (r, f) => {
    matches(r, f, { ...registered, exportId: id }); requireFact(f.independentCopy && f.userDestinationConfirmed && f.auditRecorded);
  }, 'committed'),
  deleteBackup: define(registered, { ...registered, containerAbsent: boolean, keyAbsent: boolean, cleanupReadback: boolean }, (r, f) => {
    matches(r, f, registered); requireFact(f.containerAbsent && f.keyAbsent && f.cleanupReadback);
  }, 'committed'),
};
