import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

import {
  createPinvouBrowserCoreCatalog,
  isChromeDiagnosticTool,
  isPinvouBrowserCoreTool,
  isPinvouBrowserInputTool,
  mergePinvouBrowserCatalog,
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

test('trusted-input classification belongs to BrowserCore', () => {
  assert.equal(isPinvouBrowserCoreTool('click'), true);
  assert.equal(isPinvouBrowserInputTool('click'), true);
  assert.equal(isPinvouBrowserInputTool('handle_dialog'), true);
  assert.equal(isPinvouBrowserInputTool('take_snapshot'), false);
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

test('BrowserCore documents non-retryable partial form outcomes', () => {
  const catalog = createPinvouBrowserCoreCatalog();
  const fillForm = catalog.toolsListResult.tools.find((tool) => tool.name === 'fill_form');

  assert.match(fillForm.description, /validated before the first write/);
  assert.match(fillForm.description, /non-retryable structured partial outcome/);
  assert.match(fillForm.description, /must not be replayed as a whole/);
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
  assert.deepEqual(Object.keys(runtime).sort(), ['element', 'evaluate', 'point', 'snapshot', 'version', 'waitFor']);
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
