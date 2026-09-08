/**
 * Stable, platform-neutral Pinvou BrowserCore contract.
 *
 * Agent-facing tool names do not reveal whether the current task is backed by
 * WebView2/CDP, WKWebView or WebKitGTK. Chrome-only diagnostics remain optional
 * extensions and never replace the common core catalog.
 */

export const PINVOU_BROWSER_CHROME_DIAGNOSTIC_TOOL_NAMES = new Set([
  'emulate',
  'get_console_message',
  'get_network_request',
  'lighthouse_audit',
  'list_console_messages',
  'list_network_requests',
  'performance_analyze_insight',
  'performance_start_trace',
  'performance_stop_trace',
  'take_heapsnapshot',
]);

const FALLBACK_SCHEMAS = {
  list_pages: {
    type: 'object',
    properties: {},
    additionalProperties: true,
  },
  new_page: {
    type: 'object',
    properties: {
      url: { type: 'string', description: 'URL whose navigation should be requested in a new page.' },
      background: { type: 'boolean' },
    },
    required: ['url'],
    additionalProperties: true,
  },
  select_page: {
    type: 'object',
    properties: { pageId: { type: 'number' }, bringToFront: { type: 'boolean' } },
    required: ['pageId'],
    additionalProperties: true,
  },
  close_page: {
    type: 'object',
    properties: { pageId: { type: 'number' } },
    required: ['pageId'],
    additionalProperties: true,
  },
  navigate_page: {
    type: 'object',
    properties: {
      type: { type: 'string', enum: ['url', 'back', 'forward', 'reload'] },
      url: { type: 'string' },
    },
    additionalProperties: true,
  },
  take_snapshot: {
    type: 'object',
    properties: { verbose: { type: 'boolean' } },
    additionalProperties: false,
  },
  click: {
    type: 'object',
    properties: {
      uid: { type: 'string' },
      dblClick: { type: 'boolean' },
      includeSnapshot: { type: 'boolean' },
    },
    required: ['uid'],
    additionalProperties: true,
  },
  drag: {
    type: 'object',
    properties: {
      from_uid: { type: 'string' },
      to_uid: { type: 'string' },
      includeSnapshot: { type: 'boolean' },
    },
    required: ['from_uid', 'to_uid'],
    additionalProperties: true,
  },
  fill: {
    type: 'object',
    properties: {
      uid: { type: 'string' },
      value: { type: 'string' },
      includeSnapshot: { type: 'boolean' },
    },
    required: ['uid', 'value'],
    additionalProperties: true,
  },
  fill_form: {
    type: 'object',
    properties: {
      elements: {
        type: 'array',
        items: {
          type: 'object',
          properties: { uid: { type: 'string' }, value: { type: 'string' } },
          required: ['uid', 'value'],
          additionalProperties: false,
        },
      },
      includeSnapshot: { type: 'boolean' },
    },
    required: ['elements'],
    additionalProperties: true,
  },
  type_text: {
    type: 'object',
    properties: { text: { type: 'string' }, submitKey: { type: 'string' } },
    required: ['text'],
    additionalProperties: true,
  },
  press_key: {
    type: 'object',
    properties: { key: { type: 'string' }, includeSnapshot: { type: 'boolean' } },
    required: ['key'],
    additionalProperties: true,
  },
  hover: {
    type: 'object',
    properties: { uid: { type: 'string' }, includeSnapshot: { type: 'boolean' } },
    required: ['uid'],
    additionalProperties: true,
  },
  wait_for: {
    type: 'object',
    properties: {
      text: { type: 'array', items: { type: 'string' }, minItems: 1 },
      timeout: {
        type: 'integer',
        minimum: 0,
        maximum: 12_000,
        description: 'Maximum wait in milliseconds (up to 12000).',
      },
    },
    required: ['text'],
    additionalProperties: true,
  },
  evaluate_script: {
    type: 'object',
    properties: {
      function: { type: 'string' },
      args: { type: 'array', items: {} },
    },
    required: ['function'],
    additionalProperties: true,
  },
  resize_page: {
    type: 'object',
    properties: {
      width: { type: 'number', description: 'Page width.' },
      height: { type: 'number', description: 'Page height.' },
    },
    required: ['width', 'height'],
    additionalProperties: true,
  },
  handle_dialog: {
    type: 'object',
    properties: {
      action: {
        type: 'string',
        enum: ['accept', 'dismiss'],
        description: 'Whether to dismiss or accept the dialog.',
      },
      promptText: {
        type: 'string',
        description: 'Optional prompt text to enter into the dialog.',
      },
    },
    required: ['action'],
    additionalProperties: true,
  },
};

export const PINVOU_BROWSER_CORE_TOOL_NAMES = new Set(Object.keys(FALLBACK_SCHEMAS));

const TOOL_DESCRIPTIONS = {
  list_pages: 'Get a list of pages open in the current task browser.',
  new_page: 'Open a new task-owned browser tab and submit a URL navigation request. Success does not verify that the page loaded; use take_snapshot to verify it.',
  select_page: 'Select a task-owned page for future browser calls. The selected page becomes the user-visible active tab in the task browser; only select when cross-page work is needed.',
  close_page: 'Close a task-owned page. The last page cannot be closed.',
  navigate_page: 'Submit URL, history, or reload navigation for the selected page. Success does not verify that the page loaded; use take_snapshot to verify it.',
  take_snapshot: 'Take a text snapshot of the selected page with element uids valid until the next snapshot (any new snapshot — including ones returned by tools with includeSnapshot — invalidates previous uids).',
  click: 'Click an element using task-local native input.',
  drag: 'Drag one element onto another using task-local native input.',
  fill: 'Fill an input using task-local native input.',
  fill_form: 'Fill multiple form fields using task-local native input. The whole batch is validated before the first write; an interrupted batch returns a non-retryable structured partial outcome and must not be replayed as a whole.',
  type_text: 'Type text into the currently focused element.',
  press_key: 'Press a key or key combination.',
  hover: 'Hover over an element.',
  wait_for: 'Wait for any specified text to appear.',
  evaluate_script: 'Evaluate a JavaScript function in the selected page.',
  resize_page: "Resize the selected page's content viewport.",
  handle_dialog: 'Accept or dismiss an open browser dialog.',
};

export function createPinvouBrowserCoreCatalog({
  includeAdvancedPointerInput = true,
  includeViewportResize = true,
  includeDialog = true,
} = {}) {
  const tools = mergePinvouBrowserCatalog([], { synthesizeMissingCore: true })
    .filter((tool) => includeAdvancedPointerInput || !['drag', 'hover'].includes(tool.name))
    .filter((tool) => includeViewportResize || tool.name !== 'resize_page')
    .filter((tool) => includeDialog || tool.name !== 'handle_dialog');
  return {
    initializeResult: {
      protocolVersion: '2024-11-05',
      capabilities: { tools: {} },
      serverInfo: { name: 'pinvou-browser-core', version: '1' },
    },
    toolsListResult: {
      tools: tools.map((tool) => ({
        ...tool,
        description: TOOL_DESCRIPTIONS[tool.name] || tool.description,
      })),
    },
  };
}

export function isPinvouBrowserCoreTool(name) {
  return PINVOU_BROWSER_CORE_TOOL_NAMES.has(name);
}

export function isChromeDiagnosticTool(name) {
  return PINVOU_BROWSER_CHROME_DIAGNOSTIC_TOOL_NAMES.has(name);
}

/**
 * Uses the locked upstream schema when available so existing Windows callers
 * remain compatible, while making the ownership and ordering of the common
 * catalog independent from chrome-devtools-mcp.
 */
export function mergePinvouBrowserCatalog(
  tools,
  {
    includeChromeDiagnostics = false,
    synthesizeMissingCore = true,
    preserveUpstreamOrder = false,
  } = {},
) {
  if (preserveUpstreamOrder && !synthesizeMissingCore) {
    return (Array.isArray(tools) ? tools : []).filter((tool) => (
      isPinvouBrowserCoreTool(tool?.name) ||
      (includeChromeDiagnostics && isChromeDiagnosticTool(tool?.name)) ||
      tool?.name === 'take_screenshot' ||
      tool?.name === 'upload_file'
    ));
  }
  const upstream = new Map(
    (Array.isArray(tools) ? tools : [])
      .filter((tool) => tool && typeof tool.name === 'string')
      .map((tool) => [tool.name, tool]),
  );
  const common = [];
  for (const name of PINVOU_BROWSER_CORE_TOOL_NAMES) {
    const tool = upstream.get(name);
    if (tool) {
      common.push(tool);
      continue;
    }
    if (!synthesizeMissingCore) continue;
    const inputSchema = FALLBACK_SCHEMAS[name];
    if (!inputSchema) continue;
    common.push({
      name,
      description: `Pinvou BrowserCore ${name}`,
      inputSchema,
    });
  }
  if (!includeChromeDiagnostics) return common;
  for (const name of PINVOU_BROWSER_CHROME_DIAGNOSTIC_TOOL_NAMES) {
    const tool = upstream.get(name);
    if (tool) common.push(tool);
  }
  return common;
}
