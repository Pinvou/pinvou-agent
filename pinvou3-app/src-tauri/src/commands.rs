//! Tauri 命令实现。前端通过 `invoke(name, args)` 调到这里。
//!
//! 暴露的命令：
//! - `chat(message)`         — 发送用户消息（流式响应通过 chat:* 事件）
//! - `get_settings()`        — 读 `~/.pinvou3/settings.json`（UserPrefs）
//! - `update_settings(prefs)`— 写盘；GUI 项立即生效，引擎相关项需重启 app
//! - `clear_session()`       — 清前端显示（MVP）；后端 session 重启 app 才真清
//! - `get_monitor_snapshot()`— Monitor 视图完整数据
//! - `get_backend_status()`  — ChatRoom 顶部 live dot 用，简版健康指示
//!
//! 阶段 C 新增（多对话历史）：
//! - `list_sessions()` / `create_session()` / `load_session(id)`
//! - `delete_session(id)` / `rename_session(id, title)` / `get_active_session()`

use deepseek_tui::models::Message;
use deepseek_tui::session_manager::{SavedSession, SessionMetadata};
use deepseek_tui::tools::user_input::{UserInputAnswer, UserInputResponse};
use serde::Serialize;
use tauri::State;

use crate::bridge::mode_state::{PlanPhase, SerializableMode, SessionModeState};
use crate::bridge::prefs::UserPrefs;
use crate::bridge::sessions::SessionStore;
use crate::engine::AppEngine;
use crate::monitor::{MonitorSnapshot, MonitorState, VllmStatus};

/// 接收用户消息并转发给 Engine。
/// 立即返回，LLM 流式输出通过 Tauri Event 异步推给前端。
///
/// `attachments` 是前端已经过 `ingest_file` 处理后的 IngestResult 数组。
/// 本函数把它们拼到 message 末尾，格式：
/// ```text
/// ---
/// 用户附上了以下文件：
///
/// ### {basename} ({kind}, ~{tokens} tokens)
/// {markdown 或 警告}
/// ---
/// ```
#[tauri::command]
pub async fn chat(
    message: String,
    attachments: Option<Vec<crate::file_ingest::IngestResult>>,
    engine: State<'_, AppEngine>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    let trimmed = message.trim();
    if trimmed.is_empty() && attachments.as_ref().map_or(true, |a| a.is_empty()) {
        return Err("empty message".to_string());
    }
    let full = build_message_with_attachments(message, attachments.unwrap_or_default());
    // 取当前 active session 的 mode + phase;无 active session 时默认 Yolo+None(首条消息场景)
    let (mode, phase) = store
        .active_id()
        .map(|id| {
            let s = store.mode_state(&id);
            (s.mode, s.plan_phase)
        })
        .unwrap_or((SerializableMode::Yolo, PlanPhase::None));
    // M2: 用户主动消息重置 auto-continue 计数器(新任务从 0 开始算 max 3 次)
    if let Some(id) = store.active_id() {
        store.reset_auto_continue(&id);
    }
    engine
        .inner()
        .send_user_message(full, mode.to_app_mode(), phase)
        .await
        .map_err(|e| format!("send_user_message failed: {e:?}"))
}

/// 拼接 user 文本 + 附件 markdown。
/// 图片走 model_no_vision 警告块，让 LLM 明确知道「有图但看不到内容」。
fn build_message_with_attachments(
    text: String,
    attachments: Vec<crate::file_ingest::IngestResult>,
) -> String {
    if attachments.is_empty() {
        return text;
    }
    let mut out = String::new();
    if !text.trim().is_empty() {
        out.push_str(&text);
        out.push_str("\n\n");
    }
    out.push_str(
        "---\n用户附上了以下文件。**文件完整内容已嵌入下方代码块,可直接使用,\
         不需要再调 read_file / file_search 重新读取。** 如需保存修改版本,用 \
         write_file 写到 PINVOU3_WORKSPACE 下。\n\n",
    );
    for a in &attachments {
        out.push_str(&format!(
            "### {} ({}, {} bytes",
            a.basename,
            a.kind,
            a.byte_size
        ));
        if a.token_estimate > 0 {
            out.push_str(&format!(", ~{} tokens", a.token_estimate));
        }
        out.push_str(")\n");
        // 真实路径 —— AI 如果一定要 read_file 也能找到对的位置，
        // 同时避免 AI 凭想象编造 workspace/<timestamp>-... 这种伪路径
        out.push_str(&format!("原始路径: `{}`\n", a.path));
        if a.kind == "image" {
            out.push_str(
                "⚠️ 当前模型 Qwen3.6 没有视觉能力,只知道用户附了这张图,**无法**分析像素内容。\
                请明确告诉用户你看不到,不要臆测图里的东西。\n",
            );
        } else if let Some(md) = &a.markdown {
            out.push_str("```\n");
            out.push_str(md);
            if !md.ends_with('\n') { out.push('\n'); }
            out.push_str("```\n");
        } else if let Some(warning) = &a.warning {
            out.push_str(&format!("⚠️ {warning}\n"));
        }
        out.push('\n');
    }
    out.push_str("---\n");
    out
}

/// 从 disk 读最新 UserPrefs。
/// 注意走 disk 而非 engine.bridge.prefs——如果用户手改 settings.json，
/// `get_settings()` 能立刻拿到，不需要 reload bridge。
#[tauri::command]
pub async fn get_settings() -> Result<UserPrefs, String> {
    Ok(UserPrefs::load())
}

/// 持久化 UserPrefs 到 `~/.pinvou3/settings.json`。
///
/// **当前 MVP 限制**：写盘后不重启 Engine。所以：
/// - GUI 视觉项（theme / color_scheme）：前端立即应用，不需要后端介入
/// - 语言切换：写盘成功，但 LLM 的 `locale_tag` 只在下次重启 app 时生效
/// - advanced 字段：同上，重启 app 后生效
///
/// Phase C 会做 in-place engine restart（处理 in-flight turn）。
#[tauri::command]
pub async fn update_settings(prefs: UserPrefs) -> Result<(), String> {
    prefs.save().map_err(|e| format!("save settings failed: {e:?}"))
}

/// 清当前会话历史。
///
/// **当前 MVP 限制**：仅返回 Ok 让前端清显示；后端 EngineHandle 仍持
/// 累积的消息历史，下次 chat 时 LLM 仍能看到之前的对话。真清需要重启
/// app（spawn 全新 Engine）。
///
/// 实装路径（Phase C）：发 `Op::Shutdown` 给 engine + 在 Tauri State 上
/// 替换 AppEngine 为新 spawn 出来的实例。
#[tauri::command]
pub async fn clear_session() -> Result<(), String> {
    eprintln!("[pinvou3-app] clear_session: frontend cleared, backend session unchanged (MVP)");
    Ok(())
}

/// Monitor 视图完整数据。前端每 5s 拉一次。
#[tauri::command]
pub async fn get_monitor_snapshot(
    monitor: State<'_, MonitorState>,
) -> Result<MonitorSnapshot, String> {
    Ok(monitor.snapshot().await)
}

/// ChatRoom 顶部 live dot 简版指示：vLLM 是否在线。
#[derive(Debug, Clone, Serialize)]
pub struct BackendStatus {
    pub vllm_online: bool,
    pub last_check_ms: u64,
}

#[tauri::command]
pub async fn get_backend_status(
    monitor: State<'_, MonitorState>,
) -> Result<BackendStatus, String> {
    let snap = monitor.snapshot().await;
    let vllm_online = matches!(
        snap.vllm.as_ref().map(|v| v.status),
        Some(VllmStatus::Ready) | Some(VllmStatus::Busy)
    );
    Ok(BackendStatus {
        vllm_online,
        last_check_ms: snap.generated_at_ms,
    })
}

// ===================== 阶段 C: 多对话历史 =====================

/// 列出所有 session 元数据，按 updated_at 倒序。前端历史面板渲染用。
/// 返回 SessionMetadata 数组（id/title/时间/token/model/workspace 等字段）。
#[tauri::command]
pub async fn list_sessions(
    store: State<'_, SessionStore>,
) -> Result<Vec<SessionMetadata>, String> {
    store.list().map_err(|e| format!("list_sessions: {e:?}"))
}

/// 新建空 session 并设为 active。返回创建的 SessionMetadata。
/// 引擎层的 session 状态切换由 chat() 下次发消息时自然处理（暂不发 SyncSession）。
#[tauri::command]
pub async fn create_session(
    store: State<'_, SessionStore>,
    engine: State<'_, AppEngine>,
) -> Result<SessionMetadata, String> {
    let model = engine.inner().bridge.model();
    let workspace = engine.inner().bridge.workspace.clone();
    let session = store
        .create_new(model, workspace)
        .map_err(|e| format!("create_session: {e:?}"))?;
    store.set_active(Some(session.metadata.id.clone()));
    // 清空 engine 内部 session，否则新对话会接在旧上下文后面
    engine
        .inner()
        .sync_session(session.metadata.id.clone(), Vec::new())
        .await
        .map_err(|e| format!("sync engine session: {e:?}"))?;
    Ok(session.metadata)
}

/// 加载指定 session 的完整对话（含 messages）。
/// 前端切换历史时调用 → 用返回的 messages 重渲染对话区。
#[tauri::command]
pub async fn load_session(
    id: String,
    store: State<'_, SessionStore>,
    engine: State<'_, AppEngine>,
) -> Result<SavedSession, String> {
    let session = store
        .load(&id)
        .map_err(|e| format!("load_session({id}): {e:?}"))?;
    store.set_active(Some(id.clone()));
    // 把 engine 的内部 session 状态替换成这个 session 的 messages，
    // 否则 engine 仍持有旧 session 的 messages,下次发消息会续在旧上下文后,
    // 造成 session 间「串台」(bug fix 2026-05-13)
    engine
        .inner()
        .sync_session(id, session.messages.clone())
        .await
        .map_err(|e| format!("sync engine session: {e:?}"))?;
    Ok(session)
}

/// 删除 session（含 artifacts 目录）。
#[tauri::command]
pub async fn delete_session(
    id: String,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    store
        .delete(&id)
        .map_err(|e| format!("delete_session({id}): {e:?}"))
}

/// 重命名 session 标题。
#[tauri::command]
pub async fn rename_session(
    id: String,
    title: String,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    store
        .set_title(&id, title)
        .map_err(|e| format!("rename_session({id}): {e:?}"))
}

/// 取当前 active session id（前端启动时高亮历史面板用）。
#[tauri::command]
pub async fn get_active_session(
    store: State<'_, SessionStore>,
) -> Result<Option<String>, String> {
    Ok(store.active_id())
}

/// 落盘 session 的 messages 数组。前端每轮 TurnComplete 后调用,
/// 把累积的对话历史同步到后端。前端是 messages 的 source of truth。
#[tauri::command]
pub async fn save_session_messages(
    id: String,
    messages: Vec<Message>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    store
        .update_messages(&id, messages)
        .map_err(|e| format!("save_session_messages({id}): {e:?}"))
}

/// 落盘 session 的产物 paths 列表。前端跟踪 write_file 调用后调用,
/// 跟 save_session_messages 一起落 (TurnComplete 时)。重启/切换 session 后,
/// 从 SavedSession.artifacts 重建前端产物列表。
#[tauri::command]
pub async fn save_session_artifacts(
    id: String,
    paths: Vec<String>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    store
        .update_artifacts(&id, paths)
        .map_err(|e| format!("save_session_artifacts({id}): {e:?}"))
}

// ===================== 阶段 C: 取消生成 + 编辑/重发 =====================

/// 取消当前生成（生成中按⏹️停止按钮）。
/// engine 立即 cancel_token.cancel()，turn loop 跳出后会发 TurnComplete 事件，
/// 前端通过 chat:done 解锁 busy 状态。
#[tauri::command]
pub async fn cancel_generation(engine: State<'_, AppEngine>) -> Result<(), String> {
    engine.inner().cancel_current();
    Ok(())
}

/// 编辑/重发最后一轮 user 消息。
/// engine 砍掉 session 末尾最近的 user+assistant 后，用 new_message 重发。
/// 前端在调这个命令之前必须自己更新 state.messages（删最后一对，加新 user）。
#[tauri::command]
pub async fn edit_last_turn(
    new_message: String,
    engine: State<'_, AppEngine>,
) -> Result<(), String> {
    if new_message.trim().is_empty() {
        return Err("empty new_message".into());
    }
    engine
        .inner()
        .edit_last_turn(new_message)
        .await
        .map_err(|e| format!("edit_last_turn: {e:?}"))
}

// ===================== 阶段 C: 产物面板 =====================

/// 产物文件元数据。前端右栏 list 用。
#[derive(Debug, Clone, Serialize)]
pub struct ArtifactInfo {
    /// 文件大小（字节）
    pub size: u64,
    /// 文件 mime-ish 分类：md / html / image / pdf / text / binary
    pub kind: String,
    /// 文件存在标记（前端跟踪的路径可能被外部删了）
    pub exists: bool,
}

/// 读 artifact 文件的纯文本（md/json/txt 等）。文件不存在或不是文本 → 报错。
/// 路径必须在用户家目录下（防 ../../../etc/passwd 之类逃逸）。
#[tauri::command]
pub async fn read_artifact_text(path: String) -> Result<String, String> {
    let p = validate_user_path(&path)?;
    std::fs::read_to_string(&p)
        .map_err(|e| format!("read_artifact_text({}): {e}", p.display()))
}

/// 读 artifact 元数据：大小 / 类型 / 是否存在。
#[tauri::command]
pub async fn artifact_info(path: String) -> Result<ArtifactInfo, String> {
    let p = match validate_user_path(&path) {
        Ok(p) => p,
        Err(_) => {
            return Ok(ArtifactInfo {
                size: 0,
                kind: "denied".into(),
                exists: false,
            })
        }
    };
    let meta = match std::fs::metadata(&p) {
        Ok(m) => m,
        Err(_) => {
            return Ok(ArtifactInfo {
                size: 0,
                kind: "missing".into(),
                exists: false,
            })
        }
    };
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let kind = match ext.as_str() {
        "md" | "markdown" => "md",
        "html" | "htm" => "html",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" => "image",
        "pdf" => "pdf",
        // 让前端能识别 office 格式 → 调 ingest_file 转 md 内嵌预览
        "docx" | "pptx" | "odt" => "docx",
        "xlsx" | "ods" => "xlsx",
        "doc" | "ppt" | "xls" | "rtf" => "legacy_office",
        "txt" | "log" | "csv" | "json" | "yaml" | "yml" | "toml" | "xml"
        | "rs" | "py" | "js" | "ts" | "go" | "c" | "cpp" | "h" | "hpp" | "sh" => "text",
        _ => "binary",
    };
    Ok(ArtifactInfo {
        size: meta.len(),
        kind: kind.into(),
        exists: true,
    })
}

/// 用系统默认应用打开文件（xdg-open / 文件管理器）。
#[tauri::command]
pub async fn open_in_system(path: String) -> Result<(), String> {
    let p = validate_user_path(&path)?;
    std::process::Command::new("xdg-open")
        .arg(&p)
        .spawn()
        .map_err(|e| format!("xdg-open({}) failed: {e}", p.display()))?;
    Ok(())
}

/// 用文件管理器打开**所在目录**（不是文件本身）。xdg-open 一个目录路径
/// → Ubuntu 走 Nautilus / Files；跨发行版（GNOME/KDE/XFCE）freedesktop 标准兼容。
#[tauri::command]
pub async fn open_containing_folder(path: String) -> Result<(), String> {
    let p = validate_user_path(&path)?;
    let dir = p
        .parent()
        .ok_or_else(|| format!("no parent dir for {}", p.display()))?;
    std::process::Command::new("xdg-open")
        .arg(dir)
        .spawn()
        .map_err(|e| format!("xdg-open({}) failed: {e}", dir.display()))?;
    Ok(())
}

/// 在 Tauri 新窗口里加载 HTML 产物。绕过 snap 浏览器对 `~/.xxx/` 隐藏目录的沙箱限制。
/// 同一文件再次调用 → focus 已有窗口而非新建,防窗口爆炸。
#[tauri::command]
pub async fn open_artifact_window(
    path: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder, Manager};

    let p = validate_user_path(&path)?;
    if !p.is_file() {
        return Err(format!("not a file: {}", p.display()));
    }
    // 用文件 inode 做稳定 label,防同一文件多次打开建多窗口。Tauri label 只允许 a-zA-Z0-9-_。
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    p.to_string_lossy().hash(&mut hasher);
    let label = format!("artifact-{:x}", hasher.finish());

    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return Ok(());
    }

    let url_str = format!("file://{}", p.display());
    let url = url_str.parse().map_err(|e| format!("parse file url: {e}"))?;
    let title = p
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("产物")
        .to_string();

    WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(url))
        .title(title)
        .inner_size(1024.0, 768.0)
        .resizable(true)
        .build()
        .map_err(|e| format!("build artifact window: {e}"))?;
    Ok(())
}

// ===================== 阶段 C: 输入文件上传 =====================

/// 把一个用户上传的文件转成 markdown（或标记不支持），返回 IngestResult。
/// 前端在 chip 行展示 token 估算 / 警告，发送时拼接 markdown 到 user message。
#[tauri::command]
pub async fn ingest_file(path: String) -> Result<crate::file_ingest::IngestResult, String> {
    let p = crate::file_ingest::validate_path(&path)?;
    Ok(crate::file_ingest::ingest(&p))
}

/// 返回系统工具检测结果（pandoc / pdftotext 是否可用）。
/// 前端启动时调一次，缺工具时给一次性 toast 引导 apt install。
#[tauri::command]
pub async fn detect_system_tools() -> Result<crate::file_ingest::SystemTools, String> {
    Ok(crate::file_ingest::system_tools())
}

/// 把剪贴板粘贴的图片 bytes 落盘到 `~/.pinvou3/pastes/<ts>-<name>` → 返回路径，
/// 前端拿到 path 后再 invoke `ingest_file`。
/// 只用于粘贴图片场景；选文件 / 拖拽走 Tauri native dialog 直接拿原 path。
#[tauri::command]
pub async fn save_paste_image(filename: String, bytes: Vec<u8>) -> Result<String, String> {
    let path = crate::file_ingest::save_paste_image(&filename, &bytes)?;
    Ok(path.to_string_lossy().to_string())
}

/// 手动触发上下文压缩。用户点 token 进度条 → 立即压缩当前对话历史。
/// 触发后 engine 会发 CompactionStarted / Completed / Failed 事件，
/// 通过 chat:compaction 系列 event 通知前端。
#[tauri::command]
pub async fn compact_now(engine: State<'_, AppEngine>) -> Result<(), String> {
    engine
        .inner()
        .compact_now()
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

/// 用户点 💡 进入 Plan 流程：设 mode=Plan + phase=Planning。
/// 下一条 chat 消息会带 mode=Plan 发送，底座自动切只读工具集 + ReadOnly sandbox。
#[tauri::command]
pub async fn set_plan_mode_next(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<SessionModeState, String> {
    store.set_mode_state(&session_id, SerializableMode::Plan, PlanPhase::Planning);
    Ok(store.mode_state(&session_id))
}

/// 用户点 [⚡ 直接动手]（Planning 态 chip 退出按钮）：跳过 plan 流程，凭对话历史自由干。
/// mode 切回 Yolo + phase=None。对话历史天然保留，AI 在 YOLO 下能看到之前讨论的 context。
#[tauri::command]
pub async fn exit_plan_to_yolo(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<SessionModeState, String> {
    store.set_mode_state(&session_id, SerializableMode::Yolo, PlanPhase::None);
    Ok(store.mode_state(&session_id))
}

/// 用户点 plan_card [✅ 就这么干]：接受 plan，切 YOLO 执行。
/// 流程：
///   1. 设 mode=Yolo, phase=Executing
///   2. 用 plan_markdown 作为指令前缀发一条 user message 触发执行
/// 前端在调用前应在消息流追加 user 气泡显示「✅ 就这么干」让用户感知。
#[tauri::command]
pub async fn accept_plan(
    session_id: String,
    plan_markdown: String,
    store: State<'_, SessionStore>,
    engine: State<'_, AppEngine>,
) -> Result<SessionModeState, String> {
    store.set_mode_state(&session_id, SerializableMode::Yolo, PlanPhase::Executing);
    // 简短指令——主约束由 M1 per-turn system-reminder 提供(bridge 按 phase=Executing 注入)。
    let instruction = format!(
        "用户已批准方案,立即开始执行。方案:\n\n{plan_markdown}"
    );
    engine
        .inner()
        .send_user_message(
            instruction,
            SerializableMode::Yolo.to_app_mode(),
            PlanPhase::Executing,
        )
        .await
        .map_err(|e| format!("accept_plan send_user_message: {e:?}"))?;
    Ok(store.mode_state(&session_id))
}

// 修法 D 删除了 revise_plan 命令.
// 用户点 [✏️ 改改] 时前端走 DeepSeek-TUI 底座做法:不切 phase, 仅 input 预填"修订方案:"前缀.
// phase 保持 Ready, 下一条 chat 触发的 Ready reminder 已包含"用户发新消息=隐式修订"语义.

/// 用户点 plan_card [🚪 算了]：放弃整个任务，回 YOLO 默认态。
/// 与 exit_plan_to_yolo 区别：⚡ 是「不要 plan 直接干」，🚪 是「这事不干了」。
#[tauri::command]
pub async fn discard_plan(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<SessionModeState, String> {
    store.set_mode_state(&session_id, SerializableMode::Yolo, PlanPhase::None);
    Ok(store.mode_state(&session_id))
}

// ===================== request_user_input 工具气泡 =====================

/// 前端选择气泡点击后调用：把用户选择回传给 engine,解锁 await_user_input。
/// answers 数组里每项 { id, label, value } 对应底座 `UserInputAnswer`。
#[tauri::command]
pub async fn submit_user_input(
    tool_call_id: String,
    answers: Vec<UserInputAnswer>,
    engine: State<'_, AppEngine>,
) -> Result<(), String> {
    let response = UserInputResponse { answers };
    engine
        .inner()
        .submit_user_input(tool_call_id, response)
        .await
        .map_err(|e| format!("submit_user_input: {e:?}"))
}

/// 前端 ✕ 按钮 / 切换 session 时调用：取消 request_user_input。
/// engine 把工具结果置为 "User input cancelled" error,LLM 收到后会继续 turn。
#[tauri::command]
pub async fn cancel_user_input(
    tool_call_id: String,
    engine: State<'_, AppEngine>,
) -> Result<(), String> {
    engine
        .inner()
        .cancel_user_input(tool_call_id)
        .await
        .map_err(|e| format!("cancel_user_input: {e:?}"))
}

/// 路径校验：必须是绝对路径 + 路径解析后无 `..` 逃逸 + 不命中敏感清单。
///
/// pinvou3 是本地单用户工具，不像 web 服务有跨用户边界，所以不强制 $HOME
/// 限制（允许 AI 在 /tmp / /opt / /mnt 等用户授权位置产出文件）。仅黑名单
/// 拦截两类位置：(1) 用户凭据目录/文件，避免 AI 误把私钥/.env 内容读进
/// LLM context 传给外部 vLLM；(2) 系统级敏感文件如 /etc/shadow。
fn validate_user_path(raw: &str) -> Result<std::path::PathBuf, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// L2-1: A 方案放宽 — /tmp 下任意文件可校验通过（不强 $HOME 限制）。
    #[test]
    fn validate_user_path_allows_tmp() {
        let tmp = std::env::temp_dir().join("pinvou3-validate-test");
        std::fs::create_dir_all(&tmp).unwrap();
        let p = tmp.join("foo.txt");
        std::fs::write(&p, "").unwrap();
        let result = validate_user_path(p.to_str().unwrap());
        std::fs::remove_dir_all(&tmp).ok();
        assert!(result.is_ok(), "/tmp 下普通文件应该通过, got {result:?}");
    }

    /// L2-2: 凭据组件名拦截 — 任何路径深度命中 .ssh/id_rsa 即拒绝。
    #[test]
    fn validate_user_path_blocks_ssh() {
        // 不需要文件真实存在，canonicalize 失败时退回 raw path 继续校验
        let p = "/home/anyuser/.ssh/id_rsa";
        let result = validate_user_path(p);
        assert!(result.is_err(), "凭据路径必须拒绝, got {result:?}");
        let err = result.unwrap_err();
        assert!(
            err.contains(".ssh") || err.contains("id_rsa"),
            "错误信息应指明命中的组件, got {err}"
        );
    }

    /// L2-3: 系统级敏感前缀拦截 — /etc/shadow 等被列在 BLOCKED_PREFIXES。
    #[test]
    fn validate_user_path_blocks_etc_shadow() {
        let result = validate_user_path("/etc/shadow");
        assert!(result.is_err(), "/etc/shadow 必须拒绝, got {result:?}");
        assert!(result.unwrap_err().contains("system-sensitive"));
    }
}
