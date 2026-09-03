#!/usr/bin/env node
// 阅读器窗口 diff tab 冒烟：pending 拉取走 get_codex_workspace_diff（args 不含 workspacePath/kind）、
// 标签带 diffSuffix 后缀、同路径文件/diff 双 tab 共存、diff tab 无 reveal/open、
// 关闭后重推重新加载（缓存清除）。
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

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-reader-diff-smoke-'));
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
    // One-shot smoke page: file watching is never used and only ENOSPC-flakes
    // server startup on hosts with low inotify limits.
    server: { host: '127.0.0.1', port: 0, strictPort: false, watch: null },
  });
  await vite.listen();
  const address = vite.httpServer.address();
  const url = `http://127.0.0.1:${address.port}/tests/fixtures/reader_app_diff_smoke.html`;

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

  // 启动拉取 pending → diff tab 打开，走 get_codex_workspace_diff。
  await page.waitForFunction(() => document.querySelector('.hljs.language-diff'), { timeout: 10000 });
  const diffCalls = await page.evaluate(() => window.__invokeLog.filter((entry) => entry.command === 'get_codex_workspace_diff'));
  assert(
    diffCalls.length === 1
      && diffCalls[0].args.sessionId === 's-1'
      && diffCalls[0].args.relativePath === 'src/main.py'
      && !('workspacePath' in diffCalls[0].args)
      && !('kind' in diffCalls[0].args),
    'diff 拉取参数错误（应恰为 sessionId + relativePath）',
    diffCalls,
  );
  const pendingCalls = await page.evaluate(() => window.__invokeLog.filter((entry) => entry.command === 'take_code_reader_pending'));
  assert(pendingCalls.length === 1, '启动时应拉取一次 pending 队列', pendingCalls);

  // 标签文本带 diffSuffix（zh '(差异)'）；diff tab 无 reveal/open 按钮。
  let names = await page.evaluate(tabNames);
  assert(names.length === 1 && names[0] === 'main.py(差异)', 'diff tab 名称应带后缀', names);
  let toolbarButtons = await page.evaluate(() => [...document.querySelectorAll('button[title]')].map((el) => el.title));
  assert(!toolbarButtons.includes('在文件管理器中显示') && !toolbarButtons.includes('用系统应用打开'), 'diff tab 不应有 reveal/open 按钮', toolbarButtons);

  // 事件推送同路径文件请求（无 kind）→ 第二个 tab（文件与 diff 共存）。
  await page.evaluate(() => window.__readerOpenHandler({
    payload: { kind: null, sessionId: 's-1', workspacePath: null, relativePath: 'src/main.py' },
  }));
  await page.waitForFunction(() => document.body.innerText.includes("print('src/main.py')"), { timeout: 5000 });
  names = await page.evaluate(tabNames);
  assert(names.length === 2 && names.includes('main.py(差异)') && names.includes('main.py'), '同路径 diff/file 应共存两个 tab', names);

  // 文件 tab 激活 → reveal/open 出现。
  await page.evaluate(() => {
    const tabs = [...document.querySelectorAll('.group button[title]')].filter((el) => el.title !== '关闭标签页');
    tabs.find((el) => el.textContent.trim() === 'main.py')?.click();
  });
  await page.waitForFunction(() => [...document.querySelectorAll('button[title]')].some((el) => el.title === '在文件管理器中显示'), { timeout: 5000 });
  toolbarButtons = await page.evaluate(() => [...document.querySelectorAll('button[title]')].map((el) => el.title));
  assert(toolbarButtons.includes('在文件管理器中显示') && toolbarButtons.includes('用系统应用打开'), '文件 tab 应有 reveal/open 按钮', toolbarButtons);

  // 切回 diff tab → reveal/open 消失。
  await page.evaluate(() => {
    const tabs = [...document.querySelectorAll('.group button[title]')].filter((el) => el.title !== '关闭标签页');
    tabs.find((el) => el.textContent.trim() === 'main.py(差异)')?.click();
  });
  await page.waitForFunction(() => document.querySelector('.hljs.language-diff'), { timeout: 5000 });

  // 关闭 diff tab 后重推同请求 → 再次调用 get_codex_workspace_diff（缓存清除路径）。
  await page.evaluate(() => {
    const closeButtons = [...document.querySelectorAll('button[aria-label="关闭标签页"]')];
    closeButtons.find((el) => el.closest('.group')?.innerText.includes('main.py(差异)'))?.click();
  });
  await page.waitForFunction(() => [...document.querySelectorAll('.group button[title]')].filter((el) => el.title !== '关闭标签页').length === 1, { timeout: 5000 });
  await page.evaluate(() => window.__readerOpenHandler({
    payload: { kind: 'diff', sessionId: 's-1', workspacePath: null, relativePath: 'src/main.py' },
  }));
  await page.waitForFunction(() => document.querySelectorAll('.hljs.language-diff').length > 0, { timeout: 5000 });
  const diffCallsAfterReopen = await page.evaluate(() => window.__invokeLog.filter((entry) => entry.command === 'get_codex_workspace_diff'));
  assert(diffCallsAfterReopen.length === 2, '关闭后重推应重新加载 diff', diffCallsAfterReopen);

  assert(pageErrors.length === 0, '浏览器运行时异常', pageErrors);

  console.log('reader_app_diff_smoke: ok');
} catch (error) {
  console.error('FAIL:', error?.stack || error);
  process.exitCode = 1;
} finally {
  if (browser) await browser.close();
  if (vite) await vite.close();
  fs.rmSync(profile, { recursive: true, force: true });
}
