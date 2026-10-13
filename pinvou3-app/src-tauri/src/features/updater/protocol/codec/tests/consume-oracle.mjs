import { pathToFileURL } from 'node:url';
const reference = pathToFileURL(process.argv[1]);
const { consumeRequestDigest } = await import(reference);
const { parseJson, canonicalize } = await import(new URL('./canonical-json.mjs', reference));
const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const samples = JSON.parse(Buffer.concat(chunks).toString('utf8'));
const results = samples.map((sample) => {
    try {
        const request = parseJson(Buffer.from(sample, 'base64'));
        const digest = consumeRequestDigest(request);
        const excluded = new Set(['consumeRequestDigest', 'recoverySecret', 'requestId', 'headers']);
        const semantic = Object.fromEntries(Object.entries(request).filter(([key]) => !excluded.has(key)));
        return [digest, Buffer.from(canonicalize(semantic)).toString('base64')];
    } catch { return null; }
});
process.stdout.write(JSON.stringify(results));
