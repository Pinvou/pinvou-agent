import { pathToFileURL } from 'node:url';
const { parseJson, canonicalize } = await import(pathToFileURL(process.argv[1]).href);
const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const samples = JSON.parse(Buffer.concat(chunks).toString('utf8'));
const results = samples.map(sample => {
  try {
    return Buffer.from(canonicalize(parseJson(Buffer.from(sample, 'base64')))).toString('base64');
  } catch {
    return null;
  }
});
process.stdout.write(JSON.stringify(results));
