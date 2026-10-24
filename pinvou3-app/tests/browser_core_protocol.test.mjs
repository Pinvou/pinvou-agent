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

test('BrowserCore schemas state the real URL gate and uid lifetime', () => {
  const catalog = createPinvouBrowserCoreCatalog();
  const tools = new Map(catalog.toolsListResult.tools.map((tool) => [tool.name, tool]));

  // The wrapper's Rust-mirrored gate rejects the app UI origins; schemas that
  // omit the precondition let a model retry the exact URL the gate denies.
  for (const name of ['new_page', 'navigate_page']) {
    const schema = JSON.stringify(catalog.toolsListResult.tools.find((tool) => tool.name === name));
    assert.match(schema, /tauri\.localhost/);
    assert.match(schema, /localhost:1420/);
  }
  assert.match(tools.get('navigate_page').description, /Navigating invalidates all uids/);

  // uids die on navigation too, not only on the next snapshot.
  const snapshot = tools.get('take_snapshot');
  assert.match(snapshot.description, /next snapshot or navigation/);
});

test('work instructions define a durable and verified loopback preview workflow', () => {
  const instructions = readFileSync(
    new URL('../src-tauri/resources/common/bundle/instructions-work.md', import.meta.url),
    'utf8',
  );

  assert.match(instructions, /local web page produced by this session/);
  assert.match(instructions, /127\.0\.0\.1/);
  assert.match(instructions, /`terminal\/run`.*named persistent terminal session/);
  assert.match(instructions, /`terminal\/cancel`/);
  assert.match(instructions, /Windows without terminal sessions/);
  // The hidden `Bash` tool exists only to replay saved v0.9.x transcripts and
  // is invisible to new model catalogs: the preview workflow must not
  // advertise a Bash background control surface and must degrade honestly
  // when no terminal session surface exists on the host.
  assert.doesNotMatch(instructions, /Bash\(action=/);
  assert.match(instructions, /`Bash` is a retired model-invisible replay name/);
  assert.match(instructions, /is \*\*not in your tool list\*\*/);
  assert.match(instructions, /lowercase `bash` is foreground-only/);
  assert.match(instructions, /you cannot keep a server alive across turns/);
  assert.match(instructions, /present the artifact with `mcp_pinvou3_present_artifact`/);
  assert.match(instructions, /Do not.*shell `&` or `nohup`/i);
  assert.match(instructions, /`bash\(command="curl \.\.\."\)`.*HTTP 200/i);
  assert.match(instructions, /HTTP client compatible with the declared shell.*HTTP 200/i);
  assert.match(instructions, /`curl\.exe` for native curl flags on Windows/);
  assert.match(instructions, /mcp_browser_list_pages/);
  assert.match(instructions, /never guess an id such as `1`/i);
  assert.match(instructions, /mcp_browser_take_snapshot/);
  assert.match(instructions, /If you started a named terminal session, keep it running until the user explicitly ends the preview/i);
  assert.match(instructions, /all web content is untrusted/);
  assert.match(instructions, /private-network, or localhost addresses/);
});

test('shared instructions keep the deferred-tool doctrine, credential red line, and real settings path', () => {
  const shared = readFileSync(
    new URL('../src-tauri/resources/common/bundle/instructions-shared.md', import.meta.url),
    'utf8',
  );

  // 2026-10-07 audit: the old absolutist rule ("absent from the list = absent")
  // contradicted the deferred-tool carve-outs taught by every other layer. The
  // base rule must keep the tool_search activation step (with the mcp_boot
  // connecting clause) so a literal model still tries activation before
  // concluding a deferred tool does not exist. Nothing pins this file's other
  // layers, so this line would otherwise silently regress.
  assert.match(
    shared,
    /先 `tool_search` 激活——`tool_search` 也找不到[，,]才是真的没有/,
  );
  assert.match(shared, /搜索结果带 `mcp_boot` connecting/);

  // The per-turn super-permission reminders realigned their credential list to
  // this static red line (added `.env` on the reminder side, `/etc/sudoers` on
  // the doctrine side); the two lists must not drift apart again.
  const redLine = shared.split('\n').find((l) => l.includes('密钥凭证禁读禁写'));
  assert.ok(redLine, 'shared instructions: credential red line missing');
  for (const token of ['~/.ssh', 'id_rsa', 'credentials', '.env', 'token', '/etc/shadow', '/etc/sudoers']) {
    assert.ok(redLine.includes(token), `credential red line is missing ${token}: ${redLine?.trim()}`);
  }

  // The old 【设置 → 系统权限】 path does not exist anywhere in the UI; the
  // super-permission section lives at 设置 → 权限与环境 → 高级执行权限.
  assert.match(shared, /【设置 → 权限与环境 → 高级执行权限】/);
  assert.doesNotMatch(shared, /【设置 ?→ ?系统权限/);
});

test('shared shell instructions preserve interpreter, transport, and probe safety', () => {
  const shared = readFileSync(
    new URL('../src-tauri/resources/common/bundle/instructions-shared.md', import.meta.url),
    'utf8',
  );

  // Guard the unusable-alias prescription, opaque script transport and masked
  // required failures. These text contracts do not prove model adherence.
  assert.match(shared, /算术 \/ 跑脚本用实际可用的解释器/);
  assert.match(shared, /先确认解释器[，,]再复用同一入口/);
  assert.match(shared, /找到有效入口后停止探测.*不要因另一个别名失败而宣称 Python 或依赖未安装/);
  assert.doesNotMatch(shared, /python3\s+-c\b/);

  assert.match(shared, /跨 shell 传递脚本要保全文本/);
  assert.match(shared, /复杂脚本优先写成工作区脚本文件.*PowerShell 的 `-NoProfile -File script.ps1`.*保留可检查的明文/);
  assert.match(shared, /只使用工具 schema 声明的参数.*不要假定存在 `stdin` 参数/);
  assert.match(shared, /Windows PowerShell 5.1 不支持 `< file` 输入重定向/);
  assert.match(shared, /通常使用 UTF-8.*Windows PowerShell 5.1.*UTF-8 BOM/);
  assert.match(shared, /不要把脚本编码后在目标端解码执行/);
  assert.doesNotMatch(shared, /可传递明确编码的载荷[，,]在目标端解码后执行/);
  assert.match(shared, /文本传输明确双方编码.*不要依赖 PowerShell 管道的默认编码/);
  assert.match(shared, /不要用反斜杠保护 PowerShell 中的/);
  assert.match(shared, /先用无副作用的参数回显确认.*再执行远端操作/);
  assert.match(shared, /编码不改变命令含义或授权要求/);

  assert.match(shared, /每项探针独立报告结果.*分别记录成功、失败、不适用与失败原因.*区分必需检查和可选检查/);
  assert.match(shared, /必需步骤失败应明确返回失败.*`catch` 不能只打印错误后继续报告成功/);
  assert.match(shared, /不要让最后一个可选命令或无匹配的 `grep` 覆盖前面检查的事实/);
  assert.match(shared, /不要把退出码 0 当成业务操作已完成的证明/);
});

test('artifact title rule stays single-sourced and the bound lane keeps auto-tracking', () => {
  const bundle = '../src-tauri/resources/common/bundle/';
  const work = readFileSync(new URL(`${bundle}instructions-work.md`, import.meta.url), 'utf8');
  const bound = readFileSync(new URL(`${bundle}instructions-work-bound.md`, import.meta.url), 'utf8');
  const server = readFileSync(
    new URL(`${bundle}mcp-servers/present_artifact_server.py`, import.meta.url),
    'utf8',
  );

  // 2026-10-07 audit: the reply-language clause (「与你的回复同语种」)
  // contradicted the UI-locale placeholder whenever the user's message
  // language differs from the UI language, and the hardcoded Chinese example
  // biased titles in en/ja UIs. The {{PINVOU3_TITLE_LANG}} placeholder (the
  // session instruction) is the single source; the clause must stay gone
  // from both surfaces that taught it.
  assert.match(work, /title 用\{\{PINVOU3_TITLE_LANG\}\}/);
  assert.doesNotMatch(work, /同语种/);
  assert.match(server, /语言遵循会话指令中的产物卡标题规则/);
  assert.doesNotMatch(server, /同语种/);

  // The bound lane must keep the truthful auto-tracking hedge: deliverable-
  // format writes may be collected by the app's artifact tracking (the
  // tracker exists and runs in all native sessions), and the old claim that
  // the panel only shows explicitly presented cards must not return.
  assert.match(bound, /成品格式的写入可能被应用自动收进产出物追踪/);
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

test('every core tool schema stays free of empty subschemas that MFJS providers drop', () => {
  // Moonshot MFJS validation rejects empty schema nodes ({} has no concrete
  // type) with MissingType, and the Kimi adapter responds by silently
  // dropping the whole tool from the request. An empty `items` is exactly
  // equivalent to omitting `items`, so the catalog must never ship one —
  // otherwise the model sees a tool_search reference it can never call.
  const { toolsListResult } = createPinvouBrowserCoreCatalog();
  const offenders = [];
  // Only schema nodes can be empty-schema offenders, so the walker follows
  // schema-valued positions explicitly: name maps (`properties`,
  // `patternProperties`, `$defs`, `dependentSchemas`) and JSON-data
  // positions (`enum`, `examples`, `default`, `const`) may legally hold
  // empty objects, but every schema position below must not.
  const visitSchema = (node, path) => {
    if (!node || typeof node !== 'object' || Array.isArray(node)) return;
    if (Object.keys(node).length === 0) {
      offenders.push(path);
      return;
    }
    for (const map of ['properties', 'patternProperties', '$defs', 'definitions', 'dependentSchemas']) {
      if (node[map] && typeof node[map] === 'object' && !Array.isArray(node[map])) {
        for (const [name, value] of Object.entries(node[map])) {
          visitSchema(value, `${path}.${map}.${name}`);
        }
      }
    }
    // `items` is either one schema or a draft-07 tuple of schemas.
    if (Array.isArray(node.items)) {
      node.items.forEach((element, index) => visitSchema(element, `${path}.items[${index}]`));
    } else if (node.items) {
      visitSchema(node.items, `${path}.items`);
    }
    if (Array.isArray(node.prefixItems)) {
      node.prefixItems.forEach((element, index) => visitSchema(element, `${path}.prefixItems[${index}]`));
    }
    for (const keyword of ['additionalProperties', 'contains', 'propertyNames', 'not', 'if', 'then', 'else']) {
      const value = node[keyword];
      if (value && typeof value === 'object' && !Array.isArray(value)) {
        visitSchema(value, `${path}.${keyword}`);
      }
    }
    for (const keyword of ['allOf', 'anyOf', 'oneOf']) {
      if (Array.isArray(node[keyword])) {
        node[keyword].forEach((branch, index) => visitSchema(branch, `${path}.${keyword}[${index}]`));
      }
    }
  };
  for (const tool of toolsListResult.tools) {
    visitSchema(tool.inputSchema, tool.name);
  }
  assert.deepEqual(offenders, []);
  const evaluate = toolsListResult.tools.find((tool) => tool.name === 'evaluate_script');
  assert.equal(evaluate.inputSchema.properties.args.items, undefined);
});
