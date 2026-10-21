import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import { languageFromLocaleTags } from '../src/shared/i18n.js';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

test('system locale maps to a supported initial language', () => {
  assert.equal(languageFromLocaleTags(['zh-CN']), 'zh');
  assert.equal(languageFromLocaleTags(['zh-Hant-TW']), 'zh');
  assert.equal(languageFromLocaleTags(['ja-JP']), 'ja');
  assert.equal(languageFromLocaleTags(['en-US']), 'en');
  assert.equal(languageFromLocaleTags(['fr-FR']), 'en');
});

test('missing system locale falls back to English', () => {
  assert.equal(languageFromLocaleTags([]), 'en');
  assert.equal(languageFromLocaleTags([undefined, '']), 'en');
});

test('mobile web smoke uses locale-independent navigation selectors', () => {
  const mobileShell = fs.readFileSync(
    path.join(appRoot, 'src', 'components', 'layout', 'MobileShell.jsx'),
    'utf8',
  );
  const main = fs.readFileSync(path.join(appRoot, 'src', 'app', 'main.jsx'), 'utf8');
  const webUiSmoke = fs.readFileSync(
    path.join(appRoot, '..', 'remote-control-relay', 'test', 'web-ui.smoke.cjs'),
    'utf8',
  );

  assert.match(mobileShell, /data-testid="mobile-navigation-open"/u);
  assert.match(main, /data-testid="mobile-navigation-close"/u);
  assert.match(webUiSmoke, /click\('\[data-testid="mobile-navigation-open"\]'\)/u);
  assert.match(
    webUiSmoke,
    /mobilePage\.mouse\.click\(370, 400\)/u,
    'closing must go through a real pointer click landed 90px right of the 280px drawer: '
      + 'trusted events focus the scrim button, which is what the scrim-focus record pins '
      + '(a synthetic DOM click() never moves focus, so it cannot catch that regression)',
  );
  assert.doesNotMatch(webUiSmoke, /button\[aria-label="(?:打开|关闭)导航"\]/u);
});

test('mobile web smoke keeps animation records independent of the host motion preference', () => {
  const webUiSmoke = fs.readFileSync(
    path.join(appRoot, '..', 'remote-control-relay', 'test', 'web-ui.smoke.cjs'),
    'utf8',
  );

  // The animation records require real transition properties, and the app is
  // allowed to drop them under prefers-reduced-motion: reduce. Every page must
  // therefore opt into no-preference explicitly, or the suite goes red on a
  // host whose OS asks for reduced motion even though the app honors its
  // accessibility contract.
  assert.match(
    webUiSmoke,
    /\{ name: 'prefers-reduced-motion', value: 'no-preference' \}/u,
    'pages must emulate prefers-reduced-motion: no-preference before the animation records run',
  );
  assert.match(webUiSmoke, /pinNoMotionPreference\(desktopPage\)/u);
  assert.match(webUiSmoke, /pinNoMotionPreference\(mobilePage\)/u);
  // The reduced-motion degenerate path must stay covered on its own: transition
  // properties become none while open/close keep working (app keeps
  // motion-reduce:transition-none).
  assert.match(
    webUiSmoke,
    /\{ name: 'prefers-reduced-motion', value: 'reduce' \}/u,
    'the reduce contract needs its own journey (transition-property none, drawer still usable)',
  );
});
