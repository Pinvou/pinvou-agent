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
let monitorIntervalId = null;
let gpuUtilHistory = []; // 5 个最近 util 滑窗，render 取 max（A+B：1s 采样 + 5s 窗口峰值）
const GPU_UTIL_WINDOW = 5;

// 阶段 C: 多对话历史 —— 前端是 messages 的 source of truth。
// state.messages 对齐 deepseek-tui Message schema (role + content[]).
// 每次 TurnComplete 调 save_session_messages 落盘。切换 session 时
// load_session 拿回 messages 重渲染。
let activeSessionId = null;
let messages = [];          // 当前 session 的完整消息数组 (Anthropic Messages API schema:
                            // role + content[ContentBlock]. ContentBlock: text/tool_use/tool_result/thinking)
let pendingAssistantText = "";    // 当前 assistant message 内尚未 flush 的 text 段缓冲
let pendingAssistantBlocks = [];  // 当前 assistant message 已 flush 的 content blocks (text + tool_use)

// flush 当前 text 段为 text block, 加到 pendingAssistantBlocks. 遇 tool_use 或 turn 结束前调.
function flushPendingTextBlock() {
  if (pendingAssistantText) {
    pendingAssistantBlocks.push({ type: "text", text: pendingAssistantText });
    pendingAssistantText = "";
  }
}
// 把当前 assistant message 整体 push 到 messages (tool_result 来时 + chat:done 时调).
function flushAssistantMessageToHistory() {
  flushPendingTextBlock();
  if (pendingAssistantBlocks.length) {
    messages.push({ role: "assistant", content: pendingAssistantBlocks });
    pendingAssistantBlocks = [];
  }
}
function resetPendingAssistant() {
  pendingAssistantText = "";
  pendingAssistantBlocks = [];
}
let sessionsCache = [];     // 左侧历史列表数据（list_sessions 结果）
let artifacts = [];         // 当前 session 的产物列表（前端跟踪，重启 app 后丢）
const toolMeta = new Map(); // tool_call_id → {name, args}，给 tool_end 拿原始 args 用

// 阶段 C: 输入附件——发送前调 ingest_file 转 md，每条 chip 关联一份 IngestResult
let pendingAttachments = []; // { id, result: IngestResult, status: "parsing"|"ready"|"error" }
let attachSeq = 0;

// 阶段 D: Plan / YOLO 双模式状态机
// modeState = { mode, plan_phase, pinvou_review_enabled }
// pinvou_review_enabled 与 Plan/YOLO 正交,开启后 plan 期 accept_plan 触发 EXIT GATE。
// 设计:docs/Pinvou-品悟设计.md §5。
// 后端 SessionStore 是 source of truth，前端切 session 时同步拉一遍。
let modeState = { mode: "yolo", plan_phase: "none", pinvou_review_enabled: false };

// ── Thinking 指示器:Braille 10 帧 + 分阶段计时,以"气泡"形式跟随消息流尾部 ──
// (业界惯例: ChatGPT/Claude 都把 thinking 反馈放在最新消息下方, 不混进 mode 状态条)
// phase = "thinking"(LLM 思考/流式) | "tool"(工具调用中, 文案变"调用 xxx... Ns")
// 每次 phase 切换重置计时器 → 用户看到"每个小阶段花了多少时间".
const BRAILLE_FRAMES = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
let thinkingTicker = null;       // setInterval handle
let thinkingFrameIdx = 0;
let thinkingStartedAt = 0;
let thinkingFrame = BRAILLE_FRAMES[0];
let thinkingElapsedSec = 0;
let thinkingPhase = "thinking";
let thinkingToolName = "";
let thinkingBubbleEl = null;     // 单例气泡 DOM, 跟随 chatArea 尾部

function renderThinkingBubble() {
  if (!thinkingTicker) {
    removeThinkingBubble();
    return;
  }
  if (!thinkingBubbleEl) {
    thinkingBubbleEl = document.createElement("div");
    thinkingBubbleEl.className = "thinking-bubble";
    chatArea.appendChild(thinkingBubbleEl);
    scrollToBottom();
  } else if (chatArea.lastElementChild !== thinkingBubbleEl) {
    // 新消息/工具卡插入后, thinking 气泡不在末尾 → 重挪到末尾
    chatArea.appendChild(thinkingBubbleEl);
    scrollToBottom();
  }
  thinkingBubbleEl.textContent = thinkingPhase === "tool" && thinkingToolName
    ? `${thinkingFrame} 调用 ${thinkingToolName}... ${thinkingElapsedSec}s`
    : `${thinkingFrame} 思考中... ${thinkingElapsedSec}s`;
}

function removeThinkingBubble() {
  if (thinkingBubbleEl) {
    thinkingBubbleEl.remove();
    thinkingBubbleEl = null;
  }
}

function startThinking() {
  if (thinkingTicker) return;
  thinkingFrameIdx = 0;
  thinkingPhase = "thinking";
  thinkingToolName = "";
  thinkingStartedAt = Date.now();
  thinkingFrame = BRAILLE_FRAMES[0];
  thinkingElapsedSec = 0;
  renderThinkingBubble();
  thinkingTicker = setInterval(() => {
    thinkingFrameIdx = (thinkingFrameIdx + 1) % BRAILLE_FRAMES.length;
    thinkingFrame = BRAILLE_FRAMES[thinkingFrameIdx];
    thinkingElapsedSec = Math.floor((Date.now() - thinkingStartedAt) / 1000);
    renderThinkingBubble();
  }, 100);
}

function stopThinking() {
  if (thinkingTicker) {
    clearInterval(thinkingTicker);
    thinkingTicker = null;
  }
  thinkingFrame = BRAILLE_FRAMES[0];
  thinkingElapsedSec = 0;
  thinkingPhase = "thinking";
  thinkingToolName = "";
  removeThinkingBubble();
}

/** 切到工具阶段:重置计时,文案变成"调用 xxx... Ns"。 */
function switchThinkingToTool(toolName) {
  if (!thinkingTicker) return;
  thinkingPhase = "tool";
  thinkingToolName = toolName || "";
  thinkingStartedAt = Date.now();
  thinkingElapsedSec = 0;
  renderThinkingBubble();
}

/** 切回思考阶段:工具完成,重置计时,文案回到"思考中... Ns"。 */
function switchThinkingToIdle() {
  if (!thinkingTicker) return;
  thinkingPhase = "thinking";
  thinkingToolName = "";
  thinkingStartedAt = Date.now();
  thinkingElapsedSec = 0;
  renderThinkingBubble();
}

/**
 * 渲染 plan_card：消息流内嵌的方案卡片，**两层结构**：
 *   - plan 层（来自 update_plan）：高层 strategy，含 explanation + phase 步骤
 *   - todos 层（来自 checklist_write / todo_write）：每个 phase 下的细分待办
 * 任一存在就渲染，两个都有就分两段。
 *
 * 状态机：active → approved（点 ✅）/ revising（点 ✏️）/ discarded（点 🚪）/ frozen（被新卡片覆盖）
 *
 * snapshots: { plan?: PlanSnap, todos?: TodosSnap }
 *   PlanSnap  = { explanation?, items: [{ step, status }] }
 *   TodosSnap = { items: [{ id, content, status }], completion_pct, in_progress_id }
 */
function renderPlanReadyCard(snapshots) {
  freezeOldPlanCards();
  const card = document.createElement("div");
  card.className = "msg-row msg-plan-card";
  card.dataset.cardState = "active";
  const planMarkdown = composePlanMarkdown(snapshots);
  card.dataset.planMarkdown = planMarkdown;
  card.innerHTML = `
    <div class="plan-card-box">
      <div class="plan-card-header">✨ 方案准备好</div>
      <div class="plan-card-body"></div>
      <div class="plan-card-sep"></div>
      <div class="plan-card-footer">
        <div class="plan-card-prompt">下一步：</div>
        <div class="plan-card-actions">
          <button class="plan-card-btn plan-card-accept" type="button">✅ 就这么干</button>
          <button class="plan-card-btn plan-card-revise" type="button">✏️ 改改</button>
          <button class="plan-card-btn plan-card-discard" type="button">🚪 算了</button>
        </div>
        <div class="plan-card-status" hidden></div>
      </div>
    </div>
  `;
  const bodyEl = card.querySelector(".plan-card-body");
  renderSnapshotsInto(bodyEl, snapshots);
  card.querySelector(".plan-card-accept").addEventListener("click", () => onPlanAccept(card));
  card.querySelector(".plan-card-revise").addEventListener("click", () => onPlanRevise(card));
  card.querySelector(".plan-card-discard").addEventListener("click", () => onPlanDiscard(card));
  chatArea.appendChild(card);
  scrollToBottom();
}

/** 拼 accept 时发给后端的 plan markdown：含 plan + todos 全部内容,方便 AI 按方案执行。 */
function composePlanMarkdown(snapshots) {
  const lines = [];
  const plan = snapshots && snapshots.plan;
  const todos = snapshots && snapshots.todos;
  if (plan && Array.isArray(plan.items)) {
    if (plan.explanation) {
      lines.push("**方案：**", plan.explanation, "");
    }
    lines.push("**步骤：**");
    plan.items.forEach((item, i) => {
      const sym = item.status === "completed" ? "●" : item.status === "in_progress" ? "◎" : "○";
      lines.push(`${i + 1}. ${sym} ${item.step}`);
    });
    lines.push("");
  }
  if (todos && Array.isArray(todos.items)) {
    lines.push("**细分待办：**");
    todos.items.forEach((item, i) => {
      const sym = item.status === "completed" ? "●" : item.status === "in_progress" ? "◎" : "○";
      lines.push(`${i + 1}. ${sym} ${item.content}`);
    });
  }
  return lines.length > 0 ? lines.join("\n") : "（plan 为空）";
}

/** 把 plan + todos 渲染到卡片 body。两层独立 section,标签清晰区分。 */
function renderSnapshotsInto(el, snapshots) {
  el.innerHTML = "";
  const plan = snapshots && snapshots.plan;
  const todos = snapshots && snapshots.todos;
  if (!plan && !todos) {
    el.textContent = "（plan 为空）";
    return;
  }
  if (plan && Array.isArray(plan.items) && plan.items.length > 0) {
    el.appendChild(renderLayerSection("📋 方案", plan.explanation, plan.items, "step"));
  }
  if (todos && Array.isArray(todos.items) && todos.items.length > 0) {
    el.appendChild(renderLayerSection("✅ 细分待办", null, todos.items, "content"));
  }
}

/** 渲染一层(plan 或 todos): 标签 + 可选 explanation + 步骤列表。
 *  itemField: "step" (plan) or "content" (todos) —— 字段名归一化。 */
function renderLayerSection(label, explanation, items, itemField) {
  const wrap = document.createElement("section");
  wrap.className = "plan-card-layer";
  const head = document.createElement("div");
  head.className = "plan-card-layer-head";
  head.textContent = label;
  wrap.appendChild(head);
  if (explanation) {
    const p = document.createElement("p");
    p.className = "plan-card-explanation";
    p.textContent = explanation;
    wrap.appendChild(p);
  }
  const ol = document.createElement("ol");
  ol.className = "plan-card-steps";
  for (const item of items) {
    const li = document.createElement("li");
    li.dataset.status = item.status || "pending";
    const sym = item.status === "completed" ? "●" : item.status === "in_progress" ? "◎" : "○";
    li.innerHTML = `<span class="plan-step-sym">${sym}</span> <span class="plan-step-text"></span>`;
    li.querySelector(".plan-step-text").textContent = item[itemField] || "";
    ol.appendChild(li);
  }
  wrap.appendChild(ol);
  return wrap;
}

/** 新 plan_card 出现前，把所有旧的 active 卡片冻结成 "📜 已过期"。 */
function freezeOldPlanCards() {
  chatArea.querySelectorAll('.msg-plan-card[data-card-state="active"]').forEach((old) => {
    setPlanCardFrozen(old, "📜 已被新方案覆盖");
  });
}

function setPlanCardFrozen(card, label) {
  card.dataset.cardState = "frozen";
  card.querySelectorAll(".plan-card-btn").forEach((b) => (b.disabled = true));
  const status = card.querySelector(".plan-card-status");
  status.hidden = false;
  status.textContent = label;
}

async function onPlanAccept(card) {
  if (card.dataset.cardState !== "active") return;
  if (!activeSessionId) return;
  card.dataset.cardState = "approved";
  card.querySelectorAll(".plan-card-btn").forEach((b) => (b.disabled = true));
  const status = card.querySelector(".plan-card-status");
  status.hidden = false;
  status.textContent = "✅ 已批准";
  appendUserMessage("✅ 就这么干");
  messages.push({ role: "user", content: [{ type: "text", text: "✅ 就这么干" }] });
  const planMd = card.dataset.planMarkdown || "";
  setBusy(true);
  try {
    const state = await invoke("accept_plan", {
      sessionId: activeSessionId,
      planMarkdown: planMd,
    });
    modeState = {
      mode: state.mode,
      plan_phase: state.plan_phase,
      pinvou_review_enabled: !!state.pinvou_review_enabled,
    };
    updateModeUI();
  } catch (e) {
    // Pinvou Review GATE 失败时,e 是 JSON 字符串 {gate_error, message, detail}
    const gateInfo = parseGateError(e);
    if (gateInfo) {
      // 反 freeze:plan 还没真接受,允许用户后续重点
      card.dataset.cardState = "active";
      card.querySelectorAll(".plan-card-btn").forEach((b) => (b.disabled = false));
      status.hidden = true;
      setBusy(false);
      if (gateInfo.gate_error === "missing_review_report") {
        await autoTriggerPinvouReview(card, "品悟还没看过这个方案");
        return;
      }
      appendSystemMessage(`⚠️ Pinvou EXIT GATE 阻塞: ${gateInfo.message}`);
      return;
    }
    appendSystemMessage("⚠️ accept_plan 失败: " + e);
    setBusy(false);
  }
}

async function onPlanRevise(card) {
  if (card.dataset.cardState !== "active") return;
  if (!activeSessionId) return;
  // 修法 D: 照搬 DeepSeek-TUI 底座做法 —— 不改 mode_state, 不 freeze 卡片(用户可反悔点 ✅),
  // 只在输入框预填"修订方案:"前缀. AI 看到前缀 + 当前 phase=Ready 触发的 reminder
  // "用户发新消息=隐式修订,必须重新调 update_plan",自然重出方案.
  input.value = "修订方案:";
  input.focus();
  input.setSelectionRange(input.value.length, input.value.length);
  input.placeholder = "继续写: 你想怎么改方案…";
}

async function onPlanDiscard(card) {
  if (card.dataset.cardState !== "active") return;
  if (!activeSessionId) return;
  setPlanCardFrozen(card, "🚪 已退出 Plan");
  try {
    const state = await invoke("discard_plan", { sessionId: activeSessionId });
    modeState = { mode: state.mode, plan_phase: state.plan_phase };
    updateModeUI();
  } catch (e) {
    appendSystemMessage("⚠️ discard_plan 失败: " + e);
  }
}

// ── request_user_input 选择气泡 ─────────────────────────────────────
// 跟 plan_card 类似但更轻：1-3 个问题, 每题 2-3 个选项, 用户选完所有题
// 后才 submit_user_input。期间 engine 在 await_user_input loop 等。

const userInputCards = new Map(); // tool_call_id → { card, answers, questions }

function renderUserInputCard(toolCallId, questions) {
  if (!Array.isArray(questions) || questions.length === 0) return;
  const card = document.createElement("div");
  card.className = "msg-row msg-user-input";
  card.dataset.toolCallId = toolCallId;
  card.dataset.cardState = "active";
  card.innerHTML = `
    <div class="user-input-box">
      <div class="user-input-header">🤔 AI 想问你几个问题</div>
      <div class="user-input-body"></div>
      <div class="user-input-status" hidden></div>
    </div>
  `;
  const body = card.querySelector(".user-input-body");
  const answers = new Array(questions.length).fill(null);
  questions.forEach((q, qi) => {
    const block = document.createElement("div");
    block.className = "user-input-question";
    const headEl = document.createElement("div");
    headEl.className = "user-input-q-header";
    headEl.textContent = q.header || `Q${qi + 1}`;
    const qEl = document.createElement("div");
    qEl.className = "user-input-q-text";
    qEl.textContent = q.question || "";
    const opts = document.createElement("div");
    opts.className = "user-input-options";
    (q.options || []).forEach((opt) => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "user-input-option";
      btn.dataset.qIndex = String(qi);
      btn.innerHTML = `<span class="user-input-option-label"></span><span class="user-input-option-desc"></span>`;
      btn.querySelector(".user-input-option-label").textContent = opt.label || "";
      btn.querySelector(".user-input-option-desc").textContent = opt.description || "";
      btn.addEventListener("click", () => {
        // 同一题选过的取消高亮，本题所有按钮去 selected
        opts.querySelectorAll(".user-input-option").forEach((b) => b.classList.remove("selected"));
        // 收起任何已展开的 Other 输入区
        opts.querySelectorAll(".user-input-other-box").forEach((box) => box.remove());
        btn.classList.add("selected");
        answers[qi] = {
          id: q.id,
          label: opt.label,
          value: opt.label,
        };
        maybeSubmit();
      });
      opts.appendChild(btn);
    });
    // [💬 其他(自己写)] —— Claude Code 同款,所有 question 自动加,让用户自由输入。
    // 点击 inline 展开 textarea + 提交按钮,不占消息流。
    const otherBtn = document.createElement("button");
    otherBtn.type = "button";
    otherBtn.className = "user-input-option user-input-other";
    otherBtn.dataset.qIndex = String(qi);
    otherBtn.innerHTML = `<span class="user-input-option-label">💬 其他(自己写)</span><span class="user-input-option-desc">如果上面选项不合适,自己说一下</span>`;
    otherBtn.addEventListener("click", () => {
      // 已展开就关闭
      const existing = opts.querySelector(".user-input-other-box");
      if (existing) { existing.remove(); return; }
      // 清掉之前的选项高亮 + 已答(用户在改主意)
      opts.querySelectorAll(".user-input-option").forEach((b) => b.classList.remove("selected"));
      answers[qi] = null;
      // 创建 inline 输入区
      const box = document.createElement("div");
      box.className = "user-input-other-box";
      box.innerHTML = `
        <textarea class="user-input-other-textarea" rows="2" placeholder="写下你想说的..."></textarea>
        <div class="user-input-other-actions">
          <button class="user-input-other-cancel" type="button">取消</button>
          <button class="user-input-other-submit" type="button">提交</button>
        </div>
      `;
      const textarea = box.querySelector(".user-input-other-textarea");
      box.querySelector(".user-input-other-cancel").addEventListener("click", () => box.remove());
      box.querySelector(".user-input-other-submit").addEventListener("click", () => {
        const val = textarea.value.trim();
        if (!val) { textarea.focus(); return; }
        otherBtn.classList.add("selected");
        answers[qi] = { id: q.id, label: "其他", value: val };
        box.remove();
        maybeSubmit();
      });
      textarea.addEventListener("keydown", (e) => {
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
          e.preventDefault();
          box.querySelector(".user-input-other-submit").click();
        }
      });
      opts.appendChild(box);
      // 卡片可能比视口高,box 展开在卡片底部容易滚出视口看不到。
      // focus 不一定滚动,显式 scrollIntoView 保证用户看到 textarea。
      setTimeout(() => {
        textarea.focus();
        box.scrollIntoView({ behavior: "smooth", block: "center" });
      }, 30);
    });
    opts.appendChild(otherBtn);
    block.appendChild(headEl);
    block.appendChild(qEl);
    block.appendChild(opts);
    body.appendChild(block);
  });
  chatArea.appendChild(card);
  scrollToBottom();
  userInputCards.set(toolCallId, { card, answers, questions });

  async function maybeSubmit() {
    // 所有题都选完 → submit
    if (answers.some((a) => a == null)) return;
    card.querySelectorAll(".user-input-option").forEach((b) => (b.disabled = true));
    const status = card.querySelector(".user-input-status");
    status.hidden = false;
    status.textContent = "提交中…";
    try {
      await invoke("submit_user_input", { toolCallId, answers });
      // 在用户气泡追加用户的选择(让对话流可读),不持久化到 messages.json
      // "其他"选项 label="其他"+value=用户输入,要展示 value 才看得见实际内容
      const summary = answers
        .map((a, i) => {
          const text = a.label === "其他" ? `(其他) ${a.value}` : a.label;
          return `${questions[i].header}: ${text}`;
        })
        .join(" · ");
      appendUserMessage("✓ " + summary);
      // 用户选择是一个分界点:关闭当前 assistant 气泡,把已累积 text 入栈 messages,
      // 下一段 LLM 输出会开新气泡(视觉上接在用户气泡之后,不会串到 request_user_input
      // 之前的旧气泡里)。
      // chat:tool_end 会自动 flush + push user/tool_result message, 这里只关闭 bubble
      // 让后续 LLM 输出开新气泡, 避免串到工具前的气泡里.
      flushAssistantMessageToHistory();
      closeAssistantBubble();
      status.textContent = "✓ 已提交";
    } catch (e) {
      status.textContent = "⚠️ 提交失败: " + e;
      card.querySelectorAll(".user-input-option").forEach((b) => (b.disabled = false));
    }
  }
}

function finalizeUserInputCard(toolCallId, success) {
  const entry = userInputCards.get(toolCallId);
  if (!entry) return;
  const { card } = entry;
  card.dataset.cardState = success ? "submitted" : "cancelled";
  card.querySelectorAll(".user-input-option").forEach((b) => (b.disabled = true));
  const status = card.querySelector(".user-input-status");
  status.hidden = false;
  status.textContent = success ? "✓ 已提交" : "✕ 已取消";
  userInputCards.delete(toolCallId);
}

// ── Plan 模式死锁兜底卡片 ────────────────────────────────────────────
// AI 在 Plan 模式调了不在工具集的工具(常见 write_file/edit_file/exec_shell)
// 失败时弹这个卡片,给用户两条出路:让 AI 重出方案 / 跳过方案直接干。

function showPlanStuckCard(toolName) {
  // 同一 turn 内多次失败不重复插入,等用户处理完旧的
  if (chatArea.querySelector('.msg-plan-stuck:not([data-resolved])')) return;
  const card = document.createElement("div");
  card.className = "msg-row msg-plan-stuck";
  const safeName = toolName ? String(toolName) : "(unknown)";
  card.innerHTML = `
    <div class="plan-stuck-box">
      <div class="plan-stuck-text">
        ⚠️ AI 在 Plan 模式调用了 <code class="plan-stuck-tool"></code> 但被白名单挡掉。
        Plan 模式只能讨论方案,不能动手。给你两个出路:
      </div>
      <div class="plan-stuck-actions">
        <button class="plan-stuck-btn plan-stuck-replan" type="button">📋 让 AI 重出方案</button>
        <button class="plan-stuck-btn plan-stuck-go" type="button">⚡ 直接动手(跳过方案)</button>
      </div>
      <div class="plan-stuck-status" hidden></div>
    </div>
  `;
  card.querySelector(".plan-stuck-tool").textContent = safeName;
  card.querySelector(".plan-stuck-replan").addEventListener("click", () => onPlanStuckReplan(card));
  card.querySelector(".plan-stuck-go").addEventListener("click", () => onPlanStuckGo(card));
  chatArea.appendChild(card);
  scrollToBottom();
}

async function onPlanStuckReplan(card) {
  if (card.dataset.resolved) return;
  card.dataset.resolved = "true";
  card.querySelectorAll(".plan-stuck-btn").forEach((b) => (b.disabled = true));
  const status = card.querySelector(".plan-stuck-status");
  status.hidden = false;
  status.textContent = "📋 让 AI 重出方案…";
  input.value = "请用 update_plan 工具输出完整方案,不要直接调写工具。";
  await send();
}

async function onPlanStuckGo(card) {
  if (card.dataset.resolved) return;
  card.dataset.resolved = "true";
  card.querySelectorAll(".plan-stuck-btn").forEach((b) => (b.disabled = true));
  const status = card.querySelector(".plan-stuck-status");
  status.hidden = false;
  if (!activeSessionId) {
    status.textContent = "⚠️ 没有 active session";
    return;
  }
  try {
    const state = await invoke("exit_plan_to_yolo", { sessionId: activeSessionId });
    modeState = { mode: state.mode, plan_phase: state.plan_phase };
    updateModeUI();
    status.textContent = "⚡ 已切到 YOLO,自动接着干";
  } catch (e) {
    status.textContent = "⚠️ 退出 Plan 失败: " + e;
    return;
  }
  // 跟 onPlanStuckReplan 对称: 预填具体指令立即发送, 避免用户发"继续"模糊词
  // 触发 Qwen3.6 把 history 残留的 system-reminder 文本回显的 LLM 偏差.
  input.value = "按上面讨论的方案继续执行任务,直接写文件/跑命令,不要再讨论方案。";
  await send();
}

// ── M3: Plan 文本兜底卡片(AI 没用 plan 工具但 text 写了方案) ───────
function renderPlanTextFallbackCard(text) {
  if (chatArea.querySelector('.msg-plan-fallback:not([data-resolved])')) return;
  const card = document.createElement("div");
  card.className = "msg-row msg-plan-fallback";
  card.dataset.text = text;
  card.innerHTML = `
    <div class="plan-fallback-box">
      <div class="plan-fallback-header">📝 AI 给了方案但没用 plan 工具</div>
      <div class="plan-fallback-text">
        AI 在文本里描述了方案,但没调 update_plan 工具,所以没出方案卡片。你想怎么处理?
      </div>
      <div class="plan-fallback-actions">
        <button class="plan-fallback-btn plan-fallback-accept" type="button">✅ 直接采纳这段</button>
        <button class="plan-fallback-btn plan-fallback-retry" type="button">📋 让 AI 用工具重出</button>
        <button class="plan-fallback-btn plan-fallback-discard" type="button">🚪 算了</button>
      </div>
      <div class="plan-fallback-status" hidden></div>
    </div>
  `;
  card.querySelector(".plan-fallback-accept").addEventListener("click", async () => {
    if (card.dataset.resolved) return;
    card.dataset.resolved = "true";
    card.querySelectorAll(".plan-fallback-btn").forEach((b) => (b.disabled = true));
    const status = card.querySelector(".plan-fallback-status");
    status.hidden = false;
    status.textContent = "✅ 采纳中...";
    if (!activeSessionId) { status.textContent = "⚠️ 无 active session"; return; }
    appendUserMessage("✅ 采纳此方案");
    messages.push({ role: "user", content: [{ type: "text", text: "✅ 采纳此方案" }] });
    try {
      const state = await invoke("accept_plan", {
        sessionId: activeSessionId,
        planMarkdown: card.dataset.text || "",
      });
      modeState = { mode: state.mode, plan_phase: state.plan_phase };
      updateModeUI();
      status.textContent = "✅ 已采纳,AI 开始执行";
    } catch (err) {
      status.textContent = "⚠️ accept_plan 失败: " + err;
    }
  });
  card.querySelector(".plan-fallback-retry").addEventListener("click", async () => {
    if (card.dataset.resolved) return;
    card.dataset.resolved = "true";
    card.querySelectorAll(".plan-fallback-btn").forEach((b) => (b.disabled = true));
    const status = card.querySelector(".plan-fallback-status");
    status.hidden = false;
    status.textContent = "📋 让 AI 重出...";
    input.value = "请用 update_plan 工具把上面的方案重新输出一遍,我才能在卡片上决策。";
    await send();
  });
  card.querySelector(".plan-fallback-discard").addEventListener("click", async () => {
    if (card.dataset.resolved) return;
    card.dataset.resolved = "true";
    card.querySelectorAll(".plan-fallback-btn").forEach((b) => (b.disabled = true));
    const status = card.querySelector(".plan-fallback-status");
    status.hidden = false;
    if (!activeSessionId) { status.textContent = "🚪 已忽略"; return; }
    try {
      const state = await invoke("discard_plan", { sessionId: activeSessionId });
      modeState = { mode: state.mode, plan_phase: state.plan_phase };
      updateModeUI();
      status.textContent = "🚪 已退出 Plan";
    } catch (err) {
      status.textContent = "⚠️ discard 失败: " + err;
    }
  });
  chatArea.appendChild(card);
  scrollToBottom();
}

// ── M2: Executing 自驱上限触达提示 ───────────────────────────────────
function renderExecutionStuckCard(tries) {
  if (chatArea.querySelector('.msg-execution-stuck:not([data-resolved])')) return;
  const card = document.createElement("div");
  card.className = "msg-row msg-execution-stuck";
  card.innerHTML = `
    <div class="plan-stuck-box">
      <div class="plan-stuck-text">
        🛑 AI 执行卡住了 (已自动尝试 ${tries} 次仍未真正产出文件)。你可以:
      </div>
      <div class="plan-stuck-actions">
        <button class="plan-stuck-btn plan-stuck-replan" type="button">📋 让 AI 重出方案</button>
        <button class="plan-stuck-btn plan-stuck-go" type="button">⚡ 我自己来</button>
      </div>
      <div class="plan-stuck-status" hidden></div>
    </div>
  `;
  card.querySelector(".plan-stuck-replan").addEventListener("click", async () => {
    if (card.dataset.resolved) return;
    card.dataset.resolved = "true";
    card.querySelectorAll(".plan-stuck-btn").forEach((b) => (b.disabled = true));
    input.value = "你卡住了。请重新用 update_plan 工具列方案,我们再开始。";
    await send();
  });
  card.querySelector(".plan-stuck-go").addEventListener("click", async () => {
    if (card.dataset.resolved) return;
    card.dataset.resolved = "true";
    card.querySelectorAll(".plan-stuck-btn").forEach((b) => (b.disabled = true));
    if (!activeSessionId) return;
    try {
      const state = await invoke("discard_plan", { sessionId: activeSessionId });
      modeState = { mode: state.mode, plan_phase: state.plan_phase };
      updateModeUI();
    } catch (_) {}
  });
  chatArea.appendChild(card);
  scrollToBottom();
}

// ── i18n 字典（极简版，DOM 扫描 data-i18n / data-i18n-placeholder） ──
const I18N = {
  "zh-Hans": {
    "app.title": "pinvou3 智能助手",
    "brand.sub": "本地 · GB10",
    "nav.chatroom": "工作会话",
    "nav.workflow": "工作流",
    "nav.monitor": "监控",
    "nav.settings": "设置",
    "view.chatroom.title": "工作会话",
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
    "monitor.gpu_status": "状态",
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
    "workflow.empty.hint": "现在请在工作会话提需求。",
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
    "pane.artifacts.empty": "本对话还没有产物",
    "pane.preview.empty": "选择一个产物预览",
    "pane.preview.open_external": "用系统应用打开",
    "pane.preview.unsupported": "这种文件类型只能用系统应用打开",
    "modal.confirm": "确认",
    "modal.cancel": "取消",
  },
  "en": {
    "app.title": "pinvou3 Assistant",
    "brand.sub": "Local · GB10",
    "nav.chatroom": "Session",
    "nav.workflow": "WorkFlow",
    "nav.monitor": "Monitor",
    "nav.settings": "Settings",
    "view.chatroom.title": "Session",
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
    "monitor.gpu_status": "Status",
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
    "workflow.empty.hint": "For now, ask in Session.",
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
    "pane.artifacts.empty": "No artifacts in this conversation yet",
    "pane.preview.empty": "Select an artifact to preview",
    "pane.preview.open_external": "Open with system app",
    "pane.preview.unsupported": "This file type can only be opened with system app",
    "modal.confirm": "OK",
    "modal.cancel": "Cancel",
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
  // 监控按需采样：进入 monitor 启 1s interval，离开清掉
  // 避免不在监控页时白跑 nvidia-smi / vLLM probe
  if (name === "monitor") {
    if (!monitorIntervalId) {
      gpuUtilHistory = []; // 重置滑窗
      pollMonitor();
      monitorIntervalId = setInterval(pollMonitor, 1000);
    }
  } else if (monitorIntervalId) {
    clearInterval(monitorIntervalId);
    monitorIntervalId = null;
  }
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
    const vramLabel = document.querySelector('[data-i18n="monitor.vram"]');
    const vramBarWrap = document.getElementById("gpu-vram-bar").parentElement;
    if (snap.gpu.vram_total_mib > 0) {
      if (vramLabel) vramLabel.textContent = i18nText("monitor.vram");
      if (vramBarWrap) vramBarWrap.style.display = "";
      const vramPct = Math.round((snap.gpu.vram_used_mib / snap.gpu.vram_total_mib) * 100);
      document.getElementById("gpu-vram-bar").style.width = vramPct + "%";
      document.getElementById("gpu-vram-text").textContent =
        `${fmtMiB(snap.gpu.vram_used_mib)} / ${fmtMiB(snap.gpu.vram_total_mib)}`;
    } else {
      // GB10 等 unified-memory 设备：nvidia-smi 不报独立 VRAM，
      // 替换显示温度·功耗(更能反映 GPU 是否在工作)。无进度条(数据是绝对值不是占比)。
      if (vramLabel) vramLabel.textContent = i18nText("monitor.gpu_status");
      if (vramBarWrap) vramBarWrap.style.display = "none";
      const temp = snap.gpu.temperature_c;
      const power = snap.gpu.power_w;
      const tempStr = temp != null ? `${temp}°C` : "—";
      const powerStr = power != null ? `${power.toFixed(1)} W` : "—";
      document.getElementById("gpu-vram-text").textContent = `${tempStr} · ${powerStr}`;
    }
    // GPU util 滑窗 max：单次采样易错过短推理峰（GB10 idle=0% / 推理=96% 持续几秒）
    gpuUtilHistory.push(snap.gpu.utilization_pct);
    if (gpuUtilHistory.length > GPU_UTIL_WINDOW) gpuUtilHistory.shift();
    const utilMax = Math.max(0, ...gpuUtilHistory);
    document.getElementById("gpu-util-bar").style.width = utilMax + "%";
    document.getElementById("gpu-util-text").textContent = utilMax + "%";
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
      v.prefix_cache_hit_pct != null ? v.prefix_cache_hit_pct.toFixed(1) + "%" : "—";
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
  // 同步 thinking 指示器:busy=true 启动 Braille 动画 + 计时
  if (b) startThinking();
  else stopThinking();
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
  const meta = document.createElement("div");
  meta.className = "user-meta";
  meta.textContent = new Date().toTimeString().slice(0, 5);
  wrap.appendChild(label);
  wrap.appendChild(bubble);
  wrap.appendChild(meta);
  row.appendChild(wrap);
  chatArea.appendChild(row);
  scrollToBottom();
  updateMessageActions();
}
function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  })[c]);
}

// ════════════════════════════════════════════════════════════════════
// Pinvou Review v2 — careful 卡片 + 品悟气泡 + 3 按钮 + Fallback
// 设计:docs/Pinvou-品悟设计.md §4-§6
// ════════════════════════════════════════════════════════════════════

/** Careful hook 拦截卡片(红色醒目)。chat:tool_end 时 metadata.safety_level=="dangerous" 触发。 */
function renderCarefulBlockedCard(args, metadata) {
  const row = document.createElement("div");
  row.className = "msg-row msg-careful-blocked";
  const cmd = (args && (args.command || args.cmd)) || "(命令未知)";
  const reasons = (metadata.reasons || []).map((r) => `<li>${escapeHtml(r)}</li>`).join("");
  const suggestions = (metadata.suggestions || []).map((s) => `<li>${escapeHtml(s)}</li>`).join("");
  row.innerHTML = `
    <div class="careful-blocked-box">
      <div class="careful-blocked-header">🛑 Careful Hook 拦截了一条破坏性命令</div>
      <div class="careful-blocked-cmd"><code>${escapeHtml(cmd)}</code></div>
      <div class="careful-blocked-section">
        <div class="careful-blocked-label">为什么拦?</div>
        <ul>${reasons || "<li>命中破坏性 pattern</li>"}</ul>
      </div>
      ${suggestions ? `
      <div class="careful-blocked-section">
        <div class="careful-blocked-label">建议</div>
        <ul>${suggestions}</ul>
      </div>` : ""}
      <div class="careful-blocked-foot">
        这个 hook 在所有模式默认开启,与 Plan/YOLO/品悟开关无关。
        如果你确认要跑,自己在终端执行 —— LLM 不会被允许跑破坏性命令。
      </div>
    </div>
  `;
  chatArea.appendChild(row);
  scrollToBottom();
}

// === Pinvou Review:GATE / 提取 / 渲染 / Fallback ===

const PINVOU_REVIEW_RE = /## PINVOU REVIEW REPORT[\s\S]*$/m;

function extractPinvouReviewReport(text) {
  if (!text) return null;
  const m = text.match(PINVOU_REVIEW_RE);
  return m ? m[0].trim() : null;
}

function overrideAllCriticalInReport(report) {
  return report.replace(
    /^(\|[^|]*\|\s*CRITICAL\s*\|\s*)RAISED(\s*\|[^|]*\|)$/gim,
    "$1OVERRIDDEN_BY_USER$2",
  ).replace(
    /^(\|[^|]*\|\s*CRITICAL\s*\|\s*OVERRIDDEN_BY_USER\s*\|\s*)[^|]*(\|)$/gim,
    "$1用户拍板继续$2",
  );
}

/** Fallback:LLM 没按格式输出表格时合成 OVERRIDDEN_BY_USER 占位表格。
 *  设计依据:§10.1 v1 lessons learned + commit 7b983b6 教训。 */
function synthesizeOverriddenReport(reasonHint) {
  return `## PINVOU REVIEW REPORT

| Finding | Severity | Status | User Decision |
|---------|----------|--------|---------------|
| ${reasonHint || "Pinvou 输出未按表格格式,用户已阅读并 override"} | CRITICAL | OVERRIDDEN_BY_USER | 用户拍板继续 |

**VERDICT**: user override —— Pinvou 未按格式输出表格,用户已读完意见后强制放行`;
}

/** chat:done 后把 3 按钮附加到 Pinvou 气泡(气泡本身在 chat:delta 期间已经渲染成紫色)。
 *  - rowEl: 渲染好的 Pinvou row(data-pinvou-persona="pinvou-plan")
 *  - report: 提取到的 PINVOU REVIEW REPORT(null = LLM 没按格式输出,fallback 模式)
 *  - planCardEl: 关联的 plan card,3 按钮回调需要它的 dataset.planMarkdown */
function attachPinvouReviewActions(rowEl, report, planCardEl) {
  if (!rowEl) return;
  const wrap = rowEl.querySelector(".msg-wrap-pinvou");
  if (!wrap) return;
  // freeze 关联的 plan card(防止用户跳过 review 直接重点 ✅)
  if (planCardEl) {
    planCardEl.querySelectorAll(".plan-card-btn").forEach((b) => (b.disabled = true));
    const cardStatus = planCardEl.querySelector(".plan-card-status");
    if (cardStatus) {
      cardStatus.hidden = false;
      cardStatus.textContent = "⏸ 品悟已审,看下面按钮";
    }
  }
  const actions = document.createElement("div");
  actions.className = "pinvou-review-actions";
  actions.innerHTML = `
    <button class="pinvou-review-btn" data-action="accept" type="button">✅ 直接执行</button>
    <button class="pinvou-review-btn" data-action="revise" type="button">↻ AI 改方案</button>
    <button class="pinvou-review-btn" data-action="add" type="button">⊕ 我加一句</button>
  `;
  const status = document.createElement("div");
  status.className = "pinvou-review-status";
  status.hidden = true;
  wrap.appendChild(actions);
  wrap.appendChild(status);

  // accept 时用的 report:有 report 走 overrideAllCritical;无 report (fallback) 合成 placeholder
  const effectiveReport = report
    ? overrideAllCriticalInReport(report)
    : synthesizeOverriddenReport("Pinvou 用自然语言提了意见(见上方),用户阅读后决策");

  actions.querySelector('[data-action="accept"]').addEventListener("click", async () => {
    if (rowEl.dataset.resolved) return;
    rowEl.dataset.resolved = "true";
    actions.querySelectorAll("button").forEach((b) => (b.disabled = true));
    status.hidden = false;
    status.textContent = "👍 用户 override 所有 CRITICAL,继续执行...";
    if (!planCardEl || !activeSessionId) return;
    const planMd = planCardEl.dataset.planMarkdown || "";
    const fullMd = `${planMd}\n\n${effectiveReport}`;
    appendUserMessage("✅ 就这么干(品悟顾虑已 override)");
    messages.push({ role: "user", content: [{ type: "text", text: "✅ 就这么干(品悟顾虑已 override)" }] });
    setBusy(true);
    try {
      const state = await invoke("accept_plan", {
        sessionId: activeSessionId,
        planMarkdown: fullMd,
      });
      modeState = {
        mode: state.mode,
        plan_phase: state.plan_phase,
        pinvou_review_enabled: !!state.pinvou_review_enabled,
      };
      updateModeUI();
    } catch (e) {
      appendSystemMessage("⚠️ accept_plan 仍失败: " + e);
      setBusy(false);
    }
  });

  actions.querySelector('[data-action="revise"]').addEventListener("click", () => {
    if (rowEl.dataset.resolved) return;
    rowEl.dataset.resolved = "true";
    actions.querySelectorAll("button").forEach((b) => (b.disabled = true));
    status.hidden = false;
    status.textContent = "↻ 让 AI 改方案...";
    input.value = "修订方案: 按 Pinvou 上面提的 CRITICAL,改一下方案。";
    input.focus();
  });

  actions.querySelector('[data-action="add"]').addEventListener("click", () => {
    if (rowEl.dataset.resolved) return;
    rowEl.dataset.resolved = "true";
    actions.querySelectorAll("button").forEach((b) => (b.disabled = true));
    status.hidden = false;
    status.textContent = "⊕ 把你的话补进 plan,一起改...";
    input.value = "我也担心: ";
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
  });
}

// 标记:GATE 失败后等待 LLM 跑 review,chat:done 时用这个找回 plan card 引用
let pendingPinvouReview = null;

/** 共用底层:用简短摘要在前端显示 user 气泡 + 发完整 prompt 给后端。
 *  前端 messages 数组只存简短摘要(避免 SKILL.md 累积污染 history + 持久化)。
 *  后端 engine session 看到完整 prompt(本地小模型必须 eager loading)。 */
async function dispatchPinvouTrigger(persona, frontendSummary, fullPrompt) {
  if (!activeSessionId || busy) return;
  pendingAssistantPersona = persona; // 下一个 assistant 气泡用 Pinvou 样式
  appendUserMessage(frontendSummary);
  messages.push({
    role: "user",
    content: [{ type: "text", text: frontendSummary }],
  });
  setBusy(true);
  try {
    await invoke("chat", { message: fullPrompt, attachments: [] });
  } catch (err) {
    appendSystemMessage("⚠️ " + (err && err.toString ? err.toString() : err));
    setBusy(false);
  }
}

async function autoTriggerPinvouReview(planCardEl, reason) {
  pendingPinvouReview = { planCardEl };
  appendSystemMessage(`🟣 ${reason} —— 自动让品悟先看一眼...`);
  // 不靠 LLM 主动 read_file 加载 skill(本地 Qwen3.6 不会 progressive disclosure):
  // 直接从后端读 SKILL.md body 塞进 LLM context,user 气泡只显示简短摘要。
  let fullPrompt = "/pinvou-review-plan";
  try {
    const skillBody = await invoke("read_skill_body", { name: "pinvou-review-plan" });
    fullPrompt = `[品悟自动触发 /pinvou-review-plan,完整角色定义如下]\n\n${skillBody}`;
  } catch (e) {
    appendSystemMessage(`⚠️ 加载 pinvou-review-plan skill 失败: ${e}`);
  }
  await dispatchPinvouTrigger("pinvou-plan", "🟣 触发品悟审方案", fullPrompt);
}

// 任务收口 final review:advisory 性质,无 GATE 无 3 按钮。
let pendingFinalReview = false;

async function autoTriggerPinvouFinal() {
  pendingFinalReview = true;
  appendSystemMessage("🟣 任务完成 —— 让品悟核验一下产出...");
  let fullPrompt = "/pinvou-review-final";
  try {
    const skillBody = await invoke("read_skill_body", { name: "pinvou-review-final" });
    fullPrompt = `[品悟自动触发 /pinvou-review-final,完整角色定义如下]\n\n${skillBody}`;
  } catch (e) {
    appendSystemMessage(`⚠️ 加载 pinvou-review-final skill 失败: ${e}`);
  }
  await dispatchPinvouTrigger("pinvou-final", "🟣 触发品悟验收", fullPrompt);
}

/** 拼当前 assistant 完整 text(已 flush blocks + 未 flush pending)。chat:done 提取用。 */
function collectLastAssistantText() {
  const parts = [];
  for (const b of pendingAssistantBlocks) {
    if (b && b.type === "text" && typeof b.text === "string") {
      parts.push(b.text);
    }
  }
  if (pendingAssistantText) parts.push(pendingAssistantText);
  return parts.join("\n");
}

/** 解析 accept_plan/exit_plan_to_yolo 失败时后端返回的 GATE 错误(JSON 字符串)。
 *  非 JSON 返回 null(普通错误)。 */
function parseGateError(err) {
  const s = typeof err === "string" ? err : (err && err.toString ? err.toString() : "");
  if (!s.includes("gate_error")) return null;
  try { return JSON.parse(s); } catch { return null; }
}

// === Workflow toggle: 顶部品悟 review ON/OFF ===

async function togglePinvouReview() {
  if (!activeSessionId) return;
  const newEnabled = !modeState.pinvou_review_enabled;
  try {
    const state = await invoke("set_pinvou_review", {
      sessionId: activeSessionId,
      enabled: newEnabled,
    });
    modeState = {
      mode: state.mode,
      plan_phase: state.plan_phase,
      pinvou_review_enabled: !!state.pinvou_review_enabled,
    };
    updatePinvouReviewToggleUI();
  } catch (e) {
    appendSystemMessage("⚠️ set_pinvou_review 失败: " + e);
  }
}

function updatePinvouReviewToggleUI() {
  const btn = document.getElementById("pinvou-review-toggle");
  if (!btn) return;
  const enabled = !!modeState.pinvou_review_enabled;
  btn.dataset.enabled = enabled ? "true" : "false";
  btn.title = enabled
    ? "品悟 review: 开 (关键阶段让品悟把关,优化输出质量)"
    : "品悟 review: 关 (点击开启,关键阶段让品悟把关,优化输出质量)";
}

document.addEventListener("DOMContentLoaded", () => {
  const btn = document.getElementById("pinvou-review-toggle");
  if (btn) btn.addEventListener("click", togglePinvouReview);
});

// ════════════════════════════════════════════════════════════════════
// Pinvou Review v2 模块结束
// ════════════════════════════════════════════════════════════════════

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
// 一次性 flag:autoTriggerPinvouReview/Final 设置后,下一个 beginAssistantBubble
// 把 LLM 输出气泡渲染成品悟样式(紫色 + label "🟣 品悟")。消费后清空。
// 这样避免 v2 之前的"两次显示同一内容"bug(原 LLM 气泡 + 重新渲染品悟气泡)。
let pendingAssistantPersona = null; // null | "pinvou-plan" | "pinvou-final"

function beginAssistantBubble() {
  const row = document.createElement("div");
  const wrap = document.createElement("div");
  const label = document.createElement("div");
  const bubble = document.createElement("div");

  if (pendingAssistantPersona === "pinvou-plan" || pendingAssistantPersona === "pinvou-final") {
    row.className = "msg-row msg-pinvou";
    wrap.className = "msg-wrap msg-wrap-pinvou";
    label.className = "speaker-label speaker-pinvou";
    label.textContent = pendingAssistantPersona === "pinvou-plan"
      ? "🟣 品悟"
      : "🟣 品悟 · 任务验收";
    bubble.className = "bubble bubble-pinvou rendered";
    row.dataset.pinvouPersona = pendingAssistantPersona; // chat:done 据此附 3 按钮
    pendingAssistantPersona = null; // 一次性消费
  } else {
    row.className = "msg-row msg-assistant";
    wrap.className = "msg-wrap msg-wrap-assistant";
    label.className = "speaker-label speaker-assistant";
    const time = new Date().toTimeString().slice(0, 5);
    label.innerHTML = `<span class="label-name">QWEN3.6</span><span class="label-meta">· ${time}</span>`;
    bubble.className = "bubble bubble-assistant rendered";
  }
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
  // 流式期间末尾插入光标(每次重渲染都会被清掉,所以这里 append 回去)
  const cursor = document.createElement("span");
  cursor.className = "stream-cursor";
  currentAssistantBubble.appendChild(cursor);
  scrollToBottom();
}
function closeAssistantBubble() {
  if (currentAssistantBubble) {
    const cursor = currentAssistantBubble.querySelector(".stream-cursor");
    if (cursor) cursor.remove();
    enhanceCodeBlocks(currentAssistantBubble);
  }
  currentAssistantBubble = null;
  currentAssistantRawText = "";
  updateMessageActions();
}

/** 对 assistant bubble 内的 <pre><code> 包装一层 .code-block,加语言标签 + COPY 按钮。
 *  幂等:已包装的 pre 跳过。 */
function enhanceCodeBlocks(container) {
  if (!container) return;
  container.querySelectorAll("pre").forEach((pre) => {
    if (pre.parentElement?.classList.contains("code-block")) return;
    const code = pre.querySelector("code");
    const cls = code?.className || "";
    const m = cls.match(/language-(\S+)/);
    const lang = m ? m[1].toUpperCase() : "";
    const wrap = document.createElement("div");
    wrap.className = "code-block";
    const head = document.createElement("div");
    head.className = "code-block-head";
    const langEl = document.createElement("span");
    langEl.className = "code-block-lang";
    langEl.textContent = lang;
    const copyBtn = document.createElement("button");
    copyBtn.type = "button";
    copyBtn.className = "code-block-copy";
    copyBtn.textContent = "⧉ COPY";
    copyBtn.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(code?.textContent || pre.textContent || "");
        copyBtn.classList.add("copied");
        copyBtn.textContent = "✓ COPIED";
        setTimeout(() => {
          copyBtn.classList.remove("copied");
          copyBtn.textContent = "⧉ COPY";
        }, 1400);
      } catch (e) {
        console.warn("clipboard write failed", e);
      }
    });
    head.appendChild(langEl);
    head.appendChild(copyBtn);
    pre.parentNode.insertBefore(wrap, pre);
    wrap.appendChild(head);
    wrap.appendChild(pre);
  });
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
  resetPendingAssistant();
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
const ARG_PRIMARY_FIELDS = ["prompt", "code", "command", "query", "path", "content", "text", "url"];
const OUT_PRIMARY_FIELDS = ["stdout", "output", "result", "content", "text", "summary", "note", "message", "error"];

// subagent 工具家族 - 用专用样式区分(🤖 蓝色边框),让用户一眼识别"子 agent 在跑"
const SUBAGENT_TOOL_NAMES = new Set([
  "agent_open",
  "agent_spawn",
  "agent_eval",
  "agent_result",
  "agent_cancel",
  "agent_close",
  "agent_list",
  "resume_agent",
  "delegate_to_agent",
]);
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
  const isSubagent = SUBAGENT_TOOL_NAMES.has(name);
  // subagent 工具加 tool-subagent class → CSS 用蓝色边框 + 浅蓝背景,跟普通工具视觉区分
  card.className = isSubagent ? "tool-card tool-running tool-subagent" : "tool-card tool-running";
  card.dataset.toolId = id;
  const iconMap = {
    read_file: "📄", write_file: "📝", edit_file: "✏️", list_dir: "📁",
    file_search: "🔎", grep_files: "🔎",
    web_search: "🌐", fetch_url: "🌐", web_run: "🌐",
    exec_shell: "💻", exec_shell_wait: "💻", exec_shell_interact: "💻",
    code_execution: "🐍",
    update_plan: "📋", todo_write: "✅", checklist_write: "✅",
    request_user_input: "💬",
    // subagent 工具家族统一 🤖,跟主线工具区分。具体动词 (open/eval/close) 仍在 name 里显示
    agent_open: "🤖", agent_spawn: "🤖", agent_eval: "🤖", agent_result: "🤖",
    agent_cancel: "🤖", agent_close: "🤖", agent_list: "🤖",
    resume_agent: "🤖", delegate_to_agent: "🤖",
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
  toolMeta.clear();
  userInputCards.clear();
  closeAssistantBubble();
  resetPendingAssistant();
  const row = document.createElement("div");
  row.className = "msg-row msg-system";
  row.innerHTML = `<div class="bubble bubble-system"><span class="bubble-system-prefix">PINVOU3 · READY</span><br/>${escapeHtml(i18nText("chat.welcome"))}</div>`;
  chatArea.appendChild(row);
}

/** 用 state.messages 重渲染对话区（切换 session 时用）。 */
function rerenderFromMessages() {
  clearChatDOM();
  // turn 边界: 一个 turn 由 user.text message 开启, 累积该 turn 内的 plan/todos snapshot,
  // 在下一个 user.text 出现前(或遍历结束)渲染 1 个 plan_card. 跟实时 chat:plan_ready
  // 在 turn 末尾 emit 一次的行为对齐, 避免历史还原显示 N 个 plan_card.
  let lastPlanSnap = null;
  let lastTodosSnap = null;
  let pendingPlanCard = false;
  // 当前 turn 是否 Executing (accept_plan 触发的 turn): user.text 以"✅"开头视为 accept.
  // Executing turn 末尾即使有 update_plan(标 completed) 也不渲染新 plan_card —— 跟实时一致.
  let currentTurnIsExecuting = false;

  function flushHistoricAssistantText(text) {
    if (!text) return;
    beginAssistantBubble();
    currentAssistantRawText = text;
    currentAssistantBubble.innerHTML = renderMarkdown(text);
    closeAssistantBubble();
  }
  function flushPendingPlanCard() {
    if (!currentTurnIsExecuting && pendingPlanCard && (lastPlanSnap || lastTodosSnap)) {
      renderHistoricPlanCard(lastPlanSnap, lastTodosSnap);
    }
    pendingPlanCard = false;
    lastPlanSnap = null;
    lastTodosSnap = null;
  }

  for (const m of messages) {
    const blocks = Array.isArray(m.content) ? m.content : [];
    if (m.role === "user") {
      const textParts = blocks.filter((c) => c.type === "text").map((c) => c.text);
      if (textParts.length) {
        flushPendingPlanCard();
        const userText = textParts.join("");
        appendUserMessage(userText);
        currentTurnIsExecuting = userText.startsWith("✅");
      }
      // tool_result 段(同 turn 内): 不算边界, 更新对应历史 tool card 状态
      for (const c of blocks) {
        if (c.type !== "tool_result") continue;
        applyHistoricToolResult(c.tool_use_id, c.content, c.is_error === true);
      }
      continue;
    }
    if (m.role !== "assistant") continue;
    // assistant: 按 block 顺序遍历 text + tool_use
    let textBuf = "";
    for (const b of blocks) {
      if (b.type === "text") {
        textBuf += b.text;
      } else if (b.type === "thinking") {
        // 跳过
      } else if (b.type === "tool_use") {
        flushHistoricAssistantText(textBuf);
        textBuf = "";
        // 累积 plan snapshot 但不立即渲染卡片, 等 turn 结束
        if (b.name === "update_plan") {
          lastPlanSnap = b.input;
          pendingPlanCard = true;
        } else if (b.name === "checklist_write" || b.name === "todo_write") {
          lastTodosSnap = b.input;
          pendingPlanCard = true;
        }
        renderHistoricToolUse(b);
      }
    }
    flushHistoricAssistantText(textBuf);
  }
  // 最后一个 turn 末尾(没有下一个 user.text 触发)
  flushPendingPlanCard();
  // 还在 pending 的历史 tool card → turn 被中断未拿到 tool_result, 标灰让用户看清.
  for (const [id, meta] of Array.from(toolMeta.entries())) {
    if (
      meta.name === "request_user_input" ||
      meta.name === "update_plan" ||
      meta.name === "checklist_write" ||
      meta.name === "todo_write"
    ) {
      toolMeta.delete(id);
      continue;
    }
    appendToolCallEnd(id, "(无返回 · 可能被中断)", false);
    toolMeta.delete(id);
  }
  scrollToBottom();
}

// 历史还原: tool_use → tool card(pending 态). update_plan/request_user_input 走专用 card.
function renderHistoricToolUse(block) {
  const { id, name, input } = block;
  toolMeta.set(id, { name, args: input });
  if (name === "request_user_input") {
    const questions = (input || {}).questions || [];
    if (questions.length) renderUserInputCard(id, questions);
    return;
  }
  if (name === "update_plan" || name === "checklist_write" || name === "todo_write") {
    // 不渲染通用 tool card, plan_card 已经接管. 但仍 toolMeta 占位让 tool_result 应用时不报警告.
    return;
  }
  appendToolCallStart(id, name, input);
}

function applyHistoricToolResult(toolUseId, content, isError) {
  const meta = toolMeta.get(toolUseId);
  if (!meta) {
    // 老 session 可能没匹配上 (tool_use 缺失), 静默忽略
    return;
  }
  if (meta.name === "request_user_input") {
    finalizeUserInputCard(toolUseId, !isError);
    toolMeta.delete(toolUseId);
    return;
  }
  if (meta.name === "update_plan" || meta.name === "checklist_write" || meta.name === "todo_write") {
    // 已由 renderHistoricPlanCard 接管, 不需要 tool card 更新
    toolMeta.delete(toolUseId);
    return;
  }
  if (!isError && meta.name === "write_file") {
    const path = extractArtifactPath(meta.args);
    if (path) trackArtifact(path);
  }
  appendToolCallEnd(toolUseId, content, !isError);
  toolMeta.delete(toolUseId);
}

// 历史 plan_card: 重用 renderPlanReadyCard 但立即 freeze, 标"历史方案".
function renderHistoricPlanCard(planInput, todosInput) {
  const snapshots = {};
  if (planInput) {
    // update_plan tool input 字段叫 "plan"(见 DeepSeek-TUI plan.rs schema),
    // 但实时 chat:plan_ready 的 PlanSnapshot 字段叫 "items". 两种 shape 都兼容.
    const planItems = planInput.plan || planInput.items || [];
    snapshots.plan = {
      explanation: planInput.explanation,
      items: Array.isArray(planItems) ? planItems : [],
    };
  }
  if (todosInput) {
    const todos = todosInput.todos || todosInput.items || [];
    snapshots.todos = { items: Array.isArray(todos) ? todos : [] };
  }
  if (!snapshots.plan && !snapshots.todos) return;
  renderPlanReadyCard(snapshots);
  const card = chatArea.querySelector('.msg-plan-card[data-card-state="active"]:last-of-type')
    || chatArea.querySelector('.msg-plan-card[data-card-state="active"]');
  if (card) setPlanCardFrozen(card, "📜 历史方案");
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
    title.addEventListener("click", () => {
      switchView("chatroom");
      switchToSession(meta.id);
    });

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
  const ok = await appConfirm(
    i18nText("history.confirm_delete") + "\n\n" + (meta.title || meta.id),
    { title: i18nText("history.delete"), kind: "warning" }
  );
  if (!ok) return;
  try {
    await invoke("delete_session", { id: meta.id });
    sessionsCache = sessionsCache.filter((m) => m.id !== meta.id);
    if (activeSessionId === meta.id) {
      // 删的是当前 session：优先切到剩余最新一条；都没了再建空 session
      activeSessionId = null;
      messages = [];
      resetPendingAssistant();
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
    resetPendingAssistant();
    artifacts = [];
    activeArtifactPath = null;
    renderArtifactList();
    clearUnreadArtifacts();
    lastInputTokens = 0;
    updateTokenBar(0);
    if (artifactPreviewEl) artifactPreviewEl.innerHTML = "";
    clearChatDOM();
    await refreshHistoryList();
    await syncModeStateFromBackend();
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
    const ok = await appConfirm("当前对话还在响应,打断并切换?", {
      title: "切换对话",
      kind: "warning",
    });
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
    resetPendingAssistant();
    // 从 SavedSession.artifacts 重建前端产物列表 (storage_path 是绝对路径)
    const savedArtifacts = Array.isArray(saved.artifacts) ? saved.artifacts : [];
    artifacts = savedArtifacts.map((a) => {
      const path = a.storage_path || a.path || "";
      return {
        path,
        basename: path.split(/[\\/]/).pop() || path,
        created_at: Date.parse(a.created_at) || Date.now(),
        updated_at: Date.parse(a.created_at) || Date.now(),
      };
    });
    activeArtifactPath = null;
    renderArtifactList();
    clearUnreadArtifacts();
    lastInputTokens = 0;
    updateTokenBar(0);
    if (artifactPreviewEl) artifactPreviewEl.innerHTML = "";
    rerenderFromMessages();
    renderHistoryList();
    await syncModeStateFromBackend();
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

/** 把当前 messages + artifacts 落盘到后端（每轮 TurnComplete 调用一次）。 */
async function persistMessages() {
  if (!activeSessionId) return;
  try {
    await invoke("save_session_messages", { id: activeSessionId, messages });
    // artifacts 一起落盘,重启 / 切换 session 后能恢复
    await invoke("save_session_artifacts", {
      id: activeSessionId,
      paths: artifacts.map((a) => a.path),
    });
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

newSessionBtn?.addEventListener("click", () => {
  switchView("chatroom");
  createNewSession();
});

// ── 阶段 C: 右栏产物面板 ──────────────────────────────────────────

const rightPane = document.getElementById("right-pane");
const rightPaneToggle = document.getElementById("right-pane-toggle");
const rightPaneBadge = document.getElementById("right-pane-badge");
const artifactListEl = document.getElementById("artifact-list");
const artifactPreviewEl = document.getElementById("artifact-preview");
let activeArtifactPath = null;
let unreadArtifacts = 0;

// 阶段 C: token 进度条 —— 上下文使用率监控
let maxModelLen = 32768;     // 兜底值，monitor 拉到真实 max_model_len 后覆盖
let lastInputTokens = 0;     // 最近一轮 TurnComplete.usage.input_tokens

/** 右栏是否「正在可见地展示产物」——展开 + 产物 tab 激活。
 *  dataset.collapsed 默认 "auto": 大窗下 CSS 自动显示, 小窗下 overlay 隐藏.
 *  之前只认 "false" 漏掉 "auto+大窗" 这个**默认显示**场景 → badge 永不清零. */
function isArtifactsTabVisible() {
  const collapsed = rightPane?.dataset.collapsed || "auto";
  const expanded = collapsed === "false"
    || (collapsed === "auto" && window.innerWidth > 1280);
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
  if (isArtifactsTabVisible()) { clearUnreadArtifacts(); return; } // 正在看 → 顺手清掉历史残留
  unreadArtifacts += 1;
  renderUnreadBadge();
}

// 窗口尺寸变化时(小窗→大窗) auto 状态自动展开右栏 → 清未读
window.addEventListener("resize", () => {
  if (isArtifactsTabVisible()) clearUnreadArtifacts();
});

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
  const ok = await appConfirm(
    "立即压缩当前对话上下文？\n\n早期消息会被摘要替换,无法恢复。",
    { title: "压缩上下文", kind: "warning" }
  );
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

/** 把一个产物路径加入 state.artifacts。重复 path 的覆盖（最近一次写为准）。
 *  badge unread 只在 path 真新增时 bump,避免 file watcher 多次 Modify 事件
 *  把计数刷到天上去。 */
function trackArtifact(path) {
  const existing = artifacts.findIndex((a) => a.path === path);
  const basename = path.split(/[\\/]/).pop() || path;
  const entry = existing >= 0
    ? { ...artifacts[existing], updated_at: Date.now() }
    : { path, basename, created_at: Date.now(), updated_at: Date.now() };
  if (existing >= 0) artifacts.splice(existing, 1);
  artifacts.unshift(entry);
  renderArtifactList();
  if (existing < 0) bumpUnreadArtifacts();
}

/** 从 artifacts 数组中移除一条 (file watcher Remove 事件触发)。 */
function untrackArtifact(path) {
  const idx = artifacts.findIndex((a) => a.path === path);
  if (idx < 0) return;
  artifacts.splice(idx, 1);
  // 如果删的是当前正在预览的,清空预览区
  if (activeArtifactPath === path) {
    activeArtifactPath = null;
    if (artifactPreviewEl) artifactPreviewEl.innerHTML = "";
  }
  renderArtifactList();
}

/** 外部打开产物: HTML 走 Tauri 新 webview 窗口 (绕 snap 浏览器对 ~/.xxx/ 的沙箱限制),
 *  其他类型走 xdg-open 调系统应用。 */
function openArtifactExternal(path) {
  const ext = (path.split(".").pop() || "").toLowerCase();
  const cmd = (ext === "html" || ext === "htm") ? "open_artifact_window" : "open_in_system";
  return invoke(cmd, { path }).catch((err) => {
    appendSystemMessage("⚠️ 打开失败: " + err);
  });
}

/** 用文件管理器打开产物**所在目录**（不是文件本身）。 */
function openArtifactFolder(path) {
  return invoke("open_containing_folder", { path }).catch((err) => {
    appendSystemMessage("⚠️ 打开目录失败: " + err);
  });
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
    const folderBtn = document.createElement("button");
    folderBtn.className = "artifact-open-btn";
    folderBtn.type = "button";
    folderBtn.title = "打开所在目录";
    folderBtn.textContent = "📂";
    folderBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      openArtifactFolder(a.path);
    });
    const openBtn = document.createElement("button");
    openBtn.className = "artifact-open-btn";
    openBtn.type = "button";
    openBtn.title = i18nText("pane.preview.open_external");
    openBtn.textContent = "↗";
    openBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      openArtifactExternal(a.path);
    });

    li.appendChild(iconEl);
    li.appendChild(nameEl);
    li.appendChild(folderBtn);
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
      // iframe 是独立 document,主文档的 contextmenu listener 管不到,
      // 必须把禁右键脚本注入 srcdoc 顶部 (用户右键 reload 会刷掉所有前端状态)
      const guard = `<script>document.addEventListener('contextmenu',function(e){e.preventDefault();})<\/script>`;
      iframe.srcdoc = guard + text;
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
      openArtifactExternal(a.path);
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
      openArtifactExternal(a.path);
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

// ── 右栏宽度拖拽 (大窗下生效, 浮层模式禁用) ─────────────────────
// 拖左边缘 → 改 --right-pane-w → 落 localStorage
const RIGHT_PANE_W_KEY = "pinvou3.rightPaneWidth";
const RIGHT_PANE_W_MIN = 240;
const rightPaneResizer = document.getElementById("right-pane-resizer");

function applyRightPaneWidth(px) {
  // 大窗 60vw 上限留主对话区呼吸空间; 小窗 overlay 模式允许 90vw (CSS 那一侧再加一道 90vw 兜底)
  const ratio = window.innerWidth > 1280 ? 0.6 : 0.9;
  const maxW = Math.max(RIGHT_PANE_W_MIN, Math.floor(window.innerWidth * ratio));
  const clamped = Math.min(maxW, Math.max(RIGHT_PANE_W_MIN, Math.round(px)));
  document.documentElement.style.setProperty("--right-pane-w", `${clamped}px`);
  return clamped;
}

// 启动时恢复
try {
  const saved = parseInt(localStorage.getItem(RIGHT_PANE_W_KEY) || "", 10);
  if (Number.isFinite(saved)) applyRightPaneWidth(saved);
} catch {}

// 自模拟双击: 两次 mousedown 间隔 < 350ms 且都没拖动 → 重置默认。
// 不用原生 dblclick: resizer 只有 6px 宽,双击微移到相邻元素时不在同一 target 不触发。
let lastResizerMouseDownTs = 0;
rightPaneResizer?.addEventListener("mousedown", (e) => {
  e.preventDefault();
  const now = Date.now();
  const isDoubleClick = now - lastResizerMouseDownTs < 350;
  lastResizerMouseDownTs = now;
  if (isDoubleClick) {
    applyRightPaneWidth(360);
    try { localStorage.removeItem(RIGHT_PANE_W_KEY); } catch {}
    lastResizerMouseDownTs = 0; // 防三连击再次触发
    return;
  }
  const startX = e.clientX;
  const startW = rightPane.getBoundingClientRect().width;
  let dragged = false;
  document.body.classList.add("is-resizing-right-pane");
  const onMove = (ev) => {
    const dx = ev.clientX - startX;
    if (Math.abs(dx) > 2) dragged = true;
    applyRightPaneWidth(startW - dx);
  };
  const onUp = () => {
    document.removeEventListener("mousemove", onMove);
    document.removeEventListener("mouseup", onUp);
    document.body.classList.remove("is-resizing-right-pane");
    // 没拖动时不写 localStorage(避免单击 = 落值 = 让宽度恢复成"刚才偶然的尺寸")
    if (dragged) {
      const cur = rightPane.getBoundingClientRect().width;
      try { localStorage.setItem(RIGHT_PANE_W_KEY, String(Math.round(cur))); } catch {}
    }
  };
  document.addEventListener("mousemove", onMove);
  document.addEventListener("mouseup", onUp);
});

// ── Sidebar 宽度拖拽 ──────────────────────────────────────────────
// 拖右边缘 → 改 --sidebar-w → 落 localStorage。同一套模式 follow right-pane。
const SIDEBAR_W_KEY = "pinvou3.sidebarWidth";
const SIDEBAR_W_MIN = 160;
const SIDEBAR_W_DEFAULT = 220;
const sidebarEl = document.querySelector(".sidebar");
const sidebarResizer = document.getElementById("sidebar-resizer");

function applySidebarWidth(px) {
  // 上限 40vw 留出对话区
  const maxW = Math.max(SIDEBAR_W_MIN, Math.floor(window.innerWidth * 0.4));
  const clamped = Math.min(maxW, Math.max(SIDEBAR_W_MIN, Math.round(px)));
  document.documentElement.style.setProperty("--sidebar-w", `${clamped}px`);
  return clamped;
}

try {
  const saved = parseInt(localStorage.getItem(SIDEBAR_W_KEY) || "", 10);
  if (Number.isFinite(saved)) applySidebarWidth(saved);
} catch {}

let lastSidebarMouseDownTs = 0;
sidebarResizer?.addEventListener("mousedown", (e) => {
  e.preventDefault();
  const now = Date.now();
  const isDoubleClick = now - lastSidebarMouseDownTs < 350;
  lastSidebarMouseDownTs = now;
  if (isDoubleClick) {
    applySidebarWidth(SIDEBAR_W_DEFAULT);
    try { localStorage.removeItem(SIDEBAR_W_KEY); } catch {}
    lastSidebarMouseDownTs = 0;
    return;
  }
  const startX = e.clientX;
  const startW = sidebarEl.getBoundingClientRect().width;
  let dragged = false;
  document.body.classList.add("is-resizing-sidebar");
  const onMove = (ev) => {
    const dx = ev.clientX - startX;
    if (Math.abs(dx) > 2) dragged = true;
    applySidebarWidth(startW + dx);
  };
  const onUp = () => {
    document.removeEventListener("mousemove", onMove);
    document.removeEventListener("mouseup", onUp);
    document.body.classList.remove("is-resizing-sidebar");
    if (dragged) {
      const cur = sidebarEl.getBoundingClientRect().width;
      try { localStorage.setItem(SIDEBAR_W_KEY, String(Math.round(cur))); } catch {}
    }
  };
  document.addEventListener("mousemove", onMove);
  document.addEventListener("mouseup", onUp);
});

// ── 阶段 C: 输入栏多文件上传 ──────────────────────────────────────

const attachBtn = document.getElementById("attach-btn");
const attachmentRow = document.getElementById("attachment-row");

// 阶段 D: Plan/YOLO 双模式 UI 引用
const planBtn = document.getElementById("plan-btn");
const composerEl = document.getElementById("composer");
const modeChipRow = document.getElementById("mode-chip-row");
const modeChipText = document.getElementById("mode-chip-text");
const modeChipAction = document.getElementById("mode-chip-action");
const modeChipJump = document.getElementById("mode-chip-jump");
const modeChipProgress = document.getElementById("mode-chip-progress");

// 缓存最新的 plan/todos snapshot(由 chat:plan_snapshot event 更新)
// chip 进度条渲染 + accept_plan 时拼 plan_markdown 用。
// 各带 _ts(时间戳),pickProgressItems 选较新的渲染——避免 AI 在 Executing 调
// update_plan 更新 plan 后,chip 仍显示 Planning 阶段的老 todos 数据。
let latestPlanSnapshot = null;
let latestPlanSnapshotTs = 0;
let latestTodosSnapshot = null;
let latestTodosSnapshotTs = 0;
let planProgressExpanded = false;

/**
 * 根据 modeState 同步所有视觉锚点：
 *   - composer[data-plan-phase]
 *   - plan-btn[data-active / disabled / title]
 *   - mode-chip-row 显隐 + 文案 + ⚡ 退出按钮
 */
function updateModeUI() {
  const { mode, plan_phase } = modeState;
  composerEl.dataset.planPhase = plan_phase;
  // plan-btn 三态:
  //   YOLO/none → 可点(进入 Plan), hover "进入 Plan 模式"
  //   Plan/planning → 可点(退出 Plan, 等价 chip 上的 ⚡ 直接动手), hover "退出 Plan 模式"
  //   Plan/ready → disabled(等用户在 plan_card 上决策, 灯泡误触会丢失方案上下文)
  //   YOLO/executing → 可点(二次确认中断 + 重开 Plan)
  planBtn.dataset.active = mode === "plan" ? "true" : "false";
  planBtn.disabled = plan_phase === "ready";
  if (plan_phase === "ready") {
    planBtn.title = "请先在卡片上决策方案";
  } else if (mode === "plan") {
    planBtn.title = "退出 Plan 模式";
  } else if (plan_phase === "executing") {
    planBtn.title = "中断当前 · 重开 Plan";
  } else {
    planBtn.title = "进入 Plan 模式";
  }

  // thinking 前缀:busy 时 "⠋ Ns · " 拼在文案最前,按 phase 切换文字。
  // tool phase 时显示具体工具名;thinking phase 时默认"思考中"(下面 fallback chip 用)。
  // (thinking 反馈走独立气泡 renderThinkingBubble,不再拼到 chip 文案)

  // chip 显示逻辑——统一状态条:模式标签 + thinking 反馈 + 操作按钮
  modeChipJump.hidden = true;
  modeChipProgress.hidden = true;  // V4 简化:chip 不再显示 plan 进度列表(冗余 + AI 行为不可靠)

  if (plan_phase === "planning") {
    modeChipRow.hidden = false;
    // 首句价值描述: 让用户搞懂"为啥要 Plan + 何时该用",降低误触成本。
    modeChipText.textContent = busy
      ? `💡 Plan 模式 · 讨论中`
      : `💡 Plan 模式：让 AI 先列方案再执行，复杂任务且 没想好时开启`;
    modeChipAction.hidden = false;
    modeChipAction.textContent = "⚡ 直接动手";
    modeChipAction.dataset.kind = "exit_plan";
  } else if (plan_phase === "ready") {
    modeChipRow.hidden = false;
    modeChipText.textContent = `✨ AI 给出方案 · 看下面卡片决策`;
    modeChipAction.hidden = true;
    modeChipJump.hidden = false;  // 卡片可能滚出视口,给跳转按钮
  } else if (plan_phase === "executing") {
    modeChipRow.hidden = false;
    modeChipText.textContent = `🏃 执行中`;
    // 中断走输入框 ⏹️ (业界惯例), chip 仅作状态显示
    modeChipAction.hidden = true;
  } else {
    // YOLO/none: chip 完全隐藏. busy 时 thinking 气泡(消息流尾部)承担反馈, 不借壳 chip.
    modeChipRow.hidden = true;
    modeChipAction.hidden = true;
    delete modeChipAction.dataset.kind;
  }
}

/** 选取进度展示用的 items:挑**最近更新**的那个 snapshot(plan vs todos)。
 *  归一化成 {label, status} 让 renderChipProgress 不关心来源。
 *  时间戳来自 chat:plan_snapshot event 到达时间,而非工具调用时间——足够准确。 */
function pickProgressItems() {
  const planValid = latestPlanSnapshot && Array.isArray(latestPlanSnapshot.items) && latestPlanSnapshot.items.length > 0;
  const todosValid = latestTodosSnapshot && Array.isArray(latestTodosSnapshot.items) && latestTodosSnapshot.items.length > 0;
  if (!planValid && !todosValid) return [];
  // 都有效 → 选时间戳新的;只有一个有效 → 用它
  const useTodos = todosValid && (!planValid || latestTodosSnapshotTs >= latestPlanSnapshotTs);
  if (useTodos) {
    return latestTodosSnapshot.items.map((i) => ({
      label: i.content || "",
      status: i.status || "pending",
    }));
  }
  return latestPlanSnapshot.items.map((i) => ({
    label: i.step || "",
    status: i.status || "pending",
  }));
}

/** 渲染 chip 进度列表。默认 5 行可见(completed 折叠到最近 1 + in_progress + pending 前 3),
 *  超过弹 +n more 折叠按钮。expanded 状态显示全部。 */
function renderChipProgress(items) {
  if (items.length === 0) {
    modeChipProgress.hidden = true;
    return;
  }
  modeChipProgress.hidden = false;
  modeChipProgress.classList.toggle("expanded", planProgressExpanded);
  const display = planProgressExpanded ? items : compactItems(items);
  const ol = document.createElement("ol");
  for (const it of display) {
    const li = document.createElement("li");
    li.dataset.status = it.status;
    const sym = it.status === "completed" ? "●" : it.status === "in_progress" ? "◎" : "○";
    li.innerHTML = `<span class="mode-chip-progress-sym"></span><span class="mode-chip-progress-text"></span>`;
    li.querySelector(".mode-chip-progress-sym").textContent = sym;
    li.querySelector(".mode-chip-progress-text").textContent = it.label;
    ol.appendChild(li);
  }
  modeChipProgress.innerHTML = "";
  modeChipProgress.appendChild(ol);
  // 折叠/展开按钮
  if (!planProgressExpanded && items.length > display.length) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "mode-chip-progress-toggle";
    btn.textContent = `+${items.length - display.length} 更多 ▾`;
    btn.addEventListener("click", () => {
      planProgressExpanded = true;
      renderChipProgress(items);
    });
    modeChipProgress.appendChild(btn);
  } else if (planProgressExpanded && items.length > 5) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "mode-chip-progress-toggle";
    btn.textContent = "折叠 ▴";
    btn.addEventListener("click", () => {
      planProgressExpanded = false;
      renderChipProgress(items);
    });
    modeChipProgress.appendChild(btn);
  }
}

/** 紧凑模式:completed 已折叠到最近 1 个 + in_progress 全部 + pending 前 3 个。最多 5 行。 */
function compactItems(items) {
  if (items.length <= 5) return items;
  const lastCompletedIdx = items.findLastIndex
    ? items.findLastIndex((i) => i.status === "completed")
    : (() => {
        for (let i = items.length - 1; i >= 0; i--) {
          if (items[i].status === "completed") return i;
        }
        return -1;
      })();
  const result = [];
  if (lastCompletedIdx >= 0) result.push(items[lastCompletedIdx]);
  const rest = items.filter(
    (i, idx) => idx !== lastCompletedIdx && i.status !== "completed"
  );
  for (const it of rest) {
    if (result.length >= 5) break;
    result.push(it);
  }
  return result;
}

/** 切换/新建 session 后从后端拉最新 mode_state，UI 同步。 */
async function syncModeStateFromBackend() {
  // 切 session 必清 snapshot:plan/todos 是 per-session in-memory,前 session 数据不该跨界
  latestPlanSnapshot = null;
  latestTodosSnapshot = null;
  planProgressExpanded = false;
  if (!activeSessionId) {
    modeState = { mode: "yolo", plan_phase: "none", pinvou_review_enabled: false };
    updateModeUI();
    updatePinvouReviewToggleUI();
    return;
  }
  try {
    const state = await invoke("get_mode_state", { sessionId: activeSessionId });
    modeState = {
      mode: state.mode || "yolo",
      plan_phase: state.plan_phase || "none",
      pinvou_review_enabled: !!state.pinvou_review_enabled,
    };
  } catch (e) {
    console.warn("get_mode_state failed", e);
    modeState = { mode: "yolo", plan_phase: "none", pinvou_review_enabled: false };
  }
  updateModeUI();
  updatePinvouReviewToggleUI();
}

// plan-btn 点击 (toggle):
//   YOLO/none → 进入 Plan
//   Plan/planning → 退出 Plan (等价 chip 的 ⚡ 直接动手, 保留对话历史回 YOLO)
//   YOLO/executing → 二次确认中断 + 重开 Plan
//   Plan/ready → disabled (前置拦截)
planBtn.addEventListener("click", async () => {
  if (planBtn.disabled) return;
  if (!activeSessionId) {
    await createNewSession();
    if (!activeSessionId) return;
  }
  // Plan + planning 已激活 → toggle 退出 (busy 时由 exitPlanFlow 内部先 cancel_generation)
  if (modeState.mode === "plan" && modeState.plan_phase === "planning") {
    await exitPlanFlow();
    return;
  }
  // 执行中 → 二次确认
  if (modeState.plan_phase === "executing") {
    const ok = await appConfirm("当前任务还在执行,中断并开启新的 Plan?", {
      title: "中断当前",
      kind: "warning",
    });
    if (!ok) return;
    await cancelActiveTurn();
  }
  try {
    const state = await invoke("set_plan_mode_next", { sessionId: activeSessionId });
    modeState = { mode: state.mode, plan_phase: state.plan_phase };
    updateModeUI();
  } catch (e) {
    appendSystemMessage("⚠️ 进入 Plan 模式失败: " + e);
  }
});

// chip [📌 跳到卡片] 按钮:Ready 态卡片可能滚出视口,提供 scrollIntoView 路径
modeChipJump.addEventListener("click", () => {
  const activeCard = chatArea.querySelector('.msg-plan-card[data-card-state="active"]');
  if (activeCard) {
    activeCard.scrollIntoView({ behavior: "smooth", block: "center" });
    activeCard.classList.add("plan-card-pulse");
    setTimeout(() => activeCard.classList.remove("plan-card-pulse"), 1200);
  }
});

// 统一中断 helper: 所有"中断/停止"路径必须先调它,确保 turn 真的能跳出。
//   1. 取消所有 active request_user_input 卡片
//      —— engine 在 await_user_input 的 oneshot 上阻塞时, cancel_generation 不一定唤醒它,
//         前端必须主动 cancel_user_input 才能让 turn 跳出
//   2. 取消整个 turn (cancel_generation)
//   3. Executing 态时自动 discard_plan 同步前后端
//      —— 业界惯例 ⏹️ 仅停生成,但 Executing 是 plan 接受后的"YOLO 干活态",
//         用户停下后期望整个任务结束(否则后端 mode 错位到下一轮 chat)。
// Planning 态保持 X1 不改 mode (用户停下后仍在 Plan 模式可继续讨论).
async function cancelActiveTurn() {
  for (const id of Array.from(userInputCards.keys())) {
    try { await invoke("cancel_user_input", { toolCallId: id }); } catch (_) {}
    finalizeUserInputCard(id, false);
  }
  if (busy) {
    try { await invoke("cancel_generation"); } catch (_) {}
  }
  if (modeState.plan_phase === "executing" && activeSessionId) {
    try {
      const state = await invoke("discard_plan", { sessionId: activeSessionId });
      modeState = { mode: state.mode, plan_phase: state.plan_phase };
      updateModeUI();
    } catch (_) {}
  }
}

// 退出 Plan 公共流程: cancelActiveTurn 兜底, 再切 mode 回 Yolo/None.
// chip [⚡ 直接动手] 和灯泡 toggle 共用。
async function exitPlanFlow() {
  if (!activeSessionId) return;
  await cancelActiveTurn();
  try {
    const state = await invoke("exit_plan_to_yolo", { sessionId: activeSessionId });
    modeState = { mode: state.mode, plan_phase: state.plan_phase };
    updateModeUI();
  } catch (e) {
    appendSystemMessage("⚠️ 退出 Plan 失败: " + e);
  }
}

// chip ⚡ 按钮：仅承载 "⚡ 直接动手" (Planning 态退出 Plan).
// Executing 态的中断走输入框 ⏹️ (cancelActiveTurn 内置 executing→discard 同步).
modeChipAction.addEventListener("click", async () => {
  if (!activeSessionId) return;
  if (modeChipAction.dataset.kind === "exit_plan") {
    await exitPlanFlow();
  }
});
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


// ── Tauri 事件订阅 ────────────────────────────────────────────────
listen("chat:delta", (e) => {
  const text = e.payload?.text || "";
  pendingAssistantText += text;
  appendAssistantDelta(text);
});
listen("chat:tool_start", (e) => {
  const { id, name, args } = e.payload || {};
  if (!toolMeta.has(id) || args != null) {
    toolMeta.set(id, { name, args });
  }
  if (name) switchThinkingToTool(name);
  // 持久化: tool_use 是当前 assistant message 的一部分. 先 flush text 再 push tool_use.
  flushPendingTextBlock();
  pendingAssistantBlocks.push({
    type: "tool_use",
    id,
    name,
    input: args || {},
  });
  // request_user_input: 不渲染默认 tool card,等 chat:user_input_required event 单独渲染选择气泡.
  if (name === "request_user_input") return;
  appendToolCallStart(id, name, args);
});
listen("chat:tool_end", (e) => {
  const { id, output, success, metadata } = e.payload || {};
  const meta = toolMeta.get(id);
  switchThinkingToIdle();
  // 持久化: tool_result 是新的 user message. 先 flush 当前 assistant message, 再 push user message.
  // closeAssistantBubble 防止后续 text 串到工具前的 bubble 里.
  const resultContent = typeof output === "string" ? output : JSON.stringify(output);
  flushAssistantMessageToHistory();
  closeAssistantBubble();
  const trBlock = { type: "tool_result", tool_use_id: id, content: resultContent };
  if (!success) trBlock.is_error = true;
  messages.push({ role: "user", content: [trBlock] });
  // request_user_input 结束: 关闭选择气泡, 不调 appendToolCallEnd.
  if (meta && meta.name === "request_user_input") {
    finalizeUserInputCard(id, success);
    toolMeta.delete(id);
    return;
  }
  appendToolCallEnd(id, output, success);
  // Careful hook: DeepSeek-TUI shell.rs 拦截 Dangerous → 渲染红色卡片
  if (metadata && metadata.safety_level === "dangerous" && metadata.blocked) {
    renderCarefulBlockedCard(meta && meta.args, metadata);
  }
  // write_file 成功 → 加入产物列表
  if (success) {
    if (meta && meta.name === "write_file") {
      const path = extractArtifactPath(meta.args);
      if (path) trackArtifact(path);
    }
  }
  // 兜底:Plan 模式下 AI 调了被白名单/sandbox 拦的工具 → 弹兜底卡片给两条出路。
  // 底座两种拒绝错误文本都要覆盖:
  //   - "not available in the current tool catalog"  通用 catalog 拒绝(write_file/edit_file)
  //   - "unavailable in Plan mode"                    Plan 模式专属拒绝(exec_shell/code_execution)
  if (
    !success &&
    modeState.mode === "plan" &&
    typeof output === "string" &&
    (output.includes("not available in the current tool catalog")
      || output.includes("unavailable in Plan mode")
      || output.includes("PermissionDenied"))
  ) {
    showPlanStuckCard(meta && meta.name);
  }
  toolMeta.delete(id);
});

// 底座 emit Event::UserInputRequired → bridge 转发为 chat:user_input_required
// payload: { id: tool_call_id, questions: [{header, id, question, options:[{label, description}]}] }
listen("chat:user_input_required", (e) => {
  const payload = e.payload || {};
  renderUserInputCard(payload.id, payload.questions || []);
});
listen("chat:usage", (e) => {
  const input = Number(e.payload?.input_tokens || 0);
  if (input > 0) {
    lastInputTokens = input;
    updateTokenBar(input);
  }
});
// File watcher 推送的产物事件：sessions/<id>/workspace/ 下任何新文件 / 修改
// → 自动跟踪到 artifacts。覆盖 write_file 无法捕获的场景 (如 exec_shell pandoc 出 docx)。
// 同一 file 短时间内可能多次 fire (Create + 多次 Modify),trackArtifact 已按 path 去重。
listen("artifact:disk", (e) => {
  const payload = e.payload || {};
  if (payload.session_id !== activeSessionId) return; // 只跟当前 session
  if (!payload.path) return;
  if (payload.event === "removed") {
    untrackArtifact(payload.path);
  } else {
    trackArtifact(payload.path);
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

  // Pinvou Review v2 阶段 A:plan review 跑完,把 3 按钮附加到 Pinvou 气泡。
  // 气泡本身已经在 chat:delta 期间渲染成紫色(beginAssistantBubble 看 pendingAssistantPersona)。
  if (pendingPinvouReview) {
    const lastAssistant = collectLastAssistantText();
    const report = extractPinvouReviewReport(lastAssistant);
    const pinvouRows = chatArea.querySelectorAll('.msg-row[data-pinvou-persona="pinvou-plan"]');
    const lastPinvouRow = pinvouRows.length ? pinvouRows[pinvouRows.length - 1] : null;
    if (lastPinvouRow) {
      attachPinvouReviewActions(lastPinvouRow, report, pendingPinvouReview.planCardEl);
    } else {
      // 没找到气泡(LLM 没输出任何内容)→ fallback 提示
      appendSystemMessage("⚠️ Pinvou 没回应,可能是模型卡住。手动点 ✅ 会再次触发 GATE。");
    }
    pendingPinvouReview = null;
  }

  // 阶段 D:final review 已经渲染成紫色 Pinvou 气泡(beginAssistantBubble 已处理),
  // advisory 性质,不附加按钮,只清 flag。
  if (pendingFinalReview) {
    pendingFinalReview = false;
  }

  // 把累积的 assistant message (text + 任何未配对的 tool_use) flush 到 messages.
  flushAssistantMessageToHistory();
  closeAssistantBubble();
  setBusy(false);

  // 执行 plan 完成 → 回 yolo 默认态(plan_phase 从 executing → none).
  // 同步后端 store, 防止下条消息 chat 命令读到 phase=executing 错位.
  let wasExecutingTransition = false;
  if (modeState.plan_phase === "executing") {
    wasExecutingTransition = true;
    modeState = { mode: "yolo", plan_phase: "none", pinvou_review_enabled: modeState.pinvou_review_enabled };
    updateModeUI();
    if (activeSessionId) {
      try { await invoke("discard_plan", { sessionId: activeSessionId }); } catch (_) {}
    }
  }

  // Pinvou Review v2 阶段 D:任务收口 final review
  // executing→none transition 是任务收口信号。pinvou_review_enabled 时自动 advisory review。
  // pendingFinalReview 防止 final review 自己触发的 chat:done 又递归触发新一轮 final。
  if (wasExecutingTransition && modeState.pinvou_review_enabled && !pendingFinalReview) {
    await autoTriggerPinvouFinal();
  }

  // 持久化整轮（含 user + assistant）到 disk
  await persistMessages();

  // 通知等待 done 的 waiters（用于 cancel-then-switch 流程）
  const resolvers = pendingDoneResolvers;
  pendingDoneResolvers = [];
  for (const r of resolvers) r();
});

// chat:plan_snapshot —— 每次 update_plan/checklist_write/todo_write 工具调用后触发,
// 实时更新 chip 进度区。跟 plan_ready 解耦(后者控制 plan_card 弹出)。
// 各 snapshot 带时间戳,pickProgressItems 选最新的渲染。
listen("chat:plan_snapshot", (e) => {
  const payload = e.payload || {};
  if (payload.session_id && payload.session_id !== activeSessionId) return;
  const now = Date.now();
  if (payload.plan_snapshot) {
    latestPlanSnapshot = payload.plan_snapshot;
    latestPlanSnapshotTs = now;
  }
  if (payload.todos_snapshot) {
    latestTodosSnapshot = payload.todos_snapshot;
    latestTodosSnapshotTs = now;
  }
  updateModeUI();
});

// chat:plan_ready 后端 payload schema:
//   { session_id, plan_snapshot?, todos_snapshot? }
// plan_snapshot 来自 update_plan (strategy 层): { explanation, items:[{step,status}] }
// todos_snapshot 来自 checklist_write/todo_write (leaf 层): { items:[{id,content,status}], completion_pct, in_progress_id }
// 任一非空就渲染 plan_card；都有就两层渲染。
listen("chat:plan_ready", (e) => {
  const payload = e.payload || {};
  if (payload.session_id && payload.session_id !== activeSessionId) return;
  modeState.plan_phase = "ready";
  updateModeUI();
  renderPlanReadyCard({
    plan: payload.plan_snapshot,
    todos: payload.todos_snapshot,
  });
});

// M3: Planning 态 + AI 没调 plan 工具 + assistant text > 300 字 → 后端 emit。
// 前端拿最后一条 assistant text 做关键词命中检测,命中才弹文本兜底卡片。
const PLAN_FALLBACK_KEYWORDS = ["方案", "步骤", "以下", "技术栈", "实现", "设计", "**"];
listen("chat:plan_text_fallback", (e) => {
  const payload = e.payload || {};
  if (payload.session_id && payload.session_id !== activeSessionId) return;
  let lastText = "";
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].role === "assistant") {
      const parts = messages[i].content || [];
      for (const p of parts) {
        if (p.type === "text" && p.text) lastText += p.text;
      }
      break;
    }
  }
  if (!lastText) return;
  const hit = PLAN_FALLBACK_KEYWORDS.some((kw) => lastText.includes(kw));
  if (!hit) return;
  renderPlanTextFallbackCard(lastText);
});

// M2: Executing 自驱 3 次后仍卡 → 弹卡顿提示
listen("chat:execution_stuck", (e) => {
  const payload = e.payload || {};
  if (payload.session_id && payload.session_id !== activeSessionId) return;
  renderExecutionStuckCard(payload.auto_continue_tried || 0);
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
  // 修法 D: Ready 态用户直接发消息 = 隐式修订. phase 保持 Ready,
  // 由后端 Ready reminder ("用户发新消息=隐式修订,必须重出 update_plan") 引导 AI.
  // 不 freeze 旧 plan_card —— 让用户保留撤回(回头点 ✅)的可能.
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
    // busy 时点击 ⏹️ = 通用"停止生成"(业界惯例: Cursor/Claude Code).
    // 仅停 turn, 不改 mode_state —— 用户停下后仍在原 mode 可继续讨论.
    // 想退 Plan 模式走 chip [⚡ 直接动手] 或灯泡 toggle.
    await cancelActiveTurn();
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

// 禁掉 webview 默认右键菜单 (含「重新加载」)。
// 用户右键以为是普通菜单，点了 reload 整个前端状态会丢，体验差。
document.addEventListener("contextmenu", (e) => e.preventDefault());

// ── 自定义 confirm modal (替代 GTK 原生 dialog.ask) ──────────────
const modalOverlay = document.getElementById("modal-overlay");
const modalTitleEl = document.getElementById("modal-title");
const modalBodyEl = document.getElementById("modal-body");
const modalConfirmBtn = document.getElementById("modal-btn-confirm");
const modalCancelBtn = document.getElementById("modal-btn-cancel");

/** 主题一致的 confirm 弹窗。
 *  返回 Promise<boolean>：true=确认 / false=取消 / Esc 也是取消。
 *  kind: "default" | "warning"  (warning 的确认按钮变红色) */
function appConfirm(message, opts = {}) {
  return new Promise((resolve) => {
    if (!modalOverlay) {
      // fallback：极端情况 modal DOM 不在
      resolve(confirm(message));
      return;
    }
    modalTitleEl.textContent = opts.title || i18nText("modal.confirm");
    modalBodyEl.textContent = message;
    modalConfirmBtn.textContent = opts.confirmText || i18nText("modal.confirm");
    modalCancelBtn.textContent = opts.cancelText || i18nText("modal.cancel");
    modalOverlay.dataset.kind = opts.kind || "default";
    modalOverlay.dataset.open = "true";

    function cleanup() {
      modalOverlay.dataset.open = "false";
      modalConfirmBtn.removeEventListener("click", onConfirm);
      modalCancelBtn.removeEventListener("click", onCancel);
      modalOverlay.removeEventListener("click", onBackdrop);
      document.removeEventListener("keydown", onKey);
    }
    function onConfirm() { cleanup(); resolve(true); }
    function onCancel() { cleanup(); resolve(false); }
    function onBackdrop(e) { if (e.target === modalOverlay) onCancel(); }
    function onKey(e) {
      if (e.key === "Escape") { e.preventDefault(); onCancel(); }
      else if (e.key === "Enter") { e.preventDefault(); onConfirm(); }
    }
    modalConfirmBtn.addEventListener("click", onConfirm);
    modalCancelBtn.addEventListener("click", onCancel);
    modalOverlay.addEventListener("click", onBackdrop);
    document.addEventListener("keydown", onKey);
    setTimeout(() => modalConfirmBtn.focus(), 0);
  });
}

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

  // Backend live dot 仍周期拉（10s 一次，lightweight 只 probe vLLM）。
  // Monitor 拉取改成按需：switchView('monitor') 时启 1s interval，离开清。
  pollBackendStatus();
  setInterval(pollBackendStatus, 10000);
})();
