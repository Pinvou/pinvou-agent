mod adapter;
mod contracts;
mod error;
mod event;
mod manifest;
// C1 freezes the private store; C2 wires it into runner/service.
#[allow(dead_code)]
mod private_prediction;
mod registry;
mod report;
mod runner;
mod security;
mod service;
mod store;

pub use adapter::BenchmarkAdapter;
pub use contracts::*;
pub use error::{BenchmarkError, Result};
pub use event::{RunEvent, RunEventKind};
pub use manifest::{ModelIdentity, RunManifest};
pub use private_prediction::{PrivatePredictionContentType, PrivatePredictionPayload, ScorerView};
pub use registry::BenchmarkRegistry;
pub use report::{ReportArtifact, publish_markdown_report};
pub use runner::{NativeAgentRunner, TaskRunner};
pub use service::{BenchmarkService, RunSummary};
pub use store::{RecoveredRun, RunStore};
