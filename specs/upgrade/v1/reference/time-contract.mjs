// Closed wire DATA only. Neither this syntax check nor a signed timestamp
// establishes a clock observation, protected continuity or upgrade eligibility.
import { validateClaims, validateSchema } from './schema-registry.mjs';
import { requireCondition } from './errors.mjs';

function assertNonce(value) {
  requireCondition(typeof value === 'string' && /^[A-Za-z0-9_-]{43}$/u.test(value),
    'TIME_NONCE_INVALID');
  const bytes = Buffer.from(value, 'base64url');
  requireCondition(bytes.length === 32 && bytes.toString('base64url') === value, 'TIME_NONCE_INVALID');
}

export function assertTimeRequest(request) {
  validateSchema('time/request', request);
  assertNonce(request.requestNonce);
  return request;
}

export function assertTimeClaims(claims) {
  validateClaims('time/response', claims);
  assertNonce(claims.requestNonce);
  return claims;
}
