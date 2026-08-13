#!/usr/bin/env node
/**
 * 工具商店技能包上传冒烟:加载 Vite dist + mock Tauri(desktop,可写权限),
 * 验证 header「上传技能包」按钮触发导入、成功弹窗、列表展示上传技能 description、
 * 拖放 zip 走字节通道 import_skill_package_bytes。
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
    function invoke(cmd, args){
      window.__PINVOU_MOCK_CALLS__.push({cmd: cmd, args: args || {}});
      switch(cmd){
        case 'get_settings': return Promise.resolve({theme:'liquid-light',language:'zh-Hans'});
        case 'get_selected_pet': return Promise.resolve('lingling');
        case 'get_effective_model_config': return Promise.resolve({model:'m',base_url:'http://127.0.0.1:8000/v1',api_key_set:false});
        case 'get_app_version': return Promise.resolve('0.8.0');
        case 'get_backend_status': return Promise.resolve({online:true,ok:true,status:'online'});
        case 'check_for_update': return Promise.resolve({available:false});
        case 'get_mode_state': return Promise.resolve({mode:'yolo',plan_phase:'none'});
        case 'get_super_permission_status': return Promise.resolve(false);
        case 'detect_local_vllm_setup': return Promise.resolve({eligible:false});
        case 'list_marketplace_tools': return Promise.resolve([]);
        case 'get_marketplace_tool_auth_status': return Promise.resolve({status:'not_installed'});
        case 'list_marketplace_skills': return Promise.resolve([
          {id:'government-writing',title:'党政机关公文写作',installed:false,user_uploaded:false},
          {id:'my-test-skill',title:'my-test-skill',description:'用大模型整理会议纪要',installed:true,user_uploaded:true,subtitle:''},
        ]);
        case 'import_skill_package': return Promise.resolve(true);
        case 'import_skill_package_bytes': return Promise.resolve(true);
        case 'open_external_url': return Promise.resolve(null);
        default: return Promise.resolve(null);
      }
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
  try {
    const page = await browser.newPage();
    await page.evaluateOnNewDocument(injectSource());
    await page.goto(url, { waitUntil: 'networkidle0' });
    await page.waitForFunction(() => document.querySelector('[data-nav="toolstore"]'), { timeout: 20000 });
    await page.evaluate(() => { document.querySelector('[data-nav="toolstore"]').dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true })); });
    await page.waitForFunction(() => document.body.innerText.includes('工具商店'), { timeout: 10000 });

    // 1. header 上传按钮存在
    const hasBtn = await page.evaluate(() => !!document.querySelector('[data-testid="tool-store-upload-btn"]'));
    rec('header「上传技能包」按钮渲染', hasBtn);

    // 2. 点击按钮 → import_skill_package 被调用 → 成功弹窗
    const clicked = await page.evaluate(() => { document.querySelector('[data-testid="tool-store-upload-btn"]').click(); return true; });
    rec('点击上传按钮', !!clicked);
    await sleep(500);
    const btnCall = await page.evaluate(() => window.__PINVOU_MOCK_CALLS__.filter(c => c.cmd === 'import_skill_package').length);
    rec('按钮触发 import_skill_package', btnCall >= 1);
    const importedToast = await page.evaluate(() => document.body.innerText.includes('技能包已导入'));
    rec('导入成功弹窗「技能包已导入」', importedToast);

    // 3. 列表视图展示上传技能;点击列表项 → 详情弹窗显示 description
    rec('切换到列表视图', await clickExact(page, '列表'));
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

    // 4. 拖放 zip → 走 import_skill_package_bytes,filename/dataBase64 正确
    const dropSent = await page.evaluate(async () => {
      const zipBytes = new Uint8Array([0x50, 0x4b, 0x03, 0x04, 1, 2, 3]); // 'PK\x03\x04'
      const file = new File([zipBytes], 'my-skill.zip', { type: 'application/zip' });
      const dt = new DataTransfer();
      dt.items.add(file);
      document.dispatchEvent(new DragEvent('drop', { dataTransfer: dt, bubbles: true, cancelable: true }));
      await new Promise(r => setTimeout(r, 600));
      const call = window.__PINVOU_MOCK_CALLS__.find(c => c.cmd === 'import_skill_package_bytes');
      if (!call) return { ok: false, why: 'no call' };
      let decoded = null;
      try { decoded = atob(call.args.dataBase64); } catch (_) {}
      const correctName = call.args.filename === 'my-skill.zip';
      const correctBytes = decoded === 'PK\x03\x04' + String.fromCharCode(1, 2, 3);
      return { ok: correctName && correctBytes, why: JSON.stringify({ name: call.args.filename, bytesOk: correctBytes }) };
    });
    rec('拖放 zip 触发 import_skill_package_bytes', dropSent.ok, dropSent.why);
    const dropToast = await page.evaluate(() => document.body.innerText.includes('技能包已导入'));
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
