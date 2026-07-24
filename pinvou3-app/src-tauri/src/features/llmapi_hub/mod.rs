pub mod adapter;
pub mod identity;
pub mod models;
mod platform;
pub mod provisioning;
pub mod store;
pub mod usage;

pub const DEFAULT_ADMIN_BASE_URL: &str = "";
pub const DEFAULT_CHAT_BASE_URL: &str =
    crate::platform::prefs::BUILTIN_LLMAPI_DEFAULT_CHAT_BASE_URL;
pub const DEFAULT_MODEL: &str = crate::platform::prefs::BUILTIN_LLMAPI_DEFAULT_MODEL;
pub const ENABLE_CHAT_ENV: &str = "PINVOU3_USE_LLMAPI_HUB";

pub fn enabled_for_chat() -> bool {
    std::env::var(ENABLE_CHAT_ENV)
        .map(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "on"))
        .unwrap_or(false)
}

pub fn select_model(available_models: &[String], configured_default: Option<&str>) -> String {
    crate::platform::prefs::select_builtin_llmapi_model(available_models, configured_default)
}
