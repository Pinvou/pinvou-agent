//! Fixed request Schema resources and caching, never caller-selected retrieval.
use super::{ClientRequestError, ClientRequestOperation};
use jsonschema::{Draft, Retrieve, Uri, Validator};
use serde_json::Value;
use std::sync::OnceLock;
macro_rules! frozen {
    ($path:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../../../specs/upgrade/v1/schemas/",
            $path
        ))
    };
}
const RESOURCES: [(&str, &str); 19] = [
    (
        "urn:pinvou:upgrade:v1:time:request",
        frozen!("time/request.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:api:current-facts",
        frozen!("api/current-facts.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:api:event-facts",
        frozen!("api/event-facts.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:api:gray-targeting",
        frozen!("api/gray-targeting.json"),
    ),
    ("urn:pinvou:upgrade:v1:api:scope", frozen!("api/scope.json")),
    (
        "urn:pinvou:upgrade:v1:api:status-request",
        frozen!("api/status-request.json"),
    ),
    ("urn:pinvou:upgrade:v1:common", frozen!("common.json")),
    (
        "urn:pinvou:upgrade:v1:credentials:activate-transaction-event",
        frozen!("credentials/activate-transaction-event.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:credentials:authorization-activate",
        frozen!("credentials/authorization-activate.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:credentials:authorization-install",
        frozen!("credentials/authorization-install.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:credentials:authorization-preinstall",
        frozen!("credentials/authorization-preinstall.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:credentials:decision",
        frozen!("credentials/decision.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:credentials:install-transaction-event",
        frozen!("credentials/install-transaction-event.json"),
    ),
    (
        "urn:pinvou:upgrade:v1:credentials:preinstall-transaction-event",
        frozen!("credentials/preinstall-transaction-event.json"),
    ),
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
    (
        "urn:pinvou:upgrade:v1:credentials:telemetry-event",
        frozen!("credentials/telemetry-event.json"),
    ),
];
const SCHEMAS: [(ClientRequestOperation, &str); 27] = [
    (
        ClientRequestOperation::Time,
        frozen!("api/time-request.json"),
    ),
    (
        ClientRequestOperation::Check,
        frozen!("api/check-request.json"),
    ),
    (
        ClientRequestOperation::Refresh,
        frozen!("api/refresh-request.json"),
    ),
    (
        ClientRequestOperation::DownloadInfo,
        frozen!("api/download-info-request.json"),
    ),
    (
        ClientRequestOperation::ValidatePreinstall,
        frozen!("api/validate-preinstall-request.json"),
    ),
    (
        ClientRequestOperation::ConsumePreinstall,
        frozen!("api/consume-preinstall-request.json"),
    ),
    (
        ClientRequestOperation::CancelPreinstall,
        frozen!("api/cancel-preinstall-request.json"),
    ),
    (
        ClientRequestOperation::ExecutionReviewPreinstall,
        frozen!("api/execution-review-preinstall-request.json"),
    ),
    (
        ClientRequestOperation::ValidateInstall,
        frozen!("api/validate-install-request.json"),
    ),
    (
        ClientRequestOperation::ConsumeInstall,
        frozen!("api/consume-install-request.json"),
    ),
    (
        ClientRequestOperation::CancelInstall,
        frozen!("api/cancel-install-request.json"),
    ),
    (
        ClientRequestOperation::ExecutionReviewInstall,
        frozen!("api/execution-review-install-request.json"),
    ),
    (
        ClientRequestOperation::ValidateActivate,
        frozen!("api/validate-activate-request.json"),
    ),
    (
        ClientRequestOperation::ConsumeActivate,
        frozen!("api/consume-activate-request.json"),
    ),
    (
        ClientRequestOperation::CancelActivate,
        frozen!("api/cancel-activate-request.json"),
    ),
    (
        ClientRequestOperation::ExecutionReviewActivate,
        frozen!("api/execution-review-activate-request.json"),
    ),
    (
        ClientRequestOperation::ConsumeStatus,
        frozen!("api/consume-status-request.json"),
    ),
    (
        ClientRequestOperation::HelperCleanup,
        frozen!("api/helper-cleanup-request.json"),
    ),
    (
        ClientRequestOperation::ChannelChange,
        frozen!("api/channel-change-request.json"),
    ),
    (
        ClientRequestOperation::TelemetryEvents,
        frozen!("api/events-telemetry-event-request.json"),
    ),
    (
        ClientRequestOperation::PreinstallEvents,
        frozen!("api/events-preinstall-transaction-event-request.json"),
    ),
    (
        ClientRequestOperation::InstallEvents,
        frozen!("api/events-install-transaction-event-request.json"),
    ),
    (
        ClientRequestOperation::ActivateEvents,
        frozen!("api/events-activate-transaction-event-request.json"),
    ),
    (
        ClientRequestOperation::ReconciliationEvents,
        frozen!("api/events-reconciliation-event-request.json"),
    ),
    (
        ClientRequestOperation::RecoveryReviewEvents,
        frozen!("api/events-recovery-review-event-request.json"),
    ),
    (
        ClientRequestOperation::ReconciliationAction,
        frozen!("api/reconciliation-action-request.json"),
    ),
    (
        ClientRequestOperation::RecoveryReviewAction,
        frozen!("api/recovery-review-action-request.json"),
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
            .map(|(_, value)| value.clone())
            .ok_or_else(|| "unregistered client request schema resource".into())
    }
}
type Registry = Vec<(ClientRequestOperation, Validator)>;
static REGISTRY: OnceLock<Result<Registry, ClientRequestError>> = OnceLock::new();
pub(super) fn validator(
    operation: ClientRequestOperation,
) -> Result<&'static Validator, ClientRequestError> {
    let registry = REGISTRY
        .get_or_init(|| {
            let unavailable = ClientRequestError::SchemaUnavailable;
            let resources: Vec<(&'static str, Value)> = RESOURCES
                .iter()
                .map(|(id, source)| {
                    serde_json::from_str(source)
                        .map(|value| (*id, value))
                        .map_err(|_| unavailable)
                })
                .collect::<Result<_, _>>()?;
            SCHEMAS
                .iter()
                .map(|(operation, source)| {
                    let schema: Value = serde_json::from_str(source).map_err(|_| unavailable)?;
                    let validator = jsonschema::options()
                        .with_draft(Draft::Draft202012)
                        .should_validate_formats(false)
                        .with_retriever(Embedded {
                            resources: resources.clone(),
                        })
                        .build(&schema)
                        .map_err(|_| unavailable)?;
                    Ok((*operation, validator))
                })
                .collect()
        })
        .as_ref()
        .map_err(|error| *error)?;
    registry
        .iter()
        .find(|(op, _)| *op == operation)
        .map(|(_, validator)| validator)
        .ok_or(ClientRequestError::SchemaUnavailable)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn request_resources_are_local_and_operation_inventory_is_exact_unique() {
        let resources: Vec<_> = RESOURCES
            .iter()
            .map(|(id, s)| (*id, serde_json::from_str(s).unwrap()))
            .collect();
        let embedded = Embedded {
            resources: resources.clone(),
        };
        for id in [
            "https://example.invalid/schema",
            "file:///schema.json",
            "urn:pinvou:upgrade:v1:api:check-response",
        ] {
            let uri: Uri<String> = id.parse().unwrap();
            assert!(embedded.retrieve(&uri).is_err());
            assert!(
                jsonschema::options()
                    .with_retriever(Embedded {
                        resources: resources.clone()
                    })
                    .build(&serde_json::json!({"$ref":id}))
                    .is_err()
            );
        }
        let names: std::collections::BTreeSet<_> =
            SCHEMAS.iter().map(|(op, _)| op.as_str()).collect();
        assert_eq!(
            names,
            ClientRequestOperation::ALL
                .into_iter()
                .map(|op| op.as_str())
                .collect()
        );
        assert_eq!(names.len(), 27);
        for (op, _) in SCHEMAS {
            assert!(validator(op).is_ok());
        }
    }
}
