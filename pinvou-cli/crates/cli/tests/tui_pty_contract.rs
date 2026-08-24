#![cfg(feature = "distributed")]

use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use pinvou_controller::{ControllerPaths, ControllerSession, LocalIpcListener};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

#[test]
fn no_arguments_on_a_pipe_fail_fast_without_starting_controller() {
    let executable = env!("CARGO_BIN_EXE_pinvou");
    let (root, scope) = isolated_root("non-tty");
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
}

#[test]
fn json_without_a_subcommand_never_reaches_tty_or_controller_initialization() {
    let executable = env!("CARGO_BIN_EXE_pinvou");
    let (root, scope) = isolated_root("json-no-command");
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
    let root = std::path::PathBuf::from(r"D:\pinvou-pty-contract").join(&unique);
    std::fs::create_dir_all(root.join("runtime")).unwrap();

    let previous = install_test_paths(&root, &unique);
    let paths = ControllerPaths::discover().unwrap();
    restore_test_paths(previous);
    let (ready_tx, ready_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut listener = LocalIpcListener::bind(paths.endpoint()).unwrap();
        ready_tx.send(()).unwrap();
        let session = ControllerSession::new("tui-pty-controller").unwrap();
        listener.serve_one(&session).unwrap();
        listener.serve_one(&session).unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let sentinel = format!("PINVOU_PRIMARY_SENTINEL_{unique}");
    let mut command = CommandBuilder::new("cmd.exe");
    command.args(["/d", "/q", "/c"]);
    command.arg(format!(
        "echo {sentinel} & {}",
        env!("CARGO_BIN_EXE_pinvou")
    ));
    command.env("LOCALAPPDATA", &root);
    command.env("HOME", &root);
    command.env("XDG_DATA_HOME", root.join("data"));
    command.env("XDG_RUNTIME_DIR", root.join("runtime"));
    command.env("PINVOU_CONTROLLER_SESSION_SCOPE_FOR_TEST", &unique);
    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let (output_tx, output_rx) = mpsc::channel();
    let reader_thread = std::thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) if output_tx.send(buffer[..read].to_vec()).is_err() => break,
                Ok(_) => {}
            }
        }
    });
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
        if let Ok(chunk) = output_rx.recv_timeout(Duration::from_millis(50)) {
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
        while let Ok(chunk) = output_rx.try_recv() {
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
    while let Ok(chunk) = output_rx.recv_timeout(Duration::from_millis(100)) {
        output.extend_from_slice(&chunk);
    }
    reader_thread.join().unwrap();
    server.join().unwrap();
    std::fs::remove_dir_all(&root).unwrap();

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
    (
        std::path::PathBuf::from(r"D:\pinvou-pty-contract").join(&unique),
        unique,
    )
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
    let output_root = std::path::Path::new(r"D:\pinvou-temp");
    let stdout_path = output_root.join(format!("pinvou-{label}-{nonce}.stdout"));
    let stderr_path = output_root.join(format!("pinvou-{label}-{nonce}.stderr"));
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
    std::fs::remove_file(stdout_path).unwrap();
    std::fs::remove_file(stderr_path).unwrap();
    (status, stdout, stderr, elapsed)
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

type PreviousPaths = Vec<(&'static str, Option<std::ffi::OsString>)>;

fn install_test_paths(root: &std::path::Path, scope: &str) -> PreviousPaths {
    let settings = [
        ("LOCALAPPDATA", root.as_os_str().to_owned()),
        ("HOME", root.as_os_str().to_owned()),
        ("XDG_DATA_HOME", root.join("data").into_os_string()),
        ("XDG_RUNTIME_DIR", root.join("runtime").into_os_string()),
        ("PINVOU_CONTROLLER_SESSION_SCOPE_FOR_TEST", scope.into()),
    ];
    settings
        .into_iter()
        .map(|(name, value)| {
            let previous = std::env::var_os(name);
            unsafe { std::env::set_var(name, value) };
            (name, previous)
        })
        .collect()
}

fn restore_test_paths(previous: PreviousPaths) {
    for (name, value) in previous {
        match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }
}
