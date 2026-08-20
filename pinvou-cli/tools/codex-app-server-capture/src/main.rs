use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use codex_app_server_capture::capture::{
    CaptureConfig, CommandSpec, ProxyConfig, run_capture, run_proxy,
};
use codex_app_server_capture::runner::{S2RunConfig, run_marker_helper, run_s2};
use codex_app_server_capture::s2::{S2Evidence, validate};

#[derive(Debug, Parser)]
#[command(about = "Capture codex app-server JSONL traffic without mixing in stderr")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(name = "__marker-helper", hide = true)]
    MarkerHelper,
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
    /// Execute the real A-D S2 scenarios and write capture/evidence/report artifacts.
    RunS2 {
        #[arg(long)]
        output_dir: Option<PathBuf>,
        #[arg(long)]
        executable: Option<OsString>,
        #[arg(long)]
        trusted_approval_wrapper: Option<PathBuf>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, default_value_t = 120_000)]
        scenario_timeout_ms: u64,
        #[arg(long, default_value_t = 600_000)]
        global_timeout_ms: u64,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Commands::MarkerHelper => run_marker_helper(),
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
        Commands::RunS2 {
            output_dir,
            executable,
            trusted_approval_wrapper,
            model,
            scenario_timeout_ms,
            global_timeout_ms,
        } => {
            let outcome = run_s2(S2RunConfig {
                output_dir,
                executable,
                trusted_approval_wrapper,
                model,
                scenario_timeout: std::time::Duration::from_millis(scenario_timeout_ms),
                global_timeout: std::time::Duration::from_millis(global_timeout_ms),
                #[cfg(debug_assertions)]
                test_child_env: Vec::new(),
            })?;
            println!("S2 PASS: {}", outcome.output_dir.display());
            Ok(())
        }
    }
}
