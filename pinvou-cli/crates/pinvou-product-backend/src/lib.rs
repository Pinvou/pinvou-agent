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

/// 在窗口化产品宿主里执行一次 agentic 单任务(产品等价工具链,含 shell)。
/// 类型经 `pinvou3_lib` 重导出,调用方无需依赖引擎内部类型。
pub fn run_agentic_task(
    request: pinvou3_lib::agentic_task::AgenticTaskRequest,
) -> Result<pinvou3_lib::agentic_task::AgenticTaskReport> {
    pinvou3_lib::agentic_task::run_agentic_task_headless(request)
}

pub use pinvou3_lib::agentic_task::{AgenticTaskReport, AgenticTaskRequest};
