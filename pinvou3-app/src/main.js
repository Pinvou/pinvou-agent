// pinvou3-app 前端入口 — Week 1 骨架版
// 协议：Tauri command/event。后端见 src-tauri/src/commands.rs + events.rs。
//
// 用 window.__TAURI__ 全局对象（tauri.conf.json: app.withGlobalTauri=true），
// 这样前端是纯静态 HTML/JS，无需构建步骤。

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const chatArea = $("chat-area");
const input = $("input");
const sendBtn = $("send-btn");
const statusEl = $("status");

let currentAssistantMsg = null; // 当前正在流式输出的助手消息 element
let busy = false;

// ────────────────────────────────────────────────
// 渲染
// ────────────────────────────────────────────────

function appendMessage(role, text) {
  const wrap = document.createElement("div");
  wrap.className = `msg msg-${role}`;
  const bubble = document.createElement("div");
  bubble.className = "bubble";
  bubble.textContent = text;
  wrap.appendChild(bubble);
  chatArea.appendChild(wrap);
  chatArea.scrollTop = chatArea.scrollHeight;
  return bubble;
}

function appendToolCall(name, args) {
  const card = document.createElement("div");
  card.className = "tool-card tool-running";
  card.innerHTML = `
    <div class="tool-head">
      <span class="tool-icon">🔧</span>
      <span class="tool-name">${escapeHtml(name)}</span>
      <span class="tool-state">运行中…</span>
    </div>
    <div class="tool-args">${escapeHtml(JSON.stringify(args).slice(0, 200))}</div>
  `;
  chatArea.appendChild(card);
  chatArea.scrollTop = chatArea.scrollHeight;
  return card;
}

function finishToolCall(card, output, success) {
  if (!card) return;
  card.className = `tool-card ${success ? "tool-success" : "tool-error"}`;
  const stateEl = card.querySelector(".tool-state");
  if (stateEl) stateEl.textContent = success ? "✓" : "✗";
  const preview = document.createElement("div");
  preview.className = "tool-output";
  preview.textContent = String(output).slice(0, 400);
  card.appendChild(preview);
  chatArea.scrollTop = chatArea.scrollHeight;
}

function setStatus(label, kind) {
  statusEl.textContent = label;
  statusEl.className = `status status-${kind || "idle"}`;
}

function setBusy(b) {
  busy = b;
  sendBtn.disabled = b;
  input.disabled = b;
  setStatus(b ? "运行中…" : "就绪", b ? "busy" : "idle");
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", "\"": "&quot;", "'": "&#39;" }[c]),
  );
}

// ────────────────────────────────────────────────
// 后端事件订阅
// ────────────────────────────────────────────────

const toolCards = new Map(); // tool call_id → DOM 节点

listen("chat:delta", (e) => {
  const text = e.payload?.text || "";
  if (!currentAssistantMsg) {
    currentAssistantMsg = appendMessage("assistant", "");
  }
  currentAssistantMsg.textContent += text;
  chatArea.scrollTop = chatArea.scrollHeight;
});

listen("chat:tool_start", (e) => {
  const { id, name, args } = e.payload || {};
  const card = appendToolCall(name, args);
  toolCards.set(id, card);
});

listen("chat:tool_end", (e) => {
  const { id, output, success } = e.payload || {};
  finishToolCall(toolCards.get(id), output, success);
  toolCards.delete(id);
});

listen("chat:done", (e) => {
  const error = e.payload?.error;
  if (error) {
    appendMessage("system", `⚠️ ${error}`);
  }
  currentAssistantMsg = null;
  setBusy(false);
});

// ────────────────────────────────────────────────
// 发送
// ────────────────────────────────────────────────

async function send() {
  const text = input.value.trim();
  if (!text || busy) return;
  input.value = "";
  appendMessage("user", text);
  setBusy(true);
  try {
    await invoke("chat", { message: text });
  } catch (err) {
    appendMessage("system", `⚠️ ${err}`);
    setBusy(false);
  }
}

sendBtn.addEventListener("click", send);
input.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    send();
  }
});
