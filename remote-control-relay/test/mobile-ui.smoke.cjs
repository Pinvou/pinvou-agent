#!/usr/bin/env node
/**
 * 手机 Remote 用户旅程：真 relay + 真 mobile Web + 模拟桌面 WebSocket。
 *
 * 不启动 Tauri/模型；验证扫码后的浏览器协议、渲染和双向动作。
 * exit 0=PASS, exit 1=FAIL, exit 2=缺 Chromium/puppeteer。
 */
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');
const WebSocket = require('ws');

let puppeteer;
try {
  puppeteer = require('../../pinvou3-app/node_modules/puppeteer-core');
} catch (_) {
  console.error('SKIP: missing pinvou3-app/node_modules/puppeteer-core');
  process.exit(2);
}

const CHROME = process.env.CHROME || [
  '/snap/bin/chromium',
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
  '/usr/bin/google-chrome',
  '/usr/bin/google-chrome-stable',
].find(candidate => fs.existsSync(candidate));
if (!CHROME) {
  console.error('SKIP: missing chromium/chrome');
  process.exit(2);
}

const relayDir = path.resolve(__dirname, '..');
const port = 30_000 + Math.floor(Math.random() * 10_000);
const basePath = '/pinvou3/remote';
const wsUrl = `ws://127.0.0.1:${port}${basePath}/ws`;
const roomId = `mobile_ui_${Date.now()}`;
const token = `token_${Date.now()}_mobile`;
const secret = `secret_${Date.now()}_desktop`;
const sessionId = 'session-mobile-smoke';
const actions = [];
const results = [];
let relay;
let browser;
let desktop;

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

function record(name, pass, detail = '') {
  results.push({ name, pass });
  console.log(`${pass ? '✅' : '❌'} ${name}${detail ? `  ${detail}` : ''}`);
  assert.ok(pass, name);
}

function waitForOutput(child, pattern, timeoutMs = 5000) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`relay startup timeout: ${pattern}`)), timeoutMs);
    const onData = chunk => {
      if (!pattern.test(String(chunk))) return;
      clearTimeout(timer);
      child.stdout.off('data', onData);
      resolve();
    };
    child.stdout.on('data', onData);
    child.once('exit', code => {
      clearTimeout(timer);
      reject(new Error(`relay exited during startup: ${code}`));
    });
  });
}

function openSocket() {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(wsUrl);
    ws.once('open', () => resolve(ws));
    ws.once('error', reject);
  });
}

function nextSocketMessage(ws, type, timeoutMs = 5000) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      ws.off('message', onMessage);
      reject(new Error(`timeout waiting for desktop ${type}`));
    }, timeoutMs);
    const onMessage = raw => {
      const msg = JSON.parse(String(raw));
      if (msg.type !== type) return;
      clearTimeout(timer);
      ws.off('message', onMessage);
      resolve(msg);
    };
    ws.on('message', onMessage);
  });
}

function sendDesktop(ws, type, payload = {}, targetSession = sessionId) {
  ws.send(JSON.stringify({
    type,
    room_id: roomId,
    session_id: targetSession,
    payload,
  }));
}

function sessionSnapshot() {
  return {
    snapshot_source: 'live',
    session: {
      id: sessionId,
      title: '手机远控自动化会话',
      mode: 'yolo',
      status: 'idle',
      message_count: 2,
      updated_at: new Date().toISOString(),
    },
    messages: [
      { id: 'm1', role: 'user', content: '历史问题', tools: [], blocks: [{ type: 'text', text: '历史问题' }] },
      { id: 'm2', role: 'assistant', content: '历史回答', tools: [], blocks: [{ type: 'text', text: '历史回答' }] },
    ],
    chat_items: [],
    pending_user_inputs: [],
    running_tools: [],
    artifacts: [{
      id: 'artifact-1',
      basename: 'report.md',
      path_tail: 'artifacts/report.md',
      kind: 'Markdown',
      byte_size: 128,
    }],
    busy: false,
  };
}

function chips(mode = 'yolo') {
  return {
    mode,
    model_id: null,
    effective_model_id: 'model-local',
    effective_model_name: 'Qwen Local',
    global_model_id: 'model-local',
    models: [{ id: 'model-local', name: 'Qwen Local', model: 'qwen-local' }],
  };
}

function attachDesktopResponder(ws) {
  ws.on('message', raw => {
    const msg = JSON.parse(String(raw));
    if (msg.type !== 'mobile_action') return;
    const action = msg.payload || {};
    actions.push(action);
    switch (action.type) {
      case 'request_session_list':
        sendDesktop(ws, 'session_list', {
          active_session_id: sessionId,
          sessions: [
            { id: sessionId, title: '手机远控自动化会话', message_count: 2, active: true },
            { id: 'session-other', title: '另一个会话', message_count: 1, active: false },
          ],
        });
        break;
      case 'request_snapshot':
        sendDesktop(ws, 'session_snapshot', sessionSnapshot());
        break;
      case 'request_chips':
        sendDesktop(ws, 'chips_snapshot', chips());
        break;
      case 'request_artifacts':
        sendDesktop(ws, 'artifact_list', { artifacts: sessionSnapshot().artifacts });
        break;
      case 'request_artifact_preview':
        sendDesktop(ws, 'artifact_preview', {
          artifact_id: 'artifact-1',
          basename: 'report.md',
          path_tail: 'artifacts/report.md',
          preview: { type: 'markdown', content: '# 手机产物预览\n\n内容可读。' },
        });
        break;
      case 'set_mode':
        sendDesktop(ws, 'chips_snapshot', chips(action.payload && action.payload.mode));
        break;
      default:
        break;
    }
  });
}

async function connectDesktop() {
  const ws = await openSocket();
  attachDesktopResponder(ws);
  const registered = nextSocketMessage(ws, 'room_registered');
  ws.send(JSON.stringify({
    type: 'desktop_register',
    room_id: roomId,
    session_id: sessionId,
    pairing_token: token,
    desktop_secret: secret,
  }));
  await registered;
  return ws;
}

async function waitForAction(type, startAt = 0, predicate = () => true, timeoutMs = 5000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const found = actions.slice(startAt).find(action => action.type === type && predicate(action));
    if (found) return found;
    await sleep(25);
  }
  throw new Error(`timeout waiting for mobile action ${type}`);
}

async function clickExact(page, selector, text) {
  return page.evaluate((sel, expected) => {
    const node = [...document.querySelectorAll(sel)].find(item => (item.textContent || '').trim() === expected);
    if (!node) return false;
    node.click();
    return true;
  }, selector, text);
}

async function clickSheetItem(page, title) {
  return page.evaluate(expected => {
    const node = [...document.querySelectorAll('#sheetBody .sheet-item')]
      .find(item => (item.querySelector('.sheet-title')?.textContent || '').trim() === expected);
    if (!node) return false;
    node.click();
    return true;
  }, title);
}

async function main() {
  relay = spawn(process.execPath, [path.join(relayDir, 'server.js')], {
    cwd: relayDir,
    env: {
      ...process.env,
      PORT: String(port),
      PINVOU_REMOTE_PUBLIC_BASE_PATH: basePath,
      DESKTOP_RECONNECT_GRACE_MS: '3000',
      HEARTBEAT_INTERVAL_MS: '5000',
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  await waitForOutput(relay, /pinvou remote relay listening/);
  desktop = await connectDesktop();

  browser = await puppeteer.launch({
    executablePath: CHROME,
    headless: 'new',
    args: ['--no-sandbox', '--disable-gpu', '--no-first-run', '--no-default-browser-check'],
    userDataDir: fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-remote-mobile-')),
  });
  const page = await browser.newPage();
  await page.setViewport({ width: 390, height: 844, deviceScaleFactor: 1 });
  const pageErrors = [];
  page.on('pageerror', error => pageErrors.push(error.message));
  await page.goto(`http://127.0.0.1:${port}${basePath}/r/${roomId}#token=${encodeURIComponent(token)}`, {
    waitUntil: 'networkidle0',
  });

  await page.waitForFunction(() => document.querySelector('#status')?.textContent === '已连接'
    && document.body.innerText.includes('手机远控自动化会话'), { timeout: 10000 });
  record('① 手机使用 fragment token 配对并加载 Session 列表',
    await page.evaluate(() => document.querySelector('#sheetTitle')?.textContent === '选择 Session'
      && !document.querySelector('#sheetOverlay')?.classList.contains('hidden')));

  record('② Session 选择面板同时展示当前与其他会话',
    await page.evaluate(() => document.body.innerText.includes('手机远控自动化会话')
      && document.body.innerText.includes('另一个会话')));
  assert.equal(await clickSheetItem(page, '手机远控自动化会话'), true);
  await page.waitForFunction(() => document.body.innerText.includes('历史问题')
    && document.body.innerText.includes('历史回答')
    && document.querySelector('#artifactTextChip')?.textContent === '1 个产物', { timeout: 10000 })
    .catch(async error => {
      const debug = await page.evaluate(() => ({
        status: document.querySelector('#status')?.textContent,
        title: document.querySelector('#title')?.textContent,
        artifacts: document.querySelector('#artifactTextChip')?.textContent,
        body: document.body.innerText.slice(0, 1200),
      }));
      console.error('session restore debug:', JSON.stringify({ actions, debug }));
      throw error;
    });
  record('③ 进入 Session 后恢复历史、模式、模型与产物摘要',
    await page.evaluate(() => document.querySelector('#modeChip')?.textContent.includes('YOLO')
      && document.querySelector('#modelChip')?.textContent.includes('Qwen Local')));

  const beforeMessage = actions.length;
  await page.type('#input', '手机自动化消息');
  await page.click('#actionButton');
  const userMessage = await waitForAction('user_message', beforeMessage);
  const clientId = userMessage.client_message_id;
  record('④ 手机消息携带 client_message_id 发往桌面',
    userMessage.payload?.content === '手机自动化消息' && /^cm_/.test(clientId || ''), clientId || '');

  // 桌面回显同一 client id 时，手机不能再插入一条重复用户气泡。
  sendDesktop(desktop, 'message_append', {
    role: 'user', content: '手机自动化消息', client_message_id: clientId,
  });
  sendDesktop(desktop, 'assistant_delta', { text: '远程回答第一段' });
  sendDesktop(desktop, 'assistant_delta', { text: '，第二段。' });
  sendDesktop(desktop, 'session_status', { status: '空闲' });
  await page.waitForFunction(() => document.body.innerText.includes('远程回答第一段，第二段。'));
  const messageState = await page.evaluate(() => ({
    userCopies: [...document.querySelectorAll('.msg.user .bubble')]
      .filter(node => node.textContent === '手机自动化消息').length,
    answer: document.body.innerText.includes('远程回答第一段，第二段。'),
  }));
  record('⑤ 桌面增量回复实时渲染且用户消息回显去重',
    messageState.userCopies === 1 && messageState.answer, JSON.stringify(messageState));

  sendDesktop(desktop, 'tool_call_start', {
    id: 'tool-1', name: 'write_file', args: { path: 'artifacts/report.md' },
  });
  sendDesktop(desktop, 'tool_call_end', {
    id: 'tool-1', name: 'write_file', args: { path: 'artifacts/report.md' }, success: true,
  });
  sendDesktop(desktop, 'tool_call_end', {
    id: 'present-1', name: 'mcp_pinvou3_present_artifact',
    args: { path: 'artifacts/report.md' }, success: true,
  });
  await page.waitForFunction(() => document.querySelector('.tool-card .tool-status')?.textContent === '完成'
    && document.querySelector('.artifact')?.textContent.includes('report.md'));
  record('⑥ 工具状态与 present_artifact 成品卡实时渲染', true);

  const beforeInput = actions.length;
  sendDesktop(desktop, 'user_input_required', {
    id: 'rui-1',
    questions: [{
      id: 'approve', question: '是否批准执行？',
      options: [{ label: '批准', value: 'yes', description: '继续执行' }],
    }],
  });
  await page.waitForFunction(() => document.body.innerText.includes('是否批准执行？'));
  assert.equal(await clickExact(page, '.choice', '批准继续执行'), true);
  assert.equal(await clickExact(page, '.card .primary', '提交选择'), true);
  const submitted = await waitForAction('submit_user_input', beforeInput);
  record('⑦ request_user_input 可在手机选择并提交',
    submitted.payload?.tool_call_id === 'rui-1'
      && submitted.payload?.answers?.[0]?.value === 'yes');

  const beforePlan = actions.length;
  sendDesktop(desktop, 'plan_ready', {
    plan_snapshot: {
      explanation: '远程方案',
      items: [{ step: '先检查数据', status: 'pending' }],
    },
    todos_snapshot: null,
  });
  await page.waitForFunction(() => document.body.innerText.includes('先检查数据'));
  assert.equal(await clickExact(page, '.plan-card button', '就这么干'), true);
  const accepted = await waitForAction('accept_plan', beforePlan);
  record('⑧ Plan 卡可在手机批准并回传方案',
    accepted.payload?.plan_markdown?.includes('先检查数据'));

  const beforeMode = actions.length;
  await page.click('#modeChip');
  await page.waitForFunction(() => document.querySelector('#sheetTitle')?.textContent === '切换 Mode');
  assert.equal(await clickSheetItem(page, 'Plan'), true);
  const modeAction = await waitForAction('set_mode', beforeMode);
  await page.waitForFunction(() => document.querySelector('#modeChip')?.textContent.includes('Plan'));
  record('⑨ 手机可切换 Plan/YOLO 模式', modeAction.payload?.mode === 'plan');

  const beforePreview = actions.length;
  await page.click('#artifactTextChip');
  await page.waitForFunction(() => document.querySelector('#sheetTitle')?.textContent === '产物');
  assert.equal(await clickSheetItem(page, 'report.md'), true);
  const previewAction = await waitForAction('request_artifact_preview', beforePreview);
  await page.waitForFunction(() => document.querySelector('#previewBody')?.textContent.includes('手机产物预览'));
  record('⑩ 手机只能按 artifact id 请求并展示受限预览',
    previewAction.payload?.artifact_id === 'artifact-1');

  desktop.terminate();
  await page.waitForFunction(() => document.querySelector('#status')?.textContent === '桌面重连中', { timeout: 5000 });
  const reconnectText = await page.evaluate(() => document.body.innerText.includes('桌面连接暂时中断'));
  desktop = await connectDesktop();
  await page.waitForFunction(() => document.querySelector('#status')?.textContent !== '桌面重连中'
    && document.querySelector('#input')?.disabled === false, { timeout: 5000 });
  record('⑪ 桌面宽限期重连时手机暂停操作并恢复', reconnectText);

  desktop.send(JSON.stringify({ type: 'desktop_disconnect', payload: { reason: 'qr_refreshed' } }));
  await page.waitForFunction(() => !document.querySelector('#takeoverScreen')?.classList.contains('hidden')
    && document.body.innerText.includes('远程连接已结束'), { timeout: 5000 });
  record('⑫ 刷新二维码后旧手机进入连接结束页', true);

  record('⑬ 全程无手机页面运行时错误', pageErrors.length === 0, pageErrors.slice(0, 2).join(' | '));
  console.log(`\n✅ ALL ${results.length} REMOTE MOBILE JOURNEYS PASS`);
}

main().catch(error => {
  console.error('FATAL mobile remote smoke:', error.stack || error.message);
  process.exitCode = 1;
}).finally(async () => {
  try { desktop?.close(); } catch (_) {}
  try { await browser?.close(); } catch (_) {}
  try { relay?.kill('SIGTERM'); } catch (_) {}
});
