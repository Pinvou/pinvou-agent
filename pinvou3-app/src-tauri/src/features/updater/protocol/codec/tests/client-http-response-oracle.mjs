// Reuses actual original complete operation/OpenAPI oracle inputs independently.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {spawnSync} from 'node:child_process';
import {pathToFileURL} from 'node:url';
const root=pathToFileURL(process.argv[1]);
async function original(file) {
  // Resolve repository-relative fixtures from the known original reference URL.
  const url=new URL('../../../../pinvou3-app/src-tauri/src/features/updater/protocol/codec/tests/'+file,root);
  const source=await readFile(url,'utf8');
  const child=spawnSync(process.execPath,['--input-type=module','-e',source,process.argv[1]],
    {encoding:'utf8',maxBuffer:64*1024*1024});
  assert.equal(child.status,0,'original HTTP fixture failed: '+child.stderr);
  return JSON.parse(child.stdout);
}
const operations=await original('client-response-oracle.mjs');
const errors=await original('client-error-response-oracle.mjs');
const cases=operations.cases.map(c=>({...c,status:200}));
cases.push(...errors);
for(const c of operations.cases.filter(c=>c.name==='complete-positive')) {
  for(const status of [0,201,204,302,399,600,65535])
    cases.push({...c,status,name:'undeclared-status-negative',accepted:false,code:'API_CONTRACT_INVALID'});
  for(const status of [400,409,503])
    cases.push({...c,status,name:'operation-body-in-error-status-negative',accepted:false,code:'API_CONTRACT_INVALID'});
}
process.stdout.write(JSON.stringify(cases));
