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
      scheduledFor: '2026-07-10T08:00:00Z', createdAt: '2026-07-10T08:00:00Z', unread: true
    }];
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
          models: [{ id: 'model-active', name: 'Smoke Model', model: '/wire-model' }],
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
        case 'list_workflows': return Promise.resolve([]);
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
        case 'load_session':
          if (args && args.id === 'sched-run-1') {
            return Promise.resolve({
              metadata: { id: 'sched-run-1', title: 'Daily brief run' },
              messages: [
                { role: 'user', content: [{ type: 'text', text: 'Run the daily brief' }] },
                { role: 'assistant', content: [{ type: 'text', text: 'Daily brief complete' }] }
              ],
              artifacts: []
            });
          }
          return Promise.resolve({ metadata: { id: args && args.id, title: 'New chat' }, messages: [], artifacts: [] });
        case 'create_session': {
          const meta = { id: 'session-' + (++SESSION_SEQ), title: '新对话', createdAt: new Date().toISOString() };
          SESSIONS.unshift(meta);
          return Promise.resolve(meta);
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
            '  "cwds": [],',
            '  "allowShell": false,',
            '  "trustMode": false,',
            '  "autoApprove": false,',
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
        case 'create_scheduled_task': {
          const input = args && args.input || {};
          const minuteMatch = /FREQ=MINUTELY;INTERVAL=([0-9]+)/.exec(input.rrule || '');
          const nextDelayMs = minuteMatch ? Number(minuteMatch[1]) * 60000 : 4 * 86400000;
          const created = Object.assign({}, input, {
            id: 'task-' + (++TASK_SEQ),
            scheduleLabel: minuteMatch
              ? (Number(minuteMatch[1]) === 1 ? '每分钟' : '每 ' + Number(minuteMatch[1]) + ' 分钟')
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
        }
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

  await page.evaluate(() => window.TauriBridge.sendMessage('普通聊天 JSON 回归测试'));
  await page.waitForFunction(() => {
    const state = window.TauriBridge && window.TauriBridge.getState ? window.TauriBridge.getState() : {};
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    return invokes.some(x => x.cmd === 'chat') && state && state.busy === false;
  }, { timeout: 10000 });
  await sleep(200);
  const unrelatedJsonState = await page.evaluate(() => {
    const state = window.TauriBridge && window.TauriBridge.getState ? window.TauriBridge.getState() : {};
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

  const navClicked = await clickExactText(page, '定时任务');
  await sleep(500);
  const defaultState = await page.evaluate(() => ({
    navClicked: !!document.querySelector('[data-testid="scheduled-page"]'),
    hasTitle: document.body.innerText.includes('定时任务'),
    templateCount: document.querySelectorAll('button[data-testid^="scheduled-template-"]').length,
    hasDailyBrief: !!document.querySelector('[data-testid="scheduled-template-daily-brief"]'),
    detailVisible: !!document.querySelector('[data-testid="scheduled-detail"]'),
    listDeleteCount: document.querySelectorAll('[data-testid="scheduled-list-delete"]').length,
    sampleTextPresent: /每日项目状态提醒|每周资料整理提醒|项目A/.test(document.body.innerText)
  }));
  await page.evaluate(() => { window.__scheduledTaskTest.invokes = []; });
  await page.click('[data-testid="scheduled-template-daily-brief"]');
  await page.waitForFunction(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    return invokes.filter(x => x.cmd === 'create_scheduled_task').length === 1 &&
      !!document.querySelector('[data-testid="scheduled-live-title"]');
  }, { timeout: 10000 });
  const templateCreateState = await page.evaluate(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    const create = invokes.find(x => x.cmd === 'create_scheduled_task');
    const name = document.querySelector('[data-testid="scheduled-live-title"]');
    const prompt = document.querySelector('[data-testid="scheduled-live-prompt"]');
    return {
      editorAbsent: !document.querySelector('[data-testid="scheduled-draft-editor"]') && !document.querySelector('[data-testid="scheduled-draft-confirm"]'),
      name: name && name.value,
      promptPresent: !!(prompt && prompt.value),
      editable: !!(name && prompt && !name.disabled && !name.readOnly && !prompt.disabled && !prompt.readOnly),
      navUnread: !!document.querySelector('[data-testid="scheduled-nav-unread"]'),
      createCalls: invokes.filter(x => x.cmd === 'create_scheduled_task').length,
      model: create && create.args && create.args.input && create.args.input.model,
      paused: create && create.args && create.args.input && create.args.input.paused
    };
  });
  await page.evaluate(() => { window.__scheduledTaskTest.folderResult = 'D:/picked-workspace'; });
  await page.click('[data-testid="scheduled-detail-pick-folder"]');
  await page.waitForFunction(() => {
    const input = document.querySelector('[data-testid="scheduled-live-project"]');
    return input && input.value === 'D:/picked-workspace';
  }, { timeout: 10000 });
  const folderPickerState = await page.evaluate(() => {
    const calls = window.__scheduledTaskTest.dialogCalls || [];
    const input = document.querySelector('[data-testid="scheduled-live-project"]');
    return {
      path: input && input.value,
      options: calls[calls.length - 1] || null
    };
  });
  await page.evaluate(() => {
    const inputSetter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    const textareaSetter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
    const name = document.querySelector('[data-testid="scheduled-live-title"]');
    const prompt = document.querySelector('[data-testid="scheduled-live-prompt"]');
    inputSetter.call(name, '编辑后的每日简报');
    name.dispatchEvent(new Event('input', { bubbles: true }));
    textareaSetter.call(prompt, '完全自定义的任务说明');
    prompt.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await page.waitForFunction(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    const updates = invokes.filter(x => x.cmd === 'update_scheduled_task');
    return updates.some(x => x.args && x.args.input && x.args.input.name === '编辑后的每日简报') &&
      updates.some(x => x.args && x.args.input && x.args.input.prompt === '完全自定义的任务说明');
  }, { timeout: 10000 });
  await page.click('[data-testid="scheduled-live-repeat"]');
  await page.click('[data-testid="scheduled-live-repeat-option"][data-value="daily"]');
  await page.evaluate(() => {
    const inputSetter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    const time = document.querySelector('[data-testid="scheduled-live-time"]');
    inputSetter.call(time, '09:30');
    time.dispatchEvent(new Event('input', { bubbles: true }));
    time.dispatchEvent(new Event('change', { bubbles: true }));
  });
  await page.waitForFunction(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    const updates = invokes.filter(x => x.cmd === 'update_scheduled_task');
    return updates.some(x => x.args && x.args.input && x.args.input.name === '编辑后的每日简报') &&
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
      time: document.querySelector('[data-testid="scheduled-live-time"]') && document.querySelector('[data-testid="scheduled-live-time"]').value,
      selectedTemplateHidden: !document.querySelector('[data-testid="scheduled-template-daily-brief"]'),
      remainingTemplateCount: document.querySelectorAll('button[data-testid^="scheduled-template-"]').length
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
    const state = window.TauriBridge.getState();
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
    const state = window.TauriBridge && window.TauriBridge.getState ? window.TauriBridge.getState() : {};
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
  await page.click('[data-testid="scheduled-run-back"]');
  await page.waitForFunction(() => {
    const title = document.querySelector('[data-testid="scheduled-live-title"]');
    return !!document.querySelector('[data-testid="scheduled-page"]') &&
      !!document.querySelector('button[data-testid="scheduled-run-row"]') &&
      title && title.value === '编辑后的每日简报';
  }, { timeout: 10000 });
  const runReturnState = await page.evaluate(() => {
    const state = window.TauriBridge && window.TauriBridge.getState ? window.TauriBridge.getState() : {};
    return {
      scheduledVisible: !!document.querySelector('[data-testid="scheduled-page"]'),
      route: document.querySelector('[data-testid="app-root"]') && document.querySelector('[data-testid="app-root"]').dataset.currentView,
      contextCleared: !state.scheduledRunContext,
      selectedId: state.selectedScheduledTaskId,
      detailTitle: document.querySelector('[data-testid="scheduled-live-title"]') && document.querySelector('[data-testid="scheduled-live-title"]').value,
      runHistoryVisible: !!document.querySelector('button[data-testid="scheduled-run-row"]'),
      compactLayout: !!document.querySelector('[data-testid="scheduled-detail-prompt"]') &&
        !!document.querySelector('[data-testid="scheduled-detail-settings"]') &&
        !!document.querySelector('[data-testid="scheduled-detail-frequency"]'),
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

  await page.evaluate(() => { window.__scheduledTaskTest.failures.run_scheduled_task_now = 'run now failed visibly'; });
  await page.click('[data-testid="scheduled-run-now"]');
  await page.waitForFunction(() => document.body.innerText.includes('run now failed visibly'), { timeout: 10000 });
  const runNowFailureState = await page.evaluate(() => ({
    errorVisible: document.body.innerText.includes('run now failed visibly'),
    buttonEnabledAgain: !document.querySelector('[data-testid="scheduled-run-now"]').disabled
  }));
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
    const state = window.TauriBridge.getState();
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
  await page.click('[data-testid="scheduled-list-delete"]');
  await page.waitForSelector('[data-testid="scheduled-delete-confirmation"]', { timeout: 10000 });
  const deletePromptState = await page.evaluate(() => ({
    deleteCalls: window.__scheduledTaskTest.invokes.filter(x => x.cmd === 'delete_scheduled_task').length,
    promptVisible: !!document.querySelector('[data-testid="scheduled-delete-confirmation"]')
  }));
  await page.click('[data-testid="scheduled-delete-cancel"]');
  await sleep(150);
  const deleteCancelState = await page.evaluate(() => ({
    deleteCalls: window.__scheduledTaskTest.invokes.filter(x => x.cmd === 'delete_scheduled_task').length,
    detailStillVisible: !!document.querySelector('[data-testid="scheduled-detail"]'),
    promptHidden: !document.querySelector('[data-testid="scheduled-delete-confirmation"]')
  }));

  await page.click('[data-testid="scheduled-list-delete"]');
  await page.waitForSelector('[data-testid="scheduled-delete-confirmation"]', { timeout: 10000 });
  await page.evaluate(() => {
    window.__scheduledTaskTest.failures.delete_scheduled_task = 'delete failed visibly';
  });
  await page.click('[data-testid="scheduled-delete-confirm"]');
  await page.waitForFunction(() => document.body.innerText.includes('delete failed visibly'), { timeout: 10000 });
  const deleteFailureState = await page.evaluate(() => ({
    errorVisible: document.body.innerText.includes('delete failed visibly'),
    deleteCalls: window.__scheduledTaskTest.invokes.filter(x => x.cmd === 'delete_scheduled_task').length,
    taskStillPresent: !!document.querySelector('[data-testid="scheduled-detail"]')
  }));
  await page.evaluate(() => { delete window.__scheduledTaskTest.failures.delete_scheduled_task; });
  await page.click('[data-testid="scheduled-delete-confirm"]');
  await page.waitForFunction(() => !document.querySelector('[data-testid="scheduled-detail"]'), { timeout: 10000 });
  const deleteConfirmState = await page.evaluate(() => ({
    deleteCalls: window.__scheduledTaskTest.invokes.filter(x => x.cmd === 'delete_scheduled_task').length,
    detailClosed: !document.querySelector('[data-testid="scheduled-detail"]')
  }));

  await page.evaluate(() => { window.__scheduledTaskTest.invokes = []; });
  await page.click('[data-testid="scheduled-create-menu"]');
  const openChatClicked = await clickExactText(page, '通过聊天创建');
  // 新流程:点击只预填输入框,不自动发送 —— 先等引导词就位。
  await page.waitForFunction(() => {
    const state = window.TauriBridge && window.TauriBridge.getState ? window.TauriBridge.getState() : {};
    return !!state.scheduledTaskPendingGuide;
  }, { timeout: 10000 });
  await sleep(500);
  const preSend = await page.evaluate(() => {
    const state = window.TauriBridge && window.TauriBridge.getState ? window.TauriBridge.getState() : {};
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

  await page.evaluate(() => window.TauriBridge.sendMessage('我想创建一个定时任务：工作日每天早上 8 点半做 AI 招聘情报晨报'));
  await page.waitForFunction(() => {
    const state = window.TauriBridge && window.TauriBridge.getState ? window.TauriBridge.getState() : {};
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    const title = document.querySelector('[data-testid="scheduled-live-title"]');
    return state.scheduledTaskAutoOpenId &&
      invokes.filter(x => x.cmd === 'create_scheduled_task').length === 1 &&
      !!document.querySelector('[data-testid="scheduled-page"]') &&
      title && title.value === 'AI 招聘情报晨报';
  }, { timeout: 10000 });
  const chatAutoCreateState = await page.evaluate(() => {
    const state = window.TauriBridge && window.TauriBridge.getState ? window.TauriBridge.getState() : {};
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
      sourceSessionAbsent: !!(input && !Object.prototype.hasOwnProperty.call(input, 'sourceSessionId')),
      selectedId: state.selectedScheduledTaskId,
      autoOpenId: state.scheduledTaskAutoOpenId
    };
  });
  await page.evaluate(() => {
    const inputSetter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    const time = document.querySelector('[data-testid="scheduled-live-time"]');
    inputSetter.call(time, '10:15');
    time.dispatchEvent(new Event('input', { bubbles: true }));
    time.dispatchEvent(new Event('change', { bubbles: true }));
  });
  await page.waitForFunction(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    return invokes.some(x => x.cmd === 'update_scheduled_task' && x.args && x.args.input &&
      /BYDAY=MO,WE;BYHOUR=10;BYMINUTE=15/.test(x.args.input.rrule || ''));
  }, { timeout: 10000 });
  const rruleRoundTripState = await page.evaluate(() => {
    const invokes = (window.__scheduledTaskTest && window.__scheduledTaskTest.invokes) || [];
    const update = invokes.filter(x => x.cmd === 'update_scheduled_task' && x.args && x.args.input && x.args.input.rrule).pop();
    return { rrule: update && update.args.input.rrule };
  });

  await page.evaluate(() => window.TauriBridge.createScheduledTask({
    name: '五分钟任务', prompt: '每五分钟检查一次',
    rrule: 'FREQ=MINUTELY;INTERVAL=5', cwds: ['D:/picked-workspace'], model: '/wire-model', mode: 'agent',
    allowShell: false, trustMode: false, autoApprove: false, paused: false
  }));
  await page.waitForFunction(() => {
    const title = document.querySelector('[data-testid="scheduled-live-title"]');
    return title && title.value === '五分钟任务';
  }, { timeout: 10000 });
  const intervalDisplayBefore = await page.evaluate(() => {
    const summaries = Array.from(document.querySelectorAll('[data-testid="scheduled-task-summary"]'));
    const summary = summaries.find(el => (el.textContent || '').includes('每 5 分钟'));
    const repeat = document.querySelector('[data-testid="scheduled-live-repeat"]');
    return {
      summary: summary && summary.textContent,
      allSummaries: summaries.map(el => el.textContent),
      repeatLabel: repeat && repeat.textContent.trim(),
      timeInputAbsent: !document.querySelector('[data-testid="scheduled-live-time"]')
    };
  });
  await sleep(1200);
  const intervalDisplayAfter = await page.evaluate(() => {
    const summaries = Array.from(document.querySelectorAll('[data-testid="scheduled-task-summary"]'));
    const summary = summaries.find(el => (el.textContent || '').includes('每 5 分钟'));
    return { summary: summary && summary.textContent, allSummaries: summaries.map(el => el.textContent) };
  });

  await browser.close();
  server.close();

  const pass = navClicked &&
    unrelatedJsonState.hasDraftState === false &&
    unrelatedJsonState.confirmVisible === false &&
    defaultState.navClicked &&
    defaultState.hasTitle &&
    defaultState.templateCount === 3 &&
    defaultState.hasDailyBrief &&
    defaultState.detailVisible === false &&
    defaultState.listDeleteCount === 0 &&
    defaultState.sampleTextPresent === false &&
    templateCreateState.editorAbsent &&
    templateCreateState.name === '每日简报' &&
    templateCreateState.promptPresent &&
    templateCreateState.editable &&
    templateCreateState.navUnread &&
    templateCreateState.createCalls === 1 &&
    templateCreateState.model === '/wire-model' &&
    templateCreateState.paused === true &&
    folderPickerState.path === 'D:/picked-workspace' &&
    folderPickerState.options && folderPickerState.options.directory === true &&
    folderPickerState.options.multiple === false &&
    templateEditState.updateCalls >= 4 &&
    templateEditState.name === '编辑后的每日简报' &&
    templateEditState.prompt === '完全自定义的任务说明' &&
    templateEditState.repeat === 'daily' &&
    templateEditState.time === '09:30' &&
    templateEditState.selectedTemplateHidden &&
    templateEditState.remainingTemplateCount === 2 &&
    hoverStabilityState.sameNode &&
    hoverStabilityState.hovered &&
    blankRequiredState.title === '编辑后的每日简报' &&
    blankRequiredState.blankUpdates === 0 &&
    unreadBeforeOpen.task &&
    unreadBeforeOpen.run &&
    runChatState.inChatView &&
    runChatState.route === 'scheduled' &&
    runChatState.backVisible &&
    runChatState.transcriptVisible &&
    runChatState.editResendVisible &&
    runChatState.sessionId === 'sched-run-1' &&
    runChatState.taskName === '编辑后的每日简报' &&
    runChatState.model === '/wire-model' &&
    failedRunOpenState.stayedInScheduled &&
    failedRunOpenState.route === 'scheduled' &&
    failedRunOpenState.contextAbsent &&
    failedRunOpenState.selectedId === 'task-1' &&
    failedRunOpenState.errorVisible &&
    failedRunOpenState.taskUnread &&
    failedRunOpenState.runUnread &&
    failedRunOpenState.chatPolluted === false &&
    runReturnState.scheduledVisible &&
    runReturnState.route === 'scheduled' &&
    runReturnState.contextCleared &&
    runReturnState.selectedId === 'task-1' &&
    runReturnState.detailTitle === '编辑后的每日简报' &&
    runReturnState.runHistoryVisible &&
    runReturnState.compactLayout &&
    runReturnState.unreadCleared &&
    runReturnState.navUnreadCleared &&
    pollAfter.taskRefreshes >= 1 &&
    pollAfter.detailRefreshes >= 1 &&
    pollAfter.runRefreshes >= 1 &&
    runNowFailureState.errorVisible &&
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
    deleteCancelState.promptHidden &&
    deleteFailureState.errorVisible &&
    deleteFailureState.deleteCalls === 1 &&
    deleteFailureState.taskStillPresent &&
    deleteConfirmState.deleteCalls === 2 &&
    deleteConfirmState.detailClosed &&
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
    chatAutoCreateState.sourceSessionAbsent &&
    chatAutoCreateState.selectedId === 'task-2' &&
    chatAutoCreateState.autoOpenId === 'task-2' &&
    rruleRoundTripState.rrule === 'FREQ=WEEKLY;BYDAY=MO,WE;BYHOUR=10;BYMINUTE=15' &&
    intervalDisplayBefore.repeatLabel === '每 5 分钟' &&
    intervalDisplayBefore.timeInputAbsent &&
    /每 5 分钟 · 下次 .*（(?:4|5)分\d+秒后）/.test(intervalDisplayBefore.summary || '') &&
    intervalDisplayAfter.summary && intervalDisplayAfter.summary !== intervalDisplayBefore.summary &&
    errors.length === 0;

  if (!pass) {
    console.error('FAIL scheduled tasks UI', JSON.stringify({
      navClicked, unrelatedJsonState, defaultState, templateCreateState, folderPickerState, templateEditState, hoverStabilityState, blankRequiredState, unreadBeforeOpen,
      failedRunOpenState, runChatState, runReturnState, pollAfter, runNowFailureState,
      saveRetryState, runningSpinnerState, runningChatState,
      deletePromptState, deleteCancelState, deleteFailureState, deleteConfirmState, openChatClicked, preSend,
      chatAutoCreateState, rruleRoundTripState, intervalDisplayBefore, intervalDisplayAfter, errors
    }, null, 2));
    process.exit(1);
  }
  console.log('PASS scheduled tasks UI');
})();
