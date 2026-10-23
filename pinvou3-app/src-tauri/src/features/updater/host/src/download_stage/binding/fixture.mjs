// Ephemeral reference keys sign public test claims; no secret is retained.
import {pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
const base = pathToFileURL(process.argv[1]);
const {createFixtureSet, NOW} = await import(new URL('./tests/fixtures.mjs', base));
const {canonicalize} = await import(base);
const {signEnvelope} = await import(new URL('./signatures.mjs', base));
const f = createFixtureSet();
const target = 'windows-x86_64-nsis-perMachine';
const pack = structuredClone(f.metadata.package);
const root = structuredClone(f.metadata.root);
for (const policy of root.roles) {
  if (policy.targetKey === pack.scope.targetKey) policy.targetKey = target;
}
pack.scope.targetKey = target;
pack.finalInstaller.format = 'nsis';
for (const helper of pack.helpers) helper.targetKey = target;
const rootClaims = canonicalize(root);
// Reuse the original ephemeral fixture keys and reference signing implementation.
// Every successor changes ALL Root and Package signing keys; no key is exported.
const second = createFixtureSet();
const third = createFixtureSet();
function windowsRoot(set, version, issuedAt) {
  const value = structuredClone(set.metadata.root);
  value.version = version;
  value.issuedAt = issuedAt;
  value.expiresAt = NOW + 86_400_000;
  for (const policy of value.roles) {
    if (policy.targetKey !== null) policy.targetKey = target;
  }
  return value;
}
const root2 = windowsRoot(second, 2, NOW + 1000);
const root3 = windowsRoot(third, 3, NOW + 1500);
const rotate = (value, oldSet, newSet) => Buffer.from(signEnvelope(value,
  [...oldSet.keys.slice(0, 2), ...newSet.keys.slice(0, 2)])).toString('utf8');
const expiredIntermediate = {...root2, expiresAt: NOW + 1500};
const expiredFinal = {...root3, expiresAt: NOW + 1600};
const rotation = {
  now: NOW + 2000,
  root2: rotate(root2, f, second), root3: rotate(root3, second, third),
  root2ClaimsPin: createHash('sha256').update(canonicalize(root2)).digest('hex'),
  root3ClaimsPin: createHash('sha256').update(canonicalize(root3)).digest('hex'),
  package2: Buffer.from(second.sign(pack)).toString('utf8'),
  package3: Buffer.from(third.sign(pack)).toString('utf8'),
  missingOldThreshold: Buffer.from(signEnvelope(root2, second.keys.slice(0, 2))).toString('utf8'),
  missingNewThreshold: Buffer.from(signEnvelope(root2, f.keys.slice(0, 2))).toString('utf8'),
  wrongProduct: rotate({...root2, product: 'other'}, f, second),
  expiredIntermediate: rotate(expiredIntermediate, f, second),
  expiredFinal: rotate(expiredFinal, second, third),
};
process.stdout.write(JSON.stringify({
  root: Buffer.from(f.sign(root)).toString('utf8'),
  otherRoot: Buffer.from(f.sign({...root, version: 2})).toString('utf8'),
  claimsPin: createHash('sha256').update(rootClaims).digest('hex'),
  package: Buffer.from(f.sign(pack)).toString('utf8'),
  expected: Buffer.from(canonicalize({role: 'package', product: 'pinvou', component: 'app', scope: {targetKey: target}})).toString('utf8'),
  now: NOW, expiresAt: root.expiresAt,
  rotation,
}));
