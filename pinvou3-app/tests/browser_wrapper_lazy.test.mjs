#!/usr/bin/env node
// browser-wrapper.mjs 懒启动代理的端到端测试（假 Chrome + 假 MCP bin，不碰真实浏览器）：
//  1) 懒启动：initialize/tools/list 由 shim 直接应答，不启动 Chrome、不写端口文件；
//  2) 首个 tools/call 触发启动：拉起 Chrome → 写端口文件 → 握手 MCP 子进程 → 透明代理；
//  3) 启动失败：tools/call 收到可读错误，wrapper 保持存活，tools/list 仍可应答；
//  4) 启动期取消：notifications/cancelled 的请求在启动完成后不被转发。
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

// 假 Chrome 依赖 shebang 直接执行，Windows 无此语义；Windows 侧由真实链路验证。
if (process.platform === 'win32') {
  console.log('skip: win32 不支持 shebang 假 Chrome');
  process.exit(0);
}

const WRAPPER = new URL(
  '../src-tauri/resources/common/bundle/mcp-servers/browser-wrapper.mjs',
  import.meta.url
).pathname;

// --- 假 MCP server：NDJSON stdio，initialize/tools/list 静态应答，其余请求 echo ---
const FAKE_MCP = `let buf='';
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
    profileDir: join(root, 'home', 'profile'),
    marker: join(root, 'chrome-started'),
  };
}

// 驱动 wrapper：NDJSON 请求/应答，id → resolver。
function driveWrapper(fx, env = {}) {
  const child = spawn(process.execPath, [WRAPPER, fx.mcpBin, fx.portJson, fx.profileDir], {
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
  // 先走 stdin 关闭的优雅退出路径（wrapper 回收自启 Chrome），再 SIGKILL 兜底。
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
  // SIGKILL 的 wrapper 来不及回收自启假 Chrome：按 fixture 路径名兜底清理。
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

// 2) 首个 tools/call 触发启动：拉起 Chrome、写端口文件、握手后透明代理。
{
  const fx = makeFixture();
  const { child, send, request } = driveWrapper(fx);
  try {
    await request('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 't', version: '0' } });
    send({ jsonrpc: '2.0', method: 'notifications/initialized' });
    await request('tools/list');
    assert.equal(existsSync(fx.marker), false);
    const call = await request('tools/call', { name: 'fake_tool', arguments: {} });
    assert.equal(call.result.echoed, 'tools/call');
    assert.equal(existsSync(fx.marker), true, 'tools/call 应触发 Chrome 启动');
    const portFile = JSON.parse(readFileSync(fx.portJson, 'utf8'));
    assert.ok(portFile.port > 0 && portFile.port < 65536);
    assert.equal(portFile.owner, 'mcp');
    // 代理建立后后续调用直接透传。
    const call2 = await request('tools/call', { name: 'fake_tool', arguments: {} });
    assert.equal(call2.result.echoed, 'tools/call');
  } finally {
    await cleanup(fx, child);
  }
}

// 3) 启动失败（Chrome 不存在）：tools/call 收到可读错误，wrapper 存活可重试。
{
  const fx = makeFixture();
  const { child, send, request } = driveWrapper(fx, {
    PINVOU_BROWSER_CHROME_PATH: join(fx.root, 'no-such-chrome'),
  });
  try {
    await request('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 't', version: '0' } });
    send({ jsonrpc: '2.0', method: 'notifications/initialized' });
    const call = await request('tools/call', { name: 'fake_tool', arguments: {} });
    assert.ok(call.error, '启动失败应应答 JSON-RPC error');
    assert.match(call.error.message, /浏览器不可用/);
    // 失败原因落盘（Rust 侧 browser_unavailability_reason 注入来源）。
    const lastError = JSON.parse(readFileSync(join(fx.root, 'home', 'last-error.json'), 'utf8'));
    assert.match(lastError.reason, /未找到 Chrome/);
    // wrapper 保持 shim 态存活：目录仍可应答，不制造 connect 期失败噪音。
    const list = await request('tools/list');
    assert.equal(list.result.tools.length, 1);
    assert.equal(child.exitCode, null);
  } finally {
    await cleanup(fx, child);
  }
}

// 4) 启动期取消：notifications/cancelled 的请求在启动完成后不被转发。
{
  const fx = makeFixture();
  // 假 Chrome 延迟 2s 就绪，给取消通知留出确定的启动窗口。
  const driver = driveWrapper(fx, { FAKE_CHROME_DELAY_MS: '2000' });
  const { child, send, request, responses } = driver;
  try {
    await request('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 't', version: '0' } });
    send({ jsonrpc: '2.0', method: 'notifications/initialized' });
    const cancelled = request('tools/call', { name: 'fake_tool', arguments: {} });
    const cancelledId = driver.lastId;
    send({ jsonrpc: '2.0', method: 'notifications/cancelled', params: { requestId: cancelledId } });
    cancelled.catch(() => {}); // 取消的请求不应答：吞掉超时 reject
    await sleep(4000); // 等启动完成（假 Chrome 2s 就绪 + 握手）
    assert.equal(existsSync(fx.marker), true, '启动应已完成');
    assert.equal(
      responses.some((m) => m.id === cancelledId),
      false,
      '已取消的请求不应转发/应答'
    );
    const call = await request('tools/call', { name: 'fake_tool', arguments: {} });
    assert.equal(call.result.echoed, 'tools/call', '取消后代理仍正常工作');
  } finally {
    await cleanup(fx, child);
  }
}

console.log('browser-wrapper lazy proxy tests: ok');
