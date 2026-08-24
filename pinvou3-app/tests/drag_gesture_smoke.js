#!/usr/bin/env node
/**
 * 长按起手冒烟：主窗口展开侧边栏 → 分别长按「系统监控」和 Coding 会话 →
 * 断言以对应 kind 调了 begin_detach_drag(原生鬼影由此接管,headless 无法继续验,属手动验收)。
 * 用法：node pinvou3-app/tests/drag_gesture_smoke.js   (PASS→0 / FAIL→1 / 缺依赖→2)
 */
const fs = require('fs'), path = require('path'), os = require('os');
const { startUiTestServer } = require('./ui_test_server');
function loadPuppeteer(){ try{return require('puppeteer-core');}catch{ /* fall through */ }
  const npx=path.join(os.homedir(),'.npm','_npx');
  if(fs.existsSync(npx))for(const d of fs.readdirSync(npx)){const p=path.join(npx,d,'node_modules','puppeteer-core');
    if(fs.existsSync(p)){try{return require(p);}catch{ /* next */ }}}
  console.error('SKIP: 找不到 puppeteer-core');process.exit(2);}
const puppeteer=loadPuppeteer();
const CHROME=process.env.CHROME||['/snap/bin/chromium','/usr/bin/chromium','/usr/bin/chromium-browser','/usr/bin/google-chrome','/usr/bin/google-chrome-stable'].find(p=>fs.existsSync(p));
if(!CHROME){console.error('SKIP: 未找到 chromium');process.exit(2);}
const PROFILE=fs.mkdtempSync(path.join(os.tmpdir(),'pinvou-drag-'));
function injectSource(){return `(function(){
  window.__CALLS__=[];
  function resp(cmd){
    switch(cmd){
      case 'get_settings': return {theme:'liquid-light',language:'zh-Hans'};
      case 'get_platform_capabilities': return {codexAcpSupported:true,detachWindows:true};
      case 'get_effective_model_config': return {model:'m',base_url:'http://127.0.0.1:8000/v1',api_key_set:false};
      case 'list_codex_acp_sessions': return [{id:'coding-detach-1',title:'Coding撕离测试',agent_id:'codex',agent_name:'Codex',workspace_kind:'temporary',workspace_path:'',workspace_available:true,updated_at:new Date().toISOString()}];
      case 'list_sessions': case 'list_personas': case 'list_marketplace_tools':
      case 'list_workspace_files': case 'check_dependencies':
      case 'get_session_persona_events': case 'get_session_pinvou_reviews': return [];
      case 'get_super_permission_status': return false;
      case 'get_backend_status': return {online:true,ok:true,status:'online',model:'m'};
      case 'check_for_update': return {available:false};
      case 'find_resumable_run': return null;
      case 'get_mode_state': return {mode:'yolo',plan_phase:'none'};
      case 'get_active_persona': return null;
      default: return null;
    }
  }
  window.__TAURI__={
    core:{invoke:async(cmd,args)=>{window.__CALLS__.push({cmd,args});return resp(cmd);}},
    event:{listen:async()=>(()=>{}),emit:async()=>{}},
    window:{getCurrentWindow:()=>({minimize(){},maximize(){},close(){},toggleMaximize(){},isMaximized:async()=>false,onResized:async()=>(()=>{}),startDragging(){}})},
    dialog:{open:async()=>null}
  };
})();`;}
// eslint-disable-next-line unicorn/prefer-top-level-await -- smoke 脚本既有 async main() 结构
(async()=>{
  const {url:INDEX}=await startUiTestServer();
  const browser=await puppeteer.launch({executablePath:CHROME,headless:'new',userDataDir:PROFILE,args:['--no-sandbox']});
  const page=await browser.newPage();
  const pageErrors=[];
  page.on('pageerror',e=>pageErrors.push(e.message));
  await page.setViewport({width:1440,height:1000,deviceScaleFactor:1});
  await page.evaluateOnNewDocument(injectSource());
  await page.goto(INDEX,{waitUntil:'networkidle0'});
  await new Promise(r=> { setTimeout(r,1500); });
  let ok=true;

  const toggle=await page.$('[data-sidebar-toggle]');
  if(!toggle){console.error('FAIL: 无 data-sidebar-toggle',pageErrors.length?'PAGEERROR: '+pageErrors.join(' | '):'');ok=false;}
  else{ await toggle.click(); await new Promise(r=> { setTimeout(r,400); }); }

  // 取「系统监控」导航行(data-nav="monitor"),在其上长按起手。
  const box=ok?await page.evaluate(()=>{
    const row=document.querySelector('[data-nav="monitor"]');
    if(!row) return null;
    const r=row.getBoundingClientRect();
    return {x:r.x,y:r.y,w:r.width,h:r.height};
  }):null;
  if(ok&&!box){console.error('FAIL: 找不到 monitor 行');ok=false;}
  else if(box){
    const sx=box.x+20, sy=box.y+box.h/2;       // 行左侧(图标处)
    await page.mouse.move(sx,sy);
    await page.mouse.down();
    await new Promise(r=> { setTimeout(r,480); });    // 长按 > 350ms,期间不动
    const calls=await page.evaluate(()=>window.__CALLS__.filter(c=>c.cmd==='begin_detach_drag'));
    await page.mouse.up();
    if(!calls.some(c=>c.args&&c.args.kind==='monitor')){console.error('FAIL: 长按未以 kind=monitor 调 begin_detach_drag，实际:',JSON.stringify(calls));ok=false;}
  }
  const codingBox=ok?await page.evaluate(()=>{
    const row=document.querySelector('[data-drag-kind="codex-session"]');
    if(!row) return null;
    const r=row.getBoundingClientRect();
    return {x:r.x,y:r.y,w:r.width,h:r.height};
  }):null;
  if(ok&&!codingBox){console.error('FAIL: 找不到可撕离的 Coding 会话行');ok=false;}
  else if(codingBox){
    const sx=codingBox.x+20, sy=codingBox.y+codingBox.h/2;
    await page.mouse.move(sx,sy);
    await page.mouse.down();
    await new Promise(r=> { setTimeout(r,480); });
    const calls=await page.evaluate(()=>window.__CALLS__.filter(c=>c.cmd==='begin_detach_drag'));
    await page.mouse.up();
    if(!calls.some(c=>c.args&&c.args.kind==='codex-session'&&c.args.id==='coding-detach-1')){console.error('FAIL: Coding 会话长按未携带固定 session id，实际:',JSON.stringify(calls));ok=false;}
  }
  await browser.close();fs.rmSync(PROFILE,{recursive:true,force:true});
  if(ok){console.log('PASS: 系统监控与 Coding 会话长按均触发正确 begin_detach_drag');process.exit(0);} process.exit(1);
})();
