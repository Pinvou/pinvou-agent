use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{classify_pressure, PinvouOsRuntime, ResourceGovernorPolicy, ResourceObservation};

pub type ResourceSampler = Arc<dyn Fn() -> ResourceObservation + Send + Sync + 'static>;

/// 启动常驻 Resource Agent。
///
/// 采样持续发生；为避免稳定状态每 5 秒永久膨胀账本，只在压力等级变化时立即写入，
/// 或至少每 30 秒写一条心跳证据。该节流不经过模型，也不改变 Governor 的硬阈值。
pub fn spawn_resource_agent(
    runtime: PinvouOsRuntime,
    sampler: ResourceSampler,
    cadence: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let cadence = cadence.max(Duration::from_secs(1));
        let mut ticker = tokio::time::interval(cadence);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_persisted = None::<Instant>;
        loop {
            ticker.tick().await;
            let sampler = sampler.clone();
            let observation = match tokio::task::spawn_blocking(move || sampler()).await {
                Ok(observation) => observation,
                Err(error) => {
                    log::warn!("PinvouOS Resource Agent sampler failed: {error}");
                    continue;
                }
            };
            let pressure = classify_pressure(&observation, ResourceGovernorPolicy::default());
            let pressure_changed = pressure != runtime.snapshot().resources.pressure;
            let heartbeat_due =
                last_persisted.is_none_or(|instant| instant.elapsed() >= Duration::from_secs(30));
            if pressure_changed || heartbeat_due {
                match runtime.observe_resources(observation) {
                    Ok(_) => last_persisted = Some(Instant::now()),
                    Err(error) => {
                        log::warn!("PinvouOS Resource Agent observation failed: {error:#}")
                    }
                }
            }
        }
    })
}
