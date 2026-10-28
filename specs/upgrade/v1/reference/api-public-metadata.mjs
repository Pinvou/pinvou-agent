// Fixed-origin public DATA only; never authorizes a private candidate.
export const PUBLIC_METADATA_PATH = '/v1/upgrade/public-metadata/{product}/{component}/{snapshotSha256}/{objectSha256}';

const identifier = { type: 'string', minLength: 1, maxLength: 128,
  pattern: '^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$', not: { pattern: '[^A-Za-z0-9._:-]' } };
const hash = { type: 'string', minLength: 64, maxLength: 64, pattern: '^[0-9a-f]{64}$' };
const error = code => ({ type: 'object', properties: { protocolVersion: { const: 1 }, code: { const: code } },
  required: ['protocolVersion', 'code'], additionalProperties: false });
const machineError = (code, retryable) => ({ type: 'object', properties: { protocolVersion: { const: 1 },
  requestId: { $ref: '#/components/schemas/common/$defs/identifier' }, code: { const: code }, retryable: { const: retryable } },
  required: ['protocolVersion', 'requestId', 'code', 'retryable'], additionalProperties: false });

export const PUBLIC_METADATA_GET = {
  operationId: 'read-public-metadata', tags: ['trust-metadata'], security: [], 'x-owner-task': 'T27',
  'x-maximum-response-bytes': 1_048_576,
  description: 'Fixed-origin original canonical public metadata DATA only. The product and component must match the authenticated publication, '
    + 'and snapshotSha256 must match its exact current Snapshot envelope. objectSha256 must match the complete original Timestamp, '
    + 'Snapshot, or one of the authenticated Snapshot.entries in its complete public closure (including public Target, '
    + 'baseline/ordinary/bridge Release and their Package metadata). A private-only object is never returned, even if its hash is known; '
    + 'an object also independently present in the public closure is public. A missing or changed snapshot requires a new check; '
    + 'it never proves absence of an update or grants installation authority. Source failure or inability to establish membership '
    + 'is unavailable, not missing. No redirects, content encoding, or total elapsed download deadline.',
  parameters: [
    ...['product', 'component'].map(name => ({ name, in: 'path', required: true, schema: identifier })),
    ...['snapshotSha256', 'objectSha256'].map(name => ({ name, in: 'path', required: true, schema: hash })),
  ],
  responses: {
    200: { description: 'Exact original canonical public Timestamp, Snapshot, Target, Release or Package envelope; no wrapper.',
      content: { 'application/json': { schema: { oneOf: ['timestamp', 'snapshot', 'target', 'release', 'package']
        .map(name => ({ $ref: `#/components/schemas/metadata.${name}` })) } } } },
    400: { description: 'Invalid selector.', content: { 'application/json': { schema: machineError('CONTRACT_INVALID', false) } } },
    404: { description: 'Unknown or non-public DATA, or selected snapshot is no longer current.',
      content: { 'application/json': { schema: error('PUBLIC_METADATA_NOT_FOUND') } } },
    503: { description: 'Protected source unavailable; no missing or freshness inference.',
      content: { 'application/json': { schema: machineError('SERVICE_UNAVAILABLE', true) } } },
  },
};
