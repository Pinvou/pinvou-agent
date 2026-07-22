use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::time::timeout;

use crate::platform::paths;

#[allow(dead_code)]
pub(crate) const FORMAL_BOOTSTRAP_HOST: &str = "https://bootstrap.magic.h3c.com";
// Internal beta default. Switch back to FORMAL_BOOTSTRAP_HOST for formal releases.
pub(crate) const DEFAULT_BOOTSTRAP_HOST: &str = "https://sohord10.h3c.com";
pub(crate) const FALLBACK_SN: &str = "219904A17T4257W00018";
pub(crate) const SMARTHUB_OTA_KEY: &str = "smarthubOta";
const MISSING_BIOS_SN_ERROR: &str = "读取设备 BIOS SN 失败，无法执行更新查询";

const BOOTSTRAP_PATH: &str = "/v2/bootstrap";
const PRODUCT_ID: &str = "61de63cd22271b82ccd9e1bc258b55e0";
const SECRET_KEY: &str = "664a7836315deb989e5f1451b5860774";
const SIGN_TYPE: &str = "0";

#[derive(Debug, Clone)]
pub(crate) struct BootstrapResolution {
    pub ota_host: String,
    pub sn: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowsOtaBootstrapConfig {
    pub bootstrap_host: String,
    pub source: BootstrapConfigSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BootstrapConfigSource {
    Default,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowsBootstrapIdentity {
    pub raw_bios_sn: Option<String>,
    pub effective_sn: String,
    pub source: BootstrapSnSource,
    pub matched_prefix: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BootstrapSnSource {
    Bios,
    Fallback,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapConfigFile {
    #[serde(default)]
    bootstrap_host: String,
}

#[derive(Debug, Serialize)]
struct DomainBootstrapRequest {
    device_id: String,
    product_id: String,
    timestamp: String,
    sign: String,
    sign_type: String,
}

#[derive(Debug, Deserialize)]
struct DomainBootstrapResponse {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    code: i64,
    #[serde(default, alias = "msg")]
    message: String,
    data: Option<HashMap<String, String>>,
}

impl DomainBootstrapResponse {
    fn is_success(&self) -> bool {
        matches!(self.code, 0 | 200) && self.success.unwrap_or(true)
    }
}

impl WindowsOtaBootstrapConfig {
    pub(crate) fn load() -> Self {
        let path = bootstrap_config_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default_config();
        };
        let text = text.trim_start_matches('\u{feff}');
        if text.trim().is_empty() {
            return Self::default_config();
        }
        let Ok(file) = serde_json::from_str::<BootstrapConfigFile>(text) else {
            return Self::default_config();
        };
        match normalize_http_url(&file.bootstrap_host) {
            Some(bootstrap_host) => Self {
                bootstrap_host,
                source: BootstrapConfigSource::File,
            },
            None => Self::default_config(),
        }
    }

    fn default_config() -> Self {
        Self {
            bootstrap_host: DEFAULT_BOOTSTRAP_HOST.to_string(),
            source: BootstrapConfigSource::Default,
        }
    }
}

fn bootstrap_config_path() -> PathBuf {
    paths::pinvou3_home().join("windows-ota-bootstrap.json")
}

pub(crate) async fn resolve_ota_host(
    client: &reqwest::Client,
) -> Result<BootstrapResolution, String> {
    let config = WindowsOtaBootstrapConfig::load();
    let identity = WindowsBootstrapIdentity::from_bios_sn(read_bios_sn().await.as_deref());
    let ota_host = request_smarthub_ota(client, &config.bootstrap_host, &identity.effective_sn)
        .await
        .map_err(|e| format!("获取域名引导失败：{e}"))?;
    let update_sn = identity
        .update_sn()
        .ok_or_else(|| MISSING_BIOS_SN_ERROR.to_string())?;
    Ok(BootstrapResolution {
        ota_host,
        sn: update_sn.to_string(),
    })
}

pub(crate) async fn request_smarthub_ota(
    client: &reqwest::Client,
    bootstrap_host: &str,
    device_id: &str,
) -> Result<String, String> {
    let host = normalize_http_url(bootstrap_host)
        .ok_or_else(|| "域名引导地址无效，无法请求更新服务".to_string())?;
    let timestamp = Utc::now().timestamp_millis().to_string();
    let request = DomainBootstrapRequest::new(device_id, &timestamp);
    let url = format!("{host}{BOOTSTRAP_PATH}");
    let response = client
        .post(&url)
        .json(&request)
        .send()
        .await
        .map_err(|e| format_reqwest_error("域名引导请求失败", &e))?
        .error_for_status()
        .map_err(|e| format_reqwest_error("域名引导响应异常", &e))?
        .json::<DomainBootstrapResponse>()
        .await
        .map_err(|e| format_reqwest_error("域名引导响应解析失败", &e))?;
    parse_smarthub_ota(response)
}

impl DomainBootstrapRequest {
    fn new(device_id: &str, timestamp: &str) -> Self {
        Self {
            device_id: device_id.to_string(),
            product_id: PRODUCT_ID.to_string(),
            timestamp: timestamp.to_string(),
            sign: bootstrap_sign(device_id, timestamp),
            sign_type: SIGN_TYPE.to_string(),
        }
    }
}

impl WindowsBootstrapIdentity {
    pub(crate) fn from_bios_sn(raw: Option<&str>) -> Self {
        let trimmed = raw.map(str::trim).filter(|v| !v.is_empty());
        let matched = trimmed.is_some_and(is_target_sn);
        if let Some(sn) = trimmed.filter(|_| matched) {
            return Self {
                raw_bios_sn: Some(sn.to_string()),
                effective_sn: sn.to_string(),
                source: BootstrapSnSource::Bios,
                matched_prefix: true,
            };
        }
        Self {
            raw_bios_sn: trimmed.map(ToString::to_string),
            effective_sn: FALLBACK_SN.to_string(),
            source: BootstrapSnSource::Fallback,
            matched_prefix: false,
        }
    }

    pub(crate) fn update_sn(&self) -> Option<&str> {
        self.raw_bios_sn.as_deref()
    }
}

fn parse_smarthub_ota(response: DomainBootstrapResponse) -> Result<String, String> {
    if !response.is_success() {
        return Err(format!(
            "请求域名引导服务失败：code={} msg={}",
            response.code, response.message
        ));
    }
    let data = response
        .data
        .ok_or_else(|| "域名引导服务返回的数据为空".to_string())?;
    let (_, value) = data
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(SMARTHUB_OTA_KEY))
        .ok_or_else(|| format!("通过域名引导未找到对应的服务地址，key：{SMARTHUB_OTA_KEY}"))?;
    normalize_http_url(value).ok_or_else(|| "域名引导返回的 OTA 地址无效".to_string())
}

fn format_reqwest_error(context: &str, error: &reqwest::Error) -> String {
    let mut message = if error.is_timeout() {
        format!("{context}: 连接超时，请确认域名引导服务地址可访问")
    } else if error.is_connect() {
        format!("{context}: 无法连接域名引导服务，请确认网络、VPN 或防火墙配置")
    } else {
        format!("{context}: {error}")
    };
    let mut source = error.source();
    while let Some(err) = source {
        message.push_str(&format!("；{err}"));
        source = err.source();
    }
    message
}

fn normalize_http_url(value: &str) -> Option<String> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let parsed = reqwest::Url::parse(trimmed).ok()?;
    let scheme_ok = matches!(parsed.scheme(), "http" | "https");
    if scheme_ok && parsed.host_str().is_some() {
        Some(trimmed.to_string())
    } else {
        None
    }
}

fn bootstrap_sign(device_id: &str, timestamp: &str) -> String {
    let input = format!(
        "device_id={device_id}&product_id={PRODUCT_ID}&secret={SECRET_KEY}&sign_type={SIGN_TYPE}&keys={timestamp}"
    );
    format!("{:x}", md5::compute(input.as_bytes()))
}

fn is_target_sn(sn: &str) -> bool {
    sn.starts_with("2198") || sn.starts_with("2199")
}

async fn read_bios_sn() -> Option<String> {
    let output = timeout(Duration::from_secs(3), async {
        crate::process::HiddenTokioCommand::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "try { (Get-CimInstance -ClassName Win32_BIOS -ErrorAction Stop).SerialNumber } catch { '' }",
        ])
        .output()
        .await
    })
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let sn = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sn.is_empty() {
        None
    } else {
        Some(sn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(json: &str) -> DomainBootstrapResponse {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn bootstrap_sign_matches_csharp_contract() {
        assert_eq!(
            bootstrap_sign(FALLBACK_SN, "1700000000000"),
            "39e4e4aa0e6e9acb7377e97f25dc8c45"
        );
    }

    #[test]
    fn parse_smarthub_ota_accepts_message_alias_and_case_insensitive_key() {
        let ota = parse_smarthub_ota(response(
            r#"{"success":true,"code":200,"msg":"操作成功","data":{"SMARTHubOTA":"https://api.example.com/"}}"#,
        ))
        .unwrap();
        assert_eq!(ota, "https://api.example.com");
    }

    #[test]
    fn parse_smarthub_ota_accepts_magic_bootstrap_code_zero() {
        let ota = parse_smarthub_ota(response(
            r#"{"code":0,"data":{"upgrade":"https://magic.h3c.com","smarthubOta":"https://wisdomscreen.h3c.com"}}"#,
        ))
        .unwrap();
        assert_eq!(ota, "https://wisdomscreen.h3c.com");
    }

    #[test]
    fn parse_smarthub_ota_rejects_missing_data_key_and_invalid_url() {
        assert!(parse_smarthub_ota(response(
            r#"{"success":true,"code":200,"message":"操作成功"}"#
        ))
        .is_err());
        assert!(parse_smarthub_ota(response(
            r#"{"success":true,"code":200,"message":"操作成功","data":{"mqtt":"https://mqtt.example.com"}}"#,
        ))
        .is_err());
        assert!(parse_smarthub_ota(response(
            r#"{"success":true,"code":200,"message":"操作成功","data":{"smarthubOta":"not-a-url"}}"#,
        ))
        .is_err());
    }

    #[test]
    fn bootstrap_config_path_respects_home_override() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-bootstrap-path-test");
        let root = crate::os::platform_compat_path("/tmp/pinvou3-bootstrap-path-test");
        assert_eq!(
            bootstrap_config_path(),
            root.join("windows-ota-bootstrap.json")
        );
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }

    #[test]
    fn bootstrap_config_defaults_when_file_missing_empty_invalid_or_bad_url() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let root =
            std::env::temp_dir().join(format!("pinvou3-bootstrap-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("PINVOU3_HOME", &root);
        let path = bootstrap_config_path();

        assert_eq!(
            WindowsOtaBootstrapConfig::load().bootstrap_host,
            DEFAULT_BOOTSTRAP_HOST
        );
        std::fs::write(&path, "  ").unwrap();
        assert_eq!(
            WindowsOtaBootstrapConfig::load().bootstrap_host,
            DEFAULT_BOOTSTRAP_HOST
        );
        std::fs::write(&path, "{bad").unwrap();
        assert_eq!(
            WindowsOtaBootstrapConfig::load().bootstrap_host,
            DEFAULT_BOOTSTRAP_HOST
        );
        std::fs::write(&path, r#"{"bootstrapHost":"ftp://bad.example.com"}"#).unwrap();
        assert_eq!(
            WindowsOtaBootstrapConfig::load().bootstrap_host,
            DEFAULT_BOOTSTRAP_HOST
        );

        let _ = std::fs::remove_dir_all(&root);
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }

    #[test]
    fn bootstrap_config_uses_valid_custom_host_without_overwriting_file() {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let root =
            std::env::temp_dir().join(format!("pinvou3-bootstrap-custom-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("PINVOU3_HOME", &root);
        let path = bootstrap_config_path();
        let raw = "\u{feff}{\"bootstrapHost\":\"http://127.0.0.1:8788/\"}";
        std::fs::write(&path, raw).unwrap();

        let config = WindowsOtaBootstrapConfig::load();
        assert_eq!(config.bootstrap_host, "http://127.0.0.1:8788");
        assert_eq!(config.source, BootstrapConfigSource::File);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);

        let _ = std::fs::remove_dir_all(&root);
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }

    #[test]
    fn identity_uses_bios_sn_only_for_target_prefixes() {
        let id = WindowsBootstrapIdentity::from_bios_sn(Some(" 2198ABC "));
        assert_eq!(id.effective_sn, "2198ABC");
        assert_eq!(id.update_sn(), Some("2198ABC"));
        assert_eq!(id.source, BootstrapSnSource::Bios);
        assert!(id.matched_prefix);

        let id = WindowsBootstrapIdentity::from_bios_sn(Some("2199XYZ"));
        assert_eq!(id.effective_sn, "2199XYZ");
        assert_eq!(id.update_sn(), Some("2199XYZ"));
        assert_eq!(id.source, BootstrapSnSource::Bios);

        let id = WindowsBootstrapIdentity::from_bios_sn(Some("1199XYZ"));
        assert_eq!(id.effective_sn, FALLBACK_SN);
        assert_eq!(id.update_sn(), Some("1199XYZ"));
        assert_eq!(id.source, BootstrapSnSource::Fallback);
        assert!(!id.matched_prefix);

        for raw in [Some(""), Some("   "), None] {
            let id = WindowsBootstrapIdentity::from_bios_sn(raw);
            assert_eq!(id.effective_sn, FALLBACK_SN);
            assert_eq!(id.update_sn(), None);
            assert_eq!(id.source, BootstrapSnSource::Fallback);
            assert!(!id.matched_prefix);
        }
    }

    #[test]
    fn request_body_uses_effective_sn() {
        let id = WindowsBootstrapIdentity::from_bios_sn(Some("BADSN"));
        let req = DomainBootstrapRequest::new(&id.effective_sn, "1700000000000");
        let value = serde_json::to_value(req).unwrap();
        assert_eq!(value["device_id"], FALLBACK_SN);
        assert_eq!(value["product_id"], PRODUCT_ID);
        assert_eq!(value["sign_type"], SIGN_TYPE);
        assert_eq!(value["sign"], "39e4e4aa0e6e9acb7377e97f25dc8c45");
    }
}
