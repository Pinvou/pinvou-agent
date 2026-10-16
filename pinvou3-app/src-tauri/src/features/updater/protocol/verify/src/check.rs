//! Authenticated successful response correlation; metadata acceptance remains external.
use crate::{VerificationError, VerificationInputs, verify_with_supplied_root};
use pinvou_upgrade_codec::{
    CheckResponseError, CheckResponseOperation, ClientRequestError, ClientRequestOperation,
    Document, Error, InputKind, RootPolicyShape, SchemaCheckedEnvelope, SourceProfileError,
    SuppliedProtocolTime,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckVerificationError {
    Request(ClientRequestError),
    Response(CheckResponseError),
    Verification(VerificationError),
    Json(Error),
    Source(SourceProfileError),
    RequestBindingMismatch,
}
impl std::fmt::Display for CheckVerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(e) => e.fmt(f),
            Self::Response(e) => e.fmt(f),
            Self::Verification(e) => e.fmt(f),
            Self::Json(e) => e.fmt(f),
            Self::Source(e) => e.fmt(f),
            Self::RequestBindingMismatch => f.write_str("CHECK_REQUEST_BINDING_MISMATCH"),
        }
    }
}
impl std::error::Error for CheckVerificationError {}

/// Independent original request, existing Root and safe time; not remote authority.
pub struct CheckVerificationInputs<'a> {
    root: &'a RootPolicyShape,
    request: &'a Document,
    now: &'a SuppliedProtocolTime,
    denied: &'a [String],
}
impl<'a> CheckVerificationInputs<'a> {
    pub fn new(
        root: &'a RootPolicyShape,
        request: &'a Document,
        now: &'a SuppliedProtocolTime,
    ) -> Self {
        Self {
            root,
            request,
            now,
            denied: &[],
        }
    }
    pub fn with_denied_key_ids(mut self, ids: &'a [String]) -> Self {
        self.denied = ids;
        self
    }
}

/// Canonical signed credential identities and original transport bytes only.
/// This does not qualify metadata/Registry/generation, accept a workflow writer,
/// attest native facts or grant selection/execution rights.
pub struct SignatureCheckedCheckResponse<'a> {
    decision: SchemaCheckedEnvelope,
    telemetry: SchemaCheckedEnvelope,
    response_bytes: Vec<u8>,
    request: &'a Document,
    root: &'a RootPolicyShape,
    operation: CheckResponseOperation,
    supplied_now: f64,
}
impl SignatureCheckedCheckResponse<'_> {
    pub fn decision(&self) -> &SchemaCheckedEnvelope {
        &self.decision
    }
    pub fn telemetry_credential(&self) -> &SchemaCheckedEnvelope {
        &self.telemetry
    }
    pub fn original_response_bytes(&self) -> &[u8] {
        &self.response_bytes
    }
    pub fn supplied_request(&self) -> &Document {
        self.request
    }
    pub fn supplied_root(&self) -> &RootPolicyShape {
        self.root
    }
    pub fn operation(&self) -> CheckResponseOperation {
        self.operation
    }
    pub fn supplied_time_millis(&self) -> f64 {
        self.supplied_now
    }
}

/// Complete frozen request/response contracts plus actual authentication of both
/// new credentials and normative request-fact correlation. The old Refresh
/// request credential has contract semantics only: its expiry does not impose a
/// download duration limit. Metadata and workflow owner checks remain necessary.
pub fn verify_check_response<'a>(
    response_bytes: &[u8],
    operation: CheckResponseOperation,
    inputs: CheckVerificationInputs<'a>,
) -> Result<SignatureCheckedCheckResponse<'a>, CheckVerificationError> {
    let request_operation = match operation {
        CheckResponseOperation::Check => ClientRequestOperation::Check,
        CheckResponseOperation::Refresh => ClientRequestOperation::Refresh,
    };
    inputs
        .request
        .assert_supplied_client_request_contract(request_operation)
        .map_err(CheckVerificationError::Request)?;
    let response = parse(response_bytes)?;
    response
        .assert_supplied_check_response_contract(operation)
        .map_err(CheckVerificationError::Response)?;
    let request = inputs.request.value();
    let scope = &request["scope"];
    let facts = &request["facts"];
    let stable = &facts["stableFacts"];
    for field in ["product", "component", "targetKey"] {
        if stable[field] != scope[field] {
            return Err(CheckVerificationError::RequestBindingMismatch);
        }
    }
    let profile = parse(stable.to_string().as_bytes())?
        .source_profile()
        .map_err(CheckVerificationError::Source)?;
    // Expected identity comes exclusively from the independent validated request.
    let expected = |role: &str| {
        parse(format!(
        "{{\"role\":\"{role}\",\"product\":{},\"component\":{},\"scope\":{{\"installationScopeId\":{},\"channel\":{},\"channelRevision\":{},\"targetKey\":{}}}}}",
        scope["product"], scope["component"], scope["installationScopeId"], scope["channel"], scope["channelRevision"], scope["targetKey"],
    ).as_bytes())
    };
    let authenticate = |field: &str, role: &str| {
        // Embedded objects have canonical identities; transport substrings are
        // not original canonical envelopes. The contract already bounds each.
        let canonical = parse(response.value()[field].to_string().as_bytes())?
            .canonical_bytes()
            .map_err(CheckVerificationError::Json)?;
        let context = expected(role)?;
        verify_with_supplied_root(
            &canonical,
            VerificationInputs::new(inputs.root, &context, inputs.now)
                .with_denied_key_ids(inputs.denied),
        )
        .map(|proof| proof.envelope)
        .map_err(CheckVerificationError::Verification)
    };
    let decision = authenticate("decision", "decision")?;
    let telemetry = authenticate("telemetryCredential", "telemetry-event")?;
    let claims = decision.envelope().signed().value();
    if claims["requestNonce"] != request["requestNonce"]
        || claims["installId"] != scope["installId"]
        || claims["currentVersion"] != stable["canonicalAppVersion"]
        || claims["sourceProfileId"].as_str() != Some(profile.source_profile_id())
    {
        return Err(CheckVerificationError::RequestBindingMismatch);
    }
    for field in [
        "host",
        "clientFactsDigest",
        "updaterFactsDigest",
        "helperFactsDigest",
        "launcherFactsDigest",
    ] {
        if claims[field] != facts[field] {
            return Err(CheckVerificationError::RequestBindingMismatch);
        }
    }
    if operation == CheckResponseOperation::Refresh
        && claims["updateAvailable"] == true
        && claims["update"]["workflowId"] != request["workflowId"]
    {
        return Err(CheckVerificationError::RequestBindingMismatch);
    }
    Ok(SignatureCheckedCheckResponse {
        decision,
        telemetry,
        response_bytes: response_bytes.to_vec(),
        request: inputs.request,
        root: inputs.root,
        operation,
        supplied_now: inputs.now.as_millis(),
    })
}
fn parse(bytes: &[u8]) -> Result<Document, CheckVerificationError> {
    Document::parse(bytes, InputKind::Metadata).map_err(CheckVerificationError::Json)
}
