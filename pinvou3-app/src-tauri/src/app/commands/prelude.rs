pub(super) use deepseek_tui::models::Message;
pub(super) use deepseek_tui::session_manager::{SavedSession, SessionMetadata};
pub(super) use deepseek_tui::tools::user_input::{UserInputAnswer, UserInputResponse};
pub(super) use serde::{Deserialize, Serialize};
pub(super) use tauri::{AppHandle, Emitter, Manager, State};

pub(super) use crate::core::mode_state::{SerializableMode, SessionModeState};
pub(super) use crate::features::assistant::engine_pool::EnginePool;
pub(super) use crate::features::knowledge::KnowledgeService;
pub(super) use crate::features::monitor::{MonitorSnapshot, MonitorState, VllmStatus};
pub(super) use crate::features::sessions::{SessionKind, SessionStore};
pub(super) use crate::platform::credential_store::{
    CredentialEditAction, CredentialState, CredentialStore, SystemCredentialStore,
};
pub(super) use crate::platform::prefs::{
    AdvancedPrefs, ColorScheme, ImageCapabilityOverride, Language, NotificationPrefs, SavedModel,
    SearchPrefs, SearchProvider, SidebarPrefs, Theme, UserPrefs,
};

/// Keep the Tauri transport boundary in `app::commands` while domain modules
/// retain the implementation. The generated function deliberately keeps the
/// original name and signature, so the frontend protocol does not change.
macro_rules! async_command_passthrough {
    ($domain:ident, $name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty) => {
        #[tauri::command]
        pub async fn $name($($arg: $ty),*) -> $ret {
            $domain::$name($($arg),*).await
        }
    };
}

macro_rules! sync_command_passthrough {
    ($domain:ident, $name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty) => {
        #[tauri::command]
        pub fn $name($($arg: $ty),*) -> $ret {
            $domain::$name($($arg),*)
        }
    };
    ($domain:ident, $name:ident($($arg:ident: $ty:ty),* $(,)?)) => {
        #[tauri::command]
        pub fn $name($($arg: $ty),*) {
            $domain::$name($($arg),*)
        }
    };
}

pub(super) use async_command_passthrough;
pub(super) use sync_command_passthrough;
