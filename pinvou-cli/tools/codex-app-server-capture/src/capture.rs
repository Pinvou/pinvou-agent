use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

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
    R: Read,
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
    R: Read,
    W: Write + Send + 'static,
{
    let output = File::create(&config.output)
        .with_context(|| format!("failed to create capture {}", config.output.display()))?;
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

    let stdout_recorder = Arc::clone(&recorder);
    let stdout_thread = thread::spawn(move || {
        read_lines(stdout, |line| {
            stdout_recorder
                .lock()
                .map_err(|_| anyhow::anyhow!("capture recorder lock poisoned"))?
                .record(CaptureChannel::ServerToClient, line)?;
            let frame: serde_json::Value = serde_json::from_str(line)
                .with_context(|| "app-server stdout contained a non-JSON protocol line")?;
            if !frame.is_object() {
                bail!("app-server stdout protocol frame must be a JSON object");
            }
            if forward_stdout {
                writeln!(server_writer, "{line}")?;
                server_writer.flush()?;
            }
            Ok(())
        })
    });

    let stderr_recorder = Arc::clone(&recorder);
    let stderr_thread = thread::spawn(move || {
        read_lines(stderr, |line| {
            stderr_recorder
                .lock()
                .map_err(|_| anyhow::anyhow!("capture recorder lock poisoned"))?
                .record(CaptureChannel::Stderr, line)
        })
    });

    let client_result = {
        let mut stdin = child.stdin.take().context("child stdin was not piped")?;
        read_lines(client_reader, |frame| {
            if frame.trim().is_empty() {
                return Ok(());
            }
            let value: serde_json::Value = serde_json::from_str(frame)
                .with_context(|| "client stdin contained a non-JSON protocol line")?;
            if !value.is_object() {
                bail!("client stdin protocol frame must be a JSON object");
            }
            let mut recorder = recorder
                .lock()
                .map_err(|_| anyhow::anyhow!("capture recorder lock poisoned"))?;
            recorder.record(CaptureChannel::ClientToServer, &frame)?;
            writeln!(stdin, "{frame}").context("failed to write app-server stdin")?;
            stdin.flush().context("failed to flush app-server stdin")?;
            Ok(())
        })
    };

    if client_result.is_err() {
        let _ = child.kill();
    }

    let status = child.wait().context("failed waiting for app-server")?;
    stdout_thread
        .join()
        .map_err(|_| anyhow::anyhow!("stdout capture thread panicked"))??;
    stderr_thread
        .join()
        .map_err(|_| anyhow::anyhow!("stderr capture thread panicked"))??;
    client_result?;
    if !status.success() {
        bail!("app-server exited with status {status}");
    }
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
        if line.contains(['\r', '\n']) {
            bail!("capture payload must be a single line");
        }
        serde_json::to_writer(
            &mut self.writer,
            &CaptureRecord {
                monotonic_ns: (self.clock)(),
                channel,
                line: line.to_owned(),
            },
        )?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }
}
