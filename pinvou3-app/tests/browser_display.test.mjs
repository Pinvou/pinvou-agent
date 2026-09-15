import assert from 'node:assert/strict';
import test from 'node:test';

import {
  browserAddressValue,
  browserTabLabel,
  isInternalBlankPageUrl,
  shouldShowNativeBrowserSurface,
} from '../src/features/browser/browser-display.mjs';

test('an initialized blank document is presented as a new tab instead of about:blank', () => {
  const label = 'New tab';
  for (const url of [
    'about:blank',
    'about:blank#pinvou-session-0123456789abcdef',
    'about:blank#pinvou-tab-fedcba9876543210',
  ]) {
    assert.equal(isInternalBlankPageUrl(url), true);
    assert.equal(browserAddressValue(url), '');
    assert.equal(browserTabLabel({ url, title: '' }, label), label);
  }
});

test('real pages retain their actual address and title', () => {
  assert.equal(isInternalBlankPageUrl('https://example.com'), false);
  assert.equal(browserAddressValue('https://example.com'), 'https://example.com');
  assert.equal(
    browserTabLabel({ url: 'https://example.com', title: 'Example' }, 'New tab'),
    'Example',
  );
});

test('the native surface appears only after the first status confirms a real page', () => {
  const realPage = {
    statusResolved: true,
    running: true,
    url: 'https://example.com',
    suspended: false,
  };
  assert.equal(shouldShowNativeBrowserSurface(realPage), true);
  assert.equal(shouldShowNativeBrowserSurface({ ...realPage, statusResolved: false }), false);
  assert.equal(shouldShowNativeBrowserSurface({ ...realPage, running: false }), false);
  assert.equal(shouldShowNativeBrowserSurface({ ...realPage, suspended: true }), false);
  assert.equal(shouldShowNativeBrowserSurface({ ...realPage, url: 'about:blank' }), false);
  assert.equal(
    shouldShowNativeBrowserSurface({
      ...realPage,
      url: 'about:blank#pinvou-session-0123456789abcdef',
    }),
    false,
  );
});
