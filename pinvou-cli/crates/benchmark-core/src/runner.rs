use std::sync::{Arc, Mutex};

use agent_backend_api::{
    AgentBackendError, AgentOutputContractId, AgentRunObserver, AgentTaskInput, AgentToolPolicyId,
    HeadlessAgentBackend, PrepareRequest, PrivateInputHandle, PrivateInputResolver,
    ResolvedPrivateInput, SafeAgentEvent, SafeRunStatus,
};
use async_trait::async_trait;

use crate::{
    BenchmarkError, BenchmarkTask, ExecutionRequest, Prediction, Result, RunContext, TaskOutcome,
    TaskStatus, ToolObservation,
};

#[derive(Default)]
struct CollectingObserver(Mutex<Vec<ToolObservation>>);
impl AgentRunObserver for CollectingObserver {
    fn on_event(&self, event: &SafeAgentEvent) {
        if let SafeAgentEvent::ToolFinished {
            tool_name,
            status,
            elapsed,
            ..
        } = event
            && let Ok(mut tools) = self.0.lock()
        {
            tools.push(ToolObservation {
                canonical_name: safe_tool_name(tool_name),
                failed: *status != SafeRunStatus::Completed,
                elapsed_ms: elapsed.as_millis() as u64,
            });
        }
    }
}

fn safe_tool_name(name: &str) -> String {
    const ALLOWED: &[&str] = &[
        "File",
        "Web",
        "image_analyze",
        "web_search",
        "fetch_url",
        "exec_shell",
        "read_file",
        "write_file",
        "append_file",
        "edit_file",
        "mcp_pinvou3_present_artifact",
        "kb_search",
        "kb_open_source",
        "web_fetch",
        "apply_patch",
        "list_files",
    ];
    if ALLOWED.contains(&name) {
        name.to_owned()
    } else {
        "[redacted-tool]".to_owned()
    }
}

#[async_trait]
pub trait TaskRunner: Send + Sync {
    async fn run_task(&self, task: &BenchmarkTask, context: &RunContext) -> Result<TaskOutcome>;
}

pub struct NativeAgentRunner<B> {
    backend: Arc<B>,
    private_inputs: Arc<dyn PrivateInputResolver>,
}

impl<B> NativeAgentRunner<B> {
    pub fn new(backend: Arc<B>) -> Self {
        Self {
            backend,
            private_inputs: Arc::new(UnavailablePrivateInputs),
        }
    }

    pub fn with_private_inputs(
        backend: Arc<B>,
        private_inputs: Arc<dyn PrivateInputResolver>,
    ) -> Self {
        Self {
            backend,
            private_inputs,
        }
    }
}

impl<B> NativeAgentRunner<B>
where
    B: HeadlessAgentBackend + 'static,
{
    async fn cleanup_timed_out_session(&self, session: &agent_backend_api::AgentSessionHandle) {
        let cleanup_timeout = std::time::Duration::from_secs(2);
        let _ = tokio::time::timeout(cleanup_timeout, self.backend.cancel(session)).await;
        let _ = tokio::time::timeout(cleanup_timeout, self.backend.close(session.clone())).await;
    }
}

struct UnavailablePrivateInputs;

#[async_trait]
impl PrivateInputResolver for UnavailablePrivateInputs {
    async fn resolve(
        &self,
        _handle: &PrivateInputHandle,
    ) -> std::result::Result<ResolvedPrivateInput, AgentBackendError> {
        Err(AgentBackendError::Operation(
            "private input resolver is not configured".into(),
        ))
    }
}

#[async_trait]
impl<B> TaskRunner for NativeAgentRunner<B>
where
    B: HeadlessAgentBackend + 'static,
{
    async fn run_task(&self, task: &BenchmarkTask, _context: &RunContext) -> Result<TaskOutcome> {
        let (prompt, attachments, timeout_duration, tool_policy, output_contract) =
            match task.execution() {
                ExecutionRequest::NativeTurn {
                    prompt_handle,
                    attachments,
                    timeout,
                    tool_policy,
                    output_contract,
                } => (
                    prompt_handle.clone(),
                    attachments.clone(),
                    *timeout,
                    tool_policy.as_str().to_owned(),
                    output_contract.as_str().to_owned(),
                ),
                ExecutionRequest::ExternalHarness { .. } => {
                    return Err(BenchmarkError::coded("external_harness_unsupported"));
                }
            };
        let tool_policy = AgentToolPolicyId::new(tool_policy)
            .map_err(|_| BenchmarkError::coded("unsupported_tool_policy"))?;
        let output_contract = AgentOutputContractId::new(output_contract)
            .map_err(|_| BenchmarkError::coded("unsupported_output_contract"))?;
        let deadline = tokio::time::Instant::now() + timeout_duration;
        let mut resolved_attachments = Vec::with_capacity(attachments.len());
        for attachment in &attachments {
            let resolved = tokio::time::timeout_at(
                deadline,
                self.private_inputs.resolve_attachment(attachment),
            )
            .await
            .map_err(|_| BenchmarkError::coded("task_timeout"))?
            .map_err(|_| BenchmarkError::coded("attachment_resolution_failed"))?;
            resolved_attachments.push(resolved);
        }
        let session = tokio::time::timeout_at(
            deadline,
            self.backend.prepare(
                PrepareRequest::new(task.task_id(), attachments)
                    .with_resolved_attachments(resolved_attachments)
                    .with_tool_policy(tool_policy),
            ),
        )
        .await
        .map_err(|_| BenchmarkError::coded("task_timeout"))?
        .map_err(|_| BenchmarkError::coded("backend_prepare_failed"))?;
        let observer = Arc::new(CollectingObserver::default());
        let result = tokio::time::timeout_at(
            deadline,
            self.backend.run(
                &session,
                AgentTaskInput::new(task.task_id(), prompt).with_output_contract(output_contract),
                self.private_inputs.clone(),
                observer.clone(),
            ),
        )
        .await;
        if result.is_err() {
            self.cleanup_timed_out_session(&session).await;
            return Err(BenchmarkError::coded("task_timeout"));
        }
        let result = result.expect("timeout branch returned above");
        let private_output = match &result {
            Ok(outcome) => match outcome.output_handle() {
                Some(handle) => {
                    match tokio::time::timeout_at(deadline, self.backend.resolve_output(handle))
                        .await
                    {
                        Ok(resolved) => Some(resolved),
                        Err(_) => {
                            self.cleanup_timed_out_session(&session).await;
                            return Err(BenchmarkError::coded("task_timeout"));
                        }
                    }
                }
                None => None,
            },
            Err(_) => None,
        };
        let close_result =
            match tokio::time::timeout_at(deadline, self.backend.close(session.clone())).await {
                Ok(result) => result,
                Err(_) => {
                    self.cleanup_timed_out_session(&session).await;
                    return Err(BenchmarkError::coded("task_timeout"));
                }
            };
        if close_result.is_err() {
            return Err(BenchmarkError::coded("backend_close_failed"));
        }
        let outcome = result.map_err(|error| match error {
            AgentBackendError::Operation(code) if code == "missing_final_answer" => {
                BenchmarkError::coded("missing_final_answer")
            }
            _ => BenchmarkError::coded("backend_run_failed"),
        })?;
        let private_output = match private_output {
            Some(Ok(output)) => Some(output),
            Some(Err(_)) => return Err(BenchmarkError::coded("private_output_resolution_failed")),
            None => None,
        };
        let status = match outcome.status() {
            SafeRunStatus::Completed => TaskStatus::Completed,
            SafeRunStatus::Failed => TaskStatus::Failed,
            SafeRunStatus::Cancelled => TaskStatus::Cancelled,
        };
        let prediction = outcome
            .output_handle()
            .map(|handle| Prediction::backend(handle.expose_to_backend()));
        let tools = observer
            .0
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default();
        let usage = outcome.usage().map(|usage| crate::UsageMetrics {
            input_tokens: usage.input_tokens(),
            output_tokens: usage.output_tokens(),
            cache_hit_tokens: usage.cache_hit_tokens(),
            cache_miss_tokens: usage.cache_miss_tokens(),
        });
        let mut result = TaskOutcome::new(
            task.task_id(),
            status,
            prediction,
            Vec::new(),
            outcome.elapsed().as_millis() as u64,
        )
        .with_tool_observations(tools);
        if let Some(usage) = usage {
            result = result.with_usage(usage);
        }
        if let Some(output) = private_output {
            result = result.with_private_output(output);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::safe_tool_name;

    #[test]
    fn product_policy_tool_names_are_preserved_in_observations() {
        for name in ["File", "Web", "image_analyze"] {
            assert_eq!(safe_tool_name(name), name);
        }
        assert_eq!(safe_tool_name("private-tool-sentinel"), "[redacted-tool]");
    }
}
