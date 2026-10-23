// Ephemeral reference keys sign public test claims; no secret is retained.
import {pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
const base = pathToFileURL(process.argv[1]);
const {createFixtureSet, NOW} = await import(new URL('./tests/fixtures.mjs', base));
const {canonicalize} = await import(base);
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
process.stdout.write(JSON.stringify({
  root: Buffer.from(f.sign(root)).toString('utf8'),
  otherRoot: Buffer.from(f.sign({...root, version: 2})).toString('utf8'),
  claimsPin: createHash('sha256').update(rootClaims).digest('hex'),
  package: Buffer.from(f.sign(pack)).toString('utf8'),
  expected: Buffer.from(canonicalize({role: 'package', product: 'pinvou', component: 'app', scope: {targetKey: target}})).toString('utf8'),
  now: NOW, expiresAt: root.expiresAt,
}));
