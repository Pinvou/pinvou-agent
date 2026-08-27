use serde_json::{Value, json};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    io::Write,
    path::PathBuf,
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DesktopRuntimeProfile {
    pub catalog_revision: String,
    pub revision: String,
    pub selection_id: String,
    pub display_name: String,
    pub provider_id: String,
    pub provider_display_name: String,
    pub configured: bool,
    pub requires_api_key: bool,
    pub model_id: String,
    pub reasoning_level: Option<String>,
    pub payload: Value,
}

#[cfg(test)]
pub(crate) fn load_desktop_runtime_profile() -> Result<Option<DesktopRuntimeProfile>, String> {
    Ok(load_desktop_runtime_profiles()?.into_iter().next())
}

pub(crate) fn load_desktop_runtime_profiles() -> Result<Vec<DesktopRuntimeProfile>, String> {
    let path = desktop_settings_path()?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("failed to read Pinvou Desktop settings: {error}")),
    };
    let settings: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "Pinvou Desktop settings are not valid JSON".to_string())?;
    let advanced = settings
        .get("advanced")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            "Pinvou Desktop settings have no advanced model configuration".to_string()
        })?;
    let models = advanced
        .get("saved_models")
        .and_then(Value::as_array)
        .ok_or_else(|| "Pinvou Desktop settings have no saved models".to_string())?;
    let active_id = advanced.get("active_model_id").and_then(Value::as_str);
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    let settings_revision = format!("{:016x}", hasher.finish());
    let mut profiles = models
        .iter()
        .map(|model| profile_from_model(model, advanced, &settings_revision))
        .collect::<Result<Vec<_>, _>>()?;
    for template in provider_templates() {
        let already_saved = models.iter().any(|model| {
            model.get("preset").and_then(Value::as_str) == Some(template.preset)
                && model.get("model").and_then(Value::as_str) == Some(template.model)
        });
        if !already_saved {
            let shared_credential = models.iter().find_map(|model| {
                let same_provider =
                    model.get("preset").and_then(Value::as_str) == Some(template.preset);
                let state = model.get("credential_state").and_then(Value::as_str);
                let configured = model.get("has_secret").and_then(Value::as_bool) == Some(true)
                    || matches!(state, Some("configured" | "env_override"));
                (same_provider && configured)
                    .then(|| model.get("credential_ref").cloned())
                    .flatten()
            });
            profiles.push(profile_from_template(
                template,
                &settings_revision,
                shared_credential,
            ));
        }
    }
    profiles.sort_by_key(|profile| {
        (
            active_id != Some(profile.selection_id.as_str()),
            !profile.configured,
        )
    });
    Ok(profiles)
}

pub(crate) fn mark_model_credential_configured(selection_id: &str) -> Result<(), String> {
    let path = desktop_settings_path()?;
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("failed to read Pinvou Desktop settings: {error}"))?;
    let mut settings: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "Pinvou Desktop settings are not valid JSON".to_string())?;
    let models = settings
        .pointer_mut("/advanced/saved_models")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "Pinvou Desktop settings have no saved models".to_string())?;
    if !models
        .iter()
        .any(|model| model.get("id").and_then(Value::as_str) == Some(selection_id))
    {
        let template = provider_templates()
            .iter()
            .find(|template| format!("pinvou-provider-{}", template.key) == selection_id)
            .ok_or_else(|| "selected Pinvou Desktop model no longer exists".to_string())?;
        models.push(json!({
            "id":selection_id,
            "name":template.display_name,
            "preset":template.preset,
            "model":template.model,
            "base_url":template.base_url,
            "provider_kind":"official_api",
            "vendor":template.vendor,
            "reasoning_effort":"auto"
        }));
    }
    let model = models
        .iter_mut()
        .find(|model| model.get("id").and_then(Value::as_str) == Some(selection_id))
        .ok_or_else(|| "selected Pinvou Desktop model no longer exists".to_string())?;
    let object = model
        .as_object_mut()
        .ok_or_else(|| "selected Pinvou Desktop model is invalid".to_string())?;
    object.insert(
        "credential_ref".into(),
        json!({
            "service":"pinvou3-model-api-key",
            "account":format!("model:{selection_id}"),
            "version":1
        }),
    );
    object.insert(
        "credential_state".into(),
        Value::String("configured".into()),
    );
    object.insert("has_secret".into(), Value::Bool(true));
    object.remove("api_key");

    let parent = path
        .parent()
        .ok_or_else(|| "Pinvou Desktop settings path has no parent".to_string())?;
    let temporary = parent.join(format!(
        ".settings.json.pinvou-node-{}.tmp",
        std::process::id()
    ));
    let encoded = serde_json::to_vec_pretty(&settings)
        .map_err(|error| format!("failed to encode Pinvou Desktop settings: {error}"))?;
    let mut staged = std::fs::File::create(&temporary)
        .map_err(|error| format!("failed to stage Pinvou Desktop settings: {error}"))?;
    staged
        .write_all(&encoded)
        .and_then(|_| staged.sync_all())
        .map_err(|error| format!("failed to flush Pinvou Desktop settings: {error}"))?;
    let current = std::fs::read(&path)
        .map_err(|error| format!("failed to verify Pinvou Desktop settings revision: {error}"))?;
    if current != bytes {
        let _ = std::fs::remove_file(&temporary);
        return Err(
            "Pinvou Desktop settings changed while the API key was being saved; reopen /model and retry"
                .into(),
        );
    }
    std::fs::rename(&temporary, &path)
        .map_err(|error| format!("failed to commit Pinvou Desktop settings: {error}"))?;
    Ok(())
}

fn profile_from_model(
    model: &Value,
    advanced: &serde_json::Map<String, Value>,
    settings_revision: &str,
) -> Result<DesktopRuntimeProfile, String> {
    let selection_id = required_string(model, "id")?;
    let model_id = required_string(model, "model")?;
    let base_url = required_string(model, "base_url")?;
    let preset = required_string(model, "preset")?;
    let provider = desktop_provider(model, &preset, &base_url);
    let provider_display_name = provider_display_name(&provider).to_owned();
    let credential = model
        .get("credential_ref")
        .filter(|reference| reference.is_object())
        .cloned();
    let requires_auth = !matches!(preset.as_str(), "local_vllm") && !is_loopback(&base_url);
    let credential_state = model.get("credential_state").and_then(Value::as_str);
    let configured = !requires_auth
        || model.get("has_secret").and_then(Value::as_bool) == Some(true)
        || matches!(credential_state, Some("configured" | "env_override"))
        || (credential_state.is_none() && credential.is_some());
    let reasoning_level = model
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned);
    let context_window_tokens = positive_u32(model.get("context_window_tokens"))
        .or_else(|| matches!(preset.as_str(), "local_vllm").then_some(128_000));
    let max_output_tokens = positive_u32(model.get("max_output_tokens")).or_else(|| {
        matches!(preset.as_str(), "local_vllm")
            .then(|| positive_u32(advanced.get("max_output_tokens")).unwrap_or(24_576))
    });
    let revision = format!("desktop-{settings_revision}-{selection_id}");
    Ok(DesktopRuntimeProfile {
        catalog_revision: settings_revision.to_owned(),
        revision: revision.clone(),
        selection_id,
        display_name: model
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&model_id)
            .to_owned(),
        provider_id: provider.clone(),
        provider_display_name,
        configured,
        requires_api_key: requires_auth,
        model_id,
        reasoning_level: reasoning_level.clone(),
        payload: json!({
            "revision": revision,
            "provider": provider,
            "model": required_string(model, "model")?,
            "base_url": base_url,
            "auth_mode": if requires_auth { Value::String("api_key".into()) } else { Value::String("none".into()) },
            "credential": credential,
            "requires_auth": requires_auth,
            "context_window_tokens": context_window_tokens,
            "max_output_tokens": max_output_tokens,
            "reasoning_effort": reasoning_level
        }),
    })
}

fn positive_u32(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
}

fn desktop_settings_path() -> Result<PathBuf, String> {
    if let Some(root) = std::env::var_os("PINVOU3_HOME") {
        return Ok(PathBuf::from(root).join("settings.json"));
    }
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE")
    } else {
        std::env::var_os("HOME")
    }
    .ok_or_else(|| "user home is unavailable".to_string())?;
    Ok(PathBuf::from(home).join(".pinvou3").join("settings.json"))
}

fn required_string(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("Pinvou Desktop active model has no {key}"))
}

fn desktop_provider(model: &Value, preset: &str, base_url: &str) -> String {
    if base_url
        .trim_end_matches('/')
        .eq_ignore_ascii_case("https://api.deepseek.com")
    {
        return "deepseek".into();
    }
    if let Some(vendor) = model.get("vendor").and_then(Value::as_str) {
        let mapped = match vendor.trim().to_ascii_lowercase().as_str() {
            "deepseek" => Some("deepseek"),
            "kimi" | "moonshot" => Some("moonshot"),
            "glm" | "zai" | "zhipu" => Some("zai"),
            "minimax" => Some("minimax"),
            "mimo" | "xiaomi" | "xiaomi-mimo" => Some("xiaomi-mimo"),
            "doubao" | "volcengine" => Some("volcengine"),
            "anthropic" | "claude" => Some("anthropic"),
            "xai" | "grok" => Some("xai"),
            "qwen" | "tencent" | "openai" | "gemini" | "google" => Some("openai"),
            _ => None,
        };
        if let Some(mapped) = mapped {
            return mapped.into();
        }
    }
    match preset {
        "local_vllm" => "vllm",
        "deepseek" => "deepseek",
        "kimi" => "moonshot",
        "doubao" => "volcengine",
        "minimax" => "minimax",
        "glm" => "zai",
        "mimo" => "xiaomi-mimo",
        "anthropic" => "anthropic",
        "xai" => "xai",
        _ => "openai",
    }
    .into()
}

fn provider_display_name(provider: &str) -> &str {
    match provider {
        "deepseek" => "DeepSeek",
        "moonshot" => "Kimi / Moonshot",
        "zai" => "GLM / Z.ai",
        "minimax" => "MiniMax",
        "xiaomi-mimo" => "Xiaomi MiMo",
        "volcengine" => "Doubao / Volcengine",
        "anthropic" => "Anthropic",
        "xai" => "xAI",
        "vllm" => "Local vLLM",
        _ => "OpenAI Compatible",
    }
}

struct ProviderTemplate {
    key: &'static str,
    preset: &'static str,
    vendor: &'static str,
    provider_id: &'static str,
    display_name: &'static str,
    model: &'static str,
    base_url: &'static str,
}

fn provider_templates() -> &'static [ProviderTemplate] {
    &[
        ProviderTemplate {
            key: "deepseek-v4-pro",
            preset: "deepseek",
            vendor: "deepseek",
            provider_id: "deepseek",
            display_name: "DeepSeek",
            model: "deepseek-v4-pro",
            base_url: "https://api.deepseek.com",
        },
        ProviderTemplate {
            key: "deepseek-v4-flash",
            preset: "deepseek",
            vendor: "deepseek",
            provider_id: "deepseek",
            display_name: "DeepSeek",
            model: "deepseek-v4-flash",
            base_url: "https://api.deepseek.com",
        },
        ProviderTemplate {
            key: "kimi-k3",
            preset: "kimi",
            vendor: "kimi",
            provider_id: "moonshot",
            display_name: "Kimi / Moonshot",
            model: "kimi-k3",
            base_url: "https://api.moonshot.cn/v1",
        },
        ProviderTemplate {
            key: "kimi-k2-7-code",
            preset: "kimi",
            vendor: "kimi",
            provider_id: "moonshot",
            display_name: "Kimi / Moonshot",
            model: "kimi-k2.7-code",
            base_url: "https://api.moonshot.cn/v1",
        },
        ProviderTemplate {
            key: "qwen3-8-max",
            preset: "qwen",
            vendor: "qwen",
            provider_id: "openai",
            display_name: "Qwen",
            model: "qwen3.8-max",
            base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        },
        ProviderTemplate {
            key: "qwen3-7-plus",
            preset: "qwen",
            vendor: "qwen",
            provider_id: "openai",
            display_name: "Qwen",
            model: "qwen3.7-plus",
            base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        },
        ProviderTemplate {
            key: "doubao-seed-evolving",
            preset: "doubao",
            vendor: "doubao",
            provider_id: "volcengine",
            display_name: "Doubao",
            model: "doubao-seed-evolving",
            base_url: "https://ark.cn-beijing.volces.com/api/v3",
        },
        ProviderTemplate {
            key: "minimax-m3",
            preset: "minimax",
            vendor: "minimax",
            provider_id: "minimax",
            display_name: "MiniMax",
            model: "MiniMax-M3",
            base_url: "https://api.minimaxi.com/v1",
        },
        ProviderTemplate {
            key: "glm-5-2",
            preset: "glm",
            vendor: "glm",
            provider_id: "zai",
            display_name: "GLM",
            model: "glm-5.2",
            base_url: "https://open.bigmodel.cn/api/paas/v4",
        },
        ProviderTemplate {
            key: "mimo-v2-5-pro",
            preset: "mimo",
            vendor: "mimo",
            provider_id: "xiaomi-mimo",
            display_name: "Xiaomi MiMo",
            model: "mimo-v2.5-pro",
            base_url: "https://api.xiaomimimo.com/v1",
        },
        ProviderTemplate {
            key: "gpt-5-6-terra",
            preset: "openai",
            vendor: "openai",
            provider_id: "openai",
            display_name: "OpenAI",
            model: "gpt-5.6-terra",
            base_url: "https://api.openai.com/v1",
        },
        ProviderTemplate {
            key: "gpt-5-6-sol",
            preset: "openai",
            vendor: "openai",
            provider_id: "openai",
            display_name: "OpenAI",
            model: "gpt-5.6-sol",
            base_url: "https://api.openai.com/v1",
        },
        ProviderTemplate {
            key: "gpt-5-6-luna",
            preset: "openai",
            vendor: "openai",
            provider_id: "openai",
            display_name: "OpenAI",
            model: "gpt-5.6-luna",
            base_url: "https://api.openai.com/v1",
        },
        ProviderTemplate {
            key: "claude-sonnet-5",
            preset: "anthropic",
            vendor: "anthropic",
            provider_id: "anthropic",
            display_name: "Anthropic",
            model: "claude-sonnet-5",
            base_url: "https://api.anthropic.com/v1",
        },
        ProviderTemplate {
            key: "claude-fable-5",
            preset: "anthropic",
            vendor: "anthropic",
            provider_id: "anthropic",
            display_name: "Anthropic",
            model: "claude-fable-5",
            base_url: "https://api.anthropic.com/v1",
        },
        ProviderTemplate {
            key: "gemini-3-6-flash",
            preset: "gemini",
            vendor: "gemini",
            provider_id: "openai",
            display_name: "Gemini",
            model: "gemini-3.6-flash",
            base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        },
        ProviderTemplate {
            key: "grok-4-3",
            preset: "xai",
            vendor: "xai",
            provider_id: "xai",
            display_name: "xAI",
            model: "grok-4.3",
            base_url: "https://api.x.ai/v1",
        },
    ]
}

fn profile_from_template(
    template: &ProviderTemplate,
    settings_revision: &str,
    credential: Option<Value>,
) -> DesktopRuntimeProfile {
    let selection_id = format!("pinvou-provider-{}", template.key);
    let revision = format!("desktop-{settings_revision}-{selection_id}");
    let configured = credential.is_some();
    DesktopRuntimeProfile {
        catalog_revision: settings_revision.to_owned(),
        revision: revision.clone(),
        selection_id: selection_id.clone(),
        display_name: template.model.to_owned(),
        provider_id: template.provider_id.to_owned(),
        provider_display_name: template.display_name.to_owned(),
        configured,
        requires_api_key: true,
        model_id: template.model.to_owned(),
        reasoning_level: Some("auto".into()),
        payload: json!({
            "revision":revision,
            "provider":template.provider_id,
            "model":template.model,
            "base_url":template.base_url,
            "auth_mode":"api_key",
            "credential":credential.unwrap_or_else(|| json!({
                "service":"pinvou3-model-api-key",
                "account":format!("model:{selection_id}"),
                "version":1
            })),
            "requires_auth":true,
            "context_window_tokens":Value::Null,
            "max_output_tokens":Value::Null,
            "reasoning_effort":"auto"
        }),
    }
}

fn is_loopback(base_url: &str) -> bool {
    let lower = base_url.to_ascii_lowercase();
    lower.starts_with("http://127.0.0.1")
        || lower.starts_with("http://localhost")
        || lower.starts_with("http://[::1]")
        || lower.starts_with("https://127.0.0.1")
        || lower.starts_with("https://localhost")
        || lower.starts_with("https://[::1]")
}

#[cfg(test)]
mod tests {
    use super::{load_desktop_runtime_profile, load_desktop_runtime_profiles};
    use serde_json::Value;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn loads_active_custom_provider_without_reading_plaintext_credentials() {
        let _guard = env_lock().lock().unwrap();
        let root =
            std::env::temp_dir().join(format!("pinvou-desktop-profile-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("settings.json"),
            r#"{"advanced":{"active_model_id":"custom","saved_models":[{"id":"custom","preset":"openai_compatible","model":"model-x","base_url":"https://example.invalid/v1","reasoning_effort":"high","credential_ref":{"service":"pinvou3-model-api-key","account":"model:custom","version":1},"api_key":"must-not-be-used"}]}}"#,
        )
        .unwrap();
        let previous = std::env::var_os("PINVOU3_HOME");
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };
        let profile = load_desktop_runtime_profile().unwrap().unwrap();
        match previous {
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        assert_eq!(profile.model_id, "model-x");
        assert_eq!(profile.reasoning_level.as_deref(), Some("high"));
        assert_eq!(profile.payload["reasoning_effort"], "high");
        assert_eq!(profile.payload["context_window_tokens"], Value::Null);
        assert_eq!(profile.payload["max_output_tokens"], Value::Null);
        assert_eq!(profile.payload["provider"], "openai");
        assert_eq!(profile.payload["credential"]["account"], "model:custom");
        assert!(!profile.payload.to_string().contains("must-not-be-used"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lists_active_then_configured_models_and_preserves_provider_groups() {
        let _guard = env_lock().lock().unwrap();
        let root = std::env::temp_dir().join(format!(
            "pinvou-desktop-profile-catalog-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("settings.json"),
            r#"{"advanced":{"active_model_id":"missing-key","saved_models":[{"id":"configured-fast","name":"DeepSeek Fast","preset":"deepseek","model":"deepseek-v4-flash","base_url":"https://api.deepseek.com","credential_ref":{"service":"pinvou3-model-api-key","account":"model:configured-fast","version":1},"credential_state":"configured","has_secret":true},{"id":"missing-key","name":"DeepSeek Pro","preset":"deepseek","model":"deepseek-v4-pro","base_url":"https://api.deepseek.com","credential_state":"missing","has_secret":false},{"id":"configured-kimi","name":"Kimi Code","preset":"kimi","model":"kimi-k2.7-code","base_url":"https://api.moonshot.cn/v1","credential_ref":{"service":"pinvou3-model-api-key","account":"model:configured-kimi","version":1},"credential_state":"configured","has_secret":true}]}}"#,
        )
        .unwrap();
        let previous = std::env::var_os("PINVOU3_HOME");
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };
        let profiles = load_desktop_runtime_profiles().unwrap();
        match previous {
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        assert_eq!(profiles[0].selection_id, "missing-key");
        assert!(
            !profiles[0].configured,
            "active remains first even without a key"
        );
        assert_eq!(profiles[1].selection_id, "configured-fast");
        assert_eq!(profiles[2].selection_id, "configured-kimi");
        assert_eq!(profiles[0].provider_id, profiles[1].provider_id);
        assert_ne!(profiles[1].model_id, profiles[0].model_id);
        assert_ne!(profiles[1].provider_id, profiles[2].provider_id);
        let kimi_sibling = profiles
            .iter()
            .find(|profile| profile.model_id == "kimi-k3")
            .expect("provider catalog keeps sibling models");
        assert!(
            kimi_sibling.configured,
            "one Provider key unlocks sibling models"
        );
        assert_eq!(
            kimi_sibling.payload["credential"]["account"],
            "model:configured-kimi"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
