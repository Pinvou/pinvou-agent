//! Owned pre-ready keeper job. Atomic JOB_LIST process creation closes the
//! CreateProcess→Assign crash window. Clearing KILL is private to the spawn
//! module's actual ready-intent plus typed ACK transition, never a readiness bool.
use super::*;
use windows_sys::Win32::{
    Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE, LocalFree},
    Security::{
        Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
        SECURITY_ATTRIBUTES,
    },
    System::{
        JobObjects::{
            CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
            JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
        },
        Threading::{
            DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
            LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST,
            UpdateProcThreadAttribute,
        },
    },
};

const STAGE_FLAGS: u32 = JOB_OBJECT_LIMIT_ACTIVE_PROCESS | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
#[must_use = "retain until actual typed keeper ACK plus durable ready publication"]
pub(in crate::platform::windows) struct StagingJob {
    job: Handle,
}
/// A reduced-access native lease for a process-creation JOB_LIST. It keeps the
/// exact job alive but cannot detach it or authorize a process to resume.
pub(in crate::platform::windows) struct StagingLease(Handle);
impl StagingLease {
    pub(in crate::platform::windows) fn raw(&self) -> HANDLE {
        self.0.raw()
    }
}

/// A JOB_LIST built from the retained native stage handle. The caller must
/// keep this value alive until process creation returns. It grants no resume
/// or custody authority and includes no inherited handles.
pub(in crate::platform::windows) struct JobOnlyAttributes {
    storage: Vec<usize>,
    handle: Box<HANDLE>,
    _lease: StagingLease,
    initialized: bool,
}
impl JobOnlyAttributes {
    #[allow(unsafe_code)]
    pub(in crate::platform::windows) fn new(job: &StagingJob) -> Result<Self> {
        let lease = job.creation_lease()?;
        let mut needed = 0usize;
        if unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut needed) } != 0
            || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER
            || needed == 0
            || needed > 65_536
        {
            return Err(Error::SecurityRefused);
        }
        let mut value = Self {
            storage: vec![0usize; needed.div_ceil(size_of::<usize>())],
            handle: Box::new(lease.raw()),
            _lease: lease,
            initialized: false,
        };
        if unsafe { InitializeProcThreadAttributeList(value.raw(), 1, 0, &mut needed) } == 0 {
            return Err(Error::SecurityRefused);
        }
        value.initialized = true;
        set_job_attribute(value.raw(), &mut value.handle)?;
        Ok(value)
    }
    pub(in crate::platform::windows) fn raw(&self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_ptr().cast_mut().cast()
    }
}
impl Drop for JobOnlyAttributes {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        if self.initialized {
            unsafe { DeleteProcThreadAttributeList(self.raw()) };
        }
    }
}

#[allow(unsafe_code)]
fn set_job_attribute(list: LPPROC_THREAD_ATTRIBUTE_LIST, handle: &mut Box<HANDLE>) -> Result<()> {
    if unsafe {
        UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
            (&mut **handle as *mut HANDLE).cast(),
            size_of::<HANDLE>(),
            ptr::null_mut(),
            ptr::null(),
        )
    } == 0
    {
        return Err(Error::SecurityRefused);
    }
    Ok(())
}
struct Descriptor(*mut std::ffi::c_void);
impl Drop for Descriptor {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
impl StagingJob {
    #[allow(unsafe_code)]
    pub(in crate::platform::windows) fn create() -> Result<Self> {
        // No name, inherited handle or untrusted access. Explicit OWNER RIGHTS
        // removes implicit owner WRITE_DAC. Only the full service's SY access
        // can assign/set/query; restricting BU+IU grants no staging-job access.
        let text: Vec<_> = "O:BAG:BAD:P(A;;RC;;;OW)(A;;0x00020007;;;SY)\0"
            .encode_utf16()
            .collect();
        let mut descriptor = ptr::null_mut();
        let mut size = 0;
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                &mut size,
            )
        } == 0
        {
            return Err(Error::SecurityRefused);
        }
        let _descriptor = Descriptor(descriptor);
        if descriptor.is_null() || size == 0 || size > 65_536 {
            return Err(Error::SecurityRefused);
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let original = Handle::owned(unsafe { CreateJobObjectW(&attributes, ptr::null()) })?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = STAGE_FLAGS;
        limits.BasicLimitInformation.ActiveProcessLimit = 1;
        if unsafe {
            SetInformationJobObject(
                original.raw(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(Error::HandoffOutcomeUnknown);
        }
        // CreateJobObject's creator handle is immediately reduced. No ALL_ACCESS
        // mask is requested or transferred to the child. A second handle exists
        // before the original closes, so KILL custody has no handle gap.
        let mut reduced = ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                original.raw(),
                GetCurrentProcess(),
                &mut reduced,
                0x00020007,
                0,
                0,
            )
        } == 0
        {
            return Err(Error::HandoffOutcomeUnknown);
        }
        let value = Self {
            job: Handle::owned(reduced)?,
        };
        drop(original);
        value.verify_limits()?;
        Ok(value)
    }
    #[allow(unsafe_code)]
    fn verify_limits(&self) -> Result<()> {
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        let mut written = 0;
        if unsafe {
            QueryInformationJobObject(
                self.job.raw(),
                JobObjectExtendedLimitInformation,
                (&mut limits as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                &mut written,
            )
        } == 0
            || written != size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32
            || limits.BasicLimitInformation.LimitFlags != STAGE_FLAGS
            || limits.BasicLimitInformation.ActiveProcessLimit != 1
        {
            return Err(Error::SecurityRefused);
        }
        Ok(())
    }
    #[allow(unsafe_code)]
    pub(in crate::platform::windows) fn verify_member(&self, process: &Handle) -> Result<()> {
        self.verify_member_raw(process.raw())
    }
    #[allow(unsafe_code)]
    fn verify_member_raw(&self, process: HANDLE) -> Result<()> {
        let mut member = 0;
        if unsafe { IsProcessInJob(process, self.job.raw(), &mut member) } == 0 || member == 0 {
            return Err(Error::SecurityRefused);
        }
        Ok(())
    }
    pub(in crate::platform::windows) fn verify_staged(&self) -> Result<()> {
        self.verify_limits()
    }
    #[allow(unsafe_code)]
    pub(in crate::platform::windows) fn creation_lease(&self) -> Result<StagingLease> {
        self.verify_limits()?;
        self.duplicate_creation_lease()
    }
    #[allow(unsafe_code)]
    fn duplicate_creation_lease(&self) -> Result<StagingLease> {
        let mut raw = ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                self.job.raw(),
                GetCurrentProcess(),
                &mut raw,
                0x00020005,
                0,
                0,
            )
        } == 0
        {
            return Err(Error::HandoffOutcomeUnknown);
        }
        Ok(StagingLease(Handle::owned(raw)?))
    }
    /// CLI keeps KILL_ON_JOB_CLOSE for its entire lifetime. Only the exact
    /// durably admitted writer and live keeper acknowledgement may remove the
    /// staging-only one-process limit; the creator cannot provide a boolean.
    #[allow(unsafe_code)]
    pub(in crate::platform::windows) fn allow_admitted_cli_children(
        &self,
        process: &Handle,
        published: &super::super::registry::writers::PublishedAdmission<'_, '_>,
        writer: &super::super::writer_job::WriterJob,
        ack: &super::super::writer_keeper::KeeperJobAck,
        keeper: &mut super::KeeperPeer<'_>,
    ) -> Result<()> {
        published.recheck(writer, ack, keeper)?;
        if unsafe { CompareObjectHandles(process.raw(), writer.original().raw()) } == 0 {
            return Err(Error::PeerRefused);
        }
        self.verify_member(process)?;
        if self.verify_cli_guarded().is_err() {
            self.verify_limits()?;
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if unsafe {
                SetInformationJobObject(
                    self.job.raw(),
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            } == 0
            {
                return Err(Error::HandoffOutcomeUnknown);
            }
        }
        self.verify_cli_guarded()?;
        self.verify_member(process)?;
        published.recheck(writer, ack, keeper)
    }
    #[allow(unsafe_code)]
    pub(in crate::platform::windows) fn verify_cli_guarded(&self) -> Result<()> {
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        let mut written = 0;
        if unsafe {
            QueryInformationJobObject(
                self.job.raw(),
                JobObjectExtendedLimitInformation,
                (&mut limits as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                &mut written,
            )
        } == 0
            || written != size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32
            || limits.BasicLimitInformation.LimitFlags != JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        {
            return Err(Error::SecurityRefused);
        }
        Ok(())
    }
    #[allow(unsafe_code)]
    pub(super) fn verify_detached(&self) -> Result<()> {
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        let mut written = 0;
        if unsafe {
            QueryInformationJobObject(
                self.job.raw(),
                JobObjectExtendedLimitInformation,
                (&mut limits as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                &mut written,
            )
        } == 0
            || written != size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32
            || limits.BasicLimitInformation.LimitFlags != 0
        {
            return Err(Error::HandoffOutcomeUnknown);
        }
        Ok(())
    }
    /// Private to the spawn module. Its only caller first requires the actual
    /// native ready intent AND typed current keeper ACK. No bool interface.
    #[allow(unsafe_code)]
    pub(super) fn detach(&self) -> Result<()> {
        // A previous uncertain Set may already have succeeded. Current native
        // zero flags plus the same fresh ready authority permit convergence;
        // no KILL re-enable or replacement job is attempted.
        if self.verify_detached().is_ok() {
            return Ok(());
        }
        self.verify_staged()?;
        let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        if unsafe {
            SetInformationJobObject(
                self.job.raw(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(Error::HandoffOutcomeUnknown);
        }
        self.verify_detached()
    }
    /// Native observation only; never an installation/safe-empty grant.
    #[allow(unsafe_code)]
    pub(super) fn active_processes(&self) -> Result<u32> {
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        let mut written = 0;
        if unsafe {
            QueryInformationJobObject(
                self.job.raw(),
                JobObjectBasicAccountingInformation,
                (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                &mut written,
            )
        } == 0
            || written != size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32
        {
            return Err(Error::HandoffOutcomeUnknown);
        }
        Ok(accounting.ActiveProcesses)
    }
    pub(super) fn creation_attributes<'a>(
        &self,
        listener: &'a super::InheritedListenerLease,
    ) -> Result<CreationAttributes<'a>> {
        CreationAttributes::new(self, listener)
    }
}

/// Exactly JOB_LIST plus the actual opaque listener's one HANDLE_LIST entry.
pub(super) struct CreationAttributes<'a> {
    storage: Vec<usize>,
    handle: Box<HANDLE>,
    listener_handle: Box<HANDLE>,
    _job_lease: StagingLease,
    _listener: &'a super::InheritedListenerLease,
    initialized: bool,
}
impl<'a> CreationAttributes<'a> {
    #[allow(unsafe_code)]
    fn new(job: &StagingJob, listener: &'a super::InheritedListenerLease) -> Result<Self> {
        // Owned native lease lets every error return the original staging
        // custody without a Rust self-borrow. It never enters HANDLE_LIST.
        let job_lease = job.creation_lease()?;
        let mut needed = 0usize;
        if unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), 2, 0, &mut needed) } != 0
            || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER
            || needed == 0
            || needed > 65_536
        {
            return Err(Error::SecurityRefused);
        }
        let storage = vec![0usize; needed.div_ceil(size_of::<usize>())];
        let mut value = Self {
            storage,
            handle: Box::new(job_lease.raw()),
            listener_handle: Box::new(listener.raw()),
            _job_lease: job_lease,
            _listener: listener,
            initialized: false,
        };
        if unsafe { InitializeProcThreadAttributeList(value.raw(), 2, 0, &mut needed) } == 0 {
            return Err(Error::SecurityRefused);
        }
        value.initialized = true;
        set_job_attribute(value.raw(), &mut value.handle)?;
        if unsafe {
            UpdateProcThreadAttribute(
                value.raw(),
                0,
                windows_sys::Win32::System::Threading::PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                (&mut *value.listener_handle as *mut HANDLE).cast(),
                size_of::<HANDLE>(),
                ptr::null_mut(),
                ptr::null(),
            )
        } == 0
        {
            return Err(Error::SecurityRefused);
        }
        Ok(value)
    }
    pub(super) fn raw(&self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_ptr().cast_mut().cast()
    }
}
impl Drop for CreationAttributes<'_> {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                DeleteProcThreadAttributeList(self.raw());
            }
        }
    }
}
