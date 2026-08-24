use std::path::PathBuf;

use crate::backend::RuntimeStatus;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TurnState {
    Idle,
    Starting,
    Streaming { turn_id: Option<String> },
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
pub enum Interaction {
    None,
    Approval {
        approval_id: String,
        tool: String,
        summary: String,
        options: Vec<String>,
    },
    Input {
        input_id: String,
        prompt: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Overlay {
    None,
    Help { commands: Vec<&'static str> },
    RuntimeList,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Model {
    pub workspace: PathBuf,
    pub runtime: RuntimeStatus,
    pub connection: ConnectionState,
    pub turn: TurnState,
    pub transcript: Transcript,
    pub composer: Composer,
    pub interaction: Interaction,
    pub overlay: Overlay,
    pub status_message: Option<String>,
    pub should_quit: bool,
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
            interaction: Interaction::None,
            overlay: Overlay::None,
            status_message: None,
            should_quit: false,
        }
    }
}
