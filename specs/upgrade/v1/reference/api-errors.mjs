import { ERROR_DEFINITIONS } from './api-shapes.mjs';
import { assertApiShape } from './api-registry.mjs';
import { requireCondition } from './errors.mjs';

/** Never serialize Error.message, submitted values, SQL, credential or URL.
 * The externally visible response is assembled only from frozen fields.
 */
export function errorResponse(code, requestId, retryAfterMs = null) {
  const definition = ERROR_DEFINITIONS[code];
  requireCondition(definition !== undefined && code !== 'SCOPE_UNACTIVATED', 'API_ERROR_UNKNOWN');
  const body = { protocolVersion: 1, requestId, code, retryable: definition.retryable };
  if (retryAfterMs !== null) body.retryAfterMs = retryAfterMs;
  assertApiShape('error', body);
  return { status: definition.status, body };
}
export function unactivatedCheck(requestId, checkAfterMs) {
  const body = { protocolVersion: 1, requestId, code: 'SCOPE_UNACTIVATED', retryable: true, checkAfterMs };
  assertApiShape('unactivated', body);
  return { status: 409, body };
}
