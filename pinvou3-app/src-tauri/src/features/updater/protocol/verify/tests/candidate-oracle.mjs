// Full frozen operation; fresh private keys never leave this Node process.
import { pathToFileURL } from 'node:url';
import { randomBytes } from 'node:crypto';
const reference=pathToFileURL(process.argv[1]);
const {createFixtureSet,NOW}=await import(new URL('./tests/fixtures.mjs',reference));
const {signEnvelope,envelopeReference}=await import(reference);
const {verifyCandidateRelease}=await import(new URL('./metadata-chain.mjs',reference));
const {canonicalize}=await import(new URL('./canonical-json.mjs',reference));
const {sha256,rolloutCommitment}=await import(new URL('./digests.mjs',reference));
const fixtures=createFixtureSet();const clone=structuredClone;const rows=[];
function make(group,name,{mutate=()=>{},openingChange=()=>{},contextChange=()=>{},
 rootChange=()=>{},targetWire=b=>b,releaseWire=b=>b,baseline=false,
 signTarget=s=>fixtures.sign(s),deniedKeyIds=[],now=NOW,accepted=false}={}) {
 const root=clone(fixtures.metadata.root);rootChange(root);
 const target=clone(fixtures.metadata.target),release=clone(fixtures.metadata.release);
 const pkg=clone(fixtures.metadata.package);pkg.appVersion='3.0.0';pkg.fullPackage.packageId='candidate-package-3';
 release.scope.releaseId='candidate-release-3';release.appVersion='3.0.0';
 release.targets[0].packageManifest=envelopeReference(fixtures.sign(pkg));
 mutate(target,release);
 const releaseBytes=releaseWire(fixtures.sign(release));
 const opening={targetKey:target.scope.targetKey,deploymentId:'candidate-deployment-3',
 deploymentRevision:1,rolloutId:'candidate-rollout-3',rolloutRevision:1,
 releaseEnvelopeSha256:sha256(releaseBytes),leafSalt:randomBytes(32).toString('hex')};
 target.rolloutSetCommitment=rolloutCommitment(opening);
 if(baseline)target.baselineRelease=envelopeReference(releaseBytes);
 const targetBytes=targetWire(signTarget(target));
 const replacement=openingChange(opening);const suppliedOpening=replacement===undefined?opening:replacement;
 const context={product:'pinvou',component:'app',channel:target.scope.channel,
 targetKey:target.scope.targetKey,expectedReleaseId:release.scope.releaseId};contextChange(context);
 let code=null;try {verifyCandidateRelease({trustedRoot:root,...context,targetBytes,
 releaseBytes,opening:suppliedOpening,now,deniedKeyIds});}catch(error){if(typeof error.code!=='string')throw error;code=error.code;}
 if((code===null)!==accepted)throw new Error(name+': unexpected frozen result '+code);
 rows.push({group,name,accepted,code,trustedRoot:root,context,now,opening:suppliedOpening,deniedKeyIds,
 targetBytes:Buffer.from(targetBytes).toString('base64'),releaseBytes:Buffer.from(releaseBytes).toString('base64')});
}
make('binding','genuine-empty-catalog-candidate',{accepted:true});
make('binding','opening-restored-positive',{openingChange:o=>{o.rolloutRevision++;o.rolloutRevision--;},accepted:true});
make('binding','null-opening-refuses',{openingChange:()=>null});
make('binding','baseline-is-not-candidate',{baseline:true});
for(const field of ['deploymentId','rolloutId'])make('binding','opening-'+field,
 {openingChange:o=>{o[field]+='-changed';}});
for(const field of ['deploymentRevision','rolloutRevision'])make('binding','opening-'+field,
 {openingChange:o=>{o[field]++;}});
make('binding','opening-release-hash',{openingChange:o=>{o.releaseEnvelopeSha256='b'.repeat(64);}});
make('binding','opening-leaf-salt',{openingChange:o=>{o.leafSalt='b'.repeat(64);}});
make('binding','opening-target-key',{openingChange:o=>{o.targetKey='linux-arm64-deb-perMachine';}});
make('binding','opening-unknown-field',{openingChange:o=>{o.unrecognized=true;}});
make('binding','release-target-membership',{mutate:(t,r)=>{
 r.targets[0].targetKey='linux-arm64-deb-perMachine';
 const arm=clone(fixtures.metadata.package);arm.appVersion='3.0.0';arm.scope.targetKey=r.targets[0].targetKey;
 for(const helper of arm.helpers)helper.targetKey=arm.scope.targetKey;
 r.targets[0].packageManifest=envelopeReference(fixtures.sign(arm));
}});
for(const [field,value] of [['product','other-product'],['component','engine'],['channel','beta'],
 ['targetKey','linux-arm64-deb-perMachine'],['expectedReleaseId','other-release']]) {
 make('context','independent-'+field,{contextChange:c=>{c[field]=value;}});
}
make('context','release-cannot-switch-component',{mutate:(t,r)=>{r.component='engine';}});
make('context','independent-extras-ignored',{contextChange:c=>{c.ignored={value:1};},accepted:true});
const transform=(mutate)=>bytes=>{const e=JSON.parse(Buffer.from(bytes));mutate(e);return Buffer.from(canonicalize(e));};
make('signatures','invalid-target-signature',{targetWire:transform(e=>{e.signatures[0].signature=Buffer.alloc(64).toString('base64url');})});
make('signatures','invalid-release-signature',{releaseWire:transform(e=>{e.signatures[0].signature=Buffer.alloc(64).toString('base64url');})});
const two=s=>signEnvelope(s,fixtures.keys.slice(2));
const threshold=root=>{const p=root.roles.find(p=>p.role==='target');p.keyIds=[fixtures.descriptors[2].keyId,fixtures.descriptors[3].keyId];p.threshold=2;};
make('signatures','two-real-target-signatures',{rootChange:threshold,signTarget:two,accepted:true});
make('signatures','target-threshold-unmet',{rootChange:threshold});
make('signatures','bad-extra-after-valid-target-threshold',{rootChange:r=>{threshold(r);r.roles.find(p=>p.role==='target').threshold=1;},
 signTarget:two,targetWire:transform(e=>{e.signatures.at(-1).signature=Buffer.alloc(64).toString('base64url');})});
make('signatures','known-key-outside-role',{signTarget:s=>signEnvelope(s,[fixtures.keys[0],fixtures.keys[3]])});
make('signatures','global-deny',{deniedKeyIds:[fixtures.descriptors[3].keyId]});
make('signatures','unused-global-deny-positive',{deniedKeyIds:[fixtures.descriptors[0].keyId],accepted:true});
make('windows','expired-root',{now:fixtures.metadata.root.expiresAt});
make('windows','future-root',{now:fixtures.metadata.root.issuedAt-1});
make('windows','expired-target',{now:fixtures.metadata.target.expiresAt});
make('windows','expired-release',{mutate:(t,r)=>{r.issuedAt=NOW-1000;r.expiresAt=NOW;}});
make('wire_policy','exact-target-channel-policy',{rootChange:r=>{r.roles.find(p=>p.role==='target').channel='beta';}});
make('wire_policy','target-noncanonical',{targetWire:b=>Buffer.concat([Buffer.from(b),Buffer.from(' ')])});
make('wire_policy','release-noncanonical',{releaseWire:b=>Buffer.concat([Buffer.from(b),Buffer.from(' ')])});
make('wire_policy','target-closed-schema',{targetWire:transform(e=>{e.signed.unrecognized=true;})});
make('wire_policy','release-closed-schema',{releaseWire:transform(e=>{e.signed.unrecognized=true;})});
make('wire_policy','target-original-one-mib-limit',{targetWire:b=>Buffer.concat([Buffer.from(b),Buffer.alloc(1048577-b.length,32)])});
console.log(JSON.stringify(rows));
