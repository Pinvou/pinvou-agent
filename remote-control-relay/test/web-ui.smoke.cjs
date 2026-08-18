#!/usr/bin/env node
/**
 * Full WebUI v2 user journey: real relay + built shared UI + simulated desktop.
 *
 * The test deliberately does not start Tauri or a model. It verifies the browser
 * boundary and the endpoint-scoped Relay protocol used by both desktop-sized and
 * mobile-sized browsers.
 *
 * exit 0 = PASS, exit 1 = FAIL, exit 2 = Chromium/puppeteer unavailable.
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

const windowsChrome = process.platform === 'win32' ? [
  process.env.LOCALAPPDATA && path.join(process.env.LOCALAPPDATA, 'Google/Chrome/Application/chrome.exe'),
  process.env.PROGRAMFILES && path.join(process.env.PROGRAMFILES, 'Google/Chrome/Application/chrome.exe'),
  process.env['PROGRAMFILES(X86)'] && path.join(process.env['PROGRAMFILES(X86)'], 'Google/Chrome/Application/chrome.exe'),
  process.env.PROGRAMFILES && path.join(process.env.PROGRAMFILES, 'Microsoft/Edge/Application/msedge.exe'),
  process.env['PROGRAMFILES(X86)'] && path.join(process.env['PROGRAMFILES(X86)'], 'Microsoft/Edge/Application/msedge.exe'),
] : [];
const CHROME = process.env.CHROME || [
  ...windowsChrome,
  '/snap/bin/chromium',
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
  '/usr/bin/google-chrome',
  '/usr/bin/google-chrome-stable',
].filter(Boolean).find(candidate => fs.existsSync(candidate));
if (!CHROME) {
  console.error('SKIP: missing chromium/chrome/edge (set CHROME to an executable path)');
  process.exit(2);
}

const relayDir = path.resolve(__dirname, '..');
const webDist = path.join(relayDir, 'web', 'dist');
const webIndex = path.join(webDist, 'index.html');
if (!fs.existsSync(webIndex)) {
  console.error('FAIL: WebUI dist is missing; run `npm --prefix pinvou3-app run build:web` first');
  process.exit(1);
}
const webPolicy = JSON.parse(fs.readFileSync(path.join(webDist, 'platform', 'web', 'access-policy.json'), 'utf8'));

// 端口在 main() 开头让 OS 分配空闲端口(见 allocatePort)。此前在 30000-39999
// 随机取值,与 Linux 临时端口段(ip_local_port_range 默认 32768 起)重叠,
// CI 上会与其它连接 EADDRINUSE 撞车。
let port = 0;
const basePath = '/pinvou3/remote';
let httpBase = '';
let wsUrl = '';
const endpointId = `endpoint_webui_${Date.now()}`;
const accessToken = `access_webui_${Date.now()}_0123456789`;
const desktopSecret = `desktop_webui_${Date.now()}_0123456789`;
const streamEpoch = `epoch_webui_${Date.now()}`;
const barrierEpoch = `${streamEpoch}_frontend_ready`;
const barrierAfterSeq = 40;
const fragment = `#endpoint=${encodeURIComponent(endpointId)}&token=${encodeURIComponent(accessToken)}`;
const results = [];
const relayHttpRequestTargets = [];
const pageErrors = [];
const browserWebSocketUrls = [];
const browserWebSocketFrames = [];
const browserProfile = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-webui-smoke-'));
let relay;
let browser;
let desktop;

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

function allocatePort() {
  return new Promise((resolve, reject) => {
    const probe = require('node:net').createServer();
    probe.once('error', reject);
    probe.listen(0, '127.0.0.1', () => {
      const { port: freePort } = probe.address();
      probe.close(() => resolve(freePort));
    });
  });
}

function record(name, pass, detail = '') {
  results.push({ name, pass });
  console.log(`${pass ? 'PASS' : 'FAIL'} ${name}${detail ? `  ${detail}` : ''}`);
  assert.ok(pass, name);
}

function waitForOutput(child, pattern, timeoutMs = 8_000) {
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

function minimalRpcResult(command, args = {}) {
  switch (command) {
    case 'get_settings':
      return { theme: 'genesis', language: 'zh-Hans' };
    case 'get_app_version':
      return 'webui-smoke-v2';
    case 'list_models':
      return { models: [], active_model_id: null };
    case 'get_backend_status':
      return { vllm_online: false, max_model_len: 32768 };
    case 'get_disabled_connectors':
      return ['smoke-roundtrip'];
    case 'web_access_create_session_and_chat':
      return {
        id: `session-smoke-${Date.now()}`,
        title: '新对话',
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
        message_count: 0,
        transcript_revision: 'empty-smoke-revision',
      };
    case 'web_access_upload_attachment_chunk':
      if (!args.commit) return null;
      return {
        handle: `attachment_${String(args.uploadId || 'smoke').replace(/[^A-Za-z0-9_-]/g, '_')}`,
        kind: 'text',
        basename: String(args.fileName || 'smoke.txt'),
        token_estimate: 1,
        byte_size: Number(args.total || 0),
        warning: null,
      };
    case 'get_mode_state':
      return { mode: 'yolo' };
    case 'voice_asr_status':
      return { ready: false, missing: ['desktop_component'] };
    case 'kb_model_status':
      return { installed: false, downloading: false };
    case 'web_access_list_host_files': {
      const roots = [
        { name: 'Home', path: 'C:\\Users\\smoke' },
        { name: 'C:', path: 'C:\\' },
        { name: 'D:', path: 'D:\\' },
      ];
      if (args.path === 'D:\\') {
        return {
          path: 'D:\\', parent: null, roots,
          entries: [{ name: 'report.txt', path: 'D:\\report.txt', is_dir: false, size: 12 }],
        };
      }
      return { path: 'C:\\Users\\smoke', parent: 'C:\\Users', roots, entries: [] };
    }
    case 'get_effective_model_config':
    case 'find_resumable_run':
    case 'get_active_persona':
    case 'get_session_model_id':
    case 'session_mounted_collection':
      return null;
    default:
      if (/^(list_|kb_collection_list$|kb_documents$)/.test(command)) return [];
      if (/^(get_|read_)/.test(command)) return null;
      return { ok: true };
  }
}

class SimulatedDesktop {
  constructor(ws) {
    this.ws = ws;
    this.messages = [];
    this.rpcRequests = [];
    this.leaseId = null;
    this.waiters = new Set();
    this.deferredRpc = new Map();
    ws.on('message', raw => this.onMessage(raw));
  }

  deferNextRpc(command) {
    let resolveSeen;
    const seen = new Promise(resolve => { resolveSeen = resolve; });
    const deferred = { command, request: null, resolveSeen };
    this.deferredRpc.set(command, deferred);
    return {
      seen,
      respond: result => {
        assert.ok(deferred.request, `${command} has not reached the desktop`);
        this.respondRpc(deferred.request, true, result);
      },
      reject: (error, errorCode = 'command_failed') => {
        assert.ok(deferred.request, `${command} has not reached the desktop`);
        this.respondRpc(deferred.request, false, null, error, errorCode);
      },
    };
  }

  respondRpc(message, ok, result, error, errorCode) {
    this.send({
      type: 'rpc_response',
      lease_id: message.lease_id,
      id: message.id,
      client_request_id: message.client_request_id,
      ok,
      result,
      error,
      error_code: errorCode,
    });
  }

  onMessage(raw) {
    const message = JSON.parse(String(raw));
    this.messages.push(message);
    if (message.type === 'web_client_connected') this.leaseId = message.lease_id;
    if (message.type === 'client_ready') {
      this.send({
        type: 'desktop_snapshot',
        lease_id: message.lease_id,
        stream_epoch: message.stream_epoch || streamEpoch,
        seq: Number(message.after_seq || 0),
        snapshot: {
          desktop_connected: true,
          backend_version: 'webui-smoke-v2',
          capabilities: {
            protocol_version: 2,
            commands: webPolicy.allowed_commands,
            events: webPolicy.allowed_events,
          },
        },
      });
    }
    if (message.type === 'rpc_request') {
      this.rpcRequests.push(message);
      const deferred = this.deferredRpc.get(message.command);
      if (deferred) {
        this.deferredRpc.delete(message.command);
        deferred.request = message;
        deferred.resolveSeen(message);
      } else {
        this.respondRpc(message, true, minimalRpcResult(message.command, message.args));
      }
    }
    for (const waiter of [...this.waiters]) {
      if (!waiter.predicate(message)) continue;
      clearTimeout(waiter.timer);
      this.waiters.delete(waiter);
      waiter.resolve(message);
    }
  }

  send(message) {
    this.ws.send(JSON.stringify({ v: 2, endpoint_id: endpointId, ...message }));
  }

  waitFor(predicate, startAt = 0, timeoutMs = 10_000) {
    const found = this.messages.slice(startAt).find(predicate);
    if (found) return Promise.resolve(found);
    return new Promise((resolve, reject) => {
      const waiter = {
        predicate,
        resolve,
        timer: setTimeout(() => {
          this.waiters.delete(waiter);
          reject(new Error('timeout waiting for simulated desktop message'));
        }, timeoutMs),
      };
      this.waiters.add(waiter);
    });
  }
}

async function connectDesktop() {
  const ws = await openSocket();
  const simulated = new SimulatedDesktop(ws);
  const registered = simulated.waitFor(message => message.type === 'desktop_endpoint_registered');
  ws.send(JSON.stringify({
    v: 2,
    type: 'desktop_endpoint_register',
    endpoint_id: endpointId,
    access_token: accessToken,
    desktop_secret: desktopSecret,
  }));
  await registered;
  return simulated;
}

function observePage(page) {
  page.on('pageerror', error => pageErrors.push(error.message));
}

async function observeBrowserProtocol(page) {
  const cdp = await page.createCDPSession();
  await cdp.send('Network.enable');
  cdp.on('Network.webSocketCreated', event => {
    browserWebSocketUrls.push(String(event.url || ''));
  });
  cdp.on('Network.webSocketFrameSent', event => {
    const payload = event.response && event.response.payloadData;
    if (typeof payload !== 'string') return;
    try { browserWebSocketFrames.push(JSON.parse(payload)); } catch (_) {}
  });
}

async function installFrontendBarrierProbe(page) {
  await page.evaluateOnNewDocument((id, epoch, afterSeq) => {
    sessionStorage.setItem(`pinvou.web.cursor.${id}`, JSON.stringify({
      stream_epoch: epoch,
      after_seq: afterSeq,
    }));
    window.__webuiConnectionHistory = [];
    window.addEventListener('pinvou:web-connection', event => {
      window.__webuiConnectionHistory.push(event.detail);
    });
  }, endpointId, barrierEpoch, barrierAfterSeq);
}

async function waitForSharedUi(page, width, height) {
  await page.waitForFunction(() => (
    window.PinvouPlatform?.kind === 'web'
      && window.PinvouPlatform.getConnectionState?.().status === 'connected'
      && window.TauriBridge?.available === true
  ), { timeout: 15_000 });
  await page.waitForFunction(() => {
    const root = document.querySelector('#root');
    return root && root.children.length > 0 && document.body.innerText.trim().length > 10;
  }, { timeout: 15_000 });
  const layout = await page.evaluate(() => ({
    viewportWidth: window.innerWidth,
    viewportHeight: window.innerHeight,
    rootWidth: Math.round(document.querySelector('#root').getBoundingClientRect().width),
    rootHeight: Math.round(document.querySelector('#root').getBoundingClientRect().height),
    scrollWidth: document.documentElement.scrollWidth,
    scripts: [...document.scripts].map(script => script.src).filter(Boolean),
    textLength: document.body.innerText.trim().length,
  }));
  assert.equal(layout.viewportWidth, width);
  assert.equal(layout.viewportHeight, height);
  assert.ok(layout.rootWidth >= width - 2, JSON.stringify(layout));
  assert.ok(layout.rootHeight >= height - 2, JSON.stringify(layout));
  assert.ok(layout.scrollWidth <= width + 4, JSON.stringify(layout));
  assert.ok(layout.scripts.some(src => /platform\/web\/bootstrap\.js(?:$|\?)/.test(src)), JSON.stringify(layout.scripts));
  assert.ok(layout.scripts.some(src => /main[^/]*\.js(?:$|\?)/.test(src)), JSON.stringify(layout.scripts));
  return layout;
}

async function main() {
  port = await allocatePort();
  httpBase = `http://127.0.0.1:${port}${basePath}/`;
  wsUrl = `ws://127.0.0.1:${port}${basePath}/ws`;
  // Instrument Node's HTTP request listener in the child process. Puppeteer's
  // high-level navigation URL intentionally preserves the document fragment,
  // although the fragment is not sent on the wire; req.url is the actual HTTP
  // request target seen by the Relay and is therefore the boundary to assert.
  const relayWrapper = [
    "const http = require('node:http');",
    "const { pathToFileURL } = require('node:url');",
    'const originalCreateServer = http.createServer;',
    'http.createServer = function (listener) {',
    '  return originalCreateServer.call(http, function (req, res) {',
    "    if (process.send) process.send({ kind: 'http_request', target: req.url || '' });",
    '    return listener(req, res);',
    '  });',
    '};',
    'import(pathToFileURL(process.argv[1]).href);',
  ].join('\n');
  relay = spawn(process.execPath, ['-e', relayWrapper, path.join(relayDir, 'server.js')], {
    cwd: relayDir,
    env: {
      ...process.env,
      PORT: String(port),
      PINVOU_REMOTE_PUBLIC_BASE_PATH: basePath,
      PINVOU_REMOTE_STATE_PATH: path.join(browserProfile, 'relay-state.json'),
      WS_AUTH_TIMEOUT_MS: '5000',
      HEARTBEAT_INTERVAL_MS: '5000',
    },
    stdio: ['ignore', 'pipe', 'pipe', 'ipc'],
  });
  relay.on('message', message => {
    if (message?.kind === 'http_request') relayHttpRequestTargets.push(String(message.target));
  });
  relay.stderr.on('data', chunk => process.stderr.write(chunk));
  await waitForOutput(relay, /pinvou remote relay listening/);

  browser = await puppeteer.launch({
    executablePath: CHROME,
    headless: 'new',
    args: ['--no-sandbox', '--disable-gpu', '--no-first-run', '--no-default-browser-check'],
    userDataDir: browserProfile,
  });

  const desktopPage = await browser.newPage();
  observePage(desktopPage);
  await observeBrowserProtocol(desktopPage);
  await installFrontendBarrierProbe(desktopPage);
  await desktopPage.setViewport({ width: 1440, height: 900, deviceScaleFactor: 1 });
  await desktopPage.goto(`${httpBase}conversations/current${fragment}`, { waitUntil: 'networkidle0' });

  await desktopPage.waitForFunction(() => (
    window.__webuiConnectionHistory?.some(item => item.status === 'desktop_offline')
  ), { timeout: 10_000 });
  const missingEndpointState = await desktopPage.evaluate(id => ({
    closedPermanently: window.PinvouWebClient?.closedPermanently,
    cursor: JSON.parse(sessionStorage.getItem(`pinvou.web.cursor.${id}`) || '{}'),
    history: window.__webuiConnectionHistory?.map(item => item.status) || [],
  }), endpointId);
  record('endpoint_not_found 不会让 Web 客户端永久失效',
    missingEndpointState.closedPermanently === false
      && missingEndpointState.history.includes('desktop_offline')
      && missingEndpointState.cursor.stream_epoch === barrierEpoch
      && missingEndpointState.cursor.after_seq === barrierAfterSeq);

  const preStateReadyReplay = await desktopPage.evaluate((id, epoch, seq) => {
    const client = window.PinvouWebClient;
    client.handleRemoteEvent({
      type: 'event',
      event: 'chat:delta',
      stream_epoch: epoch,
      seq: seq + 1,
      payload: { session_id: 'pre-state-ready-smoke', text: 'must-not-be-acked' },
    });
    return {
      frontendReady: client.frontendReady,
      stateReady: client.stateReady,
      cursor: JSON.parse(sessionStorage.getItem(`pinvou.web.cursor.${id}`) || '{}'),
    };
  }, endpointId, barrierEpoch, barrierAfterSeq);
  record('durable state 未就绪时拒绝 replay 且不推进游标',
    preStateReadyReplay.frontendReady === true
      && preStateReadyReplay.stateReady === false
      && preStateReadyReplay.cursor.stream_epoch === barrierEpoch
      && preStateReadyReplay.cursor.after_seq === barrierAfterSeq);

  desktop = await connectDesktop();
  const browserConnected = await desktop.waitFor(message => message.type === 'web_client_connected');
  const capabilityReady = await desktop.waitFor(message => (
    message.type === 'client_ready' && message.state_ready === false
  ));
  const capabilityReadyIndex = desktop.messages.indexOf(capabilityReady);
  const stateReady = await desktop.waitFor(message => (
    message.type === 'client_ready' && message.state_ready === true
  ), capabilityReadyIndex + 1);
  const recoveredConnection = await desktopPage.evaluate(() => ({
    state: window.PinvouPlatform?.getConnectionState?.().status,
    joined: window.PinvouWebClient?.joined,
    desktopOnline: window.PinvouWebClient?.desktopOnline,
  }));
  record('桌面稍后注册后，同一 Web 页面自动恢复连接',
    recoveredConnection.state === 'connected'
      && recoveredConnection.joined === true
      && recoveredConnection.desktopOnline === true);
  const connectedIndex = desktop.messages.indexOf(browserConnected);
  const stateReadyIndex = desktop.messages.indexOf(stateReady);
  const subscribedAfterCapabilities = desktop.messages.slice(capabilityReadyIndex + 1, stateReadyIndex)
    .some(message => message.type === 'event_subscribe' && message.event === 'chat:delta');
  const rpcBeforeReady = desktop.messages.slice(connectedIndex + 1, capabilityReadyIndex)
    .some(message => message.type === 'rpc_request');
  const initializationRpcBetweenBarriers = desktop.messages.slice(capabilityReadyIndex + 1, stateReadyIndex)
    .some(message => message.type === 'rpc_request' && message.command === 'list_sessions');
  const preReplayState = await desktopPage.evaluate(id => ({
    cursor: JSON.parse(sessionStorage.getItem(`pinvou.web.cursor.${id}`) || '{}'),
    chatDeltaListenerRegistered: window.PinvouWebClient?.listeners?.has('chat:delta') === true,
  }), endpointId);
  const readyEvidence = {
    browserEpoch: browserConnected.stream_epoch === barrierEpoch,
    browserCursor: browserConnected.after_seq === barrierAfterSeq,
    capabilityEpoch: capabilityReady.stream_epoch === barrierEpoch,
    capabilityCursor: capabilityReady.after_seq === barrierAfterSeq,
    capabilityBarrier: capabilityReady.state_ready === false,
    stateEpoch: stateReady.stream_epoch === barrierEpoch,
    stateCursor: stateReady.after_seq === barrierAfterSeq,
    stateBarrier: stateReady.state_ready === true,
    subscribedAfterCapabilities,
    noRpcBeforeReady: !rpcBeforeReady,
    initializationRpcBetweenBarriers,
    chatDeltaListenerRegistered: preReplayState.chatDeltaListenerRegistered,
    browserCursorUnchanged: preReplayState.cursor.after_seq === barrierAfterSeq,
  };
  const readyPassed = Object.values(readyEvidence).every(Boolean);
  if (!readyPassed) console.error('two-phase ready evidence:', readyEvidence, {
    connectedIndex,
    capabilityReadyIndex,
    stateReadyIndex,
    chatDeltaSubscriptions: desktop.messages
      .map((message, index) => ({ message, index }))
      .filter(({ message }) => message.type === 'event_subscribe' && message.event === 'chat:delta')
      .map(({ index }) => index),
  });
  record('两阶段 ready 先协商能力，durable state 就绪后才开放事件 replay', readyPassed);

  desktop.send({
    type: 'event',
    lease_id: browserConnected.lease_id,
    event: 'chat:delta',
    stream_epoch: barrierEpoch,
    seq: barrierAfterSeq + 1,
    payload: { session_id: 'frontend-ready-smoke', text: 'replayed-after-ready' },
  });
  await desktopPage.waitForFunction((id, expectedSeq) => {
    const cursor = JSON.parse(sessionStorage.getItem(`pinvou.web.cursor.${id}`) || '{}');
    return cursor.after_seq === expectedSeq;
  }, { timeout: 8_000 }, endpointId, barrierAfterSeq + 1);
  record('client_ready 后 replay 事件才推进浏览器游标', true);

  const desktopLayout = await waitForSharedUi(desktopPage, 1440, 900);
  record('共享 WebUI 在 1440x900 电脑浏览器完整渲染', desktopLayout.textLength > 10);

  const hostPickerNavigation = await desktopPage.evaluate(async () => {
    const waitFor = async predicate => {
      for (let attempt = 0; attempt < 100; attempt += 1) {
        if (predicate()) return;
        await new Promise(resolve => setTimeout(resolve, 20));
      }
      throw new Error('host picker state timeout');
    };
    const rowNames = () => Array.from(document.querySelectorAll('.pinvou-host-picker-name'))
      .map(node => node.textContent);
    // Home 根入口的文案来自 main.jsx 注入的 PinvouHostFilePickerStrings(shared/i18n.js
    // uiPlatformMisc.hostFilePicker)。断言必须从该对象取值,不得硬编码某语言文案,
    // 否则会与页面默认语言策略或 picker 内 fallback 文本隐性耦合。
    const homeLabel = (window.PinvouHostFilePickerStrings || {}).home;
    if (!homeLabel) throw new Error('host picker strings missing localized home label');
    const pickerPromise = window.PinvouHostFilePicker.open({ multiple: true });
    await waitFor(() => document.querySelector('.pinvou-host-picker-root-button:not(:disabled)'));
    document.querySelector('.pinvou-host-picker-root-button').click();
    await waitFor(() => rowNames().includes('D:'));
    const rootNames = rowNames();
    const dDrive = Array.from(document.querySelectorAll('.pinvou-host-picker-row'))
      .find(row => row.querySelector('.pinvou-host-picker-name')?.textContent === 'D:');
    dDrive.click();
    await waitFor(() => document.querySelector('.pinvou-host-picker-path')?.textContent === 'D:\\');
    const driveNames = rowNames();
    document.querySelector('.pinvou-host-picker-toolbar .pinvou-host-picker-icon').click();
    await waitFor(() => rowNames().includes('D:'));
    const rootsAfterUp = rowNames();
    document.querySelector('.pinvou-host-picker-actions .pinvou-host-picker-button').click();
    await pickerPromise;
    return { homeLabel, rootNames, driveNames, rootsAfterUp };
  });
  record('远程文件选择器动态列出盘符，盘根目录上一级返回此电脑',
    !!hostPickerNavigation.homeLabel
      && hostPickerNavigation.rootNames.includes(hostPickerNavigation.homeLabel)
      && ['C:', 'D:'].every(name => hostPickerNavigation.rootNames.includes(name))
      && hostPickerNavigation.driveNames.includes('report.txt')
      && !hostPickerNavigation.driveNames.includes('C:')
      && hostPickerNavigation.rootsAfterUp.includes(hostPickerNavigation.homeLabel)
      && ['C:', 'D:'].every(name => hostPickerNavigation.rootsAfterUp.includes(name)),
    JSON.stringify(hostPickerNavigation));

  const joinFrames = browserWebSocketFrames.filter(message => message.type === 'web_client_join');
  const v2ClientFrames = browserWebSocketFrames.filter(message => (
    message.type === 'web_client_join' || message.lease_id
  ));
  record('Web 客户端所有已发送协议帧都显式携带 v2',
    v2ClientFrames.length > 0 && v2ClientFrames.every(message => message.v === 2));
  record('v2 Web join 只携带 access_token，不携带 desktop_secret',
    joinFrames.length >= 2 && joinFrames.every(message => (
      message.v === 2
        && message.endpoint_id === endpointId
        && message.access_token === accessToken
        && !Object.hasOwn(message, 'desktop_secret')
        && !Object.hasOwn(message, 'session_id')
        && !Object.hasOwn(message, 'room_id')
    )));
  const exactFixedAssets = [
    `${basePath}/platform/web/bootstrap.js`,
    `${basePath}/platform/tauri/bridge.js`,
    `${basePath}/platform/web/access-policy.json`,
  ];
  record('extensionless SPA 深链仍连接固定 base WebSocket 并加载固定资源',
    browserWebSocketUrls.length >= 2
      && browserWebSocketUrls.every(url => url === wsUrl)
      && exactFixedAssets.every(target => relayHttpRequestTargets.includes(target))
      && !relayHttpRequestTargets.some(target => target.includes('/conversations/current/web-')),
    `${browserWebSocketUrls.join(', ')} | ${exactFixedAssets.join(', ')}`);

  const firstLease = desktop.leaseId;
  const takeoverStart = desktop.messages.length;
  const mobilePage = await browser.newPage();
  observePage(mobilePage);
  await mobilePage.setViewport({ width: 390, height: 844, deviceScaleFactor: 1 });
  await mobilePage.goto(`${httpBase}${fragment}`, { waitUntil: 'networkidle0' });
  const secondConnected = await desktop.waitFor(
    message => message.type === 'web_client_connected' && message.lease_id !== firstLease,
    takeoverStart,
  );
  const mobileLayout = await waitForSharedUi(mobilePage, 390, 844);
  record('同一共享 WebUI 在 390x844 手机浏览器无横向溢出', mobileLayout.scrollWidth <= 394);

  // 移动壳层：紧凑视口应呈现顶栏 + 4 个底部 Tab，侧栏窄轨完全让出横向空间。
  const mobileShell = await mobilePage.evaluate(() => {
    const rect = (selector) => {
      const node = document.querySelector(selector);
      if (!node) return null;
      const r = node.getBoundingClientRect();
      return { bottom: r.bottom, visible: r.height > 0 && r.width > 0 };
    };
    const sidebar = document.querySelector('[data-testid="app-sidebar"]');
    return {
      topBar: rect('[data-testid="mobile-top-bar"]'),
      tabBar: rect('[data-testid="mobile-tab-bar"]'),
      tabCount: document.querySelectorAll('[data-testid="mobile-tab-bar"] button').length,
      sidebarHidden: !sidebar || sidebar.getBoundingClientRect().width === 0,
    };
  });
  record('390x844 呈现移动壳层：顶栏 + 底部 Tab，侧栏窄轨隐藏',
    !!(mobileShell.topBar && mobileShell.topBar.visible)
      && !!(mobileShell.tabBar && mobileShell.tabBar.visible)
      && mobileShell.tabBar.bottom <= 845
      && mobileShell.tabCount === 4
      && mobileShell.sidebarHidden,
    JSON.stringify(mobileShell));

  const mobileComposerGap = await mobilePage.evaluate(() => {
    const disclaimer = document.querySelector('[data-testid="chat-disclaimer"]')?.getBoundingClientRect();
    const tabBar = document.querySelector('[data-testid="mobile-tab-bar"]')?.getBoundingClientRect();
    return disclaimer && tabBar ? {
      disclaimerBottom: disclaimer.bottom,
      tabTop: tabBar.top,
      gap: tabBar.top - disclaimer.bottom,
    } : null;
  });
  record('手机输入区免责声明紧邻底部 Tab，不重复保留桌面端大底距',
    !!mobileComposerGap && mobileComposerGap.gap >= 0 && mobileComposerGap.gap <= 24,
    JSON.stringify(mobileComposerGap));

  const compactViewportLock = await mobilePage.evaluate(async () => {
    const root = document.querySelector('[data-testid="app-root"]');
    const composer = document.querySelector('[data-testid="chat-composer-wrap"] textarea');
    composer.focus();
    document.documentElement.scrollTop = 320;
    document.body.scrollTop = 320;
    await new Promise(resolve => setTimeout(resolve, 180));
    const rootRect = root.getBoundingClientRect();
    const composerRect = composer.getBoundingClientRect();
    return {
      rootPosition: getComputedStyle(root).position,
      rootTop: rootRect.top,
      rootBottom: rootRect.bottom,
      composerBottom: composerRect.bottom,
      scrollY: window.scrollY,
      documentOverflow: getComputedStyle(document.documentElement).overflow,
      bodyPosition: getComputedStyle(document.body).position,
    };
  });
  // Chromium 不会弹出真机软键盘，用缩短 visual viewport 模拟键盘占据屏幕下半部。
  await mobilePage.setViewport({ width: 390, height: 520, deviceScaleFactor: 1 });
  await mobilePage.waitForFunction(() => (
    document.querySelector('[data-testid="app-root"]')?.getBoundingClientRect().bottom <= 521
  ), { timeout: 5_000 });
  const compactKeyboardViewport = await mobilePage.evaluate(() => {
    const rootRect = document.querySelector('[data-testid="app-root"]').getBoundingClientRect();
    const composerRect = document.querySelector('[data-testid="chat-composer-wrap"]').getBoundingClientRect();
    return {
      rootTop: rootRect.top,
      rootBottom: rootRect.bottom,
      composerTop: composerRect.top,
      composerBottom: composerRect.bottom,
      scrollY: window.scrollY,
    };
  });
  await mobilePage.setViewport({ width: 390, height: 844, deviceScaleFactor: 1 });
  await mobilePage.waitForFunction(() => (
    document.querySelector('[data-testid="app-root"]')?.getBoundingClientRect().bottom >= 843
  ), { timeout: 5_000 });
  record('手机输入框聚焦时锁定文档滚动，应用与输入框不会被 Safari 推出可视区',
    compactViewportLock.rootPosition === 'fixed'
      && compactViewportLock.rootTop >= -1
      && compactViewportLock.rootBottom <= 845
      && compactViewportLock.composerBottom <= compactViewportLock.rootBottom
      && compactViewportLock.scrollY === 0
      && compactViewportLock.documentOverflow === 'hidden'
      && compactViewportLock.bodyPosition === 'fixed'
      && compactKeyboardViewport.rootTop >= -1
      && compactKeyboardViewport.rootBottom <= 521
      && compactKeyboardViewport.composerTop >= 0
      && compactKeyboardViewport.composerBottom <= compactKeyboardViewport.rootBottom
      && compactKeyboardViewport.scrollY === 0,
    JSON.stringify({ initial: compactViewportLock, keyboard: compactKeyboardViewport }));

  await mobilePage.click('[data-testid="mobile-tab-more"]');
  await mobilePage.waitForSelector('[data-testid="mobile-more-sheet"]', { timeout: 5_000 });
  const moreItems = await mobilePage.evaluate(() => (
    Array.from(document.querySelectorAll('[data-testid="mobile-more-sheet"] button'))
      .map(node => node.getAttribute('data-testid'))
  ));
  record('「更多」底部面板承载设置/搜索等次级入口',
    moreItems.includes('mobile-more-settings') && moreItems.includes('mobile-more-search'),
    moreItems.join(','));
  await mobilePage.click('[data-testid="mobile-more-settings"]');
  await mobilePage.waitForSelector('[data-testid="settings-dialog"]', { timeout: 5_000 });
  const mobileSettingsLayout = await mobilePage.evaluate(() => {
    const bounds = (selector) => {
      const node = document.querySelector(selector);
      const rect = node && node.getBoundingClientRect();
      return rect && { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom, width: rect.width };
    };
    const segment = document.querySelector('[data-testid="settings-segmented"]');
    const segmentButtons = segment ? Array.from(segment.querySelectorAll('button')).map((node) => {
      const rect = node.getBoundingClientRect();
      return { left: rect.left, right: rect.right, top: rect.top, width: rect.width };
    }) : [];
    return {
      viewport: document.documentElement.clientWidth,
      dialog: bounds('[data-testid="settings-dialog"]'),
      nav: bounds('[data-testid="settings-nav"]'),
      content: bounds('[data-testid="settings-content"]'),
      segment: bounds('[data-testid="settings-segmented"]'),
      segmentButtons,
      scrollWidth: document.documentElement.scrollWidth,
    };
  });
  record('手机设置页使用顶部分类 + 单列正文，分段选项同排且不溢出',
    !!mobileSettingsLayout.dialog
      && !!mobileSettingsLayout.nav
      && !!mobileSettingsLayout.content
      && mobileSettingsLayout.nav.bottom <= mobileSettingsLayout.content.top + 1
      && mobileSettingsLayout.content.width >= mobileSettingsLayout.dialog.width - 2
      && mobileSettingsLayout.segmentButtons.length >= 2
      && mobileSettingsLayout.segmentButtons.every(button => button.width >= 60)
      && mobileSettingsLayout.segmentButtons.every(button => button.right <= mobileSettingsLayout.viewport)
      && mobileSettingsLayout.scrollWidth <= mobileSettingsLayout.viewport + 4,
    JSON.stringify(mobileSettingsLayout));
  await mobilePage.click('[data-testid="settings-close"]');
  await mobilePage.waitForFunction(
    () => !document.querySelector('[data-testid="settings-dialog"]'),
    { timeout: 5_000 },
  );

  await mobilePage.click('[data-testid="mobile-navigation-open"]');
  await mobilePage.waitForFunction(
    () => document.querySelector('[data-testid="app-sidebar"]')?.getBoundingClientRect().width > 0,
    { timeout: 5_000 },
  );
  const mobileSidebarLayout = await mobilePage.evaluate(() => {
    const sidebar = document.querySelector('[data-testid="app-sidebar"]');
    const rect = sidebar.getBoundingClientRect();
    const primaryNav = document.querySelector('[data-testid="sidebar-primary-nav"]');
    const primaryRect = primaryNav && primaryNav.getBoundingClientRect();
    const navButtons = primaryNav ? Array.from(primaryNav.querySelectorAll(':scope > div')).map((node) => {
      const buttonRect = node.getBoundingClientRect();
      return { top: buttonRect.top, bottom: buttonRect.bottom, height: buttonRect.height };
    }) : [];
    const recents = document.querySelector('[data-testid="sidebar-recents"]');
    const recentsRect = recents && recents.getBoundingClientRect();
    return {
      viewport: document.documentElement.clientWidth,
      width: rect.width,
      rightGap: document.documentElement.clientWidth - rect.right,
      primaryBottom: primaryRect ? primaryRect.bottom : null,
      recentsTop: recentsRect ? recentsRect.top : null,
      navButtons,
    };
  });
  record('手机会话抽屉压缩顶部导航高度，为任务列表留出纵向空间',
    mobileSidebarLayout.width <= 281
      && mobileSidebarLayout.navButtons.length >= 7
      && mobileSidebarLayout.navButtons.every(button => button.height <= 41)
      && mobileSidebarLayout.primaryBottom <= 400
      && mobileSidebarLayout.recentsTop <= 405,
    JSON.stringify(mobileSidebarLayout));
  // The z-40 sidebar covers the center of the z-30 full-screen overlay. A
  // Puppeteer coordinate click would hit a sidebar session instead of closing
  // the drawer, so invoke the overlay's click handler directly.
  await mobilePage.evaluate(() => document.querySelector('[data-testid="mobile-navigation-close"]').click());
  await mobilePage.waitForFunction(
    () => document.querySelector('[data-testid="app-sidebar"]')?.getBoundingClientRect().width === 0,
    { timeout: 5_000 },
  );
  await mobilePage.waitForSelector('[data-testid="chat-greeting"]', { timeout: 5_000 });

  const compactGreeting = await mobilePage.evaluate(() => {
    const node = document.querySelector('[data-testid="chat-greeting"]');
    if (!node) return null;
    const style = getComputedStyle(node);
    const rect = node.getBoundingClientRect();
    return { fontSize: parseFloat(style.fontSize), height: rect.height };
  });
  record('手机欢迎语使用紧凑字号且不会产生桌面字号的孤行',
    !!compactGreeting && compactGreeting.fontSize <= 32 && compactGreeting.height < 90,
    JSON.stringify(compactGreeting));

  await mobilePage.click('[data-testid="composer-tool-menu-trigger"]');
  await mobilePage.waitForSelector('[data-testid="composer-tool-menu"]', { timeout: 5_000 });
  const toolMenuBounds = await mobilePage.evaluate(() => {
    const node = document.querySelector('[data-testid="composer-tool-menu"]');
    const trigger = document.querySelector('[data-testid="composer-tool-menu-trigger"]');
    const rect = node.getBoundingClientRect();
    const triggerRect = trigger.getBoundingClientRect();
    return {
      left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom, width: rect.width,
      parent: node.parentElement === document.body,
      triggerTop: triggerRect.top,
      viewport: document.documentElement.clientWidth,
      viewportH: document.documentElement.clientHeight,
    };
  });
  record('手机工具菜单始终完整落在视口安全边距内',
    toolMenuBounds.left >= 11 && toolMenuBounds.right <= toolMenuBounds.viewport - 11,
    JSON.stringify(toolMenuBounds));
  // 垂直锚定回归：菜单必须 portal 到 <body>（脱离 composer 的 backdrop-filter 包含块），
  // 底边紧贴触发按钮上方（约 8px 间距）、整体落在视口内，防止「跳得太高」重现。
  record('手机工具菜单 portal 到 body 并锚定在触发按钮上方',
    toolMenuBounds.parent === true
      && toolMenuBounds.top >= 11
      && toolMenuBounds.bottom <= toolMenuBounds.triggerTop
      && toolMenuBounds.triggerTop - toolMenuBounds.bottom <= 16,
    JSON.stringify(toolMenuBounds));
  await mobilePage.evaluate(() => document.querySelector('[data-testid="composer-tool-menu-trigger"]').click());
  await desktopPage.waitForFunction(() => (
    window.PinvouPlatform?.getConnectionState?.().status === 'replaced'
  ), { timeout: 8_000 });
  record('新 Web 客户端接管唯一 endpoint lease', secondConnected.lease_id !== firstLease);

  const roundtripStart = desktop.rpcRequests.length;
  const rpcResult = await mobilePage.evaluate(() => window.__TAURI__.core.invoke('get_disabled_connectors'));
  const explicitRpc = desktop.rpcRequests.slice(roundtripStart)
    .find(message => message.command === 'get_disabled_connectors');
  record('WebUI RPC 经真实 Relay 往返并携带幂等请求 ID',
    rpcResult?.[0] === 'smoke-roundtrip'
      && explicitRpc?.id
      && explicitRpc.client_request_id === explicitRpc.id);

  const firstTurnCommand = 'web_access_create_session_and_chat';
  const optimisticText = '首条消息立即显示测试';
  const firstTurnPayload = `${optimisticText}\n\n---\nPinvou 场景路由测试`;
  const deferredFirstTurn = desktop.deferNextRpc(firstTurnCommand);
  await mobilePage.evaluate(
    ({ text, payload }) => window.TauriBridge.chat.sendMessage(text, { pinvouPayloadText: payload }),
    { text: optimisticText, payload: firstTurnPayload },
  );
  const firstTurnRequest = await deferredFirstTurn.seen;
  await mobilePage.waitForFunction(text => (
    document.body.innerText.includes(text)
      && !!document.querySelector('[data-testid="message-delivery-sending"]')
      && !document.querySelector('[data-testid="chat-greeting"]')
  ), { timeout: 5_000 }, optimisticText);
  record('WebUI 新对话首条消息不等待桌面 RPC 即刻显示',
    firstTurnRequest.command === firstTurnCommand
      && /^first_turn_/.test(firstTurnRequest.client_request_id || '')
      && firstTurnRequest.args?.message === firstTurnPayload);

  const optimisticSessionId = 'session-optimistic-smoke';
  deferredFirstTurn.respond({
    id: optimisticSessionId,
    title: '新对话',
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    message_count: 0,
    transcript_revision: 'empty-optimistic-revision',
  });
  await mobilePage.waitForFunction(sessionId => (
    window.TauriBridge.state.get('sessions').activeSessionId === sessionId
      && !!document.querySelector('[data-testid="message-delivery-accepted"]')
  ), { timeout: 5_000 }, optimisticSessionId);

  desktop.send({
    type: 'event',
    lease_id: desktop.leaseId,
    event: 'chat:user_message',
    stream_epoch: streamEpoch,
    seq: 1,
    payload: {
      session_id: optimisticSessionId,
      content: optimisticText,
      operation: 'append',
      base_transcript_revision: 'empty-optimistic-revision',
    },
  });
  await mobilePage.waitForFunction(id => {
    const cursor = JSON.parse(sessionStorage.getItem(`pinvou.web.cursor.${id}`) || '{}');
    return cursor.after_seq === 1;
  }, { timeout: 5_000 }, endpointId);
  const optimisticUserCount = await mobilePage.evaluate(() => (
    window.TauriBridge.state.get('chat').chatItems.filter(item => item && item.type === 'user').length
  ));
  record('首条消息回执事件不会重复插入用户气泡', optimisticUserCount === 1);

  await mobilePage.evaluate(() => window.TauriBridge.sessions.createNewSession());
  await mobilePage.waitForSelector('[data-testid="chat-greeting"]', { timeout: 5_000 });
  const failedText = '首条消息失败重试测试';
  const deferredFailure = desktop.deferNextRpc(firstTurnCommand);
  await mobilePage.evaluate(text => window.TauriBridge.chat.sendMessage(text), failedText);
  const failedRequest = await deferredFailure.seen;

  // The previous Session may still stream while the next draft is waiting for
  // its atomic create-and-chat response. Its background event must not replace
  // the visible draft working set with a fresh empty buffer.
  desktop.send({
    type: 'event',
    lease_id: desktop.leaseId,
    event: 'chat:delta',
    stream_epoch: streamEpoch,
    seq: 2,
    payload: { session_id: optimisticSessionId, text: 'late-previous-session-delta' },
  });
  await mobilePage.waitForFunction(id => {
    const cursor = JSON.parse(sessionStorage.getItem(`pinvou.web.cursor.${id}`) || '{}');
    return cursor.after_seq === 2;
  }, { timeout: 5_000 }, endpointId);
  const preservedSecondDraft = await mobilePage.evaluate(text => {
    const sessions = window.TauriBridge.state.get('sessions');
    const chat = window.TauriBridge.state.get('chat');
    return {
      activeSessionId: sessions.activeSessionId,
      userTexts: chat.chatItems
        .filter(item => item && item.type === 'user')
        .map(item => item.text),
      sending: !!document.querySelector('[data-testid="message-delivery-sending"]'),
      greeting: !!document.querySelector('[data-testid="chat-greeting"]'),
      expectedText: text,
    };
  }, failedText);
  record('旧会话迟到事件不会清空第二个新对话的乐观消息',
    preservedSecondDraft.activeSessionId === null
      && preservedSecondDraft.userTexts.includes(failedText)
      && preservedSecondDraft.sending
      && !preservedSecondDraft.greeting,
    JSON.stringify(preservedSecondDraft));

  deferredFailure.reject('simulated first-turn rejection', 'command_failed');
  await mobilePage.waitForSelector('[data-testid="message-delivery-failed"] button', { timeout: 5_000 });

  const deferredRetry = desktop.deferNextRpc(firstTurnCommand);
  await mobilePage.click('[data-testid="message-delivery-failed"] button');
  const retryRequest = await deferredRetry.seen;
  const retrySessionId = 'session-retry-smoke';
  deferredRetry.respond({
    id: retrySessionId,
    title: '新对话',
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    message_count: 0,
    transcript_revision: 'empty-retry-revision',
  });
  await mobilePage.waitForFunction(sessionId => (
    window.TauriBridge.state.get('sessions').activeSessionId === sessionId
      && !!document.querySelector('[data-testid="message-delivery-accepted"]')
  ), { timeout: 5_000 }, retrySessionId);
  record('确定性失败保留消息并允许使用新幂等 ID 重试',
    failedRequest.client_request_id !== retryRequest.client_request_id
      && /^first_turn_/.test(retryRequest.client_request_id || ''));

  await mobilePage.evaluate(() => window.TauriBridge.sessions.createNewSession());
  await mobilePage.evaluate(async () => {
    const file = new File(['remove-me'], 'remove-me.txt', { type: 'text/plain' });
    await window.TauriBridge.attachments.uploadDeviceFiles([file]);
  });
  await mobilePage.waitForFunction(() => (
    window.TauriBridge.state.get('chat').attachments.some(attachment => attachment.status === 'ready')
  ), { timeout: 5_000 });
  const discardStart = desktop.messages.length;
  const removedHandle = await mobilePage.evaluate(() => {
    const attachment = window.TauriBridge.state.get('chat').attachments
      .find(candidate => candidate.status === 'ready');
    window.TauriBridge.attachments.removeAttachment(attachment.id);
    return attachment.result.handle;
  });
  const discardRequest = await desktop.waitFor(message => (
    message.type === 'rpc_request'
      && message.command === 'web_access_discard_attachment'
  ), discardStart, 5_000);
  record('移除已上传附件会立即释放桌面端句柄',
    discardRequest.args?.handle === removedHandle
      && await mobilePage.evaluate(() => window.TauriBridge.state.get('chat').attachments.length === 0));

  await mobilePage.evaluate(async () => {
    const file = new File(['consumed'], 'consumed.txt', { type: 'text/plain' });
    await window.TauriBridge.attachments.uploadDeviceFiles([file]);
  });
  await mobilePage.waitForFunction(() => (
    window.TauriBridge.state.get('chat').attachments.some(attachment => attachment.status === 'ready')
  ), { timeout: 5_000 });
  const consumedHandle = await mobilePage.evaluate(() => (
    window.TauriBridge.state.get('chat').attachments
      .find(attachment => attachment.status === 'ready').result.handle
  ));
  const deferredBackgroundSuccess = desktop.deferNextRpc(firstTurnCommand);
  await mobilePage.evaluate(text => window.TauriBridge.chat.sendMessage(text), '切页附件消费测试');
  const backgroundRequest = await deferredBackgroundSuccess.seen;
  await mobilePage.evaluate(() => window.TauriBridge.sessions.createNewSession());
  await mobilePage.waitForSelector('[data-testid="chat-greeting"]', { timeout: 5_000 });
  deferredBackgroundSuccess.respond({
    id: 'session-background-attachment-smoke',
    title: '新对话',
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    message_count: 0,
    transcript_revision: 'empty-background-attachment-revision',
  });
  await mobilePage.waitForFunction(() => (
    window.TauriBridge.state.get('chat').attachments.length === 0
  ), { timeout: 5_000 });
  const backgroundSuccessState = await mobilePage.evaluate(() => ({
    activeSessionId: window.TauriBridge.state.get('sessions').activeSessionId,
    attachments: window.TauriBridge.state.get('chat').attachments.length,
    greeting: !!document.querySelector('[data-testid="chat-greeting"]'),
  }));
  record('首轮发送期间切页不会把已消费附件遗留给新草稿',
    backgroundRequest.args?.attachmentHandles?.includes(consumedHandle)
      && backgroundSuccessState.activeSessionId === null
      && backgroundSuccessState.attachments === 0
      && backgroundSuccessState.greeting,
    JSON.stringify(backgroundSuccessState));

  await mobilePage.evaluate(async () => {
    window.__webuiSmokeEvents = [];
    window.__webuiSmokeUnlisten = await window.__TAURI__.event.listen('chat:delta', event => {
      window.__webuiSmokeEvents.push(event);
    });
  });
  await desktop.waitFor(message => message.type === 'event_subscribe' && message.event === 'chat:delta');
  desktop.send({
    type: 'event',
    lease_id: desktop.leaseId,
    event: 'chat:delta',
    stream_epoch: streamEpoch,
    seq: 3,
    payload: { session_id: 'webui-smoke', text: 'stream-event-smoke' },
  });
  await mobilePage.waitForFunction(() => window.__webuiSmokeEvents?.length === 1);
  const cursor = await mobilePage.evaluate(id => (
    JSON.parse(sessionStorage.getItem(`pinvou.web.cursor.${id}`) || '{}')
  ), endpointId);
  assert.deepEqual(cursor, { stream_epoch: streamEpoch, after_seq: 3 });

  const reconnectStart = desktop.messages.length;
  await mobilePage.reload({ waitUntil: 'networkidle0' });
  const resumed = await desktop.waitFor(message => (
    message.type === 'web_client_connected'
      && message.stream_epoch === streamEpoch
      && message.after_seq === 3
  ), reconnectStart, 15_000);
  await waitForSharedUi(mobilePage, 390, 844);
  record('事件序号写入游标并在页面重连时续传',
    resumed.stream_epoch === streamEpoch && resumed.after_seq === 3);

  const credentialLeak = relayHttpRequestTargets.find(target => (
    target.includes('#')
      || target.includes(endpointId)
      || target.includes(accessToken)
      || target.includes(desktopSecret)
  ));
  record('fragment endpoint/token 未进入 Relay 收到的 HTTP request-target',
    relayHttpRequestTargets.length > 0 && !credentialLeak,
    credentialLeak || `${relayHttpRequestTargets.length} requests inspected`);
  record('全程无浏览器运行时错误', pageErrors.length === 0, pageErrors.slice(0, 3).join(' | '));
  record('页面启动阶段确实调用了 allowlisted 桌面 RPC',
    desktop.rpcRequests.some(request => request.command === 'get_settings')
      && desktop.rpcRequests.some(request => request.command === 'list_sessions'));

  console.log(`\nALL ${results.length} FULL WEBUI JOURNEYS PASS`);
}

main().catch(error => {
  console.error('FATAL full WebUI smoke:', error.stack || error.message);
  console.error('WebUI smoke diagnostics:', JSON.stringify({
    browserWebSocketUrls,
    recentBrowserFrames: browserWebSocketFrames.slice(-20),
    pageErrors: pageErrors.slice(-10),
    desktopMessages: desktop?.messages?.slice(-20) || [],
  }, null, 2));
  process.exitCode = 1;
}).finally(async () => {
  try { desktop?.ws?.close(); } catch (_) {}
  try { await browser?.close(); } catch (_) {}
  try { relay?.kill('SIGTERM'); } catch (_) {}
  try { fs.rmSync(browserProfile, { recursive: true, force: true }); } catch (_) {}
  await sleep(25);
});
