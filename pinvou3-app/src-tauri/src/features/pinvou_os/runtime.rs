use std::fs::{self, OpenOptions};
use std::io::{BufRead as _, BufReader, Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _, Result};
use fs2::FileExt as _;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::governor::{classify_pressure, directive_for_agent, ResourceGovernorPolicy};
use super::model::*;

type RuntimeEventSink = dyn Fn(EventEnvelope) + Send + Sync + 'static;

#[derive(Clone)]
pub struct PinvouOsRuntime {
    inner: Arc<RuntimeInner>,
}

struct RuntimeInner {
    ledger_path: PathBuf,
    append_lock: Mutex<()>,
    snapshot: RwLock<RuntimeSnapshot>,
    next_sequence: AtomicU64,
    event_sink: RwLock<Option<Arc<RuntimeEventSink>>>,
    governor_policy: ResourceGovernorPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenMissionRequest {
    pub objective: String,
    #[serde(default = "default_mission_priority")]
    pub priority: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_at_ms: Option<i64>,
}

fn default_mission_priority() -> u8 {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RegisterMissionAgentRequest {
    pub display_name: String,
    pub role: String,
    pub capabilities: Vec<CapabilityContract>,
    #[serde(default = "default_mission_priority")]
    pub priority: u8,
    pub interruptibility: Interruptibility,
    pub mission_id: String,
    pub run_id: String,
}

#[derive(Default)]
struct EventContext {
    source_actor_id: String,
    mission_id: Option<String>,
    run_id: Option<String>,
    causation_id: Option<String>,
    correlation_id: Option<String>,
}

impl EventContext {
    fn kernel() -> Self {
        Self {
            source_actor_id: KERNEL_ACTOR_ID.to_string(),
            ..Self::default()
        }
    }
}

impl PinvouOsRuntime {
    pub fn boot(ledger_path: PathBuf) -> Result<Self> {
        if let Some(parent) = ledger_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create PinvouOS runtime dir {}", parent.display()))?;
        }
        let snapshot = replay_ledger(&ledger_path)?;
        let next_sequence = snapshot.last_sequence.saturating_add(1).max(1);
        let runtime = Self {
            inner: Arc::new(RuntimeInner {
                ledger_path,
                append_lock: Mutex::new(()),
                snapshot: RwLock::new(snapshot),
                next_sequence: AtomicU64::new(next_sequence),
                event_sink: RwLock::new(None),
                governor_policy: ResourceGovernorPolicy::default(),
            }),
        };
        runtime.bootstrap()?;
        Ok(runtime)
    }

    pub fn ledger_path(&self) -> &Path {
        &self.inner.ledger_path
    }

    pub fn set_event_sink(&self, sink: impl Fn(EventEnvelope) + Send + Sync + 'static) {
        *self.inner.event_sink.write() = Some(Arc::new(sink));
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.inner.snapshot.read().clone()
    }

    pub fn open_mission(&self, request: OpenMissionRequest) -> Result<MissionStart> {
        let objective = request.objective.trim();
        if objective.is_empty() {
            bail!("mission objective must not be empty");
        }
        if request
            .deadline_at_ms
            .is_some_and(|deadline| deadline <= now_ms())
        {
            bail!("mission deadline must be in the future");
        }

        let created_at_ms = now_ms();
        let mission_id = new_entity_id("mission");
        let run_id = new_entity_id("run");
        let mission = Mission {
            mission_id: mission_id.clone(),
            objective: objective.to_string(),
            priority: request.priority,
            status: MissionStatus::Active,
            created_at_ms,
            deadline_at_ms: request.deadline_at_ms,
        };
        let run = Run {
            run_id: run_id.clone(),
            mission_id: mission_id.clone(),
            attempt: 1,
            status: RunStatus::Running,
            started_at_ms: created_at_ms,
            ended_at_ms: None,
        };

        let opened = self.append(
            EventContext {
                source_actor_id: "actor:user".to_string(),
                mission_id: Some(mission_id.clone()),
                correlation_id: Some(mission_id.clone()),
                ..EventContext::default()
            },
            RuntimeEvent::MissionOpened {
                mission: mission.clone(),
            },
        )?;
        self.append(
            EventContext {
                source_actor_id: KERNEL_ACTOR_ID.to_string(),
                mission_id: Some(mission_id.clone()),
                run_id: Some(run_id.clone()),
                causation_id: Some(opened.event_id),
                correlation_id: Some(mission_id),
            },
            RuntimeEvent::RunStarted { run: run.clone() },
        )?;

        Ok(MissionStart { mission, run })
    }

    pub fn register_mission_agent(
        &self,
        request: RegisterMissionAgentRequest,
    ) -> Result<AgentManifest> {
        validate_capability_contracts(&request.capabilities)?;
        let snapshot = self.inner.snapshot.read();
        let mission = snapshot
            .missions
            .get(&request.mission_id)
            .ok_or_else(|| anyhow!("unknown mission {}", request.mission_id))?;
        if mission.status != MissionStatus::Active {
            bail!("mission {} is not active", request.mission_id);
        }
        let run = snapshot
            .runs
            .get(&request.run_id)
            .ok_or_else(|| anyhow!("unknown run {}", request.run_id))?;
        if run.mission_id != request.mission_id || run.status != RunStatus::Running {
            bail!("run {} is not active for mission", request.run_id);
        }
        drop(snapshot);

        let agent = AgentManifest {
            agent_id: new_entity_id("agent"),
            display_name: required_text(&request.display_name, "agent display name")?,
            kind: AgentKind::Mission,
            role: required_text(&request.role, "agent role")?,
            capabilities: request.capabilities,
            priority: request.priority,
            interruptibility: request.interruptibility,
            observed_state: AgentState::Running,
            desired_state: AgentState::Running,
            mission_id: Some(request.mission_id.clone()),
            run_id: Some(request.run_id.clone()),
            created_at_ms: now_ms(),
        };
        self.append(
            EventContext {
                source_actor_id: KERNEL_ACTOR_ID.to_string(),
                mission_id: Some(request.mission_id.clone()),
                run_id: Some(request.run_id),
                correlation_id: Some(request.mission_id),
                ..EventContext::default()
            },
            RuntimeEvent::AgentRegistered {
                agent: agent.clone(),
            },
        )?;
        Ok(agent)
    }

    pub fn explain_capability(&self, capability_id: &str) -> CapabilityAvailability {
        let capability_id = capability_id.trim();
        let snapshot = self.inner.snapshot.read();
        let mut candidates = Vec::new();
        let mut available = false;
        let mut resource_blocked = false;
        let mut state_blocked = false;
        let mut has_runtime_preconditions = false;

        for agent in snapshot.agents.values() {
            let Some(capability) = agent
                .capabilities
                .iter()
                .find(|capability| capability.capability_id == capability_id)
            else {
                continue;
            };
            candidates.push(agent.agent_id.clone());
            has_runtime_preconditions |= !capability.preconditions.is_empty();
            let blocked_by_resources = capability.resource_class == ResourceClass::Heavy
                && snapshot.resources.pressure >= ResourcePressure::Hot;
            let runnable = matches!(agent.observed_state, AgentState::Idle | AgentState::Running)
                && matches!(agent.desired_state, AgentState::Idle | AgentState::Running);
            resource_blocked |= blocked_by_resources;
            state_blocked |= !runnable;
            available |= runnable && !blocked_by_resources;
        }

        candidates.sort();
        let mut reason_codes = Vec::new();
        let state = if candidates.is_empty() {
            reason_codes.push("no_registered_executor".to_string());
            CapabilityAvailabilityState::Unsupported
        } else if available {
            if has_runtime_preconditions {
                reason_codes.push("runtime_preconditions_apply".to_string());
            }
            CapabilityAvailabilityState::Available
        } else {
            if resource_blocked {
                reason_codes.push("blocked_by_resource_governor".to_string());
            }
            if state_blocked {
                reason_codes.push("executor_not_runnable".to_string());
            }
            CapabilityAvailabilityState::TemporarilyUnavailable
        };

        CapabilityAvailability {
            capability_id: capability_id.to_string(),
            state,
            candidate_agent_ids: candidates,
            reason_codes,
        }
    }

    pub fn observe_resources(&self, observation: ResourceObservation) -> Result<ResourceDecision> {
        validate_resource_observation(&observation)?;
        let before = self.snapshot();
        let previous_pressure = before.resources.pressure;
        let pressure = classify_pressure(&observation, self.inner.governor_policy);
        let observed = self.append(
            EventContext {
                source_actor_id: RESOURCE_AGENT_ID.to_string(),
                correlation_id: Some("resource-pressure".to_string()),
                ..EventContext::default()
            },
            RuntimeEvent::ResourceObserved {
                observation: observation.clone(),
                pressure,
            },
        )?;

        let pressure_changed = pressure != previous_pressure;
        let mut pressure_claim_id = before.resources.active_pressure_claim_id.clone();
        let mut directive_cause = observed.event_id.clone();
        if pressure_changed {
            if let Some(claim_id) = before.resources.active_pressure_claim_id {
                self.append(
                    EventContext {
                        source_actor_id: RESOURCE_AGENT_ID.to_string(),
                        causation_id: Some(observed.event_id.clone()),
                        correlation_id: Some("resource-pressure".to_string()),
                        ..EventContext::default()
                    },
                    RuntimeEvent::ClaimRetracted {
                        claim_id,
                        retracted_at_ms: now_ms(),
                        reason: format!("resource pressure changed to {pressure:?}"),
                    },
                )?;
                pressure_claim_id = None;
            }
        }
        if pressure != ResourcePressure::Normal && (pressure_changed || pressure_claim_id.is_none())
        {
            let claim_id = new_entity_id("claim");
            let claim = WorldClaim {
                claim_id: claim_id.clone(),
                subject: "device.resources".to_string(),
                predicate: "pressure_level".to_string(),
                value: json!({
                    "level": pressure,
                    "temperatureC": observation.temperature_c,
                    "memoryUsedPct": observation.memory_used_pct,
                    "cpuUsagePct": observation.cpu_usage_pct,
                }),
                confidence: 1.0,
                asserted_by_actor_id: RESOURCE_AGENT_ID.to_string(),
                evidence_event_ids: vec![observed.event_id.clone()],
                asserted_at_ms: now_ms(),
                active: true,
                retracted_at_ms: None,
                retraction_reason: None,
            };
            let asserted = self.append(
                EventContext {
                    source_actor_id: RESOURCE_AGENT_ID.to_string(),
                    causation_id: Some(observed.event_id.clone()),
                    correlation_id: Some("resource-pressure".to_string()),
                    ..EventContext::default()
                },
                RuntimeEvent::ClaimAsserted { claim },
            )?;
            directive_cause = asserted.event_id;
            pressure_claim_id = Some(claim_id);
        }

        let mut directives = Vec::new();
        if pressure_changed {
            for agent in before.agents.values() {
                let Some((action, hard)) = directive_for_agent(
                    agent,
                    previous_pressure,
                    pressure,
                    self.inner.governor_policy,
                ) else {
                    continue;
                };
                let directive = ControlDirective {
                    directive_id: new_entity_id("directive"),
                    target_agent_id: agent.agent_id.clone(),
                    action,
                    reason: format!(
                        "resource pressure transition {previous_pressure:?} -> {pressure:?}"
                    ),
                    hard,
                    issued_at_ms: now_ms(),
                    status: DirectiveStatus::Pending,
                    acknowledged_at_ms: None,
                    acknowledgement_detail: None,
                };
                self.append(
                    EventContext {
                        source_actor_id: GOVERNOR_ACTOR_ID.to_string(),
                        mission_id: agent.mission_id.clone(),
                        run_id: agent.run_id.clone(),
                        causation_id: Some(directive_cause.clone()),
                        correlation_id: Some("resource-pressure".to_string()),
                    },
                    RuntimeEvent::DirectiveIssued {
                        directive: directive.clone(),
                    },
                )?;
                directives.push(directive);
            }
        }

        Ok(ResourceDecision {
            pressure,
            observation_event_id: observed.event_id,
            pressure_claim_id,
            directives,
        })
    }

    pub fn acknowledge_directive(
        &self,
        directive_id: &str,
        applied: bool,
        detail: String,
    ) -> Result<ControlDirective> {
        let snapshot = self.inner.snapshot.read();
        let directive = snapshot
            .directives
            .get(directive_id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown directive {directive_id}"))?;
        if directive.status != DirectiveStatus::Pending {
            bail!("directive {directive_id} was already acknowledged");
        }
        let agent = snapshot
            .agents
            .get(&directive.target_agent_id)
            .cloned()
            .ok_or_else(|| anyhow!("directive target no longer exists"))?;
        drop(snapshot);

        let status = if applied {
            DirectiveStatus::Applied
        } else {
            DirectiveStatus::Rejected
        };
        let resulting_state = if applied {
            match directive.action {
                DirectiveAction::Pause => AgentState::Paused,
                DirectiveAction::Resume => AgentState::Running,
                DirectiveAction::Stop => AgentState::Stopped,
            }
        } else {
            agent.observed_state
        };
        self.append(
            EventContext {
                source_actor_id: format!("adapter:{}", directive.target_agent_id),
                mission_id: agent.mission_id,
                run_id: agent.run_id,
                causation_id: Some(directive.directive_id.clone()),
                correlation_id: Some("resource-pressure".to_string()),
            },
            RuntimeEvent::DirectiveAcknowledged {
                directive_id: directive.directive_id.clone(),
                target_agent_id: directive.target_agent_id.clone(),
                status,
                resulting_state,
                acknowledged_at_ms: now_ms(),
                detail,
            },
        )?;
        self.inner
            .snapshot
            .read()
            .directives
            .get(directive_id)
            .cloned()
            .ok_or_else(|| anyhow!("directive projection disappeared"))
    }

    pub fn list_events(
        &self,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Vec<EventEnvelope>> {
        let limit = limit.clamp(1, 1_000);
        let after_sequence = after_sequence.unwrap_or(0);
        let events = read_ledger_events(&self.inner.ledger_path)?;
        Ok(events
            .into_iter()
            .filter(|event| event.sequence > after_sequence)
            .take(limit)
            .collect())
    }

    fn bootstrap(&self) -> Result<()> {
        self.append(
            EventContext::kernel(),
            RuntimeEvent::RuntimeStarted {
                process_id: std::process::id(),
            },
        )?;
        if self.inner.snapshot.read().identity.is_none() {
            self.append(
                EventContext::kernel(),
                RuntimeEvent::IdentityDeclared {
                    identity: PinvouIdentity::default(),
                },
            )?;
        }
        for agent in builtin_system_agents(now_ms()) {
            if !self
                .inner
                .snapshot
                .read()
                .agents
                .contains_key(&agent.agent_id)
            {
                self.append(
                    EventContext::kernel(),
                    RuntimeEvent::AgentRegistered { agent },
                )?;
            }
        }
        Ok(())
    }

    fn append(&self, context: EventContext, event: RuntimeEvent) -> Result<EventEnvelope> {
        let _append_guard = self.inner.append_lock.lock();
        let sequence = self.inner.next_sequence.load(Ordering::Relaxed);
        let occurred_at_ms = now_ms();
        let envelope = EventEnvelope {
            schema_version: SCHEMA_VERSION,
            sequence,
            event_id: format!("event-{sequence:016x}"),
            occurred_at_ms,
            source_actor_id: context.source_actor_id,
            mission_id: context.mission_id,
            run_id: context.run_id,
            causation_id: context.causation_id,
            correlation_id: context.correlation_id,
            event,
        };
        append_envelope(&self.inner.ledger_path, &envelope)?;
        apply_event(&mut self.inner.snapshot.write(), &envelope);
        self.inner
            .next_sequence
            .store(sequence.saturating_add(1), Ordering::Relaxed);
        let sink = self.inner.event_sink.read().clone();
        drop(_append_guard);
        if let Some(sink) = sink {
            sink(envelope.clone());
        }
        Ok(envelope)
    }
}

fn append_envelope(path: &Path, envelope: &EventEnvelope) -> Result<()> {
    let mut options = OpenOptions::new();
    options.create(true).append(true).read(true).write(true);
    super::platform::configure_private_ledger(&mut options);
    let mut file = options
        .open(path)
        .with_context(|| format!("open PinvouOS ledger {}", path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("lock PinvouOS ledger {}", path.display()))?;
    // 进程在一行中途被强杀时，文件尾可能没有换行。先把残行封口，保证本次
    // 完整事件单独成行；重放会只跳过那条残行，不会把恢复后的第一条有效事件
    // 一起吞掉。
    if file.metadata().map(|metadata| metadata.len()).unwrap_or(0) > 0 {
        file.seek(std::io::SeekFrom::End(-1))
            .context("seek PinvouOS ledger tail")?;
        let mut tail = [0_u8; 1];
        file.read_exact(&mut tail)
            .context("read PinvouOS ledger tail")?;
        if tail[0] != b'\n' {
            file.write_all(b"\n")
                .context("terminate torn PinvouOS ledger record")?;
        }
    }
    let mut payload = serde_json::to_vec(envelope).context("serialize PinvouOS event")?;
    payload.push(b'\n');
    file.write_all(&payload)
        .with_context(|| format!("append PinvouOS ledger {}", path.display()))?;
    file.flush().context("flush PinvouOS ledger")?;
    file.sync_data().context("sync PinvouOS ledger")?;
    fs2::FileExt::unlock(&file).context("unlock PinvouOS ledger")?;
    Ok(())
}

fn replay_ledger(path: &Path) -> Result<RuntimeSnapshot> {
    let mut snapshot = RuntimeSnapshot::default();
    for event in read_ledger_events(path)? {
        if event.sequence <= snapshot.last_sequence {
            log::warn!(
                "ignoring out-of-order PinvouOS event sequence={} last={}",
                event.sequence,
                snapshot.last_sequence
            );
            continue;
        }
        apply_event(&mut snapshot, &event);
    }
    Ok(snapshot)
}

fn read_ledger_events(path: &Path) -> Result<Vec<EventEnvelope>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("read PinvouOS ledger {}", path.display()))
        }
    };
    let mut events = Vec::new();
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = line.with_context(|| format!("read PinvouOS ledger line {}", index + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<EventEnvelope>(&line) {
            Ok(event) if event.schema_version > SCHEMA_VERSION => {
                bail!(
                    "PinvouOS ledger schema v{} is newer than supported v{}",
                    event.schema_version,
                    SCHEMA_VERSION
                );
            }
            Ok(event) => events.push(event),
            Err(error) => log::warn!(
                "ignoring malformed PinvouOS ledger line {} in {}: {}",
                index + 1,
                path.display(),
                error
            ),
        }
    }
    Ok(events)
}

fn apply_event(snapshot: &mut RuntimeSnapshot, envelope: &EventEnvelope) {
    snapshot.last_sequence = envelope.sequence;
    match &envelope.event {
        RuntimeEvent::RuntimeStarted { .. } => {}
        RuntimeEvent::IdentityDeclared { identity } => snapshot.identity = Some(identity.clone()),
        RuntimeEvent::AgentRegistered { agent } => {
            snapshot
                .agents
                .insert(agent.agent_id.clone(), agent.clone());
        }
        RuntimeEvent::MissionOpened { mission } => {
            snapshot
                .missions
                .insert(mission.mission_id.clone(), mission.clone());
        }
        RuntimeEvent::RunStarted { run } => {
            snapshot.runs.insert(run.run_id.clone(), run.clone());
        }
        RuntimeEvent::ResourceObserved {
            observation,
            pressure,
        } => {
            snapshot.resources.pressure = *pressure;
            snapshot.resources.last_observation = Some(observation.clone());
        }
        RuntimeEvent::ClaimAsserted { claim } => {
            if claim.subject == "device.resources" && claim.predicate == "pressure_level" {
                snapshot.resources.active_pressure_claim_id = Some(claim.claim_id.clone());
            }
            snapshot
                .claims
                .insert(claim.claim_id.clone(), claim.clone());
        }
        RuntimeEvent::ClaimRetracted {
            claim_id,
            retracted_at_ms,
            reason,
        } => {
            if let Some(claim) = snapshot.claims.get_mut(claim_id) {
                claim.active = false;
                claim.retracted_at_ms = Some(*retracted_at_ms);
                claim.retraction_reason = Some(reason.clone());
            }
            if snapshot.resources.active_pressure_claim_id.as_deref() == Some(claim_id) {
                snapshot.resources.active_pressure_claim_id = None;
            }
        }
        RuntimeEvent::DirectiveIssued { directive } => {
            if let Some(agent) = snapshot.agents.get_mut(&directive.target_agent_id) {
                agent.desired_state = match directive.action {
                    DirectiveAction::Pause => AgentState::Paused,
                    DirectiveAction::Resume => AgentState::Running,
                    DirectiveAction::Stop => AgentState::Stopped,
                };
            }
            snapshot
                .directives
                .insert(directive.directive_id.clone(), directive.clone());
        }
        RuntimeEvent::DirectiveAcknowledged {
            directive_id,
            target_agent_id,
            status,
            resulting_state,
            acknowledged_at_ms,
            detail,
        } => {
            if let Some(directive) = snapshot.directives.get_mut(directive_id) {
                directive.status = *status;
                directive.acknowledged_at_ms = Some(*acknowledged_at_ms);
                directive.acknowledgement_detail = Some(detail.clone());
            }
            if *status == DirectiveStatus::Applied {
                if let Some(agent) = snapshot.agents.get_mut(target_agent_id) {
                    agent.observed_state = *resulting_state;
                }
            }
        }
    }
}

fn required_text(value: &str, label: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        bail!("{label} must not be empty");
    }
    if value.chars().count() > 512 {
        bail!("{label} is too long");
    }
    Ok(value.to_string())
}

fn validate_capability_contracts(capabilities: &[CapabilityContract]) -> Result<()> {
    if capabilities.is_empty() {
        bail!("mission agent must declare at least one capability contract");
    }
    for capability in capabilities {
        required_text(&capability.capability_id, "capability id")?;
        required_text(&capability.summary, "capability summary")?;
        if capability.version == 0 {
            bail!("capability version must be positive");
        }
        if !capability.input_schema.is_object() || !capability.output_schema.is_object() {
            bail!("capability input/output schema must be JSON objects");
        }
    }
    Ok(())
}

fn validate_resource_observation(observation: &ResourceObservation) -> Result<()> {
    for (label, value) in [
        ("cpu_usage_pct", observation.cpu_usage_pct),
        ("memory_used_pct", observation.memory_used_pct),
        ("gpu_usage_pct", observation.gpu_usage_pct),
    ] {
        if value.is_some_and(|value| !value.is_finite() || !(0.0..=100.0).contains(&value)) {
            bail!("{label} must be between 0 and 100");
        }
    }
    if observation
        .temperature_c
        .is_some_and(|value| !value.is_finite() || !(-50.0..=150.0).contains(&value))
    {
        bail!("temperature_c is outside the supported range");
    }
    if observation
        .power_w
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        bail!("power_w must be non-negative");
    }
    Ok(())
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn new_entity_id(prefix: &str) -> String {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let serial = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!(
        "{prefix}-{:x}-{:x}-{serial:x}",
        now_ms().max(0),
        std::process::id()
    )
}

fn builtin_system_agents(created_at_ms: i64) -> Vec<AgentManifest> {
    vec![
        builtin_agent(
            "agent:front",
            "Front Agent",
            "与用户保持唯一、连续的 Pinvou 交互面",
            capability(
                "user.interact",
                "接收一次用户意图并形成一个一致的 Pinvou 响应",
                ResourceClass::Moderate,
                Interruptibility::Checkpoint,
                &[],
                &[],
            ),
            AgentState::Running,
            100,
            created_at_ms,
        ),
        builtin_agent(
            "agent:surface",
            "Surface Agent",
            "观察当前屏幕与可访问性场景图",
            capability(
                "surface.observe",
                "把当前屏幕投影为带证据的结构化 Surface IR",
                ResourceClass::Heavy,
                Interruptibility::Immediate,
                &["screen_capture_or_accessibility_available"],
                &["screen_read"],
            ),
            AgentState::Starting,
            85,
            created_at_ms,
        ),
        builtin_agent(
            RESOURCE_AGENT_ID,
            "Resource Agent",
            "持续观察设备关键资源并提交 Claim",
            capability(
                "resource.observe",
                "采集一次设备资源快照并判定资源压力",
                ResourceClass::Light,
                Interruptibility::Immediate,
                &[],
                &["device_metrics_read"],
            ),
            AgentState::Running,
            100,
            created_at_ms,
        ),
        builtin_agent(
            "agent:device",
            "Device Agent",
            "维护设备、网络与外设能力事实",
            capability(
                "device.inspect",
                "查询一次设备或连接能力并返回结构化事实",
                ResourceClass::Light,
                Interruptibility::Immediate,
                &[],
                &["device_metadata_read"],
            ),
            AgentState::Starting,
            90,
            created_at_ms,
        ),
        builtin_agent(
            "agent:memory",
            "Memory Agent",
            "维护连续身份所需的长期与工作记忆",
            capability(
                "memory.context",
                "为一个目标编译最小、可追溯的相关上下文",
                ResourceClass::Moderate,
                Interruptibility::Checkpoint,
                &[],
                &["private_memory_read"],
            ),
            AgentState::Starting,
            95,
            created_at_ms,
        ),
        builtin_agent(
            "agent:policy",
            "Policy Agent",
            "把权限、安全与用户边界投影为可执行约束",
            capability(
                "policy.authorize",
                "对一个拟议动作给出允许、拒绝或需确认的决定",
                ResourceClass::Light,
                Interruptibility::Immediate,
                &[],
                &[],
            ),
            AgentState::Starting,
            100,
            created_at_ms,
        ),
        builtin_agent(
            "agent:attention",
            "Attention Agent",
            "在并发目标之间分配注意力与打断预算",
            capability(
                "attention.allocate",
                "根据优先级、时限和资源为活跃 Run 计算注意力分配",
                ResourceClass::Light,
                Interruptibility::Immediate,
                &[],
                &[],
            ),
            AgentState::Starting,
            95,
            created_at_ms,
        ),
    ]
}

fn builtin_agent(
    agent_id: &str,
    display_name: &str,
    role: &str,
    capability: CapabilityContract,
    observed_state: AgentState,
    priority: u8,
    created_at_ms: i64,
) -> AgentManifest {
    AgentManifest {
        agent_id: agent_id.to_string(),
        display_name: display_name.to_string(),
        kind: AgentKind::System,
        role: role.to_string(),
        interruptibility: capability.interruptibility,
        capabilities: vec![capability],
        priority,
        observed_state,
        desired_state: AgentState::Running,
        mission_id: None,
        run_id: None,
        created_at_ms,
    }
}

fn capability(
    capability_id: &str,
    summary: &str,
    resource_class: ResourceClass,
    interruptibility: Interruptibility,
    preconditions: &[&str],
    permissions: &[&str],
) -> CapabilityContract {
    CapabilityContract {
        capability_id: capability_id.to_string(),
        version: 1,
        summary: summary.to_string(),
        input_schema: json!({ "type": "object" }),
        output_schema: json!({ "type": "object" }),
        preconditions: preconditions
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        permissions: permissions
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        side_effects: Vec::new(),
        resource_class,
        interruptibility,
        idempotent: false,
    }
}
