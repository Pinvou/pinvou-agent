use chrono::Utc;
use std::time::Instant;

use crate::credential_store::{CredentialReference, CredentialStore};

use super::adapter::{HttpLlmApiHubAdapter, LlmApiHubAdapter, NewApiTokenUsage};
use super::identity::{IdentityResolver, SystemIdentityResolver};
use super::models::{
    BuiltinLlmApiModelsResponse, DeviceBindingStatus, EnsureLlmApiBindingResponse,
    LlmApiAdminOverviewResponse, LlmApiBinding, LlmApiError, LlmApiErrorCode, LlmApiIdentity,
    LlmApiPolicy, LlmApiStatusResponse, ProvisioningStatus, QuotaStatus,
};
use super::store::{admin_overview_items, FileLlmApiBindingStore, LlmApiBindingStore};

pub fn ensure_binding<S, C, A, I>(
    store: &S,
    credentials: &C,
    adapter: &A,
    identity_resolver: &I,
) -> Result<EnsureLlmApiBindingResponse, LlmApiError>
where
    S: LlmApiBindingStore,
    C: CredentialStore,
    A: LlmApiHubAdapter,
    I: IdentityResolver,
{
    let identity = identity_resolver.resolve_identity()?;
    let policy = LlmApiPolicy::default();
    let mut binding = store
        .get_binding(&identity.pinvou_user_id, &identity.device_binding_id)?
        .unwrap_or_else(|| LlmApiBinding::new(&identity, policy.clone()));

    if !binding.enabled || binding.provisioning_status == ProvisioningStatus::Disabled {
        return Err(LlmApiError::new(
            LlmApiErrorCode::ServiceDisabled,
            "该用户 AI 服务已被禁用",
            false,
        ));
    }

    if binding.provisioning_status == ProvisioningStatus::Ready {
        if let Some(reference) = binding.token_credential_ref.as_ref() {
            if credentials.get(reference)?.is_some() {
                return Ok(EnsureLlmApiBindingResponse {
                    status: ProvisioningStatus::Ready,
                    created: false,
                    retryable: false,
                    message: "AI 服务已开通".to_string(),
                });
            }
        }
    }

    let created = binding.newapi_user_id.is_none() && binding.newapi_token_id.is_none();

    binding.policy = policy.clone();
    binding.mark_status(ProvisioningStatus::QueryingUser);
    binding.clear_error();
    store.upsert_binding(binding.clone())?;

    let user = match adapter.lookup_user(&identity) {
        Ok(user) => user,
        Err(err) => {
            binding.mark_error(&err);
            store.upsert_binding(binding)?;
            return Err(err);
        }
    };
    binding.newapi_user_id = Some(user.id.clone());
    binding.mark_status(ProvisioningStatus::CreatingToken);
    store.upsert_binding(binding.clone())?;

    let token = match adapter.create_token(&identity, &user.id) {
        Ok(token) => token,
        Err(err) => {
            binding.mark_error(&err);
            store.upsert_binding(binding)?;
            return Err(err);
        }
    };
    binding.newapi_token_id = Some(token.id.clone());
    let credential_ref = CredentialReference::for_llmapi_token(
        &identity.pinvou_user_id,
        &identity.device_binding_id,
    );
    credentials.set(&credential_ref, &token.token)?;
    binding.token_credential_ref = Some(credential_ref);
    binding.mark_status(ProvisioningStatus::ConfiguringPolicy);
    store.upsert_binding(binding.clone())?;

    if let Err(err) = adapter.configure_policy(&identity, &user.id, &token.id, &policy) {
        binding.mark_error(&err);
        store.upsert_binding(binding)?;
        return Err(err);
    }

    binding.mark_status(ProvisioningStatus::Ready);
    binding.clear_error();
    store.upsert_binding(binding)?;

    Ok(EnsureLlmApiBindingResponse {
        status: ProvisioningStatus::Ready,
        created,
        retryable: false,
        message: "AI 服务已开通".to_string(),
    })
}

pub fn ensure_binding_for_current_user() -> Result<EnsureLlmApiBindingResponse, LlmApiError> {
    let started_at = Instant::now();
    let store = FileLlmApiBindingStore::default();
    let credentials = crate::credential_store::SystemCredentialStore::new();
    let identity = SystemIdentityResolver;
    let resolved = identity.resolve_identity()?;
    log::info!(
        "[llmapi_hub][provisioning] ensure start user_id={} device_id={}",
        resolved.pinvou_user_id,
        resolved.device_binding_id
    );
    if let Some(binding) =
        store.get_binding(&resolved.pinvou_user_id, &resolved.device_binding_id)?
    {
        log::info!(
            "[llmapi_hub][provisioning] existing binding found user_id={} status={:?} enabled={} has_token_ref={}",
            resolved.pinvou_user_id,
            binding.provisioning_status,
            binding.enabled,
            binding.token_credential_ref.is_some()
        );
        if binding.enabled && binding.provisioning_status == ProvisioningStatus::Ready {
            let mut binding = binding;
            ensure_binding_api_key(&store, &credentials, &resolved, &mut binding)?;
            log::info!(
                "[llmapi_hub][provisioning] ensure ready from existing binding user_id={} elapsed_ms={}",
                resolved.pinvou_user_id,
                started_at.elapsed().as_millis()
            );
            return Ok(EnsureLlmApiBindingResponse {
                status: ProvisioningStatus::Ready,
                created: false,
                retryable: false,
                message: "AI 服务已开通".to_string(),
            });
        }
    } else {
        log::info!(
            "[llmapi_hub][provisioning] no existing binding user_id={}",
            resolved.pinvou_user_id
        );
    }
    if let Some(response) = ensure_binding_from_local_api_key(&store, &credentials, &resolved)? {
        log::info!(
            "[llmapi_hub][provisioning] ensure ready from local api key user_id={} elapsed_ms={}",
            resolved.pinvou_user_id,
            started_at.elapsed().as_millis()
        );
        return Ok(response);
    }
    if let Some(response) = ensure_binding_from_user_session(&store, &credentials, &resolved)? {
        log::info!(
            "[llmapi_hub][provisioning] ensure ready from user session user_id={} elapsed_ms={}",
            resolved.pinvou_user_id,
            started_at.elapsed().as_millis()
        );
        return Ok(response);
    }
    log::info!(
        "[llmapi_hub][provisioning] fallback to generated device login user_id={}",
        resolved.pinvou_user_id
    );
    let response = ensure_binding_from_generated_device_login(&store, &credentials, &resolved)?;
    log::info!(
        "[llmapi_hub][provisioning] ensure ready from generated device login user_id={} elapsed_ms={}",
        resolved.pinvou_user_id,
        started_at.elapsed().as_millis()
    );
    Ok(response)
}

pub fn login_user_session_system(
    username: String,
    password: String,
) -> Result<EnsureLlmApiBindingResponse, LlmApiError> {
    let store = FileLlmApiBindingStore::default();
    let credentials = crate::credential_store::SystemCredentialStore::new();
    let identity = SystemIdentityResolver.resolve_identity()?;
    let adapter = HttpLlmApiHubAdapter::for_token_usage();
    let session = adapter.login_user_session(&username, &password)?;
    let reference = CredentialReference::for_llmapi_user_session(
        &identity.pinvou_user_id,
        &identity.device_binding_id,
    );
    let payload = serde_json::json!({
        "user_id": session.user_id,
        "access_token": session.access_token,
    });
    credentials.set(&reference, &payload.to_string())?;
    if let Some(response) = ensure_binding_from_user_session(&store, &credentials, &identity)? {
        return Ok(response);
    }
    Err(LlmApiError::new(
        LlmApiErrorCode::ProvisioningFailed,
        "New API user session was saved but default API key provisioning did not start",
        true,
    ))
}

pub fn save_user_session_system(
    user_id: String,
    access_token: String,
) -> Result<EnsureLlmApiBindingResponse, LlmApiError> {
    let store = FileLlmApiBindingStore::default();
    let credentials = crate::credential_store::SystemCredentialStore::new();
    let identity = SystemIdentityResolver.resolve_identity()?;
    let user_id = user_id.trim();
    let access_token = access_token.trim();
    if user_id.is_empty() || access_token.is_empty() {
        return Err(LlmApiError::new(
            LlmApiErrorCode::ProvisioningFailed,
            "New API user id or access token is empty",
            false,
        ));
    }
    let reference = CredentialReference::for_llmapi_user_session(
        &identity.pinvou_user_id,
        &identity.device_binding_id,
    );
    let payload = serde_json::json!({
        "user_id": user_id,
        "access_token": access_token,
    });
    credentials.set(&reference, &payload.to_string())?;
    if let Some(response) = ensure_binding_from_user_session(&store, &credentials, &identity)? {
        return Ok(response);
    }
    Err(LlmApiError::new(
        LlmApiErrorCode::ProvisioningFailed,
        "New API user session was saved but default API key provisioning did not start",
        true,
    ))
}

fn ensure_binding_from_local_api_key<C>(
    store: &FileLlmApiBindingStore,
    credentials: &C,
    identity: &LlmApiIdentity,
) -> Result<Option<EnsureLlmApiBindingResponse>, LlmApiError>
where
    C: CredentialStore,
{
    let started_at = Instant::now();
    log::info!(
        "[llmapi_hub][provisioning] local api key check start user_id={}",
        identity.pinvou_user_id
    );
    let reference = CredentialReference::for_llmapi_token(
        &identity.pinvou_user_id,
        &identity.device_binding_id,
    );
    let Some(token) = credentials.get(&reference)? else {
        log::info!(
            "[llmapi_hub][provisioning] local api key missing user_id={} elapsed_ms={}",
            identity.pinvou_user_id,
            started_at.elapsed().as_millis()
        );
        return Ok(None);
    };
    let adapter = HttpLlmApiHubAdapter::for_token_usage();
    let usage = match adapter.token_usage(&token) {
        Ok(usage) => usage,
        Err(err) => {
            log::warn!(
                "[llmapi_hub][provisioning] local api key usage failed user_id={} code={:?} retryable={} elapsed_ms={} message={}",
                identity.pinvou_user_id,
                err.code,
                err.retryable,
                started_at.elapsed().as_millis(),
                err.message
            );
            return Ok(None);
        }
    };
    let policy = LlmApiPolicy::default();
    let existing = store.get_binding(&identity.pinvou_user_id, &identity.device_binding_id)?;
    let created = existing.is_none();
    let mut binding = existing.unwrap_or_else(|| LlmApiBinding::new(identity, policy.clone()));

    if !binding.enabled || binding.provisioning_status == ProvisioningStatus::Disabled {
        return Err(LlmApiError::new(
            LlmApiErrorCode::ServiceDisabled,
            "该用户 AI 服务已被禁用",
            false,
        ));
    }

    binding.policy = policy;
    binding.token_credential_ref = Some(reference);
    binding.mark_status(ProvisioningStatus::Ready);
    apply_token_usage(&mut binding, usage);
    sync_available_models(&adapter, &token, identity, &mut binding);
    binding.clear_error();
    store.upsert_binding(binding)?;

    log::info!(
        "[llmapi_hub][provisioning] local api key ready user_id={} elapsed_ms={}",
        identity.pinvou_user_id,
        started_at.elapsed().as_millis()
    );
    Ok(Some(EnsureLlmApiBindingResponse {
        status: ProvisioningStatus::Ready,
        created,
        retryable: false,
        message: "AI 服务已开通".to_string(),
    }))
}

fn ensure_binding_from_user_session<C>(
    store: &FileLlmApiBindingStore,
    credentials: &C,
    identity: &LlmApiIdentity,
) -> Result<Option<EnsureLlmApiBindingResponse>, LlmApiError>
where
    C: CredentialStore,
{
    let started_at = Instant::now();
    log::info!(
        "[llmapi_hub][provisioning] user session check start user_id={}",
        identity.pinvou_user_id
    );
    let Some(session) = HttpLlmApiHubAdapter::user_session_from_credentials(credentials, identity)?
    else {
        log::info!(
            "[llmapi_hub][provisioning] user session missing user_id={} elapsed_ms={}",
            identity.pinvou_user_id,
            started_at.elapsed().as_millis()
        );
        return Ok(None);
    };
    log::info!(
        "[llmapi_hub][provisioning] user session found user_id={} newapi_user_id={}",
        identity.pinvou_user_id,
        session.user_id
    );

    let policy = LlmApiPolicy::default();
    let mut binding = store
        .get_binding(&identity.pinvou_user_id, &identity.device_binding_id)?
        .unwrap_or_else(|| LlmApiBinding::new(identity, policy.clone()));

    if !binding.enabled || binding.provisioning_status == ProvisioningStatus::Disabled {
        return Err(LlmApiError::new(
            LlmApiErrorCode::ServiceDisabled,
            "该用户 AI 服务已被禁用",
            false,
        ));
    }

    let created = binding.newapi_user_id.is_none() && binding.newapi_token_id.is_none();
    binding.policy = policy;
    binding.newapi_user_id = Some(session.user_id);
    binding.mark_status(ProvisioningStatus::Ready);
    binding.clear_error();

    if let Err(err) = ensure_binding_api_key(store, credentials, identity, &mut binding) {
        log::warn!(
            "[llmapi_hub][provisioning] user session api key ensure failed user_id={} code={:?} retryable={} elapsed_ms={} message={}",
            identity.pinvou_user_id,
            err.code,
            err.retryable,
            started_at.elapsed().as_millis(),
            err.message
        );
        binding.mark_error(&err);
        store.upsert_binding(binding)?;
        return Err(err);
    }

    log::info!(
        "[llmapi_hub][provisioning] user session ready user_id={} elapsed_ms={}",
        identity.pinvou_user_id,
        started_at.elapsed().as_millis()
    );
    Ok(Some(EnsureLlmApiBindingResponse {
        status: ProvisioningStatus::Ready,
        created,
        retryable: false,
        message: "AI 服务已开通".to_string(),
    }))
}

fn ensure_binding_from_generated_device_login<C>(
    store: &FileLlmApiBindingStore,
    credentials: &C,
    identity: &LlmApiIdentity,
) -> Result<EnsureLlmApiBindingResponse, LlmApiError>
where
    C: CredentialStore,
{
    let started_at = Instant::now();
    let username = identity.pinvou_user_id.as_str();
    let password = generated_device_password(identity)?;
    log::info!(
        "[llmapi_hub][provisioning] generated device login start user_id={} device_id={}",
        identity.pinvou_user_id,
        identity.device_binding_id
    );
    let adapter = HttpLlmApiHubAdapter::for_token_usage();
    let session = adapter
        .login_user_session(username, &password)
        .map_err(|err| {
            log::warn!(
                "[llmapi_hub][provisioning] generated device login failed user_id={} code={:?} retryable={} elapsed_ms={} message={}",
                identity.pinvou_user_id,
                err.code,
                err.retryable,
                started_at.elapsed().as_millis(),
                err.message
            );
            LlmApiError::new(
                err.code,
                format!(
                    "内置模型自动登录失败，请确认后台已存在当前设备账户并使用设备 SN 派生密码: {}",
                    err.message
                ),
                err.retryable,
            )
        })?;
    log::info!(
        "[llmapi_hub][provisioning] generated device login ok user_id={} newapi_user_id={} elapsed_ms={}",
        identity.pinvou_user_id,
        session.user_id,
        started_at.elapsed().as_millis()
    );
    let reference = CredentialReference::for_llmapi_user_session(
        &identity.pinvou_user_id,
        &identity.device_binding_id,
    );
    let payload = serde_json::json!({
        "user_id": session.user_id,
        "access_token": session.access_token,
    });
    credentials.set(&reference, &payload.to_string())?;
    log::info!(
        "[llmapi_hub][provisioning] generated device session stored user_id={}",
        identity.pinvou_user_id
    );
    ensure_binding_from_user_session(store, credentials, identity)?.ok_or_else(|| {
        LlmApiError::new(
            LlmApiErrorCode::ProvisioningFailed,
            "内置模型自动登录成功，但 default API key 同步未启动",
            true,
        )
    })
}

fn generated_device_password(identity: &LlmApiIdentity) -> Result<String, LlmApiError> {
    let hash_prefix = identity
        .bios_sn_hash
        .trim()
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(12)
        .collect::<String>()
        .to_ascii_lowercase();
    if hash_prefix.len() < 12 {
        return Err(LlmApiError::new(
            LlmApiErrorCode::DeviceBindingFailed,
            "设备 SN 派生信息不足，无法生成内置模型登录密码",
            false,
        ));
    }
    Ok(format!("Pv3-{hash_prefix}"))
}

pub fn status_for_current_user<S, I>(
    store: &S,
    identity_resolver: &I,
) -> Result<LlmApiStatusResponse, LlmApiError>
where
    S: LlmApiBindingStore,
    I: IdentityResolver,
{
    let identity = identity_resolver.resolve_identity()?;
    let binding = store.get_binding(&identity.pinvou_user_id, &identity.device_binding_id)?;
    Ok(match binding {
        Some(binding) => status_from_binding(&binding),
        None => LlmApiStatusResponse {
            pinvou_user_id: Some(identity.pinvou_user_id),
            device_binding_status: DeviceBindingStatus::Bound,
            enabled: true,
            provisioning_status: ProvisioningStatus::NotStarted,
            quota: None,
            last_call_status: None,
            last_error_code: None,
            last_error_message: None,
        },
    })
}

pub fn status_for_current_user_system() -> Result<LlmApiStatusResponse, LlmApiError> {
    let store = FileLlmApiBindingStore::default();
    let identity = SystemIdentityResolver.resolve_identity()?;
    let Some(mut binding) =
        store.get_binding(&identity.pinvou_user_id, &identity.device_binding_id)?
    else {
        return Ok(LlmApiStatusResponse {
            pinvou_user_id: Some(identity.pinvou_user_id),
            device_binding_status: DeviceBindingStatus::Bound,
            enabled: true,
            provisioning_status: ProvisioningStatus::NotStarted,
            quota: None,
            last_call_status: None,
            last_error_code: None,
            last_error_message: None,
        });
    };

    let credentials = crate::credential_store::SystemCredentialStore::new();
    refresh_binding_quota_from_newapi(&store, &credentials, &identity, &mut binding)?;
    Ok(status_from_binding(&binding))
}

pub fn available_models_system() -> Result<BuiltinLlmApiModelsResponse, LlmApiError> {
    ensure_binding_for_current_user()?;
    Ok(local_builtin_models_response())
}

pub fn set_default_model_system(model: &str) -> Result<BuiltinLlmApiModelsResponse, LlmApiError> {
    let model = model.trim();
    if model.is_empty() {
        return Err(LlmApiError::new(
            LlmApiErrorCode::ProvisioningFailed,
            "内置模型名称不能为空",
            false,
        ));
    }
    let mut prefs = crate::bridge::prefs::UserPrefs::load();
    if !prefs
        .advanced
        .builtin_llmapi_available_models
        .iter()
        .any(|available| available == model)
    {
        return Err(LlmApiError::new(
            LlmApiErrorCode::ProvisioningFailed,
            format!("后台当前未返回模型: {model}"),
            true,
        ));
    }

    prefs.advanced.builtin_llmapi_default_model = Some(model.to_string());
    prefs.ensure_builtin_llmapi_model();
    prefs.save().map_err(|err| {
        LlmApiError::new(
            LlmApiErrorCode::Unavailable,
            format!("保存内置模型配置失败: {err}"),
            true,
        )
    })?;

    let store = FileLlmApiBindingStore::default();
    let identity = SystemIdentityResolver.resolve_identity()?;
    if let Some(mut binding) =
        store.get_binding(&identity.pinvou_user_id, &identity.device_binding_id)?
    {
        binding.policy.allowed_models = vec![model.to_string()];
        binding.updated_at = Utc::now();
        store.upsert_binding(binding)?;
    }

    Ok(local_builtin_models_response())
}

fn refresh_binding_quota_from_newapi<C>(
    store: &FileLlmApiBindingStore,
    credentials: &C,
    identity: &LlmApiIdentity,
    binding: &mut LlmApiBinding,
) -> Result<(), LlmApiError>
where
    C: CredentialStore,
{
    log::info!(
        "[llmapi_hub][provisioning] api key ensure start user_id={} status={:?} enabled={}",
        identity.pinvou_user_id,
        binding.provisioning_status,
        binding.enabled
    );
    if !binding.enabled || binding.provisioning_status != ProvisioningStatus::Ready {
        log::info!(
            "[llmapi_hub][provisioning] api key ensure skipped user_id={} status={:?} enabled={}",
            identity.pinvou_user_id,
            binding.provisioning_status,
            binding.enabled
        );
        return Ok(());
    }
    ensure_binding_api_key(store, credentials, identity, binding)
}

fn apply_token_usage(binding: &mut LlmApiBinding, usage: NewApiTokenUsage) {
    binding.usage.period = super::models::current_period();
    binding.usage.limit_tokens = usage.total_granted;
    binding.usage.used_tokens = usage.total_used;
    binding.usage.remaining_tokens = usage.total_available;
    binding.usage.last_synced_at = Some(Utc::now());
    binding.updated_at = Utc::now();
}

fn selected_model_from_binding(binding: &LlmApiBinding) -> String {
    let prefs = crate::bridge::prefs::UserPrefs::load();
    let available_models = if prefs.advanced.builtin_llmapi_available_models.is_empty() {
        binding.policy.allowed_models.clone()
    } else {
        prefs.advanced.builtin_llmapi_available_models.clone()
    };
    crate::llmapi_hub::select_model(
        &available_models,
        prefs.advanced.builtin_llmapi_default_model.as_deref(),
    )
}

fn local_builtin_models_response() -> BuiltinLlmApiModelsResponse {
    let prefs = crate::bridge::prefs::UserPrefs::load();
    let available_models = prefs.advanced.builtin_llmapi_available_models.clone();
    let default_model = crate::llmapi_hub::select_model(
        &available_models,
        prefs.advanced.builtin_llmapi_default_model.as_deref(),
    );
    BuiltinLlmApiModelsResponse {
        available_models,
        default_model,
    }
}

fn sync_available_models(
    adapter: &HttpLlmApiHubAdapter,
    token: &str,
    identity: &LlmApiIdentity,
    binding: &mut LlmApiBinding,
) {
    let started_at = Instant::now();
    log::info!(
        "[llmapi_hub][provisioning] available model sync start user_id={}",
        identity.pinvou_user_id
    );
    match adapter.available_models(token) {
        Ok(models) => {
            let mut prefs = crate::bridge::prefs::UserPrefs::load();
            let selected = crate::llmapi_hub::select_model(
                &models,
                prefs.advanced.builtin_llmapi_default_model.as_deref(),
            );
            if selected.trim().is_empty() {
                log::warn!(
                    "[llmapi_hub][provisioning] available model sync returned no selected model user_id={} elapsed_ms={}",
                    identity.pinvou_user_id,
                    started_at.elapsed().as_millis()
                );
                return;
            }
            prefs.advanced.builtin_llmapi_available_models = models.clone();
            prefs.advanced.builtin_llmapi_default_model = Some(selected.clone());
            prefs.ensure_builtin_llmapi_model();
            if let Err(err) = prefs.save() {
                log::warn!(
                    "[llmapi_hub][provisioning] available model sync settings save failed user_id={} error={}",
                    identity.pinvou_user_id,
                    err
                );
            }
            binding.policy.allowed_models = vec![selected.clone()];
            binding.updated_at = Utc::now();
            log::info!(
                "[llmapi_hub][provisioning] available model sync ok user_id={} selected={} count={} elapsed_ms={}",
                identity.pinvou_user_id,
                selected,
                models.len(),
                started_at.elapsed().as_millis()
            );
        }
        Err(err) => {
            log::warn!(
                "[llmapi_hub][provisioning] available model sync failed user_id={} code={:?} retryable={} elapsed_ms={} message={}",
                identity.pinvou_user_id,
                err.code,
                err.retryable,
                started_at.elapsed().as_millis(),
                err.message
            );
        }
    }
}

fn ensure_binding_api_key<C>(
    store: &FileLlmApiBindingStore,
    credentials: &C,
    identity: &LlmApiIdentity,
    binding: &mut LlmApiBinding,
) -> Result<(), LlmApiError>
where
    C: CredentialStore,
{
    let started_at = Instant::now();
    log::info!(
        "[llmapi_hub][provisioning] api key ensure start user_id={} status={:?} enabled={}",
        identity.pinvou_user_id,
        binding.provisioning_status,
        binding.enabled
    );
    if !binding.enabled || binding.provisioning_status != ProvisioningStatus::Ready {
        log::info!(
            "[llmapi_hub][provisioning] api key ensure skipped user_id={} status={:?} enabled={}",
            identity.pinvou_user_id,
            binding.provisioning_status,
            binding.enabled
        );
        return Ok(());
    }

    let reference = binding.token_credential_ref.clone().unwrap_or_else(|| {
        CredentialReference::for_llmapi_token(&identity.pinvou_user_id, &identity.device_binding_id)
    });
    log::info!(
        "[llmapi_hub][provisioning] api key reference resolved user_id={} service={} account={}",
        identity.pinvou_user_id,
        reference.service,
        reference.account
    );
    log::info!(
        "[llmapi_hub][provisioning] token usage adapter create start user_id={}",
        identity.pinvou_user_id
    );
    let adapter = HttpLlmApiHubAdapter::for_token_usage();
    log::info!(
        "[llmapi_hub][provisioning] token usage adapter create done user_id={} elapsed_ms={}",
        identity.pinvou_user_id,
        started_at.elapsed().as_millis()
    );

    log::info!(
        "[llmapi_hub][provisioning] credential get token start user_id={} service={} account={}",
        identity.pinvou_user_id,
        reference.service,
        reference.account
    );
    let token_lookup = credentials.get(&reference);
    log::info!(
        "[llmapi_hub][provisioning] credential get token returned user_id={} elapsed_ms={}",
        identity.pinvou_user_id,
        started_at.elapsed().as_millis()
    );
    if let Some(token) = token_lookup? {
        log::info!(
            "[llmapi_hub][provisioning] api key found locally user_id={}",
            identity.pinvou_user_id
        );
        match adapter.token_usage(&token) {
            Ok(usage) => {
                binding.token_credential_ref = Some(reference);
                apply_token_usage(binding, usage);
                sync_available_models(&adapter, &token, identity, binding);
                binding.clear_error();
                store.upsert_binding(binding.clone())?;
                log::info!(
                    "[llmapi_hub][provisioning] api key usage ok user_id={} model={} used={} remaining={} elapsed_ms={}",
                    identity.pinvou_user_id,
                    binding.policy.allowed_models.first().map(String::as_str).unwrap_or(""),
                    binding.usage.used_tokens,
                    binding.usage.remaining_tokens,
                    started_at.elapsed().as_millis()
                );
                return Ok(());
            }
            Err(err) => {
                log::warn!(
                    "[llmapi_hub][provisioning] api key usage failed user_id={} code={:?} retryable={} elapsed_ms={} message={}",
                    identity.pinvou_user_id,
                    err.code,
                    err.retryable,
                    started_at.elapsed().as_millis(),
                    err.message
                );
                log::info!(
                    "[llmapi_hub][provisioning] user session lookup after api key failure start user_id={}",
                    identity.pinvou_user_id
                );
                let session_lookup =
                    HttpLlmApiHubAdapter::user_session_from_credentials(credentials, identity)?;
                log::info!(
                    "[llmapi_hub][provisioning] user session lookup after api key failure done user_id={} found={}",
                    identity.pinvou_user_id,
                    session_lookup.is_some()
                );
                if session_lookup.is_none() {
                    log::warn!(
                        "[llmapi_hub][provisioning] api key refresh impossible because user session missing user_id={}",
                        identity.pinvou_user_id
                    );
                    return Err(err);
                }
            }
        }
    } else {
        log::info!(
            "[llmapi_hub][provisioning] api key missing locally user_id={}",
            identity.pinvou_user_id
        );
    }

    log::info!(
        "[llmapi_hub][provisioning] user session lookup for refresh start user_id={}",
        identity.pinvou_user_id
    );
    let session = HttpLlmApiHubAdapter::user_session_from_credentials(credentials, identity)?
        .ok_or_else(|| {
            log::warn!(
                "[llmapi_hub][provisioning] api key refresh failed because user session missing user_id={}",
                identity.pinvou_user_id
            );
            LlmApiError::new(
                LlmApiErrorCode::ProvisioningFailed,
                "内置模型登录会话缺失，无法刷新 default API key；请重新登录后重试",
                true,
            )
        })?;
    log::info!(
        "[llmapi_hub][provisioning] user session lookup for refresh done user_id={} newapi_user_id={}",
        identity.pinvou_user_id,
        session.user_id
    );
    log::info!(
        "[llmapi_hub][provisioning] default api key refresh start user_id={} newapi_user_id={}",
        identity.pinvou_user_id,
        session.user_id
    );
    let refreshed = adapter.default_token(&session)?;
    log::info!(
        "[llmapi_hub][provisioning] credential set refreshed token start user_id={} service={} account={}",
        identity.pinvou_user_id,
        reference.service,
        reference.account
    );
    credentials.set(&reference, &refreshed.token)?;
    log::info!(
        "[llmapi_hub][provisioning] credential set refreshed token done user_id={} elapsed_ms={}",
        identity.pinvou_user_id,
        started_at.elapsed().as_millis()
    );
    let usage = adapter.token_usage(&refreshed.token)?;

    binding.newapi_user_id = Some(session.user_id);
    binding.newapi_token_id = Some(refreshed.id.clone());
    binding.token_credential_ref = Some(reference);
    apply_token_usage(binding, usage);
    sync_available_models(&adapter, &refreshed.token, identity, binding);
    binding.mark_status(ProvisioningStatus::Ready);
    binding.clear_error();
    store.upsert_binding(binding.clone())?;
    log::info!(
        "[llmapi_hub][provisioning] default api key refresh ok user_id={} token_id={} model={} used={} remaining={} elapsed_ms={}",
        identity.pinvou_user_id,
        refreshed.id,
        binding.policy.allowed_models.first().map(String::as_str).unwrap_or(""),
        binding.usage.used_tokens,
        binding.usage.remaining_tokens,
        started_at.elapsed().as_millis()
    );
    Ok(())
}

pub fn ready_model_config<S, C, I>(
    store: &S,
    credentials: &C,
    identity_resolver: &I,
) -> Result<ReadyModelConfig, LlmApiError>
where
    S: LlmApiBindingStore,
    C: CredentialStore,
    I: IdentityResolver,
{
    let started_at = Instant::now();
    log::info!("[llmapi_hub][provisioning] ready_model_config start");
    let identity = identity_resolver.resolve_identity()?;
    log::info!(
        "[llmapi_hub][provisioning] ready_model_config identity resolved user_id={} device_id={}",
        identity.pinvou_user_id,
        identity.device_binding_id
    );
    let binding = store
        .get_binding(&identity.pinvou_user_id, &identity.device_binding_id)?
        .ok_or_else(|| {
            LlmApiError::new(
                LlmApiErrorCode::ProvisioningFailed,
                "当前用户尚未开通 AI 服务",
                true,
            )
        })?;
    log::info!(
        "[llmapi_hub][provisioning] ready_model_config binding loaded user_id={} status={:?} enabled={} has_token_ref={}",
        identity.pinvou_user_id,
        binding.provisioning_status,
        binding.enabled,
        binding.token_credential_ref.is_some()
    );
    if !binding.enabled {
        return Err(LlmApiError::new(
            LlmApiErrorCode::ServiceDisabled,
            "该用户 AI 服务已被禁用",
            false,
        ));
    }
    if binding.provisioning_status != ProvisioningStatus::Ready {
        return Err(LlmApiError::new(
            LlmApiErrorCode::ProvisioningFailed,
            "当前用户 AI 服务尚未就绪",
            true,
        ));
    }
    let reference = binding.token_credential_ref.clone().ok_or_else(|| {
        LlmApiError::new(
            LlmApiErrorCode::ProvisioningFailed,
            "AI 服务凭据引用缺失",
            true,
        )
    })?;
    log::info!(
        "[llmapi_hub][provisioning] ready_model_config credential get start user_id={} service={} account={}",
        identity.pinvou_user_id,
        reference.service,
        reference.account
    );
    let credential = credentials.get(&reference);
    log::info!(
        "[llmapi_hub][provisioning] ready_model_config credential get returned user_id={} elapsed_ms={}",
        identity.pinvou_user_id,
        started_at.elapsed().as_millis()
    );
    if credential?.is_none() {
        return Err(LlmApiError::new(
            LlmApiErrorCode::ProvisioningFailed,
            "AI 服务凭据缺失，请重试开通",
            true,
        ));
    }
    log::info!(
        "[llmapi_hub][provisioning] ready_model_config ok user_id={} elapsed_ms={}",
        identity.pinvou_user_id,
        started_at.elapsed().as_millis()
    );
    let model = selected_model_from_binding(&binding);
    log::info!(
        "[llmapi_hub][provisioning] ready_model_config selected model user_id={} model={}",
        identity.pinvou_user_id,
        model
    );
    Ok(ReadyModelConfig {
        provider: "openai".to_string(),
        base_url: crate::llmapi_hub::DEFAULT_CHAT_BASE_URL.to_string(),
        model,
        token_credential_ref: reference,
    })
}

pub fn ready_saved_model<S, C, I>(
    store: &S,
    credentials: &C,
    identity_resolver: &I,
) -> Result<crate::bridge::prefs::SavedModel, LlmApiError>
where
    S: LlmApiBindingStore,
    C: CredentialStore,
    I: IdentityResolver,
{
    log::info!("[llmapi_hub][provisioning] ready_saved_model start");
    let config = ready_model_config(store, credentials, identity_resolver)?;
    log::info!(
        "[llmapi_hub][provisioning] ready_saved_model config ready base_url={} model={}",
        config.base_url,
        config.model
    );
    Ok(crate::bridge::prefs::SavedModel {
        id: crate::bridge::prefs::BUILTIN_LLMAPI_MODEL_ID.to_string(),
        name: crate::bridge::prefs::BUILTIN_LLMAPI_MODEL_NAME.to_string(),
        preset: crate::bridge::prefs::ModelPreset::OpenaiCompatible,
        model: config.model,
        base_url: config.base_url,
        api_key: String::new(),
        credential_ref: Some(config.token_credential_ref),
        credential_state: crate::credential_store::CredentialState::Configured,
        has_secret: true,
        credential_action: None,
    })
}

pub fn ready_saved_model_system() -> Result<crate::bridge::prefs::SavedModel, LlmApiError> {
    let started_at = Instant::now();
    log::info!("[llmapi_hub][provisioning] ready_saved_model_system start");
    let store = FileLlmApiBindingStore::default();
    log::info!("[llmapi_hub][provisioning] ready_saved_model_system store ready");
    let credentials = crate::credential_store::SystemCredentialStore::new();
    log::info!("[llmapi_hub][provisioning] ready_saved_model_system credential store ready");
    let identity = SystemIdentityResolver.resolve_identity()?;
    log::info!(
        "[llmapi_hub][provisioning] ready_saved_model_system identity resolved user_id={} device_id={}",
        identity.pinvou_user_id,
        identity.device_binding_id
    );
    let Some(mut binding) =
        store.get_binding(&identity.pinvou_user_id, &identity.device_binding_id)?
    else {
        return Err(LlmApiError::new(
            LlmApiErrorCode::ProvisioningFailed,
            "Current user has not enabled the built-in AI service",
            true,
        ));
    };
    log::info!(
        "[llmapi_hub][provisioning] ready_saved_model_system binding loaded user_id={} status={:?} enabled={} has_token_ref={}",
        identity.pinvou_user_id,
        binding.provisioning_status,
        binding.enabled,
        binding.token_credential_ref.is_some()
    );
    ensure_binding_api_key(&store, &credentials, &identity, &mut binding)?;
    log::info!(
        "[llmapi_hub][provisioning] ready_saved_model_system api key ensured user_id={} elapsed_ms={}",
        identity.pinvou_user_id,
        started_at.elapsed().as_millis()
    );
    let model = ready_saved_model(&store, &credentials, &StaticIdentityResolver(identity))?;
    log::info!(
        "[llmapi_hub][provisioning] ready_saved_model_system ok elapsed_ms={}",
        started_at.elapsed().as_millis()
    );
    Ok(model)
}

pub fn ready_saved_model_from_local_binding_system(
) -> Result<crate::bridge::prefs::SavedModel, LlmApiError> {
    let started_at = Instant::now();
    log::info!("[llmapi_hub][provisioning] ready_saved_model_local start");
    let store = FileLlmApiBindingStore::default();
    let identity = SystemIdentityResolver.resolve_identity()?;
    log::info!(
        "[llmapi_hub][provisioning] ready_saved_model_local identity resolved user_id={} device_id={}",
        identity.pinvou_user_id,
        identity.device_binding_id
    );
    let binding = store
        .get_binding(&identity.pinvou_user_id, &identity.device_binding_id)?
        .ok_or_else(|| {
            LlmApiError::new(
                LlmApiErrorCode::ProvisioningFailed,
                "Current user has not enabled the built-in AI service",
                true,
            )
        })?;
    log::info!(
        "[llmapi_hub][provisioning] ready_saved_model_local binding loaded user_id={} status={:?} enabled={} has_token_ref={}",
        identity.pinvou_user_id,
        binding.provisioning_status,
        binding.enabled,
        binding.token_credential_ref.is_some()
    );
    if !binding.enabled {
        return Err(LlmApiError::new(
            LlmApiErrorCode::ServiceDisabled,
            "该用户 AI 服务已被禁用",
            false,
        ));
    }
    if binding.provisioning_status != ProvisioningStatus::Ready {
        return Err(LlmApiError::new(
            LlmApiErrorCode::ProvisioningFailed,
            "当前用户 AI 服务尚未就绪",
            true,
        ));
    }
    let reference = binding.token_credential_ref.clone().ok_or_else(|| {
        LlmApiError::new(
            LlmApiErrorCode::ProvisioningFailed,
            "AI 服务凭据引用缺失",
            true,
        )
    })?;
    log::info!(
        "[llmapi_hub][provisioning] ready_saved_model_local ok user_id={} elapsed_ms={}",
        identity.pinvou_user_id,
        started_at.elapsed().as_millis()
    );
    let model = selected_model_from_binding(&binding);
    log::info!(
        "[llmapi_hub][provisioning] ready_saved_model_local selected model user_id={} model={}",
        identity.pinvou_user_id,
        model
    );
    Ok(crate::bridge::prefs::SavedModel {
        id: crate::bridge::prefs::BUILTIN_LLMAPI_MODEL_ID.to_string(),
        name: crate::bridge::prefs::BUILTIN_LLMAPI_MODEL_NAME.to_string(),
        preset: crate::bridge::prefs::ModelPreset::OpenaiCompatible,
        model,
        base_url: crate::llmapi_hub::DEFAULT_CHAT_BASE_URL.to_string(),
        api_key: String::new(),
        credential_ref: Some(reference),
        credential_state: crate::credential_store::CredentialState::Configured,
        has_secret: true,
        credential_action: None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyModelConfig {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub token_credential_ref: CredentialReference,
}

pub fn set_user_enabled_system(
    pinvou_user_id: &str,
    enabled: bool,
) -> Result<LlmApiBinding, LlmApiError> {
    let store = FileLlmApiBindingStore::default();
    let updated = store.set_enabled(pinvou_user_id, enabled)?;
    if let (Some(token_id), Ok(adapter)) = (
        updated.newapi_token_id.as_deref(),
        HttpLlmApiHubAdapter::from_system_credentials(),
    ) {
        let _ = adapter.set_token_enabled(token_id, enabled);
    }
    Ok(updated)
}

pub fn admin_overview_system(
    query: Option<String>,
    status: Option<ProvisioningStatus>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<LlmApiAdminOverviewResponse, LlmApiError> {
    let store = FileLlmApiBindingStore::default();
    let mut items = admin_overview_items(store.list_bindings()?);
    if let Some(query) = query
        .map(|q| q.trim().to_string())
        .filter(|q| !q.is_empty())
    {
        items.retain(|item| item.pinvou_user_id.contains(&query));
    }
    if let Some(status) = status {
        items.retain(|item| item.provisioning_status == status);
    }
    items.sort_by_key(|item| item.updated_at);
    items.reverse();
    let total = items.len();
    let offset = offset.unwrap_or(0);
    let limit = limit.unwrap_or(50);
    let items = items.into_iter().skip(offset).take(limit).collect();
    Ok(LlmApiAdminOverviewResponse { items, total })
}

pub fn retry_binding_system(
    pinvou_user_id: String,
    device_binding_id: String,
) -> Result<EnsureLlmApiBindingResponse, LlmApiError> {
    let store = FileLlmApiBindingStore::default();
    let credentials = crate::credential_store::SystemCredentialStore::new();
    let adapter = HttpLlmApiHubAdapter::from_system_credentials()?;
    let binding = store
        .get_binding(&pinvou_user_id, &device_binding_id)?
        .ok_or_else(|| {
            LlmApiError::new(
                LlmApiErrorCode::UserNotFound,
                "未找到指定用户的 LLM API Hub 绑定",
                false,
            )
        })?;
    let identity = StaticIdentityResolver(LlmApiIdentity {
        pinvou_user_id: binding.pinvou_user_id,
        device_binding_id: binding.device_binding_id,
        bios_sn_hash: String::new(),
    });
    ensure_binding(&store, &credentials, &adapter, &identity)
}

fn status_from_binding(binding: &LlmApiBinding) -> LlmApiStatusResponse {
    LlmApiStatusResponse {
        pinvou_user_id: Some(binding.pinvou_user_id.clone()),
        device_binding_status: DeviceBindingStatus::Bound,
        enabled: binding.enabled,
        provisioning_status: binding.provisioning_status,
        quota: Some(QuotaStatus::from(&binding.usage)),
        last_call_status: None,
        last_error_code: binding.last_error_code,
        last_error_message: binding.last_error_message.clone(),
    }
}

#[derive(Debug, Clone)]
pub struct StaticIdentityResolver(pub LlmApiIdentity);

impl IdentityResolver for StaticIdentityResolver {
    fn resolve_identity(&self) -> Result<LlmApiIdentity, LlmApiError> {
        Ok(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_store::{CredentialStore, MemoryCredentialStore};
    use crate::llmapi_hub::adapter::tests::MockLlmApiHubAdapter;
    use crate::llmapi_hub::store::MemoryLlmApiBindingStore;

    fn identity() -> StaticIdentityResolver {
        StaticIdentityResolver(LlmApiIdentity {
            pinvou_user_id: "u_1".to_string(),
            device_binding_id: "dev_a".to_string(),
            bios_sn_hash: "hash_a".to_string(),
        })
    }

    #[test]
    fn ensure_binding_success_uses_existing_user_and_stores_secret() {
        let store = MemoryLlmApiBindingStore::default();
        let credentials = MemoryCredentialStore::default();
        let adapter = MockLlmApiHubAdapter::default();
        let result = ensure_binding(&store, &credentials, &adapter, &identity()).unwrap();
        assert_eq!(result.status, ProvisioningStatus::Ready);
        assert!(result.created);

        let binding = store.get_binding("u_1", "dev_a").unwrap().unwrap();
        assert_eq!(binding.provisioning_status, ProvisioningStatus::Ready);
        assert_eq!(binding.newapi_user_id.as_deref(), Some("newapi-user-1"));
        assert_eq!(binding.newapi_token_id.as_deref(), Some("newapi-token-1"));
        let reference = binding.token_credential_ref.unwrap();
        assert_eq!(
            credentials.get(&reference).unwrap().as_deref(),
            Some("sk-mock-token-123456789")
        );
    }

    #[test]
    fn ensure_binding_is_idempotent_when_ready() {
        let store = MemoryLlmApiBindingStore::default();
        let credentials = MemoryCredentialStore::default();
        let adapter = MockLlmApiHubAdapter::default();
        ensure_binding(&store, &credentials, &adapter, &identity()).unwrap();
        let result = ensure_binding(&store, &credentials, &adapter, &identity()).unwrap();
        assert!(!result.created);
        assert_eq!(store.list_bindings().unwrap().len(), 1);
    }

    #[test]
    fn token_failure_keeps_retryable_failed_state_after_user_lookup() {
        let store = MemoryLlmApiBindingStore::default();
        let credentials = MemoryCredentialStore::default();
        let adapter = MockLlmApiHubAdapter {
            fail_create_token: Some(LlmApiError::new(
                LlmApiErrorCode::ServiceUnreachable,
                "service down",
                true,
            )),
            ..Default::default()
        };
        let err = ensure_binding(&store, &credentials, &adapter, &identity()).unwrap_err();
        assert_eq!(err.code, LlmApiErrorCode::ServiceUnreachable);
        let binding = store.get_binding("u_1", "dev_a").unwrap().unwrap();
        assert_eq!(binding.provisioning_status, ProvisioningStatus::Failed);
        assert_eq!(binding.newapi_user_id.as_deref(), Some("newapi-user-1"));
        assert!(binding.token_credential_ref.is_none());
    }

    #[test]
    fn missing_backend_user_fails_without_creating_token() {
        let store = MemoryLlmApiBindingStore::default();
        let credentials = MemoryCredentialStore::default();
        let adapter = MockLlmApiHubAdapter {
            fail_lookup_user: Some(LlmApiError::new(
                LlmApiErrorCode::UserNotFound,
                "backend user not found",
                false,
            )),
            ..Default::default()
        };

        let err = ensure_binding(&store, &credentials, &adapter, &identity()).unwrap_err();
        assert_eq!(err.code, LlmApiErrorCode::UserNotFound);
        let binding = store.get_binding("u_1", "dev_a").unwrap().unwrap();
        assert_eq!(binding.provisioning_status, ProvisioningStatus::Failed);
        assert!(binding.newapi_user_id.is_none());
        assert!(binding.newapi_token_id.is_none());
        assert!(binding.token_credential_ref.is_none());
    }

    #[test]
    fn identity_error_does_not_call_adapter() {
        #[derive(Debug)]
        struct FailingIdentity;
        impl IdentityResolver for FailingIdentity {
            fn resolve_identity(&self) -> Result<LlmApiIdentity, LlmApiError> {
                Err(LlmApiError::new(
                    LlmApiErrorCode::DeviceNotBound,
                    "not bound",
                    false,
                ))
            }
        }

        let store = MemoryLlmApiBindingStore::default();
        let credentials = MemoryCredentialStore::default();
        let adapter = MockLlmApiHubAdapter::default();
        let err = ensure_binding(&store, &credentials, &adapter, &FailingIdentity).unwrap_err();
        assert_eq!(err.code, LlmApiErrorCode::DeviceNotBound);
        assert!(store.list_bindings().unwrap().is_empty());
    }

    #[test]
    fn generated_device_password_uses_short_sn_hash_prefix() {
        let identity = LlmApiIdentity {
            pinvou_user_id: "dev_712e51900a17f79f".to_string(),
            device_binding_id: "dev_712e51900a17f79f".to_string(),
            bios_sn_hash: "712e51900a17f79f3599c12f1c67e53926d194c25adb023072424994e98a3ba6"
                .to_string(),
        };

        let password = generated_device_password(&identity).unwrap();
        assert_eq!(password, "Pv3-712e51900a17");
        assert!((8..=20).contains(&password.len()));
    }

    #[test]
    fn ready_model_config_uses_openai_compatible_hub() {
        let store = MemoryLlmApiBindingStore::default();
        let credentials = MemoryCredentialStore::default();
        let adapter = MockLlmApiHubAdapter::default();
        ensure_binding(&store, &credentials, &adapter, &identity()).unwrap();

        let config = ready_model_config(&store, &credentials, &identity()).unwrap();
        assert_eq!(config.provider, "openai");
        assert_eq!(config.base_url, crate::llmapi_hub::DEFAULT_CHAT_BASE_URL);
        assert_eq!(config.model, crate::llmapi_hub::DEFAULT_MODEL);
    }

    #[test]
    fn ready_saved_model_reuses_existing_saved_model_path() {
        let store = MemoryLlmApiBindingStore::default();
        let credentials = MemoryCredentialStore::default();
        let adapter = MockLlmApiHubAdapter::default();
        ensure_binding(&store, &credentials, &adapter, &identity()).unwrap();

        let model = ready_saved_model(&store, &credentials, &identity()).unwrap();
        assert_eq!(
            model.preset,
            crate::bridge::prefs::ModelPreset::OpenaiCompatible
        );
        assert_eq!(model.base_url, crate::llmapi_hub::DEFAULT_CHAT_BASE_URL);
        assert!(model.credential_ref.is_some());
        assert!(model.api_key.is_empty());
    }

    #[test]
    fn admin_overview_items_are_filterable_without_sensitive_values() {
        let store = MemoryLlmApiBindingStore::default();
        let credentials = MemoryCredentialStore::default();
        let adapter = MockLlmApiHubAdapter::default();
        ensure_binding(&store, &credentials, &adapter, &identity()).unwrap();

        let items = crate::llmapi_hub::store::admin_overview_items(store.list_bindings().unwrap());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].pinvou_user_id, "u_1");
        assert_eq!(items[0].provisioning_status, ProvisioningStatus::Ready);
        let json = serde_json::to_string(&items).unwrap();
        assert!(!json.contains("sk-mock-token"));
        assert!(!json.contains("hash_a"));
    }
}
