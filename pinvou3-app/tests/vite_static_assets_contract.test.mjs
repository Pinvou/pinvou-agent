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
    built.filter((relative) => !expected.includes(relative)),
    [],
    'built index introduced classic runtime references absent from source index',
  );
  assert.ok(
    built.some((relative) => relative.startsWith('platform/tauri/')),
    'desktop index must keep the tauri bridge scripts',
  );
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

  for (const relative of expected) {
    const sourcePath = resolveContainedRuntimePath(sourceRoot, relative);
    const builtPath = resolveContainedRuntimePath(distRoot, relative);
    assert.ok(fs.statSync(sourcePath).isFile(), `missing runtime source: ${relative}`);
    assert.ok(fs.existsSync(builtPath), `missing runtime build asset: ${relative}`);
    assert.deepEqual(
      fs.readFileSync(builtPath),
      fs.readFileSync(sourcePath),
      `runtime build asset differs from source: ${relative}`,
    );
  }
});

test('web build index strips tauri-only bridge scripts', {
  skip: fs.existsSync(webDistIndexPath) ? false : 'run npm run build:web to verify web artifacts',
}, () => {
  const sourceIndex = fs.readFileSync(path.join(sourceRoot, 'index.html'), 'utf8');
  const expected = localClassicScriptPaths(sourceIndex);
  const webIndex = fs.readFileSync(webDistIndexPath, 'utf8');
  const built = localClassicScriptPaths(webIndex);

  assert.deepEqual(
    built.filter((relative) => !expected.includes(relative)),
    [],
    'web index introduced classic runtime references absent from source index',
  );
  assert.ok(
    built.every((relative) => !relative.startsWith('platform/tauri/')),
    `web index must not reference platform/tauri/ scripts: ${built.filter((relative) => relative.startsWith('platform/tauri/')).join(', ')}`,
  );
  for (const relative of ['platform/web/bootstrap.js', 'platform/web/bridge.js', 'shared/bridge-messages.js']) {
    assert.ok(built.includes(relative), `web index must keep ${relative}`);
  }
  assert.equal(
    webIndex.includes('window.PinvouPlatform = Object.freeze({ kind: "desktop", isWeb: false })'),
    false,
    'web index must not inline the desktop platform marker',
  );
});
