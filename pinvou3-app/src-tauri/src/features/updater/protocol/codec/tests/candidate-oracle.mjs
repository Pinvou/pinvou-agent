import { pathToFileURL } from 'node:url';
import { readFile } from 'node:fs/promises';
import { compileFunction } from 'node:vm';
const reference=pathToFileURL(process.argv[1]);
const source=await readFile(reference,'utf8');
const chainSource=await readFile(new URL('./metadata-chain.mjs',reference),'utf8');
function extract(text,pattern){const match=text.match(pattern);if(!match)throw new Error('original candidate fragment unavailable');return match[0];}
const helpers=extract(source,/const isMetadata = [\s\S]*?const schemaName = [^\n]+;/u);
const parser=extract(source,/function parseEnvelope\(bytes, expected\) \{\n[\s\S]*?\n\}/u);
const decoder=extract(source,/function decodeBase64Url\(text, size, code\) \{\n[\s\S]*?\n\}/u);
const candidate=extract(chainSource,/export function verifyCandidateRelease\([\s\S]*?\n\}/u);
const guards=[
 extract(candidate,/requireCondition\(target\.baselineRelease\.envelopeSha256 !== sha256\(releaseBytes\), 'CANDIDATE_IS_BASELINE'\);/u),
 extract(candidate,/assertOpening\(target, sha256\(releaseBytes\), opening\);/u),
 extract(candidate,/requireCondition\(release\.targets\.some\(\(entry\) => entry\.targetKey === target\.scope\.targetKey\), 'CANDIDATE_TARGET_MISSING'\);/u),
].join('\n');
const {canonicalize,LIMITS,parseJson}=await import(new URL('./canonical-json.mjs',reference));
const {requireCondition}=await import(new URL('./errors.mjs',reference));
const {validateSchema}=await import(new URL('./schema-registry.mjs',reference));
const {METADATA_LIFETIMES,sameJson,assertOpening}=await import(new URL('./semantics.mjs',reference));
const {sha256}=await import(new URL('./digests.mjs',reference));
const [parseEnvelope,decodeBase64Url]=compileFunction(helpers+'\n'+parser+'\n'+decoder+'\nreturn [parseEnvelope,decodeBase64Url];',
 ['canonicalize','LIMITS','parseJson','requireCondition','validateSchema','METADATA_LIFETIMES','sameJson']
)(canonicalize,LIMITS,parseJson,requireCondition,validateSchema,METADATA_LIFETIMES,sameJson);
const bind=compileFunction(guards,['target','release','releaseBytes','opening','requireCondition','sha256','assertOpening']);
const chunks=[];for await(const chunk of process.stdin)chunks.push(chunk);
const cases=JSON.parse(Buffer.concat(chunks).toString('utf8'));
const results=cases.map(({targetRaw,releaseRaw,openingRaw,targetExpected,releaseExpected})=>{
 try {
  const target=parseEnvelope(Buffer.from(targetRaw,'base64'),targetExpected);
  const bytes=Buffer.from(releaseRaw,'base64');
  const release=parseEnvelope(bytes,releaseExpected);
  for(const e of [target,release])for(const record of e.signatures)decodeBase64Url(record.signature,64,'SIGNATURE_ENCODING_INVALID');
  if(target.signed.role!=='target'||release.signed.role!=='release')return false;
  const opening=parseJson(Buffer.from(openingRaw,'base64'),LIMITS.metadataBytes);
  validateSchema('bindings/opening',opening);
  bind(target.signed,release.signed,bytes,opening,requireCondition,sha256,assertOpening);
  return true;
 }catch{return false;}
});
process.stdout.write(JSON.stringify(results));
