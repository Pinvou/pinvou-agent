import { pathToFileURL } from 'node:url';
const reference=pathToFileURL(process.argv[1]);
const { compareVersions }=await import(reference);
const { parseJson }=await import(new URL('./canonical-json.mjs', reference));
const chunks=[];
for await(const chunk of process.stdin) chunks.push(chunk);
const results=JSON.parse(Buffer.concat(chunks).toString('utf8')).map(([left,right])=>{
  try { return compareVersions(parseJson(Buffer.from(left,'base64')),parseJson(Buffer.from(right,'base64'))); }
  catch { return null; }
});
process.stdout.write(JSON.stringify(results));
