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
use serde::Serialize;
use tauri::State;

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
) -> Result<(), String> {
    let trimmed = message.trim();
    if trimmed.is_empty() && attachments.as_ref().map_or(true, |a| a.is_empty()) {
        return Err("empty message".to_string());
    }
    let full = build_message_with_attachments(message, attachments.unwrap_or_default());
    engine
        .inner()
        .send_user_message(full)
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

/// 路径校验：必须是绝对路径 + 落在用户家目录下 + 路径解析后无 `..` 逃逸。
/// 防止前端拿伪造路径让后端读 /etc/shadow 或 ~/.ssh 之类。
fn validate_user_path(raw: &str) -> Result<std::path::PathBuf, String> {
    use std::path::PathBuf;
    let p = PathBuf::from(raw);
    if !p.is_absolute() {
        return Err(format!("path must be absolute: {raw}"));
    }
    let canon = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => return Err("HOME not set".into()),
    };
    if !canon.starts_with(&home) {
        return Err(format!("path {} not under $HOME", canon.display()));
    }
    // 敏感子目录拦截（跟 instructions.md 的软引导一致）
    for blocked in &[".ssh", ".gnupg", ".aws", ".docker", ".kube"] {
        if canon
            .components()
            .any(|c| c.as_os_str() == std::ffi::OsStr::new(blocked))
        {
            return Err(format!("path {} crosses sensitive dir {}", canon.display(), blocked));
        }
    }
    Ok(canon)
}
