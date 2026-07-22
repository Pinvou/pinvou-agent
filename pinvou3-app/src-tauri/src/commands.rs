//! Tauri 命令实现。前端通过 `invoke(name, args)` 调到这里。
//!
//! 暴露的命令：
//! - `chat(message)`         — 发送用户消息（流式响应通过 chat:* 事件）
//! - `get_settings()`        — 读 `~/.pinvou3/settings.json`（UserPrefs）
//! - `update_settings(prefs)`— 写盘；GUI 项立即生效，引擎相关项需重启 app
//! - `clear_session()`       — 清前端显示（MVP）；后端 session 重启 app 才真清
//! - `get_monitor_snapshot()`— Monitor 视图完整数据
//! - `get_backend_status()`  — ChatRoom 顶部 live dot 用，简版健康指示
//! - `discover_local_vllm()` — 设置页手动探测本机 vLLM 候选端点
//!
//! 阶段 C 新增（多对话历史）：
//! - `list_sessions()` / `create_session()` / `load_session(id)`
//! - `delete_session(id)` / `rename_session(id, title)` / `get_active_session()`

use deepseek_tui::models::Message;
use deepseek_tui::session_manager::{SavedSession, SessionMetadata};
use deepseek_tui::tools::user_input::{UserInputAnswer, UserInputResponse};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::bridge::mode_state::{SerializableMode, SessionModeState};
use crate::bridge::prefs::{SavedModel, SearchProvider, UserPrefs};
use crate::bridge::sessions::{SessionKind, SessionStore};
use crate::credential_store::{
    CredentialEditAction, CredentialState, CredentialStore, SystemCredentialStore,
};
use crate::engine_pool::EnginePool;
use crate::knowledge::KnowledgeService;
use crate::monitor::{MonitorSnapshot, MonitorState, VllmStatus};

#[derive(Debug, Clone, Serialize)]
pub struct SessionListItem {
    #[serde(flatten)]
    pub metadata: SessionMetadata,
    pub pinned: bool,
    pub pinned_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HiddenSessionListItem {
    #[serde(flatten)]
    pub metadata: SessionMetadata,
    pub hidden_at: Option<String>,
    #[serde(rename = "archived_at")]
    pub archived_at: Option<String>,
}

/// 仅普通 chat 会话可用的命令守卫（transcript/产物由前端覆盖持久化的路径）。
/// 重命名/置顶/归档/删除等元数据操作按 SessionKind 分发，不走这个守卫。
fn ensure_chat_session(store: &SessionStore, id: &str, action: &str) -> Result<(), String> {
    match store
        .session_kind(id)
        .map_err(|error| format!("{action}({id}): {error:?}"))?
    {
        SessionKind::Chat => Ok(()),
        SessionKind::ScheduledRun => Err(format!(
            "{action}({id}): scheduled-run sessions are managed from Scheduled"
        )),
    }
}

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
    restrict_tools: Option<bool>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
    app: AppHandle,
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
    let execution_workspace = store
        .execution_workspace(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    let is_scheduled = store.scheduled_profile(&sid).is_some();
    if is_scheduled {
        for attachment in attachments.as_deref().unwrap_or_default() {
            validate_staged_attachment_basename(&attachment.basename).map_err(|error| {
                format!(
                    "invalid scheduled attachment basename {:?}: {error}",
                    attachment.basename
                )
            })?;
        }
    }
    let attachment_dir = if is_scheduled {
        // The scheduled engine runs in the configured project workspace, so
        // staged paths must remain relative to that same root. Isolate every
        // run under an app-named hidden directory to avoid cross-run clashes.
        format!(".pinvou3/scheduled-attachments/{sid}/attachments")
    } else {
        "attachments".to_string()
    };
    let raw_message = message.clone();
    crate::timing::start_turn(&sid);
    let mut full = build_message_with_attachments_in_dir(
        message,
        attachments.unwrap_or_default(),
        &execution_workspace,
        &attachment_dir,
    );
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
    // [phase marker 下线] 原 active_skill 非工作流分支每 turn 注入 `<phase id=.../>`
    // marker reminder,但消费链(底座抽取 → chat:phase_changed → 前端 chips)已整体拆除,
    // marker 产出后无人消费。已删。active_skill 的 pending_instruction(skill body)仍走上面。

    // Side B 卡片池: 加持后首条消息一次性 prepend 完整人设 body(agency-agents-zh)。
    // 之后每 turn 只靠 equip_anchor 轻锚点维持身份(EnginePool 注入),不再重灌 body。
    if let Some(body) = store.take_pending_persona_body(&sid) {
        full = format!("{body}\n\n---\n\n{full}");
    }
    let memory_enabled = crate::memory::memory_enabled();
    if memory_enabled {
        crate::memory::record_turn_user(&sid, &raw_message);
    }
    match crate::memory::runtime_snapshot(&sid) {
        Ok(snapshot) => {
            let _ = app.emit(
                "chat:memory",
                serde_json::json!({
                    "session_id": sid.clone(),
                    "items": snapshot.items,
                    "runtime_path": snapshot.runtime_path,
                }),
            );
        }
        Err(err) => {
            eprintln!("[pinvou3-app] refresh memory runtime failed for session {sid}: {err}");
        }
    }
    // Agentic RAG:该 session 挂了知识集 → 每 turn prepend Self-RAG 自检引导,让模型自己
    // 调 kb_search 工具(engine 已注入)检索、严格基于结果作答、无依据就说不知道。不再自动
    // 注入片段(注入式已废弃)。collection_name 是单行查询,直接调即可(非大查询不必 spawn)。
    //
    // 关键防线:kb_search 的可见性是 engine config.disallowed_tools 控制的,而知识库模型/
    // 索引状态可能在 engine spawn 后才变化。挂集 turn 先刷新 live engine 的工具门控;
    // 若 kb_search 当前仍不可用,不要注入“必须调用 kb_search”的提示,避免模型把提示/sudo
    // 状态当普通文本复述给用户。
    if let Some(cid) = store.mounted_collection(&sid) {
        let disallowed = compute_disallowed_tools(&app);
        let kb_search_hidden = disallowed
            .iter()
            .any(|t| t.eq_ignore_ascii_case("kb_search"));
        pool.set_disallowed_all(disallowed).await;
        if !kb_search_hidden {
            let coll_name = app
                .try_state::<KnowledgeService>()
                .and_then(|kb| kb.l1().collection_name(cid).ok().flatten());
            full = format!(
                "{}\n\n---\n\n{full}",
                build_kb_agentic_guide(coll_name.as_deref())
            );
        }
    }
    // 取该 session 的 mode。
    let mode = store.mode_state(&sid).mode;
    match pool
        .send_user_message(
            &sid,
            full,
            mode.to_app_mode(),
            restrict_tools.unwrap_or(false),
        )
        .await
    {
        Ok(()) => Ok(()),
        Err(e) => {
            crate::timing::finish_turn(&sid, "send_error", Some(&format!("{e:?}")));
            Err(format!("send_user_message failed: {e:?}"))
        }
    }
}

/// Agentic RAG 的 Self-RAG 自检引导:挂了知识集时每 turn prepend(动态状态走 per-turn
/// 注入)。引导模型自调 `kb_search`、严格基于检索结果作答、无依据就说不知道——治本地小
/// 模型"该查不查 → 凭记忆幻觉"(去掉注入式兜底后这是关键防线)。
fn build_kb_agentic_guide(collection_name: Option<&str>) -> String {
    let title = collection_name.unwrap_or("本地知识集");
    format!(
        "<system-reminder>\n\
         本会话挂载了知识集《{title}》。涉及用户本地资料/文档的问题,你**必须先调用 \
         `kb_search` 工具**检索,再**严格基于返回的片段**作答并注明来源文件;检索不到相关\
         内容就如实告诉用户「未在知识集中找到」,**绝不凭记忆编造**。与本地资料无关的闲聊/\
         常识问题不必检索,正常回答即可。\n\
         </system-reminder>"
    )
}

/// 给会话挂载一个知识集(会话级粘连)。后续每条消息发送前自动检索注入。
#[tauri::command]
pub fn session_mount_collection(
    session_id: String,
    collection_id: i64,
    store: State<'_, SessionStore>,
    knowledge: State<'_, KnowledgeService>,
    app: AppHandle,
) -> Result<(), String> {
    // 完全门控:embedding 模型没就绪 → 知识库整体不可用,拒绝挂载。前端会置灰入口,
    // 这里是防绕过兜底(草稿态直调 / 旧前端 / 命令注入)。
    if !knowledge.semantic_ready() {
        return Err("embedding 模型未就绪,知识库暂不可用".to_string());
    }
    store.set_mounted_collection(&session_id, Some(collection_id));
    // 挂载已落盘成功,emit 失败不应让命令失败(只影响远程客户端的同步提示)。
    let _ = app.emit(
        "remote_control:kb_mount_changed",
        serde_json::json!({ "session_id": session_id, "collection_id": collection_id }),
    );
    // 同时把变更同步给正在远控的 mobile 端(若该 session 正在被远控),避免 mobile UI 陈旧。
    // 与 set_disabled_connectors 的双向广播对称:桌面本地变更也必须通知 mobile。
    broadcast_kb_mount_to_mobile(&app, &session_id, Some(collection_id));
    Ok(())
}

/// 摘下会话的知识集挂载。
#[tauri::command]
pub fn session_unmount_collection(
    session_id: String,
    store: State<'_, SessionStore>,
    app: AppHandle,
) {
    store.set_mounted_collection(&session_id, None);
    let _ = app.emit(
        "remote_control:kb_mount_changed",
        serde_json::json!({ "session_id": session_id, "collection_id": null }),
    );
    broadcast_kb_mount_to_mobile(&app, &session_id, None);
}

/// 桌面本地 KB 挂载/摘挂变更 → 推给 mobile(若 session 正在被远控)。
/// payload 形状与 manager.rs dispatch 内 emit 的 kb_mount_changed 一致,确保
/// mobile handleDesktopEvent 单一 case 能同时处理 mobile-triggered 和 desktop-triggered。
fn broadcast_kb_mount_to_mobile(app: &AppHandle, session_id: &str, collection_id: Option<i64>) {
    if let Some(manager) = app.try_state::<crate::remote_control::RemoteControlManager>() {
        let payload = serde_json::json!({
            "session_id": session_id,
            "collection_id": collection_id,
        });
        manager.broadcast_to_mobile(session_id, "kb_mount_changed", payload);
    }
}

/// 读会话当前挂载的知识集 id(前端切会话时重读,恢复挂载条显示)。
#[tauri::command]
pub fn session_mounted_collection(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Option<i64> {
    store.mounted_collection(&session_id)
}

/// 把图片附件拷进 session workspace 的 `attachments/` 子目录,返回供 `image_analyze`
/// 使用的 **workspace 相对路径**(image_analyze 只接受不逃逸 workspace 的相对路径)。
/// 失败返回 None,上层降级为提示无法读图。
fn validate_staged_attachment_basename(basename: &str) -> Result<(), String> {
    if basename.is_empty() {
        return Err("basename is empty".to_string());
    }
    if basename.contains('/') || basename.contains('\\') {
        return Err("basename must not contain path separators".to_string());
    }
    let bytes = basename.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err("basename must not contain a drive prefix".to_string());
    }

    let mut components = std::path::Path::new(basename).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(_)), None) => Ok(()),
        _ => Err("basename must be exactly one normal path component".to_string()),
    }
}

// Staging security boundary:
// - basename validation blocks traversal, absolute paths, drive prefixes, and separators;
// - canonical parent walking rejects pre-existing symlink/junction escapes;
// - create_new prevents overwriting an existing file or following a pre-existing target link;
// - exclusive reservation gives benign concurrent writers distinct names.
//
// This does not claim to defeat a malicious process that already has write access to the same
// workspace and actively swaps a parent between validation and open. Such a process already has
// equivalent write authority; closing that residual race requires platform-specific handle-relative
// APIs (for example openat-style directory handles), deliberately outside this local staging helper.
fn prepare_staging_directory(
    workspace: &std::path::Path,
    attachment_dir: &str,
) -> Option<(String, std::path::PathBuf, std::path::PathBuf)> {
    let attachment_dir = attachment_dir.trim_end_matches('/');
    let relative = std::path::Path::new(attachment_dir);
    if relative.as_os_str().is_empty() || relative.is_absolute() {
        return None;
    }
    let components = relative
        .components()
        .map(|component| match component {
            std::path::Component::Normal(name) => Some(name.to_os_string()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;

    std::fs::create_dir_all(workspace).ok()?;
    let canonical_workspace = std::fs::canonicalize(workspace).ok()?;
    let mut canonical_parent = canonical_workspace.clone();
    for component in components {
        canonical_parent.push(component);
        match std::fs::create_dir(&canonical_parent) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return None,
        }
        let metadata = std::fs::symlink_metadata(&canonical_parent).ok()?;
        if !metadata.is_dir() {
            return None;
        }
        let resolved = std::fs::canonicalize(&canonical_parent).ok()?;
        if !resolved.starts_with(&canonical_workspace) {
            return None;
        }
        // Continue from the resolved parent rather than the user-visible path.
        // A pre-existing link can therefore never redirect creation of the
        // next component outside the canonical execution workspace.
        canonical_parent = resolved;
    }

    Some((
        attachment_dir.to_string(),
        canonical_workspace,
        canonical_parent,
    ))
}

fn reserve_unique_staged_file(
    directory: &std::path::Path,
    initial_name: String,
    stem: &str,
    suffix: &str,
) -> Option<(std::fs::File, std::path::PathBuf, String)> {
    const MAX_CANDIDATES: usize = 10_000;
    for attempt in 0..MAX_CANDIDATES {
        let candidate = if attempt == 0 {
            initial_name.clone()
        } else {
            format!("{stem}-{attempt}{suffix}")
        };
        let path = directory.join(&candidate);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Some((file, path, candidate)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

#[cfg(unix)]
fn staged_reserved_target_is_unchanged(file: &std::fs::File, path: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let (Ok(opened), Ok(named)) = (file.metadata(), std::fs::symlink_metadata(path)) else {
        return false;
    };
    named.file_type().is_file() && opened.dev() == named.dev() && opened.ino() == named.ino()
}

#[cfg(windows)]
fn staged_reserved_target_is_unchanged(_file: &std::fs::File, path: &std::path::Path) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let Ok(named) = std::fs::symlink_metadata(path) else {
        return false;
    };
    named.file_type().is_file() && named.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
}

#[cfg(not(any(unix, windows)))]
fn staged_reserved_target_is_unchanged(_file: &std::fs::File, path: &std::path::Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

fn staged_target_is_safe(
    file: &std::fs::File,
    path: &std::path::Path,
    canonical_workspace: &std::path::Path,
) -> bool {
    staged_reserved_target_is_unchanged(file, path)
        && std::fs::canonicalize(path)
            .is_ok_and(|resolved| resolved.starts_with(canonical_workspace))
}

fn stage_image_in_workspace(
    src: &str,
    basename: &str,
    workspace: &std::path::Path,
    attachment_dir: &str,
) -> Option<String> {
    validate_staged_attachment_basename(basename).ok()?;
    let (attachment_dir, canonical_workspace, directory) =
        prepare_staging_directory(workspace, attachment_dir)?;
    let mut source = std::fs::File::open(src).ok()?;
    let (stem, suffix) = match basename.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (basename.to_string(), String::new()),
    };
    let (mut destination, path, candidate) =
        reserve_unique_staged_file(&directory, basename.to_string(), &stem, &suffix)?;
    if !staged_target_is_safe(&destination, &path, &canonical_workspace) {
        return None;
    }
    if std::io::copy(&mut source, &mut destination).is_err() {
        return None;
    }
    if !staged_target_is_safe(&destination, &path, &canonical_workspace) {
        return None;
    }
    Some(format!("{attachment_dir}/{candidate}"))
}

/// 把远控上传的临时源文件复制进 session workspace，再把稳定绝对路径交给正常附件链路。
/// 远控上传目录可以在消息进入桌面端后立即清理；图片分析和大文本按路径读取不会再与
/// 临时目录删除竞争。目录使用隐藏前缀，避免把输入附件误当成用户产出物展示。
pub(crate) fn stage_remote_attachment_source(
    src: &str,
    basename: &str,
    workspace: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let relative =
        stage_image_in_workspace(src, basename, workspace, ".pinvou3/remote-attachments")?;
    Some(workspace.join(relative))
}

/// 附件内联预算(token 估算)。单文件超过 INLINE_MAX、或多附件累计超过 TOTAL_BUDGET
/// 的部分,不再全量嵌入 prompt——256K 窗口一条消息就能撑爆(实测 5000 行 xlsx 转
/// CSV ≈ 237K tokens,直接顶穿 vLLM 262144 上限),且即使不炸窗口,小模型在超长
/// 内联里的注意力质量也差。超限附件改注入「落盘路径 + 预览」,引导模型按需
/// read_file 分页 / exec_shell 聚合(底座 read_file 原生支持 start_line/max_lines)。
const ATTACH_INLINE_MAX_TOKENS: u32 = 8_000;
const ATTACH_TOTAL_BUDGET_TOKENS: u32 = 16_000;
/// 路径模式的开头预览:行数与字符双上限,先到为准。
const ATTACH_PREVIEW_LINES: usize = 20;
const ATTACH_PREVIEW_MAX_CHARS: usize = 1_500;

/// 把超限附件的转换产物写进指定的 workspace 相对目录(防重名递增),返回
/// workspace 相对路径。普通对话的 text 仍直接使用原路径；scheduled 对话会复制
/// 到 run 专属目录，避免无人值守引擎依赖 workspace 外路径。
fn stage_text_in_workspace(
    content: &str,
    basename: &str,
    ext: &str,
    workspace: &std::path::Path,
    attachment_dir: &str,
) -> Option<String> {
    use std::io::Write as _;
    stage_text_in_workspace_with_writer(
        basename,
        ext,
        workspace,
        attachment_dir,
        |destination, _path| destination.write_all(content.as_bytes()),
    )
}

fn stage_text_in_workspace_with_writer<F>(
    basename: &str,
    ext: &str,
    workspace: &std::path::Path,
    attachment_dir: &str,
    writer: F,
) -> Option<String>
where
    F: FnOnce(&mut std::fs::File, &std::path::Path) -> std::io::Result<()>,
{
    validate_staged_attachment_basename(basename).ok()?;
    let (attachment_dir, canonical_workspace, directory) =
        prepare_staging_directory(workspace, attachment_dir)?;
    let stem = basename.rsplit_once('.').map_or(basename, |(s, _)| s);
    let suffix = format!(".{ext}");
    let (mut destination, path, candidate) =
        reserve_unique_staged_file(&directory, format!("{stem}{suffix}"), stem, &suffix)?;
    if !staged_target_is_safe(&destination, &path, &canonical_workspace) {
        return None;
    }
    if writer(&mut destination, &path).is_err() {
        // Deliberately leave the app-named orphan in place. Unlinking by path after a failed
        // post-check would introduce another check-then-unlink race and could delete a replacement
        // installed by a concurrent writer.
        return None;
    }
    if !staged_target_is_safe(&destination, &path, &canonical_workspace) {
        return None;
    }
    Some(format!("{attachment_dir}/{candidate}"))
}

/// 转换产物落盘时的扩展名:表格是 CSV(awk/python 可直接吃),pandoc 产物是
/// markdown,其余(pdftotext/LibreOffice txt/邮件)是纯文本。
fn converted_ext(kind: &str) -> &'static str {
    match kind {
        "xlsx" | "ods" | "xls" | "et" => "csv",
        "docx" | "odt" | "archive" => "md",
        _ => "txt",
    }
}

/// 取 markdown 开头若干行做预览,返回 (预览, 总行数)。
fn attachment_preview(md: &str) -> (String, usize) {
    let total_lines = md.lines().count();
    let mut preview = String::new();
    for (i, line) in md.lines().enumerate() {
        if i >= ATTACH_PREVIEW_LINES
            || preview.chars().count() + line.chars().count() > ATTACH_PREVIEW_MAX_CHARS
        {
            break;
        }
        preview.push_str(line);
        preview.push('\n');
    }
    (preview, total_lines)
}

/// 超限附件的注入段:落盘(text 类直接用原始路径)+ 预览 + 工具引导。
/// 显式声明「只看到预览」——否则小模型会拿前 20 行当全量数据静默作答。
fn push_large_attachment_section(
    out: &mut String,
    a: &crate::file_ingest::IngestResult,
    md: &str,
    workspace: &std::path::Path,
    attachment_dir: &str,
    stage_original_text: bool,
) {
    let read_path = if a.kind == "text" && !stage_original_text {
        a.path.clone()
    } else {
        match stage_text_in_workspace(
            md,
            &a.basename,
            converted_ext(&a.kind),
            workspace,
            attachment_dir,
        ) {
            Some(rel) => rel,
            None => {
                out.push_str(
                    "⚠️ 此文件过大无法内嵌,且转换产物落盘失败。请告知用户该附件无法处理,\
                     不要臆测其内容。\n",
                );
                return;
            }
        }
    };
    let (preview, total_lines) = attachment_preview(md);
    out.push_str(&format!(
        "⚠️ 此文件约 ~{} tokens,过大,完整内容**没有**嵌入本消息。你只看到下面的开头预览,\
         **绝不能**只凭预览回答涉及全文/全表的问题。\n\
         完整内容已是纯文本,共 {} 行,路径: `{}`\n\
         预览(仅开头几行):\n```\n{}```\n\
         需要完整内容时:\n\
         - 统计/筛选/聚合(尤其表格数据):优先用 exec_shell 写 awk 或 python 一次算出结果,不要逐页通读\n\
         - 通读/定位:用 read_file 分页(start_line/max_lines;返回 truncated=\"true\" 时按 next_start_line 续读)\n",
        a.token_estimate, total_lines, read_path, preview
    ));
}

/// 按指定 workspace 相对目录拼接 user 文本 + 附件 markdown。
/// 图片拷进 workspace 后引导 LLM 调 image_analyze 读图(Qwen3.6 有视觉能力);
/// 文本类附件按 token 预算分流:小→全量内联,大→落盘+路径+预览(见常量注释)。
fn build_message_with_attachments_in_dir(
    text: String,
    attachments: Vec<crate::file_ingest::IngestResult>,
    workspace: &std::path::Path,
    attachment_dir: &str,
) -> String {
    if attachments.is_empty() {
        return text;
    }
    let mut out = String::new();
    if !text.trim().is_empty() {
        out.push_str(&text);
        out.push_str("\n\n");
    }
    out.push_str("---\n用户附上了以下文件:\n\n");
    let mut inline_spent: u32 = 0;
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
            // 把图拷进 workspace,硬约束引导 LLM 调 image_analyze 读图。
            // 关键:不能说"你有视觉能力"——那会让模型以为可直接描述而凭空幻觉
            // (实测同一张图,不调工具时编造内容,调工具才得真相)。改成"你现在
            // 一无所知,调用前绝不描述",把模糊建议变成具体硬规则(Qwen3.6 对具体
            // 硬规则遵循好、对抽象意图无效)。
            match stage_image_in_workspace(&a.path, &a.basename, workspace, attachment_dir) {
                Some(rel) => {
                    out.push_str(&format!(
                        "🖼 用户附了一张图片,存在 workspace 的 `{rel}`。\n\
                        ⚠️ 你现在**看不到这张图的任何内容**,对图里有什么**一无所知**。\
                        在调用 image_analyze 工具并拿到返回结果之前,你**绝对不能**描述、\
                        猜测或编造图里有什么——包括「这是什么」「帅吗」「什么颜色」「是不是某某文档」\
                        这类**任何**关于图的问题。凭空作答=幻觉,是严重错误。\n\
                        要回答**任何**跟这张图有关的问题,**必须先**调用:\n\
                        `image_analyze(image_path=\"{rel}\", prompt=\"<按用户问题要看的,如:描述这张图/读出文字/这是什么>\")`\n\
                        拿到工具返回的描述后,再据此如实回答用户。\n",
                    ));
                }
                None => {
                    out.push_str(
                        "⚠️ 这张图片暂存到 workspace 失败,无法用 image_analyze 读取。\
                        请告知用户图片无法处理,不要臆测图里的内容。\n",
                    );
                }
            }
        } else if let Some(md) = &a.markdown {
            let fits = a.token_estimate <= ATTACH_INLINE_MAX_TOKENS
                && inline_spent.saturating_add(a.token_estimate) <= ATTACH_TOTAL_BUDGET_TOKENS;
            if fits {
                inline_spent = inline_spent.saturating_add(a.token_estimate);
                out.push_str(
                    "**以下代码块是文件完整内容,可直接使用,不需要再调 read_file / \
                     file_search 重新读取。**如需保存修改版本,用 write_file 写到 \
                     PINVOU3_WORKSPACE 下;大产物用 append_file 分块追加。\n",
                );
                out.push_str("```\n");
                out.push_str(md);
                if !md.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("```\n");
            } else {
                push_large_attachment_section(
                    &mut out,
                    a,
                    md,
                    workspace,
                    attachment_dir,
                    attachment_dir != "attachments",
                );
            }
        } else if let Some(warning) = &a.warning {
            out.push_str(&format!("⚠️ {warning}\n"));
        }
        out.push('\n');
    }
    out.push_str("---\n");
    out
}

/// 普通对话附件入口。pub 仅为 L1 dialog harness 复用(lib.rs re-export)，
/// 不是对外 API；scheduled chat 走上面的 run 专属目录入口。
pub fn build_message_with_attachments(
    text: String,
    attachments: Vec<crate::file_ingest::IngestResult>,
    workspace: &std::path::Path,
) -> String {
    build_message_with_attachments_in_dir(text, attachments, workspace, "attachments")
}

/// 从 disk 读最新 UserPrefs。
/// 注意走 disk 而非 engine.bridge.prefs——如果用户手改 settings.json，
/// `get_settings()` 能立刻拿到，不需要 reload bridge。
#[tauri::command]
pub async fn get_settings() -> Result<UserPrefs, String> {
    Ok(refresh_safe_prefs(UserPrefs::load()))
}

fn sanitize_command_error(context: &str, err: impl std::fmt::Display) -> String {
    format!(
        "{context}: {}",
        crate::credential_store::redact_secret(&err.to_string())
    )
}

fn prepare_prefs_for_save(mut prefs: UserPrefs) -> Result<UserPrefs, String> {
    let store = SystemCredentialStore::new();
    let migration = prefs.migrate_plaintext_api_keys_with_store(&store);
    if !migration.failed_model_ids.is_empty() || !migration.failed_search_providers.is_empty() {
        return Err("credential store unavailable; please reconfigure API Key".to_string());
    }
    prefs.sanitize_plaintext_api_keys();
    // migrate/sanitize 后补一次真实回读,确保写盘的 credential_state 反映存储实际内容
    // (避免 keep_existing 时 credential_ref 存在但存储为空 → 写入假阳性 Configured)。
    prefs.refresh_credential_states_with_store(&store);
    Ok(prefs)
}

fn refresh_safe_prefs(mut prefs: UserPrefs) -> UserPrefs {
    prefs.refresh_credential_states_with_store(&SystemCredentialStore::new());
    prefs.sanitize_plaintext_api_keys();
    prefs
}

fn apply_model_credential(
    mut model: SavedModel,
    old: Option<&SavedModel>,
) -> Result<SavedModel, String> {
    let store = SystemCredentialStore::new();
    let action = model.credential_action.unwrap_or_else(|| {
        if model.api_key.trim().is_empty() {
            CredentialEditAction::KeepExisting
        } else {
            CredentialEditAction::Replace
        }
    });

    match action {
        CredentialEditAction::KeepExisting => {
            if let Some(old) = old {
                model.credential_ref = old.credential_ref.clone();
                model.credential_state = old.credential_state;
                model.has_secret = old.has_secret;
            } else if model.api_key.trim().is_empty() {
                model.mark_missing();
            } else {
                let reference = model.credential_reference();
                store
                    .set(&reference, model.api_key.trim())
                    .map_err(|e| e.user_message())?;
                model.mark_configured(reference);
            }
        }
        CredentialEditAction::Replace => {
            let key = model.api_key.trim().to_string();
            if key.is_empty() {
                model.mark_missing();
            } else {
                let reference = model.credential_reference();
                store.set(&reference, &key).map_err(|e| e.user_message())?;
                model.mark_configured(reference);
            }
        }
        CredentialEditAction::Delete => {
            let reference = model
                .credential_ref
                .clone()
                .or_else(|| old.and_then(|m| m.credential_ref.clone()))
                .unwrap_or_else(|| model.credential_reference());
            store.delete(&reference).map_err(|e| e.user_message())?;
            model.mark_missing();
        }
    }
    model.clear_plaintext_key();
    Ok(model)
}

fn resolve_saved_model_key(model_id: Option<&str>) -> Result<Option<String>, String> {
    let prefs = UserPrefs::load();
    let model = model_id
        .and_then(|id| prefs.model_by_id(id))
        .or_else(|| prefs.active_model());
    let Some(model) = model else {
        return Ok(None);
    };
    let Some(reference) = &model.credential_ref else {
        return Ok(None);
    };
    SystemCredentialStore::new()
        .get(reference)
        .map_err(|e| e.user_message())
}

#[tauri::command]
pub async fn submit_feedback(
    request: crate::feedback::FeedbackSubmitRequest,
) -> Result<crate::feedback::FeedbackReceipt, String> {
    crate::feedback::submit_feedback(request)
        .await
        .map_err(|e| e.to_string())
}

/// 实际生效的模型配置（环境变量可能覆盖 settings.json）。
/// 前端设置页初始化时优先用这个，避免"改了 settings 但实际不生效"的困惑。
#[derive(Debug, Clone, Serialize)]
pub struct EffectiveModelConfig {
    pub preset: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub credential_state: CredentialState,
    pub has_secret: bool,
    pub provider: String,
    /// 被环境变量覆盖的字段名列表（如 `["model", "base_url"]`）。
    /// 空列表表示全部走 settings.json，用户修改会生效。
    pub env_overrides: Vec<String>,
}

#[tauri::command]
pub async fn get_effective_model_config(
    pool: State<'_, EnginePool>,
) -> Result<EffectiveModelConfig, String> {
    // 读 disk 最新 prefs(GUI 可能刚改过模型/默认),boot 快照会过时。
    let mut bridge = pool.bridge.clone();
    bridge.prefs = UserPrefs::load();
    bridge.session_model = None; // 全局视角,不绑定具体 session
    let mut env_overrides = Vec::new();
    if std::env::var("DEEPSEEK_MODEL").is_ok() {
        env_overrides.push("model".to_string());
    }
    if std::env::var("DEEPSEEK_BASE_URL").is_ok() {
        env_overrides.push("base_url".to_string());
    }
    if std::env::var("DEEPSEEK_API_KEY").is_ok() {
        env_overrides.push("api_key".to_string());
    }
    if std::env::var("DEEPSEEK_PROVIDER").is_ok_and(|provider| provider == bridge.provider()) {
        env_overrides.push("provider".to_string());
    }
    let preset = bridge
        .prefs
        .active_model()
        .map(|m| m.preset)
        .unwrap_or_default()
        .as_str();
    let active = bridge.prefs.active_model();
    Ok(EffectiveModelConfig {
        preset: preset.to_string(),
        model: bridge.model(),
        base_url: bridge.base_url(),
        api_key: String::new(),
        credential_state: if std::env::var("DEEPSEEK_API_KEY").is_ok() {
            CredentialState::EnvOverride
        } else {
            active
                .map(|m| m.credential_state)
                .unwrap_or(CredentialState::Missing)
        },
        has_secret: active.map(|m| m.has_secret).unwrap_or(false),
        provider: bridge.provider(),
        env_overrides,
    })
}

/// 「添加模型」方案:列出已保存模型 + 当前全局默认 id(前端高亮)。
#[derive(Debug, Clone, Serialize)]
pub struct ModelsView {
    pub models: Vec<SavedModel>,
    pub active_model_id: Option<String>,
}

#[tauri::command]
pub async fn list_models() -> Result<ModelsView, String> {
    let prefs = refresh_safe_prefs(UserPrefs::load());
    Ok(ModelsView {
        models: prefs.advanced.saved_models.clone(),
        active_model_id: prefs.advanced.active_model_id.clone(),
    })
}

/// 用户在编辑模型弹窗里主动点击“显示”时，读取该模型已保存的 API Key。
/// 环境变量覆盖的凭据不回显，避免给出一个前端并不拥有、保存也不会覆盖的值。
#[tauri::command]
pub async fn reveal_model_api_key(id: String) -> Result<Option<String>, String> {
    let prefs = refresh_safe_prefs(UserPrefs::load());
    let model = prefs
        .model_by_id(&id)
        .ok_or_else(|| format!("model not found: {id}"))?;
    if model.credential_state == CredentialState::EnvOverride {
        return Ok(None);
    }
    let Some(reference) = &model.credential_ref else {
        return Ok(None);
    };
    SystemCredentialStore::new()
        .get(reference)
        .map_err(|e| sanitize_command_error("reveal_model_api_key", e.user_message()))
}

/// 增或改一条模型(按 id)。前端负责生成稳定 id。
#[tauri::command]
pub async fn save_model(model: SavedModel) -> Result<(), String> {
    let mut prefs = UserPrefs::load();
    let old = prefs.model_by_id(&model.id).cloned();
    let model = apply_model_credential(model, old.as_ref())
        .map_err(|e| sanitize_command_error("save_model", e))?;
    prefs.upsert_model(model);
    prefs.save().map_err(|e| format!("save_model: {e:?}"))
}

/// 删一条模型。至少保留一条;删到当前 active 会自动回退列表首条。
#[tauri::command]
pub async fn delete_model(id: String) -> Result<(), String> {
    let mut prefs = UserPrefs::load();
    if prefs.advanced.saved_models.len() <= 1 {
        return Err("至少保留一个模型".to_string());
    }
    if let Some(reference) = prefs
        .model_by_id(&id)
        .and_then(|m| m.credential_ref.clone())
    {
        SystemCredentialStore::new()
            .delete(&reference)
            .map_err(|e| sanitize_command_error("delete_model", e.user_message()))?;
    }
    prefs.remove_model(&id);
    prefs.save().map_err(|e| format!("delete_model: {e:?}"))
}

/// 设全局默认模型(新建会话继承它)。不打断已在用的会话——它们各自保持 spawn
/// 时的模型,想换在该会话的 chip 里切。
#[tauri::command]
pub async fn set_active_model(id: String) -> Result<(), String> {
    let mut prefs = UserPrefs::load();
    if prefs.model_by_id(&id).is_none() {
        return Err(format!("model not found: {id}"));
    }
    prefs.advanced.active_model_id = Some(id);
    prefs.save().map_err(|e| format!("set_active_model: {e:?}"))
}

/// 切某会话当前模型(聊天 chip 热切):写 per-session 绑定 + evict 该会话 engine,
/// 下次发消息用新模型重建。`model_id = None` = 回退全局默认。
/// 前端须保证非生成中调用(evict 会打断正在跑的 turn)。
#[tauri::command]
pub async fn set_session_model(
    session_id: String,
    model_id: Option<String>,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    if let Some(mid) = &model_id {
        if UserPrefs::load().model_by_id(mid).is_none() {
            return Err(format!("model not found: {mid}"));
        }
    }
    pool.switch_session_model(&session_id, model_id)
        .await
        .map_err(|error| format!("set_session_model({session_id}): {error:#}"))
}

/// 读取聊天 chip 应显示的模型 id。定时会话尚未手动切换时显示任务初始模型，
/// 手动切换后与普通会话一样显示交互选择。
#[tauri::command]
pub async fn get_session_model_id(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<Option<String>, String> {
    Ok(store.session_model_id(&session_id))
}

fn parse_search_provider(raw: &str) -> Result<SearchProvider, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "bing" => Ok(SearchProvider::Bing),
        "metaso" => Ok(SearchProvider::Metaso),
        "bocha" => Ok(SearchProvider::Bocha),
        "baidu" => Ok(SearchProvider::Baidu),
        "tavily" => Ok(SearchProvider::Tavily),
        other => Err(format!("不支持的搜索源: {other}")),
    }
}

fn resolve_saved_search_key(provider: SearchProvider) -> Result<Option<String>, String> {
    for name in provider.env_key_names() {
        if let Ok(value) = std::env::var(name) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Ok(Some(trimmed.to_string()));
            }
        }
    }
    let mut prefs = UserPrefs::load();
    prefs.refresh_credential_states_with_store(&SystemCredentialStore::new());
    let Some(credential) = prefs.search.credentials.get(&provider) else {
        return Ok(None);
    };
    let Some(reference) = &credential.credential_ref else {
        return Ok(None);
    };
    SystemCredentialStore::new()
        .get(reference)
        .map_err(|error| error.user_message())
        .map(|value| {
            value
                .map(|key| key.trim().to_string())
                .filter(|key| !key.is_empty())
        })
}

#[tauri::command]
pub async fn test_search_provider(
    provider: String,
    api_key: Option<String>,
) -> Result<String, String> {
    let provider = parse_search_provider(&provider)?;
    if provider == SearchProvider::Bing {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .map_err(|e| format!("client: {e}"))?;
        return match client
            .get("https://www.bing.com/search")
            .query(&[("q", "pinvou")])
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => Ok("Bing 搜索可用".to_string()),
            Ok(resp) => Err(format!("Bing HTTP {}", resp.status().as_u16())),
            Err(e) => Err(format!("Bing 搜索不可达: {e}")),
        };
    }
    let provided_key = api_key.unwrap_or_default().trim().to_string();
    let key = if provided_key.is_empty() {
        resolve_saved_search_key(provider)?.unwrap_or_default()
    } else {
        provided_key
    };
    if key.trim().is_empty() {
        return Err("请先填写并保存该搜索源的 API Key".to_string());
    }
    Ok("搜索源凭据已配置".to_string())
}

/// 测试连接:GET {base_url}/models(OpenAI 兼容标准端点),验 base_url + key 可达。
#[tauri::command]
pub async fn test_model_connection(
    base_url: String,
    api_key: String,
    model_id: Option<String>,
) -> Result<String, String> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| format!("client: {e}"))?;
    let mut req = client.get(&url);
    let provided_key = api_key.trim().to_string();
    let key = if provided_key.is_empty() {
        resolve_saved_model_key(model_id.as_deref())?.unwrap_or_default()
    } else {
        provided_key
    };
    if !key.trim().is_empty() {
        req = req.bearer_auth(key.trim());
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            Ok(format!("连接成功 (HTTP {})", resp.status().as_u16()))
        }
        Ok(resp) => Err(format!("HTTP {}", resp.status().as_u16())),
        Err(e) => Err(format!("连接失败: {e}")),
    }
}

#[derive(Debug, Deserialize)]
pub struct VoiceTranscriptionRequest {
    /// WAV bytes captured by the WebView.
    pub audio_bytes: Vec<u8>,
}

#[derive(Debug, Serialize)]
pub struct VoiceTranscriptionResponse {
    pub text: String,
    pub source: String,
}

#[derive(Debug, Serialize)]
pub struct VoiceCommandError {
    pub category: String,
    pub stage: String,
    pub message: String,
}

impl VoiceCommandError {
    fn new(category: &str, stage: &str, message: impl Into<String>) -> Self {
        Self {
            category: category.to_string(),
            stage: stage.to_string(),
            message: message.into(),
        }
    }
}

fn local_asr_command_name() -> String {
    crate::os::asr_tool_path().to_string_lossy().into_owned()
}

fn local_asr_model_name() -> String {
    std::env::var("PINVOU3_ASR_MODEL")
        .or_else(|_| std::env::var("PINVOU3_DEEPSPEECH2_MODEL"))
        .unwrap_or_else(|_| "sensevoice-q8".to_string())
}

fn local_asr_language() -> String {
    std::env::var("PINVOU3_ASR_LANG")
        .or_else(|_| std::env::var("PINVOU3_DEEPSPEECH2_LANG"))
        .unwrap_or_else(|_| "zh".to_string())
}

fn local_asr_timeout() -> std::time::Duration {
    let secs = std::env::var("PINVOU3_ASR_TIMEOUT_SECS")
        .or_else(|_| std::env::var("PINVOU3_DEEPSPEECH2_TIMEOUT_SECS"))
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(60);
    std::time::Duration::from_secs(secs)
}

fn voice_temp_wav_path() -> std::path::PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    std::env::temp_dir().join(format!("pinvou3-voice-{}-{stamp}.wav", std::process::id()))
}

#[cfg(windows)]
fn hide_child_console(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_child_console(_command: &mut std::process::Command) {}

struct LocalAsrOutput {
    text: String,
}

fn compact_process_output(stdout: &str, stderr: &str) -> String {
    let joined = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if joined.chars().count() <= 2000 {
        return joined;
    }
    let tail = joined
        .chars()
        .rev()
        .take(2000)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("...{tail}")
}

fn parse_local_asr_text(stdout: &str, stderr: &str) -> Option<String> {
    let combined = format!("{stdout}\n{stderr}");
    let result_prefixes = [
        "result:",
        "asr result:",
        "recognition result:",
        "text:",
        "output:",
    ];

    for line in combined.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        for prefix in result_prefixes {
            if lower.starts_with(prefix) {
                let text = line[prefix.len()..].trim();
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
        if (line.starts_with("['") && line.ends_with("']"))
            || (line.starts_with("[\"") && line.ends_with("\"]"))
        {
            let text = line
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim_matches('\'')
                .trim_matches('"')
                .trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
        if lower.contains("error")
            || lower.contains("warning")
            || lower.contains("paddlespeech")
            || lower.contains("sensevoice")
            || lower.contains("funasr")
            || lower.contains("gguf")
            || lower.contains("python")
            || lower.contains("download")
            || lower.starts_with('[')
        {
            continue;
        }
        if line
            .chars()
            .any(|ch| ch.is_alphabetic() || ('\u{4e00}'..='\u{9fff}').contains(&ch))
        {
            return Some(line.to_string());
        }
    }
    None
}

fn run_local_asr_cli(wav_path: &std::path::Path) -> Result<LocalAsrOutput, VoiceCommandError> {
    use std::io::Read;
    use std::process::Stdio;

    let executable = local_asr_command_name();
    let model = local_asr_model_name();
    let language = local_asr_language();
    let timeout = local_asr_timeout();

    let mut command = std::process::Command::new(&executable);
    command
        .arg("asr")
        .arg("--model")
        .arg(&model)
        .arg("--lang")
        .arg(&language)
        .arg("--input")
        .arg(wav_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_child_console(&mut command);

    let mut child = command.spawn().map_err(|e| {
        let message = if e.kind() == std::io::ErrorKind::NotFound {
            crate::os::asr_missing_message().to_string()
        } else {
            format!("Failed to start local SenseVoice/FunASR ASR: {e}")
        };
        VoiceCommandError::new("recognition_failed", "transcribing", message)
    })?;

    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(VoiceCommandError::new(
                        "recognition_failed",
                        "transcribing",
                        format!(
                            "Local SenseVoice/FunASR ASR timed out after {} seconds. Check that the q8 model is bundled and the runtime works offline.",
                            timeout.as_secs()
                        ),
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(VoiceCommandError::new(
                    "recognition_failed",
                    "transcribing",
                    format!("Failed while waiting for local SenseVoice/FunASR ASR: {e}"),
                ));
            }
        }
    };

    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }

    if !status.success() {
        return Err(VoiceCommandError::new(
            "recognition_failed",
            "transcribing",
            format!(
                "Local SenseVoice/FunASR ASR failed (exit {}): {}",
                status
                    .code()
                    .map_or_else(|| "unknown".to_string(), |code| code.to_string()),
                compact_process_output(&stdout, &stderr)
            ),
        ));
    }

    let text = parse_local_asr_text(&stdout, &stderr).ok_or_else(|| {
        VoiceCommandError::new(
            "empty_result",
            "transcribing",
            format!(
                "Local SenseVoice/FunASR ASR returned no usable text: {}",
                compact_process_output(&stdout, &stderr)
            ),
        )
    })?;

    Ok(LocalAsrOutput { text })
}

/// Transcribe a short one-shot voice capture from the desktop WebView using
/// local SenseVoice/FunASR ASR.
#[tauri::command]
pub async fn transcribe_voice_audio(
    request: VoiceTranscriptionRequest,
) -> Result<VoiceTranscriptionResponse, VoiceCommandError> {
    if request.audio_bytes.len() < 44 {
        return Err(VoiceCommandError::new(
            "recording_failed",
            "recording",
            "Recorded audio is empty or invalid.",
        ));
    }

    let wav_path = voice_temp_wav_path();
    let audio_bytes = request.audio_bytes;
    let asr_output = tokio::task::spawn_blocking(move || {
        std::fs::write(&wav_path, &audio_bytes).map_err(|e| {
            VoiceCommandError::new(
                "recording_failed",
                "recording",
                format!("Failed to write temporary voice audio: {e}"),
            )
        })?;
        // 优先用内置 SenseVoice 引擎（转码+识别+清洗全在 Rust，无需 shim/环境变量）；
        // 引擎或模型未就绪时回退原 CLI 路径（PINVOU3_ASR_CMD / pinvou-asr）。
        let result = if crate::os::asr_bundled_runtime_status().is_none()
            && crate::voice_asr::engine_path().is_file()
            && crate::voice_asr::model_path().is_file()
        {
            crate::voice_asr::transcribe(&wav_path)
                .map(|text| LocalAsrOutput { text })
                .map_err(|e| VoiceCommandError::new("recognition_failed", "transcribing", e))
        } else {
            // macOS 特判:引擎就位但模型未下载(用户刚装好的正常窗口)。此前直接走
            // run_local_asr_cli,它用 pinvou-asr shim 协议(asr --model ... --input)spawn
            // sense-voice 引擎本体 —— 引擎不认这个参数 → 非零退出 → 用户看到困惑的 "ASR
            // failed (exit N)"。改为返回明确的"模型未下载"错误。
            #[cfg(target_os = "macos")]
            if crate::os::asr_bundled_runtime_status().is_none()
                && crate::voice_asr::engine_path().is_file()
                && !crate::voice_asr::model_path().is_file()
            {
                Err(VoiceCommandError::new(
                    "recognition_failed",
                    "transcribing",
                    "本地语音识别模型未下载。请在设置页下载语音模型后重试。".to_string(),
                ))
            } else {
                run_local_asr_cli(&wav_path)
            }
            #[cfg(not(target_os = "macos"))]
            {
                run_local_asr_cli(&wav_path)
            }
        };
        let _ = std::fs::remove_file(&wav_path);
        result
    })
    .await
    .map_err(|e| {
        VoiceCommandError::new(
            "recognition_failed",
            "transcribing",
            format!("Local SenseVoice/FunASR ASR task failed: {e}"),
        )
    })??;

    Ok(VoiceTranscriptionResponse {
        text: asr_output.text,
        source: "pinvou-webview-sensevoice-local".to_string(),
    })
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
    prepare_prefs_for_save(prefs)?
        .save()
        .map_err(|e| format!("save settings failed: {e:?}"))
}

/// 保存设置后立即重启应用（模型/后端切换后需要重启才能生效）。
#[tauri::command]
pub async fn save_settings_and_restart(
    prefs: UserPrefs,
    app: tauri::AppHandle,
) -> Result<(), String> {
    prepare_prefs_for_save(prefs)?
        .save()
        .map_err(|e| format!("save settings failed: {e:?}"))?;
    eprintln!("[pinvou3-app] settings saved, restarting app...");
    app.restart();
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
    let snapshot = crate::monitor::sample_all(
        &monitor,
        &crate::monitor::vllm_base_url(),
        crate::monitor::vllm_configured_model(),
    )
    .await;
    Ok(snapshot)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg(target_os = "linux")]
pub struct DiscoverLocalVllmRequest {
    pub current_base_url: Option<String>,
    pub saved_base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg(target_os = "linux")]
pub struct LocalVllmCandidate {
    pub base_url: String,
    pub status: VllmStatus,
    pub model: Option<String>,
    pub max_model_len: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg(target_os = "linux")]
pub struct LocalVllmDiscovery {
    pub candidates: Vec<LocalVllmCandidate>,
}

/// 手动探测本机 vLLM。只探小白名单候选地址；不做端口扫描,不探局域网。
#[cfg(target_os = "linux")]
#[tauri::command]
pub async fn discover_local_vllm(
    request: Option<DiscoverLocalVllmRequest>,
) -> Result<LocalVllmDiscovery, String> {
    let mut urls = Vec::new();
    if let Some(req) = request {
        push_local_vllm_candidate(&mut urls, req.current_base_url.as_deref());
        push_local_vllm_candidate(&mut urls, req.saved_base_url.as_deref());
    }
    for port in [8000u16, 8001, 8002] {
        push_local_vllm_candidate(&mut urls, Some(&format!("http://127.0.0.1:{port}/v1")));
    }

    let mut candidates = Vec::new();
    for base_url in urls {
        if let Some(snapshot) = crate::monitor::vllm_snapshot(&base_url, None).await {
            candidates.push(LocalVllmCandidate {
                base_url: snapshot.upstream,
                status: snapshot.status,
                model: snapshot.model,
                max_model_len: snapshot.max_model_len,
            });
        }
    }
    Ok(LocalVllmDiscovery { candidates })
}

#[cfg(target_os = "linux")]
fn push_local_vllm_candidate(out: &mut Vec<String>, raw: Option<&str>) {
    let Some(raw) = raw else {
        return;
    };
    let Some(url) = normalize_local_vllm_base_url(raw) else {
        return;
    };
    if !out.iter().any(|existing| existing == &url) {
        out.push(url);
    }
}

#[cfg(target_os = "linux")]
fn normalize_local_vllm_base_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let rest = trimmed.strip_prefix("http://")?;
    let host_port = rest.split('/').next()?;
    let (host, port) = host_port.rsplit_once(':')?;
    if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
        return None;
    }
    let port: u16 = port.parse().ok()?;
    if !matches!(port, 8000 | 8001 | 8002) {
        return None;
    }
    Some(format!("http://{host}:{port}/v1"))
}

/// ChatRoom 顶部 live dot 简版指示：vLLM 是否在线。
#[derive(Debug, Clone, Serialize)]
pub struct BackendStatus {
    pub vllm_online: bool,
    pub last_check_ms: u64,
    /// vLLM 真实上下文窗口（前端 token 进度数据的分母）。
    /// 随 live-dot 轮询下发，监控页未打开时也能保持准确。
    pub max_model_len: Option<u32>,
}

#[tauri::command]
pub async fn get_backend_status(
    _monitor: State<'_, MonitorState>,
) -> Result<BackendStatus, String> {
    // Lightweight: 只 probe 当前 active model,不跑 nvidia-smi / RAM 采样。
    let vllm = crate::monitor::active_model_snapshot().await;
    let vllm_online = vllm.as_ref().is_some_and(|v| {
        v.health_status == "verified" && matches!(v.status, VllmStatus::Ready | VllmStatus::Busy)
    });
    let max_model_len = vllm.as_ref().and_then(|v| v.max_model_len);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    Ok(BackendStatus {
        vllm_online,
        last_check_ms: now_ms,
        max_model_len,
    })
}

// ===================== 阶段 C: 多对话历史 =====================

/// 列出所有 session 元数据，按 updated_at 倒序。前端历史面板渲染用。
/// 返回 SessionMetadata 数组（id/title/时间/token/model/workspace 等字段）。
/// [2026-06-04 白浪:chat 与工作流彻底分开] 过滤工作流宿主 session(绑定带 project_dir
/// 即是,bindings 开机回灌持久化)——它们仅作 SubAgent 运行时,不进 chat 侧栏。
#[tauri::command]
pub async fn list_sessions(store: State<'_, SessionStore>) -> Result<Vec<SessionListItem>, String> {
    let mut metas = store.list().map_err(|e| format!("list_sessions: {e:?}"))?;
    metas.retain(|m| {
        matches!(store.session_kind(&m.id), Ok(SessionKind::Chat))
            && !store.is_hidden(&m.id)
            && store
                .active_skill(&m.id)
                .map_or(true, |b| b.project_dir.is_none())
    });
    Ok(metas
        .into_iter()
        .map(|metadata| SessionListItem {
            pinned: store.is_pinned(&metadata.id),
            pinned_at: store.pinned_at(&metadata.id),
            metadata,
        })
        .collect())
}

/// 列出已从左侧任务列表收起的 session（含收起的定时运行会话）。前端设置页渲染用。
#[tauri::command]
pub async fn list_archived_sessions(
    store: State<'_, SessionStore>,
) -> Result<Vec<HiddenSessionListItem>, String> {
    let mut metas = store
        .list()
        .map_err(|e| format!("list_archived_sessions: {e:?}"))?;
    metas.extend(
        store
            .list_scheduled()
            .map_err(|e| format!("list_archived_sessions: {e:?}"))?,
    );
    metas.retain(|m| {
        store.is_hidden(&m.id)
            && store
                .active_skill(&m.id)
                .map_or(true, |b| b.project_dir.is_none())
    });
    metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(metas
        .into_iter()
        .map(|metadata| {
            let hidden_at = store.hidden_at(&metadata.id);
            HiddenSessionListItem {
                archived_at: hidden_at.clone(),
                hidden_at,
                metadata,
            }
        })
        .collect())
}

/// 新建空 session 并设为 active。返回创建的 SessionMetadata。
/// 引擎层的 session 状态切换由 chat() 下次发消息时自然处理（暂不发 SyncSession）。
#[tauri::command]
pub async fn create_session(
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<SessionMetadata, String> {
    let (model, model_id) = pool.default_model_for_new_session();
    let workspace = pool.bridge.workspace.clone();
    let session = store
        .create_new(model, model_id, workspace)
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
    set_active: Option<bool>,
    store: State<'_, SessionStore>,
) -> Result<SavedSession, String> {
    let session = store
        .load(&id)
        .map_err(|e| format!("load_session({id}): {e:?}"))?;
    if set_active.unwrap_or(true) {
        store.set_active(Some(id.clone()));
    }
    // 多 session 并发:切换不再 SyncSession 替换全局引擎(那是旧单引擎模型)。该 session
    // 有自己独立的 engine(已起则持有自己的上下文、还在跑就继续跑;未起则下次 chat 时
    // lazy spawn 并注水这里返回的 messages)。本命令只切 active 指针 + 返回 messages 给前端渲染。
    Ok(session)
}

/// 删除 session（含 artifacts 目录）。按 SessionKind 分发：定时运行会话联动
/// 删除该次 Session、Run 与底座 Task（任务定义、共享工作间和其他运行保留）。
#[tauri::command]
pub async fn delete_session(
    id: String,
    app: AppHandle,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    match store
        .session_kind(&id)
        .map_err(|e| format!("delete_session({id}): {e:?}"))?
    {
        SessionKind::Chat => {
            // 先回收该 session 的 engine(cancel 在跑的 turn + shutdown + abort forwarder),
            // 再删盘上数据,避免僵尸 engine 继续往已删 session 写产物。
            pool.evict(&id).await;
            let result = store
                .delete(&id)
                .map_err(|e| format!("delete_session({id}): {e:?}"));
            if result.is_ok() {
                pool.forget_session(&id);
            }
            result
        }
        SessionKind::ScheduledRun => {
            let scheduled = app
                .try_state::<crate::scheduled_tasks::ScheduledTaskState>()
                .ok_or_else(|| "Scheduled task runtime is unavailable".to_string())?;
            scheduled.delete_run_for_session(&id).await
        }
    }
}

/// 重命名 session 标题。普通会话与定时运行会话共用 Session 元数据。
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

/// 设置历史对话置顶状态。普通会话与定时运行会话共用置顶表。
#[tauri::command]
pub async fn set_session_pinned(
    id: String,
    pinned: bool,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    // 先 load 一次确认 session 存在,避免置顶表残留无效 id。
    store
        .load(&id)
        .map_err(|e| format!("set_session_pinned({id}): {e:?}"))?;
    store.set_pinned(&id, pinned);
    Ok(())
}

/// 设置 session 是否从左侧任务列表收起。普通会话与定时运行会话共用收起表。
#[tauri::command]
pub async fn set_session_archived(
    id: String,
    archived: bool,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    // 先 load 一次确认 session 存在,避免收起表残留无效 id。
    store
        .load(&id)
        .map_err(|e| format!("set_session_archived({id}): {e:?}"))?;
    store.set_hidden(&id, archived);
    Ok(())
}

/// 取当前 active session id（前端启动时高亮历史面板用）。
#[tauri::command]
pub async fn get_active_session(store: State<'_, SessionStore>) -> Result<Option<String>, String> {
    Ok(store.active_id())
}

/// 落盘普通 chat session 的 messages 数组。前端是普通 chat 的 source of truth；
/// scheduled-run transcript 由 Engine `SessionUpdated` 独占持久化，拒绝 UI 覆盖。
#[tauri::command]
pub async fn save_session_messages(
    id: String,
    messages: Vec<Message>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    ensure_chat_session(&store, &id, "save_session_messages")?;
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
    ensure_chat_session(&store, &id, "save_session_artifacts")?;
    store
        .update_artifacts(&id, paths)
        .map_err(|e| format!("save_session_artifacts({id}): {e:?}"))
}

/// 扫描 session workspace 目录,返回实际存在的产物文件绝对路径(过滤隐藏/临时文件)。
/// 前端切换 session 时用它对账 —— 让产物面板以**磁盘真相**为准,不受跟踪遗漏 /
/// app 中途重启(内存跟踪丢失)影响。过滤规则与 file_watcher::should_skip 对齐。
#[tauri::command]
pub async fn list_workspace_files(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<Vec<String>, String> {
    list_workspace_files_for_session(&session_id, &store)
}

fn list_workspace_files_for_session(
    session_id: &str,
    store: &SessionStore,
) -> Result<Vec<String>, String> {
    let execution_workspace = store
        .execution_workspace(session_id)
        .map_err(|error| format!("resolve execution workspace for {session_id}: {error:#}"))?;
    let mut out = Vec::new();
    for dir in [
        execution_workspace,
        crate::bridge::paths::session_artifacts_dir(session_id),
    ] {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if name.is_empty()
                    || name.starts_with('.')
                    || name.starts_with("~$")
                    || name.ends_with('~')
                    || name.ends_with(".swp")
                    || name.ends_with(".swo")
                    || name.ends_with(".tmp")
                    || name.ends_with(".bak")
                {
                    continue;
                }
                out.push(path.to_string_lossy().to_string());
            }
        }
    }
    out.sort();
    Ok(out)
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

/// Return the app-owned snapshot of shell jobs for one session. Polling this
/// command does not touch Engine lifecycle or conversation state.
#[tauri::command]
pub async fn list_shell_tasks(
    session_id: String,
    pool: State<'_, EnginePool>,
) -> Result<Vec<deepseek_tui::tools::shell::ShellJobSnapshot>, String> {
    pool.list_shell_tasks(&session_id)
        .await
        .map_err(|error| format!("list_shell_tasks({session_id}): {error:#}"))
}

/// Cancel a detached or foreground-backed shell by its stable task id.
#[tauri::command]
pub async fn cancel_shell_task(
    session_id: String,
    task_id: String,
    pool: State<'_, EnginePool>,
) -> Result<deepseek_tui::tools::shell::ShellResult, String> {
    pool.cancel_shell_task(&session_id, &task_id)
        .await
        .map_err(|error| format!("cancel_shell_task({session_id}, {task_id}): {error:#}"))
}

/// 落盘连接器开关 → 联动技能 → 重算完整禁用工具列表 → 广播到所有在跑引擎。
///
/// 抽出 `set_disabled_connectors` 的 4 步副作用序列,让 web 远程指令管理器可在
/// 非 Tauri 命令上下文复用。`app` 为 `None` 时(无 AppHandle)退化为只读
/// `disabled_tool_names()`,不补 `kb_search` gate(远程指令调用方已自行管控)。
pub async fn apply_disabled_connectors(
    app: Option<&AppHandle>,
    pool: &EnginePool,
    connector_ids: Vec<String>,
) -> Result<(), String> {
    let app_clone = app.cloned();
    let disallowed = tokio::task::spawn_blocking(move || -> Result<Vec<String>, String> {
        crate::bridge::marketplace::save_disabled_connectors(&connector_ids);
        crate::bridge::skill_marketplace::refresh_disabled_skills();
        Ok(match &app_clone {
            Some(a) => compute_disallowed_tools(a),
            None => crate::bridge::marketplace::disabled_tool_names(),
        })
    })
    .await
    .map_err(|e| format!("apply_disabled_connectors join: {e}"))??;
    pool.set_disallowed_all(disallowed).await;
    Ok(())
}

/// 计算当前应「对模型隐藏」的工具全名**完整列表**(小写)。
///
/// 因 `EnginePool::set_disallowed_all` 是**全量替换** `config.disallowed_tools`,任何调用方
/// 都必须传完整列表,不能传增量。组成 = 市场连接器开关禁用的工具名 +(知识库不可用时)`kb_search`。
/// 知识库"可用" = 有已入库内容 **且** embedding 模型已就绪(semantic_ready)。embedding 模型按需
/// 下载,没装时知识库走完全门控 → kb_search 进列表 → 模型目录里看不到 → AI 不再宣称能本地检索;
/// 库删光文件后同理。KnowledgeService state 取不到时保守隐藏(宁可少功能不误宣传)。
pub fn compute_disallowed_tools(app: &AppHandle) -> Vec<String> {
    let mut tools = crate::bridge::marketplace::disabled_tool_names();
    let kb_usable = app
        .try_state::<KnowledgeService>()
        .map(|s| s.has_indexed_content() && s.semantic_ready())
        .unwrap_or(false);
    if !kb_usable {
        tools.push("kb_search".to_string());
    }
    tools
}

/// pinvou3 工具开关(全局持久):设置当前被关掉的连接器(connector_ids = 市场工具 id)。
/// 落盘 → 推算成模型可见工具全名广播给所有在跑引擎 → 隐藏这些工具。空 = 全开。
/// 持久:用户关一次,所有新对话/新窗口都继承,直到手动开回。
///
/// 同时 emit `remote_control:tools_changed`,让桌面 chip 即时刷新;并通过 manager
/// 把 tools_changed 推给正在远控的 mobile 端(若 session 正在被远控),避免 mobile UI 陈旧。
#[tauri::command]
pub async fn set_disabled_connectors(
    connector_ids: Vec<String>,
    app: AppHandle,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    apply_disabled_connectors(Some(&app), &pool, connector_ids).await?;
    // emit 失败不应让命令失败:disabled_connectors.json 已落盘,engine pool 已生效,
    // emit 失败只影响 UI 即时刷新(下次 reload 自愈)。与 kb_mount_changed emit 同步策略。
    let _ = app.emit("remote_control:tools_changed", ());
    // 推给正在远控的 mobile(若该 session 正在被远控)。
    if let Some(manager) = app.try_state::<crate::remote_control::RemoteControlManager>() {
        if let Some(sid) = manager.current_session_id() {
            manager.broadcast_to_mobile(&sid, "tools_changed", serde_json::json!({}));
        }
    }
    Ok(())
}

/// pinvou3 工具开关:读全局被禁用的连接器 id 列表(前端启动时加载,初始化开关状态)。
#[tauri::command]
pub async fn get_disabled_connectors() -> Result<Vec<String>, String> {
    Ok(crate::bridge::marketplace::load_disabled_connectors())
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryProfileState {
    pub profile: crate::memory::MemoryProfile,
    pub runtime: Option<crate::memory::RuntimeMemorySnapshot>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryWriteState<T> {
    pub value: T,
    pub runtime: Option<crate::memory::RuntimeMemorySnapshot>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryOverviewState {
    pub profile: crate::memory::MemoryProfile,
    pub preferences: Vec<crate::memory::PreferenceFile>,
    pub work_context: Vec<crate::memory::WorkContextFile>,
    pub current_focus: Vec<crate::memory::TimedMemoryItem>,
    pub recent_activity: Vec<crate::memory::TimedMemoryItem>,
    pub recent_work: Vec<crate::memory::RecentWorkItem>,
    pub pending: Vec<crate::memory::PendingMemoryItem>,
    pub never: Vec<crate::memory::NeverMemoryItem>,
    pub runtime: Option<crate::memory::RuntimeMemorySnapshot>,
    pub snapshot_path: String,
}

fn resolve_memory_session_id(session_id: Option<String>, store: &SessionStore) -> Option<String> {
    session_id.or_else(|| store.active_id())
}

fn emit_memory_write_events(
    app: &AppHandle,
    session_id: &str,
    events: &[crate::memory::MemoryWriteEvent],
) {
    if events.is_empty() {
        return;
    }
    let _ = app.emit(
        "chat:memory_write",
        serde_json::json!({
            "session_id": session_id,
            "events": events,
        }),
    );
}

fn emit_memory_snapshot(
    app: &AppHandle,
    session_id: &str,
    snapshot: &crate::memory::RuntimeMemorySnapshot,
) {
    let _ = app.emit(
        "chat:memory",
        serde_json::json!({
            "session_id": session_id,
            "items": &snapshot.items,
            "runtime_path": &snapshot.runtime_path,
        }),
    );
}

fn refresh_memory_runtime_for_command(
    session_id: Option<String>,
    store: &SessionStore,
    app: &AppHandle,
) -> Result<Option<crate::memory::RuntimeMemorySnapshot>, String> {
    match resolve_memory_session_id(session_id, store) {
        Some(sid) => {
            let snapshot = crate::memory::runtime_snapshot(&sid)
                .map_err(|e| format!("render runtime memory: {e}"))?;
            emit_memory_snapshot(app, &sid, &snapshot);
            Ok(Some(snapshot))
        }
        None => Ok(None),
    }
}

#[tauri::command]
pub async fn get_memory_profile(
    session_id: Option<String>,
    store: State<'_, SessionStore>,
) -> Result<MemoryProfileState, String> {
    let profile = crate::memory::load_profile().map_err(|e| format!("load profile: {e}"))?;
    let runtime = match resolve_memory_session_id(session_id, &store) {
        Some(sid) => Some(
            crate::memory::runtime_snapshot(&sid)
                .map_err(|e| format!("render runtime memory: {e}"))?,
        ),
        None => None,
    };
    Ok(MemoryProfileState { profile, runtime })
}

#[tauri::command]
pub async fn update_memory_profile(
    patch: crate::memory::ProfilePatch,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryProfileState, String> {
    let profile =
        crate::memory::update_profile(patch).map_err(|e| format!("update profile: {e}"))?;
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryProfileState { profile, runtime })
}

#[tauri::command]
pub async fn clear_memory_profile(
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryProfileState, String> {
    let profile = crate::memory::clear_profile().map_err(|e| format!("clear profile: {e}"))?;
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryProfileState { profile, runtime })
}

#[tauri::command]
pub async fn get_memory_overview(
    session_id: Option<String>,
    store: State<'_, SessionStore>,
) -> Result<MemoryOverviewState, String> {
    let profile = crate::memory::load_profile().map_err(|e| format!("load profile: {e}"))?;
    let preferences =
        crate::memory::list_preferences().map_err(|e| format!("load preferences: {e}"))?;
    let work_context =
        crate::memory::load_work_context().map_err(|e| format!("load work context: {e}"))?;
    let current_focus =
        crate::memory::load_current_focus().map_err(|e| format!("load current focus: {e}"))?;
    let recent_activity =
        crate::memory::load_recent_activity().map_err(|e| format!("load recent activity: {e}"))?;
    let recent_work =
        crate::memory::load_recent_work().map_err(|e| format!("load recent work: {e}"))?;
    let pending =
        crate::memory::load_pending_memory().map_err(|e| format!("load pending memory: {e}"))?;
    let never =
        crate::memory::load_never_memory().map_err(|e| format!("load never memory: {e}"))?;
    let runtime = match resolve_memory_session_id(session_id, &store) {
        Some(sid) => Some(
            crate::memory::runtime_snapshot(&sid)
                .map_err(|e| format!("render runtime memory: {e}"))?,
        ),
        None => None,
    };
    let snapshot_path = crate::memory::write_memory_snapshot_document(
        &profile,
        &preferences,
        &work_context,
        &current_focus,
        &recent_activity,
        &recent_work,
        &pending,
        &never,
        runtime.as_ref(),
    )
    .map_err(|e| format!("write memory snapshot: {e}"))?
    .display()
    .to_string();
    Ok(MemoryOverviewState {
        profile,
        preferences,
        work_context,
        current_focus,
        recent_activity,
        recent_work,
        pending,
        never,
        runtime,
        snapshot_path,
    })
}

#[tauri::command]
pub async fn list_pending_memory() -> Result<Vec<crate::memory::PendingMemoryItem>, String> {
    crate::memory::load_pending_memory().map_err(|e| format!("load pending memory: {e}"))
}

#[tauri::command]
pub async fn suggest_memory(
    suggestion: crate::memory::MemorySuggestion,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<crate::memory::PendingMemoryItem>, String> {
    let item = crate::memory::enqueue_memory_candidate(suggestion)
        .map_err(|e| format!("suggest memory: {e}"))?;
    if let Some(sid) = resolve_memory_session_id(session_id.clone(), &store) {
        emit_memory_write_events(
            &app,
            &sid,
            &[crate::memory::MemoryWriteEvent {
                kind: item.kind.clone(),
                action: "pending".to_string(),
                id: item.id.clone(),
                text: item.content.clone(),
            }],
        );
    }
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: item,
        runtime,
    })
}

#[tauri::command]
pub async fn confirm_pending_memory(
    id: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<Option<crate::memory::MemoryWriteEvent>>, String> {
    let event = crate::memory::confirm_pending_memory(&id)
        .map_err(|e| format!("confirm pending memory: {e}"))?;
    if let (Some(sid), Some(event)) = (
        resolve_memory_session_id(session_id.clone(), &store),
        event.as_ref(),
    ) {
        emit_memory_write_events(&app, &sid, std::slice::from_ref(event));
    }
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: event,
        runtime,
    })
}

#[tauri::command]
pub async fn ignore_pending_memory(
    id: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<Option<crate::memory::MemoryWriteEvent>>, String> {
    let event = crate::memory::ignore_pending_memory(&id)
        .map_err(|e| format!("ignore pending memory: {e}"))?;
    if let (Some(sid), Some(event)) = (
        resolve_memory_session_id(session_id.clone(), &store),
        event.as_ref(),
    ) {
        emit_memory_write_events(&app, &sid, std::slice::from_ref(event));
    }
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: event,
        runtime,
    })
}

#[tauri::command]
pub async fn never_pending_memory(
    id: String,
    reason: Option<String>,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<Option<crate::memory::MemoryWriteEvent>>, String> {
    let event = crate::memory::never_pending_memory(&id, reason)
        .map_err(|e| format!("never pending memory: {e}"))?;
    if let (Some(sid), Some(event)) = (
        resolve_memory_session_id(session_id.clone(), &store),
        event.as_ref(),
    ) {
        emit_memory_write_events(&app, &sid, std::slice::from_ref(event));
    }
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: event,
        runtime,
    })
}

#[tauri::command]
pub async fn list_recent_work_memory() -> Result<Vec<crate::memory::RecentWorkItem>, String> {
    crate::memory::load_recent_work().map_err(|e| format!("load recent work memory: {e}"))
}

#[tauri::command]
pub async fn upsert_recent_work_memory(
    patch: crate::memory::RecentWorkPatch,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<crate::memory::RecentWorkItem>, String> {
    let item =
        crate::memory::upsert_recent_work(patch).map_err(|e| format!("upsert recent work: {e}"))?;
    if let Some(sid) = resolve_memory_session_id(session_id.clone(), &store) {
        emit_memory_write_events(
            &app,
            &sid,
            &[crate::memory::MemoryWriteEvent {
                kind: "recent_work".to_string(),
                action: "remembered".to_string(),
                id: item.id.clone(),
                text: item.title.clone(),
            }],
        );
    }
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: item,
        runtime,
    })
}

#[tauri::command]
pub async fn archive_recent_work_memory(
    id: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<bool>, String> {
    let changed =
        crate::memory::archive_recent_work(&id).map_err(|e| format!("archive recent work: {e}"))?;
    if changed {
        if let Some(sid) = resolve_memory_session_id(session_id.clone(), &store) {
            emit_memory_write_events(
                &app,
                &sid,
                &[crate::memory::MemoryWriteEvent {
                    kind: "recent_work".to_string(),
                    action: "archived".to_string(),
                    id,
                    text: "近期工作已归档".to_string(),
                }],
            );
        }
    }
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: changed,
        runtime,
    })
}

#[tauri::command]
pub async fn delete_memory_preference(
    id: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<bool>, String> {
    let changed =
        crate::memory::delete_preference(&id).map_err(|e| format!("delete preference: {e}"))?;
    if changed {
        if let Some(sid) = resolve_memory_session_id(session_id.clone(), &store) {
            emit_memory_write_events(
                &app,
                &sid,
                &[crate::memory::MemoryWriteEvent {
                    kind: "preference".to_string(),
                    action: "deleted".to_string(),
                    id,
                    text: "偏好已删除".to_string(),
                }],
            );
        }
    }
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: changed,
        runtime,
    })
}

#[tauri::command]
pub async fn update_memory_preference(
    id: String,
    patch: crate::memory::MemoryTextPatch,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<Option<crate::memory::PreferenceFile>>, String> {
    let item = crate::memory::update_preference(&id, patch)
        .map_err(|e| format!("update preference: {e}"))?;
    if let (Some(sid), Some(item)) = (
        resolve_memory_session_id(session_id.clone(), &store),
        item.as_ref(),
    ) {
        emit_memory_write_events(
            &app,
            &sid,
            &[crate::memory::MemoryWriteEvent {
                kind: "preference".to_string(),
                action: "remembered".to_string(),
                id: item.id.clone(),
                text: item.text.clone(),
            }],
        );
    }
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: item,
        runtime,
    })
}

#[tauri::command]
pub async fn update_work_context_memory(
    id: String,
    patch: crate::memory::MemoryTextPatch,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<Option<crate::memory::WorkContextFile>>, String> {
    let item = crate::memory::update_work_context(&id, patch)
        .map_err(|e| format!("update work context: {e}"))?;
    if let (Some(sid), Some(item)) = (
        resolve_memory_session_id(session_id.clone(), &store),
        item.as_ref(),
    ) {
        emit_memory_write_events(
            &app,
            &sid,
            &[crate::memory::MemoryWriteEvent {
                kind: "work_context".to_string(),
                action: "remembered".to_string(),
                id: item.id.clone(),
                text: item.text.clone(),
            }],
        );
    }
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: item,
        runtime,
    })
}

#[tauri::command]
pub async fn delete_work_context_memory(
    id: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<bool>, String> {
    let changed =
        crate::memory::delete_work_context(&id).map_err(|e| format!("delete work context: {e}"))?;
    if changed {
        if let Some(sid) = resolve_memory_session_id(session_id.clone(), &store) {
            emit_memory_write_events(
                &app,
                &sid,
                &[crate::memory::MemoryWriteEvent {
                    kind: "work_context".to_string(),
                    action: "deleted".to_string(),
                    id,
                    text: "工作背景已删除".to_string(),
                }],
            );
        }
    }
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: changed,
        runtime,
    })
}

#[tauri::command]
pub async fn update_timed_memory(
    kind: String,
    id: String,
    patch: crate::memory::MemoryTextPatch,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<Option<crate::memory::TimedMemoryItem>>, String> {
    let item = crate::memory::update_timed_memory(&kind, &id, patch)
        .map_err(|e| format!("update timed memory: {e}"))?;
    if let (Some(sid), Some(item)) = (
        resolve_memory_session_id(session_id.clone(), &store),
        item.as_ref(),
    ) {
        emit_memory_write_events(
            &app,
            &sid,
            &[crate::memory::MemoryWriteEvent {
                kind: item.kind.clone(),
                action: "remembered".to_string(),
                id: item.id.clone(),
                text: item.text.clone(),
            }],
        );
    }
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: item,
        runtime,
    })
}

#[tauri::command]
pub async fn delete_timed_memory(
    kind: String,
    id: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
    app: AppHandle,
) -> Result<MemoryWriteState<bool>, String> {
    let changed = crate::memory::delete_timed_memory(&kind, &id)
        .map_err(|e| format!("delete timed memory: {e}"))?;
    if changed {
        if let Some(sid) = resolve_memory_session_id(session_id.clone(), &store) {
            emit_memory_write_events(
                &app,
                &sid,
                &[crate::memory::MemoryWriteEvent {
                    kind,
                    action: "deleted".to_string(),
                    id,
                    text: "记忆已删除".to_string(),
                }],
            );
        }
    }
    let runtime = refresh_memory_runtime_for_command(session_id, &store, &app)?;
    Ok(MemoryWriteState {
        value: changed,
        runtime,
    })
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
    // 定时会话不走 ensure_chat_session:编辑重发与继续追问同路,EnginePool 内部
    // 按 scheduled_profile 做 turn gate;会话管理类命令(删除/改名/归档)仍然拒绝。
    pool.edit_last_turn(&sid, new_message)
        .await
        .map_err(|e| format!("edit_last_turn: {e:?}"))
}

// ===================== 阶段 C: 产物面板 =====================

/// 产物文件元数据。前端右栏 list 用。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ArtifactInfo {
    /// 文件大小（字节）
    pub size: u64,
    /// 文件 mime-ish 分类：md / html / image / pdf / text / binary
    pub kind: String,
    /// 文件存在标记（前端跟踪的路径可能被外部删了）
    pub exists: bool,
    /// 最后修改时间（epoch 秒）。取不到给 0。前端列表「最后修改」/ 详情「修改时间」用。
    pub modified: i64,
}

/// 读 artifact 文件的纯文本（md/json/txt 等）。文件不存在或不是文本 → 报错。
/// 路径必须在用户家目录下（防 ../../../etc/passwd 之类逃逸）。
#[tauri::command]
pub async fn read_artifact_text(path: String) -> Result<String, String> {
    read_artifact_text_impl(&path)
}

pub(crate) fn read_artifact_text_impl(path: &str) -> Result<String, String> {
    let p = validate_user_path(path)?;
    std::fs::read_to_string(&p).map_err(|e| format!("read_artifact_text({}): {e}", p.display()))
}

const MAX_EDITABLE_MARKDOWN_BYTES: usize = 10 * 1024 * 1024;

/// 写回 Markdown artifact。只允许覆盖已存在的 .md/.markdown 文件。
#[tauri::command]
pub async fn write_artifact_text(path: String, content: String) -> Result<(), String> {
    write_artifact_text_impl(&path, &content)
}

pub(crate) fn write_artifact_text_impl(path: &str, content: &str) -> Result<(), String> {
    let p = validate_user_path(path)?;
    if !p.is_file() {
        return Err(format!("not a file: {}", p.display()));
    }
    ensure_editable_artifact_path(&p)?;

    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "md" && ext != "markdown" {
        return Err("only markdown artifacts can be edited".into());
    }

    if content.len() > MAX_EDITABLE_MARKDOWN_BYTES {
        return Err("markdown artifact is too large to save".into());
    }

    atomic_write_utf8(&p, content).map_err(|e| format!("write_artifact_text({}): {e}", p.display()))
}

fn ensure_editable_artifact_path(path: &std::path::Path) -> Result<(), String> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|e| format!("resolve artifact path({}): {e}", path.display()))?;
    let sessions_root = crate::bridge::paths::sessions_root();
    let sessions_root = std::fs::canonicalize(&sessions_root).map_err(|e| {
        format!(
            "markdown artifact is outside session storage: cannot resolve sessions root({}): {e}",
            sessions_root.display()
        )
    })?;
    let rel = canonical
        .strip_prefix(&sessions_root)
        .map_err(|_| "markdown artifact is outside session storage".to_string())?;
    let mut components = rel.components();
    let session = components
        .next()
        .and_then(|c| match c {
            std::path::Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .ok_or_else(|| "markdown artifact is outside a session".to_string())?;
    if session.is_empty() || session.starts_with('_') {
        return Err("markdown artifact is outside an editable session".to_string());
    }
    let area = components
        .next()
        .and_then(|c| match c {
            std::path::Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .ok_or_else(|| "markdown artifact is outside session artifacts".to_string())?;
    if area != "artifacts" && area != "workspace" {
        return Err("markdown artifact is outside session artifacts".to_string());
    }
    Ok(())
}

fn atomic_write_utf8(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;

    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("artifact.md");
    let tmp = parent.join(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let backup = parent.join(format!(
        ".{file_name}.bak-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));

    let write_result = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
        drop(f);

        #[cfg(windows)]
        {
            std::fs::rename(path, &backup)?;
            if let Err(e) = std::fs::rename(&tmp, path) {
                let _ = std::fs::rename(&backup, path);
                return Err(e);
            }
            let _ = std::fs::remove_file(&backup);
        }

        #[cfg(not(windows))]
        {
            std::fs::rename(&tmp, path)?;
        }

        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();

    if write_result.is_err() {
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(&backup);
    }
    write_result
}

/// 题目转安全文件名:去掉路径分隔/非法字符,截长,空了给兜底。
fn sanitize_title_filename(title: &str, fallback: &str) -> String {
    let cleaned: String = title
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r' | '\0'
            )
        })
        .collect::<String>()
        .trim()
        .chars()
        .take(48)
        .collect();
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned
    }
}

/// 从结案呈报解析「成品清单/工件清单」段申报的成品相对路径。
/// 与引擎 validate_deliverable.check_artifact_manifest 同一约定:
/// 标题段内的列表项,路径写在反引号里(无反引号则取首个空白分隔 token),
/// 到下一个标题为止。
fn parse_product_manifest(report_text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_section = false;
    for line in report_text.lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            let h = t.trim_start_matches('#').trim();
            in_section = h.starts_with("成品清单") || h.starts_with("工件清单");
            continue;
        }
        if !in_section {
            continue;
        }
        let s = line.trim();
        if !(s.starts_with('-') || s.starts_with('*')) {
            continue;
        }
        let item = s.trim_start_matches(['-', '*', ' ']).trim();
        let rel = if let Some(a) = item.find('`') {
            item[a + 1..].split('`').next().unwrap_or("")
        } else {
            item.split_whitespace().next().unwrap_or("")
        };
        if !rel.is_empty() {
            out.push(rel.to_string());
        }
    }
    out
}

/// 旧奏折(无成品清单段)的成品推断:语义归因,三路信号给六部计分。
/// ① 本部章节(### 标题点名 <bu>_N.md)含成品描述词(整合/最终/成品/完整/汇总);
/// ② **被质检对象**——质检章节(标题含审核/校验/验收)里「对〈X部〉…的」的 X
///   (质检节内的成品词描述被审对象,绝不归审核者——天真就近归因实测翻车);
/// ③ 对账表「交付」达成行点名的部。
/// 返回 (bu_id, 得分);得分 <8(单一信号)不可信,调用方回退。
fn infer_product_bu(report: &str) -> Option<(String, i32)> {
    const BU: &[(&str, &str)] = &[
        ("兵部", "bingbu"),
        ("户部", "hubu"),
        ("礼部", "libu"),
        ("刑部", "xingbu"),
        ("工部", "gongbu"),
        ("吏部", "libu_renshi"),
    ];
    const QA_WORDS: &[&str] = &["审核", "校验", "验收", "质检"];
    const PRODUCT_WORDS: &[(&str, i32)] = &[
        ("整合", 3),
        ("最终", 3),
        ("成品", 4),
        ("完整", 2),
        ("汇总", 2),
    ];
    let mut scores: std::collections::HashMap<&str, i32> = Default::default();
    // 按 markdown 标题分节
    let mut sections: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in report.lines() {
        if line.starts_with("##") && !cur.is_empty() {
            sections.push(std::mem::take(&mut cur));
        }
        cur.push_str(line);
        cur.push('\n');
    }
    sections.push(cur);
    for sec in &sections {
        let head = sec.lines().next().unwrap_or("");
        if QA_WORDS.iter().any(|w| head.contains(w)) {
            // 质检节:只认「对〈X部〉」归因(中文部名或拼音 id 都认)
            for (cn, en) in BU {
                if sec.contains(&format!("对{cn}")) || sec.contains(&format!("对{en}")) {
                    *scores.entry(en).or_default() += 5;
                }
            }
            continue;
        }
        // 标题点名 <bu>_<数字>(后随数字才算,防 libu_ 误吃 libu_renshi_1.md)
        let names_bu = |head: &str, en: &str| {
            let pat = format!("{en}_");
            head.match_indices(&pat).any(|(i, _)| {
                head[i + pat.len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
            })
        };
        if let Some((_, en)) = BU.iter().rev().find(|(_, en)| names_bu(head, en)) {
            let pts: i32 = PRODUCT_WORDS
                .iter()
                .filter(|(k, _)| sec.contains(k))
                .map(|(_, w)| w)
                .sum();
            *scores.entry(en).or_default() += pts;
        }
    }
    for line in report.lines() {
        if line.contains("交付") && (line.contains('✅') || line.contains("达成")) {
            for (cn, en) in BU {
                if line.contains(cn) {
                    *scores.entry(en).or_default() += 3;
                }
            }
        }
    }
    scores
        .into_iter()
        .max_by_key(|(_, s)| *s)
        .map(|(b, s)| (b.to_string(), s))
}

/// md 首个一级/二级标题做客户可读的展示标题(客户看不懂 libu_1.md 这种衙门名)。
fn md_display_title(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(8192)]);
    text.lines().find_map(|l| {
        let t = l.trim_start();
        t.strip_prefix("## ")
            .or_else(|| t.strip_prefix("# "))
            .map(|h| h.trim().chars().take(60).collect::<String>())
    })
}

/// 「产出物」跨会话索引:遍历 `~/.pinvou3/sessions/*.json`,把每个会话跟踪的
/// artifacts 汇成一张扁平表(供本地知识 → 产出物 tab 用)。只走磁盘真相:
/// 文件已被删则跳过;mtime/size 现取 fs。
#[derive(Debug, Deserialize)]
struct DvSessionView {
    metadata: DvMeta,
    #[serde(default)]
    artifacts: Vec<DvArtifact>,
}
#[derive(Debug, Deserialize)]
struct DvMeta {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
}
#[derive(Debug, Deserialize)]
struct DvArtifact {
    storage_path: std::path::PathBuf,
    #[serde(default)]
    byte_size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeliverableItem {
    name: String,
    path: String,
    ext: String,
    category: String,
    #[serde(rename = "sessionId")]
    session_id: String,
    source: String,
    mtime: i64,
    size: u64,
}

const DELIVERABLE_EXTS: &[&str] = &[
    "pptx", "ppt", "docx", "doc", "pdf", "html", "htm", "xlsx", "xls", "md", "csv", "png", "jpg",
    "jpeg", "svg", "gif", "webp", "zip",
];

fn deliverable_category(ext: &str) -> &'static str {
    match ext {
        "html" | "htm" | "mhtml" | "mht" => "web",
        "ppt" | "pptx" | "odp" | "dps" => "ppt",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "heic" => "img",
        _ => "doc",
    }
}

#[tauri::command]
pub async fn list_deliverable_index() -> Result<Vec<DeliverableItem>, String> {
    let sessions_dir = crate::bridge::paths::sessions_root();
    let entries = match std::fs::read_dir(&sessions_dir) {
        Ok(e) => e,
        Err(_) => return Ok(Vec::new()),
    };
    let mut by_path: std::collections::HashMap<String, DeliverableItem> =
        std::collections::HashMap::new();

    for entry in entries.flatten() {
        let file = entry.path();
        if !file.is_file() || file.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(view) = serde_json::from_str::<DvSessionView>(&raw) else {
            continue;
        };

        for art in view.artifacts {
            let p = &art.storage_path;
            let Ok(meta) = std::fs::metadata(p) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let name = p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if !DELIVERABLE_EXTS.contains(&ext.as_str()) {
                continue;
            }
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let path = p.to_string_lossy().to_string();
            let item = DeliverableItem {
                name,
                path: path.clone(),
                ext: ext.clone(),
                category: deliverable_category(&ext).to_string(),
                session_id: view.metadata.id.clone(),
                source: view.metadata.title.clone(),
                mtime,
                size: if meta.len() > 0 {
                    meta.len()
                } else {
                    art.byte_size
                },
            };
            by_path
                .entry(path)
                .and_modify(|cur| {
                    if item.mtime >= cur.mtime {
                        *cur = item.clone();
                    }
                })
                .or_insert(item);
        }
    }

    let mut out: Vec<DeliverableItem> = by_path.into_values().collect();
    out.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.name.cmp(&b.name)));
    Ok(out)
}

/// 奏折「成品箱」:**宝箱内容 = 奏折申报的成品**(白浪定的原则,后端不猜)。
/// - products:回奏官在 final_report.md「## 成品清单」段申报的文件(硬闸已核验
///   存在)。衙门式文件名(如 libu_1.md)物化成**以题目命名**的副本给客户——
///   题目取太子立项 zhiyi.json 的 title;非 md 成品(.pptx 等,名字本来就达意)
///   原样装箱。旧 run 没有成品清单段 → 回退:题目命名的 final_report 副本 +
///   deliverables/ 非 md 二进制成品。
/// - papers:deliverables/ 下未被申报为成品的 md = 六部过程文书,折叠降级。
#[tauri::command]
pub async fn list_deliverables(project_dir: String) -> Result<serde_json::Value, String> {
    let p = validate_user_path(&project_dir)?;
    let canon_root = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
    let title = std::fs::read_to_string(p.join("_state").join("zhiyi.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("title").and_then(|t| t.as_str()).map(str::to_string));
    let title_base = sanitize_title_filename(title.as_deref().unwrap_or(""), "最终成品");

    let report_text = std::fs::read_to_string(p.join("final_report.md")).unwrap_or_default();
    let declared = parse_product_manifest(&report_text);

    let mut products: Vec<serde_json::Value> = Vec::new();
    let mut product_canon: Vec<std::path::PathBuf> = Vec::new();

    if !declared.is_empty() {
        // 奏折申报路线:逐件解析,衙门式 md 文件名 → 题目命名副本
        let mut md_idx = 0usize;
        for rel in &declared {
            let cand = p.join(rel);
            let Ok(canon) = std::fs::canonicalize(&cand) else {
                continue;
            };
            if !canon.starts_with(&canon_root) || !canon.is_file() {
                continue;
            }
            let orig_name = canon
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let is_md = orig_name.to_lowercase().ends_with(".md");
            let bytes = std::fs::read(&canon).unwrap_or_default();
            if is_md {
                md_idx += 1;
                let fname = if md_idx == 1 {
                    format!("{title_base}.md")
                } else {
                    format!("{title_base}·{md_idx}.md")
                };
                let dst = p.join(&fname);
                let stale = std::fs::read(&dst).map(|b| b != bytes).unwrap_or(true);
                if stale {
                    let _ = std::fs::write(&dst, &bytes);
                }
                let title = md_display_title(&bytes)
                    .unwrap_or_else(|| fname.trim_end_matches(".md").to_string());
                products.push(serde_json::json!({
                    "name": fname, "title": title, "path": dst.to_string_lossy(), "size": bytes.len(),
                }));
            } else {
                let stem = orig_name
                    .rsplit_once('.')
                    .map(|(a, _)| a)
                    .unwrap_or(&orig_name);
                products.push(serde_json::json!({
                    "name": orig_name, "title": stem, "path": canon.to_string_lossy(), "size": bytes.len(),
                }));
            }
            product_canon.push(canon);
        }
    }
    if products.is_empty() && !report_text.is_empty() {
        // 回退1(旧奏折没写成品清单):语义推断成品归属部——得分≥8(多路信号
        // 汇聚)才可信;取该部序号最大的 deliverable(整合终稿通常是末批)。
        let inferred = infer_product_bu(&report_text)
            .filter(|(_, score)| *score >= 8)
            .and_then(|(bu, _)| {
                let mut best: Option<(u32, std::path::PathBuf)> = None;
                if let Ok(entries) = std::fs::read_dir(p.join("deliverables")) {
                    for e in entries.flatten() {
                        let path = e.path();
                        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                        if let Some(seq) = name
                            .strip_prefix(&format!("{bu}_"))
                            .and_then(|r| r.strip_suffix(".md"))
                            .and_then(|n| n.parse::<u32>().ok())
                        {
                            if best.as_ref().map_or(true, |(s, _)| seq > *s) {
                                best = Some((seq, path));
                            }
                        }
                    }
                }
                best.map(|(_, path)| path)
            });
        if let Some(src) = inferred {
            let bytes = std::fs::read(&src).unwrap_or_default();
            let fname = format!("{title_base}.md");
            let dst = p.join(&fname);
            let stale = std::fs::read(&dst).map(|b| b != bytes).unwrap_or(true);
            if stale {
                let _ = std::fs::write(&dst, &bytes);
            }
            let title = md_display_title(&bytes)
                .unwrap_or_else(|| fname.trim_end_matches(".md").to_string());
            products.push(serde_json::json!({
                "name": fname, "title": title, "path": dst.to_string_lossy(), "size": bytes.len(),
            }));
            if let Ok(canon) = std::fs::canonicalize(&src) {
                product_canon.push(canon);
            }
        }
    }
    if products.is_empty() && !report_text.is_empty() {
        // 回退2(推断也不可信):题目命名的 final_report 副本,不至于空箱
        let fname = format!("{title_base}.md");
        let dst = p.join(&fname);
        let stale = std::fs::read(&dst)
            .map(|b| b != report_text.as_bytes())
            .unwrap_or(true);
        if stale {
            let _ = std::fs::write(&dst, report_text.as_bytes());
        }
        products.push(serde_json::json!({
            "name": fname, "title": title_base.clone(), "path": dst.to_string_lossy(), "size": report_text.len(),
        }));
    }

    // deliverables/:非 md 且未申报 → 也算成品(工件清单核验过的二进制);
    // md 且未申报 → 过程文书
    let mut papers: Vec<serde_json::Value> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(p.join("deliverables")) {
        let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            if !path.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty()
                || name.starts_with('.')
                || name.ends_with(".tmp")
                || name.ends_with('~')
            {
                continue;
            }
            let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if product_canon.contains(&canon) {
                continue; // 已申报装箱,不重复列
            }
            let size = path.metadata().map(|m| m.len()).unwrap_or(0);
            let is_md = name.to_lowercase().ends_with(".md");
            let title = if is_md {
                std::fs::read(&path).ok().and_then(|b| md_display_title(&b))
            } else {
                None
            }
            .unwrap_or_else(|| {
                name.rsplit_once('.')
                    .map(|(a, _)| a)
                    .unwrap_or(&name)
                    .to_string()
            });
            let item = serde_json::json!({
                "name": name, "title": title, "path": path.to_string_lossy(), "size": size,
            });
            if is_md {
                papers.push(item);
            } else {
                products.push(item);
            }
        }
    }
    Ok(serde_json::json!({ "products": products, "papers": papers }))
}

/// 读 artifact 元数据：大小 / 类型 / 是否存在。
#[tauri::command]
pub async fn artifact_info(path: String) -> Result<ArtifactInfo, String> {
    artifact_info_impl(&path)
}

/// [2026-06-07] 读图片 → base64 data url,给 FilePreviewModal 内联预览 png/jpg
/// (csp=null 不拦 data:,比 asset 协议 scope 省事)。validate_user_path 防穿越。
#[tauri::command]
pub async fn read_artifact_image_b64(path: String) -> Result<String, String> {
    let p = validate_user_path(&path).map_err(|_| "路径不允许".to_string())?;
    if !p.is_file() {
        return Err(format!("图片不存在: {path}"));
    }
    let bytes = std::fs::read(&p).map_err(|e| format!("读取失败: {e}"))?;
    if bytes.len() > 25_000_000 {
        return Err("图片过大(>25MB),请用外部打开".into());
    }
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    let mime = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        _ => "image/png",
    };
    let b64 = crate::file_ingest::base64_encode(&bytes);
    Ok(format!("data:{mime};base64,{b64}"))
}

/// [2026-06-22] pptx 封面缩略图：打开 .pptx(zip)读 `docProps/thumbnail.jpeg`
/// → base64 data url，给产物卡顶部 16:9 封面用。无缩略图 / 非 zip / 损坏 → Ok(None)
/// （前端据此回退紧凑态，不报错）。本地数据、无外链，内网离线安全。
/// validate_user_path 防路径穿越；跨平台（zip 纯 Rust，Windows/Linux 一致）。
#[tauri::command]
pub async fn read_artifact_thumbnail(path: String) -> Result<Option<String>, String> {
    use std::io::Read;
    let p = validate_user_path(&path).map_err(|_| "路径不允许".to_string())?;
    if !p.is_file() {
        return Ok(None);
    }
    let file = std::fs::File::open(&p).map_err(|e| format!("打开失败: {e}"))?;
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(z) => z,
        Err(_) => return Ok(None), // 非 zip / 损坏：前端走紧凑态
    };
    // OOXML 缩略图固定路径；兜底几种扩展名（Office 默认写 .jpeg）。
    for name in [
        "docProps/thumbnail.jpeg",
        "docProps/thumbnail.jpg",
        "docProps/thumbnail.png",
    ] {
        let mut entry = match archive.by_name(name) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.size() > 25_000_000 {
            return Ok(None);
        }
        let mut buf = Vec::new();
        if entry.read_to_end(&mut buf).is_err() || buf.is_empty() {
            continue;
        }
        let mime = if name.ends_with(".png") {
            "image/png"
        } else {
            "image/jpeg"
        };
        let b64 = crate::file_ingest::base64_encode(&buf);
        return Ok(Some(format!("data:{mime};base64,{b64}")));
    }
    Ok(None)
}

pub(crate) fn artifact_info_impl(path: &str) -> Result<ArtifactInfo, String> {
    let p = match validate_user_path(path) {
        Ok(p) => p,
        Err(_) => {
            return Ok(ArtifactInfo {
                size: 0,
                kind: "denied".into(),
                exists: false,
                modified: 0,
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
                modified: 0,
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
        | "ts" | "go" | "c" | "cpp" | "h" | "hpp" | "sh" | "bash" | "zsh" | "fish" | "bat"
        | "cmd" | "ps1" | "pl" | "pm" | "lua" | "swift" | "kt" | "kts" | "scala" | "groovy"
        | "dart" | "r" | "m" | "jl" | "erl" | "hrl" | "css" | "scss" | "sass" | "less" | "vue"
        | "svelte" | "mdx" | "sql" | "ini" | "conf" | "cfg" | "env" | "properties" | "reg"
        | "diff" | "patch" | "lock" | "proto" | "graphql" | "gql" | "prisma" => "text",
        _ => "binary",
    };
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(ArtifactInfo {
        size: meta.len(),
        kind: kind.into(),
        exists: true,
        modified,
    })
}

/// PDF 预览逐页转图的页数上限：太多页 data URI 会撑爆前端内存。
const VISUAL_PDF_MAX_PAGES: u32 = 30;

/// 产物可视化预览结果。前端按 `mode` 渲染。
#[derive(Debug, Clone, Serialize)]
pub struct VisualResult {
    /// "html"(iframe srcDoc 渲染) | "images"(逐张图) | "unsupported"(走统一兜底卡)
    pub mode: String,
    /// mode=html：图片已内联的自包含 HTML
    pub html: Option<String>,
    /// mode=images：图片 data URI 列表（pdf 多页 / 单图）
    pub images: Vec<String>,
    /// 缺工具 / 转换失败 / 截断 的人话提示
    pub warning: Option<String>,
}

impl VisualResult {
    fn unsupported(warning: Option<String>) -> Self {
        VisualResult {
            mode: "unsupported".into(),
            html: None,
            images: vec![],
            warning,
        }
    }
}

/// 可视化预览结果缓存（按 路径|mtime 键）。soffice/pdftoppm 一次 1-3s，缓存后二次秒开。
fn visual_cache() -> &'static parking_lot::Mutex<std::collections::HashMap<String, VisualResult>> {
    static CACHE: std::sync::OnceLock<
        parking_lot::Mutex<std::collections::HashMap<String, VisualResult>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| parking_lot::Mutex::new(std::collections::HashMap::new()))
}

/// 把 office/pdf/图片产物转成可视化预览：office→自包含 HTML，pdf→逐页 PNG，图片→data URI。
/// 结果按 路径+mtime 缓存。md/html/text 不走这里（前端直接读文本渲染）。
/// 转换慢且阻塞 → 丢到 `spawn_blocking`，不堵 tokio reactor。
#[tauri::command]
pub async fn render_artifact_visual(path: String) -> Result<VisualResult, String> {
    let p = validate_user_path(&path)?;
    if !p.is_file() {
        return Err(format!("not a file: {}", p.display()));
    }
    let mtime = std::fs::metadata(&p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let cache_key = format!("{}|{}", p.display(), mtime);
    if let Some(hit) = visual_cache().lock().get(&cache_key).cloned() {
        return Ok(hit);
    }

    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let p2 = p.clone();
    let result = tokio::task::spawn_blocking(move || -> VisualResult {
        use crate::file_ingest as fi;
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" => {
                match fi::image_file_to_data_uri(&p2) {
                    Ok(uri) => VisualResult {
                        mode: "images".into(),
                        html: None,
                        images: vec![uri],
                        warning: None,
                    },
                    Err(e) => VisualResult::unsupported(Some(e)),
                }
            }
            // PDF / 演示稿 → 逐页 PNG。演示稿先转 PDF 再逐页(每页=一张幻灯片)。
            "pdf" | "pptx" | "ppt" | "odp" => {
                let conv = if ext == "pdf" {
                    fi::pdf_to_png_data_uris(&p2, VISUAL_PDF_MAX_PAGES)
                } else {
                    fi::office_to_png_data_uris(&p2, VISUAL_PDF_MAX_PAGES)
                };
                match conv {
                    Ok((imgs, truncated)) => VisualResult {
                        mode: "images".into(),
                        html: None,
                        images: imgs,
                        warning: truncated
                            .then(|| format!("页数较多，仅渲染前 {VISUAL_PDF_MAX_PAGES} 页")),
                    },
                    Err(e) => VisualResult::unsupported(Some(e)),
                }
            }
            // 文字文档 / 电子表格 → 自包含 HTML(版式 + 内联图片)。
            "docx" | "odt" | "rtf" | "doc" | "xlsx" | "ods" | "xls" => {
                match fi::libreoffice_to_inline_html(&p2) {
                    Ok(html) => VisualResult {
                        mode: "html".into(),
                        html: Some(html),
                        images: vec![],
                        warning: None,
                    },
                    Err(e) => VisualResult::unsupported(Some(e)),
                }
            }
            _ => VisualResult::unsupported(None),
        }
    })
    .await
    .map_err(|e| format!("render_artifact_visual join: {e}"))?;

    // unsupported 不缓存：可能是工具暂缺，装上后下次重试。
    if result.mode != "unsupported" {
        visual_cache().lock().insert(cache_key, result.clone());
    }
    Ok(result)
}

/// 跨平台"用系统默认程序打开"(文件 / 目录 / URL)。
/// Windows: `cmd /C start`（`start` 是 cmd 内建；首个引号参数是窗口标题，用空串占位）；
/// macOS: `open`；Linux/其它: `xdg-open`（freedesktop，跨发行版兼容）。
/// 调用方对文件/目录路径应先过 `strip_verbatim` 去掉 `\\?\` 前缀（start/explorer 不认）。
fn shell_open(arg: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c.arg(arg);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(arg);
        c
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(arg);
        c
    };
    cmd.spawn()
        .map_err(|e| format!("open({arg}) failed: {e}"))?;
    Ok(())
}

fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(target_os = "windows")]
fn reveal_path_in_file_manager(target: &std::path::Path) -> Result<(), String> {
    let target = strip_verbatim(target);
    std::process::Command::new("explorer")
        .arg(format!("/select,{target}"))
        .spawn()
        .map_err(|e| format!("explorer select failed: {e}"))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn reveal_path_in_file_manager(target: &std::path::Path) -> Result<(), String> {
    let target = strip_verbatim(target);
    std::process::Command::new("open")
        .args(["-R", &target])
        .spawn()
        .map_err(|e| format!("open -R failed: {e}"))?;
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn reveal_path_in_file_manager(target: &std::path::Path) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| format!("no parent dir for {}", target.display()))?;

    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() {
        if let Ok(url) = tauri::Url::from_directory_path(target) {
            let uri = url.to_string();
            let items_arg = format!("array:string:{uri}");
            if let Ok(output) = std::process::Command::new("dbus-send")
                .args([
                    "--session",
                    "--dest=org.freedesktop.FileManager1",
                    "--type=method_call",
                    "/org/freedesktop/FileManager1",
                    "org.freedesktop.FileManager1.ShowItems",
                    &items_arg,
                    "string:",
                ])
                .output()
            {
                if output.status.success() {
                    return Ok(());
                }
            }
        }
    }

    shell_open(&strip_verbatim(parent))
}

/// 去掉 Windows `canonicalize` 产出的 `\\?\` verbatim 前缀（含 UNC 形式）。
/// start/explorer 不识别该前缀，不剥会"打不开"。非 Windows 原样返回。
fn strip_verbatim(p: &std::path::Path) -> String {
    let s = p.to_string_lossy();
    #[cfg(target_os = "windows")]
    {
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{rest}");
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return rest.to_string();
        }
    }
    s.to_string()
}

fn file_url_from_path(p: &std::path::Path) -> Result<tauri::Url, String> {
    let normal_path = std::path::PathBuf::from(strip_verbatim(p));
    tauri::Url::from_file_path(&normal_path)
        .map_err(|_| format!("convert file url: {}", p.display()))
}

/// 外部链接白名单：前端 webview 万一被 XSS 时的最后一道防线。
/// **扩这个列表必须同步加测试**（见 `external_allowlist_*` 单测）。
const EXTERNAL_URL_ALLOWLIST: &[&str] = &[
    "https://metaso.cn/",
    "https://open.bochaai.com/",
    "https://console.bce.baidu.com/",
    "https://app.tavily.com/",
    "https://www.iwencai.com/",
    "https://agent.qcc.com/",
    // 智慧芽开放平台:智慧芽 MCP API Key 获取说明
    "https://open.zhihuiya.com/",
    // MegaCube 官网(侧边栏 footer 入口跳转)
    "https://www.h3c.com/",
    // 飞书/Lark OAuth(device flow 授权页 + 账号页);连接飞书走这里开浏览器
    "https://open.feishu.cn/",
    "https://accounts.feishu.cn/",
    "https://www.feishu.cn/",
    "https://open.larksuite.com/",
    "https://accounts.larksuite.com/",
    // Obsidian 官网:知识库连接器探测到未安装时,引导用户下载
    "https://obsidian.md/",
];

/// URL 是否命中外部链接白名单(纯函数,便于单测)。
fn url_in_external_allowlist(url: &str) -> bool {
    EXTERNAL_URL_ALLOWLIST.iter().any(|p| url.starts_with(p))
}

/// 用系统默认浏览器打开**允许列表**里的 https URL。
/// 用于 Settings 面板的"获取 API key"链接(Metaso/Bocha 注册页)等。
/// 白名单写死,前端没法用这个 command 打开任意 URL。
#[tauri::command]
pub async fn open_external_url(url: String) -> Result<(), String> {
    if !url_in_external_allowlist(&url) {
        return Err(format!("URL not in allowlist: {url}"));
    }
    shell_open(&url)
}

/// 本机 Obsidian 状态(供工具市场"连接"前分支)。
/// state: `not_installed` | `no_vault` | `vault_missing` | `ok`
#[derive(serde::Serialize)]
pub struct ObsidianStatus {
    pub state: String,
    pub vault_path: Option<String>,
}

/// Obsidian 桌面端记录库列表的 `obsidian.json` 路径(跨平台)。
fn obsidian_config_path() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|p| {
            std::path::Path::new(&p)
                .join("obsidian")
                .join("obsidian.json")
        })
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|p| {
            std::path::Path::new(&p).join("Library/Application Support/obsidian/obsidian.json")
        })
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("HOME")
            .map(|p| std::path::Path::new(&p).join(".config/obsidian/obsidian.json"))
    }
}

/// 从 obsidian.json 文本里挑出当前库路径:优先 `open:true`,否则 `ts` 最大。
/// 与 `mcp-servers/obsidian/server.py` 的 `_autodiscover_vault` 同规则,需保持一致。
fn pick_vault_path(text: &str) -> Option<String> {
    let text = text.trim_start_matches('\u{feff}'); // 剥 BOM
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    let vaults = json.get("vaults")?.as_object()?;
    if vaults.is_empty() {
        return None;
    }
    let pick = vaults
        .values()
        .find(|v| v.get("open").and_then(|o| o.as_bool()).unwrap_or(false))
        .or_else(|| {
            vaults
                .values()
                .max_by_key(|v| v.get("ts").and_then(|t| t.as_i64()).unwrap_or(0))
        })?;
    pick.get("path")?.as_str().map(|s| s.to_string())
}

/// 探测本机 Obsidian 状态,供工具市场"连接 Obsidian"前分支:
/// 没装就引导下载,没库就引导建库,而不是默默装上一个用不了的连接器。
#[tauri::command]
pub fn detect_obsidian() -> ObsidianStatus {
    let not_installed = || ObsidianStatus {
        state: "not_installed".into(),
        vault_path: None,
    };
    let cfg = match obsidian_config_path() {
        Some(p) if p.is_file() => p,
        _ => return not_installed(),
    };
    let text = match std::fs::read_to_string(&cfg) {
        Ok(t) => t,
        Err(_) => return not_installed(),
    };
    match pick_vault_path(&text) {
        None => ObsidianStatus {
            state: "no_vault".into(),
            vault_path: None,
        },
        Some(p) if std::path::Path::new(&p).is_dir() => ObsidianStatus {
            state: "ok".into(),
            vault_path: Some(p),
        },
        Some(p) => ObsidianStatus {
            state: "vault_missing".into(),
            vault_path: Some(p),
        },
    }
}

/// 把成品卡里可能的**相对**路径落到产物所属 session 的 workspace。
///
/// 背景:present_artifact 没调成(模型把工具名漂成 `pinvou-present_artifact` 之类
/// → NotAvailable)时,成品卡由 write_file 兜底补出,path 直接用了 write_file 的
/// 相对参数(如 `snake-game.html`)。点 Open 把相对路径丢给 `validate_user_path`
/// → 直接拒「path must be absolute」。这里先按 workspace 解析,绝对路径原样返回
/// (present_artifact 成功解析的 / 产物面板 list_workspace_files 给的已是绝对)。
///
/// `session_id` = 卡片携带的**产物所属** session,**优先**用它而非全局 active_id:
/// 切回「已访问过(有 buffer)」的会话时,前端走 switchActiveTo 不调 load_session,
/// 后端 active_id 不更新 → 仍指向切走时去的那个 session → 相对路径被拼到错的
/// workspace(报「not a file」)。卡片自带 session 才能跨会话切换稳定解析。
/// None 时(老卡无此字段 / 绝对路径)回退 active_id,行为同旧版。
fn resolve_artifact_path(
    raw: &str,
    session_id: Option<&str>,
    store: &SessionStore,
) -> Result<String, String> {
    if std::path::Path::new(raw).is_absolute() {
        return Ok(raw.to_string());
    }
    let sid = session_id
        .map(|s| s.to_string())
        .or_else(|| store.active_id());
    match sid {
        Some(sid) => store
            .execution_workspace(&sid)
            .map(|workspace| resolve_artifact_path_in_workspace(raw, &workspace))
            .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}")),
        None => Ok(raw.to_string()),
    }
}

pub(crate) fn resolve_artifact_path_in_workspace(raw: &str, workspace: &std::path::Path) -> String {
    if std::path::Path::new(raw).is_absolute() {
        raw.to_string()
    } else {
        workspace.join(raw).to_string_lossy().into_owned()
    }
}

/// 用系统默认应用打开文件（跨平台：Win `start` / mac `open` / Linux `xdg-open`）；
/// 相对路径先按产物所属 session（前端传 `sessionId`，缺则 active）的 workspace 解析。
#[tauri::command]
pub async fn open_in_system(
    path: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    let resolved = resolve_artifact_path(&path, session_id.as_deref(), &store)?;
    let p = validate_user_path(&resolved)?;
    shell_open(&strip_verbatim(&p))
}

/// 用文件管理器打开**所在目录**（不是文件本身）。跨平台：Win explorer / mac Finder /
/// Linux Nautilus 等（freedesktop，跨 GNOME/KDE/XFCE 兼容）。
#[tauri::command]
pub async fn open_containing_folder(
    path: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    let resolved = resolve_artifact_path(&path, session_id.as_deref(), &store)?;
    let p = validate_user_path(&resolved)?;
    let dir = p
        .parent()
        .ok_or_else(|| format!("no parent dir for {}", p.display()))?;
    shell_open(&strip_verbatim(dir))
}

/// 在文件管理器里定位 session 文件夹。对标 WorkBuddy:打开所有任务文件夹的上级目录,
/// 并尽可能选中当前任务文件夹；Linux 文件管理器不支持选中时退回打开 sessions 根目录。
#[tauri::command]
pub async fn reveal_session_folder(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    if !valid_session_id(&session_id) {
        return Err("invalid session id".into());
    }
    store
        .load(&session_id)
        .map_err(|e| format!("load_session({session_id}): {e:?}"))?;
    // 定时运行会话没有独立 runtime 目录，打开它所属任务的共享工作间。
    if store.scheduled_profile(&session_id).is_some() {
        let dir = store
            .execution_workspace(&session_id)
            .map_err(|e| format!("reveal_session_folder({session_id}): {e:#}"))?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("create scheduled task workspace {}: {e}", dir.display()))?;
        return shell_open(&strip_verbatim(&dir));
    }
    let dir = crate::bridge::paths::sessions_root().join(&session_id);
    if !dir.is_dir() {
        return Err(format!("session folder not found: {}", dir.display()));
    }
    reveal_path_in_file_manager(&dir)
}

/// 打开某个定时任务独享的工作间。工作间由 automation id 稳定派生，任务的多次运行
/// 共享该目录；首次打开早于首次运行时按需创建，不接受前端传入任意文件系统路径。
#[tauri::command]
pub async fn open_scheduled_task_folder(automation_id: String) -> Result<(), String> {
    if !valid_session_id(&automation_id) {
        return Err("invalid automation id".into());
    }
    let dir = crate::bridge::paths::scheduled_task_workspace_dir(&automation_id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("create scheduled task workspace {}: {e}", dir.display()))?;
    shell_open(&strip_verbatim(&dir))
}

/// 在 Tauri 新窗口里加载 HTML 产物。绕过 snap 浏览器对 `~/.xxx/` 隐藏目录的沙箱限制。
/// 同一文件再次调用 → focus 已有窗口而非新建,防窗口爆炸。
#[tauri::command]
pub async fn open_artifact_window(
    path: String,
    session_id: Option<String>,
    app: tauri::AppHandle,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    let resolved = resolve_artifact_path(&path, session_id.as_deref(), &store)?;
    let p = validate_user_path(&resolved)?;
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

    let url = file_url_from_path(&p)?;
    let title = p
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("产物")
        .to_string();

    WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(url))
        .title(title)
        .inner_size(1024.0, 768.0)
        .center()
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

// ===================== 卡片池: 专家面具 =====================

/// 列出全部专家卡的**摘要**（不含 body，~200 卡）。前端进卡片池拉一次缓存。
/// Side B: body 太大（~6K 字/张），不随 list 下发，加持/详情时按需取。
#[tauri::command]
pub async fn list_personas() -> Result<Vec<crate::personas::PersonaSummary>, String> {
    Ok(crate::personas::all_summaries())
}

/// 读单个专家的完整人设正文（详情 modal 预览用）。
#[tauri::command]
pub async fn read_persona_body(persona_id: String) -> Result<String, String> {
    crate::personas::get(&persona_id)
        .map(|c| c.body.clone())
        .ok_or_else(|| format!("未知专家面具: {persona_id}"))
}

/// 给当前 session 加持一张专家面具（点卡片"加持给 AI"）。
/// Side B: 存 persona_id + 把完整 body 挂为 pending（下一条 chat 一次性 prepend）；
/// 之后每 turn 只注入轻锚点。返回摘要供前端渲染挂件 + 系统消息。
#[tauri::command]
pub async fn equip_persona(
    session_id: String,
    persona_id: String,
    store: State<'_, SessionStore>,
) -> Result<crate::personas::PersonaSummary, String> {
    let card =
        crate::personas::get(&persona_id).ok_or_else(|| format!("未知专家面具: {persona_id}"))?;
    let summary = card.summary();
    store.set_pending_persona_body(
        &session_id,
        Some(crate::personas::equip_body_injection(&card)),
    );
    store.set_active_persona(&session_id, Some(persona_id));
    Ok(summary)
}

// ── 用户自创卡 CRUD ────────────────────────────────────────────────

/// 前端建/改卡传入的字段(不含 id/source —— create 由后端生成 id;update 用 persona_id)。
#[derive(Debug, serde::Deserialize)]
pub struct PersonaInput {
    pub name: String,
    pub dept: String,
    #[serde(default)]
    pub emoji: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub body: String,
}

impl PersonaInput {
    fn into_card(self, id: String) -> crate::personas::PersonaCard {
        crate::personas::PersonaCard {
            id,
            dept: self.dept,
            name: self.name,
            description: self.description,
            emoji: if self.emoji.is_empty() {
                "🃏".into()
            } else {
                self.emoji
            },
            color: if self.color.is_empty() {
                "#7C3AED".into()
            } else {
                self.color
            },
            body: self.body,
            source: "user".into(),
            // 用户自创卡都是干活的领域卡,照常带全量工具;元卡标记只属内置卡。
            conversational_only: false,
        }
    }
}

/// 新建自制卡 → 写 `~/.pinvou3/user/personas/<id>.json`,返回摘要(含生成的 id)。
#[tauri::command]
pub async fn create_persona(
    input: PersonaInput,
) -> Result<crate::personas::PersonaSummary, String> {
    crate::personas::create_user_persona(input.into_card(String::new()))
}

/// 编辑自制卡(persona_id 必须是 user- 前缀)。
#[tauri::command]
pub async fn update_persona(
    persona_id: String,
    input: PersonaInput,
) -> Result<crate::personas::PersonaSummary, String> {
    crate::personas::update_user_persona(input.into_card(persona_id))
}

/// 删除自制卡。
#[tauri::command]
pub async fn delete_persona(persona_id: String) -> Result<(), String> {
    crate::personas::delete_user_persona(&persona_id)
}

/// 保存某 session 的卡牌加持/卸下事件时间线(sidecar,不进 messages)。
/// events 是前端定义的 opaque JSON 数组,后端只透明落盘。
#[tauri::command]
pub async fn save_session_persona_events(
    session_id: String,
    events: serde_json::Value,
) -> Result<(), String> {
    let path = crate::bridge::paths::session_persona_events(&session_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建 session 目录失败: {e}"))?;
    }
    let json = serde_json::to_string(&events).map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("写卡牌事件失败: {e}"))
}

/// 读某 session 的卡牌事件时间线(无则返回空数组)。
#[tauri::command]
pub async fn get_session_persona_events(session_id: String) -> Result<serde_json::Value, String> {
    let path = crate::bridge::paths::session_persona_events(&session_id);
    match std::fs::read_to_string(&path) {
        Ok(txt) => Ok(serde_json::from_str(&txt).unwrap_or_else(|_| serde_json::json!([]))),
        Err(_) => Ok(serde_json::json!([])),
    }
}

/// Pinvou 召唤检阅时间线（opaque JSON，后端透明落盘，同 persona_events 范式）。
/// 前端每次召唤后存，load_session 时读回，rerender 按 pos 插回审查卡——独立于
/// messages，绝不进 LLM 上下文（设计 §6 / `docs/品悟v4-常驻检阅助手设计.md`）。
/// 落盘前保留盘上已有的 resolution：防止后续全量 save（典型=核账 record 用不含 resolution
/// 的快照）冲掉 Boss 已做的逐条裁决。按数组下标对齐——pinvouReviews 是 append-only、每条
/// review 内容不可变，下标稳定可靠。new 自带 resolution 就用 new（允许 Boss 改裁决）；new
/// 缺失才继承 old。根治「resolution 写进 sidecar 后被无 resolution 的全量 save 覆盖」的实测 bug。
fn preserve_resolutions(path: &std::path::Path, new: serde_json::Value) -> serde_json::Value {
    let old: serde_json::Value = match std::fs::read_to_string(path) {
        Ok(txt) => match serde_json::from_str(&txt) {
            Ok(v) => v,
            Err(_) => return new,
        },
        Err(_) => return new,
    };
    merge_resolutions(old, new)
}

/// 纯合并逻辑（抽出便于单测）：new 缺 resolution 的条目继承 old 同下标的。
fn merge_resolutions(old: serde_json::Value, mut new: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    let old_arr = match old.as_array() {
        Some(a) => a,
        None => return new,
    };
    let new_arr = match new.as_array_mut() {
        Some(a) => a,
        None => return new,
    };
    for (i, entry) in new_arr.iter_mut().enumerate() {
        let old_entry = match old_arr.get(i) {
            Some(e) => e,
            None => continue,
        };
        for field in ["issues", "recommendations"] {
            let ptr = format!("/review/{field}");
            let old_items = match old_entry.pointer(&ptr).and_then(Value::as_array) {
                Some(a) => a,
                None => continue,
            };
            let new_items = match entry.pointer_mut(&ptr).and_then(Value::as_array_mut) {
                Some(a) => a,
                None => continue,
            };
            for (j, ni) in new_items.iter_mut().enumerate() {
                if ni.get("resolution").map_or(false, |v| !v.is_null()) {
                    continue; // new 已带裁决，尊重 new（含 Boss 改裁决/取消）
                }
                if let Some(old_res) = old_items.get(j).and_then(|x| x.get("resolution")) {
                    if !old_res.is_null() {
                        if let Some(obj) = ni.as_object_mut() {
                            obj.insert("resolution".to_string(), old_res.clone());
                        }
                    }
                }
            }
        }
    }
    new
}

#[tauri::command]
pub async fn save_session_pinvou_reviews(
    session_id: String,
    reviews: serde_json::Value,
) -> Result<(), String> {
    let path = crate::bridge::paths::session_pinvou_reviews(&session_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建 session 目录失败: {e}"))?;
    }
    let merged = preserve_resolutions(&path, reviews);
    let json = serde_json::to_string(&merged).map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("写 Pinvou 审查失败: {e}"))
}

/// 读某 session 的 Pinvou 审查时间线（无则返回空数组）。
#[tauri::command]
pub async fn get_session_pinvou_reviews(session_id: String) -> Result<serde_json::Value, String> {
    let path = crate::bridge::paths::session_pinvou_reviews(&session_id);
    match std::fs::read_to_string(&path) {
        Ok(txt) => Ok(serde_json::from_str(&txt).unwrap_or_else(|_| serde_json::json!([]))),
        Err(_) => Ok(serde_json::json!([])),
    }
}

/// 摘下当前 session 的专家面具（点挂件取消 / 卡片"已加持"再点）。
#[tauri::command]
pub async fn unequip_persona(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    store.set_active_persona(&session_id, None);
    store.set_pending_persona_body(&session_id, None);
    Ok(())
}

/// 查当前 session 加持的专家面具摘要（前端启动 / 切 session 时拉，用于还原挂件）。
/// 无加持返回 None。
#[tauri::command]
pub async fn get_active_persona(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<Option<crate::personas::PersonaSummary>, String> {
    Ok(store
        .active_persona_id(&session_id)
        .and_then(|pid| crate::personas::get(&pid).map(|c| c.summary())))
}

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
    // preflight:云端模型但 API Key 缺失 → 直接返回友好错误,而不是让空 key 打到
    // 云端变 401(根因:macOS Keychain 存空值 + 假阳性,详见 credential_store.rs)。
    // 本地 vllm 走 LOCAL_VLLM_API_KEY 兜底,放行。
    if pool.bridge.provider() != "vllm" && pool.bridge.api_key().trim().is_empty() {
        return Err("未配置 API Key，请先在「设置 → 模型」中配置。".to_string());
    }
    let session = store
        .load(&sid)
        .map_err(|e| format!("summon_pinvou load({sid}): {e:?}"))?;
    let workspace = store
        .execution_workspace(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    crate::pinvou_review::summon(
        &pool.bridge,
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
    use crate::bridge::paths;
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
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<StartSkillSessionResult, String> {
    use crate::bridge::mode_state::ActiveSkillBinding;
    use crate::bridge::paths;
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
        store.set_active(Some(sid.clone()));
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
    store.set_active(Some(sid.clone()));

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
/// 3. 加载 `h3c-ppt` skill 拿 phases，把 session 绑定到该 skill + project_dir
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
    use crate::bridge::mode_state::ActiveSkillBinding;

    // 0. 按 scenario 解析所属工作流(WorkflowRegistry 扫 bundle/workflow/*/workflow.json)。
    //    enabled=false 只挡新建,历史项目不受影响(resolver 侧不过滤)。
    let wf = crate::workflow_registry::by_scenario(&scenario).ok_or_else(|| {
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
        .execution_workspace(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    let project_dir = tokio::task::spawn_blocking({
        let workspace = workspace.clone();
        let scenario = scenario.clone();
        move || crate::harness::init_project(&workspace, &scenario, &brief)
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
            if let Some(state) = crate::harness::read_full_agent_state(&ws) {
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
        .execution_workspace(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    let action = tokio::task::spawn_blocking(move || crate::harness::step_fresh(&ws))
        .await
        .map_err(|e| format!("spawn_blocking step_fresh: {e}"))?;

    match action {
        // [拆对话线 C] step_fresh 直接返回 SpawnAgent，Harness 直派真 SubAgent，
        // executing 态，主 session 空闲（无品悟交代/自演）。
        crate::harness::HarnessAction::SpawnAgent {
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
        crate::harness::HarnessAction::SpawnAgentBatch {
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
            let k = crate::harness::per_page_concurrency();
            let first = crate::harness::batch_seed_and_take(&sid, &base_role, tasks, k);
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
            crate::engine::emit_fanout(&app, &sid, &base_role); // 初始 fan-out 状态 → 前端
            Ok(format!("spawning {role_name} ({n} pages, 在飞={k})"))
        }
        crate::harness::HarnessAction::Blocked { message } => {
            Err(format!("workflow blocked: {message}"))
        }
        _ => Ok("no dispatch (already running or not applicable)".to_string()),
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
        .execution_workspace(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    let rid = role_id.clone();
    let action = tokio::task::spawn_blocking(move || crate::harness::retry_role(&ws, &rid))
        .await
        .map_err(|e| format!("spawn_blocking retry_role: {e}"))?;

    match action {
        crate::harness::HarnessAction::SpawnAgent {
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
        crate::harness::HarnessAction::SpawnAgentBatch {
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
            let k = crate::harness::per_page_concurrency();
            let first = crate::harness::batch_seed_and_take(&sid, &base_role, tasks, k);
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
            crate::engine::emit_fanout(&app, &sid, &base_role); // 初始 fan-out 状态 → 前端
            Ok(format!(
                "retry → spawning {role_name} ({n} pages, 在飞={k})"
            ))
        }
        crate::harness::HarnessAction::Blocked { message } => {
            Err(format!("retry blocked: {message}"))
        }
        crate::harness::HarnessAction::Error(e) => Err(format!("retry error: {e}")),
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
        .map(|p| crate::harness::workflow_of_project(std::path::Path::new(p)))
        .unwrap_or_else(|| crate::harness::workflow_of_role(&role_id));
    let skills_dir = crate::harness::workflow_root_for(&workflow);
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
    let workflow = crate::harness::workflow_of_project(std::path::Path::new(&project_dir));
    let skills_dir = crate::harness::workflow_root_for(&workflow);
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

/// 取一个角色的执行日志尾部 N 条（从 `_state/workflow_flow.log` 按 role_id 过滤）。
/// 详情 Drawer 的 "执行日志" Tab 用。
#[tauri::command]
pub async fn get_role_logs(
    role_id: String,
    project_dir: String,
    tail: Option<usize>,
) -> Result<Vec<serde_json::Value>, String> {
    let log_path = std::path::PathBuf::from(&project_dir)
        .join("_state")
        .join("workflow_flow.log");
    if !log_path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&log_path).map_err(|e| format!("read log: {e}"))?;
    let limit = tail.unwrap_or(200);
    let mut out: Vec<serde_json::Value> = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // role_id 过滤；缺字段或匹配则收下
        let matches = rec
            .get("role_id")
            .and_then(|v| v.as_str())
            .map(|r| r == role_id)
            .unwrap_or(true); // 没有 role_id 字段的全局事件保留
        if matches {
            out.push(rec);
        }
    }
    let start = out.len().saturating_sub(limit);
    Ok(out[start..].to_vec())
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
        .execution_workspace(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    let rid = role_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        // 找到 project_dir
        let project = match crate::harness::find_project_dir(&workspace) {
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
        let scheduler = crate::harness::scheduler_path_for(
            &crate::harness::workflow_name_for_scenario(&scenario),
        );
        let output = std::process::Command::new("python3")
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
        .execution_workspace(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    let stop_reason = reason
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "user_stopped".to_string());

    let ws = workspace.clone();
    let marker_sid = sid.clone();
    let marker_reason = stop_reason.clone();
    let result = tokio::task::spawn_blocking(move || {
        crate::harness::stop_workflow(&ws, &marker_sid, &marker_reason)
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
        .execution_workspace(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    let engine = pool
        .get_or_spawn(&sid)
        .await
        .map_err(|e| format!("get engine for {sid}: {e:?}"))?;
    let rid = role_id.clone();
    let action =
        tokio::task::spawn_blocking(move || crate::harness::approve_gate(&workspace, &rid))
            .await
            .map_err(|e| format!("spawn_blocking: {e}"))?;
    // approve 后 step_fresh 推进到下一角色：SpawnAgent（直派）/ AllDone / WaitForHuman。
    // 用 apply_harness_action 统一处理（set phase / emit / 派发），其值化结果回前端。
    let next_label = match &action {
        crate::harness::HarnessAction::SpawnAgent { .. } => "dispatch",
        crate::harness::HarnessAction::AllDone => "all_done",
        crate::harness::HarnessAction::WaitForHuman { .. } => "waiting",
        crate::harness::HarnessAction::Blocked { .. } => "blocked",
        _ => "noop",
    };
    let handled =
        crate::engine::apply_harness_action(action, &app, &engine.workspace, &engine.handle, &sid)
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
        .execution_workspace(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    let engine = pool
        .get_or_spawn(&sid)
        .await
        .map_err(|e| format!("get engine for {sid}: {e:?}"))?;
    let rid = role_id.clone();
    let r = reason.clone();
    let action =
        tokio::task::spawn_blocking(move || crate::harness::reject_gate(&workspace, &rid, &r))
            .await
            .map_err(|e| format!("spawn_blocking: {e}"))?;
    // reject 后 reject_gate 返回 SpawnAgent（重新派发同角色 SubAgent，附拒绝原因）。
    let next_label = match &action {
        crate::harness::HarnessAction::SpawnAgent { .. } => "redo",
        crate::harness::HarnessAction::Blocked { .. } => "blocked",
        _ => "noop",
    };
    let handled =
        crate::engine::apply_harness_action(action, &app, &engine.workspace, &engine.handle, &sid)
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
        .execution_workspace(&sid)
        .map_err(|error| format!("resolve execution workspace for {sid}: {error:#}"))?;
    tokio::task::spawn_blocking(move || {
        crate::harness::read_full_agent_state(&workspace).unwrap_or(serde_json::json!(null))
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
        // scenario 已没有对应工作流(如已下线存档的 h3c-ppt 项目)→ 跳过。
        // 否则老 PPT 半途 run(永远不会完成)mtime 最新时,每次开机都恢复进僵尸会话。
        if crate::workflow_registry::by_scenario(&scenario).is_none() {
            continue;
        }
        let mtime = std::fs::metadata(&progress)
            .and_then(|md| md.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        if best.as_ref().map_or(true, |(bt, _, _, _)| mtime > *bt) {
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
    Ok(crate::workflow_registry::discover()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn marketplace_auth_status_only_oauth_is_connected_for_oauth_tools() {
        use deepseek_tui::mcp::oauth::McpAuthStatus;

        let (status, _, token_present) =
            marketplace_auth_status_fields(true, true, true, Some(McpAuthStatus::OAuth));
        assert_eq!(status, "connected");
        assert!(token_present);

        for auth_status in [
            McpAuthStatus::NotLoggedIn,
            McpAuthStatus::Unsupported,
            McpAuthStatus::BearerToken,
        ] {
            let (status, _, token_present) =
                marketplace_auth_status_fields(true, true, true, Some(auth_status));
            assert_eq!(status, "config_installed_auth_pending");
            assert!(!token_present);
        }
    }

    #[test]
    fn marketplace_auth_status_preserves_non_oauth_installed_semantics() {
        let (status, _, token_present) = marketplace_auth_status_fields(true, false, false, None);
        assert_eq!(status, "connected");
        assert!(!token_present);

        let (status, _, token_present) = marketplace_auth_status_fields(false, false, false, None);
        assert_eq!(status, "not_installed");
        assert!(!token_present);
    }

    #[test]
    fn marketplace_auth_status_requires_mcp_config_for_oauth_connected() {
        use deepseek_tui::mcp::oauth::McpAuthStatus;

        let (status, _, token_present) =
            marketplace_auth_status_fields(true, true, false, Some(McpAuthStatus::OAuth));
        assert_eq!(status, "auth_pending");
        assert!(!token_present);
    }

    struct TempPinvou3Home {
        root: PathBuf,
        previous: Option<String>,
    }

    impl TempPinvou3Home {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "pinvou3-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let previous = std::env::var("PINVOU3_HOME").ok();
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            std::env::set_var("PINVOU3_HOME", &root);
            Self { root, previous }
        }
    }

    impl Drop for TempPinvou3Home {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var("PINVOU3_HOME", value),
                None => std::env::remove_var("PINVOU3_HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn write_json(path: &Path, value: serde_json::Value) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    }

    fn write_test_oauth_marketplace_files(server_name: &str, mcp_server: serde_json::Value) {
        let manifest = serde_json::json!({
            "id": "yuandian-mcp",
            "name": "华宇元典法律数据",
            "description": "test",
            "version": "1.0.0",
            "icon": "BookOpen",
            "category": "kb",
            "mcp_tools": [],
            "command": "",
            "args": [],
            "servers": [{
                "name": server_name,
                "url": mcp_server.get("url").and_then(|v| v.as_str()).unwrap_or("not-a-url"),
                "scopes": ["legal"],
                "oauth": { "client_id": "test-client" }
            }]
        });
        write_json(
            &crate::bridge::paths::bundle_mcp_servers_dir()
                .join("yuandian-mcp")
                .join("manifest.json"),
            manifest,
        );
        write_json(
            &crate::bridge::paths::pinvou3_home()
                .join("marketplace")
                .join("installed.json"),
            serde_json::json!(["yuandian-mcp"]),
        );
        write_json(
            &crate::bridge::paths::mcp_config_path(),
            serde_json::json!({ "servers": { server_name: mcp_server } }),
        );
    }

    fn test_oauth_store_key(server_name: &str, url: &str) -> String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        use sha2::{Digest, Sha256};

        let mut payload = Vec::with_capacity(server_name.len() + url.len() + 1);
        payload.extend_from_slice(server_name.as_bytes());
        payload.push(0);
        payload.extend_from_slice(url.as_bytes());
        let digest = Sha256::digest(&payload);
        format!("mcp_oauth_{}", URL_SAFE_NO_PAD.encode(digest))
    }

    fn set_test_oauth_token(server_name: &str, url: &str, value: &str) -> String {
        let key = test_oauth_store_key(server_name, url);
        codewhale_secrets::Secrets::auto_detect()
            .set(&key, value)
            .unwrap();
        key
    }

    fn delete_test_oauth_token_key(key: &str) {
        let _ = codewhale_secrets::Secrets::auto_detect().delete(key);
    }

    fn test_oauth_token_exists(key: &str) -> bool {
        codewhale_secrets::Secrets::auto_detect()
            .get(key)
            .unwrap()
            .is_some()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn marketplace_auth_status_does_not_treat_missing_or_corrupt_token_as_connected() {
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _home = TempPinvou3Home::new("oauth-status");
        let server_name = "yuandian-mcp-status-test";
        let url = "not-a-url";
        write_test_oauth_marketplace_files(
            server_name,
            serde_json::json!({
                "url": url,
                "scopes": ["legal"],
                "oauth": { "client_id": "test-client" }
            }),
        );

        let missing = get_marketplace_tool_auth_status("yuandian-mcp".to_string())
            .await
            .unwrap();
        assert!(missing.installed);
        assert!(missing.oauth_required);
        assert!(missing.mcp_configured);
        assert!(!missing.oauth_token_present);
        assert_eq!(missing.status, "config_installed_auth_pending");

        let key = set_test_oauth_token(server_name, url, "not-json");
        let corrupt = get_marketplace_tool_auth_status("yuandian-mcp".to_string())
            .await
            .unwrap();
        assert!(corrupt.installed);
        assert!(corrupt.oauth_required);
        assert!(corrupt.mcp_configured);
        assert!(!corrupt.oauth_token_present);
        assert_eq!(corrupt.status, "config_installed_auth_pending");
        delete_test_oauth_token_key(&key);
    }

    #[tokio::test]
    async fn forkguard_marketplace_oauth_replacement_waits_for_previous_flow() {
        let coordinator = MarketplaceOAuthLoginCoordinator::default();
        let first = coordinator.register("yuandian-mcp", "request-1").await;
        let second = coordinator.register("yuandian-mcp", "request-2").await;

        assert!(
            first.cancellation_token.is_cancelled(),
            "registering a replacement must cancel the older OAuth flow"
        );
        assert!(
            coordinator.is_current("yuandian-mcp", "request-2").await,
            "the replacement must own the tool slot"
        );

        coordinator
            .finish("yuandian-mcp", "request-1", first.completion_sender)
            .await;
        wait_for_oauth_completion(
            second
                .previous_completion
                .expect("replacement should wait for the previous flow"),
        )
        .await;

        assert!(!second.cancellation_token.is_cancelled());
        coordinator
            .finish("yuandian-mcp", "request-2", second.completion_sender)
            .await;
        assert!(!coordinator.is_current("yuandian-mcp", "request-2").await);
    }

    #[tokio::test]
    async fn forkguard_marketplace_oauth_cancel_waits_until_flow_finishes() {
        let coordinator = std::sync::Arc::new(MarketplaceOAuthLoginCoordinator::default());
        let registration = coordinator.register("yuandian-mcp", "request-1").await;
        let cancellation_token = registration.cancellation_token.clone();
        let cancel_coordinator = std::sync::Arc::clone(&coordinator);
        let cancel_task =
            tokio::spawn(
                async move { cancel_coordinator.cancel("yuandian-mcp", "request-1").await },
            );
        tokio::task::yield_now().await;

        assert!(cancellation_token.is_cancelled());
        assert!(
            !cancel_task.is_finished(),
            "cancel must not return before the OAuth future has stopped"
        );

        coordinator
            .finish("yuandian-mcp", "request-1", registration.completion_sender)
            .await;
        assert!(cancel_task.await.unwrap());
        let newer = coordinator.register("yuandian-mcp", "request-2").await;
        assert!(
            !coordinator.cancel("yuandian-mcp", "stale-request").await,
            "a stale request id must not cancel a newer flow"
        );
        assert!(!newer.cancellation_token.is_cancelled());
        coordinator
            .finish("yuandian-mcp", "request-2", newer.completion_sender)
            .await;
    }

    #[tokio::test]
    async fn forkguard_marketplace_oauth_remembers_cancel_before_register() {
        let coordinator = MarketplaceOAuthLoginCoordinator::default();
        assert!(
            coordinator
                .cancel("yuandian-mcp", "request-before-register")
                .await
        );

        let registration = coordinator
            .register("yuandian-mcp", "request-before-register")
            .await;
        assert!(
            registration.cancellation_token.is_cancelled(),
            "a fast UI cancel must stop an OAuth command even if it registers later"
        );
        coordinator
            .finish(
                "yuandian-mcp",
                "request-before-register",
                registration.completion_sender,
            )
            .await;
    }

    #[test]
    fn uninstall_marketplace_tool_deletes_oauth_token_before_mcp_config() {
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _home = TempPinvou3Home::new("oauth-uninstall");
        let server_name = "yuandian-mcp-uninstall-test";
        let url = "not-a-url";
        write_test_oauth_marketplace_files(
            server_name,
            serde_json::json!({
                "url": url,
                "scopes": ["legal"],
                "oauth": { "client_id": "test-client" }
            }),
        );
        let key = set_test_oauth_token(server_name, url, "not-json");
        assert!(test_oauth_token_exists(&key));

        uninstall_marketplace_tool("yuandian-mcp".to_string()).unwrap();

        assert!(!test_oauth_token_exists(&key));
        assert!(!crate::bridge::marketplace::MarketplaceManager::new()
            .installed_ids()
            .contains(&"yuandian-mcp".to_string()));
        let mcp_content = std::fs::read_to_string(crate::bridge::paths::mcp_config_path()).unwrap();
        let mcp: serde_json::Value = serde_json::from_str(&mcp_content).unwrap();
        assert!(mcp["servers"].get(server_name).is_none());
    }

    #[test]
    fn uninstall_marketplace_tool_aborts_if_oauth_token_delete_fails() {
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _home = TempPinvou3Home::new("oauth-uninstall-error");
        let server_name = "yuandian-mcp-uninstall-error-test";
        write_test_oauth_marketplace_files(
            server_name,
            serde_json::json!({
                "scopes": ["legal"],
                "oauth": { "client_id": "test-client" }
            }),
        );

        let err = uninstall_marketplace_tool("yuandian-mcp".to_string()).unwrap_err();
        assert!(err.contains("删除 MCP OAuth token 失败"));
        assert!(crate::bridge::marketplace::MarketplaceManager::new()
            .installed_ids()
            .contains(&"yuandian-mcp".to_string()));
        let mcp_content = std::fs::read_to_string(crate::bridge::paths::mcp_config_path()).unwrap();
        let mcp: serde_json::Value = serde_json::from_str(&mcp_content).unwrap();
        assert!(mcp["servers"].get(server_name).is_some());
    }

    struct TestPinvouHome {
        root: std::path::PathBuf,
        previous: Option<String>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for TestPinvouHome {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var("PINVOU3_HOME", value),
                None => std::env::remove_var("PINVOU3_HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// 管理一个测试期间临时设置的 PINVOU3_E2E 环境变量,drop 时还原。
    /// verify_upload 是 e2e 专用命令,生产构建(env 未置 1)必须直接拒绝,不得读盘。
    struct TestE2EFlag {
        previous: Option<String>,
    }
    impl TestE2EFlag {
        fn enable() -> Self {
            let previous = std::env::var("PINVOU3_E2E").ok();
            std::env::set_var("PINVOU3_E2E", "1");
            Self { previous }
        }
    }
    impl Drop for TestE2EFlag {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var("PINVOU3_E2E", value),
                None => std::env::remove_var("PINVOU3_E2E"),
            }
        }
    }

    #[tokio::test]
    async fn verify_upload_refuses_in_production_env() {
        // 生产路径:PINVOU3_E2E 未置。即使文件真的落盘也必须拒绝,且错误里
        // 不得出现 home 路径(防止向 webview 泄露用户名/目录布局)。
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // 显式清空 PINVOU3_E2E 模拟生产构建,RAII 保证测试结束后还原。
        let _restore = RemoveE2EOnDrop::clear();

        let err = verify_upload("up_testprod".to_string()).await.unwrap_err();
        assert!(
            !err.contains(".pinvou3") && !err.contains('/') && !err.contains('\\'),
            "生产环境错误不得泄露 home 路径,实际={err}"
        );
        assert!(
            err.contains("e2e") || err.contains("E2E") || err.contains("disabled"),
            "应明确说明未启用,实际={err}"
        );
    }

    struct RemoveE2EOnDrop {
        previous: Option<String>,
    }
    impl RemoveE2EOnDrop {
        fn clear() -> Self {
            let previous = std::env::var("PINVOU3_E2E").ok();
            std::env::remove_var("PINVOU3_E2E");
            Self { previous }
        }
    }
    impl Drop for RemoveE2EOnDrop {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(v) => std::env::set_var("PINVOU3_E2E", v),
                None => std::env::remove_var("PINVOU3_E2E"),
            }
        }
    }

    #[tokio::test]
    async fn verify_upload_returns_sha256_when_e2e_enabled_and_not_leak_path() {
        let _home = test_pinvou_home("verify-upload-e2e");
        let _e2e = TestE2EFlag::enable();

        let upload_dir = crate::bridge::paths::pinvou3_home()
            .join("uploads")
            .join("up_ok1");
        std::fs::create_dir_all(&upload_dir).unwrap();
        let data = b"hello verify_upload";
        std::fs::write(upload_dir.join("data.bin"), data).unwrap();

        let out = verify_upload("up_ok1".to_string()).await.unwrap();
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(data);
        let expected = format!("{:x}", h.finalize());
        assert_eq!(out.sha256, expected);
        assert_eq!(out.byte_size, data.len() as u64);
    }

    #[tokio::test]
    async fn verify_upload_missing_file_does_not_leak_path_or_distinguish_errors() {
        let _home = test_pinvou_home("verify-upload-missing");
        let _e2e = TestE2EFlag::enable();

        // 文件不存在:错误消息必须收敛为单一不透明文案,不得回显绝对路径,也不得
        // 把 NotFound 等 io::ErrorKind 暴露给调用方(存在性 oracle)。
        let err = verify_upload("up_missing1".to_string()).await.unwrap_err();
        assert!(
            !err.contains(".pinvou3") && !err.contains('/') && !err.contains('\\'),
            "缺失文件错误不得泄露 home 路径,实际={err}"
        );
        assert!(
            !err.contains("No such file") && !err.to_lowercase().contains("not found"),
            "不得区分 NotFound 与其它 io 错误(存在性 oracle),实际={err}"
        );
    }

    fn test_pinvou_home(tag: &str) -> TestPinvouHome {
        let guard = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = std::env::temp_dir().join(format!("{tag}-{}", std::process::id()));
        let previous = std::env::var("PINVOU3_HOME").ok();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("PINVOU3_HOME", &root);
        TestPinvouHome {
            root,
            previous,
            _guard: guard,
        }
    }

    fn session_artifact_path(session_id: &str, name: &str) -> std::path::PathBuf {
        let dir = crate::bridge::paths::session_artifacts_dir(session_id);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn direct_skill_reinstall_reapplies_persisted_disable() {
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = std::env::temp_dir().join(format!(
            "pinvou3-skill-reinstall-test-{}",
            std::process::id()
        ));
        let previous = std::env::var("PINVOU3_HOME").ok();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("PINVOU3_HOME", &root);

        crate::bridge::marketplace::save_disabled_connectors(&["skill:visualizer".to_string()]);
        install_marketplace_skill_sync("visualizer").unwrap();
        assert!(deepseek_tui::skills::is_skill_disabled("visualizer"));

        uninstall_marketplace_skill_sync("visualizer").unwrap();
        assert!(
            !deepseek_tui::skills::is_skill_disabled("visualizer"),
            "卸载后底座运行态不应保留不存在的 skill"
        );

        // disabled_connectors.json 仍保留用户的关闭选择。重装命令必须主动刷新，
        // 不能等用户再切一次 composer 开关。
        install_marketplace_skill_sync("visualizer").unwrap();
        assert!(
            deepseek_tui::skills::is_skill_disabled("visualizer"),
            "重装后 UI 的关闭状态必须与底座运行态一致"
        );

        crate::bridge::marketplace::save_disabled_connectors(&[]);
        crate::bridge::skill_marketplace::refresh_disabled_skills();
        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn file_url_from_path_encodes_local_artifact_paths() {
        let tmp =
            std::env::temp_dir().join(format!("pinvou3 file-url test {}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("中文 page.html");
        std::fs::write(&path, "<!doctype html>").unwrap();

        let canon = std::fs::canonicalize(&path).unwrap();
        let url = file_url_from_path(&canon).unwrap();
        let text = url.as_str();

        assert_eq!(url.scheme(), "file");
        assert!(text.starts_with("file://"), "unexpected file URL: {text}");
        assert!(
            !text.contains('\\'),
            "file URL must not contain backslashes: {text}"
        );
        assert!(
            !text.contains(r"\\?\"),
            "file URL must not contain verbatim prefix: {text}"
        );
        assert!(
            text.contains("%20"),
            "spaces should be percent-encoded: {text}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_artifact_text_allows_markdown() {
        let _home = test_pinvou_home("pinvou3-md-write-test");
        let md = session_artifact_path("s1", "note.md");
        std::fs::write(&md, "# Old\n").unwrap();

        write_artifact_text_impl(md.to_str().unwrap(), "# New\n\nBody").unwrap();

        assert_eq!(std::fs::read_to_string(&md).unwrap(), "# New\n\nBody");
    }

    #[test]
    fn write_artifact_text_allows_markdown_extension() {
        let _home = test_pinvou_home("pinvou3-markdown-write-test");
        let md = session_artifact_path("s1", "note.markdown");
        std::fs::write(&md, "old").unwrap();

        write_artifact_text_impl(md.to_str().unwrap(), "new").unwrap();

        assert_eq!(std::fs::read_to_string(&md).unwrap(), "new");
    }

    #[test]
    fn write_artifact_text_blocks_non_markdown() {
        let _home = test_pinvou_home("pinvou3-non-md-write-test");
        let txt = session_artifact_path("s1", "note.txt");
        std::fs::write(&txt, "old").unwrap();

        let err = write_artifact_text_impl(txt.to_str().unwrap(), "new").unwrap_err();

        assert!(err.contains("only markdown artifacts"));
        assert_eq!(std::fs::read_to_string(&txt).unwrap(), "old");
    }

    #[test]
    fn write_artifact_text_blocks_markdown_outside_session_storage() {
        let _home = test_pinvou_home("pinvou3-md-outside-session-test");
        let outside =
            std::env::temp_dir().join(format!("pinvou3-md-outside-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        let md = outside.join("note.md");
        std::fs::write(&md, "old").unwrap();

        let err = write_artifact_text_impl(md.to_str().unwrap(), "new").unwrap_err();

        assert!(err.contains("outside session storage"));
        assert_eq!(std::fs::read_to_string(&md).unwrap(), "old");
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn write_artifact_text_blocks_sensitive_path() {
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-sensitive-md-write-test-{}",
            std::process::id()
        ));
        let sensitive_dir = tmp.join(".ssh");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&sensitive_dir).unwrap();
        let md = sensitive_dir.join("note.md");
        std::fs::write(&md, "old").unwrap();

        let err = write_artifact_text_impl(md.to_str().unwrap(), "new").unwrap_err();

        assert!(err.contains("sensitive component"));
        assert_eq!(std::fs::read_to_string(&md).unwrap(), "old");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_artifact_text_requires_existing_file() {
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-missing-md-write-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let md = tmp.join("missing.md");

        let err = write_artifact_text_impl(md.to_str().unwrap(), "new").unwrap_err();

        assert!(err.contains("not a file"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_artifact_text_blocks_directory_target() {
        let _home = test_pinvou_home("pinvou3-md-dir-write-test");
        let md_dir = session_artifact_path("s1", "note.md");
        std::fs::create_dir_all(&md_dir).unwrap();

        let err = write_artifact_text_impl(md_dir.to_str().unwrap(), "new").unwrap_err();

        assert!(err.contains("not a file"));
        assert!(md_dir.is_dir());
    }

    #[test]
    fn write_artifact_text_blocks_oversized_content() {
        let _home = test_pinvou_home("pinvou3-md-large-write-test");
        let md = session_artifact_path("s1", "note.md");
        std::fs::write(&md, "old").unwrap();
        let content = "x".repeat(MAX_EDITABLE_MARKDOWN_BYTES + 1);

        let err = write_artifact_text_impl(md.to_str().unwrap(), &content).unwrap_err();

        assert!(err.contains("too large"));
        assert_eq!(std::fs::read_to_string(&md).unwrap(), "old");
    }

    #[test]
    fn write_artifact_text_preserves_utf8_content() {
        let _home = test_pinvou_home("pinvou3-md-utf8-write-test");
        let md = session_artifact_path("s1", "note.md");
        std::fs::write(&md, "old").unwrap();
        let content = "# 标题\n\n| 名称 | 状态 |\n| --- | --- |\n| Alpha | 进行中 |\n\n```text\n保留中文\n```";

        write_artifact_text_impl(md.to_str().unwrap(), content).unwrap();

        assert_eq!(std::fs::read_to_string(&md).unwrap(), content);
    }

    #[test]
    fn write_artifact_text_cleans_backup_file_after_success() {
        let _home = test_pinvou_home("pinvou3-md-backup-clean-test");
        let md = session_artifact_path("s1", "note.md");
        std::fs::write(&md, "old").unwrap();

        write_artifact_text_impl(md.to_str().unwrap(), "new").unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(md.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".bak-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "backup files left behind: {leftovers:?}"
        );
        assert_eq!(std::fs::read_to_string(&md).unwrap(), "new");
    }

    #[test]
    fn write_artifact_text_cleans_temp_file_on_error() {
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-temp-clean-write-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let md = tmp.join("note.md");
        std::fs::create_dir_all(&md).unwrap();

        let err = atomic_write_utf8(&md, "new").unwrap_err();

        assert!(!err.to_string().is_empty());
        let leftovers: Vec<_> = std::fs::read_dir(&tmp)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// accept_plan 切 Yolo 后注入的执行指令必须裹住方案全文 + 带"立即执行"信号——
    /// 否则切了模式但 AI 收到一句空指令,不知道要执行什么(切了白切)。
    #[test]
    fn accept_plan_instruction_embeds_full_plan() {
        let plan = "1. 建目录\n2. 写 index.html\n3. 起本地服务器";
        let msg = accept_plan_instruction(plan);
        assert!(msg.contains(plan), "注入指令丢了方案全文");
        assert!(msg.contains("立即开始执行"), "缺少明确执行信号");
    }

    /// 挂集时 Self-RAG 引导:含知识集名 + 必调 kb_search + 无依据说不知道;空名兜底。
    #[test]
    fn agentic_guide_mentions_collection_and_kb_search() {
        let g = build_kb_agentic_guide(Some("硬件资料"));
        assert!(g.contains("《硬件资料》"));
        assert!(g.contains("kb_search"));
        assert!(g.contains("绝不凭记忆编造"));
        assert!(build_kb_agentic_guide(None).contains("《本地知识集》"));
    }

    #[test]
    fn parses_local_asr_plain_text_output() {
        let text = parse_local_asr_text("hello from voice\n", "").expect("plain text");
        assert_eq!(text, "hello from voice");
    }

    #[test]
    fn parses_local_asr_list_output() {
        let text = parse_local_asr_text("[INFO] loading\n['hello from voice']\n", "")
            .expect("list output");
        assert_eq!(text, "hello from voice");
    }

    /// present_artifact 漂工具名失败时,成品卡兜底用 write_file 的相对 path
    /// (如 `snake-game.html`),点 Open 必须先按 session workspace 解析成绝对路径,
    /// 否则 `validate_user_path` 直接拒「path must be absolute」。
    #[test]
    fn resolve_artifact_path_relative_joins_active_workspace() {
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-resolve-test");
        let store = SessionStore::boot().expect("boot");

        // 无 active session 且无显式 session → 相对路径原样返回(行为同旧版)
        assert_eq!(
            resolve_artifact_path("snake-game.html", None, &store).expect("no active workspace"),
            "snake-game.html"
        );

        // 有 active session、无显式 session → 回退 active 的 workspace
        store.set_active(Some("sess-1".into()));
        let want = crate::bridge::paths::session_workspace_dir("sess-1")
            .join("snake-game.html")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            resolve_artifact_path("snake-game.html", None, &store).expect("active workspace"),
            want
        );

        // 显式 session **优先**于 active:卡片自带 session 才能跨会话切换稳定解析
        // (active 停在切走时去的会话也不影响)。这是本次跨 session「打不开」的修复点。
        let want_explicit = crate::bridge::paths::session_workspace_dir("sess-owner")
            .join("snake-game.html")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            resolve_artifact_path("snake-game.html", Some("sess-owner"), &store)
                .expect("explicit workspace"),
            want_explicit
        );

        // 绝对路径原样返回,无视 session(present_artifact 成功 / 产物面板给的已是绝对)
        #[cfg(unix)]
        assert_eq!(
            resolve_artifact_path(
                "/home/u/.pinvou3/sessions/x/workspace/a.html",
                Some("sess-owner"),
                &store
            )
            .expect("absolute artifact"),
            "/home/u/.pinvou3/sessions/x/workspace/a.html"
        );
    }

    #[test]
    fn scheduled_attachment_staging_and_artifact_resolution_use_task_workspace() {
        use crate::bridge::sessions::{ScheduledRunMode, ScheduledRunProfile};

        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = std::env::temp_dir().join(format!(
            "pinvou3-scheduled-workspace-test-{}",
            std::process::id()
        ));
        let previous = std::env::var("PINVOU3_HOME").ok();
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("PINVOU3_HOME", &root);
        let scheduled_root = root.join("scheduled");
        std::fs::create_dir_all(&root).expect("test root");
        let source = root.join("source.png");
        std::fs::write(&source, b"\x89PNG\r\n\x1a\nfake-bytes").expect("image source");
        let store =
            SessionStore::boot_with_scheduled_root(scheduled_root.clone()).expect("session store");
        let scheduled = store
            .create_scheduled_run(ScheduledRunProfile {
                task_id: "workspace-task".to_string(),
                model: "scheduled-model".to_string(),
                model_id: None,
                workspace: root.join("ignored-task-workspace"),
                mode: ScheduledRunMode::Agent,
                allow_shell: false,
                trust_mode: false,
                auto_approve: false,
            })
            .expect("scheduled session");
        // transcript 覆盖类命令仍拒绝 scheduled 会话(引擎独占持久化)。
        let manage_error =
            ensure_chat_session(&store, &scheduled.metadata.id, "save_session_messages")
                .expect_err("scheduled runs must reject UI transcript overwrites");
        assert!(manage_error.contains("scheduled-run sessions are managed from Scheduled"));
        let locked = store
            .execution_workspace(&scheduled.metadata.id)
            .expect("locked scheduled workspace");
        // 每次运行对话独立，同一 automation 共享自己的工作间；输入路径不会生效。
        assert_eq!(
            locked,
            scheduled_root.join("workspace-task").join("workspace")
        );
        let workspace = locked.clone();
        std::fs::create_dir_all(&workspace).expect("session workspace");
        let staged_dir = format!(
            ".pinvou3/scheduled-attachments/{}/attachments",
            scheduled.metadata.id
        );
        let prompt = build_message_with_attachments_in_dir(
            "inspect".to_string(),
            vec![crate::file_ingest::ingest(&source)],
            &locked,
            &staged_dir,
        );

        let staged_relative = format!("{staged_dir}/source.png");
        let large_text_relative = format!("{staged_dir}/big.txt");
        let large_text_prompt = build_message_with_attachments_in_dir(
            "inspect all".to_string(),
            vec![mk_attachment("text", "big.log", 9_000, 50_000)],
            &locked,
            &staged_dir,
        );
        let report = workspace.join("report.md");
        std::fs::write(&report, "scheduled result").expect("scheduled workspace artifact");

        assert_eq!(locked, workspace);
        assert!(workspace.join(&staged_relative).exists());
        assert!(prompt.contains(&staged_relative));
        assert!(workspace.join(&large_text_relative).exists());
        assert!(large_text_prompt.contains(&large_text_relative));
        assert!(
            list_workspace_files_for_session(&scheduled.metadata.id, &store)
                .expect("scan scheduled workspace")
                .contains(&report.to_string_lossy().into_owned()),
            "workspace reconciliation must scan the configured execution workspace"
        );
        assert_eq!(
            resolve_artifact_path("report.md", Some(&scheduled.metadata.id), &store)
                .expect("scheduled artifact workspace"),
            workspace.join("report.md").to_string_lossy().into_owned()
        );

        drop(store);
        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(root);
    }

    /// 定时运行会话按 SessionKind 分发后，重命名/置顶/归档复用 Session 元数据
    /// (rename_session / set_session_pinned / set_session_archived 的真实后端路径)，
    /// 而删除仍然只能走 automation 联动，不允许把 sched-* 当普通会话直删。
    #[test]
    fn scheduled_session_metadata_dispatch_supports_rename_pin_archive() {
        use crate::bridge::sessions::{ScheduledRunMode, ScheduledRunProfile};

        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = std::env::temp_dir().join(format!(
            "pinvou3-scheduled-metadata-test-{}",
            std::process::id()
        ));
        let previous = std::env::var("PINVOU3_HOME").ok();
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("PINVOU3_HOME", &root);
        let store =
            SessionStore::boot_with_scheduled_root(root.join("scheduled")).expect("session store");
        let scheduled = store
            .create_scheduled_run(ScheduledRunProfile {
                task_id: "metadata-task".to_string(),
                model: "scheduled-model".to_string(),
                model_id: None,
                workspace: root.join("ignored"),
                mode: ScheduledRunMode::Yolo,
                allow_shell: false,
                trust_mode: true,
                auto_approve: true,
            })
            .expect("scheduled session");
        let id = scheduled.metadata.id.clone();
        assert!(matches!(
            store.session_kind(&id),
            Ok(SessionKind::ScheduledRun)
        ));

        // rename_session 路径：set_title 对 scheduled 会话生效并落盘。
        store
            .set_title(&id, "重命名后的定时运行".to_string())
            .expect("rename");
        assert_eq!(
            store.load(&id).expect("reload").metadata.title,
            "重命名后的定时运行"
        );

        // set_session_pinned 路径：共用置顶表。
        store.set_pinned(&id, true);
        assert!(store.is_pinned(&id));
        assert!(store.pinned_at(&id).is_some());

        // set_session_archived 路径：共用收起表,且归档列表能列出 sched-* 会话。
        store.set_hidden(&id, true);
        assert!(store.is_hidden(&id));
        assert!(store
            .list_scheduled()
            .expect("list scheduled")
            .iter()
            .any(|metadata| metadata.id == id));
        // 收起会强制取消置顶(与普通会话一致)。
        assert!(!store.is_pinned(&id));
        store.set_hidden(&id, false);
        assert!(!store.is_hidden(&id));

        // 删除不允许绕过 automation 联动直删。
        let delete_error = store
            .delete(&id)
            .expect_err("scheduled sessions must not be deleted as ordinary chats");
        assert!(delete_error
            .to_string()
            .contains("deleted through their automation"));

        drop(store);
        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ordinary_execution_workspace_behavior_is_unchanged() {
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = std::env::temp_dir().join(format!(
            "pinvou3-chat-workspace-test-{}",
            std::process::id()
        ));
        let previous = std::env::var("PINVOU3_HOME").ok();
        let _ = std::fs::remove_dir_all(&root);
        std::env::set_var("PINVOU3_HOME", &root);
        let store = SessionStore::boot().expect("session store");
        let chat = store
            .create_new(
                "chat-model".to_string(),
                None,
                root.join("legacy-metadata-workspace"),
            )
            .expect("ordinary chat");
        let expected = crate::bridge::paths::session_workspace_dir(&chat.metadata.id);

        assert_eq!(
            store
                .execution_workspace(&chat.metadata.id)
                .expect("ordinary execution workspace"),
            expected
        );
        assert_eq!(
            resolve_artifact_path("report.md", Some(&chat.metadata.id), &store)
                .expect("ordinary artifact workspace"),
            expected.join("report.md").to_string_lossy().into_owned()
        );

        drop(store);
        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(root);
    }

    /// merge_resolutions 锁住实测 bug:勾选写进 sidecar 的 resolution 不能被后续不含
    /// resolution 的全量 save(典型=核账 record 的原始快照)覆盖,否则核账无法跳过「接受现状」。
    #[test]
    fn merge_preserves_old_resolution_when_new_missing() {
        let old = serde_json::json!([
            {"pos":10,"review":{"issues":[{"text":"a","resolution":"modify"},{"text":"b","resolution":"accept"}],"recommendations":[]}}
        ]);
        // 核账 record 的 new:同一条 review 的 issues 不带 resolution(原始快照态)+ 追加 pos=44
        let new = serde_json::json!([
            {"pos":10,"review":{"issues":[{"text":"a"},{"text":"b"}],"recommendations":[]}},
            {"pos":44,"review":{"issues":[],"recommendations":[]}}
        ]);
        let merged = merge_resolutions(old, new);
        assert_eq!(merged[0]["review"]["issues"][0]["resolution"], "modify");
        assert_eq!(merged[0]["review"]["issues"][1]["resolution"], "accept");
    }

    #[test]
    fn merge_respects_new_resolution_for_changed_verdict() {
        // Boss 改裁决:new 自带新值,不被 old 覆盖
        let old = serde_json::json!([{"pos":10,"review":{"issues":[{"text":"a","resolution":"modify"}],"recommendations":[]}}]);
        let new = serde_json::json!([{"pos":10,"review":{"issues":[{"text":"a","resolution":"accept"}],"recommendations":[]}}]);
        let merged = merge_resolutions(old, new);
        assert_eq!(merged[0]["review"]["issues"][0]["resolution"], "accept");
    }

    #[test]
    fn merge_covers_recommendations_field() {
        let old = serde_json::json!([{"pos":1,"review":{"issues":[],"recommendations":[{"topic":"t","resolution":"modify"}]}}]);
        let new =
            serde_json::json!([{"pos":1,"review":{"issues":[],"recommendations":[{"topic":"t"}]}}]);
        let merged = merge_resolutions(old, new);
        assert_eq!(
            merged[0]["review"]["recommendations"][0]["resolution"],
            "modify"
        );
    }

    /// `open_external_url` 必须只放 metaso.cn / open.bochaai.com / console.bce.baidu.com /
    /// app.tavily.com,任何其他 host / 任何其他 scheme(http、file、javascript)都立即
    /// reject——这是前端 webview 万一被 XSS 的最后一道防线,不许扩大白名单不加测试。
    #[tokio::test]
    async fn open_external_url_rejects_off_allowlist_targets() {
        let rejected = [
            "http://metaso.cn/",                       // 非 https
            "https://evil.example.com/",               // host 不在白名单
            "https://metaso.cn.evil.com/",             // 子域钓鱼
            "https://console.bce.baidu.com.evil.com/", // 百度子域钓鱼
            "https://app.tavily.com.evil.com/",        // tavily 子域钓鱼
            "https://bce.baidu.com/",                  // 非 console 子域,不放行
            "javascript:alert(1)",                     // js scheme
            "file:///etc/passwd",                      // file scheme
            "https://google.com/",                     // 任何第三方域
            "",                                        // 空串
            "metaso.cn/",                              // 缺 scheme
        ];
        for url in rejected {
            let err = open_external_url(url.to_string()).await.err();
            assert!(err.is_some(), "must reject URL: {url:?}");
            assert!(
                err.as_deref().unwrap().contains("allowlist"),
                "reject reason should name allowlist for {url:?}, got {err:?}"
            );
        }
    }

    /// 扩 EXTERNAL_URL_ALLOWLIST 必须加测试:目标域名放行,仿冒/非 https 拒绝。
    #[test]
    fn external_allowlist_allows_known_targets_rejects_lookalikes() {
        assert!(url_in_external_allowlist("https://obsidian.md/download"));
        assert!(url_in_external_allowlist("https://metaso.cn/"));
        assert!(!url_in_external_allowlist(
            "https://open.chineselaw.com/oauth/authorize"
        ));
        assert!(!url_in_external_allowlist(
            "https://passport.legalmind.cn/ssologin?appId=apiplatform"
        ));
        assert!(url_in_external_allowlist(
            "https://open.zhihuiya.com/dashboard/api-keys"
        ));
        assert!(!url_in_external_allowlist("https://obsidian.md.evil.com/"));
        assert!(!url_in_external_allowlist(
            "https://open.zhihuiya.com.evil.com/dashboard/api-keys"
        ));
        assert!(!url_in_external_allowlist("http://obsidian.md/"));
        assert!(!url_in_external_allowlist("http://open.zhihuiya.com/"));
        assert!(!url_in_external_allowlist("https://evil.example.com/"));
    }

    /// detect_obsidian 选库规则:open:true 优先,否则 ts 最大;空/无 vaults → None;容忍 BOM。
    #[test]
    fn pick_vault_path_prefers_open_then_latest() {
        let j = r#"{"vaults":{"a":{"path":"/A","ts":100},"b":{"path":"/B","ts":1,"open":true}}}"#;
        assert_eq!(pick_vault_path(j).as_deref(), Some("/B"));
        let j = r#"{"vaults":{"a":{"path":"/A","ts":100},"b":{"path":"/B","ts":9}}}"#;
        assert_eq!(pick_vault_path(j).as_deref(), Some("/A"));
        assert_eq!(pick_vault_path(r#"{"vaults":{}}"#), None);
        let j = "\u{feff}{\"vaults\":{\"a\":{\"path\":\"/A\",\"ts\":1,\"open\":true}}}";
        assert_eq!(pick_vault_path(j).as_deref(), Some("/A"));
    }

    /// 成品归属推断:成品词出现在质检节时必须归被审对象,不归审核者
    /// (天真就近归因实测把成品判给 xingbu——刑部节里"对礼部整合的最终报告
    /// 进行审核"的关键词全在审核者章节内)。
    #[test]
    fn infer_product_bu_attributes_to_audited_not_auditor() {
        let report = "\
## 各部成果\n\n\
### 4. 礼部（libu_1.md）—— 报告撰写与方案整合\n\n\
核心产出：将各部数据整合为完整的研究报告\n\n\
### 5. 刑部（xingbu_1.md）—— 方案质量审核验收\n\n\
核心产出：对礼部整合的最终报告进行全面质量审核\n\n\
## 结果对账\n\n\
| 具体方案交付 | ✅ 达成 | 礼部报告提供三套完整方案 |\n";
        let (bu, score) = infer_product_bu(report).expect("应有推断结果");
        assert_eq!(bu, "libu", "成品应归被审的礼部,不归审核者刑部");
        assert!(score >= 8, "多路信号应汇聚到可信阈值,实得 {score}");
    }

    /// libu_ 前缀不得误吃 libu_renshi_N.md(后随数字才算点名)。
    #[test]
    fn infer_product_bu_no_prefix_confusion() {
        let report = "### 吏部（libu_renshi_1.md）—— 流程规范\n\n整合完整的最终成品汇总\n";
        let (bu, _) = infer_product_bu(report).expect("应有推断结果");
        assert_eq!(bu, "libu_renshi");
    }

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
    #[cfg(unix)]
    fn validate_user_path_blocks_etc_shadow() {
        let result = validate_user_path("/etc/shadow");
        assert!(result.is_err(), "/etc/shadow 必须拒绝, got {result:?}");
        assert!(result.unwrap_err().contains("system-sensitive"));
    }

    /// 图片附件 → build_message_with_attachments 的视觉通路:验证图被拷进 workspace
    /// 的 attachments/ 且 prompt 引导 LLM 调 image_analyze(不再走 OCR)。
    /// 用临时 png + 临时 workspace,不依赖外部文件,正常跑(非 ignore)。
    #[test]
    fn image_attachment_stages_and_guides_image_analyze() {
        let tmp = std::env::temp_dir().join(format!("pinvou3-vis-test-{}", std::process::id()));
        let ws = tmp.join("workspace");
        std::fs::create_dir_all(&ws).expect("建 workspace");
        let src = tmp.join("shot.png");
        std::fs::write(&src, b"\x89PNG\r\n\x1a\nfake-bytes").expect("写假 png");

        let r = crate::file_ingest::ingest(&src);
        assert_eq!(r.kind, "image", "应识别为 image");
        assert!(
            r.markdown.is_none(),
            "图片不再预解析出 markdown(OCR 已移除)"
        );

        let prompt = build_message_with_attachments("这张图里画了什么？".to_string(), vec![r], &ws);
        assert!(
            prompt.contains("image_analyze"),
            "prompt 应引导调 image_analyze"
        );
        assert!(
            prompt.contains("attachments/shot.png"),
            "prompt 应给出 workspace 相对路径"
        );
        assert!(
            ws.join("attachments/shot.png").exists(),
            "图片应被拷进 workspace 的 attachments/"
        );
        assert!(!prompt.contains("没有视觉能力"), "不应再出现无视觉提示");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn remote_attachment_source_survives_upload_temp_cleanup() {
        let tmp =
            std::env::temp_dir().join(format!("pinvou3-remote-source-test-{}", std::process::id()));
        let ws = tmp.join("workspace");
        std::fs::create_dir_all(&ws).expect("建 workspace");

        let image_src = tmp.join("remote.png");
        std::fs::write(&image_src, b"\x89PNG\r\n\x1a\nremote-image").unwrap();
        let mut image = crate::file_ingest::ingest(&image_src);
        let staged_image =
            stage_remote_attachment_source(image_src.to_str().unwrap(), &image.basename, &ws)
                .expect("暂存远控图片");
        image.path = staged_image.to_string_lossy().to_string();
        std::fs::remove_file(&image_src).unwrap();
        let image_prompt = build_message_with_attachments("看图".into(), vec![image], &ws);
        assert!(image_prompt.contains("image_analyze"));
        assert!(ws.join("attachments/remote.png").exists());

        let text_src = tmp.join("remote.txt");
        std::fs::write(&text_src, "远控大文本\n".repeat(20_000)).unwrap();
        let mut text = crate::file_ingest::ingest(&text_src);
        assert!(text.token_estimate > ATTACH_INLINE_MAX_TOKENS);
        let staged_text =
            stage_remote_attachment_source(text_src.to_str().unwrap(), &text.basename, &ws)
                .expect("暂存远控大文本");
        text.path = staged_text.to_string_lossy().to_string();
        std::fs::remove_file(&text_src).unwrap();
        let text_prompt = build_message_with_attachments("查全文".into(), vec![text], &ws);
        assert!(staged_text.exists());
        assert!(text_prompt.contains(&staged_text.to_string_lossy().to_string()));
        assert!(!text_prompt.contains("此文件过大无法内嵌,且转换产物落盘失败"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 造一个指定 kind / token 估算的 IngestResult,markdown 是 `rows` 行可定位文本。
    fn mk_attachment(
        kind: &str,
        basename: &str,
        rows: usize,
        tokens: u32,
    ) -> crate::file_ingest::IngestResult {
        let md: String = (1..=rows).map(|i| format!("row-{i},value-{i}\n")).collect();
        crate::file_ingest::IngestResult {
            kind: kind.into(),
            basename: basename.into(),
            path: format!("/tmp/fake/{basename}"),
            markdown: Some(md),
            token_estimate: tokens,
            byte_size: 1,
            warning: None,
        }
    }

    fn mk_test_ws(tag: &str) -> std::path::PathBuf {
        let ws =
            std::env::temp_dir().join(format!("pinvou3-attach-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        std::fs::create_dir_all(&ws).expect("建 workspace");
        ws
    }

    #[test]
    fn staged_attachment_basename_rejects_path_traversal_and_drive_prefixes() {
        for invalid in ["", ".", "..", "../x", "..\\x", "/x", "C:\\x"] {
            assert!(
                validate_staged_attachment_basename(invalid).is_err(),
                "staged basename must reject {invalid:?}"
            );
        }

        for valid in ["x", "report.txt", "报告 2026.xlsx"] {
            assert!(
                validate_staged_attachment_basename(valid).is_ok(),
                "ordinary basename must remain valid: {valid:?}"
            );
        }
    }

    #[cfg(unix)]
    fn try_link_file(target: &std::path::Path, link: &std::path::Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn try_link_file(target: &std::path::Path, link: &std::path::Path) -> bool {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }

    #[cfg(not(any(unix, windows)))]
    fn try_link_file(_target: &std::path::Path, _link: &std::path::Path) -> bool {
        false
    }

    #[cfg(unix)]
    fn try_link_dir(target: &std::path::Path, link: &std::path::Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn try_link_dir(target: &std::path::Path, link: &std::path::Path) -> bool {
        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return true;
        }
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .is_ok_and(|status| status.success())
    }

    #[cfg(not(any(unix, windows)))]
    fn try_link_dir(_target: &std::path::Path, _link: &std::path::Path) -> bool {
        false
    }

    #[test]
    fn staged_targets_never_follow_preexisting_dangling_symlinks() {
        let root = mk_test_ws("target-symlink");
        let workspace = root.join("workspace");
        let outside = root.join("outside");
        let attachments = workspace.join("attachments");
        std::fs::create_dir_all(&attachments).expect("attachments");
        std::fs::create_dir_all(&outside).expect("outside");
        let source = root.join("source.png");
        std::fs::write(&source, b"safe image bytes").expect("source image");
        let escaped_image = outside.join("escaped.png");
        let image_link = attachments.join("source.png");
        if !try_link_file(&escaped_image, &image_link) {
            eprintln!("symlink creation unavailable; skipping platform-specific assertion");
            let _ = std::fs::remove_dir_all(root);
            return;
        }

        let escaped_text = outside.join("escaped.txt");
        let text_link = attachments.join("report.txt");
        if !try_link_file(&escaped_text, &text_link) {
            let _ = std::fs::remove_file(&image_link);
            let _ = std::fs::remove_dir_all(root);
            return;
        }

        let staged_image = stage_image_in_workspace(
            source.to_string_lossy().as_ref(),
            "source.png",
            &workspace,
            "attachments",
        )
        .expect("safe image fallback name");
        let staged_text =
            stage_text_in_workspace("safe text", "report.md", "txt", &workspace, "attachments")
                .expect("safe text fallback name");

        assert_eq!(staged_image, "attachments/source-1.png");
        assert_eq!(staged_text, "attachments/report-1.txt");
        assert!(!escaped_image.exists(), "image staging escaped workspace");
        assert!(!escaped_text.exists(), "text staging escaped workspace");
        assert_eq!(
            std::fs::read(workspace.join(&staged_image)).expect("staged image"),
            b"safe image bytes"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join(&staged_text)).expect("staged text"),
            "safe text"
        );

        let _ = std::fs::remove_file(image_link);
        let _ = std::fs::remove_file(text_link);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn staged_parent_chain_rejects_symlink_escape_without_touching_external_tree() {
        let root = mk_test_ws("parent-symlink");
        let workspace = root.join("workspace");
        let outside = root.join("outside");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&outside).expect("outside");
        let sentinel = outside.join("sentinel.txt");
        std::fs::write(&sentinel, "unchanged").expect("sentinel");
        let linked_parent = workspace.join("linked");
        if !try_link_dir(&outside, &linked_parent) {
            eprintln!(
                "directory symlink creation unavailable; skipping platform-specific assertion"
            );
            let _ = std::fs::remove_dir_all(root);
            return;
        }

        assert!(
            stage_text_in_workspace(
                "must stay inside",
                "report.md",
                "txt",
                &workspace,
                "linked/attachments",
            )
            .is_none(),
            "an escaping parent link must reject the stage"
        );
        assert_eq!(
            std::fs::read_to_string(&sentinel).expect("sentinel remains"),
            "unchanged"
        );
        assert!(
            !outside.join("attachments").exists(),
            "validation must happen before creating children through the link"
        );

        #[cfg(windows)]
        let _ = std::fs::remove_dir(&linked_parent);
        #[cfg(not(windows))]
        let _ = std::fs::remove_file(&linked_parent);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_staging_reserves_distinct_targets_atomically() {
        let workspace = mk_test_ws("concurrent-create-new");
        let workers = 32;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(workers));
        let mut joins = Vec::new();
        for index in 0..workers {
            let barrier = barrier.clone();
            let workspace = workspace.clone();
            joins.push(std::thread::spawn(move || {
                let content = format!("worker-{index}-{}", "x".repeat(256 * 1024));
                barrier.wait();
                stage_text_in_workspace(&content, "report.md", "txt", &workspace, "attachments")
                    .expect("concurrent stage")
            }));
        }
        let paths = joins
            .into_iter()
            .map(|join| join.join().expect("worker joins"))
            .collect::<Vec<_>>();
        let unique = paths.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(
            unique.len(),
            workers,
            "every writer needs an exclusive path"
        );
        assert!(
            paths.iter().all(|path| workspace.join(path).exists()),
            "every reserved target must contain its writer's output"
        );
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn atomic_staging_keeps_legal_image_and_text_behavior() {
        let workspace = mk_test_ws("legal-atomic-staging");
        let source = workspace.join("source.png");
        std::fs::write(&source, b"image bytes").expect("source");

        let image = stage_image_in_workspace(
            source.to_string_lossy().as_ref(),
            "safe.png",
            &workspace,
            "attachments",
        )
        .expect("image stage");
        let text =
            stage_text_in_workspace("text bytes", "safe.md", "txt", &workspace, "attachments")
                .expect("text stage");

        assert_eq!(
            std::fs::read(workspace.join(image)).unwrap(),
            b"image bytes"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join(text)).unwrap(),
            "text bytes"
        );
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn post_reservation_failure_never_unlinks_the_current_path() {
        let workspace = mk_test_ws("post-reserve-failure");
        let reserved = std::sync::Arc::new(std::sync::Mutex::new(None));
        let observed = reserved.clone();
        let result = stage_text_in_workspace_with_writer(
            "report.md",
            "txt",
            &workspace,
            "attachments",
            move |_destination, path| {
                *observed.lock().expect("capture path") = Some(path.to_path_buf());
                Err(std::io::Error::other("injected post-reservation failure"))
            },
        );
        assert!(result.is_none());
        let reserved_path = reserved
            .lock()
            .expect("reserved path")
            .clone()
            .expect("writer observed reserved path");
        assert!(
            reserved_path.exists(),
            "failure must leave the exclusively-created orphan instead of unlinking by path"
        );

        let replacement_workspace = mk_test_ws("post-reserve-replacement");
        let replacement_survived = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let replacement_flag = replacement_survived.clone();
        let result = stage_text_in_workspace_with_writer(
            "report.md",
            "txt",
            &replacement_workspace,
            "attachments",
            move |_destination, path| {
                let orphan = path.with_extension("reserved-orphan");
                if std::fs::rename(path, &orphan).is_ok() {
                    std::fs::write(path, "replacement").expect("install replacement");
                    replacement_flag.store(true, std::sync::atomic::Ordering::Release);
                }
                Err(std::io::Error::other("injected after path replacement"))
            },
        );
        assert!(result.is_none());
        if replacement_survived.load(std::sync::atomic::Ordering::Acquire) {
            assert_eq!(
                std::fs::read_to_string(replacement_workspace.join("attachments/report.txt"))
                    .expect("replacement remains"),
                "replacement"
            );
        }

        let _ = std::fs::remove_dir_all(workspace);
        let _ = std::fs::remove_dir_all(replacement_workspace);
    }

    /// 小附件维持全量内联:内容在代码块里,且明确告知无需 read_file。
    #[test]
    fn small_attachment_stays_inline() {
        let ws = mk_test_ws("inline");
        let prompt = build_message_with_attachments(
            "看下这个".into(),
            vec![mk_attachment("xlsx", "small.xlsx", 10, 100)],
            &ws,
        );
        assert!(prompt.contains("row-10,value-10"), "小附件应全量内联");
        assert!(
            prompt.contains("不需要再调 read_file"),
            "内联段应声明无需 read_file"
        );
        assert!(!ws.join("attachments").exists(), "小附件不应落盘");
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 大表格 → 落盘 CSV + 预览 + 工具引导,完整内容不进 prompt。
    #[test]
    fn large_spreadsheet_goes_path_mode() {
        let ws = mk_test_ws("xlsx");
        let a = mk_attachment("xlsx", "data.xlsx", 5000, 100_000);
        let full_md = a.markdown.clone().unwrap();
        let prompt = build_message_with_attachments("分析一下".into(), vec![a], &ws);

        assert!(
            !prompt.contains("row-5000,value-5000"),
            "完整内容不应进 prompt"
        );
        assert!(prompt.contains("row-1,value-1"), "应有开头预览");
        assert!(
            prompt.contains("attachments/data.csv"),
            "应给出落盘 CSV 相对路径"
        );
        assert!(
            prompt.contains("read_file") && prompt.contains("exec_shell"),
            "应引导工具消化"
        );
        assert!(prompt.contains("没有**嵌入"), "应声明未嵌入完整内容");
        let staged = std::fs::read_to_string(ws.join("attachments/data.csv")).expect("CSV 应落盘");
        assert_eq!(staged, full_md, "落盘内容应与转换产物一致");
        // 体量验证:prompt 远小于全量(预览+引导 vs 5000 行)
        assert!(prompt.len() < full_md.len() / 10, "prompt 应远小于全量内容");
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 大纯文本(txt/csv 原文件):直接引用原始路径,不落盘副本。
    #[test]
    fn large_text_uses_original_path() {
        let ws = mk_test_ws("text");
        let a = mk_attachment("text", "big.log", 9000, 50_000);
        let prompt = build_message_with_attachments("查错误".into(), vec![a], &ws);

        assert!(prompt.contains("/tmp/fake/big.log"), "应引用原始路径");
        assert!(!ws.join("attachments").exists(), "text 类不应落盘副本");
        assert!(
            !prompt.contains("row-9000,value-9000"),
            "完整内容不应进 prompt"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 多附件累计超预算:单个都不超 INLINE_MAX,但第三个把累计顶过 TOTAL_BUDGET → 转路径模式。
    #[test]
    fn cumulative_budget_overflows_to_path_mode() {
        let ws = mk_test_ws("budget");
        let prompt = build_message_with_attachments(
            "汇总".into(),
            vec![
                mk_attachment("docx", "a.docx", 50, 7_000),
                mk_attachment("docx", "b.docx", 60, 7_000),
                mk_attachment("docx", "c.docx", 70, 7_000),
            ],
            &ws,
        );
        assert!(prompt.contains("row-50,value-50"), "a 应内联");
        assert!(
            prompt.contains("row-60,value-60"),
            "b 应内联(累计 14K ≤ 16K)"
        );
        assert!(
            !prompt.contains("row-70,value-70"),
            "c 应转路径模式(累计 21K > 16K)"
        );
        assert!(ws.join("attachments/c.md").exists(), "c 的产物应落盘为 md");
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 为多种附件类型生成「问题 + 附件内容」的最终 prompt，写到 /tmp 供外部脚本
    /// 发真实 vLLM 验证模型能否据附件作答。依赖 /tmp/e2e_files，故 `#[ignore]`。
    #[test]
    #[ignore = "生成多类型 prompt 供 vLLM 验证"]
    fn e2e_build_llm_cases() {
        let dir = std::path::Path::new("/tmp/e2e_files");
        if !dir.exists() {
            eprintln!("跳过: 无 /tmp/e2e_files");
            return;
        }
        let cases = [
            (
                "sample.png",
                "这张图里的项目编号是多少？只回答编号。",
                "/tmp/llm_png.txt",
            ),
            (
                "scan.pdf",
                "这份文件的文号是多少？只回答文号。",
                "/tmp/llm_scan.txt",
            ),
            (
                "sample.xlsx",
                "表格里李四的金额是多少？只回答数字。",
                "/tmp/llm_xlsx.txt",
            ),
            (
                "sample.pptx",
                "演示文稿第一章讲什么？提到的编号是？",
                "/tmp/llm_pptx.txt",
            ),
            (
                "mail.eml",
                "这封邮件的主题是什么？正文里的编号是多少？",
                "/tmp/llm_eml.txt",
            ),
            (
                "bundle.zip",
                "这个压缩包里图片上的项目编号是多少？",
                "/tmp/llm_zip.txt",
            ),
        ];
        for (f, q, out) in cases {
            let p = dir.join(f);
            if !p.exists() {
                continue;
            }
            let r = crate::file_ingest::ingest(&p);
            let ws = std::env::temp_dir().join("pinvou3-e2e-ws");
            let _ = std::fs::create_dir_all(&ws);
            let prompt = build_message_with_attachments(q.to_string(), vec![r], &ws);
            std::fs::write(out, prompt).expect("写 prompt");
        }
    }
}

// ---------------------------------------------------------------------------
// 工具市场
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_marketplace_tools(
) -> Result<Vec<crate::bridge::marketplace::MarketplaceToolInfo>, String> {
    let mgr = crate::bridge::marketplace::MarketplaceManager::new();
    let tools = mgr.list_tools();
    Ok(tools)
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketplaceOAuthLoginResult {
    pub status: String,
    pub message: String,
    pub server_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketplaceAuthStatus {
    pub installed: bool,
    pub mcp_configured: bool,
    pub oauth_required: bool,
    pub oauth_token_present: bool,
    pub status: String,
    pub server_name: Option<String>,
    pub message: String,
}

#[derive(Clone)]
struct ActiveMarketplaceOAuthLogin {
    request_id: String,
    cancellation_token: tokio_util::sync::CancellationToken,
    completion: tokio::sync::watch::Receiver<bool>,
}

#[derive(Default)]
struct MarketplaceOAuthLoginCoordinator {
    state: tokio::sync::Mutex<MarketplaceOAuthLoginCoordinatorState>,
}

#[derive(Default)]
struct MarketplaceOAuthLoginCoordinatorState {
    active: std::collections::HashMap<String, ActiveMarketplaceOAuthLogin>,
    pending_cancellations: std::collections::HashMap<String, String>,
}

struct MarketplaceOAuthLoginRegistration {
    cancellation_token: tokio_util::sync::CancellationToken,
    completion_sender: tokio::sync::watch::Sender<bool>,
    previous_completion: Option<tokio::sync::watch::Receiver<bool>>,
}

impl MarketplaceOAuthLoginCoordinator {
    async fn register(&self, tool_id: &str, request_id: &str) -> MarketplaceOAuthLoginRegistration {
        let cancellation_token = tokio_util::sync::CancellationToken::new();
        let (completion_sender, completion) = tokio::sync::watch::channel(false);
        let mut state = self.state.lock().await;
        let cancelled_before_register = state
            .pending_cancellations
            .remove(tool_id)
            .is_some_and(|pending_request_id| pending_request_id == request_id);
        let previous = state.active.insert(
            tool_id.to_string(),
            ActiveMarketplaceOAuthLogin {
                request_id: request_id.to_string(),
                cancellation_token: cancellation_token.clone(),
                completion,
            },
        );
        if let Some(previous) = previous.as_ref() {
            previous.cancellation_token.cancel();
        }
        if cancelled_before_register {
            cancellation_token.cancel();
        }
        MarketplaceOAuthLoginRegistration {
            cancellation_token,
            completion_sender,
            previous_completion: previous.map(|active| active.completion),
        }
    }

    async fn is_current(&self, tool_id: &str, request_id: &str) -> bool {
        self.state
            .lock()
            .await
            .active
            .get(tool_id)
            .is_some_and(|active| active.request_id == request_id)
    }

    async fn finish(
        &self,
        tool_id: &str,
        request_id: &str,
        completion_sender: tokio::sync::watch::Sender<bool>,
    ) {
        let mut state = self.state.lock().await;
        if state
            .active
            .get(tool_id)
            .is_some_and(|active| active.request_id == request_id)
        {
            state.active.remove(tool_id);
        }
        drop(state);
        let _ = completion_sender.send(true);
    }

    async fn cancel(&self, tool_id: &str, request_id: &str) -> bool {
        let completion = {
            let mut state = self.state.lock().await;
            let Some(active) = state
                .active
                .get(tool_id)
                .filter(|active| active.request_id == request_id)
            else {
                if state.active.contains_key(tool_id) {
                    return false;
                }
                state
                    .pending_cancellations
                    .insert(tool_id.to_string(), request_id.to_string());
                return true;
            };
            active.cancellation_token.cancel();
            active.completion.clone()
        };
        wait_for_oauth_completion(completion).await;
        true
    }
}

async fn wait_for_oauth_completion(mut completion: tokio::sync::watch::Receiver<bool>) {
    if *completion.borrow() {
        return;
    }
    let _ = completion.changed().await;
}

fn marketplace_oauth_login_coordinator() -> &'static MarketplaceOAuthLoginCoordinator {
    static COORDINATOR: std::sync::OnceLock<MarketplaceOAuthLoginCoordinator> =
        std::sync::OnceLock::new();
    COORDINATOR.get_or_init(MarketplaceOAuthLoginCoordinator::default)
}

#[tauri::command]
pub async fn install_marketplace_tool(
    tool_id: String,
    config: Option<std::collections::HashMap<String, String>>,
) -> Result<(), String> {
    let user_config = config.unwrap_or_default();
    let install_tool_id = tool_id.clone();
    tokio::task::spawn_blocking(move || {
        let mgr = crate::bridge::marketplace::MarketplaceManager::new();
        mgr.install(&install_tool_id, &user_config)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))??;

    let should_validate = {
        let mgr = crate::bridge::marketplace::MarketplaceManager::new();
        mgr.requires_remote_connection_validation(&tool_id)
    };
    if should_validate {
        let validation_result = {
            let mgr = crate::bridge::marketplace::MarketplaceManager::new();
            mgr.validate_remote_connection(&tool_id).await
        };
        if let Err(err) = validation_result {
            let rollback_tool_id = tool_id.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let mgr = crate::bridge::marketplace::MarketplaceManager::new();
                mgr.uninstall(&rollback_tool_id)
            })
            .await;
            return Err(err);
        }
    }

    tokio::task::spawn_blocking(move || {
        let mgr = crate::bridge::marketplace::MarketplaceManager::new();
        // 联动:装该 MCP 声明的配套技能(引擎+引导整体到位)。
        // skill 是增强,装失败只记日志、不让已成功的 MCP 安装回滚。
        for sid in mgr.companion_skills(&tool_id) {
            if let Err(e) =
                crate::bridge::skill_marketplace::SkillMarketplaceManager::new().install(&sid)
            {
                eprintln!("[marketplace] 配套技能 '{sid}' 安装失败: {e}");
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

fn marketplace_oauth_error_result(
    server_name: String,
    error: anyhow::Error,
) -> MarketplaceOAuthLoginResult {
    let detail = format!("{error:#}");
    let lower = detail.to_ascii_lowercase();
    let (status, message) = if lower.contains("oauth login was cancelled") {
        ("cancelled", "已取消等待浏览器授权，可稍后重新授权。")
    } else if lower.contains("timed out waiting for oauth callback") {
        (
            "timeout",
            "授权超时，未收到浏览器回调。若浏览器停在 open.chineselaw.com/service-error，说明元典授权服务返回失败，请关闭页面后重试。",
        )
    } else if lower.contains("service-error") || lower.contains("status code 404") {
        (
            "service_error",
            "元典授权服务返回错误或 404，当前未完成授权。请稍后重试，或联系元典开放平台确认该账号/应用权限。",
        )
    } else if lower.contains("oauth provider") || lower.contains("authorization") {
        (
            "provider_error",
            "元典 OAuth 授权服务拒绝了本次授权，当前未完成连接。请确认账号权限后重试。",
        )
    } else {
        (
            "failed",
            "元典授权失败，当前未完成连接。请重试；如仍失败，请保留浏览器错误页和日志。",
        )
    };

    eprintln!("[marketplace] MCP OAuth login for '{server_name}' failed: {detail}");
    MarketplaceOAuthLoginResult {
        status: status.to_string(),
        message: message.to_string(),
        server_name,
    }
}

fn marketplace_oauth_server_from_mcp_config(
    server_name: &str,
) -> Result<Option<deepseek_tui::mcp::McpServerConfig>, String> {
    let mcp_path = crate::bridge::paths::mcp_config_path();
    if !mcp_path.is_file() {
        return Ok(None);
    }
    let content =
        std::fs::read_to_string(&mcp_path).map_err(|e| format!("读取 mcp.json 失败: {e}"))?;
    let config: deepseek_tui::mcp::McpConfig =
        serde_json::from_str(&content).map_err(|e| format!("解析 mcp.json 失败: {e}"))?;
    Ok(config.servers.get(server_name).cloned())
}

fn marketplace_auth_status_fields(
    installed: bool,
    oauth_required: bool,
    mcp_configured: bool,
    auth_status: Option<deepseek_tui::mcp::oauth::McpAuthStatus>,
) -> (&'static str, &'static str, bool) {
    if oauth_required
        && mcp_configured
        && matches!(
            auth_status,
            Some(deepseek_tui::mcp::oauth::McpAuthStatus::OAuth)
        )
    {
        (
            "connected",
            "已完成元典 OAuth 授权，可以在新会话中使用华宇元典法律数据。",
            true,
        )
    } else if oauth_required && mcp_configured {
        (
            "config_installed_auth_pending",
            "已写入 MCP 配置，但尚未完成元典 OAuth 授权。",
            false,
        )
    } else if oauth_required && installed {
        (
            "auth_pending",
            "工具已安装，但 MCP 配置或授权状态不完整，请重新连接。",
            false,
        )
    } else if oauth_required {
        ("not_installed", "尚未连接华宇元典法律数据。", false)
    } else if installed {
        ("connected", "工具已安装。", false)
    } else {
        ("not_installed", "工具尚未安装。", false)
    }
}

#[tauri::command]
pub async fn get_marketplace_tool_auth_status(
    tool_id: String,
) -> Result<MarketplaceAuthStatus, String> {
    let mgr = crate::bridge::marketplace::MarketplaceManager::new();
    let installed = mgr.installed_ids().iter().any(|id| id == &tool_id);
    let server_name = mgr.oauth_remote_server_name(&tool_id);
    let oauth_required = server_name.is_some();
    let mut mcp_configured = false;
    let mut auth_status = None;

    if let Some(name) = server_name.as_deref() {
        match marketplace_oauth_server_from_mcp_config(name) {
            Ok(Some(server)) => {
                mcp_configured = true;
                auth_status =
                    Some(deepseek_tui::mcp::oauth::auth_status_for_server(name, &server).await);
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!(
                    "[marketplace] failed to read OAuth status for '{name}' from mcp.json: {error}"
                );
            }
        }
    }

    let (status, message, oauth_token_present) =
        marketplace_auth_status_fields(installed, oauth_required, mcp_configured, auth_status);

    Ok(MarketplaceAuthStatus {
        installed,
        mcp_configured,
        oauth_required,
        oauth_token_present,
        status: status.to_string(),
        server_name,
        message: message.to_string(),
    })
}

#[tauri::command]
pub async fn start_marketplace_tool_oauth_login(
    tool_id: String,
    request_id: String,
) -> Result<MarketplaceOAuthLoginResult, String> {
    let mgr = crate::bridge::marketplace::MarketplaceManager::new();
    let server_name = mgr
        .oauth_remote_server_name(&tool_id)
        .ok_or_else(|| format!("工具 '{tool_id}' 未声明远程 MCP OAuth 登录"))?;
    let mcp_path = crate::bridge::paths::mcp_config_path();
    let content =
        std::fs::read_to_string(&mcp_path).map_err(|e| format!("读取 mcp.json 失败: {e}"))?;
    let config: deepseek_tui::mcp::McpConfig =
        serde_json::from_str(&content).map_err(|e| format!("解析 mcp.json 失败: {e}"))?;
    let server = config
        .servers
        .get(&server_name)
        .cloned()
        .ok_or_else(|| format!("mcp.json 未找到服务 '{server_name}'"))?;

    let coordinator = marketplace_oauth_login_coordinator();
    let registration = coordinator.register(&tool_id, &request_id).await;
    if let Some(previous_completion) = registration.previous_completion {
        wait_for_oauth_completion(previous_completion).await;
    }
    if registration.cancellation_token.is_cancelled()
        || !coordinator.is_current(&tool_id, &request_id).await
    {
        coordinator
            .finish(&tool_id, &request_id, registration.completion_sender)
            .await;
        return Ok(MarketplaceOAuthLoginResult {
            status: "cancelled".to_string(),
            message: "已取消等待浏览器授权，可稍后重新授权。".to_string(),
            server_name,
        });
    }

    let login_result = deepseek_tui::mcp::oauth::perform_oauth_login_for_server_with_cancel(
        &server_name,
        &server,
        None,
        None,
        None,
        registration.cancellation_token.clone(),
    )
    .await;
    coordinator
        .finish(&tool_id, &request_id, registration.completion_sender)
        .await;

    match login_result {
        Ok(()) => Ok(MarketplaceOAuthLoginResult {
            status: "connected".to_string(),
            message: "元典 OAuth 授权已完成。".to_string(),
            server_name,
        }),
        Err(e) => Ok(marketplace_oauth_error_result(server_name, e)),
    }
}

#[tauri::command]
pub async fn cancel_marketplace_tool_oauth_login(
    tool_id: String,
    request_id: String,
) -> Result<bool, String> {
    Ok(marketplace_oauth_login_coordinator()
        .cancel(&tool_id, &request_id)
        .await)
}

#[tauri::command]
pub fn uninstall_marketplace_tool(tool_id: String) -> Result<(), String> {
    let mgr = crate::bridge::marketplace::MarketplaceManager::new();
    let companions = mgr.companion_skills(&tool_id); // 卸前先取(manifest 不删,卸后也能读,保险先读)
    if let Some(server_name) = mgr.oauth_remote_server_name(&tool_id) {
        match marketplace_oauth_server_from_mcp_config(&server_name)? {
            Some(server) => {
                deepseek_tui::mcp::oauth::delete_oauth_tokens_for_server(&server_name, &server)
                    .map_err(|e| format!("删除 MCP OAuth token 失败: {e:#}"))?;
            }
            None => {
                eprintln!(
                    "[marketplace] OAuth server '{server_name}' not found in mcp.json while uninstalling '{tool_id}'"
                );
            }
        }
    }
    mgr.uninstall(&tool_id)?;
    // 联动:删配套技能(best-effort,删不掉不影响 MCP 卸载)。
    for sid in companions {
        let _ = crate::bridge::skill_marketplace::SkillMarketplaceManager::new().uninstall(&sid);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 技能市场（与工具市场并列：工具=MCP server，技能=SKILL.md 目录落 bundle/skills/）
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_marketplace_skills(
) -> Result<Vec<crate::bridge::skill_marketplace::MarketplaceSkillInfo>, String> {
    Ok(crate::bridge::skill_marketplace::SkillMarketplaceManager::new().list_skills())
}

#[tauri::command]
pub async fn install_marketplace_skill(skill_id: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || install_marketplace_skill_sync(&skill_id))
        .await
        .map_err(|e| format!("任务执行失败: {e}"))?
}

fn install_marketplace_skill_sync(skill_id: &str) -> Result<(), String> {
    crate::bridge::skill_marketplace::SkillMarketplaceManager::new().install(skill_id)?;
    // disabled_connectors.json 会保留 `skill:<id>` 的用户选择。技能卸载后启动时，
    // refresh 会因未安装而从底座运行态过滤掉；重装成功后必须立即再推一次，避免
    // composer 显示“已关闭”但模型实际仍能 load_skill。
    crate::bridge::skill_marketplace::refresh_disabled_skills();
    Ok(())
}

/// 弹文件选择框选 zip 技能包并导入。前端无法用 plugin-dialog 的 JS API
/// (单 HTML 无 bundler 引不进),所以选文件走 Rust 端 dialog。
/// 返回 true=已导入,false=用户取消。
#[tauri::command]
pub fn import_skill_package(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri_plugin_dialog::DialogExt;
    let Some(picked) = app
        .dialog()
        .file()
        .add_filter("技能包 (zip)", &["zip"])
        .blocking_pick_file()
    else {
        return Ok(false); // 用户取消
    };
    let path = picked
        .into_path()
        .map_err(|e| format!("解析文件路径: {e}"))?;
    crate::bridge::skill_marketplace::SkillMarketplaceManager::new()
        .import_package(&path.to_string_lossy())?;
    crate::bridge::skill_marketplace::refresh_disabled_skills();
    Ok(true)
}

#[tauri::command]
pub fn uninstall_marketplace_skill(skill_id: String) -> Result<(), String> {
    uninstall_marketplace_skill_sync(&skill_id)
}

fn uninstall_marketplace_skill_sync(skill_id: &str) -> Result<(), String> {
    crate::bridge::skill_marketplace::SkillMarketplaceManager::new().uninstall(skill_id)?;
    crate::bridge::skill_marketplace::refresh_disabled_skills();
    Ok(())
}

// ───────────────────────────────────────────────────────────────────────────
// Session timing 诊断读取接口(2026-07 新增)
//
// timing_events.jsonl 此前已有耗时数据,但没有读取接口；token usage 则只发给前端
// 内存态进度条、不落盘。这两个命令把 sidecar 接通为内部诊断基础
// (模型耗时/失败轮次/上下文消耗),不是顶级历史产品入口。
// ───────────────────────────────────────────────────────────────────────────

/// 读取 session 的全部 timeline 事件(user_start / assistant_done 对 + usage),
/// 按 timestamp 升序。空 session / 无 timing 文件返回空数组(诊断面板按空态渲染)。
#[tauri::command]
pub async fn get_session_timeline(
    session_id: String,
) -> Result<Vec<crate::timing::TimelineEvent>, String> {
    // 防路径穿越:timing reader 内部走 sessions_root().join(session_id),
    // 必须先校验 session_id 字符集(只允许 [A-Za-z0-9_-]),否则可构造 ../ 越界。
    crate::bridge::sessions::validate_session_id(&session_id).map_err(|e| format!("{e:?}"))?;
    tokio::task::spawn_blocking(move || crate::timing::read_timeline(&session_id))
        .await
        .map_err(|error| format!("读取 session timeline 任务失败: {error}"))?
        .map_err(|error| format!("读取 session timeline 失败: {error}"))
}

/// 聚合 session 级 stats:轮数 / token 累计 / cache 命中 / 成功失败数 / 首末时间。
/// 可供后续诊断入口消费。token 来自 timing_events 的 usage 字段(2026-07 起写入);
/// 老于此的 session 这些字段为 0(只显示 turn_count + 时间)。
#[tauri::command]
pub async fn get_session_stats(
    session_id: String,
) -> Result<crate::timing::SessionTimelineStats, String> {
    // 同 get_session_timeline:校验 session_id 字符集防路径穿越。
    crate::bridge::sessions::validate_session_id(&session_id).map_err(|e| format!("{e:?}"))?;
    tokio::task::spawn_blocking(move || crate::timing::compute_stats(&session_id))
        .await
        .map_err(|error| format!("统计 session timeline 任务失败: {error}"))?
        .map_err(|error| format!("统计 session timeline 失败: {error}"))
}

/// web 远程 e2e 专用:校验 mobile 上传的文件已落盘且 sha256 匹配。生产代码不调用。
///
/// 路径布局与 web-remote manager 落盘约定对齐:
/// `<pinvou3_home>/uploads/<upload_id>/data.bin`(`<pinvou3_home>` 默认 `~/.pinvou3`,
/// 测试可用 `PINVOU3_HOME` 重定位)。返回 sha256(小写 hex)+ 字节数,e2e 比对客户端已知值。
#[tauri::command]
pub async fn verify_upload(upload_id: String) -> Result<VerifyUploadOutput, String> {
    // 该命令仅供 web 远控 e2e 校验落盘文件 sha256,生产构建(PINVOU3_E2E 未置 "1")
    // 必须直接拒绝:它仍是已注册的 Tauri command,webview 内任意 JS(XSS / 注入)都能
    // invoke 它,没有 env 守卫就会变成一个未鉴权的「读任意 upload 文件 + 泄露 sha256/
    // 字节数」oracle。守卫只放行测试机显式开启的 e2e 场景。
    let e2e_enabled = matches!(std::env::var("PINVOU3_E2E").as_deref(), Ok("1"));
    if !e2e_enabled {
        return Err("verify_upload is disabled: e2e-only command (set PINVOU3_E2E=1)".to_string());
    }
    // 防 upload_id 路径穿越:只允许 [A-Za-z0-9_-],与 session_id 校验一致。
    crate::bridge::sessions::validate_session_id(&upload_id)
        .map_err(|_| "invalid upload_id".to_string())?;
    // 落盘文件名保留原始扩展名(PR #213 审查 #1),不再是固定 data.bin。verify_upload
    // 用目录里唯一的普通文件定位(e2e 场景目录下只有一个落盘文件),避免对扩展名硬编码。
    let upload_dir = crate::bridge::paths::pinvou3_home()
        .join("uploads")
        .join(&upload_id);
    let file_path = match std::fs::read_dir(&upload_dir).ok().and_then(|it| {
        it.filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .next()
            .map(|e| e.path())
    }) {
        Some(p) => p,
        None => return Err("upload not available".to_string()),
    };
    // 有界读取:上限 20MiB(与上传单文件上限 UPLOAD_LIMIT_BYTES 对齐),防止超大/异常
    // 文件把内存撑爆。所有读失败统一收敛为不透明文案,既不回显绝对路径(泄露 home /
    // 用户名),也不把 io::ErrorKind 暴露出去(避免 NotFound vs PermissionDenied 的存在性 oracle)。
    const VERIFY_UPLOAD_MAX_BYTES: usize = 20 * 1024 * 1024;
    let mut file = tokio::fs::File::open(&file_path)
        .await
        .map_err(|_| "upload not available".to_string())?;
    // 先按 metadata 长度预检,避免无界 read_to_end 把超大残留文件全读进内存。
    let meta_len = file
        .metadata()
        .await
        .map(|m| m.len())
        .unwrap_or(VERIFY_UPLOAD_MAX_BYTES as u64);
    if meta_len as usize > VERIFY_UPLOAD_MAX_BYTES {
        return Err("upload not available".to_string());
    }
    let mut bytes = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut file, &mut bytes)
        .await
        .map_err(|_| "upload not available".to_string())?;
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let sha256 = format!("{:x}", hasher.finalize());
    Ok(VerifyUploadOutput {
        sha256,
        byte_size: bytes.len() as u64,
    })
}

#[derive(Debug, serde::Serialize)]
pub struct VerifyUploadOutput {
    pub sha256: String,
    pub byte_size: u64,
}
