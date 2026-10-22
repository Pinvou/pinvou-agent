// Fixed draft-owner wire contracts. Schema validity grants no authority.
// Keep their actual HTTP transports separate from the legacy common wrapper.
import contracts from './draft-workflow-contracts.json' with { type: 'json' };
import { schemas as trustSchemas } from './build-schemas.mjs';
import { API_BASE, API_SCHEMAS, addShape } from './api-shapes.mjs';
import { OPERATIONS } from './api-operations.mjs';

const resourceIds = new Map([...trustSchemas].map(([name, schema]) =>
  [name.replaceAll('/', '.'), schema.$id]));
for (const name of API_SCHEMAS.keys()) resourceIds.set(`api.${name}`, API_BASE + name);
for (const name of Object.keys(contracts.schemas)) resourceIds.set(name, API_BASE + name.slice(4));

// OpenAPI annotations remain in the public document. Versioned standalone
// schemas use ordinary JSON Schema vocabulary, so strict validators need no
// permissive keyword registration or disabled strict mode.
function standalone(value) {
  if (Array.isArray(value)) return value.map(standalone);
  if (value === null || typeof value !== 'object') return value;
  const result = {};
  if (Object.hasOwn(value, 'properties') && value.type === undefined) result.type = 'object';
  // The original floor version refinements inherit string type via allOf.
  if (value.type === undefined && value.allOf?.some(item => item.$ref === '#/components/schemas/common/$defs/version')) result.type = 'string';
  for (const [key, child] of Object.entries(value)) {
    if (key === 'discriminator' || key.startsWith('x-')) continue;
    if (key === '$ref') {
      const prefix = '#/components/schemas/';
      if (typeof child !== 'string' || !child.startsWith(prefix)) throw new Error('DRAFT_SCHEMA_REFERENCE_INVALID');
      const [name, ...pointer] = child.slice(prefix.length).split('/');
      const id = resourceIds.get(name);
      if (id === undefined) throw new Error('DRAFT_SCHEMA_REFERENCE_UNKNOWN');
      result.$ref = id + (pointer.length ? '#/' + pointer.join('/') : '');
    } else result[key] = standalone(child);
  }
  return result;
}
for (const [name, shape] of Object.entries(contracts.schemas)) {
  if (!name.startsWith('api.') || API_SCHEMAS.has(name.slice(4))) throw new Error('DRAFT_SCHEMA_DUPLICATE');
  addShape(name.slice(4), standalone(shape));
}
for (const [path, endpoint] of Object.entries(contracts.paths)) {
  const post = endpoint.post;
  if (Object.keys(endpoint).length !== 1 || !post || OPERATIONS.some(item => item.path === path || item.name === post.operationId)) throw new Error('DRAFT_OPERATION_DUPLICATE');
  const request = post.requestBody.content['application/json'].schema.$ref;
  const response = post.responses['200'].content['application/json'].schema.$ref;
  const shapeName = ref => {
    const prefix = '#/components/schemas/api.';
    if (typeof ref !== 'string' || !ref.startsWith(prefix) || !API_SCHEMAS.has(ref.slice(prefix.length))) throw new Error('DRAFT_OPERATION_SCHEMA_UNKNOWN');
    return ref.slice(prefix.length);
  };
  OPERATIONS.push({ name: post.operationId, path, owner: post['x-owner-task'],
    purpose: post['x-purpose'], management: true, errors: post['x-error-codes'],
    requestShape: shapeName(request), responseShape: shapeName(response),
    privateFloorResponse: path.startsWith('/v1/admin/path-policy/floor-drafts/'), openapi: post });
}
export const DRAFT_WORKFLOW_SCHEMAS = contracts.schemas;
export const DRAFT_WORKFLOW_SECURITY = contracts.securitySchemes;
