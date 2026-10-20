// Independent ephemeral Ed25519 fixtures; no signing secret reaches disk/output.
import {readFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {pathToFileURL} from 'node:url';
import {dirname, join} from 'node:path';
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
const profile = process.argv[7] ?? 'legacy';
if (profile !== 'legacy') {
  const dir = dirname(process.argv[4]);
  const claim = async (file, format, name) => ({...identity(await readFile(join(dir,file))),format,identity:name,publisher:'test-publisher'});
  const first = await claim('helper-first.exe','helper','first-helper-native');
  const execution = await claim('helper-execution.exe','helper','execution-helper-native');
  pkg.helper = execution;
  pkg.launcher = await claim('launcher.exe','launcher','launcher-native');
  pkg.helpers = [
    {helperId:'helper-first',version:'1.0.0',targetKey:target,nativeIdentity:first,installationTarget:'logical/first-not-a-zip-member'},
    {helperId:'helper-execution',version:'2.0.0',targetKey:target,nativeIdentity:execution,installationTarget:'logical/execution-not-a-zip-member'}
  ];
  pkg.executionHelperId = 'helper-execution';
  if (profile === 'missing-helper') pkg.helpers.pop();
  if (profile === 'duplicate-helper') pkg.helpers[0].helperId = pkg.helpers[1].helperId;
  if (profile === 'helper-target') pkg.helpers[0].targetKey = 'linux-x86_64-deb-perMachine';
  if (profile === 'missing-execution-id') pkg.executionHelperId = 'not-declared';
  if (profile === 'helper-canonical') pkg.helper = {...pkg.helper,publisher:'another-declared-publisher'};
}
if (process.argv[6] === 'wrong-native') {
  pkg.finalInstaller.sha256 = 'b'.repeat(64);
  pkg.provenance.finalInstallerSha256 = pkg.finalInstaller.sha256;
}
const packageBytes = f.sign(pkg);
const expected = {role:'package',product:'pinvou',component:'app',scope:{targetKey:target}};
if (profile === 'legacy' || profile === 'roles') verifyEnvelope(packageBytes,{trustedRoot:f.metadata.root,expected,now:NOW});
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
