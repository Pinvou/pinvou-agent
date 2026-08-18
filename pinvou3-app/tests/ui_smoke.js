#!/usr/bin/env node
/**
 * pinvou3 前端 UI 冒烟回归测试 — headless chromium + mock TauriBridge,加载真 src/index.html。
 * 覆盖易回归的前端通路:
 *   ① 启动保持在草稿页(activeSessionId=null)。
 *   ② 工具商店渲染出 Obsidian 与钉钉连接器卡。
 *   ③ 聊天流产物卡(artifact_card)挂出「品/悟」召唤 pinvou 按钮。
 *   ④ 记忆候选事件渲染卡片，且确认/忽略/不再提示分别调用正确后端命令。
 *   ⑤ session 内未发送的 composer 草稿在跳转其他页面后仍能恢复。
 * 依赖:puppeteer-core(自动从 node_modules / ~/.npm/_npx 发现)+ 系统 chromium(或 env CHROME 指定)。
 * 用法:node pinvou3-app/tests/ui_smoke.js   (全 PASS → exit 0,任一 FAIL → exit 1,缺依赖 → exit 2)
 */
const fs = require('fs'), path = require('path'), os = require('os');
const { startUiTestServer } = require('./ui_test_server');

function loadPuppeteer() {
  try { return require('puppeteer-core'); } catch (e) { /* fall through */ }
  const npx = path.join(os.homedir(), '.npm', '_npx');
  if (fs.existsSync(npx)) {
    for (const d of fs.readdirSync(npx)) {
      const p = path.join(npx, d, 'node_modules', 'puppeteer-core');
      if (fs.existsSync(p)) { try { return require(p); } catch (e) { /* next */ } }
    }
  }
  console.error('SKIP: 找不到 puppeteer-core。装一个再跑:  npm i -D puppeteer-core   (或  npx -y puppeteer-core)');
  process.exit(2);
}
const puppeteer = loadPuppeteer();
const CHROME = process.env.CHROME ||
  [
    'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
    'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
    'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
    'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
    '/snap/bin/chromium',
    '/usr/bin/chromium',
    '/usr/bin/chromium-browser',
    '/usr/bin/google-chrome',
    '/usr/bin/google-chrome-stable',
  ].find(p => fs.existsSync(p));
if (!CHROME) { console.error('SKIP: 未找到 chromium/chrome,可用 env CHROME=/path/to/chromium 指定'); process.exit(2); }
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-smoke-'));

// mock TauriBridge 会话同时覆盖 write_file、MCP producer 和含 path 的
// exec_shell 诊断结果，验证只有真实 producer 触发 artifact。
function injectSource() {
  return `(function(){
    window.__TAURI_EVENT_HANDLERS__={};
    window.__TAURI_INVOKES__=[];
    window.__KB_MODEL_STATUS__={installed:true,ready:true,loading:false};
    window.__KB_MODEL_DOWNLOAD_ARGS__=[];
    let SESSIONS=[
      {id:'s-pinned-old',title:'置顶旧会话',created_at:'2026-06-01T08:00:00Z',updated_at:'2026-06-01T08:00:00Z',pinned:true,pinned_at:'2026-07-20T08:00:00Z'},
      {id:'s-attachment',title:'看看这个\\n\\n📎 PINV',title_attachment_names:['PINVOU-M0-开源决策基线.md'],created_at:Date.now()-2000,updated_at:Date.now()-2000},
      {id:'s1',title:'第三季度财报分析',created_at:Date.now()-1000,updated_at:Date.now()}
    ];
    let CODEX_SESSIONS=[{id:'codex-1',agent_id:'codex',title:'Codex回归会话',created_at:new Date(Date.now()-1000).toISOString(),updated_at:new Date().toISOString(),workspace_kind:'temporary',workspace_path:''}];
    const LONG_CODEX_COMMAND='overflow-marker-'+('x'.repeat(1200));
    let ARCHIVED_SESSIONS=[];
    let MOUNTED_COLLECTIONS=[];
    let MOUNTED_COLLECTIONS_REVISION=0;
    window.__SHELL_JOBS__=[{
      id:'task-history-done',job_id:'history-1',command:'history-shell',cwd:'C:/tmp',status:'Completed',
      exit_code:0,elapsed_ms:20,stdout_tail:'history output',stderr_tail:'',stdout_len:14,stderr_len:0,
      stdin_available:false,stale:false,linked_task_id:null,
    }];
    window.__CANCEL_SHELL_ARGS__=null;
    const CONV={s1:{metadata:{id:'s1'},artifacts:['/home/x/会议纪要.md'],messages:[
      {role:'user',content:[{type:'text',text:'整理纪要'}]},
      {role:'assistant',content:[{type:'text',text:'已生成会议纪要。'},{type:'tool_use',id:'t1',name:'write_file',input:{path:'/home/x/会议纪要.md',content:'# 会议纪要'}}]},
      {role:'user',content:[{type:'tool_result',tool_use_id:'t1',content:'written'}]},
      {role:'assistant',content:[{type:'tool_use',id:'t-shell',name:'exec_shell',input:{command:'python validator.py --json'}}]},
      {role:'user',content:[{type:'tool_result',tool_use_id:'t-shell',content:'{"ok":true,"path":"/home/x/validator-fake.html"}'}]},
      {role:'assistant',content:[{type:'tool_use',id:'t-shell-history',name:'exec_shell',input:{command:'history-shell'}}]},
      {role:'user',content:[{type:'tool_result',tool_use_id:'t-shell-history',content:'history output'}]},
      {role:'assistant',content:[{type:'tool_use',id:'t-mcp',name:'mcp_pptx_make_pptx',input:{title:'季度报告'}}]},
      {role:'user',content:[{type:'tool_result',tool_use_id:'t-mcp',content:'{"path":"/home/x/季度报告.pptx"}'}]},
      {role:'assistant',content:[{type:'text',text:'\\u0060\\u0060\\u0060json\\n{"name":"Reviewer\\\'s Agent","body":"hidden-prompt","description":"It\\\'s a highlighted JSON card"}\\n\\u0060\\u0060\\u0060'}]},
      {role:'assistant',content:[{type:'text',text:'\\u0060\\u0060\\u0060card-question\\n{"question":"继续执行？","options":["继续","取消"]}\\n\\u0060\\u0060\\u0060'}]}]}};
    function invoke(cmd,args){
      window.__TAURI_INVOKES__.push({cmd:cmd,args:args||{}});
      switch(cmd){
        case 'get_settings': return Promise.resolve({theme:'liquid-light',language:'zh-Hans'});
        case 'get_platform_capabilities': return Promise.resolve({codexAcpSupported:true});
        case 'get_effective_model_config': return Promise.resolve({model:'qwen36_35b_256k',base_url:'http://127.0.0.1:8000/v1',api_key_set:false});
        case 'list_sessions': return Promise.resolve(SESSIONS);
        case 'list_codex_acp_sessions': return Promise.resolve(CODEX_SESSIONS);
        case 'get_codex_acp_status': return Promise.resolve({installed:false,node_supported:false,authenticated:false});
        case 'get_acp_agent_status': return Promise.resolve({agent_id:args.agentId||'codex',installed:true,node_supported:true,authenticated:true});
        case 'get_codex_acp_session_info': return Promise.resolve({session_id:args.sessionId,models:[],current_model_id:'',modes:null,config_options:[]});
        case 'get_codex_acp_timeline': return Promise.resolve([
          {version:1,sessionId:'codex-1',turnId:'copy-turn',seq:1,timestamp:'2026-08-04T01:00:00Z',event:{type:'user_message',data:{content:[{type:'text',text:'Test copy layout'}]}}},
          {version:1,sessionId:'codex-1',turnId:'copy-turn',seq:2,timestamp:'2026-08-04T01:00:01Z',event:{type:'turn_started',data:{status:'running'}}},
          {version:1,sessionId:'codex-1',turnId:'copy-turn',seq:3,timestamp:'2026-08-04T01:00:02Z',event:{type:'agent_message_chunk',data:{update:{content:{type:'text',text:'Codex copy layout'}}}}},
          {version:1,sessionId:'codex-1',turnId:'copy-turn',seq:4,timestamp:'2026-08-04T01:00:03Z',event:{type:'turn_completed',data:{status:'Completed',error:null}}},
          {version:1,sessionId:'codex-1',turnId:'overflow-turn',seq:10,timestamp:'2026-08-04T01:01:00Z',event:{type:'user_message',data:{content:[{type:'text',text:'Test streaming overflow'}]}}},
          {version:1,sessionId:'codex-1',turnId:'overflow-turn',seq:11,timestamp:'2026-08-04T01:01:01Z',event:{type:'turn_started',data:{status:'running'}}},
          {version:1,sessionId:'codex-1',turnId:'overflow-turn',seq:12,timestamp:'2026-08-04T01:01:02Z',event:{type:'agent_thought_chunk',data:{update:{content:{type:'text',text:'reasoning-marker-'+('r'.repeat(1200))}}}}},
          {version:1,sessionId:'codex-1',turnId:'overflow-turn',seq:13,timestamp:'2026-08-04T01:01:03Z',event:{type:'plan',data:{update:{entries:[{content:'plan-marker-'+('p'.repeat(1200)),status:'in_progress'}]}}}},
          {version:1,sessionId:'codex-1',turnId:'overflow-turn',seq:14,timestamp:'2026-08-04T01:01:04Z',event:{type:'tool_call',data:{update:{toolCallId:'overflow-tool',title:LONG_CODEX_COMMAND,kind:'execute',status:'in_progress',rawInput:{command:LONG_CODEX_COMMAND,cwd:'C:/tmp'}}}}},
        ]);
        case 'get_codex_acp_pending_permissions': return Promise.resolve([]);
        case 'get_codex_acp_pending_elicitations': return Promise.resolve([]);
        case 'list_codex_workspace': return Promise.resolve({entries:[]});
        case 'get_codex_workspace_changes': return Promise.resolve({changes:[]});
        case 'list_archived_sessions': return Promise.resolve(ARCHIVED_SESSIONS);
        case 'get_super_permission_status': return Promise.resolve(false);
        case 'list_personas': return Promise.resolve([]);
        case 'get_backend_status':
          if(window.__PAUSE_BACKEND_STATUS_POLL__) return new Promise(function(){});
          return Promise.resolve({online:true,ok:true,status:'online',model:'qwen36_35b_256k'});
        case 'get_memory_overview': return Promise.resolve({profile:null,preferences:[],work_context:[],current_focus:[],recent_activity:[],recent_work:[],pending:[],never:[],runtime:null,snapshot_path:''});
        case 'confirm_pending_memory': return Promise.resolve({value:true});
        case 'ignore_pending_memory': return Promise.resolve({value:true});
        case 'never_pending_memory': return Promise.resolve({value:true});
        case 'check_for_update': return Promise.resolve({available:false});
        case 'check_dependencies': return Promise.resolve([]);
        case 'list_marketplace_tools': return Promise.resolve([]);
        case 'create_session': return Promise.resolve({id:'s-new',metadata:{id:'s-new'}});
        case 'set_session_archived':
          if (args && args.archived) {
            const session = SESSIONS.find(function(s){ return s.id === args.id; }) || CODEX_SESSIONS.find(function(s){ return s.id === args.id; }) || { id: args.id, title: '第三季度财报分析', created_at: Date.now()-1000, updated_at: Date.now() };
            SESSIONS = SESSIONS.filter(function(s){ return s.id !== args.id; });
            CODEX_SESSIONS = CODEX_SESSIONS.filter(function(s){ return s.id !== args.id; });
            ARCHIVED_SESSIONS = [Object.assign({}, session, { archived_at: '2026-07-21T10:00:00Z' })].concat(ARCHIVED_SESSIONS.filter(function(s){ return s.id !== args.id; }));
          } else {
            const archived = ARCHIVED_SESSIONS.find(function(s){ return s.id === args.id; });
            ARCHIVED_SESSIONS = ARCHIVED_SESSIONS.filter(function(s){ return s.id !== args.id; });
            if (archived) SESSIONS = [archived].concat(SESSIONS);
          }
          return Promise.resolve(null);
        case 'set_plan_mode_next': return Promise.resolve({mode:'plan',plan_phase:'planning'});
        case 'exit_plan_to_yolo': return Promise.resolve({mode:'yolo',plan_phase:'none'});
        case 'get_mode_state': return Promise.resolve({mode:'yolo',plan_phase:'none'});
        case 'get_active_persona': return Promise.resolve(null);
        case 'session_mounted_collections_snapshot': return Promise.resolve({revision:MOUNTED_COLLECTIONS_REVISION,collections:MOUNTED_COLLECTIONS});
        case 'session_mounted_collections': return Promise.resolve(MOUNTED_COLLECTIONS);
        case 'session_mounted_collection': return Promise.resolve(MOUNTED_COLLECTIONS.find(function(entry){ return entry.enabled; })?.collectionId || null);
        case 'session_set_mounted_collections': MOUNTED_COLLECTIONS=(args.collections||[]).map(function(entry){ return {collectionId:entry.collectionId,enabled:entry.enabled!==false}; }); MOUNTED_COLLECTIONS_REVISION+=1; return Promise.resolve(MOUNTED_COLLECTIONS);
        case 'session_add_mounted_collection': {
          const entry=MOUNTED_COLLECTIONS.find(function(item){return item.collectionId===args.collectionId;});
          if(entry) entry.enabled=true; else MOUNTED_COLLECTIONS.push({collectionId:args.collectionId,enabled:true});
          MOUNTED_COLLECTIONS_REVISION+=1;
          return Promise.resolve({revision:MOUNTED_COLLECTIONS_REVISION,collections:MOUNTED_COLLECTIONS});
        }
        case 'session_set_mounted_collection_enabled': {
          const entry=MOUNTED_COLLECTIONS.find(function(item){return item.collectionId===args.collectionId;});
          if(entry) entry.enabled=args.enabled!==false;
          MOUNTED_COLLECTIONS_REVISION+=1;
          return Promise.resolve({revision:MOUNTED_COLLECTIONS_REVISION,collections:MOUNTED_COLLECTIONS});
        }
        case 'session_remove_mounted_collection': MOUNTED_COLLECTIONS=MOUNTED_COLLECTIONS.filter(function(item){return item.collectionId!==args.collectionId;}); MOUNTED_COLLECTIONS_REVISION+=1; return Promise.resolve({revision:MOUNTED_COLLECTIONS_REVISION,collections:MOUNTED_COLLECTIONS});
        case 'session_unmount_collection': MOUNTED_COLLECTIONS=[]; MOUNTED_COLLECTIONS_REVISION+=1; return Promise.resolve({revision:MOUNTED_COLLECTIONS_REVISION,collections:MOUNTED_COLLECTIONS});
        case 'kb_model_status': return Promise.resolve(window.__KB_MODEL_STATUS__);
        case 'kb_model_download':
          window.__KB_MODEL_DOWNLOAD_ARGS__.push(args||{});
          if(!(args&&args.repair)) return Promise.reject(new Error('mock embedding load failed'));
          window.__KB_MODEL_STATUS__={installed:true,ready:true,loading:false,failed:false,error:null};
          return Promise.resolve(window.__KB_MODEL_STATUS__);
        case 'kb_collection_list': return Promise.resolve([
          {id:7,name:'项目资料',docCount:3},
          {id:8,name:'团队规范',docCount:5},
        ]);
        case 'list_workspace_files': return Promise.resolve([]);
        case 'get_session_persona_events': return Promise.resolve([]);
        case 'get_session_pinvou_reviews': return Promise.resolve([]);
        case 'summon_pinvou': return Promise.resolve({personas:[{id:'travel',label:'旅行规划',primary:true}],alternates:['budget'],trace:'看了下，有几点确认',recommendations:[{topic:'预算',pick:'中档',why:'稳妥'}],issues:[{severity:'high',kind:'quality',persona:'travel',text:'日期冲突',suggestion:'对齐'}],coverage:[],framework:[],risk:'medium',confidence:0.8});
        case 'load_session': return Promise.resolve(CONV[args&&args.id]||{metadata:{id:'x'},messages:[],artifacts:[]});
        case 'get_session_timeline': return Promise.resolve([
          {turn_id:'copy-deepseek',event:'user_start',timestamp:1000,ui_turn_index:0},
          {turn_id:'copy-deepseek',event:'assistant_done',timestamp:3000,status:'Completed',usage:{input_tokens:12,output_tokens:4}},
        ]);
        case 'list_shell_tasks': return Promise.resolve(window.__SHELL_JOBS__);
        case 'cancel_shell_task':
          window.__CANCEL_SHELL_ARGS__=args;
          if(window.__CANCEL_SHELL_ERROR__) return Promise.reject('kill failed');
          window.__SHELL_JOBS__=window.__SHELL_JOBS__.map(function(job){
            return job.id===args.taskId ? Object.assign({},job,{status:'Killed',exit_code:130}) : job;
          });
          return Promise.resolve({task_id:args.taskId,status:'Killed',exit_code:130,stdout:'',stderr:'',duration_ms:1});
        case 'artifact_info': return Promise.resolve({exists:true,kind:'md',size:2048,modified:1});
        case 'read_artifact_text': return Promise.resolve('# 会议纪要');
        case 'render_artifact_visual': return Promise.resolve({mode:'unsupported'});
        case 'detect_local_vllm_setup': {
          const engineState = window.__VLLM_STATE__ || 'stopped';
          return Promise.resolve(window.__VLLM_ELIGIBLE__ && engineState !== 'starting'
            ? {eligible:true,may_offer_setup:true,is_megacube:true,has_packages:true,vllm_online:false,engine_state:engineState,already_bootstrapped:false}
            : {eligible:false,may_offer_setup:!!window.__VLLM_ELIGIBLE__,is_megacube:engineState==='starting',has_packages:engineState==='starting',vllm_online:false,engine_state:engineState,already_bootstrapped:false});
        }
        case 'bootstrap_local_vllm': return new Promise(function(){}); // 永不 resolve,停在 bootstrapping 态供测步骤指示
        default: return Promise.resolve(null);
      }
    }
    window.__TAURI__={core:{invoke:invoke},event:{emit:function(){return Promise.resolve();},listen:function(name,handler){
      const handlers=window.__TAURI_EVENT_HANDLERS__[name]||(window.__TAURI_EVENT_HANDLERS__[name]=[]);
      handlers.push(handler);
      return Promise.resolve(function(){const i=handlers.indexOf(handler);if(i>=0)handlers.splice(i,1);});
    }},
      window:{getCurrentWindow:function(){return {minimize(){},maximize(){},close(){},toggleMaximize(){},isMaximized(){return Promise.resolve(false);},onResized(){return Promise.resolve(function(){});},startDragging(){}};}},
      dialog:{open:function(){return Promise.resolve(null);}}};
  })();`;
}

const sleep = ms => new Promise(r => setTimeout(r, ms));
async function clickText(page, t) {
  return page.evaluate((t) => {
    const els = [...document.querySelectorAll('span,div,button,a')].filter(el => (el.textContent || '').trim() === t);
    const el = els[els.length - 1];
    if (el) { el.scrollIntoView({ block: 'center' }); el.click(); return true; }
    return false;
  }, t);
}
async function expand(page) {
  return page.evaluate(() => {
    const hasVisibleHistory = [...document.querySelectorAll('span')]
      .some(node => (node.textContent || '').trim() === '第三季度财报分析' && node.getBoundingClientRect().left < 330);
    if (hasVisibleHistory) return true;
    const b = document.querySelector('[data-sidebar-toggle]') || document.querySelector('[title*="侧边栏"],[title*="展开"]');
    if (b) { b.click(); return true; }
    return false;
  });
}

(async () => {
  const { url: INDEX } = await startUiTestServer();
  const results = [];
  const rec = (name, pass, detail) => { results.push({ name, pass }); console.log(`${pass ? '✅' : '❌'} ${name}${detail ? '  ' + detail : ''}`); };

  const browser = await puppeteer.launch({ executablePath: CHROME, headless: 'new', args: ['--no-sandbox', '--disable-gpu', '--no-first-run', '--no-default-browser-check'], userDataDir: PROFILE });
  const page = await browser.newPage();
  const errs = [];
  page.on('pageerror', e => errs.push(e.stack || e.message));
  await page.evaluateOnNewDocument(injectSource());
  await page.setViewport({ width: 1440, height: 1000, deviceScaleFactor: 1 });
  await page.goto(INDEX, { waitUntil: 'networkidle0' });
  await page.waitForFunction(() => window.TauriBridge && document.body && document.body.innerText.includes('PINVOU'), { timeout: 20000 }).catch(() => {});
  await sleep(2000);

  // 入口能渲染 DOM 不代表样式加载成功：WebKit 若复用旧 index.html、CSS 404，
  // 所有旧 view 会以裸 DOM 一起铺开。直接检查 Tailwind 的关键计算样式。
  const visualShell = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="app-root"]');
    if (!root) return { found: false };
    const style = getComputedStyle(root);
    return {
      found: true,
      display: style.display,
      height: Math.round(root.getBoundingClientRect().height),
      viewportHeight: window.innerHeight,
      overflow: style.overflow,
      backgroundColor: style.backgroundColor,
    };
  });
  rec(
    '⓪ 前端样式完整加载（非裸 HTML）',
    visualShell.found
      && visualShell.display === 'flex'
      && Math.abs(visualShell.height - visualShell.viewportHeight) <= 2
      && visualShell.overflow === 'hidden'
      && visualShell.backgroundColor !== 'rgba(0, 0, 0, 0)',
    JSON.stringify(visualShell),
  );

  // Windows 平板尺寸会展示浮动语音按钮。用浏览器输入通道覆盖鼠标、触控笔与触摸，
  // 并动态验证 capture 丢失、pointercancel、窗口失焦后的视觉态和点击语义。
  await page.setViewport({ width: 1000, height: 800, deviceScaleFactor: 1 });
  await sleep(350);
  await page.evaluate(() => {
    const button = document.querySelector('[data-testid="floating-voice-button"]');
    window.__FLOATING_VOICE_COMPAT_CLICKS__ = 0;
    if (button) button.addEventListener('click', () => { window.__FLOATING_VOICE_COMPAT_CLICKS__ += 1; });
  });
  const floatingVoiceSnapshot = () => page.evaluate(() => {
    const button = document.querySelector('[data-testid="floating-voice-button"]');
    if (!button) return null;
    const rect = button.getBoundingClientRect();
    const wrapRect = button.parentElement.getBoundingClientRect();
    return {
      x: rect.left + rect.width / 2,
      y: rect.top + rect.height / 2,
      left: wrapRect.left,
      top: wrapRect.top,
      pressed: button.getAttribute('data-pressed'),
      voiceCalls: window.__TAURI_INVOKES__.filter(call => call.cmd === 'voice_asr_status').length,
      clicks: window.__FLOATING_VOICE_COMPAT_CLICKS__,
    };
  });
  const floatingVoiceStart = await floatingVoiceSnapshot();
  let floatingVoiceDrag = { found: false };
  if (floatingVoiceStart) {
    await page.mouse.move(floatingVoiceStart.x, floatingVoiceStart.y);
    await page.mouse.down();
    await page.mouse.move(floatingVoiceStart.x + 12, floatingVoiceStart.y);
    await sleep(60);
    const mouseDuring = await floatingVoiceSnapshot();
    await page.mouse.up();
    await sleep(60);
    const mouseAfter = await floatingVoiceSnapshot();

    const lostStart = mouseAfter;
    await page.evaluate(() => {
      const button = document.querySelector('[data-testid="floating-voice-button"]');
      window.__FLOATING_VOICE_POINTER_ID__ = null;
      button?.addEventListener('pointerdown', event => { window.__FLOATING_VOICE_POINTER_ID__ = event.pointerId; }, { once: true });
    });
    await page.mouse.move(lostStart.x, lostStart.y);
    await page.mouse.down();
    await page.mouse.move(lostStart.x + 12, lostStart.y);
    await page.evaluate(() => {
      const button = document.querySelector('[data-testid="floating-voice-button"]');
      const pointerId = window.__FLOATING_VOICE_POINTER_ID__;
      if (button && pointerId !== null && button.hasPointerCapture(pointerId)) button.releasePointerCapture(pointerId);
      if (button && pointerId !== null) {
        button.dispatchEvent(new PointerEvent('lostpointercapture', {
          bubbles: true, pointerId, pointerType: 'mouse', isPrimary: true,
        }));
      }
    });
    await sleep(30);
    const lostCapture = await floatingVoiceSnapshot();
    await page.mouse.up();
    await sleep(60);
    const lostAfter = await floatingVoiceSnapshot();

    const input = await page.createCDPSession();
    const penStart = lostAfter;
    await input.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: penStart.x, y: penStart.y, buttons: 0, pointerType: 'pen' });
    await input.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: penStart.x, y: penStart.y, button: 'left', buttons: 1, clickCount: 1, pointerType: 'pen' });
    const penPressed = await floatingVoiceSnapshot();
    await page.evaluate(() => {
      const button = document.querySelector('[data-testid="floating-voice-button"]');
      const rect = button.getBoundingClientRect();
      const init = {
        bubbles: true, pointerId: 302, pointerType: 'touch', isPrimary: true, button: 0,
        clientX: rect.left + rect.width / 2, clientY: rect.top + rect.height / 2,
      };
      button.dispatchEvent(new PointerEvent('pointerdown', { ...init, buttons: 1 }));
      button.dispatchEvent(new PointerEvent('pointerup', { ...init, buttons: 0 }));
    });
    await sleep(20);
    const penAfterTouch = await floatingVoiceSnapshot();
    await input.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: penStart.x + 12, y: penStart.y, buttons: 1, pointerType: 'pen' });
    const penDuring = await floatingVoiceSnapshot();
    await input.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: penStart.x + 12, y: penStart.y, button: 'left', buttons: 0, clickCount: 1, pointerType: 'pen' });
    await sleep(60);
    const penAfter = await floatingVoiceSnapshot();

    await page.evaluate(() => {
      const button = document.querySelector('[data-testid="floating-voice-button"]');
      const rect = button.getBoundingClientRect();
      button.dispatchEvent(new PointerEvent('pointerdown', {
        bubbles: true, pointerId: 301, pointerType: 'pen', isPrimary: false, button: 0, buttons: 1,
        clientX: rect.left + rect.width / 2, clientY: rect.top + rect.height / 2,
      }));
    });
    await sleep(20);
    const nonPrimaryPen = await floatingVoiceSnapshot();

    await page.evaluate(() => {
      const button = document.querySelector('[data-testid="floating-voice-button"]');
      const rect = button.getBoundingClientRect();
      const init = {
        bubbles: true, pointerType: 'touch', button: 0, buttons: 1,
        clientX: rect.left + rect.width / 2, clientY: rect.top + rect.height / 2,
      };
      button.dispatchEvent(new PointerEvent('pointerdown', { ...init, pointerId: 401, isPrimary: true }));
      button.dispatchEvent(new PointerEvent('pointerdown', { ...init, pointerId: 402, isPrimary: false }));
      button.dispatchEvent(new PointerEvent('pointerup', { ...init, pointerId: 402, isPrimary: false, buttons: 0 }));
    });
    await sleep(20);
    const multiTouchSecondaryEnded = await floatingVoiceSnapshot();
    await page.evaluate(() => {
      const button = document.querySelector('[data-testid="floating-voice-button"]');
      const rect = button.getBoundingClientRect();
      button.dispatchEvent(new PointerEvent('pointercancel', {
        bubbles: true, pointerId: 401, pointerType: 'touch', isPrimary: true, button: 0, buttons: 0,
        clientX: rect.left + rect.width / 2, clientY: rect.top + rect.height / 2,
      }));
    });
    await sleep(20);
    const multiTouchCancelled = await floatingVoiceSnapshot();

    const touchStart = penAfter;
    await input.send('Input.dispatchTouchEvent', {
      type: 'touchStart',
      touchPoints: [{ x: touchStart.x, y: touchStart.y, id: 41, radiusX: 1, radiusY: 1, force: 1 }],
    });
    await input.send('Input.dispatchTouchEvent', {
      type: 'touchMove',
      touchPoints: [{ x: touchStart.x + 12, y: touchStart.y, id: 41, radiusX: 1, radiusY: 1, force: 1 }],
    });
    const touchDuring = await floatingVoiceSnapshot();
    await input.send('Input.dispatchTouchEvent', { type: 'touchCancel', touchPoints: [] });
    await sleep(40);
    const touchCancelled = await floatingVoiceSnapshot();

    const composerBefore = touchCancelled;
    const composerCenter = await page.evaluate(() => {
      const button = document.querySelector('[data-testid="composer-voice-button"]');
      if (!button) return null;
      const rect = button.getBoundingClientRect();
      return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
    });
    if (composerCenter) await page.mouse.click(composerCenter.x, composerCenter.y);
    await sleep(80);
    const composerAfter = await floatingVoiceSnapshot();
    await page.evaluate(() => window.TauriBridge.voice.clearVoiceInput());
    await sleep(20);

    const blurStart = touchCancelled;
    await page.mouse.move(blurStart.x, blurStart.y);
    await page.mouse.down();
    await page.mouse.move(blurStart.x + 12, blurStart.y);
    await page.evaluate(() => window.dispatchEvent(new Event('blur')));
    await sleep(30);
    const blurred = await floatingVoiceSnapshot();
    await page.mouse.move(1, 1);
    await page.mouse.up();
    await sleep(40);
    const blurAfter = await floatingVoiceSnapshot();

    await page.evaluate(() => document.querySelector('[data-testid="floating-voice-button"]')?.focus());
    await page.keyboard.press('Enter');
    await sleep(80);
    const keyboardAfter = await floatingVoiceSnapshot();
    await page.evaluate(() => window.TauriBridge.voice.clearVoiceInput());

    floatingVoiceDrag = {
      found: true,
      mouseMovedImmediately: mouseDuring.pressed === 'true'
        && (Math.abs(mouseDuring.left - floatingVoiceStart.left) > 4 || Math.abs(mouseDuring.top - floatingVoiceStart.top) > 4),
      mouseCompatibleClickSuppressed: mouseAfter.pressed === 'false'
        && mouseAfter.clicks > floatingVoiceStart.clicks
        && mouseAfter.voiceCalls === floatingVoiceStart.voiceCalls,
      lostCaptureCleared: lostCapture.pressed === 'false',
      lostCaptureClickSuppressed: lostAfter.clicks > mouseAfter.clicks
        && lostAfter.voiceCalls === mouseAfter.voiceCalls,
      penPathPassed: penPressed.pressed === 'true'
        && penAfterTouch.pressed === 'true'
        && penDuring.pressed === 'true'
        && (Math.abs(penDuring.left - penStart.left) > 4 || Math.abs(penDuring.top - penStart.top) > 4)
        && penAfter.pressed === 'false'
        && penAfter.voiceCalls === lostAfter.voiceCalls,
      nonPrimaryPenIgnored: nonPrimaryPen.pressed === 'false',
      secondTouchIgnored: multiTouchSecondaryEnded.pressed === 'true' && multiTouchCancelled.pressed === 'false',
      touchCancelCleared: touchDuring.pressed === 'true'
        && touchCancelled.pressed === 'false'
        && touchCancelled.voiceCalls === penAfter.voiceCalls,
      composerClickWorked: !!composerCenter && composerAfter.voiceCalls > composerBefore.voiceCalls,
      blurCleared: blurred.pressed === 'false' && blurAfter.voiceCalls === composerAfter.voiceCalls,
      keyboardClickWorked: keyboardAfter.voiceCalls > blurAfter.voiceCalls,
    };
  }
  rec(
    '⓪b 浮动语音按钮 mouse/touch/pen 拖动及异常终止行为',
    floatingVoiceDrag.found
      && floatingVoiceDrag.mouseMovedImmediately
      && floatingVoiceDrag.mouseCompatibleClickSuppressed
      && floatingVoiceDrag.lostCaptureCleared
      && floatingVoiceDrag.lostCaptureClickSuppressed
      && floatingVoiceDrag.penPathPassed
      && floatingVoiceDrag.nonPrimaryPenIgnored
      && floatingVoiceDrag.secondTouchIgnored
      && floatingVoiceDrag.touchCancelCleared
      && floatingVoiceDrag.composerClickWorked
      && floatingVoiceDrag.blurCleared
      && floatingVoiceDrag.keyboardClickWorked,
    JSON.stringify(floatingVoiceDrag),
  );
  await page.setViewport({ width: 1440, height: 1000, deviceScaleFactor: 1 });
  await sleep(250);

  const markdownHighlight = await page.evaluate(() => {
    const render = window.PinvouMarkdownRenderer && window.PinvouMarkdownRenderer.renderMarkdown;
    if (typeof render !== 'function') return { found: false };

    const fixture = document.createElement('section');
    fixture.style.cssText = 'position:fixed;left:-10000px;top:0;width:720px;visibility:hidden;';
    document.body.appendChild(fixture);
    const markdown = [
      '```json',
      '{"enabled": true, "message": "hello"}',
      '```',
      '',
      '```diff',
      '-oldValue',
      '+newValue',
      '```',
    ].join('\n');

    function addSample(mode, structure) {
      const wrapper = document.createElement('div');
      let content;
      if (structure === 'nested') {
        wrapper.className = `${mode}-code`;
        content = document.createElement('div');
        content.className = 'msg-md';
        wrapper.appendChild(content);
      } else if (structure === 'persona') {
        content = wrapper;
        content.className = `persona-body ${mode}-code`;
      } else {
        content = wrapper;
        content.className = `msg-md ${mode}-code`;
      }
      content.innerHTML = render(markdown);
      fixture.appendChild(wrapper);
      // 生产 CSS 自 #166 起 rescope 为 `.dark .dark-code ...`（需 .dark 祖先），
      // 与应用真实主题结构一致：暗态由 <html class="dark"> 决定（main.jsx / DetachedShell.jsx）。
      // 暗色样本必须在采样期间给 documentElement 加 .dark，亮态必须确保无 .dark，
      // 否则 computed style 会算到错的分支。采样后还原，避免污染后续检查。
      const root = document.documentElement;
      const hadDark = root.classList.contains('dark');
      if (mode === 'dark') root.classList.add('dark'); else root.classList.remove('dark');
      const pre = content.querySelector('pre[data-language-id="json"]');
      const stringToken = pre && pre.querySelector('.hljs-string');
      const attrToken = pre && pre.querySelector('.hljs-attr');
      const addition = content.querySelector('.language-diff .hljs-addition');
      const result = {
        languageId: pre && pre.dataset.languageId,
        stringColor: stringToken && getComputedStyle(stringToken).color,
        attrColor: attrToken && getComputedStyle(attrToken).color,
        preBackground: pre && getComputedStyle(pre).backgroundColor,
        label: pre && getComputedStyle(pre, '::before').content,
        diffBackground: addition && getComputedStyle(addition).backgroundColor,
      };
      if (hadDark) root.classList.add('dark'); else root.classList.remove('dark');
      return result;
    }

    const lightNested = addSample('light', 'nested');
    const lightSame = addSample('light', 'same');
    const darkNested = addSample('dark', 'nested');
    const darkSame = addSample('dark', 'same');
    const darkPersona = addSample('dark', 'persona');

    const sanitized = document.createElement('div');
    sanitized.innerHTML = render([
      '<img src="x" onerror="window.__MARKDOWN_XSS__=true">',
      '<script>window.__MARKDOWN_XSS__=true</script>',
      '',
      '```json',
      '{"safe": true}',
      '```',
    ].join('\n'));
    const sanitizedPre = sanitized.querySelector('pre[data-language-id="json"]');
    const security = {
      noScriptElement: !sanitized.querySelector('script'),
      noEventAttribute: !sanitized.querySelector('img')?.hasAttribute('onerror'),
      noExecution: window.__MARKDOWN_XSS__ !== true,
      dataAttributePreserved: sanitizedPre?.dataset.languageId === 'json',
      highlightMarkupPreserved: !!sanitizedPre?.querySelector('.hljs-attr'),
    };
    fixture.remove();
    return { found: true, lightNested, lightSame, darkNested, darkSame, darkPersona, security };
  });
  const transparent = value => !value || value === 'rgba(0, 0, 0, 0)' || value === 'transparent';
  rec(
    'Markdown highlighting uses sanitized DOM and consistent computed themes',
    markdownHighlight.found
      && Object.values(markdownHighlight.security || {}).every(Boolean)
      && markdownHighlight.lightNested.languageId === 'json'
      && markdownHighlight.lightNested.stringColor === markdownHighlight.lightSame.stringColor
      && markdownHighlight.lightNested.attrColor === markdownHighlight.lightSame.attrColor
      && markdownHighlight.darkNested.stringColor === markdownHighlight.darkSame.stringColor
      && markdownHighlight.darkNested.attrColor === markdownHighlight.darkSame.attrColor
      && markdownHighlight.darkSame.stringColor === markdownHighlight.darkPersona.stringColor
      && markdownHighlight.darkSame.attrColor === markdownHighlight.darkPersona.attrColor
      && markdownHighlight.darkSame.stringColor !== markdownHighlight.lightSame.stringColor
      && markdownHighlight.darkSame.preBackground === markdownHighlight.darkPersona.preBackground
      && !transparent(markdownHighlight.darkSame.diffBackground)
      && !transparent(markdownHighlight.lightSame.diffBackground)
      && String(markdownHighlight.darkPersona.label).includes('JSON'),
    JSON.stringify(markdownHighlight),
  );

  // ① 启动落草稿页。
  const st = await page.evaluate(() => {
    const s = window.TauriBridge.state.getMany(['chat', 'vllm']);
    return { activeSessionId: s.activeSessionId };
  });
  rec('① 启动保持草稿页', st.activeSessionId == null, JSON.stringify(st));

  await expand(page); await sleep(300);
  const sidebarTaskShell = await page.evaluate(() => ({
    hasTaskList: document.body.innerText.includes('任务列表'),
    hasPinnedGroup: document.body.innerText.includes('置顶任务'),
    hasRegularGroup: /任务 \(\d+\)/.test(document.body.innerText),
    hasFilterButton: !!document.querySelector('[data-testid="sidebar-task-filter"]'),
  }));
  rec('①a 侧边栏任务合并为单一任务列表',
    sidebarTaskShell.hasTaskList && !sidebarTaskShell.hasPinnedGroup && !sidebarTaskShell.hasRegularGroup && sidebarTaskShell.hasFilterButton,
    JSON.stringify(sidebarTaskShell));
  const pinnedSidebarState = await page.evaluate(() => {
    const visible = (node) => {
      const rect = node.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    };
    const pinnedRows = [...document.querySelectorAll('[data-testid="regular-sidebar-item"]')]
      .filter(node => (node.textContent || '').includes('置顶旧会话') && node.getBoundingClientRect().left < 330 && visible(node));
    const todayLabel = [...document.querySelectorAll('span')]
      .find(node => /^今天 \(\d+\)$/.test((node.textContent || '').trim()) && node.getBoundingClientRect().left < 330 && visible(node));
    const pinnedRow = pinnedRows[0];
    return {
      count: pinnedRows.length,
      beforeToday: !!(pinnedRow && todayLabel && pinnedRow.getBoundingClientRect().top < todayLabel.getBoundingClientRect().top),
      pinVisible: !!(pinnedRow && [...pinnedRow.querySelectorAll('svg')].some(icon => icon.classList.contains('rotate-45'))),
    };
  });
  rec('①a-1 默认置顶会话跨日期分组上浮且不重复',
    pinnedSidebarState.count === 1 && pinnedSidebarState.beforeToday && pinnedSidebarState.pinVisible,
    JSON.stringify(pinnedSidebarState));
  const attachmentSidebarState = await page.evaluate(() => {
    const row = [...document.querySelectorAll('[data-testid="regular-sidebar-item"]')]
      .find(node => (node.textContent || '').includes('PINVOU-M0-开源决策基线.md'));
    return {
      exists: !!row,
      text: row ? row.textContent || '' : '',
      hasMarkdownIcon: !!(row && row.querySelector('svg path[fill="#42a5f5"]')),
    };
  });
  rec(
    '①a-2 附件会话标题隐藏协议符号并显示对应文件图标',
    attachmentSidebarState.exists
      && !attachmentSidebarState.text.includes('📎')
      && attachmentSidebarState.hasMarkdownIcon,
    JSON.stringify(attachmentSidebarState),
  );
  await page.click('[data-testid="sidebar-task-filter"]'); await sleep(200);
  const sidebarTaskFilterMenu = await page.evaluate(() => {
    const menu = document.querySelector('[data-testid="sidebar-task-filter-menu"]');
    const text = menu ? menu.textContent || '' : '';
    return {
      exists: !!menu,
      hasAll: text.includes('全部'),
      hasPinned: text.includes('置顶'),
      hasScheduled: text.includes('定时任务'),
      hasPinnedFirst: text.includes('置顶优先'),
      hasRecent: text.includes('最近更新'),
      hasCurrentChat: text.includes('当前会话'),
      hasRegularChat: text.includes('普通会话'),
    };
  });
  rec('①a-3 任务筛选弹层只保留有效筛选与排序项',
    sidebarTaskFilterMenu.exists && sidebarTaskFilterMenu.hasAll && sidebarTaskFilterMenu.hasPinned &&
    sidebarTaskFilterMenu.hasScheduled && sidebarTaskFilterMenu.hasPinnedFirst && sidebarTaskFilterMenu.hasRecent &&
    !sidebarTaskFilterMenu.hasCurrentChat && !sidebarTaskFilterMenu.hasRegularChat,
    JSON.stringify(sidebarTaskFilterMenu));
  await page.keyboard.press('Escape'); await sleep(200);

  // ①a-3 对话管理页必须按 Codex 会话类型路由，不能误走普通聊天 switchToSession。
  await clickText(page, '查看全部'); await sleep(500);
  const codexManagedOpen = await page.evaluate(() => {
    const label = [...document.querySelectorAll('span')]
      .find(node => (node.textContent || '').trim() === 'Codex回归会话' && node.getBoundingClientRect().left > 300);
    const row = label && label.closest('div[class*="cursor-pointer"]');
    if (!row) return { found: false };
    row.click();
    return { found: true };
  });
  await sleep(500);
  const codexManagedState = await page.evaluate(() => ({
    view: document.querySelector('[data-testid="app-root"]')?.getAttribute('data-current-view'),
    activeId: localStorage.getItem('pinvou_codex_active_session'),
  }));
  rec('①a-3 对话管理页正确打开 Codex 会话',
    codexManagedOpen.found && codexManagedState.view === 'codex' && codexManagedState.activeId === 'codex-1',
    JSON.stringify({ ...codexManagedOpen, ...codexManagedState }));

  const codexAssistantCopy = await page.evaluate(async () => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: async text => { window.__CODEX_ASSISTANT_COPY_TEXT__ = text; } },
    });
    const turn = [...document.querySelectorAll('section')]
      .find(node => node.innerText.includes('Codex copy layout'));
    const action = turn?.querySelector('[data-testid="assistant-message-actions"]');
    const button = action?.querySelector('[data-testid="assistant-message-copy"]');
    const footer = action?.closest('[data-testid="assistant-message-footer"]');
    const children = [...(footer?.children || [])];
    if (!button || children.length < 2) return { found: false, childCount: children.length };
    button.click();
    await new Promise(resolve => setTimeout(resolve, 50));
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: async () => { throw new Error('clipboard denied'); } },
    });
    document.execCommand = () => false;
    button.click();
    await new Promise(resolve => setTimeout(resolve, 50));
    const firstRect = children[0].getBoundingClientRect();
    return {
      found: true,
      copied: window.__CODEX_ASSISTANT_COPY_TEXT__ || '',
      failureFeedback: button.textContent.trim(),
      failureTitle: button.getAttribute('title') || '',
      childCount: children.length,
      sameRow: children.every(node => {
        const rect = node.getBoundingClientRect();
        return Math.abs((rect.top + rect.height / 2) - (firstRect.top + firstRect.height / 2)) < 2;
      }),
    };
  });
  rec('①a-3b Codex 复制操作与完成状态保持同一行',
    codexAssistantCopy.found && codexAssistantCopy.copied === 'Codex copy layout' &&
    codexAssistantCopy.failureFeedback === '复制失败' && codexAssistantCopy.failureTitle === '复制失败' &&
    codexAssistantCopy.sameRow,
    JSON.stringify(codexAssistantCopy));

  const codexStreamingOverflow = await page.evaluate(async () => {
    const turn = document.querySelector('[data-conversation-turn="overflow-turn"]');
    const summary = turn?.querySelector('[data-testid="conversation-tool-group-summary"]');
    const plan = turn?.querySelector('[data-testid="conversation-plan"]');
    const controlledState = (toggle) => {
      const controls = toggle?.getAttribute('aria-controls') || '';
      return {
        expanded: toggle?.getAttribute('aria-expanded') || '',
        controls,
        detailsPresent: Boolean(controls && document.getElementById(controls)),
      };
    };
    const summaryState = controlledState(summary);
    const reasoningToggle = turn?.querySelector('[data-testid="conversation-reasoning-toggle"]');
    const reasoningBefore = controlledState(reasoningToggle);
    reasoningToggle?.click();
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const reasoningAfter = controlledState(reasoningToggle);
    const reasoning = turn?.querySelector('[data-testid="conversation-reasoning-content"]');
    const commandButton = turn?.querySelector('[data-testid="conversation-compact-item-toggle"]');
    const commandTitle = commandButton?.querySelector('span.min-w-0.flex-1 > span.truncate');
    const turnRect = turn?.getBoundingClientRect();
    const contained = [reasoning, plan, summary, commandButton].every(node => {
      if (!node || !turnRect) return false;
      const rect = node.getBoundingClientRect();
      return rect.left >= turnRect.left - 1 && rect.right <= turnRect.right + 1;
    });
    const reasoningRect = reasoning?.getBoundingClientRect();
    const planRect = plan?.getBoundingClientRect();
    const summaryRect = summary?.getBoundingClientRect();
    reasoningToggle?.click();
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const reasoningCollapsed = controlledState(reasoningToggle);
    const commandClipped = Boolean(commandTitle && commandTitle.scrollWidth > commandTitle.clientWidth
      && commandButton.scrollWidth <= commandButton.clientWidth + 1);
    const commandBefore = controlledState(commandButton);
    commandButton?.click();
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const commandAfter = controlledState(commandButton);
    commandButton?.click();
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const commandCollapsed = controlledState(commandButton);
    return {
      found: Boolean(turn && reasoning && plan && summary && commandButton && commandTitle),
      turnIds: [...document.querySelectorAll('[data-conversation-turn]')]
        .map(node => node.getAttribute('data-conversation-turn')),
      hasOverflowText: document.body.innerText.includes('Test streaming overflow'),
      summaryCount: document.querySelectorAll('[data-testid="conversation-tool-group-summary"]').length,
      summary: summary?.textContent.trim() || '',
      summaryContainsRawCommand: Boolean(summary?.textContent.includes('overflow-marker-')),
      contained,
      ordered: Boolean(reasoningRect && planRect && summaryRect
        && reasoningRect.bottom <= planRect.top + 1
        && planRect.bottom <= summaryRect.top + 1),
      commandClipped,
      accessibility: {
        summaryState,
        reasoningBefore,
        reasoningAfter,
        reasoningCollapsed,
        commandBefore,
        commandAfter,
        commandCollapsed,
      },
    };
  });
  rec('①a-3c Codex 流式超长命令保持在工具卡内',
    codexStreamingOverflow.found
      && codexStreamingOverflow.summary === '正在执行 · 执行 Shell 命令 · 1 项'
      && !codexStreamingOverflow.summaryContainsRawCommand
      && codexStreamingOverflow.contained
      && codexStreamingOverflow.ordered
      && codexStreamingOverflow.commandClipped,
    JSON.stringify(codexStreamingOverflow));
  const unifiedA11y = codexStreamingOverflow.accessibility || {};
  rec('①a-3c-1 统一对话详情向辅助技术同步展开状态',
    unifiedA11y.summaryState?.expanded === 'true'
      && unifiedA11y.summaryState?.detailsPresent
      && unifiedA11y.reasoningBefore?.expanded === 'false'
      && !unifiedA11y.reasoningBefore?.controls
      && !unifiedA11y.reasoningBefore?.detailsPresent
      && unifiedA11y.reasoningAfter?.expanded === 'true'
      && Boolean(unifiedA11y.reasoningAfter?.controls)
      && unifiedA11y.reasoningAfter?.detailsPresent
      && unifiedA11y.reasoningCollapsed?.expanded === 'false'
      && !unifiedA11y.reasoningCollapsed?.controls
      && !unifiedA11y.reasoningCollapsed?.detailsPresent
      && unifiedA11y.commandBefore?.expanded === 'false'
      && !unifiedA11y.commandBefore?.controls
      && !unifiedA11y.commandBefore?.detailsPresent
      && unifiedA11y.commandAfter?.expanded === 'true'
      && Boolean(unifiedA11y.commandAfter?.controls)
      && unifiedA11y.commandAfter?.detailsPresent
      && unifiedA11y.commandCollapsed?.expanded === 'false'
      && !unifiedA11y.commandCollapsed?.controls
      && !unifiedA11y.commandCollapsed?.detailsPresent,
    JSON.stringify(unifiedA11y));

  await page.evaluate(async () => {
    const events = [
      {version:1,sessionId:'codex-1',turnId:'overflow-turn',seq:15,timestamp:'2026-08-04T01:01:05Z',event:{type:'tool_call_update',data:{update:{toolCallId:'overflow-tool',status:'completed',rawOutput:{formatted_output:'ok',exit_code:0}}}}},
      {version:1,sessionId:'codex-1',turnId:'overflow-turn',seq:16,timestamp:'2026-08-04T01:01:06Z',event:{type:'turn_completed',data:{status:'Completed',error:null}}},
    ];
    for (const payload of events) {
      for (const handler of (window.__TAURI_EVENT_HANDLERS__['acp:event'] || [])) await handler({ payload });
    }
  });
  await sleep(100);
  const codexCompletedOverflow = await page.evaluate(async () => {
    const turn = document.querySelector('[data-conversation-turn="overflow-turn"]');
    const summary = turn?.querySelector('[data-testid="conversation-tool-group-summary"]');
    const turnRect = turn?.getBoundingClientRect();
    const summaryRect = summary?.getBoundingClientRect();
    const controls = summary?.getAttribute('aria-controls') || '';
    const expandedBefore = summary?.getAttribute('aria-expanded') || '';
    const detailsBefore = Boolean(controls && document.getElementById(controls));
    summary?.click();
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    return {
      summary: summary?.textContent.trim() || '',
      containsRawCommand: Boolean(summary?.textContent.includes('overflow-marker-')),
      contained: Boolean(turnRect && summaryRect && summaryRect.left >= turnRect.left - 1 && summaryRect.right <= turnRect.right + 1),
      controls,
      expandedBefore,
      detailsBefore,
      controlsAfter: summary?.getAttribute('aria-controls') || '',
      expandedAfter: summary?.getAttribute('aria-expanded') || '',
      detailsAfter: Boolean(controls && document.getElementById(controls)),
    };
  });
  rec('①a-3d Codex 工具完成后摘要保持稳定',
    codexCompletedOverflow.summary === '执行步骤 · 1 项'
      && !codexCompletedOverflow.containsRawCommand
      && codexCompletedOverflow.contained,
    JSON.stringify(codexCompletedOverflow));
  rec('①a-3d-1 统一工具组折叠状态与详情 DOM 一致',
    codexCompletedOverflow.expandedBefore === 'true'
      && Boolean(codexCompletedOverflow.controls)
      && codexCompletedOverflow.detailsBefore
      && codexCompletedOverflow.expandedAfter === 'false'
      && !codexCompletedOverflow.controlsAfter
      && !codexCompletedOverflow.detailsAfter,
    JSON.stringify(codexCompletedOverflow));

  await clickText(page, '查看全部'); await sleep(400);
  const managedActiveState = await page.evaluate(() => {
    const label = [...document.querySelectorAll('span')]
      .find(node => (node.textContent || '').trim() === 'Codex回归会话' && node.getBoundingClientRect().left > 300);
    const row = label && label.closest('div[class*="cursor-pointer"]');
    return {
      found: !!row,
      activeClass: !!(row && (row.classList.contains('bg-[#333537]') || row.classList.contains('bg-[#E1E5EA]'))),
    };
  });
  rec('①a-4 对话管理页不残留当前会话选中背景',
    managedActiveState.found && !managedActiveState.activeClass,
    JSON.stringify(managedActiveState));
  await clickText(page, '批量管理'); await sleep(200);
  await page.evaluate(() => {
    const label = [...document.querySelectorAll('span')]
      .find(node => (node.textContent || '').trim() === 'Codex回归会话' && node.getBoundingClientRect().left > 300);
    label && label.closest('div[class*="cursor-pointer"]')?.click();
  });
  await clickText(page, '收纳'); await sleep(700);
  const codexBatchArchive = await page.evaluate(() => ({
    invoked: window.__TAURI_INVOKES__.some(call => call.cmd === 'set_session_archived' && call.args.id === 'codex-1' && call.args.archived === true),
    archived: document.body.innerText.includes('已收纳到【对话管理-已收纳】'),
    activeId: localStorage.getItem('pinvou_codex_active_session'),
  }));
  rec('①a-5 批量收纳 Codex 会话等待后端成功并清除激活态',
    codexBatchArchive.invoked && codexBatchArchive.archived && codexBatchArchive.activeId === null,
    JSON.stringify(codexBatchArchive));
  await page.evaluate(() => document.querySelector('button[aria-label="取消"]')?.click());

  // 模型表单点击遮罩（弹窗外部）直接关闭——与修复后的其它弹窗行为一致；
  // 显式点击“取消”同样可关闭。
  const modelModalOpened = await page.evaluate(() => {
    const nav = document.querySelector('[data-testid="nav-settings"]');
    if (!nav) return false;
    nav.click();
    return true;
  });
  await sleep(700);
  const modelModalOutsideClick = await page.evaluate(async () => {
    const settle = () => new Promise(resolve => setTimeout(resolve, 50));
    const modelSection = [...document.querySelectorAll('aside button')]
      .find(button => (button.textContent || '').trim() === '模型');
    if (modelSection) {
      modelSection.click();
      await settle();
    }
    const add = document.querySelector('[data-testid="settings-model-add"]');
    if (!add) return { addFound: false, opened: false, closedByOutside: false, reopened: false, cancelled: false };
    add.click();
    await settle();
    const backdrop = document.querySelector('[data-testid="model-form-backdrop"]');
    const opened = !!document.querySelector('[data-testid="model-form-dialog"]');
    if (backdrop) backdrop.click();
    await settle();
    const closedByOutside = !document.querySelector('[data-testid="model-form-dialog"]');
    // 关闭后设置面板会重渲染，需重新查询按钮（旧引用已脱离文档）
    document.querySelector('[data-testid="settings-model-add"]')?.click();
    await settle();
    const reopened = !!document.querySelector('[data-testid="model-form-dialog"]');
    const cancel = document.querySelector('[data-testid="model-form-cancel"]');
    if (cancel) cancel.click();
    await settle();
    return {
      addFound: true,
      opened,
      closedByOutside,
      reopened,
      cancelled: !document.querySelector('[data-testid="model-form-dialog"]'),
    };
  });
  rec(
    '模型编辑弹窗点击外部可关闭且显式取消仍可关闭',
    modelModalOpened && modelModalOutsideClick.addFound && modelModalOutsideClick.opened
      && modelModalOutsideClick.closedByOutside && modelModalOutsideClick.reopened && modelModalOutsideClick.cancelled,
    JSON.stringify(modelModalOutsideClick),
  );


  // 手机先向尚未在桌面打开的后台 session 发消息：hydration 必须先把磁盘 messages
  // 重建成 chatItems；否则桌面随后切入时只剩这条手机消息，历史和产物卡都像“丢了”。
  await page.evaluate(async () => {
    const handlers = window.__TAURI_EVENT_HANDLERS__['chat:user_message'] || [];
    for (const handler of handlers) {
      await handler({
        id: 'event-mobile-admission',
        payload: {
          session_id: 's1',
          content: '手机补充消息',
          operation: 'append',
          base_transcript_revision: 'ui-smoke-baseline',
        },
      });
    }
  });

  // ② 工具商店关键连接器卡
  await page.evaluate(() => document.querySelector('[data-nav="toolstore"]')?.click());
  await sleep(1500);
  const connectors = await page.evaluate(() => {
    const text = document.body.innerText;
    return { obsidian: text.includes('Obsidian'), dingtalk: text.includes('钉钉') };
  });
  rec('② 工具商店渲染 Obsidian 与钉钉卡', connectors.obsidian && connectors.dingtalk, JSON.stringify(connectors));

  // ③ 聊天流产物卡 品/悟 召唤按钮
  await clickText(page, '第三季度财报分析');
  await sleep(2200);
  const hit = await page.evaluate(() => [...document.querySelectorAll('button')]
    .map(b => ({ t: b.getAttribute('title') || '', x: (b.textContent || '').trim() }))
    .filter(o => o.t.includes('查错') || o.t.includes('发散') || /品|悟/.test(o.x)).length);
  const restored = await page.evaluate(() => {
    const text = document.body.innerText;
    const artifactPaths = window.TauriBridge.state.getMany(['chat', 'vllm']).chatItems
      .filter(item => item.type === 'artifact_card')
      .map(item => item.path);
    const restoredShellCards = window.TauriBridge.state.getMany(['chat', 'vllm']).chatItems.filter(item =>
      item.type === 'tool' && item.name === 'exec_shell' && item.args && item.args.command === 'history-shell');
    return {
      history: text.includes('整理纪要'),
      mobile: text.includes('手机补充消息'),
      mcpArtifact: artifactPaths.some(path => String(path).includes('季度报告.pptx')),
      shellFakeArtifact: artifactPaths.some(path => String(path).includes('validator-fake.html')),
      shellHistoryCount: restoredShellCards.length,
      shellHistoryTaskId: restoredShellCards[0] && restoredShellCards[0].taskId,
      shellHistoryOutput: restoredShellCards[0] && restoredShellCards[0].output,
    };
  });
  rec('③ 后台session恢复时仅 MCP producer 结果生成产物卡、Shell 历史卡不重复',
    hit >= 2 && restored.history && restored.mobile && restored.mcpArtifact && !restored.shellFakeArtifact &&
    restored.shellHistoryCount === 1 && restored.shellHistoryTaskId === 'task-history-done' &&
    restored.shellHistoryOutput === 'history output',
    JSON.stringify({ hit, ...restored }));

  const multiKb = await page.evaluate(async () => {
    const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
    document.querySelector('[data-testid="kb-mount-trigger"]')?.click();
    await wait(100);
    const row = name => [...document.querySelectorAll('[data-testid="kb-mount-row"]')]
      .find(node => (node.textContent || '').includes(name));
    row('项目资料')?.querySelector('[data-testid="kb-mount-toggle"]')?.click();
    await wait(100);
    row('团队规范')?.querySelector('[data-testid="kb-mount-toggle"]')?.click();
    await wait(100);
    row('项目资料')?.querySelector('[data-testid="kb-mount-toggle"]')?.click();
    await wait(100);
    row('团队规范')?.querySelector('[data-testid="kb-mount-remove"]')?.click();
    await wait(100);
    const knowledge = window.TauriBridge.state.get('knowledge');
    return {
      mountedCollections: knowledge.mountedCollections,
      mountedCollection: knowledge.mountedCollection,
      menuText: document.body.innerText,
      commands: window.__TAURI_INVOKES__
        .filter(call => /^session_(?:add|set|remove)_mounted_collection/.test(call.cmd))
        .map(call => ({ cmd: call.cmd, args: call.args })),
    };
  });
  rec('③a-1 多知识库可追加、单独停用和移除且不覆盖其他挂载项',
    multiKb.commands.length === 4 &&
    multiKb.commands[0].cmd === 'session_add_mounted_collection' &&
    multiKb.commands[1].cmd === 'session_add_mounted_collection' &&
    multiKb.commands[2].cmd === 'session_set_mounted_collection_enabled' &&
    multiKb.commands[3].cmd === 'session_remove_mounted_collection' &&
    multiKb.mountedCollections.length === 1 &&
    multiKb.mountedCollections[0].collectionId === 7 &&
    multiKb.mountedCollections[0].enabled === false &&
    multiKb.mountedCollection === null &&
    multiKb.menuText.includes('项目资料') && multiKb.menuText.includes('已停用'),
    JSON.stringify(multiKb));
  const kbRuntimeGate = await page.evaluate(async () => {
    const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
    window.__KB_MODEL_STATUS__ = { installed: true, ready: false, loading: true };
    const trigger = document.querySelector('[data-testid="kb-mount-trigger"]');
    // 上一个场景结束时菜单仍展开：先关闭，再打开以刷新运行时状态。
    trigger?.click();
    await wait(50);
    trigger?.click();
    await wait(100);
    const row = [...document.querySelectorAll('[data-testid="kb-mount-row"]')]
      .find(node => (node.textContent || '').includes('团队规范'));
    const toggle = row?.querySelector('[data-testid="kb-mount-toggle"]');
    const before = window.__TAURI_INVOKES__.filter(call => call.cmd === 'session_add_mounted_collection').length;
    toggle?.click();
    await wait(50);
    const after = window.__TAURI_INVOKES__.filter(call => call.cmd === 'session_add_mounted_collection').length;
    return {
      disabled: Boolean(toggle && toggle.disabled),
      blockedCopy: document.body.innerText.includes('Embedding 模型正在加载或加载失败'),
      mountCallsUnchanged: before === after,
    };
  });
  rec('③a-2 模型文件已安装但运行时未就绪时不允许挂载',
    kbRuntimeGate.disabled && kbRuntimeGate.blockedCopy && kbRuntimeGate.mountCallsUnchanged,
    JSON.stringify(kbRuntimeGate));
  const kbRepairFlow = await page.evaluate(async () => {
    const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
    window.__KB_MODEL_STATUS__ = {
      installed: true,
      ready: false,
      loading: false,
      failed: true,
      error: 'mock embedding load failed',
    };
    await window.TauriBridge.knowledge.downloadKbModel(false).catch(() => {});
    document.querySelector('[data-nav="knowledge"]')?.click();
    await wait(300);
    const failedVisible = document.body.innerText.includes('Embedding 模型加载失败');
    const repairButton = [...document.querySelectorAll('button')]
      .find(button => (button.textContent || '').includes('重新下载并修复'));
    repairButton?.click();
    await wait(250);
    const calls = window.__KB_MODEL_DOWNLOAD_ARGS__.slice();
    return {
      failedVisible,
      repairFound: Boolean(repairButton),
      repairRequested: calls.some(args => args && args.repair === true),
      recovered: document.body.innerText.includes('AI 知识库'),
    };
  });
  rec('③a-3 模型加载失败时可重新下载、验证并恢复知识库',
    kbRepairFlow.failedVisible && kbRepairFlow.repairFound && kbRepairFlow.repairRequested && kbRepairFlow.recovered,
    JSON.stringify(kbRepairFlow));
  await clickText(page, '第三季度财报分析');
  await sleep(300);
  const assistantCopy = await page.evaluate(async () => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: async text => { window.__ASSISTANT_COPY_TEXT__ = text; } },
    });
    const action = [...document.querySelectorAll('[data-testid="assistant-message-actions"]')]
      .find(node => node.closest('[data-conversation-turn]')?.innerText.includes('已生成会议纪要'));
    const button = action?.querySelector('[data-testid="assistant-message-copy"]');
    if (!button) return { found: false };
    const turn = action.closest('[data-conversation-turn]');
    const footer = action.closest('[data-testid="assistant-message-footer"]');
    const footerChildren = [...(footer?.children || [])];
    button.click();
    await new Promise(resolve => setTimeout(resolve, 50));
    return {
      found: true,
      copied: window.__ASSISTANT_COPY_TEXT__ || '',
      renderedCard: turn?.innerText.includes("Reviewer's Agent") && turn?.innerText.includes("It's a highlighted JSON card"),
      renderedQuestion: turn?.innerText.includes('继续执行？') && turn?.innerText.includes('继续') && turn?.innerText.includes('取消'),
      hiddenPayloadAbsent: !turn?.innerText.includes('hidden-prompt') && !turn?.innerText.includes('"question"'),
      feedback: button.textContent.trim(),
      title: button.getAttribute('title') || '',
      singleAction: turn?.querySelectorAll('[data-testid="assistant-message-actions"]').length === 1,
      sharedFooter: Boolean(footer),
      footerChildCount: footerChildren.length,
      sameRow: footerChildren.length > 1 && footerChildren.every((node) => {
        const firstRect = footerChildren[0].getBoundingClientRect();
        const rect = node.getBoundingClientRect();
        return Math.abs((rect.top + rect.height / 2) - (firstRect.top + firstRect.height / 2)) < 2;
      }),
    };
  });
  rec('③a 每条完成态助手回复提供一键复制并显示成功反馈',
    assistantCopy.found && assistantCopy.copied.includes('已生成会议纪要。') &&
    assistantCopy.copied.includes("Reviewer's Agent\n\nIt's a highlighted JSON card") &&
    assistantCopy.copied.includes('继续执行？\n\n1. 继续\n2. 取消') &&
    assistantCopy.renderedCard && assistantCopy.renderedQuestion && assistantCopy.hiddenPayloadAbsent &&
    assistantCopy.feedback === '已复制' && assistantCopy.title === '已复制' &&
    assistantCopy.singleAction && assistantCopy.sharedFooter && assistantCopy.sameRow,
    JSON.stringify(assistantCopy));

  const assistantExportShare = await page.evaluate(async () => {
    const action = [...document.querySelectorAll('[data-testid="assistant-message-actions"]')]
      .find(node => node.closest('[data-conversation-turn]')?.innerText.includes('已生成会议纪要'));
    const exportButton = action?.querySelector('[data-testid="assistant-message-export"]');
    const shareButton = action?.querySelector('[data-testid="assistant-message-share"]');
    if (!exportButton || !shareButton) return { found: false };

    exportButton.click();
    await new Promise(resolve => setTimeout(resolve, 30));
    const exportMenu = document.querySelector('[data-testid="assistant-message-export-menu"]');
    const markdownOption = exportMenu?.querySelector('[data-testid="assistant-export-md"]');
    const htmlOption = exportMenu?.querySelector('[data-testid="assistant-export-html"]');
    const markdownDefault = (markdownOption?.querySelectorAll('svg').length || 0) === 2;
    const exportAriaLinked = exportButton.getAttribute('aria-controls') === exportMenu?.id &&
      exportMenu?.getAttribute('aria-labelledby') === exportButton.id &&
      exportButton.getAttribute('aria-haspopup') === 'menu' &&
      exportButton.getAttribute('aria-expanded') === 'true';
    const exportInitialFocus = document.activeElement === markdownOption;
    exportMenu?.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
    const exportArrowDown = document.activeElement === htmlOption;
    exportMenu?.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
    const exportArrowWrap = document.activeElement === markdownOption;
    exportMenu?.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }));
    const exportArrowUpWrap = document.activeElement === htmlOption;
    exportMenu?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Home', bubbles: true }));
    const exportHome = document.activeElement === markdownOption;
    exportMenu?.dispatchEvent(new KeyboardEvent('keydown', { key: 'End', bubbles: true }));
    const exportEnd = document.activeElement === htmlOption;
    exportMenu?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await new Promise(resolve => setTimeout(resolve, 30));
    const exportEscapeRestored = !document.querySelector('[data-testid="assistant-message-export-menu"]') &&
      document.activeElement === exportButton && exportButton.getAttribute('aria-expanded') === 'false';

    exportButton.click();
    await new Promise(resolve => setTimeout(resolve, 30));
    document.querySelector('[data-testid="assistant-export-html"]')?.click();
    await new Promise(resolve => setTimeout(resolve, 30));
    const exportInvoke = [...window.__TAURI_INVOKES__]
      .reverse()
      .find(call => call.cmd === 'export_assistant_response');
    const exportSelectionRestored = document.activeElement === exportButton;

    shareButton.click();
    await new Promise(resolve => setTimeout(resolve, 30));
    let shareMenu = document.querySelector('[data-testid="assistant-message-share-menu"]');
    const targets = ['wechat', 'wecom', 'feishu', 'dingtalk', 'qq'];
    const allTargets = targets.every(target => shareMenu?.querySelector(`[data-testid="assistant-share-${target}"]`));
    const systemShare = shareMenu?.querySelector('[data-testid="assistant-share-system"]');
    const qqShare = shareMenu?.querySelector('[data-testid="assistant-share-qq"]');
    const shareAriaLinked = shareButton.getAttribute('aria-controls') === shareMenu?.id &&
      shareMenu?.getAttribute('aria-labelledby') === shareButton.id &&
      shareButton.getAttribute('aria-expanded') === 'true';
    const shareInitialFocus = document.activeElement === systemShare;
    const shareStructure = shareMenu?.querySelector('[role="separator"]') &&
      shareMenu?.querySelector('[role="group"][aria-labelledby]') &&
      [...shareMenu.querySelectorAll('[role="menuitem"]')].every(item => item.getAttribute('aria-label'));
    shareMenu?.dispatchEvent(new KeyboardEvent('keydown', { key: 'End', bubbles: true }));
    const shareEnd = document.activeElement === qqShare;
    shareMenu?.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
    const shareArrowWrap = document.activeElement === systemShare;
    shareMenu?.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }));
    const shareArrowUpWrap = document.activeElement === qqShare;
    shareMenu?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }));
    await new Promise(resolve => setTimeout(resolve, 30));
    const tabClosedWithoutRestore = !document.querySelector('[data-testid="assistant-message-share-menu"]') &&
      document.activeElement !== shareButton;

    shareButton.click();
    await new Promise(resolve => setTimeout(resolve, 30));
    shareMenu = document.querySelector('[data-testid="assistant-message-share-menu"]');
    shareMenu?.querySelector('[data-testid="assistant-share-feishu"]')?.click();
    await new Promise(resolve => setTimeout(resolve, 30));
    const shareInvoke = [...window.__TAURI_INVOKES__]
      .reverse()
      .find(call => call.cmd === 'open_assistant_share_target');
    return {
      found: true,
      exportMenu: Boolean(exportMenu),
      markdownDefault,
      exportAriaLinked,
      exportInitialFocus,
      exportArrowDown,
      exportArrowWrap,
      exportArrowUpWrap,
      exportHome,
      exportEnd,
      exportEscapeRestored,
      exportSelectionRestored,
      exportFormat: exportInvoke?.args?.format || '',
      exportName: exportInvoke?.args?.defaultName || '',
      htmlStandalone: exportInvoke?.args?.content?.startsWith('<!doctype html>') || false,
      htmlSafe: !/<script>/i.test(exportInvoke?.args?.content || ''),
      shareMenu: Boolean(shareMenu),
      allTargets,
      shareAriaLinked,
      shareInitialFocus,
      shareStructure: Boolean(shareStructure),
      shareEnd,
      shareArrowWrap,
      shareArrowUpWrap,
      tabClosedWithoutRestore,
      shareSelectionRestored: document.activeElement === shareButton,
      shareTarget: shareInvoke?.args?.target || '',
      shareFeedback: action.innerText,
    };
  });
  rec('③a-4 回复可默认导出 Markdown、选择 HTML，并复制后打开常见通信应用',
    assistantExportShare.found && assistantExportShare.exportMenu && assistantExportShare.markdownDefault &&
    assistantExportShare.exportAriaLinked && assistantExportShare.exportInitialFocus &&
    assistantExportShare.exportArrowDown && assistantExportShare.exportArrowWrap &&
    assistantExportShare.exportArrowUpWrap && assistantExportShare.exportHome && assistantExportShare.exportEnd &&
    assistantExportShare.exportEscapeRestored && assistantExportShare.exportSelectionRestored &&
    assistantExportShare.exportFormat === 'html' && assistantExportShare.exportName.endsWith('.html') &&
    assistantExportShare.htmlStandalone && assistantExportShare.htmlSafe && assistantExportShare.shareMenu &&
    assistantExportShare.allTargets && assistantExportShare.shareAriaLinked &&
    assistantExportShare.shareInitialFocus && assistantExportShare.shareStructure &&
    assistantExportShare.shareEnd && assistantExportShare.shareArrowWrap && assistantExportShare.shareArrowUpWrap &&
    assistantExportShare.tabClosedWithoutRestore && assistantExportShare.shareSelectionRestored &&
    assistantExportShare.shareTarget === 'feishu' &&
    assistantExportShare.shareFeedback.includes('请粘贴发送'),
    JSON.stringify(assistantExportShare));

  // 会话输入框是页面条件渲染的：跳工具商店会卸载 ChatView，返回同一 session
  // 必须恢复未发送内容，不得因组件重建清空。
  const composerDraft = '这是尚未发送的 session 草稿';
  const draftInputFound = await page.evaluate((value) => {
    const textarea = document.querySelector('[data-testid="chat-composer-input"]');
    if (!textarea) return false;
    const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
    setter.call(textarea, value);
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
    return true;
  }, composerDraft);
  await sleep(100);
  const draftEntered = await page.evaluate((value) =>
    document.querySelector('[data-testid="chat-composer-input"]')?.value === value, composerDraft);
  await page.evaluate(() => document.querySelector('[data-nav="toolstore"]')?.click());
  await sleep(700);
  const draftViewChanged = await page.evaluate(() =>
    document.querySelector('[data-testid="app-root"]')?.getAttribute('data-current-view') === 'toolStore');
  await clickText(page, '第三季度财报分析');
  await sleep(900);
  const restoredDraft = await page.evaluate(() =>
    document.querySelector('[data-testid="chat-composer-input"]')?.value || '');
  await clickText(page, '新对话');
  await sleep(300);
  const newSessionDraft = await page.evaluate(() =>
    document.querySelector('[data-testid="chat-composer-input"]')?.value || '');
  await clickText(page, '第三季度财报分析');
  await sleep(900);
  const restoredSessionDraft = await page.evaluate(() =>
    document.querySelector('[data-testid="chat-composer-input"]')?.value || '');
  rec('③e session 未发送草稿跨页面恢复',
    draftInputFound && draftEntered && draftViewChanged && restoredDraft === composerDraft &&
    newSessionDraft === '' && restoredSessionDraft === composerDraft,
    JSON.stringify({ draftInputFound, draftEntered, draftViewChanged, restoredDraft, newSessionDraft, restoredSessionDraft }));

  // 尚未物化的新对话没有 session buffer。后台已有 session 收到事件时，
  // 临时切换工作集再恢复，仍必须保住当前输入框草稿。
  await clickText(page, '新对话');
  await sleep(300);
  const pendingDraft = '后台事件期间也不能丢失的新对话草稿';
  const pendingDraftResult = await page.evaluate(async (value) => {
    const textarea = document.querySelector('[data-testid="chat-composer-input"]');
    if (!textarea) return { inputFound: false };
    const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
    setter.call(textarea, value);
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
    for (const handler of (window.__TAURI_EVENT_HANDLERS__['chat:usage'] || [])) {
      await handler({
        event: 'chat:usage',
        payload: { session_id: 's1', input_tokens: 42 },
      });
    }
    return {
      inputFound: true,
      activeSessionId: window.TauriBridge.state.getMany(['sessions']).activeSessionId,
      bridgeDraft: window.TauriBridge.chat.getComposerDraft(),
      visibleDraft: textarea.value,
    };
  }, pendingDraft);
  rec('③f 新对话草稿不被后台 session 事件清空',
    pendingDraftResult.inputFound &&
    pendingDraftResult.activeSessionId === null &&
    pendingDraftResult.bridgeDraft === pendingDraft &&
    pendingDraftResult.visibleDraft === pendingDraft,
    JSON.stringify(pendingDraftResult));
  await page.evaluate(() => {
    const textarea = document.querySelector('[data-testid="chat-composer-input"]');
    if (!textarea) return;
    const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
    setter.call(textarea, '');
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await clickText(page, '第三季度财报分析');
  await sleep(900);

  // 实时事件也必须使用同一 producer 判定：validator 的 shell JSON 不能进产物面板，
  // 真 MCP producer 返回路径仍要被跟踪。
  await page.evaluate(async () => {
    async function emit(name, payload) {
      const handlers = window.__TAURI_EVENT_HANDLERS__[name] || [];
      for (const handler of handlers) await handler({ payload });
    }
    await emit('chat:tool_start', { session_id:'s1', id:'live-shell', name:'exec_shell', args:{ command:'validator --json' } });
    await emit('chat:tool_delta', { session_id:'s1', id:'live-shell', stream:'stdout', content:'downloaded 42%\n' + Array.from({ length: 80 }, (_, i) => 'progress line ' + i).join('\n') + '\nlive progress tail' });
  });
  await sleep(200);
  const liveShell = await page.evaluate(() => {
    const item = window.TauriBridge.state.getMany(['chat', 'vllm']).chatItems.find(item => item.toolId === 'live-shell');
    return {
      running: item && item.state === 'running',
      output: item && item.output,
      visible: document.body.innerText.includes('downloaded 42%'),
    };
  });
  rec('live exec_shell output is visible before tool_end', liveShell.running && liveShell.output.includes('downloaded 42%') && liveShell.visible, JSON.stringify(liveShell));
  await page.evaluate(async () => {
    async function emit(name, payload) {
      const handlers = window.__TAURI_EVENT_HANDLERS__[name] || [];
      for (const handler of handlers) await handler({ payload });
    }
    await emit('chat:tool_start', { session_id:'s1', id:'live-shell-wait', name:'exec_shell_wait', args:{ task_id:'shell-task-1', wait:true } });
    await emit('chat:tool_delta', { session_id:'s1', id:'live-shell-wait', stream:'stdout', content:'tick 42\n' });
  });
  await sleep(100);
  const liveShellWait = await page.evaluate(() => {
    const item = window.TauriBridge.state.getMany(['chat', 'vllm']).chatItems.find(item => item.toolId === 'live-shell-wait');
    return {
      running: item && item.state === 'running',
      output: item && item.output,
      visible: document.body.innerText.includes('tick 42'),
    };
  });
  rec('live exec_shell_wait output is visible before tool_end', liveShellWait.running && liveShellWait.output.includes('tick 42') && liveShellWait.visible, JSON.stringify(liveShellWait));
  await page.evaluate(async () => {
    async function emit(name, payload) {
      const handlers = window.__TAURI_EVENT_HANDLERS__[name] || [];
      for (const handler of handlers) await handler({ payload });
    }
    await emit('chat:tool_start', { session_id:'s1', id:'split-terminal', name:'exec_shell', args:{ command:'split terminal frames' } });
    await emit('chat:tool_delta', { session_id:'s1', id:'split-terminal', stream:'stdout', content:'line one\r' });
    await emit('chat:tool_delta', { session_id:'s1', id:'split-terminal', stream:'stdout', content:'\nDownloading 10%\r' });
    await emit('chat:tool_delta', { session_id:'s1', id:'split-terminal', stream:'stdout', content:'Downloading \u001b[3' });
    await emit('chat:tool_delta', { session_id:'s1', id:'split-terminal', stream:'stdout', content:'2m90%\u001b[0' });
    await emit('chat:tool_delta', { session_id:'s1', id:'split-terminal', stream:'stdout', content:'m' });
  });
  await sleep(100);
  const splitTerminal = await page.evaluate(() => {
    const item = window.TauriBridge.state.getMany(['chat', 'vllm']).chatItems.find(item => item.toolId === 'split-terminal');
    return item && item.output;
  });
  rec('terminal parser preserves CRLF and ANSI state across live chunks',
    splitTerminal === 'line one\nDownloading 90%' && !splitTerminal.includes('\u001b') && !splitTerminal.includes('10%'),
    JSON.stringify(splitTerminal));
  await page.evaluate(async () => {
    async function emit(name, payload) {
      const handlers = window.__TAURI_EVENT_HANDLERS__[name] || [];
      for (const handler of handlers) await handler({ payload });
    }
    await emit('chat:tool_start', { session_id:'s1', id:'split-terminal-stderr', name:'exec_shell', args:{ command:'split stderr style' } });
    await emit('chat:tool_delta', { session_id:'s1', id:'split-terminal-stderr', stream:'stderr', content:'\u001b[3' });
    await emit('chat:tool_delta', { session_id:'s1', id:'split-terminal-stderr', stream:'stderr', content:'1m错误\u001b[0' });
    await emit('chat:tool_delta', { session_id:'s1', id:'split-terminal-stderr', stream:'stderr', content:'m' });
  });
  const splitTerminalStderr = await page.evaluate(() => {
    const item = window.TauriBridge.state.getMany(['chat', 'vllm']).chatItems.find(item => item.toolId === 'split-terminal-stderr');
    return item && item.output;
  });
  rec('terminal parser preserves stderr ANSI state across live chunks',
    splitTerminalStderr === '[STDERR] 错误' && !splitTerminalStderr.includes('\u001b'),
    JSON.stringify(splitTerminalStderr));
  await page.evaluate(async () => {
    async function emit(name, payload) {
      const handlers = window.__TAURI_EVENT_HANDLERS__[name] || [];
      for (const handler of handlers) await handler({ payload });
    }
    await emit('chat:tool_start', { session_id:'s1', id:'background-shell', name:'exec_shell', args:{ command:'winget install WPS' } });
    await emit('chat:tool_delta', { session_id:'s1', id:'background-shell', stream:'stdout', content:'\u001b[32mDownloading 10%\u001b[0m\rDownloading 90%' });
    await emit('chat:tool_end', {
      session_id:'s1', id:'background-shell', success:true, output:'Command started in background',
      metadata:{ backgrounded:true, status:'Running', task_id:'shell-bg-1' }
    });
    await emit('chat:done', { session_id:'s1', status:'Completed' });
  });
  await sleep(150);
  const backgroundShell = await page.evaluate(() => {
    const item = window.TauriBridge.state.getMany(['chat', 'vllm']).chatItems.find(item => item.toolId === 'background-shell');
    const button = document.querySelector('[data-testid="cancel-shell-task"][data-shell-task-id="shell-bg-1"]');
    if (button) button.click();
    return {
      running: item && item.state === 'running' && item.background === true,
      taskId: item && item.taskId,
      output: item && item.output,
      button: !!button,
    };
  });
  await sleep(50);
  const backgroundCancelInvoke = await page.evaluate(() => window.__TAURI_INVOKES__.some(entry =>
    entry.cmd === 'cancel_shell_task' && entry.args.sessionId === 's1' && entry.args.taskId === 'shell-bg-1'));
  rec('background shell survives chat:done, collapses terminal progress, and cancels by task id',
    backgroundShell.running && backgroundShell.taskId === 'shell-bg-1' &&
      backgroundShell.output.includes('Downloading 90%') && !backgroundShell.output.includes('10%') &&
      backgroundShell.button && backgroundCancelInvoke,
    JSON.stringify({ backgroundShell, backgroundCancelInvoke }));
  await page.evaluate(async () => {
    const handlers = window.__TAURI_EVENT_HANDLERS__['chat:shell_task_status'] || [];
    for (const handler of handlers) await handler({ payload: {
      session_id:'s1', tool_id:'background-shell', task_id:'shell-bg-1', status:'Killed', exit_code:null,
      stdout_tail:'Downloading 90%\nDownloaded final tail',
      stderr_tail:'installer warning from final stderr'
    }});
  });
  const backgroundKilled = await page.evaluate(() => {
    const item = window.TauriBridge.state.getMany(['chat', 'vllm']).chatItems.find(item => item.toolId === 'background-shell');
    return {
      terminal: item && item.state === 'failed' && item.shellStatus === 'Killed' && item.background === false,
      output: item && item.output,
    };
  });
  rec('background shell terminal event reconciles final stdout and stderr tails',
    backgroundKilled.terminal &&
      backgroundKilled.output === 'Downloading 90%\nDownloaded final tail\n[STDERR] installer warning from final stderr',
    JSON.stringify(backgroundKilled));
  await page.evaluate(async () => {
    async function emit(name, payload) {
      const handlers = window.__TAURI_EVENT_HANDLERS__[name] || [];
      for (const handler of handlers) await handler({ payload });
    }
    await emit('chat:tool_end', { session_id:'s1', id:'live-shell', success:true, output:'{"ok":true,"path":"live-validator-fake.html"}' });
    await emit('chat:tool_end', { session_id:'s1', id:'live-shell-wait', success:true, output:'tick 42\ntick 100\n' });
    await emit('chat:tool_start', { session_id:'s1', id:'live-mcp', name:'mcp_gongwen_make_gongwen', args:{} });
    await emit('chat:tool_end', { session_id:'s1', id:'live-mcp', success:true, output:'{"path":"live-report.docx"}' });
  });
  const liveArtifacts = await page.evaluate(() => window.TauriBridge.state.getMany(['chat', 'vllm']).artifacts.map(item => item.path));
  rec('③b 实时 tool_end 不跟踪 shell path、保留 MCP 产物', liveArtifacts.includes('live-report.docx') && !liveArtifacts.includes('live-validator-fake.html'), JSON.stringify(liveArtifacts));

  // ④ 后端 pending 事件必须直达 React 候选卡；三个决策按钮必须调用各自命令。
  async function emitMemoryCandidate(id, text) {
    await page.evaluate(async ({ id, text }) => {
      const handlers = window.__TAURI_EVENT_HANDLERS__['chat:memory_write'] || [];
      for (const handler of handlers) {
        await handler({ payload: { session_id: 's1', events: [{ action: 'pending', kind: 'preference', id, text }] } });
      }
    }, { id, text });
    await sleep(250);
  }
  async function clickMemoryAction(id, testId) {
    return page.evaluate(({ id, testId }) => {
      const card = document.querySelector(`[data-testid="memory-candidate-card"][data-memory-id="${id}"]`);
      const button = card && card.querySelector(`[data-testid="${testId}"]`);
      if (!button) return false;
      button.click();
      return true;
    }, { id, testId });
  }
  await emitMemoryCandidate('mem-confirm', '回答默认先给结论');
  const candidateVisible = await page.evaluate(() => {
    const card = document.querySelector('[data-testid="memory-candidate-card"][data-memory-id="mem-confirm"]');
    return !!card && card.textContent.includes('记忆候选') && card.textContent.includes('回答默认先给结论');
  });
  const confirmClicked = await clickMemoryAction('mem-confirm', 'memory-candidate-confirm');
  await emitMemoryCandidate('mem-ignore', '回答尽量简洁');
  const ignoreClicked = await clickMemoryAction('mem-ignore', 'memory-candidate-ignore');
  await emitMemoryCandidate('mem-never', '不要使用过多术语');
  const neverClicked = await clickMemoryAction('mem-never', 'memory-candidate-never');
  await sleep(350);
  const memoryCommands = await page.evaluate(() => window.__TAURI_INVOKES__
    .filter(call => ['confirm_pending_memory', 'ignore_pending_memory', 'never_pending_memory'].includes(call.cmd))
    .map(call => `${call.cmd}:${call.args.id}`));
  rec('④ 记忆候选卡渲染+三个决策动作贯通', candidateVisible && confirmClicked && ignoreClicked && neverClicked &&
    memoryCommands.includes('confirm_pending_memory:mem-confirm') &&
    memoryCommands.includes('ignore_pending_memory:mem-ignore') &&
    memoryCommands.includes('never_pending_memory:mem-never'), JSON.stringify(memoryCommands));

  // ShellManager 快照轮询：按 task_id 更新同一卡片、明确标注 tail 省略、取消直达 kill，
  // 终态展示退出码且不依赖 chat:done。
  await page.evaluate(async () => {
    window.__SHELL_JOBS__=[{
      id:'task-live-1',job_id:'1',command:'long-running-test',cwd:'C:/tmp',status:'Running',
      exit_code:null,elapsed_ms:500,stdout_tail:'...tick 1\r\ntick 2\r\nprogress 10%\rprogress 80%',stderr_tail:'',
      stdout_len:4096,stderr_len:0,stdin_available:true,stale:false,linked_task_id:null,
    }];
    const handlers=window.__TAURI_EVENT_HANDLERS__['chat:tool_start']||[];
    for(const handler of handlers) await handler({payload:{session_id:'s1',id:'shell-realtime',name:'exec_shell',args:{command:'long-running-test'}}});
  });
  await sleep(700);
  const shellRunning = await page.evaluate(() => {
    const item=window.TauriBridge.state.getMany(['chat', 'vllm']).chatItems.find(it=>it.taskId==='task-live-1');
    return item && {state:item.state,taskId:item.taskId,output:item.output};
  });
  await page.evaluate(() => window.TauriBridge.chat.cancelShellTask('s1','task-live-1'));
  await sleep(700);
  const shellCancelled = await page.evaluate(() => {
    const item=window.TauriBridge.state.getMany(['chat', 'vllm']).chatItems.find(it=>it.taskId==='task-live-1');
    return {item:item&&{state:item.state,exitCode:item.exitCode,output:item.output},args:window.__CANCEL_SHELL_ARGS__};
  });
  rec('③c Shell 实时 tail、省略标记、task_id 取消与退出码',
    shellRunning && shellRunning.state==='running' && shellRunning.output.includes('输出已省略') &&
    shellRunning.output.includes('tick 1') && shellRunning.output.includes('tick 2') &&
    shellRunning.output.includes('progress 80%') && !shellRunning.output.includes('progress 10%') &&
    shellCancelled.item && shellCancelled.item.state==='failed' && shellCancelled.item.exitCode===130 && shellCancelled.item.output.includes('退出码: 130') &&
    shellCancelled.args && shellCancelled.args.sessionId==='s1' && shellCancelled.args.taskId==='task-live-1',
    JSON.stringify({shellRunning,shellCancelled}));

  // 相同命令不能靠 command 字符串猜 task_id；先用 task-id 合成卡承接，
  // tool_end 暴露真实 id 后再与对应工具卡合并。首次看到已结束的 detached job 也不能丢。
  await page.evaluate(async () => {
    window.__SHELL_JOBS__=[
      {id:'task-dup-a',job_id:'2',command:'same-command',cwd:'C:/tmp',status:'Running',exit_code:null,elapsed_ms:100,stdout_tail:'A',stderr_tail:'',stdout_len:1,stderr_len:0,stdin_available:true,stale:false,linked_task_id:null},
      {id:'task-dup-b',job_id:'3',command:'same-command',cwd:'C:/tmp',status:'Running',exit_code:null,elapsed_ms:100,stdout_tail:'B',stderr_tail:'',stdout_len:1,stderr_len:0,stdin_available:true,stale:false,linked_task_id:null},
      {id:'task-detached-done',job_id:'4',command:'fast-detached',cwd:'C:/tmp',status:'Completed',exit_code:0,elapsed_ms:20,stdout_tail:'done fast',stderr_tail:'',stdout_len:9,stderr_len:0,stdin_available:false,stale:false,linked_task_id:null},
    ];
    const starts=window.__TAURI_EVENT_HANDLERS__['chat:tool_start']||[];
    for(const handler of starts) {
      await handler({payload:{session_id:'s1',id:'dup-tool-a',name:'exec_shell',args:{command:'same-command'}}});
      await handler({payload:{session_id:'s1',id:'dup-tool-b',name:'exec_shell',args:{command:'same-command'}}});
    }
  });
  await sleep(700);
  const ambiguousBeforeEnd = await page.evaluate(() => {
    const items=window.TauriBridge.state.getMany(['chat', 'vllm']).chatItems;
    return ['dup-tool-a','dup-tool-b'].map(id => {
      const item=items.find(it=>it.toolId===id); return item && item.taskId;
    });
  });
  await page.evaluate(async () => {
    const ends=window.__TAURI_EVENT_HANDLERS__['chat:tool_end']||[];
    for(const handler of ends) {
      await handler({payload:{session_id:'s1',id:'dup-tool-a',success:true,output:'running in background',metadata:{task_id:'task-dup-a',status:'Running'}}});
      await handler({payload:{session_id:'s1',id:'dup-tool-b',success:true,output:'running in background',metadata:{task_id:'task-dup-b',status:'Running'}}});
    }
  });
  await sleep(500);
  const shellIdentity = await page.evaluate(() => {
    const items=window.TauriBridge.state.getMany(['chat', 'vllm']).chatItems;
    const a=items.filter(it=>it.taskId==='task-dup-a');
    const b=items.filter(it=>it.taskId==='task-dup-b');
    const done=items.find(it=>it.taskId==='task-detached-done');
    return {a:a.map(it=>it.toolId),b:b.map(it=>it.toolId),done:done&&{state:done.state,output:done.output}};
  });
  rec('③d Shell 同命令 task_id 不错配、已结束 detached job 不丢',
    ambiguousBeforeEnd.every(value=>value==null) &&
    shellIdentity.a.length===1 && shellIdentity.a[0]==='dup-tool-a' &&
    shellIdentity.b.length===1 && shellIdentity.b[0]==='dup-tool-b' &&
    shellIdentity.done && shellIdentity.done.state==='done' && shellIdentity.done.output.includes('done fast'),
    JSON.stringify({ambiguousBeforeEnd,shellIdentity}));

  const unchangedPollNotifications = await page.evaluate(async () => {
    window.__PAUSE_BACKEND_STATUS_POLL__=true;
    let count=0;
    const unsubscribe=window.TauriBridge.state.subscribe('chat', () => { count+=1; });
    await new Promise(resolve=>setTimeout(resolve,650));
    unsubscribe();
    window.__PAUSE_BACKEND_STATUS_POLL__=false;
    return count;
  });
  rec('③d-2 Shell 快照未变化时不做全量状态广播', unchangedPollNotifications===0, String(unchangedPollNotifications));

  await page.evaluate(() => {
    window.__CANCEL_SHELL_ERROR__=true;
    const button=[...document.querySelectorAll('button')].find(node=>
      node.textContent.trim()==='取消' && node.parentElement && node.parentElement.textContent.includes('same-command'));
    if(button) button.click();
  });
  await sleep(400);
  const cancelFailureState = await page.evaluate(() => {
    const button=[...document.querySelectorAll('button')].find(node=>
      node.textContent.trim()==='取消' && node.parentElement && node.parentElement.textContent.includes('same-command'));
    return {visible:document.body.innerText.includes('取消失败: kill failed'),retryable:!!button&&!button.disabled};
  });
  rec('③e Shell 取消失败有卡片提示且不产生永久取消态',
    cancelFailureState.visible && cancelFailureState.retryable, JSON.stringify(cancelFailureState));
  await page.evaluate(() => { window.__CANCEL_SHELL_ERROR__=false; });

  // ⑤ 品悟检阅 modal 本地化渲染:threading t 不报错 + 裁决标签/trace 出现(i18n 回归)
  await page.evaluate(() => window.TauriBridge.interaction.summonPinvou('/home/x/会议纪要.md'));
  await sleep(900);
  const modal = await page.evaluate(() => {
    const txt = document.body.innerText;
    return { trace: txt.includes('有几点确认'), adopt: txt.includes('采纳建议'), skip: txt.includes('跳过'), persona: txt.includes('旅行规划') };
  });
  rec('⑤ 品悟检阅卡本地化渲染(t 线程通)', modal.trace && modal.adopt && modal.skip, JSON.stringify(modal));

  // ⑥ composer 模式 chip:渲染 + 默认 YOLO + 下拉两项 + 点 Plan 真切到 Plan(防 setPlanModeNext 草稿态静默 return 回归)
  const chip = await page.evaluate(() => {
    // title 前缀匹配:compact 下 chip 收成图标(无可见文字),且 title 现含当前模式名 → 用 title 判模式
    const b = document.querySelector('[title^="切换工作模式"]');
    if (!b) return { found: false };
    const label = (b.getAttribute('title') || '').trim();
    b.click();
    return { found: true, label };
  });
  await sleep(300);
  const chipMenu = await page.evaluate(() => {
    const txt = document.body.innerText;
    return { yoloDesc: txt.includes('直接动手执行'), planDesc: txt.includes('先出方案') };
  });
  await clickText(page, 'Plan');
  await sleep(700);
  const afterLabel = await page.evaluate(() => {
    const b = document.querySelector('[title^="切换工作模式"]');
    return b ? (b.getAttribute('title') || '').trim() : '';
  });
  rec('⑥ chip 渲染+下拉两项+点Plan切到Plan', chip.found && /YOLO/.test(chip.label || '') && chipMenu.yoloDesc && chipMenu.planDesc && /Plan/.test(afterLabel), JSON.stringify({ ...chip, ...chipMenu, afterLabel }));

  // ⑤b 收纳成功 toast 的「前往查看」必须直达对话管理页并展开「已收纳」面板，且按钮不折行。
  await expand(page); await sleep(200);
  const archiveMenuOpened = await page.evaluate(() => {
    const label = [...document.querySelectorAll('span')]
      .find(node => (node.textContent || '').trim() === '第三季度财报分析' && node.getBoundingClientRect().left < 330);
    const row = label && label.closest('div[class*="cursor-pointer"]');
    if (!row) return false;
    const rect = row.getBoundingClientRect();
    row.dispatchEvent(new MouseEvent('contextmenu', {
      bubbles: true,
      cancelable: true,
      clientX: rect.left + Math.min(rect.width - 12, 180),
      clientY: rect.top + rect.height / 2,
    }));
    return true;
  });
  await sleep(250);
  await clickText(page, '收纳');
  await sleep(250);
  await clickText(page, '确认收纳');
  await sleep(450);
  const archiveToastBefore = await page.evaluate(() => {
    const button = [...document.querySelectorAll('button')].find(node => (node.textContent || '').trim() === '前往查看');
    const rect = button && button.getBoundingClientRect();
    return {
      opened: !!button,
      noWrap: !!rect && rect.width >= 74 && rect.height <= 34,
      text: document.body.innerText.includes('已收纳到【对话管理-已收纳】'),
    };
  });
  await clickText(page, '前往查看');
  await sleep(600);
  const archiveToastGoto = await page.evaluate(() => ({
    currentView: document.querySelector('[data-testid="app-root"]')?.getAttribute('data-current-view'),
    archivedTabVisible: document.body.innerText.includes('已收纳'),
    archivedVisible: document.body.innerText.includes('第三季度财报分析'),
    noSettingsError: !document.body.innerText.includes('设置页加载失败'),
  }));
  rec('⑤b 收纳 toast 前往查看直达对话管理-已收纳且按钮不折行',
    archiveMenuOpened && archiveToastBefore.opened && archiveToastBefore.noWrap && archiveToastBefore.text &&
    archiveToastGoto.currentView === 'search' && archiveToastGoto.archivedTabVisible && archiveToastGoto.archivedVisible && archiveToastGoto.noSettingsError,
    JSON.stringify({ archiveMenuOpened, archiveToastBefore, archiveToastGoto }));

  // ⑥ 开机加载中不弹框；确认 stopped 后才渲染启用引导。
  await page.evaluate(() => { window.__VLLM_ELIGIBLE__ = true; window.__VLLM_STATE__ = 'starting'; });
  await page.evaluate(() => window.TauriBridge.vllm.detectLocalVllmSetup());
  await sleep(300);
  const startingSetup = await page.evaluate(() => document.body.innerText.includes('启用本地大模型'));
  rec('⑥ MegaCube 引擎 starting 时不弹启用框', !startingSetup, JSON.stringify({ popup: startingSetup }));

  // 将时钟推进到 12 分钟截止之后，等待内部自动轮询一次；卡死的 starting 必须恢复重试入口。
  await page.evaluate(() => {
    window.__VLLM_REAL_DATE_NOW__ = Date.now;
    const base = Date.now();
    Date.now = () => base + 13 * 60 * 1000;
  });
  await sleep(3300);
  const timedOutSetup = await page.evaluate(() => {
    const setup = window.TauriBridge.state.getMany(['chat', 'vllm']).vllmSetup || {};
    return {
      popup: document.body.innerText.includes('启用本地大模型'),
      state: setup.engine_state,
      timedOut: setup.detection_timed_out,
    };
  });
  rec('⑦ MegaCube 引擎 starting 超时后恢复重试入口', timedOutSetup.popup && timedOutSetup.state === 'failed' && timedOutSetup.timedOut === true, JSON.stringify(timedOutSetup));

  await page.evaluate(() => {
    Date.now = window.__VLLM_REAL_DATE_NOW__;
    window.__VLLM_STATE__ = 'stopped';
  });
  await page.evaluate(() => { window.__VLLM_ELIGIBLE__ = true; });
  await page.evaluate(() => window.TauriBridge.vllm.detectLocalVllmSetup());
  await sleep(500);
  const setup = await page.evaluate(() => {
    const btns = [...document.querySelectorAll('button')].map(b => (b.textContent || '').trim());
    return { title: document.body.innerText.includes('启用本地大模型'), enable: btns.includes('启用'), skip: btns.includes('暂不'), never: btns.includes('不再提醒') };
  });
  rec('⑧ MegaCube 引导框 eligible 渲染(标题+启用/暂不/不再提醒)', setup.title && setup.enable && setup.skip && setup.never, JSON.stringify(setup));

  // ⑨ 点「不再提醒」→ 二次确认子态(警示文案 + 确认不启用/再想想);点「再想想」回到初始态
  await clickText(page, '不再提醒');
  await sleep(300);
  const decline = await page.evaluate(() => {
    const btns = [...document.querySelectorAll('button')].map(b => (b.textContent || '').trim());
    return { warn: document.body.innerText.includes('不再自动弹出'), confirm: btns.includes('确认不启用'), reconsider: btns.includes('再想想') };
  });
  await clickText(page, '再想想');
  await sleep(200);
  rec('⑨ 不再提醒→二次确认渲染(警示+确认/再想想)', decline.warn && decline.confirm && decline.reconsider, JSON.stringify(decline));

  // ⑩ 点「启用」→ 立即进入等待系统授权；mock bootstrap 永不 resolve 停在进行中。
  await clickText(page, '启用');
  await sleep(700);
  const prog = await page.evaluate(() => {
    const txt = document.body.innerText;
    return { auth: txt.includes('等待系统授权'), wait: txt.includes('等待模型加载就绪'), elapsed: txt.includes('已等待') };
  });
  rec('⑩ 点启用后等待系统授权+计时渲染', prog.auth && prog.wait && prog.elapsed, JSON.stringify(prog));

  // 旧兼容渲染路径也必须暴露与详情 DOM 一致的展开状态。
  await page.evaluate(() => localStorage.setItem('pinvou_conversation_ui_v2', 'false'));
  await page.reload({ waitUntil: 'networkidle0' });
  await page.waitForFunction(() => window.TauriBridge && document.body && document.body.innerText.includes('PINVOU'), { timeout: 20000 }).catch(() => {});
  await sleep(1200);
  await expand(page); await sleep(200);
  await page.waitForSelector('[data-testid="codex-sidebar-item"]', { timeout: 10000 }).catch(() => {});
  await page.evaluate(() => document.querySelector('[data-testid="codex-sidebar-item"]')?.click());
  await sleep(500);
  const legacyConversationA11y = await page.evaluate(async () => {
    const state = (toggle) => {
      const controls = toggle?.getAttribute('aria-controls') || '';
      return {
        expanded: toggle?.getAttribute('aria-expanded') || '',
        controls,
        detailsPresent: Boolean(controls && document.getElementById(controls)),
      };
    };
    const settle = () => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const reasoningToggle = document.querySelector('[data-testid="conversation-reasoning-toggle"]');
    const reasoningBefore = state(reasoningToggle);
    reasoningToggle?.click();
    await settle();
    const reasoningAfter = state(reasoningToggle);
    reasoningToggle?.click();
    await settle();
    const reasoningCollapsed = state(reasoningToggle);

    const summary = document.querySelector('[data-testid="conversation-tool-group-summary"]');
    const groupBefore = state(summary);
    summary?.click();
    await settle();
    const groupAfter = state(summary);

    const compactToggle = document.querySelector('[data-testid="conversation-compact-item-toggle"]');
    const compactBefore = state(compactToggle);
    compactToggle?.click();
    await settle();
    const compactAfter = state(compactToggle);
    compactToggle?.click();
    await settle();
    const compactCollapsed = state(compactToggle);
    summary?.click();
    await settle();
    const groupCollapsed = state(summary);
    const controls = [reasoningAfter.controls, groupAfter.controls, compactAfter.controls].filter(Boolean);
    return {
      found: Boolean(reasoningToggle && summary && compactToggle),
      reasoningBefore,
      reasoningAfter,
      reasoningCollapsed,
      groupBefore,
      groupAfter,
      groupCollapsed,
      compactBefore,
      compactAfter,
      compactCollapsed,
      uniqueControls: controls.length === 3 && new Set(controls).size === controls.length,
    };
  });
  rec('⑩a 旧兼容对话详情向辅助技术同步展开状态',
    legacyConversationA11y.found
      && legacyConversationA11y.uniqueControls
      && legacyConversationA11y.reasoningBefore.expanded === 'false'
      && !legacyConversationA11y.reasoningBefore.controls
      && !legacyConversationA11y.reasoningBefore.detailsPresent
      && legacyConversationA11y.reasoningAfter.expanded === 'true'
      && Boolean(legacyConversationA11y.reasoningAfter.controls)
      && legacyConversationA11y.reasoningAfter.detailsPresent
      && legacyConversationA11y.reasoningCollapsed.expanded === 'false'
      && !legacyConversationA11y.reasoningCollapsed.controls
      && !legacyConversationA11y.reasoningCollapsed.detailsPresent
      && legacyConversationA11y.groupBefore.expanded === 'false'
      && !legacyConversationA11y.groupBefore.controls
      && !legacyConversationA11y.groupBefore.detailsPresent
      && legacyConversationA11y.groupAfter.expanded === 'true'
      && Boolean(legacyConversationA11y.groupAfter.controls)
      && legacyConversationA11y.groupAfter.detailsPresent
      && legacyConversationA11y.groupCollapsed.expanded === 'false'
      && !legacyConversationA11y.groupCollapsed.controls
      && !legacyConversationA11y.groupCollapsed.detailsPresent
      && legacyConversationA11y.compactBefore.expanded === 'false'
      && !legacyConversationA11y.compactBefore.controls
      && !legacyConversationA11y.compactBefore.detailsPresent
      && legacyConversationA11y.compactAfter.expanded === 'true'
      && Boolean(legacyConversationA11y.compactAfter.controls)
      && legacyConversationA11y.compactAfter.detailsPresent
      && legacyConversationA11y.compactCollapsed.expanded === 'false'
      && !legacyConversationA11y.compactCollapsed.controls
      && !legacyConversationA11y.compactCollapsed.detailsPresent,
    JSON.stringify(legacyConversationA11y));

  if (errs.length) console.log('⚠️ PAGEERRORS:', errs.slice(0, 3).join(' | '));
  await browser.close();

  const failed = results.filter(r => !r.pass).length;
  console.log(failed ? `\n❌ ${failed}/${results.length} FAILED` : `\n✅ ALL ${results.length} PASS`);
  process.exit(failed ? 1 : 0);
})().catch(e => { console.error('FATAL', e.message); process.exit(1); });
