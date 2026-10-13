import { pathToFileURL } from 'node:url';
const reference = pathToFileURL(process.argv[1]);
const { statusBindingHash, recoveryHandleHash, domainBytes } = await import(reference);
const { parseJson } = await import(new URL('./canonical-json.mjs', reference));
const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const samples = JSON.parse(Buffer.concat(chunks).toString('utf8'));
const results = samples.map((sample) => {
    try {
        if (Object.hasOwn(sample, 'secret')) return recoveryHandleHash(Buffer.from(sample.secret, 'base64'));
        const binding = parseJson(Buffer.from(sample.binding, 'base64'));
        return [statusBindingHash(binding), domainBytes('pinvou-consume-status-v1', binding).toString('base64')];
    } catch { return null; }
});
process.stdout.write(JSON.stringify(results));
