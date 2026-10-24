// Frozen contracts + both actual verifications + explicit normative inheritance.
import {pathToFileURL} from 'node:url';
import {sign as cryptoSign} from 'node:crypto';
const reference=pathToFileURL(process.argv[1]);
const {createFixtureSet,NOW}=await import(new URL('./tests/fixtures.mjs',reference));
const {assertApiContract}=await import(new URL('./api-registry.mjs',reference));
const {verifyEnvelope,signEnvelope,signingInput}=await import(reference);
const {parseJson,canonicalize}=await import(new URL('./canonical-json.mjs',reference));
const {sha256}=await import(new URL('./digests.mjs',reference));
const fixture=createFixtureSet(),clone=structuredClone,rows=[];
// Genuine signatures over deliberately legacy/malformed claims bypass only
// signing preflight, so these controls target the receiving verifier's schema.
const rawSignDownload=s=>canonicalize({signed:s,signatures:[{keyId:fixture.descriptors[3].keyId,
 signature:cryptoSign(null,signingInput(s),fixture.keys[3]).toString('base64url')}]});
const inherited=['product','component','scope','installId','decisionId','decisionRevision','telemetrySessionId',
 'currentVersion','host','selectionGeneration','metadataSet','registryVersion','sourceProfileId',
 'clientFactsDigest','updaterFactsDigest','helperFactsDigest','launcherFactsDigest','forwardPath','update'];
const equal=(a,b)=>Buffer.from(canonicalize(a)).equals(Buffer.from(canonicalize(b)));
const refuse=code=>{const error=new Error(code);error.code=code;throw error;};
function verify(requestBytes,responseBytes,trustedRoot,now,deniedKeyIds) {
 const request=parseJson(requestBytes);assertApiContract('download-info','request',request);
 const response=parseJson(responseBytes);assertApiContract('download-info','response',response);
 const scope=request.scope,expectedScope={installationScopeId:scope.installationScopeId,channel:scope.channel,
 channelRevision:scope.channelRevision,targetKey:scope.targetKey};
 for(const [object,role]of [[request.decision,'decision'],[response.credential,'download']])
  verifyEnvelope(canonicalize(object),{trustedRoot,expected:{role,product:scope.product,component:scope.component,scope:expectedScope},now,deniedKeyIds});
 const old=request.decision.signed,current=response.credential.signed;
 if(!old.updateAvailable||request.packageId!==old.update.hopChain.packageId||current.packageId!==request.packageId)
  refuse('DOWNLOAD_REQUEST_BINDING_MISMATCH');
 for(const field of inherited)if(!equal(old[field],current[field]))refuse('DOWNLOAD_REQUEST_BINDING_MISMATCH');
 if(now>=response.expiresAt)refuse('DOWNLOAD_URL_EXPIRED');
}
function make(group,name,{decisionChange=()=>{},downloadChange=()=>{},rootChange=()=>{},requestChange=()=>{},
 envelopeChange=()=>{},responseChange=()=>{},requestWire=b=>b,responseWire=b=>b,now=NOW,deniedKeyIds=[],
 signDecision=s=>fixture.sign(s),signDownload=s=>fixture.sign(s),accepted=false}={}) {
 const root=clone(fixture.metadata.root);rootChange(root);
 const decision=clone(fixture.claims.decision),download=clone(fixture.claims.download);
 download.sourceBinding.urlSha256=sha256(Buffer.from('https://files.example.invalid/artifact?ticket=public-test','utf8'));
 decisionChange(decision);downloadChange(download);
 const request={protocolVersion:1,scope:{product:'pinvou',component:'app',installId:'install-1',...clone(fixture.claims.decision.scope)},
 decision:parseJson(signDecision(decision)),downloadKey:'download-key-1',packageId:'package-2'};requestChange(request);
 const response={protocolVersion:1,requestId:'request-independent',credential:parseJson(signDownload(download)),
 packageId:download.packageId,size:download.package.size,sha256:download.package.sha256,
 url:'https://files.example.invalid/artifact?ticket=public-test',expiresAt:download.exp,fileSourceClass:'public-test'};
 envelopeChange(request,response);responseChange(response);
 const requestBytes=requestWire(Buffer.from(canonicalize(request))),responseBytes=responseWire(Buffer.from(canonicalize(response)));
 let code=null;try{verify(requestBytes,responseBytes,root,now,deniedKeyIds);}catch(e){if(typeof e.code!=='string')throw e;code=e.code;}
 if((code===null)!==accepted)throw new Error(name+': unexpected composed result '+code);
 rows.push({group,name,accepted,code,trustedRoot:root,now,deniedKeyIds,
 requestBytes:Buffer.from(requestBytes).toString('base64'),responseBytes:Buffer.from(responseBytes).toString('base64')});
}
make('positive','signed-download-info',{accepted:true});
make('positive','transport-whitespace',{responseWire:b=>Buffer.from(' '+JSON.stringify(JSON.parse(b),null,2)+'\n'),accepted:true});
make('positive','distinct-role-jti-and-key',{rootChange:r=>{r.roles.find(p=>p.role==='download').keyIds=[fixture.descriptors[2].keyId];},
 downloadChange:d=>{d.jti='new-download-jti';d.signingKeyId=fixture.descriptors[2].keyId;},signDownload:s=>signEnvelope(s,[fixture.keys[2]]),accepted:true});
make('positive','shorter-url-window',{responseChange:r=>{r.expiresAt=NOW+1000;},accepted:true});
make('positive','independent-transport-request-id',{responseChange:r=>{r.requestId='another-request';},accepted:true});
for(const field of ['installId','decisionId','decisionRevision','telemetrySessionId','currentVersion','selectionGeneration','registryVersion',
 'sourceProfileId','clientFactsDigest','updaterFactsDigest','helperFactsDigest','launcherFactsDigest']) {
 make('inheritance','inherited-'+field,{downloadChange:d=>{
 d[field]=typeof d[field]==='number'?d[field]+1:field==='currentVersion'?'1.0.1':field.endsWith('Digest')||field==='sourceProfileId'?'b'.repeat(64):'foreign-identity';}});
}
make('inheritance','inherited-host',{downloadChange:d=>{d.host.osVersion='24.04';}});
make('inheritance','inherited-metadata-set',{downloadChange:d=>{d.metadataSet.timestamp.envelopeSha256='b'.repeat(64);}});
make('inheritance','inherited-forward-path',{downloadChange:d=>{d.forwardPath={policyId:'policy-2',revision:2,sha256:'b'.repeat(64),edgeSha256:'c'.repeat(64)};}});
make('inheritance','inherited-update',{downloadChange:d=>{d.update.workflowId='foreign-workflow';}});
for(const field of ['product','component'])make('context','download-'+field,{downloadChange:d=>{d[field]=field==='product'?'foreign-product':'engine';}});
for(const [field,value]of Object.entries({installationScopeId:'other-scope',channel:'beta',channelRevision:2,targetKey:'linux-arm64-deb-perMachine'})) {
 make('context','download-scope-'+field,{downloadChange:d=>{
 d.scope[field]=value;
 if(field==='channel')d.metadataSet.target.scope.channel=value;
 if(field==='targetKey'){d.host.arch='arm64';d.metadataSet.target.scope.targetKey=value;
 for(const c of [d.update.endpointChain,d.update.hopChain])c.packageManifest.scope.targetKey=value;}
 }});
}
make('context','independent-request-scope',{requestChange:r=>{r.scope.installationScopeId='other-scope';}});
make('package','requested-other-package',{requestChange:r=>{r.packageId='another-package';}});
make('package','decision-no-update',{decisionChange:d=>{d.updateAvailable=false;d.update=null;d.reason='UP_TO_DATE';}});
make('package','download-another-hop-package',{downloadChange:d=>{d.packageId='another-package';d.update.packageId='another-package';
 d.update.hopChain.packageId='another-package';d.update.endpointChain.packageId='another-package';}});
for(const field of ['packageId','size','sha256','expiresAt'])make('package','outer-'+field,{responseChange:r=>{
 r[field]=field==='size'?r.size+1:field==='expiresAt'?r.credential.signed.exp+1:field==='sha256'?'b'.repeat(64):'foreign-package';}});
const threshold=r=>{for(const role of ['decision','download']){const p=r.roles.find(p=>p.role===role);p.keyIds=fixture.descriptors.slice(2).map(d=>d.keyId);p.threshold=2;}};
const two=s=>signEnvelope(s,fixture.keys.slice(2));
make('signature','both-actual-two-signer-thresholds',{rootChange:threshold,signDecision:two,signDownload:two,accepted:true});
make('signature','decision-threshold-unmet',{rootChange:threshold,signDownload:two});
make('signature','download-threshold-unmet',{rootChange:threshold,signDecision:two});
for(const field of ['decision','credential']) {
 const get=(request,response)=>field==='decision'?request.decision:response.credential;
 make('signature',field+'-invalid-signature',{envelopeChange:(q,r)=>{get(q,r).signatures[0].signature=Buffer.alloc(64).toString('base64url');}});
 make('signature',field+'-bad-extra-after-valid-threshold',{rootChange:r=>{threshold(r);r.roles.find(p=>p.role==='decision').threshold=1;r.roles.find(p=>p.role==='download').threshold=1;},
 signDecision:two,signDownload:two,envelopeChange:(q,r)=>{get(q,r).signatures.at(-1).signature=Buffer.alloc(64).toString('base64url');}});
 make('signature',field+'-duplicate-signature',{envelopeChange:(q,r)=>{get(q,r).signatures.push(clone(get(q,r).signatures[0]));}});
}
make('signature','global-key-denied',{deniedKeyIds:[fixture.descriptors[3].keyId]});
make('signature','unused-key-denial',{deniedKeyIds:[fixture.descriptors[0].keyId],accepted:true});
make('signature','wrong-download-role-policy',{rootChange:r=>{r.roles.find(p=>p.role==='download').channel='beta';}});
make('window','expired-request-decision',{now:fixture.claims.decision.exp,
 downloadChange:d=>{d.iat=fixture.claims.decision.exp;d.nbf=d.iat;d.exp=d.iat+900000;}});
make('window','new-download-at-expiry',{now:NOW+1000,downloadChange:d=>{d.exp=NOW+1000;}});
make('window','download-future-nbf',{downloadChange:d=>{d.nbf=NOW+1;}});
make('window','url-at-expiry',{now:NOW+1000,responseChange:r=>{r.expiresAt=NOW+1000;}});
make('window','url-last-valid-millisecond',{now:NOW+999,responseChange:r=>{r.expiresAt=NOW+1000;},accepted:true});
make('window','download-last-valid-millisecond',{now:NOW+999,downloadChange:d=>{d.exp=NOW+1000;},accepted:true});
make('window','expired-root',{rootChange:r=>{r.issuedAt=NOW-1;r.expiresAt=NOW;}});
make('window','future-root',{now:NOW-1});
make('wire','request-closed-schema',{requestChange:r=>{r.unexpected=true;}});
make('wire','response-closed-schema',{responseChange:r=>{r.unexpected=true;}});
make('wire','credential-closed-schema',{envelopeChange:(q,r)=>{r.credential.signed.unexpected=true;}});
make('wire','duplicate-outer-key',{responseWire:b=>Buffer.from('{"protocolVersion":1,'+b.toString().slice(1))});
make('wire','response-over-one-mib',{responseWire:b=>Buffer.concat([b,Buffer.alloc(1048577-b.length,32)])});
make('wire','embedded-over-64k',{envelopeChange:(q,r)=>{const sig=clone(r.credential.signatures[0]);while(canonicalize(r.credential).length<=65536)r.credential.signatures.push(clone(sig));}});

make('source','url-replaced-without-new-signature',{responseChange:r=>{r.url='https://files.example.invalid/other';}});
make('source','fresh-signed-url-same-object',{downloadChange:d=>{d.sourceBinding.urlSha256=sha256(Buffer.from('https://files.example.invalid/artifact?ticket=rotated','utf8'));},responseChange:r=>{r.url='https://files.example.invalid/artifact?ticket=rotated';},accepted:true});
for(const field of ['sourceId','objectId','objectVersion'])make('source','new-signed-'+field,{downloadChange:d=>{d.sourceBinding[field]='another-object-record';},accepted:true});
make('source','missing-mandatory-binding',{downloadChange:d=>{delete d.sourceBinding;},signDownload:rawSignDownload});
make('source','unknown-binding-field',{downloadChange:d=>{d.sourceBinding.extra=true;},signDownload:rawSignDownload});
make('source','signed-wrong-url-hash',{downloadChange:d=>{d.sourceBinding.urlSha256='0'.repeat(64);}});

console.log(JSON.stringify(rows));
