//! Upload-package removal needs completed engine teardown, rather than merely
//! queuing Shutdown after a directory move. Reuse the foundation's shutdown;
//! its observed run completion follows the MCP shutdown pass in Engine::run.

use super::*;
use deepseek_tui::core::engine::{EngineHandle, EngineRunCompletion};
use tokio::sync::{OwnedMutexGuard, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock, watch};

const RELEASE_TIMEOUT: Duration = Duration::from_secs(15);
const BUSY: &str = "marketplace_uninstall_busy";
const RELEASE_FAILED: &str = "marketplace_uninstall_release_failed";

#[derive(Clone, Default)]
pub(super) struct RuntimeMaintenance {
    blocked: Arc<SyncMutex<bool>>,
    spawning: Arc<RwLock<()>>,
    // Include engines already removed by idle reclaim/model changes: their
    // shutdown may still be queued, and they can still hold the package cwd.
    engines: Arc<SyncMutex<Vec<watch::Receiver<Option<EngineRunCompletion>>>>>,
}

pub(crate) struct UploadUninstallGuard {
    maintenance: RuntimeMaintenance,
    _spawning: Option<OwnedRwLockWriteGuard<()>>,
    turns: Vec<OwnedMutexGuard<()>>,
}

impl Drop for UploadUninstallGuard {
    fn drop(&mut self) {
        *self.maintenance.blocked.lock() = false;
    }
}

impl RuntimeMaintenance {
    fn ensure_idle(&self, lifecycles: &SessionTurnLifecycles) -> Result<(), String> {
        if lifecycles
            .states
            .lock()
            .values()
            .any(|lifecycle| lifecycle.is_active())
        {
            Err(BUSY.into())
        } else {
            Ok(())
        }
    }

    async fn request_idle_retirement(
        &self,
        handle: &EngineHandle,
        lifecycles: &SessionTurnLifecycles,
    ) -> Result<bool, String> {
        // Core readiness proves production/dequeue, not that the forwarder has
        // persisted and emitted the terminal. Check its owner after actor ready,
        // before commit; a refusal leaves the same cached actor usable.
        // Even NormalReturn cannot prove that a previously ended actor's events
        // were consumed. A failed handshake must not authorize forwarder abort.
        tokio::time::timeout(
            RELEASE_TIMEOUT,
            handle.shutdown_when_idle(|| self.ensure_idle(lifecycles).is_ok()),
        )
        .await
        .map_err(|_| RELEASE_FAILED.to_string())?
        .map_err(|_| RELEASE_FAILED.to_string())
    }
    pub(super) async fn read(&self) -> OwnedRwLockReadGuard<()> {
        self.spawning.clone().read_owned().await
    }

    pub(super) fn reserve(&self, lifecycle: &Arc<TurnLifecycle>) -> Result<TurnReservation> {
        // Serialize the check with reservation itself. A reservation winning
        // first is observed as active by the uninstaller; the opposite order
        // refuses before attachments/admission/turn state can be changed.
        let blocked = self.blocked.lock();
        if *blocked {
            bail!("session_turn_in_progress");
        }
        lifecycle.reserve()
    }

    pub(super) fn track(&self, handle: &EngineHandle) {
        let (completed, receipt) = watch::channel(None);
        let mut engines = self.engines.lock();
        engines.retain(|receipt| *receipt.borrow() != Some(EngineRunCompletion::NormalReturn));
        engines.push(receipt);
        // Retain only the receipt, never an op sender or event queue: observation
        // must not keep an otherwise unowned engine (or session content) alive.
        let completion = handle.wait_run_completion();
        tokio::spawn(async move {
            let outcome = completion.await;
            let _ = completed.send(Some(outcome));
        });
    }

    async fn begin(&self) -> Result<UploadUninstallGuard, String> {
        {
            let mut blocked = self.blocked.lock();
            if *blocked {
                return Err(BUSY.into());
            }
            *blocked = true;
        }
        // Own the reset before awaiting, so cancellation also reopens admission.
        let mut guard = UploadUninstallGuard {
            maintenance: self.clone(),
            _spawning: None,
            turns: Vec::new(),
        };
        guard._spawning = Some(
            tokio::time::timeout(RELEASE_TIMEOUT, self.spawning.clone().write_owned())
                .await
                .map_err(|_| RELEASE_FAILED.to_string())?,
        );
        Ok(guard)
    }

    async fn wait_for_shutdown(&self) -> Result<(), String> {
        let engines = self.engines.lock().clone();
        // One overall deadline, including channels of previously retired engines.
        tokio::time::timeout(RELEASE_TIMEOUT, async {
            for mut receipt in engines {
                loop {
                    let outcome = *receipt.borrow_and_update();
                    match outcome {
                        Some(EngineRunCompletion::NormalReturn) => break,
                        Some(_) => return Err(RELEASE_FAILED.to_string()),
                        None => receipt
                            .changed()
                            .await
                            .map_err(|_| RELEASE_FAILED.to_string())?,
                    }
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| RELEASE_FAILED.to_string())??;
        self.engines.lock().clear();
        Ok(())
    }
}

impl EnginePool {
    /// Called only for uploaded packages, before any uninstall state mutation.
    /// Keep this guard inside the blocking uninstall task: dropping the command
    /// future must not admit a new engine while that task is moving the package.
    pub(crate) async fn prepare_upload_uninstall(&self) -> Result<UploadUninstallGuard, String> {
        let pool = self.clone();
        finish_preparation(async move { pool.prepare_upload_uninstall_inner().await }).await
    }

    async fn prepare_upload_uninstall_inner(&self) -> Result<UploadUninstallGuard, String> {
        // Fail quickly for a known running task; the fenced recheck below is
        // still authoritative when a reservation races this advisory probe.
        self.marketplace_uninstall
            .ensure_idle(&self.turn_lifecycles)?;
        let mut guard = self.marketplace_uninstall.begin().await?;
        let mut session_ids: Vec<String> = self.entries.lock().await.keys().cloned().collect();
        session_ids.sort();
        for id in &session_ids {
            let turn_lock = self.turn_locks.for_session(id).await;
            guard
                .turns
                .push(turn_lock.try_lock_owned().map_err(|_| BUSY.to_string())?);
            if self.is_compaction_in_flight(id) {
                return Err(BUSY.into());
            }
        }
        // Check every reservation, even a cold session not yet in entries.
        // New reservations cannot race this check while blocked is set.
        self.marketplace_uninstall
            .ensure_idle(&self.turn_lifecycles)?;
        // App admission cannot observe every autonomous turn. Ask each actor
        // to seal its own complete idle boundary before using destructive
        // reclaim. A busy actor keeps its tasks, pending results and goal timer.
        for id in session_ids {
            let Some(engine) = self.handle_for(&id).await else {
                continue;
            };
            let may_retire = self
                .marketplace_uninstall
                .request_idle_retirement(&engine.handle, &self.turn_lifecycles)
                .await?;
            if !may_retire {
                return Err(BUSY.to_string());
            }
            // Never timeout/drop reclaim after it removes the entry. The
            // detached preparation worker owns all of its cleanup to the end.
            self.evict_locked(&id).await;
        }
        self.marketplace_uninstall.wait_for_shutdown().await?;
        Ok(guard)
    }
}

async fn finish_preparation(
    work: impl Future<Output = Result<UploadUninstallGuard, String>> + Send + 'static,
) -> Result<UploadUninstallGuard, String> {
    // Dropping the command's wait must not drop a reclaim halfway through,
    // after its entry was removed but before Shutdown was sent. The worker
    // keeps ownership; an unclaimed result drops its guard after cleanup.
    tokio::spawn(work)
        .await
        .map_err(|_| RELEASE_FAILED.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn upload_uninstall_blocks_boot_and_admission_and_preserves_active_turns() {
        let maintenance = RuntimeMaintenance::default();
        let lifecycles = SessionTurnLifecycles::default();
        let lifecycle = lifecycles.for_session("cold-reservation");
        let reservation = maintenance.reserve(&lifecycle).unwrap();
        let guard = maintenance.begin().await.unwrap();
        assert_eq!(maintenance.ensure_idle(&lifecycles).unwrap_err(), BUSY);
        assert!(
            lifecycle.is_active(),
            "uninstall must not cancel an active turn"
        );
        assert!(
            maintenance
                .reserve(&lifecycles.for_session("new-turn"))
                .is_err()
        );
        assert!(maintenance.spawning.clone().try_read_owned().is_err());
        drop(guard);
        drop(reservation);
        assert!(maintenance.reserve(&lifecycle).is_ok());
        assert!(maintenance.spawning.clone().try_read_owned().is_ok());
    }

    #[tokio::test]
    async fn upload_uninstall_cancellation_reopens_admission() {
        let maintenance = RuntimeMaintenance::default();
        let reader = maintenance.read().await;
        let state = maintenance.clone();
        let begin = tokio::spawn(async move { state.begin().await });
        tokio::task::yield_now().await;
        assert!(*maintenance.blocked.lock());
        begin.abort();
        match begin.await {
            Err(error) => assert!(error.is_cancelled()),
            Ok(_) => panic!("cancelled preflight must not complete"),
        }
        assert!(!*maintenance.blocked.lock());
        drop(reader);
        assert!(maintenance.begin().await.is_ok());
    }

    #[tokio::test]
    async fn upload_uninstall_command_cancellation_does_not_abandon_cleanup() {
        let maintenance = RuntimeMaintenance::default();
        let state = maintenance.clone();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let command = tokio::spawn(finish_preparation(async move {
            let guard = state.begin().await?;
            entered_tx.send(()).unwrap();
            release_rx.await.unwrap();
            Ok(guard)
        }));
        entered_rx.await.unwrap();
        command.abort();
        assert!(command.await.is_err());
        assert!(
            *maintenance.blocked.lock(),
            "cleanup still owns admission after the caller leaves"
        );
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while *maintenance.blocked.lock() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("finished unclaimed cleanup must release its guard");
        assert!(maintenance.begin().await.is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn upload_uninstall_does_not_treat_shutdown_enqueue_as_exit() {
        let maintenance = RuntimeMaintenance::default();
        let (completed, receipt) = watch::channel(None);
        maintenance.engines.lock().push(receipt);
        assert_eq!(
            maintenance.wait_for_shutdown().await.unwrap_err(),
            RELEASE_FAILED
        );
        completed
            .send(Some(EngineRunCompletion::NormalReturn))
            .unwrap();
        maintenance.wait_for_shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn upload_uninstall_preserves_dequeued_terminal_until_host_finishes() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = deepseek_tui::core::engine::EngineConfig {
            workspace: directory.path().to_owned(),
            ..Default::default()
        };
        config
            .features
            .disable(deepseek_tui::features::Feature::Mcp);
        let handle = deepseek_tui::core::engine::spawn_engine(
            config,
            &deepseek_tui::config::Config::default(),
        );
        let maintenance = RuntimeMaintenance::default();
        let lifecycles = SessionTurnLifecycles::default();
        let lifecycle = lifecycles.for_session("dequeued-terminal");
        maintenance.ensure_idle(&lifecycles).unwrap();
        let _guard = maintenance.begin().await.unwrap();
        handle.get_session_snapshot().await.unwrap();
        while handle.rx_event.write().await.try_recv().is_ok() {}

        // Same owner transitions used by the real forwarder: an autonomous
        // start arrives after preflight; receiving TurnComplete has not yet
        // finished persistence/finalization. This exercises the production
        // helper and predicate, without mocking the actor's handshake.
        lifecycle.on_started("autonomous-terminal".into());
        assert!(lifecycle.is_active());
        assert!(
            !maintenance
                .request_idle_retirement(&handle, &lifecycles)
                .await
                .unwrap()
        );
        handle
            .get_session_snapshot()
            .await
            .expect("busy refusal must keep the cached actor usable");
        assert!(lifecycle.claim_terminal().is_some());
        assert!(
            lifecycle.is_active(),
            "terminal delivery still owns the closing window"
        );
        assert!(
            !maintenance
                .request_idle_retirement(&handle, &lifecycles)
                .await
                .unwrap()
        );
        handle
            .get_session_snapshot()
            .await
            .expect("closing refusal must not seal a dead cached handle");

        lifecycle.finish_terminal_emission();
        assert!(
            maintenance
                .request_idle_retirement(&handle, &lifecycles)
                .await
                .unwrap()
        );
        assert_eq!(
            handle.wait_run_completion().await,
            EngineRunCompletion::NormalReturn
        );
        // Normal producer completion alone cannot certify an unknown consumer.
        assert_eq!(
            maintenance
                .request_idle_retirement(&handle, &lifecycles)
                .await
                .unwrap_err(),
            RELEASE_FAILED
        );
    }

    #[tokio::test]
    async fn upload_uninstall_receipt_does_not_retain_engine_event_queue() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = deepseek_tui::core::engine::EngineConfig {
            workspace: directory.path().to_owned(),
            ..Default::default()
        };
        config
            .features
            .disable(deepseek_tui::features::Feature::Mcp);
        let handle = deepseek_tui::core::engine::spawn_engine(
            config,
            &deepseek_tui::config::Config::default(),
        );
        let events = Arc::downgrade(&handle.rx_event);
        let receipt = handle.wait_run_completion();
        handle.send(Op::Shutdown).await.unwrap();
        drop(handle);
        assert!(
            events.upgrade().is_none(),
            "a receipt must not retain the handle's event queue"
        );
        assert_eq!(
            tokio::time::timeout(RELEASE_TIMEOUT, receipt)
                .await
                .unwrap(),
            EngineRunCompletion::NormalReturn
        );
    }

    #[tokio::test]
    async fn upload_uninstall_rejects_abnormal_or_unobserved_shutdown() {
        for outcome in [
            EngineRunCompletion::AbnormalTermination,
            EngineRunCompletion::NotObserved,
        ] {
            let maintenance = RuntimeMaintenance::default();
            let (_completed, receipt) = watch::channel(Some(outcome));
            maintenance.engines.lock().push(receipt);
            assert_eq!(
                maintenance.wait_for_shutdown().await.unwrap_err(),
                RELEASE_FAILED
            );
        }
    }

    struct LiveMcp {
        handle: deepseek_tui::core::engine::EngineHandle,
        maintenance: RuntimeMaintenance,
        package: std::path::PathBuf,
        recycled: std::path::PathBuf,
        ready: std::path::PathBuf,
        allow: std::path::PathBuf,
        _directory: tempfile::TempDir,
    }

    impl Drop for LiveMcp {
        fn drop(&mut self) {
            // Also release the handshake if an assertion panics, so the
            // fixture cannot leave an indefinitely booting child behind.
            let _ = std::fs::write(&self.allow, b"continue");
            let _ = self.handle.tx_op.try_send(Op::Shutdown);
        }
    }

    fn live_mcp(gated: bool) -> LiveMcp {
        let directory = tempfile::tempdir().unwrap();
        let package = directory.path().join("bundles/text-to-image");
        let cwd = package.join("mcp");
        let recycled = directory.path().join("recycle-bin/text-to-image");
        let ready = directory.path().join("handshake-started");
        let allow = directory.path().join("allow-handshake");
        if !gated {
            std::fs::write(&allow, b"continue").unwrap();
        }
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(recycled.parent().unwrap()).unwrap();
        let script = cwd.join("server.py");
        std::fs::write(&script, r#"
import json, pathlib, sys, time
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    if request['method'] == 'initialize':
        pathlib.Path(sys.argv[1]).write_text('ready')
        while not pathlib.Path(sys.argv[2]).exists():
            time.sleep(0.01)
        result = {'protocolVersion': '2024-11-05', 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'fixture', 'version': '1'}}
    elif request['method'] == 'tools/list':
        result = {'tools': [{'name': 'fixture', 'description': 'fixture', 'inputSchema': {'type': 'object'}}]}
    else:
        result = {}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#).unwrap();
        let config_path = directory.path().join("mcp.json");
        std::fs::write(
            &config_path,
            serde_json::to_vec(&serde_json::json!({
                "servers": {"text-to-image": {
                    "command": crate::platform::paths::python_command(),
                    "args": ["-u", script, ready, allow], "cwd": cwd
                }}
            }))
            .unwrap(),
        )
        .unwrap();
        let mut config = deepseek_tui::core::engine::EngineConfig {
            workspace: directory.path().to_owned(),
            mcp_config_path: config_path,
            ..Default::default()
        };
        config.features.enable(deepseek_tui::features::Feature::Mcp);
        let handle = deepseek_tui::core::engine::spawn_engine(
            config,
            &deepseek_tui::config::Config::default(),
        );
        let maintenance = RuntimeMaintenance::default();
        maintenance.track(&handle);
        LiveMcp {
            handle,
            maintenance,
            package,
            recycled,
            ready,
            allow,
            _directory: directory,
        }
    }

    #[tokio::test]
    async fn upload_uninstall_waits_for_live_mcp_cwd_before_moving_package() {
        let fixture = live_mcp(false);
        let snapshot = tokio::time::timeout(RELEASE_TIMEOUT, fixture.handle.bootstrap_mcp())
            .await
            .unwrap()
            .unwrap()
            .snapshot;
        assert!(
            snapshot
                .servers
                .iter()
                .any(|server| server.name == "text-to-image"
                    && server.connected
                    && !server.tools.is_empty()),
            "the fixture MCP must really be running before uninstall"
        );
        // Windows rejects moving this parent while the child's cwd is open.
        // POSIX allows it; restore it so both exercise the same teardown.
        match std::fs::rename(&fixture.package, &fixture.recycled) {
            Ok(()) => std::fs::rename(&fixture.recycled, &fixture.package).unwrap(),
            Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied),
        }
        let _guard = fixture.maintenance.begin().await.unwrap();
        fixture.handle.send(Op::Shutdown).await.unwrap();
        fixture.maintenance.wait_for_shutdown().await.unwrap();
        std::fs::rename(&fixture.package, &fixture.recycled)
            .expect("MCP cwd must be released before recycling");
        assert!(fixture.recycled.join("mcp/server.py").is_file());
    }

    #[tokio::test]
    async fn upload_uninstall_waits_for_mcp_that_is_still_starting() {
        let fixture = live_mcp(true);
        tokio::time::timeout(RELEASE_TIMEOUT, async {
            while !fixture.ready.is_file() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fixture MCP must enter its gated handshake");
        let _guard = fixture.maintenance.begin().await.unwrap();
        fixture.handle.send(Op::Shutdown).await.unwrap();
        let mut shutdown = Box::pin(fixture.maintenance.wait_for_shutdown());
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut shutdown)
                .await
                .is_err(),
            "pending MCP boot must not outlive the engine's shutdown receipt"
        );
        std::fs::write(&fixture.allow, b"continue").unwrap();
        shutdown.await.unwrap();
        std::fs::rename(&fixture.package, &fixture.recycled)
            .expect("late-boot MCP must also release its cwd");
    }
}
