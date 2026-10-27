//! Native protected writer admission owner. Nothing here grants a freeze,
//! proves a save, deletes history, or initializes a missing owner on load.
mod record;
mod storage;
use super::super::{
    installed::StartedServiceContext,
    keeper_spawn::{KeeperPeer, NativeKeeperExit},
    peer::Peer,
    pipe::Pipe,
    process::RetainedProcess,
    writer_job::{
        NativeJobZero, NativePendingJobZero, NativeRecoveredJobZero, NativeUnassignedWriterExit,
        PendingWriterJob, ReservedWriterJob, WriterJob,
    },
    writer_keeper::{KeeperJobAck, KeeperReleasedAck},
};
use super::*;
use std::sync::Arc;
use windows_sys::Win32::Foundation::CompareObjectHandles;

pub(super) const NAMESPACE: &str = "writer-admissions-v1";
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::platform::windows) enum WriterJobIntentPhase {
    Reserved,
    Admitted,
    Draining,
    Closed,
}

/// One native operation serializer, exact protected readback and optional live
/// authenticated original. No Clone/Serialize/Deserialize or external factory.
pub(in crate::platform::windows) struct ProtectedAdmissionIntent<'host> {
    _guard: BootstrapGuard,
    store: Store,
    host: &'host StartedServiceContext,
    service_epoch: u64,
    head_raw: Vec<u8>,
    row: record::Row,
    row_raw: Vec<u8>,
    actual_original: Option<Arc<RetainedProcess>>,
}
/// Borrowing this keeps the native operation guard alive until grant send.
/// This is an admission publication, never preparation/freeze authority.
pub(in crate::platform::windows) struct PublishedAdmission<'token, 'host> {
    intent: &'token ProtectedAdmissionIntent<'host>,
}
/// Native-census selection only. The caller cannot construct a PID/name selector.
pub(in crate::platform::windows) struct WriterReference {
    scope: String,
    genesis: String,
    entry: record::Entry,
}
pub(in crate::platform::windows) struct WriterCensus {
    references: Vec<WriterReference>,
    participants: Vec<pinvou_upgrade_ownership::live::Participant>,
}
/// Exact durable retirement readback. It permits retiring service custody only;
/// it contains no saved-data, root-fence, preparation release or freeze grant.
pub(in crate::platform::windows) struct ClosedWriterRead {
    _guard: BootstrapGuard,
    row: record::Row,
    raw: Vec<u8>,
}
impl ClosedWriterRead {
    pub(in crate::platform::windows) fn protected_record_bytes(&self) -> &[u8] {
        &self.raw
    }
    pub(in crate::platform::windows) fn job_name(&self) -> &str {
        &self.row.identity.job_name
    }
    pub(in crate::platform::windows) fn identity_bytes(&self) -> Result<Vec<u8>> {
        record::encode(&self.row.identity)
    }
    /// The token owns the native operation guard through the keeper release.
    /// Do not reacquire it while verifying the immutable terminal record.
    pub(in crate::platform::windows) fn recheck(&self, host: &StartedServiceContext) -> Result<()> {
        let store = Store::new(&host.context.seed);
        let actual = running(&store, host)?;
        let private = store.key(false, None)?.ok_or(Error::SetupIncomplete)?;
        let reference = WriterReference {
            scope: self.row.identity.scope.clone(),
            genesis: self.row.identity.genesis.clone(),
            entry: self.row.entry(&self.raw),
        };
        if storage::confirm_closed(&private, &actual, &reference)?.1 != self.raw {
            return Err(Error::ScopeConflict);
        }
        Ok(())
    }
}
impl WriterCensus {
    pub(in crate::platform::windows) fn into_references(self) -> Vec<WriterReference> {
        self.references
    }
    pub(in crate::platform::windows) fn references(&self) -> &[WriterReference] {
        &self.references
    }
    pub(in crate::platform::windows) fn participants(
        &self,
    ) -> &[pinvou_upgrade_ownership::live::Participant] {
        &self.participants
    }
}

/// ONLY original fresh Store::commit(None) may call this. Caller composes live
/// preparation genesis in the same original transaction, then verifies both.
pub(super) fn init_in(private: &Key, tx: &Handle, host: &HostRegistration) -> Result<()> {
    storage::init_in(private, tx, host)
}
/// Presence and exact complete bounded current census; no initialization.
pub(super) fn verify_in(private: &Key, host: &HostRegistration, tx: Option<&Handle>) -> Result<()> {
    storage::load(private, host, tx).map(|_| ())
}
pub(super) fn verify_fresh_in(private: &Key, host: &HostRegistration) -> Result<()> {
    let loaded = storage::load(private, host, None)?;
    if loaded.head.revision != 1 || !loaded.active.is_empty() || !loaded.pending.is_empty() {
        return Err(Error::StateCommitUnknown);
    }
    Ok(())
}
pub(super) fn custody_empty_in(
    private: &Key,
    host: &HostRegistration,
    tx: &Handle,
) -> Result<bool> {
    let loaded = storage::load(private, host, Some(tx))?;
    Ok(loaded.active.is_empty() && loaded.pending.is_empty())
}
/// For root's native preparation transaction. DATA is derived from this exact
/// protected read set, not an occupied flag or caller participant list.
pub(super) fn census_in(
    private: &Key,
    host: &HostRegistration,
    tx: &Handle,
) -> Result<WriterCensus> {
    census(storage::load(private, host, Some(tx))?)
}
fn census(loaded: storage::Loaded) -> Result<WriterCensus> {
    let mut entries = Vec::with_capacity(loaded.active.len());
    for (row, raw) in loaded.active {
        let participant = row.identity.participant(&row.id)?;
        let reference = WriterReference {
            scope: loaded.genesis.scope.clone(),
            genesis: loaded.genesis.genesis.clone(),
            entry: row.entry(&raw),
        };
        entries.push((reference, participant));
    }
    // Public IDs are domain hashes, independent of private row-key ordering.
    // Keep reference/participant pairs together through ordering so callers
    // cannot silently zip a differently ordered native selection and inventory.
    entries.sort_by(|a, b| a.1.id.cmp(&b.1.id));
    if entries.windows(2).any(|pair| pair[0].1.id == pair[1].1.id) {
        return Err(Error::StateCorrupt);
    }
    let (references, participants) = entries.into_iter().unzip();
    Ok(WriterCensus {
        references,
        participants,
    })
}
fn running(store: &Store, host: &StartedServiceContext) -> Result<HostRegistration> {
    let actual = store
        .read(&host.context.seed)?
        .ok_or(Error::SetupIncomplete)?;
    host.authenticate_running(&actual)?;
    Ok(actual)
}
fn current(store: &Store, host: &StartedServiceContext) -> Result<storage::Loaded> {
    let actual = running(store, host)?;
    let private = store.key(false, None)?.ok_or(Error::SetupIncomplete)?;
    storage::load(&private, &actual, None)
}

/// Actual read-only same-transaction census. No missing namespace is converted
/// to an empty journal, and no query creates a keeper or performs recovery.
#[allow(unsafe_code)]
pub(in crate::platform::windows) fn inspect_journal(
    host: &StartedServiceContext,
) -> Result<crate::WriterJournalStatus> {
    let store = Store::new(&host.context.seed);
    let _guard = BootstrapGuard::acquire(&host.context.seed, false)?;
    let tx = Handle::owned(unsafe {
        CreateTransaction(ptr::null_mut(), ptr::null_mut(), 0, 0, 0, 0, ptr::null())
    })?;
    let actual = store
        .read_in(&host.context.seed, Some(&tx))?
        .ok_or(Error::SetupIncomplete)?;
    host.authenticate_running(&actual)?;
    let private = store.key(false, Some(&tx))?.ok_or(Error::SetupIncomplete)?;
    let loaded = storage::load(&private, &actual, Some(&tx))?;
    let counts = journal_counts(&loaded)?;
    let status = crate::WriterJournalStatus {
        protocol_version: 1,
        observation: crate::WriterStatusObservation::JournalOnly,
        installation_scope_id: actual.installation_scope_id.clone(),
        host_epoch: actual.host_epoch,
        journal_revision: loaded.head.revision,
        counts,
    };
    status.validate().map_err(|_| Error::StateCorrupt)?;
    host.authenticate_running(&actual)?;
    // Dropping the read-only transaction abandons it without any publication.
    Ok(status)
}

fn journal_counts(loaded: &storage::Loaded) -> Result<crate::WriterJournalCounts> {
    if loaded
        .active
        .len()
        .checked_add(loaded.pending.len())
        .is_none_or(|n| n > pinvou_upgrade_ownership::live::MAX_PARTICIPANTS)
        || loaded
            .pending
            .iter()
            .any(|(row, _)| row.phase != record::Phase::Closed)
    {
        return Err(Error::StateCorrupt);
    }
    let mut counts = crate::WriterJournalCounts {
        reserved: 0,
        admitted: 0,
        draining: 0,
        pending_release: u32::try_from(loaded.pending.len()).map_err(|_| Error::StateCorrupt)?,
    };
    for (row, _) in &loaded.active {
        match row.phase {
            record::Phase::Reserved => counts.reserved += 1,
            record::Phase::Admitted => counts.admitted += 1,
            record::Phase::Draining => counts.draining += 1,
            record::Phase::Closed => return Err(Error::StateCorrupt),
        }
    }
    Ok(counts)
}
#[allow(unsafe_code)]
fn original_identity(
    host: &StartedServiceContext,
    reservation: &ReservedWriterJob,
    peer: &Peer,
    pipe: &Pipe,
) -> Result<(String, record::Identity)> {
    peer.recheck(pipe)?;
    if !peer.role.is_writer()
        || reservation.origin_host_epoch() != host.startup.host_epoch
        || unsafe { CompareObjectHandles(reservation.original().raw(), peer.process.raw()) } == 0
        || reservation.original().exited()?
    {
        return Err(Error::PeerRefused);
    }
    retained_reservation_identity(host, reservation, &peer.process)
}
fn retained_reservation_identity(
    host: &StartedServiceContext,
    reservation: &ReservedWriterJob,
    original: &RetainedProcess,
) -> Result<(String, record::Identity)> {
    let actual = running(&Store::new(&host.context.seed), host)?;
    let name = reservation.protected_name();
    let id = name
        .strip_prefix("Global\\PinvouWriter-")
        .filter(|v| json::digest(v))
        .ok_or(Error::StateCorrupt)?;
    let cli_path = host.context.layout.app.join("pinvou.exe");
    let cli_image_sha256 = if super::super::layout::ordinal_equal(
        original.image.to_str().ok_or(Error::ImageRefused)?,
        cli_path.to_str().ok_or(Error::ImageRefused)?,
    )? {
        Some(
            host.context
                .manifest
                .app_files
                .iter()
                .find(|entry| entry.relative_name == "pinvou.exe")
                .ok_or(Error::ImageRefused)?
                .sha256
                .clone(),
        )
    } else {
        None
    };
    Ok((
        id.to_owned(),
        record::Identity {
            scope: actual.installation_scope_id,
            genesis: actual.genesis_manifest_sha256,
            service_sid: actual
                .service_identity
                .service_sid
                .ok_or(Error::ServiceIdentityRefused)?,
            origin_host_epoch: reservation.origin_host_epoch(),
            job_name: name.to_owned(),
            original: original.selector.clone(),
            user_sid: original.token.user.sid.clone(),
            logon: original.token.user.logon,
            session_id: original.token.user.session_id,
            app_image_sha256: host.context.app.sha256.clone(),
            cli_image_sha256,
            integrity: original.token.integrity,
            elevated: original.token.elevated,
            administrator: original.token.administrator,
            interactive: original.token.interactive,
            network: original.token.network,
            app_container: original.token.app_container,
        },
    ))
}
/// A read-only native transaction is needed even on resume: no active
/// preparation may already have closed writer admission. Never assess a budget
/// separately and publish later.
#[allow(unsafe_code)]
fn admission_open(store: &Store, host: &StartedServiceContext, expected: &[u8]) -> Result<()> {
    let tx = Handle::owned(unsafe {
        CreateTransaction(ptr::null_mut(), ptr::null_mut(), 0, 0, 0, 0, ptr::null())
    })?;
    let actual = store
        .read_in(&host.context.seed, Some(&tx))?
        .ok_or(Error::SetupIncomplete)?;
    host.authenticate_running(&actual)?;
    let private = store.key(false, Some(&tx))?.ok_or(Error::SetupIncomplete)?;
    if storage::load(&private, &actual, Some(&tx))?.head_raw != expected {
        return Err(Error::ScopeConflict);
    }
    if preparation::current_in(&private, &actual, Some(&tx))?.is_some() {
        return Err(Error::Busy);
    }
    keeper::require_active_record_in(&private, &actual, Some(&tx))?;
    // Read-only TxR is abandoned by close after all borrowed keys drop.
    Ok(())
}
fn reopen_from_custody<'h>(
    host: &'h StartedServiceContext,
    name: &str,
    original: Arc<RetainedProcess>,
    origin_epoch: u64,
) -> Result<ProtectedAdmissionIntent<'h>> {
    // This helper is private; only actual native custody wrappers below supply
    // these facts. There is no raw-name/PID public admission or recovery path.
    let guard = BootstrapGuard::acquire(&host.context.seed, false)?;
    let store = Store::new(&host.context.seed);
    let loaded = current(&store, host)?;
    if origin_epoch != host.startup.host_epoch {
        return Err(Error::ScopeConflict);
    }
    let (row, raw) = loaded
        .active
        .iter()
        .find(|(r, _)| r.identity.job_name == name)
        .ok_or(Error::ScopeConflict)?;
    let identity = &row.identity;
    let token = &original.token;
    if identity.origin_host_epoch != origin_epoch
        || identity.original != original.selector
        || identity.user_sid != token.user.sid
        || identity.logon != token.user.logon
        || identity.session_id != token.user.session_id
        || identity.integrity != token.integrity
        || identity.elevated != token.elevated
        || identity.administrator != token.administrator
        || identity.interactive != token.interactive
        || identity.network != token.network
        || identity.app_container != token.app_container
    {
        return Err(Error::ScopeConflict);
    }
    Ok(ProtectedAdmissionIntent {
        _guard: guard,
        store,
        host,
        service_epoch: host.startup.host_epoch,
        head_raw: loaded.head_raw.clone(),
        row: row.clone(),
        row_raw: raw.clone(),
        actual_original: Some(original),
    })
}

impl Store {
    /// Retire only an actual still-unassigned reservation whose continuously
    /// held original has exited. No job name is opened, adopted or released;
    /// even a foreign object at that name is outside this proof's authority.
    /// Exact native absence is distinct from a failed read. A committed row is
    /// closed/released with history intact and preparation obligations intact.
    pub(in crate::platform::windows) fn retire_unassigned_writer(
        host: &StartedServiceContext,
        proof: &NativeUnassignedWriterExit<'_>,
    ) -> Result<()> {
        let _guard = BootstrapGuard::acquire(&host.context.seed, false)?;
        let reservation = proof.recheck(host)?;
        let store = Store::new(&host.context.seed);
        let (id, identity) =
            retained_reservation_identity(host, reservation, reservation.original())?;
        let mut loaded = current(&store, host)?;
        let actual = running(&store, host)?;
        let private = store.key(false, None)?.ok_or(Error::SetupIncomplete)?;
        let selected = storage::lookup_row(&private, &actual, &id)?;
        let Some((mut row, mut raw)) = selected else {
            if loaded
                .active
                .iter()
                .chain(loaded.pending.iter())
                .any(|(r, _)| r.id == id || r.identity.original == identity.original)
            {
                return Err(Error::ScopeConflict);
            }
            // No native assignment and no protected row: only this in-memory
            // reservation is retired. Generic read errors never reach here.
            proof.recheck(host)?;
            return Ok(());
        };
        if row.identity != identity {
            return Err(Error::ScopeConflict);
        }
        if row.phase == record::Phase::Reserved {
            proof.recheck(host)?;
            (loaded, row, raw) = storage::commit(
                &store,
                host,
                &loaded.head_raw,
                Some(&raw),
                storage::Change::Close,
            )?;
        }
        // Only Reserved -> Closed (revision 2) can belong to this typed
        // pre-assignment owner. Never retire an admitted/draining incarnation.
        let first = record::Row::reserved(id, identity);
        let first_hash = json::sha256(&record::encode(&first)?);
        if row.phase != record::Phase::Closed
            || row.revision != 2
            || row.first_record_sha256.as_deref() != Some(first_hash.as_str())
            || row.previous_sha256.as_deref() != Some(first_hash.as_str())
        {
            return Err(Error::ScopeConflict);
        }
        let entry = row.entry(&raw);
        if loaded
            .pending
            .iter()
            .any(|(r, bytes)| r.entry(bytes) == entry)
        {
            proof.recheck(host)?;
            storage::commit(
                &store,
                host,
                &loaded.head_raw,
                Some(&raw),
                storage::Change::Release,
            )?;
        } else if loaded
            .active
            .iter()
            .chain(loaded.pending.iter())
            .any(|(r, _)| r.id == row.id)
        {
            return Err(Error::ScopeConflict);
        }
        let reference = WriterReference {
            scope: row.identity.scope.clone(),
            genesis: row.identity.genesis.clone(),
            entry,
        };
        if storage::confirm_closed(&private, &actual, &reference)?.1 != raw {
            return Err(Error::StateCommitUnknown);
        }
        proof.recheck(host)?;
        Ok(())
    }
    /// Bounded durable retirement work survives a service crash between Closed,
    /// actual keeper release and its final protected readback.
    pub(in crate::platform::windows) fn writer_pending_release_census(
        host: &StartedServiceContext,
    ) -> Result<Vec<WriterReference>> {
        let _guard = BootstrapGuard::acquire(&host.context.seed, false)?;
        let loaded = current(&Store::new(&host.context.seed), host)?;
        Ok(loaded
            .pending
            .iter()
            .map(|(row, raw)| WriterReference {
                scope: loaded.genesis.scope.clone(),
                genesis: loaded.genesis.genesis.clone(),
                entry: row.entry(raw),
            })
            .collect())
    }
    /// Closed owns BootstrapGuard through this entire acknowledgement. No
    /// prepare/Saved/fence state is changed; only exact retired custody leaves
    /// the bounded pending-release index.
    pub(in crate::platform::windows) fn finish_writer_release(
        host: &StartedServiceContext,
        closed: &ClosedWriterRead,
        ack: &KeeperReleasedAck,
        peer: &mut KeeperPeer<'_>,
    ) -> Result<()> {
        closed.recheck(host)?;
        keeper::require_active_peer(host, peer)?;
        let store = Store::new(&host.context.seed);
        let actual = running(&store, host)?;
        if ack.keeper_selector() != peer.selector()?
            || ack.installation_scope_id() != actual.installation_scope_id
            || ack.host_epoch() != actual.host_epoch
            || ack.registration_digest() != actual.public().digest()?
            || ack.job_name() != closed.job_name()
            || ack.job_binding() != json::sha256(&closed.identity_bytes()?)
        {
            return Err(Error::ScopeConflict);
        }
        let loaded = current(&store, host)?;
        let entry = closed.row.entry(&closed.raw);
        if loaded
            .pending
            .iter()
            .any(|(row, raw)| row.entry(raw) == entry)
        {
            storage::commit(
                &store,
                host,
                &loaded.head_raw,
                Some(&closed.raw),
                storage::Change::Release,
            )?;
        } else if loaded.head.active.iter().any(|r| r.id == entry.id)
            || loaded.pending.iter().any(|(row, _)| row.id == entry.id)
        {
            return Err(Error::ScopeConflict);
        }
        closed.recheck(host)?;
        keeper::require_active_peer(host, peer)
    }
    /// A continuously held, authenticated keeper's observed kernel exit proves
    /// its own handle table was destroyed. It cannot close a live writer row;
    /// the genuine Closed token is still mandatory and owns the guard.
    pub(in crate::platform::windows) fn finish_writer_release_after_keeper_exit(
        host: &Arc<StartedServiceContext>,
        closed: &ClosedWriterRead,
        exited: &NativeKeeperExit<'_>,
    ) -> Result<()> {
        closed.recheck(host)?;
        keeper::require_active_exit(host, exited)?;
        let store = Store::new(&host.context.seed);
        let loaded = current(&store, host)?;
        let entry = closed.row.entry(&closed.raw);
        if loaded
            .pending
            .iter()
            .any(|(row, raw)| row.entry(raw) == entry)
        {
            storage::commit(
                &store,
                host,
                &loaded.head_raw,
                Some(&closed.raw),
                storage::Change::Release,
            )?;
        } else if loaded.head.active.iter().any(|r| r.id == entry.id)
            || loaded.pending.iter().any(|(row, _)| row.id == entry.id)
        {
            return Err(Error::ScopeConflict);
        }
        closed.recheck(host)?;
        keeper::require_active_exit(host, exited)
    }
    /// Only older service incarnations, selected from the complete native
    /// protected read set. A PID, missing row or caller list cannot select one.
    pub(in crate::platform::windows) fn writer_recovery_census(
        host: &StartedServiceContext,
    ) -> Result<WriterCensus> {
        let _guard = BootstrapGuard::acquire(&host.context.seed, false)?;
        let store = Store::new(&host.context.seed);
        let mut loaded = current(&store, host)?;
        loaded
            .active
            .retain(|(row, _)| row.identity.origin_host_epoch < host.startup.host_epoch);
        census(loaded)
    }
    pub(in crate::platform::windows) fn confirm_writer_closed(
        host: &StartedServiceContext,
        reference: &WriterReference,
    ) -> Result<ClosedWriterRead> {
        let guard = BootstrapGuard::acquire(&host.context.seed, false)?;
        let store = Store::new(&host.context.seed);
        let actual = running(&store, host)?;
        let private = store.key(false, None)?.ok_or(Error::SetupIncomplete)?;
        let (row, raw) = storage::confirm_closed(&private, &actual, reference)?;
        Ok(ClosedWriterRead {
            _guard: guard,
            row,
            raw,
        })
    }
    pub(in crate::platform::windows) fn resume_writer_custody<'h>(
        host: &'h StartedServiceContext,
        job: &WriterJob,
    ) -> Result<ProtectedAdmissionIntent<'h>> {
        let token = reopen_from_custody(
            host,
            job.protected_name(),
            job.retained_original(),
            job.origin_host_epoch(),
        )?;
        job.recheck_intent_binding(host, &token)?;
        Ok(token)
    }
    pub(in crate::platform::windows) fn resume_writer_pending<'h>(
        host: &'h StartedServiceContext,
        job: &PendingWriterJob,
    ) -> Result<ProtectedAdmissionIntent<'h>> {
        let token = reopen_from_custody(
            host,
            job.protected_name(),
            job.retained_original(),
            job.origin_host_epoch(),
        )?;
        job.recheck_intent_binding(host, &token)?;
        Ok(token)
    }
    pub(in crate::platform::windows) fn writer_census(
        &self,
        host: &StartedServiceContext,
    ) -> Result<WriterCensus> {
        let _guard = BootstrapGuard::acquire(&host.context.seed, false)?;
        census(current(self, host)?)
    }
    pub(in crate::platform::windows) fn begin_writer_original<'h>(
        host: &'h StartedServiceContext,
        reservation: &ReservedWriterJob,
        peer: &Peer,
        pipe: &Pipe,
    ) -> Result<ProtectedAdmissionIntent<'h>> {
        Self::begin_writer_checked(host, &peer.process, || {
            original_identity(host, reservation, peer, pipe)
        })
    }
    #[allow(unsafe_code)]
    pub(in crate::platform::windows) fn begin_writer_spawned<'h>(
        host: &'h StartedServiceContext,
        reservation: &ReservedWriterJob,
        child: &super::super::cli_spawn::SuspendedCli,
        peer: &Peer,
        pipe: &Pipe,
    ) -> Result<ProtectedAdmissionIntent<'h>> {
        Self::begin_writer_checked(host, child.original(), || {
            child.recheck(host, peer, pipe)?;
            if reservation.origin_host_epoch() != host.startup.host_epoch
                || unsafe {
                    windows_sys::Win32::Foundation::CompareObjectHandles(
                        reservation.original().raw(),
                        child.original().raw(),
                    )
                } == 0
            {
                return Err(Error::PeerRefused);
            }
            retained_reservation_identity(host, reservation, child.original())
        })
    }
    fn begin_writer_checked<'h>(
        host: &'h StartedServiceContext,
        original: &Arc<RetainedProcess>,
        check: impl Fn() -> Result<(String, record::Identity)>,
    ) -> Result<ProtectedAdmissionIntent<'h>> {
        let guard = BootstrapGuard::acquire(&host.context.seed, false)?;
        let store = Store::new(&host.context.seed);
        let (id, identity) = check()?;
        let loaded = current(&store, host)?;
        let row = record::Row::reserved(id, identity);
        row.validate(&loaded.genesis)?;
        if let Some((existing, raw)) = loaded.active.iter().find(|(r, _)| r.id == row.id) {
            if existing != &row {
                return Err(Error::ScopeConflict);
            }
            admission_open(&store, host, &loaded.head_raw)?;
            return Ok(ProtectedAdmissionIntent {
                _guard: guard,
                store,
                host,
                service_epoch: host.startup.host_epoch,
                head_raw: loaded.head_raw.clone(),
                row: existing.clone(),
                row_raw: raw.clone(),
                actual_original: Some(Arc::clone(original)),
            });
        }
        if loaded
            .active
            .iter()
            .any(|(r, _)| r.identity.original == row.identity.original)
        {
            return Err(Error::IpcConflict);
        }
        let (next, row, row_raw) = storage::commit(
            &store,
            host,
            &loaded.head_raw,
            None,
            storage::Change::Reserve(row),
        )?;
        if check()? != (row.id.clone(), row.identity.clone()) {
            return Err(Error::PeerRefused);
        }
        Ok(ProtectedAdmissionIntent {
            _guard: guard,
            store,
            host,
            service_epoch: host.startup.host_epoch,
            head_raw: next.head_raw,
            row,
            row_raw,
            actual_original: Some(Arc::clone(original)),
        })
    }
    /// Retry only the same retained reservation/actual original after an
    /// uncertain Reserved commit, never generate a replacement name or slot.
    pub(in crate::platform::windows) fn resume_writer_original<'h>(
        host: &'h StartedServiceContext,
        reservation: &ReservedWriterJob,
        peer: &Peer,
        pipe: &Pipe,
    ) -> Result<ProtectedAdmissionIntent<'h>> {
        let guard = BootstrapGuard::acquire(&host.context.seed, false)?;
        let store = Store::new(&host.context.seed);
        let (id, identity) = original_identity(host, reservation, peer, pipe)?;
        let loaded = current(&store, host)?;
        let (row, row_raw) = loaded
            .active
            .iter()
            .find(|(r, _)| r.id == id)
            .ok_or(Error::PrerequisiteUnavailable)?;
        if row.identity != identity || row.phase != record::Phase::Reserved {
            return Err(Error::ScopeConflict);
        }
        admission_open(&store, host, &loaded.head_raw)?;
        Ok(ProtectedAdmissionIntent {
            _guard: guard,
            store,
            host,
            service_epoch: host.startup.host_epoch,
            head_raw: loaded.head_raw.clone(),
            row: row.clone(),
            row_raw: row_raw.clone(),
            actual_original: Some(Arc::clone(&peer.process)),
        })
    }
    /// Reconcile the SAME retained pre-effect reservation. Exact existing
    /// Reserved resumes; confirmed absence publishes the same identity through
    /// exclusive native creation in TxR. Generic read failures never mean absent.
    pub(in crate::platform::windows) fn resume_or_begin_writer_original<'h>(
        host: &'h StartedServiceContext,
        reservation: &ReservedWriterJob,
        peer: &Peer,
        pipe: &Pipe,
    ) -> Result<ProtectedAdmissionIntent<'h>> {
        Self::begin_writer_original(host, reservation, peer, pipe)
    }
    pub(in crate::platform::windows) fn recover_writer<'h>(
        host: &'h StartedServiceContext,
        reference: &WriterReference,
    ) -> Result<ProtectedAdmissionIntent<'h>> {
        let guard = BootstrapGuard::acquire(&host.context.seed, false)?;
        let store = Store::new(&host.context.seed);
        let loaded = current(&store, host)?;
        if loaded.genesis.scope != reference.scope || loaded.genesis.genesis != reference.genesis {
            return Err(Error::ScopeConflict);
        }
        let (row, raw) = loaded
            .active
            .iter()
            .find(|(r, b)| r.entry(b) == reference.entry)
            .ok_or(Error::ScopeConflict)?;
        if row.identity.origin_host_epoch >= host.startup.host_epoch {
            return Err(Error::ScopeConflict);
        }
        Ok(ProtectedAdmissionIntent {
            _guard: guard,
            store,
            host,
            service_epoch: host.startup.host_epoch,
            head_raw: loaded.head_raw.clone(),
            row: row.clone(),
            row_raw: raw.clone(),
            actual_original: None,
        })
    }
}
impl<'host> ProtectedAdmissionIntent<'host> {
    pub(in crate::platform::windows) fn participant(
        &self,
    ) -> Result<pinvou_upgrade_ownership::live::Participant> {
        self.recheck(self.host)?;
        self.row.identity.participant(&self.row.id)
    }

    /// The typed ACK proves receipt of this exact original Job. A current held
    /// keeper process and protected Active record are required again at grant;
    /// an old ACK, matching name or record digest alone cannot admit a writer.
    fn require_retention(&self, ack: &KeeperJobAck, peer: &mut KeeperPeer<'_>) -> Result<()> {
        self.recheck(self.host)?;
        keeper::require_active_peer(self.host, peer)?;
        let actual = running(&self.store, self.host)?;
        if ack.keeper_selector() != peer.selector()?
            || ack.installation_scope_id() != actual.installation_scope_id
            || ack.host_epoch() != actual.host_epoch
            || ack.registration_digest() != actual.public().digest()?
            || ack.job_name() != self.job_name()
            || ack.job_binding() != json::sha256(&record::encode(&self.row.identity)?)
        {
            return Err(Error::ScopeConflict);
        }
        Ok(())
    }
    /// Canonical native identity for the private keeper binding. This carries
    /// no authorization independently of this still-held protected token.
    pub(in crate::platform::windows) fn identity_bytes(&self) -> Result<Vec<u8>> {
        self.recheck(self.host)?;
        record::encode(&self.row.identity)
    }
    pub(in crate::platform::windows) fn reference(&self) -> Result<WriterReference> {
        self.recheck(self.host)?;
        Ok(WriterReference {
            scope: self.scope_id().to_owned(),
            genesis: self.genesis_sha256().to_owned(),
            entry: self.row.entry(&self.row_raw),
        })
    }
    /// Lost ACK / confirmed Admitted commit retry on the SAME retained job;
    /// never allocate another intent, process or job for this original writer.
    pub(in crate::platform::windows) fn published_admission<'a>(
        &'a self,
        job: &WriterJob,
        ack: &KeeperJobAck,
        peer: &mut KeeperPeer<'_>,
    ) -> Result<PublishedAdmission<'a, 'host>> {
        self.recheck(self.host)?;
        if self.phase() != WriterJobIntentPhase::Admitted {
            return Err(Error::ScopeConflict);
        }
        job.recheck_for_admission(self.host, self)?;
        self.require_retention(ack, peer)?;
        Ok(PublishedAdmission { intent: self })
    }
    pub(in crate::platform::windows) fn recheck(&self, host: &StartedServiceContext) -> Result<()> {
        if !ptr::eq(self.host, host) || host.startup.host_epoch != self.service_epoch {
            return Err(Error::ServiceIdentityRefused);
        }
        // No BootstrapGuard reacquisition: this token already owns it. Store
        // reads and authenticate_running must remain non-reentrant readers.
        let loaded = current(&self.store, host)?;
        if loaded.head_raw != self.head_raw
            || !loaded
                .active
                .iter()
                .any(|(r, raw)| r == &self.row && raw == &self.row_raw)
        {
            return Err(Error::ScopeConflict);
        }
        Ok(())
    }
    /// Opaque exact bytes freshly read from the actual protected key. Borrow is
    /// to the token's previously verified same bytes, never to caller DATA.
    pub(in crate::platform::windows) fn protected_record_bytes(&self) -> Result<&[u8]> {
        self.recheck(self.host)?;
        Ok(&self.row_raw)
    }
    pub(in crate::platform::windows) fn scope_id(&self) -> &str {
        &self.row.identity.scope
    }
    pub(in crate::platform::windows) fn genesis_sha256(&self) -> &str {
        &self.row.identity.genesis
    }
    pub(in crate::platform::windows) fn service_sid(&self) -> &str {
        &self.row.identity.service_sid
    }
    pub(in crate::platform::windows) fn job_name(&self) -> &str {
        &self.row.identity.job_name
    }
    pub(in crate::platform::windows) fn original_selector(
        &self,
    ) -> &crate::identity::ProcessSelector {
        &self.row.identity.original
    }
    pub(in crate::platform::windows) fn origin_host_epoch(&self) -> u64 {
        self.row.identity.origin_host_epoch
    }
    pub(in crate::platform::windows) fn phase(&self) -> WriterJobIntentPhase {
        match self.row.phase {
            record::Phase::Reserved => WriterJobIntentPhase::Reserved,
            record::Phase::Admitted => WriterJobIntentPhase::Admitted,
            record::Phase::Draining => WriterJobIntentPhase::Draining,
            record::Phase::Closed => WriterJobIntentPhase::Closed,
        }
    }
    pub(in crate::platform::windows) fn actual_original(&self) -> Option<&RetainedProcess> {
        self.actual_original.as_deref()
    }
    pub(in crate::platform::windows) fn transition_admitted<'a>(
        &'a mut self,
        job: &WriterJob,
        ack: &KeeperJobAck,
        peer: &mut KeeperPeer<'_>,
    ) -> Result<PublishedAdmission<'a, 'host>> {
        self.recheck(self.host)?;
        if self.phase() != WriterJobIntentPhase::Reserved {
            return Err(Error::ScopeConflict);
        }
        job.recheck_for_admission(self.host, self)?;
        self.require_retention(ack, peer)?;
        let (loaded, row, raw) = storage::commit(
            &self.store,
            self.host,
            &self.head_raw,
            Some(&self.row_raw),
            storage::Change::Admit,
        )?;
        self.head_raw = loaded.head_raw;
        self.row = row;
        self.row_raw = raw;
        job.recheck_for_admission(self.host, self)?;
        self.recheck(self.host)?;
        self.require_retention(ack, peer)?;
        Ok(PublishedAdmission { intent: self })
    }
    /// Root's internal coordinator only. Current real protected preparation,
    /// freeze epoch and exact inventory participant are derived IN this TxR.
    pub(in crate::platform::windows) fn begin_draining(&mut self) -> Result<()> {
        self.recheck(self.host)?;
        if !matches!(
            self.phase(),
            WriterJobIntentPhase::Admitted | WriterJobIntentPhase::Draining
        ) {
            return Err(Error::ScopeConflict);
        }
        let (loaded, row, raw) = storage::commit(
            &self.store,
            self.host,
            &self.head_raw,
            Some(&self.row_raw),
            storage::Change::Drain,
        )?;
        self.head_raw = loaded.head_raw;
        self.row = row;
        self.row_raw = raw;
        self.recheck(self.host)
    }
    pub(in crate::platform::windows) fn close_live(&self, zero: &NativeJobZero<'_>) -> Result<()> {
        self.recheck(self.host)?;
        zero.recheck_intent(self.host, self)?;
        self.close()
    }
    pub(in crate::platform::windows) fn close_pending(
        &self,
        zero: &NativePendingJobZero<'_>,
    ) -> Result<()> {
        self.recheck(self.host)?;
        zero.recheck_intent(self.host, self)?;
        if self.phase() != WriterJobIntentPhase::Reserved {
            return Err(Error::ScopeConflict);
        }
        self.close()
    }
    pub(in crate::platform::windows) fn close_recovered(
        &self,
        zero: &NativeRecoveredJobZero<'_, '_, '_>,
    ) -> Result<()> {
        self.recheck(self.host)?;
        zero.recheck_intent(self.host, self)?;
        self.close()
    }
    fn close(&self) -> Result<()> {
        // After successful close this old token deliberately becomes stale.
        // It remains guarded until drop, and cannot mint a new admission/freeze.
        storage::commit(
            &self.store,
            self.host,
            &self.head_raw,
            Some(&self.row_raw),
            storage::Change::Close,
        )
        .map(|_| ())
    }
}
impl PublishedAdmission<'_, '_> {
    pub(in crate::platform::windows) fn recheck(
        &self,
        job: &WriterJob,
        ack: &KeeperJobAck,
        peer: &mut KeeperPeer<'_>,
    ) -> Result<()> {
        self.intent.recheck(self.intent.host)?;
        if self.intent.phase() != WriterJobIntentPhase::Admitted {
            return Err(Error::ScopeConflict);
        }
        job.recheck_for_admission(self.intent.host, self.intent)?;
        self.intent.require_retention(ack, peer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pure parsed-record DATA only. No host, guard, registry token, process,
    // job or native zero authority is constructed or claimed by these tests.
    fn loaded(genesis: record::Genesis, rows: Vec<record::Row>) -> storage::Loaded {
        let mut head = record::Head::initial(&record::encode(&genesis).unwrap());
        let mut active = Vec::new();
        for row in rows {
            row.validate(&genesis).unwrap();
            let raw = record::encode(&row).unwrap();
            head = head
                .advance(&record::encode(&head).unwrap(), &row, &raw)
                .unwrap();
            active.push((row, raw));
        }
        let head_raw = record::encode(&head).unwrap();
        storage::Loaded {
            genesis,
            head,
            head_raw,
            active,
            pending: Vec::new(),
        }
    }

    #[test]
    fn journal_diagnostic_counts_uncertain_reservations_and_closed_pending_release_separately() {
        let (genesis, base) = record::census_test_data();
        let mut input = loaded(genesis, vec![base.clone()]);
        // Synthetic aggregation DATA, not a native read set or a transition
        // grant. Production first validates each row/history through load().
        for phase in [record::Phase::Admitted, record::Phase::Draining] {
            let mut row = base.clone();
            row.phase = phase;
            input.active.push((row, Vec::new()));
        }
        let mut closed = base;
        closed.phase = record::Phase::Closed;
        input.pending.push((closed.clone(), Vec::new()));
        assert_eq!(
            journal_counts(&input).unwrap(),
            crate::WriterJournalCounts {
                reserved: 1,
                admitted: 1,
                draining: 1,
                pending_release: 1,
            }
        );
        input.active.push((closed, Vec::new()));
        assert!(journal_counts(&input).is_err());
        input.active.pop();
        input.pending[0].0.phase = record::Phase::Admitted;
        assert!(journal_counts(&input).is_err());
        input.pending.clear();
        while input.active.len() <= pinvou_upgrade_ownership::live::MAX_PARTICIPANTS {
            input.active.push(input.active[0].clone());
        }
        assert!(journal_counts(&input).is_err());
    }

    #[test]
    fn census_keeps_native_selection_bound_when_public_hash_order_reverses_private_order() {
        let (genesis, base) = record::census_test_data();
        // Find a deterministic pair of valid DATA records whose public domain
        // hashes reverse their private-key order. Neither ordering is injected
        // into production census; it derives both IDs from these real schemas.
        let mut prior: Option<(record::Row, pinvou_upgrade_ownership::live::Participant)> = None;
        let mut reversed = None;
        for number in 1..=256u32 {
            let mut row = base.clone();
            row.id = format!("{number:064x}");
            row.identity.job_name = format!("Global\\PinvouWriter-{}", row.id);
            row.identity.original.process_id = number + 1000;
            let participant = row.identity.participant(&row.id).unwrap();
            if let Some((old, old_participant)) = prior.take() {
                if old_participant.id > participant.id {
                    reversed = Some((old, old_participant, row, participant));
                    break;
                }
            }
            prior = Some((row, participant));
        }
        let (private_first, public_last, private_last, public_first) =
            reversed.expect("fixture must exercise different public/private ordering");
        assert!(private_first.id < private_last.id);
        let expected_first_key = private_last.id.clone();
        let expected_last_key = private_first.id.clone();
        let seen = census(loaded(genesis, vec![private_first, private_last])).unwrap();
        assert_eq!(seen.participants.len(), 2);
        assert_eq!(seen.references.len(), 2);
        assert!(seen.participants[0] == public_first && seen.participants[1] == public_last);
        assert_eq!(seen.references[0].entry.id, expected_first_key);
        assert_eq!(seen.references[1].entry.id, expected_last_key);
        for (reference, participant) in seen.references.iter().zip(&seen.participants) {
            assert!(reference.entry.id != participant.id); // no private-name suffix exposure
        }
    }

    #[test]
    fn census_refuses_duplicate_public_identity_instead_of_deduplicating_or_misbinding() {
        let (genesis, row) = record::census_test_data();
        let mut input = loaded(genesis, vec![row.clone()]);
        // A malformed duplicate DATA read set exercises the defensive census
        // collision path; it does not fabricate any privileged native token.
        input
            .active
            .push((row.clone(), record::encode(&row).unwrap()));
        assert!(matches!(census(input), Err(Error::StateCorrupt)));
    }
}
