#!/usr/bin/env node
// StockQuoteCard unit-suffix fixture (review follow-up):
// market fields from the iWenCai-shaped payload may carry unit suffixes
// ('+1.25%', '1688.00万'); the card must keep the permissive Number.parseFloat
// (Number() would yield NaN and render '--'), while a missing field must stay
// undefined so the fmt fallback renders '--' instead of crashing on toFixed.
// Loads the real JSX module through a bare Vite SSR server (same approach the
// repo uses for browser smoke fixtures, minus puppeteer) because node:test
// cannot import .jsx natively.
import assert from 'node:assert/strict';
import { after, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';

const hadWindow = Object.prototype.hasOwnProperty.call(globalThis, 'window');
// useBridge.js reads window.TauriBridge at module load; the undefined stub hits
// its { available: false } fallback, which is all the card render path needs.
globalThis.window = globalThis.window || { TauriBridge: undefined };

const vite = await createServer({
  configFile: false,
  // URL.pathname is not a native path on Windows ('/D:/...') and stays
  // percent-encoded on POSIX; fileURLToPath is the portable form.
  root: fileURLToPath(new URL('..', import.meta.url)),
  logLevel: 'error',
  // One-shot SSR module load: file watching is never used and only ENOSPC-flakes
  // server startup on hosts with low inotify limits.
  server: { middlewareMode: true, watch: null },
  optimizeDeps: { noDiscovery: true },
});
const { StockQuoteCard } = await vite.ssrLoadModule('/src/features/tools/tool-common.jsx');

after(async () => {
  await vite.close();
  if (!hadWindow) delete globalThis.window;
});

const t = { stockOpen: 'Open', stockHigh: 'High', stockLow: 'Low' };

test('unit-suffixed market values parse leniently instead of rendering --', () => {
  const html = renderToStaticMarkup(React.createElement(StockQuoteCard, {
    data: {
      name: '贵州茅台', code: '600519',
      price: '1701.50', changePercent: '+1.25%', open: '1688.00万', high: '1712.00亿', low: '1680.00',
    },
    t,
  }));
  assert.ok(html.includes('1701.50'), 'plain price must render numerically');
  assert.ok(html.includes('1.25'), 'unit-suffixed changePercent must parse, not render --');
  assert.ok(html.includes('1688.00'), 'unit-suffixed open must parse, not render --');
  assert.ok(html.includes('1712.00'), 'unit-suffixed high must parse, not render --');
  assert.ok(!html.includes('NaN'), 'no NaN may leak into the card');
});

test('missing fields fall back to -- without toFixed crashes', () => {
  const html = renderToStaticMarkup(React.createElement(StockQuoteCard, {
    data: { name: '贵州茅台', code: '600519', price: '1701.50' },
    t,
  }));
  assert.ok(html.includes('1701.50'), 'present price renders');
  assert.ok((html.match(/--/g) || []).length >= 4, 'missing changePercent/open/high/low render --');
  assert.ok(!html.includes('NaN'), 'no NaN may leak into the card');
});
