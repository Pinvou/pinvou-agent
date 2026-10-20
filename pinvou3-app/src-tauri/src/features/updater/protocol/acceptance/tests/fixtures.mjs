// Real ephemeral QA signatures. Private keys never leave this Node process.
import { createHash } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import { join } from 'node:path';
const reference=pathToFileURL(process.argv[2]);
const {createFixtureSet,NOW}=await import(new URL('./tests/fixtures.mjs',reference));
const {signEnvelope,envelopeReference,verifyPublicChain}=await import(new URL('./signatures.mjs',reference)).then(async m=>({...m,...await import(new URL('./metadata-chain.mjs',reference))}));
const directory=process.argv[3];mkdirSync(directory);
const f=createFixtureSet(), engine=createFixtureSet({component:'engine',signingKeys:f.keys});
const root=structuredClone(f.metadata.root);root.roles.push(...engine.metadata.root.roles.filter(p=>p.role!=='root'));
// Two genuine ordinary-role keys allow a different valid signature envelope
// over the very same signed body (still an envelope high-water equivocation).
for(const role of root.roles) if(role.role!=='root') role.keyIds.push(f.descriptors[0].keyId);
const anchor=signEnvelope(root,f.keys.slice(0,2));writeFileSync(join(directory,'anchor.json'),anchor);
function bundle(name,version,{component='app',roots=[],mutateTimestamp=()=>{},missing=false,corrupt=false}={}) {
 const set=component==='app'?f:engine;
 const pkg=structuredClone(set.metadata.package),pkgBytes=set.sign(pkg);
 const release=structuredClone(set.metadata.release);release.revision=version;release.targets[0].packageManifest=envelopeReference(pkgBytes);
 const releaseBytes=set.sign(release);
 const target=structuredClone(set.metadata.target);target.version=version;target.baselineRelease=envelopeReference(releaseBytes);
 const targetBytes=set.sign(target);
 const snapshot=structuredClone(set.metadata.snapshot);snapshot.version=version;snapshot.entries=[targetBytes,releaseBytes,pkgBytes].map(envelopeReference);
 const snapshotBytes=set.sign(snapshot);
 const timestamp=structuredClone(set.metadata.timestamp);timestamp.version=version;timestamp.snapshot=envelopeReference(snapshotBytes);mutateTimestamp(timestamp);
 const timestampBytes=set.sign(timestamp);
 const members=[targetBytes,releaseBytes,pkgBytes];
 if (!missing&&!corrupt) verifyPublicChain({trustedRoot:root,product:'pinvou',component,now:NOW+1000,timestampBytes,snapshotBytes,members});
 const files={timestamp:timestampBytes,snapshot:snapshotBytes,target:targetBytes,release:releaseBytes,package:pkgBytes};
 for(const [role,bytes] of Object.entries(files)) writeFileSync(join(directory,`${name}-${role}.json`),corrupt&&role==='package'?Buffer.from('{}'):bytes);
 const request={product:'pinvou',component,suppliedNowMs:NOW+1000,roots,timestamp:`${name}-timestamp.json`,snapshot:`${name}-snapshot.json`,members:(missing?['target','release']:['target','release','package']).map(role=>`${name}-${role}.json`),deniedKeyIds:[],deniedByRole:{}};
 writeFileSync(join(directory,`${name}.json`),JSON.stringify(request));
}
bundle('v1',1);bundle('v2',2);bundle('v3',3);bundle('engine',1,{component:'engine'});
bundle('equiv2',2,{mutateTimestamp:t=>{t.issuedAt++;}});
bundle('missing',2,{missing:true});bundle('corrupt',2,{corrupt:true});
let previous=root;
for(const version of [2,3]) {
 const next=structuredClone(root);next.version=version;next.issuedAt=NOW+10;next.expiresAt=NOW+86400000;
 writeFileSync(join(directory,`root${version}.json`),signEnvelope(next,f.keys.slice(0,2)));previous=next;
}
bundle('root-chain',2,{roots:['root2.json','root3.json']});
const bad=structuredClone(root);bad.version=2;bad.issuedAt=NOW+10;bad.expiresAt=NOW+86400000;
bad.roles=bad.roles.filter(p=>p.component!=='engine');writeFileSync(join(directory,'root-no-engine.json'),signEnvelope(bad,f.keys.slice(0,2)));
bundle('root-no-engine',2,{roots:['root-no-engine.json']});
const rotationKeys=createFixtureSet().keys.slice(0,3);
const {publicKeyDescriptor}=await import(new URL('./signatures.mjs',reference));
const rotated=structuredClone(root);rotated.version=2;rotated.issuedAt=NOW+10;
const newDescriptors=rotationKeys.map(publicKeyDescriptor);rotated.keys.push(...newDescriptors);
rotated.roles.find(role=>role.role==='root').keyIds=newDescriptors.map(key=>key.keyId);
writeFileSync(join(directory,'rotation.json'),signEnvelope(rotated,[...f.keys.slice(0,2),...rotationKeys.slice(0,2)]));
writeFileSync(join(directory,'rotation-invalid.json'),signEnvelope(rotated,f.keys.slice(0,2)));
bundle('rotated',2,{roots:['rotation.json']});
bundle('rotation-invalid',2,{roots:['rotation-invalid.json']});
const originalTimestamp=JSON.parse(Buffer.from(f.bytes.timestamp).toString('utf8')).signed;
writeFileSync(join(directory,'same-body-other-signature.json'),signEnvelope(originalTimestamp,[f.keys[0]]));
process.stdout.write(JSON.stringify({pin:createHash('sha256').update(anchor).digest('hex'),now:NOW}));
