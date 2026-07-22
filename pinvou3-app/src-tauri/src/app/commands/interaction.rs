/// 手动触发上下文压缩。用户点 token 进度条 → 立即压缩当前对话历史。
/// 触发后 engine 会发 CompactionStarted / Completed / Failed 事件，
/// 通过 chat:compaction 系列 event 通知前端。
#[tauri::command]
pub async fn compact_now(
    session_id: Option<String>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    pool.compact_now(&sid)
        .await
        .map_err(|e| format!("compact_now: {e:?}"))
}

// ===================== 阶段 D: Plan / YOLO 双模式 =====================

/// 查询当前 session 的 mode 状态（前端启动 / 切换 session 时拉一次）。
#[tauri::command]
pub async fn get_mode_state(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<SessionModeState, String> {
    Ok(store.mode_state(&session_id))
}

// ===================== 卡片池: 专家面具 =====================

/// 列出全部专家卡的**摘要**（不含 body，~200 卡）。前端进卡片池拉一次缓存。
/// Side B: body 太大（~6K 字/张），不随 list 下发，加持/详情时按需取。

/// 用户在 composer chip 选 Plan：设 mode=Plan。
/// 下一条 chat 消息带 mode=Plan 发送，底座自动切只读工具集 + ReadOnly sandbox。
#[tauri::command]
pub async fn set_plan_mode_next(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<SessionModeState, String> {
    store
        .set_mode(&session_id, SerializableMode::Plan)
        .map_err(|error| format!("set_plan_mode_next({session_id}): {error:#}"))?;
    Ok(store.mode_state(&session_id))
}

/// 用户在 composer chip 选 Yolo（从 Plan 退回）：mode 切 Yolo。
/// 对话历史天然保留，AI 在 YOLO 下能看到之前讨论的 context。
#[tauri::command]
pub async fn exit_plan_to_yolo(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<SessionModeState, String> {
    store
        .set_mode(&session_id, SerializableMode::Yolo)
        .map_err(|error| format!("exit_plan_to_yolo({session_id}): {error:#}"))?;
    Ok(store.mode_state(&session_id))
}

/// `accept_plan` 切 Yolo 后注入的执行指令文本。抽成函数供单测钉契约:
/// 必须裹住方案全文 + 带明确"立即执行"信号,否则切了 Yolo 但 AI 收到空指令不知道干嘛。
fn accept_plan_instruction(plan_markdown: &str) -> String {
    format!("用户已批准方案,立即开始执行。方案:\n\n{plan_markdown}")
}

/// 用户点 plan_card [✅ 就这么干]：接受 plan，切 YOLO 执行(对齐底座 accept-yolo)。
/// 流程：
///   1. 设 mode=Yolo
///   2. 用 plan_markdown 作为指令前缀发一条 user message 触发执行(底座共享 PlanState 仍在)
/// 前端在调用前应在消息流追加 user 气泡显示「✅ 就这么干」让用户感知。
#[tauri::command]
pub async fn accept_plan(
    session_id: String,
    plan_markdown: String,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<SessionModeState, String> {
    store
        .set_mode(&session_id, SerializableMode::Yolo)
        .map_err(|error| format!("accept_plan({session_id}): {error:#}"))?;
    let instruction = accept_plan_instruction(&plan_markdown);
    pool.send_user_message(
        &session_id,
        instruction,
        SerializableMode::Yolo.to_app_mode(),
        false,
    )
    .await
    .map_err(|e| format!("accept_plan send_user_message: {e:?}"))?;
    Ok(store.mode_state(&session_id))
}

/// 超级权限开关：当前用户能否跑 sudo 免密。
/// 源真相 = `/etc/sudoers.d/pinvou3` 是否存在；前端启动时调一次同步 UI 状态。
#[tauri::command]
pub async fn get_super_permission_status() -> Result<bool, String> {
    Ok(crate::super_permission::is_enabled())
}

/// 切换超级权限。开启时 pkexec 弹系统密码框写 sudoers，关闭时 pkexec 删文件。
/// 切换后同步当前 session 让新 system prompt 立即生效（注入/抹掉 sudo 引导段）。
/// 返回真实生效状态（pkexec 失败/取消时不会变）。
#[tauri::command]
pub async fn set_super_permission(
    enabled: bool,
    pool: State<'_, EnginePool>,
) -> Result<bool, String> {
    if enabled {
        crate::super_permission::enable()?;
    } else {
        crate::super_permission::disable()?;
    }
    // 多 session 并发:重写所有已起 engine 的 session 专属 instructions(含新 sudo 引导块),
    // engine 下个 turn rehydrate 时从 disk 重读 → 「下次 turn 生效」。低频操作,不为即时
    // 生效去 SyncSession 打断在跑的 turn。未起的 session 首次 spawn 时自然带上新引导。
    pool.refresh_all_instructions().await;
    Ok(crate::super_permission::is_enabled())
}

/// 读 pinvou3 内置 skill 的 body(去掉 frontmatter)。
/// 用途:前端 autoTriggerPinvouReview 把完整 SKILL.md 内容塞进 user message,
/// 不依赖本地 Qwen3.6 主动 read_file —— 弱模型不会主动用 progressive disclosure。
/// 设计依据:docs/Pinvou-品悟设计.md §10.5 (即将补)
#[tauri::command]
pub async fn read_skill_body(name: String) -> Result<String, String> {
    use crate::bridge::paths;
    let safe_name: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if safe_name != name || safe_name.is_empty() {
        return Err(format!("invalid skill name: {name}"));
    }
    // h3c-ppt 在 workflow/,review 等 skill 在 skills/;先查 workflow 再 fallback skills。
    let wf_path = paths::bundle_workflow_dir()
        .join(&safe_name)
        .join("SKILL.md");
    let path = if wf_path.is_file() {
        wf_path
    } else {
        paths::bundle_skills_dir().join(&safe_name).join("SKILL.md")
    };
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("read SKILL.md ({}): {e}", path.display()))?;
    // 剥 frontmatter ---\n...\n---\n
    let body = if let Some(rest) = content.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            rest[end + 5..].trim_start().to_string()
        } else if let Some(end) = rest.find("\n---") {
            rest[end + 4..].trim_start().to_string()
        } else {
            content
        }
    } else {
        content
    };
    Ok(body)
}

// 修法 D 删除了 revise_plan 命令.
// 用户点 [✏️ 改改] 时前端走 DeepSeek-TUI 底座做法:不切 phase, 仅 input 预填"修订方案:"前缀.
// phase 保持 Ready, 下一条 chat 触发的 Ready reminder 已包含"用户发新消息=隐式修订"语义.

/// 用户点 plan_card [🚪 算了]：放弃这个方案,但**留在当前模式**(Plan 不踢回 Yolo)。
/// "算了"= 这个方案不要了,不等于退出规划态;要换模式用户自己点 chip。
/// 与 accept_plan(切 Yolo 执行) / exit_plan_to_yolo(切 Yolo 直接干) 区别:discard 只关卡片、不动 mode。
#[tauri::command]
pub async fn discard_plan(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<SessionModeState, String> {
    // 不动 mode——放弃方案 ≠ 退出 Plan;仅回传当前状态供前端刷新卡片。
    Ok(store.mode_state(&session_id))
}

// ===================== request_user_input 工具气泡 =====================

/// 前端选择气泡点击后调用：把用户选择回传给 engine,解锁 await_user_input。
/// answers 数组里每项 { id, label, value } 对应底座 `UserInputAnswer`。
#[tauri::command]
pub async fn submit_user_input(
    tool_call_id: String,
    answers: Vec<UserInputAnswer>,
    session_id: Option<String>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    let response = UserInputResponse { answers };
    pool.submit_user_input(&sid, tool_call_id, response)
        .await
        .map_err(|e| format!("submit_user_input: {e:?}"))
}

/// [2026-06-06] 工作流素材上传：把用户选的文件拷进当前 run 的 配套材料/ 目录。
/// 前端素材收集卡片「📎 上传素材」按钮 → dialogOpen 选文件 → 调此命令落盘。
/// materials_auditor 重扫 配套材料/ 即可识别。返回实际落盘的文件名（含同名去重后的名）。
#[tauri::command]
pub async fn add_run_materials(
    session_id: Option<String>,
    paths: Vec<String>,
    store: State<'_, SessionStore>,
) -> Result<Vec<String>, String> {
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    let workspace = store
        .execution_workspace(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    let project = crate::harness::find_project_dir(&workspace)
        .ok_or_else(|| "当前 session 无工作流项目".to_string())?;
    let dst_dir = project.join("配套材料");
    std::fs::create_dir_all(&dst_dir).map_err(|e| format!("建配套材料目录失败: {e}"))?;
    let mut added = Vec::new();
    for p in &paths {
        let src = std::path::Path::new(p);
        let base = src
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("非法路径: {p}"))?;
        // 同名去重（参照 attach_file 的命名逻辑）
        let (stem, ext) = match base.rsplit_once('.') {
            Some((s, e)) => (s.to_string(), format!(".{e}")),
            None => (base.to_string(), String::new()),
        };
        let mut candidate = base.to_string();
        let mut n = 1;
        while dst_dir.join(&candidate).exists() {
            candidate = format!("{stem}-{n}{ext}");
            n += 1;
        }
        std::fs::copy(src, dst_dir.join(&candidate))
            .map_err(|e| format!("拷贝 {base} 失败: {e}"))?;
        added.push(candidate);
    }
    Ok(added)
}

/// 前端 ✕ 按钮 / 切换 session 时调用：取消 request_user_input。
/// engine 把工具结果置为 "User input cancelled" error,LLM 收到后会继续 turn。
#[tauri::command]
pub async fn cancel_user_input(
    tool_call_id: String,
    session_id: Option<String>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    pool.cancel_user_input(&sid, tool_call_id)
        .await
        .map_err(|e| format!("cancel_user_input: {e:?}"))
}

// (render_surface 回流 / cloud_keys 云模型配置是独立 feature,不在本 PR——
//  本 PR 只含工作流基座 + 三省六部)

#[tauri::command]
pub async fn restart_engine(
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    // 多 session 并发:重启 = evict 当前 active session 的 engine(取消在跑 turn +
    // Shutdown + abort forwarder),下次 chat 时 EnginePool 重新 spawn 干净的并从磁盘
    // rehydrate 历史。也是 engine-busy 卡死时的恢复路径。
    if let Some(sid) = store.active_id() {
        pool.evict(&sid).await;
    }
    Ok(())
}

// ===================== Pinvou v4 召唤式检阅 =====================

/// Boss 主动召唤 Pinvou 检阅当前 session 的工作（设计 `docs/品悟v4-常驻检阅助手设计.md`）。
/// 取该 session 全部 messages → 投影/全喂 → 单次独立 LLM 审查 → 返回 personas/issues。
/// 纯召唤、不替 Boss 决策；自动触发已彻底移除。
#[tauri::command]
pub async fn summon_pinvou(
    session_id: Option<String>,
    focus: Option<String>,
    mode: Option<String>,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<crate::pinvou_review::PinvouReview, String> {
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    let session = store
        .load(&sid)
        .map_err(|e| format!("summon_pinvou load({sid}): {e:?}"))?;
    let bridge = pool
        .fresh_bridge_for(&sid)
        .await
        .map_err(|e| format!("summon_pinvou prepare bridge({sid}): {e:#}"))?;
    let workspace = store
        .execution_workspace(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    crate::pinvou_review::summon(
        &bridge,
        &session.messages,
        &workspace,
        &sid,
        focus.as_deref(),
        mode.as_deref(),
    )
    .await
    .map_err(|e| format!("summon_pinvou: {e:?}"))
}

/// 路径校验：必须是绝对路径 + 路径解析后无 `..` 逃逸 + 不命中敏感清单。
///
/// pinvou3 是本地单用户工具，不像 web 服务有跨用户边界，所以不强制 $HOME
/// 限制（允许 AI 在 /tmp / /opt / /mnt 等用户授权位置产出文件）。仅黑名单
/// 拦截两类位置：(1) 用户凭据目录/文件，避免 AI 误把私钥/.env 内容读进
/// LLM context 传给外部 vLLM；(2) 系统级敏感文件如 /etc/shadow。
pub(crate) fn validate_user_path(raw: &str) -> Result<std::path::PathBuf, String> {
    use std::path::PathBuf;
    let p = PathBuf::from(raw);
    if !p.is_absolute() {
        return Err(format!("path must be absolute: {raw}"));
    }
    let canon = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
    let canon_str = canon.to_string_lossy();

    // 凭据/配置类组件名拦截（任意路径深度，命中目录名或文件名即拒绝）
    const BLOCKED_COMPONENTS: &[&str] = &[
        ".ssh",
        ".gnupg",
        ".aws",
        ".docker",
        ".kube",
        ".password-store",
        "id_rsa",
        "id_ed25519",
        "id_ecdsa",
        "id_dsa",
        "credentials.json",
        ".env",
    ];
    for blocked in BLOCKED_COMPONENTS {
        if canon
            .components()
            .any(|c| c.as_os_str() == std::ffi::OsStr::new(blocked))
        {
            return Err(format!(
                "path {} crosses sensitive component {}",
                canon.display(),
                blocked
            ));
        }
    }

    // 系统级敏感路径前缀拦截
    const BLOCKED_PREFIXES: &[&str] = &[
        "/etc/shadow",
        "/etc/gshadow",
        "/etc/sudoers",
        "/etc/ssh/",
        "/root/",
        "/var/log/auth",
        "/proc/",
        "/sys/",
    ];
    for prefix in BLOCKED_PREFIXES {
        if canon_str.starts_with(prefix) {
            return Err(format!(
                "path {} is in system-sensitive area",
                canon.display()
            ));
        }
    }

    Ok(canon)
}
