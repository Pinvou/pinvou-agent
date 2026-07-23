//! 匿名设备遥测：唯一设备登记、2 分钟心跳、TurnComplete 用量可靠补报。
//!
//! 只上传设备运行元数据和 token 计数；不上传消息正文、session 标题、文件路径、
//! 工具参数或工具输出。服务不可用时只写本地 outbox，不阻塞 Engine/UI。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::Engine as _;
use parking_lot::Mutex;
use rand::RngCore;
use reqwest::{Client, StatusCode};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const DEFAULT_BASE_URL: &str = "https://pinvou.com/pinvou3/telemetry";
// 协议版本门槛，不是客户端身份凭证；真实性和资源边界由服务端另行保护。
const ENROLLMENT_TOKEN: &str = "pinvou_tel_enroll_v1_7Jk9pQ3mZ8xW2sN6cR4tY5uH";
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2 * 60);
const OUTBOX_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const OUTBOX_MAX_EVENTS: usize = 5_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IdentityFile {
    version: u8,
    fallback_claim: String,
    claim_digest: String,
    #[serde(default)]
    registration_secret: String,
    device_id: Option<String>,
    device_token: Option<String>,
}

#[derive(Debug, Clone)]
struct HardwareIdentity {
    claim: String,
    source: String,
    quality: String,
    digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UsageEvent {
    event_id: String,
    occurred_at: u64,
    input_tokens: u64,
    output_tokens: u64,
    success: bool,
}

#[derive(Debug, Serialize)]
struct RegisterRequest<'a> {
    enrollment_token: &'static str,
    registration_secret: &'a str,
    hardware_claim: &'a str,
    hardware_source: &'a str,
    identity_quality: &'a str,
    app_version: &'a str,
    platform: &'static str,
    arch: &'static str,
}

#[derive(Debug, Deserialize)]
struct RegisterResponse {
    device_id: String,
    device_token: String,
}

#[derive(Debug, Serialize)]
struct HeartbeatRequest<'a> {
    device_id: &'a str,
    registration_secret: &'a str,
    app_version: &'a str,
    platform: &'static str,
    arch: &'static str,
    state: &'static str,
    last_activity_at: Option<u64>,
}

#[derive(Debug, Serialize)]
struct EventsRequest<'a> {
    device_id: &'a str,
    events: &'a [UsageEvent],
}

#[derive(Debug, Deserialize)]
struct EventsResponse {
    #[serde(default)]
    accepted: Vec<String>,
    #[serde(default)]
    duplicates: Vec<String>,
}

struct TelemetryInner {
    identity_path: PathBuf,
    identity: Mutex<IdentityFile>,
    hardware: HardwareIdentity,
    outbox: Mutex<Connection>,
    client: Client,
    base_url: String,
    app_version: &'static str,
    active_turns: AtomicUsize,
    last_activity_at: AtomicU64,
    last_error: Mutex<Option<String>>,
}

#[derive(Clone)]
pub struct TelemetryState {
    inner: Arc<TelemetryInner>,
}

impl TelemetryState {
    pub fn boot(app_version: &'static str) -> Result<Option<Self>> {
        if matches!(
            std::env::var("PINVOU_TELEMETRY_DISABLED").ok().as_deref(),
            Some("1" | "true" | "yes")
        ) {
            return Ok(None);
        }
        let root = crate::platform::paths::pinvou3_home().join("telemetry");
        fs::create_dir_all(&root)
            .with_context(|| format!("create telemetry dir {}", root.display()))?;
        let identity_path = root.join("identity.json");
        let mut identity = load_or_create_identity(&identity_path)?;
        let hardware = detect_hardware_identity(&identity.fallback_claim);
        if !identity.claim_digest.is_empty() && identity.claim_digest != hardware.digest {
            // 硬件身份发生变化（例如克隆了 ~/.pinvou3）：旧凭证不能跟到另一台设备。
            identity.device_id = None;
            identity.device_token = None;
            identity.registration_secret = random_id("rs_", 32);
        }
        identity.version = 2;
        identity.claim_digest = hardware.digest.clone();
        save_identity(&identity_path, &identity)?;

        let outbox_path = root.join("outbox.sqlite3");
        let mut connection = Connection::open(&outbox_path)
            .with_context(|| format!("open telemetry outbox {}", outbox_path.display()))?;
        initialize_outbox(&mut connection, now_ms())?;
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(12))
            .user_agent(format!("pinvou3/{app_version}"))
            .build()?;
        let base_url = std::env::var("PINVOU_TELEMETRY_BASE_URL")
            .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        Ok(Some(Self {
            inner: Arc::new(TelemetryInner {
                identity_path,
                identity: Mutex::new(identity),
                hardware,
                outbox: Mutex::new(connection),
                client,
                base_url,
                app_version,
                active_turns: AtomicUsize::new(0),
                last_activity_at: AtomicU64::new(0),
                last_error: Mutex::new(None),
            }),
        }))
    }

    pub fn spawn(&self) {
        let state = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                let result = state.upload_once().await;
                state.report_result(result.as_ref().err());
                tokio::time::sleep(HEARTBEAT_INTERVAL).await;
            }
        });
    }

    pub fn on_turn_started(&self) {
        self.inner.active_turns.fetch_add(1, Ordering::Relaxed);
        self.inner
            .last_activity_at
            .store(now_ms(), Ordering::Relaxed);
    }

    pub fn record_turn(&self, input_tokens: u32, output_tokens: u32, success: bool) {
        let _ =
            self.inner
                .active_turns
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    Some(value.saturating_sub(1))
                });
        let occurred_at = now_ms();
        self.inner
            .last_activity_at
            .store(occurred_at, Ordering::Relaxed);
        let event = UsageEvent {
            event_id: random_id("evt_", 18),
            occurred_at,
            input_tokens: u64::from(input_tokens),
            output_tokens: u64::from(output_tokens),
            success,
        };
        if let Err(error) = self.insert_event(&event) {
            eprintln!("[pinvou3-telemetry] persist usage event failed: {error:#}");
        }
    }

    fn insert_event(&self, event: &UsageEvent) -> Result<()> {
        let mut connection = self.inner.outbox.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO usage_events
             (event_id, occurred_at, input_tokens, output_tokens, success)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event.event_id,
                event.occurred_at,
                event.input_tokens,
                event.output_tokens,
                event.success as u8,
            ],
        )?;
        prune_outbox_transaction(&transaction, event.occurred_at)?;
        transaction.commit()?;
        Ok(())
    }

    async fn upload_once(&self) -> Result<()> {
        let (device_id, device_token) = self.ensure_registered().await?;
        self.upload_events(&device_id, &device_token).await?;
        let state = if self.inner.active_turns.load(Ordering::Relaxed) > 0 {
            "generating"
        } else {
            "online"
        };
        let last_activity = self.inner.last_activity_at.load(Ordering::Relaxed);
        let registration_secret = self.inner.identity.lock().registration_secret.clone();
        let response = self
            .inner
            .client
            .post(format!("{}/v1/heartbeat", self.inner.base_url))
            .bearer_auth(&device_token)
            .json(&HeartbeatRequest {
                device_id: &device_id,
                registration_secret: &registration_secret,
                app_version: self.inner.app_version,
                platform: std::env::consts::OS,
                arch: std::env::consts::ARCH,
                state,
                last_activity_at: (last_activity > 0).then_some(last_activity),
            })
            .send()
            .await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            self.clear_credentials()?;
            anyhow::bail!("device credential rejected; will register again");
        }
        response.error_for_status()?;
        Ok(())
    }

    async fn ensure_registered(&self) -> Result<(String, String)> {
        {
            let identity = self.inner.identity.lock();
            if let (Some(device_id), Some(device_token)) =
                (identity.device_id.clone(), identity.device_token.clone())
            {
                return Ok((device_id, device_token));
            }
        }
        let registration_secret = self.inner.identity.lock().registration_secret.clone();
        let response = self
            .inner
            .client
            .post(format!("{}/v1/register", self.inner.base_url))
            .json(&RegisterRequest {
                enrollment_token: ENROLLMENT_TOKEN,
                registration_secret: &registration_secret,
                hardware_claim: &self.inner.hardware.claim,
                hardware_source: &self.inner.hardware.source,
                identity_quality: &self.inner.hardware.quality,
                app_version: self.inner.app_version,
                platform: std::env::consts::OS,
                arch: std::env::consts::ARCH,
            })
            .send()
            .await?
            .error_for_status()?
            .json::<RegisterResponse>()
            .await?;
        if response.device_id.is_empty() || response.device_token.is_empty() {
            anyhow::bail!("registration response missing device credential");
        }
        let mut identity = self.inner.identity.lock();
        identity.device_id = Some(response.device_id.clone());
        identity.device_token = Some(response.device_token.clone());
        save_identity(&self.inner.identity_path, &identity)?;
        Ok((response.device_id, response.device_token))
    }

    async fn upload_events(&self, device_id: &str, device_token: &str) -> Result<()> {
        let events = self.pending_events(100)?;
        if events.is_empty() {
            return Ok(());
        }
        let response = self
            .inner
            .client
            .post(format!("{}/v1/events", self.inner.base_url))
            .bearer_auth(device_token)
            .json(&EventsRequest {
                device_id,
                events: &events,
            })
            .send()
            .await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            self.clear_credentials()?;
            anyhow::bail!("device credential rejected while uploading usage");
        }
        let ack = response
            .error_for_status()?
            .json::<EventsResponse>()
            .await?;
        let mut ids = ack.accepted;
        ids.extend(ack.duplicates);
        self.delete_events(&ids)?;
        Ok(())
    }

    fn pending_events(&self, limit: usize) -> Result<Vec<UsageEvent>> {
        let connection = self.inner.outbox.lock();
        let mut statement = connection.prepare(
            "SELECT event_id, occurred_at, input_tokens, output_tokens, success
             FROM usage_events ORDER BY occurred_at ASC LIMIT ?1",
        )?;
        let rows = statement.query_map([limit as u64], |row| {
            Ok(UsageEvent {
                event_id: row.get(0)?,
                occurred_at: row.get(1)?,
                input_tokens: row.get(2)?,
                output_tokens: row.get(3)?,
                success: row.get::<_, u8>(4)? != 0,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn delete_events(&self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut connection = self.inner.outbox.lock();
        let transaction = connection.transaction()?;
        for id in ids {
            transaction.execute("DELETE FROM usage_events WHERE event_id = ?1", [id])?;
        }
        transaction.commit()?;
        Ok(())
    }

    fn clear_credentials(&self) -> Result<()> {
        let mut identity = self.inner.identity.lock();
        identity.device_id = None;
        identity.device_token = None;
        save_identity(&self.inner.identity_path, &identity)
    }

    fn report_result(&self, error: Option<&anyhow::Error>) {
        let mut last = self.inner.last_error.lock();
        match error {
            Some(error) => {
                let message = format!("{error:#}");
                if last.as_deref() != Some(message.as_str()) {
                    eprintln!("[pinvou3-telemetry] upload pending: {message}");
                    *last = Some(message);
                }
            }
            None if last.take().is_some() => {
                eprintln!("[pinvou3-telemetry] upload recovered");
            }
            None => {}
        }
    }
}

fn load_or_create_identity(path: &Path) -> Result<IdentityFile> {
    if let Ok(text) = fs::read_to_string(path) {
        if let Ok(mut identity) = serde_json::from_str::<IdentityFile>(&text) {
            if !identity.fallback_claim.is_empty() {
                if identity.registration_secret.is_empty() {
                    identity.registration_secret = random_id("rs_", 32);
                }
                return Ok(identity);
            }
        }
    }
    let identity = IdentityFile {
        version: 2,
        fallback_claim: random_id("fallback_", 24),
        claim_digest: String::new(),
        registration_secret: random_id("rs_", 32),
        device_id: None,
        device_token: None,
    };
    save_identity(path, &identity)?;
    Ok(identity)
}

fn prune_outbox(connection: &mut Connection, now: u64) -> Result<()> {
    let transaction = connection.transaction()?;
    prune_outbox_transaction(&transaction, now)?;
    transaction.commit()?;
    Ok(())
}

fn initialize_outbox(connection: &mut Connection, now: u64) -> Result<()> {
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=FULL;
         CREATE TABLE IF NOT EXISTS usage_events (
           event_id TEXT PRIMARY KEY,
           occurred_at INTEGER NOT NULL,
           input_tokens INTEGER NOT NULL,
           output_tokens INTEGER NOT NULL,
           success INTEGER NOT NULL
         );",
    )?;
    prune_outbox(connection, now)
}

fn prune_outbox_transaction(transaction: &rusqlite::Transaction<'_>, now: u64) -> Result<()> {
    let cutoff = now.saturating_sub(OUTBOX_RETENTION.as_millis() as u64);
    transaction.execute("DELETE FROM usage_events WHERE occurred_at < ?1", [cutoff])?;
    transaction.execute(
        "DELETE FROM usage_events
         WHERE event_id IN (
           SELECT event_id FROM usage_events
           ORDER BY occurred_at DESC, event_id DESC
           LIMIT -1 OFFSET ?1
         )",
        [OUTBOX_MAX_EVENTS as u64],
    )?;
    Ok(())
}

fn save_identity(path: &Path, identity: &IdentityFile) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(identity)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&temporary, path)?;
    Ok(())
}

fn detect_hardware_identity(fallback_claim: &str) -> HardwareIdentity {
    if let Ok(value) = std::env::var("PINVOU_TELEMETRY_DEVICE_CLAIM") {
        if let Some(value) = normalize_hardware_value(&value) {
            return hardware_identity(format!("override:{value}"), "env_override", "managed_asset");
        }
    }
    #[cfg(target_os = "linux")]
    {
        for (path, source, quality) in [
            (
                "/sys/class/dmi/id/product_uuid",
                "dmi_product_uuid",
                "hardware_serial",
            ),
            (
                "/sys/class/dmi/id/product_serial",
                "dmi_product_serial",
                "hardware_serial",
            ),
            (
                "/sys/class/dmi/id/board_serial",
                "dmi_board_serial",
                "hardware_serial",
            ),
            ("/etc/machine-id", "machine_id", "os_machine"),
        ] {
            if let Ok(value) = fs::read_to_string(path) {
                if let Some(value) = normalize_hardware_value(&value) {
                    return hardware_identity(format!("linux:{source}:{value}"), source, quality);
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        // ioreg -d2 -c IOPlatformExpertDevice reads IOPlatformUUID,
        // a stable machine-unique identifier that needs no elevated privileges.
        // (Note: the class is IOPlatformExpertDevice, NOT IOPlatformExpertNode.)
        if let Ok(out) = std::process::Command::new("/usr/sbin/ioreg")
            .args(["-d2", "-c", "IOPlatformExpertDevice"])
            .output()
        {
            let s = String::from_utf8_lossy(&out.stdout);
            for line in s.lines() {
                if line.contains("IOPlatformUUID") {
                    if let Some(value) = line.split('=').nth(1) {
                        // 与 Linux/Windows 分支一致:过 normalize_hardware_value 占位过滤
                        // (拒绝全零 UUID / FFFFFFFF / ToBeFilledByO.E.M. 等),避免极端
                        // VM/烧录异常把占位 UUID 当真机 ID 上报。
                        if let Some(v) = normalize_hardware_value(
                            value.trim().trim_matches('"'),
                        ) {
                            return hardware_identity(
                                format!("macos:ioplatformuuid:{v}"),
                                "ioplatformuuid",
                                "hardware_serial",
                            );
                        }
                    }
                }
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        let script = "try { (Get-CimInstance Win32_ComputerSystemProduct -ErrorAction Stop).UUID } catch { '' }";
        let output = crate::platform::process::HiddenCommand::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .output();
        if let Ok(output) = output {
            if output.status.success() {
                if let Some(value) =
                    normalize_hardware_value(&String::from_utf8_lossy(&output.stdout))
                {
                    return hardware_identity(
                        format!("windows:system_uuid:{value}"),
                        "system_uuid",
                        "hardware_serial",
                    );
                }
            }
        }
    }
    hardware_identity(
        format!("fallback:{fallback_claim}"),
        "persisted_random",
        "installation_only",
    )
}

fn hardware_identity(claim: String, source: &str, quality: &str) -> HardwareIdentity {
    let digest = format!("{:x}", Sha256::digest(claim.as_bytes()));
    HardwareIdentity {
        claim,
        source: source.to_string(),
        quality: quality.to_string(),
        digest,
    }
}

fn normalize_hardware_value(value: &str) -> Option<String> {
    let normalized = value.trim().to_ascii_lowercase();
    let compact: String = normalized
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        .collect();
    let placeholders = [
        "unknown",
        "none",
        "defaultstring",
        "tobefilledbyo.e.m.",
        "00000000-0000-0000-0000-000000000000",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
    ];
    (compact.len() >= 8 && !placeholders.contains(&compact.as_str())).then_some(compact)
}

fn random_id(prefix: &str, bytes: usize) -> String {
    let mut data = vec![0_u8; bytes];
    rand::rng().fill_bytes(&mut data);
    format!(
        "{prefix}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_placeholder_hardware_values() {
        assert!(normalize_hardware_value("00000000-0000-0000-0000-000000000000").is_none());
        assert!(normalize_hardware_value(" real-device-001 ").is_some());
    }

    #[test]
    fn event_ids_are_unique() {
        assert_ne!(random_id("evt_", 18), random_id("evt_", 18));
    }

    #[test]
    fn heartbeat_interval_is_two_minutes() {
        assert_eq!(HEARTBEAT_INTERVAL, Duration::from_secs(120));
    }

    #[test]
    fn legacy_identity_gets_registration_secret() {
        let path = std::env::temp_dir().join(format!(
            "pinvou-telemetry-identity-{}-{}.json",
            std::process::id(),
            random_id("test_", 8)
        ));
        fs::write(
            &path,
            r#"{"version":1,"fallback_claim":"fallback_existing","claim_digest":"digest","device_id":"dev_existing","device_token":"token_existing"}"#,
        )
        .unwrap();
        let identity = load_or_create_identity(&path).unwrap();
        assert!(identity.registration_secret.starts_with("rs_"));
        assert_eq!(identity.device_id.as_deref(), Some("dev_existing"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn outbox_prunes_expired_and_excess_events() {
        let mut connection = Connection::open_in_memory().unwrap();
        let now = OUTBOX_RETENTION.as_millis() as u64 + 10_000;
        initialize_outbox(&mut connection, now).unwrap();
        let transaction = connection.transaction().unwrap();
        transaction
            .execute(
                "INSERT INTO usage_events VALUES ('evt_expired_0001', 1, 1, 1, 1)",
                [],
            )
            .unwrap();
        for index in 0..OUTBOX_MAX_EVENTS + 5 {
            transaction
                .execute(
                    "INSERT INTO usage_events VALUES (?1, ?2, 1, 1, 1)",
                    params![format!("evt_recent_{index:08}"), now + index as u64],
                )
                .unwrap();
        }
        transaction.commit().unwrap();

        prune_outbox(&mut connection, now + OUTBOX_MAX_EVENTS as u64).unwrap();
        let count: usize = connection
            .query_row("SELECT COUNT(*) FROM usage_events", [], |row| row.get(0))
            .unwrap();
        let expired: usize = connection
            .query_row(
                "SELECT COUNT(*) FROM usage_events WHERE event_id = 'evt_expired_0001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, OUTBOX_MAX_EVENTS);
        assert_eq!(expired, 0);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_machine_identity_uses_ioplatformuuid() {
        // On a real Mac, ioreg should always return IOPlatformUUID.
        // This test verifies the macOS branch is wired up correctly.
        // Note: PINVOU_TELEMETRY_DEVICE_CLAIM must not be set, otherwise
        // detect_hardware_identity short-circuits to an override:* claim.
        // 用锁保护:与 linux_update.rs/macos_update.rs 的 ENV_LOCK 模式一致,避免并行
        // 测试修改全局 env var 时数据竞争(edition 2024 后 set_var/remove_var 是 unsafe)。
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::remove_var("PINVOU_TELEMETRY_DEVICE_CLAIM");
        let id = detect_hardware_identity("test_fallback");
        assert!(
            id.claim.starts_with("macos:ioplatformuuid:"),
            "expected macos:ioplatformuuid: prefix, got: {}",
            id.claim
        );
        // UUID is 36 chars (8-4-4-4-12 hex with dashes), so total > prefix + 36
        assert!(
            id.claim.len() > "macos:ioplatformuuid:".len() + 30,
            "UUID too short, got: {}",
            id.claim
        );
    }
}
