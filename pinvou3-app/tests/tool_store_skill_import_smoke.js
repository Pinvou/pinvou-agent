#!/usr/bin/env node
/**
 * 工具商店技能包上传冒烟:加载 Vite dist + mock Tauri(desktop,可写权限),
 * 验证 header「上传技能包」按钮触发导入、成功弹窗、列表展示上传技能 description、
 * 拖放 zip 走字节通道 import_plugin_package_bytes_cmd。
 * 前置:先 npm run build:ui。
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
    path.join(process.env.ProgramFiles || 'C:\\Program Files', 'Google', 'Chrome', 'Application', 'chrome.exe'),
    path.join(process.env.LOCALAPPDATA || '', 'Google', 'Chrome', 'Application', 'chrome.exe'),
    path.join(process.env.ProgramFiles || 'C:\\Program Files', 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
  ].filter(Boolean).find(fs.existsSync);
if (!CHROME) { console.error('SKIP: 未找到 chromium/chrome'); process.exit(2); }

function injectSource() {
  return `(function(){
    window.__TAURI_EVENT_HANDLERS__={};
    window.__PINVOU_MOCK_CALLS__=[];
    var handlers={
      get_settings:function(){return {theme:'liquid-light',language:'zh-Hans'};},
      get_selected_pet:function(){return 'lingling';},
      get_effective_model_config:function(){return {model:'m',base_url:'http://127.0.0.1:8000/v1',api_key_set:false};},
      get_app_version:function(){return '0.8.0';},
      get_backend_status:function(){return {online:true,ok:true,status:'online'};},
      check_for_update:function(){return {available:false};},
      get_mode_state:function(){return {mode:'yolo',plan_phase:'none'};},
      get_super_permission_status:function(){return false;},
      detect_local_vllm_setup:function(){return {eligible:false};},
      list_marketplace_tools:function(){return [];},
      get_marketplace_tool_auth_status:function(){return {status:'not_installed'};},
      list_marketplace_skills:function(){return [
        {id:'government-writing',title:'党政机关公文写作',installed:false,user_uploaded:false},
        {id:'my-test-skill',title:'my-test-skill',description:'用大模型整理会议纪要',installed:true,user_uploaded:true,subtitle:''},
      ];},
      import_plugin_package_cmd:function(){return true;},
      import_plugin_package_bytes_cmd:function(){return true;},
      uninstall_marketplace_skill:function(){return null;},
      open_external_url:function(){return null;},
      // 启动期只读命令:null 即走既有回退分支(与旧 default resolve(null) 行为一致)。
      bundle_readiness:function(){return null;},
      dingtalk_skills_state:function(){return null;},
      feishu_skills_state:function(){return null;},
      get_bundle_visibility:function(){return null;},
      get_disabled_connectors:function(){return null;},
      get_mode_defaults:function(){return null;},
      get_monitor_snapshot:function(){return null;},
      get_platform_capabilities:function(){return null;},
      get_project_skills_enabled:function(){return null;},
      kb_model_load_after_first_frame:function(){return null;},
      kb_model_status:function(){return null;},
      list_archived_sessions:function(){return null;},
      list_models:function(){return null;},
      list_personas:function(){return null;},
      list_scheduled_runs:function(){return null;},
      list_scheduled_tasks:function(){return null;},
      list_sessions:function(){return null;},
      refresh_connector_auth_gates:function(){return null;},
      report_frontend_startup:function(){return null;},
      report_pending_update_result:function(){return null;},
      take_pet_navigation:function(){return null;},
      take_pet_reply:function(){return null;},
      tmeet_skills_state:function(){return null;},
      web_access_bridge_ready:function(){return null;},
      web_access_status:function(){return null;},
      wecom_skills_state:function(){return null;},
    };
    function invoke(cmd, args){
      window.__PINVOU_MOCK_CALLS__.push({cmd: cmd, args: args || {}});
      if (Object.prototype.hasOwnProperty.call(handlers, cmd)) return Promise.resolve(handlers[cmd](args));
      // 未注册的命令直接 reject：防止前端命令名漂移再被 default 假绿掩盖。
      return Promise.reject(new Error('unregistered command: ' + cmd));
    }
    window.__TAURI__={core:{invoke},event:{emit:function(){return Promise.resolve();},listen(){return Promise.resolve(()=>{});}}};
  })();`;
}

const sleep = ms => new Promise(r => setTimeout(r, ms));
let failures = 0;
const rec = (name, ok, debug) => { console.log(`${ok ? '✅' : '❌'} ${name}${ok ? '' : (debug ? ' :: ' + debug : '')}`); if (!ok) failures++; };
async function clickExact(page, text) {
  return page.evaluate((t) => {
    const els = [...document.querySelectorAll('button,span,div,a')].filter(el => (el.textContent || '').trim() === t);
    const el = els[els.length - 1];
    if (!el) return false;
    el.scrollIntoView({ block: 'center' }); el.click(); return true;
  }, text);
}

(async () => {
  const { url } = await startUiTestServer();
  const browser = await puppeteer.launch({ executablePath: CHROME, headless: 'new', args: ['--no-sandbox'] });
  let page;
  try {
    page = await browser.newPage();
    await page.evaluateOnNewDocument(injectSource());
    await page.goto(url, { waitUntil: 'networkidle0' });
    await page.waitForFunction(() => document.querySelector('[data-nav="toolstore"]'), { timeout: 20000 });
    await page.evaluate(() => { document.querySelector('[data-nav="toolstore"]').dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true })); });
    await page.waitForFunction(() => document.body.innerText.includes('插件中心'), { timeout: 10000 });

    // 1. header 上传按钮存在
    const hasBtn = await page.evaluate(() => !!document.querySelector('[data-testid="tool-store-upload-btn"]'));
    rec('header「上传技能包」按钮渲染', hasBtn);

    // 2. 点击按钮 → import_plugin_package_cmd 被调用 → 成功弹窗
    const clicked = await page.evaluate(() => { document.querySelector('[data-testid="tool-store-upload-btn"]').click(); return true; });
    rec('点击上传按钮', !!clicked);
    await sleep(500);
    const btnCall = await page.evaluate(() => window.__PINVOU_MOCK_CALLS__.filter(c => c.cmd === 'import_plugin_package_cmd').length);
    rec('按钮触发 import_plugin_package_cmd', btnCall >= 1);
    const importedToast = await page.evaluate(() => document.body.innerText.includes('插件包已导入'));
    rec('导入成功弹窗「技能包已导入」', importedToast);

    // 3. 列表视图(唯一视图)展示上传技能;点击列表项 → 详情弹窗显示 description
    await sleep(400);
    const listed = await page.evaluate(() => document.body.innerText.includes('my-test-skill'));
    rec('列表渲染上传技能条目', listed);
    const openedDetail = await page.evaluate(() => {
      const els = [...document.querySelectorAll('div')].filter(el => (el.textContent || '').includes('my-test-skill'));
      const el = els[els.length - 1];
      if (!el) return false;
      el.click(); return true;
    });
    rec('点击列表项打开详情', !!openedDetail);
    await sleep(300);
    const descShown = await page.evaluate(() => document.body.innerText.includes('用大模型整理会议纪要'));
    rec('详情弹窗渲染上传技能 description', descShown);
    // 关闭详情弹窗(点蒙层)
    await page.evaluate(() => { const els = [...document.querySelectorAll('div')]; const m = els.find(el => el.onclick && String(el.onclick).includes('setSelectedTool')); if (m) m.click(); });
    await sleep(300);

    // 4. description 参与搜索
    await page.type('[data-testid="tool-store-search"]', '整理会议纪要');
    await sleep(300);
    const searchHit = await page.evaluate(() => document.body.innerText.includes('my-test-skill'));
    rec('搜索命中上传技能(description 参与检索)', searchHit);
    // 清空搜索回到正常态
    await page.evaluate(() => { const s = document.querySelector('[data-testid="tool-store-search"]'); s.value=''; s.dispatchEvent(new Event('input',{bubbles:true})); });
    await sleep(300);

    // 4. 拖放 zip → 走 import_plugin_package_bytes_cmd,filename/dataBase64 正确
    const dropSent = await page.evaluate(async () => {
      const zipBytes = new Uint8Array([0x50, 0x4b, 0x03, 0x04, 1, 2, 3]); // 'PK\x03\x04'
      const file = new File([zipBytes], 'my-skill.zip', { type: 'application/zip' });
      const dt = new DataTransfer();
      dt.items.add(file);
      document.dispatchEvent(new DragEvent('drop', { dataTransfer: dt, bubbles: true, cancelable: true }));
      await new Promise(r => setTimeout(r, 600));
      const call = window.__PINVOU_MOCK_CALLS__.find(c => c.cmd === 'import_plugin_package_bytes_cmd');
      if (!call) return { ok: false, why: 'no call' };
      let decoded = null;
      try { decoded = atob(call.args.dataBase64); } catch (_) {}
      const correctName = call.args.filename === 'my-skill.zip';
      const correctBytes = decoded === 'PK\x03\x04' + String.fromCharCode(1, 2, 3);
      return { ok: correctName && correctBytes, why: JSON.stringify({ name: call.args.filename, bytesOk: correctBytes }) };
    });
    rec('拖放 zip 触发 import_plugin_package_bytes_cmd', dropSent.ok, dropSent.why);
    const dropToast = await page.evaluate(() => document.body.innerText.includes('插件包已导入'));
    rec('拖放导入成功弹窗', dropToast);

    // 5. 上传技能可从 UI 卸载(路由必须命中 skill 分支而非通用工具分支)
    const uninstalled = await page.evaluate(async () => {
      const rows = [...document.querySelectorAll('div')].filter(el =>
        (el.textContent || '').includes('my-test-skill') && el.querySelector('button'));
      const row = rows[rows.length - 1];
      if (!row) return { ok: false, why: 'no row' };
      const btn = row.querySelector('button');
      if (!btn) return { ok: false, why: 'no button' };
      btn.click();
      await new Promise(r => setTimeout(r, 600));
      const call = window.__PINVOU_MOCK_CALLS__.find(c => c.cmd === 'uninstall_marketplace_skill');
      return { ok: !!call && call.args.skillId === 'my-test-skill', why: JSON.stringify(call && call.args) };
    });
    rec('上传技能可卸载(uninstall_marketplace_skill 命中)', uninstalled.ok, uninstalled.why);
  } finally {
    await browser.close();
  }
  console.log(failures ? `\n❌ ${failures} FAIL` : '\n✅ ALL PASS');
  process.exit(failures ? 1 : 0);
})().catch(e => { console.error(e); process.exit(1); });
