//! Complete supplied client request contracts, never signature or execution rights.
mod credentials;
mod guards;
mod registry;
use crate::{
    ConsumeDigestError, CredentialSemanticsError, Document, Error, InputKind,
    MetadataSemanticsError,
};
/// Requests and responses use the same exact frozen 27-client operation catalog.
/// Alias preserves the established enum, name parsing and existing interfaces.
pub type ClientRequestOperation = crate::ClientResponseOperation;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientRequestError {
    Json(Error),
    TrustedTime(crate::TimeContractError),
    Semantics(CredentialSemanticsError),
    Time(MetadataSemanticsError),
    Digest(ConsumeDigestError),
    SchemaUnavailable,
    InvalidContract,
    CredentialTooLarge,
    ScopeMismatch,
    ConsumeMismatch,
    PurposeMismatch,
    EventMismatch,
    EvidenceMismatch,
    IncrementalNotEnabled,
}
impl std::fmt::Display for ClientRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => e.fmt(f),
            Self::TrustedTime(crate::TimeContractError::InvalidRequest) => {
                f.write_str("API_CONTRACT_INVALID")
            }
            Self::TrustedTime(e) => e.fmt(f),
            Self::Semantics(e) => e.fmt(f),
            Self::Time(e) => e.fmt(f),
            Self::Digest(e) => e.fmt(f),
            Self::SchemaUnavailable => f.write_str("API_SCHEMA_UNAVAILABLE"),
            Self::InvalidContract => f.write_str("API_CONTRACT_INVALID"),
            Self::CredentialTooLarge => f.write_str("CREDENTIAL_TOO_LARGE"),
            Self::ScopeMismatch => f.write_str("API_SCOPE_MISMATCH"),
            Self::ConsumeMismatch => f.write_str("API_CONSUME_MISMATCH"),
            Self::PurposeMismatch => f.write_str("API_PURPOSE_MISMATCH"),
            Self::EventMismatch => f.write_str("API_EVENT_MISMATCH"),
            Self::EvidenceMismatch => f.write_str("API_EVIDENCE_MISMATCH"),
            Self::IncrementalNotEnabled => f.write_str("INCREMENTAL_NOT_ENABLED"),
        }
    }
}
impl std::error::Error for ClientRequestError {}
impl Document {
    /// Original complete client request API contract. Credential semantics use
    /// max(iat,nbf), never trusted current time. This does not apply the later
    /// post-authentication consume binding, current read set, owner or network.
    pub fn assert_supplied_client_request_contract(
        &self,
        operation: ClientRequestOperation,
    ) -> Result<(), ClientRequestError> {
        let canonical = self.canonical_bytes().map_err(ClientRequestError::Json)?;
        let reparsed =
            Document::parse(&canonical, InputKind::Metadata).map_err(ClientRequestError::Json)?;
        if !registry::validator(operation)?.is_valid(reparsed.value()) {
            return Err(ClientRequestError::InvalidContract);
        }
        if operation == ClientRequestOperation::Time {
            return reparsed
                .time_request_shape()
                .map(|_| ())
                .map_err(ClientRequestError::TrustedTime);
        }
        credentials::assert_credentials(reparsed.value())?;
        guards::assert_relationships(reparsed.value(), operation)
    }
}
