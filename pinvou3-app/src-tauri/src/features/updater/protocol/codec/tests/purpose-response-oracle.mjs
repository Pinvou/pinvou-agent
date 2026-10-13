// Calls complete original six-operation API response contracts, no fake verifier.
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';
const root = pathToFileURL(process.argv[1]);
const {assertApiContract,assertApiShape}=await import(new URL('./api-registry.mjs',root));
const {checkSemantics}=await import(new URL('./semantics.mjs',root));
const {canonicalize,parseJson}=await import(new URL('./canonical-json.mjs',root));
const {createFixtureSet}=await import(new URL('./tests/fixtures.mjs',root));
const fixture = createFixtureSet();
const cases = [];
const roles=['authorization-preinstall','authorization-install','authorization-activate',
 'preinstall-transaction-event','install-transaction-event','activate-transaction-event'];
const envelopes=Object.fromEntries(roles.map(role=>[role,parseJson(fixture.sign(fixture.claims[role]))]));
function emit(operation, name, value) {
  let accepted = false;
  try {
    assertApiContract(operation, 'response', value);
    accepted = true;
  } catch {}
 if(name.endsWith('positive'))assert.equal(accepted,true,operation+' '+name);
 if(name.endsWith('negative')||name.startsWith('wrong-role-'))assert.equal(accepted,false,operation+' '+name);
 cases.push({ operation, name, accepted,
    bytes: Buffer.from(canonicalize(value)).toString('base64') });
}
for(const kind of ['validate','consume'])for(const purpose of ['preinstall','install','activate']){
 const operation=kind+'-'+purpose;const consuming=kind==='consume';
 const field=consuming?'transactionCredential':'authorization';
 const role=consuming?purpose+'-transaction-event':'authorization-'+purpose;
 const b={protocolVersion:1,requestId:'request-1',authorizationRevision:0,[field]:structuredClone(envelopes[role])};
 if(consuming)Object.assign(b,{authorizationState:'consumed',transactionId:b[field].signed.transactionId});
 const mutate = (name, change) => {
    const value = structuredClone(b);
    change(value);
    emit(operation, name, value);
  };
 emit(operation,'complete-positive',b);
 if(consuming){
  const v=structuredClone(b);v.transactionId='other-transaction';assert.notEqual(v.transactionId,b.transactionId);
  assertApiShape(operation+'-response',v);const c=v[field].signed;checkSemantics(c,{now:Math.max(c.iat,c.nbf)});
  assert.throws(()=>assertApiContract(operation,'response',v),{code:'API_RESPONSE_MISMATCH'});
  emit(operation,'transaction-mismatch',v);v.transactionId=b.transactionId;emit(operation,'transaction-restored-positive',v);
 }
 for(const other of roles)if(other!==role)mutate('wrong-role-'+other,v=>v[field]=structuredClone(envelopes[other]));
 mutate('revision-request-independent-positive',v=>{v.authorizationRevision=9007199254740991;v.requestId='independent-request';});
 mutate('future-nbf-positive',v=>v[field].signed.nbf++);
 mutate('nbf-at-exp-negative',v=>v[field].signed.nbf=v[field].signed.exp);
 mutate('long-ttl-negative',v=>v[field].signed.exp=v[field].signed.iat+(consuming?37*86400000+1:300001));
 mutate('zero-signature-positive',v=>v[field].signatures[0].signature='A'.repeat(86));
 mutate('duplicate-signature-positive',v=>v[field].signatures.push(structuredClone(v[field].signatures[0])));
 const boundary=structuredClone(b);const sig=structuredClone(boundary[field].signatures[0]);
 while(canonicalize(boundary[field]).length<=65536)boundary[field].signatures.push(structuredClone(sig));
 emit(operation,'over64KiB-negative',boundary);boundary[field].signatures.pop();emit(operation,'last-whole-signature-within64KiB-positive',boundary);
 function walk(value, path = []) {
  if(value!==null&&typeof value==='object'){
   if(!Array.isArray(value))for(const key of Object.keys(value))mutate('missing-'+[...path,key].join('.'),v=>{let at=v;for(const p of path)at=at[p];delete at[key];});
   for(const[k,child]of Object.entries(value))walk(child,[...path,k]);
  }else for(const replacement of [null,true,0,9007199254740992,'',[],{}])mutate('scalar-'+path.join('.'),v=>{let at=v;for(const p of path.slice(0,-1))at=at[p];at[path.at(-1)]=replacement;});
 }
 walk(b);
}
process.stdout.write(JSON.stringify(cases));
