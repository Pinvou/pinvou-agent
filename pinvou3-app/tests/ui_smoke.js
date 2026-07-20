#!/usr/bin/env node
/**
 * pinvou3 前端 UI 冒烟回归测试 — headless chromium + mock TauriBridge,加载真 src/index.html。
 * 覆盖三条易回归的前端通路:
 *   ① resumeWorkflowOnBoot:有僵尸 run 时启动仍落草稿页(activeSessionId=null)+ 只挂看板,不劫持聊天会话。
 *   ② 工具商店渲染出 Obsidian 与钉钉连接器卡。
 *   ③ 聊天流产物卡(artifact_card)挂出「品/悟」召唤 pinvou 按钮。
 *   ④ 记忆候选事件渲染卡片，且确认/忽略/不再提示分别调用正确后端命令。
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
  ['/snap/bin/chromium', '/usr/bin/chromium', '/usr/bin/chromium-browser', '/usr/bin/google-chrome', '/usr/bin/google-chrome-stable'].find(p => fs.existsSync(p));
if (!CHROME) { console.error('SKIP: 未找到 chromium/chrome,可用 env CHROME=/path/to/chromium 指定'); process.exit(2); }
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-smoke-'));

// mock TauriBridge:find_resumable_run 返回僵尸 run;会话同时覆盖 write_file、
// MCP producer 和含 path 的 exec_shell 诊断结果，验证只有真实 producer 触发 artifact。
function injectSource() {
  return `(function(){
    window.__TAURI_EVENT_HANDLERS__={};
    window.__TAURI_INVOKES__=[];
    const ZOMBIE={session_id:'s-zombie',project_dir:'/x/wf',scenario:'sansheng_liubu'};
    const WF_STATE={project_dir:'/x/wf',scenario:'sansheng_liubu',all_completed:false,roles:{taizi:{name:'太子',status:'running'},zhongshu:{name:'中书',status:'pending'}}};
    const SESSIONS=[{id:'s1',title:'第三季度财报分析',created_at:1,updated_at:9}];
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
      {role:'user',content:[{type:'tool_result',tool_use_id:'t-mcp',content:'{"path":"/home/x/季度报告.pptx"}'}]}]}};
    function invoke(cmd,args){
      window.__TAURI_INVOKES__.push({cmd:cmd,args:args||{}});
      switch(cmd){
        case 'get_settings': return Promise.resolve({theme:'liquid-light',language:'zh-Hans'});
        case 'get_effective_model_config': return Promise.resolve({model:'qwen36_35b_256k',base_url:'http://127.0.0.1:8000/v1',api_key_set:false});
        case 'get_llmapi_status': return window.__LLMAPI_STATUS_ERROR__
          ? Promise.reject(new Error('temporary llmapi status failure'))
          : Promise.resolve(window.__LLMAPI_STATUS__ || {
              backend_user_exists:false,
              backend_user_state:'not_exists',
              stale:false
            });
        case 'get_llmapi_models': return window.__LLMAPI_MODELS_ERROR__
          ? Promise.reject(new Error('temporary llmapi models failure'))
          : Promise.resolve(window.__LLMAPI_MODELS__ || {available_models:[],default_model:''});
        case 'list_sessions': return Promise.resolve(SESSIONS);
        case 'get_super_permission_status': return Promise.resolve(false);
        case 'list_personas': return Promise.resolve([]);
        case 'get_backend_status': return Promise.resolve({online:true,ok:true,status:'online',model:'qwen36_35b_256k'});
        case 'get_memory_overview': return Promise.resolve({profile:null,preferences:[],work_context:[],current_focus:[],recent_activity:[],recent_work:[],pending:[],never:[],runtime:null,snapshot_path:''});
        case 'confirm_pending_memory': return Promise.resolve({value:true});
        case 'ignore_pending_memory': return Promise.resolve({value:true});
        case 'never_pending_memory': return Promise.resolve({value:true});
        case 'check_for_update': return Promise.resolve({available:false});
        case 'find_resumable_run': return Promise.resolve(ZOMBIE);
        case 'get_workflow_state': return Promise.resolve(WF_STATE);
        case 'start_workflow': return Promise.resolve({session_id:'s-kick-fail',project_dir:'/x/kick-fail'});
        case 'kick_workflow': return window.__KICK_WORKFLOW_ERROR__
          ? Promise.reject(new Error('模型服务预检失败：HTTP 401'))
          : Promise.resolve('spawning');
        case 'stop_workflow': window.__STOP_WORKFLOW_ARGS__=args; return Promise.resolve({ok:true,session_id:'s-zombie',scenario:'sansheng_liubu',brief:{user_request_raw:'原始三省六部需求'}});
        case 'check_dependencies': return Promise.resolve([]);
        case 'list_marketplace_tools': return Promise.resolve([]);
        case 'create_session': return Promise.resolve({id:'s-new',metadata:{id:'s-new'}});
        case 'set_plan_mode_next': return Promise.resolve({mode:'plan',plan_phase:'planning'});
        case 'exit_plan_to_yolo': return Promise.resolve({mode:'yolo',plan_phase:'none'});
        case 'get_mode_state': return Promise.resolve({mode:'yolo',plan_phase:'none'});
        case 'get_active_persona': return Promise.resolve(null);
        case 'list_workflows': return Promise.resolve([]);
        case 'list_workspace_files': return Promise.resolve([]);
        case 'get_session_persona_events': return Promise.resolve([]);
        case 'get_session_pinvou_reviews': return Promise.resolve([]);
        case 'summon_pinvou': return Promise.resolve({personas:[{id:'travel',label:'旅行规划',primary:true}],alternates:['budget'],trace:'看了下，有几点确认',recommendations:[{topic:'预算',pick:'中档',why:'稳妥'}],issues:[{severity:'high',kind:'quality',persona:'travel',text:'日期冲突',suggestion:'对齐'}],coverage:[],framework:[],risk:'medium',confidence:0.8});
        case 'load_session': return Promise.resolve(CONV[args&&args.id]||{metadata:{id:'x'},messages:[],artifacts:[]});
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
        case 'detect_local_vllm_setup': return Promise.resolve(window.__VLLM_ELIGIBLE__
          ? {eligible:true,is_megacube:true,has_packages:true,vllm_online:false,already_bootstrapped:false}
          : {eligible:false,is_megacube:false,has_packages:false,vllm_online:false,already_bootstrapped:false});
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
async function expand(page) { return page.evaluate(() => { const b = document.querySelector('[title*="侧边栏"],[title*="展开"]'); if (b) { b.click(); return true; } return false; }); }

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

  // ① 启动落草稿页(僵尸 run 不劫持)
  const st = await page.evaluate(() => {
    const s = window.TauriBridge.getState();
    return { activeSessionId: s.activeSessionId, wfActive: !!(s.workflow && s.workflow.run && s.workflow.run.active), wfSid: s.workflow && s.workflow.run && s.workflow.run.sessionId };
  });
  rec('① 僵尸run不劫持启动(落草稿页+挂看板)', (st.activeSessionId == null) && st.wfActive === true && st.wfSid === 's-zombie', JSON.stringify(st));

  const llmApiCacheFallback = await page.evaluate(async () => {
    window.__LLMAPI_STATUS__ = {
      backend_user_exists: true,
      backend_user_state: 'exists',
      stale: false,
      provisioning_status: 'ready',
    };
    window.__LLMAPI_MODELS__ = {
      available_models: ['deepseek-v4-flash'],
      default_model: 'deepseek-v4-flash',
    };
    await window.TauriBridge.getLlmApiStatus();
    await window.TauriBridge.getLlmApiModels();

    window.__LLMAPI_STATUS__ = {
      backend_user_exists: false,
      backend_user_state: 'unknown',
      stale: true,
      provisioning_status: 'not_started',
    };
    await window.TauriBridge.getLlmApiStatus();
    const retainedAfterUnknown = window.TauriBridge.getState();
    const keptOnUnknown = retainedAfterUnknown.llmApiStatus
      && retainedAfterUnknown.llmApiStatus.backend_user_state === 'exists';

    window.__LLMAPI_STATUS_ERROR__ = true;
    window.__LLMAPI_MODELS_ERROR__ = true;
    await window.TauriBridge.getLlmApiStatus().catch(() => {});
    await window.TauriBridge.getLlmApiModels().catch(() => {});
    const retained = window.TauriBridge.getState();
    const keptExisting = retained.llmApiStatus && retained.llmApiStatus.backend_user_exists === true;
    const keptModels = retained.llmApiModels && retained.llmApiModels.default_model === 'deepseek-v4-flash';

    window.__LLMAPI_STATUS_ERROR__ = false;
    window.__LLMAPI_STATUS__ = {
      backend_user_exists: false,
      backend_user_state: 'not_exists',
      stale: false,
      provisioning_status: 'not_started',
      last_error_code: 'service_disabled',
    };
    await window.TauriBridge.getLlmApiStatus();
    const clearedWhenAuthoritative = window.TauriBridge.getState().llmApiModels === null;
    return { keptOnUnknown, keptExisting, keptModels, clearedWhenAuthoritative };
  });
  rec('LLM API 暂时失败保留账户和模型缓存，仅明确不存在或禁用时清理',
    llmApiCacheFallback.keptOnUnknown && llmApiCacheFallback.keptExisting
      && llmApiCacheFallback.keptModels && llmApiCacheFallback.clearedWhenAuthoritative,
    JSON.stringify(llmApiCacheFallback));

  // 模型表单可能包含尚未保存的名称、地址和密钥，点击遮罩层不能意外丢失草稿；
  // 只有显式点击“取消”才关闭。
  const modelModalOpened = await page.evaluate(() => {
    const nav = document.querySelector('[data-testid="nav-settings"]');
    if (!nav) return false;
    nav.click();
    return true;
  });
  await sleep(700);
  const modelModalOutsideClick = await page.evaluate(async () => {
    const settle = () => new Promise(resolve => setTimeout(resolve, 50));
    const add = document.querySelector('[data-testid="settings-model-add"]');
    if (!add) return { addFound: false, opened: false, stayedOpen: false, cancelled: false };
    add.click();
    await settle();
    const backdrop = document.querySelector('[data-testid="model-form-backdrop"]');
    const opened = !!document.querySelector('[data-testid="model-form-dialog"]');
    if (backdrop) backdrop.click();
    await settle();
    const stayedOpen = !!document.querySelector('[data-testid="model-form-dialog"]');
    const cancel = document.querySelector('[data-testid="model-form-cancel"]');
    if (cancel) cancel.click();
    await settle();
    return {
      addFound: true,
      opened,
      stayedOpen,
      cancelled: !document.querySelector('[data-testid="model-form-dialog"]'),
    };
  });
  rec(
    '模型编辑弹窗点击外部不关闭且显式取消仍可关闭',
    modelModalOpened && modelModalOutsideClick.addFound && modelModalOutsideClick.opened
      && modelModalOutsideClick.stayedOpen && modelModalOutsideClick.cancelled,
    JSON.stringify(modelModalOutsideClick),
  );

  // ①b 工作流运行中可停止；停止后原需求自动进入新任务编辑框。
  page.on('dialog', async dialog => { await dialog.accept(); });
  await expand(page); await sleep(300);
  await page.evaluate(() => document.querySelector('[data-nav="workflow"]')?.click()); await sleep(700);
  const stopButton = await page.evaluate(() => {
    const button = document.querySelector('[data-testid="workflow-stop-restart"]');
    if (!button) return false;
    button.click();
    return true;
  });
  await sleep(700);
  const stopped = await page.evaluate(() => ({
    status: window.TauriBridge.getState().workflow.run.status,
    sessionId: window.__STOP_WORKFLOW_ARGS__ && window.__STOP_WORKFLOW_ARGS__.sessionId,
    brief: (document.querySelector('textarea') || {}).value || '',
  }));
  rec('①b 工作流可停止并预填原需求重开', stopButton && stopped.status === 'stopped' && stopped.sessionId === 's-zombie' && stopped.brief === '原始三省六部需求', JSON.stringify(stopped));

  // ①c stop marker 是最终状态：迟到快照即使仍带 running/reviewing，也不能让角色卡回跳。
  const stoppedAfterLateSnapshot = await page.evaluate(async () => {
    const handlers = window.__TAURI_EVENT_HANDLERS__['workflow:full_state'] || [];
    for (const handler of handlers) {
      await handler({ payload: {
        session_id: 's-zombie', stopped: true, project_dir: '/x/wf', scenario: 'sansheng_liubu',
        roles: { taizi: { name: '太子', status: 'running' }, zhongshu: { name: '中书', status: 'reviewing' } },
      } });
    }
    const run = window.TauriBridge.getState().workflow.run;
    return { status: run.status, taizi: run.agents.taizi.status, zhongshu: run.agents.zhongshu.status };
  });
  rec(
    '①c 已停止工作流不被迟到快照恢复为执行中',
    stoppedAfterLateSnapshot.status === 'stopped'
      && stoppedAfterLateSnapshot.taizi === 'stopped'
      && stoppedAfterLateSnapshot.zhongshu === 'stopped',
    JSON.stringify(stoppedAfterLateSnapshot),
  );
  await clickText(page, '取消'); await sleep(300);

  // ①d kick 失败必须 reject 给新建任务弹窗，不能把“项目已创建”误当成启动成功。
  const kickFailure = await page.evaluate(async () => {
    window.__KICK_WORKFLOW_ERROR__ = true;
    let error = '';
    try {
      await window.TauriBridge.startWorkflowTask('sansheng_liubu', { user_request_raw: '测试启动失败' });
    } catch (e) {
      error = String((e && e.message) || e);
    }
    window.__KICK_WORKFLOW_ERROR__ = false;
    return {
      error,
      calls: window.__TAURI_INVOKES__.filter(call => call.cmd === 'start_workflow' || call.cmd === 'kick_workflow').map(call => call.cmd),
    };
  });
  rec(
    '①d kick失败向调用方透传具体错误',
    kickFailure.error.includes('HTTP 401')
      && kickFailure.calls.slice(-2).join(',') === 'start_workflow,kick_workflow',
    JSON.stringify(kickFailure),
  );

  // ①e 后端 blocked 事件和持久化 full_state 都必须把看板置为 blocked，并显示原因。
  const blockedState = await page.evaluate(async () => {
    for (const handler of (window.__TAURI_EVENT_HANDLERS__['workflow:project_started'] || [])) {
      await handler({ payload: { session_id: 's-blocked', project_dir: '/x/blocked', scenario: 'sansheng_liubu' } });
    }
    for (const handler of (window.__TAURI_EVENT_HANDLERS__['workflow:blocked'] || [])) {
      await handler({ payload: { session_id: 's-blocked', status: 'blocked', stage: 'warmup', message: 'HTTP 401: authorization failed' } });
    }
    for (const handler of (window.__TAURI_EVENT_HANDLERS__['workflow:full_state'] || [])) {
      await handler({ payload: { session_id: 's-blocked', blocked: true, blocked_reason: 'HTTP 401: authorization failed', roles: { taizi: { status: 'pending' } } } });
    }
    const run = window.TauriBridge.getState().workflow.run;
    return {
      status: run.status,
      blockedCards: (run.cards || []).filter(card => card.workflowBlocked).map(card => card.text),
    };
  });
  rec(
    '①e 预热失败显示权威阻塞状态且不重复错误卡',
    blockedState.status === 'blocked'
      && blockedState.blockedCards.length === 1
      && blockedState.blockedCards[0].includes('HTTP 401'),
    JSON.stringify(blockedState),
  );

  // 手机先向尚未在桌面打开的后台 session 发消息：hydration 必须先把磁盘 messages
  // 重建成 chatItems；否则桌面随后切入时只剩这条手机消息，历史和产物卡都像“丢了”。
  await page.evaluate(async () => {
    const handlers = window.__TAURI_EVENT_HANDLERS__['remote_control:mobile_user_message'] || [];
    for (const handler of handlers) {
      await handler({ payload: { session_id: 's1', content: '手机补充消息', client_message_id: 'cm-regression' } });
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
    const artifactPaths = window.TauriBridge.getState().chatItems
      .filter(item => item.type === 'artifact_card')
      .map(item => item.path);
    const restoredShellCards = window.TauriBridge.getState().chatItems.filter(item =>
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
    const item = window.TauriBridge.getState().chatItems.find(item => item.toolId === 'live-shell');
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
    const item = window.TauriBridge.getState().chatItems.find(item => item.toolId === 'live-shell-wait');
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
    const item = window.TauriBridge.getState().chatItems.find(item => item.toolId === 'split-terminal');
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
    const item = window.TauriBridge.getState().chatItems.find(item => item.toolId === 'split-terminal-stderr');
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
    const item = window.TauriBridge.getState().chatItems.find(item => item.toolId === 'background-shell');
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
    const item = window.TauriBridge.getState().chatItems.find(item => item.toolId === 'background-shell');
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
  const liveArtifacts = await page.evaluate(() => window.TauriBridge.getState().artifacts.map(item => item.path));
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
    const item=window.TauriBridge.getState().chatItems.find(it=>it.taskId==='task-live-1');
    return item && {state:item.state,taskId:item.taskId,output:item.output};
  });
  await page.evaluate(() => window.TauriBridge.cancelShellTask('s1','task-live-1'));
  await sleep(700);
  const shellCancelled = await page.evaluate(() => {
    const item=window.TauriBridge.getState().chatItems.find(it=>it.taskId==='task-live-1');
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
    const items=window.TauriBridge.getState().chatItems;
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
    const items=window.TauriBridge.getState().chatItems;
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
    let count=0;
    const unsubscribe=window.TauriBridge.subscribe(() => { count+=1; });
    await new Promise(resolve=>setTimeout(resolve,650));
    unsubscribe();
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
  await page.evaluate(() => window.TauriBridge.summonPinvou('/home/x/会议纪要.md'));
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

  // ⑦ MegaCube(GB10) 本地大模型引导框:eligible 时渲染标题+启用/暂不/不再提醒(默认 mock 返 eligible:false 不弹,这里末尾翻 __VLLM_ELIGIBLE__ 再手动触发 detect,不污染前序测试)
  await page.evaluate(() => { window.__VLLM_ELIGIBLE__ = true; });
  await page.evaluate(() => window.TauriBridge.detectLocalVllmSetup());
  await sleep(500);
  const setup = await page.evaluate(() => {
    const btns = [...document.querySelectorAll('button')].map(b => (b.textContent || '').trim());
    return { title: document.body.innerText.includes('启用本地大模型'), enable: btns.includes('启用'), skip: btns.includes('暂不'), never: btns.includes('不再提醒') };
  });
  rec('⑦ MegaCube 引导框 eligible 渲染(标题+启用/暂不/不再提醒)', setup.title && setup.enable && setup.skip && setup.never, JSON.stringify(setup));

  // ⑧ 点「不再提醒」→ 二次确认子态(警示文案 + 确认不启用/再想想);点「再想想」回到初始态
  await clickText(page, '不再提醒');
  await sleep(300);
  const decline = await page.evaluate(() => {
    const btns = [...document.querySelectorAll('button')].map(b => (b.textContent || '').trim());
    return { warn: document.body.innerText.includes('不再自动弹出'), confirm: btns.includes('确认不启用'), reconsider: btns.includes('再想想') };
  });
  await clickText(page, '再想想');
  await sleep(200);
  rec('⑧ 不再提醒→二次确认渲染(警示+确认/再想想)', decline.warn && decline.confirm && decline.reconsider, JSON.stringify(decline));

  // ⑨ 点「启用」→ 进行中步骤指示渲染(授权/等待步骤 + 计时;mock bootstrap 永不 resolve 停在进行中)
  await clickText(page, '启用');
  await sleep(700);
  const prog = await page.evaluate(() => {
    const txt = document.body.innerText;
    return { auth: txt.includes('授权并启动引擎'), wait: txt.includes('等待模型加载就绪'), elapsed: txt.includes('已等待') };
  });
  rec('⑨ 引导进行中步骤指示+计时渲染', prog.auth && prog.wait && prog.elapsed, JSON.stringify(prog));

  if (errs.length) console.log('⚠️ PAGEERRORS:', errs.slice(0, 3).join(' | '));
  await browser.close();

  const failed = results.filter(r => !r.pass).length;
  console.log(failed ? `\n❌ ${failed}/${results.length} FAILED` : `\n✅ ALL ${results.length} PASS`);
  process.exit(failed ? 1 : 0);
})().catch(e => { console.error('FATAL', e.message); process.exit(1); });
