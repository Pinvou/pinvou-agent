use sha2::{Digest, Sha256};

use super::models::{LlmApiError, LlmApiErrorCode, LlmApiIdentity};

pub const PINVOU_BOUND_BIOS_SN_SHA256_ENV: &str = "PINVOU3_BOUND_BIOS_SN_SHA256";

pub trait IdentityResolver {
    fn resolve_identity(&self) -> Result<LlmApiIdentity, LlmApiError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemIdentityResolver;

impl IdentityResolver for SystemIdentityResolver {
    fn resolve_identity(&self) -> Result<LlmApiIdentity, LlmApiError> {
        resolve_current_identity()
    }
}

pub fn resolve_current_identity() -> Result<LlmApiIdentity, LlmApiError> {
    #[cfg(not(target_os = "windows"))]
    {
        return Err(LlmApiError::new(
            LlmApiErrorCode::UnsupportedPlatform,
            "LLM API Hub is currently only available on Windows",
            false,
        ));
    }

    #[cfg(target_os = "windows")]
    {
        let bios_sn = crate::platform::os::bios_serial_number().map_err(|err| {
            LlmApiError::new(
                LlmApiErrorCode::DeviceBindingFailed,
                format!("Failed to read device binding information: {err}"),
                true,
            )
        })?;
        let bound_hash = std::env::var(PINVOU_BOUND_BIOS_SN_SHA256_ENV)
            .ok()
            .map(|v| v.trim().to_ascii_lowercase())
            .filter(|v| !v.is_empty());

        resolve_identity_from_parts(&bios_sn, bound_hash.as_deref())
    }
}

pub fn resolve_identity_from_parts(
    bios_sn: &str,
    expected_bios_hash: Option<&str>,
) -> Result<LlmApiIdentity, LlmApiError> {
    let normalized = normalize_bios_serial(bios_sn).ok_or_else(|| {
        LlmApiError::new(
            LlmApiErrorCode::DeviceBindingFailed,
            "Device BIOS SN is empty or invalid",
            true,
        )
    })?;
    let bios_sn_hash = sha256_hex(normalized.as_bytes());

    if let Some(expected) = expected_bios_hash {
        let expected = expected.trim().to_ascii_lowercase();
        if expected.is_empty() {
            return Err(LlmApiError::new(
                LlmApiErrorCode::DeviceNotBound,
                "Current device is not bound",
                false,
            ));
        }
        if expected != bios_sn_hash {
            return Err(LlmApiError::new(
                LlmApiErrorCode::DeviceBindingFailed,
                "Current device does not match the bound device",
                false,
            ));
        }
    }

    let device_binding_id = device_binding_id_from_hash(&bios_sn_hash);
    Ok(LlmApiIdentity {
        pinvou_user_id: device_binding_id.clone(),
        device_binding_id,
        bios_sn_hash,
    })
}

pub fn normalize_bios_serial(input: &str) -> Option<String> {
    let normalized = input
        .trim()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    if normalized.is_empty()
        || matches!(
            normalized.as_str(),
            "DEFAULTSTRING" | "TOBEFILLEDBYO.E.M." | "SYSTEMSERIALNUMBER" | "NONE" | "UNKNOWN"
        )
    {
        None
    } else {
        Some(normalized)
    }
}

pub fn sha256_hex(input: &[u8]) -> String {
    let digest = Sha256::digest(input);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn device_binding_id_from_hash(hash: &str) -> String {
    let prefix = hash.chars().take(16).collect::<String>();
    format!("dev_{prefix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_bios_serial() {
        assert_eq!(
            normalize_bios_serial(" abc 123 "),
            Some("ABC123".to_string())
        );
        assert_eq!(normalize_bios_serial("Default String"), None);
        assert_eq!(normalize_bios_serial("unknown"), None);
    }

    #[test]
    fn derives_stable_device_binding_id() {
        let identity = resolve_identity_from_parts("abc-123", None).unwrap();
        let again = resolve_identity_from_parts(" ABC-123 ", None).unwrap();
        assert_eq!(identity.device_binding_id, again.device_binding_id);
        assert_eq!(identity.pinvou_user_id, identity.device_binding_id);
        assert_eq!(identity.bios_sn_hash, again.bios_sn_hash);
        assert!(identity.device_binding_id.starts_with("dev_"));
    }

    #[test]
    fn rejects_missing_bios_serial() {
        let err = resolve_identity_from_parts("", None).unwrap_err();
        assert_eq!(err.code, LlmApiErrorCode::DeviceBindingFailed);
    }

    #[test]
    fn rejects_mismatched_binding_hash() {
        let err = resolve_identity_from_parts("abc", Some("deadbeef")).unwrap_err();
        assert_eq!(err.code, LlmApiErrorCode::DeviceBindingFailed);
    }

    #[test]
    fn accepts_matching_binding_hash() {
        let hash = sha256_hex("ABC".as_bytes());
        let identity = resolve_identity_from_parts("abc", Some(&hash)).unwrap();
        assert_eq!(identity.bios_sn_hash, hash);
        assert_eq!(identity.pinvou_user_id, identity.device_binding_id);
    }
}
