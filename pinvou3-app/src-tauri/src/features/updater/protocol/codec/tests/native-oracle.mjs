import { pathToFileURL } from 'node:url';
const reference = pathToFileURL(process.argv[1]);
const { assertNativeIdentity } = await import(reference);
const { parseJson, canonicalize } = await import(new URL('./canonical-json.mjs', reference));
const { validateSchema } = await import(new URL('./schema-registry.mjs', reference));
const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const pairs = JSON.parse(Buffer.concat(chunks).toString('utf8'));
const results = pairs.map((pair) => {
    let expected, actual;
    try {
        [expected, actual] = pair.map((sample) => parseJson(Buffer.from(sample, 'base64')));
        validateSchema('bindings/native-identity', expected);
        validateSchema('bindings/native-identity', actual);
    } catch { return null; }
    let equal = true;
    try { assertNativeIdentity(expected, actual); }
    catch (error) { if (error.code !== 'NATIVE_IDENTITY_MISMATCH') throw error; equal = false; }
    return [equal, Buffer.from(canonicalize(expected)).toString('base64'), Buffer.from(canonicalize(actual)).toString('base64')];
});
process.stdout.write(JSON.stringify(results));
