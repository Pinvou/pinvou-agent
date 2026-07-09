pub mod adapter;
pub mod identity;
pub mod models;
pub mod provisioning;
pub mod store;
pub mod usage;

pub const DEFAULT_ADMIN_BASE_URL: &str = "https://www.ma-xiao.com/llmapi";
pub const DEFAULT_CHAT_BASE_URL: &str = "https://www.ma-xiao.com/llmapi/v1";
pub const DEFAULT_MODEL: &str = "deepseek-v4-flash";
pub const ENABLE_CHAT_ENV: &str = "PINVOU3_USE_LLMAPI_HUB";

pub fn enabled_for_chat() -> bool {
    std::env::var(ENABLE_CHAT_ENV)
        .map(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "on"))
        .unwrap_or(false)
}

pub fn select_model(available_models: &[String], configured_default: Option<&str>) -> String {
    let configured = configured_default.map(str::trim).filter(|value| !value.is_empty());
    if let Some(configured) = configured {
        if available_models.iter().any(|model| model == configured) {
            return configured.to_string();
        }
    }
    if available_models.iter().any(|model| model == DEFAULT_MODEL) {
        return DEFAULT_MODEL.to_string();
    }
    available_models
        .first()
        .cloned()
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}
