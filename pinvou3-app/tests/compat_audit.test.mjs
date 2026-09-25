import assert from 'node:assert/strict';
import fs from 'node:fs';
import { createRequire } from 'node:module';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import postcss from 'postcss';
import tailwindcss from 'tailwindcss';

import { runAudit } from '../scripts/audit-compat.mjs';
import { staticRuntimeScripts } from '../vite.config.mjs';

// WebView compatibility contract: the desktop minimum is macOS 11
// (WKWebView = Safari 14.0). scripts/audit-compat.mjs parses every
// verbatim-copied runtime script and HTML inline script at the Safari 14
// syntax ceiling and flags post-14 runtime APIs (guarded calls may carry a
// `safari14-ok` line marker), and scans built dist assets: JS chunks for the
// same parse/API violations, CSS for the `inset:` shorthand that only the
// build's target-aware lightningcss pass expands to physical properties.
// `npm run audit:compat` runs the full audit after a build; the tests below
// pin both the always-available layers and the dist-CSS scan via fixtures.
const testRoot = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.resolve(testRoot, '..');
const readSrc = (...parts) => fs.readFileSync(path.join(appRoot, 'src', ...parts), 'utf8');
const { browserslistToTargets, transform: transformCss } = createRequire(import.meta.url)('lightningcss');

test('static runtime scripts and HTML inline scripts stay within the Safari 14 baseline', () => {
  // A non-existent dist dir keeps the test hermetic: it must pass in CI
  // without a prior UI build and must not depend on a developer's stale dist.
  const violations = runAudit({ distDir: path.join(appRoot, '.compat-test-no-dist') });
  assert.deepEqual(violations, []);
});

test('legacy polyfills load before app modules in every entry', () => {
  const entries = [
    ['index.html', /app\/main\.jsx/],
    ['reader.html', /app\/reader-main\.jsx/],
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

test('vendored browser runtimes stay retired and the polyfill ships as a static script', () => {
  assert.ok(staticRuntimeScripts.has('shared/legacy-polyfills.js'));
  assert.ok(!staticRuntimeScripts.has('vendor/tailwind.js'));
  assert.ok(!staticRuntimeScripts.has('vendor/marked.min.js'));
  assert.ok(!staticRuntimeScripts.has('vendor/purify.min.js'));
  assert.ok(!fs.existsSync(path.join(appRoot, 'src/vendor/marked.min.js')));
  assert.ok(!fs.existsSync(path.join(appRoot, 'src/vendor/purify.min.js')));
  assert.ok(!fs.existsSync(path.join(appRoot, 'src/vendor/tailwind.js')));
  // The retired vendor scripts must not be reintroduced through index.html
  // either — a stray tag would 404 on every startup.
  assert.ok(!/vendor\/marked\.min\.js/.test(readSrc('index.html')));
  assert.ok(!/vendor\/purify\.min\.js/.test(readSrc('index.html')));
  for (const entry of ['index.html', 'reader.html']) {
    assert.ok(!/vendor\/tailwind\.js/.test(readSrc(entry)));
    assert.ok(!/tailwind\.config/.test(readSrc(entry)));
  }
});

test('the auditor flags violations under minifier-shaped syntax (complete walker contract)', () => {
  // Adversarial fixtures in the exact shapes a minifier emits for dist
  // chunks: sequence expressions, parameter default/pattern positions, and
  // RegExp constructor calls. An earlier hand-maintained child-key table
  // skipped these node types entirely, so a lookbehind hidden behind `(0, ...)`
  // sailed through the audit. Every fixture below MUST be reported.
  const distDir = fs.mkdtempSync(path.join(os.tmpdir(), 'compat-audit-adv-'));
  try {
    fs.mkdirSync(path.join(distDir, 'assets'), { recursive: true });
    fs.writeFileSync(path.join(distDir, 'assets', 'adv.js'), [
      'const x = (0, /(?<=a)b/);',
      'function f(y = /(?<=a)b/) {}',
      'new RegExp("(?<=a)b");',
      'new RegExp(src, "v");',
      'const z = (arr.findLast(n => n), 1);',
      'function g(w = [1, 2].findLastIndex(() => 0)) {}',
      'const s = (0, structuredClone({}));',
    ].join('\n'));
    const violations = runAudit({ distDir });
    const expected = [
      [1, 'lookbehind assertion'],
      [2, 'lookbehind assertion'],
      [3, 'lookbehind assertion'],
      [4, 'regex flag "v"'],
      [5, '.findLast() invocation'],
      [6, '.findLastIndex() invocation'],
      [7, 'structuredClone'],
    ];
    for (const [line, needle] of expected) {
      assert.ok(
        violations.some(v => v.startsWith(`dist:adv.js:${line}:`) && v.includes(needle)),
        `line ${line} must be reported (${needle}); got: ${JSON.stringify(violations)}`,
      );
    }
  } finally {
    fs.rmSync(distDir, { recursive: true, force: true });
  }
});

test('build-time tailwind emits inset utilities as physical properties for Safari 14.0', async () => {
  const result = await postcss([
    tailwindcss({
      content: [{ raw: '<div class="fixed inset-0"></div>', extension: 'html' }],
      corePlugins: { preflight: false },
    }),
  ]).process('@tailwind utilities;', { from: undefined });
  const compiled = transformCss({
    filename: 'tailwind.css',
    code: Buffer.from(result.css),
    targets: browserslistToTargets(['safari 14']),
    minify: true,
  }).code.toString();
  assert.match(compiled, /\.inset-0\{top:0;bottom:0;left:0;right:0\}/);
  assert.doesNotMatch(compiled, /\binset:0/);
});

test('the auditor flags inset shorthand that survives into dist CSS (Safari 14.0 contract)', () => {
  // The compiled-CSS layer guards the other half of the contract above: the
  // physical-property output depends on vite's `target/cssTarget: 'safari14'`
  // reaching the lightningcss minify pass, and a config change there is
  // silent. The audit therefore fails on the shorthand in the built artifact
  // itself. Fixtures cover the shapes that must and must not be reported:
  // Tailwind's raw JIT output (`inset:0px`), the expanded physical form, the
  // unrelated clip-path `inset()` function, and the real minified output of
  // Tailwind's `--tw-ring-inset` custom property / `.ring-inset` class name,
  // which must not phantom-match as shorthand declarations.
  const distDir = fs.mkdtempSync(path.join(os.tmpdir(), 'compat-audit-css-'));
  try {
    fs.mkdirSync(path.join(distDir, 'assets'), { recursive: true });
    fs.writeFileSync(path.join(distDir, 'assets', 'main.css'), [
      '.overlay{position:fixed;inset:0px;z-index:50}',
      '.ok{position:fixed;top:0;bottom:0;left:0;right:0}',
      '.shape{clip-path:inset(50%)}',
      '*{--tw-numeric-fraction: ;--tw-ring-inset: ;--tw-ring-offset-width:0px}',
      '.ring-inset{--tw-ring-inset:inset}',
    ].join('\n'));
    const violations = runAudit({ distDir });
    assert.equal(violations.length, 1, `exactly the surviving shorthand must be reported; got: ${JSON.stringify(violations)}`);
    assert.match(violations[0], /^dist:main\.css:1: inset shorthand survives the build/);
    assert.match(violations[0], /inset:0px/);
  } finally {
    fs.rmSync(distDir, { recursive: true, force: true });
  }
});
