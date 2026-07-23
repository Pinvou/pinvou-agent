#!/usr/bin/env node
/**
 * pinvou3 前端 UI 冒烟回归测试 — headless chromium + mock TauriBridge,加载真 src/index.html。
 * 覆盖三条易回归的前端通路:
 *   ① resumeWorkflowOnBoot:有僵尸 run 时启动仍落草稿页(activeSessionId=null)+ 只挂看板,不劫持聊天会话。
 *   ② 工具商店渲染出 Obsidian 与钉钉连接器卡。
 *   ③ 聊天流产物卡(artifact_card)挂出「品/悟」召唤 pinvou 按钮。
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

// mock TauriBridge:find_resumable_run 返回僵尸 run;会话同时覆盖 write_file、
// MCP producer 和含 path 的 exec_shell 诊断结果，验证只有真实 producer 触发 artifact。
function injectSource() {
  return `(function(){
    window.__TAURI_EVENT_HANDLERS__={};
    const ZOMBIE={session_id:'s-zombie',project_dir:'/x/wf',scenario:'sansheng_liubu'};
    const WF_STATE={project_dir:'/x/wf',scenario:'sansheng_liubu',all_completed:false,roles:{taizi:{name:'太子',status:'running'},zhongshu:{name:'中书',status:'pending'}}};
    const WF_TEMPLATE={id:'sansheng-liubu',name:'三省六部帮你办',enabled:true,scenarios:['sansheng_liubu'],ui:{header:'🏛️ 三省六部帮你办',template:{title:'🏛️ 三省六部帮你办',badge:'11 agent',desc:'太子接旨 → 中书省起草 → 门下省审议 → 尚书省派单 → 六部并行办差 → 回奏呈报。'},agentDefs:[{id:'taizi',name:'太子',color:'#C9A227'},{id:'zhongshu',name:'中书省',color:'#4285F4'}],lanes:[{lane:0,title:'接旨',agents:['taizi']},{lane:1,title:'起草',agents:['zhongshu']}]}};
    let SESSIONS=[{id:'s1',title:'第三季度财报分析',created_at:1,updated_at:9}];
    let ARCHIVED_SESSIONS=[];
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
      switch(cmd){
        case 'get_settings': return Promise.resolve({theme:'liquid-light',language:'zh-Hans'});
        case 'get_effective_model_config': return Promise.resolve({model:'qwen36_35b_256k',base_url:'http://127.0.0.1:8000/v1',api_key_set:false});
        case 'list_sessions': return Promise.resolve(SESSIONS);
        case 'list_archived_sessions': return Promise.resolve(ARCHIVED_SESSIONS);
        case 'get_super_permission_status': return Promise.resolve(false);
        case 'list_personas': return Promise.resolve([]);
        case 'get_backend_status': return Promise.resolve({online:true,ok:true,status:'online',model:'qwen36_35b_256k'});
        case 'check_for_update': return Promise.resolve({available:false});
        case 'find_resumable_run': return Promise.resolve(ZOMBIE);
        case 'get_workflow_state': return Promise.resolve(WF_STATE);
        case 'stop_workflow': window.__STOP_WORKFLOW_ARGS__=args; return Promise.resolve({ok:true,session_id:'s-zombie',scenario:'sansheng_liubu',brief:{user_request_raw:'原始三省六部需求'}});
        case 'check_dependencies': return Promise.resolve([]);
        case 'list_marketplace_tools': return Promise.resolve([]);
        case 'create_session': return Promise.resolve({id:'s-new',metadata:{id:'s-new'}});
        case 'set_session_archived':
          if (args && args.archived) {
            const session = SESSIONS.find(function(s){ return s.id === args.id; }) || { id: args.id, title: '第三季度财报分析', created_at: 1, updated_at: 9 };
            SESSIONS = SESSIONS.filter(function(s){ return s.id !== args.id; });
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
        case 'list_workflows': return Promise.resolve([WF_TEMPLATE]);
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
  page.on('pageerror', e => errs.push(e.message));
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
  rec('①a-1 任务筛选弹层只保留有效筛选与排序项',
    sidebarTaskFilterMenu.exists && sidebarTaskFilterMenu.hasAll && sidebarTaskFilterMenu.hasPinned &&
    sidebarTaskFilterMenu.hasScheduled && sidebarTaskFilterMenu.hasPinnedFirst && sidebarTaskFilterMenu.hasRecent &&
    !sidebarTaskFilterMenu.hasCurrentChat && !sidebarTaskFilterMenu.hasRegularChat,
    JSON.stringify(sidebarTaskFilterMenu));
  await page.keyboard.press('Escape'); await sleep(200);

  // ①b 工作流入口已合入专家池：从「专家池 > 专家团队」进入三省六部运行态，
  // 仍可停止并预填原需求重开。
  page.on('dialog', async dialog => { await dialog.accept(); });
  await expand(page); await sleep(300);
  await page.evaluate(() => document.querySelector('[data-nav="cardpool"]')?.click()); await sleep(700);
  const expertPoolShell = await page.evaluate(() => {
    const text = document.body.innerText;
    return {
      hasWorkflowNav: !!document.querySelector('[data-nav="workflow"]'),
      hasExpertPool: text.includes('专家池'),
      hasIndividualTab: text.includes('个人专家'),
      hasTeamTab: text.includes('专家团队'),
      view: document.querySelector('[data-testid="app-root"]')?.getAttribute('data-current-view'),
    };
  });
  rec('①b-0 工作流入口合入专家池并显示双 Tab',
    !expertPoolShell.hasWorkflowNav && expertPoolShell.hasExpertPool && expertPoolShell.hasIndividualTab &&
    expertPoolShell.hasTeamTab && expertPoolShell.view === 'cardpool',
    JSON.stringify(expertPoolShell));
  await clickText(page, '专家团队'); await sleep(700);
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
    await emit('chat:tool_end', { session_id:'s1', id:'live-shell', success:true, output:'{"ok":true,"path":"live-validator-fake.html"}' });
    await emit('chat:tool_start', { session_id:'s1', id:'live-mcp', name:'mcp_gongwen_make_gongwen', args:{} });
    await emit('chat:tool_end', { session_id:'s1', id:'live-mcp', success:true, output:'{"path":"live-report.docx"}' });
  });
  const liveArtifacts = await page.evaluate(() => window.TauriBridge.getState().artifacts.map(item => item.path));
  rec('③b 实时 tool_end 不跟踪 shell path、保留 MCP 产物', liveArtifacts.includes('live-report.docx') && !liveArtifacts.includes('live-validator-fake.html'), JSON.stringify(liveArtifacts));

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

  // ④ 品悟检阅 modal 本地化渲染:threading t 不报错 + 裁决标签/trace 出现(i18n 回归)
  await page.evaluate(() => window.TauriBridge.summonPinvou('/home/x/会议纪要.md'));
  await sleep(900);
  const modal = await page.evaluate(() => {
    const txt = document.body.innerText;
    return { trace: txt.includes('有几点确认'), adopt: txt.includes('采纳建议'), skip: txt.includes('跳过'), persona: txt.includes('旅行规划') };
  });
  rec('④ 品悟检阅卡本地化渲染(t 线程通)', modal.trace && modal.adopt && modal.skip, JSON.stringify(modal));

  // ⑤ composer 模式 chip:渲染 + 默认 YOLO + 下拉两项 + 点 Plan 真切到 Plan(防 setPlanModeNext 草稿态静默 return 回归)
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
  rec('⑤ chip 渲染+下拉两项+点Plan切到Plan', chip.found && /YOLO/.test(chip.label || '') && chipMenu.yoloDesc && chipMenu.planDesc && /Plan/.test(afterLabel), JSON.stringify({ ...chip, ...chipMenu, afterLabel }));

  // ⑤b 收纳成功 toast 的「前往查看」必须直达设置页数据管理，且按钮不折行。
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
      text: document.body.innerText.includes('已收纳到【设置-任务收纳】'),
    };
  });
  await clickText(page, '前往查看');
  await sleep(600);
  const archiveToastGoto = await page.evaluate(() => ({
    currentView: document.querySelector('[data-testid="app-root"]')?.getAttribute('data-current-view'),
    title: [...document.querySelectorAll('h1')].some(node => (node.textContent || '').trim() === '数据管理'),
    archivedVisible: document.body.innerText.includes('第三季度财报分析'),
    noSettingsError: !document.body.innerText.includes('设置页加载失败'),
  }));
  rec('⑤b 收纳 toast 前往查看直达设置-数据管理且按钮不折行',
    archiveMenuOpened && archiveToastBefore.opened && archiveToastBefore.noWrap && archiveToastBefore.text &&
    archiveToastGoto.currentView === 'settings' && archiveToastGoto.title && archiveToastGoto.archivedVisible && archiveToastGoto.noSettingsError,
    JSON.stringify({ archiveMenuOpened, archiveToastBefore, archiveToastGoto }));

  // ⑥ 开机加载中不弹框；确认 stopped 后才渲染启用引导。
  await page.evaluate(() => { window.__VLLM_ELIGIBLE__ = true; window.__VLLM_STATE__ = 'starting'; });
  await page.evaluate(() => window.TauriBridge.detectLocalVllmSetup());
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
    const setup = window.TauriBridge.getState().vllmSetup || {};
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
  await page.evaluate(() => window.TauriBridge.detectLocalVllmSetup());
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

  if (errs.length) console.log('⚠️ PAGEERRORS:', errs.slice(0, 3).join(' | '));
  await browser.close();

  const failed = results.filter(r => !r.pass).length;
  console.log(failed ? `\n❌ ${failed}/${results.length} FAILED` : `\n✅ ALL ${results.length} PASS`);
  process.exit(failed ? 1 : 0);
})().catch(e => { console.error('FATAL', e.message); process.exit(1); });
