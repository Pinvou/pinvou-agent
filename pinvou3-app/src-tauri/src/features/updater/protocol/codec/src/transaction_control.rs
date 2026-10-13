//! Supplied transaction-control response shape, never trusted outcome or review.
use crate::{Document, Error, InputKind};
use jsonschema::{Draft, Retrieve, Uri, Validator};
use serde_json::Value;
use std::sync::OnceLock;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransactionControlOperation {
    ConsumeStatus,
    CancelPreinstall,
    CancelInstall,
    CancelActivate,
    ReviewPreinstall,
    ReviewInstall,
    ReviewActivate,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransactionControlError {
    Json(Error),
    SchemaUnavailable,
    InvalidContract,
}
impl std::fmt::Display for TransactionControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => e.fmt(f),
            Self::SchemaUnavailable => f.write_str("API_SCHEMA_UNAVAILABLE"),
            Self::InvalidContract => f.write_str("API_CONTRACT_INVALID"),
        }
    }
}
impl std::error::Error for TransactionControlError {}
macro_rules! frozen {
    ($path:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/",
            $path
        ))
    };
}
const RESOURCES: [(&str, &str); 4] = [
    ("urn:pinvou:upgrade:v1:common", frozen!("common.json")),
    (
        "urn:pinvou:upgrade:v1:api:status-response",
        frozen!("api/status-response.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:api:read-set",
        frozen!("api/read-set.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:api:revision-ref",
        frozen!("api/revision-ref.json"),
    ),
];
const SCHEMAS: [(TransactionControlOperation, &str); 7] = [
    (
        TransactionControlOperation::ConsumeStatus,
        frozen!("api/consume-status-response.json"),
    ),
    (
        TransactionControlOperation::CancelPreinstall,
        frozen!("api/cancel-preinstall-response.json"),
    ),
    (
        TransactionControlOperation::CancelInstall,
        frozen!("api/cancel-install-response.json"),
    ),
    (
        TransactionControlOperation::CancelActivate,
        frozen!("api/cancel-activate-response.json"),
    ),
    (
        TransactionControlOperation::ReviewPreinstall,
        frozen!("api/execution-review-preinstall-response.json"),
    ),
    (
        TransactionControlOperation::ReviewInstall,
        frozen!("api/execution-review-install-response.json"),
    ),
    (
        TransactionControlOperation::ReviewActivate,
        frozen!("api/execution-review-activate-response.json"),
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
            .ok_or_else(|| "unregistered transaction response schema resource".into())
    }
}
type Registry = Vec<(TransactionControlOperation, Validator)>;
static REGISTRY: OnceLock<Result<Registry, TransactionControlError>> = OnceLock::new();
fn registry() -> Result<&'static Registry, TransactionControlError> {
    REGISTRY
        .get_or_init(|| {
            let resources: Vec<(&'static str, Value)> = RESOURCES
                .iter()
                .map(|(id, s)| {
                    serde_json::from_str(s)
                        .map(|v| (*id, v))
                        .map_err(|_| TransactionControlError::SchemaUnavailable)
                })
                .collect::<Result<_, _>>()?;
            SCHEMAS
                .iter()
                .map(|(op, s)| {
                    let schema: Value = serde_json::from_str(s)
                        .map_err(|_| TransactionControlError::SchemaUnavailable)?;
                    let validator = jsonschema::options()
                        .with_draft(Draft::Draft202012)
                        .should_validate_formats(false)
                        .with_retriever(Embedded {
                            resources: resources.clone(),
                        })
                        .build(&schema)
                        .map_err(|_| TransactionControlError::SchemaUnavailable)?;
                    Ok((*op, validator))
                })
                .collect()
        })
        .as_ref()
        .map_err(|e| *e)
}
impl Document {
    /// Original complete response API contract for consume-status, cancellation
    /// and execution review. These closed Schemas contain no credential. Field
    /// equality/uniqueItems follow the original Schema; no current read-set,
    /// object identity, request correlation, state/ACK or clock proof is granted.
    pub fn assert_supplied_transaction_control_contract(
        &self,
        operation: TransactionControlOperation,
    ) -> Result<(), TransactionControlError> {
        let canonical = self
            .canonical_bytes()
            .map_err(TransactionControlError::Json)?;
        let reparsed = Document::parse(&canonical, InputKind::Metadata)
            .map_err(TransactionControlError::Json)?;
        let validator = registry()?
            .iter()
            .find(|(op, _)| *op == operation)
            .ok_or(TransactionControlError::SchemaUnavailable)?;
        if !validator.1.is_valid(reparsed.value()) {
            return Err(TransactionControlError::InvalidContract);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transaction_response_resources_are_closed_and_never_network_retrieved() {
        let r = Embedded {
            resources: RESOURCES
                .iter()
                .map(|(id, s)| (*id, serde_json::from_str(s).unwrap()))
                .collect(),
        };
        for id in [
            "https://example.invalid/schema",
            "file:///schema.json",
            "urn:pinvou:upgrade:v1:api:model-read-ref",
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
