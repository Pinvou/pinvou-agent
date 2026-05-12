// pinvou3-app 前端入口 — Week 2 版（B5/B6/B7 合并）
//
// 协议：Tauri command/event。后端见 src-tauri/src/commands.rs + engine.rs。
// withGlobalTauri=true → window.__TAURI__ 全局对象，无构建步骤。
//
// 核心能力：
//   B5  · 套 pinvou2 "创始之境" 主题
//   B6  · 消息流（用户 / 助手 / 系统 三角色）+ marked.js 渲染助手 markdown
//   B7  · 工具卡片正规化（参数折叠、JSON 美化、状态徽章）

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// ── DOM ────────────────────────────────────────────────────────────
const chatArea = document.getElementById("chat-area");
const input = document.getElementById("input");
const sendBtn = document.getElementById("send-btn");
const statusDot = document.getElementById("status-dot");
const statusText = document.getElementById("status-text");

// ── 状态 ───────────────────────────────────────────────────────────
let currentAssistantBubble = null;       // 当前在累积的助手气泡
let currentAssistantRawText = "";        // 助手累积的原始 markdown（用于增量重渲染）
const toolCards = new Map();             // tool call_id → DOM 节点
let busy = false;

// ── marked / DOMPurify 配置 ────────────────────────────────────────
if (window.marked) {
  marked.setOptions({
    gfm: true,
    breaks: true,
    headerIds: false,
    mangle: false,
  });
}

function renderMarkdown(text) {
  if (!window.marked || !window.DOMPurify) return escapeHtml(text);
  return DOMPurify.sanitize(marked.parse(text || ""));
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => (
    { "&": "&amp;", "<": "&lt;", ">": "&gt;", "\"": "&quot;", "'": "&#39;" }[c]
  ));
}

// ── 状态栏 ─────────────────────────────────────────────────────────
function setStatus(label, kind) {
  statusText.textContent = label;
  const k = kind || "ready";
  statusDot.className = "status-dot" + (k !== "ready" ? " " + k : "");
  statusText.className = "status-text" + (k !== "ready" ? " " + k : "");
}

function setBusy(b) {
  busy = b;
  sendBtn.disabled = b;
  input.disabled = b;
  if (b) setStatus("WORKING", "busy");
  else   setStatus("READY", "ready");
}

// ── 消息渲染 ───────────────────────────────────────────────────────
/** 追加用户消息（textContent，不渲染 markdown） */
function appendUserMessage(text) {
  const row = document.createElement("div");
  row.className = "msg-row msg-user";
  const wrap = document.createElement("div");
  wrap.className = "msg-wrap msg-wrap-user";
  const label = document.createElement("div");
  label.className = "speaker-label speaker-user";
  label.textContent = "你";
  const bubble = document.createElement("div");
  bubble.className = "bubble bubble-user";
  bubble.textContent = text;
  wrap.appendChild(label);
  wrap.appendChild(bubble);
  row.appendChild(wrap);
  chatArea.appendChild(row);
  scrollToBottom();
}

/** 追加系统消息（小字 + 警告色） */
function appendSystemMessage(text) {
  const row = document.createElement("div");
  row.className = "msg-row msg-system";
  const bubble = document.createElement("div");
  bubble.className = "bubble bubble-system";
  bubble.textContent = text;
  row.appendChild(bubble);
  chatArea.appendChild(row);
  scrollToBottom();
}

/** 开始一个新的助手气泡（流式累积入口） */
function beginAssistantBubble() {
  const row = document.createElement("div");
  row.className = "msg-row msg-assistant";
  const wrap = document.createElement("div");
  wrap.className = "msg-wrap msg-wrap-assistant";
  const label = document.createElement("div");
  label.className = "speaker-label speaker-assistant";
  label.textContent = "Qwen3.6";
  const bubble = document.createElement("div");
  bubble.className = "bubble bubble-assistant rendered";
  wrap.appendChild(label);
  wrap.appendChild(bubble);
  row.appendChild(wrap);
  chatArea.appendChild(row);
  currentAssistantBubble = bubble;
  currentAssistantRawText = "";
  return bubble;
}

/** 流式增量 token —— 累积到原始字符串后重新 marked 渲染 */
function appendAssistantDelta(text) {
  if (!currentAssistantBubble) beginAssistantBubble();
  currentAssistantRawText += text;
  currentAssistantBubble.innerHTML = renderMarkdown(currentAssistantRawText);
  scrollToBottom();
}

/** 关闭当前助手气泡（下一次 delta 会开新的） */
function closeAssistantBubble() {
  currentAssistantBubble = null;
  currentAssistantRawText = "";
}

// ── 工具卡片 ───────────────────────────────────────────────────────
// 智能提取工具调用的"主字段" —— LLM 往往把核心内容塞在一个固定字段里
// （例如 code_execution 的 args 是 {"code": "..."}，output 是 {"stdout": "..."}）
// 返回 { fieldName, text }，UI 用 fieldName 当标签 + text 当内容
const ARG_PRIMARY_FIELDS = ["code", "command", "query", "path", "content", "text", "url"];
const OUT_PRIMARY_FIELDS = ["stdout", "output", "result", "content", "text", "summary", "note", "message", "error"];

// 多行/自解释字段 → 不显示字段名前缀（"code" 后面跟代码块本身就一目了然）
const SELF_EXPLANATORY = new Set(["code", "command", "stdout", "output", "content", "text"]);

function smartExtract(value, primaryFields) {
  let parsed = value;
  if (typeof value === "string") {
    try { parsed = JSON.parse(value); } catch { return { fieldName: null, text: value }; }
  }
  if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
    for (const k of primaryFields) {
      if (typeof parsed[k] === "string" && parsed[k].length > 0) {
        // 部分工具结果二次嵌套（stdout 又是 JSON 字符串），递归再剥一层
        const inner = parsed[k];
        try {
          const reparsed = JSON.parse(inner);
          for (const k2 of primaryFields) {
            if (typeof reparsed[k2] === "string") return { fieldName: k2, text: reparsed[k2] };
          }
        } catch { /* 不是 JSON，直接用 */ }
        return { fieldName: k, text: inner };
      }
    }
    return { fieldName: null, text: JSON.stringify(parsed, null, 2) };
  }
  return { fieldName: null, text: String(parsed) };
}

function pretty(value, primaryFields, maxLen = 4000) {
  if (value == null) return { fieldName: null, text: "" };
  const r = smartExtract(value, primaryFields || ARG_PRIMARY_FIELDS);
  let text = r.text;
  if (text.length > maxLen) text = text.slice(0, maxLen) + `\n…（截断 ${text.length - maxLen} 字符）`;
  return { fieldName: r.fieldName, text };
}

/** 渲染带"主字段标签"的工具 args/output 文本块 + 可折叠 */
function renderToolText(el, value, primaryFields) {
  const { fieldName, text } = pretty(value, primaryFields);
  if (fieldName && !SELF_EXPLANATORY.has(fieldName)) {
    // 短标签 + 内容：[path] /tmp/foo.csv
    const tag = document.createElement("span");
    tag.className = "tool-field-tag";
    tag.textContent = fieldName;
    const body = document.createElement("span");
    body.textContent = " " + text;
    el.innerHTML = "";
    el.appendChild(tag);
    el.appendChild(body);
  } else {
    el.textContent = text;
  }
  requestAnimationFrame(() => {
    // 用真实内容高度判断：> 4.5 行（line-height 1.5 × 11px × 4.5 ≈ 75px）才折叠
    const lineHeight = parseFloat(getComputedStyle(el).lineHeight) || 16;
    const threshold = lineHeight * 4.5;
    if (el.scrollHeight > threshold + 2) {
      el.classList.add("collapsible");
      const toggle = document.createElement("span");
      toggle.className = "tool-toggle";
      toggle.textContent = "展开 ▾";
      toggle.addEventListener("click", (e) => {
        e.stopPropagation();
        const expanded = el.classList.toggle("expanded");
        toggle.textContent = expanded ? "折叠 ▴" : "展开 ▾";
      });
      el.parentNode.insertBefore(toggle, el.nextSibling);
    }
  });
}

function appendToolCallStart(id, name, args) {
  const card = document.createElement("div");
  card.className = "tool-card tool-running";
  card.dataset.toolId = id;
  // 工具名 → emoji 图标（直观提示工具类型）
  const iconMap = {
    read_file: "📄", write_file: "📝", edit_file: "✏️", list_dir: "📁",
    file_search: "🔎", grep_files: "🔎",
    web_search: "🌐", fetch_url: "🌐", web_run: "🌐",
    exec_shell: "💻", exec_shell_wait: "💻", exec_shell_interact: "💻",
    code_execution: "🐍",
    update_plan: "📋", todo_write: "✅", checklist_write: "✅",
    request_user_input: "💬",
    agent_spawn: "🐋",
  };
  const icon = iconMap[name] || "🔧";
  card.innerHTML = `
    <div class="tool-head">
      <span class="tool-icon">${icon}</span>
      <span class="tool-name"></span>
      <span class="tool-meta"></span>
      <span class="tool-state">RUNNING</span>
    </div>
    <div class="tool-args"></div>
  `;
  card.querySelector(".tool-name").textContent = name || "(unknown)";
  card.querySelector(".tool-meta").textContent = id ? `#${String(id).slice(-6)}` : "";
  const argsEl = card.querySelector(".tool-args");
  renderToolText(argsEl, args, ARG_PRIMARY_FIELDS);
  chatArea.appendChild(card);
  toolCards.set(id, card);
  scrollToBottom();

  // 关闭当前助手气泡：工具调用打断了文本流；下次 delta 开新气泡
  closeAssistantBubble();
  return card;
}

function appendToolCallEnd(id, output, success) {
  const card = toolCards.get(id);
  if (!card) return; // 工具结束但没记录 start（不应发生）
  card.classList.remove("tool-running");
  card.classList.add(success ? "tool-success" : "tool-error");
  const stateEl = card.querySelector(".tool-state");
  if (stateEl) stateEl.textContent = success ? "DONE" : "FAILED";

  const outputEl = document.createElement("div");
  outputEl.className = "tool-output";
  card.appendChild(outputEl);
  renderToolText(outputEl, output, OUT_PRIMARY_FIELDS);

  toolCards.delete(id);
  scrollToBottom();
}

// ── 滚动 ───────────────────────────────────────────────────────────
function scrollToBottom() {
  // 用 rAF 避免布局抖动
  requestAnimationFrame(() => {
    chatArea.scrollTop = chatArea.scrollHeight;
  });
}

// ── Tauri 事件订阅 ────────────────────────────────────────────────
listen("chat:delta", (e) => {
  const text = e.payload?.text || "";
  appendAssistantDelta(text);
});

listen("chat:tool_start", (e) => {
  const { id, name, args } = e.payload || {};
  appendToolCallStart(id, name, args);
});

listen("chat:tool_end", (e) => {
  const { id, output, success } = e.payload || {};
  appendToolCallEnd(id, output, success);
});

listen("chat:done", (e) => {
  const error = e.payload?.error;
  if (error) appendSystemMessage("⚠️ " + error);
  closeAssistantBubble();
  setBusy(false);
});

// ── 发送 ───────────────────────────────────────────────────────────
async function send() {
  const text = input.value.trim();
  if (!text || busy) return;
  input.value = "";
  autoResize();
  appendUserMessage(text);
  setBusy(true);
  try {
    await invoke("chat", { message: text });
  } catch (err) {
    appendSystemMessage("⚠️ " + (err && err.toString ? err.toString() : err));
    setBusy(false);
  }
}

// textarea 自动高度
function autoResize() {
  input.style.height = "auto";
  input.style.height = Math.min(input.scrollHeight, 160) + "px";
}

sendBtn.addEventListener("click", send);
input.addEventListener("input", autoResize);
input.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    send();
  }
});

// 初始状态
setBusy(false);
input.focus();
