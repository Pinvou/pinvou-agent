#![cfg(feature = "distributed")]

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use pinvou_controller::{
    ControllerPaths, ControllerSession, HostPlatform, LocalEndpoint, LocalIpcListener,
};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

#[cfg(windows)]
use std::fs::OpenOptions;

#[test]
fn no_arguments_on_a_pipe_fail_fast_without_starting_controller() {
    let executable = env!("CARGO_BIN_EXE_pinvou");
    let (root, scope) = isolated_root("non-tty");
    let cleanup = TestDirectory::absent(root.clone());
    assert!(!root.exists());
    let (warm, _, _, _) = run_isolated(executable, &[], &root, &scope, "warm");
    assert_eq!(warm.code(), Some(2));

    let mut host_baseline = Vec::new();
    for attempt in 1..=3 {
        let (status, _, _, elapsed) = run_isolated(
            executable,
            &["--version"],
            &root,
            &scope,
            &format!("baseline-{attempt}"),
        );
        assert!(status.success());
        host_baseline.push(elapsed);
    }
    let mut timings = Vec::new();
    for attempt in 1..=3 {
        let (status, stdout, stderr, elapsed) = run_isolated(
            executable,
            &[],
            &root,
            &scope,
            &format!("attempt-{attempt}"),
        );
        timings.push(elapsed);
        assert_eq!(status.code(), Some(2));
        assert!(stdout.is_empty());
        assert!(String::from_utf8_lossy(&stderr).contains("当前不是交互终端，请使用具体子命令"));
        assert!(
            !root.exists(),
            "the TTY fast path must not discover or start a Controller"
        );
    }
    eprintln!("warm host baseline: {host_baseline:?}; non-TTY timings: {timings:?}");
    let baseline_max = *host_baseline.iter().max().unwrap();
    let non_tty_max = *timings.iter().max().unwrap();
    if baseline_max < Duration::from_millis(750) {
        assert!(
            non_tty_max < Duration::from_secs(1),
            "three warm non-TTY timings must stay below one second: {timings:?}"
        );
    } else {
        // Endpoint protection on the Windows test host can hold every debug
        // child, including `--version`, for about one second after main exits.
        // Calibrate only that external floor; the TUI fast path itself gets a
        // strict 250 ms incremental budget and still must leave no Controller state.
        assert!(
            non_tty_max <= baseline_max + Duration::from_millis(250),
            "non-TTY routing added too much work above host process latency: baseline={host_baseline:?}, non_tty={timings:?}"
        );
    }
    drop(cleanup);
    assert!(
        !root.exists(),
        "temporary non-TTY directory must be removed"
    );
}

#[test]
fn json_without_a_subcommand_never_reaches_tty_or_controller_initialization() {
    let executable = env!("CARGO_BIN_EXE_pinvou");
    let (root, scope) = isolated_root("json-no-command");
    let cleanup = TestDirectory::absent(root.clone());
    let arguments = ["--output", "json"];
    let (warm, _, _, _) = run_isolated(executable, &arguments, &root, &scope, "warm-json");
    assert_eq!(warm.code(), Some(2));
    let (status, stdout, stderr, elapsed) =
        run_isolated(executable, &arguments, &root, &scope, "json");
    eprintln!("warm JSON-without-command timing: {elapsed:?}");
    assert_eq!(status.code(), Some(2));
    assert!(stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&stderr).contains("JSON output requires an explicit subcommand")
    );
    assert!(!root.exists());
    drop(cleanup);
    assert!(!root.exists(), "temporary JSON directory must be removed");
}

#[test]
fn no_arguments_enter_and_restore_a_real_pseudoterminal() {
    let unique = format!(
        "pinvou-tui-pty-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = temporary_parent().join(&unique);
    let cleanup = TestDirectory::create(root.clone());
    std::fs::create_dir_all(root.join("runtime/pinvou")).unwrap();
    let paths = controller_paths(&root, &unique);
    let server = FakeControllerServer::start(paths.endpoint().clone()).unwrap();

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let sentinel = format!("PINVOU_PRIMARY_SENTINEL_{unique}");
    let mut command = primary_shell_command(&root, &sentinel, env!("CARGO_BIN_EXE_pinvou"));
    command.env("LOCALAPPDATA", &root);
    command.env("HOME", &root);
    command.env("XDG_DATA_HOME", root.join("data"));
    command.env("XDG_RUNTIME_DIR", root.join("runtime"));
    command.env("PINVOU_CONTROLLER_SESSION_SCOPE_FOR_TEST", &unique);
    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().unwrap();
    let mut reader = BoundedPtyReader::start(reader);
    let mut writer = pair.master.take_writer().unwrap();
    let mut output = Vec::new();
    let mut answered_cursor_query = false;
    let enter_deadline = Instant::now() + Duration::from_secs(5);
    while !contains(&output, b"\x1b[?2004h") {
        if Instant::now() >= enter_deadline {
            let _ = child.kill();
            panic!(
                "pinvou TUI did not enter the terminal; output={:?}",
                String::from_utf8_lossy(&output)
            );
        }
        if let Ok(chunk) = reader.recv_timeout(Duration::from_millis(50)) {
            output.extend_from_slice(&chunk);
            if !answered_cursor_query && contains(&output, b"\x1b[6n") {
                writer.write_all(b"\x1b[1;1R").unwrap();
                writer.flush().unwrap();
                answered_cursor_query = true;
            }
        }
    }
    writer.write_all(&[3]).unwrap();
    writer.flush().unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut timed_out = false;
    let status = loop {
        while let Ok(chunk) = reader.try_recv() {
            output.extend_from_slice(&chunk);
        }
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            timed_out = true;
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    drop(child);
    drop(writer);
    drop(pair.master);
    reader.finish(&mut output, Duration::from_secs(1)).unwrap();
    server.shutdown().unwrap();
    drop(cleanup);
    assert!(!root.exists(), "temporary PTY directory must be removed");

    assert!(
        !timed_out,
        "pinvou TUI did not exit after Ctrl+C; output={:?}",
        String::from_utf8_lossy(&output)
    );
    assert!(
        status.as_ref().is_some_and(|status| status.success()),
        "PTY child exit: {status:?}; output={:?}",
        String::from_utf8_lossy(&output)
    );
    #[cfg(not(windows))]
    assert_sequence(
        &output,
        &[
            b"\x1b[?1049h",
            b"\x1b[?25l",
            b"\x1b[?2004h",
            b"Pinvou Agent",
            b"\x1b[?2004l",
            b"\x1b[?25h",
            b"\x1b[?1049l",
        ],
    );
    // Windows ConPTY consumes alternate-screen enter/leave and projects them
    // as screen-buffer updates, so their original 1049 bytes are not visible
    // to the master. Bracketed paste and cursor restoration remain observable.
    #[cfg(windows)]
    assert_sequence(
        &output,
        &[
            b"\x1b[?2004h",
            b"Pinvou Agent",
            b"\x1b[?2004l",
            b"\x1b[?25h",
        ],
    );
    #[cfg(windows)]
    {
        let disable_paste = find_from(&output, b"\x1b[?2004l", 0).unwrap();
        let first_sentinel = find_from(&output, sentinel.as_bytes(), 0).unwrap();
        let restored_sentinel = find_from(
            &output,
            sentinel.as_bytes(),
            first_sentinel + sentinel.len(),
        )
        .expect("leaving the real alternate screen must restore the primary-buffer sentinel");
        assert!(
            restored_sentinel > disable_paste,
            "primary buffer must be restored during TUI teardown"
        );
        eprintln!(
            "ConPTY restoration evidence: disable_paste={disable_paste}, restored_sentinel={restored_sentinel}"
        );
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn find_from(haystack: &[u8], needle: &[u8], offset: usize) -> Option<usize> {
    haystack[offset..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|position| offset + position)
}

fn isolated_root(label: &str) -> (std::path::PathBuf, String) {
    let unique = format!(
        "pinvou-tui-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    (temporary_parent().join(&unique), unique)
}

fn isolated_command(executable: &str, root: &std::path::Path, scope: &str) -> Command {
    let mut command = Command::new(executable);
    command
        .env("LOCALAPPDATA", root)
        .env("HOME", root)
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("PINVOU_CONTROLLER_SESSION_SCOPE_FOR_TEST", scope);
    command
}

fn run_isolated(
    executable: &str,
    arguments: &[&str],
    root: &std::path::Path,
    scope: &str,
    label: &str,
) -> (std::process::ExitStatus, Vec<u8>, Vec<u8>, Duration) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let output_root = temporary_parent();
    let stdout_path = output_root.join(format!("pinvou-{label}-{nonce}.stdout"));
    let stderr_path = output_root.join(format!("pinvou-{label}-{nonce}.stderr"));
    let output_files = OutputFiles::new(stdout_path.clone(), stderr_path.clone());
    let stdout_file = std::fs::File::create(&stdout_path).unwrap();
    let stderr_file = std::fs::File::create(&stderr_path).unwrap();
    let mut command = isolated_command(executable, root, scope);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    let started = Instant::now();
    let status = command.status().unwrap();
    let elapsed = started.elapsed();
    let stdout = std::fs::read(&stdout_path).unwrap();
    let stderr = std::fs::read(&stderr_path).unwrap();
    drop(output_files);
    (status, stdout, stderr, elapsed)
}

#[cfg(windows)]
fn temporary_parent() -> PathBuf {
    PathBuf::from(r"D:\pinvou-temp")
}

#[cfg(unix)]
fn temporary_parent() -> PathBuf {
    std::env::temp_dir()
}

fn assert_sequence(output: &[u8], expected: &[&[u8]]) {
    let mut offset = 0;
    for needle in expected {
        let relative = output[offset..]
            .windows(needle.len())
            .position(|window| window == *needle)
            .unwrap_or_else(|| {
                panic!(
                    "missing {:?} after offset {offset}; output={:?}",
                    String::from_utf8_lossy(needle),
                    String::from_utf8_lossy(output)
                )
            });
        offset += relative + needle.len();
    }
}

fn controller_paths(root: &Path, scope: &str) -> ControllerPaths {
    let platform = HostPlatform::current().unwrap();
    let data_root = if matches!(platform, HostPlatform::Windows) {
        root.join("pinvou")
    } else {
        root.join("data/pinvou")
    };
    ControllerPaths::from_roots(platform, data_root, root.join("runtime"), scope).unwrap()
}

#[cfg(windows)]
fn primary_shell_command(root: &Path, sentinel: &str, executable: &str) -> CommandBuilder {
    let helper = root.join("launch-pinvou.cmd");
    std::fs::write(
        &helper,
        "@echo %PINVOU_TEST_SENTINEL%\r\n@\"%PINVOU_TEST_EXE%\"\r\n",
    )
    .unwrap();
    let mut command = CommandBuilder::new("cmd.exe");
    command.args(["/d", "/q", "/c"]);
    command.arg(helper);
    command.env("PINVOU_TEST_SENTINEL", sentinel);
    command.env("PINVOU_TEST_EXE", executable);
    command
}

#[cfg(unix)]
fn primary_shell_command(_root: &Path, sentinel: &str, executable: &str) -> CommandBuilder {
    let mut command = CommandBuilder::new("/bin/sh");
    command.args([
        "-c",
        "printf '%s\\n' \"$PINVOU_TEST_SENTINEL\"; exec \"$PINVOU_TEST_EXE\"",
    ]);
    command.env("PINVOU_TEST_SENTINEL", sentinel);
    command.env("PINVOU_TEST_EXE", executable);
    command
}

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn create(path: PathBuf) -> Self {
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn absent(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

struct OutputFiles {
    stdout: PathBuf,
    stderr: PathBuf,
}

impl OutputFiles {
    fn new(stdout: PathBuf, stderr: PathBuf) -> Self {
        Self { stdout, stderr }
    }
}

impl Drop for OutputFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.stdout);
        let _ = std::fs::remove_file(&self.stderr);
    }
}

struct BoundedPtyReader {
    output: mpsc::Receiver<Vec<u8>>,
    done: mpsc::Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl BoundedPtyReader {
    fn start(mut reader: Box<dyn Read + Send>) -> Self {
        let (output_tx, output) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut buffer = [0_u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) if output_tx.send(buffer[..read].to_vec()).is_err() => break,
                    Ok(_) => {}
                }
            }
            let _ = done_tx.send(());
        });
        Self {
            output,
            done,
            thread: Some(thread),
        }
    }

    fn recv_timeout(&self, timeout: Duration) -> Result<Vec<u8>, mpsc::RecvTimeoutError> {
        self.output.recv_timeout(timeout)
    }

    fn try_recv(&self) -> Result<Vec<u8>, mpsc::TryRecvError> {
        self.output.try_recv()
    }

    fn finish(&mut self, output: &mut Vec<u8>, timeout: Duration) -> Result<(), String> {
        self.done
            .recv_timeout(timeout)
            .map_err(|_| "PTY reader did not finish within its deadline".to_owned())?;
        while let Ok(chunk) = self.output.try_recv() {
            output.extend_from_slice(&chunk);
        }
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| "PTY reader thread panicked".to_owned())?;
        }
        Ok(())
    }
}

struct FakeControllerServer {
    endpoint: LocalEndpoint,
    stop: Arc<AtomicBool>,
    done: mpsc::Receiver<Result<(), String>>,
    thread: Option<JoinHandle<()>>,
}

impl FakeControllerServer {
    fn start(endpoint: LocalEndpoint) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_endpoint = endpoint.clone();
        let (ready_tx, ready) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut listener = match LocalIpcListener::bind(&thread_endpoint) {
                Ok(listener) => {
                    let _ = ready_tx.send(Ok(()));
                    listener
                }
                Err(error) => {
                    let error = error.to_string();
                    let _ = ready_tx.send(Err(error.clone()));
                    let _ = done_tx.send(Err(error));
                    return;
                }
            };
            let session = match ControllerSession::new("tui-pty-controller") {
                Ok(session) => session,
                Err(error) => {
                    let _ = done_tx.send(Err(error.to_string()));
                    return;
                }
            };
            let result = loop {
                match listener.serve_one(&session) {
                    Ok(()) if thread_stop.load(Ordering::Acquire) => break Ok(()),
                    Ok(()) => {}
                    Err(_) if thread_stop.load(Ordering::Acquire) => break Ok(()),
                    Err(error) => break Err(error.to_string()),
                }
            };
            let _ = done_tx.send(result);
        });
        match ready.recv_timeout(Duration::from_secs(1)) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                thread.join().map_err(|_| {
                    "fake Controller thread panicked while binding the endpoint".to_owned()
                })?;
                return Err(error);
            }
            Err(_) => {
                drop(thread);
                return Err("fake Controller did not bind within its deadline".to_owned());
            }
        }
        Ok(Self {
            endpoint,
            stop,
            done,
            thread: Some(thread),
        })
    }

    fn shutdown(mut self) -> Result<(), String> {
        self.stop_and_join()
    }

    fn stop_and_join(&mut self) -> Result<(), String> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        self.stop.store(true, Ordering::Release);
        wake_listener(&self.endpoint)?;
        let result = match self.done.recv_timeout(Duration::from_secs(1)) {
            Ok(result) => result,
            Err(_) => {
                // Do not turn a failed test helper into an unbounded test hang.
                drop(thread);
                return Err("fake Controller did not stop within its deadline".to_owned());
            }
        };
        thread
            .join()
            .map_err(|_| "fake Controller thread panicked".to_owned())?;
        result
    }
}

impl Drop for FakeControllerServer {
    fn drop(&mut self) {
        let _ = self.stop_and_join();
    }
}

#[cfg(windows)]
fn wake_listener(endpoint: &LocalEndpoint) -> Result<(), String> {
    let LocalEndpoint::WindowsPipe(name) = endpoint else {
        return Err("expected an isolated Windows pipe".to_owned());
    };
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(name)
        .map(|_| ())
        .map_err(|error| format!("failed to wake isolated Controller pipe: {error}"))
}

#[cfg(unix)]
fn wake_listener(endpoint: &LocalEndpoint) -> Result<(), String> {
    let LocalEndpoint::UnixSocket(path) = endpoint else {
        return Err("expected an isolated Unix socket".to_owned());
    };
    std::os::unix::net::UnixStream::connect(path)
        .map(|_| ())
        .map_err(|error| format!("failed to wake isolated Controller socket: {error}"))
}
