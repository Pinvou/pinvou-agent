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
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_pinvou"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("当前不是交互终端，请使用具体子命令"));
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
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_pinvou"));
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
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
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
