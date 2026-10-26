//! Query-only transfer into the actual continuously held keeper process. A
//! successful DuplicateHandle is not keeper receipt, durable ACK, or admission.
use super::super::writer_job::WriterJobQueryBorrow;
use super::*;
use windows_sys::Win32::{
    Foundation::{GetHandleInformation, HANDLE},
    System::JobObjects::{
        JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
        QueryInformationJobObject,
    },
};
const QUERY_AND_READ_CONTROL: u32 = 0x0002_0004;
#[must_use = "retain local query backup until actual separate keeper ACK is committed"]
pub(in crate::platform::windows) struct PendingQueryCopy<'keeper> {
    keeper: KeeperPeer<'keeper>,
    local_query: Handle,
    destination: ProcessSelector,
    /// Address only inside that held process. Never interpreted as a local
    /// handle, serialized authority, caller-selected handle, or argv value.
    remote_handle: usize,
}
/// Owned backup and original remote binding. Parking releases only the enum's
/// Rust borrow; the actual keeper remains owned by the native manager.
#[must_use = "retain the original local backup and unresolved remote custody"]
pub(in crate::platform::windows) struct ParkedQueryCopy {
    local_query: Handle,
    destination: ProcessSelector,
    remote_handle: usize,
}
#[must_use = "restore the same owned parked copy before returning the failure"]
pub(in crate::platform::windows) struct QueryResumeFailure {
    pub(in crate::platform::windows) parked: ParkedQueryCopy,
    pub(in crate::platform::windows) error: Error,
}
#[must_use = "an uncertain duplicate must preserve pending local/kernel custody"]
pub(in crate::platform::windows) struct QueryCopyFailure<'keeper> {
    pub(in crate::platform::windows) pending: Option<PendingQueryCopy<'keeper>>,
    pub(in crate::platform::windows) error: Error,
}
impl PendingQueryCopy<'_> {
    pub(in crate::platform::windows) fn park(self) -> ParkedQueryCopy {
        let Self {
            keeper: _,
            local_query,
            destination,
            remote_handle,
        } = self;
        ParkedQueryCopy {
            local_query,
            destination,
            remote_handle,
        }
    }
    pub(in crate::platform::windows) fn keeper_selector(&self) -> Result<&ProcessSelector> {
        self.keeper.selector()
    }
    /// Only the root-owned closed native keeper wire may convey this address.
    /// The keeper must query/inspect its actual handle and return a separate
    /// authenticated challenge-bound ACK before the owner admits any writer.
    pub(in crate::platform::windows) fn remote_handle_value(&self) -> usize {
        self.remote_handle
    }
    pub(in crate::platform::windows) fn local_query_for_wire(&self) -> &Handle {
        &self.local_query
    }
    pub(in crate::platform::windows) fn bind_challenge(
        &mut self,
        event: NativeChallengeEvent,
    ) -> Result<PendingNativeChallenge> {
        self.recheck()?;
        let pending = self.keeper.bind_challenge(event)?;
        self.recheck()?;
        Ok(pending)
    }
    pub(in crate::platform::windows) fn recheck(&mut self) -> Result<()> {
        self.keeper.recheck()?;
        if self.keeper.selector()? != &self.destination {
            return Err(Error::SecurityRefused);
        }
        verify_remote(
            self.keeper.query_copy_process(),
            self.remote_handle,
            &self.local_query,
        )
    }
}
impl ParkedQueryCopy {
    pub(in crate::platform::windows) fn keeper_selector(&self) -> &ProcessSelector {
        &self.destination
    }
    /// Original destination-process address, including zero for unknown transfer.
    /// This observation never authorizes another transfer or an empty result.
    pub(in crate::platform::windows) fn remote_handle_value(&self) -> usize {
        self.remote_handle
    }
    #[allow(unsafe_code)]
    pub(in crate::platform::windows) fn resume<'keeper>(
        self,
        mut keeper: KeeperPeer<'keeper>,
        source: &WriterJobQueryBorrow<'_, '_, '_>,
    ) -> std::result::Result<PendingQueryCopy<'keeper>, QueryResumeFailure> {
        let checked = (|| -> Result<()> {
            keeper.recheck()?;
            if keeper.selector()? != &self.destination {
                return Err(Error::SecurityRefused);
            }
            let original = source.held_job()?;
            if unsafe { CompareObjectHandles(original.raw(), self.local_query.raw()) } == 0 {
                return Err(Error::SecurityRefused);
            }
            // Duplicate-back only. Never duplicate another handle into the
            // destination, even if the old transfer address is zero/unknown.
            verify_remote(
                keeper.query_copy_process(),
                self.remote_handle,
                &self.local_query,
            )?;
            keeper.recheck()?;
            if keeper.selector()? != &self.destination
                || unsafe { CompareObjectHandles(source.held_job()?.raw(), self.local_query.raw()) }
                    == 0
            {
                return Err(Error::SecurityRefused);
            }
            Ok(())
        })();
        if let Err(error) = checked {
            return Err(QueryResumeFailure {
                parked: self,
                error,
            });
        }
        let Self {
            local_query,
            destination,
            remote_handle,
        } = self;
        Ok(PendingQueryCopy {
            keeper,
            local_query,
            destination,
            remote_handle,
        })
    }
}
impl<'keeper> KeeperPeer<'keeper> {
    // These native destination borrows stay private to this closed query-copy
    // implementation. No external raw-handle or caller-implemented destination.
    fn query_copy_process(&self) -> &Handle {
        match self {
            Self::Running(keeper) => &keeper.child.process,
            Self::Recovered(keeper) => &keeper.process,
        }
    }
    fn query_copy_service_sid(&self) -> &str {
        match self {
            Self::Running(keeper) => &keeper.child.prepared.token.service_sid,
            Self::Recovered(keeper) => keeper.reference.service_sid(),
        }
    }
    #[allow(unsafe_code)]
    pub(in crate::platform::windows) fn duplicate_job_query(
        mut self,
        source: &WriterJobQueryBorrow<'_, '_, '_>,
    ) -> std::result::Result<PendingQueryCopy<'keeper>, QueryCopyFailure<'keeper>> {
        let prepare = (|| -> Result<(Handle, ProcessSelector)> {
            self.recheck()?;
            // Capture the actual authenticated destination before ANY remote
            // effect; even an uncertain transfer must retain this exact binding.
            let destination = self.selector()?.clone();
            let original_job = source.held_job()?;
            let copy = duplicate_local(original_job)?;
            inspect_query_job(copy.raw(), self.query_copy_service_sid())?;
            query_is_job(copy.raw())?;
            if unsafe { CompareObjectHandles(original_job.raw(), copy.raw()) } == 0 {
                return Err(Error::SecurityRefused);
            }
            Ok((copy, destination))
        })();
        let (local_query, destination) = match prepare {
            Ok(value) => value,
            Err(error) => {
                return Err(QueryCopyFailure {
                    pending: None,
                    error,
                });
            }
        };
        let mut remote = ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                local_query.raw(),
                self.query_copy_process().raw(),
                &mut remote,
                QUERY_AND_READ_CONTROL,
                0,
                0,
            )
        } == 0
        {
            // Even a failed transfer cannot release the retained source or
            // authorize a replacement job. No DUPLICATE_CLOSE_SOURCE is used.
            return Err(QueryCopyFailure {
                pending: Some(PendingQueryCopy {
                    keeper: self,
                    local_query,
                    destination,
                    remote_handle: 0,
                }),
                error: Error::HandoffOutcomeUnknown,
            });
        }
        let mut pending = PendingQueryCopy {
            keeper: self,
            local_query,
            destination,
            remote_handle: remote as usize,
        };
        // Revalidate the actual protected admission source after the native
        // transfer. The pending object retains native custody on every failure;
        // a successful duplicate is still not a keeper ACK or writer grant.
        let checked = (|| -> Result<()> {
            let original_job = source.held_job()?;
            if unsafe { CompareObjectHandles(original_job.raw(), pending.local_query.raw()) } == 0 {
                return Err(Error::SecurityRefused);
            }
            pending.recheck()
        })();
        if let Err(error) = checked {
            return Err(QueryCopyFailure {
                pending: Some(pending),
                error,
            });
        }
        Ok(pending)
    }
}
impl RunningKeeper {
    pub(in crate::platform::windows) fn duplicate_job_query<'keeper>(
        &'keeper mut self,
        source: &WriterJobQueryBorrow<'_, '_, '_>,
    ) -> std::result::Result<PendingQueryCopy<'keeper>, QueryCopyFailure<'keeper>> {
        KeeperPeer::Running(self).duplicate_job_query(source)
    }
}
impl RecoveredKeeper {
    pub(in crate::platform::windows) fn duplicate_job_query<'keeper>(
        &'keeper mut self,
        source: &WriterJobQueryBorrow<'_, '_, '_>,
    ) -> std::result::Result<PendingQueryCopy<'keeper>, QueryCopyFailure<'keeper>> {
        KeeperPeer::Recovered(self).duplicate_job_query(source)
    }
}
#[allow(unsafe_code)]
fn duplicate_local(source: &Handle) -> Result<Handle> {
    let mut raw = ptr::null_mut();
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            source.raw(),
            GetCurrentProcess(),
            &mut raw,
            QUERY_AND_READ_CONTROL,
            0,
            0,
        )
    } == 0
    {
        return Err(Error::HandoffOutcomeUnknown);
    }
    let copy = Handle::owned(raw)?;
    let mut flags = 0;
    if unsafe { GetHandleInformation(copy.raw(), &mut flags) } == 0 || flags & 1 != 0 {
        return Err(Error::SecurityRefused);
    }
    Ok(copy)
}
#[allow(unsafe_code)]
fn verify_remote(keeper: &Handle, remote: usize, original: &Handle) -> Result<()> {
    if remote == 0 {
        return Err(Error::HandoffOutcomeUnknown);
    }
    let mut raw = ptr::null_mut();
    if unsafe {
        DuplicateHandle(
            keeper.raw(),
            remote as HANDLE,
            GetCurrentProcess(),
            &mut raw,
            QUERY_AND_READ_CONTROL,
            0,
            0,
        )
    } == 0
    {
        return Err(Error::HandoffOutcomeUnknown);
    }
    let roundtrip = Handle::owned(raw)?;
    if unsafe { CompareObjectHandles(roundtrip.raw(), original.raw()) } == 0 {
        return Err(Error::SecurityRefused);
    }
    query_is_job(roundtrip.raw())
}
#[allow(unsafe_code)]
fn query_is_job(job: HANDLE) -> Result<()> {
    let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
    let mut written = 0;
    if unsafe {
        QueryInformationJobObject(
            job,
            JobObjectBasicAccountingInformation,
            (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
            size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
            &mut written,
        )
    } == 0
        || written != size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32
    {
        return Err(Error::SecurityRefused);
    }
    // The count is intentionally not returned as a zero/death/admission proof.
    Ok(())
}

/// Reuse the native writer owner's exact descriptor policy for every copy.
fn inspect_query_job(job: HANDLE, service_sid: &str) -> Result<()> {
    security::NativeSecurity::writer_job(service_sid)?.inspect_writer_job(job)
}
