/**
 * workflow feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  "use strict";
  var registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["workflow"] = function (context) {
    var state = context.state;
    var notify = context.notify;
    var invoke = context.invoke;
    var bt = context.bt;
    var addSystemItem = context.addSystemItem;
    var dialogOpen = context.dialogOpen;
    var resetPendingAssistant = context.resetPendingAssistant;
    var syncModeState = context.syncModeState;
    var refreshHistoryList = context.refreshHistoryList;
    var markWorkflowRunStopped = context.markWorkflowRunStopped;
    var refreshRunState = context.refreshRunState;
    var resolveRunCard = context.resolveRunCard;
    var resolveRunCardsForRole = context.resolveRunCardsForRole;
  // ── skill 工作流：动作（invoke 包装）[2026-06-06 恢复] ────────────
  // 合并 973a6f0 把这 6 个函数定义弄丢了(导出/UI调用/后端命令都在,独缺定义)→
  // 导出对象构建撞 ReferenceError(loadSkills undefined)→ window.TauriBridge 整个没装上。
  // 从 943af78 原样恢复。
  function setCurrentPhase(phaseId, source) {
    if (!phaseId) return;
    var wf = state.workflow;
    // 大小写归一匹配 phases
    var match = wf.phases.find(function (p) { return String(p.id).toLowerCase() === String(phaseId).toLowerCase(); });
    var canonical = match ? match.id : phaseId;
    wf.currentPhaseId = canonical;
    if (wf.reachedPhaseIds.indexOf(canonical) < 0) wf.reachedPhaseIds.push(canonical);
    notify();
  }
  async function loadSkills() {
    state.workflow.loadState = "loading"; notify();
    try {
      state.workflow.skills = await invoke("list_skills_v2");
      state.workflow.loadState = "ready";
    } catch (e) { state.workflow.skills = []; state.workflow.loadState = "error"; }
    notify();
  }
  async function activateSkill(name) {
    // 入口捕获触发会话：await 期间用户可能已切走，响应后不得无条件劫持
    // activeSessionId（审计；与 web 版对齐）。已切走则只登记 bindings。
    var sid = state.activeSessionId;
    try {
      var res = await invoke("start_skill_session", { name: name });
      var skill = res.skill || {};
      var meta = res.session || res.metadata || {};
      // 仅当会话未被切走时才劫持 activeSessionId：`!sid` 分支会在入口无会话、
      // await 期间用户新建聊天会话时仍无条件劫持（审计补丁）——统一收敛为等值
      // 比较（入口 sid 为 null 且响应时仍为 null 时劫持，原语义不变）。
      if (state.activeSessionId === sid) {
        state.activeSessionId = meta.id || state.activeSessionId;
        state.messages = []; state.chatItems = []; resetPendingAssistant();
        state.workflow.activeSkillName = skill.name || name;
        state.workflow.phases = skill.phases || [];
        state.workflow.currentPhaseId = skill.current_phase_id || (skill.phases && skill.phases[0] && skill.phases[0].id) || null;
        state.workflow.reachedPhaseIds = state.workflow.currentPhaseId ? [state.workflow.currentPhaseId] : [];
      }
      if (meta.id) state.workflow.bindings[meta.id] = skill.name || name;
      await refreshHistoryList();
      await syncModeState();
      notify();
      return res;
    } catch (e) { addSystemItem(bt("workflowActivateFailed") + e); notify(); return null; }
  }
  async function deactivateSkill() {
    // 入口捕获触发会话：await 期间用户可能已切走，解绑与全局清空不得
    // 作用于别的会话（否则 B 的激活技能显示被清掉，审计 R2）。
    var sid = state.activeSessionId;
    if (sid) {
      // invoke 形状保持原样（协议指纹按文本计算）；发起瞬间 activeSessionId === sid。
      try { await invoke("unbind_session_skill", { sessionId: state.activeSessionId }); } catch (_) {}
      // 后端解绑已生效：bindings 键是入口 sid、与 await 后谁 active 无关，必须
      // 无条件同步删除（bindings 无重算路径，漏删会留下永久幽灵徽标，复审补丁）。
      delete state.workflow.bindings[sid];
    }
    if (sid && state.activeSessionId !== sid) return;
    state.workflow.activeSkillName = null;
    state.workflow.phases = [];
    state.workflow.currentPhaseId = null;
    state.workflow.reachedPhaseIds = [];
    notify();
  }
  async function openDemo(name) {
    state.workflow.demo = { open: true, name: name, loading: true, kind: null, content: null, error: null, description: null, duration: null };
    notify();
    try {
      var d = await invoke("read_skill_demo", { name: name });
      // await 期间用户可能已关闭弹窗或改开别的 demo：陈旧响应不得把已关闭
      // 的弹窗重新弹开、也不得覆盖别的 demo 内容（审计 R3）。
      if (!state.workflow.demo || state.workflow.demo.name !== name) return;
      state.workflow.demo = {
        open: true, name: name, loading: false,
        kind: d.file_kind, path: d.file_path, content: d.content,
        error: null, description: d.description, duration: d.duration,
      };
    } catch (e) {
      if (!state.workflow.demo || state.workflow.demo.name !== name) return;
      state.workflow.demo = { open: true, name: name, loading: false, kind: null, content: null, error: String(e) };
    }
    notify();
  }
  function closeDemo() { state.workflow.demo = null; notify(); }

  // ── 卡片流工作流：动作（invoke 包装）────────────────────────────
  // 新建任务：建项目（project_started 事件设 run 态）→ kick 派发首个 agent（无聊天）。
  // busy 闸防双击重复建 run（与 web 版对齐，复审补丁；UI 模态的 starting 闸为主防）。
  async function startWorkflowTask(scenario, brief) {
    if (state.workflow.starting) return null;
    state.workflow.starting = true;
    try {
      var res;
      try {
        res = await invoke("start_workflow", { scenario: scenario, briefInit: brief || null });
      } catch (e) {
        addSystemItem(bt("workflowCreateFailed") + e);
        throw e;
      }
      try {
        await invoke("kick_workflow", { sessionId: res.session_id });
      } catch (e) {
        addSystemItem(bt("workflowStartFailed") + e);
        throw e;
      }
      return res;
    } finally { state.workflow.starting = false; }
  }
  // 停止整个 run：后端先落 stop marker 再取消所有后台 SubAgent；返回旧 brief，
  // 供工作流页打开“修改需求并重新开始”的预填表单。
  async function stopWorkflowTask(reason) {
    var sid = state.workflow.run.sessionId;
    if (!sid) throw new Error(bt("workflowNoStoppableRun"));
    var result = await invoke("stop_workflow", {
      sessionId: sid,
      reason: reason || "user_stopped",
    });
    // stop_workflow 等待期间用户可能已启动新 run（project_started 整体替换
    // run 对象）：此时不得把新 run 标记为 stopped（审计 R4），否则新 run 的
    // 事件会被 stopped 闸全部吞掉、看板冻结而实际仍在跑。
    if (state.workflow.run.sessionId === sid) {
      markWorkflowRunStopped();
    }
    notify();
    return result || {};
  }
  // 模板页数据源:已发现且 enabled 的工作流(含 ui 块)。失败回空数组(模板页显示空态)。
  async function listWorkflows() {
    try { return (await invoke("list_workflows")) || []; }
    catch (e) { console.warn("list_workflows failed", e); return []; }
  }
  function selectWorkflowRole(roleId) { state.workflow.run.selectedRole = roleId; notify(); }
  function closeWorkflowDrawer() { state.workflow.run.selectedRole = null; notify(); }
  function resetWorkflowRun() {
    state.workflow.run = { active: false, sessionId: null, projectDir: null, scenario: null, status: "idle", agents: {}, cards: [], selectedRole: null };
    notify();
  }
  // 抽屉数据：按 role 拉产出 / gate / 日志（projectDir 来自 run）。
  function getRolePrompt(roleId, projectDir) { return invoke("get_role_prompt", { roleId: roleId, projectDir: projectDir || null }); }
  function getRoleOutputs(roleId) { return invoke("get_role_outputs", { roleId: roleId, projectDir: state.workflow.run.projectDir }); }
  function getGateReport(roleId) { return invoke("get_gate_report", { roleId: roleId, projectDir: state.workflow.run.projectDir }); }
  function getRoleLogs(roleId, tail) { return invoke("get_role_logs", { roleId: roleId, projectDir: state.workflow.run.projectDir, tail: tail || 50 }); }
  // 交互卡动作
  async function submitWorkflowUserInput(cardId, toolCallId, answers) {
    try { await invoke("submit_user_input", { toolCallId: toolCallId, answers: answers, sessionId: state.workflow.run.sessionId }); resolveRunCard(cardId, "submitted"); }
    catch (e) { addSystemItem(bt("workflowSubmitFailed") + e); }
  }
  // [2026-06-06] 素材上传：复用系统文件选择器(dialogOpen) → 拷进当前 run 的 配套材料/。
  // 返回落盘文件名数组(含同名去重);失败 throw 给调用方(卡片上报错)。
  async function pickAndAddMaterials() {
    if (!dialogOpen) { addSystemItem(bt("filePickUnavailable")); return []; }
    var selected = await dialogOpen({ multiple: true });
    if (!selected) return [];
    var paths = Array.isArray(selected) ? selected : [selected];
    var added = await invoke("add_run_materials", { sessionId: state.workflow.run.sessionId, paths: paths });
    addSystemItem(bt("workflowMaterialsAdded")(added.length, added));
    return added;
  }
  // [新建任务模态] 只弹系统选择器拿路径,不拷贝(run 还没建)。返回路径数组。
  async function pickFiles() {
    if (!dialogOpen) { addSystemItem(bt("filePickUnavailable")); return []; }
    var selected = await dialogOpen({ multiple: true });
    if (!selected) return [];
    return Array.isArray(selected) ? selected : [selected];
  }
  async function pickFolder() {
    if (!dialogOpen) throw new Error(bt("workflowFolderPickerUnavailable"));
    var selected = await dialogOpen({
      directory: true,
      multiple: false,
      title: bt("workflowPickWorkDirTitle"),
    });
    if (!selected) return null;
    return Array.isArray(selected) ? (selected[0] || null) : selected;
  }
  // 知识库「添加文件夹」：递归导入需要返回目录路径数组（可多选）。
  // 后端 kb_collection_add_sources → expand_import_roots 会用 WalkDir 递归展开目录。
  async function pickFolders() {
    if (!dialogOpen) { addSystemItem(bt("filePickUnavailable")); return []; }
    var selected = await dialogOpen({ directory: true, multiple: true, title: bt("kbPickFolderTitle") });
    if (!selected) return [];
    return Array.isArray(selected) ? selected : [selected];
  }
  async function pickFeedbackFiles() {
    if (!dialogOpen) return [];
    var selected = await dialogOpen({
      multiple: true,
      filters: [
        { name: bt("workflowMediaFilterName"), extensions: ["png", "jpg", "jpeg", "gif", "webp", "mp4", "mov", "webm"] },
      ],
    });
    if (!selected) return [];
    return Array.isArray(selected) ? selected : [selected];
  }
  // [新建任务模态] start_workflow 建好 run 后,把已选路径拷进该 session 的配套材料/。
  async function addMaterialsToSession(sessionId, paths) {
    if (!paths || !paths.length) return [];
    return invoke("add_run_materials", { sessionId: sessionId, paths: paths });
  }
  // cardId 可为 null:看板 agent 卡上的"确认通过"只有 roleId(刷新后内存 gate 卡已清空)。
  // 批准只需 roleId + sessionId,绝不依赖前端那张内存卡是否存在(那正是"按钮点了没反应"的 bug)。
  async function approveWorkflowGate(cardId, roleId) {
    // 入口捕获触发 run：await 期间可能已启动新 run（project_started 整体替换
    // run 对象），批准结果不得落到新 run 的卡片上（审计）。
    var runSid = state.workflow.run.sessionId;
    try {
      await invoke("approve_workflow_gate", { roleId: roleId, sessionId: state.workflow.run.sessionId });
      if (state.workflow.run.sessionId !== runSid) return;
      if (cardId) resolveRunCard(cardId, "approved");
      resolveRunCardsForRole(roleId, "approved");
      await refreshRunState();   // 刷新真实状态:huizou gate_waiting→completed,看板按钮随之消失
      notify();
    } catch (e) {
      // 陈旧失败提示不得弹给新 run 用户（复审补丁：catch 与成功路径同款 run 身份校验）。
      if (state.workflow.run.sessionId === runSid) addSystemItem(bt("workflowApproveFailed") + e);
    }
  }
  async function rejectWorkflowGate(cardId, roleId, reason) {
    var runSid = state.workflow.run.sessionId;
    try {
      await invoke("reject_workflow_gate", { roleId: roleId, reason: reason || bt("workflowRejectDefaultReason"), sessionId: state.workflow.run.sessionId });
      if (state.workflow.run.sessionId !== runSid) return;
      if (cardId) resolveRunCard(cardId, "rejected");
      resolveRunCardsForRole(roleId, "rejected");
      await refreshRunState();
      notify();
    } catch (e) {
      if (state.workflow.run.sessionId === runSid) addSystemItem(bt("workflowRejectFailed") + e);
    }
  }
  // 从失败节点续跑:重置该角色为 pending(清重试)后重新调度,上游已完成节点不重跑。
  async function retryWorkflowRole(roleId) {
    try {
      const r = await invoke("retry_workflow_role", { roleId: roleId, sessionId: state.workflow.run.sessionId });
      addSystemItem(bt("workflowRerunPrefix") + roleId + ": " + r);
    } catch (e) { addSystemItem(bt("workflowRerunFailed") + e); }
  }

    return {
      setCurrentPhase: setCurrentPhase,
      loadSkills: loadSkills,
      activateSkill: activateSkill,
      deactivateSkill: deactivateSkill,
      openDemo: openDemo,
      closeDemo: closeDemo,
      startWorkflowTask: startWorkflowTask,
      stopWorkflowTask: stopWorkflowTask,
      listWorkflows: listWorkflows,
      selectWorkflowRole: selectWorkflowRole,
      closeWorkflowDrawer: closeWorkflowDrawer,
      resetWorkflowRun: resetWorkflowRun,
      getRolePrompt: getRolePrompt,
      getRoleOutputs: getRoleOutputs,
      getGateReport: getGateReport,
      getRoleLogs: getRoleLogs,
      submitWorkflowUserInput: submitWorkflowUserInput,
      pickAndAddMaterials: pickAndAddMaterials,
      pickFiles: pickFiles,
      pickFolder: pickFolder,
      pickFolders: pickFolders,
      pickFeedbackFiles: pickFeedbackFiles,
      addMaterialsToSession: addMaterialsToSession,
      approveWorkflowGate: approveWorkflowGate,
      rejectWorkflowGate: rejectWorkflowGate,
      retryWorkflowRole: retryWorkflowRole
    };
  };
})(window);
