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
use crate::bridge::review_gate::{check_exit_gate, GateError};
use crate::bridge::sessions::SessionStore;
use crate::engine_pool::EnginePool;
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
    session_id: Option<String>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    let trimmed = message.trim();
    if trimmed.is_empty() && attachments.as_ref().map_or(true, |a| a.is_empty()) {
        return Err("empty message".to_string());
    }
    // 多 session 并发:消息显式路由到指定 session(前端传 session_id);兼容旧前端时
    // 回退到全局 active_id。每条消息按各自 session 取 mode/phase/skill,送到对应 engine。
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    let mut full = build_message_with_attachments(message, attachments.unwrap_or_default());
    // 工作流 Phase 可视化:用户在工作流页"启用"卡片 = start_skill_session
    // 新建一个绑定了 skill 的 session。该 session 第一条 chat 消息时,
    // 把 skill body + phase 规则 prepend 一次,后续 turn 靠 LLM session 上下文保持。
    //
    // 另外 — 实测 Qwen3.6 在长上下文里对 system prompt 顶端的 phase marker
    // MANDATORY 段遵循率衰减(h3c-ppt 跑到 p5+ 后频繁不出 `<phase id=".."/>`
    // marker),每个 user turn 都重申一遍约束,把信号搬到距 LLM 最近的位置。
    if let Some(injected) = store.take_pending_skill_instruction(&sid) {
        full = format!("{injected}\n\n---\n\n{full}");
    }
    if let Some(skill) = store.active_skill(&sid) {
        let reminder = format!(
            "<system-reminder>\n\
             工作流 `{name}` 提醒:你的下一条回复**必须**以 `<phase id=\"...\"/>` \
             单独一行开头(literal XML 标签,不是 markdown 加粗,不是自然语言)。\n\
             - 自然语言里写 \"进入 P5\" / \"现在做 HTML 实现\" **不算** marker,\
             必须输出字面 `<phase id=\"p5\"/>` 然后空行然后正文。\n\
             - 跳过 phase 也要显式输出对应 marker(从 p3 直接做 p5 → 输出 \
             `<phase id=\"p5\"/>`,别不出标)。\n\
             - 留在同一 phase 多个 turn 也要每次都输出该 phase 的 marker,\
             不要因为\"没换 phase\"省略。\n\
             pinvou3 UI chips 条靠这个 marker 推进度,漏标 = 用户看不到进展。\n\
             </system-reminder>",
            name = skill.name,
        );
        full = format!("{reminder}\n\n{full}");
    }
    // 取该 session 的 mode + phase。
    let s = store.mode_state(&sid);
    let (mode, phase) = (s.mode, s.plan_phase);
    // M2: 用户主动消息重置 auto-continue 计数器(新任务从 0 开始算 max 3 次)
    store.reset_auto_continue(&sid);
    pool.send_user_message(&sid, full, mode.to_app_mode(), phase)
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
         write_file 写到 PINVOU3_WORKSPACE 下;大产物用 append_file 分块追加。\n\n",
    );
    for a in &attachments {
        out.push_str(&format!(
            "### {} ({}, {} bytes",
            a.basename, a.kind, a.byte_size
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
            if !md.ends_with('\n') {
                out.push('\n');
            }
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
    prefs
        .save()
        .map_err(|e| format!("save settings failed: {e:?}"))
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

/// Monitor 视图完整数据。**按需采样**——前端只在监控页面 mount 时启 1s
/// interval 调本 command，每次都重新跑 sample_all。GPU util 瞬时易错过推理
/// 峰，前端维护 5 个值滑窗 max 弥补。
#[tauri::command]
pub async fn get_monitor_snapshot(
    monitor: State<'_, MonitorState>,
) -> Result<MonitorSnapshot, String> {
    Ok(crate::monitor::sample_all(&monitor, &crate::monitor::vllm_base_url()).await)
}

/// ChatRoom 顶部 live dot 简版指示：vLLM 是否在线。
#[derive(Debug, Clone, Serialize)]
pub struct BackendStatus {
    pub vllm_online: bool,
    pub last_check_ms: u64,
}

#[tauri::command]
pub async fn get_backend_status(_monitor: State<'_, MonitorState>) -> Result<BackendStatus, String> {
    // Lightweight: 只 probe vLLM,不跑 nvidia-smi / RAM 采样
    let vllm = crate::monitor::vllm_snapshot(&crate::monitor::vllm_base_url()).await;
    let vllm_online = matches!(
        vllm.as_ref().map(|v| v.status),
        Some(VllmStatus::Ready) | Some(VllmStatus::Busy)
    );
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    Ok(BackendStatus {
        vllm_online,
        last_check_ms: now_ms,
    })
}

// ===================== 阶段 C: 多对话历史 =====================

/// 列出所有 session 元数据，按 updated_at 倒序。前端历史面板渲染用。
/// 返回 SessionMetadata 数组（id/title/时间/token/model/workspace 等字段）。
#[tauri::command]
pub async fn list_sessions(store: State<'_, SessionStore>) -> Result<Vec<SessionMetadata>, String> {
    store.list().map_err(|e| format!("list_sessions: {e:?}"))
}

/// 新建空 session 并设为 active。返回创建的 SessionMetadata。
/// 引擎层的 session 状态切换由 chat() 下次发消息时自然处理（暂不发 SyncSession）。
#[tauri::command]
pub async fn create_session(
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<SessionMetadata, String> {
    let model = pool.bridge.model();
    let workspace = pool.bridge.workspace.clone();
    let session = store
        .create_new(model, workspace)
        .map_err(|e| format!("create_session: {e:?}"))?;
    store.set_active(Some(session.metadata.id.clone()));
    // 多 session 并发:不预热 engine(lazy)。新建的空 session 没有历史,首条 chat
    // 时 EnginePool.get_or_spawn 会为它 spawn 一个带专属 workspace 的 engine。
    Ok(session.metadata)
}

/// 加载指定 session 的完整对话（含 messages）。
/// 前端切换历史时调用 → 用返回的 messages 重渲染对话区。
#[tauri::command]
pub async fn load_session(
    id: String,
    store: State<'_, SessionStore>,
) -> Result<SavedSession, String> {
    let session = store
        .load(&id)
        .map_err(|e| format!("load_session({id}): {e:?}"))?;
    store.set_active(Some(id.clone()));
    // 多 session 并发:切换不再 SyncSession 替换全局引擎(那是旧单引擎模型)。该 session
    // 有自己独立的 engine(已起则持有自己的上下文、还在跑就继续跑;未起则下次 chat 时
    // lazy spawn 并注水这里返回的 messages)。本命令只切 active 指针 + 返回 messages 给前端渲染。
    Ok(session)
}

/// 删除 session（含 artifacts 目录）。
#[tauri::command]
pub async fn delete_session(
    id: String,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    // 先回收该 session 的 engine(cancel 在跑的 turn + shutdown + abort forwarder),
    // 再删盘上数据,避免僵尸 engine 继续往已删 session 写产物。
    pool.evict(&id).await;
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
pub async fn get_active_session(store: State<'_, SessionStore>) -> Result<Option<String>, String> {
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

/// 落盘 session 的产物 paths 列表。前端跟踪 write_file / append_file 调用后调用,
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
pub async fn cancel_generation(
    session_id: Option<String>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    // 多 session:取消指定 session(前端传 session_id);兼容旧前端回退 active。
    if let Some(sid) = session_id.or_else(|| store.active_id()) {
        pool.cancel(&sid).await;
    }
    Ok(())
}

/// 编辑/重发最后一轮 user 消息。
/// engine 砍掉 session 末尾最近的 user+assistant 后，用 new_message 重发。
/// 前端在调这个命令之前必须自己更新 state.messages（删最后一对，加新 user）。
#[tauri::command]
pub async fn edit_last_turn(
    new_message: String,
    session_id: Option<String>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    if new_message.trim().is_empty() {
        return Err("empty new_message".into());
    }
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    pool.edit_last_turn(&sid, new_message)
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
    std::fs::read_to_string(&p).map_err(|e| format!("read_artifact_text({}): {e}", p.display()))
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
        "txt" | "log" | "csv" | "json" | "yaml" | "yml" | "toml" | "xml" | "rs" | "py" | "js"
        | "ts" | "go" | "c" | "cpp" | "h" | "hpp" | "sh" => "text",
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
pub async fn open_artifact_window(path: String, app: tauri::AppHandle) -> Result<(), String> {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

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
    let url = url_str
        .parse()
        .map_err(|e| format!("parse file url: {e}"))?;
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
///
/// **Pinvou Review GATE**: `pinvou_review_enabled = true` 时,plan 期切 YOLO 也需要 Pinvou
/// 看过 plan(防绕过)。前端可传入 plan_markdown=None 表示"我承认还没 plan,纯空手切 YOLO"
/// (这种情况不触发 GATE)。
#[tauri::command]
pub async fn exit_plan_to_yolo(
    session_id: String,
    plan_markdown: Option<String>,
    store: State<'_, SessionStore>,
) -> Result<SessionModeState, String> {
    let enabled = store.mode_state(&session_id).pinvou_review_enabled;
    if enabled {
        if let Some(md) = plan_markdown.as_deref() {
            if !md.trim().is_empty() {
                check_exit_gate(md).map_err(|e| serialize_gate_error(&e))?;
            }
        }
    }
    store.set_mode_state(&session_id, SerializableMode::Yolo, PlanPhase::None);
    Ok(store.mode_state(&session_id))
}

/// 用户点 plan_card [✅ 就这么干]：接受 plan，切 YOLO 执行。
/// 流程：
///   1. **Pinvou Review GATE** (仅 pinvou_review_enabled = true):校验 plan_markdown
///      末尾的 ## PINVOU REVIEW REPORT 表格,CRITICAL 全部 RESOLVED/OVERRIDDEN_BY_USER。
///      失败时返回结构化错误 JSON,前端据此自动追加 /pinvou-review-plan 或提示用户。
///   2. 设 mode=Yolo, phase=Executing
///   3. 用 plan_markdown 作为指令前缀发一条 user message 触发执行
/// 前端在调用前应在消息流追加 user 气泡显示「✅ 就这么干」让用户感知。
#[tauri::command]
pub async fn accept_plan(
    session_id: String,
    plan_markdown: String,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<SessionModeState, String> {
    let enabled = store.mode_state(&session_id).pinvou_review_enabled;
    if enabled {
        check_exit_gate(&plan_markdown).map_err(|e| serialize_gate_error(&e))?;
    }
    store.set_mode_state(&session_id, SerializableMode::Yolo, PlanPhase::Executing);
    // 简短指令——主约束由 M1 per-turn system-reminder 提供(bridge 按 phase=Executing 注入)。
    let instruction = format!("用户已批准方案,立即开始执行。方案:\n\n{plan_markdown}");
    pool.send_user_message(
        &session_id,
        instruction,
        SerializableMode::Yolo.to_app_mode(),
        PlanPhase::Executing,
    )
    .await
    .map_err(|e| format!("accept_plan send_user_message: {e:?}"))?;
    Ok(store.mode_state(&session_id))
}

/// 把 GateError 序列化成 JSON 字符串,前端 parseGateError() 解析 {gate_error, message, detail}。
fn serialize_gate_error(err: &GateError) -> String {
    let kind = match err {
        GateError::MissingReviewReport => "missing_review_report",
        GateError::MalformedReport { .. } => "malformed_report",
        GateError::UnresolvedCritical { .. } => "unresolved_critical",
    };
    serde_json::to_string(&serde_json::json!({
        "gate_error": kind,
        "message": err.to_string(),
        "detail": err,
    }))
    .unwrap_or_else(|_| err.to_string())
}

/// 用户切换品悟 review 开关(UI 顶部 toggle)。
/// 与 Plan/YOLO 切换正交:品悟 toggle 不动 mode/phase,只控是否触发 EXIT GATE + 品悟气泡。
/// careful hook 不依赖此开关,跨所有模式默认开启(DeepSeek-TUI shell.rs 强制 BLOCKED Dangerous)。
#[tauri::command]
pub async fn set_pinvou_review(
    session_id: String,
    enabled: bool,
    store: State<'_, SessionStore>,
) -> Result<SessionModeState, String> {
    store.set_pinvou_review(&session_id, enabled);
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
    let safe_name: String = name.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect();
    if safe_name != name || safe_name.is_empty() {
        return Err(format!("invalid skill name: {name}"));
    }
    let path = paths::bundle_skills_dir().join(&safe_name).join("SKILL.md");
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

// ===================== 工作流 Phase 可视化 MVP1 =====================
// list_skills_v2 / read_skill_demo / start_skill_session / unbind_session_skill
// 四件套支撑「工作流」视图 + skill per-session 绑定。设计与边界见
// `/home/hexin/.claude/plans/workflow-phase-elegant-zephyr.md`。

/// 工作流卡片不显示的 skill 名单。这些是 pinvou3 自带的基础能力组件
/// (review 流程内部用),不应作为用户主动启用的工作流入口。
///
/// 后续真正物理隔离会把这俩从 `bundle/skills/` 移到独立目录 +
/// DeepSeek-TUI fork patch 让 EngineConfig 支持多 skills_dir。当前
/// 用 skiplist 软隔离,工作量小,效果一致。
const WORKFLOW_HIDDEN_SKILLS: &[&str] = &["pinvou-review-plan", "pinvou-review-final"];

/// 工作流视图卡片渲染需要的 skill 摘要 — 跟 DeepSeek-TUI runtime_api 的
/// `SkillEntry` 不同,这里额外把 phases / demo 元数据序列化给前端 (底座
/// 没把这俩字段暴露到 REST,所以 pinvou3-app 自己读 SkillRegistry 拼)。
#[derive(Debug, Serialize)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    /// 永远是 "bundle"(只扫 bundle/skills 单源)— 字段保留是为了前端
    /// 卡片角标 / 跟未来多源场景兼容。
    pub source: &'static str,
    pub phases: Vec<deepseek_tui::skills::PhaseDef>,
    pub demo: DemoSummary,
}

#[derive(Debug, Serialize)]
pub struct DemoSummary {
    pub has_file: bool,
    pub has_preview: bool,
    pub description: Option<String>,
    pub duration: Option<String>,
}

impl From<&deepseek_tui::skills::DemoInfo> for DemoSummary {
    fn from(d: &deepseek_tui::skills::DemoInfo) -> Self {
        Self {
            has_file: d.file.is_some(),
            has_preview: d.preview.is_some(),
            description: d.description.clone(),
            duration: d.duration_estimate.clone(),
        }
    }
}

/// 列出 `~/.pinvou3/bundle/skills/` 下的所有用户业务 skill。
/// pinvou-review-* 这种系统基础能力通过 `WORKFLOW_HIDDEN_SKILLS` 过滤掉
/// (它们不应出现在工作流卡片入口里)。
#[tauri::command]
pub async fn list_skills_v2() -> Result<Vec<SkillSummary>, String> {
    use crate::bridge::paths;
    use deepseek_tui::skills::SkillRegistry;

    let dir = paths::bundle_skills_dir();
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
            phases: s.phases.clone(),
            demo: (&s.demo).into(),
        })
        .collect();
    // 有 phases 的排前面
    out.sort_by(|a, b| b.phases.len().cmp(&a.phases.len()).then(a.name.cmp(&b.name)));
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
    use crate::bridge::paths;
    use deepseek_tui::skills::SkillRegistry;

    if WORKFLOW_HIDDEN_SKILLS.contains(&name.as_str()) {
        return Err(format!("skill {name} 是系统基础能力,无 demo 入口"));
    }

    let dir = paths::bundle_skills_dir();
    if !dir.exists() {
        return Err(format!("skills dir not found: {}", dir.display()));
    }
    let registry = SkillRegistry::discover(&dir);
    let skill = registry
        .get(&name)
        .ok_or_else(|| format!("skill not found: {name}"))?
        .clone();

    let file_path = skill.demo.file.clone();
    let preview_path = skill.demo.preview.as_ref().map(|p| p.to_string_lossy().to_string());

    let (file_kind, content) = match file_path.as_ref() {
        None => ("none", None),
        Some(p) => {
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default();
            match ext.as_str() {
                "html" | "htm" => ("html", None),
                "png" | "jpg" | "jpeg" | "webp" | "svg" | "gif" => ("image", None),
                "md" | "txt" | "json" => {
                    let raw = std::fs::read(p)
                        .map_err(|e| format!("read demo {}: {e}", p.display()))?;
                    if raw.len() > 1024 * 1024 {
                        ("text", Some("[demo 超过 1MB,已截断 — 用系统应用打开看完整]".to_string()))
                    } else {
                        let s = String::from_utf8_lossy(&raw).to_string();
                        ("text", Some(s))
                    }
                }
                _ => ("unknown", None),
            }
        }
    };

    Ok(SkillDemoPayload {
        file_path: file_path.map(|p| p.to_string_lossy().to_string()),
        file_kind,
        content,
        preview_path,
        description: skill.demo.description.clone(),
        duration: skill.demo.duration_estimate.clone(),
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
    pub phases: Vec<deepseek_tui::skills::PhaseDef>,
    pub current_phase_id: Option<String>,
}

/// 用户在「工作流」视图点 skill 卡片「启用」 → 新建一个 session 并把该 skill
/// 绑定到这个 session。每次点都新建独立 session,skill 仅对该 session 生效
/// (不再有全局 active_skill 单例)。
#[tauri::command]
pub async fn start_skill_session(
    name: String,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<StartSkillSessionResult, String> {
    use crate::bridge::mode_state::ActiveSkillBinding;
    use crate::bridge::paths;
    use deepseek_tui::skills::SkillRegistry;

    if WORKFLOW_HIDDEN_SKILLS.contains(&name.as_str()) {
        return Err(format!(
            "{name} 是系统基础能力,不能直接启用为工作流"
        ));
    }

    // 1) 只在 bundle/skills 里找 — 跟 list_skills_v2 source of truth 保持一致
    let dir = paths::bundle_skills_dir();
    if !dir.exists() {
        return Err(format!("skills dir not found: {}", dir.display()));
    }
    let registry = SkillRegistry::discover(&dir);
    let skill = registry
        .get(&name)
        .ok_or_else(|| format!("skill not found: {name}"))?
        .clone();

    // 2) 新建 session(沿用 create_session 的 model + workspace 取值)
    let model = pool.bridge.model();
    let workspace = pool.bridge.workspace.clone();
    let session = store
        .create_new(model, workspace)
        .map_err(|e| format!("create_session: {e:?}"))?;
    let sid = session.metadata.id.clone();
    store.set_active(Some(sid.clone()));

    // 3) 多 session 并发:不预热 engine(lazy)。首条 chat 时 EnginePool 为这个空 session
    //    spawn 专属 engine,空历史无需 SyncSession。

    // 4) 绑定 skill。pending_instruction 是单行 nudge — 底座 system prompt
    //    已经自动注入了 skill 列表 + phase 强约束(crates/tui/src/skills/mod.rs)
    //    + 自动从回复抽 `<phase id="..."/>` emit Event::PhaseChanged,所以这里
    //    只发一条触发提示,不重复 dump skill body / phase 规则。
    let injected = format!(
        "[pinvou3-app] 用户在「工作流」视图启用了 skill: `{name}`。\
         请按该 skill 的 phases 流程响应 — engine 会自动从你回复里抽 \
         `<phase id=\"...\"/>` marker 驱动 UI 上方的 phase chip 条,\
         **每条回复必须以该 marker 单独一行开头**(详见 system prompt 里 \
         「Phase tracking — MANDATORY for phased skills」段)。",
        name = skill.name,
    );

    let first_phase = skill.phases.first().map(|p| p.id.clone());
    let phases = skill.phases.clone();
    store.bind_skill(
        &sid,
        ActiveSkillBinding {
            name: skill.name.clone(),
            pending_instruction: Some(injected),
            phases: phases.clone(),
        },
    );

    Ok(StartSkillSessionResult {
        session: session.metadata,
        skill: ActiveSkillState {
            name: skill.name,
            phases,
            current_phase_id: first_phase,
        },
    })
}

/// 解除指定 session 的 skill 绑定(用户点 chips 区 ✕)。
/// 不删 session,只清绑定 — chips strip 隐藏,普通对话照常继续。
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
        let first = b.phases.first().map(|p| p.id.clone());
        ActiveSkillState {
            name: b.name,
            phases: b.phases,
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
