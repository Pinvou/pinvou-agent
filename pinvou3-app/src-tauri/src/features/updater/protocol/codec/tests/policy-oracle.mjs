import { pathToFileURL } from 'node:url';
import { readFile } from 'node:fs/promises';
import { compileFunction } from 'node:vm';
const reference=pathToFileURL(process.argv[1]);
const source=await readFile(reference,'utf8');
const extracted=source.match(/function policyFor\(root, signed\) \{\n[\s\S]*?\n\}/u);
if(!extracted)throw new Error('original policyFor unavailable');
const { requireCondition }=await import(new URL('./errors.mjs',reference));
// Execute the original unchanged function body; no matching algorithm is copied.
const originalPolicyFor=compileFunction('return ('+extracted[0]+');',['requireCondition'])(requireCondition);
const { validateTrustedRoot }=await import(reference);
const { parseJson, canonicalize }=await import(new URL('./canonical-json.mjs',reference));
function contextShape(context) {
  if(context===null||typeof context!=='object'||Array.isArray(context)
    ||typeof context.role!=='string'||typeof context.product!=='string'
    ||!Object.hasOwn(context,'component')
    ||!(context.component===null||typeof context.component==='string')
    ||context.scope===null||typeof context.scope!=='object'||Array.isArray(context.scope))throw new Error('context shape');
  for(const field of ['channel','targetKey']) {
    const value=context.scope[field];
    if(value!==undefined&&value!==null&&typeof value!=='string')throw new Error('context shape');
  }
}
const chunks=[];for await(const chunk of process.stdin)chunks.push(chunk);
const results=JSON.parse(Buffer.concat(chunks).toString('utf8')).map(([rootBytes,contextBytes])=>{
  try {
    const root=parseJson(Buffer.from(rootBytes,'base64'));
    const context=parseJson(Buffer.from(contextBytes,'base64'));
    validateTrustedRoot(root);contextShape(context);
    return Buffer.from(canonicalize(originalPolicyFor(root,context))).toString('base64');
  } catch{return null;}
});
process.stdout.write(JSON.stringify(results));
