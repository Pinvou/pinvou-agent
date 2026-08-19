use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use codex_app_server_capture::capture::{
    CaptureConfig, CommandSpec, ProxyConfig, run_capture, run_proxy,
};
use codex_app_server_capture::s2::{S2Evidence, validate};

#[derive(Debug, Parser)]
#[command(about = "Capture codex app-server JSONL traffic without mixing in stderr")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Transparent stdin/stdout proxy for adaptive scenarios such as approval and interrupt.
    Proxy {
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        executable: Option<OsString>,
    },
    /// Replay client JSON objects from a JSONL file, capturing all three channels.
    Replay {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        executable: Option<OsString>,
    },
    /// Validate S2 evidence with fail-closed F1-F3 gates.
    ValidateS2 {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Commands::Proxy { output, executable } => run_proxy(
            ProxyConfig {
                output,
                command: CommandSpec::codex(executable),
            },
            std::io::stdin(),
            std::io::stdout(),
        ),
        Commands::Replay {
            input,
            output,
            executable,
        } => run_capture(CaptureConfig {
            input,
            output,
            command: CommandSpec::codex(executable),
        }),
        Commands::ValidateS2 { input, output } => {
            let evidence: S2Evidence = serde_json::from_slice(
                &std::fs::read(&input)
                    .with_context(|| format!("failed to read evidence {}", input.display()))?,
            )
            .with_context(|| format!("invalid S2 evidence JSON in {}", input.display()))?;
            let report = validate(evidence);
            let rendered = serde_json::to_string_pretty(&report)?;
            if let Some(output) = output {
                std::fs::write(&output, format!("{rendered}\n"))
                    .with_context(|| format!("failed to write report {}", output.display()))?;
            } else {
                println!("{rendered}");
            }
            if !report.valid {
                bail!("S2 run is INVALID; PASS outputs and baseline update are suppressed");
            }
            Ok(())
        }
    }
}
