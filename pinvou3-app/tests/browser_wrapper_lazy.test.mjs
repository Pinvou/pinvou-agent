#!/usr/bin/env node
// End-to-end tests for the browser-wrapper.mjs lazy proxy, using fake Chrome
// and MCP binaries without touching a real browser:
//  1) the shim answers initialize/tools/list without preparing a browser or
//     writing a port file;
//  2) an unavailable native host reports host-backend-unavailable on Windows
//     and unsupported on macOS/Linux, without starting external browser helpers.
import assert from 'node:assert/strict';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as sleep } from 'node:timers/promises';
import { spawnNdjsonChild, stopNdjsonChild } from './helpers/ndjson-child-driver.mjs';

// This fixture verifies that the Windows-only Chrome DevTools backend never
// falls back to external Chrome. Linux uses BrowserCore/WebKitWebDriver and is
// covered by browser_core_protocol plus Linux integration tests.
if (process.platform !== 'win32') {
  console.log('skip: Windows Chrome DevTools no-host fallback test');
  process.exit(0);
}

// The Windows success path needs a real Tauri WebView2 host. Opt in only to
// verify that a missing host fails explicitly without falling back to Chrome.
if (process.env.PINVOU3_TEST_BROWSER_NO_HOST !== '1') {
  console.log('skip: Windows native-host path requires the Tauri app process');
  process.exit(0);
}

const WRAPPER = fileURLToPath(new URL(
  '../src-tauri/resources/common/bundle/mcp-servers/browser-wrapper.mjs',
  import.meta.url
));

// Fake MCP server: NDJSON stdio, static initialize/tools/list responses, and
// echo responses for other requests.
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
      process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:msg.id,result:{tools:[{name:'list_pages',description:'x',inputSchema:{type:'object'}}]}})+'\\n');
    }else if(msg.id!=null){
      process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:msg.id,result:{echoed:msg.method}})+'\\n');
    }
  }
});
`;

// Fake Chrome: serve /json/version on --remote-debugging-port, write a marker,
// and stay alive.
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
  toolsListResult: { tools: [{ name: 'list_pages', description: 'x', inputSchema: { type: 'object' } }] },
};

function makeFixture() {
  const root = mkdtempSync(join(tmpdir(), 'browser-wrapper-test-'));
  const binDir = join(root, 'pkg', 'build', 'src', 'bin');
  mkdirSync(binDir, { recursive: true });
  const mcpBin = join(binDir, 'fake-mcp.mjs');
  writeFileSync(mcpBin, FAKE_MCP);
  // The wrapper expects the catalog three levels above the MCP binary.
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

// Drive the wrapper with NDJSON requests and map response ids to resolvers.
function driveWrapper(fx, env = {}) {
  return spawnNdjsonChild({
    args: [WRAPPER, fx.mcpBin, fx.portJson],
    env: {
      ...process.env,
      PINVOU_BROWSER_CHROME_PATH: fx.chromeBin,
      FAKE_CHROME_MARKER: fx.marker,
      ...env,
    },
    stderr: 'inherit',
    timeoutMs: 30_000,
    collectResponses: true,
    unrefTimeout: true,
  });
}

async function cleanup(fx, child) {
  try {
    await stopNdjsonChild(child, 800);
  } catch {
    /* ignore */
  }
  // Port-file and last-error writes can race cleanup, so allow bounded retries.
  rmSync(fx.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
}

// 1) Lazy startup: the shim handles the handshake and catalog without starting
// Chrome or writing a port file.
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
      ['list_pages']
    );
    // Allow enough time for an incorrect eager startup to write its marker.
    await sleep(1500);
    assert.equal(existsSync(fx.marker), false, 'lazy tools/list must not start Chrome');
    assert.equal(existsSync(fx.portJson), false, 'lazy startup must not write a port file');
    assert.equal(child.exitCode, null, 'wrapper must remain alive');
  } finally {
    await cleanup(fx, child);
  }
}

// 2) An unavailable native host or automation backend fails explicitly without
// starting the configured external Chrome binary.
{
  const fx = makeFixture();
  const { child, send, request, responses } = driveWrapper(fx);
  try {
    await request('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 't', version: '0' } });
    send({ jsonrpc: '2.0', method: 'notifications/initialized' });
    await request('tools/list');
    assert.equal(existsSync(fx.marker), false);

    // Put request and cancellation in one stdin chunk so cancellation is
    // registered before the startup-failure microtask runs.
    const cancelledId = 9000;
    child.stdin.write([
      JSON.stringify({
        jsonrpc: '2.0',
        id: cancelledId,
        method: 'tools/call',
        params: { name: 'list_pages', arguments: {} },
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
      'startup failure must not reply to a cancelled buffered request',
    );
    assert.equal(
      existsSync(join(fx.root, 'home', 'last-error.json')),
      false,
      'a cancelled startup request must not pollute the next session last-error',
    );

    const call = await request('tools/call', { name: 'list_pages', arguments: {} });
    assert.ok(call.error, 'a missing native backend must return a JSON-RPC error');
    assert.match(call.error.message, /host-backend-unavailable/);
    assert.match(call.error.message, /external Chrome will not be started/);
    assert.equal(existsSync(fx.marker), false, 'tools/call must not start configured Chrome');
    assert.equal(existsSync(fx.portJson), false, 'external Chrome must not get a CDP port file');
    assert.equal(existsSync(fx.mcpAttemptFile), false, 'host failure must not start upstream MCP');
    const lastError = JSON.parse(readFileSync(join(fx.root, 'home', 'last-error.json'), 'utf8'));
    assert.deepEqual(
      Object.keys(lastError).sort((a, b) => a.localeCompare(b)),
      ['at', 'code'],
    );
    assert.equal(lastError.code, 'browser/host-backend-unavailable');
    // The wrapper remains in shim mode with a usable catalog so a later host
    // recovery or app upgrade can retry.
    const list = await request('tools/list');
    assert.equal(list.result.tools.length, 1);
    assert.equal(child.exitCode, null);
  } finally {
    await cleanup(fx, child);
  }
}

console.log('browser-wrapper lazy proxy tests: ok');
