//! Persistent configuration, the durable RPC idempotency ledger, pending
//! revocation replay, and user-custom Relay settings.
//!
//! These helpers are stateless and touch only the private JSON files under the
//! pinvou3 home directory. They never lock `Inner`; the facade reads the result
//! and updates `Inner` under its own guard.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    platform, ActiveEndpoint, PendingRevocation, PendingRevocationLedger, RpcLedger,
    WebAccessConfig, WebAccessInfo,
};
use crate::platform::paths;

pub(super) fn pairing_info(endpoint: &ActiveEndpoint) -> WebAccessInfo {
    WebAccessInfo {
        endpoint_id: endpoint.config.endpoint_id.clone(),
        url: endpoint.url.clone(),
        qr_data_url: crate::features::connectors::connector_cli::make_qr(&endpoint.url),
        status: endpoint.status,
    }
}

pub(super) fn fresh_config(allow_host_workspace: bool) -> WebAccessConfig {
    WebAccessConfig {
        relay_url: remote_relay_ws_url(),
        endpoint_id: format!("ep_{}", crate::features::remote_control::short_token(24)),
        access_token: crate::features::remote_control::short_token(48),
        desktop_secret: crate::features::remote_control::short_token(48),
        allow_host_workspace,
    }
}

pub(super) fn validate_config(config: &WebAccessConfig) -> Result<(), String> {
    if config.relay_url.len() > 2_048
        || !(config.relay_url.starts_with("ws://") || config.relay_url.starts_with("wss://"))
        || config.relay_url.chars().any(char::is_whitespace)
    {
        return Err("Web access relay_url is invalid".to_string());
    }
    if config.endpoint_id.len() < 8
        || config.endpoint_id.len() > 128
        || !config
            .endpoint_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("Web access endpoint_id is invalid".to_string());
    }
    if !(24..=1_024).contains(&config.access_token.len())
        || !(24..=1_024).contains(&config.desktop_secret.len())
    {
        return Err("Web access credentials are invalid".to_string());
    }
    Ok(())
}

pub(super) fn config_path() -> PathBuf {
    paths::pinvou3_home().join("web-access.json")
}

pub(super) fn process_lock_path() -> PathBuf {
    paths::pinvou3_home().join("web-access.lock")
}

pub(super) fn pending_revocations_path() -> PathBuf {
    paths::pinvou3_home().join("web-access-pending-revocations.json")
}

pub(super) fn acquire_process_lock(path: &Path) -> Result<File, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid Web access lock path: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    platform::configure_private_open_options(&mut options);
    let file = options
        .open(path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    platform::enforce_private_permissions(&file, path)
        .map_err(|error| format!("set private permissions on {}: {error}", path.display()))?;
    file.try_lock_exclusive().map_err(|error| {
        format!(
            "Web access is already owned by another desktop process ({}): {error}",
            path.display()
        )
    })?;
    Ok(file)
}

pub(super) fn rpc_ledger_path() -> PathBuf {
    paths::pinvou3_home().join("web-access-rpc-ledger.json")
}

pub(super) fn load_config() -> Result<Option<WebAccessConfig>, String> {
    load_config_from(&config_path())
}

pub(super) fn load_config_from(path: &Path) -> Result<Option<WebAccessConfig>, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    let mut text = String::new();
    file.take((super::MAX_WEB_ACCESS_CONFIG_BYTES + 1) as u64)
        .read_to_string(&mut text)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    if text.len() > super::MAX_WEB_ACCESS_CONFIG_BYTES {
        return Err(format!(
            "Web access config is larger than {} KiB: {}",
            super::MAX_WEB_ACCESS_CONFIG_BYTES / 1024,
            path.display()
        ));
    }
    let config: WebAccessConfig = serde_json::from_str(&text)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    validate_config(&config)?;
    Ok(Some(config))
}

pub(super) fn persist_config(config: &WebAccessConfig) -> Result<(), String> {
    validate_config(config)?;
    atomic_write_private_json(&config_path(), config, "Web access config")
}

pub(super) fn remove_config() -> Result<(), String> {
    remove_private_file(&config_path())
}

// ── 用户自定义 Relay 地址 ─────────────────────────────────────────────
// 设置页填写的域名/IP 规范化为「WebSocket 地址 + 页面基址」一对后持久化；
// 生效优先级：运行时 env 覆盖 > PINVOU_REMOTE_* env > 本设置 > 内置默认。

const RELAY_DEFAULT_BASE_PATH: &str = "/pinvou3/remote";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelaySettings {
    pub relay_url: String,
    pub public_base_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelaySettingsInfo {
    pub relay_url: String,
    pub public_base_url: String,
    pub custom: bool,
    pub default_relay_url: String,
    pub default_public_base_url: String,
}

pub(super) fn relay_settings_path() -> PathBuf {
    paths::pinvou3_home().join("web-relay.json")
}

pub(super) fn validate_relay_settings(settings: &RelaySettings) -> Result<(), String> {
    if settings.relay_url.len() > 2_048
        || !(settings.relay_url.starts_with("ws://") || settings.relay_url.starts_with("wss://"))
        || settings.relay_url.chars().any(char::is_whitespace)
    {
        return Err("relay address does not normalize to a valid ws(s) URL".to_string());
    }
    if settings.public_base_url.len() > 2_048
        || !(settings.public_base_url.starts_with("http://")
            || settings.public_base_url.starts_with("https://"))
        || settings.public_base_url.chars().any(char::is_whitespace)
    {
        return Err("relay address does not normalize to a valid http(s) base".to_string());
    }
    Ok(())
}

/// 把用户填写的 Relay 地址规范化。接受裸域名/IP（默认按 TLS 处理成 wss/https）、
/// `ws(s)://`、`http(s)://` 前缀以及可选自定义路径；无 TLS 证书的环境需要显式
/// 写 `ws://` 或 `http://` 前缀，不做静默降级。
pub(super) fn normalize_relay_address(input: &str) -> Result<RelaySettings, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("relay address is empty".to_string());
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err("relay address must not contain whitespace".to_string());
    }
    if trimmed.len() > 1_024 {
        return Err("relay address is too long".to_string());
    }
    let (ws_scheme, http_scheme, rest) = if let Some(rest) = trimmed.strip_prefix("wss://") {
        ("wss", "https", rest)
    } else if let Some(rest) = trimmed.strip_prefix("ws://") {
        ("ws", "http", rest)
    } else if let Some(rest) = trimmed.strip_prefix("https://") {
        ("wss", "https", rest)
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        ("ws", "http", rest)
    } else if trimmed.contains("://") {
        return Err("relay address only supports ws/wss/http/https".to_string());
    } else {
        ("wss", "https", trimmed)
    };
    if rest.contains('?') || rest.contains('#') || rest.contains('@') {
        return Err("relay address must not contain userinfo, query or fragment".to_string());
    }
    let (host, path) = match rest.find('/') {
        Some(index) => (&rest[..index], rest[index..].trim_end_matches('/')),
        None => (rest, ""),
    };
    if host.is_empty() {
        return Err("relay address is missing a host".to_string());
    }
    // 路径规则：不填 → 内置 /pinvou3/remote；填了 → 尊重自定义（容忍粘贴完整
    // ws 地址时结尾多出的 /ws，避免「复制现有 relay_url 再保存」变成 /ws/ws）。
    let base_path = if path.is_empty() {
        RELAY_DEFAULT_BASE_PATH.to_string()
    } else {
        path.strip_suffix("/ws").unwrap_or(path).to_string()
    };
    let settings = RelaySettings {
        relay_url: format!("{ws_scheme}://{host}{base_path}/ws"),
        public_base_url: format!("{http_scheme}://{host}{base_path}"),
    };
    validate_relay_settings(&settings)?;
    Ok(settings)
}

/// 读取失败（不存在/损坏/非法）一律回落默认 relay：设置文件不是凭据，宁可
/// 降级也不把「启用 Web 访问」整个卡死；重新保存会原子覆盖坏文件。
pub(super) fn load_relay_settings() -> Option<RelaySettings> {
    let file = File::open(relay_settings_path()).ok()?;
    let mut text = String::new();
    file.take((super::MAX_WEB_ACCESS_CONFIG_BYTES + 1) as u64)
        .read_to_string(&mut text)
        .ok()?;
    if text.len() > super::MAX_WEB_ACCESS_CONFIG_BYTES {
        return None;
    }
    let settings: RelaySettings = serde_json::from_str(&text).ok()?;
    validate_relay_settings(&settings).ok()?;
    Some(settings)
}

pub(super) fn pending_revocation_key(pending: &PendingRevocation) -> String {
    format!("{}\n{}", pending.relay_url, pending.endpoint_id)
}

pub(super) fn validate_pending_revocation(pending: &PendingRevocation) -> Result<(), String> {
    if pending.relay_url.len() > 2_048
        || !(pending.relay_url.starts_with("ws://") || pending.relay_url.starts_with("wss://"))
        || pending.relay_url.chars().any(char::is_whitespace)
    {
        return Err("pending Web revocation has an invalid relay_url".to_string());
    }
    if pending.endpoint_id.len() < 4
        || pending.endpoint_id.len() > 128
        || !pending
            .endpoint_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("pending Web revocation has an invalid endpoint_id".to_string());
    }
    if pending.desktop_secret.len() < 24 || pending.desktop_secret.len() > 1_024 {
        return Err("pending Web revocation has an invalid desktop secret".to_string());
    }
    Ok(())
}

pub(super) fn validate_pending_revocation_ledger(
    ledger: &PendingRevocationLedger,
) -> Result<(), String> {
    if ledger.version != super::PENDING_REVOCATIONS_VERSION {
        return Err(format!(
            "unsupported pending Web revocation version {}",
            ledger.version
        ));
    }
    if ledger.entries.len() > super::MAX_PENDING_REVOCATIONS {
        return Err(format!(
            "pending Web revocation count exceeds {}",
            super::MAX_PENDING_REVOCATIONS
        ));
    }
    let mut keys = HashSet::new();
    for pending in &ledger.entries {
        validate_pending_revocation(pending)?;
        if !keys.insert(pending_revocation_key(pending)) {
            return Err(format!(
                "duplicate pending Web revocation for {}",
                pending.endpoint_id
            ));
        }
    }
    Ok(())
}

pub(super) fn load_pending_revocations() -> Result<PendingRevocationLedger, String> {
    load_pending_revocations_from(&pending_revocations_path())
}

pub(super) fn load_pending_revocations_from(
    path: &Path,
) -> Result<PendingRevocationLedger, String> {
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PendingRevocationLedger::default());
        }
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    if data.len() > super::MAX_PENDING_REVOCATIONS_FILE_BYTES {
        return Err(format!(
            "pending Web revocation file is larger than {} KiB: {}",
            super::MAX_PENDING_REVOCATIONS_FILE_BYTES / 1024,
            path.display()
        ));
    }
    let ledger: PendingRevocationLedger = serde_json::from_slice(&data)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    validate_pending_revocation_ledger(&ledger)?;
    Ok(ledger)
}

pub(super) fn persist_pending_revocations_to(
    path: &Path,
    ledger: &PendingRevocationLedger,
) -> Result<(), String> {
    validate_pending_revocation_ledger(ledger)?;
    if ledger.entries.is_empty() {
        remove_private_file(path)
    } else {
        atomic_write_private_json(path, ledger, "pending Web revocations")
    }
}

pub(super) fn queue_pending_revocation(config: &WebAccessConfig) -> Result<(), String> {
    queue_pending_revocation_at(&pending_revocations_path(), PendingRevocation::from(config))
}

pub(super) fn queue_pending_revocation_at(
    path: &Path,
    pending: PendingRevocation,
) -> Result<(), String> {
    validate_pending_revocation(&pending)?;
    let mut ledger = load_pending_revocations_from(path)?;
    let key = pending_revocation_key(&pending);
    if let Some(existing) = ledger
        .entries
        .iter()
        .find(|entry| pending_revocation_key(entry) == key)
    {
        return if existing == &pending {
            Ok(())
        } else {
            Err(format!(
                "conflicting pending Web revocation credentials for {}",
                pending.endpoint_id
            ))
        };
    }
    if ledger.entries.len() >= super::MAX_PENDING_REVOCATIONS {
        return Err(format!(
            "pending Web revocation queue is full ({})",
            super::MAX_PENDING_REVOCATIONS
        ));
    }
    ledger.entries.push_back(pending);
    persist_pending_revocations_to(path, &ledger)
}

pub(super) fn acknowledge_pending_revocation(pending: &PendingRevocation) -> Result<(), String> {
    acknowledge_pending_revocation_at(&pending_revocations_path(), pending)
}

pub(super) fn acknowledge_pending_revocation_at(
    path: &Path,
    pending: &PendingRevocation,
) -> Result<(), String> {
    let mut ledger = load_pending_revocations_from(path)?;
    ledger.entries.retain(|entry| entry != pending);
    persist_pending_revocations_to(path, &ledger)
}

pub(super) fn eligible_pending_revocations(
    ledger: &PendingRevocationLedger,
    current: Option<&WebAccessConfig>,
) -> Vec<PendingRevocation> {
    ledger
        .entries
        .iter()
        .filter(|pending| {
            current.is_none_or(|config| {
                pending.relay_url != config.relay_url
                    || pending.endpoint_id != config.endpoint_id
                    || pending.desktop_secret != config.desktop_secret
            })
        })
        .cloned()
        .collect()
}

pub(super) fn pending_revocation_ack(value: &Value, endpoint_id: &str) -> bool {
    let message_endpoint = value.get("endpoint_id").and_then(Value::as_str);
    match value.get("type").and_then(Value::as_str) {
        Some("desktop_endpoint_revoked") | Some("desktop_endpoint_replaced") => {
            message_endpoint == Some(endpoint_id)
        }
        Some("error")
            if value.get("code").and_then(Value::as_str) == Some("endpoint_not_found") =>
        {
            message_endpoint.is_none_or(|value| value == endpoint_id)
        }
        _ => false,
    }
}

pub(super) fn load_or_initialize_rpc_ledger(endpoint_id: &str) -> Result<RpcLedger, String> {
    let path = rpc_ledger_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let ledger = RpcLedger::for_endpoint(endpoint_id);
            persist_rpc_ledger(&ledger)?;
            return Ok(ledger);
        }
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    let mut ledger: RpcLedger = serde_json::from_str(&text)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    if ledger.endpoint_id != endpoint_id {
        let ledger = RpcLedger::for_endpoint(endpoint_id);
        persist_rpc_ledger(&ledger)?;
        return Ok(ledger);
    }
    if ledger.version != super::RPC_LEDGER_VERSION {
        return Err(format!(
            "unsupported Web RPC ledger version {} in {}",
            ledger.version,
            path.display()
        ));
    }
    let mut request_ids = HashSet::new();
    for entry in &ledger.entries {
        if entry.request_id.is_empty()
            || entry.request_id.len() > 256
            || entry.fingerprint.len() != 64
            || !entry
                .fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(format!(
                "invalid Web RPC ledger entry in {}",
                path.display()
            ));
        }
        if !request_ids.insert(entry.request_id.as_str()) {
            return Err(format!(
                "duplicate Web RPC request id in {}: {}",
                path.display(),
                entry.request_id
            ));
        }
    }
    if ledger.entries.len() > super::RPC_CACHE_CAPACITY {
        let overflow = ledger.entries.len() - super::RPC_CACHE_CAPACITY;
        ledger.entries.drain(..overflow);
        persist_rpc_ledger(&ledger)?;
    }
    Ok(ledger)
}

pub(super) fn persist_rpc_ledger(ledger: &RpcLedger) -> Result<(), String> {
    if ledger.version != super::RPC_LEDGER_VERSION || ledger.endpoint_id.is_empty() {
        return Err("refusing to persist an invalid Web RPC ledger".to_string());
    }
    atomic_write_private_json(&rpc_ledger_path(), ledger, "Web RPC ledger")
}

pub(super) fn remove_rpc_ledger() -> Result<(), String> {
    remove_private_file(&rpc_ledger_path())
}

pub(super) fn atomic_write_private_json<T: Serialize>(
    path: &Path,
    value: &T,
    label: &str,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid {label} path: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let data =
        serde_json::to_vec_pretty(value).map_err(|error| format!("serialize {label}: {error}"))?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("web-access");
    let mut temporary = None;
    for _ in 0..16 {
        let candidate = parent.join(format!(
            ".{file_name}.tmp-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        platform::configure_private_open_options(&mut options);
        match options.open(&candidate) {
            Ok(file) => {
                platform::enforce_private_permissions(&file, &candidate).map_err(|error| {
                    format!(
                        "set private permissions on {}: {error}",
                        candidate.display()
                    )
                })?;
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("create {}: {error}", candidate.display())),
        }
    }
    let (temporary, mut file) =
        temporary.ok_or_else(|| format!("allocate a temporary file next to {}", path.display()))?;
    let result = (|| {
        file.write_all(&data)
            .map_err(|error| format!("write {}: {error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("sync {}: {error}", temporary.display()))?;
        drop(file);
        platform::atomic_replace(&temporary, path)
            .map_err(|error| format!("commit {}: {error}", path.display()))?;
        platform::sync_parent_directory(parent)
            .map_err(|error| format!("sync {}: {error}", parent.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub(super) fn remove_private_file(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            if let Some(parent) = path.parent() {
                platform::sync_parent_directory(parent)
                    .map_err(|error| format!("sync {}: {error}", parent.display()))?;
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove {}: {error}", path.display())),
    }
}

pub(super) fn remote_public_base_url() -> String {
    if let Ok(url) = std::env::var("PINVOU_REMOTE_PUBLIC_URL") {
        return url;
    }
    if let Some(settings) = load_relay_settings() {
        return settings.public_base_url;
    }
    super::DEFAULT_PUBLIC_BASE_URL.to_string()
}

pub(super) fn remote_relay_ws_url() -> String {
    if let Ok(url) = std::env::var("PINVOU_REMOTE_RELAY_WS_URL") {
        return url;
    }
    if let Some(settings) = load_relay_settings() {
        return settings.relay_url;
    }
    super::DEFAULT_RELAY_WS_URL.to_string()
}

/// Development/manual-test override for the active connection only. Unlike
/// `PINVOU_REMOTE_RELAY_WS_URL`, this value is never written to the persistent
/// Web access config, so testing a local relay cannot strand the normal link.
pub(super) fn apply_runtime_relay_override(mut config: WebAccessConfig) -> WebAccessConfig {
    if let Ok(relay_url) = std::env::var("PINVOU_REMOTE_RUNTIME_RELAY_WS_URL") {
        if !relay_url.trim().is_empty() {
            config.relay_url = relay_url;
        }
    }
    config
}

pub(super) fn public_url(config: &WebAccessConfig) -> String {
    let mut url = format!(
        "{}/#endpoint={}&token={}",
        remote_public_base_url().trim_end_matches('/'),
        percent_encode(&config.endpoint_id),
        percent_encode(&config.access_token),
    );
    if config.relay_url != super::DEFAULT_RELAY_WS_URL {
        url.push_str("&relay=");
        url.push_str(&percent_encode(&config.relay_url));
    }
    url
}

pub(super) fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Fresh opaque epoch token for the stream journal. Used by `StreamState` in the
/// parent module, so it stays `pub(super)`.
pub(super) fn new_stream_epoch() -> String {
    format!("epoch_{}", crate::features::remote_control::short_token(24))
}
