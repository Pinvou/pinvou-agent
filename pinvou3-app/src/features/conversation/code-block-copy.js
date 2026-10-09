// 对话 markdown 代码块(pre.pinvou-code-block)的一键复制按钮。
// ConversationMarkdown 用 dangerouslySetInnerHTML 注入渲染结果,按钮只能在渲染后
// 挂到 DOM 上;本模块集中按钮创建、点击识别、取文与复制反馈,逻辑与 React 解耦以便单测。
import { copyClipboardText } from '../../shared/clipboard.js';

export const CODE_COPY_BUTTON_CLASS = 'pinvou-code-copy-btn';
export const CODE_COPY_BLOCK_SELECTOR = 'pre.pinvou-code-block';
// 复制反馈的屏幕阅读器通道:title/aria-label 变更不会被 NVDA/JAWS 主动播报,
// 反馈文案写入按钮旁这个 polite live region,复位时清空(与 AssistantMessageActions
// 的 aria-live 反馈同模式)。
export const CODE_COPY_LIVE_CLASS = 'pinvou-code-copy-live';

// uiConversation 词条表 → 按钮文案;成功/失败反馈复用整条回复复制的既有词条,
// 不新增重复 key。
export function codeBlockCopyLabels(copy) {
  const table = copy && typeof copy === 'object' ? copy : {};
  return {
    copyCode: table.copyCode,
    copied: table.copyReplySuccess,
    failed: table.copyReplyFailed,
  };
}

const FEEDBACK_MS = 1600;

// 安装标记:JS expando 属性而非 data-* attribute——HTML 解析只能产生 attribute,
// 产生不了 JS 属性,据此把本模块安装的按钮/live region 与消息原生 HTML 里伪造的
// 同类名元素区分开:动作通道由 findCodeCopyButton/findLiveRegion 验标记,外观通道
// 由 ensureCodeCopyButtons 摘除伪按钮的本模块类名(见下)。
const CODE_COPY_BUTTON_FLAG = '__pinvouCodeCopy';
const CODE_COPY_LIVE_FLAG = '__pinvouCodeCopyLive';

// 与 components/icons.jsx 的 Copy/Check/X 同一份 lucide 路径;按钮是渲染后注入的原生
// DOM(非 React 组件),只能内联 SVG 字符串,改动图标时两边同步。
const COPY_ICON_SVG = '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>';
const CHECK_ICON_SVG = '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 6 9 17l-5-5"/></svg>';
const X_ICON_SVG = '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>';

// 反馈复位定时器按按钮登记,重复点击先清旧定时器,避免早先的复位把新一轮反馈刷掉。
const feedbackTimers = new WeakMap();

function setButtonLabel(button, label) {
  button.setAttribute('title', label);
  button.setAttribute('aria-label', label);
}

// 给容器内所有尚未装按钮的代码块补按钮;流式渲染每次 throttle 都整体替换 HTML,
// 因此本函数必须幂等(已装过的块跳过),返回新装数量。已装块顺带同步 title/aria-label:
// html 不随语言切换重建,历史消息的按钮提示文案只能在这里换;反馈态(is-copied/
// is-failed)期间 title 是反馈文案,跳过以免刷掉。伪造元素(无安装标记,不限标签——
// CSS 类名选择器不限标签,伪 div/span 同样算)全量摘掉本模块类名:伪元素回落为消毒
// HTML 既有的普通渲染,防伪同时覆盖动作与外观两个通道,也不再抑制真按钮。归属按
// 最近代码块判定:嵌套 pre 时外层只认自己直接挂的按钮/live region,不被后代满足
// (否则外层缺件不自愈、文案刷新落错对象)。live region 缺件(被单独删除)时补装。
export function ensureCodeCopyButtons(container, labels = {}) {
  if (!container || typeof container.querySelectorAll !== 'function') return 0;
  let installed = 0;
  for (const pre of container.querySelectorAll(CODE_COPY_BLOCK_SELECTOR)) {
    const owns = (node) => node.closest(CODE_COPY_BLOCK_SELECTOR) === pre;
    let genuine = null;
    for (const candidate of pre.querySelectorAll(`.${CODE_COPY_BUTTON_CLASS}`)) {
      if (candidate[CODE_COPY_BUTTON_FLAG] === true) {
        if (!genuine && owns(candidate)) genuine = candidate;
        continue;
      }
      candidate.classList.remove(CODE_COPY_BUTTON_CLASS, 'is-copied', 'is-failed');
    }
    for (const span of pre.querySelectorAll(`.${CODE_COPY_LIVE_CLASS}`)) {
      if (span[CODE_COPY_LIVE_FLAG] !== true) span.classList.remove(CODE_COPY_LIVE_CLASS);
    }
    const live = findLiveRegion(pre);
    if (genuine) {
      if (!genuine.classList.contains('is-copied')
        && !genuine.classList.contains('is-failed')) {
        setButtonLabel(genuine, labels.copyCode || 'Copy code');
      }
      if (!live) installLiveRegion(pre);
      continue;
    }
    const doc = pre.ownerDocument;
    if (!doc) continue;
    const button = doc.createElement('button');
    button.setAttribute('type', 'button');
    button.setAttribute('class', CODE_COPY_BUTTON_CLASS);
    button[CODE_COPY_BUTTON_FLAG] = true;
    setButtonLabel(button, labels.copyCode || 'Copy code');
    button.innerHTML = COPY_ICON_SVG;
    // eslint-disable-next-line unicorn/prefer-dom-node-append -- 单测用 domino 建 DOM,它没有 ParentNode#append(同 message-clipboard.js 的 domino 豁免)
    pre.appendChild(button);
    if (!live) installLiveRegion(pre);
    installed += 1;
  }
  return installed;
}

// 安装 live region:与按钮同模式的 expando 归属标记,反馈写入只认本模块装的。
function installLiveRegion(pre) {
  const doc = pre.ownerDocument;
  if (!doc) return null;
  const live = doc.createElement('span');
  live.setAttribute('class', CODE_COPY_LIVE_CLASS);
  live.setAttribute('aria-live', 'polite');
  live[CODE_COPY_LIVE_FLAG] = true;
  // eslint-disable-next-line unicorn/prefer-dom-node-append -- 单测用 domino 建 DOM,它没有 ParentNode#append(同 message-clipboard.js 的 domino 豁免)
  pre.appendChild(live);
  return live;
}

// 块内定位本模块安装的 live region(按 expando 标记+归属:嵌套 pre 时外层只认
// 自己直接挂的,忽略伪造同名 span 与后代块的区域)。
function findLiveRegion(pre) {
  if (!pre || typeof pre.querySelectorAll !== 'function') return null;
  for (const span of pre.querySelectorAll(`.${CODE_COPY_LIVE_CLASS}`)) {
    if (span[CODE_COPY_LIVE_FLAG] === true
      && span.closest(CODE_COPY_BLOCK_SELECTOR) === pre) return span;
  }
  return null;
}

// 事件委托入口:从点击目标向上找复制按钮(命中图标 SVG 也能找到)。带安装标记才
// 认账:消息原生 HTML 伪造的同类名 button 经解析不带 JS expando,点击识别为 null,
// 不会误触发复制。
export function findCodeCopyButton(target) {
  if (!target || typeof target.closest !== 'function') return null;
  const button = target.closest(`button.${CODE_COPY_BUTTON_CLASS}`);
  return button && button[CODE_COPY_BUTTON_FLAG] === true ? button : null;
}

// 容器内容的结构变化(innerHTML 整体替换、流式增量等)会静默销毁已装按钮,
// 而 React 的 innerHTML diff 与 effect 依赖并不总能覆盖这类替换(实测冷启动时
// 按钮被替换掉且 effect 不再触发)。挂 MutationObserver 自愈:任何 childList
// 变化后幂等补装(ensure 对已有按钮的块零操作,不会自激)。返回断开函数;
// 无 MutationObserver 的环境(Node/domino 单测)返回空操作。
export function observeCodeCopyButtons(container, labels = {}) {
  if (!container || typeof MutationObserver !== 'function') return () => {};
  let queued = false;
  const observer = new MutationObserver(() => {
    // 同一次 innerHTML 替换产生一批记录,合并到一次补装;回调本身是微任务,
    // 这里只需防止补装引发的自身记录重复排队(补装是无操作时不再排队)。
    if (queued) return;
    queued = true;
    queueMicrotask(() => {
      queued = false;
      // 不检查 isConnected:离线容器同样可能被替换内容,补装进离线节点无害。
      ensureCodeCopyButtons(container, labels);
    });
  });
  observer.observe(container, { childList: true, subtree: true });
  return () => observer.disconnect();
}

// 复制内容取 code 子元素的纯文本:按钮自身挂在 pre 里,直接读 pre.textContent
// 会混入按钮文案;高亮 span 的 textContent 拼接结果与源码一致。
export function codeBlockTextFromButton(button) {
  const pre = button && typeof button.closest === 'function'
    ? button.closest(CODE_COPY_BLOCK_SELECTOR)
    : null;
  const code = pre ? pre.querySelector('code') : null;
  return code ? String(code.textContent || '') : '';
}

export async function copyCodeBlockFromButton(button, options = {}) {
  const labels = options.labels || {};
  const writeText = options.writeText || copyClipboardText;
  const text = codeBlockTextFromButton(button);
  // 空代码块(code 无文本)不写剪贴板、如实按失败反馈:与 copyClipboardText 对空
  // 文本返回 false 的既有语义一致(整条回复复制对空文本同样视为不可复制)。
  let copied = false;
  if (text) {
    try {
      // Promise.resolve 归一非 Promise 返回值;catch 兜 reject,外层 try 兜同步 throw。
      copied = await Promise.resolve(writeText(text)).catch(() => false);
    } catch {
      copied = false;
    }
  }
  // 已知限制:流式期间 writeText 异步落定前按钮可能被整体 innerHTML 替换销毁,
  // 反馈将作用在游离节点上不可见(复制本身已成功);复位路径对游离节点幂等无害。
  showCopyFeedback(button, Boolean(copied), labels);
  return Boolean(copied);
}

// 反馈文案写入按钮所在代码块的 live region;按 expando 标记定位本模块安装的
// (伪造同名 span 不劫持),游离/缺失时静默跳过。
function setLiveFeedback(button, text) {
  const pre = button && typeof button.closest === 'function'
    ? button.closest(CODE_COPY_BLOCK_SELECTOR)
    : null;
  const live = findLiveRegion(pre);
  if (live) live.textContent = text;
}

function showCopyFeedback(button, copied, labels) {
  if (!button || typeof button.setAttribute !== 'function') return;
  setButtonLabel(button, copied
    ? (labels.copied || 'Copied')
    : (labels.failed || 'Copy failed'));
  button.classList.remove('is-copied', 'is-failed');
  button.classList.add(copied ? 'is-copied' : 'is-failed');
  // 成功/失败都置换图标:失败若仅靠变红区分不满足非颜色冗余表达(WCAG 1.4.1),
  // 与 AssistantMessageActions 失败态换 X 图标的惯例一致。
  button.innerHTML = copied ? CHECK_ICON_SVG : X_ICON_SVG;
  setLiveFeedback(button, copied
    ? (labels.copied || 'Copied')
    : (labels.failed || 'Copy failed'));
  const previous = feedbackTimers.get(button);
  if (previous) clearTimeout(previous);
  const timer = setTimeout(() => {
    feedbackTimers.delete(button);
    // 复位时按钮可能已随流式重渲染脱离文档,以下操作均幂等。
    button.classList.remove('is-copied', 'is-failed');
    button.innerHTML = COPY_ICON_SVG;
    setButtonLabel(button, labels.copyCode || 'Copy code');
    setLiveFeedback(button, '');
  }, FEEDBACK_MS);
  feedbackTimers.set(button, timer);
  // Node 单测里不让反馈定时器拖住进程退出。
  if (typeof timer.unref === 'function') timer.unref();
}
