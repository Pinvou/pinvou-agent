//! Service-held per-writer process-tree custody. This is not a filesystem fence,
//! a save acknowledgement, or permission to terminate business processes.
//!
//! The client must be behind its startup write/child-spawn barrier before this
//! adapter is called. The service must durably commit/read back admission before
//! releasing that barrier. Existing children cannot be captured retroactively.
use super::{
    handle::Handle,
    installed::StartedServiceContext,
    peer::Peer,
    pipe::{Pipe, nonce},
    process::RetainedProcess,
    registry::{
        Store,
        writers::{ProtectedAdmissionIntent, WriterJobIntentPhase},
    },
    security::NativeSecurity,
};
use crate::{
    error::{Error, Result},
    identity::ProcessSelector,
};
use std::{mem::size_of, sync::Arc};
use windows_sys::Win32::{
    Foundation::{CompareObjectHandles, ERROR_ALREADY_EXISTS, GetLastError},
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation, OpenJobObjectW,
            QueryInformationJobObject, SetInformationJobObject,
        },
        Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_SYNCHRONIZE,
            PROCESS_TERMINATE,
        },
    },
};

/// No Debug/Serialize: the private object name is protected registration data.
/// Creation alone does not admit a writer or authorize a grant reply.
pub(super) struct ReservedWriterJob {
    original: Arc<RetainedProcess>,
    name: String,
    service_sid: String,
    scope: String,
    genesis: String,
    origin_host_epoch: u64,
}
pub(super) struct PendingWriterJob {
    core: JobCore,
}
pub(super) struct WriterJob {
    core: JobCore,
}
/// Reopened original protected named job; no reconstructed original process
/// handle, no process/path/PID admission and no API for assigning more members.
pub(super) struct RecoveredWriterJob {
    job: Handle,
    name: String,
    service_sid: String,
    scope: String,
    genesis: String,
    original_selector: ProcessSelector,
    origin_host_epoch: u64,
}
pub(super) struct RecoveredWriterJobOpenFailure {
    pub(super) custody: Option<RecoveredWriterJob>,
    pub(super) error: Error,
}
/// The dispatcher must retain this custody while admission stays closed. An
/// error after AssignProcessToJobObject must not lose the only service handles.
pub(super) struct WriterJobAssignmentFailure {
    pub(super) pending: PendingWriterJob,
    pub(super) error: Error,
}
pub(super) struct WriterJobCreationFailure {
    pub(super) reservation: ReservedWriterJob,
    pub(super) error: Error,
}
struct JobCore {
    job: Handle,
    original: Arc<RetainedProcess>,
    name: String,
    service_sid: String,
    scope: String,
    genesis: String,
    origin_host_epoch: u64,
}
/// Borrowed source for the fixed keeper query duplication only. Private fields,
/// no Clone/Debug/Serialize, raw-handle/PID/name/path constructor or ACK state.
/// Retaining this borrow also retains the exact protected operation token.
pub(super) struct WriterJobQueryBorrow<'job, 'token, 'host> {
    core: &'job JobCore,
    host: &'host StartedServiceContext,
    intent: &'token ProtectedAdmissionIntent<'host>,
}
impl WriterJobQueryBorrow<'_, '_, '_> {
    /// Native Windows keeper adapter only. The caller must duplicate strictly
    /// QUERY|READ_CONTROL; this method does not transfer or manufacture an ACK.
    pub(super) fn held_job(&self) -> Result<&Handle> {
        self.core.recheck(self.host)?;
        verify_intent(
            self.host,
            self.intent,
            &self.core.name,
            &self.core.scope,
            &self.core.genesis,
            &self.core.service_sid,
            &self.core.original.selector,
            self.core.origin_host_epoch,
        )?;
        Ok(&self.core.job)
    }
}

/// Observation DATA only. In particular this cannot construct NativeJobZero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WriterJobObservation {
    pub(super) active_processes: u32,
    pub(super) original_exited: bool,
}
/// Borrowed native custody, with no bool/JSON constructor. Original process exit
/// plus zero active job members is terminal because this adapter has no API for
/// adding another process after assignment. It proves only contained processes.
pub(super) struct NativeJobZero<'a> {
    owner: &'a WriterJob,
}
/// A still-owned pre-assignment reservation and its actual original exit.
/// Reserved can never contain a Pending/Assigned job or a keeper query copy:
/// successful create consumes it, and every later failure retains Pending.
/// This proves no assignment by this owner, never zero for an existing name.
pub(super) struct NativeUnassignedWriterExit<'a> {
    owner: &'a ReservedWriterJob,
}
impl NativeUnassignedWriterExit<'_> {
    pub(super) fn recheck(&self, host: &StartedServiceContext) -> Result<&ReservedWriterJob> {
        let (scope, genesis, service_sid) = current_scope(host)?;
        if self.owner.scope != scope
            || self.owner.genesis != genesis
            || self.owner.service_sid != service_sid
            || self.owner.origin_host_epoch != host.startup.host_epoch
        {
            return Err(Error::ScopeConflict);
        }
        if !self.owner.original.exited()? {
            return Err(Error::Busy);
        }
        Ok(self.owner)
    }
}
pub(super) struct NativePendingJobZero<'a> {
    owner: &'a PendingWriterJob,
}
/// Separate recovered-job proof: no claim that an original process handle was
/// retained across the service restart, or that its PID was reconstructed.
pub(super) struct NativeRecoveredJobZero<'job, 'token, 'record> {
    owner: &'job RecoveredWriterJob,
    intent: &'token ProtectedAdmissionIntent<'record>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RecoveredWriterJobObservation {
    pub(super) active_processes: u32,
}

fn current_scope(host: &StartedServiceContext) -> Result<(String, String, String)> {
    let record = Store::new(&host.context.seed)
        .read(&host.context.seed)?
        .ok_or(Error::SetupIncomplete)?;
    host.authenticate_running(&record)?;
    let sid = record
        .service_identity
        .service_sid
        .ok_or(Error::ServiceIdentityRefused)?;
    Ok((
        record.installation_scope_id,
        record.genesis_manifest_sha256,
        sid,
    ))
}
impl ReservedWriterJob {
    /// Only an authenticated live App peer on the actual pipe can request
    /// custody. Raw PIDs, JSON selectors, paths and commands are not admission.
    pub(super) fn reserve(host: &StartedServiceContext, peer: &Peer, pipe: &Pipe) -> Result<Self> {
        let (scope, genesis, service_sid) = current_scope(host)?;
        peer.recheck(pipe)?;
        if !peer.role.is_writer() {
            return Err(Error::PeerRefused);
        }
        let name = format!("Global\\PinvouWriter-{}", nonce()?);
        peer.recheck(pipe)?;
        Ok(Self {
            original: Arc::clone(&peer.process),
            name,
            service_sid,
            scope,
            genesis,
            origin_host_epoch: host.startup.host_epoch,
        })
    }
    /// Reserve for the actual suspended process retained by the service. The
    /// requester's pipe is checked only as the creator's authority; it is
    /// never represented as the child process's pipe.
    pub(super) fn reserve_spawned(
        host: &StartedServiceContext,
        child: &super::cli_spawn::SuspendedCli,
        peer: &Peer,
        pipe: &Pipe,
    ) -> Result<Self> {
        let (scope, genesis, service_sid) = current_scope(host)?;
        child.recheck(host, peer, pipe)?;
        let name = format!("Global\\PinvouWriter-{}", nonce()?);
        child.recheck(host, peer, pipe)?;
        Ok(Self {
            original: Arc::clone(child.original()),
            name,
            service_sid,
            scope,
            genesis,
            origin_host_epoch: host.startup.host_epoch,
        })
    }
    /// Persist this internally generated name and original identity in the
    /// protected intent BEFORE create. Never put it in IPC or public reports.
    pub(super) fn protected_name(&self) -> &str {
        &self.name
    }
    pub(super) fn original(&self) -> &RetainedProcess {
        &self.original
    }
    pub(super) fn origin_host_epoch(&self) -> u64 {
        self.origin_host_epoch
    }
    pub(super) fn prove_unassigned_exit(
        &self,
        host: &StartedServiceContext,
    ) -> Result<NativeUnassignedWriterExit<'_>> {
        let proof = NativeUnassignedWriterExit { owner: self };
        proof.recheck(host)?;
        Ok(proof)
    }
    /// A token minted only by the protected registry owner after exact durable
    /// intent readback is mandatory. Its actual held original must equal this
    /// reservation; serialized selectors cannot stand in for that handle.
    #[allow(unsafe_code)]
    pub(super) fn create(
        self,
        host: &StartedServiceContext,
        peer: &Peer,
        pipe: &Pipe,
        intent: &ProtectedAdmissionIntent<'_>,
    ) -> std::result::Result<PendingWriterJob, WriterJobCreationFailure> {
        self.create_checked(host, intent, |original| recheck_peer(original, peer, pipe))
    }
    pub(super) fn create_spawned(
        self,
        host: &StartedServiceContext,
        child: &super::cli_spawn::SuspendedCli,
        peer: &Peer,
        pipe: &Pipe,
        intent: &ProtectedAdmissionIntent<'_>,
    ) -> std::result::Result<PendingWriterJob, WriterJobCreationFailure> {
        self.create_checked(host, intent, |original| {
            recheck_spawned(host, original, child, peer, pipe)
        })
    }
    #[allow(unsafe_code)]
    fn create_checked(
        self,
        host: &StartedServiceContext,
        intent: &ProtectedAdmissionIntent<'_>,
        check: impl Fn(&RetainedProcess) -> Result<()>,
    ) -> std::result::Result<PendingWriterJob, WriterJobCreationFailure> {
        let attempt = || -> Result<Handle> {
            let (scope, genesis, sid) = current_scope(host)?;
            if scope != self.scope || genesis != self.genesis || sid != self.service_sid {
                return Err(Error::ScopeConflict);
            }
            verify_intent(
                host,
                intent,
                &self.name,
                &self.scope,
                &self.genesis,
                &self.service_sid,
                &self.original.selector,
                self.origin_host_epoch,
            )?;
            verify_creation_original(host, intent, &self.original)?;
            check(&self.original)?;
            let security = NativeSecurity::writer_job(&self.service_sid)?;
            let attributes = security.attributes();
            let wide: Vec<_> = self.name.encode_utf16().chain(Some(0)).collect();
            let raw = unsafe { CreateJobObjectW(&attributes, wide.as_ptr()) };
            // Read immediately before any further native call. No adoption of
            // a preexisting object, including a retry of an uncertain create.
            let status = unsafe { GetLastError() };
            let job = Handle::owned(raw)?;
            if status == ERROR_ALREADY_EXISTS {
                return Err(Error::SecurityRefused);
            }
            security.inspect_writer_job(job.raw())?;
            configure(&job)?;
            verify_intent(
                host,
                intent,
                &self.name,
                &self.scope,
                &self.genesis,
                &self.service_sid,
                &self.original.selector,
                self.origin_host_epoch,
            )?;
            verify_creation_original(host, intent, &self.original)?;
            check(&self.original)?;
            Ok(job)
        };
        match attempt() {
            Err(error) => Err(WriterJobCreationFailure {
                reservation: self,
                error,
            }),
            Ok(job) => Ok(PendingWriterJob {
                core: JobCore {
                    job,
                    original: self.original,
                    name: self.name,
                    service_sid: self.service_sid,
                    scope: self.scope,
                    genesis: self.genesis,
                    origin_host_epoch: self.origin_host_epoch,
                },
            }),
        }
    }
}
impl PendingWriterJob {
    pub(super) fn keeper_query_borrow<'job, 'token, 'host>(
        &'job self,
        host: &'host StartedServiceContext,
        intent: &'token ProtectedAdmissionIntent<'host>,
    ) -> Result<WriterJobQueryBorrow<'job, 'token, 'host>> {
        self.recheck_intent_binding(host, intent)?;
        Ok(WriterJobQueryBorrow {
            core: &self.core,
            host,
            intent,
        })
    }
    pub(super) fn retained_original(&self) -> Arc<RetainedProcess> {
        Arc::clone(&self.core.original)
    }
    pub(super) fn origin_host_epoch(&self) -> u64 {
        self.core.origin_host_epoch
    }
    pub(super) fn recheck_intent_binding(
        &self,
        host: &StartedServiceContext,
        intent: &ProtectedAdmissionIntent<'_>,
    ) -> Result<()> {
        self.core.recheck(host)?;
        verify_intent(
            host,
            intent,
            &self.core.name,
            &self.core.scope,
            &self.core.genesis,
            &self.core.service_sid,
            &self.core.original.selector,
            self.core.origin_host_epoch,
        )?;
        if intent.phase() != WriterJobIntentPhase::Reserved {
            return Err(Error::ScopeConflict);
        }
        Ok(())
    }
    pub(super) fn prove_zero<'a>(
        &'a self,
        host: &StartedServiceContext,
        intent: &ProtectedAdmissionIntent<'_>,
    ) -> Result<NativePendingJobZero<'a>> {
        self.recheck_intent_binding(host, intent)?;
        if !self.core.original.exited()? || accounting(&self.core.job)?.ActiveProcesses != 0 {
            return Err(Error::Busy);
        }
        Ok(NativePendingJobZero { owner: self })
    }
    /// For the service's protected intent only; never put this in IPC/reporting.
    pub(super) fn protected_name(&self) -> &str {
        &self.core.name
    }
    pub(super) fn assign(
        self,
        host: &StartedServiceContext,
        peer: &Peer,
        pipe: &Pipe,
        intent: &ProtectedAdmissionIntent<'_>,
    ) -> std::result::Result<WriterJob, WriterJobAssignmentFailure> {
        self.assign_checked(host, intent, |original| recheck_peer(original, peer, pipe))
    }
    pub(super) fn assign_spawned(
        self,
        host: &StartedServiceContext,
        child: &super::cli_spawn::SuspendedCli,
        peer: &Peer,
        pipe: &Pipe,
        intent: &ProtectedAdmissionIntent<'_>,
    ) -> std::result::Result<WriterJob, WriterJobAssignmentFailure> {
        self.assign_checked(host, intent, |original| {
            recheck_spawned(host, original, child, peer, pipe)
        })
    }
    fn assign_checked(
        self,
        host: &StartedServiceContext,
        intent: &ProtectedAdmissionIntent<'_>,
        check: impl Fn(&RetainedProcess) -> Result<()>,
    ) -> std::result::Result<WriterJob, WriterJobAssignmentFailure> {
        let attempt = || -> Result<()> {
            self.core.recheck(host)?;
            verify_intent(
                host,
                intent,
                &self.core.name,
                &self.core.scope,
                &self.core.genesis,
                &self.core.service_sid,
                &self.core.original.selector,
                self.core.origin_host_epoch,
            )?;
            verify_creation_original(host, intent, &self.core.original)?;
            check(&self.core.original)?;
            // A retry keeps the same job and original handles. A previous
            // Assign may already have succeeded before a later query failed.
            if !member(&self.core.job, &self.core.original)? {
                assign_original(&self.core.job, &self.core.original)?;
            }
            // Assignment is irreversible for this process lifetime. Any later
            // error is withheld admission, never proof of no side effect.
            check(&self.core.original)?;
            self.core.recheck(host)?;
            verify_intent(
                host,
                intent,
                &self.core.name,
                &self.core.scope,
                &self.core.genesis,
                &self.core.service_sid,
                &self.core.original.selector,
                self.core.origin_host_epoch,
            )?;
            verify_creation_original(host, intent, &self.core.original)
        };
        match attempt() {
            Ok(()) => Ok(WriterJob { core: self.core }),
            Err(error) => Err(WriterJobAssignmentFailure {
                pending: self,
                error,
            }),
        }
    }
}

#[allow(unsafe_code)]
fn recheck_spawned(
    host: &StartedServiceContext,
    original: &RetainedProcess,
    child: &super::cli_spawn::SuspendedCli,
    peer: &Peer,
    pipe: &Pipe,
) -> Result<()> {
    child.recheck(host, peer, pipe)?;
    if unsafe { CompareObjectHandles(original.raw(), child.original().raw()) } == 0
        || original.exited()?
    {
        return Err(Error::PeerRefused);
    }
    Ok(())
}

#[allow(unsafe_code)]
fn recheck_peer(original: &RetainedProcess, peer: &Peer, pipe: &Pipe) -> Result<()> {
    peer.recheck(pipe)?;
    if !peer.role.is_writer()
        || unsafe { CompareObjectHandles(original.raw(), peer.process.raw()) } == 0
        || original.exited()?
    {
        return Err(Error::PeerRefused);
    }
    Ok(())
}
impl JobCore {
    fn recheck(&self, host: &StartedServiceContext) -> Result<()> {
        let (scope, genesis, sid) = current_scope(host)?;
        if scope != self.scope
            || genesis != self.genesis
            || sid != self.service_sid
            || host.startup.host_epoch != self.origin_host_epoch
        {
            return Err(Error::ScopeConflict);
        }
        NativeSecurity::writer_job(&sid)?.inspect_writer_job(self.job.raw())?;
        check_limits(&self.job)
    }
}
impl WriterJob {
    pub(super) fn keeper_query_borrow<'job, 'token, 'host>(
        &'job self,
        host: &'host StartedServiceContext,
        intent: &'token ProtectedAdmissionIntent<'host>,
    ) -> Result<WriterJobQueryBorrow<'job, 'token, 'host>> {
        self.recheck_intent_binding(host, intent)?;
        Ok(WriterJobQueryBorrow {
            core: &self.core,
            host,
            intent,
        })
    }
    pub(super) fn retained_original(&self) -> Arc<RetainedProcess> {
        Arc::clone(&self.core.original)
    }
    pub(super) fn origin_host_epoch(&self) -> u64 {
        self.core.origin_host_epoch
    }
    pub(super) fn recheck_intent_binding(
        &self,
        host: &StartedServiceContext,
        intent: &ProtectedAdmissionIntent<'_>,
    ) -> Result<()> {
        self.core.recheck(host)?;
        verify_intent(
            host,
            intent,
            &self.core.name,
            &self.core.scope,
            &self.core.genesis,
            &self.core.service_sid,
            &self.core.original.selector,
            self.core.origin_host_epoch,
        )
    }
    /// Registry owner consumes this custody borrow before publishing Admitted;
    /// no caller bool or numeric selector can replace actual membership.
    #[allow(unsafe_code)]
    pub(super) fn recheck_for_admission(
        &self,
        host: &StartedServiceContext,
        intent: &ProtectedAdmissionIntent<'_>,
    ) -> Result<()> {
        self.core.recheck(host)?;
        verify_intent(
            host,
            intent,
            &self.core.name,
            &self.core.scope,
            &self.core.genesis,
            &self.core.service_sid,
            &self.core.original.selector,
            self.core.origin_host_epoch,
        )?;
        let actual_original = intent.actual_original().ok_or(Error::PeerRefused)?;
        if !matches!(
            intent.phase(),
            WriterJobIntentPhase::Reserved | WriterJobIntentPhase::Admitted
        ) || unsafe { CompareObjectHandles(actual_original.raw(), self.core.original.raw()) }
            == 0
            || self.core.original.exited()?
            || !member(&self.core.job, &self.core.original)?
            || accounting(&self.core.job)?.ActiveProcesses == 0
        {
            return Err(Error::PeerRefused);
        }
        Ok(())
    }
    /// For protected committed admission readback only, never public reporting.
    pub(super) fn protected_name(&self) -> &str {
        &self.core.name
    }
    pub(super) fn original(&self) -> &RetainedProcess {
        &self.core.original
    }
    pub(super) fn observe(&self, host: &StartedServiceContext) -> Result<WriterJobObservation> {
        self.core.recheck(host)?;
        let original_exited = self.core.original.exited()?;
        // For a live original this queries membership on the retained ORIGINAL
        // handle, never a process opened from a client-supplied PID.
        if !original_exited && !member(&self.core.job, &self.core.original)? {
            return Err(Error::StateCorrupt);
        }
        let active_processes = accounting(&self.core.job)?.ActiveProcesses;
        if !original_exited && active_processes == 0 {
            return Err(Error::Unavailable);
        }
        Ok(WriterJobObservation {
            active_processes,
            original_exited,
        })
    }
    pub(super) fn prove_zero<'a>(
        &'a self,
        host: &StartedServiceContext,
    ) -> Result<NativeJobZero<'a>> {
        let observation = self.observe(host)?;
        if !observation.original_exited || observation.active_processes != 0 {
            return Err(Error::Busy);
        }
        Ok(NativeJobZero { owner: self })
    }
}
impl NativePendingJobZero<'_> {
    pub(super) fn recheck_intent(
        &self,
        host: &StartedServiceContext,
        intent: &ProtectedAdmissionIntent<'_>,
    ) -> Result<()> {
        self.owner.prove_zero(host, intent).map(|_| ())
    }
}
impl NativeJobZero<'_> {
    pub(super) fn recheck_intent(
        &self,
        host: &StartedServiceContext,
        intent: &ProtectedAdmissionIntent<'_>,
    ) -> Result<()> {
        self.recheck(host)?;
        let core = &self.owner.core;
        verify_intent(
            host,
            intent,
            &core.name,
            &core.scope,
            &core.genesis,
            &core.service_sid,
            &core.original.selector,
            core.origin_host_epoch,
        )
    }
    pub(super) fn recheck(&self, host: &StartedServiceContext) -> Result<()> {
        self.owner.prove_zero(host).map(|_| ())
    }
    pub(super) fn original(&self) -> &RetainedProcess {
        self.owner.original()
    }
}

/// Compare every identity component with the registry owner's nonserializable
/// token. The selector compares protected association DATA only; recovery never
/// opens a process from it or treats it as original handle/UTC authority.
fn verify_intent(
    host: &StartedServiceContext,
    intent: &ProtectedAdmissionIntent<'_>,
    name: &str,
    scope: &str,
    genesis: &str,
    service_sid: &str,
    original: &ProcessSelector,
    origin_host_epoch: u64,
) -> Result<()> {
    intent.recheck(host)?;
    let (actual_scope, actual_genesis, actual_sid) = current_scope(host)?;
    if actual_scope != scope
        || actual_genesis != genesis
        || actual_sid != service_sid
        || intent.scope_id() != scope
        || intent.genesis_sha256() != genesis
        || intent.service_sid() != service_sid
        || intent.job_name() != name
        || intent.original_selector() != original
        || intent.origin_host_epoch() != origin_host_epoch
        || origin_host_epoch == 0
        || !valid_private_job_name(name)
    {
        return Err(Error::ScopeConflict);
    }
    match intent.phase() {
        WriterJobIntentPhase::Reserved
        | WriterJobIntentPhase::Admitted
        | WriterJobIntentPhase::Draining => Ok(()),
        WriterJobIntentPhase::Closed => Err(Error::ScopeConflict),
    }
}
#[allow(unsafe_code)]
fn verify_creation_original(
    host: &StartedServiceContext,
    intent: &ProtectedAdmissionIntent<'_>,
    original: &RetainedProcess,
) -> Result<()> {
    if intent.phase() != WriterJobIntentPhase::Reserved
        || intent.origin_host_epoch() != host.startup.host_epoch
    {
        return Err(Error::ScopeConflict);
    }
    let protected_original = intent.actual_original().ok_or(Error::PeerRefused)?;
    if unsafe { CompareObjectHandles(protected_original.raw(), original.raw()) } == 0
        || protected_original.exited()?
        || original.exited()?
    {
        return Err(Error::PeerRefused);
    }
    Ok(())
}
fn valid_private_job_name(name: &str) -> bool {
    name.strip_prefix("Global\\PinvouWriter-")
        .is_some_and(|suffix| {
            suffix.len() == 64
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}
impl RecoveredWriterJob {
    /// Open ONLY the original name supplied by the protected journal token.
    /// Name absence, access failure, type mismatch or uncertain policy is not
    /// emptiness. Never use CreateJobObjectW or configure on a recovery path.
    #[allow(unsafe_code)]
    pub(super) fn open_existing(
        host: &StartedServiceContext,
        intent: &ProtectedAdmissionIntent<'_>,
    ) -> std::result::Result<Self, RecoveredWriterJobOpenFailure> {
        let open = || -> Result<Handle> {
            verify_intent(
                host,
                intent,
                intent.job_name(),
                intent.scope_id(),
                intent.genesis_sha256(),
                intent.service_sid(),
                intent.original_selector(),
                intent.origin_host_epoch(),
            )?;
            if intent.origin_host_epoch() >= host.startup.host_epoch {
                return Err(Error::ScopeConflict);
            }
            let name: Vec<_> = intent.job_name().encode_utf16().chain(Some(0)).collect();
            // JOB_OBJECT_QUERY plus READ_CONTROL only. No recreation or assign.
            let raw = unsafe { OpenJobObjectW(0x0002_0004, 0, name.as_ptr()) };
            Handle::owned(raw).map_err(|_| Error::PrerequisiteUnavailable)
        };
        let job = open().map_err(|error| RecoveredWriterJobOpenFailure {
            custody: None,
            error,
        })?;
        let recovered = Self {
            job,
            name: intent.job_name().to_owned(),
            service_sid: intent.service_sid().to_owned(),
            scope: intent.scope_id().to_owned(),
            genesis: intent.genesis_sha256().to_owned(),
            original_selector: intent.original_selector().clone(),
            origin_host_epoch: intent.origin_host_epoch(),
        };
        if let Err(error) = recovered.recheck(host, intent) {
            return Err(RecoveredWriterJobOpenFailure {
                custody: Some(recovered),
                error,
            });
        }
        Ok(recovered)
    }
    fn recheck(
        &self,
        host: &StartedServiceContext,
        intent: &ProtectedAdmissionIntent<'_>,
    ) -> Result<()> {
        verify_intent(
            host,
            intent,
            &self.name,
            &self.scope,
            &self.genesis,
            &self.service_sid,
            &self.original_selector,
            self.origin_host_epoch,
        )?;
        if self.origin_host_epoch >= host.startup.host_epoch {
            return Err(Error::ScopeConflict);
        }
        NativeSecurity::writer_job(&self.service_sid)?.inspect_writer_job(self.job.raw())?;
        check_limits(&self.job)
    }
    pub(super) fn observe(
        &self,
        host: &StartedServiceContext,
        intent: &ProtectedAdmissionIntent<'_>,
    ) -> Result<RecoveredWriterJobObservation> {
        self.recheck(host, intent)?;
        let active_processes = accounting(&self.job)?.ActiveProcesses;
        // Close the registry/state observation interval. Failure withholds the
        // result; query failure, missing token or name absence never means zero.
        self.recheck(host, intent)?;
        Ok(RecoveredWriterJobObservation { active_processes })
    }
    pub(super) fn prove_zero<'job, 'token, 'record>(
        &'job self,
        host: &StartedServiceContext,
        intent: &'token ProtectedAdmissionIntent<'record>,
    ) -> Result<NativeRecoveredJobZero<'job, 'token, 'record>> {
        if self.observe(host, intent)?.active_processes != 0 {
            return Err(Error::Busy);
        }
        Ok(NativeRecoveredJobZero {
            owner: self,
            intent,
        })
    }
}
impl NativeRecoveredJobZero<'_, '_, '_> {
    pub(super) fn recheck_intent(
        &self,
        host: &StartedServiceContext,
        intent: &ProtectedAdmissionIntent<'_>,
    ) -> Result<()> {
        self.recheck(host)?;
        self.owner.recheck(host, intent)
    }
    pub(super) fn recheck(&self, host: &StartedServiceContext) -> Result<()> {
        self.owner.prove_zero(host, self.intent).map(|_| ())
    }
}

#[allow(unsafe_code)]
fn configure(job: &Handle) -> Result<()> {
    // No BREAKAWAY_OK, SILENT_BREAKAWAY_OK, KILL_ON_JOB_CLOSE, process/time/memory
    // limits, or UI restrictions. A service crash must leave business alive.
    let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    if unsafe {
        SetInformationJobObject(
            job.raw(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err(Error::Unavailable);
    }
    check_limits(job)
}
#[allow(unsafe_code)]
fn check_limits(job: &Handle) -> Result<()> {
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    let mut written = 0;
    if unsafe {
        QueryInformationJobObject(
            job.raw(),
            JobObjectExtendedLimitInformation,
            (&mut limits as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            &mut written,
        )
    } == 0
        || written as usize != size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()
        || limits.BasicLimitInformation.LimitFlags != 0
    {
        return Err(Error::SecurityRefused);
    }
    Ok(())
}
#[allow(unsafe_code)]
fn member(job: &Handle, original: &RetainedProcess) -> Result<bool> {
    let mut contained = 0;
    if unsafe { IsProcessInJob(original.raw(), job.raw(), &mut contained) } == 0 {
        return Err(Error::Unavailable);
    }
    Ok(contained != 0)
}
#[allow(unsafe_code)]
fn assign_original(job: &Handle, original: &RetainedProcess) -> Result<()> {
    if original.exited()? {
        return Err(Error::PeerRefused);
    }
    // Assignment needs SET_QUOTA and TERMINATE by Windows contract. The handle
    // stays internal and is dropped after assignment; no kill API is offered.
    let assignment = Handle::owned(unsafe {
        OpenProcess(
            PROCESS_SET_QUOTA
                | PROCESS_TERMINATE
                | PROCESS_QUERY_LIMITED_INFORMATION
                | PROCESS_SYNCHRONIZE,
            0,
            original.selector.process_id,
        )
    })?;
    // Retaining the original while opening by PID prevents PID recycling from
    // becoming authority. CompareObjectHandles tests actual kernel identity.
    if unsafe { CompareObjectHandles(assignment.raw(), original.raw()) } == 0
        || original.exited()?
    {
        return Err(Error::PeerRefused);
    }
    if unsafe { AssignProcessToJobObject(job.raw(), assignment.raw()) } == 0 {
        return Err(Error::Unavailable);
    }
    if !member(job, original)? {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}
#[allow(unsafe_code)]
fn accounting(job: &Handle) -> Result<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION> {
    let mut value = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
    let mut written = 0;
    if unsafe {
        QueryInformationJobObject(
            job.raw(),
            JobObjectBasicAccountingInformation,
            (&mut value as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
            size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
            &mut written,
        )
    } == 0
        || written as usize != size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>()
    {
        return Err(Error::Unavailable);
    }
    Ok(value)
}

#[cfg(test)]
#[path = "writer_job_tests.rs"]
mod tests;
