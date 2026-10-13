//! Same-handle declared native member materialization; no native authenticity.
//! Other duplicate writers/cursors, namespace/ACL and current rights remain owners.
use super::{PackageCheckError, PackageInspection, bind_manifest, claim_bytes, inspect_members};
use pinvou_upgrade_archive::{
    ArchiveError, ExpectedMember, MaterializedMember, materialize_member,
};
use pinvou_upgrade_cache_object::{CacheObjectError, ExclusiveCacheFile, ReadOnlyCacheHandle};
use pinvou_upgrade_codec::{Document, Error, InputKind, NativeIdentityError, NativeIdentityShape};

#[derive(Clone, Copy)]
pub enum NativeMemberKind<'a> {
    FinalInstaller,
    ExecutionHelper,
    Launcher,
    HelperArtifact(&'a str),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeMaterializeError {
    Package(PackageCheckError),
    Archive(ArchiveError),
    Cache(CacheObjectError),
    Json(Error),
    NativeIdentity(NativeIdentityError),
    InvalidSelector,
    SourceUnavailable,
}
impl std::fmt::Display for NativeMaterializeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Package(e) => e.fmt(f),
            Self::Archive(e) => e.fmt(f),
            Self::Cache(e) => e.fmt(f),
            Self::Json(e) => e.fmt(f),
            Self::NativeIdentity(e) => e.fmt(f),
            Self::InvalidSelector => f.write_str("PACKAGE_NATIVE_SELECTOR_INVALID"),
            Self::SourceUnavailable => f.write_str("PACKAGE_SOURCE_UNAVAILABLE"),
        }
    }
}
impl std::error::Error for NativeMaterializeError {}

/// Original materialized byte facts and complete supplied five-field identity,
/// with one actual readonly destination duplicate. The original destination or
/// other duplicates can still be writable; this is not sealing/writer retirement.
pub struct ObservedNativeMember<'a> {
    member: MaterializedMember<'a>,
    identity: NativeIdentityShape,
    file: ReadOnlyCacheHandle,
}
impl<'a> ObservedNativeMember<'a> {
    pub fn member(&self) -> &MaterializedMember<'a> {
        &self.member
    }
    pub fn identity(&self) -> &NativeIdentityShape {
        &self.identity
    }
    pub fn file(&self) -> &ReadOnlyCacheHandle {
        &self.file
    }
}

/// The owner supplies an independent safe ZIP locator: native claims contain no
/// source path. The locator must match the selected declared size/hash. Metadata
/// and selector gates precede any source handle duplication/reading. Existing
/// complete source/rescan/sync/destination EOF verification performs real copying.
/// Failure can leave untrusted partial destination bytes; no path cleanup/reopen.
pub fn materialize_native_member<'a>(
    source: &ReadOnlyCacheHandle,
    destination: &mut ExclusiveCacheFile,
    input: PackageInspection<'a, '_>,
    kind: NativeMemberKind<'_>,
    member_name: &str,
    mut cancelled: impl FnMut() -> bool,
) -> Result<ObservedNativeMember<'a>, NativeMaterializeError> {
    poll(&mut cancelled)?;
    let envelope = bind_manifest(&input).map_err(NativeMaterializeError::Package)?;
    let signed = envelope.envelope().signed().value();
    let value = match kind {
        NativeMemberKind::FinalInstaller => &signed["finalInstaller"],
        NativeMemberKind::ExecutionHelper => &signed["helper"],
        NativeMemberKind::Launcher => &signed["launcher"],
        NativeMemberKind::HelperArtifact(id) => &signed["helpers"]
            .as_array()
            .ok_or(NativeMaterializeError::InvalidSelector)?
            .iter()
            .find(|helper| helper["helperId"].as_str() == Some(id))
            .ok_or(NativeMaterializeError::InvalidSelector)?["nativeIdentity"],
    };
    let (size, hash) = claim_bytes(value).ok_or(NativeMaterializeError::InvalidSelector)?;
    let expected =
        ExpectedMember::new(member_name, size, hash).map_err(NativeMaterializeError::Archive)?;
    let identity = Document::parse(
        &serde_json::to_vec(value)
            .map_err(|_| NativeMaterializeError::Json(Error::Canonicalization))?,
        InputKind::Metadata,
    )
    .map_err(NativeMaterializeError::Json)?
    .native_identity_shape()
    .map_err(NativeMaterializeError::NativeIdentity)?;
    poll(&mut cancelled)?;
    // Duplicate the same readonly OS object/access. Never open the source path.
    // Shared cursor and any other original writers require owner coordination.
    let mut reader = source
        .file()
        .try_clone()
        .map_err(|_| NativeMaterializeError::SourceUnavailable)?;
    inspect_members(
        &mut reader,
        input.artifact,
        input.limits,
        signed,
        &mut cancelled,
    )
    .map_err(NativeMaterializeError::Package)?;
    let member = materialize_member(
        &mut reader,
        destination.file_mut(),
        input.artifact,
        expected,
        input.limits,
        &mut cancelled,
    )
    .map_err(NativeMaterializeError::Archive)?;
    poll(&mut cancelled)?;
    let file = destination
        .read_only_duplicate()
        .map_err(NativeMaterializeError::Cache)?;
    poll(&mut cancelled)?;
    Ok(ObservedNativeMember {
        member,
        identity,
        file,
    })
}
fn poll(cancelled: &mut impl FnMut() -> bool) -> Result<(), NativeMaterializeError> {
    if cancelled() {
        Err(NativeMaterializeError::Archive(ArchiveError::Cancelled))
    } else {
        Ok(())
    }
}
