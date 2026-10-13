//! Supplied event/task receipts, never trusted commitment or occupancy release.
use crate::{Document, Error, InputKind};
use jsonschema::{Draft, Retrieve, Uri, Validator};
use serde_json::Value;
use std::sync::OnceLock;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptResponseOperation {
    TelemetryEvents,
    PreinstallEvents,
    InstallEvents,
    ActivateEvents,
    ReconciliationEvents,
    RecoveryReviewEvents,
    ReconciliationAction,
    RecoveryReviewAction,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptResponseError {
    Json(Error),
    SchemaUnavailable,
    InvalidContract,
}
impl std::fmt::Display for ReceiptResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => e.fmt(f),
            Self::SchemaUnavailable => f.write_str("API_SCHEMA_UNAVAILABLE"),
            Self::InvalidContract => f.write_str("API_CONTRACT_INVALID"),
        }
    }
}
impl std::error::Error for ReceiptResponseError {}
macro_rules! frozen {
    ($path:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/",
            $path
        ))
    };
}
const RESOURCES: [(&str, &str); 1] = [("urn:pinvou:upgrade:v1:common", frozen!("common.json"))];
const SCHEMAS: [(ReceiptResponseOperation, &str); 8] = [
    (
        ReceiptResponseOperation::TelemetryEvents,
        frozen!("api/events-telemetry-event-response.json"),
    ),
    (
        ReceiptResponseOperation::PreinstallEvents,
        frozen!("api/events-preinstall-transaction-event-response.json"),
    ),
    (
        ReceiptResponseOperation::InstallEvents,
        frozen!("api/events-install-transaction-event-response.json"),
    ),
    (
        ReceiptResponseOperation::ActivateEvents,
        frozen!("api/events-activate-transaction-event-response.json"),
    ),
    (
        ReceiptResponseOperation::ReconciliationEvents,
        frozen!("api/events-reconciliation-event-response.json"),
    ),
    (
        ReceiptResponseOperation::RecoveryReviewEvents,
        frozen!("api/events-recovery-review-event-response.json"),
    ),
    (
        ReceiptResponseOperation::ReconciliationAction,
        frozen!("api/reconciliation-action-response.json"),
    ),
    (
        ReceiptResponseOperation::RecoveryReviewAction,
        frozen!("api/recovery-review-action-response.json"),
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
            .ok_or_else(|| "unregistered receipt response schema resource".into())
    }
}
type Registry = Vec<(ReceiptResponseOperation, Validator)>;
static REGISTRY: OnceLock<Result<Registry, ReceiptResponseError>> = OnceLock::new();
fn registry() -> Result<&'static Registry, ReceiptResponseError> {
    REGISTRY
        .get_or_init(|| {
            let resources: Vec<(&'static str, Value)> = RESOURCES
                .iter()
                .map(|(id, s)| {
                    serde_json::from_str(s)
                        .map(|v| (*id, v))
                        .map_err(|_| ReceiptResponseError::SchemaUnavailable)
                })
                .collect::<Result<_, _>>()?;
            SCHEMAS
                .iter()
                .map(|(op, s)| {
                    let schema: Value = serde_json::from_str(s)
                        .map_err(|_| ReceiptResponseError::SchemaUnavailable)?;
                    let validator = jsonschema::options()
                        .with_draft(Draft::Draft202012)
                        .should_validate_formats(false)
                        .with_retriever(Embedded {
                            resources: resources.clone(),
                        })
                        .build(&schema)
                        .map_err(|_| ReceiptResponseError::SchemaUnavailable)?;
                    Ok((*op, validator))
                })
                .collect()
        })
        .as_ref()
        .map_err(|e| *e)
}
impl Document {
    /// Complete original event/task-action response contract only. Receipts
    /// contain no credential and have no extra original semantic guards.
    /// Unit success grants no request/event/task correlation, committed outcome,
    /// lineage admission, occupancy release or permission to revive an executor.
    pub fn assert_supplied_receipt_response_contract(
        &self,
        operation: ReceiptResponseOperation,
    ) -> Result<(), ReceiptResponseError> {
        let canonical = self.canonical_bytes().map_err(ReceiptResponseError::Json)?;
        let reparsed =
            Document::parse(&canonical, InputKind::Metadata).map_err(ReceiptResponseError::Json)?;
        let validator = registry()?
            .iter()
            .find(|(op, _)| *op == operation)
            .ok_or(ReceiptResponseError::SchemaUnavailable)?;
        if !validator.1.is_valid(reparsed.value()) {
            return Err(ReceiptResponseError::InvalidContract);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipt_response_resources_are_closed_and_never_network_retrieved() {
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
