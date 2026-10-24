// Test-only current credential. Fresh signing keys never leave this process.
import { pathToFileURL } from 'node:url';
const reference = pathToFileURL(process.argv[1]);
const { createFixtureSet, DOWNLOAD_URL, NOW } = await import(new URL('./tests/fixtures.mjs', reference));
const { sha256 } = await import(new URL('./digests.mjs', reference));
const { verifyEnvelope } = await import(new URL('./signatures.mjs', reference));
const set = createFixtureSet();
const url = process.argv[2] ?? DOWNLOAD_URL;
const claims = set.claims.download;
claims.sourceBinding.urlSha256 = sha256(Buffer.from(url, 'utf8'));
const bytes = set.sign(claims);
const expectedContext = { role: claims.role, product: claims.product, component: claims.component, scope: claims.scope };
verifyEnvelope(bytes, { trustedRoot: set.metadata.root, expected: expectedContext, now: NOW });
process.stdout.write(JSON.stringify({ name: 'download', trustedRoot: set.metadata.root,
  expectedContext, now: NOW, url, envelopeBase64Url: Buffer.from(bytes).toString('base64url'),
  expectedEnvelopeSha256: sha256(bytes) }));
