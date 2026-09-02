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
    /evaluate\(\(\) => document\.querySelector\('\[data-testid="mobile-navigation-close"\]'\)\.click\(\)\)/u,
    'the sidebar covers the overlay center, so closing must use a DOM click instead of coordinates',
  );
  assert.doesNotMatch(webUiSmoke, /button\[aria-label="(?:打开|关闭)导航"\]/u);
});
