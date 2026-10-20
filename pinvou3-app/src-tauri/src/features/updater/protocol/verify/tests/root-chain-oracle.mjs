// Fresh private keys stay in this process; returned fixtures contain public evidence only.
import { readFileSync } from 'node:fs';
import { generateKeyPairSync } from 'node:crypto';
import { pathToFileURL } from 'node:url';
const base = pathToFileURL(process.argv[1]);
const { canonicalize } = await import(new URL('./canonical-json.mjs',base));
const { publicKeyDescriptor, signEnvelope, verifyRootSuccessor, verifyRootChain } = await import(new URL('./signatures.mjs',base));
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
// Public test fixtures only. Every emitted chain runs the complete frozen reference.
function chainScenario(count = 3, reuseKeys = false) {
  const seed = rotation();
  const initial = seed.old;
  if (reuseKeys) {
    initial.roles = initial.roles.filter((policy) => policy.role === 'root');
    initial.keys = seed.oldPairs.map((item) => item.descriptor);
  }
  const roots = [], signers = [], successors = [];
  let previousPairs = seed.oldPairs;
  for (let index = 0; index < count; index++) {
    const nextPairs = reuseKeys ? seed.oldPairs : [pair(), pair(), pair()];
    const next = clone(initial);
    next.version = initial.version + index + 1;
    next.issuedAt = seed.now - 1;
    next.expiresAt = seed.now + DAY;
    next.keys = unique([...nextPairs, ...(reuseKeys ? [] : seed.meta)])
      .map((item) => item.descriptor);
    next.roles.find((policy) => policy.role === 'root').keyIds = nextPairs
      .map((item) => item.descriptor.keyId);
    const signingPairs = unique([...previousPairs.slice(0, 2), ...nextPairs.slice(0, 2)]);
    roots.push(next);
    signers.push(signingPairs);
    successors.push(signedBytes(next, signingPairs));
    previousPairs = nextPairs;
  }
  return { ...seed, initial, roots, signers, successors };
}
function resign(scenario, index) {
  scenario.successors[index] = signedBytes(scenario.roots[index], scenario.signers[index]);
}
function appendChain(group, name, scenario, {
  successors = scenario.successors, product = scenario.initial.product,
  now = scenario.now, activeEnvelopes = [], repeatSteps = 1,
} = {}, code = null, rustCode = code) {
  const expanded = Array.from({ length: repeatSteps }, () => successors).flat();
  let actual = null;
  try {
    verifyRootChain(expanded, { trustedRoot: scenario.initial, product, now, activeEnvelopes });
  } catch (error) {
    if (typeof error.code !== 'string') throw error;
    actual = error.code;
  }
  if (actual !== code) throw new Error(`${group}/${name}: expected ${code}, actual ${actual}`);
  rows.push({
    group, name, trustedRoot: scenario.initial,
    expected: context({ ...scenario.initial, product }), now, repeatSteps,
    successors: successors.map((bytes) => Buffer.from(bytes).toString('base64url')),
    activeEnvelopes: activeEnvelopes.map((entry) => ({ ...entry,
      bytes: Buffer.from(entry.bytes).toString('base64url') })),
    accepted: actual === null, rustCode,
  });
}
const normal = chainScenario();
appendChain('continuity', 'three-real-distinct-dual-threshold-steps', normal);
appendChain('bounds', 'one-real-step', normal, { successors: normal.successors.slice(0, 1) });
appendChain('bounds', 'empty-chain', normal, { successors: [] }, 'ROOT_CHAIN_INVALID');
appendChain('bounds', '4097-count-refuses-before-parse', normal,
  { successors: [Buffer.from('invalid')], repeatSteps: 4097 }, 'ROOT_CHAIN_INVALID');
const upper = chainScenario(4096, true);
appendChain('bounds', '4096-genuine-consecutive-resigned-steps', upper);
const expired = chainScenario();
expired.initial.expiresAt = expired.now - 1;
for (let index = 0; index < 2; index++) {
  expired.roots[index].issuedAt = expired.now - DAY;
  expired.roots[index].expiresAt = expired.now - 1;
  resign(expired, index);
}
appendChain('windows', 'expired-old-and-intermediates-fresh-final', expired);
const expiredFinal = chainScenario();
expiredFinal.roots.at(-1).issuedAt = expiredFinal.now - DAY;
expiredFinal.roots.at(-1).expiresAt = expiredFinal.now;
resign(expiredFinal, 2);
appendChain('windows', 'expired-final-refuses', expiredFinal, {}, 'TIME_WINDOW_CLOSED');
const futureIntermediate = chainScenario();
futureIntermediate.roots[1].issuedAt = futureIntermediate.now + 1;
resign(futureIntermediate, 1);
appendChain('windows', 'future-intermediate-refuses', futureIntermediate, {}, 'TIME_WINDOW_INVALID');
const longIntermediate = chainScenario();
longIntermediate.roots[1].expiresAt = longIntermediate.roots[1].issuedAt + 366 * DAY;
resign(longIntermediate, 1);
appendChain('windows', 'overlong-intermediate-refuses', longIntermediate, {}, 'TIME_WINDOW_INVALID');
appendChain('continuity', 'skip-first-step', normal,
  { successors: normal.successors.slice(1) }, 'ROOT_CONTINUITY_INVALID');
appendChain('continuity', 'reverse-order', normal,
  { successors: [...normal.successors].reverse() }, 'ROOT_CONTINUITY_INVALID');
appendChain('continuity', 'repeat-version-after-valid-prefix', normal,
  { successors: [normal.successors[0], normal.successors[0]] }, 'ROOT_CONTINUITY_INVALID');
appendChain('continuity', 'independent-product-mismatch', normal,
  { product: 'another-product' }, 'CONTEXT_MISMATCH');
const restored = chainScenario();
restored.roots[0].roles.find((policy) => policy.role === 'timestamp').keyIds =
  [restored.meta[1].descriptor.keyId];
resign(restored, 0);
appendChain('active', 'intermediate-retirement-final-restoration', restored,
  { activeEnvelopes: [active(restored)] });
const retiredFinal = chainScenario();
retiredFinal.roots.at(-1).roles.find((policy) => policy.role === 'timestamp').keyIds =
  [retiredFinal.meta[1].descriptor.keyId];
resign(retiredFinal, 2);
appendChain('active', 'final-retirement-refuses', retiredFinal,
  { activeEnvelopes: [active(retiredFinal)] }, 'SIGNING_KEY_NOT_AUTHORIZED');
const badExtra = chainScenario();
const changed = JSON.parse(badExtra.successors[1]);
changed.signatures.push({ keyId: 'f'.repeat(64), signature: Buffer.alloc(64).toString('base64url') });
badExtra.successors[1] = Buffer.from(canonicalize(changed));
appendChain('signatures', 'bad-middle-extra-does-not-return-prefix', badExtra, {}, 'SIGNING_KEY_NOT_AUTHORIZED');
const nextOnly = chainScenario();
nextOnly.successors[1] = signedBytes(nextOnly.roots[1], nextOnly.signers[1].slice(2));
appendChain('signatures', 'middle-next-only-refuses', nextOnly, {}, 'SIGNATURE_THRESHOLD_UNMET');
const oldOnly = chainScenario();
oldOnly.successors[1] = signedBytes(oldOnly.roots[1], oldOnly.signers[1].slice(0, 2));
appendChain('signatures', 'middle-old-only-refuses', oldOnly, {}, 'SIGNATURE_THRESHOLD_UNMET');
const shared = chainScenario(3, true);
appendChain('signatures', 'shared-real-signers-count-each-policy', shared);
const futureOld = chainScenario();
futureOld.initial.issuedAt = futureOld.now + 1;
futureOld.initial.expiresAt = futureOld.now + DAY;
appendChain('windows', 'future-initial-root-refuses', futureOld, {}, 'TIME_WINDOW_INVALID');
const futureFinal = chainScenario();
futureFinal.roots.at(-1).issuedAt = futureFinal.now + 1;
resign(futureFinal, 2);
appendChain('windows', 'future-final-root-refuses', futureFinal, {}, 'TIME_WINDOW_CLOSED');
const nonmonotone = chainScenario();
nonmonotone.roots[0].issuedAt = nonmonotone.initial.issuedAt - 1;
nonmonotone.roots[0].expiresAt = nonmonotone.now - 1;
resign(nonmonotone, 0);
appendChain('windows', 'no-invented-relative-time-monotonicity', nonmonotone);
const finalBadActive = chainScenario();
const goodActive = active(finalBadActive);
const badActive = { ...goodActive, expected: { ...goodActive.expected, component: 'wrong-component' } };
appendChain('active', 'bad-final-active-after-valid-prefix', finalBadActive,
  { activeEnvelopes: [goodActive, badActive] }, 'CONTEXT_MISMATCH');
appendChain('active', 'explicit-empty-final-read-set', normal, { activeEnvelopes: [] });
appendChain('active', 'repeated-final-metadata-preserves-key', normal,
  { activeEnvelopes: [active(normal), active(normal)] });
const max = chainScenario(1, true);
max.initial.version = Number.MAX_SAFE_INTEGER - 1;
max.roots[0].version = Number.MAX_SAFE_INTEGER;
resign(max, 0);
appendChain('continuity', 'last-safe-final-root', max);
const tooLarge = chainScenario();
tooLarge.successors[0] = Buffer.concat([tooLarge.successors[0],
  Buffer.alloc(1048577 - tooLarge.successors[0].length, 32)]);
appendChain('input', 'intermediate-original-one-mib-profile', tooLarge, {}, 'JSON_INPUT_LIMIT');
const noncanonical = chainScenario();
noncanonical.successors[0] = Buffer.concat([noncanonical.successors[0], Buffer.from(' ')]);
appendChain('input', 'intermediate-original-canonical-bytes', noncanonical, {}, 'ENVELOPE_NOT_CANONICAL');
const duplicate = chainScenario();
duplicate.successors[1] = Buffer.from(`{"signed":null,${duplicate.successors[1].toString().slice(1)}`);
appendChain('input', 'middle-original-duplicate-rejection', duplicate, {}, 'JSON_DUPLICATE_KEY');
const badFinalSchema = chainScenario();
const badFinal = JSON.parse(badFinalSchema.successors[2]);
badFinal.signed.unrecognized = true;
badFinalSchema.successors[2] = Buffer.from(canonicalize(badFinal));
appendChain('input', 'final-full-closed-schema', badFinalSchema, {}, 'SCHEMA_INVALID', 'ENVELOPE_SCHEMA_INVALID');
console.log(JSON.stringify(rows));
