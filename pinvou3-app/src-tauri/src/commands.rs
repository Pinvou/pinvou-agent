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
use tauri::{AppHandle, Emitter, State};

use crate::bridge::mode_state::{PlanPhase, SerializableMode, SessionModeState};
use crate::bridge::prefs::{ModelPreset, UserPrefs};
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
        if crate::workflow_registry::is_workflow_id(&skill.name) {
            // [pinvou3-fork] 真 dispatch 架构:绑定名命中 WorkflowRegistry = workflow
            // session(不是挂载式 skill;对所有工作流通用——h3c-ppt/sansheng-liubu/…)。
            // 品悟在 workflow session 里是**监工**,不自演角色、不走 phase marker 流程
            // (phase 机制已废弃,进度靠 workflow:agent_state_changed 事件驱动卡片)。
            // 每个 user turn 重申监工身份,压过 instructions.md 的通用助手引导
            // (后者教用 write_file/exec_shell,但监工工具白名单根本没给 → 否则品悟会
            // 像 e2e 实测那样瞎读一堆不存在的文件去"推进")。见 feedback_skill_vs_workflow。
            let reminder = "<system-reminder>\n\
                你在一个工作流项目里,身份是**监工(品悟)**,不是执行者。\n\
                - 真正的角色工作由 Harness 自动派发的\
                独立 SubAgent 完成,你看不到、也不干预它们的执行过程。\n\
                - 你的职责只有三件:① 跟用户报告工作流进展 ② Harness 调你时交代任务或评审\
                交付物 ③ 回答用户关于本项目的问题。\n\
                - 你**没有** write_file / edit_file / append_file / exec_shell 等执行类工具,\
                也**不要**尝试自己写文件、跑命令、或读一堆文件去\"推进\"——那是 SubAgent 的活,\
                你做不了也不该做。\n\
                - 用户问进展时,用 read_file 看项目 `_state/` 下已有的交付物和 \
                `_state/workflow_progress.json` 如实回答;不清楚就说不清楚,**不要瞎猜文件名乱读**。\n\
                </system-reminder>";
            full = format!("{reminder}\n\n{full}");
        } else {
            // 其他普通挂载式 skill(review 等)保留 phase marker 机制,供前端 chips 进度。
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
    }

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

/// 实际生效的模型配置（环境变量可能覆盖 settings.json）。
/// 前端设置页初始化时优先用这个，避免"改了 settings 但实际不生效"的困惑。
#[derive(Debug, Clone, Serialize)]
pub struct EffectiveModelConfig {
    pub preset: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub provider: String,
    /// 被环境变量覆盖的字段名列表（如 `["model", "base_url"]`）。
    /// 空列表表示全部走 settings.json，用户修改会生效。
    pub env_overrides: Vec<String>,
}

#[tauri::command]
pub async fn get_effective_model_config(
    pool: State<'_, EnginePool>,
) -> Result<EffectiveModelConfig, String> {
    let bridge = &pool.bridge;
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
    let preset = match bridge.prefs.advanced.model_preset.unwrap_or_default() {
        ModelPreset::LocalVllm => "local_vllm",
        ModelPreset::Deepseek => "deepseek",
        ModelPreset::Kimi => "kimi",
        ModelPreset::OpenaiCompatible => "openai_compatible",
        ModelPreset::Qwen => "qwen",
        ModelPreset::Doubao => "doubao",
        ModelPreset::Minimax => "minimax",
        ModelPreset::Glm => "glm",
        ModelPreset::Mimo => "mimo",
    };
    Ok(EffectiveModelConfig {
        preset: preset.to_string(),
        model: bridge.model(),
        base_url: bridge.base_url(),
        api_key: bridge.api_key(),
        provider: bridge.provider(),
        env_overrides,
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
    /// vLLM 真实上下文窗口（前端 token 进度数据的分母）。
    /// 随 live-dot 轮询下发，监控页未打开时也能保持准确。
    pub max_model_len: Option<u32>,
}

#[tauri::command]
pub async fn get_backend_status(_monitor: State<'_, MonitorState>) -> Result<BackendStatus, String> {
    // Lightweight: 只 probe vLLM,不跑 nvidia-smi / RAM 采样
    let vllm = crate::monitor::vllm_snapshot(
        &crate::monitor::vllm_base_url(),
        crate::monitor::vllm_configured_model(),
    )
    .await;
    let vllm_online = matches!(
        vllm.as_ref().map(|v| v.status),
        Some(VllmStatus::Ready) | Some(VllmStatus::Busy)
    );
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
pub async fn list_sessions(store: State<'_, SessionStore>) -> Result<Vec<SessionMetadata>, String> {
    let mut metas = store.list().map_err(|e| format!("list_sessions: {e:?}"))?;
    metas.retain(|m| {
        store
            .active_skill(&m.id)
            .map_or(true, |b| b.project_dir.is_none())
    });
    Ok(metas)
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

/// 题目转安全文件名:去掉路径分隔/非法字符,截长,空了给兜底。
fn sanitize_title_filename(title: &str, fallback: &str) -> String {
    let cleaned: String = title
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r' | '\0'))
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
            let Ok(canon) = std::fs::canonicalize(&cand) else { continue };
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
                products.push(serde_json::json!({
                    "name": fname, "path": dst.to_string_lossy(), "size": bytes.len(),
                }));
            } else {
                products.push(serde_json::json!({
                    "name": orig_name, "path": canon.to_string_lossy(), "size": bytes.len(),
                }));
            }
            product_canon.push(canon);
        }
    }
    if products.is_empty() {
        // 回退路线(旧 run / 奏折没写成品清单):题目命名的 final_report 副本
        if !report_text.is_empty() {
            let fname = format!("{title_base}.md");
            let dst = p.join(&fname);
            let stale = std::fs::read(&dst)
                .map(|b| b != report_text.as_bytes())
                .unwrap_or(true);
            if stale {
                let _ = std::fs::write(&dst, report_text.as_bytes());
            }
            products.push(serde_json::json!({
                "name": fname, "path": dst.to_string_lossy(), "size": report_text.len(),
            }));
        }
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
            if name.is_empty() || name.starts_with('.') || name.ends_with(".tmp") || name.ends_with('~') {
                continue;
            }
            let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if product_canon.contains(&canon) {
                continue; // 已申报装箱,不重复列
            }
            let size = path.metadata().map(|m| m.len()).unwrap_or(0);
            let item = serde_json::json!({
                "name": name, "path": path.to_string_lossy(), "size": size,
            });
            if name.to_lowercase().ends_with(".md") {
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
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("png").to_ascii_lowercase();
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
        "https://app.tavily.com/",
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
    // h3c-ppt 在 workflow/,review 等 skill 在 skills/;先查 workflow 再 fallback skills。
    let wf_path = paths::bundle_workflow_dir().join(&safe_name).join("SKILL.md");
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

/// [2026-06-06] 工作流素材上传：把用户选的文件拷进当前 run 的 配套材料/ 目录。
/// 前端素材收集卡片「📎 上传素材」按钮 → dialogOpen 选文件 → 调此命令落盘。
/// materials_auditor 重扫 配套材料/ 即可识别。返回实际落盘的文件名（含同名去重后的名）。
#[tauri::command]
pub async fn add_run_materials(
    session_id: Option<String>,
    paths: Vec<String>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<Vec<String>, String> {
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    let workspace = pool.bridge.session_workspace(&sid);
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
    let workspace = pool.bridge.session_workspace(&sid);
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
        return Err(format!(
            "{name} 是系统基础能力,不能直接启用为工作流"
        ));
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
        let session_data = store.load(&sid)
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
    let model = pool.bridge.model();
    let workspace = pool.bridge.workspace.clone();
    let session = store
        .create_new(model, workspace)
        .map_err(|e| format!("create_session: {e:?}"))?;
    let sid = session.metadata.id.clone();
    store.set_active(Some(sid.clone()));

    // 多 session 并发:不预热 engine(lazy)。首条 chat 时 EnginePool 为这个空 session
    //    spawn 专属 engine,空历史无需 SyncSession。

    let injected = format!(
        "[pinvou3-app] 用户在「工作流」视图启用了 skill: `{name}`。\
         请按该 skill 的 phases 流程响应 — engine 会自动从你回复里抽 \
         `<phase id=\"...\"/>` marker 驱动 UI 上方的 phase chip 条,\
         **每条回复必须以该 marker 单独一行开头**(详见 system prompt 里 \
         「Phase tracking — MANDATORY for phased skills」段)。",
        name = skill.name,
    );

    store.bind_skill(
        &sid,
        ActiveSkillBinding {
            name: skill.name.clone(),
            pending_instruction: Some(injected),
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
    let wf = crate::workflow_registry::by_scenario(&scenario)
        .ok_or_else(|| format!("scenario `{scenario}` 没有对应的工作流(bundle/workflow/*/workflow.json)"))?;
    if !wf.enabled {
        return Err(format!("工作流 `{}` 已禁用(workflow.json enabled=false)", wf.id));
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
        let model = pool.bridge.model();
        let session = store
            .create_new(model, pool.bridge.workspace.clone())
            .map_err(|e| format!("create_session: {e:?}"))?;
        let sid = session.metadata.id.clone();
        // 人话 title，工作流页/调试时一眼看出是哪个 PPT 项目
        store.set_title(&sid, session_title.clone()).ok();
        sid
    };

    // 2. 在**该 session 的 workspace**下初始化项目目录。harness forwarder 也按
    //    bridge.session_workspace(session_id) 找项目,两处路径必须一致,否则推进找不到项目。
    let workspace = pool.bridge.session_workspace(&sid);
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
    let ws = pool.bridge.session_workspace(&sid);
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
        crate::harness::HarnessAction::SpawnAgentBatch { base_role, role_name, tasks } => {
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
                engine.handle.send(op).await.map_err(|e| format!("fan-out spawn: {e:?}"))?;
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
    let ws = pool.bridge.session_workspace(&sid);
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
        crate::harness::HarnessAction::SpawnAgentBatch { base_role, role_name, tasks } => {
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
                engine.handle.send(op).await.map_err(|e| format!("fan-out spawn: {e:?}"))?;
            }
            crate::engine::emit_fanout(&app, &sid, &base_role); // 初始 fan-out 状态 → 前端
            Ok(format!("retry → spawning {role_name} ({n} pages, 在飞={k})"))
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
        vec![serde_json::Value::String(format!("deliverables/{bu}_{seq}.md"))]
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
                let name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
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
        name.starts_with(prefix) && name.ends_with(suffix) && name.len() >= prefix.len() + suffix.len()
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
    let content =
        std::fs::read_to_string(&log_path).map_err(|e| format!("read log: {e}"))?;
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
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("read gate report {}: {e}", path.display()))?;
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
    let role_entry = obj
        .entry(role_id.clone())
        .or_insert(serde_json::json!({}));
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
    pool: State<'_, EnginePool>,
) -> Result<serde_json::Value, String> {
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    let workspace = pool.bridge.session_workspace(&sid);
    let rid = role_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        // 找到 project_dir
        let project = match crate::harness::find_project_dir(&workspace) {
            Some(p) => p,
            None => return Err("no project found".to_string()),
        };
        // 读 scenario
        let scenario_content = std::fs::read_to_string(
            project.join("_state").join("workflow_progress.json"),
        )
        .unwrap_or_default();
        let scenario = serde_json::from_str::<serde_json::Value>(&scenario_content)
            .ok()
            .and_then(|v| v.get("scenario").and_then(|s| s.as_str()).map(String::from))
            .unwrap_or_else(|| "solution_deck".to_string());
        // 走 scheduler 通用入口（用 std::process::Command 直接调）
        let scheduler =
            crate::harness::scheduler_path_for(&crate::harness::workflow_name_for_scenario(&scenario));
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
    let workspace = pool.bridge.session_workspace(&sid);
    let engine = pool
        .get_or_spawn(&sid)
        .await
        .map_err(|e| format!("get engine for {sid}: {e:?}"))?;
    let rid = role_id.clone();
    let action = tokio::task::spawn_blocking(move || {
        crate::harness::approve_gate(&workspace, &rid)
    }).await.map_err(|e| format!("spawn_blocking: {e}"))?;
    // approve 后 step_fresh 推进到下一角色：SpawnAgent（直派）/ AllDone / WaitForHuman。
    // 用 apply_harness_action 统一处理（set phase / emit / 派发），其值化结果回前端。
    let next_label = match &action {
        crate::harness::HarnessAction::SpawnAgent { .. } => "dispatch",
        crate::harness::HarnessAction::AllDone => "all_done",
        crate::harness::HarnessAction::WaitForHuman { .. } => "waiting",
        crate::harness::HarnessAction::Blocked { .. } => "blocked",
        _ => "noop",
    };
    let handled = crate::engine::apply_harness_action(
        action,
        &app,
        &engine.bridge,
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
    let workspace = pool.bridge.session_workspace(&sid);
    let engine = pool
        .get_or_spawn(&sid)
        .await
        .map_err(|e| format!("get engine for {sid}: {e:?}"))?;
    let rid = role_id.clone();
    let r = reason.clone();
    let action = tokio::task::spawn_blocking(move || {
        crate::harness::reject_gate(&workspace, &rid, &r)
    }).await.map_err(|e| format!("spawn_blocking: {e}"))?;
    // reject 后 reject_gate 返回 SpawnAgent（重新派发同角色 SubAgent，附拒绝原因）。
    let next_label = match &action {
        crate::harness::HarnessAction::SpawnAgent { .. } => "redo",
        crate::harness::HarnessAction::Blocked { .. } => "blocked",
        _ => "noop",
    };
    let handled = crate::engine::apply_harness_action(
        action,
        &app,
        &engine.bridge,
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
    pool: State<'_, EnginePool>,
) -> Result<serde_json::Value, String> {
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    let workspace = pool.bridge.session_workspace(&sid);
    tokio::task::spawn_blocking(move || {
        crate::harness::read_full_agent_state(&workspace)
            .unwrap_or(serde_json::json!(null))
    }).await.map_err(|e| format!("spawn_blocking: {e}"))
}

/// [2026-06-06] 找最近一个「进行中」的工作流 run，供 app 启动后前端自动恢复看板。
/// 扫所有 session 的 skill binding：有 project_dir（=工作流会话）且 workflow_progress.json
/// 里存在未完成角色的，按 progress 文件 mtime 取最近一个。
/// 返回 {session_id, project_dir, scenario}，无则返回 null。
#[tauri::command]
pub async fn find_resumable_run(store: State<'_, SessionStore>) -> Result<serde_json::Value, String> {
    let metas = store.list().map_err(|e| format!("list: {e:?}"))?;
    let mut best: Option<(std::time::SystemTime, String, String, String)> = None;
    for m in metas {
        let Some(binding) = store.active_skill(&m.id) else { continue };
        let Some(pd) = binding.project_dir else { continue };
        let progress = std::path::Path::new(&pd).join("_state").join("workflow_progress.json");
        let Ok(content) = std::fs::read_to_string(&progress) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) else { continue };
        // 未全完成 = roles 非空且存在 status != completed 的角色
        let unfinished = v.get("roles").and_then(|r| r.as_object()).is_some_and(|rs| {
            !rs.is_empty()
                && rs
                    .values()
                    .any(|r| r.get("status").and_then(|s| s.as_str()) != Some("completed"))
        });
        if !unfinished {
            continue;
        }
        let Some(scenario) = v
            .get("scenario")
            .and_then(|s| s.as_str())
            .map(String::from)
        else {
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
        let phases = if b.project_dir.is_some() { Vec::new() } else { b.phases };
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
        let new = serde_json::json!([{"pos":1,"review":{"issues":[],"recommendations":[{"topic":"t"}]}}]);
        let merged = merge_resolutions(old, new);
        assert_eq!(merged[0]["review"]["recommendations"][0]["resolution"], "modify");
    }

    /// `open_external_url` 必须只放 metaso.cn / open.bochaai.com / console.bce.baidu.com /
    /// app.tavily.com,任何其他 host / 任何其他 scheme(http、file、javascript)都立即
    /// reject——这是前端 webview 万一被 XSS 的最后一道防线,不许扩大白名单不加测试。
    #[tokio::test]
    async fn open_external_url_rejects_off_allowlist_targets() {
        let rejected = [
            "http://metaso.cn/",              // 非 https
            "https://evil.example.com/",      // host 不在白名单
            "https://metaso.cn.evil.com/",    // 子域钓鱼
            "https://console.bce.baidu.com.evil.com/", // 百度子域钓鱼
            "https://app.tavily.com.evil.com/", // tavily 子域钓鱼
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
