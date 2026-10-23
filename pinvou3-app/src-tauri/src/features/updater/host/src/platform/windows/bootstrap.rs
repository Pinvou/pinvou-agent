//! Read-only original installer probe. Ready remains a native source CAS.
use super::{
    pipe::{Event, Pipe, nonce},
    process::RetainedProcess,
    registry::Store,
    scm,
    source::SourceInstallerContext as Source,
};
use crate::{
    error::{Error, Result},
    ipc, json,
    registration::{HostRegistration, Phase},
};

struct Session {
    pipe: Pipe,
    stop: Event,
    server: RetainedProcess,
    id: String,
}
impl Session {
    fn open(source: &Source, record: &HostRegistration) -> Result<Self> {
        source.recheck()?;
        let server = scm::live_process(&source.seed, &source.layout)?;
        server.verify_host_query_security()?;
        let startup = record
            .startup_attestation
            .as_ref()
            .ok_or(Error::SetupIncomplete)?;
        if startup.process_id != server.selector.process_id
            || startup.creation_file_time != server.selector.creation_file_time
        {
            return Err(Error::ServiceIdentityRefused);
        }
        let current = Store::new(&source.seed).read_public(&source.seed)?;
        if current.digest()? != record.public().digest()? {
            return Err(Error::ScopeConflict);
        }
        let bootstrap = record.phase == Phase::ServiceConfigured;
        if !bootstrap && record.phase != Phase::Ready {
            return Err(Error::SetupIncomplete);
        }
        let pipe = Pipe::connect(&source.seed)?;
        if pipe.peer_process_id()? != server.selector.process_id || server.exited()? {
            return Err(Error::ServiceIdentityRefused);
        }
        let stop = Event::new()?;
        let challenge = nonce()?;
        let kind = if bootstrap {
            "bootstrap-hello"
        } else {
            "hello"
        };
        let hello = ipc::Hello {
            host_ipc_version: 1,
            kind: kind.into(),
            client_nonce: challenge.clone(),
            expected_registration_digest: current.digest()?,
            operation_id: if bootstrap {
                Some(record.installer_identity.operation_id.clone())
            } else {
                None
            },
        };
        pipe.write_frame(&json::encode(&hello, true)?, &stop, Some(server.raw()))?;
        let bytes = pipe.read_frame(&stop, Some(server.raw()))?;
        let id = if bootstrap {
            let (id, reply) = ipc::response::<ipc::BootstrapHelloReply>(&bytes, None, 0, kind)?;
            if reply.profile_kind != source.seed.profile_kind
                || reply.profile_id != source.seed.profile_id
                || reply.installation_scope_id != record.installation_scope_id
                || reply.operation_id != record.installer_identity.operation_id
                || reply.phase != record.phase
                || reply.registration_revision != record.revision
                || reply.registration_digest != current.digest()?
                || reply.host_epoch != record.host_epoch
                || reply.host_instance_nonce != startup.instance_nonce
                || reply.client_nonce != challenge
                || !json::digest(&reply.server_nonce)
                || !reply.capabilities.is_exact(false, false)
            {
                return Err(Error::ServiceIdentityRefused);
            }
            id
        } else {
            let (id, reply) = ipc::response::<ipc::HelloReply>(&bytes, None, 0, kind)?;
            if reply.profile_kind != source.seed.profile_kind
                || reply.profile_id != source.seed.profile_id
                || reply.installation_scope_id != record.installation_scope_id
                || reply.registration_revision != record.revision
                || reply.registration_digest != current.digest()?
                || reply.host_epoch != record.host_epoch
                || reply.host_instance_nonce != startup.instance_nonce
                || reply.client_nonce != challenge
                || !json::digest(&reply.server_nonce)
                || !reply.capabilities.is_exact(true, false)
            {
                return Err(Error::ServiceIdentityRefused);
            }
            id
        };
        let value = Self {
            pipe,
            stop,
            server,
            id,
        };
        value.recheck(source, record)?;
        Ok(value)
    }
    fn recheck(&self, source: &Source, record: &HostRegistration) -> Result<()> {
        source.recheck()?;
        if self.server.exited()?
            || self.pipe.peer_process_id()? != self.server.selector.process_id
            || scm::live_process(&source.seed, &source.layout)?.selector != self.server.selector
            || Store::new(&source.seed)
                .read(&source.seed)?
                .ok_or(Error::StateCorrupt)?
                .digest()?
                != record.digest()?
        {
            return Err(Error::ScopeConflict);
        }
        Ok(())
    }
}
pub(super) fn probe(source: &Source, record: &HostRegistration) -> Result<()> {
    let session = Session::open(source, record)?;
    let startup = record
        .startup_attestation
        .as_ref()
        .ok_or(Error::SetupIncomplete)?;
    let challenge = nonce()?;
    let body = ipc::ProbeBody {
        operation_id: record.installer_identity.operation_id.clone(),
        expected_revision: record.revision,
        challenge: challenge.clone(),
    };
    let request = ipc::Request {
        host_ipc_version: 1,
        session_id: session.id.clone(),
        sequence: 1,
        kind: "bootstrap-probe".into(),
        body: serde_json::to_value(body).map_err(|_| Error::IpcInvalid)?,
    };
    session.pipe.write_frame(
        &json::encode(&request, true)?,
        &session.stop,
        Some(session.server.raw()),
    )?;
    let bytes = session
        .pipe
        .read_frame(&session.stop, Some(session.server.raw()))?;
    let (_, reply) =
        ipc::response::<ipc::ProbeReply>(&bytes, Some(&session.id), 1, "bootstrap-probe")?;
    if reply.operation_id != record.installer_identity.operation_id
        || reply.installation_scope_id != record.installation_scope_id
        || reply.phase != Phase::ServiceConfigured
        || reply.registration_revision != record.revision
        || reply.host_epoch != record.host_epoch
        || reply.instance_nonce != startup.instance_nonce
        || reply.process_id != session.server.selector.process_id
        || reply.creation_file_time != session.server.selector.creation_file_time
        || reply.helper_image_sha256 != record.helper.sha256
        || reply.manifest_sha256 != record.genesis_manifest_sha256
        || reply.challenge != challenge
        || reply.probe_state != "checked"
    {
        return Err(Error::ServiceIdentityRefused);
    }
    session.recheck(source, record)
}
pub(super) fn ready_status(source: &Source, record: &HostRegistration) -> Result<()> {
    if record.phase != Phase::Ready {
        return Err(Error::SetupIncomplete);
    }
    let session = Session::open(source, record)?;
    let request = ipc::Request {
        host_ipc_version: 1,
        session_id: session.id.clone(),
        sequence: 1,
        kind: "status".into(),
        body: serde_json::json!({}),
    };
    session.pipe.write_frame(
        &json::encode(&request, true)?,
        &session.stop,
        Some(session.server.raw()),
    )?;
    let bytes = session
        .pipe
        .read_frame(&session.stop, Some(session.server.raw()))?;
    let (_, reply) = ipc::response::<ipc::StatusReply>(&bytes, Some(&session.id), 1, "status")?;
    if reply.phase != Phase::Ready
        || reply.helper_build_id != record.helper.build_id
        || reply.launcher_build_id != record.launcher.build_id
        || reply.app_build_id != record.app.build_id
        || reply.host_epoch != record.host_epoch
        || reply.registration_revision != record.revision
        || !reply.capabilities.is_exact(true, false)
    {
        return Err(Error::ServiceIdentityRefused);
    }
    session.recheck(source, record)
}
