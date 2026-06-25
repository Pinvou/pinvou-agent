#!/usr/bin/env node
/**
 * 撕离按钮冒烟：主窗口加载 → 展开侧边栏 → 「系统监控」项的 ⧉(data-tearoff="monitor")可见，
 * 点击后以 {kind:'monitor'} 调 open_detached_window。
 * 侧边栏默认折叠,⧉ 仅展开态渲染,故先点 data-sidebar-toggle 展开。
 * 用法：node pinvou3-app/tests/tearoff_buttons_smoke.js   (PASS→0 / FAIL→1 / 缺依赖→2)
 */
const fs = require('fs'), path = require('path'), os = require('os');
function loadPuppeteer(){ try{return require('puppeteer-core');}catch(e){}
  const npx=path.join(os.homedir(),'.npm','_npx');
  if(fs.existsSync(npx))for(const d of fs.readdirSync(npx)){const p=path.join(npx,d,'node_modules','puppeteer-core');
    if(fs.existsSync(p)){try{return require(p);}catch(e){}}}
  console.error('SKIP: 找不到 puppeteer-core');process.exit(2);}
const puppeteer=loadPuppeteer();
const INDEX='file://'+path.join(__dirname,'..','src','index.html');
const CHROME=process.env.CHROME||['/snap/bin/chromium','/usr/bin/chromium','/usr/bin/chromium-browser','/usr/bin/google-chrome','/usr/bin/google-chrome-stable'].find(p=>fs.existsSync(p));
if(!CHROME){console.error('SKIP: 未找到 chromium');process.exit(2);}
const PROFILE=fs.mkdtempSync(path.join(os.tmpdir(),'pinvou-tearoff-'));
// mock 返回各命令的合法形状(参照 ui_smoke.js),否则 App 用空数据渲染会抛错(bs.sessions.map…)。
function injectSource(){return `(function(){
  window.__CALLS__=[];
  function resp(cmd){
    switch(cmd){
      case 'get_settings': return {theme:'liquid-light',language:'zh-Hans'};
      case 'get_effective_model_config': return {model:'qwen36_35b_256k',base_url:'http://127.0.0.1:8000/v1',api_key_set:false};
      case 'list_sessions': return [];
      case 'list_personas': return [];
      case 'list_marketplace_tools': return [];
      case 'list_workflows': return [];
      case 'list_workspace_files': return [];
      case 'check_dependencies': return [];
      case 'get_session_persona_events': return [];
      case 'get_session_pinvou_reviews': return [];
      case 'get_super_permission_status': return false;
      case 'get_backend_status': return {online:true,ok:true,status:'online',model:'qwen36_35b_256k'};
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
(async()=>{
  const browser=await puppeteer.launch({executablePath:CHROME,headless:'new',userDataDir:PROFILE,args:['--no-sandbox']});
  const page=await browser.newPage();
  await page.evaluateOnNewDocument(injectSource());
  await page.goto(INDEX,{waitUntil:'networkidle0'});
  await new Promise(r=>setTimeout(r,1500));
  let ok=true;

  // 展开侧边栏(默认折叠)
  const toggle=await page.$('[data-sidebar-toggle]');
  if(!toggle){console.error('FAIL: 没找到 [data-sidebar-toggle]');ok=false;}
  else{ await toggle.click(); await new Promise(r=>setTimeout(r,400)); }

  const btn=ok?await page.$('[data-tearoff="monitor"]'):null;
  if(ok&&!btn){console.error('FAIL: 展开后没找到 [data-tearoff="monitor"] 入口');ok=false;}
  else if(btn){
    await btn.click(); await new Promise(r=>setTimeout(r,200));
    const calls=await page.evaluate(()=>window.__CALLS__.filter(c=>c.cmd==='open_detached_window'));
    if(!calls.some(c=>c.args&&c.args.kind==='monitor')){console.error('FAIL: 点击未以 kind=monitor 调 open_detached_window，实际:',JSON.stringify(calls));ok=false;}
  }
  await browser.close();fs.rmSync(PROFILE,{recursive:true,force:true});
  if(ok){console.log('PASS: ⧉ 弹出入口调用正确(kind=monitor)');process.exit(0);} process.exit(1);
})();
