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

// Image contract (PR #658 review): artifact HTML reaches the preview card as
// iframe srcDoc built by buildArtifactPreviewDocument, which supplies neither
// the artifact directory nor an asset resolver, so a relative image src
// resolves against the application URL and renders broken while also hitting
// the host. The visual-design/poster-scene model contract therefore mandates
// inline data URLs. Pin both sides of that handoff here: the builder must
// carry a data-URL image through byte-for-byte (renderable under srcDoc with
// the app's csp:null), and must not fabricate a <base> that would pretend a
// relative src is resolvable — it passes through untouched, i.e. stays broken
// by contract, which is exactly why the skill layer must inline.
const PIXEL_DATA_URL =
  'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';
const inlinedDoc = buildArtifactPreviewDocument(
  `<!doctype html><html><body><img alt="hero" src="${PIXEL_DATA_URL}"></body></html>`,
);
assert.equal(
  inlinedDoc.split(`src="${PIXEL_DATA_URL}"`).length - 1,
  1,
  'builder must preserve an inline data-URL image exactly once, byte-for-byte',
);
assert.match(inlinedDoc, /<img alt="hero" src="data:image\/png;base64,/);

const relativeDoc = buildArtifactPreviewDocument(
  '<!doctype html><html><body><img src="photo.png"></body></html>',
);
assert.ok(
  relativeDoc.includes('src="photo.png"'),
  'builder passes a relative image src through untouched (no rewrite contract)',
);
assert.ok(
  !/<base\s/i.test(relativeDoc),
  'builder must not inject <base>: relative images stay unresolvable in the srcDoc preview',
);
assert.ok(
  !relativeDoc.includes(PIXEL_DATA_URL),
  'builder must not fabricate image data the artifact never inlined',
);

console.log('artifact preview navigation tests passed');
