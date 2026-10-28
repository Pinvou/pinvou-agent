//! App feature adapter: synchronous host calls are executed by the command's
//! blocking worker. No UI DTO grants native authority or opens another origin.
#[cfg(any(feature = "upgrade-host-lab", feature = "upgrade-host-production"))]
pub use pinvou_upgrade_host::{TrustedTimeRefreshReply, TrustedTimeStatus};

#[cfg(not(any(feature = "upgrade-host-lab", feature = "upgrade-host-production")))]
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedTimeStatus {
    protocol_version: u8,
    state: &'static str,
    refreshing: bool,
    refresh_failure: Option<&'static str>,
}
#[cfg(not(any(feature = "upgrade-host-lab", feature = "upgrade-host-production")))]
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedTimeRefreshReply {
    protocol_version: u8,
    state: &'static str,
}

pub(crate) fn supported() -> bool {
    #[cfg(any(feature = "upgrade-host-lab", feature = "upgrade-host-production"))]
    {
        pinvou_upgrade_host::trusted_time_diagnostics_supported()
    }
    #[cfg(not(any(feature = "upgrade-host-lab", feature = "upgrade-host-production")))]
    {
        false
    }
}
pub(crate) fn inspect() -> Result<TrustedTimeStatus, String> {
    #[cfg(any(feature = "upgrade-host-lab", feature = "upgrade-host-production"))]
    {
        pinvou_upgrade_host::inspect_trusted_time().map_err(|e| e.code().to_owned())
    }
    #[cfg(not(any(feature = "upgrade-host-lab", feature = "upgrade-host-production")))]
    {
        Ok(TrustedTimeStatus {
            protocol_version: 1,
            state: "unsupported",
            refreshing: false,
            refresh_failure: None,
        })
    }
}
pub(crate) fn request_refresh() -> Result<TrustedTimeRefreshReply, String> {
    #[cfg(any(feature = "upgrade-host-lab", feature = "upgrade-host-production"))]
    {
        pinvou_upgrade_host::request_trusted_time_refresh().map_err(|e| e.code().to_owned())
    }
    #[cfg(not(any(feature = "upgrade-host-lab", feature = "upgrade-host-production")))]
    {
        Err("HOST_UNSUPPORTED".to_owned())
    }
}
