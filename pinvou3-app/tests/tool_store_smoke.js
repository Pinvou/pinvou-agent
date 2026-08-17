#!/usr/bin/env node
/**
 * 工具商店真实浏览器旅程：加载 Vite dist，mock Tauri 命令/事件，但点击真实 React UI。
 * 覆盖 MCP 装卸、Obsidian 前置探测、配套技能联动、独立技能和五类授权连接器。
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
const CHROME = process.env.CHROME ||
  [
    '/snap/bin/chromium',
    '/usr/bin/chromium',
    '/usr/bin/chromium-browser',
    '/usr/bin/google-chrome',
    '/usr/bin/google-chrome-stable',
    path.join(process.env.ProgramFiles || 'C:\\Program Files', 'Google', 'Chrome', 'Application', 'chrome.exe'),
    path.join(process.env['ProgramFiles(x86)'] || 'C:\\Program Files (x86)', 'Google', 'Chrome', 'Application', 'chrome.exe'),
    path.join(process.env.LOCALAPPDATA || '', 'Google', 'Chrome', 'Application', 'chrome.exe'),
    path.join(process.env.ProgramFiles || 'C:\\Program Files', 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
    path.join(process.env['ProgramFiles(x86)'] || 'C:\\Program Files (x86)', 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
  ].filter(Boolean).find(fs.existsSync);
if (!CHROME) { console.error('SKIP: 未找到 chromium/chrome'); process.exit(2); }
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-tool-store-'));

function injectSource() {
  return `(function(){
    const TOOL_META={
      weather:['高德天气',[]],iwencai:['同花顺问财',[]],qcc:['企查查',[]],
      'patsnap-search':['智慧芽专利&文献融合检索',[]],
      'tencent-docs':['腾讯文档 MCP',['tencent-docs-skill']],
      'canva-mcp':['Canva 可画',[]],
      'yuandian-mcp':['华宇元典法律数据',[]],
      obsidian:['Obsidian 知识库',[]],pptx:['PPT 生成',[]],gongwen:['公文写作',['government-writing']]
    };
    const OAUTH_SERVERS={'yuandian-mcp':'yuandian_mcp','canva-mcp':'canva_mcp',qcc:'qcc-company'};
    const BLOCKING_INSTALL_OAUTH_TOOLS=new Set(['yuandian-mcp','canva-mcp']);
    const state=window.__TOOL_STORE_TEST__={
      installed:{},skills:{visualizer:false},connected:{feishu:false,wecom:false,dingtalk:false,tmeet:false,ima:false},
      oauthAuth:{},oauthRequests:{},finishOAuthInstall:null,calls:[],obsidianChecks:0,composerChanged:0
    };
    window.addEventListener('pinvou:tools-changed',()=>{state.composerChanged++;});
    window.__TAURI_EVENT_HANDLERS__={};
    const tools=()=>Object.entries(TOOL_META).map(([id,[name,companions]])=>({id,name,description:'test',version:'1.0.0',icon:'',category:'test',installed:!!state.installed[id],companion_skills:companions}));
    const skills=()=>[{id:'government-writing',title:'党政机关公文写作',installed:!!state.installed.gongwen,user_uploaded:false},{id:'visualizer',title:'数据分析可视化',installed:!!state.skills.visualizer,user_uploaded:false}];
    function record(cmd,args){state.calls.push({cmd,args:args||{}});}
    function invoke(cmd,args){
      record(cmd,args);
      switch(cmd){
        case 'get_settings': return Promise.resolve({theme:'liquid-light',language:'zh-Hans'});
        case 'get_selected_pet': return Promise.resolve('lingling');
        case 'get_effective_model_config': return Promise.resolve({model:'qwen36_35b_256k',base_url:'http://127.0.0.1:8000/v1',api_key_set:false});
        case 'get_app_version': return Promise.resolve('0.6.1');
        case 'list_models': return Promise.resolve({models:[],active_model_id:null});
        case 'list_scheduled_tasks': return Promise.resolve([]);
        case 'list_scheduled_task_recent_runs': return Promise.resolve([]);
        case 'list_sessions': return Promise.resolve([]);
        case 'get_super_permission_status': return Promise.resolve(false);
        case 'list_personas': return Promise.resolve([]);
        case 'get_backend_status': return Promise.resolve({online:true,ok:true,status:'online'});
        case 'check_for_update': return Promise.resolve({available:false});
        case 'find_resumable_run': return Promise.resolve(null);
        case 'list_workspace_files': case 'get_session_persona_events': case 'get_session_pinvou_reviews': return Promise.resolve([]);
        case 'get_mode_state': return Promise.resolve({mode:'yolo',plan_phase:'none'});
        case 'get_active_persona': return Promise.resolve(null);
        case 'detect_local_vllm_setup': return Promise.resolve({eligible:false});
        case 'list_marketplace_tools': return Promise.resolve(tools());
        case 'get_marketplace_tool_auth_status': {
          const installed=!!state.installed[args.toolId];
          return Promise.resolve(state.oauthAuth[args.toolId] || {
            installed,
            mcp_configured: installed,
            oauth_required: !!OAUTH_SERVERS[args.toolId],
            oauth_token_present: false,
            status: installed ? 'config_installed_auth_pending' : 'not_installed',
            server_name: OAUTH_SERVERS[args.toolId],
            message: installed ? '已写入 MCP 配置，但尚未完成 OAuth 授权。' : '尚未连接该工具。',
          });
        }
        case 'list_marketplace_skills': return Promise.resolve(skills());
        case 'install_marketplace_tool':
          if(BLOCKING_INSTALL_OAUTH_TOOLS.has(args.toolId)) return new Promise(resolve=>{state.finishOAuthInstall=()=>{state.installed[args.toolId]=true;state.finishOAuthInstall=null;resolve(null);};});
          state.installed[args.toolId]=true; return Promise.resolve(null);
        case 'uninstall_marketplace_tool': state.installed[args.toolId]=false; return Promise.resolve(null);
        case 'start_marketplace_tool_oauth_login': return new Promise(resolve=>{state.oauthRequests[args.requestId]={toolId:args.toolId,resolve};});
        case 'cancel_marketplace_tool_oauth_login': {
          const request=state.oauthRequests[args.requestId];
          if(request&&request.toolId===args.toolId){delete state.oauthRequests[args.requestId];request.resolve({status:'cancelled',message:'已取消等待浏览器授权',server_name:OAUTH_SERVERS[args.toolId]});}
          return Promise.resolve(true);
        }
        case 'install_marketplace_skill': state.skills[args.skillId]=true; return Promise.resolve(null);
        case 'uninstall_marketplace_skill': state.skills[args.skillId]=false; return Promise.resolve(null);
        case 'detect_obsidian': state.obsidianChecks++; return Promise.resolve(state.obsidianChecks===1?{state:'no_vault'}:{state:'ok',vault_path:'/tmp/test-vault'});
        case 'feishu_status': return Promise.resolve({connected:state.connected.feishu});
        case 'wecom_status': return Promise.resolve({connected:state.connected.wecom});
        case 'dingtalk_status': return Promise.resolve({connected:state.connected.dingtalk});
        case 'tmeet_status': return Promise.resolve({connected:state.connected.tmeet});
        case 'ima_status': return Promise.resolve({connected:state.connected.ima,credentials_present:state.connected.ima,skill_installed:state.connected.ima});
        case 'ima_connect': state.connected.ima=true;state.skills['ima-skills']=true;state.lastImaConnect=args; return Promise.resolve({ok:true,connected:true});
        case 'ima_logout': state.connected.ima=false;state.skills['ima-skills']=false; return Promise.resolve({ok:true,connected:false});
        case 'feishu_ensure_cli': case 'wecom_ensure_cli': case 'dingtalk_ensure_cli': case 'tmeet_ensure_cli': case 'feishu_connect_begin': case 'wecom_connect_begin': case 'dingtalk_connect_begin': case 'tmeet_connect_begin': return Promise.resolve(null);
        case 'feishu_apply_skills': case 'wecom_apply_skills': case 'dingtalk_apply_skills': case 'tmeet_apply_skills': case 'open_external_url': return Promise.resolve(null);
        default: return Promise.resolve(null);
      }
    }
    window.__emitTauri=async function(name,payload){for(const h of (window.__TAURI_EVENT_HANDLERS__[name]||[])) await h({payload:payload||{}});};
    window.__TAURI__={core:{invoke},event:{emit:function(name,payload){return window.__emitTauri(name,payload);},listen(name,handler){const hs=window.__TAURI_EVENT_HANDLERS__[name]||(window.__TAURI_EVENT_HANDLERS__[name]=[]);hs.push(handler);return Promise.resolve(()=>{const i=hs.indexOf(handler);if(i>=0)hs.splice(i,1);});}},
      window:{getCurrentWindow(){return {minimize(){},maximize(){},close(){},toggleMaximize(){},isMaximized(){return Promise.resolve(false);},onResized(){return Promise.resolve(()=>{});},startDragging(){}};}},dialog:{open(){return Promise.resolve(null);}}};
  })();`;
}

const sleep = ms => new Promise(r => setTimeout(r, ms));
const TOOL_STORE_SEARCH_SELECTOR = '[data-testid="tool-store-search"], input[placeholder="搜索连接器、skill、插件等"], input[placeholder="搜索 MCP、API 或工作流工具"]';
async function getToolStoreSearchInput(page) {
  const input = await page.$(TOOL_STORE_SEARCH_SELECTOR);
  if (input) return input;
  const handle = await page.evaluateHandle(() => (
    [...document.querySelectorAll('input')]
      .find(el => (el.getAttribute('placeholder') || '').includes('搜索'))
      || null
  ));
  const el = handle.asElement();
  if (!el) await handle.dispose();
  return el;
}
async function clickExact(page, text) {
  const ok = await page.evaluate(t => {
    const els = [...document.querySelectorAll('button,span,div,a')].filter(el => (el.textContent || '').trim() === t);
    const el = els[els.length - 1];
    if (!el) return false;
    el.scrollIntoView({block:'center'}); el.click(); return true;
  }, text);
  if (!ok) throw new Error(`找不到可点击文本: ${text}`);
}
async function search(page, query) {
  const input = await getToolStoreSearchInput(page);
  if (!input) throw new Error('工具商店搜索框未渲染');
  await input.click();
  // macOS 上 Ctrl+A 不触发全选,用原生 setter 清空并派发 input,保证跨平台一致
  await page.evaluate((el) => {
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(el, '');
    el.dispatchEvent(new Event('input', { bubbles: true }));
  }, input);
  await input.type(query);
  await sleep(180);
}
async function action(page, query, label, backendId = '') {
  await search(page, query);
  const ok = await page.evaluate((query, label, backendId) => {
    const buttons=[...document.querySelectorAll('button')].filter(b=>(b.textContent||'').trim()===label && !b.disabled);
    if (backendId) {
      const exact = buttons.find(b => b.getAttribute('data-tool-id') === backendId);
      if (exact) { exact.scrollIntoView({block:'center'}); exact.click(); return true; }
    }
    const button=buttons.find(b=>{let p=b;for(let i=0;i<7&&p;i++,p=p.parentElement)if((p.textContent||'').includes(query))return true;return false;}) || (buttons.length===1?buttons[0]:null);
    if(!button)return false;button.scrollIntoView({block:'center'});button.click();return true;
  }, query, label, backendId);
  if (!ok) throw new Error(`${query} 找不到操作按钮 ${label}`);
  await sleep(220);
}
async function dismiss(page) {
  const exists = await page.evaluate(() => [...document.querySelectorAll('button')].some(b => (b.textContent || '').trim() === '知道了'));
  if (exists) { await clickExact(page, '知道了'); await sleep(80); }
}
async function closeDetail(page, title) {
  await page.evaluate(title => {
    const heading=[...document.querySelectorAll('h2')].find(h=>(h.textContent||'').trim()===title);
    if(!heading)return;
    let modal=heading;
    while(modal&&!(modal.classList.contains('fixed')&&modal.classList.contains('inset-0')))modal=modal.parentElement;
    const close=modal&&[...modal.querySelectorAll('button')].find(b=>(b.textContent||'').trim()===''&&b.querySelector('svg'));
    if(close)close.click();
  },title);
  await sleep(100);
}

(async () => {
  const { url } = await startUiTestServer();
  const browser = await puppeteer.launch({executablePath:CHROME,headless:'new',args:['--no-sandbox','--disable-gpu','--no-first-run'],userDataDir:PROFILE});
  const page = await browser.newPage();
  const errors=[]; page.on('pageerror', e=>errors.push(e.message));
  await page.evaluateOnNewDocument(injectSource());
  await page.setViewport({width:1440,height:1000});
  await page.goto(url,{waitUntil:'networkidle0'});
  await page.waitForFunction(() => window.TauriBridge && document.querySelector('[data-nav="toolstore"]'), { timeout: 20000 }).catch(() => {});
  await sleep(500);
  const results=[];
  const rec=(name,pass,detail='')=>{results.push({name,pass});console.log(`${pass?'✅':'❌'} ${name}${detail?'  '+detail:''}`);};

  const navClicked = await page.evaluate(()=>{
    const el=document.querySelector('[data-nav="toolstore"]');
    if(!el)return false;
    el.dispatchEvent(new MouseEvent('click',{bubbles:true,cancelable:true}));
    return true;
  });
  await page.waitForFunction((selector) => (
    !!document.querySelector(selector)
    || [...document.querySelectorAll('input')].some(el => (el.getAttribute('placeholder') || '').includes('搜索'))
  ), { timeout: 10000 }, TOOL_STORE_SEARCH_SELECTOR).catch(() => {});
  const toolStoreLoaded = await page.evaluate((navClicked, selector)=>navClicked&&document.body.innerText.includes('工具商店')&&(
    !!document.querySelector(selector)
    || [...document.querySelectorAll('input')].some(el => (el.getAttribute('placeholder') || '').includes('搜索'))
  ), navClicked, TOOL_STORE_SEARCH_SELECTOR);
  const navDebug = toolStoreLoaded ? '' : await page.evaluate(() => JSON.stringify({
    currentView: document.querySelector('[data-testid="app-root"]')?.getAttribute('data-current-view') || null,
    navs: [...document.querySelectorAll('[data-nav]')].map(node => ({ nav: node.getAttribute('data-nav'), text: (node.textContent || '').trim(), title: node.getAttribute('title') || '' })),
    text: document.body.innerText.slice(0, 240),
  })).then(detail => JSON.stringify({ detail: JSON.parse(detail), errors: errors.slice(0, 5) }));
  rec('工具商店真实页面加载', toolStoreLoaded, navDebug);

  await action(page,'高德天气','配置','weather');
  rec('高德天气安装前展示必填 Key 配置',await page.evaluate(()=>{
    const input=document.querySelector('input[type="password"][placeholder="粘贴高德 Web 服务 Key"]');
    return document.body.innerText.includes('高德天气 Key')&&!!input;
  }));
  const weatherInput=await page.$('input[type="password"][placeholder="粘贴高德 Web 服务 Key"]');
  await weatherInput.type('amap-test-token');
  await clickExact(page,'连接'); await sleep(260); await dismiss(page);
  rec('高德天气经 Env 凭据配置连接',await page.evaluate(()=>{
    const call=[...window.__TOOL_STORE_TEST__.calls].reverse().find(x=>x.cmd==='install_marketplace_tool'&&x.args.toolId==='weather');
    return window.__TOOL_STORE_TEST__.installed.weather&&call?.args?.config?.AMAP_KEY==='amap-test-token';
  }));

  await action(page,'同花顺问财','配置','iwencai');
  rec('同花顺问财安装前展示必填 Key 配置',await page.evaluate(()=>{
    const input=document.querySelector('input[type="password"][placeholder="粘贴 IWENCAI_API_KEY"]');
    return document.body.innerText.includes('问财 Key')&&!!input;
  }));
  const iwencaiInput=await page.$('input[type="password"][placeholder="粘贴 IWENCAI_API_KEY"]');
  await iwencaiInput.type('iwencai-test-token');
  await clickExact(page,'连接'); await sleep(260); await dismiss(page);
  rec('同花顺问财经 Env 凭据配置连接',await page.evaluate(()=>{
    const call=[...window.__TOOL_STORE_TEST__.calls].reverse().find(x=>x.cmd==='install_marketplace_tool'&&x.args.toolId==='iwencai');
    return window.__TOOL_STORE_TEST__.installed.iwencai&&call?.args?.config?.IWENCAI_API_KEY==='iwencai-test-token';
  }));

  const composerChangedBeforeQcc = await page.evaluate(()=>window.__TOOL_STORE_TEST__.composerChanged);
  await action(page,'企查查','连接','qcc');
  rec('企查查走 OAuth 且不展示 API Key 输入',await page.evaluate(()=>document.body.innerText.includes('正在连接「企查查」')&&[...document.querySelectorAll('button')].some(b=>(b.textContent||'').trim()==='取消')&&!document.querySelector('input[type="password"]')));
  await clickExact(page,'取消'); await sleep(180);
  rec('企查查取消命令与授权请求使用同一 requestId',await page.evaluate(()=>{
    const calls=window.__TOOL_STORE_TEST__.calls;
    const start=[...calls].reverse().find(x=>x.cmd==='start_marketplace_tool_oauth_login'&&x.args.toolId==='qcc');
    const cancel=[...calls].reverse().find(x=>x.cmd==='cancel_marketplace_tool_oauth_login'&&x.args.toolId==='qcc');
    return !!start&&!!cancel&&start.args.requestId===cancel.args.requestId;
  }));
  rec('企查查取消授权后保持待授权态',await page.evaluate(()=>window.__TOOL_STORE_TEST__.installed.qcc&&[...document.querySelectorAll('button')].some(b=>(b.textContent||'').trim()==='重新授权')));
  rec('企查查未授权不通知 composer 刷新',await page.evaluate(before=>window.__TOOL_STORE_TEST__.composerChanged===before, composerChangedBeforeQcc));
  await dismiss(page);

  for (const [query,id,buttonId] of [['PPT 生成','pptx','pptx'],['公文写作','gongwen','government-writing']]) {
    await action(page,query,'安装',buttonId); await dismiss(page);
    const installed=await page.evaluate(id=>!!window.__TOOL_STORE_TEST__.installed[id],id);
    rec(`${query} 经 UI 安装`,installed);
  }

  await action(page,'智慧芽专利&文献','配置','patsnap-search');
  rec('智慧芽安装前展示 API Key 配置',await page.evaluate(()=>{
    const input=document.querySelector('input[type="password"][placeholder="粘贴你的智慧芽 API Key"]');
    return document.body.innerText.includes('填写智慧芽 API Key')&&!!input;
  }));
  const patsnapInput=await page.$('input[type="password"][placeholder="粘贴你的智慧芽 API Key"]');
  await patsnapInput.type('patsnap-test-token');
  await clickExact(page,'连接'); await sleep(260); await dismiss(page);
  rec('智慧芽经 Header 凭据配置连接',await page.evaluate(()=>{
    const call=[...window.__TOOL_STORE_TEST__.calls].reverse().find(x=>x.cmd==='install_marketplace_tool'&&x.args.toolId==='patsnap-search');
    return window.__TOOL_STORE_TEST__.installed['patsnap-search']&&call?.args?.config?.PATSNAP_API_KEY==='patsnap-test-token';
  }));

  await action(page,'腾讯文档 MCP','配置','tencent-docs');
  rec('腾讯文档安装前展示个人 Token 配置',await page.evaluate(()=>{
    const input=document.querySelector('input[type="password"][placeholder="粘贴腾讯文档个人 Token"]');
    return document.body.innerText.includes('连接腾讯文档')&&!!input;
  }));
  const tdocInput=await page.$('input[type="password"][placeholder="粘贴腾讯文档个人 Token"]');
  await tdocInput.type('tdoc-test-token');
  await clickExact(page,'连接'); await sleep(260); await dismiss(page);
  rec('腾讯文档经 Token 凭据配置连接',await page.evaluate(()=>{
    const call=[...window.__TOOL_STORE_TEST__.calls].reverse().find(x=>x.cmd==='install_marketplace_tool'&&x.args.toolId==='tencent-docs');
    return window.__TOOL_STORE_TEST__.installed['tencent-docs']&&call?.args?.config?.TENCENT_DOCS_TOKEN==='tdoc-test-token';
  }));

  await action(page,'腾讯 ima','配置','ima');
  rec('腾讯 ima 安装前展示 Client ID 和 API Key 配置',await page.evaluate(()=>{
    const client=document.querySelector('input[type="password"][placeholder="Client ID"]');
    const key=document.querySelector('input[type="password"][placeholder="API Key"]');
    return document.body.innerText.includes('连接腾讯 ima')&&!!client&&!!key;
  }));
  const imaClientInput=await page.$('input[type="password"][placeholder="Client ID"]');
  await imaClientInput.type('ima-client-test');
  const imaKeyInput=await page.$('input[type="password"][placeholder="API Key"]');
  await imaKeyInput.type('ima-api-test');
  await clickExact(page,'连接'); await sleep(260); await dismiss(page);
  rec('腾讯 ima 走 OpenAPI Skill 连接命令',await page.evaluate(()=>{
    const state=window.__TOOL_STORE_TEST__;
    const generic=state.calls.some(x=>x.cmd==='install_marketplace_tool'&&x.args.toolId==='ima');
    return state.connected.ima&&state.skills['ima-skills']&&state.lastImaConnect?.clientId==='ima-client-test'&&state.lastImaConnect?.apiKey==='ima-api-test'&&!generic;
  }));

  await action(page,'Obsidian 知识库','安装','obsidian');
  rec('Obsidian 缺库时先展示引导',await page.evaluate(()=>document.body.innerText.includes('还没有笔记库')));
  await clickExact(page,'我已新建，重新检测'); await sleep(250); await dismiss(page);
  rec('Obsidian 重检成功后安装',await page.evaluate(()=>!!window.__TOOL_STORE_TEST__.installed.obsidian&&window.__TOOL_STORE_TEST__.obsidianChecks===2));

  await search(page,'党政机关公文写作');
  rec('公文配套技能与 MCP 安装态联动',await page.evaluate(()=>[...document.querySelectorAll('button')].some(b=>(b.textContent||'').trim()==='卸载')));
  await action(page,'数据分析可视化','安装','visualizer'); await dismiss(page);
  rec('独立可视化技能经 UI 安装',await page.evaluate(()=>window.__TOOL_STORE_TEST__.skills.visualizer));

  await action(page,'高德天气','卸载','weather'); await dismiss(page);
  rec('MCP 经 UI 卸载并刷新状态',await page.evaluate(()=>!window.__TOOL_STORE_TEST__.installed.weather));

  const composerChangedBeforeYuandian = await page.evaluate(()=>window.__TOOL_STORE_TEST__.composerChanged);
  await action(page,'华宇元典法律数据','连接','yuandian-mcp');
  rec('元典写配置阶段不可取消',await page.evaluate(()=>document.body.innerText.includes('正在写入 MCP 配置')&&![...document.querySelectorAll('button')].some(b=>(b.textContent||'').trim()==='取消')));
  await page.evaluate(()=>window.__TOOL_STORE_TEST__.finishOAuthInstall()); await sleep(180);
  rec('元典 OAuth loading 弹窗可取消',await page.evaluate(()=>{
    const text = document.body.innerText;
    return (text.includes('正在连接元典法律') || text.includes('正在连接「华宇元典法律数据」'))
      && [...document.querySelectorAll('button')].some(b=>(b.textContent||'').trim()==='取消');
  }));
  await clickExact(page,'取消'); await sleep(180);
  rec('元典取消命令与授权请求使用同一 requestId',await page.evaluate(()=>{
    const calls=window.__TOOL_STORE_TEST__.calls;
    const start=[...calls].reverse().find(x=>x.cmd==='start_marketplace_tool_oauth_login');
    const cancel=[...calls].reverse().find(x=>x.cmd==='cancel_marketplace_tool_oauth_login');
    return !!start&&!!cancel&&start.args.toolId===cancel.args.toolId&&start.args.requestId===cancel.args.requestId;
  }));
  rec('元典取消授权后不显示已连接',await page.evaluate(()=>!document.body.innerText.includes('已连接「华宇元典法律数据」')));
  rec('元典取消授权后保持待授权态',await page.evaluate(()=>window.__TOOL_STORE_TEST__.installed['yuandian-mcp']&&[...document.querySelectorAll('button')].some(b=>(b.textContent||'').trim()==='重新授权')));
  rec('元典未授权不通知 composer 刷新',await page.evaluate(before=>window.__TOOL_STORE_TEST__.composerChanged===before, composerChangedBeforeYuandian));
  await dismiss(page);

  const composerChangedBeforeCanva = await page.evaluate(()=>window.__TOOL_STORE_TEST__.composerChanged);
  await action(page,'Canva 可画','连接','canva-mcp');
  rec('Canva 写配置阶段使用自身名称',await page.evaluate(()=>document.body.innerText.includes('正在连接「Canva 可画」')&&document.body.innerText.includes('正在写入 MCP 配置')));
  await page.evaluate(()=>window.__TOOL_STORE_TEST__.finishOAuthInstall()); await sleep(180);
  rec('Canva OAuth loading 弹窗可取消',await page.evaluate(()=>document.body.innerText.includes('正在连接「Canva 可画」')&&[...document.querySelectorAll('button')].some(b=>(b.textContent||'').trim()==='取消')));
  await clickExact(page,'取消'); await sleep(180);
  rec('Canva 取消命令保持工具与 requestId 一致',await page.evaluate(()=>{
    const calls=window.__TOOL_STORE_TEST__.calls;
    const start=[...calls].reverse().find(x=>x.cmd==='start_marketplace_tool_oauth_login');
    const cancel=[...calls].reverse().find(x=>x.cmd==='cancel_marketplace_tool_oauth_login');
    return !!start&&!!cancel&&start.args.toolId==='canva-mcp'&&cancel.args.toolId==='canva-mcp'&&start.args.requestId===cancel.args.requestId;
  }));
  rec('Canva 取消授权后保持待授权态',await page.evaluate(()=>window.__TOOL_STORE_TEST__.installed['canva-mcp']&&[...document.querySelectorAll('button')].some(b=>(b.textContent||'').trim()==='重新授权')));
  rec('Canva 未授权不通知 composer 刷新',await page.evaluate(before=>window.__TOOL_STORE_TEST__.composerChanged===before, composerChangedBeforeCanva));
  await dismiss(page);

  const connectors=[
    ['飞书（Lark）','feishu','feishu:connected',['feishu_ensure_cli','feishu_connect_begin']],
    ['企业微信','wecom','wecom:connected',['wecom_ensure_cli','wecom_connect_begin']],
    ['钉钉','dingtalk','dingtalk:connected',['dingtalk_ensure_cli','dingtalk_connect_begin','dingtalk_apply_skills']],
    ['腾讯会议','tmeet','tmeet:connected',['tmeet_ensure_cli','tmeet_connect_begin','tmeet_apply_skills']],
  ];
  for(const [query,id,event,commands] of connectors){
    await action(page,query,'连接',id);
    if(id==='tmeet'){
      await page.evaluate(() => window.__emitTauri('tmeet:qr', {
        phase: 'authorize',
        url: 'https://meeting.tencent.com/test-auth',
        qr_data_url: 'data:image/svg+xml;base64,PHN2Zy8+',
      }));
      await sleep(120);
      rec('腾讯会议收到授权 URL 后自动打开浏览器', await page.evaluate(() => {
        const call = [...window.__TOOL_STORE_TEST__.calls].reverse()
          .find(x => x.cmd === 'open_external_url');
        return call?.args?.url === 'https://meeting.tencent.com/test-auth'
          && document.body.innerText.includes('已打开浏览器登录页');
      }));
      const beforeApply = await page.evaluate(() => window.__TOOL_STORE_TEST__.calls
        .filter(x => x.cmd === 'tmeet_apply_skills').length);
      await page.evaluate(() => window.__emitTauri('tmeet:connected', {}));
      await sleep(180);
      rec('腾讯会议成功事件必须二次确认真实登录态', await page.evaluate((beforeApply) => {
        const afterApply = window.__TOOL_STORE_TEST__.calls
          .filter(x => x.cmd === 'tmeet_apply_skills').length;
        return afterApply === beforeApply
          && !document.body.innerText.includes('已连接腾讯会议')
          && document.body.innerText.includes('腾讯会议授权未完成');
      }, beforeApply));
    }
    await page.evaluate((id,event)=>{window.__TOOL_STORE_TEST__.connected[id]=true;return window.__emitTauri(event,{});},id,event);
    await sleep(180); await dismiss(page);
    const info=await page.evaluate(({commands})=>({calls:commands.every(c=>window.__TOOL_STORE_TEST__.calls.some(x=>x.cmd===c)),seen:window.__TOOL_STORE_TEST__.calls.map(x=>x.cmd)}),{commands});
    rec(`${query} 授权编排命令与成功事件`,info.calls,info.calls?'':JSON.stringify(info.seen.slice(-12)));
    await closeDetail(page,query);
  }

  const calls=await page.evaluate(()=>window.__TOOL_STORE_TEST__.calls);
  rec('用户 Key 工具安装调用携带对应配置',calls.filter(x=>x.cmd==='install_marketplace_tool').every(x=>{
    if(x.args.toolId==='weather')return Object.keys(x.args.config||{}).join(',')==='AMAP_KEY';
    if(x.args.toolId==='iwencai')return Object.keys(x.args.config||{}).join(',')==='IWENCAI_API_KEY';
    if(x.args.toolId==='patsnap-search')return Object.keys(x.args.config||{}).join(',')==='PATSNAP_API_KEY';
    if(x.args.toolId==='tencent-docs')return Object.keys(x.args.config||{}).join(',')==='TENCENT_DOCS_TOKEN';
    if(x.args.toolId==='ima')return false;
    return x.args&&x.args.toolId&&!x.args.config;
  }));
  rec('页面无未处理 JavaScript 异常',errors.length===0,errors.slice(0,2).join(' | '));

  await browser.close();
  fs.rmSync(PROFILE,{recursive:true,force:true});
  const failed=results.filter(r=>!r.pass).length;
  console.log(failed?`\n❌ ${failed}/${results.length} FAILED`:`\n✅ ALL ${results.length} PASS`);
  process.exit(failed?1:0);
})().catch(e=>{console.error('FATAL',e.stack||e);process.exit(1);});
