(function () {
  "use strict";

  var registry = window.__PINVOU_TAURI_BRIDGE_FEATURES__ = window.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["workflow-runtime"] = function (context) {
    var state = context.state;
    var invoke = context.invoke;
    var listen = context.listen;
    var notify = context.notify;
    var refreshHistoryList = context.refreshHistoryList;

  // ── 卡片流工作流：运行态 helper ──────────────────────────────────
  // 事件只认本次 run 的 session（payload 无 session_id 时放行，兼容）。
  function isRunSession(p) {
    var s = state.workflow.run.sessionId;
    return !p || !p.session_id || !s || p.session_id === s;
  }
  function applyAgentPatch(roleId, patch) {
    if (!roleId) return;
    var agents = state.workflow.run.agents;
    agents[roleId] = Object.assign(agents[roleId] || { id: roleId }, patch);
  }
  function markWorkflowRunStopped() {
    state.workflow.run.status = "stopped";
    Object.keys(state.workflow.run.agents || {}).forEach(function (id) {
      var agent = state.workflow.run.agents[id];
      if (agent && (agent.status === "running" || agent.status === "reviewing" || agent.status === "briefing")) {
        agent.status = "stopped";
      }
    });
    (state.workflow.run.cards || []).forEach(function (card) {
      if (!card.resolved) { card.resolved = true; card.cardState = "cancelled"; }
    });
  }
  function markWorkflowRunBlocked(payload) {
    var p = payload || {};
    var run = state.workflow.run;
    run.status = "blocked";
    var text = "⚙️ 工作流卡住：" + (p.message || p.blocked_reason || p.reason || "未知原因");
    var existing = (run.cards || []).find(function (card) { return card.workflowBlocked; });
    if (existing) {
      existing.text = text;
      existing.resolved = false;
    } else {
      pushRunCard({ kind: "system", text: text, resolved: false, workflowBlocked: true });
    }
  }
  function mergeFullState(p) {
    var run = state.workflow.run;
    if (p.project_dir) run.projectDir = p.project_dir;
    if (p.scenario) run.scenario = p.scenario;
    // [工作流分离] 后端按 scenario 解析出 workflow_id + workflow.json 的 ui 块,
    // 前端泳道/表单/标题/奏折全按 run.ui 渲染(不再硬编码各工作流)。
    if (p.workflow_id) run.workflowId = p.workflow_id;
    if (p.ui) run.ui = p.ui;
    var roles = p.roles || {};
    // [B2 修] full_state 是权威全量快照:快照里没有的角色条目要删——尚书省派单后
    // 静态六部被差事节点取代,留着陈旧条目会让泳道误判"六部在场"而不插差事批次泳道
    // (实测:六部卡全员显示"等待尚书省交付",而差事其实已经在跑)。
    Object.keys(state.workflow.run.agents).forEach(function (rid) {
      if (!(rid in roles)) delete state.workflow.run.agents[rid];
    });
    Object.keys(roles).forEach(function (rid) {
      var r = roles[rid] || {};
      applyAgentPatch(rid, {
        id: rid, name: r.name || rid, status: r.status || "pending",
        error: typeof r.error === "string" && r.error.trim() ? r.error : null,
        retries: r.retries == null ? 0 : r.retries,
        last_gate_verdict: r.last_gate_verdict || null,
        outputs_present: r.outputs_present || 0,
        last_run_ts: r.last_run_ts || null,
        depends_on: r.depends_on || [],
        wave: r.wave, bu: r.bu,   // [B2 E1] 差事分层 + 取头像/配色
      });
    });
    // stop marker 是最终状态。迟到或手动刷新的 full_state 仍可能携带盘上旧的
    // running/reviewing 状态，不能让已停止的角色卡回跳成“执行中”。
    if (p.stopped) markWorkflowRunStopped();
    else if (p.blocked || p.status === "blocked") markWorkflowRunBlocked(p);
    else if (p.all_completed) run.status = "complete";
  }
  // [2026-06-06] 快照恢复：把前端 run 态挂回一个已存在的工作流 run（app 重启/切会话后）。
  // 拉后端快照(get_workflow_state) → 点亮 run 态(复刻 project_started 结构) → mergeFullState
  // 填角色 → 看板和「🔄 重跑」按钮全部恢复。非工作流会话(无 roles)返回 false、无副作用。
  async function attachRun(sessionId, staleRunning) {
    try {
      var snap = await invoke("get_workflow_state", { sessionId: sessionId });
      if (!snap || !snap.roles || Object.keys(snap.roles).length === 0) return false;
      state.workflow.run = {
        active: true, sessionId: sessionId, projectDir: snap.project_dir || null,
        scenario: snap.scenario || null,
        status: snap.stopped ? "stopped" : (snap.blocked ? "blocked" : (snap.all_completed ? "complete" : "running")),
        agents: {}, cards: [], selectedRole: null,
      };
      mergeFullState(snap);
      // [恢复] app 重启后,盘上记 running/reviewing 的角色其 SubAgent 已随重启死亡 →
      // 标记 stale(需要重跑),卡片才显示「🔄 重跑」按钮;否则卡在"在工作"无出口。
      // staleRunning 只在启动恢复(resumeWorkflowOnBoot)传 true;app 内切活跃 run 不传。
      if (staleRunning) {
        var ag = state.workflow.run.agents;
        Object.keys(ag).forEach(function (rid) {
          var s = ag[rid] && ag[rid].status;
          if (s === "running" || s === "reviewing" || s === "briefing") ag[rid].status = "stale";
        });
      }
      notify();
      return true;
    } catch (e) { console.warn("attachRun failed", e); return false; }
  }
  // app 启动后自动恢复最近一个进行中的工作流 run（后端扫 binding 找）。
  async function resumeWorkflowOnBoot() {
    try {
      var r = await invoke("find_resumable_run");
      if (r && r.session_id) {
        // [方案A] 不再 switchToSession 劫持聊天会话——启动恒落干净草稿页。
        // 只把工作流看板挂回(attachRun 填 state.workflow.run,不动 activeSessionId
        // 也不切 currentView),用户主动切「工作流」tab 才看到那个 run。
        await attachRun(r.session_id, true); // 僵死 running → stale,露出重跑按钮
      }
    } catch (e) { console.warn("resumeWorkflowOnBoot failed", e); }
  }
  function pushRunCard(card) { card.cardId = ++context.itemIdSeq; state.workflow.run.cards.push(card); }
  function resolveRunCard(cardId, cardState) {
    state.workflow.run.cards.forEach(function (c) { if (c.cardId === cardId) { c.resolved = true; c.cardState = cardState; } });
    notify();
  }
  // 按角色 resolve 所有未处理的 gate 卡(去重 + 兜底:即便 cardId 对不上也清干净)。
  function resolveRunCardsForRole(roleId, cardState) {
    if (!roleId) return;
    state.workflow.run.cards.forEach(function (c) {
      if (c.kind === "gate" && c.roleId === roleId && !c.resolved) { c.resolved = true; c.cardState = cardState; }
    });
  }
  // 批准/打回后拉一次后端真实状态,把看板角色态(gate_waiting→completed/running)刷正确。
  async function refreshRunState() {
    try {
      var sid = state.workflow.run.sessionId; if (!sid) return;
      var snap = await invoke("get_workflow_state", { sessionId: sid });
      if (snap && snap.roles) mergeFullState(snap);
    } catch (e) { console.warn("refreshRunState failed", e); }
  }

  // ── 卡片流工作流：事件监听 ───────────────────────────────────────
  listen("workflow:full_state", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    mergeFullState(p); notify();
  });
  listen("workflow:agent_state_changed", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    if (state.workflow.run.status === "stopped") return;
    applyAgentPatch(p.role_id, { name: p.role_name || p.role_id, status: p.status || "running" });
    notify();
  });
  // [per_page] fan-out 逐页状态 → 工作流界面把该节点展开成 N 个 SubAgent chip。
  // payload: { base_role, pages:[{page,status}] }，status ∈ queued|running|done|retrying。
  listen("workflow:fanout", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    if (state.workflow.run.status === "stopped") return;
    if (!state.workflow.run.fanout) state.workflow.run.fanout = {};
    var pages = p.pages || [];
    state.workflow.run.fanout[p.base_role] = { total: pages.length, pages: pages };
    notify();
  });
  listen("workflow:complete", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    if (state.workflow.run.status === "stopped") return;
    state.workflow.run.status = "complete";
    // [edict-obs] 后端带回成品路径 → 弹成品卡(一键打开 deck)
    if (p.artifact) {
      pushRunCard({ kind: "artifact", path: p.artifact, text: "🎉 工作流完成，成品已生成", resolved: false });
    }
    notify();
  });
  listen("workflow:blocked", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    if (state.workflow.run.status === "stopped") return;
    markWorkflowRunBlocked(p);
    notify();
  });
  listen("workflow:stopped", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    markWorkflowRunStopped();
    notify();
  });
  listen("workflow:gate_approval", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    if (state.workflow.run.status === "stopped") return;
    var findings = p.findings || (p.gate_description ? [p.gate_description] : []);
    // 去重:同一角色已有未处理的 gate 卡 → 只更新 findings,不叠新卡(huizou 反复过闸会重复 emit)。
    var dup = (state.workflow.run.cards || []).find(function (c) { return c.kind === "gate" && c.roleId === p.role_id && !c.resolved; });
    if (dup) { dup.findings = findings; notify(); return; }
    // 后端 emit 的是 gate_description(单串)，不是 findings —— 兜底收进 findings。
    pushRunCard({ kind: "gate", roleId: p.role_id, roleName: p.role_name || p.role_id, findings: findings, resolved: false });
    notify();
  });
  // [edict-obs] SubAgent 实时进展(底座每步/每个工具调用自动发)。
  listen("workflow:agent_progress", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    if (state.workflow.run.status === "stopped") return;
    if (!state.workflow.run.progress) state.workflow.run.progress = {};
    var key = p.role_id || p.agent_id;
    if (!key) return;
    // per_page 成员 "<role>#p01" 归并到基础节点显示
    var base = key.indexOf("#") > -1 ? key.split("#")[0] : key;
    state.workflow.run.progress[base] = p.status || "";
    notify();
  });
  // [edict-obs] per-role token 账本快照(每次 LLM 调用后推一次累计值)。
  listen("workflow:token_usage", function (e) {
    var p = e.payload || {}; if (!isRunSession(p)) return;
    if (state.workflow.run.status === "stopped") return;
    if (!state.workflow.run.tokens) state.workflow.run.tokens = {};
    var key = p.role_id || p.agent_id;
    if (!key) return;
    var base = key.indexOf("#") > -1 ? key.split("#")[0] : key;
    state.workflow.run.tokens[base] = {
      input: p.input_tokens_total || 0, output: p.output_tokens_total || 0, calls: p.calls || 0,
    };
    notify();
  });
  // request_user_input：本次 run 的 session 弹问答卡到看板底部交互区
  // （与上面 chat:user_input_required 的 chat 渲染并存，互不影响——工作流页看 run.cards）。
  listen("chat:user_input_required", function (e) {
    var p = e.payload || {};
    if (!state.workflow.run.sessionId || p.session_id !== state.workflow.run.sessionId) return;
    if (state.workflow.run.status === "stopped") return;
    var qs = p.questions || []; if (!Array.isArray(qs) || !qs.length) return;
    pushRunCard({ kind: "user_input", toolCallId: p.id, questions: qs, resolved: false });
    notify();
  });

    return {
      isRunSession: isRunSession,
      applyAgentPatch: applyAgentPatch,
      markWorkflowRunStopped: markWorkflowRunStopped,
      markWorkflowRunBlocked: markWorkflowRunBlocked,
      mergeFullState: mergeFullState,
      attachRun: attachRun,
      resumeWorkflowOnBoot: resumeWorkflowOnBoot,
      pushRunCard: pushRunCard,
      resolveRunCard: resolveRunCard,
      resolveRunCardsForRole: resolveRunCardsForRole,
      refreshRunState: refreshRunState,
    };
  };
})();
