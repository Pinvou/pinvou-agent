#!/usr/bin/env node
// 真实浏览器全链路上传验证驱动:用 pinvou3-app 已有的 puppeteer-core 驱动真实
// Chrome/Chromium 打开 relay 服务的真实手机端页面,完成「选 session → 选文件 →
// 等待桌面端 ingest 完成」全流程,验证多分块文件经真实 mobile web
// → 真实 relay → 真实桌面端 streaming task + file_ingest::ingest 全链路;
//随后覆盖:多分块文本文件、超限拒绝、abort 中止、文件名 XSS 转义、连击拦截。
// 由 Rust e2e(real_browser_upload_full_stack)拉起,参数来自 params JSON 文件。
// 浏览器二进制由 params.chromeBin 指定(环境 CHROME 或 Chrome for Testing 缓存)。

import { existsSync, readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const puppeteer = require('../../pinvou3-app/node_modules/puppeteer-core');

const params = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const {
  pageUrl,
  sessionTitle,
  sessionIdShort,
  chromeBin,
  smallFilePath,
  smallFileName,
  largeFilePath,
  largeFileName,
  oversizeFilePath,
  oversizeFileName,
  abortSlowFilePath,
  abortSlowFileName,
} = params;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
let browser = null;

function fail(message) {
  console.error(`[real-browser-upload] FAIL: ${message}`);
  process.exitCode = 1;
  throw new Error(message);
}

async function waitFor(desc, fn, timeoutMs = 120000, intervalMs = 300) {
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

// 装一个一次性 MutationObserver,记录「addSystem(...) 是否真被调用」。
// 为什么不直接等 system 文本出现?因为 applySessionSnapshot 在每次 snapshot 到达时
// 会 messages.innerHTML='' 重渲染(pre-existing 行为),会把刚加的 system 消息清掉。
// 用 MutationObserver 把每次 add 的 .system 节点留痕到 window.__seenSystem,
// 即便被后续 snapshot 清掉,记录仍在。
async function installSystemObserver(page) {
  await page.evaluate(() => {
    window.__seenSystem = [];
    const target = document.getElementById('messages');
    if (!target) return;
    const obs = new MutationObserver((muts) => {
      for (const m of muts) {
        for (const n of m.addedNodes) {
          if (n.nodeType === 1 && n.className === 'system') {
            window.__seenSystem.push(n.textContent || '');
          }
        }
      }
    });
    obs.observe(target, { childList: true, subtree: true });
  });
}

async function seenSystemContaining(page, substring) {
  const all = await page.evaluate(() => window.__seenSystem || []);
  return all.filter((t) => t.includes(substring));
}

async function main() {
  browser = await puppeteer.launch({
    executablePath: chromeBin,
    headless: true,
    args: ['--no-sandbox', '--disable-dev-shm-usage'],
  });
  const page = await browser.newPage();
  page.on('console', (msg) => {
    const t = msg.text();
    if (t && !/Failed to load resource|favicon/i.test(t)) {
      console.log(`[browser:console] ${t}`);
    }
  });
  page.on('pageerror', (err) => {
    console.log(`[browser:pageerror] ${err && err.message ? err.message : err}`);
  });

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
  const uploadCardByFilename = (filename) =>
    page.evaluate((expected) => {
      const cards = [...document.querySelectorAll('#attachmentPreview .attachment-card')];
      const hit = cards.find((c) => {
        const t = c.querySelector('.att-title');
        return t && t.textContent === expected;
      });
      if (!hit) return null;
      return {
        status: hit.className,
        sub: (hit.querySelector('.att-sub') || {}).textContent || '',
      };
    }, filename);
  const waitForCardDone = (filename, timeoutMs = 300_000) =>
    waitFor(
      `卡片 ${filename} 变 done`,
      () => uploadCardByFilename(filename).then((c) => c && /done/.test(c.status) && c),
      timeoutMs,
    );

  console.log('[real-browser-upload] 打开房间页面…');
  await page.goto(pageUrl, { waitUntil: 'domcontentloaded', timeout: 60_000 });
  await installSystemObserver(page);
  // 监听 fileInput change 事件,验证 puppeteer uploadFile 是否真触发了 onchange。
  await page.evaluate(() => {
    const fi = document.getElementById('fileInput');
    if (!fi) return;
    // 通过捕获阶段监听,先于 webclient 的 onchange 跑,这样才能看到 files 还没被清空时的状态。
    fi.addEventListener('change', () => {
      const f = fi.files && fi.files[0];
      // eslint-disable-next-line no-console
      console.log('[browser:fileInput-onchange-cap] files.length=' + (fi.files ? fi.files.length : 0)
        + ' first.size=' + (f ? f.size : '(no file)')
        + ' first.name=' + (f ? f.name : '(no file)'));
    }, true);
    // 同时监听所有 .system 节点的追加(包括错误路径的 addSystem,如「已有附件正在上传」
    // 「空文件无法上传」「附件超过 20MiB 上限」「附件上传失败」),定位 multi-chunk 卡在哪个分支。
    window.__seenSystemAll = [];
    const msgs = document.getElementById('messages');
    if (msgs) {
      const obs = new MutationObserver((muts) => {
        for (const m of muts) {
          for (const n of m.addedNodes) {
            if (n.nodeType === 1 && n.className === 'system') {
              window.__seenSystemAll.push(n.textContent || '');
              // eslint-disable-next-line no-console
              console.log('[browser:addSystem] ' + (n.textContent || '').slice(0, 120));
            }
          }
        }
      });
      obs.observe(msgs, { childList: true, subtree: true });
    }
  });

  // 1. 选 session
  await waitFor('session 面板出现目标 session', () => sheetItemExists(sessionTitle, sessionIdShort), 60_000);
  await clickSheetItem(sessionTitle, sessionIdShort);
  console.log('[real-browser-upload] 已选择 session');
  await waitFor('attachBtn 可用', () => page.$eval('#attachBtn', (el) => !el.disabled), 30_000);

  // 2. 小文本(2 MiB)→ 多分块路径(< 20MiB),验证 base64 多块合并正确
  console.log('[real-browser-upload] 上传小文本文件…');
  await page.$eval('#fileInput', (el) => { el.value = ''; });
  await (await page.$('#fileInput')).uploadFile(smallFilePath);
  await waitForCardDone(smallFileName, 120_000);
  const smallHits = await seenSystemContaining(page, `附件 ${smallFileName} 已就绪`);
  if (smallHits.length === 0) fail(`小文件成功路径未触发 addSystem('附件 ${smallFileName} 已就绪。')`);
  console.log('[real-browser-upload] 小文本已就绪');

  // 3. 多分块文件(4 MiB = 6 个 768KiB 分块)→ 多分块路径全链路
  console.log('[real-browser-upload] 上传多分块文件(4MiB)…');
  // 关键:waitForCardDone 看到 DOM 卡片变 done,但 requestAttachFile 内部 await
  // Promise 的 check() 轮询周期(200ms)还没跑到 resolve,finally(=uploadBusy=false)
  // 可能尚未执行。直接发起下一次 uploadFile 会被「uploadBusy 守卫」误拒。这里显式
  // 等 attachBtn 重新 enabled(其 disabled 直接由 uploadBusy 驱动),消除 race。
  await waitFor('attachBtn 在上次上传结束后重新可用', () =>
    page.$eval('#attachBtn', (el) => !el.disabled), 10_000);
  await page.$eval('#fileInput', (el) => { el.value = ''; });
  const poll2 = setInterval(async () => {
    try {
      const snapshot = await page.evaluate(() => {
        const cards = [...document.querySelectorAll('#attachmentPreview .attachment-card')].map((c) => ({
          title: (c.querySelector('.att-title') || {}).textContent,
          sub: (c.querySelector('.att-sub') || {}).textContent,
          cls: c.className,
        }));
        const attachDisabled = document.getElementById('attachBtn') ? document.getElementById('attachBtn').disabled : '(no btn)';
        return { cards, attachDisabled };
      });
      console.log(`[real-browser-upload:poll2] ${JSON.stringify(snapshot)}`);
    } catch (e) {
      console.log(`[real-browser-upload:poll2-err] ${e.message}`);
    }
  }, 5000);
  try {
    await (await page.$('#fileInput')).uploadFile(largeFilePath);
    await waitForCardDone(largeFileName, 120_000);
  } finally {
    clearInterval(poll2);
  }
  const largeHits = await seenSystemContaining(page, `附件 ${largeFileName} 已就绪`);
  if (largeHits.length === 0) fail(`多分块文件成功路径未触发 addSystem('附件 ${largeFileName} 已就绪。')`);
  console.log('[real-browser-upload] 多分块全链路上传成功');

  // 纯附件消息也必须进入完整 user_message 链路；桌面端会在这里把 uploads 临时源文件
  // 暂存进 session workspace 后再清理，避免图片/大文本 path 提前失效。
  await page.$eval('#input', (el) => { el.value = ''; });
  await page.click('#actionButton');
  await waitFor(
    '纯附件消息提交后清空附件卡片',
    () => page.$eval('#attachmentPreview', (el) => el.classList.contains('hidden')),
    30_000,
  );
  const attachmentBubble = await page.evaluate(() => {
    const bubbles = [...document.querySelectorAll('#messages .msg.user .bubble')];
    return bubbles.length ? bubbles[bubbles.length - 1].textContent : '';
  });
  if (!attachmentBubble.includes(smallFileName) || !attachmentBubble.includes(largeFileName)) {
    fail(`纯附件用户气泡未显示文件名: ${attachmentBubble}`);
  }

  // 4. 超限(20MiB + 1)→ 客户端预检拒绝,不发起 attach_file_start
  console.log('[real-browser-upload] 上传超限文件(20MiB+1),预期客户端拒绝…');
  await page.$eval('#fileInput', (el) => { el.value = ''; });
  await (await page.$('#fileInput')).uploadFile(oversizeFilePath);
  await waitFor(
    '超限文件被客户端预检拒绝(system 提示)',
    () => seenSystemContaining(page, '超过 20MiB 上限').then((h) => h.length > 0),
    60_000,
  );
  // 超限绝不能进 attach 链路:卡片不应出现 done。
  const overCard = await uploadCardByFilename(oversizeFileName);
  if (overCard && /done/.test(overCard.status)) {
    fail(`超限文件 ${oversizeFileName} 不应进 attach 链路,但卡片显示 done`);
  }
  console.log('[real-browser-upload] 超限文件已按预期拒绝');

  // 5. abort:用专用大文件(abortSlowFilePath,Rust 侧构造 16MiB)上传,卡片出现即点 ×。
  //    16MiB 经 relay + base64 + ack 在本机耗时数秒,足以让 × 在 'uploading' 状态停留
  //    可见 → 可点击。4MiB 太快(秒级完成),× 一闪即逝,无法可靠点击。
  console.log('[real-browser-upload] 验证 abort 中止路径…');
  let aborted = false;
  for (let attempt = 0; attempt < 3 && !aborted; attempt++) {
    await waitFor('attachBtn 可用', () =>
      page.$eval('#attachBtn', (el) => !el.disabled), 10_000);
    await page.$eval('#fileInput', (el) => { el.value = ''; });
    await (await page.$('#fileInput')).uploadFile(abortSlowFilePath);
    // 卡片一旦出现且 status != done/error,× 就可见。等卡片可见后立即点。
    await waitFor('abort 卡片出现', () => uploadCardByFilename(abortSlowFileName), 30_000);
    const clicked = await page.evaluate((filename) => {
      const card = [...document.querySelectorAll('#attachmentPreview .attachment-card')]
        .find((c) => { const t = c.querySelector('.att-title'); return t && t.textContent === filename; });
      if (!card) return false;
      const x = card.querySelector('.att-x');
      if (!x) return false;
      x.click();
      return true;
    }, abortSlowFileName);
    if (!clicked) continue;
    let outcome = null;
    for (let i = 0; i < 60; i++) {
      await sleep(100);
      const aborts = await seenSystemContaining(page, '附件上传已中止');
      if (aborts.length > 0) { outcome = 'aborted'; break; }
    }
    if (outcome === 'aborted') { aborted = true; break; }
  }
  if (!aborted) fail('3 次 abort 重试均未触发 attach_file_aborted');

  // 6. XSS 转义:attachmentPreview 内任何时候都不应有 <script> 节点(文件名 /
  //    预览 / ingest warning 都走 textContent / escapeHtml)。puppeteer uploadFile
  //    使用真实磁盘文件的 basename,无法直接构造 <script>.txt,因此此处只做
  //    「到目前为止渲染过的卡片都无 script 注入」断言(防御性回归)。
  const scriptCount = await page.evaluate(() => document.querySelectorAll('#attachmentPreview script').length);
  if (scriptCount > 0) fail(`attachmentPreview 内存在 ${scriptCount} 个 <script> 节点,转义失败`);
  console.log('[real-browser-upload] XSS 转义断言通过');

  // 7. 连击拦截:上传进行中 attachBtn 应被禁用(用 16MiB 文件确保上传持续)
  console.log('[real-browser-upload] 验证连击拦截…');
  await waitFor('attachBtn 可用', () =>
    page.$eval('#attachBtn', (el) => !el.disabled), 10_000);
  await page.$eval('#fileInput', (el) => { el.value = ''; });
  await (await page.$('#fileInput')).uploadFile(abortSlowFilePath);
  await waitFor('上传中卡片出现', () => uploadCardByFilename(abortSlowFileName), 30_000);
  const attachDisabled = await page.$eval('#attachBtn', (el) => el.disabled);
  if (!attachDisabled) fail('上传进行中 attachBtn 必须禁用');
  // 等待这次上传结束(或 abort 超时),避免污染下一次断言
  await waitForCardDone(abortSlowFileName, 60_000).catch(() => {});
  console.log('[real-browser-upload] 连击拦截已生效');

  // 这次上传已经成功，因此它是待发送附件，不是磁盘泄漏。通过纯附件消息正常消费，
  // 再由 Rust 侧断言 uploads 临时目录已清空、session workspace 中稳定副本存在。
  await page.evaluate(() => {
    // 第一条纯附件消息在本 e2e 的轻量 dispatcher 中不会真正启动 Engine，因而也不会
    // 自然产生 session_status。补一条真实桌面端完成回合时会发送的 idle 状态。
    window.handleDesktopEvent({ type: 'session_status', payload: { status: '空闲' } });
    const input = document.getElementById('input');
    if (input) input.value = '';
    window.submitComposer();
  });
  await waitFor('连击场景附件已提交', () => page.evaluate((filename) => {
    return [...document.querySelectorAll('#messages .msg.user .bubble')]
      .some((node) => node.textContent.includes(filename));
  }, abortSlowFileName), 10_000);
  await sleep(250);

  console.log('[real-browser-upload] PASS');
}

main()
  .catch((error) => {
    if (process.exitCode === 0 || process.exitCode === undefined) {
      console.error(`[real-browser-upload] ERROR: ${error && error.stack ? error.stack : error}`);
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
