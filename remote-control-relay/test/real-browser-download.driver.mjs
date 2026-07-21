#!/usr/bin/env node
// 真实浏览器全链路下载验证驱动:用 pinvou3-app 已有的 puppeteer-core 驱动真实
// Chrome/Chromium 打开 relay 服务的真实手机端页面,完成「选 session → 打开产物 →
// 预览 → 下载」全流程,验证接近上限(64MiB)文件经真实 relay + 真实桌面端 +
// 真实浏览器下载管线字节一致;随后覆盖超限拒绝、重复点击拦截、中途断连中断。
// 由 Rust e2e(real_browser_download_full_stack)拉起,参数来自 params JSON 文件。
// 浏览器二进制由 params.chromeBin 指定(环境 CHROME 或 Chrome for Testing 缓存)。

import { createHash } from 'node:crypto';
import { existsSync, readFileSync, readdirSync, statSync, writeFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const puppeteer = require('../../pinvou3-app/node_modules/puppeteer-core');

const params = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const {
  pageUrl,
  sessionTitle,
  sessionIdShort,
  artifactName,
  oversizeName,
  downloadDir,
  controlFile,
  sourceFile,
  expectedSize,
  chromeBin,
} = params;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
let browser = null;

function fail(message) {
  console.error(`[real-browser] FAIL: ${message}`);
  process.exitCode = 1;
  throw new Error(message);
}

function sha256File(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}

async function waitFor(desc, fn, timeoutMs = 60000, intervalMs = 300) {
  const deadline = Date.now() + timeoutMs;
  let last;
  for (;;) {
    try {
      last = await fn();
      if (last) return last;
    } catch (error) {
      last = error;
    }
    if (Date.now() > deadline) {
      fail(`等待超时(${timeoutMs}ms): ${desc}; 最后结果: ${JSON.stringify(last)}`);
    }
    await sleep(intervalMs);
  }
}

async function main() {
  mkdirSync(downloadDir, { recursive: true });
  browser = await puppeteer.launch({
    executablePath: chromeBin,
    headless: true,
    args: ['--no-sandbox', '--disable-dev-shm-usage'],
  });
  const page = await browser.newPage();
  const cdp = await page.createCDPSession();
  // 真实落盘:blob + a[download] 走浏览器真实下载管线,保存到 downloadDir。
  try {
    await cdp.send('Page.setDownloadBehavior', { behavior: 'allow', downloadPath: downloadDir });
  } catch {
    await cdp.send('Browser.setDownloadBehavior', { behavior: 'allow', downloadPath: downloadDir });
  }

  const systemHas = (substring) =>
    page.evaluate(
      (text) =>
        [...document.querySelectorAll('.system')].some((node) => node.textContent.includes(text)),
      substring,
    );
  const waitSystemText = (substring, timeoutMs = 60000) =>
    waitFor(`消息流出现「${substring}」`, () => systemHas(substring), timeoutMs);
  const sheetItemExists = (title, fallbackText) =>
    page.evaluate(
      (expected, fallback) =>
        [...document.querySelectorAll('#sheetBody .sheet-item')].some((item) => {
          const node = item.querySelector('.sheet-title');
          const text = node ? node.textContent : '';
          return text === expected || (fallback && text.includes(fallback));
        }),
      title,
      fallbackText || '',
    );
  const clickSheetItem = async (title, fallbackText) => {
    const clicked = await page.evaluate(
      (expected, fallback) => {
        const hit = [...document.querySelectorAll('#sheetBody .sheet-item')].find((item) => {
          const node = item.querySelector('.sheet-title');
          const text = node ? node.textContent : '';
          return text === expected || (fallback && text.includes(fallback));
        });
        if (!hit) return false;
        hit.click();
        return true;
      },
      title,
      fallbackText || '',
    );
    if (!clicked) fail(`面板里找不到条目「${title}」`);
  };
  const previewVisible = () =>
    page.evaluate(() => !document.getElementById('previewOverlay').classList.contains('hidden'));
  const waitPreview = (timeoutMs = 60000) => waitFor('预览层打开', previewVisible, timeoutMs);

  console.log('[real-browser] 打开房间页面…');
  await page.goto(pageUrl, { waitUntil: 'domcontentloaded', timeout: 60_000 });

  // 1. 加入房间后自动弹出 session 面板,选择目标 session。
  await waitFor('session 面板出现目标 session', () => sheetItemExists(sessionTitle, sessionIdShort), 60_000);
  await clickSheetItem(sessionTitle, sessionIdShort);
  console.log('[real-browser] 已选择 session');

  // 2. 打开产物面板,选中接近上限(64MiB)的产物并预览。
  await page.click('#artifactChip');
  await waitFor('产物面板出现近上限产物', () => sheetItemExists(artifactName), 60_000);
  await clickSheetItem(artifactName);
  await waitPreview();

  // 3. 点击「下载」,等待 64MiB 经真实 relay + 真实浏览器重组完成并真实落盘。
  await page.click('#previewDownload');
  console.log('[real-browser] 近上限下载已开始,等待完成…');
  await waitSystemText(`已下载 ${artifactName}`, 300_000);
  const target = join(downloadDir, artifactName);
  await waitFor(
    `浏览器保存 ${artifactName} 到下载目录`,
    () => {
      if (!existsSync(target)) return false;
      if (statSync(target).size !== expectedSize) return false;
      return !readdirSync(downloadDir).some((f) => f.endsWith('.crdownload'));
    },
    180_000,
    500,
  );
  const gotHash = sha256File(target);
  const expectHash = sha256File(sourceFile);
  if (gotHash !== expectHash) {
    fail(`下载文件 sha256 不一致: got ${gotHash}, expect ${expectHash}`);
  }
  console.log(`[real-browser] 64MiB 全链路字节一致(sha256 ${gotHash.slice(0, 16)}…)`);

  // 4. 超过上限(64MiB+1)的产物必须被拒绝,且不产生下载。
  await page.click('#previewClose');
  await page.click('#artifactChip');
  await clickSheetItem(oversizeName);
  await waitPreview();
  await page.click('#previewDownload');
  await waitSystemText('too large', 60_000);
  if (readdirSync(downloadDir).includes(oversizeName)) {
    fail('超限产物不应落盘');
  }
  console.log('[real-browser] 超限产物已按预期拒绝');

  // 5. 重复点击拦截 + 中途断连:再次下载 64MiB,下载中按钮必须禁用,
  //    真实点击不会再发请求;然后杀掉 relay,页面必须提示中断。
  await page.click('#previewClose');
  await page.click('#artifactChip');
  await clickSheetItem(artifactName);
  await waitPreview();
  await page.click('#previewDownload');
  await waitSystemText(`正在下载 ${artifactName}`, 60_000);
  const disabled = await page.$eval('#previewDownload', (el) => el.disabled);
  if (!disabled) fail('下载进行中「下载」按钮应处于禁用状态');
  await page.click('#previewDownload').catch(() => {});
  await sleep(2000);
  const stillBusy = await page.$eval('#previewDownload', (el) => el.disabled);
  if (!stillBusy) fail('重复点击后下载不应被重新开始(按钮应保持禁用)');
  console.log('[real-browser] 重复点击已被拦截(按钮禁用,桌面端计数由 Rust 侧断言)');
  writeFileSync(controlFile, 'kill_relay');
  await waitSystemText('中断', 60_000);
  console.log('[real-browser] 中途断连已按预期中断并提示');

  console.log('[real-browser] PASS');
}

main()
  .catch((error) => {
    if (process.exitCode === 0 || process.exitCode === undefined) {
      console.error(`[real-browser] ERROR: ${error && error.stack ? error.stack : error}`);
      process.exitCode = 1;
    }
  })
  .finally(async () => {
    if (browser) {
      try {
        await browser.close();
      } catch {}
    }
  });
