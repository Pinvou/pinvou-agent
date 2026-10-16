// Fresh private keys stay in this process; returned fixtures contain public evidence only.
import { readFileSync } from 'node:fs';
import { generateKeyPairSync } from 'node:crypto';
import { pathToFileURL } from 'node:url';
const base = pathToFileURL(process.argv[1]);
const { canonicalize } = await import(new URL('./canonical-json.mjs',base));
const { publicKeyDescriptor, signEnvelope, verifyArchivedPackage, verifyArchivedRelease, verifyEnvelope } = await import(new URL('./signatures.mjs',base));
const inventory = JSON.parse(readFileSync(new URL('../vectors/crypto/envelopes.json',base)));
const clone = structuredClone;
const rows = [];
const DAY = 86_400_000;
const context = (signed) => ({
  role: signed.role, product: signed.product,
  component: signed.component, scope: clone(signed.scope),
});
function pair() {
  const keys = generateKeyPairSync('ed25519');
  return { ...keys, descriptor: publicKeyDescriptor(keys.publicKey) };
}
function unique(pairs) {
  return [...new Map(pairs.map((item) => [item.descriptor.keyId, item])).values()];
}
const signedBytes = (signed, pairs) => Buffer.from(
  signEnvelope(signed, unique(pairs).map((item) => item.privateKey)),
);
function rotation(overlap = false) {
  const oldPairs = [pair(), pair(), pair()];
  const fresh = [pair(), pair(), pair()];
  const meta = [pair(), pair()];
  const nextPairs = overlap ? [oldPairs[0], fresh[1], fresh[2]] : fresh;
  const old = clone(inventory.trustedRoot);
  const next = clone(old);
  const now = old.issuedAt + 1000;
  for (const [root, pairs] of [[old, oldPairs], [next, nextPairs]]) {
    root.keys = unique([...pairs, ...meta]).map((item) => item.descriptor);
    for (const policy of root.roles) {
      policy.keyIds = (policy.role === 'root' ? pairs : [meta[0]])
        .map((item) => item.descriptor.keyId);
      policy.threshold = policy.role === 'root' ? 2 : 1;
    }
  }
  next.version = old.version + 1;
  next.issuedAt = now;
  next.expiresAt = now + 365 * DAY;
  return { old, next, now, oldPairs, nextPairs, meta,
    signers: unique([...oldPairs.slice(0, 2), ...nextPairs.slice(0, 2)]) };
}
function active(rotationCase, name = 'timestamp', mutate = () => {}) {
  const vector = inventory.vectors.find((item) => item.name === name);
  const signed = JSON.parse(Buffer.from(vector.envelopeBase64Url, 'base64url')).signed;
  if (Object.hasOwn(signed, 'signingKeyId')) {
    signed.signingKeyId = rotationCase.meta[0].descriptor.keyId;
  }
  mutate(signed);
  return { bytes: signedBytes(signed, [rotationCase.meta[0]]), expected: context(signed) };
}
// Public historical-identity test fixtures only; no accepted-data deletion authority.
function archivedCase(operation) {
  const seed = rotation();
  const entry = active(seed, operation);
  return { ...seed, operation, signed: JSON.parse(entry.bytes).signed,
    archivedRoot: seed.old, expected: entry.expected };
}
function appendArchive(group, name, scenario, {
  signed = scenario.signed, signers = [scenario.meta[0]],
  bytes = signedBytes(signed, signers), archivedRoot = scenario.archivedRoot,
  expected = context(signed), now = scenario.now, ordinaryCode = undefined,
} = {}, code = null, rustCode = code) {
  const options = { archivedRoot, expected };
  if (scenario.operation === 'package') options.now = now;
  let actual = null;
  try {
    (scenario.operation === 'package' ? verifyArchivedPackage : verifyArchivedRelease)(bytes, options);
  } catch (error) {
    if (typeof error.code !== 'string') throw error;
    actual = error.code;
  }
  if (actual !== code) throw new Error(`${group}/${name}: expected ${code}, actual ${actual}`);
  if (ordinaryCode !== undefined) {
    let ordinaryActual = null;
    try { verifyEnvelope(bytes, { trustedRoot: archivedRoot, expected, now }); }
    catch (error) {
      if (typeof error.code !== 'string') throw error;
      ordinaryActual = error.code;
    }
    if (ordinaryActual !== ordinaryCode) {
      throw new Error(`${group}/${name}: ordinary expected ${ordinaryCode}, actual ${ordinaryActual}`);
    }
  }
  rows.push({ group, name, operation: scenario.operation, archivedRoot, expected, now,
    bytes: Buffer.from(bytes).toString('base64url'), accepted: actual === null, rustCode,
    ordinaryCode });
}
const pkg = archivedCase('package');
appendArchive('package', 'original-production-package', pkg);
appendArchive('package', 'expired-root-still-identifies-package', pkg,
  { now: pkg.archivedRoot.expiresAt + 1, ordinaryCode: 'TIME_WINDOW_CLOSED' });
appendArchive('package', 'root-expiry-equality-is-history', pkg,
  { now: pkg.archivedRoot.expiresAt, ordinaryCode: 'TIME_WINDOW_CLOSED' });
appendArchive('package', 'future-root-still-refuses-package', pkg,
  { now: pkg.archivedRoot.issuedAt - 1 }, 'TIME_WINDOW_INVALID');
const invalidPackageRoot = archivedCase('package');
invalidPackageRoot.archivedRoot.expiresAt = invalidPackageRoot.archivedRoot.issuedAt;
appendArchive('package', 'package-root-original-window-must-be-positive', invalidPackageRoot,
  {}, 'TIME_WINDOW_INVALID');
const incremental = clone(pkg.signed);
incremental.incrementalPackages = [{
  packageId: 'delta-2', size: 1, sha256: 'a'.repeat(64),
  baseVersion: '1.0.0', baseArtifactSha256: 'b'.repeat(64), resultArtifactSize: 123,
  resultArtifactSha256: 'a'.repeat(64), algorithm: 'test-delta',
  algorithmVersion: '1', format: 'test-delta',
}];
appendArchive('package', 'archive-still-requires-production-full-package', pkg,
  { signed: incremental }, 'INCREMENTAL_PRODUCTION_DISABLED');
const release = archivedCase('release');
appendArchive('release', 'original-release-identity', release);
appendArchive('release', 'expired-root-and-release-are-original-identities', release,
  { now: release.archivedRoot.expiresAt + DAY, ordinaryCode: 'TIME_WINDOW_CLOSED' });
const futureRelease = archivedCase('release');
futureRelease.archivedRoot.issuedAt += 1000 * DAY;
futureRelease.archivedRoot.expiresAt += 1000 * DAY;
futureRelease.signed.issuedAt += 1500 * DAY;
futureRelease.signed.expiresAt += 1500 * DAY;
appendArchive('release', 'release-has-no-current-now-restriction', futureRelease,
  { ordinaryCode: 'TIME_WINDOW_CLOSED' });
const reversedTimes = archivedCase('release');
reversedTimes.archivedRoot.issuedAt = reversedTimes.signed.issuedAt + DAY;
reversedTimes.archivedRoot.expiresAt = reversedTimes.archivedRoot.issuedAt + DAY;
appendArchive('release', 'root-issued-later-than-release-is-not-extra-policy', reversedTimes);
const afterRoot = archivedCase('release');
afterRoot.signed.issuedAt = afterRoot.archivedRoot.expiresAt + DAY;
afterRoot.signed.expiresAt = afterRoot.signed.issuedAt + DAY;
appendArchive('release', 'release-issued-after-root-expiry-is-identity-only', afterRoot);
const invalidReleaseWindow = clone(release.signed);
invalidReleaseWindow.expiresAt = invalidReleaseWindow.issuedAt;
appendArchive('release', 'release-original-window-must-be-positive', release,
  { signed: invalidReleaseWindow }, 'TIME_WINDOW_INVALID');
const overlongRelease = clone(release.signed);
overlongRelease.expiresAt = overlongRelease.issuedAt + 91 * DAY;
appendArchive('release', 'release-original-window-remains-bounded', release,
  { signed: overlongRelease }, 'TIME_WINDOW_INVALID');
const invalidReleaseRoot = archivedCase('release');
invalidReleaseRoot.archivedRoot.expiresAt = invalidReleaseRoot.archivedRoot.issuedAt + 366 * DAY;
appendArchive('release', 'release-root-original-window-remains-bounded', invalidReleaseRoot,
  {}, 'TIME_WINDOW_INVALID');
for (const scenario of [pkg, release]) {
  const thresholdRoot = clone(scenario.archivedRoot);
  const policy = thresholdRoot.roles.find((item) => item.role === scenario.operation);
  policy.keyIds = scenario.meta.map((item) => item.descriptor.keyId);
  policy.threshold = 2;
  appendArchive('signatures', `${scenario.operation}-two-actual-signatures`, scenario,
    { archivedRoot: thresholdRoot, signers: scenario.meta });
  appendArchive('signatures', `${scenario.operation}-one-is-insufficient`, scenario,
    { archivedRoot: thresholdRoot }, 'SIGNATURE_THRESHOLD_UNMET');
  const invalid = JSON.parse(signedBytes(scenario.signed, [scenario.meta[0]]));
  invalid.signatures[0].signature = Buffer.alloc(64).toString('base64url');
  appendArchive('signatures', `${scenario.operation}-invalid-actual-signature`, scenario,
    { bytes: Buffer.from(canonicalize(invalid)) }, 'SIGNATURE_INVALID');
  appendArchive('signatures', `${scenario.operation}-descriptor-outside-role`, scenario,
    { signers: [scenario.meta[0], scenario.oldPairs[0]] }, 'SIGNING_KEY_NOT_AUTHORIZED');
}
for (const scenario of [pkg, release]) {
  appendArchive('context', `${scenario.operation}-wrong-required-role`, scenario,
    { expected: { ...scenario.expected, role: 'timestamp' } }, 'ARCHIVE_ROLE_INVALID');
  appendArchive('context', `${scenario.operation}-wrong-independent-product`, scenario,
    { expected: { ...scenario.expected, product: 'another-product' } }, 'CONTEXT_MISMATCH');
  appendArchive('context', `${scenario.operation}-wrong-independent-component`, scenario,
    { expected: { ...scenario.expected, component: 'another-component' } }, 'CONTEXT_MISMATCH');
  appendArchive('context', `${scenario.operation}-wrong-complete-scope`, scenario,
    { expected: { ...scenario.expected, scope: { ...scenario.expected.scope, channelRevision: 2 } } },
    'CONTEXT_MISMATCH');
  const bytes = signedBytes(scenario.signed, [scenario.meta[0]]);
  appendArchive('input', `${scenario.operation}-over-original-one-mib`, scenario,
    { bytes: Buffer.concat([bytes, Buffer.alloc(1048577 - bytes.length, 32)]) }, 'JSON_INPUT_LIMIT');
  appendArchive('input', `${scenario.operation}-original-canonical-bytes`, scenario,
    { bytes: Buffer.concat([bytes, Buffer.from(' ')]) }, 'ENVELOPE_NOT_CANONICAL');
  const invalidSchema = JSON.parse(bytes);
  invalidSchema.signed.unrecognized = true;
  appendArchive('input', `${scenario.operation}-closed-full-schema`, scenario,
    { bytes: Buffer.from(canonicalize(invalidSchema)) }, 'SCHEMA_INVALID', 'ENVELOPE_SCHEMA_INVALID');
}
console.log(JSON.stringify(rows));
