use chrono::Utc;
use std::time::Instant;

use crate::credential_store::{CredentialReference, CredentialStore};

use super::adapter::{HttpLlmApiHubAdapter, LlmApiHubAdapter, NewApiTokenUsage};
use super::identity::{IdentityResolver, SystemIdentityResolver};
use super::models::{
    BackendUserState, BuiltinLlmApiModelsResponse, DeviceBindingStatus,
    EnsureLlmApiBindingResponse, LlmApiAdminOverviewResponse, LlmApiBinding, LlmApiError,
    LlmApiErrorCode, LlmApiIdentity, LlmApiPolicy, LlmApiStatusResponse, ProvisioningStatus,
    QuotaStatus,
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
    binding.newapi_username = Some(user.username.clone());
    binding.newapi_display_name = user.display_name.clone();
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
            if let Err(err) = ensure_binding_api_key(&store, &credentials, &resolved, &mut binding)
            {
                log::warn!(
                    "[llmapi_hub][provisioning] existing binding api key ensure failed user_id={} code={:?} retryable={} message={}",
                    resolved.pinvou_user_id,
                    err.code,
                    err.retryable,
                    err.message
                );
                if err.code == LlmApiErrorCode::UserNotFound {
                    invalidate_deleted_backend_user(
                        &store,
                        &credentials,
                        &resolved,
                        &mut binding,
                        &err,
                    );
                    return Err(err);
                }
                if let Some(response) =
                    ensure_binding_from_saved_password(&store, &credentials, &resolved)?
                {
                    log::info!(
                        "[llmapi_hub][provisioning] ensure ready from saved password after ready binding failure user_id={} elapsed_ms={}",
                        resolved.pinvou_user_id,
                        started_at.elapsed().as_millis()
                    );
                    return Ok(response);
                }
                return Err(err);
            }
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
    match ensure_binding_from_local_api_key(&store, &credentials, &resolved) {
        Ok(Some(response)) => {
            log::info!(
                "[llmapi_hub][provisioning] ensure ready from local api key user_id={} elapsed_ms={}",
                resolved.pinvou_user_id,
                started_at.elapsed().as_millis()
            );
            return Ok(response);
        }
        Ok(None) => {}
        Err(err) => {
            invalidate_current_binding_if_deleted(&store, &credentials, &resolved, &err);
            return Err(err);
        }
    }
    match ensure_binding_from_user_session(&store, &credentials, &resolved) {
        Ok(Some(response)) => {
            log::info!(
                "[llmapi_hub][provisioning] ensure ready from user session user_id={} elapsed_ms={}",
                resolved.pinvou_user_id,
                started_at.elapsed().as_millis()
            );
            return Ok(response);
        }
        Ok(None) => {}
        Err(err) => {
            log::warn!(
                "[llmapi_hub][provisioning] ensure from user session failed user_id={} code={:?} retryable={} message={}",
                resolved.pinvou_user_id,
                err.code,
                err.retryable,
                err.message
            );
            if err.code == LlmApiErrorCode::UserNotFound {
                invalidate_current_binding_if_deleted(&store, &credentials, &resolved, &err);
                return Err(err);
            }
            if let Some(response) =
                ensure_binding_from_saved_password(&store, &credentials, &resolved)?
            {
                log::info!(
                    "[llmapi_hub][provisioning] ensure ready from saved password after session failure user_id={} elapsed_ms={}",
                    resolved.pinvou_user_id,
                    started_at.elapsed().as_millis()
                );
                return Ok(response);
            }
            return Err(err);
        }
    }
    if let Some(response) = ensure_binding_from_saved_password(&store, &credentials, &resolved)? {
        log::info!(
            "[llmapi_hub][provisioning] ensure ready from saved password user_id={} elapsed_ms={}",
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
    store_user_session(&credentials, &identity, &session)?;
    store_user_password(&credentials, &identity, &password)?;
    if let Some(response) = ensure_binding_from_user_session(&store, &credentials, &identity)? {
        let username = username.trim();
        if !username.is_empty() {
            if let Some(mut binding) =
                store.get_binding(&identity.pinvou_user_id, &identity.device_binding_id)?
            {
                binding.newapi_username = Some(username.to_string());
                store.upsert_binding(binding)?;
            }
        }
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

fn store_user_session<C>(
    credentials: &C,
    identity: &LlmApiIdentity,
    session: &super::adapter::NewApiUserSession,
) -> Result<(), LlmApiError>
where
    C: CredentialStore,
{
    let reference = CredentialReference::for_llmapi_user_session(
        &identity.pinvou_user_id,
        &identity.device_binding_id,
    );
    let payload = serde_json::json!({
        "user_id": session.user_id,
        "access_token": session.access_token,
    });
    credentials.set(&reference, &payload.to_string())?;
    Ok(())
}

fn store_user_password<C>(
    credentials: &C,
    identity: &LlmApiIdentity,
    password: &str,
) -> Result<(), LlmApiError>
where
    C: CredentialStore,
{
    let password = password.trim();
    if password.is_empty() {
        return Ok(());
    }
    let reference = CredentialReference::for_llmapi_user_password(
        &identity.pinvou_user_id,
        &identity.device_binding_id,
    );
    credentials.set(&reference, password)?;
    Ok(())
}

fn refresh_user_session_from_saved_password<C>(
    adapter: &HttpLlmApiHubAdapter,
    credentials: &C,
    identity: &LlmApiIdentity,
) -> Result<Option<super::adapter::NewApiUserSession>, LlmApiError>
where
    C: CredentialStore,
{
    let started_at = Instant::now();
    let reference = CredentialReference::for_llmapi_user_password(
        &identity.pinvou_user_id,
        &identity.device_binding_id,
    );
    let saved_password = credentials
        .get(&reference)?
        .map(|password| password.trim().to_string())
        .filter(|password| !password.is_empty());
    let (password, password_source) = match saved_password {
        Some(password) => (password, "saved password"),
        None => {
            log::warn!(
                "[llmapi_hub][provisioning] user session refresh saved password missing, fallback to generated device password user_id={}",
                identity.pinvou_user_id
            );
            (
                generated_device_password(identity)?,
                "generated device password",
            )
        }
    };

    log::info!(
        "[llmapi_hub][provisioning] user session refresh start user_id={} source={}",
        identity.pinvou_user_id,
        password_source
    );
    let session = adapter
        .login_user_session(&identity.pinvou_user_id, &password)
        .map_err(|err| {
            log::warn!(
                "[llmapi_hub][provisioning] user session refresh failed user_id={} source={} code={:?} retryable={} elapsed_ms={} message={}",
                identity.pinvou_user_id,
                password_source,
                err.code,
                err.retryable,
                started_at.elapsed().as_millis(),
                err.message
            );
            err
        })?;
    store_user_session(credentials, identity, &session)?;
    store_user_password(credentials, identity, &password)?;
    log::info!(
        "[llmapi_hub][provisioning] user session refresh ok user_id={} source={} newapi_user_id={} elapsed_ms={}",
        identity.pinvou_user_id,
        password_source,
        session.user_id,
        started_at.elapsed().as_millis()
    );
    Ok(Some(session))
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
            if err.code == LlmApiErrorCode::UserNotFound {
                return Err(err);
            }
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
    binding.newapi_username = Some(identity.pinvou_user_id.clone());
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
    let adapter = HttpLlmApiHubAdapter::for_token_usage();
    if let Err(err) =
        sync_current_user_profile(&adapter, credentials, &session, identity, &mut binding)
    {
        binding.mark_error(&err);
        store.upsert_binding(binding)?;
        return Err(err);
    }
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

fn ensure_binding_from_saved_password<C>(
    store: &FileLlmApiBindingStore,
    credentials: &C,
    identity: &LlmApiIdentity,
) -> Result<Option<EnsureLlmApiBindingResponse>, LlmApiError>
where
    C: CredentialStore,
{
    let started_at = Instant::now();
    log::info!(
        "[llmapi_hub][provisioning] saved password check start user_id={}",
        identity.pinvou_user_id
    );
    let reference = CredentialReference::for_llmapi_user_password(
        &identity.pinvou_user_id,
        &identity.device_binding_id,
    );
    let saved_password = credentials
        .get(&reference)?
        .map(|password| password.trim().to_string())
        .filter(|password| !password.is_empty());
    let (password, password_source) = match saved_password {
        Some(password) => (password, "saved password"),
        None => {
            log::info!(
                "[llmapi_hub][provisioning] saved password missing, fallback to generated device password user_id={} elapsed_ms={}",
                identity.pinvou_user_id,
                started_at.elapsed().as_millis()
            );
            (
                generated_device_password(identity)?,
                "generated device password",
            )
        }
    };

    let adapter = HttpLlmApiHubAdapter::for_token_usage();
    let session = adapter
        .login_user_session(&identity.pinvou_user_id, &password)
        .map_err(|err| {
            log::warn!(
                "[llmapi_hub][provisioning] password login failed user_id={} source={} code={:?} retryable={} elapsed_ms={} message={}",
                identity.pinvou_user_id,
                password_source,
                err.code,
                err.retryable,
                started_at.elapsed().as_millis(),
                err.message
            );
            err
        })?;
    store_user_session(credentials, identity, &session)?;
    store_user_password(credentials, identity, &password)?;
    log::info!(
        "[llmapi_hub][provisioning] password login ok user_id={} source={} newapi_user_id={} elapsed_ms={}",
        identity.pinvou_user_id,
        password_source,
        session.user_id,
        started_at.elapsed().as_millis()
    );
    ensure_binding_from_user_session(store, credentials, identity)
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
    store_user_session(credentials, identity, &session)?;
    store_user_password(credentials, identity, &password)?;
    log::info!(
        "[llmapi_hub][provisioning] generated device session and password stored user_id={}",
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
        None => status_without_binding(&identity),
    })
}

pub fn status_for_current_user_system() -> Result<LlmApiStatusResponse, LlmApiError> {
    let started_at = Instant::now();
    log::info!("[llmapi_hub][provisioning] status start");
    let store = FileLlmApiBindingStore::default();
    let identity = SystemIdentityResolver.resolve_identity()?;
    log::info!(
        "[llmapi_hub][provisioning] status identity resolved user_id={} device_id={} elapsed_ms={}",
        identity.pinvou_user_id,
        identity.device_binding_id,
        started_at.elapsed().as_millis()
    );
    let ensure_result = ensure_binding_for_current_user();
    log::info!(
        "[llmapi_hub][provisioning] status ensure returned user_id={} ok={} elapsed_ms={}",
        identity.pinvou_user_id,
        ensure_result.is_ok(),
        started_at.elapsed().as_millis()
    );
    let status = match store.get_binding(&identity.pinvou_user_id, &identity.device_binding_id)? {
        Some(binding) => {
            log::info!(
                "[llmapi_hub][provisioning] status binding loaded user_id={} newapi_user_id={} token_id={} binding_status={:?} stored_username={} stored_display_name={} limit={} used={} remaining={}",
                identity.pinvou_user_id,
                binding.newapi_user_id.as_deref().unwrap_or(""),
                binding.newapi_token_id.as_deref().unwrap_or(""),
                binding.provisioning_status,
                binding.newapi_username.as_deref().unwrap_or(""),
                binding.newapi_display_name.as_deref().unwrap_or(""),
                binding.usage.limit_tokens,
                binding.usage.used_tokens,
                binding.usage.remaining_tokens
            );
            let mut status = status_from_binding(&binding);
            if let Err(err) = ensure_result {
                status.auto_login_failed = true;
                status = status_after_refresh_failure(status, Some(err.code), Some(err.message));
            }
            status
        }
        None => {
            let mut status = status_without_binding(&identity);
            if let Err(err) = ensure_result {
                status = status_after_refresh_failure(status, Some(err.code), Some(err.message));
            }
            status
        }
    };
    log::info!(
        "[llmapi_hub][provisioning] status ok user_id={} backend_user_exists={} status={:?} backend_username={} backend_display_name={} limit={} used={} remaining={} elapsed_ms={}",
        identity.pinvou_user_id,
        status.backend_user_exists,
        status.provisioning_status,
        status.backend_username.as_deref().unwrap_or(""),
        status.backend_display_name.as_deref().unwrap_or(""),
        status.quota.as_ref().map(|q| q.limit_tokens).unwrap_or(0),
        status.quota.as_ref().map(|q| q.used_tokens).unwrap_or(0),
        status.quota.as_ref().map(|q| q.remaining_tokens).unwrap_or(0),
        started_at.elapsed().as_millis()
    );
    Ok(status)
}

pub fn local_status_for_current_user_system() -> Result<LlmApiStatusResponse, LlmApiError> {
    let store = FileLlmApiBindingStore::default();
    let identity = SystemIdentityResolver.resolve_identity()?;
    match store.get_binding(&identity.pinvou_user_id, &identity.device_binding_id)? {
        Some(binding) => Ok(status_from_binding(&binding)),
        None => Ok(status_without_binding(&identity)),
    }
}

fn invalidate_current_binding_if_deleted<C>(
    store: &FileLlmApiBindingStore,
    credentials: &C,
    identity: &LlmApiIdentity,
    err: &LlmApiError,
) where
    C: CredentialStore,
{
    if err.code != LlmApiErrorCode::UserNotFound {
        return;
    }
    match store.get_binding(&identity.pinvou_user_id, &identity.device_binding_id) {
        Ok(Some(mut binding)) => {
            invalidate_deleted_backend_user(store, credentials, identity, &mut binding, err)
        }
        Ok(None) => clear_deleted_backend_user_artifacts(credentials, identity, None),
        Err(store_err) => log::warn!(
            "[llmapi_hub][provisioning] deleted backend user binding lookup failed user_id={} code={:?} message={}",
            identity.pinvou_user_id,
            store_err.code,
            store_err.message
        ),
    }
}

fn invalidate_deleted_backend_user<C>(
    store: &FileLlmApiBindingStore,
    credentials: &C,
    identity: &LlmApiIdentity,
    binding: &mut LlmApiBinding,
    err: &LlmApiError,
) where
    C: CredentialStore,
{
    let token_reference = reset_binding_for_deleted_backend_user(binding, err);

    if let Err(store_err) = store.upsert_binding(binding.clone()) {
        log::warn!(
            "[llmapi_hub][provisioning] deleted backend user binding reset failed user_id={} code={:?} message={}",
            identity.pinvou_user_id,
            store_err.code,
            store_err.message
        );
    }
    clear_deleted_backend_user_artifacts(credentials, identity, token_reference);
}

fn reset_binding_for_deleted_backend_user(
    binding: &mut LlmApiBinding,
    err: &LlmApiError,
) -> Option<CredentialReference> {
    let token_reference = binding.token_credential_ref.clone();
    binding.newapi_user_id = None;
    binding.newapi_username = None;
    binding.newapi_display_name = None;
    binding.newapi_token_id = None;
    binding.token_credential_ref = None;
    binding.policy.allowed_models.clear();
    binding.usage = super::models::LlmUsageSnapshot::new(
        super::models::current_period(),
        binding.policy.quota_limit_tokens,
    );
    binding.provisioning_status = ProvisioningStatus::NotStarted;
    binding.last_error_code = Some(LlmApiErrorCode::UserNotFound);
    binding.last_error_message = Some(err.message.clone());
    binding.updated_at = Utc::now();
    token_reference
}

fn clear_deleted_backend_user_artifacts<C>(
    credentials: &C,
    identity: &LlmApiIdentity,
    token_reference: Option<CredentialReference>,
) where
    C: CredentialStore,
{
    let canonical_token = CredentialReference::for_llmapi_token(
        &identity.pinvou_user_id,
        &identity.device_binding_id,
    );
    let mut references = vec![
        canonical_token,
        CredentialReference::for_llmapi_user_session(
            &identity.pinvou_user_id,
            &identity.device_binding_id,
        ),
        CredentialReference::for_llmapi_user_password(
            &identity.pinvou_user_id,
            &identity.device_binding_id,
        ),
    ];
    if let Some(reference) = token_reference {
        if !references.contains(&reference) {
            references.push(reference);
        }
    }
    for reference in references {
        if let Err(credential_err) = credentials.delete(&reference) {
            log::warn!(
                "[llmapi_hub][provisioning] deleted backend user credential cleanup failed user_id={} service={} account={} message={}",
                identity.pinvou_user_id,
                reference.service,
                reference.account,
                credential_err.user_message()
            );
        }
    }

    let mut prefs = crate::bridge::prefs::UserPrefs::load();
    prefs.advanced.builtin_llmapi_available_models.clear();
    prefs.advanced.builtin_llmapi_default_model = None;
    prefs.ensure_builtin_llmapi_model();
    if let Err(save_err) = prefs.save() {
        log::warn!(
            "[llmapi_hub][provisioning] deleted backend user model cache cleanup failed user_id={} message={}",
            identity.pinvou_user_id,
            save_err
        );
    }
}

pub fn unavailable_status_for_current_user_system(
    code: Option<LlmApiErrorCode>,
    message: Option<String>,
) -> Result<LlmApiStatusResponse, LlmApiError> {
    let status = local_status_for_current_user_system()?;
    Ok(status_after_refresh_failure(status, code, message))
}

pub fn available_models_system() -> Result<BuiltinLlmApiModelsResponse, LlmApiError> {
    let started_at = Instant::now();
    log::info!("[llmapi_hub][provisioning] available models system start");
    let cached = local_builtin_models_response();
    if let Err(err) = ensure_binding_for_current_user() {
        if err.code != LlmApiErrorCode::UserNotFound && !cached.available_models.is_empty() {
            log::warn!(
                "[llmapi_hub][provisioning] available models refresh failed; using cached models elapsed_ms={} count={} code={:?} retryable={} message={}",
                started_at.elapsed().as_millis(),
                cached.available_models.len(),
                err.code,
                err.retryable,
                err.message
            );
            return Ok(cached);
        }
        return Err(err);
    }
    let response = local_builtin_models_response();
    log::info!(
        "[llmapi_hub][provisioning] available models system ok elapsed_ms={} count={} default_model={}",
        started_at.elapsed().as_millis(),
        response.available_models.len(),
        response.default_model
    );
    Ok(response)
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

fn apply_token_usage(binding: &mut LlmApiBinding, usage: NewApiTokenUsage) {
    binding.usage.period = super::models::current_period();
    binding.usage.limit_tokens = usage.total_granted;
    binding.usage.used_tokens = usage.total_used;
    binding.usage.remaining_tokens = usage.total_available;
    binding.usage.last_synced_at = Some(Utc::now());
    binding.updated_at = Utc::now();
}

fn apply_user_self_quota(binding: &mut LlmApiBinding, remaining_quota: u64, used_quota: u64) {
    binding.usage.period = super::models::current_period();
    binding.usage.limit_tokens = remaining_quota.saturating_add(used_quota);
    binding.usage.used_tokens = used_quota;
    binding.usage.remaining_tokens = remaining_quota;
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

fn apply_current_user_profile(
    user: super::adapter::NewApiUser,
    identity: &LlmApiIdentity,
    binding: &mut LlmApiBinding,
) {
    let incoming_user_id = user.id;
    let incoming_username = user.username;
    let incoming_display_name = user
        .display_name
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let incoming_quota = user.quota;
    let incoming_used_quota = user.used_quota;
    log::info!(
        "[llmapi_hub][provisioning] current user profile received pinvou_user_id={} newapi_user_id={} username={} display_name={} quota={:?} used_quota={:?}",
        identity.pinvou_user_id,
        incoming_user_id,
        incoming_username,
        incoming_display_name.as_deref().unwrap_or(""),
        incoming_quota,
        incoming_used_quota
    );
    binding.newapi_user_id = Some(incoming_user_id);
    binding.newapi_username = Some(incoming_username);
    binding.newapi_display_name = incoming_display_name;
    if let (Some(remaining_quota), Some(used_quota)) = (incoming_quota, incoming_used_quota) {
        apply_user_self_quota(binding, remaining_quota, used_quota);
    }
    binding.updated_at = Utc::now();
    log::info!(
        "[llmapi_hub][provisioning] current user profile synced user_id={} stored_username={} stored_display_name={} has_quota={} limit={} used={} remaining={}",
        identity.pinvou_user_id,
        binding.newapi_username.as_deref().unwrap_or(""),
        binding.newapi_display_name.as_deref().unwrap_or(""),
        incoming_quota.is_some() && incoming_used_quota.is_some(),
        binding.usage.limit_tokens,
        binding.usage.used_tokens,
        binding.usage.remaining_tokens
    );
}

fn sync_current_user_profile<C>(
    adapter: &HttpLlmApiHubAdapter,
    credentials: &C,
    session: &super::adapter::NewApiUserSession,
    identity: &LlmApiIdentity,
    binding: &mut LlmApiBinding,
) -> Result<(), LlmApiError>
where
    C: CredentialStore,
{
    match adapter.current_user(session) {
        Ok(user) => {
            apply_current_user_profile(user, identity, binding);
            Ok(())
        }
        Err(err) => {
            log::warn!(
                "[llmapi_hub][provisioning] current user profile sync failed user_id={} code={:?} retryable={} message={}",
                identity.pinvou_user_id,
                err.code,
                err.retryable,
                err.message
            );
            if err.code == LlmApiErrorCode::PermissionDenied {
                match refresh_user_session_from_saved_password(adapter, credentials, identity) {
                    Ok(Some(refreshed_session)) => match adapter.current_user(&refreshed_session) {
                        Ok(user) => {
                            log::info!(
                                "[llmapi_hub][provisioning] current user profile retry after session refresh ok user_id={}",
                                identity.pinvou_user_id
                            );
                            apply_current_user_profile(user, identity, binding);
                            return Ok(());
                        }
                        Err(retry_err) => {
                            log::warn!(
                                "[llmapi_hub][provisioning] current user profile retry after session refresh failed user_id={} code={:?} retryable={} message={}",
                                identity.pinvou_user_id,
                                retry_err.code,
                                retry_err.retryable,
                                retry_err.message
                            );
                            return Err(retry_err);
                        }
                    },
                    Ok(None) => {}
                    Err(refresh_err) => {
                        let refresh_err = classify_managed_device_account_login_error(
                            refresh_err,
                            identity,
                            binding,
                        );
                        log::warn!(
                            "[llmapi_hub][provisioning] current user profile session refresh failed user_id={} code={:?} retryable={} message={}",
                            identity.pinvou_user_id,
                            refresh_err.code,
                            refresh_err.retryable,
                            refresh_err.message
                        );
                        return Err(refresh_err);
                    }
                }
            }
            Err(err)
        }
    }
}

fn classify_managed_device_account_login_error(
    err: LlmApiError,
    identity: &LlmApiIdentity,
    binding: &LlmApiBinding,
) -> LlmApiError {
    let is_managed_device_account =
        binding.newapi_username.as_deref() == Some(identity.pinvou_user_id.as_str());
    let message = err.message.to_ascii_lowercase();
    let login_says_account_unavailable = message.contains("password is incorrect")
        || message.contains("user has been banned")
        || message.contains("用户名或密码错误")
        || message.contains("用户已被封禁");
    if is_managed_device_account && login_says_account_unavailable {
        return LlmApiError::new(
            LlmApiErrorCode::UserNotFound,
            "Backend-managed device account no longer accepts its fixed credentials",
            false,
        );
    }
    err
}

fn should_refresh_api_key_after_usage_error(err: &LlmApiError) -> bool {
    matches!(err.code, LlmApiErrorCode::PermissionDenied)
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
        if let Some(session) =
            HttpLlmApiHubAdapter::user_session_from_credentials(credentials, identity)?
        {
            binding.token_credential_ref = Some(reference);
            sync_current_user_profile(&adapter, credentials, &session, identity, binding)?;
            sync_available_models(&adapter, &token, identity, binding);
            binding.clear_error();
            store.upsert_binding(binding.clone())?;
            log::info!(
                "[llmapi_hub][provisioning] api key ensured from current user profile user_id={} model={} used={} remaining={} elapsed_ms={}",
                identity.pinvou_user_id,
                binding.policy.allowed_models.first().map(String::as_str).unwrap_or(""),
                binding.usage.used_tokens,
                binding.usage.remaining_tokens,
                started_at.elapsed().as_millis()
            );
            return Ok(());
        }
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
                if err.code == LlmApiErrorCode::UserNotFound {
                    return Err(err);
                }
                if !should_refresh_api_key_after_usage_error(&err) {
                    binding.token_credential_ref = Some(reference);
                    binding.updated_at = Utc::now();
                    store.upsert_binding(binding.clone())?;
                    log::warn!(
                        "[llmapi_hub][provisioning] keep local api key after usage failure user_id={} code={:?} elapsed_ms={}",
                        identity.pinvou_user_id,
                        err.code,
                        started_at.elapsed().as_millis()
                    );
                    return Ok(());
                }
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

    sync_current_user_profile(&adapter, credentials, &session, identity, binding)?;
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
        context_window_tokens: None,
        max_output_tokens: None,
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
        context_window_tokens: None,
        max_output_tokens: None,
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

fn status_without_binding(identity: &LlmApiIdentity) -> LlmApiStatusResponse {
    LlmApiStatusResponse {
        pinvou_user_id: Some(identity.pinvou_user_id.clone()),
        backend_user_exists: false,
        backend_user_state: BackendUserState::NotExists,
        stale: false,
        backend_username: None,
        backend_display_name: None,
        auto_login_failed: false,
        device_binding_status: DeviceBindingStatus::Bound,
        enabled: true,
        provisioning_status: ProvisioningStatus::NotStarted,
        quota: None,
        last_call_status: None,
        last_error_code: None,
        last_error_message: None,
    }
}

fn status_from_binding(binding: &LlmApiBinding) -> LlmApiStatusResponse {
    let authoritative_unavailable = matches!(
        binding.last_error_code,
        Some(LlmApiErrorCode::UserNotFound | LlmApiErrorCode::ServiceDisabled)
    );
    let backend_user_exists = !authoritative_unavailable
        && (binding.newapi_user_id.is_some()
            || binding.provisioning_status == ProvisioningStatus::Ready);
    let backend_username = backend_user_exists.then(|| {
        binding
            .newapi_username
            .clone()
            .unwrap_or_else(|| binding.pinvou_user_id.clone())
    });
    let backend_display_name = backend_user_exists
        .then(|| binding.newapi_display_name.clone())
        .flatten();
    log::info!(
        "[llmapi_hub][provisioning] status_from_binding user_id={} backend_username={} backend_display_name={} binding_display_name={} limit={} used={} remaining={}",
        binding.pinvou_user_id,
        backend_username.as_deref().unwrap_or(""),
        backend_display_name.as_deref().unwrap_or(""),
        binding.newapi_display_name.as_deref().unwrap_or(""),
        binding.usage.limit_tokens,
        binding.usage.used_tokens,
        binding.usage.remaining_tokens
    );
    let backend_user_state = if authoritative_unavailable {
        BackendUserState::NotExists
    } else if backend_user_exists {
        BackendUserState::Exists
    } else {
        BackendUserState::Unknown
    };
    LlmApiStatusResponse {
        pinvou_user_id: Some(binding.pinvou_user_id.clone()),
        backend_user_exists,
        backend_user_state,
        stale: false,
        backend_username,
        backend_display_name,
        auto_login_failed: false,
        device_binding_status: DeviceBindingStatus::Bound,
        enabled: binding.enabled,
        provisioning_status: binding.provisioning_status,
        quota: backend_user_exists.then(|| QuotaStatus::from(&binding.usage)),
        last_call_status: None,
        last_error_code: binding.last_error_code,
        last_error_message: binding.last_error_message.clone(),
    }
}

fn status_after_refresh_failure(
    mut status: LlmApiStatusResponse,
    code: Option<LlmApiErrorCode>,
    message: Option<String>,
) -> LlmApiStatusResponse {
    let authoritative_unavailable = matches!(
        code,
        Some(LlmApiErrorCode::UserNotFound | LlmApiErrorCode::ServiceDisabled)
    );
    status.backend_user_state = if authoritative_unavailable {
        status.backend_user_exists = false;
        status.backend_username = None;
        status.backend_display_name = None;
        status.quota = None;
        BackendUserState::NotExists
    } else if status.backend_user_exists {
        BackendUserState::Exists
    } else {
        BackendUserState::Unknown
    };
    status.stale = !authoritative_unavailable;
    status.last_error_code = code;
    status.last_error_message = message;
    status
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
    fn transient_status_failure_preserves_known_existing_account() {
        let mut binding = LlmApiBinding::new(&identity().0, LlmApiPolicy::default());
        binding.newapi_user_id = Some("newapi-user-1".to_string());
        binding.provisioning_status = ProvisioningStatus::Ready;

        let status = status_after_refresh_failure(
            status_from_binding(&binding),
            Some(LlmApiErrorCode::ServiceUnreachable),
            Some("service down".to_string()),
        );

        assert!(status.backend_user_exists);
        assert_eq!(status.backend_user_state, BackendUserState::Exists);
        assert!(status.stale);
        assert_eq!(
            status.last_error_code,
            Some(LlmApiErrorCode::ServiceUnreachable)
        );
    }

    #[test]
    fn transient_status_failure_is_unknown_not_missing_without_cache() {
        let status = status_after_refresh_failure(
            status_without_binding(&identity().0),
            Some(LlmApiErrorCode::ServiceUnreachable),
            Some("timeout".to_string()),
        );

        assert!(!status.backend_user_exists);
        assert_eq!(status.backend_user_state, BackendUserState::Unknown);
        assert!(status.stale);
    }

    #[test]
    fn authoritative_user_not_found_remains_not_exists() {
        let mut binding = LlmApiBinding::new(&identity().0, LlmApiPolicy::default());
        binding.newapi_user_id = Some("newapi-user-1".to_string());
        binding.newapi_username = Some("u_1".to_string());
        binding.newapi_display_name = Some("User One".to_string());
        binding.provisioning_status = ProvisioningStatus::Ready;

        let status = status_after_refresh_failure(
            status_from_binding(&binding),
            Some(LlmApiErrorCode::UserNotFound),
            Some("not found".to_string()),
        );

        assert!(!status.backend_user_exists);
        assert_eq!(status.backend_user_state, BackendUserState::NotExists);
        assert!(!status.stale);
        assert!(status.backend_username.is_none());
        assert!(status.backend_display_name.is_none());
        assert!(status.quota.is_none());
    }

    #[test]
    fn authoritative_disabled_user_is_unavailable_even_with_cached_binding() {
        let mut binding = LlmApiBinding::new(&identity().0, LlmApiPolicy::default());
        binding.newapi_user_id = Some("newapi-user-1".to_string());
        binding.newapi_username = Some("u_1".to_string());
        binding.newapi_display_name = Some("User One".to_string());
        binding.provisioning_status = ProvisioningStatus::Ready;

        let status = status_after_refresh_failure(
            status_from_binding(&binding),
            Some(LlmApiErrorCode::ServiceDisabled),
            Some("disabled".to_string()),
        );

        assert!(!status.backend_user_exists);
        assert_eq!(status.backend_user_state, BackendUserState::NotExists);
        assert!(!status.stale);
        assert!(status.backend_username.is_none());
        assert!(status.backend_display_name.is_none());
        assert!(status.quota.is_none());
    }

    #[test]
    fn cached_disabled_state_is_also_unavailable_without_refresh() {
        let mut binding = LlmApiBinding::new(&identity().0, LlmApiPolicy::default());
        binding.newapi_user_id = Some("newapi-user-1".to_string());
        binding.provisioning_status = ProvisioningStatus::Ready;
        binding.last_error_code = Some(LlmApiErrorCode::ServiceDisabled);

        let status = status_from_binding(&binding);

        assert!(!status.backend_user_exists);
        assert_eq!(status.backend_user_state, BackendUserState::NotExists);
        assert!(status.quota.is_none());
    }

    #[test]
    fn deleted_backend_user_clears_binding_identity_token_and_quota() {
        let identity = identity().0;
        let mut binding = LlmApiBinding::new(&identity, LlmApiPolicy::default());
        let token_reference = CredentialReference::for_llmapi_token(
            &identity.pinvou_user_id,
            &identity.device_binding_id,
        );
        binding.newapi_user_id = Some("newapi-user-1".to_string());
        binding.newapi_username = Some("u_1".to_string());
        binding.newapi_display_name = Some("User One".to_string());
        binding.newapi_token_id = Some("token-1".to_string());
        binding.token_credential_ref = Some(token_reference.clone());
        binding.policy.allowed_models = vec!["deepseek-v4-flash".to_string()];
        binding.usage.used_tokens = 400;
        binding.usage.remaining_tokens = 600;
        binding.usage.last_synced_at = Some(Utc::now());
        binding.provisioning_status = ProvisioningStatus::Ready;
        let err = LlmApiError::new(LlmApiErrorCode::UserNotFound, "backend user deleted", false);

        let removed_reference = reset_binding_for_deleted_backend_user(&mut binding, &err);

        assert_eq!(removed_reference, Some(token_reference));
        assert!(binding.newapi_user_id.is_none());
        assert!(binding.newapi_username.is_none());
        assert!(binding.newapi_display_name.is_none());
        assert!(binding.newapi_token_id.is_none());
        assert!(binding.token_credential_ref.is_none());
        assert!(binding.policy.allowed_models.is_empty());
        assert_eq!(binding.provisioning_status, ProvisioningStatus::NotStarted);
        assert_eq!(binding.usage.used_tokens, 0);
        assert_eq!(binding.usage.last_synced_at, None);
        assert_eq!(binding.last_error_code, Some(LlmApiErrorCode::UserNotFound));

        let status = status_from_binding(&binding);
        assert_eq!(status.backend_user_state, BackendUserState::NotExists);
        assert!(!status.backend_user_exists);
        assert!(status.quota.is_none());
    }

    #[test]
    fn managed_device_login_rejection_is_authoritative_but_manual_account_is_not() {
        let identity = identity().0;
        let mut managed_binding = LlmApiBinding::new(&identity, LlmApiPolicy::default());
        managed_binding.newapi_username = Some(identity.pinvou_user_id.clone());
        let login_error = || {
            LlmApiError::new(
                LlmApiErrorCode::ProvisioningFailed,
                "Username or password is incorrect, or user has been banned",
                true,
            )
        };

        let classified =
            classify_managed_device_account_login_error(login_error(), &identity, &managed_binding);
        assert_eq!(classified.code, LlmApiErrorCode::UserNotFound);

        let mut manual_binding = managed_binding;
        manual_binding.newapi_username = Some("manual-user".to_string());
        let unchanged =
            classify_managed_device_account_login_error(login_error(), &identity, &manual_binding);
        assert_eq!(unchanged.code, LlmApiErrorCode::ProvisioningFailed);
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
