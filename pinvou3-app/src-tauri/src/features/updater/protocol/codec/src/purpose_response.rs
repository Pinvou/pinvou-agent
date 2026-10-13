//! Supplied validate/consume response contracts, never authorization or an ACK.
use crate::{
    BoundedOutput, CredentialSemanticsError, Document, Error, InputKind, MetadataSemanticsError,
};
use jsonschema::{Draft, Retrieve, Uri, Validator};
use serde_json::Value;
use std::sync::OnceLock;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PurposeResponseOperation {
    ValidatePreinstall,
    ValidateInstall,
    ValidateActivate,
    ConsumePreinstall,
    ConsumeInstall,
    ConsumeActivate,
}
impl PurposeResponseOperation {
    fn purpose(self) -> &'static str {
        match self {
            Self::ValidatePreinstall | Self::ConsumePreinstall => "preinstall",
            Self::ValidateInstall | Self::ConsumeInstall => "install",
            Self::ValidateActivate | Self::ConsumeActivate => "activate",
        }
    }
    fn consuming(self) -> bool {
        matches!(
            self,
            Self::ConsumePreinstall | Self::ConsumeInstall | Self::ConsumeActivate
        )
    }
    fn credential_field(self) -> &'static str {
        if self.consuming() {
            "transactionCredential"
        } else {
            "authorization"
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PurposeResponseError {
    Json(Error),
    Semantics(CredentialSemanticsError),
    Time(MetadataSemanticsError),
    SchemaUnavailable,
    InvalidContract,
    CredentialTooLarge,
    PurposeMismatch,
    BindingMismatch,
}
impl std::fmt::Display for PurposeResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => e.fmt(f),
            Self::Semantics(e) => e.fmt(f),
            Self::Time(e) => e.fmt(f),
            Self::SchemaUnavailable => f.write_str("API_SCHEMA_UNAVAILABLE"),
            Self::InvalidContract => f.write_str("API_CONTRACT_INVALID"),
            Self::CredentialTooLarge => f.write_str("CREDENTIAL_TOO_LARGE"),
            Self::PurposeMismatch => f.write_str("API_PURPOSE_MISMATCH"),
            Self::BindingMismatch => f.write_str("API_RESPONSE_MISMATCH"),
        }
    }
}
impl std::error::Error for PurposeResponseError {}
macro_rules! frozen {
    ($path:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/",
            $path
        ))
    };
}
const RESOURCES: [(&str, &str); 7] = [
    ("urn:pinvou:upgrade:v1:common", frozen!("common.json")),
    (
        "urn:pinvou:upgrade:v1:credentials:authorization-preinstall",
        frozen!("credentials/authorization-preinstall.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:credentials:authorization-install",
        frozen!("credentials/authorization-install.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:credentials:authorization-activate",
        frozen!("credentials/authorization-activate.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:credentials:preinstall-transaction-event",
        frozen!("credentials/preinstall-transaction-event.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:credentials:install-transaction-event",
        frozen!("credentials/install-transaction-event.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:credentials:activate-transaction-event",
        frozen!("credentials/activate-transaction-event.json"),
    ),
];
const SCHEMAS: [(PurposeResponseOperation, &str); 6] = [
    (
        PurposeResponseOperation::ValidatePreinstall,
        frozen!("api/validate-preinstall-response.json"),
    ),
    (
        PurposeResponseOperation::ValidateInstall,
        frozen!("api/validate-install-response.json"),
    ),
    (
        PurposeResponseOperation::ValidateActivate,
        frozen!("api/validate-activate-response.json"),
    ),
    (
        PurposeResponseOperation::ConsumePreinstall,
        frozen!("api/consume-preinstall-response.json"),
    ),
    (
        PurposeResponseOperation::ConsumeInstall,
        frozen!("api/consume-install-response.json"),
    ),
    (
        PurposeResponseOperation::ConsumeActivate,
        frozen!("api/consume-activate-response.json"),
    ),
];
struct Embedded {
    resources: Vec<(&'static str, Value)>,
}
impl Retrieve for Embedded {
    fn retrieve(
        &self,
        uri: &Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        self.resources
            .iter()
            .find(|(id, _)| *id == uri.as_str())
            .map(|(_, v)| v.clone())
            .ok_or_else(|| "unregistered purpose response schema resource".into())
    }
}
type Registry = Vec<(PurposeResponseOperation, Validator)>;
static REGISTRY: OnceLock<Result<Registry, PurposeResponseError>> = OnceLock::new();
fn registry() -> Result<&'static Registry, PurposeResponseError> {
    REGISTRY
        .get_or_init(|| {
            let resources: Vec<(&'static str, Value)> = RESOURCES
                .iter()
                .map(|(id, s)| {
                    serde_json::from_str(s)
                        .map(|v| (*id, v))
                        .map_err(|_| PurposeResponseError::SchemaUnavailable)
                })
                .collect::<Result<_, _>>()?;
            SCHEMAS
                .iter()
                .map(|(op, s)| {
                    let schema: Value = serde_json::from_str(s)
                        .map_err(|_| PurposeResponseError::SchemaUnavailable)?;
                    let validator = jsonschema::options()
                        .with_draft(Draft::Draft202012)
                        .should_validate_formats(false)
                        .with_retriever(Embedded {
                            resources: resources.clone(),
                        })
                        .build(&schema)
                        .map_err(|_| PurposeResponseError::SchemaUnavailable)?;
                    Ok((*op, validator))
                })
                .collect()
        })
        .as_ref()
        .map_err(|e| *e)
}
impl Document {
    /// Original complete three-purpose validate/consume response contract. The
    /// operation selects a fixed Schema, never caller-defined resources. Claims
    /// semantics use max(iat,nbf), not trusted current time. A consumed status is
    /// only supplied data, never a durable outcome or permission to revive an owner.
    pub fn assert_supplied_purpose_response_contract(
        &self,
        operation: PurposeResponseOperation,
    ) -> Result<(), PurposeResponseError> {
        let canonical = self.canonical_bytes().map_err(PurposeResponseError::Json)?;
        let reparsed =
            Document::parse(&canonical, InputKind::Metadata).map_err(PurposeResponseError::Json)?;
        let validator = registry()?
            .iter()
            .find(|(op, _)| *op == operation)
            .ok_or(PurposeResponseError::SchemaUnavailable)?;
        if !validator.1.is_valid(reparsed.value()) {
            return Err(PurposeResponseError::InvalidContract);
        }
        let value = reparsed.value();
        let envelope = &value[operation.credential_field()];
        let mut output = BoundedOutput {
            bytes: Vec::new(),
            exceeded: false,
        };
        serde_jcs::to_writer(&mut output, envelope).map_err(|_| {
            PurposeResponseError::Json(if output.exceeded {
                Error::OutputLimit
            } else {
                Error::Canonicalization
            })
        })?;
        if output.bytes.len() > crate::CREDENTIAL_BYTES {
            return Err(PurposeResponseError::CredentialTooLarge);
        }
        let claims = &envelope["signed"];
        let iat = claims["iat"]
            .as_f64()
            .ok_or(PurposeResponseError::InvalidContract)?;
        let nbf = claims["nbf"]
            .as_f64()
            .ok_or(PurposeResponseError::InvalidContract)?;
        let time = Document {
            value: Value::from(iat.max(nbf)),
        }
        .protocol_time_shape()
        .map_err(PurposeResponseError::Time)?;
        crate::credential_semantics::assert_claims_semantics(claims, &time)
            .map_err(PurposeResponseError::Semantics)?;
        if !operation.consuming() && claims["purpose"] != operation.purpose() {
            return Err(PurposeResponseError::PurposeMismatch);
        }
        if operation.consuming() && value["transactionId"] != claims["transactionId"] {
            return Err(PurposeResponseError::BindingMismatch);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn purpose_retrieval_is_closed_even_under_parent_network_features() {
        let r = Embedded {
            resources: RESOURCES
                .iter()
                .map(|(id, s)| (*id, serde_json::from_str(s).unwrap()))
                .collect(),
        };
        for id in [
            "https://example.invalid/schema",
            "file:///schema.json",
            "urn:pinvou:upgrade:v1:credentials:decision",
        ] {
            let uri: Uri<String> = id.parse().unwrap();
            assert!(r.retrieve(&uri).is_err());
            assert!(
                jsonschema::options()
                    .with_retriever(Embedded { resources: vec![] })
                    .build(&serde_json::json!({"$ref":id}))
                    .is_err()
            );
        }
        for (id, _) in RESOURCES {
            let uri: Uri<String> = id.parse().unwrap();
            assert_eq!(r.retrieve(&uri).unwrap()["$id"], id);
        }
    }
}
