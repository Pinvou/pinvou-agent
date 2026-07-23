use super::super::{
    identity::{resolve_identity_from_parts, PINVOU_BOUND_BIOS_SN_SHA256_ENV},
    models::{LlmApiError, LlmApiErrorCode, LlmApiIdentity},
};

pub(crate) fn resolve_current_identity() -> Result<LlmApiIdentity, LlmApiError> {
    let bios_sn = crate::platform::os::bios_serial_number().map_err(|err| {
        LlmApiError::new(
            LlmApiErrorCode::DeviceBindingFailed,
            format!("Failed to read device binding information: {err}"),
            true,
        )
    })?;
    let bound_hash = std::env::var(PINVOU_BOUND_BIOS_SN_SHA256_ENV)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());

    resolve_identity_from_parts(&bios_sn, bound_hash.as_deref())
}
