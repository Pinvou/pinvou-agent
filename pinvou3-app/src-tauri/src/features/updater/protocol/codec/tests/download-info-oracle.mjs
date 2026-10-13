// The actual original complete download-info API contract; no fake verifier.
import{pathToFileURL}from'node:url';
import assert from'node:assert/strict';
const root=pathToFileURL(process.argv[1]);
const{assertApiContract,assertApiShape}=await import(new URL('./api-registry.mjs',root));
const{checkSemantics}=await import(new URL('./semantics.mjs',root));
const{parseJson,canonicalize}=await import(new URL('./canonical-json.mjs',root));
const{createFixtureSet}=await import(new URL('./tests/fixtures.mjs',root));
const f=createFixtureSet();const credential=parseJson(f.sign(f.claims.download));
const b={protocolVersion:1,requestId:'request-1',credential,packageId:credential.signed.packageId,
 size:credential.signed.package.size,sha256:credential.signed.package.sha256,url:'https://example.invalid/file',
 expiresAt:credential.signed.exp,fileSourceClass:'primary'};
const cases=[];
function emit(name,value){let accepted=false;try{assertApiContract('download-info','response',value);accepted=true;}catch{}
 cases.push({name,accepted,bytes:Buffer.from(canonicalize(value)).toString('base64')});}
const mutate=(name,fn)=>{const v=structuredClone(b);fn(v);emit(name,v);};
const bind=(name,fn)=>{const v=structuredClone(b);fn(v);assert.notDeepEqual(v,b);
 assertApiShape('download-info-response',v);const c=v.credential.signed;checkSemantics(c,{now:Math.max(c.iat,c.nbf)});
 assert.throws(()=>assertApiContract('download-info','response',v),{code:'API_RESPONSE_MISMATCH'});emit(name,v);};
emit('complete-positive',b);
bind('packageId-mismatch',v=>v.packageId='other-package');
bind('size-mismatch',v=>v.size++);
bind('hash-mismatch',v=>v.sha256='0'.repeat(64));
bind('expiry-after-credential',v=>v.expiresAt=v.credential.signed.exp+1);
bind('expiry-at-nbf',v=>v.expiresAt=v.credential.signed.nbf);
bind('expiry-before-nbf',v=>v.expiresAt=v.credential.signed.nbf-1);
mutate('expiry-nbf-plus-one-positive',v=>v.expiresAt=v.credential.signed.nbf+1);
mutate('future-nbf-positive',v=>v.credential.signed.nbf++);
for(const [name,url]of Object.entries({'opaque-url-positive':'https://','scheme-only-with-newline-positive':'https://\n',
 'maximum-ascii-url-positive':'https://'+'a'.repeat(8184),'over-ascii-url-negative':'https://'+'a'.repeat(8185),
 'maximum-unicode-url-positive':'https://'+'\u{1F600}'.repeat(8184),'over-unicode-url-negative':'https://'+'\u{1F600}'.repeat(8185),
 'http-negative':'http://example.invalid/file'}))mutate(name,v=>v.url=url);
mutate('independent-source-request-positive',v=>{v.fileSourceClass='another-source';v.requestId='another-request';});
mutate('zero-signature-positive',v=>v.credential.signatures[0].signature='A'.repeat(86));
mutate('duplicate-signature-positive',v=>v.credential.signatures.push(structuredClone(v.credential.signatures[0])));
mutate('long-ttl-negative',v=>v.credential.signed.exp=v.credential.signed.iat+900001);
const boundary=structuredClone(b);const sig=structuredClone(boundary.credential.signatures[0]);
while(canonicalize(boundary.credential).length<=65536)boundary.credential.signatures.push(structuredClone(sig));
emit('over-credential64KiB-negative',boundary);boundary.credential.signatures.pop();emit('last-whole-signature-within64KiB-positive',boundary);
function walk(value,path=[]){
 if(value!==null&&typeof value==='object'){
  if(!Array.isArray(value))for(const key of Object.keys(value))mutate('missing-'+[...path,key].join('.'),v=>{let at=v;for(const p of path)at=at[p];delete at[key];});
  for(const[k,child]of Object.entries(value))walk(child,[...path,k]);
 }else for(const replacement of [null,true,0,9007199254740992,'',[],{}])mutate('scalar-'+path.join('.'),v=>{let at=v;for(const p of path.slice(0,-1))at=at[p];at[path.at(-1)]=replacement;});
}
walk(b);process.stdout.write(JSON.stringify(cases));
