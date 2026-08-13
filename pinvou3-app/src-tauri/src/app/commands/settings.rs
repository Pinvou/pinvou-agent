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

/// save_model 的返回:auto 档保存时探测是否发生及回填结果(前端据此展示提示)。
#[derive(Debug, Clone, Serialize)]
pub struct SaveModelOutcome {
    /// 自动探测结果;None = 未触发(非 auto 档或未请求探测)。
    pub image_probe: Option<ImageProbeOutcome>,
}

/// 自动探测回填结果。
#[derive(Debug, Clone, Serialize)]
pub struct ImageProbeOutcome {
    /// `supported` / `unsupported` / `unknown`(连接不通,未做识图探测)。
    pub status: String,
    /// 回填后的档位 `auto` / `pinvou` / `enabled` / `disabled`;None = 保持 auto。
    pub applied_override: Option<String>,
    pub summary: String,
}

/// 增或改一条模型(按 id)。前端负责生成稳定 id。
/// `probe_image_capability=true` 且档位为 auto(自动探测)时,保存前执行
/// 连接 + 识图探测并按结果回填 override(见 `probe_and_fill_image_capability`)。
#[tauri::command]
pub async fn save_model(
    model: SavedModel,
    probe_image_capability: Option<bool>,
    pool: State<'_, EnginePool>,
) -> Result<SaveModelOutcome, String> {
    let model_id = model.id.clone();
    let mut model = model;
    // 自动探测在写盘前执行:结果回填 override 后随本次保存一起落盘。
    let image_probe = if probe_image_capability.unwrap_or(false)
        && model.image_capability_override == ImageCapabilityOverride::Auto
    {
        Some(probe_and_fill_image_capability(&mut model).await)
    } else {
        None
    };
    UserPrefs::update_transaction(|prefs| {
        let old = prefs.model_by_id(&model.id).cloned();
        let model = apply_model_credential(model, old.as_ref())
            .map_err(|e| sanitize_command_error("save_model", e))?;
        prefs.upsert_model(model);
        Ok(())
    })
    .map_err(|e| sanitize_command_error("save_model", e))?;
    pool.mark_model_updated(&model_id);
    Ok(SaveModelOutcome { image_probe })
}

/// 「保存时检测」回填(「保存时检测」档保存时,一律检测,无表内加速):
/// 1. 连接探测不通 → 无法确认 → 回填 `Pinvou`(自动处理,内置表兜底);
/// 2. 连接通 → 识图探测(探测链路与真实链路对齐:always-thinking 模型注入 thinking):
///    - 识别出测试色 → `Enabled`(支持图片);
///    - 明确不支持(unsupported/unverified)→ **不落盘**(保持 `Auto`),
///      返回待决策信号,由前端让用户选择:再次检测 / 去配置视觉模型 /
///      直接保存落「自动处理」——「不支持图片」只来自用户手动选择,
///      机器检测只提示,不钉死;
///    - `error`(网络超时/5xx/401/DNS 等瞬时故障)→ 回填 `Pinvou`
///      (自动处理,内置表兜底)——无法确认不降级已验证模型;
///    - 意外状态 → 保持 `Auto`,不冒充结论。
async fn probe_and_fill_image_capability(model: &mut SavedModel) -> ImageProbeOutcome {
    let connection = probe_model_connection(&model.base_url, &model.api_key, Some(&model.id)).await;
    if !connection.ok {
        model.image_capability_override = ImageCapabilityOverride::Pinvou;
        return ImageProbeOutcome {
            status: "unknown".to_string(),
            applied_override: Some("pinvou".to_string()),
            summary: format!("无法连接模型服务，已按自动处理：{}", connection.message),
        };
    }
    let probe = run_image_capability_probe(
        &model.model,
        &model.base_url,
        &model.api_key,
        Some(&model.id),
    )
    .await;
    match probe.status.as_str() {
        "supported" => {
            model.image_capability_override = ImageCapabilityOverride::Enabled;
            ImageProbeOutcome {
                status: "supported".to_string(),
                applied_override: Some("enabled".to_string()),
                summary: probe.summary,
            }
        }
        // 明确不支持:不落盘,交给用户决策(再次检测/去配置视觉模型/直接保存落自动处理)。
        "unsupported" | "unverified" => ImageProbeOutcome {
            status: probe.status.clone(),
            applied_override: None,
            summary: format!("检测到该模型未能识别图片：{}", probe.summary),
        },
        // 瞬时故障:无法确认,落「自动处理」(内置表兜底,不降级已验证模型)。
        "error" => {
            model.image_capability_override = ImageCapabilityOverride::Pinvou;
            ImageProbeOutcome {
                status: "error".to_string(),
                applied_override: Some("pinvou".to_string()),
                summary: format!(
                    "检测失败（网络或服务问题），已按自动处理：{}",
                    probe.summary
                ),
            }
        }
        other => ImageProbeOutcome {
            status: other.to_string(),
            applied_override: None,
            summary: probe.summary,
        },
    }
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
    /// 当前有效模型 endpoint 是否本机 loopback(设计 §11.8/§11.9):
    /// false 且走 native 直发时前端提示"图片将发送给模型服务商";
    /// true 时图片字节不离开本机,前端不得显示云上传字样。
    pub is_local_endpoint: bool,
    /// 兜底视觉模型 endpoint 是否本机(§11.8/§11.9):None 表示未配置可用视觉模型。
    /// fallback 路径的图片字节发给视觉模型,云端视觉模型时前端同样必须提示。
    pub vision_is_local_endpoint: Option<bool>,
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
        is_local_endpoint: bridge.is_local_endpoint(),
        vision_is_local_endpoint: bridge.vision_uses_local_endpoint(),
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
    Ok(probe_model_connection(&base_url, &api_key, model_id.as_deref()).await)
}

/// 连接探测核心:GET models probe URL 验证服务可达与凭据可用。
/// 复用方:设置页「测试连接」按钮与「自动探测」保存回填(连接不通 → 未知 →
/// 回填 pinvou 决策,不做识图探测)。
pub async fn probe_model_connection(
    base_url: &str,
    api_key: &str,
    model_id: Option<&str>,
) -> ModelConnectionTestResult {
    let url = crate::core::model_endpoint::models_probe_url(base_url);
    let parsed_url = match reqwest::Url::parse(&url) {
        Ok(url) => url,
        Err(e) => {
            return model_connection_result(
                false,
                "invalid_url",
                "服务地址格式不正确",
                Some(e.to_string()),
                None,
            );
        }
    };
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            return model_connection_result(
                false,
                "client_error",
                "连接测试初始化失败，请稍后重试",
                Some(format!("client: {e}")),
                None,
            );
        }
    };
    // Anthropic 官方端点用 x-api-key + anthropic-version,不接受 Bearer。
    let is_anthropic = crate::core::model_endpoint::is_anthropic_api_url(&parsed_url);
    let mut req = client.get(parsed_url);
    let provided_key = api_key.trim().to_string();
    let key = if provided_key.is_empty() {
        match resolve_saved_model_key(model_id) {
            Ok(key) => key.unwrap_or_default(),
            Err(e) => {
                return model_connection_result(
                    false,
                    "credential_unavailable",
                    "无法读取已保存的 API Key，请重新填写",
                    Some(e),
                    None,
                );
            }
        }
    } else {
        provided_key
    };
    if !key.trim().is_empty() {
        if is_anthropic {
            req = req
                .header("x-api-key", key.trim())
                .header("anthropic-version", "2023-06-01");
        } else {
            req = req.bearer_auth(key.trim());
        }
    }
    match req.send().await {
        Ok(resp) => model_connection_http_result(resp.status()),
        Err(e) => model_connection_error_result(&e),
    }
}

/// 测试图片输入能力的结果(设计 §7.3)。`status` 稳定三值,前端据此分态展示:
/// - `supported`: API 接受了图片且给出非空回复;`verified=true` 表示回复提到测试图
///   主体颜色(红/red),是强确认。没提到也算 supported,防止模型表达差异误杀。
/// - `unsupported`: 400/422 等明确参数拒绝,provider 错误摘要在 `summary`。
/// - `error`: 网络/鉴权/超时/空回复等其他失败,与「不支持」严格区分,
///   前端提示用户先确认连接与密钥。
#[derive(Debug, Clone, Serialize)]
pub struct ImageCapabilityTestResult {
    pub status: String,
    pub verified: bool,
    pub summary: String,
    pub http_status: Option<u16>,
}

/// 内置 64×64 纯红 PNG(base64,133 字节)。离线生成的字节常量:不读磁盘、不引新依赖。
/// 字节必须保持合法 PNG(IHDR/IDAT/IEND 块 CRC 与 zlib 数据均有效),由
/// `image_capability_test_png_is_valid_png` 锚定——损坏的测试图会被 provider
/// 以"failed to decode image"400 拒绝,误报为模型不支持图片。
const IMAGE_CAPABILITY_TEST_PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAATElEQVR42u3PQQkAAAgAsetfWiP4FgYrsKZeS0BAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEDgCiyp3PDiJWLS3AAAAABJRU5ErkJggg==";
/// 视觉探针提示词:让模型用一个词说出主体颜色,回复短、可校验。
const IMAGE_CAPABILITY_TEST_PROMPT: &str = "这张图片的主体颜色是什么？用一个词回答。";
const IMAGE_CAPABILITY_TEST_MAX_TOKENS: u32 = 64;
/// always-thinking 模型(kimi-for-coding 等)的探测上限:thinking 开启时先输出
/// 大量 reasoning 再输出答案,64 token 会被 thinking 吃掉或触发网关 max_tokens
/// 下限 400,导致识图探测误判(2026-08 kimi-for-coding 二次实测)。1024 足够
/// reasoning 起步 + 一句「红色」答案。
const IMAGE_CAPABILITY_TEST_MAX_TOKENS_THINKING: u32 = 1024;

fn image_capability_result(
    status: &str,
    verified: bool,
    summary: String,
    http_status: Option<u16>,
) -> ImageCapabilityTestResult {
    ImageCapabilityTestResult {
        status: status.to_string(),
        verified,
        summary,
        http_status,
    }
}

/// 折叠空白并按字符截断,避免 provider 大段错误/回复撑爆设置页。
fn summarize_image_probe_text(text: &str, max_chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = collapsed.chars();
    let taken: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{taken}…")
    } else {
        taken
    }
}

fn image_capability_test_payload(model: &str) -> serde_json::Value {
    // 注意:不得携带 temperature——kimi-for-coding 等模型只接受默认值,显式传会 400。
    let thinking_model =
        crate::features::assistant::image_capability::moonshot_model_requires_explicit_thinking(
            model,
        );
    let mut payload = serde_json::json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": IMAGE_CAPABILITY_TEST_PROMPT },
                {
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:image/png;base64,{IMAGE_CAPABILITY_TEST_PNG_BASE64}"),
                    },
                },
            ],
        }],
        // always-thinking 模型:thinking 输出吃 token,上限提到 1024;
        // 其余模型保持 64(回答「红色」绰绰有余)。
        "max_tokens": if thinking_model {
            IMAGE_CAPABILITY_TEST_MAX_TOKENS_THINKING
        } else {
            IMAGE_CAPABILITY_TEST_MAX_TOKENS
        },
        "stream": false,
    });
    // always-thinking 模型(kimi-for-coding 等):官方接入要求 thinking 保持开启,
    // 与 bridge 真实链路同一口径(`moonshot_model_requires_explicit_thinking`)。
    // 省略该参数网关会拒绝请求,导致识图探测误判为「不支持图片」(2026-08 实测)。
    if thinking_model {
        payload["thinking"] = serde_json::json!({ "type": "enabled" });
    }
    payload
}

/// Anthropic Messages 协议的识图探测 payload:图片用 `image` 块 + base64 source
/// (OpenAI 兼容端点的 image_url data URL 不被 Anthropic 接受)。max_tokens 1024
/// 与 OpenAI 路径同档,保证回复空间。
fn image_capability_test_payload_anthropic(model: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "max_tokens": IMAGE_CAPABILITY_TEST_MAX_TOKENS_THINKING,
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": IMAGE_CAPABILITY_TEST_PROMPT },
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": IMAGE_CAPABILITY_TEST_PNG_BASE64,
                    },
                },
            ],
        }],
    })
}

fn extract_chat_reply_summary(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let content = value
        .get("choices")?
        .as_array()?
        .first()?
        .get("message")?
        .get("content")?;
    match content {
        serde_json::Value::String(text) => Some(text.trim().to_string()),
        // 部分 provider 把 content 拆成 parts 数组。
        serde_json::Value::Array(parts) => Some(
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(|text| text.as_str()))
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string(),
        ),
        _ => None,
    }
}

fn extract_provider_error_summary(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let message = value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(|message| message.as_str())
        .or_else(|| value.get("message").and_then(|message| message.as_str()))?;
    Some(summarize_image_probe_text(message, 200))
}

fn image_capability_response_summary(body: &str, status_code: u16) -> String {
    extract_provider_error_summary(body)
        .or_else(|| {
            let raw = summarize_image_probe_text(body, 200);
            if raw.is_empty() {
                None
            } else {
                Some(raw)
            }
        })
        .unwrap_or_else(|| format!("HTTP {status_code}"))
}

/// 400/422 错误体是否构成"明确不支持图片输入":需**同时**提及图片相关概念
/// 且带否定/拒绝语境(does not support image、仅支持文本…)。只提 image 不够——
/// 模型名(gpt-image-1 not found)、网关正文同样可能含 image,不能一律归为
/// 模型不支持图片(审阅缺口 #104)。其余 400/422 判"无法确认"。
fn image_rejection_signal(text: &str) -> bool {
    let lower = text.to_lowercase();
    let mentions_image = [
        "image",
        "vision",
        "multimodal",
        "图片",
        "图像",
        "识图",
        "视觉",
    ]
    .iter()
    .any(|keyword| lower.contains(keyword));
    if !mentions_image {
        return false;
    }
    [
        "not support",
        "not accept",
        "cannot",
        "can't",
        "won't",
        "unable to",
        "unsupported",
        "unknown variant", // unknown variant `image_url`(schema 拒绝,如 DeepSeek)
        "expected `text`", // 只接受 text 内容块,未定义 image_url 类型
        "expected text",
        "does not allow",
        "only text",
        "only supports text",
        "only accepts text",
        "text-only",
        "不支持",
        "无法识别",
        "不能识别",
        "无法处理",
        "不能处理",
        "不处理",
        "仅支持文本",
        "只支持文本",
        "仅处理文本",
    ]
    .iter()
    .any(|keyword| lower.contains(keyword))
}

/// 2xx 回复是否**明确表示看不到图片**("我看不到图片"等):模型自己承认
/// 不能看图,是"不支持图像识别"的直接证据;其余未识别出测试色的回复
/// (描述了图片/纯模板话术)统一归"未能正确识别图像,原因未知"。
fn reply_explicitly_cannot_see(reply: &str) -> bool {
    let lower = reply.to_lowercase();
    [
        "看不到",
        "没有图片",
        "未看到",
        "无法看到",
        "不能看到",
        "看不了",
        "看不到图片",
        "cannot see",
        "can't see",
        "can not see",
        "no image",
        "no picture",
        "don't see",
        "do not see",
        "not see any",
    ]
    .iter()
    .any(|keyword| lower.contains(keyword))
}

/// 按模型回复文本分类(OpenAI choices 与 Anthropic content 提取出文本后共用):
/// ① 识别出测试色 → 支持;② 明确说看不到图片 → 不支持图像识别;
/// ③ 其余未识别出测试色(描述图片/模板话术/给错颜色)→ 未能正确识别图像,原因未知
/// ——可能是请求层(图片未送达/网关剥离)、模型行为(拒绝回答)或能力,不冒充结论。
fn classify_image_reply(reply: &str, status_code: u16) -> ImageCapabilityTestResult {
    let summarized = summarize_image_probe_text(reply, 120);
    let recognized = reply.to_lowercase().contains("red") || reply.contains('红');
    if recognized {
        image_capability_result("supported", true, summarized, Some(status_code))
    } else if reply_explicitly_cannot_see(reply) {
        image_capability_result(
            "unsupported",
            false,
            format!("模型回复看不到图片，不支持图像识别（模型回复：{summarized}）"),
            Some(status_code),
        )
    } else {
        image_capability_result(
            "unverified",
            false,
            format!("未能正确识别图像，原因未知（模型回复：{summarized}）"),
            Some(status_code),
        )
    }
}

fn classify_image_capability_http(
    status: reqwest::StatusCode,
    body: &str,
) -> ImageCapabilityTestResult {
    let status_code = status.as_u16();
    if status.is_success() {
        return match extract_chat_reply_summary(body) {
            Some(reply) if !reply.is_empty() => classify_image_reply(&reply, status_code),
            _ => image_capability_result(
                "unverified",
                false,
                "服务接受了请求但返回了空回复，未能正确识别图像，原因未知".to_string(),
                Some(status_code),
            ),
        };
    }
    let summary = image_capability_response_summary(body, status_code);
    match status_code {
        // 400/422 统一两档:错误体点名图片/视觉输入拒绝 → 不支持图像识别;
        // 其余(模型名、参数、网关格式)→ 未能正确识别图像,原因未知。
        400 | 422 if image_rejection_signal(&summary) => {
            image_capability_result("unsupported", false, summary, Some(status_code))
        }
        400 | 422 => image_capability_result(
            "unverified",
            false,
            format!("未能正确识别图像，原因未知（HTTP {status_code}）：{summary}"),
            Some(status_code),
        ),
        _ => image_capability_result("error", false, summary, Some(status_code)),
    }
}

fn image_capability_transport_error(err: &reqwest::Error) -> ImageCapabilityTestResult {
    let raw = crate::platform::credential_store::redact_secret(&err.to_string());
    let raw_lower = raw.to_lowercase();
    let summary = if err.is_timeout() {
        format!("连接超时，请检查网络或服务是否启动: {raw}")
    } else if raw_lower.contains("certificate")
        || raw_lower.contains("tls")
        || raw_lower.contains("ssl")
    {
        format!("安全证书校验失败，请检查代理或网络环境: {raw}")
    } else if raw_lower.contains("dns")
        || raw_lower.contains("lookup")
        || raw_lower.contains("name or service not known")
    {
        format!("无法解析服务地址，请检查网络: {raw}")
    } else if raw_lower.contains("connection refused")
        || raw_lower.contains("os error 10061")
        || raw_lower.contains("actively refused")
    {
        format!("无法连接到服务，请确认服务已启动: {raw}")
    } else {
        format!("网络连接失败，请检查网络后重试: {raw}")
    };
    image_capability_result("error", false, summary, None)
}

/// 识图探测核心(设计 §7.3):POST {base_url}/chat/completions 携带内置纯色 PNG,
/// 返回 `ImageCapabilityTestResult`(不发 Result 错误——连接失败等也收敛为结果)。
/// 入参与凭据解析和 test_model_connection 一致:表单新填 key 优先,否则读已保存凭据。
/// 复用方:设置页「测试图片能力」按钮与「自动探测」保存回填。
pub async fn run_image_capability_probe(
    model: &str,
    base_url: &str,
    api_key: &str,
    model_id: Option<&str>,
) -> ImageCapabilityTestResult {
    let model = model.trim().to_string();
    if model.is_empty() {
        return image_capability_result(
            "error",
            false,
            "模型 ID 为空，请先填写模型".to_string(),
            None,
        );
    }
    // 复用 strip_chat_completions_suffix:用户 base_url 已含 /v1/chat/completions 时
    // 不再拼出重复路径(prefs/model.rs 同一口径)。
    let url = format!(
        "{}/chat/completions",
        crate::platform::prefs::model::strip_chat_completions_suffix(base_url)
    );
    let parsed_url = match reqwest::Url::parse(&url) {
        Ok(url) => url,
        Err(e) => {
            return image_capability_result(
                "error",
                false,
                format!("服务地址格式不正确: {e}"),
                None,
            );
        }
    };
    // Anthropic 原生协议:URL /v1/messages + x-api-key/anthropic-version,payload
    // 用 image base64 source;OpenAI 兼容端点走 chat/completions + image_url。
    let is_anthropic = crate::core::model_endpoint::is_anthropic_api_url(&parsed_url);
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            return image_capability_result("error", false, format!("测试初始化失败: {e}"), None);
        }
    };
    let provided_key = api_key.trim().to_string();
    let key = if provided_key.is_empty() {
        match resolve_saved_model_key(model_id) {
            Ok(key) => key.unwrap_or_default(),
            Err(e) => {
                return image_capability_result(
                    "error",
                    false,
                    format!("无法读取已保存的 API Key，请重新填写: {e}"),
                    None,
                );
            }
        }
    } else {
        provided_key
    };
    if is_anthropic {
        let messages_url = crate::core::model_endpoint::anthropic_messages_url(base_url);
        let mut req = client
            .post(messages_url)
            .json(&image_capability_test_payload_anthropic(&model));
        if !key.trim().is_empty() {
            req = req
                .header("x-api-key", key.trim())
                .header("anthropic-version", "2023-06-01");
        }
        return match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                if status.is_success() {
                    // Anthropic 成功响应无 choices:content 是 text/image 块数组,
                    // 提取文本后走同一套回复分类;空回复 → 无法确认。
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
                        if let Some(reply) =
                            crate::core::model_endpoint::anthropic_messages_text(&value)
                        {
                            if !reply.trim().is_empty() {
                                return classify_image_reply(&reply, status.as_u16());
                            }
                        }
                    }
                    image_capability_result(
                        "unverified",
                        false,
                        "服务接受了请求但返回了空回复，未能正确识别图像，原因未知".to_string(),
                        Some(status.as_u16()),
                    )
                } else {
                    classify_image_capability_http(status, &body)
                }
            }
            Err(e) => image_capability_transport_error(&e),
        };
    }
    let mut req = client
        .post(url)
        .json(&image_capability_test_payload(&model));
    if !key.trim().is_empty() {
        req = req.bearer_auth(key.trim());
    }
    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            classify_image_capability_http(status, &body)
        }
        Err(e) => image_capability_transport_error(&e),
    }
}

/// 设置页「测试图片能力」按钮入口:仅由用户主动点击触发,无任何启动/定时自动测试。
#[tauri::command]
pub async fn test_image_input_capability(
    model: String,
    base_url: String,
    api_key: String,
    model_id: Option<String>,
) -> Result<ImageCapabilityTestResult, String> {
    Ok(run_image_capability_probe(&model, &base_url, &api_key, model_id.as_deref()).await)
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
    fn image_capability_payload_omits_temperature_and_embeds_data_url() {
        let payload = image_capability_test_payload("kimi-for-coding");
        assert_eq!(
            payload.get("model").and_then(|m| m.as_str()),
            Some("kimi-for-coding")
        );
        // kimi-for-coding 等模型只接受默认 temperature,显式传会 400。
        assert!(payload.get("temperature").is_none());
        // kimi-for-coding 是 always-thinking 模型:官方接入要求 thinking 开启,
        // 探测请求必须带,否则 Moonshot 网关拒绝请求误判为「不支持图片」(2026-08 实测);
        // 且 max_tokens 需足够 thinking 输出(64 会被 reasoning 吃掉或 400)。
        assert_eq!(
            payload["thinking"]["type"].as_str(),
            Some("enabled"),
            "kimi-for-coding 探测请求必须带 thinking: {{type: enabled}}"
        );
        assert_eq!(
            payload.get("max_tokens").and_then(|v| v.as_u64()),
            Some(IMAGE_CAPABILITY_TEST_MAX_TOKENS_THINKING as u64)
        );
        let content = payload["messages"][0]["content"]
            .as_array()
            .expect("content must be multimodal parts");
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"].as_str(), Some("text"));
        assert_eq!(
            content[0]["text"].as_str(),
            Some(IMAGE_CAPABILITY_TEST_PROMPT)
        );
        assert_eq!(content[1]["type"].as_str(), Some("image_url"));
        let url = content[1]["image_url"]["url"]
            .as_str()
            .expect("image part must carry a data url");
        assert!(url.starts_with("data:image/png;base64,"));
        assert!(url.len() > "data:image/png;base64,".len() + 100);
    }

    #[test]
    fn image_capability_payload_thinking_only_for_always_thinking_models() {
        // 非 Moonshot always-thinking 模型:不注入 thinking,max_tokens 保持 64。
        let payload = image_capability_test_payload("gpt-4o");
        assert!(
            payload.get("thinking").is_none(),
            "gpt-4o 探测请求不应携带 thinking"
        );
        assert_eq!(
            payload.get("max_tokens").and_then(|v| v.as_u64()),
            Some(IMAGE_CAPABILITY_TEST_MAX_TOKENS as u64),
            "普通模型 max_tokens 保持 64"
        );
        // always-thinking 模型:注入 thinking 且 max_tokens 提到 1024
        // (64 会被 reasoning 吃掉或触发网关下限 400)。
        let payload = image_capability_test_payload("kimi-k3");
        assert_eq!(
            payload["thinking"]["type"].as_str(),
            Some("enabled"),
            "kimi-k3 同属 always-thinking,需注入 thinking"
        );
        assert_eq!(
            payload.get("max_tokens").and_then(|v| v.as_u64()),
            Some(IMAGE_CAPABILITY_TEST_MAX_TOKENS_THINKING as u64),
            "thinking 模型 max_tokens 需足够 reasoning 输出"
        );
        let payload = image_capability_test_payload("kimi-k2.7-code");
        assert_eq!(
            payload["thinking"]["type"].as_str(),
            Some("enabled"),
            "kimi-k2.7-code 同属 always-thinking,需注入 thinking"
        );
        assert_eq!(
            payload.get("max_tokens").and_then(|v| v.as_u64()),
            Some(IMAGE_CAPABILITY_TEST_MAX_TOKENS_THINKING as u64)
        );
    }

    /// 锚定内置测试图是合法 PNG:块 CRC 与 zlib 数据全部有效、64×64 RGB 纯红。
    /// 损坏的测试图会被 provider 以"failed to decode image"400 拒绝,
    /// 把"测试图坏了"误报成"模型不支持图片"。
    #[test]
    fn image_capability_test_png_is_valid_png() {
        use base64::Engine;

        let png = base64::engine::general_purpose::STANDARD
            .decode(IMAGE_CAPABILITY_TEST_PNG_BASE64)
            .expect("base64 must decode");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "PNG signature");

        let mut idat = Vec::new();
        let mut pos = 8;
        let mut seen_ihdr = false;
        while pos < png.len() {
            let len = u32::from_be_bytes(png[pos..pos + 4].try_into().unwrap()) as usize;
            let kind = &png[pos + 4..pos + 8];
            let data = &png[pos + 8..pos + 8 + len];
            let crc_stored =
                u32::from_be_bytes(png[pos + 8 + len..pos + 12 + len].try_into().unwrap());
            let mut crc = flate2::Crc::new();
            crc.update(&png[pos + 4..pos + 8 + len]);
            assert_eq!(
                crc_stored,
                crc.sum(),
                "CRC mismatch in chunk {}",
                String::from_utf8_lossy(kind)
            );
            match kind {
                b"IHDR" => {
                    seen_ihdr = true;
                    assert_eq!(&data[..4], &64u32.to_be_bytes(), "width");
                    assert_eq!(&data[4..8], &64u32.to_be_bytes(), "height");
                    assert_eq!(data[8], 8, "bit depth");
                    assert_eq!(data[9], 2, "color type RGB");
                }
                b"IDAT" => idat.extend_from_slice(data),
                b"IEND" => break,
                other => panic!("unexpected chunk {}", String::from_utf8_lossy(other)),
            }
            pos += 12 + len;
        }
        assert!(seen_ihdr && !idat.is_empty());

        use std::io::Read as _;
        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(&idat[..])
            .read_to_end(&mut raw)
            .expect("IDAT must inflate");
        let stride = 64 * 3 + 1;
        assert_eq!(raw.len(), stride * 64, "inflated size");
        for row in raw.chunks(stride) {
            assert_eq!(row[0], 0, "filter byte");
            assert!(
                row[1..].chunks_exact(3).all(|p| p == [255, 0, 0]),
                "solid red"
            );
        }
    }

    #[test]
    fn image_capability_supported_verified_when_reply_mentions_color() {
        let body = r#"{"choices":[{"message":{"content":"红色"}}]}"#;
        let result = classify_image_capability_http(reqwest::StatusCode::OK, body);
        assert_eq!(result.status, "supported");
        assert!(result.verified);
        assert_eq!(result.summary, "红色");
        assert_eq!(result.http_status, Some(200));

        let body = r#"{"choices":[{"message":{"content":"The dominant color is red."}}]}"#;
        let result = classify_image_capability_http(reqwest::StatusCode::OK, body);
        assert_eq!(result.status, "supported");
        assert!(result.verified);
    }

    #[test]
    fn image_capability_unverified_when_reply_omits_color() {
        // API 接受图片但回复没识别出测试色(纯红):统一归"未能正确识别图像,
        // 原因未知"——描述图片内容、给错颜色、模板话术都不冒充结论。
        let body = r#"{"choices":[{"message":{"content":"I see a small square picture."}}]}"#;
        let result = classify_image_capability_http(reqwest::StatusCode::OK, body);
        assert_eq!(result.status, "unverified");
        assert!(!result.verified);
        assert!(result.summary.contains("未能正确识别图像"));
        assert!(result.summary.contains("square"));

        let body = r#"{"choices":[{"message":{"content":"图片是蓝色的"}}]}"#;
        let result = classify_image_capability_http(reqwest::StatusCode::OK, body);
        assert_eq!(result.status, "unverified");

        // 纯文本模板回复同样"原因未知",不宣称支持也不宣称不支持。
        let body = r#"{"choices":[{"message":{"content":"你好，我是AI助手，有什么可以帮你？"}}]}"#;
        let result = classify_image_capability_http(reqwest::StatusCode::OK, body);
        assert_eq!(result.status, "unverified");
        assert!(result.summary.contains("未能正确识别图像"));
    }

    #[test]
    fn image_capability_explicit_cannot_see_is_unsupported() {
        // 模型明确说看不到图片:自己承认不能看图,是"不支持图像识别"
        // 的直接证据。
        let body = r#"{"choices":[{"message":{"content":"我看不到任何图片，请直接告诉我问题"}}]}"#;
        let result = classify_image_capability_http(reqwest::StatusCode::OK, body);
        assert_eq!(result.status, "unsupported");
        assert!(!result.verified);
        assert!(result.summary.contains("不支持图像识别"));
        assert!(result.summary.contains("看不到图片"));

        // 英文表述同样命中。
        let body =
            r#"{"choices":[{"message":{"content":"I cannot see any image in the message"}}]}"#;
        let result = classify_image_capability_http(reqwest::StatusCode::OK, body);
        assert_eq!(result.status, "unsupported");
    }

    #[test]
    fn image_capability_empty_reply_is_unverified() {
        let body = r#"{"choices":[{"message":{"content":"  "}}]}"#;
        let result = classify_image_capability_http(reqwest::StatusCode::OK, body);
        assert_eq!(result.status, "unverified");
        assert!(!result.verified);
        assert!(result.summary.contains("空回复"));
    }

    #[test]
    fn image_capability_400_is_unsupported_with_provider_summary() {
        let body = r#"{"error":{"message":"this model does not support image input","type":"invalid_request_error"}}"#;
        let result = classify_image_capability_http(reqwest::StatusCode::BAD_REQUEST, body);
        assert_eq!(result.status, "unsupported");
        assert!(!result.verified);
        assert_eq!(result.summary, "this model does not support image input");
        assert_eq!(result.http_status, Some(400));
    }

    #[test]
    fn image_capability_400_422_unverified_when_not_image_rejection() {
        // 400/422 未点名图片的拒绝(模型名不存在、参数错误、网关格式):
        // 统一"未能正确识别图像,原因未知",不归"不支持图像识别"。
        let result = classify_image_capability_http(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"The model `gpt-image-1` does not exist"}}"#,
        );
        assert_eq!(result.status, "unverified");
        assert!(result.summary.contains("未能正确识别图像"));
        assert!(result.summary.contains("does not exist"));

        let result = classify_image_capability_http(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"max_tokens 65536 exceeds the maximum"}}"#,
        );
        assert_eq!(result.status, "unverified");

        // 泛拒绝(无可见配置问题)同样"原因未知"。
        let result = classify_image_capability_http(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"Request rejected by gateway"}}"#,
        );
        assert_eq!(result.status, "unverified");
        assert!(result.summary.contains("Request rejected by gateway"));

        // 422 提及 image 但无否定语境(invalid image payload):同上。
        let result = classify_image_capability_http(
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            "invalid image payload",
        );
        assert_eq!(result.status, "unverified");
        assert!(result.summary.contains("invalid image payload"));
    }

    #[test]
    fn image_capability_400_unsupported_only_when_negative_image_context() {
        // 明确"不支持图片输入":mention image + 否定语境 → unsupported。
        let result = classify_image_capability_http(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"this model does not support image input"}}"#,
        );
        assert_eq!(result.status, "unsupported");
        assert_eq!(result.summary, "this model does not support image input");
        assert_eq!(result.http_status, Some(400));

        // 网关层拒绝的图片请求(OpenAI 风格 only text)→ unsupported。
        let result = classify_image_capability_http(
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            r#"{"error":{"message":"invalid image_url: this API only supports text content"}}"#,
        );
        assert_eq!(result.status, "unsupported");
        assert_eq!(result.http_status, Some(422));

        // DeepSeek 真实报错:网关 schema 未定义 image_url 内容块、只接受 text。
        // "unknown variant `image_url`, expected `text`" 是明确的不支持图片输入。
        let result = classify_image_capability_http(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"Failed to deserialize the JSON body into the target type: messages[0]: unknown variant `image_url`, expected `text` at line 1 column 401"}}"#,
        );
        assert_eq!(result.status, "unsupported");
        assert_eq!(result.http_status, Some(400));
    }

    #[test]
    fn image_capability_auth_and_server_failures_are_errors() {
        let unauthorized = classify_image_capability_http(
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{"error":{"message":"Incorrect API key provided"}}"#,
        );
        assert_eq!(unauthorized.status, "error");
        assert_eq!(unauthorized.summary, "Incorrect API key provided");

        let server = classify_image_capability_http(reqwest::StatusCode::BAD_GATEWAY, "");
        assert_eq!(server.status, "error");
        assert_eq!(server.summary, "HTTP 502");
    }

    #[test]
    fn image_capability_reply_parts_array_is_joined() {
        let body = r#"{"choices":[{"message":{"content":[{"type":"text","text":"red"},{"type":"text","text":"square"}]}}]}"#;
        let result = classify_image_capability_http(reqwest::StatusCode::OK, body);
        assert_eq!(result.status, "supported");
        assert!(result.verified);
        assert_eq!(result.summary, "red square");
    }

    #[test]
    fn image_capability_summary_truncates_long_text() {
        let long = "a".repeat(300);
        let summary = summarize_image_probe_text(&long, 120);
        assert_eq!(summary.chars().count(), 121);
        assert!(summary.ends_with('…'));
        let whitespace = summarize_image_probe_text("  red\n\tcolor  ", 120);
        assert_eq!(whitespace, "red color");
    }

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
