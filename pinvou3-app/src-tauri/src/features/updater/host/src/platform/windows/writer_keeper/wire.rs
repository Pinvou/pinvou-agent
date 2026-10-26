//! Closed private protocol. Serializable messages are DATA only. ACK constructors
//! additionally require the native one-use event signal and held process/job
//! rechecks; no DTO or caller-selected number becomes a native capability.
use super::super::{
    keeper_spawn::{KeeperPeer, PendingQueryCopy, RunningKeeper},
    pipe::Event,
    registry::{
        Store,
        keeper::ProtectedKeeperIntent,
        writers::{ClosedWriterRead, ProtectedAdmissionIntent},
    },
};
use super::*;
use crate::{
    json,
    registration::{Phase, PublicRegistration},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Request {
    pub(super) keeper_wire_version: u8,
    pub(super) challenge: String,
    pub(super) challenge_serial: u64,
    pub(super) event: String,
    pub(super) registration_digest: String,
    pub(super) action: Action,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum Action {
    Authenticate {},
    Ready {},
    Retain {
        job_name: String,
        job_binding: String,
        handle: String,
    },
    Census {},
    Release {
        job_name: String,
        job_binding: String,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Binding {
    pub(super) job_name: String,
    pub(super) job_binding: String,
    pub(super) active_processes: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Reply {
    pub(super) keeper_wire_version: u8,
    pub(super) challenge: String,
    pub(super) request_sha256: String,
    pub(super) keeper: ProcessSelector,
    pub(super) retained: Vec<Binding>,
    pub(super) native_complete: bool,
}
impl Request {
    pub(super) fn validate(&self) -> Result<()> {
        if self.keeper_wire_version != 1
            || !json::digest(&self.challenge)
            || self.challenge_serial == 0
            || self.challenge_serial > json::SAFE_MAX
            || !json::digest(&self.registration_digest)
        {
            return Err(Error::IpcInvalid);
        }
        self.event_address()?;
        match &self.action {
            Action::Retain {
                job_name,
                job_binding,
                handle,
            } => {
                validate_binding(job_name, job_binding)?;
                address(handle)?;
            }
            Action::Release {
                job_name,
                job_binding,
            } => {
                validate_binding(job_name, job_binding)?;
            }
            Action::Authenticate { .. } | Action::Ready { .. } | Action::Census { .. } => {}
        }
        Ok(())
    }
    pub(super) fn event_address(&self) -> Result<HANDLE> {
        address(&self.event)
    }
}
fn validate_binding(name: &str, binding: &str) -> Result<()> {
    if !objects::valid_job_name(name) || !json::digest(binding) {
        return Err(Error::IpcInvalid);
    }
    Ok(())
}
pub(super) fn address(text: &str) -> Result<HANDLE> {
    if text.len() != 16
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::IpcInvalid);
    }
    let value = u64::from_str_radix(text, 16).map_err(|_| Error::IpcInvalid)?;
    let value = usize::try_from(value).map_err(|_| Error::IpcInvalid)?;
    // Real handles are nonzero, aligned values, never pseudo handles.
    if value == 0 || value % 4 != 0 || value >= isize::MAX as usize {
        return Err(Error::IpcInvalid);
    }
    Ok(value as HANDLE)
}
fn current(host: &StartedServiceContext) -> Result<PublicRegistration> {
    let store = Store::new(&host.context.seed);
    let private = store
        .read(&host.context.seed)?
        .ok_or(Error::SetupIncomplete)?;
    host.authenticate_running(&private)?;
    let record = store.read_public(&host.context.seed)?;
    if record.phase != Phase::Ready || record.digest()? != private.public().digest()? {
        return Err(Error::ServiceIdentityRefused);
    }
    Ok(record)
}
fn exchange(
    host: &StartedServiceContext,
    event: &PendingNativeChallenge,
    action: Action,
    stop: &Event,
) -> Result<(ObservedAck, Reply)> {
    let record = current(host)?;
    if event.host_epoch() != record.host_epoch {
        return Err(Error::ScopeConflict);
    }
    let request = Request {
        keeper_wire_version: 1,
        challenge: event.nonce().to_owned(),
        challenge_serial: event.serial(),
        event: format!("{:016x}", event.remote()),
        registration_digest: record.digest()?,
        action,
    };
    request.validate()?;
    let bytes = json::encode(&request, true)?;
    let pipe = Pipe::keeper_connect(&host.context.seed)?;
    // GetNamedPipeServerProcessId reports the SERVICE CREATOR on this inherited
    // endpoint. It is deliberately not consulted as a keeper authentication.
    pipe.write_frame(&bytes, stop, None)?;
    let reply: Reply = json::decode(&pipe.read_frame(stop, None)?, true)?;
    if reply.keeper_wire_version != 1
        || reply.challenge != request.challenge
        || reply.request_sha256 != json::sha256(&bytes)
        || reply.keeper != *event.keeper()
        || reply.retained.len() > 128
    {
        return Err(Error::PeerRefused);
    }
    reply.keeper.validate()?;
    let mut names = BTreeSet::new();
    for item in &reply.retained {
        validate_binding(&item.job_name, &item.job_binding)?;
        if !names.insert(item.job_name.clone()) {
            return Err(Error::IpcInvalid);
        }
    }
    if current(host)?.digest()? != record.digest()? {
        return Err(Error::ServiceIdentityRefused);
    }
    event.observed_signal()?;
    Ok((
        ObservedAck {
            keeper: event.keeper().clone(),
            challenge: request.challenge,
            registration_digest: record.digest()?,
            installation_scope_id: record.installation_scope_id,
            host_epoch: record.host_epoch,
        },
        reply,
    ))
}

#[must_use = "authenticated connection is not empty custody, durable ready or writer grant"]
pub(in crate::platform::windows) struct KeeperConnectionProof {
    observed: ObservedAck,
}
#[must_use = "commit/read back protected Ready and detach staging before writer admission"]
pub(in crate::platform::windows) struct KeeperReadyAck {
    observed: ObservedAck,
}
#[must_use = "commit/read back exact protected writer retention before any writer grant"]
pub(in crate::platform::windows) struct KeeperJobAck {
    observed: ObservedAck,
    job_name: String,
    job_binding: String,
}
#[must_use = "only exact retired kernel custody; never saved data or released preparation"]
pub(in crate::platform::windows) struct KeeperReleasedAck {
    observed: ObservedAck,
    job_name: String,
    job_binding: String,
}
/// A complete bounded native table/map observation. Counts are DATA, not a
/// NativeRecoveredJobZero, writer-close token, saved-data proof or exit grant.
pub(in crate::platform::windows) struct KeeperCensusObservation {
    observed: ObservedAck,
    jobs: Vec<RetainedJobObservation>,
}
pub(in crate::platform::windows) struct RetainedJobObservation {
    job_name: String,
    job_binding: String,
    active_processes: u32,
}
macro_rules! observed_getters {
    ($name:ident) => {
        impl $name {
            pub(in crate::platform::windows) fn keeper_selector(&self) -> &ProcessSelector {
                &self.observed.keeper
            }
            pub(in crate::platform::windows) fn challenge(&self) -> &str {
                &self.observed.challenge
            }
            pub(in crate::platform::windows) fn registration_digest(&self) -> &str {
                &self.observed.registration_digest
            }
            pub(in crate::platform::windows) fn installation_scope_id(&self) -> &str {
                &self.observed.installation_scope_id
            }
            pub(in crate::platform::windows) fn host_epoch(&self) -> u64 {
                self.observed.host_epoch
            }
        }
    };
}
observed_getters!(KeeperConnectionProof);
observed_getters!(KeeperReadyAck);
observed_getters!(KeeperJobAck);
observed_getters!(KeeperReleasedAck);
observed_getters!(KeeperCensusObservation);
impl KeeperJobAck {
    pub(in crate::platform::windows) fn job_name(&self) -> &str {
        &self.job_name
    }
    pub(in crate::platform::windows) fn job_binding(&self) -> &str {
        &self.job_binding
    }
}
impl KeeperReleasedAck {
    pub(in crate::platform::windows) fn job_name(&self) -> &str {
        &self.job_name
    }
    pub(in crate::platform::windows) fn job_binding(&self) -> &str {
        &self.job_binding
    }
}
impl KeeperCensusObservation {
    pub(in crate::platform::windows) fn jobs(&self) -> &[RetainedJobObservation] {
        &self.jobs
    }
}
impl RetainedJobObservation {
    pub(in crate::platform::windows) fn job_name(&self) -> &str {
        &self.job_name
    }
    pub(in crate::platform::windows) fn job_binding(&self) -> &str {
        &self.job_binding
    }
    pub(in crate::platform::windows) fn active_processes(&self) -> u32 {
        self.active_processes
    }
}
pub(in crate::platform::windows) fn authenticate_connection(
    host: &StartedServiceContext,
    keeper: &mut KeeperPeer<'_>,
    stop: &Event,
) -> Result<KeeperConnectionProof> {
    keeper.recheck()?;
    let event = keeper.bind_challenge(NativeChallengeEvent::new(host)?)?;
    let (observed, _) = exchange(host, &event, Action::Authenticate {}, stop)?;
    keeper.recheck()?;
    Ok(KeeperConnectionProof { observed })
}
pub(in crate::platform::windows) fn acknowledge_initial_ready(
    host: &std::sync::Arc<StartedServiceContext>,
    intent: &ProtectedKeeperIntent,
    keeper: &mut RunningKeeper,
    stop: &Event,
) -> Result<KeeperReadyAck> {
    keeper.recheck()?;
    intent.require_suspended(host, keeper.reservation_nonce(), keeper.selector()?)?;
    let event = keeper.bind_challenge(NativeChallengeEvent::new(host)?)?;
    let (observed, reply) = exchange(host, &event, Action::Ready {}, stop)?;
    if !reply.native_complete || !reply.retained.is_empty() {
        return Err(Error::Referenced);
    }
    keeper.recheck()?;
    intent.require_suspended(host, keeper.reservation_nonce(), keeper.selector()?)?;
    Ok(KeeperReadyAck { observed })
}

fn binding(intent: &ProtectedAdmissionIntent<'_>) -> Result<String> {
    intent.identity_bytes().map(|bytes| json::sha256(&bytes))
}
pub(in crate::platform::windows) fn acknowledge_query_copy(
    host: &StartedServiceContext,
    intent: &ProtectedAdmissionIntent<'_>,
    pending: &mut PendingQueryCopy<'_>,
    stop: &Event,
) -> Result<KeeperJobAck> {
    intent.recheck(host)?;
    pending.recheck()?;
    objects::inspect_job(
        pending.local_query_for_wire().raw(),
        intent.job_name(),
        intent.service_sid(),
    )?;
    let job_binding = binding(intent)?;
    let event = pending.bind_challenge(NativeChallengeEvent::new(host)?)?;
    let action = Action::Retain {
        job_name: intent.job_name().to_owned(),
        job_binding: job_binding.clone(),
        handle: format!("{:016x}", pending.remote_handle_value()),
    };
    let (observed, reply) = exchange(host, &event, action, stop)?;
    if !reply
        .retained
        .iter()
        .any(|b| b.job_name == intent.job_name() && b.job_binding == job_binding)
    {
        return Err(Error::HandoffOutcomeUnknown);
    }
    // Exact duplicate-back comparison rechecks THAT remote handle after the
    // actor has adopted and signalled. Native job name/rights/security also stay
    // checked against this protected intent, not the received census fields.
    pending.recheck()?;
    intent.recheck(host)?;
    objects::inspect_job(
        pending.local_query_for_wire().raw(),
        intent.job_name(),
        intent.service_sid(),
    )?;
    if pending.keeper_selector()? != &observed.keeper {
        return Err(Error::ServiceIdentityRefused);
    }
    Ok(KeeperJobAck {
        observed,
        job_name: intent.job_name().to_owned(),
        job_binding,
    })
}

/// The actual ClosedWriterRead retains the native operation guard throughout
/// this request. Rechecking it does not reacquire BootstrapGuard. No caller name,
/// serialized receipt or absent process can construct the retirement request.
pub(in crate::platform::windows) fn release_closed_query(
    host: &StartedServiceContext,
    closed: &ClosedWriterRead,
    keeper: &mut KeeperPeer<'_>,
    stop: &Event,
) -> Result<KeeperReleasedAck> {
    closed.recheck(host)?;
    keeper.recheck()?;
    let job_name = closed.job_name().to_owned();
    let job_binding = json::sha256(&closed.identity_bytes()?);
    let event = keeper.bind_challenge(NativeChallengeEvent::new(host)?)?;
    let (observed, reply) = exchange(
        host,
        &event,
        Action::Release {
            job_name: job_name.clone(),
            job_binding: job_binding.clone(),
        },
        stop,
    )?;
    if !reply.native_complete || reply.retained.iter().any(|b| b.job_name == job_name) {
        return Err(Error::HandoffOutcomeUnknown);
    }
    closed.recheck(host)?;
    keeper.recheck()?;
    Ok(KeeperReleasedAck {
        observed,
        job_name,
        job_binding,
    })
}

/// Caller retains the actual native admission/keeper operation guard across
/// this observation; it is deliberately not an empty/exit grant. Native unknown
/// handles and unsupported/over-capacity snapshots produce Unknown, never an
/// empty result. Recovery connections do not require an empty actor map.
pub(in crate::platform::windows) fn inspect_keeper_census(
    host: &StartedServiceContext,
    keeper: &mut KeeperPeer<'_>,
    stop: &Event,
) -> Result<KeeperCensusObservation> {
    keeper.recheck()?;
    let event = keeper.bind_challenge(NativeChallengeEvent::new(host)?)?;
    let (observed, reply) = exchange(host, &event, Action::Census {}, stop)?;
    if !reply.native_complete {
        return Err(Error::HandoffOutcomeUnknown);
    }
    keeper.recheck()?;
    let jobs = reply
        .retained
        .into_iter()
        .map(|job| RetainedJobObservation {
            job_name: job.job_name,
            job_binding: job.job_binding,
            active_processes: job.active_processes,
        })
        .collect();
    Ok(KeeperCensusObservation { observed, jobs })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_parsing_never_turns_addresses_or_json_into_native_ack() {
        for invalid in [
            "0",
            "0000000000000000",
            "ffffffffffffffff",
            "0000000000000001",
            "000000000000000A",
            "00000000000000040",
        ] {
            assert!(address(invalid).is_err());
        }
        assert!(address("0000000000000040").is_ok());
        assert!(
            json::decode::<Request>(br#"{"keeperWireVersion":1,"keeperWireVersion":1}"#, true)
                .is_err()
        );
        assert!(json::decode::<Action>(br#"{"kind":"execute","command":"x"}"#, true).is_err());
        assert!(json::decode::<Action>(br#"{"kind":"authenticate","handle":"40"}"#, true).is_err());
    }
}
