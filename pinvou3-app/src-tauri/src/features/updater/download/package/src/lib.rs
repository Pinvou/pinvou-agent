//! Same actual FullPack ZIP byte binding to supplied canonical Package claims.
//! No signature trust, native publisher proof, protected token or execution right.
use pinvou_upgrade_archive::{ArchiveError, ArchiveLimits, PackageInventory, inspect_file};
use pinvou_upgrade_codec::{
    CanonicalAppVersion, ContextBindingError, Document, EnvelopeShapeError, InputKind,
    MetadataReferenceError, MetadataReferenceShape, MetadataSemanticsError, RootWindowMode,
    SchemaCheckedEnvelope, SchemaError, SuppliedProtocolTime, assert_supplied_expected_context,
    assert_supplied_metadata_semantics, decode_canonical_envelope,
};
use pinvou_upgrade_transfer::Artifact;
mod materialize;
pub use materialize::{
    NativeMaterializeError, NativeMemberKind, ObservedNativeMember, materialize_native_member,
};
use std::{collections::BTreeSet, fs::File};

/// Independent supplied expectations, never a trust/owner token. The caller
/// protects the same source handle and supplies the current qualification.
pub struct PackageInspection<'a, 'b> {
    pub artifact: Artifact<'a>,
    pub manifest_bytes: &'b [u8],
    pub reference: &'b MetadataReferenceShape,
    pub expected_context: &'b Document,
    pub expected_version: &'b CanonicalAppVersion,
    pub supplied_time: &'b SuppliedProtocolTime,
    pub limits: ArchiveLimits,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageCheckError {
    Envelope(EnvelopeShapeError),
    Schema(SchemaError),
    Context(ContextBindingError),
    Reference(MetadataReferenceError),
    Semantics(MetadataSemanticsError),
    WrongRole,
    VersionMismatch,
    ArtifactMismatch,
    NativeMemberMissing,
    Archive(ArchiveError),
}
impl std::fmt::Display for PackageCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Envelope(e) => e.fmt(f),
            Self::Schema(e) => e.fmt(f),
            Self::Context(e) => e.fmt(f),
            Self::Reference(e) => e.fmt(f),
            Self::Semantics(e) => e.fmt(f),
            Self::Archive(e) => e.fmt(f),
            Self::WrongRole => f.write_str("PACKAGE_ROLE_REQUIRED"),
            Self::VersionMismatch => f.write_str("PACKAGE_VERSION_MISMATCH"),
            Self::ArtifactMismatch => f.write_str("PACKAGE_ARTIFACT_MISMATCH"),
            Self::NativeMemberMissing => f.write_str("PACKAGE_NATIVE_MEMBER_MISSING"),
        }
    }
}
impl std::error::Error for PackageCheckError {}

/// Synchronous work on the owner's blocking worker. All metadata gates precede
/// filesystem access. No inventory or native selection escapes on partial failure.
/// Direct members remain opaque; native identity/publisher checks are still T03.
pub fn inspect_full_pack<'a>(
    source: &mut File,
    input: PackageInspection<'a, '_>,
    mut cancelled: impl FnMut() -> bool,
) -> Result<PackageInventory<'a>, PackageCheckError> {
    if cancelled() {
        return Err(PackageCheckError::Archive(ArchiveError::Cancelled));
    }
    let envelope = bind_manifest(&input)?;
    inspect_members(
        source,
        input.artifact,
        input.limits,
        envelope.envelope().signed().value(),
        &mut cancelled,
    )
}

fn bind_manifest(
    input: &PackageInspection<'_, '_>,
) -> Result<SchemaCheckedEnvelope, PackageCheckError> {
    let envelope = decode_canonical_envelope(input.manifest_bytes, InputKind::Metadata)
        .map_err(PackageCheckError::Envelope)?
        .check_full_schema()
        .map_err(PackageCheckError::Schema)?;
    let signed = envelope.envelope().signed().value();
    if signed["role"] != "package" {
        return Err(PackageCheckError::WrongRole);
    }
    assert_supplied_expected_context(&envelope, input.expected_context)
        .map_err(PackageCheckError::Context)?;
    input
        .reference
        .assert_original_envelope(input.manifest_bytes)
        .map_err(PackageCheckError::Reference)?;
    assert_supplied_metadata_semantics(&envelope, input.supplied_time, RootWindowMode::Current)
        .map_err(PackageCheckError::Semantics)?;
    if signed["appVersion"] != input.expected_version.as_str() {
        return Err(PackageCheckError::VersionMismatch);
    }
    let full = claim_bytes(&signed["fullPackage"]).ok_or(PackageCheckError::ArtifactMismatch)?;
    if signed["fullPackage"]["packageId"] != input.artifact.package_id()
        || full != (input.artifact.size(), input.artifact.sha256())
    {
        return Err(PackageCheckError::ArtifactMismatch);
    }
    Ok(envelope)
}

fn inspect_members<'a>(
    source: &mut File,
    artifact: Artifact<'a>,
    limits: ArchiveLimits,
    signed: &serde_json::Value,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<PackageInventory<'a>, PackageCheckError> {
    let inventory = inspect_file(source, artifact, limits, &mut *cancelled)
        .map_err(PackageCheckError::Archive)?;
    // Index once. Repeated equivalent native identities may bind the same member;
    // V1 provides no native source path, and installationTarget is not one.
    let present: BTreeSet<_> = inventory
        .members()
        .iter()
        .map(|m| (m.size(), m.sha256()))
        .collect();
    for identity in ["finalInstaller", "helper", "launcher"]
        .iter()
        .map(|name| &signed[*name])
        .chain(
            signed["helpers"]
                .as_array()
                .ok_or(PackageCheckError::NativeMemberMissing)?
                .iter()
                .map(|helper| &helper["nativeIdentity"]),
        )
    {
        if cancelled() {
            return Err(PackageCheckError::Archive(ArchiveError::Cancelled));
        }
        let expected = claim_bytes(identity).ok_or(PackageCheckError::NativeMemberMissing)?;
        if !present.contains(&expected) {
            return Err(PackageCheckError::NativeMemberMissing);
        }
    }
    if cancelled() {
        return Err(PackageCheckError::Archive(ArchiveError::Cancelled));
    }
    Ok(inventory)
}
fn claim_bytes(value: &serde_json::Value) -> Option<(u64, [u8; 32])> {
    // Full frozen Schema already proves safe positive integral binary64 size.
    // as_f64 preserves original acceptance of integral JSON number spellings.
    let size = value["size"].as_f64()?;
    if !size.is_finite() || size.fract() != 0.0 || !(1.0..=9_007_199_254_740_991.0).contains(&size)
    {
        return None;
    }
    let identity = Artifact::new(
        "member-byte-identity",
        size as u64,
        value["sha256"].as_str()?,
    )
    .ok()?;
    Some((identity.size(), identity.sha256()))
}

#[cfg(test)]
mod tests;
