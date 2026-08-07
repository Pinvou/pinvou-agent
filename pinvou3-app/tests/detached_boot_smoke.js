#!/usr/bin/env node
/**
 * detached 启动冒烟：分别加载 workflow/session/monitor 独立窗口，
 * 断言 ① 只渲染撕离面板 ② session 还原目标历史 ③ monitor 能渲染实时快照。
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

// 最小 mock Tauri 底座：返回一个可辨识的历史会话和系统状态快照。
function injectSource() {
  return `(function(){
    var sessionId = 'detached-session-42';
    window.__TAURI__ = { core: { invoke: async(cmd)=>{
      if (cmd === 'get_platform_capabilities') return {};
      if (cmd === 'get_settings') return { theme:'genesis', language:'zh-Hans' };
      if (cmd === 'get_selected_pet') return 'lingling';
      if (cmd === 'get_effective_model_config') return {};
      if (cmd === 'get_app_version') return '0.7.6-test';
      if (cmd === 'list_models' || cmd === 'list_archived_sessions' || cmd === 'list_personas') return [];
      if (cmd === 'list_sessions') return [{ id:sessionId, title:'分离测试会话', updated_at:'2026-08-07T00:00:00Z', message_count:1 }];
      if (cmd === 'load_session') return { metadata:{ id:sessionId, title:'分离测试会话' }, messages:[{ role:'user', content:[{ type:'text', text:'DETACHED_SESSION_HISTORY_OK' }] }], artifacts:[] };
      if (cmd === 'get_mode_state') return { mode:'yolo' };
      if (cmd === 'get_super_permission_status') return false;
      if (cmd === 'get_backend_status') return { vllm_online:false };
      if (cmd === 'get_memory_overview') return {};
      if (cmd === 'get_monitor_snapshot') return {
        generated_at_ms:Date.now(),
        cpu:{ name:'DETACHED-MONITOR-CPU', total_usage_pct:37 },
        ram:{ used_kib:4194304, total_kib:8388608, swap_used_kib:0, swap_total_kib:1048576 },
        app:{ pinvou3_version:'DETACHED_MONITOR_OK', deepseek_tui_version:'test', session_uptime_secs:90 },
        self_perf:{}
      };
      if (/^list_|^get_session_|^kb_/.test(cmd)) return [];
      return {};
    } }, event: { listen: async()=>(()=>{}), emit: async()=>{} } };
  })();`;
}

(async () => {
  const { url } = await startUiTestServer();
  const browser = await puppeteer.launch({ executablePath: CHROME, headless: 'new',
    userDataDir: PROFILE, args: ['--no-sandbox'] });
  const page = await browser.newPage();
  await page.evaluateOnNewDocument(injectSource());
  await page.goto(url + '?detached=1&kind=workflow', { waitUntil: 'networkidle0' });
  await new Promise(r => setTimeout(r, 1500)); // 等 babel 编译 + 首渲染

  const detachedFlag = await page.evaluate(() => window.__PINVOU_DETACHED__ === true);
  const text = await page.evaluate(() => document.body.innerText || '');

  let ok = true;
  if (!detachedFlag) { console.error('FAIL: __PINVOU_DETACHED__ 未置 true'); ok = false; }
  if (!/撕离窗口|workflow/.test(text)) { console.error('FAIL: 未渲染撕离标题栏，innerText=', JSON.stringify(text.slice(0,120))); ok = false; }
  if (/新对话|近期/.test(text)) { console.error('FAIL: detached 模式仍渲染了侧边栏(出现 新对话/近期)'); ok = false; }

  await page.goto(url + '?detached=1&kind=session&id=detached-session-42', { waitUntil: 'networkidle0' });
  try {
    await page.waitForFunction(() => document.body.innerText.includes('DETACHED_SESSION_HISTORY_OK'), { timeout: 5000 });
  } catch (_) {
    console.error('FAIL: detached session 未渲染目标会话历史'); ok = false;
  }
  const activeSessionId = await page.evaluate(() => window.TauriBridge.state.get('sessions').activeSessionId);
  if (activeSessionId !== 'detached-session-42') {
    console.error('FAIL: detached session 未绑定目标 id，实际:', activeSessionId); ok = false;
  }

  await page.goto(url + '?detached=1&kind=monitor', { waitUntil: 'networkidle0' });
  try {
    await page.waitForFunction(() => document.body.innerText.includes('DETACHED-MONITOR-CPU')
      && document.body.innerText.includes('DETACHED_MONITOR_OK'), { timeout: 5000 });
  } catch (_) {
    console.error('FAIL: detached monitor 未渲染系统状态快照'); ok = false;
  }

  await browser.close(); fs.rmSync(PROFILE, { recursive: true, force: true });
  if (ok) { console.log('PASS: detached workflow/session/monitor 启动与状态投影正常'); process.exit(0); }
  process.exit(1);
})();
