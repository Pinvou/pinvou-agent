//! Native keeper lifecycle owner. Only original registration initializes it.
//! A reservation or a running process is never a writer-retention receipt.
use super::super::{
    installed::StartedServiceContext,
    keeper_spawn::{
        DetachedStagingReceipt, KeeperChild, KeeperPeer, NativeKeeperExit, PreparedKeeperSpawn,
        RecoveredKeeper, RunningKeeper,
    },
    writer_keeper::KeeperReadyAck,
};
use super::*;
use crate::identity::ProcessSelector;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const NAMESPACE: &str = "writer-custody-v1";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Genesis {
    version: u8,
    scope: String,
    manifest: String,
    bootstrap: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Phase {
    Idle,
    Reserved,
    Suspended,
    Ready,
    Active,
}

/// Branch selection only; all effects still require native custody checks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::platform::windows) enum KeeperIntentPhase {
    Reserved,
    Suspended,
    Ready,
    Active,
}

/// Called inside the writer's existing TxR and native operation guard.
/// A durable Active record alone does not prove a live keeper or retention.
pub(super) fn require_active_record_in(
    private: &Key,
    actual: &HostRegistration,
    tx: Option<&Handle>,
) -> Result<()> {
    if load(private, actual, tx)?.0.phase != Phase::Active {
        return Err(Error::PrerequisiteUnavailable);
    }
    Ok(())
}

/// Does not reacquire BootstrapGuard: the admission token already owns it.
pub(super) fn require_active_peer(
    host: &StartedServiceContext,
    peer: &mut KeeperPeer<'_>,
) -> Result<()> {
    peer.recheck()?;
    let store = Store::new(&host.context.seed);
    let actual = store
        .read(&host.context.seed)?
        .ok_or(Error::SetupIncomplete)?;
    host.authenticate_running(&actual)?;
    let private = store.key(false, None)?.ok_or(Error::SetupIncomplete)?;
    let (head, _) = load(&private, &actual, None)?;
    let record = head.reservation.as_ref().ok_or(Error::ScopeConflict)?;
    if head.phase != Phase::Active
        || record.nonce != peer.reservation_nonce()
        || record.child.as_ref() != Some(peer.selector()?)
        || record.helper_sha256 != host.context.helper.sha256
    {
        return Err(Error::ScopeConflict);
    }
    peer.recheck()
}

pub(super) fn require_active_exit(
    host: &Arc<StartedServiceContext>,
    exited: &NativeKeeperExit<'_>,
) -> Result<()> {
    let selector = exited.keeper_selector()?;
    exited.require_current(host, exited.reservation_nonce(), selector)?;
    let store = Store::new(&host.context.seed);
    let actual = store
        .read(&host.context.seed)?
        .ok_or(Error::SetupIncomplete)?;
    host.authenticate_running(&actual)?;
    let private = store.key(false, None)?.ok_or(Error::SetupIncomplete)?;
    let (head, _) = load(&private, &actual, None)?;
    let record = head.reservation.as_ref().ok_or(Error::ScopeConflict)?;
    if head.phase != Phase::Active
        || record.nonce != exited.reservation_nonce()
        || record.child.as_ref() != Some(selector)
        || record.helper_sha256 != host.context.helper.sha256
    {
        return Err(Error::ScopeConflict);
    }
    exited.require_current(host, exited.reservation_nonce(), selector)
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReadyEvidence {
    challenge: String,
    registration_digest: String,
    host_epoch: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Reservation {
    first_revision: u64,
    nonce: String,
    parent: ProcessSelector,
    origin_epoch: u64,
    helper_sha256: String,
    child: Option<ProcessSelector>,
    ready: Option<ReadyEvidence>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Head {
    version: u8,
    genesis_sha256: String,
    revision: u64,
    previous_sha256: Option<String>,
    phase: Phase,
    reservation: Option<Reservation>,
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let raw = json::encode(value, false)?;
    if raw.len() > 16_384 {
        return Err(Error::StateCorrupt);
    }
    Ok(raw)
}
fn decode<T: serde::de::DeserializeOwned + Serialize>(raw: &[u8]) -> Result<T> {
    if raw.is_empty() || raw.len() > 16_384 {
        return Err(Error::StateCorrupt);
    }
    let value = json::decode(raw, false)?;
    if encode(&value)? != raw {
        return Err(Error::StateCorrupt);
    }
    Ok(value)
}
fn genesis(private: &Key, actual: &HostRegistration) -> Result<Vec<u8>> {
    let bootstrap = read(private, PREPARATION_BOOTSTRAP)?;
    pinvou_upgrade_ownership::InitialOwnership::parse(
        &bootstrap,
        &preparation::binding_for(actual)?,
    )
    .map_err(|_| Error::StateCorrupt)?;
    encode(&Genesis {
        version: 1,
        scope: actual.installation_scope_id.clone(),
        manifest: actual.genesis_manifest_sha256.clone(),
        bootstrap: json::sha256(&bootstrap),
    })
}
fn initial(genesis: &[u8]) -> Head {
    Head {
        version: 1,
        genesis_sha256: json::sha256(genesis),
        revision: 0,
        previous_sha256: None,
        phase: Phase::Idle,
        reservation: None,
    }
}
fn valid(head: &Head, genesis_raw: &[u8], actual: &HostRegistration) -> Result<()> {
    if head.version != 1
        || head.genesis_sha256 != json::sha256(genesis_raw)
        || head.revision > json::SAFE_MAX
        || (head.revision == 0) != head.previous_sha256.is_none()
        || head
            .previous_sha256
            .as_ref()
            .is_some_and(|s| !json::digest(s))
        || (head.phase == Phase::Idle) != head.reservation.is_none()
    {
        return Err(Error::StateCorrupt);
    }
    if let Some(r) = &head.reservation {
        let offset = match head.phase {
            Phase::Reserved => 0,
            Phase::Suspended => 1,
            Phase::Ready => 2,
            Phase::Active => 3,
            Phase::Idle => return Err(Error::StateCorrupt),
        };
        r.parent.validate().map_err(|_| Error::StateCorrupt)?;
        if r.first_revision == 0
            || r.first_revision.checked_add(offset) != Some(head.revision)
            || !json::digest(&r.nonce)
            || !json::digest(&r.helper_sha256)
            || r.origin_epoch == 0
            || r.origin_epoch > actual.host_epoch
            || (head.phase == Phase::Reserved) != r.child.is_none()
            || matches!(head.phase, Phase::Ready | Phase::Active) != r.ready.is_some()
        {
            return Err(Error::StateCorrupt);
        }
        if let Some(child) = &r.child {
            child.validate().map_err(|_| Error::StateCorrupt)?;
            if child == &r.parent {
                return Err(Error::StateCorrupt);
            }
        }
        if let Some(ready) = &r.ready {
            if !json::digest(&ready.challenge)
                || !json::digest(&ready.registration_digest)
                || ready.host_epoch != r.origin_epoch
            {
                return Err(Error::StateCorrupt);
            }
        }
    }
    Ok(())
}
fn successor(before: &Head, after: &Head, before_raw: &[u8]) -> Result<()> {
    if after.revision != before.revision.checked_add(1).ok_or(Error::StateCorrupt)?
        || after.previous_sha256 != Some(json::sha256(before_raw))
        || after.genesis_sha256 != before.genesis_sha256
    {
        return Err(Error::StateCorrupt);
    }
    match (before.phase, after.phase) {
        (Phase::Idle, Phase::Reserved) => {
            if after.reservation.as_ref().map(|r| r.first_revision) != Some(after.revision) {
                return Err(Error::StateCorrupt);
            }
        }
        (Phase::Active, Phase::Idle) => {
            if after.reservation.is_some() {
                return Err(Error::StateCorrupt);
            }
        }
        (Phase::Reserved, Phase::Suspended)
        | (Phase::Suspended, Phase::Ready)
        | (Phase::Ready, Phase::Active) => {
            let old = before.reservation.as_ref().ok_or(Error::StateCorrupt)?;
            let new = after.reservation.as_ref().ok_or(Error::StateCorrupt)?;
            if old.first_revision != new.first_revision
                || old.nonce != new.nonce
                || old.parent != new.parent
                || old.origin_epoch != new.origin_epoch
                || old.helper_sha256 != new.helper_sha256
                || (before.phase != Phase::Reserved && old.child != new.child)
                || (before.phase == Phase::Ready && old.ready != new.ready)
            {
                return Err(Error::StateCorrupt);
            }
        }
        _ => return Err(Error::StateCorrupt),
    }
    Ok(())
}
fn namespace(private: &Key, write_access: bool, tx: Option<&Handle>) -> Result<Key> {
    open(private.0, NAMESPACE, write_access, tx)?.ok_or(Error::SetupIncomplete)
}
fn load(private: &Key, actual: &HostRegistration, tx: Option<&Handle>) -> Result<(Head, Vec<u8>)> {
    let key = namespace(private, false, tx)?;
    let expected = genesis(private, actual)?;
    if read(&key, "Genesis")? != expected {
        return Err(Error::StateCorrupt);
    }
    let raw = read(&key, "Head")?;
    let head: Head = decode(&raw)?;
    valid(&head, &expected, actual)?;
    let history = open(key.0, "history", false, tx)?.ok_or(Error::StateCorrupt)?;
    let own =
        open(history.0, &revision_name(head.revision), false, tx)?.ok_or(Error::StateCorrupt)?;
    if read(&own, "Record")? != raw {
        return Err(Error::StateCorrupt);
    }
    if head.revision == 0 {
        if head != initial(&expected) {
            return Err(Error::StateCorrupt);
        }
    } else {
        let prior = open(history.0, &revision_name(head.revision - 1), false, tx)?
            .ok_or(Error::StateCorrupt)?;
        let prior_raw = read(&prior, "Record")?;
        let before: Head = decode(&prior_raw)?;
        valid(&before, &expected, actual)?;
        successor(&before, &head, &prior_raw)?;
    }
    Ok((head, raw))
}
pub(super) fn init_in(private: &Key, tx: &Handle, actual: &HostRegistration) -> Result<()> {
    let genesis_raw = genesis(private, actual)?;
    let raw = encode(&initial(&genesis_raw))?;
    let key = create(private.0, NAMESPACE, tx, false, true)?;
    let history = create(key.0, "history", tx, false, true)?;
    let first = create(history.0, &revision_name(0), tx, false, true)?;
    write(&key, "Genesis", &genesis_raw)?;
    write(&key, "Head", &raw)?;
    write(&first, "Record", &raw)?;
    if load(private, actual, Some(tx))?.1 != raw {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}
pub(super) fn verify_fresh_in(private: &Key, actual: &HostRegistration) -> Result<()> {
    let (head, _) = load(private, actual, None)?;
    if head.phase != Phase::Idle || head.revision != 0 {
        return Err(Error::StateCommitUnknown);
    }
    Ok(())
}

/// Native operation guard and exact protected readback, never deserialized.
pub(in crate::platform::windows) struct ProtectedKeeperIntent {
    _guard: BootstrapGuard,
    host: Arc<StartedServiceContext>,
    store: Store,
    head: Head,
    raw: Vec<u8>,
}

/// An exact selection from the native protected journal. This is DATA for a
/// native recovery adapter, not a process handle, liveness or a custody ACK.
pub(in crate::platform::windows) struct KeeperReference {
    host_scope: String,
    service_sid: String,
    raw: Vec<u8>,
    reservation: Reservation,
}
impl KeeperReference {
    pub(in crate::platform::windows) fn require_active(
        &self,
        host: &StartedServiceContext,
    ) -> Result<()> {
        self.recheck(host)?;
        let selected: Head = decode(&self.raw)?;
        if selected.phase != Phase::Active {
            return Err(Error::HandoffOutcomeUnknown);
        }
        Ok(())
    }
    pub(in crate::platform::windows) fn installation_scope_id(&self) -> &str {
        &self.host_scope
    }
    pub(in crate::platform::windows) fn service_sid(&self) -> &str {
        &self.service_sid
    }
    pub(in crate::platform::windows) fn reservation_nonce(&self) -> &str {
        &self.reservation.nonce
    }
    pub(in crate::platform::windows) fn creator_selector(&self) -> &ProcessSelector {
        &self.reservation.parent
    }
    pub(in crate::platform::windows) fn keeper_selector(&self) -> Result<&ProcessSelector> {
        self.reservation
            .child
            .as_ref()
            .ok_or(Error::HandoffOutcomeUnknown)
    }
    pub(in crate::platform::windows) fn helper_sha256(&self) -> &str {
        &self.reservation.helper_sha256
    }
    pub(in crate::platform::windows) fn origin_host_epoch(&self) -> u64 {
        self.reservation.origin_epoch
    }
    /// Does not reacquire BootstrapGuard: safe when a native caller owns it.
    pub(in crate::platform::windows) fn recheck(&self, host: &StartedServiceContext) -> Result<()> {
        let store = Store::new(&host.context.seed);
        let actual = store
            .read(&host.context.seed)?
            .ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&actual)?;
        let private = store.key(false, None)?.ok_or(Error::SetupIncomplete)?;
        if actual.installation_scope_id != self.host_scope
            || actual.service_identity.service_sid.as_deref() != Some(self.service_sid.as_str())
            || actual.host_epoch < self.reservation.origin_epoch
            || load(&private, &actual, None)?.1 != self.raw
        {
            return Err(Error::ScopeConflict);
        }
        Ok(())
    }
}
impl ProtectedKeeperIntent {
    pub(in crate::platform::windows) fn phase(&self) -> Result<KeeperIntentPhase> {
        self.current()?;
        match self.head.phase {
            Phase::Reserved => Ok(KeeperIntentPhase::Reserved),
            Phase::Suspended => Ok(KeeperIntentPhase::Suspended),
            Phase::Ready => Ok(KeeperIntentPhase::Ready),
            Phase::Active => Ok(KeeperIntentPhase::Active),
            Phase::Idle => Err(Error::ScopeConflict),
        }
    }
    fn current(&self) -> Result<HostRegistration> {
        let actual = self
            .store
            .read(&self.host.context.seed)?
            .ok_or(Error::SetupIncomplete)?;
        self.host.authenticate_running(&actual)?;
        let private = self.store.key(false, None)?.ok_or(Error::SetupIncomplete)?;
        if load(&private, &actual, None)?.1 != self.raw {
            return Err(Error::ScopeConflict);
        }
        Ok(actual)
    }
    fn require(&self, host: &Arc<StartedServiceContext>, nonce: &str) -> Result<&Reservation> {
        let actual = self.current()?;
        let record = self.head.reservation.as_ref().ok_or(Error::ScopeConflict)?;
        if !Arc::ptr_eq(host, &self.host)
            || record.nonce != nonce
            || record.origin_epoch > actual.host_epoch
            || record.helper_sha256 != self.host.context.helper.sha256
        {
            return Err(Error::ScopeConflict);
        }
        Ok(record)
    }
    pub(in crate::platform::windows) fn require_reserved(
        &self,
        host: &Arc<StartedServiceContext>,
        nonce: &str,
        helper_sha256: &str,
    ) -> Result<()> {
        let record = self.require(host, nonce)?;
        if self.head.phase != Phase::Reserved
            || record.helper_sha256 != helper_sha256
            || record.origin_epoch != self.host.startup.host_epoch
            || record.parent != self.host.context.process.selector
        {
            return Err(Error::ScopeConflict);
        }
        Ok(())
    }
    pub(in crate::platform::windows) fn require_suspended(
        &self,
        host: &Arc<StartedServiceContext>,
        nonce: &str,
        child: &ProcessSelector,
    ) -> Result<()> {
        let record = self.require(host, nonce)?;
        if self.head.phase != Phase::Suspended
            || record.child.as_ref() != Some(child)
            || record.origin_epoch != self.host.startup.host_epoch
            || record.parent != self.host.context.process.selector
        {
            return Err(Error::ScopeConflict);
        }
        Ok(())
    }
    pub(in crate::platform::windows) fn require_ready(
        &self,
        host: &Arc<StartedServiceContext>,
        nonce: &str,
        child: &ProcessSelector,
        ack: &KeeperReadyAck,
    ) -> Result<()> {
        let record = self.require(host, nonce)?;
        let actual = self.current()?;
        if self.head.phase != Phase::Ready
            || record.child.as_ref() != Some(child)
            || record.origin_epoch != actual.host_epoch
            || record.parent != self.host.context.process.selector
            || ack.keeper_selector() != child
            || ack.installation_scope_id() != actual.installation_scope_id
            || ack.host_epoch() != actual.host_epoch
            || ack.registration_digest() != actual.public().digest()?
            || record.ready.as_ref() != Some(&evidence(ack))
        {
            return Err(Error::ScopeConflict);
        }
        Ok(())
    }
    /// Retry of the same staging handoff after an uncertain Active commit.
    /// This permits no new readiness proof; native zero-limit receipt remains
    /// mandatory and must carry the original Ready challenge.
    pub(in crate::platform::windows) fn require_active_ready(
        &self,
        host: &Arc<StartedServiceContext>,
        nonce: &str,
        child: &ProcessSelector,
        ack: &KeeperReadyAck,
    ) -> Result<()> {
        self.require_active(host, nonce, child)?;
        let actual = self.current()?;
        let record = self.head.reservation.as_ref().ok_or(Error::ScopeConflict)?;
        if record.origin_epoch != actual.host_epoch
            || record.parent != self.host.context.process.selector
            || record.ready.as_ref() != Some(&evidence(ack))
            || ack.keeper_selector() != child
            || ack.installation_scope_id() != actual.installation_scope_id
            || ack.host_epoch() != actual.host_epoch
            || ack.registration_digest() != actual.public().digest()?
        {
            return Err(Error::ScopeConflict);
        }
        Ok(())
    }
    pub(in crate::platform::windows) fn record_child(
        &mut self,
        child: &mut KeeperChild,
    ) -> Result<()> {
        child.recheck()?;
        let record = self.require(&self.host, child.reservation_nonce())?;
        if self.head.phase == Phase::Suspended && record.child.as_ref() == Some(child.selector()?) {
            return self.require_suspended(
                &self.host,
                child.reservation_nonce(),
                child.selector()?,
            );
        }
        if self.head.phase != Phase::Reserved || record.child.is_some() {
            return Err(Error::ScopeConflict);
        }
        let mut next = self.head.clone();
        next.phase = Phase::Suspended;
        next.reservation.as_mut().ok_or(Error::StateCorrupt)?.child =
            Some(child.selector()?.clone());
        self.publish(next)
    }
    pub(in crate::platform::windows) fn record_ready(
        &mut self,
        keeper: &mut RunningKeeper,
        ack: &KeeperReadyAck,
    ) -> Result<()> {
        keeper.recheck()?;
        let record = self.require(&self.host, keeper.reservation_nonce())?;
        let actual = self.current()?;
        if self.head.phase == Phase::Ready {
            return self.require_ready(
                &self.host,
                keeper.reservation_nonce(),
                keeper.selector()?,
                ack,
            );
        }
        if self.head.phase != Phase::Suspended
            || record.child.as_ref() != Some(keeper.selector()?)
            || ack.keeper_selector() != keeper.selector()?
            || ack.installation_scope_id() != actual.installation_scope_id
            || ack.host_epoch() != actual.host_epoch
            || ack.registration_digest() != actual.public().digest()?
        {
            return Err(Error::ScopeConflict);
        }
        let mut next = self.head.clone();
        next.phase = Phase::Ready;
        next.reservation.as_mut().ok_or(Error::StateCorrupt)?.ready = Some(evidence(ack));
        self.publish(next)
    }
    /// Consumes the native staging-limit proof; a persisted Ready record or a
    /// serialized ACK cannot authorize activation by itself.
    pub(in crate::platform::windows) fn record_active(
        &mut self,
        mut receipt: DetachedStagingReceipt<'_>,
        ack: &KeeperReadyAck,
    ) -> Result<()> {
        let record = self.head.reservation.as_ref().ok_or(Error::ScopeConflict)?;
        let selector = record.child.as_ref().ok_or(Error::ScopeConflict)?;
        if self.head.phase == Phase::Active {
            self.require_active_ready(&self.host, &record.nonce, selector, ack)?;
            return receipt.require_current(&self.host, &record.nonce, selector, ack);
        }
        self.require_ready(&self.host, &record.nonce, selector, ack)?;
        receipt.require_current(&self.host, &record.nonce, selector, ack)?;
        let mut next = self.head.clone();
        next.phase = Phase::Active;
        self.publish(next)?;
        receipt
            .recheck_limits0()
            .map_err(|_| Error::HandoffOutcomeUnknown)
    }
    pub(in crate::platform::windows) fn require_active(
        &self,
        host: &Arc<StartedServiceContext>,
        nonce: &str,
        child: &ProcessSelector,
    ) -> Result<()> {
        let record = self.require(host, nonce)?;
        if self.head.phase != Phase::Active || record.child.as_ref() != Some(child) {
            return Err(Error::ScopeConflict);
        }
        Ok(())
    }
    #[allow(unsafe_code)]
    fn publish(&mut self, mut next: Head) -> Result<()> {
        self.current()?;
        next.revision = self
            .head
            .revision
            .checked_add(1)
            .filter(|n| *n <= json::SAFE_MAX)
            .ok_or(Error::Unavailable)?;
        next.previous_sha256 = Some(json::sha256(&self.raw));
        successor(&self.head, &next, &self.raw)?;
        let wanted = encode(&next)?;
        let tx = Handle::owned(unsafe {
            CreateTransaction(ptr::null_mut(), ptr::null_mut(), 0, 0, 0, 0, ptr::null())
        })?;
        {
            let actual = self
                .store
                .read_in(&self.host.context.seed, Some(&tx))?
                .ok_or(Error::SetupIncomplete)?;
            self.host.authenticate_running(&actual)?;
            let private = self
                .store
                .key(true, Some(&tx))?
                .ok_or(Error::SetupIncomplete)?;
            if load(&private, &actual, Some(&tx))?.1 != self.raw {
                return Err(Error::ScopeConflict);
            }
            if matches!(next.phase, Phase::Idle | Phase::Reserved | Phase::Ready)
                && !writers::custody_empty_in(&private, &actual, &tx)?
            {
                // A fresh keeper cannot account for existing or unproven
                // writer custody. Keep that occupancy and use recovery.
                return Err(Error::PrerequisiteUnavailable);
            }
            let key = namespace(&private, true, Some(&tx))?;
            let history = open(key.0, "history", true, Some(&tx))?.ok_or(Error::StateCorrupt)?;
            let revision = create(history.0, &revision_name(next.revision), &tx, false, true)?;
            write(&revision, "Record", &wanted)?;
            write(&key, "Head", &wanted)?;
            if load(&private, &actual, Some(&tx))?.1 != wanted {
                return Err(Error::StateCorrupt);
            }
        }
        if unsafe { CommitTransaction(tx.raw()) } == 0 {
            return Err(Error::StateCommitUnknown);
        }
        drop(tx);
        let confirm = || -> Result<()> {
            let actual = self
                .store
                .read(&self.host.context.seed)?
                .ok_or(Error::StateCommitUnknown)?;
            self.host.authenticate_running(&actual)?;
            let private = self
                .store
                .key(false, None)?
                .ok_or(Error::StateCommitUnknown)?;
            let key = namespace(&private, false, None)?;
            let revision = open(
                key.0,
                &format!("history\\{}", revision_name(next.revision)),
                false,
                None,
            )?
            .ok_or(Error::StateCommitUnknown)?;
            for handle in [private.0, key.0, revision.0] {
                if unsafe { RegFlushKey(handle) } != 0 {
                    return Err(Error::StateCommitUnknown);
                }
            }
            if load(&private, &actual, None)?.1 != wanted || read(&revision, "Record")? != wanted {
                return Err(Error::StateCommitUnknown);
            }
            Ok(())
        };
        confirm().map_err(|_| Error::StateCommitUnknown)?;
        self.head = next;
        self.raw = wanted;
        Ok(())
    }
}
fn evidence(ack: &KeeperReadyAck) -> ReadyEvidence {
    ReadyEvidence {
        challenge: ack.challenge().to_owned(),
        registration_digest: ack.registration_digest().to_owned(),
        host_epoch: ack.host_epoch(),
    }
}
impl Store {
    /// Only a continuously held, previously authenticated original process can
    /// prove exit. Missing names/PIDs, lost connections or an empty actor map
    /// never authorize clearing this slot. Original preparation stays intact.
    pub(in crate::platform::windows) fn retire_exited_keeper(
        host: Arc<StartedServiceContext>,
        exited: &NativeKeeperExit<'_>,
    ) -> Result<()> {
        let mut intent = read_intent(host)?;
        let selector = exited.keeper_selector()?;
        if intent.head.phase == Phase::Idle {
            // The prior Active history must be this exact exited original,
            // never merely any empty current slot after a failed publication.
            let actual = intent.current()?;
            let private = intent
                .store
                .key(false, None)?
                .ok_or(Error::SetupIncomplete)?;
            let key = namespace(&private, false, None)?;
            let previous_revision = intent
                .head
                .revision
                .checked_sub(1)
                .ok_or(Error::ScopeConflict)?;
            let previous = open(
                key.0,
                &format!("history\\{}", revision_name(previous_revision)),
                false,
                None,
            )?
            .ok_or(Error::StateCorrupt)?;
            let before: Head = decode(&read(&previous, "Record")?)?;
            let reservation = before.reservation.as_ref().ok_or(Error::ScopeConflict)?;
            if before.phase != Phase::Active
                || reservation.nonce != exited.reservation_nonce()
                || reservation.child.as_ref() != Some(selector)
            {
                return Err(Error::ScopeConflict);
            }
            // load() above authenticates the exact previous hash/successor.
            // Check the complete writer set again in one native read-only TxR.
            #[allow(unsafe_code)]
            let tx = Handle::owned(unsafe {
                CreateTransaction(ptr::null_mut(), ptr::null_mut(), 0, 0, 0, 0, ptr::null())
            })?;
            if !writers::custody_empty_in(&private, &actual, &tx)?
                || load(&private, &actual, Some(&tx))?.1 != intent.raw
            {
                return Err(Error::ScopeConflict);
            }
            exited.require_current(&intent.host, exited.reservation_nonce(), selector)?;
            for handle in [private.0, key.0, previous.0] {
                #[allow(unsafe_code)]
                if unsafe { RegFlushKey(handle) } != 0 {
                    return Err(Error::StateCommitUnknown);
                }
            }
            intent.current()?;
            return exited.require_current(&intent.host, exited.reservation_nonce(), selector);
        }
        intent.require_active(&intent.host, exited.reservation_nonce(), selector)?;
        exited.require_current(&intent.host, exited.reservation_nonce(), selector)?;
        let mut next = intent.head.clone();
        next.phase = Phase::Idle;
        next.reservation = None;
        intent.publish(next)?;
        exited
            .require_current(&intent.host, exited.reservation_nonce(), selector)
            .map_err(|_| Error::HandoffOutcomeUnknown)
    }
    pub(in crate::platform::windows) fn recover_keeper_custody(
        host: Arc<StartedServiceContext>,
        keeper: &mut RecoveredKeeper,
    ) -> Result<ProtectedKeeperIntent> {
        keeper.recheck()?;
        let intent = retained_intent(host, keeper.reservation_nonce())?;
        intent.require_active(&intent.host, keeper.reservation_nonce(), keeper.selector()?)?;
        Ok(intent)
    }
    pub(in crate::platform::windows) fn resume_keeper_reservation(
        host: Arc<StartedServiceContext>,
        prepared: &PreparedKeeperSpawn,
    ) -> Result<ProtectedKeeperIntent> {
        let intent = retained_intent(host, prepared.reservation_nonce())?;
        intent.require_reserved(
            &intent.host,
            prepared.reservation_nonce(),
            prepared.helper_sha256(),
        )?;
        if prepared.parent_selector() != &intent.host.context.process.selector {
            return Err(Error::ScopeConflict);
        }
        Ok(intent)
    }
    pub(in crate::platform::windows) fn resume_or_begin_keeper_reservation(
        host: Arc<StartedServiceContext>,
        prepared: &PreparedKeeperSpawn,
    ) -> Result<ProtectedKeeperIntent> {
        Self::begin_keeper(host, prepared)
    }
    pub(in crate::platform::windows) fn resume_keeper_child(
        host: Arc<StartedServiceContext>,
        child: &mut KeeperChild,
    ) -> Result<ProtectedKeeperIntent> {
        child.recheck()?;
        let selector = child.selector()?;
        let intent = retained_intent(host, child.reservation_nonce())?;
        let record = intent
            .head
            .reservation
            .as_ref()
            .ok_or(Error::ScopeConflict)?;
        if !matches!(intent.head.phase, Phase::Reserved | Phase::Suspended)
            || (intent.head.phase == Phase::Suspended && record.child.as_ref() != Some(selector))
        {
            return Err(Error::ScopeConflict);
        }
        Ok(intent)
    }
    pub(in crate::platform::windows) fn keeper_reference(
        host: &StartedServiceContext,
    ) -> Result<Option<KeeperReference>> {
        let _guard = BootstrapGuard::acquire(&host.context.seed, false)?;
        let store = Store::new(&host.context.seed);
        let actual = store
            .read(&host.context.seed)?
            .ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&actual)?;
        let private = store.key(false, None)?.ok_or(Error::SetupIncomplete)?;
        let (head, raw) = load(&private, &actual, None)?;
        let Some(reservation) = head.reservation else {
            return Ok(None);
        };
        Ok(Some(KeeperReference {
            host_scope: actual.installation_scope_id,
            service_sid: actual
                .service_identity
                .service_sid
                .ok_or(Error::ServiceIdentityRefused)?,
            raw,
            reservation,
        }))
    }
    /// Resume uncertain publication only from the SAME retained native keeper.
    /// A new name, process selector or replacement reservation is forbidden.
    pub(in crate::platform::windows) fn resume_keeper_custody(
        host: Arc<StartedServiceContext>,
        keeper: &mut RunningKeeper,
    ) -> Result<ProtectedKeeperIntent> {
        keeper.recheck()?;
        let guard = BootstrapGuard::acquire(&host.context.seed, false)?;
        let store = Store::new(&host.context.seed);
        let actual = store
            .read(&host.context.seed)?
            .ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&actual)?;
        let private = store.key(false, None)?.ok_or(Error::SetupIncomplete)?;
        let (head, raw) = load(&private, &actual, None)?;
        let record = head.reservation.as_ref().ok_or(Error::ScopeConflict)?;
        if record.nonce != keeper.reservation_nonce()
            || record.child.as_ref() != Some(keeper.selector()?)
            || record.origin_epoch != actual.host_epoch
            || record.parent != host.context.process.selector
            || record.helper_sha256 != host.context.helper.sha256
        {
            return Err(Error::ScopeConflict);
        }
        Ok(ProtectedKeeperIntent {
            _guard: guard,
            host,
            store,
            head,
            raw,
        })
    }
    pub(in crate::platform::windows) fn begin_keeper(
        host: Arc<StartedServiceContext>,
        prepared: &PreparedKeeperSpawn,
    ) -> Result<ProtectedKeeperIntent> {
        let guard = BootstrapGuard::acquire(&host.context.seed, false)?;
        let store = Store::new(&host.context.seed);
        let actual = store
            .read(&host.context.seed)?
            .ok_or(Error::SetupIncomplete)?;
        host.authenticate_running(&actual)?;
        let private = store.key(false, None)?.ok_or(Error::SetupIncomplete)?;
        let (head, raw) = load(&private, &actual, None)?;
        if prepared.parent_selector() != &host.context.process.selector
            || prepared.helper_sha256() != host.context.helper.sha256
            || !json::digest(prepared.reservation_nonce())
        {
            return Err(Error::ScopeConflict);
        }
        let mut intent = ProtectedKeeperIntent {
            _guard: guard,
            host,
            store,
            head,
            raw,
        };
        if intent.head.phase == Phase::Reserved {
            intent.require_reserved(
                &intent.host,
                prepared.reservation_nonce(),
                prepared.helper_sha256(),
            )?;
            return Ok(intent);
        }
        if intent.head.phase != Phase::Idle {
            return Err(Error::ScopeConflict);
        }
        let mut next = intent.head.clone();
        next.phase = Phase::Reserved;
        next.reservation = Some(Reservation {
            first_revision: intent
                .head
                .revision
                .checked_add(1)
                .ok_or(Error::Unavailable)?,
            nonce: prepared.reservation_nonce().to_owned(),
            parent: prepared.parent_selector().clone(),
            origin_epoch: actual.host_epoch,
            helper_sha256: prepared.helper_sha256().to_owned(),
            child: None,
            ready: None,
        });
        intent.publish(next)?;
        Ok(intent)
    }
}

fn retained_intent(host: Arc<StartedServiceContext>, nonce: &str) -> Result<ProtectedKeeperIntent> {
    let intent = read_intent(host)?;
    intent.require(&intent.host, nonce)?;
    Ok(intent)
}

fn read_intent(host: Arc<StartedServiceContext>) -> Result<ProtectedKeeperIntent> {
    let guard = BootstrapGuard::acquire(&host.context.seed, false)?;
    let store = Store::new(&host.context.seed);
    let actual = store
        .read(&host.context.seed)?
        .ok_or(Error::SetupIncomplete)?;
    host.authenticate_running(&actual)?;
    let private = store.key(false, None)?.ok_or(Error::SetupIncomplete)?;
    let (head, raw) = load(&private, &actual, None)?;
    let intent = ProtectedKeeperIntent {
        _guard: guard,
        host,
        store,
        head,
        raw,
    };
    Ok(intent)
}
