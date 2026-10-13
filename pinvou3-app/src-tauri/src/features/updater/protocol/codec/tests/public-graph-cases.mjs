import {pathToFileURL} from 'node:url';
const reference=pathToFileURL(process.argv[1]);
const {createFixtureSet,NOW}=await import(new URL('./tests/fixtures.mjs',reference));
const {verifyEnvelope,envelopeReference}=await import(reference);
const {verifyPublicChain}=await import(new URL('./metadata-chain.mjs',reference));
const fixtures=createFixtureSet();
const output=[];
function make(name,{mutate=()=>{},memberTransform=(m)=>m,entryTransform=(e)=>e,
 trustedRoot=fixtures.metadata.root,releaseTransform=()=>{},targetTransform=()=>{},timestampTransform=()=>{},context={product:'pinvou',component:'app'}}={}) {
 const packageManifest=structuredClone(fixtures.metadata.package);
 const release=structuredClone(fixtures.metadata.release);
 const target=structuredClone(fixtures.metadata.target);
 mutate(target,release,packageManifest);
 const packageBytes=fixtures.sign(packageManifest);
 release.targets[0].packageManifest=envelopeReference(packageBytes);
 releaseTransform(release);
 const releaseBytes=fixtures.sign(release);
 target.baselineRelease=envelopeReference(releaseBytes);
 targetTransform(target);
 const targetBytes=fixtures.sign(target);
 const declared=[targetBytes,releaseBytes,packageBytes];
 const members=memberTransform(declared);
 const snapshot={...structuredClone(fixtures.metadata.snapshot),
  entries:entryTransform(declared.map(envelopeReference))};
 const snapshotBytes=fixtures.sign(snapshot);
 const timestamp={...structuredClone(fixtures.metadata.timestamp),snapshot:envelopeReference(snapshotBytes)};
 timestampTransform(timestamp);
 const timestampBytes=fixtures.sign(timestamp);
 // Authentication, role-policy scope, semantics and fixed fixture time must
 // independently pass BEFORE any expected graph rejection is accepted as proof.
 for(const bytes of [timestampBytes,snapshotBytes,...members]){
  const signed=JSON.parse(Buffer.from(bytes).toString('utf8')).signed;
  verifyEnvelope(bytes,{trustedRoot,now:NOW,
   expected:{role:signed.role,product:'pinvou',component:'app',scope:signed.scope}});
 }
 const args={trustedRoot,...context,now:NOW,timestampBytes,snapshotBytes,members};
 let accepted;
 try{verifyPublicChain(args);accepted=true;}catch{accepted=false;}
 output.push({name,accepted,now:NOW,context,
  timestamp:Buffer.from(timestampBytes).toString('base64'),
  snapshot:Buffer.from(snapshotBytes).toString('base64'),
  members:members.map(bytes=>Buffer.from(bytes).toString('base64'))});
}
make('strong-signed-complete-positive');
for(const publisher of ['different-publisher','test-publisher']){
 make('package-launcher-'+publisher,{mutate:(t,r,p)=>{p.launcher.publisher=publisher;}});
}
make('package-version-mismatch',{mutate:(t,r,p)=>{p.appVersion='3.0.0';}});
make('package-mode-subset-mismatch',{mutate:(t,r,p)=>{p.activationModes=['directInstall'];}});
make('mode-order-positive',{mutate:(t,r,p)=>{p.activationModes.reverse();}});
make('missing-package',{memberTransform:m=>m.slice(0,2),entryTransform:e=>e.slice(0,2)});
make('missing-release',{memberTransform:m=>[m[0],m[2]],entryTransform:e=>[e[0],e[2]]});
make('no-target',{memberTransform:m=>m.slice(1),entryTransform:e=>e.slice(1)});
make('missing-declared-member',{memberTransform:m=>m.slice(0,2)});
make('duplicate-actual-member',{memberTransform:m=>[...m,m[2]]});
make('unreferenced-actual-member',{memberTransform:m=>[...m,fixtures.bytes.timestamp]});
make('wrong-context-component',{context:{product:'pinvou',component:'engine'}});
make('wrong-context-product',{context:{product:'other-product',component:'app'}});
make('context-extras-positive',{context:{product:'pinvou',component:'app',ignored:{value:1}}});
for(const field of ['size','version','expiresAt']){
 make('snapshot-target-ref-'+field,{entryTransform:entries=>{
  const copy=structuredClone(entries);copy[0][field]++;return copy;
 }});
}
make('snapshot-member-hash-missing',{entryTransform:entries=>{
 const copy=structuredClone(entries);copy[0].envelopeSha256='b'.repeat(64);return copy;
}});
make('target-release-reference-size',{targetTransform:t=>{t.baselineRelease.size++;}});
make('target-release-reference-scope',{targetTransform:t=>{t.baselineRelease.scope.releaseId='different-release';}});
make('release-package-reference-size',{releaseTransform:r=>{r.targets[0].packageManifest.size++;}});
const armRoot=structuredClone(fixtures.metadata.root);
const armPolicy=structuredClone(armRoot.roles.find(p=>p.role==='package'));
armPolicy.targetKey='linux-arm64-deb-perMachine';armRoot.roles.push(armPolicy);
make('public-target-mismatch',{trustedRoot:armRoot,mutate:(t,r,p)=>{
 p.scope.targetKey='linux-arm64-deb-perMachine';
 for(const helper of p.helpers)helper.targetKey=p.scope.targetKey;
 r.targets[0].targetKey=p.scope.targetKey;
}});
make('timestamp-snapshot-ref-size',{timestampTransform:t=>{t.snapshot.size++;}});
make('timestamp-snapshot-ref-hash',{timestampTransform:t=>{t.snapshot.envelopeSha256='b'.repeat(64);}});
const newerPackage=structuredClone(fixtures.metadata.package);
newerPackage.appVersion='3.0.0';newerPackage.fullPackage.packageId='package-3';
const newerPackageBytes=fixtures.sign(newerPackage);
const newerRelease=structuredClone(fixtures.metadata.release);
newerRelease.scope.releaseId='release-3';newerRelease.appVersion='3.0.0';
newerRelease.targets[0].packageManifest=envelopeReference(newerPackageBytes);
const newerReleaseBytes=fixtures.sign(newerRelease);
const expandedMembers=m=>[...m,newerReleaseBytes,newerPackageBytes];
const expandedEntries=e=>[...e,envelopeReference(newerReleaseBytes),envelopeReference(newerPackageBytes)];
make('unreachable-candidate-leak',{memberTransform:expandedMembers,entryTransform:expandedEntries});
make('ordinary-path-reachable-positive',{memberTransform:expandedMembers,entryTransform:expandedEntries,
 targetTransform:t=>{t.ordinaryPaths=[{deploymentId:'deployment-3',deploymentRevision:1,
  approvalId:'path-3',approvalRevision:1,release:envelopeReference(newerReleaseBytes)}];}});
make('bridge-path-reachable-positive',{memberTransform:expandedMembers,entryTransform:expandedEntries,
 targetTransform:t=>{t.bridgePaths=[{deploymentId:'deployment-3',deploymentRevision:2,
  deploymentState:'superseded',eligibilityId:'bridge-3',eligibilityRevision:1,
  release:envelopeReference(newerReleaseBytes)}];}});
process.stdout.write(JSON.stringify(output));
