import { pathToFileURL } from 'node:url';
import { readFile } from 'node:fs/promises';
import { compileFunction } from 'node:vm';
const signature=pathToFileURL(process.argv[1]);
const source=await readFile(signature,'utf8');
const chainSource=await readFile(new URL('./metadata-chain.mjs',signature),'utf8');
function extract(source,pattern){const match=source.match(pattern);if(!match)throw new Error('original Release/Package helper unavailable');return match[0];}
const helpers=extract(source,/const isMetadata = [\s\S]*?const schemaName = [^\n]+;/u);
const parser=extract(source,/function parseEnvelope\(bytes, expected\) \{\n[\s\S]*?\n\}/u);
const decoder=extract(source,/function decodeBase64Url\(text, size, code\) \{\n[\s\S]*?\n\}/u);
const binding=extract(chainSource,/function assertReleasePackageBinding\(release, target, packageManifest\) \{\n[\s\S]*?\n\}/u);
const {canonicalize,LIMITS,parseJson}=await import(new URL('./canonical-json.mjs',signature));
const {requireCondition}=await import(new URL('./errors.mjs',signature));
const {validateSchema}=await import(new URL('./schema-registry.mjs',signature));
const {METADATA_LIFETIMES,sameJson}=await import(new URL('./semantics.mjs',signature));
const {assertReference}=await import(signature);
const [parseEnvelope,decodeBase64Url]=compileFunction(helpers+'\n'+parser+'\n'+decoder+'\nreturn [parseEnvelope,decodeBase64Url];',
 ['canonicalize','LIMITS','parseJson','requireCondition','validateSchema','METADATA_LIFETIMES','sameJson']
)(canonicalize,LIMITS,parseJson,requireCondition,validateSchema,METADATA_LIFETIMES,sameJson);
const bind=compileFunction(binding+'\nreturn assertReleasePackageBinding;',['requireCondition','sameJson'])(requireCondition,sameJson);
const targets=['windows-x86_64-nsis-perMachine','linux-x86_64-deb-perMachine','linux-arm64-deb-perMachine','macos-universal-dmg-perMachine'];
const chunks=[];for await(const chunk of process.stdin)chunks.push(chunk);
const cases=JSON.parse(Buffer.concat(chunks).toString('utf8'));
const results=cases.map(({release,packageRaw,releaseExpected,packageExpected,targetRaw})=>{
 try {
  const key=parseJson(Buffer.from(targetRaw,'base64'),LIMITS.metadataBytes);
  if(!targets.includes(key))return false;
  const before=parseEnvelope(Buffer.from(release,'base64'),releaseExpected);
  const packageBytes=Buffer.from(packageRaw,'base64');
  const after=parseEnvelope(packageBytes,packageExpected);
  for(const e of [before,after])for(const record of e.signatures)decodeBase64Url(record.signature,64,'SIGNATURE_ENCODING_INVALID');
  if(before.signed.role!=='release'||after.signed.role!=='package')return false;
  const target=before.signed.targets.find((entry)=>entry.targetKey===key);
  requireCondition(target!==undefined,'RELEASE_TARGET_MISSING');
  assertReference(target.packageManifest,packageBytes);
  bind(before.signed,target,after.signed);
  return true;
 }catch{return false;}
});
process.stdout.write(JSON.stringify(results));
