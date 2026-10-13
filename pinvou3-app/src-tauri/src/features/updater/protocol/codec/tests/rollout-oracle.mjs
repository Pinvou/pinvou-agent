import { pathToFileURL } from 'node:url';
const reference = pathToFileURL(process.argv[1]);
const { rolloutCommitment, domainBytes } = await import(reference);
const { parseJson } = await import(new URL('./canonical-json.mjs', reference));
const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const samples = JSON.parse(Buffer.concat(chunks).toString('utf8'));
const results = samples.map((sample) => {
    try {
        const opening = parseJson(Buffer.from(sample, 'base64'));
        return [rolloutCommitment(opening), domainBytes('pinvou-rollout-set-v1', opening === null ? [] : [opening]).toString('base64')];
    } catch { return null; }
});
process.stdout.write(JSON.stringify(results));
