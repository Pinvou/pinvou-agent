use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use pinvou_protocol::{
    HelloClient, HelloServer, IpcMessage, RuntimeEventEnvelope, encode_frame, read_frame,
};

struct ProcessGuard {
    child: Child,
    lock: PathBuf,
    endpoint_dir: Option<PathBuf>,
}

impl ProcessGuard {
    fn new(child: Child, lock: PathBuf, _endpoint: &str) -> Self {
        #[cfg(target_os = "linux")]
        let endpoint_dir = PathBuf::from(_endpoint).parent().map(PathBuf::from);
        #[cfg(not(target_os = "linux"))]
        let endpoint_dir = None;
        Self {
            child,
            lock,
            endpoint_dir,
        }
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.lock);
        if let Some(endpoint_dir) = &self.endpoint_dir {
            let _ = std::fs::remove_dir_all(endpoint_dir);
        }
    }
}

#[test]
fn controller_client_reaches_real_node_process_and_receives_schema_event() {
    let unique = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    #[cfg(windows)]
    let endpoint = format!(r"\\.\pipe\pinvou-node-contract-{unique}");
    #[cfg(target_os = "linux")]
    let endpoint = std::env::temp_dir()
        .join(format!("pinvou-node-{unique}/node.sock"))
        .display()
        .to_string();
    let lock = std::env::temp_dir().join(format!("pinvou-node-{unique}.lock"));
    let child = Command::new(env!("CARGO_BIN_EXE_pinvou-node"))
        .arg("--endpoint")
        .arg(&endpoint)
        .arg("--instance-id")
        .arg("m1-instance")
        .arg("--lock-file")
        .arg(&lock)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _guard = ProcessGuard::new(child, lock, &endpoint);
    let mut client = (0..80)
        .find_map(|_| {
            let result = connect_node(&endpoint, "m1-instance").ok();
            if result.is_none() {
                std::thread::sleep(Duration::from_millis(25));
            }
            result
        })
        .expect("node must become ready");
    let health = node_request(
        &mut client,
        1,
        "health",
        serde_json::json!({"instance_id":"m1-instance"}),
    );
    assert_eq!(health.payload()["status"], "ok");
    let echo = node_request(
        &mut client,
        2,
        "runtime.echo",
        serde_json::json!({"instance_id":"m1-instance","text":"three-process-echo"}),
    );
    let event = RuntimeEventEnvelope::from_value(echo.payload().clone()).unwrap();
    assert_eq!(event.kind(), "text.delta");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(event.payload().get()).unwrap()["content"],
        "three-process-echo"
    );
}

#[test]
fn node_wire_version_mismatch_returns_code_three_and_disconnects() {
    use std::io::{Read as _, Write as _};
    let unique = format!(
        "v2-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    #[cfg(windows)]
    let endpoint = format!(r"\\.\pipe\pinvou-node-contract-{unique}");
    #[cfg(target_os = "linux")]
    let endpoint = std::env::temp_dir()
        .join(format!("pinvou-node-{unique}/node.sock"))
        .display()
        .to_string();
    let lock = std::env::temp_dir().join(format!("pinvou-node-{unique}.lock"));
    let child = Command::new(env!("CARGO_BIN_EXE_pinvou-node"))
        .arg("--endpoint")
        .arg(&endpoint)
        .arg("--instance-id")
        .arg("v2-instance")
        .arg("--lock-file")
        .arg(&lock)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _guard = ProcessGuard::new(child, lock, &endpoint);
    let mut stream = (0..80)
        .find_map(|_| {
            let stream = open_endpoint(&endpoint).ok();
            if stream.is_none() {
                std::thread::sleep(Duration::from_millis(25));
            }
            stream
        })
        .expect("node endpoint must become ready");
    let payload = br#"{"kind":"hello","protocol_version":2,"client_info":{}}"#;
    stream
        .write_all(&(payload.len() as u32).to_le_bytes())
        .unwrap();
    stream.write_all(payload).unwrap();
    let response: IpcMessage = read_frame(&mut stream).unwrap();
    assert_eq!(response.payload()["code"], 3);
    let mut byte = [0_u8; 1];
    assert_eq!(stream.read(&mut byte).unwrap(), 0);
}

trait TestStream: std::io::Read + std::io::Write {}
impl<T: std::io::Read + std::io::Write> TestStream for T {}
#[cfg(windows)]
fn open_endpoint(endpoint: &str) -> std::io::Result<Box<dyn TestStream>> {
    Ok(Box::new(
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(endpoint)?,
    ))
}

fn connect_node(
    endpoint: &str,
    instance_id: &str,
) -> Result<Box<dyn TestStream>, Box<dyn std::error::Error>> {
    use std::io::Write as _;
    let mut stream = open_endpoint(endpoint)?;
    let hello = HelloClient::new(serde_json::json!({"client":"m1-contract"}))?;
    stream.write_all(&encode_frame(&hello)?)?;
    let answer: HelloServer = read_frame(&mut stream)?;
    if answer.instance_id() != instance_id {
        return Err("instance mismatch".into());
    }
    Ok(stream)
}

fn node_request(
    stream: &mut Box<dyn TestStream>,
    id: u64,
    method: &str,
    payload: serde_json::Value,
) -> IpcMessage {
    use std::io::Write as _;
    let request = IpcMessage::request(serde_json::json!(id), method, payload).unwrap();
    stream.write_all(&encode_frame(&request).unwrap()).unwrap();
    read_frame(stream).unwrap()
}
#[cfg(target_os = "linux")]
fn open_endpoint(endpoint: &str) -> std::io::Result<Box<dyn TestStream>> {
    Ok(Box::new(std::os::unix::net::UnixStream::connect(endpoint)?))
}
