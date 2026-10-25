import { mkdir, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { schemas as trustSchemas } from './build-schemas.mjs';
import { API_SCHEMAS, API_BASE, ERROR_DEFINITIONS } from './api-shapes.mjs';
import { OPERATIONS } from './api-operations.mjs';
import './api-management.mjs';
import { SCAN_CAPTURE_SECURITY } from './api-scan-captures.mjs';
import { DRAFT_WORKFLOW_SCHEMAS, DRAFT_WORKFLOW_SECURITY } from './api-draft-workflows.mjs';
import { ROOT_DISTRIBUTION_PATH, ROOT_DISTRIBUTION_GET } from './api-root-distribution.mjs';

// Bundle URI references into ordinary OpenAPI-local references for generic
// consumers. Standalone JSON Schemas retain their versioned resource IDs.
const resources = new Map([...trustSchemas].map(([name, schema]) => [schema.$id, name.replaceAll('/', '.')])
  .concat([...API_SCHEMAS].map(([name, schema]) => [schema.$id, `api.${name}`])));
function bundle(value) {
  if (Array.isArray(value)) return value.map(bundle);
  if (value === null || typeof value !== 'object') return value;
  const result = {};
  for (const [key, item] of Object.entries(value)) {
    if (key === '$id') continue;
    if (key === '$ref') {
      const [uri, pointer = ''] = item.split('#');
      const name = resources.get(uri);
      if (name === undefined) throw new Error('OPENAPI_REFERENCE_UNKNOWN');
      result[key] = `#/components/schemas/${name}${pointer}`;
    } else result[key] = bundle(item);
  }
  return result;
}
const paths = {};
for (const operation of OPERATIONS) {
  if (operation.openapi !== undefined) { paths[operation.path] = { post: operation.openapi }; continue; }
  const responses = { 200: { description: 'Committed result or authorized immutable replay with a fresh requestId.',
    content: { 'application/json': { schema: { $ref: `#/components/schemas/api.${operation.name}-response` } } } } };
  const statuses = [...new Set(operation.errors.map((code) => ERROR_DEFINITIONS[code].status))];
  for (const status of statuses) {
    const codes = operation.errors.filter((code) => ERROR_DEFINITIONS[code].status === status);
    const alternatives = codes.map((code) => code === 'SCOPE_UNACTIVATED' ? { $ref: '#/components/schemas/api.unactivated' }
      : bundle(API_SCHEMAS.get('error').oneOf.find((shape) => shape.properties.code.const === code)));
    responses[status] = { description: codes.includes('SCOPE_UNACTIVATED')
      ? 'SCOPE_UNACTIVATED includes checkAfterMs and creates no decision or session; other conflicts use their closed error shapes.'
      : 'Stable error; no secrets or record-existence details.',
    content: { 'application/json': { schema: { oneOf: alternatives } } } };
  }
  paths[operation.path] = { post: { operationId: operation.name, tags: [operation.management ? 'management' : 'client'],
    'x-owner-task': operation.owner, 'x-purpose': operation.purpose, 'x-error-codes': operation.errors,
    security: operation.management ? [{ managementIdentity: [] }] : [],
    description: 'Validate the closed contract and current server-owned read set. A schema-valid request grants no permission.',
    requestBody: { required: true, content: { 'application/json': { schema: { $ref: `#/components/schemas/api.${operation.name}-request` } } } }, responses } };
  if (operation.name === 'time') {
    const post = paths[operation.path].post;
    post['x-maximum-request-bytes'] = 65_536;
    post['x-maximum-response-bytes'] = 65_536;
    post.description = 'T26 fresh-request time observation. No session, upgrade eligibility or credential renewal. '
      + 'Request and response are bounded to 64 KiB; the response is the direct canonical signed Envelope.';
    post.responses[200].description = 'Canonical time Envelope only; no unsigned wrapper or requestId. '
      + 'Clock recovery additionally requires protected Root continuity, the original nonce, caller-proven '
      + 'monotonic send/receive observations and explicit uncertainty/RTT policies.';
  }
}
// Root distribution is not a client Command operation or a bootstrap trust grant.
paths[ROOT_DISTRIBUTION_PATH] = { get: ROOT_DISTRIBUTION_GET };
export const openapi = { openapi: '3.1.0', jsonSchemaDialect: 'https://json-schema.org/draft/2020-12/schema',
  info: { title: 'Pinvou Upgrade V1 Control Contract', version: '1.0.0',
    description: 'T02 executable contract. No backend implementation, provider or production deployment is selected.' },
  servers: [{ url: '/' }], paths, components: { securitySchemes: { managementIdentity: {
    type: 'apiKey', in: 'header', name: 'Authorization', description: 'Opaque identity from the later T05 provider; scopes, MFA and approvals are rechecked on the server.' } },
  schemas: Object.fromEntries([...trustSchemas.values(), ...API_SCHEMAS.values()].map((schema) => [resources.get(schema.$id), bundle(schema)])) } };

// Preserve fixed owner-specific OpenAPI annotations and transport contracts.
Object.assign(openapi.components.schemas, DRAFT_WORKFLOW_SCHEMAS);
Object.assign(openapi.components.securitySchemes, DRAFT_WORKFLOW_SECURITY);
Object.assign(openapi.components.securitySchemes, SCAN_CAPTURE_SECURITY);

export { API_SCHEMAS, API_BASE, OPERATIONS };
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  await mkdir(new URL('../schemas/api/', import.meta.url), { recursive: true });
  for (const [name, schema] of API_SCHEMAS) {
    await writeFile(new URL(`../schemas/api/${name}.json`, import.meta.url), JSON.stringify(schema, null, 2) + '\n');
  }
  // JSON is a YAML 1.2 subset, keeping one deterministic serialization and
  // avoiding a second serializer dependency. Standard YAML loaders can read it.
  await writeFile(new URL('../openapi.yaml', import.meta.url), JSON.stringify(openapi, null, 2) + '\n');
}
