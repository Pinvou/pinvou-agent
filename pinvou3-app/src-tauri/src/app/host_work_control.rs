//! Resource Governor 到受信 HostWork Adapter 的异步执行闭环。
//!
//! 本模块位于 app 组合层：PinvouOS feature 只拥有确定性账本与 Governor，具体
//! Scheduled/Knowledge/Connector/Supervisor 依赖不能反向进入领域层。每个静态工作有
//! 独立 worker；任何一个 adapter 卡住都不会占用资源采样调用栈或其他 adapter worker。

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use parking_lot::RwLock;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

use crate::features::assistant::engine_pool::EnginePool;
use crate::features::connectors::connector_cli::ConnectorConn;
use crate::features::knowledge::KnowledgeService;
use crate::features::pinvou_os::{
    AppCgroupResourceObservation, HostWork, HostWorkAction, HostWorkDirective,
    HostWorkDirectiveAcknowledgement, HostWorkDirectiveStatus, HostWorkDispatchRecord,
    HostWorkHandle, HostWorkKind, HostWorkObservedState, HostWorkReconciliationOutcome,
    Interruptibility, PinvouOsRuntime, ReconcileHostWorkDirectiveRequest, RegisterHostWorkRequest,
    ResourceClass,
};
use crate::features::scheduled::tasks::ScheduledTaskState;

const OWNER_SCHEDULED: &str = "host:scheduled-aggregate";
const OWNER_DETACHED_SUBAGENTS: &str = "host:detached-subagents-aggregate";
const OWNER_KNOWLEDGE: &str = "host:knowledge-aggregate";
const OWNER_CONNECTORS: &str = "host:connectors-aggregate";
const OWNER_APP_CGROUP: &str = "host:supervisor-app";
const OWNER_ASR_CGROUP: &str = "host:supervisor-asr";
const APP_CGROUP_CACHE_MAX_AGE_MS: i64 = 15_000;
const APP_CGROUP_CACHE_FUTURE_SKEW_MS: i64 = 5_000;

/// Supervisor worker 单写、Resource sampler 只读的进程内桥。读取只使用 `try_read`：
/// writer 短暂更新时直接返回缺失，绝不让 5 秒采样循环等待外部 I/O 或锁。
#[derive(Clone, Default)]
pub(crate) struct AppCgroupTelemetryCache {
    inner: Arc<RwLock<AppCgroupTelemetryCacheState>>,
}

#[derive(Default)]
struct AppCgroupTelemetryCacheState {
    trusted: bool,
    observation: Option<AppCgroupResourceObservation>,
}

impl AppCgroupTelemetryCache {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn read_for_resource_sampler(
        &self,
        sampled_at_ms: i64,
    ) -> Option<AppCgroupResourceObservation> {
        let state = self.inner.try_read()?;
        if !state.trusted {
            return None;
        }
        let observation = state.observation.as_ref()?;
        if observation.observed_at_ms <= 0
            || observation.observed_at_ms
                > sampled_at_ms.saturating_add(APP_CGROUP_CACHE_FUTURE_SKEW_MS)
            || sampled_at_ms.saturating_sub(observation.observed_at_ms)
                > APP_CGROUP_CACHE_MAX_AGE_MS
        {
            return None;
        }
        Some(observation.clone())
    }

    fn publish(&self, observation: AppCgroupResourceObservation) {
        let mut state = self.inner.write();
        state.observation = Some(observation);
        state.trusted = true;
    }

    fn mark_stale(&self) {
        // 保留旧绝对计数仅供下一次受信状态继续比较；reader 看到 trusted=false 后
        // 不会把旧值注入采样，更不会用一次超时伪造恢复。
        self.inner.write().trusted = false;
    }
}

#[derive(Clone)]
struct HostWorkSpec {
    owner: &'static str,
    kind: HostWorkKind,
    resource_class: ResourceClass,
    priority: u8,
    interruptibility: Interruptibility,
    essential: bool,
    governable: bool,
    supported_actions: BTreeSet<HostWorkAction>,
}

impl HostWorkSpec {
    fn registration(&self) -> RegisterHostWorkRequest {
        RegisterHostWorkRequest {
            owner: self.owner.to_string(),
            kind: self.kind,
            resource_class: self.resource_class,
            priority: self.priority,
            interruptibility: self.interruptibility,
            essential: self.essential,
            governable: self.governable,
            supported_actions: self.supported_actions.clone(),
            // 注册先建立可信身份；worker 随后用 adapter status 写真实状态。Runtime 不允许
            // 把 terminal state 直接注册为 live，这也让重启后的 stopped→running 能通过
            // generation renew 明确表达为新实例。
            initial_observed_state: HostWorkObservedState::Unknown,
        }
    }

    fn stop_only(
        owner: &'static str,
        kind: HostWorkKind,
        resource_class: ResourceClass,
        priority: u8,
        interruptibility: Interruptibility,
    ) -> Self {
        Self {
            owner,
            kind,
            resource_class,
            priority,
            interruptibility,
            essential: false,
            governable: true,
            supported_actions: BTreeSet::from([HostWorkAction::Stop]),
        }
    }
}

#[derive(Debug, Clone)]
struct AdapterObservation {
    state: HostWorkObservedState,
    detail: String,
}

impl AdapterObservation {
    fn new(state: HostWorkObservedState, detail: impl Into<String>) -> Self {
        Self {
            state,
            detail: bounded_detail(detail),
        }
    }

    fn unknown(detail: impl Into<String>) -> Self {
        Self::new(HostWorkObservedState::Unknown, detail)
    }
}

#[derive(Debug, Clone)]
struct AdapterAcknowledgement {
    kind: HostWorkDirectiveAcknowledgement,
    detail: String,
}

impl AdapterAcknowledgement {
    fn new(kind: HostWorkDirectiveAcknowledgement, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: bounded_detail(detail),
        }
    }

    fn applied(detail: impl Into<String>) -> Self {
        Self::new(HostWorkDirectiveAcknowledgement::Applied, detail)
    }

    fn rejected(detail: impl Into<String>) -> Self {
        Self::new(HostWorkDirectiveAcknowledgement::Rejected, detail)
    }

    fn outcome_unknown(detail: impl Into<String>) -> Self {
        Self::new(HostWorkDirectiveAcknowledgement::OutcomeUnknown, detail)
    }
}

#[async_trait]
trait TrustedHostWorkAdapter: Send + Sync {
    fn spec(&self) -> &HostWorkSpec;

    async fn status(&self) -> AdapterObservation;

    fn mark_status_stale(&self) {}

    async fn apply(&self, action: HostWorkAction, directive_id: &str) -> AdapterAcknowledgement;
}

#[derive(Debug, Clone, Copy)]
struct WorkerConfig {
    poll_interval: Duration,
    action_timeout: Duration,
    status_timeout: Duration,
    reconciliation_grace: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(1),
            action_timeout: Duration::from_secs(60),
            status_timeout: Duration::from_secs(10),
            reconciliation_grace: Duration::from_secs(10),
        }
    }
}

/// Tauri State 只负责持有 worker 生命周期，不暴露任何 command 或 Renderer 写入口。
pub(crate) struct HostWorkControlPlane {
    cancel: CancellationToken,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for HostWorkControlPlane {
    fn drop(&mut self) {
        self.cancel.cancel();
        for task in &self.tasks {
            task.abort();
        }
    }
}

pub(crate) fn spawn_trusted_host_work_control(
    app: AppHandle,
    runtime: PinvouOsRuntime,
    app_cgroup_telemetry: AppCgroupTelemetryCache,
) -> Result<HostWorkControlPlane> {
    start_control_plane(
        runtime,
        production_adapters(app, app_cgroup_telemetry),
        WorkerConfig::default(),
    )
}

fn start_control_plane(
    runtime: PinvouOsRuntime,
    adapters: Vec<Arc<dyn TrustedHostWorkAdapter>>,
    config: WorkerConfig,
) -> Result<HostWorkControlPlane> {
    // 所有绑定先同步完成，Resource Agent 才能启动并在第一次采样就看到完整 Registry。
    // status/控制 I/O 不在这里执行，setup 主线程不会等待外部 adapter。
    let mut bindings = Vec::with_capacity(adapters.len());
    for adapter in adapters {
        let handle = bind_registration(&runtime, adapter.spec())?;
        bindings.push((adapter, handle));
    }

    let cancel = CancellationToken::new();
    let mut tasks = Vec::with_capacity(bindings.len());
    for (adapter, handle) in bindings {
        let runtime = runtime.clone();
        let worker_cancel = cancel.child_token();
        tasks.push(tauri::async_runtime::spawn(async move {
            run_worker(runtime, adapter, handle, worker_cancel, config).await;
        }));
    }
    Ok(HostWorkControlPlane { cancel, tasks })
}

fn bind_registration(runtime: &PinvouOsRuntime, spec: &HostWorkSpec) -> Result<HostWorkHandle> {
    let matches = runtime
        .snapshot()
        .host_works
        .values()
        .filter(|work| work.owner == spec.owner && work.kind == spec.kind)
        .cloned()
        .collect::<Vec<_>>();
    let handle = match matches.as_slice() {
        [] => runtime
            .register_host_work(spec.registration())
            .map(|(handle, _)| handle),
        [work] => {
            validate_rebind_contract(work, spec)?;
            runtime
                .rebind_host_work(spec.owner, spec.kind, work.generation)
                .map(|(handle, _)| handle)
        }
        _ => bail!(
            "HostWork registration key {} + {:?} is not unique",
            spec.owner,
            spec.kind
        ),
    }?;
    reconcile_bound_governance(runtime, &handle, spec.owner);
    Ok(handle)
}

fn reconcile_bound_governance(runtime: &PinvouOsRuntime, handle: &HostWorkHandle, owner: &str) {
    // 只让 Runtime 基于最近可信 pressure 签 Pending；此处绝不内联调用 adapter。
    if let Err(error) = runtime.reconcile_host_work_governance(handle) {
        log::warn!("[pinvou-os][host-work:{owner}] governance reconciliation failed: {error:#}");
    }
}

fn validate_rebind_contract(work: &HostWork, spec: &HostWorkSpec) -> Result<()> {
    if work.resource_class != spec.resource_class
        || work.priority != spec.priority
        || work.interruptibility != spec.interruptibility
        || work.essential != spec.essential
        || work.governable != spec.governable
        || work.supported_actions != spec.supported_actions
    {
        bail!(
            "HostWork {} registration contract changed without a generation migration",
            spec.owner
        );
    }
    Ok(())
}

async fn run_worker(
    runtime: PinvouOsRuntime,
    adapter: Arc<dyn TrustedHostWorkAdapter>,
    mut handle: HostWorkHandle,
    cancel: CancellationToken,
    config: WorkerConfig,
) {
    let mut attempted = HashMap::<String, AdapterAcknowledgement>::new();
    let mut reconciliation_started = HashMap::<String, Instant>::new();

    loop {
        if let Err(error) = drive_worker_once(
            &runtime,
            adapter.as_ref(),
            &mut handle,
            &mut attempted,
            &mut reconciliation_started,
            config,
        )
        .await
        {
            log::warn!(
                "[pinvou-os][host-work:{}] control worker iteration failed: {error:#}",
                adapter.spec().owner
            );
        }

        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(config.poll_interval) => {}
        }
    }
}

async fn drive_worker_once(
    runtime: &PinvouOsRuntime,
    adapter: &dyn TrustedHostWorkAdapter,
    handle: &mut HostWorkHandle,
    attempted: &mut HashMap<String, AdapterAcknowledgement>,
    reconciliation_started: &mut HashMap<String, Instant>,
    config: WorkerConfig,
) -> Result<()> {
    let mut pending = runtime.pending_host_work_directives(handle)?;
    pending.sort_by(|left, right| {
        left.issued_at_ms
            .cmp(&right.issued_at_ms)
            .then_with(|| left.directive_id.cmp(&right.directive_id))
    });
    for directive in pending {
        let acknowledgement = match attempted.get(&directive.directive_id) {
            Some(cached) => cached.clone(),
            None => {
                // durable marker 必须在 Runtime 锁内 append+fsync 完成；真正 adapter I/O
                // 只在锁外执行。任何已有 marker 或旧 boot 遗留 Pending 都只能 status-only，
                // 不能把“没有 ACK”误当成“肯定没有发生副作用”。
                let acknowledgement = match runtime.record_host_work_directive_dispatch(
                    handle,
                    &directive.directive_id,
                )? {
                    HostWorkDispatchRecord::NewlyRecorded => {
                        call_apply(adapter, &directive, config).await
                    }
                    HostWorkDispatchRecord::AlreadyRecorded => {
                        AdapterAcknowledgement::outcome_unknown(
                            "durable dispatch was already recorded; action was not replayed",
                        )
                    }
                    HostWorkDispatchRecord::InheritedPending => {
                        AdapterAcknowledgement::outcome_unknown(
                            "pending directive was inherited from a prior runtime; action was not replayed",
                        )
                    }
                };
                attempted.insert(directive.directive_id.clone(), acknowledgement.clone());
                acknowledgement
            }
        };
        runtime.acknowledge_host_work_directive(
            handle,
            &directive.directive_id,
            acknowledgement.kind,
            acknowledgement.detail.clone(),
        )?;
        attempted.remove(&directive.directive_id);
    }

    let mut requiring_reconciliation =
        runtime.host_work_directives_requiring_reconciliation(handle)?;
    requiring_reconciliation.sort_by(|left, right| {
        left.issued_at_ms
            .cmp(&right.issued_at_ms)
            .then_with(|| left.directive_id.cmp(&right.directive_id))
    });
    for directive in &requiring_reconciliation {
        reconcile_directive(
            runtime,
            adapter,
            handle,
            directive,
            reconciliation_started,
            config,
        )
        .await?;
    }

    // 普通观测不能越过任何 unresolved directive；reconcile 结果会在下一轮重新读取。
    if !runtime.pending_host_work_directives(handle)?.is_empty()
        || !runtime
            .host_work_directives_requiring_reconciliation(handle)?
            .is_empty()
    {
        return Ok(());
    }

    let observation = call_status(adapter, config).await;
    sync_normal_observation(runtime, handle, observation)?;
    Ok(())
}

async fn call_apply(
    adapter: &dyn TrustedHostWorkAdapter,
    directive: &HostWorkDirective,
    config: WorkerConfig,
) -> AdapterAcknowledgement {
    match tokio::time::timeout(
        config.action_timeout,
        adapter.apply(directive.action, &directive.directive_id),
    )
    .await
    {
        Ok(acknowledgement) => acknowledgement,
        Err(_) => AdapterAcknowledgement::outcome_unknown("adapter action timed out"),
    }
}

async fn call_status(
    adapter: &dyn TrustedHostWorkAdapter,
    config: WorkerConfig,
) -> AdapterObservation {
    match tokio::time::timeout(config.status_timeout, adapter.status()).await {
        Ok(observation) => observation,
        Err(_) => {
            adapter.mark_status_stale();
            AdapterObservation::unknown("adapter status timed out")
        }
    }
}

async fn reconcile_directive(
    runtime: &PinvouOsRuntime,
    adapter: &dyn TrustedHostWorkAdapter,
    handle: &HostWorkHandle,
    directive: &HostWorkDirective,
    reconciliation_started: &mut HashMap<String, Instant>,
    config: WorkerConfig,
) -> Result<()> {
    let started = *reconciliation_started
        .entry(directive.directive_id.clone())
        .or_insert_with(Instant::now);
    let observation = call_status(adapter, config).await;
    let target = action_target_state(directive.action);
    let (outcome, observed_state, detail) = if observation.state == target {
        (
            HostWorkReconciliationOutcome::Confirmed,
            Some(observation.state),
            format!("directive target confirmed: {}", observation.detail),
        )
    } else if observation.state == HostWorkObservedState::Unknown
        || started.elapsed() < config.reconciliation_grace
    {
        (
            HostWorkReconciliationOutcome::OutcomeUnknown,
            None,
            format!("directive outcome remains unknown: {}", observation.detail),
        )
    } else {
        (
            HostWorkReconciliationOutcome::NotApplied,
            Some(observation.state),
            format!("directive target not observed: {}", observation.detail),
        )
    };
    let reconciled = runtime.reconcile_host_work_directive(
        handle,
        &directive.directive_id,
        ReconcileHostWorkDirectiveRequest {
            outcome,
            observed_state,
            detail: bounded_detail(detail),
        },
    )?;
    if matches!(
        reconciled.status,
        HostWorkDirectiveStatus::Reconciled | HostWorkDirectiveStatus::Rejected
    ) {
        reconciliation_started.remove(&directive.directive_id);
    }
    Ok(())
}

fn sync_normal_observation(
    runtime: &PinvouOsRuntime,
    handle: &mut HostWorkHandle,
    observation: AdapterObservation,
) -> Result<()> {
    let work = runtime
        .snapshot()
        .host_works
        .get(handle.work_id())
        .filter(|work| work.generation == handle.generation())
        .cloned()
        .ok_or_else(|| anyhow!("HostWork projection disappeared"))?;
    if work.observed_state == observation.state {
        return Ok(());
    }

    if is_terminal(work.observed_state) {
        // generation 内的终态不可逆。Unknown 不是新实例证据，另一个终态也不能改写
        // 已落账事实；只有 adapter 明确观测到 live 状态时才 renew generation。
        if matches!(
            observation.state,
            HostWorkObservedState::Starting
                | HostWorkObservedState::Running
                | HostWorkObservedState::Paused
        ) {
            let owner = work.owner.clone();
            let (renewed, _) = runtime.renew_host_work_registration(handle, observation.state)?;
            *handle = renewed;
            reconcile_bound_governance(runtime, handle, &owner);
        }
        return Ok(());
    }

    runtime.observe_host_work(handle, observation.state, observation.detail)?;
    Ok(())
}

fn is_terminal(state: HostWorkObservedState) -> bool {
    matches!(
        state,
        HostWorkObservedState::Stopped
            | HostWorkObservedState::Completed
            | HostWorkObservedState::Failed
    )
}

fn action_target_state(action: HostWorkAction) -> HostWorkObservedState {
    match action {
        HostWorkAction::Pause => HostWorkObservedState::Paused,
        HostWorkAction::Stop => HostWorkObservedState::Stopped,
        HostWorkAction::Resume => HostWorkObservedState::Running,
    }
}

fn bounded_detail(detail: impl Into<String>) -> String {
    let detail = detail.into();
    let trimmed = detail.trim();
    if trimmed.is_empty() {
        return "adapter did not provide detail".to_string();
    }
    trimmed.chars().take(500).collect()
}

fn scheduled_spec() -> HostWorkSpec {
    HostWorkSpec::stop_only(
        OWNER_SCHEDULED,
        HostWorkKind::ScheduledRun,
        ResourceClass::Heavy,
        40,
        Interruptibility::Checkpoint,
    )
}

fn detached_subagents_spec() -> HostWorkSpec {
    HostWorkSpec::stop_only(
        OWNER_DETACHED_SUBAGENTS,
        HostWorkKind::DetachedSubAgent,
        ResourceClass::Moderate,
        50,
        Interruptibility::Immediate,
    )
}

fn knowledge_spec() -> HostWorkSpec {
    HostWorkSpec::stop_only(
        OWNER_KNOWLEDGE,
        HostWorkKind::KnowledgeJob,
        ResourceClass::Heavy,
        30,
        Interruptibility::Checkpoint,
    )
}

fn connectors_spec() -> HostWorkSpec {
    HostWorkSpec::stop_only(
        OWNER_CONNECTORS,
        HostWorkKind::ConnectorJob,
        ResourceClass::Moderate,
        40,
        Interruptibility::Immediate,
    )
}

fn app_cgroup_spec() -> HostWorkSpec {
    HostWorkSpec {
        owner: OWNER_APP_CGROUP,
        kind: HostWorkKind::AppCgroup,
        resource_class: ResourceClass::Heavy,
        priority: 100,
        interruptibility: Interruptibility::Atomic,
        essential: true,
        governable: false,
        supported_actions: BTreeSet::new(),
    }
}

fn asr_cgroup_spec() -> HostWorkSpec {
    HostWorkSpec::stop_only(
        OWNER_ASR_CGROUP,
        HostWorkKind::AsrCgroup,
        ResourceClass::Heavy,
        70,
        Interruptibility::Checkpoint,
    )
}

fn production_adapters(
    app: AppHandle,
    app_cgroup_telemetry: AppCgroupTelemetryCache,
) -> Vec<Arc<dyn TrustedHostWorkAdapter>> {
    let mut adapters: Vec<Arc<dyn TrustedHostWorkAdapter>> = vec![
        Arc::new(ScheduledAdapter {
            app: app.clone(),
            spec: scheduled_spec(),
        }),
        Arc::new(DetachedSubagentsAdapter {
            app: app.clone(),
            spec: detached_subagents_spec(),
        }),
        Arc::new(KnowledgeAdapter {
            app: app.clone(),
            spec: knowledge_spec(),
        }),
        Arc::new(ConnectorsAdapter {
            app: app.clone(),
            spec: connectors_spec(),
        }),
    ];
    if std::env::consts::OS == "linux" {
        use crate::platform::host_supervisor::{HostSupervisorClient, ManagedHostWork};

        adapters.push(Arc::new(SupervisorAdapter {
            client: HostSupervisorClient::new(),
            target: ManagedHostWork::PinvouAsr,
            spec: asr_cgroup_spec(),
            app_cgroup_telemetry: None,
        }));
        adapters.push(Arc::new(SupervisorAdapter {
            client: HostSupervisorClient::new(),
            target: ManagedHostWork::PinvouApp,
            spec: app_cgroup_spec(),
            app_cgroup_telemetry: Some(app_cgroup_telemetry),
        }));
    }
    adapters
}

struct DetachedSubagentsAdapter {
    app: AppHandle,
    spec: HostWorkSpec,
}

fn detached_cancel_acknowledgement(result: Result<usize>) -> AdapterAcknowledgement {
    match result {
        Ok(count) => AdapterAcknowledgement::applied(format!(
            "cancel_subagents requested for {count} owned detached subagent(s)"
        )),
        Err(error) => AdapterAcknowledgement::outcome_unknown(format!(
            "detached-subagent cancellation had an unknown outcome: {error:#}"
        )),
    }
}

#[async_trait]
impl TrustedHostWorkAdapter for DetachedSubagentsAdapter {
    fn spec(&self) -> &HostWorkSpec {
        &self.spec
    }

    async fn status(&self) -> AdapterObservation {
        let Some(pool) = self.app.try_state::<EnginePool>() else {
            return AdapterObservation::unknown("engine pool is unavailable");
        };
        let status = pool.host_work_detached_subagent_status();
        if status.idle_count > 0 {
            AdapterObservation::new(
                HostWorkObservedState::Running,
                format!(
                    "{} owned detached subagent(s) are live in idle sessions",
                    status.idle_count
                ),
            )
        } else if status.deferred_by_active_turn_count > 0 {
            // 旧 detached 与新 foreground 共用同一 engine 的 SubAgentManager；此时
            // 无法选择性取消旧 agent。Unknown 防止 Pending race 后伪确认 Stopped。
            AdapterObservation::unknown(format!(
                "{} detached subagent(s) are deferred by an active foreground turn",
                status.deferred_by_active_turn_count
            ))
        } else {
            AdapterObservation::new(
                HostWorkObservedState::Stopped,
                "no owned detached subagent is live",
            )
        }
    }

    async fn apply(&self, action: HostWorkAction, _directive_id: &str) -> AdapterAcknowledgement {
        if action != HostWorkAction::Stop {
            return AdapterAcknowledgement::rejected(
                "detached-subagent adapter only supports stop",
            );
        }
        let Some(pool) = self.app.try_state::<EnginePool>() else {
            return AdapterAcknowledgement::rejected("engine pool is unavailable");
        };
        detached_cancel_acknowledgement(pool.cancel_detached_subagents_for_governor().await)
    }
}

struct ScheduledAdapter {
    app: AppHandle,
    spec: HostWorkSpec,
}

#[async_trait]
impl TrustedHostWorkAdapter for ScheduledAdapter {
    fn spec(&self) -> &HostWorkSpec {
        &self.spec
    }

    async fn status(&self) -> AdapterObservation {
        let Some(state) = self.app.try_state::<ScheduledTaskState>() else {
            return AdapterObservation::unknown("scheduled runtime is unavailable");
        };
        match state.host_work_active_run_count().await {
            Ok(0) => AdapterObservation::new(
                HostWorkObservedState::Stopped,
                "no queued or running scheduled runs",
            ),
            Ok(count) => AdapterObservation::new(
                HostWorkObservedState::Running,
                format!("{count} scheduled run(s) queued or running"),
            ),
            Err(error) => {
                AdapterObservation::unknown(format!("scheduled status failed: {error:#}"))
            }
        }
    }

    async fn apply(&self, action: HostWorkAction, _directive_id: &str) -> AdapterAcknowledgement {
        if action != HostWorkAction::Stop {
            return AdapterAcknowledgement::rejected("scheduled adapter only supports stop");
        }
        let Some(state) = self.app.try_state::<ScheduledTaskState>() else {
            return AdapterAcknowledgement::rejected("scheduled runtime is unavailable");
        };
        match state.stop_active_runs_for_governor().await {
            Ok(count) => AdapterAcknowledgement::applied(format!(
                "stop requested for {count} queued or running scheduled run(s)"
            )),
            Err(error) => AdapterAcknowledgement::outcome_unknown(format!(
                "scheduled stop did not reach a known outcome: {error:#}"
            )),
        }
    }
}

struct KnowledgeAdapter {
    app: AppHandle,
    spec: HostWorkSpec,
}

#[async_trait]
impl TrustedHostWorkAdapter for KnowledgeAdapter {
    fn spec(&self) -> &HostWorkSpec {
        &self.spec
    }

    async fn status(&self) -> AdapterObservation {
        let Some(service) = self.app.try_state::<KnowledgeService>() else {
            return AdapterObservation::unknown("knowledge service is unavailable");
        };
        let scan = service.status();
        let index = service.index_status();
        knowledge_observation(scan.running, index.running, index.resumable)
    }

    async fn apply(&self, action: HostWorkAction, _directive_id: &str) -> AdapterAcknowledgement {
        if action != HostWorkAction::Stop {
            return AdapterAcknowledgement::rejected("knowledge adapter only supports stop");
        }
        let Some(service) = self.app.try_state::<KnowledgeService>() else {
            return AdapterAcknowledgement::rejected("knowledge service is unavailable");
        };
        service.cancel_scan();
        match service.cancel_index() {
            Ok(()) => AdapterAcknowledgement::applied(
                "knowledge scan and index cancellation was requested",
            ),
            Err(error) => AdapterAcknowledgement::outcome_unknown(format!(
                "knowledge cancellation was only partially acknowledged: {error}"
            )),
        }
    }
}

fn knowledge_observation(
    scan_running: bool,
    index_running: bool,
    index_resumable: bool,
) -> AdapterObservation {
    if scan_running || index_running {
        AdapterObservation::new(
            HostWorkObservedState::Running,
            "knowledge scan or index work is active",
        )
    } else if index_resumable {
        // resumable 表示磁盘上保留了 interrupted checkpoint，不代表后台仍有执行体。
        AdapterObservation::new(
            HostWorkObservedState::Stopped,
            "knowledge work is inactive; a resumable index checkpoint is retained",
        )
    } else {
        AdapterObservation::new(
            HostWorkObservedState::Stopped,
            "knowledge scan and index work are inactive",
        )
    }
}

struct ConnectorsAdapter {
    app: AppHandle,
    spec: HostWorkSpec,
}

#[async_trait]
impl TrustedHostWorkAdapter for ConnectorsAdapter {
    fn spec(&self) -> &HostWorkSpec {
        &self.spec
    }

    async fn status(&self) -> AdapterObservation {
        let Some(connections) = self.app.try_state::<ConnectorConn>() else {
            return AdapterObservation::unknown("connector connection registry is unavailable");
        };
        let Some(count) = connections.governed_running_count() else {
            return AdapterObservation::unknown(
                "connector ownership registry status is unavailable",
            );
        };
        if count == 0 {
            AdapterObservation::new(
                HostWorkObservedState::Stopped,
                "no governed connector connection flow is running",
            )
        } else {
            AdapterObservation::new(
                HostWorkObservedState::Running,
                format!("{count} governed connector connection flow(s) are running"),
            )
        }
    }

    async fn apply(&self, action: HostWorkAction, _directive_id: &str) -> AdapterAcknowledgement {
        if action != HostWorkAction::Stop {
            return AdapterAcknowledgement::rejected("connector adapter only supports stop");
        }
        let Some(connections) = self.app.try_state::<ConnectorConn>() else {
            return AdapterAcknowledgement::rejected(
                "connector connection registry is unavailable",
            );
        };
        let mut pids = match connections.cancel_governed() {
            Ok(pids) => pids,
            Err(error) => return AdapterAcknowledgement::outcome_unknown(error),
        };
        pids.sort_unstable();
        pids.dedup();
        let count = pids.len();
        let stop_app = self.app.clone();
        let killed = tokio::task::spawn_blocking(move || {
            let connections = stop_app.state::<ConnectorConn>();
            connections.stop_cancelled_pids(pids)
        })
        .await;
        match killed {
            Ok(Ok(())) => AdapterAcknowledgement::applied(format!(
                "stop confirmed for {count} governed connector process group(s)"
            )),
            Ok(Err(error)) => AdapterAcknowledgement::outcome_unknown(format!(
                "connector process-group stop was not confirmed: {error}"
            )),
            Err(error) => AdapterAcknowledgement::outcome_unknown(format!(
                "connector stop worker failed after dispatch: {error}"
            )),
        }
    }
}

struct SupervisorAdapter {
    client: crate::platform::host_supervisor::HostSupervisorClient,
    target: crate::platform::host_supervisor::ManagedHostWork,
    spec: HostWorkSpec,
    app_cgroup_telemetry: Option<AppCgroupTelemetryCache>,
}

enum SupervisorStopPrecondition {
    ExactGeneration(String),
    AlreadyStopped,
    AlreadyStopping,
    Unknown,
}

fn supervisor_stop_precondition(
    receipt: &crate::platform::host_supervisor::SupervisorReceipt,
) -> SupervisorStopPrecondition {
    use crate::platform::host_supervisor::{
        ObservedWorkState, SupervisorAction, SupervisorOutcome,
    };

    // Status 只有在 daemon 完成固定 descriptor 身份、effective systemd/cgroup 策略
    // 与实际 unit 状态核验后才会返回 Reconciled。OutcomeUnknown 可能仍携带用于取证的
    // observation，但它绝不能成为控制新实例的 authority。
    if receipt.action != SupervisorAction::Status
        || receipt.outcome != SupervisorOutcome::Reconciled
    {
        return SupervisorStopPrecondition::Unknown;
    }

    let Some(observation) = receipt.observation.as_ref() else {
        return SupervisorStopPrecondition::Unknown;
    };
    match observation.state {
        ObservedWorkState::Inactive => SupervisorStopPrecondition::AlreadyStopped,
        ObservedWorkState::Deactivating => SupervisorStopPrecondition::AlreadyStopping,
        ObservedWorkState::Active | ObservedWorkState::Activating | ObservedWorkState::Failed => {
            observation
                .instance_generation
                .clone()
                .map(SupervisorStopPrecondition::ExactGeneration)
                .unwrap_or(SupervisorStopPrecondition::Unknown)
        }
        ObservedWorkState::Unknown => SupervisorStopPrecondition::Unknown,
    }
}

fn update_app_cgroup_telemetry(
    cache: &AppCgroupTelemetryCache,
    receipt: &crate::platform::host_supervisor::SupervisorReceipt,
) {
    use crate::platform::host_supervisor::{ManagedHostWork, SupervisorAction, SupervisorOutcome};

    if receipt.target != ManagedHostWork::PinvouApp
        || receipt.action != SupervisorAction::Status
        || receipt.outcome != SupervisorOutcome::Reconciled
    {
        cache.mark_stale();
        return;
    }
    let Some(observation) = receipt.observation.as_ref() else {
        cache.mark_stale();
        return;
    };
    let Some(instance_generation) = observation.instance_generation.as_ref() else {
        cache.mark_stale();
        return;
    };
    if !valid_supervisor_instance_generation(instance_generation) {
        cache.mark_stale();
        return;
    }
    let Ok(observed_at_ms) = i64::try_from(receipt.observed_at_unix_ms) else {
        cache.mark_stale();
        return;
    };
    let full_avg10 = observation
        .cgroup
        .memory_pressure
        .as_ref()
        .and_then(|pressure| pressure.full.as_ref())
        .and_then(|full| full.avg10);
    if full_avg10.is_some_and(|value| !value.is_finite() || !(0.0..=100.0).contains(&value)) {
        cache.mark_stale();
        return;
    }
    cache.publish(AppCgroupResourceObservation {
        observed_at_ms,
        instance_generation: instance_generation.clone(),
        memory_current_bytes: observation.cgroup.memory_current_bytes,
        memory_high_bytes: observation.cgroup.memory_high_bytes,
        memory_max_bytes: observation.cgroup.memory_max_bytes,
        memory_events_high: observation.cgroup.memory_events.get("high").copied(),
        memory_events_oom: observation.cgroup.memory_events.get("oom").copied(),
        memory_events_oom_kill: observation.cgroup.memory_events.get("oom_kill").copied(),
        memory_pressure_full_avg10: full_avg10,
    });
}

fn valid_supervisor_instance_generation(generation: &str) -> bool {
    generation.len() == 32
        && generation != "00000000000000000000000000000000"
        && generation
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
}

#[async_trait]
impl TrustedHostWorkAdapter for SupervisorAdapter {
    fn spec(&self) -> &HostWorkSpec {
        &self.spec
    }

    async fn status(&self) -> AdapterObservation {
        use crate::platform::host_supervisor::HostSupervisorError;

        match self.client.status(self.target).await {
            Ok(receipt) => {
                if let Some(cache) = self.app_cgroup_telemetry.as_ref() {
                    update_app_cgroup_telemetry(cache, &receipt);
                }
                supervisor_observation(&receipt)
            }
            Err(HostSupervisorError::Unsupported) => {
                self.mark_status_stale();
                AdapterObservation::unknown("host supervisor is unsupported")
            }
            Err(HostSupervisorError::Unavailable(_)) => {
                self.mark_status_stale();
                AdapterObservation::unknown("host supervisor status is unavailable")
            }
            Err(HostSupervisorError::InvalidRequest(_)) => {
                self.mark_status_stale();
                AdapterObservation::unknown("host supervisor rejected the bounded status request")
            }
            Err(HostSupervisorError::Protocol(_)) => {
                self.mark_status_stale();
                AdapterObservation::unknown("host supervisor status response failed validation")
            }
        }
    }

    fn mark_status_stale(&self) {
        if let Some(cache) = self.app_cgroup_telemetry.as_ref() {
            cache.mark_stale();
        }
    }

    async fn apply(&self, action: HostWorkAction, directive_id: &str) -> AdapterAcknowledgement {
        use crate::platform::host_supervisor::{HostSupervisorError, SupervisorOutcome};

        if action != HostWorkAction::Stop || !self.spec.supported_actions.contains(&action) {
            return AdapterAcknowledgement::rejected(
                "supervisor adapter does not allow this action",
            );
        }
        // Stop 前必须重读固定 descriptor 的可信 Status，并把该回执的 systemd
        // InvocationID 原样作为实例 generation 前置条件。descriptor revision 只是
        // 静态协议版本，绝不能拿来控制可能已经重启的新实例。
        let expected_generation = match self.client.status(self.target).await {
            Ok(receipt) => match supervisor_stop_precondition(&receipt) {
                SupervisorStopPrecondition::ExactGeneration(generation) => generation,
                SupervisorStopPrecondition::AlreadyStopped => {
                    return AdapterAcknowledgement::applied(
                        "fixed supervisor descriptor is already inactive",
                    );
                }
                SupervisorStopPrecondition::AlreadyStopping => {
                    return AdapterAcknowledgement::applied(
                        "fixed supervisor descriptor is already deactivating",
                    );
                }
                SupervisorStopPrecondition::Unknown => {
                    return AdapterAcknowledgement::outcome_unknown(
                        "fixed supervisor descriptor has no trusted live instance generation",
                    );
                }
            },
            Err(HostSupervisorError::Unsupported | HostSupervisorError::InvalidRequest(_)) => {
                return AdapterAcknowledgement::rejected(
                    "host supervisor rejected the bounded status precondition",
                );
            }
            Err(HostSupervisorError::Unavailable(_) | HostSupervisorError::Protocol(_)) => {
                return AdapterAcknowledgement::outcome_unknown(
                    "host supervisor status precondition was not confirmed",
                );
            }
        };

        match self
            .client
            .stop(self.target, directive_id, &expected_generation)
            .await
        {
            Ok(receipt) => match receipt.outcome {
                SupervisorOutcome::Applied
                | SupervisorOutcome::AlreadyApplied
                | SupervisorOutcome::Reconciled => AdapterAcknowledgement::applied(format!(
                    "fixed supervisor descriptor acknowledged {:?}",
                    receipt.outcome
                )),
                SupervisorOutcome::OutcomeUnknown => AdapterAcknowledgement::outcome_unknown(
                    "fixed supervisor descriptor returned outcome_unknown",
                ),
                SupervisorOutcome::Rejected => AdapterAcknowledgement::rejected(
                    "fixed supervisor descriptor rejected the stop request",
                ),
            },
            Err(HostSupervisorError::Unsupported | HostSupervisorError::InvalidRequest(_)) => {
                AdapterAcknowledgement::rejected("host supervisor rejected the bounded request")
            }
            Err(HostSupervisorError::Unavailable(_) | HostSupervisorError::Protocol(_)) => {
                AdapterAcknowledgement::outcome_unknown(
                    "host supervisor response was not confirmed",
                )
            }
        }
    }
}

fn supervisor_observation(
    receipt: &crate::platform::host_supervisor::SupervisorReceipt,
) -> AdapterObservation {
    use crate::platform::host_supervisor::{
        ObservedWorkState, SupervisorAction, SupervisorOutcome,
    };

    if receipt.action != SupervisorAction::Status
        || receipt.outcome != SupervisorOutcome::Reconciled
    {
        return AdapterObservation::unknown(
            "host supervisor status did not pass descriptor and protection reconciliation",
        );
    }

    let Some(observation) = receipt.observation.as_ref() else {
        return AdapterObservation::unknown("host supervisor returned no observation");
    };
    let state = match observation.state {
        ObservedWorkState::Active => HostWorkObservedState::Running,
        ObservedWorkState::Activating => HostWorkObservedState::Starting,
        ObservedWorkState::Inactive => HostWorkObservedState::Stopped,
        ObservedWorkState::Failed => HostWorkObservedState::Failed,
        ObservedWorkState::Deactivating | ObservedWorkState::Unknown => {
            HostWorkObservedState::Unknown
        }
    };
    AdapterObservation::new(
        state,
        format!(
            "fixed supervisor descriptor state is {:?}",
            observation.state
        ),
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::features::pinvou_os::{ResourceObservation, ResourcePressure};

    static NEXT_TEMP: AtomicUsize = AtomicUsize::new(1);

    struct TempLedger {
        root: PathBuf,
        ledger: PathBuf,
    }

    impl TempLedger {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "pinvou-host-work-control-{label}-{}-{nonce}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).expect("create temp ledger root");
            Self {
                ledger: root.join("events.jsonl"),
                root,
            }
        }
    }

    impl Drop for TempLedger {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    struct FakeAdapter {
        spec: HostWorkSpec,
        state: AtomicU8,
        acknowledgement: HostWorkDirectiveAcknowledgement,
        apply_count: AtomicUsize,
        status_count: AtomicUsize,
        apply_delay_ms: u64,
        complete_on_apply: AtomicBool,
    }

    impl FakeAdapter {
        fn new(owner: &'static str, acknowledgement: HostWorkDirectiveAcknowledgement) -> Self {
            Self {
                spec: HostWorkSpec::stop_only(
                    owner,
                    HostWorkKind::KnowledgeJob,
                    ResourceClass::Heavy,
                    10,
                    Interruptibility::Immediate,
                ),
                state: AtomicU8::new(encode_state(HostWorkObservedState::Running)),
                acknowledgement,
                apply_count: AtomicUsize::new(0),
                status_count: AtomicUsize::new(0),
                apply_delay_ms: 0,
                complete_on_apply: AtomicBool::new(true),
            }
        }

        fn with_delay(mut self, delay_ms: u64) -> Self {
            self.apply_delay_ms = delay_ms;
            self
        }

        fn without_completion(self) -> Self {
            self.complete_on_apply.store(false, Ordering::Relaxed);
            self
        }

        fn set_state(&self, state: HostWorkObservedState) {
            self.state.store(encode_state(state), Ordering::SeqCst);
        }

        fn apply_count(&self) -> usize {
            self.apply_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl TrustedHostWorkAdapter for FakeAdapter {
        fn spec(&self) -> &HostWorkSpec {
            &self.spec
        }

        async fn status(&self) -> AdapterObservation {
            self.status_count.fetch_add(1, Ordering::SeqCst);
            AdapterObservation::new(
                decode_state(self.state.load(Ordering::SeqCst)),
                "fake adapter status",
            )
        }

        async fn apply(
            &self,
            action: HostWorkAction,
            _directive_id: &str,
        ) -> AdapterAcknowledgement {
            assert_eq!(action, HostWorkAction::Stop);
            self.apply_count.fetch_add(1, Ordering::SeqCst);
            if self.apply_delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(self.apply_delay_ms)).await;
            }
            if self.complete_on_apply.load(Ordering::SeqCst) {
                self.set_state(HostWorkObservedState::Stopped);
            }
            AdapterAcknowledgement::new(self.acknowledgement, "fake adapter acknowledgement")
        }
    }

    fn encode_state(state: HostWorkObservedState) -> u8 {
        match state {
            HostWorkObservedState::Unknown => 0,
            HostWorkObservedState::Starting => 1,
            HostWorkObservedState::Running => 2,
            HostWorkObservedState::Paused => 3,
            HostWorkObservedState::Stopped => 4,
            HostWorkObservedState::Completed => 5,
            HostWorkObservedState::Failed => 6,
        }
    }

    fn decode_state(state: u8) -> HostWorkObservedState {
        match state {
            0 => HostWorkObservedState::Unknown,
            1 => HostWorkObservedState::Starting,
            2 => HostWorkObservedState::Running,
            3 => HostWorkObservedState::Paused,
            4 => HostWorkObservedState::Stopped,
            5 => HostWorkObservedState::Completed,
            6 => HostWorkObservedState::Failed,
            _ => HostWorkObservedState::Unknown,
        }
    }

    fn test_config() -> WorkerConfig {
        WorkerConfig {
            poll_interval: Duration::from_millis(10),
            action_timeout: Duration::from_secs(2),
            status_timeout: Duration::from_millis(200),
            reconciliation_grace: Duration::from_millis(500),
        }
    }

    fn critical_observation() -> ResourceObservation {
        ResourceObservation {
            sampled_at_ms: chrono::Utc::now().timestamp_millis(),
            cpu_usage_pct: Some(10.0),
            memory_used_pct: Some(100.0),
            gpu_usage_pct: None,
            temperature_c: None,
            power_w: None,
            app_cgroup: None,
        }
    }

    async fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + timeout;
        loop {
            if predicate() {
                return;
            }
            assert!(Instant::now() < deadline, "condition did not become true");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn slow_adapter_neither_blocks_resource_observation_nor_other_worker() {
        let temp = TempLedger::new("isolation");
        let runtime = PinvouOsRuntime::boot(temp.ledger.clone()).expect("runtime");
        let slow = Arc::new(
            FakeAdapter::new("test:slow", HostWorkDirectiveAcknowledgement::Applied)
                .with_delay(400),
        );
        let fast = Arc::new(FakeAdapter::new(
            "test:fast",
            HostWorkDirectiveAcknowledgement::Applied,
        ));
        let plane = start_control_plane(
            runtime.clone(),
            vec![slow.clone(), fast.clone()],
            test_config(),
        )
        .expect("control plane");
        wait_until(Duration::from_secs(1), || {
            runtime
                .snapshot()
                .host_works
                .values()
                .all(|work| work.observed_state == HostWorkObservedState::Running)
        })
        .await;

        let started = Instant::now();
        let decision = runtime
            .observe_resources(critical_observation())
            .expect("critical observation");
        assert_eq!(decision.pressure, ResourcePressure::Critical);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "resource observation waited for an adapter"
        );
        wait_until(Duration::from_millis(250), || fast.apply_count() == 1).await;
        wait_until(Duration::from_millis(250), || {
            let snapshot = runtime.snapshot();
            let fast_work_id = &snapshot
                .host_works
                .values()
                .find(|work| work.owner == "test:fast")
                .expect("fast work")
                .work_id;
            snapshot.host_work_directives.values().any(|directive| {
                &directive.work_id == fast_work_id
                    && directive.status == HostWorkDirectiveStatus::Reconciled
            })
        })
        .await;
        assert_eq!(slow.apply_count(), 1);
        drop(plane);
    }

    #[tokio::test]
    async fn outcome_unknown_uses_status_only_and_new_instance_renews_generation() {
        let temp = TempLedger::new("unknown-generation");
        let runtime = PinvouOsRuntime::boot(temp.ledger.clone()).expect("runtime");
        let adapter = Arc::new(
            FakeAdapter::new(
                "test:unknown",
                HostWorkDirectiveAcknowledgement::OutcomeUnknown,
            )
            .without_completion(),
        );
        let plane = start_control_plane(runtime.clone(), vec![adapter.clone()], test_config())
            .expect("control plane");
        wait_until(Duration::from_secs(1), || {
            runtime
                .snapshot()
                .host_works
                .values()
                .next()
                .is_some_and(|work| work.observed_state == HostWorkObservedState::Running)
        })
        .await;
        let decision = runtime
            .observe_resources(critical_observation())
            .expect("critical observation");
        let directive_id = decision.host_work_directives[0].directive_id.clone();
        wait_until(Duration::from_secs(1), || {
            runtime
                .snapshot()
                .host_work_directives
                .get(&directive_id)
                .is_some_and(|directive| {
                    directive.acknowledgement
                        == Some(HostWorkDirectiveAcknowledgement::OutcomeUnknown)
                })
        })
        .await;
        assert_eq!(adapter.apply_count(), 1);

        adapter.set_state(HostWorkObservedState::Stopped);
        wait_until(Duration::from_secs(1), || {
            runtime
                .snapshot()
                .host_work_directives
                .get(&directive_id)
                .is_some_and(|directive| {
                    directive.status == HostWorkDirectiveStatus::Reconciled
                        && directive.reconciliation
                            == Some(HostWorkReconciliationOutcome::Confirmed)
                })
        })
        .await;
        assert_eq!(
            adapter.apply_count(),
            1,
            "outcome_unknown replayed the action"
        );

        adapter.set_state(HostWorkObservedState::Running);
        wait_until(Duration::from_secs(1), || {
            runtime
                .snapshot()
                .host_works
                .values()
                .next()
                .is_some_and(|work| work.generation == 2)
        })
        .await;
        wait_until(Duration::from_secs(1), || adapter.apply_count() == 2).await;
        let generations = runtime
            .snapshot()
            .host_work_directives
            .values()
            .map(|directive| directive.generation)
            .collect::<BTreeSet<_>>();
        assert_eq!(generations, BTreeSet::from([1, 2]));
        drop(plane);
    }

    #[tokio::test]
    async fn restart_rebinds_inherited_pending_without_replaying_an_unknown_side_effect() {
        let temp = TempLedger::new("inherited-pending");
        let directive_id = {
            let runtime = PinvouOsRuntime::boot(temp.ledger.clone()).expect("first runtime");
            let spec = HostWorkSpec::stop_only(
                "test:rebind",
                HostWorkKind::KnowledgeJob,
                ResourceClass::Heavy,
                10,
                Interruptibility::Immediate,
            );
            let handle = bind_registration(&runtime, &spec).expect("initial registration");
            runtime
                .observe_host_work(
                    &handle,
                    HostWorkObservedState::Running,
                    "test work started".to_string(),
                )
                .expect("running observation");
            runtime
                .observe_resources(critical_observation())
                .expect("critical observation")
                .host_work_directives[0]
                .directive_id
                .clone()
        };

        let runtime = PinvouOsRuntime::boot(temp.ledger.clone()).expect("restarted runtime");
        let adapter = Arc::new(FakeAdapter::new(
            "test:rebind",
            HostWorkDirectiveAcknowledgement::Applied,
        ));
        let plane = start_control_plane(runtime.clone(), vec![adapter.clone()], test_config())
            .expect("rebound control plane");
        wait_until(Duration::from_secs(2), || {
            runtime
                .snapshot()
                .host_work_directives
                .get(&directive_id)
                .is_some_and(|directive| {
                    directive.status == HostWorkDirectiveStatus::Rejected
                        && directive.acknowledgement
                            == Some(HostWorkDirectiveAcknowledgement::OutcomeUnknown)
                        && directive.reconciliation
                            == Some(HostWorkReconciliationOutcome::NotApplied)
                        && directive.dispatch_recorded_at_ms.is_none()
                })
        })
        .await;
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.host_works.len(), 1);
        assert_eq!(
            snapshot
                .host_works
                .values()
                .next()
                .expect("work")
                .generation,
            1
        );
        assert_eq!(
            adapter.apply_count(),
            0,
            "a prior-boot Pending without a marker may already have executed"
        );
        drop(plane);
    }

    #[tokio::test]
    async fn dispatch_marker_append_failure_prevents_adapter_apply() {
        let temp = TempLedger::new("dispatch-marker-append-failure");
        let runtime = PinvouOsRuntime::boot(temp.ledger.clone()).expect("runtime");
        let adapter = FakeAdapter::new(
            "test:dispatch-marker-append-failure",
            HostWorkDirectiveAcknowledgement::Applied,
        );
        let mut handle = bind_registration(&runtime, adapter.spec()).expect("registration");
        runtime
            .observe_host_work(
                &handle,
                HostWorkObservedState::Running,
                "test work started".to_string(),
            )
            .expect("running observation");
        let directive = runtime
            .observe_resources(critical_observation())
            .expect("critical observation")
            .host_work_directives[0]
            .clone();

        let backup = temp.root.join("events.backup.jsonl");
        std::fs::rename(&temp.ledger, &backup).expect("move ledger aside");
        std::fs::create_dir(&temp.ledger).expect("replace ledger path with directory");
        let mut attempted = HashMap::new();
        let mut reconciliation_started = HashMap::new();
        assert!(
            drive_worker_once(
                &runtime,
                &adapter,
                &mut handle,
                &mut attempted,
                &mut reconciliation_started,
                test_config(),
            )
            .await
            .is_err(),
            "the worker must stop before adapter I/O when marker durability is unknown"
        );
        assert_eq!(adapter.apply_count(), 0);
        let snapshot = runtime.snapshot();
        let projected = &snapshot.host_work_directives[&directive.directive_id];
        assert_eq!(projected.dispatch_recorded_at_ms, None);
        assert_eq!(projected.acknowledgement, None);

        std::fs::remove_dir(&temp.ledger).expect("remove injected ledger directory");
        std::fs::rename(&backup, &temp.ledger).expect("restore ledger");
        drop(runtime);
        let replayed = PinvouOsRuntime::boot(temp.ledger.clone()).expect("replay durable prefix");
        let snapshot = replayed.snapshot();
        let projected = &snapshot.host_work_directives[&directive.directive_id];
        assert_eq!(projected.dispatch_recorded_at_ms, None);
        assert_eq!(projected.acknowledgement, None);
    }

    #[tokio::test]
    async fn restart_after_dispatch_and_apply_before_ack_uses_status_only() {
        let temp = TempLedger::new("dispatch-after-apply-before-ack");
        let adapter = Arc::new(FakeAdapter::new(
            "test:dispatched-applied",
            HostWorkDirectiveAcknowledgement::Applied,
        ));
        let directive_id = {
            let runtime = PinvouOsRuntime::boot(temp.ledger.clone()).expect("first runtime");
            let handle = bind_registration(&runtime, adapter.spec()).expect("registration");
            runtime
                .observe_host_work(
                    &handle,
                    HostWorkObservedState::Running,
                    "test work started".to_string(),
                )
                .expect("running observation");
            let directive = runtime
                .observe_resources(critical_observation())
                .expect("critical observation")
                .host_work_directives[0]
                .clone();
            assert_eq!(
                runtime
                    .record_host_work_directive_dispatch(&handle, &directive.directive_id)
                    .expect("durable dispatch marker"),
                HostWorkDispatchRecord::NewlyRecorded
            );
            let acknowledgement = call_apply(adapter.as_ref(), &directive, test_config()).await;
            assert_eq!(
                acknowledgement.kind,
                HostWorkDirectiveAcknowledgement::Applied
            );
            assert_eq!(adapter.apply_count(), 1);
            directive.directive_id
        };

        let runtime = PinvouOsRuntime::boot(temp.ledger.clone()).expect("restarted runtime");
        let plane = start_control_plane(runtime.clone(), vec![adapter.clone()], test_config())
            .expect("rebound control plane");
        wait_until(Duration::from_secs(1), || {
            runtime
                .snapshot()
                .host_work_directives
                .get(&directive_id)
                .is_some_and(|directive| {
                    directive.status == HostWorkDirectiveStatus::Reconciled
                        && directive.acknowledgement
                            == Some(HostWorkDirectiveAcknowledgement::OutcomeUnknown)
                        && directive.reconciliation
                            == Some(HostWorkReconciliationOutcome::Confirmed)
                        && directive.dispatch_recorded_at_ms.is_some()
                })
        })
        .await;
        assert_eq!(
            adapter.apply_count(),
            1,
            "the durable dispatch fence must prevent a second apply"
        );
        drop(plane);
    }

    #[tokio::test]
    async fn restart_after_dispatch_before_apply_fails_closed_to_status_only() {
        let temp = TempLedger::new("dispatch-before-apply");
        let directive_id = {
            let runtime = PinvouOsRuntime::boot(temp.ledger.clone()).expect("first runtime");
            let spec = HostWorkSpec::stop_only(
                "test:dispatched-not-applied",
                HostWorkKind::KnowledgeJob,
                ResourceClass::Heavy,
                10,
                Interruptibility::Immediate,
            );
            let handle = bind_registration(&runtime, &spec).expect("registration");
            runtime
                .observe_host_work(
                    &handle,
                    HostWorkObservedState::Running,
                    "test work started".to_string(),
                )
                .expect("running observation");
            let directive = runtime
                .observe_resources(critical_observation())
                .expect("critical observation")
                .host_work_directives[0]
                .clone();
            assert_eq!(
                runtime
                    .record_host_work_directive_dispatch(&handle, &directive.directive_id)
                    .expect("durable dispatch marker"),
                HostWorkDispatchRecord::NewlyRecorded
            );
            directive.directive_id
        };

        let runtime = PinvouOsRuntime::boot(temp.ledger.clone()).expect("restarted runtime");
        let adapter = Arc::new(
            FakeAdapter::new(
                "test:dispatched-not-applied",
                HostWorkDirectiveAcknowledgement::Applied,
            )
            .without_completion(),
        );
        let plane = start_control_plane(runtime.clone(), vec![adapter.clone()], test_config())
            .expect("rebound control plane");
        wait_until(Duration::from_secs(2), || {
            runtime
                .snapshot()
                .host_work_directives
                .get(&directive_id)
                .is_some_and(|directive| {
                    directive.status == HostWorkDirectiveStatus::Rejected
                        && directive.acknowledgement
                            == Some(HostWorkDirectiveAcknowledgement::OutcomeUnknown)
                        && directive.reconciliation
                            == Some(HostWorkReconciliationOutcome::NotApplied)
                        && directive.dispatch_recorded_at_ms.is_some()
                })
        })
        .await;
        assert_eq!(
            adapter.apply_count(),
            0,
            "a marker is an attempt fence, not permission to replay"
        );
        drop(plane);
    }

    #[test]
    fn terminal_work_ignores_unknown_without_renewing_or_overwriting() {
        let temp = TempLedger::new("terminal-unknown");
        let runtime = PinvouOsRuntime::boot(temp.ledger.clone()).expect("runtime");
        let spec = HostWorkSpec::stop_only(
            "test:terminal-unknown",
            HostWorkKind::KnowledgeJob,
            ResourceClass::Heavy,
            10,
            Interruptibility::Immediate,
        );
        let mut handle = bind_registration(&runtime, &spec).expect("registration");
        runtime
            .observe_host_work(
                &handle,
                HostWorkObservedState::Stopped,
                "test work stopped".to_string(),
            )
            .expect("terminal observation");

        sync_normal_observation(
            &runtime,
            &mut handle,
            AdapterObservation::unknown("status temporarily unavailable"),
        )
        .expect("unknown observation is a no-op after terminal");

        let work = runtime
            .snapshot()
            .host_works
            .get(handle.work_id())
            .cloned()
            .expect("work");
        assert_eq!(handle.generation(), 1);
        assert_eq!(work.generation, 1);
        assert_eq!(work.observed_state, HostWorkObservedState::Stopped);
    }

    #[test]
    fn resumable_knowledge_checkpoint_is_not_active_work() {
        let checkpoint = knowledge_observation(false, false, true);
        assert_eq!(checkpoint.state, HostWorkObservedState::Stopped);
        assert!(checkpoint.detail.contains("checkpoint"));

        assert_eq!(
            knowledge_observation(true, false, true).state,
            HostWorkObservedState::Running
        );
        assert_eq!(
            knowledge_observation(false, true, true).state,
            HostWorkObservedState::Running
        );
    }

    #[test]
    fn supervisor_stop_precondition_uses_status_invocation_id() {
        use crate::platform::host_supervisor::{
            CgroupObservation, HostWorkObservation, ManagedHostWork, ObservedWorkState,
            SupervisorAction, SupervisorOutcome, SupervisorReceipt,
        };

        let invocation_id = "0123456789abcdef0123456789abcdef";
        let mut receipt = SupervisorReceipt {
            protocol_version: pinvou_host_supervisor_protocol::PROTOCOL_VERSION,
            request_id: "status:test".to_string(),
            target: ManagedHostWork::PinvouAsr,
            descriptor_revision: ManagedHostWork::PinvouAsr.descriptor_revision().to_string(),
            expected_instance_generation: None,
            action: SupervisorAction::Status,
            outcome: SupervisorOutcome::Reconciled,
            observation: Some(HostWorkObservation {
                instance_generation: Some(invocation_id.to_string()),
                state: ObservedWorkState::Active,
                sub_state: "running".to_string(),
                unit_result: "success".to_string(),
                main_pid: None,
                restart_count: None,
                cgroup: CgroupObservation::default(),
            }),
            detail: "status".to_string(),
            observed_at_unix_ms: 1,
        };

        match supervisor_stop_precondition(&receipt) {
            SupervisorStopPrecondition::ExactGeneration(actual) => {
                assert_eq!(actual, invocation_id);
                assert_ne!(actual, ManagedHostWork::PinvouAsr.descriptor_revision());
            }
            _ => panic!("active ASR status must yield its exact InvocationID"),
        }

        receipt
            .observation
            .as_mut()
            .expect("observation")
            .instance_generation = None;
        assert!(matches!(
            supervisor_stop_precondition(&receipt),
            SupervisorStopPrecondition::Unknown
        ));

        receipt
            .observation
            .as_mut()
            .expect("observation")
            .instance_generation = Some(invocation_id.to_string());
        receipt.outcome = SupervisorOutcome::OutcomeUnknown;
        assert!(matches!(
            supervisor_stop_precondition(&receipt),
            SupervisorStopPrecondition::Unknown
        ));
        assert_eq!(
            supervisor_observation(&receipt).state,
            HostWorkObservedState::Unknown
        );

        receipt.observation.as_mut().expect("observation").state = ObservedWorkState::Inactive;
        assert!(matches!(
            supervisor_stop_precondition(&receipt),
            SupervisorStopPrecondition::Unknown
        ));
        assert_eq!(
            supervisor_observation(&receipt).state,
            HostWorkObservedState::Unknown
        );

        receipt.outcome = SupervisorOutcome::Reconciled;
        assert!(matches!(
            supervisor_stop_precondition(&receipt),
            SupervisorStopPrecondition::AlreadyStopped
        ));
        assert_eq!(
            supervisor_observation(&receipt).state,
            HostWorkObservedState::Stopped
        );
    }

    #[test]
    fn app_cgroup_cache_only_accepts_reconciled_status_and_preserves_absolute_counters() {
        use crate::platform::host_supervisor::{
            CgroupObservation, HostWorkObservation, ManagedHostWork, ObservedWorkState,
            SupervisorAction, SupervisorOutcome, SupervisorReceipt,
        };
        use pinvou_host_supervisor_protocol::{MemoryPressure, PressureLine};

        let cache = AppCgroupTelemetryCache::new();
        let mut memory_events = std::collections::BTreeMap::new();
        memory_events.insert("high".to_string(), 11);
        memory_events.insert("oom".to_string(), 3);
        memory_events.insert("oom_kill".to_string(), 2);
        let mut receipt = SupervisorReceipt {
            protocol_version: pinvou_host_supervisor_protocol::PROTOCOL_VERSION,
            request_id: "status:cache".to_string(),
            target: ManagedHostWork::PinvouApp,
            descriptor_revision: ManagedHostWork::PinvouApp.descriptor_revision().to_string(),
            expected_instance_generation: None,
            action: SupervisorAction::Status,
            outcome: SupervisorOutcome::Reconciled,
            observation: Some(HostWorkObservation {
                instance_generation: Some("0123456789abcdef0123456789abcdef".to_string()),
                state: ObservedWorkState::Active,
                sub_state: "running".to_string(),
                unit_result: "success".to_string(),
                main_pid: Some(42),
                restart_count: Some(0),
                cgroup: CgroupObservation {
                    memory_current_bytes: Some(3_000),
                    memory_peak_bytes: Some(3_500),
                    memory_events,
                    memory_pressure: Some(MemoryPressure {
                        some: None,
                        full: Some(PressureLine {
                            avg10: Some(1.25),
                            ..PressureLine::default()
                        }),
                    }),
                    pids_current: Some(8),
                    memory_high_bytes: Some(4_000),
                    memory_max_bytes: Some(8_000),
                    memory_swap_max_bytes: Some(2_000),
                },
            }),
            detail: "trusted status".to_string(),
            observed_at_unix_ms: 10_000,
        };

        update_app_cgroup_telemetry(&cache, &receipt);
        let observed = cache
            .read_for_resource_sampler(10_100)
            .expect("trusted cache observation");
        assert_eq!(observed.memory_current_bytes, Some(3_000));
        assert_eq!(observed.memory_high_bytes, Some(4_000));
        assert_eq!(observed.memory_max_bytes, Some(8_000));
        assert_eq!(observed.memory_events_high, Some(11));
        assert_eq!(observed.memory_events_oom, Some(3));
        assert_eq!(observed.memory_events_oom_kill, Some(2));
        assert_eq!(observed.memory_pressure_full_avg10, Some(1.25));

        receipt.outcome = SupervisorOutcome::OutcomeUnknown;
        update_app_cgroup_telemetry(&cache, &receipt);
        assert!(cache.read_for_resource_sampler(10_200).is_none());
    }

    #[test]
    fn app_cgroup_cache_reader_fails_closed_without_waiting_for_writer_or_io() {
        let cache = AppCgroupTelemetryCache::new();
        let _writer = cache.inner.write();

        // `try_read` 在 writer 临界区直接返回 None；这里没有 async、Supervisor client
        // 或文件/socket 调用，Resource sampler 不会被 HostWork I/O 反压。
        assert!(cache.read_for_resource_sampler(10_000).is_none());
    }

    #[test]
    fn production_scope_controls_detached_subagents_but_never_foreground_turns() {
        let specs = vec![
            scheduled_spec(),
            detached_subagents_spec(),
            knowledge_spec(),
            connectors_spec(),
        ];
        let specs = if std::env::consts::OS == "linux" {
            let mut linux_specs = specs;
            linux_specs.extend([asr_cgroup_spec(), app_cgroup_spec()]);
            linux_specs
        } else {
            specs
        };
        assert!(specs
            .iter()
            .all(|spec| spec.kind != HostWorkKind::EngineTurn));
        let detached = specs
            .iter()
            .find(|spec| spec.kind == HostWorkKind::DetachedSubAgent)
            .expect("detached-subagent aggregate");
        assert_eq!(
            detached.supported_actions,
            BTreeSet::from([HostWorkAction::Stop])
        );
        if std::env::consts::OS == "linux" {
            let app = specs
                .iter()
                .find(|spec| spec.kind == HostWorkKind::AppCgroup)
                .expect("app cgroup observation");
            assert!(app.essential);
            assert!(!app.governable);
            assert!(app.supported_actions.is_empty());
        }
    }

    #[test]
    fn partial_detached_cancel_is_acknowledged_as_outcome_unknown() {
        let acknowledgement =
            detached_cancel_acknowledgement(Err(anyhow!("one active session was skipped")));
        assert_eq!(
            acknowledgement.kind,
            HostWorkDirectiveAcknowledgement::OutcomeUnknown
        );
    }
}
