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

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-diff-smoke-'));
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
  const url = `http://127.0.0.1:${address.port}/tests/fixtures/diff_viewer_smoke.html`;

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
  await page.goto(url, { waitUntil: 'networkidle0' });
  await page.waitForSelector('[data-testid="edit-output"] [data-testid="diff-view"]', { timeout: 10000 });

  const initial = await page.evaluate(() => {
    const scope = document.querySelector('[data-testid="edit-output"]');
    const view = scope.querySelector('[data-testid="diff-view"]');
    const header = scope.querySelector('[data-testid="diff-file-header"]');
    const summary = scope.querySelector('[data-testid="diff-summary"]');
    const diagnostics = scope.querySelector('[data-testid="diff-diagnostics"]');
    const lines = [...scope.querySelectorAll('[data-testid="diff-line"]')];
    const firstDel = scope.querySelector('[data-diff-kind="del"]');
    const firstAdd = scope.querySelector('[data-diff-kind="add"]');
    const style = getComputedStyle(view);
    return {
      header: header && header.innerText,
      summary: summary && summary.innerText,
      diagnostics: diagnostics && diagnostics.innerText,
      lineCount: lines.length,
      addCount: lines.filter((line) => line.dataset.diffKind === 'add').length,
      delCount: lines.filter((line) => line.dataset.diffKind === 'del').length,
      firstDel: firstDel && { oldNo: firstDel.dataset.oldNo, newNo: firstDel.dataset.newNo },
      firstAdd: firstAdd && { oldNo: firstAdd.dataset.oldNo, newNo: firstAdd.dataset.newNo },
      maxHeight: style.maxHeight,
      overflowY: style.overflowY,
      clientHeight: view.clientHeight,
      scrollHeight: view.scrollHeight,
      fallbackHasDiff: !!document.querySelector('[data-testid="fallback-output"] [data-testid="diff-view"]'),
      fallbackText: document.querySelector('[data-testid="fallback-output"]')?.innerText || '',
      writeHeader: document.querySelector('[data-testid="write-output"] [data-testid="diff-file-header"]')?.innerText || '',
      writeAddCount: document.querySelectorAll('[data-testid="write-output"] [data-diff-kind="add"]').length,
      writeBackground: getComputedStyle(document.querySelector('[data-testid="write-output"] [data-testid="diff-view"]')).backgroundColor,
    };
  });

  assert(initial.header?.includes('b/src/example.js'), '未渲染文件路径', initial);
  assert(initial.header?.includes('+18') && initial.header?.includes('−18'), '增删统计错误', initial);
  assert(initial.lineCount === 36 && initial.addCount === 18 && initial.delCount === 18, 'diff 行渲染数量错误', initial);
  assert(initial.firstDel?.oldNo === '1' && initial.firstDel?.newNo === '', '删除行双行号错误', initial);
  assert(initial.firstAdd?.oldNo === '' && initial.firstAdd?.newNo === '1', '新增行双行号错误', initial);
  assert(initial.summary === 'Replaced 18 occurrences in src/example.js', '摘要未独立渲染', initial);
  assert(initial.diagnostics?.includes('simulated diagnostic') && !initial.summary.includes('diagnostics'), 'LSP 诊断未独立渲染', initial);
  assert(initial.maxHeight === '200px' && initial.overflowY === 'auto' && initial.scrollHeight > initial.clientHeight, '默认 200px 滚动状态错误', initial);
  assert(!initial.fallbackHasDiff && initial.fallbackText.includes('Replaced 0 occurrences'), '非 diff 文本没有降级到普通输出', initial);
  assert(initial.writeHeader.includes('b/notes/new file.md') && initial.writeHeader.includes('+2'), 'File.write 没有路由到深色 diff 视图', initial);
  assert(initial.writeAddCount === 2 && initial.writeBackground !== 'rgba(0, 0, 0, 0)', 'File.write 深色 diff 渲染错误', initial);

  await page.click('[data-testid="edit-output"] [aria-label="展开完整 diff"]');
  await page.waitForFunction(() => document.querySelector('[data-testid="edit-output"] [data-testid="diff-view"]')?.clientHeight > 200);
  const expanded = await page.$eval('[data-testid="edit-output"] [data-testid="diff-view"]', (view) => ({
    maxHeight: getComputedStyle(view).maxHeight,
    clientHeight: view.clientHeight,
    scrollHeight: view.scrollHeight,
    collapseButton: !!view.querySelector('[aria-label="收起 diff"]'),
  }));
  assert(expanded.maxHeight === 'none' && expanded.clientHeight === expanded.scrollHeight && expanded.collapseButton, '展开完整 diff 失败', expanded);

  await page.click('[data-testid="edit-output"] [aria-label="收起 diff"]');
  await page.waitForFunction(() => document.querySelector('[data-testid="edit-output"] [data-testid="diff-view"]')?.clientHeight <= 200);
  const collapsed = await page.$eval('[data-testid="edit-output"] [data-testid="diff-view"]', (view) => ({
    maxHeight: getComputedStyle(view).maxHeight,
    clientHeight: view.clientHeight,
    scrollHeight: view.scrollHeight,
    expandButton: !!view.querySelector('[aria-label="展开完整 diff"]'),
  }));
  assert(collapsed.maxHeight === '200px' && collapsed.scrollHeight > collapsed.clientHeight && collapsed.expandButton, '收起 diff 失败', collapsed);
  assert(pageErrors.length === 0, '浏览器运行时异常', pageErrors);

  console.log('diff_viewer_ui_smoke: ok');
} catch (error) {
  console.error('FAIL:', error?.stack || error);
  process.exitCode = 1;
} finally {
  if (browser) await browser.close();
  if (vite) await vite.close();
  fs.rmSync(profile, { recursive: true, force: true });
}
