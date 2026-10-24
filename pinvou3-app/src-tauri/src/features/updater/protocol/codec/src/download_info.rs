//! Frozen download-info response contract only; this grants no authenticity or rights.
use crate::{
    BoundedOutput, CredentialSemanticsError, Document, Error, InputKind, MetadataSemanticsError,
};
use jsonschema::{Draft, Retrieve, Uri, Validator};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadInfoContractError {
    Json(Error),
    Semantics(CredentialSemanticsError),
    Time(MetadataSemanticsError),
    SchemaUnavailable,
    InvalidContract,
    CredentialTooLarge,
    BindingMismatch,
}
impl std::fmt::Display for DownloadInfoContractError {
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
impl std::error::Error for DownloadInfoContractError {}

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
        "urn:pinvou:upgrade:v1:credentials:download",
        frozen_schema!("credentials/download.json"),
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
            .ok_or_else(|| "unregistered download response schema resource".into())
    }
}
static REGISTRY: OnceLock<Result<Validator, DownloadInfoContractError>> = OnceLock::new();
fn registry() -> Result<&'static Validator, DownloadInfoContractError> {
    REGISTRY
        .get_or_init(|| {
            fn build(source: &str) -> Result<Validator, DownloadInfoContractError> {
                let resources = RESOURCES
                    .iter()
                    .map(|(id, source)| {
                        serde_json::from_str(source)
                            .map(|v| (*id, v))
                            .map_err(|_| DownloadInfoContractError::SchemaUnavailable)
                    })
                    .collect::<Result<_, _>>()?;
                let schema: Value = serde_json::from_str(source)
                    .map_err(|_| DownloadInfoContractError::SchemaUnavailable)?;
                jsonschema::options()
                    .with_draft(Draft::Draft202012)
                    .should_validate_formats(false)
                    .with_retriever(EmbeddedResources { resources })
                    .build(&schema)
                    .map_err(|_| DownloadInfoContractError::SchemaUnavailable)
            }
            build(frozen_schema!("api/download-info-response.json"))
        })
        .as_ref()
        .map_err(|e| *e)
}
impl Document {
    /// Complete original supplied download-info API response contract. Canonical
    /// credential semantics use max(iat,nbf), not authenticated current time.
    /// URL/source/expiry remain contract data, not an approved destination or a
    /// total download deadline. Success never creates fetch/install rights.
    pub fn assert_supplied_download_info_contract(&self) -> Result<(), DownloadInfoContractError> {
        let canonical = self
            .canonical_bytes()
            .map_err(DownloadInfoContractError::Json)?;
        let reparsed = Document::parse(&canonical, InputKind::Metadata)
            .map_err(DownloadInfoContractError::Json)?;
        if !registry()?.is_valid(reparsed.value()) {
            return Err(DownloadInfoContractError::InvalidContract);
        }
        let value = reparsed.value();
        {
            let envelope = &value["credential"];
            let mut output = BoundedOutput {
                bytes: Vec::new(),
                exceeded: false,
            };
            serde_jcs::to_writer(&mut output, envelope).map_err(|_| {
                DownloadInfoContractError::Json(if output.exceeded {
                    Error::OutputLimit
                } else {
                    Error::Canonicalization
                })
            })?;
            if output.bytes.len() > crate::CREDENTIAL_BYTES {
                return Err(DownloadInfoContractError::CredentialTooLarge);
            }
            let claims = &envelope["signed"];
            // Full fixed Schema already proves nonnegative safe integer times.
            let iat = claims["iat"]
                .as_f64()
                .ok_or(DownloadInfoContractError::InvalidContract)?;
            let nbf = claims["nbf"]
                .as_f64()
                .ok_or(DownloadInfoContractError::InvalidContract)?;
            let time = Document {
                value: Value::from(iat.max(nbf)),
            }
            .protocol_time_shape()
            .map_err(DownloadInfoContractError::Time)?;
            crate::credential_semantics::assert_claims_semantics(claims, &time)
                .map_err(DownloadInfoContractError::Semantics)?;
        }
        let claims = &value["credential"]["signed"];
        let expires = value["expiresAt"]
            .as_f64()
            .ok_or(DownloadInfoContractError::InvalidContract)?;
        let exp = claims["exp"]
            .as_f64()
            .ok_or(DownloadInfoContractError::InvalidContract)?;
        let nbf = claims["nbf"]
            .as_f64()
            .ok_or(DownloadInfoContractError::InvalidContract)?;
        // Bind the exact transport URL bytes. URI normalization would permit a
        // different signed query, escape spelling or destination to be substituted.
        let url = value["url"]
            .as_str()
            .ok_or(DownloadInfoContractError::InvalidContract)?;
        let url_hash: String = Sha256::digest(url.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if value["packageId"] != claims["packageId"]
            || value["size"] != claims["package"]["size"]
            || value["sha256"] != claims["package"]["sha256"]
            || claims["sourceBinding"]["urlSha256"] != url_hash
            || expires > exp
            || expires <= nbf
        {
            return Err(DownloadInfoContractError::BindingMismatch);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_download_retriever_denies_every_external_or_unregistered_resource() {
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
