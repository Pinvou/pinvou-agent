#!/usr/bin/env node
/**
 * 会话中「打开」技能开关的 pending/锁死语义 smoke：
 * - 活动会话中打开技能 → 开关仍可改回（未提交，不锁死）；
 * - 真实发送一轮（bridge.chat.sendMessage，mock 后端受理）→ 开关锁死（只增不减）。
 * 依赖先运行 `npm run build:ui`。
 */
const fs = require('fs'), path = require('path'), os = require('os');
const { startUiTestServer } = require('./ui_test_server');

function loadPuppeteer() {
  try { return require('puppeteer-core'); } catch (_) { /* fall through */ }
  const npx = path.join(os.homedir(), '.npm', '_npx');
  if (fs.existsSync(npx)) for (const d of fs.readdirSync(npx)) {
    const p = path.join(npx, d, 'node_modules', 'puppeteer-core');
    if (fs.existsSync(p)) try { return require(p); } catch (_) { /* next */ }
  }
  console.error('SKIP: 找不到 puppeteer-core');
  process.exit(2);
}

const puppeteer = loadPuppeteer();
const chromeCandidates = [
  '/snap/bin/chromium',
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
  '/usr/bin/google-chrome',
  '/usr/bin/google-chrome-stable',
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  '/Applications/Chromium.app/Contents/MacOS/Chromium',
  '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
  path.join(process.env.ProgramFiles || 'C:\\Program Files', 'Google', 'Chrome', 'Application', 'chrome.exe'),
  path.join(process.env['ProgramFiles(x86)'] || 'C:\\Program Files (x86)', 'Google', 'Chrome', 'Application', 'chrome.exe'),
  path.join(process.env.LOCALAPPDATA || '', 'Google', 'Chrome', 'Application', 'chrome.exe'),
  path.join(process.env.ProgramFiles || 'C:\\Program Files', 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
  path.join(process.env['ProgramFiles(x86)'] || 'C:\\Program Files (x86)', 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
].filter(Boolean);
const CHROME = process.env.CHROME ||
  chromeCandidates.find(fs.existsSync);
if (!CHROME) { console.error('SKIP: 未找到 chromium/chrome'); process.exit(2); }
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-pending-enable-'));

function injectSource() {
  return `(function(){
    const state=window.__PENDING_ENABLE_TEST__={calls:[],disabled:['visualizer'],committedEvents:0};
    window.addEventListener('pinvou:chat-round-committed',()=>{state.committedEvents+=1;});
    function record(cmd,args){state.calls.push({cmd,args:args||{}});}
    const session={id:'s1',title:'S1',created_at:1,updated_at:1};
    function invoke(cmd,args){
      record(cmd,args);
      switch(cmd){
        case 'get_settings': return Promise.resolve({theme:'liquid-light',language:'zh-Hans'});
        case 'get_effective_model_config': return Promise.resolve({model:'qwen36_35b_256k',base_url:'http://127.0.0.1:8000/v1',api_key_set:true});
        case 'list_sessions': return Promise.resolve([session]);
        case 'load_session': return Promise.resolve({metadata:session,messages:[],artifacts:[]});
        case 'get_super_permission_status': return Promise.resolve(false);
        case 'list_personas': return Promise.resolve([]);
        case 'get_backend_status': return Promise.resolve({online:true,ok:true,status:'online'});
        case 'check_for_update': return Promise.resolve({available:false});
        case 'find_resumable_run': return Promise.resolve(null);
        case 'list_workspace_files': case 'get_session_persona_events': case 'get_session_pinvou_reviews': case 'get_session_timeline': return Promise.resolve([]);
        case 'get_mode_state': return Promise.resolve({mode:'yolo',plan_phase:'none'});
        case 'get_active_persona': return Promise.resolve(null);
        case 'detect_local_vllm_setup': return Promise.resolve({eligible:false});
        case 'list_marketplace_tools': return Promise.resolve([]);
        case 'list_marketplace_skills': return Promise.resolve([
          {id:'visualizer',title:'数据分析可视化',description:'Chart.js 仪表盘',installed:true,user_uploaded:false},
        ]);
        case 'get_disabled_connectors': return Promise.resolve(state.disabled);
        case 'set_disabled_connectors': state.disabled=(args&&args.connectorIds)||[]; return Promise.resolve(null);
        case 'get_disabled_skills': return Promise.resolve([]);
        case 'get_bundle_visibility': return Promise.resolve([]);
        case 'feishu_skills_state': case 'wecom_skills_state': case 'dingtalk_skills_state': case 'tmeet_skills_state': return Promise.resolve({connected:false,enabled:true});
        default: return Promise.resolve(null);
      }
    }
    window.__TAURI__={core:{invoke},event:{listen(){return Promise.resolve(function(){});},emit(){return Promise.resolve();}},
      window:{getCurrentWindow(){return {minimize(){},maximize(){},close(){},toggleMaximize(){},isMaximized(){return Promise.resolve(false);},onResized(){return Promise.resolve(function(){});},startDragging(){}};}},
      dialog:{open(){return Promise.resolve(null);}}};
  })();`;
}

const sleep = ms => new Promise(r => setTimeout(r, ms));

(async () => {
  // 默认用构建产物起本地服务；设 PINVOU3_TEST_URL 可直接打正在运行的 dev server。
  const externalUrl = process.env.PINVOU3_TEST_URL || '';
  const { url } = externalUrl ? { url: externalUrl } : await startUiTestServer();
  const browser = await puppeteer.launch({ executablePath: CHROME, headless: 'new', args: ['--no-sandbox', '--disable-gpu', '--no-first-run'], userDataDir: PROFILE });
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', e => errors.push(e.message));
  await page.evaluateOnNewDocument(injectSource());
  await page.setViewport({ width: 1360, height: 900 });
  await page.goto(url, { waitUntil: 'networkidle0' });
  await sleep(1500);

  const results = [];
  const rec = (name, pass, detail = '') => { results.push({ name, pass }); console.log(`${pass ? '✅' : '❌'} ${name}${detail ? '  ' + detail : ''}`); };

  // 建立活动会话（只增不减守卫的前提）。
  await page.evaluate(() => window.TauriBridge.sessions.switchToSession('s1'));
  await sleep(600);
  const active = await page.evaluate(() => (window.TauriBridge.state.get('sessions') || {}).activeSessionId);
  rec('活动会话已建立', active === 's1', String(active));

  // 打开工具菜单：visualizer 初始为关（disabled 列表含它）。
  await page.evaluate(() => document.querySelector('button[title="工具"]').click());
  await sleep(300);
  const before = await page.evaluate(() => {
    const btn = document.querySelector('button[aria-label="visualizer"]');
    return btn ? { disabled: btn.disabled, on: btn.className.includes('bg-[#34C759]') } : null;
  });
  rec('初始为关且开关可点（允许打开）', !!before && !before.on && !before.disabled, JSON.stringify(before));

  // 会话中打开 → 未提交态：开关仍可点（允许改回，不锁死）。
  await page.evaluate(() => document.querySelector('button[aria-label="visualizer"]').click());
  await sleep(300);
  const afterEnable = await page.evaluate(() => {
    const btn = document.querySelector('button[aria-label="visualizer"]');
    const state = window.__PENDING_ENABLE_TEST__;
    return btn ? { disabled: btn.disabled, on: btn.className.includes('bg-[#34C759]'), persisted: !state.disabled.includes('visualizer') } : null;
  });
  rec('打开后未锁死（发送新一轮前可改回）', !!afterEnable && afterEnable.on && !afterEnable.disabled, JSON.stringify(afterEnable));
  rec('打开已持久化到禁用集之外', !!afterEnable && afterEnable.persisted);

  // 真实发送一轮（mock 后端受理）→ commit 事件 → 锁死。
  await page.evaluate(() => document.querySelector('button[title="工具"]').click()); // 先关弹层
  await sleep(200);
  await page.evaluate(() => window.TauriBridge.chat.sendMessage('hello'));
  await sleep(600);
  const committed = await page.evaluate(() => window.__PENDING_ENABLE_TEST__.committedEvents);
  rec('发送后轮次提交事件已派发', committed >= 1, `events=${committed}`);

  await page.evaluate(() => document.querySelector('button[title="工具"]').click());
  await sleep(300);
  const afterSend = await page.evaluate(() => {
    const btn = document.querySelector('button[aria-label="visualizer"]');
    return btn ? { disabled: btn.disabled, on: btn.className.includes('bg-[#34C759]') } : null;
  });
  rec('发送新一轮后开关锁死（只增不减）', !!afterSend && afterSend.on && afterSend.disabled, JSON.stringify(afterSend));

  rec('页面无未处理 JavaScript 异常', errors.length === 0, errors.slice(0, 2).join(' | '));

  await browser.close();
  fs.rmSync(PROFILE, { recursive: true, force: true });
  const failed = results.filter(r => !r.pass).length;
  console.log(failed ? `\n❌ ${failed}/${results.length} FAILED` : `\n✅ ALL ${results.length} PASS`);
  process.exit(failed ? 1 : 0);
})().catch(e => {
  try { fs.rmSync(PROFILE, { recursive: true, force: true }); } catch (_) {}
  console.error('FATAL', e.stack || e);
  process.exit(1);
});
