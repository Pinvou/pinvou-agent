//! pinvou3 — 本地 AI 平台。
//!
//! 默认启动 Web UI；`--coding` 旁路进入 DeepSeek-TUI 编码模式。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use clap::Parser;

use pinvou_platform::engine_factory::{create_engine, load_agents};

#[derive(Parser, Debug)]
#[command(
    name = "pinvou-platform",
    version = env!("CARGO_PKG_VERSION"),
    about = "pinvou3 — 本地 AI 平台"
)]
struct Cli {
    /// Agent prompt 目录（每个 agent = 一个 markdown 文件）
    #[arg(long, default_value = "./prompts")]
    prompts_dir: PathBuf,

    /// 以 coding 模式直接启动（旁路 DeepSeek-TUI 编码界面）
    #[arg(long)]
    coding: bool,

    /// 指定 Web 端口（默认 9876）
    #[arg(long, default_value = "9876")]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.coding {
        return launch_coding_mode();
    }

    let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut engine = create_engine(workspace)?;

    let agents = load_agents(&cli.prompts_dir);
    engine.set_agent_registry(agents);

    pinvou_platform::web::serve(engine, cli.port).await?;
    Ok(())
}

fn launch_coding_mode() -> Result<()> {
    let own_exe = std::env::current_exe()?;
    let own_dir = own_exe.parent().unwrap_or_else(|| Path::new("."));
    let coding_bin = own_dir.join("deepseek-tui");

    if coding_bin.exists() {
        exec_binary(&coding_bin)?;
        return Ok(());
    }

    eprintln!("→ 通过 cargo 启动 coding 模式...");
    let status = Command::new("cargo")
        .args(["run", "--bin", "deepseek-tui"])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("无法启动 coding 模式")?;

    if !status.success() {
        anyhow::bail!("coding 模式启动失败 (exit {:?})", status.code());
    }
    Ok(())
}

fn exec_binary(path: &Path) -> Result<()> {
    let mut child = Command::new(path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("无法执行: {}", path.display()))?;

    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("coding 模式退出 (exit {:?})", status.code());
    }
    Ok(())
}
