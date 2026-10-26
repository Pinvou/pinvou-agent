import { define, id, hash, uint, positive, boolean, oneOf, requireFact } from './values.mjs';

// Fixed owner partitions, not arbitrary paths, keys, commands or write closures.
const partition = { owner: oneOf('preparation', 'time', 'backup', 'execution', 'slot'), scopeId: id, recordId: id };
export const storageOperations = {
  readProtectedPartition: define(partition, { ...partition, revision: positive, contentSha256: hash, custodyVerified: boolean }, (r, f, c) => {
    requireFact(r.scopeId === c.installationScopeId && Object.keys(partition).every(key => r[key] === f[key]) && f.custodyVerified);
  }),
  commitProtectedPartition: define({ ...partition, expectedRevision: uint, contentSha256: hash },
    { ...partition, revision: positive, contentSha256: hash, precommitRecheck: boolean, committedReadback: boolean }, (r, f, c) => {
      requireFact(r.scopeId === c.installationScopeId && Object.keys(partition).every(key => r[key] === f[key])
        && f.revision === r.expectedRevision + 1 && f.contentSha256 === r.contentSha256 && f.precommitRecheck && f.committedReadback);
    }, 'committed'),
};
