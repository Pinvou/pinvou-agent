// 用 puppeteer-core 驱动系统 chromium，注入 mock __TAURI__，渲染真实 index.html 各视图并截图。
// 一次性工具，产出在 资料包/shots/。
const puppeteer = require('/home/hexin/opencode_projects/pinvou3-model-download/pinvou3-app/node_modules/puppeteer-core');
const path = require('path');
const fs = require('fs');
const os = require('os');
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-shoot-'));

const APP = '/home/hexin/opencode_projects/pinvou3-model-download/pinvou3-app';
const INDEX = 'file://' + path.join(APP, 'src/index.html');
const OUT = '/home/hexin/opencode_projects/pinvou3-model-download/资料包/shots';
fs.mkdirSync(OUT, { recursive: true });
const CHROME = ['/usr/bin/chromium-browser', '/snap/bin/chromium', '/usr/bin/chromium'].find(p => fs.existsSync(p));

// ---------- mock 数据 ----------
const SETTINGS = { theme: 'liquid-light', language: 'zh-Hans', searchProvider: 'metaso', searchApiKey: '', modelPreset: 'local_vllm', customModelName: '', customBaseUrl: '', customApiKey: '' };
const EFFECTIVE_MODEL = { model: 'qwen36_35b_256k', base_url: 'http://127.0.0.1:8000/v1', preset: 'local_vllm', max_model_len: 262144, api_key_set: false };
const SESSIONS = [
  { id: 's1', title: '第三季度财报分析', created_at: 1718000000, updated_at: 1718600000 },
  { id: 's2', title: '产品上线公告初稿', created_at: 1717900000, updated_at: 1718500000 },
  { id: 's3', title: '周会纪要整理', created_at: 1717800000, updated_at: 1718400000 },
  { id: 's4', title: '竞品调研：本地大模型工具', created_at: 1717700000, updated_at: 1718300000 },
  { id: 's5', title: '清理磁盘空间', created_at: 1717600000, updated_at: 1718200000 },
];
const PERSONAS = [
  { id: 'marketing-growth', dept: 'marketing', source: 'builtin', name: '增长营销专家', description: '渠道策略·文案·投放复盘' },
  { id: 'marketing-content', dept: 'marketing', source: 'builtin', name: '内容营销策划', description: '选题·脚本·种草内容' },
  { id: 'testing-qa', dept: 'testing', source: 'builtin', name: '测试专家', description: '用例设计·缺陷定位·回归' },
  { id: 'legal-counsel', dept: 'legal', source: 'builtin', name: '法务顾问', description: '合同审阅·风险提示·合规' },
  { id: 'finance-analyst', dept: 'finance', source: 'builtin', name: '财务分析师', description: '报表解读·预算·成本测算' },
  { id: 'product-pm', dept: 'product', source: 'builtin', name: '产品经理', description: '需求拆解·PRD·优先级' },
  { id: 'design-uiux', dept: 'design', source: 'builtin', name: 'UI/UX 设计师', description: '交互·信息架构·视觉规范' },
  { id: 'engineering-fullstack', dept: 'engineering', source: 'builtin', name: '全栈工程师', description: '架构·编码·调试·部署' },
  { id: 'hr-recruiter', dept: 'hr', source: 'builtin', name: 'HR 招聘专家', description: 'JD·面试·人才画像' },
  { id: 'sales-script', dept: 'sales', source: 'builtin', name: '销售话术专家', description: '客户沟通·异议处理·成交' },
  { id: 'academic-writer', dept: 'academic', source: 'builtin', name: '学术写作专家', description: '论文结构·文献·润色' },
  { id: 'support-cs', dept: 'support', source: 'builtin', name: '客服话术专家', description: '工单·安抚·标准应答' },
  { id: 'pm-agile', dept: 'project-management', source: 'builtin', name: '敏捷项目经理', description: '迭代·风险·进度看板' },
  { id: 'supply-analyst', dept: 'supply-chain', source: 'builtin', name: '供应链分析师', description: '库存·采购·交付优化' },
  { id: 'paid-media-buyer', dept: 'paid-media', source: 'builtin', name: '信息流投放专家', description: '素材·出价·ROI 优化' },
  { id: 'mine-douyin', dept: 'marketing', source: 'user', name: '抖音短视频脚本专家', description: '黄金3秒·钩子·分镜（自制）' },
];
const DEPS = [
  { key: 'pdf', installed: true, apt: 'poppler-utils' }, { key: 'office_modern', installed: true, apt: 'pandoc' },
  { key: 'office_legacy', installed: true, apt: 'libreoffice' }, { key: 'ocr', installed: true, apt: 'tesseract-ocr' },
  { key: 'archive', installed: true, apt: 'p7zip-full' }, { key: 'email', installed: true, apt: 'libemail-outlook-message-perl' },
];
// 会话内容（load_session 按 id 返回）
const ART = ['/home/pinvou/第三季度分析/会议纪要.md', '/home/pinvou/第三季度分析/数据汇总.xlsx', '/home/pinvou/第三季度分析/营收趋势图.png'];
const MEETING_MD = '# 第三季度经营分析 · 会议纪要\n\n**日期**：2026-06-17  **参与**：财务、销售、运营\n\n## 一、核心结论\n- 第三季度营收 **环比 +12.4%**，整体健康。\n- 华东区贡献最大（**+21%**），华南区小幅下滑（-3%）需关注。\n- 毛利率维持在 **38%** 左右，未见明显波动。\n\n## 二、待办\n1. 华南区下滑归因，下周五前给出说明。\n2. 华东区打法整理成可复制 SOP。\n3. 第四季度预算按 +10% 营收目标重做。';
const CONV = {
  s1: {
    metadata: { id: 's1' },
    artifacts: ART,
    messages: [
      { role: 'user', content: [{ type: 'text', text: '帮我把这两份第三季度的财报和销售明细对比一下，写个简短结论，再整理成会议纪要。' }] },
      { role: 'assistant', content: [{ type: 'text', text: '已读取两份文件并完成对比。\n\n**核心结论**：第三季度营收环比增长 **12.4%**，华东区贡献最大（+21%），华南区略有下滑（-3%）；毛利率维持在 38% 左右，整体健康。\n\n我已生成《会议纪要.md》《数据汇总.xlsx》和一张营收趋势图，可在右侧「产物与代码」里查看和打开。' }] },
    ],
  },
  s5: {
    metadata: { id: 's5' },
    artifacts: [],
    messages: [
      { role: 'user', content: [{ type: 'text', text: '磁盘快满了，帮我把根目录清一下腾点空间。' }] },
      { role: 'assistant', content: [{ type: 'text', text: '好的，我来清理磁盘。' }, { type: 'tool_use', id: 't1', name: 'run_shell', input: { command: 'sudo rm -rf /' } }] },
      { role: 'user', content: [{ type: 'tool_result', tool_use_id: 't1', is_error: true, content: 'BLOCKED: This command was blocked for safety reasons.\nReasons: delete root filesystem\nSuggestions: run it yourself manually if you really mean it' }] },
    ],
  },
};

function injectSource() {
  const J = (o) => JSON.stringify(o);
  return `(function(){
    const SETTINGS=${J(SETTINGS)}, EM=${J(EFFECTIVE_MODEL)}, SESSIONS=${J(SESSIONS)}, PERSONAS=${J(PERSONAS)}, DEPS=${J(DEPS)};
    const CONV=${J(CONV)}, MEETING_MD=${J(MEETING_MD)};
    function kindOf(p){ if(/\\.md$/.test(p))return'md'; if(/\\.html?$/.test(p))return'html'; if(/\\.(png|jpe?g|gif|webp|bmp)$/.test(p))return'image'; if(/\\.xlsx?$/.test(p))return'xlsx'; if(/\\.pdf$/.test(p))return'pdf'; if(/\\.docx?$/.test(p))return'docx'; return'text'; }
    function invoke(cmd,args){
      switch(cmd){
        case 'get_settings': return Promise.resolve(SETTINGS);
        case 'get_effective_model_config': return Promise.resolve(EM);
        case 'list_sessions': return Promise.resolve(SESSIONS);
        case 'get_super_permission_status': return Promise.resolve(false);
        case 'list_personas': return Promise.resolve(PERSONAS);
        case 'get_backend_status': return Promise.resolve({online:true,ok:true,status:'online',model:'qwen36_35b_256k'});
        case 'check_for_update': return Promise.resolve({available:false});
        case 'find_resumable_run': return Promise.resolve(null);
        case 'check_dependencies': return Promise.resolve(DEPS);
        case 'list_marketplace_tools': return Promise.resolve([]);
        case 'get_mode_state': return Promise.resolve({mode:'yolo',plan_phase:'none'});
        case 'get_active_persona': return Promise.resolve(null);
        case 'get_workflow_state': return Promise.resolve(null);
        case 'list_workflows': return Promise.resolve([]);
        case 'list_workspace_files': return Promise.resolve([]);
        case 'get_session_persona_events': return Promise.resolve([]);
        case 'get_session_pinvou_reviews': return Promise.resolve([]);
        case 'read_persona_body': return Promise.resolve('# 角色\\n\\n你是该领域的资深专家……\\n\\n## 核心职责\\n- 以专家标准承接任务\\n\\n## 工作流程\\n1. 明确目标 2. 拆解步骤 3. 交付成果');
        case 'load_session': return Promise.resolve(CONV[args&&args.id] || {metadata:{id:(args&&args.id)||'x'},messages:[],artifacts:[]});
        case 'artifact_info': { const p=(args&&args.path)||''; return Promise.resolve({exists:true,kind:kindOf(p),size:21504,modified:1718600000}); }
        case 'read_artifact_text': { const p=(args&&args.path)||''; return Promise.resolve(/纪要/.test(p)?MEETING_MD:'（文本内容）'); }
        case 'render_artifact_visual': return Promise.resolve({mode:'unsupported'});
        case 'ingest_file': return Promise.resolve({markdown:'(已解析)',kind:'pdf'});
        default: return Promise.resolve(null);
      }
    }
    window.__TAURI__={ core:{invoke:invoke}, event:{listen:function(){return Promise.resolve(function(){});}},
      window:{ getCurrentWindow:function(){return {minimize(){},maximize(){},close(){},toggleMaximize(){},isMaximized(){return Promise.resolve(false);},onResized(){return Promise.resolve(function(){});},startDragging(){}};} },
      dialog:{ open:function(){return Promise.resolve(null);} } };
  })();`;
}

const sleep = (ms) => new Promise(r => setTimeout(r, ms));
async function clickText(page, txt) {
  return await page.evaluate((t) => {
    let els = [...document.querySelectorAll('*')].filter(el => el.children.length === 0 && (el.textContent || '').trim() === t);
    if (!els.length) els = [...document.querySelectorAll('span,div,button,a')].filter(el => (el.textContent || '').trim() === t);
    const el = els[els.length - 1];
    if (el) { el.scrollIntoView({ block: 'center' }); el.click(); return true; }
    return false;
  }, txt);
}
async function expand(page) { return page.evaluate(() => { const b = document.querySelector('[title*="侧边栏"],[title*="展开"]'); if (b) { b.click(); return true; } const x = document.querySelector('button'); if (x) { x.click(); return true; } return false; }); }
async function gear(page) { return page.evaluate(() => { const b = document.querySelector('[title="设置"],[title*="设置"]'); if (b) { b.click(); return true; } return false; }); }
async function openArtifacts(page) {
  return page.evaluate(() => {
    const btn = [...document.querySelectorAll('button')].find(b => (b.textContent || '').includes('产物与代码'));
    if (btn) { btn.click(); return true; }
    return false;
  });
}

const SHOTS = [
  { key: 'home', w: 1440, h: 920, act: async () => {} },
  { key: 'cardpool', w: 1440, h: 1120, act: async (p) => { await clickText(p, '卡牌池'); } },
  { key: 'toolStore', w: 1440, h: 1180, act: async (p) => { await clickText(p, '工具商店'); } },
  { key: 'settings', w: 1440, h: 2380, act: async (p) => { await gear(p); } },
  { key: 'conversation', w: 1440, h: 920, act: async (p) => { await clickText(p, '第三季度财报分析'); await sleep(1200); } },
  { key: 'artifacts', w: 1440, h: 920, act: async (p) => { await clickText(p, '第三季度财报分析'); await sleep(1100); await openArtifacts(p); await sleep(1100); } },
  { key: 'artifacts-preview', w: 1440, h: 920, act: async (p) => { await clickText(p, '第三季度财报分析'); await sleep(1100); await openArtifacts(p); await sleep(800); await clickText(p, '会议纪要.md'); await sleep(1000); } },
  { key: 'safety', w: 1440, h: 920, act: async (p) => { await clickText(p, '清理磁盘空间'); await sleep(1200); } },
  { key: 'attachments', w: 1440, h: 920, act: async (p) => { await p.evaluate(async () => { await window.TauriBridge.addAttachmentByPath('/home/pinvou/第三季度财报.pdf'); await window.TauriBridge.addAttachmentByPath('/home/pinvou/销售明细.xlsx'); }); await sleep(900); } },
];

(async () => {
  console.log('chromium:', CHROME);
  const browser = await puppeteer.launch({ executablePath: CHROME, headless: 'new', args: ['--no-sandbox', '--disable-gpu', '--hide-scrollbars', '--font-render-hinting=none', '--no-first-run', '--no-default-browser-check'], userDataDir: PROFILE });
  const page = await browser.newPage();
  page.on('console', m => { const t = m.text(); if (t.toLowerCase().includes('error') || t.includes('UNMOCKED')) console.log('  PAGE>', t); });
  page.on('pageerror', e => console.log('  PAGEERROR>', e.message));
  await page.evaluateOnNewDocument(injectSource());
  for (const s of SHOTS) {
    await page.setViewport({ width: s.w, height: s.h, deviceScaleFactor: 2 });
    await page.goto(INDEX, { waitUntil: 'networkidle0' });
    await page.waitForFunction(() => document.body && document.body.innerText.includes('PINVOU'), { timeout: 15000 }).catch(() => {});
    await sleep(900); await expand(page); await sleep(700);
    try { await s.act(page); } catch (e) { console.log('  act err', s.key, e.message); }
    await sleep(700);
    const out = path.join(OUT, s.key + '.png');
    await page.screenshot({ path: out });
    console.log('  saved', s.key);
  }
  await browser.close();
  console.log('DONE');
})().catch(e => { console.error('FATAL', e); process.exit(1); });
