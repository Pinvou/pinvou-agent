#!/usr/bin/env node
// browser-wrapper.mjs 懒启动代理的端到端测试（假 Chrome + 假 MCP bin，不碰真实浏览器）：
//  1) initialize/tools/list 由 shim 直接应答，不准备浏览器、不写端口文件；
//  2) 原生宿主不可用时，Windows 明确报 host-backend-unavailable，macOS/Linux
//     明确报 unsupported；即使配置了假 Chrome/MCP，也不启动外部浏览器或代理。
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as sleep } from 'node:timers/promises';

// 该 fixture 专门验证 Windows 私有的 Chrome DevTools 后端不会回退外部
// Chrome。Linux 已走 BrowserCore/WebKitWebDriver，由 browser_core_protocol 与
// Linux 集成测试覆盖，不应再把假 Chrome 注入它的运行路径。
if (process.platform !== 'win32') {
  console.log('skip: Windows Chrome DevTools 后端的无宿主回退测试');
  process.exit(0);
}

// Windows 成功路径需要真实 Tauri WebView2 宿主。默认跳过；CI/本地可设置下面的
// 测试开关，仅验证“宿主缺失时明确失败且不回退外部 Chrome”。
if (process.env.PINVOU3_TEST_BROWSER_NO_HOST !== '1') {
  console.log('skip: Windows 原生宿主链路需要 Tauri 应用进程');
  process.exit(0);
}

const WRAPPER = fileURLToPath(new URL(
  '../src-tauri/resources/common/bundle/mcp-servers/browser-wrapper.mjs',
  import.meta.url
));

// --- 假 MCP server：NDJSON stdio，initialize/tools/list 静态应答，其余请求 echo ---
const FAKE_MCP = `const fs=require('node:fs');
const failBeforeHandshake=Number(process.env.FAKE_MCP_FAIL_BEFORE_HANDSHAKE||0);
const attemptFile=process.env.FAKE_MCP_ATTEMPT_FILE||'';
if(failBeforeHandshake>0&&attemptFile){
  let attempt=0;
  try{attempt=Number(fs.readFileSync(attemptFile,'utf8'))||0;}catch{}
  attempt+=1;fs.writeFileSync(attemptFile,String(attempt));
  if(attempt<=failBeforeHandshake)process.exit(1);
}
let buf='';
process.stdin.on('data',d=>{
  buf+=d;let i;
  while((i=buf.indexOf('\\n'))>=0){
    const line=buf.slice(0,i);buf=buf.slice(i+1);
    if(!line.trim())continue;
    const msg=JSON.parse(line);
    if(msg.method==='initialize'){
      process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:msg.id,result:{protocolVersion:msg.params?.protocolVersion??'2024-11-05',capabilities:{tools:{}},serverInfo:{name:'fake-mcp',version:'0'}}})+'\\n');
    }else if(msg.method==='tools/list'){
      process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:msg.id,result:{tools:[{name:'fake_tool',description:'x',inputSchema:{type:'object'}}]}})+'\\n');
    }else if(msg.id!=null){
      process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:msg.id,result:{echoed:msg.method}})+'\\n');
    }
  }
});
`;

// --- 假 Chrome：按 --remote-debugging-port 起 /json/version 服务，写 marker 后常驻 ---
const FAKE_CHROME = `#!/usr/bin/env node
const http=require('node:http');
const fs=require('node:fs');
const portArg=process.argv.find(a=>a.startsWith('--remote-debugging-port='));
const port=Number(portArg.split('=')[1]);
const delay=Number(process.env.FAKE_CHROME_DELAY_MS||0);
setTimeout(()=>{
  fs.writeFileSync(process.env.FAKE_CHROME_MARKER,'started');
  http.createServer((req,res)=>{
    if(req.url==='/json/version'){res.writeHead(200,{'content-type':'application/json'});res.end('{"webSocketDebuggerUrl":"ws://127.0.0.1/"}');}
    else{res.writeHead(404);res.end();}
  }).listen(port,'127.0.0.1');
},delay);
setInterval(()=>{},10000);
`;

const CATALOG = {
  initializeResult: {
    protocolVersion: '2024-11-05',
    capabilities: { tools: {} },
    serverInfo: { name: 'fake-mcp', version: '0' },
  },
  toolsListResult: { tools: [{ name: 'fake_tool', description: 'x', inputSchema: { type: 'object' } }] },
};

function makeFixture() {
  const root = mkdtempSync(join(tmpdir(), 'browser-wrapper-test-'));
  const binDir = join(root, 'pkg', 'build', 'src', 'bin');
  mkdirSync(binDir, { recursive: true });
  const mcpBin = join(binDir, 'fake-mcp.mjs');
  writeFileSync(mcpBin, FAKE_MCP);
  // wrapper 约定 catalog 在 MCP bin 上三级（包根）。
  writeFileSync(join(root, 'pkg', 'catalog-shim.json'), JSON.stringify(CATALOG));
  const chromeBin = join(root, 'fake-chrome.cjs');
  writeFileSync(chromeBin, FAKE_CHROME);
  chmodSync(chromeBin, 0o755);
  return {
    root,
    mcpBin,
    chromeBin,
    portJson: join(root, 'home', 'cdp-port.json'),
    marker: join(root, 'chrome-started'),
    mcpAttemptFile: join(root, 'mcp-attempts'),
  };
}

// 驱动 wrapper：NDJSON 请求/应答，id → resolver。
function driveWrapper(fx, env = {}) {
  const child = spawn(process.execPath, [WRAPPER, fx.mcpBin, fx.portJson], {
    stdio: ['pipe', 'pipe', 'inherit'],
    env: {
      ...process.env,
      PINVOU_BROWSER_CHROME_PATH: fx.chromeBin,
      FAKE_CHROME_MARKER: fx.marker,
      ...env,
    },
  });
  let buf = '';
  const pending = new Map();
  const responses = [];
  child.stdout.on('data', (d) => {
    buf += d;
    let i;
    while ((i = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, i);
      buf = buf.slice(i + 1);
      if (!line.trim()) continue;
      const msg = JSON.parse(line);
      responses.push(msg);
      if (msg.id != null && pending.has(msg.id)) {
        pending.get(msg.id)(msg);
        pending.delete(msg.id);
      }
    }
  });
  let nextId = 1;
  const send = (msg) => child.stdin.write(JSON.stringify(msg) + '\n');
  const driver = {
    child,
    send,
    responses,
    lastId: 0,
    request(method, params = {}) {
      const id = nextId++;
      driver.lastId = id;
      return new Promise((resolve, reject) => {
        pending.set(id, resolve);
        setTimeout(() => {
          if (pending.delete(id)) reject(new Error(`${method} 应答超时`));
        }, 30000).unref();
        send({ jsonrpc: '2.0', id, method, params });
      });
    },
  };
  return driver;
}

async function cleanup(fx, child) {
  // 先走 stdin 关闭的优雅退出路径，再 SIGKILL 兜底。
  try {
    child.stdin.end();
    await sleep(800);
  } catch {
    /* ignore */
  }
  try {
    child.kill('SIGKILL');
  } catch {
    /* ignore */
  }
  // SIGKILL 后按 fixture 路径名兜底清理测试子进程。
  try {
    spawnSync('pkill', ['-9', '-f', fx.root], { stdio: 'ignore' });
  } catch {
    /* ignore */
  }
  // 与 wrapper 的端口文件/last-error 写入存在竞态，加宽限重试。
  rmSync(fx.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
}

// 1) 懒启动：握手与工具目录由 shim 应答，全程不启动 Chrome、不写端口文件。
{
  const fx = makeFixture();
  const { child, send, request } = driveWrapper(fx);
  try {
    const init = await request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'test', version: '0' },
    });
    assert.equal(init.result.serverInfo.name, 'fake-mcp');
    assert.equal(init.result.protocolVersion, '2024-11-05');
    send({ jsonrpc: '2.0', method: 'notifications/initialized' });
    const list = await request('tools/list');
    assert.deepEqual(
      list.result.tools.map((t) => t.name),
      ['fake_tool']
    );
    // 给 wrapper 充分时间（若它错误地 eager 启动，假 Chrome 会写 marker/端口文件）。
    await sleep(1500);
    assert.equal(existsSync(fx.marker), false, '懒启动：tools/list 不得启动 Chrome');
    assert.equal(existsSync(fx.portJson), false, '懒启动：不得写端口文件');
    assert.equal(child.exitCode, null, 'wrapper 应保持存活');
  } finally {
    await cleanup(fx, child);
  }
}

// 2) 原生宿主/自动化后端不可用时明确失败，不启动已配置的外部 Chrome。
{
  const fx = makeFixture();
  const { child, send, request, responses } = driveWrapper(fx);
  try {
    await request('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 't', version: '0' } });
    send({ jsonrpc: '2.0', method: 'notifications/initialized' });
    await request('tools/list');
    assert.equal(existsSync(fx.marker), false);

    // 请求与取消放进同一个 stdin chunk，稳定覆盖“启动失败 microtask 前已登记取消”。
    const cancelledId = 9000;
    child.stdin.write([
      JSON.stringify({
        jsonrpc: '2.0',
        id: cancelledId,
        method: 'tools/call',
        params: { name: 'fake_tool', arguments: {} },
      }),
      JSON.stringify({
        jsonrpc: '2.0',
        method: 'notifications/cancelled',
        params: { requestId: cancelledId, reason: 'user' },
      }),
      '',
    ].join('\n'));
    await sleep(250);
    assert.equal(
      responses.some((message) => message.id === cancelledId),
      false,
      '启动失败不得向已取消的 buffered 请求回错误',
    );

    const call = await request('tools/call', { name: 'fake_tool', arguments: {} });
    assert.ok(call.error, '原生后端缺失时应应答 JSON-RPC error');
    assert.match(call.error.message, /host-backend-unavailable/);
    assert.match(call.error.message, /不会启动外部 Chrome/);
    assert.equal(existsSync(fx.marker), false, 'tools/call 不得启动已配置的外部 Chrome');
    assert.equal(existsSync(fx.portJson), false, '不得为外部 Chrome 写入 CDP 端口文件');
    assert.equal(existsSync(fx.mcpAttemptFile), false, '宿主失败时不得启动上游 MCP 子进程');
    const lastError = JSON.parse(readFileSync(join(fx.root, 'home', 'last-error.json'), 'utf8'));
    assert.match(lastError.reason, /host-backend-unavailable/);
    // wrapper 保持 shim 态，目录仍然可用，后续可在应用升级/宿主恢复后重试。
    const list = await request('tools/list');
    assert.equal(list.result.tools.length, 1);
    assert.equal(child.exitCode, null);
  } finally {
    await cleanup(fx, child);
  }
}

console.log('browser-wrapper lazy proxy tests: ok');
