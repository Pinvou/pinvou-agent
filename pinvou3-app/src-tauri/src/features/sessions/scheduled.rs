//! Scheduled-run session types and profiles.
//!
//! Scheduled execution is captured by an immutable profile persisted alongside
//! the ordinary Session JSON. These types mirror the durable fields available
//! from `Event::SessionUpdated` plus a final `SessionSnapshot`, while identity,
//! title, creation time, artifacts, and the profile remain owned by the saved
//! session / store.

use std::collections::HashMap;
use std::path::PathBuf;

use deepseek_tui::models::SystemPrompt;
use deepseek_tui::tui::app::AppMode;
use serde::{Deserialize, Serialize};

pub(crate) const SCHEDULED_PROFILE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledRunMode {
    Agent,
    Plan,
    Yolo,
}

impl ScheduledRunMode {
    pub(crate) const fn for_scheduled_auto_approve(auto_approve: bool) -> Self {
        if auto_approve {
            Self::Yolo
        } else {
            Self::Agent
        }
    }

    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Plan => "plan",
            Self::Yolo => "yolo",
        }
    }

    pub const fn to_app_mode(self) -> AppMode {
        match self {
            Self::Agent => AppMode::Agent,
            Self::Plan => AppMode::Plan,
            Self::Yolo => AppMode::Yolo,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledRunProfile {
    pub task_id: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    pub workspace: PathBuf,
    pub mode: ScheduledRunMode,
    pub allow_shell: bool,
    pub trust_mode: bool,
    pub auto_approve: bool,
}

impl ScheduledRunProfile {
    /// Scheduled execution has no interactive mode selector. Approval policy is
    /// the authority: runs that may auto-approve use Yolo, while every other run
    /// stays in Agent so the engine cannot bypass the persisted approval gate.
    pub(crate) const fn execution_mode(&self) -> ScheduledRunMode {
        ScheduledRunMode::for_scheduled_auto_approve(self.auto_approve)
    }
}

/// How an engine-owned scheduled session update changes the durable token total.
///
/// `SessionUpdated` does not carry usage, so callers must preserve the last durable
/// total. A final engine snapshot reports usage accumulated since that engine was
/// spawned; combining it with the spawn-time base produces an absolute lifetime
/// total without double-counting later turns from the same engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduledTokenAccounting {
    PreservePersisted,
    EngineCumulative {
        base_total_tokens: u64,
        engine_total_tokens: u64,
    },
}

/// Authoritative engine state to persist for one scheduled-run session.
///
/// This mirrors the durable fields available from `Event::SessionUpdated` plus a
/// final `SessionSnapshot`. Identity, title, creation time, artifacts, and the
/// immutable scheduled profile remain owned by the existing saved session/store.
#[derive(Debug, Clone)]
pub struct ScheduledEngineState {
    pub messages: Vec<deepseek_tui::models::Message>,
    pub system_prompt: Option<SystemPrompt>,
    pub model: String,
    pub workspace: PathBuf,
    pub mode: ScheduledRunMode,
    pub token_accounting: ScheduledTokenAccounting,
}

/// Authoritative engine snapshot for an ordinary chat session. The event
/// forwarder sanitizes engine-only user prompt injections before constructing
/// this value; persistence therefore never depends on a WebView staying alive.
#[derive(Debug, Clone)]
pub struct ChatEngineState {
    pub messages: Vec<deepseek_tui::models::Message>,
    pub system_prompt: Option<SystemPrompt>,
    pub model: String,
    pub workspace: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ScheduledProfileRegistry {
    pub(crate) schema_version: u32,
    #[serde(default)]
    pub(crate) sessions: HashMap<String, ScheduledRunProfile>,
}

impl Default for ScheduledProfileRegistry {
    fn default() -> Self {
        Self {
            schema_version: SCHEDULED_PROFILE_SCHEMA_VERSION,
            sessions: HashMap::new(),
        }
    }
}
