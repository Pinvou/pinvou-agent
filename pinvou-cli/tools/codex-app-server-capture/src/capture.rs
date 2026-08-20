use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::clock::{HostMonotonicClock, MonotonicClock};
use crate::protocol::{CaptureChannel, CaptureRecord};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub program: OsString,
    pub args: Vec<OsString>,
}

impl CommandSpec {
    pub fn new(program: OsString, args: impl IntoIterator<Item = impl Into<OsString>>) -> Self {
        Self {
            program,
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    pub fn codex(executable: Option<OsString>) -> Self {
        Self::new(
            executable.unwrap_or_else(|| OsString::from("codex")),
            ["app-server", "--stdio"],
        )
    }
}

#[derive(Clone, Debug)]
pub struct CaptureConfig {
    pub input: PathBuf,
    pub output: PathBuf,
    pub command: CommandSpec,
}

#[derive(Clone, Debug)]
pub struct ProxyConfig {
    pub output: PathBuf,
    pub command: CommandSpec,
}

pub fn parse_client_jsonl(input: &str) -> Result<Vec<String>> {
    input
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            let frame: serde_json::Value = serde_json::from_str(line).map_err(|error| {
                anyhow::anyhow!("invalid JSON on input line {}: {error}", index + 1)
            })?;
            if !frame.is_object() {
                bail!("input line {} must be a JSON object", index + 1);
            }
            Ok(line.trim_end_matches('\r').to_owned())
        })
        .collect()
}

pub fn read_lines<R, F>(reader: R, mut on_line: F) -> Result<()>
where
    R: Read,
    F: FnMut(&str) -> Result<()>,
{
    let mut reader = BufReader::new(reader);
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        if reader.read_until(b'\n', &mut bytes)? == 0 {
            return Ok(());
        }
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        let line = String::from_utf8_lossy(&bytes);
        on_line(&line)?;
    }
}

pub fn run_capture(config: CaptureConfig) -> Result<()> {
    ensure_distinct_files(&config.input, &config.output)?;
    let input = File::open(&config.input)
        .with_context(|| format!("failed to open input {}", config.input.display()))?;
    run_driver(
        ProxyConfig {
            output: config.output,
            command: config.command,
        },
        input,
        std::io::sink(),
        false,
    )
}

pub fn run_proxy<R, W>(config: ProxyConfig, client_reader: R, server_writer: W) -> Result<()>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    run_driver(config, client_reader, server_writer, true)
}

fn run_driver<R, W>(
    config: ProxyConfig,
    client_reader: R,
    mut server_writer: W,
    forward_stdout: bool,
) -> Result<()>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    let output = create_capture_file(&config.output)?;
    let clock = Arc::new(HostMonotonicClock::new()?);
    let recorder_clock = Arc::clone(&clock);
    let recorder = Arc::new(Mutex::new(JsonlRecorder::new(
        BufWriter::new(output),
        move || {
            recorder_clock
                .now_ns()
                .expect("initialized host monotonic clock failed")
        },
    )));

    let mut child = Command::new(&config.command.program)
        .args(&config.command.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "failed to launch app-server executable {:?}",
                config.command.program
            )
        })?;
    let stdout = child.stdout.take().context("child stdout was not piped")?;
    let stderr = child.stderr.take().context("child stderr was not piped")?;
    let child_stdin = child.stdin.take().context("child stdin was not piped")?;
    let (event_tx, event_rx) = mpsc::sync_channel(64);

    let stdout_recorder = Arc::clone(&recorder);
    let stdout_events = event_tx.clone();
    let stdout_thread = thread::spawn(move || {
        let result = read_lines(stdout, |line| {
            if forward_stdout {
                record_and_forward_server_frame(stdout_recorder.as_ref(), &mut server_writer, line)
            } else {
                record_server_frame(stdout_recorder.as_ref(), line)
            }
        });
        let _ = stdout_events.send(DriverEvent::StdoutDone(
            result.as_ref().err().map(ToString::to_string),
        ));
        result
    });

    let stderr_recorder = Arc::clone(&recorder);
    let stderr_events = event_tx.clone();
    let stderr_thread = thread::spawn(move || {
        let result = read_lines(stderr, |line| {
            stderr_recorder
                .lock()
                .map_err(|_| anyhow::anyhow!("capture recorder lock poisoned"))?
                .record(CaptureChannel::Stderr, line)
        });
        let _ = stderr_events.send(DriverEvent::StderrDone(
            result.as_ref().err().map(ToString::to_string),
        ));
        result
    });

    let client_events = event_tx.clone();
    let _client_thread = thread::spawn(move || {
        let result = read_lines(client_reader, |frame| {
            client_events
                .send(DriverEvent::ClientFrame(frame.to_owned()))
                .map_err(|_| anyhow::anyhow!("capture supervisor stopped"))
        });
        let _ = client_events.send(DriverEvent::ClientDone(
            result.as_ref().err().map(ToString::to_string),
        ));
    });
    drop(event_tx);

    let mut child_stdin = Some(child_stdin);
    let mut client_done = false;
    let mut client_error = None;
    let mut supervision_error = None;
    let mut stdout_end_deadline = None;

    let status = 'supervise: loop {
        if let Some(status) = child.try_wait().context("failed polling app-server")? {
            break status;
        }
        if stdout_end_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            supervision_error = Some(anyhow::anyhow!(
                "app-server stdout ended while the child remained running"
            ));
            let _ = child.kill();
            break 'supervise child.wait().context("failed waiting for app-server")?;
        }

        match event_rx.recv_timeout(Duration::from_millis(10)) {
            Ok(DriverEvent::ClientFrame(frame)) => {
                if frame.trim().is_empty() {
                    continue;
                }
                let send_result = (|| {
                    let value: serde_json::Value = serde_json::from_str(&frame)
                        .with_context(|| "client stdin contained a non-JSON protocol line")?;
                    if !value.is_object() {
                        bail!("client stdin protocol frame must be a JSON object");
                    }
                    let stdin = child_stdin
                        .as_mut()
                        .context("app-server stdin closed before client input completed")?;
                    record_and_send_client_frame(recorder.as_ref(), stdin, &frame)
                })();
                if let Err(error) = send_result {
                    supervision_error = Some(error);
                    let _ = child.kill();
                    break 'supervise child.wait().context("failed waiting for app-server")?;
                }
            }
            Ok(DriverEvent::ClientDone(error)) => {
                client_done = true;
                client_error = error;
                child_stdin.take();
                if client_error.is_some() {
                    let _ = child.kill();
                    break 'supervise child.wait().context("failed waiting for app-server")?;
                }
            }
            Ok(DriverEvent::StdoutDone(error)) => {
                if let Some(error) = error {
                    supervision_error = Some(anyhow::anyhow!(error));
                    let _ = child.kill();
                    break 'supervise child.wait().context("failed waiting for app-server")?;
                }
                stdout_end_deadline = Some(Instant::now() + Duration::from_millis(100));
            }
            Ok(DriverEvent::StderrDone(Some(error))) => {
                supervision_error = Some(anyhow::anyhow!(error));
                let _ = child.kill();
                break 'supervise child.wait().context("failed waiting for app-server")?;
            }
            Ok(DriverEvent::StderrDone(None))
            | Err(mpsc::RecvTimeoutError::Timeout)
            | Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }
    };
    child_stdin.take();

    stdout_thread
        .join()
        .map_err(|_| anyhow::anyhow!("stdout capture thread panicked"))??;
    stderr_thread
        .join()
        .map_err(|_| anyhow::anyhow!("stderr capture thread panicked"))??;
    let mut has_unsent_client_frames = false;
    while let Ok(event) = event_rx.try_recv() {
        match event {
            DriverEvent::ClientFrame(_) => has_unsent_client_frames = true,
            DriverEvent::ClientDone(error) => {
                client_done = true;
                client_error = error;
            }
            DriverEvent::StdoutDone(_) | DriverEvent::StderrDone(_) => {}
        }
    }
    if let Some(error) = supervision_error {
        return Err(error);
    }
    if has_unsent_client_frames {
        bail!("app-server exited with an unsent client frame still queued");
    }
    if let Some(error) = client_error {
        bail!("client input failed: {error}");
    }
    if !status.success() {
        bail!("app-server exited with status {status}");
    }
    if !client_done {
        bail!("app-server exited before client input completed");
    }
    Ok(())
}

enum DriverEvent {
    ClientFrame(String),
    ClientDone(Option<String>),
    StdoutDone(Option<String>),
    StderrDone(Option<String>),
}

fn ensure_distinct_files(input: &Path, output: &Path) -> Result<()> {
    let canonical_input = input
        .canonicalize()
        .with_context(|| format!("failed to resolve input {}", input.display()))?;
    let canonical_output = if output.exists() {
        Some(
            output
                .canonicalize()
                .with_context(|| format!("failed to resolve output {}", output.display()))?,
        )
    } else {
        output
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .and_then(|parent| parent.canonicalize().ok())
            .and_then(|parent| output.file_name().map(|name| parent.join(name)))
    };
    if canonical_output.as_ref() == Some(&canonical_input) {
        bail!("replay input and capture output resolve to the same file");
    }

    if output.exists() && files_have_same_identity(input, output)? {
        bail!("replay input and capture output identify the same file");
    }
    Ok(())
}

#[cfg(unix)]
fn files_have_same_identity(input: &Path, output: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let input_metadata = std::fs::metadata(input)?;
    let output_metadata = std::fs::metadata(output)?;
    Ok(input_metadata.dev() == output_metadata.dev()
        && input_metadata.ino() == output_metadata.ino())
}

#[cfg(windows)]
fn files_have_same_identity(input: &Path, output: &Path) -> Result<bool> {
    fn identity(file: &File) -> Result<(u32, u64)> {
        use std::mem::MaybeUninit;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };

        let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
        // SAFETY: the file handle is live and `information` points to writable storage.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) }
            == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: a successful API call initialized the complete structure.
        let information = unsafe { information.assume_init() };
        let file_index =
            ((information.nFileIndexHigh as u64) << 32) | information.nFileIndexLow as u64;
        Ok((information.dwVolumeSerialNumber, file_index))
    }

    let input_file = File::open(input)?;
    let output_file = File::open(output)?;
    Ok(identity(&input_file)? == identity(&output_file)?)
}

pub(crate) fn create_capture_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Foundation::GENERIC_WRITE;
        use windows_sys::Win32::Storage::FileSystem::{READ_CONTROL, WRITE_DAC};

        // ACL replacement is bound to this same handle, so request the rights
        // needed by GetSecurityInfo/SetSecurityInfo when the file is opened.
        options.access_mode(GENERIC_WRITE | READ_CONTROL | WRITE_DAC);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        options.mode(0o600);
        let file = options
            .open(path)
            .with_context(|| format!("failed to create capture {}", path.display()))?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        return Ok(file);
    }
    #[cfg(not(unix))]
    {
        let file = options
            .open(path)
            .with_context(|| format!("failed to create capture {}", path.display()))?;
        #[cfg(windows)]
        if let Err(error) = harden_windows_handle_acl(&file) {
            drop(file);
            let _ = std::fs::remove_file(path);
            return Err(error)
                .with_context(|| format!("failed to restrict capture ACL for {}", path.display()));
        }
        Ok(file)
    }
}

#[cfg(windows)]
pub(crate) fn harden_windows_handle_acl(file: &File) -> Result<()> {
    use std::os::windows::io::AsRawHandle;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, GetSecurityInfo, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW,
        SetSecurityInfo, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;

    let handle = file.as_raw_handle();
    let mut owner: PSID = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: the file handle is live and all output pointers are valid.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        bail!("GetSecurityInfo failed with {status}");
    }

    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_ALL_ACCESS,
        grfAccessMode: SET_ACCESS,
        grfInheritance: 0,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: owner.cast(),
        },
    };
    let mut acl: *mut ACL = null_mut();
    // SAFETY: `entry` and `acl` are valid for the call; the owner SID is owned by descriptor.
    let acl_status = unsafe { SetEntriesInAclW(1, &entry, null(), &mut acl) };
    if acl_status != ERROR_SUCCESS {
        // SAFETY: descriptor was allocated by GetNamedSecurityInfoW.
        unsafe { LocalFree(descriptor.cast()) };
        bail!("SetEntriesInAclW failed with {acl_status}");
    }
    // SAFETY: the handle and ACL are valid; null owner/group/SACL leave those fields unchanged.
    let set_status = unsafe {
        SetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            acl,
            null(),
        )
    };
    // SAFETY: both allocations are returned by Win32 local-allocation APIs.
    unsafe {
        LocalFree(acl.cast());
        LocalFree(descriptor.cast());
    }
    if set_status != ERROR_SUCCESS {
        bail!("SetSecurityInfo failed with {set_status}");
    }
    Ok(())
}

fn record_and_send_client_frame<RW, C, SW>(
    recorder: &Mutex<JsonlRecorder<RW, C>>,
    sink: &mut SW,
    frame: &str,
) -> Result<()>
where
    RW: Write,
    C: FnMut() -> u64,
    SW: Write,
{
    let mut recorder_guard = recorder
        .lock()
        .map_err(|_| anyhow::anyhow!("capture recorder lock poisoned"))?;
    recorder_guard.record(CaptureChannel::ClientToServer, frame)?;
    drop(recorder_guard);

    writeln!(sink, "{frame}").context("failed to write app-server stdin")?;
    sink.flush().context("failed to flush app-server stdin")?;
    Ok(())
}

fn record_server_frame<RW, C>(recorder: &Mutex<JsonlRecorder<RW, C>>, frame: &str) -> Result<()>
where
    RW: Write,
    C: FnMut() -> u64,
{
    let mut recorder_guard = recorder
        .lock()
        .map_err(|_| anyhow::anyhow!("capture recorder lock poisoned"))?;
    recorder_guard.record(CaptureChannel::ServerToClient, frame)?;
    drop(recorder_guard);

    let value: serde_json::Value = serde_json::from_str(frame)
        .with_context(|| "app-server stdout contained a non-JSON protocol line")?;
    if !value.is_object() {
        bail!("app-server stdout protocol frame must be a JSON object");
    }
    Ok(())
}

fn record_and_forward_server_frame<RW, C, SW>(
    recorder: &Mutex<JsonlRecorder<RW, C>>,
    sink: &mut SW,
    frame: &str,
) -> Result<()>
where
    RW: Write,
    C: FnMut() -> u64,
    SW: Write,
{
    record_server_frame(recorder, frame)?;
    writeln!(sink, "{frame}").context("failed to write proxy stdout")?;
    sink.flush().context("failed to flush proxy stdout")?;
    Ok(())
}

pub struct JsonlRecorder<W, C> {
    writer: W,
    clock: C,
}

impl<W, C> JsonlRecorder<W, C>
where
    W: Write,
    C: FnMut() -> u64,
{
    pub fn new(writer: W, clock: C) -> Self {
        Self { writer, clock }
    }

    pub fn record(&mut self, channel: CaptureChannel, line: &str) -> Result<()> {
        self.record_timestamped(channel, line).map(|_| ())
    }

    pub fn record_timestamped(&mut self, channel: CaptureChannel, line: &str) -> Result<u64> {
        if line.contains(['\r', '\n']) {
            bail!("capture payload must be a single line");
        }
        let monotonic_ns = (self.clock)();
        serde_json::to_writer(
            &mut self.writer,
            &CaptureRecord {
                monotonic_ns,
                channel,
                line: line.to_owned(),
            },
        )?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(monotonic_ns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn capture_acl_is_owner_only(file: &File) -> Result<bool> {
        use std::os::windows::io::AsRawHandle;
        use std::ptr::null_mut;
        use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
        use windows_sys::Win32::Security::Authorization::{
            EXPLICIT_ACCESS_W, GRANT_ACCESS, GetExplicitEntriesFromAclW, GetSecurityInfo,
            SE_FILE_OBJECT, TRUSTEE_IS_SID,
        };
        use windows_sys::Win32::Security::{
            ACL, DACL_SECURITY_INFORMATION, EqualSid, OWNER_SECURITY_INFORMATION,
            PSECURITY_DESCRIPTOR, PSID,
        };
        use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;

        let mut owner: PSID = null_mut();
        let mut acl: *mut ACL = null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: the file handle is live and all output pointers are valid.
        let status = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut acl,
                null_mut(),
                &mut descriptor,
            )
        };
        if status != ERROR_SUCCESS {
            bail!("GetSecurityInfo failed with {status}");
        }
        let mut count = 0_u32;
        let mut entries: *mut EXPLICIT_ACCESS_W = null_mut();
        // SAFETY: ACL is owned by descriptor and output pointers are valid.
        let entries_status = unsafe { GetExplicitEntriesFromAclW(acl, &mut count, &mut entries) };
        let valid = if entries_status == ERROR_SUCCESS && count == 1 && !entries.is_null() {
            // SAFETY: Win32 returned one initialized entry.
            let entry = unsafe { &*entries };
            // SetEntriesInAclW normalizes SET_ACCESS input into an effective
            // GRANT_ACCESS entry when enumerated back from the resulting ACL.
            entry.grfAccessMode == GRANT_ACCESS
                && entry.grfAccessPermissions == FILE_ALL_ACCESS
                && entry.grfInheritance == 0
                && entry.Trustee.TrusteeForm == TRUSTEE_IS_SID
                // SAFETY: both SIDs remain live until descriptor/entries are freed below.
                && unsafe { EqualSid(owner, entry.Trustee.ptstrName.cast()) } != 0
        } else {
            false
        };
        // SAFETY: allocations are returned by Win32 local-allocation APIs.
        unsafe {
            if !entries.is_null() {
                LocalFree(entries.cast());
            }
            LocalFree(descriptor.cast());
        }
        Ok(valid)
    }

    #[cfg(windows)]
    #[test]
    fn raw_capture_windows_acl_grants_only_the_file_owner() {
        let path = std::env::temp_dir().join(format!(
            "codex-capture-acl-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let file = create_capture_file(&path).unwrap();
        assert!(capture_acl_is_owner_only(&file).unwrap());
        drop(file);
        std::fs::remove_file(path).unwrap();
    }

    struct LockCheckingWriter<'a, W, C> {
        recorder: &'a Mutex<JsonlRecorder<W, C>>,
        write_saw_unlocked_recorder: bool,
        flush_saw_unlocked_recorder: bool,
    }

    impl<W, C> Write for LockCheckingWriter<'_, W, C> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.write_saw_unlocked_recorder = self.recorder.try_lock().is_ok();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.flush_saw_unlocked_recorder = self.recorder.try_lock().is_ok();
            Ok(())
        }
    }

    #[test]
    fn client_sink_write_and_flush_happen_after_recorder_lock_is_released() {
        let recorder = Mutex::new(JsonlRecorder::new(Vec::new(), || 1));
        let mut sink = LockCheckingWriter {
            recorder: &recorder,
            write_saw_unlocked_recorder: false,
            flush_saw_unlocked_recorder: false,
        };

        record_and_send_client_frame(&recorder, &mut sink, r#"{"id":1}"#).unwrap();

        assert!(sink.write_saw_unlocked_recorder);
        assert!(sink.flush_saw_unlocked_recorder);
    }

    #[test]
    fn server_sink_write_and_flush_happen_after_recorder_lock_is_released() {
        let recorder = Mutex::new(JsonlRecorder::new(Vec::new(), || 1));
        let mut sink = LockCheckingWriter {
            recorder: &recorder,
            write_saw_unlocked_recorder: false,
            flush_saw_unlocked_recorder: false,
        };

        record_and_forward_server_frame(&recorder, &mut sink, r#"{"id":1}"#).unwrap();

        assert!(sink.write_saw_unlocked_recorder);
        assert!(sink.flush_saw_unlocked_recorder);
    }
}
