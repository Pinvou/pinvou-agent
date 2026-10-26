import { define, id, hash, uint, boolean, oneOf, nullable, requireFact } from './values.mjs';

const version = value => typeof value === 'string' && /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.test(value)
  && value.split('.').every(part => Number.isSafeInteger(Number(part)));
const artifact = { packageId: id, size: uint, sha256: hash, metadataSha256: hash };
export const identityOperations = {
  detectInstallation: define({ expectedScopeId: id }, { installed: boolean, scopeId: id, version: nullable(version), identitySha256: nullable(hash) }, (r, f, c) => {
    requireFact(r.expectedScopeId === c.installationScopeId && f.scopeId === r.expectedScopeId);
    requireFact(f.installed ? f.version !== null && f.identitySha256 !== null : f.version === null && f.identitySha256 === null);
  }),
  normalizeVersion: define({ productVersion: version }, { productVersion: version, fileVersion: nullable(value => typeof value === 'string') }, (r, f, c) => {
    requireFact(f.productVersion === r.productVersion);
    requireFact(c.os === 'windows' ? f.fileVersion === `${r.productVersion}.0`
      && r.productVersion.split('.').every(part => Number(part) <= 65535) : f.fileVersion === null);
  }),
  verifyArtifact: define(artifact, { ...artifact, completeRead: boolean, formatValid: boolean }, (r, f) => {
    requireFact(Object.keys(r).every(key => r[key] === f[key]) && f.completeRead && f.formatValid);
  }),
  verifyExecutableIdentity: define({ ...artifact, generation: id, role: oneOf('installer', 'helper', 'launcher', 'host') },
    { ...artifact, generation: id, role: oneOf('installer', 'helper', 'launcher', 'host'), retainedObject: boolean, immediateRecheck: boolean }, (r, f) => {
      requireFact(Object.keys(r).every(key => r[key] === f[key]) && f.retainedObject && f.immediateRecheck);
    }),
  acquirePrivilege: define({ requiredRole: oneOf('helper', 'installer') }, { role: oneOf('helper', 'installer'), verified: boolean, consentObserved: boolean }, (r, f, c) => {
    requireFact(c.upgradeType !== 'silent' && f.role === r.requiredRole && f.verified && f.consentObserved);
  }),
  verifyExistingHelperPrivilege: define({ helperIdentitySha256: hash }, { helperIdentitySha256: hash, sufficient: boolean, interactivePrompt: boolean }, (r, f) => {
    requireFact(f.helperIdentitySha256 === r.helperIdentitySha256 && f.sufficient && !f.interactivePrompt);
  }),
};
