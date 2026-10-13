//! Original embedded-credential semantics and request-scope binding only.
use super::ClientRequestError;
use crate::{BoundedOutput, Document, Error};
use serde_json::{Value, json};
fn collect<'a>(value: &'a Value, envelopes: &mut Vec<&'a Value>) {
    match value {
        Value::Object(object)
            if object.contains_key("signed") && object.contains_key("signatures") =>
        {
            envelopes.push(value);
        }
        Value::Object(object) => {
            for child in object.values() {
                collect(child, envelopes);
            }
        }
        Value::Array(array) => {
            for child in array {
                collect(child, envelopes);
            }
        }
        _ => {}
    }
}
pub(super) fn assert_credentials(value: &Value) -> Result<(), ClientRequestError> {
    // Document already bounds depth/member/whole bytes; discovery adds no
    // independent envelope-count or batch-cap policy beyond those original bounds.
    let mut envelopes = Vec::new();
    collect(value, &mut envelopes);
    for envelope in envelopes {
        let mut output = BoundedOutput {
            bytes: Vec::new(),
            exceeded: false,
        };
        serde_jcs::to_writer(&mut output, envelope).map_err(|_| {
            ClientRequestError::Json(if output.exceeded {
                Error::OutputLimit
            } else {
                Error::Canonicalization
            })
        })?;
        if output.bytes.len() > crate::CREDENTIAL_BYTES {
            return Err(ClientRequestError::CredentialTooLarge);
        }
        let claims = &envelope["signed"];
        let iat = claims["iat"]
            .as_f64()
            .ok_or(ClientRequestError::InvalidContract)?;
        let nbf = claims["nbf"]
            .as_f64()
            .ok_or(ClientRequestError::InvalidContract)?;
        let time = Document {
            value: Value::from(iat.max(nbf)),
        }
        .protocol_time_shape()
        .map_err(ClientRequestError::Time)?;
        crate::credential_semantics::assert_claims_semantics(claims, &time)
            .map_err(ClientRequestError::Semantics)?;
        if let Some(scope) = value.get("scope") {
            let expected = json!({"installationScopeId":scope["installationScopeId"],
                "channel":scope["channel"],"channelRevision":scope["channelRevision"],"targetKey":scope["targetKey"]});
            // Normalized complete Value equality is original sameJson/JCS equality.
            if claims["product"] != scope["product"]
                || claims["component"] != scope["component"]
                || claims["installId"] != scope["installId"]
                || claims["scope"] != expected
            {
                return Err(ClientRequestError::ScopeMismatch);
            }
        }
    }
    Ok(())
}
