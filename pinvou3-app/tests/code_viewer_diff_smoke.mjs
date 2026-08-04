#!/usr/bin/env node
// 弹窗 diff 模式冒烟：Diff 语言徽标、diff 语法高亮（+/− 行）、截断横幅、
// reveal/open 隐藏、「在新窗口打开」回调、复制内容可用。
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

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-code-viewer-diff-smoke-'));
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
  const url = `http://127.0.0.1:${address.port}/tests/fixtures/code_viewer_diff_smoke.html`;

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
  await page.waitForSelector('[data-testid="code-viewer-modal"]', { timeout: 10000 });

  const initial = await page.evaluate(() => {
    const modal = document.querySelector('[data-testid="code-viewer-modal"]');
    const body = modal.querySelector('[data-testid="code-viewer-body"]');
    return {
      dialogText: modal.querySelector('[role="dialog"]')?.innerText || '',
      languageBadge: [...modal.querySelectorAll('span')].map((el) => el.textContent.trim()).find((text) => text === 'Diff'),
      hasDiffCode: !!body.querySelector('pre.pinvou-code-block > code.hljs.language-diff'),
      additionCount: body.querySelectorAll('.hljs-addition').length,
      deletionCount: body.querySelectorAll('.hljs-deletion').length,
      metaCount: body.querySelectorAll('.hljs-meta').length,
      commentCount: body.querySelectorAll('.hljs-comment').length,
      commentText: [...body.querySelectorAll('.hljs-comment')].map(el => el.textContent).join('\n'),
      headerOld: [...body.querySelectorAll('.hljs-diff-file-header.old')].map(el => el.textContent).join('\n'),
      headerNew: [...body.querySelectorAll('.hljs-diff-file-header.new')].map(el => el.textContent).join('\n'),
      headerColors: {
        old: getComputedStyle(body.querySelector('.hljs-diff-file-header.old')).color,
        new: getComputedStyle(body.querySelector('.hljs-diff-file-header.new')).color,
        oldBg: getComputedStyle(body.querySelector('.hljs-diff-file-header.old')).backgroundColor,
        addRow: getComputedStyle(body.querySelector('.hljs-addition')).color,
        delRow: getComputedStyle(body.querySelector('.hljs-deletion')).color,
      },
      truncatedBanner: body.innerText.includes('内容过大'),
      revealButtons: [...modal.querySelectorAll('button[title="在文件管理器中显示"]')].length,
      openButtons: [...modal.querySelectorAll('button[title="用系统应用打开"]')].length,
      hasNewWindowButton: !!modal.querySelector('[data-testid="code-viewer-open-in-new-window"]'),
      copyContentDisabled: [...modal.querySelectorAll('button')]
        .find((button) => button.title === '复制内容')?.disabled,
    };
  });

  assert(initial.dialogText.includes('main.py') && initial.dialogText.includes('src/main.py'), '头部文件名/路径缺失', initial);
  assert(initial.languageBadge === 'Diff', '语言徽标应为 Diff', initial.languageBadge);
  assert(initial.hasDiffCode, 'diff 高亮 code.hljs.language-diff 未渲染', initial);
  assert(initial.additionCount > 0 && initial.deletionCount > 0, 'diff 语法高亮 +/− token 缺失', initial);
  assert(initial.metaCount > 0, 'diff hunk 行 meta 着色缺失', initial);
  assert(initial.commentCount > 0, 'diff --git/index 头行应为 comment', initial);
  assert(
    initial.headerOld.includes('--- a/src/main.py') && initial.headerNew.includes('+++ b/src/main.py'),
    '---/+++ 文件头行应渲染为红绿文件头（hljs-diff-file-header old/new）',
    { headerOld: initial.headerOld, headerNew: initial.headerNew },
  );
  assert(
    initial.headerColors.old === initial.headerColors.delRow && initial.headerColors.new === initial.headerColors.addRow,
    '文件头文字色应与修改行一致（--- 红 / +++ 绿），且无背景块',
    initial.headerColors,
  );
  assert(
    initial.headerColors.oldBg === 'rgba(0, 0, 0, 0)' || initial.headerColors.oldBg === 'transparent',
    '文件头行不得带修改块背景',
    initial.headerColors.oldBg,
  );
  assert(initial.truncatedBanner, '截断提示条缺失', initial);
  assert(initial.revealButtons === 0 && initial.openButtons === 0, 'diff 模式不应有 reveal/open 按钮', initial);
  assert(initial.hasNewWindowButton, '「在新窗口打开」按钮缺失', initial);
  assert(initial.copyContentDisabled === false, '复制内容按钮应可用', initial);

  // 点击「在新窗口打开」→ 回调被调（面板侧负责携带 kind 参数）。
  await page.click('[data-testid="code-viewer-open-in-new-window"]');
  const newWindowCalls = await page.evaluate(() => window.__newWindowCalls || 0);
  assert(newWindowCalls === 1, '「在新窗口打开」回调未被调用', { newWindowCalls });

  // 深色模式：diff 弹窗裸文本（上下文行/分段标题）文字色跟随主题，不得黑字黑底。
  await page.evaluate(() => document.documentElement.classList.add('dark'));
  await page.waitForFunction(() => {
    const pre = document.querySelector('[data-testid="code-viewer-modal"] pre.pinvou-code-block');
    return pre && getComputedStyle(pre).color !== 'rgb(0, 0, 0)';
  });
  const darkPreColor = await page.evaluate(() => getComputedStyle(document.querySelector('[data-testid="code-viewer-modal"] pre.pinvou-code-block')).color);
  assert(darkPreColor !== 'rgb(0, 0, 0)', '深色模式下 diff 弹窗文字应为浅色', { darkPreColor });

  assert(pageErrors.length === 0, '浏览器运行时异常', pageErrors);

  console.log('code_viewer_diff_smoke: ok');
} catch (error) {
  console.error('FAIL:', error?.stack || error);
  process.exitCode = 1;
} finally {
  if (browser) await browser.close();
  if (vite) await vite.close();
  fs.rmSync(profile, { recursive: true, force: true });
}
