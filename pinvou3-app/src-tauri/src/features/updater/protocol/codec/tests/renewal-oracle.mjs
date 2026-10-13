import { pathToFileURL } from 'node:url';
import { readFile } from 'node:fs/promises';
import { compileFunction } from 'node:vm';
const reference=pathToFileURL(process.argv[1]);
const source=await readFile(reference,'utf8');
function extract(pattern){const match=source.match(pattern);if(!match)throw new Error('original renewal helper unavailable');return match[0];}
const helpers=extract(/const isMetadata = [\s\S]*?const schemaName = [^\n]+;/u);
const parser=extract(/function parseEnvelope\(bytes, expected\) \{\n[\s\S]*?\n\}/u);
const decoder=extract(/function decodeBase64Url\(text, size, code\) \{\n[\s\S]*?\n\}/u);
const {canonicalize,LIMITS,parseJson}=await import(new URL('./canonical-json.mjs',reference));
const {requireCondition}=await import(new URL('./errors.mjs',reference));
const {validateSchema}=await import(new URL('./schema-registry.mjs',reference));
const {METADATA_LIFETIMES,sameJson}=await import(new URL('./semantics.mjs',reference));
const {assertReleaseRenewal}=await import(reference);
const [parseEnvelope,decodeBase64Url]=compileFunction(helpers+'\n'+parser+'\n'+decoder+'\nreturn [parseEnvelope,decodeBase64Url];',
 ['canonicalize','LIMITS','parseJson','requireCondition','validateSchema','METADATA_LIFETIMES','sameJson']
)(canonicalize,LIMITS,parseJson,requireCondition,validateSchema,METADATA_LIFETIMES,sameJson);
const chunks=[];for await(const chunk of process.stdin)chunks.push(chunk);
const cases=JSON.parse(Buffer.concat(chunks).toString('utf8'));
const results=cases.map(({previous,next,previousExpected,nextExpected})=>{
 try {
  const before=parseEnvelope(Buffer.from(previous,'base64'),previousExpected);
  const after=parseEnvelope(Buffer.from(next,'base64'),nextExpected);
  for(const e of [before,after])for(const record of e.signatures)decodeBase64Url(record.signature,64,'SIGNATURE_ENCODING_INVALID');
  assertReleaseRenewal(before.signed,after.signed);
  return true;
 }catch{return false;}
});
process.stdout.write(JSON.stringify(results));
