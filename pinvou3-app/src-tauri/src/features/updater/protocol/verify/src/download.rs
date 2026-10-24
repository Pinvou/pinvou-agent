//! Signed DownloadInfo correlation only; destination/current owner approval is external.
use crate::{VerificationError, VerificationInputs, verify_with_supplied_root};
use pinvou_upgrade_codec::{
    ClientRequestError, ClientRequestOperation, Document, DownloadInfoContractError, Error,
    InputKind, RootPolicyShape, SchemaCheckedEnvelope, SuppliedProtocolTime,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadVerificationError {
    Request(ClientRequestError),
    Response(DownloadInfoContractError),
    Verification(VerificationError),
    Json(Error),
    RequestBindingMismatch,
    UrlExpired,
}
impl std::fmt::Display for DownloadVerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(e) => e.fmt(f),
            Self::Response(e) => e.fmt(f),
            Self::Verification(e) => e.fmt(f),
            Self::Json(e) => e.fmt(f),
            Self::RequestBindingMismatch => f.write_str("DOWNLOAD_REQUEST_BINDING_MISMATCH"),
            Self::UrlExpired => f.write_str("DOWNLOAD_URL_EXPIRED"),
        }
    }
}
impl std::error::Error for DownloadVerificationError {}
/// Independent original request plus caller's existing trust/time/global denial.
pub struct DownloadVerificationInputs<'a> {
    root: &'a RootPolicyShape,
    request: &'a Document,
    now: &'a SuppliedProtocolTime,
    denied: &'a [String],
}
impl<'a> DownloadVerificationInputs<'a> {
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
/// The immutable source and original package identity asserted by a checked
/// Download credential. Equality deliberately excludes its URL, credential ID
/// and lifetime, so a newly authenticated URL may name the same object.
///
/// This is a signed statement, not native destination ownership, a retained
/// prefix proof, current qualification or permission to fetch/install. Only the
/// response verifier constructs it; deserializing a cache record cannot do so.
#[derive(Clone, PartialEq, Eq)]
pub struct SignatureCheckedDownloadSource {
    source_id: String,
    object_id: String,
    object_version: String,
    package_id: String,
    size: u64,
    sha256: String,
}
impl SignatureCheckedDownloadSource {
    pub fn source_id(&self) -> &str {
        &self.source_id
    }
    pub fn object_id(&self) -> &str {
        &self.object_id
    }
    pub fn object_version(&self) -> &str {
        &self.object_version
    }
    pub fn package_id(&self) -> &str {
        &self.package_id
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}
/// Checked canonical Decision/Download and exact original transport bytes.
/// This is no native/metadata/Registry/workflow/destination/artifact/rights proof.
pub struct SignatureCheckedDownloadResponse<'a> {
    decision: SchemaCheckedEnvelope,
    download: SchemaCheckedEnvelope,
    source: SignatureCheckedDownloadSource,
    response_bytes: Vec<u8>,
    request: &'a Document,
    root: &'a RootPolicyShape,
    supplied_now: f64,
}
impl SignatureCheckedDownloadResponse<'_> {
    pub fn decision(&self) -> &SchemaCheckedEnvelope {
        &self.decision
    }
    pub fn download_credential(&self) -> &SchemaCheckedEnvelope {
        &self.download
    }
    pub fn source_identity(&self) -> &SignatureCheckedDownloadSource {
        &self.source
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
    pub fn supplied_time_millis(&self) -> f64 {
        self.supplied_now
    }
}
/// Verify both current credentials and full inherited qualification. Expired
/// Decision/URL requires refresh/renewal, never terminating a logical download or
/// imposing a total download duration. Existing T29 destination/file checks and
/// protected current owner qualification remain mandatory before actual use.
pub fn verify_download_info_response<'a>(
    response_bytes: &[u8],
    inputs: DownloadVerificationInputs<'a>,
) -> Result<SignatureCheckedDownloadResponse<'a>, DownloadVerificationError> {
    inputs
        .request
        .assert_supplied_client_request_contract(ClientRequestOperation::DownloadInfo)
        .map_err(DownloadVerificationError::Request)?;
    let response = parse(response_bytes)?;
    response
        .assert_supplied_download_info_contract()
        .map_err(DownloadVerificationError::Response)?;
    let request = inputs.request.value();
    let scope = &request["scope"];
    let expected = |role: &str| {
        parse(format!(
        "{{\"role\":\"{role}\",\"product\":{},\"component\":{},\"scope\":{{\"installationScopeId\":{},\"channel\":{},\"channelRevision\":{},\"targetKey\":{}}}}}",
        scope["product"], scope["component"], scope["installationScopeId"], scope["channel"], scope["channelRevision"], scope["targetKey"],
    ).as_bytes())
    };
    let authenticate = |object: &Document, role: &str| {
        let canonical = object
            .canonical_bytes()
            .map_err(DownloadVerificationError::Json)?;
        let context = expected(role)?;
        verify_with_supplied_root(
            &canonical,
            VerificationInputs::new(inputs.root, &context, inputs.now)
                .with_denied_key_ids(inputs.denied),
        )
        .map(|proof| proof.envelope)
        .map_err(DownloadVerificationError::Verification)
    };
    let decision = authenticate(
        &parse(request["decision"].to_string().as_bytes())?,
        "decision",
    )?;
    let download = authenticate(
        &parse(response.value()["credential"].to_string().as_bytes())?,
        "download",
    )?;
    let decision_claims = decision.envelope().signed().value();
    let download_claims = download.envelope().signed().value();
    if decision_claims["updateAvailable"] != true
        || request["packageId"] != decision_claims["update"]["hopChain"]["packageId"]
        || download_claims["packageId"] != request["packageId"]
    {
        return Err(DownloadVerificationError::RequestBindingMismatch);
    }
    // Compare all inherited qualification fields, excluding role-specific
    // credential identity, signatures and windows. The new role has its own
    // actual signature policy and current-time semantics, never extended here.
    for field in [
        "product",
        "component",
        "scope",
        "installId",
        "decisionId",
        "decisionRevision",
        "telemetrySessionId",
        "currentVersion",
        "host",
        "selectionGeneration",
        "metadataSet",
        "registryVersion",
        "sourceProfileId",
        "clientFactsDigest",
        "updaterFactsDigest",
        "helperFactsDigest",
        "launcherFactsDigest",
        "forwardPath",
        "update",
    ] {
        if decision_claims[field] != download_claims[field] {
            return Err(DownloadVerificationError::RequestBindingMismatch);
        }
    }
    // Complete response Schema already proved a nonnegative safe integer.
    let expires =
        response.value()["expiresAt"]
            .as_f64()
            .ok_or(DownloadVerificationError::Response(
                DownloadInfoContractError::InvalidContract,
            ))?;
    if inputs.now.as_millis() >= expires {
        return Err(DownloadVerificationError::UrlExpired);
    }
    let text = |value: Option<&str>| {
        value
            .map(str::to_owned)
            .ok_or(DownloadVerificationError::Response(
                DownloadInfoContractError::InvalidContract,
            ))
    };
    let binding = &download_claims["sourceBinding"];
    let source = SignatureCheckedDownloadSource {
        source_id: text(binding["sourceId"].as_str())?,
        object_id: text(binding["objectId"].as_str())?,
        object_version: text(binding["objectVersion"].as_str())?,
        package_id: text(download_claims["packageId"].as_str())?,
        // The checked credential Schema already proves a positive safe integer.
        size: download_claims["package"]["size"].as_f64().ok_or(
            DownloadVerificationError::Response(DownloadInfoContractError::InvalidContract),
        )? as u64,
        sha256: text(download_claims["package"]["sha256"].as_str())?,
    };
    Ok(SignatureCheckedDownloadResponse {
        decision,
        download,
        source,
        response_bytes: response_bytes.to_vec(),
        request: inputs.request,
        root: inputs.root,
        supplied_now: inputs.now.as_millis(),
    })
}
fn parse(bytes: &[u8]) -> Result<Document, DownloadVerificationError> {
    Document::parse(bytes, InputKind::Metadata).map_err(DownloadVerificationError::Json)
}
