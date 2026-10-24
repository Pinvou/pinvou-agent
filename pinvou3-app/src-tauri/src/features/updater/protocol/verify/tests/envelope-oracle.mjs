// Public test evidence only. Fresh private keys remain in this process and are
// never exported, written or included in the returned fixtures.
import { readFileSync } from 'node:fs';
import { generateKeyPairSync } from 'node:crypto';
import { pathToFileURL } from 'node:url';

const base = pathToFileURL(process.argv[1]);
const { canonicalize } = await import(new URL('./canonical-json.mjs', base));
const { sha256 } = await import(new URL('./digests.mjs', base));
const { publicKeyDescriptor, signEnvelope, verifyEnvelope, verifyRootSuccessor,
  validateTrustedRoot } = await import(new URL('./signatures.mjs', base));
const inventory = JSON.parse(readFileSync(new URL('../vectors/crypto/envelopes.json', base)));
const { createFixtureSet } = await import(new URL('./tests/fixtures.mjs', base));
const clone = structuredClone;
const rows = [];
const original = (name) => {
  const vector = inventory.vectors.find((v) => v.name === name);
  return { vector, envelope: JSON.parse(Buffer.from(vector.envelopeBase64Url, 'base64url')) };
};
const context = (signed) => ({ role: signed.role, product: signed.product,
  component: signed.component, scope: clone(signed.scope) });
function append(group, name, bytes, options, code = null, rustCode = code) {
  validateTrustedRoot(options.trustedRoot);
  let actual = null;
  try { verifyEnvelope(bytes, options); } catch (error) {
    if (typeof error.code !== 'string') throw error;
    actual = error.code;
  }
  if (actual !== code) throw new Error(`${group}/${name}: expected ${code}, received ${actual}`);
  rows.push({ group, name, bytes: Buffer.from(bytes).toString('base64url'),
    ...options, accepted: code === null, rustCode });
}
function synthetic(name, threshold = 1) {
  const { vector, envelope } = original(name);
  if (name === 'download') {
    // Use a new complete credential body; never attach fields to an old signature.
    envelope.signed = createFixtureSet({ clock: vector.now }).claims.download;
  }
  const pairs = Array.from({ length: 3 }, () => generateKeyPairSync('ed25519'))
    .map((pair) => ({ ...pair, descriptor: publicKeyDescriptor(pair.publicKey) }))
    .sort((a, b) => a.descriptor.keyId.localeCompare(b.descriptor.keyId));
  const trustedRoot = clone(inventory.trustedRoot);
  trustedRoot.keys.push(...pairs.map((pair) => pair.descriptor));
  const policy = trustedRoot.roles.find((p) => p.role === envelope.signed.role);
  policy.keyIds = pairs.map((pair) => pair.descriptor.keyId);
  policy.threshold = threshold;
  if (Object.hasOwn(envelope.signed, 'signingKeyId')) {
    envelope.signed.signingKeyId = pairs[0].descriptor.keyId;
  }
  const options = { trustedRoot, expected: context(envelope.signed), now: vector.now };
  return { signed: envelope.signed, pairs, options };
}
const signWith = (signed, pairs) => Buffer.from(signEnvelope(signed, pairs.map((p) => p.privateKey)));

for (const vector of inventory.vectors) {
  append('fixed', vector.name, Buffer.from(vector.envelopeBase64Url, 'base64url'),
    { trustedRoot: inventory.trustedRoot, expected: vector.expectedContext, now: vector.now },
    vector.name === 'download' ? 'SCHEMA_INVALID' : null,
    vector.name === 'download' ? 'ENVELOPE_SCHEMA_INVALID' : null);
}

const timestamp = synthetic('timestamp', 2);
const two = signWith(timestamp.signed, timestamp.pairs.slice(0, 2));
append('threshold', 'two-real-signatures', two, timestamp.options);
append('threshold', 'one-insufficient', signWith(timestamp.signed, timestamp.pairs.slice(0, 1)),
  timestamp.options, 'SIGNATURE_THRESHOLD_UNMET');
const single = synthetic('timestamp');
const valid = signWith(single.signed, single.pairs.slice(0, 1));
const both = signWith(single.signed, single.pairs.slice(0, 2));
append('threshold', 'extra-valid-after-threshold', both, single.options);
const invalidExtra = JSON.parse(both);
invalidExtra.signatures[1].signature = Buffer.alloc(64).toString('base64url');
append('threshold', 'invalid-extra-after-valid-prefix', Buffer.from(canonicalize(invalidExtra)),
  single.options, 'SIGNATURE_INVALID');
append('threshold', 'denied-extra-after-valid-prefix', both,
  { ...single.options, deniedKeyIds: [single.pairs[1].descriptor.keyId] }, 'SIGNING_KEY_DENIED');
append('threshold', 'denied-only-signer', valid,
  { ...single.options, deniedKeyIds: [single.pairs[0].descriptor.keyId] }, 'SIGNING_KEY_DENIED');
append('threshold', 'unrelated-deny-is-not-wildcard', valid,
  { ...single.options, deniedKeyIds: ['f'.repeat(64)] });
const unauthorized = clone(single.options);
unauthorized.trustedRoot.roles.find((p) => p.role === 'timestamp').keyIds =
  [single.pairs[1].descriptor.keyId];
append('threshold', 'declared-but-not-authorized-extra', both, unauthorized,
  'SIGNING_KEY_NOT_AUTHORIZED');
const unknown = JSON.parse(valid);
unknown.signatures.push({ keyId: 'f'.repeat(64), signature: Buffer.alloc(64).toString('base64url') });
unknown.signatures.sort((a, b) => a.keyId.localeCompare(b.keyId));
append('threshold', 'unknown-extra', Buffer.from(canonicalize(unknown)), single.options,
  'SIGNING_KEY_NOT_AUTHORIZED');
const changedBody = JSON.parse(valid);
changedBody.signed.version++;
append('threshold', 'body-tamper-with-original-signature', Buffer.from(canonicalize(changedBody)),
  single.options, 'SIGNATURE_INVALID');

const credential = synthetic('download');
const download = signWith(credential.signed, credential.pairs.slice(0, 1));
append('credential', 'real-declared-signing-key', download, credential.options);
const keyBinding = clone(credential.signed);
keyBinding.signingKeyId = credential.pairs[1].descriptor.keyId;
append('credential', 'real-signature-wrong-declared-key',
  signWith(keyBinding, credential.pairs.slice(0, 1)), credential.options,
  'CREDENTIAL_KEY_BINDING_INVALID');
append('credential', 'high-water-argument-ignored', download,
  { ...credential.options, highWater: { unrelated: true } });

for (const [name, expected] of [
  ['product', { ...credential.options.expected, product: 'another-product' }],
  ['component', { ...credential.options.expected, component: 'another-component' }],
  ['complete-scope', { ...credential.options.expected,
    scope: { ...credential.options.expected.scope, channelRevision: 2 } }],
  ['installation-scope', { ...credential.options.expected,
    scope: { ...credential.options.expected.scope, installationScopeId: 'other-installation' } }],
  ['missing-scope', Object.fromEntries(Object.entries(credential.options.expected)
    .filter(([key]) => key !== 'scope'))],
]) {
  append('context', name, download, { ...credential.options, expected },
    name === 'missing-scope' ? 'EXPECTED_CONTEXT_REQUIRED' : 'CONTEXT_MISMATCH');
}
for (const [name, mutate] of [
  ['missing-policy', (root) => { root.roles = root.roles.filter((p) => p.role !== 'download'); }],
  ['null-channel-not-wildcard', (root) => { root.roles.find((p) => p.role === 'download').channel = null; }],
  ['null-target-not-wildcard', (root) => { root.roles.find((p) => p.role === 'download').targetKey = null; }],
]) {
  const options = clone(credential.options);
  mutate(options.trustedRoot);
  append('context', name, download, options, 'ROLE_NOT_AUTHORIZED', 'ROLE_POLICY_NOT_DECLARED');
}
const wrongProduct = clone(single.options);
wrongProduct.trustedRoot.product = 'another-product';
append('context', 'root-product', valid, wrongProduct, 'ROOT_PRODUCT_MISMATCH', 'ROLE_PRODUCT_MISMATCH');

for (const [name, now] of [['body-expired', single.signed.expiresAt],
  ['body-before-issued', single.signed.issuedAt - 1]]) {
  const options = clone(single.options);
  // Keep the supplied Root fresh so body-time checks are reached.
  options.trustedRoot.issuedAt -= 1;
  options.trustedRoot.expiresAt -= 1;
  options.now = now;
  append('time', name, valid, options, 'TIME_WINDOW_CLOSED');
}
for (const [name, mutate] of [
  ['root-expired', (root, now) => { root.issuedAt = now - 100; root.expiresAt = now; }],
  ['root-future', (root, now) => { root.issuedAt = now + 1; root.expiresAt = now + 100; }],
  ['root-too-long', (root) => { root.expiresAt++; }],
]) {
  const options = clone(single.options);
  mutate(options.trustedRoot, options.now);
  append('time', name, valid, options,
    name === 'root-too-long' ? 'TIME_WINDOW_INVALID' : 'TIME_WINDOW_CLOSED');
}
const longBody = clone(single.signed);
longBody.expiresAt++;
append('time', 'real-signature-overlong-body', signWith(longBody, single.pairs.slice(0, 1)),
  single.options, 'TIME_WINDOW_INVALID');
append('time', 'credential-expired', download,
  { ...credential.options, now: credential.signed.exp }, 'TIME_WINDOW_CLOSED');

const highWater = { scope: clone(single.options.expected), version: single.signed.version,
  envelopeSha256: sha256(valid) };
append('high-water', 'exact-original-envelope', valid, { ...single.options, highWater });
append('high-water', 'rollback', valid, { ...single.options,
  highWater: { ...highWater, version: highWater.version + 1 } }, 'METADATA_ROLLBACK');
append('high-water', 'equivocation', both, { ...single.options, highWater }, 'METADATA_EQUIVOCATION');
append('high-water', 'scope', valid, { ...single.options,
  highWater: { ...highWater, scope: { ...highWater.scope, product: 'other' } } }, 'HIGH_WATER_SCOPE_INVALID');
const newer = clone(single.signed);
newer.version++;
append('high-water', 'newer-does-not-consult-old-hash', signWith(newer, single.pairs.slice(0, 1)),
  { ...single.options, highWater: { ...highWater, envelopeSha256: 'f'.repeat(64) } });

const pkg = synthetic('package');
const packageBytes = signWith(pkg.signed, pkg.pairs.slice(0, 1));
append('package', 'production-full', packageBytes, pkg.options);
append('package', 'package-high-water-refused', packageBytes, { ...pkg.options, highWater },
  'PACKAGE_HIGH_WATER_INVALID', 'HIGH_WATER_ROLE_INVALID');
const incremental = clone(pkg.signed);
incremental.incrementalPackages = [{ packageId: 'delta-2', size: 1, sha256: 'a'.repeat(64),
  baseVersion: '1.0.0', baseArtifactSha256: 'b'.repeat(64), resultArtifactSize: 123,
  resultArtifactSha256: 'a'.repeat(64), algorithm: 'test-delta', algorithmVersion: '1', format: 'test-delta' }];
const incrementalBytes = signWith(incremental, pkg.pairs.slice(0, 1));
append('package', 'incremental-default-production-refused', incrementalBytes, pkg.options,
  'INCREMENTAL_PRODUCTION_DISABLED');
append('package', 'same-signed-incremental-explicit-development', incrementalBytes,
  { ...pkg.options, production: false });

const rootCase = synthetic('root', 2);
// Replacing keys changes the Root body. Bind the supplied Root to those exact
// claims, then actually sign both the current body and its version+1 successor.
const currentRoot = clone(rootCase.options.trustedRoot);
const rootOptions = { ...rootCase.options, trustedRoot: currentRoot };
const rootBytes = signWith(currentRoot, rootCase.pairs.slice(0, 2));
append('root', 'fresh-supplied-root-two-real-signatures', rootBytes, rootOptions);
append('root', 'one-root-signature-insufficient', signWith(currentRoot, rootCase.pairs.slice(0, 1)),
  rootOptions, 'SIGNATURE_THRESHOLD_UNMET');
const successor = clone(currentRoot);
successor.version++;
const successorBytes = signWith(successor, rootCase.pairs.slice(0, 2));
verifyRootSuccessor(successorBytes, { trustedRoot: currentRoot, product: currentRoot.product,
  now: rootOptions.now });
append('root', 'otherwise-valid-signed-successor-needs-transition', successorBytes, rootOptions,
  'ROOT_TRANSITION_REQUIRED');

append('input', 'metadata-over-one-mib', Buffer.concat([valid, Buffer.alloc(1024 * 1024 + 1 - valid.length, 32)]),
  single.options, 'JSON_INPUT_LIMIT');
append('input', 'credential-over-64-kib', Buffer.concat([download, Buffer.alloc(65537 - download.length, 32)]),
  credential.options, 'JSON_INPUT_LIMIT');
append('input', 'whitespace-is-not-canonical', Buffer.concat([valid, Buffer.from(' ')]),
  single.options, 'ENVELOPE_NOT_CANONICAL');
append('input', 'duplicate-top-property', Buffer.from(`{"signed":null,${valid.toString().slice(1)}`),
  single.options, 'JSON_DUPLICATE_KEY');
for (const [name, value, code] of [
  ['oversized-string', 'a'.repeat(65537), 'JSON_STRING_LIMIT'],
  ['oversized-array', Array(4097).fill(0), 'JSON_MEMBER_LIMIT'],
  ['oversized-object', Object.fromEntries(Array.from({ length: 4097 }, (_, i) => [`k${i}`, 0])), 'JSON_MEMBER_LIMIT'],
  ['oversized-depth', Array.from({ length: 33 }).reduce((v) => [v], 0), 'JSON_DEPTH_LIMIT'],
]) {
  const envelope = JSON.parse(valid);
  envelope.signed.unrecognized = value;
  append('input', name, Buffer.from(JSON.stringify(envelope)), single.options, code);
}
const schemaInvalid = JSON.parse(valid);
schemaInvalid.signed.unrecognized = true;
append('input', 'closed-full-role-schema', Buffer.from(canonicalize(schemaInvalid)), single.options,
  'SCHEMA_INVALID', 'ENVELOPE_SCHEMA_INVALID');
const duplicateSigner = JSON.parse(valid);
duplicateSigner.signatures.push(clone(duplicateSigner.signatures[0]));
append('input', 'duplicate-signer-never-counted-twice', Buffer.from(canonicalize(duplicateSigner)),
  single.options, 'SIGNATURE_ORDER_INVALID');
const reversed = JSON.parse(both);
reversed.signatures.reverse();
append('input', 'unsorted-signers', Buffer.from(canonicalize(reversed)), single.options,
  'SIGNATURE_ORDER_INVALID');

// Measured adversarial A/R/S compatibility, not an additional point blacklist.
// Some small-order controls are accepted by the frozen provider; preserve the
// original profile and compare actual acceptance instead of inventing policy.
const fixedPackage = original('package');
const little = (n) => { const b = Buffer.alloc(32); for (let i = 0; i < 32; i++) {
  b[i] = Number(n & 255n); n >>= 8n; } return b; };
const p = (1n << 255n) - 19n;
const negativeZero = little(1n); negativeZero[31] |= 128;
const points = [['identity', little(1n)], ['zero', little(0n)], ['p', little(p)],
  ['p-plus-one', little(p + 1n)], ['negative-zero', negativeZero], ['noncurve-two', little(2n)]];
function measured(name, envelope, trustedRoot) {
  const bytes = Buffer.from(canonicalize(envelope));
  const options = { trustedRoot, expected: fixedPackage.vector.expectedContext,
    now: fixedPackage.vector.now };
  let code = null;
  try { verifyEnvelope(bytes, options); } catch (error) {
    if (error.code !== 'SIGNATURE_INVALID') throw error;
    code = error.code;
  }
  append('crypto-encoding', name, bytes, options, code);
}
for (const [aName, a] of points) for (const [rName, r] of points) {
  const trustedRoot = clone(inventory.trustedRoot), envelope = clone(fixedPackage.envelope);
  const old = envelope.signatures[0].keyId;
  const descriptor = { algorithm: 'ed25519', publicKey: a.toString('base64url') };
  const id = sha256(canonicalize(descriptor));
  Object.assign(trustedRoot.keys.find((key) => key.keyId === old), { keyId: id, ...descriptor });
  for (const policy of trustedRoot.roles) policy.keyIds = policy.keyIds.map((key) => key === old ? id : key);
  envelope.signatures[0] = { keyId: id, signature: Buffer.concat([r, Buffer.alloc(32)]).toString('base64url') };
  measured(`A-${aName}-R-${rName}-S-zero`, envelope, trustedRoot);
}
const originalSignature = Buffer.from(fixedPackage.envelope.signatures[0].signature, 'base64url');
const l = (1n << 252n) + 27742317777372353535851937790883648493n;
const originalS = originalSignature.subarray(32).reduceRight((n, byte) => (n << 8n) + BigInt(byte), 0n);
for (const [name, s] of [['L', little(l)], ['S-plus-L', little(originalS + l)], ['high-bits', Buffer.alloc(32, 255)]]) {
  const envelope = clone(fixedPackage.envelope);
  envelope.signatures[0].signature = Buffer.concat([originalSignature.subarray(0, 32), s]).toString('base64url');
  measured(`scalar-${name}`, envelope, inventory.trustedRoot);
}
process.stdout.write(JSON.stringify(rows));
