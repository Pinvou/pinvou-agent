#!/usr/bin/env node
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

const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-code-viewer-smoke-'));
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
  const url = `http://127.0.0.1:${address.port}/tests/fixtures/code_viewer_smoke.html`;

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
    const dialog = modal.querySelector('[role="dialog"]');
    const body = modal.querySelector('[data-testid="code-viewer-body"]');
    return {
      hasDialog: !!dialog,
      dialogText: dialog?.innerText || '',
      bodyOverflow: document.body.style.overflow,
      hasCodeBlock: !!body.querySelector('pre.pinvou-code-block > code.hljs'),
      keywordCount: body.querySelectorAll('.pinvou-code-block .hljs-keyword').length,
      resizeHandles: ['code-viewer-resize-x', 'code-viewer-resize-y', 'code-viewer-resize-xy']
        .map((testid) => {
          const handle = modal.querySelector(`[data-testid="${testid}"]`);
          return handle ? handle.getAttribute('role') : null;
        }),
      size: dialog ? { width: dialog.style.width, height: dialog.style.height } : null,
    };
  });

  assert(initial.hasDialog, '弹窗 dialog 未渲染', initial);
  assert(initial.dialogText.includes('example.js') && initial.dialogText.includes('src/example.js'), '头部文件名/路径缺失', initial);
  assert(initial.bodyOverflow === 'hidden', '打开弹窗时未锁定 body 滚动', initial);
  assert(initial.hasCodeBlock, '高亮 pre.pinvou-code-block 未渲染', initial);
  assert(initial.keywordCount > 0, '语法高亮 token 缺失', initial);
  assert(initial.dialogText.includes('JavaScript'), '语言徽标缺失', initial);
  assert(initial.dialogText.includes('内容过大'), '截断提示条缺失', initial);
  assert(initial.resizeHandles.every((role) => role === 'separator'), '拖拽 handle 缺失或 role 错误', initial.resizeHandles);
  assert(initial.size?.width === '1100px' && initial.size?.height === '760px', '默认尺寸错误', initial.size);

  // 右下角拖拽 (+30, +40) → 1130x800，并持久化到 localStorage。
  const handle = await page.$('[data-testid="code-viewer-resize-xy"]');
  const box = await handle.boundingBox();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 + 30, box.y + box.height / 2 + 40, { steps: 5 });
  await page.mouse.up();
  await page.waitForFunction(() => {
    const dialog = document.querySelector('[data-testid="code-viewer-modal"] [role="dialog"]');
    return dialog?.style.width === '1130px' && dialog?.style.height === '800px';
  });
  const persisted = await page.evaluate(() => JSON.parse(localStorage.getItem('pinvou_code_viewer_size') || 'null'));
  assert(persisted?.width === 1130 && persisted?.height === 800, '尺寸未持久化', persisted);

  // Esc 关闭并恢复 body 滚动。
  await page.keyboard.press('Escape');
  await page.waitForFunction(() => !document.querySelector('[data-testid="code-viewer-modal"]'));
  const overflowAfterClose = await page.evaluate(() => document.body.style.overflow);
  assert(overflowAfterClose === '', '关闭弹窗后未恢复 body 滚动', { overflowAfterClose });

  // 重新打开 → 尺寸从 localStorage 恢复。
  await page.click('[data-testid="reopen"]');
  await page.waitForFunction(() => {
    const dialog = document.querySelector('[data-testid="code-viewer-modal"] [role="dialog"]');
    return dialog?.style.width === '1130px' && dialog?.style.height === '800px';
  });

  // 双击右下角 handle → 恢复默认尺寸。CDP 需要完整 down/up 序列（clickCount 1 → 2）才会合成 dblclick。
  const corner = await page.$('[data-testid="code-viewer-resize-xy"]');
  const cornerBox = await corner.boundingBox();
  const cornerX = cornerBox.x + cornerBox.width / 2;
  const cornerY = cornerBox.y + cornerBox.height / 2;
  await page.mouse.move(cornerX, cornerY);
  await page.mouse.down();
  await page.mouse.up();
  await page.mouse.down({ clickCount: 2 });
  await page.mouse.up({ clickCount: 2 });
  await page.waitForFunction(() => {
    const dialog = document.querySelector('[data-testid="code-viewer-modal"] [role="dialog"]');
    return dialog?.style.width === '1100px' && dialog?.style.height === '760px';
  });
  const resetPersisted = await page.evaluate(() => JSON.parse(localStorage.getItem('pinvou_code_viewer_size') || 'null'));
  assert(resetPersisted?.width === 1100 && resetPersisted?.height === 760, '双击恢复默认后持久化值错误', resetPersisted);

  // 默认字号 12px / 行高 19px；A+ → 13px 并持久化；A− 回到 12px。
  const initialFont = await page.evaluate(() => {
    const pre = document.querySelector('[data-testid="code-viewer-pre"]');
    return { fontSize: pre?.style.fontSize, lineHeight: pre?.style.lineHeight };
  });
  assert(initialFont.fontSize === '12px' && initialFont.lineHeight === '19px', '默认字号/行高错误', initialFont);
  await page.click('[data-testid="code-viewer-font-increase"]');
  await page.waitForFunction(() => document.querySelector('[data-testid="code-viewer-pre"]')?.style.fontSize === '13px');
  const persistedFont = await page.evaluate(() => Number(localStorage.getItem('pinvou_code_viewer_font_size')));
  assert(persistedFont === 13, '字号未持久化', { persistedFont });
  await page.click('[data-testid="code-viewer-font-decrease"]');
  await page.waitForFunction(() => document.querySelector('[data-testid="code-viewer-pre"]')?.style.fontSize === '12px');

  // Esc 关闭后重开 → 字号从 localStorage 恢复（先调到 14px 再验证）。
  await page.click('[data-testid="code-viewer-font-increase"]');
  await page.click('[data-testid="code-viewer-font-increase"]');
  await page.waitForFunction(() => document.querySelector('[data-testid="code-viewer-pre"]')?.style.fontSize === '14px');
  await page.keyboard.press('Escape');
  await page.waitForFunction(() => !document.querySelector('[data-testid="code-viewer-modal"]'));
  await page.click('[data-testid="reopen"]');
  await page.waitForFunction(() => document.querySelector('[data-testid="code-viewer-pre"]')?.style.fontSize === '14px');

  // 深色模式：弹窗文字色跟随主题（此前 dialog 缺 text 色导致裸文本黑字黑底）。
  await page.evaluate(() => document.documentElement.classList.add('dark'));
  await page.waitForFunction(() => {
    const pre = document.querySelector('[data-testid="code-viewer-modal"] pre.pinvou-code-block');
    return pre && getComputedStyle(pre).color !== 'rgb(0, 0, 0)';
  });
  const darkPreColor = await page.evaluate(() => getComputedStyle(document.querySelector('[data-testid="code-viewer-modal"] pre.pinvou-code-block')).color);
  assert(darkPreColor !== 'rgb(0, 0, 0)', '深色模式下弹窗文字应为浅色', { darkPreColor });

  assert(pageErrors.length === 0, '浏览器运行时异常', pageErrors);

  console.log('code_viewer_modal_smoke: ok');
} catch (error) {
  console.error('FAIL:', error?.stack || error);
  process.exitCode = 1;
} finally {
  if (browser) await browser.close();
  if (vite) await vite.close();
  fs.rmSync(profile, { recursive: true, force: true });
}
