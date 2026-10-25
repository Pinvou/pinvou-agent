//! Fixed native instance/worker ownership. No detached per-request tasks.
use super::{
    dispatch::Core,
    installed::StartedServiceContext,
    peer::Peer,
    pipe::{Event, Pipe, nonce},
};
use crate::{
    error::{Error, Result},
    handoff::Role,
    ipc,
    registration::Phase,
};
use std::{sync::Arc, thread::JoinHandle, time::Duration};

const CONNECTIONS: usize = 64;
struct Workers {
    stop: Arc<Event>,
    threads: Vec<JoinHandle<()>>,
}
impl Drop for Workers {
    fn drop(&mut self) {
        let _ = self.stop.set();
        // Never join while holding the finite-domain mutex. Each worker drops
        // its own Connection (and original native leases) before returning.
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}
pub(super) fn serve(host: StartedServiceContext, stop: Arc<Event>) -> Result<()> {
    let core = Arc::new(Core::new(host));
    let mut time_worker = super::time_service::TimeWorker::start(core.clone(), stop.clone())?;
    // Keep all instances alive through reuse: the service never closes the
    // last listening name between clients, allowing a name-squatting gap.
    let mut pipes = Vec::with_capacity(CONNECTIONS);
    for index in 0..CONNECTIONS {
        pipes.push(Pipe::listener(&core.host.context.seed, index == 0)?);
    }
    let mut workers = Workers {
        stop: stop.clone(),
        threads: Vec::with_capacity(CONNECTIONS),
    };
    for pipe in pipes {
        let core = core.clone();
        let stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name("upgrade-local-peer".into())
            .spawn(move || {
                while matches!(stop.is_set(), Ok(false)) {
                    if pipe.accept(&stop).is_ok() {
                        let _ = session(&core, &pipe, &stop);
                    }
                    pipe.disconnect();
                }
            })
            .map_err(|_| Error::Unavailable)?;
        workers.threads.push(thread);
    }
    while !stop.is_set()? {
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(workers);
    time_worker.stop_and_join()
}
fn session(core: &Arc<Core>, pipe: &Pipe, stop: &Event) -> Result<()> {
    let mut authenticated = false;
    let result = (|| {
        let bytes = pipe.read_frame(stop, None)?;
        let hello = ipc::Hello::parse(&bytes)?;
        let record = core.record()?;
        let peer = Peer::authenticate(pipe, &core.host, &record)?;
        let bootstrap = hello.kind == "bootstrap-hello";
        if hello.expected_registration_digest != record.public().digest()? {
            return Err(Error::ScopeConflict);
        }
        if bootstrap {
            if peer.role != Role::SourceInstaller
                || record.phase != Phase::ServiceConfigured
                || hello.operation_id.as_deref()
                    != Some(record.installer_identity.operation_id.as_str())
            {
                return Err(Error::SetupIncomplete);
            }
        } else if record.phase != Phase::Ready {
            return Err(Error::SetupIncomplete);
        }
        let session = nonce()?;
        let server_nonce = nonce()?;
        let connection = core.connect(
            peer,
            session.clone(),
            server_nonce.clone(),
            bootstrap,
            hello.operation_id.clone(),
        )?;
        let startup = record
            .startup_attestation
            .as_ref()
            .ok_or(Error::SetupIncomplete)?;
        let seed = &core.host.context.seed;
        let reply = if bootstrap {
            ipc::success(
                &session,
                0,
                "bootstrap-hello",
                ipc::BootstrapHelloReply {
                    profile_kind: seed.profile_kind.clone(),
                    profile_id: seed.profile_id.clone(),
                    installation_scope_id: record.installation_scope_id.clone(),
                    operation_id: record.installer_identity.operation_id.clone(),
                    phase: record.phase,
                    registration_revision: record.revision,
                    registration_digest: record.public().digest()?,
                    host_epoch: record.host_epoch,
                    host_instance_nonce: startup.instance_nonce.clone(),
                    client_nonce: hello.client_nonce,
                    server_nonce,
                    capabilities: ipc::Capabilities::setup(),
                },
            )?
        } else {
            ipc::success(
                &session,
                0,
                "hello",
                ipc::HelloReply {
                    profile_kind: seed.profile_kind.clone(),
                    profile_id: seed.profile_id.clone(),
                    installation_scope_id: record.installation_scope_id.clone(),
                    registration_revision: record.revision,
                    registration_digest: record.public().digest()?,
                    host_epoch: record.host_epoch,
                    host_instance_nonce: startup.instance_nonce.clone(),
                    client_nonce: hello.client_nonce,
                    server_nonce,
                    capabilities: ipc::Capabilities::ready(
                        connection.peer.role != Role::SourceInstaller,
                    ),
                },
            )?
        };
        connection.authorize(pipe)?;
        pipe.write_frame(&reply, stop, Some(connection.peer.process.raw()))?;
        authenticated = true;
        let mut sequencer = ipc::Sequencer::new(session.clone())?;
        loop {
            let bytes = pipe.read_frame(stop, Some(connection.peer.process.raw()))?;
            let request = ipc::Request::parse(&bytes)?;
            let record = connection.authorize(pipe)?;
            let reply = match sequencer.check(&request) {
                Ok(ipc::Dispatch::Replay(cached)) => connection
                    .replay(&request, &record, cached)
                    .or_else(|error| {
                    ipc::failure(Some(&session), Some(request.sequence), error)
                })?,
                Ok(ipc::Dispatch::New) => {
                    let (reply, grant) = match connection.dispatch(&request, &record) {
                        Ok(reply) => reply,
                        Err(error) => (
                            ipc::failure(Some(&session), Some(request.sequence), error)?,
                            false,
                        ),
                    };
                    // Consumption is committed before attempting a send. A
                    // lost send never rebuilds/reissues a sensitive grant.
                    sequencer.completed(&request, reply.clone(), grant)?;
                    reply
                }
                Err(error) => {
                    let reply = ipc::failure(Some(&session), Some(request.sequence), error)?;
                    pipe.write_frame(&reply, stop, Some(connection.peer.process.raw()))?;
                    return Err(error);
                }
            };
            pipe.write_frame(&reply, stop, Some(connection.peer.process.raw()))?;
        }
    })();
    // Before authenticated hello there is no peer identity to project. Do not
    // serialize process/path/source details into error responses or logs.
    if let Err(error) = result {
        if !authenticated {
            let _ = pipe.write_frame(&ipc::failure(None, None, error)?, stop, None);
        }
    }
    result
}
