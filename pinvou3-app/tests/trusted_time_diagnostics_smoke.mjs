#!/usr/bin/env node
// Feature-only real React/DOM regression, following question_choice_card_smoke.
// Adapters are synthetic DATA; this is not an installed SCM/control-plane E2E.
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
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  '/snap/bin/chromium', '/usr/bin/chromium', '/usr/bin/chromium-browser',
  '/usr/bin/google-chrome', '/usr/bin/google-chrome-stable',
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
].find(candidate => fs.existsSync(candidate));
if (!chrome) {
  console.error('NOT RUN: Chrome/Edge unavailable; set CHROME to its executable path.');
  process.exit(2);
}

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-time-diagnostics-smoke-'));
let browser;
let launchStarted = false;
let vite;
const pageErrors = [];
const passed = [];
const status = (state, refreshing = false) => ({ protocolVersion: 1, state, refreshing });
let page;
let url;
const act = (method, ...args) => page.evaluate((name, values) => window.__timeDiagnostics[name](...values), method, args);
const snapshot = () => act('snapshot');
const calls = () => act('calls');
const settle = (name, kind, index, value, rejected = false) => act('settle', name, kind, index, value, rejected);
const render = patch => act('render', patch);
async function waitCalls(name, kind, expected) {
  await page.waitForFunction((source, type, count) => (
    window.__timeDiagnostics.calls().filter(call => call.name === source && call.kind === type).length === count
  ), { timeout: 10000 }, name, kind, expected);
}
async function idle() {
  await page.waitForFunction(() => document.querySelector('section')?.getAttribute('aria-busy') === 'false', { timeout: 10000 });
}
async function paints() {
  await page.evaluate(() => new Promise(resolve => { requestAnimationFrame(() => requestAnimationFrame(() => resolve())); }));
}
async function click(index) {
  const buttons = await page.$$('section button');
  assert.equal(buttons.length, 2);
  assert.equal(await buttons[index].evaluate(button => button.disabled), false);
  await buttons[index].click();
}
async function reset(locale = 'en') {
  await page.goto(url, { waitUntil: 'domcontentloaded' });
  await page.waitForFunction(() => !!window.__timeDiagnostics, { timeout: 10000 });
  await render({ mounted: true, locale, adapter: 'A', supported: true });
  await waitCalls('A', 'read', 1);
  return page.evaluate(language => window.__timeDiagnostics.copies[language], locale);
}
async function readState(index, state, refreshing = false) {
  await click(0);
  await waitCalls('A', 'read', index + 1);
  await settle('A', 'read', index, status(state, refreshing));
  await idle();
}

try {
  // Same dependencies and MPA fixture approach as the reviewed existing smokes.
  // The Vite cache belongs to this run, not the shared source/node_modules tree.
  vite = await createServer({
    root: appRoot, configFile: false, appType: 'mpa', logLevel: 'error',
    cacheDir: path.join(profile, 'vite-cache'), plugins: [react()],
    server: { host: '127.0.0.1', port: 0, strictPort: false, watch: null },
  });
  await vite.listen();
  const address = vite.httpServer.address();
  assert(address && typeof address !== 'string');
  url = `http://127.0.0.1:${address.port}/tests/fixtures/trusted_time_diagnostics_smoke.html`;
  launchStarted = true;
  browser = await puppeteer.launch({
    executablePath: chrome, headless: 'new', userDataDir: profile,
    args: ['--no-sandbox', '--disable-gpu', '--no-first-run', '--no-default-browser-check'],
  });
  page = await browser.newPage();
  page.on('pageerror', error => pageErrors.push(error.message));
  await page.setViewport({ width: 1000, height: 800, deviceScaleFactor: 1 });

  for (const locale of ['zh', 'en', 'ja']) {
    const copy = await reset(locale);
    const states = ['unknown', 'unsupported', 'setup_required', 'missing_policy',
      'source_unavailable', 'cleanup_pending', 'cleanup_complete', 'integrity_fault'];
    for (let index = 0; index < states.length; index += 1) {
      if (index > 0) {
        await click(0);
        await waitCalls('A', 'read', index + 1);
      }
      await settle('A', 'read', index, status(states[index]));
      await idle();
      const view = await snapshot();
      assert.equal(view.status, copy.states[states[index]], `${locale}/${states[index]}`);
      assert(view.text.includes(copy.title));
      assert(view.text.includes(copy.networkNotice));
      assert.deepEqual(view.buttons.map(button => button.text), [copy.readStatus, copy.refresh]);
      assert.equal(view.buttons[1].disabled, !['unknown', 'source_unavailable', 'cleanup_pending', 'cleanup_complete', 'integrity_fault'].includes(states[index]));
      assert.equal((await calls()).filter(call => call.kind === 'refresh').length, 0, 'Local reads must never request refresh');
    }
    let nextRead = states.length;
    await readState(nextRead, 'unknown');
    nextRead += 1;
    await click(1);
    await waitCalls('A', 'refresh', 1);
    await settle('A', 'refresh', 0, 'HOST_BUSY', true);
    await idle();
    assert.equal((await snapshot()).alert, copy.errors.busy);
    assert(!(await snapshot()).status.includes(copy.states.integrity_fault));
    // Host-running state is distinct from the view's own pending read.
    await readState(nextRead, 'source_unavailable', true);
    nextRead += 1;
    assert((await snapshot()).status.includes(copy.refreshing));
    assert.equal((await snapshot()).buttons[1].disabled, true);
    await readState(nextRead, 'cleanup_complete', true);
    nextRead += 1;
    assert((await snapshot()).status.includes(copy.states.cleanup_complete));
    assert((await snapshot()).status.includes(copy.refreshing));
    assert.equal((await snapshot()).buttons[1].disabled, true);
    await readState(nextRead, 'unknown');
    nextRead += 1;
    await click(1);
    await waitCalls('A', 'refresh', 2);
    await settle('A', 'refresh', 1, 'fixture-private-detail-do-not-render', true);
    await idle();
    assert.equal((await snapshot()).alert, copy.errors.unavailable);
    assert(!(await snapshot()).text.includes('fixture-private-detail-do-not-render'));
    assert.equal((await calls()).filter(call => call.kind === 'read').length, nextRead, 'Rejected refresh must not synthesize a new read');
    passed.push(`localized real DOM: ${locale}, eight states, local reads, Busy and failure`);
  }

  const copy = await reset();
  await settle('A', 'read', 0, status('unknown'));
  await idle();
  // Actual DOM double-click; disabled/pending handling must yield one request.
  const refresh = await page.$('section button:last-child');
  await refresh.click({ count: 2 });
  await waitCalls('A', 'refresh', 1);
  assert((await snapshot()).buttons.every(button => button.disabled));
  await settle('A', 'refresh', 0, { protocolVersion: 1, state: 'accepted' });
  await waitCalls('A', 'read', 2);
  // The invoke starts before React necessarily commits the accepted view.
  await page.waitForFunction(text => document.querySelector('[role="status"]')?.textContent.includes(text), { timeout: 10000 }, copy.accepted);
  const awaitingRead = await snapshot();
  assert(awaitingRead.status.includes(copy.accepted));
  assert.equal(awaitingRead.busy, 'true');
  assert(!awaitingRead.status.includes(copy.states.cleanup_pending), 'ACK cannot create a status');
  await settle('A', 'read', 1, status('cleanup_pending'));
  await idle();
  assert((await snapshot()).status.includes(copy.states.cleanup_pending));
  assert((await snapshot()).status.includes(copy.accepted));
  assert.equal((await calls()).filter(call => call.kind === 'refresh').length, 1);
  // Invalid accepted DATA cannot masquerade as status or cause a reread.
  await click(1);
  await waitCalls('A', 'refresh', 2);
  await settle('A', 'refresh', 1, { protocolVersion: 1, state: 'known' });
  await idle();
  assert.equal((await snapshot()).alert, copy.errors.invalidResponse);
  assert(!(await snapshot()).status.includes(copy.accepted));
  assert.equal((await calls()).filter(call => call.kind === 'read').length, 2);
  passed.push('explicit double-click yields one ACK then a real status read; invalid ACK refuses');

  await reset();
  await settle('A', 'read', 0, status('unknown'));
  await idle();
  await click(1);
  await waitCalls('A', 'refresh', 1);
  await settle('A', 'refresh', 0, { protocolVersion: 1, state: 'accepted' });
  await waitCalls('A', 'read', 2);
  await settle('A', 'read', 1, status('cleanup_pending'));
  await idle();
  assert((await snapshot()).status.includes(copy.accepted));
  await act('clearFrames');
  const bImmediate = await render({ adapter: 'B' });
  assert(!bImmediate.status.includes(copy.accepted));
  assert(!bImmediate.status.includes(copy.states.cleanup_pending));
  await waitCalls('B', 'read', 1);
  const aImmediate = await render({ adapter: 'A' });
  assert(!aImmediate.status.includes(copy.accepted), 'A→B→A must not restore historical accepted');
  assert(!aImmediate.status.includes(copy.states.cleanup_pending), 'A→B→A must not restore historical status');
  await waitCalls('A', 'read', 3);
  await settle('B', 'read', 0, status('integrity_fault'));
  await paints();
  assert.equal((await snapshot()).busy, 'true', 'Late B response cannot end A read');
  await settle('A', 'read', 2, status('source_unavailable'));
  await idle();
  await paints();
  for (const frame of await act('frames')) {
    assert(!frame.status.includes(copy.accepted), 'Old accepted flashed in a real DOM commit');
    assert(!frame.status.includes(copy.states.cleanup_pending), 'Old source status flashed in a real DOM commit');
    assert(!frame.status.includes(copy.states.integrity_fault), 'Late B published under A');
  }
  assert.equal((await snapshot()).status, copy.states.source_unavailable);
  // Keep an explicit refresh pending across an actual unmount/new source.
  await click(1);
  await waitCalls('A', 'refresh', 2);
  await render({ mounted: false });
  await render({ mounted: true, adapter: 'B' });
  await waitCalls('B', 'read', 2);
  await settle('B', 'read', 1, status('unknown'));
  await idle();
  const afterUnmount = await snapshot();
  await settle('A', 'refresh', 1, { protocolVersion: 1, state: 'accepted' });
  await paints();
  assert.deepEqual(await snapshot(), afterUnmount, 'Unmounted refresh cannot publish accepted/status');
  assert.equal((await calls()).filter(call => call.name === 'A' && call.kind === 'read').length, 3, 'Unmounted ACK must not start another source read');
  // A disposed local read cannot publish either.
  await click(0);
  await waitCalls('B', 'read', 3);
  await render({ mounted: false });
  await settle('B', 'read', 2, status('integrity_fault'));
  await paints();
  assert.equal((await snapshot()).text, '');
  passed.push('same-instance A→B→A commits and late unmounted ACK/read preserve source ownership');

  const count = (await calls()).length;
  await render({ mounted: true, supported: false });
  await paints();
  const unavailable = await snapshot();
  assert.equal(unavailable.status, copy.states.unsupported);
  assert(unavailable.buttons.every(button => button.disabled));
  await paints();
  assert.equal((await calls()).length, count, 'Unsupported mount must not invoke either adapter method');
  assert.deepEqual(pageErrors, [], 'Unexpected real browser exception');
  passed.push('unsupported component remains local and disabled');
  assert.equal(passed.length, 6);
  for (const name of passed) console.log(`PASS: ${name}`);
  console.log('trusted_time_diagnostics_smoke: 6 groups passed; React/DOM synthetic adapters only');
} catch (error) {
  console.error('FAIL:', error?.stack || error);
  process.exitCode = 1;
} finally {
  // Never remove a browser profile while process/handle ownership is uncertain.
  let closed = !launchStarted || !!browser;
  try { if (browser) await browser.close(); } catch { closed = false; process.exitCode = 1; }
  try { if (vite) await vite.close(); } catch { closed = false; process.exitCode = 1; }
  if (closed) fs.rmSync(profile, { recursive: true, force: true });
  else console.error(`Cleanup incomplete; retained owned profile: ${profile}`);
}
