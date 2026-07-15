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
    const CONV={s1:{metadata:{id:'s1'},artifacts:['/home/x/会议纪要.md'],messages:[
      {role:'user',content:[{type:'text',text:'整理纪要'}]},
      {role:'assistant',content:[{type:'text',text:'已生成会议纪要。'},{type:'tool_use',id:'t1',name:'write_file',input:{path:'/home/x/会议纪要.md',content:'# 会议纪要'}}]},
      {role:'user',content:[{type:'tool_result',tool_use_id:'t1',content:'written'}]},
      {role:'assistant',content:[{type:'tool_use',id:'t-shell',name:'exec_shell',input:{command:'python validator.py --json'}}]},
      {role:'user',content:[{type:'tool_result',tool_use_id:'t-shell',content:'{"ok":true,"path":"/home/x/validator-fake.html"}'}]},
      {role:'assistant',content:[{type:'tool_use',id:'t-mcp',name:'mcp_pptx_make_pptx',input:{title:'季度报告'}}]},
      {role:'user',content:[{type:'tool_result',tool_use_id:'t-mcp',content:'{"path":"/home/x/季度报告.pptx"}'}]}]}};
    function invoke(cmd,args){
      window.__TAURI_INVOKES__.push({cmd:cmd,args:args||{}});
      switch(cmd){
        case 'get_settings': return Promise.resolve({theme:'liquid-light',language:'zh-Hans'});
        case 'get_effective_model_config': return Promise.resolve({model:'qwen36_35b_256k',base_url:'http://127.0.0.1:8000/v1',api_key_set:false});
        case 'get_llmapi_status': return window.__LLMAPI_STATUS_ERROR__
          ? Promise.reject(new Error('temporary llmapi status failure'))
          : Promise.resolve(window.__LLMAPI_STATUS__ || null);
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
    window.__TAURI__={core:{invoke:invoke},event:{listen:function(name,handler){
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
  page.on('pageerror', e => errs.push(e.message));
  await page.evaluateOnNewDocument(injectSource());
  await page.setViewport({ width: 1440, height: 1000, deviceScaleFactor: 1 });
  await page.goto(INDEX, { waitUntil: 'networkidle0' });
  await page.waitForFunction(() => window.TauriBridge && document.body && document.body.innerText.includes('PINVOU'), { timeout: 20000 }).catch(() => {});
  await sleep(2000);

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
    };
    await window.TauriBridge.getLlmApiStatus();
    const clearedWhenAuthoritative = window.TauriBridge.getState().llmApiModels === null;
    return { keptExisting, keptModels, clearedWhenAuthoritative };
  });
  rec('LLM API 暂时失败保留账户和模型缓存，仅明确不存在时清理',
    llmApiCacheFallback.keptExisting && llmApiCacheFallback.keptModels && llmApiCacheFallback.clearedWhenAuthoritative,
    JSON.stringify(llmApiCacheFallback));

  // 手机先向尚未在桌面打开的后台 session 发消息：hydration 必须先把磁盘 messages
  // 重建成 chatItems；否则桌面随后切入时只剩这条手机消息，历史和产物卡都像“丢了”。
  await page.evaluate(async () => {
    const handlers = window.__TAURI_EVENT_HANDLERS__['remote_control:mobile_user_message'] || [];
    for (const handler of handlers) {
      await handler({ payload: { session_id: 's1', content: '手机补充消息', client_message_id: 'cm-regression' } });
    }
  });

  // ② 工具商店关键连接器卡
  await expand(page); await sleep(500);
  await clickText(page, '工具商店');
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
    return {
      history: text.includes('整理纪要'),
      mobile: text.includes('手机补充消息'),
      mcpArtifact: artifactPaths.some(path => String(path).includes('季度报告.pptx')),
      shellFakeArtifact: artifactPaths.some(path => String(path).includes('validator-fake.html')),
    };
  });
  rec('③ 后台session恢复时仅 MCP producer 结果生成产物卡', hit >= 2 && restored.history && restored.mobile && restored.mcpArtifact && !restored.shellFakeArtifact, JSON.stringify({ hit, ...restored }));

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
      session_id:'s1', tool_id:'background-shell', task_id:'shell-bg-1', status:'Killed', exit_code:null
    }});
  });
  const backgroundKilled = await page.evaluate(() => {
    const item = window.TauriBridge.getState().chatItems.find(item => item.toolId === 'background-shell');
    return item && item.state === 'failed' && item.shellStatus === 'Killed' && item.background === false;
  });
  rec('background shell terminal event closes the independent tool lifecycle', backgroundKilled);
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
