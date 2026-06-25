#!/usr/bin/env node
/**
 * 鬼影窗口启动冒烟：以 ?ghost=1&kind=workflow 加载 index.html，
 * 断言 ① window.__PINVOU_GHOST__===true ② 渲染出 chip(含「工作流」或 ⧉) ③ 无侧边栏(无 新对话/近期)。
 * 用法：node pinvou3-app/tests/ghost_boot_smoke.js   (PASS→0 / FAIL→1 / 缺依赖→2)
 */
const fs = require('fs'), path = require('path'), os = require('os');
function loadPuppeteer() {
  try { return require('puppeteer-core'); } catch (e) {}
  const npx = path.join(os.homedir(), '.npm', '_npx');
  if (fs.existsSync(npx)) for (const d of fs.readdirSync(npx)) {
    const p = path.join(npx, d, 'node_modules', 'puppeteer-core');
    if (fs.existsSync(p)) { try { return require(p); } catch (e) {} }
  }
  console.error('SKIP: 找不到 puppeteer-core'); process.exit(2);
}
const puppeteer = loadPuppeteer();
const INDEX = 'file://' + path.join(__dirname, '..', 'src', 'index.html') + '?ghost=1&kind=workflow';
const CHROME = process.env.CHROME ||
  ['/snap/bin/chromium','/usr/bin/chromium','/usr/bin/chromium-browser','/usr/bin/google-chrome','/usr/bin/google-chrome-stable'].find(p => fs.existsSync(p));
if (!CHROME) { console.error('SKIP: 未找到 chromium'); process.exit(2); }
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-ghost-'));

(async () => {
  const browser = await puppeteer.launch({ executablePath: CHROME, headless: 'new', userDataDir: PROFILE, args: ['--no-sandbox'] });
  const page = await browser.newPage();
  // 鬼影不挂 bridge,但 tauri-bridge.js 仍会跑;给个空壳 __TAURI__ 防它报错中断脚本。
  await page.evaluateOnNewDocument(`window.__TAURI__={core:{invoke:async()=>({})},event:{listen:async()=>(()=>{}),emit:async()=>{}}};`);
  await page.goto(INDEX, { waitUntil: 'networkidle0' });
  await new Promise(r => setTimeout(r, 1200));

  const ghostFlag = await page.evaluate(() => window.__PINVOU_GHOST__ === true);
  const text = await page.evaluate(() => document.body.innerText || '');

  let ok = true;
  if (!ghostFlag) { console.error('FAIL: __PINVOU_GHOST__ 未置 true'); ok = false; }
  if (!/工作流|⧉/.test(text)) { console.error('FAIL: 未渲染鬼影 chip，innerText=', JSON.stringify(text.slice(0,80))); ok = false; }
  if (/新对话|近期/.test(text)) { console.error('FAIL: 鬼影模式仍渲染了侧边栏'); ok = false; }

  await browser.close(); fs.rmSync(PROFILE, { recursive: true, force: true });
  if (ok) { console.log('PASS: 鬼影窗口只渲染 chip、无侧边栏'); process.exit(0); }
  process.exit(1);
})();
