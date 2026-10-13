import { pathToFileURL } from 'node:url';
const reference = pathToFileURL(process.argv[1]);
const { sourceProfileId } = await import(reference);
const { parseJson, canonicalize } = await import(new URL('./canonical-json.mjs', reference));
const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const samples = JSON.parse(Buffer.concat(chunks).toString('utf8'));
const fields = ['product', 'component', 'targetKey', 'canonicalAppVersion', 'application', 'helper', 'launcher', 'installation', 'dataMigration'];
const results = samples.map((sample) => {
    try {
        const facts = parseJson(Buffer.from(sample, 'base64'));
        const id = sourceProfileId(facts);
        const projection = Object.fromEntries(fields.map((field) => [field, facts[field]]));
        return [id, Buffer.from(canonicalize(projection)).toString('base64')];
    } catch { return null; }
});
process.stdout.write(JSON.stringify(results));
