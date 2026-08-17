use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunEventKind {
    Planned,
    Running,
    Completed,
    Failed,
    Timeout,
}

impl RunEventKind {
    pub(crate) fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Timeout)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEvent {
    schema_version: u16,
    task_id: String,
    kind: RunEventKind,
}

impl RunEvent {
    pub fn new(task_id: impl Into<String>, kind: RunEventKind) -> Self {
        Self {
            schema_version: 1,
            task_id: task_id.into(),
            kind,
        }
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    pub fn kind(&self) -> RunEventKind {
        self.kind
    }
}
