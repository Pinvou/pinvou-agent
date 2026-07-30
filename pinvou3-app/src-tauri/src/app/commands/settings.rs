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
    prefs.normalize_saved_model_metadata();
    let migration = prefs.migrate_plaintext_api_keys_with_store(&store);
    if !migration.failed_model_ids.is_empty() || !migration.failed_search_providers.is_empty() {
        return Err("credential store unavailable; please reconfigure API Key".to_string());
    }
    prefs.sanitize_plaintext_api_keys();
    prefs.refresh_credential_states_with_store(&store);
    Ok(prefs)
}

fn refresh_safe_prefs(mut prefs: UserPrefs) -> UserPrefs {
    prefs.normalize_saved_model_metadata();
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
    pub provider_kind: Option<String>,
    pub vendor: Option<String>,
    pub endpoint_mode: Option<String>,
    pub credential_mode: crate::features::assistant::runtime_model::ModelCredentialMode,
    pub requires_user_api_key: bool,
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
    let credential_mode = pool.credential_mode_for(effective.as_ref(), bridge.api_key_required());
    let requires_user_api_key = credential_mode
        == crate::features::assistant::runtime_model::ModelCredentialMode::UserManaged;
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
        provider_kind: effective
            .as_ref()
            .and_then(|model| model.provider_kind.clone()),
        vendor: effective.as_ref().and_then(|model| model.vendor.clone()),
        endpoint_mode: effective
            .as_ref()
            .and_then(|model| model.endpoint_mode.clone()),
        credential_mode,
        requires_user_api_key,
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
pub async fn save_model(model: SavedModel, pool: State<'_, EnginePool>) -> Result<(), String> {
    let model_id = model.id.clone();
    UserPrefs::update_transaction(|prefs| {
        let old = prefs.model_by_id(&model.id).cloned();
        let model = apply_model_credential(model, old.as_ref())
            .map_err(|e| sanitize_command_error("save_model", e))?;
        prefs.upsert_model(model);
        Ok(())
    })
    .map_err(|e| sanitize_command_error("save_model", e))?;
    pool.mark_model_updated(&model_id);
    Ok(())
}

/// 删一条模型。至少保留一条;删到当前 active 会自动回退列表首条。
#[tauri::command]
pub async fn delete_model(id: String) -> Result<(), String> {
    UserPrefs::update_transaction(|prefs| {
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
        Ok(())
    })
    .map(|_| ())
    .map_err(|e| sanitize_command_error("delete_model", e))
}

/// 设全局默认模型(新建会话继承它)。不打断已在用的会话——它们各自保持 spawn
/// 时的模型,想换在该会话的 chip 里切。
#[tauri::command]
pub async fn set_active_model(id: String) -> Result<(), String> {
    UserPrefs::update_transaction(|prefs| {
        if prefs.model_by_id(&id).is_none() {
            return Err(format!("model not found: {id}"));
        }
        prefs.advanced.active_model_id = Some(id);
        Ok(())
    })
    .map(|_| ())
    .map_err(|e| sanitize_command_error("set_active_model", e))
}

/// 切某会话当前模型(聊天 chip 热切):写 per-session 绑定 + evict 该会话 engine,
/// 下次发消息用新模型重建。`model_id = None` = 回退全局默认。
/// 前端须保证非生成中调用(evict 会打断正在跑的 turn)。
#[tauri::command]
pub async fn set_session_model(
    session_id: String,
    model_id: Option<String>,
    app: AppHandle,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    if let Some(mid) = &model_id {
        if UserPrefs::load().model_by_id(mid).is_none() {
            return Err(format!("model not found: {mid}"));
        }
    }
    pool.switch_session_model(&session_id, model_id)
        .await
        .map_err(|error| format!("set_session_model({session_id}): {error:#}"))?;
    super::sessions::emit_session_event(&app, "session:model_changed", &session_id, "model");
    Ok(())
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

/// 当前有效模型的图片输入能力(设计 §6.3/§9.2,阶段 G)。前端选图即时警告据此
/// 提示;发送时 chat 命令仍按同一条解析路径(fresh bridge + 会话模型绑定)复核。
#[derive(Debug, Clone, Serialize)]
pub struct ImageInputCapabilityInfo {
    /// `supported` / `unsupported` / `unknown`(EffectiveImageCapability::as_str)。
    pub capability: String,
    /// `native` / `vision_tool_fallback` / `unsupported`(ImageInputMode::as_str)。
    pub image_mode: String,
    /// 是否有可用的视觉模型兜底(含 Supported 主模型自复用)。
    pub has_vision_model: bool,
}

#[tauri::command]
pub async fn get_image_input_capability(
    session_id: Option<String>,
    pool: State<'_, EnginePool>,
    store: State<'_, SessionStore>,
) -> Result<ImageInputCapabilityInfo, String> {
    // 与 chat 命令的图片路由同一套解析:fresh bridge 按 session 绑定模型(含本地
    // vLLM served name 探测与运行时凭据准备)。尚无会话(全新草稿)时退化为
    // get_effective_model_config 同款 prefs 直读,按全局默认模型解析。
    let bridge = match session_id.or_else(|| store.active_id()) {
        Some(sid) => pool
            .fresh_bridge_for(&sid)
            .await
            .map_err(|error| format!("resolve image input capability for {sid}: {error:#}"))?,
        None => {
            let mut bridge = pool.bridge.clone();
            bridge.prefs = refresh_safe_prefs(UserPrefs::load());
            bridge.session_model = None;
            bridge
        }
    };
    Ok(ImageInputCapabilityInfo {
        capability: bridge.effective_image_capability().as_str().to_string(),
        image_mode: bridge.image_input_mode().as_str().to_string(),
        has_vision_model: bridge.has_vision_model(),
    })
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

#[derive(Debug, Clone, Serialize)]
pub struct ModelConnectionTestResult {
    pub ok: bool,
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
    pub http_status: Option<u16>,
}

fn model_connection_result(
    ok: bool,
    code: &str,
    message: &str,
    detail: Option<String>,
    http_status: Option<u16>,
) -> ModelConnectionTestResult {
    ModelConnectionTestResult {
        ok,
        code: code.to_string(),
        message: message.to_string(),
        detail,
        http_status,
    }
}

fn model_connection_http_result(status: reqwest::StatusCode) -> ModelConnectionTestResult {
    let status_code = status.as_u16();
    let detail = Some(format!("HTTP {status_code}"));
    if status.is_success() {
        return model_connection_result(
            true,
            "ok",
            "连接成功，服务可用",
            detail,
            Some(status_code),
        );
    }
    if status.is_redirection() {
        return model_connection_result(
            false,
            "redirect",
            "服务地址发生跳转，当前测试无法确认可用性",
            detail,
            Some(status_code),
        );
    }
    match status_code {
        400 | 422 => model_connection_result(
            false,
            "request_invalid",
            "请求格式不被服务接受，请检查模型配置",
            detail,
            Some(status_code),
        ),
        401 => model_connection_result(
            false,
            "auth_invalid",
            "API Key 无效，请检查后重新填写",
            detail,
            Some(status_code),
        ),
        403 => model_connection_result(
            false,
            "auth_forbidden",
            "当前 API Key 没有访问权限",
            detail,
            Some(status_code),
        ),
        404 => model_connection_result(
            false,
            "endpoint_not_found",
            "服务地址不正确，或该服务不支持模型列表接口",
            detail,
            Some(status_code),
        ),
        405 => model_connection_result(
            false,
            "method_not_allowed",
            "服务可以访问，但不支持当前测试方式",
            detail,
            Some(status_code),
        ),
        408 => model_connection_result(
            false,
            "timeout",
            "连接超时，请检查网络或本地服务是否启动",
            detail,
            Some(status_code),
        ),
        429 => model_connection_result(
            false,
            "rate_limited",
            "请求过于频繁或额度不足，请稍后再试",
            detail,
            Some(status_code),
        ),
        500..=599 => model_connection_result(
            false,
            "server_unavailable",
            "服务暂时不可用，请稍后再试",
            detail,
            Some(status_code),
        ),
        _ => model_connection_result(
            false,
            "http_error",
            "连接失败，请检查配置后重试",
            detail,
            Some(status_code),
        ),
    }
}

fn model_connection_error_result(err: &reqwest::Error) -> ModelConnectionTestResult {
    let raw = crate::platform::credential_store::redact_secret(&err.to_string());
    let raw_lower = raw.to_lowercase();
    let detail = Some(format!("连接失败: {raw}"));
    if err.is_timeout() {
        return model_connection_result(
            false,
            "timeout",
            "连接超时，请检查网络或本地服务是否启动",
            detail,
            None,
        );
    }
    if raw_lower.contains("certificate") || raw_lower.contains("tls") || raw_lower.contains("ssl") {
        return model_connection_result(
            false,
            "tls_error",
            "安全证书校验失败，请检查代理或网络环境",
            detail,
            None,
        );
    }
    if raw_lower.contains("dns")
        || raw_lower.contains("lookup")
        || raw_lower.contains("name or service not known")
    {
        return model_connection_result(
            false,
            "dns_failed",
            "无法解析服务地址，请检查网络",
            detail,
            None,
        );
    }
    if raw_lower.contains("connection refused")
        || raw_lower.contains("os error 10061")
        || raw_lower.contains("actively refused")
    {
        return model_connection_result(
            false,
            "connection_refused",
            "无法连接到服务，请确认本地模型服务已启动",
            detail,
            None,
        );
    }
    model_connection_result(
        false,
        "network_error",
        "网络连接失败，请检查网络后重试",
        detail,
        None,
    )
}

/// 测试连接:GET {base_url}/models(OpenAI 兼容标准端点),验 base_url + key 可达。
#[tauri::command]
pub async fn test_model_connection(
    base_url: String,
    api_key: String,
    model_id: Option<String>,
) -> Result<ModelConnectionTestResult, String> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let parsed_url = match reqwest::Url::parse(&url) {
        Ok(url) => url,
        Err(e) => {
            return Ok(model_connection_result(
                false,
                "invalid_url",
                "服务地址格式不正确",
                Some(e.to_string()),
                None,
            ));
        }
    };
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            return Ok(model_connection_result(
                false,
                "client_error",
                "连接测试初始化失败，请稍后重试",
                Some(format!("client: {e}")),
                None,
            ));
        }
    };
    let mut req = client.get(parsed_url);
    let provided_key = api_key.trim().to_string();
    let key = if provided_key.is_empty() {
        match resolve_saved_model_key(model_id.as_deref()) {
            Ok(key) => key.unwrap_or_default(),
            Err(e) => {
                return Ok(model_connection_result(
                    false,
                    "credential_unavailable",
                    "无法读取已保存的 API Key，请重新填写",
                    Some(e),
                    None,
                ));
            }
        }
    } else {
        provided_key
    };
    if !key.trim().is_empty() {
        req = req.bearer_auth(key.trim());
    }
    match req.send().await {
        Ok(resp) => Ok(model_connection_http_result(resp.status())),
        Err(e) => Ok(model_connection_error_result(&e)),
    }
}

/// 通用设置字段补丁。搜索、桌宠、模型列表和本地模型初始化状态由专用命令管理，
/// 不进入这个协议，避免调用方携带旧的完整快照覆盖其他操作刚写入的值。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeneralSettingsPatch {
    pub theme: Option<Theme>,
    pub color_scheme: Option<ColorScheme>,
    pub language: Option<Language>,
    pub memory_enabled: Option<bool>,
    pub notifications: Option<NotificationPrefs>,
    pub sidebar: Option<SidebarPrefs>,
    pub advanced: Option<AdvancedPrefs>,
}

/// WebUI 仅能修改远程端可见且被授权的设置字段。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebSettingsPatch {
    pub memory_enabled: Option<bool>,
    pub search: Option<SearchPrefs>,
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
pub async fn update_settings(patch: GeneralSettingsPatch) -> Result<UserPrefs, String> {
    persist_general_settings(patch)
}

fn apply_general_settings_patch(current: &mut UserPrefs, patch: GeneralSettingsPatch) {
    if let Some(theme) = patch.theme {
        current.theme = theme;
    }
    if let Some(color_scheme) = patch.color_scheme {
        current.color_scheme = color_scheme;
    }
    if let Some(language) = patch.language {
        current.language = language;
    }
    if let Some(memory_enabled) = patch.memory_enabled {
        current.memory_enabled = memory_enabled;
    }
    if let Some(notifications) = patch.notifications {
        current.notifications = notifications;
    }
    if let Some(sidebar) = patch.sidebar {
        current.sidebar = sidebar;
    }
    if let Some(mut advanced) = patch.advanced {
        // 这些字段有各自的专用写命令。即使高级设置来自旧快照，也无权覆盖它们。
        advanced.saved_models = current.advanced.saved_models.clone();
        advanced.active_model_id = current.advanced.active_model_id.clone();
        advanced.local_vllm_bootstrapped = current.advanced.local_vllm_bootstrapped;
        advanced.local_vllm_setup_declined = current.advanced.local_vllm_setup_declined;
        current.advanced = advanced;
    }
}

fn persist_general_settings(patch: GeneralSettingsPatch) -> Result<UserPrefs, String> {
    UserPrefs::update_transaction(|current| {
        apply_general_settings_patch(current, patch);
        *current = prepare_prefs_for_save(current.clone())?;
        Ok(())
    })
    .map(refresh_safe_prefs)
}

fn persist_search_settings(search: SearchPrefs) -> Result<UserPrefs, String> {
    UserPrefs::update_transaction(|prefs| {
        prefs.search = search;
        *prefs = prepare_prefs_for_save(prefs.clone())?;
        Ok(())
    })
    .map(refresh_safe_prefs)
    .map_err(|e| sanitize_command_error("save search settings", e))
}

pub(crate) fn persist_web_settings(patch: WebSettingsPatch) -> Result<UserPrefs, String> {
    UserPrefs::update_transaction(|prefs| {
        if let Some(memory_enabled) = patch.memory_enabled {
            prefs.memory_enabled = memory_enabled;
        }
        if let Some(search) = patch.search {
            prefs.search = search;
        }
        *prefs = prepare_prefs_for_save(prefs.clone())?;
        Ok(())
    })
    .map(refresh_safe_prefs)
    .map_err(|e| sanitize_command_error("save web settings", e))
}

/// 仅更新搜索配置。模型等其他偏好始终以磁盘最新值为准，避免前端旧快照整份回写。
#[tauri::command]
pub async fn update_search_settings(search: SearchPrefs) -> Result<UserPrefs, String> {
    persist_search_settings(search)
}

/// 保存设置后立即重启应用（模型/后端切换后需要重启才能生效）。
#[tauri::command]
pub async fn save_settings_and_restart(
    patch: GeneralSettingsPatch,
    app: tauri::AppHandle,
) -> Result<(), String> {
    persist_general_settings(patch)?;
    eprintln!("[pinvou3-app] settings saved, restarting app...");
    app.restart();
}

/// 仅保存搜索配置后重启，避免搜索设置覆盖同时发生变化的模型列表。
#[tauri::command]
pub async fn save_search_settings_and_restart(
    search: SearchPrefs,
    app: tauri::AppHandle,
) -> Result<(), String> {
    persist_search_settings(search)?;
    eprintln!("[pinvou3-app] search settings saved, restarting app...");
    app.restart();
}
use super::prelude::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::paths::tests::ENV_LOCK;

    #[test]
    fn general_settings_patch_preserves_unmentioned_and_specialized_domains() {
        let mut current = UserPrefs::default();
        current.migrate_models();
        current.search.provider = SearchProvider::Metaso;
        current.pet.enabled = true;
        let saved_models = current.advanced.saved_models.clone();
        let active_model_id = current.advanced.active_model_id.clone();

        apply_general_settings_patch(
            &mut current,
            GeneralSettingsPatch {
                language: Some(Language::En),
                ..Default::default()
            },
        );

        assert_eq!(current.language, Language::En);
        assert_eq!(current.theme, Theme::default());
        assert_eq!(current.search.provider, SearchProvider::Metaso);
        assert!(current.pet.enabled);
        assert_eq!(current.advanced.saved_models, saved_models);
        assert_eq!(current.advanced.active_model_id, active_model_id);
    }

    #[test]
    fn concurrent_general_setting_patches_preserve_both_fields() {
        use std::sync::{Arc, Barrier};

        let _env_guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let old_home = std::env::var_os("PINVOU3_HOME");
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-general-settings-patches-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let mut initial = UserPrefs::default();
        initial.migrate_models();
        initial.save().expect("initial settings should save");

        let barrier = Arc::new(Barrier::new(3));
        let theme_barrier = Arc::clone(&barrier);
        let theme_thread = std::thread::spawn(move || {
            theme_barrier.wait();
            persist_general_settings(GeneralSettingsPatch {
                theme: Some(Theme::LiquidLight),
                ..Default::default()
            })
            .expect("theme patch should save");
        });

        let language_barrier = Arc::clone(&barrier);
        let language_thread = std::thread::spawn(move || {
            language_barrier.wait();
            persist_general_settings(GeneralSettingsPatch {
                language: Some(Language::En),
                ..Default::default()
            })
            .expect("language patch should save");
        });

        barrier.wait();
        theme_thread.join().expect("theme thread should finish");
        language_thread
            .join()
            .expect("language thread should finish");

        let saved = UserPrefs::load();
        assert_eq!(saved.theme, Theme::LiquidLight);
        assert_eq!(saved.language, Language::En);

        let _ = std::fs::remove_dir_all(&tmp);
        match old_home {
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
    }
}
