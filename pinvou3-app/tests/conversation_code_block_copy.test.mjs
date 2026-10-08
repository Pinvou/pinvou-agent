import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

import domino from '@mixmark-io/domino';

import {
  CODE_COPY_BLOCK_SELECTOR,
  CODE_COPY_BUTTON_CLASS,
  CODE_COPY_LIVE_CLASS,
  codeBlockCopyLabels,
  codeBlockTextFromButton,
  copyCodeBlockFromButton,
  ensureCodeCopyButtons,
  findCodeCopyButton,
  observeCodeCopyButtons,
} from '../src/features/conversation/code-block-copy.js';
import { renderMarkdownMarkup } from '../src/shared/markdown-renderer.js';
import { dict } from './helpers/i18n-all.js'; // full three-language dict: browser entry lazy-loads via i18n.js, tests use the aggregate shim

const LABELS = { copyCode: '复制代码', copied: '已复制', failed: '复制失败' };
const source = relative => readFileSync(new URL(`../src/${relative}`, import.meta.url), 'utf8');

// copyCode 词条三语钉住:任一语言删掉词条会让此处变红,按钮将退回模块内英文兜底
// (与 assistant_message_actions.test.mjs 钉 copyReply 家族同一模式)。
for (const language of ['zh', 'en', 'ja']) {
  assert.ok(dict[language].uiConversation.copyCode, `${language}.uiConversation.copyCode must exist`);
}

function documentFromMarkdown(markdown) {
  return domino.createDocument(`<div class="codex-markdown">${renderMarkdownMarkup(markdown)}</div>`);
}

// 词条映射:成功/失败反馈复用整条回复复制的既有 key。
{
  const labels = codeBlockCopyLabels({
    copyCode: '复制代码',
    copyReplySuccess: '已复制',
    copyReplyFailed: '复制失败',
  });
  assert.deepEqual(labels, LABELS);
  assert.deepEqual(codeBlockCopyLabels(null), { copyCode: undefined, copied: undefined, failed: undefined });
}

// 代码围栏与无语言围栏(纯文本框)都渲染成 pinvou-code-block,都装得上按钮。
{
  const doc = documentFromMarkdown([
    '```js',
    'const answer = 42;',
    '```',
    '',
    '```',
    'plain boxed text',
    '```',
  ].join('\n'));
  const blocks = doc.querySelectorAll(CODE_COPY_BLOCK_SELECTOR);
  assert.equal(blocks.length, 2, 'both fenced blocks must render as pinvou-code-block');
  assert.equal(ensureCodeCopyButtons(doc, LABELS), 2);
  const buttons = doc.querySelectorAll(`button.${CODE_COPY_BUTTON_CLASS}`);
  assert.equal(buttons.length, 2);
  for (const button of buttons) {
    assert.equal(button.getAttribute('type'), 'button');
    assert.equal(button.getAttribute('aria-label'), '复制代码');
    assert.equal(button.getAttribute('title'), '复制代码');
    assert.ok(button.innerHTML.includes('M4 16c-1.1 0-2-.9-2-2V4'), 'button must render the copy icon (lucide Copy path)');
  }
  const lives = doc.querySelectorAll(`.${CODE_COPY_LIVE_CLASS}`);
  assert.equal(lives.length, 2, 'each block must install a live region next to the button');
  for (const live of lives) {
    assert.equal(live.getAttribute('aria-live'), 'polite');
    assert.equal(live.textContent, '');
  }
  // 幂等:流式重渲染后重复调用不得叠加按钮。
  assert.equal(ensureCodeCopyButtons(doc, LABELS), 0);
  assert.equal(doc.querySelectorAll(`button.${CODE_COPY_BUTTON_CLASS}`).length, 2);
}

// 空容器与非元素入参安全返回。
{
  assert.equal(ensureCodeCopyButtons(null, LABELS), 0);
  assert.equal(ensureCodeCopyButtons(domino.createDocument('<p>no code</p>'), LABELS), 0);
}

// 取文只取 code 子元素内容,不混入按钮自身文本。
{
  const doc = documentFromMarkdown('```js\nconst answer = 42;\n```');
  ensureCodeCopyButtons(doc, LABELS);
  const button = doc.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`);
  const text = codeBlockTextFromButton(button);
  assert.ok(text.includes('const answer = 42;'), `copied text must be the code, got: ${JSON.stringify(text)}`);
  assert.ok(!text.includes('复制代码'), 'copied text must not include the button label');
}

// 点击识别:命中按钮内图标也能定位到按钮;非按钮区域返回 null。
{
  const doc = documentFromMarkdown('```js\nlet x = 1;\n```');
  ensureCodeCopyButtons(doc, LABELS);
  const button = doc.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`);
  const icon = button.querySelector('svg');
  assert.equal(findCodeCopyButton(icon), button);
  assert.equal(findCodeCopyButton(button), button);
  assert.equal(findCodeCopyButton(doc.querySelector('code')), null);
  assert.equal(findCodeCopyButton(null), null);
}

// 复制成功:写入内容是代码文本,按钮进入已复制反馈态。
{
  const doc = documentFromMarkdown('```python\nprint("hi")\n```');
  ensureCodeCopyButtons(doc, LABELS);
  const button = doc.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`);
  const written = [];
  const copied = await copyCodeBlockFromButton(button, {
    labels: LABELS,
    writeText: (value) => { written.push(value); return Promise.resolve(true); },
  });
  assert.equal(copied, true);
  assert.equal(written.length, 1);
  assert.ok(written[0].includes('print("hi")'));
  assert.ok(button.classList.contains('is-copied'));
  assert.equal(button.getAttribute('title'), '已复制');
}

// 复制失败路径(writeText 返回 false 与 reject 双通道):如实进入失败反馈态,不伪造成功。
{
  const doc = documentFromMarkdown('```js\nlet y = 2;\n```');
  ensureCodeCopyButtons(doc, LABELS);
  const button = doc.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`);
  const copied = await copyCodeBlockFromButton(button, {
    labels: LABELS,
    writeText: () => Promise.resolve(false),
  });
  assert.equal(copied, false);
  assert.ok(button.classList.contains('is-failed'));
  assert.equal(button.getAttribute('title'), '复制失败');
  assert.ok(button.innerHTML.includes('M18 6 6 18'), 'failure state must swap in the X icon');
  assert.equal(button.closest(CODE_COPY_BLOCK_SELECTOR).querySelector(`.${CODE_COPY_LIVE_CLASS}`).textContent, '复制失败');

  const rejected = await copyCodeBlockFromButton(button, {
    labels: LABELS,
    writeText: () => Promise.reject(new Error('clipboard denied')),
  });
  assert.equal(rejected, false);
  assert.ok(button.classList.contains('is-failed'));

  // 失败→重试成功:状态类换态,旧失败类必须摘除(否则红绿颜色反馈自相矛盾)。
  assert.equal(await copyCodeBlockFromButton(button, {
    labels: LABELS,
    writeText: () => Promise.resolve(true),
  }), true);
  assert.ok(button.classList.contains('is-copied'));
  assert.equal(button.classList.contains('is-failed'), false, 'a retried success must clear the failed class');
}

// writeText 通道:非 Promise 返回值经 Promise.resolve 归一;同步 throw(非 reject)
// 不冒泡、按失败反馈;缺省 options 走 copyClipboardText(Node 下无剪贴板,软失败)。
{
  const doc = documentFromMarkdown('```js\nlet w = 1;\n```');
  ensureCodeCopyButtons(doc, LABELS);
  const button = doc.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`);
  assert.equal(await copyCodeBlockFromButton(button, { labels: LABELS, writeText: () => true }), true, 'non-promise writeText must resolve to success');
  assert.equal(await copyCodeBlockFromButton(button, {
    labels: LABELS,
    writeText: () => { throw new Error('sync boom'); },
  }), false);
  assert.ok(button.classList.contains('is-failed'));
  assert.equal(await copyCodeBlockFromButton(button), false, 'default clipboard channel must fail soft outside a browser');
}

// 空代码块与无 code 子元素:不碰剪贴板、如实失败反馈——与 copyClipboardText 对
// 空文本返回 false 的既有语义一致,不伪造成功。
{
  const doc = documentFromMarkdown('```js\n```');
  assert.ok(doc.querySelector(CODE_COPY_BLOCK_SELECTOR), 'empty fenced block must render as pinvou-code-block');
  ensureCodeCopyButtons(doc, LABELS);
  const button = doc.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`);
  let writeCount = 0;
  assert.equal(await copyCodeBlockFromButton(button, {
    labels: LABELS,
    writeText: () => { writeCount += 1; return true; },
  }), false);
  assert.equal(writeCount, 0, 'empty block must not touch the clipboard');
  assert.ok(button.classList.contains('is-failed'));

  const bareDoc = domino.createDocument('<div><pre class="pinvou-code-block">no code child</pre></div>');
  ensureCodeCopyButtons(bareDoc, LABELS);
  const bareButton = bareDoc.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`);
  assert.equal(codeBlockTextFromButton(bareButton), '');
  assert.equal(await copyCodeBlockFromButton(bareButton, { labels: LABELS, writeText: () => true }), false);
}

// 观察器自愈:打桩 MutationObserver 驱动真实分支——整体替换 innerHTML 后补装、
// 同一批次多次回调只排队一轮补装(queued 合并)、补装引发的自身记录不自激叠加、
// disconnect 断开观察;无 MutationObserver 的环境(domino/Node)安全降级为空操作。
{
  const realMutationObserver = globalThis.MutationObserver;
  const mutationObserverDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'MutationObserver');
  const instances = [];
  globalThis.MutationObserver = class {
    constructor(callback) { this.callback = callback; instances.push(this); }
    observe(target, options) { this.target = target; this.options = options; }
    disconnect() { this.disconnected = true; }
  };
  try {
    const doc = documentFromMarkdown('```js\nlet z = 3;\n```');
    const container = doc.querySelector('.codex-markdown');
    const disconnect = observeCodeCopyButtons(container, LABELS);
    assert.equal(instances.length, 1);
    const observer = instances[0];
    assert.equal(observer.target, container);
    assert.deepEqual(observer.options, { childList: true, subtree: true });
    ensureCodeCopyButtons(container, LABELS);
    // 模拟流式/冷启动场景:innerHTML 整体替换,已装按钮随之销毁。
    container.innerHTML = renderMarkdownMarkup('```js\nlet z = 4;\n```');
    observer.callback();
    await new Promise((resolve) => { queueMicrotask(resolve); });
    assert.ok(container.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`), 'observer must re-install buttons after innerHTML replacement');
    assert.equal(container.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`).getAttribute('title'), '复制代码', 'healed buttons must carry the labels, not the English fallback');
    // queued 合并:同一批次(微任务落盘前)多次回调只排队一轮补装。
    const realQueueMicrotask = globalThis.queueMicrotask;
    let queuedPasses = 0;
    globalThis.queueMicrotask = (fn) => { queuedPasses += 1; return realQueueMicrotask(fn); };
    try {
      observer.callback();
      observer.callback();
      assert.equal(queuedPasses, 1, 'one mutation batch must queue exactly one heal pass');
      await new Promise((resolve) => { realQueueMicrotask(resolve); });
    } finally {
      globalThis.queueMicrotask = realQueueMicrotask;
    }
    // 第二个批次:queued 复位后新批次照常自愈(钉住复位语义);补装自身的 DOM 记录
    // 再触发回调不叠加按钮(ensure 幂等,收敛不自激)。
    container.innerHTML = renderMarkdownMarkup('```js\nlet z = 5;\n```');
    observer.callback();
    await new Promise((resolve) => { queueMicrotask(resolve); });
    assert.ok(container.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`), 'a later batch must heal again after the queued flag resets');
    assert.equal(container.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`).getAttribute('title'), '复制代码');
    observer.callback();
    await new Promise((resolve) => { queueMicrotask(resolve); });
    assert.equal(container.querySelectorAll(`button.${CODE_COPY_BUTTON_CLASS}`).length, 1, 'self-records from re-install must not stack buttons');
    disconnect();
    assert.equal(observer.disconnected, true);
    // 断开后浏览器不再派发回调(桩忠实模拟:不再手动触发),自愈随之停止。
  } finally {
    if (mutationObserverDescriptor) Object.defineProperty(globalThis, 'MutationObserver', mutationObserverDescriptor);
    else delete globalThis.MutationObserver;
  }

  // 降级分支:MutationObserver 为非函数值(等价于 domino/Node 无实现)时返回空操作。
  globalThis.MutationObserver = undefined;
  try {
    const degradeDoc = documentFromMarkdown('```js\nlet z = 6;\n```');
    const disconnect = observeCodeCopyButtons(degradeDoc, LABELS);
    assert.equal(typeof disconnect, 'function');
    disconnect();
    assert.equal(observeCodeCopyButtons(null, LABELS)(), undefined);
  } finally {
    globalThis.MutationObserver = realMutationObserver;
  }
}

// 语言切换:已装按钮的 title/aria-label 随新词条刷新;反馈态期间跳过,避免刷掉反馈文案。
{
  const doc = documentFromMarkdown('```js\nlet l = 1;\n```');
  ensureCodeCopyButtons(doc, LABELS);
  const button = doc.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`);
  assert.equal(button.getAttribute('title'), '复制代码');
  ensureCodeCopyButtons(doc, { copyCode: 'Copy code', copied: 'Copied', failed: 'Copy failed' });
  assert.equal(button.getAttribute('title'), 'Copy code');
  assert.equal(button.getAttribute('aria-label'), 'Copy code');
  button.classList.add('is-copied');
  ensureCodeCopyButtons(doc, LABELS);
  assert.equal(button.getAttribute('title'), 'Copy code', 'feedback state must keep its label');
}

// 防伪造:消息原生 HTML 拼出的同类名 button 不带安装标记(JS expando 经 HTML 解析
// 无法产生),点击识别不认账;ensure 还会摘掉伪按钮的本模块类名(外观通道防伪,伪
// 按钮回落为普通按钮渲染)并照常安装真按钮——伪按钮既不可点也不再有可信样式。
// 多个伪按钮全量中和:每个都摘类名,真按钮恰一个,后续轮次收敛不叠加。
{
  const doc = domino.createDocument('<div><pre class="pinvou-code-block"><code>spoof</code><button type="button" class="pinvou-code-copy-btn is-copied"><svg></svg></button></pre></div>');
  const spoofed = doc.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`);
  assert.equal(findCodeCopyButton(spoofed), null);
  assert.equal(findCodeCopyButton(spoofed.querySelector('svg')), null);
  assert.equal(ensureCodeCopyButtons(doc, LABELS), 1, 'the block must get a genuine button despite the spoof');
  assert.equal(doc.querySelectorAll(`button.${CODE_COPY_BUTTON_CLASS}`).length, 1);
  assert.equal(spoofed.classList.contains(CODE_COPY_BUTTON_CLASS), false, 'spoofed button must lose the copy-button class');
  assert.equal(spoofed.classList.contains('is-copied'), false, 'spoofed state classes must be stripped too');
  const genuine = doc.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`);
  assert.equal(findCodeCopyButton(genuine), genuine, 'the genuine button must be recognized');

  const multiDoc = domino.createDocument('<div><pre class="pinvou-code-block"><code>x</code><button type="button" class="pinvou-code-copy-btn is-copied">a</button><button type="button" class="pinvou-code-copy-btn is-failed">b</button></pre></div>');
  assert.equal(ensureCodeCopyButtons(multiDoc, LABELS), 1);
  const multiGenuine = multiDoc.querySelectorAll(`button.${CODE_COPY_BUTTON_CLASS}`);
  assert.equal(multiGenuine.length, 1, 'exactly one genuine button must exist');
  const leftovers = [...multiDoc.querySelectorAll('pre button')].filter(b => b !== multiGenuine[0]);
  assert.equal(leftovers.length, 2);
  for (const spoof of leftovers) {
    assert.equal(spoof.classList.contains(CODE_COPY_BUTTON_CLASS), false, 'every spoofed button must be neutralized');
    assert.equal(spoof.classList.contains('is-copied'), false);
    assert.equal(spoof.classList.contains('is-failed'), false);
  }
  assert.equal(ensureCodeCopyButtons(multiDoc, LABELS), 0, 'a second pass must converge with no new installs');
  assert.equal(multiDoc.querySelectorAll(`button.${CODE_COPY_BUTTON_CLASS}`).length, 1);
  assert.equal(multiDoc.querySelectorAll(`.${CODE_COPY_LIVE_CLASS}`).length, 1);

  // 中和不限标签:CSS 类名选择器不限标签,伪 div 带同类名同样摘除。
  const tagDoc = domino.createDocument('<div><pre class="pinvou-code-block"><code>x</code><div class="pinvou-code-copy-btn is-copied">fake</div></pre></div>');
  const fakeDiv = tagDoc.querySelector(`.${CODE_COPY_BUTTON_CLASS}`);
  assert.equal(ensureCodeCopyButtons(tagDoc, LABELS), 1);
  assert.equal(fakeDiv.classList.contains(CODE_COPY_BUTTON_CLASS), false, 'non-button spoofed elements must be neutralized too');
  assert.equal(fakeDiv.classList.contains('is-copied'), false);
  assert.equal(tagDoc.querySelectorAll(`button.${CODE_COPY_BUTTON_CLASS}`).length, 1);
}

// 嵌套 pre 归属:外层只认自己直接挂的按钮/live region,不被后代满足——外层缺件
// 照常自愈、文案刷新不落错对象。
{
  const nestedDoc = domino.createDocument('<div><pre class="pinvou-code-block"><code>outer</code><pre class="pinvou-code-block"><code>inner</code></pre></pre></div>');
  assert.equal(ensureCodeCopyButtons(nestedDoc, LABELS), 2);
  const [outer, inner] = [...nestedDoc.querySelectorAll(CODE_COPY_BLOCK_SELECTOR)];
  const ownButton = (pre) => [...pre.querySelectorAll(`button.${CODE_COPY_BUTTON_CLASS}`)]
    .find((b) => b.closest(CODE_COPY_BLOCK_SELECTOR) === pre);
  assert.ok(ownButton(outer) && ownButton(inner));
  // 语言切换:外层自装按钮的文案随新词条刷新(不被内层按钮顶替)。
  ensureCodeCopyButtons(nestedDoc, { copyCode: 'Copy code', copied: 'Copied', failed: 'Copy failed' });
  assert.equal(ownButton(outer).getAttribute('title'), 'Copy code');
  assert.equal(ownButton(inner).getAttribute('title'), 'Copy code');
  // 删外层自装按钮:外层自愈补装(不被内层满足),live region 不重复。
  ownButton(outer).remove();
  assert.equal(ensureCodeCopyButtons(nestedDoc, LABELS), 1);
  assert.ok(ownButton(outer), 'outer must heal its own button');
  assert.ok(ownButton(inner), 'inner button must be untouched');
  const outerLives = [...outer.querySelectorAll(`.${CODE_COPY_LIVE_CLASS}`)]
    .filter((s) => s.closest(CODE_COPY_BLOCK_SELECTOR) === outer);
  assert.equal(outerLives.length, 1, 'healing must not duplicate the outer live region');

  // 删外层自装 live region:外层补装自己的(不被后代满足——findLiveRegion 的归属
  // 判定),反馈路由到外层 live,内层保持为空。
  const ownLives = (pre) => [...pre.querySelectorAll(`.${CODE_COPY_LIVE_CLASS}`)]
    .filter((s) => s.closest(CODE_COPY_BLOCK_SELECTOR) === pre);
  ownLives(outer)[0].remove();
  assert.equal(ensureCodeCopyButtons(nestedDoc, LABELS), 0, 'healing the outer live region installs no button');
  assert.equal(ownLives(outer).length, 1, 'outer must heal its own live region');
  assert.equal(ownLives(inner).length, 1, 'inner live region must be untouched');
  const healedOuterLive = ownLives(outer)[0];
  const innerLive = ownLives(inner)[0];
  await copyCodeBlockFromButton(ownButton(outer), { labels: LABELS, writeText: () => Promise.resolve(true) });
  assert.equal(healedOuterLive.textContent, '已复制', 'outer feedback must land in the outer live region');
  assert.equal(innerLive.textContent, '', 'inner live region must stay empty');
}

// live region 自愈与防劫持:被单独删除后 ensure 补装缺件(计数不增,真按钮不动);
// 真按钮被单独删除后自愈补装且不重复 live region;伪造同名 span 被摘类名中和,
// 模块自装 span 按 expando 标记承接反馈。
{
  const doc = documentFromMarkdown('```js\nlet h = 1;\n```');
  ensureCodeCopyButtons(doc, LABELS);
  const pre = doc.querySelector(CODE_COPY_BLOCK_SELECTOR);
  pre.querySelector(`.${CODE_COPY_LIVE_CLASS}`).remove();
  assert.equal(ensureCodeCopyButtons(doc, LABELS), 0, 'healing a missing live region must not count as a new button install');
  assert.ok(pre.querySelector(`.${CODE_COPY_LIVE_CLASS}`), 'ensure must re-install a deleted live region');
  pre.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`).remove();
  assert.equal(ensureCodeCopyButtons(doc, LABELS), 1, 'healing a deleted button counts as a new install');
  assert.equal(pre.querySelectorAll(`.${CODE_COPY_LIVE_CLASS}`).length, 1, 'healing the button must not duplicate the live region');

  const spoofDoc = domino.createDocument('<div><pre class="pinvou-code-block"><code>x</code><span class="pinvou-code-copy-live"></span></pre></div>');
  const spoofSpan = spoofDoc.querySelector(`.${CODE_COPY_LIVE_CLASS}`);
  assert.equal(ensureCodeCopyButtons(spoofDoc, LABELS), 1);
  const spoofPre = spoofDoc.querySelector(CODE_COPY_BLOCK_SELECTOR);
  const button = spoofPre.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`);
  assert.ok(button, 'a genuine block with a spoofed live span still gets a copy button');
  assert.equal(spoofSpan.classList.contains(CODE_COPY_LIVE_CLASS), false, 'spoofed live span must be neutralized');
  await copyCodeBlockFromButton(button, { labels: LABELS, writeText: () => Promise.resolve(true) });
  const spans = [...spoofPre.querySelectorAll(`.${CODE_COPY_LIVE_CLASS}`)];
  assert.equal(spans.length, 1);
  assert.equal(spans[0].textContent, '已复制', 'the module-installed span must carry the feedback');
}

// 反馈定时器:成功态切换图标,复位恢复原文案;重复点击先清旧定时器,早先复位
// 不会刷掉新一轮反馈。
{
  const doc = documentFromMarkdown('```js\nlet t = 1;\n```');
  ensureCodeCopyButtons(doc, LABELS);
  const button = doc.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`);
  const realSetTimeout = globalThis.setTimeout;
  const realClearTimeout = globalThis.clearTimeout;
  const pending = new Map();
  let nextId = 1;
  const cleared = [];
  globalThis.setTimeout = (fn, ms) => { assert.equal(ms, 1600); const id = nextId++; pending.set(id, fn); return id; };
  globalThis.clearTimeout = (id) => { cleared.push(id); pending.delete(id); };
  try {
    const write = () => Promise.resolve(true);
    await copyCodeBlockFromButton(button, { labels: LABELS, writeText: write });
    assert.ok(button.classList.contains('is-copied'));
    assert.ok(button.innerHTML.includes('M20 6 9 17l-5-5'), 'success state must swap in the check icon');
    assert.equal(button.closest(CODE_COPY_BLOCK_SELECTOR).querySelector(`.${CODE_COPY_LIVE_CLASS}`).textContent, '已复制');
    await copyCodeBlockFromButton(button, { labels: LABELS, writeText: write });
    assert.equal(pending.size, 1, 'repeat click must replace the pending reset timer');
    assert.equal(cleared.length, 1, 'repeat click must clear the previous timer');
    for (const reset of pending.values()) reset();
    assert.ok(!button.classList.contains('is-copied'));
    assert.ok(!button.innerHTML.includes('M20 6 9 17l-5-5'), 'reset must restore the copy icon');
    assert.ok(button.innerHTML.includes('M4 16c-1.1 0-2-.9-2-2V4'), 'reset must restore the lucide Copy path');
    assert.equal(button.getAttribute('title'), '复制代码');
    assert.equal(button.getAttribute('aria-label'), '复制代码');
    assert.equal(button.closest(CODE_COPY_BLOCK_SELECTOR).querySelector(`.${CODE_COPY_LIVE_CLASS}`).textContent, '', 'reset must clear the live region');
  } finally {
    globalThis.setTimeout = realSetTimeout;
    globalThis.clearTimeout = realClearTimeout;
  }
}

// 防御性守卫:非元素/非对象入参安全返回,不抛异常。
{
  assert.equal(ensureCodeCopyButtons({}, LABELS), 0);
  assert.equal(ensureCodeCopyButtons('pre', LABELS), 0);
  assert.equal(findCodeCopyButton({}), null);
  assert.equal(codeBlockTextFromButton(null), '');
  assert.equal(codeBlockTextFromButton({}), '');
  assert.equal(codeBlockCopyLabels('zh').copyCode, undefined);
  assert.equal(await copyCodeBlockFromButton(null), false);
  assert.equal(await copyCodeBlockFromButton({}), false);
}

// 游离按钮:反馈窗口内按钮被整体替换/摘除时,复制落定与复位定时器对游离节点幂等
// 无害,不得抛错(模块注释承诺的已知限制路径)。
{
  const doc = documentFromMarkdown('```js\nlet g = 1;\n```');
  ensureCodeCopyButtons(doc, LABELS);
  const button = doc.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`);
  const copied = await copyCodeBlockFromButton(button, {
    labels: LABELS,
    writeText: () => {
      button.remove();
      return Promise.resolve(true);
    },
  });
  assert.equal(copied, true, 'detachment during writeText must not break the copy');

  const resetDoc = documentFromMarkdown('```js\nlet g2 = 1;\n```');
  ensureCodeCopyButtons(resetDoc, LABELS);
  const resetButton = resetDoc.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`);
  const realSetTimeout = globalThis.setTimeout;
  const realClearTimeout = globalThis.clearTimeout;
  let pendingReset = null;
  globalThis.setTimeout = (fn) => { pendingReset = fn; return 1; };
  globalThis.clearTimeout = () => {};
  try {
    await copyCodeBlockFromButton(resetButton, { labels: LABELS, writeText: () => Promise.resolve(true) });
    resetButton.remove();
    pendingReset();
  } finally {
    globalThis.setTimeout = realSetTimeout;
    globalThis.clearTimeout = realClearTimeout;
  }
}

// 英文兜底:仅防御不可达路径(三个接入点均传完整词条,三语存在性由文件头钉住);
// 此处钉住兜底行为本身,词条缺失时不渲染出 "undefined"。
{
  const doc = documentFromMarkdown('```js\nlet f = 1;\n```');
  ensureCodeCopyButtons(doc, {});
  const button = doc.querySelector(`button.${CODE_COPY_BUTTON_CLASS}`);
  assert.equal(button.getAttribute('title'), 'Copy code');
}

// JSX 接入点源码钉:三个视图的装按钮/自愈/点击委托/文案透传形状回归时此处变红
// (纯 DOM 单测覆盖不到 React 层,与 assistant_message_actions.test.mjs 的源码钉同一模式)。
{
  const timeline = source('features/conversation/ConversationTimeline.jsx');
  assert.match(timeline, /ensureCodeCopyButtons\(containerRef\.current, copyLabels\)/);
  assert.match(timeline, /return observeCodeCopyButtons\(containerRef\.current, copyLabels\)/);
  assert.match(timeline, /const copyButton = findCodeCopyButton\(event\.target\);\s*\n\s*if \(copyButton\) \{\s*\n\s*event\.preventDefault\(\);\s*\n\s*copyCodeBlockFromButton\(copyButton, \{ labels: copyLabels \}\)/);
  const copyProps = timeline.match(/streaming=\{item\.status === 'in_progress'\}\s*copy=\{copy\}/g) || [];
  assert.equal(copyProps.length, 2, 'both ConversationMarkdown branches (commentary and main) must pass copy');
  assert.match(timeline, /\}, \[html, copyLabels\]\);/, 'the install effect must depend on copyLabels for label refresh');
  // 容器 div 的附件本身:ref/onClick 丢掉时 effect 内部与 onClick 函数体仍然完好,
  // 只有钉住挂接点才能拦住这种整体静默失效。
  assert.match(timeline, /ref=\{containerRef\}/, 'the container div must wire the ref used by the install effect');
  assert.match(timeline, /onClick=\{onClick\}\s*\n\s*dangerouslySetInnerHTML/, 'the container div must wire the delegated click handler');
  const chatView = source('features/chat/ChatView.jsx');
  assert.match(chatView, /const target = assistantSelectionTargetRef\.current;\s*\n\s*if \(!target\) return;\s*\n\s*const labels = codeBlockCopyLabels\(t\.uiConversation\);\s*\n\s*ensureCodeCopyButtons\(target, labels\);\s*\n\s*return observeCodeCopyButtons\(target, labels\);/);
  assert.match(chatView, /const copyButton = findCodeCopyButton\(e\.target\);/);
  assert.match(chatView, /copyCodeBlockFromButton\(copyButton, \{ labels: codeBlockCopyLabels\(t\.uiConversation\) \}\)/);
  assert.match(chatView, /ref=\{assistantSelectionTargetRef\}/, 'the legacy bubble div must wire the ref used by its install effect');
  const codexView = source('features/codex/CodexAcpView.jsx');
  // tempered 匹配不越出 <ConversationMarkdown .../> 自闭合范围:copy prop 换行重排
  // 不误报,删掉透传即红。
  assert.match(codexView, /<ConversationMarkdown\b(?:(?!\/>)[\s\S])*copy=\{t\.uiConversation\}/);
}

console.log('conversation_code_block_copy: all assertions passed');
