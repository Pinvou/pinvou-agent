//! Native authenticated local control client; DTOs are expectations only.
use super::{
    image::SealedImage,
    layout::{Layout, ProtectedDirectory, ordinal_equal},
    pipe::{Event, Pipe, nonce},
    process::RetainedProcess,
    registry::Store,
    scm,
};
use crate::{
    error::{Error, Result},
    handoff::Role,
    ipc, json,
    profile::Seed,
    registration::{Phase, PublicRegistration},
};
use serde::{Serialize, de::DeserializeOwned};

pub(super) struct Client {
    pub(super) seed: Seed,
    pub(super) layout: Layout,
    pub(super) record: PublicRegistration,
    pub(super) current: RetainedProcess,
    pub(super) server: RetainedProcess,
    pub(super) app: SealedImage,
    pub(super) launcher: SealedImage,
    pub(super) code: ProtectedDirectory,
    pub(super) app_root: ProtectedDirectory,
    _helper: SealedImage,
    pipe: Pipe,
    stop: Event,
    pub(super) session: String,
    pub(super) server_nonce: String,
    pub(super) instance: String,
    sequence: u64,
}
impl Client {
    pub(super) fn connect(role: Role) -> Result<Self> {
        if role == Role::SourceInstaller {
            return Err(Error::PeerRefused);
        }
        let seed = Seed::embedded()?;
        let layout = Layout::discover(&seed)?;
        let record = Store::new(&seed).read_public(&seed)?;
        if record.phase != Phase::Ready {
            return Err(Error::SetupIncomplete);
        }
        let sid = record
            .service_identity
            .service_sid
            .as_deref()
            .ok_or(Error::ServiceIdentityRefused)?;
        let current = RetainedProcess::current()?;
        if !current.token.interactive
            || current.token.user.session_id == 0
            || current.token.user.sid == "S-1-5-18"
            || current.token.app_container
            || current.token.network
            || current.token.integrity < 0x2000
        {
            return Err(Error::PeerRefused);
        }
        let expected = if role == Role::App {
            layout.app_path()
        } else {
            layout.launcher_path()
        };
        if !ordinal_equal(
            current.image.to_str().ok_or(Error::ImageRefused)?,
            expected.to_str().ok_or(Error::ImageRefused)?,
        )? {
            return Err(Error::PeerRefused);
        }
        let (code, app_root) = layout.client_roots(sid)?;
        let helper_root = code.child("helpers", false, false, Some(sid))?.child(
            &seed.build_id,
            false,
            false,
            Some(sid),
        )?;
        let helper = SealedImage::open(
            &helper_root,
            "pinvou-upgrade-helper.exe",
            &record.helper,
            &seed,
        )?;
        let app = SealedImage::open(&app_root, "pinvou3-tauri.exe", &record.app, &seed)?;
        let launcher = SealedImage::open(
            &code,
            "pinvou-upgrade-launcher.exe",
            &record.launcher,
            &seed,
        )?;
        let server = scm::live_process(&seed, &layout)?;
        server.verify_host_query_security()?;
        let pipe = Pipe::connect(&seed)?;
        if pipe.peer_process_id()? != server.selector.process_id || server.exited()? {
            return Err(Error::ServiceIdentityRefused);
        }
        let stop = Event::new()?;
        let challenge = nonce()?;
        let hello = ipc::Hello {
            host_ipc_version: 1,
            kind: "hello".into(),
            client_nonce: challenge.clone(),
            expected_registration_digest: record.digest()?,
            operation_id: None,
        };
        pipe.write_frame(&json::encode(&hello, true)?, &stop, Some(server.raw()))?;
        let bytes = pipe.read_frame(&stop, Some(server.raw()))?;
        let (session, reply) = ipc::response::<ipc::HelloReply>(&bytes, None, 0, "hello")?;
        if reply.profile_kind != seed.profile_kind
            || reply.profile_id != seed.profile_id
            || reply.installation_scope_id != record.installation_scope_id
            || reply.registration_revision != record.revision
            || reply.registration_digest != record.digest()?
            || reply.host_epoch != record.host_epoch
            || reply.client_nonce != challenge
            || !json::digest(&reply.server_nonce)
            || !json::digest(&reply.host_instance_nonce)
            || !reply.capabilities.is_exact(true, true)
        {
            return Err(Error::ServiceIdentityRefused);
        }
        let value = Self {
            seed,
            layout,
            record,
            current,
            server,
            app,
            launcher,
            code,
            app_root,
            _helper: helper,
            pipe,
            stop,
            session,
            server_nonce: reply.server_nonce,
            instance: reply.host_instance_nonce,
            sequence: 1,
        };
        value.recheck()?;
        Ok(value)
    }
    pub(super) fn recheck(&self) -> Result<()> {
        if self.server.exited()?
            || self.current.exited()?
            || self.pipe.peer_process_id()? != self.server.selector.process_id
        {
            return Err(Error::ServiceIdentityRefused);
        }
        let current = Store::new(&self.seed).read_public(&self.seed)?;
        if current.phase != Phase::Ready
            || current.host_epoch != self.record.host_epoch
            || current.installation_scope_id != self.record.installation_scope_id
            || current.genesis_seed_sha256 != self.record.genesis_seed_sha256
            || current.genesis_manifest_sha256 != self.record.genesis_manifest_sha256
            || current.helper != self.record.helper
            || current.app != self.record.app
            || current.launcher != self.record.launcher
        {
            return Err(Error::ServiceIdentityRefused);
        }
        let actual = scm::live_process(&self.seed, &self.layout)?;
        if actual.selector != self.server.selector {
            return Err(Error::ServiceIdentityRefused);
        }
        self.code.recheck()?;
        self.app_root.recheck()?;
        Ok(())
    }
    pub(super) fn request<B: Serialize, T: DeserializeOwned>(
        &mut self,
        kind: &str,
        body: &B,
    ) -> Result<T> {
        self.recheck()?;
        let sequence = self.sequence;
        self.sequence = sequence
            .checked_add(1)
            .filter(|n| *n <= json::SAFE_MAX)
            .ok_or(Error::IpcInvalid)?;
        let body = serde_json::to_value(body).map_err(|_| Error::IpcInvalid)?;
        let request = ipc::Request {
            host_ipc_version: 1,
            session_id: self.session.clone(),
            sequence,
            kind: kind.into(),
            body,
        };
        let encoded = json::encode(&request, true)?;
        // No retry after an attempted control send. The caller's action state,
        // rather than a transport reconnect, determines safe cancellation.
        self.pipe
            .write_frame(&encoded, &self.stop, Some(self.server.raw()))?;
        let bytes = self.pipe.read_frame(&self.stop, Some(self.server.raw()))?;
        let (_, reply) = ipc::response(&bytes, Some(&self.session), sequence, kind)?;
        self.recheck()?;
        Ok(reply)
    }
    pub(super) fn intent(&self) -> Result<String> {
        Ok(format!("{}-{}", self.server_nonce, &nonce()?[..32]))
    }
}

/// This path always uses the real registered App client and its authenticated
/// pipe/session sequence. Old helpers stay unsupported; no old-status fallback.
pub(crate) fn inspect_trusted_time() -> Result<crate::TrustedTimeStatus> {
    let result = (|| {
        let mut client = Client::connect(Role::App)?;
        let reply: crate::TrustedTimeStatus = client.request("time-status", &ipc::Empty {})?;
        reply.validate()?;
        Ok(reply)
    })();
    match result {
        Err(Error::Unsupported | Error::OperationUnsupported | Error::ProfileDisabled) => Ok(
            crate::TrustedTimeStatus::new(crate::TrustedTimeState::Unsupported, false),
        ),
        Err(Error::SetupIncomplete | Error::InitialTrustUnavailable) => Ok(
            crate::TrustedTimeStatus::new(crate::TrustedTimeState::SetupRequired, false),
        ),
        other => other,
    }
}
pub(crate) fn request_trusted_time_refresh() -> Result<crate::TrustedTimeRefreshReply> {
    let mut client = Client::connect(Role::App)?;
    let reply: crate::TrustedTimeRefreshReply =
        match client.request("request-time-refresh", &ipc::Empty {}) {
            Err(Error::OperationUnsupported) => return Err(Error::Unsupported),
            other => other?,
        };
    reply.validate()?;
    Ok(reply)
}
