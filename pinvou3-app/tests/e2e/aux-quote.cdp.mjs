/**
 * CDP-driven E2E for the aux-chat conversation-quote (selected-text quote) loop.
 *
 * Drives a RUNNING dev desktop app through its WebView2 remote-debugging port
 * (launch the app with WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222):
 * every interaction is a DOM/React event dispatched from Runtime.evaluate —
 * no mouse/keyboard automation, the host screen stays free for the user.
 *
 * All DOM reads use textContent and element presence, never innerText or
 * layout-dependent APIs: the window may be minimized/occluded, and WebView2
 * stops laying out hidden content (innerText then reads as empty).
 *
 * Scenarios (all against the real backend and the configured model):
 *   1. main chat round trip (real model reply with selectable prose)
 *   2. select assistant text -> quote button -> aux panel opens with 1 chip
 *   3. duplicate selection does not add a second chip
 *   4. a second distinct quote adds a chip (staging across popover rounds)
 *   5. chip removal works
 *   6. send question + quote: user bubble renders chips (no raw userselect
 *      JSON), the model's answer engages the quoted content
 *   7. quote-only send (empty draft) is allowed and answers
 *   8. closing the panel keeps staged quotes; a new quote re-opens the panel
 *   9. aux zero-tools: asking to run a command stays a pure Q&A turn
 *  10. main chat tool turn: a shell command executes and the output returns
 *
 * Usage: node tests/e2e/aux-quote.cdp.mjs [--cdp-port 9222]
 * Exits non-zero when any scenario fails.
 */
const CDP_PORT = Number(process.argv.includes('--cdp-port')
  ? process.argv[process.argv.indexOf('--cdp-port') + 1]
  : 9222);
const MODEL_WAIT_MS = 120_000;
const NONCE = String(Date.now() % 100000);

let ws;
let seq = 0;
const pending = new Map();

function cdpSend(method, params = {}) {
  const id = ++seq;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });
}

async function evaluate(expression, { awaitPromise = false } = {}) {
  const result = await cdpSend('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise,
  });
  if (result.exceptionDetails) {
    const detail = result.exceptionDetails.exception?.description
      || result.exceptionDetails.text
      || 'unknown page exception';
    throw new Error(`page eval failed: ${detail}\n  expr: ${expression.slice(0, 200)}`);
  }
  return result.result.value;
}

async function waitFor(expression, { timeoutMs = 15_000, label = 'condition', pollMs = 250 } = {}) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await evaluate(expression).catch(() => null);
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`timeout waiting for: ${label}`);
    await new Promise((r) => { setTimeout(r, pollMs); });
  }
}

async function connect(port) {
  const list = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
  const page = list.find((t) => t.type === 'page' && t.url.includes('127.0.0.1:1420'));
  if (!page) throw new Error(`no dev-app page target on 127.0.0.1:${port}/json`);
  ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener('open', resolve);
    ws.addEventListener('error', reject);
  });
  ws.addEventListener('message', (event) => {
    const message = JSON.parse(event.data);
    if (message.id && pending.has(message.id)) {
      const { resolve, reject } = pending.get(message.id);
      pending.delete(message.id);
      if (message.error) reject(new Error(message.error.message));
      else resolve(message.result);
    }
  });
  await cdpSend('Runtime.enable');
  return page.url;
}

// Injected page helpers. Self-contained: everything the scenarios need runs
// inside this one IIFE so the page never needs extra round trips for setup.
const HELPERS = `(() => {
  // Always (re)define: a page reload between runs would drop __t, while a
  // rerun against the same page must overwrite the previous helper set.
  const nativeSetter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
  const q = (sel, root) => (root || document).querySelector(sel);
  // Locate the composer by testid: its placeholder is scene-dependent
  // (ChatView swaps it for design/PPT/workbench scenes), so matching on the
  // default-home "PINVOU" text would break scenarios 1/10 in any other scene.
  const mainInput = () => q('[data-testid="chat-composer-input"]');
  window.__t = {
    setMain(text) {
      const el = mainInput();
      if (!el) return 'no-main-input';
      nativeSetter.call(el, text);
      el.dispatchEvent(new Event('input', { bubbles: true }));
      return 'ok';
    },
    sendMain() {
      const el = mainInput();
      if (!el) return 'no-main-input';
      el.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
      return 'ok';
    },
    mainText() {
      const el = q('[data-testid="chat-scroll"]');
      return el ? el.textContent : '';
    },
    // Select text from the LAST assistant block INSIDE the main timeline
    // (the aux panel's own answers are also .codex-markdown and live outside
    // the container — quoting them must stay unsupported, so scope strictly).
    selectQuote(offsetFrom) {
      const scope = q('[data-testid="chat-scroll"]') || document;
      const blocks = Array.from(scope.querySelectorAll('.msg-md, .codex-markdown'))
        .filter((el) => (el.textContent || '').trim().length > 60);
      const block = blocks[blocks.length - 1];
      if (!block) return 'no-assistant-block';
      const walker = document.createTreeWalker(block, NodeFilter.SHOW_TEXT);
      const nodes = [];
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        if ((n.textContent || '').trim().length > 6) nodes.push(n);
      }
      const node = offsetFrom === 'tail' ? nodes[nodes.length - 1] : nodes[0];
      if (!node) return 'no-text-node';
      const text = node.textContent;
      // mid quotes a DIFFERENT text node (the second one) so it can never
      // dedupe-collapse into the head/tail excerpts of neighboring nodes.
      let start = 0;
      let end = Math.min(48, text.length);
      if (offsetFrom === 'tail') {
        end = text.length;
        start = Math.max(0, end - 40);
      } else if (offsetFrom === 'mid') {
        const midNode = nodes.length > 1 ? nodes[1] : nodes[0];
        const midText = midNode.textContent;
        const midStart = nodes.length > 1 ? 0 : Math.min(56, Math.max(0, midText.length - 8));
        const midEnd = Math.min(midStart + 44, midText.length);
        const midRange = document.createRange();
        midRange.setStart(midNode, midStart);
        midRange.setEnd(midNode, midEnd);
        const midSel = window.getSelection();
        midSel.removeAllRanges();
        midSel.addRange(midRange);
        document.dispatchEvent(new MouseEvent('mouseup', {
          bubbles: true, button: 0, clientX: 300, clientY: 300,
        }));
        return midText.slice(midStart, midEnd);
      }
      const range = document.createRange();
      range.setStart(node, start);
      range.setEnd(node, end);
      const selection = window.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
      document.dispatchEvent(new MouseEvent('mouseup', {
        bubbles: true, button: 0, clientX: 300, clientY: 300,
      }));
      return text.slice(start, end);
    },
    quoteButtonInfo() {
      const btn = q('[data-testid="aux-quote-selection-button"]');
      return btn ? { text: btn.textContent, visible: true } : { visible: false };
    },
    clickQuote() {
      const btn = q('[data-testid="aux-quote-selection-button"]');
      if (!btn) return 'no-quote-button';
      btn.click();
      return 'ok';
    },
    auxPanel() {
      const panel = q('[data-testid="aux-chat-panel"]');
      if (!panel) return null;
      return {
        chips: panel.querySelectorAll('[data-testid="aux-quote-remove"]').length,
        input: (() => { const el = q('[data-testid="aux-chat-input"]'); return el ? el.value : null; })(),
        sendDisabled: (() => { const el = q('[data-testid="aux-chat-send"]'); return el ? el.disabled : null; })(),
        quoteChipsInTimeline: panel.querySelectorAll('[data-testid="conversation-user-quote"]').length,
        busyStatus: !!q('[data-testid="aux-chat-busy-hint"]', panel),
        rawLeak: panel.textContent.includes('userselect'),
        text: panel.textContent,
      };
    },
    auxAssistantText() {
      const panel = q('[data-testid="aux-chat-panel"]');
      if (!panel) return '';
      const blocks = Array.from(panel.querySelectorAll('.codex-markdown'))
        .map((el) => el.textContent || '')
        .filter((t) => t.trim());
      return blocks[blocks.length - 1] || '';
    },
    setAux(text) {
      const el = q('[data-testid="aux-chat-input"]');
      if (!el) return 'no-aux-input';
      nativeSetter.call(el, text);
      el.dispatchEvent(new Event('input', { bubbles: true }));
      return 'ok';
    },
    sendAux() {
      const el = q('[data-testid="aux-chat-send"]');
      if (!el) return 'no-aux-send';
      el.click();
      return 'ok';
    },
    closeAux() {
      // Locate by testid: the close button's aria-label is localized
      // (zh / en / ja each have their own close string), so matching on the
      // zh string breaks scenario 8 whenever the UI language is en or ja.
      const btn = q('[data-testid="aux-chat-close"]');
      if (!btn) return 'no-close-button';
      btn.click();
      return 'ok';
    },
    toolItems() {
      return document.querySelectorAll('[data-testid="conversation-compact-item-toggle"]').length;
    },
  };
  return 'ok';
})()`;

const scenarios = [];
function scenario(name, fn) {
  scenarios.push({ name, fn });
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

const sleep = (ms) => new Promise((r) => { setTimeout(r, ms); });

// Select + wait for the quote button, retrying when a still-settling timeline
// (markdown re-render detaches the selected node) collapses the selection
// before the popover state lands.
async function quoteWithRetry(offset, label) {
  for (let attempt = 0; attempt < 4; attempt += 1) {
    const selected = await evaluate(`window.__t.selectQuote(${JSON.stringify(offset)})`);
    if (typeof selected === 'string' && selected.length > 4) {
      const info = await waitFor(
        '(() => { const i = window.__t.quoteButtonInfo(); return i.visible ? i : null; })()',
        { timeoutMs: 4000, label: `${label} button appeared (attempt ${attempt + 1})` },
      ).catch(() => null);
      if (info) return { selected, info };
    }
    await sleep(700);
  }
  throw new Error(`quote button never appeared: ${label}`);
}

// Wait until the main timeline text grows beyond baseline and then stays
// unchanged for 2.5s (streaming finished).
async function waitForStableMain(baselineLength, label) {
  const deadline = Date.now() + MODEL_WAIT_MS;
  let last = '';
  let stableSince = 0;
  while (Date.now() < deadline) {
    const text = await evaluate('window.__t.mainText()');
    if (text !== last) {
      last = text;
      stableSince = Date.now();
    } else if (Date.now() - stableSince > 2500 && text.length > baselineLength) {
      return last;
    }
    await sleep(400);
  }
  throw new Error(`main reply never stabilized: ${label}`);
}

// Wait until the aux panel exists, is not busy (no busy hint), has a
// non-empty latest assistant block, and its text stopped changing.
async function waitForStableAux(label) {
  const deadline = Date.now() + MODEL_WAIT_MS;
  let last = '';
  let stableSince = 0;
  while (Date.now() < deadline) {
    const state = await evaluate('window.__t.auxPanel()');
    const text = state ? state.text : '';
    if (text !== last) {
      last = text;
      stableSince = Date.now();
    } else if (Date.now() - stableSince > 2500) {
      const answer = await evaluate('window.__t.auxAssistantText()');
      const busy = state ? state.busyStatus : true;
      if (state && !busy && answer) return state;
    }
    await sleep(400);
  }
  throw new Error(`aux reply never stabilized: ${label}`);
}

scenario('main chat real model round trip (generates a long selectable reply)', async () => {
  const question = `E2E-${NONCE}: explain the differences between HTTP and HTTPS in bullet points (ports, encryption, certificates, performance), at least 300 words in total.`;
  assert(await evaluate(`window.__t.setMain(${JSON.stringify(question)})`) === 'ok');
  const baseline = (await evaluate('window.__t.mainText()')).length;
  assert(await evaluate('window.__t.sendMain()') === 'ok');
  const text = await waitForStableMain(baseline, 'first main reply');
  assert(text.includes('HTTP'), 'reply must contain HTTP');
  assert(text.includes(NONCE) || text.length > baseline + 200, 'main reply must grow significantly');
});

scenario('selecting text shows the "quote to aux chat" button and opens the panel with a chip', async () => {
  const { selected, info } = await quoteWithRetry('head', 'first selection');
  assert(selected.length > 10, 'must select a span of assistant text');
  assert(info.text.includes('引用') || info.text.toLowerCase().includes('quote'), `unexpected button text: ${info.text}`);
  assert(await evaluate('window.__t.clickQuote()') === 'ok');
  const panel = await waitFor('window.__t.auxPanel() || null', { label: 'aux panel opened' });
  assert(panel.chips === 1, `expected 1 quote chip, got ${panel.chips}`);
  assert(!panel.rawLeak, 'panel must not show raw userselect text');
});

scenario('re-selecting the same text does not add a chip', async () => {
  await quoteWithRetry('head', 'repeat selection');
  assert(await evaluate('window.__t.clickQuote()') === 'ok');
  await sleep(800);
  const panel = await evaluate('window.__t.auxPanel()');
  assert(panel.chips === 1, `chip count must stay 1 after a duplicate quote, got ${panel.chips}`);
});

scenario('a second distinct quote accumulates a chip', async () => {
  const { selected } = await quoteWithRetry('tail', 'second selection');
  assert(selected.length > 5, 'must select a second span of text');
  assert(await evaluate('window.__t.clickQuote()') === 'ok');
  const panel = await waitFor('(() => { const p = window.__t.auxPanel(); return p && p.chips === 2 ? p : null; })()', { label: 'chip count becomes 2' });
  assert(!panel.rawLeak, 'panel must not leak userselect');
});

scenario('remove one quote chip', async () => {
  assert(await evaluate(`(() => {
    const panel = document.querySelector('[data-testid="aux-chat-panel"]');
    const btns = panel.querySelectorAll('[data-testid="aux-quote-remove"]');
    btns[btns.length - 1].click();
    return 'ok';
  })()`) === 'ok');
  await waitFor('window.__t.auxPanel().chips === 1', { label: 'chip count back to 1' });
});

scenario('send question+quote: chips render and the model engages the quoted content', async () => {
  assert(await evaluate('window.__t.setAux("Which protocol port is mentioned in the quoted content above? Reply with only the port number and the protocol name.")') === 'ok');
  assert(await evaluate('window.__t.sendAux()') === 'ok');
  const panel = await waitForStableAux('quote Q&A');
  assert(panel.quoteChipsInTimeline >= 1, 'user bubble must render quote chips after send');
  assert(!panel.rawLeak, 'timeline must not leak raw userselect JSON');
  const answer = await evaluate('window.__t.auxAssistantText()');
  assert(answer.length > 4, `aux answer is empty: ${answer.slice(0, 80)}`);
  assert(/443|80|HTTPS|HTTP/i.test(answer), `answer must mention the port/protocol, got: ${answer.slice(0, 160)}`);
  const after = await evaluate('window.__t.auxPanel()');
  assert(after.chips === 0, `staged quotes must clear after a successful send, got ${after.chips}`);
});

scenario('quote-only send without body text is allowed and answered', async () => {
  await quoteWithRetry('tail', 'quote-only scenario');
  assert(await evaluate('window.__t.clickQuote()') === 'ok');
  const panel = await waitFor('(() => { const p = window.__t.auxPanel(); return p && p.chips === 1 ? p : null; })()', { label: 'panel has 1 chip' });
  assert(panel.input === '', 'draft must stay empty');
  assert(panel.sendDisabled === false, 'send button must be enabled with quotes only');
  assert(await evaluate('window.__t.sendAux()') === 'ok');
  const done = await waitForStableAux('quote-only send');
  assert(done.quoteChipsInTimeline >= 1, 'quote-only message must also render chips');
  assert(!done.rawLeak, 'must not leak userselect');
});

scenario('closing the panel keeps staged quotes; a new selection re-opens the panel', async () => {
  // Count after cleanup: three mutually distinct excerpts (head/tail/mid),
  // so dedupe does not affect the arithmetic
  for (let i = 0; i < 5; i += 1) {
    await evaluate(`(() => {
      const panel = document.querySelector('[data-testid="aux-chat-panel"]');
      if (!panel) return 'skip';
      const btns = panel.querySelectorAll('[data-testid="aux-quote-remove"]');
      if (btns.length) btns[0].click();
      return 'ok';
    })()`);
    await sleep(200);
  }
  await quoteWithRetry('head', 'persistence scenario');
  assert(await evaluate('window.__t.clickQuote()') === 'ok');
  await waitFor('window.__t.auxPanel().chips === 1', { label: 'first staged quote' });
  await quoteWithRetry('tail', 'persistence scenario second quote');
  assert(await evaluate('window.__t.clickQuote()') === 'ok');
  await waitFor('window.__t.auxPanel().chips === 2', { label: 'staged quotes accumulate to 2' });
  assert(await evaluate('window.__t.closeAux()') === 'ok');
  await waitFor('!window.__t.auxPanel()', { label: 'panel closed' });
  await sleep(600);
  await quoteWithRetry('mid', 'select again after closing the panel');
  assert(await evaluate('window.__t.clickQuote()') === 'ok');
  const panel = await waitFor('(() => { const p = window.__t.auxPanel(); return p && p.chips === 3 ? p : null; })()', { label: 'staged quotes total 3 after panel reopens' });
  assert(!panel.rawLeak, 'must not leak userselect');
  // Cleanup: remove all chips so later scenarios are unaffected
  for (let i = 0; i < 3; i += 1) {
    await evaluate(`(() => {
      const panel = document.querySelector('[data-testid="aux-chat-panel"]');
      const btns = panel.querySelectorAll('[data-testid="aux-quote-remove"]');
      if (btns.length) btns[0].click();
      return 'ok';
    })()`);
    await sleep(250);
  }
  const cleared = await evaluate('window.__t.auxPanel()');
  assert(cleared.chips === 0, `cleanup failed, ${cleared.chips} chips remain`);
});

scenario('aux chat zero-tools: asking to run a command stays pure Q&A', async () => {
  assert(await evaluate('window.__t.setAux("Please run the shell command echo pinvou-zero-tools and tell me the output. If you cannot run tools, say so directly.")') === 'ok');
  assert(await evaluate('window.__t.sendAux()') === 'ok');
  await waitForStableAux('zero-tools check');
  const answer = await evaluate('window.__t.auxAssistantText()');
  assert(answer.length > 4, `zero-tools answer is empty: ${answer.slice(0, 80)}`);
  // Leak regression: with zero tools DeepSeek emits native tool-call markers
  // (DSML invoke blocks) as body text — after the boundary-prompt merge, the
  // answer must not contain any tool-call markers.
  assert(!/invoke name=|<｜|DSML｜|tool_calls|<tool_call/i.test(answer),
    `zero-tools answer leaked tool-call markers: ${answer.slice(0, 200)}`);
  const toolNodes = await evaluate(`(() => {
    const panel = document.querySelector('[data-testid="aux-chat-panel"]');
    return panel ? panel.querySelectorAll('[data-testid="conversation-compact-item-toggle"]').length : 0;
  })()`);
  assert(toolNodes === 0, `aux session must not show tool execution entries, got ${toolNodes}`);
});

scenario('main chat tool path: a shell command really executes and returns output', async () => {
  assert(await evaluate('window.__t.setMain("Please run the shell command echo pinvou-e2e-tools-ok and tell me the command output verbatim.")') === 'ok');
  const baseline = (await evaluate('window.__t.mainText()')).length;
  assert(await evaluate('window.__t.sendMain()') === 'ok');
  const text = await waitForStableMain(baseline, 'tool turn');
  assert(text.includes('pinvou-e2e-tools-ok'), `main chat must contain the command output, excerpt: ${text.slice(-400)}`);
  // user message (1) + tool-card command and output (2) + assistant restating
  // it (1): the marker appearing >=3 times proves the tool card really rendered
  const occurrences = text.split('pinvou-e2e-tools-ok').length - 1;
  assert(occurrences >= 3, `insufficient tool-execution traces (${occurrences} occurrences), excerpt: ${text.slice(-300)}`);
});

async function main() {
  const url = await connect(CDP_PORT);
  console.log(`connected: ${url} (nonce ${NONCE})`);
  // Fresh page state per run: clears in-page module stores (staged quotes)
  // left over from previous runs or probes.
  await cdpSend('Page.enable');
  await cdpSend('Page.reload', { ignoreCache: true });
  await sleep(6000);
  const injected = await evaluate(HELPERS);
  console.log(`helpers: ${injected}`);
  const failures = [];
  for (const { name, fn } of scenarios) {
    const started = Date.now();
    try {
      await fn();
      console.log(`PASS  ${name} (${((Date.now() - started) / 1000).toFixed(1)}s)`);
    } catch (error) {
      failures.push({ name, error });
      console.error(`FAIL  ${name}: ${error.message}`);
    }
  }
  console.log(`\n${scenarios.length - failures.length}/${scenarios.length} scenarios passed`);
  if (failures.length) {
    process.exitCode = 1;
  }
  ws.close();
}

try {
  await main();
} catch (error) {
  console.error(`driver error: ${error.message}`);
  process.exitCode = 1;
}
