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
    AdvancedPrefs, ColorScheme, Language, NotificationPrefs, SavedModel, SearchPrefs,
    SearchProvider, SidebarPrefs, Theme, UserPrefs,
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

/// 解析当前会话 id：优先入参，否则取 store.active_id()。
///
/// 收敛原本散落在 workflows/interaction/memory/chat 共 14 处的
/// `session_id.or_else(|| store.active_id()).ok_or_else(|| "no active session")`
/// 惯用法。行为保持不变：入参非空用入参，否则回退活跃会话，仍无则报错。
pub(super) fn require_active_sid(
    session_id: Option<String>,
    store: &SessionStore,
) -> Result<String, String> {
    session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 无入参且 store 无活跃会话 → Err("no active session")。
    /// 有入参 → Ok(入参)，忽略 store 活跃态。
    #[test]
    fn require_active_sid_resolves_explicit_then_active_then_err() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-require-active-sid-test");
        let store = SessionStore::boot().expect("boot SessionStore");

        // 1) 入参为 Some：直接返回，不看 store。
        assert_eq!(
            require_active_sid(Some("explicit-1".into()), &store),
            Ok("explicit-1".to_string())
        );

        // 2) 入参为 None 且无活跃会话 → 报错（empty-store 分支）。
        assert_eq!(
            require_active_sid(None, &store),
            Err("no active session".to_string())
        );

        // 3) 入参为 None 且 store 有活跃会话 → 回退 active_id。
        store.set_active(Some("active-1".into()));
        assert_eq!(require_active_sid(None, &store), Ok("active-1".to_string()));

        // 4) 入参非空时即便 store 有活跃会话，也只用入参。
        assert_eq!(
            require_active_sid(Some("explicit-2".into()), &store),
            Ok("explicit-2".to_string())
        );
    }
}
