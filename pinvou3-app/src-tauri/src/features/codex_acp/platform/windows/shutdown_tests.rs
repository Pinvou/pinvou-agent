//! Controlled stdio-peer coverage for the production PoolShutdown component.
//!
//! This deliberately does not construct AcpPool or launch the application. It
//! exercises the same unpublished Child/shutdown-sender ownership across real
//! typed ACP initialize/load traffic, then observes Windows process termination.
//! The session mutex/publication flag model the caller boundary, not the full
//! pool's registration, persistence, browser, or native tray integration.

use std::io::{BufRead, Write};
use std::os::windows::io::{AsRawHandle, BorrowedHandle, OwnedHandle};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AgentCapabilities, InitializeRequest, InitializeResponse, LoadSessionRequest,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{TerminateProcess, WaitForSingleObject};

use crate::features::codex_acp::shutdown::PoolShutdown;
use crate::platform::process::HiddenTokioCommand;

const FIXTURE_TEST: &str =
    "features::codex_acp::platform::windows::shutdown_tests::stdio_peer_fixture";
const FIXTURE_ENV: &str = "PINVOU_TEST_ACP_SHUTDOWN_STDIO_PEER";
const STDIO_READY: &str = "PINVOU_ACP_SHUTDOWN_STDIO_READY";
const LOAD_RECEIVED: &str = "PINVOU_ACP_SHUTDOWN_LOAD_RECEIVED";
const WATCHDOG: Duration = Duration::from_secs(15);

/// A separate Windows process handle observes exit without keeping Child alive.
/// On every failure path it also terminates/waits the test's own fixture only.
struct PeerExitGuard(OwnedHandle);

impl PeerExitGuard {
    fn new(child: &Child) -> Result<Self> {
        let raw = child
            .raw_handle()
            .context("fixture process handle missing")?;
        // SAFETY: Child owns this live handle throughout the clone operation.
        let handle = unsafe { BorrowedHandle::borrow_raw(raw) }.try_clone_to_owned()?;
        Ok(Self(handle))
    }

    fn is_running(&self) -> bool {
        // SAFETY: the owned handle remains live for this nonblocking wait.
        unsafe { WaitForSingleObject(self.0.as_raw_handle().cast(), 0) == WAIT_TIMEOUT }
    }

    async fn observe_exit(&self) -> Result<()> {
        let handle = self.0.try_clone()?;
        let status = tokio::task::spawn_blocking(move || {
            // SAFETY: this closure owns the process handle for the whole wait.
            unsafe { WaitForSingleObject(handle.as_raw_handle().cast(), 15_000) }
        })
        .await?;
        ensure!(
            status == WAIT_OBJECT_0,
            "fixture did not terminate: {status}"
        );
        Ok(())
    }
}

impl Drop for PeerExitGuard {
    fn drop(&mut self) {
        let raw = self.0.as_raw_handle().cast();
        // SAFETY: this is a cloned handle to only the child created by this
        // test, never a PID lookup. The wait observes termination before close.
        unsafe {
            if WaitForSingleObject(raw, 0) != WAIT_OBJECT_0 {
                let _ = TerminateProcess(raw, 101);
                let _ = WaitForSingleObject(raw, 5_000);
            }
        }
    }
}

struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(serde::Deserialize)]
#[serde(tag = "method", content = "params")]
enum FixtureRequest {
    #[serde(rename = "initialize")]
    Initialize(Box<InitializeRequest>),
    #[serde(rename = "session/load")]
    Load(LoadSessionRequest),
}

#[derive(serde::Deserialize)]
struct FixtureEnvelope {
    id: Value,
    #[serde(flatten)]
    request: FixtureRequest,
}

fn fixture_request(input: &mut impl BufRead) -> Result<FixtureEnvelope> {
    let mut line = String::new();
    ensure!(input.read_line(&mut line)? != 0, "fixture input closed");
    Ok(serde_json::from_str(&line)?)
}

/// The ignored fixture is inert unless its own Command sets the exact flag.
/// After load it parks even if stdin closes: transport EOF alone must not make
/// the parent mistake a graceful peer exit for kill_on_drop termination.
#[test]
#[ignore = "self-child stdio fixture; invoked by the controlled shutdown test"]
fn stdio_peer_fixture() -> Result<()> {
    if std::env::var_os(FIXTURE_ENV).as_deref() != Some(std::ffi::OsStr::new("1")) {
        return Ok(());
    }
    let mut output = std::io::stdout().lock();
    writeln!(output, "{STDIO_READY}")?;
    output.flush()?;
    let mut input = std::io::stdin().lock();
    let initialize = fixture_request(&mut input)?;
    let FixtureRequest::Initialize(request) = initialize.request else {
        bail!("fixture expected initialize");
    };
    let response = InitializeResponse::new(request.protocol_version)
        .agent_capabilities(AgentCapabilities::new().load_session(true));
    writeln!(
        output,
        "{}",
        serde_json::json!({ "jsonrpc": "2.0", "id": initialize.id, "result": response })
    )?;
    output.flush()?;
    let FixtureRequest::Load(request) = fixture_request(&mut input)?.request else {
        bail!("fixture expected session/load");
    };
    ensure!(request.session_id.to_string() == "shutdown-fixture-session");
    let mut errors = std::io::stderr().lock();
    writeln!(errors, "{LOAD_RECEIVED}")?;
    errors.flush()?;
    loop {
        std::thread::park();
    }
}

async fn wait_for_marker(
    input: &mut (impl tokio::io::AsyncBufRead + Unpin),
    marker: &str,
) -> Result<()> {
    loop {
        let mut line = String::new();
        ensure!(input.read_line(&mut line).await? != 0, "missing {marker}");
        // libtest may put its test-name prefix before the fixture's first
        // stdout marker. Everything through that marker precedes ACP traffic.
        if line.contains(marker) {
            return Ok(());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_cancels_unpublished_child_waiting_for_real_acp_load() -> Result<()> {
    let mut command = HiddenTokioCommand::new(std::env::current_exe()?);
    command
        .args([
            "--exact",
            FIXTURE_TEST,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(FIXTURE_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let peer = PeerExitGuard::new(&child)?;
    let stdin = child.stdin.take().context("fixture stdin missing")?;
    let mut stdout = BufReader::new(child.stdout.take().context("fixture stdout missing")?);
    let mut stderr = BufReader::new(child.stderr.take().context("fixture stderr missing")?);
    tokio::time::timeout(WATCHDOG, wait_for_marker(&mut stdout, STDIO_READY)).await??;

    let shutdown = PoolShutdown::default();
    let sessions = Arc::new(Mutex::new(()));
    let published = Arc::new(AtomicBool::new(false));
    let (pump_finished, pump_done) = oneshot::channel();
    let worker_shutdown = shutdown.clone();
    let worker_sessions = sessions.clone();
    let worker_published = published.clone();
    let mut startup = AbortOnDrop(tokio::spawn(async move {
        let _sessions = worker_shutdown.run(worker_sessions.lock()).await?;
        worker_shutdown
            .run(async move {
                // These owners remain inside the cancellation boundary until
                // publication, matching production spawn_session custody.
                let (ready_tx, ready_rx) = oneshot::channel();
                let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
                tokio::spawn(async move {
                    let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());
                    let result = Client
                        .builder()
                        .connect_with(transport, async move |connection: ConnectionTo<Agent>| {
                            let initialized = connection
                                .send_request(InitializeRequest::new(ProtocolVersion::LATEST))
                                .block_task()
                                .await?;
                            ready_tx
                                .send((connection.clone(), initialized))
                                .map_err(|_| anyhow::anyhow!("startup receiver dropped"))?;
                            let _ = shutdown_rx.await;
                            Ok(())
                        })
                        .await;
                    let _ = pump_finished.send(result);
                });
                let (connection, initialized) = ready_rx.await?;
                ensure!(initialized.agent_capabilities.load_session);
                connection
                    .send_request(LoadSessionRequest::new(
                        "shutdown-fixture-session",
                        Path::new(r"C:\"),
                    ))
                    .block_task()
                    .await?;
                worker_published.store(true, Ordering::Release);
                drop(shutdown_tx);
                drop(child);
                Ok::<(), anyhow::Error>(())
            })
            .await
    }));

    // This barrier comes from the real peer after it decoded session/load,
    // not from an assumed scheduler delay or from the client's request send.
    tokio::time::timeout(WATCHDOG, wait_for_marker(&mut stderr, LOAD_RECEIVED)).await??;
    ensure!(
        peer.is_running(),
        "fixture must be alive with load unanswered"
    );
    ensure!(
        sessions.try_lock().is_err(),
        "startup must own the pool boundary"
    );
    shutdown.cancel();
    let collector = tokio::time::timeout(WATCHDOG, sessions.lock())
        .await
        .context("collector could not progress past cancelled startup")?;
    ensure!(!published.load(Ordering::Acquire));
    drop(collector);
    let outcome = tokio::time::timeout(WATCHDOG, &mut startup.0).await??;
    let cancelled = outcome.expect_err("pending startup must be cancelled by PoolShutdown");
    ensure!(cancelled.to_string() == "acp_pool_shutting_down");
    peer.observe_exit().await?;
    // Sender drop must also let the detached connection foreground finish.
    let _ = tokio::time::timeout(WATCHDOG, pump_done).await??;
    let late = shutdown
        .run(async { published.store(true, Ordering::Release) })
        .await;
    ensure!(late.is_err() && !published.load(Ordering::Acquire));
    Ok(())
}
