import assert from 'node:assert/strict';
import test from 'node:test';
import { openapi, OPERATIONS } from '../build-api.mjs';
import { ROOT_DISTRIBUTION_PATH, ROOT_DISTRIBUTION_GET } from '../api-root-distribution.mjs';

test('Root GET is independent of all 27 existing POST client commands', () => {
  const clients = OPERATIONS.filter((operation) => !operation.management);
  assert.equal(clients.length, 27);
  for (const operation of clients) {
    assert.ok(openapi.paths[operation.path].post);
    assert.notEqual(operation.path, ROOT_DISTRIBUTION_PATH);
  }
  assert.deepEqual(openapi.paths[ROOT_DISTRIBUTION_PATH], { get: ROOT_DISTRIBUTION_GET });
  assert.deepEqual(ROOT_DISTRIBUTION_GET.tags, ['trust-metadata']);
  assert.deepEqual(ROOT_DISTRIBUTION_GET.security, []);
  assert.equal(ROOT_DISTRIBUTION_GET.requestBody, undefined);
});
test('Root GET binds exact closed selectors and the original metadata Envelope', () => {
  const get = ROOT_DISTRIBUTION_GET;
  assert.equal(ROOT_DISTRIBUTION_PATH, '/v1/upgrade/roots/{product}/{version}.json');
  assert.equal(get['x-maximum-response-bytes'], 1_048_576);
  const [product, version] = get.parameters;
  const pattern = new RegExp(product.schema.pattern);
  const forbidden = new RegExp(product.schema.not.pattern);
  const matches = (value) => pattern.test(value) && !forbidden.test(value);
  for (const valid of ['pinvou', 'Pinvou.app_x:prod-1', 'a'.repeat(128)]) assert.ok(matches(valid));
  for (const invalid of ['', '/pinvou', 'pinvou/app', 'pinvou?x', 'pinvou%2fapp', '中', 'a\n', 'a'.repeat(129)]) assert.ok(!matches(invalid));
  assert.equal(version['x-canonical-positive-decimal'], true);
  assert.deepEqual(version.schema, { type: 'integer', minimum: 1, maximum: Number.MAX_SAFE_INTEGER });
  assert.equal(get.responses[200].content['application/json'].schema.$ref, '#/components/schemas/metadata.root');
  assert.ok(openapi.components.schemas['metadata.root']);
});
test('Missing response is closed DATA and cannot express latest or continuity authority', () => {
  const shape = ROOT_DISTRIBUTION_GET.responses[404].content['application/json'].schema;
  assert.deepEqual(shape, { type: 'object', properties: { protocolVersion: { const: 1 }, code: { const: 'ROOT_NOT_FOUND' } },
    required: ['protocolVersion', 'code'], additionalProperties: false });
  for (const [status, code, retryable] of [[400, 'CONTRACT_INVALID', false], [503, 'SERVICE_UNAVAILABLE', true]]) {
    const machine = ROOT_DISTRIBUTION_GET.responses[status].content['application/json'].schema;
    assert.deepEqual(machine.required, ['protocolVersion', 'requestId', 'code', 'retryable']);
    assert.equal(machine.properties.code.const, code);
    assert.equal(machine.properties.retryable.const, retryable);
    assert.equal(machine.additionalProperties, false);
  }
  assert.match(ROOT_DISTRIBUTION_GET.responses[404].description, /does not prove chain tail/);
  assert.match(ROOT_DISTRIBUTION_GET.description, /No time, signature, continuity, latest or currentness authority/);
});
