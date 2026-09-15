#!/usr/bin/env node
// 会话模式端到端冒烟：更改列表渲染、行悬浮「添加到对话」、点击行开 diff 弹窗、
// 弹窗「在新窗口打开」携带 kind='diff' 且成功后关弹窗、deleted 行同样可开、
// 文件模式 open_code_reader 无 kind 键（回归）。
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

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-workspace-session-smoke-'));
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
    // One-shot smoke page: file watching is never used and only ENOSPC-flakes
    // server startup on hosts with low inotify limits.
    server: { host: '127.0.0.1', port: 0, strictPort: false, watch: null },
  });
  await vite.listen();
  const address = vite.httpServer.address();
  const url = `http://127.0.0.1:${address.port}/tests/fixtures/workspace_panel_session_smoke.html`;

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

  // 切到「更改」tab（徽标数量 2）。
  await page.waitForFunction(() => {
    const tab = [...document.querySelectorAll('button')].find((el) => el.textContent.includes('更改'));
    return tab?.textContent.includes('2');
  }, { timeout: 10000 });
  await page.evaluate(() => {
    [...document.querySelectorAll('button')].find((el) => el.textContent.includes('更改'))?.click();
  });
  await page.waitForFunction(() => [...document.querySelectorAll('button[title]')].some((el) => el.title === 'src/main.py'), { timeout: 5000 });

  // 两行渲染；行悬浮「添加到对话」→ 引用回调。
  const addButtonLabel = await page.evaluate(() => [...document.querySelectorAll('button[aria-label]')].map((el) => el.getAttribute('aria-label')));
  assert(addButtonLabel.includes('添加 src/main.py 到对话') && addButtonLabel.includes('添加 README.md 到对话'), '更改行悬浮添加按钮缺失', addButtonLabel);
  await page.evaluate(() => [...document.querySelectorAll('button[aria-label="添加 src/main.py 到对话"]')][0]?.click());
  const referenceLog = await page.evaluate(() => window.__referenceLog);
  assert(referenceLog.length === 1 && referenceLog[0] === 'src/main.py', '添加引用回调参数错误', referenceLog);

  // 点击 main.py 行主按钮 → diff 弹窗打开（diff 高亮、Diff 徽标、截断横幅）。
  await page.evaluate(() => [...document.querySelectorAll('button[title="src/main.py"]')][0]?.click());
  await page.waitForSelector('[data-testid="code-viewer-modal"]', { timeout: 5000 });
  const diffCalls = await page.evaluate(() => window.__invokeLog.filter((entry) => entry.command === 'get_codex_workspace_diff'));
  assert(
    diffCalls.length === 1 && diffCalls[0].args.sessionId === 's-1' && diffCalls[0].args.relativePath === 'src/main.py',
    'diff 拉取参数错误',
    diffCalls,
  );
  const dialogState = await page.evaluate(() => {
    const modal = document.querySelector('[data-testid="code-viewer-modal"]');
    const body = modal.querySelector('[data-testid="code-viewer-body"]');
    return {
      hasDiffCode: !!body.querySelector('code.hljs.language-diff'),
      badge: modal.innerText.includes('Diff'),
      truncated: body.innerText.includes('内容过大'),
    };
  });
  assert(dialogState.hasDiffCode && dialogState.badge && dialogState.truncated, 'diff 弹窗内容不完整', dialogState);

  // 「在新窗口打开」→ open_code_reader 带 kind='diff'，成功后弹窗关闭。
  await page.click('[data-testid="code-viewer-open-in-new-window"]');
  await page.waitForFunction(() => !document.querySelector('[data-testid="code-viewer-modal"]'), { timeout: 5000 });
  const readerCalls = await page.evaluate(() => window.__invokeLog.filter((entry) => entry.command === 'open_code_reader'));
  assert(
    readerCalls.length === 1
      && readerCalls[0].args.sessionId === 's-1'
      && readerCalls[0].args.relativePath === 'src/main.py'
      && readerCalls[0].args.kind === 'diff',
    'diff 模式 open_code_reader 参数错误（应含 kind=diff）',
    readerCalls,
  );

  // deleted 行（README.md）同样打开 diff 弹窗（不依赖文件存在）。
  await page.evaluate(() => [...document.querySelectorAll('button[title="README.md"]')][0]?.click());
  await page.waitForSelector('[data-testid="code-viewer-modal"]', { timeout: 5000 });
  const diffCallsAfterDeleted = await page.evaluate(() => window.__invokeLog.filter((entry) => entry.command === 'get_codex_workspace_diff'));
  assert(diffCallsAfterDeleted.length === 2 && diffCallsAfterDeleted[1].args.relativePath === 'README.md', 'deleted 行 diff 未打开', diffCallsAfterDeleted);
  await page.keyboard.press('Escape');
  await page.waitForFunction(() => !document.querySelector('[data-testid="code-viewer-modal"]'), { timeout: 5000 });

  // 文件 tab：文件行「在新窗口打开」→ open_code_reader 无 kind 键（回归）。
  await page.evaluate(() => {
    [...document.querySelectorAll('button')].find((el) => el.textContent.trim() === '文件')?.click();
  });
  await page.waitForFunction(() => [...document.querySelectorAll('button[title="在新窗口打开"]')].length > 0, { timeout: 5000 });
  await page.evaluate(() => [...document.querySelectorAll('button[title="在新窗口打开"]')][0]?.click());
  await page.waitForFunction(() => window.__invokeLog.filter((entry) => entry.command === 'open_code_reader').length === 2, { timeout: 5000 });
  const fileReaderCalls = await page.evaluate(() => window.__invokeLog.filter((entry) => entry.command === 'open_code_reader'));
  const lastCall = fileReaderCalls[fileReaderCalls.length - 1];
  assert(lastCall.args.relativePath === 'src/main.py' && !('kind' in lastCall.args), '文件模式 open_code_reader 不应带 kind', fileReaderCalls);

  assert(pageErrors.length === 0, '浏览器运行时异常', pageErrors);

  console.log('workspace_panel_session_smoke: ok');
} catch (error) {
  console.error('FAIL:', error?.stack || error);
  process.exitCode = 1;
} finally {
  if (browser) await browser.close();
  if (vite) await vite.close();
  fs.rmSync(profile, { recursive: true, force: true });
}
