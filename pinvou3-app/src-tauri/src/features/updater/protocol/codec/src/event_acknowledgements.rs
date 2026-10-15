//! Supplied original event batch/receipt correspondence, never queue authority.
use crate::{ClientRequestError, ClientRequestOperation, ClientResponseError, Document, Error};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventAcknowledgementError {
    Json(Error),
    Request(ClientRequestError),
    Response(ClientResponseError),
    UnsupportedOperation,
    InvalidStatus,
    EventConflict,
    AcknowledgementMismatch,
}

impl std::fmt::Display for EventAcknowledgementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => error.fmt(f),
            Self::Request(error) => error.fmt(f),
            Self::Response(error) => error.fmt(f),
            Self::UnsupportedOperation => f.write_str("EVENT_ACK_OPERATION_UNSUPPORTED"),
            Self::InvalidStatus => f.write_str("EVENT_ACK_HTTP_STATUS_INVALID"),
            Self::EventConflict => f.write_str("EVENT_ACK_IMMUTABLE_EVENT_CONFLICT"),
            Self::AcknowledgementMismatch => f.write_str("EVENT_ACK_BATCH_MISMATCH"),
        }
    }
}
impl std::error::Error for EventAcknowledgementError {}

impl Document {
    /// Match a supplied HTTP 200 receipt to this complete original event batch.
    /// Both documents retain their original metadata/credential contract bounds.
    /// Identical immutable duplicates retain their positions and dispositions;
    /// one credential/event key cannot describe two different canonical events.
    ///
    /// Unit success proves only supplied-document correspondence. It grants no
    /// authenticated transport, current Root/deny/time proof, remote commitment,
    /// durable queue acknowledgement or permission to erase local pending data.
    pub fn assert_supplied_event_acknowledgements(
        &self,
        response: &Document,
        operation: ClientRequestOperation,
        http_status: u16,
    ) -> Result<(), EventAcknowledgementError> {
        use ClientRequestOperation as Op;
        if !matches!(
            operation,
            Op::TelemetryEvents
                | Op::PreinstallEvents
                | Op::InstallEvents
                | Op::ActivateEvents
                | Op::ReconciliationEvents
                | Op::RecoveryReviewEvents
        ) {
            return Err(EventAcknowledgementError::UnsupportedOperation);
        }
        if http_status != 200 {
            return Err(EventAcknowledgementError::InvalidStatus);
        }
        self.assert_supplied_client_request_contract(operation)
            .map_err(EventAcknowledgementError::Request)?;
        response
            .assert_supplied_client_response_contract(operation)
            .map_err(EventAcknowledgementError::Response)?;
        let events = self.value()["events"]
            .as_array()
            .ok_or(EventAcknowledgementError::AcknowledgementMismatch)?;
        let acknowledgements = response.value()["acknowledgements"]
            .as_array()
            .ok_or(EventAcknowledgementError::AcknowledgementMismatch)?;
        if events.len() != acknowledgements.len() {
            return Err(EventAcknowledgementError::AcknowledgementMismatch);
        }
        let credential_jti = self.value()["credential"]["signed"]["jti"]
            .as_str()
            .ok_or(EventAcknowledgementError::AcknowledgementMismatch)?;
        // This map is local to the original batch, never a global event-ID set.
        let mut originals = BTreeMap::new();
        for (event, acknowledgement) in events.iter().zip(acknowledgements) {
            let event_id = event["eventId"]
                .as_str()
                .ok_or(EventAcknowledgementError::AcknowledgementMismatch)?;
            let canonical = Document {
                value: event.clone(),
            }
            .canonical_bytes()
            .map_err(EventAcknowledgementError::Json)?;
            match originals.entry((credential_jti, event_id)) {
                std::collections::btree_map::Entry::Occupied(original)
                    if original.get() != &canonical =>
                {
                    return Err(EventAcknowledgementError::EventConflict);
                }
                std::collections::btree_map::Entry::Vacant(original) => {
                    original.insert(canonical);
                }
                _ => {}
            }
            if acknowledgement["eventId"].as_str() != Some(event_id) {
                return Err(EventAcknowledgementError::AcknowledgementMismatch);
            }
        }
        Ok(())
    }
}
