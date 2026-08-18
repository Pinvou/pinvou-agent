#!/usr/bin/env node
/**
 * ACP Provider（第三方中转）管理 UI smoke：加载真实 Vite dist，mock TauriBridge，
 * 点击真实 React UI。覆盖设置页 模型 →「ACP 管理」子页：标签页、新增/保存、
 * 一键切换、删除确认、导出警告、env 冲突警告、卸载确认弹窗。
 */
const fs = require('fs');
const os = require('os');
const path = require('path');
const assert = require('assert');
const { startUiTestServer } = require('./ui_test_server');

function loadPuppeteer() {
  try { return require('puppeteer-core'); } catch (_) { /* fall through */ }
  const npx = path.join(os.homedir(), '.npm', '_npx');
  if (fs.existsSync(npx)) {
    for (const directory of fs.readdirSync(npx)) {
      const candidate = path.join(npx, directory, 'node_modules', 'puppeteer-core');
      if (fs.existsSync(candidate)) {
        try { return require(candidate); } catch (_) { /* try next */ }
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
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-acp-providers-'));

function injectSource() {
  return `(function () {
    var handlers = Object.create(null);
    var calls = [];
    var settings = { theme: 'liquid-light', language: 'zh-Hans', memory_enabled: false, notifications: { enabled: true, task_completed: true }, pet: { enabled: false }, search: { provider: 'bing', enabled_providers: ['bing'], api_key: null, credentials: {} }, advanced: {} };
    // ACP Provider mock 状态：codex 预置两个 Provider（一个带 key、一个不带）
    var providersByAgent = {
      codex: {
        currentProviderId: null,
        effectiveEntries: [
          { key: 'model_provider', value: 'pv-000000000001' },
          { key: 'model', value: 'gpt-5.2' },
          { key: 'model_providers.pv-000000000001.base_url', value: 'https://api.example.com/v1' },
        ],
        providers: [
          { id: 'pv-000000000001', name: '我的中转', baseUrl: 'https://api.example.com/v1', model: 'gpt-5.2', wireApi: 'openai', hasCredential: true, created_at: '' },
          { id: 'pv-000000000002', name: '无 Key 中转', baseUrl: 'https://api.other.com/v1', model: null, wireApi: 'anthropic', hasCredential: false, created_at: '' },
        ],
      },
      claude: { currentProviderId: null, providers: [] },
      kimi: { currentProviderId: null, providers: [] },
    };
    var envConflicts = [];
    var envEffectiveEntries = [];
    var nextProviderSeq = 0;
    var statusByAgent = {
      codex: { agent_id: 'codex', agent_name: 'Codex', installed: true, authenticated: true, version: '0.144.6', update_available: false, update_required: false, install_action: 'none', bridge_ready: true, setup_hint: null },
      claude: { agent_id: 'claude', agent_name: 'Claude Code', installed: true, authenticated: true, version: '2.0.0', update_available: false, update_required: false, install_action: 'none', bridge_ready: true, setup_hint: null },
      kimi: { agent_id: 'kimi', agent_name: 'Kimi', installed: true, authenticated: true, version: '0.9.0', update_available: false, update_required: false, install_action: 'none', bridge_ready: true, setup_hint: null },
    };
    function invoke(cmd, args) {
      calls.push({ cmd: cmd, args: args || {} });
      switch (cmd) {
        case 'get_settings': return Promise.resolve(settings);
        case 'update_settings':
          settings = Object.assign({}, settings, args.patch || {});
          return Promise.resolve(settings);
        case 'save_settings_and_restart':
          settings = Object.assign({}, settings, args.patch || {});
          return Promise.resolve(null);
        case 'get_platform_capabilities': return Promise.resolve({
          os: 'windows',
          codexAcpSupported: true,
          showMegacubeSite: false,
          showSuperPermissionSettings: false,
          usesBundledDependencyInstaller: true,
          taskCompletionNotificationsDefault: true,
        });
        case 'get_effective_model_config': return Promise.resolve({ model: null, baseUrl: null, preset: null, api_key_set: false });
        case 'list_acp_providers': {
          var agentState = providersByAgent[args.agent] || { currentProviderId: null, providers: [] };
          return Promise.resolve(Object.assign({
            envConflicts: envConflicts,
            envEffectiveEntries: envEffectiveEntries,
            officialActive: !agentState.currentProviderId,
            externalActive: false,
            configUnreadable: false,
            effectiveEntries: [],
          }, agentState));
        }
        case 'get_acp_agent_status': return Promise.resolve(statusByAgent[args.agentId] || null);
        case 'save_acp_provider': {
          var state = providersByAgent[args.agent] || (providersByAgent[args.agent] = { currentProviderId: null, providers: [] });
          // 新建按计数器分配唯一 id：固定 id 会让多次新增产生重复卡片/React key
          var record = { id: args.providerId || ('pv-0000000000' + String(9 + nextProviderSeq++)), name: args.name, baseUrl: args.baseUrl, model: args.model, wireApi: args.wireApi, hasCredential: args.apiKeyAction !== 'delete' && (!!args.apiKey || state.providers.some(function (p) { return p.id === args.providerId && p.hasCredential; })), created_at: '' };
          if (args.providerId) {
            var index = state.providers.findIndex(function (p) { return p.id === args.providerId; });
            if (index >= 0) state.providers[index] = record;
          } else {
            state.providers.push(record);
          }
          return Promise.resolve(record);
        }
        case 'switch_acp_provider':
          providersByAgent[args.agent].currentProviderId = args.providerId;
          return Promise.resolve(statusByAgent[args.agent]);
        case 'switch_acp_provider_official':
          providersByAgent[args.agent].currentProviderId = null;
          return Promise.resolve(statusByAgent[args.agent]);
        case 'delete_acp_provider': {
          var state = providersByAgent[args.agent];
          var index = state.providers.findIndex(function (p) { return p.id === args.providerId; });
          if (index >= 0) state.providers.splice(index, 1);
          if (state.currentProviderId === args.providerId) state.currentProviderId = null;
          return Promise.resolve(statusByAgent[args.agent]);
        }
        case 'export_acp_providers':
          return Promise.resolve(JSON.stringify(providersByAgent[args.agent].providers.map(function (p) { return Object.assign({}, p, { api_key: 'test-api-key-exported' }); }), null, 2));
        case 'import_acp_providers':
          return Promise.resolve({ imported: 1, idConflicts: 0, skipped: 0 });
        // 一次性模型探针：smoke 不验证探针内容，返回 null（前端静默回退占位快照）
        case 'probe_acp_agent_models': return Promise.resolve(null);
        case 'install_acp_agent': return Promise.resolve(statusByAgent[args.agent]);
        case 'cancel_acp_agent_install': statusByAgent[args.agent].installing = false; return Promise.resolve(statusByAgent[args.agent]);
        case 'uninstall_acp_agent': statusByAgent[args.agent].installed = false; return Promise.resolve(statusByAgent[args.agent]);
        case 'login_acp_agent': return Promise.resolve(statusByAgent[args.agentId]);
        case 'get_acp_provider_key': return Promise.resolve('test-api-key-saved');
        case 'logout_acp_agent': statusByAgent[args.agent].authenticated = false; return Promise.resolve(statusByAgent[args.agent]);
        case 'open_acp_agent_login_url': return Promise.resolve(null);
        case 'submit_acp_agent_login_code': return Promise.resolve(statusByAgent[args.agentId]);
        case 'list_models': return Promise.resolve({ models: [], active_model_id: null });
        case 'check_for_update': return Promise.resolve({ available: false, current_version: '0.6.1', latest_version: '0.6.1' });
        case 'get_backend_status': return Promise.resolve({ online: true, ok: true, status: 'online' });
        case 'get_selected_pet': return Promise.resolve('lingling');
        case 'list_sessions': return Promise.resolve([]);
        case 'list_personas': return Promise.resolve([]);
        case 'get_super_permission_status': return Promise.resolve({ can_use: false });
        case 'get_mode_state': return Promise.resolve({ mode: 'yolo', plan_phase: 'none' });
        case 'get_active_persona': return Promise.resolve(null);
        case 'list_scheduled_tasks': return Promise.resolve([]);
        case 'list_workspace_files': return Promise.resolve([]);
        case 'check_dependencies': return Promise.resolve([]);
        case 'find_resumable_run': return Promise.resolve(null);
        case 'get_app_version': return Promise.resolve('0.7.3');
        case 'list_acp_agents': return Promise.resolve([
          { agent_id: 'codex', agent_name: 'Codex', installed: true, authenticated: true },
          { agent_id: 'claude', agent_name: 'Claude Code', installed: true, authenticated: true },
          { agent_id: 'kimi', agent_name: 'Kimi', installed: true, authenticated: true },
        ]);
        default: return Promise.resolve(null);
      }
    }
    window.__ACP_PROVIDERS_TEST__ = {
      calls: calls,
      setEnvConflicts: function (next, entries) { envConflicts = next; envEffectiveEntries = entries || []; },
      // 安装中测试钩子：安装进行中的真实形态是 CLI 尚不可用（installed=false），
      // 安装入口保持渲染且按钮呈禁用 + 进行中文案
      setInstalling: function (agent, installing) {
        var status = statusByAgent[agent];
        status.installing = installing;
        if (installing) status.installed = false;
      },
    };
    window.__TAURI__ = {
      core: { invoke: invoke },
      event: {
        listen: function (name, handler) {
          (handlers[name] || (handlers[name] = [])).push(handler);
          return Promise.resolve(function () {});
        },
        emit: function (name, payload) {
          return Promise.all((handlers[name] || []).slice().map(function (handler) {
            return handler({ payload: payload || {} });
          }));
        },
      },
      window: {
        getCurrentWindow: function () {
          return { minimize: function () {}, maximize: function () {}, close: function () {}, toggleMaximize: function () {}, isMaximized: function () { return Promise.resolve(false); }, onResized: function () { return Promise.resolve(function () {}); }, startDragging: function () {} };
        },
      },
      dialog: { open: function () { return Promise.resolve(null); } },
    };
  })();`;
}

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const callCount = (page, cmd) => page.evaluate(command =>
  window.__ACP_PROVIDERS_TEST__.calls.filter(call => call.cmd === command).length, cmd);

async function clickSettingsSection(page, label) {
  const ok = await page.evaluate(text => {
    const buttons = [...document.querySelectorAll('aside button')].filter(button => (button.textContent || '').trim() === text);
    const button = buttons[buttons.length - 1];
    if (!button) return false;
    button.click();
    return true;
  }, label);
  if (!ok) throw new Error('找不到设置分区: ' + label);
  await sleep(300);
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

  // 打开设置 → 模型分节 → 顶端切换到「ACP 管理」子页
  await page.evaluate(() => {
    const button = [...document.querySelectorAll('button[title="设置"]')].pop();
    if (button) button.click();
  });
  await sleep(600);
  await clickSettingsSection(page, '模型');
  await page.evaluate(() => document.querySelector('[data-testid="settings-model-tab-acp"]')?.click());
  await sleep(300);
  rec('① ACP 管理子页渲染', await page.evaluate(() => !!document.querySelector('[data-testid="acp-providers-section"]')));
  rec('② 三 Agent 标签页', await page.evaluate(() =>
    ['acp-agent-tab-codex', 'acp-agent-tab-claude', 'acp-agent-tab-kimi'].every(id => !!document.querySelector(`[data-testid="${id}"]`))
  ));

  // 预置 Provider 卡片：切换按钮仅对有 key 的展示
  rec('③ 卡片与 key 徽标', await page.evaluate(() => {
    const cards = document.querySelectorAll('[data-testid^="acp-provider-card-"]');
    if (cards.length !== 2) return false;
    const firstText = cards[0].textContent || '';
    return firstText.includes('已保存密钥');
  }));
  rec('④ 无 key Provider 切换被禁用', await page.evaluate(() => {
    const buttons = [...document.querySelectorAll('[data-testid^="acp-provider-switch-pv-"]')];
    return buttons.length === 2
      && buttons.filter(button => button.disabled).length === 1
      && buttons.filter(button => !button.disabled).length === 1;
  }));

  // 新增 Provider → 表单 → 保存
  await page.click('[data-testid="acp-provider-add"]');
  await sleep(250);
  rec('⑤ 新增弹窗', await page.evaluate(() => !!document.querySelector('[data-testid="acp-provider-form-dialog"]')));
  await page.type('[data-testid="acp-provider-name"]', '新中转');
  await page.type('[data-testid="acp-provider-base-url"]', 'https://relay.example.com/v1');
  await page.type('[data-testid="acp-provider-api-key"]', 'test-api-key-123');
  await page.click('[data-testid="acp-provider-form-save"]');
  await sleep(500);
  rec('⑥ 保存调用与卡片出现', (await callCount(page, 'save_acp_provider')) === 1 && await page.evaluate(() => {
    return [...document.querySelectorAll('[data-testid^="acp-provider-card-"]')].some(card => (card.textContent || '').includes('新中转'));
  }));

  // 新建空 key：自绘二级确认弹窗（Tauri WebView2 下系统 window.confirm 不弹）——
  // 点保存后必须出现确认弹窗且**不**发保存请求；取消后确认弹窗消失仍不保存；
  // 确认后以 apiKeyAction='keep' 保存成功（空 key 允许保存，之后补）。
  await page.click('[data-testid="acp-provider-add"]');
  await sleep(250);
  await page.type('[data-testid="acp-provider-name"]', '空 Key 中转');
  await page.type('[data-testid="acp-provider-base-url"]', 'https://relay-empty.example.com/v1');
  const savesBeforeEmpty = await callCount(page, 'save_acp_provider');
  await page.click('[data-testid="acp-provider-form-save"]');
  await sleep(300);
  rec('⑥.6 空 key 保存弹确认弹窗', await page.evaluate(() => {
    const el = document.querySelector('[data-testid="acp-provider-form-confirm"]');
    return !!el && (el.textContent || '').includes('未填写 API Key');
  }) && (await callCount(page, 'save_acp_provider')) === savesBeforeEmpty);
  await page.evaluate(() => {
    const modal = document.querySelector('[data-testid="acp-provider-form-confirm"]');
    const cancel = [...modal.querySelectorAll('button')].find(button => (button.textContent || '').trim() === '取消');
    cancel.click();
  });
  await sleep(200);
  rec('⑥.7 取消确认弹窗后未保存', await page.evaluate(() =>
    !document.querySelector('[data-testid="acp-provider-form-confirm"]')
  ) && (await callCount(page, 'save_acp_provider')) === savesBeforeEmpty);
  await page.click('[data-testid="acp-provider-form-save"]');
  await sleep(300);
  await page.click('[data-testid="acp-provider-form-confirm-ok"]');
  await sleep(500);
  rec('⑥.8 确认后以 keep 保存空 key', await page.evaluate(() => {
    const saves = window.__ACP_PROVIDERS_TEST__.calls.filter(call => call.cmd === 'save_acp_provider');
    const last = saves[saves.length - 1];
    return !!last && last.args.apiKeyAction === 'keep' && !last.args.apiKey;
  }));

  // 编辑已存密钥的 Provider 并清空 key = 删除密钥：必须弹 deleteKey 确认弹窗，
  // 确认后以 apiKeyAction='delete' 上送
  await page.click('[data-testid="acp-provider-edit-pv-000000000001"]');
  await sleep(400);
  await page.evaluate(() => {
    const input = document.querySelector('[data-testid="acp-provider-api-key"]');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, '');
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await page.click('[data-testid="acp-provider-form-save"]');
  await sleep(300);
  rec('⑥.9 清空已存 key 弹删除确认弹窗', await page.evaluate(() => {
    const el = document.querySelector('[data-testid="acp-provider-form-confirm"]');
    return !!el && (el.textContent || '').includes('删除已保存的密钥');
  }));
  await page.click('[data-testid="acp-provider-form-confirm-ok"]');
  await sleep(500);
  rec('⑥.10 确认后以 delete 上送', await page.evaluate(() => {
    const saves = window.__ACP_PROVIDERS_TEST__.calls.filter(call => call.cmd === 'save_acp_provider');
    const last = saves[saves.length - 1];
    return !!last && last.args.apiKeyAction === 'delete';
  }));

  // 编辑**无已存密钥**的 Provider + 空 key：与新建同语义，也必须弹
  // 「未填写 API Key」确认（此前静默 keep 直接保存，不弹确认）
  await page.click('[data-testid="acp-provider-edit-pv-000000000002"]');
  await sleep(400);
  const savesBeforeNoKeyEdit = await callCount(page, 'save_acp_provider');
  await page.click('[data-testid="acp-provider-form-save"]');
  await sleep(300);
  rec('⑥.11 编辑无密钥 Provider 空 key 弹确认弹窗', await page.evaluate(() => {
    const el = document.querySelector('[data-testid="acp-provider-form-confirm"]');
    return !!el && (el.textContent || '').includes('未填写 API Key');
  }) && (await callCount(page, 'save_acp_provider')) === savesBeforeNoKeyEdit);
  await page.click('[data-testid="acp-provider-form-confirm-ok"]');
  await sleep(500);
  rec('⑥.12 确认后以 keep 保存', await page.evaluate(() => {
    const saves = window.__ACP_PROVIDERS_TEST__.calls.filter(call => call.cmd === 'save_acp_provider');
    const last = saves[saves.length - 1];
    return !!last && last.args.providerId === 'pv-000000000002' && last.args.apiKeyAction === 'keep';
  }));

  // Claude 细化模型槽位：切到 claude 标签页 → 新增 → 五个槽位必填；输入主模型
  // 后槽位自动跟随填充；保存时 modelSlots 随参数上送
  await page.click('[data-testid="acp-agent-tab-claude"]');
  await sleep(400);
  await page.click('[data-testid="acp-provider-add"]');
  await sleep(300);
  rec('⑥.1 claude 槽位表单渲染', await page.evaluate(() =>
    ['opus', 'sonnet', 'haiku', 'fable', 'subagent'].every(
      slot => !!document.querySelector(`[data-testid="acp-provider-slot-${slot}"]`)
    )
  ));
  await page.type('[data-testid="acp-provider-model"]', 'deepseek-v4-flash');
  rec('⑥.2 主模型联动填充槽位', await page.evaluate(() =>
    ['opus', 'sonnet', 'haiku', 'fable', 'subagent'].every(
      slot => document.querySelector(`[data-testid="acp-provider-slot-${slot}"]`).value === 'deepseek-v4-flash'
    )
  ));
  // 必填校验：关闭重开表单，不填模型（槽位为空）时保存必须被前端拦截
  await page.click('[data-testid="acp-provider-form-cancel"]');
  await sleep(300);
  await page.click('[data-testid="acp-provider-add"]');
  await sleep(300);
  await page.type('[data-testid="acp-provider-name"]', 'claude 中转');
  await page.type('[data-testid="acp-provider-base-url"]', 'https://relay-claude.example.com');
  await page.type('[data-testid="acp-provider-api-key"]', 'test-api-key-456');
  const savesBefore = await callCount(page, 'save_acp_provider');
  await page.click('[data-testid="acp-provider-form-save"]');
  await sleep(300);
  rec('⑥.3 槽位缺失时阻止保存', await page.evaluate(() =>
    !!document.querySelector('[data-testid="acp-provider-form-error"]')
  ) && (await callCount(page, 'save_acp_provider')) === savesBefore);
  // 1M 变体按厂商归属：claude 选 Kimi Code 预设只出现 k3[1m]，不出现别家变体
  await page.click('[data-testid="acp-provider-preset"]');
  await sleep(200);
  await page.click('[data-testid="acp-provider-preset-option-kimi-code"]');
  await sleep(200);
  await page.click('[data-testid="acp-provider-model"]');
  await sleep(200);
  rec('⑥.4 1M 变体按厂商过滤（Kimi Code 仅 k3[1m]）', await page.evaluate(() => {
    const values = [...document.querySelectorAll('[data-testid^="acp-provider-model-option-"]')]
      .map(option => (option.textContent || '').trim());
    return values.includes('k3[1m]')
      && !values.some(value => /\[1M\]/.test(value))
      && !values.includes('deepseek-v4-flash[1m]');
  }));
  // 候选不做字符过滤：输入后下拉仍全量展示（仅匹配项排序提前），避免
  // 「只有匹配的几个模型可选」的误解
  const optionCountBefore = await page.evaluate(() =>
    document.querySelectorAll('[data-testid^="acp-provider-model-option-"]').length
  );
  await page.type('[data-testid="acp-provider-model"]', 'k3');
  await sleep(200);
  rec('⑥.5 输入后候选仍全量展示', await page.evaluate(() =>
    document.querySelectorAll('[data-testid^="acp-provider-model-option-"]').length
  ) === optionCountBefore && optionCountBefore > 1);
  await page.click('[data-testid="acp-provider-form-cancel"]');
  await sleep(300);
  await page.click('[data-testid="acp-agent-tab-codex"]');
  await sleep(400);

  // 一键切换
  await page.evaluate(() => {
    const button = [...document.querySelectorAll('[data-testid^="acp-provider-switch-"]')].find(candidate => !candidate.disabled);
    if (button) button.click();
  });
  await sleep(500);
  rec('⑦ 切换调用', (await callCount(page, 'switch_acp_provider')) === 1);
  rec('⑧ 当前徽标', await page.evaluate(() => (document.body.textContent || '').includes('当前')));

  // 切回官方：Provider 生效时按钮可见，点击调 switch_acp_provider_official
  rec('⑧.1 切回官方按钮可见', await page.evaluate(() => !!document.querySelector('[data-testid="acp-provider-switch-official"]')));
  await page.click('[data-testid="acp-provider-switch-official"]');
  await sleep(500);
  rec('⑧.2 切回官方调用', (await callCount(page, 'switch_acp_provider_official')) === 1);

  // 官方登出：codex 有登出按钮并调用 logout_acp_agent；kimi 也有（provider remove）
  rec('⑧.3 登出按钮可见', await page.evaluate(() => !!document.querySelector('[data-testid="acp-cli-logout"]')));
  await page.click('[data-testid="acp-cli-logout"]');
  await sleep(500);
  rec('⑧.4 登出调用', (await callCount(page, 'logout_acp_agent')) === 1);
  await page.click('[data-testid="acp-agent-tab-kimi"]');
  await sleep(400);
  rec('⑧.5 kimi 有登出按钮', await page.evaluate(() => !!document.querySelector('[data-testid="acp-cli-logout"]')));
  await page.click('[data-testid="acp-agent-tab-codex"]');
  await sleep(400);

  // env 冲突警告
  await page.evaluate(() => window.__ACP_PROVIDERS_TEST__.setEnvConflicts(
    ['ANTHROPIC_BASE_URL', 'ANTHROPIC_AUTH_TOKEN'],
    [
      { key: 'ANTHROPIC_BASE_URL', value: 'https://env-override.example.com', secret: false },
      { key: 'ANTHROPIC_AUTH_TOKEN', value: '', secret: true },
    ]
  ));
  await page.click('[data-testid="acp-provider-refresh"]');
  await sleep(500);
  rec('⑨ env 冲突警告条', await page.evaluate(() => !!document.querySelector('[data-testid="acp-providers-env-warning"]')));
  rec('⑨.1 生效中配置只读区渲染', await page.evaluate(() => {
    const el = document.querySelector('[data-testid="acp-providers-effective"]');
    if (!el) return false;
    const values = [...el.querySelectorAll('.font-mono')].map(span => (span.textContent || '').trim());
    return values.some(value => value === 'gpt-5.2') && values.some(value => value === 'https://api.example.com/v1');
  }));
  rec('⑨.2 env 覆盖时徽标降格', await page.evaluate(() => {
    const section = document.querySelector('[data-testid="acp-providers-section"]');
    return !!(section && (section.textContent || '').includes('被环境变量覆盖'));
  }));
  rec('⑨.3 env 生效值：非密明文 + 凭据掩码', await page.evaluate(() => {
    const el = document.querySelector('[data-testid="acp-providers-env-warning"]');
    if (!el) return false;
    // URL 明文可见；凭据只显示「已设置」掩码，值不得出现
    const values = [...el.querySelectorAll('.font-mono')].map(span => (span.textContent || '').trim());
    return values.some(value => value === 'https://env-override.example.com')
      && (el.textContent || '').includes('已设置（值已隐藏）');
  }));

  // 导出警告
  await page.click('[data-testid="acp-provider-export"]');
  await sleep(300);
  rec('⑩ 导出明文 key 警告', await page.evaluate(() => {
    const text = document.body.textContent || '';
    return !!document.querySelector('[data-testid="acp-provider-export-json"]') && text.includes('明文');
  }));
  await page.click('[data-testid="acp-provider-export-select"]');
  await sleep(200);
  // 关闭导出弹窗（点背景），避免遮挡后续交互
  await page.mouse.click(20, 20);
  await sleep(250);

  // 删除确认
  await page.evaluate(() => {
    const button = document.querySelector('[data-testid^="acp-provider-delete-"]');
    if (button) button.click();
  });
  await sleep(250);
  rec('⑪ 删除确认弹窗', await page.evaluate(() => (document.body.textContent || '').includes('删除 Provider？')));
  await page.click('[data-testid="acp-provider-delete-confirm"]');
  await sleep(500);
  rec('⑫ 删除调用', (await callCount(page, 'delete_acp_provider')) === 1);

  // 卸载确认（默认不勾选清理）
  await page.click('[data-testid="acp-cli-uninstall"]');
  await sleep(250);
  rec('⑬ 卸载确认弹窗', await page.evaluate(() => (document.body.textContent || '').includes('卸载')));
  const cleanupChecked = await page.evaluate(() => document.querySelector('[data-testid="acp-uninstall-cleanup"]')?.checked);
  rec('⑭ 清理默认不勾选', cleanupChecked === false);
  await page.click('[data-testid="acp-uninstall-cancel"]').catch(() => {});
  // 兜底关闭：点击取消按钮（文本）
  await page.evaluate(() => {
    const buttons = [...document.querySelectorAll('button')].filter(button => (button.textContent || '').trim() === '取消');
    const button = buttons[buttons.length - 1];
    if (button) button.click();
  });
  await sleep(200);

  // 安装中状态恢复与取消：installing=true 时安装按钮禁用并显示「正在安装」，
  // 取消按钮可见；点击后调用 cancel_acp_agent_install 并恢复可安装形态
  await page.evaluate(() => window.__ACP_PROVIDERS_TEST__.setInstalling('codex', true));
  // DOM click 而非坐标点击：Provider 较多时内容更高，CLI 区滚入视口后
  // 刷新按钮可能被吸顶通知条（3.2s 自动消失）遮住，坐标点击会打在通知条上
  await page.evaluate(() => document.querySelector('[data-testid="acp-provider-refresh"]').click());
  await sleep(500);
  rec('⑯ 安装中按钮禁用且显示进行中文案', await page.evaluate(() => {
    const button = document.querySelector('[data-testid="acp-cli-install-update"]');
    return !!button && button.disabled && (button.textContent || '').includes('正在安装');
  }));
  rec('⑰ 取消安装按钮可见', await page.evaluate(() => !!document.querySelector('[data-testid="acp-cli-install-cancel"]')));
  await page.click('[data-testid="acp-cli-install-cancel"]');
  await sleep(500);
  rec('⑱ 取消安装调用', (await callCount(page, 'cancel_acp_agent_install')) === 1);
  rec('⑲ 取消后安装入口恢复可用', await page.evaluate(() => {
    const button = document.querySelector('[data-testid="acp-cli-install-update"]');
    return !!button && !button.disabled && !document.querySelector('[data-testid="acp-cli-install-cancel"]');
  }));

  const pageErrors = errors.filter(message => !message.includes('favicon'));
  rec('⑮ 无页面错误', pageErrors.length === 0, pageErrors.slice(0, 3).join(' | '));

  await browser.close();
  const failed = results.filter(result => !result.pass);
  if (failed.length) {
    console.error(`ACP providers UI smoke FAILED: ${failed.length}/${results.length}`);
    process.exit(1);
  }
  console.log('ACP providers UI smoke passed');
  process.exit(0);
})().catch(async error => {
  console.error('ACP providers UI smoke crashed:', error);
  process.exit(1);
});
