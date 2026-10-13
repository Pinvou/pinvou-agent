//! Fixed embedded V1 envelope Schema only; no cryptographic or semantic trust.
use crate::{CanonicalEnvelopeShape, Document, InputKind};
use jsonschema::{Draft, Retrieve, Uri, Validator};
use serde_json::Value;
use std::sync::OnceLock;

const COMMON_URI: &str = "urn:pinvou:upgrade:v1:common";
const COMMON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../../../../specs/upgrade/v1/schemas/common.json"
));
const SCHEMAS: [(&str, &str); 20] = [
    (
        "root",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/metadata/root.json"
        )),
    ),
    (
        "timestamp",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/metadata/timestamp.json"
        )),
    ),
    (
        "snapshot",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/metadata/snapshot.json"
        )),
    ),
    (
        "target",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/metadata/target.json"
        )),
    ),
    (
        "release",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/metadata/release.json"
        )),
    ),
    (
        "package",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/metadata/package.json"
        )),
    ),
    (
        "decision",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/credentials/decision.json"
        )),
    ),
    (
        "download",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/credentials/download.json"
        )),
    ),
    (
        "authorization-preinstall",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/credentials/authorization-preinstall.json"
        )),
    ),
    (
        "authorization-install",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/credentials/authorization-install.json"
        )),
    ),
    (
        "authorization-activate",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/credentials/authorization-activate.json"
        )),
    ),
    (
        "helper-cleanup",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/credentials/helper-cleanup.json"
        )),
    ),
    (
        "telemetry-event",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/credentials/telemetry-event.json"
        )),
    ),
    (
        "preinstall-transaction-event",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/credentials/preinstall-transaction-event.json"
        )),
    ),
    (
        "install-transaction-event",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/credentials/install-transaction-event.json"
        )),
    ),
    (
        "activate-transaction-event",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/credentials/activate-transaction-event.json"
        )),
    ),
    (
        "reconciliation-action",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/credentials/reconciliation-action.json"
        )),
    ),
    (
        "recovery-review-action",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/credentials/recovery-review-action.json"
        )),
    ),
    (
        "reconciliation-event",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/credentials/reconciliation-event.json"
        )),
    ),
    (
        "recovery-review-event",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/credentials/recovery-review-event.json"
        )),
    ),
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaError {
    Unavailable,
    InvalidEnvelope,
}
impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "ENVELOPE_SCHEMA_UNAVAILABLE",
            Self::InvalidEnvelope => "ENVELOPE_SCHEMA_INVALID",
        })
    }
}
impl std::error::Error for SchemaError {}

struct EmbeddedCommon {
    schema: Value,
}
impl Retrieve for EmbeddedCommon {
    fn retrieve(
        &self,
        uri: &Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        // Explicitly deny every other URI, even if a parent enables networking.
        // No caller-provided schema, path or payload $ref is ever interpreted.
        if uri.as_str() == COMMON_URI {
            Ok(self.schema.clone())
        } else {
            Err("unregistered upgrade schema resource".into())
        }
    }
}
struct Registry {
    validators: Vec<(&'static str, Validator)>,
}
static REGISTRY: OnceLock<Result<Registry, SchemaError>> = OnceLock::new();
fn registry() -> Result<&'static Registry, SchemaError> {
    REGISTRY
        .get_or_init(|| {
            let common: Value =
                serde_json::from_str(COMMON).map_err(|_| SchemaError::Unavailable)?;
            let mut validators = Vec::with_capacity(SCHEMAS.len());
            for (role, source) in SCHEMAS {
                let schema: Value =
                    serde_json::from_str(source).map_err(|_| SchemaError::Unavailable)?;
                let validator = jsonschema::options()
                    .with_draft(Draft::Draft202012)
                    .should_validate_formats(false)
                    .with_retriever(EmbeddedCommon {
                        schema: common.clone(),
                    })
                    .build(&schema)
                    .map_err(|_| SchemaError::Unavailable)?;
                validators.push((role, validator));
            }
            Ok(Registry { validators })
        })
        .as_ref()
        .map_err(|error| *error)
}

/// Original canonical envelope satisfying its complete frozen role Schema.
/// No public constructor/Debug/Serialize. This is not a signature, expected-context,
/// time/semantic, Root-trust, threshold or protected acceptance certificate.
pub struct SchemaCheckedEnvelope {
    envelope: CanonicalEnvelopeShape,
}
impl SchemaCheckedEnvelope {
    pub fn envelope(&self) -> &CanonicalEnvelopeShape {
        &self.envelope
    }
}
impl CanonicalEnvelopeShape {
    /// Consume this original shape into a Schema-only result, preserving bytes.
    pub fn check_full_schema(self) -> Result<SchemaCheckedEnvelope, SchemaError> {
        let role = self.signed().value()["role"]
            .as_str()
            .ok_or(SchemaError::InvalidEnvelope)?;
        let kind = if crate::shape::ROLES[..6].contains(&role) {
            InputKind::Metadata
        } else {
            InputKind::Credential
        };
        // Reparse only the factory's own bounded bytes; do not reconstruct an
        // envelope from a partial signed body or unrelated caller-provided bytes.
        let document =
            Document::parse(self.raw_bytes(), kind).map_err(|_| SchemaError::InvalidEnvelope)?;
        let validator = registry()?
            .validators
            .iter()
            .find(|(name, _)| *name == role)
            .ok_or(SchemaError::Unavailable)?;
        if !validator.1.is_valid(document.value()) {
            return Err(SchemaError::InvalidEnvelope);
        }
        Ok(SchemaCheckedEnvelope { envelope: self })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_retriever_refuses_network_files_and_unknown_urns() {
        let retriever = EmbeddedCommon {
            schema: serde_json::from_str(COMMON).unwrap(),
        };
        for uri in [
            "https://example.invalid/schema",
            "file:///schema.json",
            "urn:pinvou:upgrade:v1:unknown",
        ] {
            let uri: Uri<String> = uri.parse().unwrap();
            assert!(retriever.retrieve(&uri).is_err());
            let schema = serde_json::json!({"$ref":uri.as_str()});
            assert!(
                jsonschema::options()
                    .with_retriever(EmbeddedCommon {
                        schema: serde_json::from_str(COMMON).unwrap()
                    })
                    .build(&schema)
                    .is_err()
            );
        }
        let uri: Uri<String> = COMMON_URI.parse().unwrap();
        assert_eq!(retriever.retrieve(&uri).unwrap()["$id"], COMMON_URI);
    }
}
