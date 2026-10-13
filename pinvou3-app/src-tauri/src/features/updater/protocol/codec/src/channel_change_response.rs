//! Supplied channel-change response contract, never persisted settings or qualifications.
use crate::{Document, Error, InputKind};
use jsonschema::{Draft, Retrieve, Uri, Validator};
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelChangeResponseError {
    Json(Error),
    SchemaUnavailable,
    InvalidContract,
}
impl std::fmt::Display for ChannelChangeResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => e.fmt(f),
            Self::SchemaUnavailable => f.write_str("API_SCHEMA_UNAVAILABLE"),
            Self::InvalidContract => f.write_str("API_CONTRACT_INVALID"),
        }
    }
}
impl std::error::Error for ChannelChangeResponseError {}

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
const RESOURCES: [(&str, &str); 1] = [(
    "urn:pinvou:upgrade:v1:common",
    frozen_schema!("common.json"),
)];
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
            .ok_or_else(|| "unregistered channel response schema resource".into())
    }
}
static REGISTRY: OnceLock<Result<Validator, ChannelChangeResponseError>> = OnceLock::new();
fn registry() -> Result<&'static Validator, ChannelChangeResponseError> {
    REGISTRY
        .get_or_init(|| {
            fn build(source: &str) -> Result<Validator, ChannelChangeResponseError> {
                let resources = RESOURCES
                    .iter()
                    .map(|(id, source)| {
                        serde_json::from_str(source)
                            .map(|v| (*id, v))
                            .map_err(|_| ChannelChangeResponseError::SchemaUnavailable)
                    })
                    .collect::<Result<_, _>>()?;
                let schema: Value = serde_json::from_str(source)
                    .map_err(|_| ChannelChangeResponseError::SchemaUnavailable)?;
                jsonschema::options()
                    .with_draft(Draft::Draft202012)
                    .should_validate_formats(false)
                    .with_retriever(EmbeddedResources { resources })
                    .build(&schema)
                    .map_err(|_| ChannelChangeResponseError::SchemaUnavailable)
            }
            build(frozen_schema!("api/channel-change-response.json"))
        })
        .as_ref()
        .map_err(|e| *e)
}
impl Document {
    /// Complete original channel-change response contract only. Supplied channel
    /// and revision do not prove a settings write, current generation, correlation
    /// or right to reuse prior qualifications after changing channel.
    pub fn assert_supplied_channel_change_response_contract(
        &self,
    ) -> Result<(), ChannelChangeResponseError> {
        let canonical = self
            .canonical_bytes()
            .map_err(ChannelChangeResponseError::Json)?;
        let reparsed = Document::parse(&canonical, InputKind::Metadata)
            .map_err(ChannelChangeResponseError::Json)?;
        if !registry()?.is_valid(reparsed.value()) {
            return Err(ChannelChangeResponseError::InvalidContract);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn channel_response_retriever_denies_every_external_or_unregistered_resource() {
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
