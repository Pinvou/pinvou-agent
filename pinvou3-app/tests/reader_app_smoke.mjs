#!/usr/bin/env node
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import react from '@vitejs/plugin-react';
import puppeteer from 'puppeteer-core';
import { createServer } from 'vite';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const chrome = process.env.CHROME || [
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  '/snap/bin/chromium',
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
  '/usr/bin/google-chrome',
  '/usr/bin/google-chrome-stable',
].find((candidate) => fs.existsSync(candidate));

if (!chrome) {
  console.error('SKIP: 未找到 Chrome/Edge，可通过 CHROME 指定');
  process.exit(2);
}

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-reader-app-smoke-'));
let browser;
let vite;

function assert(condition, message, detail) {
  if (!condition) throw new Error(`${message}${detail ? `: ${JSON.stringify(detail)}` : ''}`);
}

const tabNames = () => [...document.querySelectorAll('.group button[title]')]
  .filter((el) => el.title !== '关闭标签页')
  .map((el) => el.textContent.trim());

try {
  vite = await createServer({
    root: appRoot,
    configFile: false,
    appType: 'mpa',
    logLevel: 'error',
    plugins: [react()],
    server: { host: '127.0.0.1', port: 0, strictPort: false },
  });
  await vite.listen();
  const address = vite.httpServer.address();
  const url = `http://127.0.0.1:${address.port}/tests/fixtures/reader_app_smoke.html`;

  browser = await puppeteer.launch({
    executablePath: chrome,
    headless: 'new',
    userDataDir: profile,
    args: ['--no-sandbox', '--disable-gpu', '--no-first-run', '--no-default-browser-check'],
  });
  const page = await browser.newPage();
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  await page.setViewport({ width: 1000, height: 700, deviceScaleFactor: 1 });
  await page.goto(url, { waitUntil: 'domcontentloaded' });

  // 建窗前 pending 队列拉取：main.py tab 打开并加载预览。
  await page.waitForFunction(() => document.body.innerText.includes("print('main.py')"), { timeout: 10000 });
  const pendingCalls = await page.evaluate(() => window.__invokeLog.filter((entry) => entry.command === 'take_code_reader_pending'));
  assert(pendingCalls.length === 1, '启动时应拉取一次 pending 队列', pendingCalls);
  const previewCalls = await page.evaluate(() => window.__invokeLog.filter((entry) => entry.command === 'preview_codex_workspace_file'));
  assert(
    previewCalls.length === 1 && previewCalls[0].args.workspacePath === 'D:/proj/demo' && previewCalls[0].args.relativePath === 'main.py',
    'pending 文件预览参数错误',
    previewCalls,
  );

  // 事件推送第二个文件 → 两个 tab，新 tab 激活并加载。
  await page.evaluate(() => window.__readerOpenHandler({
    payload: { sessionId: null, workspacePath: 'D:/proj/demo', relativePath: 'src/app.js' },
  }));
  await page.waitForFunction(() => document.body.innerText.includes("print('src/app.js')"), { timeout: 5000 });
  let names = await page.evaluate(tabNames);
  assert(names.length === 2, '事件推送后应有两个 tab', names);

  // 重复推送已打开文件 → 去重（仍两个 tab），激活回 main.py。
  await page.evaluate(() => window.__readerOpenHandler({
    payload: { sessionId: null, workspacePath: 'D:/proj/demo', relativePath: 'main.py' },
  }));
  await page.waitForFunction(() => document.body.innerText.includes("print('main.py')"), { timeout: 5000 });
  names = await page.evaluate(tabNames);
  assert(names.length === 2, '重复打开应去重', names);

  // 关闭 app.js tab → 剩一个 tab。
  await page.evaluate(() => {
    const closeButtons = [...document.querySelectorAll('button[aria-label="关闭标签页"]')];
    closeButtons[1]?.click();
  });
  await page.waitForFunction(() => [...document.querySelectorAll('button[aria-label="关闭标签页"]')].length === 1, { timeout: 5000 });
  names = await page.evaluate(tabNames);
  assert(names.length === 1 && names[0] === 'main.py', '关闭 tab 后应只剩 main.py', names);

  assert(pageErrors.length === 0, '浏览器运行时异常', pageErrors);

  console.log('reader_app_smoke: ok');
} catch (error) {
  console.error('FAIL:', error?.stack || error);
  process.exitCode = 1;
} finally {
  if (browser) await browser.close();
  if (vite) await vite.close();
  fs.rmSync(profile, { recursive: true, force: true });
}
