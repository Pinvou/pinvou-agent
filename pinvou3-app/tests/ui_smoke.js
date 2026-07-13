#!/usr/bin/env node
/**
 * pinvou3 前端 UI 冒烟回归测试 — headless chromium + mock TauriBridge,加载真 src/index.html。
 * 覆盖三条易回归的前端通路:
 *   ① resumeWorkflowOnBoot:有僵尸 run 时启动仍落草稿页(activeSessionId=null)+ 只挂看板,不劫持聊天会话。
 *   ② 工具商店渲染出 Obsidian 知识库卡。
 *   ③ 聊天流产物卡(artifact_card)挂出「品/悟」召唤 pinvou 按钮。
 * 依赖:puppeteer-core(自动从 node_modules / ~/.npm/_npx 发现)+ 系统 chromium(或 env CHROME 指定)。
 * 用法:node pinvou3-app/tests/ui_smoke.js   (全 PASS → exit 0,任一 FAIL → exit 1,缺依赖 → exit 2)
 */
const fs = require('fs'), path = require('path'), os = require('os');

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
const INDEX = 'file://' + path.join(__dirname, '..', 'src', 'index.html');
const CHROME = process.env.CHROME ||
  ['/snap/bin/chromium', '/usr/bin/chromium', '/usr/bin/chromium-browser', '/usr/bin/google-chrome', '/usr/bin/google-chrome-stable'].find(p => fs.existsSync(p));
if (!CHROME) { console.error('SKIP: 未找到 chromium/chrome,可用 env CHROME=/path/to/chromium 指定'); process.exit(2); }
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-smoke-'));

// mock TauriBridge:find_resumable_run 返回僵尸 run;一个会话含 write_file → 触发 artifact_card 气泡。
function injectSource() {
  return `(function(){
    window.__TAURI_EVENT_HANDLERS__={};
    const ZOMBIE={session_id:'s-zombie',project_dir:'/x/wf',scenario:'sansheng_liubu'};
    const WF_STATE={project_dir:'/x/wf',scenario:'sansheng_liubu',all_completed:false,roles:{taizi:{name:'太子',status:'running'},zhongshu:{name:'中书',status:'pending'}}};
    const SESSIONS=[{id:'s1',title:'第三季度财报分析',created_at:1,updated_at:9}];
    const CONV={s1:{metadata:{id:'s1'},artifacts:['/home/x/会议纪要.md'],messages:[
      {role:'user',content:[{type:'text',text:'整理纪要'}]},
      {role:'assistant',content:[{type:'text',text:'已生成会议纪要。'},{type:'tool_use',id:'t1',name:'write_file',input:{path:'/home/x/会议纪要.md',content:'# 会议纪要'}}]},
      {role:'user',content:[{type:'tool_result',tool_use_id:'t1',content:'written'}]}]}};
    function invoke(cmd,args){
      switch(cmd){
        case 'get_settings': return Promise.resolve({theme:'liquid-light',language:'zh-Hans'});
        case 'get_effective_model_config': return Promise.resolve({model:'qwen36_35b_256k',base_url:'http://127.0.0.1:8000/v1',api_key_set:false});
        case 'list_sessions': return Promise.resolve(SESSIONS);
        case 'get_super_permission_status': return Promise.resolve(false);
        case 'list_personas': return Promise.resolve([]);
        case 'get_backend_status': return Promise.resolve({online:true,ok:true,status:'online',model:'qwen36_35b_256k'});
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

  // 手机先向尚未在桌面打开的后台 session 发消息：hydration 必须先把磁盘 messages
  // 重建成 chatItems；否则桌面随后切入时只剩这条手机消息，历史和产物卡都像“丢了”。
  await page.evaluate(async () => {
    const handlers = window.__TAURI_EVENT_HANDLERS__['remote_control:mobile_user_message'] || [];
    for (const handler of handlers) {
      await handler({ payload: { session_id: 's1', content: '手机补充消息', client_message_id: 'cm-regression' } });
    }
  });

  // ② 工具商店 Obsidian 卡
  await expand(page); await sleep(500);
  await clickText(page, '工具商店');
  await sleep(1500);
  const obs = await page.evaluate(() => document.body.innerText.includes('Obsidian'));
  rec('② 工具商店渲染 Obsidian 卡', obs);

  // ③ 聊天流产物卡 品/悟 召唤按钮
  await clickText(page, '第三季度财报分析');
  await sleep(2200);
  const hit = await page.evaluate(() => [...document.querySelectorAll('button')]
    .map(b => ({ t: b.getAttribute('title') || '', x: (b.textContent || '').trim() }))
    .filter(o => o.t.includes('查错') || o.t.includes('发散') || /品|悟/.test(o.x)).length);
  const restored = await page.evaluate(() => {
    const text = document.body.innerText;
    return { history: text.includes('整理纪要'), mobile: text.includes('手机补充消息') };
  });
  rec('③ 后台session经手机唤醒后仍恢复历史+新消息+产物卡', hit >= 2 && restored.history && restored.mobile, JSON.stringify({ hit, ...restored }));

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

  // ⑥ MegaCube(GB10) 本地大模型引导框:eligible 时渲染标题+启用/暂不/不再提醒(默认 mock 返 eligible:false 不弹,这里末尾翻 __VLLM_ELIGIBLE__ 再手动触发 detect,不污染 ①–⑤)
  await page.evaluate(() => { window.__VLLM_ELIGIBLE__ = true; });
  await page.evaluate(() => window.TauriBridge.detectLocalVllmSetup());
  await sleep(500);
  const setup = await page.evaluate(() => {
    const btns = [...document.querySelectorAll('button')].map(b => (b.textContent || '').trim());
    return { title: document.body.innerText.includes('启用本地大模型'), enable: btns.includes('启用'), skip: btns.includes('暂不'), never: btns.includes('不再提醒') };
  });
  rec('⑥ MegaCube 引导框 eligible 渲染(标题+启用/暂不/不再提醒)', setup.title && setup.enable && setup.skip && setup.never, JSON.stringify(setup));

  // ⑦ 点「不再提醒」→ 二次确认子态(警示文案 + 确认不启用/再想想);点「再想想」回到初始态
  await clickText(page, '不再提醒');
  await sleep(300);
  const decline = await page.evaluate(() => {
    const btns = [...document.querySelectorAll('button')].map(b => (b.textContent || '').trim());
    return { warn: document.body.innerText.includes('不再自动弹出'), confirm: btns.includes('确认不启用'), reconsider: btns.includes('再想想') };
  });
  await clickText(page, '再想想');
  await sleep(200);
  rec('⑦ 不再提醒→二次确认渲染(警示+确认/再想想)', decline.warn && decline.confirm && decline.reconsider, JSON.stringify(decline));

  // ⑧ 点「启用」→ 进行中步骤指示渲染(授权/等待步骤 + 计时;mock bootstrap 永不 resolve 停在进行中)
  await clickText(page, '启用');
  await sleep(700);
  const prog = await page.evaluate(() => {
    const txt = document.body.innerText;
    return { auth: txt.includes('授权并启动引擎'), wait: txt.includes('等待模型加载就绪'), elapsed: txt.includes('已等待') };
  });
  rec('⑧ 引导进行中步骤指示+计时渲染', prog.auth && prog.wait && prog.elapsed, JSON.stringify(prog));

  if (errs.length) console.log('⚠️ PAGEERRORS:', errs.slice(0, 3).join(' | '));
  await browser.close();

  const failed = results.filter(r => !r.pass).length;
  console.log(failed ? `\n❌ ${failed}/${results.length} FAILED` : `\n✅ ALL ${results.length} PASS`);
  process.exit(failed ? 1 : 0);
})().catch(e => { console.error('FATAL', e.message); process.exit(1); });
