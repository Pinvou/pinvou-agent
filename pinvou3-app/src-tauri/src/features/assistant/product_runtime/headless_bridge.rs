//! Narrow, windowless adapter over the product EnginePool runtime.

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::future::Future;
use std::io::{Read, copy};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use agent_backend_api::{
    AgentBackendError, AgentRunObserver, AgentSessionHandle, AgentTaskInput, AgentTaskOutcome,
    HeadlessAgentBackend, PrepareRequest, PrivateInputResolver, PrivateOutputHandle,
    PrivateOutputResolver, ResolvedAttachmentSource, SafeAgentEvent, SafeRunStatus,
    SafeUsageMetrics, SecretOutput, SecretText, SuiteModelIdentity, notify_observer,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use deepseek_tui::tui::app::AppMode;
use tauri::Manager;
use tokio::sync::Mutex as AsyncMutex;

use crate::features::assistant::attachments::{
    build_message_with_attachments, stage_file_in_workspace,
};
use crate::features::assistant::engine_pool::{EnginePool, EngineToolFactory, ToolPolicy};
use crate::features::assistant::platform::headless_attachments::ensure_staged_attachments_supported;
use crate::features::assistant::product_runtime::eval_tool_policy::{
    EvalToolPolicy, resolve_eval_policy,
};
use crate::features::assistant::product_runtime::{
    EnginePoolRuntime, EvalSuiteModelGuard, ProductChatRuntime, SessionSpec, TurnInput,
};
use crate::features::{knowledge, sessions::SessionStore};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafeToolOutcome {
    pub name: String,
    pub failed: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProductTurnOutcome {
    pub status: String,
    pub assistant_text: String,
    pub usage: Option<SafeUsageMetrics>,
    pub tools: Vec<SafeToolOutcome>,
}

#[derive(Clone, Copy)]
pub struct ProductToolPolicy(EvalToolPolicy);

impl std::fmt::Debug for ProductTurnOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProductTurnOutcome")
            .field("status", &self.status)
            .field("assistant_text", &"[redacted]")
            .field("usage", &self.usage)
            .field("tools", &self.tools)
            .finish()
    }
}

#[async_trait]
pub trait ProductRuntimePort: Send + Sync {
    async fn prepare(&self, session_id: &str) -> Result<()>;
    async fn run(&self, session_id: &str, prompt: &str) -> Result<ProductTurnOutcome>;
    async fn run_with_policy(
        &self,
        _session_id: &str,
        _prompt: &str,
        _policy: ProductToolPolicy,
    ) -> Result<ProductTurnOutcome> {
        anyhow::bail!("unsupported_tool_policy")
    }
    async fn run_with_staged_attachments_and_policy(
        &self,
        _session_id: &str,
        _prompt: &str,
        _staged_workspace: &Path,
        _policy: ProductToolPolicy,
    ) -> Result<ProductTurnOutcome> {
        anyhow::bail!("unsupported_tool_policy")
    }
    async fn cancel(&self, session_id: &str) -> Result<()>;
    async fn close(&self, session_id: &str) -> Result<()>;
}

struct EnginePoolPort {
    runtime: EnginePoolRuntime,
    suite_model: EvalSuiteModelGuard,
}

impl EnginePoolPort {
    async fn run_content(
        &self,
        session_id: &str,
        content: String,
        eval_tool_policy: EvalToolPolicy,
    ) -> Result<ProductTurnOutcome> {
        let handle = self
            .runtime
            .submit(&TurnInput {
                session_id: session_id.to_owned(),
                content,
                mode: AppMode::Yolo,
                restrict_tools: false,
                eval_tool_policy: Some(eval_tool_policy),
            })
            .await?;
        let turn = self.runtime.wait_for_completion(&handle).await?;
        Ok(ProductTurnOutcome {
            status: turn.status,
            assistant_text: turn.assistant_text,
            usage: turn.usage.map(|usage| {
                SafeUsageMetrics::new(
                    usage.input_tokens,
                    usage.output_tokens,
                    usage.cache_hit_tokens,
                    usage.cache_miss_tokens,
                )
            }),
            tools: turn
                .tool_events
                .into_iter()
                .map(|tool| SafeToolOutcome {
                    name: tool.name,
                    failed: tool.failed,
                })
                .collect(),
        })
    }
}

fn prepare_product_attachment_content(
    prompt: String,
    staged_workspace: PathBuf,
    execution_root: PathBuf,
) -> Result<String> {
    let fixed_error = || anyhow::anyhow!("attachment_staging_failed");
    let mut entries = std::fs::read_dir(&staged_workspace)
        .map_err(|_| fixed_error())?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|_| fixed_error())?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut attachments = Vec::with_capacity(entries.len());
    for entry in entries {
        let file_type = entry.file_type().map_err(|_| fixed_error())?;
        let basename = entry.file_name().into_string().map_err(|_| fixed_error())?;
        if !file_type.is_file() || file_type.is_symlink() {
            return Err(fixed_error());
        }
        let staged_path = stage_file_in_workspace(
            entry.path().to_string_lossy().as_ref(),
            &basename,
            &execution_root,
            "attachments",
        )
        .ok_or_else(fixed_error)?;
        attachments.push(crate::features::files::file_ingest::ingest(
            &execution_root.join(staged_path),
        ));
    }
    Ok(build_message_with_attachments(
        prompt,
        attachments,
        &execution_root,
    ))
}

#[async_trait]
impl ProductRuntimePort for EnginePoolPort {
    async fn prepare(&self, session_id: &str) -> Result<()> {
        self.runtime
            .prepare(&SessionSpec {
                session_id: session_id.to_owned(),
                model_selection: Some(self.suite_model.derive_case_selection()?),
            })
            .await
    }

    async fn run(&self, _session_id: &str, _prompt: &str) -> Result<ProductTurnOutcome> {
        anyhow::bail!("unsupported_tool_policy")
    }

    async fn run_with_policy(
        &self,
        session_id: &str,
        prompt: &str,
        policy: ProductToolPolicy,
    ) -> Result<ProductTurnOutcome> {
        self.run_content(session_id, prompt.to_owned(), policy.0)
            .await
    }

    async fn run_with_staged_attachments_and_policy(
        &self,
        session_id: &str,
        prompt: &str,
        staged_workspace: &Path,
        policy: ProductToolPolicy,
    ) -> Result<ProductTurnOutcome> {
        let execution_root = self
            .runtime
            .eval_session_execution_root(session_id)
            .map_err(|_| anyhow::anyhow!("attachment_staging_failed"))?;
        let prompt = prompt.to_owned();
        let staged_workspace = staged_workspace.to_path_buf();
        let content = tokio::task::spawn_blocking(move || {
            prepare_product_attachment_content(prompt, staged_workspace, execution_root)
        })
        .await
        .map_err(|_| anyhow::anyhow!("attachment_staging_failed"))??;
        self.run_content(session_id, content, policy.0).await
    }

    async fn cancel(&self, session_id: &str) -> Result<()> {
        self.runtime.cancel(session_id).await;
        Ok(())
    }

    async fn close(&self, session_id: &str) -> Result<()> {
        self.runtime.schedule_eval_cleanup(session_id);
        self.runtime.close_eval_session_result(session_id).await
    }
}

#[derive(Clone)]
pub struct ProductHeadlessBackend {
    runtime: Arc<dyn ProductRuntimePort>,
    runtime_sessions: Arc<Mutex<HashMap<String, RuntimeSession>>>,
    private_outputs: Arc<Mutex<PrivateOutputs>>,
    attachment_workspaces: Arc<Mutex<HashMap<String, tempfile::TempDir>>>,
    tool_policies: Arc<Mutex<HashMap<String, ProductToolPolicy>>>,
    suite_model_identity: Option<SuiteModelIdentity>,
}

#[derive(Default)]
struct PrivateOutputs {
    values: HashMap<String, SecretText>,
    by_session: HashMap<String, Vec<String>>,
}

type RuntimeSession = Arc<AsyncMutex<RuntimeSessionState>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuntimeSessionState {
    Active,
    Closed,
}

struct PrepareCleanupGuard {
    runtime: Arc<dyn ProductRuntimePort>,
    session_id: String,
    armed: bool,
}

impl PrepareCleanupGuard {
    fn new(runtime: Arc<dyn ProductRuntimePort>, session_id: String) -> Self {
        Self {
            runtime,
            session_id,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PrepareCleanupGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let runtime = self.runtime.clone();
        let session_id = self.session_id.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = runtime.close(&session_id).await;
            });
        }
    }
}

impl ProductHeadlessBackend {
    pub fn from_runtime(runtime: Arc<dyn ProductRuntimePort>) -> Self {
        Self {
            runtime,
            runtime_sessions: Arc::new(Mutex::new(HashMap::new())),
            private_outputs: Arc::new(Mutex::new(PrivateOutputs::default())),
            attachment_workspaces: Arc::new(Mutex::new(HashMap::new())),
            tool_policies: Arc::new(Mutex::new(HashMap::new())),
            suite_model_identity: None,
        }
    }

    fn with_identity(runtime: Arc<dyn ProductRuntimePort>, identity: SuiteModelIdentity) -> Self {
        Self {
            runtime,
            runtime_sessions: Arc::new(Mutex::new(HashMap::new())),
            private_outputs: Arc::new(Mutex::new(PrivateOutputs::default())),
            attachment_workspaces: Arc::new(Mutex::new(HashMap::new())),
            tool_policies: Arc::new(Mutex::new(HashMap::new())),
            suite_model_identity: Some(identity),
        }
    }

    fn from_engine_pool(pool: EnginePool) -> Result<Self> {
        let runtime = EnginePoolRuntime::new(Arc::new(pool));
        let suite_model = runtime.capture_eval_suite_model()?;
        let identity = SuiteModelIdentity::new(
            suite_model.identity().provider.clone(),
            suite_model.identity().model.clone(),
        )
        .map_err(|_| anyhow::anyhow!("unsafe suite model identity"))?;
        Ok(Self::with_identity(
            Arc::new(EnginePoolPort {
                runtime,
                suite_model,
            }),
            identity,
        ))
    }

    #[cfg(feature = "benchmark-hooks")]
    pub fn has_staged_attachments(&self, session: &AgentSessionHandle) -> bool {
        self.attachment_workspaces
            .lock()
            .map(|workspaces| workspaces.contains_key(session.expose_to_backend()))
            .unwrap_or(false)
    }

    #[cfg(feature = "benchmark-hooks")]
    pub fn staged_attachment_workspace(
        &self,
        session: &AgentSessionHandle,
    ) -> Option<std::path::PathBuf> {
        self.attachment_workspaces
            .lock()
            .ok()
            .and_then(|workspaces| {
                workspaces
                    .get(session.expose_to_backend())
                    .map(|workspace| workspace.path().to_path_buf())
            })
    }

    fn take_private_session_state(
        &self,
        session_id: &str,
    ) -> Result<Option<tempfile::TempDir>, AgentBackendError> {
        let mut policies = self
            .tool_policies
            .lock()
            .map_err(|_| backend_error("private_session_state_failed"))?;
        policies.remove(session_id);
        drop(policies);

        let mut outputs = self
            .private_outputs
            .lock()
            .map_err(|_| backend_error("private_session_state_failed"))?;
        if let Some(ids) = outputs.by_session.remove(session_id) {
            for id in ids {
                outputs.values.remove(&id);
            }
        }
        drop(outputs);

        self.attachment_workspaces
            .lock()
            .map(|mut workspaces| workspaces.remove(session_id))
            .map_err(|_| backend_error("private_session_state_failed"))
    }

    fn runtime_session(
        &self,
        session_id: &str,
    ) -> Result<Option<RuntimeSession>, AgentBackendError> {
        self.runtime_sessions
            .lock()
            .map(|sessions| sessions.get(session_id).cloned())
            .map_err(|_| backend_error("session_lifecycle_failed"))
    }

    async fn close_runtime_locked(
        &self,
        session_id: &str,
        state: &mut RuntimeSessionState,
    ) -> Result<()> {
        if *state == RuntimeSessionState::Active {
            self.runtime.close(session_id).await?;
            *state = RuntimeSessionState::Closed;
        }
        Ok(())
    }

    async fn close_runtime_session(&self, session_id: &str) -> Result<()> {
        let Some(session) = self
            .runtime_session(session_id)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?
        else {
            return Ok(());
        };
        let mut state = session.lock().await;
        self.close_runtime_locked(session_id, &mut state).await?;
        drop(state);
        let mut sessions = self
            .runtime_sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("session_lifecycle_failed"))?;
        if sessions
            .get(session_id)
            .is_some_and(|current| Arc::ptr_eq(current, &session))
        {
            sessions.remove(session_id);
        }
        Ok(())
    }

    async fn fail_run_locked<T>(
        &self,
        session_id: &str,
        runtime_session: &RuntimeSession,
        mut state: tokio::sync::OwnedMutexGuard<RuntimeSessionState>,
        error: AgentBackendError,
    ) -> Result<T, AgentBackendError> {
        let _workspace = self.take_private_session_state(session_id);
        let _ = self.close_runtime_locked(session_id, &mut state).await;
        let closed = *state == RuntimeSessionState::Closed;
        drop(state);
        if closed {
            if let Ok(mut sessions) = self.runtime_sessions.lock() {
                if sessions
                    .get(session_id)
                    .is_some_and(|current| Arc::ptr_eq(current, runtime_session))
                {
                    sessions.remove(session_id);
                }
            }
        }
        Err(error)
    }

    fn take_staged_workspace(
        &self,
        session_id: &str,
    ) -> Result<Option<tempfile::TempDir>, AgentBackendError> {
        self.attachment_workspaces
            .lock()
            .map(|mut workspaces| workspaces.remove(session_id))
            .map_err(|_| backend_error("attachment_staging_failed"))
    }

    fn session_policy(&self, session_id: &str) -> Result<ProductToolPolicy, AgentBackendError> {
        self.tool_policies
            .lock()
            .map_err(|_| backend_error("unsupported_tool_policy"))?
            .get(session_id)
            .copied()
            .ok_or_else(|| backend_error("unsupported_tool_policy"))
    }

    fn store_private_output(
        &self,
        session_id: &str,
        assistant_text: String,
    ) -> Result<PrivateOutputHandle, AgentBackendError> {
        let output_id = format!("output-{:032x}", rand::random::<u128>());
        let mut outputs = self
            .private_outputs
            .lock()
            .map_err(|_| backend_error("private_output_store_failed"))?;
        outputs
            .values
            .insert(output_id.clone(), SecretText::new(assistant_text));
        outputs
            .by_session
            .entry(session_id.to_owned())
            .or_default()
            .push(output_id.clone());
        Ok(PrivateOutputHandle::new(output_id))
    }
}

const MAX_STAGED_ATTACHMENTS: usize = 16;
const MAX_STAGED_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;
const MAX_STAGED_ATTACHMENTS_TOTAL_BYTES: u64 = 100 * 1024 * 1024;

fn is_safe_attachment_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.is_ascii()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+'))
        && !matches!(name, "." | "..")
        && matches!(
            Path::new(name).components().collect::<Vec<_>>().as_slice(),
            [Component::Normal(_)]
        )
}

enum StagedAttachmentInput {
    Verified(ResolvedAttachmentSource),
    Legacy(File),
}

fn stage_attachments(
    sources: &[ResolvedAttachmentSource],
) -> Result<tempfile::TempDir, AgentBackendError> {
    if sources.len() > MAX_STAGED_ATTACHMENTS {
        return Err(backend_error("attachment_staging_failed"));
    }
    let workspace = tempfile::Builder::new()
        .prefix("pinvou-eval-attachment-")
        .tempdir()
        .map_err(|_| backend_error("attachment_staging_failed"))?;
    let mut names = HashSet::new();
    let mut validated = Vec::with_capacity(sources.len());
    let mut total_bytes = 0_u64;
    for source in sources {
        let name = source.suggested_name();
        if !is_safe_attachment_name(name) || !names.insert(name.to_owned()) {
            return Err(backend_error("attachment_staging_failed"));
        }
        let (size, input) = if source.has_verified_file() {
            let size = source
                .verified_file_size()
                .map_err(|_| backend_error("attachment_staging_failed"))?
                .ok_or_else(|| backend_error("attachment_staging_failed"))?;
            (size, StagedAttachmentInput::Verified(source.clone()))
        } else {
            let link_metadata = std::fs::symlink_metadata(source.local_path())
                .map_err(|_| backend_error("attachment_staging_failed"))?;
            if link_metadata.file_type().is_symlink() || !link_metadata.file_type().is_file() {
                return Err(backend_error("attachment_staging_failed"));
            }
            let source_file = File::open(source.local_path())
                .map_err(|_| backend_error("attachment_staging_failed"))?;
            let metadata = source_file
                .metadata()
                .map_err(|_| backend_error("attachment_staging_failed"))?;
            if !metadata.is_file()
                || !crate::platform::filesystem::reserved_target_is_unchanged(
                    &source_file,
                    source.local_path(),
                )
            {
                return Err(backend_error("attachment_staging_failed"));
            }
            (metadata.len(), StagedAttachmentInput::Legacy(source_file))
        };
        if size > MAX_STAGED_ATTACHMENT_BYTES {
            return Err(backend_error("attachment_staging_failed"));
        }
        total_bytes = total_bytes
            .checked_add(size)
            .filter(|total| *total <= MAX_STAGED_ATTACHMENTS_TOTAL_BYTES)
            .ok_or_else(|| backend_error("attachment_staging_failed"))?;
        validated.push((name.to_owned(), input));
    }
    for (name, mut input) in validated {
        let mut destination = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(workspace.path().join(name))
            .map_err(|_| backend_error("attachment_staging_failed"))?;
        let copied = match &mut input {
            StagedAttachmentInput::Verified(source) => source
                .try_read_verified_file(|file| {
                    copy(
                        &mut file.take(MAX_STAGED_ATTACHMENT_BYTES + 1),
                        &mut destination,
                    )
                })
                .map_err(|_| backend_error("attachment_staging_failed"))?
                .ok_or_else(|| backend_error("attachment_staging_failed"))?,
            StagedAttachmentInput::Legacy(source_file) => copy(
                &mut source_file.take(MAX_STAGED_ATTACHMENT_BYTES + 1),
                &mut destination,
            )
            .map_err(|_| backend_error("attachment_staging_failed"))?,
        };
        if copied > MAX_STAGED_ATTACHMENT_BYTES {
            return Err(backend_error("attachment_staging_failed"));
        }
    }
    Ok(workspace)
}

fn backend_error(code: &'static str) -> AgentBackendError {
    AgentBackendError::Operation(code.to_owned())
}

fn extract_final_answer(output: &str) -> Option<String> {
    const MARKER: &str = "FINAL ANSWER";
    let mut recognized = None;
    for line in output.lines() {
        let mut candidate = line.trim();
        let emphasis = ["**", "__"]
            .into_iter()
            .find(|token| candidate.starts_with(token));
        if let Some(token) = emphasis {
            candidate = candidate[token.len()..].trim_start();
        }
        let Some(prefix) = candidate.get(..MARKER.len()) else {
            continue;
        };
        if !prefix.eq_ignore_ascii_case(MARKER) {
            continue;
        }
        let mut remainder = candidate[MARKER.len()..].trim_start();
        if let Some(token) = emphasis.filter(|token| remainder.starts_with(token)) {
            remainder = remainder[token.len()..].trim_start();
        }
        let Some(answer) = remainder.strip_prefix(':') else {
            continue;
        };
        let mut answer = answer.trim();
        if let Some(token) = ["**", "__"]
            .into_iter()
            .find(|token| answer.starts_with(token) && answer.ends_with(token) && answer.len() >= 4)
        {
            answer = answer[token.len()..answer.len() - token.len()].trim();
        } else if let Some(token) = emphasis.filter(|token| answer.ends_with(token)) {
            answer = answer[..answer.len() - token.len()].trim_end();
        }
        recognized = Some((!answer.is_empty()).then(|| answer.to_owned()));
    }
    recognized.flatten()
}

fn project_private_output(
    task: &AgentTaskInput,
    assistant_text: String,
) -> Result<String, AgentBackendError> {
    match task.output_contract().map(|contract| contract.as_str()) {
        Some("gaia-final/v1") => extract_final_answer(&assistant_text)
            .ok_or_else(|| backend_error("missing_final_answer")),
        _ => Ok(assistant_text),
    }
}

#[cfg(test)]
mod final_answer_contract_tests {
    use super::extract_final_answer;

    #[test]
    fn gaia_contract_uses_the_last_marker_line_only() {
        let output = "FINAL ANSWER: stale\nFinal Answer: 42\nignored trailing explanation";
        assert_eq!(extract_final_answer(output), Some("42".to_owned()));
        assert_eq!(
            extract_final_answer("analysis FINAL ANSWER: wrong\nFINAL ANSWER: right"),
            Some("right".to_owned())
        );
    }

    #[test]
    fn gaia_contract_accepts_case_and_markdown_emphasis() {
        assert_eq!(
            extract_final_answer("**Final Answer: 42**"),
            Some("42".to_owned())
        );
        assert_eq!(
            extract_final_answer("__FINAL ANSWER__: Paris"),
            Some("Paris".to_owned())
        );
        assert_eq!(
            extract_final_answer("FINAL ANSWER: **yes**"),
            Some("yes".to_owned())
        );
    }

    #[test]
    fn gaia_contract_rejects_missing_or_empty_final_answer_markers() {
        assert_eq!(extract_final_answer("analysis only"), None);
        assert_eq!(extract_final_answer("FINAL ANSWER:   \n"), None);
        assert_eq!(
            extract_final_answer("FINAL ANSWER: stale\n**final answer:**   "),
            None
        );
    }
}

fn emit_finished(
    observer: &dyn AgentRunObserver,
    task_id: &str,
    status: SafeRunStatus,
    started: Instant,
) {
    let _ = notify_observer(
        observer,
        &SafeAgentEvent::RunFinished {
            task_id: task_id.to_owned(),
            status,
            elapsed: started.elapsed(),
        },
    );
}

fn session_id(task_id: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let safe = task_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .take(32)
        .collect::<String>();
    format!(
        "eval_{safe}_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[async_trait]
impl HeadlessAgentBackend for ProductHeadlessBackend {
    fn suite_model_identity(&self) -> Option<SuiteModelIdentity> {
        self.suite_model_identity.clone()
    }

    async fn prepare(
        &self,
        request: PrepareRequest,
    ) -> Result<AgentSessionHandle, AgentBackendError> {
        let policy = request
            .tool_policy()
            .ok_or_else(|| backend_error("unsupported_tool_policy"))
            .and_then(|id| {
                resolve_eval_policy(id.as_str())
                    .map(|policy| ProductToolPolicy(policy.id))
                    .map_err(|_| backend_error("unsupported_tool_policy"))
            })?;
        if request.attachments().len() != request.resolved_attachments().len() {
            return Err(backend_error("attachment_staging_failed"));
        }
        let workspace = if request.resolved_attachments().is_empty() {
            None
        } else {
            Some(stage_attachments(request.resolved_attachments())?)
        };
        let id = session_id(request.task_id());
        let mut cleanup = PrepareCleanupGuard::new(self.runtime.clone(), id.clone());
        if self.runtime.prepare(&id).await.is_err() {
            return Err(backend_error("prepare_failed"));
        }
        let runtime_session = Arc::new(AsyncMutex::new(RuntimeSessionState::Active));
        let registered = self
            .runtime_sessions
            .lock()
            .map(|mut sessions| {
                if sessions.contains_key(&id) {
                    false
                } else {
                    sessions.insert(id.clone(), runtime_session);
                    true
                }
            })
            .unwrap_or(false);
        if !registered {
            return Err(backend_error("prepare_failed"));
        }
        let policy_inserted = self
            .tool_policies
            .lock()
            .map(|mut policies| policies.insert(id.clone(), policy).is_none())
            .unwrap_or(false);
        if !policy_inserted {
            let _ = self.take_private_session_state(&id);
            if self.close_runtime_session(&id).await.is_ok() {
                cleanup.disarm();
            }
            return Err(backend_error("prepare_failed"));
        }
        if let Some(workspace) = workspace {
            let inserted = if let Ok(mut workspaces) = self.attachment_workspaces.lock() {
                workspaces.insert(id.clone(), workspace);
                true
            } else {
                false
            };
            if !inserted {
                let _ = self.take_private_session_state(&id);
                if self.close_runtime_session(&id).await.is_ok() {
                    cleanup.disarm();
                }
                return Err(backend_error("attachment_staging_failed"));
            }
        }
        cleanup.disarm();
        Ok(AgentSessionHandle::new(id))
    }

    async fn run(
        &self,
        session: &AgentSessionHandle,
        task: AgentTaskInput,
        private_inputs: Arc<dyn PrivateInputResolver>,
        observer: Arc<dyn AgentRunObserver>,
    ) -> Result<AgentTaskOutcome, AgentBackendError> {
        let session_id = session.expose_to_backend();
        let runtime_session = match self.runtime_session(session_id) {
            Ok(Some(session)) => session,
            Ok(None) => return Err(backend_error("session_closed")),
            Err(error) => return Err(error),
        };
        let runtime_state = runtime_session.clone().lock_owned().await;
        if *runtime_state == RuntimeSessionState::Closed {
            return Err(backend_error("session_closed"));
        }
        let staged_workspace = match self.take_staged_workspace(session_id) {
            Ok(workspace) => workspace,
            Err(error) => {
                return self
                    .fail_run_locked(session_id, &runtime_session, runtime_state, error)
                    .await;
            }
        };
        let policy = match self.session_policy(session_id) {
            Ok(policy) => policy,
            Err(error) => {
                return self
                    .fail_run_locked(session_id, &runtime_session, runtime_state, error)
                    .await;
            }
        };
        if let Err(code) = ensure_staged_attachments_supported(staged_workspace.is_some()) {
            return self
                .fail_run_locked(
                    session_id,
                    &runtime_session,
                    runtime_state,
                    backend_error(code),
                )
                .await;
        }
        let input = match private_inputs.resolve(task.prompt_handle()).await {
            Ok(input) => input,
            Err(error) => {
                return self
                    .fail_run_locked(session_id, &runtime_session, runtime_state, error)
                    .await;
            }
        };
        if staged_workspace.is_none() && !input.attachments().is_empty() {
            return self
                .fail_run_locked(
                    session_id,
                    &runtime_session,
                    runtime_state,
                    backend_error("attachments_runtime_unsupported"),
                )
                .await;
        }
        let started = Instant::now();
        let _ = notify_observer(
            observer.as_ref(),
            &SafeAgentEvent::run_started(task.task_id()),
        );
        let turn = match staged_workspace.as_ref() {
            Some(workspace) => {
                self.runtime
                    .run_with_staged_attachments_and_policy(
                        session.expose_to_backend(),
                        input.prompt().expose_to_backend(),
                        workspace.path(),
                        policy,
                    )
                    .await
            }
            None => {
                self.runtime
                    .run_with_policy(
                        session.expose_to_backend(),
                        input.prompt().expose_to_backend(),
                        policy,
                    )
                    .await
            }
        };
        let turn = match turn {
            Ok(turn) => turn,
            Err(error) => {
                emit_finished(
                    observer.as_ref(),
                    task.task_id(),
                    SafeRunStatus::Failed,
                    started,
                );
                for code in [
                    "attachments_runtime_unsupported",
                    "attachment_staging_failed",
                ] {
                    if error.to_string() == code {
                        return self
                            .fail_run_locked(
                                session_id,
                                &runtime_session,
                                runtime_state,
                                backend_error(code),
                            )
                            .await;
                    }
                }
                return self
                    .fail_run_locked(
                        session_id,
                        &runtime_session,
                        runtime_state,
                        backend_error("run_failed"),
                    )
                    .await;
            }
        };
        for tool in &turn.tools {
            let _ = notify_observer(
                observer.as_ref(),
                &SafeAgentEvent::tool_finished(
                    task.task_id(),
                    tool.name.clone(),
                    !tool.failed,
                    started.elapsed(),
                ),
            );
        }
        let elapsed = started.elapsed();
        let status = if turn.status.eq_ignore_ascii_case("completed") {
            SafeRunStatus::Completed
        } else {
            SafeRunStatus::Failed
        };
        emit_finished(observer.as_ref(), task.task_id(), status, started);
        if status != SafeRunStatus::Completed {
            return self
                .fail_run_locked(
                    session_id,
                    &runtime_session,
                    runtime_state,
                    backend_error("turn_not_completed"),
                )
                .await;
        }
        let private_output = match project_private_output(&task, turn.assistant_text) {
            Ok(output) => output,
            Err(error) => {
                return self
                    .fail_run_locked(session_id, &runtime_session, runtime_state, error)
                    .await;
            }
        };
        let output = match self.store_private_output(session_id, private_output) {
            Ok(output) => output,
            Err(error) => {
                return self
                    .fail_run_locked(session_id, &runtime_session, runtime_state, error)
                    .await;
            }
        };
        let mut outcome = AgentTaskOutcome::completed(elapsed).with_private_output(output);
        if let Some(usage) = turn.usage {
            outcome = outcome.with_usage(usage);
        }
        Ok(outcome)
    }

    async fn cancel(&self, session: &AgentSessionHandle) -> Result<(), AgentBackendError> {
        let session_id = session.expose_to_backend();
        let runtime_session = self
            .runtime_session(session_id)?
            .ok_or_else(|| backend_error("cancel_failed"))?;
        let _workspace = self.take_private_session_state(session_id)?;
        let cancel_result = self.runtime.cancel(session_id).await;
        let mut runtime_state = runtime_session.lock().await;
        if *runtime_state == RuntimeSessionState::Closed {
            return cancel_result.map_err(|_| backend_error("cancel_failed"));
        }
        let _workspace = self.take_private_session_state(session_id)?;
        self.close_runtime_locked(session_id, &mut runtime_state)
            .await
            .map_err(|_| backend_error("cancel_failed"))?;
        cancel_result.map_err(|_| backend_error("cancel_failed"))
    }

    async fn resolve_output(
        &self,
        handle: &PrivateOutputHandle,
    ) -> Result<SecretOutput, AgentBackendError> {
        PrivateOutputResolver::resolve(self, handle).await
    }

    async fn close(&self, session: AgentSessionHandle) -> Result<(), AgentBackendError> {
        let session_id = session.expose_to_backend().to_owned();
        let Some(runtime_session) = self.runtime_session(&session_id)? else {
            return Ok(());
        };
        let mut runtime_state = runtime_session.lock().await;
        let _workspace = self
            .take_private_session_state(&session_id)
            .map_err(|_| backend_error("close_failed"))?;
        self.close_runtime_locked(&session_id, &mut runtime_state)
            .await
            .map_err(|_| backend_error("close_failed"))?;
        drop(runtime_state);
        let mut sessions = self
            .runtime_sessions
            .lock()
            .map_err(|_| backend_error("close_failed"))?;
        if sessions
            .get(&session_id)
            .is_some_and(|current| Arc::ptr_eq(current, &runtime_session))
        {
            sessions.remove(&session_id);
        }
        Ok(())
    }
}

#[async_trait]
impl PrivateOutputResolver for ProductHeadlessBackend {
    async fn resolve(
        &self,
        handle: &PrivateOutputHandle,
    ) -> Result<SecretOutput, AgentBackendError> {
        let outputs = self
            .private_outputs
            .lock()
            .map_err(|_| backend_error("private_output_store_failed"))?;
        let value = outputs
            .values
            .get(handle.expose_to_backend())
            .cloned()
            .ok_or_else(|| backend_error("private_output_not_found"))?;
        Ok(SecretOutput::new(value))
    }
}

pub fn run_headless_host<T, Work, WorkFuture>(work: Work) -> Result<T>
where
    T: Send + 'static,
    Work: FnOnce(Arc<dyn HeadlessAgentBackend>) -> WorkFuture + Send + 'static,
    WorkFuture: Future<Output = Result<T>> + Send + 'static,
{
    crate::install_rustls_provider();
    crate::ensure_release_env();
    let async_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(16 * 1024 * 1024)
        .build()
        .context("build headless async runtime")?;
    tauri::async_runtime::set(async_runtime.handle().clone());
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    // 复用 lib.rs 的单一 generate_context 展开点:本 crate 内二次展开会在
    // macOS 触发 embed_plist 的 _EMBED_INFO_PLIST 重复符号链接错误。
    let mut context = crate::build_tauri_context();
    context.config_mut().app.windows.clear();
    let app = tauri::Builder::default()
        .setup(move |app| {
            if let Ok(resource_dir) = app.path().resource_dir() {
                crate::platform::paths::set_runtime_resource_dir(resource_dir);
            }
            let store = SessionStore::boot().context("boot headless session store")?;
            store.load_session_models();
            store.load_pinned_sessions();
            store.load_hidden_sessions();
            app.manage(store.clone());
            let pool = build_pool(app.handle().clone(), store)?;
            let backend: Arc<dyn HeadlessAgentBackend> =
                Arc::new(ProductHeadlessBackend::from_engine_pool(pool)?);
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let result = work(backend).await;
                let _ = result_tx.send(result);
                handle.exit(0);
            });
            Ok(())
        })
        .build(context)
        .context("build windowless Pinvou host")?;
    app.run_return(|_, _| {});
    result_rx
        .blocking_recv()
        .context("headless host exited before work completed")?
}

fn build_pool(app: tauri::AppHandle, store: SessionStore) -> Result<EnginePool> {
    let tool_factory: EngineToolFactory = Arc::new(|app, session_id| {
        vec![
            Arc::new(knowledge::KbSearchTool::new(
                app.clone(),
                session_id.to_owned(),
            )),
            Arc::new(knowledge::KbOpenSourceTool::new(
                app.clone(),
                session_id.to_owned(),
            )),
        ]
    });
    let tool_policy: ToolPolicy = Arc::new(|app| {
        let mut tools = crate::features::marketplace::disabled_tool_names();
        let kb_usable = app
            .try_state::<knowledge::KnowledgeService>()
            .map(|service| service.has_indexed_content() && service.semantic_ready())
            .unwrap_or(false);
        if !kb_usable {
            tools.extend(["kb_search".to_owned(), "kb_open_source".to_owned()]);
        }
        tools
    });
    EnginePool::new_with_dependencies(app, store, tool_factory, tool_policy)
}

#[cfg(test)]
mod tests {
    use super::{
        EvalToolPolicy, ProductHeadlessBackend, ProductRuntimePort, ProductToolPolicy,
        ProductTurnOutcome,
    };
    use agent_backend_api::{
        AgentRunObserver, AgentSessionHandle, AgentTaskInput, AgentToolPolicyId,
        HeadlessAgentBackend, PrepareRequest, PrivateInputHandle, PrivateInputResolver,
        ResolvedPrivateInput, SafeAgentEvent, SecretText,
    };
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::sync::Notify;

    #[derive(Default)]
    struct LegacyRuntime {
        run_calls: AtomicUsize,
    }

    #[async_trait]
    impl ProductRuntimePort for LegacyRuntime {
        async fn prepare(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn run(
            &self,
            _session_id: &str,
            _prompt: &str,
        ) -> anyhow::Result<ProductTurnOutcome> {
            self.run_calls.fetch_add(1, Ordering::Relaxed);
            anyhow::bail!("legacy_run_must_not_execute")
        }

        async fn cancel(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn close(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn policy_defaults_fail_closed_without_calling_legacy_runtime() {
        let runtime = LegacyRuntime::default();

        let error = runtime
            .run_with_policy(
                "session",
                "secret",
                ProductToolPolicy(EvalToolPolicy::GaiaOfflineV1),
            )
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "unsupported_tool_policy");
        assert_eq!(runtime.run_calls.load(Ordering::Relaxed), 0);

        let workspace = tempfile::tempdir().unwrap();
        let error = runtime
            .run_with_staged_attachments_and_policy(
                "session",
                "secret",
                workspace.path(),
                ProductToolPolicy(EvalToolPolicy::GaiaOfflineV1),
            )
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "unsupported_tool_policy");
        assert_eq!(runtime.run_calls.load(Ordering::Relaxed), 0);
    }

    #[derive(Default)]
    struct FailingPolicyRuntime {
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ProductRuntimePort for FailingPolicyRuntime {
        async fn prepare(&self, session_id: &str) -> anyhow::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("prepare:{session_id}"));
            Ok(())
        }

        async fn run(
            &self,
            _session_id: &str,
            _prompt: &str,
        ) -> anyhow::Result<ProductTurnOutcome> {
            anyhow::bail!("legacy_run_must_not_execute")
        }

        async fn run_with_policy(
            &self,
            session_id: &str,
            _prompt: &str,
            _policy: ProductToolPolicy,
        ) -> anyhow::Result<ProductTurnOutcome> {
            self.calls.lock().unwrap().push(format!("run:{session_id}"));
            anyhow::bail!("provider_failed")
        }

        async fn cancel(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn close(&self, session_id: &str) -> anyhow::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("close:{session_id}"));
            Ok(())
        }
    }

    struct Resolver;

    #[async_trait]
    impl PrivateInputResolver for Resolver {
        async fn resolve(
            &self,
            _handle: &PrivateInputHandle,
        ) -> Result<ResolvedPrivateInput, agent_backend_api::AgentBackendError> {
            Ok(ResolvedPrivateInput::new(SecretText::new("secret"), vec![]))
        }
    }

    struct Observer;

    impl AgentRunObserver for Observer {
        fn on_event(&self, _event: &SafeAgentEvent) {}
    }

    fn request(task_id: &str) -> PrepareRequest {
        PrepareRequest::new(task_id, vec![])
            .with_tool_policy(AgentToolPolicyId::new("pinvou-gaia-offline/v1").unwrap())
    }

    #[tokio::test]
    async fn prepare_policy_store_failure_closes_prepared_runtime_session() {
        let runtime = Arc::new(FailingPolicyRuntime::default());
        let backend = ProductHeadlessBackend::from_runtime(runtime.clone());
        let policies = backend.tool_policies.clone();
        let _ = std::panic::catch_unwind(move || {
            let _guard = policies.lock().unwrap();
            panic!("poison policy store");
        });

        let error = backend.prepare(request("poison")).await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "agent backend operation failed: prepare_failed"
        );
        let calls = runtime.calls.lock().unwrap();
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.starts_with("prepare:"))
                .count(),
            1
        );
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.starts_with("close:"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn failed_run_cleans_private_state_and_closes_runtime_only_once() {
        let runtime = Arc::new(FailingPolicyRuntime::default());
        let backend = ProductHeadlessBackend::from_runtime(runtime.clone());
        let session: AgentSessionHandle = backend.prepare(request("run-fail")).await.unwrap();

        let error = backend
            .run(
                &session,
                AgentTaskInput::new("run-fail", PrivateInputHandle::new("opaque")),
                Arc::new(Resolver),
                Arc::new(Observer),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "agent backend operation failed: run_failed"
        );
        assert!(
            !backend
                .tool_policies
                .lock()
                .unwrap()
                .contains_key(session.expose_to_backend())
        );
        assert!(
            !backend
                .runtime_sessions
                .lock()
                .unwrap()
                .contains_key(session.expose_to_backend())
        );
        assert_eq!(
            runtime
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| call.starts_with("close:"))
                .count(),
            1
        );

        backend.close(session).await.unwrap();
        assert_eq!(
            runtime
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| call.starts_with("close:"))
                .count(),
            1
        );
    }

    #[derive(Default)]
    struct PendingCloseRuntime {
        close_attempts: AtomicUsize,
        close_started: Notify,
        release_close: Notify,
    }

    #[async_trait]
    impl ProductRuntimePort for PendingCloseRuntime {
        async fn prepare(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn run(
            &self,
            _session_id: &str,
            _prompt: &str,
        ) -> anyhow::Result<ProductTurnOutcome> {
            anyhow::bail!("unused")
        }

        async fn cancel(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn close(&self, _session_id: &str) -> anyhow::Result<()> {
            self.close_attempts.fetch_add(1, Ordering::Relaxed);
            self.close_started.notify_one();
            self.release_close.notified().await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn aborted_pending_close_keeps_session_retryable() {
        let runtime = Arc::new(PendingCloseRuntime::default());
        let backend = ProductHeadlessBackend::from_runtime(runtime.clone());
        let session = backend.prepare(request("close-retry")).await.unwrap();

        let pending = tokio::spawn({
            let backend = backend.clone();
            let session = session.clone();
            async move { backend.close(session).await }
        });
        runtime.close_started.notified().await;
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());

        runtime.release_close.notify_one();
        backend.close(session).await.unwrap();
        assert_eq!(runtime.close_attempts.load(Ordering::Relaxed), 2);
    }

    #[derive(Default)]
    struct BlockingRunRuntime {
        run_started: Notify,
        release_run: Notify,
        cancel_called: Notify,
        close_calls: AtomicUsize,
    }

    #[derive(Default)]
    struct PendingCancelRuntime {
        cancel_called: Notify,
        cancel_release: Notify,
        close_calls: AtomicUsize,
    }

    #[async_trait]
    impl ProductRuntimePort for PendingCancelRuntime {
        async fn prepare(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn run(
            &self,
            _session_id: &str,
            _prompt: &str,
        ) -> anyhow::Result<ProductTurnOutcome> {
            anyhow::bail!("unused")
        }

        async fn cancel(&self, _session_id: &str) -> anyhow::Result<()> {
            self.cancel_called.notify_one();
            self.cancel_release.notified().await;
            Ok(())
        }

        async fn close(&self, _session_id: &str) -> anyhow::Result<()> {
            self.close_calls.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    #[tokio::test]
    async fn aborted_cancel_clears_private_state_and_keeps_close_retryable() {
        let runtime = Arc::new(PendingCancelRuntime::default());
        let backend = ProductHeadlessBackend::from_runtime(runtime.clone());
        let session = backend.prepare(request("cancel-abort")).await.unwrap();
        let cancelling = tokio::spawn({
            let backend = backend.clone();
            let session = session.clone();
            async move { backend.cancel(&session).await }
        });
        runtime.cancel_called.notified().await;
        cancelling.abort();
        assert!(cancelling.await.unwrap_err().is_cancelled());
        assert!(
            !backend
                .tool_policies
                .lock()
                .unwrap()
                .contains_key(session.expose_to_backend())
        );
        assert!(
            backend
                .runtime_sessions
                .lock()
                .unwrap()
                .contains_key(session.expose_to_backend())
        );

        backend.close(session).await.unwrap();
        assert_eq!(runtime.close_calls.load(Ordering::Relaxed), 1);
    }

    #[async_trait]
    impl ProductRuntimePort for BlockingRunRuntime {
        async fn prepare(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn run(
            &self,
            _session_id: &str,
            _prompt: &str,
        ) -> anyhow::Result<ProductTurnOutcome> {
            anyhow::bail!("legacy_run_must_not_execute")
        }

        async fn run_with_policy(
            &self,
            _session_id: &str,
            _prompt: &str,
            _policy: ProductToolPolicy,
        ) -> anyhow::Result<ProductTurnOutcome> {
            self.run_started.notify_one();
            self.release_run.notified().await;
            Ok(ProductTurnOutcome {
                status: "completed".to_owned(),
                assistant_text: "private answer".to_owned(),
                usage: None,
                tools: vec![],
            })
        }

        async fn cancel(&self, _session_id: &str) -> anyhow::Result<()> {
            self.cancel_called.notify_one();
            Ok(())
        }

        async fn close(&self, _session_id: &str) -> anyhow::Result<()> {
            self.close_calls.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    #[tokio::test]
    async fn run_and_close_are_serialized_without_resurrecting_private_output() {
        let runtime = Arc::new(BlockingRunRuntime::default());
        let backend = ProductHeadlessBackend::from_runtime(runtime.clone());
        let session = backend.prepare(request("run-close")).await.unwrap();
        let running = tokio::spawn({
            let backend = backend.clone();
            let session = session.clone();
            async move {
                backend
                    .run(
                        &session,
                        AgentTaskInput::new("run-close", PrivateInputHandle::new("opaque")),
                        Arc::new(Resolver),
                        Arc::new(Observer),
                    )
                    .await
            }
        });
        runtime.run_started.notified().await;
        let closing = tokio::spawn({
            let backend = backend.clone();
            let session = session.clone();
            async move { backend.close(session).await }
        });
        tokio::task::yield_now().await;
        assert_eq!(runtime.close_calls.load(Ordering::Relaxed), 0);

        runtime.release_run.notify_one();
        let outcome = running.await.unwrap().unwrap();
        let output = outcome.output_handle().unwrap().clone();
        closing.await.unwrap().unwrap();

        assert_eq!(runtime.close_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            backend
                .resolve_output(&output)
                .await
                .unwrap_err()
                .to_string(),
            "agent backend operation failed: private_output_not_found"
        );
    }

    #[tokio::test]
    async fn cancel_reaches_runtime_before_waiting_for_blocked_run() {
        let runtime = Arc::new(BlockingRunRuntime::default());
        let backend = ProductHeadlessBackend::from_runtime(runtime.clone());
        let session = backend.prepare(request("run-cancel")).await.unwrap();
        let running = tokio::spawn({
            let backend = backend.clone();
            let session = session.clone();
            async move {
                backend
                    .run(
                        &session,
                        AgentTaskInput::new("run-cancel", PrivateInputHandle::new("opaque")),
                        Arc::new(Resolver),
                        Arc::new(Observer),
                    )
                    .await
            }
        });
        runtime.run_started.notified().await;
        let cancelling = tokio::spawn({
            let backend = backend.clone();
            let session = session.clone();
            async move { backend.cancel(&session).await }
        });

        runtime.cancel_called.notified().await;
        assert!(!cancelling.is_finished());
        runtime.release_run.notify_one();
        running.await.unwrap().unwrap();
        cancelling.await.unwrap().unwrap();
        assert_eq!(runtime.close_calls.load(Ordering::Relaxed), 1);
    }

    #[derive(Default)]
    struct PendingPrepareRuntime {
        prepare_started: Notify,
        release_prepare: Notify,
        close_called: Notify,
    }

    #[async_trait]
    impl ProductRuntimePort for PendingPrepareRuntime {
        async fn prepare(&self, _session_id: &str) -> anyhow::Result<()> {
            self.prepare_started.notify_one();
            self.release_prepare.notified().await;
            Ok(())
        }

        async fn run(
            &self,
            _session_id: &str,
            _prompt: &str,
        ) -> anyhow::Result<ProductTurnOutcome> {
            anyhow::bail!("unused")
        }

        async fn cancel(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn close(&self, _session_id: &str) -> anyhow::Result<()> {
            self.close_called.notify_one();
            Ok(())
        }
    }

    #[tokio::test]
    async fn aborted_pending_prepare_eventually_closes_runtime_session() {
        let runtime = Arc::new(PendingPrepareRuntime::default());
        let backend = ProductHeadlessBackend::from_runtime(runtime.clone());
        let pending = tokio::spawn({
            let backend = backend.clone();
            async move { backend.prepare(request("prepare-abort")).await }
        });
        runtime.prepare_started.notified().await;
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        runtime.close_called.notified().await;
    }
}
