//! pinvou3-app 与 CodeWhale Engine 的桥接层。
//!
//! 职责：
//!  1. 通过 [`bridge::Pinvou3Bridge`] 把 `~/.pinvou3/settings.json` 翻译成
//!     [`EngineConfig`] / [`DtConfig`]，然后 `spawn_engine`，存到 Tauri State
//!  2. 后台 task 持续读 `EngineHandle::rx_event`，转译成 Tauri 事件
//!     （`chat:delta` / `chat:reasoning_start` / `chat:reasoning_delta` /
//!     `chat:reasoning_done` / `chat:tool_start` / `chat:tool_end` / `chat:done`
//!     / `chat:plan_ready`）
//!  3. 暴露 `send_reserved_user_message()` 给 [`commands::chat`] 调用
//!
//! 所有配置决策（model / paths / locale / allow_shell ...）都在 bridge 里，
//! 这一层只做 "boot engine + 转发事件"。Engine 自管 session 状态，多轮对话
//! 在同一个 EngineHandle 内自然累积。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use deepseek_tui::core::engine::{spawn_engine, EngineHandle};
use deepseek_tui::core::events::{Event, TurnOutcomeStatus};
use deepseek_tui::core::ops::Op;
use deepseek_tui::models::Message;
use deepseek_tui::tools::shell::{new_shared_shell_manager, SharedShellManager};
use deepseek_tui::tools::user_input::UserInputResponse;
use deepseek_tui::tui::app::AppMode;
use parking_lot::Mutex;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::broadcast;

pub(crate) use crate::features::assistant::engine_support::EngineTurnSignal;
use crate::features::assistant::engine_support::{
    apply_scheduled_turn_policy, maybe_notify_task_completed, persist_successful_tool_artifact,
    scheduled_tool_should_auto_approve, TurnCompletionTracker,
};
use crate::features::assistant::expert_roster::{
    cleanup_legacy_expert_projection, ExpertRosterSnapshot,
};
use crate::features::assistant::platform::bridge::Pinvou3Bridge;
use crate::features::assistant::turn_shell_tasks::TurnShellTaskRegistry;
use crate::features::sessions::{
    transcript_revision, ChatEngineState, ScheduledEngineState, ScheduledRunProfile,
    ScheduledTokenAccounting, SessionStore,
};
use crate::features::sessions::{SerializableMode, SessionModeState};

/// 定时任务无人值守首轮的唯一附加约束。任务 prompt 原文已作为用户消息传入，
/// 这一句只防模型把目标改写成别的事，不再堆叠更长的提示词。
const SCHEDULED_TURN_REMINDER: &str =
    "本轮是定时任务的自动执行：直接执行用户消息里的任务，不要改写、替换或扩展任务目标。";

/// 单个 session 的 engine wrapper(handle + 该 session 绑定的 bridge)。
///
/// 多引擎并发模型下,[`EnginePool`](crate::features::assistant::engine_pool::EnginePool) 为每个 session
/// 持有一个 `AppEngine`(经 [`spawn_for_session`](Self::spawn_for_session) 创建);
/// L1 headless harness 经 [`spawn_headless`](Self::spawn_headless) 单独用一个。
/// Clone 廉价(EngineHandle 内部 Arc)。
#[derive(Clone)]
pub struct AppEngine {
    pub handle: EngineHandle,
    pub bridge: Pinvou3Bridge,
    /// 本 engine 绑定的 session id（多引擎并发下 per-session 一个 AppEngine）；
    /// 发送 op 构造按它取会话策略。headless harness 无 session 概念,置空。
    pub(crate) session_id: String,
    pub(crate) workspace: PathBuf,
    /// 启动本引擎时会话是否处于多智能体模式。开关切换会先回收旧引擎，
    /// 因而该值在一个引擎实例的生命周期内保持稳定。
    multi_agent_enabled: bool,
    pub(crate) turn_events: broadcast::Sender<EngineTurnSignal>,
    pub(crate) scheduled_unattended: Arc<AtomicBool>,
    turn_lifecycle: Arc<TurnLifecycle>,
    turn_shell_tasks: Option<TurnShellTaskRegistry>,
    scheduled_disallowed_tools: Vec<String>,
}

#[derive(Debug, Default)]
struct TurnLifecycleState {
    active: bool,
    submitted: bool,
    turn_id: Option<String>,
    terminal_emitted: bool,
    terminal_closing: bool,
    reclaimed: bool,
    admission_emitted: bool,
    next_reservation_id: u64,
    /// 单调递增的轮次身份（turn generation）。每次 turn 从 idle 变 active
    /// 时自增（[`reserve`](TurnLifecycle::reserve) / [`on_submitted`] /
    /// [`on_started_transition`] 的 newly_active 分支），用于把 cancel 请求
    /// 绑定到发起时刻的轮次：cancel 在取 `turn_lock` 前后比对 epoch，不匹配
    /// 说明目标轮已结束（新一轮已 reserve），当前请求必须 no-op，避免跨轮
    /// 误取消随后启动的新 turn。
    ///
    /// 不能复用 `next_reservation_id`：`on_submitted`（定时任务路径）与
    /// `on_started_transition`（stale TurnStarted）激活 turn 时都不碰它，
    /// 会让定时 turn 的 generation 恒为旧值。
    ///
    /// [`on_submitted`]: TurnLifecycle::on_submitted
    /// [`on_started_transition`]: TurnLifecycle::on_started_transition
    turn_epoch: u64,
    active_reservation_id: Option<u64>,
    transcript_rules: Vec<TranscriptSanitizationRule>,
    /// 用户在 turn 已 submit 但尚未收到 TurnStarted 时点了停止。
    ///
    /// CodeWhale Engine 在每个新轮次入口**无条件**调
    /// `reset_cancel_token()`（`core/engine.rs:2664`，在 `TurnStarted`
    /// 之前），用全新 token 覆盖当前共享 token。若 cancel 恰好在这个窗口
    /// 调了 `cancel_current()`，取消信号会被重置丢弃。
    ///
    /// 此标记由 [`arm_pending_cancel`] 在 cancel 路径设置（仅当 turn 尚未
    /// started），由事件转发器在收到 `TurnStarted` 后通过
    /// [`take_pending_cancel`] 原子取出并重新 `cancel_current()`——此时
    /// `reset_cancel_token()` 已经执行过，cancel 命中的是当前轮次的活跃 token。
    ///
    /// 携带 arming 时的 [`turn_epoch`]：并发取消请求（C1/C2）中，排队较晚的
    /// C2 在恢复后读到的是「当前 lifecycle」（可能已是新轮）。`take_pending_cancel`
    /// 会校验 epoch 仍是当前轮，跨轮泄漏的 stale pending 被丢弃，不误取消新轮。
    ///
    /// [`arm_pending_cancel`]: TurnLifecycle::arm_pending_cancel_and_cancel
    /// [`take_pending_cancel`]: TurnLifecycle::take_pending_cancel
    /// [`turn_epoch`]: TurnLifecycleState::turn_epoch
    pending_cancel: Option<u64>,
}

/// The durable, user-visible meaning of a submitted engine operation.
///
/// `EditLast` deliberately differs from append: the engine truncates the last
/// user message and every message after it before adding the replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptOperation {
    Append,
    EditLast,
}

impl TranscriptOperation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Append => "append",
            Self::EditLast => "edit_last",
        }
    }
}

#[derive(Debug, Clone)]
struct TranscriptSanitizationRule {
    reservation_id: u64,
    submitted: bool,
    actual_user_content: String,
    display_message: Message,
    display_content: String,
    #[allow(dead_code)]
    operation: TranscriptOperation,
    baseline_revision: Option<String>,
    admission_metadata: Option<TurnAdmissionMetadata>,
    superseded: Option<Box<TranscriptSanitizationRule>>,
}

#[derive(Debug, Clone)]
struct ReservedTranscript {
    operation: TranscriptOperation,
    display_message: Message,
    display_content: String,
    baseline_revision: Option<String>,
}

/// Extra fields carried only by semantic admissions such as accepting a plan.
/// Ordinary chat and edit admissions intentionally leave this absent so their
/// wire payload stays unchanged.
#[derive(Debug, Clone)]
pub(crate) struct TurnAdmissionMetadata {
    action: &'static str,
    plan_id: String,
    mode: SerializableMode,
    mode_state: SessionModeState,
}

impl TurnAdmissionMetadata {
    pub(crate) fn accept_plan(plan_id: String, mode_state: SessionModeState) -> Self {
        Self {
            action: "accept_plan",
            plan_id,
            mode: SerializableMode::Yolo,
            mode_state,
        }
    }
}

#[derive(Debug, Clone)]
struct TurnAdmission {
    content: String,
    operation: TranscriptOperation,
    base_transcript_revision: Option<String>,
    metadata: Option<TurnAdmissionMetadata>,
}

impl TurnAdmission {
    fn user_payload(&self, session_id: &str) -> serde_json::Value {
        let mut payload = json!({
            "session_id": session_id,
            "content": self.content,
            "operation": self.operation.as_str(),
            "base_transcript_revision": self.base_transcript_revision,
        });
        if let Some(metadata) = self.metadata.as_ref() {
            payload["action"] = json!(metadata.action);
            payload["plan_id"] = json!(metadata.plan_id);
            payload["mode"] = json!(metadata.mode);
            payload["mode_state"] = json!(metadata.mode_state);
        }
        payload
    }
}

#[derive(Debug, Clone)]
struct TranscriptFallback {
    operation: TranscriptOperation,
    display_message: Message,
    baseline_revision: String,
}

/// RAII admission for one session turn.
///
/// Creating the reservation atomically marks the session busy. Dropping it
/// before the engine operation is accepted rolls that state back, including
/// any transcript sanitization rule prepared for the operation. Once
/// `mark_submitted` is called, the authoritative engine terminal event owns
/// lifecycle completion.
pub(crate) struct TurnReservation {
    lifecycle: Arc<TurnLifecycle>,
    reservation_id: u64,
    base_transcript_revision: Option<String>,
    transcript: Option<ReservedTranscript>,
    admission_metadata: Option<TurnAdmissionMetadata>,
    submitted: bool,
}

impl TurnReservation {
    fn new(lifecycle: Arc<TurnLifecycle>, reservation_id: u64) -> Self {
        Self {
            lifecycle,
            reservation_id,
            base_transcript_revision: None,
            transcript: None,
            admission_metadata: None,
            submitted: false,
        }
    }

    pub(crate) fn set_base_transcript_revision(&mut self, revision: String) {
        self.base_transcript_revision = Some(revision);
    }

    pub(crate) fn base_transcript_revision(&self) -> Option<&str> {
        self.base_transcript_revision.as_deref()
    }

    pub(crate) fn set_transcript(
        &mut self,
        operation: TranscriptOperation,
        display_message: Message,
    ) -> Result<()> {
        if self.submitted {
            anyhow::bail!("turn reservation was already submitted");
        }
        if display_message.role != "user" {
            anyhow::bail!("display transcript message must have role=user");
        }
        let display_content = match display_message.content.first() {
            Some(deepseek_tui::models::ContentBlock::Text { text, .. }) => text.clone(),
            _ => anyhow::bail!("display transcript message must start with text content"),
        };
        self.transcript = Some(ReservedTranscript {
            operation,
            display_message,
            display_content,
            baseline_revision: None,
        });
        Ok(())
    }

    pub(crate) fn set_transcript_with_baseline(
        &mut self,
        operation: TranscriptOperation,
        display_message: Message,
        baseline_revision: String,
    ) -> Result<()> {
        self.set_transcript(operation, display_message)?;
        if let Some(transcript) = self.transcript.as_mut() {
            transcript.baseline_revision = Some(baseline_revision);
        }
        Ok(())
    }

    pub(crate) fn set_admission_metadata(&mut self, metadata: TurnAdmissionMetadata) -> Result<()> {
        if self.submitted {
            anyhow::bail!("turn reservation was already submitted");
        }
        self.admission_metadata = Some(metadata);
        Ok(())
    }

    fn belongs_to(&self, lifecycle: &Arc<TurnLifecycle>) -> bool {
        Arc::ptr_eq(&self.lifecycle, lifecycle)
    }

    pub(crate) fn ensure_active(&self) -> Result<()> {
        self.lifecycle
            .ensure_reservation_active(self.reservation_id)
    }

    fn prepare_actual_user_content(&self, actual_user_content: String) -> Result<()> {
        let Some(transcript) = self.transcript.as_ref() else {
            return Ok(());
        };
        self.lifecycle.install_transcript_rule(
            self.reservation_id,
            actual_user_content,
            transcript.display_message.clone(),
            transcript.display_content.clone(),
            transcript.operation,
            transcript.baseline_revision.clone(),
            self.admission_metadata.clone(),
        )
    }

    fn mark_submitted(mut self) {
        self.lifecycle
            .mark_reservation_submitted(self.reservation_id);
        self.submitted = true;
    }
}

impl Drop for TurnReservation {
    fn drop(&mut self) {
        if !self.submitted {
            self.lifecycle.on_reservation_failed(self.reservation_id);
        }
    }
}

/// Session-scoped turn state shared by the command path and the event
/// forwarder. Every terminal path competes here, so the frontend receives
/// exactly one `chat:done` for a submitted turn.
#[derive(Debug, Default)]
pub(crate) struct TurnLifecycle {
    state: Mutex<TurnLifecycleState>,
    emission: Mutex<()>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct EmittedTerminal {
    turn_id: Option<String>,
}

#[derive(Debug)]
struct TerminalTransition {
    terminal: EmittedTerminal,
    admission: Option<TurnAdmission>,
    fallback: Option<TranscriptFallback>,
}

#[derive(Debug)]
struct StartedTransition {
    admission: Option<TurnAdmission>,
    /// 本次 Started 是否把轮次从空闲翻成活跃（去重：一轮内的重复 Started
    /// 不再重复宣告）。
    newly_active: bool,
}

pub(crate) fn emit_chat_terminal(
    app: &AppHandle,
    session_id: &str,
    status: TurnOutcomeStatus,
    error: Option<String>,
    shell_cleanup_failed: bool,
) {
    // turn 终态兜底清空挂起的输入请求（取消/中断时可能没有对应 tool_end）。
    crate::features::assistant::pending_user_input::clear_session(session_id);
    let payload = json!({
        "session_id": session_id,
        "status": format!("{status:?}"),
        "error": error,
        "shell_cleanup_failed": shell_cleanup_failed,
    });
    let _ = app.emit("chat:done", payload.clone());
    crate::features::remote_control::forward_app_event(app, "chat:done", payload);
}

fn emit_turn_started(app: &AppHandle, session_id: &str) {
    let started_payload = json!({ "session_id": session_id });
    let _ = app.emit("chat:turn_started", started_payload.clone());
    crate::features::remote_control::forward_app_event(app, "chat:turn_started", started_payload);
}

fn emit_turn_admission(app: &AppHandle, session_id: &str, admission: TurnAdmission) {
    let user_payload = admission.user_payload(session_id);
    let _ = app.emit("chat:user_message", user_payload.clone());
    crate::features::remote_control::forward_app_event(app, "chat:user_message", user_payload);
    emit_turn_started(app, session_id);
}

impl TurnLifecycle {
    /// 该 session 当前是否有进行中的 turn（reserve 占用或终态收口未完成）。
    pub(crate) fn is_active(&self) -> bool {
        let state = self.state.lock();
        state.active || state.terminal_closing
    }

    /// 当前活动轮的 turn generation（epoch），供 cancel 请求绑定发起时刻的
    /// 轮次身份。与 [`is_active`](Self::is_active) 同口径：turn 活动期间
    /// （`active` 或终态发送临界区 `terminal_closing`）返回 `Some(epoch)`，
    /// 空闲返回 `None`。cancel 在取 `turn_lock` 前后各读一次，不匹配则 no-op。
    pub(crate) fn current_turn_generation(&self) -> Option<u64> {
        let state = self.state.lock();
        if state.active || state.terminal_closing {
            Some(state.turn_epoch)
        } else {
            None
        }
    }

    /// 当前活动轮是否已提交（`SendMessage` 已入队 / `TurnStarted` 已抵达）。
    /// 供 cancel 阶段二在 generation mismatch（新轮已 reserve）时判断新轮是否
    /// 已经真正开始：仅 reserve 未 send 时，engine 里仍是旧轮遗留的子代理，
    /// 补发级联取消（CancelSubAgents）不会误杀新轮刚启动的子代理（reviewer 点 9）。
    pub(crate) fn is_current_turn_submitted(&self) -> bool {
        let state = self.state.lock();
        state.active && state.submitted
    }

    pub(crate) fn reserve(self: &Arc<Self>) -> Result<TurnReservation> {
        let reservation_id = {
            let mut state = self.state.lock();
            if state.active || state.terminal_closing {
                anyhow::bail!("session_turn_in_progress");
            }
            state.next_reservation_id = state.next_reservation_id.wrapping_add(1).max(1);
            let reservation_id = state.next_reservation_id;
            state.turn_epoch = state.turn_epoch.wrapping_add(1).max(1);
            state.active = true;
            state.submitted = false;
            state.turn_id = None;
            state.terminal_emitted = false;
            state.terminal_closing = false;
            state.reclaimed = false;
            state.admission_emitted = false;
            state.active_reservation_id = Some(reservation_id);
            state.pending_cancel = None;
            reservation_id
        };
        Ok(TurnReservation::new(self.clone(), reservation_id))
    }

    fn install_transcript_rule(
        &self,
        reservation_id: u64,
        actual_user_content: String,
        display_message: Message,
        display_content: String,
        operation: TranscriptOperation,
        baseline_revision: Option<String>,
        admission_metadata: Option<TurnAdmissionMetadata>,
    ) -> Result<()> {
        let mut state = self.state.lock();
        if !state.active || state.active_reservation_id != Some(reservation_id) {
            anyhow::bail!("turn reservation invalidated or no longer active");
        }
        state
            .transcript_rules
            .retain(|rule| rule.reservation_id != reservation_id);
        let superseded = if operation == TranscriptOperation::EditLast {
            state.transcript_rules.pop().map(Box::new)
        } else {
            None
        };
        state.transcript_rules.push(TranscriptSanitizationRule {
            reservation_id,
            submitted: false,
            actual_user_content,
            display_message,
            display_content,
            operation,
            baseline_revision,
            admission_metadata,
            superseded,
        });
        Ok(())
    }

    fn ensure_reservation_active(&self, reservation_id: u64) -> Result<()> {
        let state = self.state.lock();
        if state.active
            && !state.reclaimed
            && !state.terminal_emitted
            && state.active_reservation_id == Some(reservation_id)
        {
            Ok(())
        } else {
            anyhow::bail!("turn reservation invalidated or no longer active")
        }
    }

    fn mark_reservation_submitted(&self, reservation_id: u64) {
        let mut state = self.state.lock();
        if let Some(rule) = state
            .transcript_rules
            .iter_mut()
            .find(|rule| rule.reservation_id == reservation_id)
        {
            rule.submitted = true;
        }
        if state.active && state.active_reservation_id == Some(reservation_id) {
            state.submitted = true;
        }
    }

    fn on_reservation_failed(&self, reservation_id: u64) {
        let mut state = self.state.lock();
        if let Some(index) = state
            .transcript_rules
            .iter()
            .position(|rule| rule.reservation_id == reservation_id)
        {
            // A real TurnStarted can race ahead of EngineHandle::send returning.
            // In that case the forwarder, not the command future, has already
            // transferred ownership to the engine. Preserve the durable rule.
            if state.transcript_rules[index].submitted {
                return;
            }
            let failed = state.transcript_rules.remove(index);
            if let Some(superseded) = failed.superseded {
                state.transcript_rules.insert(index, *superseded);
            }
        }
        if state.active_reservation_id != Some(reservation_id)
            || state.submitted
            || state.turn_id.is_some()
            || state.terminal_emitted
        {
            return;
        }
        state.active = false;
        state.submitted = false;
        state.active_reservation_id = None;
        drop(state);
    }

    /// Replace every engine-only user prompt registered during this engine's
    /// lifetime with its UI display message. Rules intentionally survive turn
    /// completion because later `SessionUpdated` snapshots still contain the
    /// raw prompts from all earlier turns until the engine is rebuilt.
    fn sanitize_messages(&self, mut messages: Vec<Message>) -> (Vec<Message>, bool) {
        let state = self.state.lock();
        let active_reservation_id = state.active_reservation_id;
        let mut active_rule_matched = false;
        for rule in state.transcript_rules.iter().rev() {
            let Some(index) = messages.iter().rposition(|message| {
                message.role == "user"
                    && matches!(
                        message.content.first(),
                        Some(deepseek_tui::models::ContentBlock::Text { text, .. })
                            if text == &rule.actual_user_content
                    )
            }) else {
                continue;
            };
            messages[index] = rule.display_message.clone();
            if active_reservation_id == Some(rule.reservation_id) {
                active_rule_matched = true;
            }
        }
        (messages, active_rule_matched)
    }

    fn active_transcript_fallback(&self) -> Option<TranscriptFallback> {
        let state = self.state.lock();
        Self::active_transcript_fallback_locked(&state)
    }

    fn active_transcript_fallback_locked(state: &TurnLifecycleState) -> Option<TranscriptFallback> {
        let active = state.active_reservation_id?;
        let rule = state
            .transcript_rules
            .iter()
            .find(|rule| rule.reservation_id == active)?;
        Some(TranscriptFallback {
            operation: rule.operation,
            display_message: rule.display_message.clone(),
            baseline_revision: rule.baseline_revision.clone()?,
        })
    }

    fn take_admission_locked(state: &mut TurnLifecycleState) -> Option<TurnAdmission> {
        if state.admission_emitted {
            return None;
        }
        let active_reservation_id = state.active_reservation_id?;
        let rule = state
            .transcript_rules
            .iter()
            .find(|rule| rule.reservation_id == active_reservation_id)?;
        let admission = TurnAdmission {
            content: rule.display_content.clone(),
            operation: rule.operation,
            base_transcript_revision: rule.baseline_revision.clone(),
            metadata: rule.admission_metadata.clone(),
        };
        state.admission_emitted = true;
        Some(admission)
    }

    fn remove_unsubmitted_rule_locked(state: &mut TurnLifecycleState, reservation_id: u64) {
        let Some(index) = state
            .transcript_rules
            .iter()
            .position(|rule| rule.reservation_id == reservation_id && !rule.submitted)
        else {
            return;
        };
        let invalidated = state.transcript_rules.remove(index);
        if let Some(superseded) = invalidated.superseded {
            state.transcript_rules.insert(index, *superseded);
        }
    }

    pub(crate) fn on_submitted(&self) -> bool {
        let mut state = self.state.lock();
        if !state.active && !state.terminal_closing {
            state.turn_epoch = state.turn_epoch.wrapping_add(1).max(1);
            state.active = true;
            state.submitted = true;
            state.turn_id = None;
            state.terminal_emitted = false;
            state.reclaimed = false;
            state.admission_emitted = false;
            state.active_reservation_id = None;
            true
        } else {
            false
        }
    }

    fn on_submission_failed(&self, activated: bool) {
        if !activated {
            return;
        }
        let mut state = self.state.lock();
        if state.turn_id.is_none() && !state.terminal_emitted {
            state.active = false;
            state.submitted = false;
            state.active_reservation_id = None;
            drop(state);
        }
    }

    fn on_started_transition(&self, turn_id: String) -> Option<StartedTransition> {
        let mut state = self.state.lock();
        if state.reclaimed || state.terminal_closing {
            return None;
        }
        let newly_active = !state.active;
        if newly_active {
            state.admission_emitted = false;
            // forwarder 收到 stale/repeat TurnStarted 时可能从 idle 激活 turn
            // （reserve/on_submitted 之外的第三个 active 入口）。同样推进 epoch
            // 保证 cancel generation 严格单调，覆盖该边缘路径。
            state.turn_epoch = state.turn_epoch.wrapping_add(1).max(1);
        }
        state.active = true;
        state.submitted = true;
        state.turn_id = Some(turn_id);
        state.terminal_emitted = false;
        if let Some(reservation_id) = state.active_reservation_id {
            if let Some(rule) = state
                .transcript_rules
                .iter_mut()
                .find(|rule| rule.reservation_id == reservation_id)
            {
                rule.submitted = true;
            }
        }
        let transition = StartedTransition {
            admission: Self::take_admission_locked(&mut state),
            newly_active,
        };
        drop(state);
        Some(transition)
    }

    #[cfg(test)]
    fn on_started(&self, turn_id: String) {
        let _ = self.on_started_transition(turn_id);
    }

    fn emit_started_admission(&self, app: &AppHandle, session_id: &str, turn_id: String) -> bool {
        let _emission = self.emission.lock();
        let Some(transition) = self.on_started_transition(turn_id) else {
            return false;
        };
        if let Some(admission) = transition.admission {
            emit_turn_admission(app, session_id, admission);
        } else if transition.newly_active {
            // 底座自启的续跑轮（如子智能体完成后的父模型汇总轮）没有外部
            // admission：不发 user_message，但 turn_started 必须照发——否则
            // 界面显示空闲、停止按钮缺席、再发消息撞"已有运行中轮次"，且与
            // "停止级联取消"的既定语义冲突（复核 P1）。
            emit_turn_started(app, session_id);
        }
        true
    }

    fn claim_terminal_transition(&self) -> Option<TerminalTransition> {
        let transition = {
            let mut state = self.state.lock();
            if !state.active || !state.submitted || state.terminal_emitted {
                return None;
            }
            let admission = Self::take_admission_locked(&mut state);
            let fallback = Self::active_transcript_fallback_locked(&state);
            state.terminal_emitted = true;
            state.terminal_closing = true;
            state.active = false;
            state.submitted = false;
            state.active_reservation_id = None;
            TerminalTransition {
                terminal: EmittedTerminal {
                    turn_id: state.turn_id.take(),
                },
                admission,
                fallback,
            }
        };
        Some(transition)
    }

    /// Reclaim a lifecycle after an engine disappears. A reservation which
    /// never reached the engine mailbox is invalidated silently; only an
    /// accepted/started operation owns a user-visible terminal event.
    fn claim_reclaimed_transition(&self) -> Option<TerminalTransition> {
        let transition = {
            let mut state = self.state.lock();
            if !state.active || state.terminal_emitted {
                return None;
            }
            state.reclaimed = true;
            if !state.submitted {
                if let Some(reservation_id) = state.active_reservation_id.take() {
                    Self::remove_unsubmitted_rule_locked(&mut state, reservation_id);
                }
                state.active = false;
                state.turn_id = None;
                None
            } else {
                let admission = Self::take_admission_locked(&mut state);
                let fallback = Self::active_transcript_fallback_locked(&state);
                state.terminal_emitted = true;
                state.terminal_closing = true;
                state.active = false;
                state.submitted = false;
                state.active_reservation_id = None;
                Some(TerminalTransition {
                    terminal: EmittedTerminal {
                        turn_id: state.turn_id.take(),
                    },
                    admission,
                    fallback,
                })
            }
        };
        transition
    }

    #[cfg(test)]
    pub(crate) fn claim_terminal(&self) -> Option<EmittedTerminal> {
        self.claim_terminal_transition()
            .map(|transition| transition.terminal)
    }

    /// 测试专用：认领一个未提交 turn 的终态并立即重开闸门，等价于
    /// `emit_unsubmitted_interrupted_terminal` 去掉发 `chat:done` 的副作用，
    /// 供跨模块测试（engine_pool 的 cancel 并发测试）驱动 turn 收尾而无需 AppHandle。
    /// 不发终态信号——这些测试只关心 lifecycle 状态机的轮次切换，不验证 emit。
    #[cfg(test)]
    pub(crate) fn finish_unsubmitted_once(&self) -> bool {
        if self.claim_unsubmitted_terminal() {
            self.finish_terminal_emission();
            true
        } else {
            false
        }
    }

    fn claim_terminal_with_admission(
        &self,
        app: &AppHandle,
        session_id: &str,
    ) -> Option<EmittedTerminal> {
        let _emission = self.emission.lock();
        let transition = self.claim_terminal_transition()?;
        if let Some(admission) = transition.admission {
            emit_turn_admission(app, session_id, admission);
        }
        Some(transition.terminal)
    }

    #[cfg(test)]
    pub(crate) fn finish_once(&self, emit: impl FnOnce()) -> Option<EmittedTerminal> {
        let emitted = self.claim_terminal()?;
        emit();
        self.finish_terminal_emission();
        Some(emitted)
    }

    fn finish_terminal_emission(&self) {
        self.state.lock().terminal_closing = false;
    }

    fn claim_reclaimed_with_admission(
        &self,
        app: &AppHandle,
        session_id: &str,
    ) -> Option<TerminalTransition> {
        let _emission = self.emission.lock();
        let transition = self.claim_reclaimed_transition()?;
        if let Some(admission) = transition.admission.clone() {
            emit_turn_admission(app, session_id, admission);
        }
        Some(transition)
    }

    pub(crate) fn invalidate_unsubmitted_reservation(&self) -> bool {
        let _emission = self.emission.lock();
        let mut state = self.state.lock();
        if !state.active || state.submitted || state.terminal_emitted {
            return false;
        }
        state.reclaimed = true;
        if let Some(reservation_id) = state.active_reservation_id.take() {
            Self::remove_unsubmitted_rule_locked(&mut state, reservation_id);
        }
        state.active = false;
        state.turn_id = None;
        drop(state);
        true
    }

    /// 原子认领一个「reserved 未 submitted」turn 的终态并设置 `terminal_closing`
    /// 闸门，供需要补发 `chat:done` 的路径（cancel 在 engine 未 spawn 时）使用。
    ///
    /// 与 [`invalidate_unsubmitted_reservation`] 的关键区别：本方法认领成功后立即
    /// 置 `terminal_emitted = true` 与 `terminal_closing = true`，使随后抵达的
    /// [`reserve`](Self::reserve) 被拒（`active || terminal_closing` → 闸门关闭），
    /// 直至调用方在发完 `chat:done` 后调用 [`finish_terminal_emission`] 重新打开
    /// 闸门。这样就消除了「invalidate 返回与 chat:done 发出之间，新一轮 reserve
    /// 成功，迟到的 chat:done 错误清除新一轮 busy 状态」的跨轮竞态——权威终态路径
    /// （[`claim_terminal_transition`] / [`claim_reclaimed_transition`]）同样靠这一对
    /// 字段防止重入。
    ///
    /// [`invalidate_unsubmitted_reservation`]: Self::invalidate_unsubmitted_reservation
    /// [`finish_terminal_emission`]: Self::finish_terminal_emission
    /// [`claim_terminal_transition`]: Self::claim_terminal_transition
    /// [`claim_reclaimed_transition`]: Self::claim_reclaimed_transition
    pub(crate) fn claim_unsubmitted_terminal(&self) -> bool {
        self.claim_unsubmitted_terminal_impl(None)
    }

    /// 带目标 epoch 的原子认领：在 lifecycle state 锁内**同时**校验
    /// `turn_epoch == target` 并认领，二者之间不存在可插入轮次切换的窗口。
    ///
    /// 供 cancel 阶段二使用（`reserve_turn` 不取 `turn_lock`，generation 检查
    /// 与认领若分离，另一 worker 可在检查通过后结束旧轮并 reserve 新轮，
    /// 无 epoch 校验的认领会把新轮 reservation 误认领为 Interrupted）：
    /// epoch 不匹配时返回 `false` 且不产生任何副作用，调用方必须整体 no-op。
    pub(crate) fn claim_unsubmitted_terminal_for_epoch(&self, target: u64) -> bool {
        self.claim_unsubmitted_terminal_impl(Some(target))
    }

    fn claim_unsubmitted_terminal_impl(&self, expected_epoch: Option<u64>) -> bool {
        let _emission = self.emission.lock();
        let mut state = self.state.lock();
        if expected_epoch.is_some_and(|epoch| state.turn_epoch != epoch) {
            // 轮次已切换（目标轮已结束、新轮已 reserve）：不得认领新轮。
            return false;
        }
        if !state.active || state.submitted || state.terminal_emitted {
            return false;
        }
        state.reclaimed = true;
        if let Some(reservation_id) = state.active_reservation_id.take() {
            Self::remove_unsubmitted_rule_locked(&mut state, reservation_id);
        }
        // 与权威终态路径一致：先发信号期间 `terminal_closing` 关闸，
        // `terminal_emitted` 保证本轮不会再被任何路径二次认领。
        state.active = false;
        state.submitted = false;
        state.turn_id = None;
        state.terminal_emitted = true;
        state.terminal_closing = true;
        drop(state);
        true
    }

    /// 认领一个未提交 turn 的终态并补发 `chat:done`，最后重置闸门。
    ///
    /// 给 cancel 在 engine 尚未 spawn（reservation 处于 reserved 未 submitted 阶段）
    /// 时使用：原子认领（关闸防重入）→ 发 `Interrupted` 终态 → 重开闸门。封装成一
    /// 个方法是为了让调用方（`EnginePool::cancel`，跨模块）不必直接触碰私有的
    /// 终态收尾逻辑，与权威终态路径一样自包含「发完即重置」。
    pub(crate) fn emit_unsubmitted_interrupted_terminal(
        &self,
        app: &AppHandle,
        session_id: &str,
    ) -> bool {
        if !self.claim_unsubmitted_terminal() {
            return false;
        }
        emit_chat_terminal(app, session_id, TurnOutcomeStatus::Interrupted, None, false);
        self.finish_terminal_emission();
        true
    }

    /// 带目标 epoch 的未提交认领 + 补发 `chat:done`：认领在 state 锁内与
    /// `turn_epoch == target` 校验原子完成，目标轮已结束（新轮已 reserve）时
    /// 返回 `false` 且无副作用，避免把新轮 reservation 误认领为 Interrupted。
    pub(crate) fn emit_unsubmitted_interrupted_terminal_for_epoch(
        &self,
        app: &AppHandle,
        session_id: &str,
        target: u64,
    ) -> bool {
        if !self.claim_unsubmitted_terminal_for_epoch(target) {
            return false;
        }
        emit_chat_terminal(app, session_id, TurnOutcomeStatus::Interrupted, None, false);
        self.finish_terminal_emission();
        true
    }

    /// 标记「cancel 在 turn 已 submit 但尚未 TurnStarted 时发起」。
    ///
    /// 仅当 turn 处于 active、已 `submitted`、且 `turn_id` 仍为 None（TurnStarted
    /// 未抵达）、且 `turn_epoch == epoch`（仍是发起 cancel 的那一轮）时设置
    /// `pending_cancel = Some(epoch)`：
    /// - 必须 `submitted`：未提交的 reservation（消息尚未入队 engine）应由 cancel
    ///   走未提交认领终态路径（`emit_unsubmitted_interrupted_terminal`）立即发
    ///   `chat:done` 使 reservation 失效，而不是挂成 pending——否则空闲 engine 仍
    ///   存在时 cancel 不发终态、reservation 仍有效，原 chat future 后续照常提交，
    ///   前端 busy 在 cancel 后到 TurnStarted 之间无法复位。
    /// - 若 `turn_id` 已有值说明 TurnStarted 已被转发器消费，`cancel_current()`
    ///   直接命中当前活跃 token，无需补打。
    /// - 必须 `turn_epoch == epoch`：并发取消请求（C1/C2）中，排队较晚的 C2 在
    ///   持锁恢复后读到的是「当前 lifecycle」。cancel 已在取 `turn_lock` 前后比对
    ///   过 epoch（见 [`current_turn_generation`]/cancel 路径），此处传入**当前**
    ///   epoch 作二次锚定，确保 pending 只 arm 到目标轮，不跨轮泄漏到新轮的
    ///   TurnStarted（forwarder 经 [`take_pending_cancel`] 校验 epoch 后才重放）。
    ///
    /// **调用顺序**：必须在 `cancel_current()` **之前**调用。两者取不同的锁
    /// （lifecycle state mutex vs cancel_token mutex），无法原子合并。先 arm
    /// 再 cancel 保证：即使 TurnStarted 在两步之间抵达转发器并消费了标记，
    /// 随后的 `cancel_current()` 也只是幂等 no-op（转发器已重新 cancel）。
    ///
    /// **返回值**：`false` 表示 `state.turn_epoch != epoch`——generation 复查
    /// 通过之后、arm 之前另一 worker 已结束目标轮并启动新轮（`reserve_turn`
    /// 不取 `turn_lock`，可在 cancel 的同步段中间完成切换）。调用方**必须**
    /// 在收到 `false` 时跳过 `cancel_current` 及其级联副作用，否则取消会命中
    /// 新轮已 `reset_cancel_token` 的活跃 token，造成跨轮误取消。
    ///
    /// 其余条件不满足（未 submitted / 已 started / 非 active）时仍返回 `true`：
    /// 这些情况 epoch 匹配、仍是目标轮，`cancel_current` 命中目标轮 token
    /// （已 started 直接命中活跃 token；未 submitted 时 engine 尚无该轮 token，
    /// 取消是幂等 no-op），调用方可以安全继续。
    ///
    /// [`current_turn_generation`]: Self::current_turn_generation
    /// [`take_pending_cancel`]: Self::take_pending_cancel
    ///
    /// 在 lifecycle state 锁内原子完成「epoch 校验 + arm pending + 同步取消」。
    ///
    /// 与 [`arm_pending_cancel`] 的区别：`cancel` 闭包在**同一临界区内**持锁
    /// 执行。单独 arm 时锁在校验后即释放，调用方随后才执行 `cancel_current`，
    /// 两条同步调用之间没有 `.await` 也不构成原子性保证——多线程 runtime/OS
    /// 可以在任意指令边界切换线程。若另一 worker 在该窗口内完成「旧轮终态
    /// 收口 + 新轮 reserve/send + `reset_cancel_token`」，恢复后的旧 cancel 会
    /// 命中新轮活跃 token（reviewer 点 8）。把取消闭包移入同一临界区后，
    /// `reserve_turn`（取同一把 state 锁）无法插入「校验/arm」与「取消」之间，
    /// 跨轮窗口闭合。
    ///
    /// **idle 守卫**（G2/epoch-0 哨兵）：`turn_epoch` 从 1 起自增，但 fresh
    /// 会话（从未有过 turn）初始为 0。此时发起 cancel 快照 `target = None`，
    /// 调用方以 `target.unwrap_or(0)` 传入后，`turn_epoch != epoch` 校验（0 != 0
    /// 不成立）会通过——若 engine 已为「即将启动的定时轮」在场（`run_scheduled_turn`
    /// 的 spawn→submit 窗口），执行 `cancel_current` 会命中该新 token，取消信号
    /// 与 `reset_cancel_token` 竞速、结果不确定（定时轮被意外取消或停止丢失）。
    /// 因此 epoch 匹配后仍需 `state.active || state.terminal_closing` 才执行
    /// `cancel` 闭包：空闲（无活跃轮）时跳过取消（幂等 no-op），但返回 `true`
    /// 让调用方继续（级联取消仍由调用方按 armed 路径执行，只取消 engine 遗留
    /// 子代理、不取消尚未启动的轮）。
    ///
    /// 锁序：本方法持 lifecycle state 锁调用 `cancel` 闭包，闭包内
    /// `engine.cancel_current()` 取 engine 的 cancel_token 锁（与 state 锁无
    /// 反向依赖，forwarder 消费 pending 也是先 state 后 token），无死锁。
    /// `cancel` 必须同步、不 panic（panic 会使 Mutex 中毒），且不得再次获取
    /// 本 lifecycle 的 state 锁。
    ///
    /// 其余语义与 [`arm_pending_cancel`] 一致：epoch 匹配则设置 pending（条件
    /// 满足时）并执行 `cancel` 后返回 `true`；epoch 不匹配返回 `false` 且
    /// **不执行** `cancel`，调用方必须整体 no-op。
    ///
    /// [`arm_pending_cancel`]: Self::arm_pending_cancel_and_cancel
    pub(crate) fn arm_pending_cancel_and_cancel<F>(&self, epoch: u64, cancel: F) -> bool
    where
        F: FnOnce(),
    {
        let mut state = self.state.lock();
        if state.turn_epoch != epoch {
            // 轮次已切换（目标轮已结束、新轮已 reserve）：不 arm、不取消。
            return false;
        }
        if !state.active && !state.terminal_closing {
            // idle（无活跃轮、非终态收口期）：target 快照为 None 时以 0 哨兵
            // 传入，epoch 校验通过但并无可取消的轮。执行 cancel 会命中「即将
            // 启动的定时轮」的新 token（G2）。跳过取消闭包、不 arm（idle 也
            // 不满足 submitted 前置），但返回 true：调用方按 armed 路径继续，
            // 级联取消只清 engine 遗留子代理，不误伤尚未启动的轮。
            return true;
        }
        if state.submitted && state.turn_id.is_none() && state.active {
            state.pending_cancel = Some(epoch);
        }
        // 持锁执行同步取消：reserve_turn 需要同一把 state 锁，无法在
        // 「校验/arm」与「取消」之间插入轮次切换，旧 cancel 不可能命中新轮。
        cancel();
        true
    }

    /// 原子取出并清除 `pending_cancel` 标记。
    ///
    /// 由事件转发器在收到 `TurnStarted` 后调用：此时 CodeWhale 的
    /// `reset_cancel_token()` 已执行完毕（它在 `TurnStarted` 之前），
    /// 重新 `cancel_current()` 命中的正是本轮的活跃 token。
    ///
    /// 仅当记录的 epoch 仍是 `current_epoch`（仍是 arming 时的那一轮）时才取出
    /// 并返回 `Some`，否则清空并返回 `None`：跨轮泄漏的 stale pending（cancel
    /// arm 到旧轮后，新一轮 TurnStarted 先抵达）被丢弃，不误取消新轮。空闲时
    /// `current_epoch` 由调用方传 0（epoch 自增从 1 起，恒不匹配）。
    pub(crate) fn take_pending_cancel(&self, current_epoch: u64) -> Option<u64> {
        let mut state = self.state.lock();
        match state.pending_cancel {
            Some(epoch) if epoch == current_epoch => state.pending_cancel.take(),
            _ => {
                state.pending_cancel = None;
                None
            }
        }
    }
}

async fn finish_reclaimed_lifecycle_turn(
    lifecycle: &TurnLifecycle,
    app: &AppHandle,
    store: &SessionStore,
    session_id: &str,
    status: TurnOutcomeStatus,
    error: Option<String>,
    shell_cleanup_failed: bool,
) -> Option<EmittedTerminal> {
    let transition = lifecycle.claim_reclaimed_with_admission(app, session_id)?;
    let mut terminal_status = status;
    let mut terminal_error = error;
    if let Some(fallback) = transition.fallback {
        let store_for_save = store.clone();
        let session_for_save = session_id.to_string();
        let saved = tokio::task::spawn_blocking(move || {
            store_for_save.persist_admitted_chat_display(
                &session_for_save,
                &fallback.baseline_revision,
                fallback.display_message,
                fallback.operation == TranscriptOperation::EditLast,
            )
        })
        .await;
        match saved {
            Ok(Ok(saved)) => match transcript_revision(&saved.messages) {
                Ok(revision) => {
                    let payload = json!({
                        "session_id": session_id,
                        "transcript_revision": revision,
                    });
                    let _ = app.emit("chat:transcript_committed", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        app,
                        "chat:transcript_committed",
                        payload,
                    );
                }
                Err(revision_error) => {
                    terminal_status = TurnOutcomeStatus::Failed;
                    terminal_error = Some(format!(
                        "Reclaimed transcript revision failed: {revision_error:#}"
                    ));
                }
            },
            Ok(Err(save_error)) => {
                terminal_status = TurnOutcomeStatus::Failed;
                terminal_error = Some(format!(
                    "Reclaimed transcript persistence failed: {save_error:#}"
                ));
            }
            Err(join_error) => {
                terminal_status = TurnOutcomeStatus::Failed;
                terminal_error = Some(format!(
                    "Reclaimed transcript persistence task failed: {join_error}"
                ));
            }
        }
    }
    // 时间线必须先于 chat:done 落盘。前端收到终态后会重新读取权威时间线；
    // 若先 emit 再写文件，后台/恢复会话会读到旧快照并漏掉完成状态与耗时。
    let status_text = format!("{terminal_status:?}");
    crate::features::assistant::timing::finish_turn(
        session_id,
        &status_text,
        terminal_error.as_deref(),
    );
    emit_chat_terminal(
        app,
        session_id,
        terminal_status,
        terminal_error,
        shell_cleanup_failed,
    );
    lifecycle.finish_terminal_emission();
    Some(transition.terminal)
}

impl AppEngine {
    /// 为指定 session spawn 一个**独立** engine:绑定该 session 专属的 workspace +
    /// instructions(spawn 时由 [`build_engine_config_for_session`] 固化进 config,
    /// 不再靠 `Op::SyncSession` 动态切),并启一个带 `session_id` 的 event forwarder。
    /// 返回 `(engine, forwarder_handle)`,[`EnginePool`] 回收 session 时 abort forwarder。
    ///
    /// 调用方(EnginePool)负责复用同一份已 boot 的 `bridge`,避免每个 session 重 boot
    /// (boot 会写盘 / 设 env)。
    ///
    /// [`build_engine_config_for_session`]: crate::features::assistant::platform::bridge::Pinvou3Bridge::build_engine_config_for_session
    /// [`EnginePool`]: crate::features::assistant::engine_pool::EnginePool
    pub(crate) async fn spawn_for_session(
        app: AppHandle,
        store: SessionStore,
        bridge: Pinvou3Bridge,
        session_id: &str,
        extra_tools: Vec<std::sync::Arc<dyn deepseek_tui::tools::spec::ToolSpec>>,
        disallowed: Vec<String>,
        turn_lifecycle: Arc<TurnLifecycle>,
        shell_manager: SharedShellManager,
        turn_shell_tasks: TurnShellTaskRegistry,
    ) -> Result<(Self, tauri::async_runtime::JoinHandle<()>)> {
        // Instructions 走 Inline，不写入远端工作区。所有会话共享产品工具面；
        // 子智能体仍由 CodeWhale 的通用角色与运行时策略进一步收窄。
        let scheduled_profile = store.scheduled_profile(session_id);
        let scheduled_base_total_tokens = if scheduled_profile.is_some() {
            Some(store.load(session_id)?.metadata.total_tokens)
        } else {
            None
        };
        // 多智能体开关（ADR-0006）：会话开着开关时装配专家名册。
        let multi_agent_enabled = scheduled_profile.is_none()
            && bridge.multi_agent_mode_available(session_id)
            && store.mode_state(session_id).multi_agent;
        let roots = if scheduled_profile.is_some() {
            // 定时任务保留既有 profile/fallback 解析；其 ledger 可能是用户选择的
            // automation workspace，下面不会对它执行任何旧投影删除。
            store
                .session_roots(session_id)
                .unwrap_or_else(|_| bridge.session_roots(session_id))
        } else {
            // 普通会话的 roots 是 destructive migration 的权限边界：必须由
            // SessionStore 校验 session id 并成功解析，禁止用相同的字符串 join
            // fallback 掩盖非法 id 后再进入清理。
            store
                .session_roots(session_id)
                .with_context(|| format!("resolve managed session roots for {session_id}"))?
        };
        if scheduled_profile.is_none() {
            // 仅对可证明由 Pinvou 管理的 session ledger 做旧投影迁移。定时任务
            // 的 execution/ledger 可以是用户选择的自动化工作区，绝不能按文件名
            // 删除其中的项目 profile。
            let managed_ledger = crate::platform::paths::session_workspace_dir(session_id);
            if roots.ledger != managed_ledger {
                anyhow::bail!(
                    "refusing legacy expert cleanup outside managed session ledger: {}",
                    roots.ledger.display()
                );
            }
            cleanup_legacy_expert_projection(
                &managed_ledger,
                &crate::platform::paths::sessions_root(),
            )
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("clean legacy expert projection for {session_id}"))?;
        }
        // 启动配置的 fleet_roster 与传给 spawn_engine 的 DtConfig 必须来自同一
        // 次专家池读取；否则专家恰好在两次构造之间变更时，首轮就会出现名册
        // 与 spawn-time route 不一致。
        let expert_snapshot = multi_agent_enabled.then(ExpertRosterSnapshot::capture);
        let mut engine_config = if multi_agent_enabled {
            // 多智能体面：装配专家名册和专用资源上限；工具面仍与普通会话
            // 完全一致，普通会话不继承这些限制。
            bridge.build_engine_config_for_multi_agent(
                session_id,
                roots,
                expert_snapshot.as_deref().expect("multi-agent snapshot"),
            )
        } else {
            bridge.build_engine_config_for_session_roots(session_id, roots)
        };
        engine_config.runtime_services.shell_manager = Some(shell_manager.clone());
        // Agentic RAG:给该 session 的 engine 注入 kb_search + kb_open_source(都持
        // session_id,execute 时只访问该会话挂载的知识集)。工具常驻所有会话,挂没挂集由
        // 运行时判断。
        engine_config.extra_tools.0.extend(extra_tools);
        // 工具门控:连接器开关禁用 +(知识库为空时)隐藏 kb_search/kb_open_source。compute 返回
        // **完整**列表(已含连接器禁用),直接覆盖 build_engine_config 设的「连接器-only」初值,
        // 让新会话天生正确——空知识库就看不到知识工具,不会宣称能本地检索。
        //
        // 该列表来自 compute_disallowed_tools；多智能体会话不改写它，
        // 工具面与普通会话保持一致。
        let mut scheduled_disallowed_tools = disallowed.clone();
        // One automation run owns exactly one engine turn. Goal tools can
        // enqueue autonomous continuation turns after TurnComplete. Apply this
        // list only to the unattended turn; a later interactive continuation
        // gets a fresh engine with the ordinary catalog.
        for tool in ["create_goal", "update_goal"] {
            if !scheduled_disallowed_tools
                .iter()
                .any(|blocked| blocked == tool)
            {
                scheduled_disallowed_tools.push(tool.to_string());
            }
        }
        engine_config.disallowed_tools = if disallowed.is_empty() {
            None
        } else {
            Some(disallowed)
        };
        let dt_config = match expert_snapshot.as_deref() {
            Some(snapshot) => bridge.build_multi_agent_dt_config(snapshot),
            None => bridge.build_dt_config(),
        };
        eprintln!(
            "[pinvou3-app] spawn_engine session={} model={} workspace={} instructions={}",
            session_id,
            engine_config.model,
            engine_config.workspace.display(),
            format_instructions(&engine_config.instructions),
        );

        let workspace = engine_config.workspace.clone();
        let handle = spawn_engine(engine_config, &dt_config);
        let (turn_events, _) = broadcast::channel(32);
        let scheduled_unattended = Arc::new(AtomicBool::new(false));
        let forwarder = spawn_event_forwarder(
            app,
            handle.clone(),
            store,
            bridge.clone(),
            workspace.clone(),
            session_id.to_string(),
            turn_events.clone(),
            scheduled_profile,
            scheduled_base_total_tokens,
            scheduled_unattended.clone(),
            turn_lifecycle.clone(),
            shell_manager,
            turn_shell_tasks.clone(),
        );

        Ok((
            Self {
                handle,
                bridge,
                session_id: session_id.to_string(),
                workspace,
                multi_agent_enabled,
                turn_events,
                scheduled_unattended,
                turn_lifecycle,
                turn_shell_tasks: Some(turn_shell_tasks),
                scheduled_disallowed_tools,
            },
            forwarder,
        ))
    }

    /// 测试入口(L1 harness 用):用预先 boot 好的 bridge spawn 一个 engine,
    /// **不启 Tauri event forwarder** (不需要 AppHandle / SessionStore),
    /// 调用方自己消费 `engine.handle.rx_event` 拿到 ToolCallStarted /
    /// ToolCallComplete / TurnComplete 做断言。
    ///
    /// 不复用 [`spawn`] 是因为它强依赖 Tauri AppHandle (`spawn_event_forwarder`
    /// 里 `app.emit(...)`),测试场景没有 webview/event 系统跑不起来。
    #[allow(dead_code)] // L1 runner 接入前临时 unused
    pub async fn spawn_headless(bridge: Pinvou3Bridge) -> Result<Self> {
        let mut engine_config = bridge.build_engine_config();
        let scheduled_disallowed_tools = engine_config.disallowed_tools.clone().unwrap_or_default();
        let workspace = engine_config.workspace.clone();
        engine_config.runtime_services.shell_manager =
            Some(new_shared_shell_manager(workspace.clone()));
        let dt_config = bridge.build_dt_config();
        let handle = spawn_engine(engine_config, &dt_config);
        let (turn_events, _) = broadcast::channel(1);
        Ok(Self {
            handle,
            bridge,
            session_id: String::new(),
            workspace,
            multi_agent_enabled: false,
            turn_events,
            scheduled_unattended: Arc::new(AtomicBool::new(false)),
            turn_lifecycle: Arc::new(TurnLifecycle::default()),
            turn_shell_tasks: None,
            scheduled_disallowed_tools,
        })
    }

    /// 发用户消息给 Engine。Engine 内部自管 session，多轮自然累积。
    ///
    /// `mode` + `phase` 由 commands::chat 从 SessionStore 取当前 session 的
    /// mode_state，注入 Op::SendMessage。底座按 mode 自动切工具白名单 + sandbox。
    /// M1 弱模型加固:bridge 按 phase 在 user content 前 prepend `<system-reminder>`。
    pub async fn send_user_message(
        &self,
        content: String,
        mode: AppMode,
        persona_reminder: Option<String>,
        restrict_tools: bool,
    ) -> Result<()> {
        let expert_snapshot = self.multi_agent_enabled.then(ExpertRosterSnapshot::capture);
        let op = self.build_interactive_send_message_op(
            content,
            mode,
            persona_reminder,
            restrict_tools,
            expert_snapshot,
        )?;
        self.send_turn_op(op).await
    }

    pub(crate) async fn send_reserved_user_message(
        &self,
        content: String,
        mode: AppMode,
        persona_reminder: Option<String>,
        restrict_tools: bool,
        expert_snapshot: Option<std::sync::Arc<ExpertRosterSnapshot>>,
        reservation: TurnReservation,
    ) -> Result<()> {
        let op = self.build_interactive_send_message_op(
            content,
            mode,
            persona_reminder,
            restrict_tools,
            expert_snapshot,
        )?;
        self.send_reserved_turn_op(op, reservation).await
    }

    fn build_interactive_send_message_op(
        &self,
        content: String,
        mode: AppMode,
        persona_reminder: Option<String>,
        restrict_tools: bool,
        expert_snapshot: Option<std::sync::Arc<ExpertRosterSnapshot>>,
    ) -> Result<Op> {
        if self.multi_agent_enabled {
            let snapshot = expert_snapshot
                .as_deref()
                .context("multi-agent turn is missing its expert roster snapshot")?;
            self.bridge.build_multi_agent_send_message_op(
                &self.session_id,
                content,
                mode,
                persona_reminder,
                restrict_tools,
                &self.workspace,
                snapshot,
            )
        } else {
            if expert_snapshot.is_some() {
                anyhow::bail!("ordinary turn must not carry a multi-agent expert snapshot");
            }
            self.bridge.build_send_message_op(
                &self.session_id,
                content,
                mode,
                persona_reminder,
                restrict_tools,
            )
        }
    }

    /// Submit the initial prompt for one scheduled run using the immutable
    /// policy captured when its hidden session was created.
    pub(crate) async fn send_scheduled_message(
        &self,
        content: String,
        profile: &ScheduledRunProfile,
    ) -> Result<()> {
        let op = self.profiled_send_op(content, profile)?;
        self.handle
            .send(Op::SetDisallowedTools {
                tools: self.scheduled_disallowed_tools.clone(),
            })
            .await?;
        self.send_turn_op(op).await
    }

    fn profiled_send_op(&self, content: String, profile: &ScheduledRunProfile) -> Result<Op> {
        let mut op = self.bridge.build_send_message_op(
            &self.session_id,
            content,
            profile.execution_mode().to_app_mode(),
            Some(SCHEDULED_TURN_REMINDER.to_string()),
            false,
        )?;
        let route = self
            .bridge
            .resolve_runtime_route_for_model(&profile.model)?;
        let compaction = self.bridge.compaction_config_for_model(&profile.model);
        apply_scheduled_turn_policy(&mut op, profile, route, compaction)?;
        Ok(op)
    }

    pub(crate) fn subscribe_turns(&self) -> broadcast::Receiver<EngineTurnSignal> {
        self.turn_events.subscribe()
    }

    /// 取消当前正在生成的回复（点⏹️停止按钮）。
    /// 同步触发 cancel_token，engine turn loop 会立即跳出并发 TurnComplete 事件。
    pub fn cancel_current(&self) {
        self.handle.cancel();
    }

    async fn send_turn_op(&self, op: Op) -> Result<()> {
        let activated = self.turn_lifecycle.on_submitted();
        if !activated {
            anyhow::bail!("session_turn_in_progress");
        }
        let shell_scope = match self.turn_shell_tasks.as_ref() {
            Some(registry) => match registry.prepare_submission().await {
                Ok(scope) => Some(scope),
                Err(error) => {
                    self.turn_lifecycle.on_submission_failed(activated);
                    return Err(error).context("prepare root-turn shell scope");
                }
            },
            None => None,
        };
        match self.handle.send(op).await {
            Ok(()) => {
                if let Some(scope) = shell_scope {
                    scope.commit();
                }
                Ok(())
            }
            Err(error) => {
                self.turn_lifecycle.on_submission_failed(activated);
                Err(error)
            }
        }
    }

    async fn send_reserved_turn_op(&self, op: Op, reservation: TurnReservation) -> Result<()> {
        if !reservation.belongs_to(&self.turn_lifecycle) {
            anyhow::bail!("turn reservation belongs to a different session");
        }
        reservation.ensure_active()?;
        let actual_user_content = match &op {
            Op::SendMessage { content, .. } => content.clone(),
            Op::EditLastTurn { new_message } => new_message.clone(),
            _ => anyhow::bail!("reserved turn requires a user-message operation"),
        };
        reservation.prepare_actual_user_content(actual_user_content)?;
        let shell_scope = match self.turn_shell_tasks.as_ref() {
            Some(registry) => Some(
                registry
                    .prepare_submission()
                    .await
                    .context("prepare reserved root-turn shell scope")?,
            ),
            None => None,
        };
        match self.handle.send(op).await {
            Ok(()) => {
                if let Some(scope) = shell_scope {
                    scope.commit();
                }
                reservation.mark_submitted();
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn finish_reclaimed_turn(
        &self,
        app: &AppHandle,
        store: &SessionStore,
        session_id: &str,
        shell_cleanup_failed: bool,
    ) -> bool {
        match finish_reclaimed_lifecycle_turn(
            &self.turn_lifecycle,
            app,
            store,
            session_id,
            TurnOutcomeStatus::Interrupted,
            None,
            shell_cleanup_failed,
        )
        .await
        {
            Some(EmittedTerminal {
                turn_id: Some(turn_id),
            }) => {
                let _ = self.turn_events.send(EngineTurnSignal::Terminal {
                    turn_id,
                    status: TurnOutcomeStatus::Interrupted,
                    error: None,
                });
                true
            }
            Some(EmittedTerminal { turn_id: None }) => true,
            None => false,
        }
    }

    /// 编辑/重发最后一轮 user 消息（点 ✏️ 编辑或 🔄 重发按钮）。
    /// 上游 [`Op::EditLastTurn`] 行为：砍掉 session 末尾最近的 user 消息及之后
    /// 所有消息，然后用 `new_message` 当成新 user 消息重新发送。
    pub async fn edit_last_turn(&self, new_message: String) -> Result<()> {
        self.send_turn_op(Op::EditLastTurn { new_message }).await
    }

    pub(crate) async fn edit_last_turn_reserved(
        &self,
        new_message: String,
        reservation: TurnReservation,
    ) -> Result<()> {
        self.send_reserved_turn_op(Op::EditLastTurn { new_message }, reservation)
            .await
    }

    /// 手动触发上下文压缩（用户点 token 进度条 → 立即压缩）。
    /// 自动压缩由上游 CompactionConfig.enabled 控制（pinvou3 走默认 = on）。
    pub async fn compact_now(&self) -> Result<()> {
        let model = self.bridge.model();
        let route = if self.multi_agent_enabled {
            let snapshot = ExpertRosterSnapshot::capture();
            self.bridge
                .resolve_multi_agent_runtime_route_for_model(&model, &snapshot)?
        } else {
            self.bridge.resolve_runtime_route_for_model(&model)?
        };
        self.handle
            .send(Op::CompactContext {
                route: Box::new(route),
                compaction: Box::new(self.bridge.compaction_config_for_model(&model)),
            })
            .await?;
        Ok(())
    }

    /// 提交 request_user_input 工具的用户选择（前端选择气泡点击后调用）。
    /// 底座 `EngineHandle::submit_user_input` 把答案放回 rx_user_input channel,
    /// engine 的 await_user_input loop 收到后把 UserInputResponse 转成 ToolResult。
    pub async fn submit_user_input(
        &self,
        tool_call_id: String,
        response: UserInputResponse,
    ) -> Result<()> {
        self.handle
            .submit_user_input(tool_call_id, response)
            .await?;
        Ok(())
    }

    /// 取消 request_user_input(前端 ✕ 按钮或对话切换时调用)。
    pub async fn cancel_user_input(&self, tool_call_id: String) -> Result<()> {
        self.handle.cancel_user_input(tool_call_id).await?;
        Ok(())
    }

    /// 切换 engine 内部 session 状态：替换 messages + 切到 session-specific
    /// workspace。
    ///
    /// C 方案(P-no-disk): 不再传 `system_prompt` — `EngineConfig.instructions`
    /// 是内存 inline,底座 refresh_system_prompt 自动从中重拼 + 完整替换
    /// `{{PINVOU3_WORKSPACE}}` 占位符。原先 sync 时重写 disk + 传 SystemPrompt::Text
    /// 都是 disk-API-限制的副作用,现在彻底走掉。
    pub async fn sync_session(&self, session_id: String, messages: Vec<Message>) -> Result<()> {
        self.handle
            .send(Op::SyncSession {
                session_id: Some(session_id),
                messages,
                system_prompt: None,
                system_prompt_override: false,
                model: self.bridge.model(),
                workspace: self.workspace.clone(),
                mode: AppMode::Yolo,
            })
            .await?;
        Ok(())
    }
}

fn format_instructions(sources: &[deepseek_tui::prompts::InstructionSource]) -> String {
    use deepseek_tui::prompts::InstructionSource;
    if sources.is_empty() {
        "none".to_string()
    } else {
        sources
            .iter()
            .map(|s| match s {
                InstructionSource::File(p) => p.display().to_string(),
                InstructionSource::Inline { name, .. } => format!("inline:{name}"),
            })
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Per-turn 状态：跟踪本 turn 是否调过 plan 类工具 + 最后一次 snapshot。
/// 底座两层 plan 结构：
///   - `update_plan`         → strategy 层（`plan_snapshot`）
///   - `checklist_write` / `todo_write` → leaf task 层（`todos_snapshot`）
///
/// 任一调过 + plan_phase=Planning → TurnComplete 时 emit `chat:plan_ready`。
/// 参考底座 `tui/ui.rs:1072-1085` 的 `plan_tool_used_in_turn` 判据 +
/// `prompts/modes/plan.md` "Use update_plan ... and checklist_write ..." 双工具引导。
#[derive(Default)]
struct TurnPlanTracker {
    plan_tool_used: bool,
    /// `update_plan` 最近一次结果 JSON：`{ explanation, items: [{step, status}] }`
    /// 上游 `UpdatePlanTool::execute` 返回 "Plan updated: ...\n<json>"，截 \n 后 parse。
    last_plan_snapshot: Option<serde_json::Value>,
    /// `todo_write` / `checklist_write` 最近一次结果 JSON：
    /// `{ items: [{id, content, status}], completion_pct, in_progress_id }`
    /// 上游 `TodoWriteTool::execute` 返回 "Todo list updated (...)\n<json>"。
    last_todos_snapshot: Option<serde_json::Value>,
}

fn tool_call_result_parts(
    result: std::result::Result<
        deepseek_tui::tools::spec::ToolResult,
        deepseek_tui::tools::spec::ToolError,
    >,
) -> (String, bool, Option<serde_json::Value>) {
    match result {
        Ok(result) => (result.content, result.success, result.metadata),
        Err(error) => (format!("{error:#}"), false, None),
    }
}

#[path = "forwarder.rs"]
mod forwarder;
pub(crate) use forwarder::spawn_event_forwarder;

/// 让 main.rs 编译时知道这个模块（供 docs/CI 用）。
pub fn _force_link() -> Arc<()> {
    Arc::new(())
}

#[cfg(test)]
mod tool_result_projection_tests {
    use super::tool_call_result_parts;

    #[test]
    fn unsuccessful_tool_result_is_not_promoted_to_success() {
        let (output, success, metadata) = tool_call_result_parts(Ok(
            deepseek_tui::tools::spec::ToolResult::error("interrupted"),
        ));
        assert_eq!(output, "interrupted");
        assert!(!success, "chat:tool_end must preserve ToolResult.success");
        assert!(metadata.is_none());
    }
}

#[cfg(test)]
mod turn_lifecycle_tests {
    use super::{EmittedTerminal, TranscriptOperation, TurnAdmissionMetadata, TurnLifecycle};
    use crate::features::sessions::SessionModeState;
    use deepseek_tui::models::{ContentBlock, Message};
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn message(role: &str, text: &str) -> Message {
        Message {
            role: role.to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    fn engine_user(text: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: vec![
                ContentBlock::Text {
                    text: text.to_string(),
                    cache_control: None,
                },
                ContentBlock::Text {
                    text: "<turn_meta>private</turn_meta>".to_string(),
                    cache_control: None,
                },
            ],
        }
    }

    #[test]
    fn reclaim_and_natural_completion_emit_exactly_once_per_turn() {
        let lifecycle = TurnLifecycle::default();
        let emitted = Cell::new(0_u8);

        assert_eq!(lifecycle.finish_once(|| emitted.set(99)), None);
        lifecycle.on_submitted();
        lifecycle.on_started("turn-1".to_string());
        assert_eq!(
            lifecycle.finish_once(|| emitted.set(emitted.get() + 1)),
            Some(EmittedTerminal {
                turn_id: Some("turn-1".to_string())
            })
        );
        assert_eq!(lifecycle.finish_once(|| emitted.set(99)), None);
        assert_eq!(emitted.get(), 1);

        lifecycle.on_submitted();
        lifecycle.on_started("turn-2".to_string());
        assert!(lifecycle
            .finish_once(|| emitted.set(emitted.get() + 1))
            .is_some());
        assert_eq!(emitted.get(), 2);
    }

    #[test]
    fn failed_submission_and_idle_cancel_do_not_fake_a_terminal() {
        let lifecycle = TurnLifecycle::default();
        let activated = lifecycle.on_submitted();
        lifecycle.on_submission_failed(activated);
        assert_eq!(lifecycle.finish_once(|| panic!("must remain idle")), None);
    }

    #[test]
    fn invalidate_unsubmitted_reservation_returns_true_only_for_reserved_unsubmitted() {
        // 修复 2 的核心依赖：cancel 在 engine 未 spawn（reservation 仍处于
        // reserved 未 submitted 阶段）时调用 invalidate，必须返回 true 让调用方
        // 据此补发 chat:done 终态（否则前端 busy 永不复位）。
        let lifecycle = Arc::new(TurnLifecycle::default());

        // reserve → active=true, submitted=false → invalidate 返回 true。
        // 必须把 reservation 绑定到变量并保持存活到 invalidate 之后，否则 Drop
        // 会先触发 on_reservation_failed 把 active 复位。
        let reservation = lifecycle.reserve().expect("reserve");
        assert!(lifecycle.invalidate_unsubmitted_reservation());
        // invalidate 已收尾，标记 reservation submitted 以免 Drop 二次清理。
        reservation.mark_submitted();

        // reserve → on_started_transition（引擎真正接手，设 submitted=true）
        // → invalidate 必须返回 false：engine 已在跑，cancel 应走 cancel_current
        // 路径（TurnComplete 终态），不能再由 invalidate 补发，否则会双发 chat:done。
        let reservation2 = lifecycle.reserve().expect("reserve again");
        assert!(lifecycle
            .on_started_transition("turn-submitted".to_string())
            .is_some());
        assert!(!lifecycle.invalidate_unsubmitted_reservation());
        reservation2.mark_submitted();
    }

    #[test]
    fn claim_unsubmitted_terminal_blocks_reserve_until_emission_finishes() {
        // 跨轮竞态回归（见 claim_unsubmitted_terminal 的文档注释）：cancel 在
        // engine 未 spawn 时补发 chat:done，必须在「认领 → 发终态」之间关闸，
        // 否则新一轮 reserve 可抢先成功，迟到的 chat:done 会清掉新一轮 busy。
        // 这里直接断言闸门语义：claim 成功后、finish 前 reserve 必须被拒；
        // finish 后（终态已发完）才允许下一轮 reserve。
        let lifecycle = Arc::new(TurnLifecycle::default());

        let reservation = lifecycle.reserve().expect("reserve");
        // claim 成功 = 进入「终态发送中」临界区。
        assert!(lifecycle.claim_unsubmitted_terminal());
        reservation.mark_submitted();

        // 关键不变量：终态尚未发完（terminal_closing=true）时，下一轮 reserve
        // 必须失败——否则上一轮迟到的 chat:done 会污染新一轮 busy 状态。
        assert!(
            lifecycle.reserve().is_err(),
            "reserve must be rejected while terminal emission is in flight"
        );

        // 终态发完，闸门重开，下一轮可正常 reserve。
        lifecycle.finish_terminal_emission();
        assert!(lifecycle.reserve().is_ok());

        // 已 submitted 的 turn 不能再被 claim（engine 已接手，走 cancel_current 路径）。
        let reservation2 = lifecycle.reserve().expect("reserve");
        assert!(lifecycle
            .on_started_transition("turn-submitted".to_string())
            .is_some());
        assert!(!lifecycle.claim_unsubmitted_terminal());
        reservation2.mark_submitted();
    }

    #[test]
    fn arm_pending_cancel_requires_submitted_reservation() {
        // 空闲 engine + 未提交 reservation 窗口的回归：会话保留上一轮的空闲
        // engine 时，cancel 第二阶段若按「engine 是否存在」分流会错误走 pending
        // 分支。arm_pending_cancel 必须显式要求 submitted——只有消息确实已入队
        // engine、需要防 reset_cancel_token 覆盖时才 arm；未提交 reservation 应由
        // cancel 走未提交认领终态路径（emit_unsubmitted_interrupted_terminal）立即
        // 发 chat:done 使其失效，而不是挂成 pending。
        let lifecycle = Arc::new(TurnLifecycle::default());

        // reserve 后 active=true 但 submitted=false → arm 不得置位。
        let reservation = lifecycle.reserve().expect("reserve");
        let epoch = lifecycle.current_turn_generation().expect("active epoch");
        lifecycle.arm_pending_cancel_and_cancel(epoch, || {});
        assert!(
            lifecycle.take_pending_cancel(epoch).is_none(),
            "must not arm pending_cancel for an unsubmitted reservation"
        );

        // 走真实 send 路径：handle.send 成功后 mark_submitted → submitted=true。
        lifecycle.mark_reservation_submitted(reservation.reservation_id);
        // 已 submitted + active + turn_id=None（TurnStarted 未抵达）→ arm 置位。
        lifecycle.arm_pending_cancel_and_cancel(epoch, || {});
        assert!(
            lifecycle.take_pending_cancel(epoch).is_some(),
            "pending_cancel must be armed after submission, before TurnStarted"
        );
        // 消费 reservation 避免 Drop 副作用。
        reservation.mark_submitted();
    }

    #[test]
    fn claim_unsubmitted_terminal_invalidates_reservation() {
        // 修复的核心闭环：空闲 engine 存在时，cancel 对未提交 reservation 走认领
        // 终态路径，认领后该 reservation 必须失效——后续 send 的 ensure_active
        // 失败、消息不再提交给 engine，前端 busy 因补发的 chat:done 复位。
        let lifecycle = Arc::new(TurnLifecycle::default());
        let reservation = lifecycle.reserve().expect("reserve");

        // 认领前 reservation 有效。
        assert!(reservation.ensure_active().is_ok());

        // 认领未提交终态（cancel 第二阶段会走这里）。
        assert!(
            lifecycle.claim_unsubmitted_terminal(),
            "unsubmitted reservation must be claimable"
        );
        // claim 已把 active=false，reservation 不再有效——消息不会迟到提交。
        assert!(
            reservation.ensure_active().is_err(),
            "claimed reservation must be invalidated so the pending send fails"
        );
        // 防 Drop 二次清理：claim 已收尾，标记 submitted 阻止 on_reservation_failed。
        reservation.mark_submitted();

        // 终态发完后重开闸门，下一轮可正常 reserve（与权威终态路径一致）。
        lifecycle.finish_terminal_emission();
        assert!(lifecycle.reserve().is_ok());
    }

    #[test]
    fn pending_cancel_is_armed_only_before_turn_started_and_consumed_once() {
        // reset_cancel_token 竞态修复的核心不变量：
        // 1. arm 仅在「active、已 submitted、且 turn_id=None（TurnStarted 未抵达）」
        //    时置标记（submitted 前置见 arm_pending_cancel_requires_submitted_reservation）；
        // 2. take 原子取出并清除（防跨轮泄漏）；
        // 3. 已 started 的 turn 不 arm（cancel_current 直接命中活跃 token）。
        let lifecycle = Arc::new(TurnLifecycle::default());

        // --- 场景 A：submit 后、TurnStarted 前 arm → take 返回 Some ---
        lifecycle.on_submitted();
        let epoch_a = lifecycle.current_turn_generation().expect("active epoch");
        lifecycle.arm_pending_cancel_and_cancel(epoch_a, || {});
        assert!(
            lifecycle.take_pending_cancel(epoch_a).is_some(),
            "pending_cancel must be armed before TurnStarted"
        );
        // take 已消费，再次取返回 None。
        assert!(
            lifecycle.take_pending_cancel(epoch_a).is_none(),
            "pending_cancel must be consumed exactly once"
        );

        // --- 场景 B：TurnStarted 后 arm → 不置标记 ---
        lifecycle.on_started("turn-1".to_string());
        let epoch_b = lifecycle.current_turn_generation().expect("active epoch");
        lifecycle.arm_pending_cancel_and_cancel(epoch_b, || {});
        assert!(
            lifecycle.take_pending_cancel(epoch_b).is_none(),
            "must not arm pending_cancel after TurnStarted"
        );

        // 清理：结束当前 turn。
        assert!(lifecycle.finish_once(|| {}).is_some());
    }

    #[test]
    fn arm_pending_cancel_reports_epoch_mismatch_and_arms_current() {
        // reviewer 点 6 的原子边界：arm 在 state 锁内校验 turn_epoch == epoch。
        // 轮次已切换 → 返回 false 且不置 pending（调用方必须跳过取消副作用）；
        // epoch 匹配 → 返回 true 并正常 arm（submitted 未 started 时置 pending）。
        let lifecycle = Arc::new(TurnLifecycle::default());

        // turn1：on_submitted 激活（active+submitted+epoch=1，turn_id=None）。
        assert!(lifecycle.on_submitted());
        let target = lifecycle
            .current_turn_generation()
            .expect("turn1 active epoch");

        // 轮次切换：turn1 终态 → turn2 reserve + 提交（epoch=2）。
        assert!(lifecycle.finish_once(|| {}).is_some());
        let reservation2 = lifecycle.reserve().expect("turn2 reserve");
        lifecycle.mark_reservation_submitted(reservation2.reservation_id);
        let epoch2 = lifecycle
            .current_turn_generation()
            .expect("turn2 active epoch");
        assert_eq!(epoch2, target + 1, "turn2 must bump the epoch past target");

        // stale target：拒绝 + pending 不落到新轮。
        assert!(!lifecycle.arm_pending_cancel_and_cancel(target, || {}));
        assert!(
            lifecycle.take_pending_cancel(epoch2).is_none(),
            "rejected arm must not set pending on the new turn"
        );

        // 当前 epoch：接受 + pending 置位（turn2 仍 submitted 未 started）。
        assert!(lifecycle.arm_pending_cancel_and_cancel(epoch2, || {}));
        assert!(
            lifecycle.take_pending_cancel(epoch2).is_some(),
            "accepted arm must set pending for the current turn"
        );

        // 防 Drop 副作用。
        reservation2.mark_submitted();
    }

    #[test]
    fn arm_pending_cancel_and_cancel_holds_state_lock_through_cancel() {
        // reviewer 点 8 的确定性回归：epoch 校验/arm 与取消副作用必须在同一
        // lifecycle state 锁临界区内原子完成。若 arm 后释放锁、再单独执行
        // cancel_current，两条同步调用之间没有 await 也不构成原子性保证——
        // 另一 worker 可在该窗口内收口旧轮、reserve/发送新轮并
        // reset_cancel_token，恢复后旧 cancel 命中新轮活跃 token。
        //
        // 编排：cancel 闭包（持锁）进入后阻塞，期间另一线程尝试 reserve——
        // 必须被 state 锁挡住；放行 cancel 后 reserve 才得以完成。若实现把
        // cancel 放在锁外，reserve 会在 cancel 完成前成功，断言失败。
        let lifecycle = Arc::new(TurnLifecycle::default());
        // turn1：on_submitted 激活（active+submitted+epoch=1），cancel 目标轮。
        assert!(lifecycle.on_submitted());
        let target = lifecycle.current_turn_generation().expect("turn1 epoch");

        let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();

        // cancel 线程：arm_pending_cancel_and_cancel 持锁执行 cancel 闭包，
        // 闭包通知「已进入临界区」后阻塞，模拟取消副作用执行中。
        let lc_for_cancel = lifecycle.clone();
        let cancel_thread = std::thread::spawn(move || {
            let ok = lc_for_cancel.arm_pending_cancel_and_cancel(target, || {
                entered_tx.send(()).expect("entered");
                release_rx.recv().expect("release");
            });
            assert!(ok, "epoch must match when the cancel side effect runs");
        });

        // 等 cancel 进入临界区（已持 state 锁）。
        entered_rx.recv().expect("cancel entered");

        // 另一 worker 尝试 reserve 新轮：应被 state 锁阻塞，直到 cancel 完成。
        let (reserve_done_tx, reserve_done_rx) = std::sync::mpsc::channel::<()>();
        let lc_for_reserve = lifecycle.clone();
        let reserve_thread = std::thread::spawn(move || {
            let _ = lc_for_reserve.reserve();
            reserve_done_tx.send(()).expect("reserve done");
        });

        // 确定性断言：cancel 持锁期间 reserve 不能完成。
        assert!(
            reserve_done_rx
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "reserve must be blocked while the cancel side effect holds the state lock"
        );

        // 放行 cancel → reserve 随后完成。
        release_tx.send(()).expect("release cancel");
        cancel_thread.join().expect("cancel thread joins");
        reserve_thread.join().expect("reserve thread joins");
        assert!(
            reserve_done_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .is_ok(),
            "reserve must complete after the cancel side effect releases the state lock"
        );
    }

    #[test]
    fn arm_pending_cancel_and_cancel_skips_cancel_on_stale_epoch() {
        // reviewer 点 6 + 8：epoch 不匹配（轮次已切换）时，cancel 闭包不得
        // 执行——stale 的取消不能落到新轮已 reset_cancel_token 的活跃 token 上。
        let lifecycle = Arc::new(TurnLifecycle::default());
        // turn1：on_submitted 激活（epoch=1）。
        assert!(lifecycle.on_submitted());
        let target = lifecycle.current_turn_generation().expect("turn1 epoch");

        // 轮次切换：turn1 终态 → turn2 reserve + 提交（epoch=2）。
        assert!(lifecycle.finish_once(|| {}).is_some());
        let reservation2 = lifecycle.reserve().expect("turn2 reserve");
        lifecycle.mark_reservation_submitted(reservation2.reservation_id);
        let epoch2 = lifecycle
            .current_turn_generation()
            .expect("turn2 active epoch");
        assert_eq!(epoch2, target + 1, "turn2 must bump the epoch past target");

        // 用发起时快照 target 组合 arm+cancel：epoch 不匹配 → 返回 false 且
        // cancel 闭包不执行（若误执行，探针会置位）。
        let cancel_ran = Arc::new(AtomicBool::new(false));
        let probe = cancel_ran.clone();
        assert!(
            !lifecycle.arm_pending_cancel_and_cancel(target, move || {
                probe.store(true, Ordering::Release);
            }),
            "stale epoch must be rejected and the cancel closure skipped"
        );
        assert!(
            !cancel_ran.load(Ordering::Acquire),
            "cancel closure must not run when the epoch no longer matches"
        );
        assert!(
            lifecycle.take_pending_cancel(epoch2).is_none(),
            "stale cancel must not bind pending to the new turn"
        );

        // 防 Drop 副作用。
        reservation2.mark_submitted();
    }

    #[test]
    fn arm_pending_cancel_and_cancel_skips_cancel_when_idle_epoch_zero() {
        // G2/epoch-0 哨兵守卫：fresh 会话（从未有过 turn，turn_epoch==0）时
        // 发起 cancel，调用方以 target.unwrap_or(0) 传入 0，`turn_epoch != 0`
        // 校验会通过（0 == 0）。若 engine 已为「即将启动的定时轮」在场
        // （run_scheduled_turn 的 spawn→submit 窗口），执行 cancel 闭包会命中
        // 该新 token，取消信号与 reset_cancel_token 竞速、结果不确定。
        // idle 守卫必须跳过取消闭包（返回 true 但不执行），且不 arm pending。
        let lifecycle = Arc::new(TurnLifecycle::default());
        // fresh 会话：Default 的 TurnLifecycleState.turn_epoch == 0、active == false。
        assert_eq!(
            lifecycle.current_turn_generation(),
            None,
            "idle fresh session"
        );

        let cancel_ran = Arc::new(AtomicBool::new(false));
        let probe = cancel_ran.clone();
        // target=None 时调用方以 0 哨兵传入；epoch 匹配（0==0）但 idle →
        // 不执行 cancel 闭包、返回 true（调用方按 armed 路径继续，只清遗留
        // 子代理，不误伤尚未启动的定时轮）。
        assert!(
            lifecycle.arm_pending_cancel_and_cancel(0, move || {
                probe.store(true, Ordering::Release);
            }),
            "idle must report success so the caller can continue (cascade only)"
        );
        assert!(
            !cancel_ran.load(Ordering::Acquire),
            "cancel closure must not run on an idle fresh session (epoch-0 sentinel)"
        );
        assert!(
            lifecycle.take_pending_cancel(0).is_none(),
            "idle must not arm pending_cancel"
        );
    }

    #[test]
    fn claim_unsubmitted_terminal_for_epoch_rejects_stale_target() {
        // reviewer 点 7 的原子边界：未提交认领必须在 state 锁内与
        // turn_epoch == target 同时校验。generation 检查与认领分离时，
        // reserve_turn（不取 turn_lock）可在两者之间完成切轮，无 epoch 校验
        // 的认领会把新轮 reservation 误认领为 Interrupted、清掉新轮 busy。
        let lifecycle = Arc::new(TurnLifecycle::default());

        // turn1：reserve 未提交（epoch=1），cancel 阶段二走认领路径。
        let reservation1 = lifecycle.reserve().expect("turn1 reserve");
        let target = lifecycle
            .current_turn_generation()
            .expect("turn1 active epoch");
        assert_eq!(target, 1);

        // 模拟「generation 检查通过后、认领前」另一 worker 切轮：
        // turn1 终态（未提交认领路径）→ turn2 reserve（epoch=2，未提交）。
        assert!(lifecycle.finish_unsubmitted_once());
        let reservation2 = lifecycle.reserve().expect("turn2 reserve");
        assert_eq!(
            lifecycle.current_turn_generation(),
            Some(target + 1),
            "turn2 must bump the epoch past target"
        );

        // stale target 认领被拒：返回 false 且无副作用，turn2 完好。
        assert!(
            !lifecycle.claim_unsubmitted_terminal_for_epoch(target),
            "claim with a stale target must no-op"
        );
        assert!(
            reservation2.ensure_active().is_ok(),
            "new turn unsubmitted reservation must survive a stale claim"
        );

        // 当前 epoch 认领成功：原子校验通过后正常认领，turn2 被失效。
        assert!(lifecycle.claim_unsubmitted_terminal_for_epoch(target + 1));
        assert!(
            reservation2.ensure_active().is_err(),
            "claimed reservation must be invalidated"
        );
        lifecycle.finish_terminal_emission();

        // 防 Drop 副作用：turn1/turn2 均已认领终态（reservation 幂等）。
        drop(reservation1);
        drop(reservation2);
    }

    #[test]
    fn pending_cancel_survives_until_turn_started_consumes_it() {
        // 端到端时序模拟：cancel 在 submit 后、TurnStarted 前 arm →
        // TurnStarted 抵达时 take 消费标记并触发重新 cancel。
        let lifecycle = Arc::new(TurnLifecycle::default());

        // submit（op 入队，turn_lock 已释放，但 Engine 尚未 dequeue）。
        lifecycle.on_submitted();

        // cancel 路径：arm_pending_cancel（turn_id 仍为 None → 置标记）。
        let epoch = lifecycle.current_turn_generation().expect("active epoch");
        lifecycle.arm_pending_cancel_and_cancel(epoch, || {});

        // Engine 执行 reset_cancel_token + 发 TurnStarted → 转发器先
        // on_started_transition（设 turn_id），再 take_pending_cancel。
        // 同一轮内 on_started 不 bump epoch（active 已 true），take 仍匹配。
        lifecycle.on_started("turn-reset".to_string());
        let pending = lifecycle.take_pending_cancel(epoch);
        assert!(
            pending.is_some(),
            "pending_cancel must survive until TurnStarted consumes it"
        );

        // 消费后标记清除，下一轮不受影响。
        assert!(lifecycle.take_pending_cancel(epoch).is_none());
        assert!(lifecycle.finish_once(|| {}).is_some());
    }

    #[test]
    fn turn_epoch_advances_on_every_admission_path_and_survives_terminal() {
        // turn_epoch 是 cancel generation 的身份来源，必须在三个 active 入口
        // （reserve / on_submitted / on_started_transition newly_active）都单调
        // 自增，且与 is_active() 同口径：终态发送临界区（terminal_closing）内
        // 仍是本轮 epoch，finish 后才回 None。
        let lifecycle = Arc::new(TurnLifecycle::default());

        // 空闲 → None。
        assert_eq!(lifecycle.current_turn_generation(), None);

        // reserve 推进到 epoch=1（reserve 后未 submitted，走未提交认领终态）。
        let reservation = lifecycle.reserve().expect("reserve");
        assert_eq!(lifecycle.current_turn_generation(), Some(1));

        // 结束本轮（claim_unsubmitted 进入 terminal_closing 临界区，epoch 仍是 1）。
        // claim 成功后 reservation 失效（ensure_active 失败），无需 mark_submitted。
        assert!(lifecycle.claim_unsubmitted_terminal());
        assert_eq!(
            lifecycle.current_turn_generation(),
            Some(1),
            "terminal_closing 临界区内 generation 仍是本轮 epoch"
        );
        // finish 释放闸门后回到空闲 → None。
        lifecycle.finish_terminal_emission();
        drop(reservation);
        assert_eq!(lifecycle.current_turn_generation(), None);

        // finish 释放闸门后回到空闲 → None。
        assert_eq!(lifecycle.current_turn_generation(), None);

        // on_submitted（定时任务路径，非 reservation）推进到 epoch=2。
        assert!(lifecycle.on_submitted());
        assert_eq!(lifecycle.current_turn_generation(), Some(2));
        assert!(lifecycle.finish_once(|| {}).is_some());

        // on_started_transition 从 idle 激活（newly_active 分支）推进到 epoch=3。
        assert!(lifecycle
            .on_started_transition("turn-stale".to_string())
            .is_some());
        assert_eq!(lifecycle.current_turn_generation(), Some(3));
        assert!(lifecycle.finish_once(|| {}).is_some());
    }

    #[test]
    fn pending_cancel_tied_to_epoch_is_dropped_after_turn_change() {
        // reviewer 点 4 的核心回归：并发取消请求 C2 在旧轮 arm 了 pending_cancel，
        // 恢复时已变成新轮。pending_cancel 携带 epoch，take 时校验不匹配 → 丢弃，
        // 不误取消新轮（不触发 approve_handle.cancel 重放）。reserve() 也必须清除
        // 旧轮遗留的 stale pending_cancel，防跨轮污染。
        let lifecycle = Arc::new(TurnLifecycle::default());

        // 旧轮：submit（epoch=1），arm pending_cancel 绑定到 epoch=1。
        lifecycle.on_submitted();
        let epoch_old = lifecycle.current_turn_generation().expect("old epoch");
        lifecycle.arm_pending_cancel_and_cancel(epoch_old, || {});

        // 结束旧轮，新一轮 reserve（epoch=2）。
        assert!(lifecycle.finish_once(|| {}).is_some());
        let _reservation = lifecycle.reserve().expect("new turn");
        let epoch_new = lifecycle.current_turn_generation().expect("new epoch");
        assert_ne!(epoch_old, epoch_new);

        // forwarder 在新轮 TurnStarted 后用新轮 epoch 取 pending_cancel：
        // C2 留下的 stale pending（epoch_old）必须被丢弃，不重放 cancel。
        assert_eq!(
            lifecycle.take_pending_cancel(epoch_new),
            None,
            "stale pending_cancel bound to a previous epoch must be dropped, not replayed onto the new turn"
        );
    }

    #[test]
    fn concurrent_submission_is_rejected_until_the_active_turn_finishes() {
        let lifecycle = TurnLifecycle::default();
        assert!(lifecycle.on_submitted());
        assert!(!lifecycle.on_submitted());
        assert!(lifecycle.finish_once(|| {}).is_some());
        assert!(lifecycle.on_submitted());
    }

    #[test]
    fn forwarder_stop_and_reclaim_share_the_same_terminal_gate() {
        let lifecycle = TurnLifecycle::default();
        lifecycle.on_submitted();
        lifecycle.on_started("turn-1".to_string());
        assert!(lifecycle.finish_once(|| {}).is_some());
        assert_eq!(lifecycle.finish_once(|| panic!("duplicate terminal")), None);
    }

    #[test]
    fn terminal_side_effects_run_only_for_the_path_that_claimed_the_turn() {
        let lifecycle = TurnLifecycle::default();
        let effects = Cell::new(0_u8);
        lifecycle.on_submitted();
        lifecycle.on_started("turn-claim".to_string());

        if lifecycle.claim_terminal().is_some() {
            effects.set(effects.get() + 1);
        }
        if lifecycle.claim_terminal().is_some() {
            effects.set(effects.get() + 10);
        }

        assert_eq!(effects.get(), 1);
    }

    #[test]
    fn concurrent_rejection_happens_before_one_shot_state_is_consumed() {
        let lifecycle = Arc::new(TurnLifecycle::default());
        let first = lifecycle.reserve().expect("first admission");
        let mut one_shot = Some("skill body".to_string());

        let rejected = lifecycle.reserve();
        assert!(rejected
            .as_ref()
            .is_err_and(|error| error.to_string().contains("session_turn_in_progress")));
        assert_eq!(one_shot.as_deref(), Some("skill body"));

        let consumed_after_admission = one_shot.take();
        assert_eq!(consumed_after_admission.as_deref(), Some("skill body"));
        drop(first);
        assert!(lifecycle.reserve().is_ok(), "drop restores admission");
    }

    #[test]
    fn real_turn_started_claims_admission_and_preserves_the_rule_if_command_lags() {
        let lifecycle = Arc::new(TurnLifecycle::default());
        let mut reservation = lifecycle.reserve().expect("reserve");
        reservation.set_base_transcript_revision("revision-before".to_string());
        reservation
            .set_transcript_with_baseline(
                TranscriptOperation::Append,
                message("user", "visible prompt"),
                "revision-before".to_string(),
            )
            .expect("display transcript");
        reservation
            .prepare_actual_user_content("<private>injected</private>".to_string())
            .expect("actual prompt");

        // The engine can publish TurnStarted before EngineHandle::send returns
        // to the command future. That event is authoritative submission.
        let started = lifecycle
            .on_started_transition("turn-fast".to_string())
            .expect("started transition");
        let payload = started
            .admission
            .expect("admission")
            .user_payload("session-1");
        assert_eq!(payload["content"], "visible prompt");
        assert_eq!(payload["operation"], "append");
        assert_eq!(payload["base_transcript_revision"], "revision-before");

        // Simulate cancellation of the command future before its local guard
        // can call mark_submitted. The forwarder-owned rule must survive.
        drop(reservation);
        let (sanitized, matched) =
            lifecycle.sanitize_messages(vec![engine_user("<private>injected</private>")]);
        assert!(matched);
        assert_eq!(sanitized, vec![message("user", "visible prompt")]);
        assert!(lifecycle.claim_terminal().is_some());
    }

    #[test]
    fn reclaim_invalidates_reserved_turn_without_claiming_a_terminal() {
        let lifecycle = Arc::new(TurnLifecycle::default());
        let mut reservation = lifecycle.reserve().expect("reserve");
        reservation
            .set_transcript(
                TranscriptOperation::Append,
                message("user", "never submitted"),
            )
            .expect("display transcript");
        reservation
            .prepare_actual_user_content("private unsent prompt".to_string())
            .expect("actual prompt");

        assert!(lifecycle.claim_reclaimed_transition().is_none());
        assert_eq!(lifecycle.claim_terminal(), None);
        let invalidated = reservation
            .prepare_actual_user_content("private unsent prompt".to_string())
            .expect_err("reclaimed reservation must reject a later send");
        assert!(invalidated.to_string().contains("reservation invalidated"));
        drop(reservation);
        let next = lifecycle.reserve().expect("reclaim released reservation");
        let (messages, matched) =
            lifecycle.sanitize_messages(vec![engine_user("private unsent prompt")]);
        assert!(!matched);
        assert_eq!(messages, vec![engine_user("private unsent prompt")]);
        drop(next);
    }

    #[test]
    fn reclaim_of_submitted_accept_plan_carries_admission_once_before_terminal() {
        let lifecycle = Arc::new(TurnLifecycle::default());
        let mut reservation = lifecycle.reserve().expect("reserve");
        reservation.set_base_transcript_revision("plan-revision".to_string());
        reservation
            .set_admission_metadata(TurnAdmissionMetadata::accept_plan(
                "plan-ticket".to_string(),
                SessionModeState::default(),
            ))
            .expect("accept metadata");
        reservation
            .set_transcript_with_baseline(
                TranscriptOperation::Append,
                message("user", "✅ 就这么干"),
                "plan-revision".to_string(),
            )
            .expect("display transcript");
        reservation
            .prepare_actual_user_content("execute approved plan".to_string())
            .expect("actual prompt");
        reservation.mark_submitted();

        let reclaimed = lifecycle
            .claim_reclaimed_transition()
            .expect("submitted turn terminal");
        let payload = reclaimed
            .admission
            .expect("reclaim carries pending admission")
            .user_payload("session-plan");
        assert_eq!(payload["action"], "accept_plan");
        assert_eq!(payload["plan_id"], "plan-ticket");
        assert_eq!(payload["mode"], "yolo");
        assert_eq!(payload["mode_state"]["mode"], "yolo");
        assert_eq!(payload["content"], "✅ 就这么干");
        let fallback = reclaimed
            .fallback
            .expect("submitted reclaim carries durable transcript fallback");
        assert_eq!(fallback.operation, TranscriptOperation::Append);
        assert_eq!(fallback.baseline_revision, "plan-revision");
        assert_eq!(fallback.display_message, message("user", "✅ 就这么干"));
        assert_eq!(
            reclaimed.terminal,
            EmittedTerminal { turn_id: None },
            "admitted-but-not-started reclaim still owns one terminal"
        );
        assert!(lifecycle.claim_reclaimed_transition().is_none());
        assert!(
            lifecycle
                .on_started_transition("late-turn".to_string())
                .is_none(),
            "late TurnStarted cannot resurrect a reclaimed turn"
        );
    }

    #[test]
    fn append_sanitization_replaces_injected_user_and_preserves_tool_user_blocks() {
        let lifecycle = Arc::new(TurnLifecycle::default());
        let mut reservation = lifecycle.reserve().expect("reserve");
        reservation
            .set_transcript(
                TranscriptOperation::Append,
                message("user", "visible prompt\n\n📎 report.pdf"),
            )
            .expect("display transcript");
        reservation
            .prepare_actual_user_content("<skill>secret</skill>\nvisible prompt".to_string())
            .expect("actual prompt");

        let tool_result = Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: "tool output".to_string(),
                is_error: None,
                content_blocks: None,
            }],
        };
        let (sanitized, matched) = lifecycle.sanitize_messages(vec![
            message("assistant", "older"),
            engine_user("<skill>secret</skill>\nvisible prompt"),
            tool_result.clone(),
        ]);

        assert!(matched);
        assert_eq!(
            sanitized[1],
            message("user", "visible prompt\n\n📎 report.pdf")
        );
        assert_eq!(sanitized[2], tool_result);
    }

    #[test]
    fn edit_sanitization_keeps_engine_truncation_and_replaces_only_new_user() {
        let lifecycle = Arc::new(TurnLifecycle::default());
        let mut reservation = lifecycle.reserve().expect("reserve");
        reservation
            .set_transcript(
                TranscriptOperation::EditLast,
                message("user", "edited display"),
            )
            .expect("display transcript");
        reservation
            .prepare_actual_user_content("<persona>private</persona>\nedited".to_string())
            .expect("actual prompt");

        let (sanitized, matched) = lifecycle.sanitize_messages(vec![
            message("user", "kept first turn"),
            message("assistant", "kept answer"),
            engine_user("<persona>private</persona>\nedited"),
        ]);

        assert!(matched);
        assert_eq!(sanitized.len(), 3);
        assert_eq!(sanitized[0], message("user", "kept first turn"));
        assert_eq!(sanitized[2], message("user", "edited display"));
    }

    #[test]
    fn duplicate_raw_prompts_map_newest_rule_to_newest_message() {
        let lifecycle = Arc::new(TurnLifecycle::default());
        let mut first = lifecycle.reserve().expect("first reserve");
        first
            .set_transcript(
                TranscriptOperation::Append,
                message("user", "first display"),
            )
            .unwrap();
        first
            .prepare_actual_user_content("same raw prompt".to_string())
            .unwrap();
        first.mark_submitted();
        assert!(lifecycle.finish_once(|| {}).is_some());

        let mut second = lifecycle.reserve().expect("second reserve");
        second
            .set_transcript(
                TranscriptOperation::Append,
                message("user", "second display"),
            )
            .unwrap();
        second
            .prepare_actual_user_content("same raw prompt".to_string())
            .unwrap();

        let (both, matched) = lifecycle.sanitize_messages(vec![
            engine_user("same raw prompt"),
            message("assistant", "between"),
            engine_user("same raw prompt"),
        ]);
        assert!(matched);
        assert_eq!(both[0], message("user", "first display"));
        assert_eq!(both[2], message("user", "second display"));

        // If compaction removed the older occurrence, the surviving newest
        // prompt must still use the newest display rule.
        let (compacted, matched) =
            lifecycle.sanitize_messages(vec![engine_user("same raw prompt")]);
        assert!(matched);
        assert_eq!(compacted, vec![message("user", "second display")]);
    }

    #[test]
    fn failed_reserved_submission_rolls_back_lifecycle_and_sanitization_rule() {
        let lifecycle = Arc::new(TurnLifecycle::default());
        {
            let mut reservation = lifecycle.reserve().expect("reserve");
            reservation
                .set_transcript(TranscriptOperation::Append, message("user", "display"))
                .expect("display transcript");
            reservation
                .prepare_actual_user_content("private actual".to_string())
                .expect("actual prompt");
            // Simulate EngineHandle::send returning an error: no mark_submitted.
        }

        let next = lifecycle.reserve().expect("reservation restored");
        let (messages, matched) = lifecycle.sanitize_messages(vec![engine_user("private actual")]);
        assert!(!matched);
        assert_eq!(messages, vec![engine_user("private actual")]);
        drop(next);
    }
}

#[cfg(test)]
mod scheduled_turn_tests {
    use super::{
        apply_scheduled_turn_policy, persist_successful_tool_artifact,
        scheduled_tool_should_auto_approve, EngineTurnSignal, TurnCompletionTracker,
    };
    use crate::features::sessions::{ScheduledRunMode, ScheduledRunProfile};
    use deepseek_tui::compaction::CompactionConfig;
    use deepseek_tui::config::Config;
    use deepseek_tui::core::events::TurnOutcomeStatus;
    use deepseek_tui::core::ops::{Op, UserInputProvenance};
    use deepseek_tui::tools::goal::GoalStatus;
    use deepseek_tui::tui::app::AppMode;
    use deepseek_tui::tui::approval::ApprovalMode;
    use std::path::PathBuf;

    fn base_op() -> Op {
        let config = Config::default();
        Op::SendMessage {
            content: "scheduled prompt".to_string(),
            mode: AppMode::Yolo,
            route: Box::new(
                deepseek_tui::route_runtime::resolve_runtime_route(
                    &config,
                    config.api_provider(),
                    Some("fallback-model"),
                )
                .expect("resolve fallback route"),
            ),
            compaction: Box::new(CompactionConfig::default()),
            goal_objective: None,
            goal_token_budget: None,
            goal_status: GoalStatus::Complete,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            auto_model: true,
            allow_shell: false,
            trust_mode: false,
            auto_approve: true,
            approval_mode: ApprovalMode::Auto,
            translation_enabled: false,
            allowed_tools: None,
            dynamic_tools: Vec::new(),
            hook_executor: None,
            verbosity: None,
            provenance: UserInputProvenance::Runtime,
        }
    }

    fn scheduled_route_and_compaction(
        profile: &ScheduledRunProfile,
    ) -> (
        deepseek_tui::route_runtime::ResolvedRuntimeRoute,
        CompactionConfig,
    ) {
        let config = Config::default();
        let route = deepseek_tui::route_runtime::resolve_runtime_route(
            &config,
            config.api_provider(),
            Some(&profile.model),
        )
        .expect("resolve scheduled route");
        let compaction = CompactionConfig {
            model: profile.model.clone(),
            ..Default::default()
        };
        (route, compaction)
    }

    fn profile(auto_approve: bool) -> ScheduledRunProfile {
        ScheduledRunProfile {
            task_id: "task-1".to_string(),
            model: "scheduled-model".to_string(),
            model_id: Some("scheduled-model-id".to_string()),
            workspace: PathBuf::from("D:/scheduled-workspace"),
            mode: ScheduledRunMode::Plan,
            allow_shell: true,
            trust_mode: true,
            auto_approve,
        }
    }

    #[test]
    fn scheduled_policy_is_exact_and_preserves_external_user_authority() {
        let mut op = base_op();
        let profile = profile(false);
        let (route, compaction) = scheduled_route_and_compaction(&profile);
        apply_scheduled_turn_policy(&mut op, &profile, route, compaction).expect("send-message op");

        match op {
            Op::SendMessage {
                mode,
                route,
                auto_model,
                allow_shell,
                trust_mode,
                auto_approve,
                approval_mode,
                provenance,
                ..
            } => {
                assert_eq!(
                    mode,
                    AppMode::Agent,
                    "a legacy persisted mode must not bypass autoApprove=false"
                );
                assert_eq!(route.model(), "scheduled-model");
                assert!(!auto_model);
                assert!(allow_shell);
                assert!(trust_mode);
                assert!(!auto_approve);
                assert_eq!(approval_mode, ApprovalMode::Never);
                assert_eq!(provenance, UserInputProvenance::ExternalUser);
            }
            other => panic!("unexpected op: {other:?}"),
        }
    }

    #[test]
    fn scheduled_unattended_turn_cannot_bypass_persisted_policy() {
        let mut legacy_yolo = profile(false);
        legacy_yolo.mode = ScheduledRunMode::Yolo;
        let mut op = base_op();

        let (route, compaction) = scheduled_route_and_compaction(&legacy_yolo);
        apply_scheduled_turn_policy(&mut op, &legacy_yolo, route, compaction)
            .expect("scheduled unattended op");

        match op {
            Op::SendMessage {
                mode,
                auto_approve,
                approval_mode,
                ..
            } => {
                assert_eq!(mode, AppMode::Agent);
                assert!(!auto_approve);
                assert_eq!(approval_mode, ApprovalMode::Never);
            }
            other => panic!("unexpected op: {other:?}"),
        }
        assert!(!scheduled_tool_should_auto_approve(
            Some(&legacy_yolo),
            false
        ));
    }

    #[test]
    fn scheduled_force_prompt_never_auto_approves() {
        let auto = profile(true);
        assert!(scheduled_tool_should_auto_approve(Some(&auto), false));
        assert!(!scheduled_tool_should_auto_approve(Some(&auto), true));
        assert!(scheduled_tool_should_auto_approve(None, true));
    }

    #[test]
    fn lifecycle_waits_for_authoritative_terminal_and_deduplicates_it() {
        let mut tracker = TurnCompletionTracker::default();
        assert_eq!(
            tracker.on_started("turn-1".to_string()),
            EngineTurnSignal::Started {
                turn_id: "turn-1".to_string()
            }
        );
        tracker.on_fatal_error("fatal".to_string());
        assert_eq!(
            tracker.on_terminal(TurnOutcomeStatus::Failed, None),
            Some(EngineTurnSignal::Terminal {
                turn_id: "turn-1".to_string(),
                status: TurnOutcomeStatus::Failed,
                error: Some("fatal".to_string()),
            })
        );
        assert_eq!(
            tracker.on_terminal(TurnOutcomeStatus::Failed, Some("duplicate".to_string())),
            None
        );
    }

    #[test]
    fn scheduled_tool_artifacts_persist_without_a_webview_listener_and_reopen() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = std::env::temp_dir().join(format!(
            "pinvou3-scheduled-artifact-forwarder-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &root);
        let workspace = root.join("external-workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let report = workspace.join("report.md");
        std::fs::write(&report, "durable report").expect("artifact file");
        let store = crate::features::sessions::SessionStore::boot().expect("session store");
        let scheduled = store
            .create_scheduled_run(ScheduledRunProfile {
                task_id: "artifact-task".to_string(),
                model: "scheduled-model".to_string(),
                model_id: None,
                workspace: workspace.clone(),
                mode: ScheduledRunMode::Agent,
                allow_shell: false,
                trust_mode: false,
                auto_approve: false,
            })
            .expect("scheduled session");

        let persisted = persist_successful_tool_artifact(
            &store,
            &scheduled.metadata.id,
            &workspace,
            "write_file",
            &serde_json::json!({"path": "report.md", "content": "durable report"}),
            "Created report.md",
        )
        .expect("persist tool artifact")
        .expect("artifact candidate");
        assert_eq!(
            persisted,
            std::fs::canonicalize(&report).expect("canonical report")
        );

        let appendix = workspace.join("appendix.md");
        std::fs::write(&appendix, "patched appendix").expect("patched artifact file");
        persist_successful_tool_artifact(
            &store,
            &scheduled.metadata.id,
            &workspace,
            "File",
            &serde_json::json!({
                "action": "patch",
                "patch": "*** Begin Patch\n*** Update File: report.md\n*** Add File: appendix.md\n*** Delete File: deleted.md\n*** End Patch"
            }),
            &serde_json::json!({
                "files_applied": 2,
                "touched_files": ["report.md", "appendix.md", "deleted.md"]
            })
            .to_string(),
        )
        .expect("persist canonical patch artifacts");

        drop(store);
        let reopened = crate::features::sessions::SessionStore::boot().expect("reopen store");
        let paths: Vec<_> = reopened
            .load(&scheduled.metadata.id)
            .expect("reopen scheduled session")
            .artifacts
            .into_iter()
            .map(|artifact| artifact.storage_path)
            .collect();
        assert_eq!(
            paths,
            vec![
                persisted,
                std::fs::canonicalize(&appendix).expect("canonical appendix")
            ]
        );

        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
// 测试借 platform::paths::tests::ENV_LOCK(std Mutex)串行化全局 env;单线程测试内跨 await 持有无竞争者,不会死锁。
#[allow(clippy::await_holding_lock)]
mod live_tests {
    use super::*;
    use crate::features::monitor::SelfMetrics;
    use crate::features::sessions::SerializableMode;

    /// RAII 恢复 env 原值(本模块 #[ignore] 真机测试写 DEEPSEEK_*/PINVOU3_* env,
    /// 须保证退出时恢复——含 panic 路径,避免 `cargo test -- --ignored` 合跑时污染)。
    struct EnvRestore {
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvRestore {
        fn snapshot(names: &'static [&'static str]) -> Self {
            let saved = names.iter().map(|&n| (n, std::env::var(n).ok())).collect();
            EnvRestore { saved }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (name, val) in &self.saved {
                match val {
                    Some(v) => std::env::set_var(name, v),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    /// 真机集成(#[ignore]):打真 vLLM 跑一轮,drain rx_event 时**照 forwarder 四臂
    /// 原样喂 SelfMetrics**,证明真实事件流(TurnStarted→MessageDelta→TurnComplete+真
    /// usage)累加出合理指标 + 事件顺序符合预期(TurnStarted 在首 MessageDelta 前)。
    ///
    ///   DEEPSEEK_ALLOW_INSECURE_HTTP=1 DEEPSEEK_FORCE_HTTP1=1 \
    ///   cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml \
    ///     --lib engine::live_tests::self_metrics_populates_from_real_turn \
    ///     -- --ignored --nocapture
    #[ignore]
    #[tokio::test]
    async fn self_metrics_populates_from_real_turn() {
        // 写 DEEPSEEK_*/PINVOU3_* env:虽 #[ignore] 不入默认套件,仍须持 crate 级
        // ENV_LOCK 串行并保证退出恢复,避免被 `cargo test -- --ignored` 一起跑时污染
        // 其它测试(或本测试 panic 后留下脏 env)。
        let _lock = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _restore = EnvRestore::snapshot(&[
            "DEEPSEEK_ALLOW_INSECURE_HTTP",
            "DEEPSEEK_FORCE_HTTP1",
            "PINVOU3_SKIP_WARMUP",
        ]);
        std::env::set_var("DEEPSEEK_ALLOW_INSECURE_HTTP", "1");
        std::env::set_var("DEEPSEEK_FORCE_HTTP1", "1");
        std::env::set_var("PINVOU3_SKIP_WARMUP", "1");

        let bridge = Pinvou3Bridge::boot().expect("boot bridge");
        let engine = AppEngine::spawn_headless(bridge)
            .await
            .expect("spawn engine");

        let m = SelfMetrics::default();
        let sid = "live-test";
        let prompts = ["用一句话介绍你自己。", "再用一句话讲个冷笑话。"];

        // 跑两轮:首轮 = 冷/warmup(A 跳过 TTFT/TPS),二轮 = 暖(记)。
        engine
            .send_user_message(
                prompts[0].to_string(),
                SerializableMode::Yolo.to_app_mode(),
                None,
                false,
            )
            .await
            .expect("send_user_message #1");

        let mut rx = engine.handle.rx_event.write().await;
        let mut turns_done = 0usize;
        let mut seq: Vec<String> = Vec::new();
        let mut tool_in_turn2 = false;
        loop {
            let ev = tokio::time::timeout(std::time::Duration::from_secs(90), rx.recv())
                .await
                .expect("timeout waiting for event");
            let Some(ev) = ev else { break };
            let turn = turns_done + 1;
            match ev {
                Event::TurnStarted { .. } => {
                    seq.push(format!("t{turn}:TurnStarted"));
                    m.on_turn_started(sid);
                }
                Event::MessageDelta { content, .. } => {
                    m.on_message_delta(sid, content.chars().count());
                }
                Event::ToolCallStarted { .. } => {
                    seq.push(format!("t{turn}:ToolCallStarted"));
                    m.on_tool(sid);
                    if turns_done == 1 {
                        tool_in_turn2 = true;
                    }
                }
                Event::TurnComplete { usage, .. } => {
                    seq.push(format!("t{turn}:TurnComplete(out={})", usage.output_tokens));
                    m.on_turn_complete(
                        sid,
                        usage.input_tokens,
                        usage.output_tokens,
                        usage.prompt_cache_hit_tokens,
                        usage.prompt_cache_miss_tokens,
                    );
                    turns_done += 1;
                    if turns_done == 1 {
                        engine
                            .send_user_message(
                                prompts[1].to_string(),
                                SerializableMode::Yolo.to_app_mode(),
                                None,
                                false,
                            )
                            .await
                            .expect("send_user_message #2");
                    } else {
                        break;
                    }
                }
                _ => {}
            }
        }

        let s = m.snapshot();
        eprintln!("[live] event seq: {seq:?}");
        eprintln!(
            "[live] snapshot: ttft_count={} ttft_sum_s={:.4} tps_tokens={} tps_time_s={:.4} gen={} prompt={} cache_hit={} cache_miss={}",
            s.ttft_count, s.ttft_sum_s, s.tps_tokens, s.tps_time_s,
            s.gen_tokens_total, s.prompt_tokens_total, s.cache_hit_tokens, s.cache_miss_tokens
        );
        if s.ttft_count > 0 {
            eprintln!(
                "[live] → 稳态 TTFT={:.3}s  TPS={:.1} tok/s (已排除首轮冷启)",
                s.ttft_sum_s / s.ttft_count as f64,
                if s.tps_time_s > 0.0 {
                    s.tps_tokens as f64 / s.tps_time_s
                } else {
                    0.0
                }
            );
        }

        assert_eq!(turns_done, 2, "未跑满两轮 seq={seq:?}");
        assert!(
            s.gen_tokens_total > 0,
            "无 output token 累加(usage 空?) seq={seq:?}"
        );
        // 二轮纯文本(无工具)才断言:首轮已被 A 跳过,TTFT 应只来自二轮。
        if !tool_in_turn2 {
            assert_eq!(
                s.ttft_count, 1,
                "二轮应恰好记 1 次 TTFT(首轮跳过) seq={seq:?}"
            );
            assert!(s.tps_time_s > 0.0, "TPS 时长未记 seq={seq:?}");
        }
    }
}
