#!/usr/bin/env node
/**
 * 输入框工具菜单 smoke：验证独立“技能”按钮已消失，技能合并进“工具”菜单。
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
  path.join(process.env.ProgramFiles || 'C:\\Program Files', 'Google', 'Chrome', 'Application', 'chrome.exe'),
  path.join(process.env['ProgramFiles(x86)'] || 'C:\\Program Files (x86)', 'Google', 'Chrome', 'Application', 'chrome.exe'),
  path.join(process.env.LOCALAPPDATA || '', 'Google', 'Chrome', 'Application', 'chrome.exe'),
  path.join(process.env.ProgramFiles || 'C:\\Program Files', 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
  path.join(process.env['ProgramFiles(x86)'] || 'C:\\Program Files (x86)', 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
].filter(Boolean);
const CHROME = process.env.CHROME ||
  chromeCandidates.find(fs.existsSync);
if (!CHROME) { console.error('SKIP: 未找到 chromium/chrome'); process.exit(2); }
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-composer-tools-'));

function injectSource() {
  return `(function(){
    const state=window.__COMPOSER_TOOLS_TEST__={calls:[],disabled:[],disabledSkills:[]};
    function record(cmd,args){state.calls.push({cmd,args:args||{}});}
    function invoke(cmd,args){
      record(cmd,args);
      switch(cmd){
        case 'get_settings': return Promise.resolve({theme:'liquid-light',language:'zh-Hans'});
        case 'get_effective_model_config': return Promise.resolve({model:'qwen36_35b_256k',base_url:'http://127.0.0.1:8000/v1',api_key_set:false});
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
        case 'list_marketplace_tools': return Promise.resolve([
          {id:'gongwen',name:'公文写作',description:'公文工具',installed:true,companion_skills:['government-writing']},
        ]);
        case 'list_marketplace_skills': return Promise.resolve([
          {id:'government-writing',title:'党政机关公文写作',description:'配套技能',installed:true,user_uploaded:false},
          {id:'visualizer',title:'数据分析可视化',description:'Chart.js 仪表盘',installed:true,user_uploaded:false},
        ]);
        case 'get_disabled_connectors': return Promise.resolve(state.disabled);
        case 'set_disabled_connectors': state.disabled=(args&&args.connectorIds)||[]; return Promise.resolve(null);
        case 'get_disabled_skills': return Promise.resolve(state.disabledSkills);
        case 'set_disabled_skills': state.disabledSkills=(args&&args.skillIds)||[]; return Promise.resolve(null);
        case 'weibo_skills_state': return Promise.resolve({connected:true,enabled:true});
        case 'feishu_skills_state': case 'wecom_skills_state': case 'dingtalk_skills_state': case 'tmeet_skills_state': return Promise.resolve({connected:false,enabled:true});
        default: return Promise.resolve(null);
      }
    }
    window.__TAURI__={core:{invoke},event:{listen(){return Promise.resolve(function(){});},emit(){return Promise.resolve();}},
      window:{getCurrentWindow(){return {minimize(){},maximize(){},close(){},toggleMaximize(){},isMaximized(){return Promise.resolve(false);},onResized(){return Promise.resolve(function(){});},startDragging(){}};}},
      dialog:{open(){return Promise.resolve(null);}}};
  })();`;
}

const sleep = ms => new Promise(r => { setTimeout(r, ms); });

(async () => {
  const { url } = await startUiTestServer();
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

  const before = await page.evaluate(() => ({
    toolButton: !!document.querySelector('button[title="工具"]'),
    skillButton: !!document.querySelector('button[title="技能"]'),
  }));
  rec('输入框存在工具按钮', before.toolButton, JSON.stringify(before));
  rec('输入框不存在独立技能按钮', !before.skillButton, JSON.stringify(before));

  await page.evaluate(() => document.querySelector('button[title="工具"]').click());
  await sleep(300);
  const menu = await page.evaluate(() => document.body.innerText);
  rec('工具菜单包含内置视觉设计', menu.includes('视觉设计') && menu.includes('内置·自动'));
  rec('工具菜单包含已连接微博服务', menu.includes('微博') && menu.includes('已连接'));
  rec('独立技能出现在工具菜单', menu.includes('数据分析可视化'));
  rec('companion 技能不重复展示', !menu.includes('党政机关公文写作'));
  rec('所属 MCP 工具仍展示', menu.includes('公文写作'));

  await page.evaluate(() => document.querySelector('button[aria-label="visualizer"]').click());
  await sleep(150);
  const disabled = await page.evaluate(() => window.__COMPOSER_TOOLS_TEST__.disabled);
  rec('关闭独立技能调用 set_disabled_connectors(裸 id)', disabled.includes('visualizer') && !disabled.some(id => id.startsWith('skill:')), JSON.stringify(disabled));

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
