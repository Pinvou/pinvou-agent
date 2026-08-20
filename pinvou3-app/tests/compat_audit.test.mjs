import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import { runAudit } from '../scripts/audit-compat.mjs';
import { staticRuntimeScripts } from '../vite.config.js';

// WebView compatibility contract: the desktop minimum is macOS 11
// (WKWebView = Safari 14.0). scripts/audit-compat.mjs parses every
// verbatim-copied runtime script and HTML inline script at the Safari 14
// syntax ceiling and flags post-14 runtime APIs (guarded calls may carry a
// `safari14-ok` line marker). The dist layer is audited by `npm run
// audit:compat` after a build; this test pins the always-available layers.
const testRoot = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.resolve(testRoot, '..');
const readSrc = (...parts) => fs.readFileSync(path.join(appRoot, 'src', ...parts), 'utf8');

test('static runtime scripts and HTML inline scripts stay within the Safari 14 baseline', () => {
  // A non-existent dist dir keeps the test hermetic: it must pass in CI
  // without a prior UI build and must not depend on a developer's stale dist.
  const violations = runAudit({ distDir: path.join(appRoot, '.compat-test-no-dist') });
  assert.deepEqual(violations, []);
});

test('legacy polyfills load before tailwind runtime and app modules in every entry', () => {
  const entries = [
    ['index.html', /%BASE_URL%vendor\/tailwind\.js/, /app\/main\.jsx/],
    ['reader.html', /vendor\/tailwind\.js/, /app\/reader-main\.jsx/],
    ['pet.html', /app\/pet-main\.jsx/],
  ];
  for (const [entry, ...anchors] of entries) {
    const html = readSrc(entry);
    const polyfill = html.search(/legacy-polyfills\.js/);
    assert.ok(polyfill > 0, `${entry} must load shared/legacy-polyfills.js`);
    for (const anchor of anchors) {
      const at = html.search(anchor);
      assert.ok(at > polyfill, `${entry}: legacy-polyfills.js must precede ${anchor}`);
    }
  }
});

test('vendored markdown/purify scripts stay retired and the polyfill ships as a static script', () => {
  assert.ok(staticRuntimeScripts.has('shared/legacy-polyfills.js'));
  assert.ok(!staticRuntimeScripts.has('vendor/marked.min.js'));
  assert.ok(!staticRuntimeScripts.has('vendor/purify.min.js'));
  assert.ok(!fs.existsSync(path.join(appRoot, 'src/vendor/marked.min.js')));
  assert.ok(!fs.existsSync(path.join(appRoot, 'src/vendor/purify.min.js')));
  // The retired vendor scripts must not be reintroduced through index.html
  // either — a stray tag would 404 on every startup.
  assert.ok(!/vendor\/marked\.min\.js/.test(readSrc('index.html')));
  assert.ok(!/vendor\/purify\.min\.js/.test(readSrc('index.html')));
});

test('vendored tailwind emits inset utilities as physical properties (Safari 14.0 lacks the inset shorthand)', () => {
  const tailwind = readSrc('vendor', 'tailwind.js');
  assert.ok(
    tailwind.includes('["inset",["top","right","bottom","left"]]'),
    'inset emission table must stay expanded to top/right/bottom/left; '
      + 'refreshing vendor/tailwind.js from cdn.tailwindcss.com drops this patch',
  );
  assert.ok(
    !tailwind.includes('["inset",["inset"]]'),
    'the shorthand-only inset mapping must not come back',
  );
});
