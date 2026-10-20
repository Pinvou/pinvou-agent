use crate::{DOCUMENT_LIMIT, Error, MAX_SAFE, hash, json, valid_hash};
use pinvou_upgrade_codec::{Document, InputKind};
use pinvou_upgrade_file_http::{DestinationPolicy, FileOrigin};
use pinvou_upgrade_transfer::Artifact;
use pinvou_upgrade_verifier::{
    DownloadVerificationInputs, verify_archived_package, verify_download_info_response,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::net::{IpAddr, SocketAddr};

// These strings retain the original wire, including duplicate keys, until the
// original bounded codec rejects or authenticates it. Value decoding first would
// silently erase duplicate keys. Imported credentials/URLs are NEVER persisted.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Import {
    pub request: String,
    pub response: String,
    pub supplied_time_millis: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Manifest {
    pub protocol_version: u8,
    pub job_id: String,
    pub root: String,
    pub root_sha256: String,
    pub package: String,
    pub expected: String,
    pub qualification_sha256: String,
    pub initial_supplied_time_millis: u64,
}
pub(crate) struct Target {
    pub id: String,
    pub size: u64,
    pub digest: String,
    pub product: String,
    pub component: String,
    pub target: String,
}
impl Target {
    pub fn artifact(&self) -> Result<Artifact<'_>, Error> {
        Artifact::new(&self.id, self.size, &self.digest).map_err(|_| Error::InvalidInput)
    }
}
fn document(raw: &str) -> Result<Document, Error> {
    if raw.len() > DOCUMENT_LIMIT {
        return Err(Error::InvalidInput);
    }
    Document::parse(raw.as_bytes(), InputKind::Metadata).map_err(|_| Error::InvalidInput)
}
fn time(value: u64) -> Result<pinvou_upgrade_codec::SuppliedProtocolTime, Error> {
    if value > MAX_SAFE {
        return Err(Error::InvalidInput);
    }
    document(&value.to_string())?
        .protocol_time_shape()
        .map_err(|_| Error::InvalidInput)
}
impl Manifest {
    pub fn target(&self) -> Result<Target, Error> {
        if self.protocol_version != 1
            || !valid_hash(&self.job_id)
            || !valid_hash(&self.root_sha256)
            || !valid_hash(&self.qualification_sha256)
        {
            return Err(Error::InvalidInput);
        }
        let root = document(&self.root)?;
        let canonical = root.canonical_bytes().map_err(|_| Error::InvalidInput)?;
        if hash(&canonical) != self.root_sha256 {
            return Err(Error::PinMismatch);
        }
        let policy = root.root_policy_shape().map_err(|_| Error::InvalidInput)?;
        let expected = document(&self.expected)?;
        let now = time(self.initial_supplied_time_millis)?;
        let checked = verify_archived_package(self.package.as_bytes(), &policy, &expected, &now)
            .map_err(|_| Error::SignatureRefused)?;
        let signed = checked.envelope().envelope().signed().value();
        let full = &signed["fullPackage"];
        let text = |value: &Value| value.as_str().map(str::to_owned).ok_or(Error::InvalidInput);
        // Document intentionally represents all JSON numbers as binary64. The
        // original full Schema/semantics already proved this safe integer.
        let size = full["size"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 1.0 && *n <= MAX_SAFE as f64 && n.fract() == 0.0)
            .ok_or(Error::InvalidInput)? as u64;
        let target = Target {
            id: text(&full["packageId"])?,
            size,
            digest: text(&full["sha256"])?,
            product: text(&signed["product"])?,
            component: text(&signed["component"])?,
            target: text(&signed["scope"]["targetKey"])?,
        };
        target.artifact()?;
        Ok(target)
    }
    pub fn imported_url(&self, import: &Import) -> Result<String, Error> {
        if import.supplied_time_millis < self.initial_supplied_time_millis {
            return Err(Error::BindingMismatch);
        }
        let target = self.target()?;
        let root = document(&self.root)?
            .root_policy_shape()
            .map_err(|_| Error::InvalidInput)?;
        let request = document(&import.request)?;
        let now = time(import.supplied_time_millis)?;
        let checked = verify_download_info_response(
            import.response.as_bytes(),
            DownloadVerificationInputs::new(&root, &request, &now),
        )
        .map_err(|_| Error::SignatureRefused)?;
        let value = checked.download_credential().envelope().signed().value();
        let reference = document(&value["update"]["hopChain"]["packageManifest"].to_string())?
            .metadata_reference_shape()
            .map_err(|_| Error::BindingMismatch)?;
        reference
            .assert_original_envelope(self.package.as_bytes())
            .map_err(|_| Error::BindingMismatch)?;
        let scope = &value["scope"];
        if value["product"] != target.product
            || value["component"] != target.component
            || scope["targetKey"] != target.target
            || value["packageId"] != target.id
            || value["package"]["size"].as_f64() != Some(target.size as f64)
            || value["package"]["sha256"] != target.digest
            || qualification(&request)? != self.qualification_sha256
        {
            return Err(Error::BindingMismatch);
        }
        let response = document(&import.response)?;
        response.value()["url"]
            .as_str()
            .map(str::to_owned)
            .ok_or(Error::InvalidInput)
    }
}
pub(crate) fn qualification(request: &Document) -> Result<String, Error> {
    // A local QA job retains one original qualification projection. A newly
    // signed import can change credential/session IDs and windows, never this
    // target/selection/metadata/hop projection. This is NOT server workflow proof.
    let decision = &request.value()["decision"]["signed"];
    let mut retained = serde_json::Map::new();
    for field in [
        "product",
        "component",
        "scope",
        "installId",
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
        let value = decision.get(field).ok_or(Error::BindingMismatch)?;
        retained.insert(field.to_owned(), value.clone());
    }
    let raw = json(&Value::Object(retained))?;
    let canonical = Document::parse(&raw, InputKind::Metadata)
        .map_err(|_| Error::InvalidInput)?
        .canonical_bytes()
        .map_err(|_| Error::InvalidInput)?;
    Ok(hash(&canonical))
}
pub(crate) fn new(
    root: String,
    pin: String,
    package: String,
    expected: String,
    import: &Import,
) -> Result<Manifest, Error> {
    let manifest = Manifest {
        protocol_version: 1,
        job_id: "0".repeat(64),
        root,
        root_sha256: pin,
        package,
        expected,
        qualification_sha256: qualification(&document(&import.request)?)?,
        initial_supplied_time_millis: import.supplied_time_millis,
    };
    manifest.imported_url(import)?;
    Ok(manifest)
}

/// Explicit operator network consent, not a production destination approval.
/// There are no wildcards, redirects, proxies, caller headers or CA overrides.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkConsent {
    pub host: String,
    pub port: u16,
    pub addresses: Vec<IpAddr>,
}
impl NetworkConsent {
    pub fn validate(&self) -> Result<(), Error> {
        if self.host.is_empty()
            || self.host.len() > 253
            || self.port == 0
            || self.addresses.is_empty()
            || self.addresses.len() > 4096
            || self
                .host
                .bytes()
                .any(|b| !(b.is_ascii_alphanumeric() || b".-:".contains(&b)))
            || self
                .addresses
                .iter()
                .any(|ip| ip.is_unspecified() || ip.is_multicast())
        {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }
}
impl DestinationPolicy for NetworkConsent {
    fn permits_origin(&self, origin: &FileOrigin) -> bool {
        origin.host() == self.host && origin.port() == self.port
    }
    fn permits_address(&self, origin: &FileOrigin, address: SocketAddr) -> bool {
        self.permits_origin(origin)
            && address.port() == self.port
            && self.addresses.contains(&address.ip())
    }
}
