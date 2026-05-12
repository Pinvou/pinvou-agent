//! Web UI — axum server + 内嵌 HTML 前端。
//!
//! 路由:
//!   GET  /              → HTML 页面
//!   POST /api/chat/stream → SSE 流式 LLM 对话

use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::{
    Json, Router,
    extract::State,
    response::{Html, Sse},
    routing::{get, post},
};
use async_stream::stream as async_stream;
use futures_util::StreamExt;
use futures_util::stream::{self, Stream};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::contract_runtime::{ContractRuntime, TurnDirective};
use crate::contract_validator::ContractValidator;
use crate::engine::{MilestoneAdvanceResult, UserChoiceAnswer};
use crate::engine_factory::PinvouEngine;
use crate::harness::{AgentHarness, StreamEvent, ToolDef};
use crate::rollback::{self, RollbackOutcome};

// === App State ===

pub struct AppState {
    /// 用 `Arc<Mutex<_>>` 包装是为了让 backend looper（`build_milestone_loop_stream`）
    /// 能克隆出独立持有者跨 `async_stream` yield 边界。
    pub engine: Arc<Mutex<PinvouEngine>>,
}

// === Request / Response ===

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub message: String,
    /// 兼容字段：前端仍在发，但服务端不再使用（agent 由 CombinedPlanner 决定，
    /// 或通过 `/use <agent_id>` slash 命令切换）。后续前端去除即可删除。
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub tool_result: Option<ToolResultDto>,
}

#[derive(Debug, Deserialize)]
pub struct ToolResultDto {
    pub call_id: String,
    #[serde(default)]
    pub answers: Vec<ChoiceAnswerDto>,
    #[serde(default)]
    pub skip: bool,
}

#[derive(Debug, Deserialize)]
pub struct ChoiceAnswerDto {
    pub id: String,
    pub label: String,
    pub value: String,
}

// === Index ===

async fn index() -> Html<&'static str> {
    Html(include_str!("index.html"))
}

// === SSE Streaming Chat ===

async fn handle_chat_stream(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChatRequest>,
) -> Sse<impl Stream<Item = Result<axum::response::sse::Event, Infallible>>> {
    let stream = stream_chat(state, req).await;
    Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::new().interval(Duration::from_secs(15)))
}

fn sse_err(msg: impl Into<String>) -> axum::response::sse::Event {
    axum::response::sse::Event::default().data(serde_json::json!({"error": msg.into()}).to_string())
}

fn sse_delta(text: &str) -> axum::response::sse::Event {
    axum::response::sse::Event::default().data(serde_json::json!({"delta": text}).to_string())
}

fn sse_done() -> axum::response::sse::Event {
    axum::response::sse::Event::default().data(r#"{"done":true}"#)
}

/// 在 engine 上执行一条 slash 命令并记录历史。
fn handle_slash_command(engine: &mut PinvouEngine, raw: &str) -> RollbackOutcome {
    // 1. 解析
    let Some(cmd) = rollback::parse_command(raw) else {
        return RollbackOutcome {
            message: format!("未知命令: {}", raw.trim()),
            state_changed: false,
            trigger_replan: false,
            switch_agent: None,
        };
    };

    // 2. 记录用户输入
    engine.messages.push(crate::harness::HistoryMessage {
        role: "user".into(),
        content: raw.trim().to_string(),
    });

    // 3. 在 conv_state 上执行
    let outcome = match engine.conv_state.as_mut() {
        Some(cs) => rollback::execute(cmd, cs),
        None => RollbackOutcome {
            message: "尚无会话状态，无法执行命令".into(),
            state_changed: false,
            trigger_replan: false,
            switch_agent: None,
        },
    };

    // 4. 记录助手响应
    engine.messages.push(crate::harness::HistoryMessage {
        role: "assistant".into(),
        content: outcome.message.clone(),
    });

    outcome
}

/// SSE 事件：slash 命令执行结果（不调 LLM）
fn sse_command_result(outcome: &RollbackOutcome) -> axum::response::sse::Event {
    axum::response::sse::Event::default().data(
        serde_json::json!({
            "done": true,
            "command": {
                "message": outcome.message,
                "state_changed": outcome.state_changed,
                "trigger_replan": outcome.trigger_replan,
                "switch_agent": outcome.switch_agent,
            }
        })
        .to_string(),
    )
}

/// 构造 slash 命令的 SSE 响应（一个 delta + 一个 done）
fn build_command_sse(outcome: RollbackOutcome) -> Vec<SseItem> {
    let delta = sse_delta(&outcome.message);
    let done = sse_command_result(&outcome);
    vec![Ok(delta), Ok(done)]
}

/// 中间态：milestone 推进事件（**不含** `done` 标志），用于 auto-continue 场景。
/// 前端收到此事件应保持监听，等待后续 LLM delta。
fn sse_milestone_progress(result: &MilestoneAdvanceResult) -> axum::response::sse::Event {
    axum::response::sse::Event::default().data(
        serde_json::json!({
            "milestone": {
                "milestone_id": result.completed_milestone_id,
                "next_milestone_id": result.next_milestone_id,
                "signal": "ChoiceResult",
            }
        })
        .to_string(),
    )
}

fn sse_done_for_milestone(result: &MilestoneAdvanceResult) -> axum::response::sse::Event {
    axum::response::sse::Event::default().data(
        serde_json::json!({
            "done": true,
            "milestone": {
                "milestone_id": result.completed_milestone_id,
                "next_milestone_id": result.next_milestone_id,
                "next_action": "WaitForUser",
                "signal": "ChoiceResult",
            }
        })
        .to_string(),
    )
}

type SseItem = Result<axum::response::sse::Event, Infallible>;

/// auto-continue 场景下用的 user_message 占位符。
///
/// 为什么不能让 user_message 为空：本地 LLM（如 Qwen 系列）收到空 user 消息时
/// 会直接输出 EOS，导致 stream `response_text len=0` + 不调任何工具，
/// 阶段卡死。给一个明确的"系统过渡"消息让 LLM 知道按 system prompt 继续。
const AUTO_CONTINUE_AFTER_CONTINUE: &str =
    "（系统：用户已确认推进，请按当前阶段要求继续）";
const AUTO_CONTINUE_AFTER_TOOL_RESULT: &str =
    "（系统：用户已完成上一阶段选择，请按当前阶段要求继续）";
/// backend looper 在同一个 SSE 流内串接多个 milestone 时，下一阶段的过渡 user_message。
const AUTO_CONTINUE_AFTER_ADVANCE: &str =
    "（系统：上一阶段已通过校验，请按当前阶段要求继续）";

/// backend looper 单次 SSE 流内最多自动串接的 milestone 数。
/// OnValidOutput 阶段顺序推进时计数，超过则切断流让用户介入，
/// 防止 LLM 在某种异常情况下无限自验通过推进。
const MAX_AUTO_MILESTONE_ADVANCES: usize = 8;

/// 解析本轮发给 LLM 的 user_message。
///
/// 三种情况：
/// 1. 用户输入了"继续"等触发推进的命令 → 用 AUTO_CONTINUE_AFTER_CONTINUE
/// 2. 用户发了 tool_result（选择卡回传）且无文本消息 → 用 AUTO_CONTINUE_AFTER_TOOL_RESULT
/// 3. 用户输入了普通文本 → 透传原始消息
fn resolve_effective_message<'a>(
    raw_message: &'a str,
    continue_consumed: bool,
    has_tool_result: bool,
) -> &'a str {
    if continue_consumed {
        AUTO_CONTINUE_AFTER_CONTINUE
    } else if has_tool_result && raw_message.is_empty() {
        AUTO_CONTINUE_AFTER_TOOL_RESULT
    } else {
        raw_message
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WebTurnAction {
    CallLlm,
    Blocked(String),
    AskUser(crate::contract_runtime::ChoiceRequest),
    CompleteStep(String),
    /// Q&A 模式或无活跃 milestone：跳过 contract，直接走 LLM 流
    FreeFlow,
}

fn classify_turn_directive_for_web(directive: anyhow::Result<TurnDirective>) -> WebTurnAction {
    match directive {
        Ok(TurnDirective::CallLlm(_)) => WebTurnAction::CallLlm,
        Ok(TurnDirective::Blocked(message)) => WebTurnAction::Blocked(message),
        Ok(TurnDirective::AskUser(choice)) => WebTurnAction::AskUser(choice),
        Ok(TurnDirective::CompleteStep(message)) => WebTurnAction::CompleteStep(message),
        Err(_) => WebTurnAction::FreeFlow,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FrontendToolDecision {
    Accept,
    RejectAndAbort(String),
}

fn filter_tools_for_contract(
    all_tools: Vec<ToolDef>,
    app_tool_names: &[String],
    milestone: Option<&crate::workflow::Milestone>,
) -> Vec<ToolDef> {
    all_tools
        .into_iter()
        .filter(|tool| app_tool_names.is_empty() || app_tool_names.contains(&tool.name))
        .filter(|tool| {
            let Some(ms) = milestone else {
                return true;
            };
            let contract = &ms.contract;
            if contract
                .output_requirements
                .contains(&crate::contract::OutputRequirement::NoToolCall)
            {
                return false;
            }
            !contract.allowed_tools.is_empty()
                && contract.allowed_tools.iter().any(|name| name == &tool.name)
                && !contract
                    .forbidden_tools
                    .iter()
                    .any(|name| name == &tool.name)
        })
        .collect()
}

fn authorize_tool_call(
    milestone: Option<&crate::workflow::Milestone>,
    conv_state: Option<&crate::workflow::ConversationState>,
    tool_name: &str,
    arguments: &serde_json::Value,
) -> FrontendToolDecision {
    if let Some(ms) = milestone {
        let validation = ContractValidator::validate_tool_call(&ms.contract, tool_name, arguments);
        if !validation.ok {
            return FrontendToolDecision::RejectAndAbort(format!(
                "当前阶段输出不符合契约：{}",
                validation.issues.join("；")
            ));
        }
    }

    if tool_name == "request_user_input" {
        if let (Some(ms), Some(cs)) = (milestone, conv_state) {
            match ContractRuntime::next_directive(ms, cs, "") {
                Ok(TurnDirective::Blocked(message)) => {
                    return FrontendToolDecision::RejectAndAbort(message);
                }
                Ok(_) => {}
                Err(e) => {
                    return FrontendToolDecision::RejectAndAbort(format!(
                        "当前阶段无法确认提问预算：{e}"
                    ));
                }
            }
        }
    }

    FrontendToolDecision::Accept
}

fn reserve_request_user_input_budget(
    milestone: Option<&crate::workflow::Milestone>,
    conv_state: Option<&mut crate::workflow::ConversationState>,
) -> FrontendToolDecision {
    if let (Some(ms), Some(cs)) = (milestone, conv_state) {
        match ContractRuntime::next_directive(ms, cs, "") {
            Ok(TurnDirective::Blocked(message)) => {
                return FrontendToolDecision::RejectAndAbort(message);
            }
            Ok(_) => cs.increment_question_count(&ms.id),
            Err(e) => {
                return FrontendToolDecision::RejectAndAbort(format!(
                    "当前阶段无法确认提问预算：{e}"
                ));
            }
        }
    }

    FrontendToolDecision::Accept
}

fn choice_request_allowed_after_validation(
    choice_request: Option<serde_json::Value>,
    validation_issues: &[String],
) -> Option<serde_json::Value> {
    if validation_issues.is_empty() {
        choice_request
    } else {
        None
    }
}

/// 选择卡上下文清洗：去掉 DeepSeek-TUI tool loop 注入的横幅，
/// 只保留 LLM 自己的论证文字。
///
/// 横幅形态来自 `deepseek_harness.rs` 的 tool loop：
/// - `\n\n⏳ 正在调用工具...\n`
/// - `\n\n🔧 [<tool>] {args}\n`
/// - `\n\n📄 结果:\n<json>`
///
/// 这些对前端选择卡是噪音；用户要看的是"为什么要做这个决策"，
/// 不是 web_search 返回了哪些 JSON。
fn strip_tool_banners(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_skip_block = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("⏳ 正在调用工具")
            || trimmed.starts_with("🔧 [")
            || trimmed.starts_with("📄 结果")
        {
            in_skip_block = true;
            continue;
        }
        // 跳过 tool 结果块的后续行：JSON / 普通文本，直到遇到空行或下一段
        if in_skip_block {
            if trimmed.is_empty() {
                in_skip_block = false;
            }
            continue;
        }
        out.push_str(line);
    }
    // 末尾如果只剩 tool 输出后的空白，trim 一下
    out.trim().to_string()
}

fn validation_delta_from_issues(validation_issues: &[String]) -> Option<String> {
    if validation_issues.is_empty() {
        None
    } else {
        Some(format!(
            "\n\n当前阶段输出未通过契约校验，暂不自动进入下一阶段：{}",
            validation_issues.join("；")
        ))
    }
}

/// 阶段推进事件（不带 `done`）—— backend looper 在同一个 SSE 流内串接到下一阶段时发。
/// 前端收到此事件应仅刷新进度，不要中断对流的监听。
fn sse_stage_advanced(completed_id: &str, next_id: &str) -> axum::response::sse::Event {
    axum::response::sse::Event::default().data(
        serde_json::json!({
            "milestone": {
                "milestone_id": completed_id,
                "next_milestone_id": next_id,
                "next_action": "Advance",
                "signal": "AutoAdvance",
            }
        })
        .to_string(),
    )
}

/// 全部里程碑完成 done 事件 —— backend looper 在 OnValidOutput 阶段推进到尾时发。
fn sse_all_milestones_complete(last_id: &str) -> axum::response::sse::Event {
    axum::response::sse::Event::default().data(
        serde_json::json!({
            "done": true,
            "milestone": {
                "milestone_id": last_id,
                "next_action": "Complete",
                "signal": "AllDone",
            }
        })
        .to_string(),
    )
}

/// 单个里程碑结束但**不**自动推进 —— OnChoice 路径 / 校验失败 / 无活跃 milestone 时发。
fn sse_stage_done_wait(
    milestone_id: Option<&str>,
    validation_issues: &[String],
) -> axum::response::sse::Event {
    let next_action = if validation_issues.is_empty() {
        "WaitForUser"
    } else {
        "WaitValidation"
    };
    axum::response::sse::Event::default().data(
        serde_json::json!({
            "done": true,
            "milestone": milestone_id.map(|id| serde_json::json!({
                "milestone_id": id,
                "next_action": next_action,
                "validation_issues": validation_issues,
            })),
        })
        .to_string(),
    )
}

async fn stream_chat(
    state: Arc<AppState>,
    req: ChatRequest,
) -> Pin<Box<dyn Stream<Item = SseItem> + Send>> {
    // === Phase 1: 准备（同步）===
    // slash 命令 / plan 初始化 / tool_result 处理 / continue 命令
    // 完成后产出 auto_continue_prefix + initial_message + record_user 标志
    let prep: PrepOutcome = {
        let mut engine = state.engine.lock().await;

        // Slash 命令早退
        if req.tool_result.is_none() && rollback::is_slash_command(&req.message) {
            let outcome = handle_slash_command(&mut engine, &req.message);
            return Box::pin(stream::iter(build_command_sse(outcome)));
        }

        // Plan 初始化（首轮非 tool_result）
        if req.tool_result.is_none() {
            if let Err(e) = engine.ensure_combined_plan(&req.message).await {
                let ev = sse_err(format!("初始化计划失败: {e}"));
                return Box::pin(stream::iter(vec![Ok(ev)]));
            }
        }

        let mut auto_continue_prefix: Vec<SseItem> = Vec::new();

        // tool_result 处理：消费 choice，把 ack + 推进事件作为 prefix
        if let Some(ref tr) = req.tool_result {
            let answers: Vec<UserChoiceAnswer> = tr
                .answers
                .iter()
                .map(|a| UserChoiceAnswer {
                    id: a.id.clone(),
                    label: a.label.clone(),
                    value: a.value.clone(),
                })
                .collect();
            let result = engine.apply_choice_result(&tr.call_id, &answers, tr.skip);
            engine.messages.push(crate::harness::HistoryMessage {
                role: "assistant".into(),
                content: result.summary.clone(),
            });
            auto_continue_prefix.push(Ok(sse_delta(&result.summary)));
            auto_continue_prefix.push(Ok(sse_milestone_progress(&result)));
        }

        // "继续" 命令
        let continue_result = engine.consume_continue_command(&req.message);
        if continue_result.is_none() && !req.message.trim().is_empty() {
            engine.clear_awaiting_start_marker();
        }
        if let Some(ref result) = continue_result {
            if result.next_milestone_id.is_none() {
                if !req.message.is_empty() {
                    engine.messages.push(crate::harness::HistoryMessage {
                        role: "user".into(),
                        content: req.message.clone(),
                    });
                }
                if let Some(ref mut cs) = engine.conv_state {
                    cs.increment_turn();
                }
                engine.messages.push(crate::harness::HistoryMessage {
                    role: "assistant".into(),
                    content: result.summary.clone(),
                });
                let events = vec![
                    Ok(sse_delta(&result.summary)),
                    Ok(sse_done_for_milestone(result)),
                ];
                return Box::pin(stream::iter(events));
            }
        }

        let effective_message = resolve_effective_message(
            req.message.as_str(),
            continue_result.is_some(),
            req.tool_result.is_some(),
        )
        .to_string();

        PrepOutcome {
            auto_continue_prefix,
            initial_message: effective_message,
            user_message_to_record: if req.message.is_empty() {
                None
            } else {
                Some(req.message.clone())
            },
        }
    };

    // === Phase 2: 流式 looper ===
    // 同一个 SSE 流内最多串 MAX_AUTO_MILESTONE_ADVANCES 个 OnValidOutput 阶段。
    // 遇到 AskUser / OnChoice 完成 / Blocked / 错误 / 验证失败 即结束流，等用户行为。
    Box::pin(build_milestone_loop_stream(state.engine.clone(), prep))
}

pub(crate) struct PrepOutcome {
    pub auto_continue_prefix: Vec<SseItem>,
    pub initial_message: String,
    pub user_message_to_record: Option<String>,
}

/// 泛型 backend looper：在同一个 SSE 流内连续推进多个 OnValidOutput 阶段，
/// 遇到需要用户行为的阶段（AskUser / 校验失败 / 错误 / OnChoice 完成）即结束流。
///
/// 泛型 `H` 让测试可以用 `MockHarness` 直接构造可观察的 SSE 流。
pub(crate) fn build_milestone_loop_stream<H>(
    engine_mutex: Arc<Mutex<crate::engine::PlatformEngine<H>>>,
    prep: PrepOutcome,
) -> impl Stream<Item = SseItem> + Send
where
    H: AgentHarness + Send + Sync + 'static,
{
    let PrepOutcome {
        auto_continue_prefix,
        initial_message,
        user_message_to_record,
    } = prep;

    async_stream! {
        for item in auto_continue_prefix {
            yield item;
        }

        let mut engine = engine_mutex.lock().await;
        let mut current_message = initial_message;
        let mut user_message_to_record = user_message_to_record;
        let mut advances: usize = 0;

        'milestone_loop: loop {
            // 取当前 active milestone
            let active_milestone = engine
                .conv_state
                .as_ref()
                .and_then(|cs| cs.active_milestone().cloned());

            eprintln!(
                "[looper] 进入循环 advances={advances} active={:?} mode={:?} advance_policy={:?}",
                active_milestone.as_ref().map(|m| &m.id),
                active_milestone.as_ref().map(|m| &m.contract.mode),
                active_milestone
                    .as_ref()
                    .map(|m| &m.contract.advance_policy),
            );

            let in_qa_mode = engine
                .conv_state
                .as_ref()
                .map(|cs| cs.global_mode == crate::workflow::GlobalMode::QnA)
                .unwrap_or(false);
            let all_tools = AgentHarness::tools(&engine.harness);
            let tools = if in_qa_mode {
                vec![]
            } else {
                filter_tools_for_contract(all_tools, &[], active_milestone.as_ref())
            };

            let turn_action = if let (Some(cs), Some(ms)) =
                (engine.conv_state.as_ref(), active_milestone.as_ref())
            {
                classify_turn_directive_for_web(
                    ContractRuntime::next_directive(ms, cs, &current_message),
                )
            } else {
                WebTurnAction::FreeFlow
            };

            let chat_req = match turn_action {
                WebTurnAction::Blocked(message) | WebTurnAction::CompleteStep(message) => {
                    if let Some(ref mut cs) = engine.conv_state {
                        cs.increment_turn();
                    }
                    if let Some(text) = user_message_to_record.take() {
                        engine.messages.push(crate::harness::HistoryMessage {
                            role: "user".into(),
                            content: text,
                        });
                    }
                    engine.messages.push(crate::harness::HistoryMessage {
                        role: "assistant".into(),
                        content: message.clone(),
                    });
                    yield Ok(sse_delta(&message));
                    yield Ok(sse_done());
                    return;
                }
                WebTurnAction::AskUser(choice) => {
                    let choice_event = axum::response::sse::Event::default().data(
                        serde_json::json!({
                            "choice_request": {
                                "call_id": choice.call_id,
                                "questions": choice.questions,
                            }
                        })
                        .to_string(),
                    );
                    yield Ok(choice_event);
                    return;
                }
                WebTurnAction::CallLlm => match engine.build_next_contract_prompt(&current_message) {
                    Ok(step_prompt) => {
                        let mut r = engine.build_request(&current_message, tools);
                        r.platform_system_prompt = Some(step_prompt.system);
                        r
                    }
                    Err(_) => engine.build_request(&current_message, tools),
                },
                WebTurnAction::FreeFlow => engine.build_request(&current_message, tools),
            };

            if let Some(ref mut cs) = engine.conv_state {
                cs.increment_turn();
            }
            if let Some(text) = user_message_to_record.take() {
                engine.messages.push(crate::harness::HistoryMessage {
                    role: "user".into(),
                    content: text,
                });
            }

            // [INSTRUMENTATION] 打印实际发给 LLM 的 system_prompt 前 500 字 +
            // context 全量，便于验证 review_feedback / 其他 context 是否真的注入
            {
                let sp = chat_req
                    .platform_system_prompt
                    .as_deref()
                    .unwrap_or("");
                let sp_preview: String = sp.chars().take(500).collect();
                eprintln!(
                    "[chat_req:trace] active_ms={:?} system_prompt_len={} user_message={:?}",
                    active_milestone.as_ref().map(|m| (m.id.clone(), m.contract.mode.clone())),
                    sp.chars().count(),
                    chat_req.user_message,
                );
                eprintln!("[chat_req:trace] system_prompt_head: {sp_preview}");
                eprintln!("[chat_req:trace] context: {:?}", chat_req.context);
            }

            let mut event_stream = match AgentHarness::chat_stream(&engine.harness, chat_req).await
            {
                Ok(s) => s,
                Err(e) => {
                    yield Ok(sse_err(e.to_string()));
                    return;
                }
            };

            // per-milestone 局部状态
            let mut full_text = String::new();
            let mut invoked_tools: Vec<String> = Vec::new();
            let mut choice_state: Option<serde_json::Value> = None;

            while let Some(result) = event_stream.next().await {
                match result {
                    Ok(StreamEvent::TextDelta { content }) => {
                        full_text.push_str(&content);
                        yield Ok(sse_delta(&content));
                    }
                    Ok(StreamEvent::ToolCallStart {
                        call_id,
                        tool_name,
                        arguments,
                    }) => {
                        eprintln!(
                            "[web] ToolCallStart: {tool_name} call_id={call_id} args={arguments:?}"
                        );
                        if !invoked_tools.iter().any(|t| t == &tool_name) {
                            invoked_tools.push(tool_name.clone());
                        }
                        let decision = authorize_tool_call(
                            active_milestone.as_ref(),
                            engine.conv_state.as_ref(),
                            &tool_name,
                            &arguments,
                        );
                        if let FrontendToolDecision::RejectAndAbort(message) = decision {
                            yield Ok(sse_err(message));
                            return;
                        }
                        if tool_name == "request_user_input" {
                            choice_state = Some(serde_json::json!({
                                "call_id": call_id,
                                "questions": arguments
                                    .get("questions")
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null),
                            }));
                        }
                        // 捕获 write_file / edit_file 的 path 字段到 context，
                        // 让后续 PatchOutput 阶段 prompt 能引用 `last_output_path`。
                        if tool_name == "write_file" || tool_name == "edit_file" {
                            if let Some(path) =
                                arguments.get("path").and_then(|v| v.as_str())
                            {
                                if let Some(ref mut cs) = engine.conv_state {
                                    cs.set_context("last_output_path", path);
                                }
                            }
                        }
                        yield Ok(sse_delta(""));
                    }
                    Ok(StreamEvent::Done) => {
                        // === 校验 + 决定推进 ===
                        let (should_advance, validation_issues) = if let Some(ms) =
                            &active_milestone
                        {
                            let result = ContractValidator::validate_stage_completion(
                                &ms.contract,
                                &full_text,
                                &invoked_tools,
                            );
                            let should_advance = result.ok
                                && matches!(
                                    ms.contract.advance_policy,
                                    crate::contract::AdvancePolicy::OnValidOutput
                                );
                            let issues = if result.ok { vec![] } else { result.issues.clone() };
                            eprintln!(
                                "[looper] stage Done ms={} text_len={} invoked_tools={:?} validation_ok={} should_advance={} issues={:?}",
                                ms.id,
                                full_text.len(),
                                invoked_tools,
                                result.ok,
                                should_advance,
                                issues,
                            );
                            (should_advance, issues)
                        } else {
                            eprintln!(
                                "[looper] stage Done (no active milestone) text_len={}",
                                full_text.len()
                            );
                            (false, vec![])
                        };

                        let pending_choice = choice_state.take();
                        let allowed_choice = choice_request_allowed_after_validation(
                            pending_choice,
                            &validation_issues,
                        );

                        // 记录 assistant 输出（不管是否推进、是否要发选择卡）
                        engine.messages.push(crate::harness::HistoryMessage {
                            role: "assistant".into(),
                            content: full_text.clone(),
                        });

                        // === 分支 1: 有选择卡需要发给前端（OnChoice 路径） ===
                        if let Some(choice_data) = allowed_choice {
                            let reserve = reserve_request_user_input_budget(
                                active_milestone.as_ref(),
                                engine.conv_state.as_mut(),
                            );
                            if let FrontendToolDecision::RejectAndAbort(message) = reserve {
                                yield Ok(sse_err(message));
                                return;
                            }
                            // 把选项之前 LLM 流式输出的文字一并带给前端，
                            // 让选择卡顶部能渲染「LLM 的判断依据」。
                            // 没有这段上下文时，用户拿到的只是裸选项，决策信心差。
                            let context = strip_tool_banners(&full_text);
                            eprintln!(
                                "[web] Emitting choice_request event (context_len={})",
                                context.len()
                            );
                            let mut choice_payload = choice_data;
                            if !context.trim().is_empty() {
                                if let Some(obj) = choice_payload.as_object_mut() {
                                    obj.insert(
                                        "context".into(),
                                        serde_json::Value::String(context),
                                    );
                                }
                            }
                            let choice_event = axum::response::sse::Event::default().data(
                                serde_json::json!({ "choice_request": choice_payload }).to_string(),
                            );
                            yield Ok(choice_event);
                            return;
                        }

                        eprintln!(
                            "[web] Done — no choice_request, response_text len={}",
                            full_text.len()
                        );

                        // 校验失败的友好提示
                        if let Some(delta) = validation_delta_from_issues(&validation_issues) {
                            yield Ok(sse_delta(&delta));
                        }

                        let active_id = active_milestone.as_ref().map(|m| m.id.clone());

                        // === 分支 2: OnValidOutput 通过 → 推进 + 看是否还有下一阶段 ===
                        if should_advance {
                            if let (Some(cs), Some(ms_id)) =
                                (engine.conv_state.as_mut(), active_id.as_ref())
                            {
                                cs.mark_done(ms_id);
                            }

                            let next_active = engine
                                .conv_state
                                .as_ref()
                                .and_then(|cs| cs.active_milestone().cloned());

                            match (active_id.as_deref(), next_active) {
                                (Some(completed), Some(next)) => {
                                    advances += 1;
                                    if advances >= MAX_AUTO_MILESTONE_ADVANCES {
                                        yield Ok(sse_err(format!(
                                            "milestone 自动推进超过 {MAX_AUTO_MILESTONE_ADVANCES} 轮，已中止以避免死循环"
                                        )));
                                        return;
                                    }
                                    yield Ok(sse_stage_advanced(completed, &next.id));
                                    current_message =
                                        AUTO_CONTINUE_AFTER_ADVANCE.to_string();
                                    continue 'milestone_loop;
                                }
                                (Some(completed), None) => {
                                    yield Ok(sse_all_milestones_complete(completed));
                                    return;
                                }
                                _ => {
                                    yield Ok(sse_done());
                                    return;
                                }
                            }
                        }

                        // === 分支 3: 不推进（OnChoice 但没发选择卡 / 校验失败 / 无活跃 ms） ===
                        yield Ok(sse_stage_done_wait(
                            active_id.as_deref(),
                            &validation_issues,
                        ));
                        return;
                    }
                    Ok(StreamEvent::Error { message }) => {
                        yield Ok(sse_err(message));
                        return;
                    }
                    Ok(_) => {
                        yield Ok(sse_delta(""));
                    }
                    Err(e) => {
                        yield Ok(sse_err(e.to_string()));
                        return;
                    }
                }
            }
            // event_stream 自然结束但没遇到 Done 事件（异常情况）
            yield Ok(sse_done());
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::workflow::Milestone;
    use crate::contract::{AdvancePolicy, MilestoneContract, MilestoneMode, OutputRequirement};
    use crate::contract_runtime::TurnDirective;
    use crate::workflow::ConversationState;

    fn milestone(id: &str, mode: MilestoneMode) -> Milestone {
        Milestone {
            id: id.into(),
            label: id.into(),
            prompt_hint: Some(format!("hint {id}")),
            icon: None,
            contract: MilestoneContract {
                mode,
                question_budget: 1,
                allowed_tools: vec!["request_user_input".into()],
                advance_policy: AdvancePolicy::OnChoice,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn blocked_runtime_directive_is_web_block_not_legacy_fallback() {
        let action =
            classify_turn_directive_for_web(Ok(TurnDirective::Blocked("budget reached".into())));

        assert_eq!(action, WebTurnAction::Blocked("budget reached".into()));
    }

    #[test]
    fn request_user_input_budget_is_reserved_before_frontend_event() {
        let ms = milestone("goal", MilestoneMode::Collect);
        let mut cs = ConversationState::new("app".into(), vec![ms.clone()]);
        let args = serde_json::json!({
            "questions": [{
                "id": "goal",
                "header": "目标",
                "question": "请选择",
                "options": [
                    {"label": "A", "description": "选项 A 的详细说明，包含决策依据、关键差异和取舍点，供用户做对比时参考。"},
                    {"label": "B", "description": "选项 B 的详细说明，与 A 在关键维度上的差异点、适用场景、限制条件，方便决策。"}
                ]
            }]
        });

        let first = authorize_tool_call(Some(&ms), Some(&cs), "request_user_input", &args);
        let reserve = reserve_request_user_input_budget(Some(&ms), Some(&mut cs));
        let second = authorize_tool_call(Some(&ms), Some(&cs), "request_user_input", &args);

        assert_eq!(first, FrontendToolDecision::Accept);
        assert_eq!(reserve, FrontendToolDecision::Accept);
        assert!(matches!(
            second,
            FrontendToolDecision::RejectAndAbort(ref message)
                if message.contains("已达到提问次数上限")
        ));
        assert_eq!(cs.question_count("goal"), 1);
    }

    #[test]
    fn invalid_request_user_input_rejects_without_reserving_question_budget() {
        let mut ms = milestone("options", MilestoneMode::ProduceOptions);
        ms.contract.output_requirements = vec![OutputRequirement::MinOptions(2)];
        let cs = ConversationState::new("app".into(), vec![ms.clone()]);
        let args = serde_json::json!({
            "questions": [{
                "id": "selected_option",
                "header": "方案",
                "question": "请选择",
                "options": [{"label": "A", "description": "only one"}]
            }]
        });

        let decision = authorize_tool_call(Some(&ms), Some(&cs), "request_user_input", &args);

        assert!(matches!(
            decision,
            FrontendToolDecision::RejectAndAbort(ref message)
                if message.contains("当前阶段输出不符合契约")
        ));
        assert_eq!(cs.question_count("options"), 0);
    }

    #[test]
    fn produce_options_contract_rejects_one_option_before_frontend_receives_it() {
        use crate::contract::{MilestoneContract, MilestoneMode, OutputRequirement};
        use crate::contract_validator::ContractValidator;

        let contract = MilestoneContract {
            mode: MilestoneMode::ProduceOptions,
            allowed_tools: vec!["request_user_input".into()],
            output_requirements: vec![OutputRequirement::MinOptions(2)],
            ..Default::default()
        };
        let args = serde_json::json!({
            "questions": [{
                "id": "selected_option",
                "header": "方案",
                "question": "请选择",
                "options": [{"label": "A", "description": "only one"}]
            }]
        });
        let result = ContractValidator::validate_tool_call(&contract, "request_user_input", &args);
        assert!(!result.ok);
    }

    fn tool(name: &str) -> crate::harness::ToolDef {
        crate::harness::ToolDef {
            name: name.into(),
            description: String::new(),
            parameters: serde_json::json!({}),
        }
    }

    #[test]
    fn tool_filter_uses_active_contract_allowed_tools() {
        let mut ms = milestone("goal", MilestoneMode::Collect);
        ms.contract.allowed_tools = vec!["request_user_input".into()];
        let tools = filter_tools_for_contract(
            vec![tool("request_user_input"), tool("shell")],
            &["request_user_input".into(), "shell".into()],
            Some(&ms),
        );

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "request_user_input");
    }

    #[test]
    fn non_request_tool_call_is_rejected_when_contract_disallows_it() {
        let mut ms = milestone("goal", MilestoneMode::Collect);
        ms.contract.allowed_tools = vec!["request_user_input".into()];
        let cs = ConversationState::new("app".into(), vec![ms.clone()]);

        let decision = authorize_tool_call(Some(&ms), Some(&cs), "shell", &serde_json::json!({}));

        assert!(matches!(
            decision,
            FrontendToolDecision::RejectAndAbort(ref message)
                if message.contains("不允许调用工具 shell")
        ));
    }

    #[test]
    fn invalid_response_suppresses_choice_request() {
        let choice = Some(serde_json::json!({"call_id": "c1", "questions": []}));
        let issues = vec!["响应必须包含对比表格".to_string()];

        assert!(choice_request_allowed_after_validation(choice.clone(), &issues).is_none());
        assert_eq!(
            choice_request_allowed_after_validation(choice.clone(), &[]),
            choice
        );
    }

    #[test]
    fn suppressed_choice_request_does_not_consume_question_budget() {
        let mut ms = milestone("options", MilestoneMode::ProduceOptions);
        ms.contract.output_requirements = vec![
            OutputRequirement::MinOptions(2),
            OutputRequirement::NoOpenQuestion,
        ];
        let cs = ConversationState::new("app".into(), vec![ms.clone()]);
        let args = serde_json::json!({
            "questions": [{
                "id": "selected_option",
                "header": "方案",
                "question": "请选择",
                "options": [
                    {"label": "A", "description": "方案 A 的完整说明，含成本时间风险三个维度的分析与适用场景对比。"},
                    {"label": "B", "description": "方案 B 的完整说明，与 A 在执行难度、回报、风险上的关键差异点。"}
                ]
            }]
        });

        let decision = authorize_tool_call(Some(&ms), Some(&cs), "request_user_input", &args);
        let suppressed = choice_request_allowed_after_validation(
            Some(serde_json::json!({"call_id": "c1", "questions": args["questions"].clone()})),
            &["响应必须包含对比表格".to_string()],
        );

        assert_eq!(decision, FrontendToolDecision::Accept);
        assert!(suppressed.is_none());
        assert_eq!(cs.question_count("options"), 0);
    }

    // === slash 命令路径测试 ===

    #[test]
    // === resolve_effective_message 测试 ===

    #[test]
    fn effective_message_normal_text_passes_through() {
        let out = resolve_effective_message("帮我写周报", false, false);
        assert_eq!(out, "帮我写周报");
    }

    #[test]
    fn effective_message_continue_uses_placeholder() {
        // 用户输入"继续"，被 consume_continue_command 消费
        // effective_message 应该是过渡占位符，不能为空（否则 LLM 输出 EOS）
        let out = resolve_effective_message("继续", true, false);
        assert_eq!(out, AUTO_CONTINUE_AFTER_CONTINUE);
        assert!(!out.is_empty(), "auto-continue 不能给 LLM 空 user_message");
    }

    #[test]
    fn effective_message_tool_result_uses_placeholder() {
        // tool_result 路径：用户没发文本，只回了 choice_result
        // 不能给 LLM 空 user_message
        let out = resolve_effective_message("", false, true);
        assert_eq!(out, AUTO_CONTINUE_AFTER_TOOL_RESULT);
        assert!(!out.is_empty());
    }

    #[test]
    fn effective_message_tool_result_with_text_passes_through() {
        // 罕见：tool_result + 同时有文本消息（前端正常不会这样发）
        // 透传文本（不该用占位符覆盖用户实际输入）
        let out = resolve_effective_message("额外说明", false, true);
        assert_eq!(out, "额外说明");
    }

    #[test]
    fn effective_message_empty_no_special_returns_empty() {
        // 没有 continue 也没有 tool_result，但消息为空
        // 透传空字符串（这种情况上游应该拦掉，但函数本身保持简单）
        let out = resolve_effective_message("", false, false);
        assert_eq!(out, "");
    }

    // === backend looper 集成测试 ===

    use crate::agent_registry::{AgentDefinition, AgentRegistry};
    use crate::engine::PlatformEngine;
    use crate::engine::mock::MockHarness;
    use std::path::PathBuf;

    fn agents_registry_for_test() -> Arc<AgentRegistry> {
        let mut reg = AgentRegistry::default();
        for (id, body) in [
            ("qa", "Q&A body"),
            ("doc_generation", "Docs body"),
            ("planning", "Plans body"),
            ("generic", "Generic body"),
        ] {
            reg.register(AgentDefinition {
                id: id.into(),
                name: id.into(),
                description: format!("{id} agent"),
                emoji: None,
                body: body.into(),
            });
        }
        Arc::new(reg)
    }

    /// 把 SseItem 转换成里面的 JSON 字符串，用于 assert。
    /// 把 Debug 表示中的转义 UTF-8 字节还原为可读字符串。
    fn sse_payload(item: &SseItem) -> String {
        let event = item.as_ref().expect("SseItem 应当是 Ok");
        let raw = format!("{event:?}");
        unescape_debug_bytes(&raw)
    }

    /// 把 axum SSE Event 的 Debug 输出（含 `\xe8\xbf...` 形式的字节转义）解为 UTF-8 字符串。
    /// 非 `\xHH` 字符原样保留，便于断言里直接搜中文。
    fn unescape_debug_bytes(raw: &str) -> String {
        let mut bytes: Vec<u8> = Vec::with_capacity(raw.len());
        let chars: Vec<char> = raw.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '\\' && i + 3 < chars.len() && chars[i + 1] == 'x' {
                if let Ok(b) = u8::from_str_radix(
                    &format!("{}{}", chars[i + 2], chars[i + 3]),
                    16,
                ) {
                    bytes.push(b);
                    i += 4;
                    continue;
                }
            }
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(chars[i].encode_utf8(&mut buf).as_bytes());
            i += 1;
        }
        String::from_utf8_lossy(&bytes).to_string()
    }

    #[tokio::test]
    async fn freeform_stage_completion_advances_to_next_in_same_stream() {
        let mock = MockHarness::with_responses(vec![
            // CombinedPlanner 拆解：两个 OnValidOutput 阶段
            r#"{
                "agent": "doc_generation",
                "milestones": [
                    {"label": "草稿", "mode": "freeform", "tools": []},
                    {"label": "定稿", "mode": "final_output", "tools": []}
                ]
            }"#
            .into(),
            // 阶段 1 输出（freeform 模式，末尾句号 → NoOpenQuestion 通过）
            "这是草稿内容。".into(),
            // 阶段 2 输出（final_output 模式）
            "这是最终稿。".into(),
        ]);

        let mut engine = PlatformEngine::new(mock, PathBuf::from("."));
        engine.set_agent_registry(agents_registry_for_test());
        engine.ensure_combined_plan("写一份文档").await.unwrap();

        let engine_mutex = Arc::new(Mutex::new(engine));
        let prep = PrepOutcome {
            auto_continue_prefix: vec![],
            initial_message: "写一份文档".into(),
            user_message_to_record: Some("写一份文档".into()),
        };
        let stream = build_milestone_loop_stream(engine_mutex.clone(), prep);
        let events: Vec<SseItem> = stream.collect().await;
        let joined = events
            .iter()
            .map(sse_payload)
            .collect::<Vec<_>>()
            .join("\n");

        // 两个阶段的 LLM 输出都在同一个 SSE 流里
        assert!(
            joined.contains("草稿内容"),
            "阶段 1 输出应该在流里：{joined}"
        );
        assert!(
            joined.contains("最终稿"),
            "阶段 2 输出应该在流里：{joined}"
        );
        // 中间发了 stage_advanced（信号 AutoAdvance，不带 done）
        assert!(
            joined.contains("AutoAdvance"),
            "应该有 AutoAdvance 推进事件：{joined}"
        );
        assert!(
            joined.contains("\\\"next_action\\\":\\\"Advance\\\"")
                || joined.contains("next_action\":\"Advance"),
            "推进事件的 next_action 必须是 Advance：{joined}"
        );
        // 最后发 all_milestones_complete（done=true + signal=AllDone）
        assert!(
            joined.contains("AllDone"),
            "应该有 AllDone 收尾事件：{joined}"
        );

        // 验证 engine 状态：所有 milestone 都被 mark_done
        let engine = engine_mutex.lock().await;
        let cs = engine.conv_state.as_ref().unwrap();
        for (m, status) in &cs.milestones {
            assert_eq!(
                *status,
                crate::workflow::MilestoneStatus::Done,
                "milestone {} 应该是 Done 状态，实际 {:?}",
                m.id,
                status
            );
        }
    }

    #[tokio::test]
    async fn freeform_open_question_fails_validation_and_stops_stream() {
        // 阶段 1 输出末尾带问号 → NoOpenQuestion 失败 → 不自动推进
        let mock = MockHarness::with_responses(vec![
            r#"{
                "agent": "doc_generation",
                "milestones": [
                    {"label": "草稿", "mode": "freeform", "tools": []},
                    {"label": "定稿", "mode": "final_output", "tools": []}
                ]
            }"#
            .into(),
            "这是草稿。还有什么想补充的吗？".into(),
        ]);

        let mut engine = PlatformEngine::new(mock, PathBuf::from("."));
        engine.set_agent_registry(agents_registry_for_test());
        engine.ensure_combined_plan("写一份文档").await.unwrap();

        let engine_mutex = Arc::new(Mutex::new(engine));
        let prep = PrepOutcome {
            auto_continue_prefix: vec![],
            initial_message: "写一份文档".into(),
            user_message_to_record: Some("写一份文档".into()),
        };
        let stream = build_milestone_loop_stream(engine_mutex.clone(), prep);
        let events: Vec<SseItem> = stream.collect().await;
        let joined = events
            .iter()
            .map(sse_payload)
            .collect::<Vec<_>>()
            .join("\n");

        // 没有推进事件，也没有 AllDone
        assert!(
            !joined.contains("AutoAdvance"),
            "校验失败不应该自动推进：{joined}"
        );
        assert!(!joined.contains("AllDone"));
        // 应该看到 WaitValidation 信号
        assert!(
            joined.contains("WaitValidation"),
            "应该带 WaitValidation 信号：{joined}"
        );
        // 阶段 2 不应该被触发（response 序列只用了 plan + stage1，没 stage2）
        assert!(
            !joined.contains("定稿"),
            "阶段 2 不该被触发：{joined}"
        );

        // engine 状态：阶段 1 仍处于 Active（未被 mark_done）
        let engine = engine_mutex.lock().await;
        let cs = engine.conv_state.as_ref().unwrap();
        assert_eq!(cs.milestones[0].1, crate::workflow::MilestoneStatus::Active);
    }

    // === 新加的 SSE 事件 helper 格式 ===

    // === strip_tool_banners ===

    #[test]
    fn strip_tool_banners_keeps_llm_prose() {
        let raw = "\
基于搜索结果，我有 3 个方案推荐给你。每个方案的时间安排不同，请选择。

\n\n⏳ 正在调用工具...
🔧 [web_search] {\"query\":\"xxx\"}
📄 结果:
{\"results\":[...]}

我的判断依据是用户偏好 + 天气情况。";
        let cleaned = strip_tool_banners(raw);
        assert!(cleaned.contains("基于搜索结果，我有 3 个方案推荐给你"));
        assert!(cleaned.contains("我的判断依据"));
        assert!(!cleaned.contains("正在调用工具"));
        assert!(!cleaned.contains("[web_search]"));
        assert!(!cleaned.contains("results"));
    }

    #[test]
    fn strip_tool_banners_empty_when_only_banners() {
        let raw = "\n\n⏳ 正在调用工具...\n🔧 [exec_shell] {}\n📄 结果:\nOK\n";
        let cleaned = strip_tool_banners(raw);
        assert!(cleaned.is_empty());
    }

    #[test]
    fn strip_tool_banners_preserves_plain_text() {
        let raw = "纯文本没有工具调用。";
        assert_eq!(strip_tool_banners(raw), "纯文本没有工具调用。");
    }

    #[test]
    fn sse_stage_advanced_emits_advance_signal() {
        let event = sse_stage_advanced("ms_0", "ms_1");
        let dbg = format!("{event:?}");
        assert!(dbg.contains("ms_0"));
        assert!(dbg.contains("ms_1"));
        assert!(dbg.contains("Advance"));
        assert!(dbg.contains("AutoAdvance"));
        // 关键：不能带 done=true，否则前端会断流
        assert!(!dbg.contains("\\\"done\\\":true"));
    }

    #[test]
    fn sse_all_milestones_complete_emits_done_with_alldone_signal() {
        let event = sse_all_milestones_complete("ms_2");
        let dbg = format!("{event:?}");
        assert!(dbg.contains("ms_2"));
        assert!(dbg.contains("AllDone"));
        assert!(dbg.contains("Complete"));
        // 关键：必须带 done=true，前端据此结束流
        assert!(dbg.contains("\\\"done\\\":true") || dbg.contains("done\":true"));
    }

    #[test]
    fn sse_stage_done_wait_distinguishes_validation_vs_user_wait() {
        let waiting_user = sse_stage_done_wait(Some("ms_0"), &[]);
        let dbg_user = unescape_debug_bytes(&format!("{waiting_user:?}"));
        assert!(dbg_user.contains("WaitForUser"));

        let waiting_validation =
            sse_stage_done_wait(Some("ms_0"), &["输出末尾不能是开放问句".to_string()]);
        let dbg_val = unescape_debug_bytes(&format!("{waiting_validation:?}"));
        assert!(dbg_val.contains("WaitValidation"));
        assert!(dbg_val.contains("开放问句"));
    }

    #[test]
    fn build_command_sse_emits_delta_then_done_with_command_field() {
        let outcome = RollbackOutcome {
            message: "已回退到 a".into(),
            state_changed: true,
            trigger_replan: false,
            switch_agent: None,
        };
        let events = build_command_sse(outcome);
        assert_eq!(events.len(), 2);
        // 第一个应是 delta，第二个应是带 command 字段的 done
        let second = events
            .into_iter()
            .nth(1)
            .unwrap()
            .map_err(|_: std::convert::Infallible| ())
            .unwrap();
        // SSE Event 字段是私有的，通过 Debug 输出验证（够用）
        let dbg = format!("{:?}", second);
        assert!(dbg.contains("command"));
        assert!(dbg.contains("\\\"state_changed\\\":true") || dbg.contains("state_changed"));
    }
}

// === Milestones ===

async fn handle_milestones(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let engine = state.engine.lock().await;
    // 优先返回对话状态中的里程碑，其次返回 app.toml 默认里程碑
    let milestones: Vec<serde_json::Value> = if let Some(cs) = engine.conv_state.as_ref() {
        cs.milestones
            .iter()
            .map(|(m, s)| {
                serde_json::json!({
                    "id": m.id,
                    "label": m.label,
                    "status": match s {
                        crate::workflow::MilestoneStatus::Pending => "pending",
                        crate::workflow::MilestoneStatus::Active => "active",
                        crate::workflow::MilestoneStatus::Done => "done",
                        crate::workflow::MilestoneStatus::Skipped => "skipped",
                    },
                    "hint": m.prompt_hint,
                })
            })
            .collect()
    } else {
        vec![]
    };
    Json(serde_json::json!({ "milestones": milestones }))
}

// === Server ===

pub async fn serve(engine: PinvouEngine, port: u16) -> Result<()> {
    let state = Arc::new(AppState {
        engine: Arc::new(Mutex::new(engine)),
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/api/chat/stream", post(handle_chat_stream))
        .route("/api/milestones", get(handle_milestones))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    println!("[pinvou3] Web UI: http://{addr}");

    let url = format!("http://{addr}");
    if let Err(e) = std::process::Command::new("xdg-open").arg(&url).spawn() {
        eprintln!("[pinvou3] 无法自动打开浏览器 ({url}): {e}");
    }

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
