//! Transactional checkout guards for one-shot session state.
//!
//! Both guards live next to the mode-state machine because they only make
//! sense when paired with an in-progress turn submission:
//!
//! - [`PendingTurnInjections`] checks out the per-session one-shot persona body.
//!   Unless committed after Engine submission, `Drop` restores a value that still
//!   belongs to the same persona and has not been replaced meanwhile.
//! - [`PendingPlanClaim`] checks out the currently actionable Plan ticket.
//!   Claiming switches the session to Yolo before the execution turn is
//!   submitted. `Drop` restores Plan + ticket on every pre-submission error or
//!   cancelled command future.

use anyhow::Result;

use super::SessionModeState;
use super::SessionStore;

/// Transactional checkout of the per-session one-shot persona body.
/// Unless committed after Engine submission, Drop restores a value that still
/// belongs to the same persona and has not been replaced meanwhile.
pub(crate) struct PendingTurnInjections {
    pub(crate) store: SessionStore,
    pub(crate) session_id: String,
    pub(crate) persona: Option<(Option<String>, String)>,
    pub(crate) committed: bool,
}

impl PendingTurnInjections {
    pub(crate) fn persona_body(&self) -> Option<&str> {
        self.persona.as_ref().map(|(_, body)| body.as_str())
    }

    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for PendingTurnInjections {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        self.store
            .restore_pending_turn_injections(&self.session_id, self.persona.take());
    }
}

/// Atomic checkout of the currently actionable Plan ticket. Claiming switches
/// the session to Yolo before the execution turn is submitted. Drop restores
/// Plan + ticket on every pre-submission error or cancelled command future.
pub(crate) struct PendingPlanClaim {
    pub(crate) store: SessionStore,
    pub(crate) session_id: String,
    pub(crate) plan_id: String,
    pub(crate) accepted_state: SessionModeState,
    pub(crate) settled: bool,
}

impl PendingPlanClaim {
    pub(crate) fn accepted_state(&self) -> &SessionModeState {
        &self.accepted_state
    }

    pub(crate) fn commit(mut self) {
        self.store
            .finish_pending_plan_claim(&self.session_id, &self.plan_id);
        // accept（【✅ 就这么干】）确认执行后，会话 mode 已切 Yolo 且任务已提交：
        // 把它纳入两层持久化，否则重启/切走切回后会话恢复持久化的旧 mode
        // （Plan），表现为「方案卡切到 Yolo，切回来又变回 Plan」。
        self.store.persist_accepted_yolo_mode(&self.session_id);
        self.settled = true;
    }

    pub(crate) fn rollback(mut self) -> Result<()> {
        let result = self
            .store
            .restore_pending_plan_claim(&self.session_id, &self.plan_id);
        self.settled = true;
        result
    }
}

impl Drop for PendingPlanClaim {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        if let Err(error) = self
            .store
            .restore_pending_plan_claim(&self.session_id, &self.plan_id)
        {
            eprintln!(
                "[sessions] restore dropped plan claim for {} failed: {error:#}",
                self.session_id
            );
        }
    }
}
