//! Reopen only a sealed protected keeper selection. The old creator/epoch are
//! provenance; neither a live old parent nor an epoch reset is required here.
//! No launch, replacement name, process termination, empty-census or Ready proof.
use super::super::registry::keeper::KeeperReference;
use super::*;
use windows_sys::Win32::{
    Foundation::{GetHandleInformation, WAIT_TIMEOUT},
    System::Threading::{OpenProcess, WaitForSingleObject},
};

#[must_use = "retain native recovery custody; connection is not writer admission"]
pub(in crate::platform::windows) struct RecoveredKeeper {
    pub(super) host: Arc<StartedServiceContext>,
    pub(super) reference: KeeperReference,
    pub(super) helper_root: ProtectedDirectory,
    pub(super) process: Handle,
    /// Written only after complete successful native authentication of this
    /// continuously held process. Never populated from saved ACK/state or PID.
    pub(super) authenticated_selector: Option<ProcessSelector>,
}
/// Failure preserves the exact original selection. Once OpenProcess succeeds,
/// its actual handle remains owned even if later authentication is uncertain.
pub(in crate::platform::windows) enum KeeperRecoveryCustody {
    Selection {
        host: Arc<StartedServiceContext>,
        reference: KeeperReference,
    },
    Opened(RecoveredKeeper),
}
#[must_use = "unknown authentication is not exit, safe empty or permission to respawn"]
pub(in crate::platform::windows) struct KeeperRecoveryFailure {
    pub(in crate::platform::windows) custody: KeeperRecoveryCustody,
    pub(in crate::platform::windows) error: Error,
}

fn selected_helper(
    host: &StartedServiceContext,
    reference: &KeeperReference,
) -> Result<ProtectedDirectory> {
    reference.require_active(host)?;
    let record = current(host)?;
    let public = Store::new(&host.context.seed).read_public(&host.context.seed)?;
    if public.digest()? != record.public().digest()?
        || record.installation_scope_id != reference.installation_scope_id()
        || record.service_identity.service_sid.as_deref() != Some(reference.service_sid())
        || record.helper.sha256 != reference.helper_sha256()
        || host.context.helper.sha256 != reference.helper_sha256()
    {
        return Err(Error::ScopeConflict);
    }
    // Use only the current build's actual protected sealed helper. A different
    // helper/build cannot qualify the old keeper by hash/path/name guessing.
    let root = host
        .context
        .targets
        .code
        .child("helpers", false, false, Some(reference.service_sid()))?
        .child(
            &host.context.seed.build_id,
            false,
            false,
            Some(reference.service_sid()),
        )?;
    verify_helper(host, &root)?;
    reference.require_active(host)?;
    Ok(root)
}
pub(super) fn verify_helper(host: &StartedServiceContext, root: &ProtectedDirectory) -> Result<()> {
    root.recheck()?;
    if pinvou_upgrade_custody::identity(&host.context.helper.file)
        .map_err(|_| Error::ImageRefused)?
        != host.context.helper.identity
        || pinvou_upgrade_custody::identity(&root.file("pinvou-upgrade-helper.exe")?)
            .map_err(|_| Error::ImageRefused)?
            != host.context.helper.identity
    {
        return Err(Error::ImageRefused);
    }
    root.recheck()
}
impl RecoveredKeeper {
    #[allow(unsafe_code)]
    pub(in crate::platform::windows) fn reopen(
        host: Arc<StartedServiceContext>,
        reference: KeeperReference,
    ) -> std::result::Result<Self, KeeperRecoveryFailure> {
        let helper_root = match (|| -> Result<ProtectedDirectory> {
            reference.keeper_selector()?.validate()?;
            selected_helper(&host, &reference)
        })() {
            Ok(root) => root,
            Err(error) => {
                return Err(KeeperRecoveryFailure {
                    custody: KeeperRecoveryCustody::Selection { host, reference },
                    error,
                });
            }
        };
        // The protected exact selector is the sole PID source. No caller PID,
        // guessed name, ALL_ACCESS, write/injection/terminate rights or inheritance.
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION
                    | PROCESS_DUP_HANDLE
                    | PROCESS_SYNCHRONIZE
                    | 0x00020000,
                0,
                match reference.keeper_selector() {
                    Ok(selector) => selector.process_id,
                    Err(error) => {
                        return Err(KeeperRecoveryFailure {
                            custody: KeeperRecoveryCustody::Selection { host, reference },
                            error,
                        });
                    }
                },
            )
        };
        let process = match Handle::owned(raw) {
            Ok(process) => process,
            Err(_) => {
                return Err(KeeperRecoveryFailure {
                    custody: KeeperRecoveryCustody::Selection { host, reference },
                    error: Error::HandoffOutcomeUnknown,
                });
            }
        };
        let mut keeper = Self {
            host,
            reference,
            helper_root,
            process,
            authenticated_selector: None,
        };
        if let Err(error) = keeper.recheck() {
            return Err(KeeperRecoveryFailure {
                custody: KeeperRecoveryCustody::Opened(keeper),
                error,
            });
        }
        Ok(keeper)
    }
    pub(in crate::platform::windows) fn selector(&self) -> Result<&ProcessSelector> {
        self.reference.keeper_selector()
    }
    pub(in crate::platform::windows) fn reservation_nonce(&self) -> &str {
        self.reference.reservation_nonce()
    }
    pub(in crate::platform::windows) fn reference(&self) -> &KeeperReference {
        &self.reference
    }
    #[allow(unsafe_code)]
    pub(in crate::platform::windows) fn recheck(&mut self) -> Result<()> {
        self.reference.require_active(&self.host)?;
        let record = current(&self.host)?;
        let public = Store::new(&self.host.context.seed).read_public(&self.host.context.seed)?;
        if public.digest()? != record.public().digest()?
            || record.helper.sha256 != self.reference.helper_sha256()
            || self.host.context.helper.sha256 != self.reference.helper_sha256()
        {
            return Err(Error::ScopeConflict);
        }
        verify_helper(&self.host, &self.helper_root)?;
        let mut flags = 0;
        if unsafe { GetHandleInformation(self.process.raw(), &mut flags) } == 0
            || flags & 1 != 0
            || unsafe { WaitForSingleObject(self.process.raw(), 0) } != WAIT_TIMEOUT
            || creation_time(self.process.raw())? != self.selector()?.creation_file_time
        {
            return Err(Error::HandoffOutcomeUnknown);
        }
        let observed = RetainedProcess::open(
            self.selector()?.process_id,
            Some(self.reference.service_sid()),
        )?;
        if unsafe { CompareObjectHandles(observed.raw(), self.process.raw()) } == 0
            || observed.selector != *self.selector()?
            || observed.token.user != self.host.context.process.token.user
            || observed.token.administrator
            || observed.token.network
            || observed.token.app_container
            || !observed.token.service_sid_enabled
            || !ordinal_equal(
                observed.image.to_str().ok_or(Error::ImageRefused)?,
                self.host
                    .context
                    .layout
                    .helper_path(&self.host.context.seed)
                    .to_str()
                    .ok_or(Error::ImageRefused)?,
            )?
        {
            return Err(Error::ServiceIdentityRefused);
        }
        // No observed.parent(): the original service may have exited/restarted.
        token::verify_recovered_process(&self.host, self.reference.service_sid(), &self.process)?;
        if unsafe { WaitForSingleObject(self.process.raw(), 0) } != WAIT_TIMEOUT {
            return Err(Error::HandoffOutcomeUnknown);
        }
        verify_helper(&self.host, &self.helper_root)?;
        self.reference.require_active(&self.host)?;
        self.authenticated_selector = Some(self.selector()?.clone());
        Ok(())
    }
    pub(in crate::platform::windows) fn bind_challenge(
        &mut self,
        event: NativeChallengeEvent,
    ) -> Result<PendingNativeChallenge> {
        self.recheck()?;
        let pending = event.duplicate_into(&self.process, self.selector()?)?;
        self.recheck()?;
        Ok(pending)
    }
}
