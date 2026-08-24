#!/usr/bin/env node
/**
 * detached 启动冒烟：分别加载 session/codex-session/monitor 独立窗口，
 * 断言 ① 只渲染撕离面板 ② 普通与 Coding session 还原目标历史
 * ③ Coding session 接收实时事件 ④ monitor 能渲染实时快照。
 * 用 document.body.innerText(只含渲染出的可见文本)判断，避免 page.content() 把 <script> 里的 dict 字面量算进去。
 * 用法：node pinvou3-app/tests/detached_boot_smoke.js   (PASS→0 / FAIL→1 / 缺依赖→2)
 */
const fs = require('fs'), path = require('path'), os = require('os');
const { startUiTestServer } = require('./ui_test_server');
function loadPuppeteer() {
  try { return require('puppeteer-core'); } catch { /* fall through */ }
  const npx = path.join(os.homedir(), '.npm', '_npx');
  if (fs.existsSync(npx)) for (const d of fs.readdirSync(npx)) {
    const p = path.join(npx, d, 'node_modules', 'puppeteer-core');
    if (fs.existsSync(p)) { try { return require(p); } catch { /* next */ } }
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
    var listeners = {};
    window.__DETACHED_INVOKES__ = [];
    window.__emitDetachedTestEvent = function(name, payload){
      (listeners[name] || []).slice().forEach(function(handler){ handler({payload:payload}); });
    };
    window.__TAURI__ = { core: { invoke: async(cmd, args)=>{
      window.__DETACHED_INVOKES__.push(cmd);
      if (cmd === 'get_platform_capabilities') return {codexAcpSupported:true};
      if (cmd === 'get_settings') return { theme:'genesis', language:'zh-Hans' };
      if (cmd === 'get_selected_pet') return 'lingling';
      if (cmd === 'get_effective_model_config') return {};
      if (cmd === 'get_app_version') return '0.7.6-test';
      if (cmd === 'list_models' || cmd === 'list_archived_sessions' || cmd === 'list_personas') return [];
      if (cmd === 'list_sessions') return [{ id:sessionId, title:'分离测试会话', updated_at:'2026-08-07T00:00:00Z', message_count:1 }];
      if (cmd === 'list_codex_acp_sessions') return [
        { id:'detached-codex-42', title:'Coding分离测试会话', agent_id:'codex', agent_name:'Codex', workspace_kind:'temporary', workspace_path:'', workspace_available:true, updated_at:'2026-08-07T00:00:00Z' },
        { id:'detached-native-42', title:'品悟Coding分离测试会话', agent_id:'pinvou', agent_name:'品悟', workspace_kind:'temporary', workspace_path:'', workspace_available:true, updated_at:'2026-08-07T00:00:00Z' }
      ];
      if (cmd === 'load_session' && args && args.id === 'detached-native-42') return { metadata:{ id:'detached-native-42', title:'品悟Coding分离测试会话' }, messages:[{ role:'user', content:[{ type:'text', text:'DETACHED_NATIVE_HISTORY_OK' }] },{ role:'assistant', content:[{ type:'text', text:'DETACHED_NATIVE_REPLY_OK' }] }], artifacts:[] };
      if (cmd === 'load_session') return { metadata:{ id:sessionId, title:'分离测试会话' }, messages:[{ role:'user', content:[{ type:'text', text:'DETACHED_SESSION_HISTORY_OK' }] }], artifacts:[] };
      if (cmd === 'list_acp_agents') return [{agent_id:'codex',agent_name:'Codex'}];
      if (cmd === 'get_acp_agent_status') return {agent_id:'codex',installed:true,node_supported:true,authenticated:true};
      if (cmd === 'get_codex_acp_timeline') return [
        {version:1,sessionId:'detached-codex-42',turnId:'turn-1',seq:1,timestamp:'2026-08-07T00:00:00Z',event:{type:'user_message',data:{content:[{type:'text',text:'DETACHED_CODEX_HISTORY_OK'}]}}},
        {version:1,sessionId:'detached-codex-42',turnId:'turn-1',seq:2,timestamp:'2026-08-07T00:00:01Z',event:{type:'turn_started',data:{status:'running'}}}
      ];
      if (cmd === 'get_codex_acp_pending_permissions' || cmd === 'get_codex_acp_pending_elicitations') return [];
      if (cmd === 'get_codex_acp_session_info') return {session_id:'detached-codex-42',current_model_id:null,models:[],modes:null,config_options:[],pending_permissions:[],pending_elicitations:[]};
      if (cmd === 'list_codex_workspace') return {entries:[]};
      if (cmd === 'get_codex_workspace_changes') return {changes:[]};
      if (cmd === 'get_session_timeline') return [];
      if (cmd === 'get_pending_user_inputs') return {pending:[],busy:false};
      if (cmd === 'get_session_model_id' || cmd === 'session_mounted_collection') return null;
      if (cmd === 'kb_model_status') return {installed:true};
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
    } }, event: { listen: async(name, handler)=>{
      (listeners[name] || (listeners[name] = [])).push(handler);
      return function(){ listeners[name] = (listeners[name] || []).filter(function(item){ return item !== handler; }); };
    }, emit: async()=>{} } };
  })();`;
}

// eslint-disable-next-line unicorn/prefer-top-level-await -- smoke 脚本既有 async main() 结构
(async () => {
  const { url } = await startUiTestServer();
  const browser = await puppeteer.launch({ executablePath: CHROME, headless: 'new',
    userDataDir: PROFILE, args: ['--no-sandbox'] });
  const page = await browser.newPage();
  page.on('console', message => {
    if (message.type() === 'error') console.error('CONSOLE:', message.text());
  });
  page.on('pageerror', error => console.error('PAGEERROR:', String(error)));
  await page.evaluateOnNewDocument(injectSource());
  await page.goto(url + '?detached=1&kind=session&id=detached-session-42', { waitUntil: 'networkidle0' });
  await new Promise(r => { setTimeout(r, 1500); }); // 等 babel 编译 + 首渲染

  const detachedFlag = await page.evaluate(() => window.__PINVOU_DETACHED__ === true);
  const text = await page.evaluate(() => document.body.innerText || '');

  let ok = true;
  if (!detachedFlag) { console.error('FAIL: __PINVOU_DETACHED__ 未置 true'); ok = false; }
  if (!/撕离窗口|对话/.test(text)) { console.error('FAIL: 未渲染撕离标题栏，innerText=', JSON.stringify(text.slice(0,120))); ok = false; }
  if (/新对话|近期/.test(text)) { console.error('FAIL: detached 模式仍渲染了侧边栏(出现 新对话/近期)'); ok = false; }

  await page.goto(url + '?detached=1&kind=session&id=detached-session-42', { waitUntil: 'networkidle0' });
  try {
    await page.waitForFunction(() => document.body.innerText.includes('DETACHED_SESSION_HISTORY_OK'), { timeout: 5000 });
  } catch {
    console.error('FAIL: detached session 未渲染目标会话历史'); ok = false;
  }
  const activeSessionId = await page.evaluate(() => window.TauriBridge.state.get('sessions').activeSessionId);
  if (activeSessionId !== 'detached-session-42') {
    console.error('FAIL: detached session 未绑定目标 id，实际:', activeSessionId); ok = false;
  }

  await page.goto(url + '?detached=1&kind=codex-session&id=detached-codex-42', { waitUntil: 'networkidle0' });
  try {
    await page.waitForFunction(() => document.body.innerText.includes('DETACHED_CODEX_HISTORY_OK'), { timeout: 5000 });
  } catch {
    const diagnostic = await page.evaluate(() => ({text:document.body.innerText, invokes:window.__DETACHED_INVOKES__}));
    console.error('FAIL: detached Coding session 未渲染目标会话历史', JSON.stringify(diagnostic)); ok = false;
  }
  await page.evaluate(() => window.__emitDetachedTestEvent('acp:event', {
    version:1, sessionId:'detached-codex-42', turnId:'turn-1', seq:3,
    timestamp:'2026-08-07T00:00:02Z',
    event:{type:'agent_message_chunk',data:{update:{content:{type:'text',text:'DETACHED_CODEX_STREAM_OK'}}}}
  }));
  try {
    await page.waitForFunction(() => document.body.innerText.includes('DETACHED_CODEX_STREAM_OK'), { timeout: 5000 });
  } catch {
    console.error('FAIL: detached Coding session 未接收实时 ACP 事件'); ok = false;
  }

  await page.goto(url + '?detached=1&kind=codex-session&id=detached-native-42', { waitUntil: 'networkidle0' });
  try {
    await page.waitForFunction(() => document.body.innerText.includes('DETACHED_NATIVE_HISTORY_OK')
      && document.body.innerText.includes('DETACHED_NATIVE_REPLY_OK'), { timeout: 5000 });
  } catch {
    console.error('FAIL: detached 品悟 Coding session 未还原目标会话历史'); ok = false;
  }
  await page.evaluate(() => {
    window.__emitDetachedTestEvent('chat:turn_started', {session_id:'detached-native-42',turn_id:'native-turn-2'});
    window.__emitDetachedTestEvent('chat:delta', {session_id:'detached-native-42',text:'DETACHED_NATIVE_STREAM_OK'});
  });
  try {
    await page.waitForFunction(() => document.body.innerText.includes('DETACHED_NATIVE_STREAM_OK'), { timeout: 5000 });
  } catch {
    console.error('FAIL: detached 品悟 Coding session 未接收实时 chat 事件'); ok = false;
  }

  await page.goto(url + '?detached=1&kind=monitor', { waitUntil: 'networkidle0' });
  try {
    await page.waitForFunction(() => document.body.innerText.includes('DETACHED-MONITOR-CPU')
      && document.body.innerText.includes('DETACHED_MONITOR_OK'), { timeout: 5000 });
  } catch {
    console.error('FAIL: detached monitor 未渲染系统状态快照'); ok = false;
  }

  await browser.close(); fs.rmSync(PROFILE, { recursive: true, force: true });
  if (ok) { console.log('PASS: detached session/ACP及品悟Coding/monitor 启动与状态投影正常'); process.exit(0); }
  process.exit(1);
})();
