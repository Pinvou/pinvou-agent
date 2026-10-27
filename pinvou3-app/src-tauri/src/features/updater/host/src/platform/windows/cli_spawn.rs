//! Actual source custody for service-owned CLI creation. No pathname or token
//! supplied by a requester is accepted as an executable or authorization.
//! This retained input is not itself a process, writer admission or grant.
use super::{
    cli_token::{RequesterToken, ScopedCreationPrivileges, same_requester},
    handle::Handle,
    image::verify_bytes,
    installed::StartedServiceContext,
    keeper_spawn::staging::{JobOnlyAttributes, StagingJob},
    peer::Peer,
    pipe::{Event, Pipe},
    process::RetainedProcess,
    registry::Store,
    security::NativeSecurity,
};
use crate::{
    error::{Error, Result},
    profile::AppFile,
};
use pinvou_upgrade_custody::Identity;
use std::{fs::File, sync::Arc};

/// The process is always suspended and the kill-on-close stage remains held.
/// This value intentionally exposes no resume or detach method. A later typed
/// transition must first establish the real writer and I/O custody.
#[must_use]
pub(super) struct SuspendedCli {
    input: CliCreationInput,
    stage: StagingJob,
    process: Handle,
    thread: Handle,
    original: Arc<RetainedProcess>,
    resume_attempted: bool,
}
/// Internal ticket retains the same kernel process object; no serialized PID
/// or caller-provided value can construct a ticket for a different child.
pub(super) struct CliLaunchTicket {
    original: Arc<RetainedProcess>,
}
impl CliLaunchTicket {
    pub(super) fn original(&self) -> &RetainedProcess {
        &self.original
    }
    pub(super) fn original_arc(&self) -> Arc<RetainedProcess> {
        Arc::clone(&self.original)
    }
}
impl SuspendedCli {
    /// Revalidation uses the actual service-created process object and the
    /// original requester connection; neither a PID nor JSON can construct it.
    #[allow(unsafe_code)]
    pub(super) fn recheck(
        &self,
        host: &StartedServiceContext,
        peer: &Peer,
        pipe: &Pipe,
    ) -> Result<()> {
        if !std::ptr::eq(self.input.host.as_ref(), host)
            || unsafe {
                windows_sys::Win32::Foundation::CompareObjectHandles(
                    self.process.raw(),
                    self.original.raw(),
                )
            } == 0
            || self.original.exited()?
            || !same_requester(&self.original.fresh_token(None)?, &peer.process.token)
        {
            return Err(Error::PeerRefused);
        }
        if self.stage.verify_staged().is_err() {
            self.stage.verify_cli_guarded()?;
        }
        self.stage.verify_member(&self.process)?;
        self.input.recheck(peer, pipe)
    }

    pub(super) fn original(&self) -> &Arc<RetainedProcess> {
        &self.original
    }
    /// A connecting worker is accepted only as the held native child object,
    /// after durable custody prepared its kill-guarded staging state.
    #[allow(unsafe_code)]
    pub(super) fn recheck_worker(
        &self,
        host: &StartedServiceContext,
        process: &RetainedProcess,
    ) -> Result<()> {
        if !std::ptr::eq(self.input.host.as_ref(), host)
            || unsafe {
                windows_sys::Win32::Foundation::CompareObjectHandles(
                    self.process.raw(),
                    process.raw(),
                )
            } == 0
            || process.exited()?
            || !same_requester(&process.fresh_token(None)?, &self.original.token)
        {
            return Err(Error::PeerRefused);
        }
        self.stage.verify_cli_guarded()?;
        self.stage.verify_member(&self.process)?;
        self.input.recheck_detached()
    }
    pub(super) fn prepare_guarded_resume(
        &self,
        host: &StartedServiceContext,
        requester: &Peer,
        pipe: &Pipe,
        published: &super::registry::writers::PublishedAdmission<'_, '_>,
        job: &super::writer_job::WriterJob,
        ack: &super::writer_keeper::KeeperJobAck,
        keeper: &mut super::keeper_spawn::KeeperPeer<'_>,
    ) -> Result<()> {
        self.recheck(host, requester, pipe)?;
        self.stage
            .allow_admitted_cli_children(&self.process, published, job, ack, keeper)?;
        self.recheck(host, requester, pipe)
    }
    /// One attempt only; an ambiguous native result is retained as unknown.
    /// Caller must already own the real prepared data endpoint and proof.
    #[allow(unsafe_code)]
    pub(super) fn resume(
        &mut self,
        host: &StartedServiceContext,
        requester: &Peer,
        pipe: &Pipe,
        published: &super::registry::writers::PublishedAdmission<'_, '_>,
        job: &super::writer_job::WriterJob,
        ack: &super::writer_keeper::KeeperJobAck,
        keeper: &mut super::keeper_spawn::KeeperPeer<'_>,
    ) -> Result<()> {
        if self.resume_attempted {
            return Err(Error::HandoffOutcomeUnknown);
        }
        self.recheck(host, requester, pipe)?;
        self.stage.verify_cli_guarded()?;
        published.recheck(job, ack, keeper)?;
        self.resume_attempted = true;
        let previous =
            unsafe { windows_sys::Win32::System::Threading::ResumeThread(self.thread.raw()) };
        if previous != 1 {
            return Err(Error::HandoffOutcomeUnknown);
        }
        // A successful native return is the effect boundary. The worker may
        // exit immediately; its actual process handle remains retained.
        Ok(())
    }
}

// User-approved 32 MiB limit applies to UTF-16 bytes, before framing.
const MAX_ENVIRONMENT_UNITS: usize = crate::ipc::CLI_ENV_MAX_UNITS;
const MAX_ENVIRONMENT_IN_FLIGHT_BYTES: usize = 64 * 1024 * 1024;
static ENVIRONMENT_IN_FLIGHT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

struct EnvironmentPermit(usize);
impl EnvironmentPermit {
    fn acquire(units: usize) -> Result<Self> {
        use std::sync::atomic::Ordering;
        let bytes = units.checked_mul(2).ok_or(Error::IpcInvalid)?;
        ENVIRONMENT_IN_FLIGHT
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(bytes)
                    .filter(|next| *next <= MAX_ENVIRONMENT_IN_FLIGHT_BYTES)
            })
            .map_err(|_| Error::Busy)?;
        Ok(Self(bytes))
    }
}
impl Drop for EnvironmentPermit {
    fn drop(&mut self) {
        ENVIRONMENT_IN_FLIGHT.fetch_sub(self.0, std::sync::atomic::Ordering::AcqRel);
    }
}

/// The raw caller environment is data for its own user-token child only. It is
/// never persisted, logged, or interpreted by the privileged service.
pub(super) struct UserEnvironment(Vec<u16>, EnvironmentPermit);
impl UserEnvironment {
    #[cfg(test)]
    fn new(value: Vec<u16>) -> Result<Self> {
        let permit = EnvironmentPermit::acquire(value.len())?;
        Self::with_permit(value, permit)
    }
    fn with_permit(value: Vec<u16>, permit: EnvironmentPermit) -> Result<Self> {
        // Adopt first so all refusal paths also clear the received buffer.
        let block = Self(value, permit);
        if block.0.len() < 2 || block.0.len() > MAX_ENVIRONMENT_UNITS || !block.0.ends_with(&[0, 0])
        {
            return Err(Error::IpcInvalid);
        }
        if block.0 == [0, 0] {
            return Ok(block);
        }
        if block.0[0] == 0 {
            return Err(Error::IpcInvalid);
        }
        for entry in block.0[..block.0.len() - 1].split(|unit| *unit == 0) {
            if entry.is_empty() {
                continue;
            }
            let name = if entry[0] == u16::from(b'=') {
                &entry[1..]
            } else {
                entry
            };
            if !name
                .iter()
                .position(|unit| *unit == u16::from(b'='))
                .is_some_and(|i| i > 0)
            {
                return Err(Error::IpcInvalid);
            }
        }
        // An embedded double terminator would hide unvalidated trailing data.
        if block.0[..block.0.len() - 1]
            .windows(2)
            .any(|pair| pair == [0, 0])
        {
            return Err(Error::IpcInvalid);
        }
        Ok(block)
    }
    #[allow(unsafe_code)]
    fn clear(&mut self) {
        for unit in &mut self.0 {
            unsafe { std::ptr::write_volatile(unit, 0) };
        }
    }
}

/// One request's ordered UTF-16 environment bytes. Transport owns and must
/// separately clear each received frame; this owner clears its accumulated
/// buffer on every exit and exposes no formatting or serialization.
pub(super) struct EnvironmentReceiver {
    expected: usize,
    value: Vec<u16>,
    failed: bool,
    permit: Option<EnvironmentPermit>,
}
impl EnvironmentReceiver {
    pub(super) fn new(expected: usize) -> Result<Self> {
        if !(2..=MAX_ENVIRONMENT_UNITS).contains(&expected) {
            return Err(Error::IpcInvalid);
        }
        let permit = EnvironmentPermit::acquire(expected)?;
        let mut value = Vec::new();
        value
            .try_reserve_exact(expected)
            .map_err(|_| Error::Unavailable)?;
        Ok(Self {
            expected,
            value,
            failed: false,
            permit: Some(permit),
        })
    }
    fn push(&mut self, bytes: &[u8]) -> Result<()> {
        if self.failed
            || bytes.is_empty()
            || bytes.len() % 2 != 0
            || bytes.len() > crate::json::FRAME_MAX
            || self
                .value
                .len()
                .checked_add(bytes.len() / 2)
                .is_none_or(|n| n > self.expected)
        {
            self.failed = true;
            return Err(Error::IpcInvalid);
        }
        self.value.extend(
            bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]])),
        );
        Ok(())
    }
    fn finish(mut self) -> Result<UserEnvironment> {
        if self.failed || self.value.len() != self.expected {
            return Err(Error::IpcInvalid);
        }
        let permit = self.permit.take().ok_or(Error::StateCorrupt)?;
        UserEnvironment::with_permit(std::mem::take(&mut self.value), permit)
    }
    #[allow(unsafe_code)]
    fn clear(&mut self) {
        for unit in &mut self.value {
            unsafe { std::ptr::write_volatile(unit, 0) };
        }
    }
}
impl Drop for EnvironmentReceiver {
    fn drop(&mut self) {
        self.clear();
    }
}
impl Drop for UserEnvironment {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Snapshot the actual caller environment including hidden drive entries and
/// unpaired UTF-16 units; never format or log the returned data.
#[allow(unsafe_code)]
pub(super) fn capture_current_environment() -> Result<UserEnvironment> {
    use windows_sys::Win32::System::Environment::{
        FreeEnvironmentStringsW, GetEnvironmentStringsW,
    };
    struct NativeBlock(*mut u16);
    impl Drop for NativeBlock {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            unsafe { FreeEnvironmentStringsW(self.0) };
        }
    }
    let raw = unsafe { GetEnvironmentStringsW() };
    if raw.is_null() {
        return Err(Error::Unavailable);
    }
    let block = NativeBlock(raw);
    let mut length = 0usize;
    loop {
        if length >= MAX_ENVIRONMENT_UNITS {
            return Err(Error::IpcInvalid);
        }
        let value = unsafe { *block.0.add(length) };
        length += 1;
        if value == 0 && length >= 2 && unsafe { *block.0.add(length - 2) } == 0 {
            break;
        }
    }
    let permit = EnvironmentPermit::acquire(length)?;
    let mut value = Vec::new();
    value
        .try_reserve_exact(length)
        .map_err(|_| Error::Unavailable)?;
    value.extend_from_slice(unsafe { std::slice::from_raw_parts(block.0, length) });
    UserEnvironment::with_permit(value, permit)
}

pub(super) fn send_environment(
    client: &super::client::Client,
    block: &UserEnvironment,
) -> Result<()> {
    client.send_sensitive_units(&block.0)
}
impl UserEnvironment {
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }
}

/// Create and admit the native writer in one lifetime. The returned process
/// stays suspended until the host resumes it with its authenticated frontend
/// endpoint; the worker attaches its I/O after startup.
pub(super) fn prepare_suspended_writer(
    host: &Arc<StartedServiceContext>,
    coordinator: &super::writer_coordinator::WriterCoordinator,
    requester: &Peer,
    pipe: &Pipe,
    args: &[String],
    cwd: &std::path::Path,
    environment: UserEnvironment,
) -> Result<CliLaunchTicket> {
    let input = CliCreationInput::acquire(host, requester, pipe)?;
    let child = create_suspended_without_io(input, requester, pipe, args, cwd, environment)?;
    let ticket = CliLaunchTicket {
        original: Arc::clone(child.original()),
    };
    // On every failure the stage closes and kills the still-suspended child;
    // the coordinator retains any uncertain journal/job/keeper custody.
    coordinator
        .admit_spawned(child, requester, pipe)
        .map_err(|failure| failure.error)?;
    if let Err(failure) = coordinator.prepare_spawned_resume(&ticket, requester, pipe) {
        // Keep the protected writer state for reconciliation. The kill-on-
        // close stage ends the still-suspended worker; it is never retried.
        coordinator
            .abort_spawned_setup(&ticket)
            .map_err(|_| Error::HandoffOutcomeUnknown)?;
        return Err(failure.error);
    }
    Ok(ticket)
}

/// Read only from the same authenticated pipe and held requester process.
/// A disconnect, timeout, malformed chunk or process change destroys all
/// partial data without creating a child or publishing a writer intent.
pub(super) fn receive_environment(
    requester: &Peer,
    pipe: &Pipe,
    stop: &Event,
    mut receiver: EnvironmentReceiver,
) -> Result<UserEnvironment> {
    while receiver.value.len() < receiver.expected {
        requester.recheck(pipe)?;
        let frame = pipe.read_sensitive_frame(stop, requester.process.raw())?;
        requester.recheck(pipe)?;
        receiver.push(frame.bytes())?;
    }
    requester.recheck(pipe)?;
    receiver.finish()
}

/// Creates a suspended child under the requester's actual primary token.
/// This function neither inherits I/O handles nor resumes the child; the
/// worker attaches its standard streams after the host resumes it.
#[allow(unsafe_code)]
fn create_suspended_without_io(
    input: CliCreationInput,
    peer: &Peer,
    pipe: &Pipe,
    args: &[String],
    cwd: &std::path::Path,
    environment: UserEnvironment,
) -> Result<SuspendedCli> {
    use std::{mem::size_of, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::Foundation::CompareObjectHandles;
    use windows_sys::Win32::System::Threading::{
        CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW,
        EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION, STARTUPINFOEXW,
    };

    input.recheck(peer, pipe)?;
    if !cwd.is_absolute() {
        return Err(Error::IpcInvalid);
    }
    let mut cwd: Vec<u16> = cwd.as_os_str().encode_wide().collect();
    if cwd.contains(&0) || cwd.len() >= 32767 {
        return Err(Error::IpcInvalid);
    }
    cwd.push(0);
    let image = input.host.context.layout.app.join("pinvou.exe");
    let application: Vec<u16> = image
        .to_str()
        .ok_or(Error::ImageRefused)?
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut command = command_line(&image, args)?;
    let stage = StagingJob::create()?;
    let attributes = JobOnlyAttributes::new(&stage)?;
    let process_security = NativeSecurity::host_process()?;
    let security = process_security.attributes();
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.lpAttributeList = attributes.raw();
    let mut information = PROCESS_INFORMATION::default();
    let privileges = ScopedCreationPrivileges::enter(&input, peer, pipe)?;
    let result = unsafe {
        CreateProcessAsUserW(
            input.token.raw(),
            application.as_ptr(),
            command.as_mut_ptr(),
            &security,
            &security,
            0,
            CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            environment.0.as_ptr().cast(),
            cwd.as_ptr(),
            &startup.StartupInfo,
            &mut information,
        )
    };
    drop(privileges);
    drop(attributes);
    if result == 0 {
        return Err(Error::HandoffOutcomeUnknown);
    }
    // Adopt both returned handles before evaluating either conversion, so a
    // malformed native result cannot leak the other one on early return.
    let process = Handle::owned(information.hProcess);
    let thread = Handle::owned(information.hThread);
    let process = process?;
    let thread = thread?;
    stage.verify_member(&process)?;
    stage.verify_staged()?;
    let original = Arc::new(RetainedProcess::open(information.dwProcessId, None)?);
    if unsafe { CompareObjectHandles(process.raw(), original.raw()) } == 0
        || !same_requester(&original.token, &peer.process.token)
        || original.exited()?
    {
        return Err(Error::PeerRefused);
    }
    input.recheck(peer, pipe)?;
    Ok(SuspendedCli {
        input,
        stage,
        process,
        thread,
        original,
        resume_attempted: false,
    })
}

/// Build the exact Windows argument vector for the fixed image, without a shell
/// or search path. Arguments are user data; they do not confer authority.
fn command_line(path: &std::path::Path, args: &[String]) -> Result<Vec<u16>> {
    let image = path.to_str().ok_or(Error::IpcInvalid)?;
    if image.contains('\0') || args.iter().any(|arg| arg.contains('\0')) {
        return Err(Error::IpcInvalid);
    }
    let mut command = super::launch::quote(image);
    for argument in args {
        command.push(' ');
        command.push_str(&super::launch::quote(argument));
        if command.encode_utf16().count() >= 32767 {
            return Err(Error::IpcInvalid);
        }
    }
    let mut result: Vec<u16> = command.encode_utf16().collect();
    if result.len() >= 32767 {
        return Err(Error::IpcInvalid);
    }
    result.push(0);
    Ok(result)
}
pub(super) fn validate_arguments(host: &StartedServiceContext, args: &[String]) -> Result<()> {
    command_line(&host.context.layout.app.join("pinvou.exe"), args).map(|_| ())
}

/// Both objects come from the live service and peer, and remain held for the
/// caller's creation boundary. No Serialize/Clone/IPC constructor is provided.
pub(super) struct CliCreationInput {
    host: Arc<StartedServiceContext>,
    requester: Arc<RetainedProcess>,
    token: RequesterToken,
    file: File,
    identity: Identity,
    expected: AppFile,
}
impl CliCreationInput {
    pub(super) fn acquire(
        host: &Arc<StartedServiceContext>,
        peer: &Peer,
        pipe: &Pipe,
    ) -> Result<Self> {
        let token = RequesterToken::capture(host, peer, pipe)?;
        let expected = host
            .context
            .manifest
            .app_files
            .iter()
            .find(|entry| entry.relative_name == "pinvou.exe")
            .ok_or(Error::ImageRefused)?
            .clone();
        let mut file = host.context.targets.app.relative_file("pinvou.exe")?;
        let identity = verify_bytes(&mut file, expected.size, &expected.sha256)?;
        let value = Self {
            host: Arc::clone(host),
            requester: Arc::clone(&peer.process),
            token,
            file,
            identity,
            expected,
        };
        value.recheck(peer, pipe)?;
        Ok(value)
    }

    /// Recheck both the held bytes and the protected current pathname while
    /// retaining the original identity. A later process must be created while
    /// this exact value remains alive and be identified by its creation handle.
    #[allow(unsafe_code)]
    pub(super) fn recheck(&self, peer: &Peer, pipe: &Pipe) -> Result<()> {
        peer.recheck(pipe)?;
        if self.requester.exited()?
            || unsafe {
                windows_sys::Win32::Foundation::CompareObjectHandles(
                    self.requester.raw(),
                    peer.process.raw(),
                )
            } == 0
        {
            return Err(Error::PeerRefused);
        }
        self.recheck_detached()?;
        peer.recheck(pipe)
    }

    fn recheck_detached(&self) -> Result<()> {
        if self.requester.exited()?
            || !same_requester(&self.requester.fresh_token(None)?, &self.requester.token)
        {
            return Err(Error::PeerRefused);
        }
        let actual = Store::new(&self.host.context.seed)
            .read(&self.host.context.seed)?
            .ok_or(Error::SetupIncomplete)?;
        self.host.authenticate_running(&actual)?;
        let mut held = self.file.try_clone().map_err(|_| Error::ImageRefused)?;
        let mut current = self.host.context.targets.app.relative_file("pinvou.exe")?;
        if verify_bytes(&mut held, self.expected.size, &self.expected.sha256)? != self.identity
            || verify_bytes(&mut current, self.expected.size, &self.expected.sha256)?
                != self.identity
        {
            return Err(Error::ImageRefused);
        }
        self.host.authenticate_running(&actual)
    }

    pub(super) fn user_token(&self) -> &RequesterToken {
        &self.token
    }
    pub(super) fn host(&self) -> &StartedServiceContext {
        self.host.as_ref()
    }
    pub(super) fn image_identity(&self) -> &Identity {
        &self.identity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::Path};
    use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};
    static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn environment_block_preserves_unicode_units_and_rejects_invalid_structure() {
        let _guard = ENV_TEST_LOCK.lock().unwrap();
        let raw: Vec<u16> = "NAME=value\0=C:=C:\\work\0\u{4e2d}=x\0\0"
            .encode_utf16()
            .collect();
        let value = UserEnvironment::new(raw.clone()).unwrap();
        assert_eq!(value.0, raw);
        assert!(UserEnvironment::new(vec![0, 0]).is_ok());
        for bad in [
            vec![],
            vec![0],
            vec![0, 0, 0],
            vec![b'X' as u16, 0, 0],
            vec![b'X' as u16, b'=' as u16, 0, 0, b'Z' as u16, 0, 0],
        ] {
            assert!(UserEnvironment::new(bad).is_err());
        }
        assert!(UserEnvironment::new(vec![0; MAX_ENVIRONMENT_UNITS + 1]).is_err());
    }

    #[test]
    fn environment_receiver_requires_exact_ordered_bounded_units() {
        let _guard = ENV_TEST_LOCK.lock().unwrap();
        let mut receiver = EnvironmentReceiver::new(5).unwrap();
        receiver.push(&[b'X', 0, b'=', 0]).unwrap();
        receiver.push(&[b'Y', 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(receiver.finish().unwrap().0, vec![88, 61, 89, 0, 0]);
        let mut receiver = EnvironmentReceiver::new(2).unwrap();
        assert!(receiver.push(&[1]).is_err());
        assert!(receiver.push(&[0; 6]).is_err());
        assert!(receiver.finish().is_err());
        assert!(EnvironmentReceiver::new(MAX_ENVIRONMENT_UNITS + 1).is_err());
    }

    #[test]
    fn environment_accepts_exact_32_mib_and_refuses_larger() {
        let _guard = ENV_TEST_LOCK.lock().unwrap();
        let mut maximum = vec![u16::from(b'x'); MAX_ENVIRONMENT_UNITS];
        maximum[1] = u16::from(b'=');
        maximum[MAX_ENVIRONMENT_UNITS - 2..].fill(0);
        maximum[2] = 0xd800; // Raw unpaired UTF-16 unit is preserved.
        let mut receiver = EnvironmentReceiver::new(MAX_ENVIRONMENT_UNITS).unwrap();
        let mut frames = 0usize;
        super::super::pipe::for_sensitive_frames(&maximum, |bytes| {
            assert!(bytes.len() <= crate::json::FRAME_MAX);
            frames += 1;
            receiver.push(bytes)
        })
        .unwrap();
        assert_eq!(frames, 512);
        assert!(receiver.finish().unwrap().0 == maximum);
        assert!(EnvironmentReceiver::new(MAX_ENVIRONMENT_UNITS + 1).is_err());
        maximum.push(0);
        assert!(super::super::pipe::for_sensitive_frames(&maximum, |_| Ok(())).is_err());
    }

    #[test]
    fn environment_budget_concurrent_busy_and_interrupted_owner_release() {
        use std::sync::atomic::Ordering;
        let _guard = ENV_TEST_LOCK.lock().unwrap();
        let base = ENVIRONMENT_IN_FLIGHT.load(Ordering::Acquire);
        let first = EnvironmentPermit::acquire(MAX_ENVIRONMENT_UNITS).unwrap();
        let second = EnvironmentPermit::acquire(MAX_ENVIRONMENT_UNITS).unwrap();
        let third = std::thread::spawn(|| EnvironmentPermit::acquire(1));
        assert!(matches!(third.join().unwrap(), Err(Error::Busy)));
        drop(first);
        let replacement = EnvironmentPermit::acquire(MAX_ENVIRONMENT_UNITS).unwrap();
        drop(replacement);
        drop(second);
        assert_eq!(ENVIRONMENT_IN_FLIGHT.load(Ordering::Acquire), base);

        let mut interrupted = EnvironmentReceiver::new(8).unwrap();
        interrupted.push(&[1, 0, 2, 0]).unwrap();
        interrupted.clear();
        assert!(interrupted.value.iter().all(|unit| *unit == 0));
        drop(interrupted); // Equivalent owner drop on connection failure.
        assert_eq!(ENVIRONMENT_IN_FLIGHT.load(Ordering::Acquire), base);
    }

    #[test]
    #[allow(unsafe_code)]
    fn fixed_cli_command_round_trips_windows_argument_parsing() {
        let path = Path::new(r"C:\Program Files\Pinvou\pinvou.exe");
        let args = vec![
            "".into(),
            "two words".into(),
            "quote\"here".into(),
            r"tail\".into(),
            "中文".into(),
        ];
        let command = command_line(path, &args).unwrap();
        let mut count = 0;
        let parsed = unsafe { CommandLineToArgvW(command.as_ptr(), &mut count) };
        assert!(!parsed.is_null());
        let mut actual = Vec::new();
        for index in 0..count as usize {
            let word = unsafe { *parsed.add(index) };
            let mut length = 0usize;
            while unsafe { *word.add(length) } != 0 {
                length += 1;
            }
            let value = OsString::from_wide(unsafe { std::slice::from_raw_parts(word, length) });
            actual.push(value.to_string_lossy().into_owned());
        }
        unsafe {
            LocalFree(parsed.cast());
        }
        assert_eq!(actual[0], path.to_str().unwrap());
        assert_eq!(&actual[1..], args.as_slice());
        assert!(command_line(path, &["a\0b".into()]).is_err());
        assert!(command_line(path, &["a".repeat(32767)]).is_err());
    }
}
