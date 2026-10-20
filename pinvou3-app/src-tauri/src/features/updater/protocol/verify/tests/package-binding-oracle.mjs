// Full frozen operation; only Package is authenticated. Private keys stay here.
import { pathToFileURL } from 'node:url';
const reference=pathToFileURL(process.argv[1]);
const {createFixtureSet,NOW}=await import(new URL('./tests/fixtures.mjs',reference));
const {signEnvelope,envelopeReference}=await import(reference);
const {verifyReleasePackage}=await import(new URL('./metadata-chain.mjs',reference));
const {canonicalize}=await import(new URL('./canonical-json.mjs',reference));
const {validateSchema}=await import(new URL('./schema-registry.mjs',reference));
const fixtures=createFixtureSet(),clone=structuredClone,rows=[];
function make(group,name,{packageChange=()=>{},releaseChange=()=>{},rootChange=()=>{},
 contextChange=()=>{},packageWire=b=>b,afterReference=b=>b,releaseWire=b=>b,
 signPackage=s=>fixtures.sign(s),now=NOW,deniedKeyIds=[],accepted=false}={}) {
 const root=clone(fixtures.metadata.root);rootChange(root);
 const pkg=clone(fixtures.metadata.package),release=clone(fixtures.metadata.release);
 packageChange(pkg);const beforeReference=packageWire(signPackage(pkg));
 release.targets[0].packageManifest=envelopeReference(beforeReference);
 releaseChange(release);
 // Reuse the original envelope wrapper as caller-owned Schema material.
 const envelope=JSON.parse(Buffer.from(fixtures.bytes.release));envelope.signed=release;
 const releaseBytes=releaseWire(Buffer.from(canonicalize(envelope)));
 const suppliedRelease=JSON.parse(Buffer.from(releaseBytes)).signed;
 let releaseSchema=true;try {validateSchema('metadata/release',JSON.parse(Buffer.from(releaseBytes)));}
 catch(error){if(typeof error.code!=='string')throw error;releaseSchema=false;}
 const packageBytes=afterReference(beforeReference);
 const context={product:'pinvou',component:'app',targetKey:pkg.scope.targetKey};contextChange(context);
 let code=null;try {verifyReleasePackage({trustedRoot:root,...context,release:suppliedRelease,
 packageBytes,now,deniedKeyIds});}catch(error){if(typeof error.code!=='string')throw error;code=error.code;}
 if((code===null)!==accepted)throw new Error(name+': unexpected original result '+code);
 rows.push({group:releaseSchema?group:'factory',name,accepted,code,releaseSchema,
 trustedRoot:root,context,now,deniedKeyIds,releaseBytes:Buffer.from(releaseBytes).toString('base64'),
 packageBytes:Buffer.from(packageBytes).toString('base64')});
}
make('binding','original-package-binding',{accepted:true});
make('binding','mode-order-positive',{packageChange:p=>{p.activationModes.reverse();},accepted:true});
make('binding','release-mode-subset-positive',{releaseChange:r=>{r.targets[0].activationModes=['directInstall'];},accepted:true});
make('binding','package-version-mismatch',{packageChange:p=>{p.appVersion='3.0.0';}});
make('binding','launcher-publisher-mismatch',{packageChange:p=>{p.launcher.publisher='other-publisher';}});
make('binding','package-missing-release-mode',{packageChange:p=>{p.activationModes=['directInstall'];}});
make('binding','release-target-missing',{contextChange:c=>{c.targetKey='linux-arm64-deb-perMachine';}});
for(const field of ['size','envelopeSha256','version','expiresAt','role','scope']) {
 make('binding','original-reference-'+field,{releaseChange:r=>{
  const ref=r.targets[0].packageManifest;
  if(field==='size')ref.size++;
  if(field==='envelopeSha256')ref.envelopeSha256='b'.repeat(64);
  if(field==='version'||field==='expiresAt')ref[field]=1;
  if(field==='role')ref.role='target';
  if(field==='scope')ref.scope.targetKey='linux-arm64-deb-perMachine';
 }});
}
make('supplied_release','foreign-release-product-positive',{releaseChange:r=>{r.product='foreign-product';},accepted:true});
make('supplied_release','foreign-release-component-positive',{releaseChange:r=>{r.component='engine';},accepted:true});
make('supplied_release','expired-release-positive',{releaseChange:r=>{r.issuedAt=NOW-2000;r.expiresAt=NOW-1000;},accepted:true});
const transform=mutate=>bytes=>{const e=JSON.parse(Buffer.from(bytes));mutate(e);return Buffer.from(canonicalize(e));};
make('supplied_release','invalid-release-signature-positive',{releaseWire:transform(e=>{e.signatures[0].signature=Buffer.alloc(64).toString('base64url');}),accepted:true});
make('context','independent-product',{contextChange:c=>{c.product='other-product';}});
make('context','independent-component',{contextChange:c=>{c.component='engine';}});
make('context','independent-extras-positive',{contextChange:c=>{c.ignored={value:1};},accepted:true});
const threshold=r=>{const p=r.roles.find(p=>p.role==='package');p.keyIds=[fixtures.descriptors[2].keyId,fixtures.descriptors[3].keyId];p.threshold=2;};
const two=s=>signEnvelope(s,fixtures.keys.slice(2));
make('signatures','two-actual-package-signatures',{rootChange:threshold,signPackage:two,accepted:true});
make('signatures','package-threshold-unmet',{rootChange:threshold});
make('signatures','invalid-package-signature',{packageWire:transform(e=>{e.signatures[0].signature=Buffer.alloc(64).toString('base64url');})});
make('signatures','bad-extra-after-valid-threshold',{rootChange:r=>{threshold(r);r.roles.find(p=>p.role==='package').threshold=1;},signPackage:two,
 packageWire:transform(e=>{e.signatures.at(-1).signature=Buffer.alloc(64).toString('base64url');})});
make('signatures','known-descriptor-outside-role',{signPackage:s=>signEnvelope(s,[fixtures.keys[0],fixtures.keys[3]])});
make('signatures','unsigned-package-body-change',{packageWire:transform(e=>{e.signed.fullPackage.packageId='unsigned-change';})});
make('signatures','global-deny',{deniedKeyIds:[fixtures.descriptors[3].keyId]});
make('signatures','unused-root-key-deny-positive',{deniedKeyIds:[fixtures.descriptors[0].keyId],accepted:true});
make('wire_policy','expired-root',{now:fixtures.metadata.root.expiresAt});
make('wire_policy','future-root',{now:fixtures.metadata.root.issuedAt-1});
make('wire_policy','wrong-exact-package-target-policy',{rootChange:r=>{r.roles.find(p=>p.role==='package').targetKey='linux-arm64-deb-perMachine';}});
make('wire_policy','package-noncanonical',{packageWire:b=>Buffer.concat([Buffer.from(b),Buffer.from(' ')])});
make('wire_policy','package-closed-schema',{packageWire:transform(e=>{e.signed.unrecognized=true;})});
make('wire_policy','original-one-mib-input-limit',{afterReference:b=>Buffer.concat([Buffer.from(b),Buffer.alloc(1048577-b.length,32)])});
make('wire_policy','production-incremental-refuses',{packageChange:p=>{
 p.incrementalPackages=[{packageId:'delta-2',size:1,sha256:'a'.repeat(64),baseVersion:'1.0.0',
 baseArtifactSha256:'b'.repeat(64),resultArtifactSize:123,resultArtifactSha256:'a'.repeat(64),
 algorithm:'test-delta',algorithmVersion:'1',format:'test-delta'}];
}});
console.log(JSON.stringify(rows));
