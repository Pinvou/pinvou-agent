import { createPublicKey, sign, verify } from 'node:crypto';
import { canonicalize, LIMITS, parseJson } from './canonical-json.mjs';
import { domainBytes, equalDigest, sha256 } from './digests.mjs';
import { requireCondition } from './errors.mjs';
import { validateClaims, validateSchema } from './schema-registry.mjs';
import { checkSemantics, METADATA_LIFETIMES, sameJson } from './semantics.mjs';
import { assertTimeRequest, assertTimeClaims } from './time-contract.mjs';

const isMetadata = (role) => Object.hasOwn(METADATA_LIFETIMES, role) || role === 'package';
const schemaName = (role) => role === 'time' ? 'time/response' : `${isMetadata(role) ? 'metadata' : 'credentials'}/${role}`;

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
    } else {
      requireCondition(policy.component !== null, 'ROLE_SCOPE_INVALID');
      if (policy.role === 'time') requireCondition(policy.channel === null && policy.targetKey === null,
        'TIME_POLICY_SCOPE_INVALID');
    }
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
  requireCondition(expected?.role !== 'time', 'TIME_BOOTSTRAP_REQUIRED');
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

function timeRoot(bytes, product) {
  const envelope = parseEnvelope(bytes, { role: 'root', product, component: null, scope: {} });
  const root = envelope.signed;
  const keys = validateTrustedRoot(root);
  // Intrinsic window structure only. In particular, issuedAt is never used as
  // now to promote an expired historical Root into a current clock authority.
  requireCondition(Number.isSafeInteger(root.issuedAt) && root.issuedAt >= 0
    && Number.isSafeInteger(root.expiresAt) && root.expiresAt > root.issuedAt
    && root.expiresAt - root.issuedAt <= METADATA_LIFETIMES.root, 'TIME_ROOT_WINDOW_INVALID');
  return { envelope, root, keys };
}

function timeContext(context) {
  const required = ['request', 'trustedRootBytes', 'sentMonotonicMs', 'receivedMonotonicMs',
    'maxRoundTripMs', 'maxUncertaintyMs', 'trustedTimeFloor', 'deniedKeyIds'];
  requireCondition(context !== null && typeof context === 'object' && !Array.isArray(context)
    && required.every((field) => Object.hasOwn(context, field))
    && Object.keys(context).every((field) => required.includes(field) || field === 'rootSuccessors'),
  'TIME_CONTEXT_REQUIRED');
  for (const field of ['sentMonotonicMs', 'receivedMonotonicMs', 'maxRoundTripMs',
    'maxUncertaintyMs', 'trustedTimeFloor']) {
    requireCondition(Number.isSafeInteger(context[field]) && context[field] >= 0, 'TIME_CONTEXT_INVALID');
  }
  requireCondition(context.receivedMonotonicMs >= context.sentMonotonicMs, 'TIME_MONOTONIC_INVALID');
  const rtt = context.receivedMonotonicMs - context.sentMonotonicMs;
  requireCondition(rtt <= context.maxRoundTripMs, 'TIME_RTT_EXCEEDED');
  const denied = context.deniedKeyIds;
  requireCondition(Array.isArray(denied) && denied.length <= 4096
    && denied.every((id) => typeof id === 'string' && id.length === 64 && /^[0-9a-f]{64}$/u.test(id))
    && new Set(denied).size === denied.length, 'TIME_DENY_SET_INVALID');
  const successors = Object.hasOwn(context, 'rootSuccessors') ? context.rootSuccessors : [];
  requireCondition(Array.isArray(successors) && successors.length <= 4096, 'ROOT_CHAIN_INVALID');
  return { rtt, denied, successors };
}

/** Pure reference calculation, NOT an actual clock collector or protected
 * capability. The caller supplies the original protected Root bytes, known
 * deny set, original live-request monotonic brackets, explicit policy and
 * protected lower bound. Sent milliseconds must round down before transmission;
 * received milliseconds must round up after the original response is available
 * for verification. Independently truncating both can underestimate RTT. Server
 * uncertainty must include its sampling quantization and actual source error.
 * Zero-valued pure policies assert no real zero-latency/source-error guarantee.
 * A DTO, wall clock or old signed iat proves none of
 * those inputs. The result is DATA as of receivedMonotonicMs only; durable
 * acceptance, boot continuity and cleanup barriers remain T26 owner work.
 */
export function verifyTimeResponse(bytes, context) {
  const { rtt, denied, successors } = timeContext(context);
  const request = assertTimeRequest(context.request);
  let currentBytes = context.trustedRootBytes;
  let current = timeRoot(currentBytes, request.product);
  // The starting bytes must already be independently protected. Their original
  // Envelope may retain old+new transition signatures; rechecking those against
  // only its current Root policy would incorrectly reject a legitimate anchor.
  let newestIssuedAt = current.root.issuedAt;
  for (const nextBytes of successors) {
    const next = timeRoot(nextBytes, request.product);
    requireCondition(next.root.version === current.root.version + 1, 'ROOT_CONTINUITY_INVALID');
    requireCondition(next.envelope.signatures.every((signature) => !denied.includes(signature.keyId)),
      'SIGNING_KEY_DENIED');
    verifySignatures(next.envelope, new Map([...current.keys, ...next.keys]),
      [policyFor(current.root, next.root), policyFor(next.root, next.root)]);
    newestIssuedAt = Math.max(newestIssuedAt, next.root.issuedAt);
    current = next;
    currentBytes = nextBytes;
  }
  const root = { version: current.root.version, sha256: sha256(currentBytes) };
  requireCondition(sameJson(request.root, root), 'TIME_ROOT_BINDING_INVALID');
  const envelope = parseEnvelope(bytes, { role: 'time', product: request.product,
    component: request.component, scope: {} });
  const claims = assertTimeClaims(envelope.signed);
  requireCondition(claims.requestNonce === request.requestNonce && sameJson(claims.root, root),
    'TIME_REQUEST_BINDING_INVALID');
  requireCondition(envelope.signatures.every((signature) => !denied.includes(signature.keyId)),
    'SIGNING_KEY_DENIED');
  verifySignatures(envelope, current.keys, [policyFor(current.root, claims)]);
  // All calculations are exact before converting back to the V1 safe integer
  // domain. A capacity failure cannot become a clipped, apparently usable time.
  requireCondition(claims.uncertaintyMs <= context.maxUncertaintyMs, 'TIME_UNCERTAINTY_EXCEEDED');
  const lower = BigInt(claims.serverTime) - BigInt(claims.uncertaintyMs);
  const upper = BigInt(claims.serverTime) + BigInt(claims.uncertaintyMs) + BigInt(rtt);
  requireCondition(lower >= 0n && upper <= BigInt(Number.MAX_SAFE_INTEGER), 'TIME_INTERVAL_OVERFLOW');
  const earliest = Number(lower), latest = Number(upper);
  requireCondition(earliest >= context.trustedTimeFloor, 'TIME_ROLLBACK');
  requireCondition(newestIssuedAt <= earliest && current.root.expiresAt > latest, 'TIME_ROOT_WINDOW_CLOSED');
  return Object.freeze({ earliest, latest, asOfMonotonicMs: context.receivedMonotonicMs,
    root: Object.freeze(root) });
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

/** Historical Release identity only: validate its original window and scoped
 * signature under already protected archive material, without current rights.
 * Callers must bind archivedRoot to T07's protected Root lineage, never accept
 * a root supplied by an untrusted archive or use this for qualification.
 */
export function verifyArchivedRelease(bytes, { archivedRoot, expected }) {
  requireCondition(expected.role === 'release', 'ARCHIVE_ROLE_INVALID');
  const envelope = parseEnvelope(bytes, expected);
  const keys = validateTrustedRoot(archivedRoot);
  checkSemantics(archivedRoot, { now: archivedRoot.issuedAt });
  verifySignatures(envelope, keys, [policyFor(archivedRoot, envelope.signed)]);
  checkSemantics(envelope.signed, { now: envelope.signed.issuedAt });
  return envelope;
}

export function signEnvelope(signed, privateKeys) {
  validateClaims(schemaName(signed.role), signed);
  if (signed.role === 'time') assertTimeClaims(signed);
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
