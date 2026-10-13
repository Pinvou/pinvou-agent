// Original complete seven-operation transaction-control contracts; no trust proof.
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';
const root = pathToFileURL(process.argv[1]);
const { assertApiContract } = await import(new URL('./api-registry.mjs', root));
const cases = [];
const operations = ['consume-status','cancel-preinstall','cancel-install','cancel-activate',
  'execution-review-preinstall','execution-review-install','execution-review-activate'];
const reference = { recordKind:'transaction', recordId:'record-1', revision:1, sha256:'a'.repeat(64) };
function emit(operation, name, value) {
  let accepted = false;
  try { assertApiContract(operation,'response',value); accepted=true; } catch {}
  if(name.endsWith('positive')) assert.equal(accepted,true,operation+' '+name);
  if(name.endsWith('negative')) assert.equal(accepted,false,operation+' '+name);
  cases.push({ operation, name, accepted, bytes:Buffer.from(JSON.stringify(value)).toString('base64') });
}
for(const operation of operations) {
  const review = operation.startsWith('execution-review');
  let baseline = { protocolVersion:1, requestId:'request-1' };
  if(operation==='consume-status') Object.assign(baseline,{state:'consumed',transactionId:'transaction-1'});
  else if(review) Object.assign(baseline,{transactionId:'transaction-1',transactionRevision:0,reviewedAt:0,
    currentReadSet:[structuredClone(reference)],verifiedObjectSha256:'a'.repeat(64)});
  else Object.assign(baseline,{authorizationState:'consumed',authorizationRevision:0});
  emit(operation,'complete-positive',baseline);
  const mutate = (name, change) => {
    const value = structuredClone(baseline);
    change(value);
    emit(operation,name,value);
  };
  mutate('extra-field-negative',value=>value.extra=null);
  mutate('request-independent-positive',value=>value.requestId='other-request');
  if(operation==='consume-status') {
    for(const state of ['cancelled','expired']) {
      mutate(state+'-positive',value=>{value.state=state;value.transactionId=null;});
      mutate(state+'-non-null-negative',value=>value.state=state);
    }
    mutate('consumed-null-negative',value=>value.transactionId=null);
    mutate('pending-negative',value=>value.state='pending');
  } else if(!review) {
    for(const state of ['cancelled','expired']) mutate(state+'-positive',value=>value.authorizationState=state);
    mutate('revision-independent-positive',value=>value.authorizationRevision=9007199254740991);
  } else {
    mutate('time-revision-independent-positive',value=>{value.reviewedAt=9007199254740991;value.transactionRevision=9007199254740991;});
    mutate('duplicate-whole-ref-negative',value=>value.currentReadSet.push(structuredClone(reference)));
    mutate('duplicate-reordered-ref-negative',value=>value.currentReadSet.push({
      sha256:reference.sha256,revision:reference.revision,recordId:reference.recordId,recordKind:reference.recordKind
    }));
    mutate('same-record-different-revision-positive',value=>value.currentReadSet.push({...reference,revision:2}));
    mutate('same-record-different-hash-positive',value=>value.currentReadSet.push({...reference,sha256:'b'.repeat(64)}));
    mutate('empty-readset-negative',value=>value.currentReadSet=[]);
    mutate('4096-distinct-refs-positive',value=>value.currentReadSet=Array.from({length:4096},(_,index)=>({...reference,recordId:'record-'+index})));
    mutate('4097-refs-negative',value=>value.currentReadSet=Array.from({length:4097},(_,index)=>({...reference,recordId:'record-'+index})));
    mutate('duplicate-zero-number-negative',value=>value.currentReadSet=[{...reference,revision:0},{...reference,revision:-0}]);
  }
  function walk(value,path=[]) {
    if(value!==null && typeof value==='object') {
      if(!Array.isArray(value)) for(const key of Object.keys(value)) mutate('missing-'+[...path,key].join('.'),copy=>{
        let at=copy; for(const part of path) at=at[part]; delete at[key];
      });
      for(const [key,child] of Object.entries(value)) walk(child,[...path,key]);
    } else for(const replacement of [null,true,0,9007199254740992,'',[],{}]) mutate('scalar-'+path.join('.'),copy=>{
      let at=copy; for(const part of path.slice(0,-1)) at=at[part]; at[path.at(-1)]=replacement;
    });
  }
  walk(baseline);
}
process.stdout.write(JSON.stringify(cases));
