use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use pinvou_knowledge::backup::{self, RestoreMode};
use pinvou_knowledge::client::KnowledgeClient;
use pinvou_knowledge::server;
use pinvou_knowledge::KnowledgeService;

#[derive(Debug, Parser)]
#[command(
    name = "pinvou-knowledge-server",
    version,
    about = "Pinvou 自包含共享知识库服务"
)]
struct Args {
    #[arg(long, env = "PINVOU_KNOWLEDGE_BIND", default_value = "0.0.0.0:3210")]
    bind: SocketAddr,

    #[arg(
        long,
        env = "PINVOU_KNOWLEDGE_DATA_DIR",
        default_value = "./pinvou-knowledge-data"
    )]
    data_dir: PathBuf,

    #[arg(long, env = "PINVOU_KNOWLEDGE_MODEL_DIR")]
    model_dir: Option<PathBuf>,

    #[arg(long, hide = true)]
    health_check: Option<String>,

    #[arg(long, hide = true)]
    recover_host_owner_claim: Option<PathBuf>,

    #[arg(long, hide = true, requires = "host_owner_scope")]
    host_owner_device: Option<String>,

    #[arg(long, hide = true, value_parser = ["owner", "manage"], requires = "host_owner_device")]
    host_owner_scope: Option<String>,

    #[arg(long, hide = true)]
    backup_output: Option<PathBuf>,

    #[arg(long, hide = true, requires = "backup_output")]
    backup_recipient: Vec<String>,

    #[arg(long, hide = true)]
    restore_input: Option<PathBuf>,

    #[arg(long, hide = true, requires = "restore_input")]
    restore_identity_file: Option<PathBuf>,

    #[arg(
        long,
        hide = true,
        value_parser = ["same-host", "content-only"],
        requires = "restore_input"
    )]
    restore_mode: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if let Some(endpoint) = &args.health_check {
        KnowledgeClient::local_health_untrusted(endpoint)
            .await
            .map_err(anyhow::Error::msg)?;
        return Ok(());
    }
    if let Some(claim_path) = &args.recover_host_owner_claim {
        let boot = KnowledgeService::boot(args.data_dir.clone(), args.model_dir.clone())
            .map_err(anyhow::Error::msg)?;
        let server_id = boot
            .service
            .server_info()
            .map_err(anyhow::Error::msg)?
            .server_id;
        let claim = std::cell::RefCell::new(None);
        boot.service
            .recover_host_owner("Host PINVOU", |device_id, token| {
                let value = serde_json::json!({
                    "serverId": server_id,
                    "deviceId": device_id,
                    "token": token,
                });
                write_host_owner_claim(claim_path, &value).map_err(|error| error.to_string())?;
                claim.replace(Some(value));
                Ok(())
            })
            .map_err(anyhow::Error::msg)?;
        serde_json::to_writer(
            std::io::stdout(),
            &claim
                .into_inner()
                .ok_or_else(|| anyhow::anyhow!("恢复本机所有者失败"))?,
        )?;
        return Ok(());
    }
    if let (Some(device_id), Some(scope)) = (&args.host_owner_device, &args.host_owner_scope) {
        let grant = KnowledgeService::set_owner_device_in_data_dir(
            &args.data_dir,
            device_id,
            scope == "owner",
        )
        .map_err(anyhow::Error::msg)?;
        serde_json::to_writer(std::io::stdout(), &grant)?;
        return Ok(());
    }
    if let Some(output) = &args.backup_output {
        if args.restore_input.is_some() || args.backup_recipient.is_empty() {
            anyhow::bail!("备份参数无效");
        }
        let _data_dir_lock = pinvou_knowledge::try_lock_knowledge_data_dir(&args.data_dir)
            .map_err(anyhow::Error::msg)?;
        let manifest =
            backup::create_encrypted_backup(&args.data_dir, output, &args.backup_recipient)
                .map_err(anyhow::Error::msg)?;
        serde_json::to_writer(std::io::stdout(), &manifest)?;
        return Ok(());
    }
    if let Some(input) = &args.restore_input {
        if args.backup_output.is_some() {
            anyhow::bail!("恢复参数无效");
        }
        let identity_file = args
            .restore_identity_file
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("恢复缺少解密密钥"))?;
        let identity = std::fs::read_to_string(identity_file)?;
        let mode = match args.restore_mode.as_deref() {
            Some("same-host") => RestoreMode::SameHost,
            Some("content-only") => RestoreMode::ContentOnly,
            _ => anyhow::bail!("恢复模式无效"),
        };
        let _data_dir_lock = pinvou_knowledge::try_lock_knowledge_data_dir(&args.data_dir)
            .map_err(anyhow::Error::msg)?;
        let manifest = backup::restore_encrypted_backup(&args.data_dir, input, &identity, mode)
            .map_err(anyhow::Error::msg)?;
        serde_json::to_writer(std::io::stdout(), &manifest)?;
        return Ok(());
    }
    let boot = KnowledgeService::boot(args.data_dir.clone(), args.model_dir)
        .map_err(anyhow::Error::msg)?;
    let server_id = boot
        .service
        .server_info()
        .map_err(anyhow::Error::msg)?
        .server_id;
    boot.service
        .provision_host_owner("Host PINVOU", |device_id, token| {
            write_host_owner_claim(
                &args.data_dir.join("host-owner.claim"),
                &serde_json::json!({
                    "serverId": server_id,
                    "deviceId": device_id,
                    "token": token,
                }),
            )
            .map_err(|error| error.to_string())
        })
        .map_err(anyhow::Error::msg)?;
    eprintln!("PINVOU Knowledge data: {}", args.data_dir.display());
    eprintln!("PINVOU Knowledge listening on https://{}", args.bind);
    let service = boot.service;
    {
        let background = service.clone();
        tokio::spawn(async move {
            let _ = background.backfill_vector_signature_index().await;
        });
    }
    if service.ready() {
        let background = service.clone();
        tokio::spawn(async move {
            let _ = background.index_pending_documents().await;
        });
    }
    {
        let background = service.clone();
        tokio::spawn(async move {
            background.run_trash_retention_loop().await;
        });
    }
    server::serve(service, args.bind)
        .await
        .map_err(anyhow::Error::msg)
}

fn write_host_owner_claim(path: &std::path::Path, claim: &serde_json::Value) -> anyhow::Result<()> {
    let temporary = path.with_extension("claim.tmp");
    match std::fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    serde_json::to_writer(&mut file, claim)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    Ok(())
}
