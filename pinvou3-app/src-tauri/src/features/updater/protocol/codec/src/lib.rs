//! Bounded Upgrade V1 JSON and canonical bytes. Successful decoding is not
//! signature verification, established Root trust, current eligibility or ACK.

mod client_request;
pub use client_request::{ClientRequestError, ClientRequestOperation};
mod event_acknowledgements;
pub use event_acknowledgements::EventAcknowledgementError;
mod client_http_response;
pub use client_http_response::ClientHttpResponseError;
mod client_error_response;
pub use client_error_response::ClientErrorResponseError;
mod client_response;
pub use client_response::{ClientResponseError, ClientResponseOperation};
mod channel_change_response;
pub use channel_change_response::ChannelChangeResponseError;
mod helper_cleanup_response;
pub use helper_cleanup_response::HelperCleanupResponseError;
mod receipt_response;
pub use receipt_response::{ReceiptResponseError, ReceiptResponseOperation};
mod task_create_response;
pub use task_create_response::{TaskCreateResponseError, TaskCreateResponseOperation};
mod transaction_control;
pub use transaction_control::{TransactionControlError, TransactionControlOperation};
mod purpose_response;
pub use purpose_response::{PurposeResponseError, PurposeResponseOperation};
mod download_info;
pub use download_info::DownloadInfoContractError;
mod check_response;
pub use check_response::{CheckResponseError, CheckResponseOperation};
mod public_graph;
pub use public_graph::{PublicGraphError, assert_supplied_public_graph};
mod public_graph_source;
pub use public_graph_source::assert_supplied_public_graph_source;
mod candidate;
pub use candidate::{CandidateBindingError, assert_supplied_candidate_binding};
mod release_package;
pub use release_package::{ReleasePackageError, assert_supplied_release_package_binding};
mod renewal;
pub use renewal::{ReleaseRenewalError, assert_supplied_release_renewal};
mod bindings;
mod credential_semantics;
pub use credential_semantics::{CredentialSemanticsError, assert_supplied_credential_semantics};
mod metadata_semantics;
pub use metadata_semantics::{
    MetadataSemanticsError, PackageSemanticsMode, RootWindowMode, SuppliedProtocolTime,
    assert_supplied_metadata_semantics, assert_supplied_metadata_semantics_in_mode,
};
mod context;
pub use context::{ContextBindingError, assert_supplied_expected_context};
mod schema;
pub use schema::{SchemaCheckedEnvelope, SchemaError};
mod high_water;
pub use high_water::{HighWaterError, assert_supplied_high_water};
mod envelope;
pub use envelope::{
    CanonicalEnvelopeShape, EnvelopeShapeError, SignatureRecordShape, decode_canonical_envelope,
};
mod consume;
mod consume_binding;
pub use consume::{ConsumeDigestError, ConsumeRequestDigest};
pub use consume_binding::{
    ConsumeBindingError, ConsumeRequestShape, assert_supplied_consume_binding,
};
mod native;
mod parse;
pub use native::{NativeIdentityError, NativeIdentityShape, assert_native_identity};
mod policy;
pub use policy::{DeclaredRolePolicy, PolicyLookupError, RoleContextShape};
mod opening;
pub use opening::{OpeningBindingError, assert_rollout_opening};
mod reference;
pub use reference::{MetadataReferenceError, MetadataReferenceShape, derive_metadata_reference};
mod root;
pub use root::{RootPolicyError, RootPolicyShape};
mod rollout;
mod shape;
pub use rollout::{RolloutCommitment, RolloutCommitmentError};
mod status;
pub use status::{RecoveryHandleHash, StatusBinding, StatusBindingError, recovery_handle_hash};
mod version;
pub use version::{CanonicalAppVersion, VersionError, compare_app_versions};
mod source;
pub use source::{SourceProfile, SourceProfileError};

pub use bindings::{BindingError, KeyDescriptor, SigningInput, key_descriptor};

use std::{fmt, io};

const METADATA_BYTES: usize = 1024 * 1024;
const CREDENTIAL_BYTES: usize = 64 * 1024;
const MAXIMUM_DEPTH: usize = 32;
const STRING_BYTES: usize = 64 * 1024;
const CONTAINER_MEMBERS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Metadata,
    Credential,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InputLimit,
    InvalidUtf8,
    InvalidJson,
    DepthLimit,
    StringLimit,
    MemberLimit,
    DuplicateKey,
    InvalidNumber,
    OutputLimit,
    Canonicalization,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = match self {
            Self::InputLimit => "JSON_INPUT_LIMIT",
            Self::InvalidUtf8 => "JSON_INVALID_UTF8",
            Self::InvalidJson => "JSON_INVALID",
            Self::DepthLimit => "JSON_DEPTH_LIMIT",
            Self::StringLimit => "JSON_STRING_LIMIT",
            Self::MemberLimit => "JSON_MEMBER_LIMIT",
            Self::DuplicateKey => "JSON_DUPLICATE_KEY",
            Self::InvalidNumber => "JSON_INVALID_NUMBER",
            Self::OutputLimit => "JSON_OUTPUT_LIMIT",
            Self::Canonicalization => "JSON_CANONICALIZATION_UNAVAILABLE",
        };
        formatter.write_str(code)
    }
}

impl std::error::Error for Error {}

// No public constructor, mutable value accessor, generic Serialize or RawValue
// entrance can bypass strict parsing. Do not derive Debug over untrusted data.
pub struct Document {
    value: serde_json::Value,
}

impl Document {
    pub fn parse(bytes: &[u8], kind: InputKind) -> Result<Self, Error> {
        let maximum = match kind {
            InputKind::Metadata => METADATA_BYTES,
            InputKind::Credential => CREDENTIAL_BYTES,
        };
        if bytes.len() > maximum {
            return Err(Error::InputLimit);
        }
        std::str::from_utf8(bytes).map_err(|_| Error::InvalidUtf8)?;
        Ok(Self {
            value: parse::bounded(bytes)?,
        })
    }

    // All numbers use finite binary64, including integer input spelling. Later
    // closed-schema checks must enforce safe integer semantics, not Value kind.
    pub fn value(&self) -> &serde_json::Value {
        &self.value
    }

    // The frozen reference bounds canonical output at 1 MiB for both profiles;
    // credential input still has its separate 64 KiB wire limit. No partial
    // output escapes on failure. UTF16 ordering/number formatting use serde_jcs.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut output = BoundedOutput {
            bytes: Vec::new(),
            exceeded: false,
        };
        if serde_jcs::to_writer(&mut output, &self.value).is_err() {
            return Err(if output.exceeded {
                Error::OutputLimit
            } else {
                Error::Canonicalization
            });
        }
        Ok(output.bytes)
    }
}

struct BoundedOutput {
    bytes: Vec<u8>,
    exceeded: bool,
}

impl io::Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > METADATA_BYTES - self.bytes.len() {
            self.exceeded = true;
            return Err(io::Error::other("canonical output limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
