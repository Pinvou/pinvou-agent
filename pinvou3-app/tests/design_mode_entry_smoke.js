#!/usr/bin/env node
/**
 * 工作 / 设计 / 代码三模式入口 smoke。
 * 依赖先运行 `npm run build:ui`。
 */
const fs = require('fs');
const os = require('os');
const path = require('path');
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
const CHROME = process.env.CHROME || chromeCandidates.find(fs.existsSync);
if (!CHROME) { console.error('SKIP: 未找到 chromium/chrome'); process.exit(2); }
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-design-mode-entry-'));

function injectSource() {
  return `(function(){
    var HTML_PATH = '/tmp/pinvou3/sessions/s-design/artifacts/landing.html';
    var HTML_CONTENT = '<!doctype html><html><body><main id="app"><section class="hero"><h1 class="hero-title">Pinvou Design</h1><button class="primary">Start</button></section></main></body></html>';
    var SESSIONS = [{id:'s-design',title:'HTML设计测试',created_at:1,updated_at:9}];
    var CONV = { 's-design': {
      metadata:{id:'s-design',title:'HTML设计测试'},
      artifacts:[{path:HTML_PATH,basename:'landing.html'}],
      messages:[{role:'user',content:[{type:'text',text:'做一个 landing page'}]}]
    }};
    function invoke(cmd,args){
      switch(cmd){
        case 'get_settings': return Promise.resolve({theme:'liquid-light',language:'zh-Hans'});
        case 'get_effective_model_config': return Promise.resolve({model:'qwen36_35b_256k',base_url:'http://127.0.0.1:8000/v1',api_key_set:false});
        case 'list_sessions': return Promise.resolve(SESSIONS);
        case 'load_session': return Promise.resolve(CONV[args && args.id] || CONV['s-design']);
        case 'get_super_permission_status': return Promise.resolve(false);
        case 'list_personas': return Promise.resolve([]);
        case 'get_backend_status': return Promise.resolve({online:true,ok:true,status:'online'});
        case 'check_for_update': return Promise.resolve({available:false});
        case 'find_resumable_run': return Promise.resolve(null);
        case 'list_workflows': case 'list_workspace_files': case 'get_session_persona_events': case 'get_session_pinvou_reviews': return Promise.resolve([]);
        case 'get_mode_state': return Promise.resolve({mode:'yolo',plan_phase:'none'});
        case 'get_active_persona': return Promise.resolve(null);
        case 'detect_local_vllm_setup': return Promise.resolve({eligible:false});
        case 'list_marketplace_tools': case 'list_marketplace_skills': case 'get_disabled_connectors': return Promise.resolve([]);
        case 'artifact_info': return Promise.resolve({exists:true,kind:'html',size:HTML_CONTENT.length,modified:1});
        case 'read_artifact_text': return Promise.resolve(HTML_CONTENT);
        case 'render_artifact_visual': return Promise.resolve({mode:'unsupported'});
        default: return Promise.resolve(null);
      }
    }
    window.__TAURI__={core:{invoke},event:{listen(){return Promise.resolve(function(){});},emit(){return Promise.resolve();}},
      window:{getCurrentWindow(){return {minimize(){},maximize(){},close(){},toggleMaximize(){},isMaximized(){return Promise.resolve(false);},onResized(){return Promise.resolve(function(){});},startDragging(){}};}},
      dialog:{open(){return Promise.resolve(null);}}};
  })();`;
}

const sleep = ms => new Promise(r => setTimeout(r, ms));

async function clickExactButton(page, text) {
  return page.evaluate((text) => {
    const node = [...document.querySelectorAll('button,div,span,a')]
      .find(item => (item.textContent || '').trim() === text);
    if (!node) return false;
    const target = node.closest('button,[role="button"],div[class*="cursor-pointer"],a') || node;
    target.click();
    return true;
  }, text);
}

(async () => {
  const { url } = await startUiTestServer();
  const browser = await puppeteer.launch({ executablePath: CHROME, headless: 'new', args: ['--no-sandbox', '--disable-gpu', '--no-first-run'], userDataDir: PROFILE });
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', e => errors.push(e.stack || e.message));
  await page.evaluateOnNewDocument(injectSource());
  await page.setViewport({ width: 1360, height: 900 });
  await page.goto(url, { waitUntil: 'networkidle0' });
  await sleep(1500);

  const results = [];
  const rec = (name, pass, detail = '') => { results.push({ name, pass }); console.log(`${pass ? '✅' : '❌'} ${name}${detail ? '  ' + detail : ''}`); };

  const initial = await page.evaluate(() => {
    const switcher = document.querySelector('[data-testid="pinvou-mode-switcher"]');
    const textarea = document.querySelector('textarea');
    return {
      switcher: !!switcher,
      hasWork: !!switcher && switcher.textContent.includes('工作'),
      hasDesign: !!switcher && switcher.textContent.includes('设计'),
      hasCode: !!switcher && switcher.textContent.includes('代码'),
      placeholder: textarea && textarea.getAttribute('placeholder'),
    };
  });
  rec('默认渲染工作/设计/代码入口', initial.switcher && initial.hasWork && initial.hasDesign && initial.hasCode, JSON.stringify(initial));

  await page.evaluate(() => window.TauriBridge && window.TauriBridge.sessions && window.TauriBridge.sessions.switchToSession('s-design'));
  await sleep(900);
  await clickExactButton(page, '产物与代码');
  await sleep(400);
  await clickExactButton(page, 'landing.html');
  await sleep(900);

  const designClicked = await clickExactButton(page, '设计');
  await sleep(700);
  const design = await page.evaluate(() => {
    const textarea = document.querySelector('textarea');
    return {
      statusHidden: !document.querySelector('[data-testid="design-mode-status"]'),
      placeholder: textarea && textarea.getAttribute('placeholder'),
    };
  });
  rec('切换设计模式后隐藏状态条并展示设计 placeholder',
    designClicked && design.statusHidden && design.placeholder === '描述你想怎么调整选中的元素',
    JSON.stringify(design));

  const frameHandle = await page.$('[data-testid="artifact-html-preview-frame"]');
  const frame = frameHandle && await frameHandle.contentFrame();
  if (frame) {
    const title = await frame.$('h1.hero-title');
    if (title) {
      const box = await title.boundingBox();
      if (box) {
        await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
        await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
        await sleep(500);
      }
    }
  }
  const selectedStatus = await page.evaluate(() => document.querySelector('[data-testid="design-selected-element"]')?.textContent || '');
  rec('设计 runtime 注入后可选中 iframe 内元素',
    !!frame && selectedStatus.includes('h1') && selectedStatus.includes('h1.hero-title'),
    JSON.stringify({ selectedStatus }));

  await page.click('[data-testid="design-text-input"]');
  await page.keyboard.down('Control');
  await page.keyboard.press('A');
  await page.keyboard.up('Control');
  await page.keyboard.type('Pinvou 可视化编辑');
  await page.keyboard.press('Enter');
  await sleep(350);
  await page.click('[data-testid="design-font-size-input"]');
  await page.keyboard.down('Control');
  await page.keyboard.press('A');
  await page.keyboard.up('Control');
  await page.keyboard.type('40');
  await page.keyboard.press('Tab');
  await sleep(350);
  await page.$eval('[data-testid="design-color-input"]', (input) => {
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, '#007aff');
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
  });
  await sleep(500);

  const edited = frame ? await frame.evaluate(() => {
    const h1 = document.querySelector('h1.hero-title');
    const style = h1 ? getComputedStyle(h1) : null;
    return {
      text: h1 && h1.textContent,
      fontSize: style && style.fontSize,
      color: style && style.color,
    };
  }) : null;
  const changesLog = await page.evaluate(() => {
    const log = document.querySelector('[data-testid="design-changes-log"]');
    return { exists: !!log, text: log && log.textContent };
  });
  rec('设计面板可临时修改文案、字号和颜色并记录 changes log',
    edited && edited.text === 'Pinvou 可视化编辑' && edited.fontSize === '40px' &&
      /0,\s*122,\s*255/.test(edited.color || '') &&
      changesLog.exists && changesLog.text.includes('设计变更') && changesLog.text.includes('fontSize') && changesLog.text.includes('color'),
    JSON.stringify({ edited, changesLog }));

  await page.click('[data-testid="design-clear-changes"]');
  await sleep(500);
  const cleared = frame ? await frame.evaluate(() => {
    const h1 = document.querySelector('h1.hero-title');
    const style = h1 ? getComputedStyle(h1) : null;
    return {
      text: h1 && h1.textContent,
      fontSize: style && style.fontSize,
      color: style && style.color,
    };
  }) : null;
  const logAfterClear = await page.evaluate(() => !!document.querySelector('[data-testid="design-changes-log"]'));
  rec('清空修改后恢复预览并清空 changes log',
    cleared && cleared.text === 'Pinvou Design' && cleared.fontSize === '32px' &&
      /0,\s*0,\s*0/.test(cleared.color || '') && !logAfterClear,
    JSON.stringify({ cleared, logAfterClear }));

  const codeClicked = await clickExactButton(page, '代码');
  await sleep(250);
  const code = await page.evaluate(() => {
    const textarea = document.querySelector('textarea');
    const picker = document.querySelector('[data-testid="code-agent-picker"]');
    return {
      picker: !!picker,
      pickerText: picker && picker.textContent,
      placeholder: textarea && textarea.getAttribute('placeholder'),
    };
  });
  rec('切换代码模式后展示 Agent 选择和代码 placeholder',
    codeClicked && code.picker && code.pickerText.includes('Codex') && code.pickerText.includes('Claude Code') && code.pickerText.includes('Kimi Code') && code.placeholder === '描述要交给代码 Agent 的修改',
    JSON.stringify(code));

  await page.click('[data-testid="code-agent-provider-codex"]');
  await sleep(150);
  const persisted = await page.evaluate(() => localStorage.getItem('pinvou_mode_state_v1'));
  rec('选择 Codex 后保存代码 Agent provider',
    persisted && JSON.parse(persisted).mode === 'code' && JSON.parse(persisted).codeProvider === 'codex',
    String(persisted));

  rec('页面无未处理 JavaScript 异常', errors.length === 0, errors.slice(0, 2).join(' | '));

  await browser.close();
  fs.rmSync(PROFILE, { recursive: true, force: true });
  const failed = results.filter(r => !r.pass).length;
  console.log(failed ? `\n❌ ${failed}/${results.length} FAILED` : `\n✅ ALL ${results.length} PASS`);
  process.exit(failed ? 1 : 0);
})().catch(e => {
  try { fs.rmSync(PROFILE, { recursive: true, force: true }); } catch (_) {}
  console.error('FATAL', e.stack || e);
  process.exit(1);
});
