use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use crate::backend::{
    BackendError, ModelCandidate, PermissionMode, PermissionStatus, RuntimeStatus, SessionCandidate,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OperationToken(u64);

impl OperationToken {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingRuntimeSwitch {
    pub target: String,
    pub operation_token: OperationToken,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingSelection {
    pub target: String,
    pub operation_token: OperationToken,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingPermissionSwitch {
    pub target: PermissionMode,
    pub operation_token: OperationToken,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingInterrupt {
    pub turn_id: String,
    pub operation_token: OperationToken,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Failed(BackendError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TurnState {
    Idle,
    Starting {
        operation_token: OperationToken,
    },
    Streaming {
        operation_token: OperationToken,
        turn_id: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolState {
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TranscriptEntry {
    User(String),
    Thinking(String),
    Assistant(String),
    Error(String),
    Tool {
        tool_id: String,
        name: String,
        output: String,
        state: ToolState,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Transcript {
    entries: Vec<TranscriptEntry>,
}

impl Transcript {
    pub fn entries(&self) -> &[TranscriptEntry] {
        &self.entries
    }

    pub fn assistant_text(&self) -> String {
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                TranscriptEntry::Assistant(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn push_user(&mut self, prompt: String) {
        self.entries.push(TranscriptEntry::User(prompt));
    }

    pub(crate) fn append_assistant(&mut self, content: &str) {
        match self.entries.last_mut() {
            Some(TranscriptEntry::Assistant(text)) => text.push_str(content),
            _ => self
                .entries
                .push(TranscriptEntry::Assistant(content.to_owned())),
        }
    }

    pub(crate) fn push_assistant(&mut self, content: impl Into<String>) {
        self.entries
            .push(TranscriptEntry::Assistant(content.into()));
    }

    pub(crate) fn append_thinking(&mut self, content: &str) {
        match self.entries.last_mut() {
            Some(TranscriptEntry::Thinking(text)) => text.push_str(content),
            _ => self
                .entries
                .push(TranscriptEntry::Thinking(content.to_owned())),
        }
    }

    pub(crate) fn push_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        if message.is_empty()
            || matches!(self.entries.last(), Some(TranscriptEntry::Error(current)) if current == &message)
        {
            return;
        }
        self.entries.push(TranscriptEntry::Error(message));
    }

    pub(crate) fn start_tool(&mut self, tool_id: String, name: String) {
        self.entries.push(TranscriptEntry::Tool {
            tool_id,
            name,
            output: String::new(),
            state: ToolState::Running,
        });
    }

    pub(crate) fn append_tool_output(&mut self, tool_id: &str, chunk: &str) {
        if let Some(TranscriptEntry::Tool { output, .. }) = self.entries.iter_mut().rev().find(
            |entry| matches!(entry, TranscriptEntry::Tool { tool_id: id, .. } if id == tool_id),
        ) {
            output.push_str(chunk);
        }
    }

    pub(crate) fn complete_tool(&mut self, tool_id: &str, failed: bool) {
        if let Some(TranscriptEntry::Tool { state, .. }) = self.entries.iter_mut().rev().find(
            |entry| matches!(entry, TranscriptEntry::Tool { tool_id: id, .. } if id == tool_id),
        ) {
            *state = if failed {
                ToolState::Failed
            } else {
                ToolState::Completed
            };
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Composer {
    pub input: String,
}

#[derive(Clone, Default, Eq, PartialEq)]
pub struct SecretComposer {
    value: String,
}

impl std::fmt::Debug for SecretComposer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretComposer")
            .field("value", &"[REDACTED]")
            .finish()
    }
}

impl SecretComposer {
    pub(crate) fn push(&mut self, character: char) {
        self.value.push(character);
    }

    pub(crate) fn push_str(&mut self, value: &str) {
        self.value.push_str(value);
    }

    pub(crate) fn pop(&mut self) {
        self.value.pop();
    }

    pub(crate) fn take(&mut self) -> String {
        std::mem::take(&mut self.value)
    }

    pub(crate) fn clear(&mut self) {
        self.value.clear();
    }

    pub fn len(&self) -> usize {
        self.value.chars().count()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalRequest {
    pub turn_id: String,
    pub approval_id: String,
    pub operation_token: OperationToken,
    pub tool: String,
    pub summary: String,
    pub options: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputRequest {
    pub turn_id: String,
    pub input_id: String,
    pub operation_token: OperationToken,
    pub prompt: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Interaction {
    None,
    ApprovalPending(ApprovalRequest),
    ApprovalResolving {
        request: ApprovalRequest,
        decision: crate::action::ApprovalDecision,
    },
    InputPending(InputRequest),
    InputResolving {
        request: InputRequest,
        value: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Overlay {
    None,
    Help { commands: Vec<&'static str> },
    RuntimeList,
    ResumeList,
    ModelList,
    ModelLevelList,
    ApiKeyInput,
    PermissionList,
    FullAccessConfirmation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Model {
    pub workspace: PathBuf,
    pub runtime: RuntimeStatus,
    pub connection: ConnectionState,
    pub turn: TurnState,
    pub transcript: Transcript,
    pub composer: Composer,
    pub credential_composer: SecretComposer,
    pub selected_command: usize,
    pub input_composer: Composer,
    pub interaction: Interaction,
    pub overlay: Overlay,
    pub status_message: Option<String>,
    pub diagnostic_message: Option<String>,
    pub last_backend_error: Option<BackendError>,
    pub pending_runtime_switch: Option<PendingRuntimeSwitch>,
    pub pending_interrupt: Option<PendingInterrupt>,
    pub pending_runtime_list: Option<OperationToken>,
    pub runtime_candidates: Vec<RuntimeStatus>,
    pub selected_runtime: usize,
    pub active_session: Option<String>,
    pub session_cursor: u64,
    pub model_id: Option<String>,
    pub model_level: Option<String>,
    pub permission_profile: PermissionMode,
    pub permission_status: Option<PermissionStatus>,
    pub session_candidates: Vec<SessionCandidate>,
    pub selected_session: usize,
    pub session_query: String,
    pub pending_session_list: Option<OperationToken>,
    pub pending_resume: Option<PendingSelection>,
    pub model_candidates: Vec<ModelCandidate>,
    pub selected_model: usize,
    pub selected_model_level: usize,
    pub pending_model_list: Option<OperationToken>,
    pub model_list_background: bool,
    pub pending_model_switch: Option<PendingSelection>,
    pub pending_model_credential: Option<PendingSelection>,
    pub pending_model_level: Option<String>,
    pub selected_permission: usize,
    pub pending_permissions: Option<OperationToken>,
    pub pending_permission_switch: Option<PendingPermissionSwitch>,
    pub transcript_scroll: u16,
    pub terminal_size: Option<(u16, u16)>,
    pub last_terminal_turn_token: Option<OperationToken>,
    pub should_quit: bool,
    activity_started_at: Option<Instant>,
    activity_elapsed: Duration,
    activity_frame: u8,
    next_operation_token: u64,
}

impl Model {
    pub fn new(workspace: PathBuf, runtime: RuntimeStatus) -> Self {
        Self {
            workspace,
            runtime,
            connection: ConnectionState::Disconnected,
            turn: TurnState::Idle,
            transcript: Transcript::default(),
            composer: Composer::default(),
            credential_composer: SecretComposer::default(),
            selected_command: 0,
            input_composer: Composer::default(),
            interaction: Interaction::None,
            overlay: Overlay::None,
            status_message: None,
            diagnostic_message: None,
            last_backend_error: None,
            pending_runtime_switch: None,
            pending_interrupt: None,
            pending_runtime_list: None,
            runtime_candidates: Vec::new(),
            selected_runtime: 0,
            active_session: None,
            session_cursor: 0,
            model_id: None,
            model_level: None,
            permission_profile: PermissionMode::Request,
            permission_status: None,
            session_candidates: Vec::new(),
            selected_session: 0,
            session_query: String::new(),
            pending_session_list: None,
            pending_resume: None,
            model_candidates: Vec::new(),
            selected_model: 0,
            selected_model_level: 0,
            pending_model_list: None,
            model_list_background: false,
            pending_model_switch: None,
            pending_model_credential: None,
            pending_model_level: None,
            selected_permission: 0,
            pending_permissions: None,
            pending_permission_switch: None,
            transcript_scroll: 0,
            terminal_size: None,
            last_terminal_turn_token: None,
            should_quit: false,
            activity_started_at: None,
            activity_elapsed: Duration::ZERO,
            activity_frame: 0,
            next_operation_token: 1,
        }
    }

    pub(crate) fn reset_activity(&mut self) {
        self.activity_started_at = None;
        self.activity_elapsed = Duration::ZERO;
        self.activity_frame = 0;
    }

    pub(crate) fn advance_activity(&mut self, now: Instant) {
        if self.turn == TurnState::Idle {
            self.reset_activity();
            return;
        }
        let started_at = *self.activity_started_at.get_or_insert(now);
        self.activity_elapsed = now.saturating_duration_since(started_at);
        self.activity_frame = self.activity_frame.wrapping_add(1);
    }

    pub(crate) fn activity_elapsed(&self) -> Duration {
        self.activity_elapsed
    }

    pub(crate) fn activity_frame(&self) -> u8 {
        self.activity_frame
    }

    pub(crate) fn allocate_operation_token(&mut self) -> OperationToken {
        let token = OperationToken::new(self.next_operation_token);
        self.next_operation_token = self.next_operation_token.wrapping_add(1).max(1);
        token
    }
}
