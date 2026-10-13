// Validates actual frozen OpenAPI operation/status response schemas with Ajv.
import assert from 'node:assert/strict';
import {pathToFileURL} from 'node:url';
import {createRequire} from 'node:module';
import {readFile} from 'node:fs/promises';
const root=pathToFileURL(process.argv[1]);
const require=createRequire(root);
const {default:Ajv}=await import(pathToFileURL(require.resolve('ajv/dist/2020.js')));
const {OPERATIONS}=await import(new URL('./build-api.mjs',root));
const {ERROR_DEFINITIONS}=await import(new URL('./api-shapes.mjs',root));
const openapi=JSON.parse(await readFile(new URL('../openapi.yaml',root)));
const id='urn:pinvou:upgrade:test:openapi-components';
function references(value) {
  if(Array.isArray(value))return value.map(references);
  if(value===null||typeof value!=='object')return value;
  const result={};
  for(const[key,child]of Object.entries(value)) {
    result[key]=key==='$ref'&&typeof child==='string'&&child.startsWith('#/components/schemas/')
      ?id+'#/$defs/'+child.slice('#/components/schemas/'.length):references(child);
  }
  return result;
}
const ajv=new Ajv({strict:true,allErrors:false,validateFormats:false});
ajv.addSchema({$id:id,$defs:references(openapi.components.schemas)});
const validators=new Map();
function validate(operation,status,value) {
  const key=operation.name+':'+status;
  if(!validators.has(key)) {
    const schema=openapi.paths[operation.path].post.responses[String(status)]?.content?.['application/json']?.schema;
    validators.set(key,schema===undefined?null:ajv.compile(references(schema)));
  }
  return validators.get(key)?.(value)===true;
}
function body(code) {
  const result={protocolVersion:1,requestId:'request-1',code,retryable:ERROR_DEFINITIONS[code].retryable};
  if(code==='SCOPE_UNACTIVATED')result.checkAfterMs=1;
  return result;
}
const cases=[];
function emit(operation,status,name,value) {
  const accepted=validate(operation,status,value);
  if(name.endsWith('positive'))assert.equal(accepted,true,operation.name+' '+name);
  if(name.endsWith('negative'))assert.equal(accepted,false,operation.name+' '+name);
  cases.push({operation:operation.name,status,name,accepted,bytes:Buffer.from(JSON.stringify(value)).toString('base64')});
}
const clients=OPERATIONS.filter(op=>!op.management);
for(const operation of clients) {
  for(const [code,definition] of Object.entries(ERROR_DEFINITIONS)) {
    const baseline=body(code);
    emit(operation,definition.status,'all-declared-codes',baseline);
    for(const status of [200,400,401,403,404,409,422,429,503,599])
      emit(operation,status,'status-cross-'+code,baseline);
    if(!operation.errors.includes(code))continue;
    emit(operation,definition.status,code+'-positive',baseline);
    const mutate=(name,change)=>{const value=structuredClone(baseline);change(value);emit(operation,definition.status,name,value);};
    mutate('extra-field-negative',v=>v.extra=null);
    mutate('retryable-flipped-negative',v=>v.retryable=!v.retryable);
    mutate('request-independent-positive',v=>v.requestId='other-request');
    for(const key of Object.keys(baseline)) {
      mutate('missing-'+key+'-negative',v=>delete v[key]);
      for(const replacement of [null,true,0,9007199254740992,'',[],{}])
        mutate('scalar-'+key,v=>v[key]=replacement);
    }
    if(code==='SCOPE_UNACTIVATED') {
      mutate('check-after-maximum-positive',v=>v.checkAfterMs=86400000);
      mutate('check-after-over-negative',v=>v.checkAfterMs=86400001);
      mutate('check-after-zero-negative',v=>v.checkAfterMs=0);
      mutate('unactivated-retry-after-negative',v=>v.retryAfterMs=1);
    }else if(definition.retryable) {
      mutate('optional-retry-delay-positive',v=>v.retryAfterMs=1);
      mutate('optional-retry-delay-maximum-positive',v=>v.retryAfterMs=300000);
      mutate('optional-retry-delay-over-negative',v=>v.retryAfterMs=300001);
      mutate('optional-retry-delay-zero-negative',v=>v.retryAfterMs=0);
    }else mutate('nonretryable-delay-negative',v=>v.retryAfterMs=1);
  }
}
process.stdout.write(JSON.stringify(cases));
