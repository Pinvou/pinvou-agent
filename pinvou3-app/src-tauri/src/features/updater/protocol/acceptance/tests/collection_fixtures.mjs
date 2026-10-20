// Signed collection-boundary fixtures. Ed25519 private keys stay in this process.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

const reference = pathToFileURL(process.argv[2]);
const directory = process.argv[3];
const scenario = process.argv[4];
const { createFixtureSet, NOW } = await import(new URL('./tests/fixtures.mjs', reference));
const { signEnvelope, envelopeReference, verifyRootChain, verifyRootSuccessor } = await import(reference);
const { verifyPublicChain } = await import(new URL('./metadata-chain.mjs', reference));
const { DAY } = await import(new URL('./semantics.mjs', reference));
const f = createFixtureSet();
const root = structuredClone(f.metadata.root);
const limit = 1_048_576;
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const signed = bytes => JSON.parse(Buffer.from(bytes).toString('utf8')).signed;
const sign = body => signEnvelope(body, body.role === 'root' ? f.keys.slice(0, 2) : [f.keys[3]]);
const emit = (name, bytes) => {
  assert.ok(Buffer.byteLength(bytes) <= limit, 'every metadata object keeps its original byte bound');
  writeFileSync(join(directory, name), bytes);
  return name;
};
mkdirSync(directory);

function graph(name, version, releases, { trustedRoot = root, roots = [], now = NOW + 1000,
  packageBytes = f.bytes.package, channels = ['stable'], paths = false } = {}) {
  const targetBytes = channels.map((channel, index) => {
    const target = structuredClone(f.metadata.target);
    target.scope = { channel, targetKey: signed(packageBytes).scope.targetKey };
    target.version = version;
    target.baselineRelease = envelopeReference(releases[index]);
    target.ordinaryPaths = paths ? releases.filter((_, i) => i >= channels.length && i % channels.length === index)
      .map((bytes, i) => ({ deploymentId: `d${i}`, deploymentRevision: 1,
        approvalId: `a${i}`, approvalRevision: 1, release: envelopeReference(bytes) })) : [];
    return sign(target);
  });
  const members = [...targetBytes, ...releases, packageBytes];
  const snapshot = structuredClone(f.metadata.snapshot);
  snapshot.version = version;
  snapshot.entries = members.map(envelopeReference);
  const snapshotBytes = sign(snapshot);
  const timestamp = structuredClone(f.metadata.timestamp);
  timestamp.version = version;
  timestamp.snapshot = envelopeReference(snapshotBytes);
  const timestampBytes = sign(timestamp);
  // This independent reference verifies every signature, reference and graph,
  // including complete membership, before the real Rust CLI ever sees files.
  verifyPublicChain({ trustedRoot, product: 'pinvou', component: 'app', now,
    timestampBytes, snapshotBytes, members });
  const memberNames = members.map((bytes, i) => emit(`${name}-m${i}.json`, bytes));
  const bundle = { product: 'pinvou', component: 'app', suppliedNowMs: now, roots,
    timestamp: emit(`${name}-timestamp.json`, timestampBytes),
    snapshot: emit(`${name}-snapshot.json`, snapshotBytes), members: memberNames,
    deniedKeyIds: [], deniedByRole: {} };
  emit(`${name}.json`, Buffer.from(JSON.stringify(bundle)));
  return { bundle, members, timestampBytes, snapshotBytes, targetBytes };
}
function release(id, revision, packageBytes = f.bytes.package) {
  const body = structuredClone(f.metadata.release);
  body.scope.releaseId = id;
  body.revision = revision;
  body.targets[0].targetKey = signed(packageBytes).scope.targetKey;
  body.targets[0].packageManifest = envelopeReference(packageBytes);
  return sign(body);
}
function descriptor(name, body) {
  emit(`${name}.json`, Buffer.from(JSON.stringify(body)));
}

let details;
if (scenario === 'roots') {
  const roots = [];
  let last;
  for (let version = 2; version <= 4097; version++) {
    const next = { ...structuredClone(root), version, issuedAt: NOW + 10, expiresAt: NOW + 5000 };
    last = sign(next);
    roots.push(emit(`root-${version}.json`, last));
  }
  // A maximum-size *new batch* succeeds first; it is never forged into DB history.
  const final = verifyRootChain(roots.map((_, index) => sign({ ...structuredClone(root),
    version: index + 2, issuedAt: NOW + 10, expiresAt: NOW + 5000 })),
  { trustedRoot: root, product: 'pinvou', now: NOW + 1000 });
  assert.equal(final.signed.version, 4097);
  const first = graph('first', 1, [release('r1', 1)], { trustedRoot: final.signed, roots });
  assert.throws(() => verifyRootSuccessor(last, { trustedRoot: { ...structuredClone(root),
    version: 4096, issuedAt: NOW + 10, expiresAt: NOW + 5000 }, product: 'pinvou', now: NOW + 7000 }),
  error => error.code === 'TIME_WINDOW_CLOSED');
  descriptor('expired-prefix', { ...first.bundle, suppliedNowMs: NOW + 7000 });
  const overflow = sign({ ...structuredClone(root), version: 4098, issuedAt: NOW + 10, expiresAt: NOW + 5000 });
  verifyRootSuccessor(overflow, { trustedRoot: final.signed, product: 'pinvou', now: NOW + 1000 });
  const overflowName = emit('overflow-root-4098.json', overflow);
  // Every successor is individually valid and fresh; only the original batch
  // bound rejects this4097-entry input, rather than a future/expired tail.
  descriptor('overflow', { ...first.bundle, roots: [...roots, overflowName] });
  const next = sign({ ...structuredClone(root), version: 4098, issuedAt: NOW + 6000, expiresAt: NOW + DAY });
  const activeEnvelopes = [first.timestampBytes, first.snapshotBytes, ...first.members].map(bytes => {
    const body = signed(bytes);
    return { bytes, expected: { role: body.role, product: body.product, component: body.component, scope: body.scope } };
  });
  const secondRoot = verifyRootChain([next], { trustedRoot: final.signed, product: 'pinvou', now: NOW + 7000, activeEnvelopes });
  graph('second', 2, [release('r1', 1)], { trustedRoot: secondRoot.signed,
    roots: [emit('later-root-4098.json', next)], now: NOW + 7000 });
  assert.ok(final.signed.expiresAt < NOW + 7000);
  details = { committedSuccessors: [4096, 1], expiredNewBatchReferenceRefused: true, firstObservedAt: NOW + 1000,
    secondObservedAt: NOW + 7000, firstTailExpiresAt: final.signed.expiresAt,
    firstTailHash: hash(last), secondRootHash: hash(next), historyFirstRootHash: hash(sign({
      ...structuredClone(root), version: 2, issuedAt: NOW + 10, expiresAt: NOW + 5000 })) };
} else if (scenario === 'snapshot') {
  const targetKey = 'windows-x86_64-nsis-perMachine';
  for (const policy of root.roles) if (policy.targetKey !== null) policy.targetKey = targetKey;
  const stablePolicy = root.roles.find(policy => policy.role === 'target');
  for (const channel of ['beta', 'internal']) root.roles.push({ ...structuredClone(stablePolicy), channel });
  const pkg = structuredClone(f.metadata.package);
  pkg.scope.targetKey = targetKey;
  pkg.finalInstaller.format = 'nsis';
  for (const helper of pkg.helpers) helper.targetKey = targetKey;
  const packageBytes = sign(pkg);
  const releases = Array.from({ length: 4092 }, (_, i) => release(`r${i}`, 1, packageBytes));
  const channels = ['stable', 'beta', 'internal'];
  const maximum = graph('maximum', 1, releases, { packageBytes, channels, paths: true });
  assert.equal(maximum.members.length, 4096);
  descriptor('missing-final', { ...maximum.bundle, members: maximum.bundle.members.slice(0, -1) });
  descriptor('duplicate-final', { ...maximum.bundle,
    members: [...maximum.bundle.members.slice(0, -1), maximum.bundle.members[0]] });
  // Keep the extra-member negative within the4096 array bound. Its independently
  // valid graph has4095 members; one genuine signed unreferenced member is added.
  const near = graph('near', 2, releases.slice(0, -1), { packageBytes, channels, paths: true });
  const extra = release('extra', 1, packageBytes);
  const extraName = emit('unreferenced-release.json', extra);
  assert.throws(() => verifyPublicChain({ trustedRoot: root, product: 'pinvou', component: 'app', now: NOW + 1000,
    timestampBytes: near.timestampBytes, snapshotBytes: near.snapshotBytes, members: [...near.members, extra] }),
  error => error.code === 'PUBLIC_MEMBER_UNREFERENCED');
  descriptor('extra-final', { ...near.bundle, members: [...near.bundle.members, extraName] });
  details = { snapshotMembers: 4096, targets: 3, releases: 4092, packages: 1,
    snapshotBytes: maximum.snapshotBytes.length, largestTargetBytes: Math.max(...maximum.targetBytes.map(bytes => bytes.length)),
    extraControlReferencedMembers: near.members.length, extraControlSuppliedMembers: near.members.length + 1,
    lastOriginalName: maximum.bundle.members.at(-1), lastOriginalHash: hash(packageBytes) };
} else if (scenario === 'bounds') {
  const complete = graph('complete', 1, [release('r1', 1)]);
  const valid = Buffer.from(JSON.stringify(complete.bundle));
  assert.ok(valid.length < limit);
  writeFileSync(join(directory, 'exact.json'), Buffer.concat([Buffer.alloc(limit - valid.length, 32), valid]));
  writeFileSync(join(directory, 'over.json'), Buffer.concat([Buffer.alloc(limit - valid.length + 1, 32), valid]));
  details = { descriptorBytes: valid.length, exactBytes: limit, overBytes: limit + 1 };
} else if (scenario === 'release') {
  const r1 = release('r1', 2), r2 = release('r2', 1), old = release('r1', 1);
  graph('r1', 1, [r1]);
  graph('r2', 2, [r2]);
  graph('rollback-r1', 3, [old]);
  details = { r1Revision: 2, r2Revision: 1, sameAppVersion: signed(r1).appVersion === signed(r2).appVersion,
    r1Hash: hash(r1), r2Hash: hash(r2), rollbackHash: hash(old) };
} else {
  throw new Error('unknown collection fixture scenario');
}
const anchor = sign(root);
emit('anchor.json', anchor);
process.stdout.write(JSON.stringify({ pin: hash(anchor), now: NOW, details }));
