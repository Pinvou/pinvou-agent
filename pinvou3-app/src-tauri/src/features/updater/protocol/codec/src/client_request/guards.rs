//! Original operation request guards, not current eligibility or post-auth proof.
use super::{ClientRequestError, ClientRequestOperation as Op};
use crate::Document;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::Value;
use sha2::{Digest, Sha256};
fn purpose(operation: Op) -> Option<&'static str> {
    match operation {
        Op::ValidatePreinstall
        | Op::ConsumePreinstall
        | Op::CancelPreinstall
        | Op::ExecutionReviewPreinstall => Some("preinstall"),
        Op::ValidateInstall
        | Op::ConsumeInstall
        | Op::CancelInstall
        | Op::ExecutionReviewInstall => Some("install"),
        Op::ValidateActivate
        | Op::ConsumeActivate
        | Op::CancelActivate
        | Op::ExecutionReviewActivate => Some("activate"),
        _ => None,
    }
}
fn hash(value: &Value) -> Result<String, ClientRequestError> {
    let canonical = Document {
        value: value.clone(),
    }
    .canonical_bytes()
    .map_err(ClientRequestError::Json)?;
    Ok(Sha256::digest(canonical)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
pub(super) fn assert_relationships(value: &Value, operation: Op) -> Result<(), ClientRequestError> {
    match operation {
        Op::Check => {
            if !value["supportedIncrementalAlgorithms"]
                .as_array()
                .ok_or(ClientRequestError::InvalidContract)?
                .is_empty()
                || !value["supportedIncrementalFormats"]
                    .as_array()
                    .ok_or(ClientRequestError::InvalidContract)?
                    .is_empty()
            {
                return Err(ClientRequestError::IncrementalNotEnabled);
            }
        }
        Op::ConsumePreinstall | Op::ConsumeInstall | Op::ConsumeActivate => {
            let semantic = &value["semanticRequest"];
            let digest = Document {
                value: semantic.clone(),
            }
            .consume_request_digest()
            .map_err(ClientRequestError::Digest)?;
            if semantic["purpose"]
                != purpose(operation).ok_or(ClientRequestError::InvalidContract)?
                || value["consumeRequestDigest"] != digest.request_digest()
                || semantic["product"] != value["scope"]["product"]
                || semantic["component"] != value["scope"]["component"]
                || semantic["installationScopeId"] != value["scope"]["installationScopeId"]
            {
                return Err(ClientRequestError::ConsumeMismatch);
            }
        }
        Op::ConsumeStatus => {
            let secret = value["recoverySecret"]
                .as_str()
                .ok_or(ClientRequestError::InvalidContract)?;
            let decoded = URL_SAFE_NO_PAD
                .decode(secret)
                .map_err(|_| ClientRequestError::InvalidContract)?;
            if decoded.len() != 32 || URL_SAFE_NO_PAD.encode(decoded) != secret {
                return Err(ClientRequestError::InvalidContract);
            }
        }
        Op::ValidatePreinstall | Op::ValidateInstall | Op::ValidateActivate => {
            let valid = if operation == Op::ValidatePreinstall {
                value["freezeEpoch"].is_null()
                    && value["backup"].is_null()
                    && value["staged"].is_null()
                    && !value["preinstallSlot"].is_null()
            } else {
                !value["freezeEpoch"].is_null()
                    && value["preinstallSlot"].is_null()
                    && (if operation == Op::ValidateActivate {
                        !value["staged"].is_null()
                    } else {
                        value["staged"].is_null()
                    })
            };
            if !valid {
                return Err(ClientRequestError::PurposeMismatch);
            }
        }
        Op::CancelPreinstall | Op::CancelInstall | Op::CancelActivate => {
            if value["authorizationJti"] != value["authorization"]["signed"]["authorizationJti"]
                || value["authorization"]["signed"]["purpose"]
                    != purpose(operation).ok_or(ClientRequestError::InvalidContract)?
            {
                return Err(ClientRequestError::PurposeMismatch);
            }
        }
        Op::TelemetryEvents
        | Op::PreinstallEvents
        | Op::InstallEvents
        | Op::ActivateEvents
        | Op::ReconciliationEvents
        | Op::RecoveryReviewEvents => {
            let claims = &value["credential"]["signed"];
            let lineage = claims
                .get("telemetrySessionId")
                .or_else(|| claims.get("transactionId"))
                .unwrap_or(&claims["task"]["taskId"]);
            let started = claims
                .get("sessionStartedAt")
                .or_else(|| claims.get("transactionStartedAt"))
                .unwrap_or(&claims["taskStartedAt"]);
            let start = started
                .as_f64()
                .ok_or(ClientRequestError::InvalidContract)?;
            let end = claims["eventNotAfter"]
                .as_f64()
                .ok_or(ClientRequestError::InvalidContract)?;
            let allowed = claims["allowedEvents"]
                .as_array()
                .ok_or(ClientRequestError::InvalidContract)?;
            for event in value["events"]
                .as_array()
                .ok_or(ClientRequestError::InvalidContract)?
            {
                let expected = if claims["purpose"] == "telemetry" {
                    &event["executionPurpose"]
                } else {
                    &claims["purpose"]
                };
                let occurred = event["occurredAt"]
                    .as_f64()
                    .ok_or(ClientRequestError::InvalidContract)?;
                if event["executionPurpose"] != *expected
                    || event["lineageId"] != *lineage
                    || event["factsSha256"] != hash(&event["facts"])?
                    || occurred < start
                    || occurred >= end
                    || !allowed.contains(&event["kind"])
                {
                    return Err(ClientRequestError::EventMismatch);
                }
            }
        }
        Op::ReconciliationAction | Op::RecoveryReviewAction => {
            if value["taskRevision"] != value["credential"]["signed"]["task"]["revision"]
                || value["evidenceSha256"] != hash(&value["evidence"])?
            {
                return Err(ClientRequestError::EvidenceMismatch);
            }
        }
        Op::ExecutionReviewPreinstall
        | Op::ExecutionReviewInstall
        | Op::ExecutionReviewActivate => {
            let claims = &value["authorization"]["signed"];
            if value["transactionId"] != claims["transactionId"]
                || [
                    "preparation",
                    "freezeEpoch",
                    "backup",
                    "staged",
                    "preinstallSlot",
                ]
                .iter()
                .any(|field| value[*field] != claims[*field])
            {
                return Err(ClientRequestError::PurposeMismatch);
            }
        }
        Op::Refresh | Op::DownloadInfo | Op::HelperCleanup | Op::ChannelChange => {}
    }
    Ok(())
}
