// pinvou3-app 前端入口 — 4-view 壳版
//
// 模块：
//   navigation        · 左侧 sidebar 切 view
//   theme/settings    · 加载 ~/.pinvou3/settings.json + 应用主题/language
//   monitor           · 5s 拉 get_monitor_snapshot 渲染卡片
//   backend status    · 10s 拉 get_backend_status 更新 ChatRoom 顶部 live dot
//   chat              · 现有流式 + 工具卡片渲染（与之前一致）
//
// 协议：Tauri command/event。后端见 src-tauri/src/commands.rs。
// withGlobalTauri=true → window.__TAURI__ 全局对象，无构建步骤。

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// ── DOM ────────────────────────────────────────────────────────────
const chatArea = document.getElementById("chat-area");
const input = document.getElementById("input");
const sendBtn = document.getElementById("send-btn");
const clearBtn = document.getElementById("clear-btn");
const liveDot = document.getElementById("live-dot");
const liveText = document.getElementById("live-text");
const monitorLiveDot = document.getElementById("monitor-live-dot");
const monitorUpdated = document.getElementById("monitor-updated");
const monitorRefreshBtn = document.getElementById("monitor-refresh-btn");

// ── 状态 ───────────────────────────────────────────────────────────
let currentAssistantBubble = null;
let currentAssistantRawText = "";
const toolCards = new Map();
let busy = false;
let currentView = "chatroom";
let currentPrefs = null;

// ── i18n 字典（极简版，DOM 扫描 data-i18n / data-i18n-placeholder） ──
const I18N = {
  "zh-Hans": {
    "app.title": "pinvou3 智能助手",
    "brand.sub": "本地 · GB10",
    "nav.chatroom": "聊天室",
    "nav.workflow": "工作流",
    "nav.monitor": "监控",
    "nav.settings": "设置",
    "view.chatroom.title": "聊天室",
    "view.chatroom.sub": "跟 Qwen3.6 聊点什么",
    "view.workflow.title": "工作流",
    "view.workflow.sub": "任务拆解 · 进度可视化",
    "view.monitor.title": "监控",
    "view.monitor.sub": "系统性能 · 5 秒刷新",
    "view.settings.title": "设置",
    "view.settings.sub": "外观与语言",
    "chat.clear": "清除",
    "chat.welcome": "跟 Qwen3.6 说点什么。它能联网、读写文件、跑 shell。",
    "chat.placeholder": "跟 Qwen3.6 说点什么(Enter 发送 · Shift+Enter 换行)",
    "chat.cleared_prefix": "对话已清空",
    "chat.cleared_desc": "当前对话已清空(前端)。后端会话历史在下次重启 app 时一并清除。",
    "live.online": "在线",
    "live.offline": "离线",
    "live.checking": "检查中",
    "live.err": "错误",
    "monitor.refresh": "刷新",
    "monitor.vram": "VRAM",
    "monitor.util": "利用率",
    "monitor.memory": "内存",
    "monitor.system_ram": "系统内存",
    "monitor.used": "已用",
    "monitor.swap": "Swap",
    "monitor.status": "状态",
    "monitor.upstream": "上游",
    "monitor.queue": "运行 / 等待",
    "monitor.app": "应用",
    "monitor.app_version": "应用版本",
    "monitor.session": "会话运行",
    "monitor.updated_prefix": "更新: ",
    "monitor.gpu_unavail": "GPU 信息不可用(无 nvidia-smi)",
    "settings.theme.title": "界面风格",
    "settings.language.title": "界面语言",
    "settings.language.subtitle": "切换界面文案 · 同步通知 LLM 改用对应语言回复(重启 app 生效)",
    "settings.foot_note_prefix": "高级参数(max tokens / allow shell / 模型预设 等)需要手动修改",
    "settings.foot_note_suffix": "的 advanced 字段,重启 app 生效。",
    "theme.default_badge": "默认",
    "theme.genesis.short": "创始之境",
    "theme.genesis.name": "创始之境 · Genesis",
    "theme.genesis.desc": "暗色实验室美学,橙色 accent,Terminal 质感。",
    "theme.liquid_light.short": "澄境",
    "theme.liquid_light.name": "澄境 · 浅色",
    "theme.liquid_light.desc": "白底柔和阴影,适合明亮环境。",
    "theme.liquid_dark.short": "澄境",
    "theme.liquid_dark.name": "澄境 · 深色",
    "theme.liquid_dark.desc": "纯黑底蓝色点缀,低光环境护眼。",
    "workflow.empty.title": "工作流功能开发中",
    "workflow.empty.desc": "未来在这里能看到 AI 把你的需求拆成步骤、并行/串行执行,每步状态实时跟踪。",
    "workflow.empty.hint": "现在请在聊天室提需求。",
  },
  "en": {
    "app.title": "pinvou3 Assistant",
    "brand.sub": "Local · GB10",
    "nav.chatroom": "ChatRoom",
    "nav.workflow": "WorkFlow",
    "nav.monitor": "Monitor",
    "nav.settings": "Settings",
    "view.chatroom.title": "ChatRoom",
    "view.chatroom.sub": "Talk to Qwen3.6",
    "view.workflow.title": "WorkFlow",
    "view.workflow.sub": "Task breakdown · progress",
    "view.monitor.title": "Monitor",
    "view.monitor.sub": "System performance · 5s refresh",
    "view.settings.title": "Settings",
    "view.settings.sub": "Appearance & language",
    "chat.clear": "Clear",
    "chat.welcome": "Say something to Qwen3.6. It can browse the web, read/write files, and run shell commands.",
    "chat.placeholder": "Type a message (Enter to send · Shift+Enter for newline)",
    "chat.cleared_prefix": "Conversation cleared",
    "chat.cleared_desc": "Frontend cleared. Backend session history clears on next app restart.",
    "live.online": "ONLINE",
    "live.offline": "OFFLINE",
    "live.checking": "CHECKING",
    "live.err": "ERROR",
    "monitor.refresh": "Refresh",
    "monitor.vram": "VRAM",
    "monitor.util": "Utilization",
    "monitor.memory": "Memory",
    "monitor.system_ram": "System RAM",
    "monitor.used": "Used",
    "monitor.swap": "Swap",
    "monitor.status": "Status",
    "monitor.upstream": "Upstream",
    "monitor.queue": "Running / Waiting",
    "monitor.app": "App",
    "monitor.app_version": "App version",
    "monitor.session": "Session uptime",
    "monitor.updated_prefix": "Updated: ",
    "monitor.gpu_unavail": "GPU info unavailable (no nvidia-smi)",
    "settings.theme.title": "Theme",
    "settings.language.title": "Language",
    "settings.language.subtitle": "Switch UI language · also tells the LLM to reply in that language (restart app to take effect)",
    "settings.foot_note_prefix": "Advanced parameters (max tokens / allow shell / model preset etc.) require manually editing",
    "settings.foot_note_suffix": "and restarting the app.",
    "theme.default_badge": "Default",
    "theme.genesis.short": "Genesis",
    "theme.genesis.name": "Genesis",
    "theme.genesis.desc": "Dark lab aesthetic, orange accent, terminal feel.",
    "theme.liquid_light.short": "Liquid",
    "theme.liquid_light.name": "Liquid · Light",
    "theme.liquid_light.desc": "White background with soft shadows, suited for bright environments.",
    "theme.liquid_dark.short": "Liquid",
    "theme.liquid_dark.name": "Liquid · Dark",
    "theme.liquid_dark.desc": "Pure black with blue accents, easy on the eyes in low light.",
    "workflow.empty.title": "Workflow feature in development",
    "workflow.empty.desc": "Future: see AI break your request into steps, executed sequentially or in parallel, with real-time status.",
    "workflow.empty.hint": "For now, ask in ChatRoom.",
  },
};

function applyI18n(lang) {
  const dict = I18N[lang] || I18N["zh-Hans"];
  document.querySelectorAll("[data-i18n]").forEach((el) => {
    const v = dict[el.dataset.i18n];
    if (v != null) el.textContent = v;
  });
  document.querySelectorAll("[data-i18n-placeholder]").forEach((el) => {
    const v = dict[el.dataset.i18nPlaceholder];
    if (v != null) el.placeholder = v;
  });
  document.documentElement.lang = lang === "en" ? "en" : "zh-CN";
}

// ── marked / DOMPurify 配置 ────────────────────────────────────────
if (window.marked) {
  marked.setOptions({ gfm: true, breaks: true, headerIds: false, mangle: false });
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

// ── Navigation ─────────────────────────────────────────────────────
function switchView(name) {
  currentView = name;
  document.querySelectorAll(".view").forEach((el) => {
    el.classList.toggle("view-active", el.id === `view-${name}`);
  });
  document.querySelectorAll(".nav-link[data-view]").forEach((el) => {
    el.classList.toggle("active", el.dataset.view === name);
  });
}
document.querySelectorAll(".nav-link[data-view]").forEach((el) => {
  el.addEventListener("click", (e) => {
    e.preventDefault();
    switchView(el.dataset.view);
  });
});

// ── Settings 加载 / 应用 / 保存 ────────────────────────────────────
function applyTheme(theme) {
  document.body.dataset.theme = theme || "genesis";
  document.querySelectorAll(".theme-card").forEach((c) => {
    c.classList.toggle("selected", c.dataset.theme === theme);
  });
}
function applyLanguage(lang) {
  document.querySelectorAll(".seg-btn[data-lang]").forEach((b) => {
    b.classList.toggle("active", b.dataset.lang === lang);
  });
  applyI18n(lang);
  // live dot 文案是动态计算的，重渲染一次
  refreshLiveDotText();
}
async function loadSettings() {
  try {
    currentPrefs = await invoke("get_settings");
  } catch (e) {
    console.warn("get_settings failed", e);
    currentPrefs = { theme: "genesis", color_scheme: "system", language: "zh-Hans" };
  }
  applyTheme(currentPrefs.theme);
  applyLanguage(currentPrefs.language);
}
async function saveSettings() {
  if (!currentPrefs) return;
  try {
    await invoke("update_settings", { prefs: currentPrefs });
  } catch (e) {
    appendSystemMessage("⚠️ 保存设置失败: " + e);
  }
}
// 主题点击
document.querySelectorAll(".theme-card").forEach((card) => {
  card.addEventListener("click", () => {
    const t = card.dataset.theme;
    if (!currentPrefs) return;
    currentPrefs.theme = t;
    applyTheme(t);
    saveSettings();
  });
});
// 语言切换
document.querySelectorAll(".seg-btn[data-lang]").forEach((b) => {
  b.addEventListener("click", () => {
    if (!currentPrefs) return;
    currentPrefs.language = b.dataset.lang;
    applyLanguage(b.dataset.lang);
    saveSettings();
  });
});

// ── Monitor ────────────────────────────────────────────────────────
function fmtMiB(mib) {
  if (mib == null) return "—";
  if (mib >= 1024) return (mib / 1024).toFixed(1) + " GiB";
  return mib + " MiB";
}
function fmtKiB(kib) {
  if (kib == null) return "—";
  if (kib >= 1024 * 1024) return (kib / 1024 / 1024).toFixed(1) + " GiB";
  if (kib >= 1024) return (kib / 1024).toFixed(0) + " MiB";
  return kib + " KiB";
}
function fmtDuration(secs) {
  if (secs == null || secs < 0) return "—";
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}
function fmtTime(ms) {
  if (!ms) return "—";
  const d = new Date(ms);
  return d.toLocaleTimeString();
}

function renderMonitor(snap) {
  // GPU
  if (snap.gpu) {
    document.getElementById("gpu-name").textContent = snap.gpu.name;
    const vramPct = snap.gpu.vram_total_mib > 0
      ? Math.round((snap.gpu.vram_used_mib / snap.gpu.vram_total_mib) * 100)
      : 0;
    document.getElementById("gpu-vram-bar").style.width = vramPct + "%";
    document.getElementById("gpu-vram-text").textContent =
      `${fmtMiB(snap.gpu.vram_used_mib)} / ${fmtMiB(snap.gpu.vram_total_mib)}`;
    document.getElementById("gpu-util-bar").style.width = snap.gpu.utilization_pct + "%";
    document.getElementById("gpu-util-text").textContent = snap.gpu.utilization_pct + "%";
    document.getElementById("card-gpu").classList.remove("card-unavail");
  } else {
    document.getElementById("gpu-name").textContent = i18nText("monitor.gpu_unavail");
    document.getElementById("card-gpu").classList.add("card-unavail");
  }

  // RAM
  if (snap.ram) {
    const usedPct = snap.ram.total_kib > 0
      ? Math.round((snap.ram.used_kib / snap.ram.total_kib) * 100)
      : 0;
    document.getElementById("ram-used-bar").style.width = usedPct + "%";
    document.getElementById("ram-used-text").textContent =
      `${fmtKiB(snap.ram.used_kib)} / ${fmtKiB(snap.ram.total_kib)}`;
    const swapPct = snap.ram.swap_total_kib > 0
      ? Math.round((snap.ram.swap_used_kib / snap.ram.swap_total_kib) * 100)
      : 0;
    document.getElementById("swap-used-bar").style.width = swapPct + "%";
    document.getElementById("swap-used-text").textContent =
      `${fmtKiB(snap.ram.swap_used_kib)} / ${fmtKiB(snap.ram.swap_total_kib)}`;
  }

  // vLLM
  if (snap.vllm) {
    const v = snap.vllm;
    document.getElementById("vllm-model").textContent = v.model || "(无 model id)";
    const statusEl = document.getElementById("vllm-status");
    statusEl.textContent = v.status.toUpperCase();
    statusEl.className = "card-badge badge-" + v.status;
    document.getElementById("vllm-upstream").textContent = v.upstream || "—";
    document.getElementById("vllm-maxlen").textContent = v.max_model_len || "—";
    document.getElementById("vllm-queue").textContent =
      `${v.num_requests_running ?? "—"} / ${v.num_requests_waiting ?? "—"}`;
    document.getElementById("vllm-kv").textContent =
      v.kv_cache_usage_pct != null ? v.kv_cache_usage_pct.toFixed(1) + "%" : "—";
    monitorLiveDot.className = "live-dot " + (v.status === "offline" ? "offline" : "online");
  } else {
    document.getElementById("vllm-status").textContent = "—";
  }

  // App
  if (snap.app) {
    document.getElementById("app-version").textContent = snap.app.pinvou3_version;
    document.getElementById("dt-version").textContent = snap.app.deepseek_tui_version;
    document.getElementById("session-uptime").textContent =
      fmtDuration(snap.app.session_uptime_secs);
  }

  monitorUpdated.textContent = i18nText("monitor.updated_prefix") + fmtTime(snap.generated_at_ms);
}

async function pollMonitor() {
  try {
    const snap = await invoke("get_monitor_snapshot");
    renderMonitor(snap);
  } catch (e) {
    console.warn("get_monitor_snapshot failed", e);
  }
}
monitorRefreshBtn?.addEventListener("click", () => pollMonitor());

// ── ChatRoom 顶部 live dot ─────────────────────────────────────────
let lastLiveState = "checking"; // "online" / "offline" / "err" / "checking"
function i18nText(key) {
  const lang = currentPrefs?.language || "zh-Hans";
  return (I18N[lang] && I18N[lang][key]) || (I18N["zh-Hans"] && I18N["zh-Hans"][key]) || key;
}
function refreshLiveDotText() {
  const map = { online: "live.online", offline: "live.offline", err: "live.err", checking: "live.checking" };
  liveText.textContent = i18nText(map[lastLiveState] || "live.checking");
}
async function pollBackendStatus() {
  try {
    const s = await invoke("get_backend_status");
    lastLiveState = s.vllm_online ? "online" : "offline";
    liveDot.className = "live-dot " + lastLiveState;
  } catch (e) {
    lastLiveState = "err";
    liveDot.className = "live-dot offline";
  }
  refreshLiveDotText();
}

// ── Chat 消息渲染（与之前一致） ────────────────────────────────────
function setBusy(b) {
  busy = b;
  sendBtn.disabled = b;
  input.disabled = b;
}
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
function appendAssistantDelta(text) {
  if (!currentAssistantBubble) beginAssistantBubble();
  currentAssistantRawText += text;
  currentAssistantBubble.innerHTML = renderMarkdown(currentAssistantRawText);
  scrollToBottom();
}
function closeAssistantBubble() {
  currentAssistantBubble = null;
  currentAssistantRawText = "";
}

// 工具卡片（保留原逻辑）
const ARG_PRIMARY_FIELDS = ["code", "command", "query", "path", "content", "text", "url"];
const OUT_PRIMARY_FIELDS = ["stdout", "output", "result", "content", "text", "summary", "note", "message", "error"];
const SELF_EXPLANATORY = new Set(["code", "command", "stdout", "output", "content", "text"]);

function smartExtract(value, primaryFields) {
  let parsed = value;
  if (typeof value === "string") {
    try { parsed = JSON.parse(value); } catch { return { fieldName: null, text: value }; }
  }
  if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
    for (const k of primaryFields) {
      if (typeof parsed[k] === "string" && parsed[k].length > 0) {
        const inner = parsed[k];
        try {
          const reparsed = JSON.parse(inner);
          for (const k2 of primaryFields) {
            if (typeof reparsed[k2] === "string") return { fieldName: k2, text: reparsed[k2] };
          }
        } catch { /* nope */ }
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
function renderToolText(el, value, primaryFields) {
  const { fieldName, text } = pretty(value, primaryFields);
  if (fieldName && !SELF_EXPLANATORY.has(fieldName)) {
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
  closeAssistantBubble();
  return card;
}
function appendToolCallEnd(id, output, success) {
  const card = toolCards.get(id);
  if (!card) return;
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
function scrollToBottom() {
  requestAnimationFrame(() => {
    chatArea.scrollTop = chatArea.scrollHeight;
  });
}

// 清除按钮：清前端 + 后端 placeholder
clearBtn?.addEventListener("click", async () => {
  const prefix = i18nText("chat.cleared_prefix");
  const desc = i18nText("chat.cleared_desc");
  chatArea.innerHTML = "";
  const row = document.createElement("div");
  row.className = "msg-row msg-system";
  row.innerHTML = `<div class="bubble bubble-system"><span class="bubble-system-prefix">PINVOU3 · ${prefix.toUpperCase()}</span><br/>${escapeHtml(desc)}</div>`;
  chatArea.appendChild(row);
  toolCards.clear();
  closeAssistantBubble();
  try {
    await invoke("clear_session");
  } catch (e) {
    console.warn("clear_session failed", e);
  }
});

// ── Tauri 事件订阅 ────────────────────────────────────────────────
listen("chat:delta", (e) => {
  appendAssistantDelta(e.payload?.text || "");
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

// ── 初始化 ────────────────────────────────────────────────────────
(async function init() {
  setBusy(false);
  input.focus();
  await loadSettings();
  // 启动两个定时拉取
  pollMonitor();
  pollBackendStatus();
  setInterval(pollMonitor, 5000);
  setInterval(pollBackendStatus, 10000);
})();
