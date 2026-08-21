use std::io::Write;

use pinvou_controller::{
    ControllerError, ControllerPaths, ControllerSession, DetachedLaunch, HostPlatform,
    InstanceLock, LocalIpcListener, LocalIpcPolicy, RollingLog,
};
use pinvou_protocol::{
    HelloClient, HelloServer, IpcMessage, StableExitCode, encode_frame, read_frame,
};

fn temp_dir(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "pinvou-controller-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn paths_are_machine_local_and_never_reuse_desktop_data() {
    let root = temp_dir("paths");
    let windows = ControllerPaths::from_roots(
        HostPlatform::Windows,
        root.join("data"),
        root.join("runtime"),
        "logon-7",
    )
    .unwrap();
    assert!(
        windows
            .endpoint()
            .display()
            .contains(r"\\.\pipe\pinvou-controller-")
    );

    let linux = ControllerPaths::from_roots(
        HostPlatform::Linux,
        root.join("data-linux"),
        root.join("runtime-linux"),
        "ignored",
    )
    .unwrap();
    assert!(
        linux
            .endpoint()
            .display()
            .ends_with("pinvou/controller.sock")
    );
    assert!(!format!("{windows:?}{linux:?}").contains(".pinvou3"));
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn controller_data_root_is_tightened_to_0700_and_rejects_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = temp_dir("private-data-root");
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o755)).unwrap();
    let paths = ControllerPaths::from_roots(
        HostPlatform::Linux,
        data.clone(),
        root.join("runtime"),
        "ignored",
    )
    .unwrap();
    paths.prepare_data_root().unwrap();
    assert_eq!(
        std::fs::metadata(&data).unwrap().permissions().mode() & 0o777,
        0o700
    );

    let target = root.join("target");
    let link = root.join("linked-data");
    std::fs::create_dir(&target).unwrap();
    symlink(&target, &link).unwrap();
    let linked = ControllerPaths::from_roots(
        HostPlatform::Linux,
        link,
        root.join("runtime-linked"),
        "ignored",
    )
    .unwrap();
    assert!(matches!(
        linked.prepare_data_root(),
        Err(ControllerError::PathUnavailable)
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn local_ipc_policy_has_no_tcp_and_requires_platform_hardening() {
    let policy = LocalIpcPolicy::for_platform(HostPlatform::Windows);
    assert!(policy.rejects_remote_clients());
    assert!(policy.first_pipe_instance());
    assert!(policy.requires_logon_session_acl());
    assert!(policy.requires_peer_identity());
    assert!(!policy.has_tcp_listener());

    let unix = LocalIpcPolicy::for_platform(HostPlatform::Linux);
    assert_eq!(unix.parent_mode(), Some(0o700));
    assert_eq!(unix.socket_mode(), Some(0o600));
    assert!(unix.requires_peer_identity());
    assert!(!unix.allows_abstract_socket());
}

#[test]
fn os_lock_handle_not_pid_text_controls_single_instance() {
    let root = temp_dir("lock");
    let path = root.join("controller.lock");
    let first = InstanceLock::acquire(&path).unwrap();
    assert_eq!(first.diagnostic_pid(), std::process::id());
    assert!(matches!(
        InstanceLock::acquire(&path),
        Err(ControllerError::AlreadyRunning)
    ));
    drop(first);
    assert!(InstanceLock::acquire(&path).is_ok());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn hello_challenge_and_health_request_have_stable_contracts() {
    let session = ControllerSession::new("instance-test").unwrap();
    let hello = session
        .accept_hello(HelloClient::new(serde_json::json!({"name": "test"})).unwrap())
        .unwrap();
    assert_eq!(hello.instance_id(), "instance-test");

    let request =
        IpcMessage::request(serde_json::json!(7), "health", serde_json::json!({})).unwrap();
    let response = session.handle(request).unwrap();
    assert_eq!(response.id(), Some(&serde_json::json!(7)));
    assert_eq!(response.payload()["status"], "ok");
    assert_eq!(response.payload()["instance_id"], "instance-test");

    let detect = IpcMessage::request(
        serde_json::json!(8),
        "runtime.detect",
        serde_json::json!({}),
    )
    .unwrap();
    let response = session.handle(detect).unwrap();
    assert_eq!(response.id(), Some(&serde_json::json!(8)));
    assert_eq!(response.payload()["status"], "unavailable");
    assert_eq!(response.payload()["runtime"], "local-node");
    assert_eq!(
        response.payload()["protocol_version"],
        pinvou_protocol::IPC_VERSION
    );

    let mismatch = ControllerError::ProtocolMismatch;
    assert_eq!(mismatch.exit_code(), StableExitCode::ControllerUnavailable);
}

#[test]
fn rolling_log_redacts_secrets_and_keeps_five_backups() {
    let root = temp_dir("logs");
    let mut log = RollingLog::with_limits(root.join("controller.log"), 64, 5).unwrap();
    for index in 0..20 {
        writeln!(
            log,
            "line={index} Authorization: Bearer secret api_key=value"
        )
        .unwrap();
        log.flush().unwrap();
    }
    let files = std::fs::read_dir(&root)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(files.len() <= 6);
    for file in files {
        let contents = std::fs::read_to_string(file.path()).unwrap();
        assert!(!contents.contains("secret"));
        assert!(!contents.contains("value"));
        assert!(contents.contains("[REDACTED]"));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn default_log_policy_is_fifty_megabytes_across_five_total_files() {
    assert_eq!(RollingLog::DEFAULT_MAX_BYTES, 50 * 1024 * 1024);
    assert_eq!(RollingLog::DEFAULT_FILE_COUNT, 5);
}

#[test]
fn rolling_log_redacts_secrets_split_across_writes_and_caps_oversized_records() {
    let root = temp_dir("log-records");
    let path = root.join("controller.log");
    let mut log = RollingLog::with_limits(path.clone(), 64, 5).unwrap();
    log.write_all(b"prefix Author").unwrap();
    log.write_all(b"ization: secret\n").unwrap();
    log.write_all(&vec![b'x'; 128]).unwrap();
    log.write_all(b"\n").unwrap();
    log.flush().unwrap();
    drop(log);
    for entry in std::fs::read_dir(&root).unwrap() {
        let path = entry.unwrap().path();
        assert!(std::fs::metadata(&path).unwrap().len() <= 64);
        let text = std::fs::read_to_string(path).unwrap();
        assert!(!text.contains("secret"));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn detached_launch_is_platform_specific_and_not_login_autostart() {
    let windows = DetachedLaunch::for_platform(HostPlatform::Windows);
    assert!(windows.detached_process());
    assert!(windows.new_process_group());
    assert!(!windows.registers_login_autostart());

    let unix = DetachedLaunch::for_platform(HostPlatform::Linux);
    assert!(unix.creates_session());
    assert!(!unix.registers_login_autostart());
}

#[cfg(windows)]
#[test]
fn named_pipe_accepts_two_clients_and_multiple_instance_bound_requests() {
    let root = temp_dir("pipe");
    let paths = ControllerPaths::from_roots(
        HostPlatform::Windows,
        root.join("data"),
        root.join("runtime"),
        "contract-logon",
    )
    .unwrap();
    let endpoint = paths.endpoint().display();
    let mut listener = LocalIpcListener::bind(paths.endpoint()).unwrap();
    let clients = (0..2)
        .map(|client| {
            let endpoint = endpoint.clone();
            std::thread::spawn(move || pipe_client(&endpoint, client))
        })
        .collect::<Vec<_>>();
    let session = ControllerSession::new("challenge-instance").unwrap();
    listener.serve_one(&session).unwrap();
    listener.serve_one(&session).unwrap();
    for client in clients {
        client.join().unwrap();
    }
    drop(listener);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
fn pipe_client(endpoint: &str, client: usize) {
    use std::io::Write as _;
    let mut pipe = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(endpoint)
        {
            Ok(pipe) => break pipe,
            Err(_) => std::thread::yield_now(),
        }
    };
    let hello = HelloClient::new(serde_json::json!({"client": client})).unwrap();
    pipe.write_all(&encode_frame(&hello).unwrap()).unwrap();
    let response: HelloServer = read_frame(&mut pipe).unwrap();
    assert_eq!(response.instance_id(), "challenge-instance");
    for request_id in 0..2 {
        let request = IpcMessage::request(
            serde_json::json!(request_id),
            "health",
            serde_json::json!({"instance_id": response.instance_id()}),
        )
        .unwrap();
        pipe.write_all(&encode_frame(&request).unwrap()).unwrap();
        let response: IpcMessage = read_frame(&mut pipe).unwrap();
        assert_eq!(response.payload()["status"], "ok");
    }
}

#[cfg(windows)]
#[test]
fn wire_version_mismatch_returns_stable_code_three_then_disconnects() {
    use std::io::{Read as _, Write as _};
    let root = temp_dir("pipe-version");
    let paths = ControllerPaths::from_roots(
        HostPlatform::Windows,
        root.join("data"),
        root.join("runtime"),
        "version-logon",
    )
    .unwrap();
    let endpoint = paths.endpoint().display();
    let mut listener = LocalIpcListener::bind(paths.endpoint()).unwrap();
    let client = std::thread::spawn(move || {
        let mut pipe = loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&endpoint)
            {
                Ok(pipe) => break pipe,
                Err(_) => std::thread::yield_now(),
            }
        };
        let payload = br#"{"kind":"hello","protocol_version":2,"client_info":{}}"#;
        pipe.write_all(&(payload.len() as u32).to_le_bytes())
            .unwrap();
        pipe.write_all(payload).unwrap();
        let response: IpcMessage = read_frame(&mut pipe).unwrap();
        assert_eq!(response.payload()["code"], 3);
        let mut byte = [0_u8; 1];
        assert_eq!(pipe.read(&mut byte).unwrap(), 0);
    });
    let session = ControllerSession::new("version-instance").unwrap();
    listener.serve_one(&session).unwrap();
    client.join().unwrap();
    drop(listener);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn pathname_uds_enforces_modes_peer_identity_and_concurrent_clients() {
    use std::os::unix::fs::PermissionsExt;
    let root = temp_dir("uds");
    let paths = ControllerPaths::from_roots(
        HostPlatform::Linux,
        root.join("data"),
        root.join("runtime"),
        "unused",
    )
    .unwrap();
    let socket = match paths.endpoint() {
        pinvou_controller::LocalEndpoint::UnixSocket(path) => path.clone(),
        _ => unreachable!(),
    };
    let mut listener = LocalIpcListener::bind(paths.endpoint()).unwrap();
    assert_eq!(
        std::fs::metadata(socket.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let clients = (0..2)
        .map(|client| {
            let socket = socket.clone();
            std::thread::spawn(move || uds_client(&socket, client))
        })
        .collect::<Vec<_>>();
    let session = ControllerSession::new("uds-instance").unwrap();
    listener.serve_one(&session).unwrap();
    listener.serve_one(&session).unwrap();
    for client in clients {
        client.join().unwrap();
    }
    drop(listener);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
fn uds_client(socket: &std::path::Path, client: usize) {
    use std::{io::Write as _, os::unix::net::UnixStream};
    let mut stream = UnixStream::connect(socket).unwrap();
    let hello = HelloClient::new(serde_json::json!({"client": client})).unwrap();
    stream.write_all(&encode_frame(&hello).unwrap()).unwrap();
    let response: HelloServer = read_frame(&mut stream).unwrap();
    assert_eq!(response.instance_id(), "uds-instance");
    for request_id in 0..2 {
        let request = IpcMessage::request(
            serde_json::json!(request_id),
            "health",
            serde_json::json!({"instance_id": response.instance_id()}),
        )
        .unwrap();
        stream.write_all(&encode_frame(&request).unwrap()).unwrap();
        let response: IpcMessage = read_frame(&mut stream).unwrap();
        assert_eq!(response.payload()["status"], "ok");
    }
}
