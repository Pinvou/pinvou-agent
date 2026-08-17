//! PinvouOS 的连续运行时领域。
//!
//! 这里没有 Session：一个稳定 Pinvou Identity 持续存在；Mission、Run、Agent、
//! Event、Claim 与 Directive 表达任务、并发、因果和治理。CodeWhale/ACP 的线程标识
//! 只能由后续 execution adapter 私有持有，不能进入本模块协议。

mod governor;
mod model;
mod platform;
mod resource_agent;
mod runtime;

pub use governor::{classify_pressure, ResourceGovernorPolicy};
pub use model::*;
pub use resource_agent::{spawn_resource_agent, ResourceSampler};
pub use runtime::{OpenMissionRequest, PinvouOsRuntime, RegisterMissionAgentRequest};

#[cfg(test)]
mod tests;
