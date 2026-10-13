import { pathToFileURL } from 'node:url';
import { readFile } from 'node:fs/promises';
import { compileFunction } from 'node:vm';
const reference=pathToFileURL(process.argv[1]);
const source=await readFile(reference,'utf8');
function extract(pattern) {
  const match=source.match(pattern);
  if(!match)throw new Error('original consume binding fragment unavailable');
  return match[0];
}
const helpers=extract(/const isMetadata = [\s\S]*?const schemaName = [^\n]+;/u);
const parser=extract(/function parseEnvelope\(bytes, expected\) \{\n[\s\S]*?\n\}/u);
const decoder=extract(/function decodeBase64Url\(text, size, code\) \{\n[\s\S]*?\n\}/u);
const original=extract(/export function assertConsumeBinding\(request, authorizationBytes, options\) \{\n[\s\S]*?\n\}/u);
const prefix="export function assertConsumeBinding(request, authorizationBytes, options) {\n  validateSchema('bindings/consume-request', request);\n  const { signed } = verifyEnvelope(authorizationBytes, options);\n";
if(!original.startsWith(prefix))throw new Error('original verification boundary changed');
// Execute the exact original statements after verifyEnvelope, without stubbing
// cryptography or claiming whole-export equivalence. Full encoding/Schema precede
// them separately; authenticity/context/time/semantics remain outside this subset.
const body=original.slice(prefix.length,-2);
const { canonicalize,LIMITS,parseJson }=await import(new URL('./canonical-json.mjs',reference));
const { requireCondition }=await import(new URL('./errors.mjs',reference));
const { validateSchema }=await import(new URL('./schema-registry.mjs',reference));
const { METADATA_LIFETIMES,sameJson }=await import(new URL('./semantics.mjs',reference));
const { equalDigest,sha256 }=await import(new URL('./digests.mjs',reference));
const [parseEnvelope,decodeBase64Url]=compileFunction(helpers+'\n'+parser+'\n'+decoder+'\nreturn [parseEnvelope,decodeBase64Url];',
 ['canonicalize','LIMITS','parseJson','requireCondition','validateSchema','METADATA_LIFETIMES','sameJson']
)(canonicalize,LIMITS,parseJson,requireCondition,validateSchema,METADATA_LIFETIMES,sameJson);
const bind=compileFunction(body,['request','authorizationBytes','signed','requireCondition','sameJson','equalDigest','sha256']);
const chunks=[];for await(const chunk of process.stdin)chunks.push(chunk);
const cases=JSON.parse(Buffer.concat(chunks).toString('utf8'));
const results=cases.map(({raw,requestRaw,expected})=>{
 try {
  const request=parseJson(Buffer.from(requestRaw,'base64'),LIMITS.metadataBytes);
  validateSchema('bindings/consume-request',request);
  const bytes=Buffer.from(raw,'base64');
  const envelope=parseEnvelope(bytes,expected);
  for(const record of envelope.signatures)decodeBase64Url(record.signature,64,'SIGNATURE_ENCODING_INVALID');
  bind(request,bytes,envelope.signed,requireCondition,sameJson,equalDigest,sha256);
  return true;
 }catch{return false;}
});
process.stdout.write(JSON.stringify(results));
