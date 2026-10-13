// Reuse actual original-contract baseline fixtures, not production validators.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {pathToFileURL} from 'node:url';
const reference=pathToFileURL(process.argv[2]);
const {assertApiContract}=await import(new URL('./api-registry.mjs',reference));
const {canonicalize}=await import(new URL('./canonical-json.mjs',reference));
const {OPERATIONS}=await import(new URL('./build-api.mjs',reference));
const fixtureRoot=new URL('../../codec/tests/',import.meta.url);
async function baselineModule(file) {
  const source=await readFile(new URL(file,fixtureRoot),'utf8');
  const marker='const cases=[];';
  assert.equal(source.split(marker).length,2,'fixture boundary must remain unique');
  const head=source.split(marker)[0].replaceAll('process.argv[1]','process.argv[2]');
  const module=await import('data:text/javascript;base64,'+Buffer.from(head+
    '\nexport {baseline};').toString('base64'));
  return module.baseline;
}
const request=await baselineModule('client-request-oracle.mjs');
const response=await baselineModule('client-response-oracle.mjs');
const cases=OPERATIONS.filter(o=>!o.management).map(o=>{
  const req=request(o.name),res=response(o.name);
  assertApiContract(o.name,'request',req);assertApiContract(o.name,'response',res);
  return {operation:o.name,path:o.path,
    request:Buffer.from(canonicalize(req)).toString('utf8'),
    response:Buffer.from(canonicalize(res)).toString('utf8')};
});
assert.equal(cases.length,26);
process.stdout.write(JSON.stringify(cases));
