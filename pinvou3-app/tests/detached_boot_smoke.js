#!/usr/bin/env node
/**
 * detached 启动冒烟：以 ?detached=1&kind=workflow 加载 index.html，
 * 断言 ① window.__PINVOU_DETACHED__===true ② 渲染了撕离标题栏(含 kind) ③ 不渲染侧边栏(无「新对话/近期」)。
 * 用 document.body.innerText(只含渲染出的可见文本)判断，避免 page.content() 把 <script> 里的 dict 字面量算进去。
 * 用法：node pinvou3-app/tests/detached_boot_smoke.js   (PASS→0 / FAIL→1 / 缺依赖→2)
 */
const fs = require('fs'), path = require('path'), os = require('os');
const { startUiTestServer } = require('./ui_test_server');
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
const CHROME = process.env.CHROME ||
  ['/snap/bin/chromium','/usr/bin/chromium','/usr/bin/chromium-browser','/usr/bin/google-chrome','/usr/bin/google-chrome-stable'].find(p => fs.existsSync(p));
if (!CHROME) { console.error('SKIP: 未找到 chromium'); process.exit(2); }
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-detached-'));

// 最小 mock TauriBridge 底座：available=true，所有 invoke 返回 {}，listen 返回退订函数。
function injectSource() {
  return `(function(){
    window.__TAURI__ = { core: { invoke: async()=>({}) }, event: { listen: async()=>(()=>{}), emit: async()=>{} } };
  })();`;
}

(async () => {
  const { url } = await startUiTestServer();
  const INDEX = url + '?detached=1&kind=workflow';
  const browser = await puppeteer.launch({ executablePath: CHROME, headless: 'new',
    userDataDir: PROFILE, args: ['--no-sandbox'] });
  const page = await browser.newPage();
  await page.evaluateOnNewDocument(injectSource());
  await page.goto(INDEX, { waitUntil: 'networkidle0' });
  await new Promise(r => setTimeout(r, 1500)); // 等 babel 编译 + 首渲染

  const detachedFlag = await page.evaluate(() => window.__PINVOU_DETACHED__ === true);
  const text = await page.evaluate(() => document.body.innerText || '');

  let ok = true;
  if (!detachedFlag) { console.error('FAIL: __PINVOU_DETACHED__ 未置 true'); ok = false; }
  if (!/撕离窗口|workflow/.test(text)) { console.error('FAIL: 未渲染撕离标题栏，innerText=', JSON.stringify(text.slice(0,120))); ok = false; }
  if (/新对话|近期/.test(text)) { console.error('FAIL: detached 模式仍渲染了侧边栏(出现 新对话/近期)'); ok = false; }

  await browser.close(); fs.rmSync(PROFILE, { recursive: true, force: true });
  if (ok) { console.log('PASS: detached 启动只渲染面板、无侧边栏'); process.exit(0); }
  process.exit(1);
})();
