//! PinvouOS 的连续运行时领域。
//!
//! 这里没有 Session：一个稳定 Pinvou Identity 持续存在；Mission、Run、Agent、
//! Event、Claim 与 Directive 表达任务、并发、因果和治理。CodeWhale/ACP 的线程标识
//! 只能由后续 execution adapter 私有持有，不能进入本模块协议。

mod asr_context_agent;
mod attention_agent;
mod capability_agent;
mod connectivity_agent;
mod front_agent;
mod governor;
mod inference_agent;
mod interaction_projection;
mod memory_agent;
mod model;
mod orchestrator_agent;
mod platform;
mod policy_agent;
mod resource_agent;
mod runtime;
mod screen_observer_agent;
mod tools;

pub use asr_context_agent::{
    asr_context_compile_contract, spawn_asr_context_agent, AsrContextAgent, AsrContextSnapshot,
    AsrContextTerm, ASR_CONTEXT_AGENT_ID, ASR_CONTEXT_CAPABILITY_ID, ASR_CONTEXT_MAX_TERMS,
    ASR_CONTEXT_REFRESH_INTERVAL,
};
pub use attention_agent::*;
pub use capability_agent::*;
pub use connectivity_agent::*;
pub use front_agent::{
    accept_user_interaction, FrontIntentEnvelope, FrontIntentKind, FrontResponseMode,
    UserInteractionInput, FRONT_AGENT_ID, FRONT_AGENT_INSTRUCTION,
    FRONT_VOICE_TRANSCRIPT_INSTRUCTION,
};
pub use governor::{classify_pressure, ResourceGovernorPolicy};
pub use inference_agent::*;
pub use interaction_projection::*;
pub use memory_agent::*;
pub use model::*;
pub use orchestrator_agent::{
    build_mission_work_graph, pinvou_os_fleet_config, AtomicWorkItem, CapabilityNeed,
    MissionPlanningInput, MissionWorkGraph, WorkDisposition, ORCHESTRATOR_AGENT_ID,
    ORCHESTRATOR_AGENT_INSTRUCTION, ORCHESTRATOR_PROFILE_ID,
};
pub use policy_agent::*;
pub use resource_agent::{
    assess_device_inventory, assess_resource_observation, spawn_resource_agent, ClaimCandidate,
    DeviceAgentAssessment, DeviceHealth, DeviceInventoryObservation, DeviceKind, DeviceSnapshot,
    ObservationHealth, ResourceAgentAssessment, ResourceDirectiveCandidate,
    ResourceMitigationAction, ResourceSampler, DEVICE_AGENT_ID,
};
pub use runtime::{
    HostWorkDirectiveRequest, HostWorkHandle, OpenInteractionRunRequest, OpenMissionRequest,
    PinvouOsRuntime, ReconcileHostWorkDirectiveRequest, RegisterHostWorkRequest,
    RegisterMissionAgentRequest,
};
pub use screen_observer_agent::*;
pub use tools::{
    pinvou_os_runtime_tools, ASR_CONTEXT_STATUS_TOOL_NAME, ATTENTION_PLAN_TOOL_NAME,
    CAPABILITY_REPORT_TOOL_NAME, ORCHESTRATOR_PLAN_TOOL_NAME, RUNTIME_STATUS_TOOL_NAME,
};

#[cfg(test)]
mod tests;
