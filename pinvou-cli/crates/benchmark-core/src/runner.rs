use std::sync::{Arc, Mutex};
use std::time::Instant;

use agent_backend_api::{
    AgentBackendError, AgentOutputContractId, AgentRunObserver, AgentTaskInput, AgentTaskOutcome,
    AgentToolPolicyId, HeadlessAgentBackend, PrepareRequest, PrivateInputHandle,
    PrivateInputResolver, ResolvedPrivateInput, SafeAgentEvent, SafeRunStatus,
};
use async_trait::async_trait;

use crate::{
    BenchmarkError, BenchmarkTask, ExecutionRequest, ModelRequestObservation, Prediction, Result,
    RunContext, SafeFailureCategory, SafeFailureReason, TaskOutcome, TaskStatus, ToolObservation,
};

#[derive(Default)]
struct CollectingObserver(Mutex<Vec<ToolObservation>>);
impl AgentRunObserver for CollectingObserver {
    fn on_event(&self, event: &SafeAgentEvent) {
        if let SafeAgentEvent::ToolFinished {
            tool_name,
            status,
            elapsed,
            failure_code,
            ..
        } = event
            && let Ok(mut tools) = self.0.lock()
        {
            tools.push(ToolObservation {
                canonical_name: safe_tool_name(tool_name),
                failed: *status != SafeRunStatus::Completed,
                elapsed_ms: elapsed.as_millis() as u64,
                failure_code: failure_code.as_deref().map(safe_failure_code),
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
        "[unexpected-tool]",
    ];
    if ALLOWED.contains(&name) {
        name.to_owned()
    } else {
        "[redacted-tool]".to_owned()
    }
}

fn safe_failure_code(code: &str) -> String {
    // Tool failure codes arrive from arbitrary backend implementations.
    // Persist only conservative snake_case tokens; anything else collapses
    // to the aggregator's unknown-code sentinel, which already blocks
    // evaluation eligibility downstream.
    let is_safe = !code.is_empty()
        && code.len() <= 64
        && code
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if is_safe {
        code.to_owned()
    } else {
        "unclassified".to_owned()
    }
}

fn collected_tools(observer: &CollectingObserver) -> Vec<ToolObservation> {
    observer
        .0
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default()
}

fn backend_metrics(
    outcome: Option<&AgentTaskOutcome>,
) -> (Option<crate::UsageMetrics>, Vec<ModelRequestObservation>) {
    let Some(outcome) = outcome else {
        return (None, Vec::new());
    };
    let usage = outcome.usage().map(|usage| crate::UsageMetrics {
        input_tokens: usage.input_tokens(),
        output_tokens: usage.output_tokens(),
        cache_hit_tokens: usage.cache_hit_tokens(),
        cache_miss_tokens: usage.cache_miss_tokens(),
    });
    let model_requests = outcome
        .model_request_metrics()
        .iter()
        .map(|metric| ModelRequestObservation {
            request_duration_ms: metric.request_duration_ms(),
            ttft_ms: metric.ttft_ms(),
            input_tokens: metric.input_tokens(),
            output_tokens: metric.output_tokens(),
        })
        .collect();
    (usage, model_requests)
}

fn failed_task_outcome(
    task_id: &str,
    code: &str,
    elapsed_ms: u64,
    tools: Vec<ToolObservation>,
) -> TaskOutcome {
    // This table classifies codes surfaced through Ok-terminal outcomes or
    // backend.run Err(Operation). A few prepare-phase codes still propagate
    // as run_task Err and are classified by the service Err arm instead;
    // the two tables split by layer, so a code belongs to exactly one of
    // them (for example attachment_resolution_failed only ever reaches the
    // service arm).
    let (status, category, reason) = match code {
        "task_timeout" => (
            TaskStatus::Timeout,
            SafeFailureCategory::Timeout,
            Some(SafeFailureReason::TaskTimeout),
        ),
        "model_request_timeout" => (
            TaskStatus::Timeout,
            SafeFailureCategory::Timeout,
            Some(SafeFailureReason::ModelRequestTimeout),
        ),
        "missing_final_answer" => (
            TaskStatus::Failed,
            SafeFailureCategory::InvalidOutput,
            Some(SafeFailureReason::MissingFinalAnswer),
        ),
        "private_output_resolution_failed" => (
            TaskStatus::Failed,
            SafeFailureCategory::InvalidOutput,
            Some(SafeFailureReason::PrivateOutputResolutionFailed),
        ),
        "model_context_limit" => (
            TaskStatus::Failed,
            SafeFailureCategory::Backend,
            Some(SafeFailureReason::ModelContextLimit),
        ),
        "model_rate_limited" => (
            TaskStatus::Failed,
            SafeFailureCategory::Backend,
            Some(SafeFailureReason::ModelRateLimited),
        ),
        "model_transport_failed" => (
            TaskStatus::Failed,
            SafeFailureCategory::Backend,
            Some(SafeFailureReason::ModelTransportFailed),
        ),
        "model_protocol_failed" => (
            TaskStatus::Failed,
            SafeFailureCategory::Backend,
            Some(SafeFailureReason::ModelProtocolFailed),
        ),
        "agent_tool_failed" => (
            TaskStatus::Failed,
            SafeFailureCategory::Backend,
            Some(SafeFailureReason::AgentToolFailed),
        ),
        "attachment_staging_failed" | "attachments_runtime_unsupported" => (
            TaskStatus::Failed,
            SafeFailureCategory::Infrastructure,
            Some(SafeFailureReason::AttachmentStagingFailed),
        ),
        "backend_prepare_failed" => (
            TaskStatus::Failed,
            SafeFailureCategory::Backend,
            Some(SafeFailureReason::BackendPrepareFailed),
        ),
        "backend_close_failed" => (
            TaskStatus::Failed,
            SafeFailureCategory::Backend,
            Some(SafeFailureReason::BackendCloseFailed),
        ),
        "agent_turn_failed" => (
            TaskStatus::Failed,
            SafeFailureCategory::Backend,
            Some(SafeFailureReason::AgentTurnFailed),
        ),
        // Known lifecycle codes (session_closed, private_session_state_failed,
        // private_output_store_failed, private_output_not_found,
        // gaia_private_input_unknown, unsupported_tool_policy) intentionally
        // share this arm with unknown codes: adding a lifecycle code never
        // requires extending this table.
        _ => (
            TaskStatus::Failed,
            SafeFailureCategory::Infrastructure,
            Some(SafeFailureReason::IntegrationLifecycleFailed),
        ),
    };
    let mut outcome = TaskOutcome::new(task_id, status, None, Vec::new(), elapsed_ms)
        .with_failure_category(category)
        .with_tool_observations(tools);
    if let Some(reason) = reason {
        outcome = outcome.with_failure_reason(reason);
    }
    outcome
}

/// Terminal failure after `backend.run` already returned an outcome, so the
/// request-level metrics it carries must not be dropped (task timeout during
/// output resolution or close, close failure, private output resolution
/// failure).
fn failed_task_outcome_with_metrics(
    task_id: &str,
    code: &str,
    elapsed_ms: u64,
    tools: Vec<ToolObservation>,
    backend_outcome: Option<&AgentTaskOutcome>,
) -> TaskOutcome {
    let (usage, model_requests) = backend_metrics(backend_outcome);
    let mut outcome = failed_task_outcome(task_id, code, elapsed_ms, tools)
        .with_model_request_observations(model_requests);
    if let Some(usage) = usage {
        outcome = outcome.with_usage(usage);
    }
    outcome
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
        let run_started = Instant::now();
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
            let elapsed = run_started.elapsed().as_millis() as u64;
            self.cleanup_timed_out_session(&session).await;
            return Ok(failed_task_outcome(
                task.task_id(),
                "task_timeout",
                elapsed,
                collected_tools(observer.as_ref()),
            ));
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
                            let elapsed = run_started.elapsed().as_millis() as u64;
                            self.cleanup_timed_out_session(&session).await;
                            return Ok(failed_task_outcome_with_metrics(
                                task.task_id(),
                                "task_timeout",
                                elapsed,
                                collected_tools(observer.as_ref()),
                                Some(outcome),
                            ));
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
                    let elapsed = run_started.elapsed().as_millis() as u64;
                    self.cleanup_timed_out_session(&session).await;
                    return Ok(failed_task_outcome_with_metrics(
                        task.task_id(),
                        "task_timeout",
                        elapsed,
                        collected_tools(observer.as_ref()),
                        result.as_ref().ok(),
                    ));
                }
            };
        if close_result.is_err() {
            return Ok(failed_task_outcome_with_metrics(
                task.task_id(),
                "backend_close_failed",
                run_started.elapsed().as_millis() as u64,
                collected_tools(observer.as_ref()),
                result.as_ref().ok(),
            ));
        }
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(AgentBackendError::Operation(code)) => {
                return Ok(failed_task_outcome(
                    task.task_id(),
                    &code,
                    run_started.elapsed().as_millis() as u64,
                    collected_tools(observer.as_ref()),
                ));
            }
        };
        let private_output = match private_output {
            Some(Ok(output)) => Some(output),
            Some(Err(_)) => {
                return Ok(failed_task_outcome_with_metrics(
                    task.task_id(),
                    "private_output_resolution_failed",
                    run_started.elapsed().as_millis() as u64,
                    collected_tools(observer.as_ref()),
                    Some(&outcome),
                ));
            }
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
        let tools = collected_tools(observer.as_ref());
        let (usage, model_requests) = backend_metrics(Some(&outcome));
        let mut result = if status == TaskStatus::Failed {
            failed_task_outcome(
                task.task_id(),
                outcome
                    .failure_code()
                    .unwrap_or("integration_lifecycle_failed"),
                outcome.elapsed().as_millis() as u64,
                tools,
            )
            .with_model_request_observations(model_requests)
        } else {
            TaskOutcome::new(
                task.task_id(),
                status,
                prediction,
                Vec::new(),
                outcome.elapsed().as_millis() as u64,
            )
            .with_tool_observations(tools)
            .with_model_request_observations(model_requests)
        };
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
    use super::{safe_failure_code, safe_tool_name};

    #[test]
    fn product_policy_tool_names_are_preserved_in_observations() {
        for name in ["File", "Web", "image_analyze"] {
            assert_eq!(safe_tool_name(name), name);
        }
        assert_eq!(safe_tool_name("[unexpected-tool]"), "[unexpected-tool]");
        assert_eq!(safe_tool_name("private-tool-sentinel"), "[redacted-tool]");
    }

    #[test]
    fn tool_failure_codes_are_persisted_only_as_safe_tokens() {
        assert_eq!(safe_failure_code("missing_action"), "missing_action");
        assert_eq!(
            safe_failure_code("http_status_failed"),
            "http_status_failed"
        );
        assert_eq!(
            safe_failure_code("Backend exploded: HTTP 500\u{0}"),
            "unclassified"
        );
        assert_eq!(safe_failure_code("LEAK; DROP TABLE tasks"), "unclassified");
        assert_eq!(safe_failure_code(""), "unclassified");
        assert_eq!(safe_failure_code(&"a".repeat(65)), "unclassified");
        assert_eq!(safe_failure_code(&"a".repeat(64)), "a".repeat(64));
    }
}
