import { createHash, timingSafeEqual } from 'node:crypto';
import { canonicalize } from './canonical-json.mjs';
import { requireCondition } from './errors.mjs';
import { validateSchema } from './schema-registry.mjs';

export const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

export function assertByteIdentity(identity, bytes) {
  validateSchema('bindings/bytes', identity);
  requireCondition(bytes instanceof Uint8Array && bytes.byteLength === identity.size
    && equalDigest(sha256(bytes), identity.sha256), 'ARTIFACT_BYTES_MISMATCH');
}

export function assertNativeIdentity(expected, actual) {
  validateSchema('bindings/native-identity', expected);
  validateSchema('bindings/native-identity', actual);
  requireCondition(Buffer.from(canonicalize(expected)).equals(Buffer.from(canonicalize(actual))), 'NATIVE_IDENTITY_MISMATCH');
}

export function domainBytes(domain, value) {
  requireCondition(/^[a-z0-9-]+$/u.test(domain), 'DOMAIN_INVALID');
  return Buffer.concat([Buffer.from(domain, 'ascii'), Buffer.from([0]), canonicalize(value)]);
}

export function equalDigest(left, right) {
  if (typeof left !== 'string' || typeof right !== 'string'
    || !/^[0-9a-f]{64}$/u.test(left) || !/^[0-9a-f]{64}$/u.test(right)) return false;
  return timingSafeEqual(Buffer.from(left, 'hex'), Buffer.from(right, 'hex'));
}

export function recoveryHandleHash(secret) {
  requireCondition(secret instanceof Uint8Array && secret.byteLength === 32, 'SECRET_SIZE_INVALID');
  return sha256(secret);
}

/** The semantic request schema is validated by the caller before hashing.
 * Only explicitly listed transport/self-referential fields are excluded.
 */
export function consumeRequestDigest(request) {
  const excluded = new Set(['consumeRequestDigest', 'recoverySecret', 'requestId', 'headers']);
  const semantic = Object.fromEntries(Object.entries(request).filter(([key]) => !excluded.has(key)));
  validateSchema('bindings/consume-request', semantic);
  return sha256(canonicalize(semantic));
}

export function statusBindingHash(binding) {
  validateSchema('bindings/status', binding);
  const fields = ['recoveryHandleHash', 'product', 'component', 'installationScopeId',
    'purpose', 'authorizationJti', 'consumeKey', 'transactionId', 'consumeRequestDigest'];
  requireCondition(Object.keys(binding).length === fields.length
    && fields.every((field) => Object.hasOwn(binding, field)), 'STATUS_BINDING_INVALID');
  return sha256(domainBytes('pinvou-consume-status-v1', binding));
}

export function sourceProfileId(facts) {
  const fields = ['product', 'component', 'targetKey', 'canonicalAppVersion',
    'application', 'helper', 'launcher', 'installation', 'dataMigration'];
  requireCondition(fields.every((field) => Object.hasOwn(facts, field)), 'SOURCE_FACTS_MISSING');
  const projection = Object.fromEntries(fields.map((field) => [field, facts[field]]));
  validateSchema('bindings/source-facts', projection);
  return sha256(canonicalize(projection));
}

export function rolloutCommitment(opening) {
  if (opening === null) return sha256(domainBytes('pinvou-rollout-set-v1', []));
  validateSchema('bindings/opening', opening);
  const fields = ['targetKey', 'deploymentId', 'deploymentRevision', 'rolloutId',
    'rolloutRevision', 'releaseEnvelopeSha256', 'leafSalt'];
  requireCondition(Object.keys(opening).length === fields.length
    && fields.every((field) => Object.hasOwn(opening, field)), 'OPENING_INVALID');
  requireCondition(/^[0-9a-f]{64}$/u.test(opening.leafSalt), 'OPENING_SALT_INVALID');
  return sha256(domainBytes('pinvou-rollout-set-v1', [opening]));
}
