import assert from 'node:assert/strict';
import test from 'node:test';

import {
  browserAddressValue,
  browserTabLabel,
  isInternalBlankPageUrl,
  shouldShowNativeBrowserSurface,
} from '../src/features/browser/browser-display.mjs';

test('初始化空文档在产品界面显示为新标签页而不是 about:blank', () => {
  const label = '新标签页';
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

test('真实网页继续显示真实地址和标题', () => {
  assert.equal(isInternalBlankPageUrl('https://example.com'), false);
  assert.equal(browserAddressValue('https://example.com'), 'https://example.com');
  assert.equal(
    browserTabLabel({ url: 'https://example.com', title: 'Example' }, '新标签页'),
    'Example',
  );
});

test('原生页面只在首个状态确认真实网页后显示', () => {
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
