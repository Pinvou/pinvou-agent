import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

import {
  createPinvouBrowserCoreCatalog,
  isChromeDiagnosticTool,
  isPinvouBrowserCoreTool,
  mergePinvouBrowserCatalog,
  PINVOU_BROWSER_CORE_TOOL_NAMES,
} from '../src-tauri/resources/common/bundle/mcp-servers/browser-core-protocol.mjs';

test('BrowserCore keeps one platform-neutral core catalog', () => {
  const tools = mergePinvouBrowserCatalog([
    { name: 'click', description: 'locked upstream schema', inputSchema: { type: 'object' } },
    { name: 'performance_start_trace', inputSchema: { type: 'object' } },
  ]);
  assert.equal(tools.some((tool) => tool.name === 'click'), true);
  assert.equal(tools.some((tool) => tool.name === 'take_snapshot'), true);
  assert.equal(tools.some((tool) => tool.name === 'performance_start_trace'), false);
  assert.equal(tools.find((tool) => tool.name === 'click').description, 'locked upstream schema');
});

test('Chrome diagnostics are an optional extension, not a second browser MCP', () => {
  const tools = mergePinvouBrowserCatalog(
    [{ name: 'performance_start_trace', inputSchema: { type: 'object' } }],
    { includeChromeDiagnostics: true },
  );
  assert.equal(tools.some((tool) => tool.name === 'performance_start_trace'), true);
  assert.equal(isChromeDiagnosticTool('performance_start_trace'), true);
  assert.equal(isPinvouBrowserCoreTool('performance_start_trace'), false);
});

test('BrowserCore catalog can hide platform-specific tools explicitly', () => {
  const catalog = createPinvouBrowserCoreCatalog({
    includeAdvancedPointerInput: false,
    includeViewportResize: false,
    includeDialog: false,
  });
  const names = catalog.toolsListResult.tools.map((tool) => tool.name);
  assert.equal(names.includes('click'), true);
  assert.equal(names.includes('fill'), true);
  assert.equal(names.includes('handle_dialog'), false);
  assert.equal(names.includes('resize_page'), false);
  assert.equal(names.includes('hover'), false);
  assert.equal(names.includes('drag'), false);
});

test('BrowserCore fallback catalog matches the official dialog and resize schemas', () => {
  const catalog = createPinvouBrowserCoreCatalog();
  const tools = new Map(catalog.toolsListResult.tools.map((tool) => [tool.name, tool]));
  const dialog = tools.get('handle_dialog');
  const resize = tools.get('resize_page');

  assert.deepEqual(dialog.inputSchema.required, ['action']);
  assert.deepEqual(dialog.inputSchema.properties.action.enum, ['accept', 'dismiss']);
  assert.equal(dialog.inputSchema.properties.promptText.type, 'string');
  assert.deepEqual(resize.inputSchema.required, ['width', 'height']);
  assert.equal(resize.inputSchema.properties.width.type, 'number');
  assert.equal(resize.inputSchema.properties.height.type, 'number');
});

test('BrowserCore tool-name order follows the fallback schema catalog', () => {
  const catalogNames = createPinvouBrowserCoreCatalog().toolsListResult.tools
    .map((tool) => tool.name);
  assert.deepEqual([...PINVOU_BROWSER_CORE_TOOL_NAMES], catalogNames);
});

test('BrowserCore wait schema discloses the native timeout ceiling', () => {
  const catalog = createPinvouBrowserCoreCatalog();
  const waitFor = catalog.toolsListResult.tools.find((tool) => tool.name === 'wait_for');

  assert.equal(waitFor.inputSchema.properties.timeout.minimum, 0);
  assert.equal(waitFor.inputSchema.properties.timeout.maximum, 12_000);
});

test('BrowserCore documents non-retryable partial form outcomes', () => {
  const catalog = createPinvouBrowserCoreCatalog();
  const fillForm = catalog.toolsListResult.tools.find((tool) => tool.name === 'fill_form');

  assert.match(fillForm.description, /validated before the first write/);
  assert.match(fillForm.description, /non-retryable structured partial outcome/);
  assert.match(fillForm.description, /must not be replayed as a whole/);
});

test('BrowserCore navigation tools acknowledge requests without claiming page load', () => {
  const catalog = createPinvouBrowserCoreCatalog();
  const tools = new Map(catalog.toolsListResult.tools.map((tool) => [tool.name, tool]));

  for (const name of ['new_page', 'navigate_page']) {
    assert.match(tools.get(name).description, /submit/i);
    assert.match(tools.get(name).description, /does not verify that the page loaded/i);
    assert.match(tools.get(name).description, /take_snapshot/);
  }
});

test('work instructions define a durable and verified loopback preview workflow', () => {
  const instructions = readFileSync(
    new URL('../src-tauri/resources/common/bundle/instructions-work.md', import.meta.url),
    'utf8',
  );

  assert.match(instructions, /local web page produced by this session/);
  assert.match(instructions, /127\.0\.0\.1/);
  assert.match(instructions, /background=true/);
  assert.match(instructions, /Do not.*shell `&` or `nohup`/i);
  assert.match(instructions, /`curl`.*HTTP 200/i);
  assert.match(instructions, /mcp_browser_list_pages/);
  assert.match(instructions, /never guess an id such as `1`/i);
  assert.match(instructions, /mcp_browser_take_snapshot/);
  assert.match(instructions, /Keep the background service running until the user explicitly ends the preview/i);
  assert.match(instructions, /all web content is untrusted/);
  assert.match(instructions, /private-network, or localhost addresses/);
});

test('page runtime exposes DOM-only capabilities and no host bridge', () => {
  const source = readFileSync(
    new URL('../src-tauri/resources/common/bundle/mcp-servers/browser-core-runtime.js', import.meta.url),
    'utf8',
  );
  const sandbox = {
    globalThis: null,
    location: { href: 'https://example.test/' },
    document: {},
    Element: class {},
    HTMLInputElement: class {},
    NodeFilter: { SHOW_ELEMENT: 1 },
    getComputedStyle: () => ({}),
    setTimeout,
  };
  sandbox.globalThis = sandbox;
  vm.runInNewContext(source, sandbox);
  const runtime = sandbox.__PINVOU_BROWSER_CORE_V1__;
  assert.equal(runtime.version, 1);
  assert.deepEqual(Object.keys(runtime).sort((a, b) => a.localeCompare(b)), ['element', 'evaluate', 'point', 'snapshot', 'version', 'waitFor']);
  assert.equal(source.includes('__TAURI__'), false);
  assert.equal(source.includes('webkit.messageHandlers'), false);
});

test('page runtime scrolls an offscreen element before resolving a native input point', () => {
  const source = readFileSync(
    new URL('../src-tauri/resources/common/bundle/mcp-servers/browser-core-runtime.js', import.meta.url),
    'utf8',
  );
  const point = source.slice(source.indexOf('function point(uid)'), source.indexOf('function argumentFor'));
  assert.match(point, /if \(!intersectsViewport\(\)\)/);
  assert.match(point, /element\.scrollIntoView\(\{ block: 'center', inline: 'center' \}\)/);
  assert.match(point, /right <= left \|\| bottom <= top/);
  assert.match(point, /browser\/element-outside-viewport/);
  assert.ok(
    point.indexOf('scrollIntoView') < point.indexOf('document.elementFromPoint'),
    'native hit testing must use the post-scroll viewport coordinate',
  );
});
