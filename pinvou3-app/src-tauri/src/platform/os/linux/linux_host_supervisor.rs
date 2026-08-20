use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use pinvou_host_supervisor_protocol::{
    SupervisorReceipt, SupervisorRequest, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES,
};
use wait_timeout::ChildExt;

use crate::platform::host_supervisor::HostSupervisorError;

const SOCKET_RELATIVE_PATH: &str = "pinvou-supervisor/control.sock";
const SOCKET_UNIT: &str = "pinvou3-supervisor.socket";
const SUPERVISOR_UNIT: &str = "pinvou3-supervisor.service";
const IO_TIMEOUT: Duration = Duration::from_secs(8);
const ACTIVATE_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_RETRIES: usize = 30;

pub(crate) fn host_supervisor_request(
    request: &SupervisorRequest,
) -> Result<SupervisorReceipt, HostSupervisorError> {
    let socket_path = supervisor_socket_path()?;
    match request_at(&socket_path, request) {
        Ok(receipt) => Ok(receipt),
        Err(first_error) => {
            activate_socket_unit().map_err(|activation_error| {
                HostSupervisorError::Unavailable(format!(
                    "connect failed ({first_error}); socket activation failed ({activation_error})"
                ))
            })?;
            for _ in 0..CONNECT_RETRIES {
                match request_at(&socket_path, request) {
                    Ok(receipt) => return Ok(receipt),
                    Err(_) => thread::sleep(Duration::from_millis(50)),
                }
            }
            Err(HostSupervisorError::Unavailable(format!(
                "socket activation completed but {} did not accept a bounded request",
                socket_path.display()
            )))
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
) -> Result<SupervisorReceipt, HostSupervisorError> {
    let mut stream = UnixStream::connect(socket_path).map_err(|error| {
        HostSupervisorError::Unavailable(format!("connect {}: {error}", socket_path.display()))
    })?;
    set_close_on_exec(stream.as_raw_fd())?;
    verify_listener_uid(&stream)?;
    enable_passcred(&stream)?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|error| HostSupervisorError::Unavailable(format!("set read timeout: {error}")))?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|error| HostSupervisorError::Unavailable(format!("set write timeout: {error}")))?;

    let encoded = serde_json::to_vec(request)
        .map_err(|error| HostSupervisorError::Protocol(format!("encode request: {error}")))?;
    if encoded.len() + 1 > MAX_REQUEST_BYTES {
        return Err(HostSupervisorError::InvalidRequest(
            "encoded request exceeds protocol bound".to_string(),
        ));
    }
    stream
        .write_all(&encoded)
        .and_then(|_| stream.write_all(b"\n"))
        .and_then(|_| stream.flush())
        .map_err(|error| HostSupervisorError::Unavailable(format!("write request: {error}")))?;

    let (mut response, sender) = read_response_with_credentials(&stream)?;
    verify_supervisor_sender(sender)?;
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

fn verify_supervisor_sender(credentials: PeerCredentials) -> Result<(), HostSupervisorError> {
    let expected_uid = unsafe { libc::geteuid() };
    let expected_pid = supervisor_main_pid()?;
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
) -> Result<(Vec<u8>, PeerCredentials), HostSupervisorError> {
    let mut response = Vec::new();
    let mut sender = None;
    loop {
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

fn supervisor_main_pid() -> Result<u32, HostSupervisorError> {
    let systemctl = fixed_systemctl_path().map_err(HostSupervisorError::Unavailable)?;
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
    let status = child.wait_timeout(ACTIVATE_TIMEOUT).map_err(|error| {
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

fn activate_socket_unit() -> Result<(), String> {
    let systemctl = fixed_systemctl_path()?;
    let mut child = Command::new(systemctl)
        .args(["--user", "start", SOCKET_UNIT])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn fixed socket activation: {error}"))?;
    let status = child
        .wait_timeout(ACTIVATE_TIMEOUT)
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
            read_response_with_credentials(&receiver).expect("credentialed response");
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
            read_response_with_credentials(&receiver).expect("credentialed response");
        writer.join().expect("writer");
        assert_eq!(response, b"{}\n");
        assert_eq!(credentials.pid, std::process::id());
        assert_eq!(credentials.uid, unsafe { libc::geteuid() });
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
}
