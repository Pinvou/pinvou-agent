// Actual complete original task-create response contract. No authentication claim.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
const root=pathToFileURL(process.argv[1]);
const { assertApiContract,assertApiShape }=await import(new URL('./api-registry.mjs',root));
const { checkSemantics }=await import(new URL('./semantics.mjs',root));
const { canonicalize }=await import(new URL('./canonical-json.mjs',root));
const vectors=JSON.parse(await readFile(new URL('../vectors/crypto/envelopes.json',root)));
const original=name=>JSON.parse(Buffer.from(
    vectors.vectors.find(v=>v.name===name).envelopeBase64Url,'base64url'));
const cases=[];
function emit(operation,name,value) {
  let accepted=false,code=null;
  try{assertApiContract(operation,'response',value);accepted=true;}catch(error){code=error.code;}
  if(name.endsWith('positive'))assert.equal(accepted,true,operation+' '+name+' '+code);
  if(name.endsWith('negative'))assert.equal(accepted,false,operation+' '+name);
  if(name.startsWith('binding-') && name.endsWith('negative')) {
    assertApiShape(operation+'-response',value);
    for(const field of ['actionCredential','eventCredential']) {
      const claims=value[field].signed;
      checkSemantics(claims,{now:Math.max(claims.iat,claims.nbf)});
    }
    assert.equal(code,'API_RESPONSE_MISMATCH',operation+' '+name);
  }
  cases.push({operation,name,accepted,code,bytes:Buffer.from(JSON.stringify(value)).toString('base64')});
}
for(const [operation,prefix] of [
  ['reconciliation-create','reconciliation'],['recovery-review-create','recovery-review']
]) {
  const action=original(prefix+'-action'),event=original(prefix+'-event');
  const baseline={protocolVersion:1,requestId:'request-1',task:structuredClone(action.signed.task),
    actionCredential:action,eventCredential:event};
  emit(operation,'complete-positive',baseline);
  const mutate=(name,change)=>{
    const value=structuredClone(baseline);change(value);
    if(name.endsWith('negative')||name.endsWith('positive')) assert.equal(JSON.stringify(value)!==JSON.stringify(baseline),true,operation+' no-op mutation '+name);
    emit(operation,name,value);
  };
  mutate('request-independent-positive',v=>v.requestId='other-request');
  mutate('extra-field-negative',v=>v.extra=null);
  for(const field of ['actionCredential','eventCredential']) {
    mutate('zero-signature-'+field+'-positive',v=>v[field].signatures[0].signature='A'.repeat(86));
    mutate('duplicate-signature-'+field+'-positive',v=>v[field].signatures.push(structuredClone(v[field].signatures[0])));
    mutate('binding-task-'+field+'-negative',v=>v[field].signed.task.revision+=1);
    mutate('binding-task-'+field+'-restore-positive',v=>{
      v.task.revision+=1;v.actionCredential.signed.task.revision=v.task.revision;
      v.eventCredential.signed.task.revision=v.task.revision;
    });
  }
  for(const field of ['product','component','installId']) {
    mutate('binding-'+field+'-negative',v=>v.eventCredential.signed[field]='other-'+field);
    mutate(field+'-restore-positive',v=>{
      v.actionCredential.signed[field]='other-'+field;v.eventCredential.signed[field]='other-'+field;
    });
  }
  mutate('binding-scope-channel-negative',v=>v.eventCredential.signed.scope.channel='beta');
  mutate('scope-channel-restore-positive',v=>{
    v.actionCredential.signed.scope.channel='beta';v.eventCredential.signed.scope.channel='beta';
  });
  for(const [field,replacement] of [['installationScopeId','other-scope'],['channelRevision',2]]) {
    mutate('binding-scope-'+field+'-negative',v=>v.eventCredential.signed.scope[field]=replacement);
    mutate('scope-'+field+'-restore-positive',v=>{
      v.actionCredential.signed.scope[field]=replacement;v.eventCredential.signed.scope[field]=replacement;
    });
  }
  for(const field of ['actionCredential','eventCredential']) {
    const fixed=canonicalize(baseline[field]).length;
    const perSignature=canonicalize(baseline[field].signatures[0]).length+1;
    const maximum=1+Math.floor((65536-fixed)/perSignature);
    for(const [count,label] of [[maximum,'positive'],[maximum+1,'negative']]) {
      mutate('signature-budget-'+field+'-'+label,v=>{
        v[field].signatures=Array.from({length:count},()=>structuredClone(v[field].signatures[0]));
      });
    }
  }
  function walk(value,path=[]) {
    if(value!==null&&typeof value==='object') {
      if(!Array.isArray(value))for(const key of Object.keys(value))mutate('missing-'+[...path,key].join('.'),copy=>{
        let at=copy;for(const p of path)at=at[p];delete at[key];
      });
      for(const [key,child]of Object.entries(value))walk(child,[...path,key]);
    }else for(const replacement of [null,true,0,9007199254740992,'',[],{}])mutate('scalar-'+path.join('.'),copy=>{
      let at=copy;for(const p of path.slice(0,-1))at=at[p];at[path.at(-1)]=replacement;
    });
  }
  walk(baseline);
}
process.stdout.write(JSON.stringify(cases));
