import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  desktopPlatformMarkerScript,
  transformIndexHtmlForPlatform,
} from '../vite.config.mjs';
import { localClassicScriptPaths } from '../scripts/vite-runtime-assets.mjs';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const sourceIndex = fs.readFileSync(path.join(appRoot, 'src', 'index.html'), 'utf8');

const DESKTOP_MARKER = 'window.PinvouPlatform = Object.freeze({ kind: "desktop", isWeb: false })';

function classicScripts(html) {
  return localClassicScriptPaths(html);
}

test('desktop transform drops web-only bridge scripts and keeps everything else in order', () => {
  const source = classicScripts(sourceIndex);
  const desktop = classicScripts(transformIndexHtmlForPlatform(false, sourceIndex));

  assert.ok(
    desktop.every((relative) => !relative.startsWith('platform/web/')),
    `desktop index must not reference platform/web/ scripts: ${desktop.filter((relative) => relative.startsWith('platform/web/')).join(', ')}`,
  );
  assert.deepEqual(
    desktop.filter((relative) => relative.startsWith('platform/tauri/')),
    source.filter((relative) => relative.startsWith('platform/tauri/')),
    'desktop transform must keep every tauri bridge tag in source order',
  );
  assert.deepEqual(
    desktop.filter((relative) => !relative.startsWith('platform/')),
    source.filter((relative) => !relative.startsWith('platform/')),
    'desktop transform must keep shared/vendor tags in source order',
  );

  // Ordering pairs the web bridge contract test relies on, restricted to what
  // the desktop build still loads.
  const indexOf = (relative) => desktop.indexOf(relative);
  assert.ok(indexOf('shared/bridge-messages.js') < indexOf('platform/tauri/bridge/chat-events.js'));
  assert.ok(indexOf('shared/bridge-messages.js') < indexOf('platform/tauri/bridge.js'));
  assert.ok(indexOf('shared/legacy-polyfills.js') < indexOf('vendor/tailwind.js'));
  assert.ok(indexOf('platform/tauri/bridge.js') < desktop.length - 1);
});

test('desktop transform replaces bootstrap.js in place with the platform marker', () => {
  const html = transformIndexHtmlForPlatform(false, sourceIndex);
  const markerAt = html.indexOf(DESKTOP_MARKER);
  assert.notEqual(markerAt, -1, 'desktop marker must be present');

  const markerLine = html.split('\n').find((line) => line.includes(DESKTOP_MARKER));
  assert.ok(markerLine.trim().startsWith('<script>'), 'marker must be an inline classic script');
  assert.ok(
    !/\bsrc=/u.test(markerLine),
    'marker must not become a classic runtime script reference',
  );

  // The marker must sit exactly where bootstrap.js ran: after the contextmenu
  // guard and before authority-sync diagnostics (which read PinvouPlatform at
  // parse time) and the head timing script contract stays untouched.
  const contextMenuAt = html.indexOf("document.addEventListener('contextmenu'");
  const authoritySyncAt = html.indexOf('shared/authority-sync-diagnostics.js');
  assert.ok(contextMenuAt !== -1 && authoritySyncAt !== -1);
  assert.ok(contextMenuAt < markerAt && markerAt < authoritySyncAt);
  assert.ok(html.includes('window.__PINVOU_STARTUP__ = { mark: mark, markAt: markAt, flush: flush, entries: entries }'), 'head timing script preserved');
});

test('desktop transform only touches platform script lines', () => {
  const transformed = transformIndexHtmlForPlatform(false, sourceIndex);
  const sourceLines = sourceIndex.split('\n');
  const transformedLines = transformed.split('\n');
  assert.equal(sourceLines.length, transformedLines.length, 'line count must be preserved');

  let removed = 0;
  let markerSeen = 0;
  for (let i = 0; i < sourceLines.length; i += 1) {
    if (transformedLines[i] === sourceLines[i]) continue;
    const isWebScriptTag = /<script\b[^>]*%BASE_URL%platform\/web\//u.test(sourceLines[i]);
    assert.ok(isWebScriptTag, `only platform/web script lines may change, line ${i + 1}: ${sourceLines[i]}`);
    if (transformedLines[i] === '') removed += 1;
    if (transformedLines[i].includes(DESKTOP_MARKER)) markerSeen += 1;
  }
  assert.equal(markerSeen, 1, 'exactly one desktop marker replaces bootstrap.js');
  assert.equal(removed, 4, 'the four other web-only tags must be dropped');
});

test('web transform drops tauri-only bridge scripts and keeps the web transport in order', () => {
  const source = classicScripts(sourceIndex);
  const web = classicScripts(transformIndexHtmlForPlatform(true, sourceIndex));

  assert.ok(
    web.every((relative) => !relative.startsWith('platform/tauri/')),
    `web index must not reference platform/tauri/ scripts: ${web.filter((relative) => relative.startsWith('platform/tauri/')).join(', ')}`,
  );
  assert.deepEqual(
    web.filter((relative) => relative.startsWith('platform/web/')),
    source.filter((relative) => relative.startsWith('platform/web/')),
    'web transform must keep every web bridge tag in source order',
  );
  assert.deepEqual(
    web.filter((relative) => !relative.startsWith('platform/')),
    source.filter((relative) => !relative.startsWith('platform/')),
    'web transform must keep shared/vendor tags in source order',
  );

  const indexOf = (relative) => web.indexOf(relative);
  assert.ok(indexOf('shared/bridge-messages.js') < indexOf('platform/web/bridge.js'));
  assert.ok(indexOf('shared/chunked-file-upload.js') < indexOf('platform/web/bridge.js'));
  assert.ok(indexOf('platform/web/bridge/turn-terminal.js') < indexOf('platform/web/bridge.js'));
  assert.ok(indexOf('platform/web/bridge.js') < indexOf('platform/web/bridge/domain-adapter.js'));
  assert.ok(
    !transformIndexHtmlForPlatform(true, sourceIndex).includes(DESKTOP_MARKER),
    'web index must not inline the desktop platform marker',
  );
});

test('both transforms preserve the module entry and startup plumbing', () => {
  for (const webBuild of [false, true]) {
    const html = transformIndexHtmlForPlatform(webBuild, sourceIndex);
    assert.ok(
      html.includes('<script type="module" src="/app/main.jsx"'),
      `module entry untouched (webBuild=${webBuild})`,
    );
    assert.ok(
      html.includes('window.__PINVOU_STARTUP__ = { mark: mark, markAt: markAt, flush: flush, entries: entries }'),
      `head startup observers untouched (webBuild=${webBuild})`,
    );
    assert.ok(
      html.includes("window.TauriBridge.init()"),
      `bridge boot init untouched (webBuild=${webBuild})`,
    );
    assert.equal(
      html,
      transformIndexHtmlForPlatform(webBuild, html),
      `transform must be idempotent (webBuild=${webBuild})`,
    );
  }
});

test('transform handles a rewritten non-root base (web relay base path)', () => {
  // In the served/built html, `%BASE_URL%`/base rewriting has already replaced
  // the placeholder with the deployment base (e.g. /pinvou3/remote/).
  const based = sourceIndex.replaceAll('%BASE_URL%', '/pinvou3/remote/');
  // localClassicScriptPaths keeps the deployment base prefix, so classify by
  // the platform path segment instead of a leading `platform/`.
  const segmentClass = (html) => {
    const scripts = classicScripts(html).map((relative) => {
      const at = relative.indexOf('/platform/');
      return at >= 0 ? relative.slice(at + 1) : relative;
    });
    return {
      web: scripts.filter((relative) => relative.startsWith('platform/web/')),
      tauri: scripts.filter((relative) => relative.startsWith('platform/tauri/')),
      all: scripts,
    };
  };
  const web = segmentClass(transformIndexHtmlForPlatform(true, based));
  const desktop = segmentClass(transformIndexHtmlForPlatform(false, based));

  assert.deepEqual(web.tauri, [], 'web transform must classify tauri scripts under a non-root base');
  assert.ok(web.all.includes('platform/web/bootstrap.js') && web.all.includes('platform/web/bridge.js'),
    'web transform must keep the web transport under a non-root base');
  assert.deepEqual(desktop.web, [], 'desktop transform must classify web scripts under a non-root base');
  assert.equal(desktop.tauri.length, 19, 'desktop transform must keep all 19 tauri bridge tags under a non-root base');
  assert.ok(transformIndexHtmlForPlatform(false, based).includes(DESKTOP_MARKER),
    'desktop marker must also replace bootstrap.js under a non-root base');
});

test('desktop and web transforms together retain the full shared source manifest', () => {
  const source = classicScripts(sourceIndex);
  const desktop = classicScripts(transformIndexHtmlForPlatform(false, sourceIndex));
  const web = classicScripts(transformIndexHtmlForPlatform(true, sourceIndex));
  const union = [...new Set([...desktop, ...web])].sort(); // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic string order is the assertion's expectation
  assert.deepEqual(union, [...new Set(source)].sort()); // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic string order is the assertion's expectation
});

test('marker export stays byte-stable for the dist contract test', () => {
  assert.equal(desktopPlatformMarkerScript.includes(DESKTOP_MARKER), true);
  assert.equal(desktopPlatformMarkerScript.startsWith('<script>'), true);
  assert.equal(desktopPlatformMarkerScript.endsWith('</script>'), true);
});
