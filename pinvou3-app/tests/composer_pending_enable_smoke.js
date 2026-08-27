#!/usr/bin/env node
/**
 * 会话中「打开」技能开关的 pending/锁死语义 smoke：
 * - 活动会话中打开技能 → 开关仍可改回（未提交，不锁死）；
 * - 真实发送一轮（bridge.chat.sendMessage，mock 后端受理）→ 开关锁死（只增不减）；
 * - 发送失败（mock 后端拒绝）→ 不派发提交事件，开关保持可改回；
 * - 菜单组件随切页卸载期间新一轮被受理（排队消息 flush 场景）→
 *   重挂载后未提交的「打开」已被转正锁死（模块级监听兜底清空 pending）。
 * 依赖先运行 `npm run build:ui`。
 */
const fs = require('fs'), path = require('path'), os = require('os');
const { startUiTestServer } = require('./ui_test_server');

function loadPuppeteer() {
  try { return require('puppeteer-core'); } catch { /* fall through */ }
  const npx = path.join(os.homedir(), '.npm', '_npx');
  if (fs.existsSync(npx)) for (const d of fs.readdirSync(npx)) {
    const p = path.join(npx, d, 'node_modules', 'puppeteer-core');
    if (fs.existsSync(p)) try { return require(p); } catch { /* next */ }
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
  path.join(process.env['ProgramFiles(x86)'] || 'C:\\Program Files (x86)', 'Google', 'Chrome', 'Application', 'msedge.exe'),
].filter(Boolean);
const CHROME = process.env.CHROME ||
  chromeCandidates.find(fs.existsSync);
if (!CHROME) { console.error('SKIP: 未找到 chromium/chrome'); process.exit(2); }
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-pending-enable-'));

function injectSource() {
  return `(function(){
    const state=window.__PENDING_ENABLE_TEST__={calls:[],disabled:['visualizer','doc-writer'],committedEvents:0,chatShouldFail:false,chatDoneScheduled:false};
    window.addEventListener('pinvou:chat-round-committed',()=>{state.committedEvents+=1;});
    function record(cmd,args){state.calls.push({cmd,args:args||{}});}
    // 真实事件注册表:mock 后端在 chat 受理后补发 chat:done,驱动 bridge 复位
    // busy(chat-events.js 在 chat:done 里 state.busy=false),排队/多轮发送才走得通。
    const listeners=new Map();
    function emitTauri(name,payload){ (listeners.get(name)||[]).forEach(fn=>{try{fn({event:name,payload:payload});}catch(_){}}); }
    window.__EMIT_TAURI__=emitTauri;
    const session={id:'s1',title:'S1',created_at:1,updated_at:1};
    function invoke(cmd,args){
      record(cmd,args);
      switch(cmd){
        case 'chat':
          if(state.chatShouldFail) return Promise.reject(new Error('mock_backend_unavailable'));
          // 受理后异步补一个 chat:done(真实后端语义:done 表示本轮跑完、busy 复位)。
          if(!state.chatDoneScheduled){
            state.chatDoneScheduled=true;
            Promise.resolve().then(()=>{state.chatDoneScheduled=false;emitTauri('chat:done',{session_id:(args&&args.sessionId)||'s1'});});
          }
          return Promise.resolve(null);
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
          {id:'doc-writer',title:'文档撰写',description:'撰写文档',installed:true,user_uploaded:false},
        ]);
        case 'get_disabled_connectors': return Promise.resolve(state.disabled);
        case 'set_disabled_connectors': state.disabled=(args&&args.connectorIds)||[]; return Promise.resolve(null);
        case 'get_disabled_skills': return Promise.resolve([]);
        case 'get_bundle_visibility': return Promise.resolve([]);
        case 'feishu_skills_state': case 'wecom_skills_state': case 'dingtalk_skills_state': case 'tmeet_skills_state': return Promise.resolve({connected:false,enabled:true});
        default: return Promise.resolve(null);
      }
    }
    window.__TAURI__={core:{invoke},event:{listen(name,fn){const arr=listeners.get(name)||[];arr.push(fn);listeners.set(name,arr);return Promise.resolve(function(){const cur=listeners.get(name)||[];const i=cur.indexOf(fn);if(i>=0){cur.splice(i,1);listeners.set(name,cur);}});},emit(){return Promise.resolve();}},
      window:{getCurrentWindow(){return {minimize(){},maximize(){},close(){},toggleMaximize(){},isMaximized(){return Promise.resolve(false);},onResized(){return Promise.resolve(function(){});},startDragging(){}};}},
      dialog:{open(){return Promise.resolve(null);}}};
  })();`;
}

const sleep = ms => new Promise(r => { setTimeout(r, ms); });

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
  const readSwitch = () => page.evaluate(() => {
    const btn = document.querySelector('button[aria-label="visualizer"]');
    return btn ? { disabled: btn.disabled, on: btn.className.includes('bg-[#34C759]') } : null;
  });
  const openMenu = async () => { await page.evaluate(() => document.querySelector('button[title="工具"]').click()); await sleep(300); };
  const closeMenu = async () => { await page.evaluate(() => document.querySelector('button[title="工具"]').click()); await sleep(200); };

  // 建立活动会话（只增不减守卫的前提）。
  await page.evaluate(() => window.TauriBridge.sessions.switchToSession('s1'));
  await sleep(600);
  const active = await page.evaluate(() => (window.TauriBridge.state.get('sessions') || {}).activeSessionId);
  rec('活动会话已建立', active === 's1', String(active));

  // ---- 场景一：会话中打开 → 未提交可改回；发送受理 → 锁死 ----
  await openMenu();
  const before = await readSwitch();
  rec('初始为关且开关可点（允许打开）', !!before && !before.on && !before.disabled, JSON.stringify(before));

  await page.evaluate(() => document.querySelector('button[aria-label="visualizer"]').click());
  await sleep(300);
  const afterEnable = await page.evaluate(() => {
    const btn = document.querySelector('button[aria-label="visualizer"]');
    const state = window.__PENDING_ENABLE_TEST__;
    return btn ? { disabled: btn.disabled, on: btn.className.includes('bg-[#34C759]'), persisted: !state.disabled.includes('visualizer') } : null;
  });
  rec('打开后未锁死（发送新一轮前可改回）', !!afterEnable && afterEnable.on && !afterEnable.disabled, JSON.stringify(afterEnable));
  rec('打开已持久化到禁用集之外', !!afterEnable && afterEnable.persisted);

  await closeMenu();
  await page.evaluate(() => window.TauriBridge.chat.sendMessage('hello'));
  await sleep(600);
  const committed = await page.evaluate(() => window.__PENDING_ENABLE_TEST__.committedEvents);
  rec('发送后轮次提交事件已派发', committed >= 1, `events=${committed}`);

  await openMenu();
  const afterSend = await readSwitch();
  rec('发送新一轮后开关锁死（只增不减）', !!afterSend && afterSend.on && afterSend.disabled, JSON.stringify(afterSend));
  await closeMenu();

  // ---- 场景二：发送失败（后端拒绝）→ 不派发提交事件，未提交的「打开」保持可改回 ----
  // 用第二个工具（doc-writer）：场景一的 visualizer 已随受理转正锁死，不可再动。
  await page.evaluate(() => { window.__PENDING_ENABLE_TEST__.chatShouldFail = true; });
  await openMenu();
  const docBefore = await page.evaluate(() => {
    const btn = document.querySelector('button[aria-label="doc-writer"]');
    return btn ? { disabled: btn.disabled, on: btn.className.includes('bg-[#34C759]') } : null;
  });
  rec('doc-writer 初始为关且开关可点', !!docBefore && !docBefore.on && !docBefore.disabled, JSON.stringify(docBefore));

  await page.evaluate(() => document.querySelector('button[aria-label="doc-writer"]').click()); // 打开 → pending
  await sleep(300);
  const docEnabled = await page.evaluate(() => {
    const btn = document.querySelector('button[aria-label="doc-writer"]');
    return btn ? { disabled: btn.disabled, on: btn.className.includes('bg-[#34C759]') } : null;
  });
  rec('打开后未锁死（发送新一轮前可改回）', !!docEnabled && docEnabled.on && !docEnabled.disabled, JSON.stringify(docEnabled));
  await closeMenu();

  const committedBeforeFail = await page.evaluate(() => window.__PENDING_ENABLE_TEST__.committedEvents);
  await page.evaluate(() => window.TauriBridge.chat.sendMessage('will-fail').then(
    () => { throw new Error('send should have failed'); },
    () => 'failed-as-expected' // 主路径失败现在向调用方 reject（评审修复：失败透传，输入框由调用方恢复）
  ));
  await sleep(600);
  const afterFail = await page.evaluate(() => window.__PENDING_ENABLE_TEST__.committedEvents);
  rec('发送失败不派发提交事件（failed sends never commit）', afterFail === committedBeforeFail, `before=${committedBeforeFail} after=${afterFail}`);
  await openMenu();
  const stillPending = await page.evaluate(() => {
    const btn = document.querySelector('button[aria-label="doc-writer"]');
    return btn ? { disabled: btn.disabled, on: btn.className.includes('bg-[#34C759]') } : null;
  });
  rec('发送失败后开关仍未锁死（可改回）', !!stillPending && stillPending.on && !stillPending.disabled, JSON.stringify(stillPending));
  await page.evaluate(() => document.querySelector('button[aria-label="doc-writer"]').click()); // 改回（关）→ pending 撤销
  await sleep(300);
  const docReverted = await page.evaluate(() => {
    const btn = document.querySelector('button[aria-label="doc-writer"]');
    const state = window.__PENDING_ENABLE_TEST__;
    return btn ? { disabled: btn.disabled, on: btn.className.includes('bg-[#34C759]'), persisted: state.disabled.includes('doc-writer') } : null;
  });
  rec('失败后改回成功且已持久化回禁用集', !!docReverted && !docReverted.on && !docReverted.disabled && docReverted.persisted, JSON.stringify(docReverted));
  await closeMenu();
  await page.evaluate(() => { window.__PENDING_ENABLE_TEST__.chatShouldFail = false; });

  // ---- 场景三：菜单组件随切页卸载期间新一轮被受理 → 重挂载后已锁死 ----
  // 用户打开开关（pending）→ 切到设置页（ChatView 连带菜单卸载、组件级监听移除）
  // → 后台发送被受理（commit 事件落在无组件监听的 window 上）→ 切回聊天页
  // → 重挂载后开关必须已锁死（模块级监听已清空 pending）。
  await openMenu();
  await page.evaluate(() => document.querySelector('button[aria-label="doc-writer"]').click());
  await sleep(300);
  await closeMenu();
  await page.evaluate(() => [...document.querySelectorAll('[data-testid="nav-settings"]')].pop().click());
  await sleep(600);
  const menuGone = await page.evaluate(() => !document.querySelector('button[aria-label="doc-writer"]'));
  rec('设置页下聊天组件已卸载（监听不在场）', menuGone);
  await page.evaluate(() => window.TauriBridge.chat.sendMessage('committed-while-away'));
  await sleep(600);
  const awayCommitted = await page.evaluate(() => window.__PENDING_ENABLE_TEST__.committedEvents);
  rec('卸载期间受理的轮次已派发提交事件', awayCommitted >= 2, `events=${awayCommitted}`);
  await page.evaluate(() => document.querySelector('[data-testid="settings-close"]').click());
  await sleep(600);
  await openMenu();
  const afterRemount = await page.evaluate(() => {
    const btn = document.querySelector('button[aria-label="doc-writer"]');
    return btn ? { disabled: btn.disabled, on: btn.className.includes('bg-[#34C759]') } : null;
  });
  rec('重挂载后未提交的「打开」已转正锁死（组件不在场也不漏清）', !!afterRemount && afterRemount.on && afterRemount.disabled, JSON.stringify(afterRemount));
  await closeMenu();

  rec('页面无未处理 JavaScript 异常', errors.length === 0, errors.slice(0, 2).join(' | '));

  await browser.close();
  fs.rmSync(PROFILE, { recursive: true, force: true });
  const failed = results.filter(r => !r.pass).length;
  console.log(failed ? `\n❌ ${failed}/${results.length} FAILED` : `\n✅ ALL ${results.length} PASS`);
  process.exit(failed ? 1 : 0);
// eslint-disable-next-line unicorn/prefer-top-level-await -- smoke script keeps its existing async main() structure
})().catch(e => {
  try { fs.rmSync(PROFILE, { recursive: true, force: true }); } catch { /* profile dir already gone */ }
  console.error('FATAL', e.stack || e);
  process.exit(1);
});
