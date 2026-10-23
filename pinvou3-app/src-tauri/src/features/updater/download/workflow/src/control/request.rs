//! Original request checks before the fixed control POST; no caller authority.
use super::ControlAttempt;
use crate::{Error, Import, MAX_SAFE, manifest::Manifest};
use pinvou_upgrade_codec::{ClientRequestOperation, Document, InputKind};
use pinvou_upgrade_verifier::{VerificationInputs, verify_with_supplied_root};
use std::time::Instant;

pub(super) struct PreparedControl {
    request: String,
    supplied_time_millis: u64,
    started: Instant,
}
impl PreparedControl {
    pub(super) fn new(manifest: &Manifest, input: ControlAttempt) -> Result<Self, Error> {
        let prepared = Self {
            request: input.request,
            supplied_time_millis: input.supplied_time_millis,
            started: Instant::now(),
        };
        prepared.validate(manifest, prepared.supplied_time_millis)?;
        Ok(prepared)
    }
    pub(super) fn request(&self) -> &str {
        &self.request
    }
    fn validate(&self, manifest: &Manifest, now_ms: u64) -> Result<(), Error> {
        if now_ms < manifest.initial_supplied_time_millis || now_ms > MAX_SAFE {
            return Err(Error::BindingMismatch);
        }
        let request = Document::parse(self.request.as_bytes(), InputKind::Metadata)
            .map_err(|_| Error::InvalidInput)?;
        request
            .assert_supplied_client_request_contract(ClientRequestOperation::DownloadInfo)
            .map_err(|_| Error::InvalidInput)?;
        if crate::manifest::qualification(&request)? != manifest.qualification_sha256 {
            return Err(Error::BindingMismatch);
        }
        // Re-read the original pinned Root/Package relation before disclosing
        // the request to the fixed service. No incoming Root can replace it.
        let target = manifest.target()?;
        if request.value()["packageId"] != target.id {
            return Err(Error::BindingMismatch);
        }
        let root = Document::parse(manifest.root.as_bytes(), InputKind::Metadata)
            .map_err(|_| Error::InvalidInput)?
            .root_policy_shape()
            .map_err(|_| Error::InvalidInput)?;
        let scope = &request.value()["scope"];
        let expected = serde_json::json!({
            "role":"decision", "product":scope["product"], "component":scope["component"],
            "scope":{"installationScopeId":scope["installationScopeId"],
                "channel":scope["channel"], "channelRevision":scope["channelRevision"],
                "targetKey":scope["targetKey"]}
        });
        let expected = Document::parse(&crate::json(&expected)?, InputKind::Metadata)
            .map_err(|_| Error::InvalidInput)?;
        let now = Document::parse(now_ms.to_string().as_bytes(), InputKind::Metadata)
            .map_err(|_| Error::InvalidInput)?
            .protocol_time_shape()
            .map_err(|_| Error::InvalidInput)?;
        let decision = Document::parse(
            &crate::json(&request.value()["decision"])?,
            InputKind::Metadata,
        )
        .map_err(|_| Error::InvalidInput)?;
        verify_with_supplied_root(
            &decision
                .canonical_bytes()
                .map_err(|_| Error::InvalidInput)?,
            VerificationInputs::new(&root, &expected, &now),
        )
        .map_err(|_| Error::SignatureRefused)?;
        Ok(())
    }
    pub(super) fn finish(
        &self,
        manifest: &Manifest,
        status: u16,
        response: Vec<u8>,
    ) -> Result<Import, Error> {
        // Status errors are never treated as a Download credential or retried.
        if status != 200 {
            return Err(Error::ControlRefused);
        }
        let elapsed =
            u64::try_from(self.started.elapsed().as_millis()).map_err(|_| Error::InvalidInput)?;
        let now = self
            .supplied_time_millis
            .checked_add(elapsed)
            .filter(|value| *value <= MAX_SAFE)
            .ok_or(Error::InvalidInput)?;
        self.finish_at(manifest, response, now)
    }
    fn finish_at(&self, manifest: &Manifest, response: Vec<u8>, now: u64) -> Result<Import, Error> {
        self.validate(manifest, now)?;
        // Do not deserialize the body before the original duplicate-key-aware
        // codec/verifier. It validates the real response and both credentials.
        let input = Import {
            request: self.request.clone(),
            response: String::from_utf8(response).map_err(|_| Error::InvalidInput)?,
            supplied_time_millis: now,
        };
        manifest.imported_url(&input)?;
        Ok(input)
    }
    #[cfg(test)]
    pub(super) fn finish_for_test(
        &self,
        manifest: &Manifest,
        response: Vec<u8>,
        now: u64,
    ) -> Result<Import, Error> {
        self.finish_at(manifest, response, now)
    }
}
