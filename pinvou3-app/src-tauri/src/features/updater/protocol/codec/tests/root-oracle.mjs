import { pathToFileURL } from 'node:url';
const reference = pathToFileURL(process.argv[1]);
const { validateTrustedRoot } = await import(reference);
const { parseJson, canonicalize } = await import(new URL('./canonical-json.mjs', reference));
const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const results = JSON.parse(Buffer.concat(chunks).toString('utf8')).map((sample) => {
  try {
    const root = parseJson(Buffer.from(sample, 'base64'));
    validateTrustedRoot(root);
    return Buffer.from(canonicalize(root)).toString('base64');
  } catch { return null; }
});
process.stdout.write(JSON.stringify(results));
