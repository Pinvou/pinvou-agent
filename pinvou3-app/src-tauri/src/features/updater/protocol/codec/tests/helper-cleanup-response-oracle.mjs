import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {pathToFileURL} from 'node:url';
const root=pathToFileURL(process.argv[1]);
const {assertApiContract}=await import(new URL('./api-registry.mjs',root));
const {canonicalize}=await import(new URL('./canonical-json.mjs',root));
const vectors=JSON.parse(await readFile(new URL('../vectors/crypto/envelopes.json',root)));
const credential=JSON.parse(Buffer.from(vectors.vectors.find(v=>v.name==='helper-cleanup').envelopeBase64Url,'base64url'));
const baseline={protocolVersion:1,requestId:'request-1',credential};
const cases=[];
function emit(name,value) {
  let accepted=false;
  try{assertApiContract('helper-cleanup','response',value);accepted=true;}catch{}
  if(name.endsWith('positive'))assert.equal(accepted,true,name);
  if(name.endsWith('negative'))assert.equal(accepted,false,name);
  cases.push({name,accepted,bytes:Buffer.from(JSON.stringify(value)).toString('base64')});
}
emit('complete-positive',baseline);
const mutate=(name,change)=>{const v=structuredClone(baseline);change(v);emit(name,v);};
mutate('extra-field-negative',v=>v.extra=null);
mutate('request-independent-positive',v=>v.requestId='other-request');
mutate('zero-signature-positive',v=>v.credential.signatures[0].signature='A'.repeat(86));
mutate('duplicate-signature-positive',v=>v.credential.signatures.push(structuredClone(v.credential.signatures[0])));
mutate('future-nbf-positive',v=>v.credential.signed.nbf=v.credential.signed.iat+1);
mutate('expired-at-nbf-negative',v=>v.credential.signed.nbf=v.credential.signed.exp);
const fixed=canonicalize(credential).length;
const perSignature=canonicalize(credential.signatures[0]).length+1;
const maximum=1+Math.floor((65536-fixed)/perSignature);
for(const[count,label]of[[maximum,'positive'],[maximum+1,'negative']])mutate('signature-budget-'+label,v=>{
  v.credential.signatures=Array.from({length:count},()=>structuredClone(v.credential.signatures[0]));
});
function walk(value,path=[]) {
  if(value!==null&&typeof value==='object') {
    if(!Array.isArray(value))for(const key of Object.keys(value))mutate('missing-'+[...path,key].join('.'),copy=>{
      let at=copy;for(const p of path)at=at[p];delete at[key];
    });
    for(const[key,child]of Object.entries(value))walk(child,[...path,key]);
  }else for(const replacement of [null,true,0,9007199254740992,'',[],{}])mutate('scalar-'+path.join('.'),copy=>{
    let at=copy;for(const p of path.slice(0,-1))at=at[p];at[path.at(-1)]=replacement;
  });
}
walk(baseline);
process.stdout.write(JSON.stringify(cases));
