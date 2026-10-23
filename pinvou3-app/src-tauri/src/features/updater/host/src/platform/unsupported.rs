//! This host profile requires Windows native SCM, process, ACL and TxR semantics.
use crate::error::{Error, Result};
pub(crate) struct ProtectedDownloadBytes;
impl ProtectedDownloadBytes {
    pub(crate) fn size(&self) -> u64 {
        0
    }
    pub(crate) fn sha256(&self) -> &str {
        ""
    }
    pub(crate) fn read_at(&mut self, _: u64, _: &mut [u8]) -> Result<usize> {
        Err(Error::Unsupported)
    }
    pub(crate) fn recheck(&mut self, _: &mut dyn FnMut() -> bool) -> Result<()> {
        Err(Error::Unsupported)
    }
}
pub(crate) fn stage_download_bytes(
    _: &crate::download_stage::DownloadedPackageSource<'_>,
    _: crate::download_stage::NativeRole,
    _: &crate::download_stage::StagingLimits,
    _: &mut dyn FnMut() -> bool,
) -> Result<ProtectedDownloadBytes> {
    Err(Error::Unsupported)
}
pub(crate) fn native_supported() -> Result<()> {
    Err(Error::Unsupported)
}

pub(crate) fn prepare_restart() -> crate::error::Result<crate::restart::PreparedRestart> {
    Err(crate::Error::Unsupported)
}

pub(crate) fn harden_role_process() -> Result<()> {
    Err(Error::Unsupported)
}
pub(crate) fn execute_helper(_: crate::entry::HelperMode) -> Result<()> {
    Err(Error::Unsupported)
}
pub(crate) fn execute_launcher(_: Option<&str>) -> Result<()> {
    Err(Error::Unsupported)
}

pub(crate) fn install_local() -> Result<u32> {
    Err(Error::Unsupported)
}
