// Compose three unchanged full original operations; private keys stay in Node.
import {pathToFileURL} from 'node:url';
import {generateKeyPairSync} from 'node:crypto';
const reference=pathToFileURL(process.argv[1]);
const {createFixtureSet,NOW}=await import(new URL('./tests/fixtures.mjs',reference));
const {verifyArchivedRelease,verifyEnvelope,assertReleaseRenewal,signEnvelope,publicKeyDescriptor}=await import(reference);
const {canonicalize}=await import(new URL('./canonical-json.mjs',reference));
const DAY=86400000,old=createFixtureSet({clock:NOW-100*DAY}),clone=structuredClone,rows=[];
const currentKey=generateKeyPairSync('ed25519').privateKey,currentDescriptor=publicKeyDescriptor(currentKey);
function make(group,name,{previousChange=()=>{},nextChange=()=>{},archiveChange=()=>{},rootChange=()=>{},contextChange=()=>{},
 signPrevious=s=>old.sign(s),signNext=s=>signEnvelope(s,[currentKey]),previousWire=b=>b,nextWire=b=>b,
 now=NOW,deniedKeyIds=[],accepted=false}={}) {
 const archivedRoot=clone(old.metadata.root);archivedRoot.expiresAt=archivedRoot.issuedAt+DAY;archiveChange(archivedRoot);
 const currentRoot=clone(old.metadata.root);currentRoot.version=2;currentRoot.issuedAt=NOW;currentRoot.expiresAt=NOW+365*DAY;
 currentRoot.keys.push(currentDescriptor);currentRoot.roles.find(p=>p.role==='release').keyIds=[currentDescriptor.keyId];rootChange(currentRoot);
 const previous=clone(old.metadata.release),next=clone(previous);next.revision=2;next.issuedAt=NOW;next.expiresAt=NOW+90*DAY;
 previousChange(previous);nextChange(next);
 const context={product:'pinvou',component:'app',releaseId:'release-2'};contextChange(context);
 const previousBytes=previousWire(signPrevious(previous)),nextBytes=nextWire(signNext(next));
 const expected={role:'release',product:context.product,component:context.component,scope:{releaseId:context.releaseId}};
 let code=null;try{
  const before=verifyArchivedRelease(previousBytes,{archivedRoot,expected}).signed;
  const after=verifyEnvelope(nextBytes,{trustedRoot:currentRoot,expected,now,deniedKeyIds}).signed;
  assertReleaseRenewal(before,after);
 }catch(error){if(typeof error.code!=='string')throw error;code=error.code;}
 if((code===null)!==accepted)throw new Error(name+': unexpected original composition '+code);
 rows.push({group,name,accepted,code,archivedRoot,currentRoot,context,now,deniedKeyIds,
 previousBytes:Buffer.from(previousBytes).toString('base64'),nextBytes:Buffer.from(nextBytes).toString('base64')});
}
make('renewal','expired-history-distinct-current-signer',{accepted:true});
make('renewal','unchanged-current-root-and-release-key',{
 archiveChange:r=>{r.expiresAt=old.metadata.root.expiresAt;},
 rootChange:r=>Object.assign(r,clone(old.metadata.root)),signNext:s=>old.sign(s),accepted:true});
make('renewal','nonconsecutive-higher-revision',{nextChange:r=>{r.revision=7;},accepted:true});
make('renewal','safe-maximum-revision',{nextChange:r=>{r.revision=9007199254740991;},accepted:true});
make('renewal','revision-equal',{nextChange:r=>{r.revision=1;}});
make('renewal','revision-decreased',{previousChange:r=>{r.revision=3;}});
make('renewal','future-historical-own-time-no-cross-order',{previousChange:r=>{r.issuedAt=NOW+1000;r.expiresAt=NOW+DAY;},accepted:true});
make('renewal','historical-key-denied-not-retroactive',{deniedKeyIds:[old.descriptors[3].keyId],accepted:true});
for(const [name,change]of [
 ['notes',r=>{r.notes.en='different business';}],['appVersion',r=>{r.appVersion='2.0.1';}],
 ['targetRevision',r=>{r.targets[0].releaseTargetRevision++;}],['minimumSource',r=>{r.targets[0].minimumSourceVersion='1.0.1';}],
 ['mode',r=>{r.targets[0].activationModes=['directInstall'];}],['launcher',r=>{r.targets[0].launcher.publisher='different-publisher';}],
 ['packageReference',r=>{r.targets[0].packageManifest.envelopeSha256='b'.repeat(64);}]
])make('business','immutable-'+name,{nextChange:change});
for(const field of ['product','component','releaseId'])make('context','independent-'+field,{contextChange:c=>{c[field]=field==='product'?'foreign-product':field==='component'?'engine':'foreign-release';}});
make('context','ignored-context-extra',{contextChange:c=>{c.ignored={value:1};},accepted:true});
make('context','previous-release-scope',{previousChange:r=>{r.scope.releaseId='foreign-release';}});
make('context','next-release-scope',{nextChange:r=>{r.scope.releaseId='foreign-release';}});
const transform=mutate=>bytes=>{const e=JSON.parse(Buffer.from(bytes));mutate(e);return Buffer.from(canonicalize(e));};
const historicalThreshold=r=>{const p=r.roles.find(p=>p.role==='release');p.keyIds=old.descriptors.slice(2).map(d=>d.keyId);p.threshold=2;};
const currentThreshold=r=>{const p=r.roles.find(p=>p.role==='release');p.keyIds=[currentDescriptor.keyId,old.descriptors[2].keyId];p.threshold=2;};
const historicalTwo=s=>signEnvelope(s,old.keys.slice(2)),currentTwo=s=>signEnvelope(s,[currentKey,old.keys[2]]);
make('signature','both-actual-independent-thresholds',{archiveChange:historicalThreshold,rootChange:currentThreshold,
 signPrevious:historicalTwo,signNext:currentTwo,accepted:true});
make('signature','historical-threshold-unmet',{archiveChange:historicalThreshold});
make('signature','current-threshold-unmet',{rootChange:currentThreshold});
for(const side of ['previous','next']) {
 const wire=side==='previous'?'previousWire':'nextWire';
 make('signature',side+'-invalid-signature',{[wire]:transform(e=>{e.signatures[0].signature=Buffer.alloc(64).toString('base64url');})});
 make('signature',side+'-bad-extra-after-valid-threshold',{
 archiveChange:r=>{historicalThreshold(r);r.roles.find(p=>p.role==='release').threshold=1;},
 rootChange:r=>{currentThreshold(r);r.roles.find(p=>p.role==='release').threshold=1;},
 signPrevious:historicalTwo,signNext:currentTwo,[wire]:transform(e=>{e.signatures.at(-1).signature=Buffer.alloc(64).toString('base64url');})});
 make('signature',side+'-duplicate-signature',{[wire]:transform(e=>{e.signatures.push(clone(e.signatures[0]));})});
}
make('signature','current-key-denied',{deniedKeyIds:[currentDescriptor.keyId]});
make('signature','historical-wrong-role-policy',{archiveChange:r=>{r.roles.find(p=>p.role==='release').component='engine';}});
make('signature','current-wrong-role-policy',{rootChange:r=>{r.roles.find(p=>p.role==='release').component='engine';}});
make('window','current-root-expired',{rootChange:r=>{r.issuedAt=NOW-1;r.expiresAt=NOW;}});
make('window','current-root-future',{rootChange:r=>{r.issuedAt=NOW+1;}});
make('window','next-release-expired',{nextChange:r=>{r.issuedAt=NOW-DAY;r.expiresAt=NOW;}});
make('window','next-release-future',{nextChange:r=>{r.issuedAt=NOW+1;}});
make('window','historical-original-window-invalid',{previousChange:r=>{r.expiresAt=r.issuedAt;}});
make('window','archived-root-original-window-invalid',{archiveChange:r=>{r.expiresAt=r.issuedAt;}});
make('window','next-release-last-valid-millisecond',{now:NOW+DAY-1,nextChange:r=>{r.expiresAt=NOW+DAY;},accepted:true});
for(const side of ['previous','next']) {
 const wire=side==='previous'?'previousWire':'nextWire';
 make('wire',side+'-noncanonical',{[wire]:b=>Buffer.concat([Buffer.from(b),Buffer.from(' ')])});
 make('wire',side+'-closed-schema',{[wire]:transform(e=>{e.signed.unexpected=true;})});
 make('wire',side+'-over-one-mib',{[wire]:b=>Buffer.concat([Buffer.from(b),Buffer.alloc(1048577-b.length,32)])});
}
make('wire','revision-not-safe',{nextWire:transform(e=>{e.signed.revision=9007199254740992;})});
console.log(JSON.stringify(rows));
