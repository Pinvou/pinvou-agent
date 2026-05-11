//! Web UI — axum server + 内嵌 HTML 前端。
//!
//! 路由:
//!   GET  /              → HTML 页面
//!   POST /api/chat/stream → SSE 流式 LLM 对话

use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use axum::{
    Json, Router,
    extract::State,
    response::{Html, Sse},
    routing::{get, post},
};
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
    pub engine: Mutex<PinvouEngine>,
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

async fn stream_chat(
    state: Arc<AppState>,
    req: ChatRequest,
) -> Pin<Box<dyn Stream<Item = SseItem> + Send>> {
    let mut engine = state.engine.lock().await;

    // auto-continue 累积器：choice_result 之后不再要求用户手动"继续"，
    // 把"进入下一阶段"的事件作为 prefix 拼到 LLM 流之前。
    let mut auto_continue_prefix: Vec<SseItem> = Vec::new();

    // === Slash 命令分流（早退）===
    if req.tool_result.is_none() && rollback::is_slash_command(&req.message) {
        let outcome = handle_slash_command(&mut engine, &req.message);
        return Box::pin(stream::iter(build_command_sse(outcome)));
    }

    // Plan 初始化：CombinedPlanner（单次 LLM 调用同时拆解 + 选 agent）
    if req.tool_result.is_none() {
        if let Err(e) = engine.ensure_combined_plan(&req.message).await {
            let ev = sse_err(format!("初始化计划失败: {e}"));
            return Box::pin(stream::iter(vec![Ok(ev)]));
        }
    }

    // === tool_result 处理：将用户选择作为平台事件消费 ===
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

        // 新设计：发个 milestone 状态更新（不带 done），然后继续往下跑 LLM
        engine.messages.push(crate::harness::HistoryMessage {
            role: "assistant".into(),
            content: result.summary.clone(),
        });
        auto_continue_prefix.push(Ok(sse_delta(&result.summary)));
        auto_continue_prefix.push(Ok(sse_milestone_progress(&result)));
        // 不 return，继续走主流程（会用空 user_message 触发下一阶段的 LLM）
    }

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
    );

    // === 编排：检查活跃里程碑，构造限定范围 prompt ===
    let active_milestone = engine
        .conv_state
        .as_ref()
        .and_then(|cs| cs.active_milestone().cloned());
    let all_tools = AgentHarness::tools(&engine.harness);
    let in_qa_mode = engine
        .conv_state
        .as_ref()
        .map(|cs| cs.global_mode == crate::workflow::GlobalMode::QnA)
        .unwrap_or(false);
    let tools = if in_qa_mode {
        // Q&A 模式禁用所有工具
        vec![]
    } else {
        // 不再使用 app 级工具白名单；harness 注册的全部工具走 milestone contract 过滤
        filter_tools_for_contract(all_tools, &[], active_milestone.as_ref())
    };

    let turn_action = if let (Some(cs), Some(ms)) =
        (engine.conv_state.as_ref(), active_milestone.as_ref())
    {
        classify_turn_directive_for_web(ContractRuntime::next_directive(ms, cs, effective_message))
    } else {
        // QnA 模式或未初始化：直接进 LLM 流（无 contract 约束）
        WebTurnAction::FreeFlow
    };

    let chat_req = match turn_action {
        WebTurnAction::Blocked(message) | WebTurnAction::CompleteStep(message) => {
            if let Some(ref mut cs) = engine.conv_state {
                cs.increment_turn();
            }
            if !req.message.is_empty() {
                engine.messages.push(crate::harness::HistoryMessage {
                    role: "user".into(),
                    content: req.message.clone(),
                });
            }
            engine.messages.push(crate::harness::HistoryMessage {
                role: "assistant".into(),
                content: message.clone(),
            });
            return Box::pin(stream::iter(vec![Ok(sse_delta(&message)), Ok(sse_done())]));
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
            return Box::pin(stream::iter(vec![Ok(choice_event)]));
        }
        WebTurnAction::CallLlm => match engine.build_next_contract_prompt(effective_message) {
            Ok(step_prompt) => {
                let mut req = engine.build_request(effective_message, tools);
                req.platform_system_prompt = Some(step_prompt.system);
                req
            }
            Err(_) => engine.build_request(effective_message, tools),
        },
        WebTurnAction::FreeFlow => engine.build_request(effective_message, tools),
    };
    if let Some(ref mut cs) = engine.conv_state {
        cs.increment_turn();
    }
    // 追加用户消息到历史（tool_result 场景不重复追加空的 user message）
    if !req.message.is_empty() {
        engine.messages.push(crate::harness::HistoryMessage {
            role: "user".into(),
            content: req.message.clone(),
        });
    }

    let event_stream = match AgentHarness::chat_stream(&engine.harness, chat_req).await {
        Ok(s) => s,
        Err(e) => {
            let ev = sse_err(e.to_string());
            return Box::pin(stream::iter(vec![Ok(ev)]));
        }
    };

    // 收集响应文本 + 编排状态
    let full_text = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let ft_clone = full_text.clone();
    let state_clone = state.clone();
    let state_for_choice = state.clone();
    let milestone_for_check = active_milestone.clone();
    // 工具调用累积：用于 validate_stage_completion 的 RequiresToolCall 校验
    let invoked_tools: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let it_clone = invoked_tools.clone();
    // choice 状态：检测到 request_user_input 工具调用时设值
    let choice_state: std::sync::Arc<std::sync::Mutex<Option<serde_json::Value>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let ch_clone = choice_state.clone();
    let aborted = Arc::new(AtomicBool::new(false));
    let abort_for_stream = aborted.clone();

    let sse_stream = event_stream.then(move |result| {
        let ft_clone = ft_clone.clone();
        let state_clone = state_clone.clone();
        let state_for_choice = state_for_choice.clone();
        let milestone_for_check = milestone_for_check.clone();
        let it_clone = it_clone.clone();
        let ch_clone = ch_clone.clone();
        let abort_for_stream = abort_for_stream.clone();

        async move {
            if abort_for_stream.load(Ordering::SeqCst) {
                return Vec::new();
            }

            let event = match result {
            Ok(StreamEvent::TextDelta { content }) => {
                if let Ok(mut ft) = ft_clone.lock() {
                    ft.push_str(&content);
                }
                vec![Ok(sse_delta(&content))]
            }
            Ok(StreamEvent::ToolCallStart { call_id, tool_name, arguments }) => {
                eprintln!("[web] ToolCallStart: {tool_name} call_id={call_id} args={arguments:?}");
                // 记录本阶段调用过的工具，用于 validate_stage_completion 的 RequiresToolCall
                if let Ok(mut it) = it_clone.lock() {
                    if !it.iter().any(|t| t == &tool_name) {
                        it.push(tool_name.clone());
                    }
                }
                let decision = {
                    let engine = state_for_choice.engine.lock().await;
                    authorize_tool_call(
                        milestone_for_check.as_ref(),
                        engine.conv_state.as_ref(),
                        &tool_name,
                        &arguments,
                    )
                };

                if let FrontendToolDecision::RejectAndAbort(message) = decision {
                    abort_for_stream.store(true, Ordering::SeqCst);
                    return vec![Ok(sse_err(message))];
                }

                if tool_name == "request_user_input" {
                    // 拦截 request_user_input 工具调用，推送给前端
                    if let Ok(mut ch) = ch_clone.lock() {
                        *ch = Some(serde_json::json!({
                            "call_id": call_id,
                            "questions": arguments.get("questions").cloned().unwrap_or(serde_json::Value::Null),
                        }));
                    }
                }
                // 工具调用不直接透传给前端；request_user_input 在 Done 时发选择卡。
                vec![Ok(sse_delta(""))]
            }
            Ok(StreamEvent::Done) => {
                let (
                    response_text,
                    milestone_event,
                    should_advance_milestone,
                    choice_request,
                    validation_delta,
                ) = {
                    let ft = ft_clone.lock().unwrap();
                    let text = ft.clone();
                    let choice = ch_clone.lock().unwrap().take();

                    // 推进由 ContractValidator 单独裁决：
                    // validate_stage_completion 同时校验响应文本（NoOpenQuestion）
                    // 和阶段必调工具（RequiresToolCall）
                    let invoked = it_clone.lock().map(|v| v.clone()).unwrap_or_default();
                    let (milestone_json, should_advance, validation_issues) = if let Some(ms) = &milestone_for_check {
                        let contract_ok = ContractValidator::validate_stage_completion(
                            &ms.contract,
                            &text,
                            &invoked,
                        );
                        let should_advance = contract_ok.ok
                            && matches!(
                                ms.contract.advance_policy,
                                crate::contract::AdvancePolicy::OnValidOutput
                            );
                        let validation_issues = if contract_ok.ok {
                            vec![]
                        } else {
                            contract_ok.issues.clone()
                        };
                        let next_action = if should_advance { "Advance" } else { "Wait" };
                        (
                            Some(serde_json::json!({
                                "milestone_id": ms.id,
                                "next_action": next_action,
                                "validation_issues": validation_issues.clone(),
                            })),
                            should_advance,
                            validation_issues,
                        )
                    } else {
                        (None, false, vec![])
                    };

                    let validation_delta = validation_delta_from_issues(&validation_issues);
                    let choice = choice_request_allowed_after_validation(choice, &validation_issues);

                    (text, milestone_json, should_advance, choice, validation_delta)
                };

                let mut choice_budget_error = None;
                {
                    let mut engine = state_clone.engine.lock().await;
                    engine.messages.push(crate::harness::HistoryMessage {
                        role: "assistant".into(),
                        content: response_text.clone(),
                    });
                    if let (Some(ms), Some(mj), Some(ref mut cs)) = (
                        milestone_for_check.as_ref(),
                        milestone_event.as_ref(),
                        engine.conv_state.as_mut(),
                    ) {
                        if should_advance_milestone && mj.is_object() {
                            cs.mark_done(&ms.id);
                        }
                    }
                    if choice_request.is_some() {
                        if let FrontendToolDecision::RejectAndAbort(message) =
                            reserve_request_user_input_budget(
                                milestone_for_check.as_ref(),
                                engine.conv_state.as_mut(),
                            )
                        {
                            choice_budget_error = Some(message);
                        }
                    }
                }

                if let Some(message) = choice_budget_error {
                    return vec![Ok(sse_err(message))];
                }

                // 如果有 choice_request，优先发送（前端渲染选择卡片）
                if let Some(ref choice_data) = choice_request {
                    eprintln!("[web] Emitting choice_request event: {choice_data:?}");
                    let choice_event = axum::response::sse::Event::default()
                        .data(serde_json::json!({"choice_request": choice_data}).to_string());
                    return vec![Ok(choice_event)];
                }
                eprintln!("[web] Done — no choice_request, response_text len={}", response_text.len());

                if let Some(mj) = milestone_event {
                    let done_event = axum::response::sse::Event::default()
                        .data(serde_json::json!({"done": true, "milestone": mj}).to_string());
                    let mut events = Vec::new();
                    if let Some(delta) = validation_delta {
                        events.push(Ok(sse_delta(&delta)));
                    }
                    events.push(Ok(done_event));
                    return events;
                }
                vec![Ok(sse_done())]
            }
            Ok(StreamEvent::Error { message }) => vec![Ok(sse_err(message))],
            Ok(_) => vec![Ok(sse_delta(""))],
            Err(e) => vec![Ok(sse_err(e.to_string()))],
            };
            event
        }
    }).flat_map(stream::iter);

    if auto_continue_prefix.is_empty() {
        Box::pin(sse_stream)
    } else {
        // Auto-continue：把 choice_result 的 ack + milestone 推进事件拼到 LLM 流之前
        Box::pin(stream::iter(auto_continue_prefix).chain(sse_stream))
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
                    {"label": "A", "description": "a"},
                    {"label": "B", "description": "b"}
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
                    {"label": "A", "description": "first"},
                    {"label": "B", "description": "second"}
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
        engine: Mutex::new(engine),
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
