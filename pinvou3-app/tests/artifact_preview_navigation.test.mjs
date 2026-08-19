import assert from 'node:assert/strict';

import {
  ARTIFACT_PREVIEW_FOCUS_BOUNDARY,
  ARTIFACT_PREVIEW_OPEN_EXTERNAL,
  ARTIFACT_PREVIEW_REQUEST_CLOSE,
  ARTIFACT_PREVIEW_SIZE,
  ARTIFACT_PREVIEW_ZOOM,
  artifactPreviewBootstrap,
  artifactPreviewExternalUrlFromMessage,
  artifactPreviewFocusDirectionFromMessage,
  artifactPreviewRequestsCloseFromMessage,
  artifactPreviewSizeFromMessage,
  artifactPreviewZoomDirectionFromMessage,
  buildArtifactPreviewDocument,
  normalizeUserExternalUrl,
} from '../src/features/artifacts/artifact-preview-navigation.js';

assert.equal(
  normalizeUserExternalUrl('https://example.com/docs?q=1#intro'),
  'https://example.com/docs?q=1#intro',
);
assert.equal(artifactPreviewRequestsCloseFromMessage({ type: ARTIFACT_PREVIEW_REQUEST_CLOSE }), true);
assert.equal(artifactPreviewRequestsCloseFromMessage({ type: 'other' }), false);
assert.equal(
  artifactPreviewFocusDirectionFromMessage({ type: ARTIFACT_PREVIEW_FOCUS_BOUNDARY, direction: 'next' }),
  'next',
);
assert.equal(
  artifactPreviewFocusDirectionFromMessage({ type: ARTIFACT_PREVIEW_FOCUS_BOUNDARY, direction: 'sideways' }),
  '',
);
assert.doesNotThrow(() => new Function(artifactPreviewBootstrap()));
assert.match(artifactPreviewBootstrap(), /var CAN_CLOSE=false/);
assert.match(artifactPreviewBootstrap({ requestClose: true }), /var CAN_CLOSE=true/);
assert.deepEqual(
  artifactPreviewSizeFromMessage({ type: ARTIFACT_PREVIEW_SIZE, width: 1440.2, height: 900.1 }),
  { width: 1441, height: 901 },
);
assert.deepEqual(
  artifactPreviewSizeFromMessage({ type: ARTIFACT_PREVIEW_SIZE, width: 90_000, height: 80_000 }),
  { width: 20_000, height: 20_000 },
);
assert.equal(artifactPreviewSizeFromMessage({ type: ARTIFACT_PREVIEW_SIZE, width: 0, height: 10 }), null);
assert.equal(
  artifactPreviewZoomDirectionFromMessage({ type: ARTIFACT_PREVIEW_ZOOM, direction: 'in' }),
  'in',
);
assert.equal(
  artifactPreviewZoomDirectionFromMessage({ type: ARTIFACT_PREVIEW_ZOOM, direction: 'sideways' }),
  '',
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

const isolatedPreview = buildArtifactPreviewDocument(
  '<main><a href="https://example.com/">Docs</a><form action="https://example.com/collect"><input name="secret"></form></main>',
  { isolated: true },
);
assert.match(isolatedPreview, /Content-Security-Policy/);
assert.match(isolatedPreview, /default-src 'none'/);
assert.match(isolatedPreview, /connect-src 'none'/);
assert.match(isolatedPreview, /form-action 'none'/);
assert.match(isolatedPreview, /base-uri 'none'/);
assert.doesNotMatch(isolatedPreview, /<form action=/);
assert.match(isolatedPreview, /&lt;form action=/);
assert.doesNotMatch(isolatedPreview, /allow-same-origin/);

console.log('artifact preview navigation tests passed');
