//! Bounded Upgrade V1 JSON and canonical bytes. Successful decoding is not
//! signature verification, established Root trust, current eligibility or ACK.

mod bindings;
mod consume;
pub use consume::{ConsumeDigestError, ConsumeRequestDigest};
mod native;
mod parse;
pub use native::{NativeIdentityError, NativeIdentityShape, assert_native_identity};
mod rollout;
mod shape;
pub use rollout::{RolloutCommitment, RolloutCommitmentError};
mod status;
pub use status::{RecoveryHandleHash, StatusBinding, StatusBindingError, recovery_handle_hash};
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
