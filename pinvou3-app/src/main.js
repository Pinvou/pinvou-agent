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
const dialogOpen = window.__TAURI__.dialog?.open;
const dialogAsk = window.__TAURI__.dialog?.ask;
const getCurrentWebviewWindow = window.__TAURI__.webviewWindow?.getCurrentWebviewWindow;

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

// 阶段 C: 多对话历史 —— 前端是 messages 的 source of truth。
// state.messages 对齐 deepseek-tui Message schema (role + content[]).
// 每次 TurnComplete 调 save_session_messages 落盘。切换 session 时
// load_session 拿回 messages 重渲染。
let activeSessionId = null;
let messages = [];          // 当前 session 的完整消息数组
let pendingAssistantText = ""; // 累积中的 assistant 文本（TurnComplete 时入栈）
let sessionsCache = [];     // 左侧历史列表数据（list_sessions 结果）
let artifacts = [];         // 当前 session 的产物列表（前端跟踪，重启 app 后丢）
const toolMeta = new Map(); // tool_call_id → {name, args}，给 tool_end 拿原始 args 用

// 阶段 C: 输入附件——发送前调 ingest_file 转 md，每条 chip 关联一份 IngestResult
let pendingAttachments = []; // { id, result: IngestResult, status: "parsing"|"ready"|"error" }
let attachSeq = 0;

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
    "history.new": "新对话",
    "history.empty": "暂无历史对话",
    "history.rename": "重命名",
    "history.delete": "删除",
    "history.confirm_delete": "删除这个对话?",
    "history.untitled": "新对话",
    "chat.stop": "停止生成",
    "chat.edit": "编辑",
    "chat.regenerate": "重发",
    "pane.artifacts": "产物",
    "pane.browser": "浏览器",
    "pane.artifacts.empty": "本对话还没有产物",
    "pane.preview.empty": "选择一个产物预览",
    "pane.preview.open_external": "用系统应用打开",
    "pane.preview.unsupported": "这种文件类型只能用系统应用打开",
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
    "history.new": "New chat",
    "history.empty": "No history yet",
    "history.rename": "Rename",
    "history.delete": "Delete",
    "history.confirm_delete": "Delete this chat?",
    "history.untitled": "New chat",
    "chat.stop": "Stop generating",
    "chat.edit": "Edit",
    "chat.regenerate": "Regenerate",
    "pane.artifacts": "Artifacts",
    "pane.browser": "Browser",
    "pane.artifacts.empty": "No artifacts in this conversation yet",
    "pane.preview.empty": "Select an artifact to preview",
    "pane.preview.open_external": "Open with system app",
    "pane.preview.unsupported": "This file type can only be opened with system app",
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
    // 拉到真实 max_model_len 后更新全局值，token 进度条按这个算
    if (typeof v.max_model_len === "number" && v.max_model_len > 0) {
      maxModelLen = v.max_model_len;
      updateTokenBar(lastInputTokens); // 重渲染 bar
    }
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
/**
 * 生成中 vs 待发送：sendBtn 切换图标 + 行为。
 * - busy = true → 红色 ⏹️ stop 按钮，点击 = cancel_generation
 * - busy = false → 普通 ▶ 发送按钮，点击 = send
 * 输入栏 busy 时禁用避免用户继续打字误以为下一条也在发。
 */
function setBusy(b) {
  busy = b;
  input.disabled = b;
  sendBtn.disabled = false; // busy 时仍可点击(变 stop)
  sendBtn.classList.toggle("busy-stop", b);
  sendBtn.title = b ? i18nText("chat.stop") : "";
  updateMessageActions();
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
  updateMessageActions();
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
  updateMessageActions();
}

// ── 阶段 C: 消息 hover 操作按钮（最后一条 user/assistant 才显示） ──
/**
 * 扫描所有 msg-user / msg-assistant 行，
 * 给最后一条 user 加 ✏️ 编辑按钮，给最后一条 assistant 加 🔄 重发按钮，
 * 其他消息的 action 容器隐藏。busy 时全部隐藏。
 */
function updateMessageActions() {
  if (!chatArea) return;
  // 先全部隐藏
  chatArea.querySelectorAll(".msg-actions").forEach((el) => {
    el.style.display = "none";
  });
  if (busy) return;

  const lastUserRow = chatArea.querySelector(".msg-user:last-of-type");
  if (lastUserRow) {
    ensureUserActions(lastUserRow).style.display = "";
  }
  const lastAssistantRow = chatArea.querySelector(".msg-assistant:last-of-type");
  if (lastAssistantRow) {
    ensureAssistantActions(lastAssistantRow).style.display = "";
  }
}

function ensureUserActions(row) {
  let actions = row.querySelector(".msg-actions");
  if (actions) return actions;
  actions = document.createElement("div");
  actions.className = "msg-actions msg-actions-user";
  const editBtn = document.createElement("button");
  editBtn.type = "button";
  editBtn.className = "msg-action-btn";
  editBtn.textContent = "✏️";
  editBtn.title = i18nText("chat.edit");
  editBtn.addEventListener("click", () => startInlineEditUser(row));
  actions.appendChild(editBtn);
  row.querySelector(".msg-wrap").appendChild(actions);
  return actions;
}

function ensureAssistantActions(row) {
  let actions = row.querySelector(".msg-actions");
  if (actions) return actions;
  actions = document.createElement("div");
  actions.className = "msg-actions msg-actions-assistant";
  const regenBtn = document.createElement("button");
  regenBtn.type = "button";
  regenBtn.className = "msg-action-btn";
  regenBtn.textContent = "🔄";
  regenBtn.title = i18nText("chat.regenerate");
  regenBtn.addEventListener("click", () => regenerateLastAssistant());
  actions.appendChild(regenBtn);
  row.querySelector(".msg-wrap").appendChild(actions);
  return actions;
}

/** Inline 编辑最后一条 user 消息：bubble → textarea → 提交触发 edit_last_turn。 */
function startInlineEditUser(row) {
  if (busy) return;
  const bubble = row.querySelector(".bubble-user");
  if (!bubble) return;
  const originalText = bubble.textContent;
  const ta = document.createElement("textarea");
  ta.className = "msg-edit-input";
  ta.value = originalText;
  ta.rows = Math.min(6, Math.max(1, originalText.split("\n").length));
  let done = false;
  function commit() {
    if (done) return;
    done = true;
    const newText = ta.value.trim();
    if (!newText || newText === originalText) {
      ta.replaceWith(bubble);
      return;
    }
    submitEditLastTurn(newText);
  }
  function cancel() {
    if (done) return;
    done = true;
    ta.replaceWith(bubble);
  }
  ta.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      commit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      cancel();
    }
  });
  ta.addEventListener("blur", cancel);
  bubble.replaceWith(ta);
  ta.focus();
  ta.select();
}

/** 触发 edit_last_turn：前端先更新 state.messages（删尾 + 加新 user），
 *  再 invoke 后端发 Op::EditLastTurn。后续 chat:delta + chat:done 跟正常流程一致。 */
async function submitEditLastTurn(newText) {
  if (busy || !activeSessionId) return;
  // 删除 messages 末尾最近的 user 及之后所有
  let cut = -1;
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].role === "user") { cut = i; break; }
  }
  if (cut >= 0) messages.splice(cut);
  messages.push({ role: "user", content: [{ type: "text", text: newText }] });
  // 重渲染对话区（rerenderFromMessages 已经把新 user 渲染出来，不再重复 appendUserMessage）
  rerenderFromMessages();
  setBusy(true);
  pendingAssistantText = "";
  try {
    await invoke("edit_last_turn", { newMessage: newText });
  } catch (err) {
    appendSystemMessage("⚠️ " + (err && err.toString ? err.toString() : err));
    setBusy(false);
  }
}

/** 重发最后一条 user 消息（不修改文本）：等同于 submitEditLastTurn(原文) */
async function regenerateLastAssistant() {
  if (busy) return;
  let lastUserText = null;
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].role === "user") {
      lastUserText = messages[i].content?.find((c) => c.type === "text")?.text;
      break;
    }
  }
  if (!lastUserText) {
    appendSystemMessage("⚠️ 找不到上一条提问,无法重发");
    return;
  }
  await submitEditLastTurn(lastUserText);
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

// 清除按钮：等同于新对话（开个干净的 session）
clearBtn?.addEventListener("click", async () => {
  await createNewSession();
});

// ── 阶段 C: 多对话历史管理 ────────────────────────────────────────

const historyListEl = document.getElementById("history-list");
const newSessionBtn = document.getElementById("new-session-btn");

/** 把 chatArea 清空并显示一条 system welcome bubble。 */
function clearChatDOM() {
  chatArea.innerHTML = "";
  toolCards.clear();
  closeAssistantBubble();
  pendingAssistantText = "";
  const row = document.createElement("div");
  row.className = "msg-row msg-system";
  row.innerHTML = `<div class="bubble bubble-system"><span class="bubble-system-prefix">PINVOU3 · READY</span><br/>${escapeHtml(i18nText("chat.welcome"))}</div>`;
  chatArea.appendChild(row);
}

/** 用 state.messages 重渲染对话区（切换 session 时用）。 */
function rerenderFromMessages() {
  clearChatDOM();
  for (const m of messages) {
    if (m.role === "user") {
      const text = (m.content || [])
        .filter((c) => c.type === "text")
        .map((c) => c.text)
        .join("");
      if (text) appendUserMessage(text);
    } else if (m.role === "assistant") {
      const text = (m.content || [])
        .filter((c) => c.type === "text")
        .map((c) => c.text)
        .join("");
      if (text) {
        beginAssistantBubble();
        currentAssistantRawText = text;
        currentAssistantBubble.innerHTML = renderMarkdown(text);
        closeAssistantBubble();
      }
    }
  }
  scrollToBottom();
}

/** 渲染左侧历史列表（重读 sessionsCache 状态）。 */
function renderHistoryList() {
  if (!historyListEl) return;
  if (sessionsCache.length === 0) {
    historyListEl.innerHTML = `<li class="history-empty">${escapeHtml(i18nText("history.empty"))}</li>`;
    return;
  }
  historyListEl.innerHTML = "";
  for (const meta of sessionsCache) {
    const li = document.createElement("li");
    li.className = "history-item" + (meta.id === activeSessionId ? " active" : "");
    li.dataset.sessionId = meta.id;

    const title = document.createElement("span");
    title.className = "history-item-title";
    title.textContent = meta.title || i18nText("history.untitled");
    title.addEventListener("click", () => switchToSession(meta.id));

    const actions = document.createElement("span");
    actions.className = "history-item-actions";
    const renameBtn = document.createElement("button");
    renameBtn.className = "history-item-action";
    renameBtn.title = i18nText("history.rename");
    renameBtn.textContent = "✏️";
    renameBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      startRenameInline(li, meta);
    });
    const deleteBtn = document.createElement("button");
    deleteBtn.className = "history-item-action";
    deleteBtn.title = i18nText("history.delete");
    deleteBtn.textContent = "🗑";
    deleteBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      confirmAndDelete(meta);
    });
    actions.appendChild(renameBtn);
    actions.appendChild(deleteBtn);

    li.appendChild(title);
    li.appendChild(actions);
    historyListEl.appendChild(li);
  }
}

/** Inline 重命名：把 .history-item-title 换成 input。 */
function startRenameInline(li, meta) {
  const titleEl = li.querySelector(".history-item-title");
  if (!titleEl) return;
  const input = document.createElement("input");
  input.className = "history-item-rename-input";
  input.value = meta.title || "";
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") commit();
    if (e.key === "Escape") cancel();
  });
  input.addEventListener("blur", commit);
  titleEl.replaceWith(input);
  input.focus();
  input.select();

  let done = false;
  async function commit() {
    if (done) return;
    done = true;
    const newTitle = input.value.trim() || i18nText("history.untitled");
    try {
      await invoke("rename_session", { id: meta.id, title: newTitle });
      meta.title = newTitle;
    } catch (e) {
      console.warn("rename_session failed", e);
    }
    renderHistoryList();
  }
  function cancel() {
    if (done) return;
    done = true;
    renderHistoryList();
  }
}

async function confirmAndDelete(meta) {
  // busy 时禁止删除当前对话（避免 chat:done 写到错误 session）
  if (busy && meta.id === activeSessionId) {
    appendSystemMessage("⚠️ 当前对话正在响应,请等完成后再删除");
    return;
  }
  // 删除"唯一的空对话"无意义——后端删完前端又立刻新建一个空的，用户看不出变化
  const isEmpty = (meta.message_count || 0) === 0;
  const isOnly = sessionsCache.length === 1;
  if (isEmpty && isOnly) {
    appendSystemMessage("⚠️ 这是唯一的空对话,直接在这里输入即可");
    return;
  }
  let ok = false;
  const promptText = i18nText("history.confirm_delete") + "\n\n" + (meta.title || meta.id);
  if (typeof dialogAsk === "function") {
    ok = await dialogAsk(promptText, { title: i18nText("history.delete"), kind: "warning" });
  } else {
    ok = confirm(promptText);
  }
  if (!ok) return;
  try {
    await invoke("delete_session", { id: meta.id });
    sessionsCache = sessionsCache.filter((m) => m.id !== meta.id);
    if (activeSessionId === meta.id) {
      // 删的是当前 session：优先切到剩余最新一条；都没了再建空 session
      activeSessionId = null;
      messages = [];
      pendingAssistantText = "";
      if (sessionsCache.length > 0) {
        await switchToSession(sessionsCache[0].id);
      } else {
        await createNewSession();
      }
    } else {
      renderHistoryList();
    }
  } catch (e) {
    appendSystemMessage("⚠️ 删除失败: " + e);
  }
}

/** 启动 / +新对话 按钮：创建新 session 并切换。
 *  当前 active 已经是空对话时不重复创建，避免列表堆一排空对话。 */
async function createNewSession() {
  // 当前 session 是空的 → 复用（focus 输入栏即可，不建新的）
  if (activeSessionId && messages.length === 0) {
    input.focus();
    return;
  }
  if (busy) {
    appendSystemMessage("⚠️ 当前对话正在响应,请等完成后再新建对话");
    return;
  }
  try {
    const meta = await invoke("create_session");
    activeSessionId = meta.id;
    messages = [];
    pendingAssistantText = "";
    artifacts = [];
    activeArtifactPath = null;
    renderArtifactList();
    clearUnreadArtifacts();
    lastInputTokens = 0;
    updateTokenBar(0);
    if (artifactPreviewEl) artifactPreviewEl.innerHTML = "";
    clearChatDOM();
    await refreshHistoryList();
  } catch (e) {
    appendSystemMessage("⚠️ 新建对话失败: " + e);
  }
}

/** 切换到指定 session：加载 messages 并重渲染。
 *  busy 时弹窗询问是否打断当前生成，确认后 cancel → 等 chat:done → 再切。
 *  等 done 是为了让正在累积的 pendingAssistantText 先写回旧 session.messages,
 *  避免写到新 session 头上。 */
async function switchToSession(id) {
  if (id === activeSessionId) return;
  if (busy) {
    let ok = false;
    if (typeof dialogAsk === "function") {
      ok = await dialogAsk("当前对话还在响应,打断并切换?", {
        title: "切换对话",
        kind: "warning",
      });
    } else {
      ok = confirm("当前对话还在响应,打断并切换?");
    }
    if (!ok) return;
    try {
      const done = waitForChatDone();
      await invoke("cancel_generation");
      // 等上游真的 emit TurnComplete（最多 2s 兜底，避免 cancel 失败永久卡住）
      await Promise.race([
        done,
        new Promise((resolve) => setTimeout(resolve, 2000)),
      ]);
    } catch (e) {
      appendSystemMessage("⚠️ 打断失败,切换取消: " + e);
      return;
    }
  }
  try {
    const saved = await invoke("load_session", { id });
    activeSessionId = saved.metadata.id;
    messages = Array.isArray(saved.messages) ? saved.messages : [];
    pendingAssistantText = "";
    // 产物列表是前端跟踪的，切 session 时清空（不从 messages 重建——messages 不含 tool blocks）
    artifacts = [];
    activeArtifactPath = null;
    renderArtifactList();
    clearUnreadArtifacts();
    lastInputTokens = 0;
    updateTokenBar(0);
    if (artifactPreviewEl) artifactPreviewEl.innerHTML = "";
    rerenderFromMessages();
    renderHistoryList();
  } catch (e) {
    appendSystemMessage("⚠️ 加载对话失败: " + e);
  }
}

/** 拉一次列表 + 重渲染。 */
async function refreshHistoryList() {
  try {
    sessionsCache = await invoke("list_sessions");
  } catch (e) {
    console.warn("list_sessions failed", e);
    sessionsCache = [];
  }
  renderHistoryList();
}

/** 把当前 messages 落盘到后端（每轮 TurnComplete 调用一次）。 */
async function persistMessages() {
  if (!activeSessionId) return;
  try {
    await invoke("save_session_messages", { id: activeSessionId, messages });
    // 标题在前端没有自动生成机制（plan 提到 LLM 总结 6 字内为阶段 D 工作），
    // 但首次发消息后用 user 消息前 20 字做 placeholder
    const meta = sessionsCache.find((m) => m.id === activeSessionId);
    if (meta && (meta.title === "新对话" || meta.title === "New chat")) {
      const firstUser = messages.find((m) => m.role === "user");
      const text = firstUser?.content?.find((c) => c.type === "text")?.text;
      if (text) {
        const newTitle = text.slice(0, 20);
        await invoke("rename_session", { id: activeSessionId, title: newTitle });
        meta.title = newTitle;
        renderHistoryList();
      }
    }
  } catch (e) {
    console.warn("save_session_messages failed", e);
  }
}

newSessionBtn?.addEventListener("click", () => createNewSession());

// ── 阶段 C: 右栏产物面板 ──────────────────────────────────────────

const rightPane = document.getElementById("right-pane");
const rightPaneToggle = document.getElementById("right-pane-toggle");
const rightPaneBadge = document.getElementById("right-pane-badge");
const artifactListEl = document.getElementById("artifact-list");
const artifactPreviewEl = document.getElementById("artifact-preview");
const browserUrlEl = document.getElementById("browser-url");
const browserGoBtn = document.getElementById("browser-go");
const browserFrameEl = document.getElementById("browser-frame");
let activeArtifactPath = null;
let unreadArtifacts = 0;

// 阶段 C: token 进度条 —— 上下文使用率监控
let maxModelLen = 32768;     // 兜底值，monitor 拉到真实 max_model_len 后覆盖
let lastInputTokens = 0;     // 最近一轮 TurnComplete.usage.input_tokens

/** 右栏是否「正在可见地展示产物」——展开 + 产物 tab 激活。 */
function isArtifactsTabVisible() {
  const expanded = rightPane?.dataset.collapsed === "false";
  const activeTab = document.querySelector(".right-pane-tab.active")?.dataset.paneTab;
  return expanded && activeTab === "artifacts";
}

function renderUnreadBadge() {
  if (!rightPaneBadge) return;
  if (unreadArtifacts <= 0) {
    rightPaneBadge.classList.add("hidden");
    rightPaneBadge.textContent = "0";
  } else {
    rightPaneBadge.classList.remove("hidden");
    rightPaneBadge.textContent = unreadArtifacts > 9 ? "9+" : String(unreadArtifacts);
  }
}

function clearUnreadArtifacts() {
  if (unreadArtifacts === 0) return;
  unreadArtifacts = 0;
  renderUnreadBadge();
}

function bumpUnreadArtifacts() {
  if (isArtifactsTabVisible()) return; // 正在看就不算未读
  unreadArtifacts += 1;
  renderUnreadBadge();
}

// ── 阶段 C: token 进度条 ────────────────────────────────────────
const tokenBarEl = document.getElementById("token-bar");
const tokenBarFillEl = document.getElementById("token-bar-fill");

/** 按 used/maxModelLen 比例渲染进度条 + 颜色级别 + tooltip。 */
function updateTokenBar(used) {
  if (!tokenBarEl || !tokenBarFillEl) return;
  const max = maxModelLen || 32768;
  const pct = Math.min(100, Math.round((used / max) * 100));
  tokenBarFillEl.style.width = pct + "%";
  let level = "green";
  if (pct >= 80) level = "red";
  else if (pct >= 60) level = "yellow";
  tokenBarEl.dataset.level = level;
  const tip = `上下文: ${(used / 1000).toFixed(1)}k / ${(max / 1000).toFixed(0)}k tokens (${pct}%) — 点击立即压缩`;
  tokenBarEl.title = tip;
}

tokenBarEl?.addEventListener("click", async () => {
  if (busy) {
    appendSystemMessage("⚠️ 当前对话正在响应,请等完成后再压缩");
    return;
  }
  if (lastInputTokens === 0) {
    appendSystemMessage("ℹ️ 还没有可压缩的对话历史");
    return;
  }
  // Tauri webview 的 window.confirm 是 no-op,改用 plugin-dialog 的原生 ask
  let ok = false;
  if (typeof dialogAsk === "function") {
    ok = await dialogAsk(
      "立即压缩当前对话上下文？\n\n早期消息会被摘要替换,无法恢复。",
      { title: "压缩上下文", kind: "warning" }
    );
  } else {
    ok = confirm("立即压缩当前对话上下文？");
  }
  if (!ok) return;
  try {
    await invoke("compact_now");
  } catch (e) {
    appendSystemMessage("⚠️ " + e);
  }
});

/** 从 write_file 的 args 中提取目标 path。
 *  args 形态可能是 JSON 字符串或 object，且字段名可能是 path/file_path 等。 */
function extractArtifactPath(args) {
  if (args == null) return null;
  let obj = args;
  if (typeof args === "string") {
    try { obj = JSON.parse(args); } catch { return null; }
  }
  if (!obj || typeof obj !== "object") return null;
  return obj.path || obj.file_path || obj.target || obj.filename || null;
}

/** 把一个产物路径加入 state.artifacts。重复 path 的覆盖（最近一次写为准）。 */
function trackArtifact(path) {
  // 同路径已存在 → 更新顺序（移到最前），保留之前的 created_at
  const existing = artifacts.findIndex((a) => a.path === path);
  const basename = path.split(/[\\/]/).pop() || path;
  const entry = existing >= 0
    ? { ...artifacts[existing], updated_at: Date.now() }
    : { path, basename, created_at: Date.now(), updated_at: Date.now() };
  if (existing >= 0) artifacts.splice(existing, 1);
  artifacts.unshift(entry);
  renderArtifactList();
  bumpUnreadArtifacts();
}

/** 渲染右栏产物列表。 */
function renderArtifactList() {
  if (!artifactListEl) return;
  if (artifacts.length === 0) {
    artifactListEl.innerHTML = `<li class="artifact-empty">${escapeHtml(i18nText("pane.artifacts.empty"))}</li>`;
    return;
  }
  artifactListEl.innerHTML = "";
  for (const a of artifacts) {
    const li = document.createElement("li");
    li.className = "artifact-item" + (a.path === activeArtifactPath ? " active" : "");
    const iconMap = {
      md: "📄", markdown: "📄",
      html: "🌐", htm: "🌐",
      png: "🖼️", jpg: "🖼️", jpeg: "🖼️", gif: "🖼️", webp: "🖼️", svg: "🖼️",
      pdf: "📕",
      csv: "📊", xlsx: "📊",
      json: "🔢", yaml: "🔢", yml: "🔢",
      txt: "📝",
    };
    const ext = (a.basename.split(".").pop() || "").toLowerCase();
    const icon = iconMap[ext] || "📎";

    const iconEl = document.createElement("span");
    iconEl.className = "artifact-icon";
    iconEl.textContent = icon;
    const nameEl = document.createElement("span");
    nameEl.className = "artifact-name";
    nameEl.textContent = a.basename;
    nameEl.title = a.path;
    const openBtn = document.createElement("button");
    openBtn.className = "artifact-open-btn";
    openBtn.type = "button";
    openBtn.title = i18nText("pane.preview.open_external");
    openBtn.textContent = "↗";
    openBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      invoke("open_in_system", { path: a.path }).catch((err) => {
        appendSystemMessage("⚠️ 打开失败: " + err);
      });
    });

    li.appendChild(iconEl);
    li.appendChild(nameEl);
    li.appendChild(openBtn);
    li.addEventListener("click", () => previewArtifact(a));
    artifactListEl.appendChild(li);
  }
}

/** 预览 artifact —— 拉 artifact_info 判断类型，按类型渲染 preview 容器。 */
async function previewArtifact(a) {
  activeArtifactPath = a.path;
  renderArtifactList();
  if (!artifactPreviewEl) return;
  artifactPreviewEl.innerHTML = "<div style='opacity:.5;padding:8px'>加载中...</div>";
  let info;
  try {
    info = await invoke("artifact_info", { path: a.path });
  } catch (e) {
    artifactPreviewEl.innerHTML = `<div style="opacity:.6;padding:8px">无法读取: ${escapeHtml(String(e))}</div>`;
    return;
  }
  if (!info.exists) {
    artifactPreviewEl.innerHTML = `<div style="opacity:.6;padding:8px">文件不存在或已被删除</div>`;
    return;
  }
  if (info.kind === "md") {
    try {
      const text = await invoke("read_artifact_text", { path: a.path });
      artifactPreviewEl.innerHTML = `<div class="bubble rendered" style="background:transparent;border:none;box-shadow:none;padding:0">${renderMarkdown(text)}</div>`;
    } catch (e) {
      artifactPreviewEl.innerHTML = `<div style="opacity:.6;padding:8px">读取失败: ${escapeHtml(String(e))}</div>`;
    }
  } else if (info.kind === "html") {
    try {
      const text = await invoke("read_artifact_text", { path: a.path });
      const iframe = document.createElement("iframe");
      iframe.sandbox = "allow-same-origin allow-scripts";
      iframe.srcdoc = text;
      iframe.style.height = "100%";
      artifactPreviewEl.innerHTML = "";
      artifactPreviewEl.appendChild(iframe);
    } catch (e) {
      artifactPreviewEl.innerHTML = `<div style="opacity:.6;padding:8px">读取失败: ${escapeHtml(String(e))}</div>`;
    }
  } else if (info.kind === "image") {
    // Tauri 默认 file:// 不可直接 img src，需要 convertFileSrc。简单走 system 应用
    artifactPreviewEl.innerHTML = `<div style="padding:8px"><p style="opacity:.7;font-size:.9em">图片预览暂不支持嵌入,</p><button class="artifact-open-btn" style="opacity:1;border:1px solid currentColor;padding:6px 12px">↗ ${escapeHtml(i18nText("pane.preview.open_external"))}</button></div>`;
    artifactPreviewEl.querySelector("button").addEventListener("click", () => {
      invoke("open_in_system", { path: a.path });
    });
  } else if (info.kind === "text") {
    try {
      const text = await invoke("read_artifact_text", { path: a.path });
      const pre = document.createElement("pre");
      pre.style.cssText = "white-space:pre-wrap;word-break:break-word;font-size:.85em;";
      pre.textContent = text;
      artifactPreviewEl.innerHTML = "";
      artifactPreviewEl.appendChild(pre);
    } catch (e) {
      artifactPreviewEl.innerHTML = `<div style="opacity:.6;padding:8px">读取失败: ${escapeHtml(String(e))}</div>`;
    }
  } else {
    artifactPreviewEl.innerHTML = `<div style="padding:8px"><p style="opacity:.7">${escapeHtml(i18nText("pane.preview.unsupported"))}</p><button class="artifact-open-btn" style="opacity:1;border:1px solid currentColor;padding:6px 12px;margin-top:8px">↗ ${escapeHtml(i18nText("pane.preview.open_external"))}</button></div>`;
    artifactPreviewEl.querySelector("button").addEventListener("click", () => {
      invoke("open_in_system", { path: a.path });
    });
  }
}

// 右栏 tab 切换
document.querySelectorAll(".right-pane-tab").forEach((btn) => {
  btn.addEventListener("click", () => {
    const tab = btn.dataset.paneTab;
    document.querySelectorAll(".right-pane-tab").forEach((b) => {
      b.classList.toggle("active", b === btn);
    });
    document.querySelectorAll(".pane-tab-panel").forEach((p) => {
      p.classList.toggle("pane-tab-active", p.dataset.panePanel === tab);
    });
    // 切到产物 tab + 右栏展开 → 清 unread
    if (isArtifactsTabVisible()) clearUnreadArtifacts();
  });
});

// 右栏折叠/展开：toggle 按钮根据当前窗口宽度决定首次点击的行为
// - 大窗（>1280px）：auto 默认显示 → 首次点击隐藏
// - 小窗（≤1280px）：auto 默认隐藏（overlay）→ 首次点击展开 + 显示 backdrop
const rightPaneBackdrop = document.getElementById("right-pane-backdrop");
const rightPaneClose = document.getElementById("right-pane-close");

function setRightPaneState(state) {
  if (!rightPane) return;
  rightPane.dataset.collapsed = state;
  // 小窗下 expanded === overlay 显示 → 同步 backdrop 可见性
  const expanded = state === "false";
  if (rightPaneBackdrop) {
    rightPaneBackdrop.classList.toggle("active", expanded && window.innerWidth <= 1280);
  }
  // 展开 + 产物 tab 激活 → 清除 unread badge
  if (isArtifactsTabVisible()) clearUnreadArtifacts();
}

rightPaneToggle?.addEventListener("click", () => {
  if (!rightPane) return;
  const cur = rightPane.dataset.collapsed || "auto";
  const isWide = window.innerWidth > 1280;
  let next;
  if (cur === "auto") {
    next = isWide ? "true" : "false";
  } else {
    next = cur === "false" ? "true" : "false";
  }
  setRightPaneState(next);
});

// 浮层 ✕ 按钮 + backdrop 点击 = 关闭浮层
rightPaneClose?.addEventListener("click", () => setRightPaneState("true"));
rightPaneBackdrop?.addEventListener("click", () => setRightPaneState("true"));

// ── 阶段 C: 输入栏多文件上传 ──────────────────────────────────────

const attachBtn = document.getElementById("attach-btn");
const attachmentRow = document.getElementById("attachment-row");
const composer = document.querySelector(".composer");

const KIND_ICONS = {
  text: "📄", pdf: "📕", docx: "📘", xlsx: "📊",
  image: "🖼️", binary: "📎", oversize: "⚠️", missing: "❓",
};

function renderAttachments() {
  if (!attachmentRow) return;
  attachmentRow.innerHTML = "";
  attachmentRow.classList.toggle("has-items", pendingAttachments.length > 0);
  for (const att of pendingAttachments) {
    const chip = document.createElement("div");
    chip.className = "attachment-chip";
    if (att.status === "parsing") chip.classList.add("parsing");
    if (att.status === "error") chip.classList.add("error");
    const isImage = att.result?.kind === "image";
    const hasWarn = att.result?.warning && att.result.warning !== "model_no_vision";
    if (isImage || att.result?.warning === "model_no_vision") chip.classList.add("warn");
    if (hasWarn) chip.classList.add("warn");

    const iconChar = att.status === "parsing" ? "⏳"
      : (KIND_ICONS[att.result?.kind] || "📎");
    const icon = document.createElement("span");
    icon.className = "chip-icon";
    icon.textContent = iconChar;

    const name = document.createElement("span");
    name.className = "chip-name";
    name.textContent = att.basename;
    name.title = (att.result?.warning && att.result.warning !== "model_no_vision")
      ? `${att.path}\n⚠️ ${att.result.warning}`
      : (att.path || att.basename);

    const meta = document.createElement("span");
    meta.className = "chip-meta";
    if (att.status === "parsing") {
      meta.textContent = "解析中";
    } else if (att.result?.kind === "image") {
      meta.textContent = "图片·无视觉";
    } else if (att.result?.token_estimate) {
      meta.textContent = `~${att.result.token_estimate}t`;
    } else if (att.result?.warning) {
      meta.textContent = "✕";
    }

    const remove = document.createElement("button");
    remove.className = "chip-remove";
    remove.type = "button";
    remove.textContent = "✕";
    remove.title = "移除";
    remove.addEventListener("click", () => removeAttachment(att.id));

    chip.appendChild(icon);
    if (isImage || att.result?.warning) {
      const warn = document.createElement("span");
      warn.className = "chip-warn";
      warn.textContent = "⚠";
      chip.appendChild(warn);
    }
    chip.appendChild(name);
    chip.appendChild(meta);
    chip.appendChild(remove);
    attachmentRow.appendChild(chip);
  }
}

function removeAttachment(id) {
  pendingAttachments = pendingAttachments.filter((a) => a.id !== id);
  renderAttachments();
}

function clearAttachments() {
  pendingAttachments = [];
  renderAttachments();
}

/** 给一个 path 数组(已在用户磁盘真实存在)：直接 ingest，不拷贝。
 *  选文件 + 拖拽都走这里——保留原始路径让 AI 看到真实位置。 */
async function attachFromPaths(paths) {
  for (const path of paths) {
    const id = ++attachSeq;
    const basename = path.split(/[\\/]/).pop() || path;
    const entry = { id, basename, path, status: "parsing", result: null };
    pendingAttachments.push(entry);
    renderAttachments();
    try {
      const result = await invoke("ingest_file", { path });
      entry.result = result;
      entry.path = result.path || path;
      entry.basename = result.basename || basename;
      entry.status =
        result.kind === "oversize" || result.kind === "missing" ? "error" : "ready";
    } catch (e) {
      entry.status = "error";
      entry.result = { kind: "error", warning: String(e), token_estimate: 0, byte_size: 0 };
    }
    renderAttachments();
  }
}

/** 粘贴板的图片 blob——磁盘上没原 path，必须 save_paste_image 落盘后 ingest。 */
async function attachFromBlob(blob, suggestedName) {
  const id = ++attachSeq;
  const ext = (blob.type.split("/")[1] || "bin").split(";")[0];
  const basename = suggestedName || `paste-${id}.${ext}`;
  const entry = { id, basename, path: null, status: "parsing", result: null };
  pendingAttachments.push(entry);
  renderAttachments();
  try {
    const buf = await blob.arrayBuffer();
    const bytes = Array.from(new Uint8Array(buf));
    const path = await invoke("save_paste_image", { filename: basename, bytes });
    entry.path = path;
    const result = await invoke("ingest_file", { path });
    entry.result = result;
    entry.basename = result.basename || basename;
    entry.status =
      result.kind === "oversize" || result.kind === "missing" ? "error" : "ready";
  } catch (e) {
    entry.status = "error";
    entry.result = { kind: "error", warning: String(e), token_estimate: 0, byte_size: 0 };
  }
  renderAttachments();
}

// 点击 📎 按钮 → Tauri native dialog 拿原 path
attachBtn?.addEventListener("click", async () => {
  if (typeof dialogOpen !== "function") {
    appendSystemMessage("⚠️ 文件选择器不可用 (dialog plugin 未加载)");
    return;
  }
  try {
    const selection = await dialogOpen({ multiple: true, directory: false });
    if (!selection) return;
    const paths = Array.isArray(selection) ? selection : [selection];
    await attachFromPaths(paths);
  } catch (e) {
    console.warn("dialog open failed", e);
  }
});

// 粘贴：检测剪贴板里的 image blob (粘贴文本走 textarea 默认行为)
input.addEventListener("paste", async (e) => {
  const items = Array.from(e.clipboardData?.items || []);
  const blobs = items
    .filter((it) => it.kind === "file")
    .map((it) => it.getAsFile())
    .filter(Boolean);
  if (blobs.length > 0) {
    e.preventDefault();
    for (const blob of blobs) await attachFromBlob(blob);
  }
});

// 拖拽：用 Tauri native onDragDropEvent 拿真实 path —— 不依赖 HTML5 drop
// (后者在 webview 下拿不到 path)
(async function setupNativeDragDrop() {
  if (typeof getCurrentWebviewWindow !== "function") {
    console.warn("Tauri webviewWindow API not available, drag-drop disabled");
    return;
  }
  try {
    const win = getCurrentWebviewWindow();
    await win.onDragDropEvent((event) => {
      const payload = event.payload || {};
      // payload.type: "enter" | "over" | "drop" | "leave"
      if (payload.type === "over" || payload.type === "enter") {
        composer?.classList.add("drag-over");
      } else if (payload.type === "drop") {
        composer?.classList.remove("drag-over");
        const paths = payload.paths || [];
        if (paths.length > 0) attachFromPaths(paths);
      } else {
        composer?.classList.remove("drag-over");
      }
    });
  } catch (e) {
    console.warn("setupNativeDragDrop failed", e);
  }
})();

// 浏览器 tab url submit
browserGoBtn?.addEventListener("click", () => loadBrowserUrl());
browserUrlEl?.addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    e.preventDefault();
    loadBrowserUrl();
  }
});
function loadBrowserUrl() {
  if (!browserUrlEl || !browserFrameEl) return;
  let url = browserUrlEl.value.trim();
  if (!url) return;
  if (!/^https?:\/\//i.test(url)) url = "https://" + url;
  browserFrameEl.src = url;
}

// ── Tauri 事件订阅 ────────────────────────────────────────────────
listen("chat:delta", (e) => {
  const text = e.payload?.text || "";
  pendingAssistantText += text;
  appendAssistantDelta(text);
});
listen("chat:tool_start", (e) => {
  const { id, name, args } = e.payload || {};
  // 缓存原始 args 给 tool_end 用（提取产物路径）。
  // 防御：上游可能为同一 id 多次 fire（如 ApprovalRequired），args 为 null 时不覆盖已存。
  if (!toolMeta.has(id) || args != null) {
    toolMeta.set(id, { name, args });
  }
  appendToolCallStart(id, name, args);
});
listen("chat:tool_end", (e) => {
  const { id, output, success } = e.payload || {};
  appendToolCallEnd(id, output, success);
  // write_file 成功 → 加入产物列表
  if (success) {
    const meta = toolMeta.get(id);
    if (meta && meta.name === "write_file") {
      const path = extractArtifactPath(meta.args);
      if (path) trackArtifact(path);
    }
  }
  toolMeta.delete(id);
});
listen("chat:usage", (e) => {
  const input = Number(e.payload?.input_tokens || 0);
  if (input > 0) {
    lastInputTokens = input;
    updateTokenBar(input);
  }
});
listen("chat:compaction", (e) => {
  const phase = e.payload?.phase;
  const msg = e.payload?.message || "";
  const auto = e.payload?.auto ? "（自动）" : "";
  if (phase === "start") {
    appendSystemMessage(`⏳ 正在压缩上下文${auto} ${msg}`);
  } else if (phase === "done") {
    const before = e.payload?.messages_before;
    const after = e.payload?.messages_after;
    const detail = (before != null && after != null) ? ` (${before} → ${after} 条消息)` : "";
    appendSystemMessage(`✓ 上下文压缩完成${auto}${detail} ${msg}`);
    // 上游 CompactionCompleted 不带新 usage，前端按 messages 比例粗估更新进度条。
    // system prompt + tools schema 是不被压缩的 baseline (~tools schema 占大头,
    // 实测约总量 40%),所以最少保留 40%,真实值由下一轮 TurnComplete 修正。
    if (typeof before === "number" && typeof after === "number" && before > 0 && lastInputTokens > 0) {
      const ratio = after / before;
      const baseline = Math.round(lastInputTokens * 0.4);
      const proportional = Math.round(lastInputTokens * ratio);
      const estimate = Math.max(baseline, proportional);
      lastInputTokens = estimate;
      updateTokenBar(estimate);
    }
  } else if (phase === "fail") {
    appendSystemMessage(`⚠️ 上下文压缩失败${auto}: ${msg}`);
  }
});
// 等待下一次 chat:done 的 Promise 队列。switchToSession 在 busy 时
// 先 cancel + waitForChatDone() 再真切，避免 race。
let pendingDoneResolvers = [];
function waitForChatDone() {
  return new Promise((resolve) => pendingDoneResolvers.push(resolve));
}

listen("chat:done", async (e) => {
  const error = e.payload?.error;
  if (error) appendSystemMessage("⚠️ " + error);

  // 把累积的 assistant 文本入栈到 messages（如果有内容）
  if (pendingAssistantText) {
    messages.push({
      role: "assistant",
      content: [{ type: "text", text: pendingAssistantText }],
    });
    pendingAssistantText = "";
  }
  closeAssistantBubble();
  setBusy(false);

  // 持久化整轮（含 user + assistant）到 disk
  await persistMessages();

  // 通知等待 done 的 waiters（用于 cancel-then-switch 流程）
  const resolvers = pendingDoneResolvers;
  pendingDoneResolvers = [];
  for (const r of resolvers) r();
});

// ── 发送 ───────────────────────────────────────────────────────────
async function send() {
  const text = input.value.trim();
  const readyAttachments = pendingAttachments.filter((a) => a.status === "ready" && a.result);
  // 文本空 + 附件空 → 不发
  if (!text && readyAttachments.length === 0) return;
  if (busy) return;
  // 还有 parsing 中的附件 → 等
  if (pendingAttachments.some((a) => a.status === "parsing")) {
    appendSystemMessage("⚠️ 附件还在解析,请稍后再发");
    return;
  }
  if (!activeSessionId) {
    await createNewSession();
    if (!activeSessionId) {
      appendSystemMessage("⚠️ 创建对话失败，请重启 app");
      return;
    }
  }
  input.value = "";
  autoResize();
  // 显示 + state.messages：把附件 chip 名字附在 user 消息末尾让用户看到
  const displayText = readyAttachments.length > 0
    ? `${text}${text ? "\n\n" : ""}📎 ${readyAttachments.map((a) => a.basename).join(" · ")}`
    : text;
  appendUserMessage(displayText);
  // state.messages 存的是 LLM 看到的文本（含附件 markdown），从后端拼接出的相同结构
  // 简化：前端 state 只存 user 提问 + 附件 chip 名（不含整文 markdown，避免历史过大）
  messages.push({
    role: "user",
    content: [{ type: "text", text: displayText }],
  });
  // 把后端期望的 attachments payload 准备好（IngestResult 数组）
  const attachmentsPayload = readyAttachments.map((a) => a.result);
  clearAttachments();
  setBusy(true);
  try {
    await invoke("chat", { message: text, attachments: attachmentsPayload });
  } catch (err) {
    appendSystemMessage("⚠️ " + (err && err.toString ? err.toString() : err));
    setBusy(false);
  }
}
function autoResize() {
  input.style.height = "auto";
  input.style.height = Math.min(input.scrollHeight, 160) + "px";
}
sendBtn.addEventListener("click", async () => {
  if (busy) {
    // busy 模式下点击 = 停止生成
    try {
      await invoke("cancel_generation");
    } catch (e) {
      console.warn("cancel_generation failed", e);
    }
  } else {
    await send();
  }
});
input.addEventListener("input", autoResize);
input.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    if (busy) return; // Enter 不触发 cancel，必须点按钮才停止
    send();
  }
});

// ── 初始化 ────────────────────────────────────────────────────────
(async function init() {
  setBusy(false);
  input.focus();
  await loadSettings();

  // 历史列表 + 选最近一条 active；没历史则创建空 session
  await refreshHistoryList();
  if (sessionsCache.length > 0) {
    await switchToSession(sessionsCache[0].id);
  } else {
    await createNewSession();
  }

  // 启动两个定时拉取
  pollMonitor();
  pollBackendStatus();
  setInterval(pollMonitor, 5000);
  setInterval(pollBackendStatus, 10000);
})();
