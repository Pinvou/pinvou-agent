// Complete frozen contracts and actual signature verification, composed with
// explicit normative request-field bindings. No frozen full-operation helper exists.
import {pathToFileURL} from 'node:url';
const reference=pathToFileURL(process.argv[1]);
const {createFixtureSet,NOW,HASH}=await import(new URL('./tests/fixtures.mjs',reference));
const {verifyEnvelope,signEnvelope}=await import(reference);
const {assertApiContract}=await import(new URL('./api-registry.mjs',reference));
const {parseJson,canonicalize}=await import(new URL('./canonical-json.mjs',reference));
const {sourceProfileId}=await import(new URL('./digests.mjs',reference));
const fixture=createFixtureSet(),clone=structuredClone,rows=[];
const mismatch=()=>{const error=new Error('request binding');error.code='CHECK_REQUEST_BINDING_MISMATCH';throw error;};
const same=(a,b)=>Buffer.from(canonicalize(a)).equals(Buffer.from(canonicalize(b)));
function verify(operation,requestBytes,responseBytes,root,now,deniedKeyIds) {
 const request=parseJson(requestBytes);assertApiContract(operation,'request',request);
 const response=parseJson(responseBytes);assertApiContract(operation,'response',response);
 const {scope,facts}=request,stable=facts.stableFacts;
 for(const field of ['product','component','targetKey'])if(stable[field]!==scope[field])mismatch();
 const profile=sourceProfileId(stable);
 const expectedScope={installationScopeId:scope.installationScopeId,channel:scope.channel,
 channelRevision:scope.channelRevision,targetKey:scope.targetKey};
 for(const [field,role]of [['decision','decision'],['telemetryCredential','telemetry-event']])
  verifyEnvelope(canonicalize(response[field]),{trustedRoot:root,expected:{role,product:scope.product,
  component:scope.component,scope:expectedScope},now,deniedKeyIds});
 const claims=response.decision.signed;
 if(claims.requestNonce!==request.requestNonce||claims.installId!==scope.installId
  ||claims.currentVersion!==stable.canonicalAppVersion||claims.sourceProfileId!==profile)mismatch();
 for(const field of ['host','clientFactsDigest','updaterFactsDigest','helperFactsDigest','launcherFactsDigest'])
  if(!same(claims[field],facts[field]))mismatch();
 if(operation==='refresh'&&claims.updateAvailable&&claims.update.workflowId!==request.workflowId)mismatch();
}
function make(group,name,{operation='check',requestChange=()=>{},decisionChange=()=>{},telemetryChange=()=>{},
 rootChange=()=>{},envelopeChange=()=>{},responseChange=()=>{},requestWire=b=>b,responseWire=b=>b,
 signDecision=s=>fixture.sign(s),signTelemetry=s=>fixture.sign(s),now=NOW,deniedKeyIds=[],accepted=false}={}) {
 const root=clone(fixture.metadata.root);rootChange(root);
 const decision=clone(fixture.claims.decision),telemetry=clone(fixture.claims['telemetry-event']);
 const request={protocolVersion:1,scope:{product:'pinvou',component:'app',installId:decision.installId,...clone(decision.scope)},
 requestNonce:decision.requestNonce,facts:{stableFacts:clone(fixture.sourceFacts),host:clone(decision.host),
 clientFactsDigest:HASH,updaterFactsDigest:HASH,helperFactsDigest:HASH,launcherFactsDigest:HASH},
 grayTargeting:{mode:'disabled',serialNumber:null}};
 decision.sourceProfileId=sourceProfileId(request.facts.stableFacts);
 if(operation==='check')Object.assign(request,{executionPurpose:'install',supportedIncrementalAlgorithms:[],supportedIncrementalFormats:[]});
 else Object.assign(request,{workflowId:decision.update.workflowId,expectedWorkflowRevision:4,
 previousSessionId:decision.telemetrySessionId,immutableContextSha256:HASH,decision:parseJson(fixture.sign(decision))});
 requestChange(request);decisionChange(decision);telemetryChange(telemetry);
 const response={protocolVersion:1,requestId:'transport-request',decision:parseJson(signDecision(decision)),
 telemetryCredential:parseJson(signTelemetry(telemetry))};
 if(operation==='refresh')Object.assign(response,{workflowRevision:5,writableSessionId:decision.telemetrySessionId});
 envelopeChange(response);responseChange(response);
 const requestBytes=requestWire(Buffer.from(canonicalize(request))),responseBytes=responseWire(Buffer.from(canonicalize(response)));
 let code=null;try {verify(operation,requestBytes,responseBytes,root,now,deniedKeyIds);}
 catch(error){if(typeof error.code!=='string')throw error;code=error.code;}
 if((code===null)!==accepted)throw new Error(name+': unexpected composed result '+code);
 rows.push({group,name,operation,accepted,code,trustedRoot:root,now,deniedKeyIds,
 requestBytes:Buffer.from(requestBytes).toString('base64'),responseBytes:Buffer.from(responseBytes).toString('base64')});
}
make('positive','signed-check-update',{accepted:true});
make('positive','signed-check-no-update',{decisionChange:d=>{d.updateAvailable=false;d.update=null;d.reason='UP_TO_DATE';},accepted:true});
make('positive','signed-refresh',{operation:'refresh',accepted:true});
make('positive','refresh-no-invented-revision-increment',{operation:'refresh',responseChange:r=>{r.workflowRevision=9007199254740991;},accepted:true});
make('positive','refresh-no-update',{operation:'refresh',decisionChange:d=>{d.updateAvailable=false;d.update=null;d.reason='UP_TO_DATE';},accepted:true});
const old=createFixtureSet({clock:NOW-30*86400000,signingKeys:fixture.keys});
make('positive','expired-prior-refresh-decision',{operation:'refresh',requestChange:r=>{r.decision=parseJson(old.sign(old.claims.decision));},accepted:true});
make('positive','unverified-old-refresh-signature',{operation:'refresh',requestChange:r=>{r.decision.signatures[0].signature=Buffer.alloc(64).toString('base64url');},accepted:true});
make('positive','whitespace-unordered-transport',{responseWire:b=>Buffer.from(' '+JSON.stringify(JSON.parse(b),null,2)+'\n'),accepted:true});
for(const field of ['requestNonce','currentVersion','sourceProfileId'])
 make('facts','decision-'+field,{decisionChange:d=>{d[field]=field==='sourceProfileId'?'b'.repeat(64):field==='currentVersion'?'1.0.1':'other-nonce';}});
for(const field of ['clientFactsDigest','updaterFactsDigest','helperFactsDigest','launcherFactsDigest'])
 make('facts','decision-'+field,{decisionChange:d=>{d[field]='b'.repeat(64);}});
for(const field of ['os','arch','osVersion'])
 make('facts','request-host-'+field,{requestChange:r=>{r.facts.host[field]=field==='os'?'windows':field==='arch'?'arm64':'24.04';}});
make('facts','signed-pair-wrong-install',{decisionChange:d=>{d.installId='other-install';},telemetryChange:t=>{t.installId='other-install';}});
for(const field of ['product','component','targetKey'])
 make('facts','stable-request-'+field,{requestChange:r=>{r.facts.stableFacts[field]=field==='targetKey'?'linux-arm64-deb-perMachine':field==='component'?'engine':'foreign-product';}});
make('facts','refreshed-workflow-mismatch',{operation:'refresh',decisionChange:d=>{d.update.workflowId='foreign-workflow';}});
for(const field of ['product','component'])
 make('context','independent-'+field,{requestChange:r=>{const v=field==='product'?'foreign-product':'engine';r.scope[field]=v;r.facts.stableFacts[field]=v;}});
for(const [field,value]of Object.entries({installationScopeId:'other-scope',channel:'beta',channelRevision:2,targetKey:'linux-arm64-deb-perMachine'}))
 make('context','independent-scope-'+field,{requestChange:r=>{r.scope[field]=value;if(field==='targetKey')r.facts.stableFacts.targetKey=value;}});
for(const field of ['installId','decisionId','decisionRevision','telemetrySessionId','selectionGeneration'])
 make('pair','telemetry-'+field,{telemetryChange:t=>{t[field]=typeof t[field]==='number'?t[field]+1:'foreign-identity';}});
make('pair','refresh-writable-session',{operation:'refresh',responseChange:r=>{r.writableSessionId='foreign-session';}});
const threshold=root=>{for(const role of ['decision','telemetry-event']){const p=root.roles.find(p=>p.role===role);p.keyIds=fixture.descriptors.slice(2).map(d=>d.keyId);p.threshold=2;}};
const two=s=>signEnvelope(s,fixture.keys.slice(2));
make('signature','actual-dual-credential-thresholds',{rootChange:threshold,signDecision:two,signTelemetry:two,accepted:true});
make('signature','decision-threshold-unmet',{rootChange:threshold,signTelemetry:two});
make('signature','telemetry-threshold-unmet',{rootChange:threshold,signDecision:two});
for(const field of ['decision','telemetryCredential']) {
 make('signature',field+'-invalid-signature',{envelopeChange:r=>{r[field].signatures[0].signature=Buffer.alloc(64).toString('base64url');}});
 make('signature',field+'-bad-extra-after-valid-threshold',{rootChange:r=>{threshold(r);r.roles.find(p=>p.role==='decision').threshold=1;r.roles.find(p=>p.role==='telemetry-event').threshold=1;},
 signDecision:two,signTelemetry:two,envelopeChange:r=>{r[field].signatures.at(-1).signature=Buffer.alloc(64).toString('base64url');}});
 make('signature',field+'-duplicate-signature',{envelopeChange:r=>{r[field].signatures.push(clone(r[field].signatures[0]));}});
}
make('signature','global-key-denied',{deniedKeyIds:[fixture.descriptors[3].keyId]});
make('signature','unused-key-denied-positive',{deniedKeyIds:[fixture.descriptors[0].keyId],accepted:true});
make('signature','wrong-exact-role-policy',{rootChange:r=>{r.roles.find(p=>p.role==='telemetry-event').channel='beta';}});
for(const field of ['decision','telemetryCredential']) {
 make('window',field+'-future-nbf',{envelopeChange:r=>{const c=r[field].signed;c.nbf=NOW+1;r[field]=parseJson(fixture.sign(c));}});
 make('window',field+'-at-expiry',{now:field==='decision'?fixture.claims.decision.exp:NOW+1000,
 telemetryChange:t=>{if(field==='telemetryCredential')t.exp=NOW+1000;}});
 make('window',field+'-last-valid-millisecond',{now:field==='decision'?fixture.claims.decision.exp-1:NOW+999,
 telemetryChange:t=>{if(field==='telemetryCredential')t.exp=NOW+1000;},accepted:true});
}
make('window','expired-root',{rootChange:r=>{r.issuedAt=NOW-1;r.expiresAt=NOW;}});
make('window','future-root',{now:NOW-1});
make('wire','outer-response-closed-schema',{responseChange:r=>{r.unexpected=true;}});
make('wire','request-closed-schema',{requestChange:r=>{r.unexpected=true;}});
make('wire','credential-closed-schema',{envelopeChange:r=>{r.telemetryCredential.signed.unexpected=true;}});
make('wire','check-incremental-contract-refuses',{requestChange:r=>{r.supportedIncrementalAlgorithms=['test-delta'];}});
make('wire','duplicate-outer-json-key',{responseWire:b=>Buffer.from('{"protocolVersion":1,'+b.toString().slice(1))});
make('wire','response-over-one-mib',{responseWire:b=>Buffer.concat([b,Buffer.alloc(1048577-b.length,32)])});
make('wire','embedded-over-64k',{envelopeChange:r=>{const sig=clone(r.telemetryCredential.signatures[0]);while(canonicalize(r.telemetryCredential).length<=65536)r.telemetryCredential.signatures.push(clone(sig));}});
console.log(JSON.stringify(rows));
