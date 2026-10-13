import { pathToFileURL } from 'node:url';
import { readFile } from 'node:fs/promises';
import { compileFunction } from 'node:vm';
const reference=pathToFileURL(process.argv[1]);
const source=await readFile(reference,'utf8');
function extract(pattern) {
  const match=source.match(pattern);
  if(!match)throw new Error('original envelope helper unavailable');
  return match[0];
}
const helpers=extract(/const isMetadata = [\s\S]*?const schemaName = [^\n]+;/u);
const parser=extract(/function parseEnvelope\(bytes, expected\) \{\n[\s\S]*?\n\}/u);
const decoder=extract(/function decodeBase64Url\(text, size, code\) \{\n[\s\S]*?\n\}/u);
const { canonicalize, LIMITS, parseJson }=await import(new URL('./canonical-json.mjs',reference));
const { requireCondition }=await import(new URL('./errors.mjs',reference));
const { validateSchema }=await import(new URL('./schema-registry.mjs',reference));
const { METADATA_LIFETIMES, sameJson, checkSemantics }=await import(new URL('./semantics.mjs',reference));
// Original complete parser/encoding guards precede original checkSemantics.
// Six metadata roles only. Self-derived context isolates semantics in this test;
// independent production context binding is separately tested in B16.
const [parseEnvelope,decodeBase64Url]=compileFunction(
  helpers+'\n'+parser+'\n'+decoder+'\nreturn [parseEnvelope,decodeBase64Url];',
  ['canonicalize','LIMITS','parseJson','requireCondition','validateSchema','METADATA_LIFETIMES','sameJson']
)(canonicalize,LIMITS,parseJson,requireCondition,validateSchema,METADATA_LIFETIMES,sameJson);
const chunks=[];for await(const chunk of process.stdin)chunks.push(chunk);
const cases=JSON.parse(Buffer.concat(chunks).toString('utf8'));
const results=cases.map(({raw,expected,nowRaw,allowExpired})=>{
  try {
    const envelope=parseEnvelope(Buffer.from(raw,'base64'),expected);
    for(const record of envelope.signatures)decodeBase64Url(record.signature,64,'SIGNATURE_ENCODING_INVALID');
    const now=parseJson(Buffer.from(nowRaw,'base64'),LIMITS.metadataBytes);
    checkSemantics(envelope.signed,{now,production:true,allowExpiredRoot:allowExpired});
    return true;
  }catch{return false;}
});
process.stdout.write(JSON.stringify(results));
