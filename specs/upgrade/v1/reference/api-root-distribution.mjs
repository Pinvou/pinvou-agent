// Fixed Root distribution is independent of the 27 POST client operations.
export const ROOT_DISTRIBUTION_PATH = '/v1/upgrade/roots/{product}/{version}.json';
const error = (code) => ({ type: 'object', properties: { protocolVersion: { const: 1 }, code: { const: code } },
  required: ['protocolVersion', 'code'], additionalProperties: false });
const machineError = (code, retryable) => ({ type: 'object', properties: { protocolVersion: { const: 1 },
  requestId: { $ref: '#/components/schemas/common/$defs/identifier' }, code: { const: code }, retryable: { const: retryable } },
  required: ['protocolVersion', 'requestId', 'code', 'retryable'], additionalProperties: false });
export const ROOT_DISTRIBUTION_GET = {
  operationId: 'read-root', tags: ['trust-metadata'], security: [], 'x-owner-task': 'T26',
  'x-maximum-response-bytes': 1_048_576,
  description: 'Fixed-origin original canonical T01 Root DATA. No time, signature, continuity, latest or currentness authority. '
    + 'Expired or time-less Roots remain readable for the dedicated protected-anchor recovery verifier. '
    + 'No redirects, content encoding, retry or total elapsed download deadline.',
  parameters: [
    { name: 'product', in: 'path', required: true, schema: { type: 'string', minLength: 1, maxLength: 128,
      pattern: '^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$', not: { pattern: '[^A-Za-z0-9._:-]' } } },
    { name: 'version', in: 'path', required: true, 'x-canonical-positive-decimal': true,
      description: 'Exact ASCII decimal safe integer, without leading zero, sign, exponent, whitespace or alternate encoding.',
      schema: { type: 'integer', minimum: 1, maximum: 9_007_199_254_740_991 } },
  ],
  responses: {
    200: { description: 'Original complete canonical Root Envelope, role/product/version matching the exact selector. No wrapper.',
      content: { 'application/json': { schema: { $ref: '#/components/schemas/metadata.root' } } } },
    400: { description: 'Invalid selector. No Root bytes.', content: { 'application/json': { schema: machineError('CONTRACT_INVALID', false) } } },
    404: { description: 'Missing DATA only. This does not prove chain tail, absence of a successor or latest/current Root.',
      content: { 'application/json': { schema: error('ROOT_NOT_FOUND') } } },
    503: { description: 'Protected source unavailable; no missing/latest inference.',
      content: { 'application/json': { schema: machineError('SERVICE_UNAVAILABLE', true) } } },
  },
};
