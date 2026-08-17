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
