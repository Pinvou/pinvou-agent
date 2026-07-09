use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::time::{Duration, Instant};

use super::models::{LlmApiError, LlmApiErrorCode, LlmApiIdentity, LlmApiPolicy};
use crate::credential_store::{CredentialReference, CredentialStore, SystemCredentialStore};

pub const LOOKUP_USER_ENDPOINT_ENV: &str = "PINVOU3_LLMAPI_LOOKUP_USER_ENDPOINT";
pub const CREATE_TOKEN_ENDPOINT_ENV: &str = "PINVOU3_LLMAPI_CREATE_TOKEN_ENDPOINT";
pub const CONFIGURE_POLICY_ENDPOINT_ENV: &str = "PINVOU3_LLMAPI_CONFIGURE_POLICY_ENDPOINT";
pub const ADMIN_BASE_URL_ENV: &str = "PINVOU3_LLMAPI_ADMIN_BASE_URL";
pub const ADMIN_USER_ID_ENV: &str = "PINVOU3_LLMAPI_ADMIN_USER_ID";
pub const USER_SESSION_ENV: &str = "PINVOU3_LLMAPI_USER_SESSION";

const DEFAULT_LOOKUP_USER_PATH: &str = "/api/user/search";
const DEFAULT_TOKEN_USAGE_PATH: &str = "/api/usage/token/";
const DEFAULT_TOKEN_SEARCH_PATH: &str = "/api/token/search";
const DEFAULT_TOKEN_PATH: &str = "/api/token/";
const DEFAULT_USER_LOGIN_PATH: &str = "/api/user/login";
const DEFAULT_USER_ACCESS_TOKEN_PATH: &str = "/api/user/token";
const DEFAULT_USER_SELF_PATH: &str = "/api/user/self";
const DEFAULT_OPENAI_MODELS_PATH: &str = "/v1/models";
const DEFAULT_TOKEN_NAME: &str = "default";
const HTTP_CONNECT_TIMEOUT_SECS: u64 = 5;
const HTTP_REQUEST_TIMEOUT_SECS: u64 = 20;

pub trait LlmApiHubAdapter {
    fn lookup_user(&self, identity: &LlmApiIdentity) -> Result<NewApiUser, LlmApiError>;
    fn create_token(
        &self,
        identity: &LlmApiIdentity,
        newapi_user_id: &str,
    ) -> Result<NewApiToken, LlmApiError>;
    fn configure_policy(
        &self,
        identity: &LlmApiIdentity,
        newapi_user_id: &str,
        token_id: &str,
        policy: &LlmApiPolicy,
    ) -> Result<(), LlmApiError>;
    fn set_token_enabled(&self, _token_id: &str, _enabled: bool) -> Result<(), LlmApiError> {
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NewApiUser {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub quota: Option<u64>,
    pub used_quota: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NewApiToken {
    pub id: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NewApiUserSession {
    pub user_id: String,
    pub access_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NewApiTokenUsage {
    pub total_granted: u64,
    pub total_used: u64,
    pub total_available: u64,
    #[serde(default)]
    pub unlimited_quota: bool,
}

#[derive(Debug, Clone)]
pub struct HttpLlmApiHubAdapter {
    pub admin_base_url: String,
    admin_user_id: String,
    admin_token: String,
    lookup_user_endpoint: Option<String>,
    create_token_endpoint: Option<String>,
    configure_policy_endpoint: Option<String>,
    client: reqwest::blocking::Client,
}

#[derive(Debug, Clone, Deserialize)]
struct AdminCredentialJson {
    user_id: Option<serde_json::Value>,
    access_token: Option<String>,
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NewApiEnvelope<T> {
    success: Option<bool>,
    code: Option<bool>,
    message: Option<String>,
    data: Option<T>,
}

impl<T> NewApiEnvelope<T> {
    fn is_success(&self) -> bool {
        self.success.or(self.code).unwrap_or(false)
    }
}

#[derive(Debug, Deserialize)]
struct NewApiPage<T> {
    items: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct NewApiUserRecord {
    id: serde_json::Value,
    username: String,
    #[serde(default, alias = "displayName", alias = "name", alias = "nickname")]
    display_name: Option<String>,
    #[serde(default)]
    quota: Option<u64>,
    #[serde(default)]
    used_quota: Option<u64>,
    status: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct NewApiLoginUserRecord {
    id: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct NewApiTokenRecord {
    id: serde_json::Value,
    name: String,
    status: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModelRecord>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelRecord {
    id: String,
}

#[derive(Debug, Deserialize)]
struct TokenKeyResponse {
    key: String,
}

#[derive(Debug, Serialize)]
struct LoginRequest<'a> {
    username: &'a str,
    password: &'a str,
}

#[derive(Debug, Serialize)]
struct CustomCreateTokenRequest<'a> {
    pinvou_user_id: &'a str,
    device_binding_id: &'a str,
    newapi_user_id: &'a str,
    name: String,
    expired_time: i64,
    remain_quota: u64,
    unlimited_quota: bool,
    model_limits_enabled: bool,
    model_limits: String,
}

#[derive(Debug, Deserialize)]
struct CustomTokenResponse {
    id: serde_json::Value,
    #[serde(alias = "key")]
    token: String,
}

#[derive(Debug, Serialize)]
struct CustomConfigurePolicyRequest<'a> {
    pinvou_user_id: &'a str,
    device_binding_id: &'a str,
    newapi_user_id: &'a str,
    token_id: &'a str,
    quota_limit_tokens: u64,
    rpm_limit: u32,
    allowed_models: &'a [String],
}

impl HttpLlmApiHubAdapter {
    pub fn from_system_credentials() -> Result<Self, LlmApiError> {
        log::info!("[llmapi_hub][adapter] from_system_credentials start");
        let credentials = SystemCredentialStore::new();
        let adapter = Self::from_credentials(&credentials)?;
        log::info!("[llmapi_hub][adapter] from_system_credentials ok");
        Ok(adapter)
    }

    pub fn from_credentials(credentials: &impl CredentialStore) -> Result<Self, LlmApiError> {
        let started_at = Instant::now();
        log::info!("[llmapi_hub][adapter] from_credentials start");
        let reference = CredentialReference::for_llmapi_admin();
        log::info!(
            "[llmapi_hub][adapter] admin credential get start service={} account={}",
            reference.service,
            reference.account
        );
        let raw_admin_credential = credentials
            .get(&reference)
            .map_err(|err| {
                LlmApiError::new(
                    LlmApiErrorCode::AdminCredentialMissing,
                    err.user_message(),
                    true,
                )
            })?
            .ok_or_else(|| {
                LlmApiError::new(
                    LlmApiErrorCode::AdminCredentialMissing,
                    "Missing New API admin credential; cannot query existing backend account",
                    false,
                )
            })?;
        log::info!(
            "[llmapi_hub][adapter] admin credential get ok elapsed_ms={}",
            started_at.elapsed().as_millis()
        );
        let (credential_user_id, admin_token) = parse_admin_credential(&raw_admin_credential)?;
        let admin_user_id = credential_user_id
            .or_else(|| {
                std::env::var(ADMIN_USER_ID_ENV)
                    .ok()
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
            })
            .ok_or_else(|| {
                LlmApiError::new(
                    LlmApiErrorCode::AdminCredentialMissing,
                    format!(
                        "Missing New API admin user id; set {ADMIN_USER_ID_ENV} or store JSON {{\"user_id\":...,\"access_token\":\"...\"}}"
                    ),
                    false,
                )
            })?;

        log::info!(
            "[llmapi_hub][adapter] from_credentials build adapter start elapsed_ms={}",
            started_at.elapsed().as_millis()
        );
        let adapter = Self {
            admin_base_url: std::env::var(ADMIN_BASE_URL_ENV)
                .unwrap_or_else(|_| crate::llmapi_hub::DEFAULT_ADMIN_BASE_URL.to_string()),
            admin_user_id,
            admin_token,
            lookup_user_endpoint: std::env::var(LOOKUP_USER_ENDPOINT_ENV).ok(),
            create_token_endpoint: std::env::var(CREATE_TOKEN_ENDPOINT_ENV).ok(),
            configure_policy_endpoint: std::env::var(CONFIGURE_POLICY_ENDPOINT_ENV).ok(),
            client: blocking_client(),
        };
        log::info!(
            "[llmapi_hub][adapter] from_credentials ok elapsed_ms={}",
            started_at.elapsed().as_millis()
        );
        Ok(adapter)
    }

    pub fn for_token_usage() -> Self {
        let started_at = Instant::now();
        log::info!("[llmapi_hub][adapter] for_token_usage start");
        let adapter = Self {
            admin_base_url: std::env::var(ADMIN_BASE_URL_ENV)
                .unwrap_or_else(|_| crate::llmapi_hub::DEFAULT_ADMIN_BASE_URL.to_string()),
            admin_user_id: String::new(),
            admin_token: String::new(),
            lookup_user_endpoint: None,
            create_token_endpoint: None,
            configure_policy_endpoint: None,
            client: blocking_client(),
        };
        log::info!(
            "[llmapi_hub][adapter] for_token_usage ok elapsed_ms={}",
            started_at.elapsed().as_millis()
        );
        adapter
    }

    pub fn authorization_header_value(&self) -> String {
        format!(
            "Bearer {}",
            self.admin_token.trim_start_matches("Bearer ").trim()
        )
    }

    fn endpoint(&self, override_endpoint: Option<&String>, default_path: &str) -> String {
        if let Some(endpoint) = override_endpoint {
            let endpoint = endpoint.trim();
            if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
                return endpoint.to_string();
            }
            return join_url(&self.admin_base_url, endpoint);
        }
        join_url(&self.admin_base_url, default_path)
    }

    fn auth_request(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        request
            .header(
                reqwest::header::AUTHORIZATION,
                self.authorization_header_value(),
            )
            .header("New-Api-User", self.admin_user_id.as_str())
            .header(reqwest::header::CACHE_CONTROL, "no-store")
    }

    fn token_request(
        &self,
        request: reqwest::blocking::RequestBuilder,
        token: &str,
    ) -> reqwest::blocking::RequestBuilder {
        request
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", token.trim_start_matches("Bearer ").trim()),
            )
            .header(reqwest::header::CACHE_CONTROL, "no-store")
    }

    fn user_request(
        &self,
        request: reqwest::blocking::RequestBuilder,
        session: &NewApiUserSession,
    ) -> reqwest::blocking::RequestBuilder {
        request
            .header(reqwest::header::AUTHORIZATION, session.access_token.trim())
            .header("New-Api-User", session.user_id.as_str())
            .header(reqwest::header::CACHE_CONTROL, "no-store")
    }

    pub fn token_usage(&self, token: &str) -> Result<NewApiTokenUsage, LlmApiError> {
        let started_at = Instant::now();
        log::info!("[llmapi_hub][adapter] token usage start");
        let token = token.trim();
        if token.is_empty() {
            log::warn!("[llmapi_hub][adapter] token usage skipped because token is empty");
            return Err(LlmApiError::new(
                LlmApiErrorCode::PermissionDenied,
                "New API token is empty; cannot query token usage",
                false,
            ));
        }
        let endpoint = self.endpoint(None, DEFAULT_TOKEN_USAGE_PATH);
        log::info!(
            "[llmapi_hub][adapter] token usage request prepared endpoint={} token_len={}",
            endpoint,
            token.len()
        );
        let usage: NewApiTokenUsage = self.send_json(
            self.token_request(self.client.get(endpoint), token),
            "query token usage",
        )?;
        log::info!(
            "[llmapi_hub][adapter] token usage ok elapsed_ms={} used={} remaining={}",
            started_at.elapsed().as_millis(),
            usage.total_used,
            usage.total_available
        );
        Ok(usage)
    }

    pub fn available_models(&self, token: &str) -> Result<Vec<String>, LlmApiError> {
        let started_at = Instant::now();
        log::info!("[llmapi_hub][adapter] available models start");
        let token = token.trim();
        if token.is_empty() {
            log::warn!("[llmapi_hub][adapter] available models skipped because token is empty");
            return Err(LlmApiError::new(
                LlmApiErrorCode::PermissionDenied,
                "New API token is empty; cannot query available models",
                false,
            ));
        }
        let endpoint = self.endpoint(None, DEFAULT_OPENAI_MODELS_PATH);
        log::info!(
            "[llmapi_hub][adapter] available models request prepared endpoint={} token_len={}",
            endpoint,
            token.len()
        );
        let response = self
            .token_request(self.client.get(endpoint.clone()), token)
            .send()
            .map_err(|err| {
                log::warn!(
                    "[llmapi_hub][adapter] available models request send failed endpoint={} elapsed_ms={} error={}",
                    endpoint,
                    started_at.elapsed().as_millis(),
                    err
                );
                LlmApiError::new(
                    LlmApiErrorCode::ServiceUnreachable,
                    format!("New API available models request failed: {err}"),
                    true,
                )
            })?;
        let status = response.status();
        let body = response.text().map_err(|err| {
            log::warn!(
                "[llmapi_hub][adapter] available models response read failed status={} elapsed_ms={} error={}",
                status.as_u16(),
                started_at.elapsed().as_millis(),
                err
            );
            LlmApiError::new(
                LlmApiErrorCode::ServiceUnreachable,
                format!("New API available models response read failed: {err}"),
                true,
            )
        })?;
        log::info!(
            "[llmapi_hub][adapter] available models response received endpoint={} status={} body_len={} elapsed_ms={}",
            endpoint,
            status.as_u16(),
            body.len(),
            started_at.elapsed().as_millis()
        );
        if !status.is_success() {
            return Err(http_error("available models", status.as_u16(), &body));
        }
        let parsed: OpenAiModelsResponse = serde_json::from_str(&body).map_err(|err| {
            log::warn!(
                "[llmapi_hub][adapter] available models parse failed endpoint={} status={} body_snippet={} error={}",
                endpoint,
                status.as_u16(),
                compact_body(&body),
                err
            );
            LlmApiError::new(
                LlmApiErrorCode::ProvisioningFailed,
                format!("New API available models response format is invalid: {err}"),
                true,
            )
        })?;
        let mut models = parsed
            .data
            .into_iter()
            .map(|model| model.id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect::<Vec<_>>();
        models.sort();
        models.dedup();
        if models.is_empty() {
            return Err(LlmApiError::new(
                LlmApiErrorCode::ProvisioningFailed,
                "New API did not return any available model for current token",
                true,
            ));
        }
        log::info!(
            "[llmapi_hub][adapter] available models ok count={} first={} elapsed_ms={}",
            models.len(),
            models.first().map(String::as_str).unwrap_or(""),
            started_at.elapsed().as_millis()
        );
        Ok(models)
    }

    pub fn user_session_from_credentials(
        credentials: &impl CredentialStore,
        identity: &LlmApiIdentity,
    ) -> Result<Option<NewApiUserSession>, LlmApiError> {
        if let Ok(raw) = std::env::var(USER_SESSION_ENV) {
            let raw = raw.trim();
            if !raw.is_empty() {
                return parse_user_session(raw).map(Some);
            }
        }
        let reference = CredentialReference::for_llmapi_user_session(
            &identity.pinvou_user_id,
            &identity.device_binding_id,
        );
        credentials
            .get(&reference)
            .map_err(|err| {
                LlmApiError::new(
                    LlmApiErrorCode::ProvisioningFailed,
                    err.user_message(),
                    true,
                )
            })?
            .map(|raw| parse_user_session(&raw))
            .transpose()
    }

    pub fn default_token(&self, session: &NewApiUserSession) -> Result<NewApiToken, LlmApiError> {
        let started_at = Instant::now();
        log::info!(
            "[llmapi_hub][adapter] default token start newapi_user_id={}",
            session.user_id
        );
        let existing = self.find_default_token(session)?;
        let token_id = match existing {
            Some(id) => {
                log::info!(
                    "[llmapi_hub][adapter] default token found newapi_user_id={} token_id={}",
                    session.user_id,
                    id
                );
                id
            }
            None => {
                log::info!(
                    "[llmapi_hub][adapter] default token missing, creating newapi_user_id={}",
                    session.user_id
                );
                self.create_default_token(session)?
            }
        };
        let token = self.fetch_token_key(session, &token_id)?;
        log::info!(
            "[llmapi_hub][adapter] default token ok newapi_user_id={} token_id={} elapsed_ms={}",
            session.user_id,
            token_id,
            started_at.elapsed().as_millis()
        );
        Ok(NewApiToken {
            id: token_id,
            token,
        })
    }

    pub fn current_user(&self, session: &NewApiUserSession) -> Result<NewApiUser, LlmApiError> {
        let started_at = Instant::now();
        log::info!(
            "[llmapi_hub][adapter] current user start newapi_user_id={}",
            session.user_id
        );
        let user: NewApiUserRecord = self.send_json(
            self.user_request(self.client.get(self.endpoint(None, DEFAULT_USER_SELF_PATH)), session),
            "query current user",
        )?;
        if user.status.is_some_and(|status| status != 1) {
            return Err(LlmApiError::new(
                LlmApiErrorCode::ServiceDisabled,
                "New API current user is disabled",
                false,
            ));
        }
        let user = NewApiUser {
            id: json_value_to_string(&user.id),
            username: user.username,
            display_name: user.display_name,
            quota: user.quota,
            used_quota: user.used_quota,
        };
        log::info!(
            "[llmapi_hub][adapter] current user ok newapi_user_id={} username={} has_display_name={} elapsed_ms={}",
            user.id,
            user.username,
            user.display_name.as_ref().is_some_and(|v| !v.trim().is_empty()),
            started_at.elapsed().as_millis()
        );
        Ok(user)
    }

    pub fn login_user_session(
        &self,
        username: &str,
        password: &str,
    ) -> Result<NewApiUserSession, LlmApiError> {
        let started_at = Instant::now();
        let username = username.trim();
        log::info!(
            "[llmapi_hub][adapter] user login start username={}",
            username
        );
        if username.is_empty() || password.is_empty() {
            log::warn!("[llmapi_hub][adapter] user login skipped because username or password is empty");
            return Err(LlmApiError::new(
                LlmApiErrorCode::ProvisioningFailed,
                "New API username or password is empty",
                false,
            ));
        }

        let login_send_started_at = Instant::now();
        let response = self
            .client
            .post(self.endpoint(None, DEFAULT_USER_LOGIN_PATH))
            .header(reqwest::header::CACHE_CONTROL, "no-store")
            .json(&LoginRequest { username, password })
            .send()
            .map_err(|err| {
                LlmApiError::new(
                    LlmApiErrorCode::ServiceUnreachable,
                    format!("New API login request failed: {err}"),
                    true,
                )
            })?;
        let status = response.status();
        let cookie_header = cookie_header_from_response(&response);
        let body = response.text().map_err(|err| {
            LlmApiError::new(
                LlmApiErrorCode::ServiceUnreachable,
                format!("New API login response read failed: {err}"),
                true,
            )
        })?;
        log::info!(
            "[llmapi_hub][adapter] user login http done username={} status={} body_len={} elapsed_ms={}",
            username,
            status.as_u16(),
            body.len(),
            login_send_started_at.elapsed().as_millis()
        );
        if !status.is_success() {
            return Err(http_error("login", status.as_u16(), &body));
        }
        let envelope: NewApiEnvelope<NewApiLoginUserRecord> =
            serde_json::from_str(&body).map_err(|err| {
                LlmApiError::new(
                    LlmApiErrorCode::ProvisioningFailed,
                    format!("New API login response format is invalid: {err}"),
                    true,
                )
            })?;
        if !envelope.is_success() {
            return Err(business_error("login", envelope.message));
        }
        let user = envelope.data.ok_or_else(|| {
            LlmApiError::new(
                LlmApiErrorCode::ProvisioningFailed,
                "New API login response did not include user data",
                true,
            )
        })?;
        let user_id = json_value_to_string(&user.id);
        log::info!(
            "[llmapi_hub][adapter] user login parsed username={} newapi_user_id={}",
            username,
            user_id
        );
        if cookie_header.is_empty() {
            return Err(LlmApiError::new(
                LlmApiErrorCode::ProvisioningFailed,
                "New API login response did not include a session cookie",
                true,
            ));
        }

        let access_token: String = self.send_json(
            self.client
                .get(self.endpoint(None, DEFAULT_USER_ACCESS_TOKEN_PATH))
                .header(reqwest::header::COOKIE, cookie_header)
                .header("New-Api-User", user_id.as_str())
                .header(reqwest::header::CACHE_CONTROL, "no-store"),
            "generate user access token",
        )?;
        let access_token = access_token.trim().to_string();
        if access_token.is_empty() {
            return Err(LlmApiError::new(
                LlmApiErrorCode::ProvisioningFailed,
                "New API returned an empty user access token",
                true,
            ));
        }
        log::info!(
            "[llmapi_hub][adapter] user login ok username={} newapi_user_id={} elapsed_ms={}",
            username,
            user_id,
            started_at.elapsed().as_millis()
        );
        Ok(NewApiUserSession {
            user_id,
            access_token,
        })
    }

    fn find_default_token(
        &self,
        session: &NewApiUserSession,
    ) -> Result<Option<String>, LlmApiError> {
        let started_at = Instant::now();
        log::info!(
            "[llmapi_hub][adapter] search default token start newapi_user_id={}",
            session.user_id
        );
        let page: NewApiPage<NewApiTokenRecord> = self.send_json(
            self.user_request(
                self.client
                    .get(self.endpoint(None, DEFAULT_TOKEN_SEARCH_PATH)),
                session,
            )
            .query(&[("keyword", DEFAULT_TOKEN_NAME), ("p", "1"), ("size", "20")]),
            "search default token",
        )?;
        let token_id = page
            .items
            .into_iter()
            .find(|token| token.name == DEFAULT_TOKEN_NAME && token.status.unwrap_or(1) == 1)
            .map(|token| json_value_to_string(&token.id));
        log::info!(
            "[llmapi_hub][adapter] search default token done newapi_user_id={} found={} elapsed_ms={}",
            session.user_id,
            token_id.is_some(),
            started_at.elapsed().as_millis()
        );
        Ok(token_id)
    }

    fn create_default_token(&self, session: &NewApiUserSession) -> Result<String, LlmApiError> {
        let started_at = Instant::now();
        log::info!(
            "[llmapi_hub][adapter] create default token start newapi_user_id={}",
            session.user_id
        );
        let policy = LlmApiPolicy::default();
        let request = serde_json::json!({
            "name": DEFAULT_TOKEN_NAME,
            "expired_time": -1,
            "remain_quota": policy.quota_limit_tokens,
            "unlimited_quota": false,
            "model_limits_enabled": false,
            "model_limits": "",
        });
        self.send_success(
            self.user_request(
                self.client.post(self.endpoint(None, DEFAULT_TOKEN_PATH)),
                session,
            )
            .json(&request),
            "create default token",
        )?;
        let token_id = self.find_default_token(session)?.ok_or_else(|| {
            LlmApiError::new(
                LlmApiErrorCode::ProvisioningFailed,
                "New API default token was created but cannot be found",
                true,
            )
        })?;
        log::info!(
            "[llmapi_hub][adapter] create default token ok newapi_user_id={} token_id={} elapsed_ms={}",
            session.user_id,
            token_id,
            started_at.elapsed().as_millis()
        );
        Ok(token_id)
    }

    fn fetch_token_key(
        &self,
        session: &NewApiUserSession,
        token_id: &str,
    ) -> Result<String, LlmApiError> {
        let started_at = Instant::now();
        log::info!(
            "[llmapi_hub][adapter] fetch default token key start newapi_user_id={} token_id={}",
            session.user_id,
            token_id
        );
        let path = format!("/api/token/{token_id}/key");
        let data: TokenKeyResponse = self.send_json(
            self.user_request(self.client.post(self.endpoint(None, &path)), session),
            "fetch default token key",
        )?;
        let key = data.key.trim().to_string();
        if key.is_empty() {
            return Err(LlmApiError::new(
                LlmApiErrorCode::ProvisioningFailed,
                "New API returned an empty default token key",
                true,
            ));
        }
        log::info!(
            "[llmapi_hub][adapter] fetch default token key ok newapi_user_id={} token_id={} elapsed_ms={}",
            session.user_id,
            token_id,
            started_at.elapsed().as_millis()
        );
        Ok(key)
    }

    fn send_json<T: DeserializeOwned>(
        &self,
        request: reqwest::blocking::RequestBuilder,
        operation: &str,
    ) -> Result<T, LlmApiError> {
        let started_at = Instant::now();
        log::info!("[llmapi_hub][adapter] {} request start", operation);
        let response = request.send().map_err(|err| {
            log::warn!(
                "[llmapi_hub][adapter] {} request send failed elapsed_ms={} error={}",
                operation,
                started_at.elapsed().as_millis(),
                err
            );
            LlmApiError::new(
                LlmApiErrorCode::ServiceUnreachable,
                format!("New API {operation} request failed: {err}"),
                true,
            )
        })?;
        let status = response.status();
        let body = response.text().map_err(|err| {
            log::warn!(
                "[llmapi_hub][adapter] {} response read failed status={} elapsed_ms={} error={}",
                operation,
                status.as_u16(),
                started_at.elapsed().as_millis(),
                err
            );
            LlmApiError::new(
                LlmApiErrorCode::ServiceUnreachable,
                format!("New API {operation} response read failed: {err}"),
                true,
            )
        })?;
        log::info!(
            "[llmapi_hub][adapter] {} response received status={} body_len={} elapsed_ms={}",
            operation,
            status.as_u16(),
            body.len(),
            started_at.elapsed().as_millis()
        );
        if !status.is_success() {
            log::warn!(
                "[llmapi_hub][adapter] {} http error status={} body_len={} body_snippet={} elapsed_ms={}",
                operation,
                status.as_u16(),
                body.len(),
                compact_body(&body),
                started_at.elapsed().as_millis()
            );
            return Err(http_error(operation, status.as_u16(), &body));
        }
        let envelope: NewApiEnvelope<T> = serde_json::from_str(&body).map_err(|err| {
            log::warn!(
                "[llmapi_hub][adapter] {} response json invalid body_snippet={} elapsed_ms={} error={}",
                operation,
                compact_body(&body),
                started_at.elapsed().as_millis(),
                err
            );
            LlmApiError::new(
                LlmApiErrorCode::ProvisioningFailed,
                format!("New API {operation} response format is invalid: {err}"),
                true,
            )
        })?;
        if !envelope.is_success() {
            log::warn!(
                "[llmapi_hub][adapter] {} business error elapsed_ms={} message={}",
                operation,
                started_at.elapsed().as_millis(),
                envelope.message.as_deref().unwrap_or("")
            );
            return Err(business_error(operation, envelope.message));
        }
        let data = envelope.data.ok_or_else(|| {
            log::warn!(
                "[llmapi_hub][adapter] {} missing data elapsed_ms={}",
                operation,
                started_at.elapsed().as_millis()
            );
            LlmApiError::new(
                LlmApiErrorCode::ProvisioningFailed,
                format!("New API {operation} response did not include data"),
                true,
            )
        })?;
        log::info!(
            "[llmapi_hub][adapter] {} ok elapsed_ms={}",
            operation,
            started_at.elapsed().as_millis()
        );
        Ok(data)
    }

    fn send_success(
        &self,
        request: reqwest::blocking::RequestBuilder,
        operation: &str,
    ) -> Result<(), LlmApiError> {
        let started_at = Instant::now();
        log::info!("[llmapi_hub][adapter] {} request start", operation);
        let response = request.send().map_err(|err| {
            log::warn!(
                "[llmapi_hub][adapter] {} request send failed elapsed_ms={} error={}",
                operation,
                started_at.elapsed().as_millis(),
                err
            );
            LlmApiError::new(
                LlmApiErrorCode::ServiceUnreachable,
                format!("New API {operation} request failed: {err}"),
                true,
            )
        })?;
        let status = response.status();
        let body = response.text().map_err(|err| {
            log::warn!(
                "[llmapi_hub][adapter] {} response read failed status={} elapsed_ms={} error={}",
                operation,
                status.as_u16(),
                started_at.elapsed().as_millis(),
                err
            );
            LlmApiError::new(
                LlmApiErrorCode::ServiceUnreachable,
                format!("New API {operation} response read failed: {err}"),
                true,
            )
        })?;
        log::info!(
            "[llmapi_hub][adapter] {} response received status={} body_len={} elapsed_ms={}",
            operation,
            status.as_u16(),
            body.len(),
            started_at.elapsed().as_millis()
        );
        if !status.is_success() {
            log::warn!(
                "[llmapi_hub][adapter] {} http error status={} body_len={} body_snippet={} elapsed_ms={}",
                operation,
                status.as_u16(),
                body.len(),
                compact_body(&body),
                started_at.elapsed().as_millis()
            );
            return Err(http_error(operation, status.as_u16(), &body));
        }
        let envelope: NewApiEnvelope<serde_json::Value> =
            serde_json::from_str(&body).map_err(|err| {
                log::warn!(
                    "[llmapi_hub][adapter] {} response json invalid body_snippet={} elapsed_ms={} error={}",
                    operation,
                    compact_body(&body),
                    started_at.elapsed().as_millis(),
                    err
                );
                LlmApiError::new(
                    LlmApiErrorCode::ProvisioningFailed,
                    format!("New API {operation} response format is invalid: {err}"),
                    true,
                )
            })?;
        if !envelope.is_success() {
            log::warn!(
                "[llmapi_hub][adapter] {} business error elapsed_ms={} message={}",
                operation,
                started_at.elapsed().as_millis(),
                envelope.message.as_deref().unwrap_or("")
            );
            return Err(business_error(operation, envelope.message));
        }
        log::info!(
            "[llmapi_hub][adapter] {} ok elapsed_ms={}",
            operation,
            started_at.elapsed().as_millis()
        );
        Ok(())
    }
}

impl LlmApiHubAdapter for HttpLlmApiHubAdapter {
    fn lookup_user(&self, identity: &LlmApiIdentity) -> Result<NewApiUser, LlmApiError> {
        let endpoint = self.endpoint(self.lookup_user_endpoint.as_ref(), DEFAULT_LOOKUP_USER_PATH);
        let page: NewApiPage<NewApiUserRecord> = self.send_json(
            self.auth_request(self.client.get(endpoint)).query(&[
                ("keyword", identity.pinvou_user_id.as_str()),
                ("p", "1"),
                ("size", "20"),
            ]),
            "lookup user",
        )?;
        let user = page
            .items
            .into_iter()
            .find(|user| user.username == identity.pinvou_user_id)
            .ok_or_else(|| {
                LlmApiError::new(
                    LlmApiErrorCode::UserNotFound,
                    "New API backend account does not exist for current device",
                    false,
                )
            })?;
        if user.status.is_some_and(|status| status != 1) {
            return Err(LlmApiError::new(
                LlmApiErrorCode::ServiceDisabled,
                "New API backend account is disabled",
                false,
            ));
        }
        Ok(NewApiUser {
            id: json_value_to_string(&user.id),
            username: user.username,
            display_name: user.display_name,
            quota: user.quota,
            used_quota: user.used_quota,
        })
    }

    fn create_token(
        &self,
        identity: &LlmApiIdentity,
        newapi_user_id: &str,
    ) -> Result<NewApiToken, LlmApiError> {
        let endpoint = self.create_token_endpoint.as_ref().ok_or_else(|| {
            LlmApiError::new(
                LlmApiErrorCode::ProvisioningFailed,
                format!(
                    "QuantumNous/new-api standard /api/token only manages the authenticated user's own tokens; configure {CREATE_TOKEN_ENDPOINT_ENV} to an admin endpoint that creates or returns a token for an existing user"
                ),
                false,
            )
        })?;
        let policy = LlmApiPolicy::default();
        let request = CustomCreateTokenRequest {
            pinvou_user_id: &identity.pinvou_user_id,
            device_binding_id: &identity.device_binding_id,
            newapi_user_id,
            name: format!("Pinvou {}", identity.device_binding_id),
            expired_time: -1,
            remain_quota: policy.quota_limit_tokens,
            unlimited_quota: false,
            model_limits_enabled: true,
            model_limits: policy.allowed_models.join(","),
        };
        let data: CustomTokenResponse = self.send_json(
            self.auth_request(self.client.post(self.endpoint(Some(endpoint), "")))
                .json(&request),
            "create token",
        )?;
        Ok(NewApiToken {
            id: json_value_to_string(&data.id),
            token: data.token,
        })
    }

    fn configure_policy(
        &self,
        identity: &LlmApiIdentity,
        newapi_user_id: &str,
        token_id: &str,
        policy: &LlmApiPolicy,
    ) -> Result<(), LlmApiError> {
        let Some(endpoint) = self.configure_policy_endpoint.as_ref() else {
            return Ok(());
        };
        let request = CustomConfigurePolicyRequest {
            pinvou_user_id: &identity.pinvou_user_id,
            device_binding_id: &identity.device_binding_id,
            newapi_user_id,
            token_id,
            quota_limit_tokens: policy.quota_limit_tokens,
            rpm_limit: policy.rpm_limit,
            allowed_models: &policy.allowed_models,
        };
        self.send_success(
            self.auth_request(self.client.post(self.endpoint(Some(endpoint), "")))
                .json(&request),
            "configure policy",
        )?;
        Ok(())
    }
}

fn blocking_client() -> reqwest::blocking::Client {
    let started_at = Instant::now();
    log::info!("[llmapi_hub][adapter] blocking client build start");
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(HTTP_CONNECT_TIMEOUT_SECS))
        .timeout(Duration::from_secs(HTTP_REQUEST_TIMEOUT_SECS))
        .build()
        .expect("build New API blocking HTTP client");
    log::info!(
        "[llmapi_hub][adapter] blocking client build ok elapsed_ms={}",
        started_at.elapsed().as_millis()
    );
    client
}

fn parse_admin_credential(raw: &str) -> Result<(Option<String>, String), LlmApiError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(LlmApiError::new(
            LlmApiErrorCode::AdminCredentialMissing,
            "New API admin credential is empty",
            false,
        ));
    }
    if raw.starts_with('{') {
        let parsed: AdminCredentialJson = serde_json::from_str(raw).map_err(|err| {
            LlmApiError::new(
                LlmApiErrorCode::AdminCredentialMissing,
                format!("New API admin credential JSON is invalid: {err}"),
                false,
            )
        })?;
        let token = parsed
            .access_token
            .or(parsed.token)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                LlmApiError::new(
                    LlmApiErrorCode::AdminCredentialMissing,
                    "New API admin credential JSON must include access_token",
                    false,
                )
            })?;
        return Ok((parsed.user_id.map(|v| json_value_to_string(&v)), token));
    }
    if let Some((user_id, token)) = raw.split_once(':') {
        let user_id = user_id.trim();
        let token = token.trim();
        if !user_id.is_empty() && !token.is_empty() {
            return Ok((Some(user_id.to_string()), token.to_string()));
        }
    }
    Ok((None, raw.to_string()))
}

fn parse_user_session(raw: &str) -> Result<NewApiUserSession, LlmApiError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(LlmApiError::new(
            LlmApiErrorCode::ProvisioningFailed,
            "New API user session is empty",
            false,
        ));
    }
    if raw.starts_with('{') {
        let parsed: AdminCredentialJson = serde_json::from_str(raw).map_err(|err| {
            LlmApiError::new(
                LlmApiErrorCode::ProvisioningFailed,
                format!("New API user session JSON is invalid: {err}"),
                false,
            )
        })?;
        let user_id = parsed
            .user_id
            .as_ref()
            .map(json_value_to_string)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                LlmApiError::new(
                    LlmApiErrorCode::ProvisioningFailed,
                    "New API user session JSON must include user_id",
                    false,
                )
            })?;
        let access_token = parsed
            .access_token
            .or(parsed.token)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                LlmApiError::new(
                    LlmApiErrorCode::ProvisioningFailed,
                    "New API user session JSON must include access_token",
                    false,
                )
            })?;
        return Ok(NewApiUserSession {
            user_id,
            access_token,
        });
    }
    if let Some((user_id, access_token)) = raw.split_once(':') {
        let user_id = user_id.trim();
        let access_token = access_token.trim();
        if !user_id.is_empty() && !access_token.is_empty() {
            return Ok(NewApiUserSession {
                user_id: user_id.to_string(),
                access_token: access_token.to_string(),
            });
        }
    }
    Err(LlmApiError::new(
        LlmApiErrorCode::ProvisioningFailed,
        "New API user session must be JSON or user_id:access_token",
        false,
    ))
}

fn join_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        base.to_string()
    } else {
        format!("{base}/{path}")
    }
}

fn json_value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(v) => v.clone(),
        serde_json::Value::Number(v) => v.to_string(),
        _ => value.to_string(),
    }
}

fn cookie_header_from_response(response: &reqwest::blocking::Response) -> String {
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

fn compact_body(body: &str) -> String {
    let compact = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut snippet = compact.chars().take(500).collect::<String>();
    if compact.chars().count() > 500 {
        snippet.push_str("...");
    }
    crate::credential_store::redact_secret(&snippet)
}

fn http_error(operation: &str, status: u16, body: &str) -> LlmApiError {
    let code = match status {
        401 | 403 => LlmApiErrorCode::PermissionDenied,
        429 => LlmApiErrorCode::RateLimited,
        500..=599 => LlmApiErrorCode::ServiceUnreachable,
        _ => LlmApiErrorCode::ProvisioningFailed,
    };
    LlmApiError::new(
        code,
        format!("New API {operation} returned HTTP {status}: {body}"),
        matches!(status, 429 | 500..=599),
    )
}

fn business_error(operation: &str, message: Option<String>) -> LlmApiError {
    let message = message.unwrap_or_else(|| "unknown New API error".to_string());
    let lower = message.to_ascii_lowercase();
    let code = if lower.contains("not logged in")
        || lower.contains("access token")
        || lower.contains("权限")
        || lower.contains("privilege")
    {
        LlmApiErrorCode::PermissionDenied
    } else if lower.contains("not exist") || lower.contains("不存在") {
        LlmApiErrorCode::UserNotFound
    } else {
        LlmApiErrorCode::ProvisioningFailed
    };
    LlmApiError::new(code, format!("New API {operation} failed: {message}"), true)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[derive(Debug, Clone, Default)]
    pub struct MockLlmApiHubAdapter {
        pub fail_lookup_user: Option<LlmApiError>,
        pub fail_create_token: Option<LlmApiError>,
        pub fail_policy: Option<LlmApiError>,
    }

    impl LlmApiHubAdapter for MockLlmApiHubAdapter {
        fn lookup_user(&self, _identity: &LlmApiIdentity) -> Result<NewApiUser, LlmApiError> {
            if let Some(err) = self.fail_lookup_user.clone() {
                return Err(err);
            }
            Ok(NewApiUser {
                id: "newapi-user-1".to_string(),
                username: "u_1".to_string(),
                display_name: Some("User One".to_string()),
                quota: Some(750),
                used_quota: Some(250),
            })
        }

        fn create_token(
            &self,
            _identity: &LlmApiIdentity,
            _newapi_user_id: &str,
        ) -> Result<NewApiToken, LlmApiError> {
            if let Some(err) = self.fail_create_token.clone() {
                return Err(err);
            }
            Ok(NewApiToken {
                id: "newapi-token-1".to_string(),
                token: "sk-mock-token-123456789".to_string(),
            })
        }

        fn configure_policy(
            &self,
            _identity: &LlmApiIdentity,
            _newapi_user_id: &str,
            _token_id: &str,
            _policy: &LlmApiPolicy,
        ) -> Result<(), LlmApiError> {
            if let Some(err) = self.fail_policy.clone() {
                return Err(err);
            }
            Ok(())
        }
    }

    #[test]
    fn parses_admin_credential_json() {
        let (user_id, token) =
            parse_admin_credential(r#"{"user_id":1001,"access_token":"abc"}"#).unwrap();
        assert_eq!(user_id.as_deref(), Some("1001"));
        assert_eq!(token, "abc");
    }

    #[test]
    fn parses_admin_credential_colon_format() {
        let (user_id, token) = parse_admin_credential("1001:abc").unwrap();
        assert_eq!(user_id.as_deref(), Some("1001"));
        assert_eq!(token, "abc");
    }

    #[test]
    fn parses_user_session_json() {
        let session =
            parse_user_session(r#"{"user_id":1001,"access_token":"user-access-token"}"#).unwrap();
        assert_eq!(session.user_id, "1001");
        assert_eq!(session.access_token, "user-access-token");
    }

    #[test]
    fn parses_user_session_colon_format() {
        let session = parse_user_session("1001:user-access-token").unwrap();
        assert_eq!(session.user_id, "1001");
        assert_eq!(session.access_token, "user-access-token");
    }

    #[test]
    fn joins_urls_without_double_slashes() {
        assert_eq!(
            join_url("https://example.com/llmapi/", "/api/user/search"),
            "https://example.com/llmapi/api/user/search"
        );
    }

    #[test]
    fn parses_newapi_code_envelope_for_usage() {
        let envelope: NewApiEnvelope<NewApiTokenUsage> = serde_json::from_str(
            r#"{
                "code": true,
                "message": "",
                "data": {
                    "total_granted": 1000,
                    "total_used": 250,
                    "total_available": 750,
                    "unlimited_quota": false
                }
            }"#,
        )
        .unwrap();
        assert!(envelope.is_success());
        let usage = envelope.data.unwrap();
        assert_eq!(usage.total_granted, 1000);
        assert_eq!(usage.total_used, 250);
        assert_eq!(usage.total_available, 750);
        assert!(!usage.unlimited_quota);
    }

    #[test]
    fn parses_legacy_success_envelope() {
        let envelope: NewApiEnvelope<serde_json::Value> =
            serde_json::from_str(r#"{"success":true,"message":"","data":{"ok":true}}"#).unwrap();
        assert!(envelope.is_success());
    }
}
