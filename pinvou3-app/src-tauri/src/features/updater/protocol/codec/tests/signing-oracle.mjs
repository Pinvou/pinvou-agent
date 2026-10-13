import { createPublicKey } from 'node:crypto';
import { pathToFileURL } from 'node:url';
const reference = pathToFileURL(process.argv[1]);
const { signingInput, publicKeyDescriptor, validateTrustedRoot } = await import(reference);
const { parseJson, canonicalize } = await import(new URL('./canonical-json.mjs', reference));
const { sha256 } = await import(new URL('./digests.mjs', reference));
const chunks = [];
for await (const chunk of process.stdin) chunks.push(chunk);
const { samples, root } = JSON.parse(Buffer.concat(chunks).toString('utf8'));
const results = samples.map((sample) => {
  try {
    if (Object.hasOwn(sample, 'signed')) {
      return signingInput(parseJson(Buffer.from(sample.signed, 'base64'))).toString('base64');
    }
    const candidate = structuredClone(root);
    const oldId = candidate.keys[0].keyId;
    const descriptor = { algorithm: 'ed25519', publicKey: sample.publicKey };
    const newId = sha256(canonicalize(descriptor));
    if (!candidate.keys.some((key) => key.publicKey === sample.publicKey)) {
      candidate.keys[0] = { keyId: newId, ...descriptor };
      for (const policy of candidate.roles) {
        policy.keyIds = policy.keyIds.map((id) => id === oldId ? newId : id);
      }
    }
    validateTrustedRoot(candidate);
    const key = createPublicKey({ format: 'jwk', key: { kty: 'OKP', crv: 'Ed25519', x: sample.publicKey } });
    return publicKeyDescriptor(key).keyId;
  } catch {
    return null;
  }
});
process.stdout.write(JSON.stringify(results));
