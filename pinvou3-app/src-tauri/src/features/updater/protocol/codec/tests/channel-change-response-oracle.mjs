import assert from 'node:assert/strict';
import {pathToFileURL} from 'node:url';
const root=pathToFileURL(process.argv[1]);
const {assertApiContract}=await import(new URL('./api-registry.mjs',root));
const baseline={protocolVersion:1,requestId:'request-1',channel:'stable',channelRevision:1};
const cases=[];
function emit(name,value) {
  let accepted=false;
  try{assertApiContract('channel-change','response',value);accepted=true;}catch{}
  if(name.endsWith('positive'))assert.equal(accepted,true,name);
  if(name.endsWith('negative'))assert.equal(accepted,false,name);
  cases.push({name,accepted,bytes:Buffer.from(JSON.stringify(value)).toString('base64')});
}
emit('complete-positive',baseline);
const mutate=(name,change)=>{const v=structuredClone(baseline);change(v);emit(name,v);};
for(const channel of ['beta','internal'])mutate(channel+'-positive',v=>v.channel=channel);
mutate('unknown-channel-negative',v=>v.channel='other');
mutate('zero-revision-negative',v=>v.channelRevision=0);
mutate('maximum-revision-positive',v=>v.channelRevision=9007199254740991);
mutate('request-independent-positive',v=>v.requestId='other-request');
mutate('extra-field-negative',v=>v.extra=null);
for(const key of Object.keys(baseline)) {
  mutate('missing-'+key+'-negative',v=>delete v[key]);
  for(const replacement of [null,true,0,9007199254740992,'',[],{}])
    mutate('scalar-'+key,v=>v[key]=replacement);
}
process.stdout.write(JSON.stringify(cases));
