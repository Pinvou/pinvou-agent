// Fresh public test inputs, signed by the existing original reference factory.
// No private key, credential input or production provider is persisted.
import {pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
process.on('uncaughtException', () => { process.stderr.write('RESUME_FIXTURE_FAILED\n'); process.exitCode = 1; });
const base = pathToFileURL(process.argv[2]);
const {createFixtureSet, NOW} = await import(new URL('./tests/fixtures.mjs', base));
const {canonicalize} = await import(new URL('./canonical-json.mjs', base));
const f = createFixtureSet();
const text = value => Buffer.from(value).toString('utf8');
const root = text(canonicalize(f.metadata.root));
const request = {protocolVersion:1, scope:{product:'pinvou',component:'app',installId:'install-1',...f.claims.decision.scope},
  decision:JSON.parse(text(f.sign(f.claims.decision))),downloadKey:'resume-download-1',packageId:f.claims.download.packageId};
const cases = {};
for (const kind of ['original','rotated','sourceId','objectId','objectVersion']) {
  const claims = structuredClone(f.claims.download);
  const url = kind === 'rotated' ? 'https://files.example.invalid/renewed?opaque=two' : 'https://files.example.invalid/first?opaque=one';
  if (['sourceId','objectId','objectVersion'].includes(kind)) claims.sourceBinding[kind] += '-changed';
  if (kind === 'rotated') { claims.jti = 'renewed-download-credential'; claims.iat += 1000; claims.nbf += 1000; claims.exp += 1000; }
  claims.sourceBinding.urlSha256 = createHash('sha256').update(Buffer.from(url,'utf8')).digest('hex');
  const response = {protocolVersion:1,requestId:'resume-test',credential:JSON.parse(text(f.sign(claims))),
    packageId:claims.packageId,size:claims.package.size,sha256:claims.package.sha256,url,expiresAt:claims.exp,fileSourceClass:'isolated-test'};
  cases[kind] = {request:text(canonicalize(request)),response:text(canonicalize(response)),suppliedTimeMillis:NOW+(kind==='rotated'?1000:0)};
}
process.stdout.write(JSON.stringify({root,rootSha256:createHash('sha256').update(root).digest('hex'),
  package:text(f.sign(f.metadata.package)),expected:text(canonicalize({role:'package',product:'pinvou',component:'app',scope:{targetKey:'linux-x86_64-deb-perMachine'}})),cases}));
