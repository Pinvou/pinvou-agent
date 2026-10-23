// Four fixed metadata routes for the original reported-only capture owner.
// This module registers contracts, never a scanner or command dispatcher.
import contracts from './scan-capture-contracts.json' with { type: 'json' };
import { API_BASE, API_SCHEMAS, addShape } from './api-shapes.mjs';
import { OPERATIONS } from './api-operations.mjs';

for (const [name, schema] of Object.entries(contracts.schemas)) {
  if (!name.startsWith('scan-capture.') || API_SCHEMAS.has(name)
    || schema.$id !== API_BASE + name || schema.$schema !== 'https://json-schema.org/draft/2020-12/schema') {
    throw new Error('SCAN_CAPTURE_SCHEMA_INVALID');
  }
  addShape(name, schema);
}
const prefix = '/v1/admin/supply-chain/scan-captures/';
const routes = ['context', 'queue', 'lookup', 'resume'];
if (Object.keys(contracts.paths).length !== routes.length) throw new Error('SCAN_CAPTURE_ROUTE_INVALID');
for (const route of routes) {
  const path = prefix + route, endpoint = contracts.paths[path], post = endpoint?.post;
  if (!post || Object.keys(endpoint).length !== 1 || post.operationId !== 'scan-capture-' + route
    || OPERATIONS.some(item => item.path === path || item.name === post.operationId)) throw new Error('SCAN_CAPTURE_ROUTE_INVALID');
  const requestShape = `scan-capture.${route}-request`, responseShape = `scan-capture.${route}-response`;
  if (!API_SCHEMAS.has(requestShape) || !API_SCHEMAS.has(responseShape)
    || post.requestBody.content['application/json'].schema.$ref !== '#/components/schemas/api.' + requestShape
    || post.responses['200'].content['application/json'].schema.$ref !== '#/components/schemas/api.' + responseShape) {
    throw new Error('SCAN_CAPTURE_ROUTE_SHAPE_INVALID');
  }
  OPERATIONS.push({ name: post.operationId, path, owner: 'T10', purpose: 'management', management: true,
    errors: post['x-error-codes'], requestShape, responseShape, openapi: post });
}
export const SCAN_CAPTURE_SECURITY = contracts.securitySchemes;
