import assert from 'node:assert/strict';
import {pathToFileURL} from 'node:url';
const root=pathToFileURL(process.argv[1]);
const {assertApiContract}=await import(new URL('./api-registry.mjs',root));
const operations=['events-telemetry-event','events-preinstall-transaction-event',
  'events-install-transaction-event','events-activate-transaction-event',
  'events-reconciliation-event','events-recovery-review-event',
  'reconciliation-action','recovery-review-action'];
const cases=[];
function emit(operation,name,value) {
  let accepted=false;
  try{assertApiContract(operation,'response',value);accepted=true;}catch{}
  if(name.endsWith('positive'))assert.equal(accepted,true,operation+' '+name);
  if(name.endsWith('negative'))assert.equal(accepted,false,operation+' '+name);
  cases.push({operation,name,accepted,bytes:Buffer.from(JSON.stringify(value)).toString('base64')});
}
for(const operation of operations) {
  const events=operation.startsWith('events-');
  const baseline={protocolVersion:1,requestId:'request-1'};
  if(events)baseline.acknowledgements=[{eventId:'event-1',disposition:'committed'}];
  else Object.assign(baseline,{taskId:'task-1',taskRevision:0,disposition:'recorded'});
  emit(operation,'complete-positive',baseline);
  const mutate=(name,change)=>{const v=structuredClone(baseline);change(v);emit(operation,name,v);};
  mutate('extra-field-negative',v=>v.extra=null);
  mutate('request-independent-positive',v=>v.requestId='other-request');
  if(events) {
    for(const disposition of ['replayed','late-diagnostic'])
      mutate(disposition+'-positive',v=>v.acknowledgements[0].disposition=disposition);
    mutate('duplicate-receipt-positive',v=>v.acknowledgements.push(structuredClone(v.acknowledgements[0])));
    mutate('same-event-different-disposition-positive',v=>v.acknowledgements.push({eventId:'event-1',disposition:'late-diagnostic'}));
    mutate('4096-receipts-positive',v=>v.acknowledgements=Array.from({length:4096},()=>structuredClone(v.acknowledgements[0])));
    mutate('4097-receipts-negative',v=>v.acknowledgements=Array.from({length:4097},()=>structuredClone(v.acknowledgements[0])));
    mutate('empty-receipt-negative',v=>v.acknowledgements=[]);
    mutate('unknown-disposition-negative',v=>v.acknowledgements[0].disposition='pending');
  } else {
    for(const disposition of ['protection-retained','reconciled'])
      mutate(disposition+'-positive',v=>v.disposition=disposition);
    mutate('revision-independent-positive',v=>v.taskRevision=9007199254740991);
    mutate('unknown-disposition-negative',v=>v.disposition='approved');
  }
  function walk(value,path=[]) {
    if(value!==null&&typeof value==='object') {
      if(!Array.isArray(value))for(const key of Object.keys(value))mutate('missing-'+[...path,key].join('.'),copy=>{
        let at=copy;for(const p of path)at=at[p];delete at[key];
      });
      for(const[key,child]of Object.entries(value))walk(child,[...path,key]);
    } else for(const replacement of [null,true,0,9007199254740992,'',[],{}])mutate('scalar-'+path.join('.'),copy=>{
      let at=copy;for(const p of path.slice(0,-1))at=at[p];at[path.at(-1)]=replacement;
    });
  }
  walk(baseline);
}
process.stdout.write(JSON.stringify(cases));
