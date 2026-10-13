//! Supplied helper-cleanup response contract, never cleanup or protected-object rights.
use crate::{
    BoundedOutput, CredentialSemanticsError, Document, Error, InputKind, MetadataSemanticsError,
};
use jsonschema::{Draft, Retrieve, Uri, Validator};
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperCleanupResponseError {
    Json(Error),
    Semantics(CredentialSemanticsError),
    Time(MetadataSemanticsError),
    SchemaUnavailable,
    InvalidContract,
    CredentialTooLarge,
}
impl std::fmt::Display for HelperCleanupResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => e.fmt(f),
            Self::Semantics(e) => e.fmt(f),
            Self::Time(e) => e.fmt(f),
            Self::SchemaUnavailable => f.write_str("API_SCHEMA_UNAVAILABLE"),
            Self::InvalidContract => f.write_str("API_CONTRACT_INVALID"),
            Self::CredentialTooLarge => f.write_str("CREDENTIAL_TOO_LARGE"),
        }
    }
}
impl std::error::Error for HelperCleanupResponseError {}

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
const RESOURCES: [(&str, &str); 2] = [
    (
        "urn:pinvou:upgrade:v1:common",
        frozen_schema!("common.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:credentials:helper-cleanup",
        frozen_schema!("credentials/helper-cleanup.json"),
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
            .ok_or_else(|| "unregistered helper cleanup response schema resource".into())
    }
}
static REGISTRY: OnceLock<Result<Validator, HelperCleanupResponseError>> = OnceLock::new();
fn registry() -> Result<&'static Validator, HelperCleanupResponseError> {
    REGISTRY
        .get_or_init(|| {
            fn build(source: &str) -> Result<Validator, HelperCleanupResponseError> {
                let resources = RESOURCES
                    .iter()
                    .map(|(id, source)| {
                        serde_json::from_str(source)
                            .map(|v| (*id, v))
                            .map_err(|_| HelperCleanupResponseError::SchemaUnavailable)
                    })
                    .collect::<Result<_, _>>()?;
                let schema: Value = serde_json::from_str(source)
                    .map_err(|_| HelperCleanupResponseError::SchemaUnavailable)?;
                jsonschema::options()
                    .with_draft(Draft::Draft202012)
                    .should_validate_formats(false)
                    .with_retriever(EmbeddedResources { resources })
                    .build(&schema)
                    .map_err(|_| HelperCleanupResponseError::SchemaUnavailable)
            }
            build(frozen_schema!("api/helper-cleanup-response.json"))
        })
        .as_ref()
        .map_err(|e| *e)
}
impl Document {
    /// Complete original helper-cleanup response API contract. Credential
    /// semantics use max(iat,nbf), never authenticated current time. Unit success
    /// grants no protected-object identity, current qualification or deletion
    /// rights; cleanup owners must establish those facts independently.
    pub fn assert_supplied_helper_cleanup_response_contract(
        &self,
    ) -> Result<(), HelperCleanupResponseError> {
        let canonical = self
            .canonical_bytes()
            .map_err(HelperCleanupResponseError::Json)?;
        let reparsed = Document::parse(&canonical, InputKind::Metadata)
            .map_err(HelperCleanupResponseError::Json)?;
        if !registry()?.is_valid(reparsed.value()) {
            return Err(HelperCleanupResponseError::InvalidContract);
        }
        let value = reparsed.value();
        {
            let envelope = &value["credential"];
            let mut output = BoundedOutput {
                bytes: Vec::new(),
                exceeded: false,
            };
            serde_jcs::to_writer(&mut output, envelope).map_err(|_| {
                HelperCleanupResponseError::Json(if output.exceeded {
                    Error::OutputLimit
                } else {
                    Error::Canonicalization
                })
            })?;
            if output.bytes.len() > crate::CREDENTIAL_BYTES {
                return Err(HelperCleanupResponseError::CredentialTooLarge);
            }
            let claims = &envelope["signed"];
            // Full fixed Schema already proves nonnegative safe integer times.
            let iat = claims["iat"]
                .as_f64()
                .ok_or(HelperCleanupResponseError::InvalidContract)?;
            let nbf = claims["nbf"]
                .as_f64()
                .ok_or(HelperCleanupResponseError::InvalidContract)?;
            let time = Document {
                value: Value::from(iat.max(nbf)),
            }
            .protocol_time_shape()
            .map_err(HelperCleanupResponseError::Time)?;
            crate::credential_semantics::assert_claims_semantics(claims, &time)
                .map_err(HelperCleanupResponseError::Semantics)?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helper_cleanup_retriever_denies_every_external_or_unregistered_resource() {
        let r = EmbeddedResources {
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
