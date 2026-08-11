//! pinvou3-app 与 CodeWhale Engine 的桥接层。
//!
//! 职责：
//!  1. 通过 [`bridge::Pinvou3Bridge`] 把 `~/.pinvou3/settings.json` 翻译成
//!     [`EngineConfig`] / [`DtConfig`]，然后 `spawn_engine`，存到 Tauri State
//!  2. 后台 task 持续读 `EngineHandle::rx_event`，转译成 Tauri 事件
//!     （`chat:delta` / `chat:reasoning_start` / `chat:reasoning_delta` /
//!     `chat:reasoning_done` / `chat:tool_start` / `chat:tool_end` / `chat:done`
//!     / `chat:plan_ready`）
//!  3. 暴露 `send_user_message()` 给 [`commands::chat`] 调用
//!
//! 所有配置决策（model / paths / locale / allow_shell ...）都在 bridge 里，
//! 这一层只做 "boot engine + 转发事件"。Engine 自管 session 状态，多轮对话
//! 在同一个 EngineHandle 内自然累积。

use std::path::{Path, PathBuf};
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

use crate::core::mode_state::{SerializableMode, SessionModeState};
pub(crate) use crate::features::assistant::engine_support::EngineTurnSignal;
use crate::features::assistant::engine_support::{
    apply_scheduled_turn_policy, maybe_notify_task_completed, persist_successful_tool_artifact,
    scheduled_tool_should_auto_approve, TurnCompletionTracker,
};
use crate::features::assistant::platform::bridge::Pinvou3Bridge;
use crate::features::assistant::turn_shell_tasks::TurnShellTaskRegistry;
use crate::features::sessions::{
    transcript_revision, ChatEngineState, ScheduledEngineState, ScheduledRunProfile,
    ScheduledTokenAccounting, SessionStore,
};

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
    /// [`arm_pending_cancel`]: TurnLifecycle::arm_pending_cancel
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
    pub(crate) fn arm_pending_cancel(&self, epoch: u64) -> bool {
        let mut state = self.state.lock();
        if state.turn_epoch != epoch {
            // 轮次已切换（目标轮已结束、新轮已 reserve）：调用方必须跳过取消。
            return false;
        }
        if state.submitted && state.turn_id.is_none() && state.active {
            state.pending_cancel = Some(epoch);
        }
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
        // C 方案(P-no-disk): instructions 走 Inline,不再写 disk(远端)。
        // 工作流会话不再施加监工白名单(对话型监工已废弃);SubAgent 角色的工具
        // 由 agent_registry.json 各自约束,与此处无关。
        let scheduled_profile = store.scheduled_profile(session_id);
        let scheduled_base_total_tokens = if scheduled_profile.is_some() {
            Some(store.load(session_id)?.metadata.total_tokens)
        } else {
            None
        };
        // 多智能体开关（ADR-0006）：会话开着开关时装配专家名册。
        let multi_agent_enabled = scheduled_profile.is_none()
            && bridge
                .session_policy(session_id)
                .multi_agent_mode_available()
            && store.mode_state(session_id).multi_agent;
        let workspace = store
            .session_roots(session_id)
            .map(|roots| roots.execution)
            .unwrap_or_else(|_| bridge.session_workspace(session_id));
        let mut engine_config = if multi_agent_enabled {
            // 多智能体面：装配专家名册和专用资源上限；工具面仍与普通会话
            // 完全一致，普通会话不继承这些限制。
            bridge.build_engine_config_for_multi_agent(session_id, workspace)
        } else {
            bridge.build_engine_config_for_session_at(session_id, workspace)
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
        // 该列表来自 compute_disallowed_tools;多智能体会话不改写它——
        // 工具面与主线持平,workflow 与裸 agent 都不在禁用列表。
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
        let dt_config = bridge.build_dt_config();
        // 多智能体宿主不再改写 Workflow 审批配置："每张图必停"的旧约束已按
        // 产品定义收缩撤除，只读图按底座默认自动起跑，写入/提权图由底座的
        // require_approval_for_writes 走普通审批（确认卡只在那时出现）。

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
        let op = self.build_interactive_send_message_op(
            content,
            mode,
            persona_reminder,
            restrict_tools,
        )?;
        self.send_turn_op(op).await
    }

    pub(crate) async fn send_reserved_user_message(
        &self,
        content: String,
        mode: AppMode,
        persona_reminder: Option<String>,
        restrict_tools: bool,
        reservation: TurnReservation,
    ) -> Result<()> {
        let op = self.build_interactive_send_message_op(
            content,
            mode,
            persona_reminder,
            restrict_tools,
        )?;
        self.send_reserved_turn_op(op, reservation).await
    }

    fn build_interactive_send_message_op(
        &self,
        content: String,
        mode: AppMode,
        persona_reminder: Option<String>,
        restrict_tools: bool,
    ) -> Result<Op> {
        if self.multi_agent_enabled {
            self.bridge.build_multi_agent_send_message_op(
                &self.session_id,
                content,
                mode,
                persona_reminder,
                restrict_tools,
                &self.workspace,
            )
        } else {
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
        self.handle
            .send(Op::CompactContext {
                route: Box::new(self.bridge.resolve_runtime_route_for_model(&model)?),
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

/// [edict-obs] per-role token 账本：role_id → (input 累计, output 累计, 调用次数)。
/// 每收到一条 MailboxMessage::TokenUsage 调 add，返回该 role 最新累计快照。
#[derive(Default)]
struct TokenLedger {
    by_role: std::collections::HashMap<String, (u64, u64, u32)>,
}

impl TokenLedger {
    /// 累加一次调用，返回 (input_total, output_total, calls)。
    fn add(&mut self, role: &str, input: u64, output: u64) -> (u64, u64, u32) {
        let e = self.by_role.entry(role.to_string()).or_insert((0, 0, 0));
        e.0 += input;
        e.1 += output;
        e.2 += 1;
        *e
    }
}

/// [per_page] 把某 fan-out 节点的逐页状态(queued/running/done/retrying)推给前端，
/// 让工作流界面把该节点展开成 N 个 SubAgent chip 实时显示并发。
pub(crate) fn emit_fanout(app: &AppHandle, session_id: &str, base_role: &str) {
    let pages = crate::features::assistant::harness::fanout_snapshot(session_id, base_role);
    let _ = app.emit(
        "workflow:fanout",
        json!({
            "session_id": session_id,
            "base_role": base_role,
            "pages": pages,
        }),
    );
}

/// [pinvou3-fork] 执行一个 [`HarnessAction`](crate::features::assistant::harness::HarnessAction)：emit
/// 前端事件，派发真 SubAgent（SpawnAgent → `Op::SpawnSubAgent`）
/// 或等待/收尾（WaitForHuman/AllDone/Blocked）。由 `TurnComplete`（首轮 step_fresh）
/// 和 `AgentComplete`（SubAgent 完成后推进）两条路径共用。返回 `true` = harness
/// 推进了（调用方据此 emit `workflow:full_state` 快照）。
pub(crate) fn emit_workflow_blocked(
    app: &AppHandle,
    session_id: &str,
    workspace: &Path,
    message: &str,
) {
    eprintln!("[harness] blocked: {message}");
    crate::features::assistant::audit::append(
        workspace,
        "blocked",
        "",
        json!({ "message": crate::features::assistant::audit::clip(message) }),
    );
    let warmup_report = serde_json::from_str::<serde_json::Value>(message).ok();
    let display_message = warmup_report
        .as_ref()
        .and_then(crate::features::assistant::harness::warmup_block_reason)
        .unwrap_or_else(|| message.to_string());
    let stage = if warmup_report.is_some() {
        "warmup"
    } else {
        "runtime"
    };
    let _ = app.emit(
        "workflow:blocked",
        json!({
            "session_id": session_id,
            "status": "blocked",
            "stage": stage,
            "message": display_message,
            "warmup_report": warmup_report,
        }),
    );
}

fn tool_call_result_parts(
    result: std::result::Result<
        deepseek_tui::tools::spec::ToolResult,
        deepseek_tui::tools::spec::ToolError,
    >,
) -> (String, bool, Option<serde_json::Value>) {
    match result {
        Ok(result) => (result.content, result.success, result.metadata),
        Err(error) => (format!("{error:?}"), false, None),
    }
}

pub(crate) async fn apply_harness_action(
    action: crate::features::assistant::harness::HarnessAction,
    app: &AppHandle,
    workspace: &Path,
    handle: &EngineHandle,
    active_id: &str,
) -> bool {
    use crate::features::assistant::harness::HarnessAction as HA;
    let ws = workspace.to_path_buf();
    match action {
        HA::SpawnAgent {
            role_id,
            role_name,
            prompt,
            allowed_tools,
            write_files,
            project_dir,
            max_steps,
            output_schema,
            expects_file_output,
        } => {
            eprintln!(
                "[harness] Step C spawn → {role_name} ({role_id}) tools={allowed_tools:?} max_steps={max_steps:?} structured={}",
                output_schema.is_some()
            );
            crate::features::assistant::audit::append(
                &ws,
                "dispatch",
                &role_id,
                json!({ "role_name": &role_name }),
            );
            let _ = app.emit(
                "workflow:agent_state_changed",
                json!({
                    "session_id": active_id, "role_id": role_id,
                    "role_name": role_name, "status": "running",
                }),
            );
            let op = Op::SpawnSubAgent {
                prompt,
                role_id,
                allowed_tools,
                write_files,
                max_steps,
                output_schema,
                structured_output_root: Some(project_dir),
                expects_file_output,
            };
            if let Err(e) = handle.send(op).await {
                eprintln!("[harness] spawn subagent failed: {e:?}");
            }
            true
        }
        // [per_page] 纵向 fan-out：有界并发派发。底座在 running>=max 时硬拒绝(不排队)，
        // 故 Router 运行时自己排队：先派 K 个(per_page_concurrency)，其余留全局队列，由
        // AgentComplete 每页完成补派一个 → 在飞稳定=K。join 计数在 State(record_page_done)；
        // N 实例全到时 AgentComplete handler 对【单一逻辑节点】base_role 验收一次。
        HA::SpawnAgentBatch {
            base_role,
            role_name,
            tasks,
        } => {
            let total = tasks.len();
            let k = crate::features::assistant::harness::per_page_concurrency();
            eprintln!("[harness] Step C fan-out → {role_name} ({base_role}) {total} 页, 在飞并发={k}, 其余排队");
            crate::features::assistant::audit::append(
                &ws,
                "dispatch_batch",
                &base_role,
                json!({ "role_name": &role_name, "pages": total, "concurrency": k }),
            );
            let _ = app.emit(
                "workflow:agent_state_changed",
                json!({
                    "session_id": active_id, "role_id": &base_role,
                    "role_name": role_name, "status": "running",
                }),
            );
            let first = crate::features::assistant::harness::batch_seed_and_take(
                active_id, &base_role, tasks, k,
            );
            for t in first {
                let op = Op::SpawnSubAgent {
                    prompt: t.prompt,
                    role_id: t.agent_role, // "slide_writer#p01" → 回到 AgentComplete.role
                    allowed_tools: t.allowed_tools,
                    write_files: t.write_files,
                    max_steps: t.max_steps,
                    output_schema: t.output_schema,
                    structured_output_root: Some(t.project_dir),
                    expects_file_output: t.expects_file_output,
                };
                if let Err(e) = handle.send(op).await {
                    eprintln!("[harness] fan-out spawn failed: {e:?}");
                }
            }
            emit_fanout(app, active_id, &base_role); // 初始 fan-out 状态 → 前端
            true
        }
        HA::WaitForHuman {
            role_id,
            role_name,
            description,
        } => {
            eprintln!("[harness] waiting for human → {role_name} ({role_id})");
            crate::features::assistant::audit::append(
                &ws,
                "human_gate",
                &role_id,
                json!({ "role_name": &role_name, "description": crate::features::assistant::audit::clip(&description) }),
            );
            let _ = app.emit(
                "workflow:gate_approval",
                json!({
                    "session_id": active_id, "role_id": role_id,
                    "role_name": role_name, "gate_description": description,
                }),
            );
            true
        }
        HA::AllDone => {
            eprintln!("[harness] workflow complete");
            // [edict-obs] 定位最终成品(deck 播放器入口),带进完成事件让前端弹"成品卡"。
            // 找不到(非 deck 类工作流/产物缺失)→ artifact=null,前端只标完成不弹卡。
            let artifact: Option<String> =
                crate::features::assistant::harness::read_full_agent_state(&ws)
                    .and_then(|st| {
                        st.get("project_dir")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                    })
                    .map(|p| {
                        std::path::Path::new(&p)
                            .join("HTML_Deck")
                            .join("index.html")
                    })
                    .filter(|p| p.exists())
                    .map(|p| p.display().to_string());
            crate::features::assistant::audit::append(
                &ws,
                "complete",
                "",
                json!({ "artifact": artifact }),
            );
            let _ = app.emit(
                "workflow:complete",
                json!({ "session_id": active_id, "artifact": artifact }),
            );
            true
        }
        HA::Blocked { message } => {
            eprintln!("[harness] blocked: {message}");
            crate::features::assistant::audit::append(
                &ws,
                "blocked",
                "",
                json!({ "message": crate::features::assistant::audit::clip(&message) }),
            );
            let warmup_report = serde_json::from_str::<serde_json::Value>(&message).ok();
            let _ = app.emit(
                "workflow:blocked",
                json!({
                    "session_id": active_id, "message": message, "warmup_report": warmup_report,
                }),
            );
            true
        }
        HA::Error(e) => {
            eprintln!("[harness] error: {e}");
            false
        }
        HA::NotApplicable => false,
    }
}

/// 后台 task：持续读 rx_event 转 Tauri emit。
///
/// 关键点：监听 `Event::ApprovalRequired` 并主动 `approve_tool_call`。
/// 注意底座语义：`auto_approve = true` 时 `registered_tool_approval_required`
/// 会直接**旁路**可绕过的 Required 审批、不发事件（turn_loop.rs，非绕过名单仅
/// rlm_eval / start_mcp_server）。因此本臂对普通会话只在 auto_approve 关闭的
/// 场景（定时任务按 profile、工作流运行按 `apply_workflow_turn_policy`）收到
/// 事件；工作流编排图交给用户裁定，工作流宿主的其它副作用工具拒绝，普通会话
/// 仍沿用既有批准策略。
///
/// Plan ready 触发：监听 `update_plan` 工具结果 + TurnComplete，
/// 若 active session mode=Plan + 本 turn 调过 update_plan + plan 非空 →
/// 设 phase=Ready + emit `chat:plan_ready` 含 plan snapshot。
///
/// **多引擎并发**:每个 session 一个独立 engine + 一个独立 forwarder,所以这里捕获
/// 的 `session_id` 唯一标识本 forwarder 服务的 session。**所有 emit 的 payload 都带
/// `session_id`**,前端按它把事件分流到对应 session 的缓冲;TurnComplete 里的 mode
/// 判据(plan_ready / M2 / M3)也全部基于本 `session_id`,不再读全局 `store.active_id()`
/// (并发下 active 会变,读全局会把判据算到错误 session 上)。返回 forwarder 的
/// `JoinHandle`,EnginePool 回收 session 时 `abort()` 它。
fn spawn_event_forwarder(
    app: AppHandle,
    handle: EngineHandle,
    store: SessionStore,
    bridge: Pinvou3Bridge,
    execution_workspace: PathBuf,
    session_id: String,
    turn_events: broadcast::Sender<EngineTurnSignal>,
    scheduled_profile: Option<ScheduledRunProfile>,
    scheduled_base_total_tokens: Option<u64>,
    scheduled_unattended: Arc<AtomicBool>,
    turn_lifecycle: Arc<TurnLifecycle>,
    shell_manager: SharedShellManager,
    turn_shell_tasks: TurnShellTaskRegistry,
) -> tauri::async_runtime::JoinHandle<()> {
    let approve_handle = handle.clone();
    let plan_tracker: Arc<Mutex<TurnPlanTracker>> =
        Arc::new(Mutex::new(TurnPlanTracker::default()));
    let shell_output = crate::features::assistant::shell_output::ShellOutputMonitor::spawn(
        app.clone(),
        &shell_manager,
        session_id.clone(),
    );
    tauri::async_runtime::spawn(async move {
        // [B2] 完成账本:agent_id -> 决策 role。dedup——同一 agent_id 重复
        // AgentComplete 时跳过推进(防双推进)。角色重派(gate 失败/回滚)得新
        // agent_id,不会被误跳。
        let mut seen_completions: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        // [edict-obs] agent_id→role_id 关联(由 fork 的 AgentSpawned 第二发喂,
        // 见下方 AgentSpawned 臂注释)+ per-role token 账本。
        // 有意不清理:存活期=本 session forwarder,单 run 最多几十条目(含 fan-out 重试),
        // 跟 seen_completions 同模式 —— 别加"AgentComplete 时清理",会破坏 dedup 语义。
        let mut agent_roles: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut tool_inputs: std::collections::HashMap<String, (String, serde_json::Value)> =
            std::collections::HashMap::new();
        let mut token_ledger = TokenLedger::default();
        let mut turn_tracker = TurnCompletionTracker::default();
        let mut scheduled_engine_total_tokens = 0_u64;
        let mut scheduled_persistence_error: Option<String> = None;
        let mut latest_chat_engine_state: Option<ChatEngineState> = None;
        let mut chat_persistence_error: Option<String> = None;
        let mut active_transcript_seen = false;
        // app 侧自测推理指标累加器(TTFT/生成速度/累计tokens/KV)。try_state:headless
        // harness / 测试可能没 manage MonitorState,拿不到就整块跳过,不 panic。
        let self_metrics = app
            .try_state::<crate::features::monitor::MonitorState>()
            .map(|s| s.self_metrics());
        let mut current_turn_id: Option<String> = None;
        let mut rx = handle.rx_event.write().await;
        while let Some(event) = rx.recv().await {
            match event {
                Event::TurnStarted { turn_id, .. } => {
                    // Publish admission from the authoritative engine event,
                    // before this serial forwarder can observe any delta or
                    // terminal event for the same turn. Reclaim uses the same
                    // emission gate, so it cannot overtake this pair.
                    let admitted =
                        turn_lifecycle.emit_started_admission(&app, &session_id, turn_id.clone());
                    // 消费 pending_cancel（无论 admitted 与否，防止跨轮泄漏）。
                    // reset_cancel_token() 在 TurnStarted 之前已执行，若 cancel
                    // 在此之前 arm 了标记，现在重新 cancel 命中的是本轮活跃 token。
                    //
                    // 仅当 pending_cancel 记录的 epoch 仍是当前轮（emit_started_admission
                    // 刚为该轮推进了 epoch）才取出重放；并发取消请求在旧轮 arm 的
                    // stale pending 会被丢弃，不误取消新轮。
                    let current_epoch = turn_lifecycle.current_turn_generation().unwrap_or(0);
                    let pending_cancel = turn_lifecycle.take_pending_cancel(current_epoch);
                    if !admitted {
                        continue;
                    }
                    if pending_cancel.is_some() {
                        approve_handle.cancel();
                    }
                    active_transcript_seen = false;
                    chat_persistence_error = None;
                    current_turn_id = Some(turn_id.clone());
                    if let Err(error) = turn_shell_tasks.bind_or_prepare_turn(&turn_id).await {
                        log::error!(
                            "[pinvou3][chat] failed to bind shell task scope sid={} turn={}: {error:#}",
                            session_id,
                            turn_id
                        );
                    }
                    let _ = turn_events.send(turn_tracker.on_started(turn_id));
                    // 本轮起始打点(TTFT 起点)。底座已发此事件,原先落 `_` 被忽略。
                    if let Some(m) = &self_metrics {
                        m.on_turn_started(&session_id);
                    }
                }
                Event::MessageDelta { content, .. } => {
                    if let Some(m) = &self_metrics {
                        m.on_message_delta(&session_id, content.chars().count());
                    }
                    crate::features::memory::append_turn_assistant(&session_id, &content);
                    let payload = json!({ "session_id": session_id, "text": content });
                    let _ = app.emit("chat:delta", payload.clone());
                    crate::features::remote_control::forward_app_event(&app, "chat:delta", payload);
                }
                Event::ThinkingStarted { index } => {
                    let payload = json!({ "session_id": session_id, "index": index });
                    let _ = app.emit("chat:reasoning_start", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:reasoning_start",
                        payload,
                    );
                }
                Event::ThinkingDelta { index, content } => {
                    // 只转发底座已经识别为 ThinkingDelta 的独立思考内容。普通
                    // MessageDelta 不做启发式猜测，避免把最终回答误折叠成思考。
                    let payload =
                        json!({ "session_id": session_id, "index": index, "text": content });
                    let _ = app.emit("chat:reasoning_delta", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:reasoning_delta",
                        payload,
                    );
                }
                Event::ThinkingComplete { index } => {
                    let payload = json!({ "session_id": session_id, "index": index });
                    let _ = app.emit("chat:reasoning_done", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:reasoning_done",
                        payload,
                    );
                }
                Event::ToolCallStarted { id, name, input } => {
                    if let Some(m) = &self_metrics {
                        m.on_tool(&session_id); // 本轮有工具 → 收尾跳过 TTFT/TPS(D2)
                    }
                    crate::features::memory::record_turn_tool_start(&session_id, &name, &input);
                    shell_output.tool_started(&id, &name, &input);
                    tool_inputs.insert(id.clone(), (name.clone(), input.clone()));
                    let payload =
                        json!({ "session_id": session_id, "id": id, "name": name, "args": input });
                    let _ = app.emit("chat:tool_start", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:tool_start",
                        payload,
                    );
                }
                Event::ToolCallComplete { id, name, result } => {
                    // 携带 metadata 让前端识别 careful hook 拦截 (safety_level=="dangerous")
                    let (output, success, metadata) = tool_call_result_parts(result);
                    let background_task_id =
                        if matches!(name.as_str(), "exec_shell" | "task_shell_start" | "Bash")
                            && metadata
                                .as_ref()
                                .and_then(|value| value.get("status"))
                                .and_then(serde_json::Value::as_str)
                                == Some("Running")
                        {
                            metadata
                                .as_ref()
                                .and_then(|value| value.get("task_id"))
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned)
                        } else {
                            None
                        };
                    if let (Some(turn_id), Some(task_id)) =
                        (current_turn_id.as_deref(), background_task_id.as_deref())
                    {
                        if !turn_shell_tasks.register_task(turn_id, task_id) {
                            log::warn!(
                                "[pinvou3][chat] shell task has no root-turn scope sid={} turn={} task={}",
                                session_id,
                                turn_id,
                                task_id
                            );
                        }
                    }
                    shell_output.tool_completed(&id, background_task_id.as_deref());
                    let tracked_input = tool_inputs
                        .remove(&id)
                        .map(|(_, input)| input)
                        .unwrap_or(serde_json::Value::Null);
                    if success {
                        if let Some(profile) = scheduled_profile.as_ref() {
                            let artifact_store = store.clone();
                            let artifact_session_id = session_id.clone();
                            let artifact_workspace = profile.workspace.clone();
                            let artifact_name = name.clone();
                            let artifact_input = tracked_input.clone();
                            let artifact_output = output.clone();
                            match tokio::task::spawn_blocking(move || {
                                persist_successful_tool_artifact(
                                    &artifact_store,
                                    &artifact_session_id,
                                    &artifact_workspace,
                                    &artifact_name,
                                    &artifact_input,
                                    &artifact_output,
                                )
                            })
                            .await
                            {
                                Ok(Ok(_)) => {}
                                Ok(Err(error)) => eprintln!(
                                    "[pinvou3-app] scheduled artifact persistence skipped: {error:#}"
                                ),
                                Err(error) => eprintln!(
                                    "[pinvou3-app] scheduled artifact persistence task failed: {error}"
                                ),
                            }
                        }
                    }
                    crate::features::memory::record_turn_tool_complete(
                        &session_id,
                        &name,
                        &tracked_input,
                        success,
                    );
                    // Plan 类工具结果：标记 + 缓存 snapshot（两层）+ 实时 emit 给前端 chip 进度区
                    if success
                        && (name == "update_plan"
                            || name == "checklist_write"
                            || name == "todo_write")
                    {
                        let mut tracker = plan_tracker.lock();
                        tracker.plan_tool_used = true;
                        // 上游格式："Plan/Todo ... updated: ...\n{json}"——切第一个 '\n' 后是 json
                        if let Some(json_part) = output.find('\n').map(|i| &output[i + 1..]) {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_part) {
                                if name == "update_plan" {
                                    tracker.last_plan_snapshot = Some(v);
                                } else {
                                    tracker.last_todos_snapshot = Some(v);
                                }
                            }
                        }
                        // emit chat:plan_snapshot 实时更新 chip 进度——跟 plan_ready 解耦,
                        // 后者是状态转移信号(Planning→Ready 弹卡片),这个是数据更新信号(刷新 chip)。
                        // **只 emit 本次工具改的那个 snapshot**,另一个置 null——这样前端只更新对应
                        // 的时间戳,pickProgressItems 才能正确按时间挑最新的(否则两个 ts 总相等)。
                        let (plan_emit, todos_emit) = if name == "update_plan" {
                            (tracker.last_plan_snapshot.clone(), None)
                        } else {
                            (None, tracker.last_todos_snapshot.clone())
                        };
                        drop(tracker);
                        let payload = json!({
                            "session_id": session_id,
                            "plan_snapshot": plan_emit,
                            "todos_snapshot": todos_emit,
                        });
                        let _ = app.emit("chat:plan_snapshot", payload.clone());
                        crate::features::remote_control::forward_app_event(
                            &app,
                            "chat:plan_snapshot",
                            payload,
                        );
                    }
                    let payload = json!({
                        "session_id": session_id,
                        "id": id,
                        "name": name,
                        "output": output,
                        "success": success,
                        "metadata": metadata,
                    });
                    // request_user_input 收口：submit/cancel 已由底座转为工具结果。
                    if name == "request_user_input" {
                        crate::features::assistant::pending_user_input::clear(&session_id, &id);
                    }
                    let _ = app.emit("chat:tool_end", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:tool_end",
                        payload,
                    );
                }
                Event::UserInputRequired { id, request } => {
                    if scheduled_profile.is_some() && scheduled_unattended.load(Ordering::Acquire) {
                        // 定时任务没有前台操作者；不能把底座卡在等待输入的超时上。
                        let h = approve_handle.clone();
                        tokio::spawn(async move {
                            if let Err(error) = h.cancel_user_input(id).await {
                                eprintln!(
                                    "[pinvou3-app] cancel scheduled user input failed: {error:?}"
                                );
                            }
                        });
                    } else {
                        // 普通对话继续交给前端或远程控制端处理。登记 pending，
                        // 供前端 remount 后经 get_pending_user_inputs 恢复确认卡。
                        let payload = json!({
                            "session_id": session_id,
                            "id": id,
                            "questions": request.questions,
                        });
                        crate::features::assistant::pending_user_input::record(
                            &session_id,
                            &id,
                            payload["questions"].clone(),
                        );
                        let _ = app.emit("chat:user_input_required", payload.clone());
                        crate::features::remote_control::forward_app_event(
                            &app,
                            "chat:user_input_required",
                            payload,
                        );
                    }
                }
                Event::SessionUpdated {
                    session_id: event_session_id,
                    messages,
                    system_prompt,
                    model,
                    workspace,
                } => {
                    if event_session_id != session_id {
                        let mismatch = format!(
                            "Engine session id mismatch: expected {session_id}, got {event_session_id}"
                        );
                        if scheduled_profile.is_some() {
                            scheduled_persistence_error = Some(mismatch);
                        } else {
                            chat_persistence_error = Some(mismatch);
                        }
                        continue;
                    }
                    if let Some(profile) = scheduled_profile.as_ref() {
                        let state = ScheduledEngineState {
                            messages,
                            system_prompt,
                            model,
                            workspace,
                            mode: profile.execution_mode(),
                            token_accounting: ScheduledTokenAccounting::PreservePersisted,
                        };
                        let store_for_save = store.clone();
                        let session_for_save = session_id.clone();
                        match tokio::task::spawn_blocking(move || {
                            store_for_save.persist_scheduled_engine_state(&session_for_save, state)
                        })
                        .await
                        {
                            Ok(Ok(_)) => scheduled_persistence_error = None,
                            Ok(Err(error)) => {
                                scheduled_persistence_error = Some(format!("{error:#}"));
                            }
                            Err(error) => {
                                scheduled_persistence_error = Some(format!(
                                    "scheduled transcript persistence task failed: {error}"
                                ));
                            }
                        }
                    } else {
                        let (messages, matched_active_rule) =
                            turn_lifecycle.sanitize_messages(messages);
                        active_transcript_seen |= matched_active_rule;
                        let state = ChatEngineState {
                            messages,
                            system_prompt,
                            model,
                            workspace,
                        };
                        latest_chat_engine_state = Some(state.clone());
                        let store_for_save = store.clone();
                        let session_for_save = session_id.clone();
                        match tokio::task::spawn_blocking(move || {
                            store_for_save.persist_chat_engine_state(&session_for_save, state)
                        })
                        .await
                        {
                            Ok(Ok(saved)) => {
                                chat_persistence_error = None;
                                if let Ok(revision) = transcript_revision(&saved.messages) {
                                    let payload = json!({
                                        "session_id": session_id,
                                        "transcript_revision": revision,
                                    });
                                    let _ = app.emit("chat:transcript_committed", payload.clone());
                                    crate::features::remote_control::forward_app_event(
                                        &app,
                                        "chat:transcript_committed",
                                        payload,
                                    );
                                }
                            }
                            Ok(Err(error)) => {
                                chat_persistence_error = Some(format!("{error:#}"));
                            }
                            Err(error) => {
                                chat_persistence_error = Some(format!(
                                    "chat transcript persistence task failed: {error}"
                                ));
                            }
                        }
                    }
                }
                Event::ApprovalRequired {
                    id,
                    tool_name,
                    approval_force_prompt,
                    ..
                } => {
                    // 多智能体会话与普通对话共用同一套审批语义（ADR-0006）：
                    // 只有无人值守的初始定时执行使用任务保存的批准策略；
                    // 其余会话按普通对话处理。
                    let should_approve = if scheduled_profile.is_some()
                        && scheduled_unattended.load(Ordering::Acquire)
                    {
                        scheduled_tool_should_auto_approve(
                            scheduled_profile.as_ref(),
                            approval_force_prompt,
                        )
                    } else {
                        true
                    };
                    eprintln!(
                        "[pinvou3-app] {} tool {} id={}",
                        if should_approve {
                            "auto-approving"
                        } else {
                            "denying"
                        },
                        tool_name,
                        id
                    );
                    let h = approve_handle.clone();
                    let id_clone = id.clone();
                    tokio::spawn(async move {
                        let result = if should_approve {
                            h.approve_tool_call(id_clone).await
                        } else {
                            h.deny_tool_call(id_clone).await
                        };
                        if let Err(e) = result {
                            eprintln!("[pinvou3-app] scheduled approval decision failed: {e:?}");
                        }
                    });
                    // 不重复 emit chat:tool_start —— 上游 ToolCallStarted（带完整 input）
                    // 已先于 ApprovalRequired fire，前端已收到正确的 args。
                    // 之前在此 emit 会用 args=null 覆盖前端 toolMeta，导致产物路径丢失。
                }
                // [edict-obs] 同一 agent_id 会收到两发 AgentSpawned:第一发来自
                // subagent manager 内部(prompt=任务文本,subagent/mod.rs:1518),
                // 第二发来自 fork 的 Op::SpawnSubAgent 成功臂(prompt=role_id)。
                // FIFO 保证第二发后到 → HashMap::insert last-write-wins,最终值
                // 必为 role_id。这是有意设计,别"去重优化"。
                // v0.8.65 上游给 AgentSpawned 加 parent_run_id/spawn_depth(谱系遥测);
                // pinvou3 forwarder 只用 id→role(prompt)关联,新字段 `..` 忽略。
                Event::AgentSpawned { id, prompt, .. } => {
                    agent_roles.insert(id, prompt);
                }
                // 台账退役（ADR-0006）：workflow 内部生命周期不再转发，子智能体
                // 状态一律走 AgentProgress / AgentComplete 与落盘 worker ledger。
                Event::WorkflowUi { .. } => {}
                // [edict-obs] SubAgent 每步进展(底座自动发,不靠 prompt 纪律)→ 前端看板。
                Event::AgentProgress { id, status, .. } => {
                    // 早期事件可能赶在第二发 AgentSpawned(role_id)之前到 —— 兜底用
                    // agent_id 而不是空串,跟 TokenUsage 臂的 fallback 语义一致。
                    let role = agent_roles.get(&id).cloned().unwrap_or_else(|| id.clone());
                    let payload = json!({
                        "session_id": session_id,
                        "agent_id": id,
                        "role_id": role,
                        "status": status,
                    });
                    let _ = app.emit("workflow:agent_progress", payload);
                }
                // [edict-obs] mailbox 信封:工具调用→progress;TokenUsage→账本+审计;
                // Completed/Failed→审计。审计写盘是同步 IO,但单行 append 微秒级,
                // 不值得为它 spawn_blocking(forwarder 本身不在 LLM 关键路径上)。
                Event::SubAgentMailbox { message, .. } => {
                    use deepseek_tui::tools::subagent::MailboxMessage as MM;
                    // 审计是应用账本：绑了项目目录的原生代码会话写会话私有目录，
                    // 不污染用户项目；其余会话账本根与执行根相同，行为不变。
                    let ws = bridge.audit_workspace(&session_id, &execution_workspace);
                    match message {
                        MM::Started { agent_id, .. } => {
                            if let Some(turn_id) = current_turn_id.as_deref() {
                                turn_shell_tasks.register_agent(turn_id, &agent_id);
                            }
                        }
                        MM::ChildSpawned {
                            parent_id,
                            child_id,
                        } => {
                            if !turn_shell_tasks.register_child_agent(&parent_id, &child_id) {
                                log::warn!(
                                    "[pinvou3][chat] child agent has no parent shell scope sid={} parent={} child={}",
                                    session_id,
                                    parent_id,
                                    child_id
                                );
                            }
                        }
                        MM::TokenUsage {
                            agent_id,
                            route,
                            usage,
                            ..
                        } => {
                            let role = agent_roles
                                .get(&agent_id)
                                .cloned()
                                .unwrap_or_else(|| agent_id.clone());
                            let (input_total, output_total, calls) = token_ledger.add(
                                &role,
                                usage.input_tokens as u64,
                                usage.output_tokens as u64,
                            );
                            let _ = app.emit(
                                "workflow:token_usage",
                                json!({
                                    "session_id": session_id,
                                    "role_id": role,
                                    "agent_id": agent_id,
                                    "model": route.model,
                                    "input_tokens_total": input_total,
                                    "output_tokens_total": output_total,
                                    "calls": calls,
                                }),
                            );
                            crate::features::assistant::audit::append(
                                &ws,
                                "token",
                                &role,
                                json!({
                                    "agent_id": agent_id,
                                    "input": usage.input_tokens,
                                    "output": usage.output_tokens,
                                }),
                            );
                        }
                        MM::ToolCallStarted {
                            agent_id,
                            tool_name,
                            step,
                        } => {
                            // 兜底语义跟 AgentProgress 臂一致(agent_id 而非空串)
                            let role = agent_roles
                                .get(&agent_id)
                                .cloned()
                                .unwrap_or_else(|| agent_id.clone());
                            let payload = json!({
                                "session_id": session_id,
                                "agent_id": agent_id,
                                "role_id": role,
                                "status": format!("🔧 {tool_name} (step {step})"),
                            });
                            let _ = app.emit("workflow:agent_progress", payload);
                        }
                        MM::Completed { agent_id, summary } => {
                            turn_shell_tasks.complete_agent(&agent_id);
                            let role = agent_roles.get(&agent_id).cloned().unwrap_or_default();
                            crate::features::assistant::audit::append(
                                &ws,
                                "agent_done",
                                &role,
                                json!({
                                    "agent_id": agent_id,
                                    "summary": crate::features::assistant::audit::clip(&summary),
                                }),
                            );
                        }
                        MM::Failed { agent_id, error } => {
                            turn_shell_tasks.complete_agent(&agent_id);
                            let role = agent_roles.get(&agent_id).cloned().unwrap_or_default();
                            crate::features::assistant::audit::append(
                                &ws,
                                "agent_failed",
                                &role,
                                json!({
                                    "agent_id": agent_id,
                                    "error": crate::features::assistant::audit::clip(&error),
                                }),
                            );
                        }
                        MM::Cancelled { agent_id } => {
                            turn_shell_tasks.complete_agent(&agent_id);
                        }
                        MM::Interrupted { agent_id, .. } => {
                            // CodeWhale 当前不保留可原位恢复的 live task；中断后只能基于
                            // checkpoint 重新派发，因此它对本轮 Shell scope 同样是终态。
                            turn_shell_tasks.complete_agent(&agent_id);
                        }
                        _ => {}
                    }
                }
                // [pinvou3-fork] 真 SubAgent(Step C)完成 → Step D Gate。executing 期间
                // 主 session 不发 TurnComplete(它空闲,subagent 在 subagent_manager 跑),
                // 所以单角色周期的"完成"信号在这里、不在 TurnComplete。
                Event::AgentComplete {
                    id,
                    result,
                    role: envelope_role,
                    failed,
                } => {
                    // 用户停止整个工作流后，SubAgent 可能在任务 abort 前抢先送达
                    // 一个完成信封。持久 stop marker 是调度熔断边界：迟到结果可留在
                    // 项目目录，但绝不能再过 gate、补派下一页或推进下一个角色。
                    if crate::features::assistant::harness::workflow_is_stopped(
                        &execution_workspace,
                    ) {
                        eprintln!("[harness] ignore late AgentComplete after workflow stop: {id}");
                        continue;
                    }
                    eprintln!(
                        "[harness] subagent {id} complete ({} chars summary, failed={failed})",
                        result.len()
                    );
                    // role/failed 是新增字段:老的工作流看板只认 agent_id,多带两个字段
                    // 不影响它;多智能体工位看板要靠这两个判断"谁完成了、成没成功",
                    // 缺了就只能显示一个没有归属的完成信号。
                    let payload = json!({
                        "session_id": session_id,
                        "agent_id": id,
                        "role_id": envelope_role
                            .clone()
                            .or_else(|| agent_roles.get(&id).cloned()),
                        "failed": failed,
                    });
                    let _ = app.emit("workflow:agent_complete", payload);
                    let state = store.mode_state(&session_id);
                    if state.active_skill.is_some() {
                        // [C2] 完成→节点关联完全用信封 role（SDAN Result.from）。
                        // harness_phase 已删,无 phase 兜底——正常 workflow subagent
                        // 派发必带 role(SubAgentAssignment.role)。
                        let decision_role: Option<String> = envelope_role.clone();
                        if decision_role.is_none() {
                            eprintln!("[harness] ⚠️AgentComplete 无 role(非 workflow subagent?) id={id},不推进");
                        }
                        // dedup:同一 agent_id 重复完成 → 跳过,防双推进。
                        // 角色重派(gate 失败/回滚)会得新 agent_id,不会被误跳。
                        let is_dup = seen_completions
                            .insert(id.clone(), decision_role.clone().unwrap_or_default())
                            .is_some();
                        if is_dup {
                            eprintln!("[harness] 重复 AgentComplete id={id},跳过");
                        } else if let Some(role) = decision_role {
                            // [per_page] role 形如 "slide_writer#p01" = fan-out 成员：
                            // 记一页 join 计数，未齐则等其余页（不推进）；齐了才对【单一
                            // 逻辑节点】base_role 验收一次。普通角色 base = role 本身。
                            let base_for_step: Option<String> = if let Some((base, page_str)) =
                                role.split_once('#')
                            {
                                let page: u32 =
                                    page_str.trim_start_matches('p').parse().unwrap_or(0);
                                let ws = execution_workspace.clone();
                                // [per_page] 先校验该页【真写成】：SSE 超时/放弃的 agent 也
                                // emit AgentComplete，但只留空壳骨架。空壳不计 done，自动重派
                                // 该页(带上限)，挡住空壳混入 batch → 避免 gate pagenum_mismatch
                                // 误判回滚的死循环。
                                let outs = crate::features::assistant::harness::batch_outputs_for(
                                    &session_id,
                                    base,
                                    page,
                                );
                                let real = crate::features::assistant::harness::page_output_is_real(
                                    &ws, base, page, &outs,
                                );
                                let mut count_done = real;
                                if real {
                                    eprintln!("[harness] per_page {role} 真写成 ✓");
                                    crate::features::assistant::harness::fanout_mark(
                                        &session_id,
                                        base,
                                        page,
                                        "done",
                                    );
                                } else {
                                    let n = crate::features::assistant::harness::page_retry_inc(
                                        &session_id,
                                        base,
                                        page,
                                    );
                                    let maxr =
                                        crate::features::assistant::harness::max_page_retry();
                                    if n <= maxr {
                                        // 空壳 → 重派该页(占用刚释放的在飞名额，不补排队页)。
                                        let ws_r = ws.clone();
                                        let base_r = base.to_string();
                                        let respawn = tokio::task::spawn_blocking(move || {
                                            crate::features::assistant::harness::respawn_page(
                                                &ws_r, &base_r, page,
                                            )
                                        })
                                        .await
                                        .unwrap_or(None);
                                        if let Some(t) = respawn {
                                            let rr = t.agent_role.clone();
                                            let op = Op::SpawnSubAgent {
                                                prompt: t.prompt,
                                                role_id: t.agent_role,
                                                allowed_tools: t.allowed_tools,
                                                write_files: t.write_files,
                                                max_steps: t.max_steps,
                                                output_schema: t.output_schema,
                                                structured_output_root: Some(t.project_dir),
                                                expects_file_output: t.expects_file_output,
                                            };
                                            if let Err(e) = approve_handle.send(op).await {
                                                eprintln!("[harness] per_page {role} 空壳→重派 {rr} 失败: {e:?}");
                                            } else {
                                                eprintln!("[harness] per_page {role} 空壳→重派(第{n}/{maxr}次) {rr}");
                                                crate::features::assistant::harness::fanout_mark(
                                                    &session_id,
                                                    base,
                                                    page,
                                                    "retrying",
                                                );
                                            }
                                        } else {
                                            eprintln!("[harness] per_page {role} 空壳但 respawn 取不到任务 → 兜底计 done");
                                            count_done = true;
                                            crate::features::assistant::harness::fanout_mark(
                                                &session_id,
                                                base,
                                                page,
                                                "done",
                                            );
                                        }
                                    } else {
                                        eprintln!("[harness] per_page {role} 空壳且重试耗尽({maxr}) → 兜底计 done(留给 gate/人工)");
                                        count_done = true;
                                        crate::features::assistant::harness::fanout_mark(
                                            &session_id,
                                            base,
                                            page,
                                            "done",
                                        );
                                    }
                                }
                                // 每页完成/重试后把最新 fan-out 状态推给前端工作流界面。
                                emit_fanout(&app, &session_id, base);
                                if count_done {
                                    let ws_d = ws.clone();
                                    let base_d = base.to_string();
                                    let complete = tokio::task::spawn_blocking(move || {
                                        crate::features::assistant::harness::record_page_done(
                                            &ws_d, &base_d, page,
                                        )
                                    })
                                    .await
                                    .unwrap_or(true);
                                    eprintln!(
                                        "[harness] per_page {role} done; batch_complete={complete}"
                                    );
                                    if complete {
                                        crate::features::assistant::harness::batch_clear(
                                            &session_id,
                                            base,
                                        );
                                        Some(base.to_string())
                                    } else {
                                        // 未齐 → 补派下一排队页，维持在飞并发=K；不推进。
                                        if let Some(t) =
                                            crate::features::assistant::harness::batch_pop_next(
                                                &session_id,
                                                base,
                                            )
                                        {
                                            let next_role = t.agent_role.clone();
                                            let op = Op::SpawnSubAgent {
                                                prompt: t.prompt,
                                                role_id: t.agent_role,
                                                allowed_tools: t.allowed_tools,
                                                write_files: t.write_files,
                                                max_steps: t.max_steps,
                                                output_schema: t.output_schema,
                                                structured_output_root: Some(t.project_dir),
                                                expects_file_output: t.expects_file_output,
                                            };
                                            if let Err(e) = approve_handle.send(op).await {
                                                eprintln!("[harness] per_page 补派 {next_role} 失败: {e:?}");
                                            } else {
                                                eprintln!(
                                                    "[harness] per_page 补派下一页 {next_role}"
                                                );
                                            }
                                        }
                                        None
                                    }
                                } else {
                                    // 重派路径：等该页重试结果，不推进、不补排队页。
                                    None
                                }
                            } else {
                                Some(role.clone())
                            };

                            if let Some(base_role) = base_for_step {
                                let ws = execution_workspace.clone();
                                // [2026-06-06] SubAgent 执行失败(failed=true,单角色)绝不走 gate：
                                // gate 只验产物存在+非空，会拿【上一轮陈旧产物】把失败洗成 PASS
                                // (实锤:web_search 不可用→PM 0步即死→旧 brief 过关)。改走
                                // agent_failed:--fail 计次→重派带失败原因，耗尽→Blocked。
                                // per_page 成员(role 带 #)不走这里:空壳检测已按页处理。
                                let failed_single = failed && !role.contains('#');
                                let err_text = result.clone();
                                let action = tokio::task::spawn_blocking(move || {
                                    if failed_single {
                                        crate::features::assistant::harness::agent_failed(
                                            &ws, &base_role, &err_text,
                                        )
                                    } else {
                                        crate::features::assistant::harness::step_after_role(
                                            &ws, &base_role,
                                        )
                                    }
                                })
                                .await
                                .unwrap_or_else(|_| {
                                    crate::features::assistant::harness::HarnessAction::Error(
                                        "spawn_blocking panicked".into(),
                                    )
                                });
                                let handled = apply_harness_action(
                                    action,
                                    &app,
                                    &execution_workspace,
                                    &approve_handle,
                                    &session_id,
                                )
                                .await;
                                if handled {
                                    let ws = execution_workspace.clone();
                                    let app_clone = app.clone();
                                    let sid_clone = session_id.clone();
                                    tokio::task::spawn_blocking(move || {
                                        if let Some(mut st) =
                                            crate::features::assistant::harness::read_full_agent_state(&ws)
                                        {
                                            if let Some(obj) = st.as_object_mut() {
                                                obj.insert("session_id".into(), json!(sid_clone));
                                            }
                                            let _ = app_clone.emit("workflow:full_state", st);
                                        }
                                    });
                                }
                            }
                        }
                    }
                }
                Event::TurnComplete {
                    usage,
                    status,
                    error,
                    // v0.8.49 上游新增 tool_catalog / base_url(调试/审计用),pinvou3 不消费
                    ..
                } => {
                    let shell_turn_id = current_turn_id.clone();
                    let shell_interrupted = status == TurnOutcomeStatus::Interrupted;
                    let mut shell_cleanup_failed = false;
                    let mut terminal_status = status;
                    let mut terminal_error = error;
                    if let Some(base_total_tokens) = scheduled_base_total_tokens {
                        scheduled_engine_total_tokens = scheduled_engine_total_tokens
                            .saturating_add(u64::from(usage.input_tokens))
                            .saturating_add(u64::from(usage.output_tokens));
                        let store_for_save = store.clone();
                        let session_for_save = session_id.clone();
                        match tokio::task::spawn_blocking(move || {
                            store_for_save.persist_scheduled_token_total(
                                &session_for_save,
                                base_total_tokens,
                                scheduled_engine_total_tokens,
                            )
                        })
                        .await
                        {
                            Ok(Ok(_)) => {}
                            Ok(Err(save_error)) => {
                                scheduled_persistence_error = Some(format!("{save_error:#}"));
                            }
                            Err(join_error) => {
                                scheduled_persistence_error = Some(format!(
                                    "scheduled token persistence task failed: {join_error}"
                                ));
                            }
                        }
                        if let Some(save_error) = scheduled_persistence_error.take() {
                            terminal_status = TurnOutcomeStatus::Failed;
                            terminal_error = Some(match terminal_error {
                                Some(engine_error) => format!(
                                    "{engine_error}; scheduled conversation persistence failed: {save_error}"
                                ),
                                None => format!(
                                    "Scheduled conversation persistence failed: {save_error}"
                                ),
                            });
                        }
                    } else {
                        let terminal_save = if active_transcript_seen {
                            latest_chat_engine_state.clone().map(|state| {
                                let store_for_save = store.clone();
                                let session_for_save = session_id.clone();
                                tokio::task::spawn_blocking(move || {
                                    store_for_save
                                        .persist_chat_engine_state(&session_for_save, state)
                                })
                            })
                        } else {
                            turn_lifecycle.active_transcript_fallback().map(|fallback| {
                                let store_for_save = store.clone();
                                let session_for_save = session_id.clone();
                                tokio::task::spawn_blocking(move || {
                                    store_for_save.persist_admitted_chat_display(
                                        &session_for_save,
                                        &fallback.baseline_revision,
                                        fallback.display_message,
                                        fallback.operation == TranscriptOperation::EditLast,
                                    )
                                })
                            })
                        };
                        if let Some(save_task) = terminal_save {
                            match save_task.await {
                                Ok(Ok(saved)) => {
                                    chat_persistence_error = None;
                                    if let Ok(revision) = transcript_revision(&saved.messages) {
                                        let payload = json!({
                                            "session_id": session_id,
                                            "transcript_revision": revision,
                                        });
                                        let _ =
                                            app.emit("chat:transcript_committed", payload.clone());
                                        crate::features::remote_control::forward_app_event(
                                            &app,
                                            "chat:transcript_committed",
                                            payload,
                                        );
                                    }
                                }
                                Ok(Err(save_error)) => {
                                    chat_persistence_error = Some(format!("{save_error:#}"));
                                }
                                Err(join_error) => {
                                    chat_persistence_error = Some(format!(
                                        "chat transcript persistence task failed: {join_error}"
                                    ));
                                }
                            }
                        }
                        if let Some(save_error) = chat_persistence_error.take() {
                            terminal_status = TurnOutcomeStatus::Failed;
                            terminal_error = Some(match terminal_error {
                                Some(engine_error) => format!(
                                    "{engine_error}; chat conversation persistence failed: {save_error}"
                                ),
                                None => format!(
                                    "Chat conversation persistence failed: {save_error}"
                                ),
                            });
                        }
                    }
                    // Keep the shell scope cancellable throughout persistence.
                    // Final cleanup runs before terminal admission is claimed;
                    // Engine reclaim can therefore still win an await race and
                    // close the same scope without leaving the UI permanently
                    // busy. The cleanup itself executes on blocking workers.
                    if let Some(turn_id) = shell_turn_id.as_deref() {
                        match turn_shell_tasks
                            .finalize_turn(turn_id, shell_interrupted)
                            .await
                        {
                            Ok(report) => {
                                if !report.killed.is_empty() {
                                    log::info!(
                                        "[pinvou3][chat] finalized {} interrupted shell task(s) sid={} turn={}",
                                        report.killed.len(),
                                        session_id,
                                        turn_id
                                    );
                                }
                                if let Some(cleanup_error) = report.failure_summary() {
                                    shell_cleanup_failed = true;
                                    log::error!(
                                        "[pinvou3][chat] shell cleanup remained incomplete sid={} turn={}: {}",
                                        session_id,
                                        turn_id,
                                        cleanup_error
                                    );
                                }
                            }
                            Err(error) => {
                                log::error!(
                                    "[pinvou3][chat] failed to finalize shell task scope sid={} turn={}: {error:#}",
                                    session_id,
                                    turn_id
                                );
                                shell_cleanup_failed = true;
                            }
                        }
                    }
                    // Persistence above can change the authoritative outcome and
                    // may await. Claim only after that result is known, but before
                    // every completion-only side effect below. If reclaim won
                    // during the await, this late TurnComplete is discarded.
                    let Some(_) = turn_lifecycle.claim_terminal_with_admission(&app, &session_id)
                    else {
                        continue;
                    };
                    // 单独发 usage 给前端 token 进度条
                    let payload = json!({
                        "session_id": session_id,
                        "input_tokens": usage.input_tokens,
                        "output_tokens": usage.output_tokens,
                    });
                    let _ = app.emit("chat:usage", payload.clone());
                    crate::features::remote_control::forward_app_event(&app, "chat:usage", payload);
                    // app 侧自测:用精确 usage 收尾本轮。部分远端模型会流式返回文本,
                    // 但最终 usage.output_tokens 为 0,因此不能用 output_tokens 门控收尾。
                    if let Some(m) = &self_metrics {
                        m.on_turn_complete(
                            &session_id,
                            usage.input_tokens,
                            usage.output_tokens,
                            usage.prompt_cache_hit_tokens,
                            usage.prompt_cache_miss_tokens,
                        );
                    }
                    // turn end:取出 tracker 快照,然后重置(下个 turn 重新累积)。
                    // 用独立 block 把 parking_lot guard 的生命周期限死在这里:下方
                    // H1 harness 段有 .await(spawn_blocking),guard 是 !Send,若跨
                    // await 会让整个 forwarder future 变 !Send(tauri::spawn 要求 Send)。
                    let (plan_used, plan_snapshot, todos_snapshot) = {
                        let mut tracker = plan_tracker.lock();
                        let plan_used = tracker.plan_tool_used;
                        let plan_snapshot = tracker.last_plan_snapshot.take();
                        let todos_snapshot = tracker.last_todos_snapshot.take();
                        *tracker = TurnPlanTracker::default();
                        (plan_used, plan_snapshot, todos_snapshot)
                    };

                    // 先完成权威时间线，再发 chat:done。前端终态事件会据此重新
                    // 读取整份时间线，补齐后台运行/页面恢复期间漏掉的本地事件。
                    let status_text = format!("{terminal_status:?}");
                    crate::features::assistant::timing::finish_turn_with_usage(
                        &session_id,
                        &status_text,
                        terminal_error.as_deref(),
                        Some(crate::features::assistant::timing::TurnUsage {
                            input_tokens: u64::from(usage.input_tokens),
                            output_tokens: u64::from(usage.output_tokens),
                            cache_hit_tokens: u64::from(usage.prompt_cache_hit_tokens.unwrap_or(0)),
                            cache_miss_tokens: u64::from(
                                usage.prompt_cache_miss_tokens.unwrap_or(0),
                            ),
                            cache_write_tokens: u64::from(
                                usage.prompt_cache_write_tokens.unwrap_or(0),
                            ),
                            reasoning_tokens: u64::from(usage.reasoning_tokens.unwrap_or(0)),
                        }),
                    );

                    {
                        // 多引擎:mode 判据基于本 forwarder 的 session_id,不读全局 active。
                        let active_id = session_id.clone();
                        let state = store.mode_state(&active_id);

                        // ── plan_ready 触发(Plan + 调过 plan 类工具) ──
                        // 底座式:Plan 模式调过 update_plan = 出方案 → 弹决策卡。不再设 phase
                        // (已砍 PlanPhase),mode 留 Plan 直到用户 accept/discard 切 Yolo。
                        // 修订(还在 Plan 时再调 update_plan)天然幂等:再弹一张新卡。
                        if plan_used && state.mode == SerializableMode::Plan {
                            if let Some(plan_id) = current_turn_id.clone() {
                                if let Some(mode_state) =
                                    store.register_pending_plan(&active_id, plan_id.clone())
                                {
                                    let payload = json!({
                                        "session_id": active_id.clone(),
                                        "plan_id": plan_id,
                                        "plan_snapshot": plan_snapshot.clone(),
                                        "todos_snapshot": todos_snapshot.clone(),
                                        "mode_state": mode_state,
                                    });
                                    let _ = app.emit("chat:plan_ready", payload.clone());
                                    crate::features::remote_control::forward_app_event(
                                        &app,
                                        "chat:plan_ready",
                                        payload,
                                    );
                                }
                            }
                        }

                        // From here the harness path may await. Publish the claimed
                        // terminal first so a concurrent Engine eviction cannot
                        // abort this forwarder and leave the frontend permanently
                        // busy. Reclaim now observes the lifecycle as terminal and
                        // cannot publish a conflicting Interrupted outcome.
                        emit_chat_terminal(
                            &app,
                            &session_id,
                            terminal_status,
                            terminal_error.clone(),
                            shell_cleanup_failed,
                        );
                        turn_lifecycle.finish_terminal_emission();

                        // ── H1: Harness Loop (skill session 图执行器) ──
                        // 本 session 绑定了 skill 且项目目录有 workflow_progress.json →
                        // harness 图执行器始终使用 engine 的实际执行工作区：普通会话是
                        // session 私有目录，定时会话是所属 automation 的专属工作间。
                        let harness_workspace = execution_workspace.clone();
                        let harness_handled = if state.active_skill.is_some() {
                            // [C2] harness_phase 已删。工作流由 kick(命令)+ AgentComplete
                            // 驱动;主 session 在工作流期间空闲,此处 TurnComplete 一般不参与
                            // 推进。仍兜底走 step_fresh:由 scheduler 据 State 决策——有角色在
                            // 跑则返回 role_running→NotApplicable(防重复派发),否则派下一个。
                            let ws = harness_workspace.clone();
                            let action = tokio::task::spawn_blocking(move || {
                                crate::features::assistant::harness::step_fresh(&ws)
                            })
                            .await
                            .unwrap_or(
                                crate::features::assistant::harness::HarnessAction::Error(
                                    "spawn_blocking panicked".into(),
                                ),
                            );

                            apply_harness_action(
                                action,
                                &app,
                                &execution_workspace,
                                &approve_handle,
                                &active_id,
                            )
                            .await
                        } else {
                            false
                        };

                        // ── H1b: harness 推进了 → 推送全量 agent 状态快照给前端 ──
                        if harness_handled {
                            let ws = harness_workspace.clone();
                            let app_clone = app.clone();
                            let sid_clone = active_id.clone();
                            tokio::task::spawn_blocking(move || {
                                if let Some(mut state) =
                                    crate::features::assistant::harness::read_full_agent_state(&ws)
                                {
                                    if let Some(obj) = state.as_object_mut() {
                                        obj.insert("session_id".into(), json!(sid_clone));
                                    }
                                    let _ = app_clone.emit("workflow:full_state", state);
                                }
                            });
                        }

                        // ── [回归底座式] M2 自驱 + M3 文本兜底已彻底砍掉 ──
                        // M2:执行不自动续跑,回底座由用户驱动。M3(Plan 写了方案没调
                        // update_plan 的救援)放弃不做:底座 update_plan→plan_ready→方案卡
                        // 这条链已可靠,漏的少数"光说不出卡"由 composer chip 手切 + plan_stuck
                        // 卡兜底,不值得用噪音判据再造一层。
                    }
                    if crate::features::memory::memory_enabled() {
                        if let Some(capture) =
                            crate::features::memory::take_turn_capture(&session_id)
                        {
                            let app_clone = app.clone();
                            let bridge_clone = bridge.clone();
                            let sid_clone = session_id.clone();
                            tauri::async_runtime::spawn(async move {
                                match crate::features::memory::review_turn_candidates_with_llm(
                                    &bridge_clone,
                                    &capture,
                                    &sid_clone,
                                )
                                .await
                                {
                                    Ok(outcome) => {
                                        let mut events = outcome.events;
                                        events.extend(outcome.pending.into_iter().filter_map(
                                            |item| {
                                                (item.status == "pending_confirm").then(|| {
                                                    crate::features::memory::MemoryWriteEvent {
                                                        kind: item.kind,
                                                        action: "pending".to_string(),
                                                        id: item.id,
                                                        text: item.content,
                                                    }
                                                })
                                            },
                                        ));
                                        if !events.is_empty() {
                                            let _ = app_clone.emit(
                                                "chat:memory_write",
                                                json!({
                                                    "session_id": sid_clone,
                                                    "events": events,
                                                }),
                                            );
                                            match crate::features::memory::runtime_snapshot(&sid_clone) {
                                            Ok(snapshot) => {
                                                let _ = app_clone.emit(
                                                    "chat:memory",
                                                    json!({
                                                        "session_id": sid_clone,
                                                        "items": snapshot.items,
                                                        "runtime_path": snapshot.runtime_path,
                                                    }),
                                                );
                                            }
                                            Err(err) => eprintln!(
                                                "[pinvou3-app] refresh memory runtime after review failed for session {sid_clone}: {err}"
                                            ),
                                        }
                                        }
                                    }
                                    Err(err) => {
                                        eprintln!(
                                        "[pinvou3-app] memory llm review failed for session {sid_clone}: {err:#}"
                                    );
                                    }
                                }
                            });
                        }
                    }
                    maybe_notify_task_completed(
                        &app,
                        &store,
                        &session_id,
                        current_turn_id.take(),
                        terminal_status,
                        terminal_error.as_deref(),
                    );
                    if let Some(signal) = turn_tracker.on_terminal(terminal_status, terminal_error)
                    {
                        let _ = turn_events.send(signal);
                    }
                }
                Event::CompactionStarted {
                    id, message, auto, ..
                } => {
                    let payload = json!({ "session_id": session_id, "phase": "start", "id": id, "auto": auto, "message": message });
                    let _ = app.emit("chat:compaction", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:compaction",
                        payload,
                    );
                }
                Event::CompactionCompleted {
                    id,
                    message,
                    auto,
                    messages_before,
                    messages_after,
                    ..
                } => {
                    let payload = json!({
                        "session_id": session_id,
                        "phase": "done",
                        "id": id,
                        "auto": auto,
                        "message": message,
                        "messages_before": messages_before,
                        "messages_after": messages_after,
                    });
                    let _ = app.emit("chat:compaction", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:compaction",
                        payload,
                    );
                }
                Event::CompactionFailed { message, auto, .. } => {
                    let payload = json!({ "session_id": session_id, "phase": "fail", "auto": auto, "message": message });
                    let _ = app.emit("chat:compaction", payload.clone());
                    crate::features::remote_control::forward_app_event(
                        &app,
                        "chat:compaction",
                        payload,
                    );
                }
                Event::Error { envelope, .. } => {
                    // 可恢复错误(如 SSE idle timeout、瞬态工具失败)turn 不会结束——
                    // 引擎会 retry / 继续跑后续步骤。绝不能发 chat:done,否则前端
                    // setBusy(false) 把"思考中"指示器掐掉,而引擎还在干活,看着像卡死
                    // (且会误触发 flush/closeBubble/plan_phase 收尾)。只飘个 advisory。
                    // 仅 recoverable==false(致命)才是真结束 → chat:done。
                    if envelope.recoverable {
                        let payload =
                            json!({ "session_id": session_id, "error": envelope.message });
                        let _ = app.emit("chat:transient_error", payload.clone());
                        crate::features::remote_control::forward_app_event(
                            &app,
                            "chat:transient_error",
                            payload,
                        );
                    } else {
                        // 底座在致命 Error 后仍会发权威 TurnComplete(Failed)。这里只缓存
                        // 文本；完成、持久化和 chat:done 全部由 TurnComplete 单点处理。
                        // [C2] harness_phase 已删。工作流绑定(active_skill)的 session 遇
                        // 致命错误(SubAgent 派发失败 / 内部 fatal)→ 可能收不到 AgentComplete
                        // = 死锁。兜底:emit blocked 通知前端,让用户看到中断可重开(宁可多
                        // 通知一次,不可无声卡死等永不到来的事件)。
                        let was_active = store.mode_state(&session_id).active_skill.is_some();
                        if was_active {
                            let _ = app.emit(
                                "workflow:blocked",
                                json!({
                                    "session_id": session_id,
                                    "message": "工作流执行中断（致命错误，可能是 SubAgent 派发失败或内部错误），请检查后重新开始。",
                                }),
                            );
                        }
                        turn_tracker.on_fatal_error(envelope.message);
                    }
                }
                _ => {}
            }
        }
        let stopped_error = "Engine event stream stopped before a terminal event".to_string();
        let mut shell_cleanup_failed = false;
        {
            // The stream can stop before `TurnStarted` bound the provisional
            // submission scope; close the active scope too, otherwise the next
            // `prepare_turn` keeps bailing on "scope already active".
            let finalize = match current_turn_id.as_deref() {
                Some(turn_id) => turn_shell_tasks.finalize_turn(turn_id, true).await,
                None => turn_shell_tasks.finalize_active_scope(true).await,
            };
            match finalize {
                Ok(report) => {
                    if let Some(error) = report.failure_summary() {
                        shell_cleanup_failed = true;
                        log::error!(
                            "[pinvou3][chat] shell cleanup remained incomplete after event stream stop sid={} turn={:?}: {}",
                            session_id,
                            current_turn_id,
                            error
                        );
                    }
                }
                Err(error) => {
                    shell_cleanup_failed = true;
                    log::error!(
                        "[pinvou3][chat] failed to close shell task scope after event stream stop sid={} turn={:?}: {error:#}",
                        session_id,
                        current_turn_id
                    );
                }
            }
        }
        match finish_reclaimed_lifecycle_turn(
            &turn_lifecycle,
            &app,
            &store,
            &session_id,
            TurnOutcomeStatus::Failed,
            Some(stopped_error.clone()),
            shell_cleanup_failed,
        )
        .await
        {
            Some(EmittedTerminal {
                turn_id: Some(turn_id),
            }) => {
                let _ = turn_events.send(EngineTurnSignal::Terminal {
                    turn_id,
                    status: TurnOutcomeStatus::Failed,
                    error: Some(stopped_error),
                });
            }
            Some(EmittedTerminal { turn_id: None }) | None => {
                let _ = turn_events.send(EngineTurnSignal::ForwarderStopped {
                    error: stopped_error,
                });
            }
        }
        eprintln!(
            "[pinvou3-app] event forwarder stopped for session {session_id} (engine shut down?)"
        );
    })
}

/// 让 main.rs 编译时知道这个模块（供 docs/CI 用）。
pub fn _force_link() -> Arc<()> {
    Arc::new(())
}

#[cfg(test)]
mod token_ledger_tests {
    use super::TokenLedger;

    #[test]
    fn accumulates_per_role() {
        let mut l = TokenLedger::default();
        assert_eq!(l.add("pm", 100, 20), (100, 20, 1));
        assert_eq!(l.add("pm", 50, 10), (150, 30, 2));
        assert_eq!(l.add("writer", 7, 3), (7, 3, 1));
        assert_eq!(l.add("pm", 0, 0), (150, 30, 3));
    }
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
    use crate::core::mode_state::SessionModeState;
    use deepseek_tui::models::{ContentBlock, Message};
    use std::cell::Cell;
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
        lifecycle.arm_pending_cancel(epoch);
        assert!(
            lifecycle.take_pending_cancel(epoch).is_none(),
            "must not arm pending_cancel for an unsubmitted reservation"
        );

        // 走真实 send 路径：handle.send 成功后 mark_submitted → submitted=true。
        lifecycle.mark_reservation_submitted(reservation.reservation_id);
        // 已 submitted + active + turn_id=None（TurnStarted 未抵达）→ arm 置位。
        lifecycle.arm_pending_cancel(epoch);
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
        lifecycle.arm_pending_cancel(epoch_a);
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
        lifecycle.arm_pending_cancel(epoch_b);
        assert!(
            lifecycle.take_pending_cancel(epoch_b).is_none(),
            "must not arm pending_cancel after TurnStarted"
        );

        // 清理：结束当前 turn。
        assert!(lifecycle.finish_once(|| {}).is_some());
    }

    #[test]
    fn arm_pending_cancel_uses_target_snapshot_not_reread_current() {
        // reviewer 点 3 的确定性回归：cancel 在 generation 复查通过后 arm
        // pending 时，必须使用发起时快照 target，而不是此刻重读的当前 epoch。
        // 若重读 current：复查通过后、arm 前另一 worker 结束旧轮并启动新轮，
        // 读到的是新轮 epoch，arm_pending_cancel(新epoch) 会把 pending 绑定到
        // 新轮（新轮恰好处于 submitted 未 started 时），后面的 cancel_current
        // 命中新 token → 原始跨轮误取消窗口仍未关闭。
        //
        // 这里直接编排「复查通过后轮次切换」：turn1 处于 submitted 未 started
        // （arm 前置满足），快照 target=epoch1；切换 turn2 并提交后，用 target
        // arm 必须被拒绝（turn_epoch 已 != target），pending 不落到新轮。
        let lifecycle = Arc::new(TurnLifecycle::default());

        // turn1：on_submitted 激活（active+submitted+epoch=1，turn_id=None）。
        assert!(lifecycle.on_submitted());
        let target = lifecycle
            .current_turn_generation()
            .expect("turn1 active epoch");

        // 复查通过后、arm 前轮次切换：turn1 终态 → turn2 reserve + 提交。
        assert!(lifecycle.finish_once(|| {}).is_some());
        let reservation2 = lifecycle.reserve().expect("turn2 reserve");
        lifecycle.mark_reservation_submitted(reservation2.reservation_id);
        let epoch2 = lifecycle
            .current_turn_generation()
            .expect("turn2 active epoch");
        assert_eq!(epoch2, target + 1, "turn2 must bump the epoch past target");

        // 用发起时快照 target arm：turn_epoch 已是 epoch2 != target → 拒绝并
        // 返回 false（reviewer 点 6：调用方据返回值跳过 cancel_current，否则
        // 取消会命中新轮已 reset_cancel_token 的活跃 token）。
        assert!(
            !lifecycle.arm_pending_cancel(target),
            "arm with the stale target snapshot must report epoch mismatch"
        );
        assert!(
            lifecycle.take_pending_cancel(epoch2).is_none(),
            "arm with the stale target snapshot must not bind pending to the new turn"
        );

        // 防 Drop 副作用。
        reservation2.mark_submitted();
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
        assert!(!lifecycle.arm_pending_cancel(target));
        assert!(
            lifecycle.take_pending_cancel(epoch2).is_none(),
            "rejected arm must not set pending on the new turn"
        );

        // 当前 epoch：接受 + pending 置位（turn2 仍 submitted 未 started）。
        assert!(lifecycle.arm_pending_cancel(epoch2));
        assert!(
            lifecycle.take_pending_cancel(epoch2).is_some(),
            "accepted arm must set pending for the current turn"
        );

        // 防 Drop 副作用。
        reservation2.mark_submitted();
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
    fn pending_cancel_does_not_leak_across_turns() {
        // 防跨轮污染：上一轮 arm 但未被消费的标记不能影响下一轮。
        // reserve() 必须清除 stale pending_cancel。
        let lifecycle = Arc::new(TurnLifecycle::default());

        lifecycle.on_submitted();
        let epoch_prev = lifecycle.current_turn_generation().expect("active epoch");
        lifecycle.arm_pending_cancel(epoch_prev);
        // 模拟 turn 未正常 started 就结束（如 engine spawn 失败）。
        assert!(lifecycle.finish_once(|| {}).is_some());

        // 新一轮 reserve 后，pending_cancel 必须被清除。
        let _reservation = lifecycle.reserve().expect("reserve");
        let epoch_next = lifecycle.current_turn_generation().expect("active epoch");
        assert!(
            lifecycle.take_pending_cancel(epoch_next).is_none(),
            "stale pending_cancel from previous turn must be cleared by reserve"
        );
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
        lifecycle.arm_pending_cancel(epoch);

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
        // 不误取消新轮（不触发 approve_handle.cancel 重放）。
        //
        // 现有 pending_cancel_does_not_leak_across_turns 只测 reserve 清除，不覆盖
        // 「新轮已 reserve 后 C2 才 arm」的窗口——此测试直接覆盖该缺口。
        let lifecycle = Arc::new(TurnLifecycle::default());

        // 旧轮：submit（epoch=1），arm pending_cancel 绑定到 epoch=1。
        lifecycle.on_submitted();
        let epoch_old = lifecycle.current_turn_generation().expect("old epoch");
        lifecycle.arm_pending_cancel(epoch_old);

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
    use crate::core::mode_state::SerializableMode;
    use crate::features::monitor::SelfMetrics;

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
