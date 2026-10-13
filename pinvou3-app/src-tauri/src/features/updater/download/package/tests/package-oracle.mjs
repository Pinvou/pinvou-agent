// Actual original strong signatures/Schema/semantics preflight; no private key output.
import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {pathToFileURL} from 'node:url';
const reference=pathToFileURL(process.argv[2]);
const{createFixtureSet,NOW,TARGET}=await import(new URL('./tests/fixtures.mjs',reference));
const{verifyEnvelope,envelopeReference}=await import(new URL('./signatures.mjs',reference));
const f=createFixtureSet();
const physical=JSON.parse(await readFile(process.argv[3],'utf8'));
const expected={role:'package',product:'pinvou',component:'app',scope:{targetKey:TARGET}};
const cases=physical.map(item=>{
  const pkg=structuredClone(f.metadata.package);
  pkg.fullPackage={packageId:'package-2',...item.artifact};
  for(const role of ['finalInstaller','helper','launcher'])Object.assign(pkg[role],item.natives[role]);
  pkg.helpers[0].nativeIdentity=structuredClone(pkg.helper);
  const extra=structuredClone(pkg.helpers[0]);extra.helperId='helper-extra';
  Object.assign(extra.nativeIdentity,item.natives.extraHelper);
  extra.installationTarget='helpers/helper-extra';
  pkg.helpers.push(extra);
  if(item.changes.finalInstallerSize)pkg.finalInstaller.size=item.changes.finalInstallerSize;
  if(item.changes.finalInstallerSHA256)pkg.finalInstaller.sha256=item.changes.finalInstallerSHA256;
  if(item.changes.extraHelperSHA256)pkg.helpers[1].nativeIdentity.sha256=item.changes.extraHelperSHA256;
  pkg.provenance.finalInstallerSha256=pkg.finalInstaller.sha256;
  const manifest=f.sign(pkg);
  const verified=verifyEnvelope(manifest,{trustedRoot:f.metadata.root,expected,now:NOW});
  assert.equal(verified.signed.role,'package');
  return{...item,rootManifest:Buffer.from(f.sign(f.metadata.root)).toString('utf8'),manifest:Buffer.from(manifest).toString('utf8'),reference:envelopeReference(manifest),
    expectedContext:expected,expectedVersion:pkg.appVersion,time:NOW};
});
assert.equal(cases.length,14);
await writeFile(process.argv[4],JSON.stringify(cases));
