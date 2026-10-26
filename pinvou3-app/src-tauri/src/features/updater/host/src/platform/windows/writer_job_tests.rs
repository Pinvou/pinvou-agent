//! Owned native primitives only. No StartedServiceContext, SCM, installer,
//! production admission, current-process job assignment, or E2E fixture.
use super::*;
use std::ptr;
use windows_sys::Win32::{
    Foundation::{INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::{
        IO::{CreateIoCompletionPort, GetQueuedCompletionStatus},
        JobObjects::{
            JOBOBJECT_ASSOCIATE_COMPLETION_PORT, JobObjectAssociateCompletionPortInformation,
        },
        SystemServices::JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO,
        Threading::{
            CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessW, PROCESS_INFORMATION, ResumeThread,
            STARTUPINFOW, TerminateProcess, WaitForSingleObject,
        },
    },
};

struct OwnedSuspendedFixture {
    process: Handle,
    thread: Handle,
    original: Option<RetainedProcess>,
}
impl OwnedSuspendedFixture {
    fn original(&self) -> &RetainedProcess {
        self.original.as_ref().unwrap()
    }
}
impl Drop for OwnedSuspendedFixture {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // Only this test's CreateProcessW result is terminated on failure. The
        // production adapter has no terminate API and never kills on close.
        if unsafe { WaitForSingleObject(self.process.raw(), 0) } == WAIT_TIMEOUT {
            unsafe { TerminateProcess(self.process.raw(), 1) };
        }
        unsafe { WaitForSingleObject(self.process.raw(), 30_000) };
    }
}
#[allow(unsafe_code)]
fn fixture() -> OwnedSuspendedFixture {
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut directory = [0u16; 32768];
    let length =
        unsafe { GetSystemDirectoryW(directory.as_mut_ptr(), directory.len() as u32) } as usize;
    assert!(length > 0 && length < directory.len());
    let path = format!(
        "{}\\cmd.exe",
        String::from_utf16(&directory[..length]).unwrap()
    );
    let application: Vec<_> = path.encode_utf16().chain(Some(0)).collect();
    // This fixed helper performs no business write or child command. Windows
    // may create console infrastructure; terminal accounting covers ALL Job
    // members. It stays suspended until the actual kernel assignment.
    let mut command: Vec<_> = format!("\"{path}\" /d /q /c exit 0")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut startup = STARTUPINFOW::default();
    startup.cb = size_of::<STARTUPINFOW>() as u32;
    let mut information = PROCESS_INFORMATION::default();
    assert_ne!(
        unsafe {
            CreateProcessW(
                application.as_ptr(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                CREATE_SUSPENDED | CREATE_NO_WINDOW,
                ptr::null(),
                ptr::null(),
                &startup,
                &mut information,
            )
        },
        0
    );
    let process = Handle::owned(information.hProcess).unwrap();
    let thread = Handle::owned(information.hThread).unwrap();
    // Establish cleanup ownership before any fallible observation/assertion.
    let mut fixture = OwnedSuspendedFixture {
        process,
        thread,
        original: None,
    };
    fixture.original = Some(RetainedProcess::open(information.dwProcessId, None).unwrap());
    assert_ne!(
        unsafe { CompareObjectHandles(fixture.process.raw(), fixture.original().raw()) },
        0
    );
    fixture
}
#[allow(unsafe_code)]
fn fixture_job() -> Handle {
    // Ordinary owned unnamed fixture. Exact SYSTEM/service-SID ACL coverage
    // requires privileged native acceptance and is explicitly not claimed here.
    let job = Handle::owned(unsafe { CreateJobObjectW(ptr::null(), ptr::null()) }).unwrap();
    configure(&job).unwrap();
    job
}

#[test]
#[allow(unsafe_code)]
fn actual_owned_process_assignment_and_terminal_accounting() {
    let fixture = fixture();
    let job = fixture_job();
    // Subscribe before assignment, so even a fast exit cannot lose the native
    // all-members-zero notification. This is stronger than a short sleep/poll
    // assumption about when Windows publishes terminal Job accounting.
    let port = Handle::owned(unsafe {
        CreateIoCompletionPort(INVALID_HANDLE_VALUE, ptr::null_mut(), 0, 1)
    })
    .unwrap();
    let association = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
        CompletionKey: 1usize as *mut _,
        CompletionPort: port.raw(),
    };
    assert_ne!(
        unsafe {
            SetInformationJobObject(
                job.raw(),
                JobObjectAssociateCompletionPortInformation,
                (&association as *const JOBOBJECT_ASSOCIATE_COMPLETION_PORT).cast(),
                size_of::<JOBOBJECT_ASSOCIATE_COMPLETION_PORT>() as u32,
            )
        },
        0
    );
    assert!(!member(&job, fixture.original()).unwrap());
    assign_original(&job, fixture.original()).unwrap();
    assert!(member(&job, fixture.original()).unwrap());
    assert_eq!(accounting(&job).unwrap().ActiveProcesses, 1);
    assert_eq!(unsafe { ResumeThread(fixture.thread.raw()) }, 1);
    assert_eq!(
        unsafe { WaitForSingleObject(fixture.process.raw(), 30_000) },
        WAIT_OBJECT_0
    );
    assert!(fixture.original().exited().unwrap());
    let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let remaining = end
            .checked_duration_since(std::time::Instant::now())
            .expect("owned Job did not publish terminal accounting");
        let mut message = 0;
        let mut key = 0;
        let mut opaque_process = ptr::null_mut();
        assert_ne!(
            unsafe {
                GetQueuedCompletionStatus(
                    port.raw(),
                    &mut message,
                    &mut key,
                    &mut opaque_process,
                    remaining.as_millis().max(1).min(u32::MAX as u128) as u32,
                )
            },
            0
        );
        assert_eq!(key, 1);
        // The opaque payload is never a caller PID, reopened process or pointer.
        if message == JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO {
            break;
        }
    }
    assert_eq!(accounting(&job).unwrap().ActiveProcesses, 0);
    // This tests the primitive only: no native sealed proof/whole freeze claim.
}

#[test]
#[allow(unsafe_code)]
fn closing_last_job_handle_preserves_owned_suspended_business_process() {
    let fixture = fixture();
    let job = fixture_job();
    assign_original(&job, fixture.original()).unwrap();
    drop(job);
    assert_eq!(
        unsafe { WaitForSingleObject(fixture.process.raw(), 0) },
        WAIT_TIMEOUT
    );
    assert!(!fixture.original().exited().unwrap());
    assert_eq!(unsafe { ResumeThread(fixture.thread.raw()) }, 1);
    assert_eq!(
        unsafe { WaitForSingleObject(fixture.process.raw(), 30_000) },
        WAIT_OBJECT_0
    );
}

#[test]
#[allow(unsafe_code)]
fn native_breakaway_policy_change_is_refused_by_recheck() {
    use windows_sys::Win32::System::JobObjects::JOB_OBJECT_LIMIT_BREAKAWAY_OK;
    let job = fixture_job();
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_BREAKAWAY_OK;
    assert_ne!(
        unsafe {
            SetInformationJobObject(
                job.raw(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        },
        0
    );
    assert_eq!(check_limits(&job), Err(Error::SecurityRefused));
    configure(&job).unwrap();
}

#[test]
#[allow(unsafe_code)]
fn observe_named_job_reopen_after_last_handle_closed_while_owned_process_lives() {
    let fixture = fixture();
    let name = format!("Local\\PinvouOwnedJobLifetime-{}", nonce().unwrap());
    let wide: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
    let raw = unsafe { CreateJobObjectW(ptr::null(), wide.as_ptr()) };
    let created_status = unsafe { GetLastError() };
    let job = Handle::owned(raw).unwrap();
    assert_ne!(created_status, ERROR_ALREADY_EXISTS);
    configure(&job).unwrap();
    assign_original(&job, fixture.original()).unwrap();
    assert!(member(&job, fixture.original()).unwrap());
    drop(job); // LAST user handle created by this owned fixture.
    assert_eq!(
        unsafe { WaitForSingleObject(fixture.process.raw(), 0) },
        WAIT_TIMEOUT
    );
    let reopened = unsafe { OpenJobObjectW(0x0002_0004, 0, wide.as_ptr()) };
    let status = if reopened.is_null() {
        unsafe { GetLastError() }
    } else {
        0
    };
    if reopened.is_null() {
        // Missing/denied/uncertain is an observed platform fact, not a desired
        // outcome, a failure expected by this test, or proof of zero members.
        eprintln!(
            "owned_named_job_lifetime reopen=false native_status={status} original_alive=true membership=unavailable"
        );
    } else {
        let reopened = Handle::owned(reopened).unwrap();
        let contained = member(&reopened, fixture.original()).unwrap();
        eprintln!(
            "owned_named_job_lifetime reopen=true native_status=0 original_alive=true membership={contained}"
        );
        assert!(contained);
    }
    // Report either actual OS observation without printing the private name.
    // This observation never mints a production recovery or freeze proof.
    assert!(!fixture.original().exited().unwrap());
    assert_eq!(unsafe { ResumeThread(fixture.thread.raw()) }, 1);
    assert_eq!(
        unsafe { WaitForSingleObject(fixture.process.raw(), 30_000) },
        WAIT_OBJECT_0
    );
}

// Remaining native coverage (NOT RUN / no fake PASS): descendant inheritance,
// denied breakaway attempt, named collision/adoption refusal, exact privileged
// SYSTEM/service-SID owner+DACL, service crash/reopen, protected journal ordering
// and full startup-barrier/save/drain/root-fence integration. These need the
// root-owned native acceptance adapter; no installed service is created here.
