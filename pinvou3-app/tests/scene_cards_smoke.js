#!/usr/bin/env node
/**
 * 场景入口卡片 + 设计工作台手动开关 smoke（design lane 并入 work 后的替代入口）。
 * 覆盖：工作/代码两段 HomeModeSwitcher、空态场景卡选择/取消、v3→v4 模式状态迁移、
 * 场景路由 meta 与能力自动安装（公文写作/数据可视化/海报/个人工作台模板卡）、
 * HTML 产物全屏 + artifact-edit-mode-toggle 手动进入可视化编辑（inspector dock /
 * AI 输入框 / 缩放 / changes 记录、重放与清空回滚）。
 * 依赖先运行 `npm run build:ui`。
 */
const fs = require('fs');
const os = require('os');
const path = require('path');
const { startUiTestServer } = require('./ui_test_server');

function loadPuppeteer() {
  try { return require('puppeteer-core'); } catch { /* fall through */ }
  const npx = path.join(os.homedir(), '.npm', '_npx');
  if (fs.existsSync(npx)) for (const d of fs.readdirSync(npx)) {
    const p = path.join(npx, d, 'node_modules', 'puppeteer-core');
    if (fs.existsSync(p)) try { return require(p); } catch { /* next */ }
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
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-scene-cards-'));

function injectSource() {
  return `(function(){
    var HTML_PATH = '/tmp/pinvou3/sessions/s-design/artifacts/landing.html';
    var DRAFT_PATH = '/tmp/pinvou3/sessions/s-design/artifacts/poster-draft.html';
    var HTML_CONTENT = '<!doctype html><html><body><main id="app"><section class="hero"><h1 class="hero-title">Pinvou Design</h1><button class="primary">Start</button><a id="external-link" href="https://example.com/docs">Docs</a></section></main></body></html>';
    var DRAFT_CONTENT = '<!doctype html><html><body><main id="app"><section class="hero"><h1 class="hero-title">Draft Poster</h1><button class="primary">Draft</button></section></main></body></html>';
    var SESSIONS = [{id:'s-design',title:'HTML设计测试',created_at:1,updated_at:9}];
    var MARKET_TOOLS = [
      {id:'gongwen', installed:false, companion_skills:['government-writing']}
    ];
    var MARKET_SKILLS = [
      {id:'government-writing', installed:false},
      {id:'visualizer', installed:false}
    ];
    window.__PINVOU_TEST_INSTALLS = [];
    window.__PINVOU_TEST_CHAT_CALLS = [];
    window.__PINVOU_TEST_SCENE_SAVES = [];
    window.__PINVOU_TEST_EXTERNAL_URLS = [];
    var SCENE_EVENTS = {
      's-design': [{pos:0,scene:'design:poster'}]
    };
    var CONV = { 's-design': {
      metadata:{id:'s-design',title:'HTML设计测试'},
      artifacts:[{path:DRAFT_PATH,basename:'poster-draft.html'},{path:HTML_PATH,basename:'landing.html'}],
      messages:[{role:'user',content:[{type:'text',text:'做一个 landing page'}]}]
    }};
    // 只写 v3 key：验证 v3→v4 迁移（design lane 折叠为 work + designSubtab→subtab）。
    localStorage.setItem('pinvou_mode_state_v3', JSON.stringify({
      draft:{mode:'work',workSubtab:'general',designSubtab:'poster'},
      sessions:{
        'session:s-design':{mode:'design',workSubtab:'document-writing',designSubtab:'poster'}
      },
      sessionOrder:['session:s-design']
    }));
    function invoke(cmd,args){
      switch(cmd){
        case 'get_settings': return Promise.resolve({theme:'liquid-light',language:'zh-Hans'});
        case 'get_platform_capabilities': return Promise.resolve({codexAcpSupported:true});
        case 'get_effective_model_config': return Promise.resolve({model:'qwen36_35b_256k',base_url:'http://127.0.0.1:8000/v1',api_key_set:false});
        case 'list_sessions': return Promise.resolve(SESSIONS);
        case 'create_session': {
          var id = 's-new-' + Date.now();
          var meta = {id:id,title:'新对话',created_at:Date.now(),updated_at:Date.now(),message_count:0};
          SESSIONS.unshift(meta);
          CONV[id] = {metadata:meta,artifacts:[],messages:[]};
          return Promise.resolve(meta);
        }
        case 'load_session': return Promise.resolve(CONV[args && args.id] || CONV['s-design']);
        case 'chat':
          window.__PINVOU_TEST_CHAT_CALLS.push(args || {});
          return Promise.resolve(null);
        case 'save_session_messages':
          if (args && args.id && CONV[args.id]) CONV[args.id].messages = args.messages || [];
          return Promise.resolve(null);
        case 'get_super_permission_status': return Promise.resolve(false);
        case 'list_personas': return Promise.resolve([]);
        case 'get_backend_status': return Promise.resolve({online:true,ok:true,status:'online'});
        case 'check_for_update': return Promise.resolve({available:false});
        case 'find_resumable_run': return Promise.resolve(null);
        case 'get_session_pinvou_scene_events':
          return Promise.resolve((SCENE_EVENTS[args && args.sessionId] || []).map(function(event){ return Object.assign({}, event); }));
        case 'save_session_pinvou_scene_events':
          SCENE_EVENTS[args && args.sessionId] = (args && args.events || []).map(function(event){ return Object.assign({}, event); });
          window.__PINVOU_TEST_SCENE_SAVES.push({sessionId:args && args.sessionId,events:SCENE_EVENTS[args && args.sessionId]});
          return Promise.resolve(null);
        case 'list_workspace_files': case 'get_session_persona_events': case 'get_session_pinvou_reviews': return Promise.resolve([]);
        case 'get_mode_state': return Promise.resolve({mode:'yolo',plan_phase:'none'});
        case 'get_active_persona': return Promise.resolve(null);
        case 'detect_local_vllm_setup': return Promise.resolve({eligible:false});
        case 'list_marketplace_tools': return Promise.resolve(MARKET_TOOLS.map(function(item){ return Object.assign({}, item); }));
        case 'list_marketplace_skills': return Promise.resolve(MARKET_SKILLS.map(function(item){ return Object.assign({}, item); }));
        case 'install_marketplace_tool':
          window.__PINVOU_TEST_INSTALLS.push({type:'tool', id:args && args.toolId});
          MARKET_TOOLS.forEach(function(item){ if (item.id === (args && args.toolId)) item.installed = true; });
          if ((args && args.toolId) === 'gongwen') {
            MARKET_SKILLS.forEach(function(item){ if (item.id === 'government-writing') item.installed = true; });
          }
          return Promise.resolve(null);
        case 'install_marketplace_skill':
          window.__PINVOU_TEST_INSTALLS.push({type:'skill', id:args && args.skillId});
          MARKET_SKILLS.forEach(function(item){ if (item.id === (args && args.skillId)) item.installed = true; });
          return Promise.resolve(null);
        case 'get_disabled_connectors': return Promise.resolve([]);
        case 'artifact_info': return Promise.resolve({exists:true,kind:'html',size:(args && args.path) === DRAFT_PATH ? DRAFT_CONTENT.length : HTML_CONTENT.length,modified:(args && args.path) === DRAFT_PATH ? 1 : 2});
        case 'read_artifact_text': return Promise.resolve((args && args.path) === DRAFT_PATH ? DRAFT_CONTENT : HTML_CONTENT);
        case 'render_artifact_visual': return Promise.resolve({mode:'unsupported'});
        case 'open_user_external_url':
          window.__PINVOU_TEST_EXTERNAL_URLS.push(args && args.url);
          return Promise.resolve(null);
        default: return Promise.resolve(null);
      }
    }
    window.__TAURI__={core:{invoke},event:{listen(){return Promise.resolve(function(){});},emit(){return Promise.resolve();}},
      window:{getCurrentWindow(){return {minimize(){},maximize(){},close(){},toggleMaximize(){},isMaximized(){return Promise.resolve(false);},onResized(){return Promise.resolve(function(){});},startDragging(){}};}},
      dialog:{open(){return Promise.resolve(null);}}};
  })();`;
}

const sleep = ms => new Promise(r => { setTimeout(r, ms); });

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
    const homeSwitcher = document.querySelector('[data-testid="home-mode-switcher"]');
    const duplicateSwitcher = document.querySelector('[data-testid="pinvou-mode-switcher"]');
    const textarea = document.querySelector('textarea');
    return {
      homeSwitcher: !!homeSwitcher,
      duplicateSwitcher: !!duplicateSwitcher,
      homeHasWork: !!document.querySelector('[data-testid="home-mode-work"]'),
      homeHasCode: !!document.querySelector('[data-testid="home-mode-code"]'),
      homeHasDesign: !!document.querySelector('[data-testid="home-mode-design"]'),
      homeText: homeSwitcher ? homeSwitcher.textContent : '',
      legacyPickers: ['work-subtab-picker', 'design-subtab-picker', 'personal-workbench-template-picker']
        .filter((id) => !!document.querySelector(`[data-testid="${id}"]`) || !!document.querySelector(`[data-testid^="${id}-option-"]`)),
      greeting: !!document.querySelector('[data-testid="chat-greeting"]'),
      cardGrid: !!document.querySelector('[data-testid="scene-card-grid"]'),
      cards: [...document.querySelectorAll('button[data-testid^="scene-card-"]')].map((node) =>
        node.getAttribute('data-testid')
      ),
      cardLabels: [...document.querySelectorAll('button[data-testid^="scene-card-"]')].map((node) =>
        (node.textContent || '').trim()
      ),
      templateCards: !!document.querySelector('[data-testid="personal-workbench-template-cards"]'),
      sceneTag: !!document.querySelector('[data-testid="pinvou-scene-tag"]'),
      placeholder: textarea && textarea.getAttribute('placeholder'),
    };
  });
  rec('主入口只剩工作/代码两段且空态欢迎语下方渲染四张场景卡',
    initial.homeSwitcher && !initial.duplicateSwitcher &&
      initial.homeHasWork && initial.homeHasCode && !initial.homeHasDesign &&
      initial.homeText.includes('工作') && initial.homeText.includes('代码') && !initial.homeText.includes('设计') &&
      initial.legacyPickers.length === 0 &&
      initial.greeting && initial.cardGrid &&
      JSON.stringify(initial.cards) === JSON.stringify([
        'scene-card-personal-workbench', 'scene-card-document-writing', 'scene-card-poster', 'scene-card-data-visualization',
      ]) &&
      initial.cardLabels.join('|') === '个人工作台|公文写作|海报|数据可视化' &&
      !initial.templateCards && !initial.sceneTag &&
      initial.placeholder === '询问 PINVOU 或输入指令',
    JSON.stringify(initial));

  await page.click('[data-testid="scene-card-document-writing"]');
  await sleep(250);
  const sceneSelected = await page.evaluate(() => {
    const textarea = document.querySelector('textarea');
    const v4raw = localStorage.getItem('pinvou_mode_state_v4');
    let v4;
    try { v4 = v4raw ? JSON.parse(v4raw) : null; } catch { v4 = null; }
    return {
      placeholder: textarea && textarea.getAttribute('placeholder'),
      tag: document.querySelector('[data-testid="pinvou-scene-tag"]')?.textContent || '',
      clear: !!document.querySelector('[data-testid="pinvou-scene-tag-clear"]'),
      cardPressed: document.querySelector('[data-testid="scene-card-document-writing"]')?.getAttribute('aria-pressed'),
      templateCards: !!document.querySelector('[data-testid="personal-workbench-template-cards"]'),
      v4,
    };
  });
  rec('点场景卡选中场景并在输入框上方出现可取消的场景标签',
    sceneSelected.placeholder === '描述公文主题、文种、收发单位和关键要求' &&
      sceneSelected.tag.includes('公文写作') &&
      sceneSelected.clear &&
      sceneSelected.cardPressed === 'true' &&
      !sceneSelected.templateCards,
    JSON.stringify(sceneSelected));
  rec('v3 模式状态迁移为 work + subtab 并落盘为 v4 形状',
    !!sceneSelected.v4 &&
      sceneSelected.v4.draft && sceneSelected.v4.draft.mode === 'work' && sceneSelected.v4.draft.subtab === 'document-writing' &&
      sceneSelected.v4.sessions && sceneSelected.v4.sessions['session:s-design'] &&
      sceneSelected.v4.sessions['session:s-design'].mode === 'work' &&
      sceneSelected.v4.sessions['session:s-design'].subtab === 'poster' &&
      Array.isArray(sceneSelected.v4.sessionOrder) && sceneSelected.v4.sessionOrder.includes('session:s-design'),
    JSON.stringify(sceneSelected.v4));

  await page.click('[data-testid="scene-card-document-writing"]');
  await sleep(250);
  const sceneDeselected = await page.evaluate(() => ({
    placeholder: document.querySelector('textarea')?.getAttribute('placeholder') || '',
    tag: !!document.querySelector('[data-testid="pinvou-scene-tag"]'),
    cardPressed: document.querySelector('[data-testid="scene-card-document-writing"]')?.getAttribute('aria-pressed'),
  }));
  rec('再次点击选中的场景卡取消回 general',
    !sceneDeselected.tag &&
      sceneDeselected.cardPressed === 'false' &&
      sceneDeselected.placeholder === '询问 PINVOU 或输入指令',
    JSON.stringify(sceneDeselected));

  await page.click('[data-testid="scene-card-document-writing"]');
  await sleep(250);
  await page.focus('textarea');
  await page.keyboard.type('发送后气泡标签测试');
  await page.keyboard.press('Enter');
  await sleep(900);
  const sentBubbleScene = await page.evaluate(() => {
    const chat = window.TauriBridge && window.TauriBridge.state && window.TauriBridge.state.get
      ? window.TauriBridge.state.get('chat')
      : {};
    const sidecars = [];
    for (let index = 0; index < localStorage.length; index += 1) {
      const key = localStorage.key(index);
      if (key && key.startsWith('pinvou_scene_events_v1:')) {
        sidecars.push({ key, value: JSON.parse(localStorage.getItem(key) || '[]') });
      }
    }
    return {
      tag: document.querySelector('[data-testid="user-message-scene-tag"]')?.textContent || '',
      composerTag: !!document.querySelector('[data-testid="pinvou-scene-tag"]'),
      chatCalls: window.__PINVOU_TEST_CHAT_CALLS || [],
      sidecars,
      messageHasScene: !!(chat.messages || []).some((message) =>
        Object.prototype.hasOwnProperty.call(message || {}, 'pinvouScene')
      ),
    };
  });
  rec('发送后专业场景显示为用户气泡只读标签且不污染 messages',
    sentBubbleScene.tag.includes('公文写作') &&
      !sentBubbleScene.composerTag &&
      sentBubbleScene.chatCalls.length === 1 &&
      /公文写作场景路由/.test(sentBubbleScene.chatCalls[0].message || '') &&
      sentBubbleScene.sidecars.some(entry =>
        entry.value.some(item => item.pos === 0 && item.scene === 'work:document-writing')
      ) &&
      !sentBubbleScene.messageHasScene,
    JSON.stringify(sentBubbleScene));

  await page.evaluate(() => localStorage.clear());
  await page.reload({ waitUntil: 'networkidle0' });
  await sleep(1500);

  await page.evaluate(() => {
    window.__PINVOU_TEST_SENT_MESSAGES = [];
    if (window.TauriBridge && window.TauriBridge.chat) {
      window.TauriBridge.chat.sendMessage = function(text, meta) {
        window.__PINVOU_TEST_SENT_MESSAGES.push({ text, meta });
        return Promise.resolve({ accepted: true });
      };
    }
  });
  await page.focus('textarea');
  await page.keyboard.type('普通工作问题');
  await page.keyboard.press('Enter');
  await sleep(250);
  const workGeneralPayload = await page.evaluate(() => {
    const sent = (window.__PINVOU_TEST_SENT_MESSAGES || [])[0] || null;
    return sent && {
      text: sent.text,
      scene: sent.meta && sent.meta.pinvouScene,
      requiredSkill: sent.meta && sent.meta.pinvouRequiredSkill,
      requiredTool: sent.meta && sent.meta.pinvouRequiredTool,
    };
  });
  rec('工作 general 发送不注入专业场景 meta',
    workGeneralPayload &&
      workGeneralPayload.text === '普通工作问题' &&
      !workGeneralPayload.scene &&
      !workGeneralPayload.requiredSkill &&
      !workGeneralPayload.requiredTool,
    JSON.stringify(workGeneralPayload));

  await page.evaluate(() => { window.__PINVOU_TEST_SENT_MESSAGES = []; });
  await page.click('[data-testid="scene-card-personal-workbench"]');
  await sleep(250);
  const personalWorkbenchState = await page.evaluate(() => {
    const textarea = document.querySelector('textarea');
    return {
      placeholder: textarea && textarea.getAttribute('placeholder'),
      textareaValue: textarea && textarea.value,
      sceneTag: document.querySelector('[data-testid="pinvou-scene-tag"]')?.textContent || '',
      legacyTemplatePicker: !!document.querySelector('[data-testid="personal-workbench-template-picker"]'),
      templateCards: !!document.querySelector('[data-testid="personal-workbench-template-cards"]'),
      templateLabels: [...document.querySelectorAll('button[data-testid^="personal-workbench-template-"]')]
        .map(node => (node.textContent || '').trim()),
    };
  });
  await page.focus('textarea');
  await page.keyboard.type('运动');
  await page.keyboard.press('Enter');
  await sleep(350);
  const personalSceneOnlyPayload = await page.evaluate(() => {
    const sent = (window.__PINVOU_TEST_SENT_MESSAGES || [])[0] || null;
    return sent && {
      text: sent.text,
      scene: sent.meta && sent.meta.pinvouScene,
      templateId: sent.meta && sent.meta.pinvouTemplateId,
      templateTitle: sent.meta && sent.meta.pinvouTemplateTitle,
      payload: sent.meta && sent.meta.pinvouPayloadText,
      textareaAfterSend: document.querySelector('textarea')?.value || '',
      userBubbleText: [...document.querySelectorAll('[data-testid="chat-message-user"], .chat-message-user')]
        .map(node => node.textContent || '')
        .join('\n'),
    };
  });
  rec('个人工作台场景卡展开模板卡网格，自由输入走默认专家隐藏 prompt 且界面只显示用户文本',
    personalWorkbenchState.placeholder === '选择模板后可编辑完整提示词' &&
      !personalWorkbenchState.legacyTemplatePicker &&
      personalWorkbenchState.templateCards &&
      personalWorkbenchState.templateLabels.join('|') === '生活记录|个人账本|学习计划|任务看板|求职管理|旅行计划|运动打卡' &&
      personalWorkbenchState.sceneTag.includes('个人工作台') &&
      personalWorkbenchState.textareaValue === '' &&
      personalSceneOnlyPayload &&
      personalSceneOnlyPayload.text === '运动' &&
      personalSceneOnlyPayload.scene === 'work:personal-workbench' &&
      !personalSceneOnlyPayload.templateId &&
      !personalSceneOnlyPayload.templateTitle &&
      /个人数字工作台/.test(personalSceneOnlyPayload.payload || '') &&
      /用户需求：\n运动/.test(personalSceneOnlyPayload.payload || '') &&
      !/个人数字工作台/.test(personalSceneOnlyPayload.userBubbleText || '') &&
      personalSceneOnlyPayload.textareaAfterSend === '',
    JSON.stringify({ personalWorkbenchState, personalSceneOnlyPayload }));

  await page.evaluate(() => { window.__PINVOU_TEST_SENT_MESSAGES = []; });
  await page.click('[data-testid="personal-workbench-template-1"]');
  await sleep(200);
  const longCustomWorkbenchPrompt = '我要做一个适合自由职业者使用的客户项目管理工作台，需要能管理客户、合同、收款节点、待办事项、沟通记录、项目风险、交付物清单和月度复盘，还要支持移动端查看，数据全部存在本地，并且界面需要像现代 iOS 工具应用一样清爽。';
  await page.evaluate((prompt) => {
    const textarea = document.querySelector('textarea');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
    setter.call(textarea, prompt);
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
  }, longCustomWorkbenchPrompt);
  await page.keyboard.press('Enter');
  await sleep(350);
  const personalTemplateReplacedPayload = await page.evaluate(() => {
    const sent = (window.__PINVOU_TEST_SENT_MESSAGES || [])[0] || null;
    return sent && {
      text: sent.text,
      scene: sent.meta && sent.meta.pinvouScene,
      templateId: sent.meta && sent.meta.pinvouTemplateId,
      templateTitle: sent.meta && sent.meta.pinvouTemplateTitle,
      payload: sent.meta && sent.meta.pinvouPayloadText,
      textareaAfterSend: document.querySelector('textarea')?.value || '',
    };
  });
  rec('个人工作台模板被整段替换后按自由输入处理',
    personalTemplateReplacedPayload &&
      personalTemplateReplacedPayload.text === longCustomWorkbenchPrompt &&
      personalTemplateReplacedPayload.scene === 'work:personal-workbench' &&
      !personalTemplateReplacedPayload.templateId &&
      !personalTemplateReplacedPayload.templateTitle &&
      /个人数字工作台/.test(personalTemplateReplacedPayload.payload || '') &&
      personalTemplateReplacedPayload.payload.includes(`用户需求：\n${longCustomWorkbenchPrompt}`) &&
      personalTemplateReplacedPayload.textareaAfterSend === '',
    JSON.stringify(personalTemplateReplacedPayload));

  await page.evaluate(() => { window.__PINVOU_TEST_SENT_MESSAGES = []; });
  await page.click('[data-testid="personal-workbench-template-1"]');
  await sleep(200);
  const personalTemplateSelected = await page.evaluate(() => {
    const textarea = document.querySelector('textarea');
    return {
      textareaValue: textarea && textarea.value,
      sceneTag: document.querySelector('[data-testid="pinvou-scene-tag"]')?.textContent || '',
      templateCardPressed: document.querySelector('[data-testid="personal-workbench-template-1"]')?.getAttribute('aria-pressed'),
      hasPromptInTextarea: /真实时薪计算器|localStorage/.test(textarea && textarea.value || ''),
    };
  });
  await page.click('[data-testid="scene-card-document-writing"]');
  await sleep(250);
  const templateDraftClearedOnSceneSwitch = await page.evaluate(() => {
    const textarea = document.querySelector('textarea');
    return {
      textareaValue: textarea && textarea.value,
      placeholder: textarea && textarea.getAttribute('placeholder'),
      sceneTag: document.querySelector('[data-testid="pinvou-scene-tag"]')?.textContent || '',
      templateCards: !!document.querySelector('[data-testid="personal-workbench-template-cards"]'),
      hasPersonalTemplatePrompt: /真实时薪计算器|个人账本|视觉要求/.test(textarea && textarea.value || ''),
    };
  });
  rec('从个人工作台模板切到其他场景会清理模板草稿',
    personalTemplateSelected.sceneTag.includes('个人工作台') &&
      personalTemplateSelected.templateCardPressed === 'true' &&
      personalTemplateSelected.hasPromptInTextarea &&
      templateDraftClearedOnSceneSwitch.sceneTag.includes('公文写作') &&
      templateDraftClearedOnSceneSwitch.placeholder === '描述公文主题、文种、收发单位和关键要求' &&
      templateDraftClearedOnSceneSwitch.textareaValue === '' &&
      !templateDraftClearedOnSceneSwitch.templateCards &&
      !templateDraftClearedOnSceneSwitch.hasPersonalTemplatePrompt,
    JSON.stringify({ personalTemplateSelected, templateDraftClearedOnSceneSwitch }));

  await page.click('[data-testid="scene-card-personal-workbench"]');
  await sleep(200);
  await page.evaluate((prompt) => {
    const textarea = document.querySelector('textarea');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
    setter.call(textarea, prompt);
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
  }, personalTemplateSelected.textareaValue);
  await page.click('[data-testid="scene-card-document-writing"]');
  await sleep(250);
  const restoredTemplateDraftClearedOnSceneSwitch = await page.evaluate(() => {
    const textarea = document.querySelector('textarea');
    return {
      textareaValue: textarea && textarea.value,
      placeholder: textarea && textarea.getAttribute('placeholder'),
      sceneTag: document.querySelector('[data-testid="pinvou-scene-tag"]')?.textContent || '',
      hasPersonalTemplatePrompt: /真实时薪计算器|个人账本|视觉要求/.test(textarea && textarea.value || ''),
    };
  });
  rec('恢复的个人工作台模板草稿即使没有选中卡片也会在切换场景时清理',
    restoredTemplateDraftClearedOnSceneSwitch.sceneTag.includes('公文写作') &&
      restoredTemplateDraftClearedOnSceneSwitch.placeholder === '描述公文主题、文种、收发单位和关键要求' &&
      restoredTemplateDraftClearedOnSceneSwitch.textareaValue === '' &&
      !restoredTemplateDraftClearedOnSceneSwitch.hasPersonalTemplatePrompt,
    JSON.stringify(restoredTemplateDraftClearedOnSceneSwitch));

  await page.click('[data-testid="scene-card-personal-workbench"]');
  await sleep(200);
  await page.click('[data-testid="personal-workbench-template-1"]');
  await sleep(200);
  await page.evaluate(() => {
    const textarea = document.querySelector('textarea');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
    setter.call(textarea, `${textarea.value}\n\n用户补充需求：暗色模式`);
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await page.keyboard.press('Enter');
  await sleep(350);
  const personalPayload = await page.evaluate(() => {
    const sent = (window.__PINVOU_TEST_SENT_MESSAGES || [])[0] || null;
    return sent && {
      text: sent.text,
      scene: sent.meta && sent.meta.pinvouScene,
      templateId: sent.meta && sent.meta.pinvouTemplateId,
      templateTitle: sent.meta && sent.meta.pinvouTemplateTitle,
      payload: sent.meta && sent.meta.pinvouPayloadText,
      textareaAfterSend: document.querySelector('textarea')?.value || '',
    };
  });
  rec('个人工作台模板卡把完整 prompt 写入输入框并直接发送当前文本',
    personalTemplateSelected.sceneTag.includes('个人工作台') &&
      personalTemplateSelected.templateCardPressed === 'true' &&
      personalTemplateSelected.hasPromptInTextarea &&
      personalPayload &&
      /请制作一个名为「个人账本」/.test(personalPayload.text || '') &&
      /用户补充需求：暗色模式/.test(personalPayload.text || '') &&
      personalPayload.scene === 'work:personal-workbench' &&
      personalPayload.templateId === 'personal-ledger' &&
      personalPayload.templateTitle === '个人账本' &&
      !personalPayload.payload &&
      personalPayload.textareaAfterSend === '',
    JSON.stringify({ personalTemplateSelected, personalPayload }));

  await page.evaluate(() => { window.__PINVOU_TEST_SENT_MESSAGES = []; });
  await page.click('[data-testid="scene-card-document-writing"]');
  await sleep(250);
  const selectedWorkScene = await page.evaluate(() => {
    const textarea = document.querySelector('textarea');
    return {
      placeholder: textarea && textarea.getAttribute('placeholder'),
      tag: document.querySelector('[data-testid="pinvou-scene-tag"]')?.textContent || '',
      clear: !!document.querySelector('[data-testid="pinvou-scene-tag-clear"]'),
    };
  });
  await page.focus('textarea');
  await page.keyboard.type('写一份项目验收通知');
  await page.click('[data-testid="pinvou-scene-tag-clear"]');
  await sleep(250);
  const clearedWorkScene = await page.evaluate(() => ({
    tag: !!document.querySelector('[data-testid="pinvou-scene-tag"]'),
    text: document.querySelector('textarea')?.value || '',
  }));
  rec('场景以标签显示并可取消且保留草稿',
    selectedWorkScene.placeholder === '描述公文主题、文种、收发单位和关键要求' &&
      selectedWorkScene.tag.includes('公文写作') &&
      selectedWorkScene.clear &&
      !clearedWorkScene.tag &&
      clearedWorkScene.text === '写一份项目验收通知',
    JSON.stringify({ selectedWorkScene, clearedWorkScene }));

  await page.click('[data-testid="scene-card-document-writing"]');
  await sleep(250);
  await page.focus('textarea');
  await page.keyboard.press('Enter');
  await sleep(700);
  const documentPayload = await page.evaluate(() => {
    const sent = (window.__PINVOU_TEST_SENT_MESSAGES || [])[0] || null;
    return sent && {
      text: sent.text,
      scene: sent.meta && sent.meta.pinvouScene,
      requiredSkill: sent.meta && sent.meta.pinvouRequiredSkill,
      requiredTool: sent.meta && sent.meta.pinvouRequiredTool,
      payload: sent.meta && sent.meta.pinvouPayloadText,
      installs: window.__PINVOU_TEST_INSTALLS || [],
      status: document.querySelector('[data-testid="scene-capability-status"]')?.textContent || '',
    };
  });
  rec('公文写作场景自动准备并强制路由到 government-writing + gongwen',
    documentPayload &&
      documentPayload.text === '写一份项目验收通知' &&
      documentPayload.scene === 'work:document-writing' &&
      documentPayload.requiredSkill === 'government-writing' &&
      documentPayload.requiredTool === 'gongwen' &&
      documentPayload.installs.some(item => item.type === 'tool' && item.id === 'gongwen') &&
      /公文写作场景路由/.test(documentPayload.payload || '') &&
      /government-writing/.test(documentPayload.payload || '') &&
      /gongwen/.test(documentPayload.payload || ''),
    JSON.stringify({ selectedWorkScene, documentPayload }));

  await page.evaluate(() => { window.__PINVOU_TEST_SENT_MESSAGES = []; });
  await page.click('[data-testid="scene-card-data-visualization"]');
  await sleep(250);
  const designDataPlaceholder = await page.evaluate(() => {
    const textarea = document.querySelector('textarea');
    return {
      placeholder: textarea && textarea.getAttribute('placeholder'),
      cardPressed: document.querySelector('[data-testid="scene-card-data-visualization"]')?.getAttribute('aria-pressed'),
      tag: document.querySelector('[data-testid="pinvou-scene-tag"]')?.textContent || '',
    };
  });
  await page.focus('textarea');
  await page.keyboard.type('把近 7 天销售额做成趋势图');
  await page.keyboard.press('Enter');
  await sleep(700);
  const dataPayload = await page.evaluate(() => {
    const sent = (window.__PINVOU_TEST_SENT_MESSAGES || [])[0] || null;
    return sent && {
      text: sent.text,
      scene: sent.meta && sent.meta.pinvouScene,
      requiredSkill: sent.meta && sent.meta.pinvouRequiredSkill,
      requiredTool: sent.meta && sent.meta.pinvouRequiredTool,
      payload: sent.meta && sent.meta.pinvouPayloadText,
      installs: window.__PINVOU_TEST_INSTALLS || [],
    };
  });
  rec('数据可视化场景自动准备并强制路由到 visualizer',
      designDataPlaceholder.placeholder === '粘贴数据或描述指标，生成可视化看板' &&
      designDataPlaceholder.cardPressed === 'true' &&
      designDataPlaceholder.tag.includes('数据可视化') &&
      dataPayload &&
      dataPayload.text === '把近 7 天销售额做成趋势图' &&
      dataPayload.scene === 'design:data-visualization' &&
      dataPayload.requiredSkill === 'visualizer' &&
      !dataPayload.requiredTool &&
      dataPayload.installs.some(item => item.type === 'skill' && item.id === 'visualizer') &&
      /数据可视化场景路由/.test(dataPayload.payload || '') &&
      /visualizer/.test(dataPayload.payload || '') &&
      /Chart\.js/.test(dataPayload.payload || '') &&
      !/Excel 仪表盘/.test((dataPayload.payload || '').split('---')[0] || ''),
    JSON.stringify({ designDataPlaceholder, dataPayload }));

  await page.evaluate(() => window.TauriBridge && window.TauriBridge.sessions && window.TauriBridge.sessions.switchToSession('s-design'));
  await sleep(900);
  await clickExactButton(page, '产物与代码');
  await page.waitForSelector('[data-testid="artifact-html-preview-frame"]', { timeout: 5000 });
  const directPreview = await page.evaluate(() => {
    const switcher = document.querySelector('[data-testid="artifact-switcher-button"]');
    const frame = document.querySelector('[data-testid="artifact-html-preview-frame"]');
    const fullscreenToggle = document.querySelector('[data-testid="artifact-fullscreen-toggle"]');
    return {
      switcher: !!switcher,
      switcherText: switcher && switcher.textContent,
      frame: !!frame,
      fullscreenPanel: !!document.querySelector('[data-testid="artifact-fullscreen-panel"]'),
      editToggle: !!document.querySelector('[data-testid="artifact-edit-mode-toggle"]'),
      toggleTitle: fullscreenToggle?.getAttribute('title') || '',
      toggleLabel: fullscreenToggle?.getAttribute('aria-label') || '',
      toggleText: (fullscreenToggle?.textContent || '').trim(),
    };
  });
  rec('点击产物与代码直接预览最新产物，全屏按钮回归纯图标且非全屏无编辑模式入口',
    directPreview.switcher && directPreview.frame &&
      /landing\.html/.test(directPreview.switcherText || '') &&
      !directPreview.fullscreenPanel &&
      !directPreview.editToggle &&
      directPreview.toggleTitle === '全屏显示' &&
      directPreview.toggleLabel === '全屏显示产物预览' &&
      directPreview.toggleText === '',
    JSON.stringify(directPreview));
  await page.evaluate(() => {
    const frame = document.querySelector('[data-testid="artifact-html-preview-frame"]');
    const link = frame && frame.contentDocument && frame.contentDocument.querySelector('#external-link');
    if (link) link.click();
  });
  await sleep(100);
  const externalPreviewLinks = await page.evaluate(() => window.__PINVOU_TEST_EXTERNAL_URLS || []);
  rec('产物 HTML 外链由宿主交给系统浏览器命令且 iframe 不跳转',
    externalPreviewLinks.length === 1
      && externalPreviewLinks[0] === 'https://example.com/docs',
    JSON.stringify(externalPreviewLinks));
  await page.click('[data-testid="artifact-switcher-button"]');
  await sleep(150);
  const artifactMenu = await page.evaluate(() => ({
    exists: !!document.querySelector('[data-testid="artifact-switcher-menu"]'),
    items: [...document.querySelectorAll('[data-testid="artifact-switcher-item"]')].map((node) => node.textContent || ''),
  }));
  rec('预览页顶部提供 iOS 风格产物切换菜单',
    artifactMenu.exists && artifactMenu.items.length === 2 &&
      artifactMenu.items.some((text) => /poster-draft\.html/.test(text)) &&
      artifactMenu.items.some((text) => /landing\.html/.test(text)),
    JSON.stringify(artifactMenu));
  await page.evaluate(() => {
    const item = [...document.querySelectorAll('[data-testid="artifact-switcher-item"]')]
      .find((node) => /poster-draft\.html/.test(node.textContent || ''));
    if (item) item.click();
  });
  await sleep(500);
  const switchedDraft = await page.evaluate(() => {
    const switcher = document.querySelector('[data-testid="artifact-switcher-button"]');
    return switcher && switcher.textContent || '';
  });
  rec('产物切换菜单可切换到其他产物',
    /poster-draft\.html/.test(switchedDraft),
    JSON.stringify({ switchedDraft }));
  await page.click('[data-testid="artifact-switcher-button"]');
  await sleep(100);
  await page.evaluate(() => {
    const item = [...document.querySelectorAll('[data-testid="artifact-switcher-item"]')]
      .find((node) => /landing\.html/.test(node.textContent || ''));
    if (item) item.click();
  });
  await sleep(500);

  await sleep(700);
  const design = await page.evaluate(() => {
    const textarea = document.querySelector('textarea');
    const homeSwitcher = document.querySelector('[data-testid="home-mode-switcher"]');
    const cardGrid = document.querySelector('[data-testid="scene-card-grid"]');
    const sceneTag = document.querySelector('[data-testid="pinvou-scene-tag"]');
    const userSceneTag = document.querySelector('[data-testid="user-message-scene-tag"]');
    return {
      statusHidden: !document.querySelector('[data-testid="design-mode-status"]'),
      placeholder: textarea && textarea.getAttribute('placeholder'),
      homeSwitcher: !!homeSwitcher,
      cardGrid: !!cardGrid,
      sceneTag: sceneTag && sceneTag.textContent,
      userSceneTag: userSceneTag && userSceneTag.textContent,
    };
  });
  rec('非空会话从共享 sidecar 恢复历史消息只读标签，场景从迁移后的模式状态恢复为海报',
    design.statusHidden &&
      !design.homeSwitcher &&
      !design.cardGrid &&
      !design.sceneTag &&
      /海报/.test(design.userSceneTag || '') &&
      design.placeholder === '描述你想生成或调整的视觉海报',
    JSON.stringify(design));

  await page.evaluate(() => {
    window.__PINVOU_TEST_SENT_MESSAGES = [];
    if (window.TauriBridge && window.TauriBridge.chat) {
      window.TauriBridge.chat.sendMessage = function(text, meta) {
        window.__PINVOU_TEST_SENT_MESSAGES.push({ text, meta });
        return Promise.resolve({ accepted: true });
      };
    }
  });
  await page.focus('textarea');
  await page.keyboard.type('设计一张科技峰会海报');
  await page.keyboard.press('Enter');
  await sleep(250);
  const posterPayload = await page.evaluate(() => {
    const sent = (window.__PINVOU_TEST_SENT_MESSAGES || [])[0] || null;
    return sent && {
      text: sent.text,
      pinvouScene: sent.meta && sent.meta.pinvouScene,
      scene: sent.meta && sent.meta.pinvouDesignScene,
      payload: sent.meta && sent.meta.pinvouPayloadText,
    };
  });
  rec('视觉海报发送时模型 payload 注入海报约束和自检要求',
      posterPayload &&
      posterPayload.text === '设计一张科技峰会海报' &&
      posterPayload.pinvouScene === 'design:poster' &&
      posterPayload.scene === 'poster' &&
      /视觉海报场景约束/.test(posterPayload.payload || '') &&
      /真实图片/.test(posterPayload.payload || '') &&
      /真实质感主视觉/.test(posterPayload.payload || '') &&
      /联网/.test(posterPayload.payload || '') &&
      /下载/.test(posterPayload.payload || '') &&
      /海报自检/.test(posterPayload.payload || ''),
    JSON.stringify(posterPayload));

  // 进入全屏：出现编辑模式手动开关，但默认不进入编辑模式
  await page.click('[data-testid="artifact-fullscreen-toggle"]');
  await sleep(400);
  const fullscreenIdle = await page.evaluate(() => {
    const editToggle = document.querySelector('[data-testid="artifact-edit-mode-toggle"]');
    const fullscreenToggle = document.querySelector('[data-testid="artifact-fullscreen-toggle"]');
    return {
      panel: !!document.querySelector('[data-testid="artifact-fullscreen-panel"]'),
      editToggle: !!editToggle,
      editText: (editToggle?.textContent || '').trim(),
      editTitle: editToggle?.getAttribute('title') || '',
      editLabel: editToggle?.getAttribute('aria-label') || '',
      editPressed: editToggle?.getAttribute('aria-pressed'),
      toggleTitle: fullscreenToggle?.getAttribute('title') || '',
      toggleLabel: fullscreenToggle?.getAttribute('aria-label') || '',
      inspector: !!document.querySelector('[data-testid="artifact-design-inspector-host"]'),
      composer: !!document.querySelector('[data-testid="artifact-design-ai-composer"]'),
      zoomControls: !!document.querySelector('[data-testid="artifact-html-zoom-controls"]'),
      footer: !!document.querySelector('[data-testid="artifact-meta-footer"]'),
    };
  });
  rec('全屏后出现编辑模式手动开关且默认不进入编辑模式',
    fullscreenIdle.panel && fullscreenIdle.editToggle &&
      fullscreenIdle.editText === '编辑模式' &&
      fullscreenIdle.editTitle === '进入编辑模式：放大预览并编辑选中元素' &&
      fullscreenIdle.editLabel === '进入编辑模式：放大预览并编辑选中元素' &&
      fullscreenIdle.editPressed === 'false' &&
      fullscreenIdle.toggleTitle === '退出全屏' &&
      fullscreenIdle.toggleLabel === '退出全屏并回到右侧预览' &&
      !fullscreenIdle.inspector && !fullscreenIdle.composer && !fullscreenIdle.zoomControls &&
      !fullscreenIdle.footer,
    JSON.stringify(fullscreenIdle));

  // 点击编辑模式开关：注入 design runtime，渲染 inspector dock / AI 输入框 / 缩放控件
  await page.click('[data-testid="artifact-edit-mode-toggle"]');
  await sleep(450);

  const inspectorPlacement = await page.evaluate(() => {
    const composer = document.querySelector('[data-testid="chat-composer-wrap"]');
    const host = document.querySelector('[data-testid="artifact-design-inspector-host"]');
    const panel = document.querySelector('[data-testid="design-inspector-panel"]');
    const preview = document.querySelector('[data-testid="artifact-preview-content"]');
    const hostStyle = host ? getComputedStyle(host) : null;
    const panelStyle = panel ? getComputedStyle(panel) : null;
    return {
      host: !!host,
      panel: !!panel,
      preview: !!preview,
      panelInComposer: !!(composer && panel && composer.contains(panel)),
      panelInArtifactHost: !!(host && panel && host.contains(panel)),
      hostPosition: hostStyle && hostStyle.position,
      panelBackground: panelStyle && panelStyle.backgroundColor,
      text: panel && panel.textContent,
    };
  });
  rec('设计 inspector 作为实底 dock 渲染在产物预览区',
    inspectorPlacement.host && inspectorPlacement.panel && inspectorPlacement.preview &&
      !inspectorPlacement.panelInComposer && inspectorPlacement.panelInArtifactHost &&
      inspectorPlacement.hostPosition !== 'absolute' &&
      /245,\s*245,\s*247|255,\s*255,\s*255/.test(inspectorPlacement.panelBackground || ''),
    JSON.stringify(inspectorPlacement));

  const fullscreen = await page.evaluate(() => {
    const panel = document.querySelector('[data-testid="artifact-fullscreen-panel"]');
    const editToggle = document.querySelector('[data-testid="artifact-edit-mode-toggle"]');
    const toggle = document.querySelector('[data-testid="artifact-fullscreen-toggle"]');
    const preview = document.querySelector('[data-testid="artifact-preview-content"]');
    const inspector = document.querySelector('[data-testid="artifact-design-inspector-host"]');
    const zoomControls = document.querySelector('[data-testid="artifact-html-zoom-controls"]');
    const fitButton = document.querySelector('[data-testid="artifact-html-zoom-fit"]');
    const previewRect = preview && preview.getBoundingClientRect();
    const inspectorRect = inspector && inspector.getBoundingClientRect();
    const panelRect = panel && panel.getBoundingClientRect();
    const frame = document.querySelector('[data-testid="artifact-html-preview-frame"]');
    return {
      panel: !!panel,
      panelTop: panelRect && Math.round(panelRect.top),
      panelLeft: panelRect && Math.round(panelRect.left),
      panelWidth: panelRect && Math.round(panelRect.width),
      panelHeight: panelRect && Math.round(panelRect.height),
      windowWidth: window.innerWidth,
      windowHeight: window.innerHeight,
      editText: (editToggle?.textContent || '').trim(),
      editTitle: editToggle?.getAttribute('title') || '',
      editPressed: editToggle?.getAttribute('aria-pressed'),
      toggleTitle: toggle && toggle.getAttribute('title'),
      inspector: !!inspector,
      footer: !!document.querySelector('[data-testid="artifact-meta-footer"]'),
      inspectorWidth: inspectorRect && Math.round(inspectorRect.width),
      inspectorRightOfPreview: !!(previewRect && inspectorRect && inspectorRect.left >= previewRect.right - 1),
      zoomControls: !!zoomControls,
      zoomText: zoomControls && zoomControls.textContent,
      fitActive: !!(fitButton && /bg-/.test(fitButton.className || '')),
      scale: frame && Number(frame.dataset.zoomScale || 0),
    };
  });
  rec('产物预览支持编辑模式全屏显示',
    fullscreen.panel && fullscreen.inspector &&
      fullscreen.editText === '退出编辑' &&
      fullscreen.editTitle === '退出编辑模式并回到右侧预览' &&
      fullscreen.editPressed === 'true' &&
      fullscreen.toggleTitle === '退出全屏' &&
      fullscreen.panelTop === 36 && fullscreen.panelLeft === 0 &&
      fullscreen.panelWidth === fullscreen.windowWidth &&
      fullscreen.panelHeight === fullscreen.windowHeight - 36,
    JSON.stringify(fullscreen));
  rec('全屏设计模式隐藏文件详情并固定右侧 inspector',
    !fullscreen.footer && fullscreen.inspectorRightOfPreview && fullscreen.inspectorWidth >= 300 && fullscreen.inspectorWidth <= 340,
    JSON.stringify(fullscreen));
  rec('全屏设计模式默认提供完整显示缩放',
    fullscreen.zoomControls && fullscreen.zoomText.includes('适应窗口') && !fullscreen.zoomText.includes('适应宽度') && !fullscreen.zoomText.includes('原始大小') &&
      fullscreen.zoomText.includes('-') && fullscreen.zoomText.includes('+'),
    JSON.stringify(fullscreen));

  const simplifiedInspector = await page.evaluate(() => {
    const panel = document.querySelector('[data-testid="design-inspector-panel"]');
    return {
      text: panel && panel.textContent,
      advanced: !!document.querySelector('[data-testid="design-advanced-content"]'),
      aiComposer: !!document.querySelector('[data-testid="artifact-design-ai-composer"]'),
      aiPlaceholder: document.querySelector('[data-testid="artifact-design-ai-input"]')?.getAttribute('placeholder') || '',
    };
  });
  rec('全屏设计预览提供 AI 输入框',
    simplifiedInspector.aiComposer &&
      simplifiedInspector.aiPlaceholder === '描述你想怎么调整这张设计' &&
      !simplifiedInspector.advanced,
    JSON.stringify(simplifiedInspector));

  await page.click('[data-testid="artifact-design-ai-input"]');
  await page.keyboard.type('把整体氛围调得更醒目');
  await page.click('[data-testid="artifact-design-ai-send"]');
  await sleep(120);
  const aiStatusBeforeFullscreenToggle = await page.evaluate(() => ({
    status: document.querySelector('[data-testid="artifact-design-ai-status"]')?.textContent || '',
    composer: !!document.querySelector('[data-testid="artifact-design-ai-composer"]'),
  }));
  // 退出全屏：全屏面板实例卸载，编辑模式随之结束，编辑模式入口消失
  await page.click('[data-testid="artifact-fullscreen-toggle"]');
  await sleep(250);
  const exitedFullscreen = await page.evaluate(() => ({
    fullscreenPanel: !!document.querySelector('[data-testid="artifact-fullscreen-panel"]'),
    composer: !!document.querySelector('[data-testid="artifact-design-ai-composer"]'),
    editToggle: !!document.querySelector('[data-testid="artifact-edit-mode-toggle"]'),
    toggleTitle: document.querySelector('[data-testid="artifact-fullscreen-toggle"]')?.getAttribute('title') || '',
  }));
  // 重新进入全屏：默认不进入编辑模式，需要再次手动打开
  await page.click('[data-testid="artifact-fullscreen-toggle"]');
  await sleep(450);
  const reenteredFullscreen = await page.evaluate(() => ({
    fullscreenPanel: !!document.querySelector('[data-testid="artifact-fullscreen-panel"]'),
    composer: !!document.querySelector('[data-testid="artifact-design-ai-composer"]'),
    editText: (document.querySelector('[data-testid="artifact-edit-mode-toggle"]')?.textContent || '').trim(),
    editPressed: document.querySelector('[data-testid="artifact-edit-mode-toggle"]')?.getAttribute('aria-pressed'),
  }));
  await page.click('[data-testid="artifact-edit-mode-toggle"]');
  await sleep(450);
  const aiStatusAfterFullscreenToggle = await page.evaluate(() => ({
    status: document.querySelector('[data-testid="artifact-design-ai-status"]')?.textContent || '',
    composer: !!document.querySelector('[data-testid="artifact-design-ai-composer"]'),
  }));
  rec('退出全屏自动退出编辑模式，重新打开后全屏 AI 状态不丢失',
    aiStatusBeforeFullscreenToggle.composer &&
      /调整中/.test(aiStatusBeforeFullscreenToggle.status) &&
      /把整体氛围调得更醒目/.test(aiStatusBeforeFullscreenToggle.status) &&
      !exitedFullscreen.fullscreenPanel && !exitedFullscreen.composer && !exitedFullscreen.editToggle &&
      exitedFullscreen.toggleTitle === '全屏显示' &&
      reenteredFullscreen.fullscreenPanel && !reenteredFullscreen.composer &&
      reenteredFullscreen.editText === '编辑模式' && reenteredFullscreen.editPressed === 'false' &&
      aiStatusAfterFullscreenToggle.composer &&
      /调整中/.test(aiStatusAfterFullscreenToggle.status) &&
      /把整体氛围调得更醒目/.test(aiStatusAfterFullscreenToggle.status),
    JSON.stringify({ aiStatusBeforeFullscreenToggle, exitedFullscreen, reenteredFullscreen, aiStatusAfterFullscreenToggle }));
  const aiComposerCompact = await page.evaluate(() => {
    const composer = document.querySelector('[data-testid="artifact-design-ai-composer"]');
    const rect = composer && composer.getBoundingClientRect();
    return {
      height: rect && Math.round(rect.height),
      status: document.querySelector('[data-testid="artifact-design-ai-status"]')?.textContent || '',
    };
  });
  rec('全屏 AI 执行态保持紧凑输入框形态',
    aiComposerCompact.height > 0 && aiComposerCompact.height <= 68 &&
      /调整中/.test(aiComposerCompact.status) &&
      /把整体氛围调得更醒目/.test(aiComposerCompact.status),
    JSON.stringify(aiComposerCompact));
  await page.click('[data-testid="artifact-design-ai-stop"]');
  await page.waitForSelector('[data-testid="artifact-design-ai-input"]', { timeout: 3000 });

  const fitCenter = await page.evaluate(() => {
    const preview = document.querySelector('[data-testid="artifact-preview-content"]');
    const scroll = document.querySelector('[data-testid="artifact-html-preview-scroll"]');
    const frame = document.querySelector('[data-testid="artifact-html-preview-frame"]');
    if (!preview || !frame) return null;
    const previewRect = preview.getBoundingClientRect();
    const frameRect = frame.getBoundingClientRect();
    const scrollStyle = scroll && getComputedStyle(scroll);
    return {
      topGap: Math.round(frameRect.top - previewRect.top),
      bottomGap: Math.round(previewRect.bottom - frameRect.bottom),
      frameHeight: Math.round(frameRect.height),
      previewHeight: Math.round(previewRect.height),
      overflow: scrollStyle && scrollStyle.overflow,
    };
  });
  rec('完整显示模式下画布垂直居中，避免底部大片留黑',
    fitCenter && fitCenter.frameHeight > 0 && fitCenter.previewHeight > fitCenter.frameHeight &&
      Math.abs(fitCenter.topGap - fitCenter.bottomGap) < Math.max(60, fitCenter.previewHeight * 0.12) &&
      fitCenter.overflow === 'hidden',
    JSON.stringify(fitCenter));

  await page.click('[data-testid="artifact-html-zoom-mode"]');
  await sleep(100);
  await page.click('[data-testid="artifact-html-zoom-actual"]');
  await sleep(250);
  const actualScale = await page.evaluate(() => Number(document.querySelector('[data-testid="artifact-html-preview-frame"]')?.dataset.zoomScale || 0));
  const actualOverflow = await page.evaluate(() => {
    const scroll = document.querySelector('[data-testid="artifact-html-preview-scroll"]');
    return scroll && getComputedStyle(scroll).overflow;
  });
  await page.click('[data-testid="artifact-html-zoom-in"]');
  await sleep(250);
  const customZoomIn = await page.evaluate(() => {
    const frame = document.querySelector('[data-testid="artifact-html-preview-frame"]');
    const scale = document.querySelector('[data-testid="artifact-html-zoom-scale"]');
    return {
      mode: frame && frame.dataset.zoomMode,
      scale: frame && Number(frame.dataset.zoomScale || 0),
      label: scale && scale.textContent,
    };
  });
  await page.click('[data-testid="artifact-html-zoom-out"]');
  await sleep(250);
  const customZoomOut = await page.evaluate(() => {
    const frame = document.querySelector('[data-testid="artifact-html-preview-frame"]');
    return {
      mode: frame && frame.dataset.zoomMode,
      scale: frame && Number(frame.dataset.zoomScale || 0),
    };
  });
  await page.click('[data-testid="artifact-html-zoom-mode"]');
  await sleep(100);
  await page.click('[data-testid="artifact-html-zoom-fit"]');
  await sleep(250);
  const fitAgainScale = await page.evaluate(() => Number(document.querySelector('[data-testid="artifact-html-preview-frame"]')?.dataset.zoomScale || 0));
  await page.$eval('[data-testid="artifact-html-preview-scroll"]', (node) => {
    node.dispatchEvent(new WheelEvent('wheel', { deltaY: -120, ctrlKey: true, bubbles: true, cancelable: true }));
  });
  await sleep(250);
  const ctrlWheelZoom = await page.evaluate(() => {
    const frame = document.querySelector('[data-testid="artifact-html-preview-frame"]');
    return {
      mode: frame && frame.dataset.zoomMode,
      scale: frame && Number(frame.dataset.zoomScale || 0),
    };
  });
  await page.click('[data-testid="artifact-html-zoom-mode"]');
  await sleep(100);
  await page.click('[data-testid="artifact-html-zoom-fit"]');
  await sleep(250);
  await page.evaluate(() => {
    const frame = document.querySelector('[data-testid="artifact-html-preview-frame"]');
    const win = frame && frame.contentWindow;
    const doc = win && win.document;
    if (doc && win) {
      doc.dispatchEvent(new win.WheelEvent('wheel', { deltaY: -120, ctrlKey: true, bubbles: true, cancelable: true }));
    }
  });
  await sleep(250);
  const iframeCtrlWheelZoom = await page.evaluate(() => {
    const frame = document.querySelector('[data-testid="artifact-html-preview-frame"]');
    return {
      mode: frame && frame.dataset.zoomMode,
      scale: frame && Number(frame.dataset.zoomScale || 0),
    };
  });
  await page.click('[data-testid="artifact-html-zoom-mode"]');
  await sleep(100);
  await page.click('[data-testid="artifact-html-zoom-fit"]');
  await sleep(250);
  rec('全屏缩放按钮会改变固定画布比例',
    fullscreen.scale > 0 && actualScale === 1 && actualOverflow === 'auto' && Math.abs(actualScale - fullscreen.scale) > 0.01,
    JSON.stringify({ fit: fullscreen.scale, actual: actualScale, actualOverflow }));
  rec('全屏缩放支持加减自由缩放并可回到完整显示',
    customZoomIn.mode === 'custom' && customZoomIn.scale > actualScale &&
      customZoomOut.mode === 'custom' && customZoomOut.scale < customZoomIn.scale &&
      Math.abs(fitAgainScale - fullscreen.scale) < 0.01 &&
      ctrlWheelZoom.mode === 'custom' && ctrlWheelZoom.scale > fitAgainScale &&
      iframeCtrlWheelZoom.mode === 'custom' && iframeCtrlWheelZoom.scale > fitAgainScale,
    JSON.stringify({ customZoomIn, customZoomOut, fitAgainScale, ctrlWheelZoom, iframeCtrlWheelZoom, fit: fullscreen.scale }));

  // 退出编辑模式但保留全屏预览
  await page.click('[data-testid="artifact-edit-mode-toggle"]');
  await sleep(300);
  const exitedEditMode = await page.evaluate(() => {
    const editToggle = document.querySelector('[data-testid="artifact-edit-mode-toggle"]');
    return {
      fullscreenPanel: !!document.querySelector('[data-testid="artifact-fullscreen-panel"]'),
      frame: !!document.querySelector('[data-testid="artifact-html-preview-frame"]'),
      inspector: !!document.querySelector('[data-testid="artifact-design-inspector-host"]'),
      composer: !!document.querySelector('[data-testid="artifact-design-ai-composer"]'),
      zoomControls: !!document.querySelector('[data-testid="artifact-html-zoom-controls"]'),
      editText: (editToggle?.textContent || '').trim(),
      editTitle: editToggle?.getAttribute('title') || '',
      editPressed: editToggle?.getAttribute('aria-pressed'),
    };
  });
  rec('退出编辑模式保留全屏预览并移除 inspector 与缩放控件',
    exitedEditMode.fullscreenPanel && exitedEditMode.frame &&
      !exitedEditMode.inspector && !exitedEditMode.composer && !exitedEditMode.zoomControls &&
      exitedEditMode.editText === '编辑模式' &&
      exitedEditMode.editTitle === '进入编辑模式：放大预览并编辑选中元素' &&
      exitedEditMode.editPressed === 'false',
    JSON.stringify(exitedEditMode));

  await page.click('[data-testid="artifact-fullscreen-toggle"]');
  await sleep(350);
  const collapsed = await page.evaluate(() => ({
    fullscreenPanel: !!document.querySelector('[data-testid="artifact-fullscreen-panel"]'),
    frame: !!document.querySelector('[data-testid="artifact-html-preview-frame"]'),
    editToggle: !!document.querySelector('[data-testid="artifact-edit-mode-toggle"]'),
    toggleTitle: document.querySelector('[data-testid="artifact-fullscreen-toggle"]')?.getAttribute('title') || '',
    toggleText: (document.querySelector('[data-testid="artifact-fullscreen-toggle"]')?.textContent || '').trim(),
  }));
  rec('退出全屏恢复右侧栏且编辑模式入口消失',
    !collapsed.fullscreenPanel && collapsed.frame &&
      !collapsed.editToggle &&
      collapsed.toggleTitle === '全屏显示' &&
      collapsed.toggleText === '',
    JSON.stringify(collapsed));

  await page.click('[data-testid="artifact-close"]');
  await sleep(250);
  await clickExactButton(page, '产物与代码');
  await sleep(350);
  const manualOpen = await page.evaluate(() => ({
    fullscreenPanel: !!document.querySelector('[data-testid="artifact-fullscreen-panel"]'),
    artifactPanel: !!document.querySelector('[data-testid="artifact-fullscreen-toggle"]'),
    editToggle: !!document.querySelector('[data-testid="artifact-edit-mode-toggle"]'),
    toggleTitle: document.querySelector('[data-testid="artifact-fullscreen-toggle"]')?.getAttribute('title') || '',
    switcherText: document.querySelector('[data-testid="artifact-switcher-button"]')?.textContent || '',
    frame: !!document.querySelector('[data-testid="artifact-html-preview-frame"]'),
  }));
  rec('手动点击产物与代码只打开右侧预览，不自动全屏也不进入编辑模式',
    !manualOpen.fullscreenPanel && manualOpen.artifactPanel &&
      !manualOpen.editToggle &&
      manualOpen.toggleTitle === '全屏显示' &&
      manualOpen.frame && /landing\.html/.test(manualOpen.switcherText),
    JSON.stringify(manualOpen));

  // 打开产物面板 → 全屏 → 点 artifact-edit-mode-toggle，进入可视化编辑
  await page.click('[data-testid="artifact-fullscreen-toggle"]');
  await sleep(400);
  await page.click('[data-testid="artifact-edit-mode-toggle"]');
  await sleep(500);

  const frameHandle = await page.$('[data-testid="artifact-html-preview-frame"]');
  let frame = frameHandle && await frameHandle.contentFrame();
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
  const selectedStatus = await page.evaluate(() => ({
    title: document.querySelector('[data-testid="design-selected-element"]')?.textContent || '',
    panelText: document.querySelector('[data-testid="design-inspector-panel"]')?.textContent || '',
    detailsVisible: !!document.querySelector('[data-testid="design-selected-details"]'),
    advancedVisible: !!document.querySelector('[data-testid="design-advanced-content"]'),
    aiPlaceholder: document.querySelector('[data-testid="artifact-design-ai-input"]')?.getAttribute('placeholder') || '',
  }));
  rec('设计 runtime 注入后可选中 iframe 内元素并展示用户化摘要',
    !!frame && selectedStatus.title.includes('已选中文字') &&
      !selectedStatus.detailsVisible && !selectedStatus.panelText.includes('body >') &&
      selectedStatus.panelText.includes('常用编辑') &&
      selectedStatus.panelText.includes('高级样式') &&
      !selectedStatus.advancedVisible &&
      !selectedStatus.panelText.includes('外边距上') &&
      !selectedStatus.panelText.includes('层级') &&
      selectedStatus.aiPlaceholder === '描述你想怎么调整已选中的元素',
    JSON.stringify(selectedStatus));

  await page.evaluate(() => { window.__PINVOU_TEST_SENT_MESSAGES = []; });
  await page.click('[data-testid="artifact-design-ai-input"]');
  await page.keyboard.type('把标题颜色改成蓝色');
  await page.click('[data-testid="artifact-design-ai-send"]');
  await sleep(200);
  const designAiSent = await page.evaluate(() => {
    const sent = (window.__PINVOU_TEST_SENT_MESSAGES || [])[0] || null;
    return sent && {
      text: sent.text,
      scene: sent.meta && sent.meta.pinvouDesignScene,
      payload: sent.meta && sent.meta.pinvouPayloadText,
    };
  });
  rec('全屏 AI 输入框发送时携带选中元素上下文并复用海报场景',
    designAiSent &&
      /当前选中的/i.test(designAiSent.text || '') &&
      /把标题颜色改成蓝色/.test(designAiSent.text || '') &&
      designAiSent.scene === 'poster' &&
      /视觉海报场景约束/.test(designAiSent.payload || ''),
    JSON.stringify(designAiSent));
  await page.click('[data-testid="design-selected-details-toggle"]');
  await sleep(150);
  const selectedDetails = await page.evaluate(() => ({
    exists: !!document.querySelector('[data-testid="design-selected-details"]'),
    text: document.querySelector('[data-testid="design-selected-details"]')?.textContent || '',
  }));
  rec('选中元素技术 selector 默认隐藏，点详情后可查看',
    selectedDetails.exists && selectedDetails.text.includes('h1') && selectedDetails.text.includes('hero-title'),
    JSON.stringify(selectedDetails));
  await page.click('[data-testid="design-selected-details-toggle"]');
  await sleep(100);

  const runtimeOverlay = frame ? await frame.evaluate(() => ({
    handles: document.querySelectorAll('[data-pinvou-design-handle]').length,
    dimensionLabel: !!document.querySelector('[data-pinvou-design-label]'),
    marginBand: !!document.querySelector('[data-pinvou-design-margin]'),
    paddingBand: !!document.querySelector('[data-pinvou-design-padding]'),
  })) : null;
  rec('设计 runtime 选中态提供尺寸标签、盒模型带和 8 个缩放手柄',
    runtimeOverlay && runtimeOverlay.handles === 8 && runtimeOverlay.dimensionLabel && runtimeOverlay.marginBand && runtimeOverlay.paddingBand,
    JSON.stringify(runtimeOverlay));

  const originalFontFamily = frame ? await frame.evaluate(() => {
    const h1 = document.querySelector('h1.hero-title');
    return h1 ? getComputedStyle(h1).fontFamily : '';
  }) : '';

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
  await page.click('[data-testid="design-font-family-input"]');
  await sleep(150);
  await page.click('[data-font-preset="Georgia"]');
  await sleep(500);

  const edited = frame ? await frame.evaluate(() => {
    const h1 = document.querySelector('h1.hero-title');
    const style = h1 ? getComputedStyle(h1) : null;
    return {
      text: h1 && h1.textContent,
      fontSize: style && style.fontSize,
      color: style && style.color,
      fontFamily: style && style.fontFamily,
    };
  }) : null;
  let changesLog = await page.evaluate(() => {
    const log = document.querySelector('[data-testid="design-changes-log"]');
    return { exists: !!log, text: log && log.textContent };
  });
  const changesCollapsed = changesLog.exists && changesLog.text.includes('设计变更') && changesLog.text.includes('展开') && !changesLog.text.includes('fontSize');
  await page.click('[data-testid="design-changes-toggle"]');
  await sleep(150);
  changesLog = await page.evaluate(() => {
    const log = document.querySelector('[data-testid="design-changes-log"]');
    return { exists: !!log, text: log && log.textContent };
  });
  rec('设计面板可临时修改文案、字号、颜色和字体并记录 changes log',
    edited && edited.text === 'Pinvou 可视化编辑' && edited.fontSize === '40px' &&
      /0,\s*122,\s*255/.test(edited.color || '') && /Georgia/i.test(edited.fontFamily || '') &&
      changesCollapsed && changesLog.exists && changesLog.text.includes('设计变更') &&
      changesLog.text.includes('fontSize') && changesLog.text.includes('color') && changesLog.text.includes('fontFamily'),
    JSON.stringify({ edited, originalFontFamily, changesCollapsed, changesLog }));

  await page.click('[data-testid="artifact-switcher-button"]');
  await sleep(100);
  await page.evaluate(() => {
    const item = [...document.querySelectorAll('[data-testid="artifact-switcher-item"]')]
      .find((node) => /poster-draft\.html/.test(node.textContent || ''));
    if (item) item.click();
  });
  await sleep(500);
  const editModeExitedOnSwitch = await page.evaluate(() => {
    const editToggle = document.querySelector('[data-testid="artifact-edit-mode-toggle"]');
    return {
      switcherText: document.querySelector('[data-testid="artifact-switcher-button"]')?.textContent || '',
      editText: (editToggle?.textContent || '').trim(),
      editPressed: editToggle?.getAttribute('aria-pressed'),
      inspector: !!document.querySelector('[data-testid="artifact-design-inspector-host"]'),
    };
  });
  rec('切换产物自动退出编辑模式',
    /poster-draft\.html/.test(editModeExitedOnSwitch.switcherText || '') &&
      editModeExitedOnSwitch.editText === '编辑模式' &&
      editModeExitedOnSwitch.editPressed === 'false' &&
      !editModeExitedOnSwitch.inspector,
    JSON.stringify(editModeExitedOnSwitch));
  await page.click('[data-testid="artifact-switcher-button"]');
  await sleep(100);
  await page.evaluate(() => {
    const item = [...document.querySelectorAll('[data-testid="artifact-switcher-item"]')]
      .find((node) => /landing\.html/.test(node.textContent || ''));
    if (item) item.click();
  });
  await sleep(700);
  // 切回后重新进入编辑模式：design runtime 重新注入并按 changes log 重放手工修改
  await page.click('[data-testid="artifact-edit-mode-toggle"]');
  await sleep(500);
  const returnedFrameHandle = await page.$('[data-testid="artifact-html-preview-frame"]');
  frame = returnedFrameHandle && await returnedFrameHandle.contentFrame();
  if (frame) {
    await frame.waitForFunction(() => document.querySelector('h1.hero-title')?.textContent === 'Pinvou 可视化编辑');
  }
  const restoredAfterArtifactSwitch = frame ? await frame.evaluate(() => {
    const h1 = document.querySelector('h1.hero-title');
    const style = h1 ? getComputedStyle(h1) : null;
    return {
      text: h1 && h1.textContent,
      fontSize: style && style.fontSize,
      color: style && style.color,
      fontFamily: style && style.fontFamily,
    };
  }) : null;
  if (frame) {
    const title = await frame.$('h1.hero-title');
    const box = title && await title.boundingBox();
    if (box) {
      await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
      await sleep(300);
    }
  }
  const restoredChangesLog = await page.evaluate(() => {
    const log = document.querySelector('[data-testid="design-changes-log"]');
    return { exists: !!log, text: log && log.textContent };
  });
  rec('切换产物再返回并重新进入编辑模式后恢复手工修改和 changes log',
    restoredAfterArtifactSwitch &&
      restoredAfterArtifactSwitch.text === 'Pinvou 可视化编辑' &&
      restoredAfterArtifactSwitch.fontSize === '40px' &&
      /0,\s*122,\s*255/.test(restoredAfterArtifactSwitch.color || '') &&
      /Georgia/i.test(restoredAfterArtifactSwitch.fontFamily || '') &&
      restoredChangesLog.exists &&
      restoredChangesLog.text.includes('设计变更 4'),
    JSON.stringify({ restoredAfterArtifactSwitch, restoredChangesLog }));

  await page.click('[data-testid="design-clear-changes"]');
  await sleep(500);
  const cleared = frame ? await frame.evaluate(() => {
    const h1 = document.querySelector('h1.hero-title');
    const style = h1 ? getComputedStyle(h1) : null;
    return {
      text: h1 && h1.textContent,
      fontSize: style && style.fontSize,
      color: style && style.color,
      fontFamily: style && style.fontFamily,
    };
  }) : null;
  const logAfterClear = await page.evaluate(() => !!document.querySelector('[data-testid="design-changes-log"]'));
  rec('清空修改后恢复预览并清空 changes log',
    cleared && cleared.text === 'Pinvou Design' && cleared.fontSize === '32px' &&
      /0,\s*0,\s*0/.test(cleared.color || '') && cleared.fontFamily === originalFontFamily && !logAfterClear,
    JSON.stringify({ cleared, originalFontFamily, logAfterClear }));

  rec('页面无未处理 JavaScript 异常', errors.length === 0, errors.slice(0, 2).join(' | '));

  await browser.close();
  fs.rmSync(PROFILE, { recursive: true, force: true });
  const failed = results.filter(r => !r.pass).length;
  console.log(failed ? `\n❌ ${failed}/${results.length} FAILED` : `\n✅ ALL ${results.length} PASS`);
  process.exit(failed ? 1 : 0);
// eslint-disable-next-line unicorn/prefer-top-level-await -- smoke script keeps its existing async main() structure
})().catch(e => {
  try { fs.rmSync(PROFILE, { recursive: true, force: true }); } catch { /* profile dir already gone */ }
  console.error('FATAL', e.stack || e);
  process.exit(1);
});
