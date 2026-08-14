#!/usr/bin/env node
/**
 * Scheduled tasks UI smoke test.
 *
 * Loads the real src/index.html with a mocked Tauri bridge. Verifies that the
 * scheduled-task page exposes three templates, creates/edits tasks immediately,
 * and opens running or completed conversations in the normal ChatView.
 */
const fs = require('fs');
const os = require('os');
const path = require('path');
const puppeteer = require('puppeteer-core');
const { startUiTestServer } = require('./ui_test_server');
const CHROME = process.env.CHROME ||
  [
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/Applications/Chromium.app/Contents/MacOS/Chromium',
    '/usr/bin/chromium',
    '/usr/bin/chromium-browser',
    '/usr/bin/google-chrome',
    '/usr/bin/google-chrome-stable',
    'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
    'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
    process.env.LOCALAPPDATA ? path.join(process.env.LOCALAPPDATA, 'Google', 'Chrome', 'Application', 'chrome.exe') : '',
    'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
    'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe'
  ].filter(Boolean).find(p => fs.existsSync(p));
if (!CHROME) {
  console.error('SKIP: missing chromium/chrome');
  process.exit(2);
}

function injectSource() {
  return `(function(){
    const SESSIONS = [];
    const TASKS = [];
    const RUNS = [{
      id: 'run-1', automationId: 'task-1', sessionId: 'sched-run-1', status: 'completed',
      scheduledFor: new Date().toISOString(), createdAt: new Date().toISOString(), unread: true,
      sessionTitle: '每天给我推送时尚新闻', pinned: false, pinnedAt: null
    }];
    const RUN_MESSAGES = [
      { role: 'user', content: [{ type: 'text', text: 'Run the daily brief' }] },
      { role: 'assistant', content: [{ type: 'text', text: 'Daily brief complete' }] }
    ];
    const LISTENERS = {};
    let SESSION_SEQ = 0;
    let TASK_SEQ = 0;
    function emit(name, payload) {
      (LISTENERS[name] || []).forEach(function(cb) { cb({ payload: payload }); });
    }
    window.__scheduledTaskTest = {
      invokes: [],
      emit,
      failures: {},
      folderResult: null,
      dialogCalls: []
    };
    function invoke(cmd, args) {
      window.__scheduledTaskTest.invokes.push({ cmd, args: args || null });
      if (window.__scheduledTaskTest.failures[cmd]) {
        return Promise.reject(new Error(window.__scheduledTaskTest.failures[cmd]));
      }
      if (cmd === "list_scheduled_tasks") return Promise.resolve(TASKS.slice());
      if (cmd === "scheduled_task_chat_prompt") {
        return Promise.resolve("我想创建一个 Pinvou 定时任务。请通过提问帮我确定方案。信息完整后输出 scheduled-task-draft 参数，系统会立即创建任务，不需要第二次确认。");
      }
      switch (cmd) {
        case 'get_settings': return Promise.resolve({ theme: 'liquid-light', language: 'zh-Hans' });
        case 'get_effective_model_config': return Promise.resolve({ model: 'Test', base_url: 'http://127.0.0.1:8000/v1', api_key_set: false });
        case 'list_models': return Promise.resolve({
          models: [
            { id: 'model-active', name: 'Smoke Model A', model: '/wire-model' },
            { id: 'model-duplicate', name: 'Smoke Model B', model: '/wire-model' }
          ],
          active_model_id: 'model-active'
        });
        case 'list_sessions': return Promise.resolve(SESSIONS);
        case 'list_archived_sessions': return Promise.resolve([]);
        case 'get_super_permission_status': return Promise.resolve(false);
        case 'get_backend_status': return Promise.resolve({ online: true, ok: true, status: 'online', model: 'Test' });
        case 'check_for_update': return Promise.resolve({ available: false });
        case 'find_resumable_run': return Promise.resolve(null);
        case 'check_dependencies': return Promise.resolve([]);
        case 'list_marketplace_tools': return Promise.resolve([]);
        case 'list_marketplace_skills': return Promise.resolve([]);
        case 'list_personas': return Promise.resolve([]);
        case 'list_skills_v2': return Promise.resolve([]);
        case 'list_workspace_files': return Promise.resolve([]);
        case 'get_mode_state': return Promise.resolve({ mode: 'yolo', plan_phase: 'none' });
        case 'get_active_persona': return Promise.resolve(null);
        case 'session_mounted_collection': return Promise.resolve(null);
        case 'get_session_model_id': return Promise.resolve(null);
        case 'get_session_persona_events': return Promise.resolve([]);
        case 'get_session_pinvou_reviews': return Promise.resolve([]);
        case 'get_memory_overview': return Promise.resolve({});
        case 'detect_local_vllm_setup': return Promise.resolve({ eligible: false });
        case 'read_scheduled_task':
          return Promise.resolve(TASKS.find(function(task) { return task.id === args.id; }) || null);
        case 'list_scheduled_task_runs':
          return Promise.resolve(args && args.id === 'task-1' ? RUNS.slice() : []);
        case 'list_scheduled_runs':
          return Promise.resolve(RUNS.slice());
        case 'load_session':
          if (args && args.id === 'sched-run-1') {
            return Promise.resolve({
              metadata: { id: 'sched-run-1', title: 'Daily brief run' },
              messages: JSON.parse(JSON.stringify(RUN_MESSAGES)),
              artifacts: []
            });
          }
          return Promise.resolve({ metadata: { id: args && args.id, title: 'New chat' }, messages: [], artifacts: [] });
        case 'create_session': {
          const meta = { id: 'session-' + (++SESSION_SEQ), title: '新对话', createdAt: new Date().toISOString() };
          SESSIONS.unshift(meta);
          return Promise.resolve(meta);
        }
        case 'rename_session': {
          const session = SESSIONS.find(function(item) { return item.id === args.id; });
          if (session) {
            session.title = args.title;
            session.updated_at = new Date().toISOString();
          }
          const run = RUNS.find(function(item) { return item.sessionId === args.id; });
          if (run) run.sessionTitle = args.title;
          return Promise.resolve(null);
        }
        case 'set_session_pinned': {
          const session = SESSIONS.find(function(item) { return item.id === args.id; });
          if (session) {
            session.pinned = !!args.pinned;
            session.pinned_at = args.pinned ? new Date().toISOString() : null;
          }
          const run = RUNS.find(function(item) { return item.sessionId === args.id; });
          if (run) {
            run.pinned = !!args.pinned;
            run.pinnedAt = args.pinned ? new Date().toISOString() : null;
          }
          return Promise.resolve(null);
        }
        case 'set_session_archived': {
          const run = RUNS.find(function(item) { return item.sessionId === args.id; });
          if (run) run.archived = !!args.archived;
          return Promise.resolve(null);
        }
        case 'delete_session': {
          const index = SESSIONS.findIndex(function(item) { return item.id === args.id; });
          if (index >= 0) SESSIONS.splice(index, 1);
          const runIndex = RUNS.findIndex(function(item) { return item.sessionId === args.id; });
          if (runIndex >= 0) RUNS.splice(runIndex, 1);
          return Promise.resolve(null);
        }
        case 'chat': {
          const sid = args && args.sessionId ? args.sessionId : 'session-' + SESSION_SEQ;
          const isScheduledGuide = !!(args && args.message && (
            args.message.includes('请一次只问我一个问题') || args.message.includes('scheduled-task-draft')
          ));
          const text = isScheduledGuide ? [
            '我先整理出一个待确认的定时任务草稿。',
            '',
            '\`\`\`scheduled-task-draft',
            '{',
            '  "name": "AI 招聘情报晨报",',
            '  "prompt": "检索并汇总...",',
            '  "rrule": "FREQ=WEEKLY;BYDAY=MO,WE;BYHOUR=8;BYMINUTE=30",',
            '  "paused": true',
            '}',
            '\`\`\`',
            '',
            '请先确认，我再创建。'
          ].join('\\n') : [
            '普通聊天里也可能出现结构化 JSON，但不应该触发定时任务确认。',
            '',
            '\`\`\`json',
            '{',
            '  "name": "不是定时任务",',
            '  "prompt": "这只是说明文字",',
            '  "rrule": "FREQ=DAILY;BYHOUR=9;BYMINUTE=0"',
            '}',
            '\`\`\`'
          ].join('\\n');
          setTimeout(function() {
            emit('chat:delta', { session_id: sid, text: text });
            emit('chat:done', { session_id: sid });
          }, 40);
          return Promise.resolve(null);
        }
        case 'edit_last_turn': {
          const sid = args && args.sessionId;
          setTimeout(function() {
            emit('chat:user_message', {
              session_id: sid,
              content: 'Run the daily brief again',
              operation: 'edit_last',
              base_transcript_revision: 'scheduled-smoke-before-edit'
            });
            emit('chat:turn_started', { session_id: sid });
            emit('chat:delta', { session_id: sid, text: 'Edited brief rerun complete' });
            RUN_MESSAGES.splice(0, RUN_MESSAGES.length,
              { role: 'user', content: [{ type: 'text', text: 'Run the daily brief again' }] },
              { role: 'assistant', content: [{ type: 'text', text: 'Edited brief rerun complete' }] }
            );
            emit('chat:transcript_committed', {
              session_id: sid,
              transcript_revision: 'scheduled-smoke-after-edit'
            });
            emit('chat:done', { session_id: sid, status: 'Completed', error: null });
          }, 40);
          return Promise.resolve(null);
        }
        case 'create_scheduled_task': {
          const input = args && args.input || {};
          const hourMatch = /FREQ=HOURLY;INTERVAL=([0-9]+)/.exec(input.rrule || '');
          const nextDelayMs = hourMatch ? Number(hourMatch[1]) * 3600000 : 4 * 86400000;
          const created = Object.assign({}, input, {
            id: 'task-' + (++TASK_SEQ),
            scheduleLabel: hourMatch
              ? (Number(hourMatch[1]) === 1 ? '每小时' : '每 ' + Number(hourMatch[1]) + ' 小时')
              : (input.rrule || ''),
            status: input.paused ? 'paused' : 'active',
            isRunning: false,
            nextRunAt: new Date(Date.now() + nextDelayMs).toISOString(),
            lastRunAt: null
          });
          created.hasUnreadRuns = RUNS.some(function(run) { return run.automationId === created.id && run.unread; });
          TASKS.unshift(created);
          return Promise.resolve(created);
        }
        case 'update_scheduled_task': {
          const task = TASKS.find(function(item) { return item.id === args.id; });
          if (!task) return Promise.reject(new Error('missing task'));
          Object.assign(task, args.input || {});
          if (args.input && args.input.rrule) task.scheduleLabel = args.input.rrule;
          return Promise.resolve(Object.assign({}, task));
        }
        case 'pause_scheduled_task': {
          const task = TASKS.find(function(item) { return item.id === args.id; });
          if (task) task.status = 'paused';
          return Promise.resolve(task || null);
        }
        case 'resume_scheduled_task': {
          const task = TASKS.find(function(item) { return item.id === args.id; });
          if (task) task.status = 'active';
          return Promise.resolve(task || null);
        }
        case 'set_scheduled_task_pinned': {
          const task = TASKS.find(function(item) { return item.id === args.id; });
          if (task) {
            task.pinned = !!args.pinned;
            task.pinnedAt = args.pinned ? new Date().toISOString() : null;
          }
          return Promise.resolve(task ? Object.assign({}, task) : null);
        }
        case 'delete_scheduled_task': {
          const index = TASKS.findIndex(function(item) { return item.id === args.id; });
          if (index >= 0) TASKS.splice(index, 1);
          return Promise.resolve(true);
        }
        case 'run_scheduled_task_now': {
          const task = TASKS.find(function(item) { return item.id === args.id; });
          window.__scheduledTaskTest.promptAtRun = task && task.prompt;
          if (task) task.isRunning = true;
          const run = {
            id: 'run-now-' + Date.now(), automationId: args.id, sessionId: 'sched-live-' + args.id,
            status: 'running', scheduledFor: new Date().toISOString(), createdAt: new Date().toISOString(), unread: false
          };
          RUNS.unshift(run);
          return Promise.resolve(run);
        }
        case 'mark_scheduled_run_viewed': {
          const run = RUNS.find(function(item) { return item.id === args.runId && item.automationId === args.automationId; });
          if (run) run.unread = false;
          const task = TASKS.find(function(item) { return item.id === args.automationId; });
          if (task) task.hasUnreadRuns = RUNS.some(function(item) { return item.automationId === args.automationId && item.unread; });
          return Promise.resolve({ automationId: args.automationId, runId: args.runId, hasUnreadRuns: !!(task && task.hasUnreadRuns) });
        }
        default: return Promise.resolve(null);
      }
    }
    window.__TAURI__ = {
      core: { invoke },
      event: {
        listen: function(name, cb){
          LISTENERS[name] = LISTENERS[name] || [];
          LISTENERS[name].push(cb);
          return Promise.resolve(function(){});
        },
        emit: function(name, payload){ emit(name, payload); return Promise.resolve(); }
      },
      window: { getCurrentWindow: function(){ return { minimize(){}, maximize(){}, close(){}, toggleMaximize(){}, isMaximized(){ return Promise.resolve(false); }, onResized(){ return Promise.resolve(function(){}); }, startDragging(){} }; } },
      dialog: { open: function(options){ window.__scheduledTaskTest.dialogCalls.push(options || {}); return Promise.resolve(window.__scheduledTaskTest.folderResult); } }
    };
  })();`;
}

const sleep = ms => new Promise(r => setTimeout(r, ms));

async function clickExactText(page, text) {
  return page.evaluate((text) => {
    const els = [...document.querySelectorAll('span,div,button,a')].filter(el => (el.textContent || '').trim() === text);
    const el = els[els.length - 1];
    if (!el) return false;
    el.scrollIntoView({ block: 'center' });
    el.click();
    return true;
  }, text);
}

async function openScheduledNav(page) {
  const clicked = await page.evaluate(() => {
    const el = document.querySelector('[data-nav="scheduled"]') ||
      document.querySelector('[title="定时任务"]') ||
      [...document.querySelectorAll('span,div,button,a')]
        .find(node => (node.textContent || '').trim() === '定时任务');
    if (!el) return false;
    el.click();
    return true;
  });
  if (!clicked) {
    const snapshot = await page.evaluate(() => (document.body.innerText || '').slice(0, 500));
    throw new Error('missing scheduled nav item: ' + snapshot);
  }
  await page.waitForSelector('[data-testid="scheduled-page"]', { timeout: 10000 });
  return true;
}

(async () => {
  const { server, url } = await startUiTestServer();
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-scheduled-tasks-'));
  const browser = await puppeteer.launch({
    executablePath: CHROME,
    headless: 'new',
    args: ['--no-sandbox', '--disable-gpu', '--no-first-run', '--no-default-browser-check'],
    userDataDir: profile
  });
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', e => errors.push(e.message));
  await page.evaluateOnNewDocument(injectSource());
  await page.setViewport({ width: 1440, height: 1000, deviceScaleFactor: 1 });
  await page.goto(url, { waitUntil: 'networkidle0' });
  await page.waitForFunction(() => window.TauriBridge && document.body.innerText.includes('PINVOU'), { timeout: 20000 }).catch(() => {});
  await sleep(1200);

  await page.evaluate(() => window.TauriBridge.chat.sendMessage('普通聊天 JSON 回归测试'));
  await page.waitForFunction(() => {
    const state = window.TauriBridge && window.TauriBridge.state ? window.TauriBridge.state.getMany(['chat', 'scheduled', 'sessions']) : {};
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    return invokes.some(x => x.cmd === 'chat') && state && state.busy === false;
  }, { timeout: 10000 });
  await sleep(200);
  const unrelatedJsonState = await page.evaluate(() => {
    const state = window.TauriBridge && window.TauriBridge.state ? window.TauriBridge.state.getMany(['chat', 'scheduled', 'sessions']) : {};
    return {
      hasDraftState: !!(state && state.scheduledTaskDraft),
      confirmVisible: !!document.querySelector('[data-testid="scheduled-task-draft-card"]')
    };
  });
  await page.evaluate(() => { window.__scheduledTaskTest.invokes = []; });

  await page.evaluate(() => {
    const b = document.querySelector('[data-sidebar-toggle]');
    if (b) b.click();
  });
  await sleep(300);

  const navClicked = await openScheduledNav(page);
  await sleep(500);
  const defaultState = await page.evaluate(() => ({
    navClicked: !!document.querySelector('[data-testid="scheduled-page"]'),
    hasTitle: document.body.innerText.includes('已安排的任务'),
    hasIntro: !!document.querySelector('[data-testid="scheduled-list-intro"]'),
    templateCount: document.querySelectorAll('button[data-testid^="scheduled-template-"]').length,
    hasDailyBrief: !!document.querySelector('[data-testid="scheduled-template-daily-brief"]'),
    detailVisible: !!document.querySelector('[data-testid="scheduled-detail"]'),
    listDeleteCount: document.querySelectorAll('[data-testid="scheduled-list-delete"]').length,
    sampleTextPresent: /每日项目状态提醒|每周资料整理提醒|项目A/.test(document.body.innerText)
  }));
  await page.evaluate(() => { window.__scheduledTaskTest.invokes = []; });
  await page.waitForSelector('[data-testid="scheduled-template-daily-brief"]', { timeout: 10000 });
  await page.click('[data-testid="scheduled-template-daily-brief"]');
  await page.waitForSelector('[data-testid="scheduled-create-dialog"]', { timeout: 10000 });
  const templateDraftState = await page.evaluate(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    const name = document.querySelector('[data-testid="scheduled-create-name"]');
    const prompt = document.querySelector('[data-testid="scheduled-create-prompt"]');
    const createSettings = document.querySelector('[data-testid="scheduled-create-settings"]');
    const createSettingsStyle = createSettings ? getComputedStyle(createSettings) : null;
    const noOuterBorder = style => !!style &&
      style.borderTopWidth === '0px' &&
      style.borderRightWidth === '0px' &&
      style.borderBottomWidth === '0px' &&
      style.borderLeftWidth === '0px';
    return {
      createCallsBeforeSubmit: invokes.filter(x => x.cmd === 'create_scheduled_task').length,
      dialogVisible: !!document.querySelector('[data-testid="scheduled-create-dialog"]'),
      title: document.body.innerText.includes('基于模板创建'),
      name: name && name.value,
      promptPresent: !!(prompt && prompt.value),
      createSettingsNoOuterBorder: noOuterBorder(createSettingsStyle)
    };
  });
  await page.click('[data-testid="scheduled-create-submit"]');
  await page.waitForFunction(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    return invokes.filter(x => x.cmd === 'create_scheduled_task').length === 1 &&
      !document.querySelector('[data-testid="scheduled-create-dialog"]') &&
      document.body.innerText.includes('每日早报');
  }, { timeout: 10000 });
  const templateAutoOpenState = await page.evaluate(() => ({
    autoOpenSuppressed: !document.querySelector('[data-testid="scheduled-detail"]') &&
      !document.querySelector('[data-testid="scheduled-live-title"]')
  }));
  await page.evaluate(() => {
    const buttons = Array.from(document.querySelectorAll('button[aria-label^="查看定时任务"]'));
    const target = buttons.find(button => /每日早报/.test(button.textContent || ''));
    if (target) {
      target.click();
      return;
    }
    if (window.TauriBridge && window.TauriBridge.scheduled.selectScheduledTask) {
      window.TauriBridge.scheduled.selectScheduledTask('task-1');
      if (window.TauriBridge.scheduled.refreshScheduledTaskData) window.TauriBridge.scheduled.refreshScheduledTaskData(20);
      return;
    }
    throw new Error('missing created daily brief task row');
  });
  await page.waitForFunction(() => {
    const title = document.querySelector('[data-testid="scheduled-live-title"]');
    return title && title.value === '每日早报';
  }, { timeout: 10000 });
  const templateCreateState = await page.evaluate((templateDraftState, templateAutoOpenState) => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    const create = invokes.find(x => x.cmd === 'create_scheduled_task');
    const name = document.querySelector('[data-testid="scheduled-live-title"]');
    const prompt = document.querySelector('[data-testid="scheduled-live-prompt"]');
    const settings = document.querySelector('[data-testid="scheduled-detail-settings"]');
    const actions = document.querySelector('[data-testid="scheduled-detail-actions-group"]');
    const history = document.querySelector('[data-testid="scheduled-run-history-list"]');
    const noOuterBorder = el => {
      const style = el ? getComputedStyle(el) : null;
      return !!style &&
        style.borderTopWidth === '0px' &&
        style.borderRightWidth === '0px' &&
        style.borderBottomWidth === '0px' &&
        style.borderLeftWidth === '0px';
    };
    return {
      draftCreatedFirst: templateDraftState.createCallsBeforeSubmit === 0 &&
        templateDraftState.dialogVisible &&
        templateDraftState.title &&
        templateDraftState.name === '每日早报' &&
        templateDraftState.promptPresent &&
        templateDraftState.createSettingsNoOuterBorder,
      autoOpenSuppressed: templateAutoOpenState.autoOpenSuppressed,
      editorAbsent: !document.querySelector('[data-testid="scheduled-create-dialog"]') &&
        !document.querySelector('[data-testid="scheduled-draft-editor"]') &&
        !document.querySelector('[data-testid="scheduled-draft-confirm"]'),
      name: name && name.value,
      promptPresent: !!(prompt && prompt.value),
      editable: !!(name && prompt && !name.disabled && !name.readOnly && !prompt.disabled && !prompt.readOnly),
      introAbsent: !document.querySelector('[data-testid="scheduled-list-intro"]'),
      unifiedSettings: !!settings &&
        !!settings.querySelector('[data-testid="scheduled-live-model"]') &&
        !!settings.querySelector('[data-testid="scheduled-live-repeat"]') &&
        !!settings.querySelector('[data-testid="scheduled-live-time"]') &&
        settings.textContent.includes('设置'),
      frequencySectionAbsent: !document.querySelector('[data-testid="scheduled-detail-frequency"]'),
      executionModeAbsent: !document.querySelector('[data-testid="scheduled-yolo-mode"]') &&
        !document.body.innerText.includes('执行模式'),
      permissionControlsAbsent: !document.body.innerText.includes('权限') &&
        !Array.from(document.querySelectorAll('label')).some(el => /Shell|信任模式/.test(el.textContent || '')),
      insetGroupsNoOuterBorder: noOuterBorder(settings) && noOuterBorder(actions) && noOuterBorder(history),
      navUnread: !!document.querySelector('[data-testid="scheduled-nav-unread"]'),
      createCalls: invokes.filter(x => x.cmd === 'create_scheduled_task').length,
      model: create && create.args && create.args.input && create.args.input.model,
      modelId: create && create.args && create.args.input && create.args.input.modelId,
      paused: create && create.args && create.args.input && create.args.input.paused
    };
  }, templateDraftState, templateAutoOpenState);
  // 目录选择已移除：详情页不再有项目行/选目录按钮，后端按 automation_id 分配工作间。
  const workspaceUiAbsent = await page.evaluate(() => (
    !document.querySelector('[data-testid="scheduled-live-project"]') &&
    !document.querySelector('[data-testid="scheduled-detail-pick-folder"]') &&
    !document.querySelector('[data-testid="scheduled-workspace-required"]')
  ));
  await page.click('[data-testid="scheduled-detail-close"]');
  await page.waitForSelector('[data-testid="scheduled-template-suggestions"]', { timeout: 10000 });
  const templateRetainedState = await page.evaluate(() => ({
    detailHidden: !document.querySelector('[data-testid="scheduled-detail"]'),
    count: document.querySelectorAll('button[data-testid^="scheduled-template-"]').length,
    hasDailyBrief: !!document.querySelector('[data-testid="scheduled-template-daily-brief"]')
  }));
  await page.evaluate(() => window.TauriBridge.scheduled.loadScheduledTaskRecentRuns());
  await page.waitForFunction(() => document.body.innerText.includes('任务列表'), { timeout: 10000 });
  await page.waitForSelector('[data-testid="scheduled-run-sidebar-item"]', { timeout: 10000 });
  await page.click('[data-testid="scheduled-run-sidebar-item"]', { button: 'right' });
  await page.waitForSelector('[data-testid="scheduled-run-sidebar-menu"]', { timeout: 10000 });
  const sidebarRecordMenuState = await page.evaluate((openState) => {
    const menu = document.querySelector('[data-testid="scheduled-run-sidebar-menu"]');
    const menuText = menu ? menu.textContent || '' : '';
    return Object.assign({}, openState, {
      hasRename: menuText.includes('重命名'),
      hasPin: menuText.includes('置顶'),
      hasDelete: menuText.includes('删除'),
      hasArchive: menuText.includes('收纳')
    });
  }, { itemVisible: true, moreVisible: true });
  await page.evaluate(() => {
    const rename = Array.from(document.querySelectorAll('button'))
      .find(button => (button.textContent || '').trim() === '重命名');
    if (!rename) throw new Error('missing scheduled record rename action');
    rename.click();
  });
  await page.waitForSelector('input', { timeout: 10000 });
  await page.keyboard.down('Control');
  await page.keyboard.press('A');
  await page.keyboard.up('Control');
  await page.keyboard.type('重命名后的时尚新闻记录');
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    return invokes.some(x => x.cmd === 'rename_session' && x.args && x.args.id === 'sched-run-1' && x.args.title === '重命名后的时尚新闻记录');
  }, { timeout: 10000 });
  await page.click('[data-testid="scheduled-run-sidebar-item"]', { button: 'right' });
  await page.waitForSelector('[data-testid="scheduled-run-sidebar-menu"]', { timeout: 10000 });
  await page.evaluate(() => {
    const pin = Array.from(document.querySelectorAll('button'))
      .find(button => (button.textContent || '').trim() === '置顶');
    if (!pin) throw new Error('missing scheduled record pin action');
    pin.click();
  });
  await page.waitForFunction(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    return invokes.some(x => x.cmd === 'set_session_pinned' && x.args && x.args.id === 'sched-run-1' && x.args.pinned === true);
  }, { timeout: 10000 });
  const sidebarRecordPinnedState = await page.evaluate(() => ({
    pinnedGroupHasRecord: document.body.innerText.includes('任务列表') &&
      document.body.innerText.includes('重命名后的时尚新闻记录'),
    noTaskPinCommand: !((window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [])
      .some(x => x.cmd === 'set_scheduled_task_pinned')
  }));
  await page.click('[data-testid="scheduled-run-sidebar-item"]', { button: 'right' });
  await page.waitForSelector('[data-testid="scheduled-run-sidebar-menu"]', { timeout: 10000 });
  await page.evaluate(() => {
    const del = Array.from(document.querySelectorAll('button'))
      .find(button => (button.textContent || '').trim() === '删除');
    if (!del) throw new Error('missing scheduled record delete action');
    del.click();
  });
  await page.evaluate(() => {
    const cancel = Array.from(document.querySelectorAll('button[title="取消"]')).pop();
    if (!cancel) throw new Error('missing scheduled record delete cancellation');
    cancel.click();
  });
  const sidebarRecordDeleteState = await page.evaluate(() => ({
    deleteSessionCalls: ((window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [])
      .filter(x => x.cmd === 'delete_session').length,
    deleteTaskCalls: ((window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [])
      .filter(x => x.cmd === 'delete_scheduled_task').length,
    recordStillVisible: document.body.innerText.includes('重命名后的时尚新闻记录'),
    listStillVisible: !!document.querySelector('[data-testid="scheduled-list"]')
  }));
  await openScheduledNav(page);
  await page.waitForSelector('[data-testid="scheduled-list"]', { timeout: 10000 });
  await page.evaluate(() => {
    const buttons = Array.from(document.querySelectorAll('button[aria-label^="查看定时任务"]'));
    const target = buttons.find(button => /每日早报/.test(button.textContent || ''));
    if (target) {
      target.click();
      return;
    }
    if (window.TauriBridge && window.TauriBridge.scheduled.selectScheduledTask) {
      window.TauriBridge.scheduled.selectScheduledTask('task-1');
      if (window.TauriBridge.scheduled.refreshScheduledTaskData) window.TauriBridge.scheduled.refreshScheduledTaskData(20);
      return;
    }
    throw new Error('missing created daily brief task row');
  });
  await page.waitForFunction(() => {
    const title = document.querySelector('[data-testid="scheduled-live-title"]');
    return title && title.value === '每日早报';
  }, { timeout: 10000 });
  await page.evaluate(() => {
    const inputSetter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    const textareaSetter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
    const name = document.querySelector('[data-testid="scheduled-live-title"]');
    const prompt = document.querySelector('[data-testid="scheduled-live-prompt"]');
    inputSetter.call(name, '编辑后的每日早报');
    name.dispatchEvent(new Event('input', { bubbles: true }));
    textareaSetter.call(prompt, '完全自定义的任务说明');
    prompt.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await page.waitForFunction(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    const updates = invokes.filter(x => x.cmd === 'update_scheduled_task');
    return updates.some(x => x.args && x.args.input && x.args.input.name === '编辑后的每日早报') &&
      updates.some(x => x.args && x.args.input && x.args.input.prompt === '完全自定义的任务说明');
  }, { timeout: 10000 });
  await page.click('[data-testid="scheduled-live-repeat"]');
  await page.waitForSelector('[data-testid="scheduled-live-repeat-option"][data-value="workdays"]', { timeout: 10000 });
  await page.click('[data-testid="scheduled-live-repeat-option"][data-value="workdays"]');
  await page.click('[data-testid="scheduled-live-time"]');
  await page.waitForSelector('[data-testid="scheduled-live-time-hour"]', { timeout: 10000 });
  await page.evaluate(() => document.querySelector('[data-testid="scheduled-live-time-hour"] button[data-value="09"]').click());
  await page.waitForFunction(() => {
    const el = document.querySelector('[data-testid="scheduled-live-time"]');
    return el && el.value.startsWith('09:');
  }, { timeout: 10000 });
  await page.evaluate(() => document.querySelector('[data-testid="scheduled-live-time-minute"] button[data-value="30"]').click());
  await page.waitForFunction(() => {
    const el = document.querySelector('[data-testid="scheduled-live-time"]');
    return el && el.value === '09:30';
  }, { timeout: 10000 });
  await page.keyboard.press('Escape');
  await page.waitForFunction(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    const updates = invokes.filter(x => x.cmd === 'update_scheduled_task');
    return updates.some(x => x.args && x.args.input && x.args.input.name === '编辑后的每日早报') &&
      updates.some(x => x.args && x.args.input && x.args.input.prompt === '完全自定义的任务说明') &&
      updates.some(x => x.args && x.args.input && /BYHOUR=9;BYMINUTE=30/.test(x.args.input.rrule || ''));
  }, { timeout: 10000 });
  const templateEditState = await page.evaluate(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    const updates = invokes.filter(x => x.cmd === 'update_scheduled_task');
    return {
      updateCalls: updates.length,
      name: document.querySelector('[data-testid="scheduled-live-title"]') && document.querySelector('[data-testid="scheduled-live-title"]').value,
      prompt: document.querySelector('[data-testid="scheduled-live-prompt"]') && document.querySelector('[data-testid="scheduled-live-prompt"]').value,
      repeat: document.querySelector('[data-testid="scheduled-live-repeat"]') && document.querySelector('[data-testid="scheduled-live-repeat"]').value,
      time: document.querySelector('[data-testid="scheduled-live-time"]') && document.querySelector('[data-testid="scheduled-live-time"]').value
    };
  });

  await page.hover('button[aria-label^="查看定时任务"]');
  await page.evaluate(() => {
    const button = document.querySelector('button[aria-label^="查看定时任务"]');
    window.__scheduledTaskTest.hoveredTaskRow = button && button.parentElement;
  });
  await sleep(1200);
  const hoverStabilityState = await page.evaluate(() => {
    const button = document.querySelector('button[aria-label^="查看定时任务"]');
    const row = button && button.parentElement;
    return {
      sameNode: !!row && row === window.__scheduledTaskTest.hoveredTaskRow,
      hovered: !!row && row.matches(':hover')
    };
  });

  await page.evaluate(() => {
    const inputSetter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    const name = document.querySelector('[data-testid="scheduled-live-title"]');
    const prompt = document.querySelector('[data-testid="scheduled-live-prompt"]');
    name.focus();
    inputSetter.call(name, '   ');
    name.dispatchEvent(new Event('input', { bubbles: true }));
    prompt.focus();
  });
  await sleep(450);
  const blankRequiredState = await page.evaluate(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    return {
      title: document.querySelector('[data-testid="scheduled-live-title"]') && document.querySelector('[data-testid="scheduled-live-title"]').value,
      blankUpdates: invokes.filter(x => x.cmd === 'update_scheduled_task' && x.args && x.args.input &&
        Object.prototype.hasOwnProperty.call(x.args.input, 'name') && !String(x.args.input.name || '').trim()).length
    };
  });

  await page.waitForSelector('button[data-testid="scheduled-run-row"]', { timeout: 10000 });
  const unreadBeforeOpen = await page.evaluate(() => ({
    task: !!document.querySelector('[data-testid="scheduled-task-unread"]'),
    run: !!document.querySelector('[data-testid="scheduled-run-unread"]')
  }));
  await page.evaluate(() => { window.__scheduledTaskTest.failures.load_session = 'scheduled run load failed'; });
  await page.click('button[data-testid="scheduled-run-row"]');
  await sleep(300);
  const failedRunOpenState = await page.evaluate(() => {
    const state = window.TauriBridge.state.getMany(['chat', 'scheduled', 'sessions']);
    return {
      stayedInScheduled: !!document.querySelector('[data-testid="scheduled-page"]'),
      route: document.querySelector('[data-testid="app-root"]') && document.querySelector('[data-testid="app-root"]').dataset.currentView,
      contextAbsent: !state.scheduledRunContext,
      selectedId: state.selectedScheduledTaskId,
      errorVisible: document.body.innerText.includes('scheduled run load failed'),
      taskUnread: !!document.querySelector('[data-testid="scheduled-task-unread"]'),
      runUnread: !!document.querySelector('[data-testid="scheduled-run-unread"]'),
      chatPolluted: (state.chatItems || []).some(function(item) {
        return String(item.html || item.text || '').includes('scheduled run load failed');
      })
    };
  });
  await page.evaluate(() => { delete window.__scheduledTaskTest.failures.load_session; });
  await page.click('button[data-testid="scheduled-run-row"]');
  await page.waitForSelector('[data-testid="scheduled-run-back"]', { timeout: 10000 });
  const runChatState = await page.evaluate(() => {
    const state = window.TauriBridge && window.TauriBridge.state ? window.TauriBridge.state.getMany(['chat', 'scheduled', 'sessions']) : {};
    return {
      inChatView: !document.querySelector('[data-testid="scheduled-page"]'),
      route: document.querySelector('[data-testid="app-root"]') && document.querySelector('[data-testid="app-root"]').dataset.currentView,
      backVisible: !!document.querySelector('[data-testid="scheduled-run-back"]'),
      transcriptVisible: document.body.innerText.includes('Daily brief complete'),
      editResendVisible: !!document.querySelector('button[title="编辑并重发"]'),
      sessionId: state.scheduledRunContext && state.scheduledRunContext.sessionId,
      taskName: state.scheduledRunContext && state.scheduledRunContext.taskName,
      model: state.scheduledRunContext && state.scheduledRunContext.model
    };
  });
  // 真实点击「编辑并重发」:定时会话与普通对话同路,后端不再拒绝。
  const editOpened = await page.evaluate(() => {
    const btn = document.querySelector('button[title="编辑并重发"]');
    if (!btn) return false;
    btn.click();
    return true;
  });
  await page.waitForFunction(() => {
    return [...document.querySelectorAll('textarea')].some(el => el.value === 'Run the daily brief');
  }, { timeout: 10000 });
  await page.evaluate(() => {
    const ta = [...document.querySelectorAll('textarea')].find(el => el.value === 'Run the daily brief');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
    setter.call(ta, 'Run the daily brief again');
    ta.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await clickExactText(page, '重新发送');
  await page.waitForFunction(() => {
    const state = window.TauriBridge && window.TauriBridge.state ? window.TauriBridge.state.getMany(['chat', 'scheduled', 'sessions']) : {};
    return state.busy === false && document.body.innerText.includes('Edited brief rerun complete');
  }, { timeout: 10000 });
  const editResendState = await page.evaluate(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    const call = invokes.filter(x => x.cmd === 'edit_last_turn').pop();
    return {
      invoked: !!call,
      sessionId: call && call.args && call.args.sessionId,
      newMessage: call && call.args && call.args.newMessage,
      rerunVisible: document.body.innerText.includes('Edited brief rerun complete'),
      errorShown: document.body.innerText.includes('⚠️')
    };
  });
  await page.click('[data-testid="scheduled-run-back"]');
  await page.waitForFunction(() => {
    const title = document.querySelector('[data-testid="scheduled-live-title"]');
    return !!document.querySelector('[data-testid="scheduled-page"]') &&
      !!document.querySelector('button[data-testid="scheduled-run-row"]') &&
      title && title.value === '编辑后的每日早报';
  }, { timeout: 10000 });
  const runReturnState = await page.evaluate(() => {
    const state = window.TauriBridge && window.TauriBridge.state ? window.TauriBridge.state.getMany(['chat', 'scheduled', 'sessions']) : {};
    const settings = document.querySelector('[data-testid="scheduled-detail-settings"]');
    return {
      scheduledVisible: !!document.querySelector('[data-testid="scheduled-page"]'),
      route: document.querySelector('[data-testid="app-root"]') && document.querySelector('[data-testid="app-root"]').dataset.currentView,
      contextCleared: !state.scheduledRunContext,
      selectedId: state.selectedScheduledTaskId,
      detailTitle: document.querySelector('[data-testid="scheduled-live-title"]') && document.querySelector('[data-testid="scheduled-live-title"]').value,
      runHistoryVisible: !!document.querySelector('button[data-testid="scheduled-run-row"]'),
      compactLayout: !!document.querySelector('[data-testid="scheduled-detail-prompt"]') &&
        !!settings &&
        !!settings.querySelector('[data-testid="scheduled-live-model"]') &&
        !!settings.querySelector('[data-testid="scheduled-live-repeat"]') &&
        !!settings.querySelector('[data-testid="scheduled-live-time"]') &&
        !document.querySelector('[data-testid="scheduled-detail-frequency"]') &&
        !document.querySelector('[data-testid="scheduled-yolo-mode"]'),
      unreadCleared: !document.querySelector('[data-testid="scheduled-run-unread"]') &&
        !document.querySelector('[data-testid="scheduled-task-unread"]'),
      navUnreadCleared: !document.querySelector('[data-testid="scheduled-nav-unread"]')
    };
  });

  const pollBefore = await page.evaluate(() => {
    const invokes = window.__scheduledTaskTest.invokes;
    return {
      tasks: invokes.filter(x => x.cmd === 'list_scheduled_tasks').length,
      detail: invokes.filter(x => x.cmd === 'read_scheduled_task').length,
      runs: invokes.filter(x => x.cmd === 'list_scheduled_task_runs').length
    };
  });
  await sleep(3300);
  const pollAfter = await page.evaluate((before) => {
    const invokes = window.__scheduledTaskTest.invokes;
    const after = {
      tasks: invokes.filter(x => x.cmd === 'list_scheduled_tasks').length,
      detail: invokes.filter(x => x.cmd === 'read_scheduled_task').length,
      runs: invokes.filter(x => x.cmd === 'list_scheduled_task_runs').length
    };
    return {
      taskRefreshes: after.tasks - before.tasks,
      detailRefreshes: after.detail - before.detail,
      runRefreshes: after.runs - before.runs
    };
  }, pollBefore);

  async function openScheduledDetailMenu() {
    await page.waitForSelector('[data-testid="scheduled-run-now"]', { timeout: 10000 });
  }

  await openScheduledDetailMenu();
  await page.click('[data-testid="scheduled-open-folder"]');
  await page.waitForFunction(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    return invokes.some(x => x.cmd === 'open_scheduled_task_folder');
  }, { timeout: 10000 });
  const folderOpenState = await page.evaluate(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    const call = invokes.filter(x => x.cmd === 'open_scheduled_task_folder').pop();
    return {
      automationId: call && call.args && call.args.automationId,
      menuClosed: !document.querySelector('[data-testid="scheduled-detail-menu-popover"]')
    };
  });

  await page.evaluate(() => { window.__scheduledTaskTest.failures.run_scheduled_task_now = 'run now failed visibly'; });
  await openScheduledDetailMenu();
  await page.click('[data-testid="scheduled-run-now"]');
  await page.waitForFunction(() => document.body.innerText.includes('run now failed visibly'), { timeout: 10000 });
  await openScheduledDetailMenu();
  const runNowFailureState = await page.evaluate(() => ({
    errorVisible: document.body.innerText.includes('run now failed visibly'),
    menuVisible: !!document.querySelector('[data-testid="scheduled-detail-menu-popover"]'),
    buttonEnabledAgain: !!document.querySelector('[data-testid="scheduled-run-now"]') &&
      !document.querySelector('[data-testid="scheduled-run-now"]').disabled
  }));
  await page.keyboard.press('Escape');
  await page.evaluate(() => { delete window.__scheduledTaskTest.failures.run_scheduled_task_now; });

  await page.evaluate(() => {
    window.__scheduledTaskTest.failures.update_scheduled_task = 'transient autosave failure';
    const textareaSetter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
    const prompt = document.querySelector('[data-testid="scheduled-live-prompt"]');
    prompt.focus();
    textareaSetter.call(prompt, '点击运行前最后一刻修改的说明');
    prompt.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await page.waitForFunction(() => {
    const state = document.querySelector('[data-testid="scheduled-save-state"]');
    return state && state.textContent.includes('保存失败');
  }, { timeout: 10000 });
  const saveRetryState = await page.evaluate(() => ({
    errorShown: document.querySelector('[data-testid="scheduled-save-state"]') &&
      document.querySelector('[data-testid="scheduled-save-state"]').textContent.includes('保存失败')
  }));
  await page.evaluate(() => { delete window.__scheduledTaskTest.failures.update_scheduled_task; });
  await openScheduledDetailMenu();
  await page.click('[data-testid="scheduled-run-now"]');
  await page.waitForFunction(() =>
    !!document.querySelector('[data-testid="scheduled-task-running"]') &&
    !!document.querySelector('[data-testid="scheduled-run-running"]'), { timeout: 10000 });
  const runningSpinnerState = await page.evaluate(() => ({
    taskSpinner: !!document.querySelector('[data-testid="scheduled-task-running"]'),
    runSpinner: !!document.querySelector('[data-testid="scheduled-run-running"]'),
    noChatSpinner: !document.querySelector('[data-testid="scheduled-chat-running"]'),
    promptAtRun: window.__scheduledTaskTest.promptAtRun
  }));
  await page.evaluate(() => {
    const spinner = document.querySelector('[data-testid="scheduled-run-running"]');
    const row = spinner && spinner.closest('button[data-testid="scheduled-run-row"]');
    if (row) row.click();
  });
  await page.waitForSelector('[data-testid="scheduled-run-back"]', { timeout: 10000 });
  await page.evaluate(() => {
    const state = window.TauriBridge.state.getMany(['chat', 'scheduled', 'sessions']);
    window.__scheduledTaskTest.emit('chat:delta', { session_id: state.activeSessionId, text: '运行中的实时内容' });
  });
  await page.waitForFunction(() => document.body.innerText.includes('运行中的实时内容'), { timeout: 10000 });
  const runningChatState = await page.evaluate(() => ({
    normalChatVisible: !document.querySelector('[data-testid="scheduled-page"]') && document.body.innerText.includes('运行中的实时内容'),
    route: document.querySelector('[data-testid="app-root"]') && document.querySelector('[data-testid="app-root"]').dataset.currentView,
    hasCustomSpinner: !!document.querySelector('[data-testid="scheduled-chat-running"]')
  }));
  await page.click('[data-testid="scheduled-run-back"]');
  await page.waitForSelector('[data-testid="scheduled-page"]', { timeout: 10000 });

  await page.evaluate(() => {
    window.__scheduledTaskTest.invokes = [];
  });
  await openScheduledDetailMenu();
  await page.click('[data-testid="scheduled-detail-delete"]');
  await page.waitForSelector('[data-testid="scheduled-detail-delete-confirmation"]', { timeout: 10000 });
  const deletePromptState = await page.evaluate(() => ({
    deleteCalls: window.__scheduledTaskTest.invokes.filter(x => x.cmd === 'delete_scheduled_task').length,
    promptVisible: !!document.querySelector('[data-testid="scheduled-detail-delete-confirmation"]')
  }));
  await page.click('[data-testid="scheduled-detail-delete-cancel"]');
  await sleep(150);
  const deleteCancelState = await page.evaluate(() => ({
    deleteCalls: window.__scheduledTaskTest.invokes.filter(x => x.cmd === 'delete_scheduled_task').length,
    detailStillVisible: !!document.querySelector('[data-testid="scheduled-detail"]'),
    promptHidden: !document.querySelector('[data-testid="scheduled-detail-delete-confirmation"]')
  }));

  await page.click('[data-testid="scheduled-detail-delete"]');
  await page.waitForSelector('[data-testid="scheduled-detail-delete-confirmation"]', { timeout: 10000 });
  const deleteConfirmBefore = await page.evaluate(() =>
    window.__scheduledTaskTest.invokes.filter(x => x.cmd === 'delete_scheduled_task').length
  );
  await page.evaluate(() => document.querySelector('[data-testid="scheduled-detail-delete-confirm"]').click());
  await page.waitForFunction((before) => {
    const invokes = window.__scheduledTaskTest.invokes || [];
    return invokes.filter(x => x.cmd === 'delete_scheduled_task').length > before;
  }, { timeout: 10000 }, deleteConfirmBefore);
  await page.waitForFunction(() => !document.querySelector('[data-testid="scheduled-detail"]'), { timeout: 10000 });
  const deleteConfirmState = await page.evaluate(() => ({
    deleteCalls: window.__scheduledTaskTest.invokes.filter(x => x.cmd === 'delete_scheduled_task').length,
    detailClosed: !document.querySelector('[data-testid="scheduled-detail"]')
  }));

  await page.evaluate(() => { window.__scheduledTaskTest.invokes = []; });
  await page.evaluate(() => document.querySelector('[data-testid="scheduled-create-menu"]').click());
  const customCreationClicked = true;
  await page.waitForSelector('[data-testid="scheduled-create-dialog"]', { timeout: 10000 });
  const customCreationBeforeSubmit = await page.evaluate(() => ({
    createCalls: window.__scheduledTaskTest.invokes.filter(x => x.cmd === 'create_scheduled_task').length,
    submitDisabled: document.querySelector('[data-testid="scheduled-create-submit"]').disabled
  }));
  await page.evaluate(() => {
    function setReactValue(selector, value) {
      const el = document.querySelector(selector);
      const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(el), 'value').set;
      const previous = el.value;
      setter.call(el, value);
      if (el._valueTracker) el._valueTracker.setValue(previous);
      el.dispatchEvent(new Event('input', { bubbles: true }));
    }
    setReactValue('[data-testid="scheduled-create-name"]', 'Custom office brief');
    setReactValue('[data-testid="scheduled-create-prompt"]', 'Summarize the important office updates for the day.');
  });
  await page.waitForFunction(() => {
    const submit = document.querySelector('[data-testid="scheduled-create-submit"]');
    return submit && !submit.disabled;
  }, { timeout: 10000 });
  await page.click('[data-testid="scheduled-create-submit"]');
  await page.waitForFunction(() => {
    const invokes = window.__scheduledTaskTest.invokes || [];
    return invokes.filter(x => x.cmd === 'create_scheduled_task').length === 1 &&
      !document.querySelector('[data-testid="scheduled-create-dialog"]');
  }, { timeout: 10000 });
  const customCreationState = await page.evaluate(() => {
    const call = window.__scheduledTaskTest.invokes.find(x => x.cmd === 'create_scheduled_task');
    const input = call && call.args && call.args.input;
    return {
      name: input && input.name,
      prompt: input && input.prompt,
      rrule: input && input.rrule,
      mode: input && input.mode,
      model: input && input.model,
      modelId: input && input.modelId,
      paused: input && input.paused,
      dialogClosed: !document.querySelector('[data-testid="scheduled-create-dialog"]')
    };
  });

  await page.evaluate(() => { window.__scheduledTaskTest.invokes = []; });
  await page.evaluate(() => document.querySelector('[data-testid="scheduled-create-from-chat"]').click());
  const openChatClicked = true;
  // 新流程:点击只预填输入框,不自动发送 —— 先等引导词就位。
  await page.waitForFunction(() => {
    const state = window.TauriBridge && window.TauriBridge.state ? window.TauriBridge.state.getMany(['chat', 'scheduled', 'sessions']) : {};
    return !!state.scheduledTaskPendingGuide;
  }, { timeout: 10000 });
  await sleep(500);
  const preSend = await page.evaluate(() => {
    const state = window.TauriBridge && window.TauriBridge.state ? window.TauriBridge.state.getMany(['chat', 'scheduled', 'sessions']) : {};
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    return {
      inChatView: !document.querySelector('[data-testid="scheduled-page"]'),
      guidePending: !!state.scheduledTaskPendingGuide,
      composerPrefilled: Array.from(document.querySelectorAll('textarea')).some(el => (el.value || '').includes('我想创建一个定时任务')),
      guideHidden: !document.body.innerText.includes('请一次只问我一个问题'),
      confirmVisible: !!document.querySelector('[data-testid="scheduled-task-draft-card"]'),
      chatCalls: invokes.filter(x => x.cmd === 'chat').length,
      createSessionCalls: invokes.filter(x => x.cmd === 'create_session').length,
      promptCalls: invokes.filter(x => x.cmd === 'scheduled_task_chat_prompt').length
    };
  });

  await page.evaluate(() => window.TauriBridge.chat.sendMessage('我想创建一个定时任务：工作日每天早上 8 点半做 AI 招聘情报晨报'));
  await page.waitForFunction(() => {
    const state = window.TauriBridge && window.TauriBridge.state ? window.TauriBridge.state.getMany(['chat', 'scheduled', 'sessions']) : {};
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    const title = document.querySelector('[data-testid="scheduled-live-title"]');
    return state.scheduledTaskAutoOpenId &&
      invokes.filter(x => x.cmd === 'create_scheduled_task').length === 1 &&
      !!document.querySelector('[data-testid="scheduled-page"]') &&
      title && title.value === 'AI 招聘情报晨报';
  }, { timeout: 10000 });
  const chatAutoCreateState = await page.evaluate(() => {
    const state = window.TauriBridge && window.TauriBridge.state ? window.TauriBridge.state.getMany(['chat', 'scheduled', 'sessions']) : {};
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    const chats = invokes.filter(x => x.cmd === 'chat');
    const create = invokes.find(x => x.cmd === 'create_scheduled_task');
    const input = create && create.args && create.args.input;
    return {
      scheduledVisible: !!document.querySelector('[data-testid="scheduled-page"]'),
      route: document.querySelector('[data-testid="app-root"]') && document.querySelector('[data-testid="app-root"]').dataset.currentView,
      payloadHasGuide: chats.length === 1 && !!(chats[0].args && chats[0].args.message && chats[0].args.message.includes('scheduled-task-draft')),
      toolsRestricted: chats.length === 1 && chats[0].args && chats[0].args.restrictTools === true,
      guideCleared: !state.scheduledTaskPendingGuide,
      confirmAbsent: !document.querySelector('[data-testid="scheduled-task-draft-card"]') && !document.querySelector('[data-testid="scheduled-task-draft-confirm"]'),
      draftStateAbsent: !state.scheduledTaskDraft,
      title: document.querySelector('[data-testid="scheduled-live-title"]') && document.querySelector('[data-testid="scheduled-live-title"]').value,
      createCalls: invokes.filter(x => x.cmd === 'create_scheduled_task').length,
      chatCalls: chats.length,
      createSessionCalls: invokes.filter(x => x.cmd === 'create_session').length,
      promptCalls: invokes.filter(x => x.cmd === 'scheduled_task_chat_prompt').length,
      model: input && input.model,
      modelId: input && input.modelId,
      sourceSessionAbsent: !!(input && !Object.prototype.hasOwnProperty.call(input, 'sourceSessionId')),
      selectedId: state.selectedScheduledTaskId,
      autoOpenId: state.scheduledTaskAutoOpenId
    };
  });

  async function toggleScheduledWeekday(value, expectedValue) {
    await page.waitForSelector(`[data-testid="scheduled-live-day-option"][data-value="${value}"]`, { timeout: 10000 });
    await page.click(`[data-testid="scheduled-live-day-option"][data-value="${value}"]`);
    await page.waitForFunction((expected) => {
      const day = document.querySelector('[data-testid="scheduled-live-day"]');
      return day && day.value === expected;
    }, { timeout: 10000 }, expectedValue);
  }

  await page.click('[data-testid="scheduled-live-day"]');
  await page.waitForSelector('[role="listbox"][aria-multiselectable="true"]', { timeout: 10000 });
  await toggleScheduledWeekday('FR', 'MO,WE,FR');
  await page.waitForFunction(() => {
    const day = document.querySelector('[data-testid="scheduled-live-day"]');
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    return day && day.value === 'MO,WE,FR' && invokes.some(x => x.cmd === 'update_scheduled_task' &&
      x.args && x.args.input && /BYDAY=MO,WE,FR/.test(x.args.input.rrule || ''));
  }, { timeout: 10000 });

  await toggleScheduledWeekday('WE', 'MO,FR');
  await toggleScheduledWeekday('FR', 'MO');
  await page.waitForFunction(() => {
    const day = document.querySelector('[data-testid="scheduled-live-day"]');
    const monday = document.querySelector('[data-testid="scheduled-live-day-option"][data-value="MO"]');
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    const updates = invokes.filter(x => x.cmd === 'update_scheduled_task' && x.args && x.args.input && x.args.input.rrule);
    const latest = updates.length ? updates[updates.length - 1].args.input.rrule : '';
    return day && day.value === 'MO' && monday && monday.disabled && /BYDAY=MO(?:;|$)/.test(latest);
  }, { timeout: 10000 });
  const lastDayUpdateCount = await page.evaluate(() =>
    window.__scheduledTaskTest.invokes.filter(x => x.cmd === 'update_scheduled_task').length
  );
  await page.evaluate(() =>
    document.querySelector('[data-testid="scheduled-live-day-option"][data-value="MO"]').click()
  );
  await sleep(100);
  const lastDayGuardState = await page.evaluate((before) => {
    const invokes = window.__scheduledTaskTest.invokes;
    const day = document.querySelector('[data-testid="scheduled-live-day"]');
    const monday = document.querySelector('[data-testid="scheduled-live-day-option"][data-value="MO"]');
    const writtenRules = invokes
      .filter(x => x.cmd === 'update_scheduled_task' && x.args && x.args.input && x.args.input.rrule)
      .map(x => x.args.input.rrule);
    return {
      valueStayed: day && day.value === 'MO',
      lastDayDisabled: !!(monday && monday.disabled),
      noExtraUpdate: invokes.filter(x => x.cmd === 'update_scheduled_task').length === before,
      emptyRuleAbsent: writtenRules.every(rule => !/(?:^|;)BYDAY=(?:;|$)/.test(rule)),
    };
  }, lastDayUpdateCount);

  await toggleScheduledWeekday('WE', 'MO,WE');
  await toggleScheduledWeekday('FR', 'MO,WE,FR');

  // 乱序补满七天：写入仍按星期排序，且多选面板不会在命中“工作日/每天”预设时中途消失。
  await toggleScheduledWeekday('SU', 'MO,WE,FR,SU');
  await toggleScheduledWeekday('TU', 'MO,TU,WE,FR,SU');
  await toggleScheduledWeekday('SA', 'MO,TU,WE,FR,SA,SU');
  await toggleScheduledWeekday('TH', 'MO,TU,WE,TH,FR,SA,SU');
  const sevenDayState = await page.evaluate(() => ({
    value: document.querySelector('[data-testid="scheduled-live-day"]') &&
      document.querySelector('[data-testid="scheduled-live-day"]').value,
    repeat: document.querySelector('[data-testid="scheduled-live-repeat"]') &&
      document.querySelector('[data-testid="scheduled-live-repeat"]').value,
    menuStayedOpen: !!document.querySelector('[role="listbox"][aria-multiselectable="true"]'),
  }));
  await page.keyboard.press('Escape');
  await page.waitForFunction(() => {
    const repeat = document.querySelector('[data-testid="scheduled-live-repeat"]');
    return repeat && repeat.value === 'daily' && !document.querySelector('[data-testid="scheduled-live-day"]');
  }, { timeout: 10000 });

  // 从“每天”重新进入每周编辑器时，不应继承七天全选；默认只保留一个明确日期。
  await page.click('[data-testid="scheduled-live-repeat"]');
  await page.waitForSelector('[data-testid="scheduled-live-repeat-option"][data-value="weekly"]', { timeout: 10000 });
  await page.click('[data-testid="scheduled-live-repeat-option"][data-value="weekly"]');
  await page.click('[data-testid="scheduled-live-day"]');
  const weeklyMultiSelectState = await page.evaluate(() => ({
    value: document.querySelector('[data-testid="scheduled-live-day"]') &&
      document.querySelector('[data-testid="scheduled-live-day"]').value,
    menuStayedOpen: !!document.querySelector('[role="listbox"][aria-multiselectable="true"]'),
    selected: Array.from(document.querySelectorAll('[data-testid="scheduled-live-day-option"][aria-selected="true"]'))
      .map(option => option.dataset.value),
  }));
  if (!weeklyMultiSelectState.value || weeklyMultiSelectState.selected.length !== 1) {
    throw new Error('switching from daily to weekly should default to one selected weekday');
  }
  await page.keyboard.press('Escape');

  await page.click('[data-testid="scheduled-live-time"]');
  await page.waitForSelector('[data-testid="scheduled-live-time-hour"]', { timeout: 10000 });
  await page.evaluate(() => document.querySelector('[data-testid="scheduled-live-time-hour"] button[data-value="10"]').click());
  await page.waitForFunction(() => {
    const el = document.querySelector('[data-testid="scheduled-live-time"]');
    return el && el.value.startsWith('10:');
  }, { timeout: 10000 });
  await page.evaluate(() => document.querySelector('[data-testid="scheduled-live-time-minute"] button[data-value="15"]').click());
  await page.waitForFunction(() => {
    const el = document.querySelector('[data-testid="scheduled-live-time"]');
    return el && el.value === '10:15';
  }, { timeout: 10000 });
  await page.keyboard.press('Escape');
  await page.waitForFunction(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    return invokes.some(x => x.cmd === 'update_scheduled_task' && x.args && x.args.input &&
      /BYDAY=[A-Z]{2};BYHOUR=10;BYMINUTE=15/.test(x.args.input.rrule || ''));
  }, { timeout: 10000 });
  const rruleRoundTripState = await page.evaluate(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    const update = invokes.filter(x => x.cmd === 'update_scheduled_task' && x.args && x.args.input && x.args.input.rrule).pop();
    return { rrule: update && update.args.input.rrule };
  });

  await page.evaluate(() => window.TauriBridge.scheduled.createScheduledTask({
    name: '五小时任务', prompt: '每五小时检查一次',
    rrule: 'FREQ=HOURLY;INTERVAL=5', model: '/wire-model', mode: 'agent', paused: false
  }));
  await page.waitForFunction(() => {
    const title = document.querySelector('[data-testid="scheduled-live-title"]');
    return title && title.value === '五小时任务';
  }, { timeout: 10000 });
  const intervalDisplayBefore = await page.evaluate(() => {
    const summaries = Array.from(document.querySelectorAll('[data-testid="scheduled-task-summary"]'));
    const summary = summaries.find(el => (el.textContent || '').includes('每 5 小时'));
    const repeat = document.querySelector('[data-testid="scheduled-live-repeat"]');
    return {
      summary: summary && summary.textContent,
      allSummaries: summaries.map(el => el.textContent),
      repeatLabel: repeat && repeat.textContent.trim(),
      intervalLabel: document.querySelector('[data-testid="scheduled-live-interval"]')?.textContent.trim(),
      intervalValue: document.querySelector('[data-testid="scheduled-live-interval"]')?.value,
      intervalRowPresent: !!document.querySelector('[data-testid="scheduled-live-interval-row"]'),
      timeValue: document.querySelector('[data-testid="scheduled-live-time"]')?.value,
      timePlaceholder: document.querySelector('[data-testid="scheduled-live-time"]')?.placeholder
    };
  });
  await sleep(1200);
  const intervalDisplayAfter = await page.evaluate(() => {
    const summaries = Array.from(document.querySelectorAll('[data-testid="scheduled-task-summary"]'));
    const summary = summaries.find(el => (el.textContent || '').includes('每 5 小时'));
    return { summary: summary && summary.textContent, allSummaries: summaries.map(el => el.textContent) };
  });
  await page.click('[data-testid="scheduled-live-interval"]');
  await page.waitForSelector('[data-testid="scheduled-live-interval-option"][data-value="2"]', { timeout: 10000 });
  await page.click('[data-testid="scheduled-live-interval-option"][data-value="2"]');
  await page.waitForFunction(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    return invokes.some(x => x.cmd === 'update_scheduled_task' && x.args && x.args.input &&
      x.args.input.rrule === 'FREQ=HOURLY;INTERVAL=2');
  }, { timeout: 10000 });
  const intervalEditState = await page.evaluate(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    const update = invokes.filter(x => x.cmd === 'update_scheduled_task' && x.args && x.args.input &&
      x.args.input.rrule === 'FREQ=HOURLY;INTERVAL=2').pop();
    const interval = document.querySelector('[data-testid="scheduled-live-interval"]');
    return {
      rrule: update && update.args.input.rrule,
      intervalLabel: interval && interval.textContent.trim(),
      intervalValue: interval && interval.value,
    };
  });

  await page.click('[data-testid="scheduled-live-time"]');
  await page.waitForSelector('[data-testid="scheduled-live-time-hour"]', { timeout: 10000 });
  await page.evaluate(() => document.querySelector('[data-testid="scheduled-live-time-hour"] button[data-value="10"]').click());
  await page.evaluate(() => document.querySelector('[data-testid="scheduled-live-time-minute"] button[data-value="15"]').click());
  await page.waitForFunction(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    return invokes.some(x => x.cmd === 'update_scheduled_task' && x.args && x.args.input &&
      x.args.input.rrule === 'FREQ=HOURLY;INTERVAL=2;BYHOUR=10;BYMINUTE=15');
  }, { timeout: 10000 });
  const explicitAnchorState = await page.evaluate(() => ({
    value: document.querySelector('[data-testid="scheduled-live-time"]')?.value,
    rrules: (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes || [])
      .filter(x => x.cmd === 'update_scheduled_task' && x.args && x.args.input && x.args.input.rrule)
      .map(x => x.args.input.rrule),
  }));

  await browser.close();
  server.close();

  const pass = navClicked &&
    unrelatedJsonState.hasDraftState === false &&
    unrelatedJsonState.confirmVisible === false &&
    defaultState.navClicked &&
    defaultState.navClicked &&
    defaultState.hasIntro &&
    defaultState.templateCount === 3 &&
    defaultState.hasDailyBrief &&
    defaultState.detailVisible === false &&
    defaultState.listDeleteCount === 0 &&
    defaultState.sampleTextPresent === false &&
    templateCreateState.autoOpenSuppressed &&
    templateCreateState.editorAbsent &&
    templateCreateState.name === '每日早报' &&
    templateCreateState.promptPresent &&
    templateCreateState.editable &&
    templateCreateState.autoOpenSuppressed &&
    templateCreateState.editable &&
    templateCreateState.frequencySectionAbsent &&
    templateCreateState.executionModeAbsent &&
    templateCreateState.permissionControlsAbsent &&
    templateCreateState.insetGroupsNoOuterBorder &&
    templateCreateState.navUnread &&
    templateCreateState.createCalls === 1 &&
    templateCreateState.model === '/wire-model' &&
    templateCreateState.modelId === 'model-active' &&
    templateCreateState.paused === false &&
    workspaceUiAbsent &&
    templateRetainedState.detailHidden &&
    templateRetainedState.count === 3 &&
    templateRetainedState.hasDailyBrief &&
    templateEditState.updateCalls >= 4 &&
    templateEditState.name === '编辑后的每日早报' &&
    templateEditState.prompt === '完全自定义的任务说明' &&
    templateEditState.repeat === 'workdays' &&
    templateEditState.time === '09:30' &&
    hoverStabilityState.sameNode &&
    hoverStabilityState.sameNode &&
    sidebarRecordMenuState.itemVisible &&
    sidebarRecordMenuState.moreVisible &&
    sidebarRecordMenuState.hasRename &&
    sidebarRecordMenuState.hasPin &&
    sidebarRecordMenuState.hasDelete &&
    sidebarRecordMenuState.hasArchive &&
    sidebarRecordPinnedState.pinnedGroupHasRecord &&
    sidebarRecordPinnedState.noTaskPinCommand &&
    sidebarRecordDeleteState.deleteSessionCalls === 0 &&
    sidebarRecordDeleteState.deleteTaskCalls === 0 &&
    sidebarRecordDeleteState.recordStillVisible &&
    blankRequiredState.title === '编辑后的每日早报' &&
    blankRequiredState.blankUpdates === 0 &&
    runChatState.inChatView &&
    runChatState.route === 'scheduled' &&
    runChatState.backVisible &&
    runChatState.transcriptVisible &&
    runChatState.editResendVisible &&
    runChatState.sessionId === 'sched-run-1' &&
    runChatState.taskName === '编辑后的每日早报' &&
    runChatState.model === '/wire-model' &&
    editOpened &&
    editResendState.invoked &&
    editResendState.sessionId === 'sched-run-1' &&
    editResendState.newMessage === 'Run the daily brief again' &&
    editResendState.rerunVisible &&
    editResendState.errorShown === false &&
    failedRunOpenState.stayedInScheduled &&
    failedRunOpenState.route === 'scheduled' &&
    failedRunOpenState.contextAbsent &&
    failedRunOpenState.selectedId === 'task-1' &&
    failedRunOpenState.errorVisible &&
    failedRunOpenState.chatPolluted === false &&
    runReturnState.scheduledVisible &&
    runReturnState.route === 'scheduled' &&
    runReturnState.contextCleared &&
    runReturnState.selectedId === 'task-1' &&
    runReturnState.detailTitle === '编辑后的每日早报' &&
    runReturnState.runHistoryVisible &&
    runReturnState.compactLayout &&
    runReturnState.unreadCleared &&
    runReturnState.navUnreadCleared &&
    pollAfter.taskRefreshes >= 1 &&
    pollAfter.detailRefreshes >= 1 &&
    pollAfter.runRefreshes >= 1 &&
    folderOpenState.automationId === 'task-1' &&
    folderOpenState.menuClosed &&
    runNowFailureState.errorVisible &&
    runNowFailureState.menuVisible === false &&
    runNowFailureState.buttonEnabledAgain &&
    runningSpinnerState.taskSpinner &&
    runningSpinnerState.runSpinner &&
    runningSpinnerState.noChatSpinner &&
    runningSpinnerState.promptAtRun === '点击运行前最后一刻修改的说明' &&
    saveRetryState.errorShown &&
    runningChatState.normalChatVisible &&
    runningChatState.route === 'scheduled' &&
    runningChatState.hasCustomSpinner === false &&
    deletePromptState.deleteCalls === 0 &&
    deletePromptState.promptVisible &&
    deleteCancelState.deleteCalls === 0 &&
    deleteCancelState.detailStillVisible &&
    deleteCancelState.detailStillVisible &&
    deleteConfirmState.deleteCalls === 1 &&
    deleteConfirmState.detailClosed &&
    customCreationClicked &&
    customCreationBeforeSubmit.createCalls === 0 &&
    customCreationBeforeSubmit.submitDisabled &&
    customCreationState.name === 'Custom office brief' &&
    customCreationState.prompt === 'Summarize the important office updates for the day.' &&
    customCreationState.rrule === 'FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=8;BYMINUTE=0' &&
    customCreationState.mode === 'yolo' &&
    customCreationState.model === '/wire-model' &&
    customCreationState.modelId === 'model-active' &&
    customCreationState.paused === false &&
    customCreationState.dialogClosed &&
    openChatClicked &&
    preSend.inChatView &&
    preSend.guidePending &&
    preSend.composerPrefilled &&
    preSend.guideHidden &&
    preSend.confirmVisible === false &&
    preSend.chatCalls === 0 &&
    preSend.createSessionCalls === 0 &&
    preSend.promptCalls === 1 &&
    chatAutoCreateState.scheduledVisible &&
    chatAutoCreateState.route === 'scheduled' &&
    chatAutoCreateState.payloadHasGuide &&
    chatAutoCreateState.toolsRestricted &&
    chatAutoCreateState.guideCleared &&
    chatAutoCreateState.confirmAbsent &&
    chatAutoCreateState.draftStateAbsent &&
    chatAutoCreateState.title === 'AI 招聘情报晨报' &&
    chatAutoCreateState.createCalls === 1 &&
    chatAutoCreateState.chatCalls === 1 &&
    chatAutoCreateState.createSessionCalls === 1 &&
    chatAutoCreateState.promptCalls === 1 &&
    chatAutoCreateState.model === '/wire-model' &&
    chatAutoCreateState.modelId === 'model-active' &&
    chatAutoCreateState.sourceSessionAbsent &&
    chatAutoCreateState.selectedId === 'task-3' &&
    chatAutoCreateState.autoOpenId === 'task-3' &&
    /^[A-Z]{2}$/.test(weeklyMultiSelectState.value || '') &&
    weeklyMultiSelectState.menuStayedOpen &&
    weeklyMultiSelectState.selected.length === 1 &&
    sevenDayState.value === 'MO,TU,WE,TH,FR,SA,SU' &&
    sevenDayState.repeat === 'weekly' &&
    sevenDayState.menuStayedOpen &&
    lastDayGuardState.valueStayed &&
    lastDayGuardState.lastDayDisabled &&
    lastDayGuardState.noExtraUpdate &&
    lastDayGuardState.emptyRuleAbsent &&
    /^FREQ=WEEKLY;BYDAY=[A-Z]{2};BYHOUR=10;BYMINUTE=15$/.test(rruleRoundTripState.rrule || '') &&
    intervalDisplayBefore.repeatLabel === '每小时' &&
    intervalDisplayBefore.intervalLabel === '5 小时' &&
    intervalDisplayBefore.intervalValue === '5' &&
    intervalDisplayBefore.intervalRowPresent &&
    intervalDisplayBefore.timeValue === '' &&
    intervalDisplayBefore.timePlaceholder === '设置起点' &&
    /每 5 小时 · 下次 .*（(?:4小时\d+分|5小时)后）/.test(intervalDisplayBefore.summary || '') &&
    (intervalDisplayAfter.summary || '').split('（')[0] ===
      (intervalDisplayBefore.summary || '').split('（')[0] &&
    /（(?:4小时\d+分|5小时)后）/.test(intervalDisplayAfter.summary || '') &&
    intervalEditState.rrule === 'FREQ=HOURLY;INTERVAL=2' &&
    intervalEditState.intervalLabel === '2 小时' &&
    intervalEditState.intervalValue === '2' &&
    explicitAnchorState.value === '10:15' &&
    explicitAnchorState.rrules.includes('FREQ=HOURLY;INTERVAL=2;BYHOUR=10;BYMINUTE=15') &&
    errors.length === 0;

  if (!pass) {
    console.error('FAIL scheduled tasks UI', JSON.stringify({
      navClicked, unrelatedJsonState, defaultState, templateCreateState, workspaceUiAbsent, templateRetainedState, templateEditState, hoverStabilityState, blankRequiredState, unreadBeforeOpen,
      sidebarRecordMenuState, sidebarRecordPinnedState, sidebarRecordDeleteState,
      failedRunOpenState, runChatState, editOpened, editResendState, runReturnState, pollAfter, folderOpenState, runNowFailureState,
      saveRetryState, runningSpinnerState, runningChatState,
      deletePromptState, deleteCancelState, deleteConfirmState,
      customCreationClicked, customCreationBeforeSubmit, customCreationState, openChatClicked, preSend,
      chatAutoCreateState, weeklyMultiSelectState, sevenDayState, lastDayGuardState, rruleRoundTripState, intervalDisplayBefore, intervalDisplayAfter, intervalEditState, explicitAnchorState, errors
    }, null, 2));
    process.exit(1);
  }
  console.log('PASS scheduled tasks UI');
})();
