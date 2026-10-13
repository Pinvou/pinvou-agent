//! Supplied Release-to-Package identity relationship, not trusted chain selection.
use crate::{
    BoundedOutput, Document, EnvelopeShapeError, Error, MetadataReferenceError,
    SchemaCheckedEnvelope, SchemaError,
};
use serde_json::Value;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleasePackageError {
    Json(Error),
    Envelope(EnvelopeShapeError),
    Schema(SchemaError),
    Reference(MetadataReferenceError),
    RoleInvalid,
    TargetInvalid,
    TargetMissing,
    BindingMismatch,
}
impl std::fmt::Display for ReleasePackageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => e.fmt(f),
            Self::Envelope(e) => e.fmt(f),
            Self::Schema(e) => e.fmt(f),
            Self::Reference(e) => e.fmt(f),
            Self::RoleInvalid => f.write_str("RELEASE_PACKAGE_ROLE_INVALID"),
            Self::TargetInvalid => f.write_str("RELEASE_TARGET_INVALID"),
            Self::TargetMissing => f.write_str("RELEASE_TARGET_MISSING"),
            Self::BindingMismatch => f.write_str("RELEASE_PACKAGE_MISMATCH"),
        }
    }
}
impl std::error::Error for ReleasePackageError {}
/// Exact verifyReleasePackage reference and post-verification binding subset.
/// Both envelopes have full Schema/encoding; target is a bounded known target key.
/// This neither performs verifyEnvelope nor authenticates the provided product,
/// component, time or target authority. No package is selected for execution.
pub fn assert_supplied_release_package_binding(
    release: &SchemaCheckedEnvelope,
    package: &SchemaCheckedEnvelope,
    target_key: &Document,
) -> Result<(), ReleasePackageError> {
    let release = release.envelope().signed().value();
    let claims = package.envelope().signed().value();
    if release["role"] != "release" || claims["role"] != "package" {
        return Err(ReleasePackageError::RoleInvalid);
    }
    let key = target_key
        .value()
        .as_str()
        .filter(|key| crate::shape::TARGETS.contains(key))
        .ok_or(ReleasePackageError::TargetInvalid)?;
    let target = release["targets"]
        .as_array()
        .and_then(|targets| targets.iter().find(|entry| entry["targetKey"] == key))
        .ok_or(ReleasePackageError::TargetMissing)?;
    // Clone only a full-Schema/resource-checked, small closed reference subtree.
    // The existing exact raw-envelope checker performs the six-field comparison.
    Document {
        value: target["packageManifest"].clone(),
    }
    .metadata_reference_shape()
    .map_err(ReleasePackageError::Reference)?
    .assert_original_envelope(package.envelope().raw_bytes())
    .map_err(ReleasePackageError::Reference)?;
    assert_target_binding(release, target, claims)
}

/// Parent-only reuse for the exact concrete target in public-graph iteration.
/// Callers supply these borrowed subtrees from full-Schema envelope factories.
pub(super) fn assert_target_binding(
    release: &Value,
    target: &Value,
    claims: &Value,
) -> Result<(), ReleasePackageError> {
    let package_modes = claims["activationModes"]
        .as_array()
        .ok_or(ReleasePackageError::BindingMismatch)?;
    let modes = target["activationModes"]
        .as_array()
        .ok_or(ReleasePackageError::BindingMismatch)?;
    if claims["scope"]["targetKey"] != target["targetKey"]
        || claims["appVersion"] != release["appVersion"]
        || canonical(&claims["launcher"])? != canonical(&target["launcher"])?
        || !modes.iter().all(|mode| package_modes.contains(mode))
    {
        return Err(ReleasePackageError::BindingMismatch);
    }
    Ok(())
}
fn canonical(value: &Value) -> Result<Vec<u8>, ReleasePackageError> {
    let mut output = BoundedOutput {
        bytes: Vec::new(),
        exceeded: false,
    };
    if serde_jcs::to_writer(&mut output, value).is_err() {
        return Err(ReleasePackageError::Json(if output.exceeded {
            Error::OutputLimit
        } else {
            Error::Canonicalization
        }));
    }
    Ok(output.bytes)
}
