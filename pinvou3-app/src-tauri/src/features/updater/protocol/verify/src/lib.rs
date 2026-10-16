//! Original V1 envelope, single-successor and complete-chain checks under supplied Root,
//! expected context, time and deny/high-water facts. This acquires no protected
//! bootstrap trust, persistent acceptance, trusted clock or execution rights.
mod archive;
mod candidate;
mod chain;
mod check;
mod download;
mod envelope;
mod error;
mod graph;
mod package;
mod signatures;
mod successor;
use envelope::parse_bound_envelope;
pub use successor::{ActiveMetadataInput, SignatureCheckedRootSuccessor, verify_root_successor};

pub use archive::{ArchivedEnvelopeIdentity, verify_archived_package, verify_archived_release};
pub use candidate::{
    CandidateVerificationError, CandidateVerificationInputs, SignatureCheckedCandidateRelease,
    verify_candidate_release,
};
pub use chain::{SignatureCheckedRootChain, verify_root_chain};
pub use check::{
    CheckVerificationError, CheckVerificationInputs, SignatureCheckedCheckResponse,
    verify_check_response,
};
pub use download::{
    DownloadVerificationError, DownloadVerificationInputs, SignatureCheckedDownloadResponse,
    verify_download_info_response,
};
pub use error::VerificationError;
pub use graph::{
    PublicChainError, PublicChainInputs, SignatureCheckedPublicChain, verify_public_chain,
};
pub use package::{
    PackageBindingInputs, PackageBindingVerificationError, SignatureCheckedPackageBinding,
    verify_release_package,
};
use pinvou_upgrade_codec::{
    Document, PackageSemanticsMode, RootPolicyShape, RootWindowMode, SchemaCheckedEnvelope,
    SuppliedProtocolTime, assert_supplied_credential_semantics, assert_supplied_high_water,
    assert_supplied_metadata_semantics_in_mode,
};
use std::collections::BTreeSet;

const METADATA_ROLES: [&str; 6] = [
    "root",
    "timestamp",
    "snapshot",
    "target",
    "release",
    "package",
];

/// Mandatory independent inputs; no public fields, Debug, Serialize or wire
/// constructor. The Root must come from the caller's existing trust state,
/// never from the incoming envelope. Defaults retain production package rules.
pub struct VerificationInputs<'a> {
    root: &'a RootPolicyShape,
    expected: &'a Document,
    now: &'a SuppliedProtocolTime,
    high_water: Option<&'a Document>,
    denied_key_ids: &'a [String],
    package_mode: PackageSemanticsMode,
}
impl<'a> VerificationInputs<'a> {
    pub fn new(
        root: &'a RootPolicyShape,
        expected: &'a Document,
        now: &'a SuppliedProtocolTime,
    ) -> Self {
        Self {
            root,
            expected,
            now,
            high_water: None,
            denied_key_ids: &[],
            package_mode: PackageSemanticsMode::Production,
        }
    }
    pub fn with_high_water(mut self, record: &'a Document) -> Self {
        self.high_water = Some(record);
        self
    }
    pub fn with_denied_key_ids(mut self, ids: &'a [String]) -> Self {
        self.denied_key_ids = ids;
        self
    }
    /// Explicit trusted-caller development semantics, never a remote option.
    pub fn with_package_mode(mut self, mode: PackageSemanticsMode) -> Self {
        self.package_mode = mode;
        self
    }
}

/// Preserves original verified bytes and their exact supplied Root lifetime.
/// No constructor/Debug/Serialize/Clone or accepted-state/ACK/permission method.
/// Time and deny/high-water facts remain supplied inputs, not current authority.
pub struct SignatureCheckedEnvelope<'a> {
    envelope: SchemaCheckedEnvelope,
    root: &'a RootPolicyShape,
    signer_ids: BTreeSet<String>,
    supplied_now: f64,
}
impl SignatureCheckedEnvelope<'_> {
    pub fn envelope(&self) -> &SchemaCheckedEnvelope {
        &self.envelope
    }
    pub fn supplied_root(&self) -> &RootPolicyShape {
        self.root
    }
    pub fn verified_signer_ids(&self) -> &BTreeSet<String> {
        &self.signer_ids
    }
    pub fn supplied_time_millis(&self) -> f64 {
        self.supplied_now
    }
}

/// Complete ordinary single-envelope checks. Root successors/archives require
/// separate operations. This entry keeps fresh supplied Root semantics.
pub fn verify_with_supplied_root<'a>(
    bytes: &[u8],
    inputs: VerificationInputs<'a>,
) -> Result<SignatureCheckedEnvelope<'a>, VerificationError> {
    let envelope = parse_bound_envelope(bytes, inputs.expected)?;
    let expected_role = inputs.expected.value()["role"].as_str();
    let denied: BTreeSet<&str> = inputs.denied_key_ids.iter().map(String::as_str).collect();
    if envelope
        .envelope()
        .signatures()
        .iter()
        .any(|signature| denied.contains(signature.key_id()))
    {
        return Err(VerificationError::SigningKeyDenied);
    }
    inputs
        .root
        .assert_supplied_current_window(inputs.now)
        .map_err(VerificationError::Time)?;
    let signed = envelope.envelope().signed();
    if expected_role == Some("root")
        && signed.canonical_bytes().map_err(VerificationError::Json)?
            != inputs.root.canonical_bytes()
    {
        return Err(VerificationError::RootTransitionRequired);
    }
    let signer_ids = signatures::verify_signatures(&envelope, inputs.root)?;
    if expected_role.is_some_and(|role| METADATA_ROLES.contains(&role)) {
        assert_supplied_metadata_semantics_in_mode(
            &envelope,
            inputs.now,
            RootWindowMode::Current,
            inputs.package_mode,
        )
        .map_err(VerificationError::Metadata)?;
        if let Some(record) = inputs.high_water {
            // Includes Package: the original high-water rule rejects it. A
            // credential ignores this argument, matching the frozen entry.
            assert_supplied_high_water(envelope.envelope(), record)
                .map_err(VerificationError::HighWater)?;
        }
    } else {
        assert_supplied_credential_semantics(&envelope, inputs.now)
            .map_err(VerificationError::Credential)?;
    }
    Ok(SignatureCheckedEnvelope {
        envelope,
        root: inputs.root,
        signer_ids,
        supplied_now: inputs.now.as_millis(),
    })
}
