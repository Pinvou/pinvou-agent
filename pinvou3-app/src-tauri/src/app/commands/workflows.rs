// ===================== 工作流 Phase 可视化 MVP1 =====================
// list_skills_v2 / read_skill_demo / start_skill_session / unbind_session_skill
// 四件套支撑「工作流」视图 + skill per-session 绑定。设计与边界见
// `/home/hexin/.claude/plans/workflow-phase-elegant-zephyr.md`。

/// 工作流卡片不显示的 skill 名单。这些是 pinvou3 自带的基础能力组件
/// (review 流程内部用),不应作为用户主动启用的工作流入口。
///
/// 后续真正物理隔离会把这俩从 `bundle/skills/` 移到独立目录 +
/// CodeWhale fork patch 让 EngineConfig 支持多 skills_dir。当前
/// 用 skiplist 软隔离,工作量小,效果一致。
const WORKFLOW_HIDDEN_SKILLS: &[&str] = &["pinvou-review-plan", "pinvou-review-final"];

/// 工作流视图卡片渲染需要的 skill 摘要 — 跟 CodeWhale runtime_api 的
/// `SkillEntry` 不同,这里额外把 phases / demo 元数据序列化给前端 (底座
/// 没把这俩字段暴露到 REST,所以 pinvou3-app 自己读 SkillRegistry 拼)。
#[derive(Debug, Serialize)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    /// 永远是 "bundle"(只扫 bundle/skills 单源)— 字段保留是为了前端
    /// 卡片角标 / 跟未来多源场景兼容。
    pub source: &'static str,
    /// (底座 v0.8.57 删除 phases/demo 元数据;字段保留作前端兼容,恒为空/默认)
    pub phases: Vec<serde_json::Value>,
    pub demo: DemoSummary,
}

#[derive(Debug, Serialize, Default)]
pub struct DemoSummary {
    pub has_file: bool,
    pub has_preview: bool,
    pub description: Option<String>,
    pub duration: Option<String>,
}

/// 列出 `~/.pinvou3/bundle/skills/` 下的所有用户业务 skill。
/// pinvou-review-* 这种系统基础能力通过 `WORKFLOW_HIDDEN_SKILLS` 过滤掉
/// (它们不应出现在工作流卡片入口里)。
#[tauri::command]
pub async fn list_skills_v2() -> Result<Vec<SkillSummary>, String> {
    use crate::platform::paths;
    use deepseek_tui::skills::SkillRegistry;

    let dir = paths::bundle_workflow_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let registry = SkillRegistry::discover(&dir);
    let mut out: Vec<SkillSummary> = registry
        .list()
        .iter()
        .filter(|s| !WORKFLOW_HIDDEN_SKILLS.contains(&s.name.as_str()))
        .map(|s| SkillSummary {
            name: s.name.clone(),
            description: s.description.clone(),
            source: "bundle",
            phases: Vec::new(),
            demo: DemoSummary::default(),
        })
        .collect();
    // 有 phases 的排前面
    out.sort_by(|a, b| {
        b.phases
            .len()
            .cmp(&a.phases.len())
            .then(a.name.cmp(&b.name))
    });
    Ok(out)
}
/// 读取一个 skill 的 demo 文件元数据 + 内容(text 类型直接附 content,
/// html/image 走 file_path + Tauri `convertFileSrc` 由前端 iframe/img 渲染)。
#[derive(Debug, Serialize)]
pub struct SkillDemoPayload {
    pub file_path: Option<String>,
    pub file_kind: &'static str, // "html" | "image" | "text" | "unknown" | "none"
    /// text 类型时附内容 (限 1MB);否则 None。
    pub content: Option<String>,
    pub preview_path: Option<String>,
    pub description: Option<String>,
    pub duration: Option<String>,
}

#[tauri::command]
pub async fn read_skill_demo(name: String) -> Result<SkillDemoPayload, String> {
    // 底座 v0.8.57 删除 SKILL.md 的 demo 元数据;命令保留(前端按 file_kind="none" 渲染空态)。
    let _ = name;
    Ok(SkillDemoPayload {
        file_path: None,
        file_kind: "none",
        content: None,
        preview_path: None,
        description: None,
        duration: None,
    })
}

/// 工作流卡片"启用"后返回的载荷:新建的 session 元数据 + 该 session 绑定的
/// skill 信息(phases 给前端初始化 chips strip)。
#[derive(Debug, Serialize)]
pub struct StartSkillSessionResult {
    pub session: SessionMetadata,
    pub skill: ActiveSkillState,
}

/// chips strip 初始化用的 skill 视图字段。
#[derive(Debug, Serialize)]
pub struct ActiveSkillState {
    pub name: String,
    /// (底座 v0.8.57 删除 PhaseDef;恒为空,chips 不再渲染)
    pub phases: Vec<serde_json::Value>,
    pub current_phase_id: Option<String>,
}

/// 用户在「工作流」视图点 skill 卡片「启用」 → 新建一个 session 并把该 skill
/// 绑定到这个 session。每次点都新建独立 session,skill 仅对该 session 生效
/// (不再有全局 active_skill 单例)。
#[tauri::command]
pub async fn start_skill_session(
    name: String,
    set_active: Option<bool>,
    app: AppHandle,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<StartSkillSessionResult, String> {
    use crate::core::mode_state::ActiveSkillBinding;
    use crate::platform::paths;
    use deepseek_tui::skills::SkillRegistry;

    if WORKFLOW_HIDDEN_SKILLS.contains(&name.as_str()) {
        return Err(format!("{name} 是系统基础能力,不能直接启用为工作流"));
    }

    // 1) 只在 bundle/skills 里找 — 跟 list_skills_v2 source of truth 保持一致
    let dir = paths::bundle_workflow_dir();
    if !dir.exists() {
        return Err(format!("skills dir not found: {}", dir.display()));
    }
    let registry = SkillRegistry::discover(&dir);
    let skill = registry
        .get(&name)
        .ok_or_else(|| format!("skill not found: {name}"))?
        .clone();

    // 2) 查找已有绑定该 skill 的 session——恢复工作流而非新建
    let existing_sid = store.find_session_with_skill(&name);
    // (底座 v0.8.57 删除 Skill.phases;chips 机制随之退役,恒为空)
    let first_phase: Option<String> = None;
    let phases: Vec<serde_json::Value> = Vec::new();

    if let Some(sid) = existing_sid {
        // 恢复：切到已有 session，重新加载对话历史。
        // 多 session 并发:不显式 sync engine,EnginePool 下次 chat 时
        // get_or_spawn 为该 session rehydrate 专属 engine。
        if set_active.unwrap_or(true) {
            store.set_active(Some(sid.clone()));
        }
        let session_data = store
            .load(&sid)
            .map_err(|e| format!("load existing session: {e:?}"))?;

        return Ok(StartSkillSessionResult {
            session: session_data.metadata,
            skill: ActiveSkillState {
                name: skill.name,
                phases,
                current_phase_id: first_phase,
            },
        });
    }

    // 3) 没有已有 session → 新建(沿用 create_session 的 model + workspace 取值)
    let (model, model_id) = pool.default_model_for_new_session();
    let workspace = pool.bridge.workspace.clone();
    let session = store
        .create_new(model, model_id, workspace)
        .map_err(|e| format!("create_session: {e:?}"))?;
    let sid = session.metadata.id.clone();
    if set_active.unwrap_or(true) {
        store.set_active(Some(sid.clone()));
    }

    // 多 session 并发:不预热 engine(lazy)。首条 chat 时 EnginePool 为这个空 session
    //    spawn 专属 engine,空历史无需 SyncSession。

    // [phase marker 下线] 原 pending_instruction 注入"按 phases 流程响应 + engine 自动抽
    // <phase> marker + Phase tracking 段"的引导,这些底座机制(Skill.phases / marker 抽取)
    // 已随 v0.8.57 退役。绑定只留 name,skill 能力走底座 progressive disclosure。
    store.bind_skill(
        &sid,
        ActiveSkillBinding {
            name: skill.name.clone(),
            pending_instruction: None,
            phases: phases.clone(),
            project_dir: None,
        },
    );
    store.save_skill_bindings();
    super::sessions::emit_session_event(&app, "session:list_changed", &sid, "created");

    Ok(StartSkillSessionResult {
        session: session.metadata,
        skill: ActiveSkillState {
            name: skill.name,
            phases,
            current_phase_id: first_phase,
        },
    })
}

/// `start_workflow` 启动一个新的工作流项目(所属工作流按 scenario 经 WorkflowRegistry 解析)。
///
/// 流程：
/// 1. 调 `harness::init_project(workspace, scenario, brief_init)` 在 workspace 下建
///    `ppt-<ts>-<scenario>/` 项目目录 + `_state/workflow_progress.json` + `_state/brief.json`
/// 2. 如未传 `session_id` → 新建一个 chat session；否则用现有 session 绑定到该项目
/// 3. 加载 `legacy-ppt-workflow` skill 拿 phases，把 session 绑定到该 skill + project_dir
/// 4. 持久化 binding 到 `_skill_bindings.json`（重启后能恢复）
/// 5. emit `workflow:project_started` + `workflow:full_state` 通知前端刷新
///
/// 注意：本命令**不主动 send_user_message** —— 前端负责切到 chat 标签页并把
/// `brief_init.user_request_raw` 预填到 input 框，等用户主动发送触发首个 turn。
/// 首个 turn 完成后 engine.rs H1 段会自然调 `harness::step_fresh` 启动需求分析师。
#[derive(Debug, Serialize)]
pub struct StartWorkflowResult {
    pub session_id: String,
    pub project_dir: String,
}

#[tauri::command]
pub async fn start_workflow(
    scenario: String,
    brief_init: Option<serde_json::Value>,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
    app: AppHandle,
) -> Result<StartWorkflowResult, String> {
    use crate::core::mode_state::ActiveSkillBinding;

    // 0. 按 scenario 解析所属工作流(WorkflowRegistry 扫 bundle/workflow/*/workflow.json)。
    //    enabled=false 只挡新建,历史项目不受影响(resolver 侧不过滤)。
    let wf =
        crate::features::workflow::workflow_registry::by_scenario(&scenario).ok_or_else(|| {
            format!("scenario `{scenario}` 没有对应的工作流(bundle/workflow/*/workflow.json)")
        })?;
    if !wf.enabled {
        return Err(format!(
            "工作流 `{}` 已禁用(workflow.json enabled=false)",
            wf.id
        ));
    }

    let brief = brief_init.unwrap_or_else(|| serde_json::json!({}));
    // 在 brief 被 move 进 spawn_blocking 前提取 session title 素材（owned String）。
    // 标题前缀 = workflow.json 的 name(多 scenario 工作流再拼 scenario id 区分)。
    let req_summary: String = brief
        .get("user_request_raw")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .chars()
        .take(16)
        .collect();
    let mut title_prefix = wf.name.clone().unwrap_or_else(|| wf.id.clone());
    if wf.scenarios.len() > 1 {
        title_prefix = format!("{title_prefix} · {scenario}");
    }
    let session_title = if req_summary.trim().is_empty() {
        title_prefix.clone()
    } else {
        format!("{title_prefix} · {}", req_summary.trim())
    };

    // 1. 决定宿主 session(每个工作流任务 = 一个隐藏宿主 session,仅作 SubAgent 运行时,
    //    见 SDAN 09 落地细则)。多 session 并发:不显式 sync engine,EnginePool 派发时
    //    get_or_spawn 为该 session 注水。
    //    ⚠️ 不 set_active [2026-06-04 白浪:chat 与工作流彻底分开]:工作流启动绝不抢
    //    用户当前 chat 会话;宿主 session 也不进侧栏(list_sessions 过滤)。
    let sid = if let Some(sid) = session_id {
        sid
    } else {
        let (model, model_id) = pool.default_model_for_new_session();
        let session = store
            .create_new(model, model_id, pool.bridge.workspace.clone())
            .map_err(|e| format!("create_session: {e:?}"))?;
        let sid = session.metadata.id.clone();
        // 人话 title，工作流页/调试时一眼看出是哪个 PPT 项目
        store.set_title(&sid, session_title.clone()).ok();
        sid
    };

    // 2. 在 engine 的实际执行工作区下初始化项目目录。普通聊天使用 session 私有目录，
    //    定时任务使用 automation 私有目录；harness forwarder 必须读取同一路径。
    let workspace = store
        .ledger_root(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    let project_dir = tokio::task::spawn_blocking({
        let workspace = workspace.clone();
        let scenario = scenario.clone();
        move || crate::features::assistant::harness::init_project(&workspace, &scenario, &brief)
    })
    .await
    .map_err(|e| format!("spawn_blocking init_project: {e}"))?
    .map_err(|e| format!("init_project: {e}"))?;
    let project_dir_str = project_dir.to_string_lossy().to_string();

    // 3.(已并入步骤 0)wf 由 WorkflowRegistry 按 scenario 解析,不再依赖 SkillRegistry
    //    /SKILL.md——工作流身份由 workflow.json 承载。

    // 4. 把 workflow 项目绑到 session(project_dir 给 harness 找项目 + session 列表标签)。
    //    ⚠️ 不塞 pending_instruction:那条"请按 skill 流程响应"会让品悟在
    //    首条消息时 load_skill 把手册拉进 context 自驱跑流程,绕过 harness(信任根:
    //    workflow 由 harness 按 workflow_progress.json 驱动,品悟绝不 load_skill 自驱)。
    //    启动入口是「让我们开始吧」→ kick_workflow → step_fresh 直接派发 Agent1,
    //    不经品悟自由 turn。
    //    ⚠️ 不塞 phases [2026-06-04 白浪:chat 与工作流不混淆]:此前把 SKILL.md(旧 16
    //    阶段手册化石)的 phases 塞进绑定 → chat 顶部 PhaseChips 渲染一条永不推进的
    //    节点列表(workflow 没人发 phase marker)。工作流进度只在 WorkflowView 看板看。
    store.bind_skill(
        &sid,
        ActiveSkillBinding {
            name: wf.id.clone(),
            pending_instruction: None,
            phases: Vec::new(),
            project_dir: Some(project_dir_str.clone()),
        },
    );
    store.save_skill_bindings();

    // 5. emit 事件让前端刷新（异步即可，失败忽略）
    let _ = app.emit(
        "workflow:project_started",
        serde_json::json!({
            "session_id": sid.clone(),
            "project_dir": project_dir_str.clone(),
            "scenario": scenario.clone(),
        }),
    );
    // 推一次全量状态（scheduler --status 输出）让 workflow 页立刻刷新
    {
        let ws = workspace.clone();
        let app_clone = app.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(state) = crate::features::assistant::harness::read_full_agent_state(&ws) {
                let _ = app_clone.emit("workflow:full_state", state);
            }
        });
    }

    Ok(StartWorkflowResult {
        session_id: sid,
        project_dir: project_dir_str,
    })
}

/// 「让我们开始吧」按钮调用：主动 kick harness `step_fresh` dispatch 第一个 agent
/// (需求分析师)，emit running + 派发真 SubAgent。点开始直接进调度;之后每个
/// agent 完成由 AgentComplete → step_after_role 链式推进(auto gate 自动过 /
/// human gate 等用户)。
#[tauri::command]
pub async fn kick_workflow(
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
    app: AppHandle,
) -> Result<String, String> {
    // 取本次工作流对应的 session(前端显式传;回退 active)。每个工作流 = 一个 session,
    // 绝不能匹配错——harness_phase / 项目目录全都按这个 sid 走。
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    let ws = store
        .ledger_root(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    let harness_workspace = ws.clone();
    let action = tokio::task::spawn_blocking(move || {
        crate::features::assistant::harness::step_fresh(&harness_workspace)
    })
    .await
    .map_err(|e| format!("spawn_blocking step_fresh: {e}"))?;

    match action {
        // [拆对话线 C] step_fresh 直接返回 SpawnAgent，Harness 直派真 SubAgent，
        // executing 态，主 session 空闲（无品悟交代/自演）。
        crate::features::assistant::harness::HarnessAction::SpawnAgent {
            role_id,
            role_name,
            prompt,
            allowed_tools,
            max_steps,
            output_schema,
            expects_file_output,
        } => {
            let engine = pool
                .get_or_spawn(&sid)
                .await
                .map_err(|e| format!("get engine for {sid}: {e:?}"))?;
            let _ = app.emit(
                "workflow:agent_state_changed",
                serde_json::json!({
                    "session_id": sid.clone(),
                    "role_id": role_id.clone(),
                    "role_name": role_name.clone(),
                    "status": "running",
                }),
            );
            let op = deepseek_tui::core::ops::Op::SpawnSubAgent {
                prompt,
                role_id,
                allowed_tools,
                max_steps,
                output_schema,
                expects_file_output,
            };
            engine
                .handle
                .send(op)
                .await
                .map_err(|e| format!("spawn subagent: {e:?}"))?;
            Ok(format!("spawning {role_name}"))
        }
        // [per_page] 纵向 fan-out：并发派 N 个 per-page SubAgent。
        crate::features::assistant::harness::HarnessAction::SpawnAgentBatch {
            base_role,
            role_name,
            tasks,
        } => {
            let engine = pool
                .get_or_spawn(&sid)
                .await
                .map_err(|e| format!("get engine for {sid}: {e:?}"))?;
            let _ = app.emit(
                "workflow:agent_state_changed",
                serde_json::json!({
                    "session_id": sid.clone(), "role_id": base_role.clone(),
                    "role_name": role_name.clone(), "status": "running",
                }),
            );
            let n = tasks.len();
            let k = crate::features::assistant::harness::per_page_concurrency();
            let first = crate::features::assistant::harness::batch_seed_and_take(
                &sid, &base_role, tasks, k,
            );
            for t in first {
                let op = deepseek_tui::core::ops::Op::SpawnSubAgent {
                    prompt: t.prompt,
                    role_id: t.agent_role,
                    allowed_tools: t.allowed_tools,
                    max_steps: t.max_steps,
                    output_schema: t.output_schema,
                    expects_file_output: t.expects_file_output,
                };
                engine
                    .handle
                    .send(op)
                    .await
                    .map_err(|e| format!("fan-out spawn: {e:?}"))?;
            }
            crate::features::assistant::engine::emit_fanout(&app, &sid, &base_role); // 初始 fan-out 状态 → 前端
            Ok(format!("spawning {role_name} ({n} pages, 在飞={k})"))
        }
        crate::features::assistant::harness::HarnessAction::Blocked { message } => {
            crate::features::assistant::engine::emit_workflow_blocked(&app, &sid, &ws, &message);
            let display_message = serde_json::from_str::<serde_json::Value>(&message)
                .ok()
                .as_ref()
                .and_then(crate::features::assistant::harness::warmup_block_reason)
                .unwrap_or(message);
            Err(format!("工作流启动失败：{display_message}"))
        }
        crate::features::assistant::harness::HarnessAction::Error(error) => {
            crate::features::assistant::harness::record_runtime_failure(
                &ws,
                "",
                "scheduler_kick",
                &error,
            );
            let message = format!("工作流调度失败：{error}");
            crate::features::assistant::engine::emit_workflow_blocked(&app, &sid, &ws, &message);
            Err(message)
        }
        crate::features::assistant::harness::HarnessAction::NotApplicable => {
            Ok("no dispatch (already running or not applicable)".to_string())
        }
        crate::features::assistant::harness::HarnessAction::WaitForHuman { .. }
        | crate::features::assistant::harness::HarnessAction::AllDone => {
            Ok("no dispatch (workflow is waiting or complete)".to_string())
        }
    }
}

/// 从失败节点续跑:重置 `role_id` 为 pending(清重试),然后重新调度。
/// 复用 harness::retry_role(reset + step_fresh) + kick 的 action→Op 派发逻辑。
/// 用户在失败节点卡片点"🔄 重跑"→走这里→该角色重新 spawn(用最新提示词),
/// 上游已 completed 节点不重跑(State 里仍 completed)。
#[tauri::command]
pub async fn retry_workflow_role(
    role_id: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
    app: AppHandle,
) -> Result<String, String> {
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    let ws = store
        .ledger_root(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    let rid = role_id.clone();
    let action = tokio::task::spawn_blocking(move || {
        crate::features::assistant::harness::retry_role(&ws, &rid)
    })
    .await
    .map_err(|e| format!("spawn_blocking retry_role: {e}"))?;

    match action {
        crate::features::assistant::harness::HarnessAction::SpawnAgent {
            role_id,
            role_name,
            prompt,
            allowed_tools,
            max_steps,
            output_schema,
            expects_file_output,
        } => {
            let engine = pool
                .get_or_spawn(&sid)
                .await
                .map_err(|e| format!("get engine for {sid}: {e:?}"))?;
            let _ = app.emit(
                "workflow:agent_state_changed",
                serde_json::json!({
                    "session_id": sid.clone(),
                    "role_id": role_id.clone(),
                    "role_name": role_name.clone(),
                    "status": "running",
                }),
            );
            let op = deepseek_tui::core::ops::Op::SpawnSubAgent {
                prompt,
                role_id,
                allowed_tools,
                max_steps,
                output_schema,
                expects_file_output,
            };
            engine
                .handle
                .send(op)
                .await
                .map_err(|e| format!("spawn subagent: {e:?}"))?;
            Ok(format!("retry → spawning {role_name}"))
        }
        // [per_page] retry 重派整批（fan-out）。
        crate::features::assistant::harness::HarnessAction::SpawnAgentBatch {
            base_role,
            role_name,
            tasks,
        } => {
            let engine = pool
                .get_or_spawn(&sid)
                .await
                .map_err(|e| format!("get engine for {sid}: {e:?}"))?;
            let _ = app.emit(
                "workflow:agent_state_changed",
                serde_json::json!({
                    "session_id": sid.clone(), "role_id": base_role.clone(),
                    "role_name": role_name.clone(), "status": "running",
                }),
            );
            let n = tasks.len();
            let k = crate::features::assistant::harness::per_page_concurrency();
            let first = crate::features::assistant::harness::batch_seed_and_take(
                &sid, &base_role, tasks, k,
            );
            for t in first {
                let op = deepseek_tui::core::ops::Op::SpawnSubAgent {
                    prompt: t.prompt,
                    role_id: t.agent_role,
                    allowed_tools: t.allowed_tools,
                    max_steps: t.max_steps,
                    output_schema: t.output_schema,
                    expects_file_output: t.expects_file_output,
                };
                engine
                    .handle
                    .send(op)
                    .await
                    .map_err(|e| format!("fan-out spawn: {e:?}"))?;
            }
            crate::features::assistant::engine::emit_fanout(&app, &sid, &base_role); // 初始 fan-out 状态 → 前端
            Ok(format!(
                "retry → spawning {role_name} ({n} pages, 在飞={k})"
            ))
        }
        crate::features::assistant::harness::HarnessAction::Blocked { message } => {
            Err(format!("retry blocked: {message}"))
        }
        crate::features::assistant::harness::HarnessAction::Error(e) => {
            Err(format!("retry error: {e}"))
        }
        _ => Ok("retry: no dispatch (check role state)".to_string()),
    }
}

/// 取一个角色的 system prompt（`roles/<role_id>.md`）+ registry meta（tools/model/max_steps 等）。
/// 详情 Drawer 的 "Role Prompt" Tab 用。
#[derive(Debug, Serialize)]
pub struct RolePromptPayload {
    pub role_id: String,
    pub prompt_md: String,
    pub registry_meta: serde_json::Value,
}

/// [B2] 差事节点 id（`<bu>~<seq>`）拆出所属部 + 序号;非差事节点返回 (role_id, None)。
/// 分隔符 `~` 与 dispatch_graph.py / harness.rs::bu_of 一致(不用 `#`，避开 per_page 页实例)。
fn split_task_node(role_id: &str) -> (&str, Option<&str>) {
    match role_id.split_once('~') {
        Some((bu, seq)) => (bu, Some(seq)),
        None => (role_id, None),
    }
}

#[tauri::command]
pub async fn get_role_prompt(
    role_id: String,
    project_dir: Option<String>,
) -> Result<RolePromptPayload, String> {
    // 按项目 scenario 解析所属工作流;没传 project_dir 时按角色反查(角色跨工作流不重叠)
    let workflow = project_dir
        .as_deref()
        .map(|p| crate::features::assistant::harness::workflow_of_project(std::path::Path::new(p)))
        .unwrap_or_else(|| crate::features::assistant::harness::workflow_of_role(&role_id));
    let skills_dir = crate::features::assistant::harness::workflow_root_for(&workflow);
    let registry_path = skills_dir.join("agent_registry.json");
    let registry: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&registry_path)
            .map_err(|e| format!("read agent_registry.json: {e}"))?,
    )
    .map_err(|e| format!("parse agent_registry.json: {e}"))?;

    // [B2] 差事节点(libu~1)按所属部(libu)查 registry 能力/prompt;非差事原样。
    let (bu, seq) = split_task_node(&role_id);
    let mut role_meta = registry
        .get("agents")
        .and_then(|a| a.get(bu))
        .cloned()
        .ok_or_else(|| format!("role {bu} not found in agent_registry.json"))?;

    let prompt_file = role_meta
        .get("prompt_file")
        .and_then(|p| p.as_str())
        .ok_or_else(|| format!("prompt_file missing for {bu}"))?;
    let prompt_path = skills_dir.join(prompt_file);
    let mut prompt_md = std::fs::read_to_string(&prompt_path)
        .map_err(|e| format!("read prompt_file {}: {e}", prompt_path.display()))?;

    // [B2] 差事节点增强:读 dynamic_routes.json 把"这次的差事"内容带进卡片——
    // 否则 libu~1/libu~2 显示同一份静态 playbook,操作员分不清哪张卡干什么。
    if seq.is_some() {
        if let Some(pd) = project_dir.as_deref() {
            let dr_path = std::path::PathBuf::from(pd)
                .join("_state")
                .join("dynamic_routes.json");
            if let Ok(txt) = std::fs::read_to_string(&dr_path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                    if let Some(node) = v.get("task_nodes").and_then(|t| t.get(&role_id)) {
                        let g = |k: &str| node.get(k).and_then(|x| x.as_str()).unwrap_or("");
                        let (title, task, reqs, out_file) =
                            (g("title"), g("task"), g("requirements"), g("output_file"));
                        let wave = node.get("wave").and_then(|x| x.as_u64());
                        // registry_meta: 改名「部·差事标题」+ 产物指向差事专属文件
                        if let Some(obj) = role_meta.as_object_mut() {
                            let bu_name = obj
                                .get("name")
                                .and_then(|x| x.as_str())
                                .unwrap_or(bu)
                                .to_string();
                            if !title.is_empty() {
                                obj.insert(
                                    "name".into(),
                                    serde_json::Value::String(format!("{bu_name}·{title}")),
                                );
                            }
                            if !out_file.is_empty() {
                                obj.insert("outputs".into(), serde_json::json!([out_file]));
                            }
                            if let Some(w) = wave {
                                obj.insert("wave".into(), serde_json::json!(w));
                            }
                        }
                        // prompt 顶部注入这次差事(以此为准),与 scheduler.build_full_prompt 同款标题
                        let mut head = String::from("## 📋 你这次的差事（以此为准）\n\n");
                        if let Some(w) = wave {
                            head.push_str(&format!("> 第 {w} 批 · {bu}\n\n"));
                        }
                        if !title.is_empty() {
                            head.push_str(&format!("**{title}**\n\n"));
                        }
                        head.push_str(task);
                        head.push('\n');
                        if !reqs.is_empty() {
                            head.push_str(&format!("\n**具体要求**：{reqs}\n"));
                        }
                        head.push_str("\n---\n\n");
                        prompt_md = format!("{head}{prompt_md}");
                    }
                }
            }
        }
    }

    Ok(RolePromptPayload {
        role_id,
        prompt_md,
        registry_meta: role_meta,
    })
}

/// 取一个角色在指定 project_dir 下实际产出的文件（按 agent_registry.outputs glob）。
/// 详情 Drawer 的 "产出文件" Tab 用。
#[derive(Debug, Serialize)]
pub struct OutputFile {
    pub path: String,
    pub size: u64,
    pub mtime_ms: u64,
    /// 前 4000 字节文本预览（二进制返回 "[binary {size} bytes]"）
    pub preview: String,
}

#[tauri::command]
pub async fn get_role_outputs(
    role_id: String,
    project_dir: String,
) -> Result<Vec<OutputFile>, String> {
    let workflow = crate::features::assistant::harness::workflow_of_project(std::path::Path::new(
        &project_dir,
    ));
    let skills_dir = crate::features::assistant::harness::workflow_root_for(&workflow);
    let registry: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(skills_dir.join("agent_registry.json"))
            .map_err(|e| format!("read registry: {e}"))?,
    )
    .map_err(|e| format!("parse registry: {e}"))?;
    // [B2] 差事节点(libu~1)产物固定 deliverables/<bu>_<seq>.md(与 dispatch_graph 编译一致);
    // 非差事节点照旧用所属部的 registry outputs glob。
    let (bu, seq) = split_task_node(&role_id);
    let outputs: Vec<serde_json::Value> = if let Some(seq) = seq {
        vec![serde_json::Value::String(format!(
            "deliverables/{bu}_{seq}.md"
        ))]
    } else {
        registry
            .get("agents")
            .and_then(|a| a.get(bu))
            .and_then(|r| r.get("outputs"))
            .and_then(|o| o.as_array())
            .cloned()
            .unwrap_or_default()
    };
    let project = std::path::PathBuf::from(&project_dir);
    if !project.exists() {
        return Err(format!("project_dir not found: {project_dir}"));
    }
    let mut files: Vec<OutputFile> = Vec::new();
    for pat in outputs {
        let Some(pat) = pat.as_str() else { continue };
        let abs_pattern = project.join(pat);
        // 简易 glob：含 `*` 时 enumerate 父目录扩展名匹配；否则直接 stat
        if abs_pattern.to_string_lossy().contains('*') {
            let parent = match abs_pattern.parent() {
                Some(p) => p.to_path_buf(),
                None => continue,
            };
            let file_name_pat = abs_pattern
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if !parent.exists() {
                continue;
            }
            let entries = match std::fs::read_dir(&parent) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let p = entry.path();
                if !p.is_file() {
                    continue;
                }
                let name = p
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                if !simple_glob_match(&file_name_pat, &name) {
                    continue;
                }
                if let Some(of) = stat_output(&p) {
                    files.push(of);
                }
            }
        } else if abs_pattern.exists() && abs_pattern.is_file() {
            if let Some(of) = stat_output(&abs_pattern) {
                files.push(of);
            }
        }
    }
    Ok(files)
}

fn simple_glob_match(pattern: &str, name: &str) -> bool {
    // 仅支持单个 `*` 通配符（典型用例：*.md / *.html）。够当前 outputs 字段用。
    if let Some(idx) = pattern.find('*') {
        let prefix = &pattern[..idx];
        let suffix = &pattern[idx + 1..];
        name.starts_with(prefix)
            && name.ends_with(suffix)
            && name.len() >= prefix.len() + suffix.len()
    } else {
        pattern == name
    }
}

fn stat_output(path: &std::path::Path) -> Option<OutputFile> {
    let meta = std::fs::metadata(path).ok()?;
    let size = meta.len();
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let preview = if size > 0 {
        let mut buf = vec![0u8; size.min(4000) as usize];
        if let Ok(mut f) = std::fs::File::open(path) {
            use std::io::Read;
            let _ = f.read_exact(&mut buf);
        }
        match std::str::from_utf8(&buf) {
            Ok(s) => s.to_string(),
            Err(_) => format!("[binary {size} bytes]"),
        }
    } else {
        String::new()
    };
    Some(OutputFile {
        path: path.to_string_lossy().to_string(),
        size,
        mtime_ms,
        preview,
    })
}

pub(super) fn read_role_logs_from_project(
    project_dir: &std::path::Path,
    role_id: &str,
    tail: usize,
) -> Result<Vec<serde_json::Value>, String> {
    let state_dir = project_dir.join("_state");
    let mut records: Vec<(String, usize, serde_json::Value)> = Vec::new();
    let mut sequence = 0usize;
    for file_name in ["flow_log.jsonl", "agent_log.jsonl", "workflow_flow.log"] {
        let path = state_dir.join(file_name);
        if !path.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(mut record) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let matches = record
                .get("role_id")
                .and_then(serde_json::Value::as_str)
                .map(|record_role| record_role == role_id)
                .unwrap_or(true);
            if !matches {
                continue;
            }
            if let Some(object) = record.as_object_mut() {
                object
                    .entry("source")
                    .or_insert_with(|| serde_json::Value::String(file_name.to_string()));
            }
            let timestamp = record
                .get("timestamp")
                .or_else(|| record.get("ts"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            records.push((timestamp, sequence, record));
            sequence += 1;
        }
    }
    records.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    let mut out = records
        .into_iter()
        .map(|(_, _, record)| record)
        .collect::<Vec<_>>();

    // flow 日志写入失败或旧版本没有日志时，workflow_progress 仍是角色状态真相源。
    // 将其中的终态错误补成最后一条诊断，确保抽屉一定能看到具体失败原因。
    let progress_path = state_dir.join("workflow_progress.json");
    if let Ok(content) = std::fs::read_to_string(&progress_path) {
        if let Ok(progress) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(role) = progress.get("roles").and_then(|roles| roles.get(role_id)) {
                let status = role
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let reason = role
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|reason| !reason.is_empty());
                if matches!(status, "failed" | "blocked") {
                    if let Some(reason) = reason {
                        out.push(serde_json::json!({
                            "timestamp": progress.get("updated_at").cloned().unwrap_or(serde_json::Value::Null),
                            "layer": "state",
                            "event": "failure_state",
                            "role_id": role_id,
                            "status": status,
                            "reason": reason,
                            "source": "workflow_progress.json",
                        }));
                    }
                }
            }
        }
    }

    let limit = tail.clamp(1, 1000);
    let start = out.len().saturating_sub(limit);
    Ok(out[start..].to_vec())
}

/// 取一个角色的执行日志尾部 N 条。新日志来自 `_state/flow_log.jsonl` 和
/// `_state/agent_log.jsonl`，同时兼容旧版 `_state/workflow_flow.log`。
/// 详情 Drawer 的“运行日志”区域使用。
#[tauri::command]
pub async fn get_role_logs(
    role_id: String,
    project_dir: String,
    tail: Option<usize>,
) -> Result<Vec<serde_json::Value>, String> {
    read_role_logs_from_project(
        std::path::Path::new(&project_dir),
        &role_id,
        tail.unwrap_or(200),
    )
}

/// 取一个角色最近一份 gate 报告（来自 `_state/gate_reports/<role>_<ts>.json`）。
/// 详情 Drawer 的 "Gate Report" Tab 用。
#[tauri::command]
pub async fn get_gate_report(
    role_id: String,
    project_dir: String,
) -> Result<Option<serde_json::Value>, String> {
    let dir = std::path::PathBuf::from(&project_dir)
        .join("_state")
        .join("gate_reports");
    if !dir.exists() {
        return Ok(None);
    }
    let prefix = format!("{role_id}_");
    let mut latest: Option<(u64, std::path::PathBuf)> = None;
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("read gate_reports: {e}"))? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let p = entry.path();
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with(&prefix) || !name.ends_with(".json") {
            continue;
        }
        let mtime_ms = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        match &latest {
            Some((m, _)) if *m >= mtime_ms => {}
            _ => latest = Some((mtime_ms, p)),
        }
    }
    let Some((_, path)) = latest else {
        return Ok(None);
    };
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("read gate report {}: {e}", path.display()))?;
    let mut v: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("parse gate report: {e}"))?;
    // 附 _report_path 给前端调试
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "_report_path".into(),
            serde_json::Value::String(path.to_string_lossy().to_string()),
        );
    }
    Ok(Some(v))
}

/// 保存项目级配置到 `_state/brief.json`（merge patch，不污染 agent_registry.json ground truth）。
#[tauri::command]
pub async fn save_project_config(
    project_dir: String,
    brief_patch: serde_json::Value,
) -> Result<(), String> {
    let brief_path = std::path::PathBuf::from(&project_dir)
        .join("_state")
        .join("brief.json");
    let mut brief = if brief_path.exists() {
        serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(&brief_path).map_err(|e| format!("read brief: {e}"))?,
        )
        .map_err(|e| format!("parse brief: {e}"))?
    } else {
        serde_json::json!({})
    };
    if let (Some(obj), Some(patch_obj)) = (brief.as_object_mut(), brief_patch.as_object()) {
        for (k, v) in patch_obj {
            obj.insert(k.clone(), v.clone());
        }
    } else {
        return Err("brief.json or patch must be JSON object".to_string());
    }
    std::fs::write(
        &brief_path,
        serde_json::to_string_pretty(&brief).map_err(|e| format!("serialize: {e}"))?,
    )
    .map_err(|e| format!("write brief: {e}"))?;
    Ok(())
}

/// 保存 per-project agent 级配置覆盖到 `_state/agent_overrides.json`（merge）。
/// scheduler.py 在 load_role_prompt / build_full_prompt 旁加 apply_overrides() 读这里。
#[tauri::command]
pub async fn save_agent_overrides(
    project_dir: String,
    role_id: String,
    patch: serde_json::Value,
) -> Result<(), String> {
    let path = std::path::PathBuf::from(&project_dir)
        .join("_state")
        .join("agent_overrides.json");
    let mut all = if path.exists() {
        serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(&path).map_err(|e| format!("read: {e}"))?,
        )
        .unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    let obj = all
        .as_object_mut()
        .ok_or_else(|| "agent_overrides.json must be JSON object".to_string())?;
    let role_entry = obj.entry(role_id.clone()).or_insert(serde_json::json!({}));
    if let (Some(role_obj), Some(patch_obj)) = (role_entry.as_object_mut(), patch.as_object()) {
        for (k, v) in patch_obj {
            role_obj.insert(k.clone(), v.clone());
        }
    } else if patch.is_object() {
        *role_entry = patch.clone();
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&all).map_err(|e| format!("serialize: {e}"))?,
    )
    .map_err(|e| format!("write: {e}"))?;
    Ok(())
}

/// 取消正在执行的某个角色：scheduler --fail role --reason cancel。
/// [C2] harness_phase 已删,无需再清；调度状态由 State(workflow_progress.json)持有。
#[tauri::command]
pub async fn cancel_workflow_role(
    role_id: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
) -> Result<serde_json::Value, String> {
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    let workspace = store
        .ledger_root(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    let rid = role_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        // 找到 project_dir
        let project = match crate::features::assistant::harness::find_project_dir(&workspace) {
            Some(p) => p,
            None => return Err("no project found".to_string()),
        };
        // 读 scenario
        let scenario_content =
            std::fs::read_to_string(project.join("_state").join("workflow_progress.json"))
                .unwrap_or_default();
        let scenario = serde_json::from_str::<serde_json::Value>(&scenario_content)
            .ok()
            .and_then(|v| v.get("scenario").and_then(|s| s.as_str()).map(String::from))
            .unwrap_or_else(|| "solution_deck".to_string());
        // 走 scheduler 通用入口（用 std::process::Command 直接调）
        let scheduler = crate::features::assistant::harness::scheduler_path_for(
            &crate::features::assistant::harness::workflow_name_for_scenario(&scenario),
        );
        let output =
            crate::platform::process::HiddenCommand::new(crate::platform::paths::python_command())
                .args([
                    scheduler.to_string_lossy().as_ref(),
                    project.to_string_lossy().as_ref(),
                    "--scenario",
                    &scenario,
                    "--fail",
                    &rid,
                    "--reason",
                    "user_cancelled",
                ])
                .output()
                .map_err(|e| format!("scheduler --fail: {e}"))?;
        Ok(serde_json::json!({
            "ok": output.status.success(),
            "stdout": String::from_utf8_lossy(&output.stdout).to_string(),
            "stderr": String::from_utf8_lossy(&output.stderr).to_string(),
        }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?;
    result
}

/// 用户主动停止整个工作流。
///
/// 顺序不可交换：先持久化 stop marker（挡住竞态中的迟到 AgentComplete），再通过
/// 底座显式取消该 session 的全部后台 SubAgent。返回原始 brief，供前端预填“修改需求
/// 并重新开始”；旧 run 保留现场，不在原状态上复活。
#[tauri::command]
pub async fn stop_workflow(
    session_id: Option<String>,
    reason: Option<String>,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    let workspace = store
        .ledger_root(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    let stop_reason = reason
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "user_stopped".to_string());

    let ws = workspace.clone();
    let marker_sid = sid.clone();
    let marker_reason = stop_reason.clone();
    let result = tokio::task::spawn_blocking(move || {
        crate::features::assistant::harness::stop_workflow(&ws, &marker_sid, &marker_reason)
    })
    .await
    .map_err(|e| format!("spawn_blocking stop_workflow: {e}"))??;

    if let Some(engine) = pool.handle_for(&sid).await {
        if let Err(e) = engine
            .handle
            .send(deepseek_tui::core::ops::Op::CancelSubAgents)
            .await
        {
            // stop marker 已成功落盘，是不可回滚的调度真相；engine 恰好退出只表示
            // 没有存活 worker 可取消，不应把 UI 留在“停止失败”。
            eprintln!("[workflow] stop marker persisted but cancel op failed: {e:?}");
        }
    }

    let _ = app.emit(
        "workflow:stopped",
        serde_json::json!({
            "session_id": sid,
            "reason": stop_reason,
            "stopped_at": result.get("stopped_at"),
        }),
    );
    Ok(result)
}

/// 用户审批通过 workflow gate → 标记角色完成 → 继续推进 harness loop。
/// 前端在审批卡片上点"确认"时调用。
#[tauri::command]
pub async fn approve_workflow_gate(
    role_id: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    let workspace = store
        .ledger_root(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    let engine = pool
        .get_or_spawn(&sid)
        .await
        .map_err(|e| format!("get engine for {sid}: {e:?}"))?;
    let rid = role_id.clone();
    let action = tokio::task::spawn_blocking(move || {
        crate::features::assistant::harness::approve_gate(&workspace, &rid)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?;
    // approve 后 step_fresh 推进到下一角色：SpawnAgent（直派）/ AllDone / WaitForHuman。
    // 用 apply_harness_action 统一处理（set phase / emit / 派发），其值化结果回前端。
    let next_label = match &action {
        crate::features::assistant::harness::HarnessAction::SpawnAgent { .. } => "dispatch",
        crate::features::assistant::harness::HarnessAction::AllDone => "all_done",
        crate::features::assistant::harness::HarnessAction::WaitForHuman { .. } => "waiting",
        crate::features::assistant::harness::HarnessAction::Blocked { .. } => "blocked",
        _ => "noop",
    };
    let handled = crate::features::assistant::engine::apply_harness_action(
        action,
        &app,
        &engine.workspace,
        &engine.handle,
        &sid,
    )
    .await;
    Ok(serde_json::json!({"ok": handled, "next": next_label}))
}

/// 用户审批拒绝 workflow gate → 让角色重做。
#[tauri::command]
pub async fn reject_workflow_gate(
    role_id: String,
    reason: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    let workspace = store
        .ledger_root(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    let engine = pool
        .get_or_spawn(&sid)
        .await
        .map_err(|e| format!("get engine for {sid}: {e:?}"))?;
    let rid = role_id.clone();
    let r = reason.clone();
    let action = tokio::task::spawn_blocking(move || {
        crate::features::assistant::harness::reject_gate(&workspace, &rid, &r)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?;
    // reject 后 reject_gate 返回 SpawnAgent（重新派发同角色 SubAgent，附拒绝原因）。
    let next_label = match &action {
        crate::features::assistant::harness::HarnessAction::SpawnAgent { .. } => "redo",
        crate::features::assistant::harness::HarnessAction::Blocked { .. } => "blocked",
        _ => "noop",
    };
    let handled = crate::features::assistant::engine::apply_harness_action(
        action,
        &app,
        &engine.workspace,
        &engine.handle,
        &sid,
    )
    .await;
    Ok(serde_json::json!({"ok": handled, "next": next_label}))
}

/// 解除指定 session 的 skill 绑定(用户点 chips 区 ✕)。
/// 不删 session,只清绑定 — chips strip 隐藏,普通对话照常继续。
/// 前端拉取工作流全量 agent 状态（初始化 + 切到工作流页时用）。
#[tauri::command]
pub async fn get_workflow_state(
    session_id: Option<String>,
    store: State<'_, SessionStore>,
) -> Result<serde_json::Value, String> {
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    let workspace = store
        .ledger_root(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    tokio::task::spawn_blocking(move || {
        crate::features::assistant::harness::read_full_agent_state(&workspace)
            .unwrap_or(serde_json::json!(null))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))
}

/// [2026-06-06] 找最近一个「进行中」的工作流 run，供 app 启动后前端自动恢复看板。
/// 扫所有 session 的 skill binding：有 project_dir（=工作流会话）且 workflow_progress.json
/// 里存在未完成角色的，按 progress 文件 mtime 取最近一个。
/// 返回 {session_id, project_dir, scenario}，无则返回 null。
#[tauri::command]
pub async fn find_resumable_run(
    store: State<'_, SessionStore>,
) -> Result<serde_json::Value, String> {
    let metas = store.list().map_err(|e| format!("list: {e:?}"))?;
    let mut best: Option<(std::time::SystemTime, String, String, String)> = None;
    for m in metas {
        let Some(binding) = store.active_skill(&m.id) else {
            continue;
        };
        let Some(pd) = binding.project_dir else {
            continue;
        };
        let progress = std::path::Path::new(&pd)
            .join("_state")
            .join("workflow_progress.json");
        if std::path::Path::new(&pd)
            .join("_state")
            .join("workflow_stopped.json")
            .is_file()
        {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&progress) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) else {
            continue;
        };
        // 未全完成 = roles 非空且存在 status != completed 的角色
        let unfinished = v
            .get("roles")
            .and_then(|r| r.as_object())
            .is_some_and(|rs| {
                !rs.is_empty()
                    && rs
                        .values()
                        .any(|r| r.get("status").and_then(|s| s.as_str()) != Some("completed"))
            });
        if !unfinished {
            continue;
        }
        let Some(scenario) = v.get("scenario").and_then(|s| s.as_str()).map(String::from) else {
            continue;
        };
        // scenario 已没有对应工作流(如已下线存档的 legacy-ppt-workflow 项目)→ 跳过。
        // 否则老 PPT 半途 run(永远不会完成)mtime 最新时,每次开机都恢复进僵尸会话。
        if crate::features::workflow::workflow_registry::by_scenario(&scenario).is_none() {
            continue;
        }
        let mtime = std::fs::metadata(&progress)
            .and_then(|md| md.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        if best.as_ref().is_none_or(|(bt, _, _, _)| mtime > *bt) {
            best = Some((mtime, m.id.clone(), pd.clone(), scenario));
        }
    }
    Ok(match best {
        Some((_, sid, pd, scenario)) => serde_json::json!({
            "session_id": sid, "project_dir": pd, "scenario": scenario
        }),
        None => serde_json::Value::Null,
    })
}

/// 列出已发现且 enabled 的工作流(含 ui 块),给前端模板页/新建表单数据驱动渲染。
/// 加第 N 个工作流 = 丢一份 workflow.json + bundle 嵌入表加一行,前端零改动。
#[tauri::command]
pub async fn list_workflows() -> Result<Vec<serde_json::Value>, String> {
    Ok(crate::features::workflow::workflow_registry::discover()
        .into_iter()
        .filter(|w| w.enabled)
        .map(|w| {
            serde_json::json!({
                "id": w.id,
                "name": w.name,
                "scenarios": w.scenarios,
                "ui": w.ui,
            })
        })
        .collect())
}

#[tauri::command]
pub async fn unbind_session_skill(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    store.unbind_skill(&session_id);
    Ok(())
}

/// 拉取指定 session 当前绑定的 skill 信息(给前端切 session 后渲染 chips)。
/// 没绑定返回 None。
#[tauri::command]
pub async fn get_session_active_skill(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<Option<ActiveSkillState>, String> {
    Ok(store.active_skill(&session_id).map(|b| {
        // [2026-06-04 白浪:chat 与工作流不混淆] workflow 绑定(带 project_dir)不回传
        // phases——兜住磁盘上历史持久化的旧绑定(带 SKILL.md 化石 phases),否则旧工作流
        // session 切回来 chat 顶部仍渲染节点条。skill 会话(无 project_dir)不受影响。
        let phases = if b.project_dir.is_some() {
            Vec::new()
        } else {
            b.phases
        };
        let first: Option<String> = None;
        let _ = &phases;
        ActiveSkillState {
            name: b.name,
            phases,
            current_phase_id: first,
        }
    }))
}

/// 拉取所有 session 当前绑定的 skill 名(给 session 列表卡片显示标签用)。
/// 返回 `{ session_id: skill_name }` 映射;没绑定的 session 不在 map 里。
/// in-memory only — app 重启后 binding 全部丢失(跟 mode_state 一致设计)。
#[tauri::command]
pub async fn list_session_skill_bindings(
    store: State<'_, SessionStore>,
) -> Result<std::collections::HashMap<String, String>, String> {
    let metas = store.list().map_err(|e| format!("list_sessions: {e:?}"))?;
    let mut out = std::collections::HashMap::new();
    for m in metas {
        if let Some(b) = store.active_skill(&m.id) {
            out.insert(m.id, b.name);
        }
    }
    Ok(out)
}
use super::prelude::*;
