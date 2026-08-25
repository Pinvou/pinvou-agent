use std::path::PathBuf;

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
    Assistant(String),
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
    pub permission_profile: PermissionMode,
    pub permission_status: Option<PermissionStatus>,
    pub session_candidates: Vec<SessionCandidate>,
    pub selected_session: usize,
    pub session_query: String,
    pub pending_session_list: Option<OperationToken>,
    pub pending_resume: Option<PendingSelection>,
    pub model_candidates: Vec<ModelCandidate>,
    pub selected_model: usize,
    pub pending_model_list: Option<OperationToken>,
    pub pending_model_switch: Option<PendingSelection>,
    pub selected_permission: usize,
    pub pending_permissions: Option<OperationToken>,
    pub pending_permission_switch: Option<PendingPermissionSwitch>,
    pub transcript_scroll: u16,
    pub terminal_size: Option<(u16, u16)>,
    pub last_terminal_turn_token: Option<OperationToken>,
    pub should_quit: bool,
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
            permission_profile: PermissionMode::Request,
            permission_status: None,
            session_candidates: Vec::new(),
            selected_session: 0,
            session_query: String::new(),
            pending_session_list: None,
            pending_resume: None,
            model_candidates: Vec::new(),
            selected_model: 0,
            pending_model_list: None,
            pending_model_switch: None,
            selected_permission: 0,
            pending_permissions: None,
            pending_permission_switch: None,
            transcript_scroll: 0,
            terminal_size: None,
            last_terminal_turn_token: None,
            should_quit: false,
            next_operation_token: 1,
        }
    }

    pub(crate) fn allocate_operation_token(&mut self) -> OperationToken {
        let token = OperationToken::new(self.next_operation_token);
        self.next_operation_token = self.next_operation_token.wrapping_add(1).max(1);
        token
    }
}
