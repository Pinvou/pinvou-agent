use std::future::Future;
use std::sync::Arc;

use agent_backend_api::HeadlessAgentBackend;
use anyhow::Result;

pub fn run_with_product_backend<T, Work, WorkFuture>(work: Work) -> Result<T>
where
    T: Send + 'static,
    Work: FnOnce(Arc<dyn HeadlessAgentBackend>) -> WorkFuture + Send + 'static,
    WorkFuture: Future<Output = Result<T>> + Send + 'static,
{
    pinvou3_lib::headless_bridge::run_headless_host(work)
}

/// Run one agentic single task in the windowed product host
/// (product-equivalent toolchain, including shell). Types are re-exported
/// through `pinvou3_lib` so callers need no engine-internal dependency.
pub fn run_agentic_task(
    request: pinvou3_lib::agentic_task::AgenticTaskRequest,
) -> Result<pinvou3_lib::agentic_task::AgenticTaskReport> {
    pinvou3_lib::agentic_task::run_agentic_task_headless(request)
}

pub use pinvou3_lib::agentic_task::{AgenticTaskReport, AgenticTaskRequest, MAX_TIMEOUT_SECS};
