import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as sleep } from 'node:timers/promises';
import test from 'node:test';

const WRAPPER = fileURLToPath(new URL(
  '../src-tauri/resources/common/bundle/mcp-servers/browser-wrapper.mjs',
  import.meta.url,
));
const SESSION_ID = 'windows-lifecycle-a';
const SESSION_TOKEN = '0123456789abcdef';
const OTHER_SESSION_TOKEN = 'fedcba9876543210';

const FAKE_MCP = String.raw`
import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';

const workspacePath = process.env.FAKE_BROWSER_WORKSPACE;
const modePath = process.env.FAKE_BROWSER_MCP_MODE;
const auditPath = process.env.FAKE_BROWSER_MCP_AUDIT;
let selectedPageId = null;
let input = '';
const pendingToolTimers = new Map();

function audit(value) {
  appendFileSync(auditPath, JSON.stringify({ pid: process.pid, at: Date.now(), ...value }) + '\n');
}

function mode() {
  try { return JSON.parse(readFileSync(modePath, 'utf8')); } catch { return {}; }
}

function currentPages() {
  const workspace = JSON.parse(readFileSync(workspacePath, 'utf8'));
  const currentMode = mode();
  const active = workspace.tabs.find((candidate) => candidate.token === workspace.active_tab);
  const activeId = Number(active?.target_id.match(/-(\d+)$/)?.[1] || 1);
  return workspace.tabs.map((tab) => {
    const suffix = Number(tab.target_id.match(/-(\d+)$/)?.[1] || 1);
    return {
      id: suffix,
      target_id: tab.target_id,
      url: currentMode.bootstrapBlank && !currentMode.navigated
        ? 'about:blank'
        : 'https://' + suffix + '.example/',
      title: 'Page ' + suffix,
      selected: (selectedPageId ?? activeId) === suffix,
    };
  });
}

function write(message) {
  process.stdout.write(JSON.stringify(message) + '\n');
}

function result(id, value) {
  write({ jsonrpc: '2.0', id, result: value });
}

function error(id, message) {
  write({ jsonrpc: '2.0', id, error: { code: -32000, message } });
}

audit({ kind: 'start' });
process.stdin.on('data', (chunk) => {
  input += chunk;
  let newline;
  while ((newline = input.indexOf('\n')) >= 0) {
    const line = input.slice(0, newline);
    input = input.slice(newline + 1);
    if (!line.trim()) continue;
    const message = JSON.parse(line);
    if (message.method === 'initialize') {
      const currentMode = mode();
      if (currentMode.handshakeError) {
        error(message.id, 'configured startup handshake rejection');
        continue;
      }
      const finishHandshake = () => result(message.id, {
          protocolVersion: message.params?.protocolVersion ?? '2024-11-05',
          capabilities: { tools: {} },
          serverInfo: { name: 'fake-windows-mcp', version: '0' },
        });
      const handshakeDelayMs = Number(currentMode.handshakeDelayMs) || 0;
      if (handshakeDelayMs > 0) setTimeout(finishHandshake, handshakeDelayMs);
      else finishHandshake();
      continue;
    }
    if (message.method === 'tools/list') {
      if (mode().toolsListError) {
        error(message.id, 'configured runtime catalog rejection');
        continue;
      }
      result(message.id, {
        tools: [
          { name: 'list_pages', inputSchema: { type: 'object', properties: {} } },
          {
            name: 'select_page',
            inputSchema: {
              type: 'object',
              properties: { pageId: { type: 'number' } },
              required: ['pageId'],
            },
          },
          {
            name: 'navigate_page',
            inputSchema: {
              type: 'object',
              properties: {
                pageId: { type: 'number' },
                type: { type: 'string' },
                url: { type: 'string' },
              },
              required: ['pageId'],
            },
          },
          {
            name: 'click',
            annotations: { category: 'input' },
            inputSchema: {
              type: 'object',
              properties: {
                pageId: { type: 'number' },
                uid: { type: 'string' },
              },
              required: ['pageId', 'uid'],
            },
          },
          {
            name: 'type_text',
            annotations: { category: 'input' },
            inputSchema: {
              type: 'object',
              properties: {
                pageId: { type: 'number' },
                text: { type: 'string' },
              },
              required: ['pageId', 'text'],
            },
          },
          {
            name: 'evaluate_script',
            inputSchema: {
              type: 'object',
              properties: {
                pageId: { type: 'number' },
                function: { type: 'string' },
              },
              required: ['pageId', 'function'],
            },
          },
          {
            name: 'take_snapshot',
            inputSchema: {
              type: 'object',
              properties: { pageId: { type: 'number' } },
              required: ['pageId'],
            },
          },
          {
            name: 'close_page',
            inputSchema: {
              type: 'object',
              properties: { pageId: { type: 'number' } },
              required: ['pageId'],
            },
          },
        ],
      });
      continue;
    }
    if (message.method === 'notifications/cancelled') {
      audit({
        kind: 'cancel',
        requestId: message.params?.requestId,
        reason: message.params?.reason,
      });
      // Deliberately ignore cooperative cancellation. The wrapper must keep
      // the pending request alive until this fake upstream really settles,
      // rather than ending the host operation while trusted input may remain.
      continue;
    }
    if (message.method !== 'tools/call') continue;

    const name = message.params?.name;
    const args = message.params?.arguments ?? {};
    audit({ kind: 'tool', name, args });
    if (name === 'list_pages') {
      const currentMode = mode();
      if (currentMode.listPagesErrorAfterNavigate && currentMode.navigated) {
        error(message.id, 'configured post-navigation list rejection');
        continue;
      }
      if (currentMode.listPagesErrorWhenSingle) {
        const workspace = JSON.parse(readFileSync(workspacePath, 'utf8'));
        if (workspace.tabs.length === 1) {
          error(message.id, 'configured post-close list rejection');
          continue;
        }
      }
      const pages = currentPages();
      result(message.id, {
        content: [{
          type: 'text',
          text: '## Pages\n' + pages.map((page) =>
            page.id + ': ' + page.title + ' (' + page.url + ')' +
              (page.selected ? ' [selected]' : '')
          ).join('\n'),
        }],
        structuredContent: { pages },
      });
      continue;
    }
    if (name === 'navigate_page') {
      const currentMode = mode();
      writeFileSync(modePath, JSON.stringify({ ...currentMode, navigated: true }));
      if (currentMode.navigateError) error(message.id, currentMode.navigateError);
      else if (currentMode.navigateToolError) {
        result(message.id, {
          isError: true,
          content: [{ type: 'text', text: currentMode.navigateToolError }],
        });
      } else result(message.id, { content: [{ type: 'text', text: 'navigated' }] });
      continue;
    }
    if (name === 'select_page') {
      selectedPageId = args.pageId;
      result(message.id, { content: [{ type: 'text', text: 'selected' }] });
      continue;
    }
    if (name === 'click') {
      const currentMode = mode();
      const complete = () => {
        pendingToolTimers.delete(message.id);
        audit({ kind: 'tool-complete', name, requestId: message.id });
        if (currentMode.clickError) error(message.id, currentMode.clickError);
        else result(message.id, { content: [{ type: 'text', text: 'clicked' }] });
      };
      const delayMs = Number(currentMode.clickDelayMs) || 0;
      if (delayMs > 0) {
        pendingToolTimers.set(message.id, setTimeout(complete, delayMs));
      } else {
        complete();
      }
      continue;
    }
    if (name === 'type_text' || name === 'evaluate_script') {
      const currentMode = mode();
      const modePrefix = name === 'type_text' ? 'typeText' : 'evaluate';
      audit({ kind: 'tool-complete', name, requestId: message.id });
      if (currentMode[modePrefix + 'Error']) {
        error(message.id, currentMode[modePrefix + 'Error']);
      } else if (currentMode[modePrefix + 'ToolError']) {
        result(message.id, {
          isError: true,
          content: [{ type: 'text', text: currentMode[modePrefix + 'ToolError'] }],
        });
      } else {
        result(message.id, {
          content: [{ type: 'text', text: name === 'type_text' ? 'typed' : 'evaluated' }],
        });
      }
      continue;
    }
    if (name === 'take_snapshot') {
      const currentMode = mode();
      const complete = () => {
        pendingToolTimers.delete(message.id);
        audit({ kind: 'tool-complete', name, requestId: message.id });
        if (currentMode.snapshotToolError) {
          result(message.id, {
            isError: true,
            content: [{ type: 'text', text: currentMode.snapshotToolError }],
          });
        } else result(message.id, { content: [{ type: 'text', text: 'snapshot' }] });
      };
      const delayMs = Number(currentMode.snapshotDelayMs) || 0;
      if (delayMs > 0) {
        pendingToolTimers.set(message.id, setTimeout(complete, delayMs));
      } else {
        complete();
      }
      continue;
    }
    error(message.id, 'unexpected tool: ' + name);
  }
});
`;

const CATALOG = {
  initializeResult: {
    protocolVersion: '2024-11-05',
    capabilities: { tools: {} },
    serverInfo: { name: 'fake-windows-mcp', version: '0' },
  },
  toolsListResult: {
    tools: [
      { name: 'list_pages', inputSchema: { type: 'object', properties: {} } },
      {
        name: 'select_page',
        inputSchema: {
          type: 'object',
          properties: { pageId: { type: 'number' } },
          required: ['pageId'],
        },
      },
      {
        name: 'navigate_page',
        inputSchema: {
          type: 'object',
          properties: { type: { type: 'string' }, url: { type: 'string' } },
          required: ['url'],
        },
      },
      {
        name: 'click',
        annotations: { category: 'input' },
        inputSchema: {
          type: 'object',
          properties: { uid: { type: 'string' } },
          required: ['uid'],
        },
      },
      {
        name: 'type_text',
        annotations: { category: 'input' },
        inputSchema: {
          type: 'object',
          properties: { text: { type: 'string' } },
          required: ['text'],
        },
      },
      {
        name: 'evaluate_script',
        inputSchema: {
          type: 'object',
          properties: { function: { type: 'string' } },
          required: ['function'],
        },
      },
      {
        name: 'take_snapshot',
        inputSchema: { type: 'object', properties: {} },
      },
      {
        name: 'close_page',
        inputSchema: {
          type: 'object',
          properties: { pageId: { type: 'number' } },
          required: ['pageId'],
        },
      },
    ],
  },
};

function atomicWriteJson(path, value) {
  const temporary = `${path}.${process.pid}.tmp`;
  writeFileSync(temporary, JSON.stringify(value));
  renameSync(temporary, path);
}

function readAudit(path) {
  if (!existsSync(path)) return [];
  return readFileSync(path, 'utf8')
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}

async function waitUntil(predicate, message, timeoutMs = 3_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await sleep(10);
  }
  assert.fail(message);
}

async function makeFixture() {
  const root = mkdtempSync(join(tmpdir(), 'pinvou-windows-lifecycle-'));
  const binDirectory = join(root, 'pkg', 'build', 'src', 'bin');
  const workspaceDirectory = join(root, 'workspaces');
  mkdirSync(binDirectory, { recursive: true });
  mkdirSync(workspaceDirectory, { recursive: true });

  const mcpBin = join(binDirectory, 'fake-mcp.mjs');
  const workspacePath = join(workspaceDirectory, `${SESSION_TOKEN}.json`);
  const otherWorkspacePath = join(workspaceDirectory, `${OTHER_SESSION_TOKEN}.json`);
  const modePath = join(root, 'mcp-mode.json');
  const auditPath = join(root, 'mcp-audit.ndjson');
  const portPath = join(root, 'cdp-port.json');
  writeFileSync(mcpBin, FAKE_MCP);
  writeFileSync(join(root, 'pkg', 'catalog-shim.json'), JSON.stringify(CATALOG));
  writeFileSync(modePath, '{}');
  writeFileSync(auditPath, '');
  atomicWriteJson(otherWorkspacePath, {
    version: 2,
    mapping_authority: 'host',
    revision: 1,
    session_token: OTHER_SESSION_TOKEN,
    active_tab: OTHER_SESSION_TOKEN,
    tabs: [{ token: OTHER_SESSION_TOKEN, target_id: 'target-b-99' }],
  });

  const cdp = createServer((request, response) => {
    if (request.url === '/json/version') {
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end('{"webSocketDebuggerUrl":"ws://127.0.0.1/devtools/browser/fake"}');
    } else {
      response.writeHead(404);
      response.end();
    }
  });
  await new Promise((resolve, reject) => {
    cdp.once('error', reject);
    cdp.listen(0, '127.0.0.1', resolve);
  });
  const port = cdp.address().port;
  atomicWriteJson(portPath, { port, owner: 'app' });

  return {
    root,
    mcpBin,
    workspacePath,
    otherWorkspacePath,
    modePath,
    auditPath,
    portPath,
    cdp,
  };
}

function createFakeHost(fixture) {
  const requestDirectory = join(fixture.root, 'host-requests');
  const handled = new Set();
  const state = {
    prepareCalls: 0,
    generation: 0,
    preparePublishesWorkspace: true,
    operationCalls: new Map(),
    operationLog: [],
    operationTimeline: [],
    operationErrors: new Map(),
    malformedOperationResponses: new Set(),
    deferredOperations: new Set(),
    failure: null,
  };

  const publishWorkspace = () => {
    state.generation += 1;
    atomicWriteJson(fixture.workspacePath, {
      version: 2,
      mapping_authority: 'host',
      revision: state.generation,
      session_token: SESSION_TOKEN,
      active_tab: SESSION_TOKEN,
      tabs: [{ token: SESSION_TOKEN, target_id: `target-a-${state.generation}` }],
    });
  };

  const respond = (request, response) => {
    atomicWriteJson(
      join(requestDirectory, `${SESSION_TOKEN}-${request.request_id}.response`),
      {
        protocol_version: request.protocol_version,
        request_id: request.request_id,
        idempotency_key: request.idempotency_key,
        ...response,
      },
    );
  };

  const timer = setInterval(() => {
    if (!existsSync(requestDirectory) || state.failure) return;
    try {
      for (const name of readdirSync(requestDirectory)) {
        if (!name.endsWith('.json') || handled.has(name)) continue;
        const request = JSON.parse(readFileSync(join(requestDirectory, name), 'utf8'));
        if (state.deferredOperations.has(request.operation)) continue;
        handled.add(name);
        state.operationCalls.set(
          request.operation,
          (state.operationCalls.get(request.operation) || 0) + 1,
        );
        state.operationLog.push(request.operation);
        state.operationTimeline.push({ operation: request.operation, at: Date.now() });

        if (request.operation === 'prepare') {
          state.prepareCalls += 1;
          if (state.preparePublishesWorkspace) publishWorkspace();
          respond(request, { ok: true, result: { ready: true } });
          continue;
        }

        const configuredError = state.operationErrors.get(request.operation);
        if (configuredError) {
          respond(request, { ok: false, error: configuredError });
          continue;
        }

        if (request.operation === 'activate_tab') {
          const workspace = JSON.parse(readFileSync(fixture.workspacePath, 'utf8'));
          const tab = workspace.tabs.find((candidate) => candidate.token === request.tab_token);
          if (!tab) {
            respond(request, { ok: false, error: 'browser/page-not-found' });
            continue;
          }
          respond(request, {
            ok: true,
            result: {
              sessionId: SESSION_ID,
              tabToken: tab.token,
              targetId: tab.target_id,
              revision: workspace.revision,
              owner: 'agent',
              lease: 'a'.repeat(32),
            },
          });
          continue;
        }

        if (request.operation === 'close_tab') {
          const workspace = JSON.parse(readFileSync(fixture.workspacePath, 'utf8'));
          const tabs = workspace.tabs.filter((tab) => tab.token !== request.tab_token);
          if (tabs.length === workspace.tabs.length || tabs.length === 0) {
            respond(request, { ok: false, error: 'browser/page-not-found' });
            continue;
          }
          atomicWriteJson(fixture.workspacePath, {
            ...workspace,
            revision: workspace.revision + 1,
            active_tab: tabs[0].token,
            tabs,
          });
          if (state.malformedOperationResponses.has(request.operation)) {
            atomicWriteJson(
              join(requestDirectory, `${SESSION_TOKEN}-${request.request_id}.response`),
              { malformed: true },
            );
          } else {
            respond(request, { ok: true, result: {} });
          }
          continue;
        }

        if ([
          'assert_host_lease',
          'begin_agent_operation',
          'refresh_agent_input',
          'refresh_agent_operation',
          'end_agent_operation',
        ].includes(request.operation)) {
          respond(request, { ok: true, result: { accepted: true } });
          continue;
        }

        respond(request, { ok: false, error: `unexpected operation: ${request.operation}` });
      }
    } catch (error) {
      state.failure = error;
    }
  }, 5);

  return {
    state,
    stopSessionA() {
      try { unlinkSync(fixture.workspacePath); } catch { /* already stopped */ }
    },
    close() {
      clearInterval(timer);
      if (state.failure) throw state.failure;
    },
  };
}

function driveWrapper(fixture) {
  const child = spawn(process.execPath, [WRAPPER, fixture.mcpBin, fixture.portPath], {
    stdio: ['pipe', 'pipe', 'pipe'],
    env: {
      ...process.env,
      PINVOU3_BROWSER_SESSION_ID: SESSION_ID,
      PINVOU3_BROWSER_SESSION_TOKEN: SESSION_TOKEN,
      FAKE_BROWSER_WORKSPACE: fixture.workspacePath,
      FAKE_BROWSER_MCP_MODE: fixture.modePath,
      FAKE_BROWSER_MCP_AUDIT: fixture.auditPath,
    },
  });
  let stdout = '';
  let stderr = '';
  let nextId = 1;
  const pending = new Map();

  child.stderr.on('data', (chunk) => { stderr += chunk; });
  child.stdout.on('data', (chunk) => {
    stdout += chunk;
    let newline;
    while ((newline = stdout.indexOf('\n')) >= 0) {
      const line = stdout.slice(0, newline);
      stdout = stdout.slice(newline + 1);
      if (!line.trim()) continue;
      const message = JSON.parse(line);
      const waiter = pending.get(message.id);
      if (!waiter) continue;
      pending.delete(message.id);
      clearTimeout(waiter.timeout);
      waiter.resolve(message);
    }
  });

  const startRequest = (method, params = {}) => {
    const id = nextId++;
    const response = new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (!pending.delete(id)) return;
        reject(new Error(`${method} timed out; wrapper stderr: ${stderr}`));
      }, 15_000);
      pending.set(id, { resolve, reject, timeout });
    });
    child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`);
    return { id, response };
  };

  return {
    child,
    stderrText() {
      return stderr;
    },
    notify(method, params = {}) {
      child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', method, params })}\n`);
    },
    startRequest,
    abandonRequest(id) {
      const waiter = pending.get(id);
      if (!waiter) return;
      pending.delete(id);
      clearTimeout(waiter.timeout);
      waiter.resolve({ abandoned: true });
    },
    request(method, params = {}) {
      return startRequest(method, params).response;
    },
  };
}

async function stopProcess(child) {
  if (child.exitCode == null) child.stdin.end();
  if (child.exitCode == null) {
    await Promise.race([
      new Promise((resolve) => child.once('exit', resolve)),
      sleep(500),
    ]);
  }
  if (child.exitCode == null) child.kill('SIGKILL');
}

function processIsAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

test('Windows proxy: a deliberately killed failed startup returns to reusable shim state', {
  skip: process.platform !== 'win32',
}, async () => {
  const fixture = await makeFixture();
  const host = createFakeHost(fixture);
  const wrapper = driveWrapper(fixture);

  try {
    await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'windows-startup-retry-test', version: '0' },
    });
    wrapper.notify('notifications/initialized');
    writeFileSync(fixture.modePath, JSON.stringify({ handshakeError: true }));

    const failed = await wrapper.request('tools/call', {
      name: 'list_pages',
      arguments: {},
    });
    assert.match(failed.error.message, /configured startup handshake rejection/);
    await waitUntil(
      () => readAudit(fixture.auditPath).filter((entry) => entry.kind === 'start').length === 1,
      'the failed startup attempt did not launch',
    );
    await sleep(100);
    assert.equal(wrapper.child.exitCode, null, 'failed-attempt cleanup must not exit the shim');
    assert.equal(wrapper.child.signalCode, null, 'failed-attempt cleanup must not signal the shim');

    writeFileSync(fixture.modePath, '{}');
    const recovered = await wrapper.request('tools/call', {
      name: 'list_pages',
      arguments: {},
    });
    assert.equal(recovered.result.structuredContent.pages.length, 1);
    assert.equal(
      readAudit(fixture.auditPath).filter((entry) => entry.kind === 'start').length,
      2,
      'the next distinct request must launch exactly one fresh MCP child',
    );
  } finally {
    host.close();
    await stopProcess(wrapper.child);
    await new Promise((resolve) => fixture.cdp.close(resolve));
    rmSync(fixture.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('Windows proxy: post-handshake setup failure retires child before shim retry', {
  skip: process.platform !== 'win32',
}, async () => {
  const fixture = await makeFixture();
  const host = createFakeHost(fixture);
  const wrapper = driveWrapper(fixture);

  try {
    await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'windows-post-handshake-retry-test', version: '0' },
    });
    wrapper.notify('notifications/initialized');
    writeFileSync(fixture.modePath, JSON.stringify({ toolsListError: true }));

    const failed = await wrapper.request('tools/call', {
      name: 'list_pages',
      arguments: {},
    });
    assert.match(failed.error.message, /configured runtime catalog rejection/);
    const firstPid = readAudit(fixture.auditPath)
      .filter((entry) => entry.kind === 'start')
      .at(-1)?.pid;
    assert.ok(Number.isInteger(firstPid));
    await waitUntil(
      () => !processIsAlive(firstPid),
      'post-handshake setup child survived after wrapper returned to shim',
    );
    assert.equal(wrapper.child.exitCode, null, 'retiring a setup child must preserve the shim');

    writeFileSync(fixture.modePath, '{}');
    const recovered = await wrapper.request('tools/call', {
      name: 'list_pages',
      arguments: {},
    });
    assert.equal(recovered.result.structuredContent.pages.length, 1);
    assert.equal(
      readAudit(fixture.auditPath).filter((entry) => entry.kind === 'start').length,
      2,
      'retry must launch one fresh child after retiring the failed setup child',
    );
  } finally {
    host.close();
    await stopProcess(wrapper.child);
    await new Promise((resolve) => fixture.cdp.close(resolve));
    rmSync(fixture.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('Windows proxy: stdin shutdown cannot release a request buffered during startup', {
  skip: process.platform !== 'win32',
}, async () => {
  const fixture = await makeFixture();
  const host = createFakeHost(fixture);
  const wrapper = driveWrapper(fixture);

  try {
    await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'windows-startup-shutdown-test', version: '0' },
    });
    wrapper.notify('notifications/initialized');
    writeFileSync(fixture.modePath, JSON.stringify({ handshakeDelayMs: 500 }));

    const buffered = wrapper.startRequest('tools/call', {
      name: 'click',
      arguments: { uid: 'must-not-run-after-stdin-close' },
    });
    wrapper.abandonRequest(buffered.id);
    await waitUntil(
      () => readAudit(fixture.auditPath).some((entry) => entry.kind === 'start'),
      'the startup child was never launched',
    );
    wrapper.child.stdin.end();
    if (wrapper.child.exitCode == null && wrapper.child.signalCode == null) {
      await Promise.race([
        new Promise((resolve) => wrapper.child.once('exit', resolve)),
        sleep(3_000).then(() => { throw new Error('startup shutdown did not exit promptly'); }),
      ]);
    }
    assert.equal(
      readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'click').length,
      0,
      'a buffered page action must not dispatch after stdin has closed',
    );
  } finally {
    host.close();
    await stopProcess(wrapper.child);
    await new Promise((resolve) => fixture.cdp.close(resolve));
    rmSync(fixture.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('Windows proxy: committed close with unusable host acknowledgement is non-retryable', {
  skip: process.platform !== 'win32',
}, async () => {
  const fixture = await makeFixture();
  const host = createFakeHost(fixture);
  const wrapper = driveWrapper(fixture);

  try {
    await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'windows-close-ack-test', version: '0' },
    });
    wrapper.notify('notifications/initialized');
    await wrapper.request('tools/call', { name: 'list_pages', arguments: {} });

    const workspace = JSON.parse(readFileSync(fixture.workspacePath, 'utf8'));
    atomicWriteJson(fixture.workspacePath, {
      ...workspace,
      revision: workspace.revision + 1,
      tabs: [
        ...workspace.tabs,
        { token: 'aaaaaaaaaaaaaaaa', target_id: 'target-a-22' },
      ],
    });
    const pages = await wrapper.request('tools/call', {
      name: 'list_pages',
      arguments: {},
    });
    assert.deepEqual(
      pages.result.structuredContent.pages.map((page) => page.id),
      [1, 22],
    );

    host.state.malformedOperationResponses.add('close_tab');
    const beforeClose = host.state.operationCalls.get('close_tab') || 0;
    const closed = await wrapper.request('tools/call', {
      name: 'close_page',
      arguments: { pageId: 1 },
    });
    assert.equal(
      closed.result.structuredContent.errorCode,
      'browser/action-commit-unknown-after-host-acknowledgement-loss',
    );
    assert.equal(closed.result.structuredContent.actionCommitState, 'unknown');
    assert.equal(closed.result.structuredContent.retryable, false);
    assert.equal(
      (host.state.operationCalls.get('close_tab') || 0) - beforeClose,
      1,
      'the logical close must be dispatched exactly once',
    );
    const committedWorkspace = JSON.parse(readFileSync(fixture.workspacePath, 'utf8'));
    assert.deepEqual(committedWorkspace.tabs.map((tab) => tab.token), ['aaaaaaaaaaaaaaaa']);

    const recovered = await wrapper.request('tools/call', {
      name: 'list_pages',
      arguments: {},
    });
    assert.deepEqual(
      recovered.result.structuredContent.pages.map((page) => page.id),
      [22],
      'the wrapper remains usable and discovers the already-committed close',
    );
  } finally {
    host.close();
    await stopProcess(wrapper.child);
    await new Promise((resolve) => fixture.cdp.close(resolve));
    rmSync(fixture.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('Windows proxy: committed close stays non-retryable when post-commit page sync fails', {
  skip: process.platform !== 'win32',
}, async () => {
  const fixture = await makeFixture();
  const host = createFakeHost(fixture);
  const wrapper = driveWrapper(fixture);

  try {
    await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'windows-close-post-sync-test', version: '0' },
    });
    wrapper.notify('notifications/initialized');
    await wrapper.request('tools/call', { name: 'list_pages', arguments: {} });

    const workspace = JSON.parse(readFileSync(fixture.workspacePath, 'utf8'));
    atomicWriteJson(fixture.workspacePath, {
      ...workspace,
      revision: workspace.revision + 1,
      tabs: [
        ...workspace.tabs,
        { token: 'aaaaaaaaaaaaaaaa', target_id: 'target-a-22' },
      ],
    });
    await wrapper.request('tools/call', { name: 'list_pages', arguments: {} });
    writeFileSync(fixture.modePath, JSON.stringify({ listPagesErrorWhenSingle: true }));

    const beforeClose = host.state.operationCalls.get('close_tab') || 0;
    const closed = await wrapper.request('tools/call', {
      name: 'close_page',
      arguments: { pageId: 1 },
    });
    assert.equal(
      closed.result.structuredContent.errorCode,
      'browser/action-committed-but-post-sync-failed',
    );
    assert.equal(closed.result.structuredContent.actionCommitState, 'committed');
    assert.equal(closed.result.structuredContent.retryable, false);
    assert.equal(
      (host.state.operationCalls.get('close_tab') || 0) - beforeClose,
      1,
      'post-commit sync failure must not redispatch the logical close',
    );
    assert.deepEqual(
      JSON.parse(readFileSync(fixture.workspacePath, 'utf8')).tabs.map((tab) => tab.token),
      ['aaaaaaaaaaaaaaaa'],
      'the close was committed before the synthetic list failure',
    );
  } finally {
    host.close();
    await stopProcess(wrapper.child);
    await new Promise((resolve) => fixture.cdp.close(resolve));
    rmSync(fixture.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('Windows proxy: native tab-close uncertainty is structured and non-retryable', {
  skip: process.platform !== 'win32',
}, async () => {
  const fixture = await makeFixture();
  const host = createFakeHost(fixture);
  const wrapper = driveWrapper(fixture);

  try {
    await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'windows-close-native-unknown-test', version: '0' },
    });
    wrapper.notify('notifications/initialized');
    await wrapper.request('tools/call', { name: 'list_pages', arguments: {} });
    const workspace = JSON.parse(readFileSync(fixture.workspacePath, 'utf8'));
    atomicWriteJson(fixture.workspacePath, {
      ...workspace,
      revision: workspace.revision + 1,
      tabs: [
        ...workspace.tabs,
        { token: 'aaaaaaaaaaaaaaaa', target_id: 'target-a-22' },
      ],
    });
    await wrapper.request('tools/call', { name: 'list_pages', arguments: {} });
    host.state.operationErrors.set(
      'close_tab',
      'browser/action-commit-unknown-after-tab-close: native close acknowledgement lost',
    );

    const beforeClose = host.state.operationCalls.get('close_tab') || 0;
    const closed = await wrapper.request('tools/call', {
      name: 'close_page',
      arguments: { pageId: 1 },
    });
    assert.equal(
      closed.result.structuredContent.errorCode,
      'browser/action-commit-unknown-after-tab-close',
    );
    assert.equal(closed.result.structuredContent.actionCommitState, 'unknown');
    assert.equal(closed.result.structuredContent.retryable, false);
    assert.equal((host.state.operationCalls.get('close_tab') || 0) - beforeClose, 1);
  } finally {
    host.close();
    await stopProcess(wrapper.child);
    await new Promise((resolve) => fixture.cdp.close(resolve));
    rmSync(fixture.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('Windows proxy: settled mutation errors are commit-unknown and never redispatched', {
  skip: process.platform !== 'win32',
}, async () => {
  const fixture = await makeFixture();
  const host = createFakeHost(fixture);
  const wrapper = driveWrapper(fixture);

  try {
    await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'windows-upstream-mutation-error-test', version: '0' },
    });
    wrapper.notify('notifications/initialized');
    await wrapper.request('tools/call', { name: 'list_pages', arguments: {} });

    const assertMutationUnknown = async ({ name, arguments: args, mode, upstreamError }) => {
      const beforeDispatches = readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === name).length;
      writeFileSync(fixture.modePath, JSON.stringify(mode));
      const response = await wrapper.request('tools/call', { name, arguments: args });
      assert.equal(response.error, undefined);
      assert.equal(response.result.isError, true);
      assert.equal(
        response.result.structuredContent.errorCode,
        'browser/action-commit-unknown-after-upstream-error',
      );
      assert.equal(response.result.structuredContent.outcome, 'unknown');
      assert.equal(response.result.structuredContent.actionCommitState, 'unknown');
      assert.equal(response.result.structuredContent.actionMayHaveCommitted, true);
      assert.equal(response.result.structuredContent.retryable, false);
      assert.equal(response.result.structuredContent.upstreamError, upstreamError);
      assert.equal(
        readAudit(fixture.auditPath)
          .filter((entry) => entry.kind === 'tool' && entry.name === name).length -
          beforeDispatches,
        1,
        `${name} must execute exactly once when its acknowledgement is an error`,
      );
    };

    const beforeLifecycle = {
      begin: host.state.operationCalls.get('begin_agent_operation') || 0,
      end: host.state.operationCalls.get('end_agent_operation') || 0,
    };
    await assertMutationUnknown({
      name: 'click',
      arguments: { uid: 'mutation-json-error' },
      mode: { clickError: 'click committed before JSON-RPC error' },
      upstreamError: 'click committed before JSON-RPC error',
    });
    await assertMutationUnknown({
      name: 'type_text',
      arguments: { text: 'committed text' },
      mode: { typeTextToolError: 'type committed before tool error' },
      upstreamError: 'type committed before tool error',
    });
    await assertMutationUnknown({
      name: 'navigate_page',
      arguments: { type: 'url', url: 'https://mutation-error.example/' },
      mode: { navigateError: 'navigation committed before JSON-RPC error' },
      upstreamError: 'navigation committed before JSON-RPC error',
    });
    await assertMutationUnknown({
      name: 'evaluate_script',
      arguments: { function: '() => { globalThis.__committed = true; }' },
      mode: { evaluateToolError: 'script committed before tool error' },
      upstreamError: 'script committed before tool error',
    });

    // A tool classified as read-only keeps the official MCP result verbatim;
    // only potentially mutating dispatches need commit-unknown conversion.
    const beforeSnapshots = readAudit(fixture.auditPath)
      .filter((entry) => entry.kind === 'tool' && entry.name === 'take_snapshot').length;
    writeFileSync(fixture.modePath, JSON.stringify({
      snapshotToolError: 'snapshot read failed',
    }));
    const snapshot = await wrapper.request('tools/call', {
      name: 'take_snapshot',
      arguments: {},
    });
    assert.equal(snapshot.result.isError, true);
    assert.equal(snapshot.result.content[0].text, 'snapshot read failed');
    assert.equal(snapshot.result.structuredContent, undefined);
    assert.equal(
      readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'take_snapshot').length -
        beforeSnapshots,
      1,
    );
    assert.equal(
      (host.state.operationCalls.get('begin_agent_operation') || 0) - beforeLifecycle.begin,
      5,
    );
    assert.equal(
      (host.state.operationCalls.get('end_agent_operation') || 0) - beforeLifecycle.end,
      5,
    );
  } finally {
    host.close();
    await stopProcess(wrapper.child);
    await new Promise((resolve) => fixture.cdp.close(resolve));
    rmSync(fixture.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('Windows proxy: host tab-navigation uncertainty is never compensated into a retryable create', {
  skip: process.platform !== 'win32',
}, async () => {
  const fixture = await makeFixture();
  const host = createFakeHost(fixture);
  const wrapper = driveWrapper(fixture);

  try {
    await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'windows-create-navigation-unknown-test', version: '0' },
    });
    wrapper.notify('notifications/initialized');
    host.state.operationErrors.set(
      'create_tab',
      'browser/action-commit-unknown-after-tab-navigation: final CAS acknowledgement lost',
    );

    const created = await wrapper.request('tools/call', {
      name: 'new_page',
      arguments: { url: 'https://create.example/' },
    });
    assert.equal(
      created.result.structuredContent.errorCode,
      'browser/action-commit-unknown-after-tab-navigation',
    );
    assert.equal(created.result.structuredContent.actionCommitState, 'unknown');
    assert.equal(created.result.structuredContent.retryable, false);
    assert.equal(host.state.operationCalls.get('create_tab'), 1);
    assert.equal(
      host.state.operationCalls.get('rollback_created_tab') || 0,
      0,
      'network/script effects cannot be proven undone by closing a staging tab',
    );
  } finally {
    host.close();
    await stopProcess(wrapper.child);
    await new Promise((resolve) => fixture.cdp.close(resolve));
    rmSync(fixture.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('Windows proxy: bootstrap navigation remains committed when its post-sync fails', {
  skip: process.platform !== 'win32',
}, async () => {
  const fixture = await makeFixture();
  const host = createFakeHost(fixture);
  const wrapper = driveWrapper(fixture);

  try {
    writeFileSync(fixture.modePath, JSON.stringify({
      bootstrapBlank: true,
      listPagesErrorAfterNavigate: true,
    }));
    await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'windows-bootstrap-navigation-commit-test', version: '0' },
    });
    wrapper.notify('notifications/initialized');

    const created = await wrapper.request('tools/call', {
      name: 'new_page',
      arguments: { url: 'https://reused.example/' },
    });
    assert.equal(
      created.result.structuredContent.errorCode,
      'browser/action-committed-but-post-sync-failed',
    );
    assert.equal(created.result.structuredContent.actionCommitState, 'committed');
    assert.equal(created.result.structuredContent.retryable, false);
    assert.equal(host.state.operationCalls.get('create_tab') || 0, 0);
    assert.equal(
      readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'navigate_page').length,
      1,
      'the bootstrap navigation must be dispatched exactly once',
    );
  } finally {
    host.close();
    await stopProcess(wrapper.child);
    await new Promise((resolve) => fixture.cdp.close(resolve));
    rmSync(fixture.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('Windows proxy: external close cancellation reaches the host tombstone before commit', {
  skip: process.platform !== 'win32',
}, async () => {
  const fixture = await makeFixture();
  const host = createFakeHost(fixture);
  const wrapper = driveWrapper(fixture);

  try {
    await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'windows-close-cancel-test', version: '0' },
    });
    wrapper.notify('notifications/initialized');
    await wrapper.request('tools/call', { name: 'list_pages', arguments: {} });

    const workspace = JSON.parse(readFileSync(fixture.workspacePath, 'utf8'));
    atomicWriteJson(fixture.workspacePath, {
      ...workspace,
      revision: workspace.revision + 1,
      tabs: [
        ...workspace.tabs,
        { token: 'aaaaaaaaaaaaaaaa', target_id: 'target-a-22' },
      ],
    });
    await wrapper.request('tools/call', { name: 'list_pages', arguments: {} });

    host.state.deferredOperations.add('close_tab');
    const endBefore = host.state.operationCalls.get('end_agent_operation') || 0;
    const closing = wrapper.startRequest('tools/call', {
      name: 'close_page',
      arguments: { pageId: 1 },
    });
    const requestDirectory = join(fixture.root, 'host-requests');
    await waitUntil(() => readdirSync(requestDirectory).some((name) => {
      if (!name.endsWith('.json')) return false;
      try {
        return JSON.parse(readFileSync(join(requestDirectory, name), 'utf8')).operation ===
          'close_tab';
      } catch {
        return false;
      }
    }), 'close_tab host request was not published');

    wrapper.abandonRequest(closing.id);
    wrapper.notify('notifications/cancelled', {
      requestId: closing.id,
      reason: 'cancel close before native commit',
    });
    await waitUntil(
      () => readdirSync(requestDirectory).some((name) => name.endsWith('.cancelled')),
      'external cancellation did not publish a durable host tombstone',
    );
    await waitUntil(
      () => (host.state.operationCalls.get('end_agent_operation') || 0) === endBefore + 1,
      'cancelled close did not settle end_agent_operation',
    );
    assert.equal(host.state.operationCalls.get('close_tab') || 0, 0);
    assert.equal(
      JSON.parse(readFileSync(fixture.workspacePath, 'utf8')).tabs.length,
      2,
      'a host mutation cancelled before claim must not close a tab',
    );
  } finally {
    host.close();
    await stopProcess(wrapper.child);
    await new Promise((resolve) => fixture.cdp.close(resolve));
    rmSync(fixture.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('Windows proxy: external cancellation tombstones an in-flight tab activation', {
  skip: process.platform !== 'win32',
}, async () => {
  const fixture = await makeFixture();
  const host = createFakeHost(fixture);
  const wrapper = driveWrapper(fixture);

  try {
    await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'windows-activation-cancel-test', version: '0' },
    });
    wrapper.notify('notifications/initialized');
    await wrapper.request('tools/call', { name: 'list_pages', arguments: {} });

    host.state.deferredOperations.add('activate_tab');
    const operationBeginBefore = host.state.operationCalls.get('begin_agent_operation') || 0;
    const snapshot = wrapper.startRequest('tools/call', {
      name: 'take_snapshot',
      arguments: {},
    });
    const requestDirectory = join(fixture.root, 'host-requests');
    await waitUntil(() => readdirSync(requestDirectory).some((name) => {
      if (!name.endsWith('.json')) return false;
      try {
        return JSON.parse(readFileSync(join(requestDirectory, name), 'utf8')).operation ===
          'activate_tab';
      } catch {
        return false;
      }
    }), 'activate_tab host request was not published');

    wrapper.abandonRequest(snapshot.id);
    wrapper.notify('notifications/cancelled', {
      requestId: snapshot.id,
      reason: 'cancel activation before host acknowledgement',
    });
    await waitUntil(
      () => readdirSync(requestDirectory).some((name) => name.endsWith('.cancelled')),
      'external cancellation did not tombstone the in-flight activation',
    );
    assert.equal(
      host.state.operationCalls.get('begin_agent_operation') || 0,
      operationBeginBefore,
      'a cancelled activation must not advance into an executable operation',
    );
    assert.equal(
      readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'take_snapshot').length,
      0,
      'a cancelled activation must not dispatch the upstream page tool',
    );
  } finally {
    host.close();
    await stopProcess(wrapper.child);
    await new Promise((resolve) => fixture.cdp.close(resolve));
    rmSync(fixture.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('Windows proxy: stdin shutdown tombstones an in-flight close before exit', {
  skip: process.platform !== 'win32',
}, async () => {
  const fixture = await makeFixture();
  const host = createFakeHost(fixture);
  const wrapper = driveWrapper(fixture);

  try {
    await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'windows-close-stdin-shutdown-test', version: '0' },
    });
    wrapper.notify('notifications/initialized');
    await wrapper.request('tools/call', { name: 'list_pages', arguments: {} });
    const workspace = JSON.parse(readFileSync(fixture.workspacePath, 'utf8'));
    atomicWriteJson(fixture.workspacePath, {
      ...workspace,
      revision: workspace.revision + 1,
      tabs: [
        ...workspace.tabs,
        { token: 'aaaaaaaaaaaaaaaa', target_id: 'target-a-22' },
      ],
    });
    await wrapper.request('tools/call', { name: 'list_pages', arguments: {} });

    host.state.deferredOperations.add('close_tab');
    const endBefore = host.state.operationCalls.get('end_agent_operation') || 0;
    const closing = wrapper.startRequest('tools/call', {
      name: 'close_page',
      arguments: { pageId: 1 },
    });
    const requestDirectory = join(fixture.root, 'host-requests');
    await waitUntil(() => readdirSync(requestDirectory).some((name) => {
      if (!name.endsWith('.json')) return false;
      try {
        return JSON.parse(readFileSync(join(requestDirectory, name), 'utf8')).operation ===
          'close_tab';
      } catch {
        return false;
      }
    }), 'close_tab host request was not published before stdin shutdown');

    wrapper.abandonRequest(closing.id);
    wrapper.child.stdin.end();
    await waitUntil(
      () => readdirSync(requestDirectory).some((name) => name.endsWith('.cancelled')),
      'stdin shutdown did not publish a durable close tombstone',
    );
    await waitUntil(
      () => (host.state.operationCalls.get('end_agent_operation') || 0) === endBefore + 1,
      'stdin shutdown did not settle end_agent_operation',
    );
    if (wrapper.child.exitCode == null && wrapper.child.signalCode == null) {
      await Promise.race([
        new Promise((resolve) => wrapper.child.once('exit', resolve)),
        sleep(2_000).then(() => { throw new Error('Windows wrapper did not exit promptly'); }),
      ]);
    }
    assert.equal(host.state.operationCalls.get('close_tab') || 0, 0);
    assert.equal(JSON.parse(readFileSync(fixture.workspacePath, 'utf8')).tabs.length, 2);
  } finally {
    host.close();
    await stopProcess(wrapper.child);
    await new Promise((resolve) => fixture.cdp.close(resolve));
    rmSync(fixture.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('Windows proxy: same wrapper recovers A once after stop while B keeps shared CDP alive', {
  skip: process.platform !== 'win32',
}, async () => {
  const fixture = await makeFixture();
  const host = createFakeHost(fixture);
  const wrapper = driveWrapper(fixture);

  try {
    const initialized = await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'windows-lifecycle-test', version: '0' },
    });
    assert.equal(initialized.result.serverInfo.name, 'fake-windows-mcp');
    wrapper.notify('notifications/initialized');

    const first = await wrapper.request('tools/call', {
      name: 'list_pages',
      arguments: {},
    });
    assert.equal(first.result.structuredContent.pages[0].id, 1);
    assert.equal(host.state.prepareCalls, 1);
    assert.equal(readAudit(fixture.auditPath).filter((entry) => entry.kind === 'start').length, 1);

    // Closing A does not stop the app-owned CDP endpoint because B is still alive.
    host.stopSessionA();
    assert.equal(existsSync(fixture.otherWorkspacePath), true);
    assert.equal(fixture.cdp.listening, true);
    const recovered = await wrapper.request('tools/call', {
      name: 'list_pages',
      arguments: {},
    });
    assert.equal(recovered.result.structuredContent.pages[0].id, 2);
    assert.equal(host.state.prepareCalls, 2, 'A is prepared exactly once after its workspace disappears');
    assert.equal(readAudit(fixture.auditPath).filter((entry) => entry.kind === 'start').length, 1, 'the MCP child remains the same process');

    // A failed recovery performs one prepare and one sync attempt, never an unbounded loop.
    host.stopSessionA();
    host.state.preparePublishesWorkspace = false;
    const beforeFailedRecovery = host.state.prepareCalls;
    const failedRecovery = await wrapper.request('tools/call', {
      name: 'list_pages',
      arguments: {},
    });
    assert.match(failedRecovery.error.message, /^browser\/workspace-missing/);
    assert.equal(host.state.prepareCalls - beforeFailedRecovery, 1);
    await sleep(100);
    assert.equal(host.state.prepareCalls - beforeFailedRecovery, 1);

    // Failure keeps proxy routing enabled; the next distinct request may prepare once again.
    host.state.preparePublishesWorkspace = true;
    const recoveredAgain = await wrapper.request('tools/call', {
      name: 'list_pages',
      arguments: {},
    });
    assert.equal(recoveredAgain.result.structuredContent.pages[0].id, 3);

    // A >1.5s official-MCP input tool keeps one begin/end operation alive via
    // serial refreshes. The heartbeat stops before end and never becomes a
    // background lease-renewal loop after the tool response.
    writeFileSync(fixture.modePath, JSON.stringify({ clickDelayMs: 1_600 }));
    const longInputBefore = {
      begin: host.state.operationCalls.get('begin_agent_operation') || 0,
      refresh: host.state.operationCalls.get('refresh_agent_input') || 0,
      end: host.state.operationCalls.get('end_agent_operation') || 0,
      log: host.state.operationLog.length,
      clicks: readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'click').length,
    };
    const longInput = await wrapper.request('tools/call', {
      name: 'click',
      arguments: { uid: 'slow-button' },
    });
    assert.equal(longInput.result.content[0].text, 'clicked');
    assert.equal(
      (host.state.operationCalls.get('begin_agent_operation') || 0) - longInputBefore.begin,
      1,
    );
    assert.ok(
      (host.state.operationCalls.get('refresh_agent_input') || 0) - longInputBefore.refresh >= 4,
      'a 1.6s trusted-input tool should refresh the 750ms window several times',
    );
    assert.equal(
      (host.state.operationCalls.get('end_agent_operation') || 0) - longInputBefore.end,
      1,
    );
    assert.equal(
      readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'click').length -
        longInputBefore.clicks,
      1,
      'heartbeat must not redispatch the logical tool',
    );
    const longInputOperations = host.state.operationLog.slice(longInputBefore.log);
    assert.ok(
      longInputOperations.lastIndexOf('refresh_agent_input') <
        longInputOperations.lastIndexOf('end_agent_operation'),
      'the in-flight heartbeat must settle before end_agent_operation',
    );
    const refreshesAfterLongInput = host.state.operationCalls.get('refresh_agent_input') || 0;
    await sleep(600);
    assert.equal(
      host.state.operationCalls.get('refresh_agent_input') || 0,
      refreshesAfterLongInput,
      'no heartbeat may survive end_agent_operation',
    );

    // Slow non-input/DOM work uses the generic 5s operation heartbeat. It must
    // neither borrow the trusted-input suppression endpoint nor leave a timer
    // running after the host operation is ended.
    writeFileSync(fixture.modePath, JSON.stringify({ snapshotDelayMs: 5_500 }));
    const slowSnapshotBefore = {
      begin: host.state.operationCalls.get('begin_agent_operation') || 0,
      genericRefresh: host.state.operationCalls.get('refresh_agent_operation') || 0,
      inputRefresh: host.state.operationCalls.get('refresh_agent_input') || 0,
      end: host.state.operationCalls.get('end_agent_operation') || 0,
      log: host.state.operationLog.length,
    };
    const slowSnapshot = await wrapper.request('tools/call', {
      name: 'take_snapshot',
      arguments: {},
    });
    assert.equal(slowSnapshot.result.content[0].text, 'snapshot');
    assert.equal(
      (host.state.operationCalls.get('begin_agent_operation') || 0) - slowSnapshotBefore.begin,
      1,
    );
    assert.ok(
      (host.state.operationCalls.get('refresh_agent_operation') || 0) -
        slowSnapshotBefore.genericRefresh >= 1,
      'a slow non-input tool must refresh the generic operation lease',
    );
    assert.equal(
      host.state.operationCalls.get('refresh_agent_input') || 0,
      slowSnapshotBefore.inputRefresh,
      'a non-input tool must not extend trusted-input suppression',
    );
    assert.equal(
      (host.state.operationCalls.get('end_agent_operation') || 0) - slowSnapshotBefore.end,
      1,
    );
    const slowSnapshotOperations = host.state.operationLog.slice(slowSnapshotBefore.log);
    assert.ok(
      slowSnapshotOperations.lastIndexOf('refresh_agent_operation') <
        slowSnapshotOperations.lastIndexOf('end_agent_operation'),
      'the generic heartbeat must settle before end_agent_operation',
    );
    const genericRefreshesAfterSnapshot =
      host.state.operationCalls.get('refresh_agent_operation') || 0;
    await sleep(5_200);
    assert.equal(
      host.state.operationCalls.get('refresh_agent_operation') || 0,
      genericRefreshesAfterSnapshot,
      'no generic heartbeat may survive end_agent_operation',
    );
    writeFileSync(fixture.modePath, JSON.stringify({ clickDelayMs: 1_600 }));

    // A rejected refresh (the host atomically observed takeover/lease loss)
    // cooperatively cancels the already claimed upstream request and waits for
    // its authoritative result. This fake upstream deliberately ignores the
    // cancellation and commits, so the committed success must win; the logical
    // click is never invoked a second time.
    host.state.operationErrors.set(
      'refresh_agent_input',
      'browser/agent-input-refresh-rejected: user takeover',
    );
    const failedHeartbeatBefore = {
      begin: host.state.operationCalls.get('begin_agent_operation') || 0,
      refresh: host.state.operationCalls.get('refresh_agent_input') || 0,
      end: host.state.operationCalls.get('end_agent_operation') || 0,
      clicks: readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'click').length,
      cancellations: readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'cancel').length,
      completed: readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool-complete' && entry.name === 'click').length,
      timeline: host.state.operationTimeline.length,
    };
    const failedHeartbeatStartedAt = Date.now();
    const failedHeartbeat = await wrapper.request('tools/call', {
      name: 'click',
      arguments: { uid: 'revoked-button' },
    });
    assert.equal(failedHeartbeat.result.content[0].text, 'clicked');
    assert.ok(
      Date.now() - failedHeartbeatStartedAt >= 1_400,
      'heartbeat failure must wait for an upstream that ignores cancellation to really settle',
    );
    assert.equal(
      (host.state.operationCalls.get('begin_agent_operation') || 0) - failedHeartbeatBefore.begin,
      1,
    );
    assert.equal(
      (host.state.operationCalls.get('refresh_agent_input') || 0) - failedHeartbeatBefore.refresh,
      1,
    );
    assert.equal(
      (host.state.operationCalls.get('end_agent_operation') || 0) - failedHeartbeatBefore.end,
      1,
    );
    assert.equal(
      readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'click').length -
        failedHeartbeatBefore.clicks,
      1,
    );
    await waitUntil(
      () => readAudit(fixture.auditPath).filter((entry) => entry.kind === 'cancel').length ===
        failedHeartbeatBefore.cancellations + 1,
      'heartbeat failure did not cancel the claimed upstream request',
    );
    const failedAudit = readAudit(fixture.auditPath);
    const completion = failedAudit
      .filter((entry) => entry.kind === 'tool-complete' && entry.name === 'click')
      .at(-1);
    const end = host.state.operationTimeline
      .slice(failedHeartbeatBefore.timeline)
      .filter((entry) => entry.operation === 'end_agent_operation')
      .at(-1);
    assert.equal(
      failedAudit.filter((entry) => entry.kind === 'tool-complete' && entry.name === 'click').length -
        failedHeartbeatBefore.completed,
      1,
    );
    assert.ok(completion && end && end.at >= completion.at, 'end must occur after real upstream completion');
    const refreshesAfterFailure = host.state.operationCalls.get('refresh_agent_input') || 0;
    await sleep(600);
    assert.equal(host.state.operationCalls.get('refresh_agent_input') || 0, refreshesAfterFailure);
    host.state.operationErrors.delete('refresh_agent_input');
    writeFileSync(fixture.modePath, '{}');

    // An external MCP cancellation is also cooperative after begin. The
    // client abandons its response, but the wrapper must retain the internal
    // request until an upstream that ignores cancellation really completes;
    // only then may it end the Rust operation.
    writeFileSync(fixture.modePath, JSON.stringify({ clickDelayMs: 1_600 }));
    const externalCancelBefore = {
      clicks: readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'click').length,
      completed: readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool-complete' && entry.name === 'click').length,
      end: host.state.operationCalls.get('end_agent_operation') || 0,
      timeline: host.state.operationTimeline.length,
    };
    const cancelled = wrapper.startRequest('tools/call', {
      name: 'click',
      arguments: { uid: 'externally-cancelled-button' },
    });
    await waitUntil(
      () => readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'click').length ===
        externalCancelBefore.clicks + 1,
      'external cancellation fixture did not dispatch its one click',
    );
    wrapper.abandonRequest(cancelled.id);
    wrapper.notify('notifications/cancelled', {
      requestId: cancelled.id,
      reason: 'user cancelled the browser tool',
    });
    await waitUntil(
      () => readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool-complete' && entry.name === 'click').length ===
        externalCancelBefore.completed + 1 &&
        (host.state.operationCalls.get('end_agent_operation') || 0) === externalCancelBefore.end + 1,
      'cancelled upstream completion and host end did not both settle',
      5_000,
    );
    const externalCancelAudit = readAudit(fixture.auditPath);
    const externalCompletion = externalCancelAudit
      .filter((entry) => entry.kind === 'tool-complete' && entry.name === 'click')
      .at(-1);
    const externalEnd = host.state.operationTimeline
      .slice(externalCancelBefore.timeline)
      .filter((entry) => entry.operation === 'end_agent_operation')
      .at(-1);
    assert.ok(
      externalCompletion && externalEnd && externalCompletion.at <= externalEnd.at,
      'external cancellation must not end the host operation before upstream completion',
    );
    writeFileSync(fixture.modePath, '{}');

    // The upstream click has committed before host cleanup begins. A failed
    // end transport must therefore keep the successful result and only emit a
    // diagnostic; returning a normal tool error could make the Agent replay it.
    host.state.operationErrors.set('end_agent_operation', 'browser/end-cleanup-failed');
    const committedBefore = {
      prepare: host.state.prepareCalls,
      begin: host.state.operationCalls.get('begin_agent_operation') || 0,
      end: host.state.operationCalls.get('end_agent_operation') || 0,
      clicks: readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'click').length,
    };
    const committedDespiteEndFailure = await wrapper.request('tools/call', {
      name: 'click',
      arguments: { uid: 'committed-before-end-failure' },
    });
    assert.equal(committedDespiteEndFailure.result.content[0].text, 'clicked');
    assert.equal(host.state.prepareCalls, committedBefore.prepare);
    assert.equal(
      (host.state.operationCalls.get('begin_agent_operation') || 0) - committedBefore.begin,
      1,
    );
    assert.equal(
      (host.state.operationCalls.get('end_agent_operation') || 0) - committedBefore.end,
      1,
    );
    assert.equal(
      readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'click').length -
        committedBefore.clicks,
      1,
      'end cleanup failure must not redispatch the committed click',
    );
    assert.match(
      wrapper.stderrText(),
      /end_agent_operation cleanup failed after committed tool result: browser\/end-cleanup-failed/,
    );
    host.state.operationErrors.delete('end_agent_operation');

    const assertNoLifecycleRetry = async ({
      operation,
      error,
      configure,
      expectedClickDelta = 0,
      expectedMutationUnknown = false,
    }) => {
      const beforePrepare = host.state.prepareCalls;
      const beforeOperation = host.state.operationCalls.get(operation) || 0;
      const beforeClicks = readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'click').length;
      configure();
      const response = await wrapper.request('tools/call', {
        name: 'click',
        arguments: { uid: 'button-1' },
      });
      if (expectedMutationUnknown) {
        assert.equal(response.error, undefined);
        assert.equal(
          response.result.structuredContent.errorCode,
          'browser/action-commit-unknown-after-upstream-error',
        );
        assert.equal(response.result.structuredContent.actionCommitState, 'unknown');
        assert.equal(response.result.structuredContent.retryable, false);
        assert.equal(response.result.structuredContent.upstreamError, error);
      } else {
        assert.equal(response.error.message, error);
      }
      assert.equal(host.state.prepareCalls, beforePrepare, `${error} must not trigger prepare`);
      assert.equal((host.state.operationCalls.get(operation) || 0) - beforeOperation, 1);
      const afterClicks = readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'click').length;
      assert.equal(afterClicks - beforeClicks, expectedClickDelta);
    };

    await assertNoLifecycleRetry({
      operation: 'activate_tab',
      error: 'browser/user-takeover',
      configure: () => host.state.operationErrors.set('activate_tab', 'browser/user-takeover'),
    });
    host.state.operationErrors.delete('activate_tab');

    await assertNoLifecycleRetry({
      operation: 'begin_agent_operation',
      error: 'browser/native-surface-missing',
      configure: () => host.state.operationErrors.set('begin_agent_operation', 'browser/native-surface-missing'),
    });
    host.state.operationErrors.delete('begin_agent_operation');

    await assertNoLifecycleRetry({
      operation: 'begin_agent_operation',
      error: 'browser/control-lease-lost',
      configure: () => writeFileSync(fixture.modePath, JSON.stringify({ clickError: 'browser/control-lease-lost' })),
      expectedClickDelta: 1,
      expectedMutationUnknown: true,
    });
  } finally {
    host.close();
    await stopProcess(wrapper.child);
    await new Promise((resolve) => fixture.cdp.close(resolve));
    rmSync(fixture.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('Windows proxy: upstream crash settles the claimed dispatch before wrapper exit', {
  skip: process.platform !== 'win32',
}, async () => {
  const fixture = await makeFixture();
  const host = createFakeHost(fixture);
  const wrapper = driveWrapper(fixture);

  try {
    await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'windows-upstream-crash-test', version: '0' },
    });
    wrapper.notify('notifications/initialized');
    await wrapper.request('tools/call', { name: 'list_pages', arguments: {} });

    writeFileSync(fixture.modePath, JSON.stringify({ clickDelayMs: 5_000 }));
    const before = {
      begin: host.state.operationCalls.get('begin_agent_operation') || 0,
      end: host.state.operationCalls.get('end_agent_operation') || 0,
      clicks: readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'click').length,
    };
    const inFlight = wrapper.startRequest('tools/call', {
      name: 'click',
      arguments: { uid: 'crash-during-dispatch' },
    });
    await waitUntil(
      () => readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'click').length ===
        before.clicks + 1,
      'crash fixture did not dispatch its click',
    );
    const upstreamPid = readAudit(fixture.auditPath)
      .filter((entry) => entry.kind === 'start')
      .at(-1)?.pid;
    assert.ok(Number.isInteger(upstreamPid), 'fake upstream pid is missing');
    process.kill(upstreamPid, 'SIGKILL');

    const outcome = await inFlight.response;
    assert.equal(
      outcome.result.structuredContent.errorCode,
      'browser/action-commit-unknown-after-upstream-interruption',
    );
    assert.equal(outcome.result.structuredContent.retryable, false);
    await waitUntil(
      () => (host.state.operationCalls.get('end_agent_operation') || 0) === before.end + 1,
      'upstream crash did not end the claimed host operation',
      5_000,
    );
    assert.equal(
      (host.state.operationCalls.get('begin_agent_operation') || 0) - before.begin,
      1,
    );
    assert.equal(
      (host.state.operationCalls.get('end_agent_operation') || 0) - before.end,
      1,
    );
    if (wrapper.child.exitCode == null && wrapper.child.signalCode == null) {
      await Promise.race([
        new Promise((resolve) => wrapper.child.once('exit', resolve)),
        sleep(5_000).then(() => { throw new Error('wrapper did not exit after upstream crash'); }),
      ]);
    }
  } finally {
    host.close();
    await stopProcess(wrapper.child);
    await new Promise((resolve) => fixture.cdp.close(resolve));
    rmSync(fixture.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

test('Windows proxy: cancelled uncooperative dispatch shuts down on the first deadline and rejects queued work', {
  skip: process.platform !== 'win32',
}, async () => {
  const fixture = await makeFixture();
  const host = createFakeHost(fixture);
  const wrapper = driveWrapper(fixture);
  let replacement = null;

  try {
    await wrapper.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'windows-cancel-shutdown-test', version: '0' },
    });
    wrapper.notify('notifications/initialized');
    await wrapper.request('tools/call', { name: 'list_pages', arguments: {} });

    writeFileSync(fixture.modePath, JSON.stringify({ clickDelayMs: 30_000 }));
    const before = {
      end: host.state.operationCalls.get('end_agent_operation') || 0,
      clicks: readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'click').length,
    };
    const inFlight = wrapper.startRequest('tools/call', {
      name: 'click',
      arguments: { uid: 'cancel-shutdown-button' },
    });
    await waitUntil(
      () => readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'click').length ===
        before.clicks + 1,
      'cancel shutdown fixture did not dispatch its click',
    );
    const listsBeforeQueued = readAudit(fixture.auditPath)
      .filter((entry) => entry.kind === 'tool' && entry.name === 'list_pages').length;

    wrapper.abandonRequest(inFlight.id);
    const cancelledAt = Date.now();
    wrapper.notify('notifications/cancelled', {
      requestId: inFlight.id,
      reason: 'cancel uncooperative dispatch',
    });
    const queued = wrapper.startRequest('tools/call', {
      name: 'list_pages',
      arguments: {},
    });
    const queuedOutcome = await Promise.race([
      queued.response,
      sleep(8_000).then(() => { throw new Error('queued call was not rejected before shutdown deadline'); }),
    ]);
    assert.match(queuedOutcome.error?.message || '', /browser\/wrapper-shutting-down/);

    if (wrapper.child.exitCode == null && wrapper.child.signalCode == null) {
      await Promise.race([
        new Promise((resolve) => wrapper.child.once('exit', resolve)),
        sleep(1_000).then(() => { throw new Error('wrapper did not exit after rejecting queued work'); }),
      ]);
    }
    const shutdownElapsedMs = Date.now() - cancelledAt;
    assert.ok(
      shutdownElapsedMs >= 4_500 && shutdownElapsedMs < 7_000,
      `cooperative cancel shutdown exceeded its first deadline: ${shutdownElapsedMs}ms`,
    );
    assert.equal(
      (host.state.operationCalls.get('end_agent_operation') || 0) - before.end,
      1,
      'cancelled claimed dispatch must end its exact host operation once',
    );
    assert.equal(
      readAudit(fixture.auditPath)
        .filter((entry) => entry.kind === 'tool' && entry.name === 'list_pages').length,
      listsBeforeQueued,
      'queued work must not reach the stopped upstream child',
    );
    assert.ok(
      readAudit(fixture.auditPath).some((entry) =>
        entry.kind === 'cancel' && entry.reason === 'cancel uncooperative dispatch'
      ),
      'the upstream cancellation notification was not published',
    );

    writeFileSync(fixture.modePath, '{}');
    replacement = driveWrapper(fixture);
    await replacement.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'windows-cancel-replacement-test', version: '0' },
    });
    replacement.notify('notifications/initialized');
    const recovered = await replacement.request('tools/call', {
      name: 'list_pages',
      arguments: {},
    });
    assert.equal(recovered.result.structuredContent.pages.length, 1);
  } finally {
    host.close();
    await stopProcess(wrapper.child);
    if (replacement) await stopProcess(replacement.child);
    await new Promise((resolve) => fixture.cdp.close(resolve));
    rmSync(fixture.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});
