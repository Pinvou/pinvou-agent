import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import react from '@vitejs/plugin-react';
import * as puppeteer from 'puppeteer-core';
import { createServer } from 'vite';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const chrome = process.env.CHROME || [
  '/snap/bin/chromium',
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
  '/usr/bin/google-chrome',
  '/usr/bin/google-chrome-stable',
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
].find(candidate => fs.existsSync(candidate));

if (!chrome) {
  console.error('SKIP: chromium/chrome not found; set env CHROME=/path/to/chromium');
  process.exit(2);
}

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-timeline-virtualization-'));
let browser;
let server;

try {
  server = await createServer({
    root: appRoot,
    configFile: false,
    appType: 'mpa',
    logLevel: 'error',
    plugins: [react()],
    server: { host: '127.0.0.1', port: 0, strictPort: false, watch: null },
  });
  await server.listen();
  const address = server.httpServer.address();
  browser = await puppeteer.launch({
    executablePath: chrome,
    headless: 'new',
    userDataDir: profile,
    args: ['--no-sandbox', '--disable-gpu', '--no-first-run', '--no-default-browser-check'],
  });
  const page = await browser.newPage();
  await page.goto(
    `http://127.0.0.1:${address.port}/tests/fixtures/conversation_timeline_performance.html`,
    { waitUntil: 'networkidle0', timeout: 30_000 },
  );
  await page.waitForFunction(() => Boolean(window.__PINVOU_TIMELINE_PERFORMANCE__), { timeout: 10_000 });

  const initialLongMount = await page.evaluate(
    () => window.__PINVOU_TIMELINE_PERFORMANCE__.runInitialLongMount(),
  );
  assert.equal(initialLongMount.virtualized, true, 'an initially long timeline must virtualize before its scroll ref is populated');
  assert.ok(initialLongMount.nodes <= 2_500, `initial long timeline mounted too many nodes: ${initialLongMount.nodes}`);

  const shortTimeline = await page.evaluate(() => window.__PINVOU_TIMELINE_PERFORMANCE__.run(50));
  assert.equal(shortTimeline.virtualRows, 0, 'short conversations must keep normal document flow');
  assert.ok(shortTimeline.topIndexes.includes(0) && shortTimeline.bottomIndexes.includes(49));
  assert.equal(shortTimeline.shortContentVisibility, 'auto', 'short conversations keep their existing content-visibility optimization');

  const longTimeline = await page.evaluate(() => window.__PINVOU_TIMELINE_PERFORMANCE__.run(1000));
  assert.ok(longTimeline.nodes <= 2_500, `resident DOM exceeded budget: ${longTimeline.nodes}`);
  assert.ok(longTimeline.maxResidentNodes <= 2_500, `scrolling DOM exceeded budget: ${longTimeline.maxResidentNodes}`);
  assert.ok(longTimeline.maxResidentVirtualRows <= 40, `too many virtual rows stayed mounted: ${longTimeline.maxResidentVirtualRows}`);
  assert.equal(longTimeline.tailUpdated, true, 'the mounted final turn must receive updates');
  assert.ok(longTimeline.topIndexes.includes(0), 'scrolling to the top must mount the first turn');
  assert.ok(longTimeline.middleIndexes.some(index => index > 200 && index < 800), 'middle scrolling must mount middle turns');
  assert.ok(longTimeline.bottomIndexes.includes(999), 'scrolling to the bottom must mount the final turn');
  assert.ok(Math.abs(longTimeline.bottomDistanceAfterMount) <= 1, `initial bottom drifted by ${longTimeline.bottomDistanceAfterMount}px`);
  assert.ok(Math.abs(longTimeline.measuredGap - 28) <= 1, `virtual row gap was ${longTimeline.measuredGap}px instead of 28px`);
  assert.ok(Math.abs(longTimeline.scrollMargin - 96) <= 1, `scroll margin was ${longTimeline.scrollMargin}px instead of 96px`);
  assert.ok(
    longTimeline.virtualContentVisibility.every(value => value === 'visible'),
    `virtual rows must use measured layout instead of content-visibility placeholders: ${longTimeline.virtualContentVisibility.join(', ')}`,
  );
  assert.ok(Math.abs(longTimeline.topTransformOffset) <= 1, `first row transform missed the header offset by ${longTimeline.topTransformOffset}px`);
  assert.ok(Math.abs(longTimeline.topScrollTop) <= 1, `top scroll position drifted by ${longTimeline.topScrollTop}px`);

  const completionMigration = await page.evaluate(
    () => window.__PINVOU_TIMELINE_PERFORMANCE__.runCompletionMigration(),
  );
  assert.equal(completionMigration.sameElement, true, 'live tail DOM identity must survive completion');
  assert.equal(completionMigration.statePreserved, true, 'live tail local UI state must survive completion');
  assert.equal(completionMigration.mountPreserved, true, 'live tail React subtree must not remount');
  assert.ok(Math.abs(completionMigration.immediateBottomDistance) <= 1, `live tail completion drifted immediately by ${completionMigration.immediateBottomDistance}px`);
  assert.ok(Math.abs(completionMigration.bottomDistance) <= 1, `completed live tail drifted by ${completionMigration.bottomDistance}px`);

  console.log('conversation timeline virtualization smoke passed', JSON.stringify({ longTimeline, completionMigration }));
} finally {
  await browser?.close().catch(() => {});
  await server?.close().catch(() => {});
  fs.rmSync(profile, { recursive: true, force: true });
}
