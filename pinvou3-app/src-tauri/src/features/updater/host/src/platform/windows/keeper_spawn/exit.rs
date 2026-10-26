//! Observed exit of the original continuously held keeper process. No reopening,
//! termination, shutdown command, saved exit flag, census or writer-zero proof.
use super::*;
use windows_sys::Win32::{
    Foundation::WAIT_OBJECT_0,
    System::Threading::{GetProcessId, WaitForSingleObject},
};

enum ExitSource<'custody> {
    Running(&'custody RunningKeeper),
    Recovered(&'custody RecoveredKeeper),
}
struct Origin<'custody> {
    host: &'custody Arc<StartedServiceContext>,
    process: &'custody Handle,
    selector: &'custody ProcessSelector,
    nonce: &'custody str,
    scope: &'custody str,
    service_sid: &'custody str,
    helper_sha256: &'custody str,
    origin_host_epoch: u64,
    helper_root: &'custody ProtectedDirectory,
}
#[must_use = "native keeper exit does not prove writer census zero or retire a slot"]
pub(in crate::platform::windows) struct NativeKeeperExit<'custody> {
    source: ExitSource<'custody>,
}
impl RunningKeeper {
    pub(in crate::platform::windows) fn observe_exit(&self) -> Result<NativeKeeperExit<'_>> {
        NativeKeeperExit::observe(ExitSource::Running(self))
    }
}
impl RecoveredKeeper {
    pub(in crate::platform::windows) fn observe_exit(&self) -> Result<NativeKeeperExit<'_>> {
        NativeKeeperExit::observe(ExitSource::Recovered(self))
    }
}
impl<'custody> NativeKeeperExit<'custody> {
    fn observe(source: ExitSource<'custody>) -> Result<Self> {
        let proof = Self { source };
        let origin = proof.origin()?;
        proof.require_current(origin.host, origin.nonce, origin.selector)?;
        Ok(proof)
    }
    fn origin(&self) -> Result<Origin<'_>> {
        match &self.source {
            ExitSource::Running(keeper) => {
                // RunningKeeper is produced only after authenticated suspended
                // custody and the original protected resume transition succeed.
                let prepared = &keeper.child.prepared;
                Ok(Origin {
                    host: &prepared.host,
                    process: &keeper.child.process,
                    selector: keeper.selector()?,
                    nonce: keeper.reservation_nonce(),
                    scope: &prepared.host.startup.installation_scope_id,
                    service_sid: &prepared.token.service_sid,
                    helper_sha256: prepared.helper_sha256(),
                    origin_host_epoch: prepared.host.startup.host_epoch,
                    helper_root: &prepared.helper_root,
                })
            }
            ExitSource::Recovered(keeper) => {
                // A retained handle in a failed Opened recovery is not enough:
                // only an actual completed native authentication qualifies it.
                if keeper.authenticated_selector.as_ref() != Some(keeper.selector()?) {
                    return Err(Error::HandoffOutcomeUnknown);
                }
                let reference = &keeper.reference;
                Ok(Origin {
                    host: &keeper.host,
                    process: &keeper.process,
                    selector: keeper.selector()?,
                    nonce: reference.reservation_nonce(),
                    scope: reference.installation_scope_id(),
                    service_sid: reference.service_sid(),
                    helper_sha256: reference.helper_sha256(),
                    origin_host_epoch: reference.origin_host_epoch(),
                    helper_root: &keeper.helper_root,
                })
            }
        }
    }
    pub(in crate::platform::windows) fn reservation_nonce(&self) -> &str {
        match &self.source {
            ExitSource::Running(keeper) => keeper.reservation_nonce(),
            ExitSource::Recovered(keeper) => keeper.reservation_nonce(),
        }
    }
    pub(in crate::platform::windows) fn keeper_selector(&self) -> Result<&ProcessSelector> {
        match &self.source {
            ExitSource::Running(keeper) => keeper.selector(),
            ExitSource::Recovered(keeper) => keeper.selector(),
        }
    }
    pub(in crate::platform::windows) fn require_current(
        &self,
        host: &Arc<StartedServiceContext>,
        nonce: &str,
        selector: &ProcessSelector,
    ) -> Result<()> {
        let origin = self.origin()?;
        let actual = current(host)?;
        let public = Store::new(&host.context.seed).read_public(&host.context.seed)?;
        if !Arc::ptr_eq(host, origin.host)
            || nonce != origin.nonce
            || selector != origin.selector
            || actual.installation_scope_id != origin.scope
            || actual.service_identity.service_sid.as_deref() != Some(origin.service_sid)
            || actual.helper.sha256 != origin.helper_sha256
            || host.context.helper.sha256 != origin.helper_sha256
            || actual.host_epoch < origin.origin_host_epoch
            || public.digest()? != actual.public().digest()?
        {
            return Err(Error::ScopeConflict);
        }
        // The owner separately checks the selected Active slot under its guard
        // and TxR before retiring it. This native exit proof must remain usable
        // for postcommit checks after that exact slot has become Idle.
        recovered::verify_helper(host, origin.helper_root)?;
        observe_handle_exit(origin.process, origin.selector)?;
        current(host)?;
        observe_handle_exit(origin.process, origin.selector)
    }
}
#[allow(unsafe_code)]
fn observe_handle_exit(process: &Handle, selector: &ProcessSelector) -> Result<()> {
    selector.validate()?;
    // creation_time uses native GetProcessTimes against this held handle. Even
    // if Windows reuses the numeric PID, this never selects that new process.
    if unsafe { GetProcessId(process.raw()) } != selector.process_id
        || creation_time(process.raw())? != selector.creation_file_time
        || unsafe { WaitForSingleObject(process.raw(), 0) } != WAIT_OBJECT_0
    {
        return Err(Error::HandoffOutcomeUnknown);
    }
    Ok(())
}
