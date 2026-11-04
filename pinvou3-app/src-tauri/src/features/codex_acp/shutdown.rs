//! Terminal cancellation for pool admission and unpublished startup owners.

use std::future::Future;

use anyhow::{Result, bail};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
pub(super) struct PoolShutdown {
    cancelled: CancellationToken,
}

impl PoolShutdown {
    pub(super) fn cancel(&self) {
        self.cancelled.cancel();
    }

    /// Cancellation drops the operation before returning. In particular,
    /// spawn_session owns its kill_on_drop Child and connection shutdown
    /// sender until the runtime is published under the pool lock.
    pub(super) async fn run<T>(&self, operation: impl Future<Output = T>) -> Result<T> {
        tokio::select! {
            biased;
            () = self.cancelled.cancelled() => bail!("acp_pool_shutting_down"),
            result = operation => Ok(result),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use tokio::sync::{Mutex, oneshot};

    use super::PoolShutdown;

    struct DropOwner(Arc<AtomicBool>);

    impl Drop for DropOwner {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_drops_pending_startup_and_releases_the_collector_lock() {
        let shutdown = PoolShutdown::default();
        let sessions = Arc::new(Mutex::new(()));
        let dropped = Arc::new(AtomicBool::new(false));
        let (started, ready) = oneshot::channel();
        let worker_shutdown = shutdown.clone();
        let worker_sessions = sessions.clone();
        let worker_dropped = dropped.clone();
        let worker = tokio::spawn(async move {
            let _guard = worker_shutdown.run(worker_sessions.lock()).await.unwrap();
            worker_shutdown
                .run(async move {
                    let _owner = DropOwner(worker_dropped);
                    let _ = started.send(());
                    std::future::pending::<()>().await;
                })
                .await
        });
        ready.await.unwrap();
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(1), async {
            assert!(worker.await.unwrap().is_err());
            let _collector = sessions.lock().await;
            assert!(dropped.load(Ordering::Acquire));
        })
        .await
        .expect("shutdown must not depend on the startup response");
    }

    #[tokio::test]
    async fn shutdown_is_shared_and_rejects_late_ready_operations() {
        let shutdown = PoolShutdown::default();
        let clone = shutdown.clone();
        shutdown.cancel();
        shutdown.cancel();
        let mut polled = false;
        assert!(clone.run(async { polled = true }).await.is_err());
        assert!(!polled, "a late startup must not spawn or publish a child");
    }

    #[tokio::test]
    async fn normal_operations_keep_their_results_and_allow_the_next_call() {
        let shutdown = PoolShutdown::default();
        assert_eq!(shutdown.run(async { 7 }).await.unwrap(), 7);
        assert_eq!(
            shutdown
                .run(async { Err::<(), _>("peer error") })
                .await
                .unwrap(),
            Err("peer error")
        );
        assert_eq!(shutdown.run(async { 8 }).await.unwrap(), 8);
    }

    #[tokio::test(start_paused = true)]
    async fn a_waiter_can_cancel_without_acquiring_the_sessions_lock() {
        let shutdown = PoolShutdown::default();
        let sessions = Mutex::new(());
        let _owner = sessions.lock().await;
        let waiting = shutdown.run(sessions.lock());
        tokio::pin!(waiting);
        tokio::select! {
            biased;
            _ = &mut waiting => panic!("lock is still owned"),
            () = std::future::ready(()) => {}
        }
        shutdown.cancel();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), waiting)
                .await
                .unwrap()
                .is_err()
        );
        assert!(sessions.try_lock().is_err());
    }
}
