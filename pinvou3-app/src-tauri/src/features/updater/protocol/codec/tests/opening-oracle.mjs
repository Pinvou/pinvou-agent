import { pathToFileURL } from 'node:url';
const reference=pathToFileURL(process.argv[1]);
const { assertOpening }=await import(reference);
const { parseJson }=await import(new URL('./canonical-json.mjs',reference));
const chunks=[];for await(const chunk of process.stdin)chunks.push(chunk);
const results=JSON.parse(Buffer.concat(chunks).toString('utf8')).map((sample)=>{
  try {const [target,hash,opening]=sample.map((bytes)=>parseJson(Buffer.from(bytes,'base64')));
    assertOpening(target,hash,opening);return true;
  } catch {return false;}
});
process.stdout.write(JSON.stringify(results));
