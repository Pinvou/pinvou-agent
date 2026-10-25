//! Supplied client HTTP error contracts, never trusted outcome or retry rights.
use crate::{ClientResponseOperation, Document, Error, InputKind};
use jsonschema::{Draft, Retrieve, Uri, Validator};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientErrorResponseError {
    Json(Error),
    SchemaUnavailable,
    InvalidContract,
}
impl std::fmt::Display for ClientErrorResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => e.fmt(f),
            Self::SchemaUnavailable => f.write_str("API_SCHEMA_UNAVAILABLE"),
            Self::InvalidContract => f.write_str("API_CONTRACT_INVALID"),
        }
    }
}
impl std::error::Error for ClientErrorResponseError {}
macro_rules! frozen {
    ($path:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/",
            $path
        ))
    };
}
const COMMON: &str = frozen!("schemas/common.json");
const ERROR_SCHEMA: &str = frozen!("schemas/api/error.json");
const UNACTIVATED_SCHEMA: &str = frozen!("schemas/api/unactivated.json");
const OPENAPI: &str = frozen!("openapi.yaml");
struct Embedded {
    common: Value,
}
impl Retrieve for Embedded {
    fn retrieve(
        &self,
        uri: &Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        if uri.as_str() == "urn:pinvou:upgrade:v1:common" {
            Ok(self.common.clone())
        } else {
            Err("unregistered client error schema resource".into())
        }
    }
}
type Allowed = BTreeMap<&'static str, BTreeSet<(u16, String)>>;
struct Registry {
    error: Validator,
    unactivated: Validator,
    allowed: Allowed,
}
static REGISTRY: OnceLock<Result<Registry, ClientErrorResponseError>> = OnceLock::new();
fn allowed_responses(openapi: &Value) -> Result<Allowed, ClientErrorResponseError> {
    let unavailable = ClientErrorResponseError::SchemaUnavailable;
    let paths = openapi["paths"].as_object().ok_or(unavailable)?;
    let mut allowed = Allowed::new();
    for path in paths.values() {
        // Root distribution is a separate GET DATA transport. This registry
        // owns only POST error contracts; it must still reject a malformed
        // POST or a missing member of the complete POST operation catalog.
        let post = match path.get("post") {
            Some(post) => post,
            None if path.get("get").is_some() => continue,
            None => return Err(unavailable),
        };
        let name = post["operationId"].as_str().ok_or(unavailable)?;
        let Ok(operation) = name.parse::<ClientResponseOperation>() else {
            continue;
        };
        let responses = post["responses"].as_object().ok_or(unavailable)?;
        let mut pairs = BTreeSet::new();
        for (status, response) in responses {
            let status: u16 = status.parse().map_err(|_| unavailable)?;
            if status == 200 {
                continue;
            }
            if !(400..=599).contains(&status) {
                return Err(unavailable);
            }
            let variants = response["content"]["application/json"]["schema"]["oneOf"]
                .as_array()
                .ok_or(unavailable)?;
            if variants.is_empty() {
                return Err(unavailable);
            }
            for variant in variants {
                let code = if variant.get("$ref").is_some() {
                    if variant["$ref"] != "#/components/schemas/api.unactivated" {
                        return Err(unavailable);
                    }
                    "SCOPE_UNACTIVATED"
                } else {
                    variant["properties"]["code"]["const"]
                        .as_str()
                        .ok_or(unavailable)?
                };
                if !pairs.insert((status, code.to_owned())) {
                    return Err(unavailable);
                }
            }
        }
        if pairs.is_empty() || allowed.insert(operation.as_str(), pairs).is_some() {
            return Err(unavailable);
        }
    }
    if allowed.len() != ClientResponseOperation::ALL.len()
        || ClientResponseOperation::ALL
            .iter()
            .any(|op| !allowed.contains_key(op.as_str()))
    {
        return Err(unavailable);
    }
    Ok(allowed)
}
fn registry() -> Result<&'static Registry, ClientErrorResponseError> {
    REGISTRY
        .get_or_init(|| {
            let unavailable = ClientErrorResponseError::SchemaUnavailable;
            let common: Value = serde_json::from_str(COMMON).map_err(|_| unavailable)?;
            let build = |source: &str| {
                let schema: Value = serde_json::from_str(source).map_err(|_| unavailable)?;
                jsonschema::options()
                    .with_draft(Draft::Draft202012)
                    .should_validate_formats(false)
                    .with_retriever(Embedded {
                        common: common.clone(),
                    })
                    .build(&schema)
                    .map_err(|_| unavailable)
            };
            let openapi: Value = serde_json::from_str(OPENAPI).map_err(|_| unavailable)?;
            Ok(Registry {
                error: build(ERROR_SCHEMA)?,
                unactivated: build(UNACTIVATED_SCHEMA)?,
                allowed: allowed_responses(&openapi)?,
            })
        })
        .as_ref()
        .map_err(|error| *error)
}
impl Document {
    /// Complete frozen client error response shape and operation/HTTP status
    /// membership. Supplied retryability/delay is data, never a retry deadline,
    /// confirmed transaction absence, execution rights or owner revival.
    pub fn assert_supplied_client_error_response_contract(
        &self,
        operation: ClientResponseOperation,
        http_status: u16,
    ) -> Result<(), ClientErrorResponseError> {
        let canonical = self
            .canonical_bytes()
            .map_err(ClientErrorResponseError::Json)?;
        let reparsed = Document::parse(&canonical, InputKind::Metadata)
            .map_err(ClientErrorResponseError::Json)?;
        let registry = registry()?;
        let value = reparsed.value();
        let validator = if value["code"] == "SCOPE_UNACTIVATED" {
            &registry.unactivated
        } else {
            &registry.error
        };
        if !validator.is_valid(value) {
            return Err(ClientErrorResponseError::InvalidContract);
        }
        let code = value["code"]
            .as_str()
            .ok_or(ClientErrorResponseError::InvalidContract)?;
        if !registry
            .allowed
            .get(operation.as_str())
            .is_some_and(|pairs| {
                pairs
                    .iter()
                    .any(|(status, allowed)| *status == http_status && allowed == code)
            })
        {
            return Err(ClientErrorResponseError::InvalidContract);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn client_error_resources_cannot_retrieve_external_or_unregistered_schema() {
        let resource = Embedded {
            common: serde_json::from_str(COMMON).unwrap(),
        };
        for id in [
            "https://example.invalid/schema",
            "file:///schema.json",
            "urn:pinvou:upgrade:v1:credentials:decision",
        ] {
            let uri: Uri<String> = id.parse().unwrap();
            assert!(resource.retrieve(&uri).is_err());
            assert!(
                jsonschema::options()
                    .with_retriever(Embedded {
                        common: resource.common.clone()
                    })
                    .build(&serde_json::json!({"$ref":id}))
                    .is_err()
            );
        }
    }
    #[test]
    fn frozen_openapi_membership_requires_every_client_operation_without_duplicates() {
        let original: Value = serde_json::from_str(OPENAPI).unwrap();
        assert_eq!(allowed_responses(&original).unwrap().len(), 27);
        let paths = original["paths"].as_object().unwrap();
        let root_get = &paths["/v1/upgrade/roots/{product}/{version}.json"];
        assert_eq!(root_get["get"]["operationId"], "read-root");
        assert!(root_get.get("post").is_none());
        let (key, value) = paths
            .iter()
            .find(|(_, path)| path["post"]["operationId"] == "check")
            .unwrap();
        let mut duplicate = original.clone();
        duplicate["paths"]["/duplicate-check"] = value.clone();
        assert_eq!(
            allowed_responses(&duplicate).err(),
            Some(ClientErrorResponseError::SchemaUnavailable)
        );
        // Replacing a required POST with a GET must not satisfy its census.
        let mut replaced = original.clone();
        let operation = replaced["paths"][key].as_object_mut().unwrap();
        let post = operation.remove("post").unwrap();
        operation.insert("get".to_owned(), post);
        assert_eq!(
            allowed_responses(&replaced).err(),
            Some(ClientErrorResponseError::SchemaUnavailable)
        );
        let mut malformed = original.clone();
        malformed["paths"][key]["post"] = Value::Null;
        assert_eq!(
            allowed_responses(&malformed).err(),
            Some(ClientErrorResponseError::SchemaUnavailable)
        );
        let mut missing = original.clone();
        missing["paths"].as_object_mut().unwrap().remove(key);
        assert_eq!(
            allowed_responses(&missing).err(),
            Some(ClientErrorResponseError::SchemaUnavailable)
        );
    }
}
