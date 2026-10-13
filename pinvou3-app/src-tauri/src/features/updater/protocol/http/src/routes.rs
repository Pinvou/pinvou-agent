//! Immutable original OpenAPI routing; no untrusted URL discovery.
use super::*;
const OPENAPI: &str = include_str!("../../../../../../../../specs/upgrade/v1/openapi.yaml");
fn inventory() -> Result<BTreeMap<String, String>, ControlHttpError> {
    let document: serde_json::Value =
        serde_json::from_str(OPENAPI).map_err(|_| ControlHttpError::RoutesUnavailable)?;
    let mut paths = BTreeMap::new();
    for (path, value) in document["paths"]
        .as_object()
        .ok_or(ControlHttpError::RoutesUnavailable)?
    {
        let post = &value["post"];
        if post["tags"] != serde_json::json!(["client"]) {
            continue;
        }
        let name = post["operationId"]
            .as_str()
            .ok_or(ControlHttpError::RoutesUnavailable)?;
        let operation: ClientRequestOperation = name
            .parse()
            .map_err(|_| ControlHttpError::RoutesUnavailable)?;
        if !path.starts_with("/v1/upgrade/")
            || !path
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'/' || b == b'-')
            || paths
                .insert(operation.as_str().to_owned(), path.clone())
                .is_some()
        {
            return Err(ControlHttpError::RoutesUnavailable);
        }
    }
    if paths.len() != Operation::ALL.len()
        || Operation::ALL
            .iter()
            .any(|op| !paths.contains_key(op.as_str()))
    {
        return Err(ControlHttpError::RoutesUnavailable);
    }
    Ok(paths)
}
pub(super) fn path(operation: Operation) -> Result<&'static str, ControlHttpError> {
    static PATHS: OnceLock<Result<BTreeMap<String, String>, ControlHttpError>> = OnceLock::new();
    PATHS
        .get_or_init(inventory)
        .as_ref()
        .map_err(|e| *e)?
        .get(operation.as_str())
        .map(String::as_str)
        .ok_or(ControlHttpError::RoutesUnavailable)
}
