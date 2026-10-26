//! Fixed-purpose restricted child custody. The pipe is transport only: its
//! server PID is the original creator and MUST NOT authenticate the keeper.
//! Actual held process/token/image plus a directly duplicated, one-use event
//! bind each acknowledgement. No acknowledgement is a durable writer grant.
mod actor;
mod objects;
mod wire;

use super::{handle::Handle, installed::StartedServiceContext, pipe::Pipe};
use crate::{
    error::{Error, Result},
    identity::ProcessSelector,
    profile::Seed,
};
use windows_sys::Win32::Foundation::HANDLE;

pub(super) use actor::run;
pub(super) use objects::{NativeChallengeEvent, PendingNativeChallenge};
pub(super) use wire::{
    KeeperCensusObservation, KeeperConnectionProof, KeeperJobAck, KeeperReadyAck,
    KeeperReleasedAck, acknowledge_initial_ready, acknowledge_query_copy, authenticate_connection,
    inspect_keeper_census, release_closed_query,
};

/// Native server custody, constructible only by the authenticated service.
/// The service owns this until spawn custody ends; it creates no keeper role.
pub(super) struct PreparedListener {
    pipe: Pipe,
    name: String,
}
/// An owned temporary duplication, independent of all host Rust borrows. Only
/// this one handle belongs in PROC_THREAD_ATTRIBUTE_HANDLE_LIST. Its lease must
/// outlive CreateProcessAsUserW, then be closed in the service.
pub(super) struct InheritedListenerLease {
    handle: Handle,
}
impl PreparedListener {
    pub(super) fn prepare(host: &StartedServiceContext) -> Result<Self> {
        let record = super::registry::Store::new(&host.context.seed)
            .read(&host.context.seed)?
            .ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&record)?;
        let pipe = Pipe::keeper_listener(&host.context.seed)?;
        let name = pipe_name(&host.context.seed);
        objects::inspect_listener(pipe.raw(), &name, false)?;
        Ok(Self { pipe, name })
    }
    pub(super) fn inheritance_lease(&self) -> Result<InheritedListenerLease> {
        objects::inspect_listener(self.pipe.raw(), &self.name, false)?;
        let handle = objects::duplicate_listener(self.pipe.raw())?;
        objects::inspect_listener(handle.raw(), &self.name, true)?;
        Ok(InheritedListenerLease { handle })
    }
}
impl InheritedListenerLease {
    pub(super) fn raw(&self) -> HANDLE {
        self.handle.raw()
    }
}
pub(super) fn pipe_name(seed: &Seed) -> String {
    format!("{}.WriterCustody", seed.pipe_name())
}
pub(super) fn create_listener(seed: &Seed) -> Result<Pipe> {
    let name = pipe_name(seed);
    let handle = objects::create_listener(&name)?;
    objects::inspect_listener(handle.raw(), &name, false)?;
    Ok(Pipe::keeper_from_owned_server(handle))
}

/// Native observations below are private and never deserializable. Getters are
/// DATA for the protected journal's own scope/epoch/selector CAS checks.
struct ObservedAck {
    keeper: ProcessSelector,
    challenge: String,
    registration_digest: String,
    installation_scope_id: String,
    host_epoch: u64,
}
