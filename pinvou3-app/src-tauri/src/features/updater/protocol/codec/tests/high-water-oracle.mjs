import {pathToFileURL} from 'node:url';
const reference=pathToFileURL(process.argv[1]);
const {assertHighWater}=await import(reference);
const {parseJson,LIMITS}=await import(new URL('./canonical-json.mjs',reference));
const chunks=[];for await(const chunk of process.stdin)chunks.push(chunk);
const cases=JSON.parse(Buffer.concat(chunks).toString('utf8'));
const results=cases.map(({raw,record})=>{
  try {
    const bytes=Buffer.from(raw,'base64');
    const envelope=parseJson(bytes,LIMITS.metadataBytes);
    const highWater=parseJson(Buffer.from(record,'base64'),LIMITS.metadataBytes);
    // Differential cases have original valid metadata shapes and positive SAFE
    // current versions. assertHighWater is the original exported function.
    assertHighWater(envelope,bytes,highWater);return true;
  }catch{return false;}
});
process.stdout.write(JSON.stringify(results));
