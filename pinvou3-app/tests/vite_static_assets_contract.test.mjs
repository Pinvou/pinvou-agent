import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  localClassicScriptPaths,
  resolveContainedRuntimePath,
} from '../scripts/vite-runtime-assets.mjs';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const sourceRoot = path.join(appRoot, 'src');
const distRoot = path.join(appRoot, 'dist');
const distIndexPath = path.join(distRoot, 'index.html');
const webDistIndexPath = path.resolve(appRoot, '../remote-control-relay/web/dist/index.html');

function assertStartupBundles({ built, expectedPlatform, outputRoot, webBuild }) {
  const startup = built.filter(relative => relative.startsWith('startup/'));
  assert.ok(startup.length > 0, `${expectedPlatform} index must load classic startup bundles`);
  // Exact pin of the intentionally unbundled startup scripts (keep in sync
  // with the allowlist test in classic_startup_bundle.test.mjs): anything
  // else retained here means a script silently dropped out of the bundle
  // manifest, so its source tag survived the transform.
  const retained = built.filter(relative => !relative.startsWith('startup/'));
  assert.deepEqual(
    new Set(retained),
    new Set(webBuild
      ? ['features/updater/update-notice-logic.js', 'platform/web/bootstrap.js', 'shared/legacy-polyfills.js']
      : ['features/updater/update-notice-logic.js', 'shared/legacy-polyfills.js']),
    `${expectedPlatform} index must retain only the intentionally unbundled startup scripts`,
  );
  assert.ok(startup.length + retained.length <= 10, `${expectedPlatform} classic startup scripts exceed the request budget`);
  for (const relative of startup) {
    assert.match(
      relative,
      new RegExp(`^startup/pinvou-${expectedPlatform}-classic-\\d+-[a-f0-9]{8}\\.js$`, 'u'),
    );
    assert.ok(
      fs.statSync(resolveContainedRuntimePath(outputRoot, relative)).isFile(),
      `missing ${expectedPlatform} startup bundle: ${relative}`,
    );
  }
}

test('classic runtime script parser recognizes only real HTML attributes', () => {
  const html = `
    <!-- <script src="/commented.js"></script> -->
    <script data-src="/data-only.js"></script>
    <script data-src="/decoy.js" data-type="module" src="%BASE_URL%quoted.js?rev=1#start"></script>
    <script SRC=%BASE_URL%unquoted.js?rev2 TYPE=text/javascript></script>
    <script src='/single-quoted.js'></script>
    <script src=https://example.test/external.js></script>
    <script src=//cdn.example.test/protocol-relative.js></script>
    <script type=module src=/module.js></script>
    <script>const source = '<script data-src="/inline-decoy.js">';</script>
  `;
  assert.deepEqual(localClassicScriptPaths(html), [
    'quoted.js',
    'unquoted.js',
    'single-quoted.js',
  ]);
});

test('classic runtime script parser fails closed for ambiguous or escaping paths', () => {
  const invalid = [
    '<script src="..\\outside.js"></script>',
    '<script src="https:\\example.test/outside.js"></script>',
    '<script src="../outside.js"></script>',
    '<script src="/safe/../../outside.js"></script>',
    '<script data-src="/decoy.js"src="/missing-separator.js"></script>',
    '<script src=/missing-close.js>',
  ];
  for (const html of invalid) {
    assert.throws(() => localClassicScriptPaths(html), { name: 'Error' });
  }
});

test('classic runtime script parser distinguishes inline scripts from invalid empty src', () => {
  assert.deepEqual(localClassicScriptPaths('<script>window.inline = true;</script>'), []);
  for (const html of [
    '<script src></script>',
    '<script src=""></script>',
    '<script src="   "></script>',
  ]) {
    assert.throws(() => localClassicScriptPaths(html), /src must be a non-empty string/u);
  }
});

test('classic runtime script parser ignores pseudo tags inside raw-text and RCDATA elements', () => {
  const html = `
    <!-- <script src="/comment.js"></script> -->
    <style>.example::before { content: '<script src="/style.js"></script>'; }</style>
    <textarea><script src="/textarea.js"></script></textarea>
    <title><script src="/title.js"></script></title>
    <xmp><script src="/xmp.js"></script></xmp>
    <iframe><script src="/iframe.js"></script></iframe>
    <noembed><script src="/noembed.js"></script></noembed>
    <noframes><script src="/noframes.js"></script></noframes>
    <noscript><script src="/noscript.js"></script></noscript>
    <custom-element data-example='<script src="/attribute.js"></script>'></custom-element>
    <script>const pseudo = '<script src="/inline.js"></script>';</script>
    <script src="%BASE_URL%real-runtime.js"></script>
  `;
  assert.deepEqual(localClassicScriptPaths(html), ['real-runtime.js']);
});

test('runtime script containment rejects paths outside source or output roots', () => {
  const root = path.join(appRoot, 'src');
  assert.equal(resolveContainedRuntimePath(root, 'shared/example.js'), path.join(root, 'shared', 'example.js'));
  assert.throws(() => resolveContainedRuntimePath(root, '../outside.js'), /escapes its root/u);
  assert.throws(() => resolveContainedRuntimePath(root, 'shared\\outside.js'), /invalid runtime script path/u);
});

test('Vite build contains every local classic runtime script referenced by index.html', {
  skip: fs.existsSync(distIndexPath) ? false : 'run npm run build:ui to verify build artifacts',
}, () => {
  const sourceIndex = fs.readFileSync(path.join(sourceRoot, 'index.html'), 'utf8');

  const expected = localClassicScriptPaths(sourceIndex).sort(); // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic string order is the assertion's expectation
  const distIndex = fs.readFileSync(distIndexPath, 'utf8');
  const built = localClassicScriptPaths(distIndex).sort(); // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic string order is the assertion's expectation
  assert.ok(expected.length > 0, 'index.html must retain classic runtime scripts');
  // The conditional platform transform strips the other platform's bridge tags
  // per mode, so the desktop index references a subset of the shared source
  // list rather than an exact copy. New references must still come from the
  // shared source manifest.
  assert.deepEqual(
    built.filter((relative) => !expected.includes(relative) && !relative.startsWith('startup/')),
    [],
    'built index introduced classic runtime references absent from source index',
  );
  assertStartupBundles({ built, expectedPlatform: 'desktop', outputRoot: distRoot, webBuild: false });
  for (const prefix of ['platform/web/']) {
    assert.ok(
      built.every((relative) => !relative.startsWith(prefix)),
      `desktop index must not reference ${prefix} scripts: ${built.filter((relative) => relative.startsWith(prefix)).join(', ')}`,
    );
  }
  assert.ok(
    distIndex.includes('window.PinvouPlatform = Object.freeze({ kind: "desktop", isWeb: false })'),
    'desktop index must inline the desktop platform marker replacing bootstrap.js',
  );

  // Since the startup-bundle rework, dist keeps verbatim copies only of
  // scripts not merged into dist/startup: tags that survived the transform
  // (legacy-polyfills, update-notice-logic), scripts pet.html references
  // directly (model-service-errors), and runtime-fetched assets
  // (access-policy.json — the desktop bridge fetches it at WebAccess boot).
  // Everything else — merged into a bundle or stripped for this platform —
  // must be absent from dist.
  const keptVerbatim = new Set(['shared/model-service-errors.js']);
  // access-policy.json is not a script tag, so it never enters `expected`;
  // pin it separately against both directions.
  const accessPolicyPath = resolveContainedRuntimePath(distRoot, 'platform/web/access-policy.json');
  const accessPolicySource = resolveContainedRuntimePath(sourceRoot, 'platform/web/access-policy.json');
  assert.ok(fs.existsSync(accessPolicyPath), 'dist must carry the runtime-fetched access policy (WebAccess boot)');
  assert.deepEqual(
    fs.readFileSync(accessPolicyPath),
    fs.readFileSync(accessPolicySource),
    'access-policy.json copy differs from source',
  );
  for (const relative of expected) {
    const sourcePath = resolveContainedRuntimePath(sourceRoot, relative);
    assert.ok(fs.statSync(sourcePath).isFile(), `missing runtime source: ${relative}`);
    const builtPath = resolveContainedRuntimePath(distRoot, relative);
    const tagSurvived = built.includes(relative);
    if (tagSurvived) {
      assert.ok(fs.existsSync(builtPath), `missing runtime build asset: ${relative}`);
      assert.deepEqual(
        fs.readFileSync(builtPath),
        fs.readFileSync(sourcePath),
        `runtime build asset differs from source: ${relative}`,
      );
      continue;
    }
    // No surviving tag: the script is served from dist/startup (or stripped
    // for the desktop platform). Only scripts another entry loads directly
    // keep a copy.
    if (keptVerbatim.has(relative)) {
      assert.ok(fs.existsSync(builtPath), `missing verbatim copy needed by another entry: ${relative}`);
      continue;
    }
    assert.ok(
      !fs.existsSync(builtPath),
      `dist still ships a dead verbatim copy of ${relative}: its code is served from dist/startup`,
    );
  }
});

test('web build index strips tauri-only bridge scripts', {
  skip: fs.existsSync(webDistIndexPath) ? false : 'run npm run build:web to verify web artifacts',
}, () => {
  const sourceIndex = fs.readFileSync(path.join(sourceRoot, 'index.html'), 'utf8');
  const expected = localClassicScriptPaths(sourceIndex);
  const webIndex = fs.readFileSync(webDistIndexPath, 'utf8');
  // build:web bakes the relay deployment base (vite.config.mjs
  // normalizeWebBasePath: PINVOU_REMOTE_PUBLIC_BASE_PATH, default
  // /pinvou3/remote) into every copied runtime URL; strip it so the built
  // index compares against source-relative paths.
  const webBase = String(process.env.PINVOU_REMOTE_PUBLIC_BASE_PATH || '/pinvou3/remote')
    .replace(/^https?:\/\/[^/]+/iu, '')
    .replace(/^\/+|\/+$/gu, '');
  const built = localClassicScriptPaths(webIndex)
    .map((relative) => (webBase && relative.startsWith(`${webBase}/`)
      ? relative.slice(webBase.length + 1)
      : relative));

  assert.deepEqual(
    built.filter((relative) => !expected.includes(relative) && !relative.startsWith('startup/')),
    [],
    'web index introduced classic runtime references absent from source index',
  );
  assert.ok(
    built.every((relative) => !relative.startsWith('platform/tauri/')),
    `web index must not reference platform/tauri/ scripts: ${built.filter((relative) => relative.startsWith('platform/tauri/')).join(', ')}`,
  );
  assertStartupBundles({
    built,
    expectedPlatform: 'web',
    outputRoot: path.dirname(webDistIndexPath),
    webBuild: true,
  });
  assert.ok(built.includes('platform/web/bootstrap.js'), 'web bootstrap must retain document.currentScript semantics');
  assert.equal(
    webIndex.includes('window.PinvouPlatform = Object.freeze({ kind: "desktop", isWeb: false })'),
    false,
    'web index must not inline the desktop platform marker',
  );
});
