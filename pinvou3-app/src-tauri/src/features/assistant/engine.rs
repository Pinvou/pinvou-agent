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
    pub(crate) workspace: PathBuf,
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
    active_reservation_id: Option<u64>,
    transcript_rules: Vec<TranscriptSanitizationRule>,
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

fn emit_turn_admission(app: &AppHandle, session_id: &str, admission: TurnAdmission) {
    let user_payload = admission.user_payload(session_id);
    let _ = app.emit("chat:user_message", user_payload.clone());
    crate::features::remote_control::forward_app_event(app, "chat:user_message", user_payload);
    let started_payload = json!({ "session_id": session_id });
    let _ = app.emit("chat:turn_started", started_payload.clone());
    crate::features::remote_control::forward_app_event(app, "chat:turn_started", started_payload);
}

impl TurnLifecycle {
    /// 该 session 当前是否有进行中的 turn（reserve 占用或终态收口未完成）。
    pub(crate) fn is_active(&self) -> bool {
        let state = self.state.lock();
        state.active || state.terminal_closing
    }

    pub(crate) fn reserve(self: &Arc<Self>) -> Result<TurnReservation> {
        let reservation_id = {
            let mut state = self.state.lock();
            if state.active || state.terminal_closing {
                anyhow::bail!("session_turn_in_progress");
            }
            state.next_reservation_id = state.next_reservation_id.wrapping_add(1).max(1);
            let reservation_id = state.next_reservation_id;
            state.active = true;
            state.submitted = false;
            state.turn_id = None;
            state.terminal_emitted = false;
            state.terminal_closing = false;
            state.reclaimed = false;
            state.admission_emitted = false;
            state.active_reservation_id = Some(reservation_id);
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
        // 执行根统一由 SessionStore::session_roots 解析（scheduled = automation
        // workspace，原生代码绑项目会话 = 项目目录，其余 = 会话私有目录）。
        // 解析失败（如 scheduled 会话缺 profile）时维持原回退：bridge 侧解析。
        let workspace = store
            .session_roots(session_id)
            .map(|roots| roots.execution)
            .unwrap_or_else(|_| bridge.session_workspace(session_id));
        let mut engine_config = bridge.build_engine_config_for_session_at(session_id, workspace);
        engine_config.runtime_services.shell_manager = Some(shell_manager.clone());
        // Agentic RAG:给该 session 的 engine 注入 kb_search + kb_open_source(都持
        // session_id,execute 时只访问该会话挂载的知识集)。工具常驻所有会话,挂没挂集由
        // 运行时判断。
        engine_config.extra_tools.0.extend(extra_tools);
        // 工具门控:连接器开关禁用 +(知识库为空时)隐藏 kb_search/kb_open_source。compute 返回
        // **完整**列表(已含连接器禁用),直接覆盖 build_engine_config 设的「连接器-only」初值,
        // 让新会话天生正确——空知识库就看不到知识工具,不会宣称能本地检索。
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
                workspace,
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
            workspace,
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
        let op =
            self.bridge
                .build_send_message_op(content, mode, persona_reminder, restrict_tools)?;
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
        let op =
            self.bridge
                .build_send_message_op(content, mode, persona_reminder, restrict_tools)?;
        self.send_reserved_turn_op(op, reservation).await
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
                max_steps,
                output_schema,
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
                    max_steps: t.max_steps,
                    output_schema: t.output_schema,
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
/// 关键点：监听 `Event::ApprovalRequired` 并主动 `approve_tool_call`——
/// 上游 `Op::SendMessage.auto_approve` 不旁路 `await_tool_approval`
/// （turn_loop.rs:1117 只看 ToolSpec.approval_requirement，不看
/// session.auto_approve），需要 frontend 端主动发 ApprovalDecision::Approved
/// 才能解锁工具执行。
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
                    if !turn_lifecycle.emit_started_admission(&app, &session_id, turn_id.clone()) {
                        continue;
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
                    let (output, success, metadata) = match result {
                        Ok(r) => (r.content, true, r.metadata),
                        Err(e) => (format!("{e:?}"), false, None),
                    };
                    let background_task_id =
                        if matches!(name.as_str(), "exec_shell" | "task_shell_start")
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
                            let artifact_output = output.clone();
                            match tokio::task::spawn_blocking(move || {
                                persist_successful_tool_artifact(
                                    &artifact_store,
                                    &artifact_session_id,
                                    &artifact_workspace,
                                    &artifact_name,
                                    &tracked_input,
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
                    crate::features::memory::record_turn_tool_complete(&session_id, &name, success);
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
                    // 只有无人值守的初始定时执行使用任务保存的批准策略；用户从
                    // 运行历史进入后继续对话，行为与普通对话一致。
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
                // [edict-obs] SubAgent 每步进展(底座自动发,不靠 prompt 纪律)→ 前端看板。
                Event::AgentProgress { id, status, .. } => {
                    // 早期事件可能赶在第二发 AgentSpawned(role_id)之前到 —— 兜底用
                    // agent_id 而不是空串,跟 TokenUsage 臂的 fallback 语义一致。
                    let role = agent_roles.get(&id).cloned().unwrap_or_else(|| id.clone());
                    let _ = app.emit(
                        "workflow:agent_progress",
                        json!({
                            "session_id": session_id,
                            "agent_id": id,
                            "role_id": role,
                            "status": status,
                        }),
                    );
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
                            model,
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
                                    "model": model,
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
                            let _ = app.emit(
                                "workflow:agent_progress",
                                json!({
                                    "session_id": session_id,
                                    "agent_id": agent_id,
                                    "role_id": role,
                                    "status": format!("🔧 {tool_name} (step {step})"),
                                }),
                            );
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
                    let _ = app.emit(
                        "workflow:agent_complete",
                        json!({ "session_id": session_id, "agent_id": id }),
                    );
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
                                                max_steps: t.max_steps,
                                                output_schema: t.output_schema,
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
                                                max_steps: t.max_steps,
                                                output_schema: t.output_schema,
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
                    let status_text = format!("{terminal_status:?}");
                    // 落 usage 进 timing_events.jsonl,作为模型耗时/失败/上下文消耗的
                    // 内部诊断数据源。usage.input/output_tokens 是 u32,
                    // prompt_cache_*_tokens / reasoning_tokens 是 Option<u32>,
                    // 统一 unwrap_or(0) + u64 转换落盘。
                    // [F3] 转发 cache_write_tokens(Anthropic cache-write 按 1.25x 计费)
                    // 与 reasoning_tokens(DeepSeek V4 思考模型主要成本),否则字段一旦缺
                    // 持久化就回填不了,影响将来真实 cost 估算。
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
            show_thinking: false,
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

        drop(store);
        let reopened = crate::features::sessions::SessionStore::boot().expect("reopen store");
        let paths: Vec<_> = reopened
            .load(&scheduled.metadata.id)
            .expect("reopen scheduled session")
            .artifacts
            .into_iter()
            .map(|artifact| artifact.storage_path)
            .collect();
        assert_eq!(paths, vec![persisted]);

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
