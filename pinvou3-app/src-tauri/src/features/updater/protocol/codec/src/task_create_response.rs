//! Supplied task-create response contracts, never task authenticity or action rights.
use crate::{
    BoundedOutput, CredentialSemanticsError, Document, Error, InputKind, MetadataSemanticsError,
};
use jsonschema::{Draft, Retrieve, Uri, Validator};
use serde_json::Value;
use std::sync::OnceLock;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskCreateResponseOperation {
    Reconciliation,
    RecoveryReview,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskCreateResponseError {
    Json(Error),
    Semantics(CredentialSemanticsError),
    Time(MetadataSemanticsError),
    SchemaUnavailable,
    InvalidContract,
    CredentialTooLarge,
    BindingMismatch,
}
impl std::fmt::Display for TaskCreateResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => e.fmt(f),
            Self::Semantics(e) => e.fmt(f),
            Self::Time(e) => e.fmt(f),
            Self::SchemaUnavailable => f.write_str("API_SCHEMA_UNAVAILABLE"),
            Self::InvalidContract => f.write_str("API_CONTRACT_INVALID"),
            Self::CredentialTooLarge => f.write_str("CREDENTIAL_TOO_LARGE"),
            Self::BindingMismatch => f.write_str("API_RESPONSE_MISMATCH"),
        }
    }
}
impl std::error::Error for TaskCreateResponseError {}
macro_rules! frozen {
    ($path:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/",
            $path
        ))
    };
}
const RESOURCES: [(&str, &str); 5] = [
    ("urn:pinvou:upgrade:v1:common", frozen!("common.json")),
    (
        "urn:pinvou:upgrade:v1:credentials:reconciliation-action",
        frozen!("credentials/reconciliation-action.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:credentials:reconciliation-event",
        frozen!("credentials/reconciliation-event.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:credentials:recovery-review-action",
        frozen!("credentials/recovery-review-action.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:credentials:recovery-review-event",
        frozen!("credentials/recovery-review-event.json"),
    ),
];
const SCHEMAS: [(TaskCreateResponseOperation, &str); 2] = [
    (
        TaskCreateResponseOperation::Reconciliation,
        frozen!("api/reconciliation-create-response.json"),
    ),
    (
        TaskCreateResponseOperation::RecoveryReview,
        frozen!("api/recovery-review-create-response.json"),
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
            .ok_or_else(|| "unregistered task response schema resource".into())
    }
}
type Registry = Vec<(TaskCreateResponseOperation, Validator)>;
static REGISTRY: OnceLock<Result<Registry, TaskCreateResponseError>> = OnceLock::new();
fn registry() -> Result<&'static Registry, TaskCreateResponseError> {
    REGISTRY
        .get_or_init(|| {
            let resources: Vec<(&'static str, Value)> = RESOURCES
                .iter()
                .map(|(id, s)| {
                    serde_json::from_str(s)
                        .map(|v| (*id, v))
                        .map_err(|_| TaskCreateResponseError::SchemaUnavailable)
                })
                .collect::<Result<_, _>>()?;
            SCHEMAS
                .iter()
                .map(|(op, s)| {
                    let schema: Value = serde_json::from_str(s)
                        .map_err(|_| TaskCreateResponseError::SchemaUnavailable)?;
                    let validator = jsonschema::options()
                        .with_draft(Draft::Draft202012)
                        .should_validate_formats(false)
                        .with_retriever(Embedded {
                            resources: resources.clone(),
                        })
                        .build(&schema)
                        .map_err(|_| TaskCreateResponseError::SchemaUnavailable)?;
                    Ok((*op, validator))
                })
                .collect()
        })
        .as_ref()
        .map_err(|e| *e)
}
impl Document {
    /// Complete original task-create response contract with supplied credential
    /// semantics at max(iat,nbf), never trusted time, task state or action rights.
    /// Full task/scope identity is checked across both credentials; field values
    /// and Schema signatures remain untrusted supplied data.
    pub fn assert_supplied_task_create_response_contract(
        &self,
        operation: TaskCreateResponseOperation,
    ) -> Result<(), TaskCreateResponseError> {
        let canonical = self
            .canonical_bytes()
            .map_err(TaskCreateResponseError::Json)?;
        let reparsed = Document::parse(&canonical, InputKind::Metadata)
            .map_err(TaskCreateResponseError::Json)?;
        let validator = registry()?
            .iter()
            .find(|(op, _)| *op == operation)
            .ok_or(TaskCreateResponseError::SchemaUnavailable)?;
        if !validator.1.is_valid(reparsed.value()) {
            return Err(TaskCreateResponseError::InvalidContract);
        }
        let value = reparsed.value();
        for field in ["actionCredential", "eventCredential"] {
            let envelope = &value[field];
            let mut output = BoundedOutput {
                bytes: Vec::new(),
                exceeded: false,
            };
            serde_jcs::to_writer(&mut output, envelope).map_err(|_| {
                TaskCreateResponseError::Json(if output.exceeded {
                    Error::OutputLimit
                } else {
                    Error::Canonicalization
                })
            })?;
            if output.bytes.len() > crate::CREDENTIAL_BYTES {
                return Err(TaskCreateResponseError::CredentialTooLarge);
            }
            let claims = &envelope["signed"];
            let iat = claims["iat"]
                .as_f64()
                .ok_or(TaskCreateResponseError::InvalidContract)?;
            let nbf = claims["nbf"]
                .as_f64()
                .ok_or(TaskCreateResponseError::InvalidContract)?;
            let time = Document {
                value: Value::from(iat.max(nbf)),
            }
            .protocol_time_shape()
            .map_err(TaskCreateResponseError::Time)?;
            crate::credential_semantics::assert_claims_semantics(claims, &time)
                .map_err(TaskCreateResponseError::Semantics)?;
        }
        let action = &value["actionCredential"]["signed"];
        let event = &value["eventCredential"]["signed"];
        // Bounded Document normalizes all JSON numbers to binary64. Whole
        // Value equality therefore matches original sameJson/JCS equality,
        // including object order, zero and array order, without projections.
        if value["task"] != action["task"]
            || value["task"] != event["task"]
            || action["scope"] != event["scope"]
            || ["product", "component", "installId"]
                .iter()
                .any(|field| action[*field] != event[*field])
        {
            return Err(TaskCreateResponseError::BindingMismatch);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn task_response_retrieval_is_closed_even_under_parent_network_features() {
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
