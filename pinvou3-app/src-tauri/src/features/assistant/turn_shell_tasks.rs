//! App-owned lifecycle for detached shell jobs.
//!
//! CodeWhale deliberately allows a detached job to outlive the tool call that
//! started it. Pinvou's main stop action is stronger: it also stops jobs that
//! belong to the interrupted root turn. The host already owns turn lifecycle
//! and receives exact background task ids, so that product policy stays here.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use deepseek_tui::tools::shell::{SharedShellManager, ShellManager, ShellResult, ShellStatus};
use parking_lot::Mutex;

#[derive(Debug)]
struct ActiveTurnShellScope {
    turn_id: String,
    baseline_task_ids: HashSet<String>,
    registered_task_ids: HashSet<String>,
    cancel_requested: bool,
}

/// Tracks detached shell jobs for the one active root turn in a session.
///
/// Exact task ids handle the ordinary path. The baseline is the race-safe
/// fallback for a process that is inserted into `ShellManager` just before
/// cancellation but whose `ToolCallComplete` has not reached the app yet.
#[derive(Clone)]
pub(crate) struct TurnShellTaskRegistry {
    shell_manager: SharedShellManager,
    active: Arc<Mutex<Option<ActiveTurnShellScope>>>,
}

impl TurnShellTaskRegistry {
    pub(crate) fn new(shell_manager: SharedShellManager) -> Self {
        Self {
            shell_manager,
            active: Arc::new(Mutex::new(None)),
        }
    }

    /// Opens a new turn scope after the authoritative `TurnStarted` event.
    pub(crate) fn begin_turn(&self, turn_id: &str) -> Result<()> {
        let baseline_task_ids = self
            .shell_manager
            .lock()
            .map_err(|_| anyhow!("Shell manager lock poisoned"))?
            .list_jobs()
            .into_iter()
            .map(|job| job.id)
            .collect();
        *self.active.lock() = Some(ActiveTurnShellScope {
            turn_id: turn_id.to_string(),
            baseline_task_ids,
            registered_task_ids: HashSet::new(),
            cancel_requested: false,
        });
        Ok(())
    }

    /// Registers the exact id returned by a detached shell tool. If stop won
    /// the race, the newly observed job is drained immediately.
    pub(crate) fn register_task(&self, turn_id: &str, task_id: &str) -> Result<Vec<ShellResult>> {
        let should_drain = {
            let mut active = self.active.lock();
            let Some(scope) = active.as_mut().filter(|scope| scope.turn_id == turn_id) else {
                return Ok(Vec::new());
            };
            scope.registered_task_ids.insert(task_id.to_string());
            scope.cancel_requested
        };
        if should_drain {
            self.drain_cancelled_turn(turn_id)
        } else {
            Ok(Vec::new())
        }
    }

    /// Marks cancellation before the engine token is triggered. Late task-id
    /// registrations can therefore observe stop even if they arrive later.
    pub(crate) fn request_cancel(&self, turn_id: &str) -> bool {
        let mut active = self.active.lock();
        let Some(scope) = active.as_mut().filter(|scope| scope.turn_id == turn_id) else {
            return false;
        };
        scope.cancel_requested = true;
        true
    }

    /// Stops every running job created after this turn's baseline. Existing
    /// jobs from an earlier turn are never candidates.
    pub(crate) fn drain_cancelled_turn(&self, turn_id: &str) -> Result<Vec<ShellResult>> {
        let (baseline_task_ids, registered_task_ids) = {
            let active = self.active.lock();
            let Some(scope) = active
                .as_ref()
                .filter(|scope| scope.turn_id == turn_id && scope.cancel_requested)
            else {
                return Ok(Vec::new());
            };
            (
                scope.baseline_task_ids.clone(),
                scope.registered_task_ids.clone(),
            )
        };

        let mut manager = self
            .shell_manager
            .lock()
            .map_err(|_| anyhow!("Shell manager lock poisoned"))?;
        let candidates = manager
            .list_jobs()
            .into_iter()
            .filter(|job| {
                job.status == ShellStatus::Running
                    && (registered_task_ids.contains(&job.id)
                        || !baseline_task_ids.contains(&job.id))
            })
            .map(|job| job.id)
            .collect::<Vec<_>>();
        kill_running_candidates(&mut manager, candidates)
    }

    /// Finalizes one turn. Interrupted turns perform a last baseline drain to
    /// cover spawn/metadata-delivery races; completed turns preserve detached
    /// jobs by design.
    pub(crate) fn finish_turn(&self, turn_id: &str, interrupted: bool) -> Result<Vec<ShellResult>> {
        if interrupted {
            self.request_cancel(turn_id);
        }
        let result = self.drain_cancelled_turn(turn_id);
        let mut active = self.active.lock();
        if active
            .as_ref()
            .is_some_and(|scope| scope.turn_id == turn_id)
        {
            *active = None;
        }
        result
    }
}

fn kill_running_candidates(
    manager: &mut ShellManager,
    candidates: Vec<String>,
) -> Result<Vec<ShellResult>> {
    let mut killed = Vec::with_capacity(candidates.len());
    let mut first_error = None;
    for task_id in candidates {
        match manager.kill(&task_id) {
            Ok(result) => killed.push(result),
            Err(error) => {
                // The process can exit between the status refresh above and
                // the kill call. Treat that terminal race as an idempotent
                // no-op without changing CodeWhale's generic kill contract.
                let became_terminal = manager
                    .inspect_job(&task_id)
                    .is_ok_and(|job| job.snapshot.status != ShellStatus::Running);
                if !became_terminal && first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(killed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepseek_tui::tools::shell::new_shared_shell_manager;

    fn sleep_command() -> &'static str {
        if std::env::consts::OS == "windows" {
            "Start-Sleep -Seconds 30"
        } else {
            "sleep 30"
        }
    }

    fn start_background(manager: &SharedShellManager) -> String {
        manager
            .lock()
            .expect("shell manager lock")
            .execute(sleep_command(), None, 600_000, true)
            .expect("start background shell")
            .task_id
            .expect("background task id")
    }

    fn status(manager: &SharedShellManager, task_id: &str) -> ShellStatus {
        manager
            .lock()
            .expect("shell manager lock")
            .inspect_job(task_id)
            .expect("shell job")
            .snapshot
            .status
    }

    #[test]
    fn forkguard_interrupted_turn_fallback_preserves_preexisting_jobs() {
        let manager = new_shared_shell_manager(std::env::temp_dir());
        let previous = start_background(&manager);
        let registry = TurnShellTaskRegistry::new(manager.clone());
        registry.begin_turn("turn-current").expect("begin turn");
        let current = start_background(&manager);

        let killed = registry
            .finish_turn("turn-current", true)
            .expect("finish interrupted turn");

        assert_eq!(killed.len(), 1);
        assert_eq!(killed[0].task_id.as_deref(), Some(current.as_str()));
        assert_eq!(status(&manager, &current), ShellStatus::Killed);
        assert_eq!(status(&manager, &previous), ShellStatus::Running);
        manager
            .lock()
            .expect("shell manager lock")
            .kill(&previous)
            .expect("cleanup previous job");
    }

    #[test]
    fn task_registered_after_stop_is_killed_immediately() {
        let manager = new_shared_shell_manager(std::env::temp_dir());
        let registry = TurnShellTaskRegistry::new(manager.clone());
        registry.begin_turn("turn-race").expect("begin turn");
        assert!(registry.request_cancel("turn-race"));
        let late = start_background(&manager);

        let killed = registry
            .register_task("turn-race", &late)
            .expect("register late task");

        assert_eq!(killed.len(), 1);
        assert_eq!(status(&manager, &late), ShellStatus::Killed);
        registry
            .finish_turn("turn-race", true)
            .expect("finish turn");
    }

    #[test]
    fn completed_turn_keeps_its_detached_job_running() {
        let manager = new_shared_shell_manager(std::env::temp_dir());
        let registry = TurnShellTaskRegistry::new(manager.clone());
        registry.begin_turn("turn-complete").expect("begin turn");
        let detached = start_background(&manager);
        registry
            .register_task("turn-complete", &detached)
            .expect("register detached task");

        let killed = registry
            .finish_turn("turn-complete", false)
            .expect("finish completed turn");

        assert!(killed.is_empty());
        assert_eq!(status(&manager, &detached), ShellStatus::Running);
        manager
            .lock()
            .expect("shell manager lock")
            .kill(&detached)
            .expect("cleanup detached job");
    }
}
