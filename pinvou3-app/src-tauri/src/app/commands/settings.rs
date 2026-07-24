/// 从 disk 读最新 UserPrefs。
/// 注意走 disk 而非 engine.bridge.prefs——如果用户手改 settings.json，
/// `get_settings()` 能立刻拿到，不需要 reload bridge。
#[tauri::command]
pub async fn get_settings() -> Result<UserPrefs, String> {
    Ok(refresh_safe_prefs(UserPrefs::load()))
}

fn sanitize_command_error(context: &str, err: impl std::fmt::Display) -> String {
    format!(
        "{context}: {}",
        crate::platform::credential_store::redact_secret(&err.to_string())
    )
}

fn prepare_prefs_for_save(mut prefs: UserPrefs) -> Result<UserPrefs, String> {
    let store = SystemCredentialStore::new();
    let migration = prefs.migrate_plaintext_api_keys_with_store(&store);
    if !migration.failed_model_ids.is_empty() || !migration.failed_search_providers.is_empty() {
        return Err("credential store unavailable; please reconfigure API Key".to_string());
    }
    prefs.sanitize_plaintext_api_keys();
    prefs.refresh_credential_states_with_store(&store);
    Ok(prefs)
}

fn refresh_safe_prefs(mut prefs: UserPrefs) -> UserPrefs {
    prefs.refresh_credential_states_with_store(&SystemCredentialStore::new());
    prefs.sanitize_plaintext_api_keys();
    prefs
}

fn apply_model_credential(
    mut model: SavedModel,
    old: Option<&SavedModel>,
) -> Result<SavedModel, String> {
    let store = SystemCredentialStore::new();
    let action = model.credential_action.unwrap_or_else(|| {
        if model.api_key.trim().is_empty() {
            CredentialEditAction::KeepExisting
        } else {
            CredentialEditAction::Replace
        }
    });

    match action {
        CredentialEditAction::KeepExisting => {
            if let Some(old) = old {
                model.credential_ref = old.credential_ref.clone();
                model.credential_state = old.credential_state;
                model.has_secret = old.has_secret;
            } else if model.api_key.trim().is_empty() {
                model.mark_missing();
            } else {
                let reference = model.credential_reference();
                store
                    .set(&reference, model.api_key.trim())
                    .map_err(|e| e.user_message())?;
                model.mark_configured(reference);
            }
        }
        CredentialEditAction::Replace => {
            let key = model.api_key.trim().to_string();
            if key.is_empty() {
                model.mark_missing();
            } else {
                let reference = model.credential_reference();
                store.set(&reference, &key).map_err(|e| e.user_message())?;
                model.mark_configured(reference);
            }
        }
        CredentialEditAction::Delete => {
            let reference = model
                .credential_ref
                .clone()
                .or_else(|| old.and_then(|m| m.credential_ref.clone()))
                .unwrap_or_else(|| model.credential_reference());
            store.delete(&reference).map_err(|e| e.user_message())?;
            model.mark_missing();
        }
    }
    model.clear_plaintext_key();
    Ok(model)
}

fn resolve_saved_model_key(model_id: Option<&str>) -> Result<Option<String>, String> {
    let prefs = UserPrefs::load();
    let model = model_id
        .and_then(|id| prefs.model_by_id(id))
        .or_else(|| prefs.active_model());
    let Some(model) = model else {
        return Ok(None);
    };
    let Some(reference) = &model.credential_ref else {
        return Ok(None);
    };
    SystemCredentialStore::new()
        .get(reference)
        .map_err(|e| e.user_message())
}

#[tauri::command]
pub async fn submit_feedback(
    request: crate::features::feedback::FeedbackSubmitRequest,
) -> Result<crate::features::feedback::FeedbackReceipt, String> {
    crate::features::feedback::submit_feedback(request)
        .await
        .map_err(|e| e.to_string())
}

/// 实际生效的模型配置（环境变量可能覆盖 settings.json）。
/// 前端设置页初始化时优先用这个，避免"改了 settings 但实际不生效"的困惑。
#[derive(Debug, Clone, Serialize)]
pub struct EffectiveModelConfig {
    pub preset: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub credential_state: CredentialState,
    pub has_secret: bool,
    pub provider: String,
    /// 被环境变量覆盖的字段名列表（如 `["model", "base_url"]`）。
    /// 空列表表示全部走 settings.json，用户修改会生效。
    pub env_overrides: Vec<String>,
}

fn session_model_from_prefs(
    prefs: &UserPrefs,
    session_model_id: Option<&str>,
) -> Option<SavedModel> {
    session_model_id.and_then(|id| prefs.model_by_id(id).cloned())
}

#[tauri::command]
pub async fn get_effective_model_config(
    session_id: Option<String>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<EffectiveModelConfig, String> {
    // 读 disk 最新 prefs，并按当前会话解析真正绑定的模型。
    let mut bridge = pool.bridge.clone();
    bridge.prefs = refresh_safe_prefs(UserPrefs::load());
    let session_model_id = session_id
        .as_deref()
        .and_then(|id| store.session_model_id(id));
    bridge.session_model = session_model_from_prefs(&bridge.prefs, session_model_id.as_deref());
    let mut env_overrides = Vec::new();
    if std::env::var("DEEPSEEK_MODEL").is_ok() {
        env_overrides.push("model".to_string());
    }
    if std::env::var("DEEPSEEK_BASE_URL").is_ok() {
        env_overrides.push("base_url".to_string());
    }
    let env_api_key = std::env::var("DEEPSEEK_API_KEY")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    if env_api_key {
        env_overrides.push("api_key".to_string());
    }
    if std::env::var("DEEPSEEK_PROVIDER").is_ok_and(|provider| provider == bridge.provider()) {
        env_overrides.push("provider".to_string());
    }
    let effective = bridge.effective_model_owned();
    let preset = effective
        .as_ref()
        .map(|model| model.preset)
        .unwrap_or_default()
        .as_str();
    Ok(EffectiveModelConfig {
        preset: preset.to_string(),
        model: bridge.model(),
        base_url: bridge.base_url(),
        api_key: String::new(),
        credential_state: if env_api_key {
            CredentialState::EnvOverride
        } else {
            effective
                .as_ref()
                .map(|model| model.credential_state)
                .unwrap_or(CredentialState::Missing)
        },
        has_secret: effective
            .as_ref()
            .map(|model| model.has_secret)
            .unwrap_or(false),
        provider: bridge.provider(),
        env_overrides,
    })
}

/// 「添加模型」方案:列出已保存模型 + 当前全局默认 id(前端高亮)。
#[derive(Debug, Clone, Serialize)]
pub struct ModelListItem {
    #[serde(flatten)]
    pub model: SavedModel,
    pub readonly: bool,
    pub system: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl From<SavedModel> for ModelListItem {
    fn from(model: SavedModel) -> Self {
        Self {
            model,
            readonly: false,
            system: false,
            kind: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelsView {
    pub models: Vec<ModelListItem>,
    pub active_model_id: Option<String>,
}

#[tauri::command]
pub async fn list_models() -> Result<ModelsView, String> {
    let prefs = refresh_safe_prefs(UserPrefs::load());
    Ok(ModelsView {
        models: prefs
            .advanced
            .saved_models
            .clone()
            .into_iter()
            .map(ModelListItem::from)
            .collect(),
        active_model_id: prefs.advanced.active_model_id.clone(),
    })
}

/// 用户在编辑模型弹窗里主动点击“显示”时，读取该模型已保存的 API Key。
/// 环境变量覆盖的凭据不回显，避免给出一个前端并不拥有、保存也不会覆盖的值。
#[tauri::command]
pub async fn reveal_model_api_key(id: String) -> Result<Option<String>, String> {
    let prefs = refresh_safe_prefs(UserPrefs::load());
    let model = prefs
        .model_by_id(&id)
        .ok_or_else(|| format!("model not found: {id}"))?;
    if model.credential_state == CredentialState::EnvOverride {
        return Ok(None);
    }
    let Some(reference) = &model.credential_ref else {
        return Ok(None);
    };
    SystemCredentialStore::new()
        .get(reference)
        .map_err(|e| sanitize_command_error("reveal_model_api_key", e.user_message()))
}

/// 增或改一条模型(按 id)。前端负责生成稳定 id。
#[tauri::command]
pub async fn save_model(model: SavedModel) -> Result<(), String> {
    let mut prefs = UserPrefs::load();
    let old = prefs.model_by_id(&model.id).cloned();
    let model = apply_model_credential(model, old.as_ref())
        .map_err(|e| sanitize_command_error("save_model", e))?;
    prefs.upsert_model(model);
    prefs.save().map_err(|e| format!("save_model: {e:?}"))
}

/// 删一条模型。至少保留一条;删到当前 active 会自动回退列表首条。
#[tauri::command]
pub async fn delete_model(id: String) -> Result<(), String> {
    let mut prefs = UserPrefs::load();
    if prefs.advanced.saved_models.len() <= 1 {
        return Err("至少保留一个模型".to_string());
    }
    if let Some(reference) = prefs
        .model_by_id(&id)
        .and_then(|m| m.credential_ref.clone())
    {
        SystemCredentialStore::new()
            .delete(&reference)
            .map_err(|e| sanitize_command_error("delete_model", e.user_message()))?;
    }
    prefs.remove_model(&id);
    prefs.save().map_err(|e| format!("delete_model: {e:?}"))
}

/// 设全局默认模型(新建会话继承它)。不打断已在用的会话——它们各自保持 spawn
/// 时的模型,想换在该会话的 chip 里切。
#[tauri::command]
pub async fn set_active_model(id: String) -> Result<(), String> {
    let mut prefs = UserPrefs::load();
    if prefs.model_by_id(&id).is_none() {
        return Err(format!("model not found: {id}"));
    }
    prefs.advanced.active_model_id = Some(id);
    prefs.save().map_err(|e| format!("set_active_model: {e:?}"))
}

/// 切某会话当前模型(聊天 chip 热切):写 per-session 绑定 + evict 该会话 engine,
/// 下次发消息用新模型重建。`model_id = None` = 回退全局默认。
/// 前端须保证非生成中调用(evict 会打断正在跑的 turn)。
#[tauri::command]
pub async fn set_session_model(
    session_id: String,
    model_id: Option<String>,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    if let Some(mid) = &model_id {
        if UserPrefs::load().model_by_id(mid).is_none() {
            return Err(format!("model not found: {mid}"));
        }
    }
    pool.switch_session_model(&session_id, model_id)
        .await
        .map_err(|error| format!("set_session_model({session_id}): {error:#}"))
}

/// 读取聊天 chip 应显示的模型 id。定时会话尚未手动切换时显示任务初始模型，
/// 手动切换后与普通会话一样显示交互选择。
#[tauri::command]
pub async fn get_session_model_id(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<Option<String>, String> {
    Ok(store.session_model_id(&session_id))
}

fn parse_search_provider(raw: &str) -> Result<SearchProvider, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "bing" => Ok(SearchProvider::Bing),
        "metaso" => Ok(SearchProvider::Metaso),
        "bocha" => Ok(SearchProvider::Bocha),
        "baidu" => Ok(SearchProvider::Baidu),
        "tavily" => Ok(SearchProvider::Tavily),
        other => Err(format!("不支持的搜索源: {other}")),
    }
}

fn resolve_saved_search_key(provider: SearchProvider) -> Result<Option<String>, String> {
    for name in provider.env_key_names() {
        if let Ok(value) = std::env::var(name) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Ok(Some(trimmed.to_string()));
            }
        }
    }
    let mut prefs = UserPrefs::load();
    prefs.refresh_credential_states_with_store(&SystemCredentialStore::new());
    let Some(credential) = prefs.search.credentials.get(&provider) else {
        return Ok(None);
    };
    let Some(reference) = &credential.credential_ref else {
        return Ok(None);
    };
    SystemCredentialStore::new()
        .get(reference)
        .map_err(|error| error.user_message())
        .map(|value| {
            value
                .map(|key| key.trim().to_string())
                .filter(|key| !key.is_empty())
        })
}

#[tauri::command]
pub async fn test_search_provider(
    provider: String,
    api_key: Option<String>,
) -> Result<String, String> {
    let provider = parse_search_provider(&provider)?;
    if provider == SearchProvider::Bing {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .map_err(|e| format!("client: {e}"))?;
        return match client
            .get("https://www.bing.com/search")
            .query(&[("q", "pinvou")])
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => Ok("Bing 搜索可用".to_string()),
            Ok(resp) => Err(format!("Bing HTTP {}", resp.status().as_u16())),
            Err(e) => Err(format!("Bing 搜索不可达: {e}")),
        };
    }
    let provided_key = api_key.unwrap_or_default().trim().to_string();
    let key = if provided_key.is_empty() {
        resolve_saved_search_key(provider)?.unwrap_or_default()
    } else {
        provided_key
    };
    if key.trim().is_empty() {
        return Err("请先填写并保存该搜索源的 API Key".to_string());
    }
    Ok("搜索源凭据已配置".to_string())
}

/// 测试连接:GET {base_url}/models(OpenAI 兼容标准端点),验 base_url + key 可达。
#[tauri::command]
pub async fn test_model_connection(
    base_url: String,
    api_key: String,
    model_id: Option<String>,
) -> Result<String, String> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| format!("client: {e}"))?;
    let mut req = client.get(&url);
    let provided_key = api_key.trim().to_string();
    let key = if provided_key.is_empty() {
        resolve_saved_model_key(model_id.as_deref())?.unwrap_or_default()
    } else {
        provided_key
    };
    if !key.trim().is_empty() {
        req = req.bearer_auth(key.trim());
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            Ok(format!("连接成功 (HTTP {})", resp.status().as_u16()))
        }
        Ok(resp) => Err(format!("HTTP {}", resp.status().as_u16())),
        Err(e) => Err(format!("连接失败: {e}")),
    }
}

/// 持久化 UserPrefs 到 `~/.pinvou3/settings.json`。
///
/// **当前 MVP 限制**：写盘后不重启 Engine。所以：
/// - GUI 视觉项（theme / color_scheme）：前端立即应用，不需要后端介入
/// - 语言切换：写盘成功，但 LLM 的 `locale_tag` 只在下次重启 app 时生效
/// - advanced 字段：同上，重启 app 后生效
///
/// Phase C 会做 in-place engine restart（处理 in-flight turn）。
#[tauri::command]
pub async fn update_settings(prefs: UserPrefs) -> Result<(), String> {
    prepare_prefs_for_save(prefs)?
        .save()
        .map_err(|e| format!("save settings failed: {e:?}"))
}

/// 保存设置后立即重启应用（模型/后端切换后需要重启才能生效）。
#[tauri::command]
pub async fn save_settings_and_restart(
    prefs: UserPrefs,
    app: tauri::AppHandle,
) -> Result<(), String> {
    prepare_prefs_for_save(prefs)?
        .save()
        .map_err(|e| format!("save settings failed: {e:?}"))?;
    eprintln!("[pinvou3-app] settings saved, restarting app...");
    app.restart();
}
use super::prelude::*;
