//! Fixed-purpose keeper child launch and retained kernel custody. This is not a
//! service, general executor, OS sandbox, keeper ACK, or writer admission grant.
//!
//! Root's native owner must commit/read back the reservation BEFORE CreateProcess,
//! then commit the actual suspended selector BEFORE ResumeThread. Persist unknown
//! outcomes and retain every returned custody/failure object; dropping one is not
//! proof that the child or a remote query handle vanished. No automatic respawn,
//! process killing, name-open recovery or empty-job inference is provided here.
mod exit;
mod job_copy;
mod recovered;
mod staging;
mod token;

use super::{
    handle::Handle,
    installed::StartedServiceContext,
    layout::{ProtectedDirectory, ordinal_equal},
    pipe::nonce,
    process::{RetainedProcess, creation_time},
    registry::{
        Store,
        keeper::{KeeperIntentPhase, ProtectedKeeperIntent},
    },
    security,
    writer_keeper::{
        InheritedListenerLease, KeeperReadyAck, NativeChallengeEvent, PendingNativeChallenge,
        PreparedListener,
    },
};
use crate::{
    error::{Error, Result},
    identity::ProcessSelector,
};
use staging::StagingJob;
use std::{mem::size_of, os::windows::ffi::OsStrExt, ptr, sync::Arc};
use token::RestrictedKeeperToken;
use windows_sys::Win32::{
    Foundation::{CompareObjectHandles, DuplicateHandle, ERROR_PRIVILEGE_NOT_HELD, GetLastError},
    System::{
        SystemInformation::{GetSystemDirectoryW, GetWindowsDirectoryW},
        Threading::{
            CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW,
            EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetCurrentProcessId,
            PROCESS_DUP_HANDLE, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW,
        },
    },
};

pub(super) use exit::NativeKeeperExit;
pub(super) use job_copy::{
    ParkedQueryCopy, PendingQueryCopy, QueryCopyFailure, QueryResumeFailure,
};
pub(super) use recovered::{KeeperRecoveryCustody, KeeperRecoveryFailure, RecoveredKeeper};

/// Closed native holders only. Initial empty readiness and staging detach stay
/// exclusive to RunningKeeper; recovery supplies connection authentication.
pub(super) enum KeeperPeer<'keeper> {
    Running(&'keeper mut RunningKeeper),
    Recovered(&'keeper mut RecoveredKeeper),
}
impl KeeperPeer<'_> {
    pub(super) fn observe_exit(&self) -> Result<NativeKeeperExit<'_>> {
        match self {
            Self::Running(keeper) => keeper.observe_exit(),
            Self::Recovered(keeper) => keeper.observe_exit(),
        }
    }
    pub(super) fn reservation_nonce(&self) -> &str {
        match self {
            Self::Running(keeper) => keeper.reservation_nonce(),
            Self::Recovered(keeper) => keeper.reservation_nonce(),
        }
    }
    pub(super) fn selector(&self) -> Result<&ProcessSelector> {
        match self {
            Self::Running(keeper) => keeper.selector(),
            Self::Recovered(keeper) => keeper.selector(),
        }
    }
    pub(super) fn recheck(&mut self) -> Result<()> {
        match self {
            Self::Running(keeper) => keeper.recheck(),
            Self::Recovered(keeper) => keeper.recheck(),
        }
    }
    pub(super) fn bind_challenge(
        &mut self,
        event: NativeChallengeEvent,
    ) -> Result<PendingNativeChallenge> {
        match self {
            Self::Running(keeper) => keeper.bind_challenge(event),
            Self::Recovered(keeper) => keeper.bind_challenge(event),
        }
    }
}

#[must_use = "durably record the reservation before creating the fixed keeper"]
pub(super) struct PreparedKeeperSpawn {
    host: Arc<StartedServiceContext>,
    helper_root: ProtectedDirectory,
    token: RestrictedKeeperToken,
    reservation_nonce: String,
    listener: PreparedListener,
}
#[must_use = "retain native child custody while its durable outcome is unresolved"]
pub(super) struct KeeperChild {
    prepared: PreparedKeeperSpawn,
    process: Handle,
    thread: Option<Handle>,
    process_id: u32,
    selector: Option<ProcessSelector>,
    resume_attempted: bool,
    staging: StagingJob,
}
#[must_use = "running is not keeper acknowledgement or writer admission"]
pub(super) struct RunningKeeper {
    child: KeeperChild,
}
#[must_use = "consume in the actual protected mark_active transition"]
pub(super) struct DetachedStagingReceipt<'keeper> {
    keeper: &'keeper mut RunningKeeper,
    challenge: String,
}

pub(super) enum FailedSpawnCustody {
    /// Includes failures before the API call or a failed API with no owned
    /// process handle. The outer durable intent remains until reconciled.
    Reservation(PreparedKeeperSpawn),
    StagedReservation {
        prepared: PreparedKeeperSpawn,
        staging: StagingJob,
    },
    Child(KeeperChild),
}
#[must_use = "retain custody; failure never grants a retry or safe-empty result"]
pub(super) struct KeeperSpawnFailure {
    pub(super) custody: FailedSpawnCustody,
    pub(super) error: Error,
    pub(super) win32_error: Option<u32>,
}
#[must_use = "retain the child after an uncertain resume or failed authentication"]
pub(super) struct KeeperResumeFailure {
    pub(super) child: KeeperChild,
    pub(super) error: Error,
}

fn current(host: &StartedServiceContext) -> Result<crate::registration::HostRegistration> {
    let record = Store::new(&host.context.seed)
        .read(&host.context.seed)?
        .ok_or(Error::SetupIncomplete)?;
    host.authenticate_running(&record)?;
    #[allow(unsafe_code)]
    if unsafe { GetCurrentProcessId() } != host.context.process.selector.process_id {
        return Err(Error::ServiceIdentityRefused);
    }
    Ok(record)
}
#[allow(unsafe_code)]
fn system_directory(windows_root: bool) -> Result<String> {
    let mut buffer = vec![0u16; 32768];
    let length = unsafe {
        if windows_root {
            GetWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32)
        } else {
            GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32)
        }
    } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(Error::SecurityRefused);
    }
    String::from_utf16(&buffer[..length]).map_err(|_| Error::SecurityRefused)
}
impl PreparedKeeperSpawn {
    pub(super) fn prepare(host: Arc<StartedServiceContext>) -> Result<Self> {
        let record = current(&host)?;
        let sid = record
            .service_identity
            .service_sid
            .as_deref()
            .ok_or(Error::ServiceIdentityRefused)?;
        if host.context.helper.sha256 != record.helper.sha256 {
            return Err(Error::ImageRefused);
        }
        let helper_root = host
            .context
            .targets
            .code
            .child("helpers", false, false, Some(sid))?
            .child(&host.context.seed.build_id, false, false, Some(sid))?;
        let token = RestrictedKeeperToken::from_service(&host, sid)?;
        let listener = PreparedListener::prepare(&host)?;
        let value = Self {
            host,
            helper_root,
            token,
            reservation_nonce: nonce()?,
            listener,
        };
        value.recheck()?;
        Ok(value)
    }
    /// Native owner persists this internally generated binding before effect.
    /// It is DATA, never a replacement for the protected commit/readback itself.
    pub(super) fn reservation_nonce(&self) -> &str {
        &self.reservation_nonce
    }
    pub(super) fn parent_selector(&self) -> &ProcessSelector {
        &self.host.context.process.selector
    }
    pub(super) fn helper_sha256(&self) -> &str {
        &self.host.context.helper.sha256
    }
    fn recheck(&self) -> Result<()> {
        let record = current(&self.host)?;
        if record.service_identity.service_sid.as_deref() != Some(self.token.service_sid.as_str())
            || record.helper.sha256 != self.host.context.helper.sha256
        {
            return Err(Error::ServiceIdentityRefused);
        }
        self.helper_root.recheck()?;
        if pinvou_upgrade_custody::identity(&self.host.context.helper.file)
            .map_err(|_| Error::ImageRefused)?
            != self.host.context.helper.identity
        {
            return Err(Error::ImageRefused);
        }
        let reopened = self.helper_root.file("pinvou-upgrade-helper.exe")?;
        if pinvou_upgrade_custody::identity(&reopened).map_err(|_| Error::ImageRefused)?
            != self.host.context.helper.identity
        {
            return Err(Error::ImageRefused);
        }
        self.helper_root.recheck()
    }
    /// The real outer owner MUST have committed the reservation above before
    /// calling this effect. No bool, serialized reference or callback mints that
    /// permission here; full native intent integration remains with that owner.
    #[allow(unsafe_code)]
    pub(super) fn create_suspended(
        self,
        intent: &ProtectedKeeperIntent,
    ) -> std::result::Result<KeeperChild, KeeperSpawnFailure> {
        let arguments = || -> Result<(Vec<u16>, Vec<u16>, Vec<u16>, Vec<u16>)> {
            intent.require_reserved(&self.host, self.reservation_nonce(), self.helper_sha256())?;
            self.recheck()?;
            let path = self
                .host
                .context
                .layout
                .helper_path(&self.host.context.seed);
            let text = path.to_str().ok_or(Error::ImageRefused)?;
            if text.contains('"') || text.contains('\0') {
                return Err(Error::ImageRefused);
            }
            let application: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            // The ONLY argument is a closed helper mode. No native handles,
            // identities, caller paths, commands or service credentials enter argv.
            let command: Vec<_> = format!("\"{text}\" --writer-custody")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let cwd: Vec<_> = system_directory(false)?
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let root = system_directory(true)?;
            // No inherited PATH/loader variables or caller-controlled environment.
            let environment: Vec<_> = format!("SystemRoot={root}\0WINDIR={root}\0\0")
                .encode_utf16()
                .collect();
            if [
                application.len(),
                command.len(),
                cwd.len(),
                environment.len(),
            ]
            .into_iter()
            .any(|n| n > 32767)
            {
                return Err(Error::ImageRefused);
            }
            Ok((application, command, cwd, environment))
        };
        let (application, mut command, cwd, environment) = match arguments() {
            Ok(value) => value,
            Err(error) => {
                return Err(KeeperSpawnFailure {
                    custody: FailedSpawnCustody::Reservation(self),
                    error,
                    win32_error: None,
                });
            }
        };
        let process_security = match security::NativeSecurity::host_process() {
            Ok(value) => value,
            Err(error) => {
                return Err(KeeperSpawnFailure {
                    custody: FailedSpawnCustody::Reservation(self),
                    error,
                    win32_error: None,
                });
            }
        };
        let attributes = process_security.attributes();
        let listener = match self.listener.inheritance_lease() {
            Ok(value) => value,
            Err(error) => {
                return Err(KeeperSpawnFailure {
                    custody: FailedSpawnCustody::Reservation(self),
                    error,
                    win32_error: None,
                });
            }
        };
        let staging = match StagingJob::create() {
            Ok(value) => value,
            Err(error) => {
                return Err(KeeperSpawnFailure {
                    custody: FailedSpawnCustody::Reservation(self),
                    error,
                    win32_error: None,
                });
            }
        };
        let creation_attributes = match staging.creation_attributes(&listener) {
            Ok(value) => value,
            Err(error) => {
                return Err(KeeperSpawnFailure {
                    custody: FailedSpawnCustody::StagedReservation {
                        prepared: self,
                        staging,
                    },
                    error,
                    win32_error: None,
                });
            }
        };
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = listener.raw();
        startup.StartupInfo.hStdOutput = ptr::null_mut();
        startup.StartupInfo.hStdError = ptr::null_mut();
        startup.lpAttributeList = creation_attributes.raw();
        let mut information = PROCESS_INFORMATION::default();
        if let Err(error) = intent
            .require_reserved(&self.host, self.reservation_nonce(), self.helper_sha256())
            .and_then(|_| self.recheck())
        {
            drop(creation_attributes);
            drop(listener);
            return Err(KeeperSpawnFailure {
                custody: FailedSpawnCustody::StagedReservation {
                    prepared: self,
                    staging,
                },
                error,
                win32_error: None,
            });
        }
        // Same-primary restricted token: no assignment/impersonation/debug/Tcb
        // privilege is requested or enabled. A privilege-not-held result refuses;
        // it is never a reason to fall back to an unrestricted System child.
        let created = unsafe {
            CreateProcessAsUserW(
                self.token.raw(),
                application.as_ptr(),
                command.as_mut_ptr(),
                &attributes,
                ptr::null(),
                1,
                CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
                environment.as_ptr().cast(),
                cwd.as_ptr(),
                &startup.StartupInfo,
                &mut information,
            )
        };
        let create_error = if created == 0 {
            Some(unsafe { GetLastError() })
        } else {
            None
        };
        drop(creation_attributes);
        drop(listener);
        if let Some(code) = create_error {
            return Err(KeeperSpawnFailure {
                custody: FailedSpawnCustody::StagedReservation {
                    prepared: self,
                    staging,
                },
                error: if code == ERROR_PRIVILEGE_NOT_HELD {
                    Error::PrerequisiteUnavailable
                } else {
                    Error::HandoffOutcomeUnknown
                },
                win32_error: Some(code),
            });
        }
        let process = match Handle::owned(information.hProcess) {
            Ok(value) => value,
            Err(error) => {
                // A successful SDK call promises owned valid handles. Preserve
                // unknown intent rather than claiming no child on contract failure.
                drop(Handle::owned(information.hThread));
                return Err(KeeperSpawnFailure {
                    custody: FailedSpawnCustody::StagedReservation {
                        prepared: self,
                        staging,
                    },
                    error,
                    win32_error: None,
                });
            }
        };
        let mut child = KeeperChild {
            prepared: self,
            process,
            thread: None,
            process_id: information.dwProcessId,
            selector: None,
            resume_attempted: false,
            staging,
        };
        let result = (|| -> Result<()> {
            child.thread = Some(Handle::owned(information.hThread)?);
            child.staging.verify_member(&child.process)?;
            child.staging.verify_staged()?;
            child.authenticate()?;
            child.reduce_process_handle()?;
            child.authenticate()
        })();
        match result {
            Ok(()) => Ok(child),
            Err(error) => Err(KeeperSpawnFailure {
                custody: FailedSpawnCustody::Child(child),
                error,
                win32_error: None,
            }),
        }
    }
}
impl KeeperChild {
    pub(super) fn reservation_nonce(&self) -> &str {
        self.prepared.reservation_nonce()
    }
    pub(super) fn recheck(&mut self) -> Result<()> {
        self.authenticate()
    }
    pub(super) fn selector(&self) -> Result<&ProcessSelector> {
        self.selector.as_ref().ok_or(Error::HandoffOutcomeUnknown)
    }
    #[allow(unsafe_code)]
    fn authenticate(&mut self) -> Result<()> {
        self.prepared.recheck()?;
        self.staging.verify_member(&self.process)?;
        let observed =
            RetainedProcess::open(self.process_id, Some(&self.prepared.token.service_sid))?;
        if unsafe { CompareObjectHandles(observed.raw(), self.process.raw()) } == 0
            || observed.selector.creation_file_time != creation_time(self.process.raw())?
            || observed.parent()?.selector != *self.prepared.parent_selector()
            || observed.token.user != self.prepared.host.context.process.token.user
            || observed.token.administrator
            || observed.token.network
            || observed.token.app_container
            || !observed.token.service_sid_enabled
            || !ordinal_equal(
                observed.image.to_str().ok_or(Error::ImageRefused)?,
                self.prepared
                    .host
                    .context
                    .layout
                    .helper_path(&self.prepared.host.context.seed)
                    .to_str()
                    .ok_or(Error::ImageRefused)?,
            )?
        {
            return Err(Error::ServiceIdentityRefused);
        }
        if self
            .selector
            .as_ref()
            .is_some_and(|s| *s != observed.selector)
        {
            return Err(Error::ServiceIdentityRefused);
        }
        self.prepared.token.verify_process(&self.process)?;
        self.selector = Some(observed.selector);
        self.prepared.recheck()
    }
    #[allow(unsafe_code)]
    fn reduce_process_handle(&mut self) -> Result<()> {
        let mut raw = ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                self.process.raw(),
                GetCurrentProcess(),
                &mut raw,
                PROCESS_QUERY_LIMITED_INFORMATION
                    | PROCESS_DUP_HANDLE
                    | PROCESS_SYNCHRONIZE
                    | 0x00020000,
                0,
                0,
            )
        } == 0
        {
            return Err(Error::HandoffOutcomeUnknown);
        }
        self.process = Handle::owned(raw)?;
        Ok(())
    }
    /// Persist the actual suspended selector first. Native service/source/child
    /// are authenticated immediately before and after the one ResumeThread call.
    #[allow(unsafe_code)]
    pub(super) fn resume(
        mut self,
        intent: &ProtectedKeeperIntent,
    ) -> std::result::Result<RunningKeeper, KeeperResumeFailure> {
        let attempt = (|| -> Result<()> {
            if self.resume_attempted {
                return Err(Error::HandoffOutcomeUnknown);
            }
            self.authenticate()?;
            intent.require_suspended(
                &self.prepared.host,
                self.prepared.reservation_nonce(),
                self.selector()?,
            )?;
            self.staging.verify_staged()?;
            let thread = self
                .thread
                .as_ref()
                .ok_or(Error::HandoffOutcomeUnknown)?
                .raw();
            self.resume_attempted = true;
            if unsafe { ResumeThread(thread) } != 1 {
                return Err(Error::HandoffOutcomeUnknown);
            }
            drop(self.thread.take());
            self.authenticate()
        })();
        match attempt {
            Ok(()) => Ok(RunningKeeper { child: self }),
            Err(error) => Err(KeeperResumeFailure { child: self, error }),
        }
    }
}
impl RunningKeeper {
    pub(super) fn reservation_nonce(&self) -> &str {
        self.child.reservation_nonce()
    }
    pub(super) fn selector(&self) -> Result<&ProcessSelector> {
        self.child.selector()
    }
    pub(super) fn recheck(&mut self) -> Result<()> {
        self.child.authenticate()
    }
    pub(super) fn bind_challenge(
        &mut self,
        event: NativeChallengeEvent,
    ) -> Result<PendingNativeChallenge> {
        self.recheck()?;
        event.duplicate_into(&self.child.process, self.selector()?)
    }
    pub(super) fn detach_staging<'keeper>(
        &'keeper mut self,
        intent: &ProtectedKeeperIntent,
        ack: &KeeperReadyAck,
    ) -> Result<DetachedStagingReceipt<'keeper>> {
        self.recheck()?;
        if intent.phase()? == KeeperIntentPhase::Active {
            intent.require_active_ready(
                &self.child.prepared.host,
                self.reservation_nonce(),
                self.selector()?,
                ack,
            )?;
            self.child.staging.verify_detached()?;
        } else {
            intent.require_ready(
                &self.child.prepared.host,
                self.reservation_nonce(),
                self.selector()?,
                ack,
            )?;
            self.child.staging.detach()?;
        }
        self.recheck()?;
        self.child.staging.verify_detached()?;
        Ok(DetachedStagingReceipt {
            keeper: self,
            challenge: ack.challenge().to_owned(),
        })
    }
}
impl DetachedStagingReceipt<'_> {
    pub(super) fn keeper_selector(&self) -> Result<&ProcessSelector> {
        self.keeper.selector()
    }
    pub(super) fn reservation_nonce(&self) -> &str {
        self.keeper.reservation_nonce()
    }
    pub(super) fn recheck_limits0(&mut self) -> Result<()> {
        self.keeper.recheck()?;
        self.keeper.child.staging.verify_detached()
    }
    pub(super) fn require_current(
        &mut self,
        host: &Arc<StartedServiceContext>,
        nonce: &str,
        selector: &ProcessSelector,
        ack: &KeeperReadyAck,
    ) -> Result<()> {
        if !Arc::ptr_eq(host, &self.keeper.child.prepared.host)
            || nonce != self.reservation_nonce()
            || selector != self.keeper_selector()?
            || ack.keeper_selector() != selector
            || ack.challenge() != self.challenge
        {
            return Err(Error::ScopeConflict);
        }
        self.recheck_limits0()
    }
}
pub(super) fn verify_current_restricted(service_sid: &str) -> Result<()> {
    token::verify_current_restricted(service_sid)
}
