//! Protected byte custody only. No current qualification or execution authority.
use crate::{
    error::{Error, Result},
    platform,
};
use pinvou_upgrade_archive::ArchiveLimits;
use std::fs::File;

mod binding;
pub(crate) use binding::{BoundPackage, bind_package, unique_member};

/// Untrusted borrowed input, not cache ownership, writer retirement or custody.
/// The host independently verifies the signed envelope and protected copy.
/// The caller retains the actual file (and any cache lease) through staging.
pub struct DownloadedPackageSource<'a> {
    pub(crate) file: &'a File,
    pub(crate) envelope: &'a [u8],
    pub(crate) root_claims_pin: &'a str,
    pub(crate) historical_time: u64,
    pub(crate) root_successors: &'a [&'a [u8]],
}
impl<'a> DownloadedPackageSource<'a> {
    pub fn new(
        file: &'a File,
        envelope: &'a [u8],
        root_claims_pin: &'a str,
        historical_time: u64,
    ) -> Result<Self> {
        if !crate::json::digest(root_claims_pin) || historical_time > crate::json::SAFE_MAX {
            return Err(Error::ImageRefused);
        }
        pinvou_upgrade_codec::Document::parse(envelope, pinvou_upgrade_codec::InputKind::Metadata)
            .map_err(|_| Error::ImageRefused)?;
        Ok(Self {
            file,
            envelope,
            root_claims_pin,
            historical_time,
            root_successors: &[],
        })
    }

    /// Borrow an ordered historical Root successor chain rooted ONLY in the
    /// independent compiled anchor. This supplies no replacement trust anchor,
    /// current Root acceptance, trusted current time or installation authority.
    /// Construction checks lengths only; cancellable verification parses/copies
    /// one original metadata object at a time during actual staging.
    pub fn new_with_root_successors(
        file: &'a File,
        envelope: &'a [u8],
        root_claims_pin: &'a str,
        historical_time: u64,
        root_successors: &'a [&'a [u8]],
    ) -> Result<Self> {
        validate_root_successors(root_successors)?;
        let mut source = Self::new(file, envelope, root_claims_pin, historical_time)?;
        source.root_successors = root_successors;
        Ok(source)
    }
}

pub(crate) fn validate_root_successors(successors: &[&[u8]]) -> Result<()> {
    if successors.len() > 4096
        || successors
            .iter()
            .any(|bytes| bytes.is_empty() || bytes.len() > crate::json::RECORD_MAX)
    {
        return Err(Error::InitialTrustUnavailable);
    }
    Ok(())
}

pub(crate) fn poll(cancelled: &mut dyn FnMut() -> bool) -> Result<()> {
    if cancelled() {
        Err(Error::StagingCanceled)
    } else {
        Ok(())
    }
}

pub(crate) fn require_available_space(available: u64, bytes: u64, reserve: u64) -> Result<()> {
    let required = bytes
        .checked_add(reserve)
        .ok_or(Error::PrerequisiteUnavailable)?;
    if available < required {
        return Err(Error::PrerequisiteUnavailable);
    }
    Ok(())
}

/// Closed roles declared by the signed production Package. No arbitrary member
/// path or caller-selected execution-helper ID is accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeRole {
    FinalInstaller,
    ExecutionHelper,
    Launcher,
}
impl NativeRole {
    pub(crate) fn field(self) -> &'static str {
        match self {
            Self::FinalInstaller => "finalInstaller",
            Self::ExecutionHelper => "helper",
            Self::Launcher => "launcher",
        }
    }
    pub(crate) fn member_kind(self) -> pinvou_upgrade_package_check::NativeMemberKind<'static> {
        match self {
            Self::FinalInstaller => pinvou_upgrade_package_check::NativeMemberKind::FinalInstaller,
            Self::ExecutionHelper => {
                pinvou_upgrade_package_check::NativeMemberKind::ExecutionHelper
            }
            Self::Launcher => pinvou_upgrade_package_check::NativeMemberKind::Launcher,
        }
    }
}

/// Resource policy supplied by trusted in-process composition, not metadata
/// authority or an authenticated preparation budget. ArchiveLimits validates
/// its own capacities. Disk reserve is independent of the bytes being staged.
pub struct StagingLimits {
    pub(crate) archive: ArchiveLimits,
    pub(crate) reserve_bytes: u64,
}
impl StagingLimits {
    pub fn new(archive: ArchiveLimits, reserve_bytes: u64) -> Result<Self> {
        if reserve_bytes == 0 || reserve_bytes > crate::json::SAFE_MAX {
            return Err(Error::SecurityRefused);
        }
        Ok(Self {
            archive,
            reserve_bytes,
        })
    }
}

/// Owns protected read-only objects and their directory ancestry until dropped.
/// There is no Clone, serialization, raw path/handle, install or launch API.
/// Neither this type nor its facts are a T03 VerifiedExecutionObject.
pub struct ProtectedByteCapability {
    inner: platform::ProtectedDownloadBytes,
    role: NativeRole,
    size: u64,
    sha256: String,
}
impl ProtectedByteCapability {
    /// Consume this byte owner into a verified same-process object owner after
    /// complete physical revalidation. This adds no current upgrade permission.
    pub fn into_execution_object(
        self,
        cancelled: impl FnMut() -> bool,
    ) -> Result<crate::VerifiedExecutionObject> {
        crate::execution_object::VerifiedExecutionObject::from_staged(self, cancelled)
    }

    pub(crate) fn seal_reference(&self) -> Result<&crate::ObjectReference> {
        self.inner.seal_reference()
    }

    pub fn role(&self) -> NativeRole {
        self.role
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Read a bounded portion of the retained native object. The OS handle and
    /// filesystem path remain private; no cursor or writable duplicate escapes.
    /// Each call reads at most 64 KiB so the owner can check cancellation between calls.
    pub fn read_at(&mut self, offset: u64, output: &mut [u8]) -> Result<usize> {
        self.inner.read_at(offset, output)
    }
    /// Recheck actual protected ancestry, reopened object IDs and full bytes.
    /// Success remains byte custody, not fresh metadata or install permission.
    pub fn recheck(&mut self, mut cancelled: impl FnMut() -> bool) -> Result<()> {
        let mut stopped = false;
        self.inner.recheck(&mut || {
            stopped |= cancelled();
            stopped
        })
    }
}

/// Blocking, in-process composition entry. The ordinary source's Root is never
/// trusted: all supplied successors must chain to the independent compiled Root.
/// Supplied time serves only historical Package/Root semantics, not trusted now.
/// Missing or malformed rotation chains refuse rather than adopting a cache Root.
/// No IPC, automatic request, download slot, execution handler or total deadline
/// is introduced. Failure preserves any partial protected output for diagnosis;
/// it is never adopted, completed, overwritten or returned as a capability.
/// The owner supplies cancellation or its original validation-budget check.
/// This module neither creates nor resets a deadline. Cancellation is latched.
pub fn stage_retained_download(
    source: &DownloadedPackageSource<'_>,
    role: NativeRole,
    limits: &StagingLimits,
    mut cancelled: impl FnMut() -> bool,
) -> Result<ProtectedByteCapability> {
    let mut stopped = false;
    let staged = platform::stage_download_bytes(source, role, limits, &mut || {
        stopped |= cancelled();
        stopped
    })?;
    let size = staged.size();
    let sha256 = staged.sha256().to_owned();
    Ok(ProtectedByteCapability {
        inner: staged,
        role,
        size,
        sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reserve_is_separate_from_validated_archive_capacity() {
        let archive = ArchiveLimits::new(100, 200, 10, 1024, 1024, 100).unwrap();
        assert!(StagingLimits::new(archive, 0).is_err());
        assert!(StagingLimits::new(archive, crate::json::SAFE_MAX + 1).is_err());
        assert!(StagingLimits::new(archive, 4096).is_ok());
    }
    #[test]
    fn space_admission_accounts_for_both_objects_and_preserves_reserve() {
        let full_pack = 8192u64;
        let native = 4096u64;
        let reserve = 1024u64;
        assert!(require_available_space(13_311, full_pack + native, reserve).is_err());
        assert!(require_available_space(13_312, full_pack + native, reserve).is_ok());
        assert!(require_available_space(1023, 0, reserve).is_err());
        assert!(require_available_space(u64::MAX, u64::MAX, reserve).is_err());
    }
    #[test]
    fn borrowed_root_chain_bounds_do_not_parse_or_sum_all_metadata() {
        // Length checks only, not a claim that these deliberately invalid bytes
        // are Root objects. Actual verification parses each bounded object.
        let one = vec![b'x'; crate::json::RECORD_MAX];
        let allowed = vec![one.as_slice(); 4096];
        assert!(validate_root_successors(&allowed).is_ok());
        assert!(validate_root_successors(&vec![one.as_slice(); 4097]).is_err());
        assert!(validate_root_successors(&[&[]]).is_err());
        assert!(validate_root_successors(&[&vec![0; crate::json::RECORD_MAX + 1]]).is_err());
        assert!(validate_root_successors(&[]).is_ok());
    }
}
