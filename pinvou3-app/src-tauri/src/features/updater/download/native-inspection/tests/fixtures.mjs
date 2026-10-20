// Independent ephemeral Ed25519 fixtures; no signing secret reaches disk/output.
import {readFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {pathToFileURL} from 'node:url';
const reference = pathToFileURL(process.argv[2]);
const {createFixtureSet, NOW, TARGET} = await import(new URL('./tests/fixtures.mjs', reference));
const {canonicalize} = await import(new URL('./canonical-json.mjs', reference));
const {envelopeReference, verifyEnvelope} = await import(new URL('./signatures.mjs', reference));
const f = createFixtureSet();
const target = process.argv[5] ?? 'windows-x86_64-nsis-perMachine';
const rewrite = value => {
  if (value === TARGET) return target;
  if (Array.isArray(value)) return value.map(rewrite);
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([k,v]) => [k,rewrite(v)]));
  return value;
};
f.metadata = rewrite(f.metadata);
f.claims = rewrite(f.claims);
const identity = bytes => ({size:bytes.length,sha256:createHash('sha256').update(bytes).digest('hex')});
const physical = identity(await readFile(process.argv[3]));
const native = identity(await readFile(process.argv[4]));
const pkg = f.metadata.package;
pkg.fullPackage = {packageId:'package-2',...physical};
for (const role of ['finalInstaller','helper','launcher']) Object.assign(pkg[role], native);
pkg.finalInstaller.format = target.startsWith('windows-') ? 'nsis' : 'deb';
pkg.finalInstaller.identity = pkg.finalInstaller.format;
pkg.helpers[0].nativeIdentity = structuredClone(pkg.helper);
pkg.provenance.finalInstallerSha256 = native.sha256;
if (process.argv[6] === 'wrong-native') {
  pkg.finalInstaller.sha256 = 'b'.repeat(64);
  pkg.provenance.finalInstallerSha256 = pkg.finalInstaller.sha256;
}
const packageBytes = f.sign(pkg);
const expected = {role:'package',product:'pinvou',component:'app',scope:{targetKey:target}};
verifyEnvelope(packageBytes,{trustedRoot:f.metadata.root,expected,now:NOW});
for (const role of ['decision','download']) {
  const c = f.claims[role];
  c.host = target.startsWith('windows-') ? {os:'windows',arch:'x86_64',osVersion:'10.0'} : c.host;
  Object.assign(c.update,{finalInstaller:pkg.finalInstaller,helper:pkg.helper,launcher:pkg.launcher});
  for (const kind of ['endpointChain','hopChain']) {
    c.update[kind].package = physical;
    c.update[kind].packageManifest = envelopeReference(packageBytes);
  }
}
f.claims.download.package = physical;
const text = value => Buffer.from(value).toString('utf8');
const request = {protocolVersion:1,scope:{product:'pinvou',component:'app',installId:'install-1',...f.claims.decision.scope},
  decision:JSON.parse(text(f.sign(f.claims.decision))),downloadKey:'download-key-1',packageId:'package-2'};
const response = {protocolVersion:1,requestId:'qa-request',credential:JSON.parse(text(f.sign(f.claims.download))),
  packageId:'package-2',...physical,url:'https://127.0.0.1:1/file',expiresAt:f.claims.download.exp,fileSourceClass:'isolated-qa'};
const root = text(canonicalize(f.metadata.root));
process.stdout.write(JSON.stringify({root,rootSha256:identity(Buffer.from(root)).sha256,
  package:text(packageBytes),expected:text(canonicalize(expected)),
  imported:{request:text(canonicalize(request)),response:text(canonicalize(response)),suppliedTimeMillis:NOW}}));
