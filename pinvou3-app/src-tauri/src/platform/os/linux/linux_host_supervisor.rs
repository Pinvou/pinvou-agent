use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use pinvou_host_supervisor_protocol::{
    SupervisorReceipt, SupervisorRequest, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES,
};
use wait_timeout::ChildExt;

use crate::platform::host_supervisor::HostSupervisorError;

const SOCKET_RELATIVE_PATH: &str = "pinvou-supervisor/control.sock";
const SOCKET_UNIT: &str = "pinvou3-supervisor.socket";
const SUPERVISOR_UNIT: &str = "pinvou3-supervisor.service";
const REQUEST_BUDGET: Duration = Duration::from_secs(8);
const ACTIVATE_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_RETRIES: usize = 30;
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(50);

pub(crate) fn host_supervisor_request(
    request: &SupervisorRequest,
) -> Result<SupervisorReceipt, HostSupervisorError> {
    let deadline = Instant::now()
        .checked_add(REQUEST_BUDGET)
        .ok_or_else(|| HostSupervisorError::Unavailable("request deadline overflow".to_string()))?;
    host_supervisor_request_until(request, deadline)
}

fn host_supervisor_request_until(
    request: &SupervisorRequest,
    deadline: Instant,
) -> Result<SupervisorReceipt, HostSupervisorError> {
    let socket_path = supervisor_socket_path()?;
    match request_at(&socket_path, request, deadline) {
        Ok(receipt) => Ok(receipt),
        Err(first_error) => {
            activate_socket_unit(deadline).map_err(|activation_error| {
                HostSupervisorError::Unavailable(format!(
                    "connect failed ({first_error}); socket activation failed ({activation_error})"
                ))
            })?;
            retry_request_until_with(
                deadline,
                Some(first_error),
                |request_deadline| request_at(&socket_path, request, request_deadline),
                Instant::now,
                thread::sleep,
            )
        }
    }
}

fn supervisor_socket_path() -> Result<PathBuf, HostSupervisorError> {
    let uid = unsafe { libc::geteuid() };
    let runtime = PathBuf::from(format!("/run/user/{uid}"));
    let metadata = std::fs::symlink_metadata(&runtime).map_err(|error| {
        HostSupervisorError::Unavailable(format!("inspect fixed runtime directory: {error}"))
    })?;
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != uid
        || metadata.mode() & 0o022 != 0
    {
        return Err(HostSupervisorError::Protocol(
            "fixed /run/user/<uid> directory is not trusted".to_string(),
        ));
    }
    Ok(runtime.join(SOCKET_RELATIVE_PATH))
}

fn request_at(
    socket_path: &Path,
    request: &SupervisorRequest,
    deadline: Instant,
) -> Result<SupervisorReceipt, HostSupervisorError> {
    let mut stream = connect_with_deadline(socket_path, deadline).map_err(|error| {
        HostSupervisorError::Unavailable(format!("connect {}: {error}", socket_path.display()))
    })?;
    set_close_on_exec(stream.as_raw_fd())?;
    verify_listener_uid(&stream)?;
    enable_passcred(&stream)?;
    let mut encoded = serde_json::to_vec(request)
        .map_err(|error| HostSupervisorError::Protocol(format!("encode request: {error}")))?;
    if encoded.len() + 1 > MAX_REQUEST_BYTES {
        return Err(HostSupervisorError::InvalidRequest(
            "encoded request exceeds protocol bound".to_string(),
        ));
    }
    encoded.push(b'\n');
    write_all_with_deadline(&mut stream, &encoded, deadline)?;

    let (mut response, sender) = read_response_with_credentials(&stream, deadline)?;
    verify_supervisor_sender(sender, deadline)?;
    if response.is_empty() || response.len() > MAX_RESPONSE_BYTES || !response.ends_with(b"\n") {
        return Err(HostSupervisorError::Protocol(
            "supervisor returned an empty, truncated, or oversized frame".to_string(),
        ));
    }
    response.pop();
    let receipt: SupervisorReceipt = serde_json::from_slice(&response)
        .map_err(|error| HostSupervisorError::Protocol(format!("decode response: {error}")))?;
    receipt
        .validate_for(request)
        .map_err(|error| HostSupervisorError::Protocol(error.to_string()))?;
    Ok(receipt)
}

fn write_all_with_deadline(
    stream: &mut UnixStream,
    bytes: &[u8],
    deadline: Instant,
) -> Result<(), HostSupervisorError> {
    let mut written = 0;
    while written < bytes.len() {
        stream
            .set_write_timeout(Some(remaining_budget(deadline)?))
            .map_err(|error| {
                HostSupervisorError::Unavailable(format!("set write timeout: {error}"))
            })?;
        match stream.write(&bytes[written..]) {
            Ok(0) => {
                return Err(HostSupervisorError::Unavailable(
                    "supervisor socket closed while writing request".to_string(),
                ));
            }
            Ok(count) => written += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(HostSupervisorError::Unavailable(format!(
                    "write request: {error}"
                )));
            }
        }
    }
    Ok(())
}

fn remaining_budget(deadline: Instant) -> Result<Duration, HostSupervisorError> {
    remaining_budget_at(deadline, Instant::now())
}

fn remaining_budget_at(deadline: Instant, now: Instant) -> Result<Duration, HostSupervisorError> {
    deadline
        .checked_duration_since(now)
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            HostSupervisorError::Unavailable(
                "host supervisor request exhausted its total deadline".to_string(),
            )
        })
}

fn retry_request_until_with<Attempt, Now, Sleep>(
    deadline: Instant,
    mut last_error: Option<HostSupervisorError>,
    mut attempt: Attempt,
    mut now: Now,
    mut sleep: Sleep,
) -> Result<SupervisorReceipt, HostSupervisorError>
where
    Attempt: FnMut(Instant) -> Result<SupervisorReceipt, HostSupervisorError>,
    Now: FnMut() -> Instant,
    Sleep: FnMut(Duration),
{
    for attempt_index in 0..CONNECT_RETRIES {
        if remaining_budget_at(deadline, now()).is_err() {
            return Err(HostSupervisorError::Unavailable(format!(
                "host supervisor retry budget was exhausted after: {}",
                last_error
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "no completed attempt".to_string())
            )));
        }
        match attempt(deadline) {
            Ok(receipt) => return Ok(receipt),
            Err(error) => last_error = Some(error),
        }
        if attempt_index + 1 < CONNECT_RETRIES {
            let Ok(remaining) = remaining_budget_at(deadline, now()) else {
                break;
            };
            sleep(CONNECT_RETRY_DELAY.min(remaining));
        }
    }
    Err(HostSupervisorError::Unavailable(format!(
        "host supervisor did not accept a bounded request: {}",
        last_error
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "no completed attempt".to_string())
    )))
}

fn connect_with_deadline(socket_path: &Path, deadline: Instant) -> std::io::Result<UnixStream> {
    let path = socket_path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if path.is_empty()
        || path.contains(&0)
        || path.len() >= address.sun_path.len()
        || remaining_budget_io(deadline).is_err()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Unix socket path or request deadline is invalid",
        ));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (destination, source) in address.sun_path.iter_mut().zip(path.iter().copied()) {
        *destination = source as libc::c_char;
    }

    let raw_fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if raw_fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let owned_fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    let address_length = std::mem::offset_of!(libc::sockaddr_un, sun_path)
        .saturating_add(path.len())
        .saturating_add(1);
    let connected = unsafe {
        libc::connect(
            owned_fd.as_raw_fd(),
            std::ptr::addr_of!(address).cast(),
            address_length as libc::socklen_t,
        )
    };
    if connected != 0 {
        let error = std::io::Error::last_os_error();
        let retryable = error.raw_os_error().is_some_and(|code| {
            code == libc::EINPROGRESS
                || code == libc::EAGAIN
                || code == libc::EWOULDBLOCK
                || code == libc::EINTR
        });
        if !retryable {
            return Err(error);
        }
        poll_connected(owned_fd.as_raw_fd(), deadline)?;
    }
    let flags = unsafe { libc::fcntl(owned_fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe {
            libc::fcntl(
                owned_fd.as_raw_fd(),
                libc::F_SETFL,
                flags & !libc::O_NONBLOCK,
            )
        } < 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { UnixStream::from_raw_fd(owned_fd.into_raw_fd()) })
}

fn poll_connected(fd: RawFd, deadline: Instant) -> std::io::Result<()> {
    loop {
        let remaining = remaining_budget_io(deadline)?;
        let timeout_ms = remaining.as_millis().max(1).min(libc::c_int::MAX as u128) as libc::c_int;
        let mut descriptor = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let result = unsafe { libc::poll(std::ptr::addr_of_mut!(descriptor), 1, timeout_ms) };
        if result == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Unix socket connect deadline elapsed",
            ));
        }
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        let mut socket_error: libc::c_int = 0;
        let mut length = std::mem::size_of_val(&socket_error) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                std::ptr::addr_of_mut!(socket_error).cast(),
                &mut length,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        return if socket_error == 0 {
            Ok(())
        } else {
            Err(std::io::Error::from_raw_os_error(socket_error))
        };
    }
}

fn remaining_budget_io(deadline: Instant) -> std::io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "host supervisor request deadline elapsed",
            )
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PeerCredentials {
    pid: u32,
    uid: u32,
}

// Ancillary-data headers require native word alignment even though the payload is bytes.
#[repr(align(8))]
struct AncillaryBuffer([u8; 128]);

fn peer_credentials(stream: &UnixStream) -> Result<PeerCredentials, HostSupervisorError> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            std::os::fd::AsRawFd::as_raw_fd(stream),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::addr_of_mut!(credentials).cast(),
            &mut length,
        )
    };
    if result != 0 || length as usize != std::mem::size_of::<libc::ucred>() {
        return Err(HostSupervisorError::Protocol(
            "cannot verify supervisor peer credentials".to_string(),
        ));
    }
    if credentials.pid <= 0 {
        return Err(HostSupervisorError::Protocol(
            "supervisor peer has no verifiable pid".to_string(),
        ));
    }
    Ok(PeerCredentials {
        pid: credentials.pid as u32,
        uid: credentials.uid,
    })
}

fn verify_listener_uid(stream: &UnixStream) -> Result<(), HostSupervisorError> {
    let credentials = peer_credentials(stream)?;
    let expected_uid = unsafe { libc::geteuid() };
    if credentials.uid != expected_uid {
        return Err(HostSupervisorError::Protocol(format!(
            "supervisor listener uid {} does not match expected {expected_uid}",
            credentials.uid
        )));
    }
    Ok(())
}

fn verify_supervisor_sender(
    credentials: PeerCredentials,
    deadline: Instant,
) -> Result<(), HostSupervisorError> {
    let expected_uid = unsafe { libc::geteuid() };
    let expected_pid = supervisor_main_pid(deadline)?;
    verify_sender_identity(credentials, expected_uid, expected_pid)
}

fn verify_sender_identity(
    credentials: PeerCredentials,
    expected_uid: u32,
    expected_pid: u32,
) -> Result<(), HostSupervisorError> {
    if credentials.uid != expected_uid || credentials.pid != expected_pid {
        return Err(HostSupervisorError::Protocol(format!(
            "supervisor peer uid/pid {}/{} does not match expected {expected_uid}/{expected_pid}",
            credentials.uid, credentials.pid
        )));
    }
    Ok(())
}

fn enable_passcred(stream: &UnixStream) -> Result<(), HostSupervisorError> {
    let enabled: libc::c_int = 1;
    if unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PASSCRED,
            std::ptr::addr_of!(enabled).cast(),
            std::mem::size_of_val(&enabled) as libc::socklen_t,
        )
    } != 0
    {
        return Err(HostSupervisorError::Protocol(format!(
            "enable response SO_PASSCRED: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

fn read_response_with_credentials(
    stream: &UnixStream,
    deadline: Instant,
) -> Result<(Vec<u8>, PeerCredentials), HostSupervisorError> {
    let mut response = Vec::new();
    let mut sender = None;
    loop {
        // SO_RCVTIMEO is a per-recv timeout. Re-arm it from the one absolute request deadline
        // before every chunk so a peer cannot stretch an 8 second request into N x 8 seconds by
        // drip-feeding a bounded multi-chunk frame.
        stream
            .set_read_timeout(Some(remaining_budget(deadline)?))
            .map_err(|error| {
                HostSupervisorError::Unavailable(format!("set read timeout: {error}"))
            })?;
        let mut bytes = [0_u8; 8192];
        let mut control = AncillaryBuffer([0_u8; 128]);
        let mut io = libc::iovec {
            iov_base: bytes.as_mut_ptr().cast(),
            iov_len: bytes.len(),
        };
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = std::ptr::addr_of_mut!(io);
        message.msg_iovlen = 1;
        message.msg_control = control.0.as_mut_ptr().cast();
        message.msg_controllen = control.0.len();
        let received = unsafe { libc::recvmsg(stream.as_raw_fd(), &mut message, 0) };
        if received < 0 {
            return Err(HostSupervisorError::Unavailable(format!(
                "recvmsg supervisor response: {}",
                std::io::Error::last_os_error()
            )));
        }
        if received == 0 {
            return Err(HostSupervisorError::Protocol(
                "supervisor closed before a complete response".to_string(),
            ));
        }
        if message.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(HostSupervisorError::Protocol(
                "supervisor response credentials were truncated".to_string(),
            ));
        }
        let credentials = credentials_from_message(&message).ok_or_else(|| {
            HostSupervisorError::Protocol(
                "supervisor response carried no SCM_CREDENTIALS".to_string(),
            )
        })?;
        if sender.is_some_and(|existing| existing != credentials) {
            return Err(HostSupervisorError::Protocol(
                "supervisor response sender credentials changed mid-frame".to_string(),
            ));
        }
        sender = Some(credentials);
        response.extend_from_slice(&bytes[..received as usize]);
        if response.len() > MAX_RESPONSE_BYTES {
            return Err(HostSupervisorError::Protocol(
                "supervisor response exceeds protocol bound".to_string(),
            ));
        }
        if response.contains(&b'\n') {
            break;
        }
    }
    Ok((
        response,
        sender.expect("credential checked for every response chunk"),
    ))
}

fn credentials_from_message(message: &libc::msghdr) -> Option<PeerCredentials> {
    let mut header = unsafe { libc::CMSG_FIRSTHDR(message) };
    while !header.is_null() {
        let current = unsafe { &*header };
        if current.cmsg_level == libc::SOL_SOCKET
            && current.cmsg_type == libc::SCM_CREDENTIALS
            && current.cmsg_len
                >= unsafe { libc::CMSG_LEN(std::mem::size_of::<libc::ucred>() as _) } as usize
        {
            let credentials =
                unsafe { std::ptr::read_unaligned(libc::CMSG_DATA(header).cast::<libc::ucred>()) };
            if credentials.pid > 0 {
                return Some(PeerCredentials {
                    pid: credentials.pid as u32,
                    uid: credentials.uid,
                });
            }
        }
        header = unsafe { libc::CMSG_NXTHDR(message, header) };
    }
    None
}

fn supervisor_main_pid(deadline: Instant) -> Result<u32, HostSupervisorError> {
    let systemctl = fixed_systemctl_path().map_err(HostSupervisorError::Unavailable)?;
    let timeout = remaining_budget(deadline)?.min(ACTIVATE_TIMEOUT);
    let mut child = Command::new(systemctl)
        .args([
            "--user",
            "show",
            SUPERVISOR_UNIT,
            "--no-pager",
            "--property=MainPID",
            "--value",
        ])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            HostSupervisorError::Unavailable(format!("query supervisor MainPID: {error}"))
        })?;
    let status = child.wait_timeout(timeout).map_err(|error| {
        HostSupervisorError::Unavailable(format!("wait supervisor MainPID: {error}"))
    })?;
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(HostSupervisorError::Unavailable(
            "query supervisor MainPID timed out".to_string(),
        ));
    };
    let stdout = child
        .stdout
        .take()
        .map(read_bounded)
        .transpose()
        .map_err(|error| {
            HostSupervisorError::Unavailable(format!("read supervisor MainPID: {error}"))
        })?
        .unwrap_or_default();
    if !status.success() {
        return Err(HostSupervisorError::Unavailable(format!(
            "query supervisor MainPID exited {status}"
        )));
    }
    stdout
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0)
        .ok_or_else(|| {
            HostSupervisorError::Protocol("supervisor service has no live MainPID".to_string())
        })
}

fn set_close_on_exec(fd: RawFd) -> Result<(), HostSupervisorError> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(HostSupervisorError::Protocol(format!(
            "cannot set socket FD_CLOEXEC: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

fn activate_socket_unit(deadline: Instant) -> Result<(), String> {
    let systemctl = fixed_systemctl_path()?;
    let timeout = deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| "socket activation request budget was exhausted".to_string())?
        .min(ACTIVATE_TIMEOUT);
    let mut child = Command::new(systemctl)
        .args(["--user", "start", SOCKET_UNIT])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn fixed socket activation: {error}"))?;
    let status = child
        .wait_timeout(timeout)
        .map_err(|error| format!("wait fixed socket activation: {error}"))?;
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("fixed socket activation timed out".to_string());
    };
    let stderr = child
        .stderr
        .take()
        .map(read_bounded)
        .transpose()
        .unwrap_or_else(|error| Some(format!("read activation error: {error}")))
        .unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "fixed socket activation exited {status}: {}",
            truncate(&stderr, 512)
        ))
    }
}

fn fixed_systemctl_path() -> Result<&'static str, String> {
    ["/usr/bin/systemctl", "/bin/systemctl"]
        .into_iter()
        .find(|candidate| Path::new(candidate).is_file())
        .ok_or_else(|| "systemctl is unavailable at an audited absolute path".to_string())
}

fn read_bounded(mut reader: impl std::io::Read) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    reader.by_ref().take(4096).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::os::unix::net::UnixStream;
    use std::thread;

    use super::*;

    #[test]
    fn peer_credentials_include_same_process_pid_and_uid() {
        let (left, _right) = UnixStream::pair().expect("socket pair");
        let credentials = peer_credentials(&left).expect("peer credentials");
        assert_eq!(credentials.uid, unsafe { libc::geteuid() });
        assert_eq!(credentials.pid, std::process::id());
    }

    #[test]
    fn response_sender_identity_rejects_wrong_supervisor_pid() {
        let credentials = PeerCredentials { pid: 41, uid: 1000 };
        assert!(verify_sender_identity(credentials, 1000, 42).is_err());
        assert!(verify_sender_identity(credentials, 1000, 41).is_ok());
    }

    #[test]
    fn fake_same_uid_responder_scm_pid_is_rejected() {
        let (receiver, sender) = UnixStream::pair().expect("socket pair");
        enable_passcred(&receiver).expect("passcred");
        let sender_fd = sender.as_raw_fd();
        let child_pid = unsafe { libc::fork() };
        assert!(child_pid >= 0, "fork failed");
        if child_pid == 0 {
            const RESPONSE: &[u8] = b"{}\n";
            let written =
                unsafe { libc::write(sender_fd, RESPONSE.as_ptr().cast(), RESPONSE.len()) };
            unsafe { libc::_exit((written != RESPONSE.len() as isize) as i32) };
        }
        drop(sender);

        let (response, credentials) =
            read_response_with_credentials(&receiver, Instant::now() + Duration::from_secs(5))
                .expect("credentialed response");
        assert_eq!(response, b"{}\n");
        assert_eq!(credentials.uid, unsafe { libc::geteuid() });
        assert_eq!(credentials.pid, child_pid as u32);
        assert!(verify_sender_identity(
            credentials,
            unsafe { libc::geteuid() },
            std::process::id(),
        )
        .is_err());

        let mut child_status = 0;
        assert_eq!(
            unsafe { libc::waitpid(child_pid, &mut child_status, 0) },
            child_pid
        );
        assert!(libc::WIFEXITED(child_status));
        assert_eq!(libc::WEXITSTATUS(child_status), 0);
    }

    #[test]
    fn response_reader_requires_kernel_sender_credentials() {
        let (receiver, mut sender) = UnixStream::pair().expect("socket pair");
        enable_passcred(&receiver).expect("passcred");
        let writer = thread::spawn(move || sender.write_all(b"{}\n").expect("response"));
        let (response, credentials) =
            read_response_with_credentials(&receiver, Instant::now() + Duration::from_secs(5))
                .expect("credentialed response");
        writer.join().expect("writer");
        assert_eq!(response, b"{}\n");
        assert_eq!(credentials.pid, std::process::id());
        assert_eq!(credentials.uid, unsafe { libc::geteuid() });
    }

    #[test]
    fn response_chunks_share_one_absolute_read_deadline() {
        let (receiver, mut sender) = UnixStream::pair().expect("socket pair");
        enable_passcred(&receiver).expect("passcred");
        let writer = thread::spawn(move || {
            sender.write_all(b"{").expect("first response chunk");
            thread::sleep(Duration::from_millis(100));
            sender.write_all(b"}").expect("second response chunk");
            thread::sleep(Duration::from_millis(100));
            sender.write_all(b"\n").expect("final response chunk");
        });

        let result =
            read_response_with_credentials(&receiver, Instant::now() + Duration::from_millis(150));
        writer.join().expect("writer");
        assert!(
            result.is_err(),
            "three individually timely chunks must not multiply the total deadline"
        );
    }

    #[test]
    fn client_socket_is_close_on_exec() {
        let (left, _right) = UnixStream::pair().expect("socket pair");
        set_close_on_exec(left.as_raw_fd()).expect("cloexec");
        let flags = unsafe { libc::fcntl(left.as_raw_fd(), libc::F_GETFD) };
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
    }

    #[test]
    fn socket_path_is_beneath_user_runtime_directory() {
        let path = supervisor_socket_path().expect("runtime path");
        assert!(path.ends_with(SOCKET_RELATIVE_PATH));
        assert!(!path.to_string_lossy().contains(".."));
    }

    #[test]
    fn activation_uses_only_fixed_unit_and_absolute_command() {
        assert_eq!(SOCKET_UNIT, "pinvou3-supervisor.socket");
        if let Ok(path) = fixed_systemctl_path() {
            assert!(path.starts_with('/'));
        }
    }

    #[test]
    fn retry_attempts_share_one_deadline_instead_of_multiplying_io_timeout() {
        let started = Instant::now();
        let deadline = started + REQUEST_BUDGET;
        let clock = Cell::new(started);
        let attempts = Cell::new(0_usize);
        let sleeps = Cell::new(0_usize);
        let result = retry_request_until_with(
            deadline,
            None,
            |received_deadline| {
                assert_eq!(received_deadline, deadline);
                attempts.set(attempts.get() + 1);
                clock.set(clock.get() + Duration::from_secs(3));
                Err::<SupervisorReceipt, _>(HostSupervisorError::Unavailable(
                    "injected slow response".to_string(),
                ))
            },
            || clock.get(),
            |duration| {
                sleeps.set(sleeps.get() + 1);
                clock.set(clock.get() + duration);
            },
        );

        assert!(result.is_err());
        assert_eq!(
            attempts.get(),
            3,
            "deadline must stop retries before 30 attempts"
        );
        assert_eq!(sleeps.get(), 2);
        assert!(clock.get() >= deadline);
    }

    #[test]
    fn exhausted_outer_deadline_starts_no_long_tail_retry_or_sleep() {
        let started = Instant::now();
        let deadline = started + REQUEST_BUDGET;
        let clock = Cell::new(started);
        let attempts = Cell::new(0_usize);
        let sleeps = Cell::new(0_usize);
        let result = retry_request_until_with(
            deadline,
            None,
            |_received_deadline| {
                attempts.set(attempts.get() + 1);
                clock.set(deadline);
                Err::<SupervisorReceipt, _>(HostSupervisorError::Unavailable(
                    "injected response consumed the whole budget".to_string(),
                ))
            },
            || clock.get(),
            |_duration| sleeps.set(sleeps.get() + 1),
        );

        assert!(result.is_err());
        assert_eq!(attempts.get(), 1);
        assert_eq!(sleeps.get(), 0);
    }
}
