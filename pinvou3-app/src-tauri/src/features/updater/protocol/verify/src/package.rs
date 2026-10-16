//! Signed Package and supplied Release relationship; Release authentication is external.
use crate::{VerificationError, VerificationInputs, verify_with_supplied_root};
use pinvou_upgrade_codec::{
    Document, Error, InputKind, ReleasePackageError, RootPolicyShape, SchemaCheckedEnvelope,
    SuppliedProtocolTime, assert_supplied_release_package_binding,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageBindingVerificationError {
    Verification(VerificationError),
    Binding(ReleasePackageError),
    Json(Error),
}
impl std::fmt::Display for PackageBindingVerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Verification(error) => error.fmt(f),
            Self::Binding(error) => error.fmt(f),
            Self::Json(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for PackageBindingVerificationError {}

/// Existing trust/time, independent product/component/targetKey and global denial.
pub struct PackageBindingInputs<'a> {
    root: &'a RootPolicyShape,
    context: &'a Document,
    now: &'a SuppliedProtocolTime,
    denied: &'a [String],
}
impl<'a> PackageBindingInputs<'a> {
    pub fn new(
        root: &'a RootPolicyShape,
        context: &'a Document,
        now: &'a SuppliedProtocolTime,
    ) -> Self {
        Self {
            root,
            context,
            now,
            denied: &[],
        }
    }
    pub fn with_denied_key_ids(mut self, ids: &'a [String]) -> Self {
        self.denied = ids;
        self
    }
}

/// Only Package is signature checked here. The supplied Release is borrowed
/// Schema material, never authenticated by this proof or usable as current rights.
pub struct SignatureCheckedPackageBinding<'a> {
    package: SchemaCheckedEnvelope,
    supplied_release: &'a SchemaCheckedEnvelope,
    root: &'a RootPolicyShape,
    supplied_now: f64,
}
impl SignatureCheckedPackageBinding<'_> {
    pub fn checked_package(&self) -> &SchemaCheckedEnvelope {
        &self.package
    }
    pub fn supplied_release(&self) -> &SchemaCheckedEnvelope {
        self.supplied_release
    }
    pub fn supplied_root(&self) -> &RootPolicyShape {
        self.root
    }
    pub fn supplied_time_millis(&self) -> f64 {
        self.supplied_now
    }
}

/// Complete frozen verifyReleasePackage semantics. Callers must independently
/// authenticate Release before qualification; this operation does not recheck
/// its product/component, signatures or original window. No partial proof escapes.
pub fn verify_release_package<'a>(
    supplied_release: &'a SchemaCheckedEnvelope,
    package_bytes: &[u8],
    inputs: PackageBindingInputs<'a>,
) -> Result<SignatureCheckedPackageBinding<'a>, PackageBindingVerificationError> {
    let release = supplied_release.envelope().signed().value();
    if release["role"] != "release" {
        return Err(PackageBindingVerificationError::Binding(
            ReleasePackageError::RoleInvalid,
        ));
    }
    let context = inputs.context.value();
    let target = release["targets"]
        .as_array()
        .and_then(|targets| {
            targets
                .iter()
                .find(|target| target["targetKey"] == context["targetKey"])
        })
        .ok_or(PackageBindingVerificationError::Binding(
            ReleasePackageError::TargetMissing,
        ))?;
    parse(target["packageManifest"].to_string().as_bytes())?
        .metadata_reference_shape()
        .map_err(ReleasePackageError::Reference)
        .map_err(PackageBindingVerificationError::Binding)?
        .assert_original_envelope(package_bytes)
        .map_err(ReleasePackageError::Reference)
        .map_err(PackageBindingVerificationError::Binding)?;
    let expected = parse(format!(
        "{{\"role\":\"package\",\"product\":{},\"component\":{},\"scope\":{{\"targetKey\":{}}}}}",
        context["product"], context["component"], context["targetKey"],
    ).as_bytes())?;
    let package = verify_with_supplied_root(
        package_bytes,
        VerificationInputs::new(inputs.root, &expected, inputs.now)
            .with_denied_key_ids(inputs.denied),
    )
    .map_err(PackageBindingVerificationError::Verification)?
    .envelope;
    let target_key = parse(context["targetKey"].to_string().as_bytes())?;
    assert_supplied_release_package_binding(supplied_release, &package, &target_key)
        .map_err(PackageBindingVerificationError::Binding)?;
    Ok(SignatureCheckedPackageBinding {
        package,
        supplied_release,
        root: inputs.root,
        supplied_now: inputs.now.as_millis(),
    })
}
fn parse(bytes: &[u8]) -> Result<Document, PackageBindingVerificationError> {
    Document::parse(bytes, InputKind::Metadata).map_err(PackageBindingVerificationError::Json)
}
