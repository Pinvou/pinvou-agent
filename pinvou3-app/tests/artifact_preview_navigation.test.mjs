import assert from 'node:assert/strict';

import {
  ARTIFACT_PREVIEW_OPEN_EXTERNAL,
  artifactPreviewExternalUrlFromMessage,
  buildArtifactPreviewDocument,
  normalizeUserExternalUrl,
} from '../src/features/artifacts/artifact-preview-navigation.js';

assert.equal(
  normalizeUserExternalUrl('https://example.com/docs?q=1#intro'),
  'https://example.com/docs?q=1#intro',
);
assert.equal(normalizeUserExternalUrl('http://127.0.0.1:8080/preview'), 'http://127.0.0.1:8080/preview');
for (const rejected of [
  'javascript:alert(1)',
  'file:///etc/passwd',
  'data:text/html,hello',
  'https://user@example.com/',
  'https:///missing-host',
  'https://\\example.com',
  'README.md',
  '',
]) {
  assert.equal(normalizeUserExternalUrl(rejected), '', `must reject ${rejected}`);
}

assert.equal(
  artifactPreviewExternalUrlFromMessage({
    type: ARTIFACT_PREVIEW_OPEN_EXTERNAL,
    url: 'https://example.com/',
  }),
  'https://example.com/',
);
assert.equal(
  artifactPreviewExternalUrlFromMessage({
    type: 'untrusted-message',
    url: 'https://example.com/',
  }),
  '',
);

const preview = buildArtifactPreviewDocument(
  '<!doctype html><html><body><a href="https://example.com/">Docs</a></body></html>',
);
assert.match(preview, /window\.parent\.postMessage/);
assert.match(preview, new RegExp(ARTIFACT_PREVIEW_OPEN_EXTERNAL));
assert.match(preview, /document\.addEventListener\("submit"/);
assert.match(preview, /<!doctype html>/i);

console.log('artifact preview navigation tests passed');
