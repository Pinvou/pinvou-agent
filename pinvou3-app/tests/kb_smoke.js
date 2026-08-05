#!/usr/bin/env node
/**
 * 本地文件与知识库（KnowledgeView）e2e 渲染 probe — headless chromium + mock 全部 kb_* 命令。
 * 先切到一级「产出物」视图验证产物预览，再切「本地知识」视图逐项验证：
 * 文件管理 subtab(分类卡/文件行/加入知识库浮层)、知识库 subtab(banner/知识集卡片/聚焦知识集/添加文件)。
 * 重点抓运行时 ReferenceError。
 * 用法: node pinvou3-app/tests/kb_smoke.js  (全 PASS→0 / FAIL→1 / 缺依赖→2)
 */
const fs = require('fs'), path = require('path'), os = require('os');
const { startUiTestServer } = require('./ui_test_server');
function loadPuppeteer() {
  try { return require('puppeteer-core'); } catch (e) {}
  const npx = path.join(os.homedir(), '.npm', '_npx');
  if (fs.existsSync(npx)) for (const d of fs.readdirSync(npx)) {
    const p = path.join(npx, d, 'node_modules', 'puppeteer-core');
    if (fs.existsSync(p)) { try { return require(p); } catch (e) {} }
  }
  console.error('SKIP: 找不到 puppeteer-core'); process.exit(2);
}
const puppeteer = loadPuppeteer();
const CHROME = process.env.CHROME || [
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  '/snap/bin/chromium',
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
  '/usr/bin/google-chrome',
  '/usr/bin/google-chrome-stable',
].find(p => fs.existsSync(p));
if (!CHROME) { console.error('SKIP: 未找到 chromium/chrome,可用 env CHROME=/path/to/chromium 指定'); process.exit(2); }
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-kb-'));

function injectSource() {
  return `(function(){
    window.__KB_CALLS__=[];
    const COLLS=[
      {id:1,name:'产品资料库',category:'产品',description:'PRD 与版本规划',createdAt:1,updatedAt:9,status:'ready',docCount:3,chunkCount:12,totalBytes:126000000},
      {id:2,name:'市场调研',category:'调研',description:'竞品与访谈',createdAt:1,updatedAt:8,status:'indexing',docCount:1,chunkCount:4,totalBytes:88000000}
    ];
    const DOCS=[
      {id:11,collectionId:1,collName:'产品资料库',path:'/home/x/路线图.md',name:'路线图.md',ext:'md',size:48000,mtime:1700000000,parseStatus:'parsed',nChunks:8},
      {id:12,collectionId:1,collName:'产品资料库',path:'/home/x/扫描件.jpg',name:'扫描件.jpg',ext:'jpg',size:620000,mtime:1700000000,parseStatus:'skipped',nChunks:0}
    ];
    const FILES=[
      {path:'/home/x/季度财报.xlsx',name:'季度财报.xlsx',ext:'xlsx',size:3400000,mtime:1700000000,isDir:false},
      {path:'/home/x/合作协议.pdf',name:'合作协议.pdf',ext:'pdf',size:1800000,mtime:1700000000,isDir:false},
      {path:'/home/x/访谈纪要.md',name:'访谈纪要.md',ext:'md',size:48000,mtime:1700000000,isDir:false}
    ];
    const OUTPUTS=[
      {path:'/home/x/session-b/跨会话报告.md',name:'跨会话报告.md',ext:'md',category:'doc',sessionId:'session-b',source:'会话 B',size:1200,mtime:1700000000}
    ];
    function invoke(cmd,args){
      window.__KB_CALLS__.push({cmd:cmd,args:args||null});
      if (window.__KB_FAIL_IMPORT_CMD__ === cmd) return Promise.reject(new Error('mock import failure'));
      switch(cmd){
        case 'get_settings': return Promise.resolve({theme:'liquid-light',language:'zh-Hans'});
        case 'get_effective_model_config': return Promise.resolve({model:'qwen36_35b_256k',base_url:'http://127.0.0.1:8000/v1',api_key_set:false});
        case 'list_sessions': return Promise.resolve([]);
        case 'get_super_permission_status': return Promise.resolve(false);
        case 'list_personas': return Promise.resolve([]);
        case 'get_backend_status': return Promise.resolve({online:true,ok:true,status:'online',model:'qwen36_35b_256k'});
        case 'check_for_update': return Promise.resolve({available:false});
        case 'find_resumable_run': return Promise.resolve(null);
        case 'check_dependencies': return Promise.resolve([]);
        case 'list_marketplace_tools': return Promise.resolve([]);
        case 'get_mode_state': return Promise.resolve({mode:'yolo',plan_phase:'none'});
        case 'get_active_persona': return Promise.resolve(null);
        case 'list_workflows': return Promise.resolve([]);
        case 'list_deliverable_index': return Promise.resolve(OUTPUTS);
        // ---- kb_* ----
        case 'kb_scan_status': return Promise.resolve({running:false,phase:'done',scanned:1248,dedupDone:0,dedupTotal:0});
        case 'kb_stats': return Promise.resolve({totalFiles:1248,totalBytes:9e9,hashed:1248,duplicateGroups:3,duplicateFiles:7,duplicateWastedBytes:1048576});
        case 'kb_type_counts': return Promise.resolve([{ext:'pdf',count:230},{ext:'docx',count:120},{ext:'xlsx',count:80},{ext:'md',count:60},{ext:'png',count:274},{ext:'zip',count:18}]);
        case 'kb_search': return Promise.resolve(FILES);
        case 'kb_find_duplicates': return Promise.resolve([]);
        case 'kb_collection_list': return Promise.resolve(COLLS);
        case 'kb_documents': return Promise.resolve((args&&args.collectionId>0)?DOCS:DOCS);
        case 'kb_index_status': return Promise.resolve(window.__KB_INDEX_STATE__ || {running:false,phase:'idle',done:0,total:0,failed:0});
        case 'kb_index_resume': window.__KB_INDEX_STATE__={...window.__KB_INDEX_STATE__,running:true,resumable:false,phase:'parsing'}; return Promise.resolve(window.__KB_INDEX_STATE__);
        case 'kb_index_cancel': window.__KB_INDEX_STATE__={...window.__KB_INDEX_STATE__,running:false,resumable:false,phase:'cancelled'}; return Promise.resolve(null);
        case 'kb_index_failed_files':
          if (window.__KB_DEFER_FAILED_PAGE__) return new Promise(resolve => { window.__KB_RESOLVE_FAILED_PAGE__ = resolve; });
          if (window.__KB_FAILED_PAGES__) return Promise.resolve(window.__KB_FAILED_PAGES__[String(args.offset)] || {files:[],nextOffset:null});
          return Promise.resolve(window.__KB_FAILED_PAGE__ || {files:[],nextOffset:null});
        case 'kb_index_retry_file': window.__KB_INDEX_STATE__={...window.__KB_INDEX_STATE__,running:true,phase:'parsing',failed:0,failedFiles:[]}; return Promise.resolve(window.__KB_INDEX_STATE__);
        case 'kb_collection_create': return Promise.resolve(3);
        case 'kb_collection_add_sources': return Promise.resolve({running:true,phase:'parsing',done:0,total:2});
        case 'kb_retrieve': return Promise.resolve([{text:'受访者认为保险报价流程过于繁琐，希望一键比价。竞品在交强险环节体验更顺畅。',score:-1.5,docName:'访谈纪要.md',docPath:'/home/x/访谈纪要.md',ord:0}]);
        case 'kb_embed_info': return Promise.resolve({enabled:true,baseUrl:'local(fastembed)',model:'bge-m3'});
        case 'kb_ask': return Promise.resolve({answer:'受访者认为保险报价流程过于繁琐，希望一键比价 [1]。竞品在交强险环节体验更顺畅 [1]。',citations:[{idx:1,docName:'访谈纪要.md',docPath:'/home/x/访谈纪要.md',ord:0,snippet:'受访者认为保险报价流程过于繁琐…'}],noContext:false});
        default: return Promise.resolve(null);
      }
    }
    window.__TAURI__={core:{invoke:invoke},event:{emit:function(){return Promise.resolve();},listen:function(){return Promise.resolve(function(){});}},
      window:{getCurrentWindow:function(){return {minimize(){},maximize(){},close(){},toggleMaximize(){},isMaximized(){return Promise.resolve(false);},onResized(){return Promise.resolve(function(){});},startDragging(){}};}},
      dialog:{open:function(){return Promise.resolve(['/home/x/新文档.pdf']);}}};
  })();`;
}

const sleep = ms => new Promise(r => setTimeout(r, ms));
async function clickContains(page, sel, text) {
  return page.evaluate((sel, text) => {
    const els = [...document.querySelectorAll(sel)].filter(el => (el.textContent || '').includes(text));
    const el = els[els.length - 1];
    if (el) { el.scrollIntoView({ block: 'center' }); el.click(); return true; }
    return false;
  }, sel, text);
}

(async () => {
  const { url: INDEX } = await startUiTestServer();
  const results = [];
  const rec = (name, pass, detail) => { results.push({ name, pass }); console.log(`${pass ? '✅' : '❌'} ${name}${detail ? '  ' + detail : ''}`); };
  const browser = await puppeteer.launch({ executablePath: CHROME, headless: 'new', args: ['--no-sandbox','--disable-gpu','--no-first-run','--no-default-browser-check'], userDataDir: PROFILE });
  const page = await browser.newPage();
  const errs = [];
  page.on('pageerror', e => errs.push(e.message));
  page.on('console', m => { if (m.type() === 'error') errs.push('console:' + m.text()); });
  await page.evaluateOnNewDocument(injectSource());
  await page.setViewport({ width: 1440, height: 1000 });
  await page.goto(INDEX, { waitUntil: 'networkidle0' });
  await page.waitForFunction(() => window.TauriBridge && document.body && document.body.innerText.includes('PINVOU'), { timeout: 20000 }).catch(() => {});
  await sleep(1500);

  await page.evaluate(() => {
    window.__OUTPUT_PREVIEW_READS__ = [];
    window.TauriBridge.artifacts.readArtifactText = async (path, sessionId) => {
      window.__OUTPUT_PREVIEW_READS__.push({ kind: 'text', path, sessionId });
      return '# 跨会话报告';
    };
    window.TauriBridge.artifacts.artifactInfo = async (path, sessionId) => {
      window.__OUTPUT_PREVIEW_READS__.push({ kind: 'info', path, sessionId });
      return { exists: true, kind: 'md', size: 1200 };
    };
  });
  const callsBeforeOutputs = await page.evaluate(() => window.__KB_CALLS__.length);

  // 切到「产出物」一级视图
  await page.evaluate(() => { const b = document.querySelector('[title*="侧边栏"],[title*="展开"]'); if (b) b.click(); });
  await sleep(400);
  const entered = await clickContains(page, 'button,div,span,a', '产出物');
  await sleep(700);
  await page.waitForFunction(() => document.body.innerText.includes('跨会话报告.md'), { timeout: 5000 }).catch(() => {});
  await sleep(300);
  await clickContains(page, 'div', '跨会话报告.md');
  await sleep(300);
  const outputPreviewSession = await page.evaluate(() => {
    const calls = window.__OUTPUT_PREVIEW_READS__ || [];
    return {
      live: calls.some(c => c.kind === 'text' && c.path.endsWith('跨会话报告.md') && c.sessionId === 'session-b'),
      modal: calls.some(c => c.kind === 'info' && c.path.endsWith('跨会话报告.md') && c.sessionId === 'session-b'),
      calls,
    };
  });
  rec('⓪ 产出物预览始终携带所属会话', outputPreviewSession.live && outputPreviewSession.modal, JSON.stringify(outputPreviewSession));
  const outputKbCalls = await page.evaluate((start) => window.__KB_CALLS__.slice(start)
    .filter(c => String(c.cmd).startsWith('kb_')).map(c => c.cmd), callsBeforeOutputs);
  rec('⓪a 产出物视图不触发知识库查询', outputKbCalls.length === 0, JSON.stringify(outputKbCalls));
  await clickContains(page, 'button', '✕'); await sleep(200);
  // 切到「本地知识」视图(产出物已独立为一级菜单)
  const callsBeforeKnowledge = await page.evaluate(() => window.__KB_CALLS__.length);
  await clickContains(page, 'button,div,span,a', '本地知识');
  await sleep(700);
  const initialKnowledgeCalls = await page.evaluate((start) => {
    const counts = {};
    window.__KB_CALLS__.slice(start).forEach(({ cmd }) => { counts[cmd] = (counts[cmd] || 0) + 1; });
    return counts;
  }, callsBeforeKnowledge);
  const initialCommands = [
    'kb_scan_status', 'kb_stats', 'kb_type_counts',
    'kb_collection_list', 'kb_documents', 'kb_embed_info', 'kb_model_status', 'kb_index_status',
  ];
  rec('⓪b 本地知识首次加载不重复请求', initialCommands.every(cmd => initialKnowledgeCalls[cmd] === 1), JSON.stringify(initialKnowledgeCalls));
  await clickContains(page, 'button', '本地文件管理');
  await sleep(1500);

  const filesView = await page.evaluate(() => {
    const x = document.body.innerText;
    return { entered: document.querySelector('[data-testid="app-root"]')?.getAttribute('data-current-view') === 'knowledge' || x.includes('本地文件与知识库'), subFiles: x.includes('本地文件管理'), subKb: x.includes('知识库'),
      cats: x.includes('文档') && x.includes('PDF') && x.includes('图片'),
      fileRow: x.includes('季度财报.xlsx') || x.includes('合作协议.pdf') };
  });
  rec('① 进入视图 + 文件管理渲染(subtab/分类卡/文件行)', filesView.entered && filesView.subFiles && filesView.cats && filesView.fileRow, JSON.stringify(filesView));

  // 文件行「加入知识库」浮层
  await page.evaluate(() => { const b = [...document.querySelectorAll('button[title]')].find(b => (b.getAttribute('title')||'').includes('加入知识库')); if (b) b.click(); });
  await sleep(500);
  const addPop = await page.evaluate(() => document.body.innerText.includes('产品资料库') && document.body.innerText.includes('加入知识库'));
  rec('② 文件行「加入知识库」浮层列出知识集', addPop);
  await page.evaluate(() => { const ov = document.querySelector('.bg-black\\/40'); if (ov) ov.click(); });
  await sleep(300);

  // 切「知识库」subtab
  await clickContains(page, 'button', '知识库');
  await sleep(1200);
  const kbView = await page.evaluate(() => {
    const x = document.body.innerText;
    return { banner: x.includes('一键构建') || x.includes('AI 知识库'), card: x.includes('产品资料库'), status: x.includes('已就绪') || x.includes('解析中'), collFiles: x.includes('知识库内文件') };
  });
  rec('③ 知识库 subtab(banner/知识集卡片/状态)', kbView.banner && kbView.card && kbView.status, JSON.stringify(kbView));

  // 聚焦知识集(精确点知识集卡片，避开「知识库内文件」表里的同名行)
  await page.evaluate(() => {
    const cards = [...document.querySelectorAll('div')].filter(d => typeof d.className === 'string' && d.className.includes('cursor-pointer') && (d.textContent || '').includes('产品资料库'));
    if (cards.length) { cards[0].scrollIntoView({ block: 'center' }); cards[0].click(); }
  });
  await sleep(1000);
  const focused = await page.evaluate(() => {
    const x = document.body.innerText;
    const reset = [...document.querySelectorAll('button')].some(b => (b.textContent || '').trim() === '全部'
      && b.parentElement && (b.parentElement.textContent || '').includes('知识库内文件'));
    return { scoped: reset, docList: x.includes('路线图.md'), addBtn: x.includes('添加文件') };
  });
  rec('④ 聚焦知识集后显示范围/文档列表/添加文件', focused.scoped && focused.docList && focused.addBtn, JSON.stringify(focused));

  // 聚焦后添加文件：dialog mock 返回路径，必须透传到当前知识集。
  await clickContains(page, 'button', '添加文件');
  await sleep(500);
  const added = await page.evaluate(() => window.__KB_CALLS__.some(c => c.cmd === 'kb_collection_add_sources'
    && c.args && c.args.collectionId === 1 && Array.isArray(c.args.paths) && c.args.paths.includes('/home/x/新文档.pdf')));
  rec('⑤ 添加文件透传当前知识集和所选路径', added);

  await page.evaluate(() => {
    const reset = [...document.querySelectorAll('button')].find(b => (b.textContent || '').trim() === '全部'
      && b.parentElement && (b.parentElement.textContent || '').includes('知识库内文件'));
    if (reset) reset.click();
  });
  await sleep(400);
  const unscoped = await page.evaluate(() => document.body.innerText.includes('所属知识库')
    && ![...document.querySelectorAll('button')].some(b => (b.textContent || '').trim() === '全部'
      && b.parentElement && (b.parentElement.textContent || '').includes('知识库内文件')));
  rec('⑥ 返回全部知识集后恢复跨库文件表', unscoped);

  // 模拟应用重启后发现中断任务：应展示保存进度并提供继续/取消，不要求重新选择整批文件。
  await page.evaluate(() => { window.__KB_INDEX_STATE__ = {
    jobId:'kb-import-test',running:false,resumable:true,collectionId:1,phase:'interrupted',
    done:3,total:8,completed:3,skipped:0,failed:0,currentPath:null,
    currentChunksDone:0,currentChunksTotal:0,failedFiles:[]
  }; });
  await clickContains(page, 'button', '本地文件管理'); await sleep(300);
  await clickContains(page, 'button', '知识库'); await sleep(700);
  const resumeUi = await page.evaluate(() => document.body.innerText.includes('发现未完成的导入任务')
    && document.body.innerText.includes('文件进度 3/8') && document.body.innerText.includes('继续导入'));
  await clickContains(page, 'button', '继续导入'); await sleep(300);
  const resumed = await page.evaluate(() => window.__KB_CALLS__.some(c => c.cmd === 'kb_index_resume'
    && c.args && c.args.jobId === 'kb-import-test'));
  rec('⑦ 中断任务显示持久化进度并可继续', resumeUi && resumed);

  await page.evaluate(() => {
    window.__KB_INDEX_STATE__ = {
      jobId:'kb-import-paged',running:false,resumable:false,collectionId:1,phase:'done_with_errors',
      done:3,total:3,completed:0,skipped:0,failed:3,currentPath:null,
      currentChunksDone:0,currentChunksTotal:0,
      failedFiles:[
        {itemId:1,name:'失败-1.md',path:'/tmp/失败-1.md',error:'解析失败'},
        {itemId:2,name:'失败-2.md',path:'/tmp/失败-2.md',error:'解析失败'}
      ]
    };
    window.__KB_FAILED_PAGES__ = {
      '0': {files:[
        {itemId:1,name:'失败-1.md',path:'/tmp/失败-1.md',error:'解析失败'},
        {itemId:2,name:'失败-2.md',path:'/tmp/失败-2.md',error:'解析失败'}
      ],nextOffset:2},
      '2': {files:[{itemId:3,name:'失败-3.md',path:'/tmp/失败-3.md',error:'解析失败'}],nextOffset:null}
    };
  });
  await page.evaluate(() => [...document.querySelectorAll('button')]
    .find(b => (b.textContent || '').trim() === '本地文件管理')?.click());
  await sleep(150);
  await page.evaluate(() => [...document.querySelectorAll('button')]
    .find(b => (b.textContent || '').trim() === '知识库')?.click());
  await sleep(450);
  await page.evaluate(() => { window.__KB_DEFER_FAILED_PAGE__ = true; });
  await clickContains(page, 'button', '加载更多失败文件'); await sleep(100);
  const retryDisabledDuringPage = await page.evaluate(() => [...document.querySelectorAll('button')]
    .filter(b => (b.textContent || '').trim() === '重试').every(b => b.disabled));
  // 同 job 的 status reset 必须递增 generation，使在途分页响应失效。
  await page.evaluate(() => [...document.querySelectorAll('button')]
    .find(b => (b.textContent || '').trim() === '本地文件管理')?.click());
  await sleep(100);
  await page.evaluate(() => [...document.querySelectorAll('button')]
    .find(b => (b.textContent || '').trim() === '知识库')?.click());
  await sleep(250);
  await page.evaluate(() => {
    window.__KB_DEFER_FAILED_PAGE__ = false;
    window.__KB_RESOLVE_FAILED_PAGE__?.({
      files:[{itemId:999,name:'过期响应.md',path:'/tmp/过期响应.md',error:'过期'}],nextOffset:null
    });
  });
  await sleep(150);
  const staleIgnored = await page.evaluate(() => !document.body.innerText.includes('过期响应.md'));
  await clickContains(page, 'button', '加载更多失败文件'); await sleep(250);
  await clickContains(page, 'button', '加载更多失败文件'); await sleep(250);
  const pagedFailures = await page.evaluate(() => ({
    visible: document.body.innerText.includes('失败-3.md'),
    requested: [0, 2].every(offset => window.__KB_CALLS__.some(c => c.cmd === 'kb_index_failed_files'
      && c.args && c.args.jobId === 'kb-import-paged' && c.args.offset === offset && c.args.limit === 50)),
    unique: ['失败-1.md','失败-2.md','失败-3.md'].every(name => document.body.innerText.split(name).length === 2),
  }));
  rec('⑧ 分页游标按服务端推进且过期同 job 响应不合并',
    retryDisabledDuringPage && staleIgnored && pagedFailures.visible && pagedFailures.requested && pagedFailures.unique,
    JSON.stringify({ retryDisabledDuringPage, staleIgnored, ...pagedFailures }));

  // 继续/取消/单文件重试的后端拒绝不能静默吞掉；错误要可见，且失败后重新拉取持久化状态。
  const exerciseImportFailure = async (cmd, state, buttonText, expectedText) => {
    await page.evaluate(({ cmd, state }) => {
      window.__KB_FAIL_IMPORT_CMD__ = cmd;
      window.__KB_INDEX_STATE__ = state;
    }, { cmd, state });
    await page.evaluate(() => [...document.querySelectorAll('button')]
      .find(b => (b.textContent || '').trim() === '本地文件管理')?.click());
    await sleep(150);
    await page.evaluate(() => [...document.querySelectorAll('button')]
      .find(b => (b.textContent || '').trim() === '知识库')?.click());
    await sleep(600);
    const before = await page.evaluate(() => window.__KB_CALLS__.filter(c => c.cmd === 'kb_index_status').length);
    await page.evaluate((buttonText) => [...document.querySelectorAll('button')]
      .find(b => (b.textContent || '').trim() === buttonText)?.click(), buttonText);
    await sleep(300);
    return page.evaluate(({ before, expectedText }) => ({
      visible: !!document.querySelector('[data-testid="kb-import-error"][role="alert"]')
        && document.body.innerText.includes(expectedText)
        && document.body.innerText.includes('mock import failure'),
      refreshed: window.__KB_CALLS__.filter(c => c.cmd === 'kb_index_status').length > before,
      commands: window.__KB_CALLS__.slice(-5).map(c => c.cmd),
    }), { before, expectedText });
  };
  const resumableState = {
    jobId:'kb-import-reject',running:false,resumable:true,collectionId:1,phase:'interrupted',
    done:1,total:2,completed:1,skipped:0,failed:0,currentPath:null,
    currentChunksDone:0,currentChunksTotal:0,failedFiles:[]
  };
  const resumeFailure = await exerciseImportFailure('kb_index_resume', resumableState, '继续导入', '继续导入失败');
  const cancelFailure = await exerciseImportFailure('kb_index_cancel', resumableState, '取消任务', '取消导入失败');
  const failedState = {
    jobId:'kb-import-retry-reject',running:false,resumable:false,collectionId:1,phase:'done_with_errors',
    done:1,total:1,completed:0,skipped:0,failed:1,currentPath:null,
    currentChunksDone:0,currentChunksTotal:0,
    failedFiles:[{itemId:99,name:'失败.md',path:'/tmp/失败.md',error:'解析失败'}]
  };
  const retryFailure = await exerciseImportFailure('kb_index_retry_file', failedState, '重试', '重试文件失败');
  await page.evaluate(() => { window.__KB_FAIL_IMPORT_CMD__ = null; });
  rec('⑨ 导入操作失败可见并刷新持久化状态',
    resumeFailure.visible && resumeFailure.refreshed
      && cancelFailure.visible && cancelFailure.refreshed
      && retryFailure.visible && retryFailure.refreshed,
    JSON.stringify({ resumeFailure, cancelFailure, retryFailure }));

  rec('⑩ 全程无运行时报错(ReferenceError 等)', errs.length === 0, errs.length ? errs.slice(0,3).join(' | ') : '');

  await browser.close();
  const failed = results.filter(r => !r.pass).length;
  console.log(failed ? `\n❌ ${failed}/${results.length} FAILED` : `\n✅ ALL ${results.length} PASS`);
  process.exit(failed ? 1 : 0);
})().catch(e => { console.error('FATAL', e.message); process.exit(1); });
