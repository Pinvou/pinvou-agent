#!/usr/bin/env node
/**
 * 插件中心回收站冒烟:加载 Vite dist + mock Tauri(desktop,可写权限),
 * 验证回收站入口(chips 行)、子页面切换(标题+返回按钮,主列表消失)、
 * 列表渲染(名称/类型/回收时间)、恢复(含凭据提示)、导出(成功提示/取消静默)、
 * package_missing 禁用恢复与导出、彻底删除二次确认、空态文案、返回主列表、
 * 已安装卡片导出按钮(成功提示文件名/取消静默/卸载后消失)、
 * 上传技能卸载提示「移入回收站」。
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
    // 回收站条目:my-mcp 可恢复(恢复后提示重填凭据);old-skill 包文件缺失(只能彻底删除)。
    var recycled=[
      {id:'my-mcp',display_name:'my-mcp.zip',kind:'mcp',recycled_at:'2026-08-20T10:00:00Z',package_missing:false},
      {id:'old-skill',display_name:'old-skill.zip',kind:'skill',recycled_at:'2026-08-21T10:00:00Z',package_missing:true},
    ];
    var skills=[
      {id:'my-test-skill',title:'my-test-skill',description:'用大模型整理会议纪要',installed:true,user_uploaded:true,subtitle:''},
    ];
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
      // 卸载后技能从后端列表消失(真实后端语义:上传技能卸载进回收站,不再列出),
      // 前端卡片随之卸载——「卸载后无导出按钮」断言依赖这一状态变化。
      list_marketplace_skills:function(){return skills.slice();},
      uninstall_marketplace_skill:function(args){
        var i=skills.findIndex(function(x){return x.id===args.skillId;});
        if(i>=0)skills.splice(i,1);
        return null;
      },
      list_recycled_plugins:function(){return recycled.slice();},
      restore_recycled_plugin:function(args){
        var i=recycled.findIndex(function(x){return x.id===args.id;});
        if(i<0)throw new Error('未知回收站条目 '+args.id);
        recycled.splice(i,1);
        return {credentials_required:args.id==='my-mcp'};
      },
      purge_recycled_plugin:function(args){
        var i=recycled.findIndex(function(x){return x.id===args.id;});
        if(i<0)throw new Error('未知回收站条目 '+args.id);
        recycled.splice(i,1);
        return null;
      },
      // 导出:__EXPORT_CANCEL__=true 模拟用户在保存对话框取消(返回 null,前端静默)。
      // 返回 Windows 风格路径,覆盖「标题只显示文件名、完整路径进副标题」的拆分逻辑。
      export_recycled_plugin:function(args){
        var hit=recycled.find(function(x){return x.id===args.id;});
        if(!hit)throw new Error('未知回收站条目 '+args.id);
        if(hit.package_missing)throw new Error('包目录缺失: '+args.id);
        return window.__EXPORT_CANCEL__?null:('C:\\\\Users\\\\test\\\\Downloads\\\\export-'+args.id+'.zip');
      },
      // 已安装插件导出(卡片操作区):同款取消/路径语义,默认文件名 <id>.zip。
      export_installed_plugin:function(args){
        return window.__EXPORT_CANCEL__?null:('C:\\\\Users\\\\test\\\\Downloads\\\\'+args.id+'.zip');
      },
      open_external_url:function(){return null;},
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
async function dismiss(page) {
  await page.evaluate(() => {
    const btn = [...document.querySelectorAll('button')].find(b => (b.textContent || '').trim() === '知道了');
    if (btn) btn.click();
  });
  await sleep(150);
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

    // 1. 回收站入口按钮存在(chips 行「仅显示已安装」旁)
    rec('回收站入口按钮渲染', await page.evaluate(() => !!document.querySelector('[data-testid="tool-store-recycle-bin"]')));

    // 2. 已安装卡片(上传技能 my-test-skill)操作区出现「导出」按钮;未安装卡片没有
    rec('已安装卡片渲染导出按钮且仅此一个', await page.evaluate(() => {
      const btns = [...document.querySelectorAll('[data-testid="tool-store-export"]')];
      return btns.length === 1 && btns[0].getAttribute('data-tool-id') === 'my-test-skill';
    }));
    // 3. 点击导出 → export_installed_plugin(id);成功提示标题只含文件名,路径进副标题;取消静默
    await page.click('[data-testid="tool-store-export"]');
    await sleep(400);
    rec('卡片导出调用 export_installed_plugin(id)', await page.evaluate(() =>
      window.__PINVOU_MOCK_CALLS__.some(c => c.cmd === 'export_installed_plugin' && c.args.id === 'my-test-skill')));
    rec('卡片导出成功提示标题只含文件名', await page.evaluate(() => {
      const title = [...document.querySelectorAll('div')].find(d => (d.textContent || '').trim() === '已导出为 my-test-skill.zip');
      return !!title && !title.textContent.includes('C:') && title.classList.contains('break-all');
    }));
    rec('卡片导出成功副标题含完整路径', await page.evaluate(() =>
      [...document.querySelectorAll('div')].some(d => (d.textContent || '').trim() === 'C:\\Users\\test\\Downloads\\my-test-skill.zip' && d.classList.contains('break-all'))));
    await dismiss(page);
    await page.evaluate(() => { window.__EXPORT_CANCEL__ = true; });
    await page.click('[data-testid="tool-store-export"]');
    await sleep(400);
    rec('卡片导出取消(返回 null)静默不提示', await page.evaluate(() =>
      window.__PINVOU_MOCK_CALLS__.filter(c => c.cmd === 'export_installed_plugin').length === 2
      && !document.body.innerText.includes('已导出为')));
    await page.evaluate(() => { window.__EXPORT_CANCEL__ = false; });

    // 4. 上传技能卸载提示「移入回收站」(预置/普通卸载不出现该文案)
    await page.evaluate(() => {
      const rows = [...document.querySelectorAll('div')].filter(el =>
        (el.textContent || '').includes('my-test-skill') && el.querySelector('button'));
      const row = rows[rows.length - 1];
      if (row) row.querySelector('button').click();
    });
    await sleep(600);
    rec('上传技能卸载提示移入回收站', await page.evaluate(() => document.body.innerText.includes('已卸载「my-test-skill」，移入回收站')));
    await dismiss(page);
    rec('卸载后卡片不再渲染导出按钮', await page.evaluate(() =>
      !document.querySelector('[data-testid="tool-store-export"]')));

    // 3. 打开回收站 → 切换为子页面(非弹窗):触发 list_recycled_plugins,
    //    页面标题/返回按钮出现,主列表(搜索框)消失;条目渲染(名称/类型徽标/回收时间)
    await page.click('[data-testid="tool-store-recycle-bin"]');
    await sleep(400);
    rec('打开回收站触发 list_recycled_plugins', await page.evaluate(() =>
      window.__PINVOU_MOCK_CALLS__.some(c => c.cmd === 'list_recycled_plugins')));
    rec('回收站渲染为子页面(标题+返回按钮,主列表消失)', await page.evaluate(() =>
      document.body.innerText.includes('插件回收站')
      && !!document.querySelector('[data-testid="recycle-bin-back"]')
      && !document.querySelector('[data-testid="tool-store-search"]')));
    rec('回收站列表渲染条目与类型徽标', await page.evaluate(() => {
      const list = document.querySelector('[data-testid="recycled-plugin-list"]');
      return !!list && document.body.innerText.includes('my-mcp.zip') && document.body.innerText.includes('old-skill.zip')
        && list.innerText.includes('MCP') && list.innerText.toUpperCase().includes('SKILL');
    }));
    rec('回收时间本地化渲染', await page.evaluate(() => {
      const list = document.querySelector('[data-testid="recycled-plugin-list"]');
      return !!list && list.innerText.includes('回收于');
    }));

    // 4. package_missing 条目恢复/导出按钮禁用,彻底删除按钮可用
    rec('package_missing 条目禁用恢复', await page.evaluate(() => {
      const restore = document.querySelector('[data-testid="recycled-restore-old-skill"]');
      const purge = document.querySelector('[data-testid="recycled-purge-old-skill"]');
      return !!restore && restore.disabled && !!purge && !purge.disabled;
    }));
    rec('package_missing 条目禁用导出', await page.evaluate(() => {
      const exp = document.querySelector('[data-testid="recycled-export-old-skill"]');
      const ok = document.querySelector('[data-testid="recycled-export-my-mcp"]');
      return !!exp && exp.disabled && !!ok && !ok.disabled;
    }));

    // 5. 导出 my-mcp:标题只含文件名(不含完整路径),路径降级为副标题(break-all 防溢出);
    //    mock 返回 null(用户取消)时不给任何提示
    await page.click('[data-testid="recycled-export-my-mcp"]');
    await sleep(400);
    rec('导出调用 export_recycled_plugin(id)', await page.evaluate(() =>
      window.__PINVOU_MOCK_CALLS__.some(c => c.cmd === 'export_recycled_plugin' && c.args.id === 'my-mcp')));
    rec('导出成功提示标题只含文件名', await page.evaluate(() => {
      const title = [...document.querySelectorAll('div')].find(d => (d.textContent || '').trim() === '已导出为 export-my-mcp.zip');
      return !!title && !title.textContent.includes('C:');
    }));
    rec('导出成功提示副标题含完整路径且带 break-all 防溢出', await page.evaluate(() => {
      const subtitle = [...document.querySelectorAll('div')].find(d => (d.textContent || '').trim() === 'C:\\Users\\test\\Downloads\\export-my-mcp.zip');
      const title = [...document.querySelectorAll('div')].find(d => (d.textContent || '').trim() === '已导出为 export-my-mcp.zip');
      return !!subtitle && subtitle.classList.contains('break-all') && !!title && title.classList.contains('break-all');
    }));
    await dismiss(page);
    await page.evaluate(() => { window.__EXPORT_CANCEL__ = true; });
    await page.click('[data-testid="recycled-export-my-mcp"]');
    await sleep(400);
    rec('导出取消(返回 null)静默不提示', await page.evaluate(() =>
      window.__PINVOU_MOCK_CALLS__.filter(c => c.cmd === 'export_recycled_plugin').length === 2
      && !document.body.innerText.includes('已导出为')));
    await page.evaluate(() => { window.__EXPORT_CANCEL__ = false; });

    // 6. 恢复 my-mcp → 命令带 id;credentials_required=true → 提示重填凭据;列表刷新移除该条目
    await page.click('[data-testid="recycled-restore-my-mcp"]');
    await sleep(600);
    rec('恢复调用 restore_recycled_plugin(id)', await page.evaluate(() =>
      window.__PINVOU_MOCK_CALLS__.some(c => c.cmd === 'restore_recycled_plugin' && c.args.id === 'my-mcp')));
    rec('恢复后提示重新填写凭据', await page.evaluate(() =>
      document.body.innerText.includes('已恢复「my-mcp.zip」') && document.body.innerText.includes('请重新填写')));
    rec('恢复成功副标题为「新会话生效」而非「已移除」', await page.evaluate(() =>
      document.body.innerText.includes('新工具需要在新会话中生效')
      && !document.body.innerText.includes('已移除，新会话将不再加载该工具')));
    await dismiss(page);
    rec('恢复后回收站列表移除该条目', await page.evaluate(() => {
      const list = document.querySelector('[data-testid="recycled-plugin-list"]');
      return !!list && !list.innerText.includes('my-mcp.zip') && list.innerText.includes('old-skill.zip');
    }));

    // 7. 彻底删除:先弹二次确认,确认后调 purge_recycled_plugin 并刷新列表
    await page.click('[data-testid="recycled-purge-old-skill"]');
    await sleep(250);
    rec('彻底删除弹二次确认', await page.evaluate(() =>
      document.body.innerText.includes('彻底删除「old-skill.zip」？') && !!document.querySelector('[data-testid="recycled-purge-confirm"]')));
    const purgeCallsBefore = await page.evaluate(() => window.__PINVOU_MOCK_CALLS__.filter(c => c.cmd === 'purge_recycled_plugin').length);
    await page.click('[data-testid="recycled-purge-confirm"]');
    await sleep(500);
    rec('确认后调用 purge_recycled_plugin(id)', await page.evaluate((before) =>
      window.__PINVOU_MOCK_CALLS__.filter(c => c.cmd === 'purge_recycled_plugin' && c.args.id === 'old-skill').length > 0
      && window.__PINVOU_MOCK_CALLS__.filter(c => c.cmd === 'purge_recycled_plugin').length === before + 1, purgeCallsBefore));
    rec('彻底删除成功提示', await page.evaluate(() => document.body.innerText.includes('已彻底删除「old-skill.zip」')));
    await dismiss(page);

    // 8. 清空后显示空态文案
    rec('回收站空态文案', await page.evaluate(() =>
      !document.querySelector('[data-testid="recycled-plugin-list"]')
      && document.body.innerText.includes('回收站是空的')));

    // 9. 返回按钮回到插件中心主列表
    await page.click('[data-testid="recycle-bin-back"]');
    await sleep(300);
    rec('返回按钮回到主列表', await page.evaluate(() =>
      !!document.querySelector('[data-testid="tool-store-search"]')
      && !document.querySelector('[data-testid="recycle-bin-back"]')));
  } finally {
    await browser.close();
  }
  console.log(failures ? `\n❌ ${failures} FAIL` : '\n✅ ALL PASS');
  process.exit(failures ? 1 : 0);
})().catch(e => { console.error(e); process.exit(1); });
