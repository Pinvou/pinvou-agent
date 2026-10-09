import { createPublicKey, sign, verify } from 'node:crypto';
import { canonicalize, LIMITS, parseJson } from './canonical-json.mjs';
import { domainBytes, equalDigest, sha256 } from './digests.mjs';
import { requireCondition } from './errors.mjs';
import { validateClaims, validateSchema } from './schema-registry.mjs';
import { checkSemantics, METADATA_LIFETIMES, sameJson } from './semantics.mjs';

const isMetadata = (role) => Object.hasOwn(METADATA_LIFETIMES, role) || role === 'package';
const schemaName = (role) => `${isMetadata(role) ? 'metadata' : 'credentials'}/${role}`;

export function signingInput(signed) {
  return domainBytes('pinvou-upgrade-signed-v1', { protocolVersion: signed.protocolVersion,
    role: signed.role, product: signed.product, component: signed.component, scope: signed.scope, signed });
}

function decodeBase64Url(text, size, code) {
  requireCondition(typeof text === 'string' && /^[A-Za-z0-9_-]+$/u.test(text), code);
  const bytes = Buffer.from(text, 'base64url');
  requireCondition(bytes.length === size && bytes.toString('base64url') === text, code);
  return bytes;
}

export function publicKeyDescriptor(key) {
  const jwk = (key.type === 'public' ? key : createPublicKey(key)).export({ format: 'jwk' });
  requireCondition(jwk.kty === 'OKP' && jwk.crv === 'Ed25519', 'KEY_ALGORITHM_INVALID');
  const descriptor = { algorithm: 'ed25519', publicKey: jwk.x };
  return { keyId: sha256(canonicalize(descriptor)), ...descriptor };
}

export function validateTrustedRoot(root) {
  canonicalize(root);
  validateClaims('metadata/root', root);
  const keys = new Map();
  for (const key of root.keys) {
    decodeBase64Url(key.publicKey, 32, 'KEY_ENCODING_INVALID');
    requireCondition(equalDigest(key.keyId, sha256(canonicalize({ algorithm: key.algorithm,
      publicKey: key.publicKey }))) && !keys.has(key.keyId), 'KEY_ID_INVALID');
    keys.set(key.keyId, key);
  }
  const policies = new Set();
  let rootPolicies = 0;
  for (const policy of root.roles) {
    const identity = Buffer.from(canonicalize({ role: policy.role, component: policy.component,
      channel: policy.channel, targetKey: policy.targetKey })).toString('utf8');
    requireCondition(!policies.has(identity), 'ROLE_POLICY_DUPLICATE');
    policies.add(identity);
    requireCondition(policy.threshold <= policy.keyIds.length
      && policy.keyIds.every((id) => keys.has(id)), 'ROLE_THRESHOLD_INVALID');
    if (policy.role === 'root') {
      rootPolicies++;
      requireCondition(policy.component === null && policy.channel === null && policy.targetKey === null,
        'ROOT_SCOPE_INVALID');
      requireCondition(policy.threshold >= 2 && policy.keyIds.length >= 3, 'ROOT_THRESHOLD_INVALID');
    } else requireCondition(policy.component !== null, 'ROLE_SCOPE_INVALID');
  }
  requireCondition(rootPolicies === 1, 'ROOT_POLICY_MISSING');
  return keys;
}

function policyFor(root, signed) {
  requireCondition(root.product === signed.product, 'ROOT_PRODUCT_MISMATCH');
  const policies = root.roles.filter((policy) => policy.role === signed.role
    && policy.component === signed.component
    && policy.channel === (signed.scope.channel ?? null)
    && policy.targetKey === (signed.scope.targetKey ?? null));
  requireCondition(policies.length === 1, 'ROLE_NOT_AUTHORIZED');
  return policies[0];
}

function parseEnvelope(bytes, expected) {
  requireCondition(expected !== null && typeof expected === 'object'
    && ['role', 'product', 'component', 'scope'].every((field) => Object.hasOwn(expected, field)),
  'EXPECTED_CONTEXT_REQUIRED');
  const envelope = parseJson(bytes, isMetadata(expected.role) ? LIMITS.metadataBytes : LIMITS.credentialBytes);
  requireCondition(Buffer.from(canonicalize(envelope)).equals(Buffer.from(bytes)), 'ENVELOPE_NOT_CANONICAL');
  validateSchema(schemaName(expected.role), envelope);
  const signed = envelope.signed;
  requireCondition(signed.role === expected.role && signed.product === expected.product
    && signed.component === expected.component && sameJson(signed.scope, expected.scope), 'CONTEXT_MISMATCH');
  const ids = envelope.signatures.map((signature) => signature.keyId);
  requireCondition(new Set(ids).size === ids.length && sameJson(ids, [...ids].sort()), 'SIGNATURE_ORDER_INVALID');
  return envelope;
}

function verifySignatures(envelope, keys, policies) {
  const input = signingInput(envelope.signed);
  const allowed = new Set(policies.flatMap((policy) => policy.keyIds));
  const verified = new Set();
  for (const signature of envelope.signatures) {
    requireCondition(allowed.has(signature.keyId) && keys.has(signature.keyId), 'SIGNING_KEY_NOT_AUTHORIZED');
    const descriptor = keys.get(signature.keyId);
    const key = createPublicKey({ format: 'jwk', key: { kty: 'OKP', crv: 'Ed25519', x: descriptor.publicKey } });
    const bytes = decodeBase64Url(signature.signature, 64, 'SIGNATURE_ENCODING_INVALID');
    requireCondition(verify(null, input, key, bytes), 'SIGNATURE_INVALID');
    verified.add(signature.keyId);
  }
  for (const policy of policies) {
    requireCondition(policy.keyIds.filter((id) => verified.has(id)).length >= policy.threshold,
      'SIGNATURE_THRESHOLD_UNMET');
  }
  if (Object.hasOwn(envelope.signed, 'signingKeyId')) {
    requireCondition(verified.has(envelope.signed.signingKeyId), 'CREDENTIAL_KEY_BINDING_INVALID');
  }
}

export function assertHighWater(envelope, bytes, highWater) {
  if (highWater === undefined) return;
  const signed = envelope.signed;
  requireCondition(signed.role !== 'package', 'PACKAGE_HIGH_WATER_INVALID');
  const scope = { role: signed.role, product: signed.product, component: signed.component, scope: signed.scope };
  requireCondition(sameJson(highWater.scope, scope), 'HIGH_WATER_SCOPE_INVALID');
  const version = signed.role === 'release' ? signed.revision : signed.version;
  requireCondition(Number.isSafeInteger(highWater.version) && highWater.version >= 1, 'HIGH_WATER_INVALID');
  requireCondition(version >= highWater.version, 'METADATA_ROLLBACK');
  if (version === highWater.version) requireCondition(equalDigest(sha256(bytes), highWater.envelopeSha256),
    'METADATA_EQUIVOCATION');
}

/** trustedRoot must be supplied by the caller's existing protected trust chain,
 * never extracted from this untrusted envelope. This is a pure verifier; it
 * neither persists acceptance nor grants current online execution eligibility.
 */
export function verifyEnvelope(bytes, { trustedRoot, expected, now, highWater, production = true, deniedKeyIds = [] }) {
  const envelope = parseEnvelope(bytes, expected);
  requireCondition(envelope.signatures.every((signature) => !deniedKeyIds.includes(signature.keyId)), 'SIGNING_KEY_DENIED');
  const keys = validateTrustedRoot(trustedRoot);
  checkSemantics(trustedRoot, { now });
  if (envelope.signed.role === 'root') requireCondition(sameJson(envelope.signed, trustedRoot), 'ROOT_TRANSITION_REQUIRED');
  verifySignatures(envelope, keys, [policyFor(trustedRoot, envelope.signed)]);
  checkSemantics(envelope.signed, { now, production });
  if (isMetadata(envelope.signed.role)) assertHighWater(envelope, bytes, highWater);
  return envelope;
}

/** Archive verification proves immutable Package identity only, even after the
 * historical Root expires. It cannot establish present download/install rights.
 */
export function verifyArchivedPackage(bytes, { archivedRoot, expected, now }) {
  requireCondition(expected.role === 'package', 'ARCHIVE_ROLE_INVALID');
  const envelope = parseEnvelope(bytes, expected);
  const keys = validateTrustedRoot(archivedRoot);
  checkSemantics(archivedRoot, { now, allowExpiredRoot: true });
  verifySignatures(envelope, keys, [policyFor(archivedRoot, envelope.signed)]);
  checkSemantics(envelope.signed, { now });
  return envelope;
}

export function signEnvelope(signed, privateKeys) {
  validateClaims(schemaName(signed.role), signed);
  const signatures = privateKeys.map((key) => {
    const descriptor = publicKeyDescriptor(key);
    return { keyId: descriptor.keyId, signature: sign(null, signingInput(signed), key).toString('base64url') };
  }).sort((left, right) => left.keyId < right.keyId ? -1 : left.keyId > right.keyId ? 1 : 0);
  const envelope = { signed, signatures };
  validateSchema(schemaName(signed.role), envelope);
  requireCondition(new Set(signatures.map((entry) => entry.keyId)).size === signatures.length, 'SIGNATURE_DUPLICATE');
  return canonicalize(envelope);
}

/** A pure binding check; successful comparison does not consume an authorization.
 * The caller still supplies trusted context/time and must atomically check current
 * qualification and ownership in the later consume implementation.
 */
export function assertConsumeBinding(request, authorizationBytes, options) {
  validateSchema('bindings/consume-request', request);
  const { signed } = verifyEnvelope(authorizationBytes, options);
  requireCondition(signed.role === `authorization-${request.purpose}`, 'CONSUME_PURPOSE_INVALID');
  requireCondition(equalDigest(request.authorizationEnvelopeSha256, sha256(authorizationBytes)), 'CONSUME_BINDING_INVALID');
  for (const field of ['product', 'component', 'purpose', 'authorizationJti', 'transactionId',
    'clientFactsDigest', 'preparation', 'freezeEpoch', 'backup', 'staged', 'preinstallSlot']) {
    requireCondition(sameJson(request[field], signed[field]), 'CONSUME_BINDING_INVALID');
  }
  requireCondition(request.installationScopeId === signed.scope.installationScopeId
    && sameJson(request.plans, signed.update.plans), 'CONSUME_BINDING_INVALID');
}

/** Successors satisfy both thresholds; new keys may appear only in this explicit
 * continuous-root operation, not in the normal envelope verifier.
 */
function verifyRootTransition(bytes, { trustedRoot, product, now, activeEnvelopes = [] }, intermediate = false) {
  const envelope = parseEnvelope(bytes, { role: 'root', product, component: null, scope: {} });
  const next = envelope.signed;
  const oldKeys = validateTrustedRoot(trustedRoot);
  checkSemantics(trustedRoot, { now, allowExpiredRoot: true });
  const nextKeys = validateTrustedRoot(next);
  requireCondition(trustedRoot.product === product && next.version === trustedRoot.version + 1, 'ROOT_CONTINUITY_INVALID');
  verifySignatures(envelope, new Map([...oldKeys, ...nextKeys]),
    [policyFor(trustedRoot, next), policyFor(next, next)]);
  checkSemantics(next, { now, allowExpiredRoot: intermediate });
  // The publication caller supplies the complete active-component read set.
  // These are signature/authorization checks, not a new eligibility decision;
  // an expired current envelope may still need its signing key preserved.
  requireCondition(Array.isArray(activeEnvelopes), 'ACTIVE_READ_SET_INVALID');
  for (const current of activeEnvelopes) {
    const active = parseEnvelope(current.bytes, current.expected);
    requireCondition(isMetadata(active.signed.role) && active.signed.role !== 'root', 'ACTIVE_READ_SET_INVALID');
    verifySignatures(active, nextKeys, [policyFor(next, active.signed)]);
  }
  return envelope;
}

export function verifyRootSuccessor(bytes, options) {
  return verifyRootTransition(bytes, options);
}

/** Offline clients may traverse expired intermediate roots, but only the final
 * unexpired Root is returned as a usable chain result. Every step is continuous
 * and satisfies both old and new thresholds; no reset or version jump exists.
 */
export function verifyRootChain(successors, { trustedRoot, product, now, activeEnvelopes = [] }) {
  requireCondition(Array.isArray(successors) && successors.length > 0 && successors.length <= 4096,
    'ROOT_CHAIN_INVALID');
  let current = trustedRoot;
  let result;
  for (let i = 0; i < successors.length; i++) {
    const intermediate = i < successors.length - 1;
    result = verifyRootTransition(successors[i], { trustedRoot: current, product, now,
      activeEnvelopes: intermediate ? [] : activeEnvelopes }, intermediate);
    current = result.signed;
  }
  return result;
}

export function assertReleaseRenewal(previous, next) {
  requireCondition(previous.role === 'release' && next.role === 'release'
    && next.revision > previous.revision, 'RELEASE_REVISION_INVALID');
  const project = ({ revision, issuedAt, expiresAt, ...business }) => business;
  requireCondition(sameJson(project(previous), project(next)), 'RELEASE_BUSINESS_CHANGED');
}

export function envelopeReference(bytes) {
  const { signed } = parseJson(bytes);
  return { role: signed.role, scope: signed.scope,
    version: signed.role === 'package' ? null : signed.role === 'release' ? signed.revision : signed.version,
    expiresAt: signed.expiresAt ?? null,
    size: bytes.length, envelopeSha256: sha256(bytes) };
}

export function assertReference(reference, bytes) {
  requireCondition(sameJson(reference, envelopeReference(bytes)), 'REFERENCE_MISMATCH');
}
