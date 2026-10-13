// Complete original non-management operation table and response acceptance.
import assert from 'node:assert/strict';
import{pathToFileURL}from'node:url';
const root=pathToFileURL(process.argv[1]);
const{OPERATIONS}=await import(new URL('./build-api.mjs',root));
const{assertApiContract,assertApiShape}=await import(new URL('./api-registry.mjs',root));
const{parseJson}=await import(new URL('./canonical-json.mjs',root));
const{checkSemantics}=await import(new URL('./semantics.mjs',root));
const{createFixtureSet}=await import(new URL('./tests/fixtures.mjs',root));
const f=createFixtureSet();
const envelope=role=>parseJson(f.sign(f.claims[role]));
const operations=OPERATIONS.filter(op=>!op.management).map(op=>op.name);
const management=OPERATIONS.filter(op=>op.management).map(op=>op.name);
assert.equal(operations.length,26);
function baseline(operation) {
  const v={protocolVersion:1,requestId:'request-1'};
  if(operation==='check'||operation==='refresh') {
    v.decision=envelope('decision');v.telemetryCredential=envelope('telemetry-event');
    if(operation==='refresh'){v.workflowRevision=0;v.writableSessionId=v.decision.signed.telemetrySessionId;}
  } else if(operation==='download-info') {
    v.credential=envelope('download');const c=v.credential.signed;
    Object.assign(v,{packageId:c.packageId,size:c.package.size,sha256:c.package.sha256,
      url:'https://example.invalid/file',expiresAt:c.exp,fileSourceClass:'primary'});
  } else if(operation.startsWith('validate-')) {
    v.authorization=envelope('authorization-'+operation.slice('validate-'.length));v.authorizationRevision=0;
  } else if(operation==='consume-status') {
    Object.assign(v,{state:'consumed',transactionId:'transaction-1'});
  } else if(operation.startsWith('consume-')) {
    v.transactionCredential=envelope(operation.slice('consume-'.length)+'-transaction-event');
    Object.assign(v,{authorizationRevision:0,authorizationState:'consumed',
      transactionId:v.transactionCredential.signed.transactionId});
  } else if(operation.startsWith('cancel-')) {
    Object.assign(v,{authorizationRevision:0,authorizationState:'cancelled'});
  } else if(operation.startsWith('execution-review-')) {
    Object.assign(v,{transactionId:'transaction-1',transactionRevision:0,reviewedAt:0,
      verifiedObjectSha256:'a'.repeat(64),currentReadSet:[
        {recordKind:'transaction',recordId:'record-1',revision:1,sha256:'a'.repeat(64)},
        {recordKind:'transaction',recordId:'record-2',revision:1,sha256:'a'.repeat(64)}]});
  } else if(operation==='helper-cleanup') {
    v.credential=envelope('helper-cleanup');
  } else if(operation==='channel-change') {
    Object.assign(v,{channel:'stable',channelRevision:1});
  } else if(operation.startsWith('events-')) {
    v.acknowledgements=[{eventId:'event-1',disposition:'committed'}];
  } else if(operation==='reconciliation-action'||operation==='recovery-review-action') {
    Object.assign(v,{taskId:'task-1',taskRevision:0,disposition:'recorded'});
  } else throw new Error('missing original client response fixture');
  return v;
}
const cases=[];
function emit(operation,name,value) {
  let accepted=false,code=null;
  try{assertApiContract(operation,'response',value);accepted=true;}catch(error){code=error.code;}
  if(name.endsWith('positive'))assert.equal(accepted,true,operation+' '+name);
  if(name.endsWith('negative'))assert.equal(accepted,false,operation+' '+name);
  if(name.includes('binding-negative')) {
    assertApiShape(operation+'-response',value);
    for(const field of ['decision','telemetryCredential','credential','authorization','transactionCredential']) {
      if(value[field]) {
        const claims=value[field].signed;
        checkSemantics(claims,{now:Math.max(claims.iat,claims.nbf)});
      }
    }
    assert.equal(code,'API_RESPONSE_MISMATCH',operation+' '+name);
  }
  cases.push({operation,name,accepted,code,bytes:Buffer.from(JSON.stringify(value)).toString('base64')});
}
for(const operation of operations) {
  const value=baseline(operation);emit(operation,'complete-positive',value);
  for(const other of operations)emit(other,'cross-family-from-'+operation,value);
  emit(operation,'extra-field-negative',{...value,extra:null});
  emit(operation,'request-independent-positive',{...value,requestId:'another-request'});
  if(operation==='check'||operation==='refresh') {
    const mismatch=structuredClone(value);mismatch.telemetryCredential.signed.installId='other-install';
    emit(operation,'identity-binding-negative',mismatch);
    if(operation==='refresh') {
      const writable=structuredClone(value);writable.writableSessionId='other-session';
      emit(operation,'writable-binding-negative',writable);
    }
  } else if(operation==='download-info') {
    emit(operation,'package-binding-negative',{...value,packageId:'other-package'});
  } else if(operation.startsWith('consume-')&&operation!=='consume-status') {
    emit(operation,'transaction-binding-negative',{...value,transactionId:'other-transaction'});
  }
  for(const field of ['decision','telemetryCredential','credential','authorization','transactionCredential']) {
    if(value[field]) {
      const expired=structuredClone(value);expired[field].signed.nbf=expired[field].signed.exp;
      emit(operation,'credential-semantics-'+field+'-negative',expired);
    }
  }
}
process.stdout.write(JSON.stringify({operations,management,cases}));
