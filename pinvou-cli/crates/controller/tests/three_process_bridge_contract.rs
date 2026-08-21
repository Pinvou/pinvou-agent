use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use pinvou_controller::ControllerPaths;
use pinvou_protocol::{
    HelloClient, HelloServer, IpcMessage, RuntimeEventEnvelope, encode_frame, read_frame,
};

struct ProcessGuard {
    child: Child,
    root: PathBuf,
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for _ in 0..50 {
            if std::fs::remove_dir_all(&self.root).is_ok() || !self.root.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

#[test]
fn raw_client_reaches_node_chat_start_through_controller_daemon() {
    let controller = PathBuf::from(env!("CARGO_BIN_EXE_pinvou-controller"));
    let node = sibling_node(&controller);
    assert!(
        node.is_absolute() && node.is_file(),
        "build pinvou-node beside the controller binary"
    );

    let root = std::env::temp_dir().join(format!(
        "pinvou-three-process-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut command = Command::new(controller);
    configure_private_roots(&mut command, &root);
    std::fs::create_dir_all(&root).unwrap();
    let stderr = root.join("controller.stderr");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(std::fs::File::create(&stderr).unwrap()));
    let paths = discover_with_private_roots(&root);
    let child = command.spawn().unwrap();
    let mut guard = ProcessGuard { child, root };

    let mut stream = connect_with_retry(&paths.endpoint().display(), &stderr);
    let hello = HelloClient::new(serde_json::json!({"client":"raw-contract"})).unwrap();
    stream.write_all(&encode_frame(&hello).unwrap()).unwrap();
    let server: HelloServer = read_frame(&mut stream).unwrap();
    let request = IpcMessage::request(
        serde_json::json!(1),
        "chat.start",
        serde_json::json!({"instance_id":server.instance_id(), "prompt":"three-process-echo"}),
    )
    .unwrap();
    stream.write_all(&encode_frame(&request).unwrap()).unwrap();
    stream.flush().unwrap();
    let event: IpcMessage = read_frame(&mut stream).unwrap();
    assert_eq!(event.topic(), Some("runtime.event"));
    let envelope = RuntimeEventEnvelope::from_value(event.payload().clone()).unwrap();
    let value = serde_json::to_value(envelope).unwrap();
    assert_eq!(value["kind"], "text.delta");
    assert_eq!(value["payload"]["content"], "three-process-echo");

    let terminal: IpcMessage = read_frame(&mut stream).unwrap();
    assert_eq!(terminal.topic(), Some("runtime.event"));
    let terminal_envelope = RuntimeEventEnvelope::from_value(terminal.payload().clone()).unwrap();
    let terminal_value = serde_json::to_value(terminal_envelope).unwrap();
    assert_eq!(terminal_value["kind"], "turn.ended");
    assert_eq!(terminal_value["stream_id"], "control");
    assert_eq!(terminal_value["rate_class"], "R0");
    assert_eq!(terminal_value["seq"], 1);
    assert_eq!(terminal_value["payload"]["end_reason"], "completed");

    guard.child.kill().unwrap();
    guard.child.wait().unwrap();
}

fn sibling_node(controller: &Path) -> PathBuf {
    #[cfg(windows)]
    let name = "pinvou-node.exe";
    #[cfg(not(windows))]
    let name = "pinvou-node";
    controller
        .parent()
        .unwrap()
        .join(name)
        .canonicalize()
        .unwrap()
}

fn configure_private_roots(command: &mut Command, root: &Path) {
    #[cfg(windows)]
    command.env("LOCALAPPDATA", root.join("local"));
    #[cfg(target_os = "linux")]
    command
        .env("HOME", root.join("home"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"));
}

fn discover_with_private_roots(root: &Path) -> ControllerPaths {
    #[cfg(windows)]
    let vars = [("LOCALAPPDATA", root.join("local"))];
    #[cfg(target_os = "linux")]
    let vars = [
        ("HOME", root.join("home")),
        ("XDG_DATA_HOME", root.join("data")),
        ("XDG_RUNTIME_DIR", root.join("runtime")),
    ];
    let old: Vec<_> = vars
        .iter()
        .map(|(name, _)| (*name, std::env::var_os(name)))
        .collect();
    for (name, value) in &vars {
        unsafe { std::env::set_var(name, value) };
    }
    let paths = ControllerPaths::discover().unwrap();
    for (name, value) in old {
        match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }
    paths
}

fn connect_with_retry(endpoint: &str, stderr: &Path) -> Box<dyn ReadWrite> {
    let mut last = None;
    for _ in 0..200 {
        match connect(endpoint) {
            Ok(stream) => return stream,
            Err(error) => last = Some(error),
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!(
        "controller endpoint unavailable: {:?}; stderr={}",
        last,
        std::fs::read_to_string(stderr).unwrap_or_default()
    );
}

#[cfg(windows)]
fn connect(endpoint: &str) -> std::io::Result<Box<dyn ReadWrite>> {
    Ok(Box::new(
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(endpoint)?,
    ))
}

#[cfg(target_os = "linux")]
fn connect(endpoint: &str) -> std::io::Result<Box<dyn ReadWrite>> {
    Ok(Box::new(std::os::unix::net::UnixStream::connect(endpoint)?))
}
