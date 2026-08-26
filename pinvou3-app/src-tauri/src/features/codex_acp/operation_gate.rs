use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Result};
use serde_json::json;

use super::{events::patch_acp_state, AcpPool, AcpSession, CodexAcpSessionInfo};

/// Owns the session's configuration slot until the operation finishes. Drop
/// based release keeps cancellation and early-return paths from leaving the
/// session permanently stuck in `configuring`.
pub(super) struct ConfigurationGuard<'a> {
    configuring: &'a AtomicBool,
}

impl Drop for ConfigurationGuard<'_> {
    fn drop(&mut self) {
        self.configuring.store(false, Ordering::Release);
    }
}

pub(super) fn begin_configuration<'a>(
    busy: &AtomicBool,
    configuring: &'a AtomicBool,
    busy_error: &str,
) -> Result<ConfigurationGuard<'a>> {
    if busy.load(Ordering::Acquire) {
        bail!(busy_error.to_string());
    }
    if configuring.swap(true, Ordering::AcqRel) {
        bail!("ACP 会话已有配置正在同步");
    }
    if busy.load(Ordering::Acquire) {
        configuring.store(false, Ordering::Release);
        bail!(busy_error.to_string());
    }
    Ok(ConfigurationGuard { configuring })
}

/// Reserve the prompt slot with a final configuration recheck. Together with
/// `begin_configuration`'s final busy recheck this closes both interleavings
/// without serializing unrelated sessions behind a global mutex.
pub(super) fn begin_prompt(busy: &AtomicBool, configuring: &AtomicBool) -> Result<()> {
    if configuring.load(Ordering::Acquire) {
        bail!("ACP 会话配置仍在同步，请稍候再发送");
    }
    if busy.swap(true, Ordering::AcqRel) {
        bail!("ACP 会话仍在生成");
    }
    if configuring.load(Ordering::Acquire) {
        busy.store(false, Ordering::Release);
        bail!("ACP 会话配置仍在同步，请稍候再发送");
    }
    Ok(())
}

/// Admit a prompt turn atomically across the fallible activity touch.
///
/// The sequence — busy admission, activity touch, timing registration — must
/// stay atomic with respect to the touch failure: registration writes both a
/// queue entry and a persisted `user_start` event that only a spawned prompt's
/// `timing::finish_turn` can pair, so registering before a touch that then
/// fails would leak both with no task left to finish them (round-8 review).
/// Running the touch inside the admitted section (busy held) preserves the
/// admission semantics and lets the failure path roll back the slot before
/// returning.
///
/// The timing registration must also happen inside the admitted section and
/// before the prompt task is spawned: the spawned task can complete (fast mock
/// response, instant connection error) and call `timing::finish_turn` before
/// `send_message` returns, so registering from the caller after the await would
/// race that finish — the completion would find an empty queue and be dropped,
/// leaving the late registration as a stale unpaired entry. Registering only
/// after admission also guarantees a rejected concurrent submit never enqueues
/// a ghost turn whose send_error finish would clear the in-flight turn's
/// queue (round-7 review invariant).
pub(super) fn admit_prompt_turn(
    busy: &AtomicBool,
    configuring: &AtomicBool,
    session_id: &str,
    touch_activity: impl FnOnce() -> Result<()>,
) -> Result<()> {
    begin_prompt(busy, configuring)?;
    if let Err(error) = touch_activity() {
        busy.store(false, Ordering::Release);
        return Err(error);
    }
    crate::features::assistant::timing::start_turn(session_id);
    Ok(())
}

enum SessionConfigChange<'a> {
    Model(&'a str),
    Mode(&'a str),
    Option {
        config_id: &'a str,
        value_id: &'a str,
    },
}

impl SessionConfigChange<'_> {
    fn config_id(&self) -> &str {
        match self {
            Self::Model(_) => "model",
            Self::Mode(_) => "mode",
            Self::Option { config_id, .. } => config_id,
        }
    }

    fn value_id(&self) -> &str {
        match self {
            Self::Model(value_id) | Self::Mode(value_id) => value_id,
            Self::Option { value_id, .. } => value_id,
        }
    }

    fn busy_error(&self) -> &'static str {
        match self {
            Self::Model(_) => "Agent 正在处理当前任务，模型将在本轮结束后才能修改",
            Self::Mode(_) => "Agent 正在处理当前任务，权限模式将在本轮结束后才能修改",
            Self::Option { .. } => "Agent 正在处理当前任务，配置将在本轮结束后才能修改",
        }
    }

    async fn apply(&self, runtime: &AcpSession) -> Result<()> {
        match self {
            Self::Model(model_id) => runtime.set_model(model_id).await,
            Self::Mode(mode_id) => runtime.set_mode(mode_id).await,
            Self::Option {
                config_id,
                value_id,
            } => runtime.set_config_option(config_id, value_id).await,
        }
    }
}

impl AcpPool {
    fn remember_config_choice(
        &self,
        session_id: &str,
        runtime: &AcpSession,
        config_id: &str,
        value_id: &str,
    ) {
        let backend = self.backend(session_id);
        let mut errors = Vec::new();
        if let Err(error) = self
            .agents
            .set_acp_config_value(session_id, config_id, value_id)
        {
            errors.push(format!("会话配置: {error:#}"));
        }
        if let Err(error) = self.config_defaults.set(backend, config_id, value_id) {
            errors.push(format!("新会话默认值: {error:#}"));
        }
        if !errors.is_empty() {
            let message = errors.join("；");
            eprintln!(
                "[pinvou3-app] failed to persist {} ACP config {}={}: {}",
                backend.display_name(),
                config_id,
                value_id,
                message
            );
            runtime.bridge.emit(
                "config_persistence_failed",
                json!({
                    "configId": config_id,
                    "valueId": value_id,
                    "message": message,
                }),
            );
        }
    }

    async fn apply_config_change(
        &self,
        session_id: &str,
        change: SessionConfigChange<'_>,
    ) -> Result<CodexAcpSessionInfo> {
        let runtime = self.get_or_spawn(session_id).await?;
        let _configuration =
            begin_configuration(&runtime.busy, &runtime.configuring, change.busy_error())?;
        let config_id = change.config_id();
        let value_id = change.value_id();
        runtime.bridge.emit(
            "config_change_requested",
            json!({ "configId": config_id, "valueId": value_id }),
        );
        if let Err(error) = change.apply(&runtime).await {
            runtime.bridge.emit(
                "config_change_failed",
                json!({
                    "configId": config_id,
                    "valueId": value_id,
                    "message": format!("{error:#}"),
                }),
            );
            return Err(error);
        }
        self.remember_config_choice(session_id, &runtime, config_id, value_id);
        runtime.bridge.emit(
            "config_change_applied",
            json!({ "configId": config_id, "valueId": value_id }),
        );
        let info = runtime.info(
            self.pending_permissions_for(session_id).await,
            self.pending_elicitations_for(session_id).await,
        );
        patch_acp_state(session_id, json!({ "session": &info }))?;
        Ok(info)
    }

    pub async fn set_model(&self, session_id: &str, model_id: &str) -> Result<CodexAcpSessionInfo> {
        self.apply_config_change(session_id, SessionConfigChange::Model(model_id))
            .await
    }

    pub async fn set_config_option(
        &self,
        session_id: &str,
        config_id: &str,
        value_id: &str,
    ) -> Result<CodexAcpSessionInfo> {
        self.apply_config_change(
            session_id,
            SessionConfigChange::Option {
                config_id,
                value_id,
            },
        )
        .await
    }

    pub async fn set_mode(&self, session_id: &str, mode_id: &str) -> Result<CodexAcpSessionInfo> {
        self.apply_config_change(session_id, SessionConfigChange::Mode(mode_id))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_and_configuration_slots_are_mutually_exclusive() {
        let busy = AtomicBool::new(false);
        let configuring = AtomicBool::new(false);

        let guard = begin_configuration(&busy, &configuring, "busy").unwrap();
        assert!(begin_prompt(&busy, &configuring).is_err());
        drop(guard);

        begin_prompt(&busy, &configuring).unwrap();
        assert!(begin_configuration(&busy, &configuring, "busy").is_err());
    }

    #[test]
    fn configuration_guard_releases_on_drop() {
        let busy = AtomicBool::new(false);
        let configuring = AtomicBool::new(false);
        {
            let _guard = begin_configuration(&busy, &configuring, "busy").unwrap();
            assert!(configuring.load(Ordering::Acquire));
        }
        assert!(!configuring.load(Ordering::Acquire));
    }

    /// The timing turn must be registered inside the admitted section, before
    /// the caller can race an immediate completion. Simulates the production
    /// sequence in `AcpPool::send_message`: `admit_prompt_turn` (admission +
    /// activity touch + registration), then a synchronous completion arriving
    /// before the method would have returned. The terminal must be attributed
    /// to the registered turn and the queue must be empty afterwards; a
    /// caller-side registration after the await would instead leave the
    /// completion dropped and a stale unpaired entry behind.
    #[test]
    fn admitted_prompt_turn_survives_immediate_completion() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-acp-admission-timing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::env::set_var("PINVOU3_HOME", &tmp);

        let busy = AtomicBool::new(false);
        let configuring = AtomicBool::new(false);
        let sid = "acp-admission-immediate-completion";

        admit_prompt_turn(&busy, &configuring, sid, || Ok(())).unwrap();
        // Admission registered the turn synchronously, before any spawned
        // prompt task can run: the queue holds it at this point.
        assert!(crate::features::assistant::timing::has_queued_active_turn(
            sid
        ));
        // Immediate completion: happens-before any post-await caller code.
        // admit_prompt_turn has already registered the turn, so the terminal
        // lands on it instead of being dropped on an empty queue.
        crate::features::assistant::timing::finish_turn(sid, "Completed", None);

        let timeline = crate::features::assistant::timing::read_timeline(sid).unwrap();
        let done: Vec<_> = timeline
            .iter()
            .filter(|e| e.event == "assistant_done")
            .collect();
        assert_eq!(done.len(), 1, "immediate completion must be recorded");
        assert_eq!(
            timeline[0].event, "user_start",
            "terminal must pair with the registered turn"
        );
        assert_eq!(done[0].turn_id, timeline[0].turn_id);
        assert!(
            !crate::features::assistant::timing::has_queued_active_turn(sid),
            "queue must be empty after the paired finish"
        );

        let _ = std::fs::remove_dir_all(tmp);
    }

    /// A submit rejected by busy admission must neither enqueue a ghost turn
    /// nor disturb the in-flight turn's queue entry: its send_error finish
    /// (if the caller emits one) finds only the in-flight turn and would be
    /// attributed correctly, and the in-flight terminal is never swallowed.
    #[test]
    fn rejected_concurrent_submit_never_clears_in_flight_turn() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-acp-reject-timing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::env::set_var("PINVOU3_HOME", &tmp);

        let busy = AtomicBool::new(false);
        let configuring = AtomicBool::new(false);
        let sid = "acp-admission-rejected-submit";

        // First submit is admitted and its turn registered.
        admit_prompt_turn(&busy, &configuring, sid, || Ok(())).unwrap();
        // Concurrent second submit is rejected by busy admission: no turn is
        // registered (no ghost queue entry).
        let rejected = admit_prompt_turn(&busy, &configuring, sid, || Ok(()));
        assert!(rejected.is_err());
        assert!(
            !crate::features::assistant::timing::has_extra_queued_turns(sid),
            "rejected submit must not enqueue a second queue entry"
        );

        // The in-flight turn finishes; a late send_error from the rejected
        // caller afterwards must be a no-op (empty queue), so the in-flight
        // terminal is recorded exactly once and never swallowed.
        crate::features::assistant::timing::finish_turn(sid, "Completed", None);
        crate::features::assistant::timing::finish_turn(sid, "send_error", Some("busy"));

        let timeline = crate::features::assistant::timing::read_timeline(sid).unwrap();
        let done: Vec<_> = timeline
            .iter()
            .filter(|e| e.event == "assistant_done")
            .collect();
        assert_eq!(
            done.len(),
            1,
            "in-flight terminal must be recorded exactly once"
        );
        assert_eq!(done[0].status.as_deref(), Some("Completed"));

        let _ = std::fs::remove_dir_all(tmp);
    }

    /// Round-8 regression (failure injection): an activity-touch failure after
    /// successful busy admission must leave no residue of any kind — the busy
    /// slot is released for the next submit, no timing queue entry remains,
    /// and no `user_start` lifecycle event is persisted (registration writes
    /// both, and with no prompt task spawned nothing would ever pair a
    /// terminal with them). The next admitted submit must then behave as if
    /// the failed one never happened.
    #[test]
    fn activity_touch_failure_releases_slot_and_defers_registration() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-acp-touch-failure-timing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::env::set_var("PINVOU3_HOME", &tmp);

        let busy = AtomicBool::new(false);
        let configuring = AtomicBool::new(false);
        let sid = "acp-admission-touch-failure";

        // Injected store failure: the session store cannot persist the
        // activity update (disk full, corrupted session file, invalid id...).
        let failed = admit_prompt_turn(&busy, &configuring, sid, || {
            Err(anyhow::anyhow!("simulated touch_activity failure"))
        });
        assert!(failed.is_err(), "touch failure must propagate to caller");

        // Busy slot rolled back: the session is immediately submittable.
        assert!(
            !busy.load(Ordering::Acquire),
            "busy slot must be released on touch failure"
        );
        // No queue entry was registered for the failed submit.
        assert!(
            !crate::features::assistant::timing::has_queued_active_turn(sid),
            "failed submit must not leave a queued timing entry"
        );
        // No lifecycle event was persisted: there is no accepted turn whose
        // start a later terminal could be misattributed to.
        let timeline = crate::features::assistant::timing::read_timeline(sid).unwrap();
        assert!(
            timeline.is_empty(),
            "failed submit must not persist any timing event"
        );

        // The next submit on the same session is unaffected: admitted, turn
        // registered, and a terminal pairs with it exactly.
        admit_prompt_turn(&busy, &configuring, sid, || Ok(())).unwrap();
        assert!(crate::features::assistant::timing::has_queued_active_turn(
            sid
        ));
        crate::features::assistant::timing::finish_turn(sid, "Completed", None);

        let timeline = crate::features::assistant::timing::read_timeline(sid).unwrap();
        let done: Vec<_> = timeline
            .iter()
            .filter(|e| e.event == "assistant_done")
            .collect();
        assert_eq!(done.len(), 1, "next turn terminal recorded exactly once");
        assert_eq!(timeline[0].event, "user_start");
        assert_eq!(done[0].turn_id, timeline[0].turn_id);

        let _ = std::fs::remove_dir_all(tmp);
    }
}
