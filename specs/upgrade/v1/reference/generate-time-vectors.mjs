// Public test material only. Fresh private keys exist in this process's memory
// and are neither serialized nor written. No protected-owner authority follows.
import { generateKeyPairSync } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { canonicalize } from './canonical-json.mjs';
import { sha256 } from './digests.mjs';
import { publicKeyDescriptor, signEnvelope } from './signatures.mjs';

export function createTimeVectors() {
  const oldKeys = Array.from({ length: 3 }, () => generateKeyPairSync('ed25519').privateKey);
  const nextKeys = Array.from({ length: 3 }, () => generateKeyPairSync('ed25519').privateKey);
  const timeKeys = Array.from({ length: 2 }, () => generateKeyPairSync('ed25519').privateKey);
  const descriptor = (keys) => keys.map(publicKeyDescriptor);
  const rootPolicy = (keys) => ({ role: 'root', component: null, channel: null, targetKey: null,
    threshold: 2, keyIds: descriptor(keys).map((entry) => entry.keyId) });
  const serverTime = 1_700_000_000_100;
  const initial = { protocolVersion: 1, role: 'root', product: 'pinvou', component: null, scope: {},
    version: 1, issuedAt: serverTime - 86_400_000, expiresAt: serverTime - 1,
    keys: descriptor(oldKeys), roles: [rootPolicy(oldKeys)] };
  const final = { ...initial, version: 2, issuedAt: serverTime - 100, expiresAt: serverTime + 86_400_000,
    keys: descriptor([...oldKeys, ...nextKeys, ...timeKeys]), roles: [rootPolicy(nextKeys),
      { role: 'time', component: 'app', channel: null, targetKey: null,
        threshold: 2, keyIds: descriptor(timeKeys).map((entry) => entry.keyId) }] };
  const initialBytes = signEnvelope(initial, oldKeys.slice(0, 2));
  // Keep the real old+new signatures in this exact final Envelope. The direct
  // case deliberately uses these same bytes as an already protected anchor.
  const finalBytes = signEnvelope(final, [...oldKeys.slice(0, 2), ...nextKeys.slice(0, 2)]);
  const root = { version: 2, sha256: sha256(finalBytes) };
  const request = { protocolVersion: 1, product: 'pinvou', component: 'app',
    requestNonce: Buffer.alloc(32, 7).toString('base64url'), root };
  const response = signEnvelope({ protocolVersion: 1, role: 'time', product: 'pinvou', component: 'app',
    scope: {}, purpose: 'trusted-time.v1', requestNonce: request.requestNonce, root,
    serverTime, uncertaintyMs: 5 }, timeKeys);
  const observation = { sentMonotonicMs: 1000, receivedMonotonicMs: 1020,
    maxRoundTripMs: 20, maxUncertaintyMs: 5, trustedTimeFloor: serverTime - 5, deniedKeyIds: [] };
  const expected = { earliest: serverTime - 5, latest: serverTime + 25, asOfMonotonicMs: 1020, root };
  return { protocolVersion: 1, boundary: 'Public cryptographic/reference DATA; not protected clock acceptance.',
    cases: [
      { name: 'expired-root-continuous-successor', request,
        trustedRootBytes: Buffer.from(initialBytes).toString('utf8'),
        rootSuccessors: [Buffer.from(finalBytes).toString('utf8')],
        responseBytes: Buffer.from(response).toString('utf8'), ...observation, expected },
      { name: 'protected-final-root-with-historical-cross-signatures', request,
        trustedRootBytes: Buffer.from(finalBytes).toString('utf8'), rootSuccessors: [],
        responseBytes: Buffer.from(response).toString('utf8'), ...observation, expected },
    ] };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const destination = new URL('../vectors/time/anchor.json', import.meta.url);
  await mkdir(new URL('.', destination), { recursive: true });
  await writeFile(destination, Buffer.concat([Buffer.from(canonicalize(createTimeVectors())), Buffer.from('\n')]));
}
