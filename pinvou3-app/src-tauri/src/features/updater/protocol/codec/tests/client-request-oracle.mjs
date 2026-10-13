// Actual complete client request contract; no verifier or current authority claim.
import assert from 'node:assert/strict';
import{pathToFileURL}from'node:url';
const root=pathToFileURL(process.argv[1]);
const{OPERATIONS}=await import(new URL('./build-api.mjs',root));
const{assertApiContract,assertApiShape}=await import(new URL('./api-registry.mjs',root));
const{parseJson,canonicalize}=await import(new URL('./canonical-json.mjs',root));
const{sha256,consumeRequestDigest}=await import(new URL('./digests.mjs',root));
const{checkSemantics}=await import(new URL('./semantics.mjs',root));
const{createFixtureSet,HASH,TARGET}=await import(new URL('./tests/fixtures.mjs',root));
const{consumeFixture}=await import(new URL('./tests/model-fixtures.mjs',root));
const f=createFixtureSet();
const envelope=role=>parseJson(f.sign(f.claims[role]));
const scope={product:'pinvou',component:'app',installId:'install-1',installationScopeId:'scope-1',
  channel:'stable',channelRevision:1,targetKey:TARGET};
const facts={stableFacts:f.sourceFacts,host:f.claims.decision.host,clientFactsDigest:HASH,
  updaterFactsDigest:HASH,helperFactsDigest:HASH,launcherFactsDigest:HASH};
const localFacts={factType:'local-state'};
function baseline(operation) {
  const v={protocolVersion:1,scope:structuredClone(scope)};
  const authPurpose=operation.split('-').at(-1);
  if(operation==='check'||operation==='refresh') {
    Object.assign(v,{requestNonce:'nonce-1',facts:structuredClone(facts),grayTargeting:{mode:'disabled',serialNumber:null}});
    if(operation==='check')Object.assign(v,{executionPurpose:'install',supportedIncrementalAlgorithms:[],supportedIncrementalFormats:[]});
    else Object.assign(v,{workflowId:'workflow-1',expectedWorkflowRevision:0,previousSessionId:'session-1',
      immutableContextSha256:HASH,decision:envelope('decision')});
  }else if(operation==='download-info') {
    Object.assign(v,{decision:envelope('decision'),downloadKey:'download-1',packageId:'package-2'});
  }else if(operation==='consume-status') {
    delete v.scope;
    Object.assign(v,{product:'pinvou',component:'app',installationScopeId:'scope-1',purpose:'install',
      authorizationJti:'jti-1',consumeKey:'consume-1',transactionId:'transaction-1',
      consumeRequestDigest:HASH,recoverySecret:Buffer.alloc(32).toString('base64url')});
  }else if(operation.startsWith('consume-')) {
    const {command}=consumeFixture(authPurpose);const{consumeRequestDigest:digest,...semanticRequest}=command.request;
    Object.assign(v,{authorization:parseJson(command.authorizationBytes),authorizationRevision:1,
      semanticRequest,consumeRequestDigest:digest});
  }else if(operation.startsWith('validate-')) {
    const a=f.claims['authorization-'+authPurpose];
    Object.assign(v,{decision:envelope('decision'),validateKey:'validate-1',facts:structuredClone(facts),
      preparation:structuredClone(a.preparation),freezeEpoch:a.freezeEpoch,backup:structuredClone(a.backup),
      staged:structuredClone(a.staged),preinstallSlot:structuredClone(a.preinstallSlot)});
  }else if(operation.startsWith('cancel-')) {
    const authorization=envelope('authorization-'+authPurpose);
    Object.assign(v,{authorization,cancelKey:'cancel-1',authorizationJti:authorization.signed.authorizationJti,
      authorizationRevision:1,reason:'user-stopped'});
  }else if(operation.startsWith('execution-review-')) {
    const authorization=envelope('authorization-'+authPurpose);const a=authorization.signed;
    Object.assign(v,{authorization,transactionId:a.transactionId,transactionRevision:1,facts:structuredClone(facts),
      verifiedObjectSha256:HASH,preparation:structuredClone(a.preparation),freezeEpoch:a.freezeEpoch,
      backup:structuredClone(a.backup),staged:structuredClone(a.staged),preinstallSlot:structuredClone(a.preinstallSlot)});
  }else if(operation==='helper-cleanup') {
    const a=f.claims['helper-cleanup'];
    Object.assign(v,{cleanupKey:'cleanup-1',facts:structuredClone(facts),preparation:structuredClone(a.preparation),
      deletionSet:structuredClone(a.deletionSet)});
  }else if(operation==='channel-change') {
    Object.assign(v,{changeKey:'change-1',expectedChannelRevision:1,newChannel:'beta',
      preparation:structuredClone(f.claims['authorization-install'].preparation)});
  }else if(operation.startsWith('events-')) {
    const credential=envelope(operation.slice('events-'.length)),c=credential.signed;
    Object.assign(v,{credential,events:[{eventId:'event-1',sequence:1,
      occurredAt:c.sessionStartedAt??c.transactionStartedAt??c.taskStartedAt,
      lineageId:c.telemetrySessionId??c.transactionId??c.task.taskId,
      executionPurpose:c.purpose==='telemetry'?'install':c.purpose,kind:c.allowedEvents[0],
      fromState:'test-from',toState:'test-to',facts:structuredClone(localFacts),
      factsSha256:sha256(canonicalize(localFacts)),targetIdentitySha256:c.purpose==='telemetry'?null:HASH}]});
  }else if(operation==='reconciliation-action'||operation==='recovery-review-action') {
    const credential=envelope(operation);
    Object.assign(v,{actionKey:'action-1',credential,taskRevision:credential.signed.task.revision,
      evidence:structuredClone(localFacts),evidenceSha256:sha256(canonicalize(localFacts))});
  }else throw new Error('missing original client request fixture');
  return v;
}
const cases=[];
function emit(operation,name,value) {
  let accepted=false,code=null;
  try{assertApiContract(operation,'request',value);accepted=true;}catch(error){code=error.code;}
  if(name.endsWith('positive'))assert.equal(accepted,true,operation+' '+name+' '+code);
  if(name.endsWith('negative'))assert.equal(accepted,false,operation+' '+name);
  const expected={
    'incremental-algorithm-negative':'INCREMENTAL_NOT_ENABLED',
    'incremental-format-negative':'INCREMENTAL_NOT_ENABLED',
    'consume-digest-negative':'API_CONSUME_MISMATCH',
    'consume-product-negative':'API_CONSUME_MISMATCH',
    'consume-component-negative':'API_CONSUME_MISMATCH',
    'consume-installationScopeId-negative':'API_CONSUME_MISMATCH',
    'purpose-cross-negative':'API_CONSUME_MISMATCH',
    'authorization-jti-negative':'API_PURPOSE_MISMATCH',
    'validate-fields-negative':'API_PURPOSE_MISMATCH',
    'validate-slot-negative':'API_PURPOSE_MISMATCH',
    'validate-staged-negative':'API_PURPOSE_MISMATCH',
    'transaction-binding-negative':'API_PURPOSE_MISMATCH',
    'preparation-binding-negative':'API_PURPOSE_MISMATCH',
    'event-lineage-negative':'API_EVENT_MISMATCH',
    'event-digest-negative':'API_EVENT_MISMATCH',
    'event-before-window-negative':'API_EVENT_MISMATCH',
    'event-at-end-negative':'API_EVENT_MISMATCH',
    'event-kind-negative':'API_EVENT_MISMATCH',
    'task-revision-negative':'API_EVIDENCE_MISMATCH',
    'evidence-digest-negative':'API_EVIDENCE_MISMATCH',
  }[name]??(name.startsWith('scope-')&&name.endsWith('negative')?'API_SCOPE_MISMATCH':null);
  if(expected!==null) {
    assertApiShape(operation+'-request',value);
    for(const field of ['decision','credential','authorization'])if(value[field]) {
      const claims=value[field].signed;checkSemantics(claims,{now:Math.max(claims.iat,claims.nbf)});
    }
    assert.equal(code,expected,operation+' '+name);
  }
  if(name==='event-purpose-schema-negative')assert.equal(code,'API_CONTRACT_INVALID');
  cases.push({operation,name,accepted,code,bytes:Buffer.from(JSON.stringify(value)).toString('base64')});
}
const operations=OPERATIONS.filter(op=>!op.management).map(op=>op.name);
const baselines=new Map(operations.map(operation=>[operation,baseline(operation)]));
for(const operation of operations) {
  const b=baselines.get(operation);emit(operation,'complete-positive',b);
  const mutate=(name,change)=>{const v=structuredClone(b);change(v);emit(operation,name,v);};
  mutate('extra-field-negative',v=>v.extra=null);
  for(const other of operations)emit(other,'cross-from-'+operation,b);
  if(b.decision||b.authorization||b.credential) {
    for(const[field,other]of Object.entries({product:'other-product',component:'other-component',installId:'other-install',
      installationScopeId:'other-scope',channel:'beta',channelRevision:2,targetKey:'linux-arm64-deb-perMachine'})) {
      mutate('scope-'+field+'-negative',v=>v.scope[field]=other);
    }
  }
  if(operation==='check') {
    mutate('incremental-algorithm-negative',v=>v.supportedIncrementalAlgorithms=['bsdiff']);
    mutate('incremental-format-negative',v=>v.supportedIncrementalFormats=['test-format']);
  }
  if(operation==='consume-status')mutate('noncanonical-recovery-negative',v=>v.recoverySecret='A'.repeat(42)+'B');
  if(operation.startsWith('consume-')&&operation!=='consume-status') {
    mutate('consume-digest-negative',v=>v.consumeRequestDigest='b'.repeat(64));
    mutate('purpose-cross-negative',v=>{
      v.semanticRequest.purpose=authPurposeOf(operation)==='install'?'activate':'install';
      v.consumeRequestDigest=consumeRequestDigest(v.semanticRequest);
    });
    for(const [field,other] of Object.entries({product:'other-product',
      component:'other-component',installationScopeId:'other-scope'})) {
      mutate('consume-'+field+'-negative',v=>{
        assert.notEqual(v.semanticRequest[field],other);
        v.semanticRequest[field]=other;
        v.consumeRequestDigest=consumeRequestDigest(v.semanticRequest);
      });
      emit(operation,'consume-'+field+'-restore-positive',b);
    }
    // Complete API does not add the later post-auth envelope hash binding.
    mutate('later-envelope-hash-independent-positive',v=>{
      v.semanticRequest.authorizationEnvelopeSha256='b'.repeat(64);
      v.consumeRequestDigest=consumeRequestDigest(v.semanticRequest);
    });
  }
  if(operation.startsWith('validate-')) {
    if(operation==='validate-preinstall') {
      mutate('validate-fields-negative',v=>v.freezeEpoch=1);
      mutate('validate-slot-negative',v=>v.preinstallSlot=null);
    }else {
      mutate('validate-fields-negative',v=>v.freezeEpoch=null);
      mutate('validate-slot-negative',v=>v.preinstallSlot=structuredClone(f.claims['authorization-preinstall'].preinstallSlot));
      if(operation==='validate-activate')mutate('validate-staged-negative',v=>v.staged=null);
      else mutate('validate-staged-negative',v=>v.staged=structuredClone(f.claims['authorization-activate'].staged));
    }
  }
  if(operation.startsWith('cancel-'))mutate('authorization-jti-negative',v=>v.authorizationJti='other-jti');
  if(operation.startsWith('execution-review-')) {
    mutate('transaction-binding-negative',v=>v.transactionId='other-transaction');
    mutate('preparation-binding-negative',v=>v.preparation.ownerEpoch+=1);
  }
  if(operation.startsWith('events-')) {
    if(operation!=='events-telemetry-event') {
      // Every non-telemetry request fixes this field with const: the Schema
      // rejects a different vocabulary purpose before the redundant API guard.
      mutate('event-purpose-schema-negative',v=>{
        v.events[0].executionPurpose=v.events[0].executionPurpose==='install'?'activate':'install';
      });
      emit(operation,'event-purpose-restore-positive',b);
    }
    mutate('event-lineage-negative',v=>v.events[0].lineageId='other-lineage');
    mutate('event-digest-negative',v=>v.events[0].factsSha256='b'.repeat(64));
    mutate('event-before-window-negative',v=>v.events[0].occurredAt-=1);
    mutate('event-at-end-negative',v=>v.events[0].occurredAt=v.credential.signed.eventNotAfter);
    mutate('event-kind-negative',v=>v.events[0].kind='unknown-kind');
  }
  if(operation==='reconciliation-action'||operation==='recovery-review-action') {
    mutate('task-revision-negative',v=>v.taskRevision+=1);
    mutate('evidence-digest-negative',v=>v.evidenceSha256='b'.repeat(64));
  }
  function walk(value,path=[]) {
    // Existing envelope Schema/semantics have their independent complete suites.
    // This batch varies the entire surrounding API shape and each scalar/required
    // member, while testing credential attachment/scope/cap/semantics explicitly.
    if(path.length===1&&['decision','authorization','credential'].includes(path[0]))return;
    if(value!==null&&typeof value==='object') {
      if(!Array.isArray(value))for(const key of Object.keys(value))mutate('missing-'+[...path,key].join('.'),copy=>{
        let at=copy;for(const part of path)at=at[part];delete at[key];
      });
      for(const[key,child]of Object.entries(value))walk(child,[...path,key]);
    }else for(const replacement of [null,true,0,9007199254740992,'',[],{}])mutate('scalar-'+path.join('.'),copy=>{
      let at=copy;for(const part of path.slice(0,-1))at=at[part];at[path.at(-1)]=replacement;
    });
  }
  walk(b);
  for(const field of ['decision','authorization','credential'])if(b[field]) {
    const fixed=canonicalize(b[field]).length;
    const perSignature=canonicalize(b[field].signatures[0]).length+1;
    const maximum=1+Math.floor((65536-fixed)/perSignature);
    for(const[count,label]of[[maximum,'positive'],[maximum+1,'negative']])
      mutate(field+'-signature-cap-'+label,v=>v[field].signatures=Array.from({length:count},()=>structuredClone(v[field].signatures[0])));
    mutate(field+'-future-nbf-positive',v=>v[field].signed.nbf=v[field].signed.iat+1);
    mutate(field+'-expired-negative',v=>v[field].signed.nbf=v[field].signed.exp);
    mutate(field+'-zero-signature-positive',v=>v[field].signatures[0].signature='A'.repeat(86));
    mutate(field+'-duplicate-signature-positive',v=>v[field].signatures.push(structuredClone(v[field].signatures[0])));
  }
}
function authPurposeOf(operation){return operation.split('-').at(-1);}
process.stdout.write(JSON.stringify({operations,cases}));
