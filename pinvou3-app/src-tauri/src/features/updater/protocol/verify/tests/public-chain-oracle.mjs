import {pathToFileURL} from 'node:url';
const reference=pathToFileURL(process.argv[1]);
const {createFixtureSet,NOW}=await import(new URL('./tests/fixtures.mjs',reference));
const {verifyEnvelope,envelopeReference,signEnvelope}=await import(reference);
const {canonicalize}=await import(new URL('./canonical-json.mjs',reference));
const {verifyPublicChain}=await import(new URL('./metadata-chain.mjs',reference));
const fixtures=createFixtureSet();
const output=[];
let group='relationships';
function make(name,{mutate=()=>{},memberTransform=(m)=>m,entryTransform=(e)=>e,
 trustedRoot=fixtures.metadata.root,releaseTransform=()=>{},targetTransform=()=>{},timestampTransform=()=>{},context={product:'pinvou',component:'app'}, preflight=true, now=NOW, deniedKeyIds=[], deniedKeyIdsByRole={}, sign=fixtures.sign, wireTransform=(role,bytes)=>bytes}={}) {
 const packageManifest=structuredClone(fixtures.metadata.package);
 const release=structuredClone(fixtures.metadata.release);
 const target=structuredClone(fixtures.metadata.target);
 mutate(target,release,packageManifest);
 const packageBytes=wireTransform('package',sign(packageManifest));
 release.targets[0].packageManifest=envelopeReference(packageBytes);
 releaseTransform(release);
 const releaseBytes=wireTransform('release',sign(release));
 target.baselineRelease=envelopeReference(releaseBytes);
 targetTransform(target);
 const targetBytes=wireTransform('target',sign(target));
 const declared=[targetBytes,releaseBytes,packageBytes];
 const members=memberTransform(declared);
 const snapshot={...structuredClone(fixtures.metadata.snapshot),
  entries:entryTransform(declared.map(envelopeReference))};
 const snapshotBytes=wireTransform('snapshot',sign(snapshot));
 const timestamp={...structuredClone(fixtures.metadata.timestamp),snapshot:envelopeReference(snapshotBytes)};
 timestampTransform(timestamp);
 const timestampBytes=wireTransform('timestamp',sign(timestamp));
 // Authentication, role-policy scope, semantics and fixed fixture time must
 // independently pass BEFORE any expected graph rejection is accepted as proof.
 if(preflight) for(const bytes of [timestampBytes,snapshotBytes,...members]){
  const signed=JSON.parse(Buffer.from(bytes).toString('utf8')).signed;
  verifyEnvelope(bytes,{trustedRoot,now:NOW,
   expected:{role:signed.role,product:'pinvou',component:'app',scope:signed.scope}});
 }
 const args={trustedRoot,...context,now,timestampBytes,snapshotBytes,members,deniedKeyIds,deniedKeyIdsByRole};
 let accepted; let code=null;
 try{verifyPublicChain(args);accepted=true;}catch(error){if(typeof error.code!=='string')throw error;accepted=false;code=error.code;}
 output.push({group,name,accepted,code,trustedRoot,now,context,deniedKeyIds,deniedKeyIdsByRole,
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


// Membership shape and multiple independent target-policy controls.
make('duplicate-within-reference-count',{memberTransform:m=>[m[0],m[0],m[2]]});
make('repeated-identical-snapshot-reference-refuses',{preflight:false,entryTransform:e=>[...e,e[0]]});
make('unordered-actual-members-positive',{memberTransform:m=>[...m].reverse()});
function multiTarget(name, brokenPolicy=false, brokenReference=false) {
 const arm='linux-arm64-deb-perMachine';
 const root=structuredClone(fixtures.metadata.root);
 for(const role of ['target','package']) {
  const policy=structuredClone(root.roles.find(p=>p.role===role));
  policy.targetKey=arm;
  if(role==='target'&&brokenPolicy)policy.channel='beta';
  root.roles.push(policy);
 }
 const pkg=structuredClone(fixtures.metadata.package);pkg.scope.targetKey=arm;
 for(const helper of pkg.helpers)helper.targetKey=arm;
 const pkgBytes=fixtures.sign(pkg);let targetBytes;
 make(name,{trustedRoot:root,preflight:!brokenPolicy&&!brokenReference,
  mutate:(t,r)=>{
   const target=structuredClone(r.targets[0]);target.targetKey=arm;
   target.packageManifest=envelopeReference(pkgBytes);r.targets.push(target);
  },
  targetTransform:t=>{
   const other=structuredClone(t);other.scope.targetKey=arm;other.hostArchitectures=['arm64'];targetBytes=fixtures.sign(other);
  },
  memberTransform:m=>[m[2],targetBytes,m[0],m[1],pkgBytes],
  entryTransform:e=>{
   const other=envelopeReference(targetBytes);
   if(brokenReference)other.scope.channel='beta';
   return [...e,other,envelopeReference(pkgBytes)];
  },
 });
}
multiTarget('two-targets-two-packages-unordered-positive');
multiTarget('second-target-independent-policy-refuses',true);
multiTarget('second-target-complete-reference-scope-refuses',false,true);

// Actual full-operation signature/time/deny controls, not extracted fragments.
group='denial';
const keyId=fixtures.descriptors[3].keyId;
make('global-deny-refuses',{preflight:false,deniedKeyIds:[keyId]});
for(const role of ['timestamp','snapshot','target','release','package']) {
 make(role+'-exact-role-deny-refuses',{preflight:false,deniedKeyIdsByRole:{[role]:[keyId]}});
}
make('unrelated-role-deny-is-ignored',{deniedKeyIdsByRole:{download:[keyId]}});
make('unused-root-key-global-deny-is-ignored',{deniedKeyIds:[fixtures.descriptors[0].keyId]});
const isolatedRoot=structuredClone(fixtures.metadata.root);
isolatedRoot.roles.find(p=>p.role==='target').keyIds=[fixtures.descriptors[2].keyId];
make('target-key-denied-only-for-release-still-passes',{trustedRoot:isolatedRoot,
 sign:signed=>signEnvelope(signed,[fixtures.keys[signed.role==='target'?2:3]]),
 deniedKeyIdsByRole:{release:[fixtures.descriptors[2].keyId]}});
group='signatures';
const thresholdRoot=structuredClone(fixtures.metadata.root);
const thresholdPolicy=thresholdRoot.roles.find(p=>p.role==='target');
thresholdPolicy.keyIds=[fixtures.descriptors[2].keyId,keyId];thresholdPolicy.threshold=2;
const twoSigners=signed=>signEnvelope(signed,signed.role==='target'?fixtures.keys.slice(2):[fixtures.keys[3]]);
make('target-two-real-signatures-positive',{trustedRoot:thresholdRoot,sign:twoSigners});
make('target-threshold-unmet',{trustedRoot:thresholdRoot,preflight:false});
const extraRoot=structuredClone(thresholdRoot);
extraRoot.roles.find(p=>p.role==='target').threshold=1;
const transformTarget=(mutate)=>(role,bytes)=>{
 if(role!=='target')return bytes;
 const object=JSON.parse(Buffer.from(bytes));mutate(object);
 return Buffer.from(canonicalize(object));
};
make('bad-extra-after-valid-threshold-refuses',{trustedRoot:extraRoot,sign:twoSigners,preflight:false,
 wireTransform:transformTarget(e=>{e.signatures.at(-1).signature=Buffer.alloc(64).toString('base64url');})});
make('known-descriptor-outside-exact-role-refuses',{preflight:false,
 sign:signed=>signEnvelope(signed,signed.role==='target'?[fixtures.keys[0],fixtures.keys[3]]:[fixtures.keys[3]])});
make('unsigned-body-change-refuses',{preflight:false,
 wireTransform:transformTarget(e=>{e.signed.selectionGeneration++;})});
group='time';
make('expired-root-refuses',{preflight:false,now:fixtures.metadata.root.expiresAt});
make('future-root-refuses',{preflight:false,now:fixtures.metadata.root.issuedAt-1});
make('expired-timestamp-refuses',{preflight:false,now:fixtures.metadata.timestamp.expiresAt});
group='wire_policy';
const wrongPolicy=structuredClone(fixtures.metadata.root);
wrongPolicy.roles.find(p=>p.role==='target').channel='beta';
make('wrong-exact-channel-policy-refuses',{trustedRoot:wrongPolicy,preflight:false});
make('target-noncanonical-original-refuses',{preflight:false,
 wireTransform:(role,bytes)=>role==='target'?Buffer.concat([Buffer.from(bytes),Buffer.from(' ')]):bytes});
make('target-closed-schema-refuses',{preflight:false,
 wireTransform:transformTarget(e=>{e.signed.unrecognized=true;})});
make('timestamp-original-input-limit-refuses',{preflight:false,
 wireTransform:(role,bytes)=>role==='timestamp'?Buffer.concat([Buffer.from(bytes),Buffer.alloc(1048577-bytes.length,32)]):bytes});
make('production-incremental-package-refuses',{preflight:false,mutate:(t,r,p)=>{
 p.incrementalPackages=[{packageId:'delta-2',size:1,sha256:'a'.repeat(64),baseVersion:'1.0.0',
 baseArtifactSha256:'b'.repeat(64),resultArtifactSize:123,resultArtifactSha256:'a'.repeat(64),
 algorithm:'test-delta',algorithmVersion:'1',format:'test-delta'}];
}});
process.stdout.write(JSON.stringify(output));
