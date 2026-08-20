#!/usr/bin/env node
/**
 * 工具商店列表视图双维度分组冒烟:加载 Vite dist + mock Tauri,点击真实 React UI。
 * 验证「按类型/按业务」segmented、二级 chips、分区 section(组名+数量)渲染与筛选联动。
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
    const TOOLS=[['weather',[]],['iwencai',[]],['qcc',[]],['patsnap-search',[]],['tencent-docs',['tencent-docs-skill']],['canva-mcp',[]],['yuandian-mcp',[]],['obsidian',[]],['pptx',[]],['gongwen',['government-writing']]];
    window.__TAURI_EVENT_HANDLERS__={};
    function invoke(cmd){
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
        case 'list_marketplace_tools': return Promise.resolve(TOOLS.map(([id,cs])=>({id,name:id,description:'',version:'1.0.0',icon:'',category:'test',installed:false,companion_skills:cs})));
        case 'get_marketplace_tool_auth_status': return Promise.resolve({status:'not_installed'});
        case 'list_marketplace_skills': return Promise.resolve([{id:'government-writing',title:'党政机关公文写作',installed:false,user_uploaded:false},{id:'visualizer',title:'数据分析可视化',installed:false,user_uploaded:false}]);
        default:
          if(/^list_|^get_/.test(cmd)) return Promise.resolve(Array.isArray([])&&cmd.startsWith('list_')?[]:null);
          return Promise.resolve(null);
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

    const chipText = await page.evaluate(() => [...document.querySelectorAll('button')].map(b => (b.textContent || '').trim()));
    for (const label of ['按类型', '按业务', '全部', 'MCP', 'Skill', 'CLI 集成', 'API & Webhook', '即将上线']) {
      rec(`类型维度 chip/segment「${label}」渲染`, chipText.includes(label));
    }

    // 主维度=类型 → 下方按业务分区
    const sectionsByType = await page.evaluate(() => [...document.querySelectorAll('h3')].map(h => (h.textContent || '').trim()));
    for (const label of ['沟通协作', '文档知识', '金融数据', '生活实用', '技能']) {
      rec(`按类型时业务分区「${label}」渲染`, sectionsByType.includes(label));
    }

    // 数量徽标 == 分区内实际渲染条目数(分区内工具卡标题同为 h3,故条目数 = h3 总数 - 1)
    const badgeSections = await page.evaluate(() => [...document.querySelectorAll('div.items-baseline')].map(head => ({
      label: (head.querySelector('h3')?.textContent || '').trim(),
      badge: (head.querySelector('span.tabular-nums')?.textContent || '').trim(),
      items: head.parentElement ? head.parentElement.querySelectorAll('h3').length - 1 : -1,
    })));
    rec('分区渲染带数量徽标', badgeSections.length > 0 && badgeSections.every(s => s.badge !== '' && /^\d+$/.test(s.badge)));
    rec('数量徽标与分区条目数一致', badgeSections.length > 0 && badgeSections.every(s => s.badge === String(s.items)),
      JSON.stringify(badgeSections));

    // 二级筛选:MCP → 只剩 MCP 条目,仍按业务分区;金融分区应为 企查查+同花顺问财 共 2 条
    rec('点击「MCP」chip', await clickExact(page, 'MCP'));
    await sleep(300);
    const mcpBadge = await page.evaluate(() => {
      const head = [...document.querySelectorAll('div.items-baseline')].find(h => (h.querySelector('h3')?.textContent || '').trim() === '金融数据');
      return head ? (head.querySelector('span.tabular-nums')?.textContent || '').trim() : null;
    });
    rec('MCP 筛选后金融分区徽标为 2', mcpBadge === '2', `实际=${mcpBadge}`);
    const mcpOnly = await page.evaluate(() => {
      const text = document.body.innerText;
      return text.includes('高德天气') && text.includes('企查查') && !text.includes('飞书（Lark）') && !text.includes('党政机关公文写作');
    });
    rec('MCP 筛选只显示 MCP 条目', mcpOnly);
    rec('MCP 筛选后仍有业务分区', await page.evaluate(() => [...document.querySelectorAll('h3')].some(h => (h.textContent || '').trim() === '金融数据')));

    // 切主维度=业务
    rec('点击「按业务」segment', await clickExact(page, '按业务'));
    await sleep(300);
    const bizChips = await page.evaluate(() => [...document.querySelectorAll('button')].map(b => (b.textContent || '').trim()));
    for (const label of ['全部', '沟通协作', '文档知识', '研发', '金融数据', '生活实用']) {
      rec(`业务维度 chip「${label}」渲染`, bizChips.includes(label));
    }
    rec('业务维度 chip 不含「技能」', !bizChips.includes('技能'));
    const sectionsByBiz = await page.evaluate(() => [...document.querySelectorAll('h3')].map(h => (h.textContent || '').trim()));
    for (const label of ['MCP', 'Skill', 'CLI 集成', 'API & Webhook', '即将上线']) {
      rec(`按业务时类型分区「${label}」渲染`, sectionsByBiz.includes(label));
    }

    // 业务筛选:金融 → 只剩金融条目,仍按类型分区
    rec('点击「金融数据」chip', await clickExact(page, '金融数据'));
    await sleep(300);
    const finOnly = await page.evaluate(() => {
      const text = document.body.innerText;
      return text.includes('企查查') && text.includes('同花顺问财') && !text.includes('飞书（Lark）') && !text.includes('高德天气');
    });
    rec('金融筛选只显示金融条目', finOnly);
    rec('金融筛选后分区为 MCP', await page.evaluate(() => {
      const h3s = [...document.querySelectorAll('h3')].map(h => (h.textContent || '').trim());
      return h3s.includes('MCP') && !h3s.includes('CLI 集成');
    }));

    // 搜索 → 平铺无分区
    await page.type('[data-testid="tool-store-search"]', '企查');
    await sleep(300);
    const searching = await page.evaluate(() => {
      const h3s = [...document.querySelectorAll('h3')].map(h => (h.textContent || '').trim());
      return document.body.innerText.includes('企查查') && !h3s.includes('MCP') && !h3s.includes('金融数据');
    });
    rec('搜索时平铺、无分区标题', searching);

    // 「我的工具」:恒为平铺,不按维度分区
    rec('点击「我的工具」', await clickExact(page, '我的工具'));
    await sleep(300);
    const myToolsFlat = await page.evaluate(() => {
      const h3s = [...document.querySelectorAll('h3')].map(h => (h.textContent || '').trim());
      return document.body.innerText.includes('我的工具') && !h3s.includes('MCP') && !h3s.includes('金融数据') && !h3s.includes('CLI 集成');
    });
    rec('我的工具平铺、无分区标题', myToolsFlat);
  } finally {
    await browser.close();
  }
  console.log(failures ? `\n❌ ${failures} FAIL` : '\n✅ ALL PASS');
  process.exit(failures ? 1 : 0);
})().catch(e => { console.error(e); process.exit(1); });
