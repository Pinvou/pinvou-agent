import assert from 'node:assert/strict';
import test from 'node:test';
import { openapi } from '../build-api.mjs';
import { PUBLIC_METADATA_GET, PUBLIC_METADATA_PATH } from '../api-public-metadata.mjs';

test('public metadata is one bounded fixed-origin DATA request', () => {
  assert.equal(PUBLIC_METADATA_PATH, '/v1/upgrade/public-metadata/{product}/{component}/{snapshotSha256}/{objectSha256}');
  assert.deepEqual(openapi.paths[PUBLIC_METADATA_PATH], { get: PUBLIC_METADATA_GET });
  assert.deepEqual(PUBLIC_METADATA_GET.security, []);
  assert.equal(PUBLIC_METADATA_GET.requestBody, undefined);
  assert.equal(PUBLIC_METADATA_GET['x-maximum-response-bytes'], 1_048_576);
  assert.match(PUBLIC_METADATA_GET.description, /private-only object is never returned/);
  assert.match(PUBLIC_METADATA_GET.description, /object also independently present in the public closure is public/);
  assert.match(PUBLIC_METADATA_GET.description, /Source failure or inability to establish membership/);
  assert.match(PUBLIC_METADATA_GET.description, /never proves absence of an update/);
});

test('selectors are canonical and the response has only public envelope roles', () => {
  const [product, component, snapshot, object] = PUBLIC_METADATA_GET.parameters;
  for (const field of [product, component]) {
    assert.equal(field.schema.maxLength, 128);
    assert.ok(new RegExp(field.schema.pattern).test('pinvou-desktop'));
    assert.ok(!new RegExp(field.schema.pattern).test('../private'));
    assert.ok(new RegExp(field.schema.not.pattern).test('a%2fb'));
  }
  for (const field of [snapshot, object]) {
    const pattern = new RegExp(field.schema.pattern);
    assert.ok(pattern.test('a'.repeat(64)));
    assert.ok(!pattern.test('A'.repeat(64)));
    assert.ok(!pattern.test('a'.repeat(63)));
  }
  const refs = PUBLIC_METADATA_GET.responses[200].content['application/json'].schema.oneOf.map(item => item.$ref);
  assert.deepEqual(refs, ['timestamp', 'snapshot', 'target', 'release', 'package'].map(name => `#/components/schemas/metadata.${name}`));
  assert.equal(PUBLIC_METADATA_GET.responses[404].content['application/json'].schema.properties.code.const, 'PUBLIC_METADATA_NOT_FOUND');
  assert.equal(PUBLIC_METADATA_GET.responses[404].content['application/json'].schema.additionalProperties, false);
  assert.equal(PUBLIC_METADATA_GET.responses[503].content['application/json'].schema.properties.code.const, 'SERVICE_UNAVAILABLE');
});
