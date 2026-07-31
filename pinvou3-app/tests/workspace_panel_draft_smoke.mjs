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

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-workspace-panel-smoke-'));
let browser;
let vite;

function assert(condition, message, detail) {
  if (!condition) throw new Error(`${message}${detail ? `: ${JSON.stringify(detail)}` : ''}`);
}

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
  const url = `http://127.0.0.1:${address.port}/tests/fixtures/workspace_panel_draft_smoke.html`;

  browser = await puppeteer.launch({
    executablePath: chrome,
    headless: 'new',
    userDataDir: profile,
    args: ['--no-sandbox', '--disable-gpu', '--no-first-run', '--no-default-browser-check'],
  });
  const page = await browser.newPage();
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  await page.setViewport({ width: 1200, height: 900, deviceScaleFactor: 1 });
  await page.goto(url, { waitUntil: 'domcontentloaded' });
  await page.waitForFunction(() => document.body.innerText.includes('main.py'), { timeout: 10000 });

  // 无会话（draft）模式：列目录只带 workspacePath，不带 sessionId。
  const listCalls = await page.evaluate(() => window.__invokeLog.filter((entry) => entry.command === 'list_codex_workspace'));
  assert(listCalls.length >= 1, '未调用 list_codex_workspace', listCalls);
  assert(
    listCalls.every((entry) => entry.args.workspacePath === 'D:/proj/demo' && entry.args.sessionId == null),
    'draft 模式列目录应只携带 workspacePath',
    listCalls,
  );

  // 面板头部显示工作区路径。
  const headerText = await page.evaluate(() => document.querySelector('aside')?.innerText || '');
  assert(headerText.includes('D:/proj/demo'), '面板头部未显示工作区路径', headerText);

  // 点击文件 → 预览同样只带 workspacePath，弹窗打开。
  await page.evaluate(() => {
    const row = [...document.querySelectorAll('button')].find((button) => button.title === 'main.py');
    row?.click();
  });
  await page.waitForSelector('[data-testid="code-viewer-modal"]', { timeout: 10000 });
  const previewCalls = await page.evaluate(() => window.__invokeLog.filter((entry) => entry.command === 'preview_codex_workspace_file'));
  assert(
    previewCalls.length === 1 && previewCalls[0].args.workspacePath === 'D:/proj/demo' && previewCalls[0].args.sessionId == null,
    'draft 模式预览应只携带 workspacePath',
    previewCalls,
  );
  await page.keyboard.press('Escape');
  await page.waitForFunction(() => !document.querySelector('[data-testid="code-viewer-modal"]'));

  // 文件行悬浮按钮 → 外部打开也只带 workspacePath。
  await page.evaluate(() => {
    const openButton = [...document.querySelectorAll('button')].find((button) => button.getAttribute('aria-label') === '用系统应用打开');
    openButton?.click();
  });
  const openCalls = await page.evaluate(() => window.__invokeLog.filter((entry) => entry.command === 'open_codex_workspace_file'));
  assert(
    openCalls.length === 1 && openCalls[0].args.workspacePath === 'D:/proj/demo' && openCalls[0].args.sessionId == null,
    'draft 模式外部打开应只携带 workspacePath',
    openCalls,
  );

  // 「更改」tab：无会话无基线，显示降级提示且不发起 changes 请求。
  await page.evaluate(() => {
    const tab = [...document.querySelectorAll('button')].find((button) => button.textContent.trim().startsWith('更改'));
    tab?.click();
  });
  await page.waitForFunction(() => document.body.innerText.includes('无法判断更改'), { timeout: 5000 });
  const changesCalls = await page.evaluate(() => window.__invokeLog.filter((entry) => entry.command === 'get_codex_workspace_changes'));
  assert(changesCalls.length === 0, 'draft 模式不应请求工作区变更', changesCalls);

  assert(pageErrors.length === 0, '浏览器运行时异常', pageErrors);

  console.log('workspace_panel_draft_smoke: ok');
} catch (error) {
  console.error('FAIL:', error?.stack || error);
  process.exitCode = 1;
} finally {
  if (browser) await browser.close();
  if (vite) await vite.close();
  fs.rmSync(profile, { recursive: true, force: true });
}
