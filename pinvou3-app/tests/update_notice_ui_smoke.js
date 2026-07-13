#!/usr/bin/env node
const fs = require('fs');
const path = require('path');
const os = require('os');
const { startUiTestServer } = require('./ui_test_server');

function loadPuppeteer() {
  try { return require('puppeteer-core'); } catch (_) {}
  const npx = path.join(os.homedir(), '.npm', '_npx');
  if (fs.existsSync(npx)) {
    for (const d of fs.readdirSync(npx)) {
      const p = path.join(npx, d, 'node_modules', 'puppeteer-core');
      if (fs.existsSync(p)) { try { return require(p); } catch (_) {} }
    }
  }
  console.error('SKIP: 找不到 puppeteer-core');
  process.exit(2);
}

const puppeteer = loadPuppeteer();
const CHROME = process.env.CHROME || [
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  '/usr/bin/chromium',
  '/usr/bin/google-chrome',
].find(p => fs.existsSync(p));

if (!CHROME) {
  console.error('SKIP: 未找到 Chrome/Edge');
  process.exit(2);
}

async function main() {
  const { url } = await startUiTestServer();
  const INDEX = url + '?mockUpdate=1';
  const browser = await puppeteer.launch({
    executablePath: CHROME,
    headless: 'new',
    args: ['--no-sandbox', '--disable-gpu'],
  });
  const page = await browser.newPage();
  page.on('pageerror', err => { throw err; });
  page.on('console', msg => {
    if (msg.type() === 'error') console.error('BROWSER:', msg.text());
  });

  await page.goto(INDEX, { waitUntil: 'networkidle0' });
  await page.waitForSelector('[data-update-notice-card="true"]', { timeout: 10000 });

  const cardText = await page.$eval('[data-update-notice-card="true"]', el => el.innerText);
  if (!cardText.includes('PINVOU v1.2.0')) throw new Error('未显示 mock 更新版本');
  if (!cardText.includes('升级并重启')) throw new Error('未显示升级按钮');

  await page.click('[data-update-notice-card="true"] button[title="关闭"]');
  await page.waitForFunction(() => !document.querySelector('[data-update-notice-card="true"]'), { timeout: 5000 });

  await page.goto(INDEX, { waitUntil: 'networkidle0' });
  await page.waitForSelector('[data-update-notice-card="true"]', { timeout: 10000 });
  await page.click('[data-update-notes-button="true"]');
  await page.waitForSelector('#settings-version-update', { timeout: 10000 });
  await page.waitForFunction(() => {
    const el = document.querySelector('#settings-version-update');
    if (!el) return false;
    const r = el.getBoundingClientRect();
    return r.top < window.innerHeight && r.bottom > 0 && document.body.innerText.includes('版本与更新');
  }, { timeout: 5000 });

  await browser.close();
  console.log('update_notice_ui_smoke: ok');
}

main().catch(err => {
  console.error('FAIL:', err && err.stack || err);
  process.exit(1);
});
