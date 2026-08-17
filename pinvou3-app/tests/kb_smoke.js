#!/usr/bin/env node
/**
 * 本地文件与知识库（KnowledgeView）e2e 渲染 probe — headless chromium + mock 全部 kb_* 命令。
 * 先切到一级「产出物」视图验证产物预览，再切统一「知识库」视图逐项验证：
 * 文件管理、本地知识库与远程知识库三个 subtab，以及本地知识库的导入和索引流程。
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
const MOCK_SHARE_LINK = 'pinvou-knowledge://share/mock-lan';

function injectSource() {
  return `(function(){
    window.__KB_CALLS__=[];
    window.__REMOTE_KB_SCOPE__='read';
    window.__REMOTE_KB_CONNECTIONS__=[{
      serverId:'cube',name:'Cube Knowledge',endpoint:'https://100.64.12.34:3210',
      scope:'read',deviceId:'pinvou'
    }];
    const COLLS=[
      {id:1,name:'产品资料库',category:'产品',description:'PRD 与版本规划',createdAt:1,updatedAt:9,status:'ready',docCount:3,chunkCount:12,totalBytes:126000000},
      {id:2,name:'市场调研',category:'调研',description:'竞品与访谈',createdAt:1,updatedAt:8,status:'indexing',docCount:1,chunkCount:4,totalBytes:88000000}
    ];
    let DOCS=[
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
        case 'kb_remove_document': {
          if (window.__KB_FAIL_REMOVE_DOCUMENT__) {
            window.__KB_FAIL_REMOVE_DOCUMENT__ = false;
            if (window.__KB_CONCURRENT_DOCUMENT__) {
              DOCS = [...DOCS, window.__KB_CONCURRENT_DOCUMENT__];
              delete window.__KB_CONCURRENT_DOCUMENT__;
            }
            return Promise.reject(new Error('mock remove failure'));
          }
          const finish = () => {
            DOCS = DOCS.filter(document => document.id !== args?.docId);
          };
          if (window.__KB_DEFER_REMOVE_DOCUMENT__) {
            window.__KB_DEFER_REMOVE_DOCUMENT__ = false;
            return new Promise(resolve => {
              window.__KB_RESOLVE_REMOVE_DOCUMENT__ = () => { finish(); resolve(null); };
            });
          }
          finish();
          return Promise.resolve(null);
        }
        case 'kb_retrieve': return Promise.resolve([{text:'受访者认为保险报价流程过于繁琐，希望一键比价。竞品在交强险环节体验更顺畅。',score:-1.5,docName:'访谈纪要.md',docPath:'/home/x/访谈纪要.md',ord:0}]);
        case 'kb_embed_info': return Promise.resolve({enabled:true,baseUrl:'local(fastembed)',model:'bge-m3'});
        case 'kb_ask': return Promise.resolve({answer:'受访者认为保险报价流程过于繁琐，希望一键比价 [1]。竞品在交强险环节体验更顺畅 [1]。',citations:[{idx:1,docName:'访谈纪要.md',docPath:'/home/x/访谈纪要.md',ord:0,snippet:'受访者认为保险报价流程过于繁琐…'}],noContext:false});
        case 'remote_kb_connections': {
          const snapshot = window.__REMOTE_KB_CONNECTIONS__.map(connection=>({
            ...connection,scope:window.__REMOTE_KB_SCOPE__,online:true,ready:true,error:null
          }));
          if (window.__REMOTE_KB_DEFER_CONNECTIONS_ONCE__) {
            window.__REMOTE_KB_DEFER_CONNECTIONS_ONCE__ = false;
            return new Promise(resolve => { window.__REMOTE_KB_RESOLVE_CONNECTIONS__ = () => resolve(snapshot); });
          }
          return Promise.resolve(snapshot);
        }
        case 'remote_kb_pending_joins': return Promise.resolve(window.__REMOTE_PENDING_JOINS__ || []);
        case 'shared_kb_host_status': return Promise.resolve(window.__REMOTE_HOST_STATUS__ || {supported:true,installed:true,running:true,endpoint:'https://127.0.0.1:3210',serviceVersion:'0.8.0',appVersion:'0.8.0',upgradeAvailable:false,clientOutdated:false});
        case 'shared_kb_host_lan_endpoints': return Promise.resolve(['https://192.168.1.20:3210']);
        case 'shared_kb_discover_nearby': {
          const discovered=[{
            endpoint:'https://192.168.1.20:3210',networkKind:'lan',serverId:'nearby-cube',
            serverIdentity:'nearby-identity',serverName:'Nearby Knowledge',protocolVersion:2,
            tlsCa:'mock-ca',caFingerprint:'AABBCCDDEEFF00112233445566778899',
            identityCode:'PINVOU-AABB-CCDD-EEFF-0011',ready:true
          }];
          if (window.__REMOTE_DISCOVERY_RESOLVED__) return Promise.resolve(discovered);
          return new Promise(resolve => {
            window.__REMOTE_RESOLVE_DISCOVERY__=()=>{
              window.__REMOTE_DISCOVERY_RESOLVED__=true;
              resolve(discovered);
            };
          });
        }
        case 'remote_kb_probe_private_endpoint': return Promise.resolve({
          endpoint:'https://192.168.1.20:3210',networkKind:'lan',serverId:'manual-cube',
          serverIdentity:'manual-identity',serverName:'Manual Knowledge',protocolVersion:2,
          tlsCa:'mock-ca',caFingerprint:'AABBCCDDEEFF00112233445566778899',
          identityCode:'PINVOU-AABB-CCDD-EEFF-0011',ready:true
        });
        case 'remote_kb_request_join_confirmed': {
          window.__REMOTE_KB_CONFIRMED_JOIN__={...(args||{})};
          const pending={
            requestId:'confirmed-join-1',serverId:args?.probe?.serverId,serverIdentity:args?.probe?.serverIdentity,
            serverName:args?.probe?.serverName,endpoint:args?.probe?.endpoint,
            deviceName:args?.deviceName || 'Direct device',createdAt:1,expiresAt:9999999999
          };
          window.__REMOTE_PENDING_JOINS__=[pending];
          return Promise.resolve({status:'pending',request:{id:pending.requestId,status:'pending'},connection:null,pending});
        }
        case 'remote_kb_connection_identity': return Promise.resolve({serverId:args?.serverId,caFingerprint:'AABBCCDDEEFF00112233445566778899',identityCode:'PINVOU-AABB-CCDD-EEFF-0011'});
        case 'shared_kb_host_reconnect': {
          const connection={serverId:'lan-cube',name:'LAN Knowledge',endpoint:'https://127.0.0.1:3210',scope:'owner',deviceId:'local-owner'};
          window.__REMOTE_KB_CONNECTIONS__=[connection];
          return Promise.resolve(connection);
        }
        case 'remote_kb_create_share': return Promise.resolve({
          id:'share-1',share:'pinvou-knowledge://shared/mock',expiresAt:Date.now()/1000+86400,
          autoApproveRead:Boolean(args?.autoApproveRead)
        });
        case 'remote_kb_shares': return Promise.resolve([]);
        case 'remote_kb_join_requests': return Promise.resolve(window.__REMOTE_OWNER_JOIN_REQUESTS__ || []);
        case 'remote_kb_devices': {
          if (window.__REMOTE_DEFER_OWNER_DEVICES_ONCE__) {
            window.__REMOTE_DEFER_OWNER_DEVICES_ONCE__ = false;
            return new Promise(resolve => {
              window.__REMOTE_RESOLVE_OWNER_DEVICES__ = () => resolve([
                {id:'local-owner',name:'Stale PINVOU',scope:'owner',revoked:false},
              ]);
            });
          }
          return Promise.resolve([{
            id:'local-owner',name:window.__REMOTE_OWNER_DEVICE_NAME__ || 'This PINVOU',scope:'owner',revoked:false,
          }]);
        }
        case 'remote_kb_model_status': return Promise.resolve({ready:true,downloading:false,error:null});
        case 'shared_kb_host_backup': return Promise.resolve({manifest:{format:1},recoveryCode:'AGE-SECRET-KEY-1MOCK'});
        case 'shared_kb_host_restore': return Promise.resolve({serverId:args?.serverId,name:'PINVOU Knowledge',endpoint:'https://127.0.0.1:3210',scope:'owner',deviceId:'local-owner'});
        case 'remote_kb_request_join': {
          if (window.__REMOTE_JOIN_PENDING_MODE__) {
            const pending={
              requestId:'pending-join-1',serverId:'pending-cube',serverIdentity:'pending-identity',
              serverName:'Pending Knowledge',endpoint:'https://192.168.1.21:3210',
              deviceName:args?.deviceName || 'Pending device',createdAt:1,expiresAt:9999999999
            };
            window.__REMOTE_PENDING_JOINS__=[pending];
            return Promise.resolve({
              status:'pending',request:{id:pending.requestId,status:'pending'},connection:null,pending
            });
          }
          const connection={
            serverId:'lan-cube',name:window.__REMOTE_JOIN_NAME__ || 'LAN Knowledge',endpoint:'https://192.168.1.20:3210',
            scope:'read',deviceId:'lan-reader'
          };
          window.__REMOTE_KB_JOIN__={args:{...(args||{})},connection:{...connection}};
          window.__REMOTE_KB_CONNECTIONS__=window.__REMOTE_KB_CONNECTIONS__
            .filter(item=>item.serverId!==connection.serverId).concat(connection);
          return Promise.resolve({status:'approved',request:{status:'approved'},connection,pending:null});
        }
        case 'remote_kb_refresh_join': {
          const pending=(window.__REMOTE_PENDING_JOINS__ || [])
            .find(item=>item.requestId===args?.requestId);
          return Promise.resolve({
            status:'pending',request:{id:args?.requestId,status:'pending'},connection:null,pending
          });
        }
        case 'remote_kb_cancel_join': {
          window.__REMOTE_PENDING_JOINS__=(window.__REMOTE_PENDING_JOINS__ || [])
            .filter(item=>item.requestId!==args?.requestId);
          return Promise.resolve({id:args?.requestId,status:'cancelled'});
        }
        case 'remote_kb_collections': return Promise.resolve([{
          id:101,name:'共享知识库',description:null,status:'ready',
          docCount:window.__REMOTE_DOCUMENT_PAGE_TEST__?201:2,
          chunkCount:window.__REMOTE_DOCUMENT_PAGE_TEST__?222:23,
          totalBytes:15409,createdAt:1,updatedAt:1,deletedAt:null
        }, ...(window.__REMOTE_EXTERNAL_COLLECTION__ ? [window.__REMOTE_EXTERNAL_COLLECTION__] : []),
          ...(window.__REMOTE_CREATED_COLLECTION__ ? [window.__REMOTE_CREATED_COLLECTION__] : [])]);
        case 'remote_kb_create_collection': {
          window.__REMOTE_CREATED_COLLECTION__={
            id:102,name:args?.name || 'Published',description:args?.description || null,status:'ready',
            docCount:0,chunkCount:0,totalBytes:0,createdAt:2,updatedAt:2,deletedAt:null
          };
          return Promise.resolve(window.__REMOTE_CREATED_COLLECTION__);
        }
        case 'remote_kb_documents': {
          const documents = [
            {id:201,collectionId:101,name:'code-plain-decoupling-改动说明.md',ext:'md',size:11196,sha256:'mock-a',status:'ready',nChunks:14,createdAt:1,updatedAt:1,deletedAt:null,error:null},
            {id:202,collectionId:101,name:'fork-modifications.en.md',ext:'md',size:4213,sha256:'mock-b',status:'ready',nChunks:9,createdAt:1,updatedAt:1,deletedAt:null,error:null}
          ];
          if (window.__REMOTE_EXTERNAL_DOCUMENT__?.collectionId === args?.collectionId) {
            documents.push(window.__REMOTE_EXTERNAL_DOCUMENT__);
          }
          if (window.__REMOTE_DOCUMENT_PAGE_TEST__) {
            for (let index = 0; index < 199; index += 1) {
              documents.push({
                id:1000+index,collectionId:101,name:'paged-'+index+'.md',ext:'md',size:128,
                sha256:'mock-page-'+index,status:'ready',nChunks:1,createdAt:1,updatedAt:1,
                deletedAt:null,error:null
              });
            }
          }
          if (window.__REMOTE_PENDING_DOCUMENT__) {
            window.__REMOTE_PENDING_POLLS__ = (window.__REMOTE_PENDING_POLLS__ || 0) + 1;
            const ready = !window.__REMOTE_PENDING_NEVER__ && window.__REMOTE_PENDING_POLLS__ >= 2;
            documents.push({id:205,collectionId:101,name:'pending.pdf',ext:'pdf',size:128,sha256:'mock-e',status:ready?'ready':'pending',nChunks:ready?2:0,createdAt:1,updatedAt:1,deletedAt:null,error:null});
            if (ready) { window.__REMOTE_PENDING_DOCUMENT__ = false; window.__REMOTE_PENDING_DOCUMENT_READY__ = true; }
          } else if (window.__REMOTE_PENDING_DOCUMENT_READY__) {
            documents.push({id:205,collectionId:101,name:'pending.pdf',ext:'pdf',size:128,sha256:'mock-e',status:'ready',nChunks:2,createdAt:1,updatedAt:1,deletedAt:null,error:null});
          }
          const deletedIds = new Set(window.__REMOTE_DELETED_DOCUMENT_IDS__ || []);
          const visibleDocuments = documents.filter(document => args?.includeDeleted || !deletedIds.has(document.id));
          const offset = Number.isInteger(args?.offset) ? args.offset : 0;
          const limit = Number.isInteger(args?.limit) ? args.limit : visibleDocuments.length;
          return Promise.resolve(visibleDocuments.slice(offset, offset + limit));
        }
        case 'remote_kb_document_statuses': {
          if (window.__REMOTE_MANY_PENDING__) {
            const ids = [...(args?.documentIds || [])];
            window.__REMOTE_STATUS_BATCHES__ = [...(window.__REMOTE_STATUS_BATCHES__ || []), ids];
            if (window.__REMOTE_BATCH_FAIL_SINGLETON_ONCE__ && ids.length === 1) {
              window.__REMOTE_BATCH_FAIL_SINGLETON_ONCE__ = false;
              return Promise.reject(new Error('mock singleton status batch failure'));
            }
            return Promise.resolve(ids.map(id => ({
              id,collectionId:101,name:'batch-'+id+'.pdf',ext:'pdf',size:128,sha256:'batch-'+id,
              status:'ready',nChunks:1,createdAt:1,updatedAt:1,deletedAt:null,error:null
            })));
          }
          if (window.__REMOTE_STATUS_FAILS__ > 0) {
            window.__REMOTE_STATUS_FAILS__ -= 1;
            return Promise.reject(new Error('mock transient status failure'));
          }
          const documents = [];
          if (window.__REMOTE_PENDING_DOCUMENT__) {
            window.__REMOTE_PENDING_POLLS__ = (window.__REMOTE_PENDING_POLLS__ || 0) + 1;
            const ready = !window.__REMOTE_PENDING_NEVER__ && window.__REMOTE_PENDING_POLLS__ >= 2;
            documents.push({id:205,collectionId:101,name:'pending.pdf',ext:'pdf',size:128,sha256:'mock-e',status:ready?'ready':'pending',nChunks:ready?2:0,createdAt:1,updatedAt:1,deletedAt:null,error:null});
            if (ready) { window.__REMOTE_PENDING_DOCUMENT__ = false; window.__REMOTE_PENDING_DOCUMENT_READY__ = true; }
          }
          return Promise.resolve(documents);
        }
        case 'remote_kb_discover_folder_files': return Promise.resolve({
          paths:['/home/x/team/plan.md','/home/x/team/reports/result.pdf'],skipped:1,limitExceeded:false
        });
        case 'remote_kb_delete_document': {
          const finish = () => {
            window.__REMOTE_DELETED_DOCUMENT_IDS__ = [...new Set([
              ...(window.__REMOTE_DELETED_DOCUMENT_IDS__ || []), args?.id
            ])];
          };
          if (window.__REMOTE_DEFER_DELETE_DOCUMENT__) {
            window.__REMOTE_DEFER_DELETE_DOCUMENT__ = false;
            return new Promise(resolve => {
              window.__REMOTE_RESOLVE_DELETE_DOCUMENT__ = () => { finish(); resolve(null); };
            });
          }
          finish();
          return Promise.resolve(null);
        }
        case 'remote_kb_restore_document':
          window.__REMOTE_DELETED_DOCUMENT_IDS__ = (window.__REMOTE_DELETED_DOCUMENT_IDS__ || [])
            .filter(id => id !== args?.id);
          return Promise.resolve(null);
        case 'remote_kb_upload_files':
          if (window.__REMOTE_MANY_PENDING__) {
            window.__REMOTE_BATCH_UPLOAD_COUNTER__ = (window.__REMOTE_BATCH_UPLOAD_COUNTER__ || 0) + 1;
            const id = 20000 + window.__REMOTE_BATCH_UPLOAD_COUNTER__;
            return Promise.resolve([{id,collectionId:101,name:'batch-'+id+'.pdf',ext:'pdf',size:128,sha256:'batch-'+id,status:'pending',nChunks:0,createdAt:1,updatedAt:1,deletedAt:null,error:null}]);
          }
          if (window.__REMOTE_UPLOAD_FAIL_ONCE__) {
            window.__REMOTE_UPLOAD_FAIL_ONCE__ = false;
            return Promise.reject(new Error('mock upload failure'));
          }
          if (window.__REMOTE_UPLOAD_DUPLICATE_ONCE__) {
            window.__REMOTE_UPLOAD_DUPLICATE_ONCE__ = false;
            return Promise.resolve([{id:201,collectionId:101,name:'code-plain-decoupling-改动说明.md',ext:'md',size:11196,sha256:'mock-a',status:'ready',nChunks:14,createdAt:1,updatedAt:1,deletedAt:null,error:null,alreadyExists:true}]);
          }
          if (window.__REMOTE_UPLOAD_INDEX_FAIL__) {
            window.__REMOTE_UPLOAD_INDEX_FAIL__ = false;
            return Promise.resolve([{id:204,collectionId:101,name:'新文档.pdf',ext:'pdf',size:128,sha256:'mock-d',status:'failed',nChunks:0,createdAt:1,updatedAt:1,deletedAt:null,error:'mock index failure'}]);
          }
          if (window.__REMOTE_UPLOAD_PENDING_ONCE__) {
            window.__REMOTE_UPLOAD_PENDING_ONCE__ = false;
            window.__REMOTE_PENDING_DOCUMENT__ = true;
            window.__REMOTE_PENDING_POLLS__ = 0;
            return Promise.resolve([{id:205,collectionId:101,name:'pending.pdf',ext:'pdf',size:128,sha256:'mock-e',status:'pending',nChunks:0,createdAt:1,updatedAt:1,deletedAt:null,error:null}]);
          }
          return Promise.resolve([{id:203,collectionId:101,name:'新文档.pdf',ext:'pdf',size:128,sha256:'mock-c',status:'ready',nChunks:1,createdAt:1,updatedAt:1,deletedAt:null,error:null}]);
        default: return Promise.resolve(null);
      }
    }
    window.__TAURI__={core:{invoke:invoke},event:{emit:function(){return Promise.resolve();},listen:function(){return Promise.resolve(function(){});}},
      window:{getCurrentWindow:function(){return {minimize(){},maximize(){},close(){},toggleMaximize(){},isMaximized(){return Promise.resolve(false);},onResized(){return Promise.resolve(function(){});},startDragging(){}};}},
      dialog:{save:function(){return Promise.resolve('/home/x/shared.pinbak');},open:function(options){if(options?.filters?.some(filter=>filter.extensions?.includes('pinbak')))return Promise.resolve('/home/x/shared.pinbak');if(options?.directory&&options?.multiple)return Promise.resolve(['/home/x/team']);return Promise.resolve(window.__REMOTE_MANY_PENDING__
        ? Array.from({length:501},(_,index)=>'/home/x/batch-'+index+'.pdf')
        : ['/home/x/新文档.pdf']);}}};
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

async function chooseRemoteUploadSource(page, testId) {
  await page.click('[data-testid="remote-upload-menu-toggle"]');
  await page.waitForSelector(`[data-testid="${testId}"]`);
  await page.click(`[data-testid="${testId}"]`);
}

(async () => {
  const { url: INDEX } = await startUiTestServer();
  const results = [];
  const rec = (name, pass, detail) => { results.push({ name, pass }); console.log(`${pass ? '✅' : '❌'} ${name}${detail ? '  ' + detail : ''}`); };
  const remoteKnowledgeSource = fs.readFileSync(path.join(__dirname, '../src/features/remote-knowledge/RemoteKnowledgeView.jsx'), 'utf8');
  rec('共享知识库危险操作不依赖 WebView window.confirm', !remoteKnowledgeSource.includes('window.confirm'), 'source contract');
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
  // 切到统一「知识库」视图(产出物已独立为一级菜单)
  const callsBeforeKnowledge = await page.evaluate(() => window.__KB_CALLS__.length);
  await page.click('[data-nav="knowledge"]');
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
  rec('⓪b 知识库首次加载不重复请求', initialCommands.every(cmd => initialKnowledgeCalls[cmd] === 1), JSON.stringify(initialKnowledgeCalls));

  await clickContains(page, 'button', '共享知识库');
  await sleep(500);
  const remoteEmbedded = await page.evaluate(() => {
    const heroArt = document.querySelector('[data-testid="remote-knowledge-hero-art"]');
    const brand = document.querySelector('[data-testid="remote-knowledge-brand"]');
    const artRect = heroArt?.getBoundingClientRect();
    const brandRect = brand?.getBoundingClientRect();
    const brandPosition = artRect && brandRect && artRect.width > 0 && artRect.height > 0 ? {
      x: ((brandRect.left + brandRect.width / 2) - artRect.left) / artRect.width,
      y: ((brandRect.top + brandRect.height / 2) - artRect.top) / artRect.height,
    } : null;
    return {
      currentView: document.querySelector('[data-testid="app-root"]')?.getAttribute('data-current-view'),
      embedded: document.querySelector('[data-testid="remote-knowledge-panel"]')?.getAttribute('data-embedded'),
      hero: !!document.querySelector('[data-testid="remote-knowledge-hero"]'),
      heroArt: heroArt?.complete && heroArt?.naturalWidth > 0,
      brand: !!brand,
      brandCentered: !!brandPosition && Math.abs(brandPosition.x - 0.728) < 0.005
        && Math.abs(brandPosition.y - 0.497) < 0.005,
      brandPosition,
      addServerVisible: !!document.querySelector('[data-testid="remote-add-server"]'),
      serverSummary: !!document.querySelector('[data-testid="remote-server-summary"]'),
      collectionsGrid: !!document.querySelector('[data-testid="remote-collections-grid"]'),
      documentsTable: !!document.querySelector('[data-testid="remote-documents-table"]'),
      remoteCalls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_connections').length,
    };
  });
  rec('⓪c 远程知识库嵌入统一知识库页面', remoteEmbedded.currentView === 'knowledge'
    && remoteEmbedded.embedded === 'true' && remoteEmbedded.hero && remoteEmbedded.heroArt && remoteEmbedded.brand && remoteEmbedded.brandCentered
    && remoteEmbedded.addServerVisible && remoteEmbedded.serverSummary
    && remoteEmbedded.collectionsGrid && remoteEmbedded.documentsTable && remoteEmbedded.remoteCalls === 1,
  JSON.stringify(remoteEmbedded));

  const selectedCollectionBefore = await page.evaluate(() => ({
    rows: document.querySelectorAll('[data-testid="remote-document-row"]').length,
    calls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_documents').length,
  }));
  await page.click('[data-testid="remote-collections-grid"] article[data-selected="true"] > button');
  await sleep(120);
  const selectedCollectionAfter = await page.evaluate(() => ({
    rows: document.querySelectorAll('[data-testid="remote-document-row"]').length,
    calls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_documents').length,
  }));
  rec('⓪c1 再次点击当前知识集不清空文档', selectedCollectionBefore.rows === 2
    && selectedCollectionAfter.rows === selectedCollectionBefore.rows
    && selectedCollectionAfter.calls === selectedCollectionBefore.calls,
  JSON.stringify({ selectedCollectionBefore, selectedCollectionAfter }));

  await page.click('[data-testid="remote-add-server"]');
  await page.waitForSelector('[data-testid="remote-connect-panel"]');
  const connectDialog = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="remote-connect-panel"][role="dialog"]');
    const nameInput = dialog?.querySelector('[data-testid="remote-device-name"]');
    const connectButton = dialog?.querySelector('[data-testid="remote-connect-submit"]');
    return {
      visible: !!dialog,
      mainForm: !!document.querySelector('[data-testid="remote-knowledge-panel"] > div > [data-testid="remote-connect-panel"]'),
      nameValue: nameInput?.value,
      namePlaceholder: nameInput?.placeholder,
      nameLabel: nameInput?.getAttribute('aria-label'),
      sourcePlaceholder: dialog?.querySelector('[data-testid="remote-invitation"]')?.placeholder,
      connectDisabled: connectButton?.disabled,
      discovering: !!dialog?.querySelector('[data-testid="remote-nearby-discovering"]'),
      discoveryCalls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'shared_kb_discover_nearby').length,
    };
  });
  if (process.env.KB_REMOTE_CONNECT_SCREENSHOT) {
    await page.screenshot({ path: path.resolve(process.env.KB_REMOTE_CONNECT_SCREENSHOT), fullPage: true });
  }
  rec('⓪c1 添加服务器使用二级弹窗且姓名默认为空', connectDialog.visible && !connectDialog.mainForm
    && connectDialog.nameValue === '' && connectDialog.namePlaceholder === '输入姓名'
    && connectDialog.nameLabel === '输入姓名' && connectDialog.sourcePlaceholder.includes('192.168.1.20')
    && connectDialog.connectDisabled && connectDialog.discovering && connectDialog.discoveryCalls === 1,
  JSON.stringify(connectDialog));
  await page.evaluate(() => window.__REMOTE_RESOLVE_DISCOVERY__?.());
  await page.waitForSelector('[data-testid="remote-nearby-list"]');
  const nearbyDiscovery = await page.evaluate(() => ({
    text: document.querySelector('[data-testid="remote-nearby-list"]')?.innerText || '',
    calls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'shared_kb_discover_nearby').length,
  }));
  rec('⓪c2 自动发现局域网知识库并显示可验证候选', nearbyDiscovery.calls === 1
    && nearbyDiscovery.text.includes('Nearby Knowledge')
    && nearbyDiscovery.text.includes('192.168.1.20:3210'), JSON.stringify(nearbyDiscovery));
  await page.type('[data-testid="remote-invitation"]', '192.168.1.20:3210');
  await page.type('[data-testid="remote-device-name"]', 'Alice');
  const directJoinReady = await page.evaluate(() => ({
    submitDisabled: document.querySelector('[data-testid="remote-connect-submit"]')?.disabled,
    invalid: document.querySelector('[data-testid="remote-invitation"]')?.getAttribute('aria-invalid'),
    help: document.querySelector('[data-testid="remote-join-source-help"]')?.innerText || '',
  }));
  rec('⓪c3 私网地址可检测但不会直接发送凭据', !directJoinReady.submitDisabled
    && directJoinReady.invalid === null && directJoinReady.help.includes('核对服务身份'),
  JSON.stringify(directJoinReady));
  await page.click('[data-testid="remote-connect-submit"]');
  await page.waitForSelector('[data-testid="remote-identity-confirmation"]');
  const identityConfirmation = await page.evaluate(() => ({
    code: document.querySelector('[data-testid="remote-identity-code"]')?.textContent || '',
    text: document.querySelector('[data-testid="remote-identity-confirmation"]')?.innerText || '',
    probeCalls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_probe_private_endpoint').length,
    joinCalls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_request_join_confirmed').length,
  }));
  rec('⓪c4 私网直连先展示稳定身份码', identityConfirmation.code === 'PINVOU-AABB-CCDD-EEFF-0011'
    && identityConfirmation.text.includes('Manual Knowledge')
    && identityConfirmation.probeCalls === 1 && identityConfirmation.joinCalls === 0,
  JSON.stringify(identityConfirmation));
  await page.evaluate(() => {
    const submit = document.querySelector('[data-testid="remote-connect-submit"]');
    submit?.click();
    submit?.click();
  });
  await page.waitForSelector('[data-testid="remote-join-feedback"][data-status="pending"]');
  const confirmedJoin = await page.evaluate(() => ({
    calls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_request_join_confirmed').length,
    args: window.__REMOTE_KB_CONFIRMED_JOIN__,
  }));
  rec('⓪c5 身份确认防重复提交且直连只创建待审批申请', confirmedJoin.calls === 1
    && confirmedJoin.args?.deviceName === 'Alice'
    && confirmedJoin.args?.confirmedIdentityCode === 'PINVOU-AABB-CCDD-EEFF-0011'
    && confirmedJoin.args?.confirmedCaFingerprint === 'AABBCCDDEEFF00112233445566778899',
  JSON.stringify(confirmedJoin));
  await page.click('[data-testid="remote-join-feedback-close"]');
  await page.waitForSelector('[data-testid="remote-connect-panel"]', { hidden: true });
  await page.waitForSelector('[data-testid="remote-pending-joins"]');
  await page.click('[data-testid="remote-cancel-pending-join"]');
  await page.waitForSelector('[data-testid="remote-pending-joins"]', { hidden: true });

  await page.click('[data-testid="remote-add-server"]');
  await page.waitForSelector('[data-testid="remote-invitation"]');
  await page.evaluate(() => {
    const input = document.querySelector('[data-testid="remote-invitation"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
    if (input && setter) {
      setter.call(input, '');
      input.dispatchEvent(new Event('input', { bubbles: true }));
    }
  });
  await page.type('[data-testid="remote-invitation"]', MOCK_SHARE_LINK);
  const joinForm = await page.evaluate(() => ({
    sourceValue: document.querySelector('[data-testid="remote-invitation"]')?.value,
    nameValue: document.querySelector('[data-testid="remote-device-name"]')?.value,
    submitDisabled: document.querySelector('[data-testid="remote-connect-submit"]')?.disabled,
    hasLegacyModes: !!document.querySelector('[data-testid="remote-connect-mode-lan"]'),
  }));
  rec('⓪c3 共享知识库使用单一加入入口', joinForm.sourceValue === MOCK_SHARE_LINK
    && joinForm.nameValue === 'Alice' && !joinForm.submitDisabled && !joinForm.hasLegacyModes,
  JSON.stringify(joinForm));

  await page.evaluate(() => {
    const submit = document.querySelector('[data-testid="remote-connect-submit"]');
    submit?.click();
    submit?.click();
  });
  await page.waitForSelector('[data-testid="remote-connect-panel"]', { hidden: true });
  await page.waitForFunction(() => {
    const summary = document.querySelector('[data-testid="remote-server-summary"]');
    return (summary?.innerText || '').includes('LAN Knowledge');
  });
  const lanConnected = await page.evaluate(() => {
    const panel = document.querySelector('[data-testid="remote-knowledge-panel"]');
    const summary = panel?.querySelector('[data-testid="remote-server-summary"]');
    const calls = (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_request_join');
    return {
      calls: calls.length,
      args: window.__REMOTE_KB_JOIN__?.args,
      fixedScope: window.__REMOTE_KB_JOIN__?.connection?.scope,
      selected: (summary?.innerText || '').includes('LAN Knowledge'),
      readOnly: (summary?.innerText || '').includes('只读'),
      upload: !!panel?.querySelector('[data-testid="remote-upload-menu-toggle"]'),
      notice: document.querySelector('[role="status"]')?.textContent || '',
    };
  });
  rec('⓪c3 加入防重复提交并立即选择获批连接', lanConnected.calls === 1
    && lanConnected.args?.source === MOCK_SHARE_LINK
    && lanConnected.args?.deviceName === 'Alice' && lanConnected.fixedScope === 'read'
    && lanConnected.selected && lanConnected.readOnly && !lanConnected.upload
    && lanConnected.notice.includes('已加入共享知识库'),
  JSON.stringify(lanConnected));

  await page.evaluate(() => {
    window.__REMOTE_JOIN_PENDING_MODE__ = true;
    document.querySelector('[data-testid="remote-add-server"]')?.click();
  });
  await page.type('[data-testid="remote-invitation"]', 'pinvou-knowledge://share/mock-pending');
  await page.type('[data-testid="remote-device-name"]', 'Pending Alice');
  await page.click('[data-testid="remote-connect-submit"]');
  await page.waitForSelector('[data-testid="remote-join-feedback"][data-status="pending"]');
  const pendingJoinFeedback = await page.evaluate(() => ({
    dialogVisible: !!document.querySelector('[data-testid="remote-connect-panel"]'),
    text: document.querySelector('[data-testid="remote-join-feedback"]')?.innerText || '',
  }));
  rec('加入待批准时在原弹窗平滑显示反馈，不会突然关闭页面', pendingJoinFeedback.dialogVisible
    && pendingJoinFeedback.text.includes('Pending Knowledge'), JSON.stringify(pendingJoinFeedback));
  await page.click('[data-testid="remote-join-feedback-close"]');
  await page.waitForSelector('[data-testid="remote-connect-panel"]', { hidden: true });
  await page.waitForSelector('[data-testid="remote-pending-joins"]');
  await page.evaluate(() => {
    delete window.__REMOTE_JOIN_PENDING_MODE__;
    document.querySelector('[data-testid="remote-cancel-pending-join"]')?.click();
  });
  await page.waitForSelector('[data-testid="remote-pending-joins"]', { hidden: true });

  const readOnlyPermission = await page.evaluate(() => {
    const panel = document.querySelector('[data-testid="remote-knowledge-panel"]');
    return {
      badge: (panel?.querySelector('[data-testid="remote-server-summary"]')?.innerText || '').includes('只读'),
      redundantHint: (panel?.innerText || '').includes('当前设备为只读权限'),
      upload: !!panel?.querySelector('[data-testid="remote-upload-menu-toggle"]'),
    };
  });
  rec('⓪d 远程只读设备使用权限标签并隐藏上传', readOnlyPermission.badge
    && !readOnlyPermission.redundantHint && !readOnlyPermission.upload,
    JSON.stringify(readOnlyPermission));

  const refreshClicked = await page.evaluate(() => {
    window.__REMOTE_KB_SCOPE__ = 'manage';
    const panel = document.querySelector('[data-testid="remote-knowledge-panel"]');
    const refresh = [...(panel?.querySelectorAll('button') || [])]
      .find(button => (button.textContent || '').trim() === '刷新');
    if (!refresh) return false;
    refresh.click();
    return true;
  });
  await page.waitForFunction(() => {
    const panel = document.querySelector('[data-testid="remote-knowledge-panel"]');
    return (panel?.innerText || '').includes('可管理')
      && !!panel?.querySelector('[data-testid="remote-upload-menu-toggle"]');
  }, { timeout: 5000 }).catch(() => {});
  const managedPermission = await page.evaluate(() => {
    const panel = document.querySelector('[data-testid="remote-knowledge-panel"]');
    return {
      manage: (panel?.innerText || '').includes('可管理'),
      hint: (panel?.innerText || '').includes('当前设备为只读权限'),
      upload: !!panel?.querySelector('[data-testid="remote-upload-menu-toggle"]'),
      remoteCalls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_connections').length,
    };
  });
  rec('⓪e 后台提权后刷新即显示上传入口', refreshClicked && managedPermission.manage
    && !managedPermission.hint && managedPermission.upload && managedPermission.remoteCalls === 3,
  JSON.stringify(managedPermission));

  await page.waitForFunction(() => !document.querySelector('[data-testid="remote-refresh-connections"]')?.disabled);
  await page.evaluate(() => {
    window.__REMOTE_HOST_STATUS__ = {
      supported:true,installed:true,running:true,endpoint:'https://127.0.0.1:3210',
      serviceVersion:'0.10.0',appVersion:'0.9.9',upgradeAvailable:true,clientOutdated:true
    };
    document.querySelector('[data-testid="remote-refresh-connections"]')?.click();
  });
  await page.waitForSelector('[data-testid="shared-kb-client-outdated"]', { timeout: 5000 }).catch(async error => {
    const debug = await page.evaluate(() => ({
      hostStatus: window.__REMOTE_HOST_STATUS__,
      calls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'shared_kb_host_status').length,
      panel: document.querySelector('[data-testid="remote-knowledge-panel"]')?.innerText || '',
      refreshDisabled: document.querySelector('[data-testid="remote-refresh-connections"]')?.disabled,
    }));
    throw new Error(`${error.message}: ${JSON.stringify(debug)}`);
  });
  const outdatedHostGuard = await page.evaluate(() => ({
    warning: document.querySelector('[data-testid="shared-kb-client-outdated"]')?.innerText || '',
    upgradeVisible: !!document.querySelector('[data-testid="shared-kb-upgrade-host"]'),
  }));
  rec('⓪f 旧客户端提示升级并禁止服务降级',
    outdatedHostGuard.warning.includes('0.9.9') && outdatedHostGuard.warning.includes('0.10.0')
      && !outdatedHostGuard.upgradeVisible,
    JSON.stringify(outdatedHostGuard));
  await page.waitForFunction(() => !document.querySelector('[data-testid="remote-refresh-connections"]')?.disabled);
  await page.evaluate(() => {
    delete window.__REMOTE_HOST_STATUS__;
    document.querySelector('[data-testid="remote-refresh-connections"]')?.click();
  });
  await page.waitForFunction(() => !document.querySelector('[data-testid="shared-kb-client-outdated"]'));

  await page.waitForFunction(() => !document.querySelector('[data-testid="remote-refresh-connections"]')?.disabled);
  const remoteContentRefreshBefore = await page.evaluate(() => ({
    collections: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_collections').length,
    documents: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_documents').length,
    hostStatus: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'shared_kb_host_status').length,
  }));
  await page.evaluate(() => {
    window.__REMOTE_EXTERNAL_COLLECTION__ = {
      id:103,name:'External refresh collection',description:null,status:'ready',
      docCount:1,chunkCount:3,totalBytes:256,createdAt:3,updatedAt:3,deletedAt:null
    };
    window.__REMOTE_EXTERNAL_DOCUMENT__ = {
      id:206,collectionId:101,name:'external-refresh.md',ext:'md',size:256,sha256:'mock-refresh',
      status:'ready',nChunks:3,createdAt:3,updatedAt:3,deletedAt:null,error:null
    };
    document.querySelector('[data-testid="remote-refresh-connections"]')?.click();
  });
  await page.waitForFunction(() => {
    const collections = document.querySelector('[data-testid="remote-collections-grid"]')?.innerText || '';
    const documents = [...document.querySelectorAll('[data-testid="remote-document-row"]')]
      .map(row => row.innerText).join('\n');
    return collections.includes('External refresh collection') && documents.includes('external-refresh.md');
  }, { timeout: 5000 }).catch(async error => {
    const debug = await page.evaluate(() => ({
      collections: document.querySelector('[data-testid="remote-collections-grid"]')?.innerText || '',
      documents: [...document.querySelectorAll('[data-testid="remote-document-row"]')].map(row => row.innerText),
      calls: (window.__KB_CALLS__ || []).filter(call => [
        'remote_kb_connections', 'remote_kb_collections', 'remote_kb_documents',
      ].includes(call.cmd)).slice(-8),
      refreshDisabled: document.querySelector('[data-testid="remote-refresh-connections"]')?.disabled,
    }));
    throw new Error(`${error.message}: ${JSON.stringify(debug)}`);
  });
  const remoteContentRefresh = await page.evaluate((before) => ({
    collectionVisible: (document.querySelector('[data-testid="remote-collections-grid"]')?.innerText || '')
      .includes('External refresh collection'),
    documentVisible: [...document.querySelectorAll('[data-testid="remote-document-row"]')]
      .some(row => (row.innerText || '').includes('external-refresh.md')),
    collectionCalls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_collections').length - before.collections,
    documentCalls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_documents').length - before.documents,
    hostStatusCalls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'shared_kb_host_status').length - before.hostStatus,
  }), remoteContentRefreshBefore);
  rec('shared knowledge refresh reloads collections and documents created by another client',
    remoteContentRefresh.collectionVisible && remoteContentRefresh.documentVisible
      && remoteContentRefresh.collectionCalls >= 1 && remoteContentRefresh.documentCalls >= 1
      && remoteContentRefresh.hostStatusCalls >= 1,
    JSON.stringify(remoteContentRefresh));
  await page.evaluate(() => {
    delete window.__REMOTE_EXTERNAL_COLLECTION__;
    delete window.__REMOTE_EXTERNAL_DOCUMENT__;
  });

  const firstDocumentRequest = await page.evaluate(() => (window.__KB_CALLS__ || [])
    .find(call => call.cmd === 'remote_kb_documents'));
  await page.evaluate(() => { window.__REMOTE_DOCUMENT_PAGE_TEST__ = true; });
  await page.click('[data-testid="remote-trash-toggle"]');
  await page.waitForSelector('[data-testid="remote-documents-load-more"]');
  const firstRemotePage = await page.evaluate(() => ({
    rows: document.querySelectorAll('[data-testid="remote-document-row"]').length,
    summary: document.querySelector('[data-testid="remote-documents-summary"]')?.innerText || '',
  }));
  await page.click('[data-testid="remote-documents-load-more"]');
  await page.waitForFunction(() => !document.querySelector('[data-testid="remote-documents-load-more"]'));
  const loadedRemotePages = await page.evaluate(() => ({
    rows: document.querySelectorAll('[data-testid="remote-document-row"]').length,
    calls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_documents').slice(-2),
  }));
  rec('⓪e1 远程文档按 200 条分页并显示真实总数', firstDocumentRequest?.args?.limit === 200
    && firstDocumentRequest?.args?.offset === 0
    && firstRemotePage.rows === 200 && firstRemotePage.summary.includes('201 份文档')
    && firstRemotePage.summary.includes('222 片段') && loadedRemotePages.rows === 201
    && loadedRemotePages.calls[0]?.args?.limit === 200 && loadedRemotePages.calls[0]?.args?.offset === 0
    && loadedRemotePages.calls[1]?.args?.limit === 200 && loadedRemotePages.calls[1]?.args?.offset === 200,
  JSON.stringify({ firstDocumentRequest, firstRemotePage, loadedRemotePages }));
  await page.evaluate(() => { window.__REMOTE_DOCUMENT_PAGE_TEST__ = false; });
  await page.click('[data-testid="remote-trash-toggle"]');
  await page.waitForFunction(() => document.querySelectorAll('[data-testid="remote-document-row"]').length === 2);

  await sleep(200);
  const refreshBeforeGeneration = await page.evaluate(() => {
    const refresh = document.querySelector('[data-testid="remote-refresh-connections"]');
    return { exists: !!refresh, disabled: refresh?.disabled, spinning: !!refresh?.querySelector('.animate-spin') };
  });
  if (!refreshBeforeGeneration.exists || refreshBeforeGeneration.disabled) {
    throw new Error(`refresh remained busy before generation test: ${JSON.stringify(refreshBeforeGeneration)}`);
  }
  await page.evaluate(() => {
    window.__REMOTE_KB_DEFER_CONNECTIONS_ONCE__ = true;
    document.querySelector('[data-testid="remote-refresh-connections"]')?.click();
  });
  await page.waitForFunction(() => typeof window.__REMOTE_KB_RESOLVE_CONNECTIONS__ === 'function');
  await page.evaluate(() => {
    window.__REMOTE_JOIN_NAME__ = 'LAN Knowledge New';
    document.querySelector('[data-testid="remote-add-server"]')?.click();
  });
  await page.type('[data-testid="remote-invitation"]', 'pinvou-knowledge://share/mock-generation');
  await page.type('[data-testid="remote-device-name"]', 'Generation Test');
  await page.click('[data-testid="remote-connect-submit"]');
  await sleep(500);
  const generationDebug = await page.evaluate(() => ({
    summary: document.querySelector('[data-testid="remote-server-summary"]')?.innerText || '',
    dialog: document.querySelector('[data-testid="remote-connect-panel"]')?.innerText || '',
    calls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_connections' || call.cmd === 'remote_kb_request_join'),
    joinName: window.__REMOTE_JOIN_NAME__,
  }));
  if (!generationDebug.summary.includes('LAN Knowledge New')) throw new Error(`connection generation setup failed: ${JSON.stringify(generationDebug)}`);
  const overlappingConnectionBusy = await page.evaluate(() => {
    const refresh = document.querySelector('[data-testid="remote-refresh-connections"]');
    return { disabled: refresh?.disabled, spinning: !!refresh?.querySelector('.animate-spin') };
  });
  await page.evaluate(() => {
    window.__REMOTE_KB_RESOLVE_CONNECTIONS__();
    delete window.__REMOTE_JOIN_NAME__;
  });
  await sleep(100);
  const connectionGeneration = await page.evaluate(() => {
    const refresh = document.querySelector('[data-testid="remote-refresh-connections"]');
    return {
      summary: document.querySelector('[data-testid="remote-server-summary"]')?.innerText || '',
      disabled: refresh?.disabled,
      spinning: !!refresh?.querySelector('.animate-spin'),
    };
  });
  rec('⓪e2 并发刷新保持 busy 计数且晚到旧连接不会复活', overlappingConnectionBusy.disabled
    && overlappingConnectionBusy.spinning && connectionGeneration.summary.includes('LAN Knowledge New')
    && !connectionGeneration.disabled && !connectionGeneration.spinning,
  JSON.stringify({ overlappingConnectionBusy, connectionGeneration }));

  await chooseRemoteUploadSource(page, 'remote-upload-folder');
  await page.waitForFunction(() => document.querySelectorAll('[data-testid="remote-upload-row"]').length === 2);
  const folderDiscovery = await page.evaluate(() => ({
    summary: document.querySelector('[data-testid="remote-folder-discovery-summary"]')?.innerText || '',
    names: [...document.querySelectorAll('[data-testid="remote-upload-row"]')].map(row => row.innerText),
    call: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_discover_folder_files').at(-1),
  }));
  rec('⓪e3 文件夹递归发现可导入文档并先预览数量', folderDiscovery.summary.includes('2 份文档')
    && folderDiscovery.summary.includes('已跳过 1 个') && folderDiscovery.names.some(name => name.includes('plan.md'))
    && folderDiscovery.names.some(name => name.includes('result.pdf'))
    && folderDiscovery.call?.args?.paths?.[0] === '/home/x/team',
  JSON.stringify(folderDiscovery));
  await page.keyboard.press('Escape');

  await chooseRemoteUploadSource(page, 'remote-upload-files');
  await page.waitForSelector('[data-testid="remote-upload-dialog"] [data-testid="remote-upload-row"]');
  const queuedRemoteUpload = await page.evaluate(() => ({
    dialog: !!document.querySelector('[data-testid="remote-upload-dialog"][role="dialog"]'),
    rows: document.querySelectorAll('[data-testid="remote-upload-row"]').length,
    text: document.querySelector('[data-testid="remote-upload-dialog"]')?.innerText || '',
  }));
  if (process.env.KB_REMOTE_UPLOAD_SCREENSHOT) {
    await page.screenshot({ path: path.resolve(process.env.KB_REMOTE_UPLOAD_SCREENSHOT), fullPage: true });
  }
  await page.evaluate(() => { window.__REMOTE_UPLOAD_FAIL_ONCE__ = true; });
  await page.evaluate(() => [...document.querySelectorAll('[data-testid="remote-upload-dialog"] button')]
    .find(button => (button.textContent || '').trim() === '上传')?.click());
  await page.waitForFunction(() => {
    const dialog = document.querySelector('[data-testid="remote-upload-dialog"]');
    const retry = [...(dialog?.querySelectorAll('button') || [])]
      .find(button => (button.textContent || '').includes('重试失败项'));
    return (dialog?.innerText || '').includes('上传失败') && retry && !retry.disabled;
  });
  const failedRemoteUpload = await page.evaluate(() => ({
    text: document.querySelector('[data-testid="remote-upload-dialog"]')?.innerText || '',
    calls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_upload_files').length,
  }));
  await page.evaluate(() => [...document.querySelectorAll('[data-testid="remote-upload-dialog"] button')]
    .find(button => (button.textContent || '').includes('重试失败项'))?.click());
  await page.waitForFunction(() => (document.querySelector('[data-testid="remote-upload-dialog"]')?.innerText || '').includes('已完成'));
  const finishedRemoteUpload = await page.evaluate(() => ({
    text: document.querySelector('[data-testid="remote-upload-dialog"]')?.innerText || '',
    calls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_upload_files').length,
  }));
  rec('⓪f 远程上传显示文件清单、索引状态与完成结果', queuedRemoteUpload.dialog
    && queuedRemoteUpload.rows === 1 && queuedRemoteUpload.text.includes('新文档.pdf')
    && failedRemoteUpload.text.includes('失败') && failedRemoteUpload.calls === 1
    && finishedRemoteUpload.text.includes('已完成') && finishedRemoteUpload.calls === 2,
  JSON.stringify({ queuedRemoteUpload, failedRemoteUpload, finishedRemoteUpload }));
  await page.keyboard.press('Escape');

  await chooseRemoteUploadSource(page, 'remote-upload-files');
  await page.waitForSelector('[data-testid="remote-upload-dialog"] [data-testid="remote-upload-row"]');
  await page.evaluate(() => { window.__REMOTE_UPLOAD_INDEX_FAIL__ = true; });
  await page.evaluate(() => [...document.querySelectorAll('[data-testid="remote-upload-dialog"] button')]
    .find(button => (button.textContent || '').trim() === '上传')?.click());
  await page.waitForFunction(() => {
    const dialog = document.querySelector('[data-testid="remote-upload-dialog"]');
    const close = [...(dialog?.querySelectorAll('button') || [])].find(button => (button.textContent || '').trim() === '完成');
    return (dialog?.innerText || '').includes('处理失败') && close && !close.disabled;
  });
  const indexedFailure = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="remote-upload-dialog"]');
    return {
      text: dialog?.innerText || '',
      retry: [...(dialog?.querySelectorAll('button') || [])].some(button => (button.textContent || '').includes('重试')),
      calls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_upload_files').length,
    };
  });
  rec('⓪g 已保存但索引失败时不重复上传', indexedFailure.text.includes('mock index failure')
    && indexedFailure.text.includes('处理失败') && !indexedFailure.retry && indexedFailure.calls === 3,
  JSON.stringify(indexedFailure));
  await page.keyboard.press('Escape');

  await chooseRemoteUploadSource(page, 'remote-upload-files');
  await page.waitForSelector('[data-testid="remote-upload-dialog"] [data-testid="remote-upload-row"]');
  await page.evaluate(() => { window.__REMOTE_UPLOAD_PENDING_ONCE__ = true; });
  await page.evaluate(() => [...document.querySelectorAll('[data-testid="remote-upload-dialog"] button')]
    .find(button => (button.textContent || '').trim() === '上传')?.click());
  await page.waitForFunction(() => {
    const dialog = document.querySelector('[data-testid="remote-upload-dialog"]');
    const row = dialog?.querySelector('[data-testid="remote-upload-row"]');
    const close = [...(dialog?.querySelectorAll('button') || [])]
      .find(button => (button.textContent || '').trim() === '收起');
    return row?.dataset.status === 'pending_index' && close && !close.disabled;
  });
  const pendingIndex = await page.evaluate(() => ({
    status: document.querySelector('[data-testid="remote-upload-row"]')?.dataset.status,
    text: document.querySelector('[data-testid="remote-upload-dialog"]')?.innerText || '',
  }));
  await page.waitForFunction(() => document.querySelector('[data-testid="remote-upload-row"]')?.dataset.status === 'success');
  const polledReady = await page.evaluate(() => ({
    status: document.querySelector('[data-testid="remote-upload-row"]')?.dataset.status,
    polls: window.__REMOTE_PENDING_POLLS__ || 0,
  }));
  rec('⓪h 上传完成后后台索引状态实时更新且不锁死弹窗', pendingIndex.status === 'pending_index'
    && pendingIndex.text.includes('处理中') && polledReady.status === 'success' && polledReady.polls >= 2,
  JSON.stringify({ pendingIndex, polledReady }));
  await page.keyboard.press('Escape');

  await page.evaluate(() => {
    window.__REMOTE_MANY_PENDING__ = true;
    window.__REMOTE_BATCH_FAIL_SINGLETON_ONCE__ = true;
    window.__REMOTE_BATCH_UPLOAD_COUNTER__ = 0;
    window.__REMOTE_STATUS_BATCHES__ = [];
    window.__REMOTE_UPLOAD_POLL_INTERVAL_MS__ = 500;
    window.__REMOTE_UPLOAD_POLL_TIMEOUT_MS__ = 10000;
  });
  await chooseRemoteUploadSource(page, 'remote-upload-files');
  await page.waitForFunction(() => document.querySelectorAll('[data-testid="remote-upload-row"]').length === 501);
  await page.evaluate(() => [...document.querySelectorAll('[data-testid="remote-upload-dialog"] button')]
    .find(button => (button.textContent || '').trim() === '上传')?.click());
  await page.waitForFunction(() => {
    const rows = [...document.querySelectorAll('[data-testid="remote-upload-row"]')];
    return rows.filter(row => row.dataset.status === 'success').length === 500
      && rows.filter(row => row.dataset.status === 'pending_index').length === 1;
  }, { polling: 10, timeout: 60000 });
  const partialBatchProgress = await page.evaluate(() => ({
    success: [...document.querySelectorAll('[data-testid="remote-upload-row"]')]
      .filter(row => row.dataset.status === 'success').length,
    pending: [...document.querySelectorAll('[data-testid="remote-upload-row"]')]
      .filter(row => row.dataset.status === 'pending_index').length,
  }));
  await page.waitForFunction(() => [...document.querySelectorAll('[data-testid="remote-upload-row"]')]
    .every(row => row.dataset.status === 'success'), { polling: 20, timeout: 60000 });
  const batchedRemoteStatuses = await page.evaluate(() => ({
    lengths: (window.__REMOTE_STATUS_BATCHES__ || []).map(ids => ids.length),
    success: [...document.querySelectorAll('[data-testid="remote-upload-row"]')]
      .filter(row => row.dataset.status === 'success').length,
  }));
  rec('⓪h1 501 份待索引文档按 500 分批且保留成功批次', partialBatchProgress.success === 500
    && partialBatchProgress.pending === 1 && batchedRemoteStatuses.success === 501
    && batchedRemoteStatuses.lengths.every(length => length > 0 && length <= 500)
    && batchedRemoteStatuses.lengths.includes(500)
    && batchedRemoteStatuses.lengths.filter(length => length === 1).length >= 2,
  JSON.stringify({ partialBatchProgress, batchedRemoteStatuses }));
  await page.evaluate(() => {
    window.__REMOTE_MANY_PENDING__ = false;
    delete window.__REMOTE_UPLOAD_POLL_INTERVAL_MS__;
    delete window.__REMOTE_UPLOAD_POLL_TIMEOUT_MS__;
  });
  await page.keyboard.press('Escape');

  await chooseRemoteUploadSource(page, 'remote-upload-files');
  await page.waitForSelector('[data-testid="remote-upload-dialog"] [data-testid="remote-upload-row"]');
  await page.evaluate(() => {
    window.__REMOTE_UPLOAD_PENDING_ONCE__ = true;
    window.__REMOTE_PENDING_NEVER__ = true;
    window.__REMOTE_STATUS_FAILS__ = 2;
    window.__REMOTE_UPLOAD_POLL_INTERVAL_MS__ = 2;
    window.__REMOTE_UPLOAD_POLL_TIMEOUT_MS__ = 30;
  });
  await page.evaluate(() => [...document.querySelectorAll('[data-testid="remote-upload-dialog"] button')]
    .find(button => (button.textContent || '').trim() === '上传')?.click());
  await page.waitForFunction(() => (document.querySelector('[data-testid="remote-upload-dialog"]')?.innerText || '').includes('仍在处理'));
  const timedOutIndex = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="remote-upload-dialog"]');
    const row = dialog?.querySelector('[data-testid="remote-upload-row"]');
    const close = [...(dialog?.querySelectorAll('button') || [])]
      .find(button => (button.textContent || '').trim() === '完成');
    return {
      status: row?.dataset.status,
      text: dialog?.innerText || '',
      spinning: !!row?.querySelector('.animate-spin'),
      closeDisabled: close?.disabled,
      polls: window.__REMOTE_PENDING_POLLS__ || 0,
    };
  });
  rec('⓪i 索引轮询容忍瞬时错误并在 60 秒语义超时后停止 spinner', timedOutIndex.status === 'pending_index'
    && timedOutIndex.text.includes('仍在处理') && !timedOutIndex.spinning
    && !timedOutIndex.closeDisabled && timedOutIndex.polls >= 1,
  JSON.stringify(timedOutIndex));
  await page.evaluate(() => {
    window.__REMOTE_PENDING_NEVER__ = false;
    delete window.__REMOTE_UPLOAD_POLL_INTERVAL_MS__;
    delete window.__REMOTE_UPLOAD_POLL_TIMEOUT_MS__;
  });
  await page.keyboard.press('Escape');

  await chooseRemoteUploadSource(page, 'remote-upload-files');
  await page.waitForSelector('[data-testid="remote-upload-dialog"] [data-testid="remote-upload-row"]');
  await page.evaluate(() => { window.__REMOTE_UPLOAD_DUPLICATE_ONCE__ = true; });
  await page.evaluate(() => [...document.querySelectorAll('[data-testid="remote-upload-dialog"] button')]
    .find(button => (button.textContent || '').trim() === '上传')?.click());
  await page.waitForFunction(() => document.querySelector('[data-testid="remote-upload-row"]')?.dataset.status === 'duplicate');
  const duplicateUpload = await page.evaluate(() => ({
    row: document.querySelector('[data-testid="remote-upload-row"]')?.innerText || '',
    notice: document.querySelector('[role="status"]')?.innerText || '',
  }));
  rec('⓪i1 相同内容返回已有文档并明确提示未重复导入', duplicateUpload.row.includes('已存在')
    && duplicateUpload.notice.includes('已存在 1'), JSON.stringify(duplicateUpload));
  await page.keyboard.press('Escape');

  await page.waitForSelector('[data-testid="remote-document-row"] [data-testid="remote-document-trash"]', { timeout: 5000 }).catch(async error => {
    const debug = await page.evaluate(() => ({
      panel: document.querySelector('[data-testid="remote-knowledge-panel"]')?.innerText || '',
      table: !!document.querySelector('[data-testid="remote-documents-table"]'),
      rows: document.querySelectorAll('[data-testid="remote-document-row"]').length,
      selectedCollection: document.querySelector('[data-selected="true"]')?.innerText || '',
      calls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_documents').slice(-5),
    }));
    throw new Error(`${error.message}: ${JSON.stringify(debug)}`);
  });
  const remoteRowsBeforeDelete = await page.$$eval('[data-testid="remote-document-row"]', rows => rows.length);
  await page.evaluate(() => { window.__REMOTE_DEFER_DELETE_DOCUMENT__ = true; });
  await page.click('[data-testid="remote-document-row"] [data-testid="remote-document-trash"]');
  const remoteDeleteConfirm = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="remote-document-trash-confirm"]');
    return {
      visible: !!dialog,
      text: dialog?.innerText || '',
      confirm: [...(dialog?.querySelectorAll('button') || [])]
        .some(button => (button.textContent || '').trim() === '移入回收站'),
    };
  });
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="remote-document-trash-confirm"]');
    [...(dialog?.querySelectorAll('button') || [])]
      .find(button => (button.textContent || '').trim() === '移入回收站')?.click();
  });
  const remoteOptimisticDelete = await page.evaluate(() => ({
    rows: document.querySelectorAll('[data-testid="remote-document-row"]').length,
    dialog: !!document.querySelector('[data-testid="remote-document-trash-confirm"]'),
    pending: typeof window.__REMOTE_RESOLVE_DELETE_DOCUMENT__ === 'function',
  }));
  await page.evaluate(() => window.__REMOTE_RESOLVE_DELETE_DOCUMENT__?.());
  await page.waitForFunction(() => (document.querySelector('[role="status"]')?.innerText || '').includes('已移入回收站'));
  const remoteDeleted = await page.evaluate(() => ({
    rows: document.querySelectorAll('[data-testid="remote-document-row"]').length,
    notice: document.querySelector('[role="status"]')?.innerText || '',
  }));
  rec('⓪i2 远程文档删除需明确确认且确认后立即从列表消失', remoteDeleteConfirm.visible
    && remoteDeleteConfirm.confirm && remoteDeleteConfirm.text.includes('可在“显示回收站”中恢复')
    && remoteOptimisticDelete.rows === remoteRowsBeforeDelete - 1 && !remoteOptimisticDelete.dialog
    && remoteOptimisticDelete.pending && remoteDeleted.rows === remoteRowsBeforeDelete - 1
    && remoteDeleted.notice.includes('已移入回收站'),
  JSON.stringify({ remoteRowsBeforeDelete, remoteDeleteConfirm, remoteOptimisticDelete, remoteDeleted }));

  const publishCallsBefore = await page.evaluate(() => ({
    create: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_create_collection').length,
    upload: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_upload_files').length,
  }));
  await page.click('[data-testid="remote-publish-local"]');
  await page.waitForSelector('[data-testid="remote-publish-dialog"]');
  await page.click('[data-testid="remote-publish-continue"]');
  await page.waitForFunction(() => document.querySelectorAll('[data-testid="remote-upload-row"]').length === 2);
  const publishPreview = await page.evaluate(() => ({
    rows: document.querySelectorAll('[data-testid="remote-upload-row"]').length,
    creates: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_create_collection').length,
  }));
  await page.keyboard.press('Escape');
  await page.waitForSelector('[data-testid="remote-upload-dialog"]', { hidden: true });
  const publishCancelled = await page.evaluate(() =>
    (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_create_collection').length);
  await page.click('[data-testid="remote-publish-local"]');
  await page.waitForSelector('[data-testid="remote-publish-dialog"]');
  await page.click('[data-testid="remote-publish-continue"]');
  await page.waitForFunction(() => document.querySelectorAll('[data-testid="remote-upload-row"]').length === 2);
  await page.evaluate(() => [...document.querySelectorAll('[data-testid="remote-upload-dialog"] button')]
    .find(button => (button.textContent || '').trim() === '上传')?.click());
  await page.waitForFunction(() => [...document.querySelectorAll('[data-testid="remote-upload-row"]')]
    .every(row => row.dataset.status === 'success'));
  const publishedLocalCollection = await page.evaluate((before) => {
    const creates = (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_create_collection');
    const uploads = (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_upload_files').slice(before.upload);
    return {
      createDelta: creates.length - before.create,
      createArgs: creates.at(-1)?.args,
      uploadCount: uploads.length,
      uploadCollectionIds: uploads.map(call => call.args?.collectionId),
      uploadPaths: uploads.flatMap(call => call.args?.paths || []),
    };
  }, publishCallsBefore);
  rec('⓪j 发布本地知识集仅在确认上传后创建共享副本', publishPreview.rows === 2
    && publishPreview.creates === publishCallsBefore.create
    && publishCancelled === publishCallsBefore.create
    && publishedLocalCollection.createDelta === 1
    && publishedLocalCollection.createArgs?.serverId === 'lan-cube'
    && publishedLocalCollection.uploadCount === 2
    && publishedLocalCollection.uploadCollectionIds.every(id => id === 102)
    && publishedLocalCollection.uploadPaths.includes('/home/x/路线图.md')
    && publishedLocalCollection.uploadPaths.includes('/home/x/扫描件.jpg'),
  JSON.stringify({ publishPreview, publishCancelled, publishedLocalCollection }));
  await page.keyboard.press('Escape');

  await page.evaluate(() => {
    window.__REMOTE_KB_SCOPE__ = 'owner';
    window.__REMOTE_KB_CONNECTIONS__ = window.__REMOTE_KB_CONNECTIONS__.map(connection => (
      connection.serverId === 'lan-cube'
        ? {...connection, endpoint:'https://127.0.0.1:3210', scope:'owner', deviceId:'local-owner'}
        : connection
    ));
    document.querySelector('[data-testid="remote-refresh-connections"]')?.click();
  });
  await page.waitForSelector('[data-testid="remote-govern"]');
  const localOwnerProtected = await page.evaluate(() => !document.querySelector('[data-testid="remote-disconnect"]'));
  await page.evaluate(() => { window.__REMOTE_KB_CONNECTIONS__ = []; });
  await page.waitForFunction(() => !document.querySelector('[data-testid="remote-refresh-connections"]')?.disabled);
  await page.click('[data-testid="remote-refresh-connections"]');
  await page.waitForSelector('[data-testid="shared-kb-reconnect-host"]');
  const recoveryEntry = await page.evaluate(() => ({
    createVisible: Boolean(document.querySelector('[data-testid="shared-kb-create-host"]')),
    reconnectVisible: Boolean(document.querySelector('[data-testid="shared-kb-reconnect-host"]')),
  }));
  await page.click('[data-testid="shared-kb-reconnect-host"]');
  await page.waitForSelector('[data-testid="remote-govern"]');
  await page.waitForSelector('[data-testid="shared-kb-host-progress"]', { hidden: true });
  const recoveredHost = await page.evaluate(() => ({
    reconnectCalls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'shared_kb_host_reconnect').length,
    disconnectVisible: Boolean(document.querySelector('[data-testid="remote-disconnect"]')),
  }));
  rec('local host owner cannot be disconnected and lost metadata exposes a recovery action',
    localOwnerProtected && recoveryEntry.reconnectVisible && !recoveryEntry.createVisible
      && recoveredHost.reconnectCalls === 1 && !recoveredHost.disconnectVisible,
  JSON.stringify({ localOwnerProtected, recoveryEntry, recoveredHost }));
  await page.evaluate(() => {
    window.__REMOTE_DEFER_OWNER_DEVICES_ONCE__ = true;
    window.__REMOTE_OWNER_DEVICE_NAME__ = 'Current PINVOU';
  });
  await page.click('[data-testid="remote-govern"]');
  await page.waitForSelector('[data-testid="remote-owner-panel"]');
  await page.click('[data-testid="remote-owner-panel"] button[aria-label="关闭"]');
  await page.click('[data-testid="remote-govern"]');
  await page.waitForFunction(() => (document.querySelector('[data-testid="remote-owner-panel"]')?.innerText || '').includes('Current PINVOU'));
  await page.evaluate(() => window.__REMOTE_RESOLVE_OWNER_DEVICES__?.());
  await sleep(120);
  const ownerRefreshIsolation = await page.evaluate(() => {
    const panel = document.querySelector('[data-testid="remote-owner-panel"]');
    return {
      text: panel?.innerText || '',
      closeLabel: panel?.querySelector('button[aria-label]')?.getAttribute('aria-label'),
    };
  });
  rec('成员管理关闭重开后忽略旧请求且关闭按钮使用当前语言',
    ownerRefreshIsolation.text.includes('Current PINVOU')
      && !ownerRefreshIsolation.text.includes('Stale PINVOU')
      && ownerRefreshIsolation.closeLabel === '关闭',
    JSON.stringify(ownerRefreshIsolation));
  const ownerJoinPollBefore = await page.evaluate(() => (window.__KB_CALLS__ || [])
    .filter(call => call.cmd === 'remote_kb_join_requests').length);
  await page.evaluate(() => {
    window.__REMOTE_OWNER_JOIN_REQUESTS__ = [{
      id:'owner-pending-1',deviceName:'New teammate',status:'pending',createdAt:1,expiresAt:9999999999
    }];
  });
  await page.waitForSelector('[data-testid="remote-owner-join-request"]', { timeout: 5000 });
  const ownerJoinAutoRefresh = await page.evaluate((before) => ({
    request: document.querySelector('[data-testid="remote-owner-join-request"]')?.innerText || '',
    badge: document.querySelector('[data-testid="remote-govern-pending-count"]')?.textContent || '',
    pollDelta: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_join_requests').length - before,
  }), ownerJoinPollBefore);
  rec('创建者端自动刷新加入申请并显示待处理数量', ownerJoinAutoRefresh.request.includes('New teammate')
    && ownerJoinAutoRefresh.badge === '1' && ownerJoinAutoRefresh.pollDelta >= 1,
  JSON.stringify(ownerJoinAutoRefresh));
  await page.evaluate(() => { window.__REMOTE_OWNER_JOIN_REQUESTS__ = []; });
  const ownerPeopleTab = await page.evaluate(() => ({
    selected: document.querySelector('[data-testid="remote-owner-people-tab"]')?.getAttribute('aria-selected'),
    hostSelected: document.querySelector('[data-testid="remote-owner-host-tab"]')?.getAttribute('aria-selected'),
    tabIndex: document.querySelector('[data-testid="remote-owner-people-tab"]')?.tabIndex,
    hostTabIndex: document.querySelector('[data-testid="remote-owner-host-tab"]')?.tabIndex,
    peoplePanelVisible: Boolean(document.querySelector('#remote-owner-people-panel')),
    backupVisible: Boolean(document.querySelector('[data-testid="shared-kb-backup"]')),
  }));
  rec('所有者面板默认聚焦成员与邀请并隐藏主机维护操作',
    ownerPeopleTab.selected === 'true' && ownerPeopleTab.hostSelected === 'false'
      && ownerPeopleTab.tabIndex === 0 && ownerPeopleTab.hostTabIndex === -1
      && ownerPeopleTab.peoplePanelVisible && !ownerPeopleTab.backupVisible,
  JSON.stringify(ownerPeopleTab));
  await page.focus('[data-testid="remote-owner-people-tab"]');
  await page.keyboard.press('ArrowRight');
  await page.waitForSelector('#remote-owner-host-panel');
  const keyboardHostTab = await page.evaluate(() => ({
    selected: document.querySelector('[data-testid="remote-owner-host-tab"]')?.getAttribute('aria-selected'),
    focused: document.activeElement?.getAttribute('data-testid'),
  }));
  await page.keyboard.press('ArrowLeft');
  await page.waitForSelector('#remote-owner-people-panel');
  await page.waitForFunction(() => document.activeElement?.getAttribute('data-testid') === 'remote-owner-people-tab');
  rec('所有者面板页签支持方向键切换和焦点跟随',
    keyboardHostTab.selected === 'true' && keyboardHostTab.focused === 'remote-owner-host-tab',
  JSON.stringify(keyboardHostTab));
  await page.waitForSelector('[data-testid="remote-share-other-endpoint"]');
  await page.evaluate(() => {
    document.querySelector('[data-testid="remote-share-other-endpoint"]')?.closest('details')?.setAttribute('open', '');
  });
  await page.type('[data-testid="remote-share-other-endpoint"]', 'cube.example.ts.net:3210');
  await page.click('[data-testid="remote-create-share"]');
  await page.waitForFunction(() => (window.__KB_CALLS__ || []).some(call => call.cmd === 'remote_kb_create_share'));
  const shareCreation = await page.evaluate(() => ({
    lanLookup: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'shared_kb_host_lan_endpoints').length,
    call: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'remote_kb_create_share').at(-1),
  }));
  const shareEndpoints = new Set(shareCreation.call?.args?.endpoints || []);
  rec('本机创建分享只写入可达地址并允许补充 Tailnet 地址',
    shareCreation.lanLookup === 1
      && shareEndpoints.has('https://192.168.1.20:3210')
      && shareEndpoints.has('cube.example.ts.net:3210')
      && !shareEndpoints.has('https://127.0.0.1:3210'),
    JSON.stringify(shareCreation));
  await page.evaluate(() => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: () => Promise.reject(new Error('clipboard denied')) },
    });
    document.execCommand = command => {
      window.__REMOTE_COPY_FALLBACK__ = command;
      return command === 'copy';
    };
  });
  await page.click('[data-testid="remote-copy-share"]');
  await page.waitForFunction(() => document.querySelector('[data-testid="remote-owner-panel"]')?.innerText.includes('共享链接已复制'));
  const shareCopy = await page.evaluate(() => ({
    fallback: window.__REMOTE_COPY_FALLBACK__,
    text: document.querySelector('[data-testid="remote-owner-panel"]')?.innerText || '',
  }));
  rec('分享链接复制失败时回退并给出成功反馈',
    shareCopy.fallback === 'copy' && shareCopy.text.includes('共享链接已复制'),
    JSON.stringify(shareCopy));
  await page.click('[data-testid="remote-owner-host-tab"]');
  await page.waitForSelector('[data-testid="shared-kb-backup"]');
  const ownerHostTab = await page.evaluate(() => ({
    selected: document.querySelector('[data-testid="remote-owner-host-tab"]')?.getAttribute('aria-selected'),
    peoplePanelVisible: Boolean(document.querySelector('#remote-owner-people-panel')),
    hostPanelVisible: Boolean(document.querySelector('#remote-owner-host-panel')),
    dangerText: document.querySelector('#remote-owner-host-panel')?.innerText || '',
  }));
  rec('服务维护独立成页并隔离停用与删除操作',
    ownerHostTab.selected === 'true' && !ownerHostTab.peoplePanelVisible
      && ownerHostTab.hostPanelVisible && ownerHostTab.dangerText.includes('停用与删除'),
  JSON.stringify(ownerHostTab));
  const hostRemoveCallsBeforeCancel = await page.evaluate(() => (window.__KB_CALLS__ || [])
    .filter(call => call.cmd === 'shared_kb_host_remove').length);
  await page.click('[data-testid="shared-kb-delete-host"]');
  await page.waitForSelector('[data-testid="shared-kb-delete-host-confirm"]');
  const hostDeleteConfirmation = await page.evaluate(() => ({
    modalCount: document.querySelectorAll('[aria-modal="true"]').length,
    ownerPanelVisible: Boolean(document.querySelector('[data-testid="remote-owner-panel"]')),
    text: document.querySelector('[data-testid="shared-kb-delete-host-confirm"]')?.innerText || '',
    cancelFocused: document.activeElement?.getAttribute('data-testid'),
  }));
  await page.click('[data-testid="shared-kb-delete-host-confirm-cancel"]');
  await page.waitForSelector('[data-testid="remote-owner-panel"]');
  await page.waitForFunction(() => document.activeElement?.getAttribute('data-testid') === 'remote-owner-host-tab');
  const hostRemoveCallsAfterCancel = await page.evaluate(() => (window.__KB_CALLS__ || [])
    .filter(call => call.cmd === 'shared_kb_host_remove').length);
  rec('所有者危险操作使用单一产品确认弹窗且取消不执行',
    hostDeleteConfirmation.modalCount === 1 && !hostDeleteConfirmation.ownerPanelVisible
      && hostDeleteConfirmation.text.includes('删除服务和数据')
      && hostDeleteConfirmation.cancelFocused === 'shared-kb-delete-host-confirm-cancel'
      && hostRemoveCallsAfterCancel === hostRemoveCallsBeforeCancel,
  JSON.stringify({ hostDeleteConfirmation, hostRemoveCallsBeforeCancel, hostRemoveCallsAfterCancel }));
  await page.click('[data-testid="shared-kb-backup"]');
  await page.waitForSelector('[data-testid="shared-kb-recovery-code"]');
  const encryptedBackup = await page.evaluate(() => ({
    code: document.querySelector('[data-testid="shared-kb-recovery-code"] textarea')?.value || '',
    call: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'shared_kb_host_backup').at(-1),
    modalCount: document.querySelectorAll('[aria-modal="true"]').length,
  }));
  await page.evaluate(() => { document.execCommand = () => false; });
  await page.click('[data-testid="shared-kb-copy-recovery"]');
  await page.waitForFunction(() => document.querySelector('[data-testid="shared-kb-recovery-code"]')?.innerText.includes('复制失败'));
  const recoveryCopyFeedback = await page.evaluate(() => ({
    alert: document.querySelector('[data-testid="shared-kb-recovery-code"] [role="alert"]')?.innerText || '',
  }));
  rec('恢复码复制失败时在当前弹窗明确提示手动复制',
    recoveryCopyFeedback.alert.includes('复制失败') && recoveryCopyFeedback.alert.includes('手动'),
    JSON.stringify(recoveryCopyFeedback));
  await page.click('[data-testid="shared-kb-recovery-done"]');

  await page.click('[data-testid="shared-kb-restore"]');
  await page.waitForSelector('[data-testid="shared-kb-restore-dialog"]');
  const restoreCallsBeforeCancel = await page.evaluate(() => (window.__KB_CALLS__ || [])
    .filter(call => call.cmd === 'shared_kb_host_restore').length);
  await page.click('[data-testid="shared-kb-restore-submit"]');
  await page.waitForSelector('[data-testid="shared-kb-restore-confirm"]');
  const sameHostConfirmation = await page.evaluate(() => ({
    modalCount: document.querySelectorAll('[aria-modal="true"]').length,
    restoreDialogVisible: Boolean(document.querySelector('[data-testid="shared-kb-restore-dialog"]')),
    text: document.querySelector('[data-testid="shared-kb-restore-confirm"]')?.innerText || '',
  }));
  await page.click('[data-testid="shared-kb-restore-confirm-cancel"]');
  await page.waitForSelector('[data-testid="shared-kb-restore-dialog"]');
  const restoreCallsAfterCancel = await page.evaluate(() => (window.__KB_CALLS__ || [])
    .filter(call => call.cmd === 'shared_kb_host_restore').length);
  await page.click('[data-testid="shared-kb-restore-submit"]');
  await page.waitForSelector('[data-testid="shared-kb-restore-confirm"]');
  await page.click('[data-testid="shared-kb-restore-confirm-submit"]');
  await page.waitForFunction((before) => (window.__KB_CALLS__ || [])
    .filter(call => call.cmd === 'shared_kb_host_restore').length > before, {}, restoreCallsBeforeCancel);
  await page.waitForSelector('[data-testid="shared-kb-restore-dialog"]', { hidden: true });
  await page.waitForSelector('[data-testid="shared-kb-restore"]');
  await page.click('[data-testid="shared-kb-restore"]');
  await page.waitForSelector('[data-testid="shared-kb-restore-dialog"]');
  await page.type('[data-testid="shared-kb-restore-dialog"] textarea', 'AGE-SECRET-KEY-1MOVE');
  await page.click('[data-testid="shared-kb-restore-submit"]');
  await page.waitForSelector('[data-testid="shared-kb-restore-confirm"]');
  const migrationConfirmation = await page.evaluate(() => ({
    modalCount: document.querySelectorAll('[aria-modal="true"]').length,
    restoreDialogVisible: Boolean(document.querySelector('[data-testid="shared-kb-restore-dialog"]')),
    text: document.querySelector('[data-testid="shared-kb-restore-confirm"]')?.innerText || '',
  }));
  await page.click('[data-testid="shared-kb-restore-confirm-submit"]');
  await page.waitForFunction(() => (window.__KB_CALLS__ || [])
    .filter(call => call.cmd === 'shared_kb_host_restore').length >= 2);
  await page.waitForSelector('[data-testid="shared-kb-restore-dialog"]', { hidden: true });
  const restoreCalls = await page.evaluate(() => (window.__KB_CALLS__ || [])
    .filter(call => call.cmd === 'shared_kb_host_restore').slice(-2));
  rec('shared backup and restore keep same-host and migration flows explicit',
    encryptedBackup.code === 'AGE-SECRET-KEY-1MOCK'
      && encryptedBackup.modalCount === 1
      && encryptedBackup.call?.args?.destination === '/home/x/shared.pinbak'
      && sameHostConfirmation.modalCount === 1 && !sameHostConfirmation.restoreDialogVisible
      && sameHostConfirmation.text.includes('恢复共享知识库')
      && restoreCallsAfterCancel === restoreCallsBeforeCancel
      && migrationConfirmation.modalCount === 1 && !migrationConfirmation.restoreDialogVisible
      && restoreCalls.length === 2
      && restoreCalls[0]?.args?.source === '/home/x/shared.pinbak'
      && restoreCalls[0]?.args?.recoveryCode === null
      && restoreCalls[1]?.args?.recoveryCode === 'AGE-SECRET-KEY-1MOVE',
    JSON.stringify({ encryptedBackup, sameHostConfirmation, restoreCallsBeforeCancel, restoreCallsAfterCancel, migrationConfirmation, restoreCalls }));
  await page.keyboard.press('Escape');

  if (process.env.KB_REMOTE_SCREENSHOT) {
    await page.evaluate(() => {
      document.querySelector('[role="status"] button')?.click();
      let node = document.querySelector('[data-testid="remote-knowledge-panel"]');
      while (node) {
        node.scrollTop = 0;
        node = node.parentElement;
      }
      document.scrollingElement?.scrollTo({ top: 0 });
    });
    await sleep(200);
    await page.screenshot({ path: path.resolve(process.env.KB_REMOTE_SCREENSHOT), fullPage: true });
  }

  await clickContains(page, 'button', '本地文件管理');
  await sleep(1500);

  const filesView = await page.evaluate(() => {
    const x = document.body.innerText;
    return { entered: document.querySelector('[data-testid="app-root"]')?.getAttribute('data-current-view') === 'knowledge' || x.includes('知识库'), subFiles: x.includes('本地文件管理'), subKb: x.includes('本地知识库'), remote: x.includes('共享知识库'),
      cats: x.includes('文档') && x.includes('PDF') && x.includes('图片'),
      fileRow: x.includes('季度财报.xlsx') || x.includes('合作协议.pdf') };
  });
  rec('① 进入统一视图 + 三分区与文件管理渲染', filesView.entered && filesView.subFiles && filesView.subKb && filesView.remote && filesView.cats && filesView.fileRow, JSON.stringify(filesView));

  // 文件行「加入知识库」浮层
  await page.evaluate(() => { const b = [...document.querySelectorAll('button[title]')].find(b => (b.getAttribute('title')||'').includes('加入知识库')); if (b) b.click(); });
  await sleep(500);
  const addPop = await page.evaluate(() => document.body.innerText.includes('产品资料库') && document.body.innerText.includes('加入知识库'));
  rec('② 文件行「加入知识库」浮层列出知识集', addPop);
  await page.evaluate(() => { const ov = document.querySelector('.bg-black\\/40'); if (ov) ov.click(); });
  await sleep(300);

  // 切「本地知识库」subtab
  await clickContains(page, 'button', '本地知识库');
  await sleep(1200);
  const kbView = await page.evaluate(() => {
    const x = document.body.innerText;
    return { banner: x.includes('一键构建') || x.includes('AI 知识库'), card: x.includes('产品资料库'), status: x.includes('已就绪') || x.includes('解析中'), collFiles: x.includes('知识库内文件') };
  });
  rec('③ 本地知识库 subtab(banner/知识集卡片/状态)', kbView.banner && kbView.card && kbView.status, JSON.stringify(kbView));

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
    return { scoped: reset, docList: x.includes('路线图.md'), addBtn: x.includes('添加') };
  });
  rec('④ 聚焦知识集后显示范围/文档列表/添加文件', focused.scoped && focused.docList && focused.addBtn, JSON.stringify(focused));

  // 聚焦后添加文件：先点「添加」触发按钮打开下拉菜单，再点「文件」菜单项；dialog mock 返回路径，必须透传到当前知识集。
  await page.evaluate(() => { const b = [...document.querySelectorAll('button')].find(b => (b.textContent || '').includes('添加') && !b.disabled); if (b) b.click(); });
  await sleep(300);
  await page.evaluate(() => { const item = document.querySelector('[data-testid="kb-add-files"]'); if (item) item.click(); });
  await sleep(500);
  const added = await page.evaluate(() => window.__KB_CALLS__.some(c => c.cmd === 'kb_collection_add_sources'
    && c.args && c.args.collectionId === 1 && Array.isArray(c.args.paths) && c.args.paths.includes('/home/x/新文档.pdf')));
  rec('⑤ 添加文件透传当前知识集和所选路径', added);

  // ⑤b 添加文件夹：步骤⑤触发索引后「添加」按钮被禁用，先等轮询将索引复位（mock 返回 idle），
  // 再点「添加」→「文件夹」菜单项；dialog mock 仍返回路径，断言同样透传到当前知识集。
  await page.waitForFunction(() => {
    const b = [...document.querySelectorAll('button')].find(b => (b.textContent || '').includes('添加') && !b.disabled);
    return !!b;
  }, { timeout: 5000 });
  const addSourcesBefore = await page.evaluate(() => window.__KB_CALLS__.filter(c => c.cmd === 'kb_collection_add_sources').length);
  await page.evaluate(() => { const b = [...document.querySelectorAll('button')].find(b => (b.textContent || '').includes('添加') && !b.disabled); if (b) b.click(); });
  await sleep(300);
  await page.evaluate(() => { const item = document.querySelector('[data-testid="kb-add-folder"]'); if (item) item.click(); });
  await sleep(500);
  const addedFolder = await page.evaluate((before) => {
    const calls = window.__KB_CALLS__.filter(c => c.cmd === 'kb_collection_add_sources');
    if (calls.length <= before) return false;
    const last = calls[calls.length - 1];
    return last.args && last.args.collectionId === 1 && Array.isArray(last.args.paths) && last.args.paths.length > 0;
  }, addSourcesBefore);
  rec('⑤b 添加文件夹同样透传当前知识集与所选路径', addedFolder);

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
  await clickContains(page, 'button', '本地知识库'); await sleep(700);
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
    .find(b => (b.textContent || '').trim() === '本地知识库')?.click());
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
    .find(b => (b.textContent || '').trim() === '本地知识库')?.click());
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
      .find(b => (b.textContent || '').trim() === '本地知识库')?.click());
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

  const localRowsBeforeDelete = await page.$$eval('[data-testid="kb-remove-document"]', buttons => buttons.length);
  await page.evaluate(() => { window.__KB_DEFER_REMOVE_DOCUMENT__ = true; });
  await page.click('[data-testid="kb-remove-document"]');
  const localDeleteConfirm = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="kb-remove-document-confirm"]');
    return {
      visible: !!dialog,
      text: dialog?.innerText || '',
      confirm: [...(dialog?.querySelectorAll('button') || [])]
        .some(button => (button.textContent || '').trim() === '移除'),
    };
  });
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="kb-remove-document-confirm"]');
    [...(dialog?.querySelectorAll('button') || [])]
      .find(button => (button.textContent || '').trim() === '移除')?.click();
  });
  const localOptimisticDelete = await page.evaluate(() => ({
    rows: document.querySelectorAll('[data-testid="kb-remove-document"]').length,
    dialog: !!document.querySelector('[data-testid="kb-remove-document-confirm"]'),
    pending: typeof window.__KB_RESOLVE_REMOVE_DOCUMENT__ === 'function',
  }));
  await page.evaluate(() => window.__KB_RESOLVE_REMOVE_DOCUMENT__?.());
  await page.waitForFunction(before => document.querySelectorAll('[data-testid="kb-remove-document"]').length === before - 1, {}, localRowsBeforeDelete);
  const localDeleted = await page.evaluate(() => ({
    rows: document.querySelectorAll('[data-testid="kb-remove-document"]').length,
    calls: (window.__KB_CALLS__ || []).filter(call => call.cmd === 'kb_remove_document').length,
  }));
  rec('⑩ 本地知识库移除使用正式确认弹窗且确认后立即更新列表', localDeleteConfirm.visible
    && localDeleteConfirm.confirm && localDeleteConfirm.text.includes('磁盘上的原文件不受影响')
    && localOptimisticDelete.rows === localRowsBeforeDelete - 1 && !localOptimisticDelete.dialog
    && localOptimisticDelete.pending && localDeleted.rows === localRowsBeforeDelete - 1
    && localDeleted.calls === 1,
  JSON.stringify({ localRowsBeforeDelete, localDeleteConfirm, localOptimisticDelete, localDeleted }));

  await page.evaluate(() => {
    window.__KB_FAIL_REMOVE_DOCUMENT__ = true;
    window.__KB_CONCURRENT_DOCUMENT__ = {
      id: 13, collectionId: 1, collName: '产品资料库', path: '/home/x/并发新增.md',
      name: '并发新增.md', ext: 'md', size: 1200, mtime: 1700000001,
      parseStatus: 'parsed', nChunks: 2,
    };
  });
  await page.click('[data-testid="kb-remove-document"]');
  await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="kb-remove-document-confirm"]');
    [...(dialog?.querySelectorAll('button') || [])]
      .find(button => (button.textContent || '').trim() === '移除')?.click();
  });
  await page.waitForFunction(() => document.body.innerText.includes('移除失败'));
  await page.waitForFunction(() => document.querySelectorAll('[data-testid="kb-remove-document"]').length === 2);
  const failedDeleteRefresh = await page.evaluate(() => ({
    rows: document.querySelectorAll('[data-testid="kb-remove-document"]').length,
    text: document.body.innerText,
  }));
  rec('⑩b 本地移除失败重新读取权威状态且不覆盖并发新增文档',
    failedDeleteRefresh.rows === 2 && failedDeleteRefresh.text.includes('并发新增.md'),
    JSON.stringify({ rows: failedDeleteRefresh.rows, hasConcurrent: failedDeleteRefresh.text.includes('并发新增.md') }));

  rec('⑪ 全程无运行时报错(ReferenceError 等)', errs.length === 0, errs.length ? errs.slice(0,3).join(' | ') : '');

  await browser.close();
  const failed = results.filter(r => !r.pass).length;
  console.log(failed ? `\n❌ ${failed}/${results.length} FAILED` : `\n✅ ALL ${results.length} PASS`);
  process.exit(failed ? 1 : 0);
})().catch(e => { console.error('FATAL', e.message); process.exit(1); });
