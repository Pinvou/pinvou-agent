//! This host profile requires Windows native SCM, process, ACL and TxR semantics.
use crate::error::{Error, Result};
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
