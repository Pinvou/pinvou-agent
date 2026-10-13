//! Frozen check/refresh API contracts only; this grants no authenticity or rights.
use crate::{
    BoundedOutput, CredentialSemanticsError, Document, Error, InputKind, MetadataSemanticsError,
};
use jsonschema::{Draft, Retrieve, Uri, Validator};
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckResponseOperation {
    Check,
    Refresh,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckResponseError {
    Json(Error),
    Semantics(CredentialSemanticsError),
    Time(MetadataSemanticsError),
    SchemaUnavailable,
    InvalidContract,
    CredentialTooLarge,
    BindingMismatch,
}
impl std::fmt::Display for CheckResponseError {
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
impl std::error::Error for CheckResponseError {}

macro_rules! frozen_schema {
    ($path:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/",
            $path
        ))
    };
}
// A closed dependency set of original embedded resources. Caller payload $refs
// are data, never schemas. Parent networking features cannot change retrieval.
const RESOURCES: [(&str, &str); 3] = [
    (
        "urn:pinvou:upgrade:v1:common",
        frozen_schema!("common.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:credentials:decision",
        frozen_schema!("credentials/decision.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:credentials:telemetry-event",
        frozen_schema!("credentials/telemetry-event.json"),
    ),
];
struct EmbeddedResources {
    resources: Vec<(&'static str, Value)>,
}
impl Retrieve for EmbeddedResources {
    fn retrieve(
        &self,
        uri: &Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        self.resources
            .iter()
            .find(|(id, _)| *id == uri.as_str())
            .map(|(_, v)| v.clone())
            .ok_or_else(|| "unregistered check response schema resource".into())
    }
}
struct Registry {
    check: Validator,
    refresh: Validator,
}
static REGISTRY: OnceLock<Result<Registry, CheckResponseError>> = OnceLock::new();
fn registry() -> Result<&'static Registry, CheckResponseError> {
    REGISTRY
        .get_or_init(|| {
            fn build(source: &str) -> Result<Validator, CheckResponseError> {
                let resources = RESOURCES
                    .iter()
                    .map(|(id, source)| {
                        serde_json::from_str(source)
                            .map(|v| (*id, v))
                            .map_err(|_| CheckResponseError::SchemaUnavailable)
                    })
                    .collect::<Result<_, _>>()?;
                let schema: Value = serde_json::from_str(source)
                    .map_err(|_| CheckResponseError::SchemaUnavailable)?;
                jsonschema::options()
                    .with_draft(Draft::Draft202012)
                    .should_validate_formats(false)
                    .with_retriever(EmbeddedResources { resources })
                    .build(&schema)
                    .map_err(|_| CheckResponseError::SchemaUnavailable)
            }
            Ok(Registry {
                check: build(frozen_schema!("api/check-response.json"))?,
                refresh: build(frozen_schema!("api/refresh-response.json"))?,
            })
        })
        .as_ref()
        .map_err(|e| *e)
}
impl Document {
    /// Exact frozen assertApiContract check/refresh response acceptance. The
    /// supplied operation selects a fixed schema. Wire whitespace/key order is
    /// allowed; canonical expansion is bounded and reparsed like the original.
    /// Credential semantics use max(iat,nbf), NOT trusted current time. Outer
    /// signatures are schema data here, not verified/ordered signature proofs.
    /// Returns unit only: no current check result, session, eligibility or trust.
    pub fn assert_supplied_check_response_contract(
        &self,
        operation: CheckResponseOperation,
    ) -> Result<(), CheckResponseError> {
        let canonical = self.canonical_bytes().map_err(CheckResponseError::Json)?;
        let reparsed =
            Document::parse(&canonical, InputKind::Metadata).map_err(CheckResponseError::Json)?;
        let registry = registry()?;
        let validator = match operation {
            CheckResponseOperation::Check => &registry.check,
            CheckResponseOperation::Refresh => &registry.refresh,
        };
        if !validator.is_valid(reparsed.value()) {
            return Err(CheckResponseError::InvalidContract);
        }
        let value = reparsed.value();
        for field in ["decision", "telemetryCredential"] {
            let envelope = &value[field];
            let mut output = BoundedOutput {
                bytes: Vec::new(),
                exceeded: false,
            };
            serde_jcs::to_writer(&mut output, envelope).map_err(|_| {
                CheckResponseError::Json(if output.exceeded {
                    Error::OutputLimit
                } else {
                    Error::Canonicalization
                })
            })?;
            if output.bytes.len() > crate::CREDENTIAL_BYTES {
                return Err(CheckResponseError::CredentialTooLarge);
            }
            let claims = &envelope["signed"];
            // Full fixed Schema already proves nonnegative safe integer times.
            let iat = claims["iat"]
                .as_f64()
                .ok_or(CheckResponseError::InvalidContract)?;
            let nbf = claims["nbf"]
                .as_f64()
                .ok_or(CheckResponseError::InvalidContract)?;
            let time = Document {
                value: Value::from(iat.max(nbf)),
            }
            .protocol_time_shape()
            .map_err(CheckResponseError::Time)?;
            crate::credential_semantics::assert_claims_semantics(claims, &time)
                .map_err(CheckResponseError::Semantics)?;
        }
        let decision = &value["decision"]["signed"];
        let telemetry = &value["telemetryCredential"]["signed"];
        for field in [
            "product",
            "component",
            "installId",
            "decisionId",
            "decisionRevision",
            "telemetrySessionId",
            "selectionGeneration",
            "scope",
        ] {
            if !crate::credential_semantics::same(&decision[field], &telemetry[field])
                .map_err(CheckResponseError::Semantics)?
            {
                return Err(CheckResponseError::BindingMismatch);
            }
        }
        if operation == CheckResponseOperation::Refresh
            && value["writableSessionId"] != decision["telemetrySessionId"]
        {
            return Err(CheckResponseError::BindingMismatch);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_response_retriever_denies_every_external_or_unregistered_resource() {
        let r = EmbeddedResources {
            resources: RESOURCES
                .iter()
                .map(|(id, s)| (*id, serde_json::from_str(s).unwrap()))
                .collect(),
        };
        for id in [
            "https://example.invalid/schema",
            "file:///schema.json",
            "urn:pinvou:upgrade:v1:credentials:download",
        ] {
            let uri: Uri<String> = id.parse().unwrap();
            assert!(r.retrieve(&uri).is_err());
            assert!(
                jsonschema::options()
                    .with_retriever(EmbeddedResources { resources: vec![] })
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
