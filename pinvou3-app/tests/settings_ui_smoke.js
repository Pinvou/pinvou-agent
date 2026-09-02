#!/usr/bin/env node
/**
 * 设置页全局配置 smoke：加载真实 Vite dist，mock TauriBridge，点击真实 React UI。
 * 覆盖模型/搜索/权限/帮助反馈四个高风险设置面板：
 * - 模型列表默认单选、删除二次确认、本地/云端模型删除、添加模型保存条件、编辑模型回显凭据和同厂商切换
 * - 搜索源交互与模型页一致，添加源未点保存不得持久化，确认重启后才写设置
 * - 高级执行权限失败时回滚并 toast 提示
 * - Community uses a dedicated Settings tab that exposes the QQ group and GitHub Discussions at the top level
 * - 反馈弹窗保持与模型/搜索一致的 iOS 规格，提交成功不使用原生 alert
 */
const fs = require('fs');
const os = require('os');
const path = require('path');
const assert = require('assert');
const { startUiTestServer } = require('./ui_test_server');

const settingsViewSource = fs.readFileSync(
  path.join(__dirname, '..', 'src', 'features', 'settings', 'SettingsView.jsx'),
  'utf8',
);
const detectStart = settingsViewSource.indexOf('async function handleDetect()');
const detectEnd = settingsViewSource.indexOf('function vllmStatusLabel(', detectStart);
assert.notStrictEqual(detectStart, -1, 'local model detect handler must exist');
assert.notStrictEqual(detectEnd, -1, 'local model detect handler boundary must exist');
const detectSource = settingsViewSource.slice(detectStart, detectEnd);
assert.match(
  detectSource,
  /loaded === true/,
  'automatic local-model fill must require an explicitly loaded model',
);
assert.doesNotMatch(
  detectSource,
  /loaded !== false/,
  'unknown model state must not be treated as loaded during automatic fill',
);

function loadPuppeteer() {
  try { return require('puppeteer-core'); } catch { /* fall through */ }
  const npx = path.join(os.homedir(), '.npm', '_npx');
  if (fs.existsSync(npx)) {
    for (const directory of fs.readdirSync(npx)) {
      const candidate = path.join(npx, directory, 'node_modules', 'puppeteer-core');
      if (fs.existsSync(candidate)) {
        try { return require(candidate); } catch { /* try next */ }
      }
    }
  }
  console.error('SKIP: 找不到 puppeteer-core');
  process.exit(2);
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
].find(candidate => fs.existsSync(candidate));
if (!CHROME) {
  console.error('SKIP: 未找到 chromium/chrome，可用 env CHROME=/path/to/chrome 指定');
  process.exit(2);
}
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-settings-'));

function injectSource() {
  return `(function () {
    var handlers = Object.create(null);
    var settings = {
      theme: 'liquid-light',
      language: 'zh-Hans',
      memory_enabled: true,
      notifications: { enabled: true, task_completed: true },
      pet: { enabled: false },
      search: {
        provider: 'bing',
        enabled_providers: ['bing', 'metaso'],
        api_key: null,
        credentials: {
          metaso: { credential_state: 'configured', has_secret: true },
        },
      },
      advanced: {},
    };
    var models = [
      {
        id: 'local-qwen',
        name: '本地 vLLM',
        preset: 'local_vllm',
        model: 'qwen36_35b_256k',
        base_url: 'http://127.0.0.1:8000/v1',
        has_secret: false,
        credential_state: 'missing',
      },
      {
        id: 'cloud-deepseek',
        name: 'DeepSeek',
        preset: 'deepseek',
        model: 'deepseek-v4-flash',
        base_url: 'https://api.deepseek.com',
        // doSave 会把 provider_kind 写回 settings;官方 API 组未显式声明 baseUrl,
        // 曾导致 findCloudProviderForModel 失配、编辑弹窗隐藏配置区与测试连接。
        provider_kind: 'official_api',
        has_secret: true,
        credential_state: 'configured',
      },
      {
        id: 'cloud-kimi-global-custom',
        name: 'Kimi 国际版',
        preset: 'kimi',
        model: 'custom-kimi-id',
        base_url: 'https://api.moonshot.ai/v1',
        vendor: 'kimi',
        reasoning_effort: 'off',
        has_secret: true,
        credential_state: 'configured',
      },
    ];
    var activeModelId = 'local-qwen';
    var superPerm = false;
    var calls = [];
    var updateResponse = { available: false, current_version: '0.6.1', latest_version: '0.6.1', notes: '', platform: 'windows' };
    var modelTestResponse = { ok: true, code: 'ok', message: '连接成功，服务可用', detail: 'HTTP 200', http_status: 200 };
    var imageTestResponse = { status: 'supported', verified: true, summary: '红色', http_status: 200 };
    var imageTestDelay = 0; // 模拟探测耗时,便于断言行内忙转态
    // save_model 失败注入:非 null 时 reject 该错误,模拟连接/写盘失败。
    var saveModelError = null;
    var dependencyCheckResponse = [];
    var memoryOverview = {
      profile: { version: 1, revision: 3, identity: { call_name: '升级前称呼', assistant_alias: 'PINVOU' }, conventions: {} },
      preferences: [], work_context: [], current_focus: [], recent_activity: [], recent_work: [], pending: [], never: [],
      runtime: null, snapshot_path: '', warnings: [],
      sources: {
        profile: { available: true }, preferences: { available: true }, work_context: { available: true },
        current_focus: { available: true }, recent_activity: { available: true }, recent_work: { available: true },
        pending: { available: true }, never: { available: true },
      },
    };
    var failMemoryOverview = false;
    var failMemoryUpdate = false;
    var pendingDownloadResolve = null;
    function record(cmd, args) { calls.push({ cmd: cmd, args: args || null }); }
    window.alert = function (message) { record('window_alert', { message: message }); };
    window.confirm = function (message) { record('window_confirm', { message: message }); return true; };
    function emit(name, payload) {
      return Promise.all((handlers[name] || []).slice().map(function (handler) {
        return handler({ payload: payload || {} });
      }));
    }
    function invoke(cmd, args) {
      record(cmd, args);
      switch (cmd) {
        case 'get_platform_capabilities': return Promise.resolve({
          os: 'windows',
          showMegacubeSite: false,
          showSuperPermissionSettings: false,
          usesBundledDependencyInstaller: true,
          taskCompletionNotificationsDefault: true,
        });
        case 'get_settings': return Promise.resolve(settings);
        case 'update_settings':
          settings = Object.assign({}, settings, args.patch || {});
          return Promise.resolve(settings);
        case 'save_settings_and_restart':
          settings = Object.assign({}, settings, args.patch || {});
          return Promise.resolve(null);
        case 'update_search_settings':
          settings = Object.assign({}, settings, { search: args.search });
          return Promise.resolve(settings);
        case 'save_search_settings_and_restart':
          settings = Object.assign({}, settings, { search: args.search });
          return Promise.resolve(null);
        case 'get_effective_model_config': return Promise.resolve({ model: 'qwen36_35b_256k', base_url: 'http://127.0.0.1:8000/v1', preset: 'local_vllm', api_key_set: false });
        case 'list_models': return Promise.resolve({ models: models.slice(), active_model_id: activeModelId });
        case 'reveal_model_api_key': return Promise.resolve(args.id === 'cloud-deepseek' ? 'sk-saved-deepseek' : null);
        case 'save_model':
          if (saveModelError) return Promise.reject(new Error(saveModelError));
          var savedModel = Object.assign({}, args.model, {
            has_secret: !!args.model.api_key,
            credential_state: args.model.preset === 'local_vllm' ? 'missing' : 'configured',
          });
          models = models.filter(function (model) { return model.id !== savedModel.id; }).concat(savedModel);
          return Promise.resolve(null);
        case 'delete_model':
          models = models.filter(function (model) { return model.id !== args.id; });
          if (activeModelId === args.id) activeModelId = models[0] && models[0].id;
          return Promise.resolve(null);
        case 'set_active_model': activeModelId = args.id; return Promise.resolve(null);
        case 'test_model_connection': return Promise.resolve(Object.assign({}, modelTestResponse));
        case 'test_image_input_capability':
          return new Promise(function (resolve) {
            setTimeout(function () { resolve(Object.assign({}, imageTestResponse)); }, imageTestDelay);
          });
        case 'discover_local_vllm': return Promise.resolve({ candidates: [
          {
            provider: 'ollama',
            label: 'Ollama',
            base_url: 'http://127.0.0.1:11434/v1',
            status: 'ready',
            model: 'qwen2.5-coder:32b',
            models: [{ id: 'qwen2.5-coder:32b', loaded: true }, { id: 'deepseek-r1:14b', loaded: false }],
            max_model_len: 32768,
          },
          {
            provider: 'vllm',
            label: 'vLLM',
            base_url: 'http://127.0.0.1:8000/v1',
            status: 'ready',
            model: 'qwen36_35b_256k',
            models: ['qwen36_35b_256k'],
            max_model_len: 262144,
          },
        ] });
        case 'detect_local_vllm_setup': return Promise.resolve({ eligible: false, has_packages: false, vllm_online: false });
        case 'get_selected_pet': return Promise.resolve('lingling');
        case 'list_sessions': return Promise.resolve([]);
        case 'get_super_permission_status': return Promise.resolve(superPerm);
        case 'set_super_permission': return Promise.reject(new Error('pkexec unavailable'));
        case 'list_personas': return Promise.resolve([]);
        case 'get_backend_status': return Promise.resolve({ online: true, ok: true, status: 'online' });
        case 'check_for_update': return Promise.resolve(Object.assign({}, updateResponse));
        case 'download_update': return new Promise(function (resolve) { pendingDownloadResolve = resolve; });
        case 'install_update': return Promise.resolve(null);
        case 'find_resumable_run': return Promise.resolve(null);
        case 'check_dependencies': return Promise.resolve(dependencyCheckResponse.slice());
        case 'install_dependencies': return Promise.resolve(null);
        case 'submit_feedback': return Promise.resolve({ status: 'submitted', message: '反馈已提交，感谢你的帮助。' });
        case 'list_marketplace_tools': return Promise.resolve([]);
        case 'get_mode_state': return Promise.resolve({ mode: 'yolo', plan_phase: 'none' });
        case 'get_active_persona': return Promise.resolve(null);
        case 'list_workspace_files': return Promise.resolve([]);
        case 'list_scheduled_tasks': return Promise.resolve([]);
        case 'list_scheduled_task_recent_runs': return Promise.resolve([]);
        case 'get_app_version': return Promise.resolve('0.6.1');
        case 'get_memory_overview':
          return failMemoryOverview ? Promise.reject(new Error('snapshot locked')) : Promise.resolve(memoryOverview);
        case 'update_memory_profile':
          if (failMemoryUpdate) return Promise.reject(new Error('profile write failed'));
          memoryOverview = Object.assign({}, memoryOverview, {
            profile: Object.assign({}, memoryOverview.profile, {
              identity: Object.assign({}, memoryOverview.profile.identity, args.patch || {}),
            }),
          });
          return Promise.resolve({
            profile: memoryOverview.profile,
            runtime: null,
            warnings: [{ code: 'runtime_refresh_failed', source: 'runtime', detail: 'runtime cache locked' }],
          });
        default: return Promise.resolve(null);
      }
    }
    window.__SETTINGS_TEST__ = {
      calls: calls,
      emit: emit,
      models: function () { return models.slice(); },
      settings: function () { return settings; },
      activeModelId: function () { return activeModelId; },
      setUpdateResponse: function (next) { updateResponse = Object.assign({}, updateResponse, next || {}); },
      setModelTestResponse: function (next) { modelTestResponse = Object.assign({}, next || {}); },
      setImageTestResponse: function (next) { imageTestResponse = Object.assign({}, next || {}); },
      setImageTestDelay: function (ms) { imageTestDelay = Number(ms) || 0; },
      setSaveModelError: function (message) { saveModelError = message || null; },
      setModelImageCapability: function (id, override) {
        models = models.map(function (m) { return m.id === id ? Object.assign({}, m, { image_capability_override: override }) : m; });
      },
      setDependencyCheckResponse: function (next) { dependencyCheckResponse = (next || []).slice(); },
      setFailMemoryOverview: function (value) { failMemoryOverview = !!value; },
      setFailMemoryUpdate: function (value) { failMemoryUpdate = !!value; },
      resolveDownload: function () {
        if (pendingDownloadResolve) {
          var resolve = pendingDownloadResolve;
          pendingDownloadResolve = null;
          resolve({ package_path: 'C:\\\\tmp\\\\pinvou.zip', installer_path: 'C:\\\\tmp\\\\pinvou.msi', latest_version: updateResponse.latest_version });
        }
      },
    };
    window.__TAURI__ = {
      core: { invoke: invoke },
      event: {
        listen: function (name, handler) {
          (handlers[name] || (handlers[name] = [])).push(handler);
          return Promise.resolve(function () {
            var index = handlers[name].indexOf(handler);
            if (index >= 0) handlers[name].splice(index, 1);
          });
        },
        emit: emit,
      },
      window: {
        getCurrentWindow: function () {
          return {
            minimize: function () {},
            maximize: function () {},
            close: function () {},
            toggleMaximize: function () {},
            isMaximized: function () { return Promise.resolve(false); },
            onResized: function () { return Promise.resolve(function () {}); },
            startDragging: function () {},
          };
        },
      },
      dialog: { open: function () { return Promise.resolve(null); } },
    };
  })();`;
}

const sleep = ms => new Promise(resolve => { setTimeout(resolve, ms); });
const callCount = (page, cmd) => page.evaluate(command =>
  window.__SETTINGS_TEST__.calls.filter(call => call.cmd === command).length, cmd);

async function clickExact(page, text) {
  const ok = await page.evaluate(t => {
    const elements = [...document.querySelectorAll('button,span,div,a,h1,h2')].filter(element => (element.textContent || '').trim() === t);
    const element = elements.find(el => el.tagName === 'BUTTON') || elements[elements.length - 1];
    if (!element) return false;
    const target = element.closest('button') || element;
    target.scrollIntoView({ block: 'center' });
    target.click();
    return true;
  }, text);
  if (!ok) throw new Error('找不到可点击文本: ' + text);
}

async function clickModalExact(page, text) {
  const ok = await page.evaluate(t => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    if (!root) return false;
    const elements = [...root.querySelectorAll('button,span,div,a,h1,h2')].filter(element => (element.textContent || '').trim() === t);
    const element = elements.find(el => el.tagName === 'BUTTON') || elements[elements.length - 1];
    if (!element) return false;
    const target = element.closest('button') || element;
    target.scrollIntoView({ block: 'center' });
    target.click();
    return true;
  }, text);
  if (!ok) {
    const dialogText = await page.evaluate(() => (document.querySelector('[data-testid="model-form-dialog"]')?.innerText || '').slice(0, 600));
    throw new Error('找不到弹窗内可点击文本: ' + text + '\n弹窗文本: ' + dialogText);
  }
}

async function clickSettingsSection(page, label) {
  const ok = await page.evaluate(text => {
    const buttons = [...document.querySelectorAll('aside button')].filter(button => (button.textContent || '').trim() === text);
    const button = buttons[buttons.length - 1];
    if (!button) return false;
    button.click();
    return true;
  }, label);
  if (!ok) throw new Error('找不到设置分区: ' + label);
  await sleep(250);
}

async function clickRowAction(page, rowText, actionText) {
  const ok = await page.evaluate((rowText, actionText) => {
    const row = [...document.querySelectorAll('div')].find(node =>
      typeof node.className === 'string'
      && node.className.includes('grid-cols-')
      && (node.textContent || '').includes(rowText));
    const button = row && [...row.querySelectorAll('button')].find(candidate => (candidate.textContent || '').trim() === actionText);
    if (!button) return false;
    button.scrollIntoView({ block: 'center' });
    button.click();
    return true;
  }, rowText, actionText);
  if (!ok) throw new Error('找不到行操作: ' + rowText + ' / ' + actionText);
}

async function clickRowRadio(page, rowText) {
  const ok = await page.evaluate(rowText => {
    const row = [...document.querySelectorAll('div')].find(node =>
      typeof node.className === 'string'
      && node.className.includes('grid-cols-')
      && (node.textContent || '').includes(rowText));
    const button = row && row.querySelector('button[title]');
    if (!button) return false;
    button.scrollIntoView({ block: 'center' });
    button.click();
    return true;
  }, rowText);
  if (!ok) throw new Error('找不到行单选: ' + rowText);
}

async function modalWidth(page, headingText) {
  return page.evaluate(text => {
    const heading = [...document.querySelectorAll('h2,h3')].find(node => (node.textContent || '').trim() === text);
    if (!heading) return null;
    let node = heading.parentElement;
    while (node && node.getBoundingClientRect().width < 200) node = node.parentElement;
    return node ? Math.round(node.getBoundingClientRect().width) : null;
  }, headingText);
}

(async () => {
  const { url } = await startUiTestServer();
  const browser = await puppeteer.launch({
    executablePath: CHROME,
    headless: 'new',
    args: ['--no-sandbox', '--disable-gpu', '--no-first-run', '--no-default-browser-check'],
    userDataDir: PROFILE,
  });
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push('console:' + message.text()); });
  await page.evaluateOnNewDocument(injectSource());
  await page.setViewport({ width: 1440, height: 1000 });
  await page.goto(url, { waitUntil: 'networkidle0' });
  await page.waitForFunction(() => window.TauriBridge && document.querySelector('[data-testid="app-root"]'), { timeout: 20000 });
  await sleep(1000);

  const results = [];
  const rec = (name, pass, detail = '') => {
    results.push({ name, pass });
    console.log(`${pass ? '✅' : '❌'} ${name}${detail ? '  ' + detail : ''}`);
  };

  await page.evaluate(() => {
    const button = [...document.querySelectorAll('button[title="设置"]')].pop();
    if (button) button.click();
  });
  await page.waitForFunction(() => document.querySelector('[data-testid="app-root"]')?.getAttribute('data-current-view') === 'settings', { timeout: 8000 });
  await sleep(400);
  rec('① 设置页可打开且无错误边界', await page.evaluate(() => document.body.innerText.includes('通用') && !document.body.innerText.includes('设置页加载失败')));

  await page.click('[data-testid="settings-section-memory"]');
  await page.waitForFunction(() => (document.querySelector('[data-testid="memory-profile-call-name"]')?.textContent || '').includes('升级前称呼'));
  rec('①a 升级后的记忆资料从权威 profile 正常回显', await page.evaluate(() =>
    (document.querySelector('[data-testid="memory-profile-call-name"]')?.textContent || '').includes('升级前称呼')));
  await page.evaluate(() => window.__SETTINGS_TEST__.setFailMemoryOverview(true));
  await page.click('[data-testid="memory-profile-call-name"] button');
  await page.click('[data-testid="memory-editor-input"]', { clickCount: 3 });
  await page.type('[data-testid="memory-editor-input"]', '升级后称呼');
  await page.click('[data-testid="memory-editor-save"]');
  await page.waitForFunction(() => !document.querySelector('[data-testid="memory-editor-input"]'));
  rec('①b 派生概览刷新失败不吞掉已保存的称呼', await page.evaluate(() =>
    (document.querySelector('[data-testid="memory-profile-call-name"]')?.textContent || '').includes('升级后称呼')));
  rec('①c 记忆加载失败有明确提示且不伪装成未设置', await page.evaluate(() =>
    document.querySelector('[data-testid="memory-settings-error"]')?.getAttribute('role') === 'alert'
      && document.querySelector('[data-testid="memory-settings-error"]')?.getAttribute('aria-live') === 'polite'));
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.setFailMemoryOverview(false);
    window.__SETTINGS_TEST__.setFailMemoryUpdate(true);
  });
  await page.click('[data-testid="memory-profile-call-name"] button');
  await page.click('[data-testid="memory-editor-input"]', { clickCount: 3 });
  await page.type('[data-testid="memory-editor-input"]', '不会保存');
  await page.click('[data-testid="memory-editor-save"]');
  await page.waitForFunction(() => !!document.querySelector('[data-testid="memory-editor-error"]'));
  rec('①d 源资料保存失败时保留编辑器并展示错误', await page.evaluate(() =>
    !!document.querySelector('[data-testid="memory-editor-input"]')
      && document.querySelector('[data-testid="memory-editor-error"]')?.getAttribute('role') === 'alert'
      && document.querySelector('[data-testid="memory-editor-error"]')?.getAttribute('aria-live') === 'assertive'));
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.setFailMemoryUpdate(false);
    const input = document.querySelector('[data-testid="memory-editor-input"]');
    const dialog = input && input.closest('.fixed');
    const buttons = dialog ? [...dialog.querySelectorAll('button')] : [];
    if (buttons.length) buttons[0].click();
  });

  await clickSettingsSection(page, '更新');
  await page.evaluate(async () => {
    window.__SETTINGS_TEST__.setUpdateResponse({
      available: true,
      current_version: '0.6.1',
      latest_version: '0.6.2',
      notes: '设置页更新按钮测试',
      platform: 'windows',
    });
    await window.TauriBridge.updater.checkForUpdate();
  });
  await sleep(500);
  const beforeDownloadCalls = await callCount(page, 'download_update');
  await page.click('#settings-version-update [data-settings-update-action="true"]');
  await page.evaluate(() => window.__SETTINGS_TEST__.emit('update:progress', { downloaded: 37, total: 100 }));
  await sleep(500);
  const updateDownloadState = await page.evaluate(() => {
    const root = document.querySelector('#settings-version-update');
    const button = root && root.querySelector('button');
    return {
      disabled: !!(button && button.disabled),
      text: button ? button.textContent.trim() : '',
      desc: root ? root.innerText : '',
    };
  });
  const afterDownloadCalls = await callCount(page, 'download_update');
  rec('①b 设置页下载按钮进入下载态后可取消并显示进度',
    beforeDownloadCalls === 0
    && afterDownloadCalls === 1
    && !updateDownloadState.disabled
    && updateDownloadState.text.includes('取消下载')
    && updateDownloadState.desc.includes('正在下载更新 37%'),
    JSON.stringify(updateDownloadState));
  await page.click('#settings-version-update [data-settings-update-action="true"]');
  await sleep(150);
  rec('①c 设置页可取消正在进行的更新下载', await callCount(page, 'cancel_download') === 1);
  await page.evaluate(() => window.__SETTINGS_TEST__.resolveDownload());
  await sleep(250);

  await clickSettingsSection(page, '模型');
  const modelList = await page.evaluate(() => {
    const text = document.body.innerText;
    const rows = [...document.querySelectorAll('div')].filter(node => typeof node.className === 'string' && node.className.includes('grid-cols-'));
    const deepseekRow = rows.find(node => (node.textContent || '').includes('deepseek-v4-flash'));
    const localRow = rows.find(node => (node.textContent || '').includes('qwen36_35b_256k') && (node.textContent || '').includes('本地模型'));
    return {
      localTag: text.includes('本地模型'),
      activeTag: text.includes('默认'),
      noStatusNoise: !text.includes('已配置') && !text.includes('未配置'),
      localHasDelete: !!localRow && (localRow.textContent || '').includes('删除'),
      qwenLocalUsesBrandIcon: !!localRow && !!localRow.querySelector('img'),
      cloudHasDelete: !!deepseekRow,
      oldSaveSearchButtonHidden: !text.includes('保存搜索配置'),
    };
  });
  rec('② 模型列表符合 iOS 列表交互标识且用户本地模型可删除', Object.values(modelList).every(Boolean), JSON.stringify(modelList));

  await clickRowRadio(page, 'deepseek-v4-flash');
  await sleep(350);
  const activeSet = await page.evaluate(() => window.__SETTINGS_TEST__.activeModelId());
  rec('③ 圆圈单选可设默认模型', activeSet === 'cloud-deepseek', activeSet);

  await clickRowAction(page, 'deepseek-v4-flash', '编辑');
  await sleep(250);
  const maskedSavedKey = await page.evaluate(() => ({
    maskedPlaceholder: [...document.querySelectorAll('input')].some(node => node.placeholder === '••••••••'),
    noConfiguredText: !document.body.innerText.includes('已配置'),
  }));
  await clickExact(page, '显示');
  await sleep(350);
  const editModelBehavior = await page.evaluate(() => {
    const text = document.body.innerText;
    const input = [...document.querySelectorAll('input')].find(node => node.value === 'sk-saved-deepseek');
    return {
      revealCall: window.__SETTINGS_TEST__.calls.some(call => call.cmd === 'reveal_model_api_key' && call.args.id === 'cloud-deepseek'),
      keyRevealed: !!input,
      sameProviderOnlyClosed: !text.includes('kimi-k3') && !text.includes('glm-5.2'),
      // 带 provider_kind 的官方模型必须仍能找到目录组,配置区与测试连接不被隐藏。
      testConnectionVisible: text.includes('测试连接'),
    };
  });
  await clickExact(page, '更换');
  await sleep(250);
  const sameProviderPicker = await page.evaluate(() => {
    const text = document.body.innerText;
    return text.includes('deepseek-v4-pro') && text.includes('deepseek-v4-flash') && !text.includes('kimi-k3') && !text.includes('glm-5.2');
  });
  rec('④ 编辑模型默认掩码显示已保存 Key，显示后回显且只允许同厂商更换', Object.values(maskedSavedKey).every(Boolean) && Object.values(editModelBehavior).every(Boolean) && sameProviderPicker, JSON.stringify({ ...maskedSavedKey, ...editModelBehavior, sameProviderPicker }));
  await clickExact(page, '取消');
  await sleep(200);

  await clickExact(page, '添加模型');
  await sleep(250);
  await clickExact(page, '深度求索 / DeepSeek');
  await sleep(250);
  await clickModalExact(page, '模型');
  await sleep(250);
  await clickModalExact(page, '自定义 DeepSeek 模型');
  await sleep(250);
  const modelIdInput = await page.$('input[placeholder="输入模型 ID"]');
  await modelIdInput.click();
  await modelIdInput.type('custom-model-id');
  const typedModelId = await page.evaluate(() => document.querySelector('input[placeholder="输入模型 ID"]')?.value || '');
  rec('④ 模型 ID 输入框可连续输入', typedModelId === 'custom-model-id', typedModelId);
  await clickExact(page, '取消');
  await sleep(200);

  const deleteBefore = await callCount(page, 'delete_model');
  await clickRowAction(page, 'deepseek-v4-flash', '删除');
  await sleep(250);
  const deleteDialog = await page.evaluate(() => document.body.innerText.includes('删除模型？') && document.body.innerText.includes('将移除该模型配置'));
  const deleteWidth = await modalWidth(page, '删除模型？');
  const deleteStillBeforeConfirm = await callCount(page, 'delete_model');
  rec('④ 删除模型先出 iOS 二次确认且未立即删除', deleteDialog && deleteWidth >= 260 && deleteWidth <= 285 && deleteStillBeforeConfirm === deleteBefore, `width=${deleteWidth}`);
  await clickExact(page, '取消');
  await sleep(200);
  await clickRowAction(page, 'deepseek-v4-flash', '删除');
  await sleep(200);
  await clickExact(page, '删除模型');
  await sleep(500);
  const deleted = await page.evaluate(() => ({
    calls: window.__SETTINGS_TEST__.calls.filter(call => call.cmd === 'delete_model').map(call => call.args.id),
    remaining: window.__SETTINGS_TEST__.models().map(model => model.id),
  }));
  rec('⑤ 确认后才调用删除模型并刷新列表', deleted.calls.includes('cloud-deepseek') && !deleted.remaining.includes('cloud-deepseek'), JSON.stringify(deleted));

  // 思考深度档位迁移：编辑 Kimi Global 自定义模型（已存 off），手输改 ID 为 kimi-k3 后
  // 档位表由 off/high 变为 low/high/max，off 不在其中，必须按新 route 归一为 low——
  // 否则界面无高亮、保存仍写旧值（底座 K3 实际按 low 执行）。
  await clickRowAction(page, 'custom-kimi-id', '编辑');
  await sleep(300);
  await page.evaluate(() => {
    const input = document.querySelector('[data-testid="model-form-dialog"] input[placeholder="输入模型 ID"]');
    if (!input) return;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(input, 'kimi-k3');
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await sleep(300);
  const kimiEffortMigration = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    if (!dialog) return { found: false, labels: [], selected: [] };
    const label = [...dialog.querySelectorAll('span')].find(node => (node.textContent || '').trim() === '思考深度');
    const row = label && label.parentElement;
    const buttons = row ? [...row.querySelectorAll('button')] : [];
    const selected = buttons.filter(node => (node.className || '').includes('bg-[#007AFF]')).map(node => (node.textContent || '').trim());
    return { found: !!row, labels: buttons.map(node => (node.textContent || '').trim()), selected };
  });
  rec('⑥ 自定义模型 ID 手输 kimi-k3 后思考深度归一为 low（关闭残留 off）',
    kimiEffortMigration.found
      && kimiEffortMigration.labels.includes('低')
      && !kimiEffortMigration.labels.includes('关闭')
      && kimiEffortMigration.selected.length === 1
      && kimiEffortMigration.selected[0] === '低',
    JSON.stringify(kimiEffortMigration));
  await clickExact(page, '取消');
  await sleep(200);

  await clickExact(page, '添加模型');
  await sleep(300);
  const freshCatalog = await page.evaluate(() => {
    const text = document.body.innerText;
    const stale = ['deepseek-chat', 'kimi-k2.5', 'glm-4-plus', 'minimax-m1', 'abab6.5s-chat', 'mimo-v2-flash', 'qwen-max', 'qwen-plus', 'qwen-turbo', 'doubao-pro-256k', 'gpt-4o', 'qwen3.8-max-preview'];
    return {
      hasSections: text.includes('Coding Plan') && text.includes('官方 API') && text.includes('自定义兼容接口'),
      hasProviders: text.includes('智谱 Coding Plan / GLM Coding Plan') && text.includes('Kimi Coding Plan') && text.includes('深度求索 / DeepSeek') && text.includes('MiniMax 中国版 / MiniMax China'),
      hasOverseas: text.includes('OpenAI') && text.includes('Anthropic Claude') && text.includes('Google Gemini') && text.includes('xAI Grok'),
      hasIntlNodes: text.includes('Kimi 国际版 / Kimi Global') && text.includes('智谱国际版 / GLM API (z.ai)')
        && text.includes('MiniMax 国际版 / MiniMax Global') && text.includes('通义千问国际版 / Qwen International'),
      hasTokenPlan: text.includes('通义千问 Token Plan'),
      providerFirst: !text.includes('deepseek-v4-pro') && !text.includes('kimi-k3'),
      noStale: stale.every(name => !text.includes(name)),
    };
  });
  rec('⑥ 添加模型首屏按厂商分组且不展示已下线推荐项', Object.values(freshCatalog).every(Boolean), JSON.stringify(freshCatalog));
  const addPickerDefault = await page.evaluate(() => {
    const text = document.body.innerText;
    return {
      hasCloudTab: [...document.querySelectorAll('button')].some(button => (button.textContent || '').trim() === '云端模型'),
      hasLocalTab: [...document.querySelectorAll('button')].some(button => (button.textContent || '').trim() === '本地模型'),
      cloudCatalogVisible: text.includes('智谱 Coding Plan / GLM Coding Plan') && text.includes('深度求索 / DeepSeek'),
      localPickerHidden: !text.includes('自动检测本地模型') && !text.includes('手动添加本地模型'),
    };
  });
  rec('⑥.1 添加模型默认展示云端 tab 且保留本地 tab 入口', Object.values(addPickerDefault).every(Boolean), JSON.stringify(addPickerDefault));

  await clickExact(page, '通义千问 Token Plan');
  await sleep(300);
  await clickModalExact(page, '模型');
  await sleep(200);
  const tokenPlanModels = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const lines = (root ? root.innerText : '').split('\n').map(line => line.trim());
    return {
      hasGa: lines.includes('qwen3.8-max'),
      hasFlash: lines.includes('qwen3.6-flash'),
      noPreview: !lines.includes('qwen3.8-max-preview'),
    };
  });
  rec('⑥.1d Token Plan 条目列正式旗舰与白名单 Flash，且不展示已下线预览版', Object.values(tokenPlanModels).every(Boolean), JSON.stringify(tokenPlanModels));
  await clickExact(page, '取消');
  await sleep(300);

  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '豆包');
  await sleep(300);
  await clickModalExact(page, '模型');
  await sleep(200);
  const doubaoModels = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const text = root ? root.innerText : '';
    return {
      hasCanonicalId: text.includes('doubao-seed-2-1-pro-260628'),
      noLegacyId: !text.includes('doubao-seed-2.1-pro'),
    };
  });
  rec('⑥.1e 豆包条目使用火山方舟规范模型 ID', Object.values(doubaoModels).every(Boolean), JSON.stringify(doubaoModels));
  await clickExact(page, '取消');
  await sleep(300);

  await clickExact(page, '添加模型');
  await sleep(300);

  await clickExact(page, '智谱 Coding Plan / GLM Coding Plan');
  await sleep(300);
  const codingPlanBeforePick = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const text = root ? root.innerText : '';
    return {
      title: text.includes('添加 智谱 Coding Plan'),
      defaultModel: text.includes('GLM-5.2'),
      noDisplayNameField: !text.includes('显示名'),
      noServiceUrlField: !text.includes('服务地址') && !(root && [...root.querySelectorAll('input')].some(input => input.value === 'https://open.bigmodel.cn/api/coding/paas/v4')),
      noNativeSelect: document.querySelectorAll('[data-testid="model-form-dialog"] select').length === 0,
    };
  });
  await clickModalExact(page, '模型');
  await sleep(200);
  await clickModalExact(page, 'GLM-5-Turbo');
  await sleep(200);
  const codingApiInput = await page.$('input[placeholder="输入 API Key"]');
  await codingApiInput.type('sk-coding-plan');
  await sleep(150);
  await page.evaluate(() => window.__SETTINGS_TEST__.setModelTestResponse({
    ok: false,
    code: 'auth_invalid',
    message: 'API Key 无效，请检查后重新填写',
    detail: 'HTTP 401',
    http_status: 401,
  }));
  await clickExact(page, '测试连接');
  await sleep(250);
  const codingPlanTestResult = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const text = root ? root.innerText : '';
    return {
      friendlyMessage: text.includes('API Key 无效，请检查后重新填写'),
      noTechnicalDetail: !text.includes('技术详情') && !text.includes('HTTP 401'),
      noAddressHint: !text.includes('改地址') && !text.includes('修改服务地址'),
      called: window.__SETTINGS_TEST__.calls.some(call => call.cmd === 'test_model_connection' && call.args.baseUrl === 'https://open.bigmodel.cn/api/coding/paas/v4'),
    };
  });
  rec('⑥.1a Coding Plan 测试连接只显示友好错误且不提示改地址', Object.values(codingPlanTestResult).every(Boolean), JSON.stringify(codingPlanTestResult));
  await clickExact(page, '保存');
  await sleep(500);
  const savedCodingPlan = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return call && call.args && call.args.model;
  });
  rec('⑥.1b Coding Plan 隐藏服务地址并保存专用地址',
    Object.values(codingPlanBeforePick).every(Boolean)
      && savedCodingPlan
      && savedCodingPlan.provider_kind === 'coding_plan'
      && savedCodingPlan.vendor === 'glm'
      && savedCodingPlan.model === 'glm-5-turbo'
      && savedCodingPlan.name === 'glm-5-turbo'
      && savedCodingPlan.base_url === 'https://open.bigmodel.cn/api/coding/paas/v4'
      && savedCodingPlan.api_key === 'sk-coding-plan'
      && savedCodingPlan.credential_action === 'replace',
    JSON.stringify({ ...codingPlanBeforePick, savedCodingPlan }));

  await clickExact(page, '添加模型');
  await sleep(250);
  await page.evaluate(() => document.querySelector('[data-testid="model-form-backdrop"]')?.click());
  await sleep(200);
  const addPickerOutsideClick = await page.evaluate(() => {
    const stayedOpen = !!document.querySelector('[data-testid="model-form-dialog"]');
    document.querySelector('[data-testid="model-form-cancel"]')?.click();
    return { stayedOpen };
  });
  await sleep(200);
  const addPickerClosedByCancel = await page.evaluate(() => !document.querySelector('[data-testid="model-form-dialog"]'));
  rec('⑥.1c 添加模型选择弹窗点击外部区域不关闭且取消可关闭',
    addPickerOutsideClick.stayedOpen && addPickerClosedByCancel,
    JSON.stringify({ ...addPickerOutsideClick, closedByCancel: addPickerClosedByCancel }));

  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '本地模型');
  await sleep(250);
  const localPickerInitial = await page.evaluate(() => {
    const text = document.body.innerText;
    const localRows = ['自动检测本地模型', '手动添加本地模型'].map(label => {
      const title = [...document.querySelectorAll('span')].find(node => (node.textContent || '').trim() === label);
      let row = title;
      while (row && !String(row.className || '').includes('items-center')) row = row.parentElement;
      return row;
    }).filter(Boolean);
    return {
      hasAutoDetect: text.includes('自动检测本地模型') && text.includes('检测 vLLM、Ollama、LM Studio'),
      hasManual: text.includes('手动添加本地模型') && text.includes('填写 API 地址和模型 ID'),
      cloudCatalogHidden: !text.includes('深度求索 / DeepSeek'),
      localIconIsGeneric: localRows.length === 2 && localRows.every(row => !row.querySelector('img') && row.querySelector('svg')),
    };
  });
  rec('⑥.2 本地模型 tab 只保留检测和手动添加两条主路径', Object.values(localPickerInitial).every(Boolean), JSON.stringify(localPickerInitial));
  await clickExact(page, '检测');
  await sleep(500);
  const localDetectUi = await page.evaluate(() => {
    const text = document.body.innerText;
    return {
      called: window.__SETTINGS_TEST__.calls.some(call => call.cmd === 'discover_local_vllm'),
      ollamaModel: text.includes('qwen2.5-coder:32b') && text.includes('deepseek-r1:14b'),
      vllmModel: text.includes('qwen36_35b_256k'),
      providerLine: text.includes('Ollama · http://127.0.0.1:11434/v1') && text.includes('vLLM · http://127.0.0.1:8000/v1'),
      notLoadedTag: text.includes('未加载') && text.includes('尚未载入内存，首次使用时会自动加载'),
      loadedSortedFirst: text.includes('qwen2.5-coder:32b') && text.includes('deepseek-r1:14b')
        && text.indexOf('qwen2.5-coder:32b') < text.indexOf('deepseek-r1:14b'),
    };
  });
  rec('⑥.3 本地模型自动检测展示多个服务与多个模型 ID', Object.values(localDetectUi).every(Boolean), JSON.stringify(localDetectUi));
  const localAddClicked = await page.evaluate(() => {
    const title = [...document.querySelectorAll('span')].find(node => (node.textContent || '').trim() === 'qwen2.5-coder:32b');
    let row = title;
    while (row && ![...row.querySelectorAll('button')].some(button => (button.textContent || '').trim() === '添加')) {
      row = row.parentElement;
    }
    const button = row && [...row.querySelectorAll('button')].find(candidate => (candidate.textContent || '').trim() === '添加');
    if (!button) return false;
    button.click();
    return true;
  });
  await sleep(500);
  const savedLocalModel = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return call && call.args && call.args.model;
  });
  rec('⑥.4 检测候选可一键添加为本地模型且不强制 API Key',
    localAddClicked
      && savedLocalModel
      && savedLocalModel.preset === 'local_vllm'
      && savedLocalModel.model === 'qwen2.5-coder:32b'
      && savedLocalModel.base_url === 'http://127.0.0.1:11434/v1'
      && savedLocalModel.context_window_tokens === 32768
      && savedLocalModel.api_key === ''
      && savedLocalModel.credential_action === 'keep_existing',
    JSON.stringify(savedLocalModel));

  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '本地模型');
  await sleep(200);
  await clickExact(page, '手动添加本地模型');
  await sleep(250);
  const manualLocalForm = await page.evaluate(() => {
    const text = document.body.innerText;
    const save = [...document.querySelectorAll('button')].reverse().find(button => (button.textContent || '').trim() === '保存');
    return {
      noDisplayNameOnCreate: !text.includes('显示名'),
      hasModelId: text.includes('本地模型 ID'),
      hasBaseUrl: text.includes('API 地址'),
      hasKeySwitch: text.includes('需要 API Key'),
      hasConnectionTest: text.includes('测试连接'),
      saveDisabledBeforeModelId: !!save && save.disabled,
      keyInputHiddenByDefault: document.querySelectorAll('input[placeholder="输入 API Key"]').length === 0,
    };
  });
  rec('⑥.5 手动添加本地模型表单保持 iOS 分组且默认无需 Key，不强制显示名', Object.values(manualLocalForm).every(Boolean), JSON.stringify(manualLocalForm));
  // 思考深度残留：新建草稿默认 DeepSeek 初始化为 high，切到「手动添加本地模型」
  // 必须把思考深度重置为 vLLM 默认 off（关闭），否则保存会显式写入 high，绕过桥接层
  // vllm→off 的 SSE timeout 约束。此处断言真实 UI 选中「关闭」。
  const manualLocalEffort = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    if (!dialog) return { found: false, labels: [], selected: [] };
    const label = [...dialog.querySelectorAll('span')].find(node => (node.textContent || '').trim() === '思考深度');
    const row = label && label.parentElement;
    const buttons = row ? [...row.querySelectorAll('button')] : [];
    const selected = buttons.filter(node => (node.className || '').includes('bg-[#007AFF]')).map(node => (node.textContent || '').trim());
    return { found: !!row, labels: buttons.map(node => (node.textContent || '').trim()), selected };
  });
  rec('⑥.5b 手动添加本地模型思考深度重置为 vLLM 默认「关闭」（不残留 high）',
    manualLocalEffort.found
      && manualLocalEffort.labels.includes('关闭')
      && manualLocalEffort.selected.length === 1
      && manualLocalEffort.selected[0] === '关闭',
    JSON.stringify(manualLocalEffort));
  // A local model's window comes from the probed max_model_len (authoritative), so no manual input is offered.
  const localNoContextWindowField = await page.evaluate(() =>
    !document.querySelector('[data-testid="model-form-context-window"]'));
  rec('⑥.5c local model form hides the context window input (probed value is authoritative)', localNoContextWindowField);
  await clickExact(page, '取消');
  await sleep(200);

  // Probe window and credential pass-through (PR #218 round 6): openai_compatible
  // + a local/private address → thinking depth enters pending "from the moment
  // the probe is scheduled" (the default four tiers from
  // localProbeTiersForKind(null) are not exposed, preventing the user from
  // picking/saving a misleading tier before the endpoint type is known); the
  // probe_local_server_kind issued after the debounce must carry the form's
  // apiKey/modelId (authenticated vLLM 401s on /v1/models without credentials,
  // misjudging generic).
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(250);
  const baseUrlInput = await page.evaluateHandle(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const label = dialog && [...dialog.querySelectorAll('label,span')].find(node => (node.textContent || '').trim() === 'API 地址');
    const row = label && label.closest('div');
    return row && row.querySelector('input');
  });
  await baseUrlInput.type('http://127.0.0.1:8000/v1', { delay: 20 });
  // Within the < 400ms debounce window: the pending copy must show and no
  // tier buttons may be offered (pre-fix, probePending was only set in the
  // timer callback, so the window fell back to the default four tiers).
  const probeWindow = await page.evaluate(() => {
    const dialog = document.querySelector('[data-testid="model-form-dialog"]');
    const text = dialog ? dialog.innerText : '';
    const effortRow = [...(dialog ? dialog.querySelectorAll('span') : [])].find(node => (node.textContent || '').trim() === '思考深度');
    const buttons = effortRow && effortRow.parentElement ? [...effortRow.parentElement.querySelectorAll('button')].map(node => (node.textContent || '').trim()) : [];
    return {
      pendingShown: text.includes('正在探测服务类型'),
      noTiersDuringWindow: buttons.length === 0,
    };
  });
  rec('⑥.5d local-compatible probe window enters pending immediately, default four tiers not exposed',
    probeWindow.pendingShown && probeWindow.noTiersDuringWindow,
    JSON.stringify(probeWindow));
  // After the debounce (400ms) settles: the probe command must be issued and
  // carry the credential args (empty form key → apiKey:null + edit-mode model
  // id; create mode modelId:null, with Rust reading the stored credential as
  // the fallback).
  await sleep(700);
  const probeCall = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'probe_local_server_kind');
    return call && call.args;
  });
  rec('⑥.5e probe command carries credential args (apiKey/modelId); authenticated endpoints no longer misjudged as generic',
    !!probeCall
      && probeCall.baseUrl === 'http://127.0.0.1:8000/v1'
      && 'apiKey' in probeCall
      && 'modelId' in probeCall,
    JSON.stringify(probeCall));
  await clickExact(page, '取消');
  await sleep(200);
  // Custom-model context-window entry: a newly released model ID not yet in the
  // catalog (e.g. a 1M-class model) previously had no way to declare its window,
  // so saves were always null and the runtime fell back to the conservative 128K.
  // Regression lock: field renders empty, manual value persists via save_model,
  // edit echoes it back, and clearing saves null.
  await clickExact(page, '添加模型');
  await sleep(300);
  await clickExact(page, '云端模型');
  await sleep(150);
  await clickExact(page, 'OpenAI Compatible');
  await sleep(300);
  const customContextEntry = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const field = root && root.querySelector('[data-testid="model-form-context-window"]');
    return {
      visible: !!field,
      empty: !!field && field.value === '',
      hintShown: !!root && root.innerText.includes('1M 填 1048576'),
    };
  });
  rec('⑥.5f custom compatible model form offers an empty context-window entry with hint',
    customContextEntry.visible && customContextEntry.empty && customContextEntry.hintShown,
    JSON.stringify(customContextEntry));
  const customModelIdInput = await page.$('input[placeholder="输入模型 ID"]');
  await customModelIdInput.type('glm-5.3-flash');
  await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const typeInto = (input, text) => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, text);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    };
    const baseUrlInput = [...root.querySelectorAll('label')].find(node => (node.textContent || '').trim() === 'API 地址').nextElementSibling;
    typeInto(baseUrlInput, 'https://example.com/v1');
    typeInto(root.querySelector('[data-testid="model-form-context-window"]'), '1048576');
  });
  const customKeyInput = await page.$('input[placeholder="输入 API Key"]');
  await customKeyInput.type('sk-ctx-test');
  await sleep(150);
  await clickExact(page, '保存');
  await sleep(500);
  const savedCustomContext = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return call && call.args && call.args.model;
  });
  rec('⑥.5g manual context window persists via save_model',
    savedCustomContext
      && savedCustomContext.model === 'glm-5.3-flash'
      && savedCustomContext.preset === 'openai_compatible'
      && savedCustomContext.provider_kind === 'custom'
      && savedCustomContext.base_url === 'https://example.com/v1'
      && savedCustomContext.context_window_tokens === 1048576,
    JSON.stringify(savedCustomContext));
  await clickRowAction(page, 'glm-5.3-flash', '编辑');
  await sleep(300);
  const editContextEcho = await page.evaluate(() => {
    const field = document.querySelector('[data-testid="model-form-context-window"]');
    return { visible: !!field, value: field ? field.value : null };
  });
  rec('⑥.5h edit model echoes the saved context window', editContextEcho.visible && editContextEcho.value === '1048576', JSON.stringify(editContextEcho));
  await page.evaluate(() => {
    const field = document.querySelector('[data-testid="model-form-context-window"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(field, '');
    field.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await sleep(150);
  await clickExact(page, '保存');
  await sleep(500);
  const clearedContext = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return call && call.args && call.args.model;
  });
  rec('⑥.5i clearing the context window saves null (system default kept, no 0/garbage value)',
    clearedContext && clearedContext.model === 'glm-5.3-flash' && clearedContext.context_window_tokens === null,
    JSON.stringify(clearedContext));
  // Input bound: context_window_tokens is persisted as u32, so oversized input
  // must be truncated to 9 digits; otherwise save_model fails with a raw serde
  // deserialization error (or Infinity silently saves null).
  await clickRowAction(page, 'glm-5.3-flash', '编辑');
  await sleep(300);
  await page.evaluate(() => {
    const field = document.querySelector('[data-testid="model-form-context-window"]');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(field, '9999999999999');
    field.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await sleep(150);
  const overflowTruncated = await page.evaluate(() =>
    document.querySelector('[data-testid="model-form-context-window"]').value);
  rec('⑥.5j oversized numeric input is truncated to 9 digits (within the u32 persist bound)', overflowTruncated === '999999999', JSON.stringify({ overflowTruncated }));
  await clickExact(page, '取消');
  await sleep(200);

  await clickExact(page, '添加模型');
  await sleep(300);
  const cloudPickerWidth = await modalWidth(page, '添加模型');
  await clickExact(page, '深度求索 / DeepSeek');
  await sleep(300);
  const addModelBeforeKey = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const text = root ? root.innerText : '';
    const save = [...document.querySelectorAll('button')].reverse().find(button => (button.textContent || '').trim() === '保存');
    return {
      noAdvancedCollapse: !text.includes('高级设置'),
      noServiceUrlField: !text.includes('服务地址'),
      hasModelPicker: text.includes('模型') && text.includes('deepseek-v4-pro'),
      hasOptionalAlias: text.includes('别名') && !!document.querySelector('[data-testid="model-form-alias"]'),
      saveDisabled: !!save && save.disabled,
      hasSingleKeyInput: document.querySelectorAll('input[placeholder="输入 API Key"]').length === 1,
    };
  });
  const addModelBeforeKeyPass = cloudPickerWidth >= 430 && cloudPickerWidth <= 455
    && addModelBeforeKey.noAdvancedCollapse
    && addModelBeforeKey.noServiceUrlField
    && addModelBeforeKey.hasModelPicker
    && addModelBeforeKey.hasOptionalAlias
    && addModelBeforeKey.saveDisabled
    && addModelBeforeKey.hasSingleKeyInput;
  rec('⑥.6 添加预置云模型表单精简且 API Key 前禁用保存', addModelBeforeKeyPass, JSON.stringify({ cloudPickerWidth, ...addModelBeforeKey }));
  const apiInput = await page.$('input[placeholder="输入 API Key"]');
  const aliasInput = await page.$('[data-testid="model-form-alias"]');
  await aliasInput.type('Daily assistant');
  await apiInput.type('sk-model-test');
  await sleep(150);
  await clickExact(page, '保存');
  await sleep(500);
  const savedModel = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return call && call.args && call.args.model;
  });
  rec('⑦ 输入 API Key 后可保存新增模型',
    savedModel
      && savedModel.model === 'deepseek-v4-pro'
      && savedModel.name === 'deepseek-v4-pro'
      && savedModel.alias === 'Daily assistant'
      && savedModel.api_key === 'sk-model-test',
    JSON.stringify(savedModel));
  const savedAliasInventory = await page.evaluate(() => {
    const title = [...document.querySelectorAll('span')]
      .find(node => (node.textContent || '').trim() === 'Daily assistant');
    const row = title && title.closest('.grid');
    const text = row ? row.innerText : '';
    return {
      aliasVisible: !!title,
      providerVisible: text.includes('DeepSeek'),
      wireModelVisible: text.includes('deepseek-v4-pro'),
    };
  });
  rec('⑦.1 保存别名后模型列表立即以别名为标题并保留服务商与模型 ID',
    Object.values(savedAliasInventory).every(Boolean),
    JSON.stringify(savedAliasInventory));

  // ⑦.img 图片输入能力/视觉模型控件:渲染默认值、排除自身、保存往返。
  await clickRowAction(page, 'deepseek-v4-pro', '编辑');
  await sleep(300);
  const imageSectionDefault = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const text = root ? root.innerText : '';
    const capabilityToggle = root && root.querySelector('[data-testid="image-capability-toggle"]');
    const visionToggle = root && root.querySelector('[data-testid="vision-model-toggle"]');
    return {
      hasCapabilityRow: !!capabilityToggle && (capabilityToggle.textContent || '').includes('自动处理'),
      hasVisionRow: !!visionToggle && (visionToggle.textContent || '').includes('无'),
      hasHelpText: text.includes('当前模型不能看图时，用该模型分析图片'),
      // §11.8/§11.9 静态隐私说明:云端外发/本地不离机。
      hasPrivacyText: text.includes('使用云端模型时，图片会发送给你选择的模型服务商') && text.includes('本地模型图片不离开本机'),
    };
  });
  rec('⑦.img.1 编辑模型展示图片输入能力/视觉模型控件、默认自动处理/无及静态隐私说明', Object.values(imageSectionDefault).every(Boolean), JSON.stringify(imageSectionDefault));
  // 图片能力三档:自动处理/支持图片/不支持图片(「保存时检测」档已下线)。
  await page.click('[data-testid="image-capability-toggle"]');
  await sleep(200);
  const imageCapabilityOptions3 = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    return root ? [...root.querySelectorAll('[data-testid^="image-capability-option-"]')].map(node => node.getAttribute('data-testid')) : [];
  });
  await page.click('[data-testid="image-capability-toggle"]');
  await sleep(200);
  rec('⑦.img.1b 图片能力三档齐全(pinvou/enabled/disabled),不再提供保存时检测',
    imageCapabilityOptions3.includes('image-capability-option-pinvou')
      && imageCapabilityOptions3.includes('image-capability-option-enabled')
      && imageCapabilityOptions3.includes('image-capability-option-disabled')
      && !imageCapabilityOptions3.includes('image-capability-option-auto'),
    JSON.stringify(imageCapabilityOptions3));
  await page.click('[data-testid="image-capability-toggle"]');
  await sleep(200);
  await page.click('[data-testid="image-capability-option-enabled"]');
  await sleep(200);
  await page.click('[data-testid="vision-model-toggle"]');
  await sleep(200);
  const visionOptions = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const options = root ? [...root.querySelectorAll('[data-testid^="vision-model-option-"]')].map(node => node.getAttribute('data-testid')) : [];
    const editing = window.__SETTINGS_TEST__.models().find(model => model.model === 'deepseek-v4-pro');
    return {
      hasNone: options.includes('vision-model-option-none'),
      hasLocalQwen: options.includes('vision-model-option-local-qwen'),
      excludesSelf: !!editing && !options.includes(`vision-model-option-${editing.id}`),
    };
  });
  rec('⑦.img.2 视觉模型下拉含「无」与其他模型且排除当前模型自身', Object.values(visionOptions).every(Boolean), JSON.stringify(visionOptions));
  // 视觉模型候选不做 disabled 过滤:disabled 可能是历史探测误判残留
  // (如 kimi-for-coding 曾因探测链路 400 被回填),应由选择时的识图探测
  // 验证(supported 才可选),而不是提前隐藏。
  // mock 修改后必须走 TauriBridge.loadModels() 刷新 bridge state,React 才会
  // 以新 savedModels 重渲染弹窗的视觉候选。
  const toggleVision = async () => { await page.click('[data-testid="vision-model-toggle"]'); await sleep(150); };
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.setModelImageCapability('local-qwen', 'disabled');
    return window.TauriBridge.models.loadModels();
  });
  await toggleVision(); // 关闭再打开,按新候选渲染
  await toggleVision();
  const visionWithDisabled = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    return root ? [...root.querySelectorAll('[data-testid^="vision-model-option-"]')].map(node => node.getAttribute('data-testid')) : [];
  });
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.setModelImageCapability('local-qwen', 'auto');
    return window.TauriBridge.models.loadModels();
  });
  await toggleVision(); await toggleVision();
  const visionAfterRestore = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    return root ? [...root.querySelectorAll('[data-testid^="vision-model-option-"]')].map(node => node.getAttribute('data-testid')) : [];
  });
  rec('⑦.img.2b 视觉模型候选不因 disabled 标记隐藏(由选择探测验证)',
    visionWithDisabled.includes('vision-model-option-local-qwen')
      && visionAfterRestore.includes('vision-model-option-local-qwen'),
    JSON.stringify({ visionWithDisabled, visionAfterRestore }));
  // ⑦.img.2c/2d 视觉模型选择探测:点击不收起列表,该行右侧显示忙转圈
  // 「正在检测识图能力」;探测未通过拒绝选择并提示排查,通过后按结果收起列表。
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.setImageTestResponse({ status: 'unsupported', verified: false, summary: 'this model does not support image input', http_status: 400 });
    window.__SETTINGS_TEST__.setImageTestDelay(400);
  });
  await page.click('[data-testid="vision-model-option-local-qwen"]');
  await sleep(150);
  const visionProbing = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const probing = root && root.querySelector('[data-testid="vision-model-probing"]');
    return {
      probingShown: !!(probing && (probing.textContent || '').includes('正在测试图片能力')),
      spinning: !!(probing && probing.querySelector('.animate-spin')),
      listStillOpen: !!root.querySelector('[data-testid="vision-model-option-local-qwen"]'),
    };
  });
  rec('⑦.img.2c 视觉模型探测中:列表不收起,该行右侧显示忙转圈', Object.values(visionProbing).every(Boolean), JSON.stringify(visionProbing));
  await sleep(500);
  const visionRejected = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const toggle = root && root.querySelector('[data-testid="vision-model-toggle"]');
    const error = root && root.querySelector('[data-testid="vision-model-probe-error"]');
    return {
      stillNone: !!(toggle && (toggle.textContent || '').includes('无')),
      errorShown: !!(error && (error.textContent || '').includes('无法作为视觉模型')),
      probingCleared: !root.querySelector('[data-testid="vision-model-probing"]'),
      listStillOpen: !!root.querySelector('[data-testid="vision-model-option-local-qwen"]'),
    };
  });
  rec('⑦.img.2d 视觉模型探测未通过:拒绝选择、提示排查且列表保持展开', Object.values(visionRejected).every(Boolean), JSON.stringify(visionRejected));
  await page.evaluate(() => {
    window.__SETTINGS_TEST__.setImageTestResponse({ status: 'supported', verified: true, summary: '红色', http_status: 200 });
    window.__SETTINGS_TEST__.setImageTestDelay(0);
  });
  await page.click('[data-testid="vision-model-option-local-qwen"]');
  await sleep(400);
  const visionAccepted = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const toggle = root && root.querySelector('[data-testid="vision-model-toggle"]');
    return {
      selected: !!(toggle && (toggle.textContent || '').includes('本地 vLLM')),
      listClosed: !root.querySelector('[data-testid="vision-model-option-local-qwen"]'),
    };
  });
  rec('⑦.img.2e 视觉模型探测通过:按结果收起列表并选中', Object.values(visionAccepted).every(Boolean), JSON.stringify(visionAccepted));
  await clickExact(page, '保存');
  await sleep(500);
  const savedImageConfig = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return call && call.args && call.args.model;
  });
  rec('⑦.img.3 保存写入图片能力 override 与视觉模型引用',
    savedImageConfig
      && savedImageConfig.image_capability_override === 'enabled'
      && savedImageConfig.vision_model_id === 'local-qwen',
    JSON.stringify(savedImageConfig && { override: savedImageConfig.image_capability_override, vision: savedImageConfig.vision_model_id }));
  await clickRowAction(page, 'deepseek-v4-pro', '编辑');
  await sleep(300);
  const imageSectionRoundTrip = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const capabilityToggle = root && root.querySelector('[data-testid="image-capability-toggle"]');
    const visionToggle = root && root.querySelector('[data-testid="vision-model-toggle"]');
    return {
      capabilityEcho: !!capabilityToggle && (capabilityToggle.textContent || '').includes('支持图片'),
      visionEcho: !!visionToggle && (visionToggle.textContent || '').includes('本地 vLLM'),
    };
  });
  rec('⑦.img.4 重新打开编辑表单回显已保存的图片能力与视觉模型', Object.values(imageSectionRoundTrip).every(Boolean), JSON.stringify(imageSectionRoundTrip));
  await page.click('[data-testid="vision-model-toggle"]');
  await sleep(200);
  await page.click('[data-testid="vision-model-option-none"]');
  await sleep(200);
  await clickExact(page, '保存');
  await sleep(500);
  const clearedVision = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return call && call.args && call.args.model;
  });
  rec('⑦.img.5 视觉模型选回「无」保存为 null 且能力 override 保留',
    clearedVision
      && clearedVision.vision_model_id === null
      && clearedVision.image_capability_override === 'enabled',
    JSON.stringify(clearedVision && { override: clearedVision.image_capability_override, vision: clearedVision.vision_model_id }));

  // ⑦.img.6-11 测试图片能力(设计 §7.3):按钮渲染、supported/unsupported/error 分态、表单变更清除结果。
  await clickRowAction(page, 'deepseek-v4-pro', '编辑');
  await sleep(300);
  const imageTestInitial = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const button = root && root.querySelector('[data-testid="image-capability-test"]');
    const result = root && root.querySelector('[data-testid="image-capability-test-result"]');
    return {
      hasButton: !!button && (button.textContent || '').includes('测试图片能力'),
      buttonEnabled: !!button && !button.disabled,
      hintShown: !!result && (result.textContent || '').includes('纯色测试图'),
    };
  });
  rec('⑦.img.6 编辑模型展示「测试图片能力」按钮且默认显示提示文案', Object.values(imageTestInitial).every(Boolean), JSON.stringify(imageTestInitial));
  await page.click('[data-testid="image-capability-test"]');
  await sleep(300);
  const imageTestSupported = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const result = root && root.querySelector('[data-testid="image-capability-test-result"]');
    const text = result ? (result.textContent || '') : '';
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'test_image_input_capability');
    return {
      text,
      args: call && call.args,
      showsSupported: text.includes('支持图片'),
      showsReply: text.includes('模型回复：红色'),
      // 当前档位已是「支持图片」(⑦.img.3 保存),不应再提示设置。
      noEnableHint: !text.includes('可在上方将图片输入能力设为'),
    };
  });
  rec('⑦.img.7 supported 结果展示模型回复摘要且按当前表单值发起测试',
    imageTestSupported.showsSupported
      && imageTestSupported.showsReply
      && imageTestSupported.noEnableHint
      && imageTestSupported.args
      && imageTestSupported.args.model === 'deepseek-v4-pro'
      && imageTestSupported.args.baseUrl === 'https://api.deepseek.com'
      && imageTestSupported.args.apiKey === '',
    JSON.stringify(imageTestSupported));
  // 档位切回「自动处理」,supported 结果应提示可设为「支持图片」。
  await page.click('[data-testid="image-capability-toggle"]');
  await sleep(200);
  await page.click('[data-testid="image-capability-option-pinvou"]');
  await sleep(200);
  await page.click('[data-testid="image-capability-test"]');
  await sleep(300);
  const imageTestAutoHint = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const result = root && root.querySelector('[data-testid="image-capability-test-result"]');
    const text = result ? (result.textContent || '') : '';
    return { text, showsEnableHint: text.includes('可在上方将图片输入能力设为') };
  });
  rec('⑦.img.8 档位为自动处理时 supported 结果提示可设为「支持图片」', imageTestAutoHint.showsEnableHint, imageTestAutoHint.text);
  await page.evaluate(() => window.__SETTINGS_TEST__.setImageTestResponse({ status: 'unsupported', verified: false, summary: 'this model does not support image input', http_status: 400 }));
  await page.click('[data-testid="image-capability-test"]');
  await sleep(300);
  const imageTestUnsupported = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const result = root && root.querySelector('[data-testid="image-capability-test-result"]');
    const text = result ? (result.textContent || '') : '';
    return {
      text,
      showsUnsupported: text.includes('不支持图像识别'),
      showsProvider: text.includes('does not support image input'),
    };
  });
  rec('⑦.img.9 unsupported 结果展示 provider 错误摘要', imageTestUnsupported.showsUnsupported && imageTestUnsupported.showsProvider, imageTestUnsupported.text);
  // 审阅缺口 #104:未识别出测试色(2xx 无关回复 / 400 非图片拒绝)统一显示
  // 「未能正确识别图像，原因未知」,不宣称支持也不宣称不支持。
  await page.evaluate(() => window.__SETTINGS_TEST__.setImageTestResponse({ status: 'unverified', verified: false, summary: '未能正确识别图像，原因未知（模型回复：一张正方形图片）', http_status: 200 }));
  await page.click('[data-testid="image-capability-test"]');
  await sleep(300);
  const imageTestUnverified = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const result = root && root.querySelector('[data-testid="image-capability-test-result"]');
    const text = result ? (result.textContent || '') : '';
    return {
      text,
      showsUnverified: text.includes('未能正确识别图像，原因未知'),
      showsProvider: text.includes('正方形图片'),
      noEnableHint: !text.includes('可在上方将图片输入能力设为'),
      notClaimingUnsupported: !text.includes('不支持图像识别'),
    };
  });
  rec('⑦.img.9b 未识别态:原因未知,不宣称支持或不支持,展示摘要且不提示设档', Object.values(imageTestUnverified).every(Boolean), imageTestUnverified.text);
  await page.evaluate(() => window.__SETTINGS_TEST__.setImageTestResponse({ status: 'error', verified: false, summary: '连接超时', http_status: null }));
  await page.click('[data-testid="image-capability-test"]');
  await sleep(300);
  const imageTestError = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const result = root && root.querySelector('[data-testid="image-capability-test-result"]');
    const text = result ? (result.textContent || '') : '';
    return { text, showsError: text.includes('测试失败') && !text.includes('不支持图像识别') && !text.includes('支持图片') };
  });
  rec('⑦.img.10 error 结果与「不支持」严格区分', imageTestError.showsError, imageTestError.text);
  // 表单值变化后上一次测试结果应清除(恢复提示文案)。已存 Key 的模型占位符是掩码,按类型选择。
  const imageTestKeyInput = await page.$('[data-testid="model-form-dialog"] input[type="password"]');
  await imageTestKeyInput.type('k');
  await sleep(200);
  const imageTestCleared = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const result = root && root.querySelector('[data-testid="image-capability-test-result"]');
    const text = result ? (result.textContent || '') : '';
    return { text, backToHint: text.includes('纯色测试图') };
  });
  rec('⑦.img.11 表单值变化后清除上一次测试结果', imageTestCleared.backToHint, imageTestCleared.text);
  await clickExact(page, '取消');
  await sleep(200);

  // ⑦.img.12 保存不再自动检测:任意档位保存都不带探测参数、直接落盘关闭。
  const echoOverride = async () => {
    await clickRowAction(page, 'deepseek-v4-pro', '编辑');
    await sleep(300);
    const state = await page.evaluate(() => {
      const root = document.querySelector('[data-testid="model-form-dialog"]');
      const toggle = root && root.querySelector('[data-testid="image-capability-toggle"]');
      return toggle ? (toggle.textContent || '') : '';
    });
    await clickExact(page, '取消');
    await sleep(200);
    return state;
  };
  await clickRowAction(page, 'deepseek-v4-pro', '编辑');
  await sleep(300);
  await page.click('[data-testid="image-capability-toggle"]');
  await sleep(200);
  await page.click('[data-testid="image-capability-option-pinvou"]');
  await sleep(200);
  await page.click('[data-testid="model-form-save"]');
  await sleep(400);
  const savedPinvou = await page.evaluate(() => {
    const saveCall = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_model');
    return {
      dialogClosed: !document.querySelector('[data-testid="model-form-dialog"]'),
      noProbeArg: !(saveCall && saveCall.args && saveCall.args.probeImageCapability),
      savedWithPinvou: !!(saveCall && saveCall.args && saveCall.args.model.image_capability_override === 'pinvou'),
    };
  });
  rec('⑦.img.12 「自动处理」档保存:不探测、直接落盘关闭',
    savedPinvou.dialogClosed && savedPinvou.noProbeArg && savedPinvou.savedWithPinvou,
    JSON.stringify(savedPinvou));
  const echoPinvou = await echoOverride();
  rec('⑦.img.12b 重开表单回显「自动处理」', echoPinvou.includes('自动处理'), echoPinvou);

  // 存量「保存时检测」(auto)档残留:重开表单按「自动处理」回显。生产链路里
  // "auto" 由 Rust serde 迁移为 pinvou 后前端才收到,此处直灌 auto 只测前端
  // 防御层(serde 迁移另有 settings 单测覆盖);mock 改档后必须 loadModels()
  // 刷新 bridge state,React 才会以新 savedModels 渲染(同 ⑦.img.2b)。
  await page.evaluate(() => window.__SETTINGS_TEST__.setModelImageCapability(
    window.__SETTINGS_TEST__.models().find(model => model.model === 'deepseek-v4-pro').id, 'auto'));
  await page.evaluate(() => window.TauriBridge.models.loadModels());
  await sleep(200);
  const echoLegacyAuto = await echoOverride();
  rec('⑦.img.12c 存量 auto 档残留按「自动处理」回显',
    echoLegacyAuto.includes('自动处理') && !echoLegacyAuto.includes('保存时检测'),
    echoLegacyAuto);

  // 保存失败(连接/写盘错误):弹窗保持 + 行内错误提示,表单输入不丢弃;
  // 修正后重试成功正常关闭(不再依赖不存在的"调用链处理")。
  await clickRowAction(page, 'deepseek-v4-pro', '编辑');
  await sleep(300);
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelError('磁盘写入失败'));
  await page.click('[data-testid="model-form-save"]');
  await sleep(400);
  const saveFailed = await page.evaluate(() => {
    const root = document.querySelector('[data-testid="model-form-dialog"]');
    const error = root && root.querySelector('[data-testid="model-form-save-error"]');
    return {
      dialogOpen: !!root,
      errorShown: !!(error && (error.textContent || '').includes('磁盘写入失败')),
    };
  });
  rec('⑦.img.13 保存失败:弹窗保持并显示行内错误',
    saveFailed.dialogOpen && saveFailed.errorShown,
    JSON.stringify(saveFailed));
  await page.evaluate(() => window.__SETTINGS_TEST__.setSaveModelError(null));
  await page.click('[data-testid="model-form-save"]');
  await sleep(400);
  const retrySaved = await page.evaluate(() => !document.querySelector('[data-testid="model-form-dialog"]'));
  rec('⑦.img.13b 修正后重试保存成功关闭弹窗', retrySaved, String(retrySaved));

  // ⑦.img.14 目录视觉能力标注自动填写「图片输入能力」:Kimi 系已标注,组默认
  // 首项 k3 即预填「支持图片」;DeepSeek 组未标注保持「自动处理」;手动改档后
  // 再换条目不再跟随。
  const capabilityToggleText = () => page.evaluate(() => {
    const toggle = document.querySelector('[data-testid="model-form-dialog"] [data-testid="image-capability-toggle"]');
    return toggle ? (toggle.textContent || '') : '';
  });
  await clickExact(page, '添加模型');
  await sleep(250);
  await clickExact(page, 'Kimi Coding Plan');
  await sleep(300);
  const capGroupDefault = await capabilityToggleText();
  await clickExact(page, '取消');
  await sleep(200);
  await clickExact(page, '添加模型');
  await sleep(250);
  await clickExact(page, '深度求索 / DeepSeek');
  await sleep(300);
  const capUnannotated = await capabilityToggleText();
  await page.click('[data-testid="image-capability-toggle"]');
  await sleep(200);
  await page.click('[data-testid="image-capability-option-disabled"]');
  await sleep(200);
  await clickModalExact(page, '模型');
  await sleep(250);
  await clickModalExact(page, 'deepseek-v4-flash');
  await sleep(250);
  const capAfterTouched = await capabilityToggleText();
  rec('⑦.img.14 目录视觉能力标注自动填写,手动改档后不再跟随',
    capGroupDefault.includes('支持图片')
      && capUnannotated.includes('自动处理')
      && capAfterTouched.includes('不支持图片'),
    JSON.stringify({ capGroupDefault, capUnannotated, capAfterTouched }));
  await clickExact(page, '取消');
  await sleep(200);

  await clickSettingsSection(page, '搜索');
  const searchList = await page.evaluate(() => {
    const text = document.body.innerText;
    return {
      hasBing: text.includes('Bing'),
      hasMetaso: text.includes('秘塔'),
      noStatusNoise: !text.includes('已配置') && !text.includes('未配置'),
      noStandaloneSave: !text.includes('保存搜索配置'),
      addInGroup: text.includes('添加搜索源'),
    };
  });
  rec('⑧ 搜索列表按模型页交互精简', Object.values(searchList).every(Boolean), JSON.stringify(searchList));

  await clickRowAction(page, '秘塔', '编辑');
  await sleep(250);
  const existingSearchNoChange = await page.evaluate(() => {
    const text = document.body.innerText;
    const save = [...document.querySelectorAll('button')].reverse().find(button => (button.textContent || '').trim() === '保存');
    return {
      dialog: text.includes('编辑搜索源') && text.includes('秘塔'),
      masked: !!document.querySelector('input[placeholder="••••••••"]'),
      saveDisabled: !!save && save.disabled,
    };
  });
  rec('⑧.1 已有搜索源未输入新 Key 时保存置灰', Object.values(existingSearchNoChange).every(Boolean), JSON.stringify(existingSearchNoChange));
  await clickExact(page, '取消');
  await sleep(150);

  const modelsBeforeSearchSave = await page.evaluate(() => window.__SETTINGS_TEST__.models().map(model => model.id).sort()); // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic order of string arrays matches assertion expectations
  const savesBeforePick = await callCount(page, 'save_search_settings_and_restart');
  await clickExact(page, '添加搜索源');
  await sleep(250);
  const searchPickerWidth = await modalWidth(page, '添加搜索源');
  await clickExact(page, '博查');
  await sleep(300);
  const pickWithoutSave = await page.evaluate(() => ({
    restartDialog: document.body.innerText.includes('重启以应用搜索配置？'),
    saveButtonDisabled: [...document.querySelectorAll('button')].some(button => (button.textContent || '').trim() === '保存' && button.disabled),
    bochaInSavedSettings: (window.__SETTINGS_TEST__.settings().search.enabled_providers || []).includes('bocha'),
  }));
  const savesAfterPick = await callCount(page, 'save_search_settings_and_restart');
  rec('⑨ 选择搜索源但未点保存不会持久化', searchPickerWidth >= 430 && searchPickerWidth <= 455 && !pickWithoutSave.restartDialog && pickWithoutSave.saveButtonDisabled && !pickWithoutSave.bochaInSavedSettings && savesAfterPick === savesBeforePick, JSON.stringify({ searchPickerWidth, ...pickWithoutSave, savesBeforePick, savesAfterPick }));

  const searchKeyInput = await page.$('input[placeholder="输入 API Key"]');
  await searchKeyInput.click();
  await searchKeyInput.type('bocha-key');
  const typedValue = await page.evaluate(() => document.querySelector('input[placeholder="输入 API Key"]')?.value || '');
  rec('⑩ 搜索源 API Key 输入框可连续输入', typedValue === 'bocha-key', typedValue);
  await clickExact(page, '保存');
  await sleep(300);
  const restartPrompt = await page.evaluate(() => document.body.innerText.includes('重启以应用搜索配置？'));
  const savesBeforeRestart = await callCount(page, 'save_search_settings_and_restart');
  rec('⑪ 搜索源保存后先提示重启，未确认前不写盘重启', restartPrompt && savesBeforeRestart === savesBeforePick, String(savesBeforeRestart));
  await clickExact(page, '现在重启');
  await sleep(300);
  const searchSaved = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'save_search_settings_and_restart');
    const search = call && call.args && call.args.search;
    return search && {
      provider: search.provider,
      enabled: search.enabled_providers,
      bochaAction: search.credentials && search.credentials.bocha && search.credentials.bocha.credential_action,
      bochaKey: search.credentials && search.credentials.bocha && search.credentials.bocha.api_key,
    };
  });
  rec('⑫ 确认重启后写入搜索源和凭据草稿', searchSaved && searchSaved.provider === 'bocha' && searchSaved.enabled.includes('bocha') && searchSaved.bochaAction === 'replace' && searchSaved.bochaKey === 'bocha-key', JSON.stringify(searchSaved));

  await clickSettingsSection(page, '搜索');
  const modelsAfterSearchSave = await page.evaluate(() => window.__SETTINGS_TEST__.models().map(model => model.id).sort()); // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic order of string arrays matches assertion expectations
  rec('新增模型后保存搜索配置不会清空模型', JSON.stringify(modelsAfterSearchSave) === JSON.stringify(modelsBeforeSearchSave), JSON.stringify({ modelsBeforeSearchSave, modelsAfterSearchSave }));
  const savesBeforeDeleteLater = await callCount(page, 'update_search_settings');
  const restartsBeforeDeleteLater = await callCount(page, 'save_search_settings_and_restart');
  await clickRowAction(page, '秘塔', '删除');
  await sleep(250);
  const searchDeleteWidth = await modalWidth(page, '删除搜索源？');
  const searchDeleteDialog = await page.evaluate(() => document.body.innerText.includes('删除搜索源？') && document.body.innerText.includes('将移除 秘塔'));
  await clickExact(page, '删除搜索源');
  await sleep(250);
  await clickExact(page, '稍后');
  await sleep(300);
  const deleteLaterSaved = await page.evaluate(() => {
    const call = [...window.__SETTINGS_TEST__.calls].reverse().find(item => item.cmd === 'update_search_settings');
    const search = call && call.args && call.args.search;
    return search && {
      enabled: search.enabled_providers,
      metasoAction: search.credentials && search.credentials.metaso && search.credentials.metaso.credential_action,
    };
  });
  const savesAfterDeleteLater = await callCount(page, 'update_search_settings');
  const restartsAfterDeleteLater = await callCount(page, 'save_search_settings_and_restart');
  rec('⑬ 删除搜索源使用窄 iOS 确认框，选择稍后会写盘但不重启',
    searchDeleteDialog
      && searchDeleteWidth >= 260
      && searchDeleteWidth <= 285
      && savesAfterDeleteLater === savesBeforeDeleteLater + 1
      && restartsAfterDeleteLater === restartsBeforeDeleteLater
      && deleteLaterSaved
      && !deleteLaterSaved.enabled.includes('metaso')
      && deleteLaterSaved.metasoAction === 'delete',
    JSON.stringify({ width: searchDeleteWidth, savesBeforeDeleteLater, savesAfterDeleteLater, restartsBeforeDeleteLater, restartsAfterDeleteLater, deleteLaterSaved }));

  await page.evaluate(() => window.__SETTINGS_TEST__.setDependencyCheckResponse([
    { key: 'voice_asr_model', installed: false, apt: '', install_action: 'voice_asr_model' },
    { key: 'knowledge_embedding_model', installed: false, apt: '', install_action: 'knowledge_embedding_model' },
  ]));
  await clickSettingsSection(page, '权限与环境');
  await page.evaluate(() => document.querySelector('#settings-dependencies button')?.click());
  await sleep(300);
  const onDemandModels = await page.evaluate(() => {
    const section = document.querySelector('#settings-dependencies');
    const text = section?.textContent || '';
    const buttons = [...(section?.querySelectorAll('button') || [])];
    buttons.at(-1)?.click();
    return {
      voiceModelVisible: text.includes('SenseVoice q8'),
      knowledgeModelVisible: text.includes('bge-m3'),
      runtimeItemHidden: !text.includes('本地语音识别'),
      buttonCount: buttons.length,
    };
  });
  await sleep(300);
  const modelInstallCall = await page.evaluate(() => {
    const call = window.__SETTINGS_TEST__.calls.find(item => item.cmd === 'install_dependencies');
    return call && call.args;
  });
  rec('⑭.1 Windows 仅展示可修复模型并调用对应下载动作',
    onDemandModels.voiceModelVisible
      && onDemandModels.knowledgeModelVisible
      && onDemandModels.runtimeItemHidden
      && onDemandModels.buttonCount === 2
      && Array.isArray(modelInstallCall?.packages)
      && modelInstallCall.packages.length === 0
      && JSON.stringify(modelInstallCall.actions) === JSON.stringify(['voice_asr_model', 'knowledge_embedding_model']),
    JSON.stringify({ onDemandModels, modelInstallCall }));
  await page.evaluate(() => {
    const row = [...document.querySelectorAll('div')].find(node => (node.textContent || '').includes('高级执行权限') && node.querySelector('[role="switch"]'));
    const button = row && row.querySelector('[role="switch"]');
    if (button) button.click();
  });
  await sleep(600);
  const permToast = await page.evaluate(() => ({
    showSuperPermissionSettings: !!(window.TauriBridge.state.get('platform').platformCapabilities || {}).showSuperPermissionSettings,
    setCall: window.__SETTINGS_TEST__.calls.some(call => call.cmd === 'set_super_permission'),
    toast: document.body.innerText.includes('pkexec unavailable') || document.body.innerText.includes('无法开启高级执行权限'),
    checked: document.querySelector('[role="switch"]')?.getAttribute('aria-checked'),
    advancedPermissionVisible: document.body.innerText.includes('高级执行权限'),
    dependencyCheckVisible: document.body.innerText.includes('依赖体检'),
  }));
  const permissionPass = permToast.showSuperPermissionSettings
    ? permToast.setCall && permToast.toast && permToast.checked === 'false'
    : !permToast.advancedPermissionVisible && permToast.dependencyCheckVisible && !permToast.setCall;
  rec('⑭ 权限与环境按平台展示并保持失败回滚', permissionPass, JSON.stringify(permToast));

  await clickSettingsSection(page, '用户交流');
  await sleep(250);
  await page.evaluate(() => {
    window.__COMMUNITY_CLIPBOARD_TEXT__ = null;
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: {
        writeText: async (value) => { window.__COMMUNITY_CLIPBOARD_TEXT__ = value; },
      },
    });
  });
  await page.click('[data-testid="community-copy-group"]');
  await sleep(50);
  const communityPanel = await page.evaluate(() => {
    const panel = document.querySelector('[data-community-panel="true"]');
    const navIds = [...document.querySelectorAll('[data-testid="settings-nav"] [data-testid^="settings-section-"]')]
      .map(node => node.getAttribute('data-testid'));
    const rect = panel && panel.getBoundingClientRect();
    const qr = panel && panel.querySelector('[data-testid="community-qr-image"]');
    const groupName = panel && panel.querySelector('[data-testid="community-group-name"]');
    const groupNumber = panel && panel.querySelector('[data-testid="community-group-number"]');
    const copyButton = panel && panel.querySelector('[data-testid="community-copy-group"]');
    const text = panel ? panel.innerText : '';
    return {
      exists: !!panel,
      separateTab: !!document.querySelector('[data-testid="settings-section-community"]')
        && (document.querySelector('[data-testid="settings-content"] h1')?.textContent || '').trim() === '用户交流',
      immediatelyAfterHelp: navIds.indexOf('settings-section-community') === navIds.indexOf('settings-section-help') + 1,
      directTopLevel: !document.querySelector('[data-community-dialog="true"]') && !document.querySelector('[data-testid="community-entry-open"]'),
      width: rect ? Math.round(rect.width) : 0,
      insideViewport: !!rect && rect.left >= -1 && rect.right <= window.innerWidth + 1 && rect.top >= -1 && rect.bottom <= window.innerHeight + 1,
      hasQrImage: !!qr && qr.complete && qr.naturalWidth === 824 && qr.naturalHeight === 824 && qr.getBoundingClientRect().width >= 220,
      correctGroupName: (groupName?.textContent || '').trim() === 'pinvou-agent官方交流群',
      correctGroupNumber: (groupNumber?.textContent || '').trim() === '1108909346',
      copyEnabled: !!copyButton && !copyButton.disabled,
      copyWorks: window.__COMMUNITY_CLIPBOARD_TEXT__ === '1108909346' && text.includes('群号已复制'),
      hasDiscussionsFallback: text.includes('GitHub Discussions'),
      hasSupportBoundary: text.includes('不等同于官方支持或工单') && text.includes('安全漏洞'),
      noHorizontalOverflow: document.documentElement.scrollWidth <= window.innerWidth + 1 && (!panel || panel.scrollWidth <= panel.clientWidth + 1),
    };
  });
  rec('⑮ Community follows Help & Feedback and directly exposes the official group', communityPanel.exists && communityPanel.separateTab && communityPanel.immediatelyAfterHelp && communityPanel.directTopLevel && communityPanel.width >= 500 && communityPanel.width <= 700 && communityPanel.insideViewport && communityPanel.hasQrImage && communityPanel.correctGroupName && communityPanel.correctGroupNumber && communityPanel.copyEnabled && communityPanel.copyWorks && communityPanel.hasDiscussionsFallback && communityPanel.hasSupportBoundary && communityPanel.noHorizontalOverflow, JSON.stringify(communityPanel));
  await clickSettingsSection(page, '帮助反馈');
  await clickExact(page, '提交反馈');
  await sleep(250);
  const feedbackModal = await page.evaluate(() => {
    const title = [...document.querySelectorAll('h2')].find(node => (node.textContent || '').trim() === '我要反馈');
    const modal = title && title.closest('[data-feedback-dialog="true"]');
    const rect = modal && modal.getBoundingClientRect();
    const buttons = [...document.querySelectorAll('button')].map(button => (button.textContent || '').trim());
    const textarea = document.querySelector('textarea[placeholder*="请描述"]');
    const titleInput = document.querySelector('input[placeholder*="一句话概括"]');
    const groupCount = modal ? modal.querySelectorAll('.rounded-\\[16px\\]').length : 0;
    const nativeAlertCalls = window.__SETTINGS_TEST__.calls.filter(call => call.cmd === 'window_alert').length;
    const scrollWidth = modal ? modal.scrollWidth : 0;
    const clientWidth = modal ? modal.clientWidth : 0;
    return {
      exists: !!modal,
      width: rect ? Math.round(rect.width) : 0,
      insideViewport: !!rect && rect.left >= -1 && rect.right <= window.innerWidth + 1 && rect.top >= -1 && rect.bottom <= window.innerHeight + 1,
      compactTitle: !!title && Number.parseFloat(window.getComputedStyle(title).fontSize) <= 20,
      hasBottomActions: buttons.includes('取消') && buttons.includes('提交反馈'),
      hasGroupedInputs: !!textarea && !!titleInput,
      singleLayerGroups: groupCount === 3,
      noNativeAlertBeforeSubmit: nativeAlertCalls === 0,
      noHorizontalOverflow: document.documentElement.scrollWidth <= window.innerWidth + 1,
      modalNoHorizontalOverflow: scrollWidth <= clientWidth + 1,
    };
  });
  rec('⑯ 反馈弹窗符合模型/搜索一致的 iOS 规格', feedbackModal.exists && feedbackModal.width >= 420 && feedbackModal.width <= 455 && feedbackModal.insideViewport && feedbackModal.compactTitle && feedbackModal.hasBottomActions && feedbackModal.hasGroupedInputs && feedbackModal.singleLayerGroups && feedbackModal.noNativeAlertBeforeSubmit && feedbackModal.noHorizontalOverflow && feedbackModal.modalNoHorizontalOverflow, JSON.stringify(feedbackModal));
  await page.evaluate(() => {
    const textarea = document.querySelector('[data-feedback-dialog="true"] textarea[placeholder*="请描述"]');
    if (!textarea) return;
    const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
    textarea.focus();
    setter.call(textarea, '反馈弹窗测试');
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
  });
  const feedbackTyped = await page.evaluate(() => document.querySelector('[data-feedback-dialog="true"] textarea[placeholder*="请描述"]')?.value || '');
  await page.evaluate(() => {
    const modal = document.querySelector('[data-feedback-dialog="true"]');
    const button = modal && [...modal.querySelectorAll('button')].find(node => (node.textContent || '').trim() === '提交反馈');
    if (button) button.click();
  });
  await sleep(400);
  const feedbackSubmit = await page.evaluate(() => ({
    nativeAlertCalls: window.__SETTINGS_TEST__.calls.filter(call => call.cmd === 'window_alert').length,
    submitCalls: window.__SETTINGS_TEST__.calls.filter(call => call.cmd === 'submit_feedback').length,
    toast: document.body.innerText.includes('反馈已提交，感谢你的帮助。'),
    dialogClosed: !document.querySelector('[data-feedback-dialog="true"]'),
  }));
  rec('⑰ 提交反馈成功使用应用内 toast，不弹系统 alert', feedbackTyped === '反馈弹窗测试' && feedbackSubmit.nativeAlertCalls === 0 && feedbackSubmit.submitCalls === 1 && feedbackSubmit.toast && feedbackSubmit.dialogClosed, JSON.stringify({ feedbackTyped, ...feedbackSubmit }));
  await sleep(200);

  await page.setViewport({ width: 760, height: 620 });
  await sleep(350);
  await clickSettingsSection(page, '通用');
  const responsiveSettings = await page.evaluate(() => {
    const aside = document.querySelector('aside');
    const panel = aside && aside.parentElement;
    const label = [...document.querySelectorAll('div')].find(node => (node.textContent || '').trim() === '界面语言');
    const panelRect = panel && panel.getBoundingClientRect();
    const labelRect = label && label.getBoundingClientRect();
    return {
      panelInsideViewport: !!panelRect && panelRect.left >= -1 && panelRect.top >= -1 && panelRect.right <= window.innerWidth + 1 && panelRect.bottom <= window.innerHeight + 1,
      labelNotVertical: !!labelRect && labelRect.width >= 54 && labelRect.height <= 28,
      noHorizontalOverflow: document.documentElement.scrollWidth <= window.innerWidth + 1,
    };
  });
  rec('⑱ 小窗口设置弹窗随窗口收缩且标签不竖排', Object.values(responsiveSettings).every(Boolean), JSON.stringify(responsiveSettings));

  await page.mouse.click(8, 8);
  await page.waitForFunction(() => document.querySelector('[data-testid="app-root"]')?.getAttribute('data-current-view') === 'chat', { timeout: 8000 });
  await page.evaluate(() => window.TauriBridge && window.TauriBridge.sessions.createNewSession && window.TauriBridge.sessions.createNewSession());
  await sleep(600);
  const responsiveGreeting = await page.evaluate(() => {
    const greeting = [...document.querySelectorAll('h1')].find(node => {
      const text = node.textContent || '';
      const fontSize = Number.parseFloat(window.getComputedStyle(node).fontSize);
      return text.includes('今天想聊点什么') || text.includes("what's good") || text.includes('今日は') || fontSize >= 30;
    });
    const rect = greeting && greeting.getBoundingClientRect();
    const fontSize = greeting ? Number.parseFloat(window.getComputedStyle(greeting).fontSize) : 0;
    return {
      exists: !!greeting,
      insideViewport: !!rect && rect.left >= -1 && rect.right <= window.innerWidth + 1,
      smallWindowFont: fontSize > 0 && fontSize <= 34,
      noHorizontalOverflow: document.documentElement.scrollWidth <= window.innerWidth + 1,
    };
  });
  rec('⑲ 小窗口欢迎文案不溢出', Object.values(responsiveGreeting).every(Boolean), JSON.stringify(responsiveGreeting));

  rec('⑳ 全程无运行时报错', errors.length === 0, errors.slice(0, 3).join(' | '));
  await browser.close();

  const failed = results.filter(result => !result.pass).length;
  console.log(failed ? `\n❌ ${failed}/${results.length} FAILED` : `\n✅ ALL ${results.length} PASS`);
  process.exit(failed ? 1 : 0);
// eslint-disable-next-line unicorn/prefer-top-level-await -- smoke script keeps its existing async main() structure
})().catch(error => {
  console.error('FATAL', error.stack || error.message);
  process.exit(1);
});
