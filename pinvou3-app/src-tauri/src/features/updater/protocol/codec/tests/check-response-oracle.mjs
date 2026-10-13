// Calls the complete original API contract; no fake verification or private keys.
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';
const root = pathToFileURL(process.argv[1]);
const { assertApiContract, assertApiShape } = await import(new URL('./api-registry.mjs', root));
const { parseJson, canonicalize } = await import(new URL('./canonical-json.mjs', root));
const { checkSemantics } = await import(new URL('./semantics.mjs', root));
const { createFixtureSet } = await import(new URL('./tests/fixtures.mjs', root));
const fixture = createFixtureSet();
const base = {protocolVersion:1,requestId:'request-1',decision:parseJson(fixture.sign(fixture.claims.decision)),
    telemetryCredential:parseJson(fixture.sign(fixture.claims['telemetry-event']))};
const cases=[];
function emit(operation,name,value){
    let accepted=false;
    try{assertApiContract(operation,'response',value);accepted=true;}catch{}
    cases.push({operation,name,accepted,bytes:Buffer.from(canonicalize(value)).toString('base64')});
}
function run(operation) {
    const b=structuredClone(base);
    if(operation==='refresh')Object.assign(b,{workflowRevision:0,writableSessionId:b.decision.signed.telemetrySessionId});
    emit(operation,'complete-positive',b);
    const mutate=(name,f)=>{const v=structuredClone(b);f(v);emit(operation,name,v);};
    const bindingNegative=(name,f)=>{
        const v=structuredClone(b);f(v);
        assert.notDeepEqual(v,b,'binding mutation must change a fact');
        assertApiShape(operation+'-response',v);
        for(const field of ['decision','telemetryCredential']){
            const c=v[field].signed;checkSemantics(c,{now:Math.max(c.iat,c.nbf)});
        }
        assert.throws(()=>assertApiContract(operation,'response',v),{code:'API_RESPONSE_MISMATCH'});
        emit(operation,name,v);
    };
    // Independently valid credentials cross all identity bindings; changing one
    // field on one side must not be masked by credential-internal rejection.
    for(const field of ['product','component','installId','decisionId','decisionRevision','telemetrySessionId','selectionGeneration']){
        bindingNegative('telemetry-'+field,v=>v.telemetryCredential.signed[field]=typeof v.telemetryCredential.signed[field]==='number'?v.telemetryCredential.signed[field]+1:'other-identity');
    }
    for(const [field,other]of Object.entries({installationScopeId:'other-scope',channel:'beta',channelRevision:2,targetKey:'linux-arm64-deb-perMachine'}))
        bindingNegative('telemetry-scope-'+field,v=>v.telemetryCredential.signed.scope[field]=other);
    for(const env of ['decision','telemetryCredential']){
        mutate(env+'-future-nbf-positive',v=>v[env].signed.nbf=v[env].signed.iat+1);
        mutate(env+'-nbf-at-exp-negative',v=>v[env].signed.nbf=v[env].signed.exp);
        mutate(env+'-long-ttl-negative',v=>v[env].signed.exp=v[env].signed.iat+(env==='decision'?900001:7*86400000+1));
        mutate(env+'-duplicate-signatures-positive',v=>v[env].signatures.push(structuredClone(v[env].signatures[0])));
        mutate(env+'-zero-signature-positive',v=>v[env].signatures[0].signature='A'.repeat(86));
        mutate(env+'-over64KiB-negative',v=>{const sig=structuredClone(v[env].signatures[0]);while(canonicalize(v[env]).length<=65536)v[env].signatures.push(structuredClone(sig));});
    }
    mutate('requestId-is-independent-positive',v=>v.requestId='independent-request');
    if(operation==='refresh'){
        bindingNegative('writableSession-negative',v=>v.writableSessionId='other-session');
        mutate('workflowRevision-independent-positive',v=>v.workflowRevision=9007199254740991);
    }
    // Every nested required field and scalar is tested with deletion and shape
    // replacements. Original production credentials are never logged here.
    function walk(value,path=[]){
        if(value!==null&&typeof value==='object'){
            if(!Array.isArray(value))for(const key of Object.keys(value)){
                mutate('missing-'+[...path,key].join('.'),v=>{let at=v;for(const p of path)at=at[p];delete at[key];});
            }
            for(const [k,child]of Object.entries(value))walk(child,[...path,k]);
        } else {
            for(const replacement of [null,true,0,9007199254740992,'',[],{}]){
                mutate('scalar-'+path.join('.'),v=>{let at=v;for(const p of path.slice(0,-1))at=at[p];at[path.at(-1)]=replacement;});
            }
        }
    }
    walk(b);
}
run('check');run('refresh');
process.stdout.write(JSON.stringify(cases));
