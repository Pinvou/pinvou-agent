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
use tauri::State;

use crate::bridge::mode_state::{PlanPhase, SerializableMode, SessionModeState};
use crate::bridge::prefs::UserPrefs;
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
    let mut full = build_message_with_attachments(
        message,
        attachments.unwrap_or_default(),
        &crate::bridge::paths::session_workspace_dir(&sid),
    );
    // Side B 卡片池: 加持后首条消息一次性 prepend 完整人设 body(agency-agents-zh)。
    // 之后每 turn 只靠 equip_anchor 轻锚点维持身份(EnginePool 注入),不再重灌 body。
    if let Some(body) = store.take_pending_persona_body(&sid) {
        full = format!("{body}\n\n---\n\n{full}");
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

/// 把图片附件拷进 session workspace 的 `attachments/` 子目录,返回供 `image_analyze`
/// 使用的 **workspace 相对路径**(image_analyze 只接受不逃逸 workspace 的相对路径)。
/// 失败返回 None,上层降级为提示无法读图。
fn stage_image_in_workspace(src: &str, basename: &str, workspace: &std::path::Path) -> Option<String> {
    let dir = workspace.join("attachments");
    std::fs::create_dir_all(&dir).ok()?;
    // 防重名:已存在则 name-1.ext / name-2.ext 递增。
    let (stem, ext) = match basename.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (basename.to_string(), String::new()),
    };
    let mut candidate = basename.to_string();
    let mut n = 1;
    while dir.join(&candidate).exists() {
        candidate = format!("{stem}-{n}{ext}");
        n += 1;
    }
    std::fs::copy(src, dir.join(&candidate)).ok()?;
    Some(format!("attachments/{candidate}"))
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

/// 把超限附件的转换产物写进 workspace 的 `attachments/`(防重名递增),返回
/// workspace 相对路径。text 类附件不走这里——原文件本身就是 read_file 可读的。
fn stage_text_in_workspace(
    content: &str,
    basename: &str,
    ext: &str,
    workspace: &std::path::Path,
) -> Option<String> {
    let dir = workspace.join("attachments");
    std::fs::create_dir_all(&dir).ok()?;
    let stem = basename.rsplit_once('.').map_or(basename, |(s, _)| s);
    let mut candidate = format!("{stem}.{ext}");
    let mut n = 1;
    while dir.join(&candidate).exists() {
        candidate = format!("{stem}-{n}.{ext}");
        n += 1;
    }
    std::fs::write(dir.join(&candidate), content).ok()?;
    Some(format!("attachments/{candidate}"))
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
) {
    let read_path = if a.kind == "text" {
        a.path.clone()
    } else {
        match stage_text_in_workspace(md, &a.basename, converted_ext(&a.kind), workspace) {
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

/// 拼接 user 文本 + 附件 markdown。
/// 图片拷进 workspace 后引导 LLM 调 image_analyze 读图(Qwen3.6 有视觉能力);
/// 文本类附件按 token 预算分流:小→全量内联,大→落盘+路径+预览(见常量注释)。
/// pub 仅为 L1 dialog harness 复用(lib.rs re-export),不是对外 API。
pub fn build_message_with_attachments(
    text: String,
    attachments: Vec<crate::file_ingest::IngestResult>,
    workspace: &std::path::Path,
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
            match stage_image_in_workspace(&a.path, &a.basename, workspace) {
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
                push_large_attachment_section(&mut out, a, md, workspace);
            }
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

/// 保存设置后立即重启应用（模型/后端切换后需要重启才能生效）。
#[tauri::command]
pub async fn save_settings_and_restart(prefs: UserPrefs, app: tauri::AppHandle) -> Result<(), String> {
    prefs
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
    Ok(crate::monitor::sample_all(&monitor, &crate::monitor::vllm_base_url()).await)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverLocalVllmRequest {
    pub current_base_url: Option<String>,
    pub saved_base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalVllmCandidate {
    pub base_url: String,
    pub status: VllmStatus,
    pub model: Option<String>,
    pub max_model_len: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalVllmDiscovery {
    pub candidates: Vec<LocalVllmCandidate>,
}

/// 手动探测本机 vLLM。只探小白名单候选地址；不做端口扫描,不探局域网。
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
        if let Some(snapshot) = crate::monitor::vllm_snapshot(&base_url).await {
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

/// 扫描 session workspace 目录,返回实际存在的产物文件绝对路径(过滤隐藏/临时文件)。
/// 前端切换 session 时用它对账 —— 让产物面板以**磁盘真相**为准,不受跟踪遗漏 /
/// app 中途重启(内存跟踪丢失)影响。过滤规则与 file_watcher::should_skip 对齐。
#[tauri::command]
pub async fn list_workspace_files(session_id: String) -> Result<Vec<String>, String> {
    let dir = crate::bridge::paths::session_workspace_dir(&session_id);
    let mut out = Vec::new();
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
    /// 最后修改时间（epoch 秒）。取不到给 0。前端列表「最后修改」/ 详情「修改时间」用。
    pub modified: i64,
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
        "txt" | "log" | "csv" | "json" | "yaml" | "yml" | "toml" | "xml"
        | "rs" | "py" | "js" | "ts" | "go" | "c" | "cpp" | "h" | "hpp" | "sh"
        | "bash" | "zsh" | "fish" | "bat" | "cmd" | "ps1"
        | "pl" | "pm" | "lua" | "swift" | "kt" | "kts" | "scala" | "groovy" | "dart"
        | "r" | "m" | "jl" | "erl" | "hrl"
        | "css" | "scss" | "sass" | "less" | "vue" | "svelte" | "mdx"
        | "sql" | "ini" | "conf" | "cfg" | "env" | "properties" | "reg"
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
        VisualResult { mode: "unsupported".into(), html: None, images: vec![], warning }
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

/// 用系统默认浏览器打开**允许列表**里的 https URL。
/// 用于 Settings 面板的"获取 API key"链接(Metaso/Bocha 注册页)。
/// 白名单写死,前端没法用这个 command 打开任意 URL。
#[tauri::command]
pub async fn open_external_url(url: String) -> Result<(), String> {
    const ALLOWED_PREFIXES: &[&str] = &[
        "https://metaso.cn/",
        "https://open.bochaai.com/",
        "https://console.bce.baidu.com/",
    ];
    if !ALLOWED_PREFIXES.iter().any(|p| url.starts_with(p)) {
        return Err(format!("URL not in allowlist: {url}"));
    }
    std::process::Command::new("xdg-open")
        .arg(&url)
        .spawn()
        .map_err(|e| format!("xdg-open({url}) failed: {e}"))?;
    Ok(())
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
    let card = crate::personas::get(&persona_id)
        .ok_or_else(|| format!("未知专家面具: {persona_id}"))?;
    let summary = card.summary();
    store.set_pending_persona_body(&session_id, Some(crate::personas::equip_body_injection(&card)));
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
            emoji: if self.emoji.is_empty() { "🃏".into() } else { self.emoji },
            color: if self.color.is_empty() { "#7C3AED".into() } else { self.color },
            body: self.body,
            source: "user".into(),
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
    pool: State<'_, EnginePool>,
) -> Result<SessionModeState, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `open_external_url` 必须只放 metaso.cn / open.bochaai.com / console.bce.baidu.com,
    /// 任何其他 host / 任何其他 scheme(http、file、javascript)都立即 reject——这是前端
    /// webview 万一被 XSS 的最后一道防线,不许扩大白名单不加测试。
    #[tokio::test]
    async fn open_external_url_rejects_off_allowlist_targets() {
        let rejected = [
            "http://metaso.cn/",              // 非 https
            "https://evil.example.com/",      // host 不在白名单
            "https://metaso.cn.evil.com/",    // 子域钓鱼
            "https://console.bce.baidu.com.evil.com/", // 百度子域钓鱼
            "https://bce.baidu.com/",         // 非 console 子域,不放行
            "javascript:alert(1)",            // js scheme
            "file:///etc/passwd",             // file scheme
            "https://google.com/",            // 任何第三方域
            "",                               // 空串
            "metaso.cn/",                     // 缺 scheme
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
        assert!(r.markdown.is_none(), "图片不再预解析出 markdown(OCR 已移除)");

        let prompt = build_message_with_attachments(
            "这张图里画了什么？".to_string(),
            vec![r],
            &ws,
        );
        assert!(prompt.contains("image_analyze"), "prompt 应引导调 image_analyze");
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

    /// 造一个指定 kind / token 估算的 IngestResult,markdown 是 `rows` 行可定位文本。
    fn mk_attachment(kind: &str, basename: &str, rows: usize, tokens: u32) -> crate::file_ingest::IngestResult {
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
        let ws = std::env::temp_dir().join(format!("pinvou3-attach-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws);
        std::fs::create_dir_all(&ws).expect("建 workspace");
        ws
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
        assert!(prompt.contains("不需要再调 read_file"), "内联段应声明无需 read_file");
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

        assert!(!prompt.contains("row-5000,value-5000"), "完整内容不应进 prompt");
        assert!(prompt.contains("row-1,value-1"), "应有开头预览");
        assert!(prompt.contains("attachments/data.csv"), "应给出落盘 CSV 相对路径");
        assert!(prompt.contains("read_file") && prompt.contains("exec_shell"), "应引导工具消化");
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
        assert!(!prompt.contains("row-9000,value-9000"), "完整内容不应进 prompt");
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
        assert!(prompt.contains("row-60,value-60"), "b 应内联(累计 14K ≤ 16K)");
        assert!(!prompt.contains("row-70,value-70"), "c 应转路径模式(累计 21K > 16K)");
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
            ("sample.png", "这张图里的项目编号是多少？只回答编号。", "/tmp/llm_png.txt"),
            ("scan.pdf", "这份文件的文号是多少？只回答文号。", "/tmp/llm_scan.txt"),
            ("sample.xlsx", "表格里李四的金额是多少？只回答数字。", "/tmp/llm_xlsx.txt"),
            ("sample.pptx", "演示文稿第一章讲什么？提到的编号是？", "/tmp/llm_pptx.txt"),
            ("mail.eml", "这封邮件的主题是什么？正文里的编号是多少？", "/tmp/llm_eml.txt"),
            ("bundle.zip", "这个压缩包里图片上的项目编号是多少？", "/tmp/llm_zip.txt"),
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
